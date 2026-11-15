//! Evaluation adapter: prediction and snapping are shared production code.
use super::*;
pub(super) use crate::cover_auto_crop::layout::{LayoutDiagnostic, ratio};
#[cfg(test)]
use crate::cover_auto_crop::layout::{SNAP_THRESHOLD, predict};

pub(super) fn crop(
    image: &DynamicImage,
    dimensions: (u32, u32),
    aspect: f64,
    binding: ReadingDirection,
    rotation: Rotation,
    snap: bool,
) -> Option<(CoverCrop, LayoutDiagnostic)> {
    let raster = analysis_raster(&rotated_image(image, rotation));
    let (crop, diagnostic) =
        crate::cover_auto_crop::layout::crop(&raster, dimensions, aspect, binding, snap)?;
    Some((inverse_rotated_crop(crop, rotation), diagnostic))
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
