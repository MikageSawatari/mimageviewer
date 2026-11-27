//! Quaternion ownership inputs and the two-stage camera, without windows or GPU resources.
use super::*;
use egui::{pos2, vec2};

fn canvas() -> Rect {
    Rect::from_min_size(pos2(17., 23.), vec2(160., 90.))
}
fn axis(axis: usize, degrees: f32) -> PlaneTilt {
    let mut vector = [0.; 3];
    vector[axis] = degrees.to_radians();
    PlaneTilt::from_rotation_vector(vector)
}
fn close(a: f32, b: f32) {
    assert!((a - b).abs() < 0.0001, "{a} != {b}");
}
fn matrix_close(a: PlaneTilt, b: [[f32; 3]; 3]) {
    for (a, b) in a
        .matrix()
        .into_iter()
        .flatten()
        .zip(b.into_iter().flatten())
    {
        close(a, b);
    }
}
fn legacy_untilted(
    p: Pos2,
    canvas: Rect,
    center: Vec2,
    k: f32,
    lens: PlaneLensKind,
) -> Option<Pos2> {
    if k <= 1.0e-7 {
        return Some(p);
    }
    let radius = 0.5 * canvas.size().min_elem();
    let c = canvas.min + center * canvas.size();
    let u = (p - c) / radius;
    let r = u.length();
    let theta = lens.theta(k * r)?;
    let sigma = if r < 1.0e-6 { k } else { theta.sin() / r };
    let d = [sigma * u.x, sigma * u.y, theta.cos()];
    if d[2] <= 1.0e-6 {
        return None;
    }
    let t = 1. / d[2];
    Some(c + Vec2::new(t * d[0], t * d[1]) * (radius / k))
}

#[test]
fn plane_tilt_exact_identity_preserves_revision_camera_for_every_pixel_and_tiny_k() {
    let canvas = canvas();
    for lens in PlaneLensKind::ALL {
        for center in [vec2(0.5, 0.5), vec2(0.03, 0.04), Vec2::ZERO] {
            for k in [0., 1.0e-8, 1.0e-7, 1.00001e-7, 0.05, 0.2] {
                for y in 0..90 {
                    for x in 0..160 {
                        let p = canvas.min + vec2(x as f32 + 0.5, y as f32 + 0.5);
                        let expected = legacy_untilted(p, canvas, center, k, lens);
                        assert_eq!(
                            camera_point(p, canvas, center, k, lens, PlaneTilt::default()),
                            expected
                        );
                        assert_eq!(
                            camera_point(
                                p,
                                canvas,
                                center,
                                k,
                                lens,
                                PlaneTilt {
                                    q: [-1., 0., 0., 0.]
                                }
                            ),
                            expected
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn plane_tilt_zero_fisheye_matches_pinhole_homography_and_keeps_the_pivot() {
    let canvas = canvas();
    let c = canvas.center();
    let focal = plane_perspective_focal_length(canvas);
    let theta = 45_f32.to_radians();
    let (sin, cos) = theta.sin_cos();
    for v in [vec2(20., 10.), vec2(-30., 15.), vec2(50., -20.)] {
        let den = cos * focal + sin * v.x;
        let expected = c + vec2(focal * v.x / den, cos * focal * v.y / den);
        let actual = camera_point(
            c + v,
            canvas,
            Vec2::splat(0.5),
            0.,
            PlaneLensKind::Equidistant,
            axis(1, 45.),
        )
        .unwrap();
        assert!((actual - expected).length() < 0.001);
        assert!((actual - (c + v)).length() > 0.1);
    }
    for lens in PlaneLensKind::ALL {
        for k in [0., 0.2] {
            for (rotation, degrees) in [(0, 35.), (1, 45.), (1, 100.), (0, 180.), (1, 230.)] {
                let q = camera_point(
                    c,
                    canvas,
                    Vec2::splat(0.5),
                    k,
                    lens,
                    axis(rotation, degrees),
                )
                .unwrap();
                assert!((q - c).length() < 0.001);
            }
        }
    }
}

#[test]
fn plane_tilt_backfaces_flip_the_correct_axis_and_keep_negative_denominators() {
    let canvas = canvas();
    let c = canvas.center();
    for lens in PlaneLensKind::ALL {
        for k in [0., 0.2] {
            for v in [vec2(-40., -20.), vec2(20., 10.), vec2(35., -25.)] {
                let p = c + v;
                let fish = camera_point(p, canvas, Vec2::splat(0.5), k, lens, PlaneTilt::default())
                    .unwrap()
                    - c;
                for (tilt, expected) in [
                    (
                        PlaneTilt {
                            q: [0., 0., 1., 0.],
                        },
                        vec2(-fish.x, fish.y),
                    ),
                    (
                        PlaneTilt {
                            q: [0., 1., 0., 0.],
                        },
                        vec2(fish.x, -fish.y),
                    ),
                ] {
                    let actual = camera_point(p, canvas, Vec2::splat(0.5), k, lens, tilt).unwrap();
                    assert!((actual - (c + expected)).length() < 0.001);
                }
            }
        }
    }
}

#[test]
fn plane_tilt_edge_on_is_fully_masked_and_invalid_ray_coordinates_stay_finite() {
    let canvas = canvas();
    let c = canvas.center();
    for tilt in [axis(0, 90.), axis(0, 270.), axis(1, 90.), axis(1, 270.)] {
        for lens in PlaneLensKind::ALL {
            for k in [0., 0.2] {
                for y in 0..90 {
                    for x in 0..160 {
                        let p = canvas.min + vec2(x as f32 + 0.5, y as f32 + 0.5);
                        let (q, valid) = camera_sample(p, canvas, Vec2::splat(0.5), k, lens, tilt);
                        assert!(q.is_finite());
                        assert!(!valid);
                    }
                }
            }
        }
    }
    let focal = plane_perspective_focal_length(canvas);
    let behind = c - vec2(4. * focal, 0.);
    let (q, valid) = camera_sample(
        behind,
        canvas,
        Vec2::splat(0.5),
        0.,
        PlaneLensKind::Equidistant,
        axis(1, 80.),
    );
    assert!(q.is_finite());
    assert!(!valid, "t <= 0 must be masked");
    for lens in PlaneLensKind::ALL {
        let (q, valid) = camera_sample(
            canvas.right_bottom(),
            canvas,
            Vec2::ZERO,
            100.,
            lens,
            axis(1, 45.),
        );
        assert!(q.is_finite());
        assert!(!valid);
    }
}

#[test]
fn plane_tilt_near_edge_on_band_matches_forward_projection_and_converges() {
    let canvas = canvas();
    let c = canvas.center();
    let cover = super::revision_tests::fixture(canvas, vec2(800., 450.));
    let material = cover
        .resolve(
            2.,
            Vec2::ZERO,
            crate::displayed_image_transform::RectPixelFit::Proportional,
        )
        .unwrap();
    assert_eq!(
        material.paint_rect,
        Rect::from_center_size(c, vec2(320., 180.))
    );
    let a = f64::from(material.paint_rect.width()) * 0.5;
    let b = f64::from(material.paint_rect.height()) * 0.5;
    // Independent forward projection of the finite board, not the inverse camera under test:
    // camera position = (cos(theta) X, Y, f - sin(theta) X), |X| <= a, |Y| <= b.
    // Thus screen x = f cos(theta) X / (f - sin(theta) X). Its two X endpoints
    // give the exact horizontal band, including the near-side perspective expansion.
    // A fixed percentage of the canvas (the old 10% assertion) is not its width.
    let f = f64::from(canvas.width()).hypot(f64::from(canvas.height())) * 0.5
        / 25_f64.to_radians().tan();
    assert!(f > a, "the whole fixture stays in front of the pinhole");
    // The least projected half-height over all X and all these rotations is still larger
    // than the canvas half-height. Therefore each included column includes ALL 90 rows.
    assert!(f * b / (f + a) > f64::from(canvas.height()) * 0.5);
    let width_per_cos_bound = 2. * f * f * a / (f * f - a * a);
    for edge in [90_f32, 270.] {
        for side in [-1_f32, 1.] {
            let mut previous_width = f64::INFINITY;
            let mut previous_count = usize::MAX;
            for offset in [1_f32, 0.5, 0.1, 0.01] {
                let degrees = edge + side * offset;
                let (sin, cos) = f64::from(degrees).to_radians().sin_cos();
                let project_x = |x: f64| f * cos * x / (f - sin * x);
                let endpoints = [project_x(-a), project_x(a)];
                let left = endpoints[0].min(endpoints[1]);
                let right = endpoints[0].max(endpoints[1]);
                let width = right - left;
                let closed_width = 2. * f * f * a * cos.abs() / (f * f - a * a * sin * sin);
                assert!((width - closed_width).abs() < 1.0e-10);
                assert!(width > 0. && width < previous_width);
                // This analytic bound tends to zero with |cos(theta)| at 90/270 degrees.
                assert!(width <= width_per_cos_bound * cos.abs() * (1. + f64::EPSILON * 8.));
                previous_width = width;

                let in_band = |x: f32| {
                    let dx = f64::from(x) - f64::from(c.x);
                    dx >= left && dx <= right
                };
                let expected_columns = (0..160)
                    .filter(|x| in_band(canvas.min.x + *x as f32 + 0.5))
                    .count();
                let mut count = 0;
                for y in 0..90 {
                    for x in 0..160 {
                        let p = canvas.min + vec2(x as f32 + 0.5, y as f32 + 0.5);
                        let (q, valid) = camera_sample(
                            p,
                            canvas,
                            Vec2::splat(0.5),
                            0.,
                            PlaneLensKind::Equidistant,
                            axis(1, degrees),
                        );
                        assert!(q.is_finite());
                        let samples_material = valid
                            && texture_point(
                                q,
                                &material,
                                cover.material.coverage,
                                cover.material.sampler_roi,
                            )
                            .is_some();
                        assert_eq!(
                            samples_material,
                            in_band(p.x),
                            "forward-projected material band: degrees={degrees}, pixel=({x},{y}), band=({left},{right}), q={q:?}",
                        );
                        count += usize::from(samples_material);
                    }
                }
                assert_eq!(count, expected_columns * 90);
                assert!(count <= previous_count);
                previous_count = count;

                // Probe continuous points too: when no pixel center hits a very thin band,
                // it must still have an interior, with rejected points just outside each edge.
                for (x, expected) in [
                    ((left + right) * 0.5, true),
                    (left - 1., false),
                    (right + 1., false),
                ] {
                    let p = c + vec2(x as f32, 0.);
                    let (q, valid) = camera_sample(
                        p,
                        canvas,
                        Vec2::splat(0.5),
                        0.,
                        PlaneLensKind::Equidistant,
                        axis(1, degrees),
                    );
                    assert!(q.is_finite());
                    assert_eq!(
                        valid
                            && texture_point(
                                q,
                                &material,
                                cover.material.coverage,
                                cover.material.sampler_roi,
                            )
                            .is_some(),
                        expected,
                        "band probe at {degrees}: {x}"
                    );
                }
                if offset == 1. {
                    assert!(
                        count > 0,
                        "one-degree band must contain real material pixels"
                    );
                }
                if offset == 0.01 {
                    assert!(left > -0.5 && right < 0.5);
                    assert_eq!(
                        count, 0,
                        "band has converged between the two central pixel columns"
                    );
                }
            }
        }
    }
}

#[test]
fn plane_tilt_screen_axis_left_composition_direction_radius_and_unit_length() {
    let radius = 200.;
    let mut right = PlaneTilt::default();
    assert!(right.apply_drag(vec2(radius, 0.), radius));
    matrix_close(right, [[0., 0., 1.], [0., 1., 0.], [-1., 0., 0.]]);
    let mut down = PlaneTilt::default();
    down.apply_drag(vec2(0., radius), radius);
    matrix_close(down, [[1., 0., 0.], [0., 0., 1.], [0., -1., 0.]]);
    let mut right_down = right;
    right_down.apply_drag(vec2(0., radius), radius);
    matrix_close(right_down, [[0., -1., 0.], [0., 0., 1.], [-1., 0., 0.]]);
    let mut down_right = down;
    down_right.apply_drag(vec2(radius, 0.), radius);
    matrix_close(down_right, [[0., 0., 1.], [-1., 0., 0.], [0., -1., 0.]]);
    let mut back = PlaneTilt {
        q: [0., 0., 1., 0.],
    };
    back.apply_drag(vec2(radius * 0.1, 0.), radius);
    assert!(back.matrix()[0][2] < 0.);
    let mut many = PlaneTilt::default();
    for _ in 0..2000 {
        many.apply_drag(vec2(0.7, -0.3), radius);
    }
    close(many.q.iter().map(|v| v * v).sum::<f32>(), 1.);
}

#[test]
fn plane_tilt_straight_and_tiny_drag_partitions_do_not_snap_during_motion() {
    let radius = 200.;
    for delta in [
        vec2(radius, radius),
        vec2(-radius, -radius),
        vec2(radius, 0.),
    ] {
        let mut joined = PlaneTilt::default();
        joined.apply_drag(delta, radius);
        for pieces in [2, 25, 100] {
            let mut split = PlaneTilt::default();
            for _ in 0..pieces {
                split.apply_drag(delta / pieces as f32, radius);
            }
            matrix_close(split, joined.matrix());
        }
    }
    let tiny = vec2(radius * 0.004 / 90., 0.);
    let mut split = PlaneTilt::default();
    assert!(split.apply_drag(tiny, radius));
    assert!(split.is_identity());
    assert!(!split.is_exact_identity());
    for _ in 1..25 {
        split.apply_drag(tiny, radius);
    }
    assert!(!split.is_identity());
    let mut joined = PlaneTilt::default();
    joined.apply_drag(tiny * 25., radius);
    matrix_close(split, joined.matrix());
    let before = joined;
    assert!(!joined.apply_drag(Vec2::ZERO, radius));
    assert_eq!(joined, before);
    let mut crossing = PlaneTilt::default();
    crossing.apply_drag(vec2(30., 0.), radius);
    crossing.apply_drag(vec2(-30., 0.), radius);
    assert!(crossing.is_identity());
    crossing.apply_drag(vec2(-30., 0.), radius);
    assert!(!crossing.is_identity());
}

#[test]
fn plane_tilt_identity_tolerance_uses_xyz_and_only_release_snaps_to_default() {
    assert_eq!(PlaneTilt::default().q, [1., 0., 0., 0.]);
    for degrees in [0.004_f32, 0.0099, 0.0101, 0.02] {
        let tilt = axis(1, degrees);
        let negative = PlaneTilt {
            q: tilt.q.map(|v| -v),
        };
        assert_eq!(tilt.is_identity(), degrees < 0.01);
        assert_eq!(negative.is_identity(), tilt.is_identity());
        assert!(!tilt.is_exact_identity());
        let mut released = tilt;
        released.snap_identity();
        if degrees < 0.01 {
            assert_eq!(released, PlaneTilt::default());
        } else {
            assert_eq!(released, tilt);
        }
    }
    let tiny = axis(1, 0.004);
    let sample = canvas().center() + vec2(50., 20.);
    assert!(
        (camera_point(
            sample,
            canvas(),
            Vec2::splat(0.5),
            0.,
            PlaneLensKind::Equidistant,
            tiny
        )
        .unwrap()
            - sample)
            .length()
            > 0.00001,
        "a live sub-tolerance tilt must not take the exact-identity camera path"
    );
    let mut turn = axis(1, 360.);
    assert!(turn.is_identity());
    turn.snap_identity();
    assert_eq!(turn, PlaneTilt::default());
    let mut returned = PlaneTilt::default();
    returned.apply_drag(vec2(23., -17.), 200.);
    returned.apply_drag(vec2(-23., 17.), 200.);
    assert!(returned.is_identity());
    returned.snap_identity();
    assert_eq!(returned, PlaneTilt::default());
}

#[test]
fn plane_tilt_wheel_keeps_zero_strength_for_tilt_or_owned_drag_without_changing_geometry() {
    let canvas = canvas();
    let cover = super::revision_tests::fixture(canvas, vec2(800., 450.));
    let anchor = PlaneViewAnchor {
        fs_idx: 0,
        item: crate::item_identity::ItemId::NONE,
        items_generation: 1,
        slice: crate::page_split::PageSlice::Full,
    };
    let pose = PlaneViewPose {
        anchor,
        strength: 0.25,
        lens_center: Vec2::splat(0.5),
    };
    let mut baseline = None;
    for (identity, owned) in [(true, false), (false, false), (true, true), (false, true)] {
        let out = plane_view_wheel_transition(PlaneViewWheelInput {
            current: Some(pose),
            anchor,
            zoom: 2.,
            pan: Vec2::ZERO,
            pointer: None,
            canvas,
            notches: -2.,
            allowed: Ok(()),
            step: crate::settings::FullscreenWheelZoomStep::Percent10,
            cover: Some(cover),
            tilt_is_identity: identity,
            tilt_drag_owned: owned,
        });
        assert_eq!(out.zoom, 2.);
        assert_eq!(out.pan, Vec2::ZERO);
        assert_eq!(out.blocked, None);
        assert_eq!(out.pose.is_some(), !identity || owned);
        if let Some(pose) = out.pose {
            assert_eq!(pose.strength, 0.);
        }
        let geometry = (out.zoom, out.pan);
        if let Some(base) = baseline {
            assert_eq!(geometry, base);
        } else {
            baseline = Some(geometry);
        }
    }
    let drag = FsCanvasDrag::PlaneTilt {
        anchor,
        start: canvas.center(),
        previous: canvas.center(),
    };
    assert!(drag.is_tilt() && drag.is_plane_drag() && !drag.is_lens());
    assert_eq!(drag.plane_anchor(), Some(anchor));
    assert!(drag.pan_start().is_none() && drag.rotation_start().is_none());
    let mut ended = drag;
    ended.retire_plane_drag();
    assert_eq!(ended, FsCanvasDrag::default());
}

#[test]
fn plane_tilt_uniform_appends_three_columns_and_does_not_change_fill_or_output_mapping() {
    let canvas = canvas();
    let cover = super::revision_tests::fixture(canvas, vec2(800., 450.));
    let t = cover
        .resolve(
            2.,
            Vec2::ZERO,
            crate::displayed_image_transform::RectPixelFit::Proportional,
        )
        .unwrap();
    let tilt = axis(1, 45.);
    let columns = tilt.matrix();
    for mode in [PlaneFillMode::Auto, PlaneFillMode::Locked(0.2)] {
        let make = |tilt| {
            crate::plane_view_wgpu::PlaneRenderParams::screen_with_fill_mode(
                &t,
                canvas,
                Vec2::splat(0.5),
                0.5,
                PlaneLensKind::Equidistant,
                tilt,
                cover.material,
                &crate::ui_fullscreen::FsBgStyle::Solid(egui::Color32::WHITE),
                egui::Color32::BLACK,
                mode,
            )
        };
        let (flat, flat_fill) = make(PlaneTilt::default());
        let (params, fill) = make(tilt);
        assert_eq!(fill, flat_fill);
        assert_eq!(params.values[2][0], flat.values[2][0]);
        assert_eq!(params.values.len(), 16);
        assert_eq!(flat.values[2][2], 1.);
        assert_eq!(params.values[2][2], 0.);
        for i in 0..3 {
            assert_eq!(
                params.values[13 + i],
                [columns[i][0], columns[i][1], columns[i][2], 0.]
            );
        }
        let output = params.for_output([320, 180]);
        for i in [0, 1, 3, 4, 5, 6, 7, 9, 10, 11, 12, 13, 14, 15] {
            assert_eq!(output.values[i], params.values[i]);
        }
        assert_eq!(output.values[2][0..3], params.values[2][0..3]);
        assert_eq!(output.values[2][3], 1.);
    }
}

#[test]
fn plane_tilt_zero_strength_outward_segment_preserves_even_offscreen_pan_exactly() {
    let canvas = canvas();
    let cover = super::revision_tests::fixture(canvas, vec2(800., 450.));
    let anchor = PlaneViewAnchor {
        fs_idx: 0,
        item: crate::item_identity::ItemId::NONE,
        items_generation: 1,
        slice: crate::page_split::PageSlice::Full,
    };
    let pose = PlaneViewPose {
        anchor,
        strength: 0.,
        lens_center: Vec2::splat(0.5),
    };
    let pan = vec2(10000.23, -10000.17);
    for (identity, owned) in [(true, false), (false, false), (true, true), (false, true)] {
        let out = plane_view_wheel_transition(PlaneViewWheelInput {
            current: Some(pose),
            anchor,
            zoom: 1.23,
            pan,
            pointer: None,
            canvas,
            notches: -5.,
            allowed: Ok(()),
            step: crate::settings::FullscreenWheelZoomStep::Percent10,
            cover: Some(cover),
            tilt_is_identity: identity,
            tilt_drag_owned: owned,
        });
        assert_eq!(out.blocked, None);
        assert_eq!(out.zoom, 1.23);
        assert_eq!(out.pan, pan);
        assert_eq!(out.pose, (!identity || owned).then_some(pose));
    }
}
