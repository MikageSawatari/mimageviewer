use eframe::egui;

use crate::rotation_db::Rotation;
use crate::settings::FullscreenFitMode;

const EPSILON: f32 = 1.0e-6;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct FullscreenFitScaleLimits {
    pub(crate) no_upscale: bool,
    pub(crate) no_downscale: bool,
}

impl FullscreenFitScaleLimits {
    pub(crate) fn active(self) -> bool {
        self.no_upscale || self.no_downscale
    }

    pub(crate) fn apply(self, mut scale: f32) -> f32 {
        if self.no_upscale {
            scale = scale.min(1.0);
        }
        if self.no_downscale {
            scale = scale.max(1.0);
        }
        scale
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum ResolvedDisplayPlacement {
    Normal {
        zoom_pan: Option<(f32, egui::Vec2)>,
    },
    Z {
        active: bool,
        factor: f32,
        zoom_pan: Option<(f32, egui::Vec2)>,
    },
}

impl ResolvedDisplayPlacement {
    fn zoom_pan(self) -> Option<(f32, egui::Vec2)> {
        match self {
            Self::Normal { zoom_pan } | Self::Z { zoom_pan, .. } => zoom_pan,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct DisplayedImageTransformInput {
    pub(crate) page_idx: usize,
    pub(crate) viewport_rect: egui::Rect,
    pub(crate) source_size: egui::Vec2,
    pub(crate) texture_size: egui::Vec2,
    pub(crate) rotation: Rotation,
    pub(crate) free_rotation_rad: f32,
    pub(crate) content_bbox: Option<egui::Rect>,
    pub(crate) fit_mode: FullscreenFitMode,
    pub(crate) fit_scale_limits: FullscreenFitScaleLimits,
    pub(crate) placement: ResolvedDisplayPlacement,
}

#[derive(Clone, Copy, Debug)]
#[allow(dead_code)]
pub(crate) struct DisplayedImageTransform {
    pub(crate) page_idx: usize,
    pub(crate) source_size: egui::Vec2,
    pub(crate) texture_size: egui::Vec2,
    pub(crate) viewport_rect: egui::Rect,
    /// 回転後の表示寸法を fit した、trim 前の画像全体矩形。
    pub(crate) full_image_rect: egui::Rect,
    /// 実際に texture を paint する矩形。表示 trim 時は bbox 部分だけになる。
    pub(crate) paint_rect: egui::Rect,
    /// ポインタ判定の broad phase。自由回転時は paint quad の AABB。
    pub(crate) hit_rect: egui::Rect,
    pub(crate) uv_rect: egui::Rect,
    pub(crate) rotation: Rotation,
    pub(crate) free_rotation_rad: f32,
    /// 要求された表示 trim。回転中は現行仕様どおり UV crop には適用しない。
    pub(crate) content_bbox: Option<egui::Rect>,
    pub(crate) fit_mode: FullscreenFitMode,
    pub(crate) fit_scale_limits: FullscreenFitScaleLimits,
    pub(crate) placement: ResolvedDisplayPlacement,
    /// texture の回転後表示ピクセル 1px あたりの画面スケール。
    pub(crate) total_scale: f32,
}

impl DisplayedImageTransform {
    pub(crate) fn resolve(input: DisplayedImageTransformInput) -> Option<Self> {
        if !rect_is_valid(input.viewport_rect)
            || !size_is_valid(input.source_size)
            || !size_is_valid(input.texture_size)
        {
            return None;
        }
        let display_size = rotated_size(input.texture_size, input.rotation);
        let fit_bbox = effective_bbox(input.rotation, input.free_rotation_rad, input.content_bbox);
        let bbox_fit = fit_bbox.map(|bbox| {
            let width = (bbox.width() * display_size.x).max(1.0);
            let height = (bbox.height() * display_size.y).max(1.0);
            let center = egui::vec2(
                (bbox.center().x - 0.5) * display_size.x,
                (bbox.center().y - 0.5) * display_size.y,
            );
            (width, height, center)
        });
        let page_fit = || {
            (input.viewport_rect.width() / display_size.x)
                .min(input.viewport_rect.height() / display_size.y)
        };
        let (fit_scale, content_center) = match (input.fit_mode, bbox_fit) {
            (FullscreenFitMode::Width, Some((width, _, center))) => {
                (input.viewport_rect.width() / width, center)
            }
            (FullscreenFitMode::Width, None) => (
                input.viewport_rect.width() / display_size.x,
                egui::Vec2::ZERO,
            ),
            (FullscreenFitMode::Height, Some((_, height, center))) => {
                (input.viewport_rect.height() / height, center)
            }
            (FullscreenFitMode::Height, None) => (
                input.viewport_rect.height() / display_size.y,
                egui::Vec2::ZERO,
            ),
            (FullscreenFitMode::Original, Some((_, _, center))) => (1.0, center),
            (FullscreenFitMode::Original, None) => (1.0, egui::Vec2::ZERO),
            (_, Some((width, height, center))) => (
                (input.viewport_rect.width() / width).min(input.viewport_rect.height() / height),
                center,
            ),
            (_, None) => (page_fit(), egui::Vec2::ZERO),
        };
        let fit_scale = input.fit_scale_limits.apply(fit_scale);
        let (total_scale, base_center) = match input.placement.zoom_pan() {
            Some((zoom, pan)) => (fit_scale * zoom, input.viewport_rect.center() + pan),
            None => (fit_scale, input.viewport_rect.center()),
        };
        if !total_scale.is_finite() || total_scale <= 0.0 {
            return None;
        }
        let center = base_center - content_center * total_scale;
        let full_image_rect = egui::Rect::from_center_size(center, display_size * total_scale);
        Self::from_resolved_rect(input, full_image_rect)
    }

    /// 見開き・連結読みのレイアウトが先に確定させた画像全体矩形から transform を構築する。
    pub(crate) fn from_resolved_rect(
        input: DisplayedImageTransformInput,
        full_image_rect: egui::Rect,
    ) -> Option<Self> {
        if !rect_is_valid(input.viewport_rect)
            || !rect_is_valid(full_image_rect)
            || !size_is_valid(input.source_size)
            || !size_is_valid(input.texture_size)
        {
            return None;
        }
        let display_size = rotated_size(input.texture_size, input.rotation);
        let scale_x = full_image_rect.width() / display_size.x;
        let scale_y = full_image_rect.height() / display_size.y;
        let total_scale = (scale_x + scale_y) * 0.5;
        if !total_scale.is_finite() || total_scale <= 0.0 {
            return None;
        }
        let fit_bbox = effective_bbox(input.rotation, input.free_rotation_rad, input.content_bbox);
        let (paint_rect, uv_rect) = fit_bbox
            .map(|bbox| (normalized_sub_rect(full_image_rect, bbox), bbox))
            .unwrap_or((full_image_rect, full_uv_rect()));
        let hit_rect = rotated_rect_aabb(
            paint_rect,
            full_image_rect.center(),
            input.free_rotation_rad,
        )
        .intersect(input.viewport_rect);
        Some(Self {
            page_idx: input.page_idx,
            source_size: input.source_size,
            texture_size: input.texture_size,
            viewport_rect: input.viewport_rect,
            full_image_rect,
            paint_rect,
            hit_rect,
            uv_rect,
            rotation: input.rotation,
            free_rotation_rad: input.free_rotation_rad,
            content_bbox: input.content_bbox,
            fit_mode: input.fit_mode,
            fit_scale_limits: input.fit_scale_limits,
            placement: input.placement,
            total_scale,
        })
    }

    pub(crate) fn screen_to_source(&self, screen: egui::Pos2) -> egui::Pos2 {
        let p = self.screen_to_source_normalized(screen);
        egui::pos2(p.x * self.source_size.x, p.y * self.source_size.y)
    }

    #[allow(dead_code)]
    pub(crate) fn source_to_screen(&self, source: egui::Pos2) -> egui::Pos2 {
        self.source_normalized_to_screen(egui::pos2(
            source.x / self.source_size.x.max(EPSILON),
            source.y / self.source_size.y.max(EPSILON),
        ))
    }

    pub(crate) fn screen_to_source_normalized(&self, screen: egui::Pos2) -> egui::Pos2 {
        let p = rotate_about(
            screen,
            self.full_image_rect.center(),
            -self.free_rotation_rad,
        );
        let u = (p.x - self.full_image_rect.left()) / self.full_image_rect.width().max(EPSILON);
        let v = (p.y - self.full_image_rect.top()) / self.full_image_rect.height().max(EPSILON);
        let (s, t) = inverse_uv(self.rotation, u, v);
        egui::pos2(s, t)
    }

    pub(crate) fn source_normalized_to_screen(&self, source: egui::Pos2) -> egui::Pos2 {
        let (u, v) = forward_uv(self.rotation, source.x, source.y);
        let p = egui::pos2(
            self.full_image_rect.left() + u * self.full_image_rect.width(),
            self.full_image_rect.top() + v * self.full_image_rect.height(),
        );
        rotate_about(p, self.full_image_rect.center(), self.free_rotation_rad)
    }

    pub(crate) fn contains_screen(&self, screen: egui::Pos2) -> bool {
        self.viewport_rect.contains(screen)
            && self.hit_rect.contains(screen)
            && self
                .uv_rect
                .contains(self.screen_to_source_normalized(screen))
    }

    pub(crate) fn screen_px_per_source_px(&self, source_size: egui::Vec2) -> f32 {
        let display_size = rotated_size(source_size, self.rotation);
        let sx = self.full_image_rect.width() / display_size.x.max(EPSILON);
        let sy = self.full_image_rect.height() / display_size.y.max(EPSILON);
        (sx + sy) * 0.5
    }

    pub(crate) fn paint_texture(
        &self,
        painter: &egui::Painter,
        texture_id: egui::TextureId,
        tint: egui::Color32,
    ) {
        let display_uvs = [(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)];
        let mut positions = [
            self.paint_rect.left_top(),
            self.paint_rect.right_top(),
            self.paint_rect.right_bottom(),
            self.paint_rect.left_bottom(),
        ];
        for position in &mut positions {
            *position = rotate_about(
                *position,
                self.full_image_rect.center(),
                self.free_rotation_rad,
            );
        }
        let mut mesh = egui::Mesh::with_texture(texture_id);
        for (index, (u, v)) in display_uvs.into_iter().enumerate() {
            let (source_u, source_v) = inverse_uv(self.rotation, u, v);
            mesh.vertices.push(egui::epaint::Vertex {
                pos: positions[index],
                uv: egui::pos2(
                    self.uv_rect.left() + source_u * self.uv_rect.width(),
                    self.uv_rect.top() + source_v * self.uv_rect.height(),
                ),
                color: tint,
            });
        }
        mesh.indices.extend_from_slice(&[0, 1, 2, 0, 2, 3]);
        painter.add(egui::Shape::mesh(mesh));
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct DisplayedPage {
    pub(crate) transform: DisplayedImageTransform,
}

impl DisplayedPage {
    pub(crate) fn new(transform: DisplayedImageTransform) -> Self {
        Self { transform }
    }

    pub(crate) fn page_idx(self) -> usize {
        self.transform.page_idx
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum FullscreenPageLayoutKind {
    #[default]
    Empty,
    Single,
    Spread,
    Continuous,
}

/// 1 フレームで実際に描画されたページ列と、その screen/source transform。
///
/// ページは画面上の描画順で保持する。gap はどのページにも属さず、gap が 0 の共有境界は
/// 先に描画されたページへ決定論的に割り当てる。
#[derive(Clone, Debug, Default)]
pub(crate) struct FullscreenPageLayout {
    kind: FullscreenPageLayoutKind,
    pages: Vec<DisplayedPage>,
}

impl FullscreenPageLayout {
    pub(crate) fn begin(&mut self, kind: FullscreenPageLayoutKind) {
        self.pages.clear();
        self.kind = kind;
    }

    pub(crate) fn clear(&mut self) {
        self.begin(FullscreenPageLayoutKind::Empty);
    }

    pub(crate) fn push(&mut self, transform: DisplayedImageTransform) {
        self.pages.push(DisplayedPage::new(transform));
    }

    pub(crate) fn hit_test(&self, pos: egui::Pos2) -> Option<&DisplayedPage> {
        self.pages
            .iter()
            .find(|page| page.transform.contains_screen(pos))
    }

    pub(crate) fn kind(&self) -> FullscreenPageLayoutKind {
        self.kind
    }

    pub(crate) fn single_page(&self) -> Option<&DisplayedPage> {
        if self.kind != FullscreenPageLayoutKind::Single || self.pages.len() != 1 {
            return None;
        }
        self.pages.first()
    }

    pub(crate) fn spread_pair(&self) -> Option<(usize, usize)> {
        if self.kind != FullscreenPageLayoutKind::Spread || self.pages.len() != 2 {
            return None;
        }
        Some((self.pages[0].page_idx(), self.pages[1].page_idx()))
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct ZTransformInput {
    pub(crate) image: DisplayedImageTransformInput,
    pub(crate) active: bool,
    pub(crate) factor: f32,
    pub(crate) cursor: egui::Pos2,
    pub(crate) pan_band: egui::Rect,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct ResolvedZTransform {
    pub(crate) transform: DisplayedImageTransform,
    pub(crate) factor: f32,
    pub(crate) aim_frame: Option<egui::Rect>,
    pub(crate) full_image_zoom: f32,
}

impl ResolvedZTransform {
    pub(crate) fn resolve(mut input: ZTransformInput) -> Option<Self> {
        input.image.fit_mode = FullscreenFitMode::Page;
        input.image.fit_scale_limits = FullscreenFitScaleLimits::default();
        input.image.free_rotation_rad = 0.0;
        let display_size = rotated_size(input.image.texture_size, input.image.rotation);
        let bbox = effective_bbox(input.image.rotation, 0.0, input.image.content_bbox);
        let (content_min, content_size) = bbox
            .map(|bbox| {
                (
                    egui::vec2(bbox.min.x * display_size.x, bbox.min.y * display_size.y),
                    egui::vec2(
                        (bbox.width() * display_size.x).max(1.0),
                        (bbox.height() * display_size.y).max(1.0),
                    ),
                )
            })
            .unwrap_or((egui::Vec2::ZERO, display_size));
        let view = input.image.viewport_rect.size();
        let cover = (view.x / content_size.x).max(view.y / content_size.y);
        let contain = (view.x / content_size.x).min(view.y / content_size.y);
        let factor_min = if cover > 0.0 {
            (contain / cover).min(1.0)
        } else {
            1.0
        };
        let factor = input.factor.clamp(factor_min, 16.0);
        let cursor_image =
            z_cursor_image_px(input.pan_band, content_min, content_size, input.cursor);
        let (zoom_pan, full_image_zoom, aim_frame) = if input.active {
            let (zoom_full, pan_full) = z_cover_zoom_pan(
                view,
                display_size,
                content_min,
                content_size,
                factor,
                cursor_image,
            );
            let resolved = if bbox.is_some() {
                z_bbox_zoom_pan_from_full(
                    zoom_full,
                    pan_full,
                    view,
                    display_size,
                    content_min,
                    content_size,
                )
            } else {
                (zoom_full, pan_full)
            };
            (Some(resolved), zoom_full, None)
        } else {
            (
                None,
                1.0,
                Some(z_aim_frame_rect(
                    input.image.viewport_rect,
                    content_min,
                    content_size,
                    factor,
                    cursor_image,
                )),
            )
        };
        input.image.placement = ResolvedDisplayPlacement::Z {
            active: input.active,
            factor,
            zoom_pan,
        };
        Some(Self {
            transform: DisplayedImageTransform::resolve(input.image)?,
            factor,
            aim_frame,
            full_image_zoom,
        })
    }
}

fn size_is_valid(size: egui::Vec2) -> bool {
    size.x.is_finite() && size.y.is_finite() && size.x > 0.0 && size.y > 0.0
}

fn rect_is_valid(rect: egui::Rect) -> bool {
    rect.min.x.is_finite()
        && rect.min.y.is_finite()
        && rect.max.x.is_finite()
        && rect.max.y.is_finite()
        && rect.width() > 0.0
        && rect.height() > 0.0
}

fn rotated_size(size: egui::Vec2, rotation: Rotation) -> egui::Vec2 {
    match rotation {
        Rotation::Cw90 | Rotation::Cw270 => egui::vec2(size.y, size.x),
        Rotation::None | Rotation::Cw180 => size,
    }
}

fn effective_bbox(
    rotation: Rotation,
    free_rotation_rad: f32,
    content_bbox: Option<egui::Rect>,
) -> Option<egui::Rect> {
    content_bbox.filter(|_| rotation.is_none() && free_rotation_rad.abs() <= EPSILON)
}

fn full_uv_rect() -> egui::Rect {
    egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0))
}

fn normalized_sub_rect(rect: egui::Rect, uv: egui::Rect) -> egui::Rect {
    egui::Rect::from_min_max(
        egui::pos2(
            rect.left() + rect.width() * uv.min.x,
            rect.top() + rect.height() * uv.min.y,
        ),
        egui::pos2(
            rect.left() + rect.width() * uv.max.x,
            rect.top() + rect.height() * uv.max.y,
        ),
    )
}

pub(crate) fn forward_uv(rotation: Rotation, s: f32, t: f32) -> (f32, f32) {
    match rotation {
        Rotation::None => (s, t),
        Rotation::Cw90 => (1.0 - t, s),
        Rotation::Cw180 => (1.0 - s, 1.0 - t),
        Rotation::Cw270 => (t, 1.0 - s),
    }
}

pub(crate) fn inverse_uv(rotation: Rotation, u: f32, v: f32) -> (f32, f32) {
    match rotation {
        Rotation::None => (u, v),
        Rotation::Cw90 => (v, 1.0 - u),
        Rotation::Cw180 => (1.0 - u, 1.0 - v),
        Rotation::Cw270 => (1.0 - v, u),
    }
}

fn rotate_about(point: egui::Pos2, center: egui::Pos2, theta: f32) -> egui::Pos2 {
    if theta.abs() <= EPSILON {
        return point;
    }
    let (sin, cos) = theta.sin_cos();
    let d = point - center;
    center + egui::vec2(d.x * cos - d.y * sin, d.x * sin + d.y * cos)
}

fn rotated_rect_aabb(rect: egui::Rect, center: egui::Pos2, theta: f32) -> egui::Rect {
    if theta.abs() <= EPSILON {
        return rect;
    }
    let mut min = egui::pos2(f32::INFINITY, f32::INFINITY);
    let mut max = egui::pos2(f32::NEG_INFINITY, f32::NEG_INFINITY);
    for corner in [
        rect.left_top(),
        rect.right_top(),
        rect.left_bottom(),
        rect.right_bottom(),
    ] {
        let p = rotate_about(corner, center, theta);
        min.x = min.x.min(p.x);
        min.y = min.y.min(p.y);
        max.x = max.x.max(p.x);
        max.y = max.y.max(p.y);
    }
    egui::Rect::from_min_max(min, max)
}

fn z_cursor_image_px(
    band: egui::Rect,
    content_min: egui::Vec2,
    content_size: egui::Vec2,
    cursor: egui::Pos2,
) -> egui::Vec2 {
    let nx = ((cursor.x - band.left()) / band.width().max(1.0)).clamp(0.0, 1.0);
    let ny = ((cursor.y - band.top()) / band.height().max(1.0)).clamp(0.0, 1.0);
    content_min + egui::vec2(nx * content_size.x, ny * content_size.y)
}

pub(crate) fn z_visible_source(
    view: egui::Vec2,
    content_size: egui::Vec2,
    factor: f32,
    cursor: egui::Vec2,
) -> (egui::Vec2, egui::Vec2) {
    let cover = (view.x / content_size.x.max(1.0)).max(view.y / content_size.y.max(1.0));
    let contain = (view.x / content_size.x.max(1.0)).min(view.y / content_size.y.max(1.0));
    let total = (cover * factor).max(contain).max(f32::EPSILON);
    let visible = egui::vec2(
        (view.x / total).min(content_size.x),
        (view.y / total).min(content_size.y),
    );
    let center = egui::vec2(
        cursor.x.clamp(
            visible.x * 0.5,
            (content_size.x - visible.x * 0.5).max(visible.x * 0.5),
        ),
        cursor.y.clamp(
            visible.y * 0.5,
            (content_size.y - visible.y * 0.5).max(visible.y * 0.5),
        ),
    );
    (visible, center)
}

pub(crate) fn z_cover_zoom_pan(
    view: egui::Vec2,
    display_size: egui::Vec2,
    content_min: egui::Vec2,
    content_size: egui::Vec2,
    factor: f32,
    cursor: egui::Vec2,
) -> (f32, egui::Vec2) {
    if !size_is_valid(view) || !size_is_valid(display_size) || !size_is_valid(content_size) {
        return (1.0, egui::Vec2::ZERO);
    }
    let page_fit = (view.x / display_size.x).min(view.y / display_size.y);
    let cover = (view.x / content_size.x).max(view.y / content_size.y);
    let contain = (view.x / content_size.x).min(view.y / content_size.y);
    let total = (cover * factor).max(contain);
    let zoom = if page_fit > 0.0 {
        total / page_fit
    } else {
        1.0
    };
    let (_, center) = z_visible_source(view, content_size, factor, cursor - content_min);
    let pan = (display_size * 0.5 - (content_min + center)) * total;
    (zoom, pan)
}

pub(crate) fn z_bbox_zoom_pan_from_full(
    zoom_full: f32,
    pan_full: egui::Vec2,
    view: egui::Vec2,
    display_size: egui::Vec2,
    content_min: egui::Vec2,
    content_size: egui::Vec2,
) -> (f32, egui::Vec2) {
    let page_fit = (view.x / display_size.x.max(1.0)).min(view.y / display_size.y.max(1.0));
    let contain = (view.x / content_size.x.max(1.0)).min(view.y / content_size.y.max(1.0));
    let total = zoom_full * page_fit;
    let zoom = if contain > 0.0 {
        total / contain
    } else {
        zoom_full
    };
    let pan = pan_full + (content_min + content_size * 0.5 - display_size * 0.5) * total;
    (zoom, pan)
}

pub(crate) fn z_aim_frame_rect(
    view_rect: egui::Rect,
    content_min: egui::Vec2,
    content_size: egui::Vec2,
    factor: f32,
    cursor: egui::Vec2,
) -> egui::Rect {
    if !size_is_valid(content_size) {
        return view_rect;
    }
    let fit = (view_rect.width() / content_size.x).min(view_rect.height() / content_size.y);
    let content_rect = egui::Rect::from_center_size(view_rect.center(), content_size * fit);
    let (visible, center) =
        z_visible_source(view_rect.size(), content_size, factor, cursor - content_min);
    egui::Rect::from_min_size(
        egui::pos2(
            content_rect.left() + (center.x - visible.x * 0.5) * fit,
            content_rect.top() + (center.y - visible.y * 0.5) * fit,
        ),
        visible * fit,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(
        fit_mode: FullscreenFitMode,
        rotation: Rotation,
        content_bbox: Option<egui::Rect>,
    ) -> DisplayedImageTransformInput {
        DisplayedImageTransformInput {
            page_idx: 7,
            viewport_rect: egui::Rect::from_min_size(
                egui::pos2(20.0, 30.0),
                egui::vec2(900.0, 700.0),
            ),
            source_size: egui::vec2(1200.0, 800.0),
            texture_size: egui::vec2(2400.0, 1600.0),
            rotation,
            free_rotation_rad: 0.0,
            content_bbox,
            fit_mode,
            fit_scale_limits: FullscreenFitScaleLimits::default(),
            placement: ResolvedDisplayPlacement::Normal {
                zoom_pan: Some((1.35, egui::vec2(23.0, -17.0))),
            },
        }
    }

    fn close(a: f32, b: f32) {
        assert!((a - b).abs() < 0.01);
    }

    fn rect_close(a: egui::Rect, b: egui::Rect) {
        close(a.left(), b.left());
        close(a.top(), b.top());
        close(a.right(), b.right());
        close(a.bottom(), b.bottom());
    }

    fn page_transform(page_idx: usize, rect: egui::Rect) -> DisplayedImageTransform {
        DisplayedImageTransform::from_resolved_rect(
            DisplayedImageTransformInput {
                page_idx,
                viewport_rect: egui::Rect::from_min_max(
                    egui::pos2(-1000.0, -1000.0),
                    egui::pos2(1000.0, 1000.0),
                ),
                source_size: rect.size(),
                texture_size: rect.size(),
                rotation: Rotation::None,
                free_rotation_rad: 0.0,
                content_bbox: None,
                fit_mode: FullscreenFitMode::Page,
                fit_scale_limits: FullscreenFitScaleLimits::default(),
                placement: ResolvedDisplayPlacement::Normal { zoom_pan: None },
            },
            rect,
        )
        .unwrap()
    }

    fn page_layout(
        kind: FullscreenPageLayoutKind,
        pages: &[(usize, egui::Rect)],
    ) -> FullscreenPageLayout {
        let mut layout = FullscreenPageLayout::default();
        layout.begin(kind);
        for &(idx, rect) in pages {
            layout.push(page_transform(idx, rect));
        }
        layout
    }

    #[test]
    fn single_page_layout_hits_only_the_displayed_page() {
        let rect = egui::Rect::from_min_max(egui::pos2(20.0, 30.0), egui::pos2(220.0, 330.0));
        let layout = page_layout(FullscreenPageLayoutKind::Single, &[(7, rect)]);

        assert_eq!(
            layout.hit_test(rect.center()).map(|page| page.page_idx()),
            Some(7)
        );
        assert!(layout.hit_test(egui::pos2(10.0, 10.0)).is_none());
    }

    #[test]
    fn continuous_layout_hits_cursor_page_in_vertical_and_horizontal_flows() {
        let vertical = page_layout(
            FullscreenPageLayoutKind::Continuous,
            &[
                (
                    10,
                    egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(80.0, 90.0)),
                ),
                (
                    11,
                    egui::Rect::from_min_max(egui::pos2(0.0, 110.0), egui::pos2(80.0, 200.0)),
                ),
            ],
        );
        assert_eq!(
            vertical
                .hit_test(egui::pos2(40.0, 45.0))
                .map(|p| p.page_idx()),
            Some(10)
        );
        assert_eq!(
            vertical
                .hit_test(egui::pos2(40.0, 155.0))
                .map(|p| p.page_idx()),
            Some(11)
        );

        let horizontal = page_layout(
            FullscreenPageLayoutKind::Continuous,
            &[
                (
                    20,
                    egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(90.0, 80.0)),
                ),
                (
                    21,
                    egui::Rect::from_min_max(egui::pos2(110.0, 0.0), egui::pos2(200.0, 80.0)),
                ),
            ],
        );
        assert_eq!(
            horizontal
                .hit_test(egui::pos2(45.0, 40.0))
                .map(|p| p.page_idx()),
            Some(20)
        );
        assert_eq!(
            horizontal
                .hit_test(egui::pos2(155.0, 40.0))
                .map(|p| p.page_idx()),
            Some(21)
        );
    }

    #[test]
    fn spread_layout_hits_physical_left_and_right_pages_for_ltr_and_rtl() {
        let left = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(95.0, 100.0));
        let right = egui::Rect::from_min_max(egui::pos2(105.0, 0.0), egui::pos2(200.0, 100.0));
        for (left_idx, right_idx) in [(30, 31), (31, 30)] {
            let layout = page_layout(
                FullscreenPageLayoutKind::Spread,
                &[(left_idx, left), (right_idx, right)],
            );
            assert_eq!(
                layout.hit_test(left.center()).map(|p| p.page_idx()),
                Some(left_idx)
            );
            assert_eq!(
                layout.hit_test(right.center()).map(|p| p.page_idx()),
                Some(right_idx)
            );
            assert_eq!(layout.spread_pair(), Some((left_idx, right_idx)));
        }
    }

    #[test]
    fn page_gap_is_not_hit_and_zero_gap_boundary_uses_first_page() {
        let left = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(90.0, 100.0));
        let right = egui::Rect::from_min_max(egui::pos2(110.0, 0.0), egui::pos2(200.0, 100.0));
        let with_gap = page_layout(FullscreenPageLayoutKind::Spread, &[(1, left), (2, right)]);
        assert!(with_gap.hit_test(egui::pos2(100.0, 50.0)).is_none());

        let touching_right =
            egui::Rect::from_min_max(egui::pos2(90.0, 0.0), egui::pos2(180.0, 100.0));
        let touching = page_layout(
            FullscreenPageLayoutKind::Spread,
            &[(1, left), (2, touching_right)],
        );
        assert_eq!(
            touching
                .hit_test(egui::pos2(90.0, 50.0))
                .map(|p| p.page_idx()),
            Some(1)
        );
    }

    #[test]
    fn screen_source_screen_round_trips_all_rotations() {
        for rotation in [
            Rotation::None,
            Rotation::Cw90,
            Rotation::Cw180,
            Rotation::Cw270,
        ] {
            for free_rotation_rad in [0.0, 0.31, -0.57] {
                let mut value = input(FullscreenFitMode::Page, rotation, None);
                value.free_rotation_rad = free_rotation_rad;
                let transform = DisplayedImageTransform::resolve(value).unwrap();
                for source in [
                    egui::pos2(0.0, 0.0),
                    egui::pos2(1200.0, 800.0),
                    egui::pos2(317.0, 629.0),
                    egui::pos2(1199.0, 1.0),
                ] {
                    let screen = transform.source_to_screen(source);
                    let restored = transform.screen_to_source(screen);
                    close(source.x, restored.x);
                    close(source.y, restored.y);
                }
            }
        }
    }

    #[test]
    fn fit_rotation_and_trim_share_paint_and_hit_geometry() {
        let trim = egui::Rect::from_min_max(egui::pos2(0.15, 0.20), egui::pos2(0.85, 0.75));
        for fit_mode in [
            FullscreenFitMode::Page,
            FullscreenFitMode::MarginFit,
            FullscreenFitMode::Width,
            FullscreenFitMode::Height,
            FullscreenFitMode::Original,
        ] {
            for rotation in [
                Rotation::None,
                Rotation::Cw90,
                Rotation::Cw180,
                Rotation::Cw270,
            ] {
                for content_bbox in [None, Some(trim)] {
                    let transform =
                        DisplayedImageTransform::resolve(input(fit_mode, rotation, content_bbox))
                            .unwrap();
                    rect_close(
                        transform.hit_rect,
                        transform.paint_rect.intersect(transform.viewport_rect),
                    );
                    let expected_uv = if rotation == Rotation::None {
                        content_bbox.unwrap_or_else(full_uv_rect)
                    } else {
                        full_uv_rect()
                    };
                    rect_close(transform.uv_rect, expected_uv);
                    let center_source =
                        transform.screen_to_source_normalized(transform.paint_rect.center());
                    close(center_source.x, expected_uv.center().x);
                    close(center_source.y, expected_uv.center().y);
                }
            }
        }
    }

    #[test]
    fn z_transform_is_the_geometry_used_for_inverse_mapping() {
        let base = input(FullscreenFitMode::Page, Rotation::None, None);
        let resolved = ResolvedZTransform::resolve(ZTransformInput {
            pan_band: base.viewport_rect,
            cursor: egui::pos2(790.0, 280.0),
            factor: 2.4,
            active: true,
            image: base,
        })
        .unwrap();
        let transform = resolved.transform;
        assert!(matches!(
            transform.placement,
            ResolvedDisplayPlacement::Z { active: true, factor, .. }
                if (factor - resolved.factor).abs() < EPSILON
        ));
        let source = egui::pos2(930.0, 260.0);
        let screen = transform.source_to_screen(source);
        let loupe_source = transform.screen_to_source(screen);
        close(source.x, loupe_source.x);
        close(source.y, loupe_source.y);
    }

    #[test]
    fn non_page_fit_and_trim_editor_mapping_matches_paint_transform() {
        let trim = egui::Rect::from_min_max(egui::pos2(0.20, 0.10), egui::pos2(0.80, 0.90));
        for fit_mode in [
            FullscreenFitMode::Width,
            FullscreenFitMode::Height,
            FullscreenFitMode::Original,
            FullscreenFitMode::MarginFit,
        ] {
            let transform =
                DisplayedImageTransform::resolve(input(fit_mode, Rotation::None, Some(trim)))
                    .unwrap();
            for source_norm in [trim.min, trim.center(), trim.max, egui::pos2(0.37, 0.64)] {
                let screen = transform.source_normalized_to_screen(source_norm);
                let editor_norm = transform.screen_to_source_normalized(screen);
                close(source_norm.x, editor_norm.x);
                close(source_norm.y, editor_norm.y);
            }
            assert!(transform.contains_screen(transform.paint_rect.center()));
        }
    }

    #[test]
    fn scale_limits_are_part_of_the_resolved_geometry() {
        let mut no_up = input(FullscreenFitMode::Page, Rotation::None, None);
        no_up.viewport_rect =
            egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(5000.0, 4000.0));
        no_up.fit_scale_limits.no_upscale = true;
        no_up.placement = ResolvedDisplayPlacement::Normal { zoom_pan: None };
        let no_up = DisplayedImageTransform::resolve(no_up).unwrap();
        close(no_up.total_scale, 1.0);

        let mut no_down = input(FullscreenFitMode::Page, Rotation::None, None);
        no_down.viewport_rect =
            egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(300.0, 200.0));
        no_down.fit_scale_limits.no_downscale = true;
        no_down.placement = ResolvedDisplayPlacement::Normal { zoom_pan: None };
        let no_down = DisplayedImageTransform::resolve(no_down).unwrap();
        close(no_down.total_scale, 1.0);
    }
}
