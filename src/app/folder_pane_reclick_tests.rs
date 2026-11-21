//! §1.828 / folder-tree-refresh-plan §14: handler-level, pre-implementation tests.
//! Keep request admission in the real pointer/input/arbitration path. Controlled
//! receivers below delay only completion, using the existing test APIs.

use super::*;
use collection_navigation::{
    PhysicalHistoryPreflight, PhysicalHistoryPreflightPayload, PhysicalHistoryPreflightPoll,
};
use std::time::{Duration, Instant};

struct ReclickFixture {
    app: AppTestEnvForTest,
    ctx: egui::Context,
    frame: u64,
    a: PathBuf,
    b: PathBuf,
    kept: PathBuf,
    removed: PathBuf,
    pos: egui::Pos2,
}

fn reclick_fixture() -> ReclickFixture {
    let (mut app, ctx, mut frame, b, _) = navigation_tree_fixture();
    ctx.options_mut(|options| options.max_passes = std::num::NonZeroUsize::new(1).unwrap());
    app.settings.sidecar_backup_enabled = false;
    app.settings.tag_sidecar_backup_enabled = false;
    app.settings.stack_script_enabled = false;
    let a = app.current_folder.clone().unwrap();
    let kept = a.join("kept");
    let removed = a.join("removed");
    let other_child = b.join("other-child");
    for path in [&kept, &removed, &other_child] {
        std::fs::create_dir(path).unwrap();
    }
    for name in ["20.jpg", "40.jpg", "60.jpg"] {
        std::fs::write(a.join(name), b"image-source").unwrap();
    }
    std::fs::write(b.join("only-b.jpg"), b"image-source").unwrap();
    app.load_folder(a.clone());
    settle(&mut app, &ctx);
    app.folder_pane
        .test_seed_listing(&a, vec![kept.clone(), removed.clone()], true);
    app.folder_pane
        .test_seed_listing(&b, vec![other_child], true);
    app.folder_pane.set_cursor(a.clone());
    app.folder_pane.set_focus_tree();
    settle_navigation_tree_scans(&mut app, &ctx, &mut frame, 600.0);
    for _ in 0..3 {
        navigation_tree_frame(&mut app, &ctx, &mut frame, vec![]);
    }
    assert!(app.is_physical_folder_listing());
    assert_eq!(app.folder_pane.active_path(), Some(a.as_path()));
    assert!(!app.main_chrome_pointer_blocked());
    assert!(app.folder_pane_open_pending.is_none());
    assert!(
        app.top_level_grid_view
            .history_navigation_transition()
            .is_none()
    );
    let pos = visible_label_click_rect(&ctx, &a).center();
    ReclickFixture {
        app,
        ctx,
        frame,
        a,
        b,
        kept,
        removed,
        pos,
    }
}

fn reclick_physical_request(app: &App) -> &PhysicalHistoryTransition {
    match app.top_level_grid_view.history_navigation_transition() {
        Some(HistoryNavigationTransition::Physical(request)) => request,
        _ => panic!("the current-row click must admit a worker physical refresh"),
    }
}

fn reclick_refresh_id(app: &App, path: &Path) -> u64 {
    let request = reclick_physical_request(app);
    assert_eq!(request.path.as_path(), path);
    assert!(matches!(
        request.navigation.history,
        MainHistoryOperation::SameLocation
    ));
    assert!(matches!(
        request.intent,
        PhysicalHistoryIntent::Navigation {
            replay: None,
            auto_fullscreen: false,
        }
    ));
    assert!(matches!(
        request.restore_intent,
        StartupListIntent::RefreshList {
            automatic_observation: false,
            book_append_refresh: None,
            ..
        }
    ));
    assert!(request.dfs_continuation.is_none());
    assert!(matches!(
        request.phase,
        PhysicalHistoryPhase::Preflighting { .. }
    ));
    request.request_id
}

fn reclick_preflight_mut(app: &mut App) -> &mut PhysicalHistoryPreflight {
    let Some(HistoryNavigationTransition::Physical(request)) =
        app.top_level_grid_view.history_navigation_transition_mut()
    else {
        panic!("expected an admitted physical request");
    };
    let PhysicalHistoryPhase::Preflighting { preflight, .. } = &mut request.phase else {
        panic!("expected the existing worker preflight phase");
    };
    preflight
}

fn reclick_hold_preflight(
    app: &mut App,
) -> (
    PhysicalHistoryPreflight,
    mpsc::Sender<Result<PhysicalHistoryPreflightPayload, String>>,
) {
    let (waiting, sender) = PhysicalHistoryPreflight::controlled_for_test();
    (
        std::mem::replace(reclick_preflight_mut(app), waiting),
        sender,
    )
}

fn reclick_worker_result(
    mut preflight: PhysicalHistoryPreflight,
) -> PhysicalHistoryPreflightPayload {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        match preflight.poll() {
            PhysicalHistoryPreflightPoll::Ready(payload) => return payload,
            PhysicalHistoryPreflightPoll::Failed(message) => panic!("preflight: {message}"),
            PhysicalHistoryPreflightPoll::Pending => {}
        }
        assert!(
            Instant::now() < deadline,
            "real refresh worker did not finish"
        );
        std::thread::yield_now();
    }
}

fn reclick_tree_identity(app: &App) -> (Vec<PathBuf>, Option<PathBuf>) {
    let rows = app.folder_pane.visible_rows();
    (
        rows.iter()
            .filter(|row| row.expanded)
            .map(|row| row.path.clone())
            .collect(),
        rows.iter()
            .find(|row| row.is_cursor)
            .map(|row| row.path.clone()),
    )
}

fn reclick_assert_no_list_request(app: &App, generation: u64) {
    assert!(
        app.top_level_grid_view
            .history_navigation_transition()
            .is_none()
    );
    assert!(app.top_level_grid_view.open_path_classification().is_none());
    assert!(app.folder_pane_open_pending.is_none());
    assert!(app.external_rescan_pending.is_none());
    assert_eq!(app.items_generation, generation);
}

fn reclick_contains(app: &App, path: &Path) -> bool {
    app.items
        .iter()
        .any(|item| item.drag_source_path() == Some(path))
}

fn reclick_single_scenario(mtime: bool, order: crate::settings::SortOrder) {
    let ReclickFixture {
        mut app,
        ctx,
        mut frame,
        a,
        b,
        kept,
        removed,
        pos,
    } = reclick_fixture();
    app.settings.sort_order = order;
    // A non-aggregated list, with enough real rows for a meaningful viewport anchor.
    for index in 0..48 {
        std::fs::write(a.join(format!("page-{index:02}.jpg")), b"image-source").unwrap();
    }
    app.load_folder(a.clone());
    settle(&mut app, &ctx);
    navigation_tree_frame(&mut app, &ctx, &mut frame, vec![]);
    let selected_path = a.join("page-24.jpg");
    app.selected = app
        .items
        .iter()
        .position(|item| item.drag_source_path() == Some(selected_path.as_path()));
    let selected_visible = app
        .visible_indices
        .iter()
        .position(|index| Some(*index) == app.selected)
        .unwrap();
    let context = app.projected_viewer_context_id();
    let generation = app.items_generation;
    let content_h = app.visible_indices.len() as f32 * 100.0;
    app.grid_scroll.record_thumbnail_geometry(
        GridScrollGeometry::new(1, 100.0, 400.0, content_h),
        context,
        generation,
        100.0,
    );
    app.last_cell_size = 100.0;
    app.grid_scroll
        .set_position((selected_visible - 1) as f32 * 100.0);
    app.grid_scroll.set_row_anchor(None);
    let expected_intent = app.current_list_refresh_intent();
    let StartupListIntent::RefreshList {
        cursor: Some(ref cursor),
        ..
    } = expected_intent
    else {
        panic!("fixture must capture a real cursor and viewport anchor");
    };
    assert_eq!(cursor.name, "page-24.jpg");
    assert_eq!(cursor.rows_above, Some(1));
    // The normal input frame paints before arbitration. Keep that real paint from
    // replacing our explicit geometry by recording the same geometry in its hook.
    let new_file = a.join("00-added.jpg");
    let new_dir = a.join("added");
    let other_new = b.join("added-in-other-expanded-branch");
    std::fs::write(&new_file, b"external").unwrap();
    std::fs::create_dir(&new_dir).unwrap();
    std::fs::create_dir(&other_new).unwrap();
    std::fs::remove_dir(&removed).unwrap();
    app.current_folder_last_mtime = mtime.then_some(SystemTime::UNIX_EPOCH);
    let history = app.folder_nav_history_snapshot();
    let tree_identity = reclick_tree_identity(&app);
    let scroll = pane_scroll_state(&ctx).unclamped_offset();
    let reveals = tree_scrolls(&ctx);
    let tree_gate = app.folder_pane.test_hold_next_scan(a.clone());
    #[cfg(all(windows, feature = "test-script"))]
    let loads = app.test_script_folder_load_requests;
    let caller_scans = folder_scan::caller_scan_count_for_test();
    // Put both edges in one real pass: the intent is captured at arbitration,
    // after the same production drawing/input phases as all other tree tests.
    navigation_tree_frame_height_before_navigation(
        &mut app,
        &ctx,
        &mut frame,
        vec![
            egui::Event::PointerMoved(pos),
            primary_event(pos, true),
            primary_event(pos, false),
        ],
        600.0,
        |app| {
            app.grid_scroll.record_thumbnail_geometry(
                GridScrollGeometry::new(1, 100.0, 400.0, content_h),
                context,
                generation,
                100.0,
            );
            app.last_cell_size = 100.0;
            app.grid_scroll
                .set_position((selected_visible - 1) as f32 * 100.0);
            app.grid_scroll.set_row_anchor(None);
        },
    );
    reclick_refresh_id(&app, &a);
    assert_eq!(
        folder_scan::caller_scan_count_for_test(),
        caller_scans,
        "reclick must not enumerate the directory on the input thread"
    );
    assert_eq!(
        reclick_physical_request(&app).restore_intent,
        expected_intent
    );
    #[cfg(all(windows, feature = "test-script"))]
    assert_eq!(app.test_script_folder_load_requests, loads + 1);
    assert_eq!(
        app.items_generation, generation,
        "worker result has not been polled; no synchronous install"
    );
    assert!(!reclick_contains(&app, &new_file));
    assert_eq!(app.current_folder.as_ref(), Some(&a));
    assert_eq!(app.folder_nav_history_snapshot(), history);
    assert_eq!(reclick_tree_identity(&app), tree_identity);
    assert_eq!(app.folder_pane.cursor_scroll, FolderPaneCursorScroll::Idle);
    assert_eq!(pane_scroll_state(&ctx).unclamped_offset(), scroll);
    assert_eq!(tree_scrolls(&ctx), reveals);
    assert!(app.folder_pane.label_gesture_requires_tree_focus());
    assert!(
        app.folder_pane.test_unverified(&a),
        "displayed active node must be rechecked"
    );
    // Release the existing tree worker and let both production adoption paths finish.
    drop(tree_gate);
    settle(&mut app, &ctx);
    assert_eq!(folder_scan::caller_scan_count_for_test(), caller_scans);
    assert_eq!(app.selected_real_file_path(), Some(selected_path));
    assert_eq!(app.grid_scroll.row_anchor(), Some(1));
    settle_navigation_tree_scans(&mut app, &ctx, &mut frame, 600.0);
    for path in [&new_file, &new_dir, &kept] {
        assert!(reclick_contains(&app, path), "missing {path:?}");
    }
    assert!(!reclick_contains(&app, &removed));
    let children = app.folder_pane.test_children(&a);
    assert!(children.contains(&new_dir) && children.contains(&kept));
    assert!(!children.contains(&removed));
    assert!(
        app.folder_pane.test_children(&b).contains(&other_new),
        "recheck covers the other displayed demand node too"
    );
    assert_eq!(app.current_folder.as_ref(), Some(&a));
    assert_eq!(app.folder_nav_history_snapshot(), history);
    assert_eq!(reclick_tree_identity(&app), tree_identity);
}

#[test]
fn folder_pane_reclick_single_name_order_refreshes_list_and_displayed_tree() {
    reclick_single_scenario(true, crate::settings::SortOrder::FileName);
}

#[test]
fn folder_pane_reclick_single_without_mtime_still_uses_worker() {
    reclick_single_scenario(false, crate::settings::SortOrder::FileName);
}

#[test]
fn folder_pane_reclick_single_duration_order_preserves_cursor_and_anchor() {
    reclick_single_scenario(true, crate::settings::SortOrder::DurationAsc);
}

#[test]
fn folder_pane_reclick_scrolled_active_row_keeps_tree_position_through_completion() {
    let (mut app, ctx, mut frame, active) = scrollable_tree_click_fixture(true);
    app.settings.sidecar_backup_enabled = false;
    app.settings.tag_sidecar_backup_enabled = false;
    ctx.options_mut(|options| options.max_passes = std::num::NonZeroUsize::new(1).unwrap());
    let rect = visible_label_click_rect(&ctx, &active);
    // First use the existing non-current-row navigation to make this off-centre
    // row active without manufacturing a scroll position or bypassing admission.
    pointer_tree_click(&mut app, &ctx, &mut frame, rect.center());
    finish_label_navigation(&mut app, &ctx, &active);
    settle_navigation_tree_scans(&mut app, &ctx, &mut frame, 600.0);
    frame += (ctx.options(|o| o.input_options.max_double_click_delay) * 60.0).ceil() as u64 + 2;
    navigation_tree_frame(&mut app, &ctx, &mut frame, vec![]);
    let rect = visible_label_click_rect(&ctx, &active);
    let offset = pane_scroll_state(&ctx).unclamped_offset().y;
    let reveals = tree_scrolls(&ctx);
    assert!(offset > 100.0);
    assert!((rect.center().y - tree_click_viewport(&ctx).center().y).abs() > 100.0);
    assert!(app.is_physical_folder_listing());
    assert_eq!(app.folder_pane.active_path(), Some(active.as_path()));
    let identity = reclick_tree_identity(&app);
    let history = app.folder_nav_history_snapshot();
    let added = active.join("added.jpg");
    std::fs::write(&added, b"external").unwrap();
    pointer_tree_click(&mut app, &ctx, &mut frame, rect.center());
    let first = reclick_refresh_id(&app, &active);
    for _ in 0..3 {
        navigation_tree_frame(&mut app, &ctx, &mut frame, vec![]);
        assert_eq!(reclick_refresh_id(&app, &active), first);
        assert_tree_click_position(&ctx, &active, offset, rect);
        assert_eq!(reclick_tree_identity(&app), identity);
    }
    assert!(app.folder_pane.label_gesture_requires_tree_focus());
    settle(&mut app, &ctx);
    settle_navigation_tree_scans(&mut app, &ctx, &mut frame, 600.0);
    assert_tree_click_position(&ctx, &active, offset, rect);
    assert_eq!(tree_scrolls(&ctx), reveals);
    assert_eq!(reclick_tree_identity(&app), identity);
    assert_eq!(app.folder_nav_history_snapshot(), history);
    assert!(reclick_contains(&app, &added));
}

fn reclick_repeat_scenario(clicks: usize) {
    let ReclickFixture {
        mut app,
        ctx,
        mut frame,
        a,
        pos,
        ..
    } = reclick_fixture();
    let history = app.folder_nav_history_snapshot();
    let generation = app.items_generation;
    #[cfg(all(windows, feature = "test-script"))]
    let loads = app.test_script_folder_load_requests;
    pointer_tree_click(&mut app, &ctx, &mut frame, pos);
    let first = reclick_refresh_id(&app, &a);
    let (worker, hold) = reclick_hold_preflight(&mut app);
    assert_label_expanded(&app, &a, true);
    for _ in 1..clicks {
        pointer_tree_click(&mut app, &ctx, &mut frame, pos);
        assert_label_expanded(&app, &a, false);
        assert_eq!(
            reclick_refresh_id(&app, &a),
            first,
            "gesture continuation must not refresh again"
        );
        assert_eq!(app.current_folder.as_ref(), Some(&a));
        assert_eq!(app.items_generation, generation);
        assert!(app.folder_pane_open_pending.is_none());
    }
    #[cfg(all(windows, feature = "test-script"))]
    assert_eq!(app.test_script_folder_load_requests, loads + 1);
    *reclick_preflight_mut(&mut app) = worker;
    drop(hold);
    settle(&mut app, &ctx);
    assert_eq!(app.folder_nav_history_snapshot(), history);
    assert_eq!(app.current_folder.as_ref(), Some(&a));
    assert_label_expanded(&app, &a, false);
}

#[test]
fn folder_pane_reclick_double_refreshes_once_and_toggles_once() {
    reclick_repeat_scenario(2);
}

#[test]
fn folder_pane_reclick_triple_refreshes_once_and_toggles_once() {
    reclick_repeat_scenario(3);
}

fn reclick_supersede_scenario(ready: bool) {
    let ReclickFixture {
        mut app,
        ctx,
        mut frame,
        a,
        pos,
        ..
    } = reclick_fixture();
    let history = app.folder_nav_history_snapshot();
    let generation = app.items_generation;
    let old_only = a.join("old-only.jpg");
    let latest = a.join("latest.jpg");
    std::fs::write(&old_only, b"first observation").unwrap();
    pointer_tree_click(&mut app, &ctx, &mut frame, pos);
    let first = reclick_refresh_id(&app, &a);
    let (worker, sender) = reclick_hold_preflight(&mut app);
    let old_result = reclick_worker_result(worker);
    std::fs::remove_file(&old_only).unwrap();
    std::fs::write(&latest, b"second observation").unwrap();
    // Only synthetic input time advances; no wall-clock sleeps or test runner work.
    frame += (ctx.options(|o| o.input_options.max_double_click_delay) * 60.0).ceil() as u64 + 2;
    if ready {
        assert!(sender.send(Ok(old_result)).is_ok());
        pointer_tree_click(&mut app, &ctx, &mut frame, pos);
    } else {
        pointer_tree_click(&mut app, &ctx, &mut frame, pos);
        assert!(
            sender.send(Ok(old_result)).is_err(),
            "superseded receiver must be retired"
        );
    }
    assert_ne!(reclick_refresh_id(&app, &a), first);
    assert_eq!(
        app.items_generation, generation,
        "even ready old results must not be installed"
    );
    assert_eq!(app.folder_nav_history_snapshot(), history);
    settle(&mut app, &ctx);
    assert!(reclick_contains(&app, &latest));
    assert!(!reclick_contains(&app, &old_only));
    assert_eq!(app.folder_nav_history_snapshot(), history);
    assert_eq!(app.current_folder.as_ref(), Some(&a));
    assert_label_expanded(&app, &a, true);
}

#[test]
fn folder_pane_reclick_after_delay_replaces_pending_refresh_and_rejects_old_result() {
    reclick_supersede_scenario(false);
}

#[test]
fn folder_pane_reclick_arbitration_only_replaces_ready_same_location_refresh() {
    // Unit coverage of arbitration: navigation_tree_frame deliberately omits
    // the root's pre-input history poll. A ready still owned by the arbiter may
    // be replaced; production may already have installed it before input.
    reclick_supersede_scenario(true);
}

fn reclick_production_frame(
    app: &mut App,
    ctx: &egui::Context,
    frame: &mut u64,
    events: Vec<egui::Event>,
) {
    *frame += 1;
    // This helper services HistoryTransition before input_update_begin, as
    // App::ui does, and drains any real hydration started by that installation.
    sidecar_tree_frame(app, ctx, *frame as f64 / 60.0, events, None);
}

#[test]
fn folder_pane_reclick_production_order_installs_ready_then_refreshes_latest_with_selection() {
    let ReclickFixture {
        mut app,
        ctx,
        mut frame,
        a,
        pos,
        ..
    } = reclick_fixture();
    let selected = a.join("60.jpg");
    app.selected = app
        .items
        .iter()
        .position(|item| item.drag_source_path() == Some(selected.as_path()));
    assert_eq!(app.selected_real_file_path(), Some(selected.clone()));
    let history = app.folder_nav_history_snapshot();
    let generation = app.items_generation;
    let old_only = a.join("00-old-only.jpg");
    let latest = a.join("01-latest.jpg");
    std::fs::write(&old_only, b"first observation").unwrap();
    for pressed in [true, false] {
        reclick_production_frame(
            &mut app,
            &ctx,
            &mut frame,
            vec![egui::Event::PointerMoved(pos), primary_event(pos, pressed)],
        );
    }
    reclick_refresh_id(&app, &a);
    let (worker, sender) = reclick_hold_preflight(&mut app);
    let old_result = reclick_worker_result(worker);
    std::fs::remove_file(&old_only).unwrap();
    std::fs::write(&latest, b"second observation").unwrap();
    assert!(sender.send(Ok(old_result)).is_ok());

    // A ready already available to the root poll is allowed to install before
    // the next gesture. Do not impose the arbitration-only non-installation
    // assertion here. Finish its normal hydration before starting a fresh click.
    reclick_production_frame(&mut app, &ctx, &mut frame, vec![]);
    assert!(app.items_generation > generation);
    assert!(reclick_contains(&app, &old_only));
    assert!(!reclick_contains(&app, &latest));
    let deadline = Instant::now() + Duration::from_secs(20);
    while app
        .top_level_grid_view
        .history_navigation_transition()
        .is_some()
        || app.sidecar_restore_active()
    {
        reclick_production_frame(&mut app, &ctx, &mut frame, vec![]);
        assert!(
            Instant::now() < deadline,
            "old refresh hydration did not finish"
        );
        std::thread::yield_now();
    }
    assert_eq!(app.selected_real_file_path(), Some(selected.clone()));
    assert_eq!(app.folder_nav_history_snapshot(), history);
    // The last hydration frame was drawn while the restore still blocked the
    // main chrome, so its labels have no hit-test witness. Draw one ordinary
    // frame after the restore ends, as the next repaint does, before pressing.
    reclick_production_frame(&mut app, &ctx, &mut frame, vec![]);
    frame += (ctx.options(|o| o.input_options.max_double_click_delay) * 60.0).ceil() as u64 + 2;
    let pos = visible_label_click_rect(&ctx, &a).center();
    let installed_generation = app.items_generation;
    for pressed in [true, false] {
        reclick_production_frame(
            &mut app,
            &ctx,
            &mut frame,
            vec![egui::Event::PointerMoved(pos), primary_event(pos, pressed)],
        );
    }
    reclick_refresh_id(&app, &a);
    let deadline = Instant::now() + Duration::from_secs(20);
    while app
        .top_level_grid_view
        .history_navigation_transition()
        .is_some()
        || app.sidecar_restore_active()
    {
        reclick_production_frame(&mut app, &ctx, &mut frame, vec![]);
        assert!(Instant::now() < deadline, "latest refresh did not finish");
        std::thread::yield_now();
    }
    assert!(app.items_generation > installed_generation);
    assert!(reclick_contains(&app, &latest));
    assert!(!reclick_contains(&app, &old_only));
    assert_eq!(app.selected_real_file_path(), Some(selected));
    assert_eq!(app.folder_nav_history_snapshot(), history);
    assert_eq!(app.current_folder.as_ref(), Some(&a));
    assert_label_expanded(&app, &a, true);
}

#[test]
fn folder_pane_reclick_same_pass_ready_order_refresh_is_replaced() {
    let ReclickFixture {
        mut app,
        ctx,
        mut frame,
        a,
        pos,
        ..
    } = reclick_fixture();
    app.settings.sort_order = crate::settings::SortOrder::DurationAsc;
    // This is the existing duration-sort refresh used by the toolbar's reload.
    // Its result becomes the arbiter's local folder_pane_open_ready, exactly as
    // B's result does in the contrasting ready-navigation regression below.
    app.apply_sort_change_reload_without_ui_io();
    let pending = app.folder_pane_open_pending.as_mut().unwrap();
    assert_eq!(pending.path, a);
    assert!(matches!(
        pending.purpose,
        FolderOpenScanPurpose::CurrentViewOrderRefresh { .. }
    ));
    assert!(matches!(
        pending.navigation.as_ref().unwrap().history,
        MainHistoryOperation::SameLocation
    ));
    let (sender, waiting) = mpsc::channel();
    let worker = std::mem::replace(&mut pending.rx, waiting);
    let old_result = worker
        .recv_timeout(Duration::from_secs(20))
        .expect("real order-refresh worker result");
    assert!(old_result.is_ok());
    let latest = a.join("latest-after-order-scan.jpg");
    std::fs::write(&latest, b"external after old scan").unwrap();
    let generation = app.items_generation;
    let history = app.folder_nav_history_snapshot();
    navigation_tree_frame(
        &mut app,
        &ctx,
        &mut frame,
        vec![egui::Event::PointerMoved(pos), primary_event(pos, true)],
    );
    assert!(sender.send(old_result).is_ok());
    navigation_tree_frame(&mut app, &ctx, &mut frame, vec![primary_event(pos, false)]);
    assert_eq!(
        app.items_generation, generation,
        "the old refresh ready must yield to this reclick before installation"
    );
    reclick_refresh_id(&app, &a);
    assert!(app.folder_pane_open_pending.is_none());
    settle(&mut app, &ctx);
    assert!(reclick_contains(&app, &latest));
    assert_eq!(app.folder_nav_history_snapshot(), history);
}

#[test]
fn folder_pane_reclick_other_row_navigation_outlives_old_refresh_result() {
    let ReclickFixture {
        mut app,
        ctx,
        mut frame,
        a,
        b,
        pos,
        ..
    } = reclick_fixture();
    pointer_tree_click(&mut app, &ctx, &mut frame, pos);
    reclick_refresh_id(&app, &a);
    let (worker, sender) = reclick_hold_preflight(&mut app);
    let stale = reclick_worker_result(worker);
    let b_pos = visible_label_click_rect(&ctx, &b).center();
    assert!(pos.distance(b_pos) > 6.0);
    pointer_tree_click(&mut app, &ctx, &mut frame, b_pos);
    assert!(
        sender.send(Ok(stale)).is_err(),
        "B must retire A's refresh receiver"
    );
    finish_label_navigation(&mut app, &ctx, &b);
    let generation = app.items_generation;
    let items = app.items.clone();
    settle(&mut app, &ctx);
    settle_navigation_tree_scans(&mut app, &ctx, &mut frame, 600.0);
    assert_eq!(app.current_folder.as_ref(), Some(&b));
    assert_eq!(app.folder_pane.active_path(), Some(b.as_path()));
    assert_eq!(app.items, items);
    assert_eq!(app.items_generation, generation);
    assert!(reclick_contains(&app, &b.join("only-b.jpg")));
    assert!(!reclick_contains(&app, &a.join("20.jpg")));
}

#[test]
fn folder_pane_reclick_touch_tap_refreshes_once_without_toggle() {
    let ReclickFixture {
        mut app,
        ctx,
        mut frame,
        a,
        pos,
        ..
    } = reclick_fixture();
    let touch = |phase| egui::Event::Touch {
        device_id: egui::TouchDeviceId(828),
        id: egui::TouchId(1),
        phase,
        pos,
        force: None,
    };
    let history = app.folder_nav_history_snapshot();
    #[cfg(all(windows, feature = "test-script"))]
    let loads = app.test_script_folder_load_requests;
    navigation_tree_frame(
        &mut app,
        &ctx,
        &mut frame,
        vec![
            touch(egui::TouchPhase::Start),
            egui::Event::PointerMoved(pos),
            primary_event(pos, true),
        ],
    );
    assert!(
        app.top_level_grid_view
            .history_navigation_transition()
            .is_none()
    );
    navigation_tree_frame(
        &mut app,
        &ctx,
        &mut frame,
        vec![
            touch(egui::TouchPhase::End),
            primary_event(pos, false),
            egui::Event::PointerGone,
        ],
    );
    let request = reclick_refresh_id(&app, &a);
    navigation_tree_frame(&mut app, &ctx, &mut frame, vec![]);
    assert_eq!(reclick_refresh_id(&app, &a), request);
    #[cfg(all(windows, feature = "test-script"))]
    assert_eq!(app.test_script_folder_load_requests, loads + 1);
    assert_label_expanded(&app, &a, true);
    settle(&mut app, &ctx);
    assert_eq!(app.folder_nav_history_snapshot(), history);
}

fn reclick_pending_pane_scenario(ready_on_release: bool) {
    let ReclickFixture {
        mut app,
        ctx,
        mut frame,
        a,
        b,
        pos,
        ..
    } = reclick_fixture();
    let b_pos = visible_label_click_rect(&ctx, &b).center();
    pointer_tree_click(&mut app, &ctx, &mut frame, b_pos);
    let pending = app
        .folder_pane_open_pending
        .as_mut()
        .expect("B click starts pane navigation");
    assert_eq!(pending.path, b);
    let first = pending.cancel.clone();
    let (sender, waiting) = mpsc::channel();
    let worker = std::mem::replace(&mut pending.rx, waiting);
    let generation = app.items_generation;
    let history = app.folder_nav_history_snapshot();
    navigation_tree_frame(
        &mut app,
        &ctx,
        &mut frame,
        vec![egui::Event::PointerMoved(pos), primary_event(pos, true)],
    );
    if ready_on_release {
        let result = worker
            .recv_timeout(Duration::from_secs(20))
            .expect("real B worker result");
        assert!(result.is_ok());
        assert!(sender.send(result).is_ok());
    }
    navigation_tree_frame(&mut app, &ctx, &mut frame, vec![primary_event(pos, false)]);
    assert!(
        !first.load(Ordering::Relaxed),
        "rejected reclick must not cancel B"
    );
    assert!(
        app.top_level_grid_view
            .history_navigation_transition()
            .is_none()
    );
    if ready_on_release {
        assert_eq!(
            app.current_folder.as_ref(),
            Some(&b),
            "B's ready is applied in the same pass as the rejected A reclick"
        );
        assert!(app.folder_pane_open_pending.is_none());
    } else {
        assert_eq!(app.current_folder.as_ref(), Some(&a));
        assert_eq!(app.items_generation, generation);
        assert_eq!(app.folder_nav_history_snapshot(), history);
        let pending = app
            .folder_pane_open_pending
            .as_mut()
            .expect("B remains pending");
        assert_eq!(pending.path, b);
        assert!(Arc::ptr_eq(&pending.cancel, &first));
        pending.rx = worker;
    }
    finish_label_navigation(&mut app, &ctx, &b);
    assert!(!first.load(Ordering::Relaxed));
    assert!(reclick_contains(&app, &b.join("only-b.jpg")));
}

#[test]
fn folder_pane_reclick_does_not_cancel_pending_pane_navigation() {
    reclick_pending_pane_scenario(false);
}

#[test]
fn folder_pane_reclick_same_pass_ready_pane_navigation_is_applied() {
    reclick_pending_pane_scenario(true);
}

#[test]
fn folder_pane_reclick_does_not_replace_pending_history_navigation() {
    let ReclickFixture {
        mut app,
        ctx,
        mut frame,
        a,
        b,
        ..
    } = reclick_fixture();
    app.load_folder(b.clone());
    settle(&mut app, &ctx);
    app.load_folder(a.clone());
    settle(&mut app, &ctx);
    navigation_tree_frame(&mut app, &ctx, &mut frame, vec![]);
    assert_eq!(
        app.folder_history_back_target(),
        Some(&FolderNavHistoryTarget::Path(b.clone()))
    );
    app.dispatch_folder_history_input_for_test(FolderHistoryDirection::Back);
    app.settle_open_path_classification_for_test();
    let request = reclick_physical_request(&app);
    assert_eq!(request.path, b);
    assert!(matches!(
        request.navigation.history,
        MainHistoryOperation::Replay(_)
    ));
    let first = request.request_id;
    let (worker, hold) = reclick_hold_preflight(&mut app);
    let generation = app.items_generation;
    let history = app.folder_nav_history_snapshot();
    let pos = visible_label_click_rect(&ctx, &a).center();
    pointer_tree_click(&mut app, &ctx, &mut frame, pos);
    assert_eq!(reclick_physical_request(&app).request_id, first);
    assert_eq!(app.items_generation, generation);
    assert_eq!(app.folder_nav_history_snapshot(), history);
    assert!(app.folder_pane_open_pending.is_none());
    *reclick_preflight_mut(&mut app) = worker;
    drop(hold);
    settle(&mut app, &ctx);
    assert_eq!(app.current_folder.as_ref(), Some(&b));
}

#[test]
fn folder_pane_reclick_does_not_replace_same_location_explicit_entry() {
    let ReclickFixture {
        mut app,
        ctx,
        mut frame,
        a,
        pos,
        ..
    } = reclick_fixture();
    let owner = OpenRequestOwner::Navigation;
    let navigation = app.capture_physical_open_navigation(&owner, &a).unwrap();
    assert!(matches!(
        navigation.history,
        MainHistoryOperation::SameLocation
    ));
    assert!(app.start_physical_history_transition_classified_at_stage(
        PhysicalHistoryIntent::Navigation {
            replay: None,
            auto_fullscreen: false
        },
        a.clone(),
        None,
        Some(OpenPathKind::Directory),
        StartupListIntent::ExplicitList,
        navigation,
        None,
        MainListRequestStage::Admission,
    ));
    let first = reclick_physical_request(&app).request_id;
    let (worker, hold) = reclick_hold_preflight(&mut app);
    pointer_tree_click(&mut app, &ctx, &mut frame, pos);
    assert_eq!(reclick_physical_request(&app).request_id, first);
    assert!(matches!(
        reclick_physical_request(&app).restore_intent,
        StartupListIntent::ExplicitList
    ));
    *reclick_preflight_mut(&mut app) = worker;
    drop(hold);
    settle(&mut app, &ctx);
}

fn reclick_rejected_scenario(kind: &str) {
    let ReclickFixture {
        mut app,
        ctx,
        mut frame,
        a,
        pos,
        ..
    } = reclick_fixture();
    match kind {
        "search" => {
            app.top_level_grid_view.replace_surface(
                top_level_grid_view::TopLevelGridSurface::Search(
                    top_level_grid_view::TopLevelSearchView::Global,
                ),
            );
            assert!(!app.is_physical_folder_listing());
        }
        "zip" => {
            // Same physical parent is still the tree's active path; the displayed
            // item is a real virtual-page kind, not a physical-folder row.
            app.items = vec![GridItem::ZipImage {
                zip_path: a.join("book.zip"),
                entry_name: "page.jpg".into(),
            }];
            app.visible_indices = vec![0];
            app.thumbnails = vec![ThumbnailState::Pending];
            app.image_metas = vec![None];
            assert!(!app.is_physical_folder_listing());
        }
        "initial" => {
            app.normal_folder_omitted_entries = None;
            assert!(!app.is_physical_folder_listing());
        }
        "fullscreen" => {
            app.fullscreen_idx = Some(0);
            app.viewer_presentation = ViewerPresentation::Fullscreen;
        }
        "flat" => app.stack_showing_flat = true,
        "snapshot" => {
            app.activate_snapshot(crate::snapshot::SnapshotSourceLabel::Mixed);
            app.settle_snapshot_page_index_for_test();
            assert!(app.folder_pane_disabled());
        }
        "modal" => {
            app.show_preferences = true;
            assert!(app.main_chrome_pointer_blocked());
        }
        _ => unreachable!(),
    }
    let generation = app.items_generation;
    let history = app.folder_nav_history_snapshot();
    let items = app.items.clone();
    let children = app.folder_pane.test_children(&a);
    pointer_tree_click(&mut app, &ctx, &mut frame, pos);
    reclick_assert_no_list_request(&app, generation);
    assert_eq!(app.items, items);
    assert_eq!(app.current_folder.as_ref(), Some(&a));
    assert_eq!(app.folder_nav_history_snapshot(), history);
    assert_eq!(app.folder_pane.test_children(&a), children);
}

#[test]
fn folder_pane_reclick_search_results_are_not_refreshed() {
    reclick_rejected_scenario("search");
}

#[test]
fn folder_pane_reclick_zip_parent_row_does_not_open_or_refresh_parent() {
    reclick_rejected_scenario("zip");
}

#[test]
fn folder_pane_reclick_initial_unpublished_listing_is_not_refreshed() {
    reclick_rejected_scenario("initial");
}

#[test]
fn folder_pane_reclick_main_fullscreen_is_not_refreshed() {
    reclick_rejected_scenario("fullscreen");
}

#[test]
fn folder_pane_reclick_flat_projection_is_not_refreshed() {
    reclick_rejected_scenario("flat");
}

#[test]
fn folder_pane_reclick_snapshot_lock_is_not_refreshed() {
    reclick_rejected_scenario("snapshot");
}

#[test]
fn folder_pane_reclick_modal_is_not_refreshed() {
    reclick_rejected_scenario("modal");
}

#[test]
fn folder_pane_reclick_delete_confirmation_started_after_tree_draw_rejects_refresh() {
    let ReclickFixture {
        mut app,
        ctx,
        mut frame,
        a,
        pos,
        ..
    } = reclick_fixture();
    let selected_path = a.join("20.jpg");
    app.selected = app
        .items
        .iter()
        .position(|item| item.drag_source_path() == Some(selected_path.as_path()));
    assert!(app.selected.is_some());
    let generation = app.items_generation;
    let history = app.folder_nav_history_snapshot();
    navigation_tree_frame(
        &mut app,
        &ctx,
        &mut frame,
        vec![egui::Event::PointerMoved(pos), primary_event(pos, true)],
    );
    navigation_tree_frame_height_before_navigation(
        &mut app,
        &ctx,
        &mut frame,
        vec![primary_event(pos, false), tree_key_event(egui::Key::Delete)],
        600.0,
        |app| {
            // Production update_frame calls this after tree/grid painting and
            // before navigation arbitration. Never confirm or run deletion.
            app.handle_delete_key(&ctx);
            assert!(app.show_delete_confirm);
            assert!(!app.delete_targets.is_empty());
            assert!(app.main_chrome_pointer_blocked());
        },
    );
    reclick_assert_no_list_request(&app, generation);
    assert_eq!(app.folder_nav_history_snapshot(), history);
    assert!(a.join("20.jpg").exists());
}

#[test]
fn folder_pane_reclick_late_saved_book_request_is_not_cancelled_by_rejected_refresh() {
    let ReclickFixture {
        mut app,
        ctx,
        mut frame,
        a,
        b,
        pos,
        ..
    } = reclick_fixture();
    app.settings.book_root = Some(a.parent().unwrap().to_path_buf());
    app.book_shelf.snapshot = Some(
        vec![crate::books::BookInfo {
            name: "B".into(),
            path: b,
            page_count: 1,
        }]
        .into(),
    );
    let generation = app.items_generation;
    let mut saved = None;
    navigation_tree_frame(
        &mut app,
        &ctx,
        &mut frame,
        vec![egui::Event::PointerMoved(pos), primary_event(pos, true)],
    );
    navigation_tree_frame_height_before_navigation(
        &mut app,
        &ctx,
        &mut frame,
        vec![primary_event(pos, false)],
        600.0,
        |app| {
            app.apply_saved_group_key_action(&ctx, crate::keymap::KeyAction::GridBookNext);
            saved = app.saved_group_open_request_id_for_test();
            assert!(
                saved.is_some(),
                "a real saved-book request owns admission before arbitration"
            );
            assert!(app.main_chrome_pointer_blocked());
        },
    );
    assert_eq!(
        app.saved_group_open_request_id_for_test(),
        saved,
        "rejected refresh must not trigger folder_pane_nav.is_some() cancellation"
    );
    reclick_assert_no_list_request(&app, generation);
}

#[test]
fn folder_pane_reclick_arrows_and_enter_do_not_refresh_current_listing() {
    let ReclickFixture {
        mut app,
        ctx,
        mut frame,
        a,
        ..
    } = reclick_fixture();
    let generation = app.items_generation;
    let history = app.folder_nav_history_snapshot();
    let label = visible_label_click_rect(&ctx, &a);
    let arrow = egui::pos2(label.left() - 3.0, label.center().y);
    pointer_tree_click(&mut app, &ctx, &mut frame, arrow);
    assert_label_expanded(&app, &a, false);
    reclick_assert_no_list_request(&app, generation);
    pointer_tree_click(&mut app, &ctx, &mut frame, arrow);
    assert_label_expanded(&app, &a, true);
    reclick_assert_no_list_request(&app, generation);
    for (key, expanded) in [(egui::Key::ArrowLeft, false), (egui::Key::ArrowRight, true)] {
        navigation_tree_frame(&mut app, &ctx, &mut frame, vec![tree_key_event(key)]);
        assert_label_expanded(&app, &a, expanded);
        reclick_assert_no_list_request(&app, generation);
    }
    // Enter already opens even the active path via PaneNavigation. Preserve
    // that explicit entry; it must not become the new RefreshList operation.
    let (hold, waiting) = mpsc::channel();
    let mut waiting = Some(waiting);
    let mut worker = None;
    navigation_tree_frame_height_before_navigation(
        &mut app,
        &ctx,
        &mut frame,
        vec![tree_key_event(egui::Key::Enter)],
        600.0,
        |app| {
            let pending = app
                .folder_pane_open_pending
                .as_mut()
                .expect("Enter keeps its existing open path");
            assert_eq!(pending.path, a);
            assert!(matches!(
                pending.purpose,
                FolderOpenScanPurpose::PaneNavigation
            ));
            worker = Some(std::mem::replace(&mut pending.rx, waiting.take().unwrap()));
        },
    );
    assert!(
        app.top_level_grid_view
            .history_navigation_transition()
            .is_none()
    );
    assert_eq!(app.items_generation, generation);
    app.folder_pane_open_pending.as_mut().unwrap().rx = worker.unwrap();
    drop(hold);
    finish_label_navigation(&mut app, &ctx, &a);
    assert_eq!(app.current_folder.as_ref(), Some(&a));
    assert_eq!(app.folder_nav_history_snapshot(), history);
}

#[test]
fn folder_pane_reclick_f_close_cursor_navigation_beats_current_name_then_other_arrow() {
    reclick_close_scenario(true, false);
}

#[test]
fn folder_pane_reclick_y_close_cursor_navigation_beats_current_name_then_other_arrow() {
    reclick_close_scenario(true, true);
}

#[test]
fn folder_pane_reclick_f_close_without_cursor_navigation_keeps_refresh() {
    reclick_close_scenario(false, false);
}

#[test]
fn folder_pane_reclick_y_close_without_cursor_navigation_keeps_refresh() {
    reclick_close_scenario(false, true);
}

fn reclick_close_scenario(move_cursor: bool, gamepad_y: bool) {
    let ReclickFixture {
        mut app,
        ctx,
        mut frame,
        a,
        b,
        pos,
        ..
    } = reclick_fixture();
    let history = app.folder_nav_history_snapshot();
    let generation = app.items_generation;
    let added = a.join("added-before-close.jpg");
    std::fs::write(&added, b"external addition").unwrap();
    let mut events = vec![
        egui::Event::PointerMoved(pos),
        primary_event(pos, true),
        primary_event(pos, false),
    ];
    if move_cursor {
        let arrow = app
            .folder_pane
            .label_display
            .as_ref()
            .unwrap()
            .rows
            .iter()
            .find(|row| row.path == b)
            .unwrap()
            .arrow_rect
            .center();
        assert_label_expanded(&app, &b, true);
        events.extend([
            egui::Event::PointerMoved(arrow),
            primary_event(arrow, true),
            primary_event(arrow, false),
        ]);
    }
    if !gamepad_y {
        events.push(tree_key_event(egui::Key::F));
    }
    navigation_tree_frame_height_before_navigation(
        &mut app,
        &ctx,
        &mut frame,
        events,
        600.0,
        |app| {
            if gamepad_y {
                // The accepted grid Y-tap handler queues this same request.
                // Keep native gamepad/foreground admission outside this fixture;
                // the name and arrow above use actual ordered pointer edges.
                app.request_folder_tree_pane_toggle(
                    crate::ui_folder_pane::FolderPaneToggleKind::CommitCursorOnClose,
                );
            }
            assert!(app.folder_tree_pane_close_navigation_pending());
            assert_eq!(
                app.folder_pane.cursor_nav_target_if_moved(),
                move_cursor.then(|| b.clone())
            );
        },
    );
    assert!(!app.settings.folder_tree_pane_visible);
    assert_eq!(app.current_folder.as_ref(), Some(&a));
    assert_eq!(app.folder_nav_history_snapshot(), history);
    assert_eq!(app.items_generation, generation);
    if move_cursor {
        assert_label_expanded(&app, &b, false);
        assert!(
            app.top_level_grid_view
                .history_navigation_transition()
                .is_none()
        );
        assert_eq!(
            app.folder_pane_open_pending
                .as_ref()
                .expect("closing must keep B's cursor navigation instead of refreshing A")
                .path,
            b
        );
        finish_label_navigation(&mut app, &ctx, &b);
        assert!(reclick_contains(&app, &b.join("only-b.jpg")));
        assert!(!reclick_contains(&app, &added));
    } else {
        // HideAndNavigate with the cursor still at A produces no close target.
        // It closes the pane and retains the independent same-location refresh.
        reclick_refresh_id(&app, &a);
        assert!(app.folder_pane_open_pending.is_none());
        settle(&mut app, &ctx);
        assert!(reclick_contains(&app, &added));
        assert_eq!(app.current_folder.as_ref(), Some(&a));
        assert_eq!(app.folder_nav_history_snapshot(), history);
    }
}

#[test]
fn folder_pane_reclick_other_row_still_navigates_without_refreshing_a() {
    let ReclickFixture {
        mut app,
        ctx,
        mut frame,
        a,
        b,
        ..
    } = reclick_fixture();
    let generation = app.items_generation;
    let pos = visible_label_click_rect(&ctx, &b).center();
    pointer_tree_click(&mut app, &ctx, &mut frame, pos);
    assert_eq!(app.current_folder.as_ref(), Some(&a));
    assert_eq!(app.items_generation, generation);
    assert!(
        app.top_level_grid_view
            .history_navigation_transition()
            .is_none()
    );
    assert_eq!(app.folder_pane_open_pending.as_ref().unwrap().path, b);
    finish_label_navigation(&mut app, &ctx, &b);
    assert!(reclick_contains(&app, &b.join("only-b.jpg")));
}
