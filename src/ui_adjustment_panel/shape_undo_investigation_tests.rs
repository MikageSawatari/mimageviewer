//! Before/after investigation of shape history and keyboard routing around section 1.679.
//! No native window, OS input, or normal-profile data is used.
use super::*;
use crate::displayed_image_transform::{
    DisplayedImageTransformInput, FullscreenFitScaleLimits, RectPixelFit, ResolvedDisplayPlacement,
};
use local_adjust_core::{LocalAdjustmentLayer, LocalEffect, LocalMask, RasterVectorMask};

fn setup(ctx: &egui::Context) -> (crate::app::AppTestEnvForTest, DisplayedImageTransform) {
    let mut app = crate::app::setup_app_for_test();
    app.items = vec![crate::grid_item::GridItem::Image(
        app.tmp.path().join("click-timing.png"),
    )];
    app.fullscreen_idx = Some(0);
    app.local_adjust_mode = true;
    app.local_adjust_mask_edit_target = LocalAdjustMaskEditTarget::Base;
    app.local_adjust_mask_paint_add = true;
    app.local_adjust_bucket_source = crate::mask_db::BucketSource::DrawnMask;
    app.local_adjust_bucket_leak_stop = 0.0;
    app.local_adjust_bucket_outset = 0.0;
    app.local_adjust_page_layers.insert(
        0,
        Arc::new(vec![LocalAdjustmentLayer::new(
            "timing",
            LocalMask::RasterVector(RasterVectorMask::empty(32, 32)),
            LocalEffect::None,
        )]),
    );
    let pixels = egui::ColorImage::filled([32, 32], egui::Color32::WHITE);
    app.fs_cache.insert(
        0,
        crate::fs_animation::FsCacheEntry::Static {
            pdf_metadata: None,
            pdf_rerender_failures: Vec::new(),
            tex: ctx.load_texture("click-timing", pixels.clone(), Default::default()),
            pixels: Arc::new(pixels),
            source_dims: Some([32, 32]),
            load_seq: 0,
            animation: crate::fs_animation::StaticAnimationState::Still,
        },
    );
    let transform = DisplayedImageTransform::resolve(DisplayedImageTransformInput {
        page_idx: 0,
        viewport_rect: egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1920.0, 1080.0)),
        source_size: egui::vec2(32.0, 32.0),
        texture_size: egui::vec2(32.0, 32.0),
        rotation: crate::rotation_db::Rotation::None,
        free_rotation_rad: 0.0,
        content_bbox: None,
        fit_mode: crate::fullscreen_specified_scale::FullscreenFitInput::legacy(
            crate::settings::FullscreenFitMode::Page,
        ),
        fit_scale_limits: FullscreenFitScaleLimits::default(),
        pixels_per_point: 1.0,
        placement: ResolvedDisplayPlacement::Normal { zoom_pan: None },
        pixel_fit: RectPixelFit::Texels,
    })
    .unwrap();
    (app, transform)
}

fn button(pos: egui::Pos2, pressed: bool) -> egui::Event {
    egui::Event::PointerButton {
        pos,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: egui::Modifiers::NONE,
    }
}

#[derive(Clone, Copy, Debug)]
enum Canvas {
    Local,
    Conceal,
    Erase,
}

fn frame(
    app: &mut App,
    ctx: &egui::Context,
    transform: &DisplayedImageTransform,
    canvas: Canvas,
    time: f64,
    events: Vec<egui::Event>,
) -> (bool, bool, bool) {
    let mut flags = (false, false, false);
    let _ = ctx
        .run_pass(
            egui::RawInput {
                screen_rect: Some(transform.viewport_rect),
                time: Some(time),
                events,
                focused: true,
                ..Default::default()
            },
            |ctx| {
                flags = ctx.input(|i| {
                    (
                        i.pointer.primary_pressed(),
                        i.pointer.primary_down(),
                        i.pointer.primary_released(),
                    )
                });
                match canvas {
                    Canvas::Local => app.handle_local_adjust_canvas_input(
                        ctx,
                        transform.viewport_rect,
                        transform,
                        Default::default(),
                    ),
                    Canvas::Conceal => app.handle_conceal_paint(ctx, transform, Default::default()),
                    Canvas::Erase => app.handle_erase_paint(ctx, transform, Default::default()),
                }
            },
        )
        .drop_without_applying_deltas();
    flags
}

fn prepare_canvas(app: &mut App, canvas: Canvas, tool: LocalAdjustMaskTool) {
    if !matches!(canvas, Canvas::Local) {
        app.local_adjust_mode = false;
        app.local_adjust_page_layers.clear();
    }
    app.conceal_mode = matches!(canvas, Canvas::Conceal);
    app.erase_mode = matches!(canvas, Canvas::Erase);
    app.local_adjust_mask_tool = tool;
    app.local_adjust_mask_brush_radius = 1.0;
    app.conceal_tool = match tool {
        LocalAdjustMaskTool::Brush => crate::conceal::ConcealTool::Brush,
        LocalAdjustMaskTool::Polygon => crate::conceal::ConcealTool::Polygon,
        LocalAdjustMaskTool::Line => crate::conceal::ConcealTool::Line,
        LocalAdjustMaskTool::VertLine => crate::conceal::ConcealTool::VertLine,
        LocalAdjustMaskTool::HorizLine => crate::conceal::ConcealTool::HorizLine,
        LocalAdjustMaskTool::Rect => crate::conceal::ConcealTool::Rect,
        LocalAdjustMaskTool::Ellipse => crate::conceal::ConcealTool::Ellipse,
        LocalAdjustMaskTool::Lasso => crate::conceal::ConcealTool::Lasso,
        _ => crate::conceal::ConcealTool::Bucket,
    };
    app.erase_tool = match tool {
        LocalAdjustMaskTool::Brush => crate::app::EraseTool::Brush,
        LocalAdjustMaskTool::Polygon => crate::app::EraseTool::Polygon,
        LocalAdjustMaskTool::Line => crate::app::EraseTool::Line,
        LocalAdjustMaskTool::VertLine => crate::app::EraseTool::VertLine,
        LocalAdjustMaskTool::HorizLine => crate::app::EraseTool::HorizLine,
        LocalAdjustMaskTool::Rect => crate::app::EraseTool::Rect,
        LocalAdjustMaskTool::Ellipse => crate::app::EraseTool::Ellipse,
        LocalAdjustMaskTool::Lasso => crate::app::EraseTool::Lasso,
        _ => crate::app::EraseTool::Bucket,
    };
    app.conceal_paint_mode = true;
    app.conceal_mask_size = [32, 32];
    app.conceal_mask = Some(vec![false; 32 * 32]);
    app.settings.conceal_brush_radius = 1.0;
    app.conceal_bucket_source = crate::mask_db::BucketSource::DrawnMask;
    app.conceal_bucket_region = crate::mask_db::BucketRegion::Connected;
    app.conceal_bucket_leak_stop = 0.0;
    app.conceal_bucket_outset = 0.0;
    app.erase_paint_mode = true;
    app.erase_mask_size = [32, 32];
    app.erase_mask = Some(vec![false; 32 * 32]);
    app.erase_brush_radius = 1.0;
    app.erase_bucket_source = crate::mask_db::BucketSource::DrawnMask;
    app.erase_bucket_region = crate::mask_db::BucketRegion::Connected;
    app.erase_bucket_leak_stop = 0.0;
    app.erase_bucket_outset = 0.0;
    app.local_adjust_bucket_region = crate::mask_db::BucketRegion::Connected;
    app.erase_base_cache.insert(
        0,
        crate::app::EraseBaseSource::raster(Arc::new(egui::ColorImage::filled(
            [32, 32],
            egui::Color32::WHITE,
        ))),
    );
}

fn shapes(app: &App, canvas: Canvas) -> Vec<crate::mask_db::Shape> {
    match canvas {
        Canvas::Local => local_adjust_target_raster_vector_mask_ref(
            &app.local_adjust_page_layers[&0][0],
            LocalAdjustMaskEditTarget::Base,
        )
        .unwrap()
        .shapes
        .iter()
        .copied()
        .map(local_adjust_shape_to_vector_shape)
        .collect(),
        Canvas::Conceal => app.conceal_shapes.clone(),
        Canvas::Erase => app.erase_shapes.clone(),
    }
}

fn undo_count(app: &App, canvas: Canvas) -> usize {
    match canvas {
        Canvas::Local => app.meta_undo.undo_len(),
        Canvas::Conceal => app.conceal_undo_stack.len(),
        Canvas::Erase => app.erase_undo_stack.len(),
    }
}

fn at(transform: &DisplayedImageTransform, uv: [f32; 2]) -> egui::Pos2 {
    local_adjust_norm_to_screen(uv, transform).unwrap()
}

fn key(app: &mut App, ctx: &egui::Context, key: egui::Key, modifiers: egui::Modifiers) {
    let _ = ctx
        .run_pass(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1920.0, 1080.0),
                )),
                focused: true,
                events: vec![
                    egui::Event::ModifiersChanged(modifiers),
                    key_event(key, modifiers, true),
                    key_event(key, modifiers, false),
                ],
                ..Default::default()
            },
            |ctx| {
                app.handle_fs_key_input(ctx, 0, false);
            },
        )
        .drop_without_applying_deltas();
}

#[test]
fn shape_undo_investigation_create_undo_redo_all_canvases() {
    for canvas in [Canvas::Local, Canvas::Conceal, Canvas::Erase] {
        for tool in [
            LocalAdjustMaskTool::HorizLine,
            LocalAdjustMaskTool::VertLine,
            LocalAdjustMaskTool::Line,
            LocalAdjustMaskTool::Rect,
            LocalAdjustMaskTool::Ellipse,
        ] {
            let ctx = egui::Context::default();
            let (mut app, transform) = setup(&ctx);
            prepare_canvas(&mut app, canvas, tool);
            let a = at(&transform, [0.3, 0.3]);
            let b = at(&transform, [0.7, 0.7]);
            for (i, events) in [
                vec![button(a, true)],
                vec![egui::Event::PointerMoved(b)],
                vec![button(b, false)],
            ]
            .into_iter()
            .enumerate()
            {
                frame(
                    &mut app,
                    &ctx,
                    &transform,
                    canvas,
                    1.0 + i as f64 * 0.1,
                    events,
                );
            }
            let created = shapes(&app, canvas);
            assert_eq!(created.len(), 1, "{canvas:?} {tool:?}: create");
            assert_eq!(undo_count(&app, canvas), 1, "{canvas:?} {tool:?}: history");
            key(&mut app, &ctx, egui::Key::Z, egui::Modifiers::CTRL);
            assert!(
                shapes(&app, canvas).is_empty(),
                "{canvas:?} {tool:?}: Ctrl+Z"
            );
            key(&mut app, &ctx, egui::Key::Y, egui::Modifiers::CTRL);
            assert_eq!(shapes(&app, canvas), created, "{canvas:?} {tool:?}: Ctrl+Y");
        }
    }
}

fn key_event(key: egui::Key, modifiers: egui::Modifiers, pressed: bool) -> egui::Event {
    egui::Event::Key {
        key,
        physical_key: Some(key),
        pressed,
        repeat: false,
        modifiers,
    }
}

fn drag(
    app: &mut App,
    ctx: &egui::Context,
    transform: &DisplayedImageTransform,
    canvas: Canvas,
    start: [f32; 2],
    end: [f32; 2],
    time: f64,
) {
    let a = at(transform, start);
    let b = at(transform, end);
    for (i, events) in [
        vec![button(a, true)],
        vec![egui::Event::PointerMoved(b)],
        vec![button(b, false)],
    ]
    .into_iter()
    .enumerate()
    {
        frame(app, ctx, transform, canvas, time + i as f64 * 0.1, events);
    }
}

#[test]
fn shape_undo_investigation_move_delete_undo_redo_all_canvases() {
    for canvas in [Canvas::Local, Canvas::Conceal, Canvas::Erase] {
        let ctx = egui::Context::default();
        let (mut app, transform) = setup(&ctx);
        prepare_canvas(&mut app, canvas, LocalAdjustMaskTool::Rect);
        drag(
            &mut app,
            &ctx,
            &transform,
            canvas,
            [0.3, 0.3],
            [0.7, 0.7],
            1.0,
        );
        let created = shapes(&app, canvas);
        app.local_adjust_mask_tool = LocalAdjustMaskTool::Select;
        app.erase_tool = crate::app::EraseTool::Select;
        app.conceal_tool = crate::conceal::ConcealTool::Select;
        drag(
            &mut app,
            &ctx,
            &transform,
            canvas,
            [0.5, 0.5],
            [0.6, 0.6],
            2.0,
        );
        let moved = shapes(&app, canvas);
        assert_ne!(moved, created, "{canvas:?}: move");
        assert_eq!(undo_count(&app, canvas), 2, "{canvas:?}: move history");
        key(&mut app, &ctx, egui::Key::Z, egui::Modifiers::CTRL);
        assert_eq!(shapes(&app, canvas), created, "{canvas:?}: undo move");
        key(&mut app, &ctx, egui::Key::Y, egui::Modifiers::CTRL);
        assert_eq!(shapes(&app, canvas), moved, "{canvas:?}: redo move");
        // History restores geometry, not an erase/conceal selection. Select again through the handler.
        drag(
            &mut app,
            &ctx,
            &transform,
            canvas,
            [0.6, 0.6],
            [0.6, 0.6],
            3.0,
        );
        let before_delete = undo_count(&app, canvas);
        key(&mut app, &ctx, egui::Key::Delete, egui::Modifiers::NONE);
        assert!(shapes(&app, canvas).is_empty(), "{canvas:?}: delete");
        assert_eq!(
            undo_count(&app, canvas),
            before_delete + 1,
            "{canvas:?}: delete history"
        );
        key(&mut app, &ctx, egui::Key::Z, egui::Modifiers::CTRL);
        assert_eq!(shapes(&app, canvas), moved, "{canvas:?}: undo delete");
        key(&mut app, &ctx, egui::Key::Y, egui::Modifiers::CTRL);
        assert!(shapes(&app, canvas).is_empty(), "{canvas:?}: redo delete");
    }
}

fn bitmap(app: &App, canvas: Canvas) -> Vec<bool> {
    match canvas {
        Canvas::Local => local_adjust_target_raster_vector_mask_ref(
            &app.local_adjust_page_layers[&0][0],
            LocalAdjustMaskEditTarget::Base,
        )
        .unwrap()
        .alpha
        .iter()
        .map(|v| *v > 0.0)
        .collect(),
        Canvas::Conceal => app.conceal_mask.clone().unwrap(),
        Canvas::Erase => app.erase_mask.clone().unwrap(),
    }
}

#[test]
fn shape_undo_investigation_polygon_is_bitmap_and_undo_redo_all_canvases() {
    for canvas in [Canvas::Local, Canvas::Conceal, Canvas::Erase] {
        let ctx = egui::Context::default();
        let (mut app, transform) = setup(&ctx);
        prepare_canvas(&mut app, canvas, LocalAdjustMaskTool::Polygon);
        for (i, uv) in [[0.3, 0.3], [0.7, 0.3], [0.5, 0.7]].into_iter().enumerate() {
            let pos = at(&transform, uv);
            frame(
                &mut app,
                &ctx,
                &transform,
                canvas,
                1.0 + i as f64,
                vec![button(pos, true)],
            );
            frame(
                &mut app,
                &ctx,
                &transform,
                canvas,
                1.1 + i as f64,
                vec![button(pos, false)],
            );
        }
        key(&mut app, &ctx, egui::Key::Enter, egui::Modifiers::NONE);
        let filled = bitmap(&app, canvas);
        assert!(filled.iter().any(|v| *v), "{canvas:?}: polygon");
        assert!(shapes(&app, canvas).is_empty(), "polygon is rasterized");
        assert_eq!(undo_count(&app, canvas), 1, "{canvas:?}: polygon history");
        key(&mut app, &ctx, egui::Key::Z, egui::Modifiers::CTRL);
        assert!(
            bitmap(&app, canvas).iter().all(|v| !*v),
            "{canvas:?}: undo polygon"
        );
        key(&mut app, &ctx, egui::Key::Y, egui::Modifiers::CTRL);
        assert_eq!(bitmap(&app, canvas), filled, "{canvas:?}: redo polygon");
    }
}

#[derive(Clone, Copy, Debug)]
enum Focus {
    Button,
    Slider,
}

#[test]
fn shape_undo_investigation_focused_widgets_block_local_undo_with_history_intact() {
    for focus in [Focus::Button, Focus::Slider] {
        let ctx = egui::Context::default();
        let (mut app, transform) = setup(&ctx);
        prepare_canvas(&mut app, Canvas::Local, LocalAdjustMaskTool::HorizLine);
        drag(
            &mut app,
            &ctx,
            &transform,
            Canvas::Local,
            [0.3, 0.3],
            [0.7, 0.7],
            1.0,
        );
        assert_eq!(app.local_adjust_selected_shape, Some(0));
        let created = shapes(&app, Canvas::Local);
        let mut value = 0.5;
        let draw = |root: &mut egui::Ui, value: &mut f32| {
            egui::CentralPanel::default().show(root, |ui| {
                let response = match focus {
                    Focus::Button => ui.button("shape tool"),
                    Focus::Slider => ui.add(egui::Slider::new(value, 0.0..=1.0)),
                };
                response.request_focus();
            });
        };
        let _ = ctx
            .run_ui(
                egui::RawInput {
                    focused: true,
                    ..Default::default()
                },
                |root| {
                    draw(root, &mut value);
                },
            )
            .drop_without_applying_deltas();
        let _ = ctx
            .run_ui(
                egui::RawInput {
                    focused: true,
                    events: vec![
                        egui::Event::ModifiersChanged(egui::Modifiers::CTRL),
                        key_event(egui::Key::Z, egui::Modifiers::CTRL, true),
                        key_event(egui::Key::Z, egui::Modifiers::CTRL, false),
                    ],
                    ..Default::default()
                },
                |root| {
                    let ctx = &root.ctx().clone();
                    assert!(ctx.egui_wants_keyboard_input(), "{focus:?}");
                    assert!(matches!(
                        app.keyboard_owner_for_pass(ctx),
                        crate::keyboard_input::KeyboardOwner::FocusedUi { .. }
                    ));
                    app.handle_fs_key_input(ctx, 0, false);
                    assert!(ctx.input(|i| i.events.iter().any(|event| matches!(
                        event,
                        egui::Event::Key {
                            key: egui::Key::Z,
                            pressed: true,
                            ..
                        }
                    ))));
                    draw(root, &mut value);
                },
            )
            .drop_without_applying_deltas();
        assert_eq!(
            shapes(&app, Canvas::Local),
            created,
            "{focus:?}: blocked undo"
        );
        assert_eq!(
            undo_count(&app, Canvas::Local),
            1,
            "{focus:?}: history retained"
        );
        // Releasing UI focus enables the identical key path; history was never missing.
        ctx.memory_mut(|memory| {
            if let Some(id) = memory.focused() {
                memory.surrender_focus(id);
            }
        });
        key(&mut app, &ctx, egui::Key::Z, egui::Modifiers::CTRL);
        assert!(
            shapes(&app, Canvas::Local).is_empty(),
            "{focus:?}: undo after focus release"
        );
        key(&mut app, &ctx, egui::Key::Y, egui::Modifiers::CTRL);
        assert_eq!(shapes(&app, Canvas::Local), created, "{focus:?}: redo");
    }
}

#[test]
fn shape_undo_investigation_bucket_undo_then_hline_undo_redo() {
    let ctx = egui::Context::default();
    let (mut app, transform) = setup(&ctx);
    prepare_canvas(&mut app, Canvas::Local, LocalAdjustMaskTool::Bucket);
    let pos = at(&transform, [0.5, 0.5]);
    frame(
        &mut app,
        &ctx,
        &transform,
        Canvas::Local,
        1.0,
        vec![button(pos, true)],
    );
    frame(
        &mut app,
        &ctx,
        &transform,
        Canvas::Local,
        1.1,
        vec![button(pos, false)],
    );
    assert!(bitmap(&app, Canvas::Local).iter().any(|v| *v));
    key(&mut app, &ctx, egui::Key::Z, egui::Modifiers::CTRL);
    assert!(bitmap(&app, Canvas::Local).iter().all(|v| !*v));
    assert_eq!(undo_count(&app, Canvas::Local), 0);
    app.local_adjust_mask_tool = LocalAdjustMaskTool::HorizLine;
    drag(
        &mut app,
        &ctx,
        &transform,
        Canvas::Local,
        [0.3, 0.3],
        [0.7, 0.7],
        2.0,
    );
    let created = shapes(&app, Canvas::Local);
    assert_eq!(created.len(), 1);
    assert_eq!(undo_count(&app, Canvas::Local), 1);
    key(&mut app, &ctx, egui::Key::Z, egui::Modifiers::CTRL);
    assert!(shapes(&app, Canvas::Local).is_empty());
    key(&mut app, &ctx, egui::Key::Y, egui::Modifiers::CTRL);
    assert_eq!(shapes(&app, Canvas::Local), created);
}
