use super::*;

fn fill(region: BucketRegion, value: bool) -> BucketFill {
    BucketFill {
        tolerance: 255,
        region,
        value,
        leak_stop: 0.0,
        outset: 0.0,
        gap_tolerance: 0.0,
    }
}

fn outline() -> Vec<bool> {
    let mut mask = vec![false; 32 * 32];
    for y in 6..26 {
        for x in 6..26 {
            mask[y * 32 + x] = x == 6 || x == 25 || y == 6 || y == 25;
        }
    }
    mask
}

#[test]
fn mask_bucket_1673_closed_outline_fills_only_inside() {
    let mut mask = outline();
    assert_eq!(
        flood_fill_drawn_mask(
            &mut mask,
            &[],
            [32, 32],
            16,
            16,
            fill(BucketRegion::Connected, true)
        ),
        BucketFillOutcome::Filled
    );
    for y in 0..32 {
        for x in 0..32 {
            assert_eq!(
                mask[y * 32 + x],
                (6..26).contains(&x) && (6..26).contains(&y)
            );
        }
    }
}

#[test]
fn mask_bucket_1673_leak_stop_closes_small_break() {
    let mut broken = outline();
    broken[6 * 32 + 16] = false;
    let mut leaking = broken.clone();
    assert_eq!(
        flood_fill_drawn_mask(
            &mut leaking,
            &[],
            [32, 32],
            16,
            16,
            fill(BucketRegion::Connected, true)
        ),
        BucketFillOutcome::Filled
    );
    assert!(leaking.iter().all(|&v| v));
    assert_eq!(
        flood_fill_drawn_mask(
            &mut broken,
            &[],
            [32, 32],
            16,
            16,
            BucketFill {
                leak_stop: 1.0,
                ..fill(BucketRegion::Connected, true)
            }
        ),
        BucketFillOutcome::Filled
    );
    assert!(broken[16 * 32 + 16]);
    assert!(!broken[0]);
    assert!(!broken[31 * 32 + 31]);
}

#[test]
fn mask_bucket_1673_unbounded_region_reaches_image_edges() {
    let mut mask = vec![false; 32 * 32];
    assert_eq!(
        flood_fill_drawn_mask(
            &mut mask,
            &[],
            [32, 32],
            16,
            16,
            BucketFill {
                leak_stop: 2.0,
                ..fill(BucketRegion::Connected, true)
            }
        ),
        BucketFillOutcome::Filled
    );
    assert!(mask.iter().all(|&v| v));
}

#[test]
fn mask_bucket_1673_matching_seed_is_unchanged_even_for_whole() {
    for region in [
        BucketRegion::Whole,
        BucketRegion::Connected,
        BucketRegion::Rect,
        BucketRegion::Ellipse,
        BucketRegion::Circle,
    ] {
        for value in [false, true] {
            let mut mask = outline();
            let before = mask.clone();
            let seed = if value { (6, 6) } else { (16, 16) };
            assert_eq!(
                flood_fill_drawn_mask(
                    &mut mask,
                    &[],
                    [32, 32],
                    seed.0,
                    seed.1,
                    fill(region, value)
                ),
                BucketFillOutcome::SeedAlreadyValue
            );
            assert_eq!(mask, before);
        }
    }
}

#[test]
fn mask_bucket_1673_composite_shapes_are_walls_in_creation_order() {
    let outer = Shape::Rect {
        op: ShapeOp::Add,
        center: (16.0, 16.0),
        half_w: 10.0,
        half_h: 10.0,
        rotation_rad: 0.0,
    };
    let inner = Shape::Rect {
        op: ShapeOp::Subtract,
        center: (16.0, 16.0),
        half_w: 8.0,
        half_h: 8.0,
        rotation_rad: 0.0,
    };
    let mut mask = vec![false; 32 * 32];
    assert_eq!(
        flood_fill_drawn_mask(
            &mut mask,
            &[outer.clone(), inner.clone()],
            [32, 32],
            16,
            16,
            fill(BucketRegion::Connected, true)
        ),
        BucketFillOutcome::Filled
    );
    assert!(mask[16 * 32 + 16]);
    assert!(!mask[0]);
    assert!(
        !mask[6 * 32 + 6],
        "only bitmap inside is written; shapes are retained separately"
    );
    let mut blocked = vec![false; 32 * 32];
    assert_eq!(
        flood_fill_drawn_mask(
            &mut blocked,
            &[inner, outer],
            [32, 32],
            16,
            16,
            fill(BucketRegion::Connected, true)
        ),
        BucketFillOutcome::SeedAlreadyValue
    );
    assert!(blocked.iter().all(|&v| !v));
}

#[test]
fn mask_bucket_1673_erase_removes_only_clicked_component() {
    let mut mask = outline();
    mask[0] = true;
    assert_eq!(
        flood_fill_drawn_mask(
            &mut mask,
            &[],
            [32, 32],
            6,
            6,
            fill(BucketRegion::Connected, false)
        ),
        BucketFillOutcome::Filled
    );
    assert!(mask[0]);
    assert_eq!(mask.iter().filter(|&&v| v).count(), 1);
}

#[test]
fn mask_bucket_1673_all_region_modes_and_outset_reuse_color_algorithm() {
    for region in [
        BucketRegion::Whole,
        BucketRegion::Connected,
        BucketRegion::Rect,
        BucketRegion::Ellipse,
        BucketRegion::Circle,
    ] {
        for outset in [0.0, 1.0] {
            for leak_stop in [0.0, 1.0] {
                for gap_tolerance in [0.0, 10.0] {
                    let before = outline();
                    let image = egui::ColorImage::new(
                        [32, 32],
                        before
                            .iter()
                            .map(|&v| {
                                if v {
                                    egui::Color32::WHITE
                                } else {
                                    egui::Color32::BLACK
                                }
                            })
                            .collect(),
                    );
                    let options = BucketFill {
                        tolerance: 0,
                        outset,
                        leak_stop,
                        gap_tolerance,
                        ..fill(region, true)
                    };
                    let mut expected = before.clone();
                    let mut actual = before;
                    assert_eq!(
                        flood_fill_bitmap_mask(&mut expected, &image, 16, 16, options),
                        BucketFillOutcome::Filled
                    );
                    assert_eq!(
                        flood_fill_drawn_mask(&mut actual, &[], [32, 32], 16, 16, options),
                        BucketFillOutcome::Filled
                    );
                    assert_eq!(
                        actual, expected,
                        "{region:?}, {outset}, {leak_stop}, {gap_tolerance}"
                    );
                    assert!(actual[16 * 32 + 16]);
                    if region != BucketRegion::Whole {
                        assert!(!actual[0]);
                    }
                }
            }
        }
    }
}

#[test]
fn mask_bucket_1673_invalid_input_preserves_mask() {
    for size in [[0, 32], [32, 0], [32, 33]] {
        let mut mask = outline();
        let before = mask.clone();
        assert_eq!(
            flood_fill_drawn_mask(
                &mut mask,
                &[],
                size,
                0,
                0,
                fill(BucketRegion::Connected, true)
            ),
            BucketFillOutcome::Invalid
        );
        assert_eq!(mask, before);
    }
}

#[test]
fn mask_bucket_1673_shape_outline_stays_filled_after_compositing() {
    let shapes: Vec<_> = [
        ((6.5, 6.5), (25.5, 6.5)),
        ((25.5, 6.5), (25.5, 25.5)),
        ((25.5, 25.5), (6.5, 25.5)),
        ((6.5, 25.5), (6.5, 6.5)),
    ]
    .into_iter()
    .map(|(p0, p1)| Shape::Line {
        op: ShapeOp::Add,
        kind: LineKind::Diagonal,
        p0,
        p1,
        thickness: 2.0,
    })
    .collect();
    let mut mask = vec![false; 32 * 32];
    assert_eq!(
        flood_fill_drawn_mask(
            &mut mask,
            &shapes,
            [32, 32],
            16,
            16,
            fill(BucketRegion::Connected, true)
        ),
        BucketFillOutcome::Filled
    );
    rasterize_shapes_into(&mut mask, &shapes, 32, 32);
    assert!(mask[16 * 32 + 16]);
    assert!(!mask[0]);
    assert!(!mask[31 * 32 + 31]);
}

#[test]
fn mask_bucket_1673_local_shape_coverage_matches_alpha_renderer() {
    use local_adjust_core::{MaskShape as S, ShapeOp as O};
    let cases = [
        S::Rect {
            op: O::Add,
            center: [13.2, 14.8],
            half_w: 7.3,
            half_h: 5.6,
            rotation_rad: 0.37,
        },
        S::Rect {
            op: O::Add,
            center: [13.2, 14.8],
            half_w: 0.1,
            half_h: 0.1,
            rotation_rad: 0.0,
        },
        S::Line {
            op: O::Add,
            kind: local_adjust_core::LineKind::Diagonal,
            p0: [-1.3, 4.2],
            p1: [33.8, 25.6],
            thickness: 0.2,
        },
        S::Line {
            op: O::Add,
            kind: local_adjust_core::LineKind::Diagonal,
            p0: [2.3, 4.2],
            p1: [25.8, 20.6],
            thickness: 3.7,
        },
        S::Ellipse {
            op: O::Add,
            center: [13.2, 14.8],
            rx: 7.3,
            ry: 5.6,
            rotation_rad: 0.37,
        },
        S::Ellipse {
            op: O::Add,
            center: [13.2, 14.8],
            rx: 0.1,
            ry: 0.1,
            rotation_rad: 0.0,
        },
    ];
    for shape in cases {
        for shapes in [
            vec![shape],
            vec![shape, shape.with_op(O::Subtract)],
            vec![shape, shape.with_op(O::Subtract), shape],
        ] {
            let mut alpha: Vec<f32> = (0..32 * 32)
                .map(|i| if i % 7 == 0 { 0.5 } else { 0.49 })
                .collect();
            let mut bitmap: Vec<bool> = alpha.iter().map(|&a| a >= 0.5).collect();
            local_adjust_core::rasterize_shapes_into(&mut alpha, 32, 32, &shapes);
            local_adjust_core::rasterize_shapes_into_bitmap(&mut bitmap, 32, 32, &shapes);
            assert_eq!(
                bitmap,
                alpha.iter().map(|&a| a >= 0.5).collect::<Vec<_>>(),
                "{shapes:?}"
            );
        }
    }
}
