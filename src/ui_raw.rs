//! Shared RAW preferences and loading presentation, also used by UI snapshots.
use crate::raw::RawBrightness;

pub(crate) const RAW_BLOCKED_PREVIEW_NOTICE: &str =
    "この RAW 形式は現像に対応していません（埋め込みプレビューを表示中）";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RawLoadingStatus {
    Queued,
    Reading,
    Developing(u8),
    Unsupported,
    Failed,
    SourceFailed,
}

impl RawLoadingStatus {
    pub(crate) fn label(self) -> String {
        match self {
            Self::Queued => "RAW 現像待ち".into(),
            Self::Reading => "RAW 読み込み中".into(),
            Self::Developing(percent) => format!("RAW 現像中 {percent}%"),
            Self::Unsupported => "この RAW 形式は現像に対応していません".into(),
            Self::Failed => "RAW の現像に失敗しました".into(),
            Self::SourceFailed => "RAW を読み込めませんでした".into(),
        }
    }
}

pub(crate) fn draw_settings(ui: &mut egui::Ui, settings: &mut crate::settings::Settings) {
    ui.label(egui::RichText::new("RAW の現像").strong());
    ui.horizontal(|ui| {
        ui.label("同時現像数:");
        ui.add(
            egui::DragValue::new(&mut settings.raw_develop_parallelism)
                .range(1..=10)
                .suffix(" 枚"),
        );
    });
    ui.horizontal(|ui| {
        ui.label("明るさ:");
        ui.selectable_value(
            &mut settings.raw_brightness,
            RawBrightness::MatchPreview,
            "プレビューに合わせる",
        );
        ui.selectable_value(
            &mut settings.raw_brightness,
            RawBrightness::None,
            "補正しない",
        );
    });
    ui.label(
        egui::RichText::new("現像中の画像が多いほどメモリを使います。変更はすぐに反映されます。")
            .weak(),
    );
}

pub(crate) fn paint_loading_label(painter: &egui::Painter, rect: egui::Rect, label: &str) {
    painter.text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        label,
        egui::FontId::proportional(18.0),
        egui::Color32::from_gray(150),
    );
}

pub(crate) fn paint_processing_status_overlay(
    painter: &egui::Painter,
    image_rect: egui::Rect,
    label: &str,
) -> egui::Rect {
    let pos = egui::pos2(image_rect.min.x + 12.0, image_rect.max.y - 12.0);
    let galley = painter.layout_no_wrap(
        label.into(),
        egui::FontId::proportional(13.0),
        egui::Color32::WHITE,
    );
    let text_rect = egui::Align2::LEFT_BOTTOM.anchor_size(pos, galley.size());
    let bg = text_rect.expand(4.0);
    painter.rect_filled(bg, 4.0, egui::Color32::from_rgba_unmultiplied(0, 0, 0, 200));
    painter.galley(text_rect.min, galley, egui::Color32::WHITE);
    bg
}

#[doc(hidden)]
pub fn draw_raw_blocked_preview_snapshot_fixture(ui: &mut egui::Ui) {
    paint_processing_status_overlay(ui.painter(), ui.max_rect(), RAW_BLOCKED_PREVIEW_NOTICE);
}

#[doc(hidden)]
pub fn draw_raw_settings_snapshot_fixture(ui: &mut egui::Ui) {
    draw_settings(ui, &mut crate::settings::Settings::default());
}

#[doc(hidden)]
pub fn draw_raw_progress_snapshot_fixture(ui: &mut egui::Ui) {
    for status in [
        RawLoadingStatus::Queued,
        RawLoadingStatus::Reading,
        RawLoadingStatus::Developing(42),
        RawLoadingStatus::Unsupported,
        RawLoadingStatus::Failed,
    ] {
        let (_, rect) = ui.allocate_space(egui::vec2(ui.available_width(), 46.0));
        paint_loading_label(ui.painter(), rect, &status.label());
    }
}
