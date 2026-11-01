//! Independent readers retire without changing main; main/linked retain their own returns.
#![cfg(windows)]
use super::*;
use crate::bookmark_browser::{BookmarkBrowserRow, BookmarkViewReturnTarget};

fn app() -> AppTestEnvForTest {
    let mut env = setup_app_for_test();
    env.initialized = true;
    env.indexer_init = IndexerInit::Unavailable;
    env.settings.update_check_enabled = false;
    env.settings.minimize_to_tray_on_close = false;
    env.last_main_focused = true;
    env
}

fn row(root: &Path, id: i64) -> BookmarkBrowserRow {
    let path = root.join(format!("row-{id}.mp4"));
    BookmarkBrowserRow {
        source: crate::bookmark_browser::BookmarkRowSource::Media {
            id,
            path: path.clone(),
            pts_secs: id as f64,
            title: None,
            is_audio: false,
        },
        item: GridItem::Video(path),
        relative_page_provenance: None,
        image_meta: None,
        marker_thumbnail: None,
        created_at_ms: id,
        missing: false,
    }
}

fn main_bookmarks(env: &mut AppTestEnvForTest) {
    let root = env.tmp.path().to_path_buf();
    let rows: Vec<_> = (1..=30).map(|id| row(&root, id)).collect();
    env.items = rows.iter().map(|row| row.item.clone()).collect();
    env.thumbnails = (0..rows.len()).map(|_| ThumbnailState::Pending).collect();
    env.image_metas = vec![None; rows.len()];
    env.visible_indices = (0..rows.len()).collect();
    env.bookmark_browser_rows = rows;
    env.items_are_bookmark_view = true;
    env.current_folder = Some(bookmark_view_synthetic_path());
    env.selected = Some(1);
    env.grid_scroll.set_position(240.0);
    env.bookmark_open_request_seq = 41;
    env.bookmark_view_state = None;
}

fn ready_read_model(
    app: &mut App,
) -> mpsc::Sender<Result<crate::bookmark_browser::BookmarkBrowserBuildResult, String>> {
    let rows = app.bookmark_browser_rows.clone();
    ready_read_model_with_rows(app, rows)
}

fn ready_read_model_with_rows(
    app: &mut App,
    rows: Vec<BookmarkBrowserRow>,
) -> mpsc::Sender<Result<crate::bookmark_browser::BookmarkBrowserBuildResult, String>> {
    let (tx, rx) = mpsc::channel();
    tx.send(Ok(crate::bookmark_browser::BookmarkBrowserBuildResult {
        registered_total: rows.len(),
        rows,
        video_thumb_overrides: app.video_thumb_overrides.clone(),
        visibility_notice: None,
    }))
    .unwrap();
    app.bookmark_browser_pending = Some(Box::new(BookmarkBrowserBuild {
        source: app.smart_folder_source_lease().unwrap(),
        switch_sequence: app.quick_folder_switch_sequence,
        worker: crate::bookmark_browser::BookmarkBrowserPending {
            cancel: Arc::new(AtomicBool::new(false)),
            rx,
        },
    }));
    tx
}

fn active(
    env: &mut App,
    window: u64,
    target: BookmarkViewReturnTarget,
    moved: Option<PathBuf>,
) -> (ViewerContextId, Arc<AtomicBool>) {
    let cancel = Arc::new(AtomicBool::new(false));
    let flag = cancel.clone();
    let id = env.build_active_context_for_test(Some(window), DetachedSource::Book, move |viewer| {
        viewer.navigation_scope = ViewerNavigationScope::DetachedPhysical;
        viewer.viewer_presentation = ViewerPresentation::DetachedWindow;
        viewer.detached_viewer_independent_active = true;
        let path = moved.clone().unwrap_or_else(|| match &target {
            BookmarkViewReturnTarget::Media(path) | BookmarkViewReturnTarget::Book(path) => {
                path.clone()
            }
        });
        viewer.current_folder = Some(path.parent().unwrap().to_path_buf());
        viewer.items = vec![GridItem::Video(path)];
        viewer.thumbnails = vec![ThumbnailState::Pending];
        viewer.image_metas = vec![None];
        viewer.visible_indices = vec![0];
        viewer.selected = Some(0);
        viewer.fullscreen_idx = Some(0);
        viewer.bookmark_view_state = moved
            .is_none()
            .then_some(BookmarkViewState::Detached { target });
        let (_tx, rx) = mpsc::channel();
        viewer.fs_pending.insert(
            0,
            FsPendingValue::new(flag, rx, 0, FsLoadPurpose::for_page(true)),
        );
    });
    env.update_detached_window_runtime_flags(window, false, "close_return_fixture");
    (id, cancel)
}

fn register(env: &mut App, ctx: &egui::Context, window: u64) -> egui::ViewportId {
    ctx.set_embed_viewports(false);
    let id = App::detached_image_window_viewport_id(window);
    ctx.run_ui(Default::default(), |ui| {
        ctx.show_viewport_deferred(id, egui::ViewportBuilder::default(), |_, _| {});
        env.capture_detached_viewport_registration(ui.ctx(), window);
    })
    .drop_without_applying_deltas();
    id
}

/// EpiIntegration's headless adapter has no native child request loop. Invoke the exact App
/// callback from its real LogicOnly transaction, then run production App::logic. No UI runs.
struct LogicClose<'a> {
    app: &'a mut App,
    viewport: Option<egui::ViewportId>,
    response: Option<eframe::ViewportCloseResponse>,
}
impl eframe::App for LogicClose<'_> {
    fn logic(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        if let Some(viewport) = self.viewport.take() {
            self.response = Some(eframe::App::on_viewport_close_requested(
                self.app, ctx, viewport,
            ));
        }
        eframe::App::logic(self.app, ctx, frame);
    }
    fn ui(&mut self, _: &mut egui::Ui, _: &mut eframe::Frame) {
        panic!("non-drawing Close must never invoke UI");
    }
}

fn close_logic(
    backend: &mut eframe::HeadlessEpiIntegration,
    env: &mut App,
    viewport: egui::ViewportId,
) -> eframe::ViewportCloseResponse {
    let mut wrapper = LogicClose {
        app: env,
        viewport: Some(viewport),
        response: None,
    };
    let (output, _) = backend.update_logic_only(&mut wrapper, Default::default());
    for command in output.viewport_commands.values().flatten() {
        if let egui::ViewportCommand::RetireApproved(approval) = command {
            backend.context().retire_approved_viewports(approval);
        }
    }
    backend
        .context()
        .take_viewport_retirement_texture_delta()
        .clear();
    wrapper.response.unwrap()
}

fn full(backend: &mut eframe::HeadlessEpiIntegration, env: &mut App) {
    let output = backend.update(
        env,
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(960.0, 640.0),
            )),
            ..Default::default()
        },
    );
    assert!(output.viewport_output.contains_key(&egui::ViewportId::ROOT));
    output.drop_without_applying_deltas();
}

fn register_standalone(env: &mut App, ctx: &egui::Context) -> egui::ViewportId {
    ctx.set_embed_viewports(false);
    let id = env.fullscreen_viewport_id();
    ctx.run_ui(Default::default(), |_| {
        ctx.show_viewport_deferred(id, egui::ViewportBuilder::default(), |_, _| {});
    })
    .drop_without_applying_deltas();
    ctx.run_ui(
        egui::RawInput {
            viewport_id: id,
            viewports: [
                (egui::ViewportId::ROOT, egui::ViewportInfo::default()),
                (
                    id,
                    egui::ViewportInfo {
                        parent: Some(egui::ViewportId::ROOT),
                        ..Default::default()
                    },
                ),
            ]
            .into_iter()
            .collect(),
            ..Default::default()
        },
        |ui| env.capture_current_detached_viewport_registration(ui.ctx()),
    )
    .drop_without_applying_deltas();
    env.fs_viewport_shown = true;
    env.fs_viewport_presentation = Some(ViewerPresentation::Fullscreen);
    id
}

#[test]
fn close_return_standalone_bookmark_navigation_survives_viewer_teardown() {
    let mut env = app();
    let ctx = egui::Context::default();
    let container = env.tmp.path().join("standalone.pdf");
    env.current_folder = Some(container.clone());
    env.items = vec![GridItem::Image(env.tmp.path().join("page.png"))];
    env.thumbnails = vec![ThumbnailState::Pending];
    env.image_metas = vec![None];
    env.visible_indices = vec![0];
    env.fullscreen_idx = Some(0);
    env.bookmark_open_request_seq = 61;
    env.bookmark_view_state = Some(BookmarkViewState::Opening {
        target: BookmarkViewReturnTarget::Book(container),
        grid: BookmarkViewReturnGridState {
            row_keys: vec![(1, 4)],
            selected_key: Some((1, 4)),
            opened_key: Some((1, 4)),
            scroll_offset_y: 123.0,
        },
    });
    let viewport = register_standalone(&mut env, &ctx);
    let mut backend = eframe::HeadlessEpiIntegration::new(ctx);
    assert_eq!(
        close_logic(&mut backend, &mut env, viewport),
        eframe::ViewportCloseResponse::RetirementTransferred
    );
    assert!(env.fullscreen_idx.is_none());
    assert!(!env.fs_viewport_shown);
    assert!(env.fullscreen_viewport_binding.is_none());
    assert!(env.bookmark_view_state.is_none());
    assert!(matches!(
        env.pending_return_to_parent,
        FullscreenCloseContinuation::Terminal(_)
    ));
    assert!(matches!(
        env.take_pending_return_to_parent_nav(),
        Some(crate::ui_main::AddressBarNav::Bookmarks)
    ));
    let Some(BookmarkViewState::Opening { grid, .. }) = env.bookmark_view_state.as_ref() else {
        panic!("the existing Bookmarks dispatcher receives the retained grid");
    };
    assert_eq!(grid.opened_key, Some((1, 4)));
    assert_eq!(grid.scroll_offset_y, 123.0);
    assert!(env.take_pending_return_to_parent_nav().is_none());
}

#[test]
fn close_return_auto_open_zip_pdf_image_folder_and_converted_source_preserve_parent_selection() {
    for kind in ["zip", "pdf", "image-folder", "converted"] {
        let mut env = app();
        let ctx = egui::Context::default();
        let logical = env.tmp.path().join("books").join(match kind {
            "zip" => "volume.zip",
            "pdf" => "volume.pdf",
            "image-folder" => "volume",
            _ => "volume.7z",
        });
        let physical = if kind == "converted" {
            env.tmp.path().join("cache").join("internal.zip")
        } else {
            logical.clone()
        };
        env.current_folder = Some(physical);
        env.archive_source_override = (kind == "converted").then(|| logical.clone());
        env.settings.auto_fullscreen_zip_pdf = true;
        env.settings.auto_fullscreen_image_folders = true;
        env.items = vec![GridItem::Image(env.tmp.path().join("page.png"))];
        env.thumbnails = vec![ThumbnailState::Pending];
        env.image_metas = vec![None];
        env.visible_indices = vec![0];
        env.fullscreen_idx = Some(0);
        assert!(env.auto_open_for_current_container(), "{kind}");
        let viewport = register_standalone(&mut env, &ctx);
        let mut backend = eframe::HeadlessEpiIntegration::new(ctx);
        assert_eq!(
            close_logic(&mut backend, &mut env, viewport),
            eframe::ViewportCloseResponse::RetirementTransferred
        );
        assert!(env.fullscreen_idx.is_none());
        assert!(
            matches!(
                env.pending_return_to_parent,
                FullscreenCloseContinuation::Terminal(_)
            ),
            "{kind}"
        );
        let nav = env.take_pending_return_to_parent_nav();
        assert!(
            matches!(nav, Some(crate::ui_main::AddressBarNav::Direct(ref folder, _)) if Some(folder.as_path()) == logical.parent()),
            "{kind}"
        );
        assert_eq!(
            env.select_after_load.as_deref(),
            logical.file_name().and_then(|name| name.to_str()),
            "{kind}"
        );
        assert!(env.take_pending_return_to_parent_nav().is_none());
    }
}

/// Compare the complete main list contract, including state that path/items equality misses.
#[derive(Debug, PartialEq)]
struct MainSnapshot {
    items: Vec<GridItem>,
    visible: Vec<usize>,
    selected: Option<usize>,
    scroll: f32,
    folder: Option<PathBuf>,
    generation: u64,
    surface: String,
    bookmarks: String,
    bookmark_rows: Vec<(u8, i64)>,
    history: FolderNavHistorySnapshot,
    continuation: String,
    select_after_load: Option<String>,
    bookmark_build_pending: bool,
}

fn main_snapshot(app: &App) -> MainSnapshot {
    assert_eq!(app.projected_viewer_context_id(), app.viewer_context_main());
    MainSnapshot {
        items: app.items.clone(),
        visible: app.visible_indices.clone(),
        selected: app.selected,
        scroll: app.grid_scroll.offset(),
        folder: app.current_folder.clone(),
        generation: app.items_generation,
        surface: format!("{:?}", app.top_level_grid_view.surface()),
        bookmarks: format!("{:?}", app.bookmark_view_state),
        bookmark_rows: app
            .bookmark_browser_rows
            .iter()
            .map(BookmarkBrowserRow::stable_key)
            .collect(),
        history: app.folder_nav_history_snapshot(),
        continuation: format!("{:?}", app.pending_return_to_parent),
        select_after_load: app.select_after_load.clone(),
        bookmark_build_pending: app.bookmark_browser_pending.is_some(),
    }
}

#[test]
fn close_return_independent_logic_only_book_and_media_preserve_main() {
    for book in [false, true] {
        let mut env = app();
        let ctx = egui::Context::default();
        let path = env
            .tmp
            .path()
            .join(if book { "book.pdf" } else { "target.mp4" });
        let target = if book {
            BookmarkViewReturnTarget::Book(path)
        } else {
            BookmarkViewReturnTarget::Media(path)
        };
        main_bookmarks(&mut env);
        let (id, cancel) = active(&mut env, 94_001, target, None);
        let viewport = register(&mut env, &ctx, 94_001);
        let dirty = env.tmp.path().join("dirty");
        env.folder_thumb_pin_dirty.insert(dirty.clone());
        env.video_thumb_overrides_dirty_paths.insert(dirty.clone());
        let before = main_snapshot(&env);
        let frame = env.frame_counter;
        let mut backend = eframe::HeadlessEpiIntegration::new(ctx.clone());
        assert_eq!(
            close_logic(&mut backend, &mut env, viewport),
            eframe::ViewportCloseResponse::RetirementTransferred
        );
        assert!(cancel.load(Ordering::Relaxed));
        assert!(env.with_viewer_context_ref(id, |_| ()).is_none());
        assert!(env.active_detached_session.is_none());
        assert!(env.locate_window_context(94_001).is_none());
        assert!(ctx.viewport_registration(viewport).is_none());
        for _ in 0..3 {
            let _ = backend.update_logic_only(&mut env.app, Default::default());
            assert_eq!(main_snapshot(&env), before);
            assert_eq!(env.frame_counter, frame);
            assert!(env.folder_thumb_pin_dirty.contains(&dirty));
            assert!(env.video_thumb_overrides_dirty_paths.contains(&dirty));
        }
        assert!(env.take_pending_return_to_parent_nav().is_none());
        assert_eq!(main_snapshot(&env), before);
    }
}

#[test]
fn close_return_independent_book_and_media_departure_keep_main_grid() {
    for book in [false, true] {
        let mut env = app();
        let original = env
            .tmp
            .path()
            .join(if book { "origin.pdf" } else { "original.mp4" });
        let target = if book {
            BookmarkViewReturnTarget::Book(original)
        } else {
            BookmarkViewReturnTarget::Media(original)
        };
        main_bookmarks(&mut env);
        let final_media = env.tmp.path().join("last-folder").join("last.mp4");
        let (owner, _) = active(&mut env, 94_002, target.clone(), Some(final_media));
        if book {
            env.with_viewer_context_state(owner, |viewer| {
                viewer.bookmark_view_state = Some(BookmarkViewState::Detached { target })
            })
            .unwrap();
        }
        let before = main_snapshot(&env);
        let ctx = egui::Context::default();
        let viewport = register(&mut env, &ctx, 94_002);
        let mut backend = eframe::HeadlessEpiIntegration::new(ctx);
        assert_eq!(
            close_logic(&mut backend, &mut env, viewport),
            eframe::ViewportCloseResponse::RetirementTransferred
        );
        assert_eq!(main_snapshot(&env), before);
        assert!(env.take_pending_return_to_parent_nav().is_none());
        assert_eq!(main_snapshot(&env), before);
    }
}

#[test]
fn close_return_independent_unpresented_media_has_no_main_return_authority() {
    for sibling_preparing in [false, true] {
        let mut env = app();
        let target = BookmarkViewReturnTarget::Media(env.tmp.path().join("original.mp4"));
        main_bookmarks(&mut env);
        let (owner, _) = active(&mut env, 94_011, target, None);
        env.with_viewer_context_state(owner, |viewer| {
            viewer.items.clear();
            viewer.selected = None;
            viewer.fullscreen_idx = None;
        })
        .unwrap();
        if sibling_preparing {
            let target = BookmarkViewReturnTarget::Media(env.tmp.path().join("sibling.mp4"));
            active(&mut env, 94_012, target, None);
        } else {
            env.active_detached_session.as_mut().unwrap().content_phase =
                DetachedSessionContentPhase::Ready;
        }
        let before = main_snapshot(&env);
        env.transfer_terminal_close_return(owner, ViewerCloseCause::User);
        assert_eq!(main_snapshot(&env), before);
    }
}

#[test]
fn close_return_independent_close_allows_explicit_bookmark_refresh_without_restoration() {
    for change in ["unchanged", "rename", "add", "remove", "empty"] {
        let mut control = None;
        // Each environment owns the shared data-dir test lock. Finish/drop the control
        // before creating the reader case, then compare main's own ordinary refresh.
        for close_reader in [false, true] {
            let mut env = app();
            let target = BookmarkViewReturnTarget::Book(env.tmp.path().join("book.pdf"));
            main_bookmarks(&mut env);
            env.bookmark_view_sort = crate::bookmark_browser::BookmarkViewSort::CreatedAtAsc;
            let ctx = egui::Context::default();
            if close_reader {
                active(&mut env, 94_003, target, None);
                let viewport = register(&mut env, &ctx, 94_003);
                let mut backend = eframe::HeadlessEpiIntegration::new(ctx.clone());
                let before = main_snapshot(&env);
                close_logic(&mut backend, &mut env, viewport);
                assert_eq!(main_snapshot(&env), before);
            }
            env.selected = Some(9);
            env.grid_scroll.set_position(420.0);
            let selected_key = env.bookmark_browser_rows[9].stable_key();
            let mut rows = env.bookmark_browser_rows.clone();
            match change {
                "rename" => {
                    let crate::bookmark_browser::BookmarkRowSource::Media { title, .. } =
                        &mut rows[9].source
                    else {
                        unreachable!()
                    };
                    *title = Some("explicit refresh revision".into());
                }
                "add" => rows.push(row(env.tmp.path(), 31)),
                "remove" => {
                    rows.remove(0);
                }
                "empty" => rows.clear(),
                _ => {}
            }
            let tx = if change == "unchanged" {
                ready_read_model(&mut env)
            } else {
                ready_read_model_with_rows(&mut env, rows)
            };
            ctx.run_ui(Default::default(), |ui| env.poll_bookmark_browser(ui.ctx()))
                .drop_without_applying_deltas();
            assert!(tx.send(Err("already consumed".into())).is_err());
            env.finish_page_identity_preparation_for_test();
            let selected = env
                .selected
                .map(|idx| env.bookmark_browser_rows[idx].stable_key());
            assert_eq!(selected, (change != "empty").then_some(selected_key));
            let outcome = (
                env.grid_scroll.offset(),
                selected,
                env.bookmark_browser_rows
                    .iter()
                    .map(BookmarkBrowserRow::stable_key)
                    .collect::<Vec<_>>(),
                env.selected,
                env.grid_scroll.reveal_requested(),
            );
            if close_reader {
                assert_eq!(
                    Some(outcome),
                    control,
                    "{change}: reader close must not change the subsequent main refresh"
                );
            } else {
                control = Some(outcome);
            }
            assert!(env.bookmark_view_state.is_none());
            assert!(!env.pending_return_to_parent.is_pending());
            assert!(env.take_pending_return_to_parent_nav().is_none());
        }
    }
}

#[test]
fn close_return_independent_close_does_not_interfere_with_bookmark_delete_receipt() {
    let mut control = None;
    for close_reader in [false, true] {
        let mut env = app();
        let target = BookmarkViewReturnTarget::Media(env.tmp.path().join("target.mp4"));
        main_bookmarks(&mut env);
        env.bookmark_view_sort = crate::bookmark_browser::BookmarkViewSort::CreatedAtAsc;
        let ctx = egui::Context::default();
        if close_reader {
            active(&mut env, 94_004, target, None);
            let viewport = register(&mut env, &ctx, 94_004);
            let mut backend = eframe::HeadlessEpiIntegration::new(ctx.clone());
            let before = main_snapshot(&env);
            close_logic(&mut backend, &mut env, viewport);
            assert_eq!(main_snapshot(&env), before);
        }
        env.selected = Some(9);
        env.grid_scroll.set_position(420.0);
        let key = env.bookmark_browser_rows[9].stable_key();
        let (tx, rx) = mpsc::channel();
        tx.send(Ok(())).unwrap();
        env.bookmark_delete_pending = Some(crate::bookmark_browser::BookmarkDeletePending {
            keys: vec![(0, 1)],
            book_containers: vec![],
            rx,
        });
        ctx.run_ui(Default::default(), |ui| env.poll_bookmark_browser(ui.ctx()))
            .drop_without_applying_deltas();
        assert!(env.bookmark_delete_pending.is_none());
        assert_eq!(env.bookmark_browser_rows.len(), 29);
        assert_eq!(env.selected, Some(8));
        assert_eq!(env.bookmark_browser_rows[8].stable_key(), key);
        let outcome = (
            env.grid_scroll.offset(),
            env.selected
                .map(|idx| env.bookmark_browser_rows[idx].stable_key()),
            env.bookmark_browser_rows
                .iter()
                .map(BookmarkBrowserRow::stable_key)
                .collect::<Vec<_>>(),
            env.selected,
            env.grid_scroll.reveal_requested(),
        );
        if close_reader {
            assert_eq!(
                Some(outcome),
                control,
                "reader close must not change main's own delete receipt adoption"
            );
        } else {
            control = Some(outcome);
        }
        assert!(env.bookmark_view_state.is_none());
        assert!(env.take_pending_return_to_parent_nav().is_none());
    }
}

fn main_terminal_bookmark_return(env: &mut App, path: PathBuf) {
    env.current_folder = Some(path.clone());
    env.bookmark_view_state = Some(BookmarkViewState::Opening {
        target: BookmarkViewReturnTarget::Book(path),
        grid: BookmarkViewReturnGridState {
            row_keys: vec![(1, 4)],
            selected_key: Some((1, 4)),
            opened_key: Some((1, 4)),
            scroll_offset_y: 123.0,
        },
    });
    env.transfer_terminal_close_return(env.viewer_context_main(), ViewerCloseCause::User);
    assert!(matches!(
        env.pending_return_to_parent,
        FullscreenCloseContinuation::Terminal(_)
    ));
}

#[test]
fn close_return_independent_active_and_parked_close_preserve_main_live_terminal_and_bookmark_build()
{
    for parked in [false, true] {
        for terminal in [false, true] {
            let mut env = app();
            let target = BookmarkViewReturnTarget::Book(env.tmp.path().join("book.pdf"));
            main_bookmarks(&mut env);
            let ctx = egui::Context::default();
            let (owner, _) = active(&mut env, 94_006, target, None);
            if parked {
                env.transition_detached_window_state(
                    94_006,
                    DetachedWindowState::Parked,
                    "independent_close_parked",
                );
            }
            let sibling = env.push_window_context_for_test(&ctx, 94_007, |viewer| {
                viewer.items = vec![GridItem::Image(PathBuf::from("C:/sibling/page.png"))];
                viewer.visible_indices = vec![0];
                viewer.selected = Some(0);
                viewer.grid_scroll.set_position(321.0);
            });
            env.update_detached_window_runtime_flags(94_007, false, "independent_close_sibling");
            if terminal {
                let path = env.tmp.path().join("main.pdf");
                main_terminal_bookmark_return(&mut env, path);
            } else {
                env.pending_return_to_parent.request_live();
            }
            // Keep an already accepted, not-yet-completed main build. Close must neither
            // cancel nor replace it; this is separate from the navigation continuation.
            let (tx, rx) = mpsc::channel();
            let cancel = Arc::new(AtomicBool::new(false));
            env.bookmark_browser_pending = Some(Box::new(BookmarkBrowserBuild {
                source: env.smart_folder_source_lease().unwrap(),
                switch_sequence: env.quick_folder_switch_sequence,
                worker: crate::bookmark_browser::BookmarkBrowserPending {
                    cancel: cancel.clone(),
                    rx,
                },
            }));
            let before = main_snapshot(&env);
            let sibling_before = env
                .with_viewer_context_state(sibling, |viewer| {
                    (
                        viewer.items.clone(),
                        viewer.visible_indices.clone(),
                        viewer.selected,
                        viewer.grid_scroll.offset(),
                        viewer.items_generation,
                    )
                })
                .unwrap();
            let viewport = register(&mut env, &ctx, 94_006);
            let mut backend = eframe::HeadlessEpiIntegration::new(ctx);
            assert_eq!(
                close_logic(&mut backend, &mut env, viewport),
                eframe::ViewportCloseResponse::RetirementTransferred
            );
            assert!(env.with_viewer_context_ref(owner, |_| ()).is_none());
            assert_eq!(
                main_snapshot(&env),
                before,
                "parked={parked}, terminal={terminal}"
            );
            assert!(!cancel.load(Ordering::Relaxed));
            assert_eq!(
                env.with_viewer_context_state(sibling, |viewer| (
                    viewer.items.clone(),
                    viewer.visible_indices.clone(),
                    viewer.selected,
                    viewer.grid_scroll.offset(),
                    viewer.items_generation
                ))
                .unwrap(),
                sibling_before
            );
            assert!(tx.send(Err("main build remains owned".into())).is_ok());
        }
    }
}

#[test]
fn close_return_remote_acquire_and_return_never_resurrects_main_terminal_request() {
    use crate::remote_ipc::session::{RemoteControlPhase, SessionHandle};
    let mut env = app();
    let path = env.tmp.path().join("main.pdf");
    main_terminal_bookmark_return(&mut env, path);
    let mut backend = eframe::HeadlessEpiIntegration::new(egui::Context::default());
    let session = SessionHandle::new();
    env.set_remote_session_handle(session.clone());
    session.acquire(mimageviewer_ipc::SessionAcquireRequest {
        client_id: "close-return".into(),
        peer: mimageviewer_ipc::SessionPeerInfo {
            connection_kind: mimageviewer_ipc::SessionConnectionKind::Direct,
            device_name: None,
        },
    });
    for _ in 0..3 {
        let _ = backend.update_logic_only(&mut env.app, Default::default());
    }
    assert_eq!(session.snapshot().phase, RemoteControlPhase::RemoteActive);
    assert!(!env.pending_return_to_parent.is_pending());
    assert!(env.take_pending_return_to_parent_nav().is_none());
    session.local_disconnect();
    for _ in 0..3 {
        let _ = backend.update_logic_only(&mut env.app, Default::default());
    }
    assert_eq!(session.snapshot().phase, RemoteControlPhase::Local);
    full(&mut backend, &mut env);
    assert!(!env.pending_return_to_parent.is_pending());
    assert!(env.take_pending_return_to_parent_nav().is_none());
}

#[test]
fn close_return_replaced_remote_and_app_exit_causes_never_publish_independent_return() {
    for cause in [
        ViewerCloseCause::Replaced,
        ViewerCloseCause::RemoteAcquire,
        ViewerCloseCause::AppExit,
    ] {
        let mut env = app();
        let target = BookmarkViewReturnTarget::Book(env.tmp.path().join("book.pdf"));
        main_bookmarks(&mut env);
        let (owner, _) = active(&mut env, 94_008, target, None);
        let before = main_snapshot(&env);
        env.transfer_terminal_close_return(owner, cause);
        assert_eq!(main_snapshot(&env), before);
        env.retire_close_returns_for_main_admission();
        env.transfer_terminal_close_return(owner, ViewerCloseCause::User);
        assert_eq!(main_snapshot(&env), before);
    }
}

#[test]
fn close_return_independent_native_hard_close_preserves_main_without_refresh() {
    let mut env = app();
    let ctx = egui::Context::default();
    let path = env.tmp.path().join("native-closed.mp4");
    let target = BookmarkViewReturnTarget::Media(path.clone());
    main_bookmarks(&mut env);
    let (owner, _) = active(&mut env, 94_009, target, None);
    env.with_viewer_context_state(owner, |viewer| {
        let player = crate::video::VideoPlayer::stream_ready_disconnected_for_test(path);
        player.mark_native_presenter_closed_for_test();
        viewer.fs_cache.insert(
            0,
            FsCacheEntry::Video {
                player: Box::new(player),
                load_seq: 0,
            },
        );
    })
    .unwrap();
    let before = main_snapshot(&env);
    // poll_video observes the production native closed flag, with no synthetic close helper.
    env.with_viewer_context(owner, |viewer| viewer.poll_video(&ctx))
        .unwrap();
    assert_eq!(main_snapshot(&env), before);
    assert!(env.take_pending_return_to_parent_nav().is_none());
    assert_eq!(main_snapshot(&env), before);
}

#[test]
fn close_return_same_path_navigation_and_new_bookmark_invalidate_old_origins_at_admission() {
    for new_bookmark in [false, true] {
        let mut env = app();
        let path = env.tmp.path().join("target.mp4");
        let target = BookmarkViewReturnTarget::Media(path);
        main_bookmarks(&mut env);
        let (old, _) = active(&mut env, 94_004, target, None);
        let same_path = env.current_folder.clone();
        if new_bookmark {
            // The real bookmark admission can begin asynchronous loading, but does not launch a player.
            let new_row = env.bookmark_browser_rows[4].clone();
            env.open_bookmark_browser_row(&egui::Context::default(), &new_row);
        } else {
            // A real independent navigation to the same Bookmark surface passes through its
            // ordinary admission. Path equality cannot authorize the previous return request.
            env.open_bookmark_browser();
            assert_eq!(env.current_folder, same_path);
        }
        let new_target = env
            .bookmark_view_state
            .as_ref()
            .and_then(BookmarkViewState::target)
            .cloned();
        let request = env.bookmark_open_request_seq;
        env.transfer_terminal_close_return(old, ViewerCloseCause::User);
        assert!(!matches!(
            env.pending_return_to_parent,
            FullscreenCloseContinuation::Terminal(_)
        ));
        assert_eq!(env.bookmark_open_request_seq, request);
        assert_eq!(
            env.bookmark_view_state
                .as_ref()
                .and_then(BookmarkViewState::target)
                .cloned(),
            new_target
        );
        // A replacement may already have retired the old context. If it remains parked,
        // its original return identity must have been cleared at the same admission.
        let _ = env.with_viewer_context_state(old, |viewer| {
            assert!(matches!(
                viewer.pending_return_to_parent,
                FullscreenCloseContinuation::None
            ))
        });
    }
}

#[test]
fn close_return_independent_loading_and_holdover_shell_close_preserve_main() {
    for shown in [false, true] {
        for os_close in [false, true] {
            // egui's immediate renderer is thread-local; isolate its fake registration from
            // ordinary headless tests just as the established backstop fixture does.
            std::thread::spawn(move || {
                let mut env = app();
                let ctx = egui::Context::default();
                let target = BookmarkViewReturnTarget::Book(env.tmp.path().join("loading.pdf"));
                main_bookmarks(&mut env);
                let (owner, _) = active(&mut env, 94_010, target, None);
                env.with_viewer_context_state(owner, |viewer| {
                    viewer.fullscreen_idx = None;
                    viewer.fs_viewport_shown = shown;
                    viewer.fs_viewport_presentation =
                        shown.then_some(ViewerPresentation::DetachedWindow);
                    viewer.window_visible = false;
                })
                .unwrap();
                let before = main_snapshot(&env);
                let calls = std::rc::Rc::new(std::cell::Cell::new(0));
                let seen = calls.clone();
                egui::Context::set_immediate_viewport_renderer(move |ctx, mut viewport| {
                    seen.set(seen.get() + 1);
                    let events = if os_close {
                        Vec::new()
                    } else {
                        vec![egui::Event::Key {
                            key: egui::Key::Escape,
                            physical_key: Some(egui::Key::Escape),
                            pressed: true,
                            repeat: false,
                            modifiers: egui::Modifiers::NONE,
                        }]
                    };
                    let output = ctx.run_ui(
                        egui::RawInput {
                            viewport_id: viewport.ids.this,
                            screen_rect: Some(egui::Rect::from_min_size(
                                egui::Pos2::ZERO,
                                egui::vec2(640.0, 480.0),
                            )),
                            events,
                            viewports: [
                                (egui::ViewportId::ROOT, egui::ViewportInfo::default()),
                                (
                                    viewport.ids.this,
                                    egui::ViewportInfo {
                                        parent: Some(egui::ViewportId::ROOT),
                                        events: if os_close {
                                            vec![egui::ViewportEvent::Close]
                                        } else {
                                            Vec::new()
                                        },
                                        ..Default::default()
                                    },
                                ),
                            ]
                            .into_iter()
                            .collect(),
                            ..Default::default()
                        },
                        |ui| (viewport.viewport_ui_cb)(ui),
                    );
                    assert!(output.viewport_output.contains_key(&viewport.ids.this));
                    output.drop_without_applying_deltas();
                });
                ctx.set_embed_viewports(false);
                env.with_viewer_context(owner, |viewer| {
                    assert!(viewer.active_detached_transition_outstanding());
                    ctx.run_ui(Default::default(), |ui| {
                        viewer.keep_fullscreen_viewport_alive(ui.ctx())
                    })
                    .drop_without_applying_deltas();
                })
                .unwrap();
                assert_eq!(calls.get(), 1, "shown={shown}, os_close={os_close}");
                assert_eq!(
                    main_snapshot(&env),
                    before,
                    "shown={shown}, os_close={os_close}"
                );
                assert!(env.take_pending_return_to_parent_nav().is_none());
                assert_eq!(main_snapshot(&env), before);
            })
            .join()
            .unwrap();
        }
    }
}

fn settle_source(app: &mut App, ctx: &egui::Context) {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        app.settle_open_path_classification_for_test();
        app.poll_collection_history_transition(ctx);
        app.poll_sidecar_restore(ctx);
        app.poll_bookmark_browser(ctx);
        app.poll_rating_view();
        app.poll_tag_view();
        app.poll_favsearch();
        app.poll_search_page_edit_prepare(ctx);
        app.poll_smart_folder(ctx);
        app.poll_collection_ui(ctx);
        app.poll_collection_grid(ctx);
        app.poll_reading_history_thumbnail_sources(ctx);
        app.finish_page_identity_preparation_for_test();
        let collection_ready = app
            .top_level_grid_view
            .collection_session()
            .is_none_or(|session| {
                matches!(
                    session.load,
                    top_level_grid_view::CollectionGridLoadState::Ready(_)
                        | top_level_grid_view::CollectionGridLoadState::Empty(_)
                ) && session.installed_items_generation == Some(app.items_generation)
            });
        if app
            .top_level_grid_view
            .history_navigation_transition()
            .is_none()
            && app.top_level_grid_view.open_path_classification().is_none()
            && !app.sidecar_restore_active()
            && app.bookmark_browser_pending.is_none()
            && app.rating_view_pending.is_none()
            && app.tag_view_pending.is_none()
            && app.favsearch_pending.is_none()
            && app.smart_folder_pending.is_none()
            && app.smart_folder_transition.is_none()
            && app.reading_history_sources_pending.is_empty()
            && collection_ready
        {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "source did not finish: {:?}",
            app.top_level_grid_view.surface()
        );
        std::thread::yield_now();
    }
}

/// Build the displayed list through each production producer. Only external Everything
/// transport and the metadata search worker completion are replaced by immutable results.
fn enter_source(env: &mut AppTestEnvForTest, ctx: &egui::Context, source: &str, book: &Path) {
    let root = env.tmp.path().to_path_buf();
    env.load_folder(root.clone());
    env.finish_page_identity_preparation_for_test();
    let row = GridItem::Folder(book.to_path_buf());
    match source {
        "folder" => {}
        "everything" => {
            let data = crate::global_search_ui::everything_dataset_for_test(vec![row]);
            crate::global_search_ui::adopt_everything_for_test(env, ctx, "independent-close", data);
        }
        "global-search" => {
            env.open_global_search();
            env.replace_search_view_items(vec![row], vec![None]);
        }
        "favorite-search" => {
            env.settings
                .favorites
                .push(crate::settings::FavoriteEntry::new(
                    "close test".into(),
                    root.clone(),
                ));
            env.search_index_db
                .as_ref()
                .unwrap()
                .upsert_children(
                    &root,
                    &root,
                    &[crate::search_index_db::IndexEntry {
                        path: book.to_path_buf(),
                        display_name: "independent-book".into(),
                        kind: crate::search_index_db::IndexKind::Folder,
                        mtime: 1,
                    }],
                )
                .unwrap();
            env.open_favsearch();
            env.favsearch.query = "independent-book".into();
            env.execute_favsearch();
        }
        "tag" => {
            env.tags_db
                .as_mut()
                .unwrap()
                .set_item_tags(
                    &crate::tags_db::item_key_for_path(book),
                    ["close-test"],
                    crate::tags_db::source::EDIT,
                )
                .unwrap();
            env.open_tag_view_for_tag("close-test");
        }
        "rating" => {
            env.rating_db
                .as_ref()
                .unwrap()
                .set(&crate::adjustment_db::normalize_path(book), 3)
                .unwrap();
            env.enter_rating_view(3);
        }
        "history" => {
            let entry = crate::reading_history_db::ReadingHistoryEntry::new(
                book.to_path_buf(),
                crate::reading_history_db::ReadingHistoryKind::Folder,
                None,
                "close test".into(),
                None,
                None,
            );
            env.reading_history_db
                .as_ref()
                .unwrap()
                .upsert(entry, 1000)
                .unwrap();
            env.enter_reading_history_from_menu();
        }
        "smart" => {
            let mut definition = crate::settings::SmartFolderDefinition::new("close test");
            definition.rules.push(crate::settings::SmartFolderRule::new(
                root.clone(),
                true,
                Default::default(),
            ));
            let id = definition.id;
            env.settings.smart_folders.push(definition);
            env.open_smart_folder(id, false);
        }
        "collection" => {
            let runtime = crate::collection_store::CollectionStoreRuntime::start_at(
                root.join("close-collections.db"),
            )
            .unwrap();
            let client = runtime.client();
            env.install_collection_runtime(runtime);
            let deadline = Instant::now() + Duration::from_secs(30);
            while !matches!(env.collection_store_client_for_read(), Ok(Some(_))) {
                env.poll_collection_ui(ctx);
                assert!(Instant::now() < deadline);
                std::thread::yield_now();
            }
            let created = client
                .create_collection("close test".into())
                .unwrap()
                .recv_timeout(Duration::from_secs(30))
                .unwrap()
                .unwrap();
            let registration = crate::collection_store::CollectionRegistration::from_trusted_path(
                book,
                crate::collection_store::CollectionResolvedKind::Folder,
            )
            .unwrap();
            client
                .add_batch(
                    created.collection_id(),
                    created.revision(),
                    vec![registration],
                )
                .unwrap()
                .recv_timeout(Duration::from_secs(30))
                .unwrap()
                .unwrap();
            env.open_collection_grid_from_navigation(created.collection_id());
        }
        _ => unreachable!("unknown source {source}"),
    }
    settle_source(env, ctx);
    env.selected = env.items.iter().position(|item| {
        item.drag_source_path()
            .is_some_and(|path| crate::folder_tree::path_eq(path, book))
    });
    assert!(
        env.selected.is_some(),
        "{source} producer omitted its book: {:?}",
        env.items
    );
    env.grid_scroll.set_position(240.0);
}

fn write_image_book(root: &Path, name: &str) -> PathBuf {
    let book = root.join(name);
    std::fs::create_dir(&book).unwrap();
    image::RgbImage::new(2, 2)
        .save(book.join("001.png"))
        .unwrap();
    book
}

fn finish_real_folder_reader(env: &mut App, ctx: &egui::Context, owner: ViewerContextId) {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        if let Some(ready) = env.poll_folder_pane_open(ctx) {
            assert!(env.resolve_main_folder_open_ready(ctx, ready).is_none());
        }
        let ready = env
            .with_viewer_context(owner, |viewer| {
                viewer.poll_sidecar_restore(ctx);
                viewer.finish_page_identity_preparation_for_test();
                assert!(!viewer.pending_return_to_parent.is_pending());
                viewer.fullscreen_idx.is_some()
            })
            .unwrap();
        if ready {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "independent scan did not install"
        );
        std::thread::yield_now();
    }
}

#[test]
fn close_return_real_producers_multi_window_loading_and_ready_retirement_preserve_main() {
    for source in [
        "everything",
        "global-search",
        "favorite-search",
        "tag",
        "rating",
        "collection",
        "history",
        "smart",
        "folder",
    ] {
        for loading in [false, true] {
            let mut env = app();
            let ctx = egui::Context::default();
            let book = write_image_book(env.tmp.path(), "independent-book");
            env.settings.detached_viewer_enabled = true;
            env.settings.detached_viewer_open_images_in_window = true;
            env.settings.auto_fullscreen_image_folders = true;
            env.settings.auto_fullscreen_zip_pdf = true;
            enter_source(&mut env, &ctx, source, &book);
            let before = main_snapshot(&env);
            let index = env.selected.unwrap();
            // Same setting-gated handler used by grid Enter/double-click, not a context fixture.
            assert!(
                env.open_grid_container_in_detached_book_context(&ctx, index),
                "{source}"
            );
            env.settle_open_path_classification_for_test();
            let owner = env
                .active_viewer_context_id()
                .expect("real open must own an independent bundle");
            assert_ne!(owner, env.viewer_context_main());
            if !loading {
                finish_real_folder_reader(&mut env, &ctx, owner);
            }
            assert_eq!(
                main_snapshot(&env),
                before,
                "{source}: opening changed main"
            );
            let window = env.active_detached_session.unwrap().window_id;
            let viewport = register(&mut env, &ctx, window);
            let mut backend = eframe::HeadlessEpiIntegration::new(ctx);
            assert_eq!(
                close_logic(&mut backend, &mut env, viewport),
                eframe::ViewportCloseResponse::RetirementTransferred,
                "{source}, loading={loading}"
            );
            assert!(env.with_viewer_context_ref(owner, |_| ()).is_none());
            assert_eq!(
                main_snapshot(&env),
                before,
                "{source}: close changed main, loading={loading}"
            );
            assert!(env.take_pending_return_to_parent_nav().is_none());
            assert_eq!(main_snapshot(&env), before);
        }
    }
}

#[test]
fn close_return_real_independent_immediate_close_cannot_consume_main_live_or_terminal() {
    for terminal in [false, true] {
        let mut env = app();
        let ctx = egui::Context::default();
        let book = write_image_book(env.tmp.path(), "independent-book");
        env.settings.detached_viewer_enabled = true;
        env.settings.detached_viewer_open_images_in_window = true;
        env.settings.auto_fullscreen_image_folders = true;
        enter_source(&mut env, &ctx, "folder", &book);
        let index = env.selected.unwrap();
        assert!(env.open_grid_container_in_detached_book_context(&ctx, index));
        let owner = env.active_viewer_context_id().unwrap();
        assert_ne!(owner, env.viewer_context_main());
        finish_real_folder_reader(&mut env, &ctx, owner);
        if terminal {
            let path = env.tmp.path().join("main.pdf");
            main_terminal_bookmark_return(&mut env, path);
        } else {
            env.pending_return_to_parent.request_live();
        }
        let before = main_snapshot(&env);
        env.with_viewer_context(owner, |viewer| {
            viewer.handle_fullscreen_close_request_immediate()
        })
        .unwrap();
        assert_eq!(main_snapshot(&env), before, "terminal={terminal}");
        // Retire through the normal active-context update too; it cannot drain main pending.
        ctx.run_ui(Default::default(), |ui| {
            assert!(env.update_active_viewer_context(ui, None).updated);
        })
        .drop_without_applying_deltas();
        assert!(env.with_viewer_context_ref(owner, |_| ()).is_none());
        assert_eq!(
            main_snapshot(&env),
            before,
            "active retirement: terminal={terminal}"
        );
    }
}

#[test]
fn close_return_real_fullfeature_media_is_shared_until_promote_or_live_fork() {
    for is_audio in [false, true] {
        for transition in ["shared-close", "promote", "live-fork"] {
            let live_fork = transition == "live-fork";
            let mut env = app();
            let ctx = egui::Context::default();
            let path = env
                .tmp
                .path()
                .join(if is_audio { "media.flac" } else { "media.mp4" });
            std::fs::write(&path, []).unwrap();
            env.settings.detached_viewer_enabled = true;
            env.settings.detached_viewer_open_images_in_window = false;
            env.settings.fullfeature_media_window = true;
            env.settings.vst3_enabled = false;
            env.current_folder = Some(env.tmp.path().to_path_buf());
            env.items = vec![if is_audio {
                GridItem::Audio(path.clone())
            } else {
                GridItem::Video(path.clone())
            }];
            env.thumbnails = vec![ThumbnailState::Pending];
            env.image_metas = vec![None];
            env.visible_indices = vec![0];
            env.selected = Some(0);
            env.fs_open_intent_from_grid = true;
            env.open_fullscreen(0, HistoryTrigger::UserChosen);
            let main = env.viewer_context_main();
            let window = env
                .active_detached_session
                .expect("real media open must bind its window")
                .window_id;
            assert_eq!(env.projected_viewer_context_id(), main);
            assert_eq!(
                env.locate_window_context(window).map(|(owner, _)| owner),
                Some(main)
            );
            assert_eq!(env.active_viewer_context_id(), Some(main));
            assert!(
                !env.active_detached_context_is_at_rest(),
                "setting alone does not create an independent bundle"
            );
            // Replace decoding only. The production setting/open/session/binding above remain real.
            env.fs_cache.insert(
                0,
                FsCacheEntry::Video {
                    player: Box::new(
                        crate::video::VideoPlayer::stream_ready_disconnected_for_test(path),
                    ),
                    load_seq: 0,
                },
            );
            if transition == "shared-close" {
                // The same real-mode open closed before separation still restores main selection.
                env.selected = None;
                env.handle_fullscreen_close_request_immediate();
                assert_eq!(env.projected_viewer_context_id(), main);
                assert_eq!(env.fullscreen_idx, None);
                assert_eq!(env.selected, Some(0));
                assert!(env.grid_scroll.reveal_requested());
                assert!(!env.pending_return_to_parent.is_pending());
                continue;
            }
            if live_fork {
                env.pending_return_to_parent.request_live();
                assert!(env.park_current_viewer_context_as_live_media(
                    &ctx,
                    "independent_close_live_fork"
                ));
                assert!(
                    matches!(
                        env.pending_return_to_parent,
                        FullscreenCloseContinuation::Live
                    ),
                    "fork must keep main's live return"
                );
            } else {
                assert!(env.promote_active_detached_video_for_main_context_change());
                assert_ne!(env.viewer_context_main(), main);
            }
            let owner = env.locate_window_context(window).unwrap().0;
            assert_ne!(owner, env.viewer_context_main());
            env.with_viewer_context_state(owner, |viewer| {
                assert!(!viewer.pending_return_to_parent.is_pending())
            })
            .unwrap();
            let before = main_snapshot(&env);
            let viewport = register(&mut env, &ctx, window);
            let mut backend = eframe::HeadlessEpiIntegration::new(ctx);
            assert_eq!(
                close_logic(&mut backend, &mut env, viewport),
                eframe::ViewportCloseResponse::RetirementTransferred
            );
            assert_eq!(
                main_snapshot(&env),
                before,
                "audio={is_audio}, live_fork={live_fork}"
            );
            assert!(env.with_viewer_context_ref(owner, |_| ()).is_none());
        }
    }
}

#[test]
fn close_return_real_bookmark_open_in_both_modes_keeps_main_out_of_opening() {
    for multi_window in [false, true] {
        for kind in ["book", "video", "audio"] {
            if !multi_window && kind == "book" {
                continue;
            } // Full-feature books use main.
            let mut env = app();
            let ctx = egui::Context::default();
            env.settings.detached_viewer_enabled = true;
            env.settings.detached_viewer_open_images_in_window = multi_window;
            env.settings.fullfeature_media_window = !multi_window;
            env.settings.vst3_enabled = false;
            let mut bookmark = row(env.tmp.path(), 1);
            if kind == "book" {
                let book = write_image_book(env.tmp.path(), "bookmark-book");
                bookmark.item = GridItem::Folder(book.clone());
                bookmark.source = crate::bookmark_browser::BookmarkRowSource::Book(
                    crate::book_bookmarks::BookBookmark {
                        id: 1,
                        container_key: crate::adjustment_db::normalize_path(&book),
                        container_path: book,
                        container_kind: crate::book_bookmarks::BookContainerKind::ImageFolder,
                        page_identity: crate::book_bookmarks::PageIdentity::RelativePath(
                            "001.png".into(),
                        ),
                        page_index_hint: 0,
                        created_at_ms: 1,
                        title: None,
                    },
                );
            } else {
                let path = env.tmp.path().join(if kind == "audio" {
                    "bookmark.flac"
                } else {
                    "bookmark.mp4"
                });
                std::fs::write(&path, []).unwrap();
                bookmark.item = if kind == "audio" {
                    GridItem::Audio(path.clone())
                } else {
                    GridItem::Video(path.clone())
                };
                bookmark.source = crate::bookmark_browser::BookmarkRowSource::Media {
                    id: 1,
                    path,
                    pts_secs: 42.0,
                    title: None,
                    is_audio: kind == "audio",
                };
            }
            env.open_bookmark_browser();
            let tx = ready_read_model_with_rows(&mut env, vec![bookmark.clone()]);
            env.poll_bookmark_browser(&ctx);
            assert!(tx.send(Err("completion consumed".into())).is_err());
            env.finish_page_identity_preparation_for_test();
            env.selected = Some(0);
            env.grid_scroll.set_position(240.0);
            let before = main_snapshot(&env);
            env.open_bookmark_browser_row(&ctx, &bookmark);
            let owner = env
                .active_viewer_context_id()
                .expect("bookmark router must create an independent loading bundle");
            assert_ne!(owner, env.viewer_context_main());
            assert!(
                env.bookmark_view_state.is_none(),
                "{kind}/{multi_window}: main must not wait for reader close"
            );
            assert_eq!(main_snapshot(&env), before, "{kind}/{multi_window}: open");
            let window = env.active_detached_session.unwrap().window_id;
            let viewport = register(&mut env, &ctx, window);
            let mut backend = eframe::HeadlessEpiIntegration::new(ctx);
            assert_eq!(
                close_logic(&mut backend, &mut env, viewport),
                eframe::ViewportCloseResponse::RetirementTransferred
            );
            assert_eq!(main_snapshot(&env), before, "{kind}/{multi_window}: close");
            assert!(env.take_pending_return_to_parent_nav().is_none());
            assert_eq!(main_snapshot(&env), before);
        }
    }
}

#[test]
fn close_return_real_bookmark_media_next_file_in_both_modes_preserves_main() {
    for multi_window in [false, true] {
        for is_audio in [false, true] {
            let mut env = app();
            let ctx = egui::Context::default();
            env.settings.detached_viewer_enabled = true;
            env.settings.detached_viewer_open_images_in_window = multi_window;
            env.settings.fullfeature_media_window = !multi_window;
            env.settings.vst3_enabled = false;
            let media_dir = env.tmp.path().join("bookmark-media");
            std::fs::create_dir(&media_dir).unwrap();
            let extension = if is_audio { "flac" } else { "mp4" };
            let first = media_dir.join(format!("a.{extension}"));
            let next = media_dir.join(format!("b.{extension}"));
            std::fs::write(&first, []).unwrap();
            std::fs::write(&next, []).unwrap();
            let bookmark = BookmarkBrowserRow {
                source: crate::bookmark_browser::BookmarkRowSource::Media {
                    id: 1,
                    path: first.clone(),
                    pts_secs: 5.0,
                    title: None,
                    is_audio,
                },
                item: if is_audio {
                    GridItem::Audio(first.clone())
                } else {
                    GridItem::Video(first.clone())
                },
                relative_page_provenance: None,
                image_meta: None,
                marker_thumbnail: None,
                created_at_ms: 1,
                missing: false,
            };
            env.open_bookmark_browser();
            let tx = ready_read_model_with_rows(&mut env, vec![bookmark.clone()]);
            env.poll_bookmark_browser(&ctx);
            assert!(tx.send(Err("completion consumed".into())).is_err());
            env.finish_page_identity_preparation_for_test();
            env.selected = Some(0);
            env.grid_scroll.set_position(240.0);
            let before = main_snapshot(&env);
            env.open_bookmark_browser_row(&ctx, &bookmark);
            let owner = env
                .active_viewer_context_id()
                .expect("bookmark row must publish its independent loading context");
            assert_ne!(owner, env.viewer_context_main());
            assert_eq!(main_snapshot(&env), before);
            let StartupOpenPathOwner::Bookmark(request) = env
                .startup_open_path_resolve_pending
                .as_ref()
                .expect("accepted resolver")
                .owner
                .clone()
            else {
                unreachable!()
            };
            assert!(
                env.bookmark_open_owner_is_current(&request),
                "main must read the reader's owner"
            );
            let mut wrong = request.clone();
            wrong.request_id.0 += 1;
            assert!(!env.bookmark_open_owner_is_current(&wrong));
            wrong = request.clone();
            wrong.target = BookmarkViewReturnTarget::Media(next.clone());
            assert!(!env.bookmark_open_owner_is_current(&wrong));
            wrong = request.clone();
            wrong.detached_lease = Some(DetachedSessionLease {
                window_id: request.detached_lease.unwrap().window_id + 1,
            });
            assert!(!env.bookmark_open_owner_is_current(&wrong));

            // Admit the actual resolver, directory scan, and listing hydration before any
            // player substitution or next-file input. A shell alone cannot prove navigation.
            let deadline = Instant::now() + Duration::from_secs(30);
            loop {
                // Production root service polls this App-global inbox before mounting the reader.
                env.poll_startup_open_path_resolve(&ctx);
                let ready = env
                    .with_viewer_context(owner, |viewer| {
                        if viewer.folder_pane_open_pending.is_some() {
                            assert!(!matches!(
                                viewer.poll_detached_physical_folder_open(&ctx),
                                DetachedPhysicalFolderOpenPoll::Failed
                            ));
                        }
                        viewer.poll_sidecar_restore(&ctx);
                        viewer.finish_page_identity_preparation_for_test();
                        viewer.startup_open_path_resolve_pending.is_none()
                            && viewer.folder_pane_open_pending.is_none()
                            && !viewer.sidecar_restore_active()
                            && viewer
                                .fullscreen_idx
                                .and_then(|idx| viewer.items.get(idx))
                                .and_then(GridItem::drag_source_path)
                                .is_some_and(|path| crate::path_key::eq_keep_drive(path, &first))
                    })
                    .expect("initial open must retain its context");
                if ready {
                    break;
                }
                assert!(
                    Instant::now() < deadline,
                    "bookmark media open did not finish: multi_window={multi_window}, audio={is_audio}"
                );
                std::thread::yield_now();
            }
            env.with_viewer_context(owner, |viewer| {
                assert!(viewer.startup_open_path_resolve_pending.is_none());
                assert!(viewer.folder_pane_open_pending.is_none());
                assert!(!viewer.sidecar_restore_active());
                let first_idx = viewer.fullscreen_idx.unwrap();
                viewer.fs_cache.insert(first_idx, FsCacheEntry::Video {
                    player: Box::new(crate::video::VideoPlayer::stream_ready_disconnected_for_test(first.clone())),
                    load_seq: 0,
                });
                // An already cached player completes deferred regular open without creating
                // a native host. Only decoding is substituted; bookmark seek runs normally.
                viewer.poll_native_video_open_pending(&ctx);
                viewer.poll_bookmark_media_open(&ctx);
                assert!(viewer.native_video_open_pending.is_none());
                assert!(viewer.bookmark_open_pending.is_none());
                assert!(matches!(&viewer.bookmark_view_state,
                    Some(BookmarkViewState::Detached { target: BookmarkViewReturnTarget::Media(path) })
                    if crate::path_key::eq_keep_drive(path, &first)));
                if let Some(FsCacheEntry::Video { player, .. }) = viewer.fs_cache.get_mut(&first_idx) {
                    // The disconnected output is reusable for fast swap. Drop that fake
                    // resource so the real handler takes its ordinary no-presenter open path.
                    drop(player.take_native_output());
                } else {
                    unreachable!();
                }
                assert!(!viewer.fs_nav_is_locked());
                viewer.navigate_native_video_fullscreen(&ctx, first_idx, 1);
                let deadline = Instant::now() + Duration::from_secs(10);
                while viewer.media_navigation_pending.is_some() {
                    viewer.poll_media_navigation_pending(&ctx);
                    assert!(Instant::now() < deadline, "next-file candidate resolution did not finish");
                    std::thread::yield_now();
                }
                let next_idx = viewer.fullscreen_idx.expect("next-file handler must open a target");
                assert_ne!(next_idx, first_idx);
                assert!(viewer.items.get(next_idx)
                    .and_then(GridItem::drag_source_path)
                    .is_some_and(|path| crate::path_key::eq_keep_drive(path, &next)));
                assert_eq!(viewer.selected, Some(next_idx));
                // Detached keeps its opening target across same-folder navigation. Its
                // eligibility changes through the actual displayed file, not a test reset.
                assert!(!viewer.bookmark_target_is_current(&BookmarkViewReturnTarget::Media(first.clone())),
                    "real next-file navigation must leave the bookmarked file");
                assert!(viewer.bookmark_view_back_nav().is_none());
                assert!(viewer.bookmark_open_pending.is_none());
                assert!(viewer.native_video_source_swap_pending.is_none());
                assert!(viewer.native_video_fast_swap_pending.is_none());
                viewer.fs_cache.insert(next_idx, FsCacheEntry::Video {
                    player: Box::new(crate::video::VideoPlayer::stream_ready_disconnected_for_test(next.clone())),
                    load_seq: 0,
                });
                viewer.poll_native_video_open_pending(&ctx);
                assert!(viewer.native_video_open_pending.is_none());
                assert!(viewer.startup_open_path_resolve_pending.is_none());
                assert!(viewer.folder_pane_open_pending.is_none());
            }).unwrap();
            assert!(env.bookmark_view_state.is_none());
            assert_eq!(
                main_snapshot(&env),
                before,
                "next-file input must affect only its reader"
            );
            let window = env.active_detached_session.unwrap().window_id;
            let viewport = register(&mut env, &ctx, window);
            let mut backend = eframe::HeadlessEpiIntegration::new(ctx);
            assert_eq!(
                close_logic(&mut backend, &mut env, viewport),
                eframe::ViewportCloseResponse::RetirementTransferred
            );
            assert!(env.with_viewer_context_ref(owner, |_| ()).is_none());
            assert!(
                !env.bookmark_open_owner_is_current(&request),
                "closed lease cannot adopt a late completion"
            );
            assert!(env.bookmark_view_state.is_none());
            assert_eq!(
                main_snapshot(&env),
                before,
                "multi_window={multi_window}, audio={is_audio}"
            );
            assert!(env.take_pending_return_to_parent_nav().is_none());
            assert_eq!(main_snapshot(&env), before);
        }
    }
}

#[test]
fn close_return_bookmark_request_currentness_reads_building_and_at_rest_owner() {
    let mut env = app();
    let ctx = egui::Context::default();
    let path = env.tmp.path().join("loading.mp4");
    let target = BookmarkViewReturnTarget::Media(path.clone());
    let request_id = crate::bookmark_browser::BookmarkOpenRequestId(71);
    env.bookmark_open_request_seq = request_id.0;
    let mut captured = None;
    let window = env
        .start_active_detached_loading_context(
            &ctx,
            None,
            DetachedSource::Video,
            path.clone(),
            |reader, window| {
                reader.bookmark_open_pending =
                    Some(crate::bookmark_browser::PendingBookmarkOpen::Media(
                        crate::bookmark_browser::PendingMediaOpen {
                            request_id,
                            path: path.clone(),
                            pts_secs: 5.0,
                            started_at: Instant::now(),
                            last_wait: None,
                        },
                    ));
                reader.bookmark_view_state = Some(BookmarkViewState::Detached {
                    target: target.clone(),
                });
                let request = crate::bookmark_browser::BookmarkOpenRequestOwner {
                    request_id,
                    target: target.clone(),
                    detached_lease: Some(DetachedSessionLease { window_id: window }),
                };
                assert_eq!(
                    reader.viewer_context_residence(reader.projected_viewer_context_id()),
                    ContextResidence::Building
                );
                assert!(reader.bookmark_open_owner_is_current(&request));
                captured = Some(request);
                DetachedLoadingContextInit::Continue
            },
        )
        .expect("loading owner must commit");
    let request = captured.unwrap();
    let owner = env.locate_window_context(window).unwrap().0;
    assert_ne!(owner, env.viewer_context_main());
    assert!(env.bookmark_view_state.is_none());
    assert!(env.bookmark_open_owner_is_current(&request));
    // A consumed request can still belong to this exact Preparing session, as before.
    env.with_viewer_context_state(owner, |reader| reader.bookmark_open_pending = None)
        .unwrap();
    assert!(env.bookmark_open_owner_is_current(&request));
    assert!(env.close_current_active_viewer_context(&ctx));
    assert!(!env.bookmark_open_owner_is_current(&request));
    assert!(env.bookmark_view_state.is_none());
}
