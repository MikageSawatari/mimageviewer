//! `show_thumb_quality_dialog_window` ダイアログの実装。
//!
//! `App` への impl 拡張として書かれており、フィールドアクセスは
//! `pub(crate)` 経由で行われる。`update()` から `self.tq.show_window(ctx)` で呼ばれる。

#![allow(unused_imports)]

use std::path::PathBuf;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering},
    mpsc,
};

use eframe::egui;

use crate::app::{App, tq_draw_preview};
use crate::catalog;
use crate::folder_tree;
use crate::gpu_info;
use crate::grid_item::{GridItem, ThumbnailState};
use crate::settings;
use crate::stats;
use crate::thumb_loader::{
    CacheDecision, LoadRequest, ThumbMsg, build_and_save_one, compute_display_px,
};
use crate::ui_helpers::{
    draw_format_rows, draw_histogram, format_bytes, format_bytes_small, format_count,
    natural_sort_key, truncate_name,
};

const THUMB_QUALITY_DIALOG_ID: &str = "サムネイル画質設定";
const THUMB_QUALITY_DIALOG_MARGIN: egui::Vec2 = egui::vec2(16.0, 24.0);
const THUMB_QUALITY_COMPACT_WIDTH: f32 = 600.0;
#[cfg(test)]
const THUMB_QUALITY_DIALOG_OBSERVATION_ID: &str = "thumb_quality_dialog_observation";

#[derive(Clone, Copy, Debug)]
struct ThumbQualityDialogLayout {
    safe_rect: egui::Rect,
    default_size: egui::Vec2,
    min_size: egui::Vec2,
    preview_reference: egui::Vec2,
}

fn thumb_quality_dialog_layout(
    content_rect: egui::Rect,
    thumbnail_grid_active: bool,
    last_cell_size: f32,
    last_cell_h: f32,
    aspect_height_ratio: f32,
) -> ThumbQualityDialogLayout {
    // Keep the title bar and resize corner inside the usable viewport even when the host is
    // narrower than the normal dialog margins. `Rect::shrink2` can invert a very small rect, so
    // cap each inset by one quarter of the corresponding extent.
    let margin = egui::vec2(
        THUMB_QUALITY_DIALOG_MARGIN
            .x
            .min(content_rect.width().max(0.0) * 0.25),
        THUMB_QUALITY_DIALOG_MARGIN
            .y
            .min(content_rect.height().max(0.0) * 0.25),
    );
    let safe_rect = content_rect.shrink2(margin);
    let safe_size = safe_rect.size().max(egui::vec2(1.0, 1.0));

    // Details rows store the whole details pane width and a 28 pt row height in `last_cell_*`.
    // Those values are useful to details scrolling, but they are not a thumbnail cell. Use a
    // bounded thumbnail-shaped reference until a real thumbnail grid is active.
    let preview_reference = if thumbnail_grid_active {
        egui::vec2(last_cell_size.max(200.0), last_cell_h.max(150.0))
    } else {
        let width = 240.0;
        let height = (width * aspect_height_ratio.max(0.1)).clamp(150.0, 320.0);
        egui::vec2(width, height)
    };
    let desired = egui::vec2(
        (preview_reference.x * 2.0 + 80.0).clamp(680.0, 1800.0),
        (preview_reference.y + 260.0).clamp(480.0, 1200.0),
    );

    ThumbQualityDialogLayout {
        safe_rect,
        default_size: desired.min(safe_size),
        min_size: egui::vec2(420.0, 320.0).min(safe_size),
        preview_reference,
    }
}

fn fit_thumb_quality_preview(reference: egui::Vec2, available_width: f32) -> egui::Vec2 {
    let max_size = egui::vec2(available_width.max(1.0), 360.0);
    let scale = (max_size.x / reference.x.max(1.0))
        .min(max_size.y / reference.y.max(1.0))
        .min(1.0);
    (reference * scale).max(egui::vec2(1.0, 1.0))
}

#[derive(Clone, Copy, Debug, Default)]
struct ThumbQualityPanelAction {
    reencode: bool,
    apply: bool,
    open_fullscreen: bool,
    preview_rect: Option<egui::Rect>,
    apply_rect: Option<egui::Rect>,
    apply_hovered: bool,
    apply_pointer_down: bool,
}

fn draw_thumb_quality_panel(
    ui: &mut egui::Ui,
    heading: &str,
    texture: &Option<egui::TextureHandle>,
    size: &mut u32,
    quality: &mut u8,
    bytes: usize,
    preview_reference: egui::Vec2,
) -> ThumbQualityPanelAction {
    let mut action = ThumbQualityPanelAction::default();
    ui.vertical(|ui| {
        ui.heading(heading);
        ui.add_space(4.0);
        let preview_size = fit_thumb_quality_preview(preview_reference, ui.available_width());
        let response = tq_draw_preview(ui, texture, preview_size.x, preview_size.y);
        action.open_fullscreen = response.clicked();
        action.preview_rect = Some(response.rect);
        ui.add_space(6.0);

        ui.horizontal(|ui| {
            ui.label("サイズ:");
            let response = ui.add(egui::Slider::new(size, 128..=1536).text("px"));
            action.reencode |= response.drag_stopped() || response.lost_focus();
        });
        ui.horizontal(|ui| {
            ui.label("品質:");
            let response = ui.add(egui::Slider::new(quality, 1..=100));
            action.reencode |= response.drag_stopped() || response.lost_focus();
        });
        ui.add_space(4.0);
        ui.label(format!(
            "{}  ({}x{})",
            format_bytes_small(bytes as u64),
            texture.as_ref().map(|t| t.size()[0]).unwrap_or(0),
            texture.as_ref().map(|t| t.size()[1]).unwrap_or(0),
        ));
        ui.add_space(4.0);
        let response = ui.button(format!("  {heading} を適用  "));
        action.apply = response.clicked();
        action.apply_rect = Some(response.rect);
        action.apply_hovered = response.hovered();
        action.apply_pointer_down = response.is_pointer_button_down_on();
    });
    action
}

#[cfg(test)]
fn thumb_quality_dialog_observation(ctx: &egui::Context) -> ThumbQualityPanelAction {
    ctx.data(|data| {
        data.get_temp(egui::Id::new(THUMB_QUALITY_DIALOG_OBSERVATION_ID))
            .unwrap_or_default()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::setup_app_for_test;
    use crate::settings::GridViewMode;

    fn raw_input(size: egui::Vec2, events: Vec<egui::Event>) -> egui::RawInput {
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
            events,
            ..Default::default()
        }
    }

    fn pointer_button(pos: egui::Pos2, pressed: bool) -> egui::Event {
        egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        }
    }

    fn prepare_dialog(app: &mut App, mode: GridViewMode) {
        app.settings.grid_view_mode = mode;
        app.last_cell_size = 1_220.0;
        app.last_cell_h = 28.0;
        app.tq.show = true;
        app.tq.sample = Some(Arc::new(image::DynamicImage::new_rgba8(16, 9)));
        app.tq.sample_display_name = Some("isolated-test.png".to_owned());
        app.tq.sample_original_size = 144;
    }

    fn run_dialog_frame(
        app: &mut App,
        ctx: &egui::Context,
        size: egui::Vec2,
        events: Vec<egui::Event>,
    ) -> egui::Rect {
        let _ = ctx.run(raw_input(size, events), |ctx| {
            app.show_thumb_quality_dialog_window(ctx);
        });
        ctx.memory(|memory| {
            memory
                .area_rect(egui::Id::new(THUMB_QUALITY_DIALOG_ID))
                .expect("thumb quality dialog must be rendered")
        })
    }

    fn run_panel_frame(
        ctx: &egui::Context,
        size: egui::Vec2,
        events: Vec<egui::Event>,
    ) -> ThumbQualityPanelAction {
        let mut action = ThumbQualityPanelAction::default();
        let _ = ctx.run(raw_input(size, events), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let mut px = 512;
                let mut quality = 80;
                action = draw_thumb_quality_panel(
                    ui,
                    "A",
                    &None,
                    &mut px,
                    &mut quality,
                    0,
                    egui::vec2(240.0, 180.0),
                );
            });
        });
        action
    }

    fn assert_inside(rect: egui::Rect, outer: egui::Rect) {
        let tolerance = 1.0;
        assert!(
            rect.min.x >= outer.min.x - tolerance
                && rect.min.y >= outer.min.y - tolerance
                && rect.max.x <= outer.max.x + tolerance
                && rect.max.y <= outer.max.y + tolerance,
            "dialog {rect:?} must remain inside {outer:?}"
        );
    }

    #[test]
    fn details_layout_does_not_treat_the_details_row_as_a_thumbnail_cell() {
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1_280.0, 900.0));
        let details = thumb_quality_dialog_layout(screen, false, 1_220.0, 28.0, 0.75);
        let thumbnail = thumb_quality_dialog_layout(screen, true, 1_220.0, 28.0, 0.75);

        assert_eq!(details.preview_reference, egui::vec2(240.0, 180.0));
        assert_eq!(details.default_size, egui::vec2(680.0, 480.0));
        assert!(thumbnail.preview_reference.x > details.preview_reference.x);
        assert!(thumbnail.default_size.x > details.default_size.x);
    }

    #[test]
    fn layout_and_preview_fit_stay_inside_narrow_content_rects() {
        for size in [egui::vec2(1_280.0, 900.0), egui::vec2(480.0, 420.0)] {
            for thumbnail_grid_active in [false, true] {
                let content = egui::Rect::from_min_size(egui::Pos2::ZERO, size);
                let layout = thumb_quality_dialog_layout(
                    content,
                    thumbnail_grid_active,
                    1_220.0,
                    28.0,
                    0.75,
                );
                assert!(layout.safe_rect.contains_rect(egui::Rect::from_center_size(
                    layout.safe_rect.center(),
                    layout.default_size,
                )));
                assert!(layout.default_size.x <= layout.safe_rect.width());
                assert!(layout.default_size.y <= layout.safe_rect.height());
                assert!(layout.min_size.x <= layout.default_size.x);
                assert!(layout.min_size.y <= layout.default_size.y);

                let fitted = fit_thumb_quality_preview(
                    layout.preview_reference,
                    layout.safe_rect.width() * 0.5,
                );
                assert!(fitted.x <= layout.safe_rect.width() * 0.5 + f32::EPSILON);
                assert!(fitted.y <= 360.0 + f32::EPSILON);
            }
        }
    }

    #[test]
    fn real_dialog_is_constrained_for_details_thumbnail_and_host_resize() {
        let mut app = setup_app_for_test();
        let large = egui::vec2(1_280.0, 900.0);
        let narrow = egui::vec2(480.0, 420.0);

        for mode in [GridViewMode::Details, GridViewMode::Thumbnail] {
            let ctx = egui::Context::default();
            crate::ui_fonts::configure_fonts_with_settings(&ctx, &app.settings.ui_font);
            prepare_dialog(&mut app, mode);
            for _ in 0..3 {
                run_dialog_frame(&mut app, &ctx, large, Vec::new());
            }
            let rect = run_dialog_frame(&mut app, &ctx, narrow, Vec::new());
            let viewport = egui::Rect::from_min_size(egui::Pos2::ZERO, narrow);
            assert_inside(rect, viewport);
            assert!(app.tq.show);
            app.close_thumb_quality_dialog();
        }
    }

    #[test]
    fn real_dialog_opens_inside_a_fresh_narrow_viewport() {
        let mut app = setup_app_for_test();
        let ctx = egui::Context::default();
        crate::ui_fonts::configure_fonts_with_settings(&ctx, &app.settings.ui_font);
        let narrow = egui::vec2(480.0, 420.0);
        prepare_dialog(&mut app, GridViewMode::Details);

        let mut rect = egui::Rect::NOTHING;
        for _ in 0..4 {
            rect = run_dialog_frame(&mut app, &ctx, narrow, Vec::new());
        }
        let viewport = egui::Rect::from_min_size(egui::Pos2::ZERO, narrow);
        assert_inside(rect, viewport);
    }

    #[test]
    fn real_dialog_size_settles_without_growing_for_wide_and_narrow_layouts() {
        for (mode, viewport, cell_size, cell_height) in [
            (
                GridViewMode::Thumbnail,
                egui::vec2(1_280.0, 900.0),
                320.0,
                240.0,
            ),
            (
                GridViewMode::Details,
                egui::vec2(1_280.0, 900.0),
                1_220.0,
                28.0,
            ),
            (
                GridViewMode::Thumbnail,
                egui::vec2(480.0, 420.0),
                240.0,
                180.0,
            ),
            (
                GridViewMode::Details,
                egui::vec2(480.0, 420.0),
                1_220.0,
                28.0,
            ),
        ] {
            let mut app = setup_app_for_test();
            let ctx = egui::Context::default();
            crate::ui_fonts::configure_fonts_with_settings(&ctx, &app.settings.ui_font);
            prepare_dialog(&mut app, mode);
            app.last_cell_size = cell_size;
            app.last_cell_h = cell_height;
            let expected_layout = thumb_quality_dialog_layout(
                egui::Rect::from_min_size(egui::Pos2::ZERO, viewport),
                mode == GridViewMode::Thumbnail,
                cell_size,
                cell_height,
                app.effective_thumb_aspect().height_ratio(),
            );

            let mut frame_heights = Vec::new();
            let mut settled_heights = Vec::new();
            for frame in 0..28 {
                let rect = run_dialog_frame(&mut app, &ctx, viewport, Vec::new());
                frame_heights.push(rect.height());
                if frame >= 6 {
                    settled_heights.push(rect.height());
                }
            }
            let min = settled_heights
                .iter()
                .copied()
                .fold(f32::INFINITY, f32::min);
            let max = settled_heights
                .iter()
                .copied()
                .fold(f32::NEG_INFINITY, f32::max);
            assert!(
                max - min <= 1.0,
                "{mode:?} at {viewport:?} kept growing after settlement: {settled_heights:?}"
            );
            if viewport.x >= 1_000.0 {
                let final_height = *frame_heights.last().unwrap();
                assert!(
                    final_height <= expected_layout.default_size.y + 64.0,
                    "{mode:?} grew from its calculated default toward the viewport maximum: \
                     expected content height {:?}, frames {frame_heights:?}",
                    expected_layout.default_size.y,
                );
            }
        }
    }

    #[test]
    fn manual_window_resize_is_retained_after_twenty_frames() {
        let mut app = setup_app_for_test();
        let ctx = egui::Context::default();
        crate::ui_fonts::configure_fonts_with_settings(&ctx, &app.settings.ui_font);
        let viewport = egui::vec2(1_280.0, 900.0);
        prepare_dialog(&mut app, GridViewMode::Details);

        let mut before = egui::Rect::NOTHING;
        for _ in 0..6 {
            before = run_dialog_frame(&mut app, &ctx, viewport, Vec::new());
        }
        let corner = before.max - egui::vec2(2.0, 2.0);
        run_dialog_frame(
            &mut app,
            &ctx,
            viewport,
            vec![
                egui::Event::PointerMoved(corner),
                pointer_button(corner, true),
            ],
        );
        let resized_corner = corner + egui::vec2(80.0, 60.0);
        run_dialog_frame(
            &mut app,
            &ctx,
            viewport,
            vec![egui::Event::PointerMoved(resized_corner)],
        );
        let resized = run_dialog_frame(
            &mut app,
            &ctx,
            viewport,
            vec![pointer_button(resized_corner, false)],
        );
        assert!(
            resized.width() > before.width() + 40.0 && resized.height() > before.height() + 30.0,
            "resize interaction did not enlarge the window: before={before:?}, resized={resized:?}"
        );

        let mut final_rect = resized;
        for _ in 0..22 {
            final_rect = run_dialog_frame(&mut app, &ctx, viewport, Vec::new());
        }
        assert!(
            (final_rect.size() - resized.size()).abs().max_elem() <= 1.0,
            "manual size must survive later auto-size passes: resized={resized:?}, final={final_rect:?}"
        );
    }

    #[test]
    fn narrow_real_window_scroll_reaches_the_b_apply_response_without_saving() {
        let mut app = setup_app_for_test();
        let ctx = egui::Context::default();
        crate::ui_fonts::configure_fonts_with_settings(&ctx, &app.settings.ui_font);
        let size = egui::vec2(480.0, 420.0);
        prepare_dialog(&mut app, GridViewMode::Details);

        let mut window = egui::Rect::NOTHING;
        for _ in 0..3 {
            window = run_dialog_frame(&mut app, &ctx, size, Vec::new());
        }
        let body_pointer = egui::pos2(window.center().x, window.center().y - 20.0);
        let scroll_down = || {
            vec![
                egui::Event::PointerMoved(body_pointer),
                egui::Event::MouseWheel {
                    unit: egui::MouseWheelUnit::Point,
                    delta: egui::vec2(0.0, -480.0),
                    modifiers: egui::Modifiers::NONE,
                },
            ]
        };
        for _ in 0..4 {
            window = run_dialog_frame(&mut app, &ctx, size, scroll_down());
        }

        let observation = thumb_quality_dialog_observation(&ctx);
        let apply_rect = observation
            .apply_rect
            .expect("B apply response must be produced by the real compact window");
        assert_inside(apply_rect, window);
        let apply_center = apply_rect.center();
        run_dialog_frame(
            &mut app,
            &ctx,
            size,
            vec![egui::Event::PointerMoved(apply_center)],
        );
        assert!(
            thumb_quality_dialog_observation(&ctx).apply_hovered,
            "B apply response must be reachable after scrolling the real dialog"
        );
        run_dialog_frame(
            &mut app,
            &ctx,
            size,
            vec![pointer_button(apply_center, true)],
        );
        assert!(
            thumb_quality_dialog_observation(&ctx).apply_pointer_down,
            "the reachable B response must accept a pointer press"
        );
        assert!(
            app.tq.show,
            "press-only test must not apply or save settings"
        );
    }

    #[test]
    fn preview_click_uses_the_rendered_fitted_rect() {
        let ctx = egui::Context::default();
        crate::ui_fonts::configure_fonts(&ctx);
        let size = egui::vec2(360.0, 520.0);

        let first = run_panel_frame(&ctx, size, Vec::new());
        assert!(!first.open_fullscreen);
        let center = first.preview_rect.expect("preview must render").center();
        assert!(
            !run_panel_frame(
                &ctx,
                size,
                vec![
                    egui::Event::PointerMoved(center),
                    pointer_button(center, true),
                ]
            )
            .open_fullscreen
        );
        assert!(run_panel_frame(&ctx, size, vec![pointer_button(center, false)]).open_fullscreen);
    }

    #[test]
    fn apply_button_remains_reachable_in_the_narrow_panel() {
        let ctx = egui::Context::default();
        crate::ui_fonts::configure_fonts(&ctx);
        let size = egui::vec2(320.0, 480.0);
        let first = run_panel_frame(&ctx, size, Vec::new());
        let apply = first.apply_rect.expect("apply button must render");
        assert_inside(apply, egui::Rect::from_min_size(egui::Pos2::ZERO, size));
        let center = apply.center();
        assert!(
            !run_panel_frame(
                &ctx,
                size,
                vec![
                    egui::Event::PointerMoved(center),
                    pointer_button(center, true),
                ],
            )
            .apply
        );
        assert!(run_panel_frame(&ctx, size, vec![pointer_button(center, false)]).apply);
    }

    #[test]
    fn title_close_button_closes_and_cleans_the_dialog() {
        let mut app = setup_app_for_test();
        let ctx = egui::Context::default();
        crate::ui_fonts::configure_fonts_with_settings(&ctx, &app.settings.ui_font);
        let size = egui::vec2(700.0, 560.0);
        prepare_dialog(&mut app, GridViewMode::Details);
        let mut rect = egui::Rect::NOTHING;
        for _ in 0..3 {
            rect = run_dialog_frame(&mut app, &ctx, size, Vec::new());
        }
        // Match egui's title-bar layout: the close glyph is centered in the right-most square
        // whose side is the full title height (window.rs `TitleBar::close_button_ui`).
        let style = ctx.style();
        let frame = egui::Frame::window(&style);
        let heading = egui::TextStyle::Heading.resolve(&style);
        let title_inner_height = ctx.fonts_mut(|fonts| fonts.row_height(&heading));
        let title_height =
            title_inner_height.max(style.spacing.interact_size.y) + frame.inner_margin.sum().y;
        let close_pos = egui::pos2(
            rect.right() - frame.stroke.width - title_height * 0.5,
            rect.top() + frame.stroke.width + title_height * 0.5,
        );
        run_dialog_frame(
            &mut app,
            &ctx,
            size,
            vec![
                egui::Event::PointerMoved(close_pos),
                pointer_button(close_pos, true),
            ],
        );
        let _ = ctx.run(
            raw_input(size, vec![pointer_button(close_pos, false)]),
            |ctx| {
                app.show_thumb_quality_dialog_window(ctx);
            },
        );

        assert!(!app.tq.show);
        assert!(app.tq.sample.is_none());
        assert!(app.tq.load_pending.is_none());
    }
}

impl App {
    pub(crate) fn show_thumb_quality_dialog_window(&mut self, ctx: &egui::Context) {
        // ── サムネイル画質設定ポップアップ ────────────────────────────
        if self.tq.show {
            let mut open = true;
            let mut apply_a = false;
            let mut apply_b = false;
            let mut reencode_a = false;
            let mut reencode_b = false;
            let mut open_fs_a = false;
            let mut open_fs_b = false;
            let mut close_requested = false;
            #[cfg(test)]
            let mut panel_b_observation = ThumbQualityPanelAction::default();
            let escape_pressed = self.dialog_escape_pressed(ctx);

            let layout = thumb_quality_dialog_layout(
                ctx.content_rect(),
                self.settings.grid_view_mode == crate::settings::GridViewMode::Thumbnail,
                self.last_cell_size,
                self.last_cell_h,
                self.effective_thumb_aspect().height_ratio(),
            );
            let dialog_rect =
                egui::Rect::from_center_size(layout.safe_rect.center(), layout.default_size);

            egui::Window::new(THUMB_QUALITY_DIALOG_ID)
                .id(egui::Id::new(THUMB_QUALITY_DIALOG_ID))
                .open(&mut open)
                .resizable(true)
                .collapsible(false)
                .default_pos(dialog_rect.min)
                .default_size(layout.default_size)
                .min_size(layout.min_size)
                .max_size(layout.safe_rect.size())
                .constrain_to(layout.safe_rect)
                .show(ctx, |ui| {
                    let content_rect = ui.available_rect_before_wrap();
                    let mut reserved = ui.new_child(
                        egui::UiBuilder::new()
                            .max_rect(content_rect)
                            .layout(egui::Layout::bottom_up(egui::Align::Min)),
                    );
                    {
                        let ui = &mut reserved;
                            // Lay out the fixed controls first. Their measured height, the
                            // separator, and the layout spacing are thereby removed from
                            // `available_size` before the scroll body claims the remainder.
                            // Keeping this bottom-up avoids feeding an estimated footer height
                            // back into a resizable Window's next-frame auto-size calculation.
                            let resize_corner = ui.visuals().resize_corner_size;
                            ui.allocate_ui_with_layout(
                                egui::vec2(
                                    ui.available_width(),
                                    ui.spacing().interact_size.y,
                                ),
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    ui.add_space(resize_corner);
                                    if ui.button("  閉じる  ").clicked() {
                                        close_requested = true;
                                    }
                                    ui.label(format!(
                                        "現在の設定: {}px / q={}",
                                        self.settings.thumb_px, self.settings.thumb_quality
                                    ));
                                },
                            );
                            ui.separator();

                            let body_rect = ui.available_rect_before_wrap();
                            let mut body = ui.new_child(
                                egui::UiBuilder::new()
                                    .max_rect(body_rect)
                                    .layout(egui::Layout::top_down(egui::Align::Min)),
                            );
                            {
                                    let ui = &mut body;
                                    let body_size = body_rect.size().max(egui::vec2(1.0, 1.0));
                                    ui.set_max_size(body_size);
                                    ui.spacing_mut().scroll =
                                        super::non_overlapping_dialog_scroll_style(
                                            ui.spacing().scroll,
                                        );
                                    egui::ScrollArea::vertical()
                                        .id_salt("thumb_quality_dialog_body")
                                        .auto_shrink([false, false])
                                        .max_height(body_size.y)
                                        .show(ui, |ui| {
                                    ui.set_max_width(ui.available_width());
                                    if self.tq.sample.is_none() {
                                        if self.tq.load_pending.is_some() {
                                            ui.label("サンプル画像を読み込み中…");
                                        } else if self.tq.load_failed {
                                            ui.label("サンプル画像を読み込めませんでした。");
                                        } else {
                                            ui.label(
                                                "画像を1枚選択してからもう一度お試しください。",
                                            );
                                        }
                                        return;
                                    }

                                    if let Some(ref display_name) = self.tq.sample_display_name {
                                        ui.label(
                                            egui::RichText::new(format!(
                                                "サンプル: {display_name}"
                                            ))
                                            .small(),
                                        );
                                    }
                                    if let Some(ref img) = self.tq.sample {
                                        let sz = self.tq.sample_original_size;
                                        let sz_str = if sz >= 1024 * 1024 {
                                            format!("{:.1} MB", sz as f64 / (1024.0 * 1024.0))
                                        } else {
                                            format!("{:.0} KB", sz as f64 / 1024.0)
                                        };
                                        ui.label(
                                            egui::RichText::new(format!(
                                                "（元サイズ {}x{} / {}）",
                                                img.width(),
                                                img.height(),
                                                sz_str
                                            ))
                                            .weak()
                                            .small(),
                                        );
                                    }

                                    let size_reference = if self.settings.grid_view_mode
                                        == crate::settings::GridViewMode::Thumbnail
                                    {
                                        format!(
                                            "現在のグリッド表示サイズ: {} × {} px  （{} 列 / アスペクト比 {}）",
                                            self.last_cell_size.round() as i32,
                                            self.last_cell_h.round() as i32,
                                            self.settings.grid_cols,
                                            self.effective_thumb_aspect().label(),
                                        )
                                    } else {
                                        format!(
                                            "プレビュー基準サイズ: {} × {} px  （アスペクト比 {}）",
                                            layout.preview_reference.x.round() as i32,
                                            layout.preview_reference.y.round() as i32,
                                            self.effective_thumb_aspect().label(),
                                        )
                                    };
                                    ui.label(egui::RichText::new(size_reference).small());
                                    ui.add_space(8.0);
                                    ui.separator();
                                    ui.add_space(6.0);

                                    if ui.available_width() < THUMB_QUALITY_COMPACT_WIDTH {
                                        let a = draw_thumb_quality_panel(
                                            ui,
                                            "A",
                                            &self.tq.a_texture,
                                            &mut self.tq.a_size,
                                            &mut self.tq.a_quality,
                                            self.tq.a_bytes,
                                            layout.preview_reference,
                                        );
                                        ui.add_space(12.0);
                                        ui.separator();
                                        ui.add_space(8.0);
                                        let b = draw_thumb_quality_panel(
                                            ui,
                                            "B",
                                            &self.tq.b_texture,
                                            &mut self.tq.b_size,
                                            &mut self.tq.b_quality,
                                            self.tq.b_bytes,
                                            layout.preview_reference,
                                        );
                                        reencode_a = a.reencode;
                                        apply_a = a.apply;
                                        open_fs_a = a.open_fullscreen;
                                        reencode_b = b.reencode;
                                        apply_b = b.apply;
                                        open_fs_b = b.open_fullscreen;
                                        #[cfg(test)]
                                        {
                                            panel_b_observation = b;
                                        }
                                    } else {
                                        ui.columns(2, |cols| {
                                            let a = draw_thumb_quality_panel(
                                                &mut cols[0],
                                                "A",
                                                &self.tq.a_texture,
                                                &mut self.tq.a_size,
                                                &mut self.tq.a_quality,
                                                self.tq.a_bytes,
                                                layout.preview_reference,
                                            );
                                            let b = draw_thumb_quality_panel(
                                                &mut cols[1],
                                                "B",
                                                &self.tq.b_texture,
                                                &mut self.tq.b_size,
                                                &mut self.tq.b_quality,
                                                self.tq.b_bytes,
                                                layout.preview_reference,
                                            );
                                            reencode_a = a.reencode;
                                            apply_a = a.apply;
                                            open_fs_a = a.open_fullscreen;
                                            reencode_b = b.reencode;
                                            apply_b = b.apply;
                                            open_fs_b = b.open_fullscreen;
                                            #[cfg(test)]
                                            {
                                                panel_b_observation = b;
                                            }
                                        });
                                    }
                                        });
                            }
                    }
                    // The Window must observe exactly its current content rect. Child UIs above
                    // perform the measured split without adding a trailing layout spacing to the
                    // resizable container's next-frame `last_content_size`.
                    ui.advance_cursor_after_rect(content_rect);
                    #[cfg(test)]
                    ui.ctx().data_mut(|data| {
                        data.insert_temp(
                            egui::Id::new(THUMB_QUALITY_DIALOG_OBSERVATION_ID),
                            panel_b_observation,
                        );
                    });
                });

            if reencode_a {
                self.reencode_tq_panel(true);
            }
            if reencode_b {
                self.reencode_tq_panel(false);
            }
            if open_fs_a || open_fs_b {
                self.tq.fullscreen = true;
                // divider 位置はリセットせず、前回の位置を維持する
            }
            if apply_a {
                self.settings.thumb_px = self.tq.a_size;
                self.settings.thumb_quality = self.tq.a_quality;
                self.settings.save();
                self.close_thumb_quality_dialog();
            } else if apply_b {
                self.settings.thumb_px = self.tq.b_size;
                self.settings.thumb_quality = self.tq.b_quality;
                self.settings.save();
                self.close_thumb_quality_dialog();
            } else if close_requested || !open || (escape_pressed && !self.tq.fullscreen) {
                // fullscreen preview overlay 表示中は overlay 側 (後続呼び出し) が
                // Escape を処理する。親ダイアログまで閉じてしまわないようガード。
                self.close_thumb_quality_dialog();
            }
        }
    }
}
