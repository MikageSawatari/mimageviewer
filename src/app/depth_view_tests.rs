//! Context/state/handler regressions. No application window or real model is opened.
use super::*;
use crate::depth_parallax::{DepthEntryBlock, DepthViewState};
use crate::fs_exclusive_view::{ExclusiveViewKind, FsExclusiveView};

#[test]
fn depth_wait_status_is_shared_with_panel_and_its_gate_never_owns_lifetime() {
    use crate::ai::runtime::AiRuntimeInitSnapshot;
    use crate::app::depth_view::{DepthWaitStatus, depth_wait_status};
    use crate::depth_parallax::{DepthSourcePhase, DepthSourceWait};
    let mut app = setup_app_for_test();
    depth(&mut app);
    let key = crate::depth_estimation::DepthSourceKey {
        texture_id: egui::TextureId::Managed(781),
    };
    let start = std::time::Instant::now();
    for reason in [
        DepthSourceWait::TexturePending,
        DepthSourceWait::NativeOnly,
        DepthSourceWait::AnnotationBake,
        DepthSourceWait::OriginalHold,
    ] {
        let FsExclusiveView::Depth(state) = &mut app.fs_exclusive_view else {
            unreachable!()
        };
        state.wait_started_at = start;
        state.wait_for_source(reason);
        assert_eq!(
            state.wait_started_at, start,
            "NotYet must not restart a continuous wait"
        );
        for runtime in [
            AiRuntimeInitSnapshot::Dormant,
            AiRuntimeInitSnapshot::Initializing,
        ] {
            let status = depth_wait_status(&app.fs_exclusive_view, &runtime).unwrap();
            assert_eq!(status, DepthWaitStatus::PreparingModel);
            assert_eq!(status.overlay_text(), "3D 写真のモデルを準備中…");
            assert!(!status.visible_at(start, start + std::time::Duration::from_millis(149)));
            assert!(status.visible_at(start, start + std::time::Duration::from_millis(150)));
            assert!(!status.visible_at(start, start - std::time::Duration::from_millis(1)));
        }
        assert_eq!(
            app.depth_panel_status(),
            app.depth_wait_status().unwrap().panel_text()
        );
    }
    app.ai_runtime_init =
        crate::ai::runtime::AiRuntimeInitOwner::failed_for_test("UI status fixture");
    assert_eq!(app.depth_wait_status(), Some(DepthWaitStatus::Calculating));
    assert_eq!(app.depth_panel_status(), "奥行きを計算中");
    assert_eq!(
        app.depth_wait_status().unwrap().overlay_text(),
        "奥行きを計算中…"
    );
    for failed in [false, true] {
        let FsExclusiveView::Depth(state) = &mut app.fs_exclusive_view else {
            unreachable!()
        };
        state.source = if failed {
            DepthSourcePhase::Failed {
                key,
                reason: "reason".into(),
            }
        } else {
            DepthSourcePhase::Ready { key }
        };
        for runtime in [
            AiRuntimeInitSnapshot::Dormant,
            AiRuntimeInitSnapshot::Initializing,
        ] {
            assert_eq!(depth_wait_status(&app.fs_exclusive_view, &runtime), None);
        }
        assert_eq!(
            app.depth_panel_status(),
            if failed {
                "失敗しました (reason)"
            } else {
                ""
            }
        );
    }
    for target in [
        FsExclusiveView::None,
        FsExclusiveView::Panorama(crate::panorama::PanoramaState::new(
            0.0,
            0.0,
            crate::panorama::PanoProjection::Perspective,
        )),
    ] {
        app.set_fs_exclusive_view(target, "depth_wait_status_non_depth");
        assert!(app.depth_wait_status().is_none());
        assert!(app.depth_panel_status().is_empty());
    }
}

#[test]
fn depth_wait_clock_survives_readback_estimation_and_resets_only_after_terminal() {
    use crate::depth_parallax::{DepthSourcePhase, DepthSourceWait};
    let render = renderer();
    let mut app = setup_app_for_test();
    let mut state = DepthViewState::new(app.edit_request_owner_context());
    let old = std::time::Instant::now() - std::time::Duration::from_secs(2);
    state.wait_started_at = old;
    let texture = render.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("depth-wait-clock"),
        size: wgpu::Extent3d {
            width: 2,
            height: 2,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    let key = crate::depth_estimation::DepthSourceKey {
        texture_id: egui::TextureId::Managed(782),
    };
    state
        .select_source(&render.device, &render.queue, &texture, key)
        .unwrap();
    assert!(matches!(state.source, DepthSourcePhase::ReadingBack { .. }));
    assert_eq!(state.wait_started_at, old);
    app.set_fs_exclusive_view(FsExclusiveView::Depth(state), "depth_wait_readback");
    assert!(app.depth_wait_status().is_some());
    let FsExclusiveView::Depth(state) = &mut app.fs_exclusive_view else {
        unreachable!()
    };
    let map = crate::depth_estimation::DepthMap {
        key,
        width: 2,
        height: 2,
        values: vec![half::f16::from_f32(0.5); 4],
        median: 0.5,
    };
    state.source = DepthSourcePhase::Estimating {
        key,
        pending: crate::depth_estimation::DepthPending::completed_for_test(state.context, 80, map),
    };
    assert_eq!(state.wait_started_at, old);
    assert!(app.depth_wait_status().is_some());
    let FsExclusiveView::Depth(state) = &mut app.fs_exclusive_view else {
        unreachable!()
    };
    state.wait_for_source(DepthSourceWait::NativeOnly);
    assert_eq!(state.wait_started_at, old);
    let next_key = crate::depth_estimation::DepthSourceKey {
        texture_id: egui::TextureId::Managed(783),
    };
    state
        .select_source(&render.device, &render.queue, &texture, next_key)
        .unwrap();
    assert_eq!(
        state.wait_started_at, old,
        "new key during pending remains one busy presentation"
    );
    state.source = DepthSourcePhase::Ready { key: next_key };
    state.wait_for_source(DepthSourceWait::TexturePending);
    assert!(state.wait_started_at > old);
    state.wait_started_at = old;
    for failed in [false, true] {
        state.source = if failed {
            DepthSourcePhase::Failed {
                key: next_key,
                reason: "fixture".into(),
            }
        } else {
            DepthSourcePhase::Ready { key: next_key }
        };
        state
            .select_source(&render.device, &render.queue, &texture, key)
            .unwrap();
        assert!(state.wait_started_at > old);
        state.wait_started_at = old;
        state
            .select_source(&render.device, &render.queue, &texture, key)
            .unwrap();
        assert_eq!(
            state.wait_started_at, old,
            "same-key pass never restarts the gate"
        );
    }
}

#[cfg(windows)]
fn renderer() -> egui_wgpu::RenderState {
    static RENDERER: std::sync::OnceLock<egui_wgpu::RenderState> = std::sync::OnceLock::new();
    RENDERER
        .get_or_init(|| {
            let mut descriptor = wgpu::InstanceDescriptor::new_without_display_handle();
            descriptor.backends = crate::wgpu_backend_policy::GpuBackendRequest::Auto.backends();
            descriptor.backend_options.dx12.shader_compiler = wgpu::Dx12Compiler::Fxc;
            let instance = wgpu::Instance::new(descriptor);
            let adapter =
                pollster::block_on(instance.request_adapter(&Default::default())).unwrap();
            let (device, queue) =
                pollster::block_on(adapter.request_device(&Default::default())).unwrap();
            let target_format = wgpu::TextureFormat::Rgba8Unorm;
            let renderer = egui_wgpu::Renderer::new(&device, target_format, Default::default());
            egui_wgpu::RenderState {
                instance,
                adapter,
                available_adapters: Vec::new(),
                device,
                queue,
                target_format,
                renderer: Arc::new(egui::epaint::mutex::RwLock::new(renderer)),
            }
        })
        .clone()
}

fn install_still(app: &mut App, ctx: &egui::Context, idx: usize, size: [usize; 2]) {
    let pixels = Arc::new(egui::ColorImage::filled(size, egui::Color32::WHITE));
    let tex = ctx.load_texture(
        format!("depth_test_{idx}"),
        (*pixels).clone(),
        Default::default(),
    );
    app.fs_cache.insert(
        idx,
        FsCacheEntry::Static {
            pdf_metadata: None,
            pdf_rerender_failures: Vec::new(),
            tex,
            pixels,
            source_dims: Some(size),
            load_seq: 0,
            animation: crate::fs_animation::StaticAnimationState::Still,
        },
    );
}

pub(super) fn install_pages(app: &mut App, ctx: &egui::Context) {
    app.items = vec![
        GridItem::Image("depth-first.jpg".into()),
        GridItem::Video("depth-video.mp4".into()),
        GridItem::Image("depth-panorama.jpg".into()),
        GridItem::Image("depth-last.jpg".into()),
    ];
    app.image_metas = vec![None; app.items.len()];
    app.thumbnails = (0..app.items.len())
        .map(|_| ThumbnailState::Pending)
        .collect();
    app.rebuild_visible_indices();
    app.fullscreen_idx = Some(0);
    app.spread_mode = crate::settings::SpreadMode::Single;
    app.reading_flow = crate::settings::ReadingFlow::Paged;
    #[cfg(windows)]
    {
        app.viewer_presentation = ViewerPresentation::Fullscreen;
        app.wgpu_render_state = Some(renderer());
    }
    install_still(app, ctx, 0, [2, 3]);
    install_still(app, ctx, 2, [2, 1]);
    install_still(app, ctx, 3, [2, 3]);
}

fn depth(app: &mut App) {
    let state = DepthViewState::new(app.edit_request_owner_context());
    app.set_fs_exclusive_view(FsExclusiveView::Depth(state), "depth_test_on");
}

#[cfg(windows)]
struct DepthOwnerEvidence {
    cancel: Option<Arc<AtomicBool>>,
    resource_id: Option<u64>,
}

#[cfg(windows)]
fn isolate_depth_renderer(app: &mut App) -> egui_wgpu::RenderState {
    let mut render = app.wgpu_render_state.clone().unwrap();
    // Share the device, but never callback slots between independently running tests.
    render.renderer = Arc::new(egui::epaint::mutex::RwLock::new(egui_wgpu::Renderer::new(
        &render.device,
        render.target_format,
        Default::default(),
    )));
    app.wgpu_render_state = Some(render.clone());
    render
}

/// Exercise pending cancellation and published GPU ownership in separate, valid phases.
#[cfg(windows)]
fn tracked_depth(app: &mut App, ready: bool) -> DepthOwnerEvidence {
    let render = app.wgpu_render_state.clone().unwrap();
    let context = app.edit_request_owner_context();
    let key = crate::depth_estimation::DepthSourceKey {
        texture_id: egui::TextureId::Managed(88_071),
    };
    let map = crate::depth_estimation::DepthMap {
        key,
        width: 2,
        height: 2,
        values: vec![half::f16::from_f32(0.5); 4],
        median: 0.5,
    };
    let mut state = DepthViewState::new(context);
    state.attach_renderer(&render);
    let evidence = if ready {
        let source = render.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("depth-owner-retirement-source"),
            size: wgpu::Extent3d {
                width: 2,
                height: 2,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        state.lru.insert(
            key,
            Arc::new(
                crate::depth_parallax_wgpu::DepthTexture::upload(
                    &render.device,
                    &render.queue,
                    map,
                )
                .unwrap(),
            ),
        );
        state
            .select_source(&render.device, &render.queue, &source, key)
            .unwrap();
        let resource_id = state.ready().unwrap().resource_id;
        state.publish(&mut render.renderer.write().callback_resources);
        DepthOwnerEvidence {
            cancel: None,
            resource_id: Some(resource_id),
        }
    } else {
        let pending =
            crate::depth_estimation::DepthPending::completed_for_test(context, 70_311, map);
        let cancel = pending.cancellation_token_for_test();
        assert!(!cancel.load(Ordering::Acquire));
        state.source = crate::depth_parallax::DepthSourcePhase::Estimating { key, pending };
        DepthOwnerEvidence {
            cancel: Some(cancel),
            resource_id: None,
        }
    };
    app.set_fs_exclusive_view(FsExclusiveView::Depth(state), "depth_owner_test");
    evidence
}

#[cfg(windows)]
fn assert_depth_owner_live(app: &App, evidence: &DepthOwnerEvidence) {
    let FsExclusiveView::Depth(state) = &app.fs_exclusive_view else {
        panic!("sibling must retain its Depth owner");
    };
    if let Some(cancel) = &evidence.cancel {
        assert!(!cancel.load(Ordering::Acquire));
        assert!(matches!(
            &state.source,
            crate::depth_parallax::DepthSourcePhase::Estimating { pending, .. }
                if pending.request_id() == 70_311
        ));
    }
    if let Some(resource_id) = evidence.resource_id {
        assert_eq!(state.ready().unwrap().resource_id, resource_id);
        assert!(state.lru.resident_bytes() > 0);
    }
}

#[cfg(windows)]
fn assert_depth_retired_with_sibling_slot(
    render: &egui_wgpu::RenderState,
    retired: &DepthOwnerEvidence,
    sibling: &DepthOwnerEvidence,
) {
    if let Some(cancel) = &retired.cancel {
        assert!(cancel.load(Ordering::Acquire));
    }
    let renderer = render.renderer.read();
    let slot = renderer
        .callback_resources
        .get::<crate::depth_parallax_wgpu::DepthRenderResourcesRef>();
    assert_eq!(slot.map(|slot| slot.0.resource_id), sibling.resource_id);
}

#[cfg(windows)]
#[test]
fn depth_actual_independent_activation_retires_own_request_or_slot_and_keeps_sibling() {
    for ready in [false, true] {
        let ctx = egui::Context::default();
        let mut app = setup_app_for_test();
        install_pages(&mut app, &ctx);
        let render = isolate_depth_renderer(&mut app);
        let own = tracked_depth(&mut app, ready);
        let mut sibling_evidence = None;
        let sibling = app.build_window_context_for_test(70_312, |owner| {
            sibling_evidence = Some(tracked_depth(owner, !ready));
        });
        let sibling_evidence = sibling_evidence.unwrap();

        app.activate_mounted_as_independent_detached(70_313);

        assert_eq!(app.viewer_presentation, ViewerPresentation::DetachedWindow);
        assert!(matches!(app.fs_exclusive_view, FsExclusiveView::None));
        assert_depth_retired_with_sibling_slot(&render, &own, &sibling_evidence);
        app.with_viewer_context(sibling, |owner| {
            assert_depth_owner_live(owner, &sibling_evidence);
        })
        .unwrap();
        assert_depth_retired_with_sibling_slot(&render, &own, &sibling_evidence);
    }
}

#[cfg(windows)]
#[test]
fn depth_actual_forks_retire_before_new_identity_and_keep_sibling_request_or_slot() {
    for live in [false, true] {
        for ready in [false, true] {
            let ctx = egui::Context::default();
            let mut app = setup_app_for_test();
            install_pages(&mut app, &ctx);
            let folder = app.tmp.path().join("depth-owner-fork");
            app.current_folder = Some(folder.clone());
            app.items[0] = GridItem::Image(folder.join("page.jpg"));
            let main = app.viewer_context_main();
            let render = isolate_depth_renderer(&mut app);
            let own = tracked_depth(&mut app, ready);
            let mut sibling_evidence = None;
            let sibling = app.build_window_context_for_test(70_314, |owner| {
                sibling_evidence = Some(tracked_depth(owner, !ready));
            });
            let sibling_evidence = sibling_evidence.unwrap();

            let child = if live {
                app.bind_window(main, 70_315).unwrap();
                app.fork_mounted_live_media_context(70_315)
            } else {
                app.fork_materialized_still_context(&folder, 0)
            };

            assert_ne!(child, main);
            assert!(matches!(app.fs_exclusive_view, FsExclusiveView::None));
            assert_depth_retired_with_sibling_slot(&render, &own, &sibling_evidence);
            app.with_viewer_context(child, |owner| {
                assert!(matches!(owner.fs_exclusive_view, FsExclusiveView::None));
            })
            .unwrap();
            app.with_viewer_context(sibling, |owner| {
                assert_depth_owner_live(owner, &sibling_evidence);
            })
            .unwrap();
            assert_depth_retired_with_sibling_slot(&render, &own, &sibling_evidence);
        }
    }
}

#[cfg(windows)]
#[test]
fn depth_fork_retirement_preserves_existing_panorama_state_and_intent_transfer() {
    for live in [false, true] {
        let ctx = egui::Context::default();
        let mut app = setup_app_for_test();
        install_pages(&mut app, &ctx);
        let folder = app.tmp.path().join("panorama-owner-fork");
        app.current_folder = Some(folder.clone());
        app.items[0] = GridItem::Image(folder.join("page.jpg"));
        app.panorama_intent.on = true;
        app.panorama_intent.projection = Some(crate::panorama::PanoProjection::Equidistant);
        let mut panorama = crate::panorama::PanoramaState::new(
            0.4,
            -0.2,
            crate::panorama::PanoProjection::Equidistant,
        );
        panorama.fov_y = 2.0;
        panorama.drag_active = true;
        panorama.last_pointer = Some(egui::pos2(11.0, 13.0));
        app.set_fs_exclusive_view(FsExclusiveView::Panorama(panorama), "panorama_fork_test");
        let main = app.viewer_context_main();
        let child = if live {
            app.bind_window(main, 70_316).unwrap();
            app.fork_mounted_live_media_context(70_316)
        } else {
            app.fork_materialized_still_context(&folder, 0)
        };
        assert!(matches!(app.fs_exclusive_view, FsExclusiveView::None));
        assert!(!app.panorama_intent.on);
        app.with_viewer_context(child, |owner| {
            let panorama = owner.panorama_state().unwrap();
            assert_eq!(
                (panorama.yaw, panorama.pitch, panorama.fov_y),
                (0.4, -0.2, 2.0)
            );
            assert_eq!(
                panorama.projection,
                crate::panorama::PanoProjection::Equidistant
            );
            assert!(panorama.drag_active);
            assert_eq!(panorama.last_pointer, Some(egui::pos2(11.0, 13.0)));
            assert!(owner.panorama_intent.on);
            assert_eq!(
                owner.panorama_intent.projection,
                Some(crate::panorama::PanoProjection::Equidistant)
            );
        })
        .unwrap();
    }
}

#[test]
fn depth_replaces_active_or_inactive_panorama_and_clears_only_its_intent() {
    let ctx = egui::Context::default();
    let mut app = setup_app_for_test();
    install_pages(&mut app, &ctx);
    for idx in [0, 2] {
        app.fullscreen_idx = Some(idx);
        app.panorama_intent.on = true;
        app.panorama_intent.projection = Some(crate::panorama::PanoProjection::Equidistant);
        app.set_fs_exclusive_view(
            FsExclusiveView::Panorama(crate::panorama::PanoramaState::new(
                0.25,
                -0.1,
                crate::panorama::PanoProjection::Equidistant,
            )),
            "panorama_test_on",
        );
        assert_eq!(app.is_panorama_mode_active(idx), idx == 2);
        depth(&mut app);
        assert!(app.panorama_state().is_none());
        assert!(!app.panorama_intent.on);
        assert_eq!(
            app.panorama_intent.projection,
            Some(crate::panorama::PanoProjection::Equidistant)
        );
        assert_eq!(
            app.fs_exclusive_view_active(idx),
            Some(ExclusiveViewKind::Depth)
        );
        app.toggle_panorama_mode(idx);
        app.reconcile_panorama_session_intent(idx);
        assert!(app.fs_exclusive_view.is_depth());
        assert!(!app.panorama_intent.on);
    }
    app.set_fs_exclusive_view(FsExclusiveView::None, "depth_test_off");
    app.fullscreen_idx = Some(2);
    app.reconcile_panorama_session_intent(2);
    assert!(matches!(app.fs_exclusive_view, FsExclusiveView::None));
}

#[cfg(windows)]
#[test]
fn depth_continues_between_stills_but_video_cannot_resurrect_it() {
    let ctx = egui::Context::default();
    let mut app = setup_app_for_test();
    install_pages(&mut app, &ctx);
    assert_eq!(app.depth_entry_allowed(0), Ok(()));
    depth(&mut app);
    for idx in [0, 2, 3] {
        app.fullscreen_idx = Some(idx);
        assert!(app.depth_view_may_continue(idx));
    }
    app.fullscreen_idx = Some(1);
    assert_eq!(app.depth_entry_allowed(1), Err(DepthEntryBlock::Video));
    assert!(!app.depth_view_may_continue(1));
    assert!(matches!(app.fs_exclusive_view, FsExclusiveView::None));
    app.fullscreen_idx = Some(2);
    assert!(!app.depth_view_may_continue(2));
    app.reconcile_panorama_session_intent(2);
    assert!(matches!(app.fs_exclusive_view, FsExclusiveView::None));
}

#[cfg(windows)]
#[test]
fn depth_missing_model_opens_setup_without_capturing_or_replacing_panorama() {
    let ctx = egui::Context::default();
    let mut app = setup_app_for_test();
    install_pages(&mut app, &ctx);
    app.fullscreen_idx = Some(2);
    app.toggle_panorama_mode(2);
    assert!(app.panorama_intent.on);
    assert!(app.depth_setup.installed_model_path().is_none());
    app.toggle_depth_parallax(&ctx, 2);
    assert!(app.depth_setup.modal_open());
    assert!(app.panorama_state().is_some());
    assert!(app.panorama_intent.on);
    assert!(!app.fs_exclusive_view.is_depth());
    app.fullscreen_idx = Some(0);
    app.poll_depth_setup(&ctx);
    assert!(!app.fs_exclusive_view.is_depth());
}

#[cfg(windows)]
#[test]
fn depth_continuation_ends_for_animation_continuous_reading_and_closed_viewer() {
    let ctx = egui::Context::default();
    let mut app = setup_app_for_test();
    for case in ["animation", "continuous", "closed"] {
        install_pages(&mut app, &ctx);
        depth(&mut app);
        let expected = match case {
            "animation" => {
                if let Some(FsCacheEntry::Static { animation, .. }) = app.fs_cache.get_mut(&0) {
                    *animation = crate::fs_animation::StaticAnimationState::FirstFrameOnly(
                        crate::canonical_image_loader::CanonicalAnimatedFormat::Gif,
                    );
                }
                DepthEntryBlock::Animation
            }
            "continuous" => {
                app.reading_flow = crate::settings::ReadingFlow::Vertical;
                DepthEntryBlock::ContinuousReading
            }
            _ => {
                app.fullscreen_idx = None;
                DepthEntryBlock::NotImageFullscreen
            }
        };
        assert_eq!(app.depth_entry_allowed(0), Err(expected), "{case}");
        assert!(!app.depth_view_may_continue(0), "{case}");
        assert!(
            matches!(app.fs_exclusive_view, FsExclusiveView::None),
            "{case}"
        );
        install_pages(&mut app, &ctx);
        assert!(!app.depth_view_may_continue(0), "{case} must not resume");
    }
}

#[cfg(windows)]
#[test]
fn depth_rejects_spread_and_editing_at_the_shared_entry_boundary() {
    let ctx = egui::Context::default();
    let mut app = setup_app_for_test();
    install_pages(&mut app, &ctx);
    app.erase_mode = true;
    assert_eq!(app.depth_entry_allowed(0), Err(DepthEntryBlock::Editing));
    app.erase_mode = false;
    app.items = vec![
        GridItem::Image("first.jpg".into()),
        GridItem::Image("second.jpg".into()),
    ];
    app.image_metas = vec![None; app.items.len()];
    app.thumbnails = (0..app.items.len())
        .map(|_| ThumbnailState::Pending)
        .collect();
    app.rebuild_visible_indices();
    install_still(&mut app, &ctx, 1, [2, 3]);
    app.spread_mode = crate::settings::SpreadMode::Ltr;
    assert!(matches!(
        app.resolve_visible_spread_pair(0),
        crate::ui_fullscreen::SpreadPair::Double { .. }
    ));
    depth(&mut app);
    assert_eq!(app.depth_entry_allowed(0), Err(DepthEntryBlock::Spread));
    assert!(!app.depth_view_may_continue(0));
    app.spread_mode = crate::settings::SpreadMode::Single;
    assert!(!app.depth_view_may_continue(0));
}

#[test]
fn depth_close_and_foreground_park_remove_the_context_owner() {
    let ctx = egui::Context::default();
    let mut app = setup_app_for_test();
    install_pages(&mut app, &ctx);
    depth(&mut app);
    app.reset_detached_pause_foreground_modes(0);
    assert!(matches!(app.fs_exclusive_view, FsExclusiveView::None));
    depth(&mut app);
    app.close_fullscreen();
    assert!(matches!(app.fs_exclusive_view, FsExclusiveView::None));
    assert!(app.fullscreen_idx.is_none());
}

#[cfg(windows)]
#[test]
fn depth_presentation_commit_retires_before_detached_or_main_window_can_draw() {
    let ctx = egui::Context::default();
    let mut app = setup_app_for_test();
    for target in [
        ViewerPresentation::DetachedWindow,
        ViewerPresentation::MainWindow,
    ] {
        install_pages(&mut app, &ctx);
        depth(&mut app);
        app.commit_viewer_presentation(target, "depth_test_presentation");
        assert!(matches!(app.fs_exclusive_view, FsExclusiveView::None));
        assert_eq!(app.viewer_presentation, target);
        app.commit_viewer_presentation(ViewerPresentation::Fullscreen, "depth_test_return");
        assert!(!app.depth_view_may_continue(0));
    }
}

#[cfg(windows)]
#[test]
fn depth_actual_still_window_toggle_retires_before_the_next_presentation() {
    let ctx = egui::Context::default();
    let mut app = setup_app_for_test();
    install_pages(&mut app, &ctx);
    app.settings.video_in_window_mode = false;
    app.native_video_in_window_active = false;
    depth(&mut app);
    app.toggle_still_window_mode();
    assert_eq!(app.viewer_presentation, ViewerPresentation::MainWindow);
    assert!(matches!(app.fs_exclusive_view, FsExclusiveView::None));
    app.toggle_still_window_mode();
    assert_eq!(app.viewer_presentation, ViewerPresentation::Fullscreen);
    assert!(!app.depth_view_may_continue(0));
    assert!(app.fullscreen_idx.is_some());
}

#[cfg(windows)]
#[test]
fn depth_actual_f12_transition_has_no_depth_frame_in_detached_or_after_return() {
    let ctx = egui::Context::default();
    let mut app = setup_app_for_test();
    install_pages(&mut app, &ctx);
    app.settings.detached_viewer_enabled = false;
    app.settings.detached_viewer_open_images_in_window = false;
    app.settings.video_in_window_mode = false;
    depth(&mut app);
    app.toggle_detached_viewer_mode();
    assert!(app.settings.detached_viewer_enabled);
    assert_eq!(app.viewer_presentation, ViewerPresentation::DetachedWindow);
    assert!(matches!(app.fs_exclusive_view, FsExclusiveView::None));
    assert!(!app.depth_view_may_continue(0));
    assert_eq!(app.fs_exclusive_view_active(0), None);
    app.toggle_detached_viewer_mode();
    assert!(!app.settings.detached_viewer_enabled);
    assert_eq!(app.viewer_presentation, ViewerPresentation::Fullscreen);
    assert!(!app.depth_view_may_continue(0));
}

#[cfg(windows)]
pub(super) fn depth_key_frame(app: &mut App, ctx: &egui::Context, keys: &[(egui::Key, bool)]) {
    ctx.begin_pass(egui::RawInput {
        events: keys
            .iter()
            .map(|&(key, pressed)| egui::Event::Key {
                key,
                physical_key: Some(key),
                pressed,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            })
            .collect(),
        ..Default::default()
    });
    let idx = app.fullscreen_idx.unwrap();
    let _ = app.handle_fs_key_input(ctx, idx, false);
    ctx.end_pass().drop_without_applying_deltas();
}

#[cfg(windows)]
#[test]
fn depth_actual_z_d_keys_preserve_committed_zoom_through_idle_and_blocked_input() {
    let _input_guard = crate::key_input::lock_test_input();
    let ctx = egui::Context::default();
    let mut app = setup_app_for_test();
    install_pages(&mut app, &ctx);
    let model_path = app.tmp.path().join("unused-depth-model.onnx");
    app.depth_setup
        .set_status_for_test(crate::addon::AddonStatus::Valid {
            model_id: "depth-key-fixture".to_owned(),
            bytes: 1,
            model_path,
            mmproj_path: None,
        });
    app.fs_zoom_factor = 2.75;
    depth_key_frame(&mut app, &ctx, &[(egui::Key::Z, true)]);
    assert!(app.fs_zoom_aiming);
    depth_key_frame(&mut app, &ctx, &[(egui::Key::Z, false)]);
    assert!(app.fs_zoom_active);
    assert!(!app.fs_zoom_aiming);

    depth_key_frame(
        &mut app,
        &ctx,
        &[(egui::Key::D, true), (egui::Key::D, false)],
    );
    assert!(app.fs_exclusive_view.is_depth());
    assert!(app.fs_zoom_active);
    assert_eq!(app.fs_zoom_factor, 2.75);
    depth_key_frame(&mut app, &ctx, &[]);
    assert!(
        app.fs_zoom_active,
        "idle Depth input must preserve committed 2D zoom"
    );
    depth_key_frame(&mut app, &ctx, &[(egui::Key::Z, true)]);
    assert!(
        app.fs_zoom_active,
        "blocked Z must not toggle the suspended zoom"
    );
    assert!(!app.fs_zoom_aiming);
    depth_key_frame(&mut app, &ctx, &[(egui::Key::Z, false)]);
    assert!(app.fs_zoom_active);
    assert_eq!(app.fs_zoom_factor, 2.75);
    depth_key_frame(
        &mut app,
        &ctx,
        &[(egui::Key::D, true), (egui::Key::D, false)],
    );
    assert!(matches!(app.fs_exclusive_view, FsExclusiveView::None));
    assert!(app.fs_zoom_active);
    assert!(!app.fs_zoom_aiming);
    assert_eq!(app.fs_zoom_factor, 2.75);

    // Panorama keeps its released contract: entering it clears committed zoom.
    app.fullscreen_idx = Some(2);
    depth_key_frame(
        &mut app,
        &ctx,
        &[(egui::Key::V, true), (egui::Key::V, false)],
    );
    assert!(app.panorama_state().is_some());
    depth_key_frame(&mut app, &ctx, &[]);
    assert!(!app.fs_zoom_active);
    assert_eq!(app.fs_zoom_factor, 2.75);
    crate::key_input::clear_test_frame();
}

#[cfg(windows)]
#[test]
fn depth_actual_touch_page_navigation_ignores_suspended_committed_z_zoom() {
    // The immediate renderer is thread-local: drive the real child viewer without
    // opening a native window or leaving a backend callback on a test-pool thread.
    std::thread::spawn(|| {
        let _input_guard = crate::key_input::lock_test_input();
        let mut destinations = Vec::new();
        for committed_zoom in [false, true] {
            let ctx = egui::Context::default();
            ctx.set_embed_viewports(false);
            crate::ui_fullscreen::install_fs_navigator_input_tracking(&ctx);
            let events = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
            let child_events = events.clone();
            let child_passes = std::rc::Rc::new(std::cell::Cell::new(0));
            let observed = child_passes.clone();
            let viewport_rect =
                egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1200.0, 800.0));
            egui::Context::set_immediate_viewport_renderer(move |ctx, mut viewport| {
                observed.set(observed.get() + 1);
                let input_events = std::mem::take(&mut *child_events.borrow_mut());
                ctx.run_ui(
                    egui::RawInput {
                        viewport_id: viewport.ids.this,
                        screen_rect: Some(viewport_rect),
                        time: Some(observed.get() as f64 / 60.0),
                        events: input_events,
                        viewports: [
                            (
                                egui::ViewportId::ROOT,
                                egui::ViewportInfo {
                                    focused: Some(false),
                                    ..Default::default()
                                },
                            ),
                            (
                                viewport.ids.this,
                                egui::ViewportInfo {
                                    parent: Some(egui::ViewportId::ROOT),
                                    focused: Some(true),
                                    inner_rect: Some(viewport_rect),
                                    native_pixels_per_point: Some(1.0),
                                    ..Default::default()
                                },
                            ),
                        ]
                        .into_iter()
                        .collect(),
                        ..Default::default()
                    },
                    |ui| (viewport.viewport_ui_cb)(ui),
                )
                .drop_without_applying_deltas();
            });
            let mut app = setup_app_for_test();
            install_pages(&mut app, &ctx);
            app.items[1] = GridItem::Image("depth-touch-next.jpg".into());
            install_still(&mut app, &ctx, 1, [2, 3]);
            app.window_visible = false; // No depth worker/model during this input test.
            app.settings.video_in_window_mode = false;
            app.settings.detached_viewer_enabled = false;
            app.settings.touch_still_chrome_learned = true;
            let model_path = app.tmp.path().join("unused-depth-model.onnx");
            app.depth_setup
                .set_status_for_test(crate::addon::AddonStatus::Valid {
                    model_id: "depth-touch-fixture".to_owned(),
                    bytes: 1,
                    model_path,
                    mmproj_path: None,
                });
            app.fs_zoom_factor = 2.75;
            if committed_zoom {
                depth_key_frame(&mut app, &ctx, &[(egui::Key::Z, true)]);
                assert!(app.fs_zoom_aiming);
                depth_key_frame(&mut app, &ctx, &[(egui::Key::Z, false)]);
                assert!(app.fs_zoom_active);
                assert!(!app.fs_zoom_aiming);
            }
            depth_key_frame(
                &mut app,
                &ctx,
                &[(egui::Key::D, true), (egui::Key::D, false)],
            );
            // Hide the actual Depth panel through its key handler so that the
            // page-side tap is centered vertically and clear of panel widgets.
            depth_key_frame(
                &mut app,
                &ctx,
                &[(egui::Key::I, true), (egui::Key::I, false)],
            );
            assert!(matches!(
                &app.fs_exclusive_view,
                FsExclusiveView::Depth(state) if !state.panel_visible
            ));
            let render_frame = |app: &mut App| {
                app.frame_counter += 1;
                ctx.run_pass(
                    egui::RawInput {
                        screen_rect: Some(viewport_rect),
                        ..Default::default()
                    },
                    |ctx| ctx.root_ui(|root_ui| app.render_fullscreen_viewport(root_ui)),
                )
                .drop_without_applying_deltas();
            };
            render_frame(&mut app);
            assert_eq!(app.fullscreen_idx, Some(0));
            for phase in [egui::TouchPhase::Start, egui::TouchPhase::End] {
                assert_eq!(
                    app.fs_zoom_owner(),
                    crate::ui_fullscreen::FsZoomOwner::Depth
                );
                assert_eq!(
                    app.fs_exclusive_view_active(0),
                    Some(ExclusiveViewKind::Depth)
                );
                let pos = egui::pos2(900.0, 400.0);
                // Match egui-winit's ordered synthetic-pointer signature. Raw
                // Touch alone leaves correlation pending and cannot authorize a tap.
                events.borrow_mut().push(egui::Event::Touch {
                    device_id: egui::TouchDeviceId(88_072),
                    id: egui::TouchId(1),
                    phase,
                    pos,
                    force: None,
                });
                if phase == egui::TouchPhase::Start {
                    events.borrow_mut().push(egui::Event::PointerMoved(pos));
                }
                events.borrow_mut().push(egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed: phase == egui::TouchPhase::Start,
                    modifiers: egui::Modifiers::NONE,
                });
                if phase == egui::TouchPhase::End {
                    events.borrow_mut().push(egui::Event::PointerGone);
                }
                render_frame(&mut app);
                assert!(
                    events.borrow().is_empty(),
                    "child viewport must receive the touch batch"
                );
                assert_eq!(app.fs_zoom_active, committed_zoom);
                assert_eq!(app.fs_zoom_factor, 2.75);
            }
            assert!(
                child_passes.get() >= 3,
                "the actual child viewer UI must run"
            );
            assert_eq!(
                app.fullscreen_idx,
                Some(1),
                "raw touch must reach PageSide navigation with committed_zoom={committed_zoom}"
            );
            assert!(app.fs_exclusive_view.is_depth());
            assert_eq!(
                app.fs_zoom_owner(),
                crate::ui_fullscreen::FsZoomOwner::Depth
            );
            destinations.push(app.items[app.fullscreen_idx.unwrap()].clone());
            depth_key_frame(
                &mut app,
                &ctx,
                &[(egui::Key::D, true), (egui::Key::D, false)],
            );
            assert!(matches!(app.fs_exclusive_view, FsExclusiveView::None));
            assert_eq!(app.fs_zoom_active, committed_zoom);
            assert!(!app.fs_zoom_aiming);
            assert_eq!(app.fs_zoom_factor, 2.75);
            assert_eq!(
                app.fs_zoom_owner(),
                if committed_zoom {
                    crate::ui_fullscreen::FsZoomOwner::ZoomMode
                } else {
                    crate::ui_fullscreen::FsZoomOwner::Image
                }
            );
            crate::key_input::clear_test_frame();
        }
        assert_eq!(
            destinations[0],
            GridItem::Image("depth-touch-next.jpg".into())
        );
        assert_eq!(destinations[1], destinations[0]);
    })
    .join()
    .unwrap();
}

#[cfg(windows)]
#[test]
fn depth_actual_viewer_pass_retires_video_spread_continuous_and_late_animation() {
    // egui's immediate renderer is thread-local. Keep this test's backend callback
    // on its own thread and execute real viewer UI passes without creating a window.
    std::thread::spawn(|| {
        let _input_guard = crate::key_input::lock_test_input();
        for case in ["video", "spread", "continuous", "late_animation"] {
            let ctx = egui::Context::default();
            ctx.set_embed_viewports(false);
            crate::ui_fullscreen::install_fs_navigator_input_tracking(&ctx);
            let child_passes = std::rc::Rc::new(std::cell::Cell::new(0));
            let observed = child_passes.clone();
            egui::Context::set_immediate_viewport_renderer(move |ctx, mut viewport| {
                observed.set(observed.get() + 1);
                ctx.run_ui(
                    egui::RawInput {
                        viewport_id: viewport.ids.this,
                        screen_rect: Some(egui::Rect::from_min_size(
                            egui::Pos2::ZERO,
                            egui::vec2(820.0, 600.0),
                        )),
                        viewports: [
                            (egui::ViewportId::ROOT, egui::ViewportInfo::default()),
                            (
                                viewport.ids.this,
                                egui::ViewportInfo {
                                    parent: Some(egui::ViewportId::ROOT),
                                    focused: Some(true),
                                    ..Default::default()
                                },
                            ),
                        ]
                        .into_iter()
                        .collect(),
                        ..Default::default()
                    },
                    |ui| (viewport.viewport_ui_cb)(ui),
                )
                .drop_without_applying_deltas();
            });
            let mut app = setup_app_for_test();
            install_pages(&mut app, &ctx);
            // Do not initialize a worker/model while testing presentation routing.
            app.window_visible = false;
            app.settings.video_in_window_mode = false;
            app.settings.detached_viewer_enabled = false;
            app.native_video_in_window_active = false;
            depth(&mut app);
            match case {
                "video" => app.fullscreen_idx = Some(1),
                "spread" => {
                    app.items[1] = GridItem::Image("depth-spread-second.jpg".into());
                    install_still(&mut app, &ctx, 1, [2, 3]);
                    app.spread_mode = crate::settings::SpreadMode::Ltr;
                    assert!(matches!(
                        app.resolve_visible_spread_pair(0),
                        crate::ui_fullscreen::SpreadPair::Double { .. }
                    ));
                }
                "continuous" => app.reading_flow = crate::settings::ReadingFlow::Vertical,
                _ => {
                    let Some(FsCacheEntry::Static { animation, .. }) = app.fs_cache.get_mut(&0)
                    else {
                        panic!("still fixture");
                    };
                    *animation = crate::fs_animation::StaticAnimationState::FirstFrameOnly(
                        crate::canonical_image_loader::CanonicalAnimatedFormat::Gif,
                    );
                }
            }
            let input = || egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(820.0, 600.0),
                )),
                ..Default::default()
            };
            ctx.run_pass(input(), |ctx| {
                ctx.root_ui(|root_ui| app.render_fullscreen_viewport(root_ui));
            })
            .drop_without_applying_deltas();
            assert!(
                matches!(app.fs_exclusive_view, FsExclusiveView::None),
                "{case}"
            );

            app.fullscreen_idx = Some(3);
            app.spread_mode = crate::settings::SpreadMode::Single;
            app.reading_flow = crate::settings::ReadingFlow::Paged;
            ctx.run_pass(input(), |ctx| {
                ctx.root_ui(|root_ui| app.render_fullscreen_viewport(root_ui));
            })
            .drop_without_applying_deltas();
            assert!(
                child_passes.get() > 0,
                "{case}: the actual child viewer UI must run"
            );
            assert!(
                matches!(app.fs_exclusive_view, FsExclusiveView::None),
                "{case}: still return"
            );
            assert_eq!(app.fs_exclusive_view_active(3), None, "{case}");
        }
        crate::key_input::clear_test_frame();
    })
    .join()
    .unwrap();
}

#[test]
fn depth_i_and_tab_toggle_only_the_depth_panel_through_the_shared_key_handler() {
    let _input_guard = crate::key_input::lock_test_input();
    let ctx = egui::Context::default();
    let mut app = setup_app_for_test();
    install_pages(&mut app, &ctx);
    depth(&mut app);
    let side_panel_mode = app.settings.fullscreen_side_panel_mode;
    for (key, visible) in [(egui::Key::I, false), (egui::Key::Tab, true)] {
        ctx.begin_pass(egui::RawInput {
            events: vec![egui::Event::Key {
                key,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            }],
            ..Default::default()
        });
        let _ = app.handle_fs_key_input(&ctx, 0, false);
        ctx.end_pass().drop_without_applying_deltas();
        let FsExclusiveView::Depth(state) = &app.fs_exclusive_view else {
            panic!("metadata key must keep the depth owner");
        };
        assert_eq!(state.panel_visible, visible, "{key:?}");
        assert_eq!(app.settings.fullscreen_side_panel_mode, side_panel_mode);
        assert!(!app.show_metadata_panel);
    }
    crate::key_input::clear_test_frame();
}

#[cfg(windows)]
#[test]
fn depth_exclusive_owner_survives_sibling_open_close_without_cross_context_reset() {
    let ctx = egui::Context::default();
    let mut app = setup_app_for_test();
    install_pages(&mut app, &ctx);
    let main = app.edit_request_owner_context();
    depth(&mut app);
    let sibling = app.build_window_context_for_test(70_311, |sibling| {
        sibling.items = vec![GridItem::Image("sibling.jpg".into())];
        sibling.fullscreen_idx = Some(0);
        sibling.set_fs_exclusive_view(
            FsExclusiveView::Panorama(crate::panorama::PanoramaState::new(
                0.4,
                0.1,
                crate::panorama::PanoProjection::Stereographic,
            )),
            "depth_test_sibling_panorama",
        );
    });
    app.with_viewer_context(sibling, |sibling| {
        assert_ne!(sibling.edit_request_owner_context(), main);
        assert!(sibling.panorama_state().is_some());
        assert!(!sibling.fs_exclusive_view.is_depth());
        sibling.set_fs_exclusive_view(FsExclusiveView::None, "depth_test_sibling_off");
    })
    .unwrap();
    assert_eq!(app.edit_request_owner_context(), main);
    assert!(app.fs_exclusive_view.is_depth());
    app.with_viewer_context(sibling, |sibling| {
        assert!(matches!(sibling.fs_exclusive_view, FsExclusiveView::None));
    })
    .unwrap();
    assert!(app.depth_view_may_continue(0));
}

#[test]
fn depth_toggle_off_uses_effective_key_and_keeps_unassigned_d_inactive() {
    let _input_guard = crate::key_input::lock_test_input();
    let ctx = egui::Context::default();
    let mut app = setup_app_for_test();
    install_pages(&mut app, &ctx);
    app.keymap = crate::keymap::Keymap::from_ini_str("[FsImage]\nFsDepthParallax = F13\n");
    for (key, stays_on) in [(egui::Key::D, true), (egui::Key::F13, false)] {
        depth(&mut app);
        ctx.begin_pass(egui::RawInput {
            events: vec![egui::Event::Key {
                key,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            }],
            ..Default::default()
        });
        let _ = app.handle_fs_key_input(&ctx, 0, false);
        ctx.end_pass().drop_without_applying_deltas();
        assert_eq!(app.fs_exclusive_view.is_depth(), stays_on, "{key:?}");
    }
    crate::key_input::clear_test_frame();
}

#[test]
fn depth_native_color_readback_and_lru_are_registered_then_retired_from_vram_accounting() {
    let mut app = setup_app_for_test();
    let render = renderer();
    let source = render.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("depth-accounting-source"),
        size: wgpu::Extent3d {
            width: 4,
            height: 4,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    let mut state = DepthViewState::new(app.edit_request_owner_context());
    state.attach_renderer(&render);
    state
        .select_source(
            &render.device,
            &render.queue,
            &source,
            crate::depth_estimation::DepthSourceKey {
                texture_id: egui::TextureId::Managed(88001),
            },
        )
        .unwrap();
    let bytes = state.resident_bytes();
    assert!(bytes > 4 * 4 * 4);
    app.set_fs_exclusive_view(FsExclusiveView::Depth(state), "depth_accounting");
    let recorded = app.vram_accounting_snapshot().subsystems_json()["depth_parallax"]["bytes"]
        .as_u64()
        .unwrap();
    assert_eq!(recorded, bytes.div_ceil(4) * 4);
    app.set_fs_exclusive_view(FsExclusiveView::None, "depth_accounting_off");
    assert_eq!(
        app.vram_accounting_snapshot().subsystems_json()["depth_parallax"]["bytes"],
        0
    );
}

#[test]
fn depth_ring_dispatch_preserves_transforms_and_conflicting_modes() {
    use crate::ring_shortcut::{RingActionId, RingShortcutContext};
    let ctx = egui::Context::default();
    let mut app = setup_app_for_test();
    install_pages(&mut app, &ctx);
    depth(&mut app);
    app.fs_zoom = 2.5;
    app.fs_pan = egui::vec2(11.0, 13.0);
    let modes_before = (
        app.slideshow_playing,
        app.fs_pixel_grid_enabled,
        app.fs_zoom_mode_engaged(),
    );
    for action in [
        RingActionId::ImageSlideshow,
        RingActionId::ImageZoomMode,
        RingActionId::ImagePixelGrid,
        RingActionId::ImageBackgroundCycle,
        RingActionId::ImageComparePin,
        RingActionId::ImageFitModeCycle,
    ] {
        app.apply_ring_action(
            &ctx,
            RingShortcutContext::ImageFullscreen,
            action,
            "depth_ring_test",
        );
        assert_eq!(
            (
                app.slideshow_playing,
                app.fs_pixel_grid_enabled,
                app.fs_zoom_mode_engaged()
            ),
            modes_before
        );
        assert_eq!(app.compare_view_mode, CompareViewMode::Off);
        assert_eq!((app.fs_zoom, app.fs_pan), (2.5, egui::vec2(11.0, 13.0)));
        assert!(app.fs_exclusive_view.is_depth());
    }
    let side_mode = app.settings.fullscreen_side_panel_mode;
    app.apply_ring_action(
        &ctx,
        RingShortcutContext::ImageFullscreen,
        RingActionId::ImageToggleMetadata,
        "depth_ring_panel",
    );
    assert!(
        matches!(&app.fs_exclusive_view, FsExclusiveView::Depth(state) if !state.panel_visible)
    );
    assert_eq!(app.settings.fullscreen_side_panel_mode, side_mode);
}

#[test]
fn depth_new_managed_texture_owns_the_wake_for_backend_registration_without_pointer_input() {
    let ctx = egui::Context::default();
    let mut app = setup_app_for_test();
    install_pages(&mut app, &ctx);
    app.window_visible = true;
    depth(&mut app);
    let render = app.wgpu_render_state.clone().unwrap();
    let mut idle_delay = std::time::Duration::ZERO;
    for _ in 0..8 {
        let output = ctx.run_pass(Default::default(), |_| {});
        idle_delay = output.viewport_output[&egui::ViewportId::ROOT].repaint_delay;
        output.drop_without_applying_deltas();
    }
    assert_ne!(
        idle_delay,
        std::time::Duration::ZERO,
        "fixture must have settled before publication"
    );
    let texture = ctx.load_texture(
        "depth-new-managed",
        egui::ColorImage::filled([2, 3], egui::Color32::WHITE),
        Default::default(),
    );
    assert!(render.renderer.read().texture(&texture.id()).is_none());
    let output = ctx.run_pass(Default::default(), |ctx| {
        app.advance_depth_source(
            ctx,
            crate::depth_parallax::DepthSourceMaterial::Ready {
                texture: texture.clone(),
            },
        );
    });
    assert_eq!(
        output.viewport_output[&egui::ViewportId::ROOT].repaint_delay,
        std::time::Duration::ZERO
    );
    assert!(
        matches!(&app.fs_exclusive_view, FsExclusiveView::Depth(state) if state.wait_reason == crate::depth_parallax::DepthSourceWait::TexturePending)
    );
    for (id, deltas) in &output.textures_delta.set {
        for delta in deltas {
            render
                .renderer
                .write()
                .update_texture(&render.device, &render.queue, *id, delta);
        }
    }
    output.drop_without_applying_deltas();
    let backing = render
        .renderer
        .read()
        .texture(&texture.id())
        .unwrap()
        .texture
        .clone()
        .unwrap();
    let FsExclusiveView::Depth(state) = &mut app.fs_exclusive_view else {
        unreachable!()
    };
    state
        .select_source(
            &render.device,
            &render.queue,
            &backing,
            crate::depth_estimation::DepthSourceKey {
                texture_id: texture.id(),
            },
        )
        .unwrap();
    assert!(matches!(
        state.source,
        crate::depth_parallax::DepthSourcePhase::ReadingBack { .. }
    ));
}

/// Run the real child-viewer UI without installing a native viewport backend.
/// Its managed source stays unregistered until a test explicitly registers it,
/// so source preparation cannot start a readback or a real depth worker.
#[cfg(windows)]
struct DepthWaitViewerFixture {
    ctx: egui::Context,
    minimized: std::rc::Rc<std::cell::Cell<bool>>,
    shapes: std::rc::Rc<std::cell::RefCell<Vec<egui::epaint::ClippedShape>>>,
    passes: std::rc::Rc<std::cell::Cell<usize>>,
}

#[cfg(windows)]
impl DepthWaitViewerFixture {
    fn new(app: &mut App) -> Self {
        let ctx = egui::Context::default();
        ctx.set_embed_viewports(false);
        crate::ui_fullscreen::install_fs_navigator_input_tracking(&ctx);
        let minimized = std::rc::Rc::new(std::cell::Cell::new(false));
        let shapes = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let passes = std::rc::Rc::new(std::cell::Cell::new(0));
        let observed_minimized = minimized.clone();
        let observed_shapes = shapes.clone();
        let observed_passes = passes.clone();
        egui::Context::set_immediate_viewport_renderer(move |ctx, mut viewport| {
            observed_passes.set(observed_passes.get() + 1);
            let output = ctx.run_ui(
                egui::RawInput {
                    viewport_id: viewport.ids.this,
                    screen_rect: Some(Self::rect()),
                    viewports: [
                        (egui::ViewportId::ROOT, egui::ViewportInfo::default()),
                        (
                            viewport.ids.this,
                            egui::ViewportInfo {
                                parent: Some(egui::ViewportId::ROOT),
                                focused: Some(true),
                                minimized: Some(observed_minimized.get()),
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
            observed_shapes.borrow_mut().extend(output.shapes.clone());
            output.drop_without_applying_deltas();
        });
        install_pages(app, &ctx);
        crate::app::source_texture_test_support::seed(app, &ctx, 0);
        isolate_depth_renderer(app);
        app.window_visible = true;
        app.settings.video_in_window_mode = false;
        app.settings.detached_viewer_enabled = false;
        app.native_video_in_window_active = false;
        app.ai_runtime_init =
            crate::ai::runtime::AiRuntimeInitOwner::failed_for_test("no runtime in UI fixture");
        depth(app);
        Self {
            ctx,
            minimized,
            shapes,
            passes,
        }
    }

    fn rect() -> egui::Rect {
        egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1093.0, 614.0))
    }

    fn frame(&self, app: &mut App) {
        self.shapes.borrow_mut().clear();
        let previous_passes = self.passes.get();
        app.frame_counter += 1;
        self.ctx
            .run_pass(
                egui::RawInput {
                    screen_rect: Some(Self::rect()),
                    ..Default::default()
                },
                |ctx| ctx.root_ui(|ui| app.render_fullscreen_viewport(ui)),
            )
            .drop_without_applying_deltas();
        assert!(
            self.passes.get() > previous_passes,
            "the actual child-viewer pass must run"
        );
    }

    fn text_rects(&self, expected: &str) -> Vec<egui::Rect> {
        fn collect(shape: &egui::Shape, expected: &str, result: &mut Vec<egui::Rect>) {
            match shape {
                egui::Shape::Text(text) if text.galley.text() == expected => {
                    result.push(egui::Rect::from_min_size(text.pos, text.galley.size()));
                }
                egui::Shape::Vec(shapes) => {
                    for shape in shapes {
                        collect(shape, expected, result);
                    }
                }
                _ => {}
            }
        }
        let mut result = Vec::new();
        for shape in self.shapes.borrow().iter() {
            collect(&shape.shape, expected, &mut result);
        }
        result
    }

    fn assert_wait_box(&self, caption: &str) -> egui::Rect {
        fn collect(shape: &egui::Shape, target: egui::Rect, result: &mut Vec<egui::Rect>) {
            match shape {
                egui::Shape::Rect(rect) if rect.rect.contains_rect(target) => {
                    result.push(rect.rect);
                }
                egui::Shape::Vec(shapes) => {
                    for shape in shapes {
                        collect(shape, target, result);
                    }
                }
                _ => {}
            }
        }
        let caption_rects = self.text_rects(caption);
        let caption_rect = *caption_rects.first().expect("wait cancellation caption");
        let mut boxes = Vec::new();
        for shape in self.shapes.borrow().iter() {
            collect(&shape.shape, caption_rect, &mut boxes);
        }
        boxes
            .into_iter()
            .find(|rect| {
                Self::rect().contains_rect(*rect)
                    && rect.width() < Self::rect().width() * 0.75
                    && rect.height() < Self::rect().height() * 0.5
            })
            .expect("the actual painter must emit a bounded box around the wait caption")
    }

    fn source_key_and_register_backing(
        &self,
        app: &mut App,
    ) -> crate::depth_estimation::DepthSourceKey {
        let source = app
            .ensure_final_composite_material(&self.ctx, 0)
            .expect("canonical final source");
        let tex = source.source_texture();
        let Some(FsCacheEntry::Static { pixels, .. }) = app.fs_cache.get(&0) else {
            panic!("still fixture");
        };
        let render = app.wgpu_render_state.as_ref().unwrap();
        render.renderer.write().update_texture(
            &render.device,
            &render.queue,
            tex.id(),
            &egui::epaint::ImageDelta::full((**pixels).clone(), Default::default()),
        );
        crate::depth_estimation::DepthSourceKey {
            texture_id: tex.id(),
        }
    }
}

#[cfg(windows)]
fn set_depth_wait_age(app: &mut App, age: std::time::Duration) {
    let FsExclusiveView::Depth(state) = &mut app.fs_exclusive_view else {
        panic!("depth fixture");
    };
    state.wait_started_at = std::time::Instant::now() - age;
}

#[cfg(windows)]
#[test]
fn depth_wait_actual_viewer_box_obeys_delay_and_panel_visibility() {
    std::thread::spawn(|| {
        let _input_guard = crate::key_input::lock_test_input();
        let mut app = setup_app_for_test();
        let viewer = DepthWaitViewerFixture::new(&mut app);
        for panel_visible in [true, false] {
            let FsExclusiveView::Depth(state) = &mut app.fs_exclusive_view else {
                panic!("depth fixture");
            };
            state.panel_visible = panel_visible;
            // A future fixture clock keeps slow CI/GPU setup from crossing the gate.
            state.wait_started_at = std::time::Instant::now() + std::time::Duration::from_secs(60);
            viewer.frame(&mut app);
            assert!(viewer.text_rects("D で取り消し").is_empty());
            set_depth_wait_age(&mut app, std::time::Duration::from_millis(151));
            viewer.frame(&mut app);
            let rect = viewer.assert_wait_box("D で取り消し");
            assert!(
                rect.top() >= 160.0
                    && rect.bottom() <= DepthWaitViewerFixture::rect().bottom() - 48.0
            );
            if panel_visible {
                let panel = viewer
                    .ctx
                    .data(|data| data.get_temp::<egui::Rect>(egui::Id::unique("depth_panel_rect")))
                    .unwrap();
                assert!(
                    !rect.intersects(panel),
                    "wait box must avoid the current visible panel"
                );
            }
            assert!(app.fs_exclusive_view.is_depth());
        }
        crate::key_input::clear_test_frame();
    })
    .join()
    .unwrap();
}

#[cfg(windows)]
#[test]
fn depth_wait_actual_viewer_caption_uses_effective_binding_and_hides_while_not_visible() {
    std::thread::spawn(|| {
        let _input_guard = crate::key_input::lock_test_input();
        let mut app = setup_app_for_test();
        let viewer = DepthWaitViewerFixture::new(&mut app);
        let FsExclusiveView::Depth(state) = &mut app.fs_exclusive_view else {
            panic!("depth fixture");
        };
        state.panel_visible = false;
        set_depth_wait_age(&mut app, std::time::Duration::from_secs(1));
        app.keymap = crate::keymap::Keymap::from_ini_str("[FsImage]\nFsDepthParallax = F13\n");
        viewer.frame(&mut app);
        viewer.assert_wait_box("F13 で取り消し");
        assert!(viewer.text_rects("D で取り消し").is_empty());
        let status = app.depth_wait_status().unwrap().overlay_text();
        assert!(!viewer.text_rects(status).is_empty());

        app.keymap = crate::keymap::Keymap::from_ini_str("[FsImage]\nFsDepthParallax = none\n");
        viewer.frame(&mut app);
        assert!(!viewer.text_rects(status).is_empty());
        assert!(viewer.text_rects("D で取り消し").is_empty());
        assert!(viewer.text_rects("F13 で取り消し").is_empty());
        app.window_visible = false;
        viewer.frame(&mut app);
        assert!(viewer.text_rects(status).is_empty());
        app.window_visible = true;
        viewer.minimized.set(true);
        viewer.frame(&mut app);
        assert!(viewer.text_rects(status).is_empty());
        viewer.minimized.set(false);
        viewer.frame(&mut app);
        assert!(!viewer.text_rects(status).is_empty());
        crate::key_input::clear_test_frame();
    })
    .join()
    .unwrap();
}

#[cfg(windows)]
#[test]
fn depth_wait_actual_viewer_box_disappears_for_ready_failed_and_off() {
    std::thread::spawn(|| {
        let _input_guard = crate::key_input::lock_test_input();
        for terminal in ["ready", "failed", "off"] {
            let mut app = setup_app_for_test();
            let viewer = DepthWaitViewerFixture::new(&mut app);
            set_depth_wait_age(&mut app, std::time::Duration::from_secs(1));
            viewer.frame(&mut app);
            viewer.assert_wait_box("D で取り消し");
            if terminal == "off" {
                app.set_fs_exclusive_view(FsExclusiveView::None, "depth_wait_test_off");
            } else {
                let key = viewer.source_key_and_register_backing(&mut app);
                let render = app.wgpu_render_state.clone().unwrap();
                let FsExclusiveView::Depth(state) = &mut app.fs_exclusive_view else {
                    panic!("depth fixture");
                };
                state.source = if terminal == "ready" {
                    let backing = render.renderer.read().texture(&key.texture_id).unwrap().texture.clone().unwrap();
                    state.select_source(&render.device, &render.queue, &backing, key).unwrap();
                    let size = backing.size();
                    let map = crate::depth_estimation::DepthMap { key, width: size.width, height: size.height,
                        values: vec![half::f16::from_f32(0.5); (size.width * size.height) as usize], median: 0.5 };
                    crate::depth_parallax::DepthSourcePhase::Estimating { key,
                        pending: crate::depth_estimation::DepthPending::completed_for_test(state.context, 80, map) }
                } else {
                    crate::depth_parallax::DepthSourcePhase::Failed {
                        key,
                        reason: "fixture failure".to_owned(),
                    }
                };
            }
            viewer.frame(&mut app);
            assert!(viewer.text_rects("D で取り消し").is_empty(), "{terminal}");
            assert!(app.depth_wait_status().is_none(), "{terminal}");
            if terminal == "failed" {
                assert!(app.depth_panel_status().contains("fixture failure"));
                assert!(matches!(
                    &app.fs_exclusive_view,
                    FsExclusiveView::Depth(state)
                        if matches!(state.source, crate::depth_parallax::DepthSourcePhase::Failed { .. })
                ));
            } else if terminal == "ready" {
                assert!(matches!(&app.fs_exclusive_view, FsExclusiveView::Depth(state) if state.ready().is_some()),
                    "the real viewer advance must adopt and upload a depth result in this pass");
                assert!(matches!(
                    &app.fs_exclusive_view,
                    FsExclusiveView::Depth(state)
                        if matches!(state.source, crate::depth_parallax::DepthSourcePhase::Ready { .. })
                ));
            }
        }
        crate::key_input::clear_test_frame();
    })
    .join()
    .unwrap();
}

#[cfg(windows)]
#[test]
fn depth_actual_mouse_page_navigation_ignores_suspended_2d_transforms() {
    // The immediate renderer is thread-local: drive the real child viewer without
    // opening a native window or leaving a backend callback on a test-pool thread.
    std::thread::spawn(|| {
        let _input_guard = crate::key_input::lock_test_input();
        let mut destinations = Vec::new();
        for (committed_zoom, panel_visible) in
            [(false, true), (true, true), (false, false), (true, false)]
        {
            let ctx = egui::Context::default();
            ctx.set_embed_viewports(false);
            crate::ui_fullscreen::install_fs_navigator_input_tracking(&ctx);
            let events = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
            let child_events = events.clone();
            let responses = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
            let observed_responses = responses.clone();
            let child_passes = std::rc::Rc::new(std::cell::Cell::new(0));
            let observed = child_passes.clone();
            let viewport_rect =
                egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1920.0, 1080.0));
            egui::Context::set_immediate_viewport_renderer(move |ctx, mut viewport| {
                observed.set(observed.get() + 1);
                let input_events = std::mem::take(&mut *child_events.borrow_mut());
                ctx.run_ui(
                    egui::RawInput {
                        viewport_id: viewport.ids.this,
                        screen_rect: Some(viewport_rect),
                        time: Some(observed.get() as f64 / 60.0),
                        events: input_events,
                        viewports: [
                            (
                                egui::ViewportId::ROOT,
                                egui::ViewportInfo {
                                    focused: Some(false),
                                    ..Default::default()
                                },
                            ),
                            (
                                viewport.ids.this,
                                egui::ViewportInfo {
                                    parent: Some(egui::ViewportId::ROOT),
                                    focused: Some(true),
                                    inner_rect: Some(viewport_rect),
                                    native_pixels_per_point: Some(1.0),
                                    ..Default::default()
                                },
                            ),
                        ]
                        .into_iter()
                        .collect(),
                        ..Default::default()
                    },
                    |ui| {
                        (viewport.viewport_ui_cb)(ui);
                        if let Some(response) = ctx.read_response(egui::Id::unique("fs_click")) {
                            observed_responses.borrow_mut().push(format!(
                                "pos={:?} clicked={} down={} hovered={} layer={:?}",
                                response.interact_pointer_pos(), response.clicked(),
                                response.is_pointer_button_down_on(), response.hovered(),
                                ctx.input(|i| i.pointer.hover_pos()).and_then(|pos| ctx.layer_id_at(pos))
                            ));
                        }
                    },
                )
                .drop_without_applying_deltas();
            });
            let mut app = setup_app_for_test();
            install_pages(&mut app, &ctx);
            app.items[1] = GridItem::Image("depth-mouse-next.jpg".into());
            install_still(&mut app, &ctx, 1, [2, 3]);
            app.window_visible = false; // No depth worker/model during this input test.
            app.settings.video_in_window_mode = false;
            app.settings.detached_viewer_enabled = false;
            app.settings.touch_still_chrome_learned = true;
            let model_path = app.tmp.path().join("unused-depth-model.onnx");
            app.depth_setup
                .set_status_for_test(crate::addon::AddonStatus::Valid {
                    model_id: "depth-mouse-fixture".to_owned(),
                    bytes: 1,
                    model_path,
                    mmproj_path: None,
                });
            app.fs_zoom_factor = 2.75;
            app.fs_zoom = 1.8;
            app.fs_pan = egui::vec2(24.0, -18.0);
            app.fs_free_rotation = 0.2;
            if committed_zoom {
                depth_key_frame(&mut app, &ctx, &[(egui::Key::Z, true)]);
                assert!(app.fs_zoom_aiming);
                depth_key_frame(&mut app, &ctx, &[(egui::Key::Z, false)]);
                assert!(app.fs_zoom_active);
                assert!(!app.fs_zoom_aiming);
            }
            depth_key_frame(
                &mut app,
                &ctx,
                &[(egui::Key::D, true), (egui::Key::D, false)],
            );
            if !panel_visible {
                depth_key_frame(
                    &mut app,
                    &ctx,
                    &[(egui::Key::I, true), (egui::Key::I, false)],
                );
            }
            assert!(matches!(
                &app.fs_exclusive_view,
                FsExclusiveView::Depth(state) if state.panel_visible == panel_visible
            ));
            let render_frame = |app: &mut App| {
                app.frame_counter += 1;
                ctx.run_pass(
                    egui::RawInput {
                        screen_rect: Some(viewport_rect),
                        ..Default::default()
                    },
                    |ctx| ctx.root_ui(|root_ui| app.render_fullscreen_viewport(root_ui)),
                )
                .drop_without_applying_deltas();
            };
            render_frame(&mut app);
            assert_eq!(app.fullscreen_idx, Some(0));
            // Model a settled, already focused viewer, as in the reported clicks.
            // Its initial focus-acquisition click is intentionally not navigation.
            app.fs_focus_regained_at = None;
            assert!(!app.fs_primary_suppression.is_active());
            // Real mouse producer, distinct from the touch regression: right
            // page-side click advances, then left page-side click returns.
            for (pos, expected) in [
                (egui::pos2(1100.0, 500.0), 1),
                (egui::pos2(250.0, 500.0), 0),
            ] {
                events.borrow_mut().push(egui::Event::PointerMoved(pos));
                render_frame(&mut app);
                for pressed in [true, false] {
                    events.borrow_mut().extend([
                        egui::Event::PointerMoved(pos),
                        egui::Event::PointerButton {
                            pos,
                            button: egui::PointerButton::Primary,
                            pressed,
                            modifiers: egui::Modifiers::NONE,
                        },
                    ]);
                    render_frame(&mut app);
                    assert!(
                        events.borrow().is_empty(),
                        "child viewer must consume mouse batch"
                    );
                    assert!(app.fs_exclusive_view.is_depth());
                    assert_eq!(app.fs_zoom_active, committed_zoom);
                    assert_eq!(app.fs_zoom_factor, 2.75);
                }
                assert_eq!(app.fullscreen_idx, Some(expected), "mouse page-side click: Z={committed_zoom}, panel={panel_visible}, pos={pos:?}, responses={:?}", responses.borrow());
            }
            assert!(
                child_passes.get() >= 3,
                "the actual child viewer UI must run"
            );
            assert_eq!(
                app.fullscreen_idx,
                Some(0),
                "raw mouse must reach PageSide navigation with committed_zoom={committed_zoom}"
            );
            assert!(app.fs_exclusive_view.is_depth());
            assert_eq!(
                app.fs_zoom_owner(),
                crate::ui_fullscreen::FsZoomOwner::Depth
            );
            destinations.push(app.items[app.fullscreen_idx.unwrap()].clone());
            depth_key_frame(
                &mut app,
                &ctx,
                &[(egui::Key::D, true), (egui::Key::D, false)],
            );
            assert!(matches!(app.fs_exclusive_view, FsExclusiveView::None));
            assert_eq!(app.fs_zoom_active, committed_zoom);
            assert!(!app.fs_zoom_aiming);
            assert_eq!(app.fs_zoom_factor, 2.75);
            assert_eq!(
                app.fs_zoom_owner(),
                if committed_zoom {
                    crate::ui_fullscreen::FsZoomOwner::ZoomMode
                } else {
                    crate::ui_fullscreen::FsZoomOwner::Image
                }
            );
            crate::key_input::clear_test_frame();
        }
        assert!(
            destinations
                .iter()
                .all(|destination| destination == &destinations[0])
        );
    })
    .join()
    .unwrap();
}
