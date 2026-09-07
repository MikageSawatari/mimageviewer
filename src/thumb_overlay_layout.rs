//! Pure thumbnail-corner overlay layout.
//!
//! This module owns measurement, padding, truncation, and lane placement only. Badge colors,
//! rounding, and painting remain in the individual UI helpers so semantically different badges do
//! not acquire a shared visual style by accident.

use eframe::egui;

pub const TOP_LEFT_OFFSET: f32 = 3.0;
pub const TOP_LEFT_GAP: f32 = 2.0;
pub const TOP_RIGHT_RESERVE: f32 = 28.0;
pub const BOTTOM_LEFT_OFFSET: f32 = 3.0;
pub const BOTTOM_ITEM_GAP: f32 = 4.0;

const FILE_BADGE_SCALE: f32 = 0.70;
const FILE_BADGE_MIN_FONT_SIZE: f32 = 7.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BadgeFontFamily {
    Proportional,
    UserText,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BadgePadding {
    pub left: f32,
    pub right: f32,
    pub top: f32,
    pub bottom: f32,
}

impl BadgePadding {
    pub const fn symmetric(horizontal: f32, vertical: f32) -> Self {
        Self {
            left: horizontal,
            right: horizontal,
            top: vertical,
            bottom: vertical,
        }
    }

    fn size(self) -> egui::Vec2 {
        egui::vec2(self.left + self.right, self.top + self.bottom)
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BadgeTextStyle {
    pub font_size: f32,
    pub family: BadgeFontFamily,
    pub padding: BadgePadding,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EditBadgeKind {
    PageOverride,
    LocalAdjust,
    Mask,
    Conceal,
    Comic,
    Crop,
    Pin,
}

impl EditBadgeKind {
    pub const fn text(self) -> &'static str {
        match self {
            Self::PageOverride => "補",
            Self::LocalAdjust => "レ",
            Self::Mask => "消",
            Self::Conceal => "隠",
            Self::Comic => "文",
            Self::Crop => "切",
            Self::Pin => "📌",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FormatBadgeKind {
    /// Native ZIP / CBZ container.
    Zip,
    /// PDF document.
    Pdf,
    /// Converted or nested RAR / 7z / LZH family.
    Archive,
    /// Video media marker, independent from the file extension label.
    Video,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BottomContainerKind {
    Folder,
    Format(FormatBadgeKind),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BadgeKind {
    BookmarkTime,
    UpscaledVideo,
    Edit(EditBadgeKind),
    Tag,
    BottomContainer(BottomContainerKind),
    Rating,
    Filename,
}

/// Lower values reserve lane space first. The value is recorded in every placement so tests and
/// callers can verify the policy without inferring it from coordinates.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum BadgePriority {
    BookmarkTime,
    UpscaledVideo,
    EditState,
    Tag,
    BottomContainer,
    Filename,
    Rating,
}

#[derive(Clone, Debug, PartialEq)]
pub struct BadgePlacement {
    pub kind: BadgeKind,
    pub priority: BadgePriority,
    pub rect: egui::Rect,
    pub text: String,
    pub style: BadgeTextStyle,
}

impl BadgePlacement {
    pub fn text_pos(&self) -> egui::Pos2 {
        self.rect.min + egui::vec2(self.style.padding.left, self.style.padding.top)
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct TopLeftOverlayLayout {
    pub bookmark_time: Option<BadgePlacement>,
    pub upscaled_video: Option<BadgePlacement>,
    pub edit_badges: Vec<BadgePlacement>,
    pub tag: Option<BadgePlacement>,
}

impl TopLeftOverlayLayout {
    pub fn lane_bottom(&self) -> Option<f32> {
        self.placements()
            .map(|placement| placement.rect.max.y)
            .reduce(f32::max)
    }

    pub fn placements(&self) -> impl Iterator<Item = &BadgePlacement> {
        self.bookmark_time
            .iter()
            .chain(self.upscaled_video.iter())
            .chain(self.edit_badges.iter())
            .chain(self.tag.iter())
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct BottomLeftOverlayLayout {
    pub container: Option<BadgePlacement>,
    pub filename: Option<BadgePlacement>,
    pub rating: Option<BadgePlacement>,
}

impl BottomLeftOverlayLayout {
    pub fn placements(&self) -> impl Iterator<Item = &BadgePlacement> {
        self.container
            .iter()
            .chain(self.filename.iter())
            .chain(self.rating.iter())
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ThumbnailOverlayLayout {
    pub top_left: TopLeftOverlayLayout,
    pub bottom_left: BottomLeftOverlayLayout,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct EditBadgeFlags {
    pub page_override: bool,
    pub local_adjust: bool,
    pub mask: bool,
    pub conceal: bool,
    pub comic: bool,
    pub crop: bool,
    pub pin: bool,
}

impl EditBadgeFlags {
    fn active(self) -> impl Iterator<Item = EditBadgeKind> {
        [
            (self.page_override, EditBadgeKind::PageOverride),
            (self.local_adjust, EditBadgeKind::LocalAdjust),
            (self.mask, EditBadgeKind::Mask),
            (self.conceal, EditBadgeKind::Conceal),
            (self.comic, EditBadgeKind::Comic),
            (self.crop, EditBadgeKind::Crop),
            (self.pin, EditBadgeKind::Pin),
        ]
        .into_iter()
        .filter_map(|(active, kind)| active.then_some(kind))
    }
}

#[derive(Clone, Copy, Debug)]
pub struct BottomContainerInput<'a> {
    pub kind: BottomContainerKind,
    pub label: &'a str,
}

#[derive(Clone, Debug)]
pub struct ThumbnailOverlayLayoutInput<'a> {
    pub cell: egui::Rect,
    pub inner: egui::Rect,
    pub bookmark_time: Option<&'a str>,
    pub upscaled_video: bool,
    pub edit_badges: EditBadgeFlags,
    pub tags: &'a [String],
    pub bottom_container: Option<BottomContainerInput<'a>>,
    pub rating_text: Option<&'a str>,
    pub filename: Option<&'a str>,
}

pub fn bookmark_time_style() -> BadgeTextStyle {
    BadgeTextStyle {
        font_size: 11.0,
        family: BadgeFontFamily::Proportional,
        // The old badge added (10, 5) to the measured text size and centered the text.
        padding: BadgePadding::symmetric(5.0, 2.5),
    }
}

pub fn upscaled_video_style(inner: egui::Rect) -> BadgeTextStyle {
    let font_size = (inner.height() * 0.10).clamp(10.0, 14.0);
    BadgeTextStyle {
        font_size,
        family: BadgeFontFamily::Proportional,
        padding: BadgePadding::symmetric(font_size * 0.45, font_size * 0.22),
    }
}

pub fn edit_badge_style() -> BadgeTextStyle {
    BadgeTextStyle {
        font_size: 11.0,
        family: BadgeFontFamily::Proportional,
        padding: BadgePadding {
            left: 4.0,
            right: 4.0,
            top: 5.0,
            bottom: 1.0,
        },
    }
}

pub fn tag_badge_style() -> BadgeTextStyle {
    BadgeTextStyle {
        font_size: 11.0,
        family: BadgeFontFamily::Proportional,
        padding: BadgePadding {
            left: 5.0,
            right: 5.0,
            top: 5.0,
            bottom: 1.0,
        },
    }
}

pub fn file_badge_font_size(inner: egui::Rect) -> f32 {
    ((inner.height() * 0.10).clamp(9.0, 16.0) * FILE_BADGE_SCALE).max(FILE_BADGE_MIN_FONT_SIZE)
}

pub fn file_badge_style(inner: egui::Rect) -> BadgeTextStyle {
    let font_size = file_badge_font_size(inner);
    BadgeTextStyle {
        font_size,
        family: BadgeFontFamily::Proportional,
        padding: BadgePadding::symmetric(font_size * 0.35, font_size * 0.2),
    }
}

pub fn folder_badge_font_size(inner: egui::Rect) -> f32 {
    let legacy_size = (inner.height() * 0.10).clamp(9.0, 16.0);
    // Folder names are variable-length CJK text, so matching the 70% format badge would make long
    // names unnecessarily hard to read. 85% with an 8.5pt floor keeps glyphs legible, while the
    // 13.5pt cap and tighter dedicated padding remove most of the reported visual imbalance.
    (legacy_size * 0.85).clamp(8.5, 13.5)
}

pub fn folder_badge_style(inner: egui::Rect) -> BadgeTextStyle {
    let font_size = folder_badge_font_size(inner);
    BadgeTextStyle {
        font_size,
        family: BadgeFontFamily::Proportional,
        padding: BadgePadding {
            left: font_size * 0.30,
            right: font_size * 0.30,
            top: font_size * 0.12,
            bottom: font_size * 0.12,
        },
    }
}

pub fn rating_badge_style() -> BadgeTextStyle {
    BadgeTextStyle {
        font_size: 12.0,
        family: BadgeFontFamily::Proportional,
        padding: BadgePadding {
            left: 5.0,
            right: 5.0,
            top: 5.0,
            bottom: 1.0,
        },
    }
}

pub fn filename_badge_style() -> BadgeTextStyle {
    BadgeTextStyle {
        font_size: 11.0,
        family: BadgeFontFamily::UserText,
        padding: BadgePadding {
            left: 4.0,
            right: 4.0,
            top: 4.0,
            bottom: 1.0,
        },
    }
}

pub fn combine_tags(tags: &[String]) -> String {
    let mut combined = String::new();
    for tag in tags.iter().filter(|tag| tag.starts_with('#')) {
        if !combined.is_empty() {
            combined.push(' ');
        }
        combined.push_str(tag);
    }
    for tag in tags.iter().filter(|tag| !tag.starts_with('#')) {
        if !combined.is_empty() {
            combined.push(' ');
        }
        combined.push_str(tag);
    }
    combined
}

/// Lay out the currently visible top-left and bottom-left overlays.
///
/// `measure` is the only connection to font machinery. The function performs no painting and is
/// deterministic for a given rectangle, input, and measurement callback.
pub fn layout_thumbnail_overlays(
    input: ThumbnailOverlayLayoutInput<'_>,
    mut measure: impl FnMut(&str, BadgeTextStyle) -> egui::Vec2,
) -> ThumbnailOverlayLayout {
    let mut top_left = TopLeftOverlayLayout::default();
    let mut cursor_x = input.cell.min.x + TOP_LEFT_OFFSET;
    let top_y = input.cell.min.y + TOP_LEFT_OFFSET;

    if let Some(text) = input.bookmark_time {
        let placement = natural_placement(
            BadgeKind::BookmarkTime,
            BadgePriority::BookmarkTime,
            text,
            bookmark_time_style(),
            egui::pos2(cursor_x, top_y),
            &mut measure,
        );
        cursor_x = placement.rect.max.x + TOP_LEFT_GAP;
        top_left.bookmark_time = Some(placement);
    }

    if input.upscaled_video {
        let placement = natural_placement(
            BadgeKind::UpscaledVideo,
            BadgePriority::UpscaledVideo,
            "UP",
            upscaled_video_style(input.inner),
            egui::pos2(cursor_x, top_y),
            &mut measure,
        );
        cursor_x = placement.rect.max.x + TOP_LEFT_GAP;
        top_left.upscaled_video = Some(placement);
    }

    for kind in input.edit_badges.active() {
        let placement = natural_placement(
            BadgeKind::Edit(kind),
            BadgePriority::EditState,
            kind.text(),
            edit_badge_style(),
            egui::pos2(cursor_x, top_y),
            &mut measure,
        );
        cursor_x = placement.rect.max.x + TOP_LEFT_GAP;
        top_left.edit_badges.push(placement);
    }

    let combined_tags = combine_tags(input.tags);
    if !combined_tags.is_empty() {
        let style = tag_badge_style();
        let max_text_width = input.cell.max.x
            - TOP_RIGHT_RESERVE
            - cursor_x
            - style.padding.left
            - style.padding.right;
        if max_text_width >= 8.0
            && let Some(text) = fit_text(&combined_tags, None, max_text_width, &mut measure, style)
        {
            top_left.tag = Some(natural_placement(
                BadgeKind::Tag,
                BadgePriority::Tag,
                &text,
                style,
                egui::pos2(cursor_x, top_y),
                &mut measure,
            ));
        }
    }

    let mut bottom_left = BottomLeftOverlayLayout::default();
    let bottom_y = input.inner.max.y - BOTTOM_LEFT_OFFSET;
    let left_x = input.inner.min.x + BOTTOM_LEFT_OFFSET;

    if let Some(container) = input.bottom_container {
        let style = match container.kind {
            BottomContainerKind::Folder => folder_badge_style(input.inner),
            BottomContainerKind::Format(_) => file_badge_style(input.inner),
        };
        let max_badge_width = match container.kind {
            BottomContainerKind::Folder => input.inner.width() * 0.80,
            BottomContainerKind::Format(_) => input.inner.width() - BOTTOM_LEFT_OFFSET * 2.0,
        };
        let max_text_width = (max_badge_width - style.padding.left - style.padding.right).max(0.0);
        if let Some(text) = fit_text(container.label, None, max_text_width, &mut measure, style) {
            let size = measured_badge_size(&text, style, &mut measure);
            bottom_left.container = Some(BadgePlacement {
                kind: BadgeKind::BottomContainer(container.kind),
                priority: BadgePriority::BottomContainer,
                rect: egui::Rect::from_min_size(egui::pos2(left_x, bottom_y - size.y), size),
                text,
                style,
            });
        }
    }

    if let Some(filename) = input.filename {
        let style = filename_badge_style();
        let available_left = bottom_left
            .container
            .as_ref()
            .map_or(left_x, |container| container.rect.max.x + BOTTOM_ITEM_GAP);
        let available_right = input.inner.max.x - BOTTOM_LEFT_OFFSET;
        let max_text_width =
            available_right - available_left - style.padding.left - style.padding.right;
        if max_text_width >= 4.0
            && let Some(text) = fit_text(filename, Some(18), max_text_width, &mut measure, style)
        {
            let size = measured_badge_size(&text, style, &mut measure);
            let center_x = (available_left + available_right) * 0.5;
            bottom_left.filename = Some(BadgePlacement {
                kind: BadgeKind::Filename,
                priority: BadgePriority::Filename,
                rect: egui::Rect::from_min_size(
                    egui::pos2(center_x - size.x * 0.5, bottom_y - size.y),
                    size,
                ),
                text,
                style,
            });
        }
    }

    if let Some(rating_text) = input.rating_text {
        let style = rating_badge_style();
        let size = measured_badge_size(rating_text, style, &mut measure);
        // The rating belongs in the corner. It only moves up when something already in the
        // bottom row would sit under it: a container badge is anchored to the same corner and
        // always does, a filename plate is centred and usually does not. Lifting it
        // unconditionally left the stars floating over the picture on a plain video cell.
        let corner = egui::Rect::from_min_size(egui::pos2(left_x, bottom_y - size.y), size);
        let blocked = bottom_left
            .container
            .iter()
            .chain(bottom_left.filename.iter())
            .any(|placement| placement.rect.intersects(corner));
        let rect = if blocked {
            let row_top = bottom_left
                .container
                .iter()
                .chain(bottom_left.filename.iter())
                .map(|placement| placement.rect.min.y)
                .reduce(f32::min)
                .unwrap_or(bottom_y);
            egui::Rect::from_min_size(egui::pos2(left_x, row_top - BOTTOM_ITEM_GAP - size.y), size)
        } else {
            corner
        };
        bottom_left.rating = Some(BadgePlacement {
            kind: BadgeKind::Rating,
            priority: BadgePriority::Rating,
            rect,
            text: rating_text.to_owned(),
            style,
        });
    }

    ThumbnailOverlayLayout {
        top_left,
        bottom_left,
    }
}

fn natural_placement(
    kind: BadgeKind,
    priority: BadgePriority,
    text: &str,
    style: BadgeTextStyle,
    pos: egui::Pos2,
    measure: &mut impl FnMut(&str, BadgeTextStyle) -> egui::Vec2,
) -> BadgePlacement {
    BadgePlacement {
        kind,
        priority,
        rect: egui::Rect::from_min_size(pos, measured_badge_size(text, style, measure)),
        text: text.to_owned(),
        style,
    }
}

fn measured_badge_size(
    text: &str,
    style: BadgeTextStyle,
    measure: &mut impl FnMut(&str, BadgeTextStyle) -> egui::Vec2,
) -> egui::Vec2 {
    measure(text, style) + style.padding.size()
}

fn fit_text(
    text: &str,
    soft_char_cap: Option<usize>,
    max_text_width: f32,
    measure: &mut impl FnMut(&str, BadgeTextStyle) -> egui::Vec2,
    style: BadgeTextStyle,
) -> Option<String> {
    if max_text_width <= 0.0 || text.is_empty() {
        return None;
    }
    let chars: Vec<char> = text.chars().collect();
    let capped = soft_char_cap
        .filter(|cap| chars.len() > *cap)
        .map(|cap| chars[..cap.saturating_sub(1)].iter().collect::<String>() + "…")
        .unwrap_or_else(|| text.to_owned());
    if measure(&capped, style).x <= max_text_width {
        return Some(capped);
    }

    let capped_chars: Vec<char> = capped.chars().collect();
    for take in (1..capped_chars.len()).rev() {
        let candidate = capped_chars[..take].iter().collect::<String>() + "…";
        if measure(&candidate, style).x <= max_text_width {
            return Some(candidate);
        }
    }
    (measure("…", style).x <= max_text_width).then(|| "…".to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn measure(text: &str, style: BadgeTextStyle) -> egui::Vec2 {
        let width = text
            .chars()
            .map(|ch| {
                if ch.is_ascii() {
                    style.font_size * 0.58
                } else {
                    style.font_size
                }
            })
            .sum();
        egui::vec2(width, style.font_size * 1.25)
    }

    fn cell(width: f32) -> (egui::Rect, egui::Rect) {
        let cell = egui::Rect::from_min_size(egui::pos2(10.0, 20.0), egui::vec2(width, 180.0));
        (cell, cell.shrink(4.0))
    }

    fn assert_pairwise_non_intersecting<'a>(
        placements: impl IntoIterator<Item = &'a BadgePlacement>,
    ) {
        let placements: Vec<_> = placements.into_iter().collect();
        for (index, left) in placements.iter().enumerate() {
            for right in placements.iter().skip(index + 1) {
                assert!(!left.rect.intersects(right.rect));
            }
        }
    }

    fn tags(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_owned()).collect()
    }

    #[test]
    fn crop_badge_uses_the_existing_edit_lane_before_pin() {
        let kinds = EditBadgeFlags {
            crop: true,
            pin: true,
            ..Default::default()
        }
        .active()
        .collect::<Vec<_>>();

        assert_eq!(kinds, vec![EditBadgeKind::Crop, EditBadgeKind::Pin]);
        assert_eq!(EditBadgeKind::Crop.text(), "切");
    }

    #[test]
    fn lane_rectangles_do_not_intersect() {
        let (cell, inner) = cell(360.0);
        let tags = tags(&["#風景", "旅行"]);
        let layout = layout_thumbnail_overlays(
            ThumbnailOverlayLayoutInput {
                cell,
                inner,
                bookmark_time: Some("12:34"),
                upscaled_video: true,
                edit_badges: EditBadgeFlags {
                    page_override: true,
                    local_adjust: true,
                    pin: true,
                    ..Default::default()
                },
                tags: &tags,
                bottom_container: Some(BottomContainerInput {
                    kind: BottomContainerKind::Format(FormatBadgeKind::Pdf),
                    label: "PDF",
                }),
                rating_text: Some("📁★★★★★"),
                filename: Some("長い資料ファイル名.pdf"),
            },
            measure,
        );
        assert_pairwise_non_intersecting(layout.top_left.placements());
        assert_pairwise_non_intersecting(layout.bottom_left.placements());
    }

    #[test]
    fn top_left_reservation_order_is_time_up_edit_then_tag() {
        let (cell, inner) = cell(360.0);
        let tags = tags(&["#tag"]);
        let layout = layout_thumbnail_overlays(
            ThumbnailOverlayLayoutInput {
                cell,
                inner,
                bookmark_time: Some("0:07"),
                upscaled_video: true,
                edit_badges: EditBadgeFlags {
                    page_override: true,
                    pin: true,
                    ..Default::default()
                },
                tags: &tags,
                bottom_container: None,
                rating_text: None,
                filename: None,
            },
            measure,
        );
        let placements: Vec<_> = layout.top_left.placements().collect();
        assert_eq!(
            placements.iter().map(|p| p.priority).collect::<Vec<_>>(),
            vec![
                BadgePriority::BookmarkTime,
                BadgePriority::UpscaledVideo,
                BadgePriority::EditState,
                BadgePriority::EditState,
                BadgePriority::Tag,
            ]
        );
        assert!(
            placements
                .windows(2)
                .all(|pair| pair[0].rect.max.x < pair[1].rect.min.x)
        );
    }

    #[test]
    fn narrow_cell_omits_tag_but_keeps_reserved_badges() {
        let (cell, inner) = cell(125.0);
        let tags = tags(&["#very-long-tag"]);
        let layout = layout_thumbnail_overlays(
            ThumbnailOverlayLayoutInput {
                cell,
                inner,
                bookmark_time: Some("12:34"),
                upscaled_video: true,
                edit_badges: EditBadgeFlags {
                    page_override: true,
                    pin: true,
                    ..Default::default()
                },
                tags: &tags,
                bottom_container: None,
                rating_text: None,
                filename: None,
            },
            measure,
        );
        assert!(layout.top_left.bookmark_time.is_some());
        assert!(layout.top_left.upscaled_video.is_some());
        assert_eq!(layout.top_left.edit_badges.len(), 2);
        assert!(layout.top_left.tag.is_none());
    }

    #[test]
    fn up_edit_pin_and_tag_are_simultaneously_non_overlapping() {
        let (cell, inner) = cell(300.0);
        let tags = tags(&["#同時表示"]);
        let layout = layout_thumbnail_overlays(
            ThumbnailOverlayLayoutInput {
                cell,
                inner,
                bookmark_time: None,
                upscaled_video: true,
                edit_badges: EditBadgeFlags {
                    page_override: true,
                    local_adjust: true,
                    mask: true,
                    conceal: true,
                    comic: true,
                    crop: true,
                    pin: true,
                },
                tags: &tags,
                bottom_container: None,
                rating_text: None,
                filename: None,
            },
            measure,
        );
        assert!(layout.top_left.tag.is_some());
        assert_pairwise_non_intersecting(layout.top_left.placements());
    }

    /// A plain video cell has no container badge, and its filename plate is centred, so the
    /// rating stays in the corner where it has always been. Lifting it there left the stars
    /// floating in the middle of the picture (reported on the build of 2026-07-31).
    #[test]
    fn rating_stays_in_the_corner_when_only_a_centred_filename_shares_the_row() {
        let (cell_rect, inner) = cell(240.0);
        let layout = layout_thumbnail_overlays(
            ThumbnailOverlayLayoutInput {
                cell: cell_rect,
                inner,
                bookmark_time: None,
                upscaled_video: false,
                edit_badges: EditBadgeFlags::default(),
                tags: &[],
                bottom_container: None,
                rating_text: Some("★★★"),
                filename: Some("clip.mp4"),
            },
            measure,
        );

        let rating = layout.bottom_left.rating.as_ref().unwrap();
        let filename = layout.bottom_left.filename.as_ref().unwrap();
        assert!(!rating.rect.intersects(filename.rect));
        assert!(
            (rating.rect.max.y - filename.rect.max.y).abs() < f32::EPSILON,
            "the rating should share the bottom row with the plate, not sit above it"
        );
    }

    /// A container badge is anchored to the same corner, so the rating has to move up - this is
    /// the "the folder name overlaps the stars" report the lift exists for.
    #[test]
    fn rating_lifts_above_a_container_badge_in_the_same_corner() {
        let (cell_rect, inner) = cell(240.0);
        let layout = layout_thumbnail_overlays(
            ThumbnailOverlayLayoutInput {
                cell: cell_rect,
                inner,
                bookmark_time: None,
                upscaled_video: false,
                edit_badges: EditBadgeFlags::default(),
                tags: &[],
                bottom_container: Some(BottomContainerInput {
                    kind: BottomContainerKind::Folder,
                    label: "album",
                }),
                rating_text: Some("📁★★★"),
                filename: None,
            },
            measure,
        );

        let rating = layout.bottom_left.rating.as_ref().unwrap();
        let container = layout.bottom_left.container.as_ref().unwrap();
        assert!(!rating.rect.intersects(container.rect));
        assert!(rating.rect.max.y <= container.rect.min.y);
    }

    /// A filename long enough to reach the corner pushes the rating up as well - the rule is
    /// "something is actually under it", not "which kind of badge it is".
    #[test]
    fn rating_lifts_when_a_wide_filename_reaches_the_corner() {
        let (cell_rect, inner) = cell(240.0);
        let layout = layout_thumbnail_overlays(
            ThumbnailOverlayLayoutInput {
                cell: cell_rect,
                inner,
                bookmark_time: None,
                upscaled_video: false,
                edit_badges: EditBadgeFlags::default(),
                tags: &[],
                bottom_container: None,
                rating_text: Some("★★★★★"),
                filename: Some("非常に長い日本語のファイル名がここに入ります.mp4"),
            },
            measure,
        );

        let rating = layout.bottom_left.rating.as_ref().unwrap();
        let filename = layout.bottom_left.filename.as_ref().unwrap();
        assert!(
            filename.rect.min.x <= rating.rect.max.x,
            "設定した前提が崩れている"
        );
        assert!(!rating.rect.intersects(filename.rect));
        assert!(rating.rect.max.y <= filename.rect.min.y);
    }

    #[test]
    fn long_cjk_tag_and_long_folder_with_rating_stay_in_their_lanes() {
        let (cell, inner) = cell(240.0);
        let tags = tags(&["#非常に長い日本語タグ名がここに入ります"]);
        let layout = layout_thumbnail_overlays(
            ThumbnailOverlayLayoutInput {
                cell,
                inner,
                bookmark_time: None,
                upscaled_video: false,
                edit_badges: EditBadgeFlags {
                    page_override: true,
                    pin: true,
                    ..Default::default()
                },
                tags: &tags,
                bottom_container: Some(BottomContainerInput {
                    kind: BottomContainerKind::Folder,
                    label: "とても長い日本語のフォルダー名で可読性を確認する",
                }),
                rating_text: Some("📁★★★★★"),
                filename: None,
            },
            measure,
        );
        let tag = layout.top_left.tag.as_ref().expect("CJK-tag");
        assert!(tag.text.ends_with('…'));
        let folder = layout.bottom_left.container.as_ref().expect("folder");
        let rating = layout.bottom_left.rating.as_ref().expect("rating");
        assert!(folder.text.ends_with('…'));
        assert!(folder.rect.width() <= inner.width() * 0.80 + f32::EPSILON);
        assert!(!folder.rect.intersects(rating.rect));
        assert!(folder.rect.max.x <= inner.max.x);
    }
}
