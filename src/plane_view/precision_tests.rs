//! Precision premises of revision 1. Candidate limits are checked here, not assumed by the resolver.
use super::*;
use crate::displayed_image_transform::{
    DisplayedImageTransform, DisplayedImageTransformInput, FullscreenFitScaleLimits, RectPixelFit,
    ResolvedDisplayPlacement, physical_pixel_extent, quantize_points_to_physical_pixels,
};
use crate::fullscreen_specified_scale::{
    FullscreenFitBasis, FullscreenFitInput, LegacySourceLayoutBasis,
};
use crate::material_display_basis::{MaterialDisplayBasis, MaterialLegacyBasis};
use crate::plane_view_wgpu::PlaneMaterialDescription;
use crate::rotation_db::Rotation;
use crate::settings::FullscreenFitMode;

const CANDIDATE_COORDINATE_LIMIT: f32 = 2_097_152.;

fn cover_input(texture: Vec2, canvas: Vec2, ppp: f32) -> PlaneCoverInput {
    let source_size = [texture.x as usize, texture.y as usize];
    let unit = Rect::from_min_max(Pos2::ZERO, egui::pos2(1., 1.));
    PlaneCoverInput {
        transform: DisplayedImageTransformInput {
            page_idx: 0,
            viewport_rect: Rect::from_min_size(Pos2::ZERO, canvas),
            source_size: texture,
            texture_size: texture,
            rotation: Rotation::None,
            free_rotation_rad: 0.,
            content_bbox: None,
            fit_mode: FullscreenFitInput::legacy(FullscreenFitMode::Height),
            fit_scale_limits: FullscreenFitScaleLimits {
                pixels_per_point: ppp,
                ..Default::default()
            },
            pixels_per_point: ppp,
            placement: ResolvedDisplayPlacement::Normal { zoom_pan: None },
            pixel_fit: RectPixelFit::Texels,
        },
        layout_source_size: None,
        material: PlaneMaterialDescription {
            source_size,
            texture_size: texture,
            coverage: unit,
            sampler_roi: unit,
            display_basis: MaterialDisplayBasis::from_sampler(
                source_size,
                crate::source_coverage::SourceCoverage::FullSource,
            ),
            material_kind: crate::gpu_lanczos::FullscreenPageMaterialKind::Complete,
        },
    }
}

fn quad(transform: &DisplayedImageTransform, input: PlaneCoverInput) -> [Pos2; 4] {
    plane_effective_quad(transform, input.material).expect("fixture has nonempty material support")
}

/// Compare the actual Q edges, including source mapping, trim and rotation-center movement.
/// Both endpoints are projected onto the reference edge's unit normal using f64 only for
/// measurement; every resolver and source-map operation producing the endpoints remains f32.
fn normal_edge_error(reference: [Pos2; 4], actual: [Pos2; 4], ppp: f32) -> f64 {
    // screen() converts each Q endpoint to physical f32 coordinates before computing k.
    let reference = reference.map(|p| p * ppp);
    let actual = actual.map(|p| p * ppp);
    let mut error = 0_f64;
    for i in 0..4 {
        let next = (i + 1) % 4;
        let dx = f64::from(reference[next].x) - f64::from(reference[i].x);
        let dy = f64::from(reference[next].y) - f64::from(reference[i].y);
        let length = dx.hypot(dy);
        assert!(length > 0., "reference Q must not collapse");
        for endpoint in [i, next] {
            let ex = f64::from(actual[endpoint].x) - f64::from(reference[endpoint].x);
            let ey = f64::from(actual[endpoint].y) - f64::from(reference[endpoint].y);
            error = error.max(((dy * ex - dx * ey) / length).abs());
        }
    }
    error
}

fn resolve_pair(
    input: PlaneCoverInput,
    zoom: f32,
    pan: Vec2,
) -> (DisplayedImageTransform, DisplayedImageTransform) {
    let continuous = input
        .resolve(zoom, pan, RectPixelFit::Proportional)
        .unwrap();
    let texels = input.resolve(zoom, pan, RectPixelFit::Texels).unwrap();
    // This is the actual operation order in from_resolved_rect: first infer BOTH axis scales
    // from the f32 rectangle, then average, then multiply by ppp, then count physical pixels.
    // A proof that substitutes the original requested fit scale would miss R11's amplification.
    let display = match input.transform.rotation {
        Rotation::None | Rotation::Cw180 => input.transform.texture_size,
        Rotation::Cw90 | Rotation::Cw270 => egui::vec2(
            input.transform.texture_size.y,
            input.transform.texture_size.x,
        ),
    };
    let rect = continuous.full_image_rect;
    let reconstructed = (rect.width() / display.x + rect.height() / display.y) * 0.5;
    assert_eq!(continuous.total_scale, reconstructed);
    assert_eq!(texels.total_scale, reconstructed);
    let ppp = input.transform.pixels_per_point;
    let expected_min = egui::pos2(
        quantize_points_to_physical_pixels(rect.min.x, ppp),
        quantize_points_to_physical_pixels(rect.min.y, ppp),
    );
    let expected_size = egui::vec2(
        physical_pixel_extent(display.x, reconstructed * ppp) as f32 / ppp,
        physical_pixel_extent(display.y, reconstructed * ppp) as f32 / ppp,
    );
    assert_eq!(
        texels.full_image_rect,
        Rect::from_min_size(expected_min, expected_size)
    );
    (continuous, texels)
}

fn near_candidate_limit(input: PlaneCoverInput, zoom: f32, negative: bool) -> Vec2 {
    let transform = input
        .resolve(zoom, Vec2::ZERO, RectPixelFit::Proportional)
        .unwrap();
    let q = quad(&transform, input);
    let rect = transform.full_image_rect;
    let points = [
        q[0],
        q[1],
        q[2],
        q[3],
        rect.left_top(),
        rect.right_top(),
        rect.right_bottom(),
        rect.left_bottom(),
    ];
    let bounds = Rect::from_points(&points);
    let ppp = input.transform.pixels_per_point;
    let limit = (CANDIDATE_COORDINATE_LIMIT - 8.) / ppp;
    if negative {
        egui::vec2(-limit - bounds.min.x, -limit - bounds.min.y)
    } else {
        egui::vec2(limit - bounds.max.x, limit - bounds.max.y)
    }
}

fn assert_candidate_error(input: PlaneCoverInput, zoom: f32, negative: bool) {
    let pan = near_candidate_limit(input, zoom, negative);
    let (continuous, texels) = resolve_pair(input, zoom, pan);
    let reference = quad(&continuous, input);
    let actual = quad(&texels, input);
    let ppp = input.transform.pixels_per_point;
    let rect = continuous.full_image_rect;
    for p in reference.into_iter().chain([
        rect.left_top(),
        rect.right_top(),
        rect.right_bottom(),
        rect.left_bottom(),
    ]) {
        assert!(p.x.abs() * ppp <= CANDIDATE_COORDINATE_LIMIT);
        assert!(p.y.abs() * ppp <= CANDIDATE_COORDINATE_LIMIT);
    }
    let error = normal_edge_error(reference, actual, ppp);
    assert!(
        error <= f64::from(PLANE_COVER_MARGIN_PHYSICAL),
        "candidate needs revision: error={error} ppp={ppp} zoom={zoom} pan={pan:?} input={input:?}",
    );
}

#[test]
fn plane_candidate_precision_boundary_measures_total_normal_error() {
    // These are candidate limits awaiting execution and lead acceptance, not a completed proof.
    for ppp in [1., 1.25, 1.75] {
        for free_degrees in [0_f32, 17., 90.] {
            for saved in [Rotation::None, Rotation::Cw90] {
                for zoom in [1.01, 3.17, 50.] {
                    for negative in [false, true] {
                        for trimmed_partial in [false, true] {
                            let mut input = cover_input(
                                egui::vec2(4096., 128.),
                                egui::vec2(1600., 900.) / ppp,
                                ppp,
                            );
                            input.transform.rotation = saved;
                            input.transform.free_rotation_rad = free_degrees.to_radians();
                            if trimmed_partial {
                                input.transform.content_bbox = Some(Rect::from_min_max(
                                    egui::pos2(0.123, 0.2),
                                    egui::pos2(0.891, 0.9007),
                                ));
                                input.material.coverage = Rect::from_min_max(
                                    egui::pos2(0.07, 0.09),
                                    egui::pos2(0.93, 0.95),
                                );
                                input.material.sampler_roi = Rect::from_min_max(
                                    egui::pos2(0.05, 0.08),
                                    egui::pos2(0.96, 0.91),
                                );
                            }
                            assert_candidate_error(input, zoom, negative);
                        }
                    }
                }
            }
        }
    }
}

#[test]
fn plane_candidate_precision_uses_pdf_and_raw_source_layout_and_containment() {
    for (layout, source, sampler) in [
        (
            egui::vec2(595., 842.),
            egui::vec2(1413., 2000.),
            egui::vec2(997., 1410.),
        ),
        (
            egui::vec2(6000., 4000.),
            egui::vec2(6000., 4000.),
            egui::vec2(1537., 1024.),
        ),
    ] {
        for ppp in [1., 1.25, 1.75] {
            for degrees in [0_f32, 17., 90.] {
                for saved in [Rotation::None, Rotation::Cw90] {
                    let mut input = cover_input(sampler, egui::vec2(1600., 900.) / ppp, ppp);
                    let basis = LegacySourceLayoutBasis {
                        layout_size: layout,
                        reference_size: source,
                    };
                    input.transform.fit_mode = FullscreenFitInput::SourceLayout {
                        mode: FullscreenFitMode::Page,
                        basis,
                    };
                    input.transform.source_size = source;
                    input.transform.rotation = saved;
                    input.transform.free_rotation_rad = degrees.to_radians();
                    input.transform.content_bbox = Some(Rect::from_min_max(
                        egui::pos2(0.13, 0.17),
                        egui::pos2(0.91, 0.87),
                    ));
                    input.layout_source_size = Some(layout);
                    input.material.source_size = [source.x as usize, source.y as usize];
                    let legacy = MaterialLegacyBasis::SourceLayout { basis };
                    input.material.display_basis = MaterialDisplayBasis::new(
                        legacy,
                        legacy,
                        legacy,
                        Ok(FullscreenFitBasis::new(layout, source).unwrap()),
                    );
                    for zoom in [1.1, 50.] {
                        for negative in [false, true] {
                            assert_candidate_error(input, zoom, negative);
                        }
                    }
                }
            }
        }
    }
}

fn contains(quad: [Pos2; 4], point: Pos2, tolerance: f64) -> bool {
    let area: f64 = (0..4)
        .map(|i| {
            let a = quad[i];
            let b = quad[(i + 1) % 4];
            f64::from(a.x) * f64::from(b.y) - f64::from(a.y) * f64::from(b.x)
        })
        .sum();
    (0..4).all(|i| {
        let a = quad[i];
        let b = quad[(i + 1) % 4];
        let dx = f64::from(b.x) - f64::from(a.x);
        let dy = f64::from(b.y) - f64::from(a.y);
        let px = f64::from(point.x) - f64::from(a.x);
        let py = f64::from(point.y) - f64::from(a.y);
        (dx * py - dy * px) * area.signum() >= -tolerance * dx.hypot(dy)
    })
}

fn assert_outside_draw_semantics(input: PlaneCoverInput, zoom: f32, pan: Vec2, covers: bool) {
    let (_, actual) = resolve_pair(input, zoom, pan);
    let ppp = input.transform.pixels_per_point;
    let q = quad(&actual, input).map(|p| p * ppp);
    let logical_canvas = input.transform.viewport_rect;
    let canvas = logical_canvas * ppp;
    let corners = [
        canvas.left_top(),
        canvas.right_top(),
        canvas.right_bottom(),
        canvas.left_bottom(),
    ];
    assert_eq!(corners.into_iter().all(|p| contains(q, p, 0.)), covers);
    for lens in PlaneLensKind::ALL {
        let params = crate::plane_view_wgpu::PlaneRenderParams::screen(
            &actual,
            logical_canvas,
            Vec2::splat(0.5),
            1.,
            lens,
            PlaneTilt::default(),
            input.material,
            &crate::ui_fullscreen::FsBgStyle::Solid(egui::Color32::BLACK),
            egui::Color32::BLACK,
        );
        let k = params.values[2][0];
        assert!(k.is_finite());
        if !covers {
            assert_eq!(k, 0., "noncovering actual Q must retain the 2D picture");
        }
        // Sample every pixel center of a fixed small lattice in this actual large canvas.
        // In the noncovering branch the mapped source/absence must exactly equal ordinary 2D.
        for y in 0..90 {
            for x in 0..160 {
                let p = canvas.min
                    + canvas.size() * egui::vec2((x as f32 + 0.5) / 160., (y as f32 + 0.5) / 90.);
                let mapped =
                    camera_point(p, canvas, Vec2::splat(0.5), k, lens, PlaneTilt::default())
                        .expect("outside-scope camera remains finite");
                if covers {
                    assert!(
                        contains(q, mapped, 1. / 64.),
                        "ray left actual Q: {input:?}"
                    );
                    assert!(
                        texture_point(
                            mapped / ppp,
                            &actual,
                            input.material.coverage,
                            input.material.sampler_roi
                        )
                        .is_some()
                    );
                } else {
                    assert_eq!(mapped, p);
                    assert_eq!(
                        texture_point(
                            mapped / ppp,
                            &actual,
                            input.material.coverage,
                            input.material.sampler_roi
                        ),
                        texture_point(
                            p / ppp,
                            &actual,
                            input.material.coverage,
                            input.material.sampler_roi
                        ),
                    );
                }
            }
        }
    }
}

#[test]
fn plane_outside_precision_distinguishes_logical_shift_from_physical_q_error() {
    let input = cover_input(egui::vec2(4096., 1.), egui::vec2(1600., 900.), 1.75);
    let (continuous, actual) = resolve_pair(input, 49., Vec2::ZERO);
    let reference = quad(&continuous, input);
    let drawn = quad(&actual, input);
    // The earlier arithmetic stopped at the logical displacement (8 points * 1.75 = 14).
    // screen() subsequently multiplies BOTH Qs by ppp in f32. At this magnitude their
    // physical endpoints round to the same 16-pixel grid, so this is not a >4px Q error.
    assert_eq!(normal_edge_error(reference, drawn, 1.) * 1.75, 14.);
    assert_eq!(reference.map(|p| p * 1.75), drawn.map(|p| p * 1.75));
    assert_eq!(normal_edge_error(reference, drawn, 1.75), 0.);
    assert_outside_draw_semantics(input, 49., Vec2::ZERO, true);
    assert_outside_draw_semantics(
        input,
        49.,
        egui::vec2(actual.full_image_rect.width(), 0.),
        false,
    );
}

#[test]
fn plane_outside_precision_preserves_known_large_coordinate_counterexamples() {
    let input = cover_input(
        egui::vec2(8192., 3.),
        egui::vec2(1600., 6301.53857421875),
        1.3,
    );
    let (continuous, actual) = resolve_pair(input, 50., Vec2::ZERO);
    assert!(normal_edge_error(quad(&continuous, input), quad(&actual, input), 1.3) > 4.);
    assert_outside_draw_semantics(input, 50., Vec2::ZERO, true);
    assert_outside_draw_semantics(
        input,
        50.,
        egui::vec2(actual.full_image_rect.width(), 0.),
        false,
    );
}

#[test]
fn plane_outside_precision_scale_reconstruction_can_lose_actual_cover() {
    let mut input = cover_input(egui::vec2(8192., 1.), egui::vec2(1600., 900.), 1.);
    input.transform.free_rotation_rad = std::f32::consts::FRAC_PI_2;
    let pan = egui::vec2(-21_632., 67_108_864.);
    let (continuous, actual) = resolve_pair(input, 50., pan);
    // Keep the real f32 operation order visible: the unrotated short-axis endpoints
    // straddle 2^26, whose two sides have different spacings. Their difference is 44996,
    // not the requested 45000. Averaging this reconstructed scale with the long-axis
    // scale shrinks the quantized long edge by 16384 and shifts its rotation center.
    assert_eq!(continuous.full_image_rect.width() / 8192., 45_000.);
    assert_eq!(continuous.full_image_rect.height(), 44_996.);
    assert_eq!(continuous.total_scale, (45_000. + 44_996.) * 0.5);
    assert_eq!(continuous.total_scale, 44_998.);
    assert_eq!(
        continuous.full_image_rect.width() - actual.full_image_rect.width(),
        16_384.,
    );
    assert_eq!(
        continuous.full_image_rect.center().x - actual.full_image_rect.center().x,
        8_192.,
    );
    let reference = quad(&continuous, input);
    let drawn = quad(&actual, input);
    assert!(normal_edge_error(reference, drawn, 1.) > 4.);
    let target = input
        .transform
        .viewport_rect
        .expand(PLANE_COVER_MARGIN_PHYSICAL);
    for corner in [
        target.left_top(),
        target.right_top(),
        target.right_bottom(),
        target.left_bottom(),
    ] {
        assert!(
            contains(reference, corner, 0.),
            "continuous Q must cover canvas+"
        );
    }
    // Unlike a displacement-only example, this exercises the actual unsupported-range
    // outcome: the continuous cover is lost, and the screen uniform must select identity.
    assert_outside_draw_semantics(input, 50., pan, false);
}

#[test]
fn plane_outside_precision_large_pan_free_quarter_turn_uses_actual_q() {
    let mut input = cover_input(egui::vec2(8192., 1.), egui::vec2(1600., 900.), 1.75);
    input.transform.free_rotation_rad = std::f32::consts::FRAC_PI_2;
    // The review specified a large pan and free 90 degrees, not a required error constant.
    // Exercise both coverage outcomes without deriving an outside-scope precision margin.
    assert_outside_draw_semantics(input, 50., egui::vec2(10_000., 1_000_000.), true);
    assert_outside_draw_semantics(input, 50., egui::vec2(100_000., 1_000_000.), false);
}
