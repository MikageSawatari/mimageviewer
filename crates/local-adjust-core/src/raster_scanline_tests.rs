//! Frozen pre-scanline oracle. Keep its geometry, bbox, and float expressions independent
//! of the production helpers so differential tests also catch preparation regressions.
use super::*;
use std::cell::Cell;
use std::time::Instant;

thread_local! {
    static VISITED_PIXELS: Cell<usize> = const { Cell::new(0) };
    static INTERVAL_TESTS: Cell<usize> = const { Cell::new(0) };
    static BOUNDARY_PROBES: Cell<usize> = const { Cell::new(0) };
}

pub(super) fn record_visited_pixels(count: usize) {
    VISITED_PIXELS.with(|visited| visited.set(visited.get() + count));
}

pub(super) fn record_interval_test() {
    INTERVAL_TESTS.with(|count| count.set(count.get() + 1));
}

pub(super) fn record_boundary_probe() {
    BOUNDARY_PROBES.with(|count| count.set(count.get() + 1));
}

fn take_interval_tests() -> usize {
    INTERVAL_TESTS.with(|count| count.replace(0))
}

fn take_boundary_probes() -> usize {
    BOUNDARY_PROBES.with(|count| count.replace(0))
}

fn take_visited_pixels() -> usize {
    VISITED_PIXELS.with(|visited| visited.replace(0))
}

fn old_rasterize<T: Copy>(
    pixels: &mut [T],
    width: usize,
    height: usize,
    shape: MaskShape,
    value: T,
) {
    match shape {
        MaskShape::Line {
            p0, p1, thickness, ..
        } => {
            let dx = p1[0] - p0[0];
            let dy = p1[1] - p0[1];
            let len = (dx * dx + dy * dy).sqrt().max(1e-6);
            let nx = -dy / len;
            let ny = dx / len;
            let half = thickness.max(1.0) * 0.5;
            let corners = vec![
                [p0[0] + nx * half, p0[1] + ny * half],
                [p1[0] + nx * half, p1[1] + ny * half],
                [p1[0] - nx * half, p1[1] - ny * half],
                [p0[0] - nx * half, p0[1] - ny * half],
            ];
            old_polygon(pixels, width, height, &corners, value);
        }
        MaskShape::Rect {
            center,
            half_w,
            half_h,
            rotation_rad,
            ..
        } => {
            let half_w = half_w.max(0.5);
            let half_h = half_h.max(0.5);
            let (s, c) = rotation_rad.sin_cos();
            let mut corners = Vec::with_capacity(4);
            for [x, y] in [
                [-half_w, -half_h],
                [half_w, -half_h],
                [half_w, half_h],
                [-half_w, half_h],
            ] {
                corners.push([center[0] + x * c - y * s, center[1] + x * s + y * c]);
            }
            old_polygon(pixels, width, height, &corners, value);
        }
        MaskShape::Ellipse {
            center,
            rx,
            ry,
            rotation_rad,
            ..
        } => {
            if width == 0 || height == 0 {
                return;
            }
            let rx = rx.max(0.5);
            let ry = ry.max(0.5);
            let radius = rx.max(ry);
            let min_x = (center[0] - radius).floor().max(0.0) as usize;
            let max_x = (center[0] + radius).ceil().min(width as f32 - 1.0) as usize;
            let min_y = (center[1] - radius).floor().max(0.0) as usize;
            let max_y = (center[1] + radius).ceil().min(height as f32 - 1.0) as usize;
            let (s, c) = (-rotation_rad).sin_cos();
            for y in min_y..=max_y {
                if min_x <= max_x {
                    record_visited_pixels(max_x - min_x + 1);
                }
                for x in min_x..=max_x {
                    let dx = x as f32 + 0.5 - center[0];
                    let dy = y as f32 + 0.5 - center[1];
                    let lx = dx * c - dy * s;
                    let ly = dx * s + dy * c;
                    if (lx / rx).powi(2) + (ly / ry).powi(2) <= 1.0 {
                        pixels[y * width + x] = value;
                    }
                }
            }
        }
    }
}

fn old_polygon<T: Copy>(
    pixels: &mut [T],
    width: usize,
    height: usize,
    points: &[[f32; 2]],
    value: T,
) {
    if points.len() < 3 || width == 0 || height == 0 {
        return;
    }
    let min_x = points
        .iter()
        .map(|p| p[0])
        .fold(f32::INFINITY, f32::min)
        .floor()
        .max(0.0) as usize;
    let max_x = points
        .iter()
        .map(|p| p[0])
        .fold(f32::NEG_INFINITY, f32::max)
        .ceil()
        .min(width as f32 - 1.0) as usize;
    let min_y = points
        .iter()
        .map(|p| p[1])
        .fold(f32::INFINITY, f32::min)
        .floor()
        .max(0.0) as usize;
    let max_y = points
        .iter()
        .map(|p| p[1])
        .fold(f32::NEG_INFINITY, f32::max)
        .ceil()
        .min(height as f32 - 1.0) as usize;
    for y in min_y..=max_y {
        if min_x <= max_x {
            record_visited_pixels(max_x - min_x + 1);
        }
        for x in min_x..=max_x {
            let px = x as f32 + 0.5;
            let py = y as f32 + 0.5;
            let mut inside = false;
            let mut j = points.len() - 1;
            for i in 0..points.len() {
                let xi = points[i][0];
                let yi = points[i][1];
                let xj = points[j][0];
                let yj = points[j][1];
                let dy = yj - yi;
                let intersects = ((yi > py) != (yj > py))
                    && dy.abs() > 1e-6
                    && (px < (xj - xi) * (py - yi) / dy + xi);
                if intersects {
                    inside = !inside;
                }
                j = i;
            }
            if inside {
                pixels[y * width + x] = value;
            }
        }
    }
}

fn assert_old_and_new(width: usize, height: usize, shapes: &[MaskShape]) {
    let original: Vec<f32> = (0..width * height)
        .map(|i| match i % 5 {
            0 => -0.0,
            1 => f32::from_bits(0x7fc0_0042),
            2 => 0.49999997,
            3 => 0.5,
            _ => 0.9,
        })
        .collect();
    let mut expected = original.clone();
    let mut actual = original.clone();
    let base: Vec<bool> = original.iter().map(|v| *v >= 0.5).collect();
    let mut expected_bool = base.clone();
    let mut actual_bool = base.clone();
    for &shape in shapes {
        old_rasterize(
            &mut expected,
            width,
            height,
            shape,
            if shape.op().is_add() { 1.0 } else { 0.0 },
        );
        old_rasterize(
            &mut expected_bool,
            width,
            height,
            shape,
            shape.op().is_add(),
        );
    }
    rasterize_shapes_into(&mut actual, width, height, shapes);
    rasterize_shapes_into_bitmap(&mut actual_bool, width, height, shapes);
    for i in 0..expected.len() {
        assert_eq!(
            actual[i].to_bits(),
            expected[i].to_bits(),
            "float pixel ({}, {}) shapes={shapes:?}",
            i % width,
            i / width
        );
        assert_eq!(
            actual_bool[i], expected_bool[i],
            "bool pixel {i} shapes={shapes:?}"
        );
        assert_eq!(actual_bool[i], actual[i] >= 0.5, "float/bool pixel {i}");
        assert_eq!(
            rasterized_shapes_value_at(base[i], width, height, shapes, i % width, i / width),
            expected_bool[i],
            "point pixel {i} shapes={shapes:?}"
        );
    }
    for initial in [false, true] {
        assert_eq!(
            rasterized_shapes_value_at(initial, width, height, shapes, width, 0),
            initial
        );
        assert_eq!(
            rasterized_shapes_value_at(initial, width, height, shapes, 0, height),
            initial
        );
    }
}

fn line(p0: [f32; 2], p1: [f32; 2], thickness: f32) -> MaskShape {
    MaskShape::Line {
        op: ShapeOp::Add,
        kind: LineKind::Diagonal,
        p0,
        p1,
        thickness,
    }
}

fn rect(center: [f32; 2], half_w: f32, half_h: f32, rotation_rad: f32) -> MaskShape {
    MaskShape::Rect {
        op: ShapeOp::Add,
        center,
        half_w,
        half_h,
        rotation_rad,
    }
}

fn ellipse(center: [f32; 2], rx: f32, ry: f32, rotation_rad: f32) -> MaskShape {
    MaskShape::Ellipse {
        op: ShapeOp::Add,
        center,
        rx,
        ry,
        rotation_rad,
    }
}

#[test]
fn scanline_matches_old_at_centers_edges_minimum_sizes_and_outside_image() {
    let shapes = [
        line([0.5, 0.5], [31.5, 23.5], 0.0),
        line([31.5, 23.5], [0.5, 0.5], 1.0),
        line([4.5, 6.0], [25.5, 6.0], 1.0),
        line([6.0, -4.5], [6.0, 30.5], 1.0),
        line([5.5, 5.5], [5.5, 5.5], 0.0),
        line([-20.0, -20.0], [-4.0, -4.0], 1.0),
        line([40.0, 40.0], [50.0, 50.0], 1.0),
        rect([8.5, 8.5], 4.0, 3.0, 0.0),
        rect([8.5, 8.5], 4.0, 3.0, std::f32::consts::FRAC_PI_2),
        rect([8.5, 8.5], 0.0, -1.0, 0.0),
        rect([-0.2, 4.7], 9.2, 0.5, 0.7),
        rect([36.2, 24.7], 9.2, 0.5, -0.7),
        ellipse([8.5, 8.5], 0.0, -1.0, 0.0),
        ellipse([8.5, 8.5], 6.0, 2.0, 0.0),
        ellipse([8.5, 8.5], 6.0, 0.5, std::f32::consts::FRAC_PI_2),
        ellipse([8.5, 8.5], 15.0, 0.5, 0.713),
        ellipse([-0.5, 0.5], 12.0, 2.0, -0.713),
        ellipse([35.0, 28.0], 12.0, 2.0, 0.713),
    ];
    for shape in shapes {
        for (width, height) in [(0, 0), (0, 7), (7, 0), (1, 1), (1, 25), (33, 1), (33, 25)] {
            assert_old_and_new(width, height, &[shape]);
            assert_old_and_new(width, height, &[shape.with_op(ShapeOp::Subtract)]);
        }
    }
    assert_old_and_new(33, 25, &shapes);
    let mixed: Vec<_> = shapes
        .iter()
        .enumerate()
        .map(|(i, s)| {
            s.with_op(if i % 2 == 0 {
                ShapeOp::Add
            } else {
                ShapeOp::Subtract
            })
        })
        .collect();
    assert_old_and_new(33, 25, &mixed);
}

#[test]
fn scanline_matches_old_tiny_dy_odd_crossings_and_nan_crossings() {
    // Exactly one edge is excluded by the historical 1e-6 dy threshold. Its
    // remaining crossing paints from the clipped left bbox, even outside the quad.
    for points in [
        [[1.5, 0.4999997], [8.5, 0.5000003], [8.5, 6.5], [1.5, 6.5]],
        [[f32::NEG_INFINITY, 0.5], [8.5, 0.5], [8.5, 6.5], [1.5, 6.5]],
        [[1.5, 0.5], [8.5, 0.5], [8.5, 6.5], [1.5, 6.5]],
    ] {
        let mut old = vec![false; 10 * 8];
        let mut new = old.clone();
        old_polygon(&mut old, 10, 8, &points, true);
        fill_polygon_alpha(&mut new, 10, 8, &points, true);
        assert_eq!(new, old, "{points:?}");
    }
}

#[test]
fn scanline_matches_old_nonfinite_ellipse_compatibility() {
    for shape in [
        ellipse([8.5, 8.5], f32::INFINITY, 3.0, 0.0),
        ellipse([8.5, 8.5], 3.0, f32::INFINITY, 0.7),
        ellipse([8.5, 8.5], f32::NAN, 3.0, 0.0),
        ellipse([f32::NAN, 8.5], 3.0, 3.0, 0.0),
        ellipse([8.5, 8.5], 3.0, 3.0, f32::INFINITY),
    ] {
        assert_old_and_new(19, 17, &[shape]);
    }
}

struct Random(u64);
impl Random {
    fn unit(&mut self) -> f32 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (self.0 >> 40) as f32 / 16777216.0
    }
    fn range(&mut self, min: f32, max: f32) -> f32 {
        min + (max - min) * self.unit()
    }
}

#[test]
fn scanline_matches_old_randomized_shapes_and_operation_order() {
    let mut random = Random(0x1673_5ca1_1e55);
    for batch in 0..90 {
        let width = 1 + batch % 65;
        let height = 1 + batch % 49;
        let mut shapes = Vec::new();
        for i in 0..9 {
            let center = [random.range(-30.0, 90.0), random.range(-30.0, 75.0)];
            let angle = random.range(-20.0, 20.0);
            let a = random.range(-1.0, 55.0);
            let b = random.range(-1.0, 30.0);
            let shape = match i % 3 {
                0 => line(
                    center,
                    [random.range(-30.0, 90.0), random.range(-30.0, 75.0)],
                    b,
                ),
                1 => rect(center, a, b, angle),
                _ => ellipse(center, a, b, angle),
            }
            .with_op(if random.unit() < 0.5 {
                ShapeOp::Add
            } else {
                ShapeOp::Subtract
            });
            assert_old_and_new(width, height, &[shape]);
            shapes.push(shape);
        }
        assert_old_and_new(width, height, &shapes);
    }
}

#[test]
fn scanline_visits_thin_shape_spans_instead_of_the_bounding_box() {
    for shape in [
        line([0.0, 0.0], [799.0, 599.0], 1.0),
        rect([400.0, 300.0], 495.0, 0.5, 0.6435),
        ellipse([400.0, 300.0], 495.0, 0.5, 0.6435),
    ] {
        let mut old = vec![false; 800 * 600];
        let mut new = old.clone();
        take_visited_pixels();
        old_rasterize(&mut old, 800, 600, shape, true);
        let old_visits = take_visited_pixels();
        rasterize_shapes_into_bitmap(&mut new, 800, 600, &[shape]);
        let new_visits = take_visited_pixels();
        assert_eq!(new, old);
        assert!(
            new_visits < old_visits / 50,
            "old={old_visits} new={new_visits} shape={shape:?}"
        );
    }
}

#[test]
fn scanline_matches_old_clipped_ellipses_and_rejects_empty_intervals() {
    // The square/strip bbox overlaps the entire image, but the circle does not.
    // This used to leave almost the whole bbox for per-pixel evaluation.
    let outside = ellipse([-216.0, -216.0], 300.0, 300.0, 0.0);
    assert_old_and_new(96, 72, &[outside]);
    let mut pixels = vec![false; 96 * 72];
    take_visited_pixels();
    take_interval_tests();
    take_boundary_probes();
    rasterize_shapes_into_bitmap(&mut pixels, 96, 72, &[outside]);
    let visits = take_visited_pixels();
    let intervals = take_interval_tests();
    let probes = take_boundary_probes();
    assert!(pixels.iter().all(|v| !v));
    assert_eq!(visits, 0);
    assert!(intervals <= 72);
    assert!(probes < 96 * 72);

    // Tangency/near-tangency, extreme aspect ratios, both local-coordinate
    // directions, and exact-center rows exercise accept/reject/bisect paths.
    for angle in [
        0.0,
        -0.0,
        0.713,
        -0.713,
        std::f32::consts::FRAC_PI_2,
        3.1415927,
    ] {
        for shape in [
            ellipse([-216.0, -216.0], 305.47012, 305.47012, angle),
            ellipse([32.5, -39.5], 40.0, 40.0, angle),
            ellipse([32.5, -39.500004], 40.0, 40.0, angle),
            ellipse([32.5, 24.5], 5000.0, 0.5, angle),
            ellipse([32.5, 24.5], 0.5, 5000.0, angle),
            ellipse([32.5, 24.5], 5000.0, 5000.0, angle),
        ] {
            assert_old_and_new(65, 49, &[shape]);
            assert_old_and_new(65, 49, &[shape.with_op(ShapeOp::Subtract)]);
        }
    }
}

/// Run explicitly with e.g.
/// cargo test -p local-adjust-core --release scanline_benchmark_8000x6000 -- --ignored --nocapture
/// Counts old bbox predicates vs new writes / single-pixel predicates. Interval
/// classifications and binary-search probes are reported separately. No product launch.
#[test]
#[ignore = "explicit 48MP before/after raster benchmark"]
fn scanline_benchmark_8000x6000() {
    for count in [1, 10] {
        let shapes: Vec<_> = (0..count)
            .map(|i| {
                line(
                    [0.0, i as f32 * 2.0],
                    [7999.0, 5999.0 - i as f32 * 2.0],
                    1.0,
                )
            })
            .collect();
        let mut old = vec![false; 8000 * 6000];
        let mut new = old.clone();
        take_visited_pixels();
        let started = Instant::now();
        for &shape in &shapes {
            old_rasterize(&mut old, 8000, 6000, shape, true);
        }
        let old_time = started.elapsed();
        let old_visits = take_visited_pixels();
        take_boundary_probes();
        let started = Instant::now();
        rasterize_shapes_into_bitmap(&mut new, 8000, 6000, &shapes);
        let new_time = started.elapsed();
        let new_visits = take_visited_pixels();
        let probes = take_boundary_probes();
        assert_eq!(new, old);
        println!(
            "8000x6000 diagonal thin Line x{count}: old={old_time:?} bbox_pixels={old_visits}, scanline={new_time:?} covered_pixels={new_visits} boundary_probes={probes}"
        );
        assert!(new_visits < old_visits / 1000);
    }
}

/// Explicitly exercise the 48MP clipped-empty counterexample and a thin rotated ellipse.
#[test]
#[ignore = "explicit 48MP before/after ellipse raster benchmark"]
fn scanline_ellipse_benchmark_8000x6000() {
    for (name, shape) in [
        (
            "clipped empty circle",
            ellipse([-18000.0, -18000.0], 25000.0, 25000.0, 0.0),
        ),
        (
            "thin rotated ellipse",
            ellipse([4000.0, 3000.0], 4950.0, 0.5, 0.6435),
        ),
    ] {
        let mut old = vec![false; 8000 * 6000];
        let mut new = old.clone();
        take_visited_pixels();
        let started = Instant::now();
        old_rasterize(&mut old, 8000, 6000, shape, true);
        let old_time = started.elapsed();
        let old_visits = take_visited_pixels();
        take_interval_tests();
        take_boundary_probes();
        let started = Instant::now();
        rasterize_shapes_into_bitmap(&mut new, 8000, 6000, &[shape]);
        let new_time = started.elapsed();
        let new_visits = take_visited_pixels();
        let intervals = take_interval_tests();
        let probes = take_boundary_probes();
        assert_eq!(new, old);
        println!(
            "8000x6000 {name}: old={old_time:?} bbox_pixels={old_visits}, scanline={new_time:?} writes_or_leaf_predicates={new_visits} interval_tests={intervals} boundary_probes={probes}"
        );
        assert!(new_visits < old_visits / 1000);
        assert!(intervals + probes < old_visits / 10);
        if name == "clipped empty circle" {
            assert_eq!(new_visits, 0);
            assert!(intervals <= 6000);
        }
    }
}
