//! Investigation only: real ZIP/DFS workers, no native application or OS input.
use super::*;
use std::io::Write;

fn write_zip(path: &Path, prefixes: &[&str], pages: usize) {
    let mut png = std::io::Cursor::new(Vec::new());
    image::DynamicImage::new_rgb8(8, 12)
        .write_to(&mut png, image::ImageFormat::Png)
        .unwrap();
    let mut writer = zip::ZipWriter::new(std::fs::File::create(path).unwrap());
    let options: zip::write::FileOptions<'_, ()> =
        zip::write::FileOptions::default().compression_method(zip::CompressionMethod::Stored);
    for prefix in prefixes {
        for page in 1..=pages {
            writer
                .start_file(format!("{prefix}{page:03}.png"), options)
                .unwrap();
            writer.write_all(png.get_ref()).unwrap();
        }
    }
    writer.finish().unwrap();
}

fn fixture(env: &mut AppTestEnvForTest) -> (PathBuf, PathBuf) {
    env.indexer_init = IndexerInit::Unavailable;
    env.settings.book_open_resume = crate::settings::ResumeMode::FromStart;
    env.settings.book_nav_resume = crate::settings::ResumeMode::FromStart;
    let flat = env.tmp.path().join("01_flat.zip");
    let nested = env.tmp.path().join("02_inner_folders.zip");
    write_zip(&flat, &[""], 16);
    write_zip(&nested, &["inner_A/", "inner_B/"], 10);
    (flat, nested)
}

fn settle(app: &mut App, ctx: &egui::Context) {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        app.poll_open_path_classification(ctx);
        app.poll_collection_history_transition(ctx);
        app.poll_zip_enumerate();
        app.finish_page_identity_preparation_for_test();
        if app.zip_enumerate_pending.is_none()
            && app.top_level_grid_view.open_path_classification().is_none()
            && app
                .top_level_grid_view
                .history_navigation_transition()
                .is_none()
            && !app.sidecar_restore_active()
        {
            break;
        }
        assert!(Instant::now() < deadline, "ZIP open did not settle");
        std::thread::yield_now();
    }
}

fn navigate(app: &mut App, ctx: &egui::Context, expected: &Path) {
    let index = app.fullscreen_idx.expect("initial page must be open");
    app.handle_fullscreen_ctrl_nav_context(ctx, index, true, false);
    let deadline = Instant::now() + Duration::from_secs(30);
    let result = loop {
        if let Some(result) = app.poll_folder_nav() {
            break result;
        }
        assert!(Instant::now() < deadline, "DFS worker did not finish");
        std::thread::yield_now();
    };
    assert_eq!(result.target.as_ref().unwrap().logical_source(), expected);
    assert!(
        result.hit_image_folder,
        "the whole ZIP qualifies as image-bearing"
    );
    app.apply_folder_nav_result(ctx, result);
    settle(app, ctx);
}

fn assert_branch_list(app: &App, expected: &Path) {
    assert_eq!(app.current_folder.as_deref(), Some(expected));
    assert_eq!(app.items.len(), 2);
    assert!(
        app.items
            .iter()
            .all(|item| matches!(item, GridItem::ZipDir { .. }))
    );
    assert!(app.zip_nav.as_ref().unwrap().current().is_empty());
    assert!(app.fullscreen_idx.is_none());
    assert!(app.fs_nav_after_pdf_enumerate.is_none());
    assert!(!app.fs_nav_deferred_reopen_wait_active());
}

#[test]
fn section1678_main_and_linked_ctrl_down_end_in_branch_list() {
    for linked in [false, true] {
        let mut env = setup_app_for_test();
        let ctx = egui::Context::default();
        let (flat, nested) = fixture(&mut env);
        env.settings.detached_viewer_open_images_in_window = false;
        env.settings.detached_viewer_enabled = linked;
        env.pending_auto_fs_open = true;
        env.load_folder(flat);
        settle(&mut env, &ctx);
        if linked {
            env.viewer_presentation = ViewerPresentation::DetachedWindow;
            env.fs_viewport_presentation = Some(ViewerPresentation::DetachedWindow);
            env.fs_viewport_shown = true;
        }
        assert!(env.main_folder_history_available());
        navigate(&mut env, &ctx, &nested);
        assert_branch_list(&env, &nested);
        assert_eq!(env.projected_viewer_context_id(), env.viewer_context_main());
        assert!(!env.navigation_scope.is_detached_physical());
    }
}

#[test]
fn section1678_direct_main_open_keeps_branch_list_but_wrapper_opens_page() {
    for prefixes in [vec!["inner_A/", "inner_B/"], vec!["wrapper/"]] {
        let mut env = setup_app_for_test();
        let ctx = egui::Context::default();
        env.indexer_init = IndexerInit::Unavailable;
        let book = env.tmp.path().join("direct.zip");
        write_zip(&book, &prefixes, 10);
        env.pending_auto_fs_open = true;
        env.load_folder(book.clone());
        settle(&mut env, &ctx);
        if prefixes.len() == 2 {
            assert_branch_list(&env, &book);
        } else {
            assert!(env.fullscreen_idx.is_some());
            assert_eq!(env.zip_nav.as_ref().unwrap().current(), &["wrapper"]);
        }
    }
}

#[test]
fn section1678_direct_independent_branch_zip_declares_initial_terminal() {
    for prefixes in [vec!["inner_A/", "inner_B/"], vec!["wrapper/"]] {
        let mut env = setup_app_for_test();
        let ctx = egui::Context::default();
        env.indexer_init = IndexerInit::Unavailable;
        env.settings.detached_viewer_open_images_in_window = true;
        let book = env.tmp.path().join("direct.zip");
        write_zip(&book, &prefixes, 10);
        env.current_folder = Some(env.tmp.path().to_path_buf());
        env.items = vec![GridItem::ZipFile(book.clone())];
        env.visible_indices = vec![0];
        assert!(env.open_grid_container_in_detached_book_context(&ctx, 0));
        env.with_active_viewer_context(|active| {
            settle(active, &ctx);
            if prefixes.len() == 2 {
                assert_branch_list(active, &book);
                assert!(active.active_detached_window_is_closing());
            } else {
                assert!(active.fullscreen_idx.is_some());
                assert!(!active.active_detached_window_is_closing());
            }
        })
        .unwrap();
    }
}

fn independent_navigation_closes(sidecar_enabled: bool) -> bool {
    let mut env = setup_app_for_test();
    let ctx = egui::Context::default();
    crate::ui_fullscreen::install_fs_navigator_input_tracking(&ctx);
    let (flat, nested) = fixture(&mut env);
    env.settings.sidecar_backup_enabled = sidecar_enabled;
    env.settings.detached_viewer_open_images_in_window = true;
    install_reading_history_surface_for_test(&mut env);
    env.items = vec![GridItem::ZipFile(flat.clone())];
    env.visible_indices = vec![0];
    env.selected = Some(0);
    env.grid_scroll.set_position(123.0);
    assert!(env.start_active_detached_book_context(
        ViewerContextDescriptor::Zip {
            path: flat.clone(),
            entry_name: None,
            archive_source_override: None,
        },
        &ctx,
        None,
        None,
    ));
    env.with_active_viewer_context(|active| settle(active, &ctx))
        .unwrap();
    // A real headless content pass makes this an established Ready session, not
    // a synthetic Opening/Preparing gap (which has a different terminal policy).
    ctx.run_pass(egui::RawInput::default(), |ctx| {
        ctx.root_ui(|ui| env.update_active_viewer_context(ui, None));
    })
    .drop_without_applying_deltas();
    assert_eq!(
        env.active_detached_session.unwrap().content_phase,
        DetachedSessionContentPhase::Ready
    );
    env.with_active_viewer_context(|active| {
        // Page turns do not affect the ZIP topology; start from the reported third page.
        active.fullscreen_idx = Some(2);
        navigate(active, &ctx, &nested);
        assert_branch_list(active, &nested);
        assert!(!active.active_detached_transition_outstanding());
        assert!(active.fs_viewport_shown);
        ctx.run_pass(egui::RawInput::default(), |ctx| {
            active.keep_fullscreen_viewport_alive(ctx);
        })
        .drop_without_applying_deltas();
    })
    .unwrap();
    assert!(env.items_are_reading_history_view);
    assert!(matches!(&env.items[..], [GridItem::ZipFile(path)] if path == &flat));
    assert_eq!(env.selected, Some(0));
    assert_eq!(env.grid_scroll.offset(), 123.0);
    env.active_detached_session.is_none()
}

#[test]
fn section1678_observed_independent_ctrl_down_reaches_cleanup() {
    for sidecar_enabled in [false, true] {
        assert!(
            independent_navigation_closes(sidecar_enabled),
            "current bug characterization: sidecar={sidecar_enabled}"
        );
    }
}

#[test]
#[ignore = "§1.678 known defect: image-bearing branch ZIP terminates an established reader"]
fn section1678_regression_independent_ctrl_down_must_keep_usable_viewer() {
    assert!(
        !independent_navigation_closes(false),
        "Ctrl+Down to a ZIP containing twenty pages silently closed the reader"
    );
}
