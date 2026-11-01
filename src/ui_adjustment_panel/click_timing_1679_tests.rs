//! Handler-level regression coverage for ordered canvas gestures (?1.679).
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

#[test]
fn section1679_bucket_same_and_split_pass_fill_all_canvases() {
    for canvas in [Canvas::Local, Canvas::Conceal, Canvas::Erase] {
        for same_pass in [true, false] {
            let ctx = egui::Context::default();
            let (mut app, transform) = setup(&ctx);
            // Each mode needs its own ready edit-chain source. An active local
            // layer without its composite correctly blocks conceal's raw source.
            if !matches!(canvas, Canvas::Local) {
                app.local_adjust_mode = false;
                app.local_adjust_page_layers.clear();
            }
            app.conceal_mode = matches!(canvas, Canvas::Conceal);
            app.erase_mode = matches!(canvas, Canvas::Erase);
            app.local_adjust_mask_tool = LocalAdjustMaskTool::Bucket;
            app.conceal_tool = crate::conceal::ConcealTool::Bucket;
            app.conceal_paint_mode = true;
            app.conceal_mask_size = [32, 32];
            app.conceal_mask = Some(vec![false; 32 * 32]);
            app.conceal_bucket_source = crate::mask_db::BucketSource::DrawnMask;
            app.conceal_bucket_leak_stop = 0.0;
            app.conceal_bucket_outset = 0.0;
            app.erase_tool = crate::app::EraseTool::Bucket;
            app.erase_paint_mode = true;
            app.erase_mask_size = [32, 32];
            app.erase_mask = Some(vec![false; 32 * 32]);
            app.erase_bucket_source = crate::mask_db::BucketSource::DrawnMask;
            app.erase_bucket_leak_stop = 0.0;
            app.erase_bucket_outset = 0.0;
            app.erase_base_cache.insert(
                0,
                crate::app::EraseBaseSource::raster(Arc::new(egui::ColorImage::filled(
                    [32, 32],
                    egui::Color32::WHITE,
                ))),
            );
            if matches!(canvas, Canvas::Conceal) {
                assert!(app.current_conceal_source_pixels(0).is_some());
            }
            let pos = transform.viewport_rect.center();
            // No PointerMoved: a stationary mouse/tap already has button positions.
            if same_pass {
                assert_eq!(
                    frame(
                        &mut app,
                        &ctx,
                        &transform,
                        canvas,
                        1.0,
                        vec![button(pos, true), button(pos, false)]
                    ),
                    (true, false, true)
                );
            } else {
                assert_eq!(
                    frame(
                        &mut app,
                        &ctx,
                        &transform,
                        canvas,
                        1.0,
                        vec![button(pos, true)]
                    ),
                    (true, true, false)
                );
                assert_eq!(
                    frame(
                        &mut app,
                        &ctx,
                        &transform,
                        canvas,
                        1.05,
                        vec![button(pos, false)]
                    ),
                    (false, false, true)
                );
            }
            let painted = match canvas {
                Canvas::Local => local_adjust_target_raster_vector_mask_ref(
                    &app.local_adjust_page_layers[&0][0],
                    LocalAdjustMaskEditTarget::Base,
                )
                .unwrap()
                .alpha
                .iter()
                .any(|&v| v > 0.0),
                Canvas::Conceal => app.conceal_mask.as_ref().unwrap().iter().any(|&v| v),
                Canvas::Erase => app.erase_mask.as_ref().unwrap().iter().any(|&v| v),
            };
            assert_eq!(painted, true, "{canvas:?} same_pass={same_pass}");
        }
    }
}

#[test]
fn section1679_rect_uses_release_position_in_every_batching() {
    // All-in-one, press then move+release, and press/move/release in separate passes.
    for batches in [1, 2, 3] {
        let ctx = egui::Context::default();
        let (mut app, transform) = setup(&ctx);
        app.local_adjust_mask_tool = LocalAdjustMaskTool::Rect;
        let start = transform.viewport_rect.center() - egui::vec2(60.0, 60.0);
        let end = start + egui::vec2(120.0, 120.0);
        let press = button(start, true);
        let moved = egui::Event::PointerMoved(end);
        let release = button(end, false);
        let events = match batches {
            1 => vec![vec![press, moved, release]],
            2 => vec![vec![press], vec![moved, release]],
            _ => vec![vec![press], vec![moved], vec![release]],
        };
        for (i, events) in events.into_iter().enumerate() {
            frame(
                &mut app,
                &ctx,
                &transform,
                Canvas::Local,
                1.0 + i as f64 * 0.05,
                events,
            );
        }
        let mask = local_adjust_target_raster_vector_mask_ref(
            &app.local_adjust_page_layers[&0][0],
            LocalAdjustMaskEditTarget::Base,
        )
        .unwrap();
        assert_eq!(mask.shapes.len(), 1, "batches={batches}");
    }
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

fn mask_pixels(app: &App, canvas: Canvas) -> Vec<bool> {
    match canvas {
        Canvas::Local => local_adjust_target_raster_vector_mask_ref(
            &app.local_adjust_page_layers[&0][0],
            LocalAdjustMaskEditTarget::Base,
        )
        .unwrap()
        .alpha
        .iter()
        .map(|&a| a > 0.0)
        .collect(),
        Canvas::Conceal => app.conceal_mask.as_ref().unwrap().clone(),
        Canvas::Erase => app.erase_mask.as_ref().unwrap().clone(),
    }
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

fn run_batches(
    app: &mut App,
    ctx: &egui::Context,
    transform: &DisplayedImageTransform,
    canvas: Canvas,
    events: Vec<egui::Event>,
    batching: usize,
) {
    let batches = match batching {
        1 => vec![events],
        2 => vec![events[..1].to_vec(), events[1..].to_vec()],
        _ => events.into_iter().map(|e| vec![e]).collect(),
    };
    for (i, events) in batches.into_iter().enumerate() {
        frame(app, ctx, transform, canvas, 1.0 + i as f64 * 0.05, events);
    }
}

#[test]
fn section1679_brush_preserves_press_intermediate_and_release_samples_and_one_undo() {
    for canvas in [Canvas::Local, Canvas::Conceal, Canvas::Erase] {
        for batching in [1, 2, 3] {
            for stationary in [true, false] {
                let ctx = egui::Context::default();
                let (mut app, transform) = setup(&ctx);
                prepare_canvas(&mut app, canvas, LocalAdjustMaskTool::Brush);
                let a = at(&transform, [0.25, 0.25]);
                let b = at(&transform, [0.75, 0.25]);
                let c = at(&transform, [0.75, 0.75]);
                let events = if stationary {
                    vec![button(a, true), button(a, false)]
                } else {
                    vec![
                        button(a, true),
                        egui::Event::PointerMoved(b),
                        button(c, false),
                    ]
                };
                run_batches(&mut app, &ctx, &transform, canvas, events, batching);
                let mask = mask_pixels(&app, canvas);
                let (lo, hi) = if matches!(canvas, Canvas::Local) {
                    (7, 23)
                } else {
                    (8, 24)
                };
                assert!(mask[lo * 32 + lo], "{canvas:?} batching={batching}: press");
                if !stationary {
                    assert!(mask[lo * 32 + hi], "{canvas:?} batching={batching}: corner");
                    assert!(
                        mask[hi * 32 + hi],
                        "{canvas:?} batching={batching}: release"
                    );
                    assert!(
                        !mask[16 * 32 + 16],
                        "must not join press directly to release"
                    );
                }
                assert_eq!(
                    undo_count(&app, canvas),
                    1,
                    "{canvas:?} batching={batching}"
                );
                assert!(app.local_adjust_mask_brush_stroke.is_none());
                assert!(app.erase_last_paint_pos.is_none());
                assert!(app.conceal_last_paint_pos.is_none());
            }
        }
    }
}

#[test]
fn section1679_all_shapes_use_the_actual_release_coordinate() {
    for canvas in [Canvas::Local, Canvas::Conceal, Canvas::Erase] {
        for tool in [
            LocalAdjustMaskTool::Rect,
            LocalAdjustMaskTool::Ellipse,
            LocalAdjustMaskTool::Line,
            LocalAdjustMaskTool::VertLine,
            LocalAdjustMaskTool::HorizLine,
        ] {
            let mut expected = None;
            for batching in [3, 2, 1] {
                let ctx = egui::Context::default();
                let (mut app, transform) = setup(&ctx);
                prepare_canvas(&mut app, canvas, tool);
                let a = at(&transform, [0.25, 0.25]);
                let b = at(&transform, [0.5, 0.5]);
                let c = at(&transform, [0.75, 0.75]);
                run_batches(
                    &mut app,
                    &ctx,
                    &transform,
                    canvas,
                    vec![
                        button(a, true),
                        egui::Event::PointerMoved(b),
                        button(c, false),
                    ],
                    batching,
                );
                let actual = shapes(&app, canvas);
                assert_eq!(actual.len(), 1, "{canvas:?} {tool:?} batching={batching}");
                // Verify independent coordinates, then compare exact geometry across batching.
                let span = if matches!(canvas, Canvas::Local) {
                    31.0
                } else {
                    32.0
                };
                match actual[0] {
                    crate::mask_db::Shape::Rect {
                        center,
                        half_w,
                        half_h,
                        ..
                    } => {
                        assert!(
                            (center.0 - span * 0.5).abs() < 0.01
                                && (center.1 - span * 0.5).abs() < 0.01
                        );
                        assert!(
                            (half_w - span * 0.25).abs() < 0.01
                                && (half_h - span * 0.25).abs() < 0.01
                        );
                    }
                    crate::mask_db::Shape::Ellipse { center, rx, ry, .. } => {
                        assert!(
                            (center.0 - span * 0.5).abs() < 0.01
                                && (center.1 - span * 0.5).abs() < 0.01
                        );
                        assert!((rx - span * 0.25).abs() < 0.01 && (ry - span * 0.25).abs() < 0.01);
                    }
                    crate::mask_db::Shape::Line {
                        p1,
                        kind: crate::mask_db::LineKind::Diagonal,
                        ..
                    } => {
                        assert!(
                            (p1.0 - span * 0.75).abs() < 0.01 && (p1.1 - span * 0.75).abs() < 0.01
                        );
                    }
                    _ => {}
                }
                if let Some(expected) = &expected {
                    assert_eq!(&actual, expected);
                } else {
                    expected = Some(actual);
                }
                assert_eq!(undo_count(&app, canvas), 1, "{canvas:?} {tool:?}");
            }
        }
    }
}

#[test]
fn section1679_two_clicks_in_one_pass_each_keep_their_own_position_and_undo() {
    for canvas in [Canvas::Local, Canvas::Conceal, Canvas::Erase] {
        for tool in [
            LocalAdjustMaskTool::Bucket,
            LocalAdjustMaskTool::Brush,
            LocalAdjustMaskTool::Rect,
            LocalAdjustMaskTool::Ellipse,
            LocalAdjustMaskTool::Line,
        ] {
            let ctx = egui::Context::default();
            let (mut app, transform) = setup(&ctx);
            prepare_canvas(&mut app, canvas, tool);
            if tool == LocalAdjustMaskTool::Bucket {
                for y in 0..32 {
                    match canvas {
                        Canvas::Local => {
                            let layer = &mut Arc::make_mut(
                                app.local_adjust_page_layers.get_mut(&0).unwrap(),
                            )[0];
                            let LocalMask::RasterVector(mask) = &mut layer.mask else {
                                unreachable!()
                            };
                            mask.alpha_mut()[y * 32 + 16] = 1.0;
                        }
                        Canvas::Conceal => app.conceal_mask.as_mut().unwrap()[y * 32 + 16] = true,
                        Canvas::Erase => app.erase_mask.as_mut().unwrap()[y * 32 + 16] = true,
                    }
                }
            }
            let a = at(&transform, [0.2, 0.2]);
            let b = at(&transform, [0.4, 0.4]);
            let c = at(&transform, [0.65, 0.65]);
            let d = at(&transform, [0.85, 0.85]);
            let stationary = matches!(
                tool,
                LocalAdjustMaskTool::Bucket | LocalAdjustMaskTool::Brush
            );
            frame(
                &mut app,
                &ctx,
                &transform,
                canvas,
                1.0,
                vec![
                    button(a, true),
                    button(if stationary { a } else { b }, false),
                    button(c, true),
                    button(if stationary { c } else { d }, false),
                ],
            );
            if stationary {
                let mask = mask_pixels(&app, canvas);
                assert!(
                    mask[6 * 32 + 6] && mask[20 * 32 + 20],
                    "{canvas:?} {tool:?}"
                );
                if tool == LocalAdjustMaskTool::Brush {
                    assert!(!mask[16 * 32 + 16]);
                }
            } else {
                assert_eq!(shapes(&app, canvas).len(), 2, "{canvas:?} {tool:?}");
            }
            assert_eq!(undo_count(&app, canvas), 2, "{canvas:?} {tool:?}");
        }
    }
}

#[test]
fn section1679_eyedropper_uses_each_press_without_waiting_for_a_held_pass() {
    for batching in [1, 2, 3] {
        let ctx = egui::Context::default();
        let (mut app, transform) = setup(&ctx);
        let pixels = Arc::new(egui::ColorImage::new(
            [32, 32],
            (0..1024)
                .map(|i| {
                    if i % 32 < 16 {
                        egui::Color32::RED
                    } else {
                        egui::Color32::BLUE
                    }
                })
                .collect(),
        ));
        let crate::fs_animation::FsCacheEntry::Static { pixels: source, .. } =
            app.fs_cache.get_mut(&0).unwrap()
        else {
            unreachable!()
        };
        *source = pixels;
        Arc::make_mut(app.local_adjust_page_layers.get_mut(&0).unwrap())[0].mask =
            LocalMask::ColorRange(Default::default());
        app.local_adjust_mask_edit_target = LocalAdjustMaskEditTarget::None;
        let a = at(&transform, [0.25, 0.5]);
        let b = at(&transform, [0.75, 0.5]);
        // Persistent color-range picker: two presses must sample in order, not
        // the aggregate pointer position twice. Each distinct color is one Undo.
        run_batches(
            &mut app,
            &ctx,
            &transform,
            Canvas::Local,
            vec![
                button(a, true),
                button(a, false),
                button(b, true),
                egui::Event::PointerMoved(a),
                button(b, false),
            ],
            batching,
        );
        let LocalMask::ColorRange(mask) = &app.local_adjust_page_layers[&0][0].mask else {
            unreachable!()
        };
        assert_eq!(mask.target_rgb, [0, 0, 255]);
        assert!(mask.initialized);
        assert_eq!(app.meta_undo.undo_len(), 2);
    }
}

#[test]
fn section1679_panel_press_is_rejected_but_canvas_release_over_panel_finishes() {
    for canvas in [Canvas::Local, Canvas::Conceal, Canvas::Erase] {
        let ctx = egui::Context::default();
        let (mut app, transform) = setup(&ctx);
        prepare_canvas(&mut app, canvas, LocalAdjustMaskTool::Brush);
        let panel = egui::pos2(100.0, 100.0);
        let a = at(&transform, [0.5, 0.5]);
        frame(
            &mut app,
            &ctx,
            &transform,
            canvas,
            1.0,
            vec![
                button(panel, true),
                egui::Event::PointerMoved(a),
                button(a, false),
            ],
        );
        assert!(!mask_pixels(&app, canvas).iter().any(|&v| v));
        frame(
            &mut app,
            &ctx,
            &transform,
            canvas,
            1.1,
            vec![button(a, true), button(panel, false)],
        );
        assert!(mask_pixels(&app, canvas).iter().any(|&v| v));
        assert_eq!(undo_count(&app, canvas), 1);
        assert!(app.local_adjust_mask_brush_stroke.is_none());
        assert!(app.erase_last_paint_pos.is_none());
        assert!(app.conceal_last_paint_pos.is_none());
    }
}

#[test]
fn section1679_old_release_then_new_press_in_one_pass_are_two_brush_gestures() {
    for canvas in [Canvas::Local, Canvas::Conceal, Canvas::Erase] {
        let ctx = egui::Context::default();
        let (mut app, transform) = setup(&ctx);
        prepare_canvas(&mut app, canvas, LocalAdjustMaskTool::Brush);
        let a = at(&transform, [0.25, 0.25]);
        let b = at(&transform, [0.75, 0.25]);
        let c = at(&transform, [0.75, 0.75]);
        frame(
            &mut app,
            &ctx,
            &transform,
            canvas,
            1.0,
            vec![button(a, true)],
        );
        // A held pass without events must leave the same document gesture alive.
        frame(&mut app, &ctx, &transform, canvas, 1.05, vec![]);
        frame(
            &mut app,
            &ctx,
            &transform,
            canvas,
            1.1,
            vec![button(b, false), button(c, true), button(c, false)],
        );
        assert_eq!(undo_count(&app, canvas), 2, "{canvas:?}");
        let mask = mask_pixels(&app, canvas);
        let (lo, hi) = if matches!(canvas, Canvas::Local) {
            (7, 23)
        } else {
            (8, 24)
        };
        assert!(mask[lo * 32 + hi] && mask[hi * 32 + hi]);
        assert!(
            !mask[16 * 32 + hi],
            "separate gestures must not be connected"
        );
    }
}

#[test]
fn section1679_rgb_picker_uses_press_position_and_disarms_once() {
    for batching in [1, 2, 3] {
        let ctx = egui::Context::default();
        let (mut app, transform) = setup(&ctx);
        let crate::fs_animation::FsCacheEntry::Static { pixels, .. } =
            app.fs_cache.get_mut(&0).unwrap()
        else {
            unreachable!()
        };
        *pixels = Arc::new(egui::ColorImage::new(
            [32, 32],
            (0..1024)
                .map(|i| {
                    if i % 32 < 16 {
                        egui::Color32::RED
                    } else {
                        egui::Color32::BLUE
                    }
                })
                .collect(),
        ));
        Arc::make_mut(app.local_adjust_page_layers.get_mut(&0).unwrap())[0].effect =
            LocalEffect::ColorFill(local_adjust_core::ColorFillParams {
                start_rgb: [0, 0, 0],
                ..Default::default()
            });
        app.local_adjust_rgb_pick_active =
            Some(crate::local_adjust_effect_ui::RgbPickTarget::ColorFillStart);
        let a = at(&transform, [0.25, 0.25]);
        let b = at(&transform, [0.75, 0.75]);
        run_batches(
            &mut app,
            &ctx,
            &transform,
            Canvas::Local,
            vec![
                button(a, true),
                egui::Event::PointerMoved(b),
                button(b, false),
            ],
            batching,
        );
        let LocalEffect::ColorFill(params) = &app.local_adjust_page_layers[&0][0].effect else {
            unreachable!()
        };
        assert_eq!(params.start_rgb, [255, 0, 0]);
        assert!(app.local_adjust_rgb_pick_active.is_none());
        assert_eq!(app.meta_undo.undo_len(), 1);
        assert!(!mask_pixels(&app, Canvas::Local).iter().any(|&v| v));
    }
}

#[test]
fn section1679_cancel_does_not_commit_unfinished_shapes_or_restart_on_held_move() {
    for canvas in [Canvas::Local, Canvas::Conceal, Canvas::Erase] {
        for terminal in [egui::Event::PointerGone, egui::Event::WindowFocused(false)] {
            let ctx = egui::Context::default();
            let (mut app, transform) = setup(&ctx);
            prepare_canvas(&mut app, canvas, LocalAdjustMaskTool::Rect);
            let a = at(&transform, [0.25, 0.25]);
            let b = at(&transform, [0.75, 0.75]);
            frame(
                &mut app,
                &ctx,
                &transform,
                canvas,
                1.0,
                vec![button(a, true)],
            );
            frame(
                &mut app,
                &ctx,
                &transform,
                canvas,
                1.1,
                vec![egui::Event::PointerMoved(b), terminal],
            );
            frame(
                &mut app,
                &ctx,
                &transform,
                canvas,
                1.2,
                vec![egui::Event::PointerMoved(b), button(b, false)],
            );
            assert!(shapes(&app, canvas).is_empty(), "{canvas:?}");
            assert_eq!(undo_count(&app, canvas), 0);
        }
    }
}

#[cfg(windows)]
fn keyboard_frame(
    app: &mut App,
    ctx: &egui::Context,
    transform: &DisplayedImageTransform,
    canvas: Canvas,
    time: f64,
    events: Vec<egui::Event>,
) {
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
                let _owner = app.keyboard_owner_for_pass(ctx);
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
}

#[cfg(windows)]
fn set_canvas_space_hold(held: bool, scrub: bool) {
    use crate::key_input::{SyntheticKeyCommand, SyntheticModifiers, SyntheticNavigationKey};
    let now = std::time::Instant::now();
    // Replace only the fake keyboard level; no native input is sent.
    crate::key_input::enqueue_test_synthetic_command(SyntheticKeyCommand::cancel_all(now));
    if held {
        crate::key_input::enqueue_test_synthetic_command(SyntheticKeyCommand::down(
            now,
            SyntheticNavigationKey::Space,
            SyntheticModifiers {
                ctrl: scrub,
                ..Default::default()
            },
        ));
    }
    crate::key_input::advance_test_synthetic_input(now);
}

#[cfg(windows)]
#[derive(Clone, Copy, Debug)]
enum CancelledGesture {
    Drawing,
    Pan,
    Scrub,
    ScrubAwaitRelease,
}

#[cfg(windows)]
#[test]
fn section1679_cancel_retires_drawing_pan_and_scrub_then_accepts_fresh_gestures() {
    // KeyHold must pass the same App keyboard ownership gate as production.
    use self::keyboard_frame as frame;
    let _serial = crate::key_input::lock_test_input();
    for canvas in [Canvas::Local, Canvas::Conceal, Canvas::Erase] {
        for gesture in [
            CancelledGesture::Drawing,
            CancelledGesture::Pan,
            CancelledGesture::Scrub,
            CancelledGesture::ScrubAwaitRelease,
        ] {
            for terminal in [
                Some(egui::Event::PointerGone),
                Some(egui::Event::WindowFocused(false)),
                None, // Tool change on the same page also retires the old gesture.
            ] {
                let ctx = egui::Context::default();
                let (mut app, transform) = setup(&ctx);
                prepare_canvas(&mut app, canvas, LocalAdjustMaskTool::Rect);
                crate::key_input::arm_test_synthetic_input(0xE679, egui::ViewportId::ROOT);
                let scrub = matches!(
                    gesture,
                    CancelledGesture::Scrub | CancelledGesture::ScrubAwaitRelease
                );
                set_canvas_space_hold(!matches!(gesture, CancelledGesture::Drawing), scrub);
                let a = at(&transform, [0.4, 0.4]);
                // Shape size is measured in source pixels, not screen points.
                let drawing_end = at(&transform, [0.75, 0.75]);
                let b = if matches!(gesture, CancelledGesture::Drawing) {
                    drawing_end
                } else {
                    a + egui::vec2(40.0, 20.0)
                };
                frame(
                    &mut app,
                    &ctx,
                    &transform,
                    canvas,
                    1.0,
                    vec![button(a, true)],
                );
                frame(
                    &mut app,
                    &ctx,
                    &transform,
                    canvas,
                    1.1,
                    vec![egui::Event::PointerMoved(b)],
                );
                match gesture {
                    CancelledGesture::Drawing => assert!(shapes(&app, canvas).is_empty()),
                    CancelledGesture::Pan => {
                        assert!(app.fs_pan_drag_start.is_some(), "{canvas:?}");
                        assert_eq!(app.fs_pan, b - a, "{canvas:?}: pan must actually move");
                    }
                    CancelledGesture::Scrub | CancelledGesture::ScrubAwaitRelease => {
                        assert!(app.fs_middle_zoom_drag.is_some(), "{canvas:?}");
                        assert!(app.fs_zoom > 1.0, "{canvas:?}: scrub must actually zoom");
                    }
                }
                if matches!(gesture, CancelledGesture::ScrubAwaitRelease) {
                    set_canvas_space_hold(false, false);
                    frame(&mut app, &ctx, &transform, canvas, 1.15, vec![]);
                    assert!(
                        app.fs_middle_zoom_drag.is_some(),
                        "chord release retains primary ownership"
                    );
                }
                let view = (app.fs_zoom, app.fs_pan);
                if terminal.is_none() {
                    app.local_adjust_mask_tool = LocalAdjustMaskTool::Ellipse;
                    app.conceal_tool = crate::conceal::ConcealTool::Ellipse;
                    app.erase_tool = crate::app::EraseTool::Ellipse;
                }
                frame(
                    &mut app,
                    &ctx,
                    &transform,
                    canvas,
                    1.2,
                    terminal.into_iter().collect(),
                );
                assert!(app.fs_pan_drag_start.is_none(), "{canvas:?} {gesture:?}");
                assert!(app.fs_middle_zoom_drag.is_none(), "{canvas:?} {gesture:?}");
                assert_eq!(
                    (app.fs_zoom, app.fs_pan),
                    view,
                    "Cancel must preserve the last applied view"
                );
                assert!(shapes(&app, canvas).is_empty());
                assert_eq!(undo_count(&app, canvas), 0);

                // The owner is Idle: the late release must not be needed for
                // cleanup. For Pan, Space stays held across Cancel and release.
                frame(
                    &mut app,
                    &ctx,
                    &transform,
                    canvas,
                    1.3,
                    vec![egui::Event::WindowFocused(true), button(b, false)],
                );
                set_canvas_space_hold(true, false);
                let c = at(&transform, [0.6, 0.6]);
                let d = c + egui::vec2(12.0, 8.0);
                frame(
                    &mut app,
                    &ctx,
                    &transform,
                    canvas,
                    1.4,
                    vec![button(c, true)],
                );
                assert_eq!(
                    app.fs_pan, view.1,
                    "{canvas:?} {gesture:?}: new press jumped"
                );
                assert_eq!(app.fs_pan_drag_start, Some((c, view.1)));
                frame(
                    &mut app,
                    &ctx,
                    &transform,
                    canvas,
                    1.5,
                    vec![button(d, false)],
                );
                assert_eq!(
                    app.fs_pan,
                    view.1 + (d - c),
                    "{canvas:?} {gesture:?}: stale pan origin"
                );
                assert!(app.fs_pan_drag_start.is_none());
                assert!(app.fs_middle_zoom_drag.is_none());
                assert!(shapes(&app, canvas).is_empty(), "pan must not draw");

                // A fresh zoom must likewise capture a new pivot and starting view.
                set_canvas_space_hold(true, true);
                let zoom_before = app.fs_zoom;
                frame(
                    &mut app,
                    &ctx,
                    &transform,
                    canvas,
                    1.6,
                    vec![button(c, true)],
                );
                assert_eq!(app.fs_zoom, zoom_before);
                frame(
                    &mut app,
                    &ctx,
                    &transform,
                    canvas,
                    1.7,
                    vec![button(c + egui::vec2(100.0, 0.0), false)],
                );
                assert!((app.fs_zoom - zoom_before * 2.0).abs() < 1e-5);
                assert!(app.fs_middle_zoom_drag.is_none());

                set_canvas_space_hold(false, false);
                frame(
                    &mut app,
                    &ctx,
                    &transform,
                    canvas,
                    1.8,
                    vec![button(a, true), button(drawing_end, false)],
                );
                assert_eq!(
                    shapes(&app, canvas).len(),
                    1,
                    "{canvas:?} {gesture:?}: fresh drawing"
                );
                assert_eq!(undo_count(&app, canvas), 1);
            }
        }
    }
}

#[test]
fn section1679_discard_and_duplicate_dispatch_do_not_repeat_canvas_edits() {
    for canvas in [Canvas::Local, Canvas::Conceal, Canvas::Erase] {
        let ctx = egui::Context::default();
        let (mut app, transform) = setup(&ctx);
        prepare_canvas(&mut app, canvas, LocalAdjustMaskTool::Brush);
        let pos = at(&transform, [0.5, 0.5]);
        let mut visits = 0;
        let _ = ctx
            .run_pass(
                egui::RawInput {
                    screen_rect: Some(transform.viewport_rect),
                    time: Some(1.0),
                    events: vec![button(pos, true), button(pos, false)],
                    ..Default::default()
                },
                |ctx| {
                    visits += 1;
                    for _ in 0..2 {
                        match canvas {
                            Canvas::Local => app.handle_local_adjust_canvas_input(
                                ctx,
                                transform.viewport_rect,
                                &transform,
                                Default::default(),
                            ),
                            Canvas::Conceal => {
                                app.handle_conceal_paint(ctx, &transform, Default::default())
                            }
                            Canvas::Erase => {
                                app.handle_erase_paint(ctx, &transform, Default::default())
                            }
                        }
                    }
                    if ctx.current_pass_index() == 0 {
                        ctx.request_discard("canvas regression");
                    }
                },
            )
            .drop_without_applying_deltas();
        assert_eq!(visits, 2);
        assert_eq!(undo_count(&app, canvas), 1, "{canvas:?}");
    }
}

#[test]
fn section1679_stationary_shapes_preserve_the_existing_split_pass_contract() {
    for canvas in [Canvas::Local, Canvas::Conceal, Canvas::Erase] {
        for tool in [
            LocalAdjustMaskTool::Rect,
            LocalAdjustMaskTool::Ellipse,
            LocalAdjustMaskTool::Line,
        ] {
            for batching in [1, 2] {
                let ctx = egui::Context::default();
                let (mut app, transform) = setup(&ctx);
                prepare_canvas(&mut app, canvas, tool);
                let a = at(&transform, [0.5, 0.5]);
                run_batches(
                    &mut app,
                    &ctx,
                    &transform,
                    canvas,
                    vec![button(a, true), button(a, false)],
                    batching,
                );
                // Conceal's existing zero-length line is retained; the other
                // shape tools reject their existing minimum-size degeneracies.
                let count = usize::from(
                    matches!(canvas, Canvas::Conceal) && tool == LocalAdjustMaskTool::Line,
                );
                assert_eq!(shapes(&app, canvas).len(), count);
                assert_eq!(undo_count(&app, canvas), count);
            }
        }
    }
}

#[test]
fn section1679_lasso_includes_all_ordered_samples() {
    for canvas in [Canvas::Local, Canvas::Conceal, Canvas::Erase] {
        for batching in [1, 2, 3] {
            let ctx = egui::Context::default();
            let (mut app, transform) = setup(&ctx);
            prepare_canvas(&mut app, canvas, LocalAdjustMaskTool::Lasso);
            let a = at(&transform, [0.25, 0.25]);
            let b = at(&transform, [0.75, 0.25]);
            let c = at(&transform, [0.75, 0.75]);
            run_batches(
                &mut app,
                &ctx,
                &transform,
                canvas,
                vec![
                    button(a, true),
                    egui::Event::PointerMoved(b),
                    button(c, false),
                ],
                batching,
            );
            assert!(
                mask_pixels(&app, canvas)[12 * 32 + 18],
                "{canvas:?} batching={batching}"
            );
            assert_eq!(undo_count(&app, canvas), 1);
        }
    }
}

#[test]
fn section1679_front_click_then_canvas_click_in_one_pass_preserves_event_ownership() {
    for canvas in [Canvas::Local, Canvas::Conceal, Canvas::Erase] {
        let ctx = egui::Context::default();
        let (mut app, transform) = setup(&ctx);
        prepare_canvas(&mut app, canvas, LocalAdjustMaskTool::Brush);
        let a = at(&transform, [0.25, 0.25]);
        let b = at(&transform, [0.75, 0.75]);
        let front_rect = egui::Rect::from_center_size(a, egui::vec2(20.0, 20.0));
        let mut front_owner = crate::ui_fullscreen::FsFrontPointerOwnership::default();
        let _ = ctx
            .run_pass(
                egui::RawInput {
                    screen_rect: Some(transform.viewport_rect),
                    time: Some(1.0),
                    events: vec![
                        button(a, true),
                        button(a, false),
                        button(b, true),
                        button(b, false),
                    ],
                    ..Default::default()
                },
                |ctx| {
                    let front = front_owner.capture_canvas_for_test(ctx, front_rect);
                    match canvas {
                        Canvas::Local => app.handle_local_adjust_canvas_input(
                            ctx,
                            transform.viewport_rect,
                            &transform,
                            front,
                        ),
                        Canvas::Conceal => app.handle_conceal_paint(ctx, &transform, front),
                        Canvas::Erase => app.handle_erase_paint(ctx, &transform, front),
                    }
                },
            )
            .drop_without_applying_deltas();
        let mask = mask_pixels(&app, canvas);
        let (lo, hi) = if matches!(canvas, Canvas::Local) {
            (7, 23)
        } else {
            (8, 24)
        };
        assert!(!mask[lo * 32 + lo] && mask[hi * 32 + hi], "{canvas:?}");
        assert_eq!(undo_count(&app, canvas), 1);
    }
}

#[test]
fn section1679_tool_change_and_generation_change_do_not_reown_a_held_press() {
    for generation_change in [false, true] {
        let ctx = egui::Context::default();
        let (mut app, transform) = setup(&ctx);
        prepare_canvas(&mut app, Canvas::Local, LocalAdjustMaskTool::Brush);
        let a = at(&transform, [0.25, 0.25]);
        let b = at(&transform, [0.75, 0.75]);
        frame(
            &mut app,
            &ctx,
            &transform,
            Canvas::Local,
            1.0,
            vec![button(a, true)],
        );
        if generation_change {
            app.fold_local_adjust_edit_state();
            app.items_generation += 1;
        } else {
            app.set_local_adjust_mask_tool_from_shortcut(LocalAdjustMaskTool::Rect);
        }
        frame(
            &mut app,
            &ctx,
            &transform,
            Canvas::Local,
            1.1,
            vec![egui::Event::PointerMoved(b), button(b, false)],
        );
        assert!(shapes(&app, Canvas::Local).is_empty());
        let mask = mask_pixels(&app, Canvas::Local);
        assert!(mask[7 * 32 + 7] && !mask[23 * 32 + 23]);
        assert_eq!(app.meta_undo.undo_len(), 1);
    }
}

#[test]
fn section1679_bucket_uses_press_seed_even_when_move_and_release_follow_in_the_same_pass() {
    for canvas in [Canvas::Local, Canvas::Conceal, Canvas::Erase] {
        for batching in [1, 2, 3] {
            let ctx = egui::Context::default();
            let (mut app, transform) = setup(&ctx);
            prepare_canvas(&mut app, canvas, LocalAdjustMaskTool::Bucket);
            for y in 0..32 {
                match canvas {
                    Canvas::Local => {
                        let layer =
                            &mut Arc::make_mut(app.local_adjust_page_layers.get_mut(&0).unwrap())
                                [0];
                        let LocalMask::RasterVector(mask) = &mut layer.mask else {
                            unreachable!()
                        };
                        mask.alpha_mut()[y * 32 + 16] = 1.0;
                    }
                    Canvas::Conceal => app.conceal_mask.as_mut().unwrap()[y * 32 + 16] = true,
                    Canvas::Erase => app.erase_mask.as_mut().unwrap()[y * 32 + 16] = true,
                }
            }
            let a = at(&transform, [0.25, 0.5]);
            let b = at(&transform, [0.75, 0.5]);
            run_batches(
                &mut app,
                &ctx,
                &transform,
                canvas,
                vec![
                    button(a, true),
                    egui::Event::PointerMoved(b),
                    button(b, false),
                ],
                batching,
            );
            let mask = mask_pixels(&app, canvas);
            assert!(
                mask[16 * 32 + 8] && !mask[16 * 32 + 24],
                "{canvas:?} batching={batching}"
            );
            assert_eq!(undo_count(&app, canvas), 1);
        }
    }
}

#[cfg(windows)]
#[test]
fn section1679_short_brush_rejects_simultaneous_preview_hold_before_release() {
    let _input = crate::key_input::lock_test_input();
    for canvas in [Canvas::Conceal, Canvas::Erase] {
        crate::key_input::arm_test_synthetic_input(0xE679, egui::ViewportId::ROOT);
        let now = std::time::Instant::now();
        crate::key_input::enqueue_test_synthetic_command(
            crate::key_input::SyntheticKeyCommand::down(
                now,
                crate::key_input::SyntheticNavigationKey::Tab,
                Default::default(),
            ),
        );
        crate::key_input::advance_test_synthetic_input(now);
        let ctx = egui::Context::default();
        crate::egui_focus_policy::install_tab_shortcut_focus_policy(&ctx);
        let (mut app, transform) = setup(&ctx);
        prepare_canvas(&mut app, canvas, LocalAdjustMaskTool::Brush);
        let p = at(&transform, [0.5, 0.5]);
        let _ = ctx
            .run_pass(
                egui::RawInput {
                    screen_rect: Some(transform.viewport_rect),
                    events: vec![button(p, true), button(p, false)],
                    ..Default::default()
                },
                |ctx| {
                    app.keyboard_owner_for_pass(ctx);
                    match canvas {
                        Canvas::Conceal => {
                            app.handle_conceal_paint(ctx, &transform, Default::default())
                        }
                        Canvas::Erase => {
                            app.handle_erase_paint(ctx, &transform, Default::default())
                        }
                        Canvas::Local => unreachable!(),
                    }
                },
            )
            .drop_without_applying_deltas();
        let hold = match canvas {
            Canvas::Conceal => app.conceal_preview_hold,
            Canvas::Erase => app.erase_preview_hold,
            Canvas::Local => unreachable!(),
        };
        assert_eq!(
            hold,
            crate::edit_preview::EditPreviewHoldState::BlockedUntilRelease,
            "{canvas:?}"
        );
        assert_eq!(undo_count(&app, canvas), 1);
        assert!(mask_pixels(&app, canvas).iter().any(|&v| v));
        assert!(
            !app.fs_primary_suppression.is_active(),
            "a completed short gesture must not latch suppression"
        );
    }
}

#[test]
fn section1679_same_page_reprojection_preserves_the_accepted_brush_gesture() {
    let ctx = egui::Context::default();
    let (mut app, transform) = setup(&ctx);
    prepare_canvas(&mut app, Canvas::Local, LocalAdjustMaskTool::Brush);
    let a = at(&transform, [0.25, 0.25]);
    let b = at(&transform, [0.75, 0.75]);
    frame(
        &mut app,
        &ctx,
        &transform,
        Canvas::Local,
        1.0,
        vec![button(a, true)],
    );
    // Same target/draft, new list generation. Existing page-edit ownership
    // deliberately preserves this interaction across a read-only reprojection.
    app.items_generation += 1;
    app.reproject_page_edit_owners();
    frame(
        &mut app,
        &ctx,
        &transform,
        Canvas::Local,
        1.1,
        vec![egui::Event::PointerMoved(b), button(b, false)],
    );
    let mask = mask_pixels(&app, Canvas::Local);
    assert!(mask[7 * 32 + 7] && mask[23 * 32 + 23]);
    assert!(app.local_adjust_mask_brush_stroke.is_none());
    assert_eq!(app.meta_undo.undo_len(), 1);
}

#[test]
fn section1679_other_viewport_hover_cannot_update_or_finish_the_owned_handle_drag() {
    for canvas in [Canvas::Local, Canvas::Conceal, Canvas::Erase] {
        let ctx = egui::Context::default();
        let (mut app, transform) = setup(&ctx);
        prepare_canvas(&mut app, canvas, LocalAdjustMaskTool::Rect);
        let start = at(&transform, [0.2, 0.2]);
        let end = at(&transform, [0.8, 0.8]);
        frame(
            &mut app,
            &ctx,
            &transform,
            canvas,
            1.0,
            vec![button(start, true), button(end, false)],
        );
        match canvas {
            Canvas::Local => {
                app.set_local_adjust_mask_tool_from_shortcut(LocalAdjustMaskTool::Select)
            }
            Canvas::Conceal => app.conceal_tool = crate::conceal::ConcealTool::Select,
            Canvas::Erase => app.erase_tool = crate::app::EraseTool::Select,
        }
        let center = at(&transform, [0.5, 0.5]);
        frame(
            &mut app,
            &ctx,
            &transform,
            canvas,
            1.1,
            vec![button(center, true)],
        );
        let before = shapes(&app, canvas);
        let undo_before = undo_count(&app, canvas);
        let other = egui::ViewportId::from_hash_of("section1679-other-canvas");
        let mut raw = egui::RawInput {
            viewport_id: other,
            screen_rect: Some(transform.viewport_rect),
            time: Some(1.15),
            events: vec![egui::Event::PointerMoved(center + egui::vec2(100.0, 0.0))],
            ..Default::default()
        };
        raw.viewports.insert(other, Default::default());
        let _ = ctx
            .run_pass(raw, |ctx| {
                assert_eq!(ctx.viewport_id(), other);
                match canvas {
                    Canvas::Local => app.handle_local_adjust_canvas_input(
                        ctx,
                        transform.viewport_rect,
                        &transform,
                        Default::default(),
                    ),
                    Canvas::Conceal => {
                        app.handle_conceal_paint(ctx, &transform, Default::default())
                    }
                    Canvas::Erase => app.handle_erase_paint(ctx, &transform, Default::default()),
                }
            })
            .drop_without_applying_deltas();
        assert_eq!(
            shapes(&app, canvas),
            before,
            "{canvas:?}: unowned hover changed geometry"
        );
        assert_eq!(undo_count(&app, canvas), undo_before);
        match canvas {
            Canvas::Local => assert!(app.local_adjust_shape_drag.is_some()),
            Canvas::Conceal => assert!(app.conceal_drag.is_some()),
            Canvas::Erase => assert!(app.erase_drag.is_some()),
        }
        let finish = center + egui::vec2(50.0, 0.0);
        frame(
            &mut app,
            &ctx,
            &transform,
            canvas,
            1.2,
            vec![button(finish, false)],
        );
        assert_ne!(
            shapes(&app, canvas),
            before,
            "{canvas:?}: owning release did not update geometry"
        );
        assert_eq!(undo_count(&app, canvas), 2);
        match canvas {
            Canvas::Local => assert!(app.local_adjust_shape_drag.is_none()),
            Canvas::Conceal => assert!(app.conceal_drag.is_none()),
            Canvas::Erase => assert!(app.erase_drag.is_none()),
        }
    }
}
