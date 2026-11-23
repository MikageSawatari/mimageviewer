//! Drive inputs retain one main-list owner through resolver and open handoff.
//! Resolver gates make replacement races deterministic without desktop input.

use super::drive_jump::{
    DriveJumpRequest, DriveJumpResolution, DriveJumpRootOrigin, finish_drive_jump_for_test,
};
use super::*;
use crate::folder_tree::{OpenablePathKind, OpenablePathResolution};
use crate::keymap::KeyAction;
use crate::ring_shortcut::{RingActionId, RingDirection, RingShortcutContext};
use std::sync::{Condvar, Mutex};
use std::time::{Duration, Instant};

fn resolution(path: &Path, kind: OpenablePathKind) -> DriveJumpResolution {
    DriveJumpResolution {
        target: Some(OpenablePathResolution {
            path: path.to_path_buf(),
            kind,
            requested_is_file: false,
        }),
        warnings: Vec::new(),
        outcome: "target",
    }
}

fn missing() -> DriveJumpResolution {
    DriveJumpResolution {
        target: None,
        warnings: vec!["ドライブが見つかりません: Z:\\".into()],
        outcome: "missing",
    }
}

struct ResolverGate {
    release: Arc<(Mutex<bool>, Condvar)>,
    started: mpsc::Receiver<(DriveJumpRequest, std::thread::ThreadId)>,
    completed: mpsc::Receiver<()>,
}

impl ResolverGate {
    fn install(app: &mut App, result: DriveJumpResolution) -> Self {
        let release = Arc::new((Mutex::new(false), Condvar::new()));
        let worker_release = release.clone();
        let (started_tx, started) = mpsc::channel();
        let (completed_tx, completed) = mpsc::channel();
        app.drive_jump_resolver = Some(Arc::new(move |request| {
            let _ = started_tx.send((request.clone(), std::thread::current().id()));
            let (lock, wake) = &*worker_release;
            let (released, timeout) = wake
                .wait_timeout_while(lock.lock().unwrap(), Duration::from_secs(30), |value| {
                    !*value
                })
                .unwrap();
            assert!(
                *released && !timeout.timed_out(),
                "resolver gate was not released"
            );
            let _ = completed_tx.send(());
            result.clone()
        }));
        Self {
            release,
            started,
            completed,
        }
    }

    fn request(&self) -> DriveJumpRequest {
        let (request, worker) = self.started.recv_timeout(Duration::from_secs(10)).unwrap();
        assert_ne!(
            worker,
            std::thread::current().id(),
            "resolution ran on the caller"
        );
        request
    }

    fn release(&self) {
        let (lock, wake) = &*self.release;
        *lock.lock().unwrap() = true;
        wake.notify_all();
    }

    fn finish_worker(&self) {
        self.release();
        self.completed
            .recv_timeout(Duration::from_secs(10))
            .unwrap();
    }
}

impl Drop for ResolverGate {
    fn drop(&mut self) {
        self.release();
    }
}

fn install_resolution(app: &mut App, result: DriveJumpResolution) {
    app.drive_jump_resolver = Some(Arc::new(move |_| result.clone()));
}

fn pending_cancel(app: &App) -> Arc<AtomicBool> {
    app.drive_jump_pending
        .as_ref()
        .expect("accepted drive request")
        .cancel
        .clone()
}

// Tests that need an accepted request still pass through the production arbiter.
fn dispatch_candidate(
    app: &mut App,
    ctx: &egui::Context,
    candidate: Option<crate::ui_main::AddressBarNav>,
) {
    if let Some(candidate) = candidate {
        let mut frame = update_phases::MainFrameNavigation::default();
        frame.keyboard_nav = Some(candidate);
        app.post_render_main_navigation(ctx, frame);
    }
}

fn switch_drive(app: &mut App, ctx: &egui::Context, letter: char, source: &'static str) {
    let candidate = app.apply_switch_drive_letter(ctx, letter, source);
    dispatch_candidate(app, ctx, candidate);
}

fn caller_io_counts() -> (usize, usize) {
    (
        crate::folder_tree::navigation_path_probe_count_for_test(),
        folder_scan::caller_scan_count_for_test(),
    )
}

fn settle(app: &mut App, ctx: &egui::Context) {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        app.poll_drive_jump(ctx);
        app.post_render_main_navigation(ctx, update_phases::MainFrameNavigation::default());
        app.poll_zip_enumerate();
        app.poll_pdf_enumerate();
        app.poll_sidecar_restore(ctx);
        app.poll_history_navigation_transition_for_test(ctx);
        app.finish_page_identity_preparation_for_test();
        if !app.drive_jump_pending()
            && app.folder_pane_open_pending.is_none()
            && app.zip_enumerate_pending.is_none()
            && app.pdf_enumerate_pending.is_none()
            && app
                .top_level_grid_view
                .history_navigation_transition()
                .is_none()
            && !app.sidecar_restore_active()
        {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "drive destination did not settle"
        );
        std::thread::yield_now();
    }
}

fn image_viewer(app: &mut App) {
    super::tests::seed_snapshot_context(app, "C:/drive-jump", &["C:/drive-jump/page.jpg"]);
    app.fullscreen_idx = Some(0);
    app.viewer_presentation = ViewerPresentation::Fullscreen;
}

#[test]
fn drive_jump_key_and_toolbar_capture_the_same_last_location_request() {
    for keyboard in [false, true] {
        let mut app = setup_app_for_test();
        let ctx = egui::Context::default();
        let remembered = PathBuf::from("D:/photos/remembered");
        app.active_quick_folder_slot = Some(QuickFolderSlotId::A);
        app.quick_folder_workspaces[0]
            .drive_current_dirs
            .insert("D:".into(), remembered.clone());
        let gate = ResolverGate::install(&mut app, missing());
        let before = app.current_folder.clone();
        let caller_io = caller_io_counts();
        let result = if keyboard {
            app.apply_location_navigation_key_action(
                &ctx,
                KeyAction::GridSwitchDriveD,
                "drive-test",
            )
        } else {
            app.apply_switch_drive_letter(&ctx, 'D', "drive-test-toolbar")
        };
        assert!(matches!(
            result,
            Some(crate::ui_main::AddressBarNav::DriveJump(_))
        ));
        assert!(
            app.drive_jump_pending.is_none(),
            "input is still an unadmitted candidate"
        );
        assert_eq!(
            caller_io_counts(),
            caller_io,
            "input performed caller-thread filesystem work"
        );
        let expected = DriveJumpRequest::LastLocation {
            drive_key: "D:".into(),
            remembered: Some(remembered),
        };
        dispatch_candidate(&mut app, &ctx, result);
        assert_eq!(app.drive_jump_pending.as_ref().unwrap().request, expected);
        assert_eq!(gate.request(), expected);
        assert_eq!(app.current_folder, before);
        assert!(app.folder_pane_open_pending.is_none());
        gate.finish_worker();
        finish_drive_jump_for_test(&mut app, &ctx);
    }
}

#[test]
fn drive_jump_key_ring_and_gamepad_share_the_drive_root_request() {
    for input in 0..3 {
        let mut app = setup_app_for_test();
        let ctx = egui::Context::default();
        let gate = ResolverGate::install(&mut app, missing());
        let nav = match input {
            0 => app.apply_location_navigation_key_action(
                &ctx,
                KeyAction::GridOpenDriveD,
                "drive-test",
            ),
            1 => app.apply_ring_action(
                &ctx,
                RingShortcutContext::Grid,
                RingActionId::drive_action('D').unwrap(),
                "drive-test",
            ),
            _ => {
                app.settings
                    .ring_shortcuts
                    .profile_mut(RingShortcutContext::Grid)
                    .slots[RingDirection::Up.slot_index()] =
                    RingActionId::drive_action('D').unwrap();
                app.trigger_gamepad_ring_action_for_test(&ctx, RingDirection::Up)
            }
        };
        assert!(matches!(
            nav,
            Some(crate::ui_main::AddressBarNav::DriveJump(_))
        ));
        assert!(app.drive_jump_pending.is_none());
        dispatch_candidate(&mut app, &ctx, nav);
        assert_eq!(
            gate.request(),
            DriveJumpRequest::Root {
                path: PathBuf::from(r"D:\"),
                origin: DriveJumpRootOrigin::DriveLetter("D:".into()),
            }
        );
        gate.finish_worker();
        finish_drive_jump_for_test(&mut app, &ctx);
    }
}

#[test]
fn drive_jump_current_root_preserves_unc_verbatim_unc_and_original_archive_drive() {
    for (displayed, source, expected) in [
        (r"\\server\share\books", None, r"\\server\share\"),
        (
            r"\\?\UNC\server\share\books",
            None,
            r"\\?\UNC\server\share\",
        ),
        (
            r"C:\cache\converted.zip",
            Some(r"E:\books\original.rar"),
            r"E:\",
        ),
    ] {
        let mut app = setup_app_for_test();
        let ctx = egui::Context::default();
        app.current_folder = Some(PathBuf::from(displayed));
        app.archive_source_override = source.map(PathBuf::from);
        let gate = ResolverGate::install(&mut app, missing());
        let candidate = app.apply_location_navigation_key_action(
            &ctx,
            KeyAction::GridOpenCurrentDriveRoot,
            "drive-test",
        );
        assert!(matches!(
            candidate,
            Some(crate::ui_main::AddressBarNav::DriveJump(_))
        ));
        assert!(app.drive_jump_pending.is_none());
        dispatch_candidate(&mut app, &ctx, candidate);
        assert_eq!(
            gate.request(),
            DriveJumpRequest::Root {
                path: PathBuf::from(expected),
                origin: DriveJumpRootOrigin::CurrentLocation,
            }
        );
        gate.finish_worker();
        finish_drive_jump_for_test(&mut app, &ctx);
    }
}

#[test]
fn drive_jump_latest_request_retires_the_previous_worker_owner() {
    let mut app = setup_app_for_test();
    let ctx = egui::Context::default();
    let gate = ResolverGate::install(&mut app, missing());
    switch_drive(&mut app, &ctx, 'D', "drive-test-first");
    gate.request();
    let old_cancel = pending_cancel(&app);
    switch_drive(&mut app, &ctx, 'E', "drive-test-last");
    let request = gate.request();
    assert!(old_cancel.load(Ordering::Relaxed));
    assert_eq!(
        request,
        DriveJumpRequest::LastLocation {
            drive_key: "E:".into(),
            remembered: None
        }
    );
    assert!(!pending_cancel(&app).load(Ordering::Relaxed));
    gate.release();
    finish_drive_jump_for_test(&mut app, &ctx);
    assert!(app.fs_feedback_toast.is_some());
}

#[test]
fn drive_jump_later_favorite_admission_rejects_old_resolver_completion() {
    let mut app = setup_app_for_test();
    let ctx = egui::Context::default();
    let old = app.tmp.path().join("old-drive");
    let favorite = app.tmp.path().join("new-favorite");
    std::fs::create_dir(&old).unwrap();
    std::fs::create_dir(&favorite).unwrap();
    let gate = ResolverGate::install(&mut app, resolution(&old, OpenablePathKind::Directory));
    switch_drive(&mut app, &ctx, 'D', "drive-test");
    gate.request();
    let cancel = pending_cancel(&app);
    let mut frame = update_phases::MainFrameNavigation::default();
    frame.toolbar_fav_nav = Some(favorite.clone());
    app.post_render_main_navigation(&ctx, frame);
    assert!(app.drive_jump_pending.is_none());
    assert!(cancel.load(Ordering::Relaxed));
    gate.finish_worker();
    settle(&mut app, &ctx);
    assert_eq!(app.current_folder.as_ref(), Some(&favorite));
}

#[test]
fn drive_jump_admission_retires_an_earlier_favorite_scan() {
    let mut app = setup_app_for_test();
    let ctx = egui::Context::default();
    let favorite = app.tmp.path().join("old-favorite");
    std::fs::create_dir(&favorite).unwrap();
    let mut frame = update_phases::MainFrameNavigation::default();
    frame.toolbar_fav_nav = Some(favorite);
    app.post_render_main_navigation(&ctx, frame);
    let old = app
        .folder_pane_open_pending
        .as_ref()
        .expect("favorite uses worker scan")
        .cancel
        .clone();
    let before = app.current_folder.clone();
    let gate = ResolverGate::install(&mut app, missing());
    switch_drive(&mut app, &ctx, 'D', "drive-test");
    assert!(old.load(Ordering::Relaxed));
    assert!(app.folder_pane_open_pending.is_none());
    gate.request();
    gate.finish_worker();
    settle(&mut app, &ctx);
    assert_eq!(app.current_folder, before);
}

#[test]
fn drive_jump_successful_later_direct_open_and_sibling_navigation_retire_resolution() {
    for sibling in [false, true] {
        let mut app = setup_app_for_test();
        let ctx = egui::Context::default();
        let origin = app.tmp.path().join("origin");
        let target = app.tmp.path().join("later");
        std::fs::create_dir(&origin).unwrap();
        std::fs::create_dir(&target).unwrap();
        app.current_folder = Some(origin.clone());
        let gate = ResolverGate::install(&mut app, missing());
        switch_drive(&mut app, &ctx, 'D', "drive-test");
        gate.request();
        let cancel = pending_cancel(&app);
        if sibling {
            app.start_folder_nav(origin, true, FolderNavMode::SiblingGrid);
            assert!(app.folder_nav_pending.is_some());
        } else {
            assert!(app.claim_open_request_owner_with_startup_intent(
                &target,
                &OpenRequestOwner::Navigation,
                true,
                MainListRequestStage::Admission
            ));
        }
        assert!(app.drive_jump_pending.is_none());
        assert!(cancel.load(Ordering::Relaxed));
        gate.finish_worker();
        app.poll_drive_jump(&ctx);
        assert!(
            app.fs_feedback_toast.is_none(),
            "stale missing result cannot notify"
        );
        app.cancel_pending_folder_nav();
    }
}

#[test]
fn drive_jump_surface_switches_retire_pending_results() {
    for operation in 0..6 {
        let mut app = setup_app_for_test();
        let ctx = egui::Context::default();
        match operation {
            3 => app.open_global_search(),
            4 => app.open_favsearch(),
            5 => app.open_tag_view(),
            _ => {}
        }
        if operation >= 3 {
            assert!(app.current_folder.is_none(), "empty-origin search fixture");
            assert!(
                app.top_level_grid_view
                    .return_to()
                    .is_none_or(|restore| matches!(
                        restore,
                        top_level_grid_view::TopLevelGridRestore::Unavailable
                    ))
            );
        }
        let gate = ResolverGate::install(&mut app, missing());
        switch_drive(&mut app, &ctx, 'D', "drive-test");
        gate.request();
        let cancel = pending_cancel(&app);
        match operation {
            0 => {
                app.activate_quick_folder_slot(QuickFolderSlotId::B);
            }
            1 => app.clear_quick_folder_slots(),
            2 => app.open_global_search(),
            3 => app.close_global_search(),
            4 => app.close_favsearch(),
            _ => app.close_tag_view(),
        }
        if operation >= 3 {
            assert!(!app.global_search.active && !app.favsearch.active && !app.tag_view.active);
        }
        assert!(app.drive_jump_pending.is_none(), "operation {operation}");
        assert!(cancel.load(Ordering::Relaxed));
        let toast = app.fs_feedback_toast.clone();
        gate.finish_worker();
        app.poll_drive_jump(&ctx);
        assert_eq!(app.fs_feedback_toast, toast);
    }
}

#[test]
fn drive_jump_remote_acquire_then_release_cannot_revive_old_result() {
    let mut app = setup_app_for_test();
    let ctx = egui::Context::default();
    let gate = ResolverGate::install(&mut app, missing());
    switch_drive(&mut app, &ctx, 'D', "drive-test");
    gate.request();
    let cancel = pending_cancel(&app);
    let handle = crate::remote_ipc::session::SessionHandle::new();
    app.set_remote_session_handle(handle.clone());
    handle.acquire(mimageviewer_ipc::SessionAcquireRequest {
        client_id: "drive-test".into(),
        peer: mimageviewer_ipc::SessionPeerInfo {
            connection_kind: mimageviewer_ipc::SessionConnectionKind::Direct,
            device_name: None,
        },
    });
    app.poll_mounted_document_open_owners(&ctx);
    app.poll_remote_session(&ctx);
    assert_eq!(
        handle.snapshot().phase,
        crate::remote_ipc::session::RemoteControlPhase::RemoteActive
    );
    assert!(app.drive_jump_pending.is_none());
    assert!(cancel.load(Ordering::Relaxed));
    switch_drive(&mut app, &ctx, 'E', "drive-test-remote-refusal");
    assert!(app.drive_jump_pending.is_none());
    handle.local_disconnect();
    app.poll_remote_session(&ctx);
    gate.finish_worker();
    app.poll_drive_jump(&ctx);
    assert!(app.drive_jump_pending.is_none());
}

#[test]
fn drive_jump_fullscreen_ring_and_linked_gamepad_close_before_admission() {
    for linked in [false, true] {
        for success in [false, true] {
            let _foreground = crate::test_native_focus::scoped(179301, 179301, true);
            let mut app = setup_app_for_test();
            let ctx = egui::Context::default();
            app.main_hwnd = Some(179300);
            app.settings.auto_fullscreen_image_folders = false;
            app.settings.auto_fullscreen_zip_pdf = false;
            let target = app.tmp.path().join("after-fullscreen-drive");
            std::fs::create_dir(&target).unwrap();
            image_viewer(&mut app);
            if linked {
                app.viewer_presentation = ViewerPresentation::DetachedWindow;
                app.fs_viewport_shown = true;
                app.fs_viewport_presentation = Some(ViewerPresentation::DetachedWindow);
                app.begin_mounted_detached_session_for_test(179301, DetachedSource::Image);
                assert_eq!(app.projected_viewer_context_id(), app.viewer_context_main());
            }
            let result = if success {
                resolution(&target, OpenablePathKind::Directory)
            } else {
                missing()
            };
            let gate = ResolverGate::install(&mut app, result);
            let candidate = if linked {
                assert_eq!(app.current_input_surface_for_test(), ActionSurface::Viewer);
                assert_eq!(
                    app.current_ring_shortcut_context(),
                    RingShortcutContext::ImageFullscreen
                );
                app.settings
                    .ring_shortcuts
                    .profile_mut(RingShortcutContext::ImageFullscreen)
                    .slots[RingDirection::Up.slot_index()] =
                    RingActionId::drive_action('Z').unwrap();
                app.trigger_gamepad_ring_action_for_test(&ctx, RingDirection::Up)
            } else {
                app.apply_ring_action(
                    &ctx,
                    RingShortcutContext::ImageFullscreen,
                    RingActionId::drive_action('Z').unwrap(),
                    "drive-test-ring",
                )
            };
            assert!(!app.projected_viewer_session_open());
            assert!(matches!(
                candidate,
                Some(crate::ui_main::AddressBarNav::DriveJump(_))
            ));
            assert!(
                app.drive_jump_pending.is_none(),
                "close completes before main admission"
            );
            dispatch_candidate(&mut app, &ctx, candidate);
            assert!(app.drive_jump_pending.is_some());
            assert!(app.fs_feedback_toast.is_none());
            gate.request();
            gate.finish_worker();
            finish_drive_jump_for_test(&mut app, &ctx);
            assert!(!app.projected_viewer_session_open());
            if success {
                settle(&mut app, &ctx);
                assert_eq!(app.current_folder.as_ref(), Some(&target));
                assert!(!app.projected_viewer_session_open());
                assert!(app.fs_feedback_toast.is_none());
            } else {
                assert!(
                    app.fs_feedback_toast
                        .as_ref()
                        .unwrap()
                        .0
                        .contains("ドライブが見つかりません")
                );
            }
        }
    }
}

#[test]
fn drive_jump_new_fullscreen_discards_old_resolution() {
    let mut app = setup_app_for_test();
    let ctx = egui::Context::default();
    let gate = ResolverGate::install(&mut app, missing());
    switch_drive(&mut app, &ctx, 'D', "drive-test");
    gate.request();
    let cancel = pending_cancel(&app);
    image_viewer(&mut app);
    gate.finish_worker();
    app.poll_drive_jump(&ctx);
    assert!(app.drive_jump_pending.is_none());
    assert!(cancel.load(Ordering::Relaxed));
    assert!(app.projected_viewer_session_open());
    assert!(app.fs_feedback_toast.is_none());
}

#[test]
fn drive_jump_failed_fullscreen_close_keeps_the_existing_request() {
    let mut app = setup_app_for_test();
    let ctx = egui::Context::default();
    let gate = ResolverGate::install(&mut app, missing());
    switch_drive(&mut app, &ctx, 'D', "drive-test-existing");
    let original = gate.request();
    let cancel = pending_cancel(&app);
    image_viewer(&mut app);
    app.drive_jump_close_backend = Some(|_, _| true);
    switch_drive(&mut app, &ctx, 'E', "drive-test-refused");
    assert_eq!(app.drive_jump_pending.as_ref().unwrap().request, original);
    assert!(!cancel.load(Ordering::Relaxed));
    assert!(app.projected_viewer_session_open());
    assert!(
        app.fs_feedback_toast
            .as_ref()
            .unwrap()
            .0
            .contains("全画面を閉じられなかった")
    );
    gate.finish_worker();
}

#[test]
fn drive_jump_snapshot_refusal_keeps_the_viewer_open_and_never_starts_resolution() {
    let mut app = setup_app_for_test();
    let ctx = egui::Context::default();
    image_viewer(&mut app);
    app.activate_snapshot(crate::snapshot::SnapshotSourceLabel::Mixed);
    assert!(app.is_snapshot_active());
    app.drive_jump_resolver = Some(Arc::new(|_| {
        panic!("snapshot refusal must precede resolver")
    }));
    app.drive_jump_close_backend = Some(|_, _| panic!("snapshot refusal must precede close"));
    switch_drive(&mut app, &ctx, 'D', "drive-test-refused");
    assert!(app.projected_viewer_session_open());
    assert!(app.drive_jump_pending.is_none());
    assert!(app.fs_feedback_toast.is_some());
}

#[test]
fn drive_jump_classified_directories_keep_worker_scan_and_original_navigation() {
    for search in [None, Some(0), Some(1), Some(2), Some(3)] {
        for name in ["ordinary", "looks-like-a-container.pdf"] {
            let mut app = match search {
                Some(kind) => App::navigation_search_fixture_for_test(kind),
                None => setup_app_for_test(),
            };
            let ctx = egui::Context::default();
            app.settings.auto_fullscreen_image_folders = false;
            app.settings.auto_fullscreen_zip_pdf = false;
            let target = app.tmp.path().join(name);
            std::fs::create_dir(&target).unwrap();
            install_resolution(&mut app, resolution(&target, OpenablePathKind::Directory));
            let caller_io = caller_io_counts();
            switch_drive(&mut app, &ctx, 'D', "drive-test");
            assert_eq!(
                caller_io_counts(),
                caller_io,
                "admission performed UI filesystem work"
            );
            let navigation = app.drive_jump_pending.as_ref().unwrap().navigation.clone();
            finish_drive_jump_for_test(&mut app, &ctx);
            assert_eq!(
                caller_io_counts(),
                caller_io,
                "classified handoff performed UI filesystem work"
            );
            let scan = app
                .folder_pane_open_pending
                .as_ref()
                .expect("classified directory must start worker scan");
            assert_eq!(scan.path, target);
            assert_eq!(
                scan.navigation.as_ref().unwrap().source_context,
                navigation.source_context
            );
            assert_eq!(
                scan.navigation.as_ref().unwrap().source_proof,
                navigation.source_proof
            );
            assert_eq!(
                scan.navigation.as_ref().unwrap().history,
                navigation.history
            );
            if search.is_some() {
                assert!(matches!(
                    scan.purpose,
                    FolderOpenScanPurpose::JumpToPhysicalFolder { .. }
                ));
                assert!(app.global_search.active || app.favsearch.active || app.tag_view.active);
            } else {
                assert!(matches!(
                    scan.purpose,
                    FolderOpenScanPurpose::DirectNavigation
                ));
            }
            settle(&mut app, &ctx);
            assert_eq!(app.current_folder.as_ref(), Some(&target));
            assert!(!app.global_search.active && !app.favsearch.active && !app.tag_view.active);
            assert!(
                app.fs_feedback_toast.is_none(),
                "nearest-parent success has no fallback warning"
            );
        }
    }
}

#[test]
fn drive_jump_missing_destination_preserves_each_accepted_search_source() {
    for kind in 0..4 {
        let mut app = App::navigation_search_fixture_for_test(kind);
        let ctx = egui::Context::default();
        let before = (
            app.current_folder.clone(),
            app.items.iter().map(GridItem::perf_key).collect::<Vec<_>>(),
            app.selected,
            app.folder_nav_history_snapshot(),
        );
        install_resolution(&mut app, missing());
        switch_drive(&mut app, &ctx, 'Z', "drive-test");
        finish_drive_jump_for_test(&mut app, &ctx);
        assert!(app.global_search.active || app.favsearch.active || app.tag_view.active);
        assert_eq!(
            (
                app.current_folder.clone(),
                app.items.iter().map(GridItem::perf_key).collect::<Vec<_>>(),
                app.selected,
                app.folder_nav_history_snapshot()
            ),
            before
        );
        assert!(app.fs_feedback_toast.is_some());
    }
}

#[test]
fn drive_jump_toolbar_union_and_tooltips_are_slot_owned_without_path_probes() {
    let mut app = setup_app_for_test();
    assert!(app.toolbar_drive_keys().is_empty());
    app.quick_folder_workspaces[0].drive_current_dirs = [
        ("D:".into(), PathBuf::from("D:/absent-a")),
        ("Z:".into(), PathBuf::from("Z:/offline")),
        ("c:".into(), PathBuf::from("C:/bad-key")),
        ("invalid".into(), PathBuf::from("bad")),
    ]
    .into();
    app.quick_folder_workspaces[1].drive_current_dirs = [
        ("D:".into(), PathBuf::from("D:/absent-b")),
        ("E:".into(), PathBuf::from("E:/absent")),
    ]
    .into();
    app.active_quick_folder_slot = Some(QuickFolderSlotId::A);
    let keys = app.toolbar_drive_keys();
    assert_eq!(keys, vec!["D:", "E:", "Z:"]);
    assert_eq!(
        app.toolbar_drive_tooltip("D:"),
        PathBuf::from("D:/absent-a").display().to_string()
    );
    app.active_quick_folder_slot = Some(QuickFolderSlotId::B);
    assert_eq!(app.toolbar_drive_keys(), keys);
    assert_eq!(
        app.toolbar_drive_tooltip("D:"),
        PathBuf::from("D:/absent-b").display().to_string()
    );
    assert_eq!(
        app.toolbar_drive_tooltip("Z:"),
        "記録なし (ルートを開きます)"
    );
    app.active_quick_folder_slot = None;
    assert_eq!(app.toolbar_drive_keys(), keys);
    assert_eq!(
        app.toolbar_drive_tooltip("D:"),
        "記録なし (ルートを開きます)"
    );
}

#[test]
fn drive_jump_removal_clears_both_slots_persists_and_does_not_cancel_admitted_work() {
    let mut app = setup_app_for_test();
    let ctx = egui::Context::default();
    app.current_folder = Some(PathBuf::from("C:/current"));
    app.active_quick_folder_slot = Some(QuickFolderSlotId::A);
    for slot in &mut app.quick_folder_workspaces {
        slot.drive_current_dirs
            .insert("D:".into(), PathBuf::from("D:/remembered"));
    }
    let gate = ResolverGate::install(&mut app, missing());
    switch_drive(&mut app, &ctx, 'D', "drive-test");
    let accepted = gate.request();
    let cancel = pending_cancel(&app);
    app.remove_toolbar_drive("D:");
    assert_eq!(app.drive_jump_pending.as_ref().unwrap().request, accepted);
    assert!(!cancel.load(Ordering::Relaxed));
    assert!(
        app.quick_folder_workspaces
            .iter()
            .all(|slot| !slot.drive_current_dirs.contains_key("D:"))
    );
    let saved = crate::settings::Settings::load();
    assert!(
        saved
            .quick_folder_drive_current_dirs
            .iter()
            .all(|slot| !slot.contains_key("D:"))
    );
    gate.finish_worker();
    finish_drive_jump_for_test(&mut app, &ctx);
    let gate = ResolverGate::install(&mut app, missing());
    switch_drive(&mut app, &ctx, 'D', "drive-test-after-removal");
    assert_eq!(
        gate.request(),
        DriveJumpRequest::LastLocation {
            drive_key: "D:".into(),
            remembered: None
        }
    );
    gate.finish_worker();
    finish_drive_jump_for_test(&mut app, &ctx);
}

#[test]
fn drive_jump_current_drive_cannot_be_removed_and_synthetic_views_have_no_selection() {
    let mut app = setup_app_for_test();
    app.current_folder = Some(PathBuf::from("D:/current"));
    app.top_level_grid_view
        .replace_surface(top_level_grid_view::TopLevelGridSurface::Folder);
    for slot in &mut app.quick_folder_workspaces {
        slot.drive_current_dirs
            .insert("D:".into(), PathBuf::from("D:/remembered"));
    }
    assert_eq!(app.current_toolbar_drive_key().as_deref(), Some("D:"));
    app.remove_toolbar_drive("D:");
    assert!(
        app.quick_folder_workspaces
            .iter()
            .all(|slot| slot.drive_current_dirs.contains_key("D:"))
    );
    app.archive_source_override = Some(PathBuf::from("E:/original/book.rar"));
    assert_eq!(app.current_toolbar_drive_key().as_deref(), Some("E:"));
    for surface in [
        top_level_grid_view::TopLevelGridSurface::DriveList,
        top_level_grid_view::TopLevelGridSurface::Rating { stars: 3 },
    ] {
        app.top_level_grid_view.replace_surface(surface);
        assert_eq!(app.current_toolbar_drive_key(), None);
    }
}

#[test]
fn drive_jump_independent_viewer_returns_transport_then_root_owns_adoption() {
    let mut app = setup_app_for_test();
    let ctx = egui::Context::default();
    let origin = app.tmp.path().join("main-origin");
    let target = app.tmp.path().join("main-drive-destination");
    std::fs::create_dir(&origin).unwrap();
    std::fs::create_dir(&target).unwrap();
    app.current_folder = Some(origin.clone());
    app.build_active_context_for_test(Some(179302), DetachedSource::Image, |detached| {
        image_viewer(detached);
        detached.viewer_presentation = ViewerPresentation::DetachedWindow;
    });
    install_resolution(&mut app, resolution(&target, OpenablePathKind::Directory));
    let transport = app
        .with_active_viewer_context(|mounted| {
            assert!(!mounted.main_folder_history_available());
            let transport = mounted.apply_ring_action(
                &ctx,
                RingShortcutContext::ImageFullscreen,
                RingActionId::drive_action('D').unwrap(),
                "drive-independent-test",
            );
            assert!(matches!(
                transport,
                Some(crate::ui_main::AddressBarNav::DriveJump(_))
            ));
            assert!(
                mounted.drive_jump_pending.is_none(),
                "mount cannot capture a main ticket"
            );
            transport
        })
        .unwrap()
        .unwrap();
    assert_eq!(app.current_folder.as_ref(), Some(&origin));
    let mut frame = update_phases::MainFrameNavigation::default();
    frame.gamepad_nav = Some(transport);
    app.post_render_main_navigation(&ctx, frame);
    let pending = app
        .drive_jump_pending
        .as_ref()
        .expect("root admits transported drive input");
    assert_eq!(pending.navigation.source_context, app.viewer_context_main());
    finish_drive_jump_for_test(&mut app, &ctx);
    settle(&mut app, &ctx);
    assert_eq!(app.current_folder.as_ref(), Some(&target));
    assert_eq!(
        app.folder_history_back_entry().map(|entry| &entry.location),
        Some(&FolderNavHistoryTarget::Path(origin))
    );
}

#[test]
fn drive_jump_main_wait_does_not_block_an_independent_mounted_grid() {
    let mut app = setup_app_for_test();
    let ctx = egui::Context::default();
    app.build_active_context_for_test(Some(179303), DetachedSource::Image, |detached| {
        super::tests::seed_snapshot_context(
            detached,
            "C:/independent-grid",
            &["C:/independent-grid/page.jpg"],
        );
    });
    let gate = ResolverGate::install(&mut app, missing());
    switch_drive(&mut app, &ctx, 'D', "drive-main-wait-test");
    gate.request();
    let cancel = pending_cancel(&app);
    assert!(app.drive_jump_pending());
    assert!(app.drive_jump_elapsed().is_some());
    assert!(!app.grid_item_input_allowed());
    app.with_active_viewer_context(|mounted| {
        assert!(
            mounted.drive_jump_pending.is_some(),
            "the main request remains owned globally"
        );
        assert!(
            !mounted.drive_jump_pending(),
            "independent grid has no drive wait"
        );
        assert!(mounted.drive_jump_elapsed().is_none());
        assert!(mounted.grid_item_input_allowed());
    })
    .expect("independent grid remains mountable during main resolution");
    assert!(app.drive_jump_pending());
    assert!(app.drive_jump_elapsed().is_some());
    assert!(!cancel.load(Ordering::Relaxed));
    gate.finish_worker();
    app.retire_drive_jump();
    assert!(cancel.load(Ordering::Relaxed));
    app.poll_drive_jump(&ctx);
    assert!(!app.drive_jump_pending());
    assert!(
        app.fs_feedback_toast.is_none(),
        "cancelled main result cannot notify"
    );
}

#[test]
fn drive_jump_search_pdf_classification_keeps_search_until_successful_enumeration() {
    for kind in 0..4 {
        for success in [false, true] {
            let mut app = App::navigation_search_fixture_for_test(kind);
            let ctx = egui::Context::default();
            app.settings.auto_fullscreen_zip_pdf = false;
            app.settings.auto_fullscreen_image_folders = false;
            let target = app.tmp.path().join("drive-destination.pdf");
            std::fs::write(&target, b"%PDF-1.4\n").unwrap();
            let before = (
                app.current_folder.clone(),
                app.selected,
                app.items.iter().map(GridItem::perf_key).collect::<Vec<_>>(),
            );
            let address_before = app.address.clone();
            let history_before = app.folder_nav_history_snapshot();
            let toast_before = app.fs_feedback_toast.clone();
            install_resolution(&mut app, resolution(&target, OpenablePathKind::File));
            let caller_io = caller_io_counts();
            switch_drive(&mut app, &ctx, 'D', "drive-pdf-test");
            finish_drive_jump_for_test(&mut app, &ctx);
            assert_eq!(
                caller_io_counts(),
                caller_io,
                "file classification was lost before PDF open"
            );
            assert!(
                app.folder_pane_open_pending.is_none(),
                "PDF cannot become a directory scan"
            );
            assert!(app.global_search.active || app.favsearch.active || app.tag_view.active);
            let pending = app
                .pdf_enumerate_pending
                .as_mut()
                .expect("PDF reached its existing container open");
            assert_eq!(pending.0, target);
            let result = if success {
                Ok(vec![crate::pdf_loader::PdfPageEntry {
                    page_num: 0,
                    mtime: 0,
                    file_size: 9,
                }])
            } else {
                Err(std::io::Error::other("controlled drive PDF failure"))
            };
            pending.2 = crate::pdf_loader::completed_enumerate_handle_for_test(&target, result);
            settle(&mut app, &ctx);
            if success {
                assert!(!app.global_search.active && !app.favsearch.active && !app.tag_view.active);
                assert_eq!(app.effective_folder().as_ref(), Some(&target));
                assert!(
                    app.items
                        .iter()
                        .any(|item| matches!(item, GridItem::PdfPage { .. }))
                );
            } else {
                assert!(app.global_search.active || app.favsearch.active || app.tag_view.active);
                assert_eq!(
                    (
                        app.current_folder.clone(),
                        app.selected,
                        app.items.iter().map(GridItem::perf_key).collect::<Vec<_>>()
                    ),
                    before
                );
                assert_eq!(app.address, address_before);
                assert_eq!(app.folder_nav_history_snapshot(), history_before);
                assert!(app.pdf_enumerate_pending.is_none());
                assert!(app.pdf_placeholder_count.is_none());
                // The existing plain-PDF Other failure restores the source;
                // unlike drive resolution failure, it does not publish a toast.
                assert_eq!(app.fs_feedback_toast, toast_before);
            }
        }
    }
}

#[test]
fn drive_jump_search_zip_classification_uses_container_open_and_exits_on_success() {
    for kind in 0..4 {
        let mut app = App::navigation_search_fixture_for_test(kind);
        let ctx = egui::Context::default();
        app.settings.auto_fullscreen_zip_pdf = false;
        app.settings.auto_fullscreen_image_folders = false;
        let target = app.tmp.path().join("drive-destination.zip");
        let file = std::fs::File::create(&target).unwrap();
        let mut writer = zip::ZipWriter::new(file);
        writer
            .start_file("page.jpg", zip::write::SimpleFileOptions::default())
            .unwrap();
        std::io::Write::write_all(&mut writer, b"metadata-only page").unwrap();
        writer.finish().unwrap();
        install_resolution(&mut app, resolution(&target, OpenablePathKind::File));
        let caller_io = caller_io_counts();
        switch_drive(&mut app, &ctx, 'D', "drive-zip-test");
        finish_drive_jump_for_test(&mut app, &ctx);
        assert_eq!(
            caller_io_counts(),
            caller_io,
            "file classification was lost before ZIP open"
        );
        assert!(
            app.folder_pane_open_pending.is_none(),
            "ZIP cannot become a directory scan"
        );
        let Some(HistoryNavigationTransition::Physical(pending)) =
            app.top_level_grid_view.history_navigation_transition()
        else {
            panic!("ZIP must retain its existing physical preflight owner");
        };
        assert_eq!(pending.path, target);
        assert_eq!(pending.classified_kind, Some(OpenPathKind::File));
        assert!(matches!(
            pending.phase,
            PhysicalHistoryPhase::Preflighting { .. }
        ));
        assert!(app.global_search.active || app.favsearch.active || app.tag_view.active);
        settle(&mut app, &ctx);
        assert!(!app.global_search.active && !app.favsearch.active && !app.tag_view.active);
        assert_eq!(app.effective_folder().as_ref(), Some(&target));
        assert!(
            app.items
                .iter()
                .any(|item| matches!(item, GridItem::ZipImage { .. }))
        );
    }
}

#[test]
fn drive_jump_removal_during_scan_keeps_owner_and_success_records_the_drive_again() {
    let mut app = setup_app_for_test();
    let ctx = egui::Context::default();
    let target = app.tmp.path().join("removed-while-scanning");
    std::fs::create_dir(&target).unwrap();
    let drive = drive_current_key_for_path(&target).unwrap();
    let letter = drive.chars().next().unwrap();
    app.current_folder = Some(PathBuf::from(if letter == 'Z' {
        "Y:/origin"
    } else {
        "Z:/origin"
    }));
    app.active_quick_folder_slot = Some(QuickFolderSlotId::A);
    for slot in &mut app.quick_folder_workspaces {
        slot.drive_current_dirs
            .insert(drive.clone(), target.clone());
    }
    install_resolution(&mut app, resolution(&target, OpenablePathKind::Directory));
    switch_drive(&mut app, &ctx, letter, "drive-removal-test");
    finish_drive_jump_for_test(&mut app, &ctx);
    let cancel = app
        .folder_pane_open_pending
        .as_ref()
        .unwrap()
        .cancel
        .clone();
    app.remove_toolbar_drive(&drive);
    assert!(!cancel.load(Ordering::Relaxed));
    assert!(
        app.quick_folder_workspaces
            .iter()
            .all(|slot| !slot.drive_current_dirs.contains_key(&drive))
    );
    settle(&mut app, &ctx);
    assert_eq!(app.current_folder.as_ref(), Some(&target));
    assert_eq!(
        app.quick_folder_workspaces[0]
            .drive_current_dirs
            .get(&drive),
        Some(&target)
    );
    assert!(
        !app.quick_folder_workspaces[1]
            .drive_current_dirs
            .contains_key(&drive)
    );
}

#[test]
fn drive_jump_modal_refusal_does_not_close_viewer_or_replace_existing_request() {
    let mut app = setup_app_for_test();
    let ctx = egui::Context::default();
    let gate = ResolverGate::install(&mut app, missing());
    switch_drive(&mut app, &ctx, 'D', "drive-test-existing");
    let original = gate.request();
    let cancel = pending_cancel(&app);
    image_viewer(&mut app);
    app.show_preferences = true;
    app.drive_jump_close_backend = Some(|_, _| panic!("modal refusal must precede close"));
    switch_drive(&mut app, &ctx, 'E', "drive-test-modal-refusal");
    assert_eq!(app.drive_jump_pending.as_ref().unwrap().request, original);
    assert!(!cancel.load(Ordering::Relaxed));
    assert!(app.projected_viewer_session_open());
    gate.finish_worker();
}

#[test]
fn drive_jump_ready_result_loses_to_same_frame_keyboard_navigation() {
    let mut app = setup_app_for_test();
    let ctx = egui::Context::default();
    let gate = ResolverGate::install(&mut app, missing());
    switch_drive(&mut app, &ctx, 'D', "drive-test-existing");
    gate.request();
    let cancel = pending_cancel(&app);
    gate.finish_worker();
    let deadline = Instant::now() + Duration::from_secs(10);
    while !app.drive_jump_ready() {
        app.poll_drive_jump(&ctx);
        assert!(Instant::now() < deadline, "resolver did not become ready");
        std::thread::yield_now();
    }
    let mut frame = update_phases::MainFrameNavigation::default();
    frame.keyboard_nav = Some(crate::ui_main::AddressBarNav::LocationJump(
        crate::ui_main::LocationJumpTarget::DriveList,
    ));
    app.post_render_main_navigation(&ctx, frame);
    assert!(app.items_are_drive_list);
    assert!(app.drive_jump_pending.is_none());
    assert!(cancel.load(Ordering::Relaxed));
    assert!(
        app.fs_feedback_toast.is_none(),
        "losing ready result cannot publish its warning"
    );
}

#[test]
fn drive_jump_current_root_without_location_preserves_existing_warning() {
    let mut app = setup_app_for_test();
    let ctx = egui::Context::default();
    app.current_folder = None;
    app.drive_jump_resolver = Some(Arc::new(|_| {
        panic!("missing current location has no worker request")
    }));
    assert!(app.apply_current_drive_root(&ctx, "drive-test").is_none());
    assert!(app.drive_jump_pending.is_none());
    assert_eq!(
        app.fs_feedback_toast.as_ref().unwrap().0,
        "現在位置のルートディレクトリが見つかりません"
    );
}
