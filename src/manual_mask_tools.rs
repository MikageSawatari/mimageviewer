//! Shared helpers for manual bitmap mask tools used by erase/conceal/local-adjust UIs.

pub(crate) const FREEHAND_MIN_DISTANCE_SQ: f32 = 4.0;
pub(crate) const POLYGON_CLOSE_RADIUS_PX: f32 = 12.0;
const POLYGON_VERTEX_MIN_DISTANCE_PX: f32 = 3.0;
const BRUSH_RADIUS_WHEEL_FACTOR: f32 = 1.1;

/// Shift+ホイールのノッチ数から筆半径を更新する。
///
/// 大きい半径では 1 ノッチごとに 1.1 倍、小さい半径では少なくとも 1px 動かす。
/// 同じノッチ数を逆向きへ適用すると、クランプされない限り元へ戻る対称な式にする。
pub(crate) fn brush_radius_after_wheel(radius: f32, notches: f32, min: f32, max: f32) -> f32 {
    let (min, max) = if min <= max { (min, max) } else { (max, min) };
    let radius = if radius.is_finite() { radius } else { min }.clamp(min, max);
    if !notches.is_finite() || notches == 0.0 {
        return radius;
    }

    let steps = notches.abs();
    let factor = BRUSH_RADIUS_WHEEL_FACTOR.powf(steps);
    let next = if notches > 0.0 {
        (radius * factor).max(radius + steps)
    } else {
        (radius / factor).min(radius - steps)
    };
    next.clamp(min, max)
}

pub(crate) fn push_freehand_point(points: &mut Vec<(f32, f32)>, point: (f32, f32)) -> bool {
    if points
        .last()
        .map(|&last| distance_sq(last, point) > FREEHAND_MIN_DISTANCE_SQ)
        .unwrap_or(true)
    {
        points.push(point);
        true
    } else {
        false
    }
}

pub(crate) fn should_close_polygon(
    points: &[(f32, f32)],
    point: (f32, f32),
    image_to_screen_scale: f32,
) -> bool {
    if points.len() < 3 {
        return false;
    }
    let scale = image_to_screen_scale.max(0.001);
    distance_sq(points[0], point) * scale * scale
        <= POLYGON_CLOSE_RADIUS_PX * POLYGON_CLOSE_RADIUS_PX
}

pub(crate) fn push_polygon_vertex(
    points: &mut Vec<(f32, f32)>,
    point: (f32, f32),
    image_to_screen_scale: f32,
) -> bool {
    let scale = image_to_screen_scale.max(0.001);
    let min_dist_sq = (POLYGON_VERTEX_MIN_DISTANCE_PX / scale).powi(2);
    if points
        .last()
        .map(|&last| distance_sq(last, point) > min_dist_sq)
        .unwrap_or(true)
    {
        points.push(point);
        true
    } else {
        false
    }
}

pub(crate) fn take_completed_polygon(points: &mut Vec<(f32, f32)>) -> Option<Vec<(f32, f32)>> {
    if points.len() >= 3 {
        Some(std::mem::take(points))
    } else {
        None
    }
}

fn distance_sq(a: (f32, f32), b: (f32, f32)) -> f32 {
    let dx = a.0 - b.0;
    let dy = a.1 - b.1;
    dx * dx + dy * dy
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn close_polygon_uses_screen_distance() {
        let points = vec![(10.0, 10.0), (40.0, 10.0), (40.0, 40.0)];
        assert!(should_close_polygon(&points, (15.0, 10.0), 2.0));
        assert!(!should_close_polygon(&points, (25.0, 10.0), 2.0));
    }

    #[test]
    fn incomplete_polygon_is_left_in_progress() {
        let mut points = vec![(0.0, 0.0), (1.0, 0.0)];
        assert!(take_completed_polygon(&mut points).is_none());
        assert_eq!(points.len(), 2);
    }

    #[test]
    fn brush_wheel_moves_small_radii_by_at_least_one_pixel() {
        assert_eq!(brush_radius_after_wheel(3.0, 1.0, 1.0, 500.0), 4.0);
        assert_eq!(brush_radius_after_wheel(4.0, -1.0, 1.0, 500.0), 3.0);
    }

    #[test]
    fn brush_wheel_scales_large_radii_proportionally() {
        assert!((brush_radius_after_wheel(200.0, 1.0, 1.0, 500.0) - 220.0).abs() < 0.001);
    }

    #[test]
    fn brush_wheel_clamps_to_the_slider_range() {
        assert_eq!(brush_radius_after_wheel(1.0, -1.0, 1.0, 220.0), 1.0);
        assert_eq!(brush_radius_after_wheel(220.0, 1.0, 1.0, 220.0), 220.0);
    }

    #[test]
    fn repeated_brush_wheel_steps_are_reversible_without_clamping() {
        let original = 37.0;
        let mut radius = original;
        for _ in 0..12 {
            radius = brush_radius_after_wheel(radius, 1.0, 1.0, 500.0);
        }
        for _ in 0..12 {
            radius = brush_radius_after_wheel(radius, -1.0, 1.0, 500.0);
        }
        assert!((radius - original).abs() < 0.001);
    }
}
