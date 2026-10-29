//! Report >>649: use the outgoing filtered listing and real input/open/worker/adoption
//! paths. Never manufacture a ListOrderSnapshot or launch a product window.
use super::*;
use std::io::{Cursor, Write};
use std::time::{Duration, Instant, UNIX_EPOCH};

const FIVE_ONLY: [bool; 6] = [false, false, false, false, false, true];

#[derive(Clone, Copy, Debug)]
enum Entry {
    Enter,
    DoubleClick,
    Gamepad,
}

fn names(app: &App) -> Vec<String> {
    app.current_grid_order()
        .iter()
        .map(|&idx| app.items[idx].name().into_owned())
        .collect()
}

fn trace(app: &App, stage: &str) {
    eprintln!(
        "649 {stage}: source={:?} rows={:?} recorded={:?} settings={:?} effective={:?} anchor={:?} fullscreen={:?}",
        app.effective_folder().as_deref().and_then(Path::file_name),
        names(app),
        app.list_order.as_ref().map(|s| s
            .entries
            .iter()
            .map(|e| e.path.file_name().unwrap())
            .collect::<Vec<_>>()),
        app.settings.rating_filter,
        app.effective_rating_filter(),
        app.rating_filter_suppressed_at
            .as_ref()
            .map(|(p, _)| p.file_name().unwrap()),
        app.fullscreen_idx,
    );
}

fn apply_open(
    app: &mut App,
    ctx: &egui::Context,
    nav: crate::ui_main::AddressBarNav,
    entry: Entry,
) {
    // Use the normal winning main-frame dispatcher, in the originating input lane.
    let mut frame = update_phases::MainFrameNavigation {
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
    };
    match entry {
        Entry::Enter => frame.keyboard_nav = Some(nav),
        Entry::DoubleClick => frame.grid_nav = Some(nav),
        Entry::Gamepad => frame.gamepad_nav = Some(nav),
    }
    app.post_render_main_navigation(ctx, frame);
}

fn settle(app: &mut App, ctx: &egui::Context) {
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        app.settle_open_path_classification_for_test();
        app.poll_history_navigation_transition_for_test(ctx);
        app.poll_zip_enumerate();
        app.poll_sidecar_restore(ctx);
        app.poll_book_composition_service_for_test(ctx);
        app.poll_book_presentation_permission(ctx);
        if app.top_level_grid_view.open_path_classification().is_none()
            && app
                .top_level_grid_view
                .history_navigation_transition()
                .is_none()
            && app.zip_enumerate_pending.is_none()
            && app.fs_nav_after_pdf_enumerate.is_none()
            && !app.sidecar_restore_active()
        {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "649 open/adoption did not settle"
        );
        std::thread::sleep(Duration::from_millis(2));
    }
}

fn assert_ratings(app: &App, paths: &[PathBuf; 3]) {
    assert_eq!(app.settings.rating_filter, FIVE_ONLY);
    // Persist inside AppTestEnv's disposable profile, including while suppression is on.
    app.settings.save();
    assert_eq!(crate::settings::Settings::load().rating_filter, FIVE_ONLY);
    let db = app.rating_db.as_ref().unwrap();
    for (path, stars) in paths.iter().zip([5, 1, 5]) {
        assert_eq!(db.get(&crate::adjustment_db::normalize_path(path)), stars);
    }
}

fn assert_unrated_pages(app: &mut App, anchor: &Path, paths: &[PathBuf; 3]) {
    trace(app, "adopted book");
    assert_eq!(app.effective_folder().as_deref(), Some(anchor));
    assert_ratings(app, paths);
    assert_eq!(app.effective_rating_filter(), [true; 6]);
    assert_eq!(
        app.rating_filter_suppressed_at
            .as_ref()
            .map(|(p, _)| p.as_path()),
        Some(anchor)
    );
    let mut page_names = names(app);
    page_names.sort();
    assert_eq!(page_names, ["001.png", "002.png"]);
    assert!(
        app.items
            .iter()
            .all(|item| matches!(item, GridItem::Image(_) | GridItem::ZipImage { .. }))
    );
    for idx in 0..app.items.len() {
        assert_eq!(app.get_rating(idx), 0, "pages must remain unrated");
    }
}

fn move_book(app: &mut App, ctx: &egui::Context, forward: bool, expected: &Path) {
    let source = app.effective_folder().unwrap();
    let idx = app.fullscreen_idx.expect("opened page");
    // Exact key-handler entry requested by the investigation, not the worker helper.
    app.handle_fullscreen_list_order_nav_context_with_operation(ctx, idx, forward, false, None);
    assert!(
        app.folder_nav_pending.is_some(),
        "649 input must be accepted"
    );
    let deadline = Instant::now() + Duration::from_secs(15);
    let result = loop {
        if let Some(result) = app.poll_folder_nav() {
            break result;
        }
        assert!(Instant::now() < deadline, "649 worker did not return");
        std::thread::sleep(Duration::from_millis(2));
    };
    let target = result
        .target
        .as_ref()
        .unwrap()
        .logical_source()
        .to_path_buf();
    eprintln!(
        "649 worker: {:?} -> {:?}, forward={forward}",
        source.file_name(),
        target.file_name()
    );
    app.apply_folder_nav_result(ctx, result);
    settle(app, ctx);
    trace(app, "navigation adopted");
    assert_eq!(
        target, expected,
        "worker must stay in the recorded filtered list"
    );
    assert_eq!(app.effective_folder().as_deref(), Some(expected));
    assert!(app.fullscreen_idx.is_some());
    // Same headless seam as list_order_worker_tests: adoption is complete; this
    // investigation does not render GPU pixels or test presentation readiness.
    app.release_fs_nav_lock();
}

fn scenario(zip_books: bool, entry: Entry, date_desc: bool) {
    // Exercise direct page opening and the existing page-list-then-page setting.
    for auto_open in [true, false] {
        let mut app = setup_app();
        let ctx = egui::Context::default();
        app.active_quick_folder_slot = None;
        app.settings.auto_fullscreen_zip_pdf = auto_open;
        app.settings.auto_fullscreen_image_folders = auto_open;
        app.settings.detached_viewer_open_images_in_window = false;
        app.settings.grid_view_mode = crate::settings::GridViewMode::Thumbnail;
        app.settings.grid_click_selection_mode = crate::settings::GridClickSelectionMode::Explorer;
        app.settings.grid_open_selected_item_on_click = false;
        app.settings.sort_order = if date_desc {
            crate::settings::SortOrder::DateDesc
        } else {
            crate::settings::SortOrder::FileName
        };
        let root = app.tmp.path().join("ratingnav649");
        std::fs::create_dir(&root).unwrap();
        let mut png = Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
            2,
            2,
            image::Rgba([30, 60, 90, 255]),
        ))
        .write_to(&mut png, image::ImageFormat::Png)
        .unwrap();
        let paths = ["A", "B", "C"].map(|name| {
            root.join(if zip_books {
                format!("{name}.zip")
            } else {
                name.into()
            })
        });
        for (i, path) in paths.iter().enumerate() {
            if zip_books {
                let mut writer = zip::ZipWriter::new(std::fs::File::create(path).unwrap());
                for page in ["001.png", "002.png"] {
                    writer
                        .start_file(page, zip::write::SimpleFileOptions::default())
                        .unwrap();
                    writer.write_all(png.get_ref()).unwrap();
                }
                writer.finish().unwrap();
            } else {
                std::fs::create_dir(path).unwrap();
                for page in ["001.png", "002.png"] {
                    std::fs::write(path.join(page), png.get_ref()).unwrap();
                }
            }
            // C is newest: date-desc order is C,B,A, distinct from name order.
            #[cfg(windows)]
            let file = {
                use std::os::windows::fs::OpenOptionsExt;
                // FILE_WRITE_ATTRIBUTES + FILE_FLAG_BACKUP_SEMANTICS also open directories.
                std::fs::OpenOptions::new()
                    .access_mode(0x0100)
                    .custom_flags(0x02000000)
                    .open(path)
                    .unwrap()
            };
            #[cfg(not(windows))]
            let file = std::fs::File::open(path).unwrap();
            file.set_modified(UNIX_EPOCH + Duration::from_secs(100 + i as u64 * 100))
                .unwrap();
            app.rating_db
                .as_ref()
                .unwrap()
                .set(&crate::adjustment_db::normalize_path(path), [5, 1, 5][i])
                .unwrap();
        }
        app.settings.rating_filter = FIVE_ONLY;
        app.load_folder(root.clone());
        app.finish_page_identity_preparation_for_test();
        let expected = if date_desc {
            vec![paths[2].clone(), paths[0].clone()]
        } else {
            vec![paths[0].clone(), paths[2].clone()]
        };
        let first = &expected[0];
        let second = &expected[1];
        let expected_names = expected
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        trace(
            &app,
            &format!(
                "parent zip={zip_books} entry={entry:?} date_desc={date_desc} auto={auto_open}"
            ),
        );
        assert_eq!(
            names(&app),
            expected_names,
            "real listing must filter out B"
        );
        assert!(app.rating_filter_suppressed_at.is_none());
        assert!(
            app.list_order.is_none(),
            "capture must come from the normal input"
        );
        app.selected = app
            .items
            .iter()
            .position(|item| item.container_path() == Some(first.as_path()));
        let idx = app.selected.unwrap();
        let nav = match entry {
            Entry::Enter => grid_enter(&mut app),
            Entry::Gamepad => app.handle_gamepad_grid_accept(&ctx),
            Entry::DoubleClick => Some(crate::ui_main::ratingnav649_double_click_for_test(
                &mut app, idx,
            )),
        }
        .expect("normal input must return the container request");
        trace(&app, "open accepted before load");
        let snapshot = app
            .list_order
            .as_ref()
            .expect("normal open captures outgoing order")
            .clone();
        assert_eq!(captured_paths(&app), expected);
        apply_open(&mut app, &ctx, nav, entry);
        settle(&mut app, &ctx);
        assert_unrated_pages(&mut app, first, &paths);
        if !auto_open {
            assert!(
                app.fullscreen_idx.is_none(),
                "page-list setting is preserved"
            );
        }
        if app.fullscreen_idx.is_none() {
            // The separate auto-open regressions below retain the Enter/folder defect.
            // Continue the >>649 membership investigation through the visible page list.
            eprintln!("649 explicit page Enter: configured_auto_open={auto_open}");
            app.selected = Some(0);
            assert!(
                grid_enter(&mut app).is_none(),
                "Enter opens the page, not another book"
            );
        }
        assert!(app.fullscreen_idx.is_some(), "unrated page must open");
        app.release_fs_nav_lock();
        move_book(&mut app, &ctx, true, second);
        assert_unrated_pages(&mut app, second, &paths);
        assert!(Arc::ptr_eq(app.list_order.as_ref().unwrap(), &snapshot));
        move_book(&mut app, &ctx, false, first);
        assert_unrated_pages(&mut app, first, &paths);
        assert!(Arc::ptr_eq(app.list_order.as_ref().unwrap(), &snapshot));
        // Existing grid Backspace route, after leaving fullscreen for its page list.
        app.close_fullscreen();
        let nav = app
            .handle_grid_parent_folder_action()
            .expect("return to physical parent");
        apply_open(&mut app, &ctx, nav, Entry::Enter);
        settle(&mut app, &ctx);
        trace(&app, "returned parent");
        assert_eq!(app.effective_folder().as_deref(), Some(root.as_path()));
        assert_eq!(names(&app), expected_names);
        assert!(app.rating_filter_suppressed_at.is_none());
        assert_eq!(app.effective_rating_filter(), FIVE_ONLY);
        assert_ratings(&app, &paths);
        assert!(Arc::ptr_eq(app.list_order.as_ref().unwrap(), &snapshot));
    }
}

fn folder_enter_auto_open(filter: [bool; 6]) {
    let mut app = setup_app();
    let ctx = egui::Context::default();
    let root = app.tmp.path().join("ratingnav649-auto-open");
    let book = root.join("A");
    std::fs::create_dir_all(&book).unwrap();
    image::RgbImage::new(2, 2)
        .save(book.join("001.png"))
        .unwrap();
    app.active_quick_folder_slot = None;
    app.settings.detached_viewer_open_images_in_window = false;
    app.settings.auto_fullscreen_zip_pdf = true;
    app.settings.auto_fullscreen_image_folders = true;
    app.settings.rating_filter = filter;
    app.rating_db
        .as_ref()
        .unwrap()
        .set(&crate::adjustment_db::normalize_path(&book), 5)
        .unwrap();
    app.load_folder(root);
    app.finish_page_identity_preparation_for_test();
    app.selected = Some(0);
    assert!(app.should_auto_fullscreen_grid_container(0));
    let nav = grid_enter(&mut app).expect("normal folder Enter");
    eprintln!(
        "649 Enter auto-open admission: requested=true pending_auto_fs_open={}",
        app.pending_auto_fs_open
    );
    apply_open(&mut app, &ctx, nav, Entry::Enter);
    settle(&mut app, &ctx);
    trace(&app, "Enter auto-open control adopted");
    assert_eq!(app.effective_folder().as_deref(), Some(book.as_path()));
    assert_eq!(names(&app), ["001.png"]);
    assert!(
        app.fullscreen_idx.is_some(),
        "configured image-folder auto-open must reach fullscreen"
    );
}

#[test]
fn ratingnav649_folder_enter_auto_open_with_rating_filter() {
    folder_enter_auto_open(FIVE_ONLY);
}

#[test]
fn ratingnav649_folder_enter_auto_open_without_rating_filter() {
    folder_enter_auto_open([true; 6]);
}

macro_rules! case {
    ($name:ident, $zip:literal, $entry:ident, $date:literal) => {
        #[test]
        fn $name() {
            scenario($zip, Entry::$entry, $date);
        }
    };
}

case!(ratingnav649_zip_enter_name, true, Enter, false);
case!(ratingnav649_zip_enter_date_desc, true, Enter, true);
case!(ratingnav649_zip_double_click_name, true, DoubleClick, false);
case!(
    ratingnav649_zip_double_click_date_desc,
    true,
    DoubleClick,
    true
);
case!(ratingnav649_zip_gamepad_name, true, Gamepad, false);
case!(ratingnav649_zip_gamepad_date_desc, true, Gamepad, true);
case!(ratingnav649_folder_enter_name, false, Enter, false);
case!(ratingnav649_folder_enter_date_desc, false, Enter, true);
case!(
    ratingnav649_folder_double_click_name,
    false,
    DoubleClick,
    false
);
case!(
    ratingnav649_folder_double_click_date_desc,
    false,
    DoubleClick,
    true
);
case!(ratingnav649_folder_gamepad_name, false, Gamepad, false);
case!(ratingnav649_folder_gamepad_date_desc, false, Gamepad, true);
