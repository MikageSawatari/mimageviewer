//! §1.853: real resolver workers and controlled completion channels, without a native UI.

use super::*;
use crate::app::tests::phase_c_support::{AppTestEnv, setup_app};
use crate::bookmark_browser::{
    BookmarkOpenRequestId, BookmarkOpenRequestOwner, BookmarkViewReturnTarget, PendingBookOpen,
    PendingBookOpenStage, PendingBookmarkOpen,
};
use std::time::{Duration, Instant};

fn fixture() -> (AppTestEnv, PathBuf) {
    let mut app = setup_app();
    app.active_quick_folder_slot = None;
    app.settings.detached_viewer_enabled = false;
    app.settings.detached_viewer_open_images_in_window = false;
    app.settings.auto_fullscreen_image_folders = false;
    app.settings.auto_fullscreen_zip_pdf = false;
    let origin = app.tmp.path().join("origin");
    std::fs::create_dir(&origin).unwrap();
    app.load_folder(origin.clone());
    app.finish_page_identity_preparation_for_test();
    (app, origin)
}

fn arm_bookmark(app: &mut App, path: &Path, id: u64) -> BookmarkOpenRequestOwner {
    let request_id = BookmarkOpenRequestId(id);
    let target = BookmarkViewReturnTarget::Book(path.to_path_buf());
    app.bookmark_view_state = Some(BookmarkViewState::Opening {
        target: target.clone(),
        grid: BookmarkViewReturnGridState {
            row_keys: vec![(0, id as i64)],
            selected_key: Some((0, id as i64)),
            opened_key: Some((0, id as i64)),
            scroll_offset_y: 0.0,
        },
    });
    app.bookmark_open_pending = Some(PendingBookmarkOpen::Book(PendingBookOpen {
        request_id,
        bookmark_source: crate::pdf_loader::LeasedEpubPath::try_new(path.to_path_buf()).unwrap(),
        bookmark: crate::book_bookmarks::BookBookmark {
            id: id as i64,
            container_key: crate::book_bookmarks::container_key(path),
            container_path: path.to_path_buf(),
            container_kind: crate::book_bookmarks::BookContainerKind::ImageFolder,
            page_identity: crate::book_bookmarks::PageIdentity::RelativePath("page.png".into()),
            page_index_hint: 0,
            created_at_ms: 1,
            title: None,
        },
        relative_page_provenance: None,
        started_at: Instant::now(),
        stage: PendingBookOpenStage::Resolving,
    }));
    BookmarkOpenRequestOwner {
        request_id,
        target,
        #[cfg(windows)]
        detached_lease: None,
    }
}

fn pending(
    owner: StartupOpenPathOwner,
    path: &Path,
) -> (
    StartupOpenPathResolvePending,
    mpsc::Sender<StartupOpenPathResolveResult>,
) {
    let (tx, rx) = mpsc::channel();
    (
        StartupOpenPathResolvePending {
            input_kind: StartupResolveInputKind::Explicit(StartupOpenPathSource::InitialStartup),
            requested: crate::pdf_loader::LeasedEpubPath::try_new(path.to_path_buf()).unwrap(),
            owner,
            navigation: None,
            cancel: Arc::new(AtomicBool::new(false)),
            rx,
            started_at: Instant::now(),
            toast_shown: false,
            scan_phase: Arc::new(StartupScanPhase::new()),
            held_resolve_for_activation_admission: None,
            diagnostic: None,
        },
        tx,
    )
}

fn scan_phase(elapsed: Duration) -> Arc<StartupScanPhase> {
    Arc::new(StartupScanPhase {
        origin: Instant::now() - elapsed,
        started_us: std::sync::atomic::AtomicU64::new(1),
    })
}

fn adopt_real_worker_result(app: &mut App, target: &Path, caller_scans: usize) {
    let pending = app.startup_open_path_resolve_pending.as_mut().unwrap();
    let result = pending.rx.recv_timeout(Duration::from_secs(30)).unwrap();
    assert!(result.folder_scan.as_ref().is_some_and(Result::is_ok));
    assert_eq!(result.resolved.as_ref().unwrap().path, target);
    assert_eq!(result.scan_policy.as_ref(), Some(&app.folder_scan_policy()));
    assert!(app.main_startup_scan_elapsed().is_some());
    assert_eq!(folder_scan::caller_scan_count_for_test(), caller_scans);
    let (tx, rx) = mpsc::channel();
    app.startup_open_path_resolve_pending.as_mut().unwrap().rx = rx;
    tx.send(result).unwrap();
    app.poll_startup_open_path_resolve(&egui::Context::default());
    app.finish_page_identity_preparation_for_test();
    assert!(app.startup_open_path_resolve_pending.is_none());
    assert_eq!(folder_scan::caller_scan_count_for_test(), caller_scans);
    assert_eq!(app.current_folder.as_deref(), Some(target));
    assert!(app.items.iter().any(|item| item.name() == "page.png"));
}

#[test]
fn section1853_initial_activation_agent_and_bookmark_directories_scan_only_on_worker() {
    for source in [
        StartupOpenPathSource::InitialStartup,
        StartupOpenPathSource::Activation,
        StartupOpenPathSource::Agent,
        StartupOpenPathSource::Bookmark,
    ] {
        let (mut app, origin) = fixture();
        let target = app.tmp.path().join("target");
        std::fs::create_dir(&target).unwrap();
        image::RgbaImage::new(2, 2)
            .save(target.join("page.png"))
            .unwrap();
        if source == StartupOpenPathSource::Bookmark {
            arm_bookmark(&mut app, &target, 853);
        }
        let scans = folder_scan::caller_scan_count_for_test();
        assert_eq!(
            app.start_startup_open_path_resolve(target.clone(), source, &egui::Context::default()),
            StartupOpenAdmission::Admitted
        );
        assert_eq!(folder_scan::caller_scan_count_for_test(), scans);
        assert_eq!(app.current_folder.as_ref(), Some(&origin));
        adopt_real_worker_result(&mut app, &target, scans);
    }
}

#[test]
fn section1853_archive_suffix_directories_keep_directory_worker_classification() {
    for name in ["directory.zip", "directory.pdf", "directory.epub"] {
        let (mut app, _) = fixture();
        let target = app.tmp.path().join(name);
        std::fs::create_dir(&target).unwrap();
        image::RgbaImage::new(2, 2)
            .save(target.join("page.png"))
            .unwrap();
        let scans = folder_scan::caller_scan_count_for_test();
        assert_eq!(
            app.start_startup_open_path_resolve(
                target.clone(),
                StartupOpenPathSource::InitialStartup,
                &egui::Context::default(),
            ),
            StartupOpenAdmission::Admitted
        );
        adopt_real_worker_result(&mut app, &target, scans);
        assert!(app.zip_enumerate_pending.is_none());
        assert!(app.pdf_enumerate_pending.is_none());
        assert!(app.archive_convert.is_none());
    }
}

#[test]
fn section1853_requested_image_parent_scans_off_caller_and_keeps_file_selection() {
    for source in [
        StartupOpenPathSource::InitialStartup,
        StartupOpenPathSource::Activation,
        StartupOpenPathSource::Agent,
    ] {
        let (mut app, _) = fixture();
        let target = app.tmp.path().join("image-parent");
        std::fs::create_dir(&target).unwrap();
        let requested = target.join("page.png");
        for name in ["first.png", "page.png"] {
            image::RgbaImage::new(2, 2).save(target.join(name)).unwrap();
        }
        let scans = folder_scan::caller_scan_count_for_test();
        assert_eq!(
            app.start_startup_open_path_resolve(requested, source, &egui::Context::default()),
            StartupOpenAdmission::Admitted
        );
        assert_eq!(folder_scan::caller_scan_count_for_test(), scans);
        adopt_real_worker_result(&mut app, &target, scans);
        assert_eq!(app.items[app.selected.unwrap()].name(), "page.png");
        assert_eq!(app.items[app.fullscreen_idx.unwrap()].name(), "page.png");
    }
}

#[test]
fn section1853_resolve_and_scan_share_stage_predicate_and_visual_grace() {
    let (mut app, _) = fixture();
    let target = app.tmp.path().join("controlled");
    let (mut request, _tx) = pending(
        StartupOpenPathOwner::InitialStartup(ViewerOpenRequest::preferences()),
        &target,
    );
    request.started_at = Instant::now() - Duration::from_secs(1);
    app.startup_open_path_resolve_pending = Some(request);
    assert!(app.main_startup_scan_elapsed().is_none());
    assert!(!app.location_navigation_pending());
    assert!(app.grid_item_input_allowed());
    app.poll_startup_open_path_resolve(&egui::Context::default());
    assert!(
        app.startup_open_path_resolve_pending
            .as_ref()
            .unwrap()
            .toast_shown
    );
    app.fs_feedback_toast = None;
    let request = app.startup_open_path_resolve_pending.as_mut().unwrap();
    request.toast_shown = false;
    request.scan_phase.begin();
    assert!(app.main_startup_scan_elapsed().is_some());
    assert!(app.location_navigation_pending());
    assert_eq!(
        app.grid_item_input_block(),
        Some(crate::filename_stack_ui::GridItemInputBlock::DirectNavigationScan)
    );
    assert!(app.direct_navigation_wait_overlay_delay().is_some());
    app.poll_startup_open_path_resolve(&egui::Context::default());
    assert!(app.fs_feedback_toast.is_none());
    assert!(
        !app.startup_open_path_resolve_pending
            .as_ref()
            .unwrap()
            .toast_shown
    );
    app.startup_open_path_resolve_pending
        .as_mut()
        .unwrap()
        .scan_phase = scan_phase(Duration::from_millis(501));
    assert!(app.direct_navigation_wait_overlay_delay().is_none());
    assert!(!app.grid_item_input_allowed());
}

#[test]
#[cfg(windows)]
fn section1853_main_scan_does_not_block_independent_mounted_grid_or_detached_owner() {
    let (mut app, _) = fixture();
    app.build_active_context_for_test(Some(185_303), DetachedSource::Image, |reader| {
        crate::app::tests::seed_snapshot_context(
            reader,
            "C:/section1853-independent",
            &["C:/section1853-independent/page.png"],
        );
    });
    let target = app.tmp.path().join("controlled");
    let (mut request, _tx) = pending(
        StartupOpenPathOwner::InitialStartup(ViewerOpenRequest::preferences()),
        &target,
    );
    request.scan_phase = scan_phase(Duration::from_secs(1));
    let cancel = Arc::clone(&request.cancel);
    app.startup_open_path_resolve_pending = Some(request);
    assert!(!app.grid_item_input_allowed());
    app.with_active_viewer_context(|reader| {
        assert!(reader.startup_open_path_resolve_pending.is_some());
        assert!(reader.main_startup_scan_elapsed().is_none());
        assert!(!reader.location_navigation_pending());
        assert!(reader.grid_item_input_allowed());
        assert!(reader.direct_navigation_wait_overlay_delay().is_none());
    })
    .expect("the independent grid stays mountable");
    assert!(!cancel.load(Ordering::Relaxed));
    assert!(app.main_startup_scan_elapsed().is_some());
    let owner = BookmarkOpenRequestOwner {
        request_id: BookmarkOpenRequestId(1853),
        target: BookmarkViewReturnTarget::Book(target),
        detached_lease: Some(DetachedSessionLease { window_id: 185_303 }),
    };
    app.startup_open_path_resolve_pending
        .as_mut()
        .unwrap()
        .owner = StartupOpenPathOwner::Bookmark(owner);
    assert!(app.main_startup_scan_elapsed().is_none());
    assert!(!app.location_navigation_pending());
    assert!(app.grid_item_input_allowed());
}

#[test]
fn section1853_bookmark_scan_timeout_exemption_requires_same_request_and_scan_stage() {
    for mode in ["scan", "resolving", "different-request"] {
        let (mut app, _) = fixture();
        let target = app.tmp.path().join("bookmark");
        let owner = arm_bookmark(&mut app, &target, 853);
        app.bookmark_open_pending
            .as_mut()
            .unwrap()
            .book_mut()
            .unwrap()
            .started_at = Instant::now() - Duration::from_secs(46);
        let mut resolve_owner = owner.clone();
        if mode == "different-request" {
            resolve_owner.request_id = BookmarkOpenRequestId(854);
        }
        let (mut request, _tx) = pending(StartupOpenPathOwner::Bookmark(resolve_owner), &target);
        if mode != "resolving" {
            request.scan_phase = scan_phase(Duration::from_secs(46));
        }
        app.startup_open_path_resolve_pending = Some(request);
        assert_eq!(
            app.main_startup_scan_owns_bookmark(owner.request_id),
            mode == "scan"
        );
        app.poll_bookmark_book_open(&egui::Context::default());
        assert_eq!(app.bookmark_open_pending.is_some(), mode == "scan");
        if mode == "scan" {
            assert!(app.fs_feedback_toast.is_none());
        } else {
            assert!(app.fs_feedback_toast.is_some());
        }
    }
}

struct ForceSpawnFailure;

impl ForceSpawnFailure {
    fn new() -> Self {
        force_startup_open_resolve_spawn_failure_for_test(true);
        Self
    }
}

impl Drop for ForceSpawnFailure {
    fn drop(&mut self) {
        force_startup_open_resolve_spawn_failure_for_test(false);
    }
}

#[test]
fn section1853_spawn_failure_and_disconnected_initial_are_terminal_without_default_fallback() {
    for spawn_failure in [false, true] {
        let (mut app, origin) = fixture();
        let target = app.tmp.path().join("not-read-on-caller");
        app.settings.startup_folder_mode = crate::settings::StartupFolderMode::Specific;
        app.settings.startup_folder_path = Some(origin.clone());
        let scans = folder_scan::caller_scan_count_for_test();
        if spawn_failure {
            let _failure = ForceSpawnFailure::new();
            assert_eq!(
                app.start_startup_open_path_resolve(
                    target,
                    StartupOpenPathSource::InitialStartup,
                    &egui::Context::default(),
                ),
                StartupOpenAdmission::Refused(ActivationOpenRefusal::Internal)
            );
        } else {
            let (request, tx) = pending(
                StartupOpenPathOwner::InitialStartup(ViewerOpenRequest::preferences()),
                &target,
            );
            app.startup_open_path_resolve_pending = Some(request);
            drop(tx);
            app.poll_startup_open_path_resolve(&egui::Context::default());
        }
        assert!(app.startup_open_path_resolve_pending.is_none());
        assert!(app.startup_open_path.is_none());
        assert_eq!(app.current_folder.as_ref(), Some(&origin));
        assert_eq!(folder_scan::caller_scan_count_for_test(), scans);
        assert!(app.fs_feedback_toast.is_some());
    }
}

#[test]
fn section1853_activation_spawn_failure_and_disconnect_resume_exact_held_resolver() {
    for spawn_failure in [false, true] {
        let (mut app, origin) = fixture();
        let target = app.tmp.path().join("activation");
        let held_owner = StartupOpenPathOwner::InitialStartup(ViewerOpenRequest::preferences());
        let (mut held, held_tx) = pending(held_owner.clone(), &origin);
        held.scan_phase = scan_phase(Duration::from_secs(1));
        let held_phase = Arc::clone(&held.scan_phase);
        let held_cancel = Arc::clone(&held.cancel);
        if spawn_failure {
            let _failure = ForceSpawnFailure::new();
            assert_eq!(
                app.spawn_startup_resolver(
                    StartupResolveInput::Explicit {
                        requested: target.clone(),
                        source: StartupOpenPathSource::Activation,
                        bookmark: None,
                    },
                    StartupOpenPathOwner::Activation(ViewerOpenRequest::preferences()),
                    crate::pdf_loader::LeasedEpubPath::try_new(target).unwrap(),
                    None,
                    Some(Box::new(held)),
                    false,
                    &egui::Context::default(),
                ),
                StartupOpenAdmission::Refused(ActivationOpenRefusal::Internal)
            );
        } else {
            let (mut activation, tx) = pending(
                StartupOpenPathOwner::Activation(ViewerOpenRequest::preferences()),
                &target,
            );
            activation.held_resolve_for_activation_admission = Some(Box::new(held));
            app.startup_open_path_resolve_pending = Some(activation);
            drop(tx);
            app.poll_startup_open_path_resolve(&egui::Context::default());
        }
        let resumed = app.startup_open_path_resolve_pending.as_ref().unwrap();
        assert_eq!(resumed.owner, held_owner);
        assert!(Arc::ptr_eq(&resumed.cancel, &held_cancel));
        assert!(Arc::ptr_eq(&resumed.scan_phase, &held_phase));
        assert!(!held_cancel.load(Ordering::Relaxed));
        assert!(matches!(
            resumed.rx.try_recv(),
            Err(mpsc::TryRecvError::Empty)
        ));
        assert_eq!(app.current_folder.as_ref(), Some(&origin));
        // The exact held channel is still live; dropping its sender changes this receiver.
        drop(held_tx);
        assert!(matches!(
            app.startup_open_path_resolve_pending
                .as_ref()
                .unwrap()
                .rx
                .try_recv(),
            Err(mpsc::TryRecvError::Disconnected)
        ));
    }
}

fn start_policy_rescan(
    app: &mut App,
    target: &Path,
    owner: StartupOpenPathOwner,
) -> Arc<AtomicBool> {
    let (mut request, tx) = pending(owner.clone(), target);
    request.scan_phase = scan_phase(Duration::from_secs(46));
    let phase = Arc::clone(&request.scan_phase);
    let cancel = Arc::clone(&request.cancel);
    tx.send(obsolete_scan_result(app, target)).unwrap();
    app.startup_open_path_resolve_pending = Some(request);
    app.poll_startup_open_path_resolve(&egui::Context::default());
    let continued = app.startup_open_path_resolve_pending.as_ref().unwrap();
    assert_eq!(continued.owner, owner);
    assert!(Arc::ptr_eq(&continued.cancel, &cancel));
    assert!(Arc::ptr_eq(&continued.scan_phase, &phase));
    assert!(continued.scan_phase.elapsed().unwrap() >= Duration::from_secs(46));
    cancel
}

fn obsolete_scan_result(app: &App, target: &Path) -> StartupOpenPathResolveResult {
    let mut policy = app.folder_scan_policy();
    policy.susie_extensions_generation = policy.susie_extensions_generation.wrapping_add(1);
    StartupOpenPathResolveResult {
        requested: target.to_path_buf(),
        resolved: Some(crate::folder_tree::OpenablePathResolution {
            path: target.to_path_buf(),
            kind: crate::folder_tree::OpenablePathKind::Directory,
            requested_is_file: false,
        }),
        rar_volume_proof: None,
        bookmark_relative_page_openable: None,
        elapsed_ms: 1.0,
        direct_media: None,
        folder_scan: Some(Err("obsolete result must never be adopted".into())),
        scan_policy: Some(policy),
        scan_policy_mixed: false,
        default_target: None,
    }
}

#[test]
fn section1853_policy_rescan_spawn_failure_is_terminal_without_synchronous_retry() {
    let (mut app, origin) = fixture();
    let target = app.tmp.path().join("rescan");
    let (mut request, tx) = pending(
        StartupOpenPathOwner::InitialStartup(ViewerOpenRequest::preferences()),
        &target,
    );
    request.scan_phase.begin();
    let cancel = Arc::clone(&request.cancel);
    tx.send(obsolete_scan_result(&app, &target)).unwrap();
    app.startup_open_path_resolve_pending = Some(request);
    let scans = folder_scan::caller_scan_count_for_test();
    let _failure = ForceSpawnFailure::new();
    app.poll_startup_open_path_resolve(&egui::Context::default());
    assert!(cancel.load(Ordering::Relaxed));
    assert!(app.startup_open_path_resolve_pending.is_none());
    assert!(app.startup_open_path.is_none());
    assert_eq!(app.current_folder.as_ref(), Some(&origin));
    assert_eq!(folder_scan::caller_scan_count_for_test(), scans);
    assert!(app.fs_feedback_toast.is_some());
}

#[test]
#[cfg(windows)]
fn section1853_detached_bookmark_resolver_worker_does_not_scan_or_block_main() {
    let (mut app, _) = fixture();
    let target = app.tmp.path().join("detached-bookmark");
    std::fs::create_dir(&target).unwrap();
    image::RgbaImage::new(2, 2)
        .save(target.join("page.png"))
        .unwrap();
    let mut owner = arm_bookmark(&mut app, &target, 1853);
    owner.detached_lease = Some(DetachedSessionLease { window_id: 185_304 });
    let bookmark = app
        .bookmark_open_pending
        .as_ref()
        .unwrap()
        .book()
        .unwrap()
        .bookmark
        .clone();
    let scans = folder_scan::caller_scan_count_for_test();
    assert_eq!(
        app.spawn_startup_resolver(
            StartupResolveInput::Explicit {
                requested: target.clone(),
                source: StartupOpenPathSource::Bookmark,
                bookmark: Some(bookmark),
            },
            StartupOpenPathOwner::Bookmark(owner),
            crate::pdf_loader::LeasedEpubPath::try_new(target.clone()).unwrap(),
            None,
            None,
            false,
            &egui::Context::default(),
        ),
        StartupOpenAdmission::Admitted
    );
    let request = app.startup_open_path_resolve_pending.as_ref().unwrap();
    let result = request.rx.recv_timeout(Duration::from_secs(30)).unwrap();
    assert_eq!(result.resolved.as_ref().unwrap().path, target);
    assert_eq!(result.bookmark_relative_page_openable, Some(true));
    assert!(result.folder_scan.is_none());
    assert!(result.scan_policy.is_none());
    assert!(request.scan_phase.elapsed().is_none());
    assert!(app.main_startup_scan_elapsed().is_none());
    assert!(app.grid_item_input_allowed());
    assert!(!app.location_navigation_pending());
    assert_eq!(folder_scan::caller_scan_count_for_test(), scans);
    // This preparation test deliberately stops before adoption: a native reader is unnecessary
    // to prove that a leased request leaves enumeration to its existing detached owner.
}

#[test]
fn section1853_bookmark_over_deadline_keeps_timeout_exemption_through_policy_rescan() {
    let (mut app, _) = fixture();
    let target = app.tmp.path().join("bookmark-rescan");
    std::fs::create_dir(&target).unwrap();
    let owner = arm_bookmark(&mut app, &target, 853);
    app.bookmark_open_pending
        .as_mut()
        .unwrap()
        .book_mut()
        .unwrap()
        .started_at = Instant::now() - Duration::from_secs(46);
    let cancel = start_policy_rescan(
        &mut app,
        &target,
        StartupOpenPathOwner::Bookmark(owner.clone()),
    );
    app.poll_bookmark_book_open(&egui::Context::default());
    assert_eq!(
        app.bookmark_open_pending
            .as_ref()
            .map(PendingBookmarkOpen::request_id),
        Some(owner.request_id)
    );
    assert!(!cancel.load(Ordering::Relaxed));
    assert!(app.main_startup_scan_owns_bookmark(owner.request_id));
    assert!(app.fs_feedback_toast.is_none());
    app.cancel_unresolved_open_for_navigation(true);
    assert!(cancel.load(Ordering::Relaxed));
    assert!(app.bookmark_open_pending.is_none());
}
