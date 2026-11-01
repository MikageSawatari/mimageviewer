//! Investigation witnesses: these assert the current defect, not the desired behavior.
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
fn section1679_bucket_same_pass_loses_local_press_but_split_pass_and_siblings_fill() {
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
            assert_eq!(
                painted,
                !matches!(canvas, Canvas::Local) || !same_pass,
                "{canvas:?} same_pass={same_pass}"
            );
        }
    }
}

#[test]
fn section1679_rect_release_batch_ignores_final_move_without_held_move_pass() {
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
        assert_eq!(
            mask.shapes.len(),
            usize::from(batches == 3),
            "batches={batches}"
        );
    }
}
