//! Revision 3: bounded cover progress, geometry-preserving exit, and partition measurements.
use super::*;
use crate::displayed_image_transform::RectPixelFit;
use crate::settings::FullscreenWheelZoomStep;
use egui::{pos2, vec2};

fn canvas(size: Vec2) -> Rect {
    Rect::from_min_size(Pos2::ZERO, size)
}

fn input(
    cover: PlaneCoverInput,
    current: Option<PlaneViewPose>,
    zoom: f32,
    pan: Vec2,
    center: Pos2,
    notches: f32,
    step: FullscreenWheelZoomStep,
) -> PlaneViewWheelInput {
    PlaneViewWheelInput {
        current,
        anchor: PlaneViewAnchor {
            fs_idx: 0,
            item: crate::item_identity::ItemId::NONE,
            items_generation: 1,
            slice: crate::page_split::PageSlice::Full,
        },
        zoom,
        pan,
        pointer: Some(center),
        canvas: cover.transform.viewport_rect,
        notches,
        allowed: Ok(()),
        step,
        cover: Some(cover),
        tilt_is_identity: true,
        tilt_drag_owned: false,
    }
}

fn run(
    cover: PlaneCoverInput,
    zoom: f32,
    pan: Vec2,
    center: Pos2,
    parts: &[f32],
    step: FullscreenWheelZoomStep,
) -> PlaneViewWheelOutcome {
    let mut out = PlaneViewWheelOutcome {
        pose: None,
        zoom,
        pan,
        blocked: None,
    };
    for &notches in parts {
        out = plane_view_wheel_transition(input(
            cover, out.pose, out.zoom, out.pan, center, notches, step,
        ));
    }
    out
}

fn close(a: f32, b: f32, tolerance: f32) {
    assert!(
        (a - b).abs() <= tolerance,
        "{a} != {b}; tolerance={tolerance}"
    );
}

#[test]
fn plane_revision3_portrait_cover_takes_three_bounded_notches_and_fill_starts_before_target() {
    let canvas = canvas(vec2(1920., 1080.));
    let cover = super::revision_tests::fixture(canvas, vec2(600., 1080.));
    let center = canvas.center();
    let (target_zoom, target_pan) = cover.cover(1., Vec2::ZERO, center).unwrap();
    close(target_zoom, 1928. / 600., 1.0e-5);
    // Inward cover resolves continuous f32 geometry; Vec2's Debug hides small
    // nonzero components behind one decimal place. This is not an outward copy.
    close(target_pan.x, 0., 0.001);
    close(target_pan.y, 0., 0.001);
    let mut zoom = 1.;
    let mut pan = Vec2::ZERO;
    for notch in 0..3 {
        let (next_zoom, next_pan, remaining) = cover.advance_cover(zoom, pan, center, 1.).unwrap();
        assert!(next_zoom / zoom <= 1.5 + 1.0e-6);
        assert!((next_pan - pan).length() <= canvas.size().length() * 0.25 + 1.0e-4);
        if notch < 2 {
            close(next_zoom / zoom, 1.5, 1.0e-6);
            assert_eq!(remaining, 0.);
        } else {
            close(next_zoom, target_zoom, 1.0e-5);
            assert!((next_pan - target_pan).length() < 0.001);
            assert!(remaining > 0. && remaining < 1.);
        }
        zoom = next_zoom;
        pan = next_pan;
    }
    for lens in PlaneLensKind::ALL {
        let coefficient = |zoom| {
            let transform = cover
                .resolve(zoom, Vec2::ZERO, RectPixelFit::Texels)
                .unwrap();
            plane_fill_coefficient(
                canvas,
                center,
                plane_effective_quad(&transform, cover.material).unwrap(),
                lens,
            )
        };
        assert_eq!(coefficient(1.5), 0.);
        // 3.205 covers the actual 1920 px canvas, but has not reached canvas+ (1928 px).
        let before_target = coefficient(3.205);
        assert!(before_target > 0. && before_target < coefficient(target_zoom));
        assert!(coefficient(3.207) >= before_target);
    }
}

#[test]
fn plane_revision3_near_unit_scale_and_translation_only_respect_the_pan_bound() {
    let canvas = canvas(Vec2::splat(1000.));
    let cover = super::revision_tests::fixture(canvas, Vec2::splat(1000.));
    let center = canvas.center();
    for (zoom, pan) in [(1.007, vec2(900., 0.)), (2., vec2(1500., 0.))] {
        let (target_zoom, target_pan) = cover.cover(zoom, pan, center).unwrap();
        if zoom == 1.007 {
            close(target_zoom, 1.008, 1.0e-6);
            close(target_pan.x, 0., 0.001);
        } else {
            assert_eq!(target_zoom, zoom, "sigma=1 must be a pure translation");
        }
        for notches in [0.01, 0.25, 1.] {
            let (next_zoom, next_pan, remaining) =
                cover.advance_cover(zoom, pan, center, notches).unwrap();
            assert!(next_zoom.is_finite() && next_pan.is_finite());
            assert!(next_zoom / zoom <= 1.5_f32.powf(notches) + 1.0e-6);
            assert!((next_pan - pan).length() <= notches * canvas.size().length() * 0.25 + 0.001);
            assert_eq!(remaining, 0.);
            assert!(next_pan.x < pan.x && next_pan.x > target_pan.x);
        }
    }
}

#[test]
fn plane_revision3_cover_remainder_uses_the_configured_normal_zoom_including_double() {
    let canvas = canvas(vec2(1920., 1080.));
    let cover = super::revision_tests::fixture(canvas, vec2(600., 1080.));
    let center = canvas.center();
    let (target_zoom, target_pan) = cover.cover(1., Vec2::ZERO, center).unwrap();
    let cost = f64::from(target_zoom).ln() / 1.5_f64.ln();
    for step in [
        FullscreenWheelZoomStep::Standard,
        FullscreenWheelZoomStep::Percent100,
    ] {
        let out = run(cover, 1., Vec2::ZERO, center, &[4.], step);
        let mut expected_zoom = target_zoom;
        crate::app::App::apply_wheel_zoom(
            &mut expected_zoom,
            &mut Vec2::ZERO,
            (4. - cost) as f32 * 40.,
            step,
            None,
            center,
        );
        close(out.zoom, expected_zoom, 1.0e-5);
        assert!((out.pan - target_pan).length() < 0.001);
        assert_eq!(out.pose.unwrap().strength, 1.);
        if step == FullscreenWheelZoomStep::Percent100 {
            assert!(
                out.zoom / target_zoom > 1.5,
                "F limits only cover progress, never the normal remainder"
            );
        }
    }
}

fn corner_difference(
    cover: PlaneCoverInput,
    a: PlaneViewWheelOutcome,
    b: PlaneViewWheelOutcome,
) -> f32 {
    let a = cover
        .continuous(a.zoom, a.pan)
        .unwrap()
        .paint_quad()
        .positions;
    let b = cover
        .continuous(b.zoom, b.pan)
        .unwrap()
        .paint_quad()
        .positions;
    a.into_iter()
        .zip(b)
        .map(|(a, b)| (a - b).length() * cover.transform.pixels_per_point)
        .fold(0., f32::max)
}

#[test]
fn plane_revision3_partition_measurements_match_without_terminal_overlap_correction() {
    for (size, source, zoom, pan, center) in [
        (
            vec2(1920., 1080.),
            vec2(600., 1080.),
            1.,
            Vec2::ZERO,
            pos2(960., 540.),
        ),
        (
            Vec2::splat(1000.),
            Vec2::splat(1000.),
            1.007,
            vec2(900., 0.),
            pos2(500., 500.),
        ),
        (
            Vec2::splat(1000.),
            Vec2::splat(1000.),
            2.,
            vec2(1450., 0.),
            pos2(500., 500.),
        ),
    ] {
        let cover = super::revision_tests::fixture(canvas(size), source);
        let initial = cover.continuous(zoom, pan).unwrap();
        assert_eq!(
            crate::ui_fullscreen::pan_correction_for_minimum_overlap(
                Rect::from_points(&initial.paint_quad().positions),
                initial.viewport_rect,
            ),
            Vec2::ZERO,
            "partition equality fixtures must start inside the minimum-overlap constraint",
        );
        for step in [
            FullscreenWheelZoomStep::Percent10,
            FullscreenWheelZoomStep::Percent100,
        ] {
            let joined = run(cover, zoom, pan, center, &[4.], step);
            for parts in [
                &[1., 1., 1., 1.][..],
                &[0.125; 32][..],
                &[0.75, 1.5, 0.25, 1.5][..],
            ] {
                let split = run(cover, zoom, pan, center, parts, step);
                let error = corner_difference(cover, joined, split);
                eprintln!(
                    "revision3 partition: size={size:?} source={source:?} start=({zoom},{pan:?}) step={step:?} parts={parts:?} zoom_delta={} pan_delta={} corner_error_physical={error}",
                    (joined.zoom - split.zoom).abs(),
                    (joined.pan - split.pan).length()
                );
                close(joined.zoom, split.zoom, 1.0e-4);
                assert!(
                    error <= 1. / 64.,
                    "partition changed the displayed placement: {error}"
                );
                assert_eq!(joined.pose, split.pose);
            }
        }
    }
}

#[test]
fn plane_revision3_partition_measurement_records_the_terminal_overlap_exception() {
    let canvas = canvas(Vec2::splat(1000.));
    let cover = super::revision_tests::fixture(canvas, Vec2::splat(1000.));
    // A resize can leave a placement 3 px outside the 48 px minimum-overlap boundary.
    // Both segments are pure translation (sigma=1); only the first split segment is clamped.
    let joined = run(
        cover,
        2.,
        vec2(1455., 0.),
        canvas.center(),
        &[0.002],
        FullscreenWheelZoomStep::Percent10,
    );
    let split = run(
        cover,
        2.,
        vec2(1455., 0.),
        canvas.center(),
        &[0.001, 0.001],
        FullscreenWheelZoomStep::Percent10,
    );
    let expected_split_pan = 1452. - 0.001 * canvas.size().length() * 0.25;
    close(joined.pan.x, 1452., 0.001);
    close(split.pan.x, expected_split_pan, 0.001);
    assert_eq!((joined.zoom, split.zoom), (2., 2.));
    let error = corner_difference(cover, joined, split);
    eprintln!(
        "revision3 terminal-overlap exception: joined={joined:?} split={split:?} corner_error_physical={error}"
    );
    close(error, 0.001 * canvas.size().length() * 0.25, 0.001);
    assert!(
        error < 0.5,
        "retain the small terminal-overlap exception's measured bound"
    );
}

#[test]
fn plane_revision3_resize_overlap_preserves_the_accepted_intermediate_difference() {
    let canvas = canvas(Vec2::splat(1000.));
    let cover = super::revision_tests::fixture(canvas, Vec2::splat(1000.));
    // A pan allowed in a larger viewport can be outside the resized viewport.
    // sigma=1, C.pan.x=496, so uncorrected progress moves exactly P per notch.
    let start_pan = vec2(2000., 0.);
    let joined = run(
        cover,
        2.,
        start_pan,
        canvas.center(),
        &[2.],
        FullscreenWheelZoomStep::Percent10,
    );
    let split = run(
        cover,
        2.,
        start_pan,
        canvas.center(),
        &[1., 1.],
        FullscreenWheelZoomStep::Percent10,
    );
    let p = canvas.size().length() * 0.25;
    close(joined.pan.x, 2000. - 2. * p, 0.001);
    // First split segment reaches pan=2000-P, then the 48 px rule moves it to 1452.
    // The second segment advances another P from that corrected placement.
    close(split.pan.x, 1452. - p, 0.001);
    assert_eq!((joined.zoom, split.zoom), (2., 2.));
    assert_eq!(joined.pose, split.pose);
    let error = corner_difference(cover, joined, split);
    close(error, 548. - p, 0.001);
    // This accepted exception is limited to segments where terminal minimum-overlap
    // correction changes the path. It is not a tolerance for ordinary cover progress.
    assert!(error > 190. && error <= 194.45);
    eprintln!(
        "revision3 accepted terminal-overlap exception: joined={joined:?} split={split:?} corner_error_physical={error}"
    );
}

#[test]
fn plane_revision3_resize_overlap_preserves_the_same_cover_and_accepted_completed_difference() {
    let canvas = canvas(Vec2::splat(1000.));
    let cover = super::revision_tests::fixture(canvas, Vec2::splat(1000.));
    let center = canvas.center();
    let start_pan = vec2(2000., 0.);
    let step = FullscreenWheelZoomStep::Percent10;
    let initial_target = cover.cover(2., start_pan, center).unwrap();
    let first = run(cover, 2., start_pan, center, &[1.], step);
    let corrected_target = cover.cover(first.zoom, first.pan, center).unwrap();
    assert_eq!(initial_target.0, 2.);
    close(initial_target.1.x, 496., 0.001);
    assert_eq!(corrected_target.0, initial_target.0);
    assert!((corrected_target.1 - initial_target.1).length() < 0.001);
    close(first.pan.x, 1452., 0.001);
    close(first.pan.y, 0., 0.001);

    // Inspect the actual cover stage, before each segment spends its normal remainder.
    let joined_cover = cover.advance_cover(2., start_pan, center, 5.).unwrap();
    let split_cover = cover
        .advance_cover(first.zoom, first.pan, center, 4.)
        .unwrap();
    for (zoom, pan, remaining) in [joined_cover, split_cover] {
        assert_eq!(zoom, initial_target.0);
        assert!((pan - initial_target.1).length() < 0.001);
        assert!(remaining > 0.);
    }

    let joined = run(cover, 2., start_pan, center, &[5.], step);
    let split = run(cover, 2., start_pan, center, &[1.; 5], step);
    // The terminal correction saves part of the split cover cost. That saving becomes
    // extra normal zoom after C; reaching the same C does not imply equal final output.
    let p = 250. * 2_f64.sqrt();
    let joined_cost = 1504. / p;
    let split_cost = 1. + 956. / p;
    let joined_factor = 1.1_f64.powf(5. - joined_cost);
    let split_factor = 1.1_f64.powf(5. - split_cost);
    close(joined_cover.2, (5. - joined_cost) as f32, 1.0e-5);
    close(split_cover.2, (5. - split_cost) as f32, 1.0e-5);
    close(joined.zoom, (2. * joined_factor) as f32, 1.0e-4);
    close(split.zoom, (2. * split_factor) as f32, 1.0e-4);
    close(joined.pan.x, (496. * joined_factor) as f32, 0.002);
    close(split.pan.x, (496. * split_factor) as f32, 0.002);
    assert_eq!(joined.pose, split.pose);
    assert_eq!(joined.pose.unwrap().strength, 1.);
    let ratio = split.zoom / joined.zoom;
    close(ratio, 1.1_f64.powf(548. / p - 1.) as f32, 1.0e-5);
    close((ratio - 1.) * 100., 5.4, 0.03);
    assert!((split.pan - joined.pan).length() > 25.);
    for out in [joined, split] {
        let transform = cover.continuous(out.zoom, out.pan).unwrap();
        let q = plane_effective_quad(&transform, cover.material).unwrap();
        let bounds = Rect::from_points(&q);
        assert!(
            bounds.contains_rect(canvas.expand(4.)),
            "both outputs have completed cover"
        );
    }

    // Record the accepted intermediate bound for this example through completion,
    // sampling the same input at 1/16-notch boundaries with and without integer splits.
    // Only minimum-overlap-corrected paths are allowed this 194 px exception.
    let mut maximum_error = 0_f32;
    for sample in 1..=80 {
        let total = sample as f32 / 16.;
        let mut parts = vec![1.; total.floor() as usize];
        let remainder = total.fract();
        if remainder > 0. {
            parts.push(remainder);
        }
        let whole = run(cover, 2., start_pan, center, &[total], step);
        let partitioned = run(cover, 2., start_pan, center, &parts, step);
        maximum_error = maximum_error.max(corner_difference(cover, whole, partitioned));
    }
    close(maximum_error, (548. - p) as f32, 0.004);
    assert!(maximum_error <= 194.45);
    eprintln!(
        "revision3 accepted terminal-overlap completion exception: joined={joined:?} split={split:?} zoom_ratio={ratio} sampled_maximum_corner_error_physical={maximum_error}; both reach the same C before spending their different normal remainders"
    );
}

#[test]
fn plane_revision3_outward_resize_retains_geometry_and_only_strength_and_lifetime_change() {
    let canvas = canvas(vec2(400., 300.));
    let cover = super::revision_tests::fixture(canvas, vec2(600., 900.));
    let base = input(
        cover,
        None,
        1.,
        Vec2::ZERO,
        canvas.center(),
        1.,
        FullscreenWheelZoomStep::Percent10,
    );
    let pose = PlaneViewPose {
        anchor: base.anchor,
        strength: 0.5,
        lens_center: Vec2::splat(0.5),
    };
    let pan = vec2(4000., -3000.);
    for (identity, owned) in [(true, false), (false, false), (true, true)] {
        let mut current = Some(pose);
        for (notches, expected_strength) in [(-1., 0.25), (-8., 0.), (-1., 0.)] {
            let mut input = input(
                cover,
                current,
                2.75,
                pan,
                canvas.center(),
                notches,
                FullscreenWheelZoomStep::Percent100,
            );
            input.tilt_is_identity = identity;
            input.tilt_drag_owned = owned;
            let out = plane_view_wheel_transition(input);
            assert_eq!(
                (out.zoom, out.pan),
                (2.75, pan),
                "outward must not clamp even a resize-invalid placement"
            );
            assert_eq!(
                out.pose.is_some(),
                expected_strength > 0. || !identity || owned
            );
            if let Some(pose) = out.pose {
                assert_eq!(pose.strength, expected_strength);
            }
            current = out.pose;
        }
    }
}

#[test]
fn plane_revision3_outward_preserves_zoom_and_pan_bits_including_signed_zero() {
    let canvas = canvas(Vec2::splat(1000.));
    let cover = super::revision_tests::fixture(canvas, Vec2::splat(1000.));
    for pan in [vec2(-0., 0.), vec2(0., -0.), vec2(4000., -3000.)] {
        for strength in [0., 0.5] {
            for (identity, owned) in [(true, false), (false, false), (true, true)] {
                let mut input = input(
                    cover,
                    None,
                    1.23,
                    pan,
                    canvas.center(),
                    -8.,
                    FullscreenWheelZoomStep::Percent10,
                );
                input.current = Some(PlaneViewPose {
                    anchor: input.anchor,
                    strength,
                    lens_center: Vec2::splat(0.5),
                });
                input.tilt_is_identity = identity;
                input.tilt_drag_owned = owned;
                let zoom_bits = input.zoom.to_bits();
                let pan_bits = [input.pan.x.to_bits(), input.pan.y.to_bits()];
                let out = plane_view_wheel_transition(input);
                assert_eq!(out.zoom.to_bits(), zoom_bits);
                assert_eq!([out.pan.x.to_bits(), out.pan.y.to_bits()], pan_bits);
                assert_eq!(out.pose.is_some(), !identity || owned);
                if let Some(pose) = out.pose {
                    assert_eq!(pose.strength, 0.);
                }
            }
        }
    }
}

#[test]
fn plane_revision3_zoom_max_uncoverable_geometry_still_stays_finite_and_bounded() {
    let canvas = canvas(Vec2::splat(1000.));
    let mut cover = super::revision_tests::fixture(canvas, Vec2::splat(1000.));
    cover.material.sampler_roi = Rect::from_min_max(pos2(0.499, 0.), pos2(0.501, 1.));
    let out = run(
        cover,
        1.,
        Vec2::ZERO,
        canvas.center(),
        &[100.],
        FullscreenWheelZoomStep::Percent100,
    );
    assert_eq!(out.zoom, crate::ui_fullscreen::ZOOM_MAX);
    assert!(out.pan.is_finite());
    assert_eq!(out.pose.unwrap().strength, 1.);
    let q = plane_effective_quad(
        &cover
            .resolve(out.zoom, out.pan, RectPixelFit::Texels)
            .unwrap(),
        cover.material,
    )
    .unwrap();
    assert_eq!(
        plane_fill_coefficient(canvas, canvas.center(), q, PlaneLensKind::Equidistant),
        0.
    );
}
