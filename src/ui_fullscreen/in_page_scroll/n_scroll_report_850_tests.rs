//! §1.850 (5ch >>857): N-order reading on a zoomed spread with a small step.
//! Records the pan sequence under the reported conditions and separates the
//! specified behaviour (the wrap reuses the reading step) from a defect
//! (revisiting a position, moving backwards, or never stopping).
use super::*;
use crate::rotation_db::Rotation;
use crate::settings::FullscreenNScrollOrder::{HorizontalFirst, VerticalFirst};

const VIEWPORT: Vec2 = Vec2::new(1920.0, 1080.0);

fn viewport() -> Rect {
    Rect::from_min_size(egui::pos2(0.0, 0.0), VIEWPORT)
}

/// Two portrait pages side by side, fitted to the viewport height at zoom 1,
/// then drawn at `zoom` around the viewport center (pan 0).
fn zoomed_spread(height_over_width: f32, zoom: f32, gap: f32) -> FullscreenPageLayout {
    let page_h = VIEWPORT.y * zoom;
    let page_w = page_h / height_over_width;
    let center = viewport().center();
    let top = center.y - page_h / 2.0;
    let left_min = center.x - page_w - gap / 2.0;
    let rects = [
        Rect::from_min_size(egui::pos2(left_min, top), Vec2::new(page_w, page_h)),
        Rect::from_min_size(
            egui::pos2(center.x + gap / 2.0, top),
            Vec2::new(page_w, page_h),
        ),
    ];
    let pages: Vec<_> = rects
        .iter()
        .enumerate()
        .map(|(idx, rect)| {
            let input = DisplayedImageTransformInput {
                page_idx: idx,
                viewport_rect: viewport(),
                source_size: rect.size() / zoom,
                texture_size: rect.size() / zoom,
                rotation: Rotation::None,
                free_rotation_rad: 0.0,
                content_bbox: None,
                fit_mode: FullscreenFitInput::legacy(FullscreenFitMode::Original),
                fit_scale_limits: FullscreenFitScaleLimits::default(),
                pixels_per_point: 1.0,
                placement: ResolvedDisplayPlacement::Normal {
                    zoom_pan: Some((zoom, Vec2::ZERO)),
                },
                pixel_fit: RectPixelFit::Proportional,
            };
            DisplayedImageTransform::from_resolved_rect(input, *rect).unwrap()
        })
        .collect();
    let mut layout = FullscreenPageLayout::default();
    layout.begin(FullscreenPageLayoutKind::Spread);
    for page in pages {
        layout.push(page);
    }
    layout
}

#[derive(Debug, Default)]
struct Walk {
    pans: Vec<Vec2>,
    wraps: usize,
    /// Secondary-axis distance of each wrap, as a fraction of the viewport.
    wrap_moves: Vec<f32>,
}

/// Steps "next" (or "prev") from the reading start (or end) until the pan stops,
/// asserting the structural invariants that a defect would break.
#[allow(clippy::too_many_arguments)]
fn walk(
    app: &mut App,
    layout: &FullscreenPageLayout,
    zoom: f32,
    direction: ReadingDirection,
    order: crate::settings::FullscreenNScrollOrder,
    next: bool,
    step: f32,
    primary_fraction: Option<f32>,
) -> Walk {
    let geometry = scroll_geometry(layout).expect("zoomed spread overflows");
    let primary = if order == HorizontalFirst { 0 } else { 1 };
    let secondary = 1 - primary;
    let horizontal = match direction {
        ReadingDirection::Ltr => -1.0,
        ReadingDirection::Rtl => 1.0,
    };
    let signs = Vec2::new(horizontal, -1.0) * if next { 1.0 } else { -1.0 };
    let mut pan = Vec2::ZERO;
    for axis in 0..2 {
        if let Some(range) = geometry.axes[axis] {
            pan[axis] = range.start(signs[axis]);
        }
    }
    app.fullscreen_page_layout = layout.clone();
    let mut walk = Walk {
        pans: vec![pan],
        ..Walk::default()
    };
    for _ in 0..1000 {
        let result = ordered_scroll_pan_with_steps(
            layout,
            zoom,
            pan,
            direction,
            order,
            next,
            step,
            primary_fraction,
            true,
        );
        if result == pan {
            return walk;
        }
        // The accepted pan is what the normal input writer keeps.
        app.set_fs_pan_from_input(zoom, result);
        assert_eq!(app.fs_pan, result, "accepted pan must be clamp-idempotent");
        let primary_move = (result[primary] - pan[primary]) * signs[primary];
        let secondary_move = (result[secondary] - pan[secondary]) * signs[secondary];
        if secondary_move == 0.0 {
            assert!(primary_move > 0.0, "a reading step must go forward");
        } else {
            assert!(
                secondary_move > 0.0,
                "a wrap must advance to a new column/row"
            );
            walk.wraps += 1;
            walk.wrap_moves.push(secondary_move / VIEWPORT[secondary]);
        }
        assert!(
            !walk.pans.contains(&result),
            "position revisited: {result:?} in {:?}",
            walk.pans
        );
        walk.pans.push(result);
        pan = result;
    }
    panic!("never stopped: {:?}", walk.pans);
}

/// The column overflow is `zoom * unit_width - viewport_width`; a wrap moves
/// `step * viewport_width`, the last one only up to the edge. Columns are
/// therefore `1 + ceil(overflow / step)`: a property of the shared step, not
/// of a loop.
fn expected_wraps(height_over_width: f32, zoom: f32, gap: f32, step: f32) -> usize {
    let unit_w = 2.0 * VIEWPORT.y * zoom / height_over_width + gap;
    let overflow = unit_w - VIEWPORT.x;
    if overflow <= 0.0 {
        0
    } else {
        (overflow / (VIEWPORT.x * step)).ceil() as usize
    }
}

#[test]
fn report_850_conditions_are_the_shared_step_not_a_loop() {
    let mut app = crate::app::setup_app_for_test();
    let mut report = String::new();
    for (label, ratio) in [("1:1.4", 1.4_f32), ("1:sqrt2", std::f32::consts::SQRT_2)] {
        for gap in [0.0_f32, 20.0] {
            for zoom in [1.5_f32, 2.0] {
                for step in [0.15_f32, 0.20, 0.85] {
                    for direction in [ReadingDirection::Rtl, ReadingDirection::Ltr] {
                        let layout = zoomed_spread(ratio, zoom, gap);
                        // Key (K / J, scroll-then-turn key) and wheel (primary = 20%/notch).
                        for primary_fraction in [None, Some(0.20)] {
                            for next in [true, false] {
                                let walk = walk(
                                    &mut app,
                                    &layout,
                                    zoom,
                                    direction,
                                    VerticalFirst,
                                    next,
                                    step,
                                    primary_fraction,
                                );
                                assert_eq!(
                                    walk.wraps,
                                    expected_wraps(ratio, zoom, gap, step),
                                    "{label} gap={gap} zoom={zoom} step={step} {direction:?}"
                                );
                                if next && primary_fraction.is_none() {
                                    report.push_str(&format!(
                                        "{label} gap={gap} zoom={zoom} step={step} {direction:?}: \
                                         columns={} wraps(x/viewport)={:?} pans={}\n",
                                        walk.wraps + 1,
                                        walk.wrap_moves,
                                        walk.pans.len()
                                    ));
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    println!("{report}");
}

#[test]
fn report_850_matches_one_n_at_150_20_and_repeats_at_150_15_and_200_20() {
    // An A-series page (1:sqrt2) fitted to a 16:9 height reproduces the three
    // reported observations exactly; 1:1.4 leaves a ~11 px third column at
    // 150% / 20% because the overflow is just above one step.
    let ratio = std::f32::consts::SQRT_2;
    assert_eq!(
        expected_wraps(ratio, 1.5, 0.0, 0.20),
        1,
        "150% / 20%: one N"
    );
    assert_eq!(
        expected_wraps(ratio, 1.5, 0.0, 0.15),
        2,
        "150% / 15%: repeats"
    );
    assert!(
        expected_wraps(ratio, 2.0, 0.0, 0.20) > 1,
        "200% / 20%: repeats"
    );
    assert_eq!(expected_wraps(1.4, 1.5, 0.0, 0.20), 2);
    let mut app = crate::app::setup_app_for_test();
    let layout = zoomed_spread(1.4, 1.5, 0.0);
    let walk = walk(
        &mut app,
        &layout,
        1.5,
        ReadingDirection::Rtl,
        VerticalFirst,
        true,
        0.20,
        None,
    );
    let tail = *walk.wrap_moves.last().unwrap() * VIEWPORT.x;
    assert!(tail > 0.0 && tail < 20.0, "tail column moves {tail} px");
}

#[test]
fn report_850_z_order_rows_follow_the_same_rule() {
    // Transposed check: Z order on a tall single page wraps rows by the same step.
    let mut app = crate::app::setup_app_for_test();
    let layout = zoomed_spread(1.4, 2.0, 0.0);
    for step in [0.15_f32, 0.20, 0.85] {
        let walk = walk(
            &mut app,
            &layout,
            2.0,
            ReadingDirection::Rtl,
            HorizontalFirst,
            true,
            step,
            None,
        );
        let overflow_y = VIEWPORT.y * 2.0 - VIEWPORT.y;
        assert_eq!(
            walk.wraps,
            (overflow_y / (VIEWPORT.y * step)).ceil() as usize,
            "step={step}"
        );
    }
}
