use super::*;

const PAPER: [u8; 3] = [245; 3];

fn art(_: u32, y: u32) -> [u8; 3] {
    [
        35 + (y % 80) as u8,
        55 + (y % 155) as u8,
        200 - (y % 80) as u8,
    ]
}

fn two_cover(
    outer: u32,
    front: u32,
    spine: u32,
    outer_pixel: impl Fn(u32, u32) -> [u8; 3],
    front_pixel: impl Fn(u32, u32) -> [u8; 3],
) -> RgbImage {
    let width = outer + front + spine;
    RgbImage::from_fn(width, 256, |x, y| {
        let mut pixel = if x < outer {
            outer_pixel(x, y)
        } else if x < outer + front {
            front_pixel(x - outer, y)
        } else {
            [18, 28, 38]
        };
        // Sparse marks only on the two outermost columns stop margin removal
        // without turning the whole paper band into a v5 ink-bearing flap.
        if x < 2 && y % 31 < 4 {
            pixel = [20; 3];
        }
        if x >= width - 2 && y % 31 < 4 {
            pixel = PAPER;
        }
        image::Rgb(pixel)
    })
}

fn final_two_rejection(image: &RgbImage, aspect: f64) -> (TwoPanelTrigger, TwoPanelDecline) {
    let result = cover_auto_crop(
        image,
        image.dimensions(),
        Some(aspect),
        ReadingDirection::Rtl,
    );
    match result {
        Err(AutoCropDecline::TwoPanel { v5, reason }) => (v5, reason),
        other => panic!("expected the two-panel rejection, got {other:?}"),
    }
}

#[test]
fn two_panel_paper_front_both_bindings_and_four_saved_rotations() {
    for spine in [0, 18] {
        let image = two_cover(80, 180, spine, |_, _| PAPER, art);
        for binding in [ReadingDirection::Rtl, ReadingDirection::Ltr] {
            let pixels = if binding == ReadingDirection::Ltr {
                image::imageops::flip_horizontal(&image)
            } else {
                image.clone()
            };
            let source = Raster::from_image(&pixels);
            let bounds = content_bounds(&source, |x, y| source.get(x, y));
            assert_eq!(bounds, [0, 0, pixels.width() as usize, 256]);
            let sub = source.region(bounds, binding == ReadingDirection::Ltr);
            let features = Features::new(&sub);
            assert!(
                v5_candidate(&sub, 0.7 * 256.0, &features, &panel_edges(&sub)).is_err(),
                "fixture must exercise the new path"
            );
            let upright =
                cover_auto_crop(&pixels, pixels.dimensions(), Some(0.7), binding).unwrap();
            let (start, end) = if binding == ReadingDirection::Ltr {
                (spine, 180 + spine)
            } else {
                (80, 260)
            };
            let (x, y, width, height) = upright.pixel_bounds(pixels.width(), pixels.height());
            assert!(x.abs_diff(start) <= 3 && (x + width).abs_diff(end) <= 3);
            assert_eq!((y, height), (0, 256));
            if spine > 0 {
                assert!(width < 180 + spine, "the end spine must be excluded");
            }
            let image = DynamicImage::ImageRgb8(pixels);
            for physical in [
                Rotation::None,
                Rotation::Cw90,
                Rotation::Cw180,
                Rotation::Cw270,
            ] {
                let rotated = match physical {
                    Rotation::None => image.clone(),
                    Rotation::Cw90 => image.rotate90(),
                    Rotation::Cw180 => image.rotate180(),
                    Rotation::Cw270 => image.rotate270(),
                };
                let saved = Rotation::from_degrees(-physical.degrees());
                let crop = detect_cover_crops(
                    &rotated,
                    (rotated.width(), rotated.height()),
                    Some(0.7),
                    binding,
                )
                .crop_for(saved)
                .unwrap();
                let [a, b, c, d] = upright.values();
                let expected = match physical {
                    Rotation::None => [a, b, c, d],
                    Rotation::Cw90 => [1_000_000 - d, a, 1_000_000 - b, c],
                    Rotation::Cw180 => [1_000_000 - c, 1_000_000 - d, 1_000_000 - a, 1_000_000 - b],
                    Rotation::Cw270 => [b, 1_000_000 - c, d, 1_000_000 - a],
                };
                assert_eq!(
                    crop.values(),
                    expected,
                    "{binding:?} {physical:?} spine={spine}"
                );
            }
        }
    }
}

#[test]
fn two_panel_art_on_outer_band_and_near_square_illustrations_decline() {
    for (outer, front) in [(80, 180), (135, 135)] {
        let image = two_cover(
            outer,
            front,
            0,
            |_, y| {
                [
                    200 - (y % 110) as u8,
                    30 + (y % 100) as u8,
                    70 + (y % 90) as u8,
                ]
            },
            art,
        );
        for binding in [ReadingDirection::Rtl, ReadingDirection::Ltr] {
            let image = if binding == ReadingDirection::Ltr {
                image::imageops::flip_horizontal(&image)
            } else {
                image.clone()
            };
            assert!(cover_auto_crop(&image, image.dimensions(), Some(0.7), binding).is_err());
        }
    }
}

#[test]
fn two_panel_final_outer_width_rejects_just_over_point_eight() {
    // w=200 exactly. Both shapes pass end/w; the final boundary must enforce
    // bx<=160 rather than inferring the band's width from end/w alone.
    let aspect = 200.0 / 256.0;
    let accepted = two_cover(160, 190, 0, |_, _| PAPER, art);
    assert!(
        cover_auto_crop(
            &accepted,
            accepted.dimensions(),
            Some(aspect),
            ReadingDirection::Rtl
        )
        .is_ok()
    );
    let wide = two_cover(161, 190, 0, |_, _| PAPER, art);
    assert_eq!(
        final_two_rejection(&wide, aspect).1,
        TwoPanelDecline::OuterWide
    );
}

#[test]
fn two_panel_signed_ink_gap_rejects_a_darker_outer_band() {
    let image = two_cover(
        80,
        180,
        0,
        |_, y| if y % 10 < 3 { [80; 3] } else { PAPER },
        |_, y| if y % 10 < 1 { [80; 3] } else { PAPER },
    );
    assert_eq!(
        final_two_rejection(&image, 0.7).1,
        TwoPanelDecline::NotPaper
    );
}

#[test]
fn two_panel_candidate_still_obeys_weak_front_and_richer_elsewhere_vetoes() {
    let weak = two_cover(
        80,
        180,
        0,
        |_, _| PAPER,
        // Both grays form a strict edge against paper. 220 is within the
        // paper-distance ink threshold, while 205 is outside: 75% ink gives
        // a gray front score of 0.45, below the unchanged 0.6 veto.
        |_, y| if y % 4 == 0 { [220; 3] } else { [205; 3] },
    );
    assert_eq!(
        final_two_rejection(&weak, 0.7),
        (TwoPanelTrigger::NoLocalEvidence, TwoPanelDecline::WeakFront)
    );
    let richer = two_cover(
        130,
        180,
        0,
        |x, y| {
            if (30..65).contains(&x) {
                if y % 16 < 8 {
                    [0, 240, 240]
                } else {
                    [240, 0, 240]
                }
            } else {
                PAPER
            }
        },
        |_, _| [60, 65, 70],
    );
    assert_eq!(
        final_two_rejection(&richer, 0.7),
        (
            TwoPanelTrigger::NoLocalEvidence,
            TwoPanelDecline::RicherElsewhere
        )
    );
}

#[test]
fn two_panel_fallback_whitelist_preserves_entry_and_v5_veto_declines() {
    for reason in [
        AutoCropDecline::NoJacketEvidence,
        AutoCropDecline::NoLocalEvidence,
        AutoCropDecline::NoSpine,
        AutoCropDecline::WeakFront,
    ] {
        assert_eq!(reason.two_panel_trigger().unwrap().decline(), reason);
    }
    for reason in [
        AutoCropDecline::RawAspectGate,
        AutoCropDecline::PageAspectMissing,
        AutoCropDecline::NotWide,
        AutoCropDecline::NoStructure,
        AutoCropDecline::RicherElsewhere,
    ] {
        assert!(reason.two_panel_trigger().is_none());
    }
    let report = AutoCropDecline::TwoPanel {
        v5: TwoPanelTrigger::NoLocalEvidence,
        reason: TwoPanelDecline::RicherElsewhere,
    };
    assert_eq!(report.as_str(), "no-local-evidence/two-richer");
}

#[test]
fn white_front_content_margin_stops_at_five_percent_on_each_edge() {
    let mut image = RgbImage::from_pixel(400, 256, image::Rgb(PAPER));
    for y in 70..190 {
        for x in 100..300 {
            image.put_pixel(x, y, image::Rgb([80, 50, 160]));
        }
    }
    let raster = Raster::from_image(&image);
    assert_eq!(
        content_bounds(&raster, |x, y| raster.get(x, y)),
        [20, 12, 380, 244]
    );
}

#[test]
fn two_panel_band_std_uses_all_rgb_values_not_luminance_variance() {
    let raster = Raster {
        width: 2,
        height: 1,
        pixels: vec![[0.0; 3], [90.0, 120.0, 150.0]],
    };
    let (mean, std) = rgb_band_stats(&raster, 1, 2);
    assert_eq!(mean, [90.0, 120.0, 150.0]);
    assert!((std - 600.0f64.sqrt()).abs() < 1e-9);
}
