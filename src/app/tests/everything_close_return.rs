use super::*;
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
fn detached_close_matches_head_return_in_both_window_modes() {
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
        // Independent open preserves main; the pre-existing terminal close recipe below
        // then returns main to the real parent. This is the HEAD comparison contract,
        // not the desired search-result-preserving behavior (backlog 1.664 follow-up).
        assert_eq!(app.items, rows);
        assert_eq!(app.selected, selected);
        assert_eq!(app.items_generation, generation);
        assert_eq!(app.grid_scroll.offset(), 240.0);
        assert!(epoch.is_current());
        let main_context = app.viewer_context_main();
        app.with_active_viewer_context(|viewer| {
            viewer.finish_page_identity_preparation_for_test();
            assert!(viewer.fullscreen_idx.is_some());
            assert_ne!(viewer.projected_viewer_context_id(), main_context);
            assert!(!viewer.global_search.active);
            assert!(matches!(viewer.global_search.accepted, SearchAccepted::Metadata { .. }));
            assert!(!viewer.everything_has_physical_child());
            assert!(viewer.auto_open_for_current_container());
            assert!(matches!(viewer.resolve_close_return_navigation(),
                Some((crate::ui_main::AddressBarNav::Direct(ref parent, _), _)) if parent == &root));
            eprintln!("independent close: multi_window={multi_window} main={main_context:?} reader={:?} search_active={} accepted=Metadata physical_child=false return=physical_parent",
                viewer.projected_viewer_context_id(), viewer.global_search.active);
            viewer.handle_fullscreen_close_request_immediate();
        })
        .expect("independent reader");
        assert_eq!(app.current_folder.as_ref(), Some(&root));
        assert_eq!(
            app.items,
            vec![
                GridItem::ZipFile(root.join("cache.zip")),
                GridItem::Folder(folder),
                GridItem::Folder(root.join("logs")),
            ],
            "independent close must retain the observed HEAD parent-list behavior"
        );
        assert!(!app.pending_return_to_parent.is_pending());
        assert!(app.global_search.pending.is_none());
        assert!(!app.everything_has_physical_child());
        assert!(
            matches!(&app.global_search.accepted, SearchAccepted::Everything { dataset, .. } if Arc::ptr_eq(dataset, &result))
        );
        eprintln!(
            "HEAD-equivalent close completed: multi_window={multi_window} main_selected={:?} main_scroll={}",
            app.selected,
            app.grid_scroll.offset()
        );
    }
}
