//! Additional §1.828 handler regressions: restore, arbitration and list owners.

use super::*;
use crate::folder_pane::FolderPaneActivation;
use collection_navigation::{PhysicalHistoryPreflight, PhysicalHistoryPreflightPoll};
use std::time::{Duration, Instant};

fn reclick_stage2_fixture() -> (AppTestEnvForTest, egui::Context, u64, PathBuf, PathBuf) {
    let (mut app, ctx, mut frame, b, _) = navigation_tree_fixture();
    ctx.options_mut(|options| options.max_passes = std::num::NonZeroUsize::new(1).unwrap());
    app.active_quick_folder_slot = None;
    app.settings.sidecar_backup_enabled = false;
    app.settings.tag_sidecar_backup_enabled = false;
    app.settings.stack_script_enabled = false;
    app.settings.auto_fullscreen_image_folders = false;
    let a = app.current_folder.clone().unwrap();
    let child = a.join("child");
    std::fs::create_dir(&child).unwrap();
    image::RgbImage::new(1, 1).save(a.join("page.png")).unwrap();
    image::RgbImage::new(1, 1)
        .save(b.join("only-b.png"))
        .unwrap();
    app.load_folder(a.clone());
    settle(&mut app, &ctx);
    app.folder_pane.test_seed_listing(&a, vec![child], true);
    app.folder_pane.set_cursor(a.clone());
    app.folder_pane.set_focus_tree();
    settle_navigation_tree_scans(&mut app, &ctx, &mut frame, 600.0);
    navigation_tree_frame(&mut app, &ctx, &mut frame, vec![]);
    assert!(app.is_physical_folder_listing());
    assert_eq!(app.folder_pane.active_path(), Some(a.as_path()));
    assert!(!app.main_chrome_pointer_blocked());
    (app, ctx, frame, a, b)
}

fn reclick_stage2_refresh_request(app: &App, path: &Path) -> u64 {
    let Some(HistoryNavigationTransition::Physical(request)) =
        app.top_level_grid_view.history_navigation_transition()
    else {
        panic!("the real current-row click must admit a physical refresh");
    };
    assert_eq!(request.path.as_path(), path);
    assert!(matches!(
        request.navigation.history,
        MainHistoryOperation::SameLocation
    ));
    assert!(matches!(
        request.restore_intent,
        StartupListIntent::RefreshList { .. }
    ));
    assert!(matches!(
        request.intent,
        PhysicalHistoryIntent::Navigation {
            replay: None,
            auto_fullscreen: false,
        }
    ));
    assert!(request.dfs_continuation.is_none());
    request.request_id
}

fn reclick_stage2_no_refresh(app: &App, generation: u64) {
    assert!(
        app.top_level_grid_view
            .history_navigation_transition()
            .is_none()
    );
    assert!(app.top_level_grid_view.open_path_classification().is_none());
    assert!(app.external_rescan_pending.is_none());
    assert_eq!(app.items_generation, generation);
}

fn reclick_stage2_sidecar_fixture() -> (AppTestEnvForTest, egui::Context, PathBuf) {
    let (mut app, ctx, _, a, _) = reclick_stage2_fixture();
    // This file did not belong to the installed list. Its adjustment is present
    // only in the sidecar, so the reclick's real install must start recovery.
    image::RgbImage::new(1, 1)
        .save(a.join("external.png"))
        .unwrap();
    let mut sidecar = crate::sidecar::SidecarFile::load(&a);
    sidecar.set_adjust(
        "external.png",
        crate::adjustment::AdjustParams {
            brightness: 12.0,
            ..Default::default()
        },
    );
    assert!(sidecar.flush_blocking());
    app.settings.sidecar_backup_enabled = true;
    assert!(
        app.current_folder_last_mtime.is_some(),
        "GridReload must use its worker branch"
    );
    assert!(app.current_list_refresh_intent().imports_sidecars());
    assert!(app.sidecar_restore.is_none());
    let key = crate::adjustment_db::normalize_path(&a.join("external.png"));
    assert!(
        app.adjustment_db
            .as_ref()
            .unwrap()
            .get_page_params(&key)
            .is_none()
    );
    (app, ctx, a)
}

fn reclick_stage2_wait_for_sidecar(
    app: &mut App,
    ctx: &egui::Context,
    relay: &std::rc::Rc<sidecar_restore::SidecarCheckingRelay>,
) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while app.sidecar_restore.is_none() {
        assert!(
            app.top_level_grid_view
                .history_navigation_transition()
                .is_some(),
            "the explicit refresh must adopt into real sidecar restoration"
        );
        assert!(
            Instant::now() < deadline,
            "the explicit refresh preflight must finish"
        );
        sidecar_tree_frame(app, ctx, 10.02, vec![], Some(relay.clone()));
        std::thread::yield_now();
    }
    assert!(
        app.top_level_grid_view
            .history_navigation_transition()
            .is_none()
    );
    while !relay.captured() {
        assert!(
            Instant::now() < deadline,
            "the real checking worker must reach its relay"
        );
        sidecar_tree_frame(app, ctx, 10.03, vec![], None);
        std::thread::yield_now();
    }
}

fn reclick_stage2_assert_sidecar_imported(app: &App, folder: &Path) {
    let key = crate::adjustment_db::normalize_path(&folder.join("external.png"));
    let params = app
        .adjustment_db
        .as_ref()
        .unwrap()
        .get_page_params(&key)
        .expect("the real sidecar import must restore the external file's adjustment");
    assert_eq!(params.brightness, 12.0);
    assert!(app.adjusted_page_keys.contains(&key));
}

#[test]
fn folder_pane_reclick_grid_reload_control_starts_same_location_sidecar_restore() {
    let (mut app, ctx, a) = reclick_stage2_sidecar_fixture();
    let imports = sidecar_restore::import_starts_for_test();
    let relay = sidecar_restore::SidecarCheckingRelay::new();
    let history = app.folder_nav_history_snapshot();
    app.reload_top_level_grid(&ctx);
    reclick_stage2_refresh_request(&app, &a);
    reclick_stage2_wait_for_sidecar(&mut app, &ctx, &relay);
    assert_eq!(sidecar_restore::import_starts_for_test(), imports + 1);
    assert!(app.sidecar_restore_blocks_projected_context());
    finish_sidecar_tree(&mut app, &ctx, &relay, 10.20);
    reclick_stage2_assert_sidecar_imported(&app, &a);
    assert_eq!(app.current_folder.as_ref(), Some(&a));
    assert_eq!(app.folder_nav_history_snapshot(), history);
}

fn reclick_stage2_sidecar_scenario(clicks: usize, across: bool) {
    let (mut app, ctx, a) = reclick_stage2_sidecar_fixture();
    let pos = visible_label_click_rect(&ctx, &a).center();
    let history = app.folder_nav_history_snapshot();
    let imports = sidecar_restore::import_starts_for_test();
    #[cfg(all(windows, feature = "test-script"))]
    let loads = app.test_script_folder_load_requests;
    let relay = sidecar_restore::SidecarCheckingRelay::new();
    sidecar_tree_frame(
        &mut app,
        &ctx,
        10.0,
        vec![egui::Event::PointerMoved(pos), primary_event(pos, true)],
        None,
    );
    sidecar_tree_frame(&mut app, &ctx, 10.01, vec![primary_event(pos, false)], None);
    reclick_stage2_refresh_request(&app, &a);
    assert!(app.folder_pane_open_pending.is_none());
    assert_label_expanded(&app, &a, true);
    reclick_stage2_wait_for_sidecar(&mut app, &ctx, &relay);
    assert_eq!(sidecar_restore::import_starts_for_test(), imports + 1);
    assert!(app.sidecar_restore_blocks_projected_context());
    assert!(ctx.input(|input| input.pointer.latest_pos().is_none()));
    let state = app.sidecar_restore.as_ref().unwrap();
    let request = state.connection_request_ids_for_test();
    let cancel = state.cancel_for_test();
    let generation = app.items_generation;
    let focus = app.folder_pane.has_focus;
    let cursor = app
        .folder_pane
        .visible_rows()
        .into_iter()
        .find(|row| row.is_cursor)
        .map(|row| row.path);
    let reveal = app.folder_pane.cursor_scroll;
    let scrolls = tree_scrolls(&ctx);
    let owner = app.keyboard_owner_for_pass(&ctx);
    for click in 1..clicks {
        let time = 10.08 + (click - 1) as f64 * 0.04;
        sidecar_tree_frame(
            &mut app,
            &ctx,
            time,
            vec![
                egui::Event::PointerMoved(pos),
                primary_event(pos, true),
                egui::Event::Text("discard during recovery".into()),
            ],
            None,
        );
        assert_label_expanded(&app, &a, false);
        assert_eq!(app.items_generation, generation);
        assert_eq!(app.folder_pane.has_focus, focus);
        assert_eq!(
            app.folder_pane
                .visible_rows()
                .into_iter()
                .find(|row| row.is_cursor)
                .map(|row| row.path),
            cursor
        );
        assert_eq!(app.folder_pane.cursor_scroll, reveal);
        assert_eq!(tree_scrolls(&ctx), scrolls);
        if let Some(state) = app.sidecar_restore.as_ref() {
            assert_eq!(state.connection_request_ids_for_test(), request);
            assert!(Arc::ptr_eq(&cancel, &state.cancel_for_test()));
            assert_eq!(app.keyboard_owner_for_pass(&ctx), owner);
            assert!(!cancel.load(Ordering::Relaxed));
            ctx.input(|input| {
                assert!(
                    !input
                        .events
                        .iter()
                        .any(|event| matches!(event, egui::Event::Text(_)))
                );
            });
        }
        if across && click == 1 {
            finish_sidecar_tree(&mut app, &ctx, &relay, time + 0.005);
        }
        sidecar_tree_frame(
            &mut app,
            &ctx,
            time + 0.01,
            vec![primary_event(pos, false)],
            None,
        );
        assert_label_expanded(&app, &a, false);
        assert!(app.folder_pane_open_pending.is_none());
        assert!(
            app.top_level_grid_view
                .history_navigation_transition()
                .is_none()
        );
        assert_eq!(app.items_generation, generation);
    }
    finish_sidecar_tree(&mut app, &ctx, &relay, 10.20);
    reclick_stage2_assert_sidecar_imported(&app, &a);
    assert_eq!(sidecar_restore::import_starts_for_test(), imports + 1);
    assert_eq!(app.current_folder.as_ref(), Some(&a));
    assert_eq!(app.folder_nav_history_snapshot(), history);
    assert!(
        app.items
            .iter()
            .any(|item| { item.drag_source_path() == Some(a.join("external.png").as_path()) })
    );
    assert_label_expanded(&app, &a, false);
    #[cfg(all(windows, feature = "test-script"))]
    assert_eq!(app.test_script_folder_load_requests, loads + 1);
}

#[test]
fn folder_pane_reclick_double_during_refresh_sidecar_restore_toggles_once() {
    reclick_stage2_sidecar_scenario(2, false);
}

#[test]
fn folder_pane_reclick_triple_during_refresh_sidecar_restore_toggles_once() {
    reclick_stage2_sidecar_scenario(3, false);
}

#[test]
fn folder_pane_reclick_triple_across_refresh_sidecar_restore_toggles_once() {
    reclick_stage2_sidecar_scenario(3, true);
}

// Capture the real pointer result, then add another producer's same-pass
// candidate immediately before the production arbitration boundary.
fn reclick_stage2_competing_frame(
    app: &mut App,
    ctx: &egui::Context,
    frame: &mut u64,
    events: Vec<egui::Event>,
    mut candidates: impl FnMut(&mut update_phases::MainFrameNavigation),
) {
    *frame += 1;
    ctx.run_ui(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1600.0, 600.0),
            )),
            time: Some(*frame as f64 / 60.0),
            events,
            ..Default::default()
        },
        |ui| {
            app.input_update_begin(ctx);
            let keyboard_nav = app.handle_keyboard(ctx);
            let (toolbar_fav_nav, address_nav) = app.render_toolbar(ui);
            let folder_pane_nav = app.render_folder_pane(ui).0;
            let grid_nav = app.render_grid(ui);
            let mut navigation = update_phases::MainFrameNavigation {
                keyboard_nav,
                toolbar_fav_nav,
                address_nav,
                folder_pane_nav,
                grid_nav,
                ..empty_main_navigation()
            };
            candidates(&mut navigation);
            app.post_render_main_navigation(ctx, navigation);
            app.finish_folder_pane_presentation(ctx, true);
        },
    )
    .drop_without_applying_deltas();
}

fn reclick_stage2_losing_refresh_scenario(grid: bool) {
    let (mut app, ctx, mut frame, a, b) = reclick_stage2_fixture();
    let pos = visible_label_click_rect(&ctx, &a).center();
    let generation = app.items_generation;
    #[cfg(all(windows, feature = "test-script"))]
    let loads = app.test_script_folder_load_requests;
    navigation_tree_frame(
        &mut app,
        &ctx,
        &mut frame,
        vec![egui::Event::PointerMoved(pos), primary_event(pos, true)],
    );
    reclick_stage2_competing_frame(
        &mut app,
        &ctx,
        &mut frame,
        vec![primary_event(pos, false)],
        |navigation| {
            assert!(matches!(
                &navigation.folder_pane_nav,
                Some(FolderPaneActivation::RefreshActive(path)) if path == &a));
            let candidate =
                crate::ui_main::AddressBarNav::Direct(b.clone(), StartupListIntent::ExplicitList);
            if grid {
                navigation.grid_nav = Some(candidate);
            } else {
                navigation.address_nav = Some(candidate);
            }
        },
    );
    settle(&mut app, &ctx);
    assert_eq!(app.current_folder.as_ref(), Some(&b));
    assert!(app.items_generation > generation);
    assert!(
        app.items
            .iter()
            .any(|item| { item.drag_source_path() == Some(b.join("only-b.png").as_path()) })
    );
    assert!(
        app.top_level_grid_view
            .history_navigation_transition()
            .is_none()
    );
    #[cfg(all(windows, feature = "test-script"))]
    assert_eq!(app.test_script_folder_load_requests, loads + 1);
}

#[test]
fn folder_pane_reclick_refresh_yields_to_same_pass_address_navigation() {
    reclick_stage2_losing_refresh_scenario(false);
}

#[test]
fn folder_pane_reclick_refresh_yields_to_same_pass_grid_navigation() {
    reclick_stage2_losing_refresh_scenario(true);
}

#[test]
fn folder_pane_reclick_other_row_open_still_precedes_same_pass_grid_navigation() {
    let (mut app, ctx, mut frame, a, b) = reclick_stage2_fixture();
    let c = a.parent().unwrap().join("c");
    let pos = visible_label_click_rect(&ctx, &b).center();
    navigation_tree_frame(
        &mut app,
        &ctx,
        &mut frame,
        vec![egui::Event::PointerMoved(pos), primary_event(pos, true)],
    );
    reclick_stage2_competing_frame(
        &mut app,
        &ctx,
        &mut frame,
        vec![primary_event(pos, false)],
        |navigation| {
            assert!(matches!(
                &navigation.folder_pane_nav,
                Some(FolderPaneActivation::Open(path)) if path == &b));
            navigation.grid_nav = Some(crate::ui_main::AddressBarNav::Direct(
                c.clone(),
                StartupListIntent::ExplicitList,
            ));
        },
    );
    assert_eq!(app.current_folder.as_ref(), Some(&a));
    assert_eq!(app.folder_pane_open_pending.as_ref().unwrap().path, b);
    finish_label_navigation(&mut app, &ctx, &b);
    assert_eq!(app.current_folder.as_ref(), Some(&b));
}

#[test]
fn folder_pane_reclick_externally_removed_current_folder_keeps_failed_list_and_prunes_tree() {
    let (mut app, ctx, mut frame, a, _) = reclick_stage2_fixture();
    let root = a.parent().unwrap().to_path_buf();
    let pos = visible_label_click_rect(&ctx, &a).center();
    let generation = app.items_generation;
    let items = app.items.clone();
    let history = app.folder_nav_history_snapshot();
    let warnings = app.feedback_toasts_for_test.len();
    // Only this fixture's disposable directory is removed; no product deletion
    // handler or metadata write participates in the external-change scenario.
    std::fs::remove_dir_all(&a).unwrap();
    pointer_tree_click(&mut app, &ctx, &mut frame, pos);
    reclick_stage2_refresh_request(&app, &a);
    settle(&mut app, &ctx);
    settle_navigation_tree_scans(&mut app, &ctx, &mut frame, 600.0);
    assert_eq!(app.current_folder.as_ref(), Some(&a));
    assert_eq!(app.items, items);
    assert_eq!(app.items_generation, generation);
    assert_eq!(app.folder_nav_history_snapshot(), history);
    assert!(
        app.feedback_toasts_for_test.len() > warnings,
        "existing list failure must be reported"
    );
    assert!(!app.folder_pane.test_children(&root).contains(&a));
    assert!(
        app.folder_pane
            .visible_rows()
            .iter()
            .all(|row| !row.path.starts_with(&a))
    );
    assert!(app.folder_pane_open_pending.is_none());
    assert!(
        app.top_level_grid_view
            .history_navigation_transition()
            .is_none()
    );
}

fn reclick_stage2_aggregate_fixture() -> (AppTestEnvForTest, egui::Context, u64, PathBuf) {
    let (mut app, ctx, mut frame, a, _) = reclick_stage2_fixture();
    std::fs::remove_file(a.join("page.png")).unwrap();
    for group in [200, 300, 400, 500, 600, 700] {
        for page in 0..2 {
            image::RgbImage::new(1, 1)
                .save(a.join(format!("{group}_p{page}.png")))
                .unwrap();
        }
    }
    app.stack_mode_requested = true;
    app.load_folder(a.clone());
    settle(&mut app, &ctx);
    assert!(app.stack_mode_aggregated());
    assert!(
        app.items
            .iter()
            .any(|item| matches!(item, GridItem::Stack { .. }))
    );
    navigation_tree_frame(&mut app, &ctx, &mut frame, vec![]);
    (app, ctx, frame, a)
}

fn reclick_stage2_aggregate_result(
    reclick: bool,
) -> (Vec<String>, Option<usize>, Option<u32>, f32) {
    let (mut app, ctx, mut frame, a) = reclick_stage2_aggregate_fixture();
    let selected = app
        .items
        .iter()
        .position(|item| {
            matches!(item,
        GridItem::Stack { key, .. } if key == "400")
        })
        .unwrap();
    app.selected = Some(selected);
    let context = app.projected_viewer_context_id();
    let generation = app.items_generation;
    let content_h = (app.items.len() + 2) as f32 * 100.0;
    let capture_geometry = |app: &mut App| {
        app.grid_scroll.record_thumbnail_geometry(
            GridScrollGeometry::new(1, 100.0, 200.0, content_h),
            context,
            generation,
            100.0,
        );
        app.last_cell_size = 100.0;
        app.grid_scroll.set_position((selected - 1) as f32 * 100.0);
        app.grid_scroll.set_row_anchor(None);
    };
    for page in 0..2 {
        image::RgbImage::new(1, 1)
            .save(a.join(format!("100_p{page}.png")))
            .unwrap();
    }
    app.current_folder_last_mtime = Some(SystemTime::UNIX_EPOCH);
    let history = app.folder_nav_history_snapshot();
    if reclick {
        let pos = visible_label_click_rect(&ctx, &a).center();
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
            capture_geometry,
        );
        reclick_stage2_refresh_request(&app, &a);
    } else {
        capture_geometry(&mut app);
        app.reload_top_level_grid(&ctx);
    }
    settle(&mut app, &ctx);
    assert!(app.stack_mode_aggregated());
    assert_eq!(app.folder_nav_history_snapshot(), history);
    assert!(
        app.items
            .iter()
            .any(|item| matches!(item, GridItem::Stack { key, .. } if key == "100"))
    );
    (
        app.items
            .iter()
            .map(|item| item.name().into_owned())
            .collect(),
        app.selected,
        app.grid_scroll.row_anchor(),
        app.grid_scroll.offset(),
    )
}

#[test]
fn folder_pane_reclick_aggregate_selection_and_anchor_match_grid_reload() {
    assert_eq!(
        reclick_stage2_aggregate_result(true),
        reclick_stage2_aggregate_result(false)
    );
}

#[test]
fn folder_pane_reclick_initial_stack_preparation_is_not_replaced() {
    let (mut app, ctx, mut frame, a, _) = reclick_stage2_fixture();
    app.stack_mode_requested = true;
    app.spawn_stack_script_worker(
        a.clone(),
        crate::filename_stack_ui::StackListingSource::Normal(
            Arc::new(Vec::new()),
            crate::filename_stack_ui::StackRatingSource::Standard,
        ),
        '_',
        crate::rating_sort::ListingOrderRequest::Standard(crate::settings::SortOrder::FileName),
        Default::default(),
        None,
        false,
        false,
    );
    assert!(matches!(
        app.stack_script_pending,
        Some(crate::filename_stack_ui::StackPreparePending::Extracting(_))
    ));
    let generation = app.items_generation;
    let history = app.folder_nav_history_snapshot();
    let pos = visible_label_click_rect(&ctx, &a).center();
    pointer_tree_click(&mut app, &ctx, &mut frame, pos);
    reclick_stage2_no_refresh(&app, generation);
    assert!(matches!(
        app.stack_script_pending,
        Some(crate::filename_stack_ui::StackPreparePending::Extracting(_))
    ));
    assert_eq!(app.folder_nav_history_snapshot(), history);
    assert!(app.folder_pane_open_pending.is_none());
    app.cancel_stack_script_pending();
}

#[test]
fn folder_pane_reclick_aggregate_return_preparation_is_not_replaced() {
    let (mut app, ctx, mut frame, a) = reclick_stage2_aggregate_fixture();
    let view = Arc::clone(app.stack_view.as_ref().unwrap());
    app.stack_return_state = Some(crate::filename_stack_ui::StackReturnState::Refreshing {
        view: Arc::clone(&view),
    });
    app.stack_reconcile_after_fullscreen_close(&ctx);
    let pending = app
        .stack_script_pending
        .as_ref()
        .expect("real aggregate-return worker");
    assert!(matches!(
        pending,
        crate::filename_stack_ui::StackPreparePending::Switching(_)
    ));
    assert!(
        !pending.open_pending(),
        "a passive return is still a preparation owner"
    );
    assert!(!app.stack_showing_flat);
    assert!(app.fullscreen_idx.is_none());
    let generation = app.items_generation;
    let items = app.items.clone();
    let pos = visible_label_click_rect(&ctx, &a).center();
    pointer_tree_click(&mut app, &ctx, &mut frame, pos);
    reclick_stage2_no_refresh(&app, generation);
    assert_eq!(app.items, items);
    assert!(matches!(
        app.stack_script_pending,
        Some(crate::filename_stack_ui::StackPreparePending::Switching(_))
    ));
    assert!(Arc::ptr_eq(app.stack_view.as_ref().unwrap(), &view));
    assert!(app.folder_pane_open_pending.is_none());
    settle(&mut app, &ctx);
    assert!(app.stack_mode_aggregated());
}

#[cfg(windows)]
#[test]
fn folder_pane_reclick_linked_reader_is_preserved_without_refresh() {
    let (mut app, ctx, mut frame, a, _) = reclick_stage2_fixture();
    app.settings.detached_viewer_enabled = true;
    app.settings.detached_viewer_open_images_in_window = false;
    app.fullscreen_idx = app
        .items
        .iter()
        .position(|item| matches!(item, GridItem::Image(_)));
    app.viewer_presentation = ViewerPresentation::DetachedWindow;
    app.fs_viewport_shown = true;
    app.fs_viewport_presentation = Some(ViewerPresentation::DetachedWindow);
    let window = 828_201;
    app.begin_mounted_detached_session_for_test(window, DetachedSource::Image);
    app.update_detached_window_runtime_flags(window, true, "reclick_linked_fixture");
    assert!(app.projected_viewer_session_open_for(SourceSwapOwnership::MountedContext));
    let session = app.active_detached_window_id();
    let index = app.fullscreen_idx;
    let generation = app.items_generation;
    let token = app.cancel_token.clone();
    let history = app.folder_nav_history_snapshot();
    let pos = visible_label_click_rect(&ctx, &a).center();
    pointer_tree_click(&mut app, &ctx, &mut frame, pos);
    reclick_stage2_no_refresh(&app, generation);
    assert_eq!(app.fullscreen_idx, index);
    assert_eq!(app.viewer_presentation, ViewerPresentation::DetachedWindow);
    assert_eq!(app.active_detached_window_id(), session);
    assert_eq!(
        app.locate_window_context(window).unwrap().0,
        app.viewer_context_main()
    );
    assert!(Arc::ptr_eq(&app.cancel_token, &token));
    assert!(!token.load(Ordering::Relaxed));
    assert_eq!(app.folder_nav_history_snapshot(), history);
}

#[cfg(windows)]
#[test]
fn folder_pane_reclick_independent_reader_does_not_block_or_change_main_refresh() {
    let (mut app, ctx, mut frame, a, _) = reclick_stage2_fixture();
    let window = 828_202;
    let image = app
        .items
        .iter()
        .position(|item| matches!(item, GridItem::Image(_)))
        .unwrap();
    let independent = app.fork_materialized_still_context(&a, image);
    app.bind_window(independent, window).unwrap();
    app.update_detached_window_runtime_flags(window, false, "reclick_independent_fixture");
    let sibling = app
        .with_viewer_context(independent, |owner| {
            owner.fullscreen_idx = owner
                .items
                .iter()
                .position(|item| matches!(item, GridItem::Image(_)));
            assert!(owner.fullscreen_idx.is_some());
            owner.viewer_presentation = ViewerPresentation::DetachedWindow;
            owner.detached_viewer_independent_active = true;
            assert!(owner.projected_viewer_session_open_for(SourceSwapOwnership::MountedContext));
            (
                owner.items.clone(),
                owner.items_generation,
                owner.cancel_token.clone(),
                owner.fullscreen_idx,
            )
        })
        .unwrap();
    assert!(!app.projected_viewer_session_open_for(SourceSwapOwnership::MountedContext));
    let added = a.join("external-independent.png");
    image::RgbImage::new(1, 1).save(&added).unwrap();
    let history = app.folder_nav_history_snapshot();
    let pos = visible_label_click_rect(&ctx, &a).center();
    pointer_tree_click(&mut app, &ctx, &mut frame, pos);
    reclick_stage2_refresh_request(&app, &a);
    settle(&mut app, &ctx);
    assert!(
        app.items
            .iter()
            .any(|item| item.drag_source_path() == Some(added.as_path()))
    );
    assert_eq!(app.folder_nav_history_snapshot(), history);
    assert_eq!(app.locate_window_context(window).unwrap().0, independent);
    assert!(!app.detached_window_manager.runtime(window).unwrap().linked);
    app.with_viewer_context(independent, |owner| {
        assert_eq!(owner.items, sibling.0);
        assert_eq!(owner.items_generation, sibling.1);
        assert!(Arc::ptr_eq(&owner.cancel_token, &sibling.2));
        assert!(!sibling.2.load(Ordering::Relaxed));
        assert_eq!(owner.fullscreen_idx, sibling.3);
        assert_eq!(
            owner.viewer_presentation,
            ViewerPresentation::DetachedWindow
        );
    })
    .unwrap();
}

#[test]
fn folder_pane_reclick_rating_physical_refresh_keeps_first_pending_request_after_delay() {
    let (mut app, ctx, mut frame, a, _) = reclick_stage2_fixture();
    let meta = crate::rating_db::RatingMeta::new(crate::rating_db::RatingItemKind::Folder)
        .with_source_path(&a);
    app.rating_db
        .as_ref()
        .unwrap()
        .set_user_rating(&crate::adjustment_db::normalize_path(&a), 4, Some(&meta))
        .unwrap();
    app.enter_rating_view_from_menu(4);
    let deadline = Instant::now() + Duration::from_secs(20);
    while app.rating_view_pending.is_some() || app.sidecar_restore_active() {
        assert!(
            Instant::now() < deadline,
            "the real rating root must settle"
        );
        app.poll_sidecar_restore(&ctx);
        app.poll_rating_view();
        std::thread::yield_now();
    }
    assert!(app.items_are_rating_view);
    assert!(
        app.items
            .iter()
            .any(|item| item.drag_source_path() == Some(a.as_path()))
    );
    let owner = app.rating_view_physical_load_owner(&a).unwrap();
    assert!(app.start_rating_physical_open(owner, StartupListIntent::ExplicitList));
    settle(&mut app, &ctx);
    assert_eq!(app.current_folder.as_ref(), Some(&a));
    assert!(matches!(
        app.folder_nav_current_target(),
        Some(FolderNavHistoryTarget::RatingPhysical(_))
    ));
    assert!(app.is_physical_folder_listing());
    app.folder_pane.set_cursor(a.clone());
    app.folder_pane.set_focus_tree();
    settle_navigation_tree_scans(&mut app, &ctx, &mut frame, 600.0);
    navigation_tree_frame(&mut app, &ctx, &mut frame, vec![]);
    let pos = visible_label_click_rect(&ctx, &a).center();
    let first_file = a.join("first-rating-refresh.png");
    image::RgbImage::new(1, 1).save(&first_file).unwrap();
    app.current_folder_last_mtime = None;
    let history = app.folder_nav_history_snapshot();
    let location = app.folder_nav_current_target();
    let generation = app.items_generation;
    let scans = folder_scan::caller_scan_count_for_test();
    pointer_tree_click(&mut app, &ctx, &mut frame, pos);
    let Some(HistoryNavigationTransition::Physical(request)) =
        app.top_level_grid_view.history_navigation_transition_mut()
    else {
        panic!("the first real reclick must admit the existing Rating refresh owner");
    };
    assert_eq!(request.path, a);
    assert!(matches!(
        &request.intent,
        PhysicalHistoryIntent::Rating {
            owner,
            restore: None,
            replay: None,
            zip_dir_prefix: None,
        } if owner.intent == RatingPhysicalLoadIntent::Refresh && owner.target_path == a
    ));
    assert!(matches!(
        request.navigation.history,
        MainHistoryOperation::SameLocation
    ));
    assert!(matches!(
        request.restore_intent,
        StartupListIntent::RefreshList { .. }
    ));
    let first = request.request_id;
    let PhysicalHistoryPhase::Preflighting { preflight, .. } = &mut request.phase else {
        panic!("Rating refresh must use the existing worker preflight");
    };
    let (waiting, sender) = PhysicalHistoryPreflight::controlled_for_test();
    let mut worker = std::mem::replace(preflight, waiting);
    assert_eq!(app.items_generation, generation);
    assert_eq!(folder_scan::caller_scan_count_for_test(), scans);
    let deadline = Instant::now() + Duration::from_secs(20);
    let payload = loop {
        assert!(
            Instant::now() < deadline,
            "the first real Rating scan must finish"
        );
        match worker.poll() {
            PhysicalHistoryPreflightPoll::Ready(payload) => break payload,
            PhysicalHistoryPreflightPoll::Failed(message) => panic!("Rating refresh: {message}"),
            PhysicalHistoryPreflightPoll::Pending => std::thread::yield_now(),
        }
    };
    assert!(matches!(
        &payload,
        collection_navigation::PhysicalHistoryPreflightPayload::Folder(_)
    ));
    // Preserve the real first scan, but hold its delivery across a distinct
    // Single. §14.7 intentionally leaves the later file for a future refresh.
    let later_file = a.join("later-rating-refresh.png");
    image::RgbImage::new(1, 1).save(&later_file).unwrap();
    frame += (ctx.options(|options| options.input_options.max_double_click_delay) * 60.0).ceil()
        as u64
        + 2;
    navigation_tree_frame(
        &mut app,
        &ctx,
        &mut frame,
        vec![egui::Event::PointerMoved(pos), primary_event(pos, true)],
    );
    reclick_stage2_competing_frame(
        &mut app,
        &ctx,
        &mut frame,
        vec![primary_event(pos, false)],
        |navigation| {
            assert!(matches!(
                &navigation.folder_pane_nav,
                Some(FolderPaneActivation::RefreshActive(path)) if path == &a
            ));
        },
    );
    let Some(HistoryNavigationTransition::Physical(request)) =
        app.top_level_grid_view.history_navigation_transition()
    else {
        panic!("the refused reclick must keep the first Rating refresh pending");
    };
    assert_eq!(request.request_id, first);
    assert!(matches!(
        &request.intent,
        PhysicalHistoryIntent::Rating { owner, .. }
            if owner.intent == RatingPhysicalLoadIntent::Refresh
    ));
    assert_eq!(app.items_generation, generation);
    assert_eq!(folder_scan::caller_scan_count_for_test(), scans);
    assert_eq!(app.folder_nav_history_snapshot(), history);
    assert_eq!(app.folder_nav_current_target(), location);
    assert_label_expanded(&app, &a, true);
    assert!(app.folder_pane_open_pending.is_none());
    assert!(
        sender.send(Ok(payload)).is_ok(),
        "the first receiver must remain live"
    );
    settle(&mut app, &ctx);
    assert!(app.items_generation > generation);
    assert_eq!(app.current_folder.as_ref(), Some(&a));
    assert!(
        app.items
            .iter()
            .any(|item| item.drag_source_path() == Some(first_file.as_path()))
    );
    assert!(
        !app.items
            .iter()
            .any(|item| item.drag_source_path() == Some(later_file.as_path()))
    );
    assert_eq!(app.folder_nav_history_snapshot(), history);
    assert_eq!(app.folder_nav_current_target(), location);
    assert!(
        app.top_level_grid_view
            .history_navigation_transition()
            .is_none()
    );
    assert!(app.is_physical_folder_listing());
}

#[test]
fn folder_pane_reclick_touch_double_tap_refreshes_each_time_without_toggle() {
    let (mut app, ctx, mut frame, a, _) = reclick_stage2_fixture();
    let pos = visible_label_click_rect(&ctx, &a).center();
    let touch = |phase| egui::Event::Touch {
        device_id: egui::TouchDeviceId(828),
        id: egui::TouchId(1),
        phase,
        pos,
        force: None,
    };
    let history = app.folder_nav_history_snapshot();
    let generation = app.items_generation;
    let start = frame;
    let mut previous = None;
    #[cfg(all(windows, feature = "test-script"))]
    let loads = app.test_script_folder_load_requests;
    for tap in 1..=2 {
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
        if let Some(request) = previous {
            assert_eq!(reclick_stage2_refresh_request(&app, &a), request);
        } else {
            assert!(
                app.top_level_grid_view
                    .history_navigation_transition()
                    .is_none()
            );
        }
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
        let request = reclick_stage2_refresh_request(&app, &a);
        assert_ne!(
            Some(request),
            previous,
            "tap {tap} must admit its own Single refresh"
        );
        previous = Some(request);
        navigation_tree_frame(&mut app, &ctx, &mut frame, vec![]);
        assert_eq!(reclick_stage2_refresh_request(&app, &a), request);
        assert_label_expanded(&app, &a, true);
        assert!(app.folder_pane_open_pending.is_none());
        assert_eq!(app.items_generation, generation);
        assert_eq!(app.folder_nav_history_snapshot(), history);
        #[cfg(all(windows, feature = "test-script"))]
        assert_eq!(app.test_script_folder_load_requests, loads + tap);
    }
    assert!(
        (frame - start) as f64 / 60.0
            < ctx.options(|options| options.input_options.max_double_click_delay),
        "the two real touch sequences must fit inside the double-click interval"
    );
    settle(&mut app, &ctx);
    assert_eq!(app.current_folder.as_ref(), Some(&a));
    assert_eq!(app.folder_nav_history_snapshot(), history);
    assert_label_expanded(&app, &a, true);
}
