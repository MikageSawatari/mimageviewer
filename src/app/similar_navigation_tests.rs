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

fn install_navigation_sequence_with_purpose(
    app: &mut App,
    target: FsNavigationSequenceTarget,
    purpose: FsNavigationPurpose,
) {
    app.fs_holdover_tex = Some(FsHoldover::NavigationSequence(FsNavigationSequence {
        previous: None,
        chrome: FsNavigationChromeContinuation::None,
        purpose,
        opened_at: std::time::Instant::now(),
        target,
    }));
}

fn install_navigation_sequence(app: &mut App, target: FsNavigationSequenceTarget) {
    install_navigation_sequence_with_purpose(app, target, FsNavigationPurpose::Ordinary);
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

fn install_rendition_failed_sequence(
    app: &mut App,
    pages: Vec<usize>,
    purpose: FsNavigationPurpose,
) {
    let items_generation = app.items_generation;
    let anchor_idx = *pages
        .first()
        .expect("rendition failure target has an anchor");
    install_navigation_sequence_with_purpose(
        app,
        FsNavigationSequenceTarget::Display(FsNavigationDisplayTarget {
            items_generation,
            anchor_idx,
            accept_rendition: true,
            phase: FsNavigationTargetPhase::RenditionFailed { pages },
        }),
        purpose,
    );
}

fn similar_navigation_purpose_with_trace(
    trace: SimilarMoveTrace,
    destination: PathBuf,
) -> FsNavigationPurpose {
    FsNavigationPurpose::SimilarBookVisit(SimilarBookNavigationIntent {
        origin: SimilarBookLocation::from_destination(SnapshotTarget::Fs(PathBuf::from(
            r"C:\trace\origin\001.jpg",
        )))
        .unwrap(),
        destination: SimilarBookLocation::from_destination(SnapshotTarget::Fs(destination))
            .unwrap(),
        diagnostic_trace: Some(trace),
    })
}

#[test]
fn similar_move_p2_trace_terminals_when_navigation_lock_is_released() {
    let destination = PathBuf::from(r"C:\trace\released\001.jpg");
    let trace = SimilarMoveTrace::new(
        SimilarMoveSource::HistoryButton,
        Some(&SnapshotTarget::Fs(destination.clone())),
    );
    let trace_id = trace.id;
    let mut app = setup_app_for_test();
    let accepted_generation = app.items_generation;
    install_navigation_sequence_with_purpose(
        &mut app,
        FsNavigationSequenceTarget::FolderItems {
            accepted_generation,
        },
        similar_navigation_purpose_with_trace(trace, destination),
    );
    app.fs_nav_locked_gen = Some(accepted_generation);

    app.release_fs_nav_lock();

    assert!(app.fs_holdover_tex.is_none());
    assert!(!app.fs_nav_is_locked());
    assert_eq!(
        SimilarMoveTrace::terminal_reasons_for_test(trace_id),
        vec!["navigation_released"]
    );
}

#[test]
fn similar_move_p2_trace_terminals_when_materialized_navigation_fails() {
    let destination = PathBuf::from(r"C:\trace\failed\001.jpg");
    let trace = SimilarMoveTrace::new(
        SimilarMoveSource::ItemButton,
        Some(&SnapshotTarget::Fs(destination.clone())),
    );
    let trace_id = trace.id;
    let mut app = setup_app_for_test();
    app.fullscreen_idx = Some(0);
    let items_generation = app.items_generation;
    install_navigation_sequence_with_purpose(
        &mut app,
        FsNavigationSequenceTarget::Display(FsNavigationDisplayTarget {
            items_generation,
            anchor_idx: 0,
            accept_rendition: true,
            phase: FsNavigationTargetPhase::Ready {
                pages: vec![0],
                presentation: FsNavigationPresentation::Failure,
            },
        }),
        similar_navigation_purpose_with_trace(trace, destination),
    );
    app.fs_nav_locked_gen = Some(items_generation);

    assert!(app.fs_nav_holdover_for_draw().is_none());

    assert!(app.fs_holdover_tex.is_none());
    assert!(!app.fs_nav_is_locked());
    assert_eq!(
        SimilarMoveTrace::terminal_reasons_for_test(trace_id),
        vec!["navigation_failed"]
    );
}

#[test]
fn similar_move_p2_completed_scan_terminals_when_ready_is_replaced() {
    let destination = PathBuf::from(r"C:\trace\ready-replaced\001.jpg");
    let trace = SimilarMoveTrace::new(
        SimilarMoveSource::BookButton,
        Some(&SnapshotTarget::Fs(destination.clone())),
    );
    let trace_id = trace.id;
    let mut ready = FolderPaneOpenReady {
        path: destination.parent().unwrap().to_path_buf(),
        scan: Ok(image_scan(std::slice::from_ref(&destination))),
        purpose: FolderOpenScanPurpose::RequiredFullscreenTarget {
            target: SnapshotTarget::Fs(destination.clone()),
            history_trigger: HistoryTrigger::UserChosen,
            navigation_purpose: similar_navigation_purpose_with_trace(trace, destination),
        },
    };

    ready.finish_diagnostic("ready_replaced");

    assert_eq!(
        SimilarMoveTrace::terminal_reasons_for_test(trace_id),
        vec!["ready_replaced"]
    );
    assert!(
        ready.purpose.diagnostic_trace().is_none(),
        "the discarded ready value can no longer drop a live trace"
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

fn book_query_with_physical_target(
    origin: &Path,
    target: &Path,
) -> crate::similar_index::BookQuery {
    use crate::similar_index::{
        BookOrigin, BookOriginPage, BookPageBaseline, BookPageMatch, BookPageMatchState,
        BookRelationHit, BookRelations, SimilarItemTarget,
    };

    let origin = Arc::new(BookOrigin {
        pages: vec![BookOriginPage {
            item_key: crate::similar_index::item_key_for_file(origin),
            baseline: BookPageBaseline::Unmatched,
        }]
        .into_boxed_slice(),
    });
    let target_item_key = crate::similar_index::item_key_for_file(target);
    let hit = BookRelationHit::new(
        crate::search_index_db::normalize_path(target.parent().unwrap()),
        2,
        crate::dupe::book::BookPair {
            a: 1,
            b: 2,
            matched: 1,
            distinctive_a: 1,
            distinctive_b: 1,
            coverage_a: 1.0,
            coverage_b: 1.0,
            relation: crate::dupe::book::Relation::Same,
            alignment: vec![(0, 0)],
        },
        vec![BookPageMatch {
            origin_slot: 0,
            state: BookPageMatchState::Strong,
            other_page_index: 0,
            other_target: Some(SimilarItemTarget::File(target.to_path_buf())),
            other_item_key: target_item_key,
            other_mtime: 1,
            other_file_size: 1,
        }],
        origin.pages.len(),
    )
    .unwrap();
    crate::similar_index::BookQuery::Ready(BookRelations {
        origin,
        hits: vec![hit],
    })
}

fn pointer_input(screen: egui::Rect, pos: egui::Pos2, pressed: bool) -> egui::RawInput {
    egui::RawInput {
        screen_rect: Some(screen),
        events: vec![
            egui::Event::PointerMoved(pos),
            egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: egui::Modifiers::NONE,
            },
        ],
        ..Default::default()
    }
}

#[cfg(windows)]
fn root_update_input(events: Vec<egui::Event>, time: f64) -> egui::RawInput {
    let mut input = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(1200.0, 800.0),
        )),
        time: Some(time),
        events,
        ..Default::default()
    };
    input
        .viewports
        .get_mut(&egui::ViewportId::ROOT)
        .unwrap()
        .focused = Some(true);
    input
}

#[cfg(windows)]
fn run_root_app_update(
    app: &mut App,
    ctx: &egui::Context,
    frame: &mut eframe::Frame,
    input: egui::RawInput,
) -> egui::FullOutput {
    ctx.run(input, |ctx| {
        <App as eframe::App>::update(app, ctx, frame);
    })
}

#[cfg(windows)]
fn settle_root_app_update_fixture(app: &mut App, ctx: &egui::Context, frame: &mut eframe::Frame) {
    app.startup_done = true;
    app.startup_init = None;
    crate::ui_fonts::configure_fonts(ctx);
    let _ = run_root_app_update(app, ctx, frame, root_update_input(Vec::new(), 0.0));
}

#[cfg(windows)]
fn plain_key_press(key: egui::Key) -> egui::Event {
    egui::Event::Key {
        key,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: egui::Modifiers::NONE,
    }
}

#[cfg(windows)]
struct SimilarNavigationKeyInputGuard {
    _serial: std::sync::MutexGuard<'static, ()>,
}

#[cfg(windows)]
impl Drop for SimilarNavigationKeyInputGuard {
    fn drop(&mut self) {
        crate::key_input::clear_test_frame();
    }
}

#[cfg(windows)]
fn similar_navigation_key_input_guard() -> SimilarNavigationKeyInputGuard {
    let serial = crate::key_input::TEST_INPUT_LOCK
        .get_or_init(|| std::sync::Mutex::new(()))
        .lock()
        .expect("fullscreen key-input test lock poisoned");
    crate::key_input::clear_test_frame();
    SimilarNavigationKeyInputGuard { _serial: serial }
}

#[cfg(windows)]
fn install_embedded_update_scene(app: &mut App, ctx: &egui::Context, pages: &[PathBuf]) {
    app.startup_done = true;
    app.startup_init = None;
    crate::ui_fonts::configure_fonts(ctx);
    seed_images(app, pages);
    let pixels = Arc::new(egui::ColorImage::filled([4, 6], egui::Color32::DARK_BLUE));
    let texture = ctx.load_texture(
        "embedded_similar_move_update_scene",
        Arc::clone(&pixels),
        egui::TextureOptions::LINEAR,
    );
    app.fs_cache.insert(
        0,
        FsCacheEntry::Static {
            tex: texture,
            pixels,
            source_dims: Some([4, 6]),
            load_seq: 1,
            animation: crate::fs_animation::StaticAnimationState::Still,
        },
    );
    app.native_video_in_window_active = true;
    app.settings.video_in_window_mode = true;
    app.viewer_presentation = ViewerPresentation::MainWindow;
    crate::ui_fullscreen::install_fs_navigator_input_tracking(ctx);
}

#[cfg(windows)]
#[test]
fn book_relation_move_button_reaches_the_exact_physical_target() {
    let ctx = egui::Context::default();
    let mut app = setup_app_for_test();
    let old_folder = app.tmp.path().join("__new").join("266707");
    let target_folder = app.tmp.path().join("__new5").join("266707");
    std::fs::create_dir_all(&old_folder).unwrap();
    std::fs::create_dir_all(&target_folder).unwrap();
    let old = old_folder.join("1.jpg");
    let first = target_folder.join("0.jpg");
    let requested = target_folder.join("1.jpg");
    std::fs::write(&old, b"old").unwrap();
    std::fs::write(&first, b"first").unwrap();
    std::fs::write(&requested, b"requested").unwrap();
    seed_images(&mut app, std::slice::from_ref(&old));
    let accepted_generation = app.items_generation;
    install_navigation_sequence_with_purpose(
        &mut app,
        FsNavigationSequenceTarget::Display(FsNavigationDisplayTarget {
            items_generation: accepted_generation,
            anchor_idx: 0,
            accept_rendition: true,
            phase: FsNavigationTargetPhase::Awaiting { pages: vec![0] },
        }),
        FsNavigationPurpose::Ordinary,
    );

    let query = book_query_with_physical_target(&old, &requested);
    let mut panel = crate::ui_metadata_panel::SimilarPanelState::default();
    let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(420.0, 320.0));
    assert!(
        crate::ui_metadata_panel::draw_book_move_action_for_test(
            &ctx,
            egui::RawInput {
                screen_rect: Some(screen),
                ..Default::default()
            },
            &query,
            &mut panel,
        )
        .is_none()
    );
    let button = crate::ui_metadata_panel::take_book_move_button_rect_for_test()
        .expect("production book result must expose its move button")
        .center();
    assert!(
        crate::ui_metadata_panel::draw_book_move_action_for_test(
            &ctx,
            pointer_input(screen, button, true),
            &query,
            &mut panel,
        )
        .is_none(),
        "press alone must not navigate"
    );
    let action = crate::ui_metadata_panel::draw_book_move_action_for_test(
        &ctx,
        pointer_input(screen, button, false),
        &query,
        &mut panel,
    )
    .expect("release on the same production button must emit its owned target");

    app.dispatch_similar_book_move_for_test(&ctx, action);
    let (tx, _) = control_required_scan(&mut app);
    assert!(matches!(
        app.fs_holdover_tex
            .as_ref()
            .and_then(FsHoldover::navigation_sequence)
            .map(|sequence| &sequence.purpose),
        Some(FsNavigationPurpose::Ordinary)
    ));
    tx.send(Ok(image_scan(&[first, requested.clone()])))
        .unwrap();
    let ready = app
        .poll_folder_pane_open(&ctx)
        .expect("controlled scan ready");
    assert!(app.resolve_main_folder_open_ready(&ctx, ready).is_none());

    let opened = app
        .fullscreen_idx
        .and_then(|idx| app.items.get(idx))
        .expect("requested book page opened");
    assert!(
        matches!(opened, GridItem::Image(path) if crate::folder_tree::path_eq(path, &requested))
    );
    assert!(app.bind_fs_navigation_sequence_to_current_target());
    let sequence = app
        .fs_holdover_tex
        .as_ref()
        .and_then(FsHoldover::navigation_sequence)
        .expect("the actual button route owns the destination presentation");
    assert!(matches!(
        (&sequence.purpose, &sequence.target),
        (
            FsNavigationPurpose::SimilarBookVisit(intent),
            FsNavigationSequenceTarget::Display(target)
        ) if intent.destination.page == SnapshotTarget::Fs(requested)
            && target.pages().contains(&app.fullscreen_idx.unwrap())
    ));
}

#[cfg(windows)]
#[test]
fn embedded_similar_move_update_pump_materializes_controlled_physical_scan() {
    let ctx = egui::Context::default();
    let mut frame = eframe::Frame::_new_kittest();
    let mut app = setup_app_for_test();
    settle_root_app_update_fixture(&mut app, &ctx, &mut frame);
    let origin_folder = app.tmp.path().join("embedded-update-origin");
    let target_folder = app.tmp.path().join("embedded-update-target");
    std::fs::create_dir_all(&origin_folder).unwrap();
    std::fs::create_dir_all(&target_folder).unwrap();
    let origin = origin_folder.join("1.jpg");
    let first = target_folder.join("0.jpg");
    let requested = target_folder.join("1.jpg");
    std::fs::write(&origin, b"origin").unwrap();
    std::fs::write(&first, b"first").unwrap();
    std::fs::write(&requested, b"requested").unwrap();
    install_embedded_update_scene(&mut app, &ctx, std::slice::from_ref(&origin));
    let hit = similar_file_hit(requested.clone());
    app.open_similar_hit_for_test(&ctx, &hit);
    let (controlled_tx, controlled_cancel) = control_required_scan(&mut app);
    let empty = run_root_app_update(
        &mut app,
        &ctx,
        &mut frame,
        root_update_input(Vec::new(), 0.1),
    );
    assert!(
        matches!(
            app.folder_pane_open_pending
                .as_ref()
                .map(|pending| &pending.purpose),
            Some(FolderOpenScanPurpose::RequiredFullscreenTarget { .. })
        ),
        "Empty pass unexpectedly retired its owner: cancelled={} fs_idx={:?} presentation={:?} in_window={} items={}",
        controlled_cancel.load(Ordering::Relaxed),
        app.fullscreen_idx,
        app.viewer_presentation,
        app.native_video_in_window_active,
        app.items.len()
    );
    assert_eq!(
        empty
            .viewport_output
            .get(&egui::ViewportId::ROOT)
            .map(|viewport| viewport.repaint_delay),
        Some(std::time::Duration::ZERO),
        "an empty controlled scan must keep the ROOT update pump awake"
    );

    controlled_tx
        .send(Ok(image_scan(&[first, requested.clone()])))
        .unwrap();
    let _ = run_root_app_update(
        &mut app,
        &ctx,
        &mut frame,
        root_update_input(Vec::new(), 0.2),
    );

    assert!(app.folder_pane_open_pending.is_none());
    let opened = app
        .fullscreen_idx
        .and_then(|idx| app.items.get(idx))
        .expect("embedded update pump must materialize the requested target");
    assert!(
        matches!(opened, GridItem::Image(path) if crate::folder_tree::path_eq(path, &requested))
    );
    assert!(matches!(
        app.fs_holdover_tex,
        Some(FsHoldover::NavigationSequence(_))
    ));
}

#[cfg(windows)]
#[test]
fn embedded_similar_move_update_pump_gives_same_frame_escape_priority() {
    let _input_guard = similar_navigation_key_input_guard();
    let ctx = egui::Context::default();
    let mut frame = eframe::Frame::_new_kittest();
    let mut app = setup_app_for_test();
    settle_root_app_update_fixture(&mut app, &ctx, &mut frame);
    let origin_folder = app.tmp.path().join("embedded-escape-origin");
    let target_folder = app.tmp.path().join("embedded-escape-target");
    std::fs::create_dir_all(&origin_folder).unwrap();
    std::fs::create_dir_all(&target_folder).unwrap();
    let origin = origin_folder.join("1.jpg");
    let requested = target_folder.join("1.jpg");
    std::fs::write(&origin, b"origin").unwrap();
    std::fs::write(&requested, b"requested").unwrap();
    install_embedded_update_scene(&mut app, &ctx, std::slice::from_ref(&origin));
    app.open_similar_hit_for_test(&ctx, &similar_file_hit(requested.clone()));
    let (controlled_tx, controlled_cancel) = control_required_scan(&mut app);
    controlled_tx
        .send(Ok(image_scan(std::slice::from_ref(&requested))))
        .unwrap();

    let _ = run_root_app_update(
        &mut app,
        &ctx,
        &mut frame,
        root_update_input(vec![plain_key_press(egui::Key::Escape)], 0.1),
    );

    assert!(controlled_cancel.load(Ordering::Relaxed));
    assert!(app.folder_pane_open_pending.is_none());
    assert!(app.fullscreen_idx.is_none());
    assert!(matches!(
        app.items.as_slice(),
        [GridItem::Image(path)] if crate::folder_tree::path_eq(path, &origin)
    ));
}

#[cfg(windows)]
#[test]
fn embedded_similar_move_update_pump_gives_same_frame_page_navigation_priority() {
    let _input_guard = similar_navigation_key_input_guard();
    let ctx = egui::Context::default();
    let mut frame = eframe::Frame::_new_kittest();
    let mut app = setup_app_for_test();
    settle_root_app_update_fixture(&mut app, &ctx, &mut frame);
    let origin_folder = app.tmp.path().join("embedded-page-nav-origin");
    let target_folder = app.tmp.path().join("embedded-page-nav-target");
    std::fs::create_dir_all(&origin_folder).unwrap();
    std::fs::create_dir_all(&target_folder).unwrap();
    let page0 = origin_folder.join("0.jpg");
    let page1 = origin_folder.join("1.jpg");
    let requested = target_folder.join("1.jpg");
    for path in [&page0, &page1, &requested] {
        std::fs::write(path, b"image").unwrap();
    }
    install_embedded_update_scene(&mut app, &ctx, &[page0.clone(), page1.clone()]);
    app.spread_mode = crate::settings::SpreadMode::Single;
    app.reading_flow = crate::settings::ReadingFlow::Paged;
    app.open_similar_hit_for_test(&ctx, &similar_file_hit(requested.clone()));
    let (controlled_tx, controlled_cancel) = control_required_scan(&mut app);
    controlled_tx
        .send(Ok(image_scan(std::slice::from_ref(&requested))))
        .unwrap();

    let _ = run_root_app_update(
        &mut app,
        &ctx,
        &mut frame,
        root_update_input(vec![plain_key_press(egui::Key::ArrowDown)], 0.1),
    );

    assert!(controlled_cancel.load(Ordering::Relaxed));
    assert!(app.folder_pane_open_pending.is_none());
    assert_eq!(app.fullscreen_idx, Some(1));
    assert!(matches!(
        app.items.as_slice(),
        [GridItem::Image(first), GridItem::Image(second)]
            if crate::folder_tree::path_eq(first, &page0)
                && crate::folder_tree::path_eq(second, &page1)
    ));
}

#[cfg(windows)]
#[test]
fn embedded_similar_move_update_pump_leaves_non_required_scan_for_normal_tail() {
    let ctx = egui::Context::default();
    let mut frame = eframe::Frame::_new_kittest();
    let mut app = setup_app_for_test();
    settle_root_app_update_fixture(&mut app, &ctx, &mut frame);
    let origin = app.tmp.path().join("embedded-pane-origin").join("1.jpg");
    let pane_folder = app.tmp.path().join("embedded-pane-target");
    std::fs::create_dir_all(origin.parent().unwrap()).unwrap();
    std::fs::create_dir_all(&pane_folder).unwrap();
    std::fs::write(&origin, b"origin").unwrap();
    install_embedded_update_scene(&mut app, &ctx, std::slice::from_ref(&origin));
    let (pane_tx, pane_rx) = mpsc::channel();
    let pane_cancel = Arc::new(AtomicBool::new(false));
    app.folder_pane_open_pending = Some(FolderPaneOpenPending {
        path: pane_folder,
        cancel: Arc::clone(&pane_cancel),
        rx: pane_rx,
        purpose: FolderOpenScanPurpose::PaneNavigation,
    });
    pane_tx.send(Ok(image_scan(&[]))).unwrap();

    let _ = run_root_app_update(
        &mut app,
        &ctx,
        &mut frame,
        root_update_input(Vec::new(), 0.1),
    );

    assert!(matches!(
        app.folder_pane_open_pending
            .as_ref()
            .map(|pending| &pending.purpose),
        Some(FolderOpenScanPurpose::PaneNavigation)
    ));
    assert!(!pane_cancel.load(Ordering::Relaxed));
    assert!(matches!(
        app.items.as_slice(),
        [GridItem::Image(path)] if crate::folder_tree::path_eq(path, &origin)
    ));
}

#[cfg(windows)]
#[test]
fn embedded_similar_move_update_pump_does_not_consume_a_passive_context_scan() {
    let ctx = egui::Context::default();
    let mut frame = eframe::Frame::_new_kittest();
    let mut app = setup_app_for_test();
    settle_root_app_update_fixture(&mut app, &ctx, &mut frame);
    let main_page = app.tmp.path().join("embedded-main-owner").join("1.jpg");
    let sibling_folder = app.tmp.path().join("embedded-passive-target");
    let sibling_page = sibling_folder.join("1.jpg");
    std::fs::create_dir_all(main_page.parent().unwrap()).unwrap();
    std::fs::create_dir_all(&sibling_folder).unwrap();
    std::fs::write(&main_page, b"main").unwrap();
    std::fs::write(&sibling_page, b"sibling").unwrap();
    install_embedded_update_scene(&mut app, &ctx, std::slice::from_ref(&main_page));
    let (sibling_tx, sibling_rx) = mpsc::channel();
    let sibling_cancel = Arc::new(AtomicBool::new(false));
    let sibling_cancel_for_context = Arc::clone(&sibling_cancel);
    let sibling_folder_for_context = sibling_folder.clone();
    let sibling_page_for_context = sibling_page.clone();
    let sibling = app.push_window_context_for_test(&ctx, 9901, move |context| {
        context.folder_pane_open_pending = Some(FolderPaneOpenPending {
            path: sibling_folder_for_context,
            cancel: sibling_cancel_for_context,
            rx: sibling_rx,
            purpose: FolderOpenScanPurpose::RequiredFullscreenTarget {
                target: SnapshotTarget::Fs(sibling_page_for_context),
                history_trigger: HistoryTrigger::UserChosen,
                navigation_purpose: FsNavigationPurpose::Ordinary,
            },
        });
    });
    sibling_tx
        .send(Ok(image_scan(std::slice::from_ref(&sibling_page))))
        .unwrap();

    let _ = run_root_app_update(
        &mut app,
        &ctx,
        &mut frame,
        root_update_input(Vec::new(), 0.1),
    );

    assert!(!sibling_cancel.load(Ordering::Relaxed));
    app.with_viewer_context(sibling, |context| {
        assert!(matches!(
            context
                .folder_pane_open_pending
                .as_ref()
                .map(|pending| &pending.purpose),
            Some(FolderOpenScanPurpose::RequiredFullscreenTarget { .. })
        ));
    })
    .unwrap();
    assert!(matches!(
        app.items.as_slice(),
        [GridItem::Image(path)] if crate::folder_tree::path_eq(path, &main_page)
    ));
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
    let trace = crate::app::SimilarMoveTrace::new(
        crate::app::SimilarMoveSource::ItemCard,
        Some(&SnapshotTarget::Fs(requested.clone())),
    );
    let trace_id = trace.id;
    app.open_similar_hit_with_trace_for_test(&ctx, &hit, trace);
    assert!(
        app.is_snapshot_active(),
        "scan admission must preserve the snapshot"
    );
    assert_eq!(
        app.fullscreen_idx,
        Some(0),
        "old page stays visible while scanning"
    );
    assert_eq!(
        app.folder_pane_open_pending
            .as_ref()
            .and_then(|pending| pending.purpose.diagnostic_trace())
            .map(|trace| trace.id),
        Some(trace_id),
        "the physical scan owns the same diagnostic trace"
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
    assert!(
        app.bind_fs_navigation_sequence_to_current_target(),
        "the first renderer poll binds the accepted folder to its display unit"
    );
    assert_eq!(
        app.fs_holdover_tex
            .as_ref()
            .and_then(FsHoldover::navigation_sequence)
            .and_then(|sequence| sequence.purpose.diagnostic_trace())
            .map(|trace| trace.id),
        Some(trace_id),
        "scan completion moves the trace into the typed display owner"
    );
    let presented_pages = {
        let sequence = app
            .fs_holdover_tex
            .as_mut()
            .and_then(FsHoldover::navigation_sequence_mut)
            .expect("successful scan keeps the typed presentation owner");
        let FsNavigationSequenceTarget::Display(target) = &mut sequence.target else {
            panic!("required target must bind to a display sequence");
        };
        let pages = target.pages().to_vec();
        target.phase = FsNavigationTargetPhase::Presenting {
            pages: pages.clone(),
            presentation: crate::app::FsNavigationPresentation::Rendition,
        };
        pages
    };
    app.observe_fs_navigation_pages_for_test(&presented_pages);
    assert!(
        app.fs_holdover_tex.is_none(),
        "exact all-live presentation consumes the diagnostic owner"
    );
    let (history, current) = app
        .similar_panel
        .book_history_snapshot_for_test()
        .expect("exact live target commits the visit");
    assert_eq!(history.len(), 2);
    assert_eq!(
        current,
        crate::search_index_db::normalize_path(&target_folder)
    );
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
    let current = SimilarBookLocation::from_grid_item(&app.items[0]).unwrap();
    app.similar_panel
        .complete_similar_book_visit(SimilarBookNavigationIntent {
            origin: SimilarBookLocation::from_destination(SnapshotTarget::Fs(PathBuf::from(
                r"C:\history\earlier\001.jpg",
            )))
            .unwrap(),
            destination: current.clone(),
            diagnostic_trace: None,
        });
    let history_before = app
        .similar_panel
        .book_history_snapshot_for_test()
        .expect("completed origin history");
    let target = SnapshotTarget::Fs(requested.clone());
    let navigation_purpose = FsNavigationPurpose::SimilarBookVisit(SimilarBookNavigationIntent {
        origin: current,
        destination: SimilarBookLocation::from_destination(target.clone()).unwrap(),
        diagnostic_trace: None,
    });

    app.open_required_fullscreen_location_with_purpose(
        &ctx,
        target_folder,
        target,
        HistoryTrigger::UserChosen,
        navigation_purpose,
    );
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
    assert_eq!(
        app.similar_panel.book_history_snapshot_for_test(),
        Some(history_before),
        "a failed scan cannot publish its unpresented destination"
    );
}

#[cfg(windows)]
#[test]
fn request_failure_retains_completed_history_until_presentation_or_true_close() {
    use crate::similar_book_query::BookQueryDemandSnapshot;

    let mut app = viewer_with_images(&[PathBuf::from(r"C:\history\b\010.jpg")]);
    let origin = SimilarBookLocation::from_destination(SnapshotTarget::Fs(PathBuf::from(
        r"C:\history\a\001.jpg",
    )))
    .unwrap();
    let destination = SimilarBookLocation::from_grid_item(&app.items[0]).unwrap();
    app.similar_panel
        .complete_similar_book_visit(SimilarBookNavigationIntent {
            origin: origin.clone(),
            destination: destination.clone(),
            diagnostic_trace: None,
        });
    let retained = app
        .similar_panel
        .book_history_snapshot_for_test()
        .expect("completed history");
    let current_item = app.items[0].clone();
    let _ = app.query_similar_book(&current_item);
    assert!(matches!(
        app.similar_panel.book_query_demand_for_test(),
        BookQueryDemandSnapshot::Active(_)
    ));
    let accepted_generation = app.items_generation;
    install_navigation_sequence_with_purpose(
        &mut app,
        FsNavigationSequenceTarget::FolderItems {
            accepted_generation,
        },
        FsNavigationPurpose::SimilarBookVisit(SimilarBookNavigationIntent {
            origin: destination,
            destination: SimilarBookLocation::from_destination(SnapshotTarget::Fs(PathBuf::from(
                r"C:\history\pending\099.jpg",
            )))
            .unwrap(),
            diagnostic_trace: None,
        }),
    );

    app.finish_fs_navigation_sequence(FsNavigationSequenceFinish::RequestFailed);

    assert!(app.fs_holdover_tex.is_none(), "failed intent is retired");
    assert_eq!(
        app.similar_panel.book_history_snapshot_for_test(),
        Some(retained.clone()),
        "only completed visits remain"
    );
    assert_eq!(
        app.similar_panel.book_query_demand_for_test(),
        BookQueryDemandSnapshot::Withdrawn
    );
    assert!(
        !app.fs_info_panel.locked,
        "failed viewer owner exits the panel"
    );

    app.fullscreen_idx = None;
    app.close_fullscreen_now();
    assert_eq!(
        app.similar_panel.book_history_snapshot_for_test(),
        Some(retained),
        "generic no-viewer cleanup must not become an explicit session close"
    );

    seed_images(&mut app, &[PathBuf::from(r"C:\history\b\011.jpg")]);
    app.observe_fs_navigation_pages_for_test(&[0]);
    let (entries, current) = app
        .similar_panel
        .book_history_snapshot_for_test()
        .expect("same book can resume the completed session");
    assert_eq!(current, "c:/history/b");
    assert_eq!(
        entries[1].page,
        SnapshotTarget::Fs(PathBuf::from(r"C:\history\b\011.jpg"))
    );

    seed_images(&mut app, &[PathBuf::from(r"C:\history\unrelated\001.jpg")]);
    app.observe_fs_navigation_pages_for_test(&[0]);
    assert!(
        app.similar_panel.book_history_snapshot_for_test().is_none(),
        "the first live page in another book starts a different session"
    );

    let unrelated = SimilarBookLocation::from_grid_item(&app.items[0]).unwrap();
    app.similar_panel
        .complete_similar_book_visit(SimilarBookNavigationIntent {
            origin: origin.clone(),
            destination: unrelated,
            diagnostic_trace: None,
        });
    app.handle_fullscreen_close_request();
    assert!(
        app.similar_panel.book_history_snapshot_for_test().is_none(),
        "an explicit close clears history without a pending sequence"
    );

    seed_images(&mut app, &[PathBuf::from(r"C:\history\b\012.jpg")]);
    let reopened = SimilarBookLocation::from_grid_item(&app.items[0]).unwrap();
    app.similar_panel
        .complete_similar_book_visit(SimilarBookNavigationIntent {
            origin,
            destination: reopened,
            diagnostic_trace: None,
        });
    install_folder_items_sequence(&mut app);
    app.handle_fullscreen_close_request();
    assert!(
        app.similar_panel.book_history_snapshot_for_test().is_none(),
        "an explicit close clears history with a pending sequence"
    );
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
            navigation_purpose: FsNavigationPurpose::Ordinary,
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
            navigation_purpose: FsNavigationPurpose::Ordinary,
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
fn similar_move_p2_rendition_failed_does_not_block_and_supersedes_old_trace() {
    let ctx = egui::Context::default();
    let mut app = viewer_with_images(&[
        PathBuf::from(r"C:\snapshot\a.jpg"),
        PathBuf::from(r"C:\snapshot\b.jpg"),
    ]);
    app.activate_snapshot(SnapshotSourceLabel::Mixed);
    app.fullscreen_idx = Some(0);
    let old_target = SnapshotTarget::Fs(PathBuf::from(r"C:\snapshot\a.jpg"));
    let old_trace = SimilarMoveTrace::new(SimilarMoveSource::ItemButton, Some(&old_target));
    let old_trace_id = old_trace.id;
    install_rendition_failed_sequence(
        &mut app,
        vec![0],
        similar_navigation_purpose_with_trace(old_trace, PathBuf::from(r"C:\snapshot\a.jpg")),
    );
    app.fs_nav_locked_gen = None;

    app.handle_fullscreen_ctrl_nav_context(&ctx, 0, true, false);

    assert_eq!(app.fullscreen_idx, Some(1));
    assert!(app.fs_info_panel.locked);
    assert_eq!(
        SimilarMoveTrace::terminal_reasons_for_test(old_trace_id),
        vec!["navigation_superseded"]
    );
}

#[test]
fn similar_move_p2_direct_fullscreen_open_supersedes_stale_display_trace() {
    let ctx = egui::Context::default();
    let mut app = viewer_with_images(&[
        PathBuf::from(r"C:\direct\a.jpg"),
        PathBuf::from(r"C:\direct\b.jpg"),
    ]);
    let old_target = SnapshotTarget::Fs(PathBuf::from(r"C:\direct\a.jpg"));
    let old_trace = SimilarMoveTrace::new(SimilarMoveSource::ItemCard, Some(&old_target));
    let old_trace_id = old_trace.id;
    install_rendition_failed_sequence(
        &mut app,
        vec![0],
        similar_navigation_purpose_with_trace(old_trace, PathBuf::from(r"C:\direct\a.jpg")),
    );

    app.open_fullscreen_from_fs_navigation(&ctx, 1, HistoryTrigger::UserChosen);

    assert_eq!(app.fullscreen_idx, Some(1));
    assert_eq!(
        SimilarMoveTrace::terminal_reasons_for_test(old_trace_id),
        vec!["navigation_superseded"]
    );
}

#[test]
fn similar_move_p2_direct_open_of_partner_page_supersedes_anchor_owned_trace() {
    let ctx = egui::Context::default();
    let mut app = viewer_with_images(&[
        PathBuf::from(r"C:\partner\anchor.jpg"),
        PathBuf::from(r"C:\partner\spread-partner.jpg"),
    ]);
    let old_target = SnapshotTarget::Fs(PathBuf::from(r"C:\partner\anchor.jpg"));
    let old_trace = SimilarMoveTrace::new(SimilarMoveSource::ItemCard, Some(&old_target));
    let old_trace_id = old_trace.id;
    install_rendition_failed_sequence(
        &mut app,
        vec![0, 1],
        similar_navigation_purpose_with_trace(old_trace, PathBuf::from(r"C:\partner\anchor.jpg")),
    );

    app.open_fullscreen_from_fs_navigation(&ctx, 1, HistoryTrigger::UserChosen);

    assert_eq!(app.fullscreen_idx, Some(1));
    assert_eq!(
        SimilarMoveTrace::terminal_reasons_for_test(old_trace_id),
        vec!["navigation_superseded"]
    );
}

#[test]
fn similar_move_p2_display_target_cancel_boundary_marks_trace_superseded_once() {
    let mut app = viewer_with_images(&[
        PathBuf::from(r"C:\cancel-boundary\anchor.jpg"),
        PathBuf::from(r"C:\cancel-boundary\replacement.jpg"),
    ]);
    let old_target = SnapshotTarget::Fs(PathBuf::from(r"C:\cancel-boundary\anchor.jpg"));
    let old_trace = SimilarMoveTrace::new(SimilarMoveSource::ItemButton, Some(&old_target));
    let old_trace_id = old_trace.id;
    install_rendition_failed_sequence(
        &mut app,
        vec![0],
        similar_navigation_purpose_with_trace(
            old_trace,
            PathBuf::from(r"C:\cancel-boundary\anchor.jpg"),
        ),
    );

    app.cancel_superseded_fs_navigation_display_target(1);

    assert!(app.fs_holdover_tex.is_none());
    assert_eq!(
        SimilarMoveTrace::terminal_reasons_for_test(old_trace_id),
        vec!["navigation_superseded"]
    );
}

#[test]
fn similar_move_p2_continuous_reanchor_supersedes_stale_display_trace() {
    let ctx = egui::Context::default();
    let mut app = viewer_with_images(&[
        PathBuf::from(r"C:\reanchor\a.jpg"),
        PathBuf::from(r"C:\reanchor\b.jpg"),
    ]);
    let old_target = SnapshotTarget::Fs(PathBuf::from(r"C:\reanchor\a.jpg"));
    let old_trace = SimilarMoveTrace::new(SimilarMoveSource::HistoryButton, Some(&old_target));
    let old_trace_id = old_trace.id;
    install_rendition_failed_sequence(
        &mut app,
        vec![0],
        similar_navigation_purpose_with_trace(old_trace, PathBuf::from(r"C:\reanchor\a.jpg")),
    );

    app.reanchor_continuous_reading_viewer(
        &ctx,
        &[0.0, 100.0],
        1,
        1,
        crate::page_split::PageSlice::Full,
        HistoryTrigger::UserChosen,
    );

    assert_eq!(app.fullscreen_idx, Some(1));
    assert_eq!(
        SimilarMoveTrace::terminal_reasons_for_test(old_trace_id),
        vec!["navigation_superseded"]
    );
}

#[test]
fn similar_move_p2_target_aware_open_preserves_the_new_display_trace() {
    let ctx = egui::Context::default();
    let mut app = viewer_with_images(&[
        PathBuf::from(r"C:\same-target\a.jpg"),
        PathBuf::from(r"C:\same-target\b.jpg"),
    ]);
    let target = SnapshotTarget::Fs(PathBuf::from(r"C:\same-target\b.jpg"));
    let trace = SimilarMoveTrace::new(SimilarMoveSource::BookButton, Some(&target));
    let trace_id = trace.id;
    assert!(app.begin_similar_book_page_navigation_sequence(
        &ctx,
        0,
        1,
        similar_navigation_purpose_with_trace(trace, PathBuf::from(r"C:\same-target\b.jpg")),
    ));

    app.open_fullscreen_from_fs_navigation(&ctx, 1, HistoryTrigger::UserChosen);

    assert_eq!(app.fullscreen_idx, Some(1));
    assert_eq!(
        app.fs_holdover_tex
            .as_ref()
            .and_then(FsHoldover::navigation_sequence)
            .and_then(|sequence| sequence.purpose.diagnostic_trace())
            .map(|trace| trace.id),
        Some(trace_id)
    );
    assert!(SimilarMoveTrace::terminal_reasons_for_test(trace_id).is_empty());
    app.release_fs_nav_lock();
}

#[test]
fn similar_move_p2_same_index_in_a_new_generation_supersedes_the_old_trace() {
    let ctx = egui::Context::default();
    let mut app = viewer_with_images(&[PathBuf::from(r"C:\generation\a.jpg")]);
    let target = SnapshotTarget::Fs(PathBuf::from(r"C:\generation\a.jpg"));
    let trace = SimilarMoveTrace::new(SimilarMoveSource::ItemButton, Some(&target));
    let trace_id = trace.id;
    install_rendition_failed_sequence(
        &mut app,
        vec![0],
        similar_navigation_purpose_with_trace(trace, PathBuf::from(r"C:\generation\a.jpg")),
    );
    app.items_generation = app.items_generation.wrapping_add(1);

    app.open_fullscreen_from_fs_navigation(&ctx, 0, HistoryTrigger::UserChosen);

    assert_eq!(app.fullscreen_idx, Some(0));
    assert_eq!(
        SimilarMoveTrace::terminal_reasons_for_test(trace_id),
        vec!["navigation_superseded"]
    );
}

#[test]
fn similar_move_p2_folder_items_setup_survives_the_reopen_accept_boundary() {
    let ctx = egui::Context::default();
    let mut app = viewer_with_images(&[PathBuf::from(r"C:\folder-setup\a.jpg")]);
    let target = SnapshotTarget::Fs(PathBuf::from(r"C:\folder-setup\a.jpg"));
    let trace = SimilarMoveTrace::new(SimilarMoveSource::HistoryButton, Some(&target));
    let trace_id = trace.id;
    let accepted_generation = app.items_generation;
    install_navigation_sequence_with_purpose(
        &mut app,
        FsNavigationSequenceTarget::FolderItems {
            accepted_generation,
        },
        similar_navigation_purpose_with_trace(trace, PathBuf::from(r"C:\folder-setup\a.jpg")),
    );

    app.open_fullscreen_from_fs_navigation(&ctx, 0, HistoryTrigger::UserChosen);

    assert_eq!(
        app.fs_holdover_tex
            .as_ref()
            .and_then(FsHoldover::navigation_sequence)
            .and_then(|sequence| sequence.purpose.diagnostic_trace())
            .map(|trace| trace.id),
        Some(trace_id)
    );
    assert!(SimilarMoveTrace::terminal_reasons_for_test(trace_id).is_empty());
    app.release_fs_nav_lock();
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
