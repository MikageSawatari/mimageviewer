//! §1.849: all physical sort/settings reloads share the existing worker owner.
use super::*;
use crate::app::top_level_grid_view::{TopLevelGridSurface, TopLevelSearchView};
use crate::ui_dialogs::batch_convert::{BatchConvertMsg, BatchConvertPending};

fn adopt(app: &mut App, held: HeldScan) {
    held.release();
    let ready = mutation_refresh_wait_for_order_scan(app);
    mutation_refresh_resolve_order_scan(app, ready);
    app.finish_page_identity_preparation_for_test();
}

fn preferences_display_order(app: &mut App) {
    // Use the existing preferences draft helper, keeping visibility unchanged.
    app.set_visibility_preferences_for_test(app.settings.file_type_visibility.clone());
    app.pref_state.as_mut().unwrap().settings.grid_display_order = GridDisplayOrder::from_rows([
        vec![GridItemDisplayKind::VideoAudio],
        vec![GridItemDisplayKind::Image],
        vec![GridItemDisplayKind::Folder, GridItemDisplayKind::Archive],
        vec![],
    ]);
    app.apply_preferences_dialog(&egui::Context::default());
    app.show_preferences = false;
}

fn names(app: &App) -> Vec<String> {
    app.items
        .iter()
        .map(|item| item.name().into_owned())
        .collect()
}

#[test]
fn sort_worker_849_non_duration_wait_blocks_keys_and_clicks() {
    for order in [SortOrder::FileName, SortOrder::DateAsc, SortOrder::SizeDesc] {
        let mut h = Harness::images();
        h.press(egui::Key::ArrowRight, egui::Modifiers::NONE);
        assert_eq!(h.env.selected, Some(1), "positive key control");
        h.click(h.cell_center(0));
        assert_eq!(h.env.selected, Some(0), "positive click control");
        let rows = h.env.items.clone();
        let generation = h.env.items_generation;
        let held = request_sort(&mut h.env, order);
        assert!(h.env.view_order_refresh_scan_pending());
        assert!(!h.env.grid_item_input_allowed());
        h.press(egui::Key::ArrowRight, egui::Modifiers::NONE);
        h.click(h.cell_center(1));
        h.press(egui::Key::Enter, egui::Modifiers::NONE);
        h.assert_grid_owner();
        assert_eq!(h.env.selected, Some(0));
        assert_eq!(h.env.items, rows);
        assert_eq!(h.env.items_generation, generation);
        assert!(h.env.fullscreen_idx.is_none());
        held.release();
        h.settle();
        assert!(h.env.grid_item_input_allowed());
        assert!(h.env.items_generation > generation);
    }
}

#[test]
fn sort_worker_849_duration_to_name_preserves_selected_name() {
    let mut h = Harness::images();
    let duration = request_sort(&mut h.env, SortOrder::DurationAsc);
    duration.release();
    h.settle();
    h.env.move_grid_cursor(Some(0), GridCursorIntent::Automatic);
    assert_eq!(h.env.items[0].name(), "B.png");
    let held = request_sort(&mut h.env, SortOrder::FileNameDesc);
    held.release();
    h.settle();
    assert_eq!(h.names(), ["C.png", "B.png", "Z.wav"]);
    assert_eq!(h.env.selected, Some(1));
    assert_eq!(h.env.items[h.env.selected.unwrap()].name(), "B.png");
}

#[test]
fn sort_worker_849_name_to_date_preserves_selected_name() {
    let mut h = Harness::images();
    let folder = h.env.current_folder.clone().unwrap();
    for (name, seconds) in [("B.png", 10), ("C.png", 20)] {
        std::fs::File::options()
            .write(true)
            .open(folder.join(name))
            .unwrap()
            .set_modified(std::time::UNIX_EPOCH + Duration::from_secs(seconds))
            .unwrap();
    }
    assert_eq!(h.env.items[h.env.selected.unwrap()].name(), "C.png");
    let held = request_sort(&mut h.env, SortOrder::DateAsc);
    held.release();
    h.settle();
    assert_eq!(h.names(), ["B.png", "C.png", "Z.wav"]);
    assert_eq!(h.env.selected, Some(1));
    assert_eq!(h.env.items[h.env.selected.unwrap()].name(), "C.png");
}

#[test]
fn sort_worker_849_preferences_display_order_uses_worker() {
    let mut h = Harness::images();
    let rows = h.env.items.clone();
    preferences_display_order(&mut h.env);
    let held = HeldScan::capture(&mut h.env);
    assert!(h.env.view_order_refresh_scan_pending());
    assert!(!h.env.grid_item_input_allowed());
    assert_eq!(h.env.items, rows);
    held.release();
    h.settle();
    assert_eq!(h.names(), ["Z.wav", "C.png", "B.png"]);
    assert_eq!(h.env.selected, Some(1));
}

fn write_zip(path: &Path) {
    use std::io::Write;
    let mut writer = zip::ZipWriter::new(std::fs::File::create(path).unwrap());
    for name in ["page.png", "nested/page.png"] {
        writer
            .start_file(name, zip::write::SimpleFileOptions::default())
            .unwrap();
        let mut png = std::io::Cursor::new(Vec::new());
        image::RgbImage::new(8, 6)
            .write_to(&mut png, image::ImageFormat::Png)
            .unwrap();
        writer.write_all(png.get_ref()).unwrap();
    }
    writer.finish().unwrap();
}

#[test]
fn sort_worker_849_zip_enumeration_preferences_leave_owner_and_use_latest_order() {
    let mut app = setup_app();
    configure(&mut app);
    app.settings.auto_fullscreen_zip_pdf = false;
    let path = app.tmp.path().join("pending.zip");
    write_zip(&path);
    app.load_zip_as_folder(path, StartupListIntent::ExplicitList);
    assert!(app.zip_nav.is_none());
    // Hold the real enumeration reply until after the preferences commit.
    let pending = app.zip_enumerate_pending.as_mut().unwrap();
    let result = pending.rx.recv_timeout(DEADLINE).unwrap();
    let cancel = pending.cancel.clone();
    let (tx, rx) = mpsc::channel();
    pending.rx = rx;
    preferences_display_order(&mut app);
    assert!(app.folder_pane_open_pending.is_none());
    assert!(Arc::ptr_eq(
        &app.zip_enumerate_pending.as_ref().unwrap().cancel,
        &cancel
    ));
    assert!(!cancel.load(Ordering::Relaxed));
    tx.send(result).unwrap();
    app.poll_zip_enumerate();
    app.finish_page_identity_preparation_for_test();
    // Image row now precedes the nested directory, unlike the original placement.
    assert!(matches!(
        app.items.as_slice(),
        [GridItem::ZipImage { .. }, GridItem::ZipDir { .. }]
    ));
    assert!(app.zip_enumerate_pending.is_none());
    assert!(app.folder_pane_open_pending.is_none());
}

#[test]
fn sort_worker_849_drive_list_preferences_do_not_scan_or_replace_rows() {
    let mut app = setup_app();
    configure(&mut app);
    app.enter_drive_list_from_navigation(None);
    app.finish_page_identity_preparation_for_test();
    let rows = app.items.clone();
    let generation = app.items_generation;
    let folder = app.current_folder.clone();
    preferences_display_order(&mut app);
    assert!(app.folder_pane_open_pending.is_none());
    assert_eq!(app.items, rows);
    assert_eq!(app.items_generation, generation);
    assert_eq!(app.current_folder, folder);
    assert_eq!(
        app.top_level_grid_view.surface(),
        &TopLevelGridSurface::DriveList
    );
}

fn completed_conversion(app: &mut App, source: PathBuf, zip: PathBuf) -> HeldScan {
    write_zip(&zip);
    let (tx, rx) = mpsc::channel();
    app.batch_convert = Some(BatchConvertPending {
        rx,
        cancel: Arc::new(AtomicBool::new(false)),
        total: 1,
        current_name: String::new(),
        converted: Vec::new(),
        failed: Vec::new(),
        skipped_existing: Vec::new(),
        skipped_collision: Vec::new(),
    });
    tx.send(BatchConvertMsg::Done {
        src: source,
        result: Ok(zip),
    })
    .unwrap();
    tx.send(BatchConvertMsg::Finished { canceled: false })
        .unwrap();
    app.poll_batch_convert();
    assert!(app.batch_convert.is_none());
    assert!(app.checked.is_empty());
    HeldScan::capture(app)
}

fn conversion_fixture() -> AppTestEnv {
    let mut app = setup_app();
    configure(&mut app);
    app.settings.skip_archive_if_zip_exists = true;
    app.settings.archive_file_handling = crate::settings::ArchiveFileHandling::Convert;
    let folder = app.tmp.path().join("conversion");
    std::fs::create_dir(&folder).unwrap();
    std::fs::write(folder.join("book.7z"), b"source").unwrap();
    app.load_folder(folder);
    app.finish_page_identity_preparation_for_test();
    assert_eq!(names(&app), ["book.7z"]);
    app.checked.insert(0);
    app
}

#[test]
fn sort_worker_849_batch_conversion_adopts_zip_and_removes_same_name_source() {
    let mut app = conversion_fixture();
    let folder = app.current_folder.clone().unwrap();
    let held = completed_conversion(&mut app, folder.join("book.7z"), folder.join("book.zip"));
    assert!(app.view_order_refresh_scan_pending());
    assert!(!app.grid_item_input_allowed());
    assert_eq!(names(&app), ["book.7z"]);
    adopt(&mut app, held);
    assert_eq!(names(&app), ["book.zip"]);
    assert!(matches!(app.items.as_slice(), [GridItem::ZipFile(_)]));
}

#[test]
fn sort_worker_849_tag_physical_child_conversion_refreshes_despite_fixed_sort() {
    let mut app = conversion_fixture();
    app.tag_view.active = true;
    app.items_are_tag_view = false;
    app.top_level_grid_view
        .replace_surface(TopLevelGridSurface::Search(TopLevelSearchView::Tag));
    assert_eq!(
        app.grid_sort_lock_reason(),
        Some(GridSortLockReason::FixedOrder)
    );
    let folder = app.current_folder.clone().unwrap();
    let held = completed_conversion(&mut app, folder.join("book.7z"), folder.join("book.zip"));
    assert!(app.view_order_refresh_scan_pending());
    adopt(&mut app, held);
    assert_eq!(names(&app), ["book.zip"]);
    assert!(matches!(app.items.as_slice(), [GridItem::ZipFile(_)]));
    assert_eq!(
        app.top_level_grid_view.surface(),
        &TopLevelGridSurface::Search(TopLevelSearchView::Tag)
    );
}

#[test]
fn sort_worker_849_container_named_directories_still_scan() {
    for name in ["book.zip", "book.rar", "book.epub", "book.pdf"] {
        let mut app = setup_app();
        configure(&mut app);
        let folder = app.tmp.path().join(name);
        std::fs::create_dir(&folder).unwrap();
        // Exercise an empty directory too: it must not be mistaken for empty pages.
        app.load_folder_with_scan(folder.clone(), Some(scan_directory(&folder)));
        app.finish_page_identity_preparation_for_test();
        let held = request_sort(&mut app, SortOrder::FileName);
        adopt(&mut app, held);
        assert_eq!(app.current_folder.as_ref(), Some(&folder));
        assert!(app.items.is_empty());
    }
}

#[test]
#[cfg(windows)]
fn sort_worker_849_collection_physical_source_sorts_and_preserves_selected_name() {
    let mut h = Harness::images();
    let folder = h.env.current_folder.clone().unwrap();
    let restore = install_collection_item_for_detached_plan(
        &mut h.env,
        &folder,
        crate::collection_store::CollectionResolvedKind::Folder,
    );
    let anchor = restore.viewport_anchor.clone().unwrap();
    let owner = h
        .env
        .collection_grid_physical_load_owner(0, &folder)
        .unwrap();
    assert!(h.env.load_folder_with_scan_owned(
        folder.clone(),
        Some(scan_directory(&folder)),
        OpenRequestOwner::CollectionGridPhysical(owner),
        StartupListIntent::ExplicitList,
    ));
    h.env.finish_page_identity_preparation_for_test();
    assert!(matches!(
        h.env
            .top_level_grid_view
            .collection_session()
            .unwrap()
            .position,
        top_level_grid_view::CollectionGridPosition::PhysicalSource { .. }
    ));
    h.env.move_grid_cursor(Some(0), GridCursorIntent::Automatic);
    assert_eq!(h.env.items[0].name(), "C.png");
    let held = request_sort(&mut h.env, SortOrder::FileName);
    assert!(h.env.view_order_refresh_scan_pending());
    adopt(&mut h.env, held);
    assert_eq!(names(&h.env), ["B.png", "C.png", "Z.wav"]);
    assert_eq!(h.env.selected, Some(1));
    assert_eq!(h.env.items[h.env.selected.unwrap()].name(), "C.png");
    assert!(
        matches!(&h.env.top_level_grid_view.collection_session().unwrap().position,
        top_level_grid_view::CollectionGridPosition::PhysicalSource { entry_id, source_key, path }
            if *entry_id == anchor.entry_id && source_key == &anchor.source_key && path == &folder)
    );
}

#[test]
fn sort_worker_849_everything_physical_child_sorts_and_preserves_selected_name() {
    let mut h = Harness::images();
    let folder = h.env.current_folder.clone().unwrap();
    crate::global_search_ui::adopt_everything_for_test(
        &mut h.env,
        &h.ctx,
        "sort child",
        crate::global_search_ui::everything_dataset_for_test(Vec::new()),
    );
    let crate::global_search_ui::SearchAccepted::Everything { position, .. } =
        &mut h.env.global_search.accepted
    else {
        panic!("Everything owner")
    };
    *position = crate::global_search_ui::EverythingPosition::PhysicalChild {
        logical_source: folder.clone(),
        return_anchor: folder.clone(),
        route: Default::default(),
    };
    h.env
        .load_folder_with_scan(folder.clone(), Some(scan_directory(&folder)));
    h.env.finish_page_identity_preparation_for_test();
    assert!(!h.env.items_are_global_search_view);
    assert!(h.env.global_search.active);
    h.env.move_grid_cursor(Some(0), GridCursorIntent::Automatic);
    assert_eq!(h.env.items[0].name(), "C.png");
    let crate::global_search_ui::SearchAccepted::Everything { position, .. } =
        &h.env.global_search.accepted
    else {
        panic!("Everything child")
    };
    let position = position.clone();
    let held = request_sort(&mut h.env, SortOrder::FileName);
    assert!(h.env.view_order_refresh_scan_pending());
    adopt(&mut h.env, held);
    assert_eq!(names(&h.env), ["B.png", "C.png", "Z.wav"]);
    assert_eq!(h.env.selected, Some(1));
    assert_eq!(h.env.items[h.env.selected.unwrap()].name(), "C.png");
    let crate::global_search_ui::SearchAccepted::Everything {
        position: after, ..
    } = &h.env.global_search.accepted
    else {
        panic!("Everything retained")
    };
    assert_eq!(after, &position);
    assert_eq!(
        h.env.top_level_grid_view.surface(),
        &TopLevelGridSurface::Search(TopLevelSearchView::Global)
    );
}
