//! §1.721: a return/reload preserves the list's explicit rating filter.
use super::*;

const FOUR: [bool; 6] = [false, false, false, false, true, false];
const THREE: [bool; 6] = [false, false, false, true, false, false];
const FOUR_FIVE: [bool; 6] = [false, false, false, false, true, true];

fn rate(app: &App, path: &Path, stars: u8) {
    app.rating_db
        .as_ref()
        .unwrap()
        .set(&crate::adjustment_db::normalize_path(path), stars)
        .unwrap();
}

fn configure(app: &mut App) {
    app.active_quick_folder_slot = None;
    app.settings.auto_fullscreen_zip_pdf = true;
    app.settings.auto_fullscreen_image_folders = true;
    app.settings.detached_viewer_open_images_in_window = false;
    app.settings.detached_viewer_enabled = false;
    app.settings.sort_order = crate::settings::SortOrder::FileName;
}

fn round_trip(book: Book, parent_stars: u8, filter: [bool; 6], suppressed: bool) {
    let mut app = setup_app_for_test();
    configure(&mut app);
    let ctx = egui::Context::default();
    let root = app.tmp.path().join("shelf");
    std::fs::create_dir(&root).unwrap();
    let (row, _) = fixture(&mut app, &root, book);
    let source = row.drag_source_path().unwrap().to_path_buf();
    let book_stars = filter
        .iter()
        .enumerate()
        .skip(1)
        .find(|(_, on)| **on)
        .unwrap()
        .0;
    rate(&app, &source, book_stars as u8);
    rate(&app, &root, parent_stars);
    let unrated = root.join("unrated.png");
    image::RgbImage::new(2, 2).save(&unrated).unwrap();
    app.settings.rating_filter = filter;
    app.load_folder(root.clone());
    settle(&mut app, &ctx, None);
    if !suppressed {
        // The badge (or editing the filter in this list) explicitly restores it.
        app.restore_rating_filter_suppression();
        app.rebuild_visible_indices();
    }
    let before_anchor = app.rating_filter_suppressed_at.clone();
    let before_visible = visible(&app);
    assert_eq!(before_anchor.is_some(), suppressed);
    if !suppressed {
        assert_eq!(before_visible, vec![row.clone()]);
    }
    let sibling_root = app.tmp.path().join("sibling");
    let sibling_item = GridItem::Image(sibling_root.join("kept.png"));
    let sibling = app.build_window_context_for_test(1721, |other| {
        other.current_folder = Some(sibling_root.clone());
        other.rating_filter_suppressed_at = Some((sibling_root.clone(), filter));
        other.items = vec![sibling_item.clone()];
    });
    let sibling_before = app
        .with_viewer_context(sibling, |other| {
            (
                other.current_folder.clone(),
                other.archive_source_override.clone(),
                other.rating_filter_suppressed_at.clone(),
                other.items.clone(),
                other.items_generation,
                other.pdf_render_epoch.snapshot(),
            )
        })
        .unwrap();

    admit(&mut app, &ctx, &row);
    assert_eq!(app.effective_folder(), Some(source.clone()));
    assert_eq!(
        app.rating_filter_suppressed_at
            .as_ref()
            .map(|(path, _)| path),
        Some(if suppressed { &root } else { &source })
    );
    assert_eq!(app.effective_rating_filter(), [true; 6]);
    assert_eq!(visible(&app), app.items);
    assert_eq!(app.items.len(), 1);
    assert_eq!(
        app.get_rating(0),
        0,
        "unrated page remains visible on entry"
    );
    app.handle_fullscreen_close_request();
    let nav = app.take_pending_return_to_parent_nav().unwrap();
    assert!(app.apply_fullscreen_close_nav_immediate(nav));
    settle(&mut app, &ctx, None);

    assert_eq!(app.effective_folder(), Some(root));
    assert_eq!(app.rating_filter_suppressed_at, before_anchor);
    assert_eq!(
        app.effective_rating_filter(),
        if suppressed { [true; 6] } else { filter }
    );
    assert_eq!(visible(&app), before_visible);
    assert_eq!(app.settings.rating_filter, filter);
    app.with_viewer_context(sibling, |other| {
        assert_eq!(other.current_folder, sibling_before.0);
        assert_eq!(other.archive_source_override, sibling_before.1);
        assert_eq!(other.rating_filter_suppressed_at, sibling_before.2);
        assert_eq!(other.items, sibling_before.3);
        assert_eq!(other.items_generation, sibling_before.4);
        assert!(sibling_before.5.is_current());
        assert_eq!(other.effective_rating_filter(), [true; 6]);
    })
    .unwrap();
}

macro_rules! books {
    ($name:ident, $book:ident) => {
        mod $name {
            use super::*;
            #[test]
            fn parent_four_filter_four() {
                round_trip(Book::$book, 4, FOUR, false);
            }
            #[test]
            fn parent_four_multiple_buckets() {
                round_trip(Book::$book, 4, FOUR_FIVE, false);
            }
            #[test]
            fn parent_four_excluded() {
                round_trip(Book::$book, 4, FIVE, false);
            }
            #[test]
            fn unrated_parent() {
                round_trip(Book::$book, 0, FOUR, false);
            }
            #[test]
            fn parent_three_filter_three() {
                round_trip(Book::$book, 3, THREE, false);
            }
            #[test]
            fn existing_parent_suppression_survives() {
                round_trip(Book::$book, 4, FOUR, true);
            }
        }
    };
}
books!(zip, Zip);
books!(pdf, Pdf);
books!(images, Folder);
books!(converted, Converted);

#[test]
fn unrated_child_parent_action_and_same_folder_reload_keep_explicit_filter() {
    let mut app = setup_app_for_test();
    configure(&mut app);
    let ctx = egui::Context::default();
    let root = app.tmp.path().join("starred");
    let child = root.join("unrated-child");
    std::fs::create_dir_all(child.join("ordinary-subfolder")).unwrap();
    let rated = root.join("rated.png");
    image::RgbImage::new(2, 2).save(&rated).unwrap();
    rate(&app, &root, 4);
    rate(&app, &rated, 4);
    app.settings.rating_filter = FOUR;
    app.load_folder(root.clone());
    settle(&mut app, &ctx, None);
    assert!(app.restore_rating_filter_suppression());
    app.rebuild_visible_indices();
    let before = visible(&app);
    assert_eq!(before, vec![GridItem::Image(rated)]);

    app.load_folder(child.clone());
    settle(&mut app, &ctx, None);
    assert_eq!(app.effective_folder(), Some(child));
    assert!(app.rating_filter_suppressed_at.is_none());
    // The same parent-action owner used by grid Backspace.
    let nav = app.handle_grid_parent_folder_action().unwrap();
    assert!(app.apply_fullscreen_close_nav_immediate(nav));
    settle(&mut app, &ctx, None);
    assert_eq!(app.effective_folder(), Some(root.clone()));
    assert!(app.rating_filter_suppressed_at.is_none());
    assert_eq!(app.effective_rating_filter(), FOUR);
    assert_eq!(visible(&app), before);

    app.load_folder(root);
    settle(&mut app, &ctx, None);
    assert!(app.rating_filter_suppressed_at.is_none());
    assert_eq!(app.effective_rating_filter(), FOUR);
    assert_eq!(visible(&app), before);
    assert_eq!(app.settings.rating_filter, FOUR);
}

#[test]
fn same_starred_folder_reload_does_not_suppress_after_badge_restore() {
    let mut app = setup_app_for_test();
    configure(&mut app);
    let ctx = egui::Context::default();
    let root = app.tmp.path().join("reload");
    std::fs::create_dir(&root).unwrap();
    for (name, stars) in [("rated.png", 4), ("unrated.png", 0)] {
        let path = root.join(name);
        image::RgbImage::new(2, 2).save(&path).unwrap();
        rate(&app, &path, stars);
    }
    rate(&app, &root, 4);
    app.settings.rating_filter = FOUR;
    app.load_folder(root.clone());
    settle(&mut app, &ctx, None);
    assert!(app.restore_rating_filter_suppression());
    app.rebuild_visible_indices();
    let before = visible(&app);
    assert_eq!(before, vec![GridItem::Image(root.join("rated.png"))]);
    app.load_folder(root);
    settle(&mut app, &ctx, None);
    assert!(app.rating_filter_suppressed_at.is_none());
    assert_eq!(app.effective_rating_filter(), FOUR);
    assert_eq!(app.settings.rating_filter, FOUR);
    assert_eq!(visible(&app), before);
}

#[test]
fn new_descendant_sibling_unrelated_and_startup_entries_still_suppress() {
    let mut app = setup_app_for_test();
    configure(&mut app);
    let ctx = egui::Context::default();
    let root = app.tmp.path().join("tree");
    let first = root.join("first");
    let sibling = root.join("second");
    let descendant = sibling.join("child");
    let unrelated = app.tmp.path().join("elsewhere");
    for path in [&first, &sibling, &descendant, &unrelated] {
        std::fs::create_dir_all(path).unwrap();
        image::RgbImage::new(2, 2)
            .save(path.join("unrated.png"))
            .unwrap();
        rate(&app, path, 4);
    }
    app.settings.rating_filter = FOUR;
    app.current_folder = None;
    app.archive_source_override = None;
    // Ctrl+Down/PageDown's adopted sibling and load_folder's descendant share
    // this boundary; clear the prior badge to test each new entry independently.
    for path in [first, sibling, descendant, unrelated] {
        app.load_folder(path.clone());
        settle(&mut app, &ctx, None);
        assert_eq!(
            app.rating_filter_suppressed_at.as_ref().map(|(p, _)| p),
            Some(&path)
        );
        assert_eq!(app.effective_rating_filter(), [true; 6]);
        assert_eq!(visible(&app), app.items);
        assert!(app.restore_rating_filter_suppression());
    }
    assert_eq!(app.settings.rating_filter, FOUR);
}

#[test]
fn synthetic_search_and_smart_origins_allow_new_physical_entry() {
    let mut app = setup_app_for_test();
    configure(&mut app);
    let ctx = egui::Context::default();
    let root = app.tmp.path().join("physical");
    std::fs::create_dir(&root).unwrap();
    rate(&app, &root, 4);
    app.settings.rating_filter = FOUR;
    for origin in [
        search_results_synthetic_path(),
        smart_folder::smart_folder_synthetic_path(uuid::Uuid::nil()),
    ] {
        assert!(!path_in_subtree_ci(&origin, &root));
        app.current_folder = Some(origin);
        app.load_folder(root.clone());
        settle(&mut app, &ctx, None);
        assert_eq!(
            app.rating_filter_suppressed_at.as_ref().map(|(p, _)| p),
            Some(&root)
        );
        assert!(app.restore_rating_filter_suppression());
    }
}

fn write_reload_zip(backing: &Path, remove_a: bool) {
    use std::io::Write as _;
    let mut png = std::io::Cursor::new(Vec::new());
    image::RgbImage::new(2, 2)
        .write_to(&mut png, image::ImageFormat::Png)
        .unwrap();
    let mut zip = ::zip::ZipWriter::new(std::fs::File::create(backing).unwrap());
    for name in if remove_a {
        ["unrated-root.png", "B/001.png"]
    } else {
        ["A/001.png", "B/001.png"]
    } {
        zip.start_file(name, ::zip::write::SimpleFileOptions::default())
            .unwrap();
        zip.write_all(png.get_ref()).unwrap();
    }
    zip.finish().unwrap();
}

fn zip_book_reload(book: Book, outer_starred: bool, remove_a: bool) {
    let mut app = setup_app_for_test();
    configure(&mut app);
    app.settings.auto_fullscreen_zip_pdf = false;
    let ctx = egui::Context::default();
    let root = app.tmp.path().join("zip-reload");
    std::fs::create_dir(&root).unwrap();
    let (row, backing) = fixture(&mut app, &root, book);
    let source = row.drag_source_path().unwrap().to_path_buf();
    write_reload_zip(&backing, false);
    if matches!(book, Book::Converted) {
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
                2,
                false,
            )
            .unwrap();
    }
    rate(&app, &source, if outer_starred { 4 } else { 0 });
    let anchor = source.join("A");
    rate(&app, &anchor, 4);
    app.settings.rating_filter = FOUR;
    app.load_folder(root);
    let outcome = app.load_folder_or_convert_archive_with_auto_fullscreen_owned(
        source.clone(),
        false,
        OpenRequestOwner::Navigation,
        StartupListIntent::ExplicitList,
    );
    assert!(matches!(
        outcome,
        FolderOpenOutcome::Loaded | FolderOpenOutcome::Classifying
    ));
    settle(&mut app, &ctx, None);
    // Explicit filter on the outer list, whether or not the outer ZIP is rated.
    app.restore_rating_filter_suppression();
    app.rebuild_visible_indices();
    let a = app
        .items
        .iter()
        .position(|item| matches!(item, GridItem::ZipDir { dir_prefix, .. } if dir_prefix == "A/"))
        .unwrap();
    assert_eq!(app.current_grid_order(), &[a]);
    app.selected = Some(a);
    grid_key(&mut app, egui::Key::Enter, egui::Modifiers::NONE);
    settle(&mut app, &ctx, None);
    assert_eq!(
        app.rating_filter_suppressed_at.as_ref().map(|(p, _)| p),
        Some(&anchor)
    );
    assert_eq!(visible(&app), app.items);
    assert_eq!(app.items.len(), 1);
    assert_eq!(app.get_rating(0), 0);

    if remove_a {
        write_reload_zip(&backing, true);
    }
    // The real F5 refresh owner captures the internal prefix and re-enumerates.
    app.reload_current_folder_preserving_override();
    settle(&mut app, &ctx, None);
    assert_eq!(app.effective_folder(), Some(source));
    assert_eq!(app.current_folder, Some(backing));
    assert_eq!(app.settings.rating_filter, FOUR);
    if remove_a {
        assert!(app.zip_nav.as_ref().unwrap().at_root());
        assert!(app.rating_filter_suppressed_at.is_none());
        assert_eq!(app.effective_rating_filter(), FOUR);
        assert_eq!(app.items.len(), 2, "the refreshed ancestor has real rows");
        assert!(
            visible(&app).is_empty(),
            "unrated ancestor rows must be filtered"
        );
    } else {
        assert_eq!(app.zip_nav.as_ref().unwrap().current(), &["A".to_owned()]);
        assert_eq!(
            app.rating_filter_suppressed_at.as_ref().map(|(p, _)| p),
            Some(&anchor)
        );
        assert_eq!(app.effective_rating_filter(), [true; 6]);
        assert_eq!(app.items.len(), 1);
        assert_eq!(visible(&app), app.items);
        assert_eq!(app.get_rating(0), 0);
    }
}

macro_rules! reload_cases {
    ($name:ident, $book:ident) => {
        mod $name {
            use super::*;
            #[test]
            fn rated_outer_f5_keeps_internal_anchor() {
                zip_book_reload(Book::$book, true, false);
            }
            #[test]
            fn unrated_outer_f5_keeps_internal_anchor() {
                zip_book_reload(Book::$book, false, false);
            }
            #[test]
            fn rated_outer_f5_missing_level_restores_filter() {
                zip_book_reload(Book::$book, true, true);
            }
            #[test]
            fn unrated_outer_f5_missing_level_restores_filter() {
                zip_book_reload(Book::$book, false, true);
            }
        }
    };
}
reload_cases!(zip_reload, Zip);
reload_cases!(converted_reload, Converted);

#[test]
fn synthetic_origins_enter_starred_physical_ancestor_of_data_dir() {
    let mut app = setup_app_for_test();
    configure(&mut app);
    let parent = crate::data_dir::get().parent().unwrap().to_path_buf();
    rate(&app, &parent, 4); // Only the disposable rating DB is written.
    app.settings.rating_filter = FOUR;
    for origin in [
        search_results_synthetic_path(),
        reading_history_synthetic_path(),
        bookmark_view_synthetic_path(),
        rating_view_synthetic_path(),
        subfolder_expansion_synthetic_path(),
        smart_folder::smart_folder_synthetic_path(uuid::Uuid::nil()),
        drive_list_synthetic_path(),
    ] {
        assert!(is_synthetic_view_path(&origin));
        assert!(path_in_subtree_ci(&origin, &parent));
        // Exercise the adoption boundary without enumerating the shared temp parent.
        app.current_folder = Some(parent.clone());
        app.maybe_restore_rating_filter_if_out_of_scope(Some(&origin), None);
        assert_eq!(
            app.rating_filter_suppressed_at.as_ref().map(|(p, _)| p),
            Some(&parent)
        );
        assert_eq!(app.effective_rating_filter(), [true; 6]);
        assert!(app.restore_rating_filter_suppression());
    }
    assert_eq!(app.settings.rating_filter, FOUR);
}

#[test]
fn drive_root_return_keeps_explicit_filter_and_visible_rows_without_drive_io() {
    let mut app = setup_app_for_test();
    configure(&mut app);
    let root = PathBuf::from("Z:/");
    let book = PathBuf::from(r"z:\book.zip");
    let row = GridItem::ZipFile(book.clone());
    rate(&app, &root, 4);
    rate(&app, &book, 4);
    app.settings.rating_filter = FOUR;
    app.items = vec![row.clone(), GridItem::Image(root.join("unrated.png"))];
    app.current_folder = Some(root);
    app.rating_filter_suppressed_at = Some((book.clone(), FOUR));
    app.maybe_restore_rating_filter_if_out_of_scope(Some(&book), None);
    app.rebuild_visible_indices();
    assert!(app.rating_filter_suppressed_at.is_none());
    assert_eq!(app.effective_rating_filter(), FOUR);
    assert_eq!(app.settings.rating_filter, FOUR);
    assert_eq!(visible(&app), vec![row]);
}

#[test]
fn non_zip_same_location_restores_stale_internal_anchor() {
    let mut app = setup_app_for_test();
    configure(&mut app);
    app.settings.rating_filter = FOUR;
    for path in ["Z:/folder", "Z:/folder.zip", "Z:/book.pdf"] {
        let current = PathBuf::from(path);
        app.current_folder = Some(current.clone());
        app.rating_filter_suppressed_at = Some((current.join("stale-child"), FOUR));
        app.maybe_restore_rating_filter_if_out_of_scope(Some(&current), None);
        assert!(app.rating_filter_suppressed_at.is_none(), "{path}");
        assert_eq!(app.effective_rating_filter(), FOUR);
    }
}

#[test]
fn subtree_helper_handles_roots_and_trailing_separators() {
    for (child, root) in [
        (r"C:\book.zip", r"c:\"),
        ("c:/book.zip", "C:/"),
        ("C:/", r"c:\"),
        ("/book.zip", "/"),
        ("/", "/"),
        ("C:/books", "c:/books/"),
        ("C:/books/", "c:/books"),
        ("C:/books/book.zip", "c:/books///"),
        (r"\\server\share\book.zip", "//SERVER/share/"),
    ] {
        assert!(
            path_in_subtree_ci(Path::new(child), Path::new(root)),
            "{child} under {root}"
        );
    }
}

#[test]
fn subtree_helper_preserves_component_and_drive_boundaries() {
    for (child, root) in [
        ("D:/book.zip", "C:/"),
        ("C:book.zip", "C:/"),
        ("C:", "C:/"),
        ("C:/books-extra/book.zip", "C:/books/"),
        ("C:/books", "C:/books/child/"),
        ("//server/share-extra/book.zip", "//server/share/"),
    ] {
        assert!(
            !path_in_subtree_ci(Path::new(child), Path::new(root)),
            "{child} outside {root}"
        );
    }
}
