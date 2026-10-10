use super::*;
use crate::book_bookmarks::{BookContainerKind, NewBookBookmark, PageIdentity};

fn policy_excluding(extension: &str) -> crate::file_type_visibility::FileTypeVisibility {
    let mut policy = crate::file_type_visibility::FileTypeVisibility::default();
    policy.excluded_extensions.insert(extension.into());
    policy
}

fn settings_ok(app: &mut App, policy: crate::file_type_visibility::FileTypeVisibility) {
    app.set_visibility_preferences_for_test(policy);
    app.apply_preferences_dialog(&egui::Context::default());
    assert!(!app.show_preferences);
    assert!(app.pref_state.is_none());
}

fn settle_listing(app: &mut App) {
    let ctx = egui::Context::default();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
    loop {
        if let Some(ready) = app.poll_folder_pane_open(&ctx) {
            #[cfg(windows)]
            assert!(app.resolve_main_folder_open_ready(&ctx, ready).is_none());
            #[cfg(not(windows))]
            if let Ok(scan) = ready.scan {
                if let FolderOpenScanPurpose::CurrentViewOrderRefresh {
                    order,
                    reload_owner,
                } = ready.purpose
                {
                    app.apply_current_view_order_refresh(ready.path, scan, order, *reload_owner);
                }
            }
        }
        app.poll_open_path_classification(&ctx);
        app.poll_zip_enumerate();
        app.poll_sidecar_restore(&ctx);
        app.poll_collection_history_transition(&ctx);
        if app.folder_pane_open_pending.is_none()
            && app.zip_enumerate_pending.is_none()
            && app.top_level_grid_view.open_path_classification().is_none()
            && app
                .top_level_grid_view
                .history_navigation_transition()
                .is_none()
            && !app.sidecar_restore_active()
        {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "listing did not settle: pane={} zip={} classification={} history={} sidecar={} fullscreen={:?}",
            app.folder_pane_open_pending.is_some(),
            app.zip_enumerate_pending.is_some(),
            app.top_level_grid_view.open_path_classification().is_some(),
            app.top_level_grid_view
                .history_navigation_transition()
                .is_some(),
            app.sidecar_restore_active(),
            app.fullscreen_idx,
        );
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
}

#[test]
fn visibility_settings_ok_snapshot_reader_close_and_reopen_reload_origin() {
    for detached in [false, true] {
        let mut app = setup_app_for_test();
        let folder = app.tmp.path().join("fixed-images");
        std::fs::create_dir(&folder).unwrap();
        for name in ["a.png", "b.jpg"] {
            image::RgbImage::new(1, 1).save(folder.join(name)).unwrap();
        }
        app.settings.auto_fullscreen_image_folders = true;
        app.load_folder(folder.clone());
        app.finish_page_identity_preparation_for_test();
        app.activate_snapshot(crate::snapshot::SnapshotSourceLabel::Mixed);
        app.finish_page_identity_preparation_for_test();
        app.open_fullscreen(0, HistoryTrigger::UserChosen);
        assert!(app.fullscreen_idx.is_some());
        #[cfg(windows)]
        if detached {
            app.viewer_presentation = ViewerPresentation::DetachedWindow;
            app.set_detached_window_binding_for_test(Some(1345));
            app.begin_active_detached_session(1345, DetachedSource::Image);
        }
        let accepted = app.items.clone();
        settings_ok(&mut app, policy_excluding("png"));
        app.poll_pending_reload(&egui::Context::default());
        assert_eq!(
            app.items, accepted,
            "accepted reader order remains unchanged"
        );
        assert!(app.is_snapshot_active());
        assert!(app.pending_reload);
        #[cfg(windows)]
        if detached {
            app.begin_active_detached_session_close("visibility-test");
            app.finish_active_detached_session_close("visibility-test");
        }
        app.close_fullscreen();
        app.poll_pending_reload(&egui::Context::default());
        settle_listing(&mut app);
        assert!(!app.is_snapshot_active());
        assert!(!app.pending_reload);
        assert_eq!(app.current_folder.as_ref(), Some(&folder));
        assert_eq!(
            app.items
                .iter()
                .map(|row| row.name().into_owned())
                .collect::<Vec<_>>(),
            ["b.jpg"]
        );
        app.deactivate_snapshot();
        assert_eq!(
            app.items.len(),
            1,
            "old saved_items cannot return on unlock"
        );
        app.open_fullscreen(0, HistoryTrigger::UserChosen);
        assert!(app.fullscreen_idx.is_some());
        assert_eq!(app.page_identity_index.policy, policy_excluding("png"));
        assert_eq!(app.items[0].name(), "b.jpg");
        app.close_fullscreen();
    }
}

fn add_bookmarks(
    app: &mut App,
    container: &Path,
    kind: BookContainerKind,
    identities: &[PageIdentity],
) {
    app.book_bookmark_service = crate::book_bookmarks::BookBookmarkService::spawn();
    let service = app.book_bookmark_service.as_ref().unwrap();
    for (index, identity) in identities.iter().enumerate() {
        service.add(
            13450 + index as u64,
            NewBookBookmark {
                container_path: container.to_path_buf(),
                container_kind: kind,
                page_identity: identity.clone(),
                page_index_hint: index,
            },
        );
    }
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let mut remaining = identities.len();
    while remaining > 0 {
        if let Ok(crate::book_bookmarks::BookBookmarkEvent::Added { result, .. }) =
            service.try_recv()
        {
            result.unwrap();
            remaining -= 1;
        }
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
}

fn settle_bookmarks(app: &mut App, index: usize) {
    app.ensure_current_book_bookmarks_loaded(index);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while app.current_book_bookmarks_request.is_some() {
        app.poll_book_bookmarks();
        assert!(
            std::time::Instant::now() < deadline,
            "bookmark projection did not settle"
        );
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
}

#[test]
fn visibility_bookmark_cache_reopen_uses_accepted_policy_and_restores_rows() {
    let mut app = setup_app_for_test();
    let folder = app.tmp.path().join("bookmark-policy");
    std::fs::create_dir(&folder).unwrap();
    for name in ["a.jpg", "b.png"] {
        image::RgbImage::new(1, 1).save(folder.join(name)).unwrap();
    }
    app.settings.auto_fullscreen_zip_pdf = true;
    app.settings.auto_fullscreen_image_folders = true;
    add_bookmarks(
        &mut app,
        &folder,
        BookContainerKind::ImageFolder,
        &[
            PageIdentity::RelativePath("a.jpg".into()),
            PageIdentity::RelativePath("b.png".into()),
        ],
    );
    app.load_folder(folder.clone());
    app.finish_page_identity_preparation_for_test();
    app.open_fullscreen(0, HistoryTrigger::UserChosen);
    assert!(app.fullscreen_idx.is_some());
    settle_bookmarks(&mut app, 0);
    assert_eq!(app.current_book_bookmarks.len(), 2);
    settings_ok(&mut app, policy_excluding("png"));
    settle_bookmarks(&mut app, 0);
    assert_eq!(
        app.current_book_bookmarks.len(),
        2,
        "live reader uses accepted policy"
    );
    app.close_fullscreen();
    assert!(app.fullscreen_idx.is_none());
    app.poll_pending_reload(&egui::Context::default());
    settle_listing(&mut app);
    app.open_fullscreen(0, HistoryTrigger::UserChosen);
    assert!(app.fullscreen_idx.is_some());
    settle_bookmarks(&mut app, 0);
    assert_eq!(app.current_book_bookmarks.len(), 1);
    assert_eq!(
        app.current_book_bookmarks_key.as_ref().unwrap().visibility,
        policy_excluding("png")
    );
    app.close_fullscreen();
    settings_ok(&mut app, Default::default());
    settle_listing(&mut app);
    app.open_fullscreen(0, HistoryTrigger::UserChosen);
    settle_bookmarks(&mut app, 0);
    assert_eq!(app.current_book_bookmarks.len(), 2);
    assert_eq!(
        crate::book_bookmarks::load_for_container_from_disk_readonly(&folder)
            .unwrap()
            .len(),
        2
    );
    app.close_fullscreen();
}

#[test]
fn visibility_in_book_pc_projects_nested_archive_provenance_and_legacy_notice() {
    let mut app = setup_app_for_test();
    let (source, cached) = crate::archive_visibility::legacy_conversion_fixture(app.tmp.path());
    let identities = [
        PageIdentity::ArchiveEntry("root.jpg".into()),
        PageIdentity::ArchiveEntry("chapter.zip/page.jpg".into()),
        PageIdentity::ArchiveEntry("notes.zip/page.jpg".into()),
        PageIdentity::ArchiveEntry("missing.jpg".into()),
    ];
    add_bookmarks(
        &mut app,
        &source,
        BookContainerKind::OtherArchive,
        &identities,
    );
    app.current_folder = Some(cached.clone());
    app.archive_source_override = Some(source.clone());
    app.top_level_grid_view
        .replace_surface(crate::app::top_level_grid_view::TopLevelGridSurface::Folder);
    app.items = vec![GridItem::ZipImage {
        zip_path: cached.clone(),
        entry_name: "root.jpg".into(),
    }];
    app.settings.file_type_visibility = policy_excluding("zip");
    settle_bookmarks(&mut app, 0);
    assert_eq!(
        app.current_book_bookmarks.len(),
        1,
        "missing row keeps existing removal route"
    );
    assert!(
        app.fs_feedback_toast
            .as_ref()
            .is_some_and(|toast| toast.0.contains("古い変換キャッシュ"))
    );
    crate::archive_visibility::regenerate_conversion_fixture(&source, &cached);
    // Open the original visible book through production admission/adoption after regeneration;
    // the cache's ZIP suffix is not an externally selectable book while ZIP is excluded.
    let previous_generation = app.items_generation;
    app.load_folder_or_convert_archive_with_auto_fullscreen_owned(
        source.clone(),
        false,
        OpenRequestOwner::Navigation,
        StartupListIntent::PreservePresentation,
    );
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
    loop {
        app.poll_collection_history_transition(&egui::Context::default());
        app.poll_open_path_classification(&egui::Context::default());
        app.poll_zip_enumerate();
        app.poll_sidecar_restore(&egui::Context::default());
        if app
            .top_level_grid_view
            .history_navigation_transition()
            .is_none()
            && app.top_level_grid_view.open_path_classification().is_none()
            && app.zip_enumerate_pending.is_none()
            && !app.sidecar_restore_active()
        {
            break;
        }
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    assert_ne!(app.items_generation, previous_generation);
    let root_page = app
        .items
        .iter()
        .position(|item| matches!(item, GridItem::ZipImage { entry_name, .. } if entry_name == "root.jpg"))
        .expect("the original book adopts its root image alongside virtual directories");
    settle_bookmarks(&mut app, root_page);
    assert_eq!(app.current_book_bookmarks.len(), 3);
    assert!(
        !app.current_book_bookmarks
            .iter()
            .any(|row| row.page_identity == identities[1])
    );
    assert!(
        app.current_book_bookmarks
            .iter()
            .any(|row| row.page_identity == identities[2])
    );
    assert_eq!(
        crate::book_bookmarks::load_for_container_from_disk_readonly(&source)
            .unwrap()
            .len(),
        4
    );
}

fn held_projection(
    app: &mut App,
    container: &Path,
) -> (
    mpsc::Sender<crate::bookmark_browser::BookBookmarkProjection>,
    Arc<std::sync::atomic::AtomicBool>,
) {
    let (tx, rx) = mpsc::channel();
    let cancel = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let key = BookBookmarkProjectionKey {
        container_key: crate::book_bookmarks::container_key(container),
        visibility: app.listing_file_type_visibility().clone(),
        context_id: app.virtual_list_context_id(),
        items_generation: app.items_generation,
    };
    app.current_book_bookmarks_key = None;
    app.current_book_bookmarks_request = Some(CurrentBookBookmarksRequest {
        key,
        cancel: Arc::clone(&cancel),
        stage: CurrentBookBookmarksStage::Projecting(rx),
    });
    (tx, cancel)
}

#[cfg(windows)]
#[test]
fn visibility_bookmark_context_bundles_keep_concurrent_policies_completions_and_close_scoped() {
    let mut app = setup_app_for_test();
    let folder = app.tmp.path().join("shared-policy-book");
    std::fs::create_dir(&folder).unwrap();
    for name in ["page.jpg", "page.png"] {
        image::RgbImage::new(1, 1).save(folder.join(name)).unwrap();
    }
    app.settings.auto_fullscreen_image_folders = true;
    app.settings.auto_fullscreen_zip_pdf = true;
    add_bookmarks(
        &mut app,
        &folder,
        BookContainerKind::ImageFolder,
        &[
            PageIdentity::RelativePath("page.jpg".into()),
            PageIdentity::RelativePath("page.png".into()),
        ],
    );
    let rows = crate::book_bookmarks::load_for_container_from_disk_readonly(&folder).unwrap();
    app.load_folder(folder.clone());
    app.finish_page_identity_preparation_for_test();
    app.fullscreen_idx = Some(0);
    let main_context = app.virtual_list_context_id();
    let main_generation = app.items_generation;
    let main_map = Arc::clone(&app.page_identity_index);
    let (main_tx, main_cancel) = held_projection(&mut app, &folder);
    let main_owner = app
        .current_book_bookmarks_request
        .as_ref()
        .unwrap()
        .key
        .clone();
    app.settings.file_type_visibility = policy_excluding("png");
    let mut detached_held = None;
    let detached = app.build_window_context_for_test(13450, |viewer| {
        viewer.load_folder(folder.clone());
        viewer.finish_page_identity_preparation_for_test();
        viewer.fullscreen_idx = Some(0);
        detached_held = Some(held_projection(viewer, &folder));
    });
    let (detached_tx, detached_cancel) = detached_held.unwrap();
    app.settings.file_type_visibility = Default::default();
    assert_eq!(main_owner.context_id, main_context);
    assert_eq!(
        app.current_book_bookmarks_request.as_ref().unwrap().key,
        main_owner
    );
    assert_eq!(main_owner.visibility, Default::default());
    assert!(!main_cancel.load(Ordering::Relaxed));
    app.ensure_current_book_bookmarks_loaded(0);
    let detached_owner = app
        .with_viewer_context(detached, |viewer| {
            viewer.ensure_current_book_bookmarks_loaded(0);
            let owner = viewer
                .current_book_bookmarks_request
                .as_ref()
                .unwrap()
                .key
                .clone();
            assert_eq!(owner.context_id, detached);
            assert_eq!(owner.visibility, policy_excluding("png"));
            assert!(viewer.current_book_bookmarks_key.is_none());
            owner
        })
        .unwrap();
    assert!(!main_cancel.load(Ordering::Relaxed));
    assert!(!detached_cancel.load(Ordering::Relaxed));
    assert_ne!(main_owner.context_id, detached_owner.context_id);
    assert_ne!(main_owner.visibility, detached_owner.visibility);
    let projected = crate::bookmark_browser::project_book_bookmarks(
        rows.clone(),
        &policy_excluding("png"),
        &AtomicBool::new(false),
    );
    detached_tx.send(projected).unwrap();
    app.with_viewer_context(detached, |viewer| {
        viewer.poll_book_bookmarks();
        assert!(viewer.current_book_bookmarks_request.is_none());
        assert_eq!(viewer.current_book_bookmarks.len(), 1);
        assert_eq!(
            viewer.current_book_bookmarks_key.as_ref(),
            Some(&detached_owner)
        );
    })
    .unwrap();
    assert_eq!(
        app.current_book_bookmarks_request.as_ref().unwrap().key,
        main_owner
    );
    assert!(!main_cancel.load(Ordering::Relaxed));
    main_tx
        .send(crate::bookmark_browser::BookBookmarkProjection {
            bookmarks: rows.clone(),
            notice: None,
        })
        .unwrap();
    app.poll_book_bookmarks();
    assert!(app.current_book_bookmarks_request.is_none());
    assert_eq!(app.current_book_bookmarks, rows);
    for _ in 0..3 {
        app.with_viewer_context(detached, |viewer| {
            viewer.ensure_current_book_bookmarks_loaded(0);
            assert!(viewer.current_book_bookmarks_request.is_none());
            assert_eq!(viewer.current_book_bookmarks.len(), 1);
            assert_eq!(
                viewer.current_book_bookmarks_key.as_ref(),
                Some(&detached_owner)
            );
        })
        .unwrap();
        app.ensure_current_book_bookmarks_loaded(0);
        assert!(app.current_book_bookmarks_request.is_none());
        assert_eq!(app.current_book_bookmarks_key.as_ref(), Some(&main_owner));
        assert_eq!(app.current_book_bookmarks, rows);
    }
    assert_eq!(app.items_generation, main_generation);
    assert!(Arc::ptr_eq(&main_map, &app.page_identity_index));
    let (main_tx, main_cancel) = held_projection(&mut app, &folder);
    let (detached_tx, detached_cancel) = app
        .with_viewer_context(detached, |viewer| held_projection(viewer, &folder))
        .unwrap();
    app.retire_context(detached, "visibility_bookmark_closed_owner", |_| ())
        .unwrap();
    assert!(detached_cancel.load(Ordering::Relaxed));
    assert!(!main_cancel.load(Ordering::Relaxed));
    assert!(
        detached_tx
            .send(crate::bookmark_browser::BookBookmarkProjection {
                bookmarks: rows.clone(),
                notice: None
            })
            .is_err()
    );
    main_tx
        .send(crate::bookmark_browser::BookBookmarkProjection {
            bookmarks: rows.clone(),
            notice: None,
        })
        .unwrap();
    app.poll_book_bookmarks();
    assert_eq!(app.current_book_bookmarks, rows);
    assert!(app.current_book_bookmarks_request.is_none());
    assert!(app.with_viewer_context(detached, |_| ()).is_err());
}

#[cfg(windows)]
#[test]
fn visibility_bookmark_actor_routes_same_book_results_to_different_context_policies() {
    let mut app = setup_app_for_test();
    let folder = app.tmp.path().join("actor-policy-book");
    std::fs::create_dir(&folder).unwrap();
    for name in ["page.jpg", "page.png"] {
        image::RgbImage::new(1, 1).save(folder.join(name)).unwrap();
    }
    app.settings.auto_fullscreen_image_folders = true;
    app.settings.auto_fullscreen_zip_pdf = true;
    add_bookmarks(
        &mut app,
        &folder,
        BookContainerKind::ImageFolder,
        &[
            PageIdentity::RelativePath("page.jpg".into()),
            PageIdentity::RelativePath("page.png".into()),
        ],
    );
    app.load_folder(folder.clone());
    app.finish_page_identity_preparation_for_test();
    app.fullscreen_idx = Some(0);
    app.ensure_current_book_bookmarks_loaded(0);
    let main_request = match app.current_book_bookmarks_request.as_ref().unwrap().stage {
        CurrentBookBookmarksStage::ReadingRows(id) => id,
        _ => panic!("real DB read expected"),
    };
    app.settings.file_type_visibility = policy_excluding("png");
    let detached = app.build_window_context_for_test(13451, |viewer| {
        viewer.load_folder(folder.clone());
        viewer.finish_page_identity_preparation_for_test();
        viewer.fullscreen_idx = Some(0);
        viewer.ensure_current_book_bookmarks_loaded(0);
    });
    app.settings.file_type_visibility = Default::default();
    let detached_request = app
        .with_viewer_context(detached, |viewer| {
            match viewer
                .current_book_bookmarks_request
                .as_ref()
                .unwrap()
                .stage
            {
                CurrentBookBookmarksStage::ReadingRows(id) => id,
                _ => panic!("real sibling DB read expected"),
            }
        })
        .unwrap();
    assert_ne!(main_request, detached_request);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        // The DB actor is global: whichever context polls it must deliver rows to the
        // request's owning bundle, including a currently deposited sibling.
        app.poll_book_bookmarks();
        let sibling_ready = app
            .with_viewer_context_ref(detached, |viewer| viewer.book_bookmarks_request().is_none())
            .unwrap();
        if app.current_book_bookmarks_request.is_none() && sibling_ready {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "context DB projection did not settle"
        );
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    assert_eq!(app.current_book_bookmarks.len(), 2);
    assert_eq!(
        app.current_book_bookmarks_key.as_ref().unwrap().visibility,
        Default::default()
    );
    app.with_viewer_context(detached, |viewer| {
        assert_eq!(viewer.current_book_bookmarks.len(), 1);
        assert_eq!(
            viewer
                .current_book_bookmarks_key
                .as_ref()
                .unwrap()
                .visibility,
            policy_excluding("png")
        );
        assert_eq!(
            viewer.current_book_bookmarks[0].page_identity,
            PageIdentity::RelativePath("page.jpg".into())
        );
    })
    .unwrap();
    assert_eq!(
        crate::book_bookmarks::load_for_container_from_disk_readonly(&folder)
            .unwrap()
            .len(),
        2
    );
}

#[cfg(windows)]
#[test]
fn visibility_bookmark_title_context_clear_preserves_sibling_viewport_claim_and_draft() {
    let mut app = setup_app_for_test();
    let id = 7;
    let viewport = egui::ViewportId::from_hash_of("bookmark-title-sibling");
    app.book_bookmark_title_edit = Some(BookBookmarkTitleEdit {
        id,
        focus_viewport: egui::ViewportId::ROOT,
        title: "main draft".into(),
        request_focus: false,
    });
    let detached = app.build_window_context_for_test(13452, |viewer| {
        viewer.book_bookmark_title_edit = Some(BookBookmarkTitleEdit {
            id,
            focus_viewport: viewport,
            title: "sibling draft".into(),
            request_focus: false,
        });
    });
    let widget = crate::ui_adjustment_panel::book_bookmark_title_edit_widget_id(id);
    app.claim_pending_text_input_focus(viewport, widget, 1);
    let sibling_claim = app.pending_text_input_focus.get();
    app.clear_book_bookmark_title_edit();
    assert!(app.book_bookmark_title_edit.is_none());
    assert_eq!(app.pending_text_input_focus.get(), sibling_claim);
    app.with_viewer_context(detached, |viewer| {
        let edit = viewer.book_bookmark_title_edit.as_ref().unwrap();
        assert_eq!(edit.id, id);
        assert_eq!(edit.title, "sibling draft");
        assert_eq!(edit.focus_viewport, viewport);
        assert_eq!(viewer.pending_text_input_focus.get(), sibling_claim);
        viewer.clear_book_bookmark_title_edit();
        assert!(viewer.pending_text_input_focus.get().is_none());
    })
    .unwrap();
    assert!(app.book_bookmark_title_edit.is_none());
}

#[cfg(windows)]
#[test]
fn visibility_collection_and_rating_converted_refresh_keep_canonical_source_and_owner() {
    use crate::app::top_level_grid_view::{CollectionGridPhysicalLoadIntent, TopLevelGridSurface};
    for collection in [false, true] {
        let mut app = setup_app_for_test();
        // A real convertible source exercises staged cache adoption. Native CBZ open may
        // instead enumerate its original ZIP, so it cannot prove a converted-source reload.
        let source = app.tmp.path().join("converted-refresh.7z");
        let mut archive = sevenz_rust2::ArchiveWriter::create(&source).unwrap();
        archive
            .push_archive_entry(
                sevenz_rust2::ArchiveEntry::new_file("root.jpg"),
                Some(std::io::Cursor::new(b"page".to_vec())),
            )
            .unwrap();
        archive.finish().unwrap();
        let db = app.archive_cache_db.as_ref().unwrap();
        let cached = db.reserve_cache_zip_path(&source).unwrap();
        crate::archive_converter::convert_to_zip(
            &source,
            &cached,
            crate::archive_converter::ArchiveFormat::SevenZ,
            &AtomicBool::new(false),
            None,
        )
        .unwrap();
        let source_meta = std::fs::metadata(&source).unwrap();
        db.record(
            &source,
            crate::ui_helpers::mtime_secs(&source_meta),
            source_meta.len() as i64,
            crate::archive_converter::ArchiveFormat::SevenZ,
            &cached,
            std::fs::metadata(&cached).unwrap().len() as i64,
            1,
            false,
        )
        .unwrap();
        app.active_quick_folder_slot = None;
        app.settings.auto_fullscreen_zip_pdf = false;
        app.settings.file_type_visibility = policy_excluding("zip");
        assert!(
            app.settings
                .file_type_visibility
                .allows_physical_file(&source)
        );
        assert!(
            !app.settings
                .file_type_visibility
                .allows_physical_file(&cached)
        );
        let owner = if collection {
            install_collection_item_for_detached_plan(
                &mut app,
                &source,
                crate::collection_store::CollectionResolvedKind::ConvertibleArchive,
            );
            OpenRequestOwner::CollectionGridPhysical(
                app.collection_grid_physical_load_owner(0, &source).unwrap(),
            )
        } else {
            app.current_folder = Some(rating_view_synthetic_path());
            app.items_are_rating_view = true;
            app.rating_view_stars = 4;
            app.rating_view_saved_folder = Some(app.tmp.path().to_path_buf());
            app.top_level_grid_view
                .replace_surface(TopLevelGridSurface::Rating { stars: 4 });
            OpenRequestOwner::RatingPhysical(app.rating_view_physical_load_owner(&source).unwrap())
        };
        app.load_folder_or_convert_archive_with_auto_fullscreen_owned(
            source.clone(),
            false,
            owner,
            StartupListIntent::ExplicitList,
        );
        settle_listing(&mut app);
        assert_eq!(app.current_folder.as_ref(), Some(&cached));
        assert_eq!(app.archive_source_override.as_ref(), Some(&source));
        assert!(!app.items.is_empty());
        let location = app.folder_nav_current_target();
        let history = app.folder_nav_history_snapshot();
        let rating_chain = app.rating_view_nav_stack.clone();
        // F5 reads the already adopted backing. Its logical source still owns visibility
        // and return/history identity, even when conversion has since been disabled.
        app.settings.archive_file_handling = crate::settings::ArchiveFileHandling::Ignore;
        std::fs::remove_file(&source).unwrap();
        assert!(!source.exists());
        match app.current_folder_reload_owner(&cached) {
            OpenRequestOwner::CollectionGridPhysical(owner) if collection => {
                assert_eq!(owner.target_path, cached);
                assert_eq!(owner.intent, CollectionGridPhysicalLoadIntent::Refresh);
                assert!(app.collection_grid_physical_load_owner_is_current(&owner, &cached));
            }
            OpenRequestOwner::RatingPhysical(owner) if !collection => {
                assert_eq!(owner.target_path, cached);
                assert_eq!(owner.intent, RatingPhysicalLoadIntent::Refresh);
                assert!(app.rating_physical_load_owner_is_current(&owner, &cached));
            }
            _ => panic!("physical parent must own its refresh"),
        }
        let previous_generation = app.items_generation;
        app.reload_current_folder_preserving_override();
        assert!(app.top_level_grid_view.open_path_classification().is_none());
        let Some(HistoryNavigationTransition::Physical(refresh)) =
            app.top_level_grid_view.history_navigation_transition()
        else {
            panic!("F5 must prepare the adopted backing through its physical history owner");
        };
        assert_eq!(refresh.path, cached);
        assert!(matches!(
            refresh.phase,
            PhysicalHistoryPhase::Preflighting { .. }
        ));
        settle_listing(&mut app);
        assert_ne!(
            app.items_generation, previous_generation,
            "the accepted backing must be reread despite Ignore and a missing original"
        );
        assert_eq!(app.current_folder.as_ref(), Some(&cached));
        assert_eq!(app.archive_source_override.as_ref(), Some(&source));
        assert_eq!(app.folder_nav_current_target(), location);
        assert_eq!(
            app.folder_nav_history_snapshot().back_stack,
            history.back_stack
        );
        assert_eq!(
            app.folder_nav_history_snapshot().forward_stack,
            history.forward_stack
        );
        assert_eq!(app.rating_view_nav_stack, rating_chain);
        assert_eq!(app.page_identity_index.policy, policy_excluding("zip"));
    }
}

#[cfg(windows)]
#[test]
fn visibility_shared_archive_admission_uses_only_the_mounted_matching_source_pair() {
    fn assert_excluded(app: &mut App, target: &Path) {
        let generation = app.items_generation;
        let folder = app.current_folder.clone();
        let original = app.archive_source_override.clone();
        let outcome = app.load_folder_or_convert_archive_with_auto_fullscreen_classified(
            target.to_path_buf(),
            false,
            OpenRequestOwner::Navigation,
            Some(OpenPathKind::File),
            None,
            StartupListIntent::PreservePresentation,
        );
        assert!(matches!(
            outcome,
            FolderOpenOutcome::Refused(FolderOpenRefusal::Admission(
                OpenAdmissionRefusal::FileTypeExcluded
            ))
        ));
        assert_eq!(app.items_generation, generation);
        assert_eq!(app.current_folder, folder);
        assert_eq!(app.archive_source_override, original);
        assert!(app.top_level_grid_view.open_path_classification().is_none());
        assert!(
            app.top_level_grid_view
                .history_navigation_transition()
                .is_none()
        );
    }

    let mut app = setup_app_for_test();
    // Admission resolves an already owned pair without filesystem or reverse-cache lookup.
    // The real conversion, cache adoption, and missing-source F5 route are covered above.
    let source = app.tmp.path().join("owned-original.7z");
    let cached = app.tmp.path().join("owned-cache.zip");
    let unrelated_cache = app.tmp.path().join("unrelated-cache.zip");
    let sibling_folder = app.tmp.path().join("sibling-folder");
    std::fs::create_dir(&sibling_folder).unwrap();
    app.current_folder = Some(cached.clone());
    app.archive_source_override = Some(source.clone());
    app.top_level_grid_view
        .replace_surface(crate::app::top_level_grid_view::TopLevelGridSurface::Folder);
    app.settings.file_type_visibility = policy_excluding("zip");
    assert_eq!(app.physical_open_visibility_source(&cached), source);
    assert_eq!(
        app.physical_open_visibility_source(&unrelated_cache),
        unrelated_cache
    );
    assert_excluded(&mut app, &unrelated_cache);

    let sibling = app.build_window_context_for_test(13455, |viewer| {
        viewer.load_folder(sibling_folder.clone());
        viewer.finish_page_identity_preparation_for_test();
    });
    app.with_viewer_context(sibling, |viewer| {
        assert_eq!(viewer.physical_open_visibility_source(&cached), cached);
        assert_excluded(viewer, &cached);
        assert_eq!(viewer.current_folder.as_ref(), Some(&sibling_folder));
        assert!(viewer.archive_source_override.is_none());
    })
    .unwrap();
    assert_eq!(app.physical_open_visibility_source(&cached), source);
    app.settings.file_type_visibility = policy_excluding("7z");
    assert!(
        app.settings
            .file_type_visibility
            .allows_physical_file(&cached)
    );
    assert!(
        !app.settings
            .file_type_visibility
            .allows_physical_file(&source)
    );
    assert_excluded(&mut app, &cached);
}

#[test]
fn visibility_bookmark_projection_rejects_same_context_late_container_completion() {
    let mut app = setup_app_for_test();
    let first = app.tmp.path().join("first");
    let second = app.tmp.path().join("second");
    for folder in [&first, &second] {
        std::fs::create_dir(folder).unwrap();
    }
    app.load_folder(first.clone());
    app.finish_page_identity_preparation_for_test();
    let context = app.virtual_list_context_id();
    let (tx, cancel) = held_projection(&mut app, &first);
    app.load_folder(second);
    app.finish_page_identity_preparation_for_test();
    assert_eq!(app.virtual_list_context_id(), context);
    tx.send(crate::bookmark_browser::BookBookmarkProjection {
        bookmarks: Vec::new(),
        notice: Some(crate::archive_visibility::ArchiveVisibilityNotice::LegacyConvertedCache),
    })
    .unwrap();
    app.poll_book_bookmarks();
    assert!(app.current_book_bookmarks_request.is_none());
    assert!(cancel.load(std::sync::atomic::Ordering::Relaxed));
    assert!(
        !app.fs_feedback_toast
            .as_ref()
            .is_some_and(|toast| toast.0.contains("古い変換キャッシュ"))
    );
}

#[test]
fn visibility_bookmark_mutation_cancels_only_own_projection_and_preserves_media_siblings() {
    for title_edit in [false, true] {
        let mut app = setup_app_for_test();
        let container = app.tmp.path().join("book");
        let other = app.tmp.path().join("other");
        std::fs::create_dir(&container).unwrap();
        image::RgbImage::new(1, 1)
            .save(container.join("page.jpg"))
            .unwrap();
        add_bookmarks(
            &mut app,
            &container,
            BookContainerKind::ImageFolder,
            &[PageIdentity::RelativePath("page.jpg".into())],
        );
        let row = crate::book_bookmarks::load_for_container_from_disk_readonly(&container)
            .unwrap()
            .remove(0);
        app.current_book_bookmarks = vec![row.clone()];
        let (tx, cancel) = held_projection(&mut app, &container);
        app.notify_bookmarks_changed(); // Timed video/audio bookmark writers use this owner.
        app.invalidate_current_book_bookmarks_for_container(&other); // Remote other-book write.
        assert!(!cancel.load(std::sync::atomic::Ordering::Relaxed));
        assert_eq!(app.current_book_bookmarks, vec![row.clone()]);
        if title_edit {
            app.set_book_bookmark_title(row.id, "changed".into());
        } else {
            app.remove_book_bookmark(row.id);
        }
        assert!(cancel.load(std::sync::atomic::Ordering::Relaxed));
        assert!(app.current_book_bookmarks_request.is_none());
        assert!(
            tx.send(crate::bookmark_browser::BookBookmarkProjection {
                bookmarks: vec![row],
                notice: None,
            })
            .is_err(),
            "late snapshot cannot resurrect a removed row or old title"
        );
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while !app.book_bookmark_pending_requests.is_empty() {
            app.poll_book_bookmarks();
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        assert!(app.current_book_bookmarks.is_empty());
        let rows =
            crate::book_bookmarks::load_for_container_from_disk_readonly(&container).unwrap();
        if title_edit {
            assert_eq!(rows[0].title.as_deref(), Some("changed"));
        } else {
            assert!(rows.is_empty());
        }
    }
}

#[cfg(windows)]
#[test]
fn visibility_bookmark_mutation_invalidates_matching_bundles_and_preserves_unrelated_owner() {
    for title_edit in [false, true] {
        let mut app = setup_app_for_test();
        let folder = app.tmp.path().join("shared-mutation-book");
        let other = app.tmp.path().join("unrelated-mutation-book");
        for path in [&folder, &other] {
            std::fs::create_dir(path).unwrap();
            for name in ["page.jpg", "page.png"] {
                image::RgbImage::new(1, 1).save(path.join(name)).unwrap();
            }
        }
        add_bookmarks(
            &mut app,
            &folder,
            BookContainerKind::ImageFolder,
            &[
                PageIdentity::RelativePath("page.jpg".into()),
                PageIdentity::RelativePath("page.png".into()),
            ],
        );
        add_bookmarks(
            &mut app,
            &other,
            BookContainerKind::ImageFolder,
            &[PageIdentity::RelativePath("page.jpg".into())],
        );
        let rows = crate::book_bookmarks::load_for_container_from_disk_readonly(&folder).unwrap();
        let other_rows =
            crate::book_bookmarks::load_for_container_from_disk_readonly(&other).unwrap();
        let id = rows
            .iter()
            .find(|row| row.page_identity == PageIdentity::RelativePath("page.jpg".into()))
            .unwrap()
            .id;
        app.load_folder(folder.clone());
        app.finish_page_identity_preparation_for_test();
        app.fullscreen_idx = Some(0);
        app.current_book_bookmarks = rows.clone();
        let (main_tx, main_cancel) = held_projection(&mut app, &folder);
        app.book_bookmark_title_edit = Some(BookBookmarkTitleEdit {
            id,
            focus_viewport: egui::ViewportId::ROOT,
            title: "main draft".into(),
            request_focus: false,
        });
        app.settings.file_type_visibility = policy_excluding("png");
        let mut sibling_held = None;
        let sibling = app.build_window_context_for_test(13453, |viewer| {
            viewer.load_folder(folder.clone());
            viewer.finish_page_identity_preparation_for_test();
            viewer.fullscreen_idx = Some(0);
            viewer.current_book_bookmarks =
                rows.iter().filter(|row| row.id == id).cloned().collect();
            sibling_held = Some(held_projection(viewer, &folder));
            viewer.book_bookmark_title_edit = Some(BookBookmarkTitleEdit {
                id,
                focus_viewport: egui::ViewportId::from_hash_of("matching-mutation-sibling"),
                title: "matching sibling draft".into(),
                request_focus: false,
            });
        });
        let (sibling_tx, sibling_cancel) = sibling_held.unwrap();
        app.settings.file_type_visibility = Default::default();
        let other_viewport = egui::ViewportId::from_hash_of("unrelated-mutation-sibling");
        let mut other_held = None;
        let unrelated = app.build_window_context_for_test(13454, |viewer| {
            viewer.load_folder(other.clone());
            viewer.finish_page_identity_preparation_for_test();
            viewer.fullscreen_idx = Some(0);
            viewer.current_book_bookmarks = other_rows.clone();
            other_held = Some(held_projection(viewer, &other));
            viewer.book_bookmark_title_edit = Some(BookBookmarkTitleEdit {
                id: other_rows[0].id,
                focus_viewport: other_viewport,
                title: "unrelated draft".into(),
                request_focus: true,
            });
        });
        let (_other_tx, other_cancel) = other_held.unwrap();
        let (other_key, other_generation) = app
            .with_viewer_context_ref(unrelated, |viewer| {
                (
                    viewer.book_bookmarks_request().unwrap().key.clone(),
                    viewer.items_generation(),
                )
            })
            .unwrap();
        let other_map = app
            .with_viewer_context(unrelated, |viewer| Arc::clone(&viewer.page_identity_index))
            .unwrap();
        app.with_viewer_context_ref(sibling, |viewer| {
            assert_eq!(
                viewer.book_bookmarks_request().unwrap().key.visibility,
                policy_excluding("png")
            );
        })
        .unwrap();
        assert_eq!(
            app.current_book_bookmarks_request
                .as_ref()
                .unwrap()
                .key
                .visibility,
            Default::default()
        );
        app.claim_pending_text_input_focus(
            other_viewport,
            crate::ui_adjustment_panel::book_bookmark_title_edit_widget_id(other_rows[0].id),
            1,
        );
        let other_claim = app.pending_text_input_focus.get();
        if title_edit {
            app.set_book_bookmark_title(id, "changed across contexts".into());
        } else {
            app.remove_book_bookmark(id);
        }
        assert!(main_cancel.load(Ordering::Relaxed));
        assert!(sibling_cancel.load(Ordering::Relaxed));
        assert!(!other_cancel.load(Ordering::Relaxed));
        assert!(app.current_book_bookmarks_request.is_none());
        assert!(app.current_book_bookmarks_key.is_none());
        assert!(app.current_book_bookmarks.is_empty());
        assert!(app.book_bookmark_title_edit.is_none());
        app.with_viewer_context_ref(sibling, |viewer| {
            assert!(viewer.book_bookmarks_request().is_none());
            assert!(viewer.book_bookmarks_key().is_none());
            assert!(viewer.book_bookmarks().is_empty());
            assert!(!viewer.book_bookmark_title_editing());
        })
        .unwrap();
        for tx in [main_tx, sibling_tx] {
            assert!(
                tx.send(crate::bookmark_browser::BookBookmarkProjection {
                    bookmarks: rows.clone(),
                    notice: None,
                })
                .is_err()
            );
        }
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while !app.book_bookmark_pending_requests.is_empty() {
            // Only Main drains the actor and all projection completions. The unrelated
            // deposited request must survive both submission and actual write completion.
            app.poll_book_bookmarks();
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        assert!(!other_cancel.load(Ordering::Relaxed));
        assert_eq!(app.pending_text_input_focus.get(), other_claim);
        app.with_viewer_context_ref(unrelated, |viewer| {
            let request = viewer.book_bookmarks_request().unwrap();
            assert_eq!(request.key, other_key);
            assert!(Arc::ptr_eq(&request.cancel, &other_cancel));
            assert!(viewer.book_bookmarks_key().is_none());
            assert_eq!(viewer.book_bookmarks(), other_rows);
            assert_eq!(viewer.items_generation(), other_generation);
            assert!(viewer.book_bookmark_title_editing());
        })
        .unwrap();
        app.with_viewer_context(unrelated, |viewer| {
            assert!(Arc::ptr_eq(&viewer.page_identity_index, &other_map));
            let draft = viewer.book_bookmark_title_edit.as_ref().unwrap();
            assert_eq!(draft.id, other_rows[0].id);
            assert_eq!(draft.focus_viewport, other_viewport);
            assert_eq!(draft.title, "unrelated draft");
            assert!(draft.request_focus);
            assert_eq!(viewer.pending_text_input_focus.get(), other_claim);
        })
        .unwrap();
        assert_eq!(
            crate::book_bookmarks::load_for_container_from_disk_readonly(&other).unwrap(),
            other_rows
        );
        let updated =
            crate::book_bookmarks::load_for_container_from_disk_readonly(&folder).unwrap();
        if title_edit {
            assert_eq!(updated.len(), rows.len());
            assert_eq!(
                updated
                    .iter()
                    .find(|row| row.id == id)
                    .unwrap()
                    .title
                    .as_deref(),
                Some("changed across contexts")
            );
        } else {
            assert_eq!(updated.len(), rows.len() - 1);
            assert!(!updated.iter().any(|row| row.id == id));
        }
    }
}

#[test]
fn visibility_staged_rating_handler_notifies_legacy_and_restores_after_regeneration() {
    let _epoch = crate::page_edit_write_epoch::TestEpochScope::fresh();
    let mut app = setup_app_for_test();
    let (source, cached) = crate::archive_visibility::legacy_conversion_fixture(app.tmp.path());
    for member in ["root.jpg", "chapter.zip/page.jpg", "notes.zip/page.jpg"] {
        let mut meta =
            crate::rating_db::RatingMeta::new(crate::rating_db::RatingItemKind::ZipImage)
                .with_source_path(&cached);
        meta.entry_name = Some(member.into());
        app.rating_db
            .as_ref()
            .unwrap()
            .set_user_rating(
                &format!("{}::{member}", cached.to_string_lossy()),
                4,
                Some(&meta),
            )
            .unwrap();
    }
    let stored = app.rating_db.as_ref().unwrap().list_by_stars(4).unwrap();
    app.settings.file_type_visibility = policy_excluding("zip");
    app.enter_rating_view(4);
    let ctx = egui::Context::default();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
    while app.rating_view_pending.is_some() || app.sidecar_restore_active() {
        app.poll_sidecar_restore(&ctx);
        app.poll_rating_view();
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    assert!(app.items_are_rating_view);
    assert!(app.items.is_empty());
    assert!(
        app.fs_feedback_toast
            .as_ref()
            .is_some_and(|toast| toast.0.contains("古い変換キャッシュ"))
    );
    assert_eq!(
        stored,
        app.rating_db.as_ref().unwrap().list_by_stars(4).unwrap()
    );
    crate::archive_visibility::regenerate_conversion_fixture(&source, &cached);
    app.reload_current_rating_view_preserving_sort();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
    while app.rating_view_pending.is_some() || app.sidecar_restore_active() {
        app.poll_sidecar_restore(&ctx);
        app.poll_rating_view();
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    assert_eq!(app.rating_view_rows.len(), 2);
    assert_eq!(
        stored,
        app.rating_db.as_ref().unwrap().list_by_stars(4).unwrap()
    );
}

#[test]
fn visibility_search_notice_keeps_nonterminal_search_active() {
    let mut app = setup_app_for_test();
    app.global_search.active = true;
    app.global_search.query = "pages".into();
    app.global_search.last_executed = "pages".into();
    app.global_search.done = false;
    let (tx, rx) = crossbeam_channel::unbounded();
    app.global_search.pending = Some(crate::indexer_manager::SearchHandle {
        cancel: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        rx,
    });
    tx.send(crate::global_search::SearchStreamEvent::Batch {
        hits: Vec::new(),
        scanned_candidates: 1,
        valid_hits: 0,
        visibility_notice: Some(
            crate::archive_visibility::ArchiveVisibilityNotice::LegacyConvertedCache,
        ),
    })
    .unwrap();
    app.poll_global_search_events(&egui::Context::default());
    assert!(app.global_search.is_searching());
    assert!(app.global_search.reject_message.is_none());
    assert!(!app.global_search.done);
    assert!(
        app.fs_feedback_toast
            .as_ref()
            .is_some_and(|toast| toast.0.contains("古い変換キャッシュ"))
    );
}
