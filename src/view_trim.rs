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
}

impl ViewTrimPageOverride {
    pub fn from_margins(margins: ViewTrimMargins) -> Self {
        Self {
            enabled: true,
            margins: margins.clamped(),
        }
    }

    pub fn bbox(self) -> Option<egui::Rect> {
        self.enabled.then_some(())?;
        self.margins.bbox()
    }
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
