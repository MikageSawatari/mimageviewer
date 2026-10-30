//! Non-drawing Close transfers a value; only the surviving main UI consumes it.
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

fn main_bookmarks(env: &mut AppTestEnvForTest, target: BookmarkViewReturnTarget) {
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
    env.bookmark_view_state = Some(BookmarkViewState::Opening {
        target,
        grid: BookmarkViewReturnGridState {
            row_keys: (1..=30).map(|id| (0, id)).collect(),
            selected_key: Some((0, 2)),
            opened_key: (0, 2),
            scroll_offset_y: 240.0,
        },
    });
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

#[test]
fn close_return_actual_logic_only_active_book_and_media_wait_for_main_full() {
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
        main_bookmarks(&mut env, target.clone());
        let (id, cancel) = active(&mut env, 94_001, target, None);
        let viewport = register(&mut env, &ctx, 94_001);
        let read_model_tx = ready_read_model(&mut env);
        let main = env.viewer_context_main();
        let generation = env.items_generation;
        let rows: Vec<_> = env
            .bookmark_browser_rows
            .iter()
            .map(BookmarkBrowserRow::stable_key)
            .collect();
        let dirty = env.tmp.path().join("dirty");
        env.folder_thumb_pin_dirty.insert(dirty.clone());
        env.video_thumb_overrides_dirty_paths.insert(dirty.clone());
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
            assert_eq!(env.projected_viewer_context_id(), main);
            assert!(matches!(
                env.pending_return_to_parent,
                FullscreenCloseContinuation::Terminal(_)
            ));
            assert!(env.bookmark_browser_pending.is_some());
            assert_eq!(env.items_generation, generation);
            assert_eq!(env.frame_counter, frame);
            assert_eq!(
                env.bookmark_browser_rows
                    .iter()
                    .map(BookmarkBrowserRow::stable_key)
                    .collect::<Vec<_>>(),
                rows
            );
            assert!(env.folder_thumb_pin_dirty.contains(&dirty));
            assert!(env.video_thumb_overrides_dirty_paths.contains(&dirty));
        }
        full(&mut backend, &mut env);
        assert!(read_model_tx.send(Err("duplicate".into())).is_err());
        assert!(!env.pending_return_to_parent.is_pending());
        assert!(matches!(
            env.bookmark_view_state,
            Some(BookmarkViewState::Restoring { .. })
        ));
        assert!(env.bookmark_browser_pending.is_some());
        assert!(ctx.viewport_registration(viewport).is_none());
    }
}

#[test]
fn close_return_media_departure_uses_last_real_folder_while_book_navigation_keeps_grid() {
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
        main_bookmarks(&mut env, target.clone());
        let final_folder = env.tmp.path().join("final-folder");
        let final_media = final_folder.join("last.mp4");
        let (id, _) = active(&mut env, 94_002, target.clone(), Some(final_media.clone()));
        if book {
            env.with_viewer_context_state(id, |viewer| {
                viewer.bookmark_view_state = Some(BookmarkViewState::Detached { target })
            })
            .unwrap();
        }
        let ctx = egui::Context::default();
        let viewport = register(&mut env, &ctx, 94_002);
        let mut backend = eframe::HeadlessEpiIntegration::new(ctx);
        assert_eq!(
            close_logic(&mut backend, &mut env, viewport),
            eframe::ViewportCloseResponse::RetirementTransferred
        );
        assert!(matches!(
            env.pending_return_to_parent,
            FullscreenCloseContinuation::Terminal(_)
        ));
        let nav = env.take_pending_return_to_parent_nav();
        if book {
            assert!(nav.is_none());
            assert!(matches!(
                env.bookmark_view_state,
                Some(BookmarkViewState::Restoring { .. })
            ));
        } else {
            assert!(
                matches!(nav, Some(crate::ui_main::AddressBarNav::Direct(ref folder, _)) if folder == &final_folder)
            );
            assert_eq!(env.select_after_load.as_deref(), Some("last.mp4"));
        }
        assert!(env.take_pending_return_to_parent_nav().is_none());
    }
}

#[test]
fn close_return_unpresented_media_fallback_requires_exact_preparing_session() {
    for sibling_preparing in [false, true] {
        let mut env = app();
        let target = BookmarkViewReturnTarget::Media(env.tmp.path().join("original.mp4"));
        main_bookmarks(&mut env, target.clone());
        let final_folder = env.tmp.path().join("last-folder");
        let (owner, _) = active(&mut env, 94_011, target, None);
        env.with_viewer_context_state(owner, |viewer| {
            // No current media cannot establish that a Ready viewer stayed at its
            // original bookmark, nor can another window's initial-open authority.
            viewer.items.clear();
            viewer.selected = None;
            viewer.fullscreen_idx = None;
            viewer.current_folder = Some(final_folder.clone());
        })
        .unwrap();
        if sibling_preparing {
            let sibling_target =
                BookmarkViewReturnTarget::Media(env.tmp.path().join("sibling.mp4"));
            let (sibling, _) = active(&mut env, 94_012, sibling_target, None);
            assert_ne!(owner, sibling);
            assert_eq!(
                env.active_detached_session.unwrap().content_phase,
                DetachedSessionContentPhase::Preparing
            );
        } else {
            env.active_detached_session.as_mut().unwrap().content_phase =
                DetachedSessionContentPhase::Ready;
        }
        env.transfer_terminal_close_return(owner, ViewerCloseCause::User);
        assert!(matches!(
            env.pending_return_to_parent,
            FullscreenCloseContinuation::Terminal(_)
        ));
        assert!(matches!(
            env.take_pending_return_to_parent_nav(),
            Some(crate::ui_main::AddressBarNav::Direct(ref folder, _)) if folder == &final_folder
        ));
        assert!(env.bookmark_view_state.is_none());
        assert!(env.take_pending_return_to_parent_nav().is_none());
    }
}

#[test]
fn close_return_main_selection_scroll_and_same_list_refresh_survive_until_consumer() {
    let mut env = app();
    let target = BookmarkViewReturnTarget::Book(env.tmp.path().join("book.pdf"));
    main_bookmarks(&mut env, target.clone());
    let (_, _) = active(&mut env, 94_003, target, None);
    let ctx = egui::Context::default();
    let viewport = register(&mut env, &ctx, 94_003);
    let mut backend = eframe::HeadlessEpiIntegration::new(ctx);
    close_logic(&mut backend, &mut env, viewport);
    env.selected = Some(9);
    env.grid_scroll.set_position(420.0);
    // Selection, scroll, and a read-model row replacement are not navigation admission.
    let read_model_tx = ready_read_model(&mut env);
    let ctx = backend.context().clone();
    ctx.run_ui(Default::default(), |ui| env.poll_bookmark_browser(ui.ctx()))
        .drop_without_applying_deltas();
    assert!(read_model_tx.send(Err("duplicate".into())).is_err());
    assert_eq!(env.selected, Some(9));
    assert_eq!(env.grid_scroll.offset(), 420.0);
    assert!(matches!(
        env.pending_return_to_parent,
        FullscreenCloseContinuation::Terminal(_)
    ));
    assert!(env.take_pending_return_to_parent_nav().is_none());
    let Some(BookmarkViewState::Restoring { grid }) = env.bookmark_view_state.as_ref() else {
        panic!("existing bookmark restoration owns the grid");
    };
    assert_eq!(grid.selected_key, Some((0, 10)));
    assert_eq!(grid.scroll_offset_y, 420.0);
    assert_eq!(grid.opened_key, (0, 2));
    let grid = grid.clone();
    let keys: Vec<_> = env
        .bookmark_browser_rows
        .iter()
        .map(BookmarkBrowserRow::stable_key)
        .collect();
    env.restore_bookmark_view_grid(grid, &keys);
    assert_eq!(env.selected, Some(9));
    assert_eq!(env.grid_scroll.offset(), 420.0);
    assert!(env.take_pending_return_to_parent_nav().is_none());
}

#[test]
fn close_return_changed_bookmark_poll_preserves_live_main_position_before_consumer() {
    for book in [false, true] {
        for change in ["rename", "add", "remove"] {
            let mut env = app();
            let path = env
                .tmp
                .path()
                .join(if book { "book.pdf" } else { "target.mp4" });
            let target = if book {
                BookmarkViewReturnTarget::Book(path)
            } else {
                BookmarkViewReturnTarget::Media(path)
            };
            main_bookmarks(&mut env, target.clone());
            env.bookmark_view_sort = crate::bookmark_browser::BookmarkViewSort::CreatedAtAsc;
            active(&mut env, 94_003, target, None);
            let ctx = egui::Context::default();
            let viewport = register(&mut env, &ctx, 94_003);
            let mut backend = eframe::HeadlessEpiIntegration::new(ctx.clone());
            close_logic(&mut backend, &mut env, viewport);
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
                    *title = Some("renamed while viewer closed".into());
                }
                "add" => rows.push(row(env.tmp.path(), 31)),
                "remove" => {
                    rows.remove(0);
                }
                _ => unreachable!(),
            }
            let generation = env.items_generation;
            let tx = ready_read_model_with_rows(&mut env, rows);
            ctx.run_ui(Default::default(), |ui| env.poll_bookmark_browser(ui.ctx()))
                .drop_without_applying_deltas();
            assert!(tx.send(Err("already consumed".into())).is_err());
            assert_ne!(
                env.items_generation, generation,
                "{book}/{change}: changed result installed"
            );
            assert_eq!(
                env.selected
                    .map(|idx| env.bookmark_browser_rows[idx].stable_key()),
                Some(selected_key)
            );
            assert_eq!(
                env.grid_scroll.offset(),
                420.0,
                "{book}/{change}: live grid before consumer"
            );
            assert!(!env.grid_scroll.reveal_requested());
            assert!(matches!(
                env.pending_return_to_parent,
                FullscreenCloseContinuation::Terminal(_)
            ));

            assert!(env.take_pending_return_to_parent_nav().is_none());
            let Some(BookmarkViewState::Restoring { grid }) = env.bookmark_view_state.as_ref()
            else {
                panic!("existing bookmark restoration owns the adopted grid");
            };
            assert_eq!(grid.selected_key, Some(selected_key));
            assert_eq!(grid.scroll_offset_y, 420.0);
            assert_eq!(grid.opened_key, (0, 2));
            assert_eq!(
                grid.row_keys,
                env.bookmark_browser_rows
                    .iter()
                    .map(BookmarkBrowserRow::stable_key)
                    .collect::<Vec<_>>()
            );

            // The install starts the ordinary asynchronous page-index hydration.
            // Complete that owner before admitting another read-model result.
            env.finish_page_identity_preparation_for_test();
            assert_eq!(env.grid_scroll.offset(), 420.0);
            assert_eq!(
                env.selected
                    .map(|idx| env.bookmark_browser_rows[idx].stable_key()),
                Some(selected_key)
            );

            // A further changed result exercises real install/restoration after the
            // consumer too. Old pre-install keys would restore the originally opened row.
            let mut rows = env.bookmark_browser_rows.clone();
            let crate::bookmark_browser::BookmarkRowSource::Media { title, .. } =
                &mut rows[0].source
            else {
                unreachable!()
            };
            *title = Some("next read-model revision".into());
            let generation = env.items_generation;
            let tx = ready_read_model_with_rows(&mut env, rows);
            ctx.run_ui(Default::default(), |ui| env.poll_bookmark_browser(ui.ctx()))
                .drop_without_applying_deltas();
            assert!(tx.send(Err("already consumed".into())).is_err());
            assert_ne!(env.items_generation, generation);
            env.finish_page_identity_preparation_for_test();
            assert_eq!(
                env.selected
                    .map(|idx| env.bookmark_browser_rows[idx].stable_key()),
                Some(selected_key)
            );
            assert_eq!(
                env.grid_scroll.offset(),
                420.0,
                "{book}/{change}: restored adopted grid"
            );
            assert!(!env.grid_scroll.reveal_requested());
            assert!(!env.pending_return_to_parent.is_pending());
            assert!(env.take_pending_return_to_parent_nav().is_none());
        }
    }
}

#[test]
fn close_return_empty_bookmark_poll_adopts_empty_live_grid() {
    let mut env = app();
    let target = BookmarkViewReturnTarget::Media(env.tmp.path().join("target.mp4"));
    main_bookmarks(&mut env, target.clone());
    active(&mut env, 94_006, target, None);
    let ctx = egui::Context::default();
    let viewport = register(&mut env, &ctx, 94_006);
    let mut backend = eframe::HeadlessEpiIntegration::new(ctx.clone());
    close_logic(&mut backend, &mut env, viewport);
    env.grid_scroll.set_position(420.0);
    let generation = env.items_generation;
    let tx = ready_read_model_with_rows(&mut env, vec![]);
    ctx.run_ui(Default::default(), |ui| env.poll_bookmark_browser(ui.ctx()))
        .drop_without_applying_deltas();
    assert!(tx.send(Err("already consumed".into())).is_err());
    assert_ne!(env.items_generation, generation, "all rows were removed");
    assert!(env.bookmark_browser_rows.is_empty());
    assert!(env.items.is_empty());
    assert_eq!(env.selected, None);
    assert_eq!(env.grid_scroll.offset(), 420.0);
    assert!(env.take_pending_return_to_parent_nav().is_none());
    let Some(BookmarkViewState::Restoring { grid }) = env.bookmark_view_state.as_ref() else {
        panic!("the return consumer must adopt the live empty grid");
    };
    assert!(
        grid.row_keys.is_empty(),
        "deleted rows cannot return as saved keys"
    );
    assert_eq!(grid.selected_key, None);
    assert_eq!(grid.opened_key, (0, 2));
    assert_eq!(grid.scroll_offset_y, 420.0);
    env.finish_page_identity_preparation_for_test();

    // The unchanged-empty read model consumes Restoring without resurrecting rows.
    let tx = ready_read_model_with_rows(&mut env, vec![]);
    ctx.run_ui(Default::default(), |ui| env.poll_bookmark_browser(ui.ctx()))
        .drop_without_applying_deltas();
    assert!(tx.send(Err("already consumed".into())).is_err());
    assert!(env.bookmark_view_state.is_none());
    assert!(env.bookmark_browser_rows.is_empty());
    assert!(env.items.is_empty());
    assert_eq!(env.selected, None);
    assert_eq!(env.grid_scroll.offset(), 420.0);
    assert!(!env.pending_return_to_parent.is_pending());
}

#[test]
fn close_return_bookmark_delete_receipt_preserves_live_main_position_before_consumer() {
    let mut env = app();
    let target = BookmarkViewReturnTarget::Media(env.tmp.path().join("target.mp4"));
    main_bookmarks(&mut env, target.clone());
    env.bookmark_view_sort = crate::bookmark_browser::BookmarkViewSort::CreatedAtAsc;
    active(&mut env, 94_004, target, None);
    let ctx = egui::Context::default();
    let viewport = register(&mut env, &ctx, 94_004);
    let mut backend = eframe::HeadlessEpiIntegration::new(ctx.clone());
    close_logic(&mut backend, &mut env, viewport);
    env.selected = Some(9);
    env.grid_scroll.set_position(420.0);
    let selected_key = env.bookmark_browser_rows[9].stable_key();
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
    assert_eq!(env.bookmark_browser_rows[8].stable_key(), selected_key);
    assert_eq!(env.grid_scroll.offset(), 420.0);
    assert!(!env.grid_scroll.reveal_requested());
    assert!(env.take_pending_return_to_parent_nav().is_none());
    let Some(BookmarkViewState::Restoring { grid }) = &env.bookmark_view_state else {
        panic!("bookmark return must own the updated live grid");
    };
    assert_eq!(grid.selected_key, Some(selected_key));
    assert_eq!(grid.scroll_offset_y, 420.0);
}

#[test]
fn close_return_same_path_navigation_and_new_bookmark_invalidate_old_origins_at_admission() {
    for new_bookmark in [false, true] {
        let mut env = app();
        let path = env.tmp.path().join("target.mp4");
        let target = BookmarkViewReturnTarget::Media(path);
        main_bookmarks(&mut env, target.clone());
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
fn close_return_remote_acquire_and_return_never_resurrects_terminal_request() {
    use crate::remote_ipc::session::{RemoteControlPhase, SessionHandle};
    let mut env = app();
    let target = BookmarkViewReturnTarget::Book(env.tmp.path().join("book.pdf"));
    main_bookmarks(&mut env, target.clone());
    active(&mut env, 94_005, target, None);
    let ctx = egui::Context::default();
    let viewport = register(&mut env, &ctx, 94_005);
    let mut backend = eframe::HeadlessEpiIntegration::new(ctx);
    close_logic(&mut backend, &mut env, viewport);
    assert!(env.pending_return_to_parent.is_pending());
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
    assert!(env.bookmark_browser_pending.is_none());
    assert!(env.take_pending_return_to_parent_nav().is_none());
    session.local_disconnect();
    for _ in 0..3 {
        let _ = backend.update_logic_only(&mut env.app, Default::default());
    }
    assert_eq!(session.snapshot().phase, RemoteControlPhase::Local);
    assert!(!env.pending_return_to_parent.is_pending());
    full(&mut backend, &mut env);
    assert!(!env.pending_return_to_parent.is_pending());
    assert!(env.take_pending_return_to_parent_nav().is_none());
}

#[test]
fn close_return_parked_sibling_full_cannot_consume_surviving_main_request() {
    let mut env = app();
    let target = BookmarkViewReturnTarget::Book(env.tmp.path().join("book.pdf"));
    main_bookmarks(&mut env, target.clone());
    let ctx = egui::Context::default();
    let old = env.push_window_context_for_test(&ctx, 94_006, |viewer| {
        viewer.bookmark_view_state = Some(BookmarkViewState::Detached { target });
        viewer.current_folder = Some(PathBuf::from("C:/fixture/book.pdf"));
    });
    env.update_detached_window_runtime_flags(94_006, false, "close_return_parked");
    env.transition_detached_window_state(
        94_006,
        DetachedWindowState::Parked,
        "close_return_parked",
    );
    let sibling = env.push_window_context_for_test(&ctx, 94_007, |_| {});
    env.update_detached_window_runtime_flags(94_007, false, "close_return_sibling");
    let viewport = register(&mut env, &ctx, 94_006);
    let mut backend = eframe::HeadlessEpiIntegration::new(ctx);
    assert_eq!(
        close_logic(&mut backend, &mut env, viewport),
        eframe::ViewportCloseResponse::RetirementTransferred
    );
    assert!(env.with_viewer_context_ref(old, |_| ()).is_none());
    assert!(matches!(
        env.pending_return_to_parent,
        FullscreenCloseContinuation::Terminal(_)
    ));
    env.with_viewer_context(sibling, |viewer| full(&mut backend, viewer))
        .unwrap();
    assert!(matches!(
        env.pending_return_to_parent,
        FullscreenCloseContinuation::Terminal(_)
    ));
    assert_eq!(
        env.locate_window_context(94_007).map(|(id, _)| id),
        Some(sibling)
    );
    assert!(env.take_pending_return_to_parent_nav().is_none());
    assert!(env.bookmark_browser_pending.is_some());
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
            opened_key: (1, 4),
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
    assert_eq!(grid.opened_key, (1, 4));
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

#[test]
fn close_return_replaced_remote_and_app_exit_causes_never_publish_user_return() {
    for cause in [
        ViewerCloseCause::Replaced,
        ViewerCloseCause::RemoteAcquire,
        ViewerCloseCause::AppExit,
    ] {
        let mut env = app();
        let target = BookmarkViewReturnTarget::Book(env.tmp.path().join("book.pdf"));
        main_bookmarks(&mut env, target.clone());
        let (owner, _) = active(&mut env, 94_008, target, None);
        env.transfer_terminal_close_return(owner, cause);
        assert!(!env.pending_return_to_parent.is_pending());
        assert!(matches!(
            env.bookmark_view_state,
            Some(BookmarkViewState::Opening { .. })
        ));
        env.retire_close_returns_for_main_admission();
        env.transfer_terminal_close_return(owner, ViewerCloseCause::User);
        assert!(!env.pending_return_to_parent.is_pending());
    }
}

#[test]
fn close_return_native_hard_close_flag_reaches_production_poll_before_teardown() {
    let mut env = app();
    let ctx = egui::Context::default();
    let path = env.tmp.path().join("native-closed.mp4");
    let target = BookmarkViewReturnTarget::Media(path.clone());
    main_bookmarks(&mut env, target.clone());
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
    let generation = env.items_generation;
    let selected = env.selected;
    let scroll = env.grid_scroll.offset();
    // No synthetic Close event/helper: poll_video sees the real existing native closed flag.
    env.with_viewer_context(owner, |viewer| viewer.poll_video(&ctx))
        .unwrap();
    assert!(matches!(
        env.pending_return_to_parent,
        FullscreenCloseContinuation::Terminal(_)
    ));
    assert_eq!(env.items_generation, generation);
    assert_eq!(env.selected, selected);
    assert_eq!(env.grid_scroll.offset(), scroll);
    assert!(env.bookmark_browser_pending.is_none());
    assert!(env.take_pending_return_to_parent_nav().is_none());
    assert!(matches!(
        env.bookmark_view_state,
        Some(BookmarkViewState::Restoring { .. })
    ));
    assert!(env.bookmark_browser_pending.is_some());
    assert!(env.take_pending_return_to_parent_nav().is_none());
}

#[test]
fn close_return_loading_and_holdover_shell_escape_and_close_use_actual_child_ui() {
    for shown in [false, true] {
        for os_close in [false, true] {
            // egui's immediate renderer is thread-local; isolate its fake registration from
            // ordinary headless tests just as the established backstop fixture does.
            std::thread::spawn(move || {
                let mut env = app();
                let ctx = egui::Context::default();
                let target = BookmarkViewReturnTarget::Book(env.tmp.path().join("loading.pdf"));
                main_bookmarks(&mut env, target.clone());
                let (owner, _) = active(&mut env, 94_010, target, None);
                env.with_viewer_context_state(owner, |viewer| {
                    viewer.fullscreen_idx = None;
                    viewer.fs_viewport_shown = shown;
                    viewer.fs_viewport_presentation =
                        shown.then_some(ViewerPresentation::DetachedWindow);
                    viewer.window_visible = false;
                })
                .unwrap();
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
                assert!(
                    matches!(
                        env.pending_return_to_parent,
                        FullscreenCloseContinuation::Terminal(_)
                    ),
                    "shown={shown}, os_close={os_close}"
                );
                assert!(env.bookmark_browser_pending.is_none());
                assert!(env.take_pending_return_to_parent_nav().is_none());
                assert!(env.bookmark_browser_pending.is_some());
                assert!(env.take_pending_return_to_parent_nav().is_none());
            })
            .join()
            .unwrap();
        }
    }
}
