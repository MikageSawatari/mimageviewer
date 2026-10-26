use super::*;
use tests::phase_c_support::setup_app;

fn snapshot(owner: ViewerContextId, paths: &[PathBuf]) -> Arc<ListOrderSnapshot> {
    Arc::new(ListOrderSnapshot {
        owner,
        entries: paths
            .iter()
            .map(|path| ListOrderEntry {
                path: path.clone(),
                kind: ListOrderEntryKind::Folder,
            })
            .collect::<Vec<_>>()
            .into(),
    })
}

fn fixture(app: &mut App, root: &Path) -> (PathBuf, PathBuf) {
    let a = root.join("z-first");
    let b = root.join("a-next");
    for folder in [&a, &b] {
        std::fs::create_dir_all(folder).unwrap();
        image::RgbImage::new(2, 2)
            .save(folder.join("page.png"))
            .unwrap();
    }
    app.current_folder = Some(a.clone());
    app.items = vec![GridItem::Image(a.join("page.png"))];
    app.visible_indices = vec![0];
    app.thumbnails = vec![ThumbnailState::Pending];
    app.fullscreen_idx = Some(0);
    app.list_order = Some(snapshot(app.viewer_context_main(), &[a.clone(), b.clone()]));
    (a, b)
}

fn receive(app: &mut App) -> FolderNavResult {
    let pending = app.folder_nav_pending.take().expect("accepted navigation");
    let result = pending
        .rx
        .recv_timeout(std::time::Duration::from_secs(15))
        .expect("worker result");
    FolderNavResult {
        target: result.target,
        hit_image_folder: result.hit_image_folder,
        forward: pending.forward,
        mode: pending.mode,
        queued_steps: app.pending_folder_nav_steps,
    }
}

fn fake_pending(
    app: &mut App,
    mode: FolderNavMode,
) -> (Arc<AtomicBool>, mpsc::Sender<FolderNavThreadResult>) {
    let (tx, rx) = mpsc::channel();
    let cancel = Arc::new(AtomicBool::new(false));
    app.folder_nav_pending = Some(FolderNavPending {
        cancel: cancel.clone(),
        rx,
        forward: true,
        mode,
    });
    (cancel, tx)
}

#[test]
fn list_order_worker_skips_empty_missing_and_container_only_folders_without_dfs_or_wrap() {
    let root = tempfile::tempdir().unwrap();
    let start = root.path().join("start");
    let empty = root.path().join("empty");
    let containers = root.path().join("containers");
    let missing = root.path().join("missing");
    let audio = root.path().join("audio");
    for dir in [&start, &empty, &containers, &audio] {
        std::fs::create_dir(dir).unwrap();
    }
    std::fs::write(containers.join("only.pdf"), b"pdf").unwrap();
    std::fs::create_dir(empty.join("descendant")).unwrap();
    std::fs::write(empty.join("descendant/page.png"), b"png").unwrap();
    std::fs::write(audio.join("track.mp3"), b"audio").unwrap();
    let order = snapshot(
        ViewerContextId::for_test(1),
        &[start, empty, containers, missing, audio.clone()],
    );
    let cancel = AtomicBool::new(false);
    let opts = crate::folder_tree::FolderTreeOptions::default();
    let next = list_order_navigation::navigate_list_order(
        &order,
        0,
        true,
        opts.clone(),
        &cancel,
        None,
        None,
    )
    .unwrap();
    assert_eq!(next.path, audio);
    assert!(next.hit_image_folder);
    assert!(
        list_order_navigation::navigate_list_order(
            &order,
            4,
            false,
            opts.clone(),
            &cancel,
            None,
            None
        )
        .is_none()
    );
    assert!(
        list_order_navigation::navigate_list_order(
            &order,
            4,
            true,
            opts.clone(),
            &cancel,
            None,
            None
        )
        .is_none()
    );
    assert!(
        list_order_navigation::navigate_list_order(
            &order,
            0,
            false,
            opts.clone(),
            &cancel,
            None,
            None
        )
        .is_none()
    );
    cancel.store(true, Ordering::Relaxed);
    assert!(
        list_order_navigation::navigate_list_order(&order, 0, true, opts, &cancel, None, None)
            .is_none()
    );
}

#[test]
fn list_order_handler_worker_prescans_and_lands_in_frozen_order_then_finds_previous() {
    let mut app = setup_app();
    let root = app.tmp.path().to_path_buf();
    let (a, b) = fixture(&mut app, &root);
    let ctx = egui::Context::default();
    let order = app.list_order.clone().unwrap();
    app.handle_fullscreen_list_order_nav_context(&ctx, 0, true, false);
    assert!(app.fs_navigation_sequence_blocks_new_target());
    assert!(app.inflight_fullscreen_folder_nav_mode(false).is_none());
    let result = receive(&mut app);
    assert_eq!(result.target.as_ref().unwrap().logical_source(), b);
    assert!(matches!(
        result.target.as_ref().unwrap().scanned,
        FolderNavScanResult::Ready(_)
    ));
    assert_eq!(
        StartupListIntent::folder_navigation(&result.mode),
        StartupListIntent::PageContinuation
    );
    assert!(matches!(
        result.mode.history_trigger(),
        HistoryTrigger::UserChosen
    ));
    app.apply_folder_nav_result(&ctx, result);
    app.finish_page_identity_preparation_for_test();
    assert_eq!(app.effective_folder(), Some(b));
    assert_eq!(app.fullscreen_idx, Some(0));
    assert!(Arc::ptr_eq(app.list_order.as_ref().unwrap(), &order));
    assert_eq!(app.list_order_position().unwrap().1, 1);
    app.release_fs_nav_lock();
    app.handle_fullscreen_list_order_nav_context(&ctx, 0, false, false);
    let result = receive(&mut app);
    assert_eq!(result.target.as_ref().unwrap().logical_source(), a);
}

#[test]
fn list_order_gate_rejects_both_directions_and_ctrl_through_sequence_and_display_wait() {
    let mut app = setup_app();
    let root = app.tmp.path().to_path_buf();
    let (a, _) = fixture(&mut app, &root);
    let ctx = egui::Context::default();
    let order = app.list_order.clone().unwrap();
    for sequence in [false, true] {
        for mode in [
            FolderNavMode::Fullscreen,
            FolderNavMode::ListOrder {
                snapshot: order.clone(),
                from_index: 0,
            },
        ] {
            let (cancel, _sender) = fake_pending(&mut app, mode.clone());
            app.fs_nav_locked_gen = Some(app.items_generation);
            if sequence {
                app.begin_fs_folder_navigation_sequence(&ctx, 0);
            }
            for forward in [true, false] {
                app.handle_fullscreen_list_order_nav_context(&ctx, 0, forward, false);
                if matches!(mode, FolderNavMode::ListOrder { .. }) {
                    app.handle_fullscreen_ctrl_nav_context(&ctx, 0, forward, false);
                    app.handle_fullscreen_sibling_nav_context(&ctx, 0, forward, false);
                }
                assert!(!cancel.load(Ordering::Relaxed));
                assert!(Arc::ptr_eq(
                    &app.folder_nav_pending.as_ref().unwrap().cancel,
                    &cancel
                ));
                assert_eq!(app.pending_folder_nav_steps, 0);
            }
            // Common admission cannot cancel/restart ListOrder or replace another traversal
            // with ListOrder, even if a caller bypasses the fullscreen handler.
            let incoming = if matches!(mode, FolderNavMode::ListOrder { .. }) {
                FolderNavMode::Fullscreen
            } else {
                FolderNavMode::ListOrder {
                    snapshot: order.clone(),
                    from_index: 0,
                }
            };
            app.start_folder_nav(a.clone(), true, incoming);
            assert!(!cancel.load(Ordering::Relaxed));
            assert_eq!(app.pending_folder_nav_steps, 0);
            app.folder_nav_pending = None;
            app.release_fs_nav_lock();
        }
    }
    // Preparation can outlive the worker during hydration/display confirmation.
    app.begin_fs_folder_navigation_sequence(&ctx, 0);
    app.handle_fullscreen_list_order_nav_context(&ctx, 0, true, false);
    assert!(app.folder_nav_pending.is_none());
    app.release_fs_nav_lock();
    app.fs_nav_locked_gen = Some(app.items_generation);
    app.handle_fullscreen_list_order_nav_context(&ctx, 0, true, false);
    assert!(app.folder_nav_pending.is_none());
}

/// Reach the actual post-worker media downgrade without rendering or desktop input.
/// The prepared list is adopted by the same navigation/hydration path as a real landing.
fn video_display_wait(
    app: &mut tests::phase_c_support::AppTestEnv,
    ctx: &egui::Context,
    list_order: bool,
) {
    let root = app.tmp.path().join("video-books");
    let (a, b) = fixture(app, &root);
    std::fs::remove_file(b.join("page.png")).unwrap();
    std::fs::write(b.join("video.mp4"), b"video").unwrap();
    if list_order {
        app.handle_fullscreen_list_order_nav_context(ctx, 0, true, false);
    } else {
        app.begin_fs_folder_navigation_sequence(ctx, 0);
        app.start_folder_nav(a, false, FolderNavMode::Fullscreen);
    }
    let result = receive(app);
    assert_eq!(result.target.as_ref().unwrap().logical_source(), b);
    app.apply_folder_nav_result(ctx, result);
    app.finish_page_identity_preparation_for_test();
    assert!(app.folder_nav_pending.is_none());
    assert_eq!(app.current_folder, Some(b));
    assert!(matches!(app.items[0], GridItem::Video(_)));
    assert_eq!(app.fullscreen_idx, Some(0));
    assert!(app.fs_nav_is_locked());
    assert!(!app.bind_fs_navigation_sequence_to_current_target());
    // Media uses the legacy display-wait owner, rather than a page presentation trace.
    assert!(!app.fs_navigation_sequence_blocks_new_target());
}

#[test]
fn list_order_video_display_wait_rejects_ctrl_without_worker() {
    for forward in [false, true] {
        let mut app = setup_app();
        let ctx = egui::Context::default();
        video_display_wait(&mut app, &ctx, true);
        app.handle_fullscreen_ctrl_nav_context(&ctx, 0, forward, false);
        assert!(app.folder_nav_pending.is_none(), "must not start DFS");
        assert_eq!(app.pending_folder_nav_steps, 0);
        assert!(app.fs_nav_is_locked());
        app.release_fs_nav_lock();
        app.handle_fullscreen_ctrl_nav_context(&ctx, 0, forward, false);
        assert!(app.folder_nav_pending.is_some(), "release ends rejection");
    }
}

#[test]
fn list_order_video_display_wait_rejects_sibling_without_worker() {
    for forward in [false, true] {
        let mut app = setup_app();
        let ctx = egui::Context::default();
        video_display_wait(&mut app, &ctx, true);
        app.handle_fullscreen_sibling_nav_context(&ctx, 0, forward, false);
        assert!(
            app.folder_nav_pending.is_none(),
            "must not start sibling DFS"
        );
        assert_eq!(app.pending_folder_nav_steps, 0);
        assert!(app.fs_nav_is_locked());
    }
}

#[test]
fn list_order_video_display_wait_rejects_list_order_and_common_admission_without_worker() {
    let mut app = setup_app();
    let ctx = egui::Context::default();
    video_display_wait(&mut app, &ctx, true);
    for forward in [false, true] {
        app.handle_fullscreen_list_order_nav_context(&ctx, 0, forward, false);
        assert!(app.folder_nav_pending.is_none());
        let current = app.effective_folder().unwrap();
        app.start_folder_nav(current, forward, FolderNavMode::Fullscreen);
        assert!(
            app.folder_nav_pending.is_none(),
            "common gate retains ownership"
        );
        assert_eq!(app.pending_folder_nav_steps, 0);
        assert!(app.fs_nav_is_locked());
    }
}

#[test]
fn list_order_video_display_wait_preserves_ordinary_fullscreen_continuations() {
    for sibling in [false, true] {
        for forward in [false, true] {
            let mut app = setup_app();
            let ctx = egui::Context::default();
            video_display_wait(&mut app, &ctx, false);
            if sibling {
                app.handle_fullscreen_sibling_nav_context(&ctx, 0, forward, false);
            } else {
                app.handle_fullscreen_ctrl_nav_context(&ctx, 0, forward, false);
            }
            let pending = app
                .folder_nav_pending
                .as_ref()
                .expect("legacy continuation");
            assert_eq!(pending.forward, forward);
            assert_eq!(
                matches!(pending.mode, FolderNavMode::SiblingFullscreen),
                sibling
            );
            assert!(app.fs_nav_is_locked());
        }
    }
}

#[test]
fn list_order_owner_survives_recapture_and_drawable_release_until_terminal_finish() {
    let mut app = setup_app();
    let ctx = egui::Context::default();
    video_display_wait(&mut app, &ctx, true);
    assert!(app.fs_list_order_navigation_blocks_new_target());
    app.capture_fs_nav_holdover(0);
    assert!(
        app.fs_list_order_navigation_blocks_new_target(),
        "recapture keeps origin"
    );
    app.items_generation += 1;
    let texture = ctx.load_texture(
        "list-order-ready-video-thumb",
        egui::ColorImage::filled([1, 1], egui::Color32::WHITE),
        egui::TextureOptions::LINEAR,
    );
    app.thumbnails[0] = ThumbnailState::loaded_raster(
        texture,
        crate::thumb_loader::ThumbLoadOrigin::SourceGenerated {
            evaluated_display_px: 64,
        },
        false,
        64,
        crate::source_coverage::SourceCoverage::FullSource,
        None,
        None,
    );
    assert!(app.fs_nav_holdover_for_draw().is_none());
    assert!(matches!(
        app.fs_holdover_tex,
        Some(FsHoldover::FolderNavigation(
            None,
            FsFolderNavigationOrigin::ListOrder
        ))
    ));
    assert!(
        app.fs_list_order_navigation_blocks_new_target(),
        "pixels do not own admission"
    );
    app.handle_fullscreen_ctrl_nav_context(&ctx, 0, true, false);
    assert!(app.folder_nav_pending.is_none());
    app.finish_fs_navigation_sequence(FsNavigationSequenceFinish::RequestFailed);
    assert!(!app.fs_list_order_navigation_blocks_new_target());
    assert!(!app.fs_nav_is_locked());
}

#[test]
fn list_order_owner_follows_still_display_sequence_and_is_cleared_on_release() {
    let mut app = setup_app();
    let root = app.tmp.path().to_path_buf();
    fixture(&mut app, &root);
    let ctx = egui::Context::default();
    app.handle_fullscreen_list_order_nav_context(&ctx, 0, true, false);
    let result = receive(&mut app);
    assert!(app.fs_list_order_navigation_blocks_new_target());
    app.apply_folder_nav_result(&ctx, result);
    app.finish_page_identity_preparation_for_test();
    assert!(app.bind_fs_navigation_sequence_to_current_target());
    assert!(app.fs_list_order_navigation_blocks_new_target());
    assert!(app.fs_navigation_sequence_blocks_new_target());
    app.release_fs_nav_lock();
    assert!(!app.fs_list_order_navigation_blocks_new_target());
}

#[test]
fn list_order_terminal_rendition_failure_admits_ctrl_sibling_and_common_targets() {
    for route in 0..5 {
        for forward in [false, true] {
            let mut app = setup_app();
            let root = app.tmp.path().to_path_buf();
            fixture(&mut app, &root);
            let ctx = egui::Context::default();
            app.handle_fullscreen_list_order_nav_context(&ctx, 0, true, false);
            let _result = receive(&mut app);
            // State-transition seam: an accepted page target has no usable rendition.
            // The production resolver must end the lock while retaining its old pixels.
            let generation = app.items_generation;
            app.fs_holdover_tex
                .as_mut()
                .unwrap()
                .navigation_sequence_mut()
                .unwrap()
                .target = FsNavigationSequenceTarget::Display(FsNavigationDisplayTarget {
                items_generation: generation,
                anchor_idx: 0,
                accept_rendition: true,
                phase: FsNavigationTargetPhase::Awaiting {
                    demand: FsNavigationDisplayDemand::navigation_only(0, vec![0]),
                },
            });
            app.thumbnails[0] = ThumbnailState::Failed;
            app.poll_fs_nav_lock(&ctx);
            let owner = app
                .fs_holdover_tex
                .as_ref()
                .expect("failure keeps the display owner");
            assert_eq!(
                owner.folder_navigation_origin(),
                FsFolderNavigationOrigin::ListOrder
            );
            assert!(matches!(
                owner.navigation_sequence().unwrap().target,
                FsNavigationSequenceTarget::Display(FsNavigationDisplayTarget {
                    phase: FsNavigationTargetPhase::RenditionFailed { .. },
                    ..
                })
            ));
            assert!(!app.fs_nav_is_locked());
            assert!(!app.fs_list_order_navigation_blocks_new_target());
            match route {
                0 => app.handle_fullscreen_ctrl_nav_context(&ctx, 0, forward, false),
                1 => app.handle_fullscreen_sibling_nav_context(&ctx, 0, forward, false),
                2 => {
                    let current = app.effective_folder().unwrap();
                    app.start_folder_nav(current, forward, FolderNavMode::Fullscreen);
                }
                3 => app.handle_fullscreen_list_order_nav_context(&ctx, 0, forward, false),
                _ => {
                    // The existing slideshow path recaptures before requesting its next DFS.
                    app.capture_fs_nav_holdover(0);
                    assert_eq!(
                        app.fs_holdover_tex
                            .as_ref()
                            .unwrap()
                            .folder_navigation_origin(),
                        FsFolderNavigationOrigin::Ordinary
                    );
                    let current = app.effective_folder().unwrap();
                    app.start_folder_nav(current, forward, FolderNavMode::SlideshowNext);
                    assert!(matches!(
                        app.folder_nav_pending.as_ref().map(|pending| &pending.mode),
                        Some(FolderNavMode::SlideshowNext)
                    ));
                }
            }
            assert!(
                app.folder_nav_pending.is_some(),
                "terminal failure admits route {route}"
            );
        }
    }
}

#[test]
fn list_order_boundary_keeps_viewer_and_snapshot_and_releases_gate() {
    let mut app = setup_app();
    let root = app.tmp.path().to_path_buf();
    let (a, _) = fixture(&mut app, &root);
    let ctx = egui::Context::default();
    let order = app.list_order.clone().unwrap();
    for forward in [false, true] {
        app.begin_fs_folder_navigation_sequence(&ctx, 0);
        app.apply_folder_nav_result(
            &ctx,
            FolderNavResult {
                target: None,
                hit_image_folder: false,
                forward,
                mode: FolderNavMode::ListOrder {
                    snapshot: order.clone(),
                    from_index: 0,
                },
                queued_steps: 0,
            },
        );
        assert!(
            matches!(app.fs_boundary_hint, Some(crate::ui_fullscreen::FsBoundaryHint::Edge { scope: crate::ui_fullscreen::FsBoundaryEdgeScope::ListOrder, at_end, .. }) if at_end == forward)
        );
        let title = crate::ui_fullscreen::FsBoundaryEdgeScope::ListOrder.title(forward);
        assert_eq!(
            title,
            if forward {
                "一覧の末尾です"
            } else {
                "一覧の先頭です"
            }
        );
        #[cfg(windows)]
        assert_eq!(
            App::native_boundary_hint_text(app.fs_boundary_hint.unwrap()),
            title
        );
        assert_eq!(app.effective_folder(), Some(a.clone()));
        assert_eq!(app.fullscreen_idx, Some(0));
        assert!(!app.fs_nav_is_locked());
        assert!(!app.fs_navigation_sequence_blocks_new_target());
        assert!(Arc::ptr_eq(app.list_order.as_ref().unwrap(), &order));
    }
}

#[test]
fn list_order_unavailable_notifies_without_starting_or_touching_dfs() {
    let mut app = setup_app();
    let ctx = egui::Context::default();
    app.handle_fullscreen_list_order_nav_context(&ctx, 0, true, false);
    assert!(app.folder_nav_pending.is_none());
    assert_eq!(
        app.fs_feedback_toast.as_ref().map(|x| x.0.as_str()),
        Some("一覧から開いた本で使えます")
    );
    assert!(app.fs_holdover_tex.is_none());
}

#[cfg(windows)]
struct ClearInput;
#[cfg(windows)]
impl Drop for ClearInput {
    fn drop(&mut self) {
        crate::key_input::clear_test_frame();
    }
}

#[cfg(windows)]
fn input_frame(
    app: &mut App,
    ctx: &egui::Context,
    vk: u32,
    repeat: bool,
    ctrl: bool,
    shift: bool,
) -> crate::ui_fullscreen::FsKeyAction {
    crate::key_input::set_test_frame(vec![crate::key_input::KeyEdge {
        operation_id: crate::key_input::KeyOperationId::unassigned(),
        source_hwnd: 1,
        source_viewport: egui::ViewportId::ROOT,
        virtual_key: vk,
        scan_code: 0,
        extended: false,
        pressed: true,
        repeat,
        ctrl,
        shift,
        alt: false,
    }]);
    ctx.begin_pass(egui::RawInput {
        focused: true,
        ..Default::default()
    });
    let result = app.handle_fs_key_input(ctx, 0, false);
    if result.list_order_nav.is_some() {
        app.handle_fs_navigation_with_list_order_operation(
            ctx,
            result.close,
            result.close_to_page_list,
            result.ctrl_nav,
            result.sibling_nav,
            None,
            result.page_nav,
            result.jump_to,
            0,
            result.list_order_nav,
            None,
        );
    }
    ctx.end_pass().drop_without_applying_deltas();
    result
}

#[cfg(windows)]
#[test]
fn list_order_egui_handler_routes_default_remap_and_preserves_ctrl_and_sibling_actions() {
    let _serial = crate::key_input::lock_test_input();
    let _clear = ClearInput;
    let mut app = setup_app();
    let root = app.tmp.path().to_path_buf();
    fixture(&mut app, &root);
    let ctx = egui::Context::default();
    assert_eq!(
        input_frame(&mut app, &ctx, 0x28, false, true, false).ctrl_nav,
        Some(1)
    );
    assert_eq!(
        input_frame(&mut app, &ctx, 0x22, false, true, false).sibling_nav,
        Some(1)
    );
    assert!(app.folder_nav_pending.is_none());
    input_frame(&mut app, &ctx, 0x28, true, true, true);
    // EachPhysicalPress retains Ctrl traversal's existing repeat-only-frame
    // dispatch. Once preparation starts, further presses/repeats are refused.
    let first_request = app.folder_nav_pending.as_ref().unwrap().cancel.clone();
    input_frame(&mut app, &ctx, 0x28, true, true, true);
    input_frame(&mut app, &ctx, 0x28, false, true, true);
    assert!(Arc::ptr_eq(
        &app.folder_nav_pending.as_ref().unwrap().cancel,
        &first_request
    ));
    assert!(!first_request.load(Ordering::Relaxed));
    assert_eq!(app.pending_folder_nav_steps, 0);
    assert!(matches!(
        app.folder_nav_pending.as_ref().map(|p| &p.mode),
        Some(FolderNavMode::ListOrder { from_index: 0, .. })
    ));
    app.cancel_pending_folder_nav();
    app.release_fs_nav_lock();
    app.keymap = crate::keymap::Keymap::from_ini_str("[FsCommon]\nFsListOrderNext = F17\n");
    input_frame(&mut app, &ctx, 0x28, false, true, true);
    assert!(app.folder_nav_pending.is_none());
    input_frame(&mut app, &ctx, 0x80, false, false, false);
    assert!(matches!(
        app.folder_nav_pending.as_ref().map(|p| &p.mode),
        Some(FolderNavMode::ListOrder { .. })
    ));
}

#[cfg(windows)]
#[test]
fn list_order_egui_handler_respects_text_and_modal_ownership() {
    let _serial = crate::key_input::lock_test_input();
    let _clear = ClearInput;
    let mut app = setup_app();
    let root = app.tmp.path().to_path_buf();
    fixture(&mut app, &root);
    let ctx = egui::Context::default();
    let field = egui::Id::unique("list-order-text");
    ctx.begin_pass(Default::default());
    ctx.root_ui(|root_ui| {
        egui::CentralPanel::default().show(root_ui, |ui| {
            crate::ime_focus::add_singleline(ui, &mut String::new(), None, |edit| edit.id(field))
                .request_focus();
        });
    });
    ctx.end_pass().drop_without_applying_deltas();
    input_frame(&mut app, &ctx, 0x28, false, true, true);
    assert!(app.folder_nav_pending.is_none());
    ctx.memory_mut(|memory| memory.surrender_focus(field));
    app.show_preferences = true;
    input_frame(&mut app, &ctx, 0x28, false, true, true);
    assert!(app.folder_nav_pending.is_none());
    app.show_preferences = false;
    input_frame(&mut app, &ctx, 0x28, false, true, true);
    assert!(app.folder_nav_pending.is_some());
}
