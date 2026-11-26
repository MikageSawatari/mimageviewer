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
    pub(crate) fn g(self, theta: f32) -> f32 {
        match self {
            Self::Equidistant => theta,
            Self::Stereographic => 2. * (theta * 0.5).tan(),
            Self::EquisolidAngle => 2. * (theta * 0.5).sin(),
        }
    }
    pub(crate) fn alpha_max(self) -> f32 {
        match self {
            Self::Equidistant => std::f32::consts::FRAC_PI_2 / 0.8,
            Self::Stereographic => 1.792_110_8,
            Self::EquisolidAngle => 2.168_202_6,
        }
    }
    pub(crate) fn coefficient(self, strength: f32) -> f32 {
        self.g(strength * self.alpha_max())
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
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct PlaneViewWheelOutcome {
    pub pose: Option<PlaneViewPose>,
    pub zoom: f32,
    pub pan: Vec2,
    pub blocked: Option<PlaneViewEntryBlock>,
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
        input.notches.max(-8. * pose.strength)
    } else {
        input.notches
    };
    pose.strength = (pose.strength + used / 8.).clamp(0., 1.);
    crate::app::App::apply_wheel_zoom(
        &mut out.zoom,
        &mut out.pan,
        used * 40.,
        input.step,
        Some(input.canvas.min + pose.lens_center * input.canvas.size()),
        input.canvas.center(),
    );
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
    strength: f32,
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
    let k = lens.coefficient(strength);
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
        })
    }
    fn close(a: f32, b: f32) {
        assert!((a - b).abs() < 1.0e-4, "{a} != {b}");
    }
    #[test]
    fn plane_lens_limits_center_monotonicity_and_identity_limit() {
        for (lens, degrees) in PlaneLensKind::ALL
            .into_iter()
            .zip([112.5, 102.6804, 124.2289])
        {
            close(lens.alpha_max().to_degrees(), degrees);
            let k = lens.coefficient(1.);
            close(lens.g(std::f32::consts::FRAC_PI_2) / k, 0.8);
            let c = canvas().center();
            assert_eq!(
                camera_point(
                    c,
                    canvas(),
                    Vec2::splat(0.5),
                    1.,
                    lens,
                    PlaneTilt::default()
                ),
                Some(c)
            );
            let mut previous = 1.;
            for i in 1..80 {
                let r = i as f32 / 100.;
                let p = c + Vec2::new(r * 300., 0.);
                let q = camera_point(
                    p,
                    canvas(),
                    Vec2::splat(0.5),
                    1.,
                    lens,
                    PlaneTilt::default(),
                )
                .unwrap();
                let w = (q.x - c.x) / (p.x - c.x);
                assert!(w >= previous - 1.0e-5);
                previous = w;
                let theta = lens.theta(k * r).unwrap();
                assert!(((q.x - c.x) - 300. / k * theta.tan()).abs() < 0.1);
                let tiny = camera_point(
                    p,
                    canvas(),
                    Vec2::splat(0.5),
                    1.0e-5,
                    lens,
                    PlaneTilt::default(),
                )
                .unwrap();
                assert!((tiny - p).length() < 0.001);
            }
            assert!(
                camera_point(
                    c + Vec2::new(240.1, 0.),
                    canvas(),
                    Vec2::splat(0.5),
                    1.,
                    lens,
                    PlaneTilt::default()
                )
                .is_none()
            );
            assert!(lens.theta(f32::INFINITY).is_none());
        }
        assert!(
            PlaneLensKind::Equidistant
                .theta(std::f32::consts::PI + 0.01)
                .is_none()
        );
        assert!(PlaneLensKind::EquisolidAngle.theta(2.01).is_none());
    }
    #[test]
    fn plane_wheel_enters_saturates_and_spends_only_strength_on_exit() {
        let first = step(None, 1., Vec2::ZERO, 1.);
        let pose = first.pose.unwrap();
        close(pose.strength, 0.125);
        assert_eq!(pose.lens_center, Vec2::new(0.125, 1. / 6.));
        let full = step(first.pose, first.zoom, first.pan, 7.);
        close(full.pose.unwrap().strength, 1.);
        let more = step(full.pose, full.zoom, full.pan, 1.);
        close(more.pose.unwrap().strength, 1.);
        assert!(more.zoom > full.zoom);
        let joined = step(first.pose, first.zoom, first.pan, -2.);
        let one = step(first.pose, first.zoom, first.pan, -1.);
        let split = step(one.pose, one.zoom, one.pan, -1.);
        assert_eq!(joined, split);
        assert!(joined.pose.is_none());
        close(joined.zoom, 1.);
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
    #[test]
    fn plane_camera_rejects_domain_horizon_and_matches_untilted_intersection() {
        for lens in PlaneLensKind::ALL {
            let c = canvas().center();
            let radius = 300.;
            let k = lens.coefficient(1.);
            assert!(
                camera_point(
                    c + Vec2::new(radius * 0.81, 0.),
                    canvas(),
                    Vec2::splat(0.5),
                    1.,
                    lens,
                    PlaneTilt::default()
                )
                .is_none()
            );
            for r in [0.000001, 0.1, 0.5, 0.7999] {
                let theta = lens.theta(k * r).unwrap();
                let q = camera_point(
                    c + Vec2::new(radius * r, 0.),
                    canvas(),
                    Vec2::splat(0.5),
                    1.,
                    lens,
                    PlaneTilt::default(),
                )
                .unwrap();
                assert!(q.is_finite());
                let expected = radius * theta.tan() / k;
                assert!((q.x - c.x - expected).abs() < expected.abs() * 0.003 + 0.001);
            }
            for strength in [0.001, 0.00001] {
                let p = c + Vec2::new(100., 50.);
                let q = camera_point(
                    p,
                    canvas(),
                    Vec2::splat(0.5),
                    strength,
                    lens,
                    PlaneTilt::default(),
                )
                .unwrap();
                assert!((q - p).length() < 0.01);
            }
        }
        assert!(
            PlaneLensKind::Equidistant
                .theta(std::f32::consts::PI + 0.001)
                .is_none()
        );
        assert!(PlaneLensKind::EquisolidAngle.theta(2.001).is_none());
    }
}
