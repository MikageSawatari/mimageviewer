//! Tag result identity belongs to the opened root row, including nested books.
//! All input is synthetic; these tests never poll the desktop's keyboard state.
use super::*;
use tests::phase_c_support::setup_app;
use top_level_grid_view::{TopLevelGridSurface, TopLevelSearchView};

fn tag_rows(app: &mut App, rows: Vec<GridItem>) {
    app.active_quick_folder_slot = None;
    app.settings.auto_fullscreen_zip_pdf = false;
    app.settings.auto_fullscreen_image_folders = false;
    app.tag_view.active = true;
    app.tag_view.run = 7;
    app.tag_view.query = "#chosen".into();
    app.tag_view.last_executed = "#chosen".into();
    app.tag_view.nav_stack.clear();
    app.items_are_tag_view = true;
    app.items = rows;
    app.visible_indices = (0..app.items.len()).collect();
    app.image_metas = vec![None; app.items.len()];
    app.thumbnails = vec![ThumbnailState::Pending; app.items.len()];
    app.selected = Some(0);
    app.top_level_grid_view
        .replace_surface(TopLevelGridSurface::Search(TopLevelSearchView::Tag));
}

fn show_book(app: &mut App, path: &Path, fullscreen: bool) {
    app.current_folder = Some(path.to_path_buf());
    app.archive_source_override = None;
    app.items_are_tag_view = false;
    app.items = vec![GridItem::Image(path.join("page.png"))];
    app.image_metas = vec![None];
    app.thumbnails = vec![ThumbnailState::Pending];
    app.visible_indices = vec![0];
    app.fullscreen_idx = fullscreen.then_some(0);
    app.tag_view.nav_stack = vec![path.to_path_buf()];
}

fn fixture(app: &mut App, root: &Path, fullscreen: bool) -> (PathBuf, PathBuf) {
    // The physical name order is deliberately opposite to the frozen result order.
    let a = root.join("z-first");
    let b = root.join("a-next");
    let empty = root.join("empty");
    for folder in [&a, &b, &empty] {
        std::fs::create_dir_all(folder).unwrap();
    }
    for folder in [&a, &b] {
        image::RgbImage::new(2, 2)
            .save(folder.join("page.png"))
            .unwrap();
    }
    tag_rows(
        app,
        vec![
            GridItem::Folder(a.clone()),
            GridItem::Folder(empty),
            GridItem::Folder(b.clone()),
        ],
    );
    app.capture_main_folder_list_order(0);
    show_book(app, &a, fullscreen);
    (a, b)
}

fn receive(app: &mut App) -> FolderNavResult {
    let pending = app
        .folder_nav_pending
        .take()
        .expect("accepted tag navigation");
    let result = pending
        .rx
        .recv_timeout(std::time::Duration::from_secs(15))
        .expect("tag list-order worker result");
    FolderNavResult {
        target: result.target,
        hit_image_folder: result.hit_image_folder,
        forward: pending.forward,
        mode: pending.mode,
        queued_steps: app.pending_folder_nav_steps,
    }
}

fn assert_request(app: &App, forward: bool, fullscreen: bool, index: usize) {
    let pending = app.folder_nav_pending.as_ref().expect("accepted request");
    assert_eq!(pending.forward, forward);
    assert!(matches!(&pending.mode,
        FolderNavMode::ListOrder { snapshot, from_index, fullscreen: actual }
        if snapshot.scope == (ListOrderScope::Tag { run: app.tag_view.run })
            && *from_index == index && *actual == fullscreen));
    assert_eq!(app.pending_folder_nav_steps, 0);
}

fn write_zip(path: &Path) {
    let mut zip = zip::ZipWriter::new(std::fs::File::create(path).unwrap());
    zip.start_file("001.png", zip::write::SimpleFileOptions::default())
        .unwrap();
    std::io::Write::write_all(&mut zip, b"page").unwrap();
    zip.finish().unwrap();
}

fn settle(app: &mut App, ctx: &egui::Context) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
    loop {
        app.settle_open_path_classification_for_test();
        app.poll_history_navigation_transition_for_test(ctx);
        app.poll_zip_enumerate();
        app.poll_pdf_enumerate();
        app.poll_sidecar_restore(ctx);
        app.poll_book_composition_service_for_test(ctx);
        app.poll_book_presentation_permission(ctx);
        if app.top_level_grid_view.open_path_classification().is_none()
            && app
                .top_level_grid_view
                .history_navigation_transition()
                .is_none()
            && app.zip_enumerate_pending.is_none()
            && app.pdf_enumerate_pending.is_none()
            && !app.sidecar_restore_active()
        {
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "tag navigation did not settle"
        );
        std::thread::yield_now();
    }
}

#[test]
fn tag_list_order_capture_only_result_books_and_preserve_nested_snapshot() {
    let mut app = setup_app();
    let root = app.tmp.path().to_path_buf();
    let parent = root.join("parent");
    let nested = parent.join("nested");
    let archive = root.join("book.zip");
    tag_rows(
        &mut app,
        vec![
            GridItem::Folder(parent.clone()),
            GridItem::Image(root.join("image.png")),
            GridItem::Folder(nested.clone()),
            GridItem::ZipFile(archive.clone()),
            GridItem::Video(root.join("video.mp4")),
            GridItem::Audio(root.join("audio.mp3")),
        ],
    );
    app.visible_indices = vec![3, 1, 2, 4, 0, 5];
    let _ = app.grid_physical_navigation(0, parent.clone(), false);
    let snapshot = app.list_order.clone().unwrap();
    assert_eq!(snapshot.scope, ListOrderScope::Tag { run: 7 });
    assert_eq!(snapshot.owner, app.viewer_context_main());
    assert_eq!(
        snapshot
            .entries
            .iter()
            .map(|entry| entry.path.clone())
            .collect::<Vec<_>>(),
        vec![archive, nested.clone(), parent.clone()]
    );
    show_book(&mut app, &parent, false);
    app.record_tag_view_nav_open(&nested);
    app.current_folder = Some(nested.clone());
    app.items = vec![GridItem::Folder(nested.join("child"))];
    app.visible_indices = vec![0];
    for result_bit in [false, true] {
        app.items_are_tag_view = result_bit;
        let child = app.items[0].drag_source_path().unwrap().to_path_buf();
        let _ = app.grid_physical_navigation(0, child, false);
        assert!(Arc::ptr_eq(app.list_order.as_ref().unwrap(), &snapshot));
        assert_eq!(
            app.list_order_position().unwrap().1,
            2,
            "root, not nearest ancestor"
        );
    }
}

#[test]
fn tag_list_order_grid_and_fullscreen_handlers_follow_results_in_both_directions() {
    for route in 0..4 {
        for forward in [false, true] {
            let mut app = setup_app();
            let root = app.tmp.path().to_path_buf();
            let fullscreen = route != 0;
            let (a, b) = fixture(&mut app, &root, fullscreen);
            if !forward {
                show_book(&mut app, &b, fullscreen);
            }
            let expected = if forward { b } else { a };
            let ctx = egui::Context::default();
            match route {
                0 => assert!(app.route_tag_view_grid_book_navigation(&ctx, forward)),
                1 => app.handle_fullscreen_ctrl_nav_context(&ctx, 0, forward, false),
                2 => app.handle_fullscreen_sibling_nav_context(&ctx, 0, forward, false),
                _ => app.handle_fullscreen_list_order_nav_context(&ctx, 0, forward, false),
            }
            assert_request(&app, forward, fullscreen, if forward { 0 } else { 2 });
            let result = receive(&mut app);
            assert_eq!(result.target.as_ref().unwrap().logical_source(), expected);
            app.apply_folder_nav_result(&ctx, result);
            app.finish_page_identity_preparation_for_test();
            assert_eq!(app.tag_view.nav_stack, vec![expected.clone()]);
            assert_eq!(app.effective_folder(), Some(expected));
            assert_eq!(app.fullscreen_idx.is_some(), fullscreen);
        }
    }
}

#[test]
fn tag_list_order_root_absent_keeps_result_grid_and_shows_existing_fullscreen_guidance() {
    for route in 0..4 {
        let mut app = setup_app();
        let root = app.tmp.path().to_path_buf();
        fixture(&mut app, &root, route != 0);
        app.tag_view.nav_stack.clear();
        let ctx = egui::Context::default();
        let expected = match route {
            0 => {
                assert!(app.route_tag_view_grid_book_navigation(&ctx, true));
                None
            }
            1 => {
                app.handle_fullscreen_ctrl_nav_context(&ctx, 0, true, false);
                Some("検索結果を開いてからCtrl+↑↓で移動できます")
            }
            2 => {
                app.handle_fullscreen_sibling_nav_context(&ctx, 0, true, false);
                Some("検索中は兄弟フォルダ移動しません")
            }
            _ => {
                app.handle_fullscreen_list_order_nav_context(&ctx, 0, true, false);
                Some("一覧から開いた本で使えます")
            }
        };
        assert!(app.list_order_position().is_none());
        assert!(app.folder_nav_pending.is_none(), "no physical navigation");
        let actual = match app.fs_boundary_hint {
            Some(crate::ui_fullscreen::FsBoundaryHint::NavNoOp { reason, .. }) => {
                Some(App::nav_noop_title(reason))
            }
            _ => app.fs_feedback_toast.as_ref().map(|toast| toast.0.as_str()),
        };
        assert_eq!(actual, expected);
    }
}

#[test]
fn tag_list_order_invalid_root_path_and_run_never_fall_back_to_physical_navigation() {
    for invalid in 0..4 {
        for fullscreen in [false, true] {
            let mut app = setup_app();
            let root = app.tmp.path().to_path_buf();
            let (a, _) = fixture(&mut app, &root, fullscreen);
            match invalid {
                0 => app.current_folder = Some(root.join("outside")),
                // Prefixes are not descendants at a path-component boundary.
                1 => app.current_folder = Some(root.join("z-first-other")),
                2 => app.tag_view.nav_stack = vec![root.join("not-a-result")],
                _ => app.tag_view.run += 1,
            }
            let ctx = egui::Context::default();
            assert!(app.list_order_position().is_none(), "{a:?}");
            if fullscreen {
                app.handle_fullscreen_ctrl_nav_context(&ctx, 0, true, false);
            } else {
                assert!(app.route_tag_view_grid_book_navigation(&ctx, true));
            }
            assert!(app.folder_nav_pending.is_none());
            assert_eq!(
                app.fs_feedback_toast.as_ref().unwrap().0,
                "一覧から開いた本で使えます"
            );
        }
    }
}

#[test]
fn tag_list_order_boundary_does_not_wrap_or_replace_the_root() {
    for fullscreen in [false, true] {
        for forward in [false, true] {
            let mut app = setup_app();
            let root = app.tmp.path().to_path_buf();
            let (a, b) = fixture(&mut app, &root, fullscreen);
            let current = if forward { b } else { a };
            show_book(&mut app, &current, fullscreen);
            let snapshot = app.list_order.clone().unwrap();
            let ctx = egui::Context::default();
            if fullscreen {
                app.handle_fullscreen_ctrl_nav_context(&ctx, 0, forward, false);
            } else {
                app.route_tag_view_grid_book_navigation(&ctx, forward);
            }
            let result = receive(&mut app);
            assert!(result.target.is_none());
            app.apply_folder_nav_result(&ctx, result);
            assert_eq!(app.tag_view.nav_stack, vec![current.clone()]);
            assert_eq!(app.current_folder, Some(current));
            assert!(Arc::ptr_eq(app.list_order.as_ref().unwrap(), &snapshot));
            assert!(app.folder_nav_pending.is_none());
            if fullscreen {
                assert!(matches!(app.fs_boundary_hint,
                    Some(crate::ui_fullscreen::FsBoundaryHint::Edge { at_end, .. })
                    if at_end == forward));
            } else {
                assert!(
                    app.fs_feedback_toast.is_some(),
                    "grid boundary notification"
                );
            }
        }
    }
}

#[test]
fn tag_list_order_adoption_replaces_only_matching_run_and_row() {
    let mut app = setup_app();
    let root = app.tmp.path().to_path_buf();
    let (a, b) = fixture(&mut app, &root, false);
    let mode = FolderNavMode::ListOrder {
        snapshot: app.list_order.clone().unwrap(),
        from_index: 0,
        fullscreen: false,
    };
    let admission = app.tag_view_nav_admission(&mode, &b);
    assert_eq!(
        app.tag_view.nav_stack,
        vec![a.clone()],
        "preparation cannot mutate return route"
    );
    app.commit_staged_search_navigation_effects(&b, false, admission, None);
    assert_eq!(app.tag_view.nav_stack, vec![b.clone()]);
    assert!(app.address.ends_with("a-next"));
    for admission in [
        TagViewNavAdmission::ListOrderRow {
            run: 6,
            row: b.clone(),
        },
        TagViewNavAdmission::ListOrderRow {
            run: 7,
            row: root.join("different-row"),
        },
        TagViewNavAdmission::Descend,
    ] {
        app.tag_view.nav_stack = vec![a.clone()];
        app.commit_staged_search_navigation_effects(&b, false, Some(admission), None);
        assert_eq!(app.tag_view.nav_stack, vec![a.clone(), b.clone()]);
    }
}

#[test]
fn tag_list_order_back_requeries_and_selects_exact_same_named_row() {
    let mut app = setup_app();
    let root = app.tmp.path().to_path_buf();
    let a = root.join("A/book.zip");
    let b = root.join("B/book.zip");
    tag_rows(
        &mut app,
        vec![GridItem::ZipFile(a.clone()), GridItem::ZipFile(b.clone())],
    );
    app.capture_main_folder_list_order(0);
    show_book(&mut app, &b, false);
    let old_run = app.tag_view.run;
    app.tag_view_back();
    assert!(app.tag_view.nav_stack.is_empty());
    assert!(app.tag_view_pending.is_some());
    assert_eq!(app.tag_view.run, old_run + 1);
    assert_eq!(
        app.select_after_load,
        Some(AfterLoadSelection::ExactPath(b.clone()))
    );
    assert!(
        app.list_order_position().is_none(),
        "old snapshot cannot be reused after requery"
    );
    app.items = vec![GridItem::ZipFile(a), GridItem::ZipFile(b.clone())];
    app.visible_indices = vec![0, 1];
    app.thumbnails = vec![ThumbnailState::Pending; 2];
    app.image_metas = vec![None; 2];
    assert!(app.try_select_after_load());
    assert_eq!(app.selected, Some(1));
    let run = app.tag_view.run;
    app.dismiss_tag_view_without_restore();
    assert_eq!(
        app.tag_view.run, run,
        "dismiss must not reset the monotonic run"
    );
}

#[test]
fn tag_list_order_deep_descents_keep_original_root_even_when_nested_rows_are_results() {
    let mut app = setup_app();
    let root = app.tmp.path().to_path_buf();
    let outer = root.join("outer");
    let inner = outer.join("inner");
    tag_rows(
        &mut app,
        vec![
            GridItem::Folder(inner.clone()),
            GridItem::Folder(outer.clone()),
        ],
    );
    app.capture_main_folder_list_order(1);
    show_book(&mut app, &outer, false);
    app.record_tag_view_nav_open(&inner);
    let mut current = inner;
    for _ in 0..101 {
        current.push("child");
        app.record_tag_view_nav_open(&current);
    }
    app.current_folder = Some(current);
    assert_eq!(app.tag_view.nav_stack.len(), MAX_FOLDER_NAV_STACK);
    assert_eq!(app.tag_view.nav_stack.first(), Some(&outer));
    assert_eq!(app.list_order_position().unwrap().1, 1);
}

#[test]
fn tag_list_order_grid_pending_rejects_without_cancel_restart_or_accumulation() {
    let mut app = setup_app();
    let root = app.tmp.path().to_path_buf();
    fixture(&mut app, &root, false);
    let ctx = egui::Context::default();
    app.route_tag_view_grid_book_navigation(&ctx, true);
    let cancel = app.folder_nav_pending.as_ref().unwrap().cancel.clone();
    for forward in [false, true] {
        assert!(app.route_tag_view_grid_book_navigation(&ctx, forward));
        assert!(Arc::ptr_eq(
            &app.folder_nav_pending.as_ref().unwrap().cancel,
            &cancel
        ));
        assert!(!cancel.load(Ordering::Relaxed));
        assert_eq!(app.pending_folder_nav_steps, 0);
    }
}

#[test]
fn tag_list_order_grid_zip_adoption_gate_keeps_owner_and_commits_new_root() {
    let mut app = setup_app();
    let root = app.tmp.path().to_path_buf();
    let (a, _) = fixture(&mut app, &root, false);
    let zip = root.join("next.zip");
    write_zip(&zip);
    tag_rows(
        &mut app,
        vec![GridItem::Folder(a.clone()), GridItem::ZipFile(zip.clone())],
    );
    app.capture_main_folder_list_order(0);
    show_book(&mut app, &a, false);
    let ctx = egui::Context::default();
    app.route_tag_view_grid_book_navigation(&ctx, true);
    let result = receive(&mut app);
    app.apply_folder_nav_result(&ctx, result);
    assert!(app.main_list_folder_navigation_adoption_pending());
    assert_eq!(app.tag_view.nav_stack, vec![a.clone()]);
    let request_id = match app.top_level_grid_view.history_navigation_transition() {
        Some(HistoryNavigationTransition::Physical(request)) => request.request_id,
        _ => panic!("ZIP adoption must have a physical owner"),
    };
    for forward in [false, true] {
        app.route_tag_view_grid_book_navigation(&ctx, forward);
        assert!(app.folder_nav_pending.is_none());
        assert!(
            matches!(app.top_level_grid_view.history_navigation_transition(),
            Some(HistoryNavigationTransition::Physical(request)) if request.request_id == request_id)
        );
        assert_eq!(app.tag_view.nav_stack, vec![a.clone()]);
    }
    settle(&mut app, &ctx);
    assert_eq!(app.tag_view.nav_stack, vec![zip.clone()]);
    assert_eq!(app.effective_folder(), Some(zip));
    assert!(app.fullscreen_idx.is_none());
    assert!(!app.main_list_folder_navigation_adoption_pending());
}

#[test]
fn tag_list_order_grid_folder_hydration_gate_keeps_owner_until_completion() {
    let mut app = setup_app();
    let root = app.tmp.path().to_path_buf();
    let (_, b) = fixture(&mut app, &root, false);
    let ctx = egui::Context::default();
    app.route_tag_view_grid_book_navigation(&ctx, true);
    let result = receive(&mut app);
    app.apply_folder_nav_result(&ctx, result);
    let cancel = app
        .sidecar_restore
        .as_ref()
        .expect("folder hydration")
        .cancel_for_test();
    assert!(app.main_list_folder_navigation_adoption_pending());
    let generation = app.items_generation;
    for forward in [false, true] {
        app.route_tag_view_grid_book_navigation(&ctx, forward);
        assert!(app.folder_nav_pending.is_none());
        assert!(Arc::ptr_eq(
            &app.sidecar_restore.as_ref().unwrap().cancel_for_test(),
            &cancel
        ));
        assert!(!cancel.load(Ordering::Relaxed));
        assert_eq!(app.items_generation, generation);
        assert_eq!(app.tag_view.nav_stack, vec![b.clone()]);
    }
    settle(&mut app, &ctx);
    assert!(!app.main_list_folder_navigation_adoption_pending());
    app.route_tag_view_grid_book_navigation(&ctx, false);
    assert_request(&app, false, false, 2);
}

#[test]
fn tag_list_order_cancel_and_scan_failure_preserve_the_opened_root() {
    let mut app = setup_app();
    let root = app.tmp.path().to_path_buf();
    let (a, _) = fixture(&mut app, &root, false);
    let ctx = egui::Context::default();
    app.route_tag_view_grid_book_navigation(&ctx, true);
    app.cancel_pending_folder_nav();
    assert_eq!(app.tag_view.nav_stack, vec![a.clone()]);
    assert_eq!(app.effective_folder(), Some(a.clone()));
    app.route_tag_view_grid_book_navigation(&ctx, true);
    let mut result = receive(&mut app);
    result.target.as_mut().unwrap().scanned =
        FolderNavScanResult::Failed("test scan failure".into());
    app.apply_folder_nav_result(&ctx, result);
    assert_eq!(app.tag_view.nav_stack, vec![a.clone()]);
    assert_eq!(app.effective_folder(), Some(a));
    assert!(app.folder_nav_pending.is_none());
    assert!(app.fs_feedback_toast.is_some());
}

#[test]
fn tag_list_order_grid_request_supersedes_direct_navigation_scan() {
    let mut app = setup_app();
    let root = app.tmp.path().to_path_buf();
    let (a, _) = fixture(&mut app, &root, false);
    let direct = root.join("direct");
    std::fs::create_dir(&direct).unwrap();
    app.open_direct_navigation_target_at_stage(
        direct,
        None,
        OpenRequestOwner::Navigation,
        None,
        None,
        StartupListIntent::ExplicitList,
        MainListRequestStage::Admission,
        DirectoryScanRoute::Worker,
    );
    assert!(app.direct_navigation_scan_pending());
    assert!(!app.main_list_folder_navigation_adoption_pending());
    let ctx = egui::Context::default();
    app.route_tag_view_grid_book_navigation(&ctx, true);
    assert_request(&app, true, false, 0);
    assert!(!app.direct_navigation_scan_pending());
    assert_eq!(app.current_folder, Some(a));
}

#[test]
fn tag_list_order_folder_then_archive_returns_through_folder_before_results() {
    let mut app = setup_app();
    let fixture_root = app.tmp.path().to_path_buf();
    let (a, root) = fixture(&mut app, &fixture_root, true);
    // Audio makes this Folder row a traversal stop. With no loose image or video,
    // the production fullscreen reopen must then open the inner book instead.
    std::fs::remove_file(root.join("page.png")).unwrap();
    std::fs::write(root.join("audio.mp3"), b"audio").unwrap();
    let archive = root.join("inner.zip");
    let mut png = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgb8(image::RgbImage::new(2, 2))
        .write_to(&mut png, image::ImageFormat::Png)
        .unwrap();
    let mut zip = zip::ZipWriter::new(std::fs::File::create(&archive).unwrap());
    zip.start_file("001.png", zip::write::SimpleFileOptions::default())
        .unwrap();
    std::io::Write::write_all(&mut zip, png.get_ref()).unwrap();
    zip.finish().unwrap();
    app.tags_db
        .as_mut()
        .unwrap()
        .set_item_tags(
            &crate::tags_db::item_key_for_path(&root),
            ["chosen"],
            crate::tags_db::source::EDIT,
        )
        .unwrap();

    let ctx = egui::Context::default();
    let run = app.tag_view.run;
    let snapshot = app.list_order.clone().unwrap();
    assert_eq!(app.tag_view.nav_stack, vec![a.clone()]);
    app.handle_fullscreen_list_order_nav_context(&ctx, 0, true, false);
    assert_request(&app, true, true, 0);
    assert_eq!(app.tag_view.nav_stack, vec![a.clone()]);
    let result = receive(&mut app);
    assert_eq!(result.target.as_ref().unwrap().logical_source(), root);
    assert_eq!(app.tag_view.nav_stack, vec![a]);
    app.apply_folder_nav_result(&ctx, result);
    let reopen_deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
    loop {
        settle(&mut app, &ctx);
        if app.fullscreen_idx.is_some() && app.fs_nav_after_pdf_enumerate.is_none() {
            break;
        }
        assert!(
            std::time::Instant::now() < reopen_deadline,
            "inner ZIP fullscreen did not reopen"
        );
        std::thread::yield_now();
    }
    assert_eq!(app.tag_view.nav_stack, vec![root.clone(), archive.clone()]);
    assert_eq!(app.effective_folder(), Some(archive.clone()));
    assert!(
        app.fullscreen_idx.is_some(),
        "inner ZIP adoption reopens fullscreen"
    );
    assert!(app.fs_nav_after_pdf_enumerate.is_none());
    assert!(matches!(app.items.first(), Some(GridItem::ZipImage { .. })));
    assert_eq!(app.list_order_position().unwrap().1, 2);
    assert!(Arc::ptr_eq(app.list_order.as_ref().unwrap(), &snapshot));
    assert_eq!(app.tag_view.run, run);

    // Leave fullscreen for the page grid before exercising the tag view's two
    // book/folder Backspace actions from that grid.
    app.close_fullscreen();
    assert!(app.fullscreen_idx.is_none());
    app.tag_view_back();
    settle(&mut app, &ctx);
    assert_eq!(app.tag_view.nav_stack, vec![root.clone()]);
    assert_eq!(app.effective_folder(), Some(root.clone()));
    assert!(!app.items_are_tag_view);
    assert!(app.items.contains(&GridItem::ZipFile(archive)));
    assert_eq!(app.tag_view.run, run, "first grid BS does not requery");
    app.tag_view_back();
    assert!(app.tag_view.nav_stack.is_empty());
    assert_eq!(app.tag_view.run, run + 1);
    assert_eq!(
        app.select_after_load,
        Some(AfterLoadSelection::ExactPath(root.clone()))
    );
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
    while app.tag_view_pending.is_some() || app.sidecar_restore_active() {
        app.poll_tag_view();
        app.poll_sidecar_restore(&ctx);
        assert!(
            std::time::Instant::now() < deadline,
            "tag results did not return"
        );
        std::thread::yield_now();
    }
    assert!(app.items_are_tag_view);
    assert!(app.tag_view.nav_stack.is_empty());
    assert!(matches!(app.items.get(app.selected.unwrap()),
        Some(GridItem::Folder(path)) if crate::folder_tree::path_eq(path, &root)));
    assert!(app.list_order_position().is_none());
}

#[test]
fn tag_list_order_normal_keyboard_tree_and_sibling_use_result_order() {
    // These are the resolved KeyAction outcomes passed by handle_keyboard.
    // Calling the dispatch directly avoids handle_keyboard's native key probes.
    for action in 0..4 {
        let mut app = setup_app();
        let root = app.tmp.path().to_path_buf();
        let (_, b) = fixture(&mut app, &root, false);
        let forward = action == 0 || action == 2;
        if !forward {
            show_book(&mut app, &b, false);
        }
        let ctx = egui::Context::default();
        app.handle_grid_book_navigation_keys(
            &ctx,
            action == 0,
            action == 1,
            action == 2,
            action == 3,
        );
        assert_request(&app, forward, false, if forward { 0 } else { 2 });
    }
}

#[test]
fn tag_list_order_ring_gesture_tree_sibling_and_gamepad_all_use_result_order() {
    use crate::ring_shortcut::{
        MouseGestureBinding, MouseGestureDirection, RightDragContext, RingActionId,
        RingShortcutContext,
    };
    for route in 0..5 {
        for forward in [false, true] {
            let mut app = setup_app();
            let root = app.tmp.path().to_path_buf();
            let (_, b) = fixture(&mut app, &root, false);
            if !forward {
                show_book(&mut app, &b, false);
            }
            let action = match (route % 2, forward) {
                (0, true) => RingActionId::TreeFolderNext,
                (0, false) => RingActionId::TreeFolderPrev,
                (1, true) => RingActionId::SiblingFolderNext,
                _ => RingActionId::SiblingFolderPrev,
            };
            let ctx = egui::Context::default();
            match route {
                0 | 1 => {
                    app.apply_ring_action(
                        &ctx,
                        RingShortcutContext::Grid,
                        action,
                        "tag-list-order-test",
                    );
                }
                2 | 3 => {
                    app.settings
                        .ring_shortcuts
                        .mouse_gesture_profile_mut(RightDragContext::Grid)
                        .bindings = vec![MouseGestureBinding::new(
                        vec![MouseGestureDirection::Up],
                        action,
                    )];
                    app.trigger_mouse_gesture_action(
                        &ctx,
                        RightDragContext::Grid,
                        &[MouseGestureDirection::Up],
                    );
                }
                _ => app.handle_gamepad_folder_nav_for_test(&ctx, forward),
            }
            assert_request(&app, forward, false, if forward { 0 } else { 2 });
        }
    }
}

#[cfg(windows)]
#[test]
fn tag_list_order_blocked_item_keyboard_tree_and_sibling_use_result_order() {
    let _serial = crate::key_input::lock_test_input();
    struct ClearInput;
    impl Drop for ClearInput {
        fn drop(&mut self) {
            crate::key_input::clear_test_frame();
        }
    }
    let _clear = ClearInput;
    for (key, vk, forward) in [
        (egui::Key::ArrowDown, 0x28, true),
        (egui::Key::ArrowUp, 0x26, false),
        (egui::Key::PageDown, 0x22, true),
        (egui::Key::PageUp, 0x21, false),
    ] {
        let mut app = setup_app();
        let root = app.tmp.path().to_path_buf();
        let (_, b) = fixture(&mut app, &root, false);
        if !forward {
            show_book(&mut app, &b, false);
        }
        let ctx = egui::Context::default();
        crate::key_input::set_test_frame(vec![crate::key_input::KeyEdge {
            operation_id: crate::key_input::KeyOperationId::unassigned(),
            source_hwnd: 1,
            source_viewport: egui::ViewportId::ROOT,
            virtual_key: vk,
            scan_code: 0,
            extended: false,
            pressed: true,
            repeat: false,
            ctrl: true,
            shift: false,
            alt: false,
        }]);
        ctx.begin_pass(egui::RawInput {
            focused: true,
            events: vec![
                egui::Event::ModifiersChanged(egui::Modifiers::CTRL),
                egui::Event::Key {
                    key,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: egui::Modifiers::CTRL,
                },
            ],
            ..Default::default()
        });
        // Production reaches this handler after the App pass has resolved its owner.
        // Supply that same proof explicitly without polling physical keyboard state.
        let owner = crate::keyboard_input::decide_keyboard_owner(
            crate::keyboard_input::KeyboardOwnershipSnapshot {
                viewport: ctx.viewport_id(),
                viewport_focused: true,
                modal: false,
                text_input: None,
                focused_ui: None,
                shortcut_scope: Some(crate::keyboard_input::ShortcutScope::new(
                    ctx.viewport_id(),
                    crate::keyboard_input::ShortcutSurface::Main,
                )),
            },
        );
        crate::keyboard_input::cache_keyboard_owner(&ctx, owner);
        app.handle_stack_refresh_keyboard(
            &ctx,
            true,
            0,
            0,
            crate::filename_stack_ui::GridItemInputBlock::DirectNavigationScan,
        );
        ctx.end_pass().drop_without_applying_deltas();
        assert_request(&app, forward, false, if forward { 0 } else { 2 });
    }
}
