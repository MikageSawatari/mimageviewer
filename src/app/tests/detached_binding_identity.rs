use super::*;

fn assert_mounted_window(app: &mut App, window: u64) {
    let owner = app.projected_viewer_context_id();
    assert_eq!(app.viewer_context_window(owner), Some(window));
    assert_eq!(app.ensure_detached_viewer_window_id(), window);
    assert_eq!(
        app.locate_window_context(window),
        Some((owner, ContextResidence::Mounted))
    );
}

fn start_cached_pdf(app: &mut App, root: &Path) -> (PathBuf, u64) {
    let pdf = root.join("binding.pdf");
    std::fs::write(
        &pdf,
        b"PDF enumeration completion is supplied at the worker channel",
    )
    .unwrap();
    let meta = std::fs::metadata(&pdf).unwrap();
    app.get_or_open_catalog(root)
        .unwrap()
        .set_pdf_meta(
            "binding.pdf",
            crate::ui_helpers::mtime_secs(&meta),
            meta.len() as i64,
            2,
            false,
        )
        .unwrap();
    app.settings.detached_viewer_open_images_in_window = true;
    app.settings.auto_fullscreen_zip_pdf = true;
    assert!(app.start_active_detached_book_context(
        ViewerContextDescriptor::Pdf {
            path: pdf.clone(),
            page_num: None
        },
        &egui::Context::default(),
        None,
        None,
    ));
    let window = app.active_detached_window_id().unwrap();
    (pdf, window)
}

#[test]
fn detached_binding_identity_book_start_resolves_published_window() {
    let mut app = setup_app_for_test();
    let root = app.tmp.path().to_path_buf();
    let (_, window) = start_cached_pdf(&mut app, &root);
    app.with_active_viewer_context(|mounted| {
        assert_eq!(mounted.pdf_placeholder_count, Some(2));
        assert_eq!(
            mounted.viewer_context_window(mounted.projected_viewer_context_id()),
            Some(window)
        );
        assert_eq!(mounted.ensure_detached_viewer_window_id(), window);
    })
    .unwrap();
}

#[test]
fn detached_binding_identity_pdf_load_poll_open_uses_published_window() {
    let mut app = setup_app_for_test();
    let root = app.tmp.path().to_path_buf();
    let (pdf, window) = start_cached_pdf(&mut app, &root);
    let main = app.projected_viewer_context_id();
    let generation = app.items_generation;
    app.with_active_viewer_context(|mounted| {
        let meta = std::fs::metadata(&pdf).unwrap();
        let (tx, rx) = mpsc::channel();
        tx.send(Ok((0..2)
            .map(|page_num| crate::pdf_loader::PdfPageEntry {
                page_num,
                mtime: crate::ui_helpers::mtime_secs(&meta),
                file_size: meta.len(),
            })
            .collect()))
            .unwrap();
        // Replace only the external worker response. Keep load_pdf_as_folder's request,
        // placeholder, deferred open, registry build/commit/mount and the real poll handler.
        mounted.pdf_enumerate_pending.as_mut().unwrap().2.rx = rx;
        mounted.poll_pdf_enumerate();
        assert_eq!(mounted.fullscreen_idx, Some(0));
        assert_eq!(mounted.active_detached_window_id(), Some(window));
        assert_eq!(mounted.ensure_detached_viewer_window_id(), window);
        assert_eq!(
            mounted.viewer_context_window(mounted.projected_viewer_context_id()),
            Some(window)
        );
    })
    .unwrap();
    assert_eq!(app.projected_viewer_context_id(), main);
    assert_eq!(app.items_generation, generation);
}

#[test]
fn detached_binding_identity_scanned_folder_opens_inside_build_with_reserved_window() {
    let mut app = setup_app_for_test();
    let folder = app.tmp.path().join("book");
    std::fs::create_dir(&folder).unwrap();
    std::fs::write(folder.join("page.jpg"), b"fixture").unwrap();
    app.settings.detached_viewer_open_images_in_window = true;
    app.settings.auto_fullscreen_image_folders = true;
    assert!(app.start_active_detached_book_context_from_scanned_folder(
        folder.clone(),
        scan_directory(&folder),
        &egui::Context::default(),
        None,
    ));
    let window = app.active_detached_window_id().unwrap();
    app.with_active_viewer_context(|mounted| {
        assert_eq!(mounted.fullscreen_idx, Some(0));
        assert_mounted_window(mounted, window);
    })
    .unwrap();
}

#[test]
fn detached_binding_identity_zip_load_poll_open_uses_published_window() {
    let mut app = setup_app_for_test();
    let zip = app.tmp.path().join("book.zip");
    app.settings.detached_viewer_open_images_in_window = true;
    assert!(app.start_active_detached_book_context(
        ViewerContextDescriptor::Zip {
            path: zip.clone(),
            entry_name: None,
            archive_source_override: None
        },
        &egui::Context::default(),
        None,
        None,
    ));
    let window = app.active_detached_window_id().unwrap();
    app.with_active_viewer_context(|mounted| {
        let (tx, rx) = mpsc::channel();
        tx.send(Ok(crate::zip_loader::ZipEnumeration {
            entries: vec![crate::zip_loader::ZipImageEntry {
                entry_name: "page.jpg".into(),
                uncompressed_size: 1,
                mtime: 1,
            }],
            has_foreign_archives: false,
            legacy_renames: Vec::new(),
        }))
        .unwrap();
        mounted.zip_enumerate_pending.as_mut().unwrap().rx = rx;
        mounted.poll_zip_enumerate();
        assert_eq!(mounted.fullscreen_idx, Some(0));
        assert_mounted_window(mounted, window);
    })
    .unwrap();
}

#[test]
fn detached_binding_identity_build_close_abort_and_mount_do_not_copy_window_ownership() {
    let mut app = setup_app_for_test();
    let main = app.projected_viewer_context_id();
    let main_window = app.ensure_detached_viewer_window_id();
    app.ensure_mounted_detached_session_binding(main_window);
    let built = app
        .build_viewer_context("binding_identity", |building, _| {
            building.reserve_window_binding_for_build(81);
            assert!(building.locate_window_context(81).is_none());
            building.prepare_viewer_presentation_close();
            assert_eq!(building.ensure_detached_viewer_window_id(), 81);
            assert!(building.locate_window_context(81).is_none());
            BuildOutcome::Commit
        })
        .unwrap();
    assert_mounted_window(&mut app, main_window);
    app.with_viewer_context(built, |mounted| {
        mounted.prepare_viewer_presentation_close();
        assert_mounted_window(mounted, 81);
    })
    .unwrap();
    assert_mounted_window(&mut app, main_window);
    assert!(
        app.build_viewer_context("binding_abort", |building, _| {
            building.reserve_window_binding_for_build(82);
            building.prepare_viewer_presentation_close();
            assert_eq!(building.ensure_detached_viewer_window_id(), 82);
            BuildOutcome::Abort("test cancellation")
        })
        .is_none()
    );
    assert_eq!(app.projected_viewer_context_id(), main);
    assert_mounted_window(&mut app, main_window);
    assert!(app.locate_window_context(82).is_none());
    app.retire_context(built, "binding_retire", |_| ()).unwrap();
    assert!(app.locate_window_context(81).is_none());
    assert_mounted_window(&mut app, main_window);
}

fn assert_projection_matches_binding(app: &mut App) {
    let binding = app.viewer_context_window(app.projected_viewer_context_id());
    assert_eq!(app.detached_viewer_window_id(), binding);
    if let Some(window) = binding {
        assert_mounted_window(app, window);
    }
}

fn install_detached_item(app: &mut App, ctx: &egui::Context, media: bool) -> u64 {
    app.settings.detached_viewer_enabled = true;
    app.settings.detached_viewer_open_images_in_window = true;
    app.items = vec![if media {
        GridItem::Video(PathBuf::from("binding-media.mp4"))
    } else {
        GridItem::Image(PathBuf::from("binding-page.jpg"))
    }];
    app.thumbnails = vec![ThumbnailState::Loaded {
        tex: ctx.load_texture(
            "binding-page",
            egui::ColorImage::filled([2, 2], egui::Color32::WHITE),
            egui::TextureOptions::LINEAR,
        ),
        from_cache: false,
        from_edit_preview: false,
        rendered_at_px: 2,
        source_dims: Some((2, 2)),
        layout_dims: None,
    }];
    app.image_metas = vec![None];
    app.visible_indices = vec![0];
    app.fullscreen_idx = Some(0);
    app.viewer_presentation = ViewerPresentation::DetachedWindow;
    let window = app.ensure_detached_viewer_window_id();
    app.ensure_mounted_detached_session_binding(window);
    app.begin_active_detached_session(
        window,
        if media {
            DetachedSource::Video
        } else {
            DetachedSource::Image
        },
    );
    window
}

#[test]
fn detached_binding_identity_terminal_retirement_then_release_uses_binding() {
    let mut app = setup_app_for_test();
    app.bind_mounted_context_for_test(31);
    // Runtime destruction precedes release of the context association. Even here there
    // is only one answer; a legacy stale-runtime guard used to invent a second window.
    app.retire_terminal_detached_viewport_identity(31, "binding_terminal");
    assert_projection_matches_binding(&mut app);
    app.unbind_window(31);
    assert_projection_matches_binding(&mut app);
}

#[test]
fn detached_binding_identity_mode_close_releases_all_contexts() {
    let mut app = setup_app_for_test();
    let ctx = egui::Context::default();
    let window = install_detached_item(&mut app, &ctx, false);
    assert!(app.close_all_detached_viewers_for_mode_change(&ctx));
    assert!(app.locate_window_context(window).is_none());
    assert_projection_matches_binding(&mut app);
}

#[test]
fn detached_binding_identity_legacy_still_park_releases_only_main_binding() {
    let mut app = setup_app_for_test();
    let ctx = egui::Context::default();
    let window = install_detached_item(&mut app, &ctx, false);
    assert!(app.park_and_close_current_active_detached_viewer_inner(&ctx, false));
    assert!(
        app.detached_image_windows
            .iter()
            .any(|snapshot| snapshot.id == window)
    );
    assert_projection_matches_binding(&mut app);
    assert!(app.locate_window_context(window).is_none());
}

#[test]
fn detached_binding_identity_mounted_live_media_park_transfers_binding() {
    let mut app = setup_app_for_test();
    let ctx = egui::Context::default();
    let window = install_detached_item(&mut app, &ctx, true);
    assert!(app.park_current_viewer_context_as_live_media_inner(&ctx, "binding_live_fork"));
    assert_projection_matches_binding(&mut app);
    app.with_window_viewer_context(window, |parked| assert_mounted_window(parked, window))
        .unwrap();
    assert_projection_matches_binding(&mut app);
}

#[test]
fn detached_binding_identity_at_rest_live_media_park_preserves_sibling_binding() {
    let mut app = setup_app_for_test();
    let ctx = egui::Context::default();
    let main_window = app.ensure_detached_viewer_window_id();
    app.ensure_mounted_detached_session_binding(main_window);
    let id = app.build_active_context_for_test(Some(61), DetachedSource::Video, |media| {
        media.items = vec![GridItem::Video(PathBuf::from("binding-media.mp4"))];
        media.fullscreen_idx = Some(0);
        media.viewer_presentation = ViewerPresentation::DetachedWindow;
    });
    assert!(app.park_active_detached_context_as_live_media(&ctx, "binding_at_rest_park"));
    assert_mounted_window(&mut app, main_window);
    app.with_viewer_context(id, |parked| assert_mounted_window(parked, 61))
        .unwrap();
    assert_mounted_window(&mut app, main_window);
}
