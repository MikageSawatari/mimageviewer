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
