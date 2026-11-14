//! Evaluation-only layout hypotheses, followed optionally by local edge snapping.
use super::*;

const SNAP_THRESHOLD: f64 = 3.0;
const SCORE_EPSILON: f64 = 1e-9;

#[derive(Clone, Copy, Debug)]
pub(super) struct LayoutDiagnostic {
    pub(super) kind: usize,
    pub(super) ratio: f64,
    predicted: [f64; 2],
    final_position: [f64; 2],
    snap_applied: bool,
    strengths: Option<[f64; 2]>,
    score: Option<f64>,
    searched_score: Option<f64>,
}

impl LayoutDiagnostic {
    pub(super) fn json(self) -> serde_json::Value {
        json!({"kind": format!("T{}", self.kind + 1), "ratio": self.ratio,
            "predicted": self.predicted, "final_position": self.final_position,
            "snap_applied": self.snap_applied, "edge_strengths": self.strengths,
            "score": self.score, "searched_score": self.searched_score,
            "snap_threshold": self.score.map(|_| SNAP_THRESHOLD)})
    }
}

pub(super) fn ratio(width: f64, panel: f64) -> Option<f64> {
    if !width.is_finite() || !panel.is_finite() || width <= 0.0 || panel <= 0.0 {
        return None;
    }
    let ratio = width / panel;
    ratio.is_finite().then_some(ratio)
}

fn predict(width: f64, panel: f64, binding: ReadingDirection) -> Option<(usize, f64, [f64; 2])> {
    let ratio = ratio(width, panel)?;
    let front = 1.02 * panel;
    let (kind, left) = if (1.35..1.85).contains(&ratio) {
        (0, width - front)
    } else if (1.85..2.4).contains(&ratio) {
        (1, 0.0)
    } else if (2.4..3.1).contains(&ratio) {
        (2, 0.63 * panel)
    } else if (3.1..3.9).contains(&ratio) {
        (3, (width - 2.0 * front - 0.1 * panel) / 2.0)
    } else {
        return None;
    };
    let rect = match binding {
        ReadingDirection::Rtl => [left, left + front],
        ReadingDirection::Ltr => [width - left - front, width - left],
    }
    .map(|edge| edge.clamp(0.0, width));
    Some((kind, ratio, rect))
}

pub(super) fn crop(
    image: &DynamicImage,
    dimensions: (u32, u32),
    aspect: f64,
    binding: ReadingDirection,
    rotation: Rotation,
    snap: bool,
) -> Option<(CoverCrop, LayoutDiagnostic)> {
    let width = f64::from(dimensions.0);
    // Intentionally unrounded. A 1px rounding error can cross a layout boundary.
    let panel = aspect * f64::from(dimensions.1);
    let (kind, ratio, predicted) = predict(width, panel, binding)?;
    let mut diagnostic = LayoutDiagnostic {
        kind,
        ratio,
        predicted,
        final_position: predicted,
        snap_applied: false,
        strengths: None,
        score: None,
        searched_score: None,
    };
    if snap {
        let raster = analysis_raster(&rotated_image(image, rotation));
        if raster.width() > 0 && raster.height() > 0 {
            let edges = ColumnEdges::new(&raster);
            let scale = width / f64::from(raster.width());
            let strength = |rect: [f64; 2]| rect.map(|x| edges.at(x / scale));
            let weighted = |values: [f64; 2]| match binding {
                ReadingDirection::Rtl => (values[0] + 2.0 * values[1]) / 3.0,
                ReadingDirection::Ltr => (2.0 * values[0] + values[1]) / 3.0,
            };
            let baseline = weighted(strength(predicted));
            let mut best = predicted;
            let mut best_score = baseline;
            let distance =
                |rect: [f64; 2]| (rect[0] - predicted[0]).abs() + (rect[1] - predicted[1]).abs();
            let left_min = ((predicted[0] - 0.15 * panel).max(0.0) / scale).ceil() as u32;
            let left_max = ((predicted[0] + 0.15 * panel).min(width) / scale).floor() as u32;
            for left in left_min..=left_max {
                let x0 = f64::from(left) * scale;
                let right_min = ((x0 + 0.95 * panel) / scale).ceil() as u32;
                let right_max = ((x0 + 1.08 * panel).min(width) / scale).floor() as u32;
                for right in right_min..=right_max {
                    let candidate = [x0, f64::from(right) * scale];
                    let score = weighted(strength(candidate));
                    if score > best_score + SCORE_EPSILON
                        || ((score - best_score).abs() <= SCORE_EPSILON
                            && distance(candidate) < distance(best) - SCORE_EPSILON)
                    {
                        best = candidate;
                        best_score = score;
                    }
                }
            }
            let best_strengths = strength(best);
            let spine = match binding {
                ReadingDirection::Rtl => best_strengths[1],
                ReadingDirection::Ltr => best_strengths[0],
            };
            diagnostic.searched_score = Some(best_score);
            // Require both a strong spine and a weighted score above threshold.
            // Weak evidence or a score tied with the exact layout keeps layout.
            if best_score >= SNAP_THRESHOLD
                && spine >= SNAP_THRESHOLD
                && best_score > baseline + SCORE_EPSILON
            {
                diagnostic.final_position = best;
                diagnostic.snap_applied = true;
            }
            let final_strengths = strength(diagnostic.final_position);
            diagnostic.strengths = Some(final_strengths);
            diagnostic.score = Some(weighted(final_strengths));
        }
    }
    let [left, right] = diagnostic.final_position;
    let ppm = |x: f64| (x.clamp(0.0, width) / width * 1_000_000.0).round() as u32;
    let [x0, y0, x1, y1] = [ppm(left), 0, ppm(right), 1_000_000];
    // Same inverse edge transform as production's saved-rotation crop path.
    let unrotated = match rotation {
        Rotation::None => [x0, y0, x1, y1],
        Rotation::Cw90 => [y0, 1_000_000 - x1, y1, 1_000_000 - x0],
        Rotation::Cw180 => [
            1_000_000 - x1,
            1_000_000 - y1,
            1_000_000 - x0,
            1_000_000 - y0,
        ],
        Rotation::Cw270 => [1_000_000 - y1, x0, 1_000_000 - y0, x1],
    };
    Some((CoverCrop::new(unrotated).ok().flatten()?, diagnostic))
}

struct ColumnEdges(Vec<f64>);

impl ColumnEdges {
    fn new(image: &RgbImage) -> Self {
        // S(x) is the mean absolute adjacent-column luminance difference over
        // central 90% of rows. Use Rec.709 luminance, smooth across 3 columns,
        // then divide by median smoothed S with a 1/255 luminance floor.
        // Input RGB and the floor are both expressed on the 0..255 scale.
        let top = (f64::from(image.height()) * 0.05).floor() as u32;
        let bottom = (f64::from(image.height()) * 0.95).ceil() as u32;
        let mut raw = vec![0.0; image.width() as usize + 1];
        let luma = |pixel: &Rgb<u8>| {
            0.2126 * f64::from(pixel[0])
                + 0.7152 * f64::from(pixel[1])
                + 0.0722 * f64::from(pixel[2])
        };
        for x in 1..image.width() {
            raw[x as usize] = (top..bottom)
                .map(|y| (luma(image.get_pixel(x, y)) - luma(image.get_pixel(x - 1, y))).abs())
                .sum::<f64>()
                / f64::from(bottom - top);
        }
        let mut smooth = vec![0.0; raw.len()];
        for x in 1..image.width() as usize {
            smooth[x] = (raw[x - 1] + raw[x] + raw[x + 1]) / 3.0;
        }
        let mut ordered = smooth[1..image.width() as usize].to_vec();
        ordered.sort_by(f64::total_cmp);
        let median = ordered
            .get(ordered.len() / 2)
            .copied()
            .unwrap_or(0.0)
            .max(1.0);
        for value in &mut smooth {
            *value /= median;
        }
        Self(smooth)
    }

    fn at(&self, x: f64) -> f64 {
        let left = (x.floor() as usize).min(self.0.len() - 1);
        let right = (left + 1).min(self.0.len() - 1);
        self.0[left] + (self.0[right] - self.0[left]) * x.fract()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(actual: f64, expected: f64) {
        assert!((actual - expected).abs() < 1e-9, "{actual} != {expected}");
    }

    #[test]
    fn evaluation_layout_types_mirror_unrounded_positions_and_exact_ratio_boundaries() {
        let image = DynamicImage::new_rgb8(350, 150);
        for (ratio, kind) in [
            (1.35, 0),
            (1.849999, 0),
            (1.85, 1),
            (2.399999, 1),
            (2.4, 2),
            (3.099999, 2),
            (3.1, 3),
            (3.899999, 3),
        ] {
            let width = 100.0 * ratio;
            let (actual_kind, actual_ratio, rtl) =
                predict(width, 100.0, ReadingDirection::Rtl).unwrap();
            let (_, _, ltr) = predict(width, 100.0, ReadingDirection::Ltr).unwrap();
            assert_eq!(actual_kind, kind);
            close(actual_ratio, ratio);
            close(rtl[1] - rtl[0], 102.0);
            close(ltr[0], width - rtl[1]);
            close(ltr[1], width - rtl[0]);
            let expected_left = match kind {
                0 => width - 102.0,
                1 => 0.0,
                2 => 63.0,
                _ => (width - 204.0 - 10.0) / 2.0,
            };
            close(rtl[0], expected_left);
        }
        for ratio in [0.0, 1.349999, 3.9, 4.0, f64::INFINITY, f64::NAN] {
            assert!(predict(ratio * 100.0, 100.0, ReadingDirection::Rtl).is_none());
        }
        // W=100.5, not round(W)=101. R is just above T2's lower bound.
        let (_, diagnostic) = crop(
            &image,
            (186, 150),
            0.67,
            ReadingDirection::Rtl,
            Rotation::None,
            false,
        )
        .unwrap();
        assert_eq!(diagnostic.kind, 1);
        close(diagnostic.ratio, 186.0 / 100.5);
        close(diagnostic.final_position[1], 1.02 * 100.5);
    }

    #[test]
    fn evaluation_layout_snap_finds_known_spine_and_keeps_flat_or_tied_layout() {
        // W=100, T3 starts at 63 with width 102. The synthetic front is shifted
        // nine pixels inward, within the +/-15 search window.
        let image = DynamicImage::ImageRgb8(RgbImage::from_fn(270, 256, |x, _| {
            if (72..174).contains(&x) {
                Rgb([200, 200, 200])
            } else {
                Rgb([0, 0, 0])
            }
        }));
        for binding in [ReadingDirection::Rtl, ReadingDirection::Ltr] {
            let image = if binding == ReadingDirection::Ltr {
                image.fliph()
            } else {
                image.clone()
            };
            let (_, diagnostic) = crop(
                &image,
                (270, 256),
                100.0 / 256.0,
                binding,
                Rotation::None,
                true,
            )
            .unwrap();
            assert!(diagnostic.snap_applied);
            let expected = match binding {
                ReadingDirection::Rtl => [72.0, 174.0],
                ReadingDirection::Ltr => [96.0, 198.0],
            };
            for (actual, expected) in diagnostic.final_position.into_iter().zip(expected) {
                assert!((actual - expected).abs() <= 2.0, "{diagnostic:?}");
            }
            assert!(diagnostic.score.unwrap() >= SNAP_THRESHOLD);
        }
        let flat = DynamicImage::new_rgb8(270, 256);
        let tied = DynamicImage::ImageRgb8(RgbImage::from_fn(270, 256, |x, _| {
            if (63..165).contains(&x) {
                Rgb([200, 200, 200])
            } else {
                Rgb([0, 0, 0])
            }
        }));
        let weak = DynamicImage::ImageRgb8(RgbImage::from_fn(270, 256, |x, _| {
            if (72..174).contains(&x) {
                Rgb([2, 2, 2])
            } else {
                Rgb([0, 0, 0])
            }
        }));
        for image in [&flat, &tied, &weak] {
            let (plain, _) = crop(
                image,
                (270, 256),
                100.0 / 256.0,
                ReadingDirection::Rtl,
                Rotation::None,
                false,
            )
            .unwrap();
            let (snapped, diagnostic) = crop(
                image,
                (270, 256),
                100.0 / 256.0,
                ReadingDirection::Rtl,
                Rotation::None,
                true,
            )
            .unwrap();
            assert_eq!(plain, snapped);
            assert!(!diagnostic.snap_applied);
        }
    }

    #[test]
    fn evaluation_layout_inverse_rotations_preserve_all_four_types_and_both_bindings() {
        for width in [150, 200, 270, 350] {
            let base = DynamicImage::new_rgb8(width, 150);
            for snap in [false, true] {
                for binding in [ReadingDirection::Rtl, ReadingDirection::Ltr] {
                    let (expected, _) = crop(
                        &base,
                        (width, 150),
                        2.0 / 3.0,
                        binding,
                        Rotation::None,
                        snap,
                    )
                    .unwrap();
                    for physical in EVAL_ROTATIONS {
                        let image = rotated_image(&base, physical);
                        let saved = Rotation::from_degrees(-physical.degrees());
                        let (actual, _) =
                            crop(&image, (width, 150), 2.0 / 3.0, binding, saved, snap).unwrap();
                        assert_eq!(
                            actual.values().map(f64::from),
                            rotated_rect(
                                expected.values().map(f64::from),
                                (1_000_000, 1_000_000),
                                physical
                            )
                        );
                    }
                }
            }
        }
    }
}
