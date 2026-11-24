//! §1.853 default-target handler coverage. All enumerated folders belong to the
//! isolated AppTestEnv; Desktop tests only resolve its path, never enumerate it.

use super::*;
use crate::app::tests::phase_c_support::{AppTestEnv, setup_app};
use crate::settings::{ListCursorHint, StartupFolderMode, StartupListRestore, StartupListTarget};
use std::time::{Duration, Instant};

fn fixture() -> AppTestEnv {
    let mut app = setup_app();
    app.initialized = true;
    app.active_quick_folder_slot = None;
    app.settings.auto_fullscreen_image_folders = false;
    app.settings.auto_fullscreen_zip_pdf = true;
    app.settings.restore_last_cursor = true;
    app.settings.sort_order = crate::settings::SortOrder::FileName;
    app
}

fn folder(app: &AppTestEnv, name: &str) -> PathBuf {
    let path = app.tmp.path().join(name);
    std::fs::create_dir_all(&path).unwrap();
    path
}

fn specific(app: &mut App, path: &Path) {
    app.settings.startup_folder_mode = StartupFolderMode::Specific;
    app.settings.startup_folder_path = Some(path.to_path_buf());
}

fn physical(path: &Path) -> StartupListTarget {
    StartupListTarget::PhysicalList {
        logical_path: path.to_path_buf(),
        zip_prefix: None,
    }
}

fn settle(app: &mut App) {
    app.finish_startup_open_path_resolve_for_test();
    let ctx = egui::Context::default();
    let deadline = Instant::now() + Duration::from_secs(30);
    while app.sidecar_restore_active() {
        app.poll_sidecar_restore(&ctx);
        app.poll_fs_nav_lock(&ctx);
        assert!(
            Instant::now() < deadline,
            "default sidecar restore did not settle"
        );
        std::thread::yield_now();
    }
    app.finish_page_identity_preparation_for_test();
}

/// Keep the actual worker's complete payload, but let the test choose when the
/// production poll receives it. No synthetic pending owner or duplicate request.
fn hold_result(
    app: &mut App,
) -> (
    mpsc::Sender<StartupOpenPathResolveResult>,
    StartupOpenPathResolveResult,
) {
    let pending = app.startup_open_path_resolve_pending.as_mut().unwrap();
    let result = pending.rx.recv_timeout(Duration::from_secs(30)).unwrap();
    let (tx, rx) = mpsc::channel();
    pending.rx = rx;
    (tx, result)
}

fn assert_scanned(result: &StartupOpenPathResolveResult, path: &Path) {
    assert!(matches!(
        result.default_target,
        Some(DefaultStartupTarget::Folder { .. })
    ));
    assert!(matches!(result.resolved.as_ref(), Some(resolved)
        if resolved.path == path && matches!(resolved.kind, crate::folder_tree::OpenablePathKind::Directory)));
    assert!(matches!(result.folder_scan, Some(Ok(_))));
    assert!(result.scan_policy.is_some());
    assert!(!result.scan_policy_mixed);
}

#[test]
fn startup_default_previous_worker_restores_same_folder_cursor_and_rows() {
    let mut app = fixture();
    let target = folder(&app, "previous");
    for index in 0..18 {
        std::fs::create_dir(target.join(format!("child-{index:02}"))).unwrap();
    }
    let record = StartupListRestore::V1 {
        target: physical(&target),
        cursor: Some(ListCursorHint {
            name: "child-12".into(),
            rows_above: Some(2),
        }),
    };
    app.settings.startup_folder_mode = StartupFolderMode::Previous;
    app.settings.startup_list_restore = Some(record.clone());
    let before = folder_scan::caller_scan_count_for_test();
    app.open_default_startup_target();
    assert!(app.current_folder.is_none());
    assert_eq!(folder_scan::caller_scan_count_for_test(), before);
    let (tx, result) = hold_result(&mut app);
    assert_scanned(&result, &target);
    tx.send(result).unwrap();
    settle(&mut app);
    assert_eq!(folder_scan::caller_scan_count_for_test(), before);
    assert_eq!(app.current_folder.as_ref(), Some(&target));
    assert_eq!(app.items[app.selected.unwrap()].name(), "child-12");
    assert_eq!(app.grid_scroll.row_anchor(), Some(2));
    assert_eq!(app.settings.startup_list_restore, Some(record));
    assert!(app.fullscreen_idx.is_none());
}

#[test]
fn startup_default_previous_missing_folder_uses_nearest_ancestor_without_old_cursor() {
    let mut app = fixture();
    let ancestor = folder(&app, "ancestor");
    std::fs::create_dir(ancestor.join("visible-child")).unwrap();
    let missing = ancestor.join("gone").join("deeper");
    app.settings.startup_folder_mode = StartupFolderMode::Previous;
    app.settings.startup_list_restore = Some(StartupListRestore::V1 {
        target: physical(&missing),
        cursor: Some(ListCursorHint {
            name: "stale-child".into(),
            rows_above: Some(9),
        }),
    });
    let before = folder_scan::caller_scan_count_for_test();
    app.open_default_startup_target();
    let (tx, result) = hold_result(&mut app);
    assert_scanned(&result, &ancestor);
    assert!(matches!(&result.default_target,
        Some(DefaultStartupTarget::Folder { intent: StartupListIntent::RestoreList { target, cursor: None }, cursor: None })
        if *target == physical(&ancestor)));
    tx.send(result).unwrap();
    settle(&mut app);
    assert_eq!(folder_scan::caller_scan_count_for_test(), before);
    assert_eq!(app.current_folder.as_ref(), Some(&ancestor));
    assert_ne!(app.grid_scroll.row_anchor(), Some(9));
    let Some(StartupListRestore::V1 { target, cursor }) = &app.settings.startup_list_restore else {
        panic!("adopted ancestor must become the restorable list");
    };
    assert_eq!(*target, physical(&ancestor));
    assert!(
        cursor
            .as_ref()
            .is_none_or(|cursor| cursor.name != "stale-child" && cursor.rows_above != Some(9))
    );
}

#[test]
fn startup_default_same_folder_keeps_existing_history_without_an_extra_departure() {
    let mut app = fixture();
    let target = folder(&app, "same-default");
    std::fs::create_dir(target.join("child")).unwrap();
    app.load_folder(target.clone());
    settle(&mut app);
    let before = app.folder_nav_history_snapshot();
    specific(&mut app, &target);
    let scans = folder_scan::caller_scan_count_for_test();
    app.open_default_startup_target();
    let (tx, result) = hold_result(&mut app);
    assert_scanned(&result, &target);
    tx.send(result).unwrap();
    settle(&mut app);
    assert_eq!(folder_scan::caller_scan_count_for_test(), scans);
    assert_eq!(app.current_folder.as_ref(), Some(&target));
    let after = app.folder_nav_history_snapshot();
    assert_eq!(after.back_stack, before.back_stack);
    assert_eq!(after.forward_stack, before.forward_stack);
}

#[test]
fn startup_default_specific_worker_preserves_cursor_only_at_the_same_place() {
    for same_place in [false, true] {
        let mut app = fixture();
        let target = folder(&app, "specific");
        std::fs::create_dir(target.join("a")).unwrap();
        std::fs::create_dir(target.join("b")).unwrap();
        specific(&mut app, &target);
        app.settings.last_folder = Some(if same_place {
            target.clone()
        } else {
            folder(&app, "other")
        });
        app.settings.last_cursor_name = Some("b".into());
        app.settings.last_cursor_rows_above = Some(1);
        let before = folder_scan::caller_scan_count_for_test();
        app.open_default_startup_target();
        let (tx, result) = hold_result(&mut app);
        assert_scanned(&result, &target);
        assert!(
            matches!(&result.default_target, Some(DefaultStartupTarget::Folder {
            intent: StartupListIntent::ExplicitList, cursor,
        }) if cursor.is_some() == same_place)
        );
        tx.send(result).unwrap();
        settle(&mut app);
        assert_eq!(folder_scan::caller_scan_count_for_test(), before);
        assert_eq!(app.current_folder.as_ref(), Some(&target));
        assert!(app.fullscreen_idx.is_none());
        if same_place {
            assert_eq!(app.items[app.selected.unwrap()].name(), "b");
            assert_eq!(app.grid_scroll.row_anchor(), Some(1));
        }
    }
}

#[test]
fn startup_default_desktop_and_previous_desktop_fallback_keep_typed_list_continuations() {
    // Resolving the Known Folder path is read-only. Its contents are deliberately
    // excluded: adoption below uses a real worker scan of the disposable fixture.
    for previous_fallback in [false, true] {
        let mut app = fixture();
        let disposable = folder(&app, "desktop-adoption");
        specific(&mut app, &disposable);
        app.open_default_startup_target();
        let (tx, mut scanned) = hold_result(&mut app);
        let mut settings = app.settings.clone();
        settings.startup_folder_mode = if previous_fallback {
            StartupFolderMode::Previous
        } else {
            StartupFolderMode::Desktop
        };
        settings.startup_list_restore = Some(StartupListRestore::V1 {
            target: StartupListTarget::Unavailable,
            cursor: Some(ListCursorHint {
                name: "must-not-survive".into(),
                rows_above: Some(7),
            }),
        });
        settings.last_folder = None;
        let desktop = crate::known_folders::desktop_dir();
        let resolved = resolve_default_startup_target(&settings);
        match desktop {
            Some(desktop) => {
                assert!(crate::folder_tree::path_eq(&resolved.requested, &desktop));
                if previous_fallback {
                    assert!(matches!(
                        resolved.default_target,
                        Some(DefaultStartupTarget::Folder {
                            intent: StartupListIntent::RestoreList { cursor: None, .. },
                            cursor: None,
                        })
                    ));
                    scanned.default_target = Some(DefaultStartupTarget::Folder {
                        intent: StartupListIntent::RestoreList {
                            target: physical(&disposable),
                            cursor: None,
                        },
                        cursor: None,
                    });
                } else {
                    assert!(matches!(
                        resolved.default_target,
                        Some(DefaultStartupTarget::Folder {
                            intent: StartupListIntent::ExplicitList,
                            cursor: None,
                        })
                    ));
                }
                tx.send(scanned).unwrap();
                settle(&mut app);
                assert_eq!(app.current_folder.as_ref(), Some(&disposable));
                assert!(app.fullscreen_idx.is_none());
            }
            None => {
                assert!(matches!(
                    resolved.default_target,
                    Some(DefaultStartupTarget::Unavailable)
                ));
                tx.send(resolved).unwrap();
                settle(&mut app);
                assert!(app.current_folder.is_none());
                assert!(app.startup_open_path_resolve_pending.is_none());
            }
        }
    }
}

#[test]
fn startup_default_drives_reading_history_and_previous_drive_list_do_not_scan() {
    for mode in [
        StartupFolderMode::Drives,
        StartupFolderMode::ReadingHistory,
        StartupFolderMode::Previous,
    ] {
        let mut app = fixture();
        app.settings.startup_folder_mode = mode;
        app.settings.startup_list_restore = Some(StartupListRestore::V1 {
            target: StartupListTarget::DriveList,
            cursor: None,
        });
        let before = folder_scan::caller_scan_count_for_test();
        app.open_default_startup_target();
        assert!(app.startup_open_path_resolve_pending.is_some());
        assert!(!app.items_are_drive_list && !app.items_are_reading_history_view);
        let (tx, result) = hold_result(&mut app);
        assert!(result.resolved.is_none());
        assert!(result.folder_scan.is_none() && result.scan_policy.is_none());
        assert!(matches!(
            (&mode, &result.default_target),
            (
                StartupFolderMode::ReadingHistory,
                Some(DefaultStartupTarget::ReadingHistory)
            ) | (
                StartupFolderMode::Drives | StartupFolderMode::Previous,
                Some(DefaultStartupTarget::Drives)
            )
        ));
        tx.send(result).unwrap();
        settle(&mut app);
        assert_eq!(folder_scan::caller_scan_count_for_test(), before);
        assert_eq!(
            app.items_are_reading_history_view,
            mode == StartupFolderMode::ReadingHistory
        );
        assert_eq!(
            app.items_are_drive_list,
            mode != StartupFolderMode::ReadingHistory
        );
    }
}

#[test]
fn startup_default_explicit_not_openable_falls_back_once_and_default_failures_are_terminal() {
    for failure in ["unavailable", "scan", "disconnected"] {
        let mut app = fixture();
        let target = folder(&app, "default");
        specific(&mut app, &target);
        let request = ViewerOpenRequest::preferences();
        assert_eq!(
            app.start_startup_open_request_with_navigation(
                ExplicitOpenPath::new(target.clone(), request),
                StartupOpenPathSource::InitialStartup,
                &egui::Context::default(),
                None,
            ),
            StartupOpenAdmission::Admitted
        );
        let (explicit_tx, mut explicit) = hold_result(&mut app);
        explicit.resolved = None;
        explicit.folder_scan = None;
        explicit.scan_policy = None;
        explicit.default_target = None;
        explicit_tx.send(explicit).unwrap();
        app.poll_startup_open_path_resolve(&egui::Context::default());
        let fallback = app
            .startup_open_path_resolve_pending
            .as_ref()
            .expect("one default fallback");
        assert_ne!(
            fallback.owner.viewer_open_adoption().request(),
            Some(request)
        );
        let (default_tx, mut default) = hold_result(&mut app);
        assert_scanned(&default, &target);
        if failure == "disconnected" {
            drop(default_tx);
        } else {
            if failure == "unavailable" {
                default.default_target = Some(DefaultStartupTarget::Unavailable);
                default.resolved = None;
                default.folder_scan = None;
                default.scan_policy = None;
            } else {
                default.folder_scan = Some(Err("forced default folder scan failure".into()));
            }
            default_tx.send(default).unwrap();
        }
        for _ in 0..3 {
            app.poll_startup_open_path_resolve(&egui::Context::default());
            assert!(
                app.startup_open_path_resolve_pending.is_none(),
                "default {failure} recursively fell back"
            );
            assert!(app.startup_open_path.is_none());
        }
        assert!(app.current_folder.is_none());
        if failure != "unavailable" {
            assert!(
                app.fs_feedback_toast.is_some(),
                "default {failure} must notify"
            );
        }
    }
}

fn acquire_remote(app: &mut App) -> crate::remote_ipc::session::SessionHandle {
    let handle = crate::remote_ipc::session::SessionHandle::new();
    app.set_remote_session_handle(handle.clone());
    handle.acquire(mimageviewer_ipc::SessionAcquireRequest {
        client_id: "startup-default-test".into(),
        peer: mimageviewer_ipc::SessionPeerInfo {
            connection_kind: mimageviewer_ipc::SessionConnectionKind::Direct,
            device_name: None,
        },
    });
    let deadline = Instant::now() + Duration::from_secs(30);
    while handle.snapshot().phase != crate::remote_ipc::session::RemoteControlPhase::RemoteActive {
        app.poll_remote_session(&egui::Context::default());
        assert!(
            Instant::now() < deadline,
            "Remote acquisition did not settle"
        );
        std::thread::yield_now();
    }
    handle
}

fn release_remote(app: &mut App, handle: &crate::remote_ipc::session::SessionHandle) {
    handle.local_disconnect();
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        app.poll_remote_session(&egui::Context::default());
        if handle.snapshot().phase == crate::remote_ipc::session::RemoteControlPhase::Local {
            break;
        }
        assert!(Instant::now() < deadline, "Remote release did not settle");
        std::thread::yield_now();
    }
    app.poll_remote_session(&egui::Context::default());
}

#[test]
fn startup_default_remote_return_redispatches_asynchronously_without_empty_explicit_request() {
    let mut app = fixture();
    let target = folder(&app, "remote-default");
    specific(&mut app, &target);
    app.open_default_startup_target();
    let (old_tx, old_result) = hold_result(&mut app);
    let cancel = app
        .startup_open_path_resolve_pending
        .as_ref()
        .unwrap()
        .cancel
        .clone();
    let handle = acquire_remote(&mut app);
    assert!(cancel.load(Ordering::Relaxed));
    assert!(app.startup_open_path_resolve_pending.is_none());
    assert!(
        app.startup_open_path.is_none(),
        "default placeholder must not become an explicit empty path"
    );
    assert!(old_tx.send(old_result).is_err());
    let before = folder_scan::caller_scan_count_for_test();
    release_remote(&mut app, &handle);
    assert!(app.startup_open_path_resolve_pending.is_some());
    assert!(app.current_folder.is_none());
    assert_eq!(folder_scan::caller_scan_count_for_test(), before);
    settle(&mut app);
    assert_eq!(folder_scan::caller_scan_count_for_test(), before);
    assert_eq!(app.current_folder.as_ref(), Some(&target));
}

#[test]
fn startup_default_remote_return_preserves_an_already_adopted_local_folder() {
    let mut app = fixture();
    let origin = folder(&app, "adopted");
    app.load_folder(origin.clone());
    settle(&mut app);
    let target = folder(&app, "interrupted-default");
    specific(&mut app, &target);
    app.open_default_startup_target();
    let (old_tx, old_result) = hold_result(&mut app);
    let handle = acquire_remote(&mut app);
    assert!(old_tx.send(old_result).is_err());
    assert!(app.startup_open_path.is_none());
    release_remote(&mut app, &handle);
    settle(&mut app);
    assert!(app.startup_open_path_resolve_pending.is_none());
    assert_eq!(app.current_folder.as_ref(), Some(&origin));
}

#[test]
fn startup_default_late_tree_favorite_and_dfs_requests_retire_the_old_default() {
    for route in ["tree", "favorite", "dfs"] {
        let mut app = fixture();
        let origin = folder(&app, "origin");
        app.load_folder(origin.clone());
        settle(&mut app);
        let stale = folder(&app, "old-default");
        let next = folder(&app, "later-navigation");
        specific(&mut app, &stale);
        app.open_default_startup_target();
        let (old_tx, old_result) = hold_result(&mut app);
        let cancel = app
            .startup_open_path_resolve_pending
            .as_ref()
            .unwrap()
            .cancel
            .clone();
        match route {
            "tree" => app.start_user_folder_pane_open(next),
            "favorite" => app.dispatch_main_input_nav_immediate(
                crate::ui_main::AddressBarNav::WorkerFavorite(next),
                false,
            ),
            "dfs" => app.start_folder_nav(origin, true, FolderNavMode::Grid),
            _ => unreachable!(),
        }
        assert!(
            cancel.load(Ordering::Relaxed),
            "{route} did not retire default"
        );
        assert!(app.startup_open_path_resolve_pending.is_none());
        assert!(
            old_tx.send(old_result).is_err(),
            "{route} left old receiver alive"
        );
        let folder_before = app.current_folder.clone();
        let generation = app.items_generation;
        app.poll_startup_open_path_resolve(&egui::Context::default());
        assert_eq!(app.current_folder, folder_before);
        assert_eq!(app.items_generation, generation);
        assert_ne!(app.current_folder.as_ref(), Some(&stale));
    }
}
