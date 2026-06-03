use crate::app::{App, ExportCropCreateDrag, ExportCropDrag};
use crate::export_crop::{CropAspectMode, CropHandle, CropRect, CropSettings};

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

impl App {
    pub(crate) fn export_crop_panel_rect(&self, full_rect: egui::Rect) -> egui::Rect {
        let pos = egui::pos2(full_rect.left() + PANEL_MARGIN, full_rect.top() + PANEL_TOP);
        let h = export_crop_panel_outer_height(full_rect, pos);
        egui::Rect::from_min_size(pos, egui::vec2(PANEL_W, h))
    }

    pub(crate) fn handle_export_crop_keys(
        &mut self,
        ctx: &egui::Context,
        fs_idx: usize,
    ) -> crate::ui_fullscreen::FsKeyAction {
        let action = crate::ui_fullscreen::FsKeyAction {
            close: false,
            nav_delta: 0,
            ctrl_nav: None,
            sibling_nav: None,
            jump_to: None,
        };
        if ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape)) {
            self.export_crop_mode = false;
            self.export_crop_drag = None;
            self.export_crop_create_drag = None;
            return action;
        }
        if ctx.input_mut(|i| i.consume_key(egui::Modifiers::CTRL, egui::Key::E)) {
            self.export_crop_mode = false;
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
        let mut open_export = false;

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
                    .fill(egui::Color32::from_rgba_unmultiplied(24, 24, 24, 238))
                    .show(ui, |ui| {
                        ui.set_min_width(PANEL_W);
                        ui.set_max_width(PANEL_W);
                        ui.set_max_height(panel_h);
                        ui.visuals_mut().override_text_color = Some(egui::Color32::WHITE);

                        ui.horizontal(|ui| {
                            ui.heading("エクスポート");
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    if ui.button("×").clicked() {
                                        close = true;
                                    }
                                },
                            );
                        });
                        ui.separator();

                        if ui.button("エクスポート...").clicked() {
                            open_export = true;
                        }
                        if ui.button("出力フォルダを開く").clicked() {
                            self.open_capture_output_dir();
                        }

                        ui.separator();
                        ui.label("切り取り");
                        let Some(image_size) = image_size else {
                            ui.add_enabled(false, egui::Button::new("画像読み込み待ち"));
                            return;
                        };

                        self.draw_export_crop_controls(ui, fs_idx, image_size);
                    });
            });

        if close {
            self.export_crop_mode = false;
            self.export_crop_drag = None;
            self.export_crop_create_drag = None;
        }
        if open_export {
            self.export_crop_mode = false;
            self.open_export_dialog_for_current(ctx, fs_idx);
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
            if let Some(mut settings) = current {
                settings.aspect_mode = aspect_mode;
                if let Some(ratio) = self.export_crop_aspect_ratio(settings, image_size) {
                    settings.rect = settings.rect.fit_to_aspect_around_center(
                        ratio,
                        image_size[0],
                        image_size[1],
                    );
                }
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
                let rect = centered_default_crop(image_size);
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
