// Session-position regressions use the real navigation/install owners and disposable data.
use super::phase_c_folder_nav_history_tests::{rating_1328_install_rows, write_1328_zip};
use super::phase_c_support::setup_app;
use super::*;
fn session1328_settle(app: &mut App) {
    let ctx = egui::Context::default();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    loop {
        app.poll_collection_history_transition(&ctx);
        app.poll_zip_enumerate();
        app.poll_pdf_enumerate();
        app.poll_subfolder_expansion(&ctx);
        app.poll_reading_history_thumbnail_sources(&ctx);
        app.poll_rating_view();
        app.poll_sidecar_restore(&ctx);
        if app
            .top_level_grid_view
            .history_navigation_transition()
            .is_none()
            && app.subfolder_expansion_pending.is_none()
            && app.subfolder_expansion_install_pending.is_none()
            && app.reading_history_sources_pending.is_empty()
            && app.rating_view_pending.is_none()
            && app.zip_enumerate_pending.is_none()
            && app.pdf_enumerate_pending.is_none()
            && !app.sidecar_restore_active()
        {
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "session fixture did not settle"
        );
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
}

fn session1328_reading(app: &mut App, root: &std::path::Path, count: usize) -> Vec<PathBuf> {
    use crate::reading_history_db::{ReadingHistoryEntry, ReadingHistoryKind};
    app.settings.auto_fullscreen_image_folders = false;
    app.settings.auto_fullscreen_zip_pdf = false;
    let paths: Vec<_> = (0..count)
        .map(|index| root.join(format!("{index:04}-book")))
        .collect();
    for (index, path) in paths.iter().enumerate() {
        std::fs::create_dir_all(path).unwrap();
        image::RgbImage::new(3, 4)
            .save(path.join("page.png"))
            .unwrap();
        let mut entry = ReadingHistoryEntry::new(
            path.clone(),
            ReadingHistoryKind::Folder,
            None,
            format!("book-{index}"),
            None,
            None,
        );
        entry.last_read_at_ms = 1_000_000 - index as i64;
        app.reading_history_db
            .as_ref()
            .unwrap()
            .upsert(entry, 1000)
            .unwrap();
    }
    app.enter_reading_history_from_menu();
    session1328_settle(app);
    assert_eq!(app.items.len(), count);
    paths
}

fn session1328_expansion(app: &mut App, root: &std::path::Path, count: usize) -> Vec<PathBuf> {
    use crate::app::subfolder_expansion::{
        SubfolderExpansionEntry, SubfolderExpansionEntryKind, SubfolderExpansionSnapshot,
    };
    app.settings.auto_fullscreen_image_folders = false;
    app.settings.auto_fullscreen_zip_pdf = false;
    app.settings.grid_display_order = crate::settings::GridDisplayOrder::from_rows([
        crate::settings::GridItemDisplayKind::ALL.to_vec(),
        vec![],
        vec![],
        vec![],
    ]);
    std::fs::create_dir_all(root).unwrap();
    let mut entries = Vec::new();
    let mut paths = Vec::new();
    for index in 0..count {
        let (path, kind) = if count == 600 && index != 310 {
            if index == 309 {
                let path = root.join(format!("{index:04}-book.zip"));
                write_1328_zip(&path);
                (path, SubfolderExpansionEntryKind::Zip)
            } else if index == 311 {
                let path = root.join(format!("{index:04}-book.pdf"));
                std::fs::write(&path, b"%PDF-1.4\n").unwrap();
                (path, SubfolderExpansionEntryKind::Pdf)
            } else {
                let path = root.join(format!("{index:04}-image.png"));
                image::RgbImage::new(3, 4).save(&path).unwrap();
                (path, SubfolderExpansionEntryKind::Image)
            }
        } else {
            let path = root.join(format!("{index:04}-book"));
            std::fs::create_dir_all(&path).unwrap();
            image::RgbImage::new(3, 4)
                .save(path.join("page.png"))
                .unwrap();
            (path, SubfolderExpansionEntryKind::Folder)
        };
        entries.push(SubfolderExpansionEntry {
            path: path.clone(),
            kind,
            mtime: index as i64,
            file_size: 0,
        });
        paths.push(path);
    }
    app.current_folder = Some(super::subfolder_expansion_synthetic_path());
    app.items_are_subfolder_expansion_view = true;
    app.top_level_grid_view
        .replace_surface(crate::app::top_level_grid_view::TopLevelGridSurface::SubfolderExpansion);
    app.subfolder_expansion_root = Some(root.to_path_buf());
    app.subfolder_expansion_roots = vec![root.to_path_buf()];
    app.subfolder_expansion_saved_folder = Some(root.to_path_buf());
    app.subfolder_expansion_snapshot = Some(SubfolderExpansionSnapshot {
        file_type_visibility: Default::default(),
        root: root.to_path_buf(),
        roots: vec![root.to_path_buf()],
        scan_filter: Default::default(),
        entries: entries.into(),
        video_thumb_overrides: Default::default(),
        diag: Default::default(),
        stack_rating_source: Default::default(),
        stack_order_request: crate::rating_sort::ListingOrderRequest::Standard(
            crate::settings::SortOrder::FileName,
        ),
    });
    assert!(app.reinstall_subfolder_expansion_snapshot());
    session1328_settle(app);
    assert_eq!(app.items.len(), count);
    paths
}

fn session1328_draw(app: &mut App, ctx: &egui::Context) {
    ctx.run_pass(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1100.0, 350.0),
            )),
            ..Default::default()
        },
        |ctx| {
            assert!(ctx.root_ui(|ui| app.render_grid(ui)).is_none());
        },
    )
    .drop_without_applying_deltas();
}

fn session1328_assert_visible(app: &App, path: &std::path::Path) {
    let index = app.selected.expect("selected returning row");
    assert!(crate::folder_tree::path_eq(
        app.items[index].container_path().unwrap(),
        path
    ));
    let row = app
        .visible_indices
        .iter()
        .position(|&i| i == index)
        .unwrap()
        / app.settings.grid_cols;
    let center = (row as f32 + 0.5) * app.last_cell_h;
    assert!(
        center >= app.scroll_offset_y && center <= app.scroll_offset_y + app.last_viewport_h,
        "row {row} center={center} scroll={} viewport={}",
        app.scroll_offset_y,
        app.last_viewport_h
    );
}

fn session1328_open(app: &mut App, index: usize, path: PathBuf) {
    app.selected = Some(index);
    let nav = app.grid_physical_navigation(index, path.clone(), false);
    assert!(app.apply_fullscreen_close_nav_immediate(nav));
    session1328_settle(app);
    assert!(crate::folder_tree::path_eq(
        &app.effective_folder().unwrap(),
        &path
    ));
}

#[test]
fn section1328_session_reading_folder_back_bs_and_forward_back() {
    for slot in [None, Some(QuickFolderSlotId::A), Some(QuickFolderSlotId::B)] {
        for bs in [false, true] {
            let mut app = setup_app();
            app.active_quick_folder_slot = slot;
            let root = app.tmp.path().join("reading");
            let paths = session1328_reading(&mut app, &root, 6);
            session1328_open(&mut app, 4, paths[4].clone());
            for round in 0..2 {
                if bs {
                    let nav = app.resolve_grid_parent_nav().expect("parent return");
                    assert!(app.apply_fullscreen_close_nav_immediate(nav));
                } else {
                    app.dispatch_main_folder_history_input(FolderHistoryDirection::Back);
                }
                session1328_settle(&mut app);
                assert_eq!(app.selected, Some(4));
                assert!(app.scroll_to_selected);
                if bs || round == 1 {
                    break;
                }
                app.dispatch_main_folder_history_input(FolderHistoryDirection::Forward);
                session1328_settle(&mut app);
            }
        }
    }
}

fn session1328_main_navigation() -> super::update_phases::MainFrameNavigation {
    super::update_phases::MainFrameNavigation {
        fav_nav: None,
        toolbar_fav_nav: None,
        fullscreen_close_nav: None,
        keyboard_nav: None,
        keyboard_nav_from_fullscreen_close: false,
        gamepad_nav: None,
        address_nav: None,
        open_folder_nav: None,
        context_nav: None,
        folder_pane_nav: None,
        grid_nav: None,
    }
}

fn session1328_key_nav(
    app: &mut App,
    ctx: &egui::Context,
    key: egui::Key,
) -> Option<crate::ui_main::AddressBarNav> {
    ctx.begin_pass(egui::RawInput {
        events: vec![egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        }],
        ..Default::default()
    });
    let nav = app.handle_keyboard(ctx);
    ctx.end_pass().drop_without_applying_deltas();
    nav
}

#[derive(Clone, Copy, Debug)]
enum Session1328ReadingReturn {
    Backspace,
    HistoryBack,
    CloseReturn,
    ParentButton,
    GamepadBack,
}

fn session1328_reading_zip_main_return(input: Session1328ReadingReturn) {
    use crate::reading_history_db::{ReadingHistoryEntry, ReadingHistoryKind};
    for slot in [None, Some(QuickFolderSlotId::A), Some(QuickFolderSlotId::B)] {
        let mut app = setup_app();
        app.active_quick_folder_slot = slot;
        let root = app.tmp.path().join("source");
        let paths = session1328_reading(&mut app, &root, 6);
        let zip = root.join("0004.zip");
        write_1328_zip(&zip);
        let db = app.reading_history_db.as_ref().unwrap();
        db.remove_key(&crate::path_key::normalize_keep_drive(&paths[4]))
            .unwrap();
        let mut entry = ReadingHistoryEntry::new(
            zip.clone(),
            ReadingHistoryKind::Zip,
            None,
            "zip".into(),
            None,
            None,
        );
        entry.last_read_at_ms = 999_996;
        db.upsert(entry, 1000).unwrap();
        app.enter_reading_history_from_menu();
        session1328_settle(&mut app);
        assert_eq!(app.selected, Some(0));
        assert_eq!(app.items[4].container_path(), Some(zip.as_path()));
        app.selected = Some(4);
        app.scroll_offset_y = 420.0;
        let ctx = egui::Context::default();
        let mut navigation = session1328_main_navigation();
        navigation.grid_nav = Some(app.grid_physical_navigation(4, zip.clone(), false));
        app.post_render_main_navigation(&ctx, navigation);
        session1328_settle(&mut app);
        assert!(!app.items_are_reading_history_view);
        assert!(app.zip_nav.as_ref().is_some_and(|nav| nav.at_root()));
        assert_eq!(
            app.reading_history_return_from
                .as_ref()
                .map(|reservation| &reservation.container),
            Some(&zip)
        );
        let saved_key = crate::path_key::normalize_keep_drive(&zip);
        assert_eq!(
            app.top_level_grid_view
                .reading_history_position()
                .and_then(|position| position.selected_key.as_deref()),
            Some(saved_key.as_str()),
        );
        let mut navigation = session1328_main_navigation();
        match input {
            Session1328ReadingReturn::Backspace => {
                // Include the real key handler AND the winning main-frame dispatcher.
                // apply_fullscreen_close_nav_immediate bypasses the failing dispatcher.
                navigation.keyboard_nav = session1328_key_nav(&mut app, &ctx, egui::Key::Backspace);
                assert!(navigation.keyboard_nav.is_some());
            }
            Session1328ReadingReturn::HistoryBack => {
                navigation.address_nav = Some(crate::ui_main::AddressBarNav::HistoryBack);
            }
            Session1328ReadingReturn::CloseReturn => {
                // Esc auto-open direct return resolves through close_return.rs before dispatch.
                app.pending_return_to_parent.request_live();
                navigation.fullscreen_close_nav = app.take_close_return_navigation();
                assert!(navigation.fullscreen_close_nav.is_some());
            }
            Session1328ReadingReturn::ParentButton => {
                navigation.address_nav = app.resolve_grid_parent_nav();
                assert!(navigation.address_nav.is_some());
            }
            Session1328ReadingReturn::GamepadBack => {
                navigation.gamepad_nav = app.handle_gamepad_grid_back();
                assert!(navigation.gamepad_nav.is_some());
            }
        }
        app.post_render_main_navigation(&ctx, navigation);
        session1328_settle(&mut app);
        assert!(app.items_are_reading_history_view);
        assert_eq!(app.selected, Some(4), "input={input:?} slot={slot:?}");
        assert_eq!(
            app.items[app.selected.unwrap()].container_path(),
            Some(zip.as_path())
        );
        assert!(app.scroll_to_selected);
        assert_eq!(app.scroll_offset_y, 420.0);

        // Explicit location commands must still discard the saved row in every input lane.
        for lane in 0..4 {
            app.selected = Some(4);
            let nav = app
                .apply_location_navigation_key_action(
                    &ctx,
                    KeyAction::GridOpenLocationReadingHistory,
                    "section1328-menu",
                )
                .expect("explicit reading-history command");
            assert!(matches!(
                nav,
                crate::ui_main::AddressBarNav::ReadingHistory(ReadingHistoryEntrance::Menu)
            ));
            let mut navigation = session1328_main_navigation();
            match lane {
                0 => navigation.keyboard_nav = Some(nav),
                1 => navigation.address_nav = Some(nav),
                2 => navigation.gamepad_nav = Some(nav),
                _ => navigation.grid_nav = Some(nav),
            }
            app.post_render_main_navigation(&ctx, navigation);
            session1328_settle(&mut app);
            assert_eq!(app.selected, Some(0), "menu lane={lane}");
        }
    }
}

#[test]
fn section1328_session_reading_zip_main_back_bs_restores_saved_row() {
    session1328_reading_zip_main_return(Session1328ReadingReturn::Backspace);
}

#[test]
fn section1328_session_reading_zip_main_toolbar_back_restores_saved_row() {
    session1328_reading_zip_main_return(Session1328ReadingReturn::HistoryBack);
}

#[test]
fn section1328_session_reading_zip_main_close_and_parent_returns_restore_saved_row() {
    for input in [
        Session1328ReadingReturn::CloseReturn,
        Session1328ReadingReturn::ParentButton,
        Session1328ReadingReturn::GamepadBack,
    ] {
        session1328_reading_zip_main_return(input);
    }
}

#[test]
fn section1328_session_zip_bs_preserves_physical_and_expansion_parent_meaning() {
    for expanded in [false, true] {
        let mut app = setup_app();
        app.active_quick_folder_slot = None;
        app.settings.auto_fullscreen_zip_pdf = false;
        let root = app.tmp.path().join("source");
        let zip = if expanded {
            session1328_expansion(&mut app, &root, 600)[309].clone()
        } else {
            std::fs::create_dir_all(root.join("a-first")).unwrap();
            let zip = root.join("book.zip");
            write_1328_zip(&zip);
            app.load_folder(root.clone());
            session1328_settle(&mut app);
            zip
        };
        let index = app
            .items
            .iter()
            .position(|item| item.container_path() == Some(zip.as_path()))
            .unwrap();
        session1328_open(&mut app, index, zip.clone());
        let ctx = egui::Context::default();
        let mut navigation = session1328_main_navigation();
        navigation.keyboard_nav = session1328_key_nav(&mut app, &ctx, egui::Key::Backspace);
        assert!(
            matches!(navigation.keyboard_nav, Some(crate::ui_main::AddressBarNav::Direct(ref parent, _)) if crate::folder_tree::path_eq(parent, &root))
        );
        app.post_render_main_navigation(&ctx, navigation);
        session1328_settle(&mut app);
        assert!(!app.items_are_reading_history_view);
        assert!(!app.items_are_subfolder_expansion_view);
        assert_eq!(app.current_folder.as_ref(), Some(&root));
        assert_eq!(
            app.items[app.selected.unwrap()].container_path(),
            Some(zip.as_path())
        );
        assert!(app.scroll_to_selected);
    }
}

#[test]
fn section1328_session_reading_deep_zip_parent_does_not_return_to_history() {
    let mut app = setup_app();
    app.active_quick_folder_slot = None;
    app.settings.auto_fullscreen_zip_pdf = false;
    let zip = app.tmp.path().join("nested.zip");
    let file = std::fs::File::create(&zip).unwrap();
    let mut writer = zip::ZipWriter::new(file);
    for name in ["root.jpg", "A/cover.jpg", "A/sub/page.jpg"] {
        writer
            .start_file(name, zip::write::SimpleFileOptions::default())
            .unwrap();
        std::io::Write::write_all(&mut writer, b"page").unwrap();
    }
    writer.finish().unwrap();
    app.load_folder(zip.clone());
    session1328_settle(&mut app);
    app.reading_history_return_from = Some(reading_history_return_for_test(zip.clone()));
    app.zip_nav_enter("A/");
    session1328_settle(&mut app);
    app.zip_nav_enter("A/sub/");
    session1328_settle(&mut app);
    assert!(app.grid_item_input_allowed());
    assert!(app.reading_history_back_nav().is_none());
    let (close_nav, _) = app
        .resolve_close_return_navigation()
        .expect("existing physical close target");
    assert!(matches!(
        close_nav,
        crate::ui_main::AddressBarNav::Direct(_, _)
    ));
    let ctx = egui::Context::default();
    assert!(session1328_key_nav(&mut app, &ctx, egui::Key::Backspace).is_none());
    assert_eq!(app.zip_nav.as_ref().unwrap().current().join("/"), "A");
    assert!(app.reading_history_back_nav().is_none());
    session1328_settle(&mut app);
    assert!(session1328_key_nav(&mut app, &ctx, egui::Key::Backspace).is_none());
    assert!(app.zip_nav.as_ref().unwrap().at_root());
    assert!(matches!(
        app.reading_history_back_nav(),
        Some(crate::ui_main::AddressBarNav::ReadingHistory(
            ReadingHistoryEntrance::Return(_)
        ))
    ));
    assert_eq!(
        app.reading_history_return_from
            .as_ref()
            .map(|reservation| &reservation.container),
        Some(&zip)
    );
}

#[test]
fn section1328_session_search_pending_saves_latest_reading_and_subfolder_rows() {
    for reading in [true, false] {
        for search in [0, 1, 2] {
            let mut app = setup_app();
            app.active_quick_folder_slot = None;
            let root = app.tmp.path().join("source");
            if reading {
                session1328_reading(&mut app, &root, 6);
            } else {
                session1328_expansion(&mut app, &root, 6);
            }
            app.selected = Some(3);
            match search {
                0 => app.open_global_search(),
                1 => app.open_favsearch(),
                _ => app.open_tag_view(),
            }
            app.selected = Some(4);
            app.scroll_offset_y = 600.0;
            let result_path = root.join("hit.png");
            match search {
                0 => app.replace_search_view_items(vec![GridItem::Image(result_path)], vec![None]),
                1 => app.apply_favsearch_results(vec![crate::search_index_db::IndexEntry {
                    path: result_path,
                    display_name: "hit.png".into(),
                    kind: crate::search_index_db::IndexKind::Folder,
                    mtime: 0,
                }]),
                _ => app.apply_tag_view_result(crate::tag_view::TagViewResult {
                    query: "#fixture".into(),
                    kind_filter: crate::tag_view::TagViewKindFilter::All,
                    summaries: vec![],
                    entries: vec![crate::tag_view::TagViewEntry {
                        applied_at_ms: None,
                        path: result_path,
                        kind: crate::tag_view::TagViewItemKind::Image,
                        mtime: 0,
                        file_size: 1,
                    }],
                    truncated: false,
                    video_thumb_overrides: Default::default(),
                }),
            }
            app.finish_page_identity_preparation_for_test();
            match search {
                0 => app.close_global_search(),
                1 => app.close_favsearch(),
                _ => app.close_tag_view(),
            }
            session1328_settle(&mut app);
            assert_eq!(app.selected, Some(4), "reading={reading} search={search}");
            assert_eq!(app.scroll_offset_y, 600.0);
        }
    }
}

#[test]
fn section1328_session_snapshot_return_preserves_source_row() {
    for reading in [true, false] {
        for via_search in [false, true] {
            let mut app = setup_app();
            app.active_quick_folder_slot = None;
            let root = app.tmp.path().join("source");
            if reading {
                session1328_reading(&mut app, &root, 6);
            } else {
                session1328_expansion(&mut app, &root, 6);
            }
            app.selected = Some(4);
            app.scroll_offset_y = 600.0;
            app.activate_snapshot(crate::snapshot::SnapshotSourceLabel::Mixed);
            assert!(app.snapshot.is_some());
            app.finish_page_identity_preparation_for_test();
            let saved = if reading {
                app.top_level_grid_view.reading_history_position()
            } else {
                app.top_level_grid_view.subfolder_expansion_position(
                    &crate::app::top_level_grid_view::SubfolderExpansionPositionKey::new(
                        &root,
                        std::slice::from_ref(&root),
                    ),
                )
            };
            assert!(saved.is_some(), "snapshot must save the source position");
            assert_eq!(saved.unwrap().scroll_offset_y, 600.0);
            assert!(matches!(
                app.projection_recipe,
                crate::app::book_materialization::ProjectionRecipe::ExtractedResult(
                    crate::app::book_materialization::ExtractedSurface::Snapshot
                )
            ));
            app.selected = Some(1);
            app.scroll_offset_y = 0.0;
            if via_search {
                app.open_global_search();
                app.replace_search_view_items(
                    vec![GridItem::Image(root.join("hit.png"))],
                    vec![None],
                );
                app.finish_page_identity_preparation_for_test();
                app.close_global_search();
            } else {
                app.deactivate_snapshot();
            }
            session1328_settle(&mut app);
            assert_eq!(
                app.selected,
                Some(4),
                "reading={reading} via_search={via_search}"
            );
            assert_eq!(app.scroll_offset_y, 600.0);
        }
    }
}

#[test]
fn section1328_session_reading_menu_always_first_and_missing_key_falls_back() {
    let mut app = setup_app();
    app.active_quick_folder_slot = None;
    let root = app.tmp.path().join("source");
    let paths = session1328_reading(&mut app, &root, 6);
    app.selected = Some(4);
    app.enter_reading_history_from_menu();
    session1328_settle(&mut app);
    assert_eq!(app.selected, Some(0));
    app.selected = Some(4);
    app.open_global_search();
    app.enter_reading_history_from_menu();
    session1328_settle(&mut app);
    assert_eq!(app.selected, Some(0));
    session1328_open(&mut app, 4, paths[4].clone());
    app.reading_history_db
        .as_ref()
        .unwrap()
        .remove_key(&crate::path_key::normalize_keep_drive(&paths[4]))
        .unwrap();
    app.dispatch_main_folder_history_input(FolderHistoryDirection::Back);
    session1328_settle(&mut app);
    assert_eq!(app.selected, Some(0));
}
#[test]
fn section1328_session_reading_zip_pending_back_restores_saved_row() {
    use crate::reading_history_db::{ReadingHistoryEntry, ReadingHistoryKind};
    let mut app = setup_app();
    app.active_quick_folder_slot = None;
    let root = app.tmp.path().join("source");
    let paths = session1328_reading(&mut app, &root, 6);
    let zip = root.join("0004.zip");
    write_1328_zip(&zip);
    let db = app.reading_history_db.as_ref().unwrap();
    db.remove_key(&crate::path_key::normalize_keep_drive(&paths[4]))
        .unwrap();
    let mut entry = ReadingHistoryEntry::new(
        zip.clone(),
        ReadingHistoryKind::Zip,
        None,
        "zip".into(),
        None,
        None,
    );
    entry.last_read_at_ms = 999_996;
    db.upsert(entry, 1000).unwrap();
    app.enter_reading_history_from_menu();
    session1328_settle(&mut app);
    app.selected = Some(4);
    app.scroll_offset_y = 420.0;
    let navigation = app.capture_main_list_navigation(
        super::MainHistoryOperation::Direct(super::DirectNavigationPurpose::Navigation),
        super::MainListSourceProof::Row,
    );
    let destination = app.main_list_requested_destination(
        &navigation,
        FolderNavHistoryTarget::Path(zip.clone()),
        None,
    );
    let adoption = app
        .prepare_main_list_adoption(navigation, destination)
        .unwrap();
    app.adopt_main_list_navigation(adoption, |app| {
        app.note_reading_history_open(4);
        app.load_zip_as_folder(zip.clone(), super::StartupListIntent::PageContinuation);
        None
    });
    assert!(app.zip_enumerate_pending.is_some());
    assert!(app.items.is_empty());
    app.dispatch_main_folder_history_input(FolderHistoryDirection::Back);
    session1328_settle(&mut app);
    assert_eq!(app.selected, Some(4));
    assert_eq!(app.scroll_offset_y, 420.0);
    assert!(app.zip_enumerate_pending.is_none());
}

#[test]
fn section1328_session_subfolder_large_mixed_zip_pdf_folder_returns() {
    for (index, pdf) in [(309, false), (311, true), (310, false)] {
        let mut app = setup_app();
        app.active_quick_folder_slot = None;
        let root = app.tmp.path().join("source");
        let paths = session1328_expansion(&mut app, &root, 600);
        app.selected = Some(index);
        let nav = app.grid_physical_navigation(index, paths[index].clone(), false);
        assert!(app.apply_fullscreen_close_nav_immediate(nav));
        if pdf {
            // Direct GridVirtual PDF opens use the enumeration owner, not staged history.
            // lib tests have no PDF worker entry point; inject pages at that existing boundary.
            app.pdf_enumerate_pending
                .as_mut()
                .expect("direct PDF enumeration")
                .2 = crate::pdf_loader::completed_enumerate_result_handle(
                &paths[index],
                Ok(crate::pdf_loader::PdfEnumerateResult {
                    pages: (0..4)
                        .map(|page_num| crate::pdf_loader::PdfPageEntry {
                            page_num,
                            mtime: 1,
                            file_size: 1,
                        })
                        .collect(),
                    direction: None,
                    stamp: None,
                }),
            );
        }
        session1328_settle(&mut app);
        app.selected = Some(3);
        app.dispatch_main_folder_history_input(FolderHistoryDirection::Back);
        session1328_settle(&mut app);
        assert_eq!(app.selected, Some(index));
        let ctx = egui::Context::default();
        crate::ui_fonts::configure_fonts(&ctx);
        session1328_draw(&mut app, &ctx);
        session1328_assert_visible(&app, &paths[index]);
    }
}

#[test]
fn section1328_session_subfolder_return_rescans_without_retained_snapshot() {
    let mut app = setup_app();
    app.active_quick_folder_slot = None;
    let root = app.tmp.path().join("source");
    let paths = session1328_expansion(&mut app, &root, 6);
    session1328_open(&mut app, 4, paths[4].clone());
    app.folder_nav_subfolder_restore.as_mut().unwrap().snapshot = None;
    app.settings.auto_fullscreen_image_folders = true;
    app.settings.auto_fullscreen_zip_pdf = true;
    app.dispatch_main_folder_history_input(FolderHistoryDirection::Back);
    session1328_settle(&mut app);
    assert_eq!(app.selected, Some(4));
    assert_eq!(app.items.len(), 6);
}

fn session1328_tall_return(reading: bool, auto_off_child: bool) {
    for slot in [None, Some(QuickFolderSlotId::A), Some(QuickFolderSlotId::B)] {
        let mut app = setup_app();
        app.active_quick_folder_slot = slot;
        app.settings.grid_cols = 10;
        app.settings.thumb_aspect_auto = true;
        app.settings.reading_history_limit = 1000;
        let root = app.tmp.path().join("source");
        let paths = if reading {
            session1328_reading(&mut app, &root, 600)
        } else {
            session1328_expansion(&mut app, &root, 600)
        };
        app.cancel_token
            .store(true, std::sync::atomic::Ordering::Relaxed);
        for index in 0..24 {
            app.auto_aspect.samples.insert(index, 4.0 / 3.0);
        }
        app.maybe_apply_auto_aspect(true);
        assert_eq!(
            app.effective_thumb_aspect(),
            crate::settings::ThumbAspect::Portrait3x4
        );
        app.selected = Some(310);
        app.scroll_to_selected = true;
        let ctx = egui::Context::default();
        crate::ui_fonts::configure_fonts(&ctx);
        session1328_draw(&mut app, &ctx);
        session1328_draw(&mut app, &ctx);
        session1328_assert_visible(&app, &paths[310]);
        let offset = app.scroll_offset_y;
        app.settings.remember_favorite_view_state = true;
        let favorite = crate::settings::FavoriteEntry::new("child".into(), paths[310].clone());
        let mut state = crate::settings::FavoriteViewState::from_settings(&app.settings);
        state.grid_cols = 6;
        state.thumb_aspect_auto = !auto_off_child;
        app.favorite_view_states.insert(favorite.id, state);
        app.settings.favorites.push(favorite);
        session1328_open(&mut app, 310, paths[310].clone());
        assert_eq!(app.settings.grid_cols, 6);
        assert_eq!(app.settings.thumb_aspect_auto, !auto_off_child);
        app.selected = Some(0);
        app.dispatch_main_folder_history_input(FolderHistoryDirection::Back);
        session1328_settle(&mut app);
        app.cancel_token
            .store(true, std::sync::atomic::Ordering::Relaxed);
        assert_eq!(app.settings.grid_cols, 10);
        assert!(app.settings.thumb_aspect_auto);
        assert_eq!(app.selected, Some(310));
        assert_eq!(
            app.effective_thumb_aspect(),
            crate::settings::ThumbAspect::Portrait3x4
        );
        session1328_draw(&mut app, &ctx);
        session1328_assert_visible(&app, &paths[310]);
        app.reconcile_favorite_view_for_current_context_at(std::time::Instant::now());
        assert!(app.subfolder_expansion_install_pending.is_none());
        session1328_draw(&mut app, &ctx);
        session1328_assert_visible(&app, &paths[310]);
        app.last_input_at = None;
        for index in 0..24 {
            app.auto_aspect.samples.insert(index, 4.0 / 3.0);
        }
        app.maybe_apply_auto_aspect(true);
        session1328_draw(&mut app, &ctx);
        session1328_draw(&mut app, &ctx);
        session1328_assert_visible(&app, &paths[310]);
        assert_eq!(app.scroll_offset_y, offset);
    }
}

#[test]
fn section1328_session_reading_tall_return_destination_layout_and_late_samples() {
    session1328_tall_return(true, false);
    session1328_tall_return(true, true);
}
#[test]
fn section1328_session_subfolder_tall_return_destination_layout_and_late_samples() {
    session1328_tall_return(false, false);
    session1328_tall_return(false, true);
}

#[test]
fn section1328_session_rating_snapshot_rows_do_not_overwrite_position_before_or_after_unfix() {
    for unfix in [false, true] {
        let mut app = setup_app();
        app.active_quick_folder_slot = None;
        let root = app.tmp.path().join("source");
        std::fs::create_dir_all(&root).unwrap();
        app.items = vec![GridItem::Image(root.join("fixture.jpg"))];
        app.rating_view_rows = rating_1328_install_rows(&mut app);
        app.rating_view_stars = 3;
        app.install_rating_view_rows();
        app.finish_page_identity_preparation_for_test();
        app.selected = Some(4);
        app.scroll_offset_y = 600.0;
        let expected = app.rating_path_key(4).unwrap();
        app.activate_snapshot(crate::snapshot::SnapshotSourceLabel::Mixed);
        app.finish_page_identity_preparation_for_test();
        app.selected = Some(1);
        app.scroll_offset_y = 0.0;
        if unfix {
            app.deactivate_snapshot();
            // The surface has changed but the pending worker has not replaced Snapshot rows.
            assert!(app.rating_view_pending.is_some());
        }
        app.note_session_grid_positions();
        app.save_departing_session_grid_positions();
        let position = app.top_level_grid_view.rating_grid_position(3).unwrap();
        assert_eq!(position.selected_key.as_deref(), Some(expected.as_str()));
        assert_eq!(position.scroll_offset_y, 600.0);
        if !unfix {
            app.deactivate_snapshot();
        }
        session1328_settle(&mut app);
        // Rebuilt Rating rows may reorder by rated_at; restore identity, not the old index.
        assert_eq!(app.rating_path_key(app.selected.unwrap()), Some(expected));
        assert_eq!(app.scroll_offset_y, 600.0);
    }
}

#[test]
fn section1328_session_r13_cancelled_rating_close_does_not_mark_current_rows_departed() {
    let mut app = setup_app();
    app.active_quick_folder_slot = None;
    let root = app.tmp.path().join("source");
    session1328_expansion(&mut app, &root, 6);
    app.rating_view_return_to = Some(expansion_origin_for_test(&app));
    app.apply_rating_view_result(crate::rating_view::RatingViewBuildResult {
        visibility_notice: None,
        stars: 3,
        rows: vec![crate::rating_view::RatingViewRow {
            key: "fixture".into(),
            item: GridItem::Image(root.join("rated.png")),
            image_meta: Some((0, 0)),
            rated_at_ms: None,
        }],
        skipped: 0,
        prepared: None,
    });
    app.finish_page_identity_preparation_for_test();
    let generation = app.items_generation;
    assert_ne!(
        app.top_level_grid_view.departed_items_generation,
        Some(generation)
    );
    app.close_rating_view();
    assert!(app.subfolder_expansion_install_pending.is_some());
    app.cancel_subfolder_expansion_pending();
    assert_eq!(app.items_generation, generation);
    assert_ne!(
        app.top_level_grid_view.departed_items_generation,
        Some(generation),
        "close was only provisional"
    );
}

#[test]
fn section1328_session_positions_are_not_cloned_and_forget_clears_all_owners() {
    use crate::app::top_level_grid_view::{SessionGridPosition, SubfolderExpansionPositionKey};
    let mut app = setup_app();
    let root = app.tmp.path().join("source");
    let key = SubfolderExpansionPositionKey::new(&root, std::slice::from_ref(&root));
    let position = SessionGridPosition {
        scroll_offset_y: 400.0,
        selected_key: Some("row".into()),
        aspect_seed: None,
    };
    app.top_level_grid_view
        .save_reading_history_position(position.clone());
    app.top_level_grid_view
        .save_subfolder_expansion_position(key.clone(), position.clone());
    app.top_level_grid_view
        .save_rating_grid_position(3, position);
    app.top_level_grid_view.departed_items_generation = Some(app.items_generation);
    let cloned = app.top_level_grid_view.clone();
    assert!(cloned.reading_history_position().is_none());
    assert!(cloned.subfolder_expansion_position(&key).is_none());
    assert!(cloned.rating_grid_position(3).is_none());
    assert!(cloned.departed_items_generation.is_none());
    let other =
        SubfolderExpansionPositionKey::new(&root.join("other"), std::slice::from_ref(&root));
    assert!(
        app.top_level_grid_view
            .subfolder_expansion_position(&other)
            .is_none()
    );
    app.clear_quick_folder_slots();
    assert!(app.top_level_grid_view.reading_history_position().is_none());
    assert!(
        app.top_level_grid_view
            .subfolder_expansion_position(&key)
            .is_none()
    );
    assert!(app.top_level_grid_view.rating_grid_position(3).is_none());
    assert!(app.top_level_grid_view.departed_items_generation.is_none());
}

#[test]
fn section1328_session_reading_stable_key_tracks_mru_reorder() {
    let mut app = setup_app();
    app.active_quick_folder_slot = None;
    let root = app.tmp.path().join("source");
    let paths = session1328_reading(&mut app, &root, 6);
    session1328_open(&mut app, 4, paths[4].clone());
    let db = app.reading_history_db.as_ref().unwrap();
    let mut entry = db
        .list_recent(1000)
        .unwrap()
        .into_iter()
        .find(|entry| entry.path == paths[4])
        .unwrap();
    entry.last_read_at_ms = 2_000_000;
    db.upsert(entry, 1000).unwrap();
    app.dispatch_main_folder_history_input(FolderHistoryDirection::Back);
    session1328_settle(&mut app);
    assert_eq!(app.selected, Some(0));
    assert_eq!(app.items[0].container_path(), Some(paths[4].as_path()));
}

#[test]
fn section1328_session_subfolder_new_reinstall_and_mismatched_root_start_first() {
    let mut app = setup_app();
    app.active_quick_folder_slot = None;
    let root = app.tmp.path().join("source");
    let paths = session1328_expansion(&mut app, &root, 6);
    app.selected = Some(4);
    app.note_session_grid_positions();
    assert!(app.reinstall_subfolder_expansion_snapshot());
    session1328_settle(&mut app);
    assert_eq!(app.selected, Some(0));
    session1328_open(&mut app, 4, paths[4].clone());
    let state = app.folder_nav_subfolder_restore.as_mut().unwrap();
    let other = root.join("different-root");
    state.root = Some(other.clone());
    state.roots = vec![other.clone()];
    let snapshot = state.snapshot.as_mut().unwrap();
    snapshot.root = other.clone();
    snapshot.roots = vec![other];
    app.dispatch_main_folder_history_input(FolderHistoryDirection::Back);
    session1328_settle(&mut app);
    assert_eq!(app.selected, Some(0));
}
#[test]
fn section1328_session_subfolder_prepare_uses_destination_sort_and_category_order_for_new_and_return()
 {
    use crate::settings::{
        FavoriteEntry, FavoriteViewState, GridDisplayOrder, GridItemDisplayKind, SortOrder,
    };
    let mut app = setup_app();
    app.active_quick_folder_slot = None;
    app.settings.auto_fullscreen_image_folders = false;
    app.settings.sort_order = SortOrder::FileName;
    let common_order = GridDisplayOrder::from_rows([
        vec![GridItemDisplayKind::Image],
        vec![GridItemDisplayKind::Archive],
        vec![GridItemDisplayKind::Folder],
        vec![GridItemDisplayKind::VideoAudio],
    ]);
    app.settings.grid_display_order = common_order.clone();
    app.settings.remember_favorite_view_state = true;
    let root = app.tmp.path().join("favorite-root");
    std::fs::create_dir_all(&root).unwrap();
    image::RgbImage::new(3, 4).save(root.join("b.png")).unwrap();
    image::RgbImage::new(3, 4).save(root.join("c.png")).unwrap();
    write_1328_zip(&root.join("a.zip"));
    let favorite = FavoriteEntry::new("source".into(), root.clone());
    let mut state = FavoriteViewState::from_settings(&app.settings);
    state.sort_order = SortOrder::DateDesc;
    state.grid_display_order = GridDisplayOrder::default();
    app.favorite_view_states.insert(favorite.id, state);
    app.settings.favorites.push(favorite);
    app.load_folder(root.clone());
    session1328_settle(&mut app);
    assert_eq!(app.settings.sort_order, SortOrder::DateDesc);
    app.start_subfolder_expansion_scan_roots(root.clone(), vec![root.clone()]);
    // Projection must not apply destination presentation to the still-visible source.
    assert_eq!(app.settings.sort_order, SortOrder::DateDesc);
    session1328_settle(&mut app);
    for returning in [false, true] {
        assert_eq!(app.settings.sort_order, SortOrder::FileName);
        assert_eq!(app.settings.grid_display_order, common_order);
        assert_eq!(
            app.items
                .iter()
                .map(|item| item.name().into_owned())
                .collect::<Vec<_>>(),
            ["b.png", "c.png", "a.zip"]
        );
        let generation = app.items_generation;
        app.reconcile_favorite_view_for_current_context_at(std::time::Instant::now());
        assert_eq!(app.items_generation, generation);
        assert!(app.subfolder_expansion_install_pending.is_none());
        if returning {
            break;
        }
        app.selected = Some(2);
        app.scroll_offset_y = 400.0;
        app.load_folder(root.clone());
        session1328_settle(&mut app);
        assert_eq!(app.settings.sort_order, SortOrder::DateDesc);
        app.dispatch_main_folder_history_input(FolderHistoryDirection::Back);
        session1328_settle(&mut app);
        assert_eq!(app.selected, Some(2));
        assert_eq!(app.scroll_offset_y, 400.0);
    }
}
