//! New main-grid destinations supersede older opens in either acceptance order.
//! Channels hold real worker/preflight results at the adoption boundary; no desktop input.
use super::*;
use collection_navigation::{PhysicalHistoryPreflight, PhysicalHistoryPreflightPayload};
use std::time::{Duration, Instant};
use tests::phase_c_support::setup_app;
use top_level_grid_view::{TopLevelGridSurface, TopLevelSearchView};

#[derive(Clone, Copy, Debug)]
enum GridRoute {
    TagListOrder,
    PhysicalDfs,
}

fn write_zip(path: &Path) {
    let mut zip = zip::ZipWriter::new(std::fs::File::create(path).unwrap());
    zip.start_file("001.png", zip::write::SimpleFileOptions::default())
        .unwrap();
    std::io::Write::write_all(&mut zip, b"page").unwrap();
    zip.finish().unwrap();
}

fn fixture(
    app: &mut App,
    root: &Path,
    route: GridRoute,
    zip_target: bool,
) -> (PathBuf, PathBuf, PathBuf) {
    app.active_quick_folder_slot = None;
    app.settings.auto_fullscreen_zip_pdf = false;
    app.settings.auto_fullscreen_image_folders = false;
    app.settings.sort_order = crate::settings::SortOrder::FileName;
    let books = root.join("books");
    let a = books.join("A");
    let b = books.join(if zip_target { "B.zip" } else { "B" });
    std::fs::create_dir_all(&a).unwrap();
    image::RgbImage::new(2, 2).save(a.join("page.png")).unwrap();
    if zip_target {
        write_zip(&b);
    } else {
        std::fs::create_dir_all(&b).unwrap();
        image::RgbImage::new(2, 2).save(b.join("page.png")).unwrap();
    }
    // ListOrder must ignore X inside A. Physical DFS would correctly descend into it,
    // so its independent-open fixture keeps X outside the traversed books directory.
    let x = match route {
        GridRoute::TagListOrder => a.join("X.zip"),
        GridRoute::PhysicalDfs => root.join("X.zip"),
    };
    write_zip(&x);
    app.load_folder(a.clone());
    app.finish_page_identity_preparation_for_test();
    app.fullscreen_idx = None;
    if matches!(route, GridRoute::TagListOrder) {
        app.tag_view.active = true;
        app.tag_view.run = 19;
        app.tag_view.query = "#chosen".into();
        app.tag_view.last_executed = "#chosen".into();
        app.tag_view.nav_stack.clear();
        app.items_are_tag_view = true;
        app.items = vec![
            GridItem::Folder(a.clone()),
            if zip_target {
                GridItem::ZipFile(b.clone())
            } else {
                GridItem::Folder(b.clone())
            },
        ];
        app.visible_indices = vec![0, 1];
        app.top_level_grid_view
            .replace_surface(TopLevelGridSurface::Search(TopLevelSearchView::Tag));
        app.capture_main_folder_list_order(0);
        app.tag_view.nav_stack = vec![a.clone()];
        app.items_are_tag_view = false;
        app.items = vec![
            GridItem::Image(a.join("page.png")),
            GridItem::ZipFile(x.clone()),
        ];
        app.visible_indices = vec![0, 1];
        app.image_metas = vec![None; 2];
        app.thumbnails = vec![ThumbnailState::Pending; 2];
    }
    (a, b, x)
}

fn start_grid(app: &mut App, ctx: &egui::Context, a: &Path, route: GridRoute) {
    match route {
        GridRoute::TagListOrder => assert!(app.route_tag_view_grid_book_navigation(ctx, true)),
        GridRoute::PhysicalDfs => app.start_folder_nav(a.to_owned(), true, FolderNavMode::Grid),
    }
    assert!(
        app.folder_nav_pending.is_some(),
        "{route:?} was not accepted"
    );
}

fn receive_grid(app: &mut App, expected: &Path) -> FolderNavThreadResult {
    let result = app
        .folder_nav_pending
        .as_mut()
        .unwrap()
        .rx
        .recv_timeout(Duration::from_secs(15))
        .expect("real grid worker result");
    assert_eq!(result.target.as_ref().unwrap().logical_source(), expected);
    result
}

fn adopt_grid(app: &mut App, ctx: &egui::Context, result: FolderNavThreadResult) {
    let pending = app
        .folder_nav_pending
        .take()
        .expect("grid request still owned");
    app.apply_folder_nav_result(
        ctx,
        FolderNavResult {
            target: result.target,
            hit_image_folder: result.hit_image_folder,
            forward: pending.forward,
            mode: pending.mode,
            queued_steps: app.pending_folder_nav_steps,
        },
    );
}

fn hold_zip_preflight(
    app: &mut App,
    path: &Path,
) -> (
    mpsc::Sender<Result<PhysicalHistoryPreflightPayload, String>>,
    PhysicalHistoryPreflightPayload,
) {
    let Some(HistoryNavigationTransition::Physical(request)) =
        app.top_level_grid_view.history_navigation_transition_mut()
    else {
        panic!("accepted ZIP must retain its physical owner");
    };
    assert_eq!(request.path, path);
    let PhysicalHistoryPhase::Preflighting { preflight, .. } = &mut request.phase else {
        panic!("ZIP must await preflight adoption");
    };
    let (controlled, sender) = PhysicalHistoryPreflight::controlled_for_test();
    *preflight = controlled;
    let raw = crate::zip_loader::enumerate_image_entries_detailed(path).unwrap();
    let payload =
        PhysicalHistoryPreflightPayload::Zip(crate::zip_loader::PreparedZipEnumeration::prepare(
            path.to_owned(),
            raw,
            app.settings.file_type_visibility.clone(),
        ));
    (sender, payload)
}

fn ordinary_open(
    app: &mut App,
    path: &Path,
) -> (
    mpsc::Sender<Result<PhysicalHistoryPreflightPayload, String>>,
    PhysicalHistoryPreflightPayload,
) {
    assert!(matches!(
        app.load_folder_or_convert_archive_with_auto_fullscreen_owned(
            path.to_owned(),
            false,
            OpenRequestOwner::Navigation,
            StartupListIntent::ExplicitList,
        ),
        FolderOpenOutcome::Loaded | FolderOpenOutcome::Classifying
    ));
    app.settle_open_path_classification_for_test();
    hold_zip_preflight(app, path)
}

fn settle(app: &mut App, ctx: &egui::Context) {
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        app.settle_open_path_classification_for_test();
        app.poll_history_navigation_transition_for_test(ctx);
        app.poll_zip_enumerate();
        app.poll_pdf_enumerate();
        app.poll_sidecar_restore(ctx);
        app.poll_book_composition_service_for_test(ctx);
        app.poll_book_presentation_permission(ctx);
        if app.top_level_grid_view.open_path_classification().is_none()
            && app
                .top_level_grid_view
                .history_navigation_transition()
                .is_none()
            && app.zip_enumerate_pending.is_none()
            && app.pdf_enumerate_pending.is_none()
            && !app.sidecar_restore_active()
        {
            return;
        }
        assert!(Instant::now() < deadline, "open adoption did not settle");
        std::thread::yield_now();
    }
}

fn assert_destination(app: &App, route: GridRoute, a: &Path, destination: &Path, ordinary: bool) {
    assert_eq!(app.effective_folder().as_deref(), Some(destination));
    assert!(app.fullscreen_idx.is_none());
    assert!(app.folder_nav_pending.is_none());
    assert_eq!(app.pending_folder_nav_steps, 0);
    if matches!(route, GridRoute::TagListOrder) {
        let expected = if ordinary {
            vec![a.to_owned(), destination.to_owned()]
        } else {
            vec![destination.to_owned()]
        };
        assert_eq!(app.tag_view.nav_stack, expected);
    }
}

#[test]
fn tag_view_open_supersession_grid_acceptance_retires_older_ordinary_zip_preflight() {
    for route in [GridRoute::TagListOrder, GridRoute::PhysicalDfs] {
        let mut app = setup_app();
        let root = app.tmp.path().to_owned();
        let (a, b, x) = fixture(&mut app, &root, route, false);
        let ctx = egui::Context::default();
        let (old_sender, old_payload) = ordinary_open(&mut app, &x);
        assert_eq!(app.effective_folder(), Some(a.clone()));
        start_grid(&mut app, &ctx, &a, route);
        assert!(
            old_sender.send(Ok(old_payload)).is_err(),
            "old ZIP owner survived {route:?}"
        );
        assert!(
            app.top_level_grid_view
                .history_navigation_transition()
                .is_none()
        );
        let result = receive_grid(&mut app, &b);
        adopt_grid(&mut app, &ctx, result);
        settle(&mut app, &ctx);
        assert_destination(&app, route, &a, &b, false);
    }
}

#[test]
fn tag_view_open_supersession_ordinary_zip_retires_pending_and_completed_grid_workers() {
    for route in [GridRoute::TagListOrder, GridRoute::PhysicalDfs] {
        for completed in [false, true] {
            let mut app = setup_app();
            let root = app.tmp.path().to_owned();
            let (a, b, x) = fixture(&mut app, &root, route, false);
            let ctx = egui::Context::default();
            start_grid(&mut app, &ctx, &a, route);
            let result = receive_grid(&mut app, &b);
            let pending = app.folder_nav_pending.as_mut().unwrap();
            let cancel = pending.cancel.clone();
            let (sender, receiver) = mpsc::channel();
            pending.rx = receiver;
            let late_result = if completed {
                assert!(sender.send(result).is_ok());
                None
            } else {
                Some(result)
            };
            let (new_sender, new_payload) = ordinary_open(&mut app, &x);
            assert!(
                cancel.load(Ordering::Relaxed),
                "{route:?}, completed={completed}"
            );
            assert!(app.folder_nav_pending.is_none());
            if let Some(result) = late_result {
                assert!(
                    sender.send(result).is_err(),
                    "pending worker receiver survived"
                );
            }
            assert!(
                app.poll_folder_nav().is_none(),
                "completed old result must not adopt"
            );
            assert_eq!(app.effective_folder(), Some(a.clone()));
            assert!(new_sender.send(Ok(new_payload)).is_ok());
            settle(&mut app, &ctx);
            assert_destination(&app, route, &a, &x, true);
        }
    }
}

#[test]
fn tag_view_open_supersession_ordinary_zip_retires_grid_dfs_staged_zip_adoption() {
    for route in [GridRoute::TagListOrder, GridRoute::PhysicalDfs] {
        let mut app = setup_app();
        let root = app.tmp.path().to_owned();
        let (a, b, x) = fixture(&mut app, &root, route, true);
        let ctx = egui::Context::default();
        start_grid(&mut app, &ctx, &a, route);
        let result = receive_grid(&mut app, &b);
        adopt_grid(&mut app, &ctx, result);
        app.settle_open_path_classification_for_test();
        let (old_sender, old_payload) = hold_zip_preflight(&mut app, &b);
        let Some(HistoryNavigationTransition::Physical(request)) =
            app.top_level_grid_view.history_navigation_transition()
        else {
            unreachable!()
        };
        assert!(!request.dfs_continuation.as_ref().unwrap().fullscreen);
        let old_id = request.request_id;
        let (new_sender, new_payload) = ordinary_open(&mut app, &x);
        assert!(
            old_sender.send(Ok(old_payload)).is_err(),
            "old DFS preflight survived"
        );
        assert!(
            matches!(app.top_level_grid_view.history_navigation_transition(),
            Some(HistoryNavigationTransition::Physical(request)) if request.request_id != old_id
                && request.dfs_continuation.is_none())
        );
        assert!(new_sender.send(Ok(new_payload)).is_ok());
        settle(&mut app, &ctx);
        assert_destination(&app, route, &a, &x, true);
    }
}

#[test]
fn tag_view_open_supersession_automatic_refresh_claim_and_retirement_keep_grid_worker() {
    for route in [GridRoute::TagListOrder, GridRoute::PhysicalDfs] {
        let mut app = setup_app();
        let root = app.tmp.path().to_owned();
        let (a, b, _) = fixture(&mut app, &root, route, false);
        let ctx = egui::Context::default();
        start_grid(&mut app, &ctx, &a, route);
        let result = receive_grid(&mut app, &b);
        let pending = app.folder_nav_pending.as_mut().unwrap();
        let cancel = pending.cancel.clone();
        let (sender, receiver) = mpsc::channel();
        pending.rx = receiver;
        // Watcher refresh is the same visible target, not a new destination admission.
        assert!(app.claim_open_request_owner_with_startup_intent(
            &a,
            &OpenRequestOwner::Navigation,
            false,
            MainListRequestStage::Admission,
        ));
        app.retire_replaced_main_list_requests_with_startup_intent(None, false);
        assert!(!cancel.load(Ordering::Relaxed));
        assert!(Arc::ptr_eq(
            &app.folder_nav_pending.as_ref().unwrap().cancel,
            &cancel
        ));
        assert!(sender.send(result).is_ok());
        let result = receive_grid(&mut app, &b);
        adopt_grid(&mut app, &ctx, result);
        settle(&mut app, &ctx);
        assert_destination(&app, route, &a, &b, false);
    }
}

#[test]
fn tag_view_open_supersession_refused_ordinary_zip_keeps_grid_worker() {
    for route in [GridRoute::TagListOrder, GridRoute::PhysicalDfs] {
        let mut app = setup_app();
        let root = app.tmp.path().to_owned();
        let (a, b, x) = fixture(&mut app, &root, route, false);
        let ctx = egui::Context::default();
        start_grid(&mut app, &ctx, &a, route);
        let result = receive_grid(&mut app, &b);
        let pending = app.folder_nav_pending.as_mut().unwrap();
        let cancel = pending.cancel.clone();
        let (sender, receiver) = mpsc::channel();
        pending.rx = receiver;
        app.settings
            .file_type_visibility
            .excluded_categories
            .insert(crate::file_type_visibility::FileTypeCategory::Zip);
        let outcome = app.load_folder_or_convert_archive_with_auto_fullscreen_owned(
            x,
            false,
            OpenRequestOwner::Navigation,
            StartupListIntent::ExplicitList,
        );
        assert!(matches!(
            outcome,
            FolderOpenOutcome::Refused(FolderOpenRefusal::Admission(
                OpenAdmissionRefusal::FileTypeExcluded
            ))
        ));
        assert!(!cancel.load(Ordering::Relaxed));
        assert!(Arc::ptr_eq(
            &app.folder_nav_pending.as_ref().unwrap().cancel,
            &cancel
        ));
        assert!(sender.send(result).is_ok());
        let result = receive_grid(&mut app, &b);
        adopt_grid(&mut app, &ctx, result);
        settle(&mut app, &ctx);
        assert_destination(&app, route, &a, &b, false);
    }
}

#[test]
fn tag_view_open_supersession_ordinary_zip_retires_grid_hydration_tail_only() {
    // Ordinary physical Grid folder adoption has no DFS tail; Grid ListOrder does.
    for route in [GridRoute::TagListOrder] {
        let mut app = setup_app();
        let root = app.tmp.path().to_owned();
        let (a, b, _) = fixture(&mut app, &root, route, false);
        let x = b.join("X.zip");
        write_zip(&x);
        let ctx = egui::Context::default();
        start_grid(&mut app, &ctx, &a, route);
        let result = receive_grid(&mut app, &b);
        adopt_grid(&mut app, &ctx, result);
        assert_eq!(app.effective_folder(), Some(b.clone()));
        assert!(app.main_list_folder_navigation_adoption_pending());
        let hydration_cancel = app
            .sidecar_restore
            .as_ref()
            .expect("real folder adoption must hydrate before finishing DFS")
            .cancel_for_test();
        let generation = app.items_generation;

        let (sender, payload) = ordinary_open(&mut app, &x);
        assert!(
            !app.main_list_folder_navigation_adoption_pending(),
            "the accepted ordinary open must retire the old grid DFS continuation"
        );
        assert!(
            app.sidecar_restore_active(),
            "retiring navigation must preserve actual hydration"
        );
        assert!(Arc::ptr_eq(
            &app.sidecar_restore.as_ref().unwrap().cancel_for_test(),
            &hydration_cancel,
        ));
        assert!(!hydration_cancel.load(Ordering::Relaxed));
        assert_eq!(app.items_generation, generation);
        assert_eq!(app.effective_folder(), Some(b.clone()));

        assert!(sender.send(Ok(payload)).is_ok());
        // The new install may wait for the old live hydration. Poll both owners rather
        // than cancelling hydration to manufacture an immediately available install.
        settle(&mut app, &ctx);
        assert_destination(&app, route, &b, &x, true);
        assert!(!app.main_list_folder_navigation_adoption_pending());
    }
}

#[cfg(windows)]
#[test]
fn tag_view_open_supersession_grid_candidate_main_fallback_retires_grid_worker() {
    for route in [GridRoute::TagListOrder, GridRoute::PhysicalDfs] {
        let mut app = setup_app();
        let root = app.tmp.path().to_owned();
        let (a, b, _) = fixture(&mut app, &root, route, false);
        let ctx = egui::Context::default();
        start_grid(&mut app, &ctx, &a, route);
        let result = receive_grid(&mut app, &b);
        let pending = app.folder_nav_pending.as_mut().unwrap();
        let cancel = pending.cancel.clone();
        let (sender, receiver) = mpsc::channel();
        pending.rx = receiver;

        app.settings.auto_fullscreen_image_folders = true;
        app.settings.detached_viewer_open_images_in_window = true;
        let candidate = root.join("mixed-candidate");
        std::fs::create_dir_all(candidate.join("child-folder")).unwrap();
        image::RgbImage::new(2, 2)
            .save(candidate.join("page.png"))
            .unwrap();
        let scan = scan_directory(&candidate);
        assert!(!app.scanned_folder_is_image_book(&candidate, &scan));
        // No shell/window is registered. Production cancellation of this fake lease
        // returns before any window or viewport operation.
        let lease = DetachedSessionLease {
            window_id: u64::MAX,
        };
        assert!(app.locate_window_context(lease.window_id).is_none());
        assert_ne!(app.detached_viewer_window_id(), Some(lease.window_id));
        let resolved = app
            .resolve_main_folder_open_ready(
                &ctx,
                FolderPaneOpenReady {
                    path: candidate.clone(),
                    scan: Ok(scan),
                    purpose: FolderOpenScanPurpose::GridFolderCandidate {
                        collection_owner: None,
                        detached_lease: lease,
                    },
                    epub_restore: None,
                    navigation: None,
                },
            )
            .expect("mixed folder candidate must resolve to a main open");
        assert_eq!(resolved.path, candidate);
        assert!(resolved.collection_owner.is_none());
        assert!(matches!(
            resolved.restore_intent,
            StartupListIntent::ExplicitList
        ));
        assert!(!app.pending_auto_fs_open);
        assert!(
            cancel.load(Ordering::Relaxed),
            "main fallback must supersede {route:?}"
        );
        assert!(app.folder_nav_pending.is_none());
        assert!(
            sender.send(result).is_err(),
            "old grid result receiver survived main fallback"
        );
        assert!(app.poll_folder_nav().is_none());
        assert!(app.locate_window_context(lease.window_id).is_none());
        assert_eq!(app.effective_folder(), Some(a));
    }
}
