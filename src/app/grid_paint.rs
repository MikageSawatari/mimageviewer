//! Grid cell painting helpers for the main thumbnail/details view.

use eframe::egui;

use super::{draw_rotated_image, drive_display_label, is_miv_upscaled_derivative};
use crate::grid_item::{GridItem, ThumbnailState};
use crate::settings::{AudioThumbnailIndicator, VideoThumbnailIndicator};
use crate::thumb_overlay_layout::{
    BottomContainerInput, BottomContainerKind, EditBadgeFlags, FormatBadgeKind,
    ThumbnailOverlayLayout, ThumbnailOverlayLayoutInput, layout_thumbnail_overlays,
};
use crate::ui_helpers::draw_play_icon;

const VIDEO_THUMBNAIL_BADGE_LABEL: &str = "動画";

/// サムネイルテクスチャをアスペクト保持で中央配置して描画する（回転対応）。
fn draw_thumb_texture(
    painter: &egui::Painter,
    inner: egui::Rect,
    tex: &egui::TextureHandle,
    rotation: crate::rotation_db::Rotation,
) -> egui::Rect {
    let tex_size = tex.size_vec2();
    // 90°/270° 回転時は幅と高さが入れ替わる
    let display_size = match rotation {
        crate::rotation_db::Rotation::Cw90 | crate::rotation_db::Rotation::Cw270 => {
            egui::vec2(tex_size.y, tex_size.x)
        }
        _ => tex_size,
    };
    let scale = (inner.width() / display_size.x).min(inner.height() / display_size.y);
    let img_rect = egui::Rect::from_center_size(inner.center(), display_size * scale);

    // 透過画像の背景はフルスクリーンと同じ黒に揃える (v0.7.0 フィードバック反映)。
    // セル全体ではなく img_rect (実際に画像が描かれる領域) だけを塗るので、
    // フォルダラベルや letterbox の白背景は維持される。
    painter.rect_filled(img_rect, 0.0, egui::Color32::BLACK);

    if rotation.is_none() {
        painter.image(
            tex.id(),
            img_rect,
            egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
            egui::Color32::WHITE,
        );
    } else {
        // 回転したテクスチャを Mesh で描画
        draw_rotated_image(painter, tex.id(), img_rect, rotation);
    }
    img_rect
}

/// 画像系アイテム (Image / ZipImage) のサムネイル状態に応じた描画。
fn draw_thumb(
    painter: &egui::Painter,
    inner: egui::Rect,
    thumb: &ThumbnailState,
    rotation: crate::rotation_db::Rotation,
    dark: bool,
    adjusted_tex: Option<&egui::TextureHandle>,
    record: &mut impl FnMut(egui::Rect),
) {
    match thumb {
        ThumbnailState::Loaded { tex, .. } => {
            let use_tex = adjusted_tex.unwrap_or(tex);
            record(draw_thumb_texture(painter, inner, use_tex, rotation));
        }
        ThumbnailState::Pending | ThumbnailState::Evicted => {
            let bg = if dark {
                egui::Color32::from_gray(50)
            } else {
                egui::Color32::from_gray(220)
            };
            painter.rect_filled(inner, 2.0, bg);
            record(inner);
            record(paint_lower_caption(
                painter,
                inner.center(),
                egui::Align2::CENTER_CENTER,
                "読込中",
                egui::FontId::proportional(12.0),
                egui::Color32::from_gray(140),
                None,
                None,
            ));
        }
        ThumbnailState::NoArt | ThumbnailState::Failed => {
            let bg = if dark {
                egui::Color32::from_rgb(80, 30, 30)
            } else {
                egui::Color32::from_rgb(255, 220, 220)
            };
            let fg = if dark {
                egui::Color32::from_rgb(255, 160, 160)
            } else {
                egui::Color32::DARK_RED
            };
            painter.rect_filled(inner, 2.0, bg);
            record(inner);
            record(paint_lower_caption(
                painter,
                inner.center(),
                egui::Align2::CENTER_CENTER,
                "読込失敗",
                egui::FontId::proportional(12.0),
                fg,
                None,
                None,
            ));
        }
    }
}

/// ファイル名スタックの集約セル右上に「N 枚」バッジを描く (v2.0.0)。
/// スタックは複数枚画像をまとめた仮想コンテナなので、通常画像との見分けを付ける。
/// スタックはチェック非対象なので右上のチェックオーバーレイとは衝突しない。
fn draw_stack_count_badge(
    painter: &egui::Painter,
    placement: &crate::thumb_overlay_layout::BadgePlacement,
) {
    let badge_rect = placement.rect;
    painter.rect_filled(
        badge_rect,
        4.0,
        egui::Color32::from_rgba_unmultiplied(0, 0, 0, 190),
    );
    let galley = painter.layout_no_wrap(
        placement.text.clone(),
        egui::FontId::proportional(placement.style.font_size),
        egui::Color32::WHITE,
    );
    painter.galley(placement.text_pos(), galley, egui::Color32::WHITE);
}

/// What the bottom-left lane shows for one item: a container badge, a filename plate, or both.
///
/// Split out of [`layout_cell_overlays`] so the item-kind rules stay unit-testable without a
/// `Painter`. The rule that a folder falls back to a plain filename until its thumbnail is
/// loaded predates the lane rework and had its own tests; keeping the mapping pure keeps them.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct BottomLeftContent<'a> {
    pub container_kind: Option<BottomContainerKind>,
    pub container_label: Option<&'a str>,
    pub filename: Option<&'a str>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct VideoThumbnailIndicatorParts {
    pub play_icon: bool,
    pub bottom_left_badge: bool,
}

/// Resolve the two mutually exclusive video-thumbnail indicator surfaces from one setting.
/// Audio uses its own setting and loaded-image-aware indicator contract.
pub(crate) fn video_thumbnail_indicator_parts(
    item: &GridItem,
    indicator: VideoThumbnailIndicator,
) -> VideoThumbnailIndicatorParts {
    if !matches!(item, GridItem::Video(_)) {
        return VideoThumbnailIndicatorParts {
            play_icon: false,
            bottom_left_badge: false,
        };
    }
    match indicator.normalized() {
        VideoThumbnailIndicator::PlayIcon => VideoThumbnailIndicatorParts {
            play_icon: true,
            bottom_left_badge: false,
        },
        VideoThumbnailIndicator::BottomLeftBadge => VideoThumbnailIndicatorParts {
            play_icon: false,
            bottom_left_badge: true,
        },
        VideoThumbnailIndicator::Hidden => VideoThumbnailIndicatorParts {
            play_icon: false,
            bottom_left_badge: false,
        },
        VideoThumbnailIndicator::Unknown => unreachable!("normalized video thumbnail indicator"),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct AudioThumbnailIndicatorParts {
    pub music_icon: bool,
    pub bottom_left_badge: bool,
}

/// Settings control art markings only; missing art always keeps the fallback icon.
pub(crate) fn audio_thumbnail_indicator_parts(
    item: &GridItem,
    thumb: &ThumbnailState,
    indicator: AudioThumbnailIndicator,
) -> AudioThumbnailIndicatorParts {
    if !matches!(item, GridItem::Audio(_)) {
        return AudioThumbnailIndicatorParts {
            music_icon: false,
            bottom_left_badge: false,
        };
    }
    if !matches!(thumb, ThumbnailState::Loaded { .. }) {
        return AudioThumbnailIndicatorParts {
            music_icon: true,
            bottom_left_badge: false,
        };
    }
    AudioThumbnailIndicatorParts {
        music_icon: indicator.normalized() == AudioThumbnailIndicator::MusicNoteIcon,
        bottom_left_badge: indicator.normalized() == AudioThumbnailIndicator::BottomLeftBadge,
    }
}

/// Shared by grid, cut/deferred painting and details thumbnail preview.
pub(crate) fn paint_audio_thumbnail_indicator(
    painter: &egui::Painter,
    rect: egui::Rect,
    dark: bool,
    parts: AudioThumbnailIndicatorParts,
) -> [Option<egui::Rect>; 2] {
    let music = parts
        .music_icon
        .then(|| crate::ui_helpers::draw_music_icon(painter, rect, dark));
    let mut badge_area = None;
    if parts.bottom_left_badge {
        let galley = painter.layout_no_wrap(
            "音声".to_owned(),
            egui::FontId::proportional(11.0),
            egui::Color32::WHITE,
        );
        let badge = egui::Rect::from_min_size(
            rect.left_bottom() - egui::vec2(0.0, galley.size().y + 4.0),
            galley.size() + egui::vec2(8.0, 4.0),
        );
        painter.rect_filled(
            badge,
            2.0,
            crate::ui_helpers::format_badge_background(FormatBadgeKind::Audio),
        );
        let ink = paint_lower_caption_galley(
            painter,
            badge.min + egui::vec2(4.0, 2.0),
            galley,
            egui::Color32::WHITE,
            None,
            None,
        );
        badge_area = Some(badge.union(ink));
    }
    [music, badge_area]
}

pub(crate) fn bottom_left_content<'a>(
    item: &GridItem,
    thumb: &ThumbnailState,
    item_name: &'a str,
    video_indicator: VideoThumbnailIndicator,
    audio_indicator: AudioThumbnailIndicator,
) -> BottomLeftContent<'a> {
    let mut container_kind = None;
    let mut container_label = None;
    let mut filename = None;
    match item {
        GridItem::Folder(_) => {
            if matches!(thumb, ThumbnailState::Loaded { .. }) {
                container_kind = Some(BottomContainerKind::Folder);
                container_label = Some(item_name);
            } else {
                filename = Some(item_name);
            }
        }
        GridItem::Video(_) => {
            if video_thumbnail_indicator_parts(item, video_indicator).bottom_left_badge {
                container_kind = Some(BottomContainerKind::Format(FormatBadgeKind::Video));
                container_label = Some(VIDEO_THUMBNAIL_BADGE_LABEL);
            }
            filename = Some(item_name);
        }
        GridItem::Audio(_) => {
            if audio_thumbnail_indicator_parts(item, thumb, audio_indicator).bottom_left_badge {
                container_kind = Some(BottomContainerKind::Format(FormatBadgeKind::Audio));
                container_label = Some("音声");
            }
            filename = Some(item_name);
        }
        GridItem::ZipFile(_) => {
            container_kind = Some(BottomContainerKind::Format(FormatBadgeKind::Zip));
            container_label = Some("ZIP");
            filename = Some(item_name);
        }
        GridItem::PdfFile(path) => {
            container_kind = Some(BottomContainerKind::Format(FormatBadgeKind::Pdf));
            container_label = Some(
                if path
                    .extension()
                    .and_then(|ext| ext.to_str())
                    .is_some_and(|ext| ext.eq_ignore_ascii_case("epub"))
                {
                    "EPUB"
                } else {
                    "PDF"
                },
            );
            filename = Some(item_name);
        }
        GridItem::ConvertibleArchive { format, .. } => {
            container_kind = Some(BottomContainerKind::Format(FormatBadgeKind::Archive));
            container_label = Some(format.label());
            filename = Some(item_name);
        }
        GridItem::ZipDir { is_archive, .. } if *is_archive => {
            let extension = item_name.rsplit('.').next().unwrap_or_default();
            let nested_format =
                crate::archive_converter::ArchiveFormat::nested_from_extension(extension)
                    .filter(|format| *format != crate::archive_converter::ArchiveFormat::Zip);
            container_kind = Some(BottomContainerKind::Format(if nested_format.is_some() {
                FormatBadgeKind::Archive
            } else {
                FormatBadgeKind::Zip
            }));
            container_label = Some(nested_format.map_or("ZIP", |format| format.label()));
            filename = Some(item_name);
        }
        GridItem::ZipDir { .. } => {
            if matches!(thumb, ThumbnailState::Loaded { .. }) {
                container_kind = Some(BottomContainerKind::Folder);
                container_label = Some(item_name);
            } else {
                filename = Some(item_name);
            }
        }
        _ => {}
    }
    BottomLeftContent {
        container_kind,
        container_label,
        filename,
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn layout_cell_overlays(
    painter: &egui::Painter,
    rect: egui::Rect,
    edit_badges: EditBadgeFlags,
    rating: u8,
    item: &GridItem,
    thumb: &ThumbnailState,
    tags: &[String],
    bookmark_time: Option<&str>,
    is_drive_list: bool,
    video_indicator: VideoThumbnailIndicator,
    audio_indicator: AudioThumbnailIndicator,
    is_checked: bool,
    filter_match_count: Option<u32>,
    media_duration: Option<&str>,
    book_resume_meter: bool,
) -> ThumbnailOverlayLayout {
    let inner = rect.shrink(4.0);
    let item_name = match item {
        GridItem::Folder(path) if is_drive_list => {
            drive_display_label(path).unwrap_or_else(|| path.display().to_string())
        }
        GridItem::Folder(path)
        | GridItem::Video(path)
        | GridItem::Audio(path)
        | GridItem::ZipFile(path)
        | GridItem::PdfFile(path) => path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default()
            .to_owned(),
        GridItem::ConvertibleArchive { path, .. } => path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default()
            .to_owned(),
        GridItem::ZipDir { dir_prefix, .. } => {
            crate::grid_item::zipdir_display_name(dir_prefix).to_owned()
        }
        _ => String::new(),
    };

    let BottomLeftContent {
        container_kind: bottom_kind,
        container_label: bottom_label,
        filename,
    } = bottom_left_content(item, thumb, &item_name, video_indicator, audio_indicator);

    let rating_text = (1..=5).contains(&rating).then(|| {
        let stars = "★".repeat(rating as usize);
        if item.is_container_ratable() {
            format!("📁{stars}")
        } else {
            stars
        }
    });
    let bottom_container = bottom_kind
        .zip(bottom_label)
        .map(|(kind, label)| BottomContainerInput { kind, label });
    let upscaled_video = matches!(item, GridItem::Video(path) if is_miv_upscaled_derivative(path));

    layout_thumbnail_overlays(
        ThumbnailOverlayLayoutInput {
            cell: rect,
            inner,
            book_resume_meter,
            checked: is_checked,
            stack_count: match item {
                GridItem::Stack { count, .. } => Some(*count),
                _ => None,
            },
            filter_match_count: filter_match_count.filter(|_| item.is_container_ratable()),
            media_duration,
            bookmark_time,
            upscaled_video,
            edit_badges,
            tags,
            bottom_container,
            rating_text: rating_text.as_deref(),
            filename,
        },
        |text, style| crate::ui_helpers::measure_thumbnail_badge_text(painter, text, style),
    )
}

/// Snap the fixed reserved strip inward to physical pixels. No content-dependent placement.
fn thumbnail_resume_meter_rect(
    painter: &egui::Painter,
    layout: &ThumbnailOverlayLayout,
) -> Option<egui::Rect> {
    let raw = layout.book_resume_meter?;
    let ppp = painter.ctx().pixels_per_point();
    let bottom = (raw.max.y * ppp).floor() / ppp;
    let rect = egui::Rect::from_min_max(
        egui::pos2(
            (raw.min.x * ppp).ceil() / ppp,
            bottom - (crate::thumb_overlay_layout::BOOK_RESUME_METER_HEIGHT * ppp).floor() / ppp,
        ),
        egui::pos2((raw.max.x * ppp).floor() / ppp, bottom),
    );
    (rect.width() >= 1.0 / ppp && rect.height() >= 1.0 / ppp).then_some(rect)
}

fn thumbnail_badge_ink_rect(
    painter: &egui::Painter,
    placement: &crate::thumb_overlay_layout::BadgePlacement,
) -> egui::Rect {
    use egui::emath::GuiRounding;
    let galley = painter.layout_no_wrap(
        placement.text.clone(),
        crate::ui_helpers::thumbnail_badge_font(placement.style),
        egui::Color32::WHITE,
    );
    // Match epaint's physical-pixel rounding of the galley origin. mesh_bounds
    // includes the rendered glyph quads, rather than the nominal line height.
    galley.mesh_bounds.translate(
        placement
            .text_pos()
            .round_to_pixels(painter.ctx().pixels_per_point())
            .to_vec2(),
    )
}

// Custom lower captions follow the common reservation. In a small cell omit a
// caption that no longer fits, rather than moving it or the fixed primary icon.
fn paint_lower_caption_galley(
    painter: &egui::Painter,
    pos: egui::Pos2,
    galley: std::sync::Arc<egui::Galley>,
    color: egui::Color32,
    marker: Option<egui::Rect>,
    band: Option<(egui::Rect, egui::Rect)>,
) -> egui::Rect {
    use egui::emath::GuiRounding;
    let ink = galley.mesh_bounds.translate(
        pos.round_to_pixels(painter.ctx().pixels_per_point())
            .to_vec2(),
    );
    if band.is_none_or(|(band, cell)| {
        cell.contains_rect(ink)
            && !band.intersects(ink)
            && marker.is_none_or(|marker| !marker.intersects(ink))
    }) {
        painter.galley(pos, galley, color);
        ink
    } else {
        egui::Rect::NOTHING
    }
}

#[allow(clippy::too_many_arguments)]
fn paint_lower_caption(
    painter: &egui::Painter,
    pos: egui::Pos2,
    anchor: egui::Align2,
    text: impl ToString,
    font: egui::FontId,
    color: egui::Color32,
    marker: Option<egui::Rect>,
    band: Option<(egui::Rect, egui::Rect)>,
) -> egui::Rect {
    let galley = painter.layout_no_wrap(text.to_string(), font, color);
    let pos = anchor.anchor_size(pos, galley.size()).min;
    paint_lower_caption_galley(painter, pos, galley, color, marker, band)
}

#[allow(clippy::too_many_arguments)]
fn paint_lower_path(
    painter: &egui::Painter,
    rect: egui::Rect,
    components: &[&str],
    color: egui::Color32,
    max_font: f32,
    min_font: f32,
    marker: Option<egui::Rect>,
    band: Option<(egui::Rect, egui::Rect)>,
) -> egui::Rect {
    let galley = crate::ui_helpers::layout_path_hierarchy(
        painter,
        components,
        color,
        rect.size(),
        max_font,
        min_font,
    );
    let size = galley.size();
    let pos = egui::pos2(
        rect.center().x - size.x * 0.5,
        rect.min.y + ((rect.height() - size.y).max(0.0)) * 0.5,
    );
    paint_lower_caption_galley(painter, pos, galley, color, marker, band)
}

/// Paint a valid saved-position fraction into the shared thumbnail strip, always left to right.
pub(crate) fn paint_thumbnail_resume_meter(
    ui: &egui::Ui,
    cell: egui::Rect,
    meter: Option<egui::Rect>,
    fraction: Option<f32>,
    is_cut: bool,
) {
    let (Some(rect), Some(fraction)) = (meter, fraction) else {
        return;
    };
    if !ui.is_rect_visible(cell) {
        return;
    }
    let mut painter = ui.painter().with_clip_rect(cell);
    if is_cut {
        painter.multiply_opacity(crate::cut_clipboard::CUT_CONTENT_OPACITY);
    }
    let palette = crate::os_theme::book_resume_meter_palette(ui.visuals().dark_mode);
    paint_resume_meter_shape(&painter, rect, fraction, palette);
}

fn paint_resume_meter_shape(
    painter: &egui::Painter,
    rect: egui::Rect,
    fraction: f32,
    palette: crate::os_theme::BookResumeMeterPalette,
) {
    painter.rect_filled(rect, 2.0, palette.track);
    let width = rect.width() * fraction;
    let fill = egui::Rect::from_min_max(rect.min, egui::pos2(rect.min.x + width, rect.max.y));
    painter.rect_filled(fill, 2.0, palette.fill);
    // A one-pixel outline would consume all of a thin strip's fill.
    if rect.height() * painter.ctx().pixels_per_point() >= 3.0 {
        painter.rect_stroke(
            rect,
            2.0,
            egui::Stroke::new(1.0 / painter.ctx().pixels_per_point(), palette.boundary),
            egui::StrokeKind::Inside,
        );
    }
}

fn draw_drive_icon(painter: &egui::Painter, inner: egui::Rect, dark: bool) -> egui::Rect {
    let side = inner.width().min(inner.height());
    let w = (side * 0.48).clamp(36.0, 72.0);
    let h = (side * 0.34).clamp(24.0, 48.0);
    let center = inner.center() - egui::vec2(0.0, 12.0);
    let body = egui::Rect::from_center_size(center, egui::vec2(w, h));
    let fill = if dark {
        egui::Color32::from_rgb(78, 88, 100)
    } else {
        egui::Color32::from_rgb(210, 218, 226)
    };
    let face = if dark {
        egui::Color32::from_rgb(48, 56, 66)
    } else {
        egui::Color32::from_rgb(244, 247, 250)
    };
    let stroke = if dark {
        egui::Color32::from_rgb(130, 145, 160)
    } else {
        egui::Color32::from_rgb(120, 134, 150)
    };
    painter.rect_filled(body, 5.0, fill);
    painter.rect_stroke(
        body,
        5.0,
        egui::Stroke::new(1.5, stroke),
        egui::StrokeKind::Middle,
    );
    let front = egui::Rect::from_min_max(
        egui::pos2(body.min.x + w * 0.12, body.center().y + h * 0.12),
        egui::pos2(body.max.x - w * 0.12, body.max.y - h * 0.16),
    );
    painter.rect_filled(front, 2.0, face);
    painter.line_segment(
        [
            egui::pos2(body.min.x + w * 0.18, body.min.y + h * 0.32),
            egui::pos2(body.max.x - w * 0.18, body.min.y + h * 0.32),
        ],
        egui::Stroke::new(1.4, stroke),
    );
    painter.circle_filled(
        egui::pos2(front.max.x - w * 0.10, front.center().y),
        (side * 0.025).clamp(2.0, 3.5),
        egui::Color32::from_rgb(70, 190, 120),
    );
    body.expand(0.75)
}

/// Frame-local item regions, produced by the painting owner in logical egui points.
/// Decorative selection backgrounds/borders are deliberately not item regions.
#[derive(Default)]
pub(crate) struct ThumbnailHitAreas {
    rects: Vec<egui::Rect>,
}

impl ThumbnailHitAreas {
    pub(crate) fn contains(&self, pos: egui::Pos2) -> bool {
        self.rects.iter().any(|rect| rect.contains(pos))
    }

    pub(crate) fn include(&mut self, area: egui::Rect) {
        if area.is_positive() {
            self.rects.push(area);
        }
    }
}

pub(crate) fn draw_cell(
    ui: &egui::Ui,
    rect: egui::Rect,
    is_selected: bool,
    is_checked: bool,
    is_spread_pair_cursor: bool,
    overlay_layout: &ThumbnailOverlayLayout,
    item: &GridItem,
    thumb: &ThumbnailState,
    rotation: crate::rotation_db::Rotation,
    // Some(tex) なら `ThumbnailState::Loaded.tex` の代わりにこちらを描画する
    // (色調補正済みサムネイルテクスチャ)。None または Loaded 以外なら生サムネ。
    adjusted_tex: Option<&egui::TextureHandle>,
    is_drive_list: bool,
    video_indicator: VideoThumbnailIndicator,
    audio_indicator: AudioThumbnailIndicator,
    is_cut: bool,
    resume_meter: Option<f32>,
) {
    let _ = paint_cell(
        ui,
        rect,
        is_selected,
        is_checked,
        is_spread_pair_cursor,
        overlay_layout,
        item,
        thumb,
        rotation,
        adjusted_tex,
        is_drive_list,
        video_indicator,
        audio_indicator,
        is_cut,
        resume_meter,
        false,
    );
}

pub(crate) fn draw_cell_with_hit_areas(
    ui: &egui::Ui,
    rect: egui::Rect,
    is_selected: bool,
    is_checked: bool,
    is_spread_pair_cursor: bool,
    overlay_layout: &ThumbnailOverlayLayout,
    item: &GridItem,
    thumb: &ThumbnailState,
    rotation: crate::rotation_db::Rotation,
    // Some(tex) なら `ThumbnailState::Loaded.tex` の代わりにこちらを描画する
    // (色調補正済みサムネイルテクスチャ)。None または Loaded 以外なら生サムネ。
    adjusted_tex: Option<&egui::TextureHandle>,
    is_drive_list: bool,
    video_indicator: VideoThumbnailIndicator,
    audio_indicator: AudioThumbnailIndicator,
    is_cut: bool,
    resume_meter: Option<f32>,
) -> ThumbnailHitAreas {
    paint_cell(
        ui,
        rect,
        is_selected,
        is_checked,
        is_spread_pair_cursor,
        overlay_layout,
        item,
        thumb,
        rotation,
        adjusted_tex,
        is_drive_list,
        video_indicator,
        audio_indicator,
        is_cut,
        resume_meter,
        true,
    )
}

fn paint_cell(
    ui: &egui::Ui,
    rect: egui::Rect,
    is_selected: bool,
    is_checked: bool,
    is_spread_pair_cursor: bool,
    overlay_layout: &ThumbnailOverlayLayout,
    item: &GridItem,
    thumb: &ThumbnailState,
    rotation: crate::rotation_db::Rotation,
    // Some(tex) なら `ThumbnailState::Loaded.tex` の代わりにこちらを描画する
    // (色調補正済みサムネイルテクスチャ)。None または Loaded 以外なら生サムネ。
    adjusted_tex: Option<&egui::TextureHandle>,
    is_drive_list: bool,
    video_indicator: VideoThumbnailIndicator,
    audio_indicator: AudioThumbnailIndicator,
    is_cut: bool,
    resume_meter: Option<f32>,
    collect_hit_areas: bool,
) -> ThumbnailHitAreas {
    let mut hit_areas = ThumbnailHitAreas::default();
    if !ui.is_rect_visible(rect) {
        return hit_areas;
    }
    let mut record = |area: egui::Rect| {
        if collect_hit_areas {
            let area = area.intersect(rect).intersect(ui.clip_rect());
            if area.is_positive() {
                hit_areas.rects.push(area);
            }
        }
    };

    let base_painter = ui.painter();
    // Labels and placeholders also belong to this cell, including at the 32pt width floor.
    let mut content_painter = base_painter.with_clip_rect(rect);
    let content_opacity = if is_cut {
        crate::cut_clipboard::CUT_CONTENT_OPACITY
    } else {
        1.0
    };
    content_painter.multiply_opacity(content_opacity.clamp(0.0, 1.0));
    let painter = &content_painter;
    let padding = 4.0;
    let inner = rect.shrink(padding);
    let defer_primary_markers =
        overlay_layout.book_resume_meter.is_some() && resume_meter.is_some();
    // Custom lower captions outside ThumbnailOverlayLayout follow the same setting-owned shift.
    let caption_shift = egui::vec2(
        0.0,
        if overlay_layout.book_resume_meter.is_some() {
            -crate::thumb_overlay_layout::BOOK_RESUME_METER_RESERVE
        } else {
            0.0
        },
    );

    let dark = ui.visuals().dark_mode;
    let name_text_color = if dark {
        egui::Color32::from_gray(210)
    } else {
        egui::Color32::from_gray(30)
    };
    let pending_placeholder_bg = if dark {
        egui::Color32::from_gray(50)
    } else {
        egui::Color32::from_gray(230)
    };

    // カーソル位置 (selected) もマルチ選択チェック済み (checked) も同じ青背景に。
    // カーソル位置を示す太い青枠は selected のときだけ付く (下の border 判定で分岐)。
    let bg = if is_selected || is_checked {
        if dark {
            egui::Color32::from_rgb(40, 70, 110)
        } else {
            egui::Color32::from_rgb(180, 210, 255)
        }
    } else if dark {
        egui::Color32::from_gray(28)
    } else {
        egui::Color32::WHITE
    };
    base_painter.rect_filled(rect, 2.0, bg);

    match item {
        GridItem::Folder(_) => match thumb {
            ThumbnailState::Loaded { tex, .. } => {
                let use_tex = adjusted_tex.unwrap_or(tex);
                record(draw_thumb_texture(painter, inner, use_tex, rotation));
            }
            ThumbnailState::Pending
            | ThumbnailState::Evicted
            | ThumbnailState::NoArt
            | ThumbnailState::Failed => {
                if is_drive_list {
                    record(draw_drive_icon(painter, inner, dark));
                } else {
                    record(paint_lower_caption(
                        painter,
                        inner.center() - egui::vec2(0.0, 14.0),
                        egui::Align2::CENTER_CENTER,
                        "📁",
                        egui::FontId::proportional(42.0),
                        egui::Color32::from_rgb(220, 170, 30),
                        None,
                        None,
                    ));
                }
            }
        },
        GridItem::Image(_) => {
            draw_thumb(
                painter,
                inner,
                thumb,
                rotation,
                dark,
                adjusted_tex,
                &mut record,
            );
        }
        GridItem::Video(_) => {
            match thumb {
                ThumbnailState::Loaded { tex, .. } => {
                    // 動画サムネは補正対象外 (adjusted_tex は常に None)
                    record(draw_thumb_texture(painter, inner, tex, rotation));
                }
                ThumbnailState::Pending | ThumbnailState::Evicted => {
                    painter.rect_filled(inner, 2.0, egui::Color32::from_gray(40));
                    record(inner);
                    record(paint_lower_caption(
                        painter,
                        inner.center(),
                        egui::Align2::CENTER_CENTER,
                        "動画",
                        egui::FontId::proportional(12.0),
                        egui::Color32::from_gray(160),
                        None,
                        None,
                    ));
                }
                ThumbnailState::NoArt | ThumbnailState::Failed => {
                    painter.rect_filled(inner, 2.0, egui::Color32::from_gray(40));
                    record(inner);
                }
            }
            if !defer_primary_markers
                && !is_cut
                && video_thumbnail_indicator_parts(item, video_indicator).play_icon
            {
                let r = (inner.width().min(inner.height()) * 0.18).max(10.0);
                draw_play_icon(painter, inner.center(), r);
                record(egui::Rect::from_center_size(
                    inner.center(),
                    egui::Vec2::splat(r * 2.0),
                ));
            }
        }
        GridItem::Audio(_) => {
            if let ThumbnailState::Loaded { tex, .. } = thumb {
                record(draw_thumb_texture(painter, inner, tex, rotation));
            } else {
                painter.rect_filled(inner, 2.0, pending_placeholder_bg);
                record(inner);
            }
            if !defer_primary_markers {
                let parts = audio_thumbnail_indicator_parts(item, thumb, audio_indicator);
                let mark_rect = if matches!(thumb, ThumbnailState::Loaded { .. }) {
                    egui::Rect::from_center_size(
                        inner.center(),
                        egui::vec2(48.0, 48.0).min(inner.size()),
                    )
                } else {
                    inner
                };
                for area in paint_audio_thumbnail_indicator(
                    painter,
                    mark_rect,
                    dark,
                    AudioThumbnailIndicatorParts {
                        bottom_left_badge: false,
                        ..parts
                    },
                )
                .into_iter()
                .flatten()
                {
                    record(area);
                }
            }
        }
        GridItem::ZipImage { .. } | GridItem::PdfPage { .. } => {
            draw_thumb(
                painter,
                inner,
                thumb,
                rotation,
                dark,
                adjusted_tex,
                &mut record,
            );
        }
        GridItem::ZipFile(_) | GridItem::PdfFile(_) => {
            let icon = if matches!(item, GridItem::ZipFile(_)) {
                "📦"
            } else {
                "📄"
            };
            match thumb {
                ThumbnailState::Loaded { tex, .. } => {
                    // ZipFile/PdfFile の代表サムネは補正対象外 (adjusted_tex は常に None)
                    record(draw_thumb_texture(painter, inner, tex, rotation));
                }
                ThumbnailState::Pending
                | ThumbnailState::Evicted
                | ThumbnailState::NoArt
                | ThumbnailState::Failed => {
                    painter.rect_filled(inner, 2.0, pending_placeholder_bg);
                    record(inner);
                    record(paint_lower_caption(
                        painter,
                        inner.center(),
                        egui::Align2::CENTER_CENTER,
                        icon,
                        egui::FontId::proportional(32.0),
                        egui::Color32::from_gray(120),
                        None,
                        None,
                    ));
                }
            }
        }
        GridItem::ConvertibleArchive { .. } => {
            // RAR / 7z / LZH: 有効な変換キャッシュ ZIP があれば、その先頭画像 / ピン画像を
            // 表示する。未変換・キャッシュ失効時は汎用アーカイブアイコンへフォールバック。
            match thumb {
                ThumbnailState::Loaded { tex, .. } => {
                    record(draw_thumb_texture(painter, inner, tex, rotation));
                }
                ThumbnailState::Pending
                | ThumbnailState::Evicted
                | ThumbnailState::NoArt
                | ThumbnailState::Failed => {
                    painter.rect_filled(inner, 2.0, pending_placeholder_bg);
                    record(inner);
                    record(paint_lower_caption(
                        painter,
                        inner.center(),
                        egui::Align2::CENTER_CENTER,
                        "🗜",
                        egui::FontId::proportional(32.0),
                        egui::Color32::from_gray(120),
                        None,
                        None,
                    ));
                }
            }
        }
        GridItem::ZipDir { is_archive, .. } => {
            // ネスト ZIP ツリーの子コンテナ (v1.3.0)。内側アーカイブは ZipFile 風
            // (📦 + ZIP バッジ + 下部ファイル名)、ただのサブフォルダは Folder 風
            // (📁 + フォルダ名バッジ) に描く。代表サムネがロード済みならそれを使う。
            if *is_archive {
                match thumb {
                    ThumbnailState::Loaded { tex, .. } => {
                        record(draw_thumb_texture(painter, inner, tex, rotation));
                    }
                    ThumbnailState::Pending
                    | ThumbnailState::Evicted
                    | ThumbnailState::NoArt
                    | ThumbnailState::Failed => {
                        painter.rect_filled(inner, 2.0, pending_placeholder_bg);
                        record(inner);
                        record(paint_lower_caption(
                            painter,
                            inner.center(),
                            egui::Align2::CENTER_CENTER,
                            "📦",
                            egui::FontId::proportional(32.0),
                            egui::Color32::from_gray(120),
                            None,
                            None,
                        ));
                    }
                }
            } else {
                match thumb {
                    ThumbnailState::Loaded { tex, .. } => {
                        record(draw_thumb_texture(painter, inner, tex, rotation));
                    }
                    ThumbnailState::Pending
                    | ThumbnailState::Evicted
                    | ThumbnailState::NoArt
                    | ThumbnailState::Failed => {
                        record(paint_lower_caption(
                            painter,
                            inner.center() - egui::vec2(0.0, 14.0),
                            egui::Align2::CENTER_CENTER,
                            "📁",
                            egui::FontId::proportional(42.0),
                            egui::Color32::from_rgb(220, 170, 30),
                            None,
                            None,
                        ));
                    }
                }
            }
        }
        GridItem::SearchContainer {
            path,
            kind,
            hit_count,
            representative,
        } => {
            let (icon, label_color) = match kind {
                crate::grid_item::SearchContainerKind::Folder => (
                    "📁",
                    if dark {
                        egui::Color32::from_gray(220)
                    } else {
                        egui::Color32::from_gray(60)
                    },
                ),
                crate::grid_item::SearchContainerKind::Zip => (
                    "📦",
                    if dark {
                        egui::Color32::from_rgb(220, 200, 150)
                    } else {
                        egui::Color32::from_rgb(130, 90, 30)
                    },
                ),
            };

            // 代表サムネがあって GPU テクスチャがロード済みなら、セル上部にサムネ、
            // 下部に少し背景色の付いたボックスで「フォルダ階層 + ヒット件数」を出す。
            // 未ロード / 代表サムネなしのときは従来どおりアイコン + 階層パスで埋める
            // (サムネが読み込まれるまでの placeholder)。
            let thumb_loaded =
                representative.is_some() && matches!(thumb, ThumbnailState::Loaded { .. });

            if thumb_loaded {
                let thumb_h = inner.height() * 0.62;
                let thumb_rect = egui::Rect::from_min_max(
                    inner.min,
                    egui::pos2(inner.max.x, inner.min.y + thumb_h),
                );
                if let ThumbnailState::Loaded { tex, .. } = thumb {
                    // 代表サムネは色調補正対象外 (adjusted_tex は常に None)
                    record(draw_thumb_texture(painter, thumb_rect, tex, rotation));
                }
                // 種別アイコン (小) を左上隅に重ねて Folder/ZIP を示す
                let badge_size = (thumb_rect.height() * 0.22).clamp(14.0, 28.0);

                // 下部の「少し背景色を付けたボックス」: ユーザー要望どおりフォルダ名を
                // サムネから切り離して読みやすくする。
                let label_rect = egui::Rect::from_min_max(
                    egui::pos2(inner.min.x, thumb_rect.max.y + 2.0),
                    inner.max,
                )
                .translate(caption_shift);
                let label_bg = if dark {
                    egui::Color32::from_rgb(38, 42, 50)
                } else {
                    egui::Color32::from_rgb(240, 240, 246)
                };
                if overlay_layout.book_resume_meter.is_some() {
                    painter.rect_filled(label_rect, 3.0, label_bg);
                }
                let marker = paint_lower_caption(
                    painter,
                    egui::pos2(thumb_rect.min.x + 4.0, thumb_rect.min.y + 4.0),
                    egui::Align2::LEFT_TOP,
                    icon,
                    egui::FontId::proportional(badge_size),
                    label_color,
                    None,
                    None,
                );
                if overlay_layout.book_resume_meter.is_none() {
                    painter.rect_filled(label_rect, 3.0, label_bg);
                }

                record(marker);
                record(label_rect);
                let badge_font = (label_rect.height() * 0.19).clamp(10.0, 14.0);
                let text_rect = egui::Rect::from_min_max(
                    egui::pos2(label_rect.min.x + 4.0, label_rect.min.y + 2.0),
                    egui::pos2(label_rect.max.x - 4.0, label_rect.max.y - badge_font * 1.3),
                );
                let path_str = path.to_string_lossy();
                let components = crate::ui_helpers::split_path_components(&path_str);
                let max_font = (label_rect.height() * 0.24).clamp(10.0, 13.0);
                record(paint_lower_path(
                    painter,
                    text_rect,
                    &components,
                    label_color,
                    max_font,
                    5.0,
                    Some(marker),
                    overlay_layout.book_resume_meter.map(|band| (band, rect)),
                ));
                let badge_text = format!("{} 枚", hit_count);
                let badge_color = if dark {
                    egui::Color32::from_rgb(240, 200, 100)
                } else {
                    egui::Color32::from_rgb(180, 80, 0)
                };
                record(paint_lower_caption(
                    painter,
                    egui::pos2(label_rect.max.x - 6.0, label_rect.max.y - 4.0),
                    egui::Align2::RIGHT_BOTTOM,
                    &badge_text,
                    egui::FontId::proportional(badge_font),
                    badge_color,
                    Some(marker),
                    overlay_layout.book_resume_meter.map(|band| (band, rect)),
                ));
            } else {
                // 代表サムネなし or 未ロード: 従来どおりアイコン + 階層パス + バッジ
                // (日付フォルダ `2025-01-01` 等を単独で識別できるよう階層を多行表示)
                let icon_size = (inner.height() * 0.18).clamp(22.0, 56.0);
                let marker = paint_lower_caption(
                    painter,
                    egui::pos2(inner.center().x, inner.min.y + icon_size * 0.75),
                    egui::Align2::CENTER_CENTER,
                    icon,
                    egui::FontId::proportional(icon_size),
                    label_color,
                    None,
                    None,
                );
                record(marker);
                let badge_font = (inner.height() * 0.07).clamp(10.0, 14.0);
                // Reserve from the bottom; translating the entire hierarchy
                // rectangle would put the terminal name inside the fixed icon.
                let text_rect = egui::Rect::from_min_max(
                    egui::pos2(inner.min.x + 4.0, inner.min.y + icon_size * 1.35),
                    egui::pos2(
                        inner.max.x - 4.0,
                        inner.max.y - badge_font * 2.2 + caption_shift.y,
                    ),
                );
                let path_str = path.to_string_lossy();
                let components = crate::ui_helpers::split_path_components(&path_str);
                let max_font = (inner.height() * 0.075).clamp(11.0, 15.0);
                record(paint_lower_path(
                    painter,
                    text_rect,
                    &components,
                    label_color,
                    max_font,
                    8.0,
                    Some(marker),
                    overlay_layout.book_resume_meter.map(|band| (band, rect)),
                ));
                let badge_text = format!("{} 枚", hit_count);
                let badge_color = if dark {
                    egui::Color32::from_rgb(240, 200, 100)
                } else {
                    egui::Color32::from_rgb(180, 80, 0)
                };
                record(paint_lower_caption(
                    painter,
                    egui::pos2(inner.max.x - 6.0, inner.max.y - 6.0) + caption_shift,
                    egui::Align2::RIGHT_BOTTOM,
                    &badge_text,
                    egui::FontId::proportional(badge_font),
                    badge_color,
                    Some(marker),
                    overlay_layout.book_resume_meter.map(|band| (band, rect)),
                ));
            }
        }
        GridItem::Stack { .. } => {
            // ファイル名スタックの集約セル: 代表画像を通常サムネと同様に描き、
            // 右上に枚数バッジ (= スタックの目印)。単独グループは GridItem::Image で
            // 描かれるのでここには来ない (= count は常に 2 以上)。
            draw_thumb(
                painter,
                inner,
                thumb,
                rotation,
                dark,
                adjusted_tex,
                &mut record,
            );
        }
        GridItem::CollectionPlaceholder { path, reason, .. } => {
            let bg = if dark {
                egui::Color32::from_rgb(52, 45, 45)
            } else {
                egui::Color32::from_rgb(244, 235, 235)
            };
            let fg = if dark {
                egui::Color32::from_rgb(235, 165, 165)
            } else {
                egui::Color32::from_rgb(145, 55, 55)
            };
            painter.rect_filled(inner, 3.0, bg);
            record(inner);
            let marker = paint_lower_caption(
                painter,
                inner.center() - egui::vec2(0.0, 10.0),
                egui::Align2::CENTER_CENTER,
                "?",
                egui::FontId::proportional((inner.height() * 0.24).clamp(22.0, 46.0)),
                fg,
                None,
                None,
            );
            record(marker);
            record(paint_lower_caption(
                painter,
                egui::pos2(inner.center().x, inner.max.y - 18.0) + caption_shift,
                egui::Align2::CENTER_BOTTOM,
                path.file_name()
                    .unwrap_or_else(|| path.as_os_str())
                    .to_string_lossy(),
                egui::FontId::proportional(12.0),
                fg,
                Some(marker),
                overlay_layout.book_resume_meter.map(|band| (band, rect)),
            ));
            record(paint_lower_caption(
                painter,
                egui::pos2(inner.center().x, inner.max.y - 3.0) + caption_shift,
                egui::Align2::CENTER_BOTTOM,
                reason.label(),
                egui::FontId::proportional(11.0),
                fg,
                Some(marker),
                overlay_layout.book_resume_meter.map(|band| (band, rect)),
            ));
        }
    }

    let meter = thumbnail_resume_meter_rect(painter, overlay_layout);
    paint_thumbnail_resume_meter(ui, rect, meter, resume_meter, is_cut);
    if resume_meter.is_some() {
        if let Some(meter) = meter {
            record(meter);
        }
    }
    for badge in overlay_layout.badge_placements() {
        record(badge.rect);
        if collect_hit_areas {
            record(thumbnail_badge_ink_rect(painter, badge));
        }
    }
    if let Some(check) = overlay_layout.check {
        record(check);
    }
    // 固定帯と接する極小セルでも、媒体アイコンと切り取りマークを隠さない。
    // 位置・大きさ・内容のopacityは従来どおり。
    if defer_primary_markers
        && matches!(item, GridItem::Video(_))
        && !is_cut
        && video_thumbnail_indicator_parts(item, video_indicator).play_icon
    {
        let r = (inner.width().min(inner.height()) * 0.18).max(10.0);
        draw_play_icon(painter, inner.center(), r);
        record(egui::Rect::from_center_size(
            inner.center(),
            egui::Vec2::splat(r * 2.0),
        ));
    }
    if defer_primary_markers && matches!(item, GridItem::Audio(_)) {
        let parts = audio_thumbnail_indicator_parts(item, thumb, audio_indicator);
        let mark_rect = if matches!(thumb, ThumbnailState::Loaded { .. }) {
            egui::Rect::from_center_size(inner.center(), egui::vec2(48.0, 48.0).min(inner.size()))
        } else {
            inner
        };
        for area in paint_audio_thumbnail_indicator(
            painter,
            mark_rect,
            dark,
            AudioThumbnailIndicatorParts {
                bottom_left_badge: false,
                ..parts
            },
        )
        .into_iter()
        .flatten()
        {
            record(area);
        }
    }
    let painter = base_painter;
    if is_cut {
        record(draw_cut_badge(painter, inner));
    }
    let painter = &content_painter;

    if let Some(placement) = overlay_layout.stack_count.as_ref() {
        draw_stack_count_badge(painter, placement);
    }

    if let Some(placement) = overlay_layout.top_left.upscaled_video.as_ref() {
        crate::ui_helpers::draw_overlay_upscaled_video_badge(painter, placement);
    }
    for placement in &overlay_layout.top_left.edit_badges {
        match placement.kind {
            crate::thumb_overlay_layout::BadgeKind::Edit(kind) => {
                crate::ui_helpers::draw_overlay_edit_badge(painter, placement, kind);
            }
            crate::thumb_overlay_layout::BadgeKind::EditOverflow => {
                crate::ui_helpers::draw_overlay_edit_overflow_badge(painter, placement);
            }
            _ => {}
        }
    }
    if let Some(placement) = overlay_layout.top_left.tag.as_ref() {
        crate::ui_helpers::draw_overlay_tag_badge(painter, placement);
    }

    if let Some(placement) = overlay_layout.bottom_left.container.as_ref() {
        match placement.kind {
            crate::thumb_overlay_layout::BadgeKind::BottomContainer(
                BottomContainerKind::Folder,
            ) => crate::ui_helpers::draw_overlay_folder_badge(painter, placement),
            crate::thumb_overlay_layout::BadgeKind::BottomContainer(
                BottomContainerKind::Format(kind),
            ) => crate::ui_helpers::draw_overlay_format_badge(painter, placement, kind),
            _ => {}
        }
    }
    if let Some(placement) = overlay_layout.bottom_left.filename.as_ref() {
        crate::ui_helpers::draw_cell_filename(painter, placement, name_text_color, dark);
    }
    if let Some(placement) = overlay_layout.bottom_left.rating.as_ref() {
        crate::ui_helpers::draw_overlay_rating_badge(
            painter,
            placement,
            item.is_container_ratable(),
        );
    }

    if let Some(placement) = overlay_layout.media_duration.as_ref() {
        crate::ui_helpers::draw_overlay_media_duration_badge(painter, placement);
    }
    if let Some(placement) = overlay_layout.filter_match_count.as_ref() {
        draw_filter_match_badge(painter, placement);
    }
    let painter = base_painter;
    let border = if is_selected {
        egui::Stroke::new(2.0, egui::Color32::from_rgb(60, 120, 220))
    } else {
        egui::Stroke::new(
            1.0,
            if dark {
                egui::Color32::from_gray(70)
            } else {
                egui::Color32::from_gray(200)
            },
        )
    };
    painter.rect_stroke(rect, 2.0, border, egui::StrokeKind::Middle);
    if is_spread_pair_cursor && !is_selected {
        draw_spread_pair_cursor(painter, rect, ui.visuals());
    }

    // チェックマークオーバーレイ
    if let Some(check_rect) = overlay_layout.check {
        let check_r = check_rect.width() * 0.5;
        let check_center = check_rect.center();
        painter.circle_filled(check_center, check_r, egui::Color32::from_rgb(40, 140, 40));
        // チェックマーク (✓)
        let s = check_r * 0.55;
        let stroke = egui::Stroke::new(2.5, egui::Color32::WHITE);
        painter.line_segment(
            [
                egui::pos2(check_center.x - s * 0.6, check_center.y),
                egui::pos2(check_center.x - s * 0.1, check_center.y + s * 0.5),
            ],
            stroke,
        );
        painter.line_segment(
            [
                egui::pos2(check_center.x - s * 0.1, check_center.y + s * 0.5),
                egui::pos2(check_center.x + s * 0.7, check_center.y - s * 0.5),
            ],
            stroke,
        );
    }
    hit_areas
}

/// Draw the cut-state marker independently from faded item content and interaction overlays.
///
/// Painter primitives keep the scissors recognizable without relying on an installed glyph.
pub(crate) fn draw_cut_badge(painter: &egui::Painter, rect: egui::Rect) -> egui::Rect {
    let side = rect.width().min(rect.height());
    if !side.is_finite() || side < 8.0 {
        return egui::Rect::NOTHING;
    }
    let radius = (side * 0.18).clamp(7.0, 26.0).min(side * 0.46);
    let center = rect.center();
    painter.circle_filled(
        center,
        radius,
        egui::Color32::from_rgba_unmultiplied(0, 0, 0, 190),
    );

    let stroke_width = (radius * 0.13).clamp(1.2, 2.4);
    let stroke = egui::Stroke::new(stroke_width, egui::Color32::WHITE);
    let upper_handle = center + egui::vec2(-radius * 0.36, -radius * 0.28);
    let lower_handle = center + egui::vec2(-radius * 0.36, radius * 0.28);
    let handle_radius = (radius * 0.19).max(1.5);
    painter.circle_stroke(upper_handle, handle_radius, stroke);
    painter.circle_stroke(lower_handle, handle_radius, stroke);
    let pivot = center + egui::vec2(-radius * 0.06, 0.0);
    painter.line_segment(
        [
            upper_handle + egui::vec2(handle_radius * 0.75, handle_radius * 0.65),
            center + egui::vec2(radius * 0.56, radius * 0.40),
        ],
        stroke,
    );
    painter.line_segment(
        [
            lower_handle + egui::vec2(handle_radius * 0.75, -handle_radius * 0.65),
            center + egui::vec2(radius * 0.56, -radius * 0.40),
        ],
        stroke,
    );
    painter.circle_filled(pivot, (stroke_width * 0.78).max(1.0), egui::Color32::WHITE);
    egui::Rect::from_center_size(center, egui::Vec2::splat(radius * 2.0))
}

pub(crate) fn draw_spread_pair_cursor(
    painter: &egui::Painter,
    rect: egui::Rect,
    visuals: &egui::Visuals,
) {
    let rect = rect.shrink(3.0);
    if rect.width() <= 4.0 || rect.height() <= 4.0 {
        return;
    }
    let color = if visuals.dark_mode {
        egui::Color32::from_rgb(130, 185, 255)
    } else {
        egui::Color32::from_rgb(35, 95, 210)
    };
    let stroke = egui::Stroke::new(2.0, color);
    draw_dashed_segment(painter, rect.left_top(), rect.right_top(), stroke);
    draw_dashed_segment(painter, rect.right_top(), rect.right_bottom(), stroke);
    draw_dashed_segment(painter, rect.right_bottom(), rect.left_bottom(), stroke);
    draw_dashed_segment(painter, rect.left_bottom(), rect.left_top(), stroke);
}

fn draw_dashed_segment(
    painter: &egui::Painter,
    start: egui::Pos2,
    end: egui::Pos2,
    stroke: egui::Stroke,
) {
    let delta = end - start;
    let len = delta.length();
    if len <= 0.1 {
        return;
    }
    let dir = delta / len;
    let dash = 7.0;
    let gap = 5.0;
    let mut pos = 0.0;
    while pos < len {
        let next = (pos + dash).min(len);
        painter.line_segment([start + dir * pos, start + dir * next], stroke);
        pos += dash + gap;
    }
}

fn draw_filter_match_badge(
    painter: &egui::Painter,
    placement: &crate::thumb_overlay_layout::BadgePlacement,
) {
    let bg_rect = placement.rect;
    painter.rect_filled(bg_rect, 3.0, egui::Color32::from_rgb(0xE6, 0x7E, 0x22));
    let galley = painter.layout_no_wrap(
        placement.text.clone(),
        egui::FontId::proportional(placement.style.font_size),
        egui::Color32::WHITE,
    );
    painter.galley(placement.text_pos(), galley, egui::Color32::WHITE);
}

pub(crate) fn primary_grid_tag_for_badge(tags: &[String]) -> Option<&str> {
    tags.iter()
        .find(|tag| tag.starts_with('#'))
        .or_else(|| tags.first())
        .map(String::as_str)
}

pub(crate) fn grid_tag_badge_hit_rect(layout: &ThumbnailOverlayLayout) -> Option<egui::Rect> {
    layout.top_left.tag.as_ref().map(|placement| placement.rect)
}

#[cfg(test)]
mod book_resume_meter_tests {
    use super::*;
    use crate::book_resume_db::ReadingMeterValue;

    fn solid_thumbnail(
        ctx: &egui::Context,
        dims: [usize; 2],
        color: egui::Color32,
    ) -> ThumbnailState {
        ThumbnailState::Loaded {
            tex: ctx.load_texture(
                format!("meter-solid-{dims:?}-{color:?}"),
                egui::ColorImage::new(dims, vec![color; dims[0] * dims[1]]),
                Default::default(),
            ),
            origin: crate::thumb_loader::ThumbLoadOrigin::SourceIntrinsic,
            from_edit_preview: false,
            rendered_at_px: 128,
            source_dims: Some((999, 17)),
            layout_dims: None,
        }
    }

    #[test]
    fn thumbnail_resume_meter_reserved_band_is_fixed_across_kinds_dpi_and_fraction() {
        for dpi in [1.0, 1.25, 1.5, 2.0] {
            let ctx = egui::Context::default();
            ctx.set_pixels_per_point(dpi);
            crate::ui_fonts::configure_fonts(&ctx);
            for width in [32.0, 48.0, 140.0, 240.0] {
                let mut band = None;
                for item in [
                    GridItem::Folder("本gyjpq".into()),
                    GridItem::ZipFile("book.zip".into()),
                    GridItem::PdfFile("book.pdf".into()),
                    GridItem::Video("ACRN0049_HD.wmv".into()),
                    GridItem::Audio("song.flac".into()),
                    GridItem::Image("image.png".into()),
                ] {
                    let mut labels = None;
                    for fraction in [None, Some(0.4), Some(1.0)] {
                        let mut output = None;
                        for _ in 0..2 {
                            output=Some(ctx.run(egui::RawInput::default(),|ctx| {egui::CentralPanel::default().show(ctx,|ui| {
                            let cell=egui::Rect::from_min_size(egui::pos2(20.3,20.7),egui::vec2(width,width));
                            let layout=layout_cell_overlays(ui.painter(),cell,EditBadgeFlags::default(),4,&item,&ThumbnailState::Pending,&[],None,false,
                                VideoThumbnailIndicator::PlayIcon,AudioThumbnailIndicator::default(), false,None,Some("22:35"),true);
                            let raw=layout.book_resume_meter.unwrap(); let meter=thumbnail_resume_meter_rect(ui.painter(),&layout).unwrap();
                            assert_eq!(raw.height(),9.0); assert_eq!(raw.min.x,cell.min.x+4.0);assert_eq!(raw.max.x,cell.max.x-4.0);
                            assert!((meter.height()*dpi-(9.0_f32*dpi).floor()).abs()<0.001);
                            assert!(layout.badge_placements().all(|b| !b.rect.intersects(meter)));
                            for b in layout.badge_placements() { assert!(!thumbnail_badge_ink_rect(ui.painter(),b).intersects(meter)); }
                            if let Some(band)=band {assert_eq!(meter,band);}else{band=Some(meter);}
                            let placements:Vec<_>=layout.badge_placements().cloned().collect();
                            if let Some(labels)=&labels{assert_eq!(&placements,labels);}else{labels=Some(placements);}
                            draw_cell(ui,cell,false,false,false,&layout,&item,&ThumbnailState::Pending,crate::rotation_db::Rotation::None,None,false,
                                VideoThumbnailIndicator::PlayIcon,AudioThumbnailIndicator::default(), false,fraction);
                            if width==140.0 && fraction==Some(0.4) && matches!(item,GridItem::Video(_)) {eprintln!("reserved-meter-measure dpi={dpi} cell={cell:?} meter={meter:?} pixels={}",meter.height()*dpi);}
                        });}));
                        }
                        let palette = crate::os_theme::book_resume_meter_palette(
                            ctx.style().visuals.dark_mode,
                        );
                        let tracks=output.unwrap().shapes.iter().filter(|s|matches!(&s.shape,egui::Shape::Rect(r) if r.fill==palette.track)).count();
                        assert_eq!(tracks, usize::from(fraction.is_some()));
                    }
                }
            }
        }
    }

    #[test]
    fn thumbnail_resume_meter_custom_lower_captions_follow_same_reservation() {
        use egui::emath::GuiRounding;
        let ctx = egui::Context::default();
        crate::ui_fonts::configure_fonts(&ctx);
        for representative in [
            None,
            Some(crate::grid_item::ContainerRepresentative {
                path: "book.png".into(),
                zip_entry: None,
                pdf_page: None,
            }),
        ] {
            for item in [
                GridItem::SearchContainer {
                    path: "book".into(),
                    kind: crate::grid_item::SearchContainerKind::Folder,
                    hit_count: 3,
                    representative,
                },
                GridItem::CollectionPlaceholder {
                    path: "missing.png".into(),
                    last_known_kind: crate::collection_store::CollectionResolvedKind::Image,
                    reason: crate::grid_item::CollectionPlaceholderReason::Missing,
                },
            ] {
                let mut baseline: Option<Vec<(String, egui::Pos2)>> = None;
                for enabled in [false, true] {
                    let mut output = None;
                    for _ in 0..2 {
                        output = Some(ctx.run(egui::RawInput::default(), |ctx| {
                            egui::CentralPanel::default().show(ctx, |ui| {
                                let cell = egui::Rect::from_min_size(
                                    egui::pos2(20.0, 20.0),
                                    egui::vec2(240.0, 190.0),
                                );
                                let thumb = solid_thumbnail(ctx, [3, 5], egui::Color32::WHITE);
                                let layout = layout_cell_overlays(
                                    ui.painter(),
                                    cell,
                                    EditBadgeFlags::default(),
                                    0,
                                    &item,
                                    &thumb,
                                    &[],
                                    None,
                                    false,
                                    VideoThumbnailIndicator::Hidden,
                                    AudioThumbnailIndicator::default(),
                                    false,
                                    None,
                                    None,
                                    enabled,
                                );
                                draw_cell(
                                    ui,
                                    cell,
                                    false,
                                    false,
                                    false,
                                    &layout,
                                    &item,
                                    &thumb,
                                    crate::rotation_db::Rotation::None,
                                    None,
                                    false,
                                    VideoThumbnailIndicator::Hidden,
                                    AudioThumbnailIndicator::default(),
                                    false,
                                    None,
                                );
                            });
                        }));
                    }
                    let texts: Vec<_> = output
                        .unwrap()
                        .shapes
                        .iter()
                        .filter_map(|s| match &s.shape {
                            egui::Shape::Text(t) => {
                                Some((t.galley.text().to_owned(), t.pos.round_to_pixels(1.0)))
                            }
                            _ => None,
                        })
                        .collect();
                    if let Some(baseline) = &baseline {
                        assert_eq!(texts.len(), baseline.len());
                        for ((text, pos), (previous, old)) in texts.iter().zip(baseline) {
                            assert_eq!(text, previous);
                            if matches!(
                                &item,
                                GridItem::SearchContainer {
                                    representative: None,
                                    ..
                                }
                            ) && text == "book"
                            {
                                // The hierarchy keeps its top and re-centers in a shorter region.
                                assert_eq!(pos.x, old.x);
                                assert!(pos.y <= old.y);
                                assert!(pos.y >= old.y - crate::thumb_overlay_layout::BOOK_RESUME_METER_RESERVE);
                                continue;
                            }
                            let shift = if text == "?" || text == "📁" {
                                0.0
                            } else {
                                -crate::thumb_overlay_layout::BOOK_RESUME_METER_RESERVE
                            };
                            assert_eq!(*pos, *old + egui::vec2(0.0, shift));
                        }
                    } else {
                        baseline = Some(texts);
                    }
                }
            }
        }
    }

    #[test]
    fn thumbnail_resume_meter_caption_fit_does_not_depend_on_scroll_clip() {
        let ctx = egui::Context::default();
        crate::ui_fonts::configure_fonts(&ctx);
        for clipped in [false, true] {
            let mut output = None;
            for _ in 0..2 {
                output = Some(ctx.run(egui::RawInput::default(), |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        if clipped {
                            ui.set_clip_rect(egui::Rect::from_min_max(
                                egui::pos2(0.0, 100.0),
                                egui::pos2(400.0, 200.0),
                            ));
                        }
                        let cell = egui::Rect::from_min_size(
                            egui::pos2(20.0, 20.0),
                            egui::vec2(180.0, 94.0),
                        );
                        let item = GridItem::CollectionPlaceholder {
                            path: "missing.png".into(),
                            last_known_kind: crate::collection_store::CollectionResolvedKind::Image,
                            reason: crate::grid_item::CollectionPlaceholderReason::Missing,
                        };
                        let layout = layout_cell_overlays(
                            ui.painter(),
                            cell,
                            EditBadgeFlags::default(),
                            0,
                            &item,
                            &ThumbnailState::Pending,
                            &[],
                            None,
                            false,
                            VideoThumbnailIndicator::Hidden,
                            AudioThumbnailIndicator::default(),
                            false,
                            None,
                            None,
                            true,
                        );
                        draw_cell(
                            ui,
                            cell,
                            false,
                            false,
                            false,
                            &layout,
                            &item,
                            &ThumbnailState::Pending,
                            crate::rotation_db::Rotation::None,
                            None,
                            false,
                            VideoThumbnailIndicator::Hidden,
                            AudioThumbnailIndicator::default(),
                            false,
                            None,
                        );
                    });
                }));
            }
            assert!(
                output
                    .unwrap()
                    .shapes
                    .iter()
                    .any(|shape| matches!(&shape.shape,
                egui::Shape::Text(text) if text.galley.text() == "見つかりません"))
            );
        }
    }

    #[test]
    fn thumbnail_resume_meter_custom_small_caption_measurements() {
        let ctx = egui::Context::default();
        crate::ui_fonts::configure_fonts(&ctx);
        for height in [94.0, 48.0, 32.0] {
            for item in [
                GridItem::SearchContainer {
                    path: "root/first/second/third/book".into(),
                    kind: crate::grid_item::SearchContainerKind::Folder,
                    hit_count: 3,
                    representative: None,
                },
                GridItem::SearchContainer {
                    path: "root/first/second/third/book".into(),
                    kind: crate::grid_item::SearchContainerKind::Folder,
                    hit_count: 3,
                    representative: Some(crate::grid_item::ContainerRepresentative {
                        path: "book.png".into(),
                        zip_entry: None,
                        pdf_page: None,
                    }),
                },
                GridItem::CollectionPlaceholder {
                    path: "missing.png".into(),
                    last_known_kind: crate::collection_store::CollectionResolvedKind::Image,
                    reason: crate::grid_item::CollectionPlaceholderReason::Missing,
                },
            ] {
                let mut output = None;
                for _ in 0..2 {
                    output = Some(ctx.run(egui::RawInput::default(), |ctx| {
                        egui::CentralPanel::default().show(ctx, |ui| {
                            let cell = egui::Rect::from_min_size(
                                egui::pos2(20.0, 20.0),
                                egui::vec2(180.0, height),
                            );
                            let thumb = solid_thumbnail(ctx, [3, 5], egui::Color32::WHITE);
                            let layout = layout_cell_overlays(
                                ui.painter(),
                                cell,
                                EditBadgeFlags::default(),
                                0,
                                &item,
                                &thumb,
                                &[],
                                None,
                                false,
                                VideoThumbnailIndicator::Hidden,
                                AudioThumbnailIndicator::default(),
                                false,
                                None,
                                None,
                                true,
                            );
                            draw_cell(
                                ui,
                                cell,
                                false,
                                false,
                                false,
                                &layout,
                                &item,
                                &thumb,
                                crate::rotation_db::Rotation::None,
                                None,
                                false,
                                VideoThumbnailIndicator::Hidden,
                                AudioThumbnailIndicator::default(),
                                false,
                                None,
                            );
                        });
                    }));
                }
                let mut marker = None;
                let mut captions = Vec::new();
                let mut terminal_name_present = false;
                for shape in output.unwrap().shapes {
                    if let egui::Shape::Text(text) = shape.shape {
                        terminal_name_present |= text.galley.text().lines().last() == Some("book");
                        let ink = text.galley.mesh_bounds.translate(text.pos.to_vec2());
                        println!(
                            "caption height={height} text={:?} ink={:?}",
                            text.galley.text(),
                            ink
                        );
                        if text.galley.text() == "?" || text.galley.text() == "📁" {
                            marker = Some(ink);
                        } else {
                            captions.push(ink);
                        }
                    }
                }
                if height == 94.0 && matches!(&item, GridItem::SearchContainer { .. }) {
                    assert!(
                        terminal_name_present,
                        "normal SearchContainer must retain book"
                    );
                }
                let marker = marker.unwrap();
                let cell =
                    egui::Rect::from_min_size(egui::pos2(20.0, 20.0), egui::vec2(180.0, height));
                let band = egui::Rect::from_min_max(
                    egui::pos2(24.0, cell.max.y - 13.0),
                    cell.max - egui::vec2(4.0, 4.0),
                );
                for ink in captions {
                    assert!(cell.contains_rect(ink));
                    assert!(!ink.intersects(marker));
                    assert!(!ink.intersects(band));
                }
            }
        }
    }

    #[test]
    fn thumbnail_resume_meter_normal_cells_retain_all_kind_labels() {
        use crate::grid_item::{ContainerRepresentative, SearchContainerKind};
        let cases = [
            (GridItem::Folder("book".into()), "book"),
            (GridItem::ZipFile("book.zip".into()), "book.zip"),
            (GridItem::PdfFile("book.pdf".into()), "book.pdf"),
            (GridItem::Video("movie.mp4".into()), "movie.mp4"),
            (GridItem::Audio("song.flac".into()), "song.flac"),
            (GridItem::Image("page.png".into()), "★"),
            (
                GridItem::ConvertibleArchive {
                    path: "book.rar".into(),
                    format: crate::archive_converter::ArchiveFormat::Rar,
                },
                "book.rar",
            ),
            (
                GridItem::ZipImage {
                    zip_path: "book.zip".into(),
                    entry_name: "page.png".into(),
                },
                "★",
            ),
            (
                GridItem::PdfPage {
                    pdf_path: "book.pdf".into(),
                    page_num: 0,
                    content_type: None,
                },
                "★",
            ),
            (
                GridItem::ZipDir {
                    zip_path: "book.zip".into(),
                    dir_prefix: "chapter/".into(),
                    is_archive: false,
                    representative: None,
                },
                "chapter",
            ),
            (
                GridItem::ZipDir {
                    zip_path: "book.zip".into(),
                    dir_prefix: "inner.zip/".into(),
                    is_archive: true,
                    representative: None,
                },
                "inner.zip",
            ),
            (
                GridItem::Stack {
                    key: "pages".into(),
                    representative: "page.png".into(),
                    count: 3,
                },
                "3",
            ),
            (
                GridItem::SearchContainer {
                    path: "root/first/second/third/book".into(),
                    kind: SearchContainerKind::Folder,
                    hit_count: 3,
                    representative: None,
                },
                "book",
            ),
            (
                GridItem::SearchContainer {
                    path: "root/first/second/third/book".into(),
                    kind: SearchContainerKind::Zip,
                    hit_count: 3,
                    representative: Some(ContainerRepresentative {
                        path: "page.png".into(),
                        zip_entry: None,
                        pdf_page: None,
                    }),
                },
                "book",
            ),
            (
                GridItem::CollectionPlaceholder {
                    path: "missing.png".into(),
                    last_known_kind: crate::collection_store::CollectionResolvedKind::Image,
                    reason: crate::grid_item::CollectionPlaceholderReason::Missing,
                },
                "missing.png",
            ),
            (
                GridItem::SearchContainer {
                    path: "root/first/second/third/long_terminal_book_with_a_name".into(),
                    kind: SearchContainerKind::Folder,
                    hit_count: 3,
                    representative: Some(ContainerRepresentative {
                        path: "page.png".into(),
                        zip_entry: None,
                        pdf_page: None,
                    }),
                },
                "long_terminal_book_with_a_name",
            ),
        ];
        for dark in [false, true] {
            for dpi in [1.0, 1.25, 1.5, 2.0] {
                let ctx = egui::Context::default();
                crate::ui_fonts::configure_fonts(&ctx);
                ctx.set_pixels_per_point(dpi);
                ctx.set_visuals(if dark {
                    egui::Visuals::dark()
                } else {
                    egui::Visuals::light()
                });
                for (item, required) in &cases {
                    for loaded in [false, true] {
                        let mut on_texts = None;
                        for (enabled, fraction) in [(false, None), (true, None), (true, Some(0.4))]
                        {
                            let cell = egui::Rect::from_min_size(
                                egui::pos2(20.0, 20.0),
                                egui::vec2(180.0, 94.0),
                            );
                            let mut band = None;
                            let mut output = None;
                            for _ in 0..2 {
                                output = Some(ctx.run(egui::RawInput::default(), |ctx| {
                                    egui::CentralPanel::default().show(ctx, |ui| {
                                        let thumb = if loaded {
                                            solid_thumbnail(ctx, [3, 5], egui::Color32::WHITE)
                                        } else {
                                            ThumbnailState::Pending
                                        };
                                        let duration =
                                            matches!(item, GridItem::Video(_) | GridItem::Audio(_))
                                                .then_some("22:35");
                                        let layout = layout_cell_overlays(
                                            ui.painter(),
                                            cell,
                                            EditBadgeFlags::default(),
                                            4,
                                            item,
                                            &thumb,
                                            &[],
                                            None,
                                            false,
                                            VideoThumbnailIndicator::PlayIcon,
                                            AudioThumbnailIndicator::default(),
                                            false,
                                            None,
                                            duration,
                                            enabled,
                                        );
                                        band = layout.book_resume_meter;
                                        draw_cell(
                                            ui,
                                            cell,
                                            false,
                                            false,
                                            false,
                                            &layout,
                                            item,
                                            &thumb,
                                            crate::rotation_db::Rotation::None,
                                            None,
                                            false,
                                            VideoThumbnailIndicator::PlayIcon,
                                            AudioThumbnailIndicator::default(),
                                            false,
                                            fraction,
                                        );
                                    });
                                }));
                            }
                            let texts: Vec<_> = output
                                .unwrap()
                                .shapes
                                .into_iter()
                                .filter_map(|shape| match shape.shape {
                                    egui::Shape::Text(t) => Some((
                                        t.galley.text().to_owned(),
                                        t.galley.mesh_bounds.translate(t.pos.to_vec2()),
                                    )),
                                    _ => None,
                                })
                                .collect();
                            assert!(
                                texts.iter().any(|(text, _)| text.contains(required)),
                                "missing {required:?}: {item:?}, loaded={loaded}, enabled={enabled}, dark={dark}, dpi={dpi}: {texts:?}"
                            );
                            if matches!(item, GridItem::Video(_) | GridItem::Audio(_)) {
                                assert!(texts.iter().any(|(text, _)| text == "22:35"));
                            }
                            if let Some(band) = band {
                                let marker = texts
                                    .iter()
                                    .find(|(text, _)| matches!(text.as_str(), "📁" | "📦" | "?"))
                                    .map(|(_, ink)| *ink);
                                for (text, ink) in &texts {
                                    assert!(
                                        !ink.intersects(band),
                                        "{item:?} {text:?} intersects band: {ink:?}"
                                    );
                                    if text.contains(required)
                                        && matches!(
                                            item,
                                            GridItem::SearchContainer { .. }
                                                | GridItem::CollectionPlaceholder { .. }
                                        )
                                    {
                                        assert!(cell.contains_rect(*ink));
                                        assert!(!ink.intersects(marker.expect("fixed marker")));
                                    }
                                }
                                if let Some(previous) = &on_texts {
                                    assert_eq!(
                                        &texts, previous,
                                        "record must not affect captions: {item:?}"
                                    );
                                } else {
                                    on_texts = Some(texts);
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn thumbnail_resume_meter_small_primary_markers_stay_above_strip() {
        let ctx = egui::Context::default();
        crate::ui_fonts::configure_fonts(&ctx);
        for (item, cut) in [
            (GridItem::Video("v.mp4".into()), false),
            (GridItem::Audio("song.flac".into()), false),
            (GridItem::PdfFile("book.pdf".into()), true),
        ] {
            let mut output = None;
            for _ in 0..2 {
                output = Some(ctx.run(egui::RawInput::default(), |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        let cell = egui::Rect::from_min_size(
                            egui::pos2(20.0, 20.0),
                            egui::vec2(32.0, 32.0),
                        );
                        let thumb = solid_thumbnail(ctx, [1, 1], egui::Color32::BLACK);
                        let layout = layout_cell_overlays(
                            ui.painter(),
                            cell,
                            EditBadgeFlags::default(),
                            0,
                            &item,
                            &thumb,
                            &[],
                            None,
                            false,
                            VideoThumbnailIndicator::PlayIcon,
                            AudioThumbnailIndicator::default(),
                            false,
                            None,
                            None,
                            true,
                        );
                        draw_cell(
                            ui,
                            cell,
                            false,
                            false,
                            false,
                            &layout,
                            &item,
                            &thumb,
                            crate::rotation_db::Rotation::None,
                            None,
                            false,
                            VideoThumbnailIndicator::PlayIcon,
                            AudioThumbnailIndicator::default(),
                            cut,
                            Some(0.5),
                        );
                    });
                }));
            }
            let shapes = output.unwrap().shapes;
            let track = crate::os_theme::book_resume_meter_palette(ctx.style().visuals.dark_mode)
                .track
                .gamma_multiply(if cut {
                    crate::cut_clipboard::CUT_CONTENT_OPACITY
                } else {
                    1.0
                });
            let meter = shapes
                .iter()
                .position(|s| matches!(&s.shape, egui::Shape::Rect(r) if r.fill == track))
                .unwrap();
            let marker = shapes
                .iter()
                .rposition(|s| {
                    matches!(
                        &s.shape,
                        egui::Shape::Path(_)
                            | egui::Shape::LineSegment { .. }
                            | egui::Shape::Circle(_)
                    )
                })
                .unwrap();
            assert!(meter < marker);
        }
    }

    #[test]
    fn thumbnail_resume_meter_is_painted_before_labels_and_primary_markers() {
        let ctx = egui::Context::default();
        crate::ui_fonts::configure_fonts(&ctx);
        let cell = egui::Rect::from_min_size(egui::pos2(20.0, 20.0), egui::vec2(140.0, 94.0));
        let mut output = None;
        for _ in 0..2 {
            output = Some(ctx.run(egui::RawInput::default(), |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    let item = GridItem::Video(std::path::PathBuf::from("v.mp4"));
                    let layout = layout_cell_overlays(
                        ui.painter(),
                        cell,
                        EditBadgeFlags::default(),
                        0,
                        &item,
                        &ThumbnailState::Pending,
                        &[],
                        None,
                        false,
                        VideoThumbnailIndicator::default(),
                        AudioThumbnailIndicator::default(),
                        true,
                        None,
                        Some("1:02:03"),
                        true,
                    );
                    assert!(layout.book_resume_meter.is_some());
                    draw_cell(
                        ui,
                        cell,
                        false,
                        true,
                        true,
                        &layout,
                        &item,
                        &ThumbnailState::Pending,
                        crate::rotation_db::Rotation::None,
                        None,
                        false,
                        VideoThumbnailIndicator::default(),
                        AudioThumbnailIndicator::default(),
                        false,
                        Some(0.5),
                    );
                });
            }));
        }
        let shapes = output.unwrap().shapes;
        let fill_color =
            crate::os_theme::book_resume_meter_palette(ctx.style().visuals.dark_mode).fill;
        let meter = shapes
            .iter()
            .position(|s| {
                matches!(&s.shape,
            egui::Shape::Rect(r) if r.fill == fill_color)
            })
            .unwrap();
        let label = shapes
            .iter()
            .rposition(|s| matches!(&s.shape, egui::Shape::Text(_)))
            .unwrap();
        let marker = shapes
            .iter()
            .position(|s| matches!(&s.shape, egui::Shape::Path(_)))
            .unwrap();
        let frame = shapes
            .iter()
            .position(|s| {
                matches!(&s.shape,
            egui::Shape::Rect(r) if r.rect == cell && r.stroke.width > 0.0)
            })
            .unwrap();
        let dashed = shapes
            .iter()
            .position(|s| {
                matches!(&s.shape,
            egui::Shape::LineSegment { stroke, .. } if stroke.width == 2.0)
            })
            .unwrap();
        let check = shapes
            .iter()
            .position(|s| {
                matches!(&s.shape,
            egui::Shape::Circle(c) if c.fill == egui::Color32::from_rgb(40, 140, 40))
            })
            .unwrap();
        assert!(
            meter < marker && marker < label && label < frame && frame < dashed && dashed < check
        );
    }

    #[test]
    fn thumbnail_resume_meter_thin_strip_retains_fill_without_outline() {
        for dpi in [1.0, 1.5, 2.0] {
            let ctx = egui::Context::default();
            ctx.set_pixels_per_point(dpi);
            for pixels in [1.0, 2.0] {
                let meter = egui::Rect::from_min_size(
                    egui::pos2(24.0, 80.0),
                    egui::vec2(92.0, pixels / dpi),
                );
                let output = ctx.run(egui::RawInput::default(), |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        paint_thumbnail_resume_meter(
                            ui,
                            meter.expand(4.0),
                            Some(meter),
                            Some(1.0),
                            false,
                        );
                    });
                });
                let palette =
                    crate::os_theme::book_resume_meter_palette(ctx.style().visuals.dark_mode);
                assert!(output.shapes.iter().any(|s| matches!(&s.shape,
                    egui::Shape::Rect(r) if r.fill == palette.fill && r.rect == meter)));
                assert!(!output.shapes.iter().any(|s| matches!(&s.shape,
                    egui::Shape::Rect(r) if r.rect == meter && r.stroke.width > 0.0)));
            }
        }
    }

    #[test]
    fn thumbnail_resume_meter_paint_always_fills_left_to_right_with_saved_fraction_and_cut_opacity()
    {
        for fraction in [0.1, 0.4, 1.0] {
            for cut in [false, true] {
                let ctx = egui::Context::default();
                let cell =
                    egui::Rect::from_min_size(egui::pos2(20.0, 20.0), egui::vec2(100.0, 80.0));
                let meter =
                    egui::Rect::from_min_max(egui::pos2(24.0, 93.0), egui::pos2(116.0, 96.0));
                let output = ctx.run(
                    egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(
                            egui::Pos2::ZERO,
                            egui::vec2(160.0, 140.0),
                        )),
                        ..Default::default()
                    },
                    |ctx| {
                        crate::os_theme::apply_resolved(ctx, crate::os_theme::ResolvedTheme::Dark);
                        egui::CentralPanel::default().show(ctx, |ui| {
                            paint_thumbnail_resume_meter(
                                ui,
                                cell,
                                Some(meter),
                                Some(fraction),
                                cut,
                            );
                        });
                    },
                );
                let color = crate::os_theme::book_resume_meter_palette(true).fill;
                let expected = if cut {
                    color.gamma_multiply(crate::cut_clipboard::CUT_CONTENT_OPACITY)
                } else {
                    color
                };
                let fill = output
                    .shapes
                    .iter()
                    .find_map(|shape| match &shape.shape {
                        egui::epaint::Shape::Rect(rect) if rect.fill == expected => Some(rect),
                        _ => None,
                    })
                    .expect("meter fill rectangle");
                assert!((fill.rect.width() - meter.width() * fraction).abs() < 0.001);
                assert_eq!(fill.rect.min.y, meter.min.y);
                assert_eq!(fill.rect.max.y, meter.max.y);
                assert_eq!(fill.rect.min.x, meter.min.x);
            }
        }
    }

    #[test]
    fn thumbnail_resume_meter_paint_has_no_track_without_value_or_layout() {
        for has_value in [false, true] {
            let ctx = egui::Context::default();
            let output = ctx.run(egui::RawInput::default(), |ctx| {
                crate::os_theme::apply_resolved(ctx, crate::os_theme::ResolvedTheme::Dark);
                egui::CentralPanel::default().show(ctx, |ui| {
                    let rect =
                        egui::Rect::from_min_size(egui::pos2(10.0, 10.0), egui::vec2(100.0, 80.0));
                    paint_thumbnail_resume_meter(
                        ui,
                        rect,
                        (!has_value).then_some(rect),
                        has_value.then_some(1.0),
                        false,
                    );
                });
            });
            let track = crate::os_theme::book_resume_meter_palette(true).track;
            assert!(!output.shapes.iter().any(|shape| matches!(&shape.shape,
                egui::epaint::Shape::Rect(rect) if rect.fill == track)));
        }
    }

    fn fixture_cell(
        ui: &mut egui::Ui,
        size: egui::Vec2,
        item: &GridItem,
        color: egui::Color32,
        value: Option<ReadingMeterValue>,
        dense: bool,
        cut: bool,
    ) {
        paint_fixture_cell(
            ui,
            size,
            item,
            color,
            value.map(ReadingMeterValue::fraction),
            dense,
            cut,
            None,
        );
    }

    #[allow(clippy::too_many_arguments)]
    fn paint_fixture_cell(
        ui: &mut egui::Ui,
        size: egui::Vec2,
        item: &GridItem,
        color: egui::Color32,
        fraction: Option<f32>,
        dense: bool,
        cut: bool,
        duration: Option<&str>,
    ) {
        let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
        let thumb = ThumbnailState::Loaded {
            tex: ui.ctx().load_texture(
                format!("resume-fixture-{}-{}", rect.min.x, rect.min.y),
                egui::ColorImage::new([1, 1], vec![color]),
                Default::default(),
            ),
            origin: crate::thumb_loader::ThumbLoadOrigin::SourceIntrinsic,
            from_edit_preview: false,
            rendered_at_px: 128,
            source_dims: Some((1, 1)),
            layout_dims: None,
        };
        let tags = if dense {
            vec!["#本".to_owned()]
        } else {
            Vec::new()
        };
        let layout = layout_cell_overlays(
            ui.painter(),
            rect,
            EditBadgeFlags {
                crop: dense,
                pin: dense,
                ..Default::default()
            },
            if dense { 4 } else { 0 },
            item,
            &thumb,
            &tags,
            None,
            false,
            VideoThumbnailIndicator::default(),
            AudioThumbnailIndicator::default(),
            dense,
            dense.then_some(42),
            duration,
            true,
        );
        draw_cell(
            ui,
            rect,
            dense,
            dense,
            false,
            &layout,
            item,
            &thumb,
            crate::rotation_db::Rotation::None,
            None,
            false,
            VideoThumbnailIndicator::default(),
            AudioThumbnailIndicator::default(),
            cut,
            fraction,
        );
    }

    fn fixture(ui: &mut egui::Ui) {
        use std::path::PathBuf;
        let folder = GridItem::Folder(PathBuf::from("長い本のフォルダ"));
        let zip = GridItem::ZipFile(PathBuf::from("book.zip"));
        let pdf = GridItem::PdfFile(PathBuf::from("book.pdf"));
        ui.spacing_mut().item_spacing = egui::vec2(8.0, 5.0);
        ui.label("保存された位置: 1/10・途中・最後 (常に左→右)");
        ui.horizontal(|ui| {
            for (item, color, ordinal) in [
                (&folder, egui::Color32::WHITE, 1),
                (&zip, egui::Color32::BLACK, 4),
                (&pdf, egui::Color32::from_rgb(245, 30, 110), 10),
            ] {
                fixture_cell(
                    ui,
                    egui::vec2(140.0, 94.0),
                    item,
                    color,
                    Some(ReadingMeterValue { ordinal, total: 10 }),
                    false,
                    false,
                );
            }
        });
        ui.label("選択・チェック・編集・タグ・評価・件数 / 切り取り / 未記録");
        ui.horizontal(|ui| {
            for (item, value, dense, cut) in [
                (
                    &folder,
                    Some(ReadingMeterValue {
                        ordinal: 6,
                        total: 10,
                    }),
                    true,
                    false,
                ),
                (
                    &zip,
                    Some(ReadingMeterValue {
                        ordinal: 6,
                        total: 10,
                    }),
                    false,
                    true,
                ),
                (&pdf, None, false, false),
            ] {
                fixture_cell(
                    ui,
                    egui::vec2(140.0, 108.0),
                    item,
                    egui::Color32::from_rgb(38, 65, 112),
                    value,
                    dense,
                    cut,
                );
            }
        });
        ui.label("狭いセル / 極小セルは既存バッジ優先");
        ui.horizontal(|ui| {
            for width in [100.0, 48.0, 32.0] {
                fixture_cell(
                    ui,
                    egui::vec2(width, width),
                    &pdf,
                    egui::Color32::WHITE,
                    Some(ReadingMeterValue {
                        ordinal: 3,
                        total: 10,
                    }),
                    true,
                    false,
                );
            }
        });
    }

    fn media_fixture(ui: &mut egui::Ui, duration_badge: bool) {
        use std::path::PathBuf;
        let video = GridItem::Video(PathBuf::from("scene.mp4"));
        let audio = GridItem::Audio(PathBuf::from("song.flac"));
        let duration = duration_badge.then_some("1:02:03");
        ui.spacing_mut().item_spacing = egui::vec2(8.0, 5.0);
        ui.label(if duration_badge {
            "再生位置メーター / 長さバッジ ON"
        } else {
            "再生位置メーター / 長さバッジ OFF"
        });
        ui.horizontal(|ui| {
            for (item, fraction, color) in [
                (&video, Some(0.35), egui::Color32::WHITE),
                (&audio, Some(0.75), egui::Color32::from_rgb(245, 30, 110)),
                (&video, None, egui::Color32::BLACK),
            ] {
                paint_fixture_cell(
                    ui,
                    egui::vec2(140.0, 94.0),
                    item,
                    color,
                    fraction,
                    false,
                    false,
                    duration,
                );
            }
        });
        ui.label("選択・チェック・編集・タグ・評価 / 切り取り / 最後");
        ui.horizontal(|ui| {
            for (item, fraction, dense, cut) in [
                (&video, 0.4, true, false),
                (&audio, 0.6, false, true),
                (&audio, 1.0, false, false),
            ] {
                paint_fixture_cell(
                    ui,
                    egui::vec2(140.0, 108.0),
                    item,
                    egui::Color32::from_rgb(38, 65, 112),
                    Some(fraction),
                    dense,
                    cut,
                    duration,
                );
            }
        });
        ui.label("狭いセル / 極小セルは既存バッジ優先");
        ui.horizontal(|ui| {
            for width in [100.0, 48.0, 32.0] {
                paint_fixture_cell(
                    ui,
                    egui::vec2(width, width),
                    &video,
                    egui::Color32::WHITE,
                    Some(0.3),
                    true,
                    false,
                    duration,
                );
            }
        });
    }

    fn reserved_meter_fixture(ui: &mut egui::Ui) {
        ui.spacing_mut().item_spacing = egui::vec2(6.0, 6.0);
        ui.label("左: ON・記録あり / 中: ON・記録なし / 右: OFF");
        let cases = [
            (
                GridItem::Folder("縦長の本gyjpq".into()),
                [3, 5],
                egui::Color32::WHITE,
                Some(0.4),
                None,
            ),
            (
                GridItem::Video("ACRN0049_HD.wmv".into()),
                [5, 3],
                egui::Color32::WHITE,
                Some(0.4),
                Some("22:35"),
            ),
            (
                GridItem::Audio("song.flac".into()),
                [3, 5],
                egui::Color32::BLACK,
                Some(0.7),
                Some("1:02:03"),
            ),
            (
                GridItem::ZipFile("black_book.zip".into()),
                [3, 5],
                egui::Color32::BLACK,
                Some(1.0),
                None,
            ),
            (
                GridItem::PdfFile("wide_book.pdf".into()),
                [5, 3],
                egui::Color32::WHITE,
                None,
                None,
            ),
            (
                GridItem::Image("image.png".into()),
                [3, 5],
                egui::Color32::WHITE,
                None,
                None,
            ),
            (
                GridItem::ConvertibleArchive {
                    path: "book.rar".into(),
                    format: crate::archive_converter::ArchiveFormat::Rar,
                },
                [5, 3],
                egui::Color32::BLACK,
                None,
                None,
            ),
            (
                GridItem::Stack {
                    key: "pages".into(),
                    representative: "page.png".into(),
                    count: 3,
                },
                [3, 5],
                egui::Color32::WHITE,
                None,
                None,
            ),
            (
                GridItem::SearchContainer {
                    path: "root/first/second/third/book".into(),
                    kind: crate::grid_item::SearchContainerKind::Folder,
                    hit_count: 3,
                    representative: None,
                },
                [3, 5],
                egui::Color32::WHITE,
                None,
                None,
            ),
            (
                GridItem::CollectionPlaceholder {
                    path: "missing.png".into(),
                    last_known_kind: crate::collection_store::CollectionResolvedKind::Image,
                    reason: crate::grid_item::CollectionPlaceholderReason::Missing,
                },
                [3, 5],
                egui::Color32::WHITE,
                None,
                None,
            ),
        ];
        for (item, dims, color, fraction, duration) in cases {
            ui.horizontal(|ui| {
                for (enabled, value) in [(true, fraction), (true, None), (false, None)] {
                    let (cell, _) =
                        ui.allocate_exact_size(egui::vec2(180.0, 94.0), egui::Sense::hover());
                    let thumb = solid_thumbnail(ui.ctx(), dims, color);
                    let layout = layout_cell_overlays(
                        ui.painter(),
                        cell,
                        EditBadgeFlags::default(),
                        4,
                        &item,
                        &thumb,
                        &[],
                        None,
                        false,
                        VideoThumbnailIndicator::PlayIcon,
                        AudioThumbnailIndicator::default(),
                        false,
                        None,
                        duration,
                        enabled,
                    );
                    draw_cell(
                        ui,
                        cell,
                        false,
                        false,
                        false,
                        &layout,
                        &item,
                        &thumb,
                        crate::rotation_db::Rotation::None,
                        None,
                        false,
                        VideoThumbnailIndicator::PlayIcon,
                        AudioThumbnailIndicator::default(),
                        false,
                        value,
                    );
                }
            });
        }
        ui.label("小セル: 全セルで同じ帯、入らない下端ラベルは省略");
        ui.horizontal(|ui| {
            for width in [100.0, 48.0, 32.0] {
                paint_fixture_cell(
                    ui,
                    egui::vec2(width, width),
                    &GridItem::Video("v.mp4".into()),
                    egui::Color32::BLACK,
                    Some(0.5),
                    true,
                    false,
                    Some("22:35"),
                );
            }
        });
    }
    fn reserved_meter_snapshot(name: &str, theme: crate::os_theme::ResolvedTheme, dpi: f32) {
        let mut ready = false;
        let mut harness = egui_kittest::Harness::builder()
            .with_size(egui::vec2(580.0, 1190.0))
            .with_pixels_per_point(dpi)
            .build(move |ctx| {
                crate::os_theme::apply_resolved(ctx, theme);
                if !ready {
                    crate::ui_fonts::configure_fonts(ctx);
                    ready = true;
                    ctx.request_repaint();
                    return;
                }
                egui::CentralPanel::default().show(ctx, reserved_meter_fixture);
            });
        harness.run();
        harness.snapshot(name);
    }
    #[test]
    fn reserved_resume_meter_snapshot_light() {
        reserved_meter_snapshot(
            "reserved_resume_meter_light",
            crate::os_theme::ResolvedTheme::Light,
            1.0,
        );
    }
    #[test]
    fn reserved_resume_meter_snapshot_dark() {
        reserved_meter_snapshot(
            "reserved_resume_meter_dark",
            crate::os_theme::ResolvedTheme::Dark,
            1.0,
        );
    }
    #[test]
    fn reserved_resume_meter_snapshot_light_high_dpi() {
        reserved_meter_snapshot(
            "reserved_resume_meter_light_high_dpi",
            crate::os_theme::ResolvedTheme::Light,
            1.5,
        );
    }
    #[test]
    fn reserved_resume_meter_snapshot_dark_high_dpi() {
        reserved_meter_snapshot(
            "reserved_resume_meter_dark_high_dpi",
            crate::os_theme::ResolvedTheme::Dark,
            1.5,
        );
    }

    fn snapshot_with_fixture(
        name: &str,
        theme: crate::os_theme::ResolvedTheme,
        dpi: f32,
        mut build_ui: impl FnMut(&mut egui::Ui),
    ) {
        let mut fonts_ready = false;
        let mut harness = egui_kittest::Harness::builder()
            .with_size(egui::vec2(480.0, 400.0))
            .with_pixels_per_point(dpi)
            .build(move |ctx| {
                crate::os_theme::apply_resolved(ctx, theme);
                if !fonts_ready {
                    crate::ui_fonts::configure_fonts(ctx);
                    fonts_ready = true;
                    ctx.request_repaint();
                    return;
                }
                egui::CentralPanel::default().show(ctx, |ui| build_ui(ui));
            });
        harness.run();
        harness.snapshot(name);
    }

    #[test]
    fn book_resume_meter_snapshot_light() {
        snapshot_with_fixture(
            "book_resume_meter_light",
            crate::os_theme::ResolvedTheme::Light,
            1.0,
            fixture,
        );
    }

    #[test]
    fn book_resume_meter_snapshot_dark() {
        snapshot_with_fixture(
            "book_resume_meter_dark",
            crate::os_theme::ResolvedTheme::Dark,
            1.0,
            fixture,
        );
    }

    #[test]
    fn book_resume_meter_snapshot_dark_high_dpi() {
        snapshot_with_fixture(
            "book_resume_meter_dark_high_dpi",
            crate::os_theme::ResolvedTheme::Dark,
            1.5,
            fixture,
        );
    }

    #[test]
    fn book_resume_meter_snapshot_light_high_dpi() {
        snapshot_with_fixture(
            "book_resume_meter_light_high_dpi",
            crate::os_theme::ResolvedTheme::Light,
            1.5,
            fixture,
        );
    }

    #[test]
    fn media_resume_meter_snapshot_light_high_dpi() {
        snapshot_with_fixture(
            "media_resume_meter_light_high_dpi",
            crate::os_theme::ResolvedTheme::Light,
            1.5,
            |ui| media_fixture(ui, true),
        );
    }

    #[test]
    fn media_resume_meter_snapshot_dark_high_dpi() {
        snapshot_with_fixture(
            "media_resume_meter_dark_high_dpi",
            crate::os_theme::ResolvedTheme::Dark,
            1.5,
            |ui| media_fixture(ui, true),
        );
    }

    #[test]
    fn media_resume_meter_snapshot_light() {
        snapshot_with_fixture(
            "media_resume_meter_light",
            crate::os_theme::ResolvedTheme::Light,
            1.0,
            |ui| media_fixture(ui, true),
        );
    }

    #[test]
    fn media_resume_meter_snapshot_dark() {
        snapshot_with_fixture(
            "media_resume_meter_dark",
            crate::os_theme::ResolvedTheme::Dark,
            1.0,
            |ui| media_fixture(ui, true),
        );
    }

    #[test]
    fn media_resume_meter_snapshot_badge_off_light() {
        snapshot_with_fixture(
            "media_resume_meter_badge_off_light",
            crate::os_theme::ResolvedTheme::Light,
            1.0,
            |ui| media_fixture(ui, false),
        );
    }

    #[test]
    fn media_resume_meter_snapshot_badge_off_dark() {
        snapshot_with_fixture(
            "media_resume_meter_badge_off_dark",
            crate::os_theme::ResolvedTheme::Dark,
            1.0,
            |ui| media_fixture(ui, false),
        );
    }
}

#[cfg(test)]
mod cut_content_paint_tests {
    use super::*;
    use std::path::PathBuf;

    fn collect_shapes<'a>(shape: &'a egui::epaint::Shape, out: &mut Vec<&'a egui::epaint::Shape>) {
        out.push(shape);
        if let egui::epaint::Shape::Vec(children) = shape {
            for child in children {
                collect_shapes(child, out);
            }
        }
    }

    #[test]
    fn cut_opacity_fades_cell_content_but_keeps_selection_and_check_opaque() {
        let ctx = egui::Context::default();
        let cell = egui::Rect::from_min_size(egui::pos2(20.0, 20.0), egui::vec2(120.0, 90.0));
        let output = ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(180.0, 140.0),
                )),
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default()
                    .frame(egui::Frame::NONE)
                    .show(ctx, |ui| {
                        draw_cell(
                            ui,
                            cell,
                            true,
                            true,
                            false,
                            &ThumbnailOverlayLayout {
                                check: Some(crate::thumb_overlay_layout::check_overlay_rect(cell)),
                                ..Default::default()
                            },
                            &GridItem::Image(PathBuf::from(r"C:\cut.jpg")),
                            &ThumbnailState::Pending,
                            crate::rotation_db::Rotation::None,
                            None,
                            false,
                            VideoThumbnailIndicator::default(),
                            AudioThumbnailIndicator::default(),
                            true,
                            None,
                        );
                    });
            },
        );
        let mut shapes = Vec::new();
        for clipped in &output.shapes {
            collect_shapes(&clipped.shape, &mut shapes);
        }

        assert!(
            shapes.iter().any(|shape| matches!(
                shape,
                egui::epaint::Shape::Rect(rect)
                    if rect.rect == cell && rect.fill.a() == 255
            )),
            "selected cell background must stay opaque"
        );
        assert!(
            shapes.iter().any(|shape| matches!(
                shape,
                egui::epaint::Shape::Rect(rect)
                    if rect.rect == cell && rect.stroke.color.a() == 255
            )),
            "selection border must stay opaque"
        );
        assert!(
            shapes.iter().any(|shape| matches!(
                shape,
                egui::epaint::Shape::Rect(rect)
                    if rect.rect == cell.shrink(4.0) && (127..=128).contains(&rect.fill.a())
            )),
            "thumbnail placeholder must use the faded content painter"
        );
        assert!(
            shapes.iter().any(|shape| matches!(
                shape,
                egui::epaint::Shape::Circle(circle) if circle.fill.a() == 255
            )),
            "check circle must stay opaque"
        );
        assert!(
            shapes.iter().any(|shape| matches!(
                shape,
                egui::epaint::Shape::Circle(circle)
                    if circle.fill == egui::Color32::from_rgba_unmultiplied(0, 0, 0, 190)
            )),
            "cut badge background must be painted at normal alpha"
        );
    }

    #[test]
    fn cut_badge_is_vector_at_small_size_and_replaces_video_play_indicator() {
        fn paint_video(is_cut: bool) -> Vec<egui::epaint::Shape> {
            let ctx = egui::Context::default();
            let output = ctx.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(80.0, 80.0),
                    )),
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default()
                        .frame(egui::Frame::NONE)
                        .show(ctx, |ui| {
                            draw_cell(
                                ui,
                                egui::Rect::from_min_size(
                                    egui::pos2(10.0, 10.0),
                                    egui::vec2(48.0, 36.0),
                                ),
                                false,
                                false,
                                false,
                                &ThumbnailOverlayLayout::default(),
                                &GridItem::Video(PathBuf::from(r"C:\cut.mp4")),
                                &ThumbnailState::Pending,
                                crate::rotation_db::Rotation::None,
                                None,
                                false,
                                VideoThumbnailIndicator::PlayIcon,
                                AudioThumbnailIndicator::default(),
                                is_cut,
                                None,
                            );
                        });
                },
            );
            let mut shapes = Vec::new();
            for clipped in &output.shapes {
                collect_shapes(&clipped.shape, &mut shapes);
            }
            shapes.into_iter().cloned().collect()
        }

        let normal = paint_video(false);
        assert!(normal.iter().any(|shape| matches!(
            shape,
            egui::epaint::Shape::Circle(circle)
                if circle.fill == egui::Color32::from_rgba_unmultiplied(0, 0, 0, 160)
        )));

        let cut = paint_video(true);
        assert!(!cut.iter().any(|shape| matches!(
            shape,
            egui::epaint::Shape::Circle(circle)
                if circle.fill == egui::Color32::from_rgba_unmultiplied(0, 0, 0, 160)
        )));
        assert!(cut.iter().any(|shape| matches!(
            shape,
            egui::epaint::Shape::Circle(circle)
                if circle.fill == egui::Color32::from_rgba_unmultiplied(0, 0, 0, 190)
        )));
        let opaque_handle_rings = cut
            .iter()
            .filter(|shape| {
                matches!(
                    shape,
                    egui::epaint::Shape::Circle(circle)
                        if circle.fill == egui::Color32::TRANSPARENT
                            && circle.stroke.color == egui::Color32::WHITE
                            && circle.stroke.width >= 1.2
                )
            })
            .count();
        assert_eq!(opaque_handle_rings, 2, "two vector scissors handle rings");
    }
}

/// サムネイル画質プレビュー用: 実グリッドと同じ `cell_w × cell_h` のセルを描画する。
/// 白背景 + 4px パディング、画像はアスペクト保持で中央配置（draw_cell と同じ方式）。
/// クリック可能で、クリック時は Response.clicked() が true になる。
pub(crate) fn tq_draw_preview(
    ui: &mut egui::Ui,
    tex: &Option<egui::TextureHandle>,
    cell_w: f32,
    cell_h: f32,
) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(cell_w, cell_h), egui::Sense::click());
    let painter = ui.painter();
    // 白背景（選択状態ではないグリッドセルと同じ）
    painter.rect_filled(rect, 2.0, egui::Color32::WHITE);

    let padding = 4.0;
    let inner = rect.shrink(padding);

    match tex {
        Some(t) => {
            let tex_size = t.size_vec2();
            let scale = (inner.width() / tex_size.x).min(inner.height() / tex_size.y);
            let img_size = tex_size * scale;
            let img_rect = egui::Rect::from_center_size(inner.center(), img_size);
            painter.image(
                t.id(),
                img_rect,
                egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                egui::Color32::WHITE,
            );
        }
        None => {
            painter.text(
                inner.center(),
                egui::Align2::CENTER_CENTER,
                "エンコード失敗",
                egui::FontId::proportional(14.0),
                egui::Color32::from_gray(120),
            );
        }
    }

    // ホバー時にカーソル変更 + 縁を青くしてクリック可能さを示す
    if response.hovered() {
        painter.rect_stroke(
            rect,
            2.0,
            egui::Stroke::new(2.0, egui::Color32::from_rgb(100, 150, 220)),
            egui::StrokeKind::Outside,
        );
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }

    response
}

/// Snapshot fixture for the three typed unavailable projections used by a collection Grid.
pub fn draw_collection_placeholder_snapshot_fixture(ui: &mut egui::Ui) {
    use crate::grid_item::CollectionPlaceholderReason;
    let cases = [
        ("missing.png", CollectionPlaceholderReason::Missing),
        ("unsupported.bin", CollectionPlaceholderReason::Unsupported),
        (
            "access-denied.jpg",
            CollectionPlaceholderReason::AccessError,
        ),
    ];
    ui.horizontal(|ui| {
        for (index, (name, reason)) in cases.into_iter().enumerate() {
            let (rect, _) = ui.allocate_exact_size(egui::vec2(145.0, 118.0), egui::Sense::hover());
            let layout = ThumbnailOverlayLayout {
                check: (index == 1).then(|| crate::thumb_overlay_layout::check_overlay_rect(rect)),
                ..Default::default()
            };
            draw_cell(
                ui,
                rect,
                index == 0,
                index == 1,
                false,
                &layout,
                &GridItem::CollectionPlaceholder {
                    path: std::path::PathBuf::from(name),
                    last_known_kind: crate::collection_store::CollectionResolvedKind::Image,
                    reason,
                },
                &ThumbnailState::Failed,
                crate::rotation_db::Rotation::None,
                None,
                false,
                VideoThumbnailIndicator::default(),
                AudioThumbnailIndicator::default(),
                false,
                None,
            );
        }
    });
}

#[allow(clippy::too_many_arguments)]
fn draw_video_indicator_snapshot_cell(
    ui: &mut egui::Ui,
    size: egui::Vec2,
    label: &str,
    item: &GridItem,
    thumb: &ThumbnailState,
    indicator: VideoThumbnailIndicator,
    audio_indicator: AudioThumbnailIndicator,
    selected: bool,
    rating: u8,
    tags: &[String],
) -> egui::Response {
    ui.label(egui::RichText::new(label).small());
    let (rect, response) = ui.allocate_exact_size(size, egui::Sense::hover());
    let layout = layout_cell_overlays(
        ui.painter(),
        rect,
        EditBadgeFlags::default(),
        rating,
        item,
        thumb,
        tags,
        None,
        false,
        indicator,
        audio_indicator,
        false,
        None,
        None,
        false,
    );
    draw_cell(
        ui,
        rect,
        selected,
        false,
        false,
        &layout,
        item,
        thumb,
        crate::rotation_db::Rotation::None,
        None,
        false,
        indicator,
        audio_indicator,
        false,
        None,
    );
    if let Some(tag_rect) = grid_tag_badge_hit_rect(&layout)
        && response.hovered()
        && ui
            .ctx()
            .input(|input| input.pointer.hover_pos())
            .is_some_and(|pos| tag_rect.contains(pos))
    {
        response.clone().on_hover_text("#hover をタグビューで探す");
    }
    response
}

fn snapshot_thumbnail(ctx: &egui::Context, name: &str) -> ThumbnailState {
    let size = [48, 27];
    let mut pixels = Vec::with_capacity(size[0] * size[1]);
    for y in 0..size[1] {
        for x in 0..size[0] {
            let color = if x < size[0] / 2 {
                egui::Color32::from_rgb(58 + y as u8 * 2, 92, 128 + x as u8)
            } else {
                egui::Color32::from_rgb(134, 72 + y as u8 * 2, 76)
            };
            pixels.push(color);
        }
    }
    ThumbnailState::Loaded {
        tex: ctx.load_texture(
            name,
            egui::ColorImage::new(size, pixels),
            Default::default(),
        ),
        origin: crate::thumb_loader::ThumbLoadOrigin::SourceIntrinsic,
        from_edit_preview: false,
        rendered_at_px: 128,
        source_dims: Some((1920, 1080)),
        layout_dims: None,
    }
}

/// Snapshot fixture that routes every cell through the production grid layout and paint helpers.
#[doc(hidden)]
pub fn draw_video_thumbnail_indicator_snapshot_fixture(ui: &mut egui::Ui) {
    use std::path::PathBuf;

    ui.set_width(440.0);
    ui.spacing_mut().item_spacing = egui::vec2(8.0, 4.0);
    ui.heading("動画サムネイルの目印");
    let loaded = snapshot_thumbnail(ui.ctx(), "video-indicator-snapshot");
    let video = GridItem::Video(PathBuf::from("scene.mp4"));
    let audio = GridItem::Audio(PathBuf::from("song.flac"));
    let dense_tags = vec!["#hover".to_owned(), "旅行".to_owned()];

    ui.horizontal(|ui| {
        ui.vertical(|ui| {
            draw_video_indicator_snapshot_cell(
                ui,
                egui::vec2(132.0, 88.0),
                "既定・代表画像あり",
                &video,
                &loaded,
                VideoThumbnailIndicator::PlayIcon,
                AudioThumbnailIndicator::default(),
                false,
                0,
                &[],
            );
        });
        ui.vertical(|ui| {
            draw_video_indicator_snapshot_cell(
                ui,
                egui::vec2(132.0, 88.0),
                "既定・生成中",
                &video,
                &ThumbnailState::Pending,
                VideoThumbnailIndicator::PlayIcon,
                AudioThumbnailIndicator::default(),
                false,
                0,
                &[],
            );
        });
        ui.vertical(|ui| {
            draw_video_indicator_snapshot_cell(
                ui,
                egui::vec2(132.0, 88.0),
                "なし・生成中",
                &video,
                &ThumbnailState::Pending,
                VideoThumbnailIndicator::Hidden,
                AudioThumbnailIndicator::default(),
                false,
                0,
                &[],
            );
        });
    });

    ui.horizontal(|ui| {
        ui.vertical(|ui| {
            draw_video_indicator_snapshot_cell(
                ui,
                egui::vec2(238.0, 104.0),
                "左下・選択・評価 / タグ",
                &video,
                &loaded,
                VideoThumbnailIndicator::BottomLeftBadge,
                AudioThumbnailIndicator::default(),
                true,
                4,
                &dense_tags,
            );
        });
        ui.vertical(|ui| {
            draw_video_indicator_snapshot_cell(
                ui,
                egui::vec2(160.0, 104.0),
                "左下・狭いセル / hover",
                &video,
                &ThumbnailState::Pending,
                VideoThumbnailIndicator::BottomLeftBadge,
                AudioThumbnailIndicator::default(),
                false,
                3,
                &dense_tags,
            );
        });
    });

    ui.horizontal(|ui| {
        for indicator in VideoThumbnailIndicator::all() {
            ui.vertical(|ui| {
                draw_video_indicator_snapshot_cell(
                    ui,
                    egui::vec2(132.0, 64.0),
                    indicator.label(),
                    &audio,
                    &ThumbnailState::Pending,
                    *indicator,
                    AudioThumbnailIndicator::default(),
                    false,
                    0,
                    &[],
                );
            });
        }
    });
}

#[cfg(test)]
mod bottom_left_content_tests {
    use super::{
        BottomContainerKind, FormatBadgeKind, VIDEO_THUMBNAIL_BADGE_LABEL, bottom_left_content,
        video_thumbnail_indicator_parts,
    };
    use crate::grid_item::{GridItem, ThumbnailState};
    use crate::settings::{AudioThumbnailIndicator, VideoThumbnailIndicator};
    use std::path::PathBuf;

    // 左下レーンのコンテナバッジ / ファイル名プレートの出し分け。レーン共通化 (§2.2) の前は
    // `cell_has_lower_left_container_badge` として同じ規則を検証していた。ユーザー報告
    // 「フォルダ名と ★ が重なる」の退行ガードなので、レイアウト層へ移した後も残す。

    fn loaded() -> ThumbnailState {
        let ctx = egui::Context::default();
        ThumbnailState::Loaded {
            tex: ctx.load_texture(
                "dummy",
                egui::ColorImage::new([1, 1], vec![egui::Color32::WHITE]),
                Default::default(),
            ),
            origin: crate::thumb_loader::ThumbLoadOrigin::SourceGenerated {
                evaluated_display_px: 128,
            },
            from_edit_preview: false,
            rendered_at_px: 128,
            source_dims: None,
            layout_dims: None,
        }
    }

    #[test]
    fn folder_shows_its_name_badge_once_the_thumbnail_is_loaded() {
        let folder = GridItem::Folder(PathBuf::from("c:/x"));
        let content = bottom_left_content(
            &folder,
            &loaded(),
            "x",
            VideoThumbnailIndicator::PlayIcon,
            AudioThumbnailIndicator::default(),
        );
        assert_eq!(content.container_kind, Some(BottomContainerKind::Folder));
        assert_eq!(content.container_label, Some("x"));
        assert_eq!(content.filename, None);
    }

    #[test]
    fn folder_falls_back_to_a_filename_plate_until_it_is_loaded() {
        let folder = GridItem::Folder(PathBuf::from("c:/x"));
        for (label, thumb) in [
            ("pending", ThumbnailState::Pending),
            ("evicted", ThumbnailState::Evicted),
            ("failed", ThumbnailState::Failed),
        ] {
            let content = bottom_left_content(
                &folder,
                &thumb,
                "x",
                VideoThumbnailIndicator::PlayIcon,
                AudioThumbnailIndicator::default(),
            );
            assert_eq!(content.container_kind, None, "{label}");
            assert_eq!(content.filename, Some("x"), "{label}");
        }
    }

    #[test]
    fn archive_types_always_show_a_format_badge() {
        let zip = GridItem::ZipFile(PathBuf::from("c:/x.zip"));
        let pdf = GridItem::PdfFile(PathBuf::from("c:/x.pdf"));
        let epub = GridItem::PdfFile(PathBuf::from("c:/x.EPUB"));
        let archive = GridItem::ConvertibleArchive {
            path: PathBuf::from("c:/x.7z"),
            format: crate::archive_converter::ArchiveFormat::SevenZ,
        };
        for indicator in VideoThumbnailIndicator::all() {
            for thumb in [
                ThumbnailState::Pending,
                ThumbnailState::Evicted,
                ThumbnailState::Failed,
            ] {
                for (item, label, kind) in [
                    (&zip, "ZIP", FormatBadgeKind::Zip),
                    (&pdf, "PDF", FormatBadgeKind::Pdf),
                    (&epub, "EPUB", FormatBadgeKind::Pdf),
                    (&archive, "7z", FormatBadgeKind::Archive),
                ] {
                    let content = bottom_left_content(
                        item,
                        &thumb,
                        "x",
                        *indicator,
                        AudioThumbnailIndicator::default(),
                    );
                    assert_eq!(
                        content.container_kind,
                        Some(BottomContainerKind::Format(kind)),
                        "{label} / {indicator:?}"
                    );
                    assert_eq!(content.container_label, Some(label));
                }
            }
        }
    }

    #[test]
    fn nested_archive_badge_types_preserve_zip_blue_and_converted_archive_orange() {
        let nested_zip = GridItem::ZipDir {
            zip_path: PathBuf::from("c:/outer.zip"),
            dir_prefix: "comic.cbz/".to_owned(),
            is_archive: true,
            representative: None,
        };
        let nested_rar = GridItem::ZipDir {
            zip_path: PathBuf::from("c:/outer.zip"),
            dir_prefix: "comic.rar/".to_owned(),
            is_archive: true,
            representative: None,
        };
        let zip = bottom_left_content(
            &nested_zip,
            &ThumbnailState::Pending,
            "comic.cbz",
            VideoThumbnailIndicator::Hidden,
            AudioThumbnailIndicator::default(),
        );
        let rar = bottom_left_content(
            &nested_rar,
            &ThumbnailState::Pending,
            "comic.rar",
            VideoThumbnailIndicator::Hidden,
            AudioThumbnailIndicator::default(),
        );
        assert_eq!(
            zip.container_kind,
            Some(BottomContainerKind::Format(FormatBadgeKind::Zip))
        );
        assert_eq!(zip.container_label, Some("ZIP"));
        assert_eq!(
            rar.container_kind,
            Some(BottomContainerKind::Format(FormatBadgeKind::Archive))
        );
        assert_eq!(rar.container_label, Some("RAR"));
    }

    #[test]
    fn image_like_items_have_no_container_badge() {
        let image = GridItem::Image(PathBuf::from("c:/x.jpg"));
        let zip_image = GridItem::ZipImage {
            zip_path: PathBuf::from("c:/x.zip"),
            entry_name: "p1.jpg".to_string(),
        };
        let pdf_page = GridItem::PdfPage {
            pdf_path: PathBuf::from("c:/x.pdf"),
            page_num: 0,
            content_type: None,
        };
        for item in [&image, &zip_image, &pdf_page] {
            let content = bottom_left_content(
                item,
                &ThumbnailState::Pending,
                "x",
                VideoThumbnailIndicator::PlayIcon,
                AudioThumbnailIndicator::default(),
            );
            assert_eq!(content.container_kind, None);
        }
        // 既定値では動画・音声ともコンテナバッジを持たず、ファイル名プレートは出る。
        let video = GridItem::Video(PathBuf::from("c:/x.mp4"));
        let content = bottom_left_content(
            &video,
            &ThumbnailState::Pending,
            "x.mp4",
            VideoThumbnailIndicator::default(),
            AudioThumbnailIndicator::default(),
        );
        assert_eq!(content.container_kind, None);
        assert_eq!(content.filename, Some("x.mp4"));
    }

    #[test]
    fn video_indicator_modes_are_mutually_exclusive_and_keep_the_existing_default() {
        let video = GridItem::Video(PathBuf::from("c:/clip.mp4"));
        for (indicator, play_icon, badge) in [
            (VideoThumbnailIndicator::PlayIcon, true, false),
            (VideoThumbnailIndicator::BottomLeftBadge, false, true),
            (VideoThumbnailIndicator::Hidden, false, false),
        ] {
            let parts = video_thumbnail_indicator_parts(&video, indicator);
            let content = bottom_left_content(
                &video,
                &ThumbnailState::Pending,
                "clip.mp4",
                indicator,
                AudioThumbnailIndicator::default(),
            );
            assert_eq!(parts.play_icon, play_icon, "{indicator:?}");
            assert_eq!(parts.bottom_left_badge, badge, "{indicator:?}");
            assert!(!(parts.play_icon && parts.bottom_left_badge));
            assert_eq!(content.container_kind.is_some(), badge, "{indicator:?}");
            if badge {
                assert_eq!(
                    content.container_kind,
                    Some(BottomContainerKind::Format(FormatBadgeKind::Video))
                );
                assert_eq!(content.container_label, Some(VIDEO_THUMBNAIL_BADGE_LABEL));
            }
        }

        let default_parts =
            video_thumbnail_indicator_parts(&video, VideoThumbnailIndicator::default());
        assert!(default_parts.play_icon);
        assert!(!default_parts.bottom_left_badge);
    }

    #[test]
    fn audio_cell_contract_is_unchanged_for_every_video_indicator_mode() {
        let audio = GridItem::Audio(PathBuf::from("c:/song.flac"));
        let expected = bottom_left_content(
            &audio,
            &ThumbnailState::Pending,
            "song.flac",
            VideoThumbnailIndicator::PlayIcon,
            AudioThumbnailIndicator::default(),
        );
        for &indicator in VideoThumbnailIndicator::all() {
            assert_eq!(
                bottom_left_content(
                    &audio,
                    &ThumbnailState::Pending,
                    "song.flac",
                    indicator,
                    AudioThumbnailIndicator::default(),
                ),
                expected,
                "{indicator:?}"
            );
            assert_eq!(
                video_thumbnail_indicator_parts(&audio, indicator),
                super::VideoThumbnailIndicatorParts {
                    play_icon: false,
                    bottom_left_badge: false,
                }
            );
        }
    }
}

/// Headless fixture: loaded art, terminal/pending fallback and cut/resume overlays use production painting.
#[doc(hidden)]
pub fn draw_audio_thumbnail_indicator_snapshot_fixture(ui: &mut egui::Ui) {
    let audio = GridItem::Audio(std::path::PathBuf::from("song.mp3"));
    let loaded = snapshot_thumbnail(ui.ctx(), "audio-indicator-art");
    let tags = vec!["音楽".to_owned()];
    ui.set_width(440.0);
    ui.spacing_mut().item_spacing = egui::vec2(6.0, 4.0);
    ui.heading("音声サムネイルの目印");
    ui.horizontal(|ui| {
        for &indicator in AudioThumbnailIndicator::all() {
            ui.vertical(|ui| {
                draw_video_indicator_snapshot_cell(
                    ui,
                    egui::vec2(136.0, 90.0),
                    indicator.label(),
                    &audio,
                    &loaded,
                    VideoThumbnailIndicator::Hidden,
                    indicator,
                    false,
                    3,
                    &tags,
                );
            });
        }
    });
    ui.horizontal(|ui| {
        for (label, thumb) in [
            ("画像なし", ThumbnailState::NoArt),
            ("失敗・なし設定", ThumbnailState::Failed),
            ("生成中", ThumbnailState::Pending),
        ] {
            ui.vertical(|ui| {
                draw_video_indicator_snapshot_cell(
                    ui,
                    egui::vec2(136.0, 84.0),
                    label,
                    &audio,
                    &thumb,
                    VideoThumbnailIndicator::Hidden,
                    AudioThumbnailIndicator::Hidden,
                    false,
                    0,
                    &[],
                );
            });
        }
    });
    ui.horizontal(|ui| {
        for &indicator in AudioThumbnailIndicator::all() {
            ui.vertical(|ui| {
                ui.small("切り取り・再生位置");
                let (rect, _) =
                    ui.allocate_exact_size(egui::vec2(136.0, 84.0), egui::Sense::hover());
                let layout = layout_cell_overlays(
                    ui.painter(),
                    rect,
                    EditBadgeFlags::default(),
                    3,
                    &audio,
                    &loaded,
                    &tags,
                    None,
                    false,
                    VideoThumbnailIndicator::Hidden,
                    indicator,
                    false,
                    None,
                    Some("3:45"),
                    true,
                );
                draw_cell(
                    ui,
                    rect,
                    false,
                    false,
                    false,
                    &layout,
                    &audio,
                    &loaded,
                    crate::rotation_db::Rotation::None,
                    None,
                    false,
                    VideoThumbnailIndicator::Hidden,
                    indicator,
                    true,
                    Some(0.6),
                );
            });
        }
    });
}

#[cfg(test)]
mod audio_indicator_tests {
    use super::*;
    #[test]
    fn audio_thumbnail_indicator_terminal_fallback_is_independent_of_setting() {
        let audio = GridItem::Audio("song.mp3".into());
        for thumb in [
            ThumbnailState::Pending,
            ThumbnailState::Evicted,
            ThumbnailState::NoArt,
            ThumbnailState::Failed,
        ] {
            for &indicator in AudioThumbnailIndicator::all() {
                assert_eq!(
                    audio_thumbnail_indicator_parts(&audio, &thumb, indicator),
                    AudioThumbnailIndicatorParts {
                        music_icon: true,
                        bottom_left_badge: false
                    }
                );
            }
        }
    }

    #[test]
    fn audio_thumbnail_indicator_loaded_art_has_exactly_the_requested_marker() {
        let ctx = egui::Context::default();
        let loaded = snapshot_thumbnail(&ctx, "audio-indicator-test");
        let audio = GridItem::Audio("song.mp3".into());
        for (indicator, expected) in [
            (
                AudioThumbnailIndicator::MusicNoteIcon,
                AudioThumbnailIndicatorParts {
                    music_icon: true,
                    bottom_left_badge: false,
                },
            ),
            (
                AudioThumbnailIndicator::BottomLeftBadge,
                AudioThumbnailIndicatorParts {
                    music_icon: false,
                    bottom_left_badge: true,
                },
            ),
            (
                AudioThumbnailIndicator::Hidden,
                AudioThumbnailIndicatorParts {
                    music_icon: false,
                    bottom_left_badge: false,
                },
            ),
            (
                AudioThumbnailIndicator::Unknown,
                AudioThumbnailIndicatorParts {
                    music_icon: true,
                    bottom_left_badge: false,
                },
            ),
        ] {
            assert_eq!(
                audio_thumbnail_indicator_parts(&audio, &loaded, indicator),
                expected
            );
            let content = bottom_left_content(
                &audio,
                &loaded,
                "song.mp3",
                VideoThumbnailIndicator::PlayIcon,
                indicator,
            );
            assert_eq!(
                content.container_kind,
                expected
                    .bottom_left_badge
                    .then_some(BottomContainerKind::Format(FormatBadgeKind::Audio))
            );
        }
        let video = GridItem::Video("scene.mp4".into());
        assert_eq!(
            audio_thumbnail_indicator_parts(
                &video,
                &loaded,
                AudioThumbnailIndicator::MusicNoteIcon
            ),
            AudioThumbnailIndicatorParts {
                music_icon: false,
                bottom_left_badge: false
            }
        );
    }
}
