use super::*;

fn resolve_ready(app: &mut App, ctx: &egui::Context) {
    let ready = app.poll_folder_pane_open(ctx).expect("released completion");
    assert!(app.resolve_main_folder_open_ready(ctx, ready).is_none());
}

#[test]
fn sort_hold_838_stale_owner_drops_result_and_releases_gate() {
    let mut h = Harness::images();
    let held = request_sort(&mut h.env, SortOrder::DurationAsc);
    // Invalidate the actual MainListNavigation Row proof captured by the request.
    h.env.items_generation = h.env.items_generation.wrapping_add(1);
    assert!(
        !h.env.main_list_navigation_is_current(
            h.env
                .folder_pane_open_pending
                .as_ref()
                .unwrap()
                .navigation
                .as_ref()
                .unwrap()
        )
    );
    let generation = h.env.items_generation;
    let rows = h.env.items.clone();
    let selected = h.env.selected;
    held.release();
    // The navigation proof may retire the request before it even yields Ready.
    if let Some(ready) = h.env.poll_folder_pane_open(&h.ctx) {
        assert!(
            h.env
                .resolve_main_folder_open_ready(&h.ctx, ready)
                .is_none()
        );
    }
    assert!(h.env.folder_pane_open_pending.is_none());
    assert!(h.env.grid_item_input_allowed());
    assert_eq!(h.env.items_generation, generation);
    assert_eq!(h.env.items, rows);
    assert_eq!(h.env.selected, selected);
    assert!(!h.env.sidecar_restore_active());
}

#[test]
fn sort_hold_838_refresh_owner_mismatch_does_not_adopt() {
    // Reuse the existing RatingPhysical owner fixture, whose reload owner has
    // a real generation validator (plain Navigation intentionally has none).
    let mut app = super::super::mutation_refresh_rating_sort_fixture();
    let held = request_sort(&mut app, SortOrder::DurationAsc);
    held.release();
    let ctx = egui::Context::default();
    let ready = app
        .poll_folder_pane_open(&ctx)
        .expect("completed sort scan");
    let FolderOpenScanPurpose::CurrentViewOrderRefresh { reload_owner, .. } = &ready.purpose else {
        panic!("refresh owner")
    };
    assert!(matches!(
        reload_owner.as_ref(),
        OpenRequestOwner::RatingPhysical(_)
    ));
    app.items_generation = app.items_generation.wrapping_add(1);
    app.select_after_load = None;
    let rows = app.items.clone();
    let selected = app.selected;
    let generation = app.items_generation;
    assert!(!app.open_request_owner_is_current(&ready.path, reload_owner));
    assert!(app.resolve_main_folder_open_ready(&ctx, ready).is_none());
    assert!(app.grid_item_input_allowed());
    assert!(app.folder_pane_open_pending.is_none());
    assert!(!app.sidecar_restore_active());
    assert_eq!(app.items, rows);
    assert_eq!(app.items_generation, generation);
    assert_eq!(app.selected, selected);
    assert_eq!(app.select_after_load, None);
}

#[test]
fn sort_hold_838_scan_error_releases_gate_without_replacing_rows() {
    let mut h = Harness::images();
    let folder = h.env.current_folder.clone().unwrap();
    // Make the actual sort worker return NotFound; do not manufacture an error reply.
    for name in ["B.png", "C.png", "Z.wav"] {
        std::fs::remove_file(folder.join(name)).unwrap();
    }
    std::fs::remove_dir(&folder).unwrap();
    let rows = h.env.items.clone();
    let generation = h.env.items_generation;
    let held = request_sort(&mut h.env, SortOrder::DurationAsc);
    assert!(held.result.is_err());
    held.release();
    resolve_ready(&mut h.env, &h.ctx);
    assert!(h.env.folder_pane_open_pending.is_none());
    assert!(h.env.grid_item_input_allowed());
    assert_eq!(h.env.items, rows);
    assert_eq!(h.env.items_generation, generation);
    assert!(
        h.env.fs_feedback_toast.is_some(),
        "existing failure notification"
    );
}

#[test]
fn sort_hold_838_disconnected_worker_releases_gate_without_replacing_rows() {
    let mut h = Harness::images();
    let held = request_sort(&mut h.env, SortOrder::DurationAsc);
    let rows = h.env.items.clone();
    let generation = h.env.items_generation;
    drop(held); // Empty receive boundary disconnects, just as in the direct-scan test.
    assert!(h.env.poll_folder_pane_open(&h.ctx).is_none());
    assert!(h.env.folder_pane_open_pending.is_none());
    assert!(h.env.grid_item_input_allowed());
    assert_eq!(h.env.items, rows);
    assert_eq!(h.env.items_generation, generation);
}

#[test]
fn sort_hold_838_park_retires_scan_and_releases_gate() {
    let mut h = Harness::images();
    let held = request_sort(&mut h.env, SortOrder::DurationAsc);
    let rows = h.env.items.clone();
    let generation = h.env.items_generation;
    h.env.pause_mounted_background_work_keep_current_frame();
    held.assert_retired();
    assert!(h.env.folder_pane_open_pending.is_none());
    assert!(h.env.grid_item_input_allowed());
    assert_eq!(h.env.items, rows);
    assert_eq!(h.env.items_generation, generation);
}

fn independent_context(h: &mut Harness, window: u64) -> ViewerContextId {
    let folder = h.env.current_folder.clone().unwrap();
    let rows = h.env.items.clone();
    let metas = h.env.image_metas.clone();
    h.env.build_window_context_for_test(window, |app| {
        app.current_folder = Some(folder);
        app.items = rows;
        app.image_metas = metas;
        app.thumbnails = (0..3).map(|_| ThumbnailState::Pending).collect();
        app.visible_indices = vec![0, 1, 2];
        app.selected = Some(0);
        app.viewer_presentation = ViewerPresentation::DetachedWindow;
    })
}

#[test]
fn sort_hold_838_retire_drops_only_own_scan() {
    let mut h = Harness::images();
    let id = independent_context(&mut h, 183805);
    let held = h
        .env
        .with_viewer_context(id, |app| request_sort(app, SortOrder::DurationAsc))
        .unwrap();
    let rows = h.env.items.clone();
    let generation = h.env.items_generation;
    h.env
        .retire_context(id, "sort_hold_838_retire", |_| ())
        .unwrap();
    held.assert_retired();
    assert!(h.env.with_viewer_context(id, |_| ()).is_err());
    assert!(h.env.grid_item_input_allowed());
    assert!(h.env.folder_pane_open_pending.is_none());
    assert_eq!(h.env.items, rows);
    assert_eq!(h.env.items_generation, generation);
}

#[test]
fn sort_hold_838_sort_replacement_keeps_gate_until_latest_adoption() {
    let mut h = Harness::images();
    let a = request_sort(&mut h.env, SortOrder::DurationAsc);
    let blocked_a = !h.env.grid_item_input_allowed();
    let generation = h.env.items_generation;
    let rows = h.env.items.clone();
    let b = request_sort(&mut h.env, SortOrder::DurationDesc);
    a.assert_retired();
    let blocked_b = !h.env.grid_item_input_allowed();
    h.tick(vec![]);
    assert_eq!(h.env.items_generation, generation, "A cannot be adopted");
    assert_eq!(h.env.items, rows);
    let FolderOpenScanPurpose::CurrentViewOrderRefresh { order, .. } =
        &h.env.folder_pane_open_pending.as_ref().unwrap().purpose
    else {
        panic!("B owns wait")
    };
    assert_eq!(order.sort_order, SortOrder::DurationDesc);
    b.release();
    h.settle();
    assert!(h.env.items_generation > generation);
    assert!(h.env.grid_item_input_allowed());
    assert_eq!(h.env.settings.sort_order, SortOrder::DurationDesc);
    assert!(
        blocked_a && blocked_b,
        "A -> B must never release item input"
    );
}

#[test]
fn sort_hold_838_direct_navigation_replacement_inherits_gate() {
    let mut h = Harness::images();
    let held = request_sort(&mut h.env, SortOrder::DurationAsc);
    let sort_blocked = !h.env.grid_item_input_allowed();
    let destination = h.env.tmp.path().join("direct-destination");
    std::fs::create_dir(&destination).unwrap();
    let generation = h.env.items_generation;
    h.env.open_direct_navigation_target_at_stage(
        destination.clone(),
        None,
        OpenRequestOwner::Navigation,
        None,
        None,
        StartupListIntent::ExplicitList,
        MainListRequestStage::Admission,
        DirectoryScanRoute::Worker,
    );
    let direct = HeldScan::capture(&mut h.env);
    held.assert_retired();
    assert!(h.env.direct_navigation_scan_pending());
    assert!(!h.env.grid_item_input_allowed());
    h.tick(vec![]);
    assert_eq!(h.env.items_generation, generation);
    direct.release();
    h.until(|h| {
        h.env.current_folder.as_ref() == Some(&destination) && !h.env.sidecar_restore_active()
    });
    assert!(h.env.grid_item_input_allowed());
    assert!(
        sort_blocked,
        "the sort half of the handoff must also block input"
    );
}

#[test]
fn sort_hold_838_detached_sort_wait_does_not_block_main_grid() {
    let mut h = Harness::images();
    let id = independent_context(&mut h, 183807);
    let held = h
        .env
        .with_viewer_context(id, |app| {
            assert!(!app.main_folder_history_available());
            request_sort(app, SortOrder::DurationAsc)
        })
        .unwrap();
    assert!(h.env.folder_pane_open_pending.is_none());
    assert!(h.env.grid_item_input_allowed());
    h.assert_grid_owner();
    h.press(egui::Key::ArrowRight, egui::Modifiers::NONE);
    assert_eq!(h.env.selected, Some(1));
    h.env
        .with_viewer_context(id, |app| {
            assert!(app.folder_pane_open_pending.is_some());
            assert_eq!(app.selected, Some(0));
            assert!(
                app.grid_item_input_allowed(),
                "main-only gate excludes detached owner"
            );
        })
        .unwrap();
    h.env
        .retire_context(id, "sort_hold_838_detached_cleanup", |_| ())
        .unwrap();
    held.assert_retired();
}

#[test]
fn sort_hold_838_main_sort_wait_does_not_block_independent_context() {
    let mut h = Harness::images();
    let id = independent_context(&mut h, 183808);
    let held = request_sort(&mut h.env, SortOrder::DurationAsc);
    let main_generation = h.env.items_generation;
    let main_selected = h.env.selected;
    h.env
        .with_viewer_context(id, |app| {
            assert!(!app.main_folder_history_available());
            assert!(app.folder_pane_open_pending.is_none());
            assert!(app.grid_item_input_allowed());
            app.fullscreen_idx = Some(0);
            app.spread_mode = crate::settings::SpreadMode::Single;
            app.reading_direction = crate::settings::ReadingDirection::Ltr;
            // Actual fullscreen key routing in the independent owner; no desktop window.
            h.ctx
                .run_pass(
                    super::super::viewport_raw_input(
                        App::detached_image_window_viewport_id(183808),
                        vec![key_event(
                            egui::Key::ArrowRight,
                            true,
                            egui::Modifiers::NONE,
                        )],
                    ),
                    |ctx| {
                        app.input_update_begin(ctx);
                        let action = app.handle_fs_key_input(ctx, 0, false);
                        assert_eq!(action.page_nav, crate::ui_fullscreen::FsPageNav::Delta(1));
                    },
                )
                .drop_without_applying_deltas();
        })
        .unwrap();
    assert_eq!(h.env.items_generation, main_generation);
    assert_eq!(h.env.selected, main_selected);
    assert!(!held.cancel.load(Ordering::Relaxed));
    assert!(h.env.folder_pane_open_pending.is_some());
}
