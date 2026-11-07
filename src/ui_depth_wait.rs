//! Pure presentation for the pending 3D-photo request.

use std::time::Duration;

use egui::{Color32, FontId, Rect, Vec2};

const PADDING: Vec2 = egui::vec2(14.0, 9.0);
const TEXT_GAP: f32 = 4.0;
const TRACK_GAP: f32 = 7.0;
const TRACK_HEIGHT: f32 = 3.0;

/// Paint a pending-depth indicator at the top center of the unobstructed image region.
///
/// The caller owns request visibility, elapsed time, effective key text, and repaint scheduling.
/// `cancel` is the complete optional second line (for example, `D で取り消し`). No interaction,
/// application state, model, or asynchronous work is created here. A region too small to contain
/// the wrapped indicator is left untouched.
pub(crate) fn paint_depth_wait_indicator(
    ui: &egui::Ui,
    available: Rect,
    text: &str,
    cancel: Option<&str>,
    elapsed: Duration,
) -> Option<Rect> {
    let available = available.intersect(ui.clip_rect());
    let content_width = available.width() - PADDING.x * 2.0;
    if !available.is_finite() || content_width <= 0.0 || text.is_empty() {
        return None;
    }

    let painter = ui.painter().with_clip_rect(available);
    let layout = |text: &str, font: FontId, color| {
        let mut job = egui::text::LayoutJob::simple(text.to_owned(), font, color, content_width);
        job.wrap.break_anywhere = true;
        painter.layout_job(job)
    };
    let heading = layout(text, FontId::proportional(17.0), Color32::WHITE);
    let weak_color =
        crate::os_theme::dark_visuals(crate::os_theme::current_text_contrast(ui.ctx()))
            .weak_text_color();
    let cancel = cancel
        .filter(|text| !text.is_empty())
        .map(|text| layout(text, egui::TextStyle::Small.resolve(ui.style()), weak_color));
    let text_width = heading
        .size()
        .x
        .max(cancel.as_ref().map_or(0.0, |galley| galley.size().x));
    let text_height = heading.size().y
        + cancel
            .as_ref()
            .map_or(0.0, |galley| TEXT_GAP + galley.size().y);
    let size = egui::vec2(
        text_width + PADDING.x * 2.0,
        text_height + PADDING.y * 2.0 + TRACK_GAP + TRACK_HEIGHT,
    );
    let rect = indicator_rect(available, size)?;
    painter.rect_filled(rect, 8.0, Color32::from_rgba_unmultiplied(30, 30, 30, 225));
    let text_x = rect.min.x + PADDING.x;
    let mut text_y = rect.min.y + PADDING.y;
    let heading_height = heading.size().y;
    painter.galley(egui::pos2(text_x, text_y), heading, Color32::WHITE);
    if let Some(cancel) = cancel {
        text_y += heading_height + TEXT_GAP;
        painter.galley(egui::pos2(text_x, text_y), cancel, weak_color);
    }

    let track = Rect::from_min_size(
        egui::pos2(text_x, rect.max.y - PADDING.y - TRACK_HEIGHT),
        egui::vec2(text_width, TRACK_HEIGHT),
    );
    painter.rect_filled(
        track,
        TRACK_HEIGHT * 0.5,
        Color32::from_rgba_unmultiplied(255, 255, 255, 45),
    );
    painter.rect_filled(
        progress_segment(track, elapsed),
        TRACK_HEIGHT * 0.5,
        Color32::from_rgb(90, 170, 255),
    );
    Some(rect)
}

fn indicator_rect(available: Rect, size: Vec2) -> Option<Rect> {
    if !available.is_finite()
        || !size.is_finite()
        || size.x <= 0.0
        || size.y <= 0.0
        || size.x > available.width()
        || size.y > available.height()
    {
        return None;
    }
    Some(Rect::from_min_size(
        egui::pos2(available.center().x - size.x * 0.5, available.min.y),
        size,
    ))
}

fn progress_segment(track: Rect, elapsed: Duration) -> Rect {
    let width = (track.width() * 0.28).max(20.0).min(track.width());
    let travel = (track.width() - width).max(0.0);
    let phase = (elapsed.as_secs_f32() * 0.8) % 1.0;
    Rect::from_min_size(
        egui::pos2(track.min.x + travel * phase, track.min.y),
        egui::vec2(width, track.height()),
    )
}

/// Visual-only fixture: a generated landscape and illustrative viewer chrome, with no App/model.
#[doc(hidden)]
pub fn draw_depth_wait_indicator_snapshot_fixture(ui: &mut egui::Ui) {
    let full = ui.max_rect();
    let painter = ui.painter();
    painter.rect_filled(full, 0.0, Color32::BLACK);
    let toolbar = Rect::from_min_size(full.min, egui::vec2(full.width(), 38.0));
    let page_info = Rect::from_min_size(
        egui::pos2(full.min.x, toolbar.max.y),
        egui::vec2(full.width(), 28.0),
    );
    let panel = Rect::from_min_max(
        egui::pos2(full.max.x - 254.0, page_info.max.y + 12.0),
        egui::pos2(full.max.x - 12.0, full.max.y - 18.0),
    );
    let image = Rect::from_min_max(
        egui::pos2(full.min.x + 30.0, page_info.max.y + 12.0),
        egui::pos2(panel.min.x - 24.0, full.max.y - 20.0),
    );
    painter.rect_filled(image, 0.0, Color32::from_rgb(112, 151, 175));
    for (height, color) in [
        (0.34, Color32::from_rgb(95, 122, 133)),
        (0.53, Color32::from_rgb(63, 95, 100)),
        (0.73, Color32::from_rgb(35, 65, 66)),
    ] {
        painter.add(egui::Shape::convex_polygon(
            vec![
                egui::pos2(image.min.x, image.max.y),
                egui::pos2(image.min.x, image.min.y + image.height() * (height + 0.12)),
                egui::pos2(
                    image.min.x + image.width() * 0.32,
                    image.min.y + image.height() * height,
                ),
                egui::pos2(image.max.x, image.min.y + image.height() * (height + 0.03)),
                image.max,
            ],
            color,
            egui::Stroke::NONE,
        ));
    }
    painter.rect_filled(toolbar, 0.0, Color32::from_gray(26));
    painter.text(
        toolbar.left_center() + egui::vec2(14.0, 0.0),
        egui::Align2::LEFT_CENTER,
        "画像表示    3D 写真",
        FontId::proportional(14.0),
        Color32::WHITE,
    );
    painter.text(
        page_info.center(),
        egui::Align2::CENTER_CENTER,
        "landscape.jpg   1 / 12",
        FontId::proportional(12.0),
        Color32::from_gray(180),
    );
    painter.rect_filled(panel, 8.0, Color32::from_gray(28));
    for (index, label) in [
        "3D 写真",
        "動き    マウス追従",
        "強さ    2.0%",
        "周期    3.0 秒",
        "奥行きを計算中…",
    ]
    .iter()
    .enumerate()
    {
        painter.text(
            panel.min + egui::vec2(14.0, 16.0 + index as f32 * 36.0),
            egui::Align2::LEFT_TOP,
            label,
            FontId::proportional(if index == 0 { 17.0 } else { 13.0 }),
            Color32::from_gray(220),
        );
    }
    let available = Rect::from_min_max(
        egui::pos2(full.min.x + 12.0, page_info.max.y + 12.0),
        egui::pos2(panel.min.x - 12.0, full.max.y - 12.0),
    );
    let rect = paint_depth_wait_indicator(
        ui,
        available,
        "奥行きを計算中…",
        Some("D で取り消し"),
        Duration::from_millis(625),
    )
    .expect("snapshot fixture has room for the depth wait indicator");
    assert!(available.contains_rect(rect));
    assert!(!rect.intersects(toolbar));
    assert!(!rect.intersects(page_info));
    assert!(!rect.intersects(panel));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn depth_wait_geometry_stays_in_safe_region() {
        let available = Rect::from_min_max(egui::pos2(12.0, 78.0), egui::pos2(815.0, 594.0));
        let rect = indicator_rect(available, egui::vec2(250.0, 80.0)).unwrap();
        assert!(available.contains_rect(rect));
        assert_eq!(rect.center().x, available.center().x);
        assert_eq!(rect.min.y, available.min.y);
        assert!(indicator_rect(available, egui::vec2(804.0, 80.0)).is_none());
        assert!(indicator_rect(available, egui::vec2(250.0, 517.0)).is_none());
    }

    #[test]
    fn depth_wait_progress_segment_matches_animation_progress() {
        for width in [10.0, 40.0, 250.0] {
            let track =
                Rect::from_min_size(egui::pos2(20.0, 100.0), egui::vec2(width, TRACK_HEIGHT));
            let start = progress_segment(track, Duration::ZERO);
            let middle = progress_segment(track, Duration::from_millis(625));
            let repeated = progress_segment(track, Duration::from_millis(1250));
            assert!(track.contains_rect(start));
            assert!(track.contains_rect(middle));
            assert_eq!(start, repeated);
            assert_eq!(middle.center().x, track.center().x);
            assert_eq!(start.width(), (width * 0.28).max(20.0).min(width));
        }
    }

    #[test]
    fn depth_wait_painter_wraps_and_omits_unbound_cancel_line() {
        let ctx = egui::Context::default();
        ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1000.0, 614.0),
                )),
                ..Default::default()
            },
            |root_ui| {
                egui::CentralPanel::default().show(root_ui, |ui| {
                    for width in [100.0, 220.0, 760.0] {
                        let available =
                            Rect::from_min_size(ui.max_rect().min, egui::vec2(width, 500.0));
                        let without_cancel = paint_depth_wait_indicator(
                            ui,
                            available,
                            "Preparing the depth model for this image…",
                            None,
                            Duration::ZERO,
                        )
                        .unwrap();
                        let with_cancel = paint_depth_wait_indicator(
                            ui,
                            available,
                            "Preparing the depth model for this image…",
                            Some("Ctrl+Shift+D to cancel"),
                            Duration::ZERO,
                        )
                        .unwrap();
                        assert!(available.contains_rect(without_cancel));
                        assert!(available.contains_rect(with_cancel));
                        assert!(with_cancel.height() > without_cancel.height());
                        assert!(width > 100.0 || without_cancel.height() > 80.0);
                    }
                });
            },
        )
        .drop_without_applying_deltas();
    }
}
