//! CPU/egui handler tests. No native application, model, or GPU is started.
use super::detached_window_manager::{DetachedWindowPurpose, HostPlacement};
use super::tests::phase_c_support::{AppTestEnv, setup_app};
use super::*;
use crate::detached_window_geometry::{PhysicalRect, PhysicalWindowPlacement};

fn fixture() -> AppTestEnv {
    let mut app = setup_app();
    app.initialized = true;
    app.active_quick_folder_slot = None;
    app.settings.detached_viewer_enabled = false;
    app.settings.detached_viewer_open_images_in_window = false;
    let folder = app.tmp.path().join("browser818-presentation");
    std::fs::create_dir(&folder).unwrap();
    for name in ["a.png", "b.png"] {
        image::RgbaImage::new(2, 2).save(folder.join(name)).unwrap();
    }
    app.current_folder = Some(folder.clone());
    app.items = vec![
        GridItem::Image(folder.join("a.png")),
        GridItem::Image(folder.join("b.png")),
    ];
    app.thumbnails = vec![ThumbnailState::Pending; 2];
    app.image_metas = vec![None; 2];
    app.visible_indices = vec![0, 1];
    app.selected = Some(0);
    app.page_identity_index = Arc::new(file_type_resume::PreparedPageIdentityIndex::prepare(
        &app.items,
        None,
        app.settings.file_type_visibility.clone(),
    ));
    for idx in 0..2 {
        app.adjustment_page_params
            .insert(idx, crate::adjustment::AdjustParams::default());
        app.fs_cache.insert(idx, FsCacheEntry::Failed);
    }
    app
}

fn browser(app: &App) -> ViewerOpenAdoption {
    let mut placement = app.default_detached_viewer_window_placement();
    placement.outer.left += 8;
    placement.outer.right += 8;
    assert!(App::detached_window_runtime_placement_is_usable(placement));
    ViewerOpenAdoption::requested(ViewerOpenRequest::new(ViewerOpenIntent::BrowserOverlay {
        placement,
    }))
    .explicit_file()
}

fn root_focused(output: &egui::FullOutput) -> bool {
    output
        .viewport_output
        .get(&egui::ViewportId::ROOT)
        .is_some_and(|output| {
            output
                .commands
                .iter()
                .any(|cmd| matches!(cmd, egui::ViewportCommand::Focus))
        })
}

fn receive(app: &App, path: PathBuf, request: ViewerOpenRequest) {
    app.activation_open_path_tx
        .send(
            crate::single_instance::ActivationOpenPath::received_with_request(
                path,
                app.remote_session_handle(),
                request,
            ),
        )
        .unwrap();
}

fn open_previous_reader(app: &mut App, presentation: ViewerPresentation, browser_view: bool) {
    app.settings.video_in_window_mode = presentation == ViewerPresentation::MainWindow;
    app.settings.detached_viewer_enabled =
        presentation == ViewerPresentation::DetachedWindow && !browser_view;
    if browser_view {
        let adoption = browser(app);
        app.open_fullscreen_with_adoption(0, HistoryTrigger::UserChosen, None, adoption);
    } else {
        app.open_fullscreen(0, HistoryTrigger::UserChosen);
    }
    assert_eq!(app.fullscreen_idx, Some(0));
    assert_eq!(app.viewer_presentation, presentation);
}

fn assert_hidden_browser_reader(
    app: &mut App,
    requested: &std::path::Path,
    request: ViewerOpenRequest,
) {
    let index = app
        .items
        .iter()
        .position(|item| matches!(item, GridItem::Image(path) if path == requested))
        .expect("the accepted hidden image must remain in the new reader");
    assert_eq!(app.fullscreen_idx, Some(index));
    assert_eq!(app.selected, Some(index));
    assert!(app.direct_media_reader_active());
    let snapshot = app.snapshot.as_ref().unwrap();
    assert!(
        matches!(&snapshot.source_label, crate::snapshot::SnapshotSourceLabel::DirectMedia { target, .. } if target == requested)
    );
    assert!(
        !snapshot
            .saved_items
            .iter()
            .any(|item| matches!(item, GridItem::Image(path) if path == requested)),
        "the hidden image must not leak into the saved origin list"
    );
    let id = app.detached_viewer_window_id().unwrap();
    let runtime = app.detached_window_manager.runtime(id).unwrap();
    assert!(runtime.linked);
    assert_eq!(runtime.purpose(), DetachedWindowPurpose::BrowserView);
    let ViewerOpenIntent::BrowserOverlay { placement } = request.intent else {
        panic!("browser request");
    };
    assert_eq!(runtime.placement, Some(placement));
    let stamp = app.viewer_sync_stamp_for_idx(index).unwrap();
    assert_eq!(app.last_viewer_sync_stamp.as_ref(), Some(&stamp));
    app.sync_detached_viewer_to_selected(&egui::Context::default());
    assert_eq!(app.fullscreen_idx, Some(index));
    assert_eq!(app.selected, Some(index));
    assert_eq!(app.last_viewer_sync_stamp.as_ref(), Some(&stamp));
    assert_eq!(app.detached_viewer_window_id(), Some(id));
}

#[test]
fn browser818_presentation_activation_hidden_image_replaces_each_previous_reader() {
    for (previous, browser_view) in [
        (ViewerPresentation::DetachedWindow, false),
        (ViewerPresentation::MainWindow, false),
        (ViewerPresentation::Fullscreen, false),
        (ViewerPresentation::DetachedWindow, true),
    ] {
        let mut app = fixture();
        let origin_folder = app.current_folder.clone();
        let origin_items = app.items.clone();
        app.settings.auto_fullscreen_image_folders = false;
        app.settings.auto_fullscreen_zip_pdf = false;
        open_previous_reader(&mut app, previous, browser_view);
        app.settings
            .file_type_visibility
            .excluded_extensions
            .insert("bmp".into());
        let ctx = egui::Context::default();
        // Repeat from a BrowserView whose reader itself owns a hidden image.
        for name in ["hidden-first.bmp", "hidden-second.bmp"] {
            let requested = app.current_folder.as_ref().unwrap().join(name);
            image::RgbaImage::new(2, 2).save(&requested).unwrap();
            assert!(
                !app.settings
                    .file_type_visibility
                    .allows_physical_file(&requested)
            );
            let old_window = app.detached_viewer_window_id();
            let request = browser(&app).request().unwrap();
            receive(&app, requested.clone(), request);
            app.poll_activation_open_paths(&ctx);
            assert_eq!(
                app.startup_open_path_resolve_pending
                    .as_ref()
                    .unwrap()
                    .owner
                    .viewer_open_adoption()
                    .request(),
                Some(request)
            );
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
            while app.startup_open_path_resolve_pending.is_some() {
                assert!(
                    std::time::Instant::now() < deadline,
                    "real activation resolver did not finish"
                );
                app.poll_startup_open_path_resolve(&ctx);
                std::thread::sleep(std::time::Duration::from_millis(2));
            }
            assert!(
                app.sidecar_restore_active(),
                "the production folder restore must own DirectMedia construction"
            );
            assert!(
                !app.items
                    .iter()
                    .any(|item| matches!(item, GridItem::Image(path) if path == &requested))
            );
            if let Some(old) = old_window {
                assert!(
                    app.detached_window_manager.runtime(old).is_some(),
                    "exercise the live old linked lease, not an already-retired window"
                );
            }
            app.finish_page_identity_preparation_for_test();
            assert_hidden_browser_reader(&mut app, &requested, request);
            if let Some(old) = old_window {
                assert_ne!(app.detached_viewer_window_id(), Some(old));
                assert!(app.detached_window_manager.runtime(old).is_none());
            }
        }
        // The normal terminal still restores DirectMedia's origin; no close guard was added.
        app.close_fullscreen_to_completion(&ctx).unwrap();
        assert!(!app.direct_media_reader_active());
        assert_eq!(app.fullscreen_idx, None);
        // Viewer teardown dispatches the ordinary asynchronous folder-history restore.
        // Check the returned grid only after that owner and its hydration have completed.
        app.finish_history_restore_preparation_for_test();
        assert_eq!(app.current_folder, origin_folder);
        assert_eq!(app.items, origin_items);
        assert_eq!(app.visible_indices, vec![0, 1]);
        assert!(app.items.iter().all(|item| item.name().ends_with(".png")));
    }
}

#[test]
fn browser818_presentation_hidden_image_immediate_action_finishes_old_reader_before_install() {
    for (previous, browser_view) in [
        (ViewerPresentation::DetachedWindow, false),
        (ViewerPresentation::MainWindow, false),
        (ViewerPresentation::Fullscreen, false),
        (ViewerPresentation::DetachedWindow, true),
    ] {
        let mut app = fixture();
        open_previous_reader(&mut app, previous, browser_view);
        let old = app.detached_viewer_window_id();
        let requested = app.current_folder.as_ref().unwrap().join("immediate.bmp");
        image::RgbaImage::new(2, 2).save(&requested).unwrap();
        app.settings
            .file_type_visibility
            .excluded_extensions
            .insert("bmp".into());
        let adoption = browser(&app);
        app.finish_main_list_after_hydration(MainListAfterHydration::DirectMedia {
            prepared: startup_ops::DirectMediaPrepared::image_fixture_for_test(requested.clone()),
            viewer_adoption: adoption,
        });
        app.finish_page_identity_preparation_for_test();
        assert_hidden_browser_reader(&mut app, &requested, adoption.request().unwrap());
        if let Some(old) = old {
            assert!(app.detached_window_manager.runtime(old).is_none());
        }
    }
}

#[test]
fn browser818_presentation_direct_media_handoff_retains_request_and_preferences_order() {
    for overlay in [false, true] {
        let mut app = fixture();
        open_previous_reader(&mut app, ViewerPresentation::DetachedWindow, false);
        let old = app.detached_viewer_window_id().unwrap();
        let requested = app.current_folder.as_ref().unwrap().join("prepared.bmp");
        image::RgbaImage::new(2, 2).save(&requested).unwrap();
        app.settings
            .file_type_visibility
            .excluded_extensions
            .insert("bmp".into());
        let adoption = if overlay {
            browser(&app)
        } else {
            ViewerOpenAdoption::requested(ViewerOpenRequest::preferences()).explicit_file()
        };
        let (target, prepared) = app
            .install_direct_media_reader_for_open(
                startup_ops::DirectMediaPrepared::image_fixture_for_test(requested.clone()),
                adoption,
            )
            .unwrap();
        assert_eq!(target, crate::snapshot::SnapshotTarget::Fs(requested));
        assert_eq!(prepared.request(), adoption.request());
        assert!(prepared.is_main_new());
        assert!(prepared.owns_file_selection());
        assert_eq!(prepared.needs_main_preparation(), !overlay);
        assert_eq!(app.detached_window_manager.runtime(old).is_none(), overlay);
        if overlay {
            assert_eq!(
                prepared.after_main_preparation(),
                prepared,
                "handoff completion is idempotent"
            );
            app.finish_required_fullscreen_load_with_adoption(
                target,
                HistoryTrigger::UserChosen,
                prepared,
            );
            app.finish_page_identity_preparation_for_test();
            assert!(
                app.direct_media_reader_active(),
                "no second old-reader close after page-index hydration"
            );
            assert_eq!(app.selected, app.fullscreen_idx);
            assert!(!prepared.context_continuation().is_main_new());
            assert_eq!(
                prepared.context_continuation().request(),
                adoption.request()
            );
        } else {
            assert_eq!(
                prepared, adoption,
                "Preferences keeps its original install-before-handoff sequence"
            );
        }
    }
}

#[test]
fn browser818_presentation_linked_seed_replaces_lease_with_f12_on_and_off() {
    for enabled in [false, true] {
        let mut app = fixture();
        app.settings.detached_viewer_enabled = true;
        app.open_fullscreen(0, HistoryTrigger::UserChosen);
        let previous = app.detached_viewer_window_id().unwrap();
        let normal = app.detached_window_seed_placement(previous);
        app.write_detached_viewer_borderless_state(
            true,
            Some(normal),
            "browser818_fixture",
            "before_browser",
        );
        app.settings.detached_viewer_enabled = enabled;
        let adoption = browser(&app);
        let expected = DetachedWindowSeed::for_adoption(adoption).placement;
        let generation = app.items_generation;
        let cancel = Arc::clone(&app.cancel_token);
        app.open_fullscreen_with_adoption(1, HistoryTrigger::UserChosen, None, adoption);
        let id = app.detached_viewer_window_id().unwrap();
        assert_ne!(id, previous);
        assert!(app.detached_window_manager.runtime(previous).is_none());
        let runtime = app.detached_window_manager.runtime(id).unwrap();
        assert_eq!(runtime.purpose(), DetachedWindowPurpose::BrowserView);
        assert!(runtime.linked);
        assert_eq!(runtime.placement, expected);
        assert!(!app.detached_viewer_independent_active);
        assert!(!app.detached_viewer_borderless_fullscreen);
        assert_eq!(app.settings.detached_viewer_enabled, enabled);
        assert_eq!(app.items_generation, generation);
        assert!(Arc::ptr_eq(&cancel, &app.cancel_token));
        assert_eq!(app.selected, Some(1));
        assert_eq!(app.fullscreen_idx, Some(1));
        let stamp = app.viewer_sync_stamp_for_idx(1).unwrap();
        assert_eq!(app.last_viewer_sync_stamp.as_ref(), Some(&stamp));
        app.sync_detached_viewer_to_selected(&egui::Context::default());
        assert_eq!(app.selected, Some(1));
        assert_eq!(app.fullscreen_idx, Some(1));
        assert_eq!(app.last_viewer_sync_stamp.as_ref(), Some(&stamp));
        assert_eq!(app.detached_viewer_window_id(), Some(id));
        assert_eq!(
            app.settings.detached_viewer_window_physical_placement,
            Some(normal)
        );
        let again = browser(&app);
        app.open_fullscreen_with_adoption(1, HistoryTrigger::UserChosen, None, again);
        assert_ne!(app.detached_viewer_window_id(), Some(id));
        assert!(app.detached_window_manager.runtime(id).is_none());
        assert_eq!(
            app.settings.detached_viewer_window_physical_placement,
            Some(normal)
        );
    }
}

#[test]
fn browser818_presentation_replacement_selection_survives_each_old_reader_close() {
    for previous in [
        ViewerPresentation::MainWindow,
        ViewerPresentation::Fullscreen,
        ViewerPresentation::DetachedWindow,
    ] {
        let mut app = fixture();
        if previous == ViewerPresentation::DetachedWindow {
            let adoption = browser(&app);
            app.open_fullscreen_with_adoption(0, HistoryTrigger::UserChosen, None, adoption);
        } else {
            // Both ordinary in-main presentations use the same terminal cursor restore.
            app.fullscreen_idx = Some(0);
            app.viewer_presentation = previous;
            app.fs_viewport_shown = previous == ViewerPresentation::Fullscreen;
            app.fs_viewport_presentation = app.fs_viewport_shown.then_some(previous);
        }
        let adoption = browser(&app);
        app.open_fullscreen_with_adoption(1, HistoryTrigger::UserChosen, None, adoption);
        assert!(app.browser_linked_viewer_session());
        let id = app.detached_viewer_window_id().unwrap();
        let stamp = app.viewer_sync_stamp_for_idx(1).unwrap();
        assert_eq!(app.selected, Some(1), "old reader: {previous:?}");
        assert_eq!(app.fullscreen_idx, Some(1));
        assert_eq!(app.last_viewer_sync_stamp.as_ref(), Some(&stamp));
        app.sync_detached_viewer_to_selected(&egui::Context::default());
        assert_eq!(app.selected, Some(1));
        assert_eq!(app.fullscreen_idx, Some(1));
        assert_eq!(app.last_viewer_sync_stamp.as_ref(), Some(&stamp));
        assert_eq!(app.detached_viewer_window_id(), Some(id));
    }
}

#[test]
fn browser818_presentation_browser_linked_navigation_and_grid_sync_ignore_f12_preference() {
    let mut app = fixture();
    let adoption = browser(&app);
    app.open_fullscreen_with_adoption(0, HistoryTrigger::UserChosen, None, adoption);
    let id = app.detached_viewer_window_id().unwrap();
    assert!(!app.settings.detached_viewer_enabled);
    let ctx = egui::Context::default();
    app.move_grid_cursor(Some(1), GridCursorIntent::Automatic);
    app.sync_detached_viewer_to_selected(&ctx);
    assert_eq!(app.fullscreen_idx, Some(1));
    assert_eq!(app.last_viewer_sync_stamp.as_ref().unwrap().idx, 1);
    assert_eq!(app.detached_viewer_window_id(), Some(id));
    app.open_fullscreen_from_fs_navigation(&ctx, 0, HistoryTrigger::UserChosen);
    assert_eq!(app.viewer_presentation, ViewerPresentation::DetachedWindow);
    assert_eq!(app.detached_viewer_window_id(), Some(id));
    assert!(!app.settings.detached_viewer_enabled);
}

#[test]
fn browser818_presentation_normal_open_still_uses_preferences_and_saved_seed() {
    for enabled in [false, true] {
        let mut app = fixture();
        let saved = app.default_detached_viewer_window_placement();
        app.settings.detached_viewer_window_physical_placement = Some(saved);
        app.settings.detached_viewer_enabled = enabled;
        app.open_fullscreen_with_adoption(
            0,
            HistoryTrigger::UserChosen,
            None,
            ViewerOpenAdoption::requested(ViewerOpenRequest::preferences()),
        );
        assert_eq!(app.viewer_session_is_detached(), enabled);
        if enabled {
            let runtime = app
                .detached_window_manager
                .runtime(app.detached_viewer_window_id().unwrap())
                .unwrap();
            assert_eq!(runtime.purpose(), DetachedWindowPurpose::Normal);
            assert_eq!(runtime.placement, Some(saved));
        }
    }
}

#[test]
fn browser818_presentation_multiple_materialized_fork_keeps_independent_sibling() {
    for enabled in [false, true] {
        let mut app = fixture();
        app.settings.detached_viewer_enabled = enabled;
        app.settings.detached_viewer_open_images_in_window = true;
        let sibling_window = 818_401;
        app.detached_window_manager.create_with_purpose(
            sibling_window,
            false,
            DetachedWindowPurpose::Normal,
        );
        let sibling = app.build_window_context_for_test(sibling_window, |app| {
            app.fs_cache.insert(81, FsCacheEntry::Failed);
        });
        app.transition_detached_window_state(
            sibling_window,
            DetachedWindowState::ParkedLive,
            "browser818_fixture",
        );
        let before = app
            .with_viewer_context(sibling, |app| {
                (app.items_generation, Arc::clone(&app.cancel_token))
            })
            .unwrap();
        let sender = app
            .with_viewer_context(sibling, |app| app.tx.clone())
            .unwrap();
        let message = || ThumbMsg {
            idx: 81,
            input_seq: 81,
            items_gen: before.0,
            request_id: crate::thumb_loader::ThumbnailRequestId::Standalone,
            payload: crate::thumb_loader::ThumbMsgPayload::Canceled,
        };
        sender.send(message()).unwrap();
        let main_generation = app.items_generation;
        let adoption = browser(&app);
        let expected = DetachedWindowSeed::for_adoption(adoption).placement;
        app.open_fullscreen_with_adoption(1, HistoryTrigger::UserChosen, None, adoption);
        assert!(app.fullscreen_idx.is_none());
        let reader = app.active_viewer_context_id().unwrap();
        app.with_viewer_context(reader, |app| {
            let runtime = app
                .detached_window_manager
                .runtime(app.detached_viewer_window_id().unwrap())
                .unwrap();
            assert_eq!(runtime.purpose(), DetachedWindowPurpose::BrowserView);
            assert!(!runtime.linked);
            assert_eq!(runtime.placement, expected);
            assert!(app.detached_viewer_independent_active);
            assert_eq!(app.fullscreen_idx, Some(1));
        })
        .unwrap();
        assert_eq!(app.items_generation, main_generation);
        assert_eq!(app.viewer_context_window(sibling), Some(sibling_window));
        assert_eq!(
            app.detached_window_state(sibling_window),
            Some(DetachedWindowState::ParkedLive)
        );
        app.with_viewer_context(sibling, |app| {
            assert_eq!(app.items_generation, before.0);
            assert!(Arc::ptr_eq(&app.cancel_token, &before.1));
            assert!(!app.cancel_token.load(Ordering::Relaxed));
            assert!(matches!(app.fs_cache.get(&81), Some(FsCacheEntry::Failed)));
            assert_eq!(app.rx.try_recv().unwrap().idx, 81);
            sender.send(message()).unwrap();
            assert_eq!(app.rx.try_recv().unwrap().input_seq, 81);
        })
        .unwrap();
        assert_eq!(app.settings.detached_viewer_enabled, enabled);
    }
}

#[test]
fn browser818_presentation_loading_seed_and_invalid_positions_use_existing_fallback() {
    for invalid in 0..3 {
        let mut app = fixture();
        let expected = if invalid == 0 {
            DetachedWindowSeed::for_adoption(browser(&app))
                .placement
                .unwrap()
        } else {
            app.default_detached_viewer_window_placement()
        };
        let seed = DetachedWindowSeed {
            purpose: DetachedWindowPurpose::BrowserView,
            placement: Some(if invalid == 1 {
                PhysicalWindowPlacement {
                    outer: PhysicalRect {
                        left: 0,
                        top: 0,
                        right: 0,
                        bottom: 0,
                    },
                    maximized: false,
                }
            } else if invalid == 2 {
                PhysicalWindowPlacement {
                    outer: PhysicalRect {
                        left: i32::MAX - 500,
                        top: i32::MAX - 500,
                        right: i32::MAX,
                        bottom: i32::MAX,
                    },
                    maximized: false,
                }
            } else {
                expected
            }),
        };
        let ctx = egui::Context::default();
        let folder = app.current_folder.clone().unwrap();
        let id = app
            .start_active_detached_loading_context_with_seed(
                &ctx,
                seed,
                DetachedSource::Image,
                folder,
                |app, id| {
                    assert_eq!(
                        app.detached_window_manager.runtime(id).unwrap().purpose(),
                        DetachedWindowPurpose::BrowserView
                    );
                    assert_eq!(app.detached_window_manager.placement(id), Some(expected));
                    DetachedLoadingContextInit::Continue
                },
            )
            .unwrap();
        assert_eq!(app.detached_window_manager.placement(id), Some(expected));
        assert!(!app.detached_window_manager.runtime(id).unwrap().linked);
    }
    let mut app = fixture();
    let expected = app.default_detached_viewer_window_placement();
    let ctx = egui::Context::default();
    let folder = app.current_folder.clone().unwrap();
    let id = app
        .start_active_detached_loading_context(
            &ctx,
            Some(expected),
            DetachedSource::Image,
            folder,
            |_, _| DetachedLoadingContextInit::Continue,
        )
        .unwrap();
    assert_eq!(
        app.detached_window_manager.runtime(id).unwrap().purpose(),
        DetachedWindowPurpose::Normal
    );
    assert_eq!(app.detached_window_manager.placement(id), Some(expected));
}

#[test]
fn browser818_presentation_show_waits_for_placement_and_only_new_window_requests_focus() {
    let mut app = fixture();
    let adoption = browser(&app);
    app.open_fullscreen_with_adoption(0, HistoryTrigger::UserChosen, None, adoption);
    let id = app.detached_viewer_window_id().unwrap();
    let viewport = App::detached_image_window_viewport_id(id);
    let ctx = egui::Context::default();
    ctx.set_embed_viewports(false);
    let output = ctx.run_pass(Default::default(), |ctx| {
        ctx.show_viewport_deferred(viewport, Default::default(), |_, _| {});
        app.request_detached_window_visibility(ctx, id, true, Some(false), true);
    });
    assert!(!root_focused(&output));
    assert!(
        !output.viewport_output[&viewport]
            .commands
            .iter()
            .any(|command| matches!(
                command,
                egui::ViewportCommand::Focus | egui::ViewportCommand::Visible(true)
            ))
    );
    assert!(
        matches!(app.detached_window_manager.runtime(id).unwrap().host_placement,
        HostPlacement::Unplaced { show_request: Some(request), .. } if request.focus)
    );
    output.drop_without_applying_deltas();
}

#[test]
fn browser818_admission_modal_defers_and_latest_request_is_the_only_owner() {
    let mut app = fixture();
    let epub = app.tmp.path().join("modal.epub");
    std::fs::write(&epub, b"book").unwrap();
    assert_eq!(
        app.route_pdf_open_failure(
            OpenRequestOwner::Navigation,
            &epub,
            PdfOpenFailure::NotConverted,
            StartupListIntent::ExplicitList
        ),
        PdfOpenFailureRoute::ConversionDialogOpened
    );
    let old = browser(&app).request().unwrap();
    let new = browser(&app).request().unwrap();
    let path = app.tmp.path().join("new.png");
    receive(&app, path.clone(), old);
    receive(&app, path, new);
    let ctx = egui::Context::default();
    let output = ctx.run_pass(Default::default(), |ctx| {
        app.poll_activation_open_paths(ctx)
    });
    assert!(!root_focused(&output));
    assert!(app.startup_open_path_resolve_pending.is_none());
    output.drop_without_applying_deltas();
    app.finish_epub_convert(crate::ui_dialogs::epub_convert::EpubConvertExit::Abort);
    let output = ctx.run_pass(Default::default(), |ctx| {
        app.poll_activation_open_paths(ctx)
    });
    assert!(!root_focused(&output));
    assert_eq!(
        app.startup_open_path_resolve_pending
            .as_ref()
            .unwrap()
            .owner
            .viewer_open_adoption()
            .request(),
        Some(new)
    );
    output.drop_without_applying_deltas();
}

#[test]
fn browser818_admission_remote_both_rejection_paths_focus_only_for_browser_intent() {
    for overlay in [false, true] {
        for returned in [false, true] {
            let mut app = fixture();
            let handle = crate::remote_ipc::session::SessionHandle::new();
            app.set_remote_session_handle(handle.clone());
            handle.acquire(mimageviewer_ipc::SessionAcquireRequest {
                client_id: "browser818".into(),
                peer: mimageviewer_ipc::SessionPeerInfo {
                    connection_kind: mimageviewer_ipc::SessionConnectionKind::Direct,
                    device_name: None,
                },
            });
            let request = if overlay {
                browser(&app).request().unwrap()
            } else {
                ViewerOpenRequest::preferences()
            };
            receive(&app, app.tmp.path().join("remote.png"), request);
            if returned {
                handle.local_disconnect();
                assert!(handle.complete_app_drain(handle.snapshot().generation));
            }
            let ctx = egui::Context::default();
            let output = ctx.run_pass(Default::default(), |ctx| {
                app.poll_activation_open_paths(ctx)
            });
            assert_eq!(root_focused(&output), overlay);
            assert!(app.activation_open_path_rx.try_recv().is_err());
            assert!(app.startup_open_path_resolve_pending.is_none());
            output.drop_without_applying_deltas();
        }
    }
}

#[test]
fn browser818_admission_sidecar_discard_has_no_focus_or_replacement() {
    let mut app = fixture();
    let folder = app.current_folder.clone().unwrap();
    let _receiver = app.activate_live_sidecar_restore_for_test(folder);
    let request = browser(&app).request().unwrap();
    receive(&app, app.tmp.path().join("discard.png"), request);
    let ctx = egui::Context::default();
    let output = ctx.run_pass(Default::default(), |ctx| {
        app.poll_activation_open_paths(ctx)
    });
    assert!(!root_focused(&output));
    assert!(app.startup_open_path_resolve_pending.is_none());
    assert!(app.activation_open_path_rx.try_recv().is_err());
    assert!(app.fs_feedback_toast.is_none());
    output.drop_without_applying_deltas();
}

#[test]
fn browser818_admission_snapshot_refusal_focuses_main_from_resolved_request() {
    let mut app = fixture();
    app.activate_snapshot(crate::snapshot::SnapshotSourceLabel::Mixed);
    app.finish_page_identity_preparation_for_test();
    let outside = app.tmp.path().join("outside");
    std::fs::create_dir(&outside).unwrap();
    let request = browser(&app).request().unwrap();
    let ctx = egui::Context::default();
    app.start_startup_open_request_with_navigation(
        ExplicitOpenPath::new(outside.clone(), request),
        StartupOpenPathSource::Activation,
        &ctx,
        None,
    );
    let owner = app
        .startup_open_path_resolve_pending
        .as_ref()
        .unwrap()
        .owner
        .clone();
    let output = ctx.run_pass(Default::default(), |ctx| {
        app.finish_startup_open_path_resolve(
            owner.clone(),
            StartupOpenPathResolveResult {
                requested: outside.clone(),
                resolved: Some(crate::folder_tree::OpenablePathResolution {
                    path: outside.clone(),
                    kind: crate::folder_tree::OpenablePathKind::Directory,
                    requested_is_file: false,
                }),
                bookmark_relative_page_openable: None,
                rar_volume_proof: None,
                elapsed_ms: 0.0,
                direct_media: None,
            },
            ctx,
        )
    });
    assert!(root_focused(&output));
    assert!(app.detached_window_manager.is_empty());
    output.drop_without_applying_deltas();
}

#[test]
fn browser818_admission_composition_refusal_keeps_normal_focus_behavior() {
    for overlay in [false, true] {
        let mut app = fixture();
        app.composition_editor_operations.owner =
            Some(book_composition_editor::CompositionEditorOwner {
                context_id: app.viewer_context_main(),
                scope: crate::book_scope::BookScopeKey::document(&app.tmp.path().join("edit.cbz"))
                    .unwrap(),
                request_id: 818_402,
            });
        let request = if overlay {
            browser(&app).request().unwrap()
        } else {
            ViewerOpenRequest::preferences()
        };
        receive(&app, app.tmp.path().join("rejected.png"), request);
        let ctx = egui::Context::default();
        let output = ctx.run_pass(Default::default(), |ctx| {
            app.poll_activation_open_paths(ctx)
        });
        assert_eq!(root_focused(&output), overlay);
        assert!(app.startup_open_path_resolve_pending.is_none());
        assert!(app.activation_open_path_rx.try_recv().is_err());
        assert!(app.fs_feedback_toast.is_some());
        output.drop_without_applying_deltas();
    }
}

#[test]
fn browser818_presentation_tray_restore_keeps_all_visibility_without_old_media_focus() {
    use crate::window_activation::MainRestorePolicy;
    for policy in [
        MainRestorePolicy::Activate,
        MainRestorePolicy::PreserveForeground,
    ] {
        let mut app = fixture();
        let path = app.tmp.path().join("retained.mp4");
        app.items[0] = GridItem::Video(path.clone());
        let player = crate::video::VideoPlayer::disconnected_for_test(path, 17.0);
        app.fs_cache.insert(
            0,
            FsCacheEntry::Video {
                player: Box::new(player),
                load_seq: 0,
            },
        );
        app.fullscreen_idx = Some(0);
        app.viewer_presentation = ViewerPresentation::Fullscreen;
        app.fs_viewport_shown = true;
        app.fs_viewport_presentation = Some(ViewerPresentation::Fullscreen);
        app.window_visible = false;
        let viewport = app.fullscreen_viewport_id();
        let generation = app.items_generation;
        let ctx = egui::Context::default();
        ctx.set_embed_viewports(false);
        let output = ctx.run_pass(Default::default(), |ctx| {
            ctx.show_viewport_deferred(viewport, Default::default(), |_, _| {});
            app.sync_after_restore_with_policy(ctx, policy);
        });
        assert!(app.window_visible);
        assert_eq!(app.items_generation, generation);
        assert_eq!(app.fullscreen_idx, Some(0));
        assert!(!root_focused(&output));
        let commands = &output.viewport_output[&viewport].commands;
        assert!(
            commands
                .iter()
                .any(|command| matches!(command, egui::ViewportCommand::Visible(true)))
        );
        assert_eq!(
            commands
                .iter()
                .any(|command| matches!(command, egui::ViewportCommand::Focus)),
            policy.requests_focus()
        );
        let Some(FsCacheEntry::Video { player, .. }) = app.fs_cache.get(&0) else {
            panic!("retained player");
        };
        assert_eq!(player.position(), 17.0);
        output.drop_without_applying_deltas();
    }
}

#[test]
fn browser818_presentation_active_independent_media_remains_parked_live() {
    let mut app = fixture();
    app.settings.fullfeature_media_window = true;
    let id = 818_403;
    let path = app.tmp.path().join("old-media.mp4");
    let sibling = app.build_window_context_for_test(id, |app| {
        app.items = vec![GridItem::Video(path.clone())];
        app.fullscreen_idx = Some(0);
        app.viewer_presentation = ViewerPresentation::DetachedWindow;
        app.detached_viewer_independent_active = true;
        app.fs_cache.insert(
            0,
            FsCacheEntry::Video {
                player: Box::new(crate::video::VideoPlayer::disconnected_for_test(path, 23.0)),
                load_seq: 0,
            },
        );
    });
    app.detached_window_manager
        .create_with_purpose(id, false, DetachedWindowPurpose::Normal);
    app.begin_active_detached_session(id, DetachedSource::Video);
    let adoption = browser(&app);
    app.open_fullscreen_with_adoption(1, HistoryTrigger::UserChosen, None, adoption);
    assert_eq!(
        app.detached_window_state(id),
        Some(DetachedWindowState::ParkedLive)
    );
    assert_eq!(app.viewer_context_window(sibling), Some(id));
    assert!(app.browser_linked_viewer_session());
    app.with_viewer_context(sibling, |app| {
        let Some(FsCacheEntry::Video { player, .. }) = app.fs_cache.get(&0) else {
            panic!("parked player");
        };
        assert_eq!(player.position(), 23.0);
    })
    .unwrap();
}

#[test]
fn browser818_presentation_restore_supersedes_only_old_show_focus_before_placement() {
    for hidden in [false, true] {
        let mut app = fixture();
        app.settings.detached_viewer_enabled = true;
        app.open_fullscreen(0, HistoryTrigger::UserChosen);
        let id = app.detached_viewer_window_id().unwrap();
        let ctx = egui::Context::default();
        app.request_detached_window_visibility(&ctx, id, true, Some(false), true);
        app.window_visible = !hidden;
        let output = ctx.run_pass(Default::default(), |ctx| {
            app.restore_for_browser_overlay(ctx)
        });
        assert!(app.window_visible);
        assert!(!root_focused(&output));
        assert!(
            matches!(app.detached_window_manager.runtime(id).unwrap().host_placement,
            HostPlacement::Unplaced { show_request: Some(request), .. } if !request.focus)
        );
        assert_eq!(app.fullscreen_idx, Some(0));
        output.drop_without_applying_deltas();
    }
}

#[test]
fn browser818_presentation_activation_restore_materialized_keeps_request_seed() {
    let mut app = fixture();
    app.settings.detached_viewer_open_images_in_window = true;
    let folder = app.tmp.path().join("unloaded-browser-folder");
    std::fs::create_dir(&folder).unwrap();
    let requested = folder.join("target.png");
    image::RgbaImage::new(2, 2).save(&requested).unwrap();
    let request = browser(&app).request().unwrap();
    let seed = DetachedWindowSeed::for_adoption(ViewerOpenAdoption::requested(request));
    let main_generation = app.items_generation;
    receive(&app, requested.clone(), request);
    let ctx = egui::Context::default();
    app.poll_activation_open_paths(&ctx);
    let (tx, rx) = mpsc::channel();
    app.startup_open_path_resolve_pending.as_mut().unwrap().rx = rx;
    tx.send(StartupOpenPathResolveResult {
        requested: requested.clone(),
        resolved: Some(crate::folder_tree::OpenablePathResolution {
            path: folder.clone(),
            kind: crate::folder_tree::OpenablePathKind::Directory,
            requested_is_file: true,
        }),
        bookmark_relative_page_openable: None,
        rar_volume_proof: None,
        elapsed_ms: 0.0,
        direct_media: None,
    })
    .unwrap();
    let output = ctx.run_pass(Default::default(), |ctx| {
        app.poll_startup_open_path_resolve(ctx)
    });
    assert!(!root_focused(&output));
    output.drop_without_applying_deltas();
    assert!(app.sidecar_restore_active());
    assert_eq!(
        app.sidecar_restore
            .as_ref()
            .unwrap()
            .viewer_open_adoption_for_test()
            .unwrap()
            .request(),
        Some(request)
    );
    app.finish_page_identity_preparation_for_test();
    let reader = app.active_viewer_context_id().unwrap();
    let id = app.viewer_context_window(reader).unwrap();
    let runtime = app.detached_window_manager.runtime(id).unwrap();
    assert_eq!(runtime.purpose(), DetachedWindowPurpose::BrowserView);
    assert_eq!(runtime.placement, seed.placement);
    assert!(!runtime.linked);
    app.with_viewer_context(reader, |app| {
        app.finish_page_identity_preparation_for_test();
        let index = app
            .items
            .iter()
            .position(|item| matches!(item, GridItem::Image(path) if path == &requested))
            .unwrap();
        assert_eq!(app.fullscreen_idx, Some(index));
        assert_eq!(app.selected, Some(index));
        assert_eq!(app.detached_viewer_window_id(), Some(id));
    })
    .unwrap();
    assert_ne!(app.items_generation, main_generation);
    assert_eq!(app.current_folder, Some(folder));
    assert!(!app.settings.detached_viewer_enabled);
}
