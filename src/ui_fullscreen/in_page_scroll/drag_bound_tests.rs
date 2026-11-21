//! Pure geometry contracts for §1.834 (including the r5–r9 counterexamples).
//! These resolved-layout fixtures do not stand in for the real-renderer tests.
use super::drag_bound::bounded_drag_pan;
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
fn fitted_axes_stay_at_nonzero_current_pan_without_recentering() {
    let cur = egui::vec2(137.0, -91.0);
    for (size, expected) in [
        (Vec2::splat(800.0), cur),
        (Vec2::splat(1000.0), cur),
        (egui::vec2(800.0, 3000.0), egui::vec2(cur.x, -1000.0)),
        (egui::vec2(3000.0, 800.0), egui::vec2(-1000.0, cur.y)),
    ] {
        close(
            bounded_drag_pan(&single(size), cur, Vec2::splat(-5000.0)),
            expected,
        );
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
    close(cur, target);
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

#[test]
fn r9_short_trim_visibility_box_rounds_inward_and_retains_one_pixel() {
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
    let cur = egui::vec2(-489.0, 1000.0);
    let layout = drawn_at(&template, cur, 1.0, RectPixelFit::Proportional);
    close(
        bounded_drag_pan(&layout, cur, egui::vec2(-510.0, 512.0)),
        egui::vec2(-489.0, 512.0),
    );
}

#[test]
fn one_two_and_three_physical_pixel_overflow_has_no_dead_band_at_either_edge() {
    for overflow in [1.0, 2.0, 3.0] {
        for axis in 0..2 {
            let mut size = Vec2::splat(1000.0);
            size[axis] += overflow;
            let rect = Rect::from_min_size(viewport().min, size);
            let layout = layout_from_pages(&[DisplayedImageTransform::from_resolved_rect(
                page_input(0, rect, 1.0, Vec2::ZERO),
                rect,
            )
            .unwrap()]);
            let mut far = Vec2::ZERO;
            far[axis] = -100.0;
            let mut edge = Vec2::ZERO;
            edge[axis] = -overflow;
            close(bounded_drag_pan(&layout, Vec2::ZERO, far), edge);
            let mut before_edge = edge;
            before_edge[axis] += 1.0;
            close(bounded_drag_pan(&layout, before_edge, far), edge);
            far[axis] = 100.0;
            close(bounded_drag_pan(&layout, edge, far), Vec2::ZERO);
            let mut before_other_edge = Vec2::ZERO;
            before_other_edge[axis] = -1.0;
            close(
                bounded_drag_pan(&layout, before_other_edge, far),
                Vec2::ZERO,
            );
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
