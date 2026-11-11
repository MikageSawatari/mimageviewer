use super::*;
use image::Rgb;

#[test]
fn analysis_area_average_suppresses_row_noise_without_broadening_columns() {
    let image = RgbImage::from_fn(8, 256, |x, y| {
        Rgb([
            if x < 4 { 30 } else { 200 },
            if y == 128 { 255 } else { 0 },
            80,
        ])
    });
    let raster = analysis_raster(&DynamicImage::ImageRgb8(image));
    assert_eq!(raster.dimensions(), (8, 256));
    for (x, y, pixel) in raster.enumerate_pixels() {
        assert_eq!(pixel[0], if x < 4 { 30 } else { 200 });
        assert_eq!(
            pixel[1],
            match y {
                127 | 129 => 64,
                128 => 128,
                _ => 0,
            }
        );
        assert_eq!(pixel[2], 80);
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
fn jacket(binding: ReadingDirection, scan: bool, skew: bool, flap: bool) -> (RgbImage, [u32; 4]) {
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
    if scan {
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
                    let (image, truth) = jacket(binding, scan, skew, flap);
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
fn decoded_resolution_keeps_boundaries_within_one_analysis_column() {
    for binding in [ReadingDirection::Rtl, ReadingDirection::Ltr] {
        let (image, _) = jacket(binding, true, true, true);
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
        "v1:rtl"
    );
    assert_eq!(
        AutoCropIdentity::new(ReadingDirection::Ltr).suffix(),
        "v1:ltr"
    );
}

#[test]
fn content_margin_tolerates_sparse_dust_and_remains_asymmetric() {
    let (image, _) = jacket(ReadingDirection::Rtl, true, false, true);
    let bounds = content_bounds(&Raster::from_image(&image));
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
        let (mut image, front) = jacket(binding, false, false, false);
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
        let result = cover_auto_crop(&raster, dims, Some(0.7), binding);
        assert!(
            matches!(
                result,
                Err(AutoCropDecline::WeakFront | AutoCropDecline::RicherElsewhere)
            ),
            "{binding:?}: {result:?}"
        );
    }
}
