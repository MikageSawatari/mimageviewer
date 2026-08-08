use eframe::egui;
use serde::{Deserialize, Serialize};

pub const MAX_VIEW_TRIM_MARGIN: f32 = 0.20;
const MIN_CONTENT_FRAC: f32 = 0.01;
const ZERO_EPS: f32 = 0.0005;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ViewTrimApplyMode {
    None,
    Auto,
    Book,
    Page,
}

impl Default for ViewTrimApplyMode {
    fn default() -> Self {
        Self::None
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ViewTrimSpreadSide {
    Left,
    Right,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ViewTrimMargins {
    pub left: f32,
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
}

impl ViewTrimMargins {
    pub fn clamped(self) -> Self {
        let mut left = clamp_margin(self.left);
        let mut top = clamp_margin(self.top);
        let mut right = clamp_margin(self.right);
        let mut bottom = clamp_margin(self.bottom);
        clamp_pair(&mut left, &mut right);
        clamp_pair(&mut top, &mut bottom);
        Self {
            left,
            top,
            right,
            bottom,
        }
    }

    pub fn is_zero(self) -> bool {
        let s = self.clamped();
        s.left <= ZERO_EPS && s.top <= ZERO_EPS && s.right <= ZERO_EPS && s.bottom <= ZERO_EPS
    }

    pub fn bbox(self) -> Option<egui::Rect> {
        let s = self.clamped();
        if s.is_zero() {
            return None;
        }
        Some(egui::Rect::from_min_max(
            egui::pos2(s.left, s.top),
            egui::pos2(1.0 - s.right, 1.0 - s.bottom),
        ))
    }

    pub fn from_bbox(bbox: egui::Rect) -> Self {
        Self {
            left: bbox.min.x,
            top: bbox.min.y,
            right: 1.0 - bbox.max.x,
            bottom: 1.0 - bbox.max.y,
        }
        .clamped()
    }

    pub fn max_with(self, other: Self) -> Self {
        Self {
            left: self.left.max(other.left),
            top: self.top.max(other.top),
            right: self.right.max(other.right),
            bottom: self.bottom.max(other.bottom),
        }
        .clamped()
    }

    pub fn average_with(self, other: Self) -> Self {
        Self {
            left: (self.left + other.left) * 0.5,
            top: (self.top + other.top) * 0.5,
            right: (self.right + other.right) * 0.5,
            bottom: (self.bottom + other.bottom) * 0.5,
        }
        .clamped()
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ViewTrimLinkedMargins {
    pub top: f32,
    pub bottom: f32,
    pub inner: f32,
    pub outer: f32,
}

impl ViewTrimLinkedMargins {
    pub fn clamped(self) -> Self {
        Self {
            top: clamp_margin(self.top),
            bottom: clamp_margin(self.bottom),
            inner: clamp_margin(self.inner),
            outer: clamp_margin(self.outer),
        }
    }

    pub fn is_zero(self) -> bool {
        let s = self.clamped();
        s.top <= ZERO_EPS && s.bottom <= ZERO_EPS && s.inner <= ZERO_EPS && s.outer <= ZERO_EPS
    }

    pub fn margins_for(self, side: ViewTrimSpreadSide) -> ViewTrimMargins {
        let s = self.clamped();
        match side {
            ViewTrimSpreadSide::Left => ViewTrimMargins {
                left: s.outer,
                top: s.top,
                right: s.inner,
                bottom: s.bottom,
            },
            ViewTrimSpreadSide::Right => ViewTrimMargins {
                left: s.inner,
                top: s.top,
                right: s.outer,
                bottom: s.bottom,
            },
        }
    }

    pub fn to_separate(self) -> (ViewTrimMargins, ViewTrimMargins) {
        (
            self.margins_for(ViewTrimSpreadSide::Left),
            self.margins_for(ViewTrimSpreadSide::Right),
        )
    }

    pub fn average_from_separate(left: ViewTrimMargins, right: ViewTrimMargins) -> Self {
        let left = left.clamped();
        let right = right.clamped();
        Self {
            top: (left.top + right.top) * 0.5,
            bottom: (left.bottom + right.bottom) * 0.5,
            inner: (left.right + right.left) * 0.5,
            outer: (left.left + right.right) * 0.5,
        }
        .clamped()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct ViewTrimBookSettings {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub spread_separate: bool,
    #[serde(default)]
    pub single: ViewTrimMargins,
    #[serde(default)]
    pub spread_linked: ViewTrimLinkedMargins,
    #[serde(default)]
    pub spread_left: ViewTrimMargins,
    #[serde(default)]
    pub spread_right: ViewTrimMargins,
}

impl Default for ViewTrimBookSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            spread_separate: false,
            single: ViewTrimMargins::default(),
            spread_linked: ViewTrimLinkedMargins::default(),
            spread_left: ViewTrimMargins::default(),
            spread_right: ViewTrimMargins::default(),
        }
    }
}

impl ViewTrimBookSettings {
    /// 保存・IPC 境界で、本単位設定に含まれる全 margin を正本の規則へ丸める。
    pub fn clamped(self) -> Self {
        Self {
            enabled: self.enabled,
            spread_separate: self.spread_separate,
            single: self.single.clamped(),
            spread_linked: self.spread_linked.clamped(),
            spread_left: self.spread_left.clamped(),
            spread_right: self.spread_right.clamped(),
        }
    }

    /// 見開きの編集表現を切り替え、現在有効な側の値を本体共通規則で引き継ぐ。
    pub fn with_spread_separate(mut self, separate: bool) -> Self {
        if self.spread_separate == separate {
            return self;
        }
        if separate {
            let (left, right) = self.spread_linked.to_separate();
            self.spread_left = left;
            self.spread_right = right;
        } else {
            self.spread_linked =
                ViewTrimLinkedMargins::average_from_separate(self.spread_left, self.spread_right);
        }
        self.spread_separate = separate;
        self
    }

    pub fn single_bbox(self) -> Option<egui::Rect> {
        self.enabled.then_some(())?;
        self.single.bbox()
    }

    pub fn spread_bbox(self, side: ViewTrimSpreadSide) -> Option<egui::Rect> {
        self.enabled.then_some(())?;
        if self.spread_separate {
            match side {
                ViewTrimSpreadSide::Left => self.spread_left.bbox(),
                ViewTrimSpreadSide::Right => self.spread_right.bbox(),
            }
        } else {
            self.spread_linked.margins_for(side).bbox()
        }
    }

    pub fn any_active(self) -> bool {
        self.single_bbox().is_some()
            || self.spread_bbox(ViewTrimSpreadSide::Left).is_some()
            || self.spread_bbox(ViewTrimSpreadSide::Right).is_some()
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ViewTrimPageOverride {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub margins: ViewTrimMargins,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spread_side: Option<ViewTrimSpreadSide>,
}

impl ViewTrimPageOverride {
    pub fn from_margins(margins: ViewTrimMargins) -> Self {
        Self {
            enabled: true,
            margins: margins.clamped(),
            spread_side: None,
        }
    }

    pub fn from_spread_margins(margins: ViewTrimMargins, side: ViewTrimSpreadSide) -> Self {
        Self {
            enabled: true,
            margins: margins.clamped(),
            spread_side: Some(side),
        }
    }

    pub fn bbox(self) -> Option<egui::Rect> {
        self.enabled.then_some(())?;
        self.margins.bbox()
    }

    pub fn margins_for_spread_side(self, side: ViewTrimSpreadSide) -> ViewTrimMargins {
        let margins = self.margins.clamped();
        match self.spread_side {
            Some(authored_side) if authored_side != side => ViewTrimMargins {
                left: margins.right,
                top: margins.top,
                right: margins.left,
                bottom: margins.bottom,
            }
            .clamped(),
            _ => margins,
        }
    }

    pub fn for_spread_side(self, side: ViewTrimSpreadSide) -> Self {
        let side_semantic = self.spread_side.is_some();
        Self {
            enabled: self.enabled,
            margins: self.margins_for_spread_side(side),
            spread_side: side_semantic.then_some(side),
        }
    }

    pub fn spread_bbox(self, side: ViewTrimSpreadSide) -> Option<egui::Rect> {
        self.enabled.then_some(())?;
        self.margins_for_spread_side(side).bbox()
    }
}

pub fn harmonize_spread_auto_bboxes(
    left: Option<egui::Rect>,
    right: Option<egui::Rect>,
) -> (Option<egui::Rect>, Option<egui::Rect>) {
    if left.is_none() && right.is_none() {
        return (None, None);
    }

    let mut left_margins = left.map(ViewTrimMargins::from_bbox).unwrap_or_default();
    let mut right_margins = right.map(ViewTrimMargins::from_bbox).unwrap_or_default();
    let top = left_margins.top.min(right_margins.top);
    let bottom = left_margins.bottom.min(right_margins.bottom);
    left_margins.top = top;
    left_margins.bottom = bottom;
    right_margins.top = top;
    right_margins.bottom = bottom;

    (left_margins.bbox(), right_margins.bbox())
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ViewTrimBookState {
    #[serde(default)]
    pub apply_mode: ViewTrimApplyMode,
    #[serde(default)]
    pub book_settings: ViewTrimBookSettings,
}

impl ViewTrimBookState {
    pub fn is_removable(self) -> bool {
        matches!(self.apply_mode, ViewTrimApplyMode::None)
            && self.book_settings == ViewTrimBookSettings::default()
    }
}

/// 保存値を本単位の基底モードへ正規化する。
/// `Page` は enabled なページ個別行からだけ導出され、本単位では保持しない。
pub fn view_trim_base_apply_mode(stored: ViewTrimApplyMode) -> ViewTrimApplyMode {
    match stored {
        ViewTrimApplyMode::Page => ViewTrimApplyMode::None,
        mode => mode,
    }
}

/// 本体表示と remote 表示が共有する旧 margin-fit 互換の基底モード解決。
pub fn effective_view_trim_base_apply_mode(
    stored: ViewTrimApplyMode,
    legacy_margin_fit_active: bool,
) -> ViewTrimApplyMode {
    let mode = view_trim_base_apply_mode(stored);
    if matches!(mode, ViewTrimApplyMode::None) && legacy_margin_fit_active {
        ViewTrimApplyMode::Auto
    } else {
        mode
    }
}

/// 基底が `Book` のときだけ、enabled なページ個別行を `Page` へ昇格させる。
pub fn effective_view_trim_apply_mode(
    base_mode: ViewTrimApplyMode,
    page_override: Option<ViewTrimPageOverride>,
) -> ViewTrimApplyMode {
    match base_mode {
        ViewTrimApplyMode::Book if page_override.is_some_and(|value| value.enabled) => {
            ViewTrimApplyMode::Page
        }
        mode => mode,
    }
}

/// `None` / `Book` / `Page` の保存値から表示 bbox を解決する。
/// `Auto` は画素走査を必要とするため、呼び出し側が別途解決する。
pub fn stored_view_trim_bbox(
    mode: ViewTrimApplyMode,
    book_settings: ViewTrimBookSettings,
    page_override: Option<ViewTrimPageOverride>,
    spread_side: Option<ViewTrimSpreadSide>,
) -> Option<egui::Rect> {
    match mode {
        ViewTrimApplyMode::Book => match spread_side {
            Some(side) => book_settings.spread_bbox(side),
            None => book_settings.single_bbox(),
        },
        ViewTrimApplyMode::Page => page_override.and_then(|value| match spread_side {
            Some(side) => value.spread_bbox(side),
            None => value.bbox(),
        }),
        ViewTrimApplyMode::None | ViewTrimApplyMode::Auto => None,
    }
}

fn clamp_margin(v: f32) -> f32 {
    if v.is_finite() {
        v.clamp(0.0, MAX_VIEW_TRIM_MARGIN)
    } else {
        0.0
    }
}

fn clamp_pair(start: &mut f32, end: &mut f32) {
    let total = *start + *end;
    let max_total = 1.0 - MIN_CONTENT_FRAC;
    if total > max_total {
        let scale = max_total / total;
        *start *= scale;
        *end *= scale;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn linked_spread_maps_inner_and_outer_to_screen_sides() {
        let linked = ViewTrimLinkedMargins {
            top: 0.03,
            bottom: 0.04,
            inner: 0.08,
            outer: 0.02,
        };

        let left = linked.margins_for(ViewTrimSpreadSide::Left);
        assert_eq!(left.left, 0.02);
        assert_eq!(left.right, 0.08);
        assert_eq!(left.top, 0.03);
        assert_eq!(left.bottom, 0.04);

        let right = linked.margins_for(ViewTrimSpreadSide::Right);
        assert_eq!(right.left, 0.08);
        assert_eq!(right.right, 0.02);
        assert_eq!(right.top, 0.03);
        assert_eq!(right.bottom, 0.04);
    }

    #[test]
    fn margins_are_clamped_to_twenty_percent() {
        let margins = ViewTrimMargins {
            left: 0.5,
            top: f32::NAN,
            right: 0.21,
            bottom: -0.4,
        }
        .clamped();

        assert_eq!(margins.left, MAX_VIEW_TRIM_MARGIN);
        assert_eq!(margins.top, 0.0);
        assert_eq!(margins.right, MAX_VIEW_TRIM_MARGIN);
        assert_eq!(margins.bottom, 0.0);
    }

    #[test]
    fn bbox_round_trip_uses_trim_margins() {
        let bbox = egui::Rect::from_min_max(egui::pos2(0.1, 0.05), egui::pos2(0.9, 0.95));
        let margins = ViewTrimMargins::from_bbox(bbox);
        assert_eq!(margins.left, 0.1);
        assert_eq!(margins.top, 0.05);
        assert!((margins.right - 0.1).abs() < 1e-6);
        assert!((margins.bottom - 0.05).abs() < 1e-6);

        let out = margins.bbox().unwrap();
        assert!((out.min.x - 0.1).abs() < 1e-6);
        assert!((out.max.y - 0.95).abs() < 1e-6);
    }

    #[test]
    fn linked_to_separate_preserves_inner_and_outer_semantics() {
        let linked = ViewTrimLinkedMargins {
            top: 0.01,
            bottom: 0.02,
            inner: 0.03,
            outer: 0.04,
        };
        let (left, right) = linked.to_separate();

        assert_eq!(left.left, 0.04);
        assert_eq!(left.right, 0.03);
        assert_eq!(right.left, 0.03);
        assert_eq!(right.right, 0.04);
        assert_eq!(left.top, 0.01);
        assert_eq!(right.bottom, 0.02);
    }

    #[test]
    fn spread_auto_bboxes_share_less_aggressive_vertical_trim() {
        let left = egui::Rect::from_min_max(egui::pos2(0.02, 0.12), egui::pos2(0.96, 0.93));
        let right = egui::Rect::from_min_max(egui::pos2(0.05, 0.04), egui::pos2(0.98, 0.88));

        let (left_out, right_out) = harmonize_spread_auto_bboxes(Some(left), Some(right));
        let left_m = ViewTrimMargins::from_bbox(left_out.unwrap());
        let right_m = ViewTrimMargins::from_bbox(right_out.unwrap());

        assert!((left_m.left - 0.02).abs() < 1e-6);
        assert!((left_m.right - 0.04).abs() < 1e-6);
        assert!((right_m.left - 0.05).abs() < 1e-6);
        assert!((right_m.right - 0.02).abs() < 1e-6);
        assert!((left_m.top - 0.04).abs() < 1e-6);
        assert!((right_m.top - 0.04).abs() < 1e-6);
        assert!((left_m.bottom - 0.07).abs() < 1e-6);
        assert!((right_m.bottom - 0.07).abs() < 1e-6);
    }

    #[test]
    fn spread_auto_bbox_missing_page_counts_as_no_vertical_trim() {
        let right = egui::Rect::from_min_max(egui::pos2(0.05, 0.04), egui::pos2(0.98, 0.88));

        let (left_out, right_out) = harmonize_spread_auto_bboxes(None, Some(right));
        let right_m = ViewTrimMargins::from_bbox(right_out.unwrap());

        assert!(left_out.is_none());
        assert!((right_m.left - 0.05).abs() < 1e-6);
        assert!((right_m.right - 0.02).abs() < 1e-6);
        assert!(right_m.top <= 1e-6);
        assert!(right_m.bottom <= 1e-6);
    }

    #[test]
    fn page_override_authored_for_spread_side_preserves_inner_outer_when_side_flips() {
        let override_left = ViewTrimPageOverride::from_spread_margins(
            ViewTrimMargins {
                left: 0.03,
                top: 0.01,
                right: 0.08,
                bottom: 0.02,
            },
            ViewTrimSpreadSide::Left,
        );

        let as_left = override_left.margins_for_spread_side(ViewTrimSpreadSide::Left);
        let as_right = override_left.margins_for_spread_side(ViewTrimSpreadSide::Right);

        assert_eq!(as_left.left, 0.03);
        assert_eq!(as_left.right, 0.08);
        assert_eq!(as_right.left, 0.08);
        assert_eq!(as_right.right, 0.03);
        assert_eq!(as_right.top, 0.01);
        assert_eq!(as_right.bottom, 0.02);
    }

    #[test]
    fn page_override_without_spread_side_stays_in_image_coordinates() {
        let override_image = ViewTrimPageOverride::from_margins(ViewTrimMargins {
            left: 0.03,
            top: 0.01,
            right: 0.08,
            bottom: 0.02,
        });

        let as_right = override_image.margins_for_spread_side(ViewTrimSpreadSide::Right);

        assert_eq!(as_right.left, 0.03);
        assert_eq!(as_right.right, 0.08);
        assert_eq!(as_right.top, 0.01);
        assert_eq!(as_right.bottom, 0.02);
    }

    #[test]
    fn book_spread_representation_switch_preserves_inner_outer_semantics() {
        let linked = ViewTrimBookSettings {
            spread_linked: ViewTrimLinkedMargins {
                top: 0.01,
                bottom: 0.02,
                inner: 0.08,
                outer: 0.03,
            },
            ..Default::default()
        };
        let separate = linked.with_spread_separate(true);
        assert!(separate.spread_separate);
        assert_eq!(separate.spread_left.left, 0.03);
        assert_eq!(separate.spread_left.right, 0.08);
        assert_eq!(separate.spread_right.left, 0.08);
        assert_eq!(separate.spread_right.right, 0.03);

        let round_trip = separate.with_spread_separate(false);
        assert!(!round_trip.spread_separate);
        assert_eq!(round_trip.spread_linked, linked.spread_linked);
    }

    #[test]
    fn effective_mode_normalizes_saved_page_and_promotes_legacy_none_to_auto() {
        assert_eq!(
            effective_view_trim_base_apply_mode(ViewTrimApplyMode::Page, false),
            ViewTrimApplyMode::None
        );
        assert_eq!(
            effective_view_trim_base_apply_mode(ViewTrimApplyMode::None, true),
            ViewTrimApplyMode::Auto
        );
        assert_eq!(
            effective_view_trim_base_apply_mode(ViewTrimApplyMode::Book, true),
            ViewTrimApplyMode::Book
        );
    }

    #[test]
    fn effective_book_mode_promotes_only_enabled_page_override() {
        let disabled = ViewTrimPageOverride {
            enabled: false,
            ..Default::default()
        };
        let enabled = ViewTrimPageOverride::from_margins(ViewTrimMargins {
            left: 0.03,
            ..Default::default()
        });

        assert_eq!(
            effective_view_trim_apply_mode(ViewTrimApplyMode::Book, Some(disabled)),
            ViewTrimApplyMode::Book
        );
        assert_eq!(
            effective_view_trim_apply_mode(ViewTrimApplyMode::Book, Some(enabled)),
            ViewTrimApplyMode::Page
        );
        assert_eq!(
            effective_view_trim_apply_mode(ViewTrimApplyMode::Auto, Some(enabled)),
            ViewTrimApplyMode::Auto
        );
    }

    #[test]
    fn stored_page_bbox_converts_authored_spread_side() {
        let page_override = ViewTrimPageOverride::from_spread_margins(
            ViewTrimMargins {
                left: 0.03,
                top: 0.01,
                right: 0.08,
                bottom: 0.02,
            },
            ViewTrimSpreadSide::Left,
        );
        let bbox = stored_view_trim_bbox(
            ViewTrimApplyMode::Page,
            ViewTrimBookSettings::default(),
            Some(page_override),
            Some(ViewTrimSpreadSide::Right),
        )
        .unwrap();
        let margins = ViewTrimMargins::from_bbox(bbox);

        assert!((margins.left - 0.08).abs() < 1e-6);
        assert!((margins.right - 0.03).abs() < 1e-6);
        assert!((margins.top - 0.01).abs() < 1e-6);
        assert!((margins.bottom - 0.02).abs() < 1e-6);
    }

    #[test]
    fn stored_book_bbox_uses_single_or_requested_spread_side_and_none_disables_it() {
        let book = ViewTrimBookSettings {
            enabled: true,
            single: ViewTrimMargins {
                top: 0.04,
                ..Default::default()
            },
            spread_linked: ViewTrimLinkedMargins {
                inner: 0.08,
                outer: 0.02,
                ..Default::default()
            },
            ..Default::default()
        };

        let single = ViewTrimMargins::from_bbox(
            stored_view_trim_bbox(ViewTrimApplyMode::Book, book, None, None).unwrap(),
        );
        let right = ViewTrimMargins::from_bbox(
            stored_view_trim_bbox(
                ViewTrimApplyMode::Book,
                book,
                None,
                Some(ViewTrimSpreadSide::Right),
            )
            .unwrap(),
        );

        assert!((single.top - 0.04).abs() < 1e-6);
        assert!((right.left - 0.08).abs() < 1e-6);
        assert!((right.right - 0.02).abs() < 1e-6);
        assert!(stored_view_trim_bbox(ViewTrimApplyMode::None, book, None, None).is_none());
    }

    #[test]
    fn separate_to_linked_uses_average_inner_and_outer() {
        let left = ViewTrimMargins {
            left: 0.02,
            top: 0.04,
            right: 0.08,
            bottom: 0.10,
        };
        let right = ViewTrimMargins {
            left: 0.12,
            top: 0.06,
            right: 0.16,
            bottom: 0.14,
        };

        let linked = ViewTrimLinkedMargins::average_from_separate(left, right);

        assert!((linked.top - 0.05).abs() < 1e-6);
        assert!((linked.bottom - 0.12).abs() < 1e-6);
        assert!((linked.inner - 0.10).abs() < 1e-6);
        assert!((linked.outer - 0.09).abs() < 1e-6);
    }
}
