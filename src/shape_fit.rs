//! 連結ビットマップ領域の内側へ、クリック位置を含む単純な図形を当てはめる純関数。
//!
//! 探索は縮小マスクで候補を絞り、原寸マスクを走査するのは確定した候補の 1 回だけにする。
//! 座標は画像 pixel 座標で、各 pixel の中心は `(x + 0.5, y + 0.5)` とする。

use std::f64::consts::{FRAC_PI_2, FRAC_PI_4, PI};

const COARSE_LONG_EDGE: usize = 512;
const MIN_SHAPE_RADIUS: f64 = 2.0;

pub const DEFAULT_ANGLE_SNAP_DEG: f32 = 2.0;
pub const DEFAULT_NEAR_CIRCLE_RATIO: f32 = 1.05;
pub const DEFAULT_MAX_HOLE_AREA_RATIO: f32 = 0.10;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FitOptions {
    pub angle_snap_deg: f32,
    pub near_circle_ratio: f32,
    pub outset: f32,
    /// region 面積に対して、この比率以下の背景成分だけを内部の欠けとして埋める。
    pub max_hole_area_ratio: f32,
}

impl Default for FitOptions {
    fn default() -> Self {
        Self {
            angle_snap_deg: DEFAULT_ANGLE_SNAP_DEG,
            near_circle_ratio: DEFAULT_NEAR_CIRCLE_RATIO,
            outset: 0.0,
            max_hole_area_ratio: DEFAULT_MAX_HOLE_AREA_RATIO,
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

#[derive(Debug, Clone, Copy)]
struct Bounds {
    x0: usize,
    y0: usize,
    x1: usize,
    y1: usize,
}

impl Bounds {
    fn width(self) -> usize {
        self.x1 - self.x0
    }

    fn height(self) -> usize {
        self.y1 - self.y0
    }
}

fn region_len(region: &[bool], w: usize, h: usize) -> Option<usize> {
    let len = w.checked_mul(h)?;
    (w > 0 && h > 0 && region.len() >= len).then_some(len)
}

fn region_bounds(region: &[bool], w: usize, h: usize) -> Option<Bounds> {
    let mut x0 = w;
    let mut y0 = h;
    let mut x1 = 0;
    let mut y1 = 0;
    for y in 0..h {
        let row = y * w;
        for x in 0..w {
            if region[row + x] {
                x0 = x0.min(x);
                y0 = y0.min(y);
                x1 = x1.max(x + 1);
                y1 = y1.max(y + 1);
            }
        }
    }
    (x0 < x1 && y0 < y1).then_some(Bounds { x0, y0, x1, y1 })
}

fn mark_exterior_component(
    region: &[bool],
    exterior: &mut [bool],
    w: usize,
    h: usize,
    seed_x: usize,
    seed_y: usize,
) -> usize {
    if region[seed_y * w + seed_x] || exterior[seed_y * w + seed_x] {
        return 0;
    }
    let fill_span = |exterior: &mut [bool], y: usize, x: usize| {
        let row = y * w;
        let mut start = x;
        while start > 0 && !region[row + start - 1] && !exterior[row + start - 1] {
            start -= 1;
        }
        let mut end = x + 1;
        while end < w && !region[row + end] && !exterior[row + end] {
            end += 1;
        }
        exterior[row + start..row + end].fill(true);
        (start, end)
    };

    let (start, end) = fill_span(exterior, seed_y, seed_x);
    let mut marked = end - start;
    let mut spans = vec![(seed_y, start, end)];
    while let Some((y, start, end)) = spans.pop() {
        for adjacent_y in [y.checked_sub(1), y.checked_add(1).filter(|yy| *yy < h)]
            .into_iter()
            .flatten()
        {
            let row = adjacent_y * w;
            // 前景を 4 近傍で扱うとき、背景は 8 近傍で外部との接続を判定する。
            // 隣接行を左右へ 1px 広げ、斜めだけで接する背景も同じ成分へ含める。
            let adjacent_start = start.saturating_sub(1);
            let adjacent_end = end.saturating_add(1).min(w);
            let mut x = adjacent_start;
            while x < adjacent_end {
                if region[row + x] || exterior[row + x] {
                    x += 1;
                    continue;
                }
                let (next_start, next_end) = fill_span(exterior, adjacent_y, x);
                marked += next_end - next_start;
                spans.push((adjacent_y, next_start, next_end));
                x = next_end;
            }
        }
    }
    marked
}

/// 外周から背景の 8 近傍で届かず、かつ region 面積に対して小さい背景成分だけを
/// 内部の欠けとみなして region へ取り込む。
fn fill_internal_holes(region: &[bool], w: usize, h: usize, max_hole_area_ratio: f32) -> Vec<bool> {
    let mut filled = region[..w * h].to_vec();
    let max_hole_area_ratio = max_hole_area_ratio.clamp(0.0, 1.0);
    if max_hole_area_ratio <= 0.0 {
        return filled;
    }
    let region_area = region[..w * h].iter().filter(|inside| **inside).count();
    let max_hole_area = (region_area as f64 * f64::from(max_hole_area_ratio)).floor() as usize;
    if max_hole_area == 0 {
        return filled;
    }

    let mut exterior = vec![false; w * h];
    for x in 0..w {
        mark_exterior_component(region, &mut exterior, w, h, x, 0);
        if h > 1 {
            mark_exterior_component(region, &mut exterior, w, h, x, h - 1);
        }
    }
    for y in 1..h.saturating_sub(1) {
        mark_exterior_component(region, &mut exterior, w, h, 0, y);
        if w > 1 {
            mark_exterior_component(region, &mut exterior, w, h, w - 1, y);
        }
    }

    // 外部でない背景を成分ごとに数える。大きい成分の画素一覧を保持すると 60MP 級で
    // 多量の追加メモリを使うため、まず面積だけを数え、小さい成分だけ 2 回目の走査で埋める。
    for idx in 0..w * h {
        if region[idx] || exterior[idx] {
            continue;
        }
        let seed_x = idx % w;
        let seed_y = idx / w;
        let area = mark_exterior_component(region, &mut exterior, w, h, seed_x, seed_y);
        if area <= max_hole_area {
            mark_exterior_component(region, &mut filled, w, h, seed_x, seed_y);
        }
    }
    filled
}

struct ScaledRegion {
    mask: Vec<bool>,
    w: usize,
    h: usize,
    seed: (usize, usize),
}

fn make_scaled_region(
    region: &[bool],
    w: usize,
    bounds: Bounds,
    seed: (usize, usize),
) -> ScaledRegion {
    let longest = bounds.width().max(bounds.height());
    let scale = if longest > COARSE_LONG_EDGE {
        COARSE_LONG_EDGE as f64 / longest as f64
    } else {
        1.0
    };
    let dw = ((bounds.width() as f64 * scale).ceil() as usize).max(1);
    let dh = ((bounds.height() as f64 * scale).ceil() as usize).max(1);
    let mut mask = vec![false; dw * dh];
    for y in 0..dh {
        let source_y =
            bounds.y0 + (((y as f64 + 0.5) / scale).floor() as usize).min(bounds.height() - 1);
        for x in 0..dw {
            let source_x =
                bounds.x0 + (((x as f64 + 0.5) / scale).floor() as usize).min(bounds.width() - 1);
            mask[y * dw + x] = region[source_y * w + source_x];
        }
    }
    let seed_x = (((seed.0 as f64 + 0.5 - bounds.x0 as f64) * scale).floor() as usize).min(dw - 1);
    let seed_y = (((seed.1 as f64 + 0.5 - bounds.y0 as f64) * scale).floor() as usize).min(dh - 1);
    mask[seed_y * dw + seed_x] = true;
    ScaledRegion {
        mask,
        w: dw,
        h: dh,
        seed: (seed_x, seed_y),
    }
}

#[derive(Debug, Clone, Copy)]
struct RectCandidate {
    center: (f64, f64),
    half_w: f64,
    half_h: f64,
    rotation_rad: f64,
    area: f64,
}

fn projected_grid_range(
    bounds: Bounds,
    seed: (f64, f64),
    u: (f64, f64),
    v: (f64, f64),
    u_scale: f64,
) -> (isize, isize, isize, isize) {
    let corners = [
        (bounds.x0 as f64, bounds.y0 as f64),
        (bounds.x1 as f64, bounds.y0 as f64),
        (bounds.x0 as f64, bounds.y1 as f64),
        (bounds.x1 as f64, bounds.y1 as f64),
    ];
    let mut min_u = f64::INFINITY;
    let mut max_u = f64::NEG_INFINITY;
    let mut min_v = f64::INFINITY;
    let mut max_v = f64::NEG_INFINITY;
    for corner in corners {
        let dx = corner.0 - seed.0;
        let dy = corner.1 - seed.1;
        let pu = (dx * u.0 + dy * u.1) / u_scale;
        let pv = dx * v.0 + dy * v.1;
        min_u = min_u.min(pu);
        max_u = max_u.max(pu);
        min_v = min_v.min(pv);
        max_v = max_v.max(pv);
    }
    (
        min_u.floor() as isize - 1,
        max_u.ceil() as isize + 1,
        min_v.floor() as isize - 1,
        max_v.ceil() as isize + 1,
    )
}

fn largest_rect_at_angle(
    region: &[bool],
    w: usize,
    h: usize,
    bounds: Bounds,
    seed: (usize, usize),
    angle: f64,
) -> Option<RectCandidate> {
    let (sin, cos) = angle.sin_cos();
    let u = (cos, sin);
    let v = (-sin, cos);
    let seed_point = (seed.0 as f64 + 0.5, seed.1 as f64 + 0.5);
    let (min_k, max_k, min_l, max_l) = projected_grid_range(bounds, seed_point, u, v, 1.0);
    let grid_w = usize::try_from(max_k - min_k + 1).ok()?;
    let grid_h = usize::try_from(max_l - min_l + 1).ok()?;
    let seed_col = usize::try_from(-min_k).ok()?;
    let seed_row = usize::try_from(-min_l).ok()?;
    if seed_col >= grid_w || seed_row >= grid_h {
        return None;
    }

    let mut heights = vec![0usize; grid_w];
    let mut left_less = vec![None; grid_w];
    let mut right_less = vec![grid_w; grid_w];
    let mut stack = Vec::with_capacity(grid_w);
    let mut best: Option<RectCandidate> = None;

    for row in 0..grid_h {
        let l = min_l + row as isize;
        for (col, height) in heights.iter_mut().enumerate() {
            let k = min_k + col as isize;
            let source_x = seed_point.0 + k as f64 * u.0 + l as f64 * v.0;
            let source_y = seed_point.1 + k as f64 * u.1 + l as f64 * v.1;
            let inside = source_x >= 0.0
                && source_y >= 0.0
                && source_x < w as f64
                && source_y < h as f64
                && region[source_y.floor() as usize * w + source_x.floor() as usize];
            *height = if inside { *height + 1 } else { 0 };
        }

        stack.clear();
        for col in 0..grid_w {
            while stack
                .last()
                .is_some_and(|previous: &usize| heights[*previous] >= heights[col])
            {
                stack.pop();
            }
            left_less[col] = stack.last().copied();
            stack.push(col);
        }
        stack.clear();
        for col in (0..grid_w).rev() {
            while stack
                .last()
                .is_some_and(|previous: &usize| heights[*previous] >= heights[col])
            {
                stack.pop();
            }
            right_less[col] = stack.last().copied().unwrap_or(grid_w);
            stack.push(col);
        }

        for col in 0..grid_w {
            let rect_h = heights[col];
            if rect_h == 0 {
                continue;
            }
            let left = left_less[col].map_or(0, |idx| idx + 1);
            let right = right_less[col];
            let top = row + 1 - rect_h;
            if !(left <= seed_col && seed_col < right && top <= seed_row && seed_row <= row) {
                continue;
            }
            let rect_w = right - left;
            let area = (rect_w * rect_h) as f64;
            if best.is_some_and(|candidate| candidate.area >= area) {
                continue;
            }
            let center_k = min_k as f64 + (left + right - 1) as f64 * 0.5;
            let center_l = min_l as f64 + (top + row) as f64 * 0.5;
            best = Some(RectCandidate {
                center: (
                    seed_point.0 + center_k * u.0 + center_l * v.0,
                    seed_point.1 + center_k * u.1 + center_l * v.1,
                ),
                half_w: rect_w as f64 * 0.5,
                half_h: rect_h as f64 * 0.5,
                rotation_rad: angle,
                area,
            });
        }
    }
    best
}

fn normalize_rect_for_output(mut candidate: RectCandidate, angle_snap_deg: f64) -> RectCandidate {
    while candidate.rotation_rad < -FRAC_PI_4 {
        candidate.rotation_rad += FRAC_PI_2;
        std::mem::swap(&mut candidate.half_w, &mut candidate.half_h);
    }
    while candidate.rotation_rad >= FRAC_PI_4 {
        candidate.rotation_rad -= FRAC_PI_2;
        std::mem::swap(&mut candidate.half_w, &mut candidate.half_h);
    }
    if candidate.rotation_rad.to_degrees().abs() <= angle_snap_deg {
        candidate.rotation_rad = 0.0;
        return candidate;
    }
    if candidate.half_w < candidate.half_h {
        std::mem::swap(&mut candidate.half_w, &mut candidate.half_h);
        candidate.rotation_rad += FRAC_PI_2;
    }
    while candidate.rotation_rad < -FRAC_PI_2 {
        candidate.rotation_rad += PI;
    }
    while candidate.rotation_rad >= FRAC_PI_2 {
        candidate.rotation_rad -= PI;
    }
    candidate
}

fn rect_candidate_is_better(candidate: RectCandidate, best: Option<RectCandidate>) -> bool {
    let Some(best) = best else {
        return true;
    };
    candidate.area > best.area + 1.0e-6
        || ((candidate.area - best.area).abs() <= 1.0e-6
            && candidate.rotation_rad.abs() < best.rotation_rad.abs())
}

pub fn fit_rect(
    region: &[bool],
    w: usize,
    h: usize,
    seed: (usize, usize),
    opt: FitOptions,
) -> Option<FittedShape> {
    region_len(region, w, h)?;
    if seed.0 >= w || seed.1 >= h || !region[seed.1 * w + seed.0] {
        return None;
    }
    let filled = fill_internal_holes(region, w, h, opt.max_hole_area_ratio);
    let bounds = region_bounds(&filled, w, h)?;
    let coarse = make_scaled_region(&filled, w, bounds, seed);
    let coarse_bounds = region_bounds(&coarse.mask, coarse.w, coarse.h)?;

    let mut best = None;
    for angle_deg in (-22..=22)
        .map(|step| f64::from(step) * 2.0)
        .chain([-45.0, 0.0])
    {
        if let Some(candidate) = largest_rect_at_angle(
            &coarse.mask,
            coarse.w,
            coarse.h,
            coarse_bounds,
            coarse.seed,
            angle_deg.to_radians(),
        ) && rect_candidate_is_better(candidate, best)
        {
            best = Some(candidate);
        }
    }
    let coarse_best = best?;

    // 2 度刻みで選んだ近傍だけを縮小マスク上で詰める。原寸を複数回回さない。
    let mut refined = Some(coarse_best);
    for step in -8..=8 {
        let angle = coarse_best.rotation_rad + f64::from(step) * 0.25_f64.to_radians();
        if let Some(candidate) = largest_rect_at_angle(
            &coarse.mask,
            coarse.w,
            coarse.h,
            coarse_bounds,
            coarse.seed,
            angle,
        ) && rect_candidate_is_better(candidate, refined)
        {
            refined = Some(candidate);
        }
    }
    let mut final_angle = refined?.rotation_rad;
    if final_angle.to_degrees().abs() <= f64::from(opt.angle_snap_deg.max(0.0)) {
        final_angle = 0.0;
    }
    let final_candidate = largest_rect_at_angle(&filled, w, h, bounds, seed, final_angle)?;
    if final_candidate.half_w < MIN_SHAPE_RADIUS || final_candidate.half_h < MIN_SHAPE_RADIUS {
        return None;
    }
    let candidate =
        normalize_rect_for_output(final_candidate, f64::from(opt.angle_snap_deg.max(0.0)));
    let outset = f64::from(opt.outset.max(0.0));
    Some(FittedShape::Rect {
        center: (candidate.center.0 as f32, candidate.center.1 as f32),
        half_w: (candidate.half_w + outset) as f32,
        half_h: (candidate.half_h + outset) as f32,
        rotation_rad: candidate.rotation_rad as f32,
    })
}

#[derive(Debug, Clone, Copy)]
struct EllipseCandidate {
    center: (f64, f64),
    major: f64,
    minor: f64,
    rotation_rad: f64,
    ratio: f64,
    area: f64,
}

fn normalize_ellipse_angle(mut angle: f64) -> f64 {
    while angle < -FRAC_PI_2 {
        angle += PI;
    }
    while angle >= FRAC_PI_2 {
        angle -= PI;
    }
    angle
}

/// major 軸方向だけ `ratio` 倍で sampling し、異方スケール後の最大内接円を求める。
fn largest_ellipse_at_transform(
    region: &[bool],
    w: usize,
    h: usize,
    bounds: Bounds,
    seed: (usize, usize),
    angle: f64,
    ratio: f64,
) -> Option<EllipseCandidate> {
    if !(ratio >= 1.0 && ratio.is_finite()) {
        return None;
    }
    let (sin, cos) = angle.sin_cos();
    let u = (cos, sin);
    let v = (-sin, cos);
    let seed_point = (seed.0 as f64 + 0.5, seed.1 as f64 + 0.5);
    let (min_k, max_k, min_l, max_l) = projected_grid_range(bounds, seed_point, u, v, ratio);
    let inner_w = usize::try_from(max_k - min_k + 1).ok()?;
    let inner_h = usize::try_from(max_l - min_l + 1).ok()?;
    let grid_w = inner_w.checked_add(2)?;
    let grid_h = inner_h.checked_add(2)?;
    let mut barrier = vec![true; grid_w.checked_mul(grid_h)?];

    for row in 0..inner_h {
        let l = min_l + row as isize;
        for col in 0..inner_w {
            let k = min_k + col as isize;
            let source_x = seed_point.0 + k as f64 * ratio * u.0 + l as f64 * v.0;
            let source_y = seed_point.1 + k as f64 * ratio * u.1 + l as f64 * v.1;
            let inside = source_x >= 0.0
                && source_y >= 0.0
                && source_x < w as f64
                && source_y < h as f64
                && region[source_y.floor() as usize * w + source_x.floor() as usize];
            barrier[(row + 1) * grid_w + col + 1] = !inside;
        }
    }

    let distance_sq = crate::mask_db::squared_distance_map(&barrier, grid_w, grid_h);
    let mut best = None;
    for row in 0..inner_h {
        let l = min_l + row as isize;
        for col in 0..inner_w {
            let idx = (row + 1) * grid_w + col + 1;
            if barrier[idx] {
                continue;
            }
            let k = min_k + col as isize;
            let radius = (distance_sq[idx].sqrt() - 0.01).max(0.0);
            if (k as f64).hypot(l as f64) > radius {
                continue;
            }
            let area = PI * ratio * radius * radius;
            if best.is_some_and(|candidate: EllipseCandidate| candidate.area >= area) {
                continue;
            }
            best = Some(EllipseCandidate {
                center: (
                    seed_point.0 + k as f64 * ratio * u.0 + l as f64 * v.0,
                    seed_point.1 + k as f64 * ratio * u.1 + l as f64 * v.1,
                ),
                major: ratio * radius,
                minor: radius,
                rotation_rad: normalize_ellipse_angle(angle),
                ratio,
                area,
            });
        }
    }
    best
}

fn ellipse_candidate_is_better(
    candidate: EllipseCandidate,
    best: Option<EllipseCandidate>,
) -> bool {
    let Some(best) = best else {
        return true;
    };
    candidate.area > best.area + 1.0e-6
        || ((candidate.area - best.area).abs() <= 1.0e-6 && candidate.ratio < best.ratio)
}

pub fn fit_ellipse(
    region: &[bool],
    w: usize,
    h: usize,
    seed: (usize, usize),
    opt: FitOptions,
    circle: bool,
) -> Option<FittedShape> {
    region_len(region, w, h)?;
    if seed.0 >= w || seed.1 >= h || !region[seed.1 * w + seed.0] {
        return None;
    }
    let filled = fill_internal_holes(region, w, h, opt.max_hole_area_ratio);
    let bounds = region_bounds(&filled, w, h)?;

    let candidate = if circle {
        largest_ellipse_at_transform(&filled, w, h, bounds, seed, 0.0, 1.0)?
    } else {
        let coarse = make_scaled_region(&filled, w, bounds, seed);
        let coarse_bounds = region_bounds(&coarse.mask, coarse.w, coarse.h)?;
        let ratios = [1.0, 1.25, 1.5, 2.0, 3.0, 4.0, 6.0];
        let mut best = None;
        for ratio in ratios {
            let angle_steps = if ratio == 1.0 { 1 } else { 12 };
            for step in 0..angle_steps {
                let angle = f64::from(step) * 15.0_f64.to_radians();
                if let Some(candidate) = largest_ellipse_at_transform(
                    &coarse.mask,
                    coarse.w,
                    coarse.h,
                    coarse_bounds,
                    coarse.seed,
                    angle,
                    ratio,
                ) && ellipse_candidate_is_better(candidate, best)
                {
                    best = Some(candidate);
                }
            }
        }
        let coarse_best = best?;

        // 軸比と角度は縮小マスク上でのみ細分化する。原寸の距離計算は最良組 1 回。
        let mut refined = Some(coarse_best);
        // 粗い候補の中間を 0.1 刻みで埋める。比率に比例した刻みにすると 2 と 3 の
        // どちらを起点にしたかで 2.5 を飛び越すため、通常域は固定刻みにする。
        let ratio_step = if coarse_best.ratio >= 5.0 { 0.2 } else { 0.1 };
        for angle_step in -15..=15 {
            let angle = coarse_best.rotation_rad + f64::from(angle_step) * 1.0_f64.to_radians();
            for ratio_step_idx in -5..=5 {
                let ratio = (coarse_best.ratio + f64::from(ratio_step_idx) * ratio_step).max(1.0);
                if let Some(candidate) = largest_ellipse_at_transform(
                    &coarse.mask,
                    coarse.w,
                    coarse.h,
                    coarse_bounds,
                    coarse.seed,
                    angle,
                    ratio,
                ) && ellipse_candidate_is_better(candidate, refined)
                {
                    refined = Some(candidate);
                }
            }
        }
        let first_refined = refined?;
        let mut refined = Some(first_refined);
        for angle_step in -2..=2 {
            let angle = first_refined.rotation_rad + f64::from(angle_step) * 0.25_f64.to_radians();
            for ratio_step_idx in -5..=5 {
                let ratio = (first_refined.ratio + f64::from(ratio_step_idx) * 0.02).max(1.0);
                if let Some(candidate) = largest_ellipse_at_transform(
                    &coarse.mask,
                    coarse.w,
                    coarse.h,
                    coarse_bounds,
                    coarse.seed,
                    angle,
                    ratio,
                ) && ellipse_candidate_is_better(candidate, refined)
                {
                    refined = Some(candidate);
                }
            }
        }
        let refined = refined?;
        largest_ellipse_at_transform(
            &filled,
            w,
            h,
            bounds,
            seed,
            refined.rotation_rad,
            refined.ratio,
        )?
    };

    if candidate.major < MIN_SHAPE_RADIUS || candidate.minor < MIN_SHAPE_RADIUS {
        return None;
    }
    let near_circle = circle || candidate.ratio < f64::from(opt.near_circle_ratio.max(1.0));
    let (rx, ry, rotation) = if near_circle {
        let radius = (candidate.major * candidate.minor).sqrt();
        (radius, radius, 0.0)
    } else {
        (
            candidate.major,
            candidate.minor,
            normalize_ellipse_angle(candidate.rotation_rad),
        )
    };
    let outset = f64::from(opt.outset.max(0.0));
    Some(FittedShape::Ellipse {
        center: (candidate.center.0 as f32, candidate.center.1 as f32),
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

    fn union_into(target: &mut [bool], other: &[bool]) {
        for (target, other) in target.iter_mut().zip(other) {
            *target |= *other;
        }
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
            region[y * w + 30..y * w + 190].fill(true);
        }
        let Some(FittedShape::Rect {
            center,
            half_w,
            half_h,
            rotation_rad,
        }) = fit_rect(&region, w, h, (80, 80), FitOptions::default())
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
            }) = fit_rect(&region, w, h, (210, 180), FitOptions::default())
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
    fn clicked_arm_of_crossing_rectangles_is_selected() {
        let (w, h) = (440, 360);
        let center = (220.0, 180.0);
        let half_w = 145.0;
        let half_h = 18.0;
        let selected_angle = 0.0_f64;
        let mut region = raster_rect(w, h, center, half_w, half_h, selected_angle);
        union_into(
            &mut region,
            &raster_rect(w, h, center, half_w, half_h, 35.0_f64.to_radians()),
        );
        let Some(FittedShape::Rect {
            half_w: fitted_half_w,
            half_h: fitted_half_h,
            rotation_rad,
            ..
        }) = fit_rect(&region, w, h, (105, 180), FitOptions::default())
        else {
            panic!("clicked arm should fit");
        };
        assert!(angle_error_deg(rotation_rad, selected_angle as f32) <= 2.0);
        let expected_area = 4.0 * half_w * half_h;
        let fitted_area = 4.0 * f64::from(fitted_half_w) * f64::from(fitted_half_h);
        assert!(
            (fitted_area / expected_area - 1.0).abs() <= 0.15,
            "fitted_area={fitted_area}, expected_area={expected_area}"
        );
    }

    #[test]
    fn rect_with_internal_text_holes_fits_the_outer_rect_at_default_gap_tolerance() {
        let (w, h) = (260, 180);
        let mut region = vec![false; w * h];
        for y in 45..135 {
            region[y * w + 30..y * w + 230].fill(true);
        }
        for y in 70..110 {
            region[y * w + 80..y * w + 95].fill(false);
            region[y * w + 130..y * w + 150].fill(false);
        }
        let Some(FittedShape::Rect {
            center,
            half_w,
            half_h,
            rotation_rad,
        }) = fit_rect(&region, w, h, (50, 90), FitOptions::default())
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
    fn zero_gap_tolerance_does_not_fill_internal_text_holes() {
        let (w, h) = (80, 60);
        let mut region = vec![false; w * h];
        for y in 10..50 {
            region[y * w + 10..y * w + 70].fill(true);
        }
        for y in 22..38 {
            region[y * w + 32..y * w + 38].fill(false);
        }

        assert_eq!(fill_internal_holes(&region, w, h, 0.0), region);
    }

    #[test]
    fn diagonally_connected_background_remains_exterior() {
        let (w, h) = (5, 5);
        let mut region = vec![true; w * h];
        region[0] = false;
        region[w + 1] = false;

        let filled = fill_internal_holes(&region, w, h, 0.5);
        assert!(!filled[0]);
        assert!(
            !filled[w + 1],
            "background connected to the edge only diagonally must not be filled"
        );
    }

    #[test]
    fn large_enclosed_gap_does_not_merge_parallel_rectangles() {
        let (w, h) = (280, 200);
        let mut region = vec![false; w * h];
        for y in 30..170 {
            region[y * w + 30..y * w + 80].fill(true);
            region[y * w + 200..y * w + 250].fill(true);
        }
        for y in 30..36 {
            region[y * w + 80..y * w + 200].fill(true);
        }
        for y in 164..170 {
            region[y * w + 80..y * w + 200].fill(true);
        }

        let Some(FittedShape::Rect {
            center,
            half_w,
            half_h,
            rotation_rad,
        }) = fit_rect(&region, w, h, (55, 100), FitOptions::default())
        else {
            panic!("clicked rectangle should fit");
        };
        assert_close(center.0, 55.0, 1.0);
        assert_close(center.1, 100.0, 1.0);
        assert_close(half_w, 25.0, 1.0);
        assert_close(half_h, 70.0, 1.0);
        assert_eq!(rotation_rad, 0.0);
        let fitted_area = 4.0 * half_w * half_h;
        let enclosing_area = 220.0 * 140.0;
        assert!(fitted_area < enclosing_area * 0.4);
    }

    #[test]
    fn thin_leak_tail_is_ignored_while_body_fits() {
        let (w, h) = (320, 220);
        let mut region = vec![false; w * h];
        for y in 70..150 {
            region[y * w + 40..y * w + 180].fill(true);
        }
        for y in 108..112 {
            region[y * w + 180..y * w + 290].fill(true);
        }
        let Some(FittedShape::Rect {
            center,
            half_w,
            half_h,
            rotation_rad,
        }) = fit_rect(&region, w, h, (80, 100), FitOptions::default())
        else {
            panic!("main body should fit");
        };
        assert_close(center.0, 110.0, 1.0);
        assert_close(center.1, 110.0, 1.0);
        assert_close(half_w, 70.0, 1.0);
        assert_close(half_h, 40.0, 1.0);
        assert_eq!(rotation_rad, 0.0);
    }

    #[test]
    fn overlapping_circles_are_selected_from_the_clicked_center() {
        let (w, h) = (260, 220);
        let centers = [(105.0, 110.0), (155.0, 110.0)];
        let mut region = raster_ellipse(w, h, centers[0], 40.0, 40.0, 0.0);
        union_into(
            &mut region,
            &raster_ellipse(w, h, centers[1], 40.0, 40.0, 0.0),
        );
        for center in centers {
            let Some(FittedShape::Ellipse {
                center: fitted_center,
                rx,
                ry,
                rotation_rad,
            }) = fit_ellipse(
                &region,
                w,
                h,
                (center.0 as usize, center.1 as usize),
                FitOptions::default(),
                true,
            )
            else {
                panic!("clicked circle should fit");
            };
            assert_close(fitted_center.0, center.0 as f32, 2.0);
            assert_close(fitted_center.1, center.1 as f32, 2.0);
            assert!((rx / 40.0 - 1.0).abs() <= 0.08, "rx={rx}");
            assert!((ry / 40.0 - 1.0).abs() <= 0.08, "ry={ry}");
            assert_eq!(rotation_rad, 0.0);
        }
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
        }) = fit_ellipse(&region, w, h, (110, 110), FitOptions::default(), false)
        else {
            panic!("circle should fit");
        };
        assert_close(center.0, 110.0, 2.0);
        assert_close(center.1, 110.0, 2.0);
        assert!((rx / 60.0 - 1.0).abs() <= 0.03, "rx={rx}");
        assert!((ry / 60.0 - 1.0).abs() <= 0.03, "ry={ry}");
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
        }) = fit_ellipse(&region, w, h, (150, 130), FitOptions::default(), false)
        else {
            panic!("ellipse should fit");
        };
        assert_close(center.0, 150.0, 2.0);
        assert_close(center.1, 130.0, 2.0);
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
        assert_eq!(fit_rect(&empty, w, h, (4, 3), FitOptions::default()), None);
        assert_eq!(
            fit_ellipse(&empty, w, h, (4, 3), FitOptions::default(), false),
            None
        );

        let mut one = empty;
        one[3 * w + 4] = true;
        assert_eq!(fit_rect(&one, w, h, (4, 3), FitOptions::default()), None);
        assert_eq!(
            fit_ellipse(&one, w, h, (4, 3), FitOptions::default(), false),
            None
        );
    }

    /// 5 度刻みで一周ぶん確かめる。最大内接矩形は raster 境界の丸めで外周の
    /// ごく一部を落としうるため、旧外接実装の厳密包含ではなく 98% の包含を不変条件とする。
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
            }) = fit_rect(&region, w, h, (200, 160), FitOptions::default())
            else {
                panic!("rect should fit at {} deg", angle.to_degrees());
            };
            let (sin, cos) = f64::from(rotation_rad).sin_cos();
            let covered = region
                .iter()
                .copied()
                .enumerate()
                .filter(|(idx, inside)| {
                    if !inside {
                        return false;
                    }
                    let dx = (idx % w) as f64 + 0.5 - f64::from(center.0);
                    let dy = (idx / w) as f64 + 0.5 - f64::from(center.1);
                    let u = cos * dx + sin * dy;
                    let v = -sin * dx + cos * dy;
                    u.abs() <= f64::from(half_w) + 0.5 && v.abs() <= f64::from(half_h) + 0.5
                })
                .count();
            assert!(
                covered as f64 / region_count as f64 >= 0.98,
                "coverage={} at {}deg",
                covered as f64 / region_count as f64,
                angle.to_degrees()
            );
        }
    }

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
            }) = fit_ellipse(&region, w, h, (180, 160), FitOptions::default(), false)
            else {
                panic!("ellipse should fit at {} deg", angle.to_degrees());
            };
            assert!(
                (rx / 70.0 - 1.0).abs() <= 0.05,
                "rx={rx}, ry={ry}, rotation={} at {}deg",
                rotation_rad.to_degrees(),
                angle.to_degrees(),
            );
            assert!(
                (ry / 28.0 - 1.0).abs() <= 0.05,
                "ry={ry} at {}deg",
                angle.to_degrees()
            );
            assert!(
                angle_error_deg(rotation_rad, angle as f32) <= 3.0,
                "rotation={}deg at {}deg",
                rotation_rad.to_degrees(),
                angle.to_degrees()
            );
        }
    }
}
