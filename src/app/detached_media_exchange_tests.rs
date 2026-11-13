//! Headless tests of the real open, navigation, activation and close handlers.
use super::*;
use crate::detached_window_geometry::{PhysicalRect, PhysicalWindowPlacement};

pub(crate) fn install_still(app: &mut App, ctx: &egui::Context, idx: usize) {
    let pixels = Arc::new(egui::ColorImage::filled([8, 12], egui::Color32::WHITE));
    app.fs_cache.insert(
        idx,
        FsCacheEntry::Static {
            tex: ctx.load_texture(
                format!("exchange_{idx}"),
                (*pixels).clone(),
                Default::default(),
            ),
            pixels,
            source_dims: Some([8, 12]),
            load_seq: 0,
            animation: crate::fs_animation::StaticAnimationState::Still,
            pdf_metadata: None,
            pdf_rerender_failures: Vec::new(),
        },
    );
}

pub(crate) fn install_media(app: &mut App, idx: usize) {
    let path = grid_item_media_path(&app.items[idx]).unwrap().to_path_buf();
    let player = crate::video::VideoPlayer::disconnected_for_test(path, 3.0);
    player.set_playing(true);
    app.fs_cache.insert(
        idx,
        FsCacheEntry::Video {
            player: Box::new(player),
            load_seq: 0,
        },
    );
}

pub(crate) fn install_items(app: &mut App, ctx: &egui::Context, root: &Path, audio: bool) {
    let media = |name: &str| {
        if audio {
            GridItem::Audio(root.join(name).with_extension("mp3"))
        } else {
            GridItem::Video(root.join(name))
        }
    };
    app.items = vec![
        media("a.mp4"),
        media("b.mp4"),
        GridItem::Image(root.join("1.png")),
        GridItem::Image(root.join("2.png")),
    ];
    for idx in 0..2 {
        std::fs::write(grid_item_media_path(&app.items[idx]).unwrap(), b"fake").unwrap();
    }
    for name in ["1.png", "2.png"] {
        image::RgbaImage::from_pixel(8, 12, image::Rgba([20, 40, 60, 255]))
            .save(root.join(name))
            .unwrap();
    }
    app.image_metas = vec![None; 4];
    app.thumbnails = (0..4).map(|_| ThumbnailState::Pending).collect();
    app.rebuild_visible_indices();
    app.selected = Some(0);
    app.spread_mode = crate::settings::SpreadMode::Single;
    app.settings.detached_viewer_enabled = false;
    app.settings.fullfeature_media_window = true;
    install_media(app, 0);
    install_still(app, ctx, 2);
    install_still(app, ctx, 3);
}

pub(crate) fn open_grid(app: &mut App, ctx: &egui::Context, idx: usize) {
    assert!(app.prepare_detached_context_for_grid_open(ctx, idx));
    app.fs_open_intent_from_grid = true;
    app.open_fullscreen(idx, HistoryTrigger::UserChosen);
}

/// A normal list open, followed by a different main open and actual media activation.
/// No test writes the independence flag.
pub(crate) fn activate_independent_media(
    app: &mut App,
    ctx: &egui::Context,
) -> (u64, ViewerContextId) {
    open_grid(app, ctx, 0);
    let a = app.detached_viewer_window_id().unwrap();
    assert!(!app.detached_viewer_independent_active);
    open_grid(app, ctx, 3);
    assert_eq!(
        app.detached_window_state(a),
        Some(DetachedWindowState::ParkedLive)
    );
    assert!(app.activate_detached_image_window_snapshot(ctx, a));
    let c = app.active_viewer_context_id().unwrap();
    app.with_viewer_context(c, |owner| {
        assert!(owner.detached_viewer_independent_active);
        assert_eq!(owner.fullscreen_idx, Some(0));
    })
    .unwrap();
    (a, c)
}

pub(crate) fn placement(maximized: bool) -> PhysicalWindowPlacement {
    PhysicalWindowPlacement {
        outer: PhysicalRect {
            left: 210,
            top: 130,
            right: 1010,
            bottom: 730,
        },
        maximized,
    }
}

/// Feed the actual async navigation handler a deterministic worker response.
pub(crate) fn resolve_navigation(
    app: &mut App,
    ctx: &egui::Context,
    target: Option<usize>,
    native: bool,
) {
    let (request_tx, request_rx) = mpsc::channel();
    let (result_tx, result_rx) = mpsc::channel();
    app.media_navigation_resolver = Some(MediaNavigationResolver {
        request_tx,
        result_rx,
        next_request_id: 0,
    });
    let current = app.fullscreen_idx.unwrap();
    if native {
        app.navigate_native_video_fullscreen(ctx, current, 1);
    } else {
        let order = app.get_nav_indices();
        app.start_manual_media_navigation(
            ctx,
            &order,
            current,
            1,
            false,
            "exchange_test",
            ManualMediaNavigationLanding::Fullscreen,
        );
    }
    let request = request_rx
        .try_recv()
        .expect("real handler submitted resolver request");
    result_tx
        .send(MediaNavigationResolverResponse {
            request_id: request.request_id,
            result: MediaNavigationResolveResult {
                target_idx: target,
                missing: Vec::new(),
            },
        })
        .unwrap();
    app.poll_media_navigation_pending(ctx);
    assert!(app.media_navigation_pending.is_none());
}

#[test]
fn independent_media_exchange_navigation_retires_a_preserves_c_and_depth_on_b() {
    for audio in [false, true] {
        let mut app = setup_app_for_test();
        let ctx = egui::Context::default();
        let root = app.tmp.path().to_path_buf();
        install_items(&mut app, &ctx, &root, audio);
        let (a, c) = activate_independent_media(&mut app, &ctx);
        let main = (
            app.items.clone(),
            app.selected,
            app.fullscreen_idx,
            app.items_generation,
        );
        app.with_viewer_context(c, |owner| {
            let items = owner.items.clone();
            let order = owner.visible_indices.clone();
            owner.set_detached_window_runtime_placement(a, placement(false), "test");
            install_still(owner, &ctx, 2);
            install_still(owner, &ctx, 3);
            resolve_navigation(owner, &ctx, None, !audio);
            assert_eq!(
                owner.detached_viewer_window_id(),
                Some(a),
                "no candidate keeps A"
            );
            assert!(owner.fs_cache.contains_key(&0));
            resolve_navigation(owner, &ctx, Some(2), !audio);
            let b = owner.detached_viewer_window_id().unwrap();
            assert_ne!(a, b);
            assert_eq!(owner.locate_window_context(a), None);
            assert_eq!(owner.detached_window_state(a), None);
            assert!(!owner.detached_image_windows.iter().any(|w| w.id == a));
            assert!(!owner.deferred_detached_image_window_views.contains_key(&a));
            assert_eq!(owner.viewer_context_window(c), Some(b));
            assert_eq!(
                owner.detached_window_runtime_placement(b),
                Some(placement(false))
            );
            assert!(
                !owner.fs_cache.contains_key(&0),
                "old player is retired before prefetch"
            );
            assert!(
                owner.fs_cache.contains_key(&3),
                "still prefetch cache survives exchange"
            );
            assert!(owner.detached_viewer_independent_active);
            assert_eq!(owner.items, items);
            assert_eq!(owner.visible_indices, order);
            assert!(!owner.pending_main_foreground_reclaim);
            owner.set_fs_exclusive_view(
                crate::fs_exclusive_view::FsExclusiveView::Depth(
                    crate::depth_parallax::DepthViewState::new(owner.edit_request_owner_context()),
                ),
                "test_depth",
            );
            owner.open_fullscreen_from_fs_navigation(&ctx, 3, HistoryTrigger::UserChosen);
            assert_eq!(owner.detached_viewer_window_id(), Some(b));
            assert!(owner.detached_viewer_independent_active);
            assert!(owner.fs_exclusive_view.is_depth());
            assert!(owner.should_block_detached_independent_still_navigation_to_video(0));
        })
        .unwrap();
        assert_eq!(
            (
                app.items.clone(),
                app.selected,
                app.fullscreen_idx,
                app.items_generation
            ),
            main
        );
    }
}

#[test]
fn independent_media_exchange_same_kind_and_audio_mode_keep_independence() {
    for (audio, audio_mode) in [(false, false), (true, false), (false, true)] {
        let mut app = setup_app_for_test();
        let ctx = egui::Context::default();
        let root = app.tmp.path().to_path_buf();
        install_items(&mut app, &ctx, &root, audio);
        let (a, c) = activate_independent_media(&mut app, &ctx);
        app.with_viewer_context(c, |owner| {
            resolve_navigation(owner, &ctx, Some(1), !audio);
            install_media(owner, 1);
            assert_eq!(owner.detached_viewer_window_id(), Some(a));
            assert!(owner.detached_viewer_independent_active);
            if audio_mode {
                owner.video_audio_mode = Some(1);
            }
            resolve_navigation(owner, &ctx, Some(2), !audio && !audio_mode);
            assert_ne!(owner.detached_viewer_window_id(), Some(a));
            assert!(owner.detached_viewer_independent_active);
            assert!(owner.video_audio_mode.is_none());
            assert!(owner.video_audio_vst.is_none());
            assert!(owner.video_audio_mode_runtime.is_none());
            assert!(
                !owner
                    .fs_cache
                    .values()
                    .any(|entry| matches!(entry, FsCacheEntry::Video { .. }))
            );
        })
        .unwrap();
    }
}

#[test]
fn independent_media_exchange_multiwindow_new_media_is_independent_at_binding() {
    for audio in [false, true] {
        let mut app = setup_app_for_test();
        let ctx = egui::Context::default();
        let root = app.tmp.path().to_path_buf();
        install_items(&mut app, &ctx, &root, audio);
        app.settings.detached_viewer_open_images_in_window = true;
        open_grid(&mut app, &ctx, 0);
        let a = app.detached_viewer_window_id().unwrap();
        assert!(app.detached_viewer_independent_active);
        resolve_navigation(&mut app, &ctx, Some(2), !audio);
        assert_eq!(app.detached_viewer_window_id(), Some(a));
        assert_eq!(
            app.selected,
            Some(2),
            "media departure keeps existing list selection follow"
        );
        assert!(app.detached_viewer_independent_active);
        app.open_fullscreen_from_fs_navigation(&ctx, 3, HistoryTrigger::UserChosen);
        assert_eq!(app.detached_viewer_window_id(), Some(a));
        assert_eq!(
            app.selected,
            Some(2),
            "independent still navigation no longer follows list"
        );
        assert!(app.detached_viewer_independent_active);
        assert!(app.should_block_detached_independent_still_navigation_to_video(0));
    }
}

#[test]
fn independent_media_exchange_linked_media_controls_keep_existing_destination() {
    for f12 in [false, true] {
        let mut app = setup_app_for_test();
        let ctx = egui::Context::default();
        let root = app.tmp.path().to_path_buf();
        install_items(&mut app, &ctx, &root, false);
        app.settings.detached_viewer_enabled = f12;
        open_grid(&mut app, &ctx, 0);
        let a = app.detached_viewer_window_id().unwrap();
        resolve_navigation(&mut app, &ctx, Some(2), true);
        assert!(!app.detached_viewer_independent_active);
        if f12 {
            assert_eq!(app.detached_viewer_window_id(), Some(a));
            assert_eq!(app.viewer_presentation, ViewerPresentation::DetachedWindow);
        } else {
            assert_eq!(app.detached_viewer_window_id(), None);
            assert_eq!(
                app.viewer_presentation,
                app.non_detached_viewer_presentation()
            );
        }
    }
}

#[test]
fn independent_media_exchange_f12_commits_independence_even_on_same_host() {
    for audio in [false, true] {
        let mut app = setup_app_for_test();
        let ctx = egui::Context::default();
        let root = app.tmp.path().to_path_buf();
        install_items(&mut app, &ctx, &root, audio);
        let (a, c) = activate_independent_media(&mut app, &ctx);
        app.with_viewer_context(c, |owner| {
            if !audio {
                owner.apply_video_presentation_switched(ViewerPresentation::DetachedWindow);
            }
            assert!(
                owner.detached_viewer_independent_active,
                "native same-host synchronization preserves session"
            );
            // fullfeature media F12 normally leaves detached. Exercise its explicit same-host
            // commit too, without fabricating a player or independent flag.
            if !audio {
                owner.request_native_video_f12_presentation(ViewerPresentation::DetachedWindow);
                assert!(!owner.detached_viewer_independent_active);
                owner.activate_mounted_as_independent_detached(a);
            }
            owner.open_fullscreen_from_fs_navigation(&ctx, 2, HistoryTrigger::UserChosen);
            let b = owner.detached_viewer_window_id().unwrap();
            owner.settings.detached_viewer_enabled = false;
            owner.toggle_detached_viewer_mode();
            assert_eq!(
                owner.viewer_presentation,
                ViewerPresentation::DetachedWindow
            );
            assert!(
                !owner.detached_viewer_independent_active,
                "F12 ON links the existing still host"
            );
            assert_eq!(owner.detached_viewer_window_id(), Some(b));
            owner.toggle_detached_viewer_mode();
            assert_eq!(
                owner.viewer_presentation,
                owner.non_detached_viewer_presentation()
            );
            assert!(!owner.detached_viewer_independent_active);
        })
        .unwrap();
    }
}

#[test]
fn independent_media_exchange_native_f12_reversal_commits_linked_session() {
    let mut app = setup_app_for_test();
    let ctx = egui::Context::default();
    let root = app.tmp.path().to_path_buf();
    install_items(&mut app, &ctx, &root, false);
    let (a, c) = activate_independent_media(&mut app, &ctx);
    app.with_viewer_context(c, |owner| {
        owner.toggle_detached_viewer_mode();
        assert!(owner.video_presentation_transition.is_transitioning());
        assert_eq!(
            owner.viewer_presentation,
            ViewerPresentation::DetachedWindow
        );
        owner.toggle_detached_viewer_mode();
        assert_eq!(owner.detached_viewer_window_id(), Some(a));
        assert_eq!(
            owner.viewer_presentation,
            ViewerPresentation::DetachedWindow
        );
        assert!(!owner.detached_viewer_independent_active);
    })
    .unwrap();
}

/// Supply native acknowledgements, then execute the real App commit effects.
fn complete_native_request(app: &mut App, ctx: &egui::Context, request: u64, host: u64) {
    assert_eq!(
        app.video_presentation_transition.request_id(),
        Some(request)
    );
    assert!(matches!(
        app.video_presentation_transition.state(),
        PresentationTransitionState::Preparing {
            progress: PreparingProgress::AwaitingNative { .. },
            ..
        }
    ));
    app.video_presentation_transition
        .dispatch(PresentationTransitionEvent::NativeReady {
            request_id: request,
            candidate: PresentationCandidate {
                presenter_hwnd: 0xA,
                native_generation: 1,
                host_hwnd: host,
                requires_retire: false,
            },
        });
    app.video_presentation_transition.drive();
    let effects = app.video_presentation_transition.take_effects();
    assert!(
        effects
            .iter()
            .any(|effect| matches!(effect, PresentationTransitionEffect::PublishNative { .. }))
    );
    app.video_presentation_transition
        .dispatch(PresentationTransitionEvent::NativeCommitted {
            request_id: request,
            candidate_generation: 1,
        });
    app.execute_video_presentation_transition_effects(ctx);
    assert!(!app.video_presentation_transition.is_transitioning());
    assert!(!app.video_presentation_transition.has_pending_effects());
}

fn run_maintenance_independence_after_live_fork(reactivate: bool) {
    let mut app = setup_app_for_test();
    let ctx = egui::Context::default();
    let root = app.tmp.path().to_path_buf();
    install_items(&mut app, &ctx, &root, false);
    let d = 76_795;
    let d_context = app.push_window_context_for_test(&ctx, d, |owner| {
        owner.items = vec![GridItem::Image(root.join("2.png"))];
        owner.image_metas = vec![None];
        owner.thumbnails = vec![ThumbnailState::Pending];
        owner.rebuild_visible_indices();
        owner.fullscreen_idx = Some(0);
        owner.viewer_presentation = ViewerPresentation::DetachedWindow;
        install_still(owner, &ctx, 0);
    });
    open_grid(&mut app, &ctx, 0);
    let a = app.detached_viewer_window_id().unwrap();
    let initial_owner = app.projected_viewer_context_id();
    assert!(
        !app.detached_viewer_independent_active,
        "A starts linked through the real list open"
    );
    assert!(app.detached_window_manager.runtime(a).unwrap().linked);
    let request = app
        .request_native_video_viewer_presentation(ViewerPresentation::DetachedWindow, false)
        .unwrap();
    let host = 0x7678;
    app.set_detached_window_host_for_test(a, host);
    app.set_detached_window_live_hwnds_for_test([host]);
    detached_placement::with_host_geometry_for_test(a, placement(false).outer, false, || {
        app.poll_video_presentation_transition(&ctx);
    });
    assert!(matches!(
        app.video_presentation_transition.state(),
        PresentationTransitionState::Preparing {
            progress: PreparingProgress::AwaitingNative { .. },
            ..
        }
    ));
    assert!(app.activate_detached_image_window_snapshot(&ctx, d));
    let (c, residence) = app.locate_window_context(a).unwrap();
    assert_ne!(
        c, initial_owner,
        "D activation forks the linked media owner"
    );
    assert_eq!(residence, ContextResidence::AtRest);
    assert_eq!(
        app.detached_window_state(a),
        Some(DetachedWindowState::ParkedLive)
    );
    assert!(!app.detached_window_manager.runtime(a).unwrap().linked);
    app.with_viewer_context(c, |owner| {
        assert!(owner.detached_viewer_independent_active);
        assert_eq!(
            owner.video_presentation_transition.request_id(),
            Some(request),
            "request ownership transfers with C"
        );
    })
    .unwrap();
    let main = (
        app.items.clone(),
        app.selected,
        app.fullscreen_idx,
        app.items_generation,
    );
    if reactivate {
        assert!(app.activate_detached_image_window_snapshot(&ctx, a));
    }
    let active_before = format!("{:?}", app.active_detached_session);
    let d_binding = app.locate_window_context(d);
    let d_runtime = app.detached_window_manager.runtime(d).unwrap().clone();
    let d_content = app
        .with_viewer_context(d_context, |owner| {
            (
                owner.items.clone(),
                owner.fullscreen_idx,
                owner.items_generation,
                owner.viewer_presentation,
                owner.detached_viewer_independent_active,
            )
        })
        .unwrap();
    let globals = (
        app.fs_viewport_shown,
        app.fs_viewport_presentation,
        app.fs_viewport_generation,
        app.detached_viewer_borderless_fullscreen,
        app.detached_viewer_restore_placement,
        format!("{:?}", app.detached_viewer_borderless_transition),
        app.detached_viewer_focus_requested,
    );
    app.with_viewer_context(c, |owner| {
        assert!(
            owner.detached_viewer_independent_active,
            "fork/reactivation has settled independence before completion"
        );
        complete_native_request(owner, &ctx, request, host);
        assert!(
            owner.detached_viewer_independent_active,
            "maintenance inherits current independence"
        );
        assert!(!owner.detached_window_manager.runtime(a).unwrap().linked);
        assert_eq!(owner.detached_viewer_window_id(), Some(a));
        assert_eq!(owner.fullscreen_idx, Some(0));
        if reactivate {
            let items = owner.items.clone();
            let order = owner.visible_indices.clone();
            install_still(owner, &ctx, 2);
            resolve_navigation(owner, &ctx, Some(2), true);
            let b = owner.detached_viewer_window_id().unwrap();
            assert_ne!(
                a, b,
                "the real image navigation still exchanges independent A for B"
            );
            assert_eq!(owner.viewer_context_window(c), Some(b));
            assert_eq!(owner.locate_window_context(a), None);
            assert_eq!(owner.detached_window_state(a), None);
            assert!(owner.detached_viewer_independent_active);
            assert!(!owner.detached_window_manager.runtime(b).unwrap().linked);
            assert_eq!(owner.items, items);
            assert_eq!(owner.visible_indices, order);
            assert_eq!(owner.fullscreen_idx, Some(2));
        }
    })
    .unwrap();
    assert_eq!(
        (
            app.items.clone(),
            app.selected,
            app.fullscreen_idx,
            app.items_generation
        ),
        main
    );
    assert_eq!(app.locate_window_context(d), d_binding);
    assert_eq!(app.viewer_context_window(d_context), Some(d));
    let current_d = app.detached_window_manager.runtime(d).unwrap();
    assert_eq!(
        (
            current_d.state,
            current_d.hwnd,
            current_d.host_incarnation,
            current_d.linked,
            current_d.placement
        ),
        (
            d_runtime.state,
            d_runtime.hwnd,
            d_runtime.host_incarnation,
            d_runtime.linked,
            d_runtime.placement
        )
    );
    assert_eq!(
        app.with_viewer_context(d_context, |owner| {
            (
                owner.items.clone(),
                owner.fullscreen_idx,
                owner.items_generation,
                owner.viewer_presentation,
                owner.detached_viewer_independent_active,
            )
        })
        .unwrap(),
        d_content
    );
    if !reactivate {
        assert_eq!(format!("{:?}", app.active_detached_session), active_before);
        assert_eq!(app.active_detached_window_id(), Some(d));
        assert_eq!(
            app.detached_window_state(a),
            Some(DetachedWindowState::ParkedLive)
        );
        assert_eq!(
            (
                app.fs_viewport_shown,
                app.fs_viewport_presentation,
                app.fs_viewport_generation,
                app.detached_viewer_borderless_fullscreen,
                app.detached_viewer_restore_placement,
                format!("{:?}", app.detached_viewer_borderless_transition),
                app.detached_viewer_focus_requested
            ),
            globals
        );
    }
}

#[test]
fn independent_media_exchange_request_independence_maintenance_after_reactivation() {
    run_maintenance_independence_after_live_fork(true);
}

#[test]
fn independent_media_exchange_request_independence_maintenance_while_parked() {
    run_maintenance_independence_after_live_fork(false);
}

#[test]
fn independent_media_exchange_request_independence_explicit_f12_completion() {
    let mut app = setup_app_for_test();
    let ctx = egui::Context::default();
    let root = app.tmp.path().to_path_buf();
    install_items(&mut app, &ctx, &root, false);
    app.settings.fullfeature_media_window = false;
    app.settings.video_in_window_mode = false;
    open_grid(&mut app, &ctx, 0);
    assert_eq!(app.viewer_presentation, ViewerPresentation::Fullscreen);
    app.toggle_detached_viewer_mode();
    assert!(app.settings.detached_viewer_enabled);
    let request = app.video_presentation_transition.request_id().unwrap();
    let PresentationTransitionState::Preparing {
        request: pending, ..
    } = app.video_presentation_transition.state()
    else {
        panic!("F12 created an asynchronous request")
    };
    assert_eq!(pending.target, ViewerPresentation::DetachedWindow);
    assert_eq!(pending.target_independent, Some(false));
    let a = app.detached_viewer_window_id().unwrap();
    // Use the existing ownership transition, rather than setting the bool by hand.
    app.activate_mounted_as_independent_detached(a);
    assert!(app.detached_viewer_independent_active);
    let host = 0x7679;
    app.set_detached_window_host_for_test(a, host);
    app.set_detached_window_live_hwnds_for_test([host]);
    detached_placement::with_host_geometry_for_test(a, placement(false).outer, false, || {
        app.poll_video_presentation_transition(&ctx);
    });
    complete_native_request(&mut app, &ctx, request, host);
    assert_eq!(app.viewer_presentation, ViewerPresentation::DetachedWindow);
    assert!(
        !app.detached_viewer_independent_active,
        "explicit F12 applies its stored decision at completion"
    );
    assert!(app.detached_window_manager.runtime(a).unwrap().linked);
    assert_eq!(app.active_detached_window_id(), Some(a));
}

#[test]
fn independent_media_exchange_descriptor_activation_and_scan_keep_resolved_session() {
    for toggle_f12 in [false, true] {
        let mut app = setup_app_for_test();
        let ctx = egui::Context::default();
        let root = app.tmp.path().to_path_buf();
        install_items(&mut app, &ctx, &root, false);
        let (_, c) = activate_independent_media(&mut app, &ctx);
        app.with_viewer_context(c, |owner| {
            owner.open_fullscreen_from_fs_navigation(&ctx, 2, HistoryTrigger::UserChosen);
        })
        .unwrap();
        let b = app.active_detached_window_id().unwrap();
        assert!(app.park_and_close_current_active_detached_viewer(&ctx));
        app.retire_context(c, "descriptor_test", |_| ()).unwrap();
        // The main list no longer resolves the parked stamp. Actual activation must
        // reconstruct the snapshot descriptor rather than a retained context/stamp.
        app.items.clear();
        app.fullscreen_idx = None;
        app.settings.detached_viewer_enabled = false;
        app.settings.detached_viewer_open_images_in_window = false;
        assert!(app.activate_detached_image_window_snapshot(&ctx, b));
        let rebuilt = app.active_viewer_context_id().unwrap();
        assert_ne!(rebuilt, c);
        app.with_viewer_context(rebuilt, |owner| {
            assert!(owner.detached_viewer_independent_active);
            assert!(owner.folder_pane_open_pending.is_some());
            assert!(owner.fullscreen_idx.is_none());
            if toggle_f12 {
                owner.toggle_detached_viewer_mode();
                assert!(!owner.detached_viewer_independent_active);
            }
        })
        .unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            let ready = app
                .with_viewer_context(rebuilt, |owner| {
                    owner.poll_mounted_document_open_owners(&ctx);
                    owner.poll_sidecar_restore(&ctx);
                    owner.fullscreen_idx.is_some() && owner.sidecar_restore.is_none()
                })
                .unwrap();
            if ready {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "descriptor scan/open did not complete"
            );
            std::thread::yield_now();
        }
        app.with_viewer_context(rebuilt, |owner| {
            assert_eq!(owner.detached_viewer_window_id(), Some(b));
            assert_eq!(
                owner.viewer_presentation,
                ViewerPresentation::DetachedWindow
            );
            assert_eq!(owner.detached_viewer_independent_active, !toggle_f12);
            let next = owner
                .items
                .iter()
                .position(|item| matches!(item, GridItem::Image(path) if path.ends_with("2.png")))
                .unwrap();
            owner.open_fullscreen_from_fs_navigation(&ctx, next, HistoryTrigger::UserChosen);
            assert_eq!(owner.detached_viewer_window_id(), Some(b));
            assert_eq!(owner.detached_viewer_independent_active, !toggle_f12);
        })
        .unwrap();
    }
}

#[test]
fn independent_media_exchange_deferred_reopen_keeps_session_and_explicit_f12() {
    for kind in 0..3 {
        for f12_steps in 0..3 {
            let mut app = setup_app_for_test();
            let ctx = egui::Context::default();
            let root = app.tmp.path().to_path_buf();
            install_items(&mut app, &ctx, &root, false);
            let (_, c) = activate_independent_media(&mut app, &ctx);
            app.with_viewer_context(c, |owner| {
                owner.open_fullscreen_from_fs_navigation(&ctx, 2, HistoryTrigger::UserChosen);
                let b = owner.detached_viewer_window_id().unwrap();
                owner.fs_viewport_shown = true;
                owner.fs_viewport_presentation = Some(ViewerPresentation::DetachedWindow);
                owner.close_fullscreen_for_folder_nav_reopen();
                assert_eq!(owner.detached_viewer_window_id(), Some(b));
                assert!(owner.fullscreen_idx.is_none());
                for _ in 0..f12_steps {
                    owner.toggle_detached_viewer_mode();
                }
                owner.items[2] = match kind {
                    1 => GridItem::ZipImage {
                        zip_path: root.join("pages.zip"),
                        entry_name: "1.png".into(),
                    },
                    2 => GridItem::PdfPage {
                        pdf_path: root.join("pages.pdf"),
                        page_num: 0,
                        content_type: None,
                    },
                    _ => GridItem::Image(root.join("1.png")),
                };
                install_still(owner, &ctx, 2);
                // An explicit-open bit can survive enumeration. It does not own independence.
                owner.fs_open_intent_from_grid = true;
                owner.open_fullscreen(2, HistoryTrigger::UserChosen);
                assert_eq!(owner.detached_viewer_window_id(), Some(b));
                let target = if f12_steps == 2 {
                    owner.non_detached_viewer_presentation()
                } else {
                    ViewerPresentation::DetachedWindow
                };
                assert_eq!(owner.viewer_presentation, target);
                assert_eq!(owner.detached_viewer_independent_active, f12_steps == 0);
                owner.open_fullscreen_from_fs_navigation(&ctx, 3, HistoryTrigger::UserChosen);
                assert_eq!(owner.detached_viewer_window_id(), Some(b));
                assert_eq!(owner.viewer_presentation, target);
                assert_eq!(owner.detached_viewer_independent_active, f12_steps == 0);
            })
            .unwrap();
        }
    }
}

#[test]
fn independent_media_exchange_retires_inflight_native_request_and_ignores_late_events() {
    let mut app = setup_app_for_test();
    let ctx = egui::Context::default();
    let root = app.tmp.path().to_path_buf();
    install_items(&mut app, &ctx, &root, false);
    let (a, c) = activate_independent_media(&mut app, &ctx);
    app.with_viewer_context(c, |owner| {
        let p = placement(false);
        owner.window_visible = true;
        owner.set_detached_window_host_for_test(a, 0x7672);
        owner.set_detached_window_live_hwnds_for_test([0x7672]);
        detached_placement::with_host_geometry_for_test(a, p.outer, false, || {
            let request = owner
                .request_native_video_viewer_presentation(ViewerPresentation::DetachedWindow, false)
                .unwrap();
            owner.poll_video_presentation_transition(&ctx);
            assert!(matches!(
                owner.video_presentation_transition.state(),
                PresentationTransitionState::Preparing {
                    progress: PreparingProgress::AwaitingNative { .. },
                    ..
                }
            ));
            resolve_navigation(owner, &ctx, Some(2), true);
            let b = owner.detached_viewer_window_id().unwrap();
            assert_ne!(a, b);
            assert!(!owner.video_presentation_transition.is_transitioning());
            assert!(!owner.video_presentation_transition.has_pending_effects());
            owner.video_presentation_transition.dispatch(
                PresentationTransitionEvent::NativeReady {
                    request_id: request,
                    candidate: PresentationCandidate {
                        presenter_hwnd: 0xA,
                        native_generation: 1,
                        host_hwnd: 0x7672,
                        requires_retire: true,
                    },
                },
            );
            owner.video_presentation_transition.dispatch(
                PresentationTransitionEvent::NativeCommitted {
                    request_id: request,
                    candidate_generation: 1,
                },
            );
            assert!(!owner.video_presentation_transition.is_transitioning());
            assert!(!owner.video_presentation_transition.has_pending_effects());
            assert_eq!(owner.detached_viewer_window_id(), Some(b));
            let successor = owner.video_presentation_transition.request_transition(
                ViewerPresentation::MainWindow,
                false,
                false,
                0,
                None,
                None,
            );
            assert!(successor > request);
            owner.video_presentation_transition.dispatch(
                PresentationTransitionEvent::NativeFailed {
                    request_id: request,
                },
            );
            assert_eq!(
                owner.video_presentation_transition.request_id(),
                Some(successor)
            );
        });
    })
    .unwrap();
}

fn wait_for_late_case(label: &str, mut ready: impl FnMut() -> bool) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while !ready() {
        assert!(
            std::time::Instant::now() < deadline,
            "{label} did not finish"
        );
        std::thread::yield_now();
    }
}

fn install_late_collection(app: &mut App, ctx: &egui::Context, root: &Path) {
    use crate::collection_store::{
        CollectionRegistration, CollectionResolvedKind, CollectionStoreRuntime,
    };
    let items = app.items.clone();
    let runtime = CollectionStoreRuntime::start_at(root.join("late-collection.db")).unwrap();
    let client = runtime.client();
    app.install_collection_runtime(runtime);
    wait_for_late_case("collection actor", || {
        app.poll_collection_ui(ctx);
        matches!(app.collection_store_client_for_read(), Ok(Some(_)))
    });
    let created = client
        .create_collection("Late Manual".into())
        .unwrap()
        .recv_timeout(std::time::Duration::from_secs(3))
        .unwrap()
        .unwrap();
    let registrations = items
        .iter()
        .map(|item| {
            let (path, kind) = match item {
                GridItem::Video(path) => (path, CollectionResolvedKind::Video),
                GridItem::Audio(path) => (path, CollectionResolvedKind::Audio),
                GridItem::Image(path) => (path, CollectionResolvedKind::Image),
                _ => unreachable!(),
            };
            CollectionRegistration::from_trusted_path(path, kind).unwrap()
        })
        .collect();
    client
        .add_batch(created.collection_id(), created.revision(), registrations)
        .unwrap()
        .recv_timeout(std::time::Duration::from_secs(3))
        .unwrap()
        .unwrap();
    app.open_collection_grid_from_navigation(created.collection_id());
    wait_for_late_case("collection root", || {
        app.poll_collection_ui(ctx);
        app.poll_collection_grid(ctx);
        app.poll_sidecar_restore(ctx);
        app.items == items && app.sidecar_restore.is_none()
    });
    install_media(app, 0);
    install_still(app, ctx, 2);
    install_still(app, ctx, 3);
}

fn run_late_manual_case(
    audio: bool,
    target_media: bool,
    native_pending: bool,
    collection: bool,
    still_kind: usize,
    cross_kind: bool,
) {
    let case = format!(
        "audio={audio} media={target_media} native_pending={native_pending} collection={collection} kind={still_kind} cross={cross_kind}"
    );
    eprintln!("late Manual case: {case}");
    let mut app = setup_app_for_test();
    let ctx = egui::Context::default();
    let root = app.tmp.path().to_path_buf();
    install_items(&mut app, &ctx, &root, audio);
    if cross_kind {
        app.items[1] = if audio {
            GridItem::Video(root.join("b.mp4"))
        } else {
            GridItem::Audio(root.join("b.mp3"))
        };
        std::fs::write(grid_item_media_path(&app.items[1]).unwrap(), b"fake").unwrap();
    }
    if collection {
        install_late_collection(&mut app, &ctx, &root);
    }
    let (a, c) = activate_independent_media(&mut app, &ctx);
    let d = 76_799;
    let image = root.join("2.png");
    let d_context = app.push_window_context_for_test(&ctx, d, |owner| {
        owner.items = vec![GridItem::Image(image)];
        owner.image_metas = vec![None];
        owner.thumbnails = vec![ThumbnailState::Pending];
        owner.rebuild_visible_indices();
        owner.fullscreen_idx = Some(0);
        owner.viewer_presentation = ViewerPresentation::DetachedWindow;
        install_still(owner, &ctx, 0);
    });
    let (request_tx, request_rx) = mpsc::channel();
    let (result_tx, result_rx) = mpsc::channel();
    app.media_navigation_resolver = Some(MediaNavigationResolver {
        request_tx,
        result_rx,
        next_request_id: 0,
    });
    let (player_identity, old_items, old_generation, old_request) = app
        .with_viewer_context(c, |owner| {
            owner.window_visible = true;
            owner.set_detached_window_host_for_test(a, 0x7673);
            owner.set_detached_window_live_hwnds_for_test([0x7673]);
            if !collection {
                owner.items[2] = match still_kind {
                    1 => GridItem::ZipImage {
                        zip_path: root.join("late.zip"),
                        entry_name: "1.png".into(),
                    },
                    2 => GridItem::PdfPage {
                        pdf_path: root.join("late.pdf"),
                        page_num: 0,
                        content_type: None,
                    },
                    _ => GridItem::Image(root.join("1.png")),
                };
            }
            let identity = match owner.fs_cache.get(&0).unwrap() {
                FsCacheEntry::Video { player, .. } => player.media_identity(),
                _ => unreachable!(),
            };
            let request = if native_pending {
                Some(detached_placement::with_host_geometry_for_test(
                    a,
                    placement(false).outer,
                    false,
                    || {
                        let request = owner
                            .request_native_video_viewer_presentation(
                                ViewerPresentation::DetachedWindow,
                                false,
                            )
                            .unwrap();
                        owner.poll_video_presentation_transition(&ctx);
                        assert!(matches!(
                            owner.video_presentation_transition.state(),
                            PresentationTransitionState::Preparing {
                                progress: PreparingProgress::AwaitingNative { .. },
                                ..
                            }
                        ));
                        request
                    },
                ))
            } else {
                None
            };
            let delta = if target_media { 1 } else { 2 };
            if audio {
                let order = owner.get_nav_indices();
                owner.start_manual_media_navigation(
                    &ctx,
                    &order,
                    0,
                    delta,
                    false,
                    "late_audio",
                    ManualMediaNavigationLanding::Fullscreen,
                );
            } else {
                owner.navigate_native_video_fullscreen(&ctx, 0, delta);
            }
            if collection {
                assert!(owner.top_level_grid_view.collection_navigation_pending());
            }
            (
                identity,
                owner.items.clone(),
                owner.items_generation,
                request,
            )
        })
        .unwrap();
    let request = if collection {
        assert!(
            request_rx.try_recv().is_err(),
            "Collection has its own request owner"
        );
        None
    } else {
        Some(request_rx.try_recv().unwrap())
    };
    let input_seq = app.input_seq;
    assert!(app.activate_detached_image_window_snapshot(&ctx, d));
    assert_eq!(app.input_seq, input_seq);
    assert_eq!(app.active_detached_window_id(), Some(d));
    if !collection {
        assert_eq!(
            app.media_navigation_pending
                .as_ref()
                .unwrap()
                .owner_window_id,
            Some(a)
        );
    }
    app.with_viewer_context(d_context, |owner| {
        owner.toggle_detached_viewer_borderless_fullscreen(&ctx);
    })
    .unwrap();
    let d_session = format!("{:?}", app.active_detached_session);
    let d_runtime = app.detached_window_manager.runtime(d).unwrap().clone();
    let d_globals = (
        app.fs_viewport_shown,
        app.fs_viewport_presentation,
        app.fs_viewport_generation,
        app.detached_viewer_borderless_fullscreen,
        app.detached_viewer_restore_placement,
        format!("{:?}", app.detached_viewer_borderless_transition),
        app.detached_viewer_focus_requested,
        app.fs_video_open_autoplay_override,
        app.fs_video_open_ignore_resume_once,
    );
    let main = (
        app.items.clone(),
        app.selected,
        app.fullscreen_idx,
        app.items_generation,
    );
    let target = if target_media { 1 } else { 2 };
    if let Some(request) = request {
        result_tx
            .send(MediaNavigationResolverResponse {
                request_id: request.request_id,
                result: MediaNavigationResolveResult {
                    target_idx: Some(target),
                    missing: Vec::new(),
                },
            })
            .unwrap();
    }
    // Real ParkedLive polling mounts A and drains its regular or Collection result.
    // New media opens use the established disconnected-output seam, never a native HWND.
    detached_placement::with_host_geometry_for_test(a, placement(false).outer, false, || {
        crate::video::VideoPlayer::with_disconnected_opens_for_test(|| {
            wait_for_late_case(&case, || {
                app.poll_parked_live_detached_windows(&ctx);
                app.with_viewer_context(c, |owner| {
                    if let Some(pending) = owner.native_video_open_pending.as_ref() {
                        assert!(
                            target_media,
                            "discarded still result must not start a native open"
                        );
                        assert_eq!(pending.owner_context_id, c);
                        assert_eq!(pending.idx, target);
                        // Existing native-open seam supplies host readiness and the decoder;
                        // owner, current item, pending completion and continuation remain real.
                        owner.poll_native_video_open_pending_with_readiness(
                            &ctx,
                            |_| true,
                            || 0,
                            |owner, idx| install_media(owner, idx),
                        );
                    }
                    !owner.top_level_grid_view.collection_navigation_pending()
                        && owner.media_navigation_pending.is_none()
                        && (!target_media || owner.fullscreen_idx == Some(target))
                })
                .unwrap()
            });
        });
    });
    assert_eq!(
        format!("{:?}", app.active_detached_session),
        d_session,
        "{case}"
    );
    assert_eq!(app.active_detached_window_id(), Some(d));
    assert_eq!(
        app.locate_window_context(d),
        Some((d_context, ContextResidence::AtRest))
    );
    assert_eq!(app.viewer_context_window(d_context), Some(d));
    let runtime = app.detached_window_manager.runtime(d).unwrap();
    assert_eq!(
        (
            runtime.state,
            runtime.hwnd,
            runtime.host_incarnation,
            runtime.linked,
            runtime.placement
        ),
        (
            d_runtime.state,
            d_runtime.hwnd,
            d_runtime.host_incarnation,
            d_runtime.linked,
            d_runtime.placement
        )
    );
    assert_eq!(
        (
            app.items.clone(),
            app.selected,
            app.fullscreen_idx,
            app.items_generation
        ),
        main
    );
    assert_eq!(
        app.locate_window_context(a),
        Some((c, ContextResidence::AtRest))
    );
    assert_eq!(
        app.detached_window_state(a),
        Some(DetachedWindowState::ParkedLive)
    );
    assert!(
        app.detached_image_windows
            .iter()
            .any(|window| window.id == a)
    );
    assert_eq!(
        (
            app.fs_viewport_shown,
            app.fs_viewport_presentation,
            app.fs_viewport_generation,
            app.detached_viewer_borderless_fullscreen,
            app.detached_viewer_restore_placement,
            format!("{:?}", app.detached_viewer_borderless_transition),
            app.detached_viewer_focus_requested,
            app.fs_video_open_autoplay_override,
            app.fs_video_open_ignore_resume_once,
        ),
        d_globals,
        "late result leaves D's shared F11/focus/output state alone: {case}"
    );
    app.with_viewer_context(c, |owner| {
        assert_eq!(owner.detached_viewer_window_id(), Some(a));
        assert_eq!(
            owner.fullscreen_idx,
            Some(if target_media { target } else { 0 })
        );
        assert_eq!(owner.items, old_items);
        // This fixture's accepted Collection root is replaced once before landing.
        // A rejected still result must return before that install and preserve its generation.
        let expected_generation = if collection && target_media {
            old_generation.wrapping_add(1)
        } else {
            old_generation
        };
        assert_eq!(owner.items_generation, expected_generation);
        assert!(owner.detached_viewer_independent_active);
        if target_media {
            let FsCacheEntry::Video { player, .. } = owner.fs_cache.get(&target).unwrap() else {
                panic!("A owns the successor player: {case}")
            };
            assert_ne!(player.media_identity(), player_identity);
            assert!(
                !owner.fs_cache.contains_key(&0),
                "old player is retired: {case}"
            );
            if let Some(request) = old_request {
                assert!(!owner.video_presentation_transition.is_transitioning());
                assert!(!owner.video_presentation_transition.has_pending_effects());
                owner.video_presentation_transition.dispatch(
                    PresentationTransitionEvent::NativeReady {
                        request_id: request,
                        candidate: PresentationCandidate {
                            presenter_hwnd: 0xA,
                            native_generation: 1,
                            host_hwnd: 0x7673,
                            requires_retire: true,
                        },
                    },
                );
                owner.video_presentation_transition.dispatch(
                    PresentationTransitionEvent::NativeCommitted {
                        request_id: request,
                        candidate_generation: 1,
                    },
                );
                assert!(!owner.video_presentation_transition.is_transitioning());
                assert!(!owner.video_presentation_transition.has_pending_effects());
                assert_eq!(owner.detached_viewer_window_id(), Some(a));
            }
        } else {
            let FsCacheEntry::Video { player, .. } = owner.fs_cache.get(&0).unwrap() else {
                panic!("A still owns its player")
            };
            assert_eq!(player.media_identity(), player_identity);
            assert!(
                player.intent_playing(),
                "rejected still result does not stop A"
            );
            if let Some(request) = old_request {
                assert_eq!(
                    owner.video_presentation_transition.request_id(),
                    Some(request)
                );
                assert!(owner.video_presentation_transition.is_transitioning());
                owner.video_presentation_transition.dispatch(
                    PresentationTransitionEvent::NativeReady {
                        request_id: request,
                        candidate: PresentationCandidate {
                            presenter_hwnd: 0xA,
                            native_generation: 1,
                            host_hwnd: 0x7673,
                            requires_retire: false,
                        },
                    },
                );
                owner.video_presentation_transition.drive();
                let effects = owner.video_presentation_transition.take_effects();
                assert!(effects.iter().any(|effect| matches!(
                    effect,
                    PresentationTransitionEffect::PublishNative { .. }
                )));
                owner.video_presentation_transition.dispatch(
                    PresentationTransitionEvent::NativeCommitted {
                        request_id: request,
                        candidate_generation: 1,
                    },
                );
                // Native readiness/commit are fake facts; App's ApplyPresentation
                // and all subsequent effects run through the real handler.
                owner.execute_video_presentation_transition_effects(&ctx);
                assert!(!owner.video_presentation_transition.is_transitioning());
                assert!(!owner.video_presentation_transition.has_pending_effects());
                assert_eq!(owner.detached_viewer_window_id(), Some(a));
                assert_eq!(owner.fullscreen_idx, Some(0));
                assert!(owner.detached_viewer_independent_active);
            }
        }
    })
    .unwrap();
    assert_eq!(
        format!("{:?}", app.active_detached_session),
        d_session,
        "native completion: {case}"
    );
    assert_eq!(
        app.detached_window_state(a),
        Some(DetachedWindowState::ParkedLive)
    );
    assert_eq!(
        (
            app.fs_viewport_shown,
            app.fs_viewport_presentation,
            app.fs_viewport_generation,
            app.detached_viewer_borderless_fullscreen,
            app.detached_viewer_restore_placement,
            format!("{:?}", app.detached_viewer_borderless_transition),
            app.detached_viewer_focus_requested,
            app.fs_video_open_autoplay_override,
            app.fs_video_open_ignore_resume_once,
        ),
        d_globals,
        "native completion leaves D alone: {case}"
    );
}

#[test]
fn independent_media_exchange_late_manual_result_keeps_active_sibling() {
    for audio in [false, true] {
        for kind in 0..3 {
            run_late_manual_case(audio, false, false, false, kind, false);
        }
    }
}

#[test]
fn independent_media_exchange_late_same_kind_media_is_adopted_in_background() {
    for audio in [false, true] {
        run_late_manual_case(audio, true, false, false, 0, false);
    }
}

#[test]
fn independent_media_exchange_late_cross_kind_media_is_adopted_in_background() {
    for audio in [false, true] {
        for collection in [false, true] {
            run_late_manual_case(audio, true, false, collection, 0, true);
        }
    }
}

#[test]
fn independent_media_exchange_late_manual_during_native_request_keeps_active_sibling() {
    run_late_manual_case(false, false, true, false, 0, false);
}

#[test]
fn independent_media_exchange_late_media_retires_old_native_request() {
    for cross in [false, true] {
        run_late_manual_case(false, true, true, false, 0, cross);
    }
}

#[test]
fn independent_media_exchange_late_collection_manual_uses_same_admission() {
    for audio in [false, true] {
        for media in [false, true] {
            run_late_manual_case(audio, media, false, true, 0, false);
        }
    }
}

#[test]
fn independent_media_exchange_late_media_keeps_existing_startup_gate() {
    let mut app = setup_app_for_test();
    let ctx = egui::Context::default();
    let root = app.tmp.path().to_path_buf();
    install_items(&mut app, &ctx, &root, true);
    let (a, c) = activate_independent_media(&mut app, &ctx);
    let d = 76_798;
    app.push_window_context_for_test(&ctx, d, |owner| {
        owner.items = vec![GridItem::Image(root.join("2.png"))];
        owner.thumbnails = vec![ThumbnailState::Pending];
        owner.image_metas = vec![None];
        owner.rebuild_visible_indices();
        owner.fullscreen_idx = Some(0);
        owner.viewer_presentation = ViewerPresentation::DetachedWindow;
        install_still(owner, &ctx, 0);
    });
    let (request_tx, request_rx) = mpsc::channel();
    let (result_tx, result_rx) = mpsc::channel();
    app.media_navigation_resolver = Some(MediaNavigationResolver {
        request_tx,
        result_rx,
        next_request_id: 0,
    });
    app.with_viewer_context(c, |owner| {
        let order = owner.get_nav_indices();
        owner.start_manual_media_navigation(
            &ctx,
            &order,
            0,
            1,
            false,
            "late_startup",
            ManualMediaNavigationLanding::Fullscreen,
        );
        assert!(owner.media_navigation_pending.is_some());
    })
    .unwrap();
    let request = request_rx.try_recv().unwrap();
    assert!(app.activate_detached_image_window_snapshot(&ctx, d));
    let d_session = format!("{:?}", app.active_detached_session);
    let (startup_tx, startup_rx) = mpsc::channel();
    app.vst3_startup_load = Some(Vst3StartupLoadPending {
        rx: startup_rx,
        progress: Arc::new(Mutex::new(String::new())),
        started_at: std::time::Instant::now(),
    });
    result_tx
        .send(MediaNavigationResolverResponse {
            request_id: request.request_id,
            result: MediaNavigationResolveResult {
                target_idx: Some(1),
                missing: Vec::new(),
            },
        })
        .unwrap();
    wait_for_late_case("parked startup gate", || {
        app.poll_parked_live_detached_windows(&ctx);
        app.with_viewer_context(c, |owner| owner.vst3_deferred_media_open == Some(1))
            .unwrap()
    });
    app.with_viewer_context(c, |owner| {
        assert_eq!(owner.fullscreen_idx, Some(1));
        assert!(
            owner.fs_cache.contains_key(&0),
            "old player remains until the load boundary"
        );
        assert!(
            !owner.fs_cache.contains_key(&1),
            "startup gate prevents early decoder creation"
        );
    })
    .unwrap();
    assert_eq!(format!("{:?}", app.active_detached_session), d_session);
    startup_tx.send(()).unwrap();
    crate::video::VideoPlayer::with_disconnected_opens_for_test(|| {
        app.poll_vst3_startup_load(&ctx)
    });
    assert!(!app.media_startup_load_pending());
    app.with_viewer_context(c, |owner| {
        assert_eq!(owner.vst3_deferred_media_open, None);
        assert!(matches!(
            owner.fs_cache.get(&1),
            Some(FsCacheEntry::Video { .. })
        ));
        assert!(!owner.fs_cache.contains_key(&0));
        assert!(owner.detached_viewer_independent_active);
    })
    .unwrap();
    assert_eq!(format!("{:?}", app.active_detached_session), d_session);
    assert_eq!(
        app.detached_window_state(a),
        Some(DetachedWindowState::ParkedLive)
    );
    assert_eq!(app.detached_viewer_window_id(), None);
}

/// Run real F11 state transitions against the existing fake HWND geometry adapter.
pub(crate) fn prepare_f11_exchange(
    app: &mut App,
    ctx: &egui::Context,
    a: u64,
    p: PhysicalWindowPlacement,
    phase: usize,
) {
    app.set_detached_window_runtime_placement(a, p, "test_f11_exchange");
    if phase == 0 {
        return;
    }
    app.window_visible = true;
    app.set_detached_window_host_for_test(a, 0x7671);
    app.set_detached_window_live_hwnds_for_test([0x7671]);
    detached_placement::with_host_geometry_for_test(a, p.outer, p.maximized, || {
        detached_placement::with_host_normal_outer_for_test(a, p.outer, || {
            app.toggle_detached_viewer_borderless_fullscreen(ctx);
            if phase == 2 {
                return;
            }
            app.step_detached_viewer_borderless_transition(ctx);
            app.step_detached_viewer_borderless_transition(ctx);
            // Complete the synthetic SW_RESTORE acknowledgement for a maximized host.
            detached_placement::set_host_geometry_for_test(a, p.outer, false);
            if matches!(
                app.detached_viewer_borderless_transition.unwrap().phase,
                DetachedViewerBorderlessTransitionPhase::AwaitUnzoom
            ) {
                app.step_detached_viewer_borderless_transition(ctx);
            }
            assert!(app.detached_viewer_borderless_fullscreen);
            app.detached_viewer_borderless_transition
                .as_mut()
                .unwrap()
                .phase = DetachedViewerBorderlessTransitionPhase::Settle {
                until: std::time::Instant::now(),
            };
            app.step_detached_viewer_borderless_transition(ctx);
            if phase >= 3 {
                app.toggle_detached_viewer_borderless_fullscreen(ctx);
                if phase == 4 {
                    app.step_detached_viewer_borderless_transition(ctx);
                    app.step_detached_viewer_borderless_transition(ctx);
                    assert!(!app.detached_viewer_borderless_fullscreen);
                    assert!(app.detached_viewer_restore_placement.is_none());
                    assert!(matches!(
                        app.detached_viewer_borderless_transition.unwrap().phase,
                        DetachedViewerBorderlessTransitionPhase::Settle { .. }
                    ));
                }
            }
        });
    });
}
