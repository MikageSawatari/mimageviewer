//! Real ROOT embedded viewer passes, with generated pixels and no native window/model.
use super::*;
use crate::depth_parallax::{DepthViewState, MotionKind};
use crate::fs_exclusive_view::FsExclusiveView;

pub(crate) struct RootDepthViewer {
    pub(crate) ctx: egui::Context,
    pub(crate) size: egui::Vec2,
    pub(crate) shapes: Vec<egui::epaint::ClippedShape>,
    minimized: bool,
    depth_painted: bool,
}

impl RootDepthViewer {
    pub(crate) fn new(app: &mut App) -> Self {
        let ctx = egui::Context::default();
        crate::ui_fonts::configure_fonts(&ctx);
        crate::ui_fullscreen::install_fs_navigator_input_tracking(&ctx);
        super::depth_view_tests::install_pages(app, &ctx);
        crate::app::source_texture_test_support::seed(app, &ctx, 0);
        super::depth_view_tests::install_ready_depth_for_export(app);
        app.set_fs_exclusive_view(
            FsExclusiveView::Depth(DepthViewState::new(app.edit_request_owner_context())),
            "root_depth_fixture",
        );
        app.viewer_presentation = ViewerPresentation::MainWindow;
        app.native_video_in_window_active = true;
        app.settings.video_in_window_mode = true;
        app.settings.detached_viewer_enabled = false;
        app.settings.page_turn_animation_enabled = false;
        app.ai_runtime_init =
            crate::ai::runtime::AiRuntimeInitOwner::failed_for_test("headless ROOT fixture");
        app.window_visible = true;
        app.settings.depth_parallax.motion = MotionKind::Mouse;
        assert!(app.depth_entry_allowed(0).is_ok());
        assert!(app.fullscreen_embedded_still_active());
        Self {
            ctx,
            size: egui::vec2(1093.0, 614.0),
            shapes: vec![],
            minimized: false,
            depth_painted: false,
        }
    }

    pub(crate) fn ready(&self, app: &mut App) -> u64 {
        let source = app.ensure_final_composite_material(&self.ctx, 0).unwrap();
        let key = crate::depth_estimation::DepthSourceKey {
            texture_id: source.source_texture().id(),
        };
        let Some(FsCacheEntry::Static { pixels, .. }) = app.fs_cache.get(&0) else {
            panic!("generated still");
        };
        let render = app.wgpu_render_state.clone().unwrap();
        render.renderer.write().update_texture(
            &render.device,
            &render.queue,
            key.texture_id,
            &egui::epaint::ImageDelta::full((**pixels).clone(), Default::default()),
        );
        let backing = render
            .renderer
            .read()
            .texture(&key.texture_id)
            .unwrap()
            .texture
            .clone()
            .unwrap();
        let FsExclusiveView::Depth(state) = &mut app.fs_exclusive_view else {
            panic!("Depth owner");
        };
        state.attach_renderer(&render);
        let map = crate::depth_estimation::DepthMap {
            key,
            width: 4,
            height: 6,
            values: vec![half::f16::from_f32(0.5); 24],
            median: 0.5,
        };
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
            .select_source(&render.device, &render.queue, &backing, key)
            .unwrap();
        state.ready().unwrap().resource_id
    }

    pub(crate) fn frame(&mut self, app: &mut App, events: Vec<egui::Event>) {
        self.frame_with_before(app, events, |_, _| {});
    }

    pub(super) fn frame_with_before(
        &mut self,
        app: &mut App,
        events: Vec<egui::Event>,
        mut before: impl FnMut(&mut App, &egui::Context),
    ) {
        app.frame_counter += 1;
        let mut depth_painted = false;
        let output = self.ctx.run_pass(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, self.size)),
                time: Some(app.frame_counter as f64 / 60.0),
                events,
                viewports: [(
                    egui::ViewportId::ROOT,
                    egui::ViewportInfo {
                        focused: Some(true),
                        minimized: Some(self.minimized),
                        ..Default::default()
                    },
                )]
                .into_iter()
                .collect(),
                ..Default::default()
            },
            |ctx| {
                assert_eq!(ctx.viewport_id(), egui::ViewportId::ROOT);
                before(app, ctx);
                ctx.root_ui(|ui| app.render_fullscreen_viewport_with_root_input(ui, None));
                // Ordinary 2D resampling also emits type-erased WGPU callbacks.
                // Only successful try_paint_depth publishes this same-pass receipt.
                depth_painted = app.capture_depth_export(ctx, true).is_some();
            },
        );
        self.shapes = output.shapes.clone();
        self.depth_painted = depth_painted;
        output.drop_without_applying_deltas();
    }

    pub(super) fn click(&mut self, app: &mut App, pos: egui::Pos2, batched: bool) {
        if batched {
            let mut events = vec![egui::Event::PointerMoved(pos)];
            events.extend([true, false].map(|pressed| pointer(pos, pressed)));
            self.frame(app, events);
        } else {
            self.frame(app, vec![egui::Event::PointerMoved(pos)]);
            for pressed in [true, false] {
                self.frame(app, vec![pointer(pos, pressed)]);
            }
        }
    }

    pub(super) fn text_rects(&self, label: &str) -> Vec<egui::Rect> {
        fn collect(
            shape: &egui::Shape,
            clip: egui::Rect,
            label: &str,
            result: &mut Vec<egui::Rect>,
        ) {
            match shape {
                egui::Shape::Text(text) if text.galley.text() == label => {
                    let rect = egui::Rect::from_min_size(text.pos, text.galley.size());
                    if clip.contains_rect(rect) {
                        result.push(rect);
                    }
                }
                egui::Shape::Vec(shapes) => {
                    for shape in shapes {
                        collect(shape, clip, label, result);
                    }
                }
                _ => {}
            }
        }
        let mut result = vec![];
        for shape in &self.shapes {
            collect(&shape.shape, shape.clip_rect, label, &mut result);
        }
        result
    }

    pub(super) fn label(&self, label: &str) -> egui::Rect {
        let rects = self.text_rects(label);
        assert_eq!(rects.len(), 1, "one visible label {label}: {rects:?}");
        let rect = rects[0];
        assert!(
            self.ctx.content_rect().contains_rect(rect),
            "{label}: {rect:?}"
        );
        rect
    }

    pub(super) fn panel(&self) -> egui::Rect {
        self.ctx
            .data(|d| d.get_temp(egui::Id::unique("depth_panel_rect")))
            .unwrap()
    }

    pub(crate) fn callbacks(&self) -> Vec<egui::Rect> {
        if !self.depth_painted {
            return vec![];
        }
        self.shapes
            .iter()
            .filter_map(|shape| match &shape.shape {
                egui::Shape::Callback(callback) => Some(callback.rect),
                _ => None,
            })
            .collect()
    }
}

pub(super) fn pointer(pos: egui::Pos2, pressed: bool) -> egui::Event {
    egui::Event::PointerButton {
        pos,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: egui::Modifiers::NONE,
    }
}

pub(super) fn wheel(pos: egui::Pos2) -> Vec<egui::Event> {
    vec![
        egui::Event::PointerMoved(pos),
        egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            delta: egui::vec2(0.0, -1500.0),
            modifiers: egui::Modifiers::NONE,
            phase: egui::TouchPhase::Move,
            source: egui::MouseWheelSource::Wheel,
        },
    ]
}

#[test]
fn depth_main_window_root_paints_fit_rotation_resize_underlay_without_mutating_2d_transform() {
    let _input = crate::key_input::lock_test_input();
    let mut app = setup_app_for_test();
    let mut viewer = RootDepthViewer::new(&mut app);
    let resource_id = viewer.ready(&mut app);
    // The real Area needs its initial sizing pass before control geometry exists.
    viewer.frame(&mut app, vec![]);
    viewer.frame(&mut app, vec![]);
    app.fs_zoom = 2.4;
    app.fs_pan = egui::vec2(55.0, -30.0);
    app.fs_zoom_active = true;
    app.fs_zoom_factor = 3.2;
    for size in [egui::vec2(1093.0, 614.0), egui::vec2(820.0, 600.0)] {
        viewer.size = size;
        for rotation in [
            crate::rotation_db::Rotation::None,
            crate::rotation_db::Rotation::Cw90,
        ] {
            app.rotation_cache.insert(0, rotation);
            for mode in [0, 1, 2] {
                app.fs_transparent_bg_mode = mode;
                viewer.frame(&mut app, vec![]);
                let transform = app.fullscreen_page_layout.single_page().unwrap().transform;
                assert_eq!(viewer.callbacks(), vec![transform.paint_rect]);
                assert_eq!(transform.rotation, rotation);
                let layout_size = if rotation == crate::rotation_db::Rotation::Cw90 {
                    egui::vec2(600.0, 400.0)
                } else {
                    egui::vec2(400.0, 600.0)
                };
                let screen = viewer.ctx.content_rect();
                let fit = (screen.width() / layout_size.x).min(screen.height() / layout_size.y);
                let expected = egui::Rect::from_center_size(screen.center(), layout_size * fit);
                let tolerance = 1.0 / viewer.ctx.pixels_per_point();
                assert!(
                    (transform.paint_rect.min - expected.min).abs().max_elem() <= tolerance
                        && (transform.paint_rect.max - expected.max).abs().max_elem() <= tolerance,
                    "Page fit: expected={expected:?}, actual={:?}",
                    transform.paint_rect
                );
                assert!(
                    viewer
                        .ctx
                        .content_rect()
                        .contains_rect(transform.paint_rect)
                );
                assert_eq!(
                    (
                        app.fs_zoom,
                        app.fs_pan,
                        app.fs_zoom_active,
                        app.fs_zoom_factor
                    ),
                    (2.4, egui::vec2(55.0, -30.0), true, 3.2)
                );
                let quad = transform.paint_quad();
                if mode == 2 {
                    let checker = app.fs_checker_texture.as_ref().unwrap().id();
                    assert!(viewer.shapes.iter().any(|shape| matches!(&shape.shape,
                        egui::Shape::Mesh(mesh) if mesh.texture_id == checker
                            && mesh.vertices.iter().map(|v| v.pos).collect::<Vec<_>>() == quad.positions)));
                } else {
                    let color = if mode == 1 {
                        egui::Color32::WHITE
                    } else {
                        egui::Color32::BLACK
                    };
                    assert!(viewer.shapes.iter().any(|shape| matches!(&shape.shape,
                        egui::Shape::Path(path) if path.fill == color && path.points == quad.positions)));
                }
                let FsExclusiveView::Depth(state) = &app.fs_exclusive_view else {
                    panic!("ROOT retired Depth");
                };
                assert_eq!(state.ready().unwrap().resource_id, resource_id);
                assert!(viewer.ctx.content_rect().contains_rect(viewer.panel()));
                viewer.label("×");
                viewer.label("書き出し…");
            }
        }
    }
    // Adopt a normal reload through the existing upload owner. Merely replacing
    // fs_cache is not a source adoption: the immutable edit material still owns
    // that item until the normal source boundary advances its generation.
    let old_texture = app
        .ensure_final_composite_material(&viewer.ctx, 0)
        .unwrap()
        .source_texture()
        .id();
    let old_key = app.current_edit_result_key(0);
    let old_final = app.final_composite_key_for_pixels(old_key, [4, 6], app.effective_params(0));
    app.fs_cache.remove(&0);
    let generation = app.items_generation;
    app.fs_upload_backlog.push_for_generation(
        generation,
        FsUploadResult::new(
            0,
            FsLoadResult::Static {
                ci: egui::ColorImage::filled([4, 6], egui::Color32::BLUE),
                source_dims: [1200, 1800],
                animation: crate::fs_animation::StaticAnimationState::Still,
            },
            9,
            FsLoadPurpose::Display,
        ),
    );
    app.poll_prefetch(&viewer.ctx, PollPrefetchOrigin::TopLevel);
    assert_ne!(
        app.current_edit_result_key(0).source_gen,
        old_key.source_gen
    );
    assert!(!app.edit_result_cache.contains_key(&old_key));
    assert!(!app.final_composite_cache.contains_key(&old_final));
    let replacement = app.ensure_final_composite_material(&viewer.ctx, 0).unwrap();
    assert_ne!(replacement.source_texture().id(), old_texture);
    viewer.frame(&mut app, vec![]);
    assert!(viewer.callbacks().is_empty());
    assert!(app.fs_exclusive_view.is_depth());
    crate::key_input::clear_test_frame();
}

#[test]
fn depth_main_window_root_wait_and_mouse_use_current_panel_and_resized_fit_coordinates() {
    let _input = crate::key_input::lock_test_input();
    let mut app = setup_app_for_test();
    let mut viewer = RootDepthViewer::new(&mut app);
    let FsExclusiveView::Depth(state) = &mut app.fs_exclusive_view else {
        unreachable!()
    };
    state.wait_started_at = std::time::Instant::now() + std::time::Duration::from_secs(60);
    viewer.frame(&mut app, vec![]);
    assert!(viewer.text_rects("D で取り消し").is_empty());
    let FsExclusiveView::Depth(state) = &mut app.fs_exclusive_view else {
        unreachable!()
    };
    state.wait_started_at = std::time::Instant::now() - std::time::Duration::from_secs(1);
    viewer.frame(&mut app, vec![]);
    let caption = viewer.label("D で取り消し");
    assert!(!caption.intersects(viewer.panel()));
    assert!(caption.top() >= 160.0 && caption.bottom() <= 614.0 - 48.0);
    viewer.ready(&mut app);
    for size in [egui::vec2(1093.0, 614.0), egui::vec2(820.0, 600.0)] {
        viewer.size = size;
        viewer.frame(&mut app, vec![]);
        assert!(viewer.text_rects("D で取り消し").is_empty());
        let image = app
            .fullscreen_page_layout
            .single_page()
            .unwrap()
            .transform
            .paint_rect;
        let media = app
            .fullscreen_page_layout
            .single_page()
            .unwrap()
            .transform
            .viewport_rect;
        let radius = media.width().min(media.height()) * 0.25;
        let point = image.center() - egui::vec2(radius * 0.5, radius * 0.5);
        viewer.frame(&mut app, vec![egui::Event::PointerMoved(point)]);
        let FsExclusiveView::Depth(state) = &app.fs_exclusive_view else {
            unreachable!()
        };
        let target = state.mouse.target;
        assert!(
            (target - egui::vec2(0.5, 0.3) * app.settings.depth_parallax.strength).length() < 1e-5
        );
        viewer.frame(
            &mut app,
            vec![egui::Event::PointerMoved(viewer.panel().center())],
        );
        let FsExclusiveView::Depth(state) = &app.fs_exclusive_view else {
            unreachable!()
        };
        assert_eq!(
            state.mouse.target, target,
            "hover over ROOT panel freezes pointer target"
        );
        viewer.frame(&mut app, vec![egui::Event::PointerGone]);
        let FsExclusiveView::Depth(state) = &app.fs_exclusive_view else {
            unreachable!()
        };
        assert_eq!(state.mouse.target, egui::Vec2::ZERO);
    }
    crate::key_input::clear_test_frame();
}

#[test]
fn depth_main_window_root_pass_rejects_ineligible_state_before_any_depth_callback() {
    let _input = crate::key_input::lock_test_input();
    for case in ["animation", "spread", "continuous", "editing"] {
        let mut app = setup_app_for_test();
        let mut viewer = RootDepthViewer::new(&mut app);
        viewer.ready(&mut app);
        viewer.frame(&mut app, vec![]);
        assert_eq!(viewer.callbacks().len(), 1, "positive control");
        match case {
            "animation" => {
                let Some(FsCacheEntry::Static { animation, .. }) = app.fs_cache.get_mut(&0) else {
                    unreachable!()
                };
                *animation = crate::fs_animation::StaticAnimationState::FirstFrameOnly(
                    crate::canonical_image_loader::CanonicalAnimatedFormat::Gif,
                );
            }
            "spread" => app.spread_mode = crate::settings::SpreadMode::Ltr,
            "continuous" => app.reading_flow = crate::settings::ReadingFlow::Vertical,
            _ => app.export_crop_mode = true,
        }
        // The fixture has a portrait still at 2 for a real two-page spread.
        if case == "spread" {
            app.items[1] = GridItem::Image("root-spread.jpg".into());
            crate::app::source_texture_test_support::seed(&mut app, &viewer.ctx, 1);
        }
        viewer.frame(&mut app, vec![]);
        assert!(viewer.callbacks().is_empty(), "{case}");
        assert!(!app.fs_exclusive_view.is_depth(), "{case}");
    }
    crate::key_input::clear_test_frame();
}

#[test]
fn depth_main_window_root_pending_polling_pauses_hidden_and_minimized_then_resumes() {
    use crate::depth_parallax::DepthSourcePhase;
    let _input = crate::key_input::lock_test_input();
    for estimating in [false, true] {
        let mut app = setup_app_for_test();
        let mut viewer = RootDepthViewer::new(&mut app);
        viewer.ready(&mut app);
        let resource = app.ensure_final_composite_material(&viewer.ctx, 0).unwrap();
        let key = crate::depth_estimation::DepthSourceKey {
            texture_id: resource.source_texture().id(),
        };
        let render = app.wgpu_render_state.clone().unwrap();
        let backing = render
            .renderer
            .read()
            .texture(&key.texture_id)
            .unwrap()
            .texture
            .clone()
            .unwrap();
        let mut state = DepthViewState::new(app.edit_request_owner_context());
        state.attach_renderer(&render);
        state
            .select_source(&render.device, &render.queue, &backing, key)
            .unwrap();
        assert!(matches!(state.source, DepthSourcePhase::ReadingBack { .. }));
        if estimating {
            state.source = DepthSourcePhase::Estimating {
                key,
                pending: crate::depth_estimation::DepthPending::completed_for_test(
                    state.context,
                    72_301,
                    crate::depth_estimation::DepthMap {
                        key,
                        width: 4,
                        height: 6,
                        values: vec![half::f16::from_f32(0.5); 24],
                        median: 0.5,
                    },
                ),
            };
        }
        app.set_fs_exclusive_view(FsExclusiveView::Depth(state), "root_pending_visibility");
        render
            .device
            .poll(wgpu::PollType::Wait {
                submission_index: None,
                timeout: None,
            })
            .unwrap();
        for hidden in [true, false] {
            app.window_visible = !hidden;
            viewer.minimized = !hidden;
            for _ in 0..2 {
                viewer.frame(&mut app, vec![]);
                let FsExclusiveView::Depth(state) = &app.fs_exclusive_view else {
                    panic!("visibility must retain owner");
                };
                assert_eq!(
                    matches!(state.source, DepthSourcePhase::Estimating { .. }),
                    estimating
                );
                assert_eq!(
                    matches!(state.source, DepthSourcePhase::ReadingBack { .. }),
                    !estimating
                );
                assert_eq!(state.source.key(), Some(key));
                assert!(state.ready().is_none());
                assert!(viewer.callbacks().is_empty());
            }
        }
        viewer.minimized = false;
        viewer.frame(&mut app, vec![]);
        let FsExclusiveView::Depth(state) = &app.fs_exclusive_view else {
            panic!("visible pending retains owner");
        };
        if estimating {
            assert!(matches!(state.source, DepthSourcePhase::Ready { .. }));
            assert_eq!(
                viewer.callbacks().len(),
                1,
                "completed estimation adopted on restored ROOT pass"
            );
        } else {
            assert!(
                matches!(
                    state.source,
                    DepthSourcePhase::Estimating { .. } | DepthSourcePhase::Failed { .. }
                ),
                "completed readback resumes; unavailable fixture runtime replies asynchronously"
            );
        }
    }
    crate::key_input::clear_test_frame();
}

#[test]
fn depth_main_window_root_o_is_blocked_and_escape_closes_the_viewer() {
    let _input = crate::key_input::lock_test_input();
    let mut app = setup_app_for_test();
    let mut viewer = RootDepthViewer::new(&mut app);
    viewer.ready(&mut app);
    viewer.frame(&mut app, vec![]);
    let events = |key| {
        [true, false]
            .map(|pressed| egui::Event::Key {
                key,
                physical_key: Some(key),
                pressed,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            })
            .to_vec()
    };
    viewer.frame(&mut app, events(egui::Key::O));
    assert!(matches!(
        app.page_overview,
        crate::page_overview::PageOverviewState::Closed
    ));
    assert!(app.fs_exclusive_view.is_depth());
    assert_eq!(app.fullscreen_idx, Some(0));
    viewer.frame(&mut app, events(egui::Key::Escape));
    assert_eq!(app.fullscreen_idx, None);
    assert!(!app.fs_exclusive_view.is_depth());
    crate::key_input::clear_test_frame();
}
