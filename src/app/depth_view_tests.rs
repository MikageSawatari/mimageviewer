//! Context/state/handler regressions. No application window or real model is opened.
use super::*;
use crate::depth_parallax::{DepthEntryBlock, DepthViewState};
use crate::fs_exclusive_view::{ExclusiveViewKind, FsExclusiveView};

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

fn install_pages(app: &mut App, ctx: &egui::Context) {
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
