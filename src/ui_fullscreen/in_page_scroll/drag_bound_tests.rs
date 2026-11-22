//! Pure geometry contracts for §1.834 (including the r5–r9 counterexamples).
//! These resolved-layout fixtures do not stand in for the real-renderer tests.
use super::drag_bound::{
    bounded_drag_pan, inward_drag_visibility_bounds, trunc_drag_bound_edge_delta,
};
use super::tests::{asymmetric_spread, close, layout_from_pages, page_input, single, viewport};
use super::*;
use crate::rotation_db::Rotation;

/// Reconstruct the existing resolved-layout fixture through the production
/// transform constructor, retaining its trim, rotation, and singleton placement.
fn drawn_at(
    layout: &FullscreenPageLayout,
    pan: Vec2,
    pixels_per_point: f32,
    pixel_fit: RectPixelFit,
) -> FullscreenPageLayout {
    let pages: Vec<_> = layout
        .displayed_pages()
        .map(|displayed| {
            let page = displayed.transform;
            let (zoom, old_pan) = normal_zoom_pan(page).unwrap();
            let rect = page.full_image_rect.translate(pan - old_pan);
            let mut input = page_input(page.page_idx, rect, zoom, pan);
            input.source_size = page.source_size;
            input.texture_size = page.texture_size;
            input.rotation = page.rotation;
            input.free_rotation_rad = page.free_rotation_rad;
            input.content_bbox = page.content_bbox;
            input.fit_mode = page.fit_input;
            input.fit_scale_limits = page.fit_scale_limits;
            input.pixels_per_point = pixels_per_point;
            input.pixel_fit = pixel_fit;
            input.placement = match page.placement {
                ResolvedDisplayPlacement::Normal { .. } => ResolvedDisplayPlacement::Normal {
                    zoom_pan: Some((zoom, pan)),
                },
                ResolvedDisplayPlacement::SingletonSpread { side, gap, .. } => {
                    ResolvedDisplayPlacement::SingletonSpread {
                        side,
                        gap,
                        zoom_pan: Some((zoom, pan)),
                    }
                }
                _ => panic!("bounded-drag fixtures use normal or singleton placement"),
            };
            DisplayedImageTransform::from_resolved_rect(input, rect).unwrap()
        })
        .collect();
    layout_from_pages(&pages)
}

#[test]
fn overflowing_axes_stop_at_each_edge_and_diagonal() {
    let layout = single(Vec2::splat(3000.0));
    for (proposed, expected) in [
        (egui::vec2(-5000.0, 0.0), egui::vec2(-1000.0, 0.0)),
        (egui::vec2(5000.0, 0.0), egui::vec2(1000.0, 0.0)),
        (egui::vec2(0.0, -5000.0), egui::vec2(0.0, -1000.0)),
        (egui::vec2(0.0, 5000.0), egui::vec2(0.0, 1000.0)),
        (Vec2::splat(-5000.0), Vec2::splat(-1000.0)),
        (Vec2::splat(5000.0), Vec2::splat(1000.0)),
        (egui::vec2(-5000.0, 5000.0), egui::vec2(-1000.0, 1000.0)),
        (egui::vec2(5000.0, -5000.0), egui::vec2(1000.0, -1000.0)),
    ] {
        close(bounded_drag_pan(&layout, Vec2::ZERO, proposed), expected);
    }
}

#[test]
fn drag_bound_fitted_axes_inside_viewport_stay_at_noncentral_pan_in_all_directions() {
    // §8.2 replaces the old blanket freeze: a fitted unit stays fixed only
    // while contained. The old (137, -91) fixture was already outside it.
    for pixels_per_point in [1.0, 1.25, 1.5] {
        for (template, cur) in [
            (single(Vec2::splat(800.0)), egui::vec2(37.0, -91.0)),
            (single(Vec2::splat(800.0)), Vec2::splat(-100.0)),
            (single(Vec2::splat(800.0)), Vec2::splat(100.0)),
            (single(Vec2::splat(1000.0)), Vec2::ZERO),
            (
                singleton(SingletonSpreadPlacement::LeftWhite, 400.0),
                egui::vec2(37.0, -91.0),
            ),
            (
                singleton(SingletonSpreadPlacement::RightWhite, 400.0),
                egui::vec2(37.0, -91.0),
            ),
        ] {
            let layout = drawn_at(&template, cur, pixels_per_point, RectPixelFit::Proportional);
            for x in [-5000.0, 0.0, 5000.0] {
                for y in [-5000.0, 0.0, 5000.0] {
                    close(bounded_drag_pan(&layout, cur, cur + egui::vec2(x, y)), cur);
                }
            }
        }
    }
}

#[test]
fn drag_bound_fitted_axes_outside_viewport_return_only_inward_without_recentering() {
    // §8.2 intentionally changes these old expectations: at (137, -91),
    // an 800-point unit protrudes 37 points at the right; a 1000-point unit
    // protrudes 137 at the right and 91 at the top. Negative y is outward.
    let cur = egui::vec2(137.0, -91.0);
    for (size, expected) in [
        (Vec2::splat(800.0), egui::vec2(100.0, -91.0)),
        (Vec2::splat(1000.0), egui::vec2(0.0, -91.0)),
        (egui::vec2(800.0, 3000.0), egui::vec2(100.0, -1000.0)),
        (egui::vec2(3000.0, 800.0), egui::vec2(-1000.0, cur.y)),
    ] {
        let layout = single(size);
        let stopped = bounded_drag_pan(&layout, cur, Vec2::splat(-5000.0));
        close(stopped, expected);
        if size == Vec2::splat(800.0) {
            // This recovery is exact: x=100 aligns the right edge with the
            // viewport. Reversing toward the former x=137 must stay inside.
            close(bounded_drag_pan(&layout, stopped, cur), stopped);
        }
    }
}

#[test]
fn drag_bound_fitted_axes_low_high_and_both_recover_within_one_physical_pixel_at_each_dpi() {
    // The 800-point square has 100 points of slack on each side. A 400-point
    // singleton plus its 20-point gap and 400-point blank forms an 820 x 400
    // unit: horizontal slack is 90, vertical slack is 300, for either side.
    for (template, allowance) in [
        (single(Vec2::splat(800.0)), Vec2::splat(100.0)),
        (
            singleton(SingletonSpreadPlacement::LeftWhite, 400.0),
            egui::vec2(90.0, 300.0),
        ),
        (
            singleton(SingletonSpreadPlacement::RightWhite, 400.0),
            egui::vec2(90.0, 300.0),
        ),
    ] {
        for pixels_per_point in [1.0, 1.25, 1.5] {
            for signs in [
                egui::vec2(-1.0, -1.0),
                egui::vec2(-1.0, 1.0),
                egui::vec2(1.0, -1.0),
                egui::vec2(1.0, 1.0),
            ] {
                // Masks cover x only, y only, and simultaneous recovery.
                for mask in [1, 2, 3] {
                    let mut cur = egui::vec2(17.0, -23.0);
                    let mut inward = cur;
                    let mut outward = cur;
                    let mut small_step = cur;
                    let mut expected = cur;
                    for axis in 0..2 {
                        if mask & (1 << axis) != 0 {
                            cur[axis] = signs[axis] * (allowance[axis] + 37.75 / pixels_per_point);
                            inward[axis] = cur[axis] - signs[axis] * 5000.0;
                            outward[axis] = cur[axis] + signs[axis] * 5000.0;
                            small_step[axis] = cur[axis] - signs[axis] * 5.0 / pixels_per_point;
                            // 37.75 physical pixels of excess, truncated to
                            // a 37-pixel recovery, leaves 0.75 pixels outside.
                            expected[axis] =
                                signs[axis] * (allowance[axis] + 0.75 / pixels_per_point);
                        }
                    }
                    let layout =
                        drawn_at(&template, cur, pixels_per_point, RectPixelFit::Proportional);
                    close(bounded_drag_pan(&layout, cur, cur), cur);
                    close(bounded_drag_pan(&layout, cur, outward), cur);
                    close(bounded_drag_pan(&layout, cur, small_step), small_step);
                    let stopped = bounded_drag_pan(&layout, cur, inward);
                    close(stopped, expected);
                    for axis in 0..2 {
                        if mask & (1 << axis) != 0 {
                            let remaining =
                                (stopped[axis].abs() - allowance[axis]) * pixels_per_point;
                            assert!(
                                (0.0..=1.0).contains(&remaining),
                                "axis={axis}, dpi={pixels_per_point}, mask={mask}, cur={cur:?}, stopped={stopped:?}, remaining_px={remaining}"
                            );
                        }
                    }
                    // The subpixel residue must not creep on repeated input,
                    // and reversal cannot move the recovered unit outward.
                    close(bounded_drag_pan(&layout, stopped, inward), stopped);
                    close(bounded_drag_pan(&layout, stopped, outward), stopped);
                }
            }
        }
    }
}

#[test]
fn outside_current_pan_moves_only_inward_and_never_snaps_or_recrosses_edge() {
    let layout = single(Vec2::splat(3000.0));
    for axis in 0..2 {
        for sign in [-1.0, 1.0] {
            let mut cur = Vec2::ZERO;
            cur[axis] = sign * 1200.0;
            close(bounded_drag_pan(&layout, cur, cur), cur);
            let mut outward = cur;
            outward[axis] += sign * 50.0;
            close(bounded_drag_pan(&layout, cur, outward), cur);
            let mut inward = cur;
            inward[axis] -= sign * 50.0;
            close(bounded_drag_pan(&layout, cur, inward), inward);
            let mut inside = cur;
            inside[axis] = sign * 900.0;
            close(bounded_drag_pan(&layout, cur, inside), inside);
            let mut edge = cur;
            edge[axis] = sign * 1000.0;
            close(bounded_drag_pan(&layout, inside, outward), edge);
        }
    }
}

#[test]
fn asymmetric_spread_edges_match_scroll_geometry_when_visibility_allows_them() {
    for transpose in [false, true] {
        let layout = asymmetric_spread(true, transpose);
        let geometry = scroll_geometry(&layout).unwrap();
        for sign in [-1.0, 1.0] {
            let primary = if transpose { 0 } else { 1 };
            let secondary = 1 - primary;
            // Keep the tall page visible while reaching both outer unit edges.
            let mut cur = Vec2::ZERO;
            cur[secondary] = geometry.axes[secondary].unwrap().max;
            let mut proposed = cur;
            proposed[primary] = sign * 5000.0;
            let mut expected = cur;
            expected[primary] = if sign < 0.0 {
                geometry.axes[primary].unwrap().min
            } else {
                geometry.axes[primary].unwrap().max
            };
            close(bounded_drag_pan(&layout, cur, proposed), expected);
        }
    }
}

#[test]
fn saved_quarter_rotation_and_display_trim_use_the_painted_edges() {
    for rotation in [Rotation::None, Rotation::Cw90] {
        for trim in [false, true] {
            let rect = Rect::from_center_size(viewport().center(), egui::vec2(1600.0, 2400.0));
            let mut input = page_input(0, rect, 1.0, Vec2::ZERO);
            input.rotation = rotation;
            if rotation == Rotation::Cw90 {
                input.source_size = rect.size().yx();
                input.texture_size = rect.size().yx();
            }
            if trim {
                input.content_bbox = Some(Rect::from_min_max(
                    egui::pos2(0.125, 0.25),
                    egui::pos2(0.875, 0.75),
                ));
            }
            let page = DisplayedImageTransform::from_resolved_rect(input, rect).unwrap();
            let layout = layout_from_pages(&[page]);
            let geometry = scroll_geometry(&layout).unwrap();
            for sign in [-1.0, 1.0] {
                let expected = Vec2::new(
                    geometry.axes[0].map_or(0.0, |r| if sign < 0.0 { r.min } else { r.max }),
                    geometry.axes[1].map_or(0.0, |r| if sign < 0.0 { r.min } else { r.max }),
                );
                close(
                    bounded_drag_pan(&layout, Vec2::ZERO, Vec2::splat(sign * 10000.0)),
                    expected,
                );
            }
        }
    }
}

fn singleton(side: SingletonSpreadPlacement, size: f32) -> FullscreenPageLayout {
    let offset = if matches!(
        side,
        SingletonSpreadPlacement::Left | SingletonSpreadPlacement::LeftWhite
    ) {
        -1.0
    } else {
        1.0
    };
    let rect = Rect::from_center_size(
        viewport().center() + egui::vec2(offset * (size + 20.0) * 0.5, 0.0),
        Vec2::splat(size),
    );
    let mut input = page_input(0, rect, 1.0, Vec2::ZERO);
    input.placement = ResolvedDisplayPlacement::SingletonSpread {
        side,
        gap: 20.0,
        zoom_pan: Some((1.0, Vec2::ZERO)),
    };
    layout_from_pages(&[DisplayedImageTransform::from_resolved_rect(input, rect).unwrap()])
}

#[test]
fn blank_companion_counts_for_unit_edges_while_vacant_slot_does_not() {
    for (side, sign) in [
        (SingletonSpreadPlacement::Left, -1.0),
        (SingletonSpreadPlacement::LeftWhite, -1.0),
        (SingletonSpreadPlacement::Right, 1.0),
        (SingletonSpreadPlacement::RightWhite, 1.0),
    ] {
        let layout = singleton(side, 800.0);
        let expected = if side.has_white_companion() {
            egui::vec2(sign * 310.0, 0.0)
        } else {
            Vec2::ZERO
        };
        close(
            bounded_drag_pan(&layout, Vec2::ZERO, egui::vec2(sign * 5000.0, 0.0)),
            expected,
        );
    }
}

#[test]
fn enlarged_singleton_moves_gradually_from_blank_side_and_reaches_blank_edge() {
    for (side, sign) in [
        (SingletonSpreadPlacement::LeftWhite, -1.0),
        (SingletonSpreadPlacement::RightWhite, 1.0),
    ] {
        let layout = singleton(side, 1600.0);
        let cur = egui::vec2(sign * 1100.0, 0.0);
        let real = layout
            .displayed_pages()
            .next()
            .unwrap()
            .transform
            .paint_rect
            .translate(cur);
        assert!(
            !real.intersect(viewport()).is_positive(),
            "only the blank is visible"
        );
        close(bounded_drag_pan(&layout, cur, cur), cur);
        let inward = cur - egui::vec2(sign * 5.0, 0.0);
        close(bounded_drag_pan(&layout, cur, inward), inward);
        let edge = egui::vec2(sign * 1110.0, 0.0);
        close(
            bounded_drag_pan(&layout, cur, egui::vec2(sign * 5000.0, 0.0)),
            edge,
        );
        close(
            bounded_drag_pan(&layout, edge, egui::vec2(sign * 5000.0, 0.0)),
            edge,
        );
    }
}

#[test]
fn r4_asymmetric_l_shaped_gap_keeps_one_real_page_visible() {
    let layout = asymmetric_spread(true, false);
    // The unit rectangle alone permits (-510, 1000), which hides both pages.
    close(
        bounded_drag_pan(&layout, Vec2::ZERO, egui::vec2(-510.0, 1000.0)),
        egui::vec2(-442.0, 1000.0),
    );
}

#[test]
fn r5_partial_visibility_widths_do_not_worsen_but_allow_inward_and_other_axis() {
    let layout = asymmetric_spread(true, false);
    for width in [0.0, 1.0, 25.0, 45.0, 48.0] {
        let cur = egui::vec2(width - 490.0, 1000.0);
        close(
            bounded_drag_pan(&layout, cur, cur - egui::vec2(50.0, 0.0)),
            cur,
        );
        let inward = cur + egui::vec2(5.0, 0.0);
        close(bounded_drag_pan(&layout, cur, inward), inward);
        let vertical = cur - egui::vec2(0.0, 100.0);
        close(bounded_drag_pan(&layout, cur, vertical), vertical);
        // Improvements below the normal threshold cannot be undone. Once the
        // threshold is met, preserving 48 points is sufficient.
        let retained = cur + egui::vec2((48.0_f32 - width).min(5.0), 0.0);
        close(bounded_drag_pan(&layout, inward, cur), retained);
    }
    close(
        bounded_drag_pan(
            &layout,
            egui::vec2(-443.0, 1000.0),
            egui::vec2(-443.0, 900.0),
        ),
        egui::vec2(-443.0, 900.0),
    );
    close(
        bounded_drag_pan(
            &layout,
            egui::vec2(-465.0, 1000.0),
            egui::vec2(-510.0, 1000.0),
        ),
        egui::vec2(-465.0, 1000.0),
    );
}

#[test]
fn invisible_elements_allow_gradual_recovery_without_jumping_or_growing_distance() {
    let layout = asymmetric_spread(true, false);
    // Left page is 30 points left of the viewport; right page is below it.
    let cur = egui::vec2(-520.0, 1000.0);
    close(bounded_drag_pan(&layout, cur, cur), cur);
    close(
        bounded_drag_pan(&layout, cur, cur - egui::vec2(10.0, 0.0)),
        cur,
    );
    let vertical = cur - egui::vec2(0.0, 50.0);
    close(bounded_drag_pan(&layout, cur, vertical), vertical);
    let mut pan = cur;
    for _ in 0..8 {
        let proposed = pan + egui::vec2(5.0, 0.0);
        close(bounded_drag_pan(&layout, pan, proposed), proposed);
        pan = proposed;
    }
    assert_eq!(pan, egui::vec2(-480.0, 1000.0));
    assert!(
        layout
            .displayed_pages()
            .next()
            .unwrap()
            .transform
            .paint_rect
            .translate(pan)
            .intersect(viewport())
            .is_positive()
    );
    close(bounded_drag_pan(&layout, pan, cur), pan);
}

#[test]
fn r6_newly_visible_page_is_reached_in_one_input_or_split_straight_inputs() {
    let layout = asymmetric_spread(true, false);
    let start = egui::vec2(-442.0, 1000.0);
    let target = egui::vec2(-510.0, 700.0);
    close(bounded_drag_pan(&layout, start, target), target);
    let mut cur = start;
    for step in 1..=20 {
        let proposed = start + (target - start) * (step as f32 / 20.0);
        cur = bounded_drag_pan(&layout, cur, proposed);
    }
    // The final step starts at -506.6: trunc(-3.4) advances only 3 points.
    // Logical pan differs by 0.4px, but the next texel-aligned paint is equal.
    close(cur, egui::vec2(-509.6, 700.0));
    assert!((cur - target).abs().max_elem() < 1.0);
    let once = drawn_at(&layout, target, 1.0, RectPixelFit::Texels);
    let split = drawn_at(&layout, cur, 1.0, RectPixelFit::Texels);
    for (once, split) in once.displayed_pages().zip(split.displayed_pages()) {
        assert_eq!(once.transform.paint_rect, split.transform.paint_rect);
        assert_eq!(once.transform.paint_quad(), split.transform.paint_quad());
        assert_eq!(once.transform.uv_rect, split.transform.uv_rect);
    }
}

#[test]
fn r7_insufficient_new_page_visibility_cannot_hide_previous_page() {
    let layout = asymmetric_spread(true, false);
    let start = egui::vec2(-442.0, 1000.0);
    let target = egui::vec2(-510.0, 762.0);
    let expected = egui::vec2(-442.0, 762.0);
    close(bounded_drag_pan(&layout, start, target), expected);
    let intermediate = bounded_drag_pan(&layout, start, egui::vec2(-502.0, 790.0));
    close(intermediate, egui::vec2(-442.0, 790.0));
    close(bounded_drag_pan(&layout, intermediate, target), expected);
}

#[test]
fn r8_asymmetric_vertical_drag_stops_at_visible_page_edges_without_sideways_motion() {
    let layout = asymmetric_spread(true, false);
    let cur = egui::vec2(-450.0, -500.0);
    close(
        bounded_drag_pan(&layout, cur, egui::vec2(-450.0, 5000.0)),
        egui::vec2(-450.0, 755.0),
    );
    close(
        bounded_drag_pan(&layout, cur, egui::vec2(-450.0, -5000.0)),
        egui::vec2(-450.0, -957.0),
    );
}

#[test]
fn r8_interpolated_drawn_pan_uses_current_origin_for_visibility_rounding() {
    let drawn_pan = egui::vec2(-200.75, 1000.0);
    let layout = drawn_at(
        &asymmetric_spread(true, false),
        drawn_pan,
        1.0,
        RectPixelFit::Texels,
    );
    let cur = egui::vec2(-442.0, 1000.0);
    let left = layout
        .displayed_pages()
        .next()
        .unwrap()
        .transform
        .paint_rect
        .translate(cur - drawn_pan);
    assert_eq!(left.right() - viewport().left(), 47.75);
    let target = egui::vec2(-442.0, 900.0);
    close(bounded_drag_pan(&layout, cur, target), target);
}

#[test]
fn visibility_quotient_roundoff_cannot_exclude_current_pan_on_an_unrequested_axis() {
    for dpi in [1.0, 1.25, 1.5] {
        let viewport = Rect::from_min_size(egui::Pos2::ZERO, Vec2::splat(1000.0));
        let rect = Rect::from_min_max(egui::pos2(-2000.0, -1000.0), egui::pos2(25.6, 2000.0));
        let mut input = page_input(0, rect, 1.0, Vec2::ZERO);
        input.viewport_rect = viewport;
        input.pixels_per_point = dpi;
        let page = DisplayedImageTransform::from_resolved_rect(input, rect).unwrap();
        let overlap = page.paint_rect.right();
        let reference = minimum_pan_overlap(page.paint_rect, viewport).x;
        assert_eq!(overlap, 25.6);
        assert!((overlap / reference) * reference > overlap);
        let layout = layout_from_pages(&[page]);
        close(
            bounded_drag_pan(&layout, Vec2::ZERO, egui::vec2(0.0, -100.0)),
            egui::vec2(0.0, -100.0),
        );
    }
}

#[test]
fn interpolated_drawn_pan_cannot_reverse_requested_axis_or_move_an_unrequested_axis() {
    let layout = drawn_at(
        &single(Vec2::splat(3000.0)),
        egui::vec2(-200.75, 70.25),
        1.0,
        RectPixelFit::Texels,
    );
    let cur = egui::vec2(-1100.0, 300.0);
    close(
        bounded_drag_pan(&layout, cur, egui::vec2(-1200.0, 250.0)),
        egui::vec2(-1100.0, 250.0),
    );
    close(
        bounded_drag_pan(&layout, cur, egui::vec2(-1095.0, 300.0)),
        egui::vec2(-1095.0, 300.0),
    );
}

fn short_trimmed_spread() -> FullscreenPageLayout {
    let left = Rect::from_min_max(egui::pos2(-480.0, -950.0), egui::pos2(520.0, 2050.0));
    let right = Rect::from_min_max(egui::pos2(540.0, 50.0), egui::pos2(1540.0, 1050.0));
    let mut right_input = page_input(1, right, 1.0, Vec2::ZERO);
    right_input.content_bbox = Some(Rect::from_min_max(
        egui::pos2(0.0, 0.488),
        egui::pos2(1.0, 0.512),
    ));
    let template = layout_from_pages(&[
        DisplayedImageTransform::from_resolved_rect(page_input(0, left, 1.0, Vec2::ZERO), left)
            .unwrap(),
        DisplayedImageTransform::from_resolved_rect(right_input, right).unwrap(),
    ]);
    let painted_right = template
        .displayed_pages()
        .nth(1)
        .unwrap()
        .transform
        .paint_rect;
    close(painted_right.min.to_vec2(), egui::vec2(540.0, 538.0));
    close(painted_right.max.to_vec2(), egui::vec2(1540.0, 562.0));
    template
}

#[test]
fn r9_short_trim_visibility_box_rounds_inward_and_retains_one_pixel() {
    let template = short_trimmed_spread();
    let cur = egui::vec2(-489.0, 1000.0);
    let layout = drawn_at(&template, cur, 1.0, RectPixelFit::Proportional);
    close(
        bounded_drag_pan(&layout, cur, egui::vec2(-510.0, 512.0)),
        egui::vec2(-489.0, 512.0),
    );
}

#[test]
fn r2_near_integer_visibility_box_keeps_positive_overlap_in_both_directions() {
    let layout = short_trimmed_spread();
    for page in layout.displayed_pages() {
        assert_eq!(normal_zoom_pan(page.transform).unwrap().1, Vec2::ZERO);
    }
    let visibility = |pan: Vec2| {
        layout
            .displayed_pages()
            .map(|page| {
                let rect = page.transform.paint_rect.translate(pan);
                let reference = minimum_pan_overlap(rect, viewport());
                let overlap = viewport().max.min(rect.max) - viewport().min.max(rect.min);
                (overlap.x / reference.x).min(overlap.y / reference.y)
            })
            .fold(f32::NEG_INFINITY, f32::max)
    };
    for sign in [1.0, -1.0] {
        let cur = egui::vec2(-489.99951171875, sign * 1000.0);
        let proposed = egui::vec2(-510.0, sign * 512.0);
        let before = visibility(cur);
        assert!(before > 0.0, "the left page starts with positive overlap");
        let actual = bounded_drag_pan(&layout, cur, proposed);
        // Right-page limits are -488.000244 (floor -489), or its mirrored
        // +488.000244 (ceil +489). Both lie outside the requested y interval.
        assert_eq!(actual, egui::vec2(cur.x, proposed.y));
        let after = visibility(actual);
        assert!(
            after >= before && after > 0.0,
            "sign={sign}, before={before}, after={after}, pan={actual:?}"
        );
        assert!(
            layout.displayed_pages().any(|page| {
                page.transform
                    .paint_rect
                    .translate(actual)
                    .intersect(viewport())
                    .is_positive()
            }),
            "some element must overlap the viewport on both axes"
        );
    }
}

#[test]
fn one_two_and_three_physical_pixel_overflow_has_no_dead_band_at_either_edge() {
    for dpi in [1.0, 1.25, 1.5] {
        let pixel = physical_pixel_scale(dpi);
        let viewport = Rect::from_min_size(egui::Pos2::ZERO, Vec2::splat(1000.0 / dpi));
        for overflow_px in [1.0, 2.0, 3.0] {
            for axis in 0..2 {
                let mut size = Vec2::splat(1000.0);
                size[axis] += overflow_px;
                let rect = Rect::from_min_size(viewport.min, size / dpi);
                let mut input = page_input(0, rect, 1.0, Vec2::ZERO);
                input.viewport_rect = viewport;
                input.source_size = size;
                input.texture_size = size;
                input.pixels_per_point = dpi;
                let layout =
                    layout_from_pages(&[
                        DisplayedImageTransform::from_resolved_rect(input, rect).unwrap()
                    ]);
                let mut far = Vec2::ZERO;
                far[axis] = -100.0;
                let mut edge = Vec2::ZERO;
                edge[axis] = -overflow_px * pixel;
                close(bounded_drag_pan(&layout, Vec2::ZERO, far), edge);
                let mut before_edge = edge;
                before_edge[axis] += pixel;
                close(bounded_drag_pan(&layout, before_edge, far), edge);
                far[axis] = 100.0;
                close(bounded_drag_pan(&layout, edge, far), Vec2::ZERO);
                let mut before_other_edge = Vec2::ZERO;
                before_other_edge[axis] = -pixel;
                close(
                    bounded_drag_pan(&layout, before_other_edge, far),
                    Vec2::ZERO,
                );
            }
        }
    }
}

#[test]
fn physical_pixel_rounding_preserves_fractions_and_repairs_integer_residue() {
    for dpi in [1.0, 1.25, 1.5] {
        let pixel = physical_pixel_scale(dpi);
        for integer in [-3.0, -2.0, -1.0, 0.0, 1.0, 2.0, 3.0] {
            for residue in [-0.0005, 0.0005] {
                let actual = trunc_drag_bound_edge_delta((integer + residue) * pixel, pixel);
                close(Vec2::splat(actual), Vec2::splat(integer * pixel));
            }
            // Visibility limits keep their fractions: ceil(n + .0005) is
            // n + 1, while floor(n + 3 - .0005) is n + 2.
            let bounds = Rect::from_min_max(
                egui::pos2((integer + 0.0005) * pixel, (integer + 0.0005) * pixel),
                egui::pos2(
                    (integer + 3.0 - 0.0005) * pixel,
                    (integer + 3.0 - 0.0005) * pixel,
                ),
            );
            let inward = inward_drag_visibility_bounds(bounds, pixel);
            close(inward.min.to_vec2(), Vec2::splat((integer + 1.0) * pixel));
            close(inward.max.to_vec2(), Vec2::splat((integer + 2.0) * pixel));
        }
        for fraction in [-3.4_f32, -1.75, -1.002, 1.002, 1.75, 3.4] {
            let actual = trunc_drag_bound_edge_delta(fraction * pixel, pixel);
            close(Vec2::splat(actual), Vec2::splat(fraction.trunc() * pixel));
        }
    }
}

#[test]
fn fractional_edges_round_toward_current_pan_in_physical_pixels_at_each_dpi() {
    for pixels_per_point in [1.0, 1.25, 1.5] {
        let overflow = 100.9 / pixels_per_point;
        let rect = Rect::from_center_size(
            viewport().center(),
            viewport().size() + Vec2::splat(2.0 * overflow),
        );
        let mut input = page_input(0, rect, 1.0, Vec2::ZERO);
        input.pixels_per_point = pixels_per_point;
        let layout =
            layout_from_pages(&[DisplayedImageTransform::from_resolved_rect(input, rect).unwrap()]);
        for sign in [-1.0, 1.0] {
            let expected = Vec2::splat(sign * 100.0 / pixels_per_point);
            let proposed = Vec2::splat(sign * 5000.0);
            close(bounded_drag_pan(&layout, Vec2::ZERO, proposed), expected);
            close(bounded_drag_pan(&layout, expected, proposed), expected);
        }
    }
}
