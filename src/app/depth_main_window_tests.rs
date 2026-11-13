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
    child: Option<ChildDepthPass>,
}

struct ChildDepthPass {
    events: std::rc::Rc<std::cell::RefCell<Vec<egui::Event>>>,
    size: std::rc::Rc<std::cell::Cell<egui::Vec2>>,
    ppp: std::rc::Rc<std::cell::Cell<f32>>,
    shapes: std::rc::Rc<std::cell::RefCell<Vec<egui::epaint::ClippedShape>>>,
    viewport: std::rc::Rc<std::cell::Cell<egui::ViewportId>>,
    passes: std::rc::Rc<std::cell::Cell<u64>>,
    commands: std::rc::Rc<std::cell::RefCell<Vec<(egui::ViewportId, bool)>>>,
    actual_ppp: std::rc::Rc<std::cell::Cell<f32>>,
    depth_painted: std::rc::Rc<std::cell::Cell<bool>>,
    modal_layers: std::rc::Rc<std::cell::RefCell<DepthModalLayers>>,
}

#[derive(Clone, Default)]
pub(crate) struct DepthModalLayers {
    pub(crate) layers: Vec<egui::LayerId>,
    pub(crate) top_modal: Option<egui::LayerId>,
    pub(crate) panel_interactive: bool,
    pub(crate) dialog_interactive: bool,
}

fn depth_modal_layers(ctx: &egui::Context) -> DepthModalLayers {
    ctx.memory(|memory| DepthModalLayers {
        layers: memory.layer_ids().collect(),
        top_modal: memory.top_modal_layer(),
        panel_interactive: memory.allows_interaction(egui::LayerId::new(
            egui::Order::Foreground,
            egui::Id::unique("depth_parallax_panel"),
        )),
        dialog_interactive: memory.allows_interaction(egui::LayerId::new(
            egui::Order::Foreground,
            egui::Id::unique("depth_export_dialog"),
        )),
    })
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
            child: None,
        }
    }

    pub(crate) fn linked(app: &mut App) -> Self {
        let mut viewer = Self::new(app);
        viewer.ctx.set_embed_viewports(false);
        app.toggle_detached_viewer_mode();
        assert_eq!(app.viewer_presentation, ViewerPresentation::DetachedWindow);
        assert!(!app.detached_viewer_independent_active);
        app.set_fs_exclusive_view(
            FsExclusiveView::Depth(DepthViewState::new(app.edit_request_owner_context())),
            "linked_depth_fixture",
        );
        let child = ChildDepthPass {
            events: Default::default(),
            size: std::rc::Rc::new(std::cell::Cell::new(viewer.size)),
            ppp: std::rc::Rc::new(std::cell::Cell::new(1.0)),
            shapes: Default::default(),
            viewport: std::rc::Rc::new(std::cell::Cell::new(egui::ViewportId::ROOT)),
            passes: Default::default(),
            commands: Default::default(),
            actual_ppp: Default::default(),
            depth_painted: Default::default(),
            modal_layers: Default::default(),
        };
        let events = child.events.clone();
        let size = child.size.clone();
        let ppp = child.ppp.clone();
        let shapes = child.shapes.clone();
        let viewport_id = child.viewport.clone();
        let passes = child.passes.clone();
        let commands = child.commands.clone();
        let actual_ppp = child.actual_ppp.clone();
        let depth_painted = child.depth_painted.clone();
        let modal_layers = child.modal_layers.clone();
        egui::Context::set_immediate_viewport_renderer(move |ctx, mut viewport| {
            passes.set(passes.get() + 1);
            viewport_id.set(viewport.ids.this);
            let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, size.get());
            let output = ctx.run_ui(
                egui::RawInput {
                    viewport_id: viewport.ids.this,
                    screen_rect: Some(rect),
                    events: std::mem::take(&mut *events.borrow_mut()),
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
                                inner_rect: Some(rect),
                                native_pixels_per_point: Some(ppp.get()),
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
                    actual_ppp.set(ui.ctx().pixels_per_point());
                    depth_painted.set(
                        crate::app::depth_export::display_painted_this_pass_for_test(ui.ctx()),
                    );
                    *modal_layers.borrow_mut() = depth_modal_layers(ui.ctx());
                },
            );
            shapes.borrow_mut().extend(output.shapes.clone());
            for (id, viewport) in &output.viewport_output {
                for command in &viewport.commands {
                    if let egui::ViewportCommand::CursorVisible(visible) = command {
                        commands.borrow_mut().push((*id, *visible));
                    }
                }
            }
            output.drop_without_applying_deltas();
        });
        viewer.child = Some(child);
        viewer
    }

    pub(crate) fn set_pixels_per_point(&self, ppp: f32) {
        self.child.as_ref().unwrap().ppp.set(ppp);
    }

    pub(crate) fn child_viewport(&self) -> egui::ViewportId {
        self.child.as_ref().unwrap().viewport.get()
    }

    pub(crate) fn child_passes(&self) -> u64 {
        self.child.as_ref().unwrap().passes.get()
    }

    pub(crate) fn modal_layers(&self) -> DepthModalLayers {
        self.child.as_ref().map_or_else(
            || depth_modal_layers(&self.ctx),
            |child| child.modal_layers.borrow().clone(),
        )
    }

    pub(crate) fn ready(&self, app: &mut App) -> u64 {
        self.ready_with_depth_values(app, [4, 6], vec![half::f16::from_f32(0.5); 24])
    }

    pub(crate) fn ready_with_depth_values(
        &self,
        app: &mut App,
        size: [u32; 2],
        values: Vec<half::f16>,
    ) -> u64 {
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
            width: size[0],
            height: size[1],
            values,
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

    pub(crate) fn frame_with_before(
        &mut self,
        app: &mut App,
        events: Vec<egui::Event>,
        mut before: impl FnMut(&mut App, &egui::Context),
    ) {
        app.frame_counter += 1;
        let events = if let Some(child) = &self.child {
            child.size.set(self.size);
            child.shapes.borrow_mut().clear();
            child.depth_painted.set(false);
            *child.events.borrow_mut() = events;
            vec![]
        } else {
            events
        };
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
                depth_painted = app
                    .capture_depth_export(ctx, self.child.is_none())
                    .is_some();
            },
        );
        self.shapes = output.shapes.clone();
        if let Some(child) = &self.child {
            self.shapes = child.shapes.borrow().clone();
            depth_painted = child.depth_painted.get();
            for (id, viewport) in &output.viewport_output {
                for command in &viewport.commands {
                    if let egui::ViewportCommand::CursorVisible(visible) = command {
                        child.commands.borrow_mut().push((*id, *visible));
                    }
                }
            }
        }
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
fn depth_collapsed_wait_indicator_can_use_the_old_panel_body() {
    let _input = crate::key_input::lock_test_input();
    let mut app = setup_app_for_test();
    let mut viewer = RootDepthViewer::new(&mut app);
    viewer.frame(&mut app, vec![]);
    let old = viewer.panel();
    let full = egui::Rect::from_min_max(
        egui::pos2(old.left() + 8., 0.),
        egui::pos2(old.right(), 614.),
    );
    for collapsed in [false, true] {
        app.settings.depth_parallax.panel_collapsed = collapsed;
        let FsExclusiveView::Depth(state) = &mut app.fs_exclusive_view else {
            unreachable!()
        };
        state.wait_started_at = std::time::Instant::now() - std::time::Duration::from_secs(1);
        let output = viewer.ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, viewer.size)),
                ..Default::default()
            },
            |ui| {
                app.draw_depth_panel(ui.ctx(), 0);
                app.draw_depth_wait_indicator(ui, full, ui.ctx(), ActionSurface::Viewer);
            },
        );
        viewer.shapes = output.shapes.clone();
        output.drop_without_applying_deltas();
        if collapsed {
            let caption = viewer.label("D で取り消し");
            assert!(old.intersects(caption));
            assert!(!viewer.panel().intersects(caption));
        } else {
            assert!(viewer.text_rects("D で取り消し").is_empty());
        }
    }
    crate::key_input::clear_test_frame();
}

#[test]
fn depth_main_window_root_paints_fit_rotation_resize_underlay_without_mutating_2d_transform() {
    let _input = crate::key_input::lock_test_input();
    for linked in [false, true] {
        let mut app = setup_app_for_test();
        let mut viewer = if linked {
            RootDepthViewer::linked(&mut app)
        } else {
            RootDepthViewer::new(&mut app)
        };
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
                            && (transform.paint_rect.max - expected.max).abs().max_elem()
                                <= tolerance,
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
        let old_final =
            app.final_composite_key_for_pixels(old_key, [4, 6], app.effective_params(0));
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
    }
    crate::key_input::clear_test_frame();
}

#[test]
fn depth_main_window_root_wait_and_mouse_use_current_panel_and_resized_fit_coordinates() {
    let _input = crate::key_input::lock_test_input();
    for linked in [false, true] {
        let mut app = setup_app_for_test();
        let mut viewer = if linked {
            RootDepthViewer::linked(&mut app)
        } else {
            RootDepthViewer::new(&mut app)
        };
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
                (target - egui::vec2(0.5, 0.3) * app.settings.depth_parallax.strength).length()
                    < 1e-5
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
    }
    crate::key_input::clear_test_frame();
}

#[test]
fn depth_linked_child_entry_panel_keys_and_dpi_use_the_current_pass_geometry() {
    std::thread::spawn(|| {
        let _input = crate::key_input::lock_test_input();
        let mut app = setup_app_for_test();
        let mut viewer = RootDepthViewer::linked(&mut app);
        let model_path = app.tmp.path().join("unused-linked.onnx");
        app.depth_setup
            .set_status_for_test(crate::addon::AddonStatus::Valid {
                model_id: "linked-headless".into(),
                bytes: 1,
                model_path,
                mmproj_path: None,
            });
        app.set_fs_exclusive_view(FsExclusiveView::None, "linked_entry_positive_control");
        let key = |key| {
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
        viewer.frame(&mut app, key(egui::Key::D));
        assert!(
            app.fs_exclusive_view.is_depth(),
            "D is handled in the linked child"
        );
        assert_ne!(viewer.child_viewport(), egui::ViewportId::ROOT);
        let commands = viewer.child.as_ref().unwrap().commands.borrow();
        assert!(
            !commands.is_empty(),
            "first child pass synchronizes cursor visibility"
        );
        assert!(
            commands
                .iter()
                .all(|(id, _)| *id == viewer.child_viewport()),
            "cursor commands target the owning child"
        );
        drop(commands);
        let depth_size = [64_u32, 96_u32];
        let values = (0..depth_size[1])
            .flat_map(|_| {
                (0..depth_size[0])
                    .map(|x| half::f16::from_f32(x as f32 / (depth_size[0] - 1) as f32))
            })
            .collect();
        let resource_id = viewer.ready_with_depth_values(&mut app, depth_size, values);
        app.settings.depth_parallax.shape_depth = false;
        viewer.frame(&mut app, vec![]);
        viewer.frame(&mut app, vec![]);
        for (ppp, size) in [
            (1.0, egui::vec2(1093., 614.)),
            (2.0, egui::vec2(820., 600.)),
            (1.5, egui::vec2(960., 640.)),
        ] {
            app.fs_focus_regained_at = None;
            app.settings.depth_parallax.focus = 0.1;
            let old = app.fullscreen_page_layout.single_page().unwrap().transform;
            let click = old.paint_rect.min + old.paint_rect.size() * egui::vec2(0.2, 0.55);
            let ctrl_button = |pressed| egui::Event::PointerButton {
                pos: click,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: egui::Modifiers::CTRL,
            };
            viewer.frame(
                &mut app,
                vec![egui::Event::PointerMoved(click), ctrl_button(true)],
            );
            viewer.set_pixels_per_point(ppp);
            viewer.size = size;
            // DPI and rect change on the release pass, with no prior repaint at the new geometry.
            viewer.frame(
                &mut app,
                vec![
                    ctrl_button(false),
                    egui::Event::ModifiersChanged(egui::Modifiers::NONE),
                ],
            );
            let current = app.fullscreen_page_layout.single_page().unwrap().transform;
            assert!(
                (viewer.child.as_ref().unwrap().actual_ppp.get() - ppp).abs() < 1e-6,
                "native DPI is applied in the actual child pass"
            );
            let FsExclusiveView::Depth(state) = &app.fs_exclusive_view else {
                unreachable!()
            };
            let resource = state.ready().unwrap();
            let sample = |transform: &crate::displayed_image_transform::DisplayedImageTransform| {
                let params = crate::depth_parallax_wgpu::DepthRenderParams::from_settings(
                    &app.settings.depth_parallax,
                    resource.depth.median,
                    egui::Vec2::ZERO,
                    transform.uv_rect,
                    transform.rotation.degrees() as u32 / 90,
                    transform.paint_rect.aspect_ratio(),
                );
                crate::depth_parallax_wgpu::focus_depth_at(
                    click,
                    transform.paint_rect,
                    params,
                    resource.depth.width,
                    resource.depth.height,
                    &resource.depth.values,
                )
                .unwrap()
            };
            let expected = sample(&current);
            assert!(
                (app.settings.depth_parallax.focus - expected).abs() < 1e-4,
                "Ctrl release samples the new child transform at {ppp} DPI"
            );
            if old.paint_rect != current.paint_rect {
                assert!(
                    (sample(&old) - expected).abs() > 0.1,
                    "the gradient distinguishes stale geometry after the DPI/rect change"
                );
            }
            viewer.frame(&mut app, vec![]);
            viewer.frame(&mut app, vec![]);
            let transform = app.fullscreen_page_layout.single_page().unwrap().transform;
            assert_eq!(viewer.callbacks(), vec![transform.paint_rect]);
            assert!(
                egui::Rect::from_min_size(egui::Pos2::ZERO, size)
                    .contains_rect(transform.paint_rect)
            );
            let radius = transform
                .viewport_rect
                .width()
                .min(transform.viewport_rect.height())
                * 0.25;
            let point = transform.paint_rect.center() - egui::vec2(radius * 0.5, radius * 0.5);
            viewer.frame(&mut app, vec![egui::Event::PointerMoved(point)]);
            let FsExclusiveView::Depth(state) = &app.fs_exclusive_view else {
                panic!("linked depth");
            };
            assert_eq!(state.ready().unwrap().resource_id, resource_id);
            assert!(
                (state.mouse.target - egui::vec2(0.5, 0.3) * app.settings.depth_parallax.strength)
                    .length()
                    < 1e-5
            );
            let panel = viewer.panel();
            let before = state.mouse.target;
            viewer.frame(&mut app, vec![egui::Event::PointerMoved(panel.center())]);
            let FsExclusiveView::Depth(state) = &app.fs_exclusive_view else {
                unreachable!()
            };
            assert_eq!(
                state.mouse.target, before,
                "current child panel excludes mouse at DPI {ppp}"
            );
            viewer.frame(&mut app, key(egui::Key::I));
            assert!(
                matches!(&app.fs_exclusive_view, FsExclusiveView::Depth(s) if !s.panel_visible)
            );
            viewer.frame(&mut app, key(egui::Key::Tab));
            assert!(matches!(&app.fs_exclusive_view, FsExclusiveView::Depth(s) if s.panel_visible));
        }
        viewer.frame(&mut app, vec![egui::Event::PointerGone]);
        app.cursor_last_activity =
            Some(std::time::Instant::now() - std::time::Duration::from_secs(120));
        viewer.child.as_ref().unwrap().commands.borrow_mut().clear();
        viewer.frame(&mut app, vec![]);
        assert!(
            viewer
                .child
                .as_ref()
                .unwrap()
                .commands
                .borrow()
                .contains(&(viewer.child_viewport(), false)),
            "idle cursor hiding targets the linked child viewport"
        );
        crate::key_input::clear_test_frame();
    })
    .join()
    .unwrap();
}

#[test]
fn depth_linked_actual_readback_submits_the_child_viewport_to_the_worker() {
    std::thread::spawn(|| {
        let _input = crate::key_input::lock_test_input();
        let mut app = setup_app_for_test();
        let mut viewer = RootDepthViewer::linked(&mut app);
        viewer.ready(&mut app);
        let source = app.ensure_final_composite_material(&viewer.ctx, 0).unwrap();
        let key = crate::depth_estimation::DepthSourceKey {
            texture_id: source.source_texture().id(),
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
        assert!(matches!(
            state.source,
            crate::depth_parallax::DepthSourcePhase::ReadingBack { .. }
        ));
        app.set_fs_exclusive_view(
            FsExclusiveView::Depth(state),
            "linked_real_worker_submission",
        );
        render
            .device
            .poll(wgpu::PollType::Wait {
                submission_index: None,
                timeout: None,
            })
            .unwrap();
        crate::depth_estimation::DepthWorker::take_submission_for_test();
        viewer.frame(&mut app, vec![]);
        assert_eq!(
            crate::depth_estimation::DepthWorker::take_submission_for_test(),
            Some((
                app.edit_request_owner_context(),
                viewer.child_viewport(),
                key
            ))
        );
        assert_ne!(viewer.child_viewport(), egui::ViewportId::ROOT);
        crate::key_input::clear_test_frame();
    })
    .join()
    .unwrap();
}

#[test]
fn depth_top_button_color_matches_the_context_and_session_entry_matrix() {
    std::thread::spawn(|| {
        let _input = crate::key_input::lock_test_input();
        for presentation in [ViewerPresentation::MainWindow, ViewerPresentation::Fullscreen,
            ViewerPresentation::DetachedWindow] {
            for (other_context, independent) in [(false, false), (false, true), (true, false)] {
                let mut app = setup_app_for_test();
                let mut viewer = if presentation == ViewerPresentation::MainWindow {
                    RootDepthViewer::new(&mut app)
                } else { RootDepthViewer::linked(&mut app) };
                let context = if other_context {
                    app.build_window_context_for_test(88_970, |owner| {
                        super::depth_view_tests::install_pages(owner, &viewer.ctx);
                        crate::app::source_texture_test_support::seed(owner, &viewer.ctx, 0);
                    })
                } else { app.viewer_context_main() };
                app.with_viewer_context(context, |owner| {
                    owner.viewer_presentation = presentation;
                    owner.detached_viewer_independent_active = independent;
                    owner.native_video_in_window_active = presentation == ViewerPresentation::MainWindow;
                    owner.settings.video_in_window_mode = presentation == ViewerPresentation::MainWindow;
                    owner.settings.detached_viewer_enabled = presentation == ViewerPresentation::DetachedWindow;
                    owner.set_fs_exclusive_view(FsExclusiveView::None, "button_entry_matrix");
                    let allowed = !other_context && (presentation != ViewerPresentation::DetachedWindow || !independent);
                    assert_eq!(owner.depth_entry_allowed(0).is_ok(), allowed);
                    for _ in 0..3 {
                        viewer.frame(owner, vec![egui::Event::PointerMoved(egui::pos2(300., 12.))]);
                    }
                    fn colors(shape: &egui::Shape, result: &mut Vec<egui::Color32>) {
                        match shape {
                            egui::Shape::Text(text) if text.galley.text() == "3D" => {
                                result.extend(text.galley.job.sections.iter().map(|s| s.format.color));
                            }
                            egui::Shape::Vec(shapes) => shapes.iter().for_each(|s| colors(s, result)),
                            _ => {}
                        }
                    }
                    let mut actual = vec![];
                    for shape in &viewer.shapes { colors(&shape.shape, &mut actual); }
                    assert_eq!(actual, vec![if allowed { egui::Color32::WHITE } else { egui::Color32::from_gray(90) }],
                        "3D button: {presentation:?}, other={other_context}, independent={independent}");
                }).unwrap();
            }
        }
        assert_eq!(crate::depth_parallax::DepthEntryBlock::Detached.message(),
            "3D 写真は、メインの画像表示と F12 の別ウィンドウで使えます");
        crate::key_input::clear_test_frame();
    }).join().unwrap();
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
