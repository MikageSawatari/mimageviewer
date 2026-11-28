//! §1.853 C/D/E: production admission and adoption boundaries, without a live application.
#![cfg(windows)]

use super::phase_c_support::{AppTestEnv, setup_app};
use super::*;

#[path = "fullscreen_close_focus_853.rs"]
mod focus;

fn directory(app: &AppTestEnv, relative: &str) -> PathBuf {
    let path = app.tmp.path().join(relative);
    std::fs::create_dir_all(&path).unwrap();
    path
}

fn mount_pdf_reader(app: &mut App, book: &Path) {
    app.current_folder = Some(book.to_path_buf());
    app.current_folder_last_mtime = None;
    app.items = vec![GridItem::PdfPage {
        pdf_path: book.to_path_buf(),
        page_num: 0,
        content_type: None,
    }];
    app.thumbnails = vec![ThumbnailState::Failed];
    app.image_metas = vec![Some((0, 0))];
    app.visible_indices = vec![0];
    app.fullscreen_idx = Some(0);
    app.settings.auto_fullscreen_zip_pdf = true;
    app.settings.detached_viewer_open_images_in_window = false;
    app.viewer_presentation = ViewerPresentation::Fullscreen;
    app.address = book.to_string_lossy().into_owned();
}

fn issue_close(app: &mut App) -> u64 {
    app.handle_fullscreen_close_request();
    let nav = app
        .take_pending_return_to_parent_nav()
        .expect("close must resolve its parent navigation");
    assert!(app.apply_fullscreen_close_nav_immediate(nav));
    let pending = app.folder_pane_open_pending.as_ref().unwrap();
    assert!(pending.purpose.closes_main_fullscreen());
    let FolderOpenScanPurpose::DirectNavigation(call) = &pending.purpose else {
        panic!("physical close return must use the directory continuation");
    };
    call.navigation
        .as_ref()
        .unwrap()
        .close_return_presentation()
        .unwrap()
        .request_id
}

fn hold_scan_reply(
    app: &mut App,
) -> (
    mpsc::Sender<FolderOpenScanResult>,
    FolderOpenScanResult,
    Arc<AtomicBool>,
) {
    let pending = app.folder_pane_open_pending.as_mut().unwrap();
    let result = pending
        .rx
        .recv_timeout(std::time::Duration::from_secs(30))
        .unwrap();
    let token = Arc::clone(&pending.cancel);
    let (tx, rx) = mpsc::channel();
    pending.rx = rx;
    (tx, result, token)
}

#[test]
fn section1853_parent_dialog_and_books_root_dispatch_scan_off_caller() {
    for entry in ["parent", "dialog", "books-root"] {
        let mut app = setup_app();
        app.active_quick_folder_slot = None;
        let target = directory(&app, "destination");
        let child = directory(&app, "destination/child");
        let origin = if entry == "parent" {
            child.clone()
        } else {
            directory(&app, "origin")
        };
        app.load_folder_and_finish_for_test(origin.clone());
        let scans = folder_scan::caller_scan_count_for_test();
        match entry {
            "parent" => {
                let nav = app.handle_grid_parent_folder_action().unwrap();
                app.post_render_main_navigation(
                    &egui::Context::default(),
                    update_phases::MainFrameNavigation {
                        keyboard_nav: Some(nav),
                        ..Default::default()
                    },
                );
            }
            "dialog" => app.post_render_main_navigation(
                &egui::Context::default(),
                update_phases::MainFrameNavigation {
                    open_folder_nav: Some(target.clone()),
                    ..Default::default()
                },
            ),
            _ => {
                app.settings.book_root = Some(target.clone());
                app.open_books_root();
            }
        }
        assert_eq!(folder_scan::caller_scan_count_for_test(), scans, "{entry}");
        assert_eq!(app.current_folder.as_ref(), Some(&origin));
        assert!(app.direct_navigation_scan_pending());
        app.finish_direct_navigation_scan_for_test();
        assert_eq!(folder_scan::caller_scan_count_for_test(), scans, "{entry}");
        assert_eq!(app.current_folder.as_ref(), Some(&target));
        assert!(app.items.contains(&GridItem::Folder(child.clone())));
        if entry == "parent" {
            assert!(
                matches!(&app.items[app.selected.unwrap()], GridItem::Folder(path) if path == &child)
            );
        }
    }
}

#[test]
fn section1853_created_sibling_zip_completion_keeps_output_selection_with_worker() {
    use crate::ui_dialogs::archive_convert::{
        ArchiveConvertCompletionPolicy, ArchiveConvertPhase, ArchiveConvertState,
    };
    let mut app = setup_app();
    let parent = directory(&app, "sibling-output");
    app.load_folder_and_finish_for_test(parent.clone());
    let output = parent.join("created.zip");
    std::fs::write(&output, []).unwrap();
    let (_tx, rx) = mpsc::channel();
    app.archive_convert = Some(Box::new(ArchiveConvertState {
        src_path: parent.join("source.7z"),
        input_seq: 0,
        format: crate::archive_converter::ArchiveFormat::SevenZ,
        password: None,
        password_input: String::new(),
        phase: ArchiveConvertPhase::Scanning,
        cancel: Arc::new(AtomicBool::new(false)),
        rx,
        pending_nav: None,
        pending_direct_nav: None,
        allow_direct_read: false,
        completion: ArchiveConvertCompletionPolicy::SiblingZip,
        restore_intent: StartupListIntent::PreservePresentation,
        pending_sibling_output: Some(output.clone()),
        auto_fullscreen: false,
        deferred_fullscreen: None,
        suppress_confirm: false,
        suppress_confirm_next_time: false,
    }));
    let scans = folder_scan::caller_scan_count_for_test();
    let generation = app.items_generation;
    app.show_archive_convert_dialog(&egui::Context::default());
    assert!(app.archive_convert.is_none());
    assert!(app.direct_navigation_scan_pending());
    assert_eq!(app.items_generation, generation);
    assert_eq!(folder_scan::caller_scan_count_for_test(), scans);
    app.finish_direct_navigation_scan_for_test();
    assert_eq!(folder_scan::caller_scan_count_for_test(), scans);
    assert!(
        matches!(&app.items[app.selected.unwrap()], GridItem::ZipFile(path) if path == &output)
    );
    assert!(app.select_after_load.is_none());
}

#[test]
fn section1853_missing_mtime_real_folder_reload_uses_staged_refresh() {
    let mut app = setup_app();
    app.active_quick_folder_slot = None;
    let folder = directory(&app, "mtime-missing");
    let chosen = directory(&app, "mtime-missing/chosen");
    app.load_folder_and_finish_for_test(folder.clone());
    app.selected = app
        .items
        .iter()
        .position(|item| matches!(item, GridItem::Folder(path) if path == &chosen));
    app.current_folder_last_mtime = None;
    let generation = app.items_generation;
    let history = app.folder_nav_history_snapshot();
    let scans = folder_scan::caller_scan_count_for_test();
    app.reload_current_physical_list();
    assert!(matches!(
        app.top_level_grid_view.history_navigation_transition(),
        Some(HistoryNavigationTransition::Physical(_))
    ));
    assert!(!app.direct_navigation_scan_pending());
    assert_eq!(app.items_generation, generation);
    app.finish_history_restore_preparation_for_test();
    assert_eq!(folder_scan::caller_scan_count_for_test(), scans);
    assert_eq!(app.current_folder.as_ref(), Some(&folder));
    assert_eq!(app.folder_nav_history_snapshot(), history);
    assert!(matches!(&app.items[app.selected.unwrap()], GridItem::Folder(path) if path == &chosen));
}

#[test]
fn section1853_converted_cache_reload_keeps_logical_source_and_explicit_sync_route() {
    let mut app = setup_app();
    let cache = directory(&app, "converted-cache");
    let page = cache.join("page.png");
    image::RgbImage::new(2, 3).save(&page).unwrap();
    app.load_folder_and_finish_for_test(cache.clone());
    let logical = app.tmp.path().join("original.7z");
    std::fs::write(&logical, []).unwrap();
    app.archive_source_override = Some(logical.clone());
    app.address = logical.to_string_lossy().into_owned();
    app.current_folder_last_mtime = None;
    let scans = folder_scan::caller_scan_count_for_test();
    app.reload_current_physical_list();
    app.finish_page_identity_preparation_for_test();
    assert_eq!(folder_scan::caller_scan_count_for_test(), scans + 1);
    assert!(!app.direct_navigation_scan_pending());
    assert!(
        app.top_level_grid_view
            .history_navigation_transition()
            .is_none()
    );
    assert_eq!(app.current_folder.as_ref(), Some(&cache));
    assert_eq!(app.archive_source_override.as_ref(), Some(&logical));
    assert_eq!(app.effective_folder().as_ref(), Some(&logical));
    assert_eq!(app.address, logical.to_string_lossy());
    assert!(app.items.contains(&GridItem::Image(page)));
}

#[test]
fn section1853_archive_named_directory_context_jump_scans_off_caller() {
    use crate::ui_dialogs::context_menu::{
        JumpToFolderDestination, JumpToFolderRequest, JumpToFolderSelection,
    };
    let mut app = setup_app();
    let origin = directory(&app, "jump-origin");
    let target = directory(&app, "jump-origin/directory.zip");
    let child = directory(&app, "jump-origin/directory.zip/child");
    app.load_folder_and_finish_for_test(origin.clone());
    assert!(app.items.contains(&GridItem::Folder(target.clone())));
    let scans = folder_scan::caller_scan_count_for_test();
    // Exercise the actual ArchiveContainer context-menu branch with no classified
    // kind. The visible Folder row proves this archive-named path is a directory.
    assert!(
        app.begin_context_jump_to_folder(JumpToFolderRequest {
            destination: JumpToFolderDestination::ArchiveContainer(target.clone()),
            selection: JumpToFolderSelection::None,
            origin: None,
        })
        .is_none()
    );
    assert!(app.direct_navigation_scan_pending());
    assert_eq!(app.current_folder.as_ref(), Some(&origin));
    assert_eq!(folder_scan::caller_scan_count_for_test(), scans);
    app.finish_direct_navigation_scan_for_test();
    app.finish_page_identity_preparation_for_test();
    assert_eq!(folder_scan::caller_scan_count_for_test(), scans);
    assert_eq!(app.current_folder.as_ref(), Some(&target));
    assert!(app.items.contains(&GridItem::Folder(child)));
}

#[test]
fn section1853_fullscreen_close_keeps_reader_until_parent_install() {
    let mut app = setup_app();
    app.active_quick_folder_slot = None;
    let parent = directory(&app, "close-parent");
    let book = parent.join("book.pdf");
    std::fs::write(&book, b"%PDF-1.4\n").unwrap();
    mount_pdf_reader(&mut app, &book);
    let items = app.items.clone();
    let generation = app.items_generation;
    let scans = folder_scan::caller_scan_count_for_test();
    let request_id = issue_close(&mut app);
    let (tx, result, _) = hold_scan_reply(&mut app);
    assert_eq!(folder_scan::caller_scan_count_for_test(), scans);
    assert_eq!(app.fullscreen_idx, Some(0));
    assert_eq!(app.current_folder.as_ref(), Some(&book));
    assert_eq!(app.items, items);
    assert_eq!(app.items_generation, generation);
    assert!(app.fs_nav_is_locked());
    assert!(
        app.fs_holdover_tex
            .as_ref()
            .and_then(FsHoldover::navigation_sequence)
            .is_some_and(|seq| matches!(seq.purpose,
                FsNavigationPurpose::CloseReturn { request_id: owner } if owner == request_id))
    );
    tx.send(result).unwrap();
    app.finish_direct_navigation_scan_for_test();
    assert_eq!(folder_scan::caller_scan_count_for_test(), scans);
    assert_eq!(app.fullscreen_idx, None);
    assert_eq!(app.current_folder.as_ref(), Some(&parent));
    assert!(matches!(&app.items[app.selected.unwrap()], GridItem::PdfFile(path) if path == &book));
    assert!(!app.fs_nav_is_locked());
    assert!(app.fs_holdover_tex.is_none());
}

#[test]
fn section1853_fullscreen_close_error_and_replacement_keep_reader_and_release_lock() {
    for terminal in ["error", "replacement"] {
        let mut app = setup_app();
        let parent = directory(&app, "close-terminal-parent");
        let book = parent.join("book.pdf");
        std::fs::write(&book, b"%PDF-1.4\n").unwrap();
        mount_pdf_reader(&mut app, &book);
        issue_close(&mut app);
        let (tx, _, token) = hold_scan_reply(&mut app);
        let generation = app.items_generation;
        let scans = folder_scan::caller_scan_count_for_test();
        if terminal == "error" {
            tx.send(FolderOpenScanResult {
                scan: Err(std::io::Error::from(std::io::ErrorKind::PermissionDenied)),
                policy: app.folder_scan_policy(),
                policy_mixed: false,
            })
            .unwrap();
            app.poll_main_embedded_required_fullscreen_folder_open(&egui::Context::default());
            assert!(app.folder_pane_open_pending.is_none());
            assert!(app.fs_feedback_toast.is_some());
        } else {
            let other = directory(&app, "close-replacement");
            app.app.load_folder(other);
            assert!(token.load(Ordering::Relaxed));
            app.cancel_folder_pane_open(PaneOpenRestoreExit::Abandoned);
        }
        assert_eq!(folder_scan::caller_scan_count_for_test(), scans);
        assert_eq!(app.fullscreen_idx, Some(0));
        assert_eq!(app.current_folder.as_ref(), Some(&book));
        assert_eq!(app.items_generation, generation);
        assert!(!app.fs_nav_is_locked());
        assert!(app.fs_holdover_tex.is_none());
    }
}

#[test]
fn section1853_second_fullscreen_close_retires_first_wait_and_adopts_same_parent() {
    let mut app = setup_app();
    let parent = directory(&app, "close-twice-parent");
    let book = parent.join("book.pdf");
    std::fs::write(&book, b"%PDF-1.4\n").unwrap();
    mount_pdf_reader(&mut app, &book);
    let scans = folder_scan::caller_scan_count_for_test();
    let first = issue_close(&mut app);
    let (late_tx, late_result, token) = hold_scan_reply(&mut app);
    let second = issue_close(&mut app);
    assert_ne!(first, second);
    assert!(token.load(Ordering::Relaxed));
    assert!(late_tx.send(late_result).is_err());
    assert_eq!(app.fullscreen_idx, Some(0));
    assert_eq!(app.current_folder.as_ref(), Some(&book));
    app.finish_direct_navigation_scan_for_test();
    assert_eq!(folder_scan::caller_scan_count_for_test(), scans);
    assert_eq!(app.fullscreen_idx, None);
    assert_eq!(app.current_folder.as_ref(), Some(&parent));
    assert!(matches!(&app.items[app.selected.unwrap()], GridItem::PdfFile(path) if path == &book));
}

#[test]
fn section1853_page_list_close_retires_but_focus_guard_preserves_parent_wait() {
    for close in ["page-list", "focus-guard"] {
        let mut app = setup_app();
        let parent = directory(&app, "close-late-parent");
        let book = parent.join("book.pdf");
        std::fs::write(&book, b"%PDF-1.4\n").unwrap();
        mount_pdf_reader(&mut app, &book);
        issue_close(&mut app);
        let (late_tx, result, token) = hold_scan_reply(&mut app);
        if close == "page-list" {
            app.close_fullscreen_to_page_list();
        } else {
            // §6.19 intentionally changes this policy: main focus restores C5
            // rather than entering the true-close boundary and cancelling it.
            app.settings.fullscreen_keep_on_app_switch = false;
            app.fs_focus_grace_elapsed = true;
            app.reconcile_fullscreen_after_main_focus(
                &egui::Context::default(),
                true,
                false,
                false,
            );
            assert!(!token.load(Ordering::Relaxed));
            assert!(app.folder_pane_open_pending.is_some());
            assert_eq!(app.fullscreen_idx, Some(0));
            late_tx.send(result).unwrap();
            app.poll_main_embedded_required_fullscreen_folder_open(&egui::Context::default());
            assert!(!app.fs_nav_is_locked());
            assert_eq!(app.fullscreen_idx, None);
            assert_eq!(app.current_folder.as_ref(), Some(&parent));
            assert!(
                matches!(&app.items[app.selected.unwrap()], GridItem::PdfFile(path) if path == &book)
            );
            continue;
        }
        assert!(token.load(Ordering::Relaxed), "{close}");
        assert!(late_tx.send(result).is_err());
        assert!(app.folder_pane_open_pending.is_none());
        let generation = app.items_generation;
        app.poll_main_embedded_required_fullscreen_folder_open(&egui::Context::default());
        app.post_render_main_navigation(&egui::Context::default(), Default::default());
        assert_eq!(app.fullscreen_idx, None);
        assert_eq!(app.current_folder.as_ref(), Some(&book));
        assert_eq!(app.items_generation, generation);
        assert!(!app.fs_nav_is_locked());
    }
}

#[test]
fn section1853_main_close_wait_does_not_block_projected_detached_context() {
    let mut app = setup_app();
    let parent = directory(&app, "close-detached-parent");
    let book = parent.join("book.pdf");
    std::fs::write(&book, b"%PDF-1.4\n").unwrap();
    mount_pdf_reader(&mut app, &book);
    issue_close(&mut app);
    let (_tx, _result, token) = hold_scan_reply(&mut app);
    assert!(!app.grid_item_input_allowed());
    let sibling = app.build_window_context_for_test(185_300, |sibling| {
        sibling.current_folder = Some(PathBuf::from("C:/disposable-sibling"));
    });
    app.with_viewer_context(sibling, |sibling| {
        assert!(!sibling.direct_navigation_scan_pending());
        assert!(sibling.grid_item_input_allowed());
        assert!(!sibling.fs_nav_is_locked());
    })
    .unwrap();
    assert!(!token.load(Ordering::Relaxed));
    assert!(app.direct_navigation_scan_pending());
    assert!(!app.grid_item_input_allowed());
    app.cancel_folder_pane_open(PaneOpenRestoreExit::Abandoned);
}

#[test]
fn section1853_collection_descendant_close_keeps_physical_owner_through_parent_wait() {
    use crate::app::top_level_grid_view::CollectionGridLoadState;
    use crate::collection_store::{
        CollectionRegistration, CollectionResolvedKind, CollectionStoreRuntime,
    };
    let mut app = setup_app();
    app.active_quick_folder_slot = None;
    let source = directory(&app, "collection-close-source");
    let book = source.join("book.pdf");
    std::fs::write(&book, b"%PDF-1.4\n").unwrap();
    let runtime =
        CollectionStoreRuntime::start_at(app.tmp.path().join("collection-close.db")).unwrap();
    let client = runtime.client();
    app.install_collection_runtime(runtime);
    let ctx = egui::Context::default();
    let ready_deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    // start_at only spawns the actor. Wait for its real Ready event through
    // App's normal consumer before submitting fixture mutations; create itself
    // rejects Starting, so its later reply timeout cannot wait for startup.
    while !matches!(app.collection_store_client_for_read(), Ok(Some(_))) {
        assert!(
            std::time::Instant::now() < ready_deadline,
            "collection fixture runtime did not become ready"
        );
        app.poll_collection_ui(&ctx);
        std::thread::yield_now();
    }
    let created = client
        .create_collection("Close return".into())
        .unwrap()
        .recv_timeout(std::time::Duration::from_secs(5))
        .unwrap()
        .unwrap();
    let collection = client
        .add_batch(
            created.collection_id(),
            created.revision(),
            vec![
                CollectionRegistration::from_trusted_path(&source, CollectionResolvedKind::Folder)
                    .unwrap(),
            ],
        )
        .unwrap()
        .recv_timeout(std::time::Duration::from_secs(5))
        .unwrap()
        .unwrap()
        .snapshot;
    let id = collection.collection_id();
    app.poll_collection_ui(&ctx);
    app.open_collection_grid(id, None);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        app.poll_collection_ui(&ctx);
        app.poll_collection_grid(&ctx);
        app.poll_collection_history_transition(&ctx);
        app.poll_sidecar_restore(&ctx);
        if app
            .top_level_grid_view
            .collection_session()
            .is_some_and(|session| {
                matches!(
                    session.load,
                    CollectionGridLoadState::Ready(_) | CollectionGridLoadState::Empty(_)
                ) && session.installed_items_generation == Some(app.items_generation)
            })
            && !app.sidecar_restore_active()
        {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "collection root did not settle"
        );
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    let nav = app.grid_physical_navigation(0, source.clone(), false);
    app.post_render_main_navigation(
        &ctx,
        update_phases::MainFrameNavigation {
            grid_nav: Some(nav),
            ..Default::default()
        },
    );
    app.finish_direct_navigation_scan_for_test();
    mount_pdf_reader(&mut app, &book);
    let scans = folder_scan::caller_scan_count_for_test();
    let _window = focus::surface_window(&app, &ctx);
    let _foreground = crate::test_native_focus::scoped(focus::SURFACE, 0, true);
    let surface = app.fullscreen_viewport_id();
    assert!(
        focus::frame(
            &ctx,
            egui::ViewportId::ROOT,
            surface,
            Some(egui::Key::Enter),
            || {
                focus::close_entry(&mut app, &ctx, "enter");
                focus::dispatch_close(&mut app, &ctx);
            }
        )
        .is_empty()
    );
    let FolderOpenScanPurpose::DirectNavigation(call) =
        &app.folder_pane_open_pending.as_ref().unwrap().purpose
    else {
        panic!("collection physical close must use directory continuation");
    };
    assert!(matches!(
        call.owner,
        OpenRequestOwner::CollectionGridPhysical(_)
    ));
    assert_eq!(app.fullscreen_idx, Some(0));
    let (tx, result, _) = hold_scan_reply(&mut app);
    tx.send(result).unwrap();
    assert_eq!(
        focus::frame(&ctx, egui::ViewportId::ROOT, surface, None, || {
            app.poll_main_embedded_required_fullscreen_folder_open(&ctx);
        }),
        [egui::ViewportId::ROOT]
    );
    assert_eq!(folder_scan::caller_scan_count_for_test(), scans);
    assert_eq!(app.current_folder.as_ref(), Some(&source));
    assert_eq!(app.fullscreen_idx, None);
    assert!(matches!(&app.items[app.selected.unwrap()], GridItem::PdfFile(path) if path == &book));
    assert_eq!(
        app.top_level_grid_view
            .collection_session()
            .unwrap()
            .identity
            .collection_id,
        id
    );
    app.shutdown_collection_runtime_for_exit();
}

#[test]
fn section1853_completed_bookmark_reader_close_uses_ordinary_parent_wait() {
    use crate::bookmark_browser::{
        BookmarkOpenRequestId, PendingBookOpen, PendingBookOpenStage, PendingBookmarkOpen,
    };
    let mut app = setup_app();
    let parent = directory(&app, "bookmark-close-parent");
    let book = parent.join("book.pdf");
    std::fs::write(&book, b"%PDF-1.4\n").unwrap();
    mount_pdf_reader(&mut app, &book);
    app.fullscreen_idx = None;
    // A completed per-book bookmark opens an ordinary reader. A global bookmark
    // grid's BookmarkViewState::Opening instead returns to the synthetic Bookmarks
    // surface through its existing branch and is outside the C5 directory scan.
    let now = std::time::Instant::now();
    app.bookmark_open_pending = Some(PendingBookmarkOpen::Book(PendingBookOpen {
        request_id: BookmarkOpenRequestId(185_301),
        bookmark_source: crate::pdf_loader::LeasedEpubPath::try_new(book.clone()).unwrap(),
        bookmark: crate::book_bookmarks::BookBookmark {
            id: 1,
            container_key: crate::book_bookmarks::container_key(&book),
            container_path: book.clone(),
            container_kind: crate::book_bookmarks::BookContainerKind::Pdf,
            page_identity: crate::book_bookmarks::PageIdentity::PdfPage(0),
            page_index_hint: 0,
            created_at_ms: 1,
            title: None,
        },
        relative_page_provenance: None,
        started_at: now,
        stage: PendingBookOpenStage::AwaitingPage {
            started_at: now,
            entered_archive_prefix: false,
        },
    }));
    app.poll_bookmark_book_open(&egui::Context::default());
    assert!(app.bookmark_open_pending.is_none());
    assert!(app.bookmark_view_state.is_none());
    assert_eq!(app.fullscreen_idx, Some(0));
    let scans = folder_scan::caller_scan_count_for_test();
    issue_close(&mut app);
    assert_eq!(app.fullscreen_idx, Some(0));
    assert_eq!(app.current_folder.as_ref(), Some(&book));
    app.finish_direct_navigation_scan_for_test();
    assert_eq!(folder_scan::caller_scan_count_for_test(), scans);
    assert_eq!(app.fullscreen_idx, None);
    assert_eq!(app.current_folder.as_ref(), Some(&parent));
    assert!(matches!(&app.items[app.selected.unwrap()], GridItem::PdfFile(path) if path == &book));
}

#[test]
fn section1853_delayed_terminal_close_effect_preserves_new_close_return_request() {
    let mut app = setup_app();
    let parent = directory(&app, "terminal-effect-parent");
    let book = parent.join("book.pdf");
    std::fs::write(&book, b"%PDF-1.4\n").unwrap();
    mount_pdf_reader(&mut app, &book);
    // Use the transition reducer to produce the old close's real content effect,
    // then delay only delivery of that effect until a new C5 has been accepted.
    app.video_presentation_transition
        .reset_stable(ViewerPresentation::Fullscreen);
    app.video_presentation_transition.request_transition(
        ViewerPresentation::MainWindow,
        false,
        false,
        0,
        None,
        None,
    );
    app.video_presentation_transition.take_effects();
    app.video_presentation_transition.drive();
    app.video_presentation_transition
        .dispatch(PresentationTransitionEvent::TerminalClose);
    let terminal = app
        .video_presentation_transition
        .take_effects()
        .into_iter()
        .find(|effect| {
            matches!(
                effect,
                PresentationTransitionEffect::TerminalSessionClose { .. }
            )
        })
        .expect("old TerminalClose must produce its delayed content effect");
    assert!(!app.video_presentation_transition.is_transitioning());
    let scans = folder_scan::caller_scan_count_for_test();
    let request_id = issue_close(&mut app);
    let (tx, result, token) = hold_scan_reply(&mut app);
    assert_eq!(
        app.execute_video_presentation_host_effect(&egui::Context::default(), terminal),
        PresentationEffectsOutcome::ClosedFullscreen
    );
    let pending = app
        .folder_pane_open_pending
        .as_ref()
        .expect("accepted new C5 must survive");
    assert!(Arc::ptr_eq(&pending.cancel, &token));
    assert!(!token.load(Ordering::Relaxed));
    assert_eq!(
        pending
            .purpose
            .close_return_presentation()
            .unwrap()
            .request_id,
        request_id
    );
    tx.send(result).unwrap();
    app.finish_direct_navigation_scan_for_test();
    assert_eq!(folder_scan::caller_scan_count_for_test(), scans);
    assert_eq!(app.current_folder.as_ref(), Some(&parent));
    assert_eq!(app.fullscreen_idx, None);
    assert!(matches!(&app.items[app.selected.unwrap()], GridItem::PdfFile(path) if path == &book));
}

#[test]
fn section1853_visibility_apply_retires_old_scan_before_current_list_refresh() {
    let mut app = setup_app();
    let origin = directory(&app, "visibility-origin");
    let target = directory(&app, "visibility-target");
    let page = origin.join("page.png");
    image::RgbImage::new(2, 3).save(&page).unwrap();
    app.load_folder_and_finish_for_test(origin.clone());
    app.app.load_folder(target);
    let (late_tx, result, token) = hold_scan_reply(&mut app);
    app.settings
        .file_type_visibility
        .excluded_extensions
        .insert("png".into());
    let scans = folder_scan::caller_scan_count_for_test();
    let ctx = egui::Context::default();
    app.apply_file_type_visibility_change(&ctx);
    assert!(token.load(Ordering::Relaxed));
    assert!(late_tx.send(result).is_err());
    assert!(!app.direct_navigation_scan_pending());
    assert!(matches!(
        app.folder_pane_open_pending.as_ref().unwrap().purpose,
        FolderOpenScanPurpose::CurrentViewOrderRefresh { .. }
    ));
    let (tx, refreshed, _) = hold_scan_reply(&mut app);
    tx.send(refreshed).unwrap();
    app.post_render_main_navigation(&ctx, Default::default());
    app.finish_page_identity_preparation_for_test();
    assert_eq!(folder_scan::caller_scan_count_for_test(), scans);
    assert_eq!(app.current_folder.as_ref(), Some(&origin));
    assert!(!app.items.contains(&GridItem::Image(page)));
}

#[test]
fn section1853_sidecar_restore_keeps_ready_scan_owned_until_restore_terminal() {
    let mut app = setup_app();
    let origin = directory(&app, "sidecar-origin");
    let target = directory(&app, "sidecar-target");
    app.load_folder_and_finish_for_test(origin.clone());
    app.app.load_folder(target.clone());
    let (tx, result, token) = hold_scan_reply(&mut app);
    tx.send(result).unwrap();
    let generation = app.items_generation;
    let started = app.folder_pane_open_pending.as_ref().unwrap().started_at;
    app.activate_sidecar_restore_modal_for_test(origin.clone());
    let scans = folder_scan::caller_scan_count_for_test();
    let ctx = egui::Context::default();
    app.post_render_main_navigation(&ctx, Default::default());
    assert!(app.sidecar_restore_active());
    assert!(app.direct_navigation_scan_pending());
    assert!(Arc::ptr_eq(
        &app.folder_pane_open_pending.as_ref().unwrap().cancel,
        &token
    ));
    assert_eq!(
        app.folder_pane_open_pending.as_ref().unwrap().started_at,
        started
    );
    assert!(!token.load(Ordering::Relaxed));
    assert_eq!(app.current_folder.as_ref(), Some(&origin));
    assert_eq!(app.items_generation, generation);
    app.poll_sidecar_restore(&ctx);
    assert!(!app.sidecar_restore_active());
    app.finish_direct_navigation_scan_for_test();
    assert_eq!(folder_scan::caller_scan_count_for_test(), scans);
    assert_eq!(app.current_folder.as_ref(), Some(&target));
}

#[test]
fn section1853_native_backdrop_seam_adopts_ready_fullscreen_close_return() {
    let mut app = setup_app();
    let parent = directory(&app, "native-backdrop-parent");
    let book = parent.join("book.pdf");
    std::fs::write(&book, b"%PDF-1.4\n").unwrap();
    mount_pdf_reader(&mut app, &book);
    let scans = folder_scan::caller_scan_count_for_test();
    issue_close(&mut app);
    let (tx, result, _) = hold_scan_reply(&mut app);
    tx.send(result).unwrap();
    assert_eq!(app.fullscreen_idx, Some(0));
    assert_eq!(app.current_folder.as_ref(), Some(&book));
    // This is the native background navigation seam, not the embedded settle
    // helper. No native engine or host is needed to execute its ready-result tail.
    app.dispatch_native_backdrop_navigation(&egui::Context::default(), None, false, None);
    assert!(app.folder_pane_open_pending.is_none());
    assert_eq!(app.fullscreen_idx, None);
    assert_eq!(app.current_folder.as_ref(), Some(&parent));
    app.finish_page_identity_preparation_for_test();
    assert_eq!(folder_scan::caller_scan_count_for_test(), scans);
    assert!(matches!(&app.items[app.selected.unwrap()], GridItem::PdfFile(path) if path == &book));
    assert!(!app.fs_nav_is_locked());
}

#[test]
fn section1853_retired_context_bundle_drops_request_owned_fullscreen_and_selection() {
    let mut app = setup_app();
    let origin = directory(&app, "bundle-origin");
    let target = directory(&app, "bundle-target");
    app.load_folder_and_finish_for_test(origin);
    app.select_after_load = Some(AfterLoadSelection::Name("must-not-leak".into()));
    app.pending_auto_fs_open = true;
    app.app.load_folder(target);
    let (late_tx, result, token) = hold_scan_reply(&mut app);
    // Promote through the production registry boundary: the former main's real
    // bundle retains its accepted directory request until that context is retired.
    let former_main = app.stash_mounted_and_start_fresh("section1853_bundle_reservation");
    assert_ne!(app.viewer_context_main(), former_main);
    assert!(app.folder_pane_open_pending.is_none());
    assert!(!app.pending_auto_fs_open);
    assert!(app.select_after_load.is_none());
    app.with_viewer_context(former_main, |owner| {
        let pending = owner.folder_pane_open_pending.as_ref().unwrap();
        assert!(Arc::ptr_eq(&pending.cancel, &token));
        let FolderOpenScanPurpose::DirectNavigation(call) = &pending.purpose else {
            panic!("the former main bundle must retain its original request");
        };
        assert!(call.auto_fullscreen);
        assert_eq!(
            call.navigation
                .as_ref()
                .unwrap()
                .selection_after_load()
                .cloned(),
            Some(AfterLoadSelection::Name("must-not-leak".into()))
        );
        assert!(!owner.pending_auto_fs_open);
        assert!(owner.select_after_load.is_none());
    })
    .unwrap();
    app.retire_context(former_main, "section1853_bundle_retired", |_| ())
        .unwrap();
    assert!(token.load(Ordering::Relaxed));
    assert!(late_tx.send(result).is_err());
    assert!(!app.viewer_context_ids().contains(&former_main));
    let unrelated = directory(&app, "bundle-unrelated");
    image::RgbImage::new(2, 3)
        .save(unrelated.join("page.png"))
        .unwrap();
    app.load_folder_and_finish_for_test(unrelated.clone());
    assert_eq!(app.current_folder.as_ref(), Some(&unrelated));
    assert!(!app.pending_auto_fs_open);
    assert!(app.select_after_load.is_none());
    assert!(app.fullscreen_idx.is_none());
}

#[test]
fn section1853_grid_folder_facet_parent_child_grandchild_and_back_stay_physical() {
    let mut app = setup_app();
    app.active_quick_folder_slot = None;
    app.settings.detached_viewer_open_images_in_window = false;
    let parent = directory(&app, "facet-parent");
    let child = directory(&app, "facet-parent/child");
    let grandchild = directory(&app, "facet-parent/child/grandchild");
    app.load_folder_and_finish_for_test(parent.clone());
    let parent_route = FacetRoute::root(FacetScope::path(&parent));
    let child_route = parent_route.child(FacetScope::path(&child));
    let grandchild_route = child_route.child(FacetScope::path(&grandchild));
    let mut parent_filter = crate::settings::FacetFilter::default();
    parent_filter
        .kinds
        .insert(crate::settings::FacetItemKind::Folder);
    app.settings.facet_filter = parent_filter.clone();
    app.rebuild_visible_indices();
    let scans = folder_scan::caller_scan_count_for_test();
    let ctx = egui::Context::default();
    let child_index = app
        .items
        .iter()
        .position(|item| matches!(item, GridItem::Folder(path) if path == &child))
        .unwrap();
    let nav = app.grid_physical_navigation(child_index, child.clone(), false);
    app.post_render_main_navigation(
        &ctx,
        update_phases::MainFrameNavigation {
            grid_nav: Some(nav),
            ..Default::default()
        },
    );
    assert_eq!(app.settings.facet_filter, parent_filter);
    assert_eq!(app.facet_navigation.route(), &parent_route);
    app.finish_direct_navigation_scan_for_test();
    assert_eq!(app.facet_navigation.route(), &child_route);
    assert!(!app.settings.facet_filter.is_active());
    let mut child_filter = crate::settings::FacetFilter::default();
    child_filter.name_query = "grandchild".into();
    app.settings.facet_filter = child_filter.clone();
    app.rebuild_visible_indices();
    let grandchild_index = app
        .items
        .iter()
        .position(|item| matches!(item, GridItem::Folder(path) if path == &grandchild))
        .unwrap();
    let nav = app.grid_physical_navigation(grandchild_index, grandchild.clone(), false);
    app.post_render_main_navigation(
        &ctx,
        update_phases::MainFrameNavigation {
            grid_nav: Some(nav),
            ..Default::default()
        },
    );
    assert_eq!(app.settings.facet_filter, child_filter);
    app.finish_direct_navigation_scan_for_test();
    assert_eq!(app.facet_navigation.route(), &grandchild_route);
    assert!(!app.settings.facet_filter.is_active());
    for (target, route, filter) in [
        (child, child_route, child_filter),
        (parent, parent_route, parent_filter),
    ] {
        let nav = app.handle_grid_parent_folder_action().unwrap();
        app.post_render_main_navigation(
            &ctx,
            update_phases::MainFrameNavigation {
                keyboard_nav: Some(nav),
                ..Default::default()
            },
        );
        app.finish_direct_navigation_scan_for_test();
        assert_eq!(app.current_folder.as_ref(), Some(&target));
        assert_eq!(app.facet_navigation.route(), &route);
        assert_eq!(app.settings.facet_filter, filter);
    }
    assert_eq!(folder_scan::caller_scan_count_for_test(), scans);
}
