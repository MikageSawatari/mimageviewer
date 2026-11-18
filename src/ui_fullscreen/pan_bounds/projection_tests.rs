use super::*;
use crate::rotation_db::Rotation;

fn layout(
    gap: f32,
    dpi: f32,
    zoom: f32,
    pan: egui::Vec2,
    sizes: [egui::Vec2; 2],
    trim: bool,
    rotated: bool,
    fit: FullscreenFitMode,
) -> FullscreenPageLayout {
    let viewport = egui::Rect::from_min_size(egui::pos2(0.6, 0.3), egui::vec2(1000.0, 1000.0));
    let rotations = [
        Rotation::None,
        if rotated {
            Rotation::Cw90
        } else {
            Rotation::None
        },
    ];
    let bounds = [if trim {
        egui::Rect::from_min_max(egui::pos2(0.125, 0.16), egui::pos2(0.875, 0.91))
    } else {
        egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0))
    }; 2];
    let geometry = spread_layout_geometry(
        rotated_display_size(sizes[0], rotations[0]),
        rotated_display_size(sizes[1], rotations[1]),
        bounds[0],
        bounds[1],
        fit != FullscreenFitMode::Original,
    );
    let gap = quantize_points_to_physical_pixels(gap, dpi);
    let fit_scale = match fit {
        FullscreenFitMode::Height => viewport.height() / geometry.fit_size.y,
        FullscreenFitMode::Original => physical_pixel_scale(dpi),
        _ => (viewport.width() - gap) / geometry.fit_size.x,
    };
    let scale = fit_scale * zoom;
    let rects = layout_spread_page_rects(
        viewport.center() + pan - geometry.content_center_offset * scale,
        geometry,
        scale,
        gap,
        dpi,
        bounds[0],
        bounds[1],
        trim,
    );
    let mut pages = [rects.left_rect, rects.right_rect].map(|rect| {
        let n = usize::from(rect == rects.right_rect);
        resolve_fs_transform_in_layout_rect(
            DisplayedImageTransformInput {
                page_idx: n,
                viewport_rect: viewport,
                source_size: sizes[n],
                texture_size: sizes[n],
                rotation: rotations[n],
                free_rotation_rad: 0.0,
                content_bbox: trim.then_some(bounds[n]),
                fit_mode: FullscreenFitInput::Legacy(fit),
                fit_scale_limits: Default::default(),
                pixels_per_point: dpi,
                placement: ResolvedDisplayPlacement::Normal {
                    zoom_pan: Some((zoom, pan)),
                },
                pixel_fit: RectPixelFit::Texels,
            },
            rect,
        )
        .unwrap()
    });
    let offsets = align_spread_pages_for_gap(&pages[0], &pages[1], (gap * dpi).round(), dpi);
    pages[0] = pages[0].translated_by(offsets.0);
    pages[1] = pages[1].translated_by(offsets.1);
    let mut layout = FullscreenPageLayout::default();
    layout.begin(FullscreenPageLayoutKind::Spread);
    for (n, transform) in pages.into_iter().enumerate() {
        layout.push_spread_occurrence(
            transform,
            SpreadPageOccurrence::navigation(n, 0),
            if n == 0 {
                SingletonSpreadPlacement::Left
            } else {
                SingletonSpreadPlacement::Right
            },
            rects.spine.1,
        );
    }
    layout
}

fn projected(
    page: &crate::displayed_image_transform::DisplayedPage,
    zoom: f32,
    pan: egui::Vec2,
    corrected: bool,
) -> egui::Rect {
    let t = page.transform;
    let (base_zoom, base_pan) = normal_zoom_pan(t).unwrap();
    let ratio = zoom / base_zoom;
    let (side, gap) = page.spread_gap.unwrap();
    let correction = if corrected {
        spread_projected_gap_correction(side, gap, ratio)
    } else {
        egui::Vec2::ZERO
    };
    let rect = fs_navigator_source_rect_aabb(&t, t.uv_rect);
    egui::Rect::from_min_max(
        t.viewport_rect.center()
            + pan
            + (rect.min - (t.viewport_rect.center() + base_pan)) * ratio
            + correction,
        t.viewport_rect.center()
            + pan
            + (rect.max - (t.viewport_rect.center() + base_pan)) * ratio
            + correction,
    )
}

fn error(
    before: &FullscreenPageLayout,
    after: &FullscreenPageLayout,
    zoom: f32,
    pan: egui::Vec2,
    corrected: bool,
) -> f32 {
    before
        .displayed_pages()
        .zip(after.displayed_pages())
        .map(|(a, b)| {
            let a = projected(a, zoom, pan, corrected);
            let b = fs_navigator_source_rect_aabb(&b.transform, b.transform.uv_rect);
            (a.min - b.min)
                .abs()
                .max_elem()
                .max((a.max - b.max).abs().max_elem())
        })
        .fold(0.0_f32, f32::max)
}

#[test]
fn spread_gap_projection_matches_final_transforms_and_edge_stop_with_zero_gap_error_budget() {
    for dpi in [1.0, 1.25, 1.5] {
        for (z0, z1) in [(1.0, 2.0), (2.0, 1.0), (1.0, 4.0)] {
            for sizes in [
                [egui::vec2(1009.0, 10000.0); 2],
                [egui::vec2(529.0, 1000.0), egui::vec2(351.0, 1000.0)],
            ] {
                for trim in [false, true] {
                    for rotated in [false, true] {
                        for fit in [FullscreenFitMode::Height, FullscreenFitMode::Original] {
                            let pan = egui::vec2(0.6, 0.35);
                            let mut errors = [0.0; 2];
                            let mut stops = [0.0; 2];
                            for (n, gap) in [0.0, 200.0].into_iter().enumerate() {
                                let a = layout(gap, dpi, z0, pan, sizes, trim, rotated, fit);
                                let b = layout(gap, dpi, z1, pan, sizes, trim, rotated, fit);
                                errors[n] = error(&a, &b, z1, pan, true) * dpi;
                                let target = egui::vec2(100000.0, -100000.0);
                                let projected_stop =
                                    clamp_fullscreen_pan_from_layout(&a, z1, target);
                                let actual_stop = clamp_fullscreen_pan_from_layout(&b, z1, target);
                                stops[n] = (projected_stop - actual_stop).abs().max_elem() * dpi;
                            }
                            assert!(
                                errors[1] <= errors[0] + 1.01,
                                "gap error {errors:?}, dpi={dpi}, z={z0}->{z1}, {fit:?}, {trim} {rotated}"
                            );
                            assert!(stops[1] <= stops[0] + 1.01, "edge stop {stops:?}");
                        }
                    }
                }
            }
        }
    }
}

#[test]
fn width_fit_projection_moves_by_signed_gap_correction_and_reduces_error() {
    let sizes = [egui::vec2(529.0, 1000.0), egui::vec2(351.0, 1000.0)];
    for (z0, z1) in [(1.0, 2.0), (2.0, 1.0), (1.0, 4.0)] {
        let pan = egui::vec2(0.6, 0.0);
        let a = layout(
            200.0,
            1.0,
            z0,
            pan,
            sizes,
            false,
            false,
            FullscreenFitMode::Width,
        );
        let b = layout(
            200.0,
            1.0,
            z1,
            pan,
            sizes,
            false,
            false,
            FullscreenFitMode::Width,
        );
        for (n, page) in a.displayed_pages().enumerate() {
            let delta = projected(page, z1, pan, true).min - projected(page, z1, pan, false).min;
            let sign = if n == 0 { 1.0 } else { -1.0 };
            assert!((delta.x - sign * 100.0 * (z1 / z0 - 1.0)).abs() < 0.01);
            assert_eq!(delta.y, 0.0);
        }
        assert!(error(&a, &b, z1, pan, true) < error(&a, &b, z1, pan, false));
    }
}

#[test]
fn singleton_and_two_page_projection_share_the_fixed_gap_formula() {
    for side in [
        SingletonSpreadPlacement::Left,
        SingletonSpreadPlacement::Right,
        SingletonSpreadPlacement::LeftWhite,
        SingletonSpreadPlacement::RightWhite,
    ] {
        assert_eq!(
            spread_projected_gap_correction(side, 200.0, 2.0).x,
            if side.side() == SingletonSpreadPlacement::Left {
                100.0
            } else {
                -100.0
            }
        );
    }
    assert_eq!(
        spread_projected_gap_correction(SingletonSpreadPlacement::Center, 200.0, 2.0),
        egui::Vec2::ZERO
    );
}

#[test]
fn asymmetric_and_tiny_spreads_keep_input_clamp_results_on_complete_draw() {
    for sizes in [
        [egui::vec2(20.0, 40.0), egui::vec2(11.0, 35.0)],
        [egui::vec2(529.0, 1000.0), egui::vec2(351.0, 1000.0)],
    ] {
        for dpi in [1.0, 1.25, 1.5] {
            for gap in [0.0, 12.0, 200.0] {
                for zoom in [0.1, 0.2, 1.0, 2.0] {
                    let before = layout(
                        gap,
                        dpi,
                        1.0,
                        egui::Vec2::ZERO,
                        sizes,
                        false,
                        false,
                        FullscreenFitMode::Original,
                    );
                    for target in [egui::vec2(10000.0, 0.0), egui::vec2(-10000.0, 10000.0)] {
                        // Drag, wheel zoom, pinch and page scroll all finish at
                        // this input boundary. A draw accepts all its candidates.
                        let pan = clamp_fullscreen_pan_from_layout(&before, zoom, target);
                        let drawn = layout(
                            gap,
                            dpi,
                            zoom,
                            pan,
                            sizes,
                            false,
                            false,
                            FullscreenFitMode::Original,
                        );
                        let pages: Vec<_> =
                            drawn.displayed_pages().map(|page| page.transform).collect();
                        assert_eq!(
                            draw_pan_correction(&pages),
                            egui::Vec2::ZERO,
                            "sizes={sizes:?}, dpi={dpi}, gap={gap}, zoom={zoom}, pan={pan:?}"
                        );
                    }
                }
            }
        }
    }
}
