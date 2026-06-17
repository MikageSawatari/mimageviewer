use crate::app::{App, ExportCropCreateDrag, ExportCropDrag};
use crate::export_crop::{CropAspectMode, CropHandle, CropRect, CropSettings};
use crate::keymap::KeyAction;

const PANEL_W: f32 = 220.0;
const PANEL_MARGIN: f32 = 14.0;
const PANEL_TOP: f32 = 72.0;

fn export_crop_panel_outer_height(full_rect: egui::Rect, panel_pos: egui::Pos2) -> f32 {
    (full_rect.bottom() - panel_pos.y - PANEL_MARGIN).clamp(220.0, 440.0)
}

fn outside_rects(image: egui::Rect, crop: egui::Rect) -> [egui::Rect; 4] {
    [
        egui::Rect::from_min_max(image.min, egui::pos2(image.max.x, crop.min.y)),
        egui::Rect::from_min_max(egui::pos2(image.min.x, crop.max.y), image.max),
        egui::Rect::from_min_max(
            egui::pos2(image.min.x, crop.min.y),
            egui::pos2(crop.min.x, crop.max.y),
        ),
        egui::Rect::from_min_max(
            egui::pos2(crop.max.x, crop.min.y),
            egui::pos2(image.max.x, crop.max.y),
        ),
    ]
}

fn clamp_pos_to_rect(pos: egui::Pos2, rect: egui::Rect) -> egui::Pos2 {
    egui::pos2(
        pos.x.clamp(rect.left(), rect.right()),
        pos.y.clamp(rect.top(), rect.bottom()),
    )
}

fn screen_to_image(image_rect: egui::Rect, image_size: [usize; 2], pos: egui::Pos2) -> [f32; 2] {
    let p = clamp_pos_to_rect(pos, image_rect);
    [
        ((p.x - image_rect.left()) / image_rect.width().max(1.0) * image_size[0].max(1) as f32)
            .clamp(0.0, image_size[0].max(1) as f32),
        ((p.y - image_rect.top()) / image_rect.height().max(1.0) * image_size[1].max(1) as f32)
            .clamp(0.0, image_size[1].max(1) as f32),
    ]
}

fn handle_points(rect: egui::Rect) -> [(CropHandle, egui::Pos2); 8] {
    let c = rect.center();
    [
        (CropHandle::NorthWest, rect.left_top()),
        (CropHandle::North, egui::pos2(c.x, rect.top())),
        (CropHandle::NorthEast, rect.right_top()),
        (CropHandle::East, egui::pos2(rect.right(), c.y)),
        (CropHandle::SouthEast, rect.right_bottom()),
        (CropHandle::South, egui::pos2(c.x, rect.bottom())),
        (CropHandle::SouthWest, rect.left_bottom()),
        (CropHandle::West, egui::pos2(rect.left(), c.y)),
    ]
}

fn crop_handle_cursor(handle: CropHandle) -> egui::CursorIcon {
    match handle {
        CropHandle::North | CropHandle::South => egui::CursorIcon::ResizeVertical,
        CropHandle::East | CropHandle::West => egui::CursorIcon::ResizeHorizontal,
        CropHandle::NorthWest | CropHandle::SouthEast => egui::CursorIcon::ResizeNwSe,
        CropHandle::NorthEast | CropHandle::SouthWest => egui::CursorIcon::ResizeNeSw,
        CropHandle::Body => egui::CursorIcon::Grab,
    }
}

pub(crate) fn auto_export_crop_rect_from_pixels(pixels: &egui::ColorImage) -> Option<CropRect> {
    let bbox =
        crate::margin_fit::detect_content_bbox(pixels, crate::margin_fit::DEFAULT_TOLERANCE)?;
    let [w, h] = pixels.size;
    Some(
        CropRect {
            min_x: bbox.min.x * w.max(1) as f32,
            min_y: bbox.min.y * h.max(1) as f32,
            max_x: bbox.max.x * w.max(1) as f32,
            max_y: bbox.max.y * h.max(1) as f32,
        }
        .sanitized(w, h),
    )
}

impl App {
    pub(crate) fn export_crop_panel_rect(&self, full_rect: egui::Rect) -> egui::Rect {
        let pos = egui::pos2(full_rect.left() + PANEL_MARGIN, full_rect.top() + PANEL_TOP);
        let h = export_crop_panel_outer_height(full_rect, pos);
        egui::Rect::from_min_size(pos, egui::vec2(PANEL_W, h))
    }

    /// 切り取りモードに入る。見開き double 表示中は消しゴム / 隠蔽加工と同じく
    /// Single ページへ pivot し、退場時に spread 状態を復元する。crop は表示 overlay
    /// のみで重い合成が無いため、post_filter バイパスや base cache 準備は行わない。
    pub(crate) fn enter_export_crop_mode(&mut self, fs_idx: usize) {
        let spread_pair = match self.resolve_spread_pair(fs_idx) {
            crate::ui_fullscreen::SpreadPair::Double { left, right } => Some((left, right)),
            crate::ui_fullscreen::SpreadPair::Single => None,
        };
        if let Some(pair) = spread_pair {
            self.export_crop_spread_ctx = Some(crate::app::EraseSpreadCtx {
                saved_mode: self.spread_mode,
                pair,
            });
            self.spread_mode = crate::settings::SpreadMode::Single;
            self.fullscreen_idx = Some(pair.0);
            self.fs_zoom = 1.0;
            self.fs_pan = egui::Vec2::ZERO;
        }
        self.export_crop_mode = true;
        self.export_crop_drag = None;
        self.export_crop_create_drag = None;
    }

    /// 切り取りモードを終了する。crop 矩形はドラッグ確定ごとに DB / サイドカーへ
    /// 保存済みなので、ここでは flag / ドラッグ状態のクリアと spread 復元だけ行う。
    pub(crate) fn reset_export_crop_mode(&mut self) {
        self.export_crop_mode = false;
        self.export_crop_drag = None;
        self.export_crop_create_drag = None;
        if let Some(ctx) = self.export_crop_spread_ctx.take() {
            self.spread_mode = ctx.saved_mode;
            self.fullscreen_idx = Some(ctx.pair.0);
            self.fs_zoom = 1.0;
            self.fs_pan = egui::Vec2::ZERO;
        }
    }

    pub(crate) fn handle_export_crop_keys(
        &mut self,
        ctx: &egui::Context,
        fs_idx: usize,
    ) -> crate::ui_fullscreen::FsKeyAction {
        let action = crate::ui_fullscreen::FsKeyAction {
            close: false,
            close_to_page_list: false,
            nav_delta: 0,
            ctrl_nav: None,
            sibling_nav: None,
            mouse_nav: None,
            jump_to: None,
        };
        if ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape)) {
            self.reset_export_crop_mode();
            return action;
        }
        if self.keymap.consume_action(ctx, KeyAction::CropExecute) {
            self.reset_export_crop_mode();
            self.open_export_dialog_for_current(ctx, fs_idx);
        }
        action
    }

    pub(crate) fn draw_export_crop_panel(
        &mut self,
        ctx: &egui::Context,
        full_rect: egui::Rect,
        fs_idx: usize,
        image_size: Option<[usize; 2]>,
    ) {
        if !self.export_crop_mode {
            return;
        }
        let panel_rect = self.export_crop_panel_rect(full_rect);
        let panel_pos = panel_rect.min;
        let panel_h = panel_rect.height();
        let mut close = false;

        egui::Area::new(egui::Id::new("export_crop_panel"))
            .order(egui::Order::Foreground)
            .fixed_pos(panel_pos)
            .show(ctx, |ui| {
                ui.interact(
                    egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(PANEL_W + 12.0, panel_h + 12.0),
                    ),
                    egui::Id::new("export_crop_panel_click_sink"),
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
                        ui.set_max_height(panel_h);
                        // ⚠ テーマに依存せず常に DARK visuals を使う (隠蔽加工パネルと同じ。
                        // これが無いとライトテーマで widget 背景色が崩れる)。
                        *ui.visuals_mut() = egui::Visuals::dark();
                        ui.visuals_mut().override_text_color = Some(egui::Color32::WHITE);

                        ui.horizontal(|ui| {
                            ui.heading("切り取り");
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    // 閉じる × ボタン (隠蔽加工パネルと同じ自前描画)。
                                    let (close_rect, close_resp) = ui.allocate_exact_size(
                                        egui::vec2(26.0, 22.0),
                                        egui::Sense::click(),
                                    );
                                    let close_bg = if close_resp.hovered() {
                                        egui::Color32::from_rgba_unmultiplied(220, 80, 80, 200)
                                    } else {
                                        egui::Color32::from_rgba_unmultiplied(80, 80, 80, 120)
                                    };
                                    ui.painter().rect_filled(close_rect, 4.0, close_bg);
                                    crate::ui_fullscreen::draw_icons::draw_close_icon(
                                        ui.painter(),
                                        close_rect.center(),
                                        8.0,
                                    );
                                    if close_resp.clicked() {
                                        close = true;
                                    }
                                    close_resp.on_hover_text("閉じる (Esc)");
                                },
                            );
                        });
                        ui.separator();

                        let Some(image_size) = image_size else {
                            ui.add_enabled(false, egui::Button::new("画像読み込み待ち"));
                            return;
                        };

                        self.draw_export_crop_controls(ui, fs_idx, image_size);
                    });
            });

        if close {
            self.reset_export_crop_mode();
        }
    }

    fn draw_export_crop_controls(
        &mut self,
        ui: &mut egui::Ui,
        fs_idx: usize,
        image_size: [usize; 2],
    ) {
        let current = self.export_crop_for_idx(fs_idx, image_size);
        let mut aspect_mode = current
            .map(|settings| settings.aspect_mode)
            .unwrap_or(self.export_crop_aspect_mode);
        egui::ComboBox::from_id_salt("export_crop_aspect")
            .selected_text(aspect_mode.label())
            .show_ui(ui, |ui| {
                for mode in CropAspectMode::ALL {
                    ui.selectable_value(&mut aspect_mode, mode, mode.label());
                }
            });
        if aspect_mode != self.export_crop_aspect_mode
            || current.is_some_and(|settings| settings.aspect_mode != aspect_mode)
        {
            self.export_crop_aspect_mode = aspect_mode;
            if let Some(ratio) = aspect_mode.aspect_ratio() {
                // 固定比率を選んだら、その比率で「画像中央寄せ・最大面積」の crop を
                // 即座に設定する (current が無くても作成。次のドラッグを待たない)。
                let rect = CropRect::full(image_size[0], image_size[1])
                    .fit_to_aspect_around_center(ratio, image_size[0], image_size[1]);
                self.set_export_crop_for_idx(
                    fs_idx,
                    Some(CropSettings { rect, aspect_mode }),
                    image_size,
                );
            } else if let Some(mut settings) = current {
                // 自由 / 現在比率: 既存 crop の比率モードだけ更新し rect は維持する。
                settings.aspect_mode = aspect_mode;
                self.set_export_crop_for_idx(fs_idx, Some(settings), image_size);
            }
        }

        let mut settings = current.unwrap_or(CropSettings {
            rect: CropRect::full(image_size[0], image_size[1]),
            aspect_mode,
        });
        settings.aspect_mode = aspect_mode;

        let active = current.is_some();
        ui.horizontal(|ui| {
            if ui
                .add_enabled(!active, egui::Button::new("有効化"))
                .clicked()
            {
                // 固定比率が選ばれていればその比率で中央寄せ・最大面積、
                // 自由 / 現在比率なら 80% 中央のデフォルト矩形を作る。
                let rect = match aspect_mode.aspect_ratio() {
                    Some(ratio) => CropRect::full(image_size[0], image_size[1])
                        .fit_to_aspect_around_center(ratio, image_size[0], image_size[1]),
                    None => centered_default_crop(image_size),
                };
                self.set_export_crop_for_idx(
                    fs_idx,
                    Some(CropSettings { rect, aspect_mode }),
                    image_size,
                );
            }
            if ui.add_enabled(active, egui::Button::new("解除")).clicked() {
                self.set_export_crop_for_idx(fs_idx, None, image_size);
            }
        });
        let auto_enabled = self.current_raw_source_pixels(fs_idx).is_some();
        let auto_resp = ui.add_enabled(
            auto_enabled,
            egui::Button::new("自動クロップ").min_size(egui::vec2(ui.available_width(), 24.0)),
        );
        if auto_resp.clicked() {
            if self.apply_auto_export_crop_for_idx(fs_idx) {
                self.show_feedback_toast("[自動クロップ]".to_string());
            } else {
                self.show_feedback_toast("[余白を検出できません]".to_string());
            }
        }
        auto_resp.on_hover_text("四辺の単色余白を検出して切り取り範囲に設定します");

        let mut x = settings.rect.min_x.round() as i32;
        let mut y = settings.rect.min_y.round() as i32;
        let mut w = settings.rect.width().round() as i32;
        let mut h = settings.rect.height().round() as i32;
        let before = (x, y, w, h);
        ui.horizontal(|ui| {
            ui.label("X");
            ui.add(egui::DragValue::new(&mut x).range(0..=image_size[0].saturating_sub(1) as i32));
            ui.label("Y");
            ui.add(egui::DragValue::new(&mut y).range(0..=image_size[1].saturating_sub(1) as i32));
        });
        ui.horizontal(|ui| {
            ui.label("W");
            ui.add(egui::DragValue::new(&mut w).range(1..=image_size[0].max(1) as i32));
            ui.label("H");
            ui.add(egui::DragValue::new(&mut h).range(1..=image_size[1].max(1) as i32));
        });
        if (x, y, w, h) != before {
            let rect = crate::export_crop::crop_from_xywh_inputs(
                x,
                y,
                w,
                h,
                image_size[0],
                image_size[1],
                self.export_crop_aspect_ratio(settings, image_size),
                h != before.3,
            );
            self.set_export_crop_for_idx(
                fs_idx,
                Some(CropSettings { rect, aspect_mode }),
                image_size,
            );
        }
    }

    pub(crate) fn apply_auto_export_crop_for_idx(&mut self, fs_idx: usize) -> bool {
        let Some(source) = self.current_raw_source_pixels(fs_idx) else {
            return false;
        };
        let Some(rect) = auto_export_crop_rect_from_pixels(&source) else {
            return false;
        };
        let image_size = source.size;
        self.export_crop_aspect_mode = CropAspectMode::Free;
        self.set_export_crop_for_idx(
            fs_idx,
            Some(CropSettings {
                rect,
                aspect_mode: CropAspectMode::Free,
            }),
            image_size,
        );
        true
    }

    fn export_crop_aspect_ratio(
        &self,
        settings: CropSettings,
        image_size: [usize; 2],
    ) -> Option<f32> {
        settings.aspect_mode.aspect_ratio().or_else(|| {
            (settings.aspect_mode == CropAspectMode::Keep).then(|| {
                let rect = settings.rect.sanitized(image_size[0], image_size[1]);
                rect.width() / rect.height().max(1.0)
            })
        })
    }

    pub(crate) fn draw_export_crop_overlay(
        &mut self,
        ui: &mut egui::Ui,
        image_rect: egui::Rect,
        fs_idx: usize,
        image_size: [usize; 2],
        pointer_allowed: bool,
    ) -> bool {
        let current = self.export_crop_for_idx(fs_idx, image_size);
        if !self.export_crop_mode && current.is_none() {
            return false;
        }
        let settings = current.unwrap_or(CropSettings {
            rect: CropRect::full(image_size[0], image_size[1]),
            aspect_mode: self.export_crop_aspect_mode,
        });
        let crop_screen = settings
            .rect
            .to_screen_rect(image_rect, image_size[0], image_size[1]);
        let painter = ui.painter().clone();
        for outside in outside_rects(image_rect, crop_screen) {
            painter.rect_filled(
                outside,
                0.0,
                egui::Color32::from_rgba_unmultiplied(0, 0, 0, 145),
            );
        }
        painter.rect_stroke(
            crop_screen,
            0.0,
            egui::Stroke::new(2.0, egui::Color32::from_rgb(255, 230, 100)),
            egui::StrokeKind::Inside,
        );
        painter.rect_stroke(
            crop_screen.expand(1.0),
            0.0,
            egui::Stroke::new(1.0, egui::Color32::BLACK),
            egui::StrokeKind::Outside,
        );

        if !self.export_crop_mode {
            return false;
        }

        let handle_bounds = image_rect.shrink(14.0);
        let handles = handle_points(crop_screen);
        for (_, center) in handles {
            let center = clamp_pos_to_rect(center, handle_bounds);
            painter.circle_filled(center, 5.5, egui::Color32::from_rgb(255, 245, 180));
            painter.circle_stroke(
                center,
                5.5,
                egui::Stroke::new(1.5, egui::Color32::from_rgb(30, 20, 0)),
            );
        }

        if !pointer_allowed {
            self.export_crop_drag = None;
            self.export_crop_create_drag = None;
            return false;
        }

        self.handle_export_crop_pointer(ui, image_rect, fs_idx, image_size, settings, handles)
    }

    fn handle_export_crop_pointer(
        &mut self,
        ui: &mut egui::Ui,
        image_rect: egui::Rect,
        fs_idx: usize,
        image_size: [usize; 2],
        settings: CropSettings,
        handles: [(CropHandle, egui::Pos2); 8],
    ) -> bool {
        let (primary_pressed, primary_down, primary_released, press_origin, hover_pos, total_delta) =
            ui.input(|i| {
                (
                    i.pointer.primary_pressed(),
                    i.pointer.primary_down(),
                    i.pointer.primary_released(),
                    i.pointer.press_origin(),
                    i.pointer.hover_pos(),
                    i.pointer.total_drag_delta().unwrap_or(egui::Vec2::ZERO),
                )
            });
        const HANDLE_HIT: f32 = 32.0;
        const CREATE_DRAG_THRESHOLD: f32 = 4.0;
        let pointer_pos = hover_pos.or(press_origin);
        let crop_drag_in_progress =
            self.export_crop_drag.is_some() || self.export_crop_create_drag.is_some();
        if !crop_drag_in_progress
            && self.handle_overlay_space_pan_drag(
                ui.ctx(),
                self.keymap
                    .key_held_action(ui.ctx(), KeyAction::CropSpacePan),
                pointer_pos.is_some_and(|pos| image_rect.contains(pos)),
                primary_pressed,
                primary_down,
                primary_released,
                pointer_pos,
            )
        {
            return true;
        }

        let target_at = |pos: egui::Pos2| -> Option<CropHandle> {
            for (handle, center) in handles {
                let center = clamp_pos_to_rect(center, image_rect.shrink(14.0));
                if (pos - center).length() <= HANDLE_HIT {
                    return Some(handle);
                }
            }
            let crop_screen =
                settings
                    .rect
                    .to_screen_rect(image_rect, image_size[0], image_size[1]);
            if crop_screen.contains(pos) {
                Some(CropHandle::Body)
            } else {
                None
            }
        };

        if primary_pressed && let Some(origin) = press_origin {
            if let Some(handle) = target_at(origin) {
                self.export_crop_drag = Some(ExportCropDrag {
                    handle,
                    base: settings.rect,
                });
                self.export_crop_create_drag = None;
            } else if image_rect.contains(origin) {
                self.export_crop_drag = None;
                self.export_crop_create_drag = Some(ExportCropCreateDrag {
                    start: screen_to_image(image_rect, image_size, origin),
                });
            }
        }

        let mut used = false;
        if primary_down {
            if let Some(drag) = self.export_crop_drag {
                let scale_x = image_size[0].max(1) as f32 / image_rect.width().max(1.0);
                let scale_y = image_size[1].max(1) as f32 / image_rect.height().max(1.0);
                let rect = drag.base.dragged(
                    drag.handle,
                    total_delta.x * scale_x,
                    total_delta.y * scale_y,
                    image_size[0],
                    image_size[1],
                    self.export_crop_aspect_ratio(settings, image_size),
                );
                self.set_export_crop_for_idx_memory_only(
                    fs_idx,
                    Some(CropSettings {
                        rect,
                        aspect_mode: settings.aspect_mode,
                    }),
                    image_size,
                );
                ui.ctx()
                    .set_cursor_icon(if drag.handle == CropHandle::Body {
                        egui::CursorIcon::Grabbing
                    } else {
                        crop_handle_cursor(drag.handle)
                    });
                used = true;
            } else if let (Some(create), Some(cur), Some(origin)) =
                (self.export_crop_create_drag, hover_pos, press_origin)
            {
                if (cur - origin).length() >= CREATE_DRAG_THRESHOLD {
                    let rect = crate::export_crop::crop_from_points(
                        create.start,
                        screen_to_image(image_rect, image_size, cur),
                        image_size[0],
                        image_size[1],
                        self.export_crop_aspect_ratio(settings, image_size),
                    );
                    self.set_export_crop_for_idx_memory_only(
                        fs_idx,
                        Some(CropSettings {
                            rect,
                            aspect_mode: settings.aspect_mode,
                        }),
                        image_size,
                    );
                    ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
                    used = true;
                }
            }
        }

        if primary_released {
            let final_settings = self.export_crop_for_idx(fs_idx, image_size);
            self.export_crop_drag = None;
            self.export_crop_create_drag = None;
            self.set_export_crop_for_idx(fs_idx, final_settings, image_size);
        }

        if !used && let Some(pos) = hover_pos {
            if let Some(handle) = target_at(pos) {
                ui.ctx().set_cursor_icon(crop_handle_cursor(handle));
                used = true;
            } else if image_rect.contains(pos) {
                ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
                used = true;
            }
        }

        used || self.export_crop_drag.is_some() || self.export_crop_create_drag.is_some()
    }
}

fn centered_default_crop(image_size: [usize; 2]) -> CropRect {
    let w = image_size[0].max(1) as f32;
    let h = image_size[1].max(1) as f32;
    let crop_w = (w * 0.8).max(1.0);
    let crop_h = (h * 0.8).max(1.0);
    CropRect {
        min_x: (w - crop_w) * 0.5,
        min_y: (h - crop_h) * 0.5,
        max_x: (w + crop_w) * 0.5,
        max_y: (h + crop_h) * 0.5,
    }
    .sanitized(image_size[0], image_size[1])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn white(w: usize, h: usize) -> egui::ColorImage {
        egui::ColorImage::new([w, h], vec![egui::Color32::WHITE; w * h])
    }

    fn fill(
        img: &mut egui::ColorImage,
        x0: usize,
        y0: usize,
        x1: usize,
        y1: usize,
        color: egui::Color32,
    ) {
        let [w, h] = img.size;
        for y in y0.min(h)..y1.min(h) {
            for x in x0.min(w)..x1.min(w) {
                img.pixels[y * w + x] = color;
            }
        }
    }

    #[test]
    fn auto_export_crop_rect_converts_detected_bbox_to_source_pixels() {
        let mut img = white(100, 80);
        fill(&mut img, 20, 16, 80, 64, egui::Color32::BLACK);

        let rect =
            auto_export_crop_rect_from_pixels(&img).expect("auto crop should detect content");

        assert!(
            rect.min_x >= 15.0 && rect.min_x <= 22.0,
            "min_x={}",
            rect.min_x
        );
        assert!(
            rect.min_y >= 11.0 && rect.min_y <= 18.0,
            "min_y={}",
            rect.min_y
        );
        assert!(
            rect.max_x >= 78.0 && rect.max_x <= 85.0,
            "max_x={}",
            rect.max_x
        );
        assert!(
            rect.max_y >= 62.0 && rect.max_y <= 69.0,
            "max_y={}",
            rect.max_y
        );
    }

    #[test]
    fn auto_export_crop_rect_returns_none_for_solid_page() {
        assert!(auto_export_crop_rect_from_pixels(&white(64, 64)).is_none());
    }
}
