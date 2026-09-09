use super::*;
use crate::app::folder_scan::{OmittedFolderEntryCounts, ScanMediaKind};
use crate::grid_item::{GridItem, ThumbnailState};
use crate::snapshot::{SnapshotSourceLabel, SnapshotTarget};
use crate::ui_fullscreen::FsPageNav;
use std::path::{Path, PathBuf};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
    mpsc,
};

fn seed_images(app: &mut App, paths: &[PathBuf]) {
    app.items = paths.iter().cloned().map(GridItem::Image).collect();
    app.thumbnails = vec![ThumbnailState::Pending; paths.len()];
    app.image_metas = vec![None; paths.len()];
    app.visible_indices = (0..paths.len()).collect();
    app.current_folder = paths
        .first()
        .and_then(|path| path.parent())
        .map(Path::to_path_buf);
    app.address = app
        .current_folder
        .as_deref()
        .map_or_else(String::new, |path| path.display().to_string());
    app.fullscreen_idx = (!paths.is_empty()).then_some(0);
    app.selected = app.fullscreen_idx;
    app.fs_info_panel.locked = true;
    app.fs_info_panel.open = crate::ui_helpers::MetadataPanelOpenState::ByPointer;
}

fn viewer_with_images(paths: &[PathBuf]) -> AppTestEnvForTest {
    let mut app = setup_app_for_test();
    seed_images(&mut app, paths);
    app
}

fn grid_item_keys(items: &[GridItem]) -> Vec<String> {
    items.iter().map(GridItem::perf_key).collect()
}

fn image_scan(paths: &[PathBuf]) -> ScannedDir {
    ScannedDir {
        folders: Vec::new(),
        all_media: paths
            .iter()
            .cloned()
            .map(|path| (path, ScanMediaKind::Image, 0, 0))
            .collect(),
        omitted: OmittedFolderEntryCounts::default(),
    }
}

/// Replace the real worker channel immediately after the production admission path has run.
/// The returned sender lets each test choose success, failure, disconnection, or a still-pending
/// request without sleeps or filesystem timing races.
fn control_required_scan(
    app: &mut App,
) -> (mpsc::Sender<std::io::Result<ScannedDir>>, Arc<AtomicBool>) {
    let pending = app
        .folder_pane_open_pending
        .take()
        .expect("required folder scan must be admitted");
    assert!(matches!(
        pending.purpose,
        FolderOpenScanPurpose::RequiredFullscreenTarget { .. }
    ));
    pending.cancel.store(true, Ordering::Relaxed);

    let (tx, rx) = mpsc::channel();
    let cancel = Arc::new(AtomicBool::new(false));
    app.folder_pane_open_pending = Some(FolderPaneOpenPending {
        path: pending.path,
        cancel: Arc::clone(&cancel),
        rx,
        purpose: pending.purpose,
    });
    (tx, cancel)
}

fn begin_required_physical_scan(
    app: &mut App,
    ctx: &egui::Context,
    folder: PathBuf,
    target: PathBuf,
) {
    app.open_required_fullscreen_location(
        ctx,
        folder,
        SnapshotTarget::Fs(target),
        HistoryTrigger::UserChosen,
    );
}

fn install_navigation_sequence(app: &mut App, target: FsNavigationSequenceTarget) {
    app.fs_holdover_tex = Some(FsHoldover::NavigationSequence(FsNavigationSequence {
        previous: None,
        opened_at: std::time::Instant::now(),
        target,
    }));
}

fn install_awaiting_password_sequence(app: &mut App) {
    let accepted_generation = app.items_generation;
    install_navigation_sequence(
        app,
        FsNavigationSequenceTarget::AwaitingPassword {
            accepted_generation,
        },
    );
}

fn install_folder_items_sequence(app: &mut App) {
    let accepted_generation = app.items_generation;
    install_navigation_sequence(
        app,
        FsNavigationSequenceTarget::FolderItems {
            accepted_generation,
        },
    );
}

fn install_rendition_failed_sequence(app: &mut App, pages: Vec<usize>) {
    let items_generation = app.items_generation;
    install_navigation_sequence(
        app,
        FsNavigationSequenceTarget::Display(FsNavigationDisplayTarget {
            items_generation,
            pages,
            phase: FsNavigationTargetPhase::RenditionFailed,
        }),
    );
}

fn loaded_thumbnail(ctx: &egui::Context, label: &str) -> ThumbnailState {
    let texture = ctx.load_texture(
        label,
        egui::ColorImage::filled([2, 3], egui::Color32::WHITE),
        egui::TextureOptions::LINEAR,
    );
    ThumbnailState::Loaded {
        tex: texture,
        origin: crate::thumb_loader::ThumbLoadOrigin::SourceGenerated {
            evaluated_display_px: 64,
        },
        from_edit_preview: false,
        rendered_at_px: 64,
        source_dims: Some((2, 3)),
        layout_dims: None,
    }
}

fn similar_query_hit(
    item_key: String,
    kind: crate::similar_db::ItemKind,
    page_index: Option<u32>,
    format: crate::similar_image::SimilarImageFormat,
    target: crate::similar_index::SimilarItemTarget,
) -> crate::similar_index::QueryHit {
    crate::similar_index::QueryHit {
        item_id: 1,
        item_key,
        kind,
        container_key: None,
        page_index,
        distance: 1,
        band: crate::similar_index::MatchBand::NearlyIdentical,
        mtime: 1,
        file_size: 1,
        width: 2,
        height: 3,
        format,
        target: Some(target),
    }
}

fn similar_file_hit(path: PathBuf) -> crate::similar_index::QueryHit {
    similar_query_hit(
        crate::similar_index::item_key_for_file(&path),
        crate::similar_db::ItemKind::Image,
        None,
        crate::similar_image::SimilarImageFormat::Jpeg,
        crate::similar_index::SimilarItemTarget::File(path),
    )
}

#[cfg(windows)]
#[test]
fn physical_similar_move_waits_for_scan_then_opens_only_the_requested_leaf() {
    let ctx = egui::Context::default();
    let mut app = setup_app_for_test();
    let old_folder = app.tmp.path().join("old");
    let target_folder = app.tmp.path().join("target");
    std::fs::create_dir_all(&old_folder).unwrap();
    std::fs::create_dir_all(&target_folder).unwrap();
    let old = old_folder.join("old.jpg");
    let first = target_folder.join("a.jpg");
    let requested = target_folder.join("b.jpg");
    app.items = vec![GridItem::Image(old.clone())];
    app.thumbnails = vec![ThumbnailState::Pending];
    app.image_metas = vec![None];
    app.visible_indices = vec![0];
    app.current_folder = Some(old_folder);
    app.fullscreen_idx = Some(0);
    app.selected = Some(0);
    app.fs_info_panel.locked = true;
    app.fs_info_panel.open = crate::ui_helpers::MetadataPanelOpenState::ByPointer;
    app.activate_snapshot(SnapshotSourceLabel::Mixed);

    let hit = similar_file_hit(requested.clone());
    app.open_similar_hit_for_test(&ctx, &hit);
    assert!(
        app.is_snapshot_active(),
        "scan admission must preserve the snapshot"
    );
    assert_eq!(
        app.fullscreen_idx,
        Some(0),
        "old page stays visible while scanning"
    );

    let (tx, _) = control_required_scan(&mut app);
    tx.send(Ok(image_scan(&[first, requested.clone()])))
        .unwrap();
    let ready = app
        .poll_folder_pane_open(&ctx)
        .expect("controlled scan ready");
    assert!(app.resolve_main_folder_open_ready(&ctx, ready).is_none());

    assert!(
        !app.is_snapshot_active(),
        "snapshot is dismissed only after scan success"
    );
    let opened = app
        .fullscreen_idx
        .and_then(|idx| app.items.get(idx))
        .expect("requested image opened");
    assert!(
        matches!(opened, GridItem::Image(path) if crate::folder_tree::path_eq(path, &requested))
    );
    assert!(
        app.fs_info_panel.locked,
        "viewer-internal move keeps panel lock"
    );
    assert!(matches!(
        app.fs_holdover_tex,
        Some(FsHoldover::NavigationSequence(_))
    ));
}

#[cfg(windows)]
#[test]
fn failed_physical_scan_keeps_existing_snapshot_page_and_panel_but_retires_old_dfs_owner() {
    let ctx = egui::Context::default();
    let mut app = setup_app_for_test();
    let old_folder = app.tmp.path().join("old-failure");
    let target_folder = app.tmp.path().join("target-failure");
    std::fs::create_dir_all(&old_folder).unwrap();
    std::fs::create_dir_all(&target_folder).unwrap();
    let old = old_folder.join("old.jpg");
    let requested = target_folder.join("requested.jpg");
    app.items = vec![GridItem::Image(old.clone())];
    app.thumbnails = vec![ThumbnailState::Pending];
    app.image_metas = vec![None];
    app.visible_indices = vec![0];
    app.current_folder = Some(old_folder.clone());
    app.fullscreen_idx = Some(0);
    app.selected = Some(0);
    app.fs_info_panel.locked = true;
    app.fs_info_panel.open = crate::ui_helpers::MetadataPanelOpenState::ByPointer;
    app.activate_snapshot(SnapshotSourceLabel::Mixed);
    app.capture_fs_nav_holdover(0);
    assert!(matches!(
        app.fs_holdover_tex,
        Some(FsHoldover::FolderNavigation(None))
    ));

    begin_required_physical_scan(&mut app, &ctx, target_folder, requested);
    let (tx, _) = control_required_scan(&mut app);
    tx.send(Err(std::io::Error::new(
        std::io::ErrorKind::PermissionDenied,
        "controlled failure",
    )))
    .unwrap();
    let ready = app
        .poll_folder_pane_open(&ctx)
        .expect("controlled failure ready");
    assert!(app.resolve_main_folder_open_ready(&ctx, ready).is_none());

    assert!(app.is_snapshot_active());
    assert_eq!(app.current_folder.as_deref(), Some(old_folder.as_path()));
    assert_eq!(app.fullscreen_idx, Some(0));
    assert!(matches!(&app.items[0], GridItem::Image(path) if path == &old));
    assert!(
        app.fs_info_panel.locked,
        "failed replacement is not a viewer exit"
    );
    assert!(app.fs_holdover_tex.is_none());
    assert!(!app.fs_nav_is_locked());
}

#[cfg(windows)]
#[test]
fn detached_required_scan_failure_without_a_page_exits_the_password_wait_owner() {
    let ctx = egui::Context::default();
    let mut app = setup_app_for_test();
    let folder = app.tmp.path().join("detached-failure");
    std::fs::create_dir_all(&folder).unwrap();
    app.fullscreen_idx = None;
    app.fs_info_panel.locked = true;
    app.fs_info_panel.open = crate::ui_helpers::MetadataPanelOpenState::ByPointer;
    install_awaiting_password_sequence(&mut app);
    let (tx, rx) = mpsc::channel();
    tx.send(Err(std::io::Error::new(
        std::io::ErrorKind::NotFound,
        "controlled detached failure",
    )))
    .unwrap();
    app.folder_pane_open_pending = Some(FolderPaneOpenPending {
        path: folder.clone(),
        cancel: Arc::new(AtomicBool::new(false)),
        rx,
        purpose: FolderOpenScanPurpose::RequiredFullscreenTarget {
            target: SnapshotTarget::Fs(folder.join("missing.jpg")),
            history_trigger: HistoryTrigger::UserChosen,
        },
    });

    assert!(matches!(
        app.poll_detached_physical_folder_open(&ctx),
        DetachedPhysicalFolderOpenPoll::Failed
    ));
    assert!(app.fs_holdover_tex.is_none());
    assert!(
        !app.fs_info_panel.locked,
        "page-less failure is the true viewer end"
    );
}

#[test]
fn disconnected_required_scan_uses_the_same_terminal_failure_boundary() {
    let ctx = egui::Context::default();
    let mut app = setup_app_for_test();
    app.fullscreen_idx = None;
    app.fs_info_panel.locked = true;
    app.fs_info_panel.open = crate::ui_helpers::MetadataPanelOpenState::ByPointer;
    install_awaiting_password_sequence(&mut app);
    let (tx, rx) = mpsc::channel::<std::io::Result<ScannedDir>>();
    drop(tx);
    app.folder_pane_open_pending = Some(FolderPaneOpenPending {
        path: app.tmp.path().join("worker-disconnected"),
        cancel: Arc::new(AtomicBool::new(false)),
        rx,
        purpose: FolderOpenScanPurpose::RequiredFullscreenTarget {
            target: SnapshotTarget::Fs(app.tmp.path().join("worker-disconnected/p.jpg")),
            history_trigger: HistoryTrigger::UserChosen,
        },
    });

    assert!(app.poll_folder_pane_open(&ctx).is_none());
    assert!(app.folder_pane_open_pending.is_none());
    assert!(app.fs_holdover_tex.is_none());
    assert!(!app.fs_info_panel.locked);
}

#[test]
fn page_handler_supersedes_a_pending_required_scan_and_preserves_panel_lock() {
    let ctx = egui::Context::default();
    let mut app = setup_app_for_test();
    let folder = app.tmp.path().join("handler-current");
    let target_folder = app.tmp.path().join("handler-target");
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::create_dir_all(&target_folder).unwrap();
    seed_images(&mut app, &[folder.join("a.jpg"), folder.join("b.jpg")]);
    app.capture_fs_nav_holdover(0);
    begin_required_physical_scan(
        &mut app,
        &ctx,
        target_folder.clone(),
        target_folder.join("c.jpg"),
    );
    let (_tx, cancel) = control_required_scan(&mut app);

    app.handle_fs_navigation(
        &ctx,
        false,
        false,
        None,
        None,
        None,
        FsPageNav::Target(1),
        None,
        0,
    );

    assert!(cancel.load(Ordering::Relaxed));
    assert!(app.folder_pane_open_pending.is_none());
    assert_eq!(app.fullscreen_idx, Some(1));
    assert!(app.fs_info_panel.locked);
    assert!(!matches!(
        app.fs_holdover_tex,
        Some(FsHoldover::FolderNavigation(_))
    ));
}

#[test]
fn deepest_fullscreen_open_boundary_covers_navigation_callers_that_bypass_the_wrapper() {
    let ctx = egui::Context::default();
    let mut app = setup_app_for_test();
    let folder = app.tmp.path().join("deep-current");
    let target_folder = app.tmp.path().join("deep-target");
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::create_dir_all(&target_folder).unwrap();
    seed_images(&mut app, &[folder.join("a.jpg"), folder.join("b.jpg")]);
    begin_required_physical_scan(
        &mut app,
        &ctx,
        target_folder.clone(),
        target_folder.join("c.jpg"),
    );
    let (_tx, cancel) = control_required_scan(&mut app);

    app.open_fullscreen_from_fs_navigation(&ctx, 1, HistoryTrigger::UserChosen);

    assert!(cancel.load(Ordering::Relaxed));
    assert!(app.folder_pane_open_pending.is_none());
    assert_eq!(app.fullscreen_idx, Some(1));
    assert!(app.fs_info_panel.locked);
}

#[test]
fn same_item_internal_reopen_does_not_cancel_a_required_scan() {
    let ctx = egui::Context::default();
    let mut app = setup_app_for_test();
    let folder = app.tmp.path().join("same-current");
    let target_folder = app.tmp.path().join("same-target");
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::create_dir_all(&target_folder).unwrap();
    seed_images(&mut app, &[folder.join("a.jpg")]);
    begin_required_physical_scan(
        &mut app,
        &ctx,
        target_folder.clone(),
        target_folder.join("b.jpg"),
    );
    let (_tx, cancel) = control_required_scan(&mut app);

    app.open_fullscreen(0, HistoryTrigger::AutoAdvance);

    assert!(!cancel.load(Ordering::Relaxed));
    assert!(matches!(
        app.folder_pane_open_pending
            .as_ref()
            .map(|pending| &pending.purpose),
        Some(FolderOpenScanPurpose::RequiredFullscreenTarget { .. })
    ));
    app.cancel_required_fullscreen_folder_open();
}

#[test]
fn continuous_reanchor_supersedes_scan_when_only_the_page_slice_changes() {
    let ctx = egui::Context::default();
    let mut app = setup_app_for_test();
    let folder = app.tmp.path().join("continuous-current");
    let target_folder = app.tmp.path().join("continuous-target");
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::create_dir_all(&target_folder).unwrap();
    seed_images(&mut app, &[folder.join("wide.jpg")]);
    app.fullscreen_page_slice = crate::page_split::PageSlice::Left;
    begin_required_physical_scan(
        &mut app,
        &ctx,
        target_folder.clone(),
        target_folder.join("other.jpg"),
    );
    let (_tx, cancel) = control_required_scan(&mut app);

    app.reanchor_continuous_reading_viewer(
        &ctx,
        &[0.0, 100.0],
        1,
        0,
        crate::page_split::PageSlice::Right,
        HistoryTrigger::UserChosen,
    );

    assert!(cancel.load(Ordering::Relaxed));
    assert!(app.folder_pane_open_pending.is_none());
    assert_eq!(app.fullscreen_idx, Some(0));
    assert_eq!(
        app.fullscreen_page_slice,
        crate::page_split::PageSlice::Right
    );
    assert!(app.fs_info_panel.locked);
}

#[test]
fn rendition_failed_does_not_block_the_snapshot_ctrl_navigation_handler() {
    let ctx = egui::Context::default();
    let mut app = viewer_with_images(&[
        PathBuf::from(r"C:\snapshot\a.jpg"),
        PathBuf::from(r"C:\snapshot\b.jpg"),
    ]);
    app.activate_snapshot(SnapshotSourceLabel::Mixed);
    app.fullscreen_idx = Some(0);
    install_rendition_failed_sequence(&mut app, vec![0]);
    app.fs_nav_locked_gen = None;

    app.handle_fullscreen_ctrl_nav_context(&ctx, 0, true, false);

    assert_eq!(app.fullscreen_idx, Some(1));
    assert!(app.fs_info_panel.locked);
}

#[test]
fn password_wait_can_resume_and_true_cancel_releases_the_viewer_owner() {
    let mut app = viewer_with_images(&[PathBuf::from(r"C:\pdf\old.jpg")]);
    install_folder_items_sequence(&mut app);
    app.fs_nav_locked_gen = Some(app.items_generation);
    assert!(app.suspend_fs_navigation_sequence_for_password());
    assert!(!app.fs_nav_is_locked(), "password UI must accept input");
    assert!(app.fs_navigation_continues_viewer_during_content_teardown());
    assert!(app.resume_fs_navigation_sequence_after_password());
    assert!(app.fs_nav_is_locked());

    assert!(app.suspend_fs_navigation_sequence_for_password());
    app.fullscreen_idx = None;
    let pdf = PathBuf::from(r"C:\pdf\locked.pdf");
    app.pdf_password_request = Some(PdfPasswordRequest { path: pdf.clone() });
    app.fs_nav_after_pdf_enumerate = Some(DeferredFsReopen {
        history_trigger: HistoryTrigger::UserChosen,
        resume_slideshow: false,
        target: DeferredFsTarget::Required(SnapshotTarget::PdfPage {
            pdf_path: pdf,
            page_num: 7,
        }),
        resume_to_last_page: false,
        from_explicit_open: false,
        preserve_after_password_prompt: true,
    });
    assert!(app.cancel_pdf_password_dialog_request());
    assert!(app.fs_holdover_tex.is_none());
    assert!(!app.fs_info_panel.locked);
}

#[test]
fn similar_hit_pdf_handler_polls_and_missing_required_page_never_falls_back() {
    let ctx = egui::Context::default();
    let mut app = setup_app_for_test();
    let old_folder = app.tmp.path().join("pdf-current");
    std::fs::create_dir_all(&old_folder).unwrap();
    seed_images(&mut app, &[old_folder.join("old.jpg")]);
    let pdf = app.tmp.path().join("required.pdf");
    std::fs::write(&pdf, b"controlled by test coordinator").unwrap();
    let requested_page = 9;
    let hit = similar_query_hit(
        crate::similar_index::item_key_for_pdf_page(&pdf, requested_page),
        crate::similar_db::ItemKind::PdfPage,
        Some(requested_page),
        crate::similar_image::SimilarImageFormat::Pdf,
        crate::similar_index::SimilarItemTarget::PdfPage {
            pdf_path: pdf.clone(),
            page_num: requested_page,
        },
    );

    app.open_similar_hit_for_test(&ctx, &hit);
    let (pending_path, password, pending_handle) = app
        .pdf_enumerate_pending
        .take()
        .expect("PDF handler must start asynchronous enumeration");
    assert!(crate::folder_tree::path_eq(&pending_path, &pdf));
    pending_handle.cancel();
    drop(pending_handle);
    let completed = crate::pdf_loader::completed_enumerate_handle_for_test(
        &pdf,
        Ok(vec![
            crate::pdf_loader::PdfPageEntry {
                page_num: 0,
                mtime: 1,
                file_size: 1,
            },
            crate::pdf_loader::PdfPageEntry {
                page_num: 1,
                mtime: 1,
                file_size: 1,
            },
        ]),
    );
    app.pdf_enumerate_pending = Some((pending_path, password, completed));

    app.poll_pdf_enumerate();

    assert_eq!(app.fullscreen_idx, None);
    assert!(app.pdf_enumerate_pending.is_none());
    assert!(app.fs_nav_after_pdf_enumerate.is_none());
    assert!(app.fs_holdover_tex.is_none());
    assert!(!app.fs_info_panel.locked);
    assert!(matches!(
        app.items.first(),
        Some(GridItem::PdfPage { page_num: 0, .. })
    ));
}

#[test]
fn similar_book_page_zip_handler_polls_then_materializes_real_case_and_exact_leaf() {
    let ctx = egui::Context::default();
    let mut app = setup_app_for_test();
    let old_folder = app.tmp.path().join("zip-current");
    std::fs::create_dir_all(&old_folder).unwrap();
    seed_images(&mut app, &[old_folder.join("old.jpg")]);
    let zip_path = app.tmp.path().join("Outer.ZIP");
    std::fs::write(&zip_path, b"controlled by test channel").unwrap();

    app.open_similar_book_page_for_test(
        &ctx,
        "missing-zip-page-key",
        crate::similar_index::SimilarItemTarget::ZipPage {
            zip_path: zip_path.clone(),
            entry_name: "booka/sub/p01.jpg".to_owned(),
        },
    );
    let pending = app
        .zip_enumerate_pending
        .take()
        .expect("ZIP handler must start asynchronous enumeration");
    assert!(crate::folder_tree::path_eq(&pending.zip_path, &zip_path));
    pending.cancel.store(true, Ordering::Relaxed);
    let input_seq = pending.input_seq;
    let (tx, rx) = mpsc::channel();
    tx.send(Ok(crate::zip_loader::ZipEnumeration {
        entries: vec![
            crate::zip_loader::ZipImageEntry {
                entry_name: "BookA/Sub/P01.JPG".to_owned(),
                uncompressed_size: 1,
                mtime: 0,
            },
            crate::zip_loader::ZipImageEntry {
                entry_name: "BookB/P02.JPG".to_owned(),
                uncompressed_size: 1,
                mtime: 0,
            },
        ],
        has_foreign_archives: false,
        legacy_renames: Vec::new(),
    }))
    .unwrap();
    app.zip_enumerate_pending = Some(ZipEnumeratePending {
        zip_path: zip_path.clone(),
        input_seq,
        cancel: Arc::new(AtomicBool::new(false)),
        rx,
    });

    app.poll_zip_enumerate();

    let opened = app
        .fullscreen_idx
        .and_then(|idx| app.items.get(idx))
        .expect("nested target opened after poll");
    assert!(matches!(
        opened,
        GridItem::ZipImage { zip_path: actual_outer, entry_name }
            if crate::folder_tree::path_eq(actual_outer, &zip_path)
                && entry_name == "BookA/Sub/P01.JPG"
    ));
    assert!(app.fs_info_panel.locked);
    assert!(app.zip_enumerate_pending.is_none());
    assert!(app.fs_nav_after_pdf_enumerate.is_none());
}

#[cfg(windows)]
#[test]
fn presentation_switch_exposes_and_expires_its_asset_without_overwriting_navigation_owner() {
    let ctx = egui::Context::default();
    let mut app = viewer_with_images(&[PathBuf::from(r"C:\photos\page.jpg")]);
    let thumbnail = loaded_thumbnail(&ctx, "similar_navigation_presentation");
    let expected_texture = match &thumbnail {
        ThumbnailState::Loaded { tex, .. } => tex.id(),
        _ => unreachable!(),
    };
    app.thumbnails[0] = thumbnail;
    app.settings.video_in_window_mode = true;
    app.native_video_in_window_active = true;
    app.viewer_presentation = ViewerPresentation::MainWindow;

    app.toggle_still_window_mode();
    let display = app
        .presentation_switch_holdover_for_test()
        .expect("presentation switch must expose the captured page to the viewport bridge");
    assert_eq!(display.pages.len(), 1);
    assert_eq!(
        display.pages[0].texture.source_texture_id(),
        expected_texture
    );
    assert!(!app.fs_navigation_continues_viewer_during_content_teardown());
    app.still_fullscreen_viewport_enter_suppress_until =
        Some(std::time::Instant::now() - std::time::Duration::from_millis(1));
    assert!(!app.still_fullscreen_viewport_enter_suppressed());
    assert!(app.presentation_switch_holdover_for_test().is_none());
    assert!(app.fs_holdover_tex.is_none());

    app.settings.video_in_window_mode = true;
    app.native_video_in_window_active = true;
    app.viewer_presentation = ViewerPresentation::MainWindow;
    app.toggle_still_window_mode();
    assert!(app.presentation_switch_holdover_for_test().is_some());
    app.handle_fullscreen_close_request();
    assert!(
        !app.fs_info_panel.locked,
        "presentation switch does not mask a true close"
    );

    app.fullscreen_idx = Some(0);
    app.fs_info_panel.locked = true;
    app.capture_fs_nav_holdover(0);
    assert!(matches!(
        app.fs_holdover_tex,
        Some(FsHoldover::FolderNavigation(Some(_)))
    ));
    app.settings.video_in_window_mode = true;
    app.toggle_still_window_mode();
    assert!(matches!(
        app.fs_holdover_tex,
        Some(FsHoldover::FolderNavigation(Some(_)))
    ));
    app.still_fullscreen_viewport_enter_suppress_until =
        Some(std::time::Instant::now() - std::time::Duration::from_millis(1));
    assert!(!app.still_fullscreen_viewport_enter_suppressed());
    assert!(matches!(
        app.fs_holdover_tex,
        Some(FsHoldover::FolderNavigation(Some(_)))
    ));
}

#[cfg(windows)]
#[test]
fn true_close_cancels_a_required_scan_started_by_the_similar_hit_handler() {
    let ctx = egui::Context::default();
    let mut app = setup_app_for_test();
    let current = app.tmp.path().join("true-close-current");
    let target_folder = app.tmp.path().join("true-close-target");
    std::fs::create_dir_all(&current).unwrap();
    std::fs::create_dir_all(&target_folder).unwrap();
    seed_images(&mut app, &[current.join("page.jpg")]);

    let hit = similar_file_hit(target_folder.join("other.jpg"));
    app.open_similar_hit_for_test(&ctx, &hit);
    let (_tx, cancel) = control_required_scan(&mut app);
    app.handle_fullscreen_close_request();

    assert!(cancel.load(Ordering::Relaxed));
    assert!(app.folder_pane_open_pending.is_none());
    assert!(app.fs_holdover_tex.is_none());
    assert!(!app.fs_info_panel.locked);
    assert_eq!(app.fullscreen_idx, None);
}

#[cfg(windows)]
#[test]
fn detached_similar_book_page_handler_applies_in_its_bundle_and_true_close_unlocks() {
    let ctx = egui::Context::default();
    let mut app = setup_app_for_test();
    let main_folder = app.tmp.path().join("detached-main");
    let detached_folder = app.tmp.path().join("detached-current");
    let target_folder = app.tmp.path().join("detached-target");
    std::fs::create_dir_all(&main_folder).unwrap();
    std::fs::create_dir_all(&detached_folder).unwrap();
    std::fs::create_dir_all(&target_folder).unwrap();
    let main_image = main_folder.join("main.jpg");
    let detached_image = detached_folder.join("old.jpg");
    let requested = target_folder.join("requested.jpg");
    seed_images(&mut app, std::slice::from_ref(&main_image));
    let main_item_keys = grid_item_keys(&app.items);
    let main_selected = app.selected;
    let detached_image_for_context = detached_image.clone();
    app.build_active_context_for_test(Some(8801), DetachedSource::Image, move |active| {
        seed_images(active, &[detached_image_for_context]);
        active.navigation_scope = ViewerNavigationScope::DetachedPhysical;
        active.viewer_presentation = ViewerPresentation::DetachedWindow;
        active.detached_viewer_independent_active = true;
    });

    let requested_for_handler = requested.clone();
    let (tx, cancel) = app
        .with_active_viewer_context(|active| {
            active.open_similar_book_page_for_test(
                &ctx,
                "missing-detached-item-key",
                crate::similar_index::SimilarItemTarget::File(requested_for_handler),
            );
            control_required_scan(active)
        })
        .expect("detached viewer context must be active");
    tx.send(Ok(image_scan(std::slice::from_ref(&requested))))
        .unwrap();

    app.with_active_viewer_context(|active| {
        assert!(matches!(
            active.poll_detached_physical_folder_open(&ctx),
            DetachedPhysicalFolderOpenPoll::Applied
        ));
        let opened = active
            .fullscreen_idx
            .and_then(|idx| active.items.get(idx))
            .expect("detached viewer must open the required target");
        assert!(
            matches!(opened, GridItem::Image(path) if crate::folder_tree::path_eq(path, &requested))
        );
        assert!(active.fs_info_panel.locked);
        active.handle_fullscreen_close_request();
        assert!(!active.fs_info_panel.locked);
        assert!(active.folder_pane_open_pending.is_none());
    })
    .expect("detached completion must remain in the active bundle");

    assert!(!cancel.load(Ordering::Relaxed));
    assert_eq!(grid_item_keys(&app.items), main_item_keys);
    assert_eq!(app.selected, main_selected);
    assert_eq!(app.current_folder.as_deref(), Some(main_folder.as_path()));
}

#[cfg(windows)]
#[test]
fn required_scan_completion_is_applied_only_in_its_parked_viewer_context() {
    let ctx = egui::Context::default();
    let mut app = setup_app_for_test();
    let folder = app.tmp.path().join("viewer-a-current");
    let target_folder = app.tmp.path().join("viewer-a-target");
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::create_dir_all(&target_folder).unwrap();
    let target = target_folder.join("b.jpg");
    seed_images(&mut app, &[folder.join("a.jpg")]);
    begin_required_physical_scan(&mut app, &ctx, target_folder, target.clone());
    let (tx, cancel) = control_required_scan(&mut app);
    let viewer_a = app.stash_mounted_and_start_fresh("similar_navigation_required_scan_a");

    app.items = vec![GridItem::Image(PathBuf::from(r"C:\viewer-b\page.jpg"))];
    app.thumbnails = vec![ThumbnailState::Pending];
    app.image_metas = vec![None];
    app.visible_indices = vec![0];
    app.fullscreen_idx = Some(0);
    app.selected = Some(0);
    app.handle_fullscreen_close_request();
    let b_item_keys = grid_item_keys(&app.items);
    let b_selected = app.selected;
    let b_fullscreen_idx = app.fullscreen_idx;
    let b_panel = app.fs_info_panel.clone();

    tx.send(Ok(image_scan(&[target.clone()]))).unwrap();
    app.with_viewer_context(viewer_a, |mounted| {
        let ready = mounted
            .poll_folder_pane_open(&ctx)
            .expect("A's controlled scan must complete while A is mounted");
        assert!(
            mounted
                .resolve_main_folder_open_ready(&ctx, ready)
                .is_none()
        );
        let opened = mounted
            .fullscreen_idx
            .and_then(|idx| mounted.items.get(idx))
            .expect("A must open its required target");
        assert!(
            matches!(opened, GridItem::Image(path) if crate::folder_tree::path_eq(path, &target))
        );
        assert!(mounted.fs_info_panel.locked);
        assert!(mounted.folder_pane_open_pending.is_none());
    })
    .unwrap();

    assert_eq!(grid_item_keys(&app.items), b_item_keys);
    assert_eq!(app.selected, b_selected);
    assert_eq!(app.fullscreen_idx, b_fullscreen_idx);
    assert_eq!(app.fs_info_panel.locked, b_panel.locked);
    assert_eq!(app.fs_info_panel.open, b_panel.open);
    assert!(app.folder_pane_open_pending.is_none());
    assert!(!cancel.load(Ordering::Relaxed));
}
