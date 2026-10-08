//! Shared spelling and dimensions for the still-image and native video HUD menus.

use eframe::egui;

pub(crate) const SEEK_STRIP_MENU_ROW_HEIGHT: f32 = 26.0;
pub(crate) const SEEK_STRIP_MENU_TEXT_LEFT: f32 = 14.0;
pub(crate) const SEEK_STRIP_MENU_VERTICAL_PADDING: f32 = 6.0;
pub(crate) const SEEK_STRIP_MENU_SEPARATOR_HEIGHT: f32 = 7.0;

#[derive(Clone, Copy)]
pub(crate) struct SeekStripMenuLabels {
    pub compact: bool,
    pub tiny: bool,
}

impl SeekStripMenuLabels {
    pub fn for_viewport(viewport: egui::Rect) -> Self {
        Self {
            compact: viewport.width() < 400.0 || viewport.height() < 340.0,
            tiny: viewport.width() < 240.0,
        }
    }

    pub fn preset(self, label: &str, points: f32) -> String {
        if self.tiny {
            label.to_owned()
        } else if self.compact {
            format!("{label} {points:.0}")
        } else {
            format!("{label} ({points:.0} px)")
        }
    }

    pub fn headings(self) -> [&'static str; 2] {
        if self.tiny {
            ["高さ", "プレビュー"]
        } else if self.compact {
            ["列の高さ (px)", "プレビュー (px)"]
        } else {
            ["列の高さ", "シーク位置プレビューの大きさ"]
        }
    }

    pub fn font(self) -> egui::FontId {
        egui::FontId::proportional(if self.tiny { 10.0 } else { 13.0 })
    }
}

pub(crate) fn seek_strip_menu_row_rect(
    menu_rect: egui::Rect,
    row_index: usize,
    separator_before: usize,
    row_height: f32,
) -> egui::Rect {
    let separator = if row_index >= separator_before {
        SEEK_STRIP_MENU_SEPARATOR_HEIGHT
    } else {
        0.0
    };
    egui::Rect::from_min_size(
        egui::pos2(
            menu_rect.min.x + 4.0,
            menu_rect.min.y
                + SEEK_STRIP_MENU_VERTICAL_PADDING
                + separator
                + row_index as f32 * row_height,
        ),
        egui::vec2(menu_rect.width() - 8.0, row_height - 2.0),
    )
}
