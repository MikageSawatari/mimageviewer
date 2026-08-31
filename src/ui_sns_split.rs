use crate::app::{App, SnsSplitDrag};
use crate::displayed_image_transform::DisplayedImageTransform;
use crate::export_crop::{CropHandle, CropRect};
use crate::keymap::KeyAction;
use crate::sns_split::{MAX_COUNT, MIN_COUNT, SnsSplitLayout, SnsTarget};

const PANEL_W: f32 = 240.0;
const PANEL_MARGIN: f32 = 14.0;
const PANEL_TOP: f32 = 72.0;

#[derive(Clone, Debug, PartialEq, Eq)]
struct SnsSplitPanelSummary {
    dimensions: String,
    warning: Option<&'static str>,
}

impl SnsSplitPanelSummary {
    fn from_layout(layout: SnsSplitLayout, image_size: [usize; 2]) -> Self {
        let frames = layout.frames();
        let frame = frames[0];
        let width = frame.width().round().max(1.0) as usize;
        let height = frame.height().round().max(1.0) as usize;
        Self {
            dimensions: format!("{width} x {height} x {} 枚", frames.len()),
            warning: (!layout.fits(image_size)).then_some(
                "画像が小さすぎるため、選択した枚数を配置できません。書き出しには進めません。",
            ),
        }
    }
}

fn sns_split_panel_outer_height(full_rect: egui::Rect, panel_pos: egui::Pos2) -> f32 {
    (full_rect.bottom() - panel_pos.y - PANEL_MARGIN).clamp(220.0, 360.0)
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

    pub(crate) fn enter_sns_split_mode(&mut self, fs_idx: usize) -> bool {
        if self.sns_split.is_some() {
            return true;
        }
        let (target_idx, pivot) = self.plan_page_edit_pivot(fs_idx);
        let Some(source_size) = self.fs_page_coordinate_source_size(target_idx) else {
            return false;
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
        let layout = SnsSplitLayout::centered_max(target, count, image_size);

        if let Some(pivot) = pivot {
            self.sns_split_spread_ctx = Some(pivot);
            self.enter_page_edit_single_view(target_idx);
        }
        self.sns_split = Some(layout);
        self.sns_split_drag = None;
        true
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
        }
        action
    }

    fn execute_sns_split_export(&mut self, ctx: &egui::Context, fs_idx: usize) -> bool {
        let Some(layout) = self.sns_split else {
            return false;
        };
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
        if !layout.fits(image_size) {
            // パネルにも警告は出ているが、キーを押して無反応にはしない。
            self.show_feedback_toast(
                "画像が小さすぎるため、選択した枚数を配置できません".to_string(),
            );
            return false;
        }

        let frames = layout.frames();
        self.open_export_dialog_for_sns_split(ctx, fs_idx, frames, image_size)
    }

    pub(crate) fn draw_sns_split_panel(
        &mut self,
        ctx: &egui::Context,
        full_rect: egui::Rect,
        image_size: Option<[usize; 2]>,
    ) {
        if self.sns_split.is_none() {
            return;
        }
        let panel_rect = self.sns_split_panel_rect(full_rect);
        let panel_pos = panel_rect.min;
        let panel_height = panel_rect.height();
        let mut close = false;

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

                        let Some(image_size) = image_size else {
                            ui.add_enabled(false, egui::Button::new("画像読み込み待ち"));
                            return;
                        };
                        self.draw_sns_split_controls(ui, image_size);
                    });
            });

        if close {
            self.reset_sns_split_mode();
        }
    }

    fn draw_sns_split_controls(&mut self, ui: &mut egui::Ui, image_size: [usize; 2]) {
        let Some(mut layout) = self.sns_split else {
            return;
        };

        ui.label("投稿先");
        let mut target = layout.target;
        ui.horizontal(|ui| {
            for option in SnsTarget::ALL {
                ui.selectable_value(&mut target, option, option.label());
            }
        });

        ui.add_space(6.0);
        ui.label("枚数");
        let mut count = layout.count;
        ui.horizontal(|ui| {
            for option in MIN_COUNT..=MAX_COUNT {
                ui.selectable_value(&mut count, option, option.to_string());
            }
        });

        let mut changed = false;
        if target != layout.target {
            layout = layout.with_target(target, image_size);
            changed = true;
        }
        if count != layout.count {
            layout = layout.with_count(count, image_size);
            changed = true;
        }
        if changed {
            self.settings.sns_split_target = Some(layout.target.stable_key().to_string());
            self.settings.sns_split_count = layout.count;
            self.settings.save();
            self.sns_split = Some(layout);
        }

        ui.add_space(8.0);
        let summary = SnsSplitPanelSummary::from_layout(layout, image_size);
        ui.label(summary.dimensions);
        if let Some(warning) = summary.warning {
            ui.add_space(4.0);
            ui.colored_label(egui::Color32::from_rgb(255, 180, 90), warning);
        }
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
            let group = drag.base.group.dragged(
                drag.handle,
                end[0] - start[0],
                end[1] - start[1],
                image_size[0],
                image_size[1],
                Some(SnsSplitLayout::group_aspect(
                    drag.base.target,
                    drag.base.count,
                )),
            );
            self.sns_split = Some(SnsSplitLayout { group, ..drag.base }.clamped(image_size));
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
    fn target_and_count_controls_follow_geometry_contract() {
        let image_size = [8000, 6000];
        let layout = SnsSplitLayout::centered_max(SnsTarget::X, 2, image_size)
            .with_target(SnsTarget::Instagram, image_size)
            .with_count(4, image_size);

        assert_eq!(layout.target, SnsTarget::Instagram);
        assert_eq!(layout.frames().len(), 4);
        let expected = SnsSplitLayout::group_aspect(SnsTarget::Instagram, 4);
        assert!((layout.group.width() / layout.group.height() - expected).abs() < 0.01);
    }

    #[test]
    fn panel_summary_blocks_export_when_frames_do_not_fit() {
        let image_size = [3, 3];
        let layout = SnsSplitLayout::centered_max(SnsTarget::X, 4, image_size);
        let summary = SnsSplitPanelSummary::from_layout(layout, image_size);

        assert!(summary.warning.is_some());
        assert!(summary.warning.unwrap().contains("書き出しには進めません"));
        assert_eq!(summary.dimensions, "1 x 1 x 4 枚");
    }

    #[test]
    fn execute_shortcut_keeps_mode_and_skips_dialog_when_frames_do_not_fit() {
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
            !app.keymap.consume_action(&ctx, KeyAction::SnsSplitExecute),
            "SNS 分割の Ctrl+E は handler が消費する"
        );
        let _ = ctx.end_pass();

        assert_eq!(app.sns_split, Some(layout));
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
    fn execute_shortcut_opens_numbered_single_page_export_from_spread() {
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

        assert!(app.enter_sns_split_mode(0));
        let expected_frames = app.sns_split.expect("SNS 分割が開始していない").frames();
        assert_eq!(expected_frames.len(), 3);
        assert_eq!(app.spread_mode, crate::settings::SpreadMode::Single);
        assert!(app.sns_split_spread_ctx.is_some());

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
            !app.keymap.consume_action(&ctx, KeyAction::SnsSplitExecute),
            "SNS 分割の Ctrl+E は handler が消費する"
        );
        let _ = ctx.end_pass();

        assert!(app.sns_split.is_none());
        assert!(app.sns_split_spread_ctx.is_none());
        assert_eq!(app.spread_mode, crate::settings::SpreadMode::Ltr);
        let state = app.export_dialog.take().expect("export dialog should open");
        assert_eq!(state.scale, crate::export_dialog::ExportScale::Full);
        assert_eq!(state.sns_split_frames, expected_frames);
        assert_eq!(state.selection, original_selection);
        match state.pixels {
            crate::export_dialog::ExportPixels::Single(page) => {
                assert_eq!(page.base_pixels.size, image_size);
                assert_eq!(page.base_pixels.pixels[0], egui::Color32::RED);
            }
            crate::export_dialog::ExportPixels::Spread { .. } => {
                panic!("SNS 分割は編集対象の単ページだけを snapshot する")
            }
        }
    }
}
