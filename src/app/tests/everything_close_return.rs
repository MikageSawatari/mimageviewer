use super::*;
#[path = "rating_filter_return_1671.rs"]
mod rating1671;
use crate::global_search_ui::{
    EverythingPosition, SearchAccepted, adopt_everything_for_test as adopt,
    assert_everything_return_only_for_test, everything_dataset_for_test as dataset,
};

#[derive(Clone, Copy, Debug)]
enum Book {
    Zip,
    Pdf,
    Converted,
    Folder,
}

fn fixture(app: &mut App, root: &Path, book: Book) -> (GridItem, PathBuf) {
    let backing = root.join("cache.zip");
    let mut zip = zip::ZipWriter::new(std::fs::File::create(&backing).unwrap());
    zip.start_file("001.png", zip::write::SimpleFileOptions::default())
        .unwrap();
    let mut image = std::io::Cursor::new(Vec::new());
    image::RgbImage::new(2, 2)
        .write_to(&mut image, image::ImageFormat::Png)
        .unwrap();
    std::io::Write::write_all(&mut zip, image.get_ref()).unwrap();
    zip.finish().unwrap();
    match book {
        Book::Zip => (GridItem::ZipFile(backing.clone()), backing),
        Book::Pdf => {
            let pdf = root.join("book.pdf");
            std::fs::write(&pdf, b"%PDF-1.4\n").unwrap();
            (GridItem::PdfFile(pdf.clone()), pdf)
        }
        Book::Converted => {
            let source = root.join("book.7z");
            std::fs::write(&source, b"conversion source").unwrap();
            let stamp = std::fs::metadata(&source).unwrap();
            app.archive_cache_db
                .as_ref()
                .unwrap()
                .record(
                    &source,
                    crate::ui_helpers::mtime_secs(&stamp),
                    stamp.len() as i64,
                    crate::archive_converter::ArchiveFormat::SevenZ,
                    &backing,
                    std::fs::metadata(&backing).unwrap().len() as i64,
                    1,
                    false,
                )
                .unwrap();
            (
                GridItem::ConvertibleArchive {
                    path: source,
                    format: crate::archive_converter::ArchiveFormat::SevenZ,
                },
                backing,
            )
        }
        Book::Folder => {
            let folder = root.join("images");
            std::fs::create_dir(&folder).unwrap();
            std::fs::write(folder.join("001.png"), image.get_ref()).unwrap();
            (GridItem::Folder(folder.clone()), folder)
        }
    }
}

fn open(app: &mut App, ctx: &egui::Context, row: &GridItem, auto: bool) {
    let index = app.items.iter().position(|item| item == row).unwrap();
    app.selected = Some(index);
    let path = row.drag_source_path().unwrap().to_path_buf();
    if matches!(row, GridItem::ConvertibleArchive { .. }) {
        let owner = app.main_grid_archive_open_owner(index, &path);
        let outcome = app.load_folder_or_convert_archive_with_auto_fullscreen_owned(
            path.clone(),
            auto,
            owner,
            StartupListIntent::container_open(auto),
        );
        assert!(matches!(
            outcome,
            FolderOpenOutcome::Loaded | FolderOpenOutcome::Classifying
        ));
    } else {
        let nav = app.grid_physical_navigation(index, path.clone(), auto);
        assert!(app.apply_fullscreen_close_nav_immediate(nav));
    }
    let deadline = Instant::now() + Duration::from_secs(30);
    while !app.everything_has_physical_child() {
        app.settle_open_path_classification_for_test();
        if matches!(row, GridItem::PdfFile(_)) {
            // Inject only enumeration completion, retaining production open/install ownership.
            if let Some(pending) = app.pdf_enumerate_pending.as_mut() {
                pending.2 = crate::pdf_loader::completed_enumerate_result_handle(
                    &path,
                    Ok(crate::pdf_loader::PdfEnumerateResult {
                        pages: vec![crate::pdf_loader::PdfPageEntry {
                            page_num: 0,
                            mtime: 1,
                            file_size: 1,
                        }],
                        direction: None,
                        stamp: None,
                    }),
                );
            }
        }
        app.poll_sidecar_restore(ctx);
        app.poll_collection_history_transition(ctx);
        app.poll_zip_enumerate();
        app.poll_pdf_enumerate();
        app.finish_page_identity_preparation_for_test();
        assert!(Instant::now() < deadline, "open did not adopt: {row:?}");
        std::thread::yield_now();
    }
    app.finish_page_identity_preparation_for_test();
    assert!(
        matches!(&app.global_search.accepted, SearchAccepted::Everything {
        position: EverythingPosition::PhysicalChild { logical_source, return_anchor, .. }, ..
    } if logical_source == &path && return_anchor == &path)
    );
    if auto {
        app.reopen_fullscreen_after_folder_nav_load(ctx, false, false, HistoryTrigger::UserChosen);
        assert!(app.fullscreen_idx.is_some());
    }
}

#[test]
fn section1672_everything_physical_parent_selects_original_source_and_exits_results() {
    for (book, ordinary_folder) in [
        (Book::Folder, false),
        (Book::Folder, true),
        (Book::Zip, false),
        (Book::Pdf, false),
        (Book::Converted, false),
    ] {
        let mut app = setup_app_for_test();
        app.active_quick_folder_slot = None;
        app.settings.auto_fullscreen_zip_pdf = false;
        app.settings.auto_fullscreen_image_folders = false;
        app.settings.detached_viewer_open_images_in_window = false;
        let ctx = egui::Context::default();
        let root = app.tmp.path().to_path_buf();
        let (row, _) = fixture(&mut app, &root, book);
        let source = row.drag_source_path().unwrap().to_owned();
        if ordinary_folder {
            std::fs::create_dir(source.join("child")).unwrap();
        }
        app.current_folder = Some(root.clone());
        adopt(
            &mut app,
            &ctx,
            "physical parent",
            dataset(vec![row.clone()]),
        );
        assert!(!app.grid_physical_parent_action().available());
        open(&mut app, &ctx, &row, false);
        assert!(matches!(
            app.grid_parent_action(),
            crate::app::grid_parent::GridParentAction::GlobalSearchBack
        ));
        assert!(app.grid_physical_parent_action().available());
        app.handle_grid_physical_parent_action();
        let deadline = Instant::now() + Duration::from_secs(30);
        while app.folder_pane_open_pending.is_some() || app.sidecar_restore_active() {
            if let Some(ready) = app.poll_folder_pane_open(&ctx) {
                assert!(app.resolve_main_folder_open_ready(&ctx, ready).is_none());
            }
            app.poll_sidecar_restore(&ctx);
            assert!(Instant::now() < deadline);
            std::thread::yield_now();
        }
        assert_eq!(app.current_folder.as_ref(), Some(&root));
        assert!(!app.global_search.active);
        assert!(
            app.selected
                .is_some_and(|index| app.items[index].drag_source_path() == Some(source.as_path())),
            "{book:?}/{ordinary_folder}"
        );
        assert!(
            matches!(app.grid_parent_nav_target(), Some(crate::ui_main::AddressBarNav::Direct(ref parent, _)) if Some(parent.as_path()) == root.parent())
        );
    }
}

fn close_returns_results(book: Book) {
    let mut app = setup_app_for_test();
    let ctx = egui::Context::default();
    app.settings.auto_fullscreen_zip_pdf = true;
    app.settings.auto_fullscreen_image_folders = true;
    app.settings.detached_viewer_open_images_in_window = false;
    app.settings.sort_order = crate::settings::SortOrder::FileNameDesc;
    let root = app.tmp.path().to_path_buf();
    app.current_folder = Some(root.clone());
    let (row, backing) = fixture(&mut app, &root, book);
    let result = dataset(vec![GridItem::Image(root.join("a.png")), row.clone()]);
    adopt(&mut app, &ctx, "book query", Arc::clone(&result));
    let before = app.items.clone();
    let visibility = app.global_search.visibility.clone();
    let facet = app.settings.facet_filter.clone();
    let run = app.global_search.run_sequence;
    for immediate in [true, false] {
        open(&mut app, &ctx, &row, true);
        assert_eq!(app.current_folder.as_ref(), Some(&backing));
        if immediate {
            app.handle_fullscreen_close_request_immediate();
        } else {
            // Esc/Enter/HUD all enqueue this same close request; exercise the deferred
            // consumer as well as the native/immediate consumer on the second open.
            app.handle_fullscreen_close_request();
            assert!(app.pending_return_to_parent.is_pending());
            let nav = app
                .take_pending_return_to_parent_nav()
                .expect("close return navigation");
            app.post_render_main_navigation(
                &ctx,
                update_phases::MainFrameNavigation {
                    fav_nav: None,
                    toolbar_fav_nav: None,
                    fullscreen_close_nav: Some(nav),
                    keyboard_nav: None,
                    keyboard_nav_from_fullscreen_close: false,
                    gamepad_nav: None,
                    address_nav: None,
                    open_folder_nav: None,
                    context_nav: None,
                    folder_pane_nav: None,
                    grid_nav: None,
                },
            );
        }
        assert_everything_return_only_for_test(&app);
        let deadline = Instant::now() + Duration::from_secs(30);
        while app.everything_has_physical_child() {
            app.poll_search_page_edit_prepare(&ctx);
            assert!(Instant::now() < deadline, "close return did not adopt");
            std::thread::yield_now();
        }
        assert_eq!(app.fullscreen_idx, None);
        assert_eq!(app.items, before);
        assert_eq!(app.items[app.selected.unwrap()], row);
        assert!(app.grid_scroll.reveal_requested());
        assert_eq!(app.global_search.last_executed, "book query");
        assert_eq!(app.global_search.run_sequence, run);
        assert_eq!(app.global_search.visibility, visibility);
        assert_eq!(app.settings.facet_filter, facet);
        assert_eq!(
            app.settings.sort_order,
            crate::settings::SortOrder::FileNameDesc
        );
        assert!(
            matches!(&app.global_search.accepted, SearchAccepted::Everything { dataset, .. } if Arc::ptr_eq(dataset, &result))
        );
    }
}

#[test]
fn zip_normal_close_returns_results() {
    close_returns_results(Book::Zip);
}
#[test]
fn pdf_normal_close_returns_results() {
    close_returns_results(Book::Pdf);
}
#[test]
fn converted_normal_close_returns_results() {
    close_returns_results(Book::Converted);
}
#[test]
fn image_folder_normal_close_returns_results() {
    close_returns_results(Book::Folder);
}

fn extension_named_directory_close(image_folder_book: bool) {
    for name in ["photos.zip", "photos.pdf"] {
        let mut app = setup_app_for_test();
        let ctx = egui::Context::default();
        app.settings.auto_fullscreen_zip_pdf = true;
        app.settings.auto_fullscreen_image_folders = image_folder_book;
        let root = app.tmp.path().to_path_buf();
        app.current_folder = Some(root.clone());
        let (folder, _) = fixture(&mut app, &root, Book::Folder);
        let path = root.join(name);
        std::fs::rename(folder.drag_source_path().unwrap(), &path).unwrap();
        let row = GridItem::Folder(path.clone());
        let results = dataset(vec![row.clone()]);
        adopt(&mut app, &ctx, "photos", results);
        open(&mut app, &ctx, &row, image_folder_book);
        if !image_folder_book {
            assert_eq!(app.fullscreen_idx, None);
            app.open_fullscreen(0, HistoryTrigger::UserChosen);
        }
        key(&mut app, &ctx, egui::Key::Escape);
        if let Some(nav) = app.take_pending_return_to_parent_nav() {
            assert!(app.apply_fullscreen_close_nav_immediate(nav));
        }
        if image_folder_book {
            finish_results_return(&mut app, &ctx);
            assert_eq!(app.items[app.selected.unwrap()], row);
            assert!(app.grid_scroll.reveal_requested());
        } else {
            assert!(
                app.global_search.pending.is_none(),
                "ordinary directory: {name}"
            );
            assert!(app.everything_has_physical_child());
            assert_eq!(app.current_folder.as_ref(), Some(&path));
            assert!(app.items_are_image_only_folder_pages());
        }
        assert_eq!(app.fullscreen_idx, None);
    }
}

#[test]
fn extension_named_ordinary_directory_close_keeps_its_list() {
    extension_named_directory_close(false);
}

#[test]
fn extension_named_image_folder_book_close_returns_results() {
    extension_named_directory_close(true);
}

#[test]
fn close_return_requires_the_accepted_book_and_logical_source() {
    use crate::app::book_materialization::{BookScope, ProjectionRecipe};
    for book in [Book::Zip, Book::Pdf, Book::Converted, Book::Folder] {
        let mut app = setup_app_for_test();
        let ctx = egui::Context::default();
        app.settings.auto_fullscreen_zip_pdf = true;
        app.settings.auto_fullscreen_image_folders = true;
        let root = app.tmp.path().to_path_buf();
        app.current_folder = Some(root.clone());
        let (row, _) = fixture(&mut app, &root, book);
        adopt(&mut app, &ctx, "book", dataset(vec![row.clone()]));
        open(&mut app, &ctx, &row, true);
        let recipe = app.projection_recipe.clone();
        let folder = app.current_folder.clone();
        let logical_override = app.archive_source_override.clone();
        assert!(app.everything_book_close_return_available());
        app.projection_recipe = ProjectionRecipe::OrdinaryList;
        assert!(!app.auto_open_for_current_container());
        assert!(!matches!(
            app.resolve_close_return_navigation(),
            Some((crate::ui_main::AddressBarNav::EverythingResults, _))
        ));
        app.projection_recipe = ProjectionRecipe::Book(BookScope::ImageFolder(root.join("other")));
        assert!(!app.everything_book_close_return_available());
        app.projection_recipe = recipe;
        // Current physical descendants and a conversion cache are not the accepted source.
        app.archive_source_override = None;
        app.current_folder = Some(row.drag_source_path().unwrap().join("nested.zip"));
        assert!(!app.auto_open_for_current_container());
        app.current_folder = folder;
        if matches!(book, Book::Converted) {
            assert!(!app.everything_book_close_return_available());
        }
        app.archive_source_override = logical_override;
        assert!(app.everything_book_close_return_available());
        if let ProjectionRecipe::Book(BookScope::Archive { prefix, .. }) =
            &mut app.projection_recipe
        {
            prefix.push("inside".into());
            assert!(
                app.everything_book_close_return_available(),
                "same archive, internal prefix"
            );
        }
    }
}

fn finish_results_return(app: &mut App, ctx: &egui::Context) {
    assert_everything_return_only_for_test(app);
    let deadline = Instant::now() + Duration::from_secs(30);
    while app.everything_has_physical_child() {
        app.poll_search_page_edit_prepare(ctx);
        assert!(Instant::now() < deadline, "results return did not adopt");
        std::thread::yield_now();
    }
}

// The detached viewport uses this same key/navigation pair. Unlike key(), this
// helper does not overwrite the presentation selected by the real F12 handler.
fn viewer_key(app: &mut App, ctx: &egui::Context, key: egui::Key) {
    let viewport_id = app.fullscreen_viewport_id();
    ctx.begin_pass(egui::RawInput {
        viewport_id,
        viewports: [
            (egui::ViewportId::ROOT, egui::ViewportInfo::default()),
            (
                viewport_id,
                egui::ViewportInfo {
                    parent: Some(egui::ViewportId::ROOT),
                    ..Default::default()
                },
            ),
        ]
        .into_iter()
        .collect(),
        events: vec![egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        }],
        ..Default::default()
    });
    let idx = app.fullscreen_idx.expect("open viewer");
    let action = app.handle_fs_key_input(ctx, idx, false);
    if key == egui::Key::Escape {
        assert!(action.close, "viewer Escape must request normal close");
    }
    app.handle_fs_navigation(
        ctx,
        action.close,
        action.close_to_page_list,
        action.ctrl_nav,
        action.sibling_nav,
        action.mouse_nav,
        action.page_nav,
        action.jump_to,
        idx,
    );
    ctx.end_pass().drop_without_applying_deltas();
}

#[cfg(windows)]
#[test]
fn real_f12_linked_viewer_close_returns_results_in_main_context() {
    for book in [Book::Zip, Book::Pdf, Book::Converted, Book::Folder] {
        let mut app = setup_app_for_test();
        let ctx = egui::Context::default();
        app.settings.auto_fullscreen_zip_pdf = true;
        app.settings.auto_fullscreen_image_folders = true;
        app.settings.detached_viewer_open_images_in_window = false;
        app.settings.detached_viewer_enabled = false;
        let root = app.tmp.path().to_path_buf();
        app.current_folder = Some(root.clone());
        let (row, _) = fixture(&mut app, &root, book);
        adopt(
            &mut app,
            &ctx,
            "book",
            dataset(vec![row.clone(), GridItem::Image(root.join("other.png"))]),
        );
        let results = app.items.clone();
        open(&mut app, &ctx, &row, true);
        key(&mut app, &ctx, egui::Key::F12);
        assert!(app.settings.detached_viewer_enabled);
        assert_eq!(app.viewer_presentation, ViewerPresentation::DetachedWindow);
        assert!(!app.detached_viewer_independent_active);
        let session = app.active_detached_session.expect("F12 session");
        assert_eq!(
            app.locate_window_context(session.window_id),
            Some((app.viewer_context_main(), ContextResidence::Mounted))
        );
        // Emulate the existing render-completion notification without a native window.
        let viewport_id = app.fullscreen_viewport_id();
        ctx.begin_pass(egui::RawInput {
            viewport_id,
            viewports: [
                (egui::ViewportId::ROOT, egui::ViewportInfo::default()),
                (
                    viewport_id,
                    egui::ViewportInfo {
                        parent: Some(egui::ViewportId::ROOT),
                        ..Default::default()
                    },
                ),
            ]
            .into_iter()
            .collect(),
            ..Default::default()
        });
        app.mark_active_detached_viewport_rendered(&ctx);
        app.fs_viewport_shown = true;
        app.fs_viewport_presentation = Some(ViewerPresentation::DetachedWindow);
        ctx.end_pass().drop_without_applying_deltas();
        let main = app.viewer_context_main();
        app.with_active_viewer_context(|viewer| {
            assert_eq!(viewer.projected_viewer_context_id(), main);
            assert!(viewer.global_search.active);
            assert!(viewer.everything_has_physical_child());
            viewer_key(viewer, &ctx, egui::Key::Escape);
        })
        .expect("linked viewer");
        let nav = app
            .take_pending_return_to_parent_nav()
            .expect("linked close return");
        assert!(app.apply_fullscreen_close_nav_immediate(nav));
        finish_results_return(&mut app, &ctx);
        assert_eq!(app.items, results);
        assert_eq!(app.items[app.selected.unwrap()], row);
        assert!(app.grid_scroll.reveal_requested());
        assert_eq!(app.fullscreen_idx, None);
        // Existing inactive-viewport cleanup owns the actual session/window retirement.
        ctx.begin_pass(egui::RawInput::default());
        app.keep_fullscreen_viewport_alive(&ctx);
        ctx.end_pass().drop_without_applying_deltas();
        assert!(app.active_detached_session.is_none());
        assert_eq!(app.locate_window_context(session.window_id), None);
        eprintln!(
            "linked F12 close: book={book:?} main={main:?} session_before={session:?} session_after={:?}",
            app.active_detached_session
        );
    }
}

fn key(app: &mut App, ctx: &egui::Context, key: egui::Key) {
    app.viewer_presentation = ViewerPresentation::Fullscreen;
    app.native_video_in_window_active = false;
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
    assert!(app.handle_fullscreen_root_key_input(ctx));
    ctx.end_pass().drop_without_applying_deltas();
}

#[test]
fn backspace_and_page_list_setting_keep_the_open_book_list() {
    for book in [Book::Zip, Book::Pdf, Book::Converted, Book::Folder] {
        for auto in [true, false] {
            let mut app = setup_app_for_test();
            let ctx = egui::Context::default();
            app.settings.auto_fullscreen_zip_pdf = auto;
            app.settings.auto_fullscreen_image_folders = true;
            let root = app.tmp.path().to_path_buf();
            app.current_folder = Some(root.clone());
            let (row, backing) = fixture(&mut app, &root, book);
            adopt(&mut app, &ctx, "book", dataset(vec![row.clone()]));
            open(&mut app, &ctx, &row, auto);
            if auto {
                key(&mut app, &ctx, egui::Key::Backspace);
            } else {
                assert_eq!(
                    app.fullscreen_idx, None,
                    "list setting must not open a page"
                );
                app.open_fullscreen(0, HistoryTrigger::UserChosen);
                app.handle_fullscreen_close_request_immediate();
            }
            assert_eq!(app.fullscreen_idx, None);
            assert_eq!(app.current_folder.as_ref(), Some(&backing));
            assert!(app.everything_has_physical_child());
            assert!(app.global_search.pending.is_none());
            assert!(!app.pending_return_to_parent.is_pending());
            assert!(!app.items_are_global_search_view);
        }
    }
}

#[test]
fn escape_and_enter_close_use_the_everything_return_owner() {
    for input in [egui::Key::Escape, egui::Key::Enter] {
        let mut app = setup_app_for_test();
        let ctx = egui::Context::default();
        app.settings.auto_fullscreen_zip_pdf = true;
        let root = app.tmp.path().to_path_buf();
        app.current_folder = Some(root.clone());
        let (row, _) = fixture(&mut app, &root, Book::Zip);
        adopt(&mut app, &ctx, "book", dataset(vec![row.clone()]));
        open(&mut app, &ctx, &row, true);
        key(&mut app, &ctx, input);
        let nav = app
            .take_pending_return_to_parent_nav()
            .expect("key close must request results");
        assert!(app.apply_fullscreen_close_nav_immediate(nav));
        assert_everything_return_only_for_test(&app);
    }
}

#[test]
fn metadata_favorite_and_tag_search_still_close_to_page_list() {
    for source in 0..3 {
        let mut app = setup_app_for_test();
        app.settings.auto_fullscreen_zip_pdf = true;
        let zip = app.tmp.path().join("book.zip");
        app.current_folder = Some(zip.clone());
        app.items = vec![GridItem::ZipImage {
            zip_path: zip.clone(),
            entry_name: "001.png".into(),
        }];
        app.thumbnails = vec![ThumbnailState::Pending];
        app.fullscreen_idx = Some(0);
        match source {
            0 => app.global_search.active = true,
            1 => app.favsearch.active = true,
            _ => app.tag_view.active = true,
        }
        assert!(!app.auto_open_for_current_container());
        app.handle_fullscreen_close_request_immediate();
        assert_eq!(app.fullscreen_idx, None);
        assert_eq!(app.current_folder.as_ref(), Some(&zip));
        assert!(!app.pending_return_to_parent.is_pending());
    }
}

#[test]
fn canceled_and_failed_pdf_opens_keep_results_without_a_child_return() {
    for cancel in [true, false] {
        let mut app = setup_app_for_test();
        let ctx = egui::Context::default();
        let root = app.tmp.path().to_path_buf();
        app.current_folder = Some(root.clone());
        let (row, path) = fixture(&mut app, &root, Book::Pdf);
        let result = dataset(vec![row]);
        adopt(&mut app, &ctx, "book", Arc::clone(&result));
        let rows = app.items.clone();
        assert_eq!(
            app.load_pdf_as_folder_owned(
                path.clone(),
                OpenRequestOwner::Navigation,
                StartupListIntent::ExplicitList
            ),
            FolderOpenOutcome::Loaded
        );
        assert!(!app.everything_has_physical_child());
        if cancel {
            app.pdf_enumerate_pending
                .as_ref()
                .unwrap()
                .2
                .cancel
                .store(true, Ordering::Relaxed);
        } else {
            app.pdf_enumerate_pending.as_mut().unwrap().2 =
                crate::pdf_loader::completed_enumerate_result_handle(
                    &path,
                    Err(std::io::Error::other("fake PDF open failure")),
                );
        }
        app.poll_pdf_enumerate();
        assert!(!app.everything_has_physical_child());
        assert_eq!(app.items, rows);
        assert!(app.global_search.pending.is_none());
        assert!(
            matches!(&app.global_search.accepted, SearchAccepted::Everything { dataset, .. } if Arc::ptr_eq(dataset, &result))
        );
    }
}

#[cfg(windows)]
#[test]
fn detached_close_keeps_main_results_in_both_window_modes() {
    for multi_window in [true, false] {
        let mut app = setup_app_for_test();
        let ctx = egui::Context::default();
        app.settings.auto_fullscreen_zip_pdf = true;
        app.settings.auto_fullscreen_image_folders = true;
        app.settings.detached_viewer_open_images_in_window = multi_window;
        app.settings.detached_viewer_enabled = true;
        let root = app.tmp.path().to_path_buf();
        app.current_folder = Some(root.clone());
        let (row, folder) = fixture(&mut app, &root, Book::Folder);
        // §1.671: independent readers do not borrow main's filtered membership
        // or establish suppression in main. Keep the §1.670 HEAD return contract.
        let filter = [false, false, false, false, false, true];
        app.settings.rating_filter = filter;
        app.rating_db
            .as_ref()
            .unwrap()
            .set(&crate::adjustment_db::normalize_path(&folder), 5)
            .unwrap();
        let result = dataset(vec![row.clone(), GridItem::Image(root.join("other.png"))]);
        adopt(&mut app, &ctx, "images", Arc::clone(&result));
        app.selected = Some(app.items.iter().position(|item| item == &row).unwrap());
        app.grid_scroll.set_position(240.0);
        let rows = app.items.clone();
        let selected = app.selected;
        let generation = app.items_generation;
        let epoch = app.pdf_render_epoch.snapshot();
        assert!(app.start_active_detached_book_context_from_scanned_folder(
            folder.clone(),
            crate::app::folder_scan::scan_directory(&folder),
            &ctx,
            None
        ));
        // Independent open and close both preserve the accepted main list.
        assert_eq!(app.items, rows);
        assert_eq!(app.selected, selected);
        assert_eq!(app.items_generation, generation);
        assert_eq!(app.grid_scroll.offset(), 240.0);
        assert!(epoch.is_current());
        assert!(app.rating_filter_suppressed_at.is_none());
        assert_eq!(app.effective_rating_filter(), filter);
        let main_context = app.viewer_context_main();
        app.with_active_viewer_context(|viewer| {
            viewer.finish_page_identity_preparation_for_test();
            assert!(viewer.fullscreen_idx.is_some());
            assert!(viewer.rating_filter_suppressed_at.is_none());
            assert_eq!(viewer.effective_rating_filter(), filter);
            assert_eq!(viewer.visible_indices.len(), 1, "independent reader keeps the unrated page");
            assert_ne!(viewer.projected_viewer_context_id(), main_context);
            assert!(!viewer.global_search.active);
            assert!(matches!(viewer.global_search.accepted, SearchAccepted::Metadata { .. }));
            assert!(!viewer.everything_has_physical_child());
            if multi_window {
                let enabled = viewer.settings.detached_viewer_enabled;
                let presentation = viewer.viewer_presentation;
                let idx = viewer.fullscreen_idx;
                viewer_key(viewer, &ctx, egui::Key::F12);
                assert_eq!(viewer.settings.detached_viewer_enabled, enabled);
                assert_eq!(viewer.viewer_presentation, presentation);
                assert_eq!(viewer.fullscreen_idx, idx);
                assert_ne!(viewer.projected_viewer_context_id(), main_context);
            }
            assert!(viewer.auto_open_for_current_container());
            assert!(matches!(viewer.resolve_close_return_navigation(),
                Some((crate::ui_main::AddressBarNav::Direct(ref parent, _), _)) if parent == &root));
            assert!(viewer.close_return_origin().is_none());
            viewer.handle_fullscreen_close_request_immediate();
        })
        .expect("independent reader");
        assert_eq!(app.current_folder.as_ref(), Some(&root));
        assert_eq!(app.items, rows);
        assert_eq!(app.selected, selected);
        assert_eq!(app.items_generation, generation);
        assert_eq!(app.grid_scroll.offset(), 240.0);
        assert!(epoch.is_current());
        assert!(app.global_search.active);
        assert!(!app.pending_return_to_parent.is_pending());
        assert!(app.rating_filter_suppressed_at.is_none());
        assert_eq!(app.effective_rating_filter(), filter);
        assert_eq!(app.settings.rating_filter, filter);
        assert!(app.global_search.pending.is_none());
        assert!(!app.everything_has_physical_child());
        assert!(
            matches!(&app.global_search.accepted, SearchAccepted::Everything { dataset, .. } if Arc::ptr_eq(dataset, &result))
        );
        eprintln!(
            "independent close completed: multi_window={multi_window} main_selected={:?} main_scroll={}",
            app.selected,
            app.grid_scroll.offset()
        );
    }
}

#[test]
fn section1708_physical_result_open_retires_search_focus_without_resubmitting() {
    for book in [Book::Folder, Book::Zip, Book::Pdf, Book::Converted] {
        let mut app = setup_app_for_test();
        app.active_quick_folder_slot = None;
        app.settings.auto_fullscreen_zip_pdf = false;
        app.settings.auto_fullscreen_image_folders = false;
        app.settings.detached_viewer_open_images_in_window = false;
        let ctx = egui::Context::default();
        let root = app.tmp.path().to_owned();
        let (row, _) = fixture(&mut app, &root, book);
        let result = dataset(vec![row.clone()]);
        adopt(&mut app, &ctx, "outer book", Arc::clone(&result));
        assert!(
            app.global_search.focus_request,
            "opening the search reserves its input"
        );
        let run = app.global_search.run_sequence;
        open(&mut app, &ctx, &row, false);
        assert!(
            !app.global_search.focus_request,
            "the adopted child owns grid input: {book:?}"
        );
        assert!(!app.global_search.has_focus);
        assert_eq!(app.global_search.run_sequence, run);
        assert!(app.global_search.pending.is_none());
        assert!(matches!(&app.global_search.accepted,
            SearchAccepted::Everything { dataset, .. } if Arc::ptr_eq(dataset, &result)));
        ctx.begin_pass(egui::RawInput {
            events: vec![egui::Event::Key {
                key: egui::Key::Enter,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            }],
            ..Default::default()
        });
        let _ = app.handle_keyboard(&ctx);
        ctx.end_pass().drop_without_applying_deltas();
        assert_eq!(
            app.global_search.run_sequence, run,
            "child Enter must not resubmit Everything"
        );
        assert!(app.global_search.pending.is_none());
    }
}
