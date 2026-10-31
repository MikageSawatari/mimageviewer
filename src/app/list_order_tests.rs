use super::tests::phase_c_support::setup_app;
use super::*;
use top_level_grid_view::{TopLevelGridSurface, TopLevelSearchView};

#[cfg(test)]
#[path = "list_order_ratingnav649_tests.rs"]
mod ratingnav649;

fn install_rows(app: &mut App, folder: &Path, rows: Vec<GridItem>) {
    app.current_folder = Some(folder.to_path_buf());
    app.archive_source_override = None;
    app.items = rows;
    app.image_metas = vec![None; app.items.len()];
    app.thumbnails = vec![ThumbnailState::Pending; app.items.len()];
    app.visible_indices = (0..app.items.len()).collect();
    app.selected = Some(0);
    app.top_level_grid_view
        .replace_surface(TopLevelGridSurface::Folder);
}

fn captured_paths(app: &App) -> Vec<PathBuf> {
    app.list_order
        .as_ref()
        .unwrap()
        .entries
        .iter()
        .map(|entry| entry.path.clone())
        .collect()
}

fn capture_fixture(app: &mut App, folder: &Path) -> Arc<ListOrderSnapshot> {
    install_rows(
        app,
        folder,
        vec![
            GridItem::Folder(folder.join("book-a")),
            GridItem::ZipFile(folder.join("book-b.zip")),
        ],
    );
    app.capture_main_folder_list_order(0);
    app.list_order.as_ref().unwrap().clone()
}

fn grid_enter(app: &mut App) -> Option<crate::ui_main::AddressBarNav> {
    let ctx = egui::Context::default();
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
    let result = app.handle_keyboard(&ctx);
    ctx.end_pass().drop_without_applying_deltas();
    result
}

#[test]
fn list_order_capture_copies_only_books_in_display_order_and_freezes_them() {
    let mut app = setup_app();
    let folder = app.tmp.path().join("capture");
    let book = folder.join("Book");
    install_rows(
        &mut app,
        &folder,
        vec![
            GridItem::Folder(book.clone()),
            GridItem::Image(folder.join("page.png")),
            GridItem::ZipFile(folder.join("b.zip")),
            GridItem::PdfFile(folder.join("c.epub")),
            GridItem::ConvertibleArchive {
                path: folder.join("d.7z"),
                format: crate::archive_converter::ArchiveFormat::SevenZ,
            },
            GridItem::Video(folder.join("movie.mp4")),
            GridItem::Audio(folder.join("music.mp3")),
            GridItem::Folder(folder.join("book")),
        ],
    );
    app.visible_indices = vec![4, 1, 3, 6, 2, 0, 7, 5];
    app.capture_main_folder_list_order(2);
    let snapshot = app.list_order.as_ref().unwrap().clone();
    assert_eq!(snapshot.owner, app.viewer_context_main());
    assert_eq!(
        captured_paths(&app),
        vec![
            folder.join("d.7z"),
            folder.join("c.epub"),
            folder.join("b.zip"),
            book
        ]
    );
    assert_eq!(
        snapshot
            .entries
            .iter()
            .map(|entry| entry.kind)
            .collect::<Vec<_>>(),
        vec![
            ListOrderEntryKind::ConvertibleArchive,
            ListOrderEntryKind::Pdf,
            ListOrderEntryKind::Zip,
            ListOrderEntryKind::Folder
        ]
    );
    app.visible_indices.reverse();
    app.items.clear();
    assert!(Arc::ptr_eq(app.list_order.as_ref().unwrap(), &snapshot));
    assert_eq!(snapshot.entries.len(), 4);
}

#[test]
fn list_order_capture_uses_real_details_name_and_modified_header_sorts_both_ways() {
    use crate::settings::{DetailsSortKey, GridViewMode};
    let mut app = setup_app();
    let folder = app.tmp.path().join("details");
    install_rows(
        &mut app,
        &folder,
        vec![
            GridItem::ZipFile(folder.join("c.zip")),
            GridItem::ZipFile(folder.join("a.zip")),
            GridItem::ZipFile(folder.join("b.zip")),
        ],
    );
    app.image_metas = vec![Some((10, 1)), Some((30, 1)), Some((20, 1))];
    app.settings.grid_view_mode = GridViewMode::Details;
    app.settings.details_show_modified = true;
    for (key, ascending, descending) in [
        (DetailsSortKey::Name, vec![1, 2, 0], vec![0, 2, 1]),
        (DetailsSortKey::Modified, vec![0, 2, 1], vec![1, 2, 0]),
    ] {
        app.settings.details_sort_key = DetailsSortKey::Toolbar;
        app.set_details_sort_key(key);
        assert_eq!(app.current_grid_order(), ascending);
        app.capture_main_folder_list_order(0);
        assert_eq!(
            captured_paths(&app),
            ascending
                .iter()
                .map(|&idx| app.items[idx].file_operation_path().unwrap().to_path_buf())
                .collect::<Vec<_>>()
        );
        app.set_details_sort_key(key);
        assert_eq!(app.current_grid_order(), descending);
        app.capture_main_folder_list_order(0);
        assert_eq!(
            captured_paths(&app),
            descending
                .iter()
                .map(|&idx| app.items[idx].file_operation_path().unwrap().to_path_buf())
                .collect::<Vec<_>>()
        );
    }
}

#[test]
fn list_order_capture_uses_folder_listing_name_and_date_sorts_both_ways() {
    let mut app = setup_app();
    let folder = app.tmp.path().join("toolbar-sort");
    std::fs::create_dir(&folder).unwrap();
    for (name, seconds) in [("a.zip", 300), ("b.zip", 100), ("c.zip", 200)] {
        let file = std::fs::File::create(folder.join(name)).unwrap();
        file.set_modified(std::time::UNIX_EPOCH + std::time::Duration::from_secs(seconds))
            .unwrap();
    }
    for (sort, expected) in [
        (
            crate::settings::SortOrder::FileName,
            ["a.zip", "b.zip", "c.zip"],
        ),
        (
            crate::settings::SortOrder::FileNameDesc,
            ["c.zip", "b.zip", "a.zip"],
        ),
        (
            crate::settings::SortOrder::DateAsc,
            ["b.zip", "c.zip", "a.zip"],
        ),
        (
            crate::settings::SortOrder::DateDesc,
            ["a.zip", "c.zip", "b.zip"],
        ),
    ] {
        app.settings.sort_order = sort;
        app.load_folder(folder.clone());
        app.finish_page_identity_preparation_for_test();
        app.capture_main_folder_list_order(0);
        assert_eq!(
            captured_paths(&app),
            expected.map(|name| folder.join(name)),
            "{sort:?}"
        );
    }
}

#[test]
fn list_order_capture_intersects_ctrl_f_facet_name_and_rating_filters() {
    let mut app = setup_app();
    let folder = app.tmp.path().join("filters");
    install_rows(
        &mut app,
        &folder,
        vec![
            GridItem::ZipFile(folder.join("keep-alpha.zip")),
            GridItem::ZipFile(folder.join("keep-beta.zip")),
            GridItem::ZipFile(folder.join("other-alpha.zip")),
            GridItem::ZipFile(folder.join("keep-alpha-low.zip")),
        ],
    );
    app.rating_cache.extend([(0, 5), (1, 5), (2, 5), (3, 1)]);
    app.settings.rating_filter = [false, false, false, false, false, true];
    app.settings.facet_filter.name_query = "keep".into();
    app.facet_name_tokens = crate::search_query::parse("keep");
    app.facet_name_cache = app
        .items
        .iter()
        .map(|item| item.name().to_lowercase().into_boxed_str())
        .collect();
    app.facet_name_cache_generation = Some(app.items_generation);
    app.search_query = "alpha".into();
    app.search_target =
        crate::fts_index::SearchTarget::Only(vec![crate::fts_index::SourceKind::Filename]);
    let ctx = egui::Context::default();
    app.execute_search(&ctx);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while app.search_pending.is_some() {
        assert!(
            std::time::Instant::now() < deadline,
            "local filename search did not finish"
        );
        app.poll_search(&ctx);
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    assert_eq!(app.visible_indices, vec![0]);
    app.capture_main_folder_list_order(0);
    assert_eq!(captured_paths(&app), vec![folder.join("keep-alpha.zip")]);
}

#[test]
fn list_order_physical_grid_requests_capture_before_loading_folder_zip_pdf_and_epub() {
    for extension in [None, Some("zip"), Some("pdf"), Some("epub")] {
        let mut app = setup_app();
        let folder = app.tmp.path().join("physical-request");
        let target = folder.join(extension.map_or("book".into(), |ext| format!("book.{ext}")));
        let row = match extension {
            None => GridItem::Folder(target.clone()),
            Some("zip") => GridItem::ZipFile(target.clone()),
            _ => GridItem::PdfFile(target.clone()),
        };
        install_rows(&mut app, &folder, vec![row]);
        let rows = app.items.clone();
        let _request = app.grid_physical_navigation(0, target.clone(), false);
        assert_eq!(captured_paths(&app), vec![target]);
        assert_eq!(app.items, rows);
        assert_eq!(app.current_folder.as_ref(), Some(&folder));
    }
}

#[test]
fn list_order_enter_and_explicit_archive_requests_capture_even_if_open_cannot_finish() {
    for explicit in [false, true] {
        let mut app = setup_app();
        let folder = app.tmp.path().join("archive-request");
        let archive = folder.join("missing.7z");
        install_rows(
            &mut app,
            &folder,
            vec![GridItem::ConvertibleArchive {
                path: archive.clone(),
                format: crate::archive_converter::ArchiveFormat::SevenZ,
            }],
        );
        if explicit {
            let _ = app.open_grid_container_with_mode(
                &egui::Context::default(),
                0,
                GridContainerOpenMode::PageList,
                "list-order-test",
            );
        } else {
            let _ = grid_enter(&mut app);
        }
        assert_eq!(captured_paths(&app), vec![archive]);
    }
}

#[test]
fn list_order_capture_excluded_surfaces_preserve_the_previous_snapshot() {
    let mut app = setup_app();
    let folder = app.tmp.path().join("excluded");
    let snapshot = capture_fixture(&mut app, &folder);
    let surfaces = [
        TopLevelGridSurface::DriveList,
        TopLevelGridSurface::Search(TopLevelSearchView::Global),
        TopLevelGridSurface::Search(TopLevelSearchView::Favorite),
        TopLevelGridSurface::Search(TopLevelSearchView::Tag),
        TopLevelGridSurface::Snapshot,
        TopLevelGridSurface::SubfolderExpansion,
        TopLevelGridSurface::ReadingHistory(ReadingHistoryRootRestore::unavailable()),
        TopLevelGridSurface::Bookmarks,
        TopLevelGridSurface::Rating { stars: 5 },
        TopLevelGridSurface::SmartFolder(top_level_grid_view::SmartFolderViewState::root(
            uuid::Uuid::new_v4(),
            vec![folder.clone()],
        )),
        TopLevelGridSurface::Collection(top_level_grid_view::CollectionGridIdentity {
            collection_id: crate::collection_store::CollectionId::new(),
        }),
    ];
    for surface in surfaces {
        app.top_level_grid_view.replace_surface(surface.clone());
        app.capture_main_folder_list_order(1);
        assert!(
            Arc::ptr_eq(app.list_order.as_ref().unwrap(), &snapshot),
            "{surface:?}"
        );
        assert!(app.list_order_position().is_none(), "{surface:?}");
    }
}

#[test]
fn list_order_snapshot_pin_disables_capture_and_position_without_discarding_order() {
    let mut app = setup_app();
    let folder = app.tmp.path().join("pinned");
    let snapshot = capture_fixture(&mut app, &folder);
    app.current_folder = Some(folder.join("book-a"));
    assert!(app.list_order_position().is_some());
    app.activate_snapshot(crate::snapshot::SnapshotSourceLabel::Mixed);
    assert!(app.is_snapshot_active());
    app.top_level_grid_view
        .replace_surface(TopLevelGridSurface::Folder);
    app.capture_main_folder_list_order(0);
    assert!(Arc::ptr_eq(app.list_order.as_ref().unwrap(), &snapshot));
    assert!(app.list_order_position().is_none());
    app.snapshot = None;
    assert_eq!(app.list_order_position().unwrap().1, 0);
}

#[test]
fn list_order_position_uses_current_logical_book_and_folder_component_boundaries() {
    let mut app = setup_app();
    let folder = app.tmp.path().join("position");
    let snapshot = capture_fixture(&mut app, &folder);
    for presentation in [
        ViewerPresentation::MainWindow,
        ViewerPresentation::Fullscreen,
    ] {
        app.viewer_presentation = presentation;
        for (path, expected) in [
            (folder.join("book-a"), Some(0)),
            (folder.join("book-b.zip"), Some(1)),
            (folder.join("book-a/inside.zip"), Some(0)),
            (folder.join("book-a-extra/inside.zip"), None),
            (folder.join("book-b.zip/inside"), None),
            (folder.join("elsewhere"), None),
        ] {
            app.current_folder = Some(path);
            assert_eq!(app.list_order_position().map(|(_, index)| index), expected);
            assert!(Arc::ptr_eq(app.list_order.as_ref().unwrap(), &snapshot));
        }
    }
    app.current_folder = Some(folder.join("book-a"));
    app.viewer_presentation = ViewerPresentation::DetachedWindow;
    assert!(app.list_order_position().is_none());
    app.viewer_presentation = ViewerPresentation::Fullscreen;
    app.navigation_scope = ViewerNavigationScope::DetachedPhysical;
    assert!(app.list_order_position().is_none());
}

#[test]
fn list_order_position_convertible_cache_uses_source_and_saved_pdf_is_a_new_location() {
    let mut app = setup_app();
    let folder = app.tmp.path().join("logical");
    let archive = folder.join("book.7z");
    let epub = folder.join("book.epub");
    install_rows(
        &mut app,
        &folder,
        vec![
            GridItem::ConvertibleArchive {
                path: archive.clone(),
                format: crate::archive_converter::ArchiveFormat::SevenZ,
            },
            GridItem::PdfFile(epub.clone()),
        ],
    );
    app.capture_main_folder_list_order(0);
    let snapshot = app.list_order.as_ref().unwrap().clone();
    app.current_folder = Some(folder.join("cache/converted.zip"));
    app.archive_source_override = Some(archive);
    assert_eq!(app.list_order_position().unwrap().1, 0);
    app.archive_source_override = None;
    app.current_folder = Some(epub.clone());
    app.display_epub_source = Some(crate::pdf_loader::LeasedEpubPath::try_new(epub).unwrap());
    assert!(app.display_epub_source.as_ref().unwrap().has_epub_lease());
    assert_eq!(app.list_order_position().unwrap().1, 1);
    app.current_folder = Some(folder.join("saved.pdf"));
    app.display_epub_source = None;
    assert!(app.list_order_position().is_none());
    assert!(Arc::ptr_eq(app.list_order.as_ref().unwrap(), &snapshot));
}

#[test]
fn list_order_cold_and_warm_pdf_adoption_derive_position_without_replacing_snapshot() {
    for warm in [false, true] {
        let mut app = setup_app();
        let folder = app.tmp.path().join("pdf-adoption");
        std::fs::create_dir(&folder).unwrap();
        let pdf = folder.join("book.pdf");
        std::fs::write(&pdf, b"%PDF-1.4\n").unwrap();
        if warm {
            let metadata = std::fs::metadata(&pdf).unwrap();
            app.get_or_open_catalog(&folder)
                .unwrap()
                .set_pdf_meta(
                    "book.pdf",
                    crate::ui_helpers::mtime_secs(&metadata),
                    metadata.len() as i64,
                    2,
                    false,
                )
                .unwrap();
        }
        install_rows(&mut app, &folder, vec![GridItem::PdfFile(pdf.clone())]);
        let _request = app.grid_physical_navigation(0, pdf.clone(), false);
        let snapshot = app.list_order.as_ref().unwrap().clone();
        let _ = app.load_pdf_as_folder_owned(
            pdf.clone(),
            OpenRequestOwner::Navigation,
            StartupListIntent::ExplicitList,
        );
        assert!(
            matches!(
                app.pdf_enumerate_pending.as_ref().map(|pending| &pending.5),
                Some(PdfOpenPhase::CommittedVerification { .. })
            ) == warm
        );
        if warm {
            assert_eq!(app.list_order_position().unwrap().1, 0);
        } else {
            assert!(app.list_order_position().is_none());
        }
        app.pdf_enumerate_pending.as_mut().unwrap().2 =
            crate::pdf_loader::completed_enumerate_result_handle(
                &pdf,
                Ok(vec![
                    crate::pdf_loader::PdfPageEntry {
                        page_num: 0,
                        mtime: 1,
                        file_size: 1,
                    },
                    crate::pdf_loader::PdfPageEntry {
                        page_num: 1,
                        mtime: 1,
                        file_size: 1,
                    },
                ]
                .into()),
            );
        app.poll_pdf_enumerate();
        app.finish_page_identity_preparation_for_test();
        assert_eq!(app.effective_folder().as_ref(), Some(&pdf));
        assert_eq!(app.list_order_position().unwrap().1, 0);
        assert!(Arc::ptr_eq(app.list_order.as_ref().unwrap(), &snapshot));
    }
}

#[test]
fn list_order_cached_epub_adoption_uses_logical_source_with_generation_lease() {
    use crate::epub_cache::{EpubCache, GenerationRow, WriteDenyingSource};
    let mut app = setup_app();
    let folder = app.tmp.path().join("epub-generation");
    std::fs::create_dir(&folder).unwrap();
    let epub = folder.join("book.epub");
    std::fs::write(&epub, b"source fixture").unwrap();
    // The publication/resolver/lease are real. PDF decoding is represented by the same
    // completed enumeration seam as the PDF adoption tests above.
    let mut cache = EpubCache::open_at(app.tmp.path()).unwrap();
    let reserved = cache.reserve_output(&epub).unwrap();
    let generated = reserved.final_path().to_path_buf();
    std::fs::create_dir_all(generated.parent().unwrap()).unwrap();
    std::fs::write(&generated, b"generated PDF fixture").unwrap();
    let row = GenerationRow {
        generation_id: reserved.generation_id(),
        src_path_key: crate::epub_cache::src_key(&epub),
        src_path: epub.clone(),
        src_state: crate::epub_cache::source_state(&std::fs::metadata(&epub).unwrap()),
        src_sha256: "list-order-full".into(),
        src_head_hash: "list-order-head".into(),
        pdf_file: generated.clone(),
        pdf_size: std::fs::metadata(&generated).unwrap().len(),
        page_count: 1,
        direction: "rtl".into(),
        profile: "reflow-v1".into(),
        created_at: 1,
        output_version: crate::epub_cache::CONVERTER_OUTPUT_VERSION,
    };
    let source_guard = WriteDenyingSource::open(&epub).unwrap();
    assert!(matches!(
        cache.publish(&row, &source_guard),
        Ok(crate::epub_cache::PublishOutcome::Published)
    ));
    let (_pin, target) =
        crate::pdf_loader::pin_cached_epub_for_test(&epub, app.tmp.path()).unwrap();
    assert_eq!(target.read_path.as_path(), generated);
    install_rows(&mut app, &folder, vec![GridItem::PdfFile(epub.clone())]);
    app.capture_main_folder_list_order(0);
    let snapshot = app.list_order.as_ref().unwrap().clone();
    let _ = app.load_pdf_as_folder_prepared(
        epub.clone(),
        crate::pdf_loader::PdfEnumerateResult {
            pages: vec![crate::pdf_loader::PdfPageEntry {
                page_num: 0,
                mtime: 1,
                file_size: 1,
            }],
            direction: target.epub_direction,
            stamp: Some(target.stamp.clone()),
        },
        OpenRequestOwner::Navigation,
        StartupListIntent::ExplicitList,
    );
    app.finish_page_identity_preparation_for_test();
    assert_eq!(app.effective_folder().as_ref(), Some(&epub));
    assert!(app.display_epub_source.as_ref().unwrap().has_epub_lease());
    assert_eq!(app.list_order_position().unwrap().1, 0);
    assert!(Arc::ptr_eq(app.list_order.as_ref().unwrap(), &snapshot));
}

#[test]
fn list_order_cancelled_pdf_then_existing_folder_open_still_resolves_frozen_order() {
    let mut app = setup_app();
    let folder = app.tmp.path().join("cancelled-pdf");
    std::fs::create_dir(&folder).unwrap();
    let book = folder.join("book-a");
    std::fs::create_dir(&book).unwrap();
    let pdf = folder.join("locked.pdf");
    std::fs::write(&pdf, b"%PDF-1.4\n").unwrap();
    install_rows(
        &mut app,
        &folder,
        vec![
            GridItem::Folder(book.clone()),
            GridItem::PdfFile(pdf.clone()),
        ],
    );
    app.capture_main_folder_list_order(1);
    let snapshot = app.list_order.as_ref().unwrap().clone();
    let _ = app.load_pdf_as_folder_owned(
        pdf.clone(),
        OpenRequestOwner::Navigation,
        StartupListIntent::ExplicitList,
    );
    app.pdf_enumerate_pending.as_mut().unwrap().2 =
        crate::pdf_loader::completed_enumerate_result_handle(
            &pdf,
            Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                crate::pdf_loader::PdfReadError::PasswordRequired,
            )),
        );
    app.poll_pdf_enumerate();
    assert!(app.cancel_pdf_password_dialog_request());
    app.load_folder(book);
    app.finish_page_identity_preparation_for_test();
    assert_eq!(app.list_order_position().unwrap().1, 0);
    assert!(Arc::ptr_eq(app.list_order.as_ref().unwrap(), &snapshot));
}

#[test]
fn list_order_rating_physical_owner_preserves_capture_but_can_use_existing_position() {
    let mut app = setup_app();
    let folder = app.tmp.path().join("rating-child");
    let snapshot = capture_fixture(&mut app, &folder);
    let book = folder.join("book-a");
    app.active_quick_folder_slot = None;
    app.current_folder = Some(book.clone());
    app.rating_view_stars = 4;
    app.rating_view_nav_stack = vec![book];
    let next = folder.join("book-b.zip");
    assert!(app.rating_view_physical_load_owner(&next).is_some());
    app.capture_main_folder_list_order(1);
    assert!(Arc::ptr_eq(app.list_order.as_ref().unwrap(), &snapshot));
    assert_eq!(app.list_order_position().unwrap().1, 0);
}

#[cfg(windows)]
#[test]
fn list_order_capture_non_main_context_leaves_both_snapshots_unchanged() {
    let mut app = setup_app();
    let folder = app.tmp.path().join("main");
    let snapshot = capture_fixture(&mut app, &folder);
    let child = app.build_window_context_for_test(91446, |owner| {
        install_rows(owner, &folder, vec![GridItem::Folder(folder.join("child"))]);
        owner.capture_main_folder_list_order(0);
        assert!(owner.list_order.is_none());
    });
    app.with_viewer_context(child, |owner| {
        owner.list_order = Some(snapshot.clone());
        owner.capture_main_folder_list_order(0);
        assert!(Arc::ptr_eq(owner.list_order.as_ref().unwrap(), &snapshot));
        assert!(owner.list_order_position().is_none());
    })
    .unwrap();
    assert!(Arc::ptr_eq(app.list_order.as_ref().unwrap(), &snapshot));
}
