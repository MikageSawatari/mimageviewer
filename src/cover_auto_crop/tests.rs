use super::*;
use image::Rgb;

#[test]
fn native_analysis_raster_preserves_pixels_without_smoothing_either_axis() {
    let image = RgbImage::from_fn(8, 256, |x, y| {
        Rgb([
            if x < 4 { 30 } else { 200 },
            if y == 128 { 255 } else { 0 },
            80,
        ])
    });
    let raster = analysis_raster(&DynamicImage::ImageRgb8(image.clone()));
    assert_eq!(raster.dimensions(), (8, 256));
    assert_eq!(raster, image);
}

#[test]
fn analysis_area_average_integrates_only_the_output_pixel_area() {
    let source = RgbImage::from_fn(16, 512, |x, y| {
        Rgb([if x % 2 == 0 { 40 } else { 80 }, (y % 4) as u8 * 60, 90])
    });
    let raster = analysis_raster(&DynamicImage::ImageRgb8(source));
    assert_eq!(raster.dimensions(), (8, 256));
    for (_, y, pixel) in raster.enumerate_pixels() {
        assert_eq!(*pixel, Rgb([60, if y % 2 == 0 { 30 } else { 150 }, 90]));
    }
}

/// Three broad colored bands can look like flap/front/back geometry, but have
/// no spine and no flat face. The nearly isoluminant hue changes every sixteen
/// rows; the fine print texture supplies real luminance variance within them.
fn textured_art(texture: usize, binding: ReadingDirection) -> RgbImage {
    RgbImage::from_fn(300, 256, |x, y| {
        let x = if binding == ReadingDirection::Ltr {
            299 - x
        } else {
            x
        };
        let base = match x {
            0..70 => [130i32, 90, 170],
            70..249 => [75, 75, 200],
            _ => [150, 130, 150],
        };
        let hue = if (y / 16) % 2 == 0 { -1 } else { 1 };
        let (tone, jitter) = match texture {
            // A native 256-row counterexample to the old [1/4,1/2,1/4]
            // vertical filter: std ~30 becomes ~1.4, manufacturing a flat face.
            0 => (if y % 2 == 0 { -30 } else { 30 }, 0),
            // Fine halftone dots. Only alternate rows contain horizontal edges,
            // so the texture itself is not a full-height panel boundary.
            1 => (if y % 2 == 0 && x % 2 == 0 { 30 } else { -30 }, 0),
            // Deterministic row noise with small independent pixel variation;
            // adjacent noise pixels remain below the panel-edge threshold.
            _ => {
                let row = y.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                let pixel = row ^ x.wrapping_mul(2_654_435_761);
                (
                    if row & 0x100 == 0 { -30 } else { 30 },
                    (pixel % 9) as i32 - 4,
                )
            }
        };
        let shift = [40 * hue, -20 * hue, 0];
        Rgb(std::array::from_fn(|c| {
            (base[c] + shift[c] + tone + jitter) as u8
        }))
    })
}

#[test]
fn native_analysis_preserves_ink_variance_colorfulness_and_panel_strength() {
    let image = textured_art(0, ReadingDirection::Rtl);
    let raster = analysis_raster(&DynamicImage::ImageRgb8(image.clone()));
    assert_eq!(raster, image);
    let native = Raster::from_image(&image);
    let analysis = Raster::from_image(&raster);
    assert_eq!(panel_edges(&analysis), panel_edges(&native));
    assert_eq!(panel_edges(&analysis), vec![0, 67, 73, 246, 252, 300]);
    let native_features = Features::new(&native);
    let analysis_features = Features::new(&analysis);
    for (left, right) in [(0, 67), (67, 73), (73, 246), (246, 252), (252, 300)] {
        assert_eq!(
            analysis_features.ink(left, right),
            native_features.ink(left, right)
        );
        assert_eq!(
            analysis_features.score(left, right),
            native_features.score(left, right)
        );
        assert!(!analysis_features.flat(left, right));
        let columns = &analysis_features.columns[left..right];
        let count = ((right - left) * analysis.height) as f64;
        let mean = columns.iter().map(|c| c.lum).sum::<f64>() / count;
        let std = (columns.iter().map(|c| c.lum2).sum::<f64>() / count - mean * mean).sqrt();
        assert!(std >= 29.9, "panel {left}..{right}: std={std}");
    }
}

#[test]
fn smoothing_native_rows_would_manufacture_an_accepted_front_cover() {
    let image = textured_art(0, ReadingDirection::Rtl);
    let smoothed = RgbImage::from_fn(image.width(), image.height(), |x, y| {
        Rgb(std::array::from_fn(|c| {
            (0.25 * f64::from(image.get_pixel(x, y.saturating_sub(1))[c])
                + 0.5 * f64::from(image.get_pixel(x, y)[c])
                + 0.25 * f64::from(image.get_pixel(x, (y + 1).min(image.height() - 1))[c]))
            .round() as u8
        }))
    });
    assert_eq!(
        cover_auto_crop(
            &smoothed,
            image.dimensions(),
            Some(0.7),
            ReadingDirection::Rtl
        )
        .expect("the old kernel manufactures flat panels and accepts this art")
        .values(),
        [223_333, 0, 820_000, 1_000_000]
    );
    let raster = analysis_raster(&DynamicImage::ImageRgb8(image));
    assert_eq!(
        cover_auto_crop(&raster, (300, 256), Some(0.7), ReadingDirection::Rtl)
            .map_err(AutoCropDecline::v5_reason),
        Err(AutoCropDecline::NoJacketEvidence)
    );
}

#[test]
fn fine_stripes_halftone_and_noise_do_not_manufacture_jacket_evidence() {
    for binding in [ReadingDirection::Rtl, ReadingDirection::Ltr] {
        for texture in 0..3 {
            let image = textured_art(texture, binding);
            let dims = image.dimensions();
            let raster = analysis_raster(&DynamicImage::ImageRgb8(image));
            assert_eq!(
                cover_auto_crop(&raster, dims, Some(0.7), binding)
                    .map_err(AutoCropDecline::v5_reason),
                Err(AutoCropDecline::NoJacketEvidence),
                "{binding:?}, texture={texture}"
            );
        }
    }
}

/// Print only in the top/bottom bands must contribute to panel variance.
/// A filtered margin reference erases those bands before Features can see them.
fn patterned_margin_art(band: i32, amplitude: i32, center_hue: bool) -> RgbImage {
    RgbImage::from_fn(300, 256, |x, y| {
        let z = y.min(255 - y) as i32;
        let tone = amplitude * z.min(band - z).min(8).max(0) * if y % 2 == 0 { -1 } else { 1 };
        let shade = if x < 100 {
            -20
        } else if x < 208 {
            0
        } else {
            20
        };
        let hue = if center_hue && band < y as i32 && (y as i32) < 255 - band {
            if (y / 16) % 2 == 0 {
                [30, -15, 0]
            } else {
                [-30, 15, 0]
            }
        } else {
            [0; 3]
        };
        Rgb(std::array::from_fn(|channel| {
            u8::try_from([150, 60, 120][channel] + shade + tone + hue[channel])
                .expect("generated print stays in RGB range")
        }))
    })
}

#[test]
fn patterned_margin_rows_remain_content_and_decline_without_jacket_evidence() {
    // Exact confirmation-review counterexample, including the central hue bands.
    let image = patterned_margin_art(50, 4, true);
    let raster = analysis_raster(&DynamicImage::ImageRgb8(image.clone()));
    assert_eq!(raster, image);
    let source = Raster::from_image(&raster);
    assert_eq!(
        content_bounds(&source, |x, y| source.get(x, y)),
        [0, 1, 300, 255]
    );
    assert_eq!(
        cover_auto_crop(
            &raster,
            image.dimensions(),
            Some(0.7),
            ReadingDirection::Rtl
        )
        .map_err(AutoCropDecline::v5_reason),
        Err(AutoCropDecline::NoJacketEvidence)
    );
}

#[test]
fn patterned_top_bottom_bands_with_flat_center_must_decline() {
    // These widths/amplitudes also decline in the evaluation prototype.
    for (band, amplitude) in [(40, 3), (50, 4), (64, 5)] {
        for binding in [ReadingDirection::Rtl, ReadingDirection::Ltr] {
            let image = patterned_margin_art(band, amplitude, false);
            let image = if binding == ReadingDirection::Ltr {
                image::imageops::flip_horizontal(&image)
            } else {
                image
            };
            let dims = image.dimensions();
            let raster = analysis_raster(&DynamicImage::ImageRgb8(image));
            assert!(
                cover_auto_crop(&raster, dims, Some(0.7), binding).is_err(),
                "band={band}, amplitude={amplitude}, binding={binding:?}"
            );
        }
    }
}

#[test]
fn impossible_scan_sliver_declines_without_unbounded_upscale() {
    for dims in [(2048, 1), (25_601, 256)] {
        let image = DynamicImage::new_rgb8(dims.0, dims.1);
        let raster = analysis_raster(&image);
        assert_eq!(raster.dimensions(), (0, 0));
        assert_eq!(
            cover_auto_crop(&raster, dims, Some(0.7), ReadingDirection::Rtl),
            Err(AutoCropDecline::NoStructure)
        );
    }
}

/// Distinct printed faces with no internal full-height seams. The left flap
/// contains sparse text, the spine is dense, and the back is almost blank.
fn jacket(
    binding: ReadingDirection,
    scan: bool,
    dust: bool,
    skew: bool,
    flap: bool,
) -> (RgbImage, [u32; 4]) {
    let (height, front_width, flap_width, spine_width, back_width) =
        (512, 358, if flap { 140 } else { 0 }, 36, 358);
    let margin = if scan { [23, 13, 7, 19] } else { [0; 4] };
    let content_width = flap_width + front_width + spine_width + back_width;
    let mut image = RgbImage::from_pixel(
        content_width + margin[0] + margin[2],
        height + margin[1] + margin[3],
        Rgb([248, 245, 226]),
    );
    for y in 0..height {
        let shift = if skew {
            y as i32 * 4 / height as i32 - 2
        } else {
            0
        };
        for x in 0..content_width {
            let position = (x as i32 - shift).clamp(0, content_width as i32 - 1) as u32;
            let p = if position < flap_width {
                if y % 73 < 8 {
                    [65, 65, 65]
                } else {
                    [246, 242, 220]
                }
            } else if position < flap_width + front_width {
                let wave = (y % 80) as u8;
                [35 + wave, 55 + (y % 155) as u8, 200 - wave]
            } else if position < flap_width + front_width + spine_width {
                [18, 28, 38]
            } else {
                if y % 73 < 8 {
                    [75, 75, 75]
                } else {
                    [246, 242, 220]
                }
            };
            let x = if binding == ReadingDirection::Ltr {
                content_width - 1 - x
            } else {
                x
            };
            image.put_pixel(margin[0] + x, margin[1] + y, Rgb(p));
        }
    }
    if dust {
        // Dust is below the 3% margin tolerance, on all four margins.
        for x in (3..image.width()).step_by(71) {
            image.put_pixel(x, 3, Rgb([20, 20, 20]));
            image.put_pixel(x, image.height() - 3, Rgb([20, 20, 20]));
        }
    }
    let mut x = flap_width;
    if binding == ReadingDirection::Ltr {
        x = content_width - x - front_width;
    }
    let truth = [
        margin[0] + x,
        margin[1],
        margin[0] + x + front_width,
        margin[1] + height,
    ];
    (image, truth)
}

fn iou(a: [u32; 4], b: [u32; 4]) -> f64 {
    let intersection = (a[2].min(b[2]).saturating_sub(a[0].max(b[0])) as f64)
        * (a[3].min(b[3]).saturating_sub(a[1].max(b[1])) as f64);
    let area = |r: [u32; 4]| (r[2] - r[0]) as f64 * (r[3] - r[1]) as f64;
    intersection / (area(a) + area(b) - intersection)
}

fn pixel_rect(crop: CoverCrop, dimensions: (u32, u32)) -> [u32; 4] {
    let (x, y, w, h) = crop.pixel_bounds(dimensions.0, dimensions.1);
    [x, y, x + w, y + h]
}

#[test]
fn synthetic_jackets_both_bindings_margins_dust_skew_and_spine() {
    for binding in [ReadingDirection::Rtl, ReadingDirection::Ltr] {
        for scan in [false, true] {
            for skew in [false, true] {
                for flap in [false, true] {
                    let (image, truth) = jacket(binding, scan, scan, skew, flap);
                    let dims = image.dimensions();
                    let raster = analysis_raster(&DynamicImage::ImageRgb8(image));
                    let crop = cover_auto_crop(&raster, dims, Some(0.7), binding).unwrap_or_else(
                        |reason| {
                            panic!("{binding:?} scan={scan} skew={skew} flap={flap}: {reason:?}")
                        },
                    );
                    assert!(
                        iou(pixel_rect(crop, dims), truth) >= 0.85,
                        "{binding:?} {scan} {skew} {flap}: {:?} {truth:?}",
                        crop.values()
                    );
                }
            }
        }
    }
}

#[test]
fn decoded_resolution_without_dust_keeps_boundaries_within_one_analysis_column() {
    for binding in [ReadingDirection::Rtl, ReadingDirection::Ltr] {
        // Isolated dust may disappear at a smaller decode scale. The prototype
        // does not promise identical content bounds in that case.
        let (image, _) = jacket(binding, true, false, true, true);
        let dims = image.dimensions();
        let mut previous: Option<[u32; 4]> = None;
        for height in [128, 256, 512, 1024] {
            let width =
                (image.width() as f64 * height as f64 / image.height() as f64).round() as u32;
            let decoded = image::imageops::resize(
                &image,
                width,
                height,
                image::imageops::FilterType::CatmullRom,
            );
            let raster = analysis_raster(&DynamicImage::ImageRgb8(decoded));
            assert_eq!(raster.height(), 256);
            let crop = cover_auto_crop(&raster, dims, Some(0.7), binding)
                .unwrap()
                .values();
            if let Some(previous) = previous {
                for i in [0, 2] {
                    let drift =
                        crop[i].abs_diff(previous[i]) as f64 * raster.width() as f64 / 1_000_000.0;
                    assert!(drift <= 1.1, "{binding:?} height={height} drift={drift}");
                }
            }
            previous = Some(crop);
        }
    }
}

#[test]
fn negatives_portrait_landscape_spread_and_art_with_white_panel() {
    for binding in [ReadingDirection::Rtl, ReadingDirection::Ltr] {
        for (width, height, white_panel) in [
            (140, 256, None),
            (460, 256, None),
            (358, 256, None),
            (358, 256, Some(false)),
            (358, 256, Some(true)),
        ] {
            let image = RgbImage::from_fn(width, height, |x, y| {
                if white_panel.is_some_and(|left| (x < 179) == left) {
                    Rgb([250, 250, 250])
                } else {
                    Rgb([
                        30 + (y % 120) as u8,
                        40 + (y % 150) as u8,
                        210 - (y % 100) as u8,
                    ])
                }
            });
            let dims = image.dimensions();
            let raster = analysis_raster(&DynamicImage::ImageRgb8(image));
            assert!(
                cover_auto_crop(&raster, dims, Some(0.7), binding).is_err(),
                "{binding:?} {width} white={white_panel:?}"
            );
        }
    }
}

#[test]
fn gates_and_identity_are_explicit() {
    let image = RgbImage::new(2, 2);
    assert_eq!(
        cover_auto_crop(&image, (100, 200), None, ReadingDirection::Rtl),
        Err(AutoCropDecline::RawAspectGate)
    );
    for ratio in [
        None,
        Some(f64::NAN),
        Some(0.0),
        Some(-1.0),
        Some(f64::INFINITY),
    ] {
        assert_eq!(
            cover_auto_crop(&image, (300, 200), ratio, ReadingDirection::Rtl),
            Err(AutoCropDecline::PageAspectMissing)
        );
    }
    assert_eq!(
        AutoCropIdentity::new(ReadingDirection::Rtl).suffix(),
        "v2:rtl"
    );
    assert_eq!(
        AutoCropIdentity::new(ReadingDirection::Ltr).suffix(),
        "v2:ltr"
    );
}

fn rotate_image(image: &DynamicImage, rotation: Rotation) -> DynamicImage {
    match rotation {
        Rotation::None => image.clone(),
        Rotation::Cw90 => image.rotate90(),
        Rotation::Cw180 => image.rotate180(),
        Rotation::Cw270 => image.rotate270(),
    }
}

fn rotate_crop(crop: CoverCrop, rotation: Rotation) -> CoverCrop {
    let [x0, y0, x1, y1] = crop.values();
    let values = match rotation {
        Rotation::None => [x0, y0, x1, y1],
        Rotation::Cw90 => [1_000_000 - y1, x0, 1_000_000 - y0, x1],
        Rotation::Cw180 => [
            1_000_000 - x1,
            1_000_000 - y1,
            1_000_000 - x0,
            1_000_000 - y0,
        ],
        Rotation::Cw270 => [y0, 1_000_000 - x1, y1, 1_000_000 - x0],
    };
    CoverCrop::new(values).unwrap().unwrap()
}

#[test]
fn saved_rotation_detection_matches_upright_jacket_in_source_coordinates() {
    for binding in [ReadingDirection::Rtl, ReadingDirection::Ltr] {
        for scan in [false, true] {
            let (image, _) = jacket(binding, scan, scan, true, true);
            let image = DynamicImage::ImageRgb8(image);
            let upright =
                detect_cover_crops(&image, (image.width(), image.height()), Some(0.7), binding)
                    .crop_for(Rotation::None)
                    .unwrap();
            for physical in [
                Rotation::None,
                Rotation::Cw90,
                Rotation::Cw180,
                Rotation::Cw270,
            ] {
                let rotated = rotate_image(&image, physical);
                let saved = Rotation::from_degrees(-physical.degrees());
                let results = detect_cover_crops(
                    &rotated,
                    (rotated.width(), rotated.height()),
                    Some(0.7),
                    binding,
                );
                assert_eq!(
                    results.crop_for(saved),
                    Some(rotate_crop(upright, physical)),
                    "binding={binding:?} scan={scan} physical={physical:?}"
                );
            }
        }
    }
}

#[test]
fn raw_aspect_gate_before_pixel_work_preserves_decline_reason_for_each_rotation() {
    let (pixels, _) = jacket(ReadingDirection::Rtl, true, false, false, true);
    let image = DynamicImage::ImageRgb8(pixels);
    for original_dims in [
        (700, 1000),
        (1000, 700),
        (0, 1000),
        (1000, 0),
        (0, 0),
        (849, 1000),
        (850, 1000),
    ] {
        for rotation in [
            Rotation::None,
            Rotation::Cw90,
            Rotation::Cw180,
            Rotation::Cw270,
        ] {
            let rotated = rotate_image(&image, rotation);
            let dimensions = match rotation {
                Rotation::Cw90 | Rotation::Cw270 => (original_dims.1, original_dims.0),
                _ => original_dims,
            };
            for aspect in [None, Some(f64::NAN), Some(0.7)] {
                // The old order is the oracle: rotation/raster, then raw gate.
                let expected = cover_auto_crop(
                    &analysis_raster(&rotated),
                    dimensions,
                    aspect,
                    ReadingDirection::Rtl,
                );
                let actual = detect_cover_crop_at_rotation(
                    &image,
                    original_dims,
                    aspect,
                    ReadingDirection::Rtl,
                    rotation,
                );
                assert_eq!(
                    actual.as_ref().err(),
                    expected.as_ref().err(),
                    "{original_dims:?} {rotation:?} {aspect:?}"
                );
                if raw_aspect_gate(dimensions).is_err() {
                    assert_eq!(actual, Err(AutoCropDecline::RawAspectGate));
                }
            }
        }
    }
}

#[test]
fn portrait_gate_uses_saved_rotation_and_the_unrotated_results_stay_declined() {
    let (image, _) = jacket(ReadingDirection::Rtl, true, false, false, true);
    let portrait = DynamicImage::ImageRgb8(image).rotate90();
    let dims = (portrait.width(), portrait.height());
    let results = detect_cover_crops(&portrait, dims, Some(0.7), ReadingDirection::Rtl);
    assert_eq!(results.crop_for(Rotation::None), None);
    assert_eq!(results.crop_for(Rotation::Cw180), None);
    assert_eq!(
        detect_cover_crop_at_rotation(
            &portrait,
            dims,
            Some(0.7),
            ReadingDirection::Rtl,
            Rotation::None
        ),
        Err(AutoCropDecline::RawAspectGate)
    );
    assert!(results.crop_for(Rotation::Cw270).is_some());
}

#[test]
fn saved_quarter_turn_reduces_only_after_restoring_the_upright_analysis_height() {
    let (image, _) = jacket(ReadingDirection::Rtl, true, true, true, true);
    let upright = DynamicImage::ImageRgb8(image);
    let portrait = upright.rotate90();
    let correct = analysis_raster(&portrait.rotate270());
    assert_eq!(correct, analysis_raster(&upright));
    // Reducing a portrait first irreversibly loses horizontal detail and leaves
    // a different analysis height after rotating; it cannot be reused here.
    let wrong_order = image::imageops::rotate270(&analysis_raster(&portrait));
    assert_ne!(correct.dimensions(), wrong_order.dimensions());
    let expected = cover_auto_crop(
        &correct,
        (upright.width(), upright.height()),
        Some(0.7),
        ReadingDirection::Rtl,
    )
    .unwrap();
    let result = detect_cover_crops(
        &portrait,
        (portrait.width(), portrait.height()),
        Some(0.7),
        ReadingDirection::Rtl,
    );
    assert_eq!(
        result.crop_for(Rotation::Cw270),
        Some(rotate_crop(expected, Rotation::Cw90))
    );
}

#[test]
fn content_margin_tolerates_sparse_dust_and_remains_asymmetric() {
    let (image, _) = jacket(ReadingDirection::Rtl, true, true, false, true);
    let raster = Raster::from_image(&image);
    let bounds = content_bounds(&raster, |x, y| raster.get(x, y));
    assert_eq!(
        bounds,
        [
            23,
            13,
            image.width() as usize - 7,
            image.height() as usize - 19
        ]
    );
}

#[test]
fn color_is_a_veto_and_does_not_relocate_to_the_back() {
    for binding in [ReadingDirection::Rtl, ReadingDirection::Ltr] {
        let (mut image, front) = jacket(binding, false, false, false, false);
        for y in 0..image.height() {
            for x in front[0]..front[2] {
                // A grayscale front cannot win against the vivid back by
                // changing the position rule; it must be declined instead.
                let gray = 90 + (y % 100) as u8;
                image.put_pixel(x, y, Rgb([gray, gray, gray]));
            }
            let back = if binding == ReadingDirection::Rtl {
                front[2] + 36..image.width()
            } else {
                0..front[0] - 36
            };
            for x in back {
                image.put_pixel(
                    x,
                    y,
                    Rgb(if y % 30 < 15 {
                        [230, 20, 80]
                    } else {
                        [20, 190, 230]
                    }),
                );
            }
        }
        let dims = image.dimensions();
        let raster = analysis_raster(&DynamicImage::ImageRgb8(image));
        let result =
            cover_auto_crop(&raster, dims, Some(0.7), binding).map_err(AutoCropDecline::v5_reason);
        assert!(
            matches!(
                result,
                Err(AutoCropDecline::WeakFront | AutoCropDecline::RicherElsewhere)
            ),
            "{binding:?}: {result:?}"
        );
    }
}

#[test]
fn cover_auto_photo_guard_follows_raw_gate_and_does_not_reach_jacket_rules() {
    let image = DynamicImage::new_rgb8(900, 600);
    let sample = crate::cover_auto_sample::PageAspectSample {
        aspect: Ok(2.0 / 3.0),
        dimensions: vec![Some((600.0, 900.0)); 3],
        units: crate::cover_auto_sample::PageDimensionUnits::Pixels,
    };
    for shot_exif in [false, true] {
        let mut reports = Vec::new();
        let results = detect_cover_crops_with_photo_report(
            &image,
            (900, 600),
            &sample,
            photo::PhotoEvidence {
                shot_exif,
                dimensions: (900.0, 600.0),
            },
            ReadingDirection::Rtl,
            |angle, result| reports.push((angle, result)),
        );
        assert_eq!(results.crops(), [None; 4]);
        for (angle, result) in reports {
            assert_eq!(
                result,
                Err(if matches!(angle, Rotation::Cw90 | Rotation::Cw270) {
                    AutoCropDecline::RawAspectGate
                } else {
                    AutoCropDecline::PhotoLike {
                        reason: if shot_exif {
                            photo::PhotoLikeReason::ShotExif
                        } else {
                            photo::PhotoLikeReason::TransposedPage
                        },
                    }
                })
            );
        }
    }
}

#[test]
fn cover_auto_photo_a_paper_image_and_pdf_reach_detector_instead_of_transpose_guard() {
    for (units, (width, height)) in [
        (
            crate::cover_auto_sample::PageDimensionUnits::Pixels,
            (2480.0, 3508.0),
        ),
        (
            crate::cover_auto_sample::PageDimensionUnits::PdfPoints,
            (595.28, 841.89),
        ),
    ] {
        let sample = crate::cover_auto_sample::PageAspectSample {
            aspect: Ok(width / height),
            dimensions: vec![Some((width, height)); 3],
            units,
        };
        let mut reports = Vec::new();
        detect_cover_crops_with_photo_report(
            &DynamicImage::new_rgb8(424, 300),
            (424, 300),
            &sample,
            photo::PhotoEvidence {
                shot_exif: false,
                dimensions: (height, width),
            },
            ReadingDirection::Rtl,
            |_, result| reports.push(result),
        );
        assert!(
            reports
                .into_iter()
                .all(|result| !matches!(result, Err(AutoCropDecline::PhotoLike { .. })))
        );
    }
}

#[test]
fn cover_auto_photo_keeps_missing_page_aspect_before_photo_reason() {
    let sample = crate::cover_auto_sample::PageAspectSample {
        aspect: Err(crate::cover_auto_sample::PageAspectDecline::TooFew),
        dimensions: Vec::new(),
        units: crate::cover_auto_sample::PageDimensionUnits::Pixels,
    };
    let mut reports = Vec::new();
    detect_cover_crops_with_photo_report(
        &DynamicImage::new_rgb8(900, 600),
        (900, 600),
        &sample,
        photo::PhotoEvidence {
            shot_exif: true,
            dimensions: (900.0, 600.0),
        },
        ReadingDirection::Rtl,
        |angle, result| reports.push((angle, result)),
    );
    for (angle, result) in reports {
        assert_eq!(
            result,
            Err(if matches!(angle, Rotation::Cw90 | Rotation::Cw270) {
                AutoCropDecline::RawAspectGate
            } else {
                AutoCropDecline::PageAspectMissing
            })
        );
    }
}

#[test]
fn cover_auto_photo_g2_matches_physical_and_saved_rotation_combinations() {
    use crate::cover_auto_sample::{PageAspectSample, PageDimensionUnits};
    let rotations = [
        Rotation::None,
        Rotation::Cw90,
        Rotation::Cw180,
        Rotation::Cw270,
    ];
    for units in [PageDimensionUnits::Pixels, PageDimensionUnits::PdfPoints] {
        let points = if units == PageDimensionUnits::PdfPoints {
            (90.075, 60.025)
        } else {
            (900.0, 600.0)
        };
        for shot_exif in [false, true] {
            let sample = PageAspectSample {
                aspect: Ok(points.1 / points.0),
                dimensions: vec![Some((points.1, points.0)); 3],
                units,
            };
            for physical in rotations {
                let image = match physical {
                    Rotation::None | Rotation::Cw180 => DynamicImage::new_rgb8(900, 600),
                    _ => DynamicImage::new_rgb8(600, 900),
                };
                let dims = (image.width(), image.height());
                let mut reports = Vec::new();
                detect_cover_crops_with_photo_report(
                    &image,
                    dims,
                    &sample,
                    photo::PhotoEvidence {
                        shot_exif,
                        dimensions: if matches!(physical, Rotation::Cw90 | Rotation::Cw270) {
                            (points.1, points.0)
                        } else {
                            points
                        },
                    },
                    ReadingDirection::Rtl,
                    |saved, result| reports.push((saved, result)),
                );
                for (saved, result) in reports {
                    let landscape = (physical.degrees() + saved.degrees()) % 180 == 0;
                    let expected = if landscape {
                        AutoCropDecline::PhotoLike {
                            reason: if shot_exif {
                                photo::PhotoLikeReason::ShotExif
                            } else {
                                photo::PhotoLikeReason::TransposedPage
                            },
                        }
                    } else {
                        AutoCropDecline::RawAspectGate
                    };
                    assert_eq!(
                        result,
                        Err(expected),
                        "{units:?}/{physical:?}/{saved:?}/{shot_exif}"
                    );
                }
            }
        }
    }
}
