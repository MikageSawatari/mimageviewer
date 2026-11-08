//! CPU depth preparation, run exclusively by the depth worker.
//!
//! The OpenCV spike is the numerical reference: cubic A=-0.75, REFLECT box
//! filters, an elliptical dilation, and REFLECT_101 Gaussian blur. Gamma/focus
//! belong to rendering and are deliberately absent here.

use half::f16;

pub(crate) struct PreparedDepth {
    pub width: u32,
    pub height: u32,
    pub values: Vec<f16>,
    pub median: f32,
}

pub(crate) fn prepare_depth(
    disparity: &[f32],
    input_width: u32,
    input_height: u32,
    guide_rgba: &[u8],
    guide_width: u32,
    guide_height: u32,
) -> Result<PreparedDepth, String> {
    let input_len = pixel_count(input_width, input_height)?;
    let guide_len = pixel_count(guide_width, guide_height)?;
    if disparity.len() != input_len || guide_len.checked_mul(4) != Some(guide_rgba.len()) {
        return Err("Depth input dimensions do not match pixel buffers".into());
    }
    if !disparity.iter().all(|value| value.is_finite()) {
        return Err("Depth disparity contains non-finite values".into());
    }
    let (width, height) = (guide_width as usize, guide_height as usize);
    let normalized = normalize_disparity(disparity);
    let resized = resize_cubic(
        &normalized,
        input_width as usize,
        input_height as usize,
        width,
        height,
    );
    let gray: Vec<f32> = guide_rgba
        .chunks_exact(4)
        .map(|rgba| {
            // OpenCV's RGB8 -> grayscale rounds before conversion to float.
            let gray = (rgba[0] as u32 * 9798
                + rgba[1] as u32 * 19235
                + rgba[2] as u32 * 3735
                + (1 << 14))
                >> 15;
            gray as f32 / 255.0
        })
        .collect();
    let long_edge = width.max(height) as f64;
    let radius = (long_edge / 256.0).round_ties_even().max(2.0) as usize;
    let mut values = guided_filter(&gray, &resized, width, height, radius, 1e-3);
    for value in &mut values {
        *value = value.clamp(0.0, 1.0);
    }
    let scale = long_edge / 1600.0;
    let dilation = (3.0 * scale).round_ties_even() as usize;
    if dilation > 0 {
        values = dilate_ellipse(&values, width, height, dilation);
    }
    values = gaussian_blur(&values, width, height, 1.5 * scale);
    let median = percentile(&mut values.clone(), 0.5);
    Ok(PreparedDepth {
        width: guide_width,
        height: guide_height,
        values: values.into_iter().map(f16::from_f32).collect(),
        median,
    })
}

fn pixel_count(width: u32, height: u32) -> Result<usize, String> {
    if width == 0 || height == 0 {
        return Err("Depth dimensions must be nonzero".into());
    }
    (width as usize)
        .checked_mul(height as usize)
        .ok_or_else(|| "Depth dimensions overflow".into())
}

fn percentile(values: &mut [f32], fraction: f64) -> f32 {
    let position = (values.len() - 1) as f64 * fraction;
    let lower = position.floor() as usize;
    let upper = position.ceil() as usize;
    // Only two adjacent order statistics are needed, including for a megapixel
    // median. Preserve NumPy's linear interpolation without sorting every pixel.
    let (_, lo, greater) = values.select_nth_unstable_by(lower, f32::total_cmp);
    let lo = *lo as f64;
    let hi = if lower == upper {
        lo
    } else {
        *greater.iter().min_by(|a, b| a.total_cmp(b)).unwrap() as f64
    };
    (lo + (hi - lo) * (position - lower as f64)) as f32
}

fn normalize_disparity(values: &[f32]) -> Vec<f32> {
    let mut sorted = values.to_vec();
    let lo = percentile(&mut sorted, 0.01) as f64;
    let hi = percentile(&mut sorted, 0.99) as f64;
    if hi - lo < 1e-6 {
        return vec![0.0; values.len()];
    }
    values
        .iter()
        .map(|value| (((*value as f64 - lo) / (hi - lo)).clamp(0.0, 1.0)) as f32)
        .collect()
}

fn cubic_weights(x: f32) -> [f32; 4] {
    const A: f32 = -0.75;
    let w0 = ((A * (x + 1.0) - 5.0 * A) * (x + 1.0) + 8.0 * A) * (x + 1.0) - 4.0 * A;
    let w1 = ((A + 2.0) * x - (A + 3.0)) * x * x + 1.0;
    let t = 1.0 - x;
    let w2 = ((A + 2.0) * t - (A + 3.0)) * t * t + 1.0;
    [w0, w1, w2, 1.0 - w0 - w1 - w2]
}

fn resize_cubic(src: &[f32], sw: usize, sh: usize, dw: usize, dh: usize) -> Vec<f32> {
    let mut horizontal = vec![0.0; dw * sh];
    for x in 0..dw {
        let position = ((x as f64 + 0.5) * sw as f64 / dw as f64 - 0.5) as f32;
        let base = position.floor() as isize;
        let weights = cubic_weights(position - base as f32);
        for y in 0..sh {
            horizontal[y * dw + x] = weights
                .iter()
                .enumerate()
                .map(|(i, weight)| {
                    src[y * sw + (base + i as isize - 1).clamp(0, sw as isize - 1) as usize]
                        * weight
                })
                .sum();
        }
    }
    let mut result = vec![0.0; dw * dh];
    for y in 0..dh {
        let position = ((y as f64 + 0.5) * sh as f64 / dh as f64 - 0.5) as f32;
        let base = position.floor() as isize;
        let weights = cubic_weights(position - base as f32);
        for x in 0..dw {
            result[y * dw + x] = weights
                .iter()
                .enumerate()
                .map(|(i, weight)| {
                    horizontal[(base + i as isize - 1).clamp(0, sh as isize - 1) as usize * dw + x]
                        * weight
                })
                .sum();
        }
    }
    result
}

fn reflect(mut index: isize, length: usize, repeat_edge: bool) -> usize {
    if length == 1 {
        return 0;
    }
    let length = length as isize;
    let delta = isize::from(repeat_edge);
    while index < 0 || index >= length {
        index = if index < 0 {
            -index - delta
        } else {
            2 * length - index - 2 + delta
        };
    }
    index as usize
}

fn box_mean(src: &[f32], width: usize, height: usize, radius: usize) -> Vec<f32> {
    let size = (2 * radius + 1) as f64;
    let mut horizontal = vec![0.0f64; src.len()];
    for y in 0..height {
        let mut sum: f64 = (-(radius as isize)..=radius as isize)
            .map(|x| src[y * width + reflect(x, width, true)] as f64)
            .sum();
        for x in 0..width {
            horizontal[y * width + x] = sum;
            sum += src[y * width + reflect((x + radius + 1) as isize, width, true)] as f64
                - src[y * width + reflect(x as isize - radius as isize, width, true)] as f64;
        }
    }
    // Keep one vertical rolling sum per column, visiting contiguous rows. Each
    // column retains the same f64 add/subtract order as the strided version.
    let mut sums = vec![0.0f64; width];
    for y in -(radius as isize)..=radius as isize {
        let row = &horizontal[reflect(y, height, true) * width..][..width];
        for (sum, &value) in sums.iter_mut().zip(row) {
            *sum += value;
        }
    }
    let mut result = vec![0.0; src.len()];
    for (y, row) in result.chunks_exact_mut(width).enumerate() {
        for (out, &sum) in row.iter_mut().zip(&sums) {
            *out = (sum / (size * size)) as f32;
        }
        let add = &horizontal[reflect((y + radius + 1) as isize, height, true) * width..][..width];
        let remove =
            &horizontal[reflect(y as isize - radius as isize, height, true) * width..][..width];
        for ((sum, &add), &remove) in sums.iter_mut().zip(add).zip(remove) {
            *sum += add - remove;
        }
    }
    result
}

fn guided_filter(
    guide: &[f32],
    src: &[f32],
    width: usize,
    height: usize,
    radius: usize,
    eps: f32,
) -> Vec<f32> {
    let mi = box_mean(guide, width, height, radius);
    let ms = box_mean(src, width, height, radius);
    let product: Vec<_> = guide.iter().zip(src).map(|(a, b)| a * b).collect();
    let square: Vec<_> = guide.iter().map(|a| a * a).collect();
    let corr = box_mean(&product, width, height, radius);
    let variance = box_mean(&square, width, height, radius);
    let a: Vec<_> = (0..src.len())
        .map(|i| (corr[i] - mi[i] * ms[i]) / (variance[i] - mi[i] * mi[i] + eps))
        .collect();
    let b: Vec<_> = (0..src.len()).map(|i| ms[i] - a[i] * mi[i]).collect();
    let ma = box_mean(&a, width, height, radius);
    let mb = box_mean(&b, width, height, radius);
    (0..src.len()).map(|i| ma[i] * guide[i] + mb[i]).collect()
}

fn dilate_ellipse(src: &[f32], width: usize, height: usize, radius: usize) -> Vec<f32> {
    let radius = radius as isize;
    let mut offsets = Vec::new();
    for dy in -radius..=radius {
        let dx = (radius as f64 * (1.0 - (dy * dy) as f64 / (radius * radius) as f64).sqrt())
            .round_ties_even() as isize;
        for dx in -dx..=dx {
            offsets.push((dx, dy));
        }
    }
    let mut result = vec![0.0; src.len()];
    for y in 0..height {
        for x in 0..width {
            let mut value = f32::NEG_INFINITY;
            for &(dx, dy) in &offsets {
                let (sx, sy) = (x as isize + dx, y as isize + dy);
                // OpenCV's default morphology border is minus infinity.
                if sx >= 0 && sy >= 0 && sx < width as isize && sy < height as isize {
                    value = value.max(src[sy as usize * width + sx as usize]);
                }
            }
            result[y * width + x] = value;
        }
    }
    result
}

fn gaussian_blur(src: &[f32], width: usize, height: usize, sigma: f64) -> Vec<f32> {
    // OpenCV derives the automatic float32 kernel as cvRound(sigma*8+1)|1.
    let size = ((sigma * 8.0 + 1.0).round_ties_even() as usize) | 1;
    if size == 1 {
        return src.to_vec();
    }
    let radius = (size / 2) as isize;
    let mut kernel: Vec<f64> = (-radius..=radius)
        .map(|i| (-(i * i) as f64 / (2.0 * sigma * sigma)).exp())
        .collect();
    let sum: f64 = kernel.iter().sum();
    for value in &mut kernel {
        *value /= sum;
    }
    let kernel: Vec<f32> = kernel.into_iter().map(|v| v as f32).collect();
    // Reflect once into a small reusable row instead of branching for every
    // tap of every pixel. Convolution still adds taps in ascending kernel order.
    let reflected: Vec<_> = (0..width + size - 1)
        .map(|x| reflect(x as isize - radius, width, false))
        .collect();
    let mut padded = vec![0.0; reflected.len()];
    let mut horizontal = vec![0.0; src.len()];
    for (source, out) in src
        .chunks_exact(width)
        .zip(horizontal.chunks_exact_mut(width))
    {
        for (value, &x) in padded.iter_mut().zip(&reflected) {
            *value = source[x];
        }
        for (x, out) in out.iter_mut().enumerate() {
            *out = padded[x..x + size]
                .iter()
                .zip(&kernel)
                .map(|(&v, &k)| v * k)
                .sum();
        }
    }
    let mut result = vec![0.0; src.len()];
    for (y, out) in result.chunks_exact_mut(width).enumerate() {
        for (i, &weight) in kernel.iter().enumerate() {
            let row = &horizontal
                [reflect(y as isize + i as isize - radius, height, false) * width..][..width];
            for (out, &value) in out.iter_mut().zip(row) {
                *out += value * weight;
            }
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;

    #[derive(Deserialize)]
    struct Fixture {
        stages: Stages,
        cases: Vec<Case>,
        full_size: Case,
    }

    #[derive(Deserialize)]
    struct Stages {
        source: Vec<f32>,
        normalized: Vec<f32>,
        resized: Vec<f32>,
        gray: Vec<f32>,
        guided: Vec<f32>,
        dilated: Vec<f32>,
        blurred: Vec<f32>,
    }

    #[derive(Deserialize)]
    struct Case {
        width: u32,
        height: u32,
        sample_indices: Vec<usize>,
        values: Vec<f32>,
        median: f32,
    }

    fn fixture() -> Fixture {
        serde_json::from_str(include_str!("../tests/fixtures/depth/preprocess.json")).unwrap()
    }

    fn full_size_input() -> (Vec<f32>, Vec<u8>) {
        let disparity = (0..504_u32 * 504)
            .map(|i| {
                let (x, y) = (i % 504, i / 504);
                let mut n = i ^ 0x5A17C9E3;
                n = (n ^ (n >> 16)).wrapping_mul(0x7FEB352D);
                n = (n ^ (n >> 15)).wrapping_mul(0x846CA68B);
                n ^= n >> 16;
                ((3 * x + 5 * y) % 1024) as f32 / 1024.0
                    + (n & 255) as f32 / 4096.0
                    + if (200..320).contains(&x) && (140..360).contains(&y) {
                        0.75
                    } else {
                        0.0
                    }
            })
            .collect();
        let rgba = (0..1024_u32)
            .flat_map(|y| {
                (0..1536_u32).flat_map(move |x| {
                    [
                        ((x * 37 + y * 13) % 256) as u8,
                        ((x * 19 + y * 71) % 256) as u8,
                        ((x * 83 + y * 7) % 256) as u8,
                        255,
                    ]
                })
            })
            .collect();
        (disparity, rgba)
    }

    #[test]
    fn depth_full_size_504_to_1536_matches_opencv() {
        let expected = fixture().full_size;
        let (disparity, guide) = full_size_input();
        // Same worker boundary as production: never run image processing on UI.
        let actual = std::thread::Builder::new()
            .name("depth-estimation".into())
            .spawn(move || prepare_depth(&disparity, 504, 504, &guide, 1536, 1024).unwrap())
            .unwrap()
            .join()
            .unwrap();
        assert_eq!(
            (actual.width, actual.height),
            (expected.width, expected.height)
        );
        assert_eq!(actual.values.len(), 1536 * 1024);
        assert!(expected.sample_indices.len() >= 750);
        let samples: Vec<_> = expected
            .sample_indices
            .iter()
            .map(|&i| actual.values[i].to_f32())
            .collect();
        let max_error = samples
            .iter()
            .zip(&expected.values)
            .map(|(&a, &b)| (a - b).abs())
            .fold(0.0_f32, f32::max);
        let median_error = (actual.median - expected.median).abs();
        println!(
            "depth full-size OpenCV samples={} max_error={max_error:.8} median_error={median_error:.8}",
            samples.len()
        );
        // f16 rounding contributes <=1/4096; the remaining allowance covers
        // float32 cubic/filter accumulation differences from OpenCV SIMD.
        close(&samples, &expected.values, 3e-4);
        assert!(
            median_error < 2e-5,
            "median {} vs {}",
            actual.median,
            expected.median
        );
    }

    #[test]
    #[ignore = "explicit isolated worker timing in debug and dev-runtime profiles"]
    fn depth_preprocess_worker_timing_1536() {
        use std::hint::black_box;
        let (disparity, guide) = full_size_input(); // Excluded from timing.
        let times = std::thread::Builder::new()
            .name("depth-estimation".into())
            .spawn(move || {
                let run = || {
                    prepare_depth(
                        black_box(&disparity),
                        504,
                        504,
                        black_box(&guide),
                        1536,
                        1024,
                    )
                    .unwrap()
                };
                black_box(run()); // Warm up the same worker thread and allocations.
                let mut times = Vec::new();
                for _ in 0..3 {
                    let started = std::time::Instant::now();
                    let result = black_box(run());
                    times.push(started.elapsed().as_secs_f64() * 1000.0);
                    assert_eq!(result.values.len(), 1536 * 1024);
                    // Drop is outside the timed interval; all prepare_depth-owned
                    // scratch allocation/free, median and f16 conversion are inside.
                }
                times
            })
            .unwrap()
            .join()
            .unwrap();
        let mut sorted = times.clone();
        sorted.sort_by(f64::total_cmp);
        println!(
            "depth preprocessing 504x504 -> 1536x1024 worker=depth-estimation debug_assertions={} runs_ms={times:?} median_ms={:.3}",
            cfg!(debug_assertions),
            sorted[1]
        );
    }

    fn close(actual: &[f32], expected: &[f32], tolerance: f32) {
        assert_eq!(actual.len(), expected.len());
        let (index, error) = actual
            .iter()
            .zip(expected)
            .map(|(a, b)| (a - b).abs())
            .enumerate()
            .max_by(|a, b| a.1.total_cmp(&b.1))
            .unwrap();
        assert!(
            error <= tolerance,
            "index {index}: {} vs {}; error {error} > {tolerance}",
            actual[index],
            expected[index],
        );
    }

    #[test]
    fn spike_stages_match_opencv() {
        let stage = fixture().stages;
        close(&normalize_disparity(&stage.source), &stage.normalized, 2e-7);
        close(
            &resize_cubic(&stage.normalized, 4, 3, 13, 9),
            &stage.resized,
            2e-6,
        );
        close(
            &guided_filter(&stage.gray, &stage.resized, 13, 9, 2, 1e-3),
            &stage.guided,
            2e-6,
        );
        let clipped: Vec<_> = stage.guided.iter().map(|v| v.clamp(0.0, 1.0)).collect();
        close(&dilate_ellipse(&clipped, 13, 9, 3), &stage.dilated, 1e-7);
        close(
            &gaussian_blur(&stage.dilated, 13, 9, 1.5),
            &stage.blurred,
            3e-7,
        );
    }

    #[test]
    fn prepared_depth_matches_scaled_spike_including_edges() {
        let fixture = fixture();
        for case in fixture.cases {
            // Same deterministic RGB pattern as generate_fixtures.py. Generate
            // complete input buffers even when large-case expectations sample
            // only the borders and center; median still covers the full output.
            let rgba: Vec<_> = (0..case.height)
                .flat_map(|y| {
                    (0..case.width).flat_map(move |x| {
                        [
                            ((x * 37 + y * 13) % 256) as u8,
                            ((x * 19 + y * 71) % 256) as u8,
                            ((x * 83 + y * 7) % 256) as u8,
                            255,
                        ]
                    })
                })
                .collect();
            let actual =
                prepare_depth(&fixture.stages.source, 4, 3, &rgba, case.width, case.height)
                    .unwrap();
            assert_eq!((actual.width, actual.height), (case.width, case.height));
            assert_eq!(actual.values.len(), (case.width * case.height) as usize);
            // A half-precision texture introduces at most 1/4096 at these values.
            close(
                &case
                    .sample_indices
                    .iter()
                    .map(|&i| actual.values[i].to_f32())
                    .collect::<Vec<_>>(),
                &case.values,
                3e-4,
            );
            assert!((actual.median - case.median).abs() < 2e-5);
        }
    }

    #[test]
    fn flat_depth_and_bad_inputs() {
        let rgba = vec![120; 5 * 3 * 4];
        let actual = prepare_depth(&[7.0; 4], 2, 2, &rgba, 5, 3).unwrap();
        assert_eq!(actual.median, 0.0);
        assert!(actual.values.iter().all(|v| *v == f16::ZERO));
        assert!(prepare_depth(&[], 0, 1, &rgba, 5, 3).is_err());
        assert!(prepare_depth(&[0.0], 1, 1, &rgba[..3], 1, 1).is_err());
        assert!(prepare_depth(&[f32::NAN], 1, 1, &rgba, 5, 3).is_err());
        assert!(prepare_depth(&[f32::INFINITY], 1, 1, &rgba, 5, 3).is_err());
        assert_eq!(normalize_disparity(&[1.0, 1.0 + 1e-7]), vec![0.0, 0.0]);
        assert_eq!(normalize_disparity(&[-f32::MAX, f32::MAX]), vec![0.0, 1.0]);
    }

    #[test]
    fn border_reflection_and_even_median() {
        assert_eq!(reflect(-1, 3, true), 0);
        assert_eq!(reflect(3, 3, true), 2);
        assert_eq!(reflect(-1, 3, false), 1);
        assert_eq!(reflect(3, 3, false), 1);
        assert_eq!(reflect(-100, 1, false), 0);
        assert_eq!(percentile(&mut [0.0, 1.0, 0.2, 0.6], 0.5), 0.4);
    }

    #[test]
    fn depth_percentile_selection_matches_full_sort() {
        let (input, _) = full_size_input();
        for count in [1, 2, 3, 128, input.len()] {
            let mut sorted = input[..count].to_vec();
            sorted.sort_unstable_by(f32::total_cmp);
            for fraction in [0.0, 0.01, 0.5, 0.99, 1.0] {
                let position = (count - 1) as f64 * fraction;
                let (lo, hi) = (position.floor() as usize, position.ceil() as usize);
                let expected = (sorted[lo] as f64
                    + (sorted[hi] as f64 - sorted[lo] as f64) * (position - lo as f64))
                    as f32;
                let actual = percentile(&mut input[..count].to_vec(), fraction);
                assert_eq!(
                    actual.to_bits(),
                    expected.to_bits(),
                    "count={count}, p={fraction}"
                );
            }
        }
    }
}
