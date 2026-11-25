//! §1.853: policy refresh goes through the resolver's existing poll boundary.
//! All files and settings belong to AppTestEnv; no native window is launched.
#![cfg(windows)]

use super::*;
use crate::app::tests::phase_c_support::{AppTestEnv, setup_app};

fn fixture() -> (AppTestEnv, PathBuf) {
    let mut app = setup_app();
    app.initialized = true;
    app.active_quick_folder_slot = None;
    app.settings.detached_viewer_enabled = false;
    app.settings.detached_viewer_open_images_in_window = false;
    app.settings.auto_fullscreen_zip_pdf = false;
    app.settings.auto_fullscreen_image_folders = false;
    app.settings.show_hidden_files = false;
    app.settings.file_type_visibility = Default::default();
    app.settings.archive_file_handling = crate::settings::ArchiveFileHandling::Ask;
    app.settings.epub_file_handling = crate::settings::EpubFileHandling::Ask;
    let origin = app.tmp.path().join("origin");
    let target = app.tmp.path().join("policy-target");
    std::fs::create_dir(&origin).unwrap();
    std::fs::create_dir(&target).unwrap();
    for name in [
        "keep.jpg",
        "visible.png",
        "hidden.jpg",
        "book.7z",
        "story.epub",
    ] {
        std::fs::write(target.join(name), b"fixture").unwrap();
    }
    {
        use std::os::windows::ffi::OsStrExt;
        use windows::Win32::Storage::FileSystem::{FILE_ATTRIBUTE_HIDDEN, SetFileAttributesW};
        let wide = target
            .join("hidden.jpg")
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect::<Vec<_>>();
        unsafe { SetFileAttributesW(windows::core::PCWSTR(wide.as_ptr()), FILE_ATTRIBUTE_HIDDEN) }
            .unwrap();
    }
    app.load_folder(origin);
    app.finish_page_identity_preparation_for_test();
    (app, target)
}

fn pending(
    requested: PathBuf,
) -> (
    StartupOpenPathResolvePending,
    mpsc::Sender<StartupOpenPathResolveResult>,
) {
    let (tx, rx) = mpsc::channel();
    (
        StartupOpenPathResolvePending {
            requested: crate::pdf_loader::LeasedEpubPath::try_new(requested).unwrap(),
            owner: StartupOpenPathOwner::InitialStartup(ViewerOpenRequest::preferences()),
            navigation: None,
            cancel: Arc::new(AtomicBool::new(false)),
            rx,
            started_at: std::time::Instant::now(),
            toast_shown: false,
            input_kind: StartupResolveInputKind::Explicit(StartupOpenPathSource::InitialStartup),
            scan_phase: Arc::new(StartupScanPhase::new()),
            held_resolve_for_activation_admission: None,
            diagnostic: None,
        },
        tx,
    )
}

fn scanned_result(app: &App, target: &Path) -> StartupOpenPathResolveResult {
    let policy = app.folder_scan_policy();
    StartupOpenPathResolveResult {
        requested: target.to_path_buf(),
        resolved: Some(crate::folder_tree::OpenablePathResolution {
            path: target.to_path_buf(),
            kind: crate::folder_tree::OpenablePathKind::Directory,
            requested_is_file: false,
        }),
        rar_volume_proof: None,
        bookmark_relative_page_openable: None,
        elapsed_ms: 1853.25,
        direct_media: None,
        folder_scan: Some(policy.scan(target, None).map_err(|error| error.to_string())),
        scan_policy: Some(policy),
        scan_policy_mixed: false,
        default_target: None,
    }
}

fn receive_scan(app: &App) -> StartupOpenPathResolveResult {
    app.startup_open_path_resolve_pending
        .as_ref()
        .expect("same request still pending")
        .rx
        .recv_timeout(std::time::Duration::from_secs(30))
        .expect("scan-only continuation replies")
}

fn ready_for_poll(app: &mut App, result: StartupOpenPathResolveResult) {
    let (tx, rx) = mpsc::channel();
    app.startup_open_path_resolve_pending.as_mut().unwrap().rx = rx;
    assert!(tx.send(result).is_ok());
}

#[derive(Clone, Copy)]
enum Change {
    Hidden,
    Visibility,
    Archive,
    Epub,
    SusiePublication,
    PageCountFingerprint,
    Mixed,
}

fn check_policy_refresh(change: Change) {
    let (mut app, target) = fixture();
    if matches!(change, Change::PageCountFingerprint) {
        app.settings.sort_order = crate::settings::SortOrder::PageCountAsc;
    }
    let mut old = scanned_result(&app, &target);
    let (mut request, tx) = pending(target.clone());
    let proof = app.copied_destination_source_proof();
    request.navigation = Some(app.capture_main_list_navigation(
        MainHistoryOperation::Direct(DirectNavigationPurpose::Navigation),
        proof,
    ));
    request.scan_phase.begin();
    let cancel = Arc::clone(&request.cancel);
    let phase = Arc::clone(&request.scan_phase);
    let started_at = request.started_at;
    let navigation = request.navigation.clone();
    let original_folder = app.current_folder.clone();
    let original_history = app.folder_nav_history_snapshot();
    let original_generation = app.items_generation;
    #[cfg(feature = "test-script")]
    let loads = app.test_script_folder_load_requests;
    match change {
        Change::Hidden => app.settings.show_hidden_files = true,
        Change::Visibility => {
            app.settings
                .file_type_visibility
                .excluded_extensions
                .insert("png".into());
        }
        Change::Archive => {
            app.settings.archive_file_handling = crate::settings::ArchiveFileHandling::Ignore
        }
        Change::Epub => app.settings.epub_file_handling = crate::settings::EpubFileHandling::Ignore,
        // A result from the preceding publication, without mutating the global
        // process pool used by other tests. Real publication is covered by the
        // susie_loader state test.
        Change::SusiePublication => {
            let stale = old.scan_policy.as_mut().unwrap();
            stale.susie_pool_revision = stale.susie_pool_revision.wrapping_sub(1);
        }
        Change::PageCountFingerprint => {
            let before = app.listing_page_count_facts_fingerprint();
            assert!(before.is_some());
            app.settings.skip_duplicate_images = !app.settings.skip_duplicate_images;
            assert_ne!(before, app.listing_page_count_facts_fingerprint());
        }
        Change::Mixed => old.scan_policy_mixed = true,
    }
    // A fresh physical row makes using the old scan observable even when only
    // the Susie publication changed or the result was marked mixed.
    std::fs::write(target.join("fresh.jpg"), b"fixture").unwrap();
    app.startup_open_path_resolve_pending = Some(request);
    assert!(tx.send(old).is_ok());
    let caller_scans = folder_scan::caller_scan_count_for_test();
    let ctx = egui::Context::default();
    app.poll_startup_open_path_resolve(&ctx);
    {
        let retained = app
            .startup_open_path_resolve_pending
            .as_ref()
            .expect("continuation retains owner");
        assert!(Arc::ptr_eq(&retained.cancel, &cancel));
        assert!(Arc::ptr_eq(&retained.scan_phase, &phase));
        assert_eq!(retained.started_at, started_at);
        assert_eq!(retained.navigation, navigation);
        assert!(!cancel.load(Ordering::Relaxed));
    }
    assert_eq!(app.current_folder, original_folder);
    assert_eq!(app.folder_nav_history_snapshot(), original_history);
    assert_eq!(app.items_generation, original_generation);
    assert_eq!(folder_scan::caller_scan_count_for_test(), caller_scans);
    #[cfg(feature = "test-script")]
    assert_eq!(app.test_script_folder_load_requests, loads);

    let fresh = receive_scan(&app);
    assert_eq!(fresh.scan_policy.as_ref(), Some(&app.folder_scan_policy()));
    assert!(!fresh.scan_policy_mixed);
    assert_eq!(fresh.elapsed_ms, 1853.25, "resolution was not repeated");
    let scan = fresh.folder_scan.as_ref().unwrap().as_ref().unwrap();
    let media: HashSet<_> = scan
        .all_media
        .iter()
        .map(|entry| {
            entry
                .path
                .file_name()
                .unwrap()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    assert!(media.contains("fresh.jpg"));
    assert!(media.contains("keep.jpg"));
    assert_eq!(
        media.contains("hidden.jpg"),
        matches!(change, Change::Hidden)
    );
    assert_eq!(
        media.contains("visible.png"),
        !matches!(change, Change::Visibility)
    );
    assert_eq!(
        scan.omitted.counts.ignored_archive,
        usize::from(matches!(change, Change::Archive))
    );
    assert_eq!(
        scan.omitted.counts.ignored_epub,
        usize::from(matches!(change, Change::Epub))
    );
    ready_for_poll(&mut app, fresh);
    app.poll_startup_open_path_resolve(&ctx);
    app.finish_page_identity_preparation_for_test();
    assert!(app.startup_open_path_resolve_pending.is_none());
    assert_eq!(app.current_folder.as_deref(), Some(target.as_path()));
    assert!(app.items.iter().any(|item| item.name() == "fresh.jpg"));
    assert_eq!(
        folder_scan::caller_scan_count_for_test(),
        caller_scans,
        "both mismatch handling and adoption must avoid UI scanning"
    );
    #[cfg(feature = "test-script")]
    assert_eq!(
        app.test_script_folder_load_requests, loads,
        "scan continuation must not admit another load"
    );
}

#[test]
fn section1853_hidden_setting_rescans_same_request_before_adoption() {
    check_policy_refresh(Change::Hidden);
}

#[test]
fn section1853_visibility_setting_rescans_same_request_before_adoption() {
    check_policy_refresh(Change::Visibility);
}

#[test]
fn section1853_archive_setting_rescans_same_request_before_adoption() {
    check_policy_refresh(Change::Archive);
}

#[test]
fn section1853_epub_setting_rescans_same_request_before_adoption() {
    check_policy_refresh(Change::Epub);
}

#[test]
fn section1853_susie_publication_rescans_same_request_before_adoption() {
    check_policy_refresh(Change::SusiePublication);
}

#[test]
fn section1853_page_count_fingerprint_rescans_same_request_before_adoption() {
    check_policy_refresh(Change::PageCountFingerprint);
}

#[test]
fn section1853_mixed_recognition_rescans_even_with_equal_policy() {
    check_policy_refresh(Change::Mixed);
}

#[test]
fn section1853_scan_continuation_keeps_pending_identity_and_full_resolved_payload() {
    let (mut app, target) = fixture();
    let requested = app.tmp.path().join("already-resolved.epub");
    assert!(!requested.exists());
    let (mut request, tx) = pending(requested.clone());
    assert!(request.requested.has_epub_lease());
    request.toast_shown = true;
    request.started_at = std::time::Instant::now();
    request.scan_phase.begin();
    request.diagnostic = Some(InitialNavigationTrace::begin("section1853-policy-test"));
    let trace = request.diagnostic.as_ref().unwrap().correlation();
    let proof = app.copied_destination_source_proof();
    request.navigation =
        Some(app.capture_main_list_navigation(MainHistoryOperation::SameLocation, proof));
    let navigation = request.navigation.clone();
    let (held, _held_tx) = pending(app.tmp.path().join("held-request"));
    let held_cancel = Arc::clone(&held.cancel);
    let held_phase = Arc::clone(&held.scan_phase);
    request.held_resolve_for_activation_admission = Some(Box::new(held));
    let phase = Arc::clone(&request.scan_phase);
    let phase_stamp = phase.started_us.load(Ordering::Acquire);
    let cancel = Arc::clone(&request.cancel);
    let started_at = request.started_at;
    let owner = request.owner.clone();
    let mut result = scanned_result(&app, &target);
    result.requested = requested.clone();
    result.bookmark_relative_page_openable = Some(false);
    let mut direct = DirectMediaPrepared::image_fixture_for_test(target.join("prepared-only.png"));
    direct.metadata = Some((853, 1853));
    direct.rating_generation = 991;
    direct
        .tags
        .insert("prepared-only.png".into(), vec!["retained".into()]);
    result.direct_media = Some(direct);
    result.default_target = Some(DefaultStartupTarget::Folder {
        intent: StartupListIntent::ExplicitList,
        cursor: Some(("saved-cursor.png".into(), Some(37))),
    });
    result.scan_policy_mixed = true;
    app.startup_open_path_resolve_pending = Some(request);
    assert!(tx.send(result).is_ok());
    app.poll_startup_open_path_resolve_at(
        &egui::Context::default(),
        started_at + std::time::Duration::from_secs(60),
    );
    let retained = app.startup_open_path_resolve_pending.as_ref().unwrap();
    assert_eq!(retained.owner, owner);
    assert_eq!(retained.requested.as_path(), requested.as_path());
    assert!(retained.requested.has_epub_lease());
    assert_eq!(retained.navigation, navigation);
    assert_eq!(retained.started_at, started_at);
    assert!(retained.toast_shown);
    assert!(Arc::ptr_eq(&retained.cancel, &cancel));
    assert!(Arc::ptr_eq(&retained.scan_phase, &phase));
    assert_eq!(retained.diagnostic.as_ref().unwrap().correlation(), trace);
    let held = retained
        .held_resolve_for_activation_admission
        .as_ref()
        .unwrap();
    assert!(Arc::ptr_eq(&held.cancel, &held_cancel));
    assert!(Arc::ptr_eq(&held.scan_phase, &held_phase));
    assert!(!held_cancel.load(Ordering::Relaxed));
    let result = receive_scan(&app);
    assert_eq!(
        phase.started_us.load(Ordering::Acquire),
        phase_stamp,
        "rescan must not reset the scan deadline origin"
    );
    assert_eq!(result.requested, requested);
    assert_eq!(result.resolved.as_ref().unwrap().path, target);
    assert!(matches!(
        result.resolved.as_ref().unwrap().kind,
        crate::folder_tree::OpenablePathKind::Directory
    ));
    assert_eq!(result.elapsed_ms, 1853.25);
    assert_eq!(result.bookmark_relative_page_openable, Some(false));
    let direct = result.direct_media.unwrap();
    assert_eq!(
        direct.requested_path(),
        Some(target.join("prepared-only.png").as_path())
    );
    assert_eq!(direct.metadata, Some((853, 1853)));
    assert_eq!(direct.rating_generation, 991);
    assert_eq!(
        direct.tags.get("prepared-only.png").unwrap(),
        &["retained".to_string()]
    );
    assert!(matches!(result.default_target,
        Some(DefaultStartupTarget::Folder { intent: StartupListIntent::ExplicitList,
            cursor: Some((ref name, Some(37))) }) if name == "saved-cursor.png"));
    assert!(
        result.folder_scan.as_ref().unwrap().is_ok(),
        "the missing original EPUB must not be resolved again"
    );
    assert!(!result.scan_policy_mixed);
    app.cancel_unresolved_open_for_navigation(true);
    assert!(cancel.load(Ordering::Relaxed));
    assert!(held_cancel.load(Ordering::Relaxed));
}

#[test]
fn section1853_scan_continuation_cancel_discards_late_result() {
    let (mut app, target) = fixture();
    let mut result = scanned_result(&app, &target);
    result.scan_policy_mixed = true;
    let (request, tx) = pending(target);
    let cancel = Arc::clone(&request.cancel);
    app.startup_open_path_resolve_pending = Some(request);
    let original = app.current_folder.clone();
    assert!(tx.send(result).is_ok());
    let ctx = egui::Context::default();
    app.poll_startup_open_path_resolve(&ctx);
    // Keep the real scan reply ready, then retire through the production owner.
    let late = receive_scan(&app);
    ready_for_poll(&mut app, late);
    app.cancel_unresolved_open_for_navigation(true);
    app.poll_startup_open_path_resolve(&ctx);
    assert!(cancel.load(Ordering::Relaxed));
    assert!(app.startup_open_path_resolve_pending.is_none());
    assert_eq!(app.current_folder, original);
    assert!(app.startup_open_path.is_none());
}

#[test]
fn section1853_scan_continuation_disconnected_is_terminal_without_default_fallback() {
    let (mut app, target) = fixture();
    let mut result = scanned_result(&app, &target);
    result.scan_policy_mixed = true;
    let (request, tx) = pending(target);
    let cancel = Arc::clone(&request.cancel);
    app.startup_open_path_resolve_pending = Some(request);
    let original = app.current_folder.clone();
    assert!(tx.send(result).is_ok());
    let ctx = egui::Context::default();
    app.poll_startup_open_path_resolve(&ctx);
    // The scan runs normally; replace only its transport to model disconnection.
    let _completed = receive_scan(&app);
    let (disconnected_tx, disconnected_rx) = mpsc::channel();
    drop(disconnected_tx);
    app.startup_open_path_resolve_pending.as_mut().unwrap().rx = disconnected_rx;
    app.poll_startup_open_path_resolve(&ctx);
    assert!(cancel.load(Ordering::Relaxed));
    assert!(app.startup_open_path_resolve_pending.is_none());
    assert!(app.startup_open_path.is_none());
    assert_eq!(app.current_folder, original);
    assert!(
        app.feedback_toasts_for_test
            .iter()
            .any(|(text, _)| text.contains("場所を開く準備"))
    );
}
