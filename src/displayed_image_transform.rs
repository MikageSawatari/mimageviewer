use eframe::egui;

use crate::rotation_db::Rotation;
use crate::settings::FullscreenFitMode;

const EPSILON: f32 = 1.0e-6;
const PHYSICAL_SCALE_INTEGER_EPSILON: f32 = 1.0e-4;
/// A cursor mapping span smaller than one logical point cannot provide a stable pair of distinct
/// pointer positions. Fall back to the full pan band before either axis becomes sub-point thin.
const MIN_Z_AIM_MAPPING_SPAN_POINTS: f32 = 1.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct FullscreenFitScaleLimits {
    pub(crate) no_upscale: bool,
    pub(crate) no_downscale: bool,
    pub(crate) pixels_per_point: f32,
}

impl Default for FullscreenFitScaleLimits {
    fn default() -> Self {
        Self {
            no_upscale: false,
            no_downscale: false,
            pixels_per_point: 1.0,
        }
    }
}

impl FullscreenFitScaleLimits {
    pub(crate) fn active(self) -> bool {
        self.no_upscale || self.no_downscale
    }

    pub(crate) fn apply(self, mut scale: f32) -> f32 {
        let original_scale = physical_pixel_scale(self.pixels_per_point);
        if self.no_upscale {
            scale = scale.min(original_scale);
        }
        if self.no_downscale {
            scale = scale.max(original_scale);
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
    pub(crate) pixels_per_point: f32,
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
        let fit_bbox = effective_bbox(input.free_rotation_rad, input.content_bbox)
            .map(|bbox| rotate_bbox_to_display(bbox, input.rotation));
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
            (FullscreenFitMode::Original, Some((_, _, center))) => {
                (physical_pixel_scale(input.pixels_per_point), center)
            }
            (FullscreenFitMode::Original, None) => (
                physical_pixel_scale(input.pixels_per_point),
                egui::Vec2::ZERO,
            ),
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
        let full_image_rect = snap_rect_origin_to_physical_pixel(
            full_image_rect,
            total_scale,
            input.pixels_per_point,
        );
        // 描画位置は表示空間、UV は元画像空間。**同じ矩形を両方に使わない**のが、
        // 回転時に部分矩形を丸ごと捨てていた原因だった。
        let (paint_rect, uv_rect) = effective_bbox(input.free_rotation_rad, input.content_bbox)
            .map(|source_bbox| {
                let display_bbox = rotate_bbox_to_display(source_bbox, input.rotation);
                (
                    normalized_sub_rect(full_image_rect, display_bbox),
                    source_bbox,
                )
            })
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

    /// Returns the shared fullscreen pan that places `source` at the viewport center.
    ///
    /// The resolved transform already owns every fit, trim, and rotation decision. Changing
    /// pan only translates that resolved geometry, so the inverse is the current pan plus the
    /// screen-space delta from the source point to the viewport center.
    pub(crate) fn pan_to_center_source_normalized(
        &self,
        source: egui::Pos2,
        current_pan: egui::Vec2,
    ) -> egui::Vec2 {
        current_pan + (self.viewport_rect.center() - self.source_normalized_to_screen(source))
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

    /// Returns the source UV rectangle that is both inside the active display trim
    /// and visible through the current viewport/paint clip. Free rotation can make
    /// the exact visible source shape non-rectangular, so the smallest axis-aligned
    /// source rectangle containing that polygon is returned.
    pub(crate) fn visible_source_uv_rect(&self, clip_rect: egui::Rect) -> Option<egui::Rect> {
        let clip_rect = self.viewport_rect.intersect(clip_rect);
        if !rect_is_valid(clip_rect) {
            return None;
        }
        let source_corners = [
            self.uv_rect.left_top(),
            self.uv_rect.right_top(),
            self.uv_rect.right_bottom(),
            self.uv_rect.left_bottom(),
        ];
        let mut visible_polygon = source_corners
            .into_iter()
            .map(|source| self.source_normalized_to_screen(source))
            .collect::<Vec<_>>();
        for edge in 0..4 {
            visible_polygon = clip_polygon_to_rect_edge(&visible_polygon, clip_rect, edge);
            if visible_polygon.is_empty() {
                return None;
            }
        }

        let mut min = egui::pos2(f32::INFINITY, f32::INFINITY);
        let mut max = egui::pos2(f32::NEG_INFINITY, f32::NEG_INFINITY);
        for screen in visible_polygon {
            let source = self.screen_to_source_normalized(screen);
            min.x = min.x.min(source.x);
            min.y = min.y.min(source.y);
            max.x = max.x.max(source.x);
            max.y = max.y.max(source.y);
        }
        let visible = egui::Rect::from_min_max(min, max).intersect(self.uv_rect);
        rect_is_valid(visible).then_some(visible)
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

    /// Paint a texture representing only `source_uv_rect` at the matching place
    /// in this transform. This is used by visible-region upscale outputs; the
    /// normal full-image/downscale path continues to use `paint_texture`.
    pub(crate) fn paint_texture_source_region(
        &self,
        painter: &egui::Painter,
        texture_id: egui::TextureId,
        source_uv_rect: egui::Rect,
        tint: egui::Color32,
    ) {
        paint_source_region_texture(
            painter,
            texture_id,
            self.full_image_rect,
            self.rotation,
            self.free_rotation_rad,
            source_uv_rect,
            full_uv_rect(),
            tint,
        );
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn paint_source_region_texture(
    painter: &egui::Painter,
    texture_id: egui::TextureId,
    full_image_rect: egui::Rect,
    rotation: Rotation,
    free_rotation_rad: f32,
    source_uv_rect: egui::Rect,
    texture_uv_rect: egui::Rect,
    tint: egui::Color32,
) {
    let source_uvs = [
        source_uv_rect.left_top(),
        source_uv_rect.right_top(),
        source_uv_rect.right_bottom(),
        source_uv_rect.left_bottom(),
    ];
    let texture_uvs = [
        texture_uv_rect.left_top(),
        texture_uv_rect.right_top(),
        texture_uv_rect.right_bottom(),
        texture_uv_rect.left_bottom(),
    ];
    let mut mesh = egui::Mesh::with_texture(texture_id);
    for (source, texture_uv) in source_uvs.into_iter().zip(texture_uvs) {
        let (u, v) = forward_uv(rotation, source.x, source.y);
        let position = egui::pos2(
            full_image_rect.left() + u * full_image_rect.width(),
            full_image_rect.top() + v * full_image_rect.height(),
        );
        mesh.vertices.push(egui::epaint::Vertex {
            pos: rotate_about(position, full_image_rect.center(), free_rotation_rad),
            uv: texture_uv,
            color: tint,
        });
    }
    mesh.indices.extend_from_slice(&[0, 1, 2, 0, 2, 3]);
    painter.add(egui::Shape::mesh(mesh));
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

    /// `pos` を含むページ。無ければ、`pos` を中心とする `window_size` 四方の窓と重なる
    /// ページのうち `pos` にいちばん近いものを返す。
    ///
    /// ルーペのように「カーソルが画像の外へ出ても、拡大窓に画像が残っている間は対象を
    /// 保ちたい」処理のための拡張。窓と重ならなくなれば `None` を返すので、拡大するものが
    /// 無い状態で対象を持ち続けることはない。`window_size` が 0 なら `hit_test` と同じ。
    pub(crate) fn hit_test_or_nearest_in_window(
        &self,
        pos: egui::Pos2,
        window_size: f32,
    ) -> Option<&DisplayedPage> {
        if let Some(page) = self.hit_test(pos) {
            return Some(page);
        }
        let window = egui::Rect::from_center_size(pos, egui::Vec2::splat(window_size.max(0.0)));
        self.pages
            .iter()
            .filter_map(|page| {
                let rect = page
                    .transform
                    .hit_rect
                    .intersect(page.transform.viewport_rect);
                (rect.is_positive() && rect.intersects(window))
                    .then(|| (page, rect.distance_sq_to_pos(pos)))
            })
            .min_by(|(_, a), (_, b)| a.total_cmp(b))
            .map(|(page, _)| page)
    }

    pub(crate) fn page_by_idx(&self, page_idx: usize) -> Option<&DisplayedPage> {
        self.pages.iter().find(|page| page.page_idx() == page_idx)
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

/// Screen-space basis shared by Z cursor mapping and aim-frame drawing.
///
/// Resolve this once per Z transform with the caller's actual draw scale. Keeping that content
/// rect and its cursor-mapping intersection in one value prevents the two consumers from deriving
/// different geometry; `content_fit` may exceed contain for spread Width/Height/Original modes.
#[derive(Clone, Copy, Debug)]
pub(crate) struct ZAimBasis {
    view_rect: egui::Rect,
    content_rect: egui::Rect,
    cursor_mapping_rect: egui::Rect,
    content_fit: f32,
}

impl ZAimBasis {
    pub(crate) fn resolve(
        view_rect: egui::Rect,
        pan_band: egui::Rect,
        content_size: egui::Vec2,
        content_fit: f32,
    ) -> Self {
        if !rect_is_valid(view_rect)
            || !size_is_valid(content_size)
            || !content_fit.is_finite()
            || content_fit <= 0.0
        {
            return Self {
                view_rect,
                content_rect: view_rect,
                cursor_mapping_rect: pan_band,
                content_fit: 1.0,
            };
        }

        let content_half_size = content_size * (content_fit * 0.5);
        let content_rect = egui::Rect::from_min_max(
            view_rect.center() - content_half_size,
            view_rect.center() + content_half_size,
        );
        let intersection = content_rect.intersect(pan_band);
        let cursor_mapping_rect = if rect_is_valid(intersection)
            && intersection.width() >= MIN_Z_AIM_MAPPING_SPAN_POINTS
            && intersection.height() >= MIN_Z_AIM_MAPPING_SPAN_POINTS
        {
            intersection
        } else {
            pan_band
        };

        Self {
            view_rect,
            content_rect,
            cursor_mapping_rect,
            content_fit,
        }
    }

    pub(crate) fn content_rect(self) -> egui::Rect {
        self.content_rect
    }

    pub(crate) fn content_fit(self) -> f32 {
        self.content_fit
    }

    #[cfg(test)]
    pub(crate) fn cursor_mapping_rect(self) -> egui::Rect {
        self.cursor_mapping_rect
    }
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
        let bbox = effective_bbox(0.0, input.image.content_bbox)
            .map(|bbox| rotate_bbox_to_display(bbox, input.image.rotation));
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
        let aim_basis = ZAimBasis::resolve(
            input.image.viewport_rect,
            input.pan_band,
            content_size,
            contain,
        );
        let cursor_image = z_cursor_image_px(&aim_basis, content_min, content_size, input.cursor);
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
                    &aim_basis,
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

pub(crate) fn physical_pixel_scale(pixels_per_point: f32) -> f32 {
    1.0 / normalized_pixels_per_point(pixels_per_point)
}

pub(crate) fn quantize_points_to_physical_pixels(points: f32, pixels_per_point: f32) -> f32 {
    let pixels_per_point = normalized_pixels_per_point(pixels_per_point);
    (points * pixels_per_point).round() / pixels_per_point
}

pub(crate) fn physical_scale_is_near_integer(logical_scale: f32, pixels_per_point: f32) -> bool {
    let physical_scale = logical_scale * normalized_pixels_per_point(pixels_per_point);
    if !physical_scale.is_finite() || physical_scale <= 0.0 {
        return false;
    }
    let nearest = physical_scale.round();
    nearest >= 1.0
        && (physical_scale - nearest).abs() <= PHYSICAL_SCALE_INTEGER_EPSILON * nearest.max(1.0)
}

fn normalized_pixels_per_point(pixels_per_point: f32) -> f32 {
    if pixels_per_point.is_finite() && pixels_per_point > 0.0 {
        pixels_per_point
    } else {
        1.0
    }
}

fn snap_rect_origin_to_physical_pixel(
    rect: egui::Rect,
    logical_scale: f32,
    pixels_per_point: f32,
) -> egui::Rect {
    if !physical_scale_is_near_integer(logical_scale, pixels_per_point) {
        return rect;
    }
    let snapped_min = egui::pos2(
        quantize_points_to_physical_pixels(rect.min.x, pixels_per_point),
        quantize_points_to_physical_pixels(rect.min.y, pixels_per_point),
    );
    egui::Rect::from_min_size(snapped_min, rect.size())
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

fn clip_polygon_to_rect_edge(
    polygon: &[egui::Pos2],
    clip: egui::Rect,
    edge: usize,
) -> Vec<egui::Pos2> {
    if polygon.is_empty() {
        return Vec::new();
    }
    let inside = |point: egui::Pos2| match edge {
        0 => point.x >= clip.left(),
        1 => point.x <= clip.right(),
        2 => point.y >= clip.top(),
        _ => point.y <= clip.bottom(),
    };
    let intersection = |start: egui::Pos2, end: egui::Pos2| {
        let delta = end - start;
        let t = match edge {
            0 => (clip.left() - start.x) / delta.x,
            1 => (clip.right() - start.x) / delta.x,
            2 => (clip.top() - start.y) / delta.y,
            _ => (clip.bottom() - start.y) / delta.y,
        }
        .clamp(0.0, 1.0);
        start + delta * t
    };

    let mut output = Vec::with_capacity(polygon.len() + 2);
    let mut previous = *polygon.last().unwrap();
    let mut previous_inside = inside(previous);
    for &current in polygon {
        let current_inside = inside(current);
        if current_inside {
            if !previous_inside {
                output.push(intersection(previous, current));
            }
            output.push(current);
        } else if previous_inside {
            output.push(intersection(previous, current));
        }
        previous = current;
        previous_inside = current_inside;
    }
    output
}

fn rotated_size(size: egui::Vec2, rotation: Rotation) -> egui::Vec2 {
    match rotation {
        Rotation::Cw90 | Rotation::Cw270 => egui::vec2(size.y, size.x),
        Rotation::None | Rotation::Cw180 => size,
    }
}

/// 使える部分矩形か。**自由回転中だけ降ろす。**
///
/// 保存回転では降ろさない。以前は回転していれば無条件に捨てていたが、それは
/// `content_bbox` を fit 計算では表示空間、UV では元画像空間として二重に扱っていた
/// ためで、矩形自体は回転しても使える。空間を分けたので保存回転は扱える
/// ([rotate_bbox_to_display])。自由回転は、傾けた矩形の外接が広がる分の拡大量を
/// 解けないので従来どおり降ろす。
fn effective_bbox(free_rotation_rad: f32, content_bbox: Option<egui::Rect>) -> Option<egui::Rect> {
    content_bbox.filter(|_| free_rotation_rad.abs() <= EPSILON)
}

/// 元画像空間の正規化部分矩形を、保存回転を反映した表示空間へ写す。
///
/// `content_bbox` は**元画像の画素から作られる** (`margin_fit::detect_content_bbox` は
/// 回転を知らない)。一方 `full_image_rect` と `display_size` は回転後の寸法なので、
/// 描画位置と fit にはこちらを使う。UV は元画像空間のまま渡す。
/// 写像は screen ↔ source と同じ [`forward_uv`] を使う (別の式を書かない)。
fn rotate_bbox_to_display(bbox: egui::Rect, rotation: Rotation) -> egui::Rect {
    let (ax, ay) = forward_uv(rotation, bbox.min.x, bbox.min.y);
    let (bx, by) = forward_uv(rotation, bbox.max.x, bbox.max.y);
    egui::Rect::from_min_max(
        egui::pos2(ax.min(bx), ay.min(by)),
        egui::pos2(ax.max(bx), ay.max(by)),
    )
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

#[cfg(test)]
mod bbox_space_tests {
    use super::*;

    fn rect(x0: f32, y0: f32, x1: f32, y1: f32) -> egui::Rect {
        egui::Rect::from_min_max(egui::pos2(x0, y0), egui::pos2(x1, y1))
    }

    fn about(a: egui::Rect, b: egui::Rect) {
        for (l, r) in [
            (a.min.x, b.min.x),
            (a.min.y, b.min.y),
            (a.max.x, b.max.x),
            (a.max.y, b.max.y),
        ] {
            assert!((l - r).abs() < 1e-5, "{a:?} != {b:?}");
        }
    }

    /// 元画像の左上寄りの矩形が、回転ごとに表示空間のどこへ行くか。
    #[test]
    fn a_source_rect_lands_where_the_rotation_puts_it() {
        let source = rect(0.0, 0.0, 0.25, 0.5);
        about(
            rotate_bbox_to_display(source, Rotation::None),
            rect(0.0, 0.0, 0.25, 0.5),
        );
        // 時計回り 90 度: 上辺が右辺へ。元の「上寄り」は表示では「右寄り」。
        about(
            rotate_bbox_to_display(source, Rotation::Cw90),
            rect(0.5, 0.0, 1.0, 0.25),
        );
        about(
            rotate_bbox_to_display(source, Rotation::Cw180),
            rect(0.75, 0.5, 1.0, 1.0),
        );
        about(
            rotate_bbox_to_display(source, Rotation::Cw270),
            rect(0.0, 0.75, 0.5, 1.0),
        );
    }

    /// 90 / 270 度では縦横が入れ替わる。入れ替えを忘れると fit が縦横取り違える。
    #[test]
    fn the_quarter_turns_swap_width_and_height() {
        let source = rect(0.1, 0.2, 0.4, 0.9);
        for rotation in [Rotation::Cw90, Rotation::Cw270] {
            let display = rotate_bbox_to_display(source, rotation);
            assert!((display.width() - source.height()).abs() < 1e-5);
            assert!((display.height() - source.width()).abs() < 1e-5);
        }
        for rotation in [Rotation::None, Rotation::Cw180] {
            let display = rotate_bbox_to_display(source, rotation);
            assert!((display.width() - source.width()).abs() < 1e-5);
            assert!((display.height() - source.height()).abs() < 1e-5);
        }
    }

    /// 表示空間へ写した矩形の中心を戻すと、元の中心に一致する。
    /// `screen_to_source_normalized` と同じ `inverse_uv` を通る経路の裏取り。
    #[test]
    fn mapping_back_returns_the_source_centre() {
        let source = rect(0.2, 0.05, 0.6, 0.35);
        for rotation in [
            Rotation::None,
            Rotation::Cw90,
            Rotation::Cw180,
            Rotation::Cw270,
        ] {
            let display = rotate_bbox_to_display(source, rotation);
            let (s, t) = inverse_uv(rotation, display.center().x, display.center().y);
            assert!((s - source.center().x).abs() < 1e-5, "{rotation:?}");
            assert!((t - source.center().y).abs() < 1e-5, "{rotation:?}");
        }
    }

    /// 部分矩形が降りるのは自由回転中だけ。保存回転では残る。
    #[test]
    fn only_free_rotation_drops_the_bbox() {
        let bbox = Some(rect(0.1, 0.1, 0.9, 0.9));
        assert_eq!(effective_bbox(0.0, bbox), bbox);
        assert_eq!(effective_bbox(0.2, bbox), None);
        assert_eq!(effective_bbox(0.0, None), None);
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

/// Map a screen cursor through the content-rect/pan-band intersection resolved in `ZAimBasis`.
/// The same basis must be passed to `z_aim_frame_rect` when drawing the corresponding frame.
pub(crate) fn z_cursor_image_px(
    basis: &ZAimBasis,
    content_min: egui::Vec2,
    content_size: egui::Vec2,
    cursor: egui::Pos2,
) -> egui::Vec2 {
    let band = basis.cursor_mapping_rect;
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
    basis: &ZAimBasis,
    content_min: egui::Vec2,
    content_size: egui::Vec2,
    factor: f32,
    cursor: egui::Vec2,
) -> egui::Rect {
    if !size_is_valid(content_size) {
        return basis.content_rect;
    }
    let content_rect = basis.content_rect;
    let fit = basis.content_fit;
    let (visible, center) = z_visible_source(
        basis.view_rect.size(),
        content_size,
        factor,
        cursor - content_min,
    );
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
            pixels_per_point: 1.0,
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
                pixels_per_point: 1.0,
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

    /// ルーペは端の画素を見るためにカーソルを画像の外へ少し出す使い方をする。拡大窓に
    /// 画像が残っている間は対象ページを保ち、何も拡大するものが無くなったら手放す。
    #[test]
    fn nearest_page_lookup_keeps_the_page_while_the_window_still_covers_it() {
        let rect = egui::Rect::from_min_max(egui::pos2(100.0, 100.0), egui::pos2(300.0, 400.0));
        let layout = page_layout(FullscreenPageLayoutKind::Single, &[(7, rect)]);
        let window = 100.0;

        let inside = layout
            .hit_test_or_nearest_in_window(rect.center(), window)
            .map(|page| page.page_idx());
        assert_eq!(inside, Some(7));

        // 左へ 30pt はみ出した位置。窓 (半幅 50pt) はまだ画像に届いている。
        let just_outside = layout
            .hit_test_or_nearest_in_window(egui::pos2(70.0, 250.0), window)
            .map(|page| page.page_idx());
        assert_eq!(just_outside, Some(7));

        // 半幅を超えて離れると拡大するものが無いので手放す。
        assert!(
            layout
                .hit_test_or_nearest_in_window(egui::pos2(40.0, 250.0), window)
                .is_none()
        );

        // 窓を 0 にすれば従来の hit_test と同じ。
        assert!(
            layout
                .hit_test_or_nearest_in_window(egui::pos2(70.0, 250.0), 0.0)
                .is_none()
        );
    }

    #[test]
    fn nearest_page_lookup_picks_the_closer_side_of_a_spread() {
        let left = egui::Rect::from_min_max(egui::pos2(0.0, 100.0), egui::pos2(200.0, 400.0));
        let right = egui::Rect::from_min_max(egui::pos2(260.0, 100.0), egui::pos2(460.0, 400.0));
        let layout = page_layout(FullscreenPageLayoutKind::Spread, &[(3, left), (4, right)]);

        // 左右どちらの窓にも入る谷間で、近い側を選ぶ。
        assert_eq!(
            layout
                .hit_test_or_nearest_in_window(egui::pos2(215.0, 250.0), 100.0)
                .map(|page| page.page_idx()),
            Some(3)
        );
        assert_eq!(
            layout
                .hit_test_or_nearest_in_window(egui::pos2(245.0, 250.0), 100.0)
                .map(|page| page.page_idx()),
            Some(4)
        );
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
        assert_eq!(vertical.page_by_idx(11).map(|p| p.page_idx()), Some(11));
        assert!(vertical.page_by_idx(99).is_none());

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
                    // 部分矩形は元画像空間なので、**回転しても同じものが UV になる**。
                    // 以前はここで回転時だけ全体へ倒していたが、それは fit 計算と UV で
                    // 同じ矩形を別の空間として使っていたことの辻褄合わせだった。
                    let expected_uv = content_bbox.unwrap_or_else(full_uv_rect);
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
    fn visible_source_rect_intersects_trim_and_viewport() {
        let trim = egui::Rect::from_min_max(egui::pos2(0.20, 0.10), egui::pos2(0.90, 0.80));
        let transform = DisplayedImageTransform::from_resolved_rect(
            DisplayedImageTransformInput {
                page_idx: 7,
                viewport_rect: egui::Rect::from_min_max(
                    egui::pos2(25.0, 10.0),
                    egui::pos2(75.0, 90.0),
                ),
                source_size: egui::vec2(100.0, 100.0),
                texture_size: egui::vec2(100.0, 100.0),
                rotation: Rotation::None,
                free_rotation_rad: 0.0,
                content_bbox: Some(trim),
                fit_mode: FullscreenFitMode::Page,
                fit_scale_limits: FullscreenFitScaleLimits::default(),
                pixels_per_point: 1.0,
                placement: ResolvedDisplayPlacement::Normal { zoom_pan: None },
            },
            egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(100.0, 100.0)),
        )
        .unwrap();

        let visible = transform
            .visible_source_uv_rect(egui::Rect::from_min_max(
                egui::pos2(0.0, 20.0),
                egui::pos2(60.0, 70.0),
            ))
            .unwrap();
        rect_close(
            visible,
            egui::Rect::from_min_max(egui::pos2(0.25, 0.20), egui::pos2(0.60, 0.70)),
        );
    }

    #[test]
    fn visible_source_rect_tracks_orthogonal_rotation() {
        let transform = DisplayedImageTransform::from_resolved_rect(
            DisplayedImageTransformInput {
                page_idx: 7,
                viewport_rect: egui::Rect::from_min_max(
                    egui::pos2(0.0, 0.0),
                    egui::pos2(100.0, 200.0),
                ),
                source_size: egui::vec2(200.0, 100.0),
                texture_size: egui::vec2(200.0, 100.0),
                rotation: Rotation::Cw90,
                free_rotation_rad: 0.0,
                content_bbox: None,
                fit_mode: FullscreenFitMode::Page,
                fit_scale_limits: FullscreenFitScaleLimits::default(),
                pixels_per_point: 1.0,
                placement: ResolvedDisplayPlacement::Normal { zoom_pan: None },
            },
            egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(100.0, 200.0)),
        )
        .unwrap();

        let visible = transform
            .visible_source_uv_rect(egui::Rect::from_min_max(
                egui::pos2(0.0, 50.0),
                egui::pos2(50.0, 150.0),
            ))
            .unwrap();
        rect_close(
            visible,
            egui::Rect::from_min_max(egui::pos2(0.25, 0.50), egui::pos2(0.75, 1.0)),
        );
    }

    fn assert_pan_inverse_round_trip(mut value: DisplayedImageTransformInput, target: egui::Pos2) {
        let start_pan = egui::vec2(81.0, -47.0);
        let zoom = 3.25;
        value.placement = ResolvedDisplayPlacement::Normal {
            zoom_pan: Some((zoom, start_pan)),
        };
        let transform = DisplayedImageTransform::resolve(value).unwrap();
        let centered_pan = transform.pan_to_center_source_normalized(target, start_pan);

        value.placement = ResolvedDisplayPlacement::Normal {
            zoom_pan: Some((zoom, centered_pan)),
        };
        let centered = DisplayedImageTransform::resolve(value).unwrap();
        let visible = centered
            .visible_source_uv_rect(value.viewport_rect)
            .unwrap();
        close(visible.center().x, target.x);
        close(visible.center().y, target.y);
    }

    #[test]
    fn pan_inverse_round_trips_single_rotation_and_trim() {
        assert_pan_inverse_round_trip(
            input(FullscreenFitMode::Page, Rotation::None, None),
            egui::pos2(0.43, 0.58),
        );
        assert_pan_inverse_round_trip(
            input(FullscreenFitMode::Page, Rotation::Cw90, None),
            egui::pos2(0.61, 0.47),
        );

        let trim = egui::Rect::from_min_max(egui::pos2(0.18, 0.12), egui::pos2(0.86, 0.91));
        assert_pan_inverse_round_trip(
            input(FullscreenFitMode::Page, Rotation::None, Some(trim)),
            egui::pos2(0.52, 0.55),
        );
    }

    #[test]
    fn pan_inverse_round_trips_spread_page_with_shared_pan() {
        let viewport = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1000.0, 600.0));
        let left_rect =
            egui::Rect::from_min_max(egui::pos2(-1100.0, -600.0), egui::pos2(400.0, 1200.0));
        let right_rect =
            egui::Rect::from_min_max(egui::pos2(420.0, -600.0), egui::pos2(1920.0, 1200.0));
        let start_pan = egui::vec2(43.0, -29.0);
        let page_input = |page_idx| DisplayedImageTransformInput {
            page_idx,
            viewport_rect: viewport,
            source_size: egui::vec2(1000.0, 1200.0),
            texture_size: egui::vec2(1000.0, 1200.0),
            rotation: Rotation::None,
            free_rotation_rad: 0.0,
            content_bbox: None,
            fit_mode: FullscreenFitMode::Page,
            fit_scale_limits: FullscreenFitScaleLimits::default(),
            pixels_per_point: 1.0,
            placement: ResolvedDisplayPlacement::Normal {
                zoom_pan: Some((3.0, start_pan)),
            },
        };
        let right = DisplayedImageTransform::from_resolved_rect(page_input(2), right_rect).unwrap();
        let target = egui::pos2(0.35, 0.45);
        let centered_pan = right.pan_to_center_source_normalized(target, start_pan);
        let translation = centered_pan - start_pan;

        let mut centered_layout = FullscreenPageLayout::default();
        centered_layout.begin(FullscreenPageLayoutKind::Spread);
        for (page_idx, rect) in [(1, left_rect), (2, right_rect)] {
            centered_layout.push(
                DisplayedImageTransform::from_resolved_rect(
                    page_input(page_idx),
                    rect.translate(translation),
                )
                .unwrap(),
            );
        }
        let visible = centered_layout
            .page_by_idx(2)
            .unwrap()
            .transform
            .visible_source_uv_rect(viewport)
            .unwrap();
        close(visible.center().x, target.x);
        close(visible.center().y, target.y);
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
    fn z_aim_shared_basis_covers_trimmed_and_rotated_content() {
        let trim = egui::Rect::from_min_max(egui::pos2(0.25, 0.0), egui::pos2(0.75, 1.0));
        for (rotation, content_bbox) in [(Rotation::None, Some(trim)), (Rotation::Cw90, None)] {
            let base = input(FullscreenFitMode::Page, rotation, content_bbox);
            let display_size = rotated_size(base.texture_size, rotation);
            let bbox = effective_bbox(0.0, content_bbox)
                .map(|bbox| rotate_bbox_to_display(bbox, rotation));
            let content_size = bbox
                .map(|bbox| {
                    egui::vec2(
                        (bbox.width() * display_size.x).max(1.0),
                        (bbox.height() * display_size.y).max(1.0),
                    )
                })
                .unwrap_or(display_size);
            let pan_band = base.viewport_rect.shrink2(egui::vec2(0.0, 60.0));
            let content_fit = (base.viewport_rect.width() / content_size.x)
                .min(base.viewport_rect.height() / content_size.y);
            let basis = ZAimBasis::resolve(base.viewport_rect, pan_band, content_size, content_fit);
            let cursor = basis.cursor_mapping_rect().left_center();
            let resolved = ResolvedZTransform::resolve(ZTransformInput {
                image: base,
                active: false,
                factor: 2.0,
                cursor,
                pan_band,
            })
            .unwrap();
            let frame = resolved.aim_frame.unwrap();

            close(frame.left(), basis.content_rect().left());
        }
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

    #[test]
    fn original_and_scale_limits_use_physical_pixel_baseline() {
        for pixels_per_point in [1.25, 1.5] {
            let mut original = input(FullscreenFitMode::Original, Rotation::None, None);
            original.pixels_per_point = pixels_per_point;
            original.fit_scale_limits.pixels_per_point = pixels_per_point;
            original.placement = ResolvedDisplayPlacement::Normal { zoom_pan: None };
            let original = DisplayedImageTransform::resolve(original).unwrap();
            close(original.total_scale, 1.0 / pixels_per_point);

            let mut no_up = input(FullscreenFitMode::Page, Rotation::None, None);
            no_up.viewport_rect =
                egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(5000.0, 4000.0));
            no_up.pixels_per_point = pixels_per_point;
            no_up.fit_scale_limits = FullscreenFitScaleLimits {
                no_upscale: true,
                no_downscale: false,
                pixels_per_point,
            };
            no_up.placement = ResolvedDisplayPlacement::Normal { zoom_pan: None };
            let no_up = DisplayedImageTransform::resolve(no_up).unwrap();
            close(no_up.total_scale, 1.0 / pixels_per_point);

            let mut no_down = input(FullscreenFitMode::Page, Rotation::None, None);
            no_down.viewport_rect =
                egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(300.0, 200.0));
            no_down.pixels_per_point = pixels_per_point;
            no_down.fit_scale_limits = FullscreenFitScaleLimits {
                no_upscale: false,
                no_downscale: true,
                pixels_per_point,
            };
            no_down.placement = ResolvedDisplayPlacement::Normal { zoom_pan: None };
            let no_down = DisplayedImageTransform::resolve(no_down).unwrap();
            close(no_down.total_scale, 1.0 / pixels_per_point);
        }
    }

    #[test]
    fn integer_physical_scale_snaps_origin_without_rounding_size() {
        let pixels_per_point = 1.25;
        let texture_size = egui::vec2(101.0, 99.0);
        let transform = DisplayedImageTransform::resolve(DisplayedImageTransformInput {
            page_idx: 0,
            viewport_rect: egui::Rect::from_center_size(
                egui::pos2(100.0, 100.0),
                egui::vec2(200.0, 200.0),
            ),
            source_size: texture_size,
            texture_size,
            rotation: Rotation::None,
            free_rotation_rad: 0.0,
            content_bbox: None,
            fit_mode: FullscreenFitMode::Original,
            fit_scale_limits: FullscreenFitScaleLimits {
                pixels_per_point,
                ..FullscreenFitScaleLimits::default()
            },
            pixels_per_point,
            placement: ResolvedDisplayPlacement::Normal { zoom_pan: None },
        })
        .unwrap();

        close(
            transform.full_image_rect.min.x * pixels_per_point % 1.0,
            0.0,
        );
        close(
            transform.full_image_rect.min.y * pixels_per_point % 1.0,
            0.0,
        );
        close(
            transform.full_image_rect.width() * pixels_per_point,
            texture_size.x,
        );
        close(
            transform.full_image_rect.height() * pixels_per_point,
            texture_size.y,
        );
    }

    #[test]
    fn non_integer_physical_scale_keeps_unsnapped_origin() {
        let rect = egui::Rect::from_min_size(egui::pos2(0.25, 0.75), egui::vec2(130.0, 130.0));
        let transform = DisplayedImageTransform::from_resolved_rect(
            DisplayedImageTransformInput {
                page_idx: 0,
                viewport_rect: egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(500.0, 500.0),
                ),
                source_size: egui::vec2(100.0, 100.0),
                texture_size: egui::vec2(100.0, 100.0),
                rotation: Rotation::None,
                free_rotation_rad: 0.0,
                content_bbox: None,
                fit_mode: FullscreenFitMode::Page,
                fit_scale_limits: FullscreenFitScaleLimits::default(),
                pixels_per_point: 1.0,
                placement: ResolvedDisplayPlacement::Normal { zoom_pan: None },
            },
            rect,
        )
        .unwrap();

        rect_close(transform.full_image_rect, rect);
    }
}
