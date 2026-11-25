//! §1.805: admission, worker completion and replacement share the production boundaries.
use super::phase_c_support::setup_app;
use super::*;

impl App {
    /// Settle only this new worker route. Tests keep their original post-adoption assertions.
    pub(crate) fn finish_direct_navigation_scan_for_test(&mut self) {
        let ctx = egui::Context::default();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        while self.direct_navigation_scan_pending() {
            self.poll_sidecar_restore(&ctx);
            self.poll_direct_navigation_scan_for_test(&ctx);
            if !self.direct_navigation_scan_pending() {
                self.finish_page_identity_preparation_for_test();
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
    app.open_direct_navigation_target_at_stage(
        path,
        None,
        OpenRequestOwner::Navigation,
        None,
        None,
        StartupListIntent::ExplicitList,
        MainListRequestStage::Admission,
        DirectoryScanRoute::Worker,
    );
}

fn folder(app: &super::phase_c_support::AppTestEnv, name: &str) -> PathBuf {
    let path = app.tmp.path().join(name);
    std::fs::create_dir_all(&path).unwrap();
    path
}

/// Preserve the actual worker reply, holding it ready without adopting it. Only
/// the transport is substituted; the following frame still polls the real arbiter.
fn make_scan_ready(app: &mut App) -> Arc<AtomicBool> {
    let pending = app.folder_pane_open_pending.as_mut().unwrap();
    let scan = pending
        .rx
        .recv_timeout(std::time::Duration::from_secs(30))
        .unwrap();
    let cancel = pending.cancel.clone();
    let (tx, rx) = mpsc::channel();
    tx.send(scan).unwrap();
    pending.rx = rx;
    cancel
}

#[test]
fn direct_navigation_worker_ready_scan_yields_to_same_frame_navigation_inputs() {
    for input in ["favorite", "address", "tree"] {
        let mut app = setup_app();
        app.active_quick_folder_slot = None;
        let origin = folder(&app, "origin");
        let a = folder(&app, "a");
        let b = folder(&app, "b");
        app.load_folder(origin.clone());
        app.finish_page_identity_preparation_for_test();
        open(&mut app, a.clone());
        let cancel = make_scan_ready(&mut app);
        let generation = app.items_generation;
        let mut frame = update_phases::MainFrameNavigation::default();
        match input {
            "favorite" => frame.toolbar_fav_nav = Some(b.clone()),
            "address" => {
                frame.address_nav = Some(crate::ui_main::AddressBarNav::AddressEnterPhysical(
                    b.clone(),
                ))
            }
            "tree" => {
                frame.folder_pane_nav =
                    Some(crate::folder_pane::FolderPaneActivation::Open(b.clone()));
            }
            _ => unreachable!(),
        }
        let ctx = egui::Context::default();
        app.post_render_main_navigation(&ctx, frame);
        if input != "tree" {
            assert!(cancel.load(Ordering::Relaxed), "{input}");
        }
        assert_eq!(
            app.current_folder.as_ref(),
            Some(&origin),
            "A must not be installed"
        );
        assert_eq!(app.items_generation, generation);
        assert_eq!(app.folder_pane_open_pending.as_ref().unwrap().path, b);
        make_scan_ready(&mut app);
        app.post_render_main_navigation(&ctx, update_phases::MainFrameNavigation::default());
        app.finish_page_identity_preparation_for_test();
        assert_eq!(app.current_folder.as_ref(), Some(&b), "{input}");
        assert_eq!(app.folder_nav_back_stack.len(), 1);
        assert_eq!(
            app.folder_nav_back_stack[0].location,
            FolderNavHistoryTarget::Path(origin)
        );
    }
}

#[test]
fn direct_navigation_worker_tree_completion_keeps_suffix_classification() {
    for suffix in ["epub", "7z"] {
        let mut app = setup_app();
        let target = folder(&app, &format!("tree.{suffix}"));
        app.start_folder_open_scan(target, FolderOpenScanPurpose::PaneNavigation);
        make_scan_ready(&mut app);
        app.post_render_main_navigation(
            &egui::Context::default(),
            update_phases::MainFrameNavigation::default(),
        );
        assert!(
            app.top_level_grid_view.open_path_classification().is_some(),
            "{suffix}"
        );
        assert!(!app.direct_navigation_scan_pending());
    }
}

#[test]
fn direct_navigation_worker_admission_after_real_dfs_poll_retires_sequence() {
    let mut app = setup_app();
    app.active_quick_folder_slot = None;
    let a = folder(&app, "a");
    let b = folder(&app, "b");
    let c = folder(&app, "c");
    for path in [&a, &b, &c] {
        image::RgbImage::new(2, 3)
            .save(path.join("page.png"))
            .unwrap();
    }
    app.load_folder(a.clone());
    app.finish_page_identity_preparation_for_test();
    app.capture_fs_nav_holdover(0);
    app.start_folder_nav(a.clone(), true, FolderNavMode::Fullscreen);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    let result = loop {
        if let Some(result) = app.poll_folder_nav() {
            break result;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "actual DFS did not finish"
        );
        std::thread::sleep(std::time::Duration::from_millis(1));
    };
    assert!(result.target.is_some());
    assert!(app.folder_nav_pending.is_none());
    assert!(app.fs_nav_locked_gen.is_some());
    assert!(app.fs_holdover_tex.is_some());
    // This is the poll/tail boundary at which a newer higher-priority input wins
    // instead of applying the old DFS candidate.
    app.post_render_main_navigation(
        &egui::Context::default(),
        update_phases::MainFrameNavigation {
            toolbar_fav_nav: Some(c.clone()),
            ..Default::default()
        },
    );
    assert_eq!(app.current_folder.as_ref(), Some(&a));
    assert!(app.fs_nav_locked_gen.is_none());
    assert!(app.fs_holdover_tex.is_none());
    app.finish_direct_navigation_scan_for_test();
    assert_eq!(app.current_folder.as_ref(), Some(&c));
}

#[test]
fn direct_navigation_worker_admission_does_not_take_global_fullscreen_reservation() {
    let mut app = setup_app();
    let target = folder(&app, "target");
    app.pending_auto_fs_open = true;
    open(&mut app, target);
    assert!(app.pending_auto_fs_open);
    assert!(matches!(
        app.folder_pane_open_pending.as_ref().unwrap().purpose,
        FolderOpenScanPurpose::DirectNavigation
    ));
    app.cancel_folder_pane_open(PaneOpenRestoreExit::Abandoned);
    assert!(
        app.pending_auto_fs_open,
        "global reservation is outside this request's ownership"
    );
}

#[test]
fn direct_navigation_worker_dispatch_has_zero_caller_scans_and_adopts_once() {
    use crate::ui_main::AddressBarNav;
    for producer in [
        "toolbar", "menu", "keyboard", "gamepad", "ring", "address", "recent",
    ] {
        let mut app = setup_app();
        app.active_quick_folder_slot = None;
        let target = folder(&app, "target");
        let origin = folder(&app, "origin");
        std::fs::write(target.join("page.jpg"), []).unwrap();
        app.load_folder(origin.clone());
        app.finish_page_identity_preparation_for_test();
        let before = folder_scan::caller_scan_count_for_test();
        let mut frame = update_phases::MainFrameNavigation::default();
        match producer {
            "toolbar" => frame.toolbar_fav_nav = Some(target.clone()),
            "menu" => {
                frame.fav_nav = Some(crate::ui_main::LocationJumpTarget::Physical(target.clone()))
            }
            "keyboard" => frame.keyboard_nav = Some(AddressBarNav::WorkerFavorite(target.clone())),
            "gamepad" => frame.gamepad_nav = Some(AddressBarNav::WorkerFavorite(target.clone())),
            "ring" => app.mouse_ring_nav = Some(AddressBarNav::WorkerFavorite(target.clone())),
            "address" => {
                frame.address_nav = Some(AddressBarNav::AddressEnterPhysical(target.clone()))
            }
            "recent" => frame.address_nav = Some(AddressBarNav::RecentFolder(target.clone())),
            _ => unreachable!(),
        }
        app.post_render_main_navigation(&egui::Context::default(), frame);
        assert_eq!(
            folder_scan::caller_scan_count_for_test(),
            before,
            "{producer}"
        );
        assert_eq!(app.current_folder.as_ref(), Some(&origin));
        assert!(app.direct_navigation_scan_pending(), "{producer}");
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
fn direct_navigation_worker_unmarked_producers_keep_synchronous_adoption() {
    use crate::ui_main::{AddressBarNav, LocationJumpTarget};
    for producer in [
        "dialog",
        "grid",
        "parent",
        "favorite",
        "address-input",
        "location-jump",
        "wrapper",
    ] {
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
        app.load_folder(origin);
        app.finish_page_identity_preparation_for_test();
        let before = folder_scan::caller_scan_count_for_test();
        let mut frame = update_phases::MainFrameNavigation::default();
        match producer {
            "dialog" => frame.open_folder_nav = Some(target.clone()),
            "grid" => {
                app.items = vec![GridItem::Folder(target.clone())];
                frame.grid_nav = app.accept_grid_container_open(&egui::Context::default(), 0);
            }
            "parent" => frame.keyboard_nav = app.handle_grid_parent_folder_action(),
            "favorite" => frame.keyboard_nav = Some(AddressBarNav::Favorite(target.clone())),
            "address-input" => {
                frame.address_nav = Some(AddressBarNav::AddressInput(LocationJumpTarget::Physical(
                    target.clone(),
                )))
            }
            "location-jump" => {
                frame.address_nav = Some(AddressBarNav::LocationJump(LocationJumpTarget::Physical(
                    target.clone(),
                )))
            }
            "wrapper" => app.open_direct_navigation_target(
                target.clone(),
                None,
                OpenRequestOwner::Navigation,
                None,
                None,
                StartupListIntent::ExplicitList,
            ),
            _ => unreachable!(),
        }
        if producer != "wrapper" {
            app.post_render_main_navigation(&egui::Context::default(), frame);
        }
        assert_eq!(
            folder_scan::caller_scan_count_for_test(),
            before + 1,
            "{producer}"
        );
        assert_eq!(app.current_folder.as_ref(), Some(&target), "{producer}");
        assert!(!app.direct_navigation_scan_pending(), "{producer}");
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
    assert_eq!(
        app.grid_item_input_block(),
        Some(crate::filename_stack_ui::GridItemInputBlock::DirectNavigationScan)
    );
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
        FolderOpenScanPurpose::DirectNavigation,
        FolderOpenScanPurpose::DetachedFolder,
    ] {
        let mut app = setup_app();
        let target = folder(&app, "target");
        let replaceable = matches!(purpose, FolderOpenScanPurpose::DirectNavigation);
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
fn direct_navigation_worker_saved_group_keeps_tree_scan_refusal() {
    let mut app = setup_app();
    let target = folder(&app, "target");
    let book = folder(&app, "books/First");
    app.settings.book_root = Some(book.parent().unwrap().to_path_buf());
    app.settings.active_book_name = "First".into();
    app.book_shelf.snapshot = Some(Vec::new().into());
    app.start_folder_pane_open(target);
    let cancel = app
        .folder_pane_open_pending
        .as_ref()
        .unwrap()
        .cancel
        .clone();
    app.apply_saved_group_key_action(
        &egui::Context::default(),
        crate::keymap::KeyAction::GridOpenActiveBook,
    );
    assert!(!cancel.load(Ordering::Relaxed));
    assert!(app.folder_pane_open_pending.is_some());
    assert!(app.saved_group_open_request_id_for_test().is_none());
}

#[test]
fn direct_navigation_worker_stale_failure_and_disconnect_preserve_listing() {
    for exit in ["stale", "failure", "disconnect"] {
        let mut app = setup_app();
        let origin = folder(&app, "origin");
        app.load_folder(origin.clone());
        let missing = app.tmp.path().join("missing");
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
fn direct_navigation_worker_original_navigation_is_not_recaptured() {
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
        StartupListIntent::ExplicitList,
        MainListRequestStage::Admission,
        DirectoryScanRoute::Worker,
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
        FolderOpenScanPurpose::DirectNavigation
    ));
    app.finish_direct_navigation_scan_for_test();
    assert!(app.folder_nav_back_stack.is_empty());
}

#[test]
fn direct_navigation_worker_admission_refusal_preserves_owner_before_frame_arbitration() {
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
fn direct_navigation_worker_history_wait_keeps_released_escape_stack_toggle() {
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
    assert!(!app.stack_mode_requested);
}

#[test]
fn direct_navigation_worker_block_reason_keeps_non_item_shortcuts_available() {
    use crate::filename_stack_ui::GridItemInputBlock;
    for block in [
        GridItemInputBlock::DirectNavigationScan,
        GridItemInputBlock::ViewOrderRefreshScan,
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
                app.handle_stack_refresh_keyboard(
                    &ctx,
                    true,
                    Default::default(),
                    Default::default(),
                    block,
                )
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

#[test]
fn direct_navigation_worker_rejected_address_input_cancels_prior_scan_in_actual_frame() {
    let mut app = setup_app();
    let origin = folder(&app, "origin");
    let target = folder(&app, "target");
    app.load_folder(origin.clone());
    app.finish_page_identity_preparation_for_test();
    app.post_render_main_navigation(
        &egui::Context::default(),
        update_phases::MainFrameNavigation {
            toolbar_fav_nav: Some(target),
            ..Default::default()
        },
    );
    let cancel = make_scan_ready(&mut app);
    app.settings
        .file_type_visibility
        .excluded_extensions
        .insert("zip".into());
    let rejected = app.tmp.path().join("hidden.zip");
    std::fs::write(&rejected, []).unwrap();
    app.post_render_main_navigation(
        &egui::Context::default(),
        update_phases::MainFrameNavigation {
            address_nav: Some(crate::ui_main::AddressBarNav::AddressEnterPhysical(
                rejected,
            )),
            ..Default::default()
        },
    );
    assert!(cancel.load(Ordering::Relaxed));
    assert!(!app.direct_navigation_scan_pending());
    app.post_render_main_navigation(&egui::Context::default(), Default::default());
    assert_eq!(app.current_folder.as_ref(), Some(&origin));
    assert!(app.folder_nav_back_stack.is_empty());
}

#[test]
fn direct_navigation_worker_rejected_filtered_backspace_cancels_prior_scan() {
    let mut app = setup_app();
    let origin = folder(&app, "origin");
    let target = folder(&app, "target");
    app.load_folder(origin.clone());
    app.finish_page_identity_preparation_for_test();
    open(&mut app, target);
    let cancel = make_scan_ready(&mut app);
    app.show_search_bar = true;
    app.search_filter = Some(HashSet::new().into());
    app.search_filter_origin_folder = Some(origin.clone());
    assert!(app.local_search_blocks_parent_nav());
    let ctx = egui::Context::default();
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
    let nav = app.handle_keyboard(&ctx);
    assert!(nav.is_none());
    app.post_render_main_navigation(
        &ctx,
        update_phases::MainFrameNavigation {
            keyboard_nav: nav,
            ..Default::default()
        },
    );
    ctx.end_pass().drop_without_applying_deltas();
    assert!(cancel.load(Ordering::Relaxed));
    assert!(!app.direct_navigation_scan_pending());
    assert_eq!(app.current_folder.as_ref(), Some(&origin));
}

#[test]
fn direct_navigation_worker_uncached_pdf_acceptance_retires_scan_before_pdf_completion() {
    let mut app = setup_app();
    app.active_quick_folder_slot = None;
    let origin = folder(&app, "origin");
    let target = folder(&app, "target");
    let pdf = app.tmp.path().join("uncached.pdf");
    std::fs::write(&pdf, b"%PDF-1.4\n").unwrap();
    app.load_folder(origin.clone());
    app.finish_page_identity_preparation_for_test();
    app.post_render_main_navigation(
        &egui::Context::default(),
        update_phases::MainFrameNavigation {
            toolbar_fav_nav: Some(target),
            ..Default::default()
        },
    );
    let pending = app.folder_pane_open_pending.as_mut().unwrap();
    let result = pending
        .rx
        .recv_timeout(std::time::Duration::from_secs(30))
        .unwrap();
    let cancel = pending.cancel.clone();
    let (late_tx, late_rx) = mpsc::channel();
    pending.rx = late_rx;
    let ctx = egui::Context::default();
    app.post_render_main_navigation(
        &ctx,
        update_phases::MainFrameNavigation {
            address_nav: Some(crate::ui_main::AddressBarNav::AddressEnterPhysical(
                pdf.clone(),
            )),
            ..Default::default()
        },
    );
    assert!(cancel.load(Ordering::Relaxed));
    assert!(!app.direct_navigation_scan_pending());
    assert!(app.pdf_enumerate_pending.is_some());
    // Keep PDF enumeration pending without requiring native PDFium. The actual
    // address-input admission above is production code, only its transport is held.
    let (handle, complete) = crate::pdf_loader::pending_enumerate_handle_for_test(&pdf);
    app.pdf_enumerate_pending.as_mut().unwrap().2 = handle;
    assert!(
        late_tx.send(result).is_err(),
        "the retired scan has no consumer"
    );
    app.post_render_main_navigation(&ctx, Default::default());
    assert_eq!(app.current_folder.as_ref(), Some(&origin));
    complete(Ok(vec![crate::pdf_loader::PdfPageEntry {
        page_num: 0,
        mtime: 0,
        file_size: 0,
    }]));
    app.poll_pdf_enumerate();
    app.finish_page_identity_preparation_for_test();
    assert_eq!(app.current_folder.as_ref(), Some(&pdf));
    assert_eq!(app.folder_nav_back_stack.len(), 1);
    assert!(!app.direct_navigation_scan_pending());
}

#[test]
fn direct_navigation_worker_successful_pdf_claim_outside_frame_retires_scan() {
    let mut app = setup_app();
    let origin = folder(&app, "origin");
    let target = folder(&app, "target");
    let pdf = app.tmp.path().join("claim.pdf");
    std::fs::write(&pdf, b"%PDF-1.4\n").unwrap();
    app.load_folder(origin.clone());
    app.finish_page_identity_preparation_for_test();
    open(&mut app, target);
    let cancel = make_scan_ready(&mut app);
    assert_eq!(
        // The container dispatcher owns admission; the lower-level PDF loader
        // assumes its caller has already claimed the request.
        app.load_folder_or_convert_archive_with_auto_fullscreen_owned(
            pdf,
            false,
            OpenRequestOwner::Navigation,
            StartupListIntent::ExplicitList,
        ),
        FolderOpenOutcome::Loaded
    );
    assert!(cancel.load(Ordering::Relaxed));
    assert!(!app.direct_navigation_scan_pending());
    assert!(app.pdf_enumerate_pending.is_some());
    app.post_render_main_navigation(&egui::Context::default(), Default::default());
    assert_eq!(app.current_folder.as_ref(), Some(&origin));
}

#[test]
fn direct_navigation_worker_pane_retirement_does_not_retain_older_direct_scan() {
    for direct in [false, true] {
        let mut app = setup_app();
        let target = folder(&app, "target");
        app.start_folder_open_scan(
            target,
            if direct {
                FolderOpenScanPurpose::DirectNavigation
            } else {
                FolderOpenScanPurpose::PaneNavigation
            },
        );
        let cancel = app
            .folder_pane_open_pending
            .as_ref()
            .unwrap()
            .cancel
            .clone();
        app.retire_replaced_main_list_requests(Some(MainListRequestOwner::Pane));
        assert_eq!(cancel.load(Ordering::Relaxed), direct);
        assert_eq!(app.folder_pane_open_pending.is_none(), direct);
    }
}

#[cfg(windows)]
#[test]
fn direct_navigation_worker_detached_archive_claim_preserves_main_pending_scan() {
    let mut app = setup_app();
    let target = folder(&app, "target");
    open(&mut app, target);
    let cancel = make_scan_ready(&mut app);
    app.detached_grid_archive_open_request_seq = 1805;
    let path = app.tmp.path().join("sibling.zip");
    let owner = OpenRequestOwner::DetachedGridArchive(DetachedGridArchiveOpenRequestOwner {
        request_id: 1805,
        lease: DetachedSessionLease { window_id: 80002 },
        source_path: path.clone(),
        target: DetachedZipTarget::RootBook,
        collection_restore: None,
    });
    assert!(app.claim_open_request_owner(&path, &owner));
    assert!(!cancel.load(Ordering::Relaxed));
    assert!(app.direct_navigation_scan_pending());
}

#[test]
fn direct_navigation_worker_preserves_unmarked_presentation_intents() {
    for intent in [
        StartupListIntent::PreservePresentation,
        StartupListIntent::ClassifyFolder,
    ] {
        let mut app = setup_app();
        let target = folder(&app, "target");
        let before = folder_scan::caller_scan_count_for_test();
        app.open_direct_navigation_target_at_stage(
            target.clone(),
            None,
            OpenRequestOwner::Navigation,
            None,
            None,
            intent,
            MainListRequestStage::Admission,
            DirectoryScanRoute::UiThread,
        );
        assert!(!app.direct_navigation_scan_pending());
        assert_eq!(folder_scan::caller_scan_count_for_test(), before + 1);
        assert_eq!(app.current_folder.as_ref(), Some(&target));
    }
}
