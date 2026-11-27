//! Revision 1's pure geometry and wheel-transition regressions. These never create a GPU or window.
use super::*;
use crate::displayed_image_transform::{
    DisplayedImageTransformInput, FullscreenFitScaleLimits, RectPixelFit, ResolvedDisplayPlacement,
    SingletonSpreadPlacement,
};
use crate::fullscreen_specified_scale::{FullscreenFitInput, LegacySourceLayoutBasis};
use crate::plane_view_wgpu::PlaneMaterialDescription;
use crate::rotation_db::Rotation;
use crate::settings::{FullscreenFitMode, FullscreenWheelZoomStep};
use egui::{pos2, vec2};

fn rect(size: Vec2) -> Rect {
    Rect::from_min_size(Pos2::ZERO, size)
}
fn unit() -> Rect {
    rect(Vec2::splat(1.))
}
fn corners(r: Rect) -> [Pos2; 4] {
    [
        r.left_top(),
        r.right_top(),
        r.right_bottom(),
        r.left_bottom(),
    ]
}
fn anchor() -> PlaneViewAnchor {
    PlaneViewAnchor {
        fs_idx: 0,
        item: crate::item_identity::ItemId::NONE,
        items_generation: 1,
        slice: crate::page_split::PageSlice::Full,
    }
}

pub(super) fn fixture(canvas: Rect, source: Vec2) -> PlaneCoverInput {
    PlaneCoverInput {
        transform: DisplayedImageTransformInput {
            page_idx: 0,
            viewport_rect: canvas,
            source_size: source,
            texture_size: source,
            rotation: Rotation::None,
            free_rotation_rad: 0.,
            content_bbox: None,
            fit_mode: FullscreenFitInput::Legacy(FullscreenFitMode::Page),
            fit_scale_limits: FullscreenFitScaleLimits::default(),
            pixels_per_point: 1.,
            placement: ResolvedDisplayPlacement::Normal { zoom_pan: None },
            pixel_fit: RectPixelFit::Texels,
        },
        layout_source_size: None,
        material: PlaneMaterialDescription {
            source_size: [source.x as usize, source.y as usize],
            texture_size: source,
            coverage: unit(),
            sampler_roi: unit(),
            display_basis: crate::material_display_basis::MaterialDisplayBasis::from_sampler(
                [source.x as usize, source.y as usize],
                crate::source_coverage::SourceCoverage::Unknown,
            ),
            material_kind: crate::gpu_lanczos::FullscreenPageMaterialKind::Complete,
        },
    }
}

fn transition(
    cover: Option<PlaneCoverInput>,
    current: Option<PlaneViewPose>,
    zoom: f32,
    pan: Vec2,
    center: Pos2,
    canvas: Rect,
    notches: f32,
) -> PlaneViewWheelOutcome {
    plane_view_wheel_transition(PlaneViewWheelInput {
        current,
        anchor: anchor(),
        zoom,
        pan,
        pointer: Some(center),
        canvas,
        notches,
        allowed: Ok(()),
        step: FullscreenWheelZoomStep::Percent10,
        tilt_is_identity: true,
        tilt_drag_owned: false,
        cover,
    })
}
fn wheel(
    cover: PlaneCoverInput,
    current: Option<PlaneViewPose>,
    zoom: f32,
    pan: Vec2,
    center: Pos2,
    notches: f32,
) -> PlaneViewWheelOutcome {
    transition(
        Some(cover),
        current,
        zoom,
        pan,
        center,
        cover.transform.viewport_rect,
        notches,
    )
}
fn close(a: f32, b: f32, tolerance: f32) {
    assert!(
        (a - b).abs() <= tolerance,
        "{a} != {b} (tolerance {tolerance})"
    );
}
fn quad(cover: PlaneCoverInput, zoom: f32, pan: Vec2, fit: RectPixelFit) -> [Pos2; 4] {
    plane_effective_quad(&cover.resolve(zoom, pan, fit).unwrap(), cover.material).unwrap()
}
fn signed_distance(quad: [Pos2; 4], p: Pos2) -> f64 {
    // Independent half-plane reference uses f64 from the original vertex coordinates.
    let cross = |a: Pos2, b: Pos2, c: Pos2| {
        (f64::from(b.x) - f64::from(a.x)) * (f64::from(c.y) - f64::from(a.y))
            - (f64::from(b.y) - f64::from(a.y)) * (f64::from(c.x) - f64::from(a.x))
    };
    let orientation = cross(quad[0], quad[1], quad[2]).signum();
    (0..4)
        .map(|i| {
            let a = quad[i];
            let b = quad[(i + 1) % 4];
            -orientation * cross(a, b, p)
                / (f64::from(b.x) - f64::from(a.x)).hypot(f64::from(b.y) - f64::from(a.y))
        })
        .fold(f64::NEG_INFINITY, f64::max)
}
fn assert_covers(quad: [Pos2; 4], target: Rect, tolerance: f64) {
    for p in corners(target) {
        assert!(
            signed_distance(quad, p) <= tolerance,
            "uncovered {p:?}: {quad:?}"
        );
    }
}
fn cap(canvas: Rect, center: Pos2, lens: PlaneLensKind) -> f64 {
    let far = corners(canvas)
        .into_iter()
        .map(|p| (f64::from(p.x) - f64::from(center.x)).hypot(f64::from(p.y) - f64::from(center.y)))
        .fold(0., f64::max);
    lens_g(lens, PLANE_THETA_CAP) * f64::from(canvas.size().min_elem()) * 0.5 / far
}
fn reference_point(p: Pos2, canvas: Rect, center: Pos2, k: f64, lens: PlaneLensKind) -> Pos2 {
    let dx = f64::from(p.x) - f64::from(center.x);
    let dy = f64::from(p.y) - f64::from(center.y);
    let r = dx.hypot(dy) / (0.5 * f64::from(canvas.size().min_elem()));
    if k == 0. || r == 0. {
        return p;
    }
    let theta = match lens {
        PlaneLensKind::Equidistant => k * r,
        PlaneLensKind::Stereographic => 2. * (k * r * 0.5).atan(),
        PlaneLensKind::EquisolidAngle => 2. * (k * r * 0.5).asin(),
    };
    let w = theta.tan() / (k * r);
    pos2(
        (f64::from(center.x) + dx * w) as f32,
        (f64::from(center.y) + dy * w) as f32,
    )
}
fn boundary(canvas: Rect, divisions: usize) -> Vec<Pos2> {
    let c = corners(canvas);
    (0..4)
        .flat_map(|j| {
            (0..=divisions)
                .map(move |i| c[j] + (c[(j + 1) % 4] - c[j]) * (i as f32 / divisions as f32))
        })
        .collect()
}
fn reference_max(
    canvas: Rect,
    center: Pos2,
    quad: [Pos2; 4],
    lens: PlaneLensKind,
    boundary: &[Pos2],
) -> f64 {
    if boundary
        .iter()
        .any(|p| signed_distance(quad, *p) > 0.000001)
    {
        return 0.;
    }
    let (mut lo, mut hi) = (0., cap(canvas, center, lens));
    for _ in 0..40 {
        let mid = (lo + hi) * 0.5;
        if boundary.iter().any(|p| {
            signed_distance(quad, reference_point(*p, canvas, center, mid, lens)) > 0.000001
        }) {
            hi = mid;
        } else {
            lo = mid;
        }
    }
    lo
}

#[test]
fn plane_revision_all_pixel_centers_and_dense_boundary_match_sixteen_inequalities() {
    for size in [vec2(160., 90.), vec2(90., 160.)] {
        let canvas = rect(size);
        let edge = boundary(canvas, 64);
        for source in [size * 5., vec2(600., 900.), vec2(1200., 400.)] {
            for rotation in [
                Rotation::None,
                Rotation::Cw90,
                Rotation::Cw180,
                Rotation::Cw270,
            ] {
                for degrees in [0_f32, 17.] {
                    for trimmed in [false, true] {
                        for partial in [false, true] {
                            let mut cover = fixture(canvas, source);
                            cover.transform.rotation = rotation;
                            cover.transform.free_rotation_rad = degrees.to_radians();
                            if trimmed {
                                cover.transform.content_bbox =
                                    Some(Rect::from_min_max(pos2(0.2, 0.1), pos2(0.72, 0.85)));
                            }
                            if partial {
                                cover.material.coverage =
                                    Rect::from_min_max(pos2(0.12, 0.06), pos2(0.88, 0.94));
                                cover.material.sampler_roi =
                                    Rect::from_min_max(pos2(0.08, 0.04), pos2(0.96, 0.91));
                            }
                            for normalized in [
                                vec2(0.5, 0.5),
                                vec2(0.05, 0.5),
                                vec2(0.03, 0.04),
                                Vec2::ZERO,
                            ] {
                                let center = canvas.min + normalized * size;
                                let (base_zoom, base_pan) =
                                    cover.cover(1., Vec2::ZERO, center).unwrap();
                                assert!(base_zoom < crate::ui_fullscreen::ZOOM_MAX);
                                for factor in [1., 1.1, 2., 5.] {
                                    let zoom = base_zoom * factor;
                                    let pan = cover
                                        .zoom_about_source(base_zoom, base_pan, zoom, center)
                                        .unwrap();
                                    let q = quad(cover, zoom, pan, RectPixelFit::Texels);
                                    assert_covers(q, canvas, 1. / 64.);
                                    for lens in PlaneLensKind::ALL {
                                        let k = plane_fill_coefficient(canvas, center, q, lens);
                                        assert!(k >= 0. && k.is_finite());
                                        let expected =
                                            reference_max(canvas, center, q, lens, &edge);
                                        assert!(
                                            (f64::from(k) - expected).abs()
                                                <= 0.001 * expected.max(0.001),
                                            "k {k}, reference {expected}, {lens:?}, q {q:?}"
                                        );
                                        if f64::from(k) < cap(canvas, center, lens) * 0.999
                                            && k > 0.00001
                                        {
                                            assert!(edge.iter().any(|p| signed_distance(
                                                q,
                                                reference_point(
                                                    *p,
                                                    canvas,
                                                    center,
                                                    f64::from(k) * 1.01,
                                                    lens
                                                )
                                            ) > 0.000001));
                                        }
                                        for y in 0..size.y as usize {
                                            for x in 0..size.x as usize {
                                                let p = pos2(x as f32 + 0.5, y as f32 + 0.5);
                                                let mapped = camera_point(
                                                    p,
                                                    canvas,
                                                    normalized,
                                                    k,
                                                    lens,
                                                    PlaneTilt::default(),
                                                )
                                                .unwrap();
                                                assert!(mapped.is_finite());
                                                assert!(
                                                    signed_distance(q, mapped) <= 1. / 64.,
                                                    "pixel {p:?} -> {mapped:?}, k {k}, {lens:?}"
                                                );
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

#[test]
fn plane_revision_zero_coefficient_and_center_on_support_edge_are_distinct() {
    let canvas = rect(vec2(100., 100.));
    for lens in PlaneLensKind::ALL {
        for (center, support) in [
            (
                canvas.center(),
                Rect::from_min_max(pos2(10., 0.), pos2(100., 100.)),
            ),
            (
                pos2(0., 50.),
                Rect::from_min_max(pos2(5., -100.), pos2(500., 200.)),
            ),
            (canvas.center(), canvas),
        ] {
            let q = corners(support);
            assert_eq!(plane_fill_coefficient(canvas, center, q, lens), 0.);
            assert!(boundary(canvas, 128).into_iter().any(|p| signed_distance(
                q,
                reference_point(p, canvas, center, 1.0e-3, lens)
            ) > 0.000001));
        }
        let q = corners(Rect::from_min_max(pos2(0., -100.), pos2(500., 200.)));
        let center = pos2(0., 50.);
        let k = plane_fill_coefficient(canvas, center, q, lens);
        assert!(k > 0.);
        for p in boundary(canvas, 128) {
            assert!(
                signed_distance(q, reference_point(p, canvas, center, f64::from(k), lens))
                    <= 1. / 64.
            );
        }
    }
}

#[test]
fn plane_revision_partial_material_support_intersects_paint_and_sampler_roi() {
    let canvas = rect(vec2(160., 90.));
    let mut cover = fixture(canvas, vec2(320., 90.));
    cover.material.coverage = Rect::from_min_max(pos2(0.25, 0.), pos2(0.75, 1.));
    let transform = cover
        .resolve(2., Vec2::ZERO, RectPixelFit::Proportional)
        .unwrap();
    let q = plane_effective_quad(&transform, cover.material).unwrap();
    close(
        Rect::from_points(&q).width(),
        transform.paint_rect.width() * 0.5,
        0.001,
    );
    assert_covers(q, canvas, 0.001);
    for lens in PlaneLensKind::ALL {
        assert_eq!(plane_fill_coefficient(canvas, canvas.center(), q, lens), 0.);
    }
    cover.material.sampler_roi = Rect::from_min_max(pos2(0.25, 0.1), pos2(0.75, 0.9));
    let smaller = plane_effective_quad(&transform, cover.material).unwrap();
    close(
        Rect::from_points(&smaller).width(),
        transform.paint_rect.width() * 0.25,
        0.001,
    );
    cover.material.coverage = Rect::from_min_max(pos2(2., 2.), pos2(3., 3.));
    assert!(plane_effective_quad(&transform, cover.material).is_none());
}

#[test]
fn plane_revision_fill_is_monotone_and_saturates_at_eighty_degrees() {
    let canvas = rect(vec2(160., 90.));
    for degrees in [0_f32, 17.] {
        let mut cover = fixture(canvas, vec2(600., 900.));
        cover.transform.free_rotation_rad = degrees.to_radians();
        let center = pos2(23., 31.);
        let (base_zoom, base_pan) = cover.cover(1., Vec2::ZERO, center).unwrap();
        for lens in PlaneLensKind::ALL {
            let mut previous = 0.;
            for factor in [1., 1.1, 1.46, 2.14, 3.1, 5., 10., 20.] {
                let zoom = base_zoom * factor;
                let pan = cover
                    .zoom_about_source(base_zoom, base_pan, zoom, center)
                    .unwrap();
                let q = quad(cover, zoom, pan, RectPixelFit::Proportional);
                let k = plane_fill_coefficient(canvas, center, q, lens);
                assert!(k + 0.00001 >= previous);
                assert!(f64::from(k) <= cap(canvas, center, lens) + 0.000001);
                if factor >= 10. {
                    close(k, cap(canvas, center, lens) as f32, 0.000001);
                }
                previous = k;
            }
            let far = corners(canvas)
                .into_iter()
                .map(|p| (p - center).length())
                .fold(0., f32::max);
            let theta = lens
                .theta(previous * far / (canvas.size().min_elem() * 0.5))
                .unwrap();
            assert!(theta.to_degrees() <= 80.00001);
        }
    }
}

#[test]
fn plane_revision_fill_is_scale_invariant_and_matches_documented_equidistant_angles() {
    let canvas = rect(vec2(160., 90.));
    let center = canvas.center();
    for (factor, degrees) in [(1.1, 29.7), (1.46, 54.0), (2.14, 68.7), (3.1, 76.4)] {
        let q = corners(Rect::from_center_size(center, canvas.size() * factor));
        let k = plane_fill_coefficient(canvas, center, q, PlaneLensKind::Equidistant);
        let far = (canvas.right_bottom() - center).length();
        close((k * far / 45.).to_degrees(), degrees, 0.15);
        for lens in PlaneLensKind::ALL {
            for density in [1., 1.25, 1.75, 2.] {
                close(
                    plane_fill_coefficient(
                        canvas * density,
                        center * density,
                        q.map(|p| p * density),
                        lens,
                    ),
                    plane_fill_coefficient(canvas, center, q, lens),
                    0.000001,
                );
            }
        }
    }
}

#[test]
fn plane_revision_portrait_first_notch_covers_then_applies_normal_zoom() {
    let canvas = rect(vec2(1600., 900.));
    let cover = fixture(canvas, vec2(600., 900.));
    let center = pos2(550., 450.);
    let out = wheel(cover, None, 1., Vec2::ZERO, center, 1.);
    close(out.zoom, (1608. / 600.) * 1.1, 0.00001);
    close(out.pose.unwrap().strength, 0.25, 0.000001);
    assert_covers(
        quad(cover, out.zoom, out.pan, RectPixelFit::Proportional),
        canvas.expand(4.),
        0.001,
    );
    for lens in PlaneLensKind::ALL {
        assert!(
            plane_fill_coefficient(
                canvas,
                center,
                quad(cover, out.zoom, out.pan, RectPixelFit::Texels),
                lens
            ) > 0.
        );
    }
}

#[test]
fn plane_revision_covered_zoom_keeps_source_pivot_and_pan_reentry_recovers_coverage() {
    let canvas = rect(vec2(1600., 900.));
    let cover = fixture(canvas, vec2(1600., 900.));
    let center = pos2(10., 450.);
    let before = cover.continuous(2., vec2(25., 30.)).unwrap();
    let source = before.screen_to_source_normalized(center);
    let out = wheel(cover, None, 2., vec2(25., 30.), center, 1.);
    close(out.zoom, 2.2, 0.000001);
    let after = cover.continuous(out.zoom, out.pan).unwrap();
    assert!((after.source_normalized_to_screen(source) - center).length() < 0.001);
    let panned = wheel(cover, out.pose, out.zoom, vec2(2000., 0.), center, 1.);
    assert_covers(
        quad(cover, panned.zoom, panned.pan, RectPixelFit::Texels),
        canvas,
        0.001,
    );
}

#[test]
fn plane_revision_singleton_gap_rotation_trim_and_dpi_use_canonical_resolver() {
    let canvas = rect(vec2(1600., 900.));
    for side in [
        SingletonSpreadPlacement::Left,
        SingletonSpreadPlacement::Right,
    ] {
        let mut cover = fixture(canvas, vec2(1600., 900.));
        cover.transform.pixels_per_point = 1.25;
        cover.transform.free_rotation_rad = 17_f32.to_radians();
        cover.transform.content_bbox = Some(Rect::from_min_max(pos2(0.1, 0.05), pos2(0.9, 0.95)));
        cover.transform.placement = ResolvedDisplayPlacement::SingletonSpread {
            side,
            gap: 37.,
            zoom_pan: None,
        };
        let center = pos2(10., 450.);
        let out = wheel(cover, None, 1.1, Vec2::ZERO, center, 1.);
        assert_covers(
            quad(cover, out.zoom, out.pan, RectPixelFit::Proportional),
            canvas.expand(4. / 1.25),
            0.002,
        );
        assert_covers(
            quad(cover, out.zoom, out.pan, RectPixelFit::Texels),
            canvas,
            0.002,
        );
        assert!(
            plane_fill_coefficient(
                canvas,
                center,
                quad(cover, out.zoom, out.pan, RectPixelFit::Texels),
                PlaneLensKind::Equidistant
            ) > 0.
        );
    }
}

#[test]
fn plane_revision_r8_narrow_trim_reaches_cover_below_zoom_max_without_iteration() {
    let canvas = rect(vec2(200., 200.));
    let mut cover = fixture(canvas, vec2(1000., 1000.));
    cover.transform.content_bbox = Some(Rect::from_min_max(pos2(0., 0.), pos2(0.028, 1.)));
    let out = wheel(cover, None, 1., Vec2::ZERO, canvas.center(), 1.);
    close(out.zoom, (208. / 5.6) * 1.1, 0.001);
    assert!(out.zoom < crate::ui_fullscreen::ZOOM_MAX);
    assert_covers(
        quad(cover, out.zoom, out.pan, RectPixelFit::Texels),
        canvas,
        0.002,
    );
}

#[test]
fn plane_revision_source_layout_pdf_raw_q_uses_shared_contain_path() {
    let canvas = rect(vec2(900., 700.));
    for (source, sampler, layout) in [
        (vec2(15000., 20000.), vec2(613., 819.), vec2(612., 792.)),
        (vec2(6000., 4000.), vec2(1001., 667.), vec2(6016., 4016.)),
    ] {
        let mut cover = fixture(canvas, source);
        cover.transform.texture_size = sampler;
        cover.material.texture_size = sampler;
        cover.transform.pixels_per_point = 1.25;
        cover.transform.fit_mode = FullscreenFitInput::SourceLayout {
            mode: FullscreenFitMode::Page,
            basis: LegacySourceLayoutBasis {
                layout_size: layout,
                reference_size: source,
            },
        };
        cover.layout_source_size = Some(layout);
        cover.transform.content_bbox = Some(Rect::from_min_max(pos2(0.13, 0.08), pos2(0.89, 0.91)));
        let out = wheel(cover, None, 1., Vec2::ZERO, canvas.center(), 1.);
        let continuous = quad(cover, out.zoom, out.pan, RectPixelFit::Proportional);
        let drawn = quad(cover, out.zoom, out.pan, RectPixelFit::Texels);
        assert_covers(drawn, canvas, 0.001);
        for i in 0..4 {
            let edge = (continuous[(i + 1) % 4] - continuous[i]).normalized();
            let normal = vec2(edge.y, -edge.x);
            for j in [i, (i + 1) % 4] {
                assert!(
                    (drawn[j] - continuous[j]).dot(normal).abs() * 1.25
                        <= PLANE_COVER_MARGIN_PHYSICAL
                );
            }
        }
    }
}

fn partition(
    cover: PlaneCoverInput,
    zoom: f32,
    pan: Vec2,
    center: Pos2,
    notches: f32,
    pieces: usize,
    step: FullscreenWheelZoomStep,
) -> PlaneViewWheelOutcome {
    let segment = |current, zoom, pan| {
        plane_view_wheel_transition(PlaneViewWheelInput {
            current,
            anchor: anchor(),
            zoom,
            pan,
            pointer: Some(center),
            canvas: cover.transform.viewport_rect,
            notches: notches / pieces as f32,
            allowed: Ok(()),
            step,
            tilt_is_identity: true,
            tilt_drag_owned: false,
            cover: Some(cover),
        })
    };
    let mut out = segment(None, zoom, pan);
    for _ in 1..pieces {
        out = segment(out.pose, out.zoom, out.pan);
    }
    out
}
fn assert_same(
    cover: PlaneCoverInput,
    center: Pos2,
    a: PlaneViewWheelOutcome,
    b: PlaneViewWheelOutcome,
) {
    close(a.zoom, b.zoom, 0.0001);
    close(a.pose.unwrap().strength, b.pose.unwrap().strength, 0.00001);
    assert!(
        (a.pan - b.pan).length() * cover.transform.pixels_per_point <= PLANE_COVER_MARGIN_PHYSICAL
    );
    let qa = quad(cover, a.zoom, a.pan, RectPixelFit::Texels);
    let qb = quad(cover, b.zoom, b.pan, RectPixelFit::Texels);
    for i in 0..4 {
        assert!(
            (qa[i] - qb[i]).length() * cover.transform.pixels_per_point
                <= PLANE_COVER_MARGIN_PHYSICAL
        );
    }
    for lens in PlaneLensKind::ALL {
        let ka = plane_fill_coefficient(cover.transform.viewport_rect, center, qa, lens);
        let kb = plane_fill_coefficient(cover.transform.viewport_rect, center, qb, lens);
        close(ka, kb, 0.001);
        assert!(ka > 0. && kb > 0.);
    }
}

#[test]
fn plane_revision_r7_and_r8_partitioned_notches_preserve_pan_strength_and_fill() {
    for (canvas, source, zoom, pan, center) in [
        (
            rect(vec2(1600., 900.)),
            vec2(600., 900.),
            1.,
            Vec2::ZERO,
            pos2(550., 450.),
        ),
        (
            rect(vec2(900., 900.)),
            vec2(900., 900.),
            1.02,
            vec2(0.23, 0.),
            pos2(855., 450.),
        ),
    ] {
        let cover = fixture(canvas, source);
        for step in [
            FullscreenWheelZoomStep::Standard,
            FullscreenWheelZoomStep::Percent10,
        ] {
            let joined = partition(cover, zoom, pan, center, 2., 1, step);
            assert_same(
                cover,
                center,
                joined,
                partition(cover, zoom, pan, center, 2., 2, step),
            );
            assert_same(
                cover,
                center,
                joined,
                partition(cover, zoom, pan, center, 2., 20, step),
            );
        }
    }
}

#[test]
fn plane_revision_resize_recomputes_cover_from_current_canvas() {
    let old = fixture(rect(vec2(1600., 900.)), vec2(1600., 900.));
    let first = wheel(
        old,
        None,
        1.,
        Vec2::ZERO,
        old.transform.viewport_rect.center(),
        1.,
    );
    let mut current = old;
    current.transform.viewport_rect = rect(vec2(900., 1600.));
    let next = wheel(
        current,
        first.pose,
        first.zoom,
        first.pan,
        current.transform.viewport_rect.center(),
        1.,
    );
    assert!(next.zoom > first.zoom * 1.1);
    assert_covers(
        quad(current, next.zoom, next.pan, RectPixelFit::Texels),
        current.transform.viewport_rect,
        0.001,
    );
}

#[test]
fn plane_revision_r9_resize_keeps_pure_result_and_terminal_overlap_uses_same_input() {
    let canvas = rect(vec2(1600., 900.));
    let cover = fixture(canvas, vec2(1600., 900.));
    let center = pos2(0., 450.);
    let pose = PlaneViewPose {
        anchor: anchor(),
        strength: 0.5,
        lens_center: vec2(0., 0.5),
    };
    let out = wheel(cover, Some(pose), 4., vec2(2202., 0.), center, 1.);
    close(out.zoom, 4.4, 0.000001);
    close(out.pan.x, 2502.2, 0.002);
    let leaving = wheel(
        cover,
        out.pose,
        out.zoom,
        vec2(100000., -100000.),
        center,
        -1.,
    );
    let transformed = cover.continuous(leaving.zoom, leaving.pan).unwrap();
    let painted_bounds = Rect::from_points(&transformed.paint_quad().positions);
    assert_eq!(
        crate::ui_fullscreen::pan_correction_for_minimum_overlap(painted_bounds, canvas),
        Vec2::ZERO
    );
    let overlap = painted_bounds.intersect(canvas);
    assert!(overlap.width() >= 47.99 && overlap.height() >= 47.99);
}

#[test]
fn plane_revision_terminal_overlap_uses_unclipped_paint_for_each_offscreen_direction() {
    let canvas = rect(vec2(1600., 900.));
    let center = canvas.center();
    for rotation in [Rotation::None, Rotation::Cw90] {
        for degrees in [0_f32, 17.] {
            let mut cover = fixture(canvas, vec2(1600., 900.));
            cover.transform.rotation = rotation;
            cover.transform.free_rotation_rad = degrees.to_radians();
            cover.transform.content_bbox = Some(Rect::from_min_max(pos2(0.2, 0.1), pos2(0.8, 0.9)));
            let pose = PlaneViewPose {
                anchor: anchor(),
                strength: 0.5,
                lens_center: Vec2::splat(0.5),
            };
            for pan in [
                vec2(10000., 0.),
                vec2(-10000., 0.),
                vec2(0., 10000.),
                vec2(0., -10000.),
                vec2(10000., -10000.),
            ] {
                let before = cover.continuous(4., pan).unwrap();
                assert!(!before.hit_rect.is_positive(), "fixture must be offscreen");
                let out = wheel(cover, Some(pose), 4., pan, center, -1.);
                let after = cover.continuous(out.zoom, out.pan).unwrap();
                let painted_bounds = Rect::from_points(&after.paint_quad().positions);
                let overlap = painted_bounds.intersect(canvas);
                assert!(
                    overlap.width() >= 47.99 && overlap.height() >= 47.99,
                    "pan {pan:?}, rotation {rotation:?}, degrees {degrees}: {overlap:?}"
                );
            }
        }
    }
}

#[test]
fn plane_revision_zoom_max_uncovered_support_is_identity_but_stays_plane_view() {
    let canvas = rect(vec2(200., 200.));
    let mut cover = fixture(canvas, vec2(1000., 1000.));
    cover.material.coverage = Rect::from_min_max(pos2(0.4999, 0.), pos2(0.5001, 1.));
    let out = wheel(cover, None, 1., Vec2::ZERO, canvas.center(), 1.);
    assert_eq!(out.zoom, crate::ui_fullscreen::ZOOM_MAX);
    assert!(out.pose.is_some());
    let q = quad(cover, out.zoom, out.pan, RectPixelFit::Texels);
    for lens in PlaneLensKind::ALL {
        let k = plane_fill_coefficient(canvas, canvas.center(), q, lens);
        assert_eq!(k, 0.);
        for p in boundary(canvas, 32) {
            assert_eq!(
                camera_point(p, canvas, vec2(0.5, 0.5), k, lens, PlaneTilt::default()),
                Some(p)
            );
        }
    }
}

#[test]
fn plane_revision_four_notches_saturate_and_exit_spends_only_remaining_strength() {
    let canvas = rect(vec2(800., 600.));
    let cover = fixture(canvas, vec2(800., 600.));
    let first = wheel(cover, None, 2., Vec2::ZERO, canvas.center(), 4.);
    assert_eq!(first.pose.unwrap().strength, 1.);
    let more = wheel(
        cover,
        first.pose,
        first.zoom,
        first.pan,
        canvas.center(),
        2.,
    );
    assert_eq!(more.pose.unwrap().strength, 1.);
    assert!(more.zoom > first.zoom);
    let tiny = wheel(cover, None, 2., Vec2::ZERO, canvas.center(), 0.5);
    let huge_exit = wheel(cover, tiny.pose, tiny.zoom, tiny.pan, canvas.center(), -10.);
    let exact_exit = wheel(cover, tiny.pose, tiny.zoom, tiny.pan, canvas.center(), -0.5);
    assert_eq!(huge_exit, exact_exit);
    assert!(huge_exit.pose.is_none());
    close(huge_exit.zoom, 2., 0.000001);
}

#[test]
fn plane_revision_missing_drawn_material_skips_input_without_entering_or_notifying() {
    let canvas = rect(vec2(800., 600.));
    let pose = PlaneViewPose {
        anchor: anchor(),
        strength: 0.25,
        lens_center: vec2(0.5, 0.5),
    };
    for current in [None, Some(pose)] {
        for notches in [-1., 1.] {
            let out = transition(
                None,
                current,
                2.,
                vec2(5., 7.),
                canvas.center(),
                canvas,
                notches,
            );
            assert_eq!(out.pose, current);
            assert_eq!(out.zoom, 2.);
            assert_eq!(out.pan, vec2(5., 7.));
            assert!(out.blocked.is_none());
            assert!(!out.accepted);
        }
    }
}

#[test]
fn plane_revision_camera_identity_center_domain_and_horizon_remain_finite() {
    let canvas = rect(vec2(160., 90.));
    let center = canvas.center();
    for lens in PlaneLensKind::ALL {
        for k in [0., 0.00001, 0.2] {
            assert_eq!(
                camera_point(
                    center,
                    canvas,
                    Vec2::splat(0.5),
                    k,
                    lens,
                    PlaneTilt::default()
                ),
                Some(center)
            );
            let p = center + vec2(20., 10.);
            let mapped =
                camera_point(p, canvas, Vec2::splat(0.5), k, lens, PlaneTilt::default()).unwrap();
            assert!(mapped.is_finite());
            if k <= 0.00001 {
                assert!((mapped - p).length() < 0.001);
            }
        }
        let horizon = lens.g(std::f32::consts::FRAC_PI_2);
        assert!(
            camera_point(
                center + vec2(45.1, 0.),
                canvas,
                Vec2::splat(0.5),
                horizon,
                lens,
                PlaneTilt::default()
            )
            .is_none()
        );
        assert!(lens.theta(f32::INFINITY).is_none());
    }
    assert!(
        PlaneLensKind::Equidistant
            .theta(std::f32::consts::PI + 0.001)
            .is_none()
    );
    assert!(PlaneLensKind::EquisolidAngle.theta(2.001).is_none());
}

#[test]
fn plane_revision_locked_fill_keeps_fixed_value_when_support_leaves_canvas() {
    let canvas = rect(vec2(160., 90.));
    let center = canvas.center();
    let covering = corners(canvas.expand(100.));
    let exposed = corners(Rect::from_min_size(pos2(200., 200.), vec2(20., 30.)));
    for lens in PlaneLensKind::ALL {
        let fixed = plane_fill_for_mode(canvas, center, Some(covering), lens, PlaneFillMode::Auto);
        assert!(fixed > 0.);
        assert_eq!(
            plane_fill_for_mode(canvas, center, Some(exposed), lens, PlaneFillMode::Auto),
            0.
        );
        for support in [Some(covering), Some(exposed), None] {
            assert_eq!(
                plane_fill_for_mode(canvas, center, support, lens, PlaneFillMode::Locked(fixed)),
                fixed
            );
            assert_eq!(
                plane_fill_for_mode(canvas, center, support, lens, PlaneFillMode::Locked(0.)),
                0.
            );
        }
    }
}

#[test]
fn plane_revision_locked_fill_shares_auto_cap_after_center_move_and_resize() {
    for size in [vec2(160., 90.), vec2(90., 160.), vec2(400., 150.)] {
        let canvas = Rect::from_min_size(pos2(17., 23.), size);
        for normalized in [
            Vec2::splat(0.5),
            vec2(0., 0.5),
            vec2(0.03, 0.04),
            Vec2::ZERO,
        ] {
            let center = canvas.min + normalized * size;
            for lens in PlaneLensKind::ALL {
                let cap = plane_cap_coefficient(canvas, center, lens);
                let huge_support = corners(canvas.expand(100000.));
                assert_eq!(
                    plane_fill_coefficient(canvas, center, huge_support, lens),
                    cap
                );
                for fixed in [0., cap * 0.5, cap, cap * 2.] {
                    let fill = plane_fill_for_mode(
                        canvas,
                        center,
                        None,
                        lens,
                        PlaneFillMode::Locked(fixed),
                    );
                    assert_eq!(fill, fixed.min(cap));
                    for p in corners(canvas) {
                        let theta = lens
                            .theta(fill * (p - center).length() / (0.5 * size.min_elem()))
                            .unwrap();
                        assert!(theta.to_degrees() <= 80.00001);
                        assert!(
                            camera_point(p, canvas, normalized, fill, lens, PlaneTilt::default())
                                .unwrap()
                                .is_finite()
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn plane_revision_screen_records_fill_before_strength_even_when_strength_is_zero() {
    let canvas = rect(vec2(160., 90.));
    for density in [1., 1.25, 2.] {
        let mut cover = fixture(canvas, vec2(800., 450.));
        cover.transform.pixels_per_point = density;
        let shrunk = cover
            .resolve(0.5, Vec2::ZERO, RectPixelFit::Proportional)
            .unwrap();
        for lens in PlaneLensKind::ALL {
            let fixed =
                plane_cap_coefficient(canvas * density, canvas.center() * density, lens) * 0.5;
            for strength in [0., 0.25, 0.75] {
                let (params, fill) =
                    crate::plane_view_wgpu::PlaneRenderParams::screen_with_fill_mode(
                        &shrunk,
                        canvas,
                        Vec2::splat(0.5),
                        strength,
                        lens,
                        PlaneTilt::default(),
                        cover.material,
                        &crate::ui_fullscreen::FsBgStyle::Solid(egui::Color32::WHITE),
                        egui::Color32::BLACK,
                        PlaneFillMode::Locked(fixed),
                    );
                assert_eq!(fill, fixed);
                assert_eq!(params.values[2][0], strength * fixed);
            }
        }
    }
}

#[test]
fn plane_revision_new_state_uses_auto_and_accepted_is_independent_of_geometry_change() {
    let canvas = rect(vec2(800., 600.));
    let cover = fixture(canvas, canvas.size());
    let pose = PlaneViewPose {
        anchor: anchor(),
        strength: 1.,
        lens_center: Vec2::splat(0.5),
    };
    assert_eq!(PlaneViewState::new(pose).fill_mode, PlaneFillMode::Auto);
    let saturated = wheel(
        cover,
        Some(pose),
        crate::ui_fullscreen::ZOOM_MAX,
        Vec2::ZERO,
        canvas.center(),
        1.,
    );
    assert!(saturated.accepted);
    assert!(!saturated.changes_view(Some(pose), crate::ui_fullscreen::ZOOM_MAX, Vec2::ZERO));
    for notches in [0., f32::NAN, f32::INFINITY] {
        assert!(!wheel(cover, Some(pose), 2., Vec2::ZERO, canvas.center(), notches).accepted);
    }
    assert!(!wheel(cover, None, 2., Vec2::ZERO, canvas.center(), -1.).accepted);
    let rejected = plane_view_wheel_transition(PlaneViewWheelInput {
        current: None,
        anchor: anchor(),
        zoom: 2.,
        pan: Vec2::ZERO,
        pointer: Some(canvas.center()),
        canvas,
        notches: 1.,
        allowed: Err(PlaneViewEntryBlock::Editing),
        tilt_is_identity: true,
        tilt_drag_owned: false,
        step: FullscreenWheelZoomStep::Percent10,
        cover: Some(cover),
    });
    assert!(!rejected.accepted);
    assert_eq!(rejected.blocked, Some(PlaneViewEntryBlock::Editing));
    let leaving = wheel(cover, Some(pose), 2., Vec2::ZERO, canvas.center(), -1.);
    assert!(leaving.accepted);
    assert_eq!(leaving.pose.unwrap().strength, 0.75);
}
