//! §1.805: admission, worker completion and replacement share the production boundaries.
use super::phase_c_support::setup_app;
use super::*;

impl App {
    /// Settle only this new worker route. Tests keep their original post-adoption assertions.
    pub(crate) fn finish_direct_navigation_scan_for_test(&mut self) {
        let ctx = egui::Context::default();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        while self.direct_navigation_scan_pending() {
            if let Some(ready) = self.poll_folder_pane_open(&ctx) {
                if let Some(ready) = self.resolve_main_folder_open_ready(&ctx, ready) {
                    let owner = ready.collection_owner.map_or(
                        OpenRequestOwner::Navigation,
                        OpenRequestOwner::CollectionGridPhysical,
                    );
                    self.open_direct_navigation_target_at_stage(
                        ready.path,
                        Some(ready.scan),
                        owner,
                        ready.navigation,
                        ready.epub_restore,
                        ready.restore_intent,
                        MainListRequestStage::Continuation,
                    );
                    self.finish_page_identity_preparation_for_test();
                }
                return;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "direct scan did not finish"
            );
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
    }
}

fn open(app: &mut App, path: PathBuf) {
    app.open_direct_navigation_target(
        path,
        None,
        OpenRequestOwner::Navigation,
        None,
        None,
        StartupListIntent::ExplicitList,
    );
}

fn folder(app: &super::phase_c_support::AppTestEnv, name: &str) -> PathBuf {
    let path = app.tmp.path().join(name);
    std::fs::create_dir_all(&path).unwrap();
    path
}

#[test]
fn direct_navigation_worker_dispatch_has_zero_caller_scans_and_adopts_once() {
    for producer in ["favorite", "address", "recent", "grid", "parent"] {
        let mut app = setup_app();
        app.active_quick_folder_slot = None;
        let target = folder(&app, "target");
        let origin = folder(
            &app,
            if producer == "parent" {
                "target/child"
            } else {
                "origin"
            },
        );
        std::fs::write(target.join("page.jpg"), []).unwrap();
        app.load_folder(origin.clone());
        app.finish_page_identity_preparation_for_test();
        let before = folder_scan::caller_scan_count_for_test();
        match producer {
            "favorite" => {
                let path = app.dispatch_favorite_navigation(target.clone()).unwrap();
                open(&mut app, path);
            }
            "address" => {
                app.dispatch_address_input(crate::ui_main::LocationJumpTarget::Physical(
                    target.clone(),
                ));
            }
            "grid" => {
                app.items = vec![GridItem::Folder(target.clone())];
                let nav = app
                    .accept_grid_container_open(&egui::Context::default(), 0)
                    .unwrap();
                let crate::ui_main::AddressBarNav::Direct(path, intent) = nav else {
                    panic!("physical folder route");
                };
                app.open_direct_navigation_target(
                    path,
                    None,
                    OpenRequestOwner::Navigation,
                    None,
                    None,
                    intent,
                );
            }
            "parent" => {
                let nav = app.handle_grid_parent_folder_action().unwrap();
                let crate::ui_main::AddressBarNav::Direct(path, intent) = nav else {
                    panic!("physical parent route");
                };
                app.open_direct_navigation_target(
                    path,
                    None,
                    OpenRequestOwner::Navigation,
                    None,
                    None,
                    intent,
                );
            }
            _ => open(&mut app, target.clone()),
        }
        assert_eq!(
            folder_scan::caller_scan_count_for_test(),
            before,
            "{producer}"
        );
        assert_eq!(app.current_folder.as_ref(), Some(&origin));
        assert!(app.direct_navigation_scan_pending(), "{producer}");
        assert!(!app.pending_auto_fs_open);
        assert!(!app.grid_item_input_allowed());
        app.finish_direct_navigation_scan_for_test();
        assert_eq!(folder_scan::caller_scan_count_for_test(), before);
        assert_eq!(app.current_folder.as_ref(), Some(&target));
        assert_eq!(app.address, target.to_string_lossy());
        assert_eq!(app.folder_nav_back_stack.len(), 1, "{producer}");
        assert!(
            app.items
                .iter()
                .any(|item| matches!(item, GridItem::Image(_)))
        );
        assert_eq!(app.selected, Some(0));
        assert!(app.grid_item_input_allowed());
    }
}

#[test]
fn direct_navigation_worker_replacement_and_tree_input_contract() {
    let mut app = setup_app();
    let a = folder(&app, "a");
    let b = folder(&app, "b");
    let c = folder(&app, "c");
    open(&mut app, a);
    let cancelled = app
        .folder_pane_open_pending
        .as_ref()
        .unwrap()
        .cancel
        .clone();
    open(&mut app, b.clone());
    assert!(cancelled.load(Ordering::Relaxed));
    let cancelled = app
        .folder_pane_open_pending
        .as_ref()
        .unwrap()
        .cancel
        .clone();
    app.start_folder_pane_open(c.clone());
    assert!(cancelled.load(Ordering::Relaxed));
    assert!(
        app.grid_item_input_allowed(),
        "pane scan keeps released item input"
    );
    let cancelled = app
        .folder_pane_open_pending
        .as_ref()
        .unwrap()
        .cancel
        .clone();
    open(&mut app, b.clone());
    assert!(cancelled.load(Ordering::Relaxed));
    app.finish_direct_navigation_scan_for_test();
    assert_eq!(app.current_folder.as_ref(), Some(&b));
}

#[test]
fn direct_navigation_worker_wait_overlay_grace_and_escape_keep_request() {
    let mut app = setup_app();
    let target = folder(&app, "target");
    app.stack_mode_requested = true;
    open(&mut app, target);
    assert!(app.direct_navigation_wait_overlay_delay().is_some());
    app.folder_pane_open_pending.as_mut().unwrap().started_at =
        std::time::Instant::now() - std::time::Duration::from_millis(501);
    assert!(app.direct_navigation_wait_overlay_delay().is_none());
    let ctx = egui::Context::default();
    for key in [egui::Key::Escape, egui::Key::Enter, egui::Key::Space] {
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
        let nav = app.handle_keyboard(&ctx);
        ctx.end_pass().drop_without_applying_deltas();
        assert!(nav.is_none());
        assert!(app.stack_mode_requested);
        assert!(app.direct_navigation_scan_pending());
        assert!(app.fullscreen_idx.is_none());
    }
}

#[test]
fn direct_navigation_worker_blocks_old_folder_image_zip_pdf_accept_handlers() {
    let mut app = setup_app();
    let target = folder(&app, "target");
    open(&mut app, target.clone());
    let ctx = egui::Context::default();
    for item in [
        GridItem::Folder(target.clone()),
        GridItem::Image(target.join("page.jpg")),
        GridItem::ZipFile(target.join("book.zip")),
        GridItem::PdfFile(target.join("book.pdf")),
    ] {
        app.items = vec![item];
        app.selected = Some(0);
        app.visible_indices = vec![0];
        app.thumbnails = vec![ThumbnailState::Pending];
        app.image_metas = vec![None];
        assert!(app.handle_gamepad_grid_accept(&ctx).is_none());
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
        assert!(app.handle_keyboard(&ctx).is_none());
        ctx.end_pass().drop_without_applying_deltas();
        assert!(app.direct_navigation_scan_pending());
        assert!(app.fullscreen_idx.is_none());
        assert!(app.zip_enumerate_pending.is_none());
    }
}

#[test]
fn direct_navigation_worker_dfs_admission_cancels_only_replaceable_scan() {
    for purpose in [
        FolderOpenScanPurpose::PaneNavigation,
        FolderOpenScanPurpose::DirectNavigation {
            restore_intent: StartupListIntent::ExplicitList,
            collection_owner: None,
            auto_fullscreen: false,
        },
        FolderOpenScanPurpose::DetachedFolder,
    ] {
        let mut app = setup_app();
        let target = folder(&app, "target");
        let replaceable = !matches!(purpose, FolderOpenScanPurpose::DetachedFolder);
        app.start_folder_open_scan(target.clone(), purpose);
        let cancel = app
            .folder_pane_open_pending
            .as_ref()
            .unwrap()
            .cancel
            .clone();
        app.fs_nav_locked_gen = Some(app.items_generation);
        app.fs_holdover_tex = Some(FsHoldover::NavigationSequence(FsNavigationSequence {
            previous: None,
            chrome: FsNavigationChromeContinuation::None,
            purpose: FsNavigationPurpose::Ordinary,
            opened_at: std::time::Instant::now(),
            target: FsNavigationSequenceTarget::FolderItems {
                accepted_generation: app.items_generation,
            },
        }));
        app.start_folder_nav(target, true, FolderNavMode::Fullscreen);
        assert_eq!(cancel.load(Ordering::Relaxed), replaceable);
        assert!(app.folder_nav_pending.is_some());
        assert!(
            app.fs_nav_locked_gen.is_some(),
            "accepted DFS owns the new lock"
        );
        assert!(
            app.fs_holdover_tex.is_some(),
            "cancelling the scan must preserve the new DFS sequence"
        );
        app.cancel_pending_folder_nav();
    }
}

#[test]
fn direct_navigation_worker_admission_retires_dfs_and_taken_result_sequence() {
    for result_taken in [false, true] {
        let mut app = setup_app();
        let target = folder(&app, "target");
        let (tx, rx) = mpsc::channel::<FolderNavThreadResult>();
        let cancel = Arc::new(AtomicBool::new(false));
        if !result_taken {
            app.folder_nav_pending = Some(FolderNavPending {
                cancel: cancel.clone(),
                rx,
                forward: true,
                mode: FolderNavMode::Fullscreen,
            });
        }
        app.pending_folder_nav_steps = 3;
        app.fs_nav_locked_gen = Some(app.items_generation);
        app.fs_holdover_tex = Some(FsHoldover::NavigationSequence(FsNavigationSequence {
            previous: None,
            chrome: FsNavigationChromeContinuation::None,
            purpose: FsNavigationPurpose::Ordinary,
            opened_at: std::time::Instant::now(),
            target: FsNavigationSequenceTarget::FolderItems {
                accepted_generation: app.items_generation,
            },
        }));
        open(&mut app, target);
        assert_eq!(cancel.load(Ordering::Relaxed), !result_taken);
        assert!(app.folder_nav_pending.is_none());
        assert_eq!(app.pending_folder_nav_steps, 0);
        assert!(app.fs_holdover_tex.is_none());
        assert!(app.fs_nav_locked_gen.is_none());
        drop(tx);
    }
}

#[test]
fn direct_navigation_worker_auto_fullscreen_reservation_dies_with_request() {
    let mut app = setup_app();
    let a = folder(&app, "a");
    let b = folder(&app, "b");
    app.pending_auto_fs_open = true;
    open(&mut app, a);
    assert!(!app.pending_auto_fs_open);
    assert!(matches!(
        app.folder_pane_open_pending.as_ref().unwrap().purpose,
        FolderOpenScanPurpose::DirectNavigation {
            auto_fullscreen: true,
            ..
        }
    ));
    app.cancel_folder_pane_open(PaneOpenRestoreExit::Abandoned);
    open(&mut app, b);
    assert!(matches!(
        app.folder_pane_open_pending.as_ref().unwrap().purpose,
        FolderOpenScanPurpose::DirectNavigation {
            auto_fullscreen: false,
            ..
        }
    ));
    app.finish_direct_navigation_scan_for_test();
    assert!(!app.pending_auto_fs_open);
    assert!(app.fullscreen_idx.is_none());
}

#[test]
fn direct_navigation_worker_stale_failure_and_disconnect_preserve_listing() {
    for exit in ["stale", "failure", "disconnect"] {
        let mut app = setup_app();
        let origin = folder(&app, "origin");
        app.load_folder(origin.clone());
        let missing = app.tmp.path().join("missing");
        app.pending_auto_fs_open = true;
        open(&mut app, missing);
        if exit == "stale" {
            app.quick_folder_switch_sequence += 1;
        }
        if exit == "disconnect" {
            let (tx, rx) = mpsc::channel();
            drop(tx);
            app.folder_pane_open_pending.as_mut().unwrap().rx = rx;
        }
        app.finish_direct_navigation_scan_for_test();
        assert_eq!(app.current_folder.as_ref(), Some(&origin));
        assert!(!app.pending_auto_fs_open);
        assert!(app.folder_pane_open_pending.is_none());
        if exit == "failure" {
            assert!(app.fs_feedback_toast.is_some());
        }
    }
}

#[cfg(feature = "test-script")]
#[test]
fn direct_navigation_worker_counts_admission_once_including_cancelled_request() {
    let mut app = setup_app();
    let a = folder(&app, "a");
    let b = folder(&app, "b");
    let before = app.test_script_folder_load_requests;
    open(&mut app, a);
    assert_eq!(app.test_script_folder_load_requests, before + 1);
    open(&mut app, b);
    assert_eq!(app.test_script_folder_load_requests, before + 2);
    app.finish_direct_navigation_scan_for_test();
    assert_eq!(app.test_script_folder_load_requests, before + 2);
}

#[test]
fn direct_navigation_worker_saved_group_refusal_preserves_scan_and_acceptance_replaces_it() {
    let mut app = setup_app();
    let target = folder(&app, "target");
    let book = folder(&app, "books/First");
    app.settings.book_root = Some(book.parent().unwrap().to_path_buf());
    app.settings.active_book_name = "First".into();
    app.book_shelf.snapshot = Some(Vec::new().into());
    open(&mut app, target);
    let cancel = app
        .folder_pane_open_pending
        .as_ref()
        .unwrap()
        .cancel
        .clone();
    let ctx = egui::Context::default();
    app.apply_saved_group_key_action(&ctx, crate::keymap::KeyAction::GridOpenBook1);
    assert!(
        app.direct_navigation_scan_pending(),
        "known missing slot must not retire scan"
    );
    assert!(!cancel.load(Ordering::Relaxed));
    app.apply_saved_group_key_action(&ctx, crate::keymap::KeyAction::GridOpenActiveBook);
    assert!(cancel.load(Ordering::Relaxed));
    assert!(app.folder_pane_open_pending.is_none());
    assert!(app.saved_group_open_request_id_for_test().is_some());
}

#[test]
fn direct_navigation_worker_saved_group_waits_for_new_book_list_before_rejecting_slot() {
    let mut app = setup_app();
    let target = folder(&app, "target");
    app.book_shelf.snapshot = Some(Vec::new().into());
    let (_sender, rx) = mpsc::channel();
    app.book_op_pending = Some(crate::books::BookOpPending {
        rx,
        intent: crate::books::BookOpIntent::List { navigation: None },
        shelf_scope: None,
    });
    open(&mut app, target);
    let cancel = app
        .folder_pane_open_pending
        .as_ref()
        .unwrap()
        .cancel
        .clone();
    app.apply_saved_group_key_action(
        &egui::Context::default(),
        crate::keymap::KeyAction::GridOpenBook1,
    );
    assert!(cancel.load(Ordering::Relaxed));
    assert!(
        app.saved_group_open_request_id_for_test().is_some(),
        "pending list can contain the requested slot"
    );
}

#[test]
fn direct_navigation_worker_completion_refusal_drops_fullscreen_for_direct_and_grid_scan() {
    let mut app = setup_app();
    let target = folder(&app, "target");
    let ctx = egui::Context::default();
    app.pending_auto_fs_open = true;
    open(&mut app, target.clone());
    let pending = app.folder_pane_open_pending.take().unwrap();
    pending.cancel.store(true, Ordering::Relaxed);
    let scan = folder_scan::scan_directory_with_settings(&target, &app.settings).unwrap();
    let ready = app
        .resolve_main_folder_open_ready(
            &ctx,
            FolderPaneOpenReady {
                path: target.clone(),
                scan: Ok(scan),
                purpose: pending.purpose,
                navigation: pending.navigation,
                epub_restore: None,
            },
        )
        .unwrap();
    assert!(app.pending_auto_fs_open);
    // A newly opened document modal refuses the continuation at the existing claim boundary.
    app.epub_batch_convert = Some(
        crate::ui_dialogs::epub_batch_convert::EpubBatchPending::completed_for_test(
            target.join("book.epub"),
            Ok(crate::epub_convert::SavedPdf {
                path: target.join("book.pdf"),
                reused_cache: false,
                user_data_errors: vec![],
                metadata: Default::default(),
            }),
        ),
    );
    app.open_direct_navigation_target_at_stage(
        ready.path,
        Some(ready.scan),
        OpenRequestOwner::Navigation,
        ready.navigation,
        None,
        ready.restore_intent,
        MainListRequestStage::Continuation,
    );
    assert!(!app.pending_auto_fs_open);
    // GridFolderCandidate also reserves via the resolver and re-enters this same consumer.
    app.pending_auto_fs_open = true;
    let scan = folder_scan::scan_directory_with_settings(&target, &app.settings).unwrap();
    app.open_direct_navigation_target_at_stage(
        target,
        Some(scan),
        OpenRequestOwner::Navigation,
        None,
        None,
        StartupListIntent::ClassifyFolder,
        MainListRequestStage::Continuation,
    );
    assert!(!app.pending_auto_fs_open);
    // A directory bearing a classified suffix can be refused before the classified
    // consumer. Its completed-scan reservation must end at that admission boundary too.
    let target = folder(&app, "classified-folder.epub");
    let scan = folder_scan::scan_directory_with_settings(&target, &app.settings).unwrap();
    app.pending_auto_fs_open = true;
    app.open_direct_navigation_target_at_stage(
        target,
        Some(scan),
        OpenRequestOwner::Navigation,
        None,
        None,
        StartupListIntent::ClassifyFolder,
        MainListRequestStage::Continuation,
    );
    assert!(!app.pending_auto_fs_open);
}

#[test]
fn direct_navigation_worker_original_restore_intent_and_navigation_are_not_recaptured() {
    let mut app = setup_app();
    let target = folder(&app, "target");
    let proof = app.copied_destination_source_proof();
    let navigation = app.capture_main_list_navigation(MainHistoryOperation::SameLocation, proof);
    app.open_direct_navigation_target_at_stage(
        target,
        None,
        OpenRequestOwner::Navigation,
        Some(navigation.clone()),
        None,
        StartupListIntent::PreservePresentation,
        MainListRequestStage::Admission,
    );
    let pending = app.folder_pane_open_pending.as_ref().unwrap();
    let original = pending.navigation.as_ref().unwrap();
    assert_eq!(original.history, navigation.history);
    assert_eq!(
        original.source_slot_switch_sequence,
        navigation.source_slot_switch_sequence
    );
    assert!(matches!(
        pending.purpose,
        FolderOpenScanPurpose::DirectNavigation {
            restore_intent: StartupListIntent::PreservePresentation,
            ..
        }
    ));
    app.finish_direct_navigation_scan_for_test();
    assert!(app.folder_nav_back_stack.is_empty());
}

#[test]
fn direct_navigation_worker_visibility_refusal_keeps_old_scan() {
    let mut app = setup_app();
    let a = folder(&app, "a");
    open(&mut app, a);
    let cancel = app
        .folder_pane_open_pending
        .as_ref()
        .unwrap()
        .cancel
        .clone();
    app.settings
        .file_type_visibility
        .excluded_extensions
        .insert("zip".into());
    let blocked = app.tmp.path().join("blocked.zip");
    let before = folder_scan::caller_scan_count_for_test();
    open(&mut app, blocked);
    assert_eq!(folder_scan::caller_scan_count_for_test(), before);
    assert!(!cancel.load(Ordering::Relaxed));
    assert!(app.direct_navigation_scan_pending());
}

#[test]
fn direct_navigation_worker_snapshot_refusal_keeps_existing_owner_and_history() {
    let mut app = setup_app();
    let origin = folder(&app, "origin");
    let outside = folder(&app, "outside");
    std::fs::write(origin.join("page.jpg"), []).unwrap();
    app.load_folder(origin.clone());
    app.finish_page_identity_preparation_for_test();
    app.activate_snapshot(crate::snapshot::SnapshotSourceLabel::Mixed);
    assert!(app.is_snapshot_active());
    app.start_folder_pane_open(origin);
    let cancel = app
        .folder_pane_open_pending
        .as_ref()
        .unwrap()
        .cancel
        .clone();
    let history = app.folder_nav_history_snapshot();
    let before = folder_scan::caller_scan_count_for_test();
    open(&mut app, outside);
    assert!(!cancel.load(Ordering::Relaxed));
    assert!(app.folder_pane_open_pending.is_some());
    assert_eq!(app.folder_nav_history_snapshot(), history);
    assert_eq!(folder_scan::caller_scan_count_for_test(), before);
}

#[test]
fn direct_navigation_worker_history_navigation_replaces_pending_scan() {
    let mut app = setup_app();
    app.active_quick_folder_slot = None;
    let origin = folder(&app, "origin");
    let a = folder(&app, "a");
    let back = folder(&app, "back");
    app.load_folder(origin);
    app.folder_nav_back_stack = vec![FolderNavHistoryTarget::Path(back.clone()).into()];
    open(&mut app, a);
    let cancel = app
        .folder_pane_open_pending
        .as_ref()
        .unwrap()
        .cancel
        .clone();
    app.dispatch_main_folder_history_input(FolderHistoryDirection::Back);
    assert!(cancel.load(Ordering::Relaxed));
    super::phase_c_folder_nav_history_tests::finish_staged_physical_history_for_test(&mut app);
    assert_eq!(app.current_folder.as_ref(), Some(&back));
}

#[test]
fn direct_navigation_worker_history_wait_escape_does_not_toggle_stack() {
    let mut app = setup_app();
    app.active_quick_folder_slot = None;
    let origin = folder(&app, "origin");
    let target = folder(&app, "target");
    app.load_folder(origin);
    app.stack_mode_requested = true;
    app.folder_nav_back_stack = vec![FolderNavHistoryTarget::Path(target).into()];
    app.dispatch_main_folder_history_input(FolderHistoryDirection::Back);
    assert!(app.location_navigation_pending());
    assert!(app.direct_navigation_wait_overlay_delay().is_none());
    let ctx = egui::Context::default();
    ctx.begin_pass(egui::RawInput {
        events: vec![egui::Event::Key {
            key: egui::Key::Escape,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        }],
        ..Default::default()
    });
    assert!(app.handle_keyboard(&ctx).is_none());
    ctx.end_pass().drop_without_applying_deltas();
    assert!(app.stack_mode_requested);
    assert!(app.location_navigation_pending());
}

#[test]
fn direct_navigation_worker_block_reason_keeps_non_item_shortcuts_available() {
    use crate::filename_stack_ui::GridItemInputBlock;
    for block in [
        GridItemInputBlock::LocationNavigation,
        GridItemInputBlock::BookStructuralOperation,
        GridItemInputBlock::ImageFolderZip,
        GridItemInputBlock::StackRefresh,
    ] {
        for action in [
            "HelpShowContextShortcuts",
            "GridOpenPreferences",
            "GridOpenOperationCustomize",
            "GridToggleMaximize",
        ] {
            let mut app = setup_app();
            let section = crate::keymap::KeyAction::parse_ini_name(action)
                .unwrap()
                .context()
                .ini_name();
            app.keymap =
                crate::keymap::Keymap::from_ini_str(&format!("[{section}]\n{action} = F13\n"));
            let ctx = egui::Context::default();
            ctx.begin_pass(egui::RawInput {
                events: vec![egui::Event::Key {
                    key: egui::Key::F13,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: egui::Modifiers::NONE,
                }],
                ..Default::default()
            });
            assert!(!app.shortcuts_blocked_by_text_input(&ctx));
            assert!(
                app.handle_stack_refresh_keyboard(&ctx, true, 0, 0, block)
                    .is_none()
            );
            let output = ctx.end_pass();
            let maximized = output
                .viewport_output
                .get(&egui::ViewportId::ROOT)
                .is_some_and(|viewport| {
                    viewport
                        .commands
                        .iter()
                        .any(|command| matches!(command, egui::ViewportCommand::Maximized(true)))
                });
            output.drop_without_applying_deltas();
            match action {
                "HelpShowContextShortcuts" => assert!(app.show_context_shortcuts_help),
                "GridOpenPreferences" => assert!(app.show_preferences),
                "GridOpenOperationCustomize" => assert!(app.show_operation_customize),
                _ => assert!(maximized),
            }
        }
    }
}

#[test]
fn direct_navigation_worker_route_excludes_virtual_and_sibling_contexts() {
    let mut app = setup_app();
    let path = folder(&app, "folder.zip");
    let proof = app.copied_destination_source_proof();
    let navigation =
        Some(app.capture_main_list_navigation(MainHistoryOperation::SameLocation, proof));
    assert!(!app.direct_navigation_uses_worker_scan(
        &path,
        None,
        &None,
        &OpenRequestOwner::Navigation,
        &navigation,
        DirectoryScanRoute::Worker
    ));
    assert!(app.direct_navigation_uses_worker_scan(
        &path,
        Some(OpenPathKind::Directory),
        &None,
        &OpenRequestOwner::Navigation,
        &navigation,
        DirectoryScanRoute::Worker
    ));
    assert!(!app.direct_navigation_uses_worker_scan(
        &path,
        Some(OpenPathKind::Directory),
        &None,
        &OpenRequestOwner::Navigation,
        &navigation,
        DirectoryScanRoute::UiThread
    ));
    let sibling = app.build_window_context_for_test(1805, |_| {});
    app.with_viewer_context(sibling, |app| {
        assert!(!app.direct_navigation_uses_worker_scan(
            &path,
            Some(OpenPathKind::Directory),
            &None,
            &OpenRequestOwner::Navigation,
            &navigation,
            DirectoryScanRoute::Worker
        ));
    })
    .unwrap();
}

#[test]
fn direct_navigation_worker_quick_folder_round_trip_retires_old_epoch() {
    let mut app = setup_app();
    let a = folder(&app, "a");
    let b = folder(&app, "b");
    let target = folder(&app, "target");
    app.load_folder(a.clone());
    app.set_quick_folder_slot_target(QuickFolderSlotId::A, a.clone());
    app.set_quick_folder_slot_target(QuickFolderSlotId::B, b.clone());
    app.active_quick_folder_slot = Some(QuickFolderSlotId::A);
    open(&mut app, target.clone());
    let cancel = app
        .folder_pane_open_pending
        .as_ref()
        .unwrap()
        .cancel
        .clone();
    assert!(app.start_quick_folder_slot_switch(QuickFolderSlotId::B, b));
    assert!(cancel.load(Ordering::Relaxed));
    super::phase_c_folder_nav_history_tests::finish_staged_physical_history_for_test(&mut app);
    assert!(app.start_quick_folder_slot_switch(QuickFolderSlotId::A, a.clone()));
    super::phase_c_folder_nav_history_tests::finish_staged_physical_history_for_test(&mut app);
    assert_eq!(app.current_folder.as_ref(), Some(&a));
    assert!(app.folder_pane_open_pending.is_none());
    assert_ne!(app.current_folder.as_ref(), Some(&target));
}

#[test]
fn direct_navigation_worker_remote_acquisition_retires_scan() {
    let mut app = setup_app();
    let target = folder(&app, "target");
    open(&mut app, target.clone());
    let cancel = app
        .folder_pane_open_pending
        .as_ref()
        .unwrap()
        .cancel
        .clone();
    let handle = crate::remote_ipc::session::SessionHandle::new();
    app.set_remote_session_handle(handle.clone());
    handle.acquire(mimageviewer_ipc::SessionAcquireRequest {
        client_id: "test-1805".into(),
        peer: mimageviewer_ipc::SessionPeerInfo {
            connection_kind: mimageviewer_ipc::SessionConnectionKind::Direct,
            device_name: None,
        },
    });
    app.poll_remote_session(&egui::Context::default());
    assert!(cancel.load(Ordering::Relaxed));
    assert!(app.folder_pane_open_pending.is_none());
    assert_ne!(app.current_folder.as_ref(), Some(&target));
}
