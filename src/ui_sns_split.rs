use crate::app::{App, SnsSplitDrag};
use crate::displayed_image_transform::DisplayedImageTransform;
use crate::export_crop::{CropHandle, CropRect};
use crate::keymap::KeyAction;
use crate::sns_split::{
    MAX_COUNT, MAX_SEAM_PERMILLE, MIN_COUNT, SnsFrameRatio, SnsSplitLayout, SnsTarget,
};

const PANEL_W: f32 = 264.0;
const PANEL_MARGIN: f32 = 14.0;
const PANEL_TOP: f32 = 72.0;
const PREVIEW_H: f32 = 96.0;
const MIN_PREVIEW_SEAM: f32 = 1.0;
const DEFAULT_CUSTOM_SEAM_PERMILLE: u16 = 10;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SnsSplitSeamChoice {
    None,
    Preset17,
    Custom,
}

impl SnsSplitSeamChoice {
    fn from_permille(seam_permille: u16) -> Self {
        match seam_permille {
            0 => Self::None,
            17 => Self::Preset17,
            _ => Self::Custom,
        }
    }
}

pub(crate) const SNS_SPLIT_ROTATION_DISABLED_REASON: &str =
    "回転しているページでは使えません。回転をリセットしてから実行してください";
pub(crate) const SNS_SPLIT_EXPORT_BUTTON_GUIDANCE: &str =
    "SNS 分割の書き出しはパネルの「分割して書き出す」から実行してください";
const SNS_SPLIT_TOO_SMALL_REASON: &str =
    "画像が小さすぎるため、選択した枚数を配置できません。書き出しには進めません。";
const SNS_SPLIT_INSTAGRAM_RATIO_REASON: &str = "Instagram はこの比率を切り取ります。枠の比率を 3:4 〜 1.91:1 に収めるか、投稿先を X にしてください";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SnsSplitEntryError {
    Rotated,
    ImageLoading,
}

impl SnsSplitEntryError {
    pub(crate) fn message(self) -> &'static str {
        match self {
            Self::Rotated => SNS_SPLIT_ROTATION_DISABLED_REASON,
            Self::ImageLoading => "[SNS 分割] 画像読み込み待ち",
        }
    }
}

pub(crate) fn sns_split_rotation_disabled_reason(
    rotation: crate::rotation_db::Rotation,
    free_rotation_rad: f32,
) -> Option<&'static str> {
    (!rotation.is_none() || free_rotation_rad != 0.0).then_some(SNS_SPLIT_ROTATION_DISABLED_REASON)
}

pub(crate) fn sns_split_disabled_reason(
    edit_tool_disabled_reason: Option<&'static str>,
    rotation: crate::rotation_db::Rotation,
    free_rotation_rad: f32,
) -> Option<&'static str> {
    edit_tool_disabled_reason
        .or_else(|| sns_split_rotation_disabled_reason(rotation, free_rotation_rad))
}

fn sns_split_instagram_ratio_is_out_of_range(
    layout: SnsSplitLayout,
    image_size: [usize; 2],
) -> bool {
    if layout.target != SnsTarget::Instagram {
        return false;
    }
    let (_, _, width, height) = layout.frames()[0].pixel_bounds(image_size[0], image_size[1]);
    let width = width as u128;
    let height = height as u128;
    width * 4 < height * 3 || width * 100 > height * 191
}

fn sns_split_export_disabled_reason(
    layout: SnsSplitLayout,
    image_size: [usize; 2],
) -> Option<&'static str> {
    if !layout.fits(image_size) {
        Some(SNS_SPLIT_TOO_SMALL_REASON)
    } else if sns_split_instagram_ratio_is_out_of_range(layout, image_size) {
        Some(SNS_SPLIT_INSTAGRAM_RATIO_REASON)
    } else {
        None
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct SnsSplitPanelSummary {
    dimensions: String,
    warning: Option<&'static str>,
}

impl SnsSplitPanelSummary {
    fn from_layout(layout: SnsSplitLayout, image_size: [usize; 2]) -> Self {
        let frames = layout.frames();
        let frame = frames[0];
        let (_, _, width, height) = frame.pixel_bounds(image_size[0], image_size[1]);
        let aspect = width as f64 / height.max(1) as f64;
        Self {
            dimensions: format!("1 枚 {width} x {height} ({aspect:.2})"),
            warning: sns_split_export_disabled_reason(layout, image_size),
        }
    }
}

fn sns_split_target_description(target: SnsTarget) -> String {
    let (seam_numerator, seam_denominator) = target.seam_ratio_parts();
    if seam_numerator == 0 {
        "継ぎ目の既定は「なし」".to_string()
    } else {
        let seam_percent = seam_numerator as f64 * 100.0 / seam_denominator as f64;
        format!("継ぎ目の既定は枠幅の{seam_percent:.1}%")
    }
}

fn sns_frame_ratio_from_key(key: Option<&str>) -> SnsFrameRatio {
    SnsFrameRatio::from_stable_key(key.unwrap_or_default())
}

fn stable_axis_origin(center: f32, occupied: usize, available: usize) -> usize {
    let occupied = occupied.clamp(1, available.max(1));
    let max_origin = available.max(1) - occupied;
    let desired = center.floor() as f64 - (occupied / 2) as f64;
    desired.round().clamp(0.0, max_origin as f64) as usize
}

fn stable_axis_room(center: f32, available: usize) -> usize {
    let available = available.max(1);
    let center_cell = (center.floor() as i64).clamp(0, available.saturating_sub(1) as i64) as usize;
    let reaches_left = center_cell.saturating_mul(2).saturating_add(1);
    let reaches_right = available.saturating_sub(center_cell).saturating_mul(2);
    reaches_left.min(reaches_right).clamp(1, available)
}

fn stable_centered_rect(
    group: CropRect,
    width: f32,
    height: f32,
    image_size: [usize; 2],
) -> CropRect {
    let base = group.sanitized(image_size[0], image_size[1]);
    let image_width = image_size[0].max(1);
    let image_height = image_size[1].max(1);
    let width = width.round().clamp(1.0, image_width as f32) as usize;
    let height = height.round().clamp(1.0, image_height as f32) as usize;
    let min_x = stable_axis_origin((base.min_x + base.max_x) * 0.5, width, image_width);
    let min_y = stable_axis_origin((base.min_y + base.max_y) * 0.5, height, image_height);
    CropRect {
        min_x: min_x as f32,
        min_y: min_y as f32,
        max_x: min_x.saturating_add(width) as f32,
        max_y: min_y.saturating_add(height) as f32,
    }
}

fn group_with_aspect_preserving_width(
    group: CropRect,
    group_aspect: f32,
    image_size: [usize; 2],
) -> CropRect {
    let base = group.sanitized(image_size[0], image_size[1]);
    let center_x = (base.min_x + base.max_x) * 0.5;
    let center_y = (base.min_y + base.max_y) * 0.5;
    let horizontal_room = stable_axis_room(center_x, image_size[0]) as f32;
    let vertical_room = stable_axis_room(center_y, image_size[1]) as f32;
    let aspect = group_aspect.max(0.01);
    let width = base
        .width()
        .min(horizontal_room)
        .min(vertical_room * aspect)
        .max(1.0);
    let height = (width / aspect).max(1.0);

    stable_centered_rect(base, width, height, image_size)
}

fn group_with_aspect_preserving_fit_fraction(
    group: CropRect,
    previous_group_aspect: f32,
    next_group_aspect: f32,
    image_size: [usize; 2],
) -> CropRect {
    let base = group.sanitized(image_size[0], image_size[1]);
    let center_x = (base.min_x + base.max_x) * 0.5;
    let center_y = (base.min_y + base.max_y) * 0.5;
    // stable_centered_rect と同じ離散中心セルから収容可能幅を求める。生の
    // 半ピクセル中心や単純な 2 * min(room) は、奇数幅を経由した往復で
    // 1px を失い、奇数サイズの画像では逆に最大幅を過小評価する。
    let horizontal_room = stable_axis_room(center_x, image_size[0]) as f32;
    let vertical_room = stable_axis_room(center_y, image_size[1]) as f32;
    let previous_aspect = previous_group_aspect.max(0.01);
    let next_aspect = next_group_aspect.max(0.01);
    // CropRect と枠境界は整数ピクセルへスナップされる。占有率の分母も実際に
    // 表現できる最大整数幅へ揃え、理論値の端数を毎フレーム引き直さない。
    let previous_max_width = horizontal_room
        .min(vertical_room * previous_aspect)
        .round()
        .max(1.0);
    let fit_fraction = (base.width() / previous_max_width).clamp(0.0, 1.0);
    let next_max_width = horizontal_room
        .min(vertical_room * next_aspect)
        .round()
        .max(1.0);
    let width = (next_max_width * fit_fraction).max(1.0);
    let height = (width / next_aspect).max(1.0);

    stable_centered_rect(base, width, height, image_size)
}

#[derive(Clone, Copy)]
enum SnsSplitVerticalAnchor {
    Center,
    Min,
    Max,
}

/// 理論上の group aspect を整数化したあと、先頭の実枠がプリセット比率へ最も
/// 近くなる整数高さへ揃える。割り切れない横幅でも 3:4 プリセット自身が
/// Instagram の 0.75 下限を丸め誤差だけで外れないよう、判定と同じ実枠を正にする。
fn snap_fixed_frame_height(
    layout: SnsSplitLayout,
    frame_ratio: SnsFrameRatio,
    image_size: [usize; 2],
    anchor: SnsSplitVerticalAnchor,
) -> SnsSplitLayout {
    let Some((ratio_width, ratio_height)) = frame_ratio.frame_ratio_parts() else {
        return layout;
    };
    let first_frame_width = layout.frames()[0].width().round().max(1.0) as usize;
    // 固定プリセットはすべて 3:4 以上だが、最近傍への整数丸めだけで実枠が
    // 0.75 未満になる場合がある (3k+2 px の 3:4 や極小の 4:5)。幅は保ち、
    // Instagram 下限を満たす最大整数高さへ内側から制限する。
    let height_numerator = first_frame_width as u128 * ratio_height;
    let nearest_height = (height_numerator + ratio_width / 2) / ratio_width;
    let instagram_safe_height = first_frame_width as u128 * 4 / 3;
    let desired_height = nearest_height
        .min(instagram_safe_height)
        .max(1)
        .min(usize::MAX as u128) as usize;
    let image_height = image_size[1].max(1) as f32;
    let group = layout.group.sanitized(image_size[0], image_size[1]);
    let (min_y, max_y) = match anchor {
        SnsSplitVerticalAnchor::Center => {
            let center = (group.min_y + group.max_y) * 0.5;
            // オフセンター配置では画像全高ではなく、この中心セルを保ったまま
            // 収められる高さが上限。丸めで 1px 超えた高さを後段の clamp に
            // 任せると中心セルがずれ、トポロジー往復のたびに寸法もずれる。
            let vertical_room = stable_axis_room(center, image_size[1]);
            let height = desired_height.min(vertical_room);
            let min_y = stable_axis_origin(center, height, image_size[1].max(1));
            (min_y as f32, min_y.saturating_add(height) as f32)
        }
        SnsSplitVerticalAnchor::Min => {
            let height = (desired_height as f32).min(image_height - group.min_y);
            (group.min_y, group.min_y + height)
        }
        SnsSplitVerticalAnchor::Max => {
            let height = (desired_height as f32).min(group.max_y);
            (group.max_y - height, group.max_y)
        }
    };

    SnsSplitLayout {
        group: CropRect {
            min_y,
            max_y,
            ..group
        },
        ..layout
    }
    .clamped(image_size)
}

fn transition_sns_frame_ratio(
    previous_layout: SnsSplitLayout,
    next_layout: SnsSplitLayout,
    previous_frame_ratio: SnsFrameRatio,
    next_frame_ratio: SnsFrameRatio,
    image_size: [usize; 2],
) -> SnsSplitLayout {
    let Some(next_group_aspect) =
        next_frame_ratio.group_aspect(next_layout.count, next_layout.seam_permille)
    else {
        return next_layout.clamped(image_size);
    };
    let previous_group_aspect =
        previous_frame_ratio.group_aspect(previous_layout.count, previous_layout.seam_permille);
    let group = if let Some(previous_group_aspect) = previous_group_aspect {
        group_with_aspect_preserving_fit_fraction(
            previous_layout.group,
            previous_group_aspect,
            next_group_aspect,
            image_size,
        )
    } else {
        group_with_aspect_preserving_width(previous_layout.group, next_group_aspect, image_size)
    };
    let layout = SnsSplitLayout {
        group,
        ..next_layout
    }
    .clamped(image_size);
    snap_fixed_frame_height(
        layout,
        next_frame_ratio,
        image_size,
        SnsSplitVerticalAnchor::Center,
    )
}

fn fit_sns_split_to_full_image(
    layout: SnsSplitLayout,
    frame_ratio: SnsFrameRatio,
    image_size: [usize; 2],
) -> SnsSplitLayout {
    let full = CropRect::full(image_size[0], image_size[1]);
    let group =
        if let Some(group_aspect) = frame_ratio.group_aspect(layout.count, layout.seam_permille) {
            full.fit_to_aspect_around_center(group_aspect, image_size[0], image_size[1])
        } else {
            full
        };
    let layout = SnsSplitLayout { group, ..layout }.clamped(image_size);
    snap_fixed_frame_height(
        layout,
        frame_ratio,
        image_size,
        SnsSplitVerticalAnchor::Center,
    )
}

fn drag_sns_split_layout(
    base: SnsSplitLayout,
    handle: CropHandle,
    delta_x: f32,
    delta_y: f32,
    image_size: [usize; 2],
    frame_ratio: SnsFrameRatio,
) -> SnsSplitLayout {
    let group = base.group.dragged(
        handle,
        delta_x,
        delta_y,
        image_size[0],
        image_size[1],
        frame_ratio.group_aspect(base.count, base.seam_permille),
    );
    let layout = SnsSplitLayout { group, ..base }.clamped(image_size);
    let anchor = match handle {
        CropHandle::North | CropHandle::NorthWest | CropHandle::NorthEast => {
            SnsSplitVerticalAnchor::Max
        }
        CropHandle::South | CropHandle::SouthWest | CropHandle::SouthEast => {
            SnsSplitVerticalAnchor::Min
        }
        CropHandle::West | CropHandle::East | CropHandle::Body => SnsSplitVerticalAnchor::Center,
    };
    snap_fixed_frame_height(layout, frame_ratio, image_size, anchor)
}

fn sns_split_preview_rects(
    bounds: egui::Rect,
    frames: &[CropRect],
    seam_ratio: f32,
) -> Vec<egui::Rect> {
    let Some(frame) = frames.first().copied() else {
        return Vec::new();
    };
    let count = frames.len() as f32;
    let frame_aspect = {
        let aspect = frame.width() / frame.height();
        if aspect.is_finite() && aspect > 0.0 {
            aspect
        } else {
            1.0
        }
    };
    let seam_ratio = if seam_ratio.is_finite() {
        seam_ratio.max(0.0)
    } else {
        0.0
    };
    let divisor = count + (count - 1.0) * seam_ratio;
    let bounds_width = bounds.width().max(0.0);
    let minimum_seam = if seam_ratio > 0.0 {
        MIN_PREVIEW_SEAM
    } else {
        0.0
    };
    let ratio_limited_width = bounds_width / divisor;
    let minimum_seam_limited_width = (bounds_width - (count - 1.0) * minimum_seam).max(0.0) / count;
    let frame_width = ratio_limited_width
        .min(minimum_seam_limited_width)
        .min(bounds.height().max(0.0) * frame_aspect);
    let frame_height = frame_width / frame_aspect;
    let seam_width = if seam_ratio > 0.0 {
        (frame_width * seam_ratio).max(MIN_PREVIEW_SEAM)
    } else {
        0.0
    };
    let step = frame_width + seam_width;
    let total_width = frame_width + step * (count - 1.0);
    let origin = egui::pos2(
        bounds.center().x - total_width * 0.5,
        bounds.center().y - frame_height * 0.5,
    );

    (0..frames.len())
        .map(|index| {
            egui::Rect::from_min_size(
                origin + egui::vec2(step * index as f32, 0.0),
                egui::vec2(frame_width, frame_height),
            )
        })
        .collect()
}

fn sns_split_frame_uvs(frames: &[CropRect], image_size: [usize; 2]) -> Vec<egui::Rect> {
    let width = image_size[0].max(1) as f32;
    let height = image_size[1].max(1) as f32;
    let normalized = |value: f32, extent: f32| {
        if value.is_finite() {
            (value / extent).clamp(0.0, 1.0)
        } else {
            0.0
        }
    };

    frames
        .iter()
        .map(|frame| {
            egui::Rect::from_min_max(
                egui::pos2(
                    normalized(frame.min_x, width),
                    normalized(frame.min_y, height),
                ),
                egui::pos2(
                    normalized(frame.max_x, width),
                    normalized(frame.max_y, height),
                ),
            )
        })
        .collect()
}

fn draw_sns_split_preview(
    ui: &mut egui::Ui,
    layout: SnsSplitLayout,
    image_size: [usize; 2],
    texture: Option<&egui::TextureHandle>,
) {
    let frames = layout.frames();
    let (bounds, _) = ui.allocate_exact_size(
        egui::vec2(ui.available_width().max(1.0), PREVIEW_H),
        egui::Sense::hover(),
    );
    let paint_rects = sns_split_preview_rects(bounds, &frames, layout.seam_ratio());
    let uv_rects = sns_split_frame_uvs(&frames, image_size);
    let painter = ui.painter_at(bounds);
    painter.rect_filled(
        bounds,
        4.0,
        egui::Color32::from_rgba_unmultiplied(0, 0, 0, 150),
    );

    for (paint_rect, uv_rect) in paint_rects.into_iter().zip(uv_rects) {
        if let Some(texture) = texture {
            painter.image(texture.id(), paint_rect, uv_rect, egui::Color32::WHITE);
        } else {
            painter.rect_filled(
                paint_rect,
                1.0,
                egui::Color32::from_rgba_unmultiplied(100, 110, 120, 90),
            );
            painter.rect_stroke(
                paint_rect,
                1.0,
                egui::Stroke::new(1.0, egui::Color32::from_gray(135)),
                egui::StrokeKind::Inside,
            );
        }
    }
}

fn sns_split_panel_outer_height(full_rect: egui::Rect, panel_pos: egui::Pos2) -> f32 {
    // 隠蔽加工・消しゴムと同じく、ウィンドウ下端まで伸ばす。上限を置くと、本文が
    // それより高い環境で「分割して書き出す」がスクロールの外に出る (実機報告
    // 2026-09-01: 430px 上限でボタンが見えなかった)。下限は、極端に低いウィンドウでも
    // ヘッダーと本文の先頭が読める高さ。
    (full_rect.bottom() - panel_pos.y - PANEL_MARGIN).max(240.0)
}

fn clamp_pos_to_rect(pos: egui::Pos2, rect: egui::Rect) -> egui::Pos2 {
    egui::pos2(
        pos.x.clamp(rect.left(), rect.right()),
        pos.y.clamp(rect.top(), rect.bottom()),
    )
}

fn screen_to_image(
    transform: &DisplayedImageTransform,
    image_size: [usize; 2],
    pos: egui::Pos2,
) -> [f32; 2] {
    let point = transform.screen_to_source_normalized(pos);
    [
        (point.x * image_size[0].max(1) as f32).clamp(0.0, image_size[0].max(1) as f32),
        (point.y * image_size[1].max(1) as f32).clamp(0.0, image_size[1].max(1) as f32),
    ]
}

fn image_to_screen(
    transform: &DisplayedImageTransform,
    image_size: [usize; 2],
    point: [f32; 2],
) -> egui::Pos2 {
    transform.source_normalized_to_screen(egui::pos2(
        point[0] / image_size[0].max(1) as f32,
        point[1] / image_size[1].max(1) as f32,
    ))
}

fn rect_corners(
    transform: &DisplayedImageTransform,
    image_size: [usize; 2],
    rect: CropRect,
) -> [egui::Pos2; 4] {
    [
        image_to_screen(transform, image_size, [rect.min_x, rect.min_y]),
        image_to_screen(transform, image_size, [rect.max_x, rect.min_y]),
        image_to_screen(transform, image_size, [rect.max_x, rect.max_y]),
        image_to_screen(transform, image_size, [rect.min_x, rect.max_y]),
    ]
}

fn handle_points(
    transform: &DisplayedImageTransform,
    image_size: [usize; 2],
    rect: CropRect,
) -> [(CropHandle, egui::Pos2); 8] {
    let center_x = (rect.min_x + rect.max_x) * 0.5;
    let center_y = (rect.min_y + rect.max_y) * 0.5;
    [
        (
            CropHandle::NorthWest,
            image_to_screen(transform, image_size, [rect.min_x, rect.min_y]),
        ),
        (
            CropHandle::North,
            image_to_screen(transform, image_size, [center_x, rect.min_y]),
        ),
        (
            CropHandle::NorthEast,
            image_to_screen(transform, image_size, [rect.max_x, rect.min_y]),
        ),
        (
            CropHandle::East,
            image_to_screen(transform, image_size, [rect.max_x, center_y]),
        ),
        (
            CropHandle::SouthEast,
            image_to_screen(transform, image_size, [rect.max_x, rect.max_y]),
        ),
        (
            CropHandle::South,
            image_to_screen(transform, image_size, [center_x, rect.max_y]),
        ),
        (
            CropHandle::SouthWest,
            image_to_screen(transform, image_size, [rect.min_x, rect.max_y]),
        ),
        (
            CropHandle::West,
            image_to_screen(transform, image_size, [rect.min_x, center_y]),
        ),
    ]
}

fn handle_cursor(handle: CropHandle) -> egui::CursorIcon {
    match handle {
        CropHandle::North | CropHandle::South => egui::CursorIcon::ResizeVertical,
        CropHandle::East | CropHandle::West => egui::CursorIcon::ResizeHorizontal,
        CropHandle::NorthWest | CropHandle::SouthEast => egui::CursorIcon::ResizeNwSe,
        CropHandle::NorthEast | CropHandle::SouthWest => egui::CursorIcon::ResizeNeSw,
        CropHandle::Body => egui::CursorIcon::Grab,
    }
}

fn paint_polygon(painter: &egui::Painter, points: Vec<egui::Pos2>, color: egui::Color32) {
    painter.add(egui::Shape::convex_polygon(
        points,
        color,
        egui::Stroke::NONE,
    ));
}

fn paint_outside_extent(
    painter: &egui::Painter,
    transform: &DisplayedImageTransform,
    image_size: [usize; 2],
    extent: CropRect,
    color: egui::Color32,
) {
    let full = rect_corners(
        transform,
        image_size,
        CropRect::full(image_size[0], image_size[1]),
    );
    let extent = rect_corners(transform, image_size, extent);
    for points in [
        vec![full[0], full[1], extent[1], extent[0]],
        vec![full[1], full[2], extent[2], extent[1]],
        vec![full[2], full[3], extent[3], extent[2]],
        vec![full[3], full[0], extent[0], extent[3]],
    ] {
        paint_polygon(painter, points, color);
    }
}

fn paint_seams(
    painter: &egui::Painter,
    transform: &DisplayedImageTransform,
    image_size: [usize; 2],
    frames: &[CropRect],
    color: egui::Color32,
) {
    for pair in frames.windows(2) {
        if pair[1].min_x <= pair[0].max_x {
            continue;
        }
        let seam = CropRect {
            min_x: pair[0].max_x,
            min_y: pair[0].min_y,
            max_x: pair[1].min_x,
            max_y: pair[0].max_y,
        };
        paint_polygon(
            painter,
            rect_corners(transform, image_size, seam).to_vec(),
            color,
        );
    }
}

impl App {
    pub(crate) fn sns_split_panel_rect(&self, full_rect: egui::Rect) -> egui::Rect {
        let pos = egui::pos2(full_rect.left() + PANEL_MARGIN, full_rect.top() + PANEL_TOP);
        let height = sns_split_panel_outer_height(full_rect, pos);
        egui::Rect::from_min_size(pos, egui::vec2(PANEL_W, height))
    }

    fn sns_split_rotation_error(&mut self, fs_idx: usize) -> Option<SnsSplitEntryError> {
        let rotation = self.get_rotation(fs_idx);
        sns_split_rotation_disabled_reason(rotation, self.fs_free_rotation)
            .map(|_| SnsSplitEntryError::Rotated)
    }

    pub(crate) fn enter_sns_split_mode(&mut self, fs_idx: usize) -> Result<(), SnsSplitEntryError> {
        if self.sns_split.is_some() {
            if let Some(error) = self.sns_split_rotation_error(fs_idx) {
                return Err(error);
            }
            return Ok(());
        }
        let (target_idx, pivot) = self.plan_page_edit_pivot(fs_idx);
        if let Some(error) = self.sns_split_rotation_error(target_idx) {
            return Err(error);
        }
        let Some(source_size) = self.fs_page_coordinate_source_size(target_idx) else {
            return Err(SnsSplitEntryError::ImageLoading);
        };
        let image_size = [
            source_size.x.round().max(1.0) as usize,
            source_size.y.round().max(1.0) as usize,
        ];
        let target = SnsTarget::from_stable_key(
            self.settings
                .sns_split_target
                .as_deref()
                .unwrap_or(SnsTarget::X.stable_key()),
        );
        let count = self.settings.sns_split_count.clamp(MIN_COUNT, MAX_COUNT);
        let frame_ratio = sns_frame_ratio_from_key(self.settings.sns_split_frame_ratio.as_deref());
        let layout = SnsSplitLayout::centered_max(target, count, image_size)
            .with_seam_permille(self.settings.sns_split_seam_permille, image_size);
        let layout = fit_sns_split_to_full_image(layout, frame_ratio, image_size);

        if let Some(pivot) = pivot {
            self.sns_split_spread_ctx = Some(pivot);
            self.enter_page_edit_single_view(target_idx);
        }
        self.sns_split = Some(layout);
        self.sns_split_drag = None;
        Ok(())
    }

    pub(crate) fn reset_sns_split_mode(&mut self) {
        self.sns_split = None;
        self.sns_split_drag = None;
        let pivot = self.sns_split_spread_ctx.take();
        self.leave_page_edit_single_view(pivot);
    }

    pub(crate) fn handle_sns_split_keys(
        &mut self,
        ctx: &egui::Context,
        fs_idx: usize,
    ) -> crate::ui_fullscreen::FsKeyAction {
        let action = crate::ui_fullscreen::FsKeyAction {
            close: false,
            close_to_page_list: false,
            page_nav: crate::ui_fullscreen::FsPageNav::None,
            ctrl_nav: None,
            sibling_nav: None,
            mouse_nav: None,
            jump_to: None,
        };
        if let Some(error) = self.sns_split_rotation_error(fs_idx) {
            self.reset_sns_split_mode();
            self.show_feedback_toast(error.message().to_string());
            return action;
        }
        if !self.ime_input_active(ctx) && self.consume_context_shortcuts_help_key(ctx) {
            self.show_context_shortcuts_help = true;
            return action;
        }
        if ctx.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::Escape)) {
            self.reset_sns_split_mode();
            return action;
        }
        if self.keymap.consume_action(ctx, KeyAction::SnsSplitExecute) {
            self.execute_sns_split_export(ctx, fs_idx);
        } else if self.keymap.consume_action(ctx, KeyAction::FsExport) {
            // Ctrl+E は通常のエクスポート操作のままにし、open_export_dialog の
            // 編集モード guard からパネルボタンを案内する。利用者が専用操作へ
            // 同じキーを割り当てた場合は、上の SnsSplitExecute を優先する。
            self.open_export_dialog_for_current(ctx, fs_idx);
        }
        action
    }

    fn execute_sns_split_export(&mut self, ctx: &egui::Context, fs_idx: usize) -> bool {
        let Some(layout) = self.sns_split else {
            return false;
        };
        if let Some(error) = self.sns_split_rotation_error(fs_idx) {
            self.reset_sns_split_mode();
            self.show_feedback_toast(error.message().to_string());
            return false;
        }
        let Some(source_size) = self.fs_page_coordinate_source_size(fs_idx) else {
            // キー入力に対して何も起きないと、利用者は原因を知る手段が無い。
            // open_export_dialog 側の失敗経路と同じく理由を出す。
            self.show_feedback_toast("ページの寸法をまだ取得できていません".to_string());
            return false;
        };
        let image_size = [
            source_size.x.round().max(1.0) as usize,
            source_size.y.round().max(1.0) as usize,
        ];
        if let Some(reason) = sns_split_export_disabled_reason(layout, image_size) {
            // パネルにも理由は出ているが、キーを押して無反応にはしない。
            self.show_feedback_toast(reason.to_string());
            return false;
        }

        let frames = layout.frames();
        self.open_export_dialog_for_sns_split(ctx, fs_idx, frames, image_size)
    }

    pub(crate) fn draw_sns_split_panel(
        &mut self,
        ctx: &egui::Context,
        fs_idx: usize,
        full_rect: egui::Rect,
        image_size: Option<[usize; 2]>,
        preview_texture: Option<&egui::TextureHandle>,
    ) {
        if self.sns_split.is_none() {
            return;
        }
        let panel_rect = self.sns_split_panel_rect(full_rect);
        let panel_pos = panel_rect.min;
        let panel_height = panel_rect.height();
        let mut close = false;
        let mut export_requested = false;

        egui::Area::new(egui::Id::new("sns_split_panel"))
            .order(egui::Order::Foreground)
            .fixed_pos(panel_pos)
            .show(ctx, |ui| {
                ui.interact(
                    egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(PANEL_W + 12.0, panel_height + 12.0),
                    ),
                    egui::Id::new("sns_split_panel_click_sink"),
                    egui::Sense::click(),
                );
                egui::Frame::popup(ui.style())
                    .fill(egui::Color32::from_rgba_unmultiplied(20, 20, 20, 230))
                    .stroke(egui::Stroke::new(
                        1.0,
                        egui::Color32::from_rgba_unmultiplied(255, 255, 255, 40),
                    ))
                    .corner_radius(6.0)
                    .show(ui, |ui| {
                        ui.set_min_width(PANEL_W);
                        ui.set_max_width(PANEL_W);
                        ui.set_max_height(panel_height);
                        crate::os_theme::apply_dark_ui(ui);

                        ui.horizontal(|ui| {
                            ui.heading("SNS 分割");
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    let (close_rect, close_response) = ui.allocate_exact_size(
                                        egui::vec2(26.0, 22.0),
                                        egui::Sense::click(),
                                    );
                                    let background = if close_response.hovered() {
                                        egui::Color32::from_rgba_unmultiplied(220, 80, 80, 200)
                                    } else {
                                        egui::Color32::from_rgba_unmultiplied(80, 80, 80, 120)
                                    };
                                    ui.painter().rect_filled(close_rect, 4.0, background);
                                    crate::ui_fullscreen::draw_icons::draw_close_icon(
                                        ui.painter(),
                                        close_rect.center(),
                                        8.0,
                                    );
                                    if close_response.clicked() {
                                        close = true;
                                    }
                                    close_response.on_hover_text("閉じる (Esc)");
                                },
                            );
                        });
                        ui.separator();

                        // 短い viewport ではプレビュー追加後の本文が収まらないため、
                        // ヘッダーを残して本文だけをスクロールさせる。
                        let body_height =
                            (panel_rect.bottom() - ui.cursor().top() - 12.0).max(100.0);
                        ui.allocate_ui_with_layout(
                            egui::vec2(PANEL_W, body_height),
                            egui::Layout::top_down(egui::Align::LEFT),
                            |ui| {
                                ui.set_min_width(PANEL_W);
                                ui.set_max_width(PANEL_W);
                                ui.set_min_height(body_height);
                                egui::ScrollArea::vertical()
                                    .id_salt("sns_split_panel_body")
                                    .max_height(body_height)
                                    .auto_shrink([false, false])
                                    .show(ui, |ui| {
                                        let Some(image_size) = image_size else {
                                            ui.add_enabled(
                                                false,
                                                egui::Button::new("画像読み込み待ち"),
                                            );
                                            ui.add_space(8.0);
                                            ui.label("投稿後の見え方");
                                            ui.add_space(3.0);
                                            if let Some(layout) = self.sns_split {
                                                draw_sns_split_preview(ui, layout, [1, 1], None);
                                            }
                                            ui.add_space(8.0);
                                            ui.add_enabled(
                                                false,
                                                egui::Button::new("分割して書き出す").min_size(
                                                    egui::vec2(ui.available_width(), 34.0),
                                                ),
                                            )
                                            .on_disabled_hover_text(
                                                "ページの寸法をまだ取得できていません",
                                            );
                                            return;
                                        };
                                        export_requested = self.draw_sns_split_controls(
                                            ui,
                                            image_size,
                                            preview_texture,
                                        );
                                    });
                            },
                        );
                    });
            });

        if close {
            self.reset_sns_split_mode();
        } else if export_requested {
            self.execute_sns_split_export(ctx, fs_idx);
        }
    }

    fn draw_sns_split_controls(
        &mut self,
        ui: &mut egui::Ui,
        image_size: [usize; 2],
        preview_texture: Option<&egui::TextureHandle>,
    ) -> bool {
        let Some(mut layout) = self.sns_split else {
            return false;
        };
        let previous_layout = layout;
        let mut frame_ratio =
            sns_frame_ratio_from_key(self.settings.sns_split_frame_ratio.as_deref());

        ui.label("投稿先");
        let mut target = layout.target;
        ui.horizontal(|ui| {
            for option in SnsTarget::ALL {
                ui.selectable_value(&mut target, option, option.label());
            }
        });
        ui.label(
            egui::RichText::new(sns_split_target_description(target))
                .small()
                .color(ui.visuals().weak_text_color()),
        );

        ui.add_space(6.0);
        ui.label("枚数");
        let mut count = layout.count;
        ui.horizontal(|ui| {
            for option in MIN_COUNT..=MAX_COUNT {
                ui.selectable_value(&mut count, option, option.to_string());
            }
        });

        let target_changed = target != layout.target;
        let count_changed = count != layout.count;
        let mut seam_permille = if target_changed {
            target.default_seam_permille()
        } else {
            layout.seam_permille
        };
        let mut persist_settings = target_changed || count_changed;
        let seam_choice_id = ui.make_persistent_id("sns_split_seam_choice");
        let mut seam_choice = if target_changed {
            SnsSplitSeamChoice::from_permille(seam_permille)
        } else {
            ui.data(|data| data.get_temp::<SnsSplitSeamChoice>(seam_choice_id))
                .unwrap_or_else(|| SnsSplitSeamChoice::from_permille(seam_permille))
        };

        ui.add_space(6.0);
        ui.label("継ぎ目");
        ui.horizontal_wrapped(|ui| {
            if ui
                .selectable_label(seam_choice == SnsSplitSeamChoice::None, "なし")
                .clicked()
                && seam_choice != SnsSplitSeamChoice::None
            {
                seam_choice = SnsSplitSeamChoice::None;
                seam_permille = 0;
                persist_settings = true;
            }
            if ui
                .selectable_label(seam_choice == SnsSplitSeamChoice::Preset17, "1.7%")
                .clicked()
                && seam_choice != SnsSplitSeamChoice::Preset17
            {
                seam_choice = SnsSplitSeamChoice::Preset17;
                seam_permille = 17;
                persist_settings = true;
            }
            if ui
                .selectable_label(seam_choice == SnsSplitSeamChoice::Custom, "任意")
                .clicked()
                && seam_choice != SnsSplitSeamChoice::Custom
            {
                seam_choice = SnsSplitSeamChoice::Custom;
                seam_permille = DEFAULT_CUSTOM_SEAM_PERMILLE;
                persist_settings = true;
            }
        });
        if seam_choice == SnsSplitSeamChoice::Custom {
            let mut seam_percent = f64::from(seam_permille) / 10.0;
            let response = ui.add(
                egui::DragValue::new(&mut seam_percent)
                    .range(0.0..=10.0)
                    .speed(0.1)
                    .fixed_decimals(1)
                    .suffix("%"),
            );
            if response.changed() {
                seam_permille = (seam_percent * 10.0)
                    .round()
                    .clamp(0.0, f64::from(MAX_SEAM_PERMILLE))
                    as u16;
            }
            persist_settings |=
                response.drag_stopped() || (response.changed() && !response.dragged());
        }
        ui.data_mut(|data| data.insert_temp(seam_choice_id, seam_choice));

        ui.add_space(6.0);
        ui.label("枠の比率");
        let previous_frame_ratio = frame_ratio;
        ui.horizontal_wrapped(|ui| {
            for option in SnsFrameRatio::ALL {
                ui.selectable_value(&mut frame_ratio, option, option.label());
            }
        });
        let frame_ratio_changed = frame_ratio != previous_frame_ratio;
        persist_settings |= frame_ratio_changed;

        let seam_changed = seam_permille != layout.seam_permille;
        let topology_changed =
            target_changed || count_changed || seam_changed || frame_ratio_changed;
        if target_changed {
            layout = layout.with_target(target, image_size);
        }
        if count_changed {
            layout = layout.with_count(count, image_size);
        }
        if seam_permille != layout.seam_permille {
            layout = layout.with_seam_permille(seam_permille, image_size);
        }
        if topology_changed && frame_ratio != SnsFrameRatio::Free {
            layout = transition_sns_frame_ratio(
                previous_layout,
                layout,
                previous_frame_ratio,
                frame_ratio,
                image_size,
            );
        }

        ui.add_space(6.0);
        let fit_full = ui
            .add_sized(
                [ui.available_width(), 28.0],
                egui::Button::new("画像全体に合わせる"),
            )
            .clicked();
        if fit_full {
            layout = fit_sns_split_to_full_image(layout, frame_ratio, image_size);
        }

        if topology_changed || fit_full {
            self.settings.sns_split_target = Some(layout.target.stable_key().to_string());
            self.settings.sns_split_count = layout.count;
            self.settings.sns_split_seam_permille = layout.seam_permille;
            self.settings.sns_split_frame_ratio = Some(frame_ratio.stable_key().to_string());
            self.sns_split = Some(layout);
            self.sns_split_drag = None;
        }
        if persist_settings {
            self.settings.save();
        }

        ui.add_space(8.0);
        ui.label("投稿後の見え方");
        ui.add_space(3.0);
        draw_sns_split_preview(ui, layout, image_size, preview_texture);

        ui.add_space(8.0);
        let summary = SnsSplitPanelSummary::from_layout(layout, image_size);
        ui.label(
            egui::RichText::new("枠の寸法（横/縦）")
                .small()
                .color(ui.visuals().weak_text_color()),
        );
        ui.label(summary.dimensions);
        if let Some(warning) = summary.warning {
            ui.add_space(4.0);
            ui.colored_label(egui::Color32::from_rgb(255, 180, 90), warning);
        }

        ui.add_space(8.0);
        let response = ui.add_enabled(
            summary.warning.is_none(),
            egui::Button::new("分割して書き出す").min_size(egui::vec2(ui.available_width(), 34.0)),
        );
        let response = if let Some(reason) = summary.warning {
            response.on_disabled_hover_text(reason)
        } else {
            response
        };
        response.clicked()
    }

    pub(crate) fn draw_sns_split_overlay(
        &mut self,
        ui: &mut egui::Ui,
        transform: &DisplayedImageTransform,
        image_size: [usize; 2],
        pointer_allowed: bool,
    ) -> bool {
        let Some(layout) = self.sns_split else {
            return false;
        };
        let frames = layout.frames();
        let extent = layout.frames_extent();
        let painter = ui.painter().with_clip_rect(transform.viewport_rect);
        let mask_color = egui::Color32::from_rgba_unmultiplied(0, 0, 0, 145);
        paint_outside_extent(&painter, transform, image_size, extent, mask_color);
        paint_seams(&painter, transform, image_size, &frames, mask_color);

        for (index, frame) in frames.iter().copied().enumerate() {
            let corners = rect_corners(transform, image_size, frame);
            let mut outline = corners.to_vec();
            outline.push(corners[0]);
            painter.add(egui::Shape::line(
                outline.clone(),
                egui::Stroke::new(3.0, egui::Color32::BLACK),
            ));
            painter.add(egui::Shape::line(
                outline,
                egui::Stroke::new(2.0, egui::Color32::from_rgb(120, 220, 255)),
            ));
            let center = egui::pos2(
                corners.iter().map(|point| point.x).sum::<f32>() * 0.25,
                corners.iter().map(|point| point.y).sum::<f32>() * 0.25,
            );
            painter.text(
                center + egui::vec2(1.0, 1.0),
                egui::Align2::CENTER_CENTER,
                (index + 1).to_string(),
                egui::FontId::proportional(22.0),
                egui::Color32::BLACK,
            );
            painter.text(
                center,
                egui::Align2::CENTER_CENTER,
                (index + 1).to_string(),
                egui::FontId::proportional(22.0),
                egui::Color32::WHITE,
            );
        }

        let handle_bounds = transform.viewport_rect.shrink(14.0);
        let handles = handle_points(transform, image_size, layout.group);
        for (_, center) in handles {
            let center = clamp_pos_to_rect(center, handle_bounds);
            painter.circle_filled(center, 5.5, egui::Color32::from_rgb(190, 240, 255));
            painter.circle_stroke(
                center,
                5.5,
                egui::Stroke::new(1.5, egui::Color32::from_rgb(0, 35, 50)),
            );
        }

        if !pointer_allowed {
            self.sns_split_drag = None;
            return false;
        }

        self.handle_sns_split_pointer(ui, transform, image_size, layout, handles)
    }

    fn handle_sns_split_pointer(
        &mut self,
        ui: &mut egui::Ui,
        transform: &DisplayedImageTransform,
        image_size: [usize; 2],
        layout: SnsSplitLayout,
        handles: [(CropHandle, egui::Pos2); 8],
    ) -> bool {
        let frame_ratio = sns_frame_ratio_from_key(self.settings.sns_split_frame_ratio.as_deref());
        let (primary_pressed, primary_down, primary_released, press_origin, hover_pos, total_delta) =
            ui.input(|input| {
                (
                    input.pointer.primary_pressed(),
                    input.pointer.primary_down(),
                    input.pointer.primary_released(),
                    input.pointer.press_origin(),
                    input.pointer.hover_pos(),
                    input.pointer.total_drag_delta().unwrap_or(egui::Vec2::ZERO),
                )
            });
        const HANDLE_HIT: f32 = 32.0;
        let target_at = |pos: egui::Pos2| -> Option<CropHandle> {
            for (handle, center) in handles {
                let center = clamp_pos_to_rect(center, transform.viewport_rect.shrink(14.0));
                if (pos - center).length() <= HANDLE_HIT {
                    return Some(handle);
                }
            }
            let image = screen_to_image(transform, image_size, pos);
            if image[0] >= layout.group.min_x
                && image[0] <= layout.group.max_x
                && image[1] >= layout.group.min_y
                && image[1] <= layout.group.max_y
            {
                Some(CropHandle::Body)
            } else {
                None
            }
        };

        if primary_pressed {
            self.sns_split_drag = press_origin.and_then(|origin| {
                target_at(origin).map(|handle| SnsSplitDrag {
                    handle,
                    base: layout,
                })
            });
        }

        let mut used = false;
        if primary_down && let Some(drag) = self.sns_split_drag {
            let origin = press_origin.unwrap_or_else(|| transform.viewport_rect.center());
            let start = screen_to_image(transform, image_size, origin);
            let end = screen_to_image(transform, image_size, origin + total_delta);
            self.sns_split = Some(drag_sns_split_layout(
                drag.base,
                drag.handle,
                end[0] - start[0],
                end[1] - start[1],
                image_size,
                frame_ratio,
            ));
            ui.ctx()
                .set_cursor_icon(if drag.handle == CropHandle::Body {
                    egui::CursorIcon::Grabbing
                } else {
                    handle_cursor(drag.handle)
                });
            used = true;
        }

        if primary_released {
            self.sns_split_drag = None;
        }

        if !used
            && let Some(pos) = hover_pos
            && let Some(handle) = target_at(pos)
        {
            ui.ctx().set_cursor_icon(handle_cursor(handle));
            used = true;
        }

        used || self.sns_split_drag.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_panel_reaches_the_bottom_of_every_window_it_is_given() {
        // 実機報告 (2026-09-01): 上限 430px のせいで「分割して書き出す」がスクロールの
        // 外にあり、押すまでに一度スクロールが要った。隠蔽加工・消しゴムと同じく、
        // 使える高さを全部使う。
        for window_height in [720.0_f32, 1080.0, 1440.0, 2160.0] {
            let full_rect =
                egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(2560.0, window_height));
            let panel_pos =
                egui::pos2(full_rect.left() + PANEL_MARGIN, full_rect.top() + PANEL_TOP);
            let height = sns_split_panel_outer_height(full_rect, panel_pos);
            let bottom = panel_pos.y + height;
            assert!(
                (full_rect.bottom() - bottom - PANEL_MARGIN).abs() < 0.5,
                "{window_height}px の窓で下端まで伸びていない: 底 {bottom}",
            );
        }

        // 下端まで伸ばしても、極端に低い窓ではヘッダーと本文の先頭を残す。
        let squat = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(2560.0, 200.0));
        let panel_pos = egui::pos2(squat.left() + PANEL_MARGIN, squat.top() + PANEL_TOP);
        assert_eq!(sns_split_panel_outer_height(squat, panel_pos), 240.0);
    }

    #[test]
    fn rotation_disabled_reason_covers_every_saved_and_free_rotation() {
        use crate::rotation_db::Rotation;

        assert_eq!(
            sns_split_rotation_disabled_reason(Rotation::None, 0.0),
            None
        );
        for rotation in [Rotation::Cw90, Rotation::Cw180, Rotation::Cw270] {
            assert_eq!(
                sns_split_rotation_disabled_reason(rotation, 0.0),
                Some(SNS_SPLIT_ROTATION_DISABLED_REASON)
            );
        }
        for free_rotation_rad in [0.25, -0.25, f32::MIN_POSITIVE] {
            assert_eq!(
                sns_split_rotation_disabled_reason(Rotation::None, free_rotation_rad),
                Some(SNS_SPLIT_ROTATION_DISABLED_REASON)
            );
        }
    }

    #[test]
    fn sns_split_disabled_reason_preserves_existing_edit_tool_priority() {
        use crate::rotation_db::Rotation;

        assert_eq!(
            sns_split_disabled_reason(Some("detached"), Rotation::Cw90, 0.0),
            Some("detached")
        );
        assert_eq!(
            sns_split_disabled_reason(None, Rotation::Cw180, 0.0),
            Some(SNS_SPLIT_ROTATION_DISABLED_REASON)
        );
    }

    #[test]
    fn preview_rects_match_count_size_minimum_seam_and_bounds() {
        let layout = SnsSplitLayout::centered_max(SnsTarget::X, 4, [2400, 1800]);
        let frames = layout.frames();
        let bounds =
            egui::Rect::from_min_size(egui::pos2(11.0, 17.0), egui::vec2(216.0, PREVIEW_H));
        let rects = sns_split_preview_rects(bounds, &frames, layout.seam_ratio());

        assert_eq!(rects.len(), frames.len());
        for rect in &rects[1..] {
            assert!((rect.width() - rects[0].width()).abs() < 0.001);
            assert!((rect.height() - rects[0].height()).abs() < 0.001);
        }
        for pair in rects.windows(2) {
            let gap = pair[1].left() - pair[0].right();
            let expected_gap = (pair[0].width() * layout.seam_ratio()).max(MIN_PREVIEW_SEAM);
            assert!((gap - expected_gap).abs() < 0.001);
            assert!(gap >= MIN_PREVIEW_SEAM - 0.001);
        }
        assert!(rects[0].left() >= bounds.left() - 0.001);
        assert!(rects.last().unwrap().right() <= bounds.right() + 0.001);
        assert!(rects[0].top() >= bounds.top() - 0.001);
        assert!(rects[0].bottom() <= bounds.bottom() + 0.001);
    }

    #[test]
    fn x_preview_uses_the_preset_ratio_above_the_minimum_seam() {
        let layout = SnsSplitLayout::centered_max(SnsTarget::X, 4, [2400, 1800]);
        let frames = layout.frames();
        let bounds = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(400.0, 200.0));
        let rects = sns_split_preview_rects(bounds, &frames, layout.seam_ratio());

        for pair in rects.windows(2) {
            let gap = pair[1].left() - pair[0].right();
            assert!(gap > MIN_PREVIEW_SEAM);
            assert!((gap / pair[0].width() - layout.seam_ratio()).abs() < 0.0001);
        }
        assert!(rects[0].left() >= bounds.left() - 0.001);
        assert!(rects.last().unwrap().right() <= bounds.right() + 0.001);
    }

    #[test]
    fn instagram_preview_rects_have_zero_gap() {
        let layout = SnsSplitLayout::centered_max(SnsTarget::Instagram, 3, [2400, 1800]);
        let frames = layout.frames();
        let bounds = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(216.0, PREVIEW_H));
        let rects = sns_split_preview_rects(bounds, &frames, layout.seam_ratio());

        assert_eq!(rects.len(), 3);
        for pair in rects.windows(2) {
            assert!((pair[1].left() - pair[0].right()).abs() < 0.001);
        }
    }

    #[test]
    fn target_descriptions_explain_default_seam_without_a_connection_guarantee() {
        let x = sns_split_target_description(SnsTarget::X);
        let instagram = sns_split_target_description(SnsTarget::Instagram);

        assert_eq!(x, "継ぎ目の既定は枠幅の1.7%");
        assert_eq!(instagram, "継ぎ目の既定は「なし」");
        assert!(!x.contains("必ず"));
        assert!(!instagram.contains("必ず"));
    }

    #[test]
    fn preview_uvs_convert_source_frames_to_unit_rects() {
        let frames = [
            CropRect {
                min_x: 100.0,
                min_y: 50.0,
                max_x: 400.0,
                max_y: 450.0,
            },
            CropRect {
                min_x: 400.0,
                min_y: -10.0,
                max_x: 1100.0,
                max_y: 510.0,
            },
        ];

        let uvs = sns_split_frame_uvs(&frames, [1000, 500]);

        assert_eq!(
            uvs[0],
            egui::Rect::from_min_max(egui::pos2(0.1, 0.1), egui::pos2(0.4, 0.9))
        );
        assert_eq!(
            uvs[1],
            egui::Rect::from_min_max(egui::pos2(0.4, 0.0), egui::pos2(1.0, 1.0))
        );
    }

    #[test]
    fn target_and_count_controls_follow_geometry_contract() {
        let image_size = [8000, 6000];
        let layout = SnsSplitLayout::centered_max(SnsTarget::X, 2, image_size)
            .with_target(SnsTarget::Instagram, image_size)
            .with_count(4, image_size);

        assert_eq!(layout.target, SnsTarget::Instagram);
        assert_eq!(layout.frames().len(), 4);
        assert_eq!(layout.group, CropRect::full(image_size[0], image_size[1]));
        assert_eq!(layout.seam_permille, 0);
    }

    #[test]
    fn panel_controls_wire_choices_fit_and_custom_editor_to_live_state() {
        use egui_kittest::{Harness, kittest::Queryable};
        use std::cell::RefCell;
        use std::rc::Rc;

        let image_size = [832, 1216];
        let mut app = crate::app::setup_app_for_test();
        app.settings.sns_split_target = Some("x".to_owned());
        app.settings.sns_split_count = 3;
        app.settings.sns_split_seam_permille = 17;
        app.settings.sns_split_frame_ratio = Some("free".to_owned());
        app.sns_split = Some(SnsSplitLayout::centered_max(SnsTarget::X, 3, image_size));
        let app = Rc::new(RefCell::new(app));
        let app_for_ui = Rc::clone(&app);
        let mut fonts_ready = false;
        let mut harness = Harness::builder()
            .with_size(egui::vec2(360.0, 700.0))
            .build(move |ctx| {
                if !fonts_ready {
                    crate::ui_fonts::configure_fonts(ctx);
                    fonts_ready = true;
                    ctx.request_repaint();
                    return;
                }
                egui::CentralPanel::default().show(ctx, |ui| {
                    ui.set_max_width(320.0);
                    let _ = app_for_ui
                        .borrow_mut()
                        .draw_sns_split_controls(ui, image_size, None);
                });
            });
        harness.run();

        assert!(
            harness
                .query_by_role(egui::accesskit::Role::SpinButton)
                .is_none(),
            "preset seams must not show the custom numeric editor"
        );
        harness.get_by_label("任意").click();
        harness.run();
        assert_eq!(app.borrow().sns_split.unwrap().seam_permille, 10);
        assert_eq!(app.borrow().settings.sns_split_seam_permille, 10);
        assert!(
            harness
                .query_by_role(egui::accesskit::Role::SpinButton)
                .is_some(),
            "custom seam must show the numeric editor"
        );

        let spin_center = harness
            .get_by_role(egui::accesskit::Role::SpinButton)
            .rect()
            .center();
        harness.hover_at(spin_center);
        harness.drag_at(spin_center);
        harness.run();
        let preset_crossing = spin_center + egui::vec2(7.0, 0.0);
        harness.hover_at(preset_crossing);
        harness.run();
        assert_eq!(app.borrow().sns_split.unwrap().seam_permille, 17);
        assert!(
            harness
                .query_by_role(egui::accesskit::Role::SpinButton)
                .is_some(),
            "custom editor must stay mounted while its drag crosses 1.7%"
        );
        let beyond_preset = spin_center + egui::vec2(8.0, 0.0);
        harness.hover_at(beyond_preset);
        harness.run();
        harness.drop_at(beyond_preset);
        harness.run();
        assert!(app.borrow().sns_split.unwrap().seam_permille > 17);
        assert_eq!(
            app.borrow().settings.sns_split_seam_permille,
            app.borrow().sns_split.unwrap().seam_permille
        );

        harness.get_by_label("Instagram").click();
        harness.run();
        assert_eq!(app.borrow().sns_split.unwrap().target, SnsTarget::Instagram);
        assert_eq!(app.borrow().sns_split.unwrap().seam_permille, 0);
        assert_eq!(
            app.borrow().settings.sns_split_target.as_deref(),
            Some("instagram")
        );
        assert_eq!(app.borrow().settings.sns_split_seam_permille, 0);
        assert!(
            harness
                .query_by_role(egui::accesskit::Role::SpinButton)
                .is_none()
        );

        harness.get_by_label("4").click();
        harness.run();
        harness.get_by_label("1:1").click();
        harness.run();
        assert_eq!(app.borrow().settings.sns_split_count, 4);
        assert_eq!(
            app.borrow().settings.sns_split_frame_ratio.as_deref(),
            Some("1:1")
        );

        app.borrow_mut().sns_split.as_mut().unwrap().group = CropRect {
            min_x: 216.0,
            min_y: 558.0,
            max_x: 616.0,
            max_y: 658.0,
        };
        harness.run();
        harness.get_by_label("画像全体に合わせる").click();
        harness.run();
        let fitted = app.borrow().sns_split.unwrap();
        assert_eq!(fitted.group.width(), image_size[0] as f32);
        assert_eq!(
            [fitted.frames()[0].width(), fitted.group.height()],
            [208.0, 208.0]
        );
        assert!(harness.query_by_label("1 枚 208 x 208 (1.00)").is_some());
    }

    #[test]
    fn fixed_frame_ratio_changes_preserve_width_and_do_not_accumulate_shrinkage() {
        let image_size = [2000, 1600];
        let base = SnsSplitLayout {
            target: SnsTarget::X,
            count: 3,
            seam_permille: 17,
            group: CropRect {
                min_x: 200.0,
                min_y: 300.0,
                max_x: 1800.0,
                max_y: 1300.0,
            },
        }
        .clamped(image_size);
        let center_x = (base.group.min_x + base.group.max_x) * 0.5;

        let portrait = transition_sns_frame_ratio(
            base,
            base,
            SnsFrameRatio::Free,
            SnsFrameRatio::Ratio3x4,
            image_size,
        );
        let square = transition_sns_frame_ratio(
            portrait,
            portrait,
            SnsFrameRatio::Ratio3x4,
            SnsFrameRatio::Ratio1x1,
            image_size,
        );
        let portrait_again = transition_sns_frame_ratio(
            square,
            square,
            SnsFrameRatio::Ratio1x1,
            SnsFrameRatio::Ratio3x4,
            image_size,
        );

        for layout in [portrait, square, portrait_again] {
            assert_eq!(layout.group.width(), base.group.width());
            assert!(((layout.group.min_x + layout.group.max_x) * 0.5 - center_x).abs() < 0.01);
        }
        assert_eq!(portrait_again.group, portrait.group);
    }

    #[test]
    fn fixed_topology_round_trip_restores_height_limited_layout() {
        let image_size = [1000, 300];
        let ratio = SnsFrameRatio::Ratio3x4;
        let four = fit_sns_split_to_full_image(
            SnsSplitLayout::centered_max(SnsTarget::X, 4, image_size),
            ratio,
            image_size,
        );
        assert_eq!(four.group.height(), image_size[1] as f32);

        let three_geometry = four.with_count(3, image_size);
        let three = transition_sns_frame_ratio(four, three_geometry, ratio, ratio, image_size);
        assert_eq!(three.group.height(), image_size[1] as f32);
        assert!(three.group.width() < four.group.width());

        let four_geometry = three.with_count(4, image_size);
        let restored = transition_sns_frame_ratio(three, four_geometry, ratio, ratio, image_size);
        assert_eq!(
            [restored.group.width(), restored.group.height()],
            [four.group.width(), four.group.height()]
        );
        assert!((restored.group.min_x - four.group.min_x).abs() <= 1.0);
        assert!((restored.group.max_x - four.group.max_x).abs() <= 1.0);

        let mut repeated = restored;
        for _ in 0..20 {
            let three_geometry = repeated.with_count(3, image_size);
            let three =
                transition_sns_frame_ratio(repeated, three_geometry, ratio, ratio, image_size);
            let four_geometry = three.with_count(4, image_size);
            repeated = transition_sns_frame_ratio(three, four_geometry, ratio, ratio, image_size);
        }
        assert_eq!(repeated.group, restored.group);
    }

    #[test]
    fn fixed_square_high_seam_count_round_trip_is_stable() {
        let ratio = SnsFrameRatio::Ratio1x1;
        for image_size in [[1000, 300], [999, 301]] {
            let three = fit_sns_split_to_full_image(
                SnsSplitLayout::centered_max(SnsTarget::X, 3, image_size)
                    .with_seam_permille(100, image_size),
                ratio,
                image_size,
            );
            assert_eq!(three.group.height(), image_size[1] as f32);

            let mut repeated = three;
            let mut stable_four = None;
            for _ in 0..20 {
                let four_geometry = repeated.with_count(4, image_size);
                let four =
                    transition_sns_frame_ratio(repeated, four_geometry, ratio, ratio, image_size);
                let maximal_four = fit_sns_split_to_full_image(four_geometry, ratio, image_size);
                assert_eq!(
                    [four.group.width(), four.group.height()],
                    [maximal_four.group.width(), maximal_four.group.height()]
                );
                if let Some(stable_four) = stable_four {
                    assert_eq!(four.group, stable_four);
                } else {
                    stable_four = Some(four.group);
                }

                let three_geometry = four.with_count(3, image_size);
                repeated =
                    transition_sns_frame_ratio(four, three_geometry, ratio, ratio, image_size);
                assert_eq!(repeated.group, three.group);
            }
        }
    }

    #[test]
    fn stable_axis_room_matches_even_and_odd_pixel_centering() {
        assert_eq!(stable_axis_room(500.5, 1000), 1000);
        assert_eq!(stable_axis_room(499.5, 999), 999);
        assert_eq!(stable_axis_room(250.5, 1000), 501);
    }

    #[test]
    fn fixed_square_incremental_seam_round_trip_does_not_shrink() {
        let image_size = [1000, 300];
        let ratio = SnsFrameRatio::Ratio1x1;
        let initial = fit_sns_split_to_full_image(
            SnsSplitLayout::centered_max(SnsTarget::X, 3, image_size)
                .with_seam_permille(10, image_size),
            ratio,
            image_size,
        );
        let mut layout = initial;

        for seam_permille in 11..=100 {
            let geometry = layout.with_seam_permille(seam_permille, image_size);
            layout = transition_sns_frame_ratio(layout, geometry, ratio, ratio, image_size);
            assert_eq!(
                layout.group,
                fit_sns_split_to_full_image(geometry, ratio, image_size).group,
                "seam {seam_permille} must keep a maximally fitted crop"
            );
        }
        for seam_permille in (10..100).rev() {
            let geometry = layout.with_seam_permille(seam_permille, image_size);
            layout = transition_sns_frame_ratio(layout, geometry, ratio, ratio, image_size);
            assert_eq!(
                layout.group,
                fit_sns_split_to_full_image(geometry, ratio, image_size).group,
                "seam {seam_permille} must keep a maximally fitted crop"
            );
        }

        assert_eq!(layout.group, initial.group);
    }

    #[test]
    fn off_center_fixed_count_round_trip_stabilizes_after_integer_quantization() {
        let image_size = [1600, 269];
        let ratio = SnsFrameRatio::Ratio4x5;
        let initial = SnsSplitLayout {
            target: SnsTarget::X,
            count: 4,
            seam_permille: 89,
            group: CropRect {
                min_x: 488.0,
                min_y: 0.0,
                max_x: 1364.0,
                max_y: 256.0,
            },
        }
        .clamped(image_size);
        let center_cell = |layout: SnsSplitLayout| {
            [
                ((layout.group.min_x + layout.group.max_x) * 0.5).floor() as usize,
                ((layout.group.min_y + layout.group.max_y) * 0.5).floor() as usize,
            ]
        };
        let transition_count = |layout: SnsSplitLayout, count| {
            let geometry = layout.with_count(count, image_size);
            transition_sns_frame_ratio(layout, geometry, ratio, ratio, image_size)
        };

        let two = transition_count(initial, 2);
        let baseline = transition_count(two, 4);
        assert_eq!(center_cell(baseline), center_cell(initial));
        assert_eq!(
            [baseline.group.width(), baseline.group.height()],
            [877.0, 257.0]
        );

        let mut repeated = baseline;
        for _ in 0..20 {
            let next_two = transition_count(repeated, 2);
            assert_eq!(next_two.group, two.group);
            repeated = transition_count(next_two, 4);
            assert_eq!(repeated.group, baseline.group);
            assert_eq!(center_cell(repeated), center_cell(initial));
        }
    }

    #[test]
    fn fit_to_full_image_is_exact_when_free_and_maximal_when_fixed() {
        let image_size = [832, 1216];
        let base = SnsSplitLayout::centered_max(SnsTarget::Instagram, 3, image_size);

        let free = fit_sns_split_to_full_image(base, SnsFrameRatio::Free, image_size);
        assert_eq!(free.group, CropRect::full(image_size[0], image_size[1]));

        let fixed = fit_sns_split_to_full_image(base, SnsFrameRatio::Ratio1x1, image_size);
        let expected_aspect = SnsFrameRatio::Ratio1x1
            .group_aspect(fixed.count, fixed.seam_permille)
            .unwrap();
        assert_eq!(fixed.group.width(), image_size[0] as f32);
        assert!((fixed.group.width() / fixed.group.height() - expected_aspect).abs() < 0.01);
        assert!(fixed.fits(image_size));

        let lower_boundary = fit_sns_split_to_full_image(base, SnsFrameRatio::Ratio3x4, image_size);
        let (_, _, frame_width, frame_height) =
            lower_boundary.frames()[0].pixel_bounds(image_size[0], image_size[1]);
        assert_eq!([frame_width, frame_height], [277, 369]);
        assert!(
            frame_width as u128 * 4 >= frame_height as u128 * 3,
            "integer snapping must not put the 3:4 preset below Instagram's lower boundary"
        );
    }

    #[test]
    fn fixed_presets_stay_inside_instagram_limit_after_integer_rounding() {
        let cases = [
            ([834, 1000], 3, SnsFrameRatio::Ratio3x4, [278, 370]),
            ([4, 3], 2, SnsFrameRatio::Ratio4x5, [2, 2]),
            (
                [12_582_916, 8_388_611],
                2,
                SnsFrameRatio::Ratio3x4,
                [6_291_458, 8_388_610],
            ),
        ];

        for (image_size, count, ratio, expected_frame_size) in cases {
            let layout = fit_sns_split_to_full_image(
                SnsSplitLayout::centered_max(SnsTarget::Instagram, count, image_size),
                ratio,
                image_size,
            );
            let (_, _, frame_width, frame_height) =
                layout.frames()[0].pixel_bounds(image_size[0], image_size[1]);

            assert_eq!([frame_width, frame_height], expected_frame_size);
            assert!(frame_width as u128 * 4 >= frame_height as u128 * 3);
            assert_eq!(sns_split_export_disabled_reason(layout, image_size), None);
        }
    }

    #[test]
    fn fixed_frame_ratio_drag_preserves_aspect_and_p6_anchors_for_all_handles() {
        let image_size = [1000, 800];
        let base = SnsSplitLayout {
            target: SnsTarget::Instagram,
            count: 2,
            seam_permille: 0,
            group: CropRect {
                min_x: 200.0,
                min_y: 200.0,
                max_x: 600.0,
                max_y: 400.0,
            },
        }
        .clamped(image_size);
        let cases = [
            (CropHandle::North, 0.0, -50.0, 500.0),
            (CropHandle::South, 0.0, 50.0, 500.0),
            (CropHandle::West, -100.0, 0.0, 500.0),
            (CropHandle::East, 100.0, 0.0, 500.0),
            (CropHandle::NorthWest, -80.0, -20.0, 480.0),
            (CropHandle::NorthEast, 80.0, -20.0, 480.0),
            (CropHandle::SouthWest, -80.0, 20.0, 480.0),
            (CropHandle::SouthEast, 80.0, 20.0, 480.0),
        ];
        let base_center_x = (base.group.min_x + base.group.max_x) * 0.5;
        let base_center_y = (base.group.min_y + base.group.max_y) * 0.5;

        for (handle, delta_x, delta_y, expected_width) in cases {
            let next = drag_sns_split_layout(
                base,
                handle,
                delta_x,
                delta_y,
                image_size,
                SnsFrameRatio::Ratio1x1,
            );
            assert!(
                (next.group.width() / next.group.height() - 2.0).abs() < 0.001,
                "{handle:?} must preserve the two-frame group aspect: {:?}",
                next.group
            );
            assert_eq!(
                [next.group.width(), next.group.height()],
                [expected_width, expected_width / 2.0],
                "{handle:?} must apply both pointer axes through the fixed-aspect path"
            );
            assert!(next.fits(image_size), "{handle:?} must remain in bounds");

            let next_center_x = (next.group.min_x + next.group.max_x) * 0.5;
            let next_center_y = (next.group.min_y + next.group.max_y) * 0.5;
            match handle {
                CropHandle::North => {
                    assert_eq!(next.group.max_y, base.group.max_y);
                    assert_eq!(next_center_x, base_center_x);
                }
                CropHandle::South => {
                    assert_eq!(next.group.min_y, base.group.min_y);
                    assert_eq!(next_center_x, base_center_x);
                }
                CropHandle::West => {
                    assert_eq!(next.group.max_x, base.group.max_x);
                    assert_eq!(next_center_y, base_center_y);
                }
                CropHandle::East => {
                    assert_eq!(next.group.min_x, base.group.min_x);
                    assert_eq!(next_center_y, base_center_y);
                }
                CropHandle::NorthWest => {
                    assert_eq!(next.group.max_x, base.group.max_x);
                    assert_eq!(next.group.max_y, base.group.max_y);
                }
                CropHandle::NorthEast => {
                    assert_eq!(next.group.min_x, base.group.min_x);
                    assert_eq!(next.group.max_y, base.group.max_y);
                }
                CropHandle::SouthWest => {
                    assert_eq!(next.group.max_x, base.group.max_x);
                    assert_eq!(next.group.min_y, base.group.min_y);
                }
                CropHandle::SouthEast => {
                    assert_eq!(next.group.min_x, base.group.min_x);
                    assert_eq!(next.group.min_y, base.group.min_y);
                }
                CropHandle::Body => unreachable!(),
            }
        }
    }

    #[test]
    fn free_frame_ratio_keeps_independent_edge_dragging() {
        let image_size = [1000, 800];
        let base = SnsSplitLayout {
            target: SnsTarget::Instagram,
            count: 2,
            seam_permille: 0,
            group: CropRect {
                min_x: 200.0,
                min_y: 200.0,
                max_x: 600.0,
                max_y: 400.0,
            },
        }
        .clamped(image_size);

        let next = drag_sns_split_layout(
            base,
            CropHandle::East,
            80.0,
            50.0,
            image_size,
            SnsFrameRatio::Free,
        );

        assert_eq!(next.group.width(), 480.0);
        assert_eq!(next.group.height(), 200.0);
        assert_eq!(next.group.min_x, base.group.min_x);
    }

    #[test]
    fn panel_summary_reports_the_first_measured_frame_and_aspect() {
        let image_size = [832, 1216];
        let layout = SnsSplitLayout::centered_max(SnsTarget::X, 3, image_size)
            .with_seam_permille(0, image_size);
        let summary = SnsSplitPanelSummary::from_layout(layout, image_size);

        assert_eq!(summary.dimensions, "1 枚 277 x 1216 (0.23)");
        assert_eq!(summary.warning, None);
    }

    #[test]
    fn instagram_ratio_below_three_four_is_rejected() {
        let image_size = [598, 400];
        let layout = SnsSplitLayout::centered_max(SnsTarget::Instagram, 2, image_size);

        assert_eq!(
            sns_split_export_disabled_reason(layout, image_size),
            Some(SNS_SPLIT_INSTAGRAM_RATIO_REASON)
        );
    }

    #[test]
    fn instagram_ratio_above_one_point_nine_one_is_rejected() {
        let image_size = [384, 100];
        let layout = SnsSplitLayout::centered_max(SnsTarget::Instagram, 2, image_size);

        assert_eq!(
            sns_split_export_disabled_reason(layout, image_size),
            Some(SNS_SPLIT_INSTAGRAM_RATIO_REASON)
        );
    }

    #[test]
    fn instagram_three_four_boundary_is_allowed() {
        let image_size = [600, 400];
        let layout = SnsSplitLayout::centered_max(SnsTarget::Instagram, 2, image_size);

        assert_eq!(sns_split_export_disabled_reason(layout, image_size), None);
    }

    #[test]
    fn instagram_one_point_nine_one_boundary_is_allowed() {
        let image_size = [382, 100];
        let layout = SnsSplitLayout::centered_max(SnsTarget::Instagram, 2, image_size);

        assert_eq!(sns_split_export_disabled_reason(layout, image_size), None);
    }

    #[test]
    fn instagram_ratio_check_uses_the_first_actual_frame_when_widths_differ() {
        let image_size = [574, 100];
        let layout = SnsSplitLayout::centered_max(SnsTarget::Instagram, 3, image_size);
        let frames = layout.frames();

        assert_eq!(
            frames
                .iter()
                .map(|frame| frame.pixel_bounds(image_size[0], image_size[1]).2)
                .collect::<Vec<_>>(),
            vec![191, 192, 191]
        );
        assert_eq!(sns_split_export_disabled_reason(layout, image_size), None);
    }

    #[test]
    fn x_allows_the_same_extreme_frame_ratios() {
        for image_size in [[598, 400], [384, 100]] {
            let layout = SnsSplitLayout::centered_max(SnsTarget::X, 2, image_size)
                .with_seam_permille(0, image_size);
            assert_eq!(
                sns_split_export_disabled_reason(layout, image_size),
                None,
                "X must remain the unrestricted escape path for {image_size:?}"
            );
        }
    }

    #[test]
    fn too_small_reason_keeps_priority_over_instagram_ratio() {
        let image_size = [3, 3];
        let layout = SnsSplitLayout::centered_max(SnsTarget::Instagram, 4, image_size);

        assert_eq!(
            sns_split_export_disabled_reason(layout, image_size),
            Some(SNS_SPLIT_TOO_SMALL_REASON)
        );
    }

    #[test]
    fn panel_summary_uses_the_shared_instagram_ratio_reason() {
        let image_size = [598, 400];
        let layout = SnsSplitLayout::centered_max(SnsTarget::Instagram, 2, image_size);
        let summary = SnsSplitPanelSummary::from_layout(layout, image_size);

        assert_eq!(summary.warning, Some(SNS_SPLIT_INSTAGRAM_RATIO_REASON));
    }

    #[test]
    fn panel_summary_blocks_export_when_frames_do_not_fit() {
        let image_size = [3, 3];
        let layout = SnsSplitLayout::centered_max(SnsTarget::X, 4, image_size);
        let summary = SnsSplitPanelSummary::from_layout(layout, image_size);

        assert!(summary.warning.is_some());
        assert!(summary.warning.unwrap().contains("書き出しには進めません"));
        assert_eq!(summary.dimensions, "1 枚 1 x 3 (0.33)");
    }

    #[test]
    fn execute_uses_the_shared_instagram_ratio_reason_and_keeps_the_mode() {
        let image_size = [598, 400];
        let layout = SnsSplitLayout::centered_max(SnsTarget::Instagram, 2, image_size);
        let mut app = crate::app::setup_app_for_test();
        let path = app.tmp.path().join("instagram-ratio-not-loaded.png");
        app.items = vec![crate::grid_item::GridItem::Image(path)];
        app.fullscreen_idx = Some(0);
        app.fs_early_dims.insert(0, image_size);
        app.sns_split = Some(layout);

        let opened = app.execute_sns_split_export(&egui::Context::default(), 0);

        assert!(!opened);
        assert_eq!(app.sns_split, Some(layout));
        assert!(app.export_dialog.is_none());
        assert_eq!(
            app.fs_feedback_toast.as_ref().map(|toast| toast.0.as_str()),
            Some(SNS_SPLIT_INSTAGRAM_RATIO_REASON)
        );
    }

    #[test]
    fn default_ctrl_e_uses_normal_export_guard_and_guides_to_panel_button() {
        let _key_input_guard = crate::key_input::TEST_INPUT_LOCK
            .get_or_init(|| std::sync::Mutex::new(()))
            .lock()
            .expect("key input test lock poisoned");
        crate::key_input::clear_test_frame();

        let image_size = [3, 3];
        let layout = SnsSplitLayout::centered_max(SnsTarget::X, 4, image_size);
        assert!(!layout.fits(image_size));
        let mut app = crate::app::setup_app_for_test();
        app.fullscreen_idx = Some(0);
        app.fs_early_dims.insert(0, image_size);
        app.sns_split = Some(layout);

        let ctx = egui::Context::default();
        let modifiers = egui::Modifiers::CTRL;
        ctx.begin_pass(egui::RawInput {
            modifiers,
            events: vec![egui::Event::Key {
                key: egui::Key::E,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers,
            }],
            ..Default::default()
        });
        app.keyboard_owner_for_pass(&ctx);
        let _ = app.handle_sns_split_keys(&ctx, 0);
        assert!(
            !app.keymap.consume_action(&ctx, KeyAction::FsExport),
            "通常の Ctrl+E は SNS handler が export guard へ渡す"
        );
        let _ = ctx.end_pass();

        assert_eq!(app.sns_split, Some(layout));
        assert!(app.export_dialog.is_none());
        assert_eq!(
            app.fs_feedback_toast.as_ref().map(|toast| toast.0.as_str()),
            Some(SNS_SPLIT_EXPORT_BUTTON_GUIDANCE)
        );
    }

    #[test]
    fn non_sns_export_guard_keeps_the_existing_edit_mode_message() {
        let mut app = crate::app::setup_app_for_test();
        app.export_crop_mode = true;

        app.open_export_dialog_for_current(&egui::Context::default(), 0);

        assert_eq!(
            app.fs_feedback_toast.as_ref().map(|toast| toast.0.as_str()),
            Some("編集モードを閉じてからエクスポートしてください")
        );
        assert!(app.export_dialog.is_none());
    }

    #[test]
    fn execute_keeps_mode_when_export_snapshot_is_not_ready() {
        let image_size = [80, 120];
        let layout = SnsSplitLayout::centered_max(SnsTarget::X, 3, image_size);
        assert!(layout.fits(image_size));
        let mut app = crate::app::setup_app_for_test();
        let path = app.tmp.path().join("not-loaded.png");
        app.items = vec![crate::grid_item::GridItem::Image(path)];
        app.fullscreen_idx = Some(0);
        app.fs_early_dims.insert(0, image_size);
        app.sns_split = Some(layout);

        let opened = app.execute_sns_split_export(&egui::Context::default(), 0);

        assert!(!opened);
        assert_eq!(app.sns_split, Some(layout));
        assert!(app.export_dialog.is_none());
    }

    #[test]
    fn export_preflight_exits_mode_if_rotation_changed_after_entry() {
        let image_size = [80, 120];
        let layout = SnsSplitLayout::centered_max(SnsTarget::X, 3, image_size);
        let mut app = crate::app::setup_app_for_test();
        app.fullscreen_idx = Some(0);
        app.sns_split = Some(layout);
        app.rotation_cache
            .insert(0, crate::rotation_db::Rotation::Cw180);

        let opened = app.execute_sns_split_export(&egui::Context::default(), 0);

        assert!(!opened);
        assert!(app.sns_split.is_none());
        assert!(app.export_dialog.is_none());
        assert_eq!(
            app.fs_feedback_toast.as_ref().map(|toast| toast.0.as_str()),
            Some(SNS_SPLIT_ROTATION_DISABLED_REASON)
        );
    }

    #[test]
    fn custom_execute_shortcut_opens_numbered_single_page_export_from_spread() {
        let _key_input_guard = crate::key_input::TEST_INPUT_LOCK
            .get_or_init(|| std::sync::Mutex::new(()))
            .lock()
            .expect("key input test lock poisoned");
        crate::key_input::clear_test_frame();

        let mut app = crate::app::setup_app_for_test();
        let ctx = egui::Context::default();
        let image_size = [80, 120];
        let left_path = app.tmp.path().join("sns-left.png");
        let right_path = app.tmp.path().join("sns-right.png");
        app.items = vec![
            crate::grid_item::GridItem::Image(left_path),
            crate::grid_item::GridItem::Image(right_path),
        ];
        app.visible_indices = vec![0, 1];
        app.thumbnails = vec![
            crate::grid_item::ThumbnailState::Pending,
            crate::grid_item::ThumbnailState::Pending,
        ];
        for (idx, color) in [(0, egui::Color32::RED), (1, egui::Color32::BLUE)] {
            let image = egui::ColorImage::filled(image_size, color);
            let texture = ctx.load_texture(
                format!("sns_split_export_{idx}"),
                image.clone(),
                egui::TextureOptions::LINEAR,
            );
            app.fs_cache.insert(
                idx,
                crate::fs_animation::FsCacheEntry::Static {
                    tex: texture,
                    pixels: std::sync::Arc::new(image),
                    source_dims: Some(image_size),
                    load_seq: 0,
                    animation: crate::fs_animation::StaticAnimationState::Still,
                },
            );
        }
        app.fullscreen_idx = Some(0);
        app.spread_mode = crate::settings::SpreadMode::Ltr;
        app.settings.sns_split_target = Some(SnsTarget::X.stable_key().to_string());
        app.settings.sns_split_count = 3;
        let original_selection = [false, true, false, true, false];
        app.settings.export_batch_selection = original_selection;
        app.keymap = crate::keymap::Keymap::from_ini_str("[SnsSplit]\nSnsSplitExecute = F13\n");

        assert!(app.enter_sns_split_mode(0).is_ok());
        let expected_frames = app.sns_split.expect("SNS 分割が開始していない").frames();
        assert_eq!(expected_frames.len(), 3);
        assert_eq!(app.spread_mode, crate::settings::SpreadMode::Single);
        assert!(app.sns_split_spread_ctx.is_some());

        let modifiers = egui::Modifiers::NONE;
        ctx.begin_pass(egui::RawInput {
            modifiers,
            events: vec![egui::Event::Key {
                key: egui::Key::F13,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers,
            }],
            ..Default::default()
        });
        app.keyboard_owner_for_pass(&ctx);
        let _ = app.handle_sns_split_keys(&ctx, 0);
        assert!(
            !app.keymap.consume_action(&ctx, KeyAction::SnsSplitExecute),
            "利用者が割り当てた SNS 分割実行キーは handler が消費する"
        );
        let _ = ctx.end_pass();

        assert!(app.sns_split.is_none());
        assert!(app.sns_split_spread_ctx.is_none());
        assert_eq!(app.spread_mode, crate::settings::SpreadMode::Ltr);
        let state = app.export_dialog.take().expect("export dialog should open");
        // 分割の書き出しは投稿用なので、既定で長辺を抑える (通常書き出しの既定は不変)。
        assert_eq!(
            state.scale,
            crate::export_dialog::ExportScale::LongEdge(
                crate::export_dialog::ExportScale::DEFAULT_LONG_EDGE
            )
        );
        assert_eq!(state.sns_split_frames, expected_frames);
        assert_eq!(state.selection, original_selection);
        match state.composite {
            crate::export_dialog::ExportComposite::Single(page) => {
                assert_eq!(page.predicted_size, image_size);
                assert!(matches!(
                    page.source,
                    crate::books::CompositeSource::File { ref path }
                        if path == &app.tmp.path().join("sns-left.png")
                ));
            }
            crate::export_dialog::ExportComposite::Spread { .. } => {
                panic!("SNS 分割は編集対象の単ページだけを worker 合成へ渡す")
            }
        }
    }
}
