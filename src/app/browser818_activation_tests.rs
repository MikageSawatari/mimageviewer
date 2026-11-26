//! Real activation receipt -> live folder hydration -> linked/ordinary display -> close.
//! The pre-fix characterization lives in target/browser818-repro/repro_tests.rs.
//! Only the immutable page-index reply is gated; the production loader creates
//! the continuation and deferred fullscreen owner. No native window is launched.

#![cfg(windows)]

use super::*;
use crate::app::file_type_resume::{LegacyResumeSource, PreparedPageIdentityIndex};
use crate::app::tests::phase_c_support::setup_app;
use std::path::Path;
use std::time::{Duration, Instant};

fn image_index(app: &App, path: &Path) -> usize {
    app.items
        .iter()
        .position(|item| matches!(item, GridItem::Image(actual) if actual == path))
        .expect("the real directory loader must include this fixture image")
}

fn assert_image_at(app: &App, idx: Option<usize>, path: &Path) {
    let idx = idx.expect("an image index must be present");
    assert!(
        matches!(app.items.get(idx), Some(GridItem::Image(actual)) if actual == path),
        "wrong image identity at index {idx}: expected {}",
        path.display(),
    );
}

fn exercise_activation_with_history(linked: bool) {
    let _input = crate::key_input::lock_test_input();
    let mut app = setup_app();
    let ctx = egui::Context::default();
    app.initialized = true;
    app.active_quick_folder_slot = None;
    app.settings.detached_viewer_enabled = false;
    app.settings.detached_viewer_open_images_in_window = false;
    app.settings.auto_fullscreen_image_folders = false;
    app.settings.auto_fullscreen_zip_pdf = false;
    // Prove the folder-history branch, rather than the "restore disabled -> first"
    // branch. Names also put the requested file BEFORE the saved history image.
    app.settings.restore_folder_cursor_on_revisit = true;
    app.settings.sort_order = crate::settings::SortOrder::FileName;

    let target = app.tmp.path().join("target-with-history");
    let origin = app.tmp.path().join("already-open-folder");
    std::fs::create_dir(&target).unwrap();
    std::fs::create_dir(&origin).unwrap();
    let requested = target.join("a-requested.png");
    let historical = target.join("b-history.png");
    let previous = origin.join("previous.png");
    for path in [&requested, &historical, &previous] {
        image::RgbaImage::new(2, 2).save(path).unwrap();
    }

    // Establish the saved cursor by a real visit and departure, not by inserting
    // a fake DeferredFullscreen or invoking the loaded-file helper directly.
    app.load_folder_and_finish_for_test(target.clone());
    app.finish_page_identity_preparation_for_test();
    let saved_idx = image_index(&app, &historical);
    assert_ne!(saved_idx, image_index(&app, &requested));
    app.move_grid_cursor(Some(saved_idx), GridCursorIntent::Automatic);
    app.grid_scroll.set_position(123.0);
    app.load_folder_and_finish_for_test(origin.clone());
    app.finish_page_identity_preparation_for_test();
    let saved_history = app.folder_history.get(&target).copied().unwrap();
    assert_eq!(saved_history.1, Some(saved_idx));
    assert_eq!(saved_history.0, 123.0);

    let previous_idx = image_index(&app, &previous);
    app.move_grid_cursor(Some(previous_idx), GridCursorIntent::Automatic);
    app.settings.detached_viewer_enabled = linked;
    app.open_fullscreen(previous_idx, HistoryTrigger::UserChosen);
    assert_image_at(&app, app.fullscreen_idx, &previous);
    assert_eq!(app.viewer_session_is_detached(), linked);
    if linked {
        app.detached_viewer_window_id()
            .expect("an existing linked F12 window");
        assert!(!app.detached_viewer_independent_still_session());
    }
    assert!(
        !app.sidecar_restore_active(),
        "receive activation before any restore gate"
    );

    // Ordinary second-instance open-path receipt, exactly the listener's channel
    // entry. No BrowserOverlay intent or direct call to an open helper is used.
    app.activation_open_path_tx
        .send(crate::single_instance::ActivationOpenPath::received(
            requested.clone(),
            app.remote_session_handle(),
        ))
        .unwrap();
    app.poll_activation_open_paths(&ctx);
    let pending = app.startup_open_path_resolve_pending.as_ref().unwrap();
    assert_eq!(pending.owner.source(), StartupOpenPathSource::Activation);
    assert_eq!(pending.requested.as_path(), requested.as_path());
    let request = pending
        .owner
        .viewer_open_adoption()
        .request()
        .expect("received open identity");
    assert_eq!(
        request.intent,
        crate::app::external_open::ViewerOpenIntent::Preferences
    );
    let deadline = Instant::now() + Duration::from_secs(15);
    while app.startup_open_path_resolve_pending.is_some() {
        assert!(
            Instant::now() < deadline,
            "real activation resolver did not finish"
        );
        app.poll_startup_open_path_resolve(&ctx);
        std::thread::yield_now();
    }

    // Do not poll the sidecar owner until its real completion receiver has been
    // gated. Even an already-completed worker cannot race the checkpoint below.
    let requested_idx = image_index(&app, &requested);
    let historical_idx = image_index(&app, &historical);
    assert_eq!(historical_idx, saved_idx, "unchanged saved item order");
    assert_ne!(requested_idx, historical_idx);
    assert_image_at(&app, app.selected, &requested);
    assert!(
        app.select_after_load.is_none(),
        "no competing name hint in this scenario"
    );
    assert!(
        app.post_operation_selection.is_none(),
        "no competing file operation"
    );
    assert_eq!(
        app.folder_history.get(&target).unwrap().1,
        Some(historical_idx)
    );

    // The normal loader created the live hydration/deferred-open owner. Only its
    // immutable page-index reply is held; keep its production `next` and book fence.
    let prepared = PreparedPageIdentityIndex::prepare_for_source(
        &app.items,
        app.archive_source_override.as_deref(),
        app.settings.file_type_visibility.clone(),
        LegacyResumeSource::from_recipe(&app.projection_recipe),
    );
    let context = app.sidecar_restore_projected_context();
    let generation = app.items_generation;
    let (release, held_rx) = mpsc::channel();
    {
        let state = app
            .sidecar_restore
            .as_mut()
            .expect("real activation hydration owner");
        assert_eq!(state.common.target_context, context);
        assert_eq!(state.common.items_generation, generation);
        assert_eq!(state.common.source_path.as_deref(), Some(target.as_path()));
        let ContinuationOwner::Live(continuation) = &state.common.continuation else {
            panic!("the normal folder load must own a live continuation");
        };
        assert!(!continuation.detached_physical);
        let deferred = state
            .common
            .deferred_fullscreen
            .as_ref()
            .expect("real deferred file open");
        assert_eq!(deferred.idx, requested_idx);
        assert_eq!(deferred.viewer_adoption.request(), Some(request));
        assert!(deferred.viewer_adoption.owns_file_selection());
        assert_eq!(
            deferred.item_key,
            GridItem::Image(requested.clone()).perf_key()
        );
        let old = std::mem::replace(&mut state.phase, Phase::Resuming);
        let Phase::PreparingPageIndex(preparing) = old else {
            panic!("hold the loader's actual page-index reply before its first poll");
        };
        state.phase = Phase::PreparingPageIndex(PreparingPageIndex {
            rx: held_rx,
            next: preparing.next,
        });
    }
    app.poll_sidecar_restore(&ctx);
    assert!(
        app.sidecar_restore_active(),
        "the controlled restore wait is still held"
    );
    assert_image_at(&app, app.selected, &requested);
    eprintln!(
        "[browser818-selection] linked={linked} held: selected=requested({requested_idx}), saved=history({historical_idx}), scroll={}",
        app.grid_scroll.offset(),
    );

    release.send(prepared).unwrap();
    app.finish_page_identity_preparation_for_test();
    assert!(!app.sidecar_restore_active());
    // The old code selected the history image here while displaying the requested
    // image. The same real continuation must now keep the requested cursor.
    assert_image_at(&app, app.fullscreen_idx, &requested);
    assert_image_at(&app, app.selected, &requested);
    assert_eq!(app.selected, app.fullscreen_idx);
    assert_eq!(
        app.grid_scroll.offset(),
        saved_history.0,
        "restore scroll independently of the saved cursor"
    );
    assert_eq!(app.viewer_session_is_detached(), linked);
    eprintln!(
        "[browser818-selection] linked={linked} restored: viewer=requested({requested_idx}), selected=requested({requested_idx}), saved_scroll={}, unpainted_scroll={}",
        saved_history.0,
        app.grid_scroll.offset(),
    );

    if linked {
        let window = app
            .detached_viewer_window_id()
            .expect("linked request publication");
        let stamp = app
            .last_viewer_sync_stamp
            .as_ref()
            .expect("requested image publication stamp");
        assert_eq!(stamp.idx, requested_idx);
        assert_eq!(stamp.items_generation, app.items_generation);
        let published_stamp = (*stamp).clone();
        // The old linked sync reopened the history image. Sync must now be a no-op
        // for both the displayed identity and the published request stamp.
        app.sync_detached_viewer_to_selected(&ctx);
        assert_image_at(&app, app.fullscreen_idx, &requested);
        assert_image_at(&app, app.selected, &requested);
        assert_eq!(app.last_viewer_sync_stamp.as_ref(), Some(&published_stamp));
        let stamp = app.last_viewer_sync_stamp.as_ref().unwrap();
        assert_eq!(stamp.idx, requested_idx);
        assert_eq!(stamp.items_generation, app.items_generation);
        assert_eq!(app.detached_viewer_window_id(), Some(window));
        app.close_fullscreen();
        assert!(app.fullscreen_idx.is_none());
        assert_image_at(&app, app.selected, &requested);
        assert_eq!(app.grid_scroll.offset(), saved_history.0);
        assert!(app.grid_scroll.reveal_requested());
        eprintln!("[browser818-selection] linked close: selected=requested({requested_idx})");
    } else {
        // Ordinary fullscreen does not follow the main-list selection.
        app.sync_detached_viewer_to_selected(&ctx);
        assert_image_at(&app, app.fullscreen_idx, &requested);
        assert_image_at(&app, app.selected, &requested);
        app.close_fullscreen();
        assert!(app.fullscreen_idx.is_none());
        // Ordinary close already returned to the displayed image before stage 2.
        // Keep that behavior and preserve the historical scroll/reveal policy.
        assert_image_at(&app, app.selected, &requested);
        assert!(app.grid_scroll.reveal_requested());
        assert_eq!(app.grid_scroll.offset(), saved_history.0);
        eprintln!(
            "[browser818-selection] ordinary close: selected=requested({requested_idx}), unpainted_scroll={}, reveal_requested=true",
            app.grid_scroll.offset(),
        );
    }
}

#[test]
fn browser818_selection_activation_linked_restore_sync_close_keeps_requested_image() {
    exercise_activation_with_history(true);
}

#[test]
fn browser818_selection_activation_ordinary_restore_close_keeps_requested_image() {
    exercise_activation_with_history(false);
}
