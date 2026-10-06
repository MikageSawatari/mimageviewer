//! Handler-level coverage for §1.335. Fixtures use the isolated AppTestEnv profile;
//! no test launches a native application or borrows the user's settings.

use super::*;
use crate::app::StartupListIntent;
use crate::settings::{ListCursorHint, StartupListRestore, StartupListTarget};

fn png_bytes() -> Vec<u8> {
    let mut output = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
        2,
        2,
        image::Rgba([80, 140, 210, 255]),
    ))
    .write_to(&mut output, image::ImageFormat::Png)
    .unwrap();
    output.into_inner()
}

fn write_zip(path: &Path, entries: &[&str]) {
    let png = png_bytes();
    let mut zip = zip::ZipWriter::new(std::fs::File::create(path).unwrap());
    for entry in entries {
        zip.start_file(*entry, zip::write::SimpleFileOptions::default())
            .unwrap();
        std::io::Write::write_all(&mut zip, &png).unwrap();
    }
    zip.finish().unwrap();
}

fn restore_target(app: &App) -> &StartupListTarget {
    match app.settings.startup_list_restore.as_ref().unwrap() {
        StartupListRestore::V1 { target, .. } => target,
    }
}

fn restore_cursor(app: &App) -> Option<&ListCursorHint> {
    match app.settings.startup_list_restore.as_ref().unwrap() {
        StartupListRestore::V1 { cursor, .. } => cursor.as_ref(),
    }
}

fn assert_physical(app: &App, path: &Path, prefix: Option<&str>) {
    assert_eq!(
        restore_target(app),
        &StartupListTarget::PhysicalList {
            logical_path: path.to_path_buf(),
            zip_prefix: prefix.map(str::to_owned),
        }
    );
}

fn settle_book(app: &mut App) {
    phase_c_folder_nav_history_tests::finish_staged_physical_history_for_test(app);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while app.zip_enumerate_pending.is_some() {
        app.poll_zip_enumerate();
        assert!(
            std::time::Instant::now() < deadline,
            "ZIP enumeration did not settle"
        );
        std::thread::yield_now();
    }
}

fn open_zip(app: &mut App, path: &Path, direct: bool) {
    assert!(matches!(
        app.load_folder_or_convert_archive_with_auto_fullscreen(path.to_path_buf(), direct),
        FolderOpenOutcome::Loaded | FolderOpenOutcome::Classifying
    ));
    settle_book(app);
    assert_eq!(app.current_folder.as_deref(), Some(path));
    assert!(app.zip_nav.is_some(), "a real adopted ZIP list is required");
}

fn restart_previous(env: &mut phase_c_support::AppTestEnv) {
    env.on_exit_inner();
    restart_previous_from_saved_settings(env);
}

fn restart_previous_from_saved_settings(env: &mut phase_c_support::AppTestEnv) {
    // Keep AppTestEnv's data-dir guard alive across App::drop and replacement.
    let saved = crate::settings::Settings::load();
    env.app = App::new_from_settings(saved);
    env.open_default_startup_target();
    settle_book(env);
    let ctx = egui::Context::default();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while env.sidecar_restore_active() {
        env.poll_sidecar_restore(&ctx);
        env.poll_zip_enumerate();
        env.poll_fs_nav_lock(&ctx);
        assert!(
            std::time::Instant::now() < deadline,
            "startup sidecar restoration did not settle"
        );
        std::thread::yield_now();
    }
    settle_book(env);
    env.poll_fs_nav_lock(&ctx);
}

fn book_env() -> (phase_c_support::AppTestEnv, PathBuf, PathBuf) {
    let mut env = phase_c_support::setup_app();
    let parent = env.tmp.path().join("books");
    std::fs::create_dir_all(&parent).unwrap();
    let book = parent.join("book.zip");
    write_zip(&book, &["a.png", "b.png"]);
    env.settings.startup_folder_mode = crate::settings::StartupFolderMode::Previous;
    env.settings.auto_fullscreen_zip_pdf = true;
    env.settings.restore_last_cursor = true;
    env.load_folder(parent.clone());
    assert_physical(&env, &parent, None);
    (env, parent, book)
}

#[test]
fn section1335_explicit_book_list_and_direct_read_restart_choose_different_lists() {
    for explicit_first in [false, true] {
        let (mut env, parent, book) = book_env();
        if explicit_first {
            open_zip(&mut env, &book, false);
            assert!(env.fullscreen_idx.is_none());
            assert_physical(&env, &book, Some(""));
            env.open_fullscreen(1, HistoryTrigger::UserChosen);
        } else {
            open_zip(&mut env, &book, true);
            assert_physical(&env, &parent, None);
        }
        assert!(env.fullscreen_idx.is_some());
        let expected = if explicit_first { &book } else { &parent };
        assert_physical(&env, expected, explicit_first.then_some(""));
        restart_previous(&mut env);
        assert_eq!(env.current_folder.as_ref(), Some(expected));
        assert!(env.fullscreen_idx.is_none());
        assert_physical(&env, expected, explicit_first.then_some(""));
    }
}

#[test]
fn section1335_back_to_list_acceptance_captures_viewed_page_instead_of_grid_selection() {
    let (mut env, parent, book) = book_env();
    open_zip(&mut env, &book, true);
    assert_physical(&env, &parent, None);
    env.open_fullscreen(1, HistoryTrigger::UserChosen);
    // Selection remains the departing grid cell while the viewer has moved on.
    env.selected = Some(0);
    let viewed_name = env.items[1].name().to_string();
    env.close_fullscreen_to_page_list();
    assert!(env.fullscreen_idx.is_none());
    assert_physical(&env, &book, Some(""));
    assert_eq!(restore_cursor(&env).unwrap().name, viewed_name);
    restart_previous(&mut env);
    assert_physical(&env, &book, Some(""));
    let selected = env.selected.unwrap();
    assert_eq!(env.items[selected].name(), viewed_name);
}

#[test]
#[cfg(windows)]
fn section1335_transition_acceptance_persists_before_effects_and_effects_do_not_recommit() {
    let (mut env, _, book) = book_env();
    open_zip(&mut env, &book, true);
    env.open_fullscreen(1, HistoryTrigger::UserChosen);
    env.selected = Some(0);
    let viewed_name = env.items[1].name().to_string();
    begin_test_video_presentation_transition(&mut env, ViewerPresentation::DetachedWindow, false);
    assert!(env.video_presentation_transition.is_transitioning());
    env.spread_popup_open = true;
    env.close_fullscreen_to_page_list();
    assert!(
        env.spread_popup_open,
        "normal close teardown is still deferred"
    );
    assert_physical(&env, &book, Some(""));
    assert_eq!(restore_cursor(&env).unwrap().name, viewed_name);
    let accepted = env.settings.startup_list_restore.clone();
    assert_eq!(
        env.execute_video_presentation_transition_effects(&egui::Context::default()),
        crate::app::PresentationEffectsOutcome::ClosedFullscreen
    );
    assert!(!env.spread_popup_open);
    assert_eq!(env.settings.startup_list_restore, accepted);
}

#[test]
#[cfg(windows)]
fn section1335_quit_immediately_after_transition_acceptance_restores_accepted_book_list() {
    let (mut env, _, book) = book_env();
    open_zip(&mut env, &book, true);
    begin_test_video_presentation_transition(&mut env, ViewerPresentation::DetachedWindow, false);
    env.close_fullscreen_to_page_list();
    assert!(
        env.fullscreen_idx.is_some(),
        "terminal effects have not been drained yet"
    );
    assert_physical(&env, &book, Some(""));
    restart_previous(&mut env);
    assert_eq!(env.current_folder.as_ref(), Some(&book));
    assert!(env.fullscreen_idx.is_none());
    assert_physical(&env, &book, Some(""));
}

#[test]
fn section1335_nested_zip_previous_rebuilds_effective_prefix_and_back_stack() {
    let (mut env, _, book) = book_env();
    write_zip(
        &book,
        &["other/a.png", "shelf/book/only/a.png", "shelf/second/b.png"],
    );
    open_zip(&mut env, &book, false);
    env.zip_nav_enter("shelf/");
    assert_physical(&env, &book, Some("shelf/"));
    env.zip_nav_enter("shelf/book/");
    assert_eq!(
        env.zip_nav.as_ref().unwrap().current(),
        &["shelf", "book", "only"]
    );
    assert_physical(&env, &book, Some("shelf/book/only/"));
    env.open_fullscreen(0, HistoryTrigger::UserChosen);
    restart_previous(&mut env);
    assert_physical(&env, &book, Some("shelf/book/only/"));
    assert_eq!(
        env.zip_nav.as_ref().unwrap().current(),
        &["shelf", "book", "only"]
    );
    assert!(env.zip_nav_back());
    assert_eq!(env.zip_nav.as_ref().unwrap().current(), &["shelf"]);
    assert_physical(&env, &book, Some("shelf/"));
    let selected = env.selected.unwrap();
    assert!(
        matches!(&env.items[selected], GridItem::ZipDir { dir_prefix, .. } if dir_prefix == "shelf/book/")
    );
    assert!(env.zip_nav_back());
    assert_physical(&env, &book, Some(""));
}

#[test]
fn section1335_reading_zip_dfs_preserves_last_explicit_prefix() {
    let (mut env, _, book) = book_env();
    write_zip(&book, &["first/a.png", "second/b.png"]);
    open_zip(&mut env, &book, false);
    env.zip_nav_enter("first/");
    assert_physical(&env, &book, Some("first/"));
    env.open_fullscreen(0, HistoryTrigger::UserChosen);
    let before = env.settings.startup_list_restore.clone();
    assert!(env.zip_nav_dfs_fullscreen(None, 0, true));
    assert_eq!(env.zip_nav.as_ref().unwrap().current(), &["second"]);
    assert!(env.fullscreen_idx.is_some());
    assert_eq!(env.settings.startup_list_restore, before);
    env.close_fullscreen_to_page_list();
    assert_physical(&env, &book, Some("second/"));
}

#[test]
fn section1335_typed_dfs_false_auto_fullscreen_reopens_page_without_adopting_zip_list() {
    let (mut env, parent, book) = book_env();
    let before = env.settings.startup_list_restore.clone();
    assert!(env.start_physical_history_transition_with_dfs(
        PhysicalHistoryIntent::Navigation {
            replay: None,
            auto_fullscreen: false,
        },
        book.clone(),
        Some(PhysicalHistoryDfsContinuation {
            queued_steps: 0,
            mode: FolderNavMode::SlideshowNext,
            history_trigger: HistoryTrigger::UserChosen,
            restore_video_tile: false,
            resume_slideshow: true,
            fullscreen: true,
        }),
        StartupListIntent::PageContinuation,
    ));
    assert_eq!(env.settings.startup_list_restore, before);
    env.settle_open_path_classification_for_test();
    assert_eq!(env.settings.startup_list_restore, before);
    settle_book(&mut env);
    assert_eq!(env.current_folder.as_ref(), Some(&book));
    assert!(env.fullscreen_idx.is_some());
    assert!(env.slideshow_playing);
    assert_eq!(env.settings.startup_list_restore, before);
    assert_physical(&env, &parent, None);
}

#[test]
fn section1335_preserve_reload_and_explicit_same_path_open_have_distinct_intents() {
    let (mut env, parent, book) = book_env();
    open_zip(&mut env, &book, true);
    let before = env.settings.startup_list_restore.clone();
    env.reload_current_folder_preserving_override();
    settle_book(&mut env);
    assert_eq!(env.current_folder.as_ref(), Some(&book));
    assert_eq!(env.settings.startup_list_restore, before);
    assert_physical(&env, &parent, None);
    open_zip(&mut env, &book, false);
    assert_physical(&env, &book, Some(""));
}

#[test]
fn section1335_failed_explicit_zip_and_ignored_convertible_open_do_not_adopt_candidates() {
    let (mut env, parent, _) = book_env();
    let broken = parent.join("broken.zip");
    std::fs::write(&broken, b"invalid central directory").unwrap();
    let before = env.settings.startup_list_restore.clone();
    assert!(env.start_physical_history_transition(
        PhysicalHistoryIntent::Navigation {
            replay: None,
            auto_fullscreen: false,
        },
        broken,
        StartupListIntent::ExplicitList,
    ));
    assert_eq!(env.settings.startup_list_restore, before);
    env.settle_open_path_classification_for_test();
    assert_eq!(env.settings.startup_list_restore, before);
    settle_book(&mut env);
    assert_eq!(env.current_folder.as_ref(), Some(&parent));
    assert_eq!(env.settings.startup_list_restore, before);

    let ignored = parent.join("ignored.7z");
    std::fs::write(&ignored, b"archive source").unwrap();
    env.settings
        .set_archive_file_handling(crate::settings::ArchiveFileHandling::Ignore);
    let outcome = env.load_folder_or_convert_archive_with_auto_fullscreen(ignored, false);
    assert!(matches!(
        outcome,
        FolderOpenOutcome::Ignored | FolderOpenOutcome::Classifying
    ));
    settle_book(&mut env);
    assert!(env.archive_convert.is_none());
    assert_eq!(env.current_folder.as_ref(), Some(&parent));
    assert_eq!(env.settings.startup_list_restore, before);
}

#[test]
fn section1335_normal_close_parent_branch_never_accepts_intermediate_book_list() {
    let (mut env, parent, book) = book_env();
    open_zip(&mut env, &book, true);
    let before = env.settings.startup_list_restore.clone();
    env.handle_fullscreen_close_request();
    assert!(env.pending_return_to_parent);
    assert!(env.fullscreen_idx.is_some());
    assert_eq!(env.settings.startup_list_restore, before);
    let nav = env.take_pending_return_to_parent_nav().unwrap();
    assert!(env.apply_fullscreen_close_nav_immediate(nav));
    settle_book(&mut env);
    assert_eq!(env.current_folder.as_ref(), Some(&parent));
    assert_physical(&env, &parent, None);
}

#[test]
fn section1335_converted_archive_records_logical_source_and_keeps_it_on_restart() {
    let (mut env, parent, _) = book_env();
    let source = parent.join("converted.7z");
    let cached = env.tmp.path().join("cached.zip");
    std::fs::write(&source, b"converted source fixture").unwrap();
    write_zip(&cached, &["a.png"]);
    let metadata = std::fs::metadata(&source).unwrap();
    env.archive_cache_db
        .as_ref()
        .unwrap()
        .record(
            &source,
            crate::ui_helpers::mtime_secs(&metadata),
            metadata.len() as i64,
            ArchiveFormat::SevenZ,
            &cached,
            0,
            1,
            false,
        )
        .unwrap();
    assert!(matches!(
        env.load_folder_or_convert_archive_with_auto_fullscreen(source.clone(), true),
        FolderOpenOutcome::Loaded | FolderOpenOutcome::Classifying
    ));
    settle_book(&mut env);
    assert_eq!(env.current_folder.as_ref(), Some(&cached));
    assert_eq!(env.archive_source_override.as_ref(), Some(&source));
    assert_physical(&env, &parent, None);
    assert!(env.fullscreen_idx.is_some());
    env.close_fullscreen_to_page_list();
    assert_physical(&env, &source, Some(""));
    restart_previous(&mut env);
    assert_eq!(env.current_folder.as_ref(), Some(&cached));
    assert_eq!(env.archive_source_override.as_ref(), Some(&source));
    assert_physical(&env, &source, Some(""));
    assert!(env.fullscreen_idx.is_none());
}

#[test]
#[cfg(windows)]
fn section1335_single_image_startup_and_required_page_scan_do_not_adopt_parent_lists() {
    for required in [false, true] {
        let (mut env, parent, _) = book_env();
        let media_folder = env.tmp.path().join("media");
        std::fs::create_dir_all(&media_folder).unwrap();
        let image = media_folder.join("requested.png");
        std::fs::write(&image, png_bytes()).unwrap();
        let ctx = egui::Context::default();
        let before = env.settings.startup_list_restore.clone();
        if required {
            // Similar-result required navigation carries an existing viewer session forward.
            assert!(env.load_folder_with_scan_owned(
                media_folder.clone(),
                None,
                OpenRequestOwner::Navigation,
                StartupListIntent::PageContinuation
            ));
            env.open_fullscreen(0, HistoryTrigger::UserChosen);
            env.open_required_fullscreen_location(
                &ctx,
                media_folder.clone(),
                crate::snapshot::SnapshotTarget::Fs(image.clone()),
                HistoryTrigger::UserChosen,
            );
        } else {
            env.start_startup_open_path_resolve(
                image.clone(),
                StartupOpenPathSource::InitialStartup,
                &ctx,
            );
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
            while env.startup_open_path_resolve_pending.is_some() {
                env.poll_startup_open_path_resolve(&ctx);
                assert_eq!(env.settings.startup_list_restore, before);
                assert!(
                    std::time::Instant::now() < deadline,
                    "startup resolver did not settle"
                );
                std::thread::yield_now();
            }
        }
        assert_eq!(env.settings.startup_list_restore, before);
        if env.folder_pane_open_pending.is_some() {
            let ready = mutation_refresh_wait_for_order_scan(&mut env);
            assert!(env.resolve_main_folder_open_ready(&ctx, ready).is_none());
        }
        env.settle_open_path_classification_for_test();
        settle_book(&mut env);
        assert_eq!(env.current_folder.as_ref(), Some(&media_folder));
        assert!(env.fullscreen_idx.is_some());
        assert_physical(&env, &parent, None);
        assert_eq!(env.settings.startup_list_restore, before);
    }
}

#[test]
#[cfg(windows)]
fn section1335_required_missing_page_and_cancel_preserve_restore_record() {
    for cancel in [false, true] {
        let (mut env, parent, _) = book_env();
        let folder = env.tmp.path().join("required");
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(folder.join("other.png"), png_bytes()).unwrap();
        let before = env.settings.startup_list_restore.clone();
        env.open_required_fullscreen_location(
            &egui::Context::default(),
            folder.clone(),
            crate::snapshot::SnapshotTarget::Fs(folder.join("missing.png")),
            HistoryTrigger::UserChosen,
        );
        if cancel {
            assert!(env.cancel_required_fullscreen_folder_open());
        } else {
            let ready = mutation_refresh_wait_for_order_scan(&mut env);
            assert!(
                env.resolve_main_folder_open_ready(&egui::Context::default(), ready)
                    .is_none()
            );
        }
        assert!(env.fullscreen_idx.is_none());
        assert_eq!(env.settings.startup_list_restore, before);
        assert_physical(&env, &parent, None);
    }
}

#[test]
fn section1335_snapshot_and_reading_history_direct_pages_keep_physical_restore_record() {
    for snapshot in [false, true] {
        let (mut env, parent, book) = book_env();
        open_zip(&mut env, &book, true);
        env.close_fullscreen(); // Internal close is deliberately not a list action.
        let before = env.settings.startup_list_restore.clone();
        if snapshot {
            env.activate_snapshot(crate::snapshot::SnapshotSourceLabel::Mixed);
            assert!(env.is_snapshot_active());
            env.open_fullscreen(0, HistoryTrigger::UserChosen);
        } else {
            env.enter_reading_history();
            assert!(env.items_are_reading_history_view);
            // A synthetic list is ineligible even if no playable history row exists yet.
            env.finish_main_list_open(StartupListIntent::ExplicitList);
        }
        env.close_fullscreen_to_page_list();
        assert_eq!(env.settings.startup_list_restore, before);
        env.capture_main_list_restore_cursor();
        assert_eq!(env.settings.startup_list_restore, before);
        assert_physical(&env, &parent, None);
    }
}

#[test]
fn section1335_subfolder_scan_reinstall_and_read_close_preserve_parent_cursor() {
    let (mut env, parent, _) = book_env();
    let child = parent.join("child");
    std::fs::create_dir_all(&child).unwrap();
    let page = child.join("aggregate.png");
    std::fs::write(&page, png_bytes()).unwrap();
    env.load_folder(parent.clone());
    env.selected = env
        .items
        .iter()
        .position(|item| matches!(item, GridItem::Folder(path) if path == &child));
    assert!(env.selected.is_some());
    env.capture_main_list_restore_cursor();
    let before = env.settings.startup_list_restore.clone();
    env.start_subfolder_expansion_scan_roots(parent.clone(), vec![child]);
    let ctx = egui::Context::default();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while !env.items_are_subfolder_expansion_view {
        env.poll_subfolder_expansion(&ctx);
        assert_eq!(env.settings.startup_list_restore, before);
        assert!(
            std::time::Instant::now() < deadline,
            "subfolder scan did not adopt"
        );
        std::thread::yield_now();
    }
    assert!(env.subfolder_expansion_saved_folder.as_ref().is_some());
    env.reload_current_folder_preserving_override();
    env.finish_subfolder_expansion_prepare_for_test();
    assert!(env.items_are_subfolder_expansion_view);
    assert_eq!(env.settings.startup_list_restore, before);
    let index = env
        .items
        .iter()
        .position(|item| matches!(item, GridItem::Image(path) if path == &page))
        .unwrap();
    env.open_fullscreen(index, HistoryTrigger::UserChosen);
    env.close_fullscreen_to_page_list();
    env.capture_main_list_restore_cursor();
    assert_eq!(env.settings.startup_list_restore, before);
    assert_eq!(restore_cursor(&env).unwrap().name, "child");
    env.on_exit_inner();
    assert_eq!(env.settings.startup_list_restore, before);
}

#[test]
#[cfg(windows)]
fn section1335_independent_mounted_close_cannot_adopt_or_capture_main_restore() {
    let (mut env, parent, _) = book_env();
    let before = env.settings.startup_list_restore.clone();
    let independent_folder = env.tmp.path().join("independent");
    std::fs::create_dir_all(&independent_folder).unwrap();
    let page = independent_folder.join("page.png");
    std::fs::write(&page, png_bytes()).unwrap();
    env.build_window_context_for_test(1335, |bundle| {
        bundle.current_folder = Some(independent_folder.clone());
        bundle.items = vec![GridItem::Image(page.clone())];
        bundle.thumbnails = vec![ThumbnailState::Pending];
        bundle.image_metas = vec![None];
        bundle.visible_indices = vec![0];
        bundle.selected = Some(0);
        bundle.fullscreen_idx = Some(0);
        bundle.viewer_presentation = ViewerPresentation::DetachedWindow;
    });
    env.begin_active_detached_session(1335, DetachedSource::Image);
    let closed = env.with_active_viewer_context(|app| {
        app.close_fullscreen_to_page_list();
        app.capture_main_list_restore_cursor();
        app.fullscreen_idx.is_none()
    });
    assert_eq!(closed, Some(true));
    assert_eq!(env.settings.startup_list_restore, before);
    assert_eq!(env.current_folder.as_ref(), Some(&parent));
}

#[test]
fn section1335_drive_and_other_startup_modes_retain_their_explicit_dispatch() {
    for mode in [
        crate::settings::StartupFolderMode::Drives,
        crate::settings::StartupFolderMode::ReadingHistory,
        crate::settings::StartupFolderMode::Specific,
    ] {
        let (mut env, parent, _) = book_env();
        let specific = env.tmp.path().join("specific");
        std::fs::create_dir_all(&specific).unwrap();
        env.settings.startup_folder_mode = mode;
        env.settings.startup_folder_path = Some(specific.clone());
        env.open_default_startup_target();
        settle_book(&mut env);
        assert!(env.fullscreen_idx.is_none());
        match mode {
            crate::settings::StartupFolderMode::Drives => {
                assert!(env.items_are_drive_list);
                assert_eq!(restore_target(&env), &StartupListTarget::DriveList);
                assert!(restore_cursor(&env).is_none());
                env.settings.startup_folder_mode = crate::settings::StartupFolderMode::Previous;
                restart_previous(&mut env);
                assert!(env.items_are_drive_list);
                assert_eq!(restore_target(&env), &StartupListTarget::DriveList);
            }
            crate::settings::StartupFolderMode::ReadingHistory => {
                assert!(env.items_are_reading_history_view);
                assert_physical(&env, &parent, None);
            }
            crate::settings::StartupFolderMode::Specific => {
                assert_eq!(env.current_folder.as_ref(), Some(&specific));
                assert_physical(&env, &specific, None);
            }
            _ => unreachable!(),
        }
    }
}

fn supply_prepared_pdf(app: &mut App, count: u32) {
    app.settle_open_path_classification_for_test();
    let mut transition = app
        .top_level_grid_view
        .take_history_navigation_transition()
        .unwrap();
    let HistoryNavigationTransition::Physical(request) = &mut transition else {
        panic!("PDF must retain its physical request owner");
    };
    let PhysicalHistoryPhase::Preflighting { preflight, .. } = &mut request.phase else {
        panic!("PDF must still be preflighting");
    };
    *preflight = crate::app::collection_navigation::PhysicalHistoryPreflight::ready_for_test(
        crate::app::collection_navigation::PhysicalHistoryPreflightPayload::PdfPages(
            crate::pdf_loader::PdfEnumerateResult {
                pages: (0..count)
                    .map(|page_num| crate::pdf_loader::PdfPageEntry {
                        page_num,
                        mtime: 1,
                        file_size: 1,
                    })
                    .collect(),
                direction: None,
                stamp: None,
            },
        ),
    );
    app.top_level_grid_view
        .set_history_navigation_transition(Some(transition));
}

#[test]
fn section1335_prepared_pdf_adoption_keeps_direct_and_explicit_list_intents() {
    for direct in [false, true] {
        let (mut env, parent, _) = book_env();
        let pdf = parent.join("prepared.pdf");
        std::fs::write(&pdf, b"%PDF-1.4\n").unwrap();
        let before = env.settings.startup_list_restore.clone();
        assert!(env.start_physical_history_transition(
            PhysicalHistoryIntent::Navigation {
                replay: None,
                auto_fullscreen: direct
            },
            pdf.clone(),
            if direct {
                StartupListIntent::PageContinuation
            } else {
                StartupListIntent::ExplicitList
            },
        ));
        assert_eq!(env.settings.startup_list_restore, before);
        supply_prepared_pdf(&mut env, 2);
        assert_eq!(env.settings.startup_list_restore, before);
        env.poll_collection_history_transition(&egui::Context::default());
        assert_eq!(env.current_folder.as_ref(), Some(&pdf));
        assert_eq!(env.items.len(), 2);
        assert!(env.pdf_enumerate_pending.is_none());
        if direct {
            assert!(env.fullscreen_idx.is_some());
            assert_eq!(env.settings.startup_list_restore, before);
            env.close_fullscreen_to_page_list();
        } else {
            assert!(env.fullscreen_idx.is_none());
        }
        assert_physical(&env, &pdf, None);
    }
}

#[test]
fn section1335_warm_pdf_verification_does_not_reinterpret_original_presentation_intent() {
    for (explicit, actual_count) in [(false, 2), (false, 3), (true, 2), (true, 3)] {
        let (mut env, parent, _) = book_env();
        let pdf = parent.join("warm.pdf");
        std::fs::write(&pdf, b"%PDF-1.4\n").unwrap();
        let stamp = std::fs::metadata(&pdf).unwrap();
        env.get_or_open_catalog(&parent)
            .unwrap()
            .set_pdf_meta(
                "warm.pdf",
                crate::ui_helpers::mtime_secs(&stamp),
                stamp.len() as i64,
                2,
                false,
            )
            .unwrap();
        assert_eq!(
            env.load_pdf_as_folder_owned(
                pdf.clone(),
                OpenRequestOwner::Navigation,
                if explicit {
                    StartupListIntent::ExplicitList
                } else {
                    StartupListIntent::PageContinuation
                },
            ),
            FolderOpenOutcome::Loaded
        );
        assert_eq!(env.items.len(), 2);
        assert!(matches!(
            env.pdf_enumerate_pending.as_ref().map(|pending| &pending.5),
            Some(PdfOpenPhase::CommittedVerification {
                placeholder_count: 2
            })
        ));
        if explicit {
            assert_physical(&env, &pdf, None);
        } else {
            assert_physical(&env, &parent, None);
        }
        let adopted = env.settings.startup_list_restore.clone();
        env.pdf_enumerate_pending.as_mut().unwrap().2 =
            crate::pdf_loader::completed_enumerate_result_handle(
                &pdf,
                Ok(crate::pdf_loader::PdfEnumerateResult {
                    pages: (0..actual_count)
                        .map(|page_num| crate::pdf_loader::PdfPageEntry {
                            page_num,
                            mtime: crate::ui_helpers::mtime_secs(&stamp),
                            file_size: stamp.len(),
                        })
                        .collect(),
                    direction: None,
                    stamp: None,
                }),
            );
        env.poll_pdf_enumerate();
        assert!(env.pdf_enumerate_pending.is_none());
        assert_eq!(env.items.len(), actual_count as usize);
        assert_eq!(
            env.settings.startup_list_restore, adopted,
            "verification is not a second explicit list adoption"
        );
    }
}

fn wait_smart(app: &mut App) {
    let ctx = egui::Context::default();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        app.poll_smart_folder(&ctx);
        if app.smart_folder_transition.is_none()
            && app.smart_folder_pending.is_none()
            && app.smart_folder_prepare_pending.is_none()
        {
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "Smart folder did not settle"
        );
        std::thread::yield_now();
    }
}

#[test]
fn section1335_smart_root_retains_parent_and_physical_child_acceptance_owns_new_target() {
    for direct in [false, true] {
        let (mut env, parent, _) = book_env();
        let source = env.tmp.path().join("smart-source");
        let child = source.join("child");
        std::fs::create_dir_all(&child).unwrap();
        std::fs::write(child.join("page.png"), png_bytes()).unwrap();
        let mut definition = crate::settings::SmartFolderDefinition::new("Restore fixture");
        definition.rules.push(crate::settings::SmartFolderRule::new(
            source,
            true,
            Default::default(),
        ));
        let id = definition.id;
        env.settings.smart_folders = vec![definition];
        let before = env.settings.startup_list_restore.clone();
        env.open_smart_folder(id, false);
        wait_smart(&mut env);
        assert!(env.items_are_smart_folder_view);
        env.capture_main_list_restore_cursor();
        assert_eq!(env.settings.startup_list_restore, before);
        let kind = env.smart_physical_target_kind(&child).unwrap();
        assert!(
            env.begin_smart_physical_navigation(
                child.clone(),
                kind,
                direct,
                None,
                None,
                if direct {
                    StartupListIntent::PageContinuation
                } else {
                    StartupListIntent::ExplicitList
                },
            )
            .is_ok()
        );
        assert_eq!(env.settings.startup_list_restore, before);
        wait_smart(&mut env);
        assert_eq!(env.effective_folder().as_ref(), Some(&child));
        if direct {
            assert_physical(&env, &parent, None);
            if env.fullscreen_idx.is_none() {
                env.open_fullscreen(0, HistoryTrigger::UserChosen);
            }
            env.close_fullscreen_to_page_list();
        }
        assert_physical(&env, &child, None);
        let expected_after_return = Some(StartupListRestore::V1 {
            target: StartupListTarget::PhysicalList {
                logical_path: child.clone(),
                zip_prefix: None,
            },
            // Accepted close has no grid geometry yet; leaving the shown list measures row zero.
            cursor: Some(ListCursorHint {
                name: "page.png".into(),
                rows_above: Some(0),
            }),
        });
        let synthetic = crate::app::smart_folder::smart_folder_synthetic_path(id);
        assert!(env.restore_smart_folder_for_synthetic_path(&synthetic));
        wait_smart(&mut env);
        assert!(env.items_are_smart_folder_view);
        env.capture_main_list_restore_cursor();
        assert_eq!(env.settings.startup_list_restore, expected_after_return);
    }
}

#[test]
#[cfg(windows)]
fn section1335_collection_root_child_and_descendant_use_adopted_physical_location() {
    for direct in [false, true] {
        let (mut env, parent, _) = book_env();
        let child = env.tmp.path().join("collection-source");
        let descendant = child.join("descendant");
        std::fs::create_dir_all(&descendant).unwrap();
        std::fs::write(child.join("page.png"), png_bytes()).unwrap();
        std::fs::write(descendant.join("nested.png"), png_bytes()).unwrap();
        install_collection_item_for_detached_plan(
            &mut env,
            &child,
            crate::collection_store::CollectionResolvedKind::Folder,
        );
        let before = env.settings.startup_list_restore.clone();
        env.finish_main_list_open(StartupListIntent::ExplicitList);
        env.capture_main_list_restore_cursor();
        assert_eq!(env.settings.startup_list_restore, before);
        let owner = env.collection_grid_physical_load_owner(0, &child).unwrap();
        assert!(env.load_folder_with_scan_owned(
            child.clone(),
            None,
            OpenRequestOwner::CollectionGridPhysical(owner),
            if direct {
                StartupListIntent::PageContinuation
            } else {
                StartupListIntent::ExplicitList
            }
        ));
        assert_eq!(env.effective_folder().as_ref(), Some(&child));
        if direct {
            assert_physical(&env, &parent, None);
            let page = env
                .items
                .iter()
                .position(|item| matches!(item, GridItem::Image(_)))
                .unwrap();
            env.open_fullscreen(page, HistoryTrigger::UserChosen);
            env.close_fullscreen_to_page_list();
        }
        assert_physical(&env, &child, None);
        let owner = env
            .collection_grid_physical_load_owner(0, &descendant)
            .unwrap();
        assert!(env.load_folder_with_scan_owned(
            descendant.clone(),
            None,
            OpenRequestOwner::CollectionGridPhysical(owner),
            StartupListIntent::ExplicitList
        ));
        assert_eq!(env.effective_folder().as_ref(), Some(&descendant));
        assert_physical(&env, &descendant, None);
    }
}

#[test]
fn section1335_rating_root_and_physical_child_adopt_only_successful_list_navigation() {
    let (mut env, parent, _) = book_env();
    let child = env.tmp.path().join("rating-child");
    std::fs::create_dir_all(&child).unwrap();
    std::fs::write(child.join("page.png"), png_bytes()).unwrap();
    let before = env.settings.startup_list_restore.clone();
    env.enter_rating_view_from_menu(1);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while env.rating_view_pending.is_some() {
        env.poll_rating_view();
        assert!(
            std::time::Instant::now() < deadline,
            "Rating view did not settle"
        );
        std::thread::yield_now();
    }
    assert!(env.items_are_rating_view);
    env.capture_main_list_restore_cursor();
    assert_eq!(env.settings.startup_list_restore, before);
    assert_physical(&env, &parent, None);
    let owner = env.rating_view_physical_load_owner(&child).unwrap();
    assert!(env.start_rating_physical_open(owner, StartupListIntent::ExplicitList));
    assert_eq!(env.settings.startup_list_restore, before);
    settle_book(&mut env);
    assert_eq!(env.effective_folder().as_ref(), Some(&child));
    assert_physical(&env, &child, None);
}

#[test]
fn section1335_search_root_retains_restore_and_explicit_book_child_can_adopt() {
    let (mut env, parent, book) = book_env();
    env.open_global_search();
    env.replace_search_view_items(vec![GridItem::ZipFile(book.clone())], vec![None]);
    assert!(env.items_are_global_search_view);
    let before = env.settings.startup_list_restore.clone();
    env.capture_main_list_restore_cursor();
    env.finish_main_list_open(StartupListIntent::ExplicitList);
    assert_eq!(env.settings.startup_list_restore, before);
    assert_physical(&env, &parent, None);
    env.advance_drilled_current_path(&book);
    open_zip(&mut env, &book, false);
    assert!(env.global_search.drill.is_some());
    assert_physical(&env, &book, Some(""));
}

#[test]
fn section1335_edit_spread_acceptance_uses_navigation_anchor_after_edit_target_changes() {
    let (mut env, _, book) = book_env();
    open_zip(&mut env, &book, true);
    env.open_fullscreen(0, HistoryTrigger::UserChosen);
    env.spread_mode = crate::settings::SpreadMode::Single;
    env.text_spread_ctx = Some(PageEditSpreadPivot {
        saved_mode: crate::settings::SpreadMode::Rtl,
        pair: (0, 1),
        navigation_anchor_idx: 0,
    });
    env.text_mode = true;
    env.switch_text_target_in_spread(1);
    assert_eq!(env.fullscreen_idx, Some(1));
    env.selected = Some(1);
    let anchor_name = env.items[0].name().to_string();
    env.close_fullscreen_to_page_list();
    assert_physical(&env, &book, Some(""));
    assert_eq!(restore_cursor(&env).unwrap().name, anchor_name);
    assert_eq!(env.selected, Some(0));
}

fn wait_stack(app: &mut App, ctx: &egui::Context) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while app.stack_script_pending.is_some() {
        app.poll_stack_script(ctx);
        assert!(
            std::time::Instant::now() < deadline,
            "stack switch did not settle"
        );
        std::thread::yield_now();
    }
}

#[test]
fn section1335_flat_filename_stack_acceptance_never_persists_member_name_as_aggregate_cursor() {
    use crate::filename_stack::{StackMember, StackView};
    let (mut env, parent, _) = book_env();
    let folder = env.tmp.path().join("stack-source");
    std::fs::create_dir_all(&folder).unwrap();
    let paths = [folder.join("post_0.png"), folder.join("post_1.png")];
    for page in &paths {
        std::fs::write(page, png_bytes()).unwrap();
    }
    assert!(env.load_folder_with_scan_owned(
        folder.clone(),
        None,
        OpenRequestOwner::Navigation,
        StartupListIntent::PageContinuation
    ));
    assert_physical(&env, &parent, None);
    let view = std::sync::Arc::new(StackView::build(
        folder.clone(),
        Vec::new(),
        Vec::new(),
        paths
            .iter()
            .map(|path| StackMember {
                path: path.clone(),
                mtime: 0,
                size: Some(png_bytes().len() as i64),
                is_video: false,
            })
            .collect(),
        '_',
        crate::settings::SortOrder::FileName,
    ));
    let (items, metas) = view.materialize_aggregated();
    assert!(matches!(items.first(), Some(GridItem::Stack { .. })));
    env.install_new_items(items, metas);
    env.stack_mode_requested = true;
    env.stack_view = Some(view);
    env.selected = Some(0);
    env.rebuild_visible_indices();
    let ctx = egui::Context::default();
    assert!(env.stack_try_open_from_grid(&ctx, 0, false));
    wait_stack(&mut env, &ctx);
    assert!(env.stack_showing_flat);
    assert!(env.fullscreen_idx.is_some());
    assert_physical(&env, &parent, None);
    env.close_fullscreen_to_page_list();
    assert_physical(&env, &folder, None);
    assert!(
        restore_cursor(&env).is_none(),
        "flat member cannot name an aggregate row"
    );
    env.stack_reconcile_after_fullscreen_close(&ctx);
    wait_stack(&mut env, &ctx);
    assert!(!env.stack_showing_flat);
    assert!(matches!(env.items.first(), Some(GridItem::Stack { .. })));
    assert!(restore_cursor(&env).is_none());
}

#[test]
#[cfg(windows)]
fn section1335_linked_f12_round_trip_keeps_main_record_during_grid_exposure() {
    let (mut env, parent, book) = book_env();
    env.settings.detached_viewer_open_images_in_window = false;
    open_zip(&mut env, &book, true);
    let before = env.settings.startup_list_restore.clone();
    let page = env.fullscreen_idx;
    for enabled in [true, false] {
        env.toggle_detached_viewer_mode();
        assert_eq!(env.settings.detached_viewer_enabled, enabled);
        assert_eq!(env.fullscreen_idx, page);
        env.capture_main_list_restore_cursor();
        env.persist_window_state_and_flush(PersistScope::ProcessKeepsRunning);
        assert_eq!(env.settings.startup_list_restore, before);
        assert_physical(&env, &parent, None);
    }
}

#[test]
#[cfg(windows)]
fn section1335_tray_persistence_and_restore_keep_reading_session_and_parent_list() {
    let (mut env, parent, _) = book_env();
    let media_folder = env.tmp.path().join("media-session");
    std::fs::create_dir_all(&media_folder).unwrap();
    let path = media_folder.join("clip.mp4");
    std::fs::write(&path, b"headless media fixture").unwrap();
    assert!(env.load_folder_with_scan_owned(
        media_folder.clone(),
        None,
        OpenRequestOwner::Navigation,
        StartupListIntent::PageContinuation
    ));
    let index = env
        .items
        .iter()
        .position(|item| matches!(item, GridItem::Video(_)))
        .unwrap();
    let mut player = crate::video::VideoPlayer::disconnected_for_test(path, 75.0);
    let playing_since = std::time::Instant::now();
    player.configure_native_timing_for_test(75.0, 120.0, true, false);
    player.set_playing(true);
    assert!(player.intent_playing());
    env.fs_cache.insert(
        index,
        FsCacheEntry::Video {
            player: Box::new(player),
            load_seq: 0,
        },
    );
    let original_player = match env.fs_cache.get(&index).unwrap() {
        FsCacheEntry::Video { player, .. } => std::ptr::from_ref(player.as_ref()),
        _ => unreachable!(),
    };
    env.fullscreen_idx = Some(index);
    env.selected = Some(index);
    env.viewer_presentation = ViewerPresentation::Fullscreen;
    let before = env.settings.startup_list_restore.clone();
    let generation = env.items_generation;
    close_root_to_tray(&mut env);
    assert_eq!(env.settings.startup_list_restore, before);
    env.sync_after_restore(&egui::Context::default());
    assert!(env.window_visible);
    assert_eq!(env.items_generation, generation);
    assert_eq!(env.current_folder.as_ref(), Some(&media_folder));
    assert_eq!(env.fullscreen_idx, Some(index));
    let Some(FsCacheEntry::Video { player, .. }) = env.fs_cache.get(&index) else {
        panic!("media session was recreated");
    };
    assert!(player.intent_playing());
    assert_eq!(std::ptr::from_ref(player.as_ref()), original_player);
    let position = player.position();
    assert!(position >= 75.0);
    assert!(
        position <= 75.0 + playing_since.elapsed().as_secs_f64() + 0.1,
        "live media clock must continue from its retained position: {position}"
    );
    assert_eq!(env.settings.startup_list_restore, before);
    assert_physical(&env, &parent, None);
    env.on_exit_inner();
    assert_eq!(env.settings.startup_list_restore, before);
}

#[cfg(windows)]
fn close_root_to_tray(app: &mut App) {
    // Exercise the actual window-X handler without a native window or tray thread.
    assert!(app.main_hwnd.is_none());
    assert!(app.window_visible);
    app.settings.minimize_to_tray_on_close = true;
    app.tray_controller = Some(crate::tray::TrayController::controller_for_test());
    let ctx = egui::Context::default();
    let mut raw = egui::RawInput::default();
    raw.viewports
        .entry(egui::ViewportId::ROOT)
        .or_default()
        .events
        .push(egui::ViewportEvent::Close);
    let output = ctx.run(raw, |ctx| assert!(app.maybe_intercept_close(ctx)));
    assert!(!app.window_visible);
    assert!(
        output.viewport_output[&egui::ViewportId::ROOT]
            .commands
            .iter()
            .any(|command| matches!(command, egui::ViewportCommand::CancelClose))
    );
}

#[cfg(windows)]
fn direct_tray_book_env() -> (phase_c_support::AppTestEnv, PathBuf, PathBuf) {
    let (mut env, parent, book) = book_env();
    for index in 0..12 {
        write_zip(&parent.join(format!("before-{index:02}.zip")), &["a.png"]);
    }
    env.settings.sort_order = crate::settings::SortOrder::FileName;
    env.settings.video_in_window_mode = false;
    env.settings.detached_viewer_open_images_in_window = false;
    env.settings.book_open_resume = crate::settings::ResumeMode::Resume;
    env.load_folder(parent.clone());
    let index = env
        .items
        .iter()
        .position(|item| matches!(item, GridItem::ZipFile(path) if path == &book))
        .unwrap();
    assert_eq!(index, 12);
    env.last_grid_cols = 3;
    env.last_cell_h = 40.0;
    env.scroll_offset_y = 80.0;
    env.selected = Some(index);
    // Accepted grid input owns the departing cursor capture and direct page open.
    let nav = env
        .handle_gamepad_grid_accept(&egui::Context::default())
        .expect("accepted main grid book yields its owned direct open");
    assert!(env.apply_fullscreen_close_nav_immediate(nav));
    settle_book(&mut env);
    assert_eq!(env.current_folder.as_ref(), Some(&book));
    assert!(env.fullscreen_idx.is_some());
    assert_eq!(
        restore_cursor(&env),
        Some(&ListCursorHint {
            name: "book.zip".into(),
            rows_above: Some(2),
        })
    );
    env.open_fullscreen(1, HistoryTrigger::UserChosen);
    assert_eq!(env.fullscreen_idx, Some(1));
    assert_physical(&env, &parent, None);
    (env, parent, book)
}

#[test]
#[cfg(windows)]
fn section1335_tray_direct_book_hide_restore_returns_parent_and_preserves_record_and_resume() {
    for restore_last_cursor in [true, false] {
        let (mut env, parent, book) = direct_tray_book_env();
        // This setting controls next-start behavior, not the live parent-return cursor.
        env.settings.restore_last_cursor = restore_last_cursor;
        let before = env.settings.startup_list_restore.clone();
        close_root_to_tray(&mut env);
        settle_book(&mut env);
        assert!(env.fullscreen_idx.is_none());
        assert_eq!(env.current_folder.as_ref(), Some(&parent));
        assert_eq!(env.settings.startup_list_restore, before);
        assert_eq!(
            crate::settings::Settings::load().startup_list_restore,
            before
        );
        assert_eq!(env.scroll_selected_to_rows_above, Some(2));
        let generation = env.items_generation;
        env.sync_after_restore(&egui::Context::default());
        assert!(env.window_visible);
        assert_eq!(env.items_generation, generation);
        assert_eq!(env.current_folder.as_ref(), Some(&parent));
        assert!(env.fullscreen_idx.is_none());
        assert_eq!(env.settings.startup_list_restore, before);
        assert_eq!(env.items[env.selected.unwrap()].name(), "book.zip");
        assert_eq!(env.scroll_selected_to_rows_above, Some(2));
        // Returning to the parent must not reset the independently saved reading position.
        open_zip(&mut env, &book, true);
        assert_eq!(env.fullscreen_idx, Some(1));
        assert_eq!(env.settings.startup_list_restore, before);
    }
}

#[test]
#[cfg(windows)]
fn section1335_tray_direct_book_quit_while_hidden_restarts_at_preserved_parent_list() {
    let (mut env, parent, book) = direct_tray_book_env();
    let before = env.settings.startup_list_restore.clone();
    close_root_to_tray(&mut env);
    assert!(!env.window_visible);
    env.on_exit_inner();
    assert_eq!(
        crate::settings::Settings::load().startup_list_restore,
        before
    );
    restart_previous_from_saved_settings(&mut env);
    assert_eq!(env.current_folder.as_ref(), Some(&parent));
    assert!(env.fullscreen_idx.is_none());
    assert_eq!(env.settings.startup_list_restore, before);
    assert_eq!(env.items[env.selected.unwrap()].name(), "book.zip");
    assert_eq!(env.scroll_selected_to_rows_above, Some(2));
    open_zip(&mut env, &book, true);
    assert_eq!(env.fullscreen_idx, Some(1));
}

#[test]
#[cfg(windows)]
fn section1335_tray_direct_book_close_retires_pending_required_page_navigation() {
    let (mut env, parent, _) = direct_tray_book_env();
    let before = env.settings.startup_list_restore.clone();
    let ctx = egui::Context::default();
    env.fs_info_panel.open = crate::ui_helpers::MetadataPanelOpenState::ByPointer;
    env.fs_info_panel.locked = true;
    let page = env.fullscreen_idx.unwrap();
    env.begin_fs_folder_navigation_sequence(&ctx, page);
    let folder = env.tmp.path().join("required-during-tray-close");
    std::fs::create_dir_all(&folder).unwrap();
    let image = folder.join("requested.png");
    std::fs::write(&image, png_bytes()).unwrap();
    env.open_required_fullscreen_location(
        &ctx,
        folder,
        crate::snapshot::SnapshotTarget::Fs(image),
        HistoryTrigger::UserChosen,
    );
    assert!(env.folder_pane_open_pending.is_some());
    assert!(env.fullscreen_idx.is_some());
    assert!(
        env.fs_holdover_tex
            .as_ref()
            .and_then(FsHoldover::navigation_sequence)
            .is_some()
    );
    close_root_to_tray(&mut env);
    settle_book(&mut env);
    assert_eq!(env.current_folder.as_ref(), Some(&parent));
    assert!(env.fullscreen_idx.is_none());
    assert!(env.folder_pane_open_pending.is_none());
    assert!(env.fs_nav_locked_gen.is_none());
    assert!(!env.fs_info_panel.locked);
    assert!(
        env.fs_holdover_tex
            .as_ref()
            .and_then(FsHoldover::navigation_sequence)
            .is_none()
    );
    assert_eq!(env.settings.startup_list_restore, before);
    env.sync_after_restore(&egui::Context::default());
    assert_eq!(env.current_folder.as_ref(), Some(&parent));
    assert!(env.fullscreen_idx.is_none());
}

#[test]
#[cfg(windows)]
fn section1335_tray_explicit_book_list_hide_restore_keeps_normally_opened_and_backspace_lists() {
    let _input_guard = crate::key_input::lock_test_input();
    for backspace in [false, true] {
        let (mut env, _, book) = book_env();
        env.settings.video_in_window_mode = false;
        env.settings.detached_viewer_open_images_in_window = false;
        open_zip(&mut env, &book, backspace);
        if backspace {
            env.open_fullscreen(1, HistoryTrigger::UserChosen);
            let ctx = egui::Context::default();
            ctx.begin_pass(egui::RawInput {
                events: vec![fullscreen_fixed_key_event(egui::Key::Backspace)],
                ..Default::default()
            });
            assert!(env.handle_fullscreen_root_key_input(&ctx));
            let _ = ctx.end_pass();
        }
        assert!(env.fullscreen_idx.is_none());
        assert_physical(&env, &book, Some(""));
        env.selected = Some(1);
        env.capture_main_list_restore_cursor();
        let before = env.settings.startup_list_restore.clone();
        close_root_to_tray(&mut env);
        env.sync_after_restore(&egui::Context::default());
        assert!(env.window_visible);
        assert!(env.fullscreen_idx.is_none());
        assert_eq!(env.current_folder.as_ref(), Some(&book));
        assert_eq!(env.settings.startup_list_restore, before);
        // A book whose list was shown explicitly keeps that list even if reading resumes.
        env.open_fullscreen(1, HistoryTrigger::UserChosen);
        close_root_to_tray(&mut env);
        env.sync_after_restore(&egui::Context::default());
        assert!(env.window_visible);
        assert!(env.fullscreen_idx.is_none());
        assert_eq!(env.current_folder.as_ref(), Some(&book));
        assert_physical(&env, &book, Some(""));
        assert_eq!(env.settings.startup_list_restore, before);
    }
}

#[test]
#[cfg(windows)]
fn section1335_tray_detached_and_switching_book_hide_restore_keep_session_and_parent_record() {
    for switching in [false, true] {
        let (mut env, parent, book) = book_env();
        env.settings.detached_viewer_open_images_in_window = false;
        open_zip(&mut env, &book, true);
        if switching {
            begin_test_video_presentation_transition(
                &mut env,
                ViewerPresentation::DetachedWindow,
                false,
            );
            assert!(env.video_presentation_transition.is_transitioning());
        } else {
            env.toggle_detached_viewer_mode();
            assert!(env.viewer_session_is_detached());
        }
        assert!(env.viewer_session_is_detached_or_switching());
        let before = env.settings.startup_list_restore.clone();
        let page = env.fullscreen_idx;
        let presentation = env.viewer_presentation;
        let generation = env.items_generation;
        close_root_to_tray(&mut env);
        env.sync_after_restore(&egui::Context::default());
        assert!(env.window_visible);
        assert_eq!(env.current_folder.as_ref(), Some(&book));
        assert_eq!(env.fullscreen_idx, page);
        assert_eq!(env.viewer_presentation, presentation);
        assert_eq!(env.items_generation, generation);
        assert_eq!(
            env.video_presentation_transition.is_transitioning(),
            switching
        );
        assert_eq!(env.settings.startup_list_restore, before);
        assert_physical(&env, &parent, None);
    }
}

#[test]
#[cfg(windows)]
fn section1335_pane_scan_is_explicit_even_with_image_book_auto_open_enabled() {
    let (mut env, _, _) = book_env();
    let child = env.tmp.path().join("pane-image-book");
    std::fs::create_dir_all(&child).unwrap();
    std::fs::write(child.join("page.png"), png_bytes()).unwrap();
    env.settings.auto_fullscreen_image_folders = true;
    let before = env.settings.startup_list_restore.clone();
    env.start_folder_pane_open(child.clone());
    assert_eq!(env.settings.startup_list_restore, before);
    let ready = mutation_refresh_wait_for_order_scan(&mut env);
    assert!(matches!(
        ready.purpose,
        FolderOpenScanPurpose::PaneNavigation
    ));
    let resolved = env
        .resolve_main_folder_open_ready(&egui::Context::default(), ready)
        .unwrap();
    assert_eq!(env.settings.startup_list_restore, before);
    env.load_folder_with_scan(resolved.path, Some(resolved.scan));
    assert_physical(&env, &child, None);
    assert!(env.fullscreen_idx.is_none());
}

#[test]
#[cfg(windows)]
fn section1335_grid_folder_candidate_classifies_mixed_and_image_only_without_main_commit_leak() {
    for mixed in [false, true] {
        let (mut env, parent, _) = book_env();
        let child = parent.join("candidate");
        std::fs::create_dir_all(&child).unwrap();
        std::fs::write(child.join("page.png"), png_bytes()).unwrap();
        if mixed {
            std::fs::create_dir_all(child.join("nested")).unwrap();
        }
        env.load_folder(parent.clone());
        env.settings.detached_viewer_open_images_in_window = true;
        env.settings.auto_fullscreen_image_folders = true;
        // Direct children of book_root are compiled books even with subfolders.
        env.settings.book_root = Some(env.tmp.path().join("compiled-books"));
        let index = env
            .items
            .iter()
            .position(|item| matches!(item, GridItem::Folder(path) if path == &child))
            .unwrap();
        env.selected = Some(index);
        env.capture_main_list_restore_cursor();
        let before = env.settings.startup_list_restore.clone();
        let ctx = egui::Context::default();
        assert!(env.open_grid_container_in_detached_book_context(&ctx, index));
        assert!(matches!(
            env.folder_pane_open_pending
                .as_ref()
                .map(|pending| &pending.purpose),
            Some(FolderOpenScanPurpose::GridFolderCandidate { .. })
        ));
        assert_eq!(env.settings.startup_list_restore, before);
        let ready = mutation_refresh_wait_for_order_scan(&mut env);
        let resolved = env.resolve_main_folder_open_ready(&ctx, ready);
        assert_eq!(env.settings.startup_list_restore, before);
        if mixed {
            let resolved = resolved.expect("mixed folder becomes a main list");
            env.load_folder_with_scan(resolved.path, Some(resolved.scan));
            assert!(env.fullscreen_idx.is_none());
            assert_physical(&env, &child, None);
        } else {
            assert!(
                resolved.is_none(),
                "image book opens in its independent context"
            );
            assert_eq!(env.current_folder.as_ref(), Some(&parent));
            env.with_active_viewer_context(|viewer| {
                assert_eq!(viewer.effective_folder().as_ref(), Some(&child));
                assert!(viewer.fullscreen_idx.is_some());
                viewer.close_fullscreen_to_page_list();
            })
            .expect("accepted image book owns a viewer context");
            assert_eq!(env.settings.startup_list_restore, before);
        }
    }
}

#[test]
fn section1335_failed_ab_switch_preserves_source_then_successful_explicit_switch_adopts() {
    let (mut env, parent, book) = book_env();
    let missing = env.tmp.path().join("missing-slot");
    env.set_quick_folder_slot_target(QuickFolderSlotId::A, parent.clone());
    env.set_quick_folder_slot_target(QuickFolderSlotId::B, missing.clone());
    env.active_quick_folder_slot = Some(QuickFolderSlotId::A);
    let before = env.settings.startup_list_restore.clone();
    assert!(env.start_quick_folder_slot_switch(QuickFolderSlotId::B, missing));
    assert_eq!(env.settings.startup_list_restore, before);
    settle_book(&mut env);
    assert_eq!(env.active_quick_folder_slot, Some(QuickFolderSlotId::A));
    assert_eq!(env.current_folder.as_ref(), Some(&parent));
    assert_eq!(env.settings.startup_list_restore, before);
    env.set_quick_folder_slot_target(QuickFolderSlotId::B, book.clone());
    assert!(env.start_quick_folder_slot_switch(QuickFolderSlotId::B, book.clone()));
    assert_eq!(env.settings.startup_list_restore, before);
    settle_book(&mut env);
    assert_eq!(env.active_quick_folder_slot, Some(QuickFolderSlotId::B));
    assert_eq!(env.current_folder.as_ref(), Some(&book));
    assert_physical(&env, &book, Some(""));
}

#[test]
#[cfg(windows)]
fn section1335_back_to_list_key_rejected_by_modal_then_accepted_updates_record_once() {
    let (mut env, parent, book) = book_env();
    env.settings.video_in_window_mode = false;
    open_zip(&mut env, &book, true);
    assert!(!env.viewer_session_is_detached());
    assert!(!env.fullscreen_embedded_still_active());
    let before = env.settings.startup_list_restore.clone();
    let viewed = env.fullscreen_idx;
    let ctx = egui::Context::default();
    env.show_subfolder_expansion_dialog = true;
    assert!(env.any_modal_dialog_open_for_fullscreen_keys());
    ctx.begin_pass(egui::RawInput {
        events: vec![egui::Event::Key {
            key: egui::Key::Backspace,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        }],
        ..Default::default()
    });
    assert!(
        env.handle_fullscreen_root_key_input(&ctx),
        "modal consumes leaked root input"
    );
    let _ = ctx.end_pass();
    assert_eq!(env.fullscreen_idx, viewed);
    assert_eq!(env.settings.startup_list_restore, before);
    assert_physical(&env, &parent, None);

    env.show_subfolder_expansion_dialog = false;
    ctx.begin_pass(egui::RawInput {
        events: vec![egui::Event::Key {
            key: egui::Key::Backspace,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        }],
        ..Default::default()
    });
    assert!(env.handle_fullscreen_root_key_input(&ctx));
    let _ = ctx.end_pass();
    assert!(env.fullscreen_idx.is_none());
    assert_physical(&env, &book, Some(""));
}

#[test]
#[cfg(windows)]
fn section1335_native_back_to_list_caller_rejects_repeat_and_accepts_explicit_press() {
    let (mut env, parent, _) = book_env();
    let media = env.tmp.path().join("native-media");
    std::fs::create_dir_all(&media).unwrap();
    std::fs::write(media.join("clip.mp4"), b"headless media fixture").unwrap();
    assert!(env.load_folder_with_scan_owned(
        media.clone(),
        None,
        OpenRequestOwner::Navigation,
        StartupListIntent::PageContinuation
    ));
    let index = env
        .items
        .iter()
        .position(|item| matches!(item, GridItem::Video(_)))
        .unwrap();
    env.fullscreen_idx = Some(index);
    env.selected = Some(index);
    env.viewer_presentation = ViewerPresentation::Fullscreen;
    let before = env.settings.startup_list_restore.clone();
    let ctx = egui::Context::default();
    let mut key = crate::video::native_window::NativeVideoKeyEvent {
        receipt: crate::mouse_seek_debug::test_receipt(1335),
        virtual_key: 0x08,
        scan_code: 0,
        extended: false,
        shift: false,
        ctrl: false,
        alt: false,
        repeat: true,
    };
    env.handle_native_video_key_event(&ctx, index, key);
    assert_eq!(env.fullscreen_idx, Some(index));
    assert_eq!(env.settings.startup_list_restore, before);
    assert_physical(&env, &parent, None);
    key.repeat = false;
    env.handle_native_video_key_event(&ctx, index, key);
    assert!(env.fullscreen_idx.is_none());
    assert_physical(&env, &media, None);
    assert_eq!(restore_cursor(&env).unwrap().name, "clip.mp4");
}

#[test]
fn section1335_first_app_startup_migrates_missing_record_and_second_startup_keeps_new_owner() {
    for legacy_book in [false, true] {
        let mut env = phase_c_support::setup_app();
        let folder = env.tmp.path().join("released-location");
        std::fs::create_dir_all(&folder).unwrap();
        let legacy_path = if legacy_book {
            let book = folder.join("released.zip");
            write_zip(&book, &["a.png", "b.png"]);
            book
        } else {
            for name in ["a.png", "b.png"] {
                std::fs::write(folder.join(name), png_bytes()).unwrap();
            }
            folder.clone()
        };
        let new_book = env.tmp.path().join("later-direct.zip");
        write_zip(&new_book, &["later.png"]);
        env.settings.startup_folder_mode = crate::settings::StartupFolderMode::Previous;
        env.settings.auto_fullscreen_zip_pdf = true;
        env.settings.restore_last_cursor = true;
        env.settings.last_folder = Some(legacy_path.clone());
        env.settings.last_cursor_name = Some("b.png".into());
        env.settings.last_cursor_rows_above = Some(3);
        env.settings.startup_list_restore = None;
        env.settings.save();
        // Quiesce the isolated App before removing the new key, reproducing a released DB
        // rather than seeding an already migrated App's live record.
        drop(env.app);
        crate::settings_db::reset_global_for_test();
        {
            let conn = rusqlite::Connection::open(env.tmp.path().join("settings.db")).unwrap();
            conn.execute(
                "DELETE FROM settings_kv WHERE key = 'startup_list_restore'",
                [],
            )
            .unwrap();
        }
        let first_load = crate::settings::Settings::load();
        assert_eq!(
            first_load.startup_list_restore,
            Some(StartupListRestore::V1 {
                target: StartupListTarget::PhysicalList {
                    logical_path: legacy_path.clone(),
                    zip_prefix: None
                },
                cursor: Some(ListCursorHint {
                    name: "b.png".into(),
                    rows_above: Some(3)
                }),
            })
        );
        env.app = App::new_from_settings(first_load);
        // There was no preceding explicit load in this App lifetime.
        env.open_default_startup_target();
        settle_book(&mut env);
        assert_eq!(env.current_folder.as_ref(), Some(&legacy_path));
        assert!(env.fullscreen_idx.is_none());
        assert_eq!(env.items[env.selected.unwrap()].name(), "b.png");
        assert_physical(&env, &legacy_path, legacy_book.then_some(""));
        let adopted = env.settings.startup_list_restore.clone();
        open_zip(&mut env, &new_book, true);
        assert_eq!(env.settings.last_folder.as_ref(), Some(&new_book));
        assert_eq!(env.settings.startup_list_restore, adopted);
        env.on_exit_inner();
        drop(env.app);
        let second_load = crate::settings::Settings::load();
        assert_eq!(second_load.last_folder.as_ref(), Some(&new_book));
        assert_eq!(
            second_load.startup_list_restore, adopted,
            "the released compatibility path cannot remigrate over the new list owner"
        );
        env.app = App::new_from_settings(second_load);
        env.open_default_startup_target();
        settle_book(&mut env);
        assert_eq!(env.current_folder.as_ref(), Some(&legacy_path));
        assert!(env.fullscreen_idx.is_none());
        assert_eq!(env.items[env.selected.unwrap()].name(), "b.png");
        assert_physical(&env, &legacy_path, legacy_book.then_some(""));
    }
}

#[test]
fn section1335_required_virtual_zip_exact_page_success_and_missing_page_keep_parent_record() {
    for missing in [false, true] {
        let (mut env, parent, book) = book_env();
        let source = env.tmp.path().join("required-source");
        std::fs::create_dir_all(&source).unwrap();
        std::fs::write(source.join("source.png"), png_bytes()).unwrap();
        assert!(env.load_folder_with_scan_owned(
            source,
            None,
            OpenRequestOwner::Navigation,
            StartupListIntent::PageContinuation
        ));
        env.open_fullscreen(0, HistoryTrigger::UserChosen);
        let before = env.settings.startup_list_restore.clone();
        env.open_required_fullscreen_location(
            &egui::Context::default(),
            book.clone(),
            crate::snapshot::SnapshotTarget::ZipImage {
                zip_path: book.clone(),
                entry_name: if missing { "gone.png" } else { "b.png" }.into(),
            },
            HistoryTrigger::UserChosen,
        );
        assert_eq!(env.settings.startup_list_restore, before);
        env.settle_open_path_classification_for_test();
        assert_eq!(env.settings.startup_list_restore, before);
        settle_book(&mut env);
        if missing {
            assert!(
                env.fullscreen_idx.is_none(),
                "a required page must not fall back to another leaf"
            );
        } else {
            let viewed = env.fullscreen_idx.expect("the exact ZIP page must open");
            assert!(
                matches!(&env.items[viewed], GridItem::ZipImage { entry_name, .. } if entry_name == "b.png")
            );
        }
        assert_eq!(env.settings.startup_list_restore, before);
        assert_physical(&env, &parent, None);
    }
}

#[test]
fn section1335_previous_zip_missing_or_recapsed_prefix_rejects_old_same_name_cursor() {
    for collapse_changed in [false, true] {
        let (mut env, _, book) = book_env();
        // The disappearing prefix and the new root both contain the same leaf name.
        // A name-only restore would appear to work while moving the cursor to another list.
        write_zip(&book, &["a.png", "b.png", "chapter/a.png", "chapter/b.png"]);
        open_zip(&mut env, &book, false);
        env.zip_nav_enter("chapter/");
        env.selected = env.items.iter().position(|item| item.name() == "b.png");
        env.capture_main_list_restore_cursor();
        assert_eq!(restore_cursor(&env).unwrap().name, "b.png");
        env.on_exit_inner();
        drop(env.app);
        if collapse_changed {
            write_zip(
                &book,
                &[
                    "other/a.png",
                    "other/b.png",
                    "chapter/only/a.png",
                    "chapter/only/b.png",
                ],
            );
        } else {
            write_zip(&book, &["a.png", "b.png"]);
        }
        env.app = App::new_from_settings(crate::settings::Settings::load());
        env.open_default_startup_target();
        settle_book(&mut env);
        let prefix = if collapse_changed {
            "chapter/only/"
        } else {
            ""
        };
        assert_physical(&env, &book, Some(prefix));
        assert_ne!(env.select_after_load.as_deref(), Some("b.png"));
        if let Some(selected) = env.selected {
            assert_eq!(
                env.items[selected].name(),
                "a.png",
                "same leaf name from an obsolete prefix must not be applied"
            );
        }
        assert_ne!(
            restore_cursor(&env).map(|hint| hint.name.as_str()),
            Some("b.png")
        );
    }
}

#[test]
fn section1335_reading_empty_normal_folder_then_inner_zip_keeps_restore_at_each_seam() {
    let (mut env, parent, _) = book_env();
    let folder = env.tmp.path().join("dfs-container-only");
    std::fs::create_dir_all(&folder).unwrap();
    let inner = folder.join("inner.zip");
    write_zip(&inner, &["page.png"]);
    let before = env.settings.startup_list_restore.clone();
    assert!(env.load_folder_with_scan_owned(
        folder.clone(),
        None,
        OpenRequestOwner::Navigation,
        StartupListIntent::PageContinuation
    ));
    assert_eq!(env.current_folder.as_ref(), Some(&folder));
    assert!(
        env.items
            .iter()
            .all(|item| !matches!(item, GridItem::Image(_)))
    );
    assert_eq!(env.settings.startup_list_restore, before);
    let ctx = egui::Context::default();
    let mut reason = None;
    let _ = ctx.run(egui::RawInput::default(), |ctx| {
        if reason.is_none() {
            reason = Some(env.reopen_fullscreen_after_folder_nav_load(
                ctx,
                false,
                true,
                HistoryTrigger::UserChosen,
            ));
        }
    });
    assert_eq!(reason, Some("enumerate_defer"));
    assert!(
        matches!(
            env.top_level_grid_view
                .open_path_classification()
                .and_then(|request| request.continuation.as_deref()),
            Some(ClassifiedOpenContinuation::Physical {
                dfs_continuation: Some(PhysicalHistoryDfsContinuation {
                    queued_steps: 0,
                    mode: FolderNavMode::SlideshowNext,
                    history_trigger: HistoryTrigger::UserChosen,
                    restore_video_tile: false,
                    fullscreen: true,
                    resume_slideshow: true,
                }),
                restore_intent: StartupListIntent::PageContinuation,
                ..
            })
        ) || matches!(
            env.top_level_grid_view.history_navigation_transition(),
            Some(HistoryNavigationTransition::Physical(
                PhysicalHistoryTransition {
                    dfs_continuation: Some(PhysicalHistoryDfsContinuation {
                        queued_steps: 0,
                        mode: FolderNavMode::SlideshowNext,
                        history_trigger: HistoryTrigger::UserChosen,
                        restore_video_tile: false,
                        fullscreen: true,
                        resume_slideshow: true,
                    }),
                    restore_intent: StartupListIntent::PageContinuation,
                    ..
                }
            ))
        ),
        "fresh inner-book materialization must carry its reading continuation"
    );
    assert_eq!(env.settings.startup_list_restore, before);
    env.settle_open_path_classification_for_test();
    assert_eq!(env.settings.startup_list_restore, before);
    assert!(
        matches!(
            env.top_level_grid_view.history_navigation_transition(),
            Some(HistoryNavigationTransition::Physical(
                PhysicalHistoryTransition {
                    dfs_continuation: Some(PhysicalHistoryDfsContinuation {
                        queued_steps: 0,
                        mode: FolderNavMode::SlideshowNext,
                        history_trigger: HistoryTrigger::UserChosen,
                        restore_video_tile: false,
                        fullscreen: true,
                        resume_slideshow: true,
                    }),
                    restore_intent: StartupListIntent::PageContinuation,
                    ..
                }
            ))
        ),
        "classification polling must transfer the continuation to the physical preflight owner"
    );
    settle_book(&mut env);
    assert_eq!(env.current_folder.as_ref(), Some(&inner));
    assert!(
        env.fullscreen_idx.is_some(),
        "reading continuation must reach the inner book page"
    );
    assert!(env.slideshow_playing);
    assert_eq!(env.settings.startup_list_restore, before);
    assert_physical(&env, &parent, None);
}

#[test]
fn section1335_main_folder_classification_distinguishes_image_book_setting_and_mixed_folder() {
    for (mixed, auto_image) in [(false, false), (false, true), (true, false), (true, true)] {
        let (mut env, parent, _) = book_env();
        let folder = env.tmp.path().join("main-classification");
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(folder.join("page.png"), png_bytes()).unwrap();
        if mixed {
            std::fs::create_dir_all(folder.join("nested")).unwrap();
        }
        env.settings.detached_viewer_open_images_in_window = false;
        env.settings.auto_fullscreen_image_folders = auto_image;
        let before = env.settings.startup_list_restore.clone();
        assert!(matches!(
            env.load_folder_or_convert_archive_with_auto_fullscreen(folder.clone(), true),
            FolderOpenOutcome::Loaded | FolderOpenOutcome::Classifying
        ));
        settle_book(&mut env);
        assert_eq!(env.current_folder.as_ref(), Some(&folder));
        if auto_image && !mixed {
            assert!(env.fullscreen_idx.is_some());
            assert_eq!(env.settings.startup_list_restore, before);
            assert_physical(&env, &parent, None);
        } else {
            assert!(env.fullscreen_idx.is_none());
            assert_physical(&env, &folder, None);
        }
    }
}

#[test]
#[cfg(windows)]
fn section1335_pending_grid_candidate_setting_change_uses_completed_scan_main_intent() {
    for disable_image_book in [false, true] {
        let (mut env, parent, _) = book_env();
        let child = parent.join("slow-image-candidate");
        std::fs::create_dir_all(&child).unwrap();
        std::fs::write(child.join("page.png"), png_bytes()).unwrap();
        env.load_folder(parent.clone());
        env.settings.detached_viewer_open_images_in_window = true;
        env.settings.auto_fullscreen_image_folders = true;
        let index = env
            .items
            .iter()
            .position(|item| matches!(item, GridItem::Folder(path) if path == &child))
            .unwrap();
        env.selected = Some(index);
        env.capture_main_list_restore_cursor();
        let before = env.settings.startup_list_restore.clone();
        let ctx = egui::Context::default();
        assert!(env.open_grid_container_in_detached_book_context(&ctx, index));
        assert!(env.folder_pane_open_pending.is_some());
        if disable_image_book {
            env.settings.auto_fullscreen_image_folders = false;
        } else {
            env.settings.detached_viewer_open_images_in_window = false;
        }
        let ready = mutation_refresh_wait_for_order_scan(&mut env);
        let resolved = env
            .resolve_main_folder_open_ready(&ctx, ready)
            .expect("setting change rejoins main");
        assert_eq!(env.settings.startup_list_restore, before);
        assert_eq!(
            resolved.restore_intent,
            if disable_image_book {
                StartupListIntent::ExplicitList
            } else {
                StartupListIntent::PageContinuation
            }
        );
        assert!(env.load_folder_with_scan_owned(
            resolved.path,
            Some(resolved.scan),
            OpenRequestOwner::Navigation,
            resolved.restore_intent
        ));
        if disable_image_book {
            assert!(env.fullscreen_idx.is_none());
            assert_physical(&env, &child, None);
        } else {
            assert!(env.fullscreen_idx.is_some());
            assert_eq!(env.settings.startup_list_restore, before);
            assert_physical(&env, &parent, None);
        }
    }
}

#[test]
fn section1335_filtered_search_with_retained_physical_path_keeps_parent_cursor_on_back_and_tray() {
    let (mut env, parent, _) = book_env();
    env.selected = env.items.iter().position(|item| item.name() == "book.zip");
    env.capture_main_list_restore_cursor();
    let folder = env.tmp.path().join("filtered-source");
    std::fs::create_dir_all(&folder).unwrap();
    let hit = folder.join("hit.png");
    for path in [&hit, &folder.join("unmatched.png")] {
        std::fs::write(path, png_bytes()).unwrap();
    }
    assert!(env.load_folder_with_scan_owned(
        folder.clone(),
        None,
        OpenRequestOwner::Navigation,
        StartupListIntent::PageContinuation
    ));
    let before = env.settings.startup_list_restore.clone();
    env.open_global_search();
    env.global_search
        .accumulate_hit(&crate::global_search::GlobalHit {
            path: hit.to_string_lossy().into_owned(),
            score: 1.0,
            mtime: 0,
            file_size: None,
            stars: 0,
        });
    env.drill_into_container(folder.clone(), false);
    assert!(env.items_are_global_search_view);
    assert_eq!(
        env.current_folder.as_ref(),
        Some(&folder),
        "retained path does not certify a full list"
    );
    assert_eq!(
        env.items.len(),
        1,
        "the physical folder's unmatched page is absent from this drill"
    );
    env.open_fullscreen(0, HistoryTrigger::UserChosen);
    env.close_fullscreen_to_page_list();
    assert!(env.fullscreen_idx.is_none());
    assert!(env.items_are_global_search_view);
    env.capture_main_list_restore_cursor();
    env.persist_window_state_and_flush(PersistScope::ProcessKeepsRunning);
    assert_eq!(env.settings.startup_list_restore, before);
    assert_eq!(restore_cursor(&env).unwrap().name, "book.zip");
    assert_physical(&env, &parent, None);
    env.on_exit_inner();
    assert_eq!(env.settings.startup_list_restore, before);
}

#[test]
fn section1335_departing_same_items_page_open_captures_list_cursor_before_page_navigation() {
    for zip in [false, true] {
        let (mut env, _, book) = book_env();
        let folder = env.tmp.path().join("departing-list");
        let names: Vec<_> = (0..18)
            .map(|index| format!("page-{index:02}.png"))
            .collect();
        let target = if zip {
            let entries: Vec<_> = names.iter().map(String::as_str).collect();
            write_zip(&book, &entries);
            open_zip(&mut env, &book, false);
            book.clone()
        } else {
            std::fs::create_dir_all(&folder).unwrap();
            for name in &names {
                std::fs::write(folder.join(name), png_bytes()).unwrap();
            }
            env.load_folder(folder.clone());
            folder
        };
        env.last_grid_cols = 3;
        env.last_cell_h = 40.0;
        env.scroll_offset_y = 80.0;
        env.selected = Some(12);
        // No explicit capture/save is performed here: accepted grid input owns the departure.
        assert!(
            env.handle_gamepad_grid_accept(&egui::Context::default())
                .is_none()
        );
        assert_eq!(env.fullscreen_idx, Some(12));
        assert_eq!(
            restore_cursor(&env),
            Some(&ListCursorHint {
                name: names[12].clone(),
                rows_above: Some(2)
            })
        );
        let departure = env.settings.startup_list_restore.clone();
        env.open_fullscreen(16, HistoryTrigger::UserChosen);
        assert_eq!(env.fullscreen_idx, Some(16));
        assert_eq!(env.settings.startup_list_restore, departure);
        env.on_exit_inner();
        let saved = crate::settings::Settings::load();
        assert_eq!(saved.startup_list_restore, departure);
        env.app = App::new_from_settings(saved);
        env.open_default_startup_target();
        settle_book(&mut env);
        assert_eq!(env.current_folder.as_ref(), Some(&target));
        assert!(env.fullscreen_idx.is_none());
        assert_eq!(env.items[env.selected.unwrap()].name(), names[12]);
        assert_eq!(env.scroll_selected_to_rows_above, Some(2));
    }
}

#[test]
fn section1335_hidden_same_book_reload_keeps_cursor_until_exit_from_its_settled_grid() {
    let (mut env, _, book) = book_env();
    open_zip(&mut env, &book, false);
    env.selected = Some(1);
    env.capture_main_list_restore_cursor();
    let departure = env.settings.startup_list_restore.clone();
    assert_eq!(restore_cursor(&env).unwrap().name, "b.png");

    // The same physical target is reopened for reading, without a presented grid acceptance.
    open_zip(&mut env, &book, true);
    assert!(env.fullscreen_idx.is_some());
    assert_eq!(env.settings.startup_list_restore, departure);
    env.selected = Some(0);
    env.open_fullscreen(0, HistoryTrigger::UserChosen);
    assert_eq!(env.fullscreen_idx, Some(0));
    assert_eq!(env.settings.startup_list_restore, departure);

    env.reload_current_folder_preserving_override();
    settle_book(&mut env);
    assert_eq!(env.current_folder.as_ref(), Some(&book));
    assert!(
        env.fullscreen_idx.is_none(),
        "settled reload exposes the book grid"
    );
    assert_eq!(env.items[env.selected.unwrap()].name(), "a.png");
    assert_eq!(env.settings.startup_list_restore, departure);
    env.on_exit_inner();
    assert_physical(&env, &book, Some(""));
    assert_eq!(
        restore_cursor(&env),
        Some(&ListCursorHint {
            name: "a.png".into(),
            rows_above: Some(0)
        })
    );
}

#[test]
fn section1335_direct_child_reload_exposes_grid_but_quit_keeps_parent_target_and_cursor() {
    let (mut env, parent, book) = book_env();
    env.selected = env
        .items
        .iter()
        .position(|item| matches!(item, GridItem::ZipFile(path) if path == &book));
    let nav = env
        .handle_gamepad_grid_accept(&egui::Context::default())
        .expect("accepted main grid container yields its owned open");
    let departure = env.settings.startup_list_restore.clone();
    assert_physical(&env, &parent, None);
    assert_eq!(restore_cursor(&env).unwrap().name, "book.zip");
    assert!(env.apply_fullscreen_close_nav_immediate(nav));
    settle_book(&mut env);
    assert_eq!(env.current_folder.as_ref(), Some(&book));
    assert!(env.fullscreen_idx.is_some());
    assert_eq!(env.settings.startup_list_restore, departure);
    env.open_fullscreen(1, HistoryTrigger::UserChosen);
    env.reload_current_folder_preserving_override();
    settle_book(&mut env);
    assert_eq!(env.current_folder.as_ref(), Some(&book));
    assert!(env.fullscreen_idx.is_none());
    assert_eq!(env.settings.startup_list_restore, departure);
    env.on_exit_inner();
    assert_eq!(env.settings.startup_list_restore, departure);
    assert_physical(&env, &parent, None);
}
#[test]
fn section1335_synthetic_install_captures_changed_physical_departure_without_manual_capture() {
    for subfolder in [false, true] {
        let (mut env, _, _) = book_env();
        let folder = env.tmp.path().join("physical-departure");
        let child = folder.join("child");
        std::fs::create_dir_all(&child).unwrap();
        std::fs::write(child.join("aggregate.png"), png_bytes()).unwrap();
        for index in 0..18 {
            std::fs::write(folder.join(format!("page-{index:02}.png")), png_bytes()).unwrap();
        }
        env.load_folder(folder.clone());
        let selected = env
            .items
            .iter()
            .position(|item| item.name() == "page-12.png")
            .unwrap();
        env.selected = Some(selected);
        env.last_grid_cols = 3;
        env.last_cell_h = 40.0;
        env.scroll_offset_y = 80.0;
        let expected = Some(StartupListRestore::V1 {
            target: StartupListTarget::PhysicalList {
                logical_path: folder.clone(),
                zip_prefix: None,
            },
            cursor: Some(ListCursorHint {
                name: "page-12.png".into(),
                rows_above: Some(2),
            }),
        });
        assert_ne!(env.settings.startup_list_restore, expected);
        // The synthetic install must capture the departing physical grid itself.
        if subfolder {
            env.start_subfolder_expansion_scan_roots(folder.clone(), vec![child]);
            let ctx = egui::Context::default();
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
            while !env.items_are_subfolder_expansion_view {
                env.poll_subfolder_expansion(&ctx);
                assert!(
                    std::time::Instant::now() < deadline,
                    "subfolder prepare did not install"
                );
                std::thread::yield_now();
            }
        } else {
            env.enter_reading_history();
            assert!(env.items_are_reading_history_view);
        }
        assert_eq!(env.settings.startup_list_restore, expected);
        env.on_exit_inner();
        drop(env.app);
        let saved = crate::settings::Settings::load();
        assert_eq!(saved.startup_list_restore, expected);
        env.app = App::new_from_settings(saved);
        env.open_default_startup_target();
        settle_book(&mut env);
        assert_eq!(env.current_folder.as_ref(), Some(&folder));
        assert!(env.fullscreen_idx.is_none());
        assert_eq!(env.items[env.selected.unwrap()].name(), "page-12.png");
        assert_eq!(env.scroll_selected_to_rows_above, Some(2));
    }
}

#[test]
fn section1335_startup_cursor_rows_survive_grid_page_acceptance_before_first_scroll_layout() {
    let (mut env, _, book) = book_env();
    let names: Vec<_> = (0..18)
        .map(|index| format!("page-{index:02}.png"))
        .collect();
    let entries: Vec<_> = names.iter().map(String::as_str).collect();
    write_zip(&book, &entries);
    let expected = Some(StartupListRestore::V1 {
        target: StartupListTarget::PhysicalList {
            logical_path: book.clone(),
            zip_prefix: Some("".into()),
        },
        cursor: Some(ListCursorHint {
            name: names[12].clone(),
            rows_above: Some(2),
        }),
    });
    env.settings.startup_list_restore = expected.clone();
    env.settings.save();
    drop(env.app);
    env.app = App::new_from_settings(crate::settings::Settings::load());
    env.open_default_startup_target();
    settle_book(&mut env);
    assert_eq!(env.items[env.selected.unwrap()].name(), names[12]);
    assert_eq!(env.scroll_selected_to_rows_above, Some(2));
    // No UI frame has measured this restored list's final scroll geometry.
    assert!(
        env.handle_gamepad_grid_accept(&egui::Context::default())
            .is_none()
    );
    assert_eq!(env.fullscreen_idx, Some(12));
    assert_eq!(env.settings.startup_list_restore, expected);
    env.open_fullscreen(16, HistoryTrigger::UserChosen);
    assert_eq!(env.fullscreen_idx, Some(16));
    assert_eq!(env.settings.startup_list_restore, expected);
    env.on_exit_inner();
    drop(env.app);
    let saved = crate::settings::Settings::load();
    assert_eq!(saved.startup_list_restore, expected);
    env.app = App::new_from_settings(saved);
    env.open_default_startup_target();
    settle_book(&mut env);
    assert!(env.fullscreen_idx.is_none());
    assert_eq!(env.items[env.selected.unwrap()].name(), names[12]);
    assert_eq!(env.scroll_selected_to_rows_above, Some(2));
}

#[test]
#[cfg(windows)]
fn section1335_similar_exact_page_move_preserves_accepted_departure_before_physical_materialization()
 {
    let (mut env, _, _) = book_env();
    let source = env.tmp.path().join("similar-source");
    let target = env.tmp.path().join("similar-target");
    std::fs::create_dir_all(&source).unwrap();
    std::fs::create_dir_all(&target).unwrap();
    for index in 0..18 {
        std::fs::write(source.join(format!("page-{index:02}.png")), png_bytes()).unwrap();
    }
    let requested = target.join("requested.png");
    std::fs::write(&requested, png_bytes()).unwrap();
    std::fs::write(target.join("other.png"), png_bytes()).unwrap();
    env.load_folder(source.clone());
    env.selected = Some(12);
    env.last_grid_cols = 3;
    env.last_cell_h = 40.0;
    env.scroll_offset_y = 80.0;
    let expected = Some(StartupListRestore::V1 {
        target: StartupListTarget::PhysicalList {
            logical_path: source.clone(),
            zip_prefix: None,
        },
        cursor: Some(ListCursorHint {
            name: "page-12.png".into(),
            rows_above: Some(2),
        }),
    });
    assert_ne!(env.settings.startup_list_restore, expected);
    assert!(
        env.handle_gamepad_grid_accept(&egui::Context::default())
            .is_none()
    );
    assert_eq!(env.fullscreen_idx, Some(12));
    assert_eq!(env.settings.startup_list_restore, expected);
    let hit = crate::similar_index::QueryHit {
        item_id: 1,
        item_key: crate::similar_index::item_key_for_file(&requested),
        kind: crate::similar_db::ItemKind::Image,
        container_key: None,
        page_index: None,
        distance: 1,
        band: crate::similar_index::MatchBand::NearlyIdentical,
        mtime: 1,
        file_size: 1,
        width: 2,
        height: 2,
        format: crate::similar_image::SimilarImageFormat::Png,
        target: Some(crate::similar_index::SimilarItemTarget::File(
            requested.clone(),
        )),
    };
    let ctx = egui::Context::default();
    env.open_similar_hit_for_test(&ctx, &hit);
    let ready = mutation_refresh_wait_for_order_scan(&mut env);
    assert!(env.resolve_main_folder_open_ready(&ctx, ready).is_none());
    assert_eq!(env.current_folder.as_ref(), Some(&target));
    assert!(
        matches!(env.fullscreen_idx.and_then(|index| env.items.get(index)), Some(GridItem::Image(path)) if path == &requested)
    );
    assert_eq!(env.settings.startup_list_restore, expected);
    env.on_exit_inner();
    assert_eq!(env.settings.startup_list_restore, expected);
}

#[test]
fn section1335_favorite_reading_sibling_fallbacks_preserve_last_accepted_physical_cursor() {
    for (outside_subtree, zip_sibling) in
        [(false, false), (false, true), (true, false), (true, true)]
    {
        let (mut env, parent, old_book) = book_env();
        let departing_page = parent.join("departing.png");
        std::fs::write(&departing_page, png_bytes()).unwrap();
        env.load_folder(parent.clone());
        env.selected = env
            .items
            .iter()
            .position(|item| matches!(item, GridItem::Image(path) if path == &departing_page));
        assert!(
            env.handle_gamepad_grid_accept(&egui::Context::default())
                .is_none()
        );
        let departure = env.settings.startup_list_restore.clone();
        assert_eq!(restore_cursor(&env).unwrap().name, "departing.png");
        open_zip(&mut env, &old_book, true);
        assert!(env.fullscreen_idx.is_some());
        assert_eq!(env.settings.startup_list_restore, departure);

        let next = if zip_sibling {
            let path = env.tmp.path().join("favorite-next.zip");
            write_zip(&path, &["next.png"]);
            path
        } else {
            let path = env.tmp.path().join("favorite-next-folder");
            std::fs::create_dir_all(&path).unwrap();
            std::fs::write(path.join("next.png"), png_bytes()).unwrap();
            path
        };
        let outside = env.tmp.path().join("outside-favorite-subtree");
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(outside.join("outside.png"), png_bytes()).unwrap();
        // These physical result identities belong to a drilled favorite search.
        env.favsearch.active = true;
        env.favsearch.saved_folder = Some(parent.clone());
        env.favsearch.results_paths = vec![old_book.clone(), next.clone()];
        env.favsearch.nav_stack = vec![old_book.clone()];
        let mut result = Some(FolderNavResult {
            target: outside_subtree.then(|| FolderNavTarget {
                route: FolderNavRoute::FullFeature(outside),
                smart_kind: None,
                scanned: FolderNavScanResult::NotNeeded,
            }),
            hit_image_folder: outside_subtree,
            forward: true,
            mode: FolderNavMode::Favsearch {
                root: old_book,
                fullscreen: true,
            },
            queued_steps: 0,
        });
        let ctx = egui::Context::default();
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            if let Some(result) = result.take() {
                env.apply_folder_nav_result(ctx, result);
            }
        });
        assert_eq!(env.settings.startup_list_restore, departure);
        settle_book(&mut env);
        assert_eq!(env.current_folder.as_ref(), Some(&next));
        assert_eq!(env.favsearch.nav_stack, vec![next]);
        let viewed = env
            .fullscreen_idx
            .expect("favorite sibling must continue reading");
        assert_eq!(env.items[viewed].name(), "next.png");
        assert_eq!(env.settings.startup_list_restore, departure);
        env.on_exit_inner();
        assert_eq!(env.settings.startup_list_restore, departure);
        assert_physical(&env, &parent, None);
    }
}
#[test]
#[cfg(windows)]
fn section1335_collection_root_return_from_page_b_retains_departing_list_page_a() {
    let (mut env, _, _) = book_env();
    let child = env.tmp.path().join("collection-cursor-source");
    std::fs::create_dir_all(&child).unwrap();
    for name in ["a.png", "b.png"] {
        std::fs::write(child.join(name), png_bytes()).unwrap();
    }
    install_collection_item_for_detached_plan(
        &mut env,
        &child,
        crate::collection_store::CollectionResolvedKind::Folder,
    );
    let owner = env.collection_grid_physical_load_owner(0, &child).unwrap();
    assert!(env.load_folder_with_scan_owned(
        child.clone(),
        None,
        OpenRequestOwner::CollectionGridPhysical(owner),
        StartupListIntent::ExplicitList,
    ));
    env.selected = env.items.iter().position(|item| item.name() == "a.png");
    assert!(
        env.handle_gamepad_grid_accept(&egui::Context::default())
            .is_none()
    );
    let departure = env.settings.startup_list_restore.clone();
    assert_eq!(restore_cursor(&env).unwrap().name, "a.png");
    let second = env
        .items
        .iter()
        .position(|item| item.name() == "b.png")
        .unwrap();
    env.open_fullscreen(second, HistoryTrigger::UserChosen);
    assert_eq!(env.fullscreen_idx, Some(second));
    assert_eq!(env.settings.startup_list_restore, departure);

    let nav = env
        .collection_grid_parent_nav()
        .expect("physical child owns root return");
    assert!(env.apply_fullscreen_close_nav_immediate(nav));
    assert!(env.fullscreen_idx.is_none());
    assert!(matches!(
        env.top_level_grid_view.surface(),
        crate::app::top_level_grid_view::TopLevelGridSurface::Collection(_)
    ));
    // Root loading is the accepted synthetic adoption; close must not capture its old page B.
    assert_eq!(env.settings.startup_list_restore, departure);
    assert_physical(&env, &child, None);
    env.on_exit_inner();
    assert_eq!(env.settings.startup_list_restore, departure);
}

#[test]
fn section1335_cached_smart_root_return_captures_changed_child_cursor_without_manual_capture() {
    let (mut env, _, _) = book_env();
    let source = env.tmp.path().join("smart-cursor-source");
    let child = source.join("child");
    std::fs::create_dir_all(&child).unwrap();
    for index in 0..18 {
        std::fs::write(child.join(format!("page-{index:02}.png")), png_bytes()).unwrap();
    }
    let mut definition = crate::settings::SmartFolderDefinition::new("Cached root cursor");
    definition.rules.push(crate::settings::SmartFolderRule::new(
        source,
        true,
        Default::default(),
    ));
    let id = definition.id;
    env.settings.smart_folders = vec![definition];
    env.open_smart_folder(id, false);
    wait_smart(&mut env);
    let kind = env.smart_physical_target_kind(&child).unwrap();
    assert!(
        env.begin_smart_physical_navigation(
            child.clone(),
            kind,
            false,
            None,
            None,
            StartupListIntent::ExplicitList
        )
        .is_ok()
    );
    wait_smart(&mut env);
    assert_physical(&env, &child, None);
    env.selected = env
        .items
        .iter()
        .position(|item| item.name() == "page-12.png");
    env.last_grid_cols = 3;
    env.last_cell_h = 40.0;
    env.scroll_offset_y = 80.0;
    let expected = Some(StartupListRestore::V1 {
        target: StartupListTarget::PhysicalList {
            logical_path: child.clone(),
            zip_prefix: None,
        },
        cursor: Some(ListCursorHint {
            name: "page-12.png".into(),
            rows_above: Some(2),
        }),
    });
    assert_ne!(env.settings.startup_list_restore, expected);
    let synthetic = crate::app::smart_folder::smart_folder_synthetic_path(id);
    assert!(env.restore_smart_folder_for_synthetic_path(&synthetic));
    assert!(
        env.smart_folder_transition.is_none(),
        "unchanged root reuses its parked presentation"
    );
    assert!(env.smart_folder_pending.is_none());
    assert!(env.smart_folder_prepare_pending.is_none());
    wait_smart(&mut env);
    assert!(env.items_are_smart_folder_view);
    assert_eq!(env.settings.startup_list_restore, expected);
    env.on_exit_inner();
    assert_eq!(env.settings.startup_list_restore, expected);
}

#[test]
fn section1335_explicit_page_fullscreen_from_smart_child_captures_departure_before_page_loader() {
    let (mut env, _, _) = book_env();
    let source = env.tmp.path().join("smart-explicit-page-source");
    let child = source.join("child");
    std::fs::create_dir_all(&child).unwrap();
    let book = child.join("inner.zip");
    write_zip(&book, &["a.png", "b.png"]);
    write_zip(&child.join("a-other.zip"), &["other.png"]);
    let mut definition = crate::settings::SmartFolderDefinition::new("Explicit page cursor");
    definition.rules.push(crate::settings::SmartFolderRule::new(
        source,
        true,
        Default::default(),
    ));
    let id = definition.id;
    env.settings.smart_folders = vec![definition];
    env.settings.detached_viewer_open_images_in_window = false;
    env.open_smart_folder(id, false);
    wait_smart(&mut env);
    let kind = env.smart_physical_target_kind(&child).unwrap();
    assert!(
        env.begin_smart_physical_navigation(
            child.clone(),
            kind,
            false,
            None,
            None,
            StartupListIntent::ExplicitList
        )
        .is_ok()
    );
    wait_smart(&mut env);
    let index = env
        .items
        .iter()
        .position(|item| matches!(item, GridItem::ZipFile(path) if path == &book))
        .unwrap();
    env.selected = Some(index);
    let expected = Some(StartupListRestore::V1 {
        target: StartupListTarget::PhysicalList {
            logical_path: child.clone(),
            zip_prefix: None,
        },
        cursor: Some(ListCursorHint {
            name: "inner.zip".into(),
            rows_above: Some(0),
        }),
    });
    assert_ne!(env.settings.startup_list_restore, expected);
    let nav = env.open_grid_container_with_mode(
        &egui::Context::default(),
        index,
        GridContainerOpenMode::PageFullscreen,
        "section1335",
    );
    assert!(
        nav.is_none(),
        "Smart owns the accepted physical child request"
    );
    assert_eq!(env.settings.startup_list_restore, expected);
    wait_smart(&mut env);
    settle_book(&mut env);
    assert_eq!(env.effective_folder().as_ref(), Some(&book));
    assert!(env.fullscreen_idx.is_some());
    assert_eq!(env.settings.startup_list_restore, expected);
    env.open_fullscreen(1, HistoryTrigger::UserChosen);
    env.on_exit_inner();
    assert_eq!(env.settings.startup_list_restore, expected);
}
#[test]
fn section1335_bookshelf_compiled_folder_direct_read_and_explicit_list_restart_differ() {
    for direct in [false, true] {
        let mut env = phase_c_support::setup_app();
        let root = env.tmp.path().join("bookshelf-restore");
        let child = root.join("Compiled");
        let nested = child.join("nested");
        std::fs::create_dir_all(&nested).unwrap();
        for name in ["a.png", "b.png"] {
            std::fs::write(child.join(name), png_bytes()).unwrap();
        }
        std::fs::write(nested.join("nested.png"), png_bytes()).unwrap();
        env.settings.book_root = Some(root.clone());
        env.settings.startup_folder_mode = crate::settings::StartupFolderMode::Previous;
        env.settings.restore_last_cursor = true;
        env.settings.detached_viewer_open_images_in_window = false;
        env.settings.auto_fullscreen_zip_pdf = true;
        env.settings.auto_fullscreen_image_folders = direct;
        env.open_books_root();
        assert_eq!(env.current_folder.as_ref(), Some(&root));
        assert!(env.fullscreen_idx.is_none());
        assert_physical(&env, &root, None);
        let index = env
            .items
            .iter()
            .position(|item| matches!(item, GridItem::Folder(path) if path == &child))
            .unwrap();
        env.selected = Some(index);
        let Some(crate::ui_main::AddressBarNav::Direct(path, restore_intent)) =
            env.handle_gamepad_grid_accept(&egui::Context::default())
        else {
            panic!("compiled Folder uses ordinary accepted main-grid direct navigation");
        };
        let root_departure = env.settings.startup_list_restore.clone();
        assert_eq!(restore_cursor(&env).unwrap().name, "Compiled");
        assert_eq!(env.pending_auto_fs_open, direct);
        // Use the same accepted Folder dispatch as the main input tail. Explicit archive-only
        // PageList/PageFullscreen commands do not accept Folder rows.
        env.open_direct_navigation_target(
            path,
            None,
            OpenRequestOwner::Navigation,
            None,
            None,
            restore_intent,
        );
        settle_book(&mut env);
        assert_eq!(env.current_folder.as_ref(), Some(&child));
        assert!(
            nested.is_dir(),
            "compiled classification must see the real nested directory"
        );
        if direct {
            // A direct child of book_root is a compiled book despite its nested directory.
            assert!(env.fullscreen_idx.is_some());
            assert_eq!(env.settings.startup_list_restore, root_departure);
        } else {
            assert!(env.fullscreen_idx.is_none());
            assert_physical(&env, &child, None);
            env.selected = env.items.iter().position(|item| item.name() == "a.png");
            assert!(
                env.handle_gamepad_grid_accept(&egui::Context::default())
                    .is_none()
            );
            assert_eq!(restore_cursor(&env).unwrap().name, "a.png");
        }
        let departure = env.settings.startup_list_restore.clone();
        let second = env
            .items
            .iter()
            .position(|item| item.name() == "b.png")
            .unwrap();
        env.open_fullscreen(second, HistoryTrigger::UserChosen);
        env.on_exit_inner();
        assert_eq!(env.settings.startup_list_restore, departure);
        drop(env.app);
        let saved = crate::settings::Settings::load();
        assert_eq!(saved.startup_list_restore, departure);
        env.app = App::new_from_settings(saved);
        env.open_default_startup_target();
        settle_book(&mut env);
        let expected = if direct { &root } else { &child };
        assert_eq!(env.current_folder.as_ref(), Some(expected));
        assert!(env.fullscreen_idx.is_none());
        assert_physical(&env, expected, None);
        assert_eq!(
            env.items[env.selected.unwrap()].name(),
            if direct { "Compiled" } else { "a.png" }
        );
    }
}

#[test]
#[cfg(windows)]
fn section1335_collection_and_rating_folder_carriers_keep_owner_and_open_intent() {
    for collection in [false, true] {
        let (mut env, _, _) = book_env();
        let child = env.tmp.path().join("typed-source-folder");
        std::fs::create_dir_all(&child).unwrap();
        std::fs::write(child.join("page.png"), png_bytes()).unwrap();
        if collection {
            install_collection_item_for_detached_plan(
                &mut env,
                &child,
                crate::collection_store::CollectionResolvedKind::Folder,
            );
        } else {
            env.rating_db
                .as_ref()
                .unwrap()
                .set(&crate::adjustment_db::normalize_path(&child), 1)
                .unwrap();
            env.enter_rating_view_from_menu(1);
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
            while env.rating_view_pending.is_some() {
                env.poll_rating_view();
                assert!(
                    std::time::Instant::now() < deadline,
                    "actual rated Folder row did not load"
                );
                std::thread::yield_now();
            }
        }
        let index = env
            .items
            .iter()
            .position(|item| matches!(item, GridItem::Folder(path) if path == &child))
            .unwrap();
        env.selected = Some(index);
        let before = env.settings.startup_list_restore.clone();
        let generation = env.items_generation;
        // The common UI producer carries policy independently of owner. Producing either
        // request is read-only; the existing winning consumer retains execution ownership.
        for auto in [true, false] {
            let nav = env.grid_physical_navigation(index, child.clone(), auto);
            let expected = StartupListIntent::container_open(auto);
            match &nav {
                crate::ui_main::AddressBarNav::CollectionSource {
                    path,
                    owner,
                    restore_intent,
                } => {
                    assert!(collection);
                    assert_eq!(path, &child);
                    assert!(env.collection_grid_physical_load_owner_is_current(owner, path));
                    assert_eq!(restore_intent, &expected);
                }
                crate::ui_main::AddressBarNav::RatingSource {
                    path,
                    owner,
                    restore_intent,
                } => {
                    assert!(!collection);
                    assert_eq!(path, &child);
                    assert!(env.rating_physical_load_owner_is_current(owner, path));
                    assert_eq!(restore_intent, &expected);
                }
                _ => panic!("physical Folder retains its Collection or Rating owner"),
            }
            assert_eq!(env.items_generation, generation);
            assert_eq!(env.settings.startup_list_restore, before);
            if !auto {
                assert!(env.apply_fullscreen_close_nav_immediate(nav));
                settle_book(&mut env);
                assert_eq!(env.effective_folder().as_ref(), Some(&child));
                assert!(env.fullscreen_idx.is_none());
                assert_physical(&env, &child, None);
            }
        }
    }
}

#[test]
fn section1335_rating_folder_auto_request_records_the_actual_settled_physical_list() {
    let (mut env, parent, _) = book_env();
    let child = env.tmp.path().join("rating-auto-folder");
    std::fs::create_dir_all(&child).unwrap();
    std::fs::write(child.join("page.png"), png_bytes()).unwrap();
    env.settings.auto_fullscreen_zip_pdf = true;
    env.settings.auto_fullscreen_image_folders = true;
    env.settings.detached_viewer_open_images_in_window = false;
    env.rating_db
        .as_ref()
        .unwrap()
        .set(&crate::adjustment_db::normalize_path(&child), 1)
        .unwrap();
    env.enter_rating_view_from_menu(1);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while env.rating_view_pending.is_some() {
        env.poll_rating_view();
        assert!(
            std::time::Instant::now() < deadline,
            "actual rated Folder row did not load"
        );
        std::thread::yield_now();
    }
    let index = env
        .items
        .iter()
        .position(|item| matches!(item, GridItem::Folder(path) if path == &child))
        .unwrap();
    env.selected = Some(index);
    let before = env.settings.startup_list_restore.clone();
    assert_physical(&env, &parent, None);
    // Accepted gamepad Folder input establishes the same auto-open reservation as the UI.
    // Its released Direct owner is separate from the UI's typed Rating owner below.
    assert!(
        env.handle_gamepad_grid_accept(&egui::Context::default())
            .is_some()
    );
    assert!(env.pending_auto_fs_open);
    let nav = env.grid_physical_navigation(index, child.clone(), true);
    assert!(matches!(
        &nav,
        crate::ui_main::AddressBarNav::RatingSource {
            restore_intent: StartupListIntent::ClassifyFolder,
            ..
        }
    ));
    assert!(env.apply_fullscreen_close_nav_immediate(nav));
    assert_eq!(env.settings.startup_list_restore, before);
    assert_physical(&env, &parent, None);
    settle_book(&mut env);
    assert_eq!(env.effective_folder().as_ref(), Some(&child));
    assert!(
        env.fullscreen_idx.is_none(),
        "released Rating Folder execution settles as a list"
    );
    assert_physical(&env, &child, None);
    let adopted = env.settings.startup_list_restore.clone();
    env.on_exit_inner();
    assert_eq!(env.settings.startup_list_restore, adopted);
}

#[test]
#[cfg(windows)]
fn section1335_collection_ctrl_down_zip_adoption_restores_with_sidecars_on_and_off() {
    use crate::app::top_level_grid_view::{CollectionGridLoadState, CollectionGridPosition};
    use crate::collection_store::{
        CollectionRegistration, CollectionResolvedKind, CollectionStoreRuntime,
    };

    for sidecars in [false, true] {
        let (mut env, parent, book) = book_env();
        env.settings.sidecar_backup_enabled = sidecars;
        env.settings.tag_sidecar_backup_enabled = sidecars;
        let first = env.tmp.path().join("collection-first");
        std::fs::create_dir_all(&first).unwrap();
        std::fs::write(first.join("first.png"), png_bytes()).unwrap();
        let runtime = CollectionStoreRuntime::start_at(env.tmp.path().join("collection.db"))
            .expect("isolated collection runtime");
        let client = runtime.client();
        env.install_collection_runtime(runtime);
        let ctx = egui::Context::default();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while !matches!(env.collection_store_client_for_read(), Ok(Some(_))) {
            env.poll_collection_ui(&ctx);
            assert!(
                std::time::Instant::now() < deadline,
                "collection actor not ready"
            );
            std::thread::yield_now();
        }
        let created = client
            .create_collection("Startup outer navigation".into())
            .unwrap()
            .recv_timeout(std::time::Duration::from_secs(3))
            .unwrap()
            .unwrap();
        client
            .add_batch(
                created.collection_id(),
                created.revision(),
                vec![
                    CollectionRegistration::from_trusted_path(
                        &first,
                        CollectionResolvedKind::Folder,
                    )
                    .unwrap(),
                    CollectionRegistration::from_trusted_path(&book, CollectionResolvedKind::Zip)
                        .unwrap(),
                ],
            )
            .unwrap()
            .recv_timeout(std::time::Duration::from_secs(3))
            .unwrap()
            .unwrap();
        env.open_collection_grid(created.collection_id(), None);
        while !env
            .top_level_grid_view
            .collection_session()
            .is_some_and(|session| {
                matches!(session.load, CollectionGridLoadState::Ready(_))
                    && session.installed_items_generation == Some(env.items_generation)
            })
        {
            env.poll_collection_ui(&ctx);
            env.poll_collection_grid(&ctx);
            assert!(
                std::time::Instant::now() < deadline,
                "collection root not ready"
            );
            std::thread::yield_now();
        }
        assert_eq!(env.items.len(), 2);
        env.selected = Some(
            env.items
                .iter()
                .position(|item| item.container_path() == Some(first.as_path()))
                .unwrap(),
        );
        assert_physical(&env, &parent, None);

        // Use the actual grid Ctrl+Down handler, including its Collection OuterGrid request.
        ctx.begin_pass(egui::RawInput {
            modifiers: egui::Modifiers::CTRL,
            events: vec![egui::Event::Key {
                key: egui::Key::ArrowDown,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::CTRL,
            }],
            ..Default::default()
        });
        assert!(env.handle_keyboard(&ctx).is_none());
        let _ = ctx.end_pass();
        assert!(env.top_level_grid_view.collection_navigation_pending());
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            env.poll_collection_ui(&ctx);
            env.poll_collection_grid(&ctx);
            env.poll_collection_navigation(&ctx);
            env.poll_sidecar_restore(&ctx);
            // App::update performs this frame poll after an adopted grid releases its holdover.
            env.poll_fs_nav_lock(&ctx);
            let adopted = env
                .top_level_grid_view
                .collection_session()
                .is_some_and(|session| {
                    matches!(&session.position, CollectionGridPosition::PhysicalSource { path, .. }
                    if path == &book)
                });
            if adopted
                && !env.top_level_grid_view.collection_navigation_pending()
                && !env.sidecar_restore_active()
            {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "Collection ZIP navigation did not settle"
            );
            std::thread::yield_now();
        }
        assert_eq!(env.current_folder.as_ref(), Some(&book));
        assert!(env.zip_nav.is_some());
        assert!(env.fullscreen_idx.is_none());
        assert!(env.fs_nav_locked_gen.is_none());
        assert_physical(&env, &book, Some(""));
        env.selected = Some(
            env.items
                .iter()
                .position(|item| item.name() == "b.png")
                .unwrap(),
        );
        assert_eq!(env.items[env.selected.unwrap()].name(), "b.png");
        env.on_exit_inner();
        assert_eq!(
            restore_cursor(&env).unwrap().name,
            "b.png",
            "the departing Collection physical list must persist its current selection"
        );
        let saved = crate::settings::Settings::load();
        assert!(matches!(
            saved.startup_list_restore,
            Some(StartupListRestore::V1 { cursor: Some(ListCursorHint { ref name, .. }), .. })
                if name == "b.png"
        ));
        restart_previous_from_saved_settings(&mut env);
        assert_physical(&env, &book, Some(""));
        assert_eq!(restore_cursor(&env).unwrap().name, "b.png");
        assert_eq!(env.items[env.selected.unwrap()].name(), "b.png");
        assert!(env.fullscreen_idx.is_none());
    }
}

#[test]
#[cfg(windows)]
fn section1335_video_ring_close_accepts_current_folder_list_for_exit_and_restart() {
    use crate::ring_shortcut::{RingActionId, RingShortcutContext};

    let (mut env, parent, _) = book_env();
    let media_folder = env.tmp.path().join("ring-direct-video");
    std::fs::create_dir_all(&media_folder).unwrap();
    let path = media_folder.join("clip.mp4");
    std::fs::write(&path, b"headless media fixture").unwrap();
    assert!(env.load_folder_with_scan_owned(
        media_folder.clone(),
        None,
        OpenRequestOwner::Navigation,
        StartupListIntent::PageContinuation,
    ));
    let index = env
        .items
        .iter()
        .position(|item| matches!(item, GridItem::Video(_)))
        .unwrap();
    // Supply the existing disconnected media harness; the ring handler owns the close.
    env.fs_cache.insert(
        index,
        FsCacheEntry::Video {
            player: Box::new(crate::video::VideoPlayer::disconnected_for_test(path, 12.0)),
            load_seq: 0,
        },
    );
    env.fullscreen_idx = Some(index);
    env.selected = Some(index);
    env.viewer_presentation = ViewerPresentation::Fullscreen;
    let before = env.settings.startup_list_restore.clone();
    assert_physical(&env, &parent, None);
    let ctx = egui::Context::default();

    // CloseFullscreen has no grid-context acceptance; an unrelated route must not commit.
    assert!(
        env.apply_ring_action(
            &ctx,
            RingShortcutContext::Grid,
            RingActionId::CloseFullscreen,
            "section1335",
        )
        .is_none()
    );
    assert_eq!(env.fullscreen_idx, Some(index));
    assert_eq!(env.settings.startup_list_restore, before);

    assert!(
        env.apply_ring_action(
            &ctx,
            RingShortcutContext::VideoFullscreen,
            RingActionId::CloseFullscreen,
            "section1335",
        )
        .is_none()
    );
    assert!(env.fullscreen_idx.is_none());
    assert_eq!(env.current_folder.as_ref(), Some(&media_folder));
    assert_physical(&env, &media_folder, None);
    assert_eq!(restore_cursor(&env).unwrap().name, "clip.mp4");
    restart_previous(&mut env);
    assert_physical(&env, &media_folder, None);
    assert_eq!(env.current_folder.as_ref(), Some(&media_folder));
    assert_eq!(env.items[env.selected.unwrap()].name(), "clip.mp4");
    assert_eq!(restore_cursor(&env).unwrap().name, "clip.mp4");
    assert!(env.fullscreen_idx.is_none());
}

#[cfg(windows)]
fn extra_video_close_env() -> (phase_c_support::AppTestEnv, PathBuf, PathBuf, usize) {
    let (mut env, parent, _) = book_env();
    let media_folder = env.tmp.path().join("extra-direct-video");
    std::fs::create_dir_all(&media_folder).unwrap();
    let path = media_folder.join("clip.mp4");
    std::fs::write(&path, b"headless media fixture").unwrap();
    assert!(env.load_folder_with_scan_owned(
        media_folder.clone(),
        None,
        OpenRequestOwner::Navigation,
        StartupListIntent::PageContinuation,
    ));
    let index = env
        .items
        .iter()
        .position(|item| matches!(item, GridItem::Video(_)))
        .unwrap();
    env.fs_cache.insert(
        index,
        FsCacheEntry::Video {
            player: Box::new(crate::video::VideoPlayer::disconnected_for_test(path, 12.0)),
            load_seq: 0,
        },
    );
    env.fullscreen_idx = Some(index);
    env.selected = Some(index);
    env.viewer_presentation = ViewerPresentation::Fullscreen;
    assert_physical(&env, &parent, None);
    (env, parent, media_folder, index)
}

#[cfg(windows)]
fn install_extra_video_source_swap(
    app: &mut App,
    index: usize,
    parked_live_window_id: Option<u64>,
) -> crate::video::NativeUiProbeForTest {
    let FsCacheEntry::Video { player, .. } = app.fs_cache.get_mut(&index).unwrap() else {
        unreachable!()
    };
    let probe = player.install_native_ui_probe_for_test();
    let native_output = player.take_native_output().unwrap();
    native_output.bump_committed_generation(5);
    let GridItem::Video(path) = &app.items[index] else {
        unreachable!()
    };
    let now = std::time::Instant::now();
    app.native_video_source_swap_pending = Some(native_video::NativeVideoSourceSwapPending {
        from_idx: index,
        target_idx: index,
        target_path: path.clone(),
        native_output,
        autoplay_override: None,
        ignore_resume: false,
        show_preparing_overlay: false,
        reason: "navigation",
        // Hold the existing debounce seam open so rejected events can be polled without
        // opening a real decoder. Accepted closes return before this seam is consulted.
        requested_at: now + std::time::Duration::from_secs(60),
        deadline: now + std::time::Duration::from_secs(120),
        input_seq: app.input_seq,
        history_trigger: HistoryTrigger::UserChosen,
        cursor_state: app.fullscreen_cursor_state(),
        parked_live_window_id,
        audio_mode_after_swap: false,
    });
    probe
}

#[cfg(windows)]
fn extra_video_close_event(
    window_close: bool,
    generation: u64,
) -> crate::video::NativeVideoOutputEvent {
    if window_close {
        crate::video::NativeVideoOutputEvent::Window(
            crate::video::native_window::NativeVideoWindowEvent::CloseRequested { generation },
        )
    } else {
        crate::video::NativeVideoOutputEvent::CloseFullscreen { generation }
    }
}

#[cfg(windows)]
fn assert_extra_video_close_restarts_list(
    env: &mut phase_c_support::AppTestEnv,
    media_folder: &Path,
) {
    assert!(env.fullscreen_idx.is_none());
    assert_eq!(env.current_folder.as_deref(), Some(media_folder));
    assert_physical(env, media_folder, None);
    assert_eq!(restore_cursor(env).unwrap().name, "clip.mp4");
    restart_previous(env);
    assert_physical(env, media_folder, None);
    assert_eq!(env.current_folder.as_deref(), Some(media_folder));
    assert_eq!(env.items[env.selected.unwrap()].name(), "clip.mp4");
    assert_eq!(restore_cursor(env).unwrap().name, "clip.mp4");
    assert!(env.fullscreen_idx.is_none());
}

#[cfg(windows)]
fn exercise_extra_native_video_close(window_close: bool) {
    let (mut env, parent, media_folder, index) = extra_video_close_env();
    let before = env.settings.startup_list_restore.clone();
    let probe = install_extra_video_source_swap(&mut env, index, None);
    let ctx = egui::Context::default();
    // fs_cache no longer owns the output, so the pending output must supply generation 5.
    assert_eq!(env.native_video_committed_generation_for(index), 0);
    probe.send_event(extra_video_close_event(window_close, 4));
    env.poll_native_video_source_swap_pending(&ctx);
    assert_eq!(env.fullscreen_idx, Some(index));
    assert!(env.native_video_source_swap_pending.is_some());
    assert_eq!(env.settings.startup_list_restore, before);
    assert_physical(&env, &parent, None);

    probe.send_event(extra_video_close_event(window_close, 5));
    env.poll_native_video_source_swap_pending(&ctx);
    assert!(env.native_video_source_swap_pending.is_none());
    assert_extra_video_close_restarts_list(&mut env, &media_folder);
}

#[test]
#[cfg(windows)]
fn section1335_extra_video_native_window_close_rejects_stale_then_restores_folder_list() {
    exercise_extra_native_video_close(true);
}

#[test]
#[cfg(windows)]
fn section1335_extra_video_native_close_fullscreen_rejects_stale_then_restores_folder_list() {
    exercise_extra_native_video_close(false);
}

#[test]
#[cfg(windows)]
fn section1335_extra_video_parked_native_closes_preserve_main_list_restore() {
    for window_close in [true, false] {
        let (mut env, parent, _, index) = extra_video_close_env();
        let before = env.settings.startup_list_restore.clone();
        let probe = install_extra_video_source_swap(&mut env, index, Some(1335));
        let ctx = egui::Context::default();
        probe.send_event(extra_video_close_event(window_close, 5));
        env.poll_native_video_source_swap_pending(&ctx);
        assert!(env.native_video_source_swap_pending.is_some());
        assert_eq!(env.fullscreen_idx, Some(index));
        assert_eq!(env.settings.startup_list_restore, before);

        env.native_video_parked_live_input_window_id = Some(1335);
        env.poll_native_video_source_swap_pending(&ctx);
        assert!(env.native_video_source_swap_pending.is_none());
        assert_eq!(env.fullscreen_idx, Some(index));
        assert_eq!(env.settings.startup_list_restore, before);
        assert_physical(&env, &parent, None);
        env.native_video_parked_live_input_window_id = None;
        restart_previous(&mut env);
        assert_physical(&env, &parent, None);
        assert_eq!(env.current_folder.as_ref(), Some(&parent));
        assert!(env.fullscreen_idx.is_none());
    }
}

#[test]
#[cfg(windows)]
fn section1335_extra_video_egui_close_key_restores_folder_list() {
    let _input_guard = crate::key_input::lock_test_input();
    let (mut env, _, media_folder, index) = extra_video_close_env();
    env.keymap = crate::keymap::Keymap::from_ini_str("[FsVideo]\nVideoCloseFullscreen = F6\n");
    let ctx = egui::Context::default();
    ctx.begin_pass(egui::RawInput {
        events: vec![fullscreen_fixed_key_event(egui::Key::F6)],
        ..Default::default()
    });
    let _ = env.keyboard_owner_for_pass(&ctx);
    env.handle_video_input(&ctx, index, None);
    let _ = ctx.end_pass();
    assert_extra_video_close_restarts_list(&mut env, &media_folder);
}

#[test]
fn section1335_extra_video_normal_parent_return_still_restores_parent_list() {
    let (mut env, parent, book) = book_env();
    open_zip(&mut env, &book, true);
    let before = env.settings.startup_list_restore.clone();
    env.handle_fullscreen_close_request();
    assert!(env.pending_return_to_parent);
    assert!(env.fullscreen_idx.is_some());
    assert_eq!(env.settings.startup_list_restore, before);
    let nav = env.take_pending_return_to_parent_nav().unwrap();
    assert!(env.apply_fullscreen_close_nav_immediate(nav));
    settle_book(&mut env);
    assert_eq!(env.current_folder.as_ref(), Some(&parent));
    assert_physical(&env, &parent, None);
    restart_previous(&mut env);
    assert_eq!(env.current_folder.as_ref(), Some(&parent));
    assert_physical(&env, &parent, None);
    assert!(env.fullscreen_idx.is_none());
}

#[test]
#[cfg(windows)]
fn section1335_tray_converted_book_returns_logical_source_parent_and_preserves_reading_resume() {
    let (mut env, parent, _) = book_env();
    let source = parent.join("converted.7z");
    let cache_folder = env.tmp.path().join("conversion-cache");
    std::fs::create_dir_all(&cache_folder).unwrap();
    let cached = cache_folder.join("cached.zip");
    std::fs::write(&source, b"converted source fixture").unwrap();
    write_zip(&cached, &["a.png", "b.png"]);
    let metadata = std::fs::metadata(&source).unwrap();
    env.archive_cache_db
        .as_ref()
        .unwrap()
        .record(
            &source,
            crate::ui_helpers::mtime_secs(&metadata),
            metadata.len() as i64,
            ArchiveFormat::SevenZ,
            &cached,
            0,
            2,
            false,
        )
        .unwrap();
    env.load_folder(parent.clone());
    env.selected = env
        .items
        .iter()
        .position(|item| item.name() == "converted.7z");
    assert!(env.selected.is_some());
    env.capture_main_list_restore_cursor();
    let before = env.settings.startup_list_restore.clone();
    assert!(matches!(
        env.load_folder_or_convert_archive_with_auto_fullscreen(source.clone(), true),
        FolderOpenOutcome::Loaded | FolderOpenOutcome::Classifying
    ));
    settle_book(&mut env);
    assert_eq!(env.current_folder.as_ref(), Some(&cached));
    assert_eq!(env.archive_source_override.as_ref(), Some(&source));
    assert!(env.fullscreen_idx.is_some());
    env.open_fullscreen(1, HistoryTrigger::UserChosen);
    assert_eq!(env.settings.startup_list_restore, before);
    close_root_to_tray(&mut env);
    settle_book(&mut env);
    assert_eq!(env.current_folder.as_ref(), Some(&parent));
    assert_ne!(env.current_folder.as_ref(), Some(&cache_folder));
    assert!(env.archive_source_override.is_none());
    assert!(env.fullscreen_idx.is_none());
    assert_eq!(env.settings.startup_list_restore, before);
    env.sync_after_restore(&egui::Context::default());
    assert!(env.window_visible);
    assert_eq!(env.current_folder.as_ref(), Some(&parent));
    assert_eq!(env.items[env.selected.unwrap()].name(), "converted.7z");
    assert_eq!(env.settings.startup_list_restore, before);
    assert!(matches!(
        env.load_folder_or_convert_archive_with_auto_fullscreen(source.clone(), true),
        FolderOpenOutcome::Loaded | FolderOpenOutcome::Classifying
    ));
    settle_book(&mut env);
    assert_eq!(env.current_folder.as_ref(), Some(&cached));
    assert_eq!(env.archive_source_override.as_ref(), Some(&source));
    assert_eq!(env.fullscreen_idx, Some(1));
    assert_eq!(env.settings.startup_list_restore, before);
}

#[test]
#[cfg(windows)]
fn section1335_tray_direct_book_with_unrelated_saved_parent_keeps_book_list_and_record() {
    let (mut env, parent, book) = book_env();
    let unrelated = env.tmp.path().join("unrelated-parent");
    std::fs::create_dir_all(&unrelated).unwrap();
    env.load_folder(unrelated.clone());
    let before = env.settings.startup_list_restore.clone();
    open_zip(&mut env, &book, true);
    assert_physical(&env, &unrelated, None);
    assert_ne!(parent, unrelated);
    let generation = env.items_generation;
    close_root_to_tray(&mut env);
    assert_eq!(env.current_folder.as_ref(), Some(&book));
    assert!(env.fullscreen_idx.is_none());
    assert_eq!(env.items_generation, generation);
    assert_eq!(env.settings.startup_list_restore, before);
    env.sync_after_restore(&egui::Context::default());
    assert!(env.window_visible);
    assert_eq!(env.current_folder.as_ref(), Some(&book));
    assert!(env.fullscreen_idx.is_none());
    assert_eq!(env.settings.startup_list_restore, before);
}

#[test]
#[cfg(windows)]
fn section1335_tray_smart_book_with_matching_physical_parent_keeps_book_list_and_synthetic_owner() {
    let (mut env, parent, book) = book_env();
    env.selected = env
        .items
        .iter()
        .position(|item| matches!(item, GridItem::ZipFile(path) if path == &book));
    env.capture_main_list_restore_cursor();
    let before = env.settings.startup_list_restore.clone();
    let mut definition = crate::settings::SmartFolderDefinition::new("Tray synthetic parent");
    definition.rules.push(crate::settings::SmartFolderRule::new(
        parent.clone(),
        true,
        Default::default(),
    ));
    let id = definition.id;
    env.settings.smart_folders = vec![definition];
    env.open_smart_folder(id, false);
    wait_smart(&mut env);
    assert!(env.items_are_smart_folder_view);
    let kind = env.smart_physical_target_kind(&book).unwrap();
    assert!(
        env.begin_smart_physical_navigation(
            book.clone(),
            kind,
            true,
            None,
            None,
            StartupListIntent::PageContinuation,
        )
        .is_ok()
    );
    wait_smart(&mut env);
    assert_eq!(env.effective_folder().as_ref(), Some(&book));
    if env.fullscreen_idx.is_none() {
        env.open_fullscreen(0, HistoryTrigger::UserChosen);
    }
    assert_physical(&env, &parent, None);
    let synthetic = crate::app::smart_folder::smart_folder_synthetic_path(id);
    assert!(matches!(
        env.resolve_return_to_parent_nav(),
        Some(crate::ui_main::AddressBarNav::Direct(path, _)) if path == synthetic
    ));
    close_root_to_tray(&mut env);
    assert_eq!(env.effective_folder().as_ref(), Some(&book));
    assert!(env.fullscreen_idx.is_none());
    assert_eq!(env.settings.startup_list_restore, before);
    assert!(env.top_level_grid_view.smart_folder().is_some());
    env.sync_after_restore(&egui::Context::default());
    assert!(env.window_visible);
    assert_eq!(env.effective_folder().as_ref(), Some(&book));
    assert!(env.fullscreen_idx.is_none());
    assert_eq!(env.settings.startup_list_restore, before);
    assert!(matches!(
        env.resolve_return_to_parent_nav(),
        Some(crate::ui_main::AddressBarNav::Direct(path, _)) if path == synthetic
    ));
}

#[cfg(windows)]
fn open_selected_image_folder_book(app: &mut App, book: &Path) {
    let Some(crate::ui_main::AddressBarNav::Direct(path, intent)) =
        app.handle_gamepad_grid_accept(&egui::Context::default())
    else {
        panic!("an ordinary image Folder uses accepted main-grid direct navigation");
    };
    assert_eq!(path, book);
    assert_eq!(intent, StartupListIntent::ClassifyFolder);
    assert!(app.pending_auto_fs_open);
    // Match App::update's Folder dispatch and its existing completed-scan book classifier.
    app.open_direct_navigation_target(path, None, OpenRequestOwner::Navigation, None, None, intent);
    settle_book(app);
    assert_eq!(app.current_folder.as_deref(), Some(book));
    assert!(app.fullscreen_idx.is_some());
    assert!(app.items_are_image_only_folder_pages());
    assert!(app.auto_open_for_current_container());
}

#[cfg(windows)]
fn direct_tray_image_folder_book_env() -> (phase_c_support::AppTestEnv, PathBuf, PathBuf) {
    let mut env = phase_c_support::setup_app();
    let parent = env.tmp.path().join("image-folder-parent");
    let book = parent.join("book");
    std::fs::create_dir_all(&book).unwrap();
    for index in 0..12 {
        std::fs::create_dir_all(parent.join(format!("before-{index:02}"))).unwrap();
    }
    for name in ["a.png", "b.png"] {
        std::fs::write(book.join(name), png_bytes()).unwrap();
    }
    // Keep this outside book_root so a genuine image-only folder scan decides book status.
    assert!(!crate::books::is_direct_book_folder(
        &env.book_root_path(),
        &book
    ));
    env.settings.startup_folder_mode = crate::settings::StartupFolderMode::Previous;
    env.settings.restore_last_cursor = true;
    env.settings.auto_fullscreen_zip_pdf = true;
    env.settings.auto_fullscreen_image_folders = true;
    env.settings.book_open_resume = crate::settings::ResumeMode::Resume;
    env.settings.detached_viewer_open_images_in_window = false;
    env.settings.video_in_window_mode = false;
    env.settings.sort_order = crate::settings::SortOrder::FileName;
    env.load_folder(parent.clone());
    let index = env
        .items
        .iter()
        .position(|item| matches!(item, GridItem::Folder(path) if path == &book))
        .unwrap();
    assert_eq!(index, 12);
    env.last_grid_cols = 3;
    env.last_cell_h = 40.0;
    env.scroll_offset_y = 80.0;
    env.selected = Some(index);
    open_selected_image_folder_book(&mut env, &book);
    assert_eq!(
        restore_cursor(&env),
        Some(&ListCursorHint {
            name: "book".into(),
            rows_above: Some(2),
        })
    );
    env.open_fullscreen(1, HistoryTrigger::UserChosen);
    assert_eq!(env.items[env.fullscreen_idx.unwrap()].name(), "b.png");
    assert_physical(&env, &parent, None);
    (env, parent, book)
}

#[cfg(windows)]
fn backspace_image_folder_book_to_list(app: &mut App, book: &Path) {
    let ctx = egui::Context::default();
    ctx.begin_pass(egui::RawInput {
        events: vec![fullscreen_fixed_key_event(egui::Key::Backspace)],
        ..Default::default()
    });
    assert!(app.handle_fullscreen_root_key_input(&ctx));
    let _ = ctx.end_pass();
    assert!(app.fullscreen_idx.is_none());
    assert_eq!(app.current_folder.as_deref(), Some(book));
    assert_physical(app, book, None);
    assert_eq!(app.items[app.selected.unwrap()].name(), "b.png");
    app.capture_main_list_restore_cursor();
}

#[test]
#[cfg(windows)]
fn section1335_tray_image_folder_book_direct_hide_restore_preserves_parent_cursor_and_resume() {
    for restore_last_cursor in [true, false] {
        let (mut env, parent, book) = direct_tray_image_folder_book_env();
        env.settings.restore_last_cursor = restore_last_cursor;
        let before = env.settings.startup_list_restore.clone();
        close_root_to_tray(&mut env);
        settle_book(&mut env);
        assert!(env.fullscreen_idx.is_none());
        assert_eq!(env.current_folder.as_ref(), Some(&parent));
        assert_eq!(env.items[env.selected.unwrap()].name(), "book");
        assert_eq!(env.scroll_selected_to_rows_above, Some(2));
        assert_eq!(env.settings.startup_list_restore, before);
        assert_eq!(
            crate::settings::Settings::load().startup_list_restore,
            before
        );
        let generation = env.items_generation;
        env.sync_after_restore(&egui::Context::default());
        assert!(env.window_visible);
        assert_eq!(env.current_folder.as_ref(), Some(&parent));
        assert_eq!(env.items_generation, generation);
        assert_eq!(env.items[env.selected.unwrap()].name(), "book");
        assert_eq!(env.scroll_selected_to_rows_above, Some(2));
        assert_eq!(env.settings.startup_list_restore, before);
        open_selected_image_folder_book(&mut env, &book);
        assert_eq!(env.fullscreen_idx, Some(1));
        assert_eq!(env.settings.startup_list_restore, before);
    }
}

#[test]
#[cfg(windows)]
fn section1335_tray_image_folder_book_backspace_hide_restore_keeps_explicit_book_list() {
    let _input_guard = crate::key_input::lock_test_input();
    for restore_last_cursor in [true, false] {
        let (mut env, _, book) = direct_tray_image_folder_book_env();
        env.settings.restore_last_cursor = restore_last_cursor;
        backspace_image_folder_book_to_list(&mut env, &book);
        let before = env.settings.startup_list_restore.clone();
        assert!(
            env.handle_gamepad_grid_accept(&egui::Context::default())
                .is_none()
        );
        assert_eq!(env.fullscreen_idx, Some(1));
        close_root_to_tray(&mut env);
        env.sync_after_restore(&egui::Context::default());
        assert!(env.window_visible);
        assert!(env.fullscreen_idx.is_none());
        assert_eq!(env.current_folder.as_ref(), Some(&book));
        assert_eq!(env.items[env.selected.unwrap()].name(), "b.png");
        assert_eq!(env.settings.startup_list_restore, before);
    }
}

#[test]
#[cfg(windows)]
fn section1335_tray_image_folder_book_direct_quit_restarts_parent_and_preserves_resume() {
    let (mut env, parent, book) = direct_tray_image_folder_book_env();
    let before = env.settings.startup_list_restore.clone();
    // Exercise ordinary process exit independently of tray's close handler.
    restart_previous(&mut env);
    assert_eq!(env.current_folder.as_ref(), Some(&parent));
    assert!(env.fullscreen_idx.is_none());
    assert_eq!(env.items[env.selected.unwrap()].name(), "book");
    assert_eq!(env.scroll_selected_to_rows_above, Some(2));
    assert_eq!(env.settings.startup_list_restore, before);
    open_selected_image_folder_book(&mut env, &book);
    assert_eq!(env.fullscreen_idx, Some(1));
    assert_eq!(env.settings.startup_list_restore, before);
}

#[test]
#[cfg(windows)]
fn section1335_tray_image_folder_book_backspace_quit_restarts_explicit_book_list() {
    let _input_guard = crate::key_input::lock_test_input();
    let (mut env, _, book) = direct_tray_image_folder_book_env();
    backspace_image_folder_book_to_list(&mut env, &book);
    let before = env.settings.startup_list_restore.clone();
    assert!(
        env.handle_gamepad_grid_accept(&egui::Context::default())
            .is_none()
    );
    assert_eq!(env.fullscreen_idx, Some(1));
    restart_previous(&mut env);
    assert_eq!(env.current_folder.as_ref(), Some(&book));
    assert!(env.fullscreen_idx.is_none());
    assert_eq!(env.items[env.selected.unwrap()].name(), "b.png");
    assert_eq!(env.settings.startup_list_restore, before);
}
