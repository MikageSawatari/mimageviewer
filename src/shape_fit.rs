//! 連結ビットマップ領域を単純な図形へ当てはめる純関数。
//!
//! 座標は画像 pixel 座標を使う。各 region pixel は
//! `[x, x + 1] x [y, y + 1]` の単位正方形として扱う。

use std::f64::consts::{FRAC_PI_2, FRAC_PI_4, PI};

pub const DEFAULT_MIN_COVERAGE: f32 = 0.75;
pub const DEFAULT_ANGLE_SNAP_DEG: f32 = 2.0;
pub const DEFAULT_NEAR_CIRCLE_RATIO: f32 = 1.05;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FitOptions {
    pub min_coverage: f32,
    pub angle_snap_deg: f32,
    pub near_circle_ratio: f32,
    pub outset: f32,
}

impl Default for FitOptions {
    fn default() -> Self {
        Self {
            min_coverage: DEFAULT_MIN_COVERAGE,
            angle_snap_deg: DEFAULT_ANGLE_SNAP_DEG,
            near_circle_ratio: DEFAULT_NEAR_CIRCLE_RATIO,
            outset: 0.0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum FittedShape {
    Rect {
        center: (f32, f32),
        half_w: f32,
        half_h: f32,
        rotation_rad: f32,
    },
    Ellipse {
        center: (f32, f32),
        rx: f32,
        ry: f32,
        rotation_rad: f32,
    },
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct Point {
    x: f64,
    y: f64,
}

impl Point {
    fn dot(self, direction: Point) -> f64 {
        self.x * direction.x + self.y * direction.y
    }
}

fn cross(origin: Point, a: Point, b: Point) -> f64 {
    (a.x - origin.x) * (b.y - origin.y) - (a.y - origin.y) * (b.x - origin.x)
}

fn region_len(region: &[bool], w: usize, h: usize) -> Option<usize> {
    let len = w.checked_mul(h)?;
    (w > 0 && h > 0 && region.len() >= len).then_some(len)
}

/// region の境界 pixel だけから、単位正方形の 4 隅を集める。
fn boundary_corners(region: &[bool], w: usize, h: usize) -> (Vec<Point>, usize) {
    let mut points = Vec::new();
    let mut count = 0;
    for y in 0..h {
        let row = y * w;
        for x in 0..w {
            if !region[row + x] {
                continue;
            }
            count += 1;
            let boundary = x == 0
                || x + 1 == w
                || y == 0
                || y + 1 == h
                || !region[row + x - 1]
                || !region[row + x + 1]
                || !region[row - w + x]
                || !region[row + w + x];
            if boundary {
                let x0 = x as f64;
                let y0 = y as f64;
                points.extend([
                    Point { x: x0, y: y0 },
                    Point { x: x0 + 1.0, y: y0 },
                    Point { x: x0, y: y0 + 1.0 },
                    Point {
                        x: x0 + 1.0,
                        y: y0 + 1.0,
                    },
                ]);
            }
        }
    }
    (points, count)
}

fn convex_hull(mut points: Vec<Point>) -> Vec<Point> {
    points.sort_by(|a, b| a.x.total_cmp(&b.x).then_with(|| a.y.total_cmp(&b.y)));
    points.dedup();
    if points.len() <= 2 {
        return points;
    }

    let mut lower = Vec::with_capacity(points.len());
    for &point in &points {
        while lower.len() >= 2
            && cross(lower[lower.len() - 2], lower[lower.len() - 1], point) <= 0.0
        {
            lower.pop();
        }
        lower.push(point);
    }

    let mut upper = Vec::with_capacity(points.len());
    for &point in points.iter().rev() {
        while upper.len() >= 2
            && cross(upper[upper.len() - 2], upper[upper.len() - 1], point) <= 0.0
        {
            upper.pop();
        }
        upper.push(point);
    }

    lower.pop();
    upper.pop();
    lower.extend(upper);
    lower
}

fn max_support_index(points: &[Point], direction: Point) -> usize {
    points
        .iter()
        .enumerate()
        .max_by(|(_, a), (_, b)| a.dot(direction).total_cmp(&b.dot(direction)))
        .map_or(0, |(idx, _)| idx)
}

/// 凸包の辺方向が反時計回りに進むのに合わせ、支持点も高々 1 周だけ進める。
fn advance_support(points: &[Point], mut idx: usize, direction: Point) -> usize {
    const EPS: f64 = 1.0e-10;
    for _ in 0..points.len().saturating_sub(1) {
        let next = (idx + 1) % points.len();
        if points[next].dot(direction) + EPS >= points[idx].dot(direction) {
            idx = next;
        } else {
            break;
        }
    }
    idx
}

#[derive(Debug, Clone, Copy)]
struct RectFit {
    center: Point,
    half_w: f64,
    half_h: f64,
    rotation_rad: f64,
}

fn min_area_rect(hull: &[Point]) -> Option<RectFit> {
    if hull.len() < 3 {
        return None;
    }

    let first_edge = Point {
        x: hull[1].x - hull[0].x,
        y: hull[1].y - hull[0].y,
    };
    let first_len = first_edge.x.hypot(first_edge.y);
    if !(first_len > 0.0) {
        return None;
    }
    let first_u = Point {
        x: first_edge.x / first_len,
        y: first_edge.y / first_len,
    };
    let first_v = Point {
        x: -first_u.y,
        y: first_u.x,
    };
    let mut max_u_idx = max_support_index(hull, first_u);
    let mut min_u_idx = max_support_index(
        hull,
        Point {
            x: -first_u.x,
            y: -first_u.y,
        },
    );
    let mut max_v_idx = max_support_index(hull, first_v);
    let mut min_v_idx = max_support_index(
        hull,
        Point {
            x: -first_v.x,
            y: -first_v.y,
        },
    );

    let mut best: Option<(f64, RectFit)> = None;
    for edge_idx in 0..hull.len() {
        let a = hull[edge_idx];
        let b = hull[(edge_idx + 1) % hull.len()];
        let edge = Point {
            x: b.x - a.x,
            y: b.y - a.y,
        };
        let edge_len = edge.x.hypot(edge.y);
        if !(edge_len > 0.0) {
            continue;
        }
        let u = Point {
            x: edge.x / edge_len,
            y: edge.y / edge_len,
        };
        let v = Point { x: -u.y, y: u.x };
        let neg_u = Point { x: -u.x, y: -u.y };
        let neg_v = Point { x: -v.x, y: -v.y };

        max_u_idx = advance_support(hull, max_u_idx, u);
        min_u_idx = advance_support(hull, min_u_idx, neg_u);
        max_v_idx = advance_support(hull, max_v_idx, v);
        min_v_idx = advance_support(hull, min_v_idx, neg_v);

        let max_u = hull[max_u_idx].dot(u);
        let min_u = -hull[min_u_idx].dot(neg_u);
        let max_v = hull[max_v_idx].dot(v);
        let min_v = -hull[min_v_idx].dot(neg_v);
        let width = max_u - min_u;
        let height = max_v - min_v;
        let area = width * height;
        if !(area > 0.0 && area.is_finite()) {
            continue;
        }

        if best.is_none_or(|(best_area, _)| area < best_area) {
            let center_u = (min_u + max_u) * 0.5;
            let center_v = (min_v + max_v) * 0.5;
            best = Some((
                area,
                RectFit {
                    center: Point {
                        x: u.x * center_u + v.x * center_v,
                        y: u.y * center_u + v.y * center_v,
                    },
                    half_w: width * 0.5,
                    half_h: height * 0.5,
                    rotation_rad: u.y.atan2(u.x),
                },
            ));
        }
    }
    best.map(|(_, rect)| rect)
}

fn normalize_rect(mut rect: RectFit) -> RectFit {
    while rect.rotation_rad < -FRAC_PI_4 {
        rect.rotation_rad += FRAC_PI_2;
        std::mem::swap(&mut rect.half_w, &mut rect.half_h);
    }
    while rect.rotation_rad >= FRAC_PI_4 {
        rect.rotation_rad -= FRAC_PI_2;
        std::mem::swap(&mut rect.half_w, &mut rect.half_h);
    }
    rect
}

fn axis_aligned_rect(points: &[Point]) -> RectFit {
    let min_x = points.iter().map(|p| p.x).fold(f64::INFINITY, f64::min);
    let max_x = points.iter().map(|p| p.x).fold(f64::NEG_INFINITY, f64::max);
    let min_y = points.iter().map(|p| p.y).fold(f64::INFINITY, f64::min);
    let max_y = points.iter().map(|p| p.y).fold(f64::NEG_INFINITY, f64::max);
    RectFit {
        center: Point {
            x: (min_x + max_x) * 0.5,
            y: (min_y + max_y) * 0.5,
        },
        half_w: (max_x - min_x) * 0.5,
        half_h: (max_y - min_y) * 0.5,
        rotation_rad: 0.0,
    }
}

pub fn fit_rect(region: &[bool], w: usize, h: usize, opt: FitOptions) -> Option<FittedShape> {
    region_len(region, w, h)?;
    let (boundary, region_count) = boundary_corners(region, w, h);
    if region_count <= 1 {
        return None;
    }
    let hull = convex_hull(boundary);
    let mut rect = normalize_rect(min_area_rect(&hull)?);
    if rect.rotation_rad.to_degrees().abs() <= f64::from(opt.angle_snap_deg.max(0.0)) {
        rect = axis_aligned_rect(&hull);
    }

    let shape_area = 4.0 * rect.half_w * rect.half_h;
    let coverage = region_count as f64 / shape_area;
    if !coverage.is_finite() || coverage < f64::from(opt.min_coverage) {
        return None;
    }

    let outset = f64::from(opt.outset.max(0.0));
    Some(FittedShape::Rect {
        center: (rect.center.x as f32, rect.center.y as f32),
        half_w: (rect.half_w + outset) as f32,
        half_h: (rect.half_h + outset) as f32,
        rotation_rad: rect.rotation_rad as f32,
    })
}

pub fn fit_ellipse(
    region: &[bool],
    w: usize,
    h: usize,
    opt: FitOptions,
    circle: bool,
) -> Option<FittedShape> {
    let len = region_len(region, w, h)?;
    let mut count = 0_u64;
    let mut mean_x = 0.0_f64;
    let mut mean_y = 0.0_f64;
    let mut m2_xx = 0.0_f64;
    let mut m2_yy = 0.0_f64;
    let mut m2_xy = 0.0_f64;

    for (idx, inside) in region[..len].iter().copied().enumerate() {
        if !inside {
            continue;
        }
        let x = (idx % w) as f64 + 0.5;
        let y = (idx / w) as f64 + 0.5;
        count += 1;
        let n = count as f64;
        let dx = x - mean_x;
        let dy = y - mean_y;
        mean_x += dx / n;
        mean_y += dy / n;
        m2_xx += dx * (x - mean_x);
        m2_yy += dy * (y - mean_y);
        m2_xy += dx * (y - mean_y);
    }
    if count <= 1 {
        return None;
    }

    let n = count as f64;
    let sigma_xx = m2_xx / n;
    let sigma_yy = m2_yy / n;
    let sigma_xy = m2_xy / n;
    let delta = ((sigma_xx - sigma_yy).powi(2) + 4.0 * sigma_xy * sigma_xy).sqrt();
    let lambda_1 = ((sigma_xx + sigma_yy + delta) * 0.5).max(0.0);
    let lambda_2 = ((sigma_xx + sigma_yy - delta) * 0.5).max(0.0);
    if !(lambda_1 > 0.0 && lambda_2 > 0.0) {
        return None;
    }

    let mut rx = 2.0 * lambda_1.sqrt();
    let mut ry = 2.0 * lambda_2.sqrt();
    let mut rotation = 0.5 * (2.0 * sigma_xy).atan2(sigma_xx - sigma_yy);
    while rotation < -FRAC_PI_2 {
        rotation += PI;
    }
    while rotation >= FRAC_PI_2 {
        rotation -= PI;
    }

    let shape_area = PI * rx * ry;
    let coverage = n / shape_area;
    if !coverage.is_finite() || coverage < f64::from(opt.min_coverage) {
        return None;
    }

    let near_circle = rx / ry < f64::from(opt.near_circle_ratio.max(1.0));
    if circle || near_circle {
        let radius = (rx * ry).sqrt();
        rx = radius;
        ry = radius;
        rotation = 0.0;
    }

    let outset = f64::from(opt.outset.max(0.0));
    Some(FittedShape::Ellipse {
        center: (mean_x as f32, mean_y as f32),
        rx: (rx + outset) as f32,
        ry: (ry + outset) as f32,
        rotation_rad: rotation as f32,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raster_rect(
        w: usize,
        h: usize,
        center: (f64, f64),
        half_w: f64,
        half_h: f64,
        rotation: f64,
    ) -> Vec<bool> {
        let (sin, cos) = rotation.sin_cos();
        (0..w * h)
            .map(|idx| {
                let dx = (idx % w) as f64 + 0.5 - center.0;
                let dy = (idx / w) as f64 + 0.5 - center.1;
                let x = cos * dx + sin * dy;
                let y = -sin * dx + cos * dy;
                x.abs() <= half_w && y.abs() <= half_h
            })
            .collect()
    }

    fn raster_ellipse(
        w: usize,
        h: usize,
        center: (f64, f64),
        rx: f64,
        ry: f64,
        rotation: f64,
    ) -> Vec<bool> {
        let (sin, cos) = rotation.sin_cos();
        (0..w * h)
            .map(|idx| {
                let dx = (idx % w) as f64 + 0.5 - center.0;
                let dy = (idx / w) as f64 + 0.5 - center.1;
                let x = cos * dx + sin * dy;
                let y = -sin * dx + cos * dy;
                x * x / (rx * rx) + y * y / (ry * ry) <= 1.0
            })
            .collect()
    }

    fn assert_close(actual: f32, expected: f32, tolerance: f32) {
        assert!(
            (actual - expected).abs() <= tolerance,
            "actual={actual}, expected={expected}, tolerance={tolerance}"
        );
    }

    fn angle_error_deg(actual: f32, expected: f32) -> f32 {
        let mut delta = (actual - expected).to_degrees();
        while delta < -90.0 {
            delta += 180.0;
        }
        while delta >= 90.0 {
            delta -= 180.0;
        }
        delta.abs()
    }

    #[test]
    fn axis_aligned_rect_recovers_center_extents_and_zero_rotation() {
        let (w, h) = (240, 180);
        let mut region = vec![false; w * h];
        for y in 45..125 {
            for x in 30..190 {
                region[y * w + x] = true;
            }
        }
        let Some(FittedShape::Rect {
            center,
            half_w,
            half_h,
            rotation_rad,
        }) = fit_rect(&region, w, h, FitOptions::default())
        else {
            panic!("rectangle should fit");
        };
        assert_close(center.0, 110.0, 1.0);
        assert_close(center.1, 85.0, 1.0);
        assert_close(half_w, 80.0, 1.0);
        assert_close(half_h, 40.0, 1.0);
        assert_eq!(rotation_rad, 0.0);
    }

    #[test]
    fn rotated_rects_recover_angle_and_area() {
        let (w, h) = (420, 360);
        for angle_deg in [20.0_f64, 35.0] {
            let angle = angle_deg.to_radians();
            let region = raster_rect(w, h, (210.0, 180.0), 110.0, 55.0, angle);
            let Some(FittedShape::Rect {
                half_w,
                half_h,
                rotation_rad,
                ..
            }) = fit_rect(&region, w, h, FitOptions::default())
            else {
                panic!("rotated rectangle should fit");
            };
            assert!(
                angle_error_deg(rotation_rad, angle as f32) <= 1.0,
                "angle={angle_deg}, fitted={}deg",
                rotation_rad.to_degrees()
            );
            let fitted_area = 4.0 * half_w * half_h;
            let expected_area = 4.0 * 110.0 * 55.0;
            assert!(
                ((fitted_area / expected_area) - 1.0).abs() <= 0.02,
                "angle={angle_deg}, fitted_area={fitted_area}, expected_area={expected_area}"
            );
        }
    }

    #[test]
    fn rect_with_internal_text_holes_fits_the_outer_rect() {
        let (w, h) = (260, 180);
        let mut region = vec![false; w * h];
        for y in 45..135 {
            for x in 30..230 {
                region[y * w + x] = true;
            }
        }
        for y in 70..110 {
            for x in 80..95 {
                region[y * w + x] = false;
            }
            for x in 130..150 {
                region[y * w + x] = false;
            }
        }
        let Some(FittedShape::Rect {
            center,
            half_w,
            half_h,
            rotation_rad,
        }) = fit_rect(&region, w, h, FitOptions::default())
        else {
            panic!("rectangle with holes should fit");
        };
        assert_close(center.0, 130.0, 1.0);
        assert_close(center.1, 90.0, 1.0);
        assert_close(half_w, 100.0, 1.0);
        assert_close(half_h, 45.0, 1.0);
        assert_eq!(rotation_rad, 0.0);
    }

    #[test]
    fn thin_leak_tail_fails_coverage_guard() {
        let (w, h) = (320, 220);
        let mut region = vec![false; w * h];
        for y in 70..150 {
            for x in 40..180 {
                region[y * w + x] = true;
            }
        }
        for y in 108..112 {
            for x in 180..290 {
                region[y * w + x] = true;
            }
        }
        assert_eq!(fit_rect(&region, w, h, FitOptions::default()), None);
    }

    #[test]
    fn circle_is_stabilized_with_zero_rotation_in_ellipse_mode() {
        let (w, h) = (220, 220);
        let region = raster_ellipse(w, h, (110.0, 110.0), 60.0, 60.0, 0.0);
        let Some(FittedShape::Ellipse {
            center,
            rx,
            ry,
            rotation_rad,
        }) = fit_ellipse(&region, w, h, FitOptions::default(), false)
        else {
            panic!("circle should fit");
        };
        assert_close(center.0, 110.0, 1.0);
        assert_close(center.1, 110.0, 1.0);
        assert_close(rx, 60.0, 1.0);
        assert_close(ry, 60.0, 1.0);
        assert_eq!(rotation_rad, 0.0);
    }

    #[test]
    fn rotated_ellipse_recovers_axes_and_angle() {
        let (w, h) = (300, 260);
        let angle = 30.0_f64.to_radians();
        let region = raster_ellipse(w, h, (150.0, 130.0), 80.0, 30.0, angle);
        let Some(FittedShape::Ellipse {
            center,
            rx,
            ry,
            rotation_rad,
        }) = fit_ellipse(&region, w, h, FitOptions::default(), false)
        else {
            panic!("ellipse should fit");
        };
        assert_close(center.0, 150.0, 1.0);
        assert_close(center.1, 130.0, 1.0);
        assert!((rx / 80.0 - 1.0).abs() <= 0.03, "rx={rx}");
        assert!((ry / 30.0 - 1.0).abs() <= 0.03, "ry={ry}");
        assert!(
            angle_error_deg(rotation_rad, angle as f32) <= 2.0,
            "rotation={}deg",
            rotation_rad.to_degrees()
        );
    }

    #[test]
    fn empty_and_single_pixel_regions_do_not_fit() {
        let (w, h) = (8, 8);
        let empty = vec![false; w * h];
        assert_eq!(fit_rect(&empty, w, h, FitOptions::default()), None);
        assert_eq!(
            fit_ellipse(&empty, w, h, FitOptions::default(), false),
            None
        );

        let mut one = empty;
        one[3 * w + 4] = true;
        assert_eq!(fit_rect(&one, w, h, FitOptions::default()), None);
        assert_eq!(fit_ellipse(&one, w, h, FitOptions::default(), false), None);
    }

    /// 5 度刻みで一周ぶん確かめる。凸包の向きと rotating calipers の支持点の進め方が
    /// ずれると、特定の角度でだけ外接しない矩形が返る。1 角度ずつのテストでは通って
    /// しまうので、包含そのものを不変条件として押さえる。
    #[test]
    fn rotated_rect_fit_always_encloses_the_region() {
        let (w, h) = (400, 320);
        for step in 0..36 {
            let angle = f64::from(step) * 5.0_f64.to_radians();
            let region = raster_rect(w, h, (200.0, 160.0), 75.0, 35.0, angle);
            let region_count = region.iter().filter(|on| **on).count();
            let Some(FittedShape::Rect {
                center,
                half_w,
                half_h,
                rotation_rad,
            }) = fit_rect(&region, w, h, FitOptions::default())
            else {
                panic!("rect should fit at {} deg", angle.to_degrees());
            };
            let (sin, cos) = f64::from(rotation_rad).sin_cos();
            for (idx, inside) in region.iter().copied().enumerate() {
                if !inside {
                    continue;
                }
                let dx = (idx % w) as f64 + 0.5 - f64::from(center.0);
                let dy = (idx / w) as f64 + 0.5 - f64::from(center.1);
                let u = cos * dx + sin * dy;
                let v = -sin * dx + cos * dy;
                assert!(
                    u.abs() <= f64::from(half_w) + 1.0 && v.abs() <= f64::from(half_h) + 1.0,
                    "pixel outside fitted rect at {} deg: u={u}, v={v}, half=({half_w}, {half_h})",
                    angle.to_degrees()
                );
            }
            let area = 4.0 * f64::from(half_w) * f64::from(half_h);
            assert!(
                area <= region_count as f64 * 1.25,
                "fitted rect too large at {} deg: area={area}, region={region_count}",
                angle.to_degrees()
            );
        }
    }

    /// 楕円も同じく一周ぶん。固有ベクトルの符号や象限を取り違えると、特定の角度でだけ
    /// 軸が入れ替わる。
    #[test]
    fn rotated_ellipse_fit_tracks_the_angle_all_the_way_around() {
        let (w, h) = (360, 320);
        for step in 0..36 {
            let angle = f64::from(step) * 5.0_f64.to_radians();
            let region = raster_ellipse(w, h, (180.0, 160.0), 70.0, 28.0, angle);
            let Some(FittedShape::Ellipse {
                rx,
                ry,
                rotation_rad,
                ..
            }) = fit_ellipse(&region, w, h, FitOptions::default(), false)
            else {
                panic!("ellipse should fit at {} deg", angle.to_degrees());
            };
            assert!((rx / 70.0 - 1.0).abs() <= 0.05, "rx={rx}");
            assert!((ry / 28.0 - 1.0).abs() <= 0.05, "ry={ry}");
            assert!(
                angle_error_deg(rotation_rad, angle as f32) <= 3.0,
                "rotation={}deg at {}deg",
                rotation_rad.to_degrees(),
                angle.to_degrees()
            );
        }
    }
}
