//! PlaneView's page identity, input transition and CPU reference camera.
use std::sync::Arc;

use egui::{Pos2, Rect, Vec2};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum PlaneLensKind {
    #[default]
    Equidistant,
    Stereographic,
    EquisolidAngle,
}

impl PlaneLensKind {
    pub const ALL: [Self; 3] = [Self::Equidistant, Self::Stereographic, Self::EquisolidAngle];
    pub fn label(self) -> &'static str {
        match self {
            Self::Equidistant => "等距離",
            Self::Stereographic => "立体射影",
            Self::EquisolidAngle => "等立体角",
        }
    }
    pub(crate) fn shader_id(self) -> f32 {
        match self {
            Self::Equidistant => 0.,
            Self::Stereographic => 1.,
            Self::EquisolidAngle => 2.,
        }
    }
    #[cfg(test)]
    pub(crate) fn g(self, theta: f32) -> f32 {
        match self {
            Self::Equidistant => theta,
            Self::Stereographic => 2. * (theta * 0.5).tan(),
            Self::EquisolidAngle => 2. * (theta * 0.5).sin(),
        }
    }
    #[cfg(test)]
    pub(crate) fn theta(self, radius: f32) -> Option<f32> {
        if !radius.is_finite() || radius < 0. {
            return None;
        }
        let theta = match self {
            Self::Equidistant => radius,
            Self::Stereographic => 2. * (radius * 0.5).atan(),
            Self::EquisolidAngle if radius <= 2. => 2. * (radius * 0.5).asin(),
            Self::EquisolidAngle => return None,
        };
        (theta < std::f32::consts::PI
            || (self == Self::EquisolidAngle && theta == std::f32::consts::PI))
            .then_some(theta)
    }
}

/// Effective paint support, using exactly the source/screen maps used by the shader.
/// The material crop and the displayed trim/slice are both source-space rectangles.
pub(crate) fn plane_effective_quad(
    transform: &crate::displayed_image_transform::DisplayedImageTransform,
    material: crate::plane_view_wgpu::PlaneMaterialDescription,
) -> Option<[Pos2; 4]> {
    let source_paint = Rect::from_points(
        &transform
            .paint_quad()
            .positions
            .map(|p| transform.screen_to_source_normalized(p)),
    );
    let material_source = Rect::from_min_max(
        material.coverage.min + material.coverage.size() * material.sampler_roi.min.to_vec2(),
        material.coverage.min + material.coverage.size() * material.sampler_roi.max.to_vec2(),
    );
    let support = source_paint
        .intersect(material.coverage)
        .intersect(material_source);
    (support.is_finite() && support.is_positive()).then(|| {
        [
            support.left_top(),
            support.right_top(),
            support.right_bottom(),
            support.left_bottom(),
        ]
        .map(|p| transform.source_normalized_to_screen(p))
    })
}

pub(crate) const PLANE_COVER_MARGIN_PHYSICAL: f32 = 4.;
const PLANE_THETA_CAP: f64 = 80. * std::f64::consts::PI / 180.;

fn lens_g(lens: PlaneLensKind, theta: f64) -> f64 {
    match lens {
        PlaneLensKind::Equidistant => theta,
        PlaneLensKind::Stereographic => 2. * (theta * 0.5).tan(),
        PlaneLensKind::EquisolidAngle => 2. * (theta * 0.5).sin(),
    }
}

/// Largest untilted coefficient allowed by the sixteen corner/edge inequalities and 80° cap.
/// Canvas, center and quad must all use the same units (screen callers use physical pixels).
pub(crate) fn plane_fill_coefficient(
    canvas: Rect,
    center: Pos2,
    quad: [Pos2; 4],
    lens: PlaneLensKind,
) -> f32 {
    if !canvas.is_positive()
        || !canvas.is_finite()
        || !center.is_finite()
        || quad.iter().any(|p| !p.is_finite())
    {
        return 0.;
    }
    let corners = [
        canvas.left_top(),
        canvas.right_top(),
        canvas.right_bottom(),
        canvas.left_bottom(),
    ];
    let radius = 0.5 * f64::from(canvas.size().min_elem());
    let delta = |p: Pos2| {
        [
            f64::from(p.x) - f64::from(center.x),
            f64::from(p.y) - f64::from(center.y),
        ]
    };
    let far = corners
        .iter()
        .map(|p| {
            let v = delta(*p);
            v[0].hypot(v[1])
        })
        .fold(0., f64::max);
    if far == 0. {
        return 0.;
    }
    let mut limit = lens_g(lens, PLANE_THETA_CAP) * radius / far;
    let area: f64 = (0..4)
        .map(|i| {
            let a = delta(quad[i]);
            let b = delta(quad[(i + 1) % 4]);
            a[0] * b[1] - a[1] * b[0]
        })
        .sum();
    if area == 0. {
        return 0.;
    }
    for j in 0..4 {
        let a = delta(quad[j]);
        let b = delta(quad[(j + 1) % 4]);
        let n = [(b[1] - a[1]) * area.signum(), (a[0] - b[0]) * area.signum()];
        let distance = n[0] * a[0] + n[1] * a[1];
        if distance < 0. {
            return 0.;
        }
        for p in corners {
            let v = delta(p);
            let rho = v[0].hypot(v[1]);
            let projection = n[0] * v[0] + n[1] * v[1];
            if rho == 0. || projection <= 0. {
                continue;
            }
            let m = distance / projection;
            if m <= 1. {
                return 0.;
            }
            let g_star = match lens {
                PlaneLensKind::Equidistant => {
                    let (mut low, mut high) = (0., PLANE_THETA_CAP);
                    for _ in 0..56 {
                        let theta = (low + high) * 0.5;
                        if theta.tan() / theta > m {
                            high = theta;
                        } else {
                            low = theta;
                        }
                    }
                    low
                }
                PlaneLensKind::Stereographic => 2. * (1. - 1. / m).max(0.).sqrt(),
                PlaneLensKind::EquisolidAngle => {
                    // Rationalized smaller root avoids cancellation for m close to one.
                    let squared = m * m;
                    2. * (2. * (squared - 1.) / (4. * squared - 1. + (8. * squared + 1.).sqrt()))
                        .max(0.)
                        .sqrt()
                }
            };
            limit = limit.min(g_star * radius / rho);
        }
    }
    // Round toward the safe side, including the f64 -> shader-f32 conversion.
    (limit as f32) * (1. - f32::EPSILON)
}

/// Frozen material plus this pass's canonical single-page geometry. Prediction and application
/// carry this value together; neither consults a subsequently rebuilt page layout.
#[derive(Clone, Copy, Debug)]
pub(crate) struct PlaneCoverInput {
    pub transform: crate::displayed_image_transform::DisplayedImageTransformInput,
    pub layout_source_size: Option<Vec2>,
    pub material: crate::plane_view_wgpu::PlaneMaterialDescription,
}

impl PlaneCoverInput {
    pub(crate) fn resolve(
        self,
        zoom: f32,
        pan: Vec2,
        pixel_fit: crate::displayed_image_transform::RectPixelFit,
    ) -> Option<crate::displayed_image_transform::DisplayedImageTransform> {
        use crate::displayed_image_transform::ResolvedDisplayPlacement;
        let placement = match self.transform.placement {
            ResolvedDisplayPlacement::Normal { .. } => ResolvedDisplayPlacement::Normal {
                zoom_pan: Some((zoom, pan)),
            },
            ResolvedDisplayPlacement::SingletonSpread { side, gap, .. } => {
                ResolvedDisplayPlacement::SingletonSpread {
                    side,
                    gap,
                    zoom_pan: Some((zoom, pan)),
                }
            }
            ResolvedDisplayPlacement::Z {
                active,
                factor,
                singleton_side,
                singleton_gap,
                ..
            } => ResolvedDisplayPlacement::Z {
                active,
                factor,
                singleton_side,
                singleton_gap,
                zoom_pan: Some((zoom, pan)),
            },
        };
        crate::ui_fullscreen::resolve_fs_image_transform(
            crate::displayed_image_transform::DisplayedImageTransformInput {
                placement,
                pixel_fit,
                ..self.transform
            },
            self.layout_source_size,
        )
    }

    fn continuous(
        self,
        zoom: f32,
        pan: Vec2,
    ) -> Option<crate::displayed_image_transform::DisplayedImageTransform> {
        self.resolve(
            zoom,
            pan,
            crate::displayed_image_transform::RectPixelFit::Proportional,
        )
    }

    fn zoom_about_source(self, zoom: f32, pan: Vec2, next_zoom: f32, center: Pos2) -> Option<Vec2> {
        let source = self
            .continuous(zoom, pan)?
            .screen_to_source_normalized(center);
        let next = self.continuous(next_zoom, pan)?;
        Some(pan + (center - next.source_normalized_to_screen(source)))
    }

    fn cover(self, zoom: f32, pan: Vec2, center: Pos2) -> Option<(f32, Vec2)> {
        let current = self.continuous(zoom, pan)?;
        let quad = plane_effective_quad(&current, self.material)?;
        let target = self
            .transform
            .viewport_rect
            .expand(PLANE_COVER_MARGIN_PHYSICAL / self.transform.pixels_per_point);
        let corners = [
            target.left_top(),
            target.right_top(),
            target.right_bottom(),
            target.left_bottom(),
        ];
        let u = (quad[1] - quad[0]).normalized();
        let v = (quad[3] - quad[0]).normalized();
        let project = |axis: Vec2| {
            corners
                .iter()
                .map(|p| (*p - quad[0]).dot(axis))
                .fold((f32::INFINITY, f32::NEG_INFINITY), |(lo, hi), x| {
                    (lo.min(x), hi.max(x))
                })
        };
        let (left, right) = project(u);
        let (top, bottom) = project(v);
        let width = (quad[1] - quad[0]).length();
        let height = (quad[3] - quad[0]).length();
        if left >= 0. && top >= 0. && right <= width && bottom <= height {
            return Some((zoom, pan));
        }
        let cover_zoom = zoom * ((right - left) / width).max((bottom - top) / height);
        let next_zoom = zoom.max(cover_zoom).min(crate::ui_fullscreen::ZOOM_MAX);
        let mut next_pan = self.zoom_about_source(zoom, pan, next_zoom, center)?;
        if cover_zoom > crate::ui_fullscreen::ZOOM_MAX {
            return Some((next_zoom, next_pan));
        }
        let next_quad =
            plane_effective_quad(&self.continuous(next_zoom, next_pan)?, self.material)?;
        let correction = |axis: Vec2, extent: f32| {
            let (lo, hi) = corners
                .iter()
                .map(|p| (*p - next_quad[0]).dot(axis))
                .fold((f32::INFINITY, f32::NEG_INFINITY), |(lo, hi), x| {
                    (lo.min(x), hi.max(x))
                });
            // Roundoff at the exact cover scale can invert the otherwise nonempty interval.
            0_f32.max(hi - extent).min(lo.max(hi - extent))
        };
        next_pan += u * correction(u, (next_quad[1] - next_quad[0]).length())
            + v * correction(v, (next_quad[3] - next_quad[0]).length());
        Some((next_zoom, next_pan))
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct PlaneTilt {
    pub yaw: f32,
    pub pitch: f32,
}

/// Keep the released pan/rotation pair together; a lens drag replaces that owner.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum FsCanvasDrag {
    Image {
        pan: Option<(Pos2, Vec2)>,
        rotation: Option<(Pos2, f32)>,
    },
    PlaneLens {
        anchor: PlaneViewAnchor,
        start: Pos2,
        center: Vec2,
    },
}
impl Default for FsCanvasDrag {
    fn default() -> Self {
        Self::Image {
            pan: None,
            rotation: None,
        }
    }
}
impl FsCanvasDrag {
    pub(crate) fn pan_start(&self) -> Option<(Pos2, Vec2)> {
        match self {
            Self::Image { pan, .. } => *pan,
            _ => None,
        }
    }
    pub(crate) fn rotation_start(&self) -> Option<(Pos2, f32)> {
        match self {
            Self::Image { rotation, .. } => *rotation,
            _ => None,
        }
    }
    pub(crate) fn pan_start_mut(&mut self) -> Option<&mut (Pos2, Vec2)> {
        match self {
            Self::Image { pan, .. } => pan.as_mut(),
            _ => None,
        }
    }
    pub(crate) fn set_pan_start(&mut self, value: Option<(Pos2, Vec2)>) {
        if let Self::Image { pan, .. } = self {
            *pan = value;
        }
    }
    pub(crate) fn set_rotation_start(&mut self, value: Option<(Pos2, f32)>) {
        if let Self::Image { rotation, .. } = self {
            *rotation = value;
        }
    }
    pub(crate) fn is_lens(&self) -> bool {
        matches!(self, Self::PlaneLens { .. })
    }
    pub(crate) fn retire_lens(&mut self) {
        if self.is_lens() {
            *self = Self::default();
        }
    }
    pub(crate) fn lens_center(
        &self,
        anchor: PlaneViewAnchor,
        pos: Pos2,
        canvas: Rect,
    ) -> Option<Vec2> {
        match *self {
            Self::PlaneLens {
                anchor: owner,
                start,
                center,
            } if owner == anchor && canvas.is_positive() => {
                Some((center + (pos - start) / canvas.size()).clamp(Vec2::ZERO, Vec2::splat(1.)))
            }
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct PlaneViewAnchor {
    pub fs_idx: usize,
    pub item: crate::item_identity::ItemId,
    pub items_generation: u64,
    pub slice: crate::page_split::PageSlice,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct PlaneViewPose {
    pub anchor: PlaneViewAnchor,
    pub strength: f32,
    pub lens_center: Vec2,
}

pub(crate) struct PlaneViewState {
    pub pose: PlaneViewPose,
    pub tilt: PlaneTilt,
    pub color: PlaneColorPhase,
    pub last_drawn: Option<Arc<crate::plane_view_wgpu::PlaneFrameSnapshot>>,
}
impl PlaneViewState {
    pub(crate) fn new(pose: PlaneViewPose) -> Self {
        Self {
            pose,
            tilt: PlaneTilt::default(),
            color: PlaneColorPhase::Empty,
            last_drawn: None,
        }
    }
}
pub(crate) enum PlaneColorPhase {
    Empty,
    Pending {
        previous: Option<Arc<crate::plane_view_wgpu::PlaneMaterial>>,
    },
    Ready(Arc<crate::plane_view_wgpu::PlaneMaterial>),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PlaneViewEntryBlock {
    NotImage,
    Animation,
    Spread,
    Continuous,
    NoGpu,
    Editing,
    Trim,
    Adjustment,
    Analysis,
    Compare,
    ZoomMode,
    PageTurn,
    OtherProjection,
}
impl PlaneViewEntryBlock {
    pub(crate) fn message(self) -> &'static str {
        match self {
            Self::NotImage => "魚眼は静止画の閲覧中に使えます",
            Self::Animation => "アニメーション画像では魚眼を使えません",
            Self::Spread => "魚眼は1ページ表示で使えます",
            Self::Continuous => "連結読みでは魚眼を使えません",
            Self::NoGpu => "魚眼にはGPU描画が必要です",
            Self::Editing => "編集モードを閉じてから魚眼を使ってください",
            Self::Trim => "表示トリムの編集を閉じてから魚眼を使ってください",
            Self::Adjustment => "補正パネルを閉じてから魚眼を使ってください",
            Self::Analysis => "分析モードでは魚眼を使えません",
            Self::Compare => "比較表示では魚眼を使えません",
            Self::ZoomMode => "全画面ズームを閉じてから魚眼を使ってください",
            Self::PageTurn => "ページめくり中は魚眼を使えません",
            Self::OtherProjection => "3D写真・360度表示では魚眼を使えません",
        }
    }
}

pub(crate) struct PlaneViewWheelInput {
    pub current: Option<PlaneViewPose>,
    pub anchor: PlaneViewAnchor,
    pub zoom: f32,
    pub pan: Vec2,
    pub pointer: Option<Pos2>,
    pub canvas: Rect,
    pub notches: f32,
    pub allowed: Result<(), PlaneViewEntryBlock>,
    pub step: crate::settings::FullscreenWheelZoomStep,
    pub cover: Option<PlaneCoverInput>,
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct PlaneViewWheelOutcome {
    pub pose: Option<PlaneViewPose>,
    pub zoom: f32,
    pub pan: Vec2,
    pub blocked: Option<PlaneViewEntryBlock>,
}
impl PlaneViewWheelOutcome {
    pub(crate) fn changes_view(self, pose: Option<PlaneViewPose>, zoom: f32, pan: Vec2) -> bool {
        self.blocked.is_none() && (self.pose != pose || self.zoom != zoom || self.pan != pan)
    }
}

pub(crate) fn plane_view_wheel_transition(input: PlaneViewWheelInput) -> PlaneViewWheelOutcome {
    let current = input.current.filter(|pose| pose.anchor == input.anchor);
    let mut out = PlaneViewWheelOutcome {
        pose: current,
        zoom: input.zoom,
        pan: input.pan,
        blocked: None,
    };
    if !input.notches.is_finite() || input.notches == 0. || !input.canvas.is_positive() {
        return out;
    }
    let Some(cover) = input.cover else {
        return out;
    };
    if current.is_none() {
        if input.notches < 0. {
            return out;
        }
        if let Err(block) = input.allowed {
            out.blocked = Some(block);
            return out;
        }
    }
    let mut pose = current.unwrap_or(PlaneViewPose {
        anchor: input.anchor,
        strength: 0.,
        lens_center: input
            .pointer
            .filter(|p| input.canvas.contains(*p))
            .map_or(Vec2::splat(0.5), |p| {
                (p - input.canvas.min) / input.canvas.size()
            }),
    });
    let used = if input.notches < 0. {
        input.notches.max(-4. * pose.strength)
    } else {
        input.notches
    };
    pose.strength = (pose.strength + used / 4.).clamp(0., 1.);
    let center = input.canvas.min + pose.lens_center * input.canvas.size();
    let Some((zoom, pan)) = (if used > 0. {
        cover.cover(out.zoom, out.pan, center)
    } else {
        Some((out.zoom, out.pan))
    }) else {
        return out;
    };
    let mut next_zoom = zoom;
    let mut unused_pan = Vec2::ZERO;
    crate::app::App::apply_wheel_zoom(
        &mut next_zoom,
        &mut unused_pan,
        used * 40.,
        input.step,
        None,
        input.canvas.center(),
    );
    let Some(mut next_pan) = cover.zoom_about_source(zoom, pan, next_zoom, center) else {
        return out;
    };
    let Some(final_transform) = cover.continuous(next_zoom, next_pan) else {
        return out;
    };
    // hit_rect is already clipped to the viewport. The minimum-overlap rule needs the
    // whole painted page, including when it is completely outside that viewport.
    let painted_bounds = Rect::from_points(&final_transform.paint_quad().positions);
    next_pan += crate::ui_fullscreen::pan_correction_for_minimum_overlap(
        painted_bounds,
        cover.transform.viewport_rect,
    );
    out.zoom = next_zoom;
    out.pan = next_pan;
    out.pose = (pose.strength > 0.).then_some(pose);
    out
}

pub(crate) fn placement_frames_match(a: Rect, b: Rect, pixels_per_point: f32) -> bool {
    a.is_finite()
        && b.is_finite()
        && [
            a.left() - b.left(),
            a.top() - b.top(),
            a.right() - b.right(),
            a.bottom() - b.bottom(),
        ]
        .into_iter()
        .all(|d| d.abs() * pixels_per_point <= 1. / 64.)
}

/// CPU counterpart of the shader camera; all rejected rays stay finite in WGSL.
#[cfg(test)]
pub(crate) fn camera_point(
    p: Pos2,
    canvas: Rect,
    center: Vec2,
    k: f32,
    lens: PlaneLensKind,
    tilt: PlaneTilt,
) -> Option<Pos2> {
    let r_scale = 0.5 * canvas.size().min_elem();
    if r_scale <= 0. {
        return None;
    }
    let c = canvas.min + center * canvas.size();
    let u = (p - c) / r_scale;
    let r = u.length();
    if k <= 1.0e-7 {
        return Some(p);
    }
    let theta = lens.theta(k * r)?;
    let sigma = if r < 1.0e-6 { k } else { theta.sin() / r };
    let d = [sigma * u.x, sigma * u.y, theta.cos()];
    let (sy, cy) = tilt.yaw.sin_cos();
    let (sp, cp) = tilt.pitch.sin_cos();
    // Rot = Ry(yaw) Rx(pitch). Its columns form the board's local basis.
    let nx = [cy, 0., -sy];
    let ny = [sy * sp, cp, cy * sp];
    let n = [sy * cp, -sp, cy * cp];
    let dot = |a: [f32; 3], b: [f32; 3]| a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
    let den = dot(n, d);
    if den <= 1.0e-6 {
        return None;
    }
    let t = n[2] / den;
    if t <= 0. {
        return None;
    }
    let v = [t * d[0], t * d[1], t * d[2] - 1.];
    Some(c + Vec2::new(dot(nx, v), dot(ny, v)) * (r_scale / k))
}

/// The CPU inverse of the paint quad, including trim/slice and partial material coverage.
#[cfg(test)]
pub(crate) fn texture_point(
    q: Pos2,
    transform: &crate::displayed_image_transform::DisplayedImageTransform,
    coverage: Rect,
    sampler_roi: Rect,
) -> Option<Pos2> {
    let delta = q - transform.full_image_rect.center();
    let (sin, cos) = transform.free_rotation_rad.sin_cos();
    let local = transform.full_image_rect.center()
        + Vec2::new(
            cos * delta.x + sin * delta.y,
            -sin * delta.x + cos * delta.y,
        );
    let in_uv = (local - transform.paint_rect.min) / transform.paint_rect.size();
    let inside = |p: Vec2| p.x >= 0. && p.y >= 0. && p.x <= 1. && p.y <= 1.;
    if !inside(in_uv) {
        return None;
    }
    let source = transform.screen_to_source_normalized(q);
    if !inside((source - coverage.min) / coverage.size()) {
        return None;
    }
    let material_min = coverage.min + coverage.size() * sampler_roi.min.to_vec2();
    let material_size = coverage.size() * sampler_roi.size();
    let tex = (source - material_min) / material_size;
    inside(tex).then_some(tex.to_pos2())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn anchor() -> PlaneViewAnchor {
        PlaneViewAnchor {
            fs_idx: 0,
            item: crate::item_identity::ItemId::NONE,
            items_generation: 1,
            slice: crate::page_split::PageSlice::Full,
        }
    }
    fn canvas() -> Rect {
        Rect::from_min_size(egui::pos2(20., 30.), Vec2::new(800., 600.))
    }
    fn step(
        current: Option<PlaneViewPose>,
        zoom: f32,
        pan: Vec2,
        notches: f32,
    ) -> PlaneViewWheelOutcome {
        plane_view_wheel_transition(PlaneViewWheelInput {
            current,
            anchor: anchor(),
            zoom,
            pan,
            pointer: Some(egui::pos2(120., 130.)),
            canvas: canvas(),
            notches,
            allowed: Ok(()),
            step: crate::settings::FullscreenWheelZoomStep::Percent10,
            cover: Some(super::revision_tests::fixture(
                canvas(),
                Vec2::new(800., 600.),
            )),
        })
    }
    fn close(a: f32, b: f32) {
        assert!((a - b).abs() < 1.0e-4, "{a} != {b}");
    }
    #[test]
    fn plane_wheel_enters_saturates_and_spends_only_strength_on_exit() {
        let first = step(None, 1., Vec2::ZERO, 1.);
        let pose = first.pose.unwrap();
        close(pose.strength, 0.25);
        assert_eq!(pose.lens_center, Vec2::new(0.125, 1. / 6.));
        let full = step(first.pose, first.zoom, first.pan, 3.);
        close(full.pose.unwrap().strength, 1.);
        let more = step(full.pose, full.zoom, full.pan, 1.);
        close(more.pose.unwrap().strength, 1.);
        assert!(more.zoom > full.zoom);
        let joined = step(first.pose, first.zoom, first.pan, -2.);
        let one = step(first.pose, first.zoom, first.pan, -1.);
        let split = step(one.pose, one.zoom, one.pan, -1.);
        assert_eq!(joined, split);
        assert!(joined.pose.is_none());
        close(joined.zoom, 1. + 8. / 600.);
        assert_eq!(step(None, 1., Vec2::ZERO, -2.).zoom, 1.);
    }
    #[test]
    fn plane_wheel_fractional_partition_anchor_and_all_entry_blocks() {
        let joined = step(None, 1., Vec2::ZERO, 3.5);
        let mut split = step(None, 1., Vec2::ZERO, 0.25);
        for _ in 0..13 {
            split = step(split.pose, split.zoom, split.pan, 0.25);
        }
        close(joined.zoom, split.zoom);
        assert!((joined.pan - split.pan).length() < 0.001);
        close(joined.pose.unwrap().strength, split.pose.unwrap().strength);
        for block in [
            PlaneViewEntryBlock::NotImage,
            PlaneViewEntryBlock::Animation,
            PlaneViewEntryBlock::Spread,
            PlaneViewEntryBlock::Continuous,
            PlaneViewEntryBlock::NoGpu,
            PlaneViewEntryBlock::Editing,
            PlaneViewEntryBlock::Trim,
            PlaneViewEntryBlock::Adjustment,
            PlaneViewEntryBlock::Analysis,
            PlaneViewEntryBlock::Compare,
            PlaneViewEntryBlock::ZoomMode,
            PlaneViewEntryBlock::PageTurn,
            PlaneViewEntryBlock::OtherProjection,
        ] {
            let out = plane_view_wheel_transition(PlaneViewWheelInput {
                current: None,
                anchor: anchor(),
                zoom: 1.,
                pan: Vec2::ZERO,
                pointer: None,
                canvas: canvas(),
                notches: 1.,
                allowed: Err(block),
                step: Default::default(),
                cover: Some(super::revision_tests::fixture(
                    canvas(),
                    Vec2::new(800., 600.),
                )),
            });
            assert_eq!(out.blocked, Some(block));
            assert!(out.pose.is_none());
            assert_eq!(out.zoom, 1.);
        }
        let mut old = joined.pose.unwrap();
        old.anchor.items_generation += 1;
        assert!(step(Some(old), 1., Vec2::ZERO, -1.).pose.is_none());
        let centered = plane_view_wheel_transition(PlaneViewWheelInput {
            current: None,
            anchor: anchor(),
            zoom: 1.,
            pan: Vec2::ZERO,
            pointer: None,
            canvas: canvas(),
            notches: 1.,
            allowed: Ok(()),
            step: Default::default(),
            cover: Some(super::revision_tests::fixture(
                canvas(),
                Vec2::new(800., 600.),
            )),
        });
        assert_eq!(centered.pose.unwrap().lens_center, Vec2::splat(0.5));
    }
    #[test]
    fn plane_lens_drag_exclusively_owns_normal_origins_and_clamps() {
        let drag = FsCanvasDrag::PlaneLens {
            anchor: anchor(),
            start: Pos2::ZERO,
            center: Vec2::splat(0.5),
        };
        assert!(drag.pan_start().is_none() && drag.rotation_start().is_none());
        assert_eq!(
            drag.lens_center(anchor(), egui::pos2(800., -600.), canvas()),
            Some(Vec2::new(1., 0.))
        );
        let mut ended = drag;
        ended.retire_lens();
        assert_eq!(ended, FsCanvasDrag::default());
    }
    #[test]
    fn plane_placement_comparison_uses_physical_tolerance() {
        for ppp in [1., 1.25, 2.] {
            let a = canvas();
            assert!(placement_frames_match(
                a,
                a.translate(Vec2::splat(0.001 / ppp)),
                ppp
            ));
            assert!(!placement_frames_match(
                a,
                a.translate(Vec2::splat(1. / ppp)),
                ppp
            ));
        }
    }
    #[test]
    fn plane_setting_serde_default_and_three_lenses() {
        let settings: crate::settings::Settings = serde_json::from_str("{}").unwrap();
        assert_eq!(settings.plane_view_lens, PlaneLensKind::Equidistant);
        for lens in PlaneLensKind::ALL {
            let settings = crate::settings::Settings {
                plane_view_lens: lens,
                ..Default::default()
            };
            let restored: crate::settings::Settings =
                serde_json::from_value(serde_json::to_value(settings).unwrap()).unwrap();
            assert_eq!(restored.plane_view_lens, lens);
        }
    }

    #[test]
    fn plane_trim_slice_quarter_turn_and_partial_coverage_use_paint_local_bounds() {
        use crate::displayed_image_transform::*;
        for rotation in [
            crate::rotation_db::Rotation::None,
            crate::rotation_db::Rotation::Cw90,
            crate::rotation_db::Rotation::Cw180,
            crate::rotation_db::Rotation::Cw270,
        ] {
            let transform = DisplayedImageTransform::resolve(DisplayedImageTransformInput {
                page_idx: 0,
                viewport_rect: canvas(),
                source_size: Vec2::new(800., 600.),
                texture_size: Vec2::new(800., 600.),
                rotation,
                free_rotation_rad: 0.,
                content_bbox: Some(Rect::from_min_max(
                    egui::pos2(0.25, 0.2),
                    egui::pos2(0.5, 0.8),
                )),
                fit_mode: crate::fullscreen_specified_scale::FullscreenFitInput::Legacy(
                    crate::settings::FullscreenFitMode::Page,
                ),
                fit_scale_limits: Default::default(),
                pixels_per_point: 1.,
                placement: ResolvedDisplayPlacement::Normal { zoom_pan: None },
                pixel_fit: RectPixelFit::Proportional,
            })
            .unwrap();
            let source = egui::pos2(0.1, 0.5);
            let screen = transform.source_normalized_to_screen(source);
            let full = Rect::from_min_max(Pos2::ZERO, egui::pos2(1., 1.));
            assert!(
                texture_point(screen, &transform, full, full).is_none(),
                "source UV is inside texture but outside displayed trim/slice"
            );
            let coverage = Rect::from_min_max(egui::pos2(0.25, 0.), egui::pos2(0.75, 1.));
            let uv = texture_point(
                transform.source_normalized_to_screen(egui::pos2(0.3, 0.5)),
                &transform,
                coverage,
                full,
            )
            .unwrap();
            close(uv.x, 0.1);
            close(uv.y, 0.5);
        }
    }
}

#[cfg(test)]
mod precision_tests;
#[cfg(test)]
mod revision_tests;
