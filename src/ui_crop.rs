use crate::app::{App, ExportCropCreateDrag, ExportCropDrag};
use crate::displayed_image_transform::DisplayedImageTransform;
use crate::export_crop::{CropAspectMode, CropHandle, CropRect, CropSettings};
use crate::keymap::KeyAction;

/// A gesture has precisely two terminals; end reasons choose one in a single policy below.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum OverlayDragFinish {
    Commit,
    Cancel,
}

/// All crop/SNS gesture end operations. Do not infer execution semantics from mode cleanup.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum OverlayDragEnd {
    Released,
    Execute,
    Escape,
    ModeExit,
    OwnerChanged,
    FocusLost,
    SecondaryPress,
    FrontUiClaim,
    SettingsChanged,
}

impl OverlayDragEnd {
    pub(crate) fn finish(self) -> OverlayDragFinish {
        match self {
            Self::Released | Self::Execute => OverlayDragFinish::Commit,
            Self::Escape
            | Self::ModeExit
            | Self::OwnerChanged
            | Self::FocusLost
            | Self::SecondaryPress
            | Self::FrontUiClaim
            | Self::SettingsChanged => OverlayDragFinish::Cancel,
        }
    }
}

pub(crate) fn overlay_drag_end(
    ctx: &egui::Context,
    front: &crate::ui_fullscreen::FsFrontPointerFrameInput,
    press_frame: u64,
) -> Option<OverlayDragEnd> {
    let interruption = ctx.input(|i| {
        if !i.focused
            || i.viewport().focused == Some(false)
            || i.events
                .iter()
                .any(|event| matches!(event, egui::Event::WindowFocused(false)))
        {
            Some(OverlayDragEnd::FocusLost)
        } else if i.pointer.secondary_pressed() {
            Some(OverlayDragEnd::SecondaryPress)
        } else if front.owns(egui::PointerButton::Primary)
            || front.owns_release(egui::PointerButton::Primary)
        {
            Some(OverlayDragEnd::FrontUiClaim)
        } else {
            None
        }
    });
    if interruption.is_some() {
        return interruption;
    }
    let frame = ctx.cumulative_frame_nr();
    let released = ctx.input(|i| {
        i.pointer.primary_released()
            || i.pointer.primary_pressed() && frame != press_frame
            || !i.pointer.primary_down()
    }) || !overlay_primary_physically_down(ctx);
    released.then_some(OverlayDragEnd::Released)
}

fn overlay_primary_physically_down(ctx: &egui::Context) -> bool {
    // Touch owns egui's logical primary pointer without holding a physical mouse button.
    if ctx.input(|i| i.any_touches()) {
        return true;
    }
    #[cfg(test)]
    return ctx
        .data(|data| data.get_temp::<bool>(egui::Id::unique("overlay_test_primary_level")))
        .unwrap_or_else(|| ctx.input(|i| i.pointer.primary_down()));
    #[cfg(all(windows, not(test)))]
    {
        use windows::Win32::UI::Input::KeyboardAndMouse::{
            GetAsyncKeyState, VK_LBUTTON, VK_RBUTTON,
        };
        use windows::Win32::UI::WindowsAndMessaging::{GetSystemMetrics, SM_SWAPBUTTON};
        let _ = ctx;
        // This is the level of a live mouse gesture, never the identity of a keyboard edge.
        // GetAsyncKeyState reports physical buttons, so honor the logical primary mapping.
        unsafe {
            let button = if GetSystemMetrics(SM_SWAPBUTTON) != 0 {
                VK_RBUTTON
            } else {
                VK_LBUTTON
            };
            GetAsyncKeyState(button.0 as i32) < 0
        }
    }
    #[cfg(all(not(windows), not(test)))]
    ctx.input(|i| i.pointer.primary_down())
}

const PANEL_W: f32 = 220.0;
const PANEL_MARGIN: f32 = 14.0;
const PANEL_TOP: f32 = 72.0;

fn export_crop_panel_outer_height(full_rect: egui::Rect, panel_pos: egui::Pos2) -> f32 {
    (full_rect.bottom() - panel_pos.y - PANEL_MARGIN).clamp(220.0, 440.0)
}

/// Whether the crop overlay may still act on the pointer this frame.
///
/// `pointer_allowed` answers "may the tool panel have this pointer", which is the wrong
/// question to ask of a drag that is already running. A drag starts on the image and owns
/// the pointer until the button comes up, so passing over the panel -- which sits in the
/// letterbox beside a portrait page -- must not end it. Ending it there froze the box for
/// good, because a create drag can only begin on a press, so coming back onto the image
/// did nothing (report 2026-09-02).
fn crop_overlay_keeps_the_pointer(
    pointer_allowed: bool,
    drag_in_progress: bool,
    primary_down: bool,
) -> bool {
    // The lifecycle service completes a released gesture before this admission check.
    // Only a still-held owner needs to retain the pointer over the panel.
    pointer_allowed || (drag_in_progress && primary_down)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ExportCropPointerStart {
    None,
    Pan,
    ForceCreate,
    Edit(CropHandle),
    Create,
}

fn classify_export_crop_pointer_start(
    primary_pressed: bool,
    space_pan_held: bool,
    force_create_held: bool,
    inside_image: bool,
    target: Option<CropHandle>,
) -> ExportCropPointerStart {
    if !primary_pressed {
        return ExportCropPointerStart::None;
    }
    if space_pan_held && inside_image {
        return ExportCropPointerStart::Pan;
    }
    if force_create_held && inside_image {
        return ExportCropPointerStart::ForceCreate;
    }
    if let Some(handle) = target {
        return ExportCropPointerStart::Edit(handle);
    }
    if inside_image {
        ExportCropPointerStart::Create
    } else {
        ExportCropPointerStart::None
    }
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
    let p = transform.screen_to_source_normalized(pos);
    [
        (p.x * image_size[0].max(1) as f32).clamp(0.0, image_size[0].max(1) as f32),
        (p.y * image_size[1].max(1) as f32).clamp(0.0, image_size[1].max(1) as f32),
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

fn crop_corners(
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
    let cx = (rect.min_x + rect.max_x) * 0.5;
    let cy = (rect.min_y + rect.max_y) * 0.5;
    [
        (
            CropHandle::NorthWest,
            image_to_screen(transform, image_size, [rect.min_x, rect.min_y]),
        ),
        (
            CropHandle::North,
            image_to_screen(transform, image_size, [cx, rect.min_y]),
        ),
        (
            CropHandle::NorthEast,
            image_to_screen(transform, image_size, [rect.max_x, rect.min_y]),
        ),
        (
            CropHandle::East,
            image_to_screen(transform, image_size, [rect.max_x, cy]),
        ),
        (
            CropHandle::SouthEast,
            image_to_screen(transform, image_size, [rect.max_x, rect.max_y]),
        ),
        (
            CropHandle::South,
            image_to_screen(transform, image_size, [cx, rect.max_y]),
        ),
        (
            CropHandle::SouthWest,
            image_to_screen(transform, image_size, [rect.min_x, rect.max_y]),
        ),
        (
            CropHandle::West,
            image_to_screen(transform, image_size, [rect.min_x, cy]),
        ),
    ]
}

fn paint_outside_crop(
    painter: &egui::Painter,
    transform: &DisplayedImageTransform,
    image_size: [usize; 2],
    crop: CropRect,
) {
    let full = crop_corners(
        transform,
        image_size,
        CropRect::full(image_size[0], image_size[1]),
    );
    let crop = crop_corners(transform, image_size, crop);
    let color = egui::Color32::from_rgba_unmultiplied(0, 0, 0, 145);
    for points in [
        vec![full[0], full[1], crop[1], crop[0]],
        vec![full[1], full[2], crop[2], crop[1]],
        vec![full[2], full[3], crop[3], crop[2]],
        vec![full[3], full[0], crop[0], crop[3]],
    ] {
        painter.add(egui::Shape::convex_polygon(
            points,
            color,
            egui::Stroke::NONE,
        ));
    }
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
    fn export_crop_drag_snapshot(&self) -> Option<&crate::app::ExportCropDragSnapshot> {
        self.export_crop_drag
            .as_ref()
            .map(|drag| &drag.before)
            .or_else(|| {
                self.export_crop_create_drag
                    .as_ref()
                    .map(|drag| &drag.before)
            })
    }

    fn begin_export_crop_drag_snapshot(
        &self,
        ctx: &egui::Context,
        idx: usize,
        image_size: [usize; 2],
    ) -> Option<crate::app::ExportCropDragSnapshot> {
        Some(crate::app::ExportCropDragSnapshot {
            owner: self.capture_page_edit_owner(idx)?,
            press_frame: ctx.cumulative_frame_nr(),
            idx,
            image_size,
            settings: self.export_crop_page_settings.get(&idx).copied(),
        })
    }

    pub(crate) fn finish_export_crop_drag(&mut self, end: OverlayDragEnd) -> bool {
        let Some(before) = self.export_crop_drag_snapshot() else {
            return false;
        };
        // A temporary mount of another viewer must not consume the foreground editor.
        if !self.page_edit_owner_is_current(&before.owner) {
            return false;
        }
        let before = self
            .export_crop_drag
            .take()
            .map(|drag| drag.before)
            .or_else(|| self.export_crop_create_drag.take().map(|drag| drag.before))
            .unwrap();
        if before.owner.items_generation != self.items_generation {
            return true;
        }
        match end.finish() {
            OverlayDragFinish::Commit => {
                let settings = self.export_crop_for_idx(before.idx, before.image_size);
                self.set_export_crop_for_idx(before.idx, settings, before.image_size);
            }
            OverlayDragFinish::Cancel => {
                // Restore the exact authored settings, including their original source_size.
                // The ordinary memory setter normalizes to the current raster, so cannot do this.
                if let Some(settings) = before.settings {
                    self.export_crop_page_settings.insert(before.idx, settings);
                    self.export_crop_pages.insert(before.idx);
                } else {
                    self.export_crop_page_settings.remove(&before.idx);
                    self.export_crop_pages.remove(&before.idx);
                }
                Self::set_page_key_presence(
                    &mut self.export_crop_page_keys,
                    &before.owner.target.page_key,
                    before.settings.is_some(),
                );
                // Some installers replace items immediately before advancing generation.
                // Their old projection must not be synchronized under the incoming page key.
                if self.page_path_key(before.idx).as_deref()
                    == Some(before.owner.target.page_key.as_str())
                {
                    self.sync_prepared_page_edit_key_for_idx(before.idx);
                }
            }
        }
        true
    }

    fn service_export_crop_drag(
        &mut self,
        ctx: &egui::Context,
        idx: usize,
        front: &crate::ui_fullscreen::FsFrontPointerFrameInput,
    ) {
        if let Some(before) = self.export_crop_drag_snapshot() {
            let end = if !self.export_crop_mode {
                Some(OverlayDragEnd::ModeExit)
            } else if before.idx != idx || before.owner.items_generation != self.items_generation {
                Some(OverlayDragEnd::OwnerChanged)
            } else {
                overlay_drag_end(ctx, front, before.press_frame)
            };
            if let Some(end) = end {
                self.finish_export_crop_drag(end);
            }
        }
    }

    pub(crate) fn export_crop_panel_rect(&self, full_rect: egui::Rect) -> egui::Rect {
        let pos = egui::pos2(full_rect.left() + PANEL_MARGIN, full_rect.top() + PANEL_TOP);
        let h = export_crop_panel_outer_height(full_rect, pos);
        egui::Rect::from_min_size(pos, egui::vec2(PANEL_W, h))
    }

    /// 切り取りモードに入る。見開き double 表示中は消しゴム / 隠蔽加工と同じく
    /// Single ページへ pivot し、退場時に spread 状態を復元する。crop は表示 overlay
    /// のみで重い合成が無いため、post_filter バイパスや base cache 準備は行わない。
    pub(crate) fn enter_export_crop_mode(&mut self, fs_idx: usize) {
        self.finish_export_crop_drag(OverlayDragEnd::ModeExit);
        let (target_idx, pivot) = self.plan_page_edit_pivot(fs_idx);
        if !self.raw_edit_target_entry_allowed(target_idx) {
            return;
        }
        self.cancel_original_material_entry_intents(None);
        if let Some(pivot) = pivot {
            self.export_crop_spread_ctx = Some(pivot);
            self.enter_page_edit_single_view(target_idx);
        }
        self.export_crop_mode = true;
    }

    /// 未確定のドラッグを取り消し、モードを終了して spread を復元する。
    pub(crate) fn reset_export_crop_mode(&mut self) {
        self.leave_export_crop_mode(OverlayDragEnd::ModeExit);
    }

    fn execute_export_crop(&mut self, ctx: &egui::Context, fs_idx: usize) {
        self.leave_export_crop_mode(OverlayDragEnd::Execute);
        self.open_export_dialog_for_current(ctx, fs_idx);
    }

    fn leave_export_crop_mode(&mut self, end: OverlayDragEnd) {
        self.finish_export_crop_drag(end);
        self.cancel_original_material_entry_intents(None);
        self.export_crop_mode = false;
        let pivot = self.export_crop_spread_ctx.take();
        self.leave_page_edit_single_view(pivot);
    }

    pub(crate) fn handle_export_crop_keys(
        &mut self,
        ctx: &egui::Context,
        fs_idx: usize,
    ) -> crate::ui_fullscreen::FsKeyAction {
        let action = crate::ui_fullscreen::FsKeyAction {
            page_transition: Default::default(),
            close: false,
            close_to_page_list: false,
            page_nav: crate::ui_fullscreen::FsPageNav::None,
            ctrl_nav: None,
            sibling_nav: None,
            mouse_nav: None,
            jump_to: None,
            music_ctrl_wheel_gate_reached: false,
        };
        if !self.ime_input_active(ctx) && self.consume_context_shortcuts_help_key(ctx) {
            self.show_context_shortcuts_help = true;
            return action;
        }
        if ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape)) {
            if !self.finish_export_crop_drag(OverlayDragEnd::Escape) {
                self.reset_export_crop_mode();
            }
            return action;
        }
        self.service_export_crop_drag(ctx, fs_idx, &Default::default());
        if self.keymap.consume_action(ctx, KeyAction::CropExecute) {
            self.execute_export_crop(ctx, fs_idx);
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

        egui::Area::new(egui::Id::unique("export_crop_panel"))
            .order(egui::Order::Foreground)
            .fixed_pos(panel_pos)
            .show(ctx, |ui| {
                ui.interact(
                    egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(PANEL_W + 12.0, panel_h + 12.0),
                    ),
                    egui::Id::unique("export_crop_panel_click_sink"),
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
                        crate::os_theme::apply_dark_ui(ui);

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
        let aspect_response = egui::ComboBox::from_id_salt("export_crop_aspect")
            .selected_text(aspect_mode.label())
            .show_ui(ui, |ui| {
                for mode in CropAspectMode::ALL {
                    ui.selectable_value(&mut aspect_mode, mode, mode.label());
                }
            });
        crate::ui_helpers::set_accessibility_label(&aspect_response.response, "切り取りの縦横比");
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
                    Some(CropSettings::authored(rect, aspect_mode, image_size)),
                    image_size,
                );
            } else if let Some(mut settings) = current {
                // 自由 / 現在比率: 既存 crop の比率モードだけ更新し rect は維持する。
                settings.aspect_mode = aspect_mode;
                self.set_export_crop_for_idx(fs_idx, Some(settings), image_size);
            }
        }

        let mut settings = current.unwrap_or_else(|| {
            CropSettings::authored(
                CropRect::full(image_size[0], image_size[1]),
                aspect_mode,
                image_size,
            )
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
                    Some(CropSettings::authored(rect, aspect_mode, image_size)),
                    image_size,
                );
            }
            if ui.add_enabled(active, egui::Button::new("解除")).clicked() {
                self.set_export_crop_for_idx(fs_idx, None, image_size);
            }
        });
        let auto_enabled = self.source_dims_for_idx(fs_idx).is_some();
        let auto_resp = ui.add_enabled(
            auto_enabled,
            egui::Button::new("自動クロップ").min_size(egui::vec2(ui.available_width(), 24.0)),
        );
        if auto_resp.clicked() {
            if self.apply_auto_export_crop_for_idx(fs_idx) {
                self.show_feedback_toast("[自動クロップ]".to_string());
            } else if self.current_raw_source_pixels(fs_idx).is_some() {
                self.show_feedback_warning("[余白を検出できません]".to_string());
            }
        }
        auto_resp.on_hover_text("四辺の単色余白を検出して切り取り範囲に設定します");

        let mut x = settings.rect.min_x.round() as i32;
        let mut y = settings.rect.min_y.round() as i32;
        let mut w = settings.rect.width().round() as i32;
        let mut h = settings.rect.height().round() as i32;
        let before = (x, y, w, h);
        ui.horizontal(|ui| {
            let label = ui.label("X");
            ui.add(egui::DragValue::new(&mut x).range(0..=image_size[0].saturating_sub(1) as i32))
                .labelled_by(label.id);
            let label = ui.label("Y");
            ui.add(egui::DragValue::new(&mut y).range(0..=image_size[1].saturating_sub(1) as i32))
                .labelled_by(label.id);
        });
        ui.horizontal(|ui| {
            let label = ui.label("W");
            ui.add(egui::DragValue::new(&mut w).range(1..=image_size[0].max(1) as i32))
                .labelled_by(label.id);
            let label = ui.label("H");
            ui.add(egui::DragValue::new(&mut h).range(1..=image_size[1].max(1) as i32))
                .labelled_by(label.id);
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
                Some(CropSettings::authored(rect, aspect_mode, image_size)),
                image_size,
            );
        }
    }

    pub(crate) fn apply_auto_export_crop_for_idx(&mut self, fs_idx: usize) -> bool {
        if !self
            .ensure_original_material_for_entry(fs_idx, crate::app::OriginalMaterialEntry::AutoCrop)
        {
            return false;
        }
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
            Some(CropSettings::authored(
                rect,
                CropAspectMode::Free,
                image_size,
            )),
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
        transform: &DisplayedImageTransform,
        fs_idx: usize,
        image_size: [usize; 2],
        pointer_allowed: bool,
        front: crate::ui_fullscreen::FsFrontPointerFrameInput,
    ) -> bool {
        self.service_export_crop_drag(ui.ctx(), fs_idx, &front);
        let current = self.export_crop_for_idx(fs_idx, image_size);
        if !self.export_crop_mode && current.is_none() {
            return false;
        }
        let settings = current.unwrap_or_else(|| {
            CropSettings::authored(
                CropRect::full(image_size[0], image_size[1]),
                self.export_crop_aspect_mode,
                image_size,
            )
        });
        let crop_corners = crop_corners(transform, image_size, settings.rect);
        let painter = ui.painter().with_clip_rect(transform.viewport_rect);
        paint_outside_crop(&painter, transform, image_size, settings.rect);
        let mut outline = crop_corners.to_vec();
        outline.push(crop_corners[0]);
        painter.add(egui::Shape::line(
            outline.clone(),
            egui::Stroke::new(3.0, egui::Color32::BLACK),
        ));
        painter.add(egui::Shape::line(
            outline,
            egui::Stroke::new(2.0, egui::Color32::from_rgb(255, 230, 100)),
        ));

        if !self.export_crop_mode {
            return false;
        }

        let handle_bounds = transform.viewport_rect.shrink(14.0);
        let handles = handle_points(transform, image_size, settings.rect);
        for (_, center) in handles {
            let center = clamp_pos_to_rect(center, handle_bounds);
            painter.circle_filled(center, 5.5, egui::Color32::from_rgb(255, 245, 180));
            painter.circle_stroke(
                center,
                5.5,
                egui::Stroke::new(1.5, egui::Color32::from_rgb(30, 20, 0)),
            );
        }

        let drag_in_progress =
            self.export_crop_drag.is_some() || self.export_crop_create_drag.is_some();
        if !crop_overlay_keeps_the_pointer(
            pointer_allowed && !front.owns(egui::PointerButton::Primary),
            drag_in_progress,
            ui.input(|i| i.pointer.primary_down()),
        ) {
            return false;
        }

        self.handle_export_crop_pointer(ui, transform, fs_idx, image_size, settings, handles, front)
    }

    fn handle_export_crop_pointer(
        &mut self,
        ui: &mut egui::Ui,
        transform: &DisplayedImageTransform,
        fs_idx: usize,
        image_size: [usize; 2],
        settings: CropSettings,
        handles: [(CropHandle, egui::Pos2); 8],
        front: crate::ui_fullscreen::FsFrontPointerFrameInput,
    ) -> bool {
        let (space_pan_held, force_create_held) = crate::keyboard_input::focused_key_state_permit(
            ui.ctx(),
        )
        .map_or((false, false), |permit| {
            (
                self.keymap.crop_space_pan_held(ui.ctx(), permit),
                self.keymap
                    .modifier_held_action(ui.ctx(), permit, KeyAction::CropForceCreateHold),
            )
        });
        self.handle_export_crop_pointer_with_holds_and_front(
            ui,
            transform,
            fs_idx,
            image_size,
            settings,
            handles,
            space_pan_held,
            force_create_held,
            front,
        )
    }

    #[allow(clippy::too_many_arguments)]
    #[cfg(test)]
    fn handle_export_crop_pointer_with_holds(
        &mut self,
        ui: &mut egui::Ui,
        transform: &DisplayedImageTransform,
        fs_idx: usize,
        image_size: [usize; 2],
        settings: CropSettings,
        handles: [(CropHandle, egui::Pos2); 8],
        space_pan_held: bool,
        force_create_held: bool,
    ) -> bool {
        self.handle_export_crop_pointer_with_holds_and_front(
            ui,
            transform,
            fs_idx,
            image_size,
            settings,
            handles,
            space_pan_held,
            force_create_held,
            Default::default(),
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn handle_export_crop_pointer_with_holds_and_front(
        &mut self,
        ui: &mut egui::Ui,
        transform: &DisplayedImageTransform,
        fs_idx: usize,
        image_size: [usize; 2],
        settings: CropSettings,
        handles: [(CropHandle, egui::Pos2); 8],
        space_pan_held: bool,
        force_create_held: bool,
        front: crate::ui_fullscreen::FsFrontPointerFrameInput,
    ) -> bool {
        self.service_export_crop_drag(ui.ctx(), fs_idx, &front);
        let (primary_pressed, primary_down, primary_released, press_origin, hover_pos, total_delta) =
            ui.input(|i| {
                (
                    !front.owns(egui::PointerButton::Primary) && i.pointer.primary_pressed(),
                    !front.owns(egui::PointerButton::Primary) && i.pointer.primary_down(),
                    !front.owns_release(egui::PointerButton::Primary)
                        && i.pointer.primary_released(),
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

        let target_at = |pos: egui::Pos2| -> Option<CropHandle> {
            for (handle, center) in handles {
                let center = clamp_pos_to_rect(center, transform.viewport_rect.shrink(14.0));
                if (pos - center).length() <= HANDLE_HIT {
                    return Some(handle);
                }
            }
            let image = screen_to_image(transform, image_size, pos);
            if image[0] >= settings.rect.min_x
                && image[0] <= settings.rect.max_x
                && image[1] >= settings.rect.min_y
                && image[1] <= settings.rect.max_y
            {
                Some(CropHandle::Body)
            } else {
                None
            }
        };
        let pointer_start = press_origin.map_or(ExportCropPointerStart::None, |origin| {
            classify_export_crop_pointer_start(
                primary_pressed,
                space_pan_held,
                force_create_held,
                transform.contains_screen(origin),
                target_at(origin),
            )
        });
        if !crop_drag_in_progress
            && self.handle_overlay_space_pan_drag(
                ui.ctx(),
                space_pan_held,
                pointer_pos.is_some_and(|pos| transform.contains_screen(pos)),
                primary_pressed,
                primary_down,
                primary_released,
                pointer_pos,
            )
        {
            return true;
        }

        if !crop_drag_in_progress && let Some(origin) = press_origin {
            match pointer_start {
                ExportCropPointerStart::Edit(handle) => {
                    let Some(before) =
                        self.begin_export_crop_drag_snapshot(ui.ctx(), fs_idx, image_size)
                    else {
                        return false;
                    };
                    self.export_crop_drag = Some(ExportCropDrag {
                        handle,
                        base: settings.rect,
                        before,
                    });
                    self.export_crop_create_drag = None;
                }
                ExportCropPointerStart::ForceCreate | ExportCropPointerStart::Create => {
                    let Some(before) =
                        self.begin_export_crop_drag_snapshot(ui.ctx(), fs_idx, image_size)
                    else {
                        return false;
                    };
                    self.export_crop_drag = None;
                    self.export_crop_create_drag = Some(ExportCropCreateDrag {
                        start: screen_to_image(transform, image_size, origin),
                        before,
                    });
                }
                ExportCropPointerStart::None | ExportCropPointerStart::Pan => {}
            }
        }

        let mut used = false;
        if primary_down {
            if let Some((handle, base)) = self
                .export_crop_drag
                .as_ref()
                .map(|drag| (drag.handle, drag.base))
            {
                let origin = press_origin.unwrap_or_else(|| transform.viewport_rect.center());
                let start = screen_to_image(transform, image_size, origin);
                let end = screen_to_image(transform, image_size, origin + total_delta);
                let rect = base.dragged(
                    handle,
                    end[0] - start[0],
                    end[1] - start[1],
                    image_size[0],
                    image_size[1],
                    self.export_crop_aspect_ratio(settings, image_size),
                );
                self.set_export_crop_for_idx_memory_only(
                    fs_idx,
                    Some(CropSettings::authored(
                        rect,
                        settings.aspect_mode,
                        image_size,
                    )),
                    image_size,
                );
                ui.ctx().set_cursor_icon(if handle == CropHandle::Body {
                    egui::CursorIcon::Grabbing
                } else {
                    crop_handle_cursor(handle)
                });
                used = true;
            } else if let (Some(start), Some(cur), Some(origin)) = (
                self.export_crop_create_drag.as_ref().map(|drag| drag.start),
                hover_pos,
                press_origin,
            ) {
                if (cur - origin).length() >= CREATE_DRAG_THRESHOLD {
                    let rect = crate::export_crop::crop_from_points(
                        start,
                        screen_to_image(transform, image_size, cur),
                        image_size[0],
                        image_size[1],
                        self.export_crop_aspect_ratio(settings, image_size),
                    );
                    self.set_export_crop_for_idx_memory_only(
                        fs_idx,
                        Some(CropSettings::authored(
                            rect,
                            settings.aspect_mode,
                            image_size,
                        )),
                        image_size,
                    );
                    ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
                    used = true;
                }
            }
        }

        if primary_released {
            self.finish_export_crop_drag(OverlayDragEnd::Released);
        }

        if !used && let Some(pos) = hover_pos {
            if force_create_held && transform.contains_screen(pos) {
                ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
                used = true;
            } else if let Some(handle) = target_at(pos) {
                ui.ctx().set_cursor_icon(crop_handle_cursor(handle));
                used = true;
            } else if transform.contains_screen(pos) {
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

    fn test_transform(rect: egui::Rect, image_size: [usize; 2]) -> DisplayedImageTransform {
        DisplayedImageTransform::resolve(
            crate::displayed_image_transform::DisplayedImageTransformInput {
                pixel_fit: crate::displayed_image_transform::RectPixelFit::Texels,
                page_idx: 0,
                viewport_rect: rect,
                source_size: egui::vec2(image_size[0] as f32, image_size[1] as f32),
                texture_size: egui::vec2(image_size[0] as f32, image_size[1] as f32),
                rotation: crate::rotation_db::Rotation::None,
                free_rotation_rad: 0.0,
                content_bbox: None,
                fit_mode: crate::fullscreen_specified_scale::FullscreenFitInput::legacy(
                    crate::settings::FullscreenFitMode::Page,
                ),
                fit_scale_limits:
                    crate::displayed_image_transform::FullscreenFitScaleLimits::default(),
                pixels_per_point: 1.0,
                placement: crate::displayed_image_transform::ResolvedDisplayPlacement::Normal {
                    zoom_pan: None,
                },
            },
        )
        .expect("test crop transform")
    }

    fn run_crop_pointer_frame(
        app: &mut App,
        ctx: &egui::Context,
        transform: &DisplayedImageTransform,
        image_size: [usize; 2],
        settings: CropSettings,
        handles: [(CropHandle, egui::Pos2); 8],
        events: Vec<egui::Event>,
        space_pan_held: bool,
        force_create_held: bool,
    ) -> bool {
        let mut used = false;
        let _ = ctx
            .run_pass(
                egui::RawInput {
                    screen_rect: Some(transform.viewport_rect),
                    events,
                    ..Default::default()
                },
                |ctx| {
                    ctx.root_ui(|root_ui| {
                        egui::CentralPanel::default().show(root_ui, |ui| {
                            used = app.handle_export_crop_pointer_with_holds(
                                ui,
                                transform,
                                0,
                                image_size,
                                settings,
                                handles,
                                space_pan_held,
                                force_create_held,
                            );
                        })
                    });
                },
            )
            .drop_without_applying_deltas();
        used
    }

    /// A drag that is already running survives the pointer wandering onto the tool panel.
    ///
    /// The panel sits in the letterbox beside a portrait page, so dragging a new box out
    /// past the left edge of the image lands on it. Dropping the drag there froze the box
    /// permanently, since only a press can begin one.
    #[test]
    fn a_running_crop_drag_keeps_the_pointer_over_the_panel() {
        assert!(crop_overlay_keeps_the_pointer(false, true, true));
    }

    /// Everything else still yields: no drag to protect, or a stale one whose button is up.
    #[test]
    fn a_disallowed_pointer_yields_when_no_drag_is_running() {
        assert!(!crop_overlay_keeps_the_pointer(false, false, true));
        assert!(!crop_overlay_keeps_the_pointer(false, true, false));
        assert!(!crop_overlay_keeps_the_pointer(false, false, false));
    }

    /// An allowed pointer never depended on a drag being under way.
    #[test]
    fn an_allowed_pointer_is_kept_whatever_the_drag_state() {
        for drag_in_progress in [false, true] {
            for primary_down in [false, true] {
                assert!(crop_overlay_keeps_the_pointer(
                    true,
                    drag_in_progress,
                    primary_down
                ));
            }
        }
    }

    #[test]
    fn crop_pointer_start_priority_is_pan_then_force_then_existing_target() {
        for target in [
            CropHandle::Body,
            CropHandle::NorthWest,
            CropHandle::North,
            CropHandle::NorthEast,
            CropHandle::East,
            CropHandle::SouthEast,
            CropHandle::South,
            CropHandle::SouthWest,
            CropHandle::West,
        ] {
            assert_eq!(
                classify_export_crop_pointer_start(true, true, true, true, Some(target)),
                ExportCropPointerStart::Pan
            );
            assert_eq!(
                classify_export_crop_pointer_start(true, false, true, true, Some(target)),
                ExportCropPointerStart::ForceCreate
            );
            assert_eq!(
                classify_export_crop_pointer_start(true, false, false, true, Some(target)),
                ExportCropPointerStart::Edit(target)
            );
        }
    }

    #[test]
    fn force_create_never_starts_outside_the_image() {
        assert_eq!(
            classify_export_crop_pointer_start(true, false, true, false, None),
            ExportCropPointerStart::None
        );
        assert_eq!(
            classify_export_crop_pointer_start(true, false, true, false, Some(CropHandle::East),),
            ExportCropPointerStart::Edit(CropHandle::East),
            "an existing handle outside the image keeps its normal edit behavior"
        );
        assert_eq!(
            classify_export_crop_pointer_start(true, false, true, true, None),
            ExportCropPointerStart::ForceCreate
        );
    }

    #[test]
    fn crop_pointer_start_requires_the_initial_primary_press() {
        assert_eq!(
            classify_export_crop_pointer_start(false, true, true, true, Some(CropHandle::Body),),
            ExportCropPointerStart::None,
            "releasing Space or changing the force modifier during the same primary hold must not reclassify the owner"
        );
        assert_eq!(
            classify_export_crop_pointer_start(true, false, false, true, None),
            ExportCropPointerStart::Create,
            "the existing empty-area create path remains available"
        );
    }

    #[test]
    fn production_crop_pointer_path_latches_force_create_until_release() {
        let mut app = crate::app::setup_app_for_test();
        let image_path = app.tmp.path().join("crop-pointer.jpg");
        std::fs::write(&image_path, b"test").unwrap();
        app.items
            .push(crate::grid_item::GridItem::Image(image_path.clone()));
        app.fullscreen_idx = Some(0);

        let ctx = egui::Context::default();
        app.export_crop_mode = true;
        let image_size = [100, 100];
        let transform = test_transform(
            egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(400.0, 400.0)),
            image_size,
        );
        let initial = CropSettings::authored(
            CropRect::full(image_size[0], image_size[1]),
            CropAspectMode::Free,
            image_size,
        );
        let handles = handle_points(&transform, image_size, initial.rect);
        let start = transform.viewport_rect.center();
        let end = start + egui::vec2(80.0, 60.0);
        let pointer_button = |pos, pressed| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };

        assert!(run_crop_pointer_frame(
            &mut app,
            &ctx,
            &transform,
            image_size,
            initial,
            handles,
            vec![
                egui::Event::PointerMoved(start),
                pointer_button(start, true)
            ],
            false,
            true,
        ));
        assert!(app.export_crop_drag.is_none());
        assert!(app.export_crop_create_drag.is_some());

        assert!(run_crop_pointer_frame(
            &mut app,
            &ctx,
            &transform,
            image_size,
            initial,
            handles,
            vec![egui::Event::PointerMoved(end)],
            false,
            false,
        ));
        assert!(
            app.export_crop_create_drag.is_some(),
            "releasing the force modifier during the same primary hold must keep the create owner"
        );
        let authored = app
            .export_crop_for_idx(0, image_size)
            .expect("the held production drag must author a new crop");
        assert!(!authored.is_full(image_size[0], image_size[1]));

        assert!(run_crop_pointer_frame(
            &mut app,
            &ctx,
            &transform,
            image_size,
            authored,
            handle_points(&transform, image_size, authored.rect),
            vec![egui::Event::PointerMoved(end), pointer_button(end, false)],
            false,
            false,
        ));
        assert!(app.export_crop_drag.is_none());
        assert!(app.export_crop_create_drag.is_none());
        assert_eq!(app.export_crop_for_idx(0, image_size), Some(authored));
    }

    #[test]
    fn production_crop_pointer_path_does_not_reclassify_after_space_pan_ends() {
        let mut app = crate::app::setup_app_for_test();
        let ctx = egui::Context::default();
        let image_size = [100, 100];
        let transform = test_transform(
            egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(400.0, 400.0)),
            image_size,
        );
        let settings = CropSettings::authored(
            CropRect::full(image_size[0], image_size[1]),
            CropAspectMode::Free,
            image_size,
        );
        let handles = handle_points(&transform, image_size, settings.rect);
        let start = transform.viewport_rect.center();
        let moved = start + egui::vec2(40.0, 20.0);
        let pointer_button = |pos, pressed| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };

        run_crop_pointer_frame(
            &mut app,
            &ctx,
            &transform,
            image_size,
            settings,
            handles,
            vec![
                egui::Event::PointerMoved(start),
                pointer_button(start, true),
            ],
            true,
            true,
        );
        assert!(app.fs_pan_drag_start.is_some());
        assert!(app.export_crop_drag.is_none());
        assert!(app.export_crop_create_drag.is_none());

        run_crop_pointer_frame(
            &mut app,
            &ctx,
            &transform,
            image_size,
            settings,
            handles,
            vec![egui::Event::PointerMoved(moved)],
            false,
            true,
        );
        assert!(app.fs_pan_drag_start.is_none());
        assert!(app.export_crop_drag.is_none());
        assert!(
            app.export_crop_create_drag.is_none(),
            "the same primary hold must not turn into force-create after Space is released"
        );

        run_crop_pointer_frame(
            &mut app,
            &ctx,
            &transform,
            image_size,
            settings,
            handles,
            vec![pointer_button(moved, false)],
            false,
            true,
        );
        assert!(app.fs_pan_drag_start.is_none());
        assert!(app.export_crop_drag.is_none());
        assert!(app.export_crop_create_drag.is_none());
    }

    #[test]
    fn crop_drag_escape_restores_memory_without_writing_db() {
        for existing in [false, true] {
            let mut app = crate::app::setup_app_for_test();
            let path = app.tmp.path().join("crop-cancel.jpg");
            std::fs::write(&path, b"test").unwrap();
            app.items.push(crate::grid_item::GridItem::Image(path));
            app.fullscreen_idx = Some(0);
            app.export_crop_mode = true;
            let image_size = [100, 100];
            let initial = existing.then(|| {
                CropSettings::authored(
                    centered_default_crop(image_size),
                    CropAspectMode::Free,
                    image_size,
                )
            });
            app.set_export_crop_for_idx(0, initial, image_size);
            let key = app.page_path_key(0).unwrap();
            let ctx = egui::Context::default();
            let transform = test_transform(
                egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(400.0, 400.0)),
                image_size,
            );
            let settings = initial.unwrap_or_else(|| {
                CropSettings::authored(CropRect::full(100, 100), CropAspectMode::Free, image_size)
            });
            let start = transform.viewport_rect.center();
            run_crop_pointer_frame(
                &mut app,
                &ctx,
                &transform,
                image_size,
                settings,
                handle_points(&transform, image_size, settings.rect),
                vec![
                    egui::Event::PointerMoved(start),
                    egui::Event::PointerButton {
                        pos: start,
                        button: egui::PointerButton::Primary,
                        pressed: true,
                        modifiers: egui::Modifiers::NONE,
                    },
                ],
                false,
                true,
            );
            run_crop_pointer_frame(
                &mut app,
                &ctx,
                &transform,
                image_size,
                settings,
                handle_points(&transform, image_size, settings.rect),
                vec![egui::Event::PointerMoved(start + egui::vec2(80.0, 60.0))],
                false,
                true,
            );
            assert_ne!(app.export_crop_for_idx(0, image_size), initial);
            let _ = ctx
                .run_pass(
                    egui::RawInput {
                        events: vec![egui::Event::Key {
                            key: egui::Key::Escape,
                            physical_key: None,
                            pressed: true,
                            repeat: false,
                            modifiers: egui::Modifiers::NONE,
                        }],
                        ..Default::default()
                    },
                    |ctx| {
                        app.handle_export_crop_keys(ctx, 0);
                    },
                )
                .drop_without_applying_deltas();
            assert_eq!(app.export_crop_db.as_ref().unwrap().get(&key), initial);
            assert_eq!(
                app.export_crop_for_idx(0, image_size),
                initial,
                "Escape must restore the pre-drag crop, including no crop"
            );
            assert!(
                app.export_crop_mode,
                "cancelling the drag keeps the tool open"
            );
        }
    }

    fn start_test_crop_drag(
        existing: bool,
        create: bool,
    ) -> (
        crate::app::AppTestEnvForTest,
        egui::Context,
        DisplayedImageTransform,
        Option<CropSettings>,
        CropSettings,
    ) {
        let mut app = crate::app::setup_app_for_test();
        for name in ["crop-a.jpg", "crop-b.jpg"] {
            let path = app.tmp.path().join(name);
            std::fs::write(&path, b"test").unwrap();
            app.items.push(crate::grid_item::GridItem::Image(path));
            app.image_metas.push(None);
            app.thumbnails
                .push(crate::grid_item::ThumbnailState::Pending);
        }
        app.fullscreen_idx = Some(0);
        app.export_crop_mode = true;
        let size = [100, 100];
        let initial = existing.then(|| {
            CropSettings::authored(centered_default_crop(size), CropAspectMode::Free, size)
        });
        app.set_export_crop_for_idx(0, initial, size);
        let settings = initial.unwrap_or_else(|| {
            CropSettings::authored(CropRect::full(100, 100), CropAspectMode::Free, size)
        });
        let ctx = egui::Context::default();
        let transform = test_transform(
            egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(400.0, 400.0)),
            size,
        );
        let start = transform.viewport_rect.center();
        let handles = handle_points(&transform, size, settings.rect);
        run_crop_pointer_frame(
            &mut app,
            &ctx,
            &transform,
            size,
            settings,
            handles,
            vec![
                egui::Event::PointerMoved(start),
                egui::Event::PointerButton {
                    pos: start,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
            false,
            create,
        );
        run_crop_pointer_frame(
            &mut app,
            &ctx,
            &transform,
            size,
            settings,
            handles,
            vec![egui::Event::PointerMoved(start + egui::vec2(80.0, 60.0))],
            false,
            create,
        );
        let draft = app.export_crop_for_idx(0, size).unwrap();
        assert_ne!(Some(draft), initial);
        (app, ctx, transform, initial, draft)
    }

    #[test]
    fn crop_drag_all_terminal_routes_preserve_memory_db_consistency() {
        for (existing, create) in [(false, true), (true, true), (true, false)] {
            for route in [
                "release",
                "missing_release",
                "escape",
                "focus",
                "right",
                "reset",
                "preview",
                "page",
                "close",
                "install",
                "remove",
            ] {
                let (mut app, ctx, transform, initial, draft) =
                    start_test_crop_drag(existing, create);
                let key = app.page_path_key(0).unwrap();
                let mut events = Vec::new();
                match route {
                    "release" => events.push(egui::Event::PointerButton {
                        pos: transform.viewport_rect.center(),
                        button: egui::PointerButton::Primary,
                        pressed: false,
                        modifiers: egui::Modifiers::NONE,
                    }),
                    "missing_release" => ctx.data_mut(|data| {
                        data.insert_temp(egui::Id::unique("overlay_test_primary_level"), false);
                    }),
                    "focus" => events.push(egui::Event::WindowFocused(false)),
                    "right" => events.push(egui::Event::PointerButton {
                        pos: transform.viewport_rect.center(),
                        button: egui::PointerButton::Secondary,
                        pressed: true,
                        modifiers: egui::Modifiers::NONE,
                    }),
                    "escape" => events.push(egui::Event::Key {
                        key: egui::Key::Escape,
                        physical_key: None,
                        pressed: true,
                        repeat: false,
                        modifiers: egui::Modifiers::NONE,
                    }),
                    "reset" => app.reset_export_crop_mode(),
                    "preview" => app.cache_current_edit_preview_if_ready(),
                    "page" => app.open_fullscreen(1, crate::app::HistoryTrigger::UserChosen),
                    "close" => app.close_fullscreen(),
                    "install" => {
                        let items = app.items.clone();
                        app.install_new_items(items, vec![None; 2]);
                    }
                    "remove" => app.remove_items_batch(&[1]),
                    _ => unreachable!(),
                }
                if matches!(
                    route,
                    "release" | "missing_release" | "focus" | "right" | "escape"
                ) {
                    let _ = ctx
                        .run_pass(
                            egui::RawInput {
                                events,
                                ..Default::default()
                            },
                            |ctx| {
                                app.handle_export_crop_keys(ctx, 0);
                            },
                        )
                        .drop_without_applying_deltas();
                }
                let expected = if matches!(route, "release" | "missing_release") {
                    Some(draft)
                } else {
                    initial
                };
                assert_eq!(
                    app.export_crop_db.as_ref().unwrap().get(&key),
                    expected,
                    "DB route={route}"
                );
                assert_eq!(
                    app.export_crop_page_settings.get(&0).copied(),
                    expected,
                    "memory route={route}"
                );
                assert_eq!(
                    app.export_crop_pages.contains(&0),
                    expected.is_some(),
                    "presence route={route}"
                );
                assert_eq!(
                    app.export_crop_page_keys.contains(&key),
                    expected.is_some(),
                    "keys route={route}"
                );
                assert!(
                    app.export_crop_drag.is_none() && app.export_crop_create_drag.is_none(),
                    "terminal route={route}"
                );
                if route == "escape" {
                    assert!(app.export_crop_mode);
                }
            }
        }
    }

    #[test]
    fn crop_execute_held_drag_exports_and_persists_current_settings() {
        let _key_input_guard = crate::key_input::lock_test_input();
        for (existing, create) in [(false, true), (true, true), (true, false)] {
            let (mut app, ctx, _, initial, draft) = start_test_crop_drag(existing, create);
            let key = app.page_path_key(0).unwrap();
            let image = egui::ColorImage::filled([100, 100], egui::Color32::WHITE);
            app.fs_cache.insert(
                0,
                crate::fs_animation::FsCacheEntry::Static {
                    pdf_metadata: None,
                    pdf_rerender_failures: Vec::new(),
                    tex: ctx.load_texture("crop-execute", image.clone(), Default::default()),
                    pixels: std::sync::Arc::new(image),
                    source_dims: Some([100, 100]),
                    load_seq: 0,
                    animation: crate::fs_animation::StaticAnimationState::Still,
                },
            );
            assert_eq!(app.export_crop_db.as_ref().unwrap().get(&key), initial);
            ctx.begin_pass(egui::RawInput {
                events: vec![
                    egui::Event::ModifiersChanged(egui::Modifiers::CTRL),
                    egui::Event::Key {
                        key: egui::Key::E,
                        physical_key: None,
                        pressed: true,
                        repeat: false,
                        modifiers: egui::Modifiers::CTRL,
                    },
                ],
                ..Default::default()
            });
            assert!(ctx.input(|input| input.pointer.primary_down()));
            app.keyboard_owner_for_pass(&ctx);
            app.handle_export_crop_keys(&ctx, 0);
            let _ = ctx.end_pass().drop_without_applying_deltas();
            assert_eq!(
                app.export_crop_db.as_ref().unwrap().get(&key),
                Some(draft),
                "execute must persist the held drag, including creation from no crop"
            );
            assert_eq!(app.export_crop_page_settings.get(&0), Some(&draft));
            assert!(!app.export_crop_mode);
            assert!(app.export_crop_drag.is_none() && app.export_crop_create_drag.is_none());
            let dialog = app.export_dialog.as_ref().expect("export dialog must open");
            let crate::export_dialog::ExportComposite::Single(page) = &dialog.composite else {
                panic!("single-page export expected");
            };
            assert_eq!(page.edits.export_crop, Some(draft));
        }
    }

    #[test]
    fn crop_drag_cancel_restores_original_authoring_dimensions() {
        let (mut app, ctx, _, initial, _) = start_test_crop_drag(true, false);
        app.finish_export_crop_drag(OverlayDragEnd::Escape);
        let original = initial.unwrap().scaled_to([200, 200]);
        app.set_export_crop_for_idx(0, Some(original), [200, 200]);
        let before = app
            .begin_export_crop_drag_snapshot(&ctx, 0, [100, 100])
            .unwrap();
        app.export_crop_create_drag = Some(ExportCropCreateDrag {
            start: [10.0, 10.0],
            before,
        });
        app.set_export_crop_for_idx_memory_only(0, initial, [100, 100]);
        let _ = ctx
            .run_pass(
                egui::RawInput {
                    events: vec![egui::Event::Key {
                        key: egui::Key::Escape,
                        physical_key: None,
                        pressed: true,
                        repeat: false,
                        modifiers: egui::Modifiers::NONE,
                    }],
                    ..Default::default()
                },
                |ctx| {
                    app.handle_export_crop_keys(ctx, 0);
                },
            )
            .drop_without_applying_deltas();
        assert_eq!(app.export_crop_page_settings.get(&0), Some(&original));
        assert_eq!(
            app.export_crop_db
                .as_ref()
                .unwrap()
                .get(&app.page_path_key(0).unwrap()),
            Some(original)
        );
    }

    #[test]
    fn crop_drag_missing_release_then_new_press_commits_once_and_starts_a_new_cancel_scope() {
        let (mut app, ctx, transform, _, committed) = start_test_crop_drag(true, true);
        let size = [100, 100];
        let handles = handle_points(&transform, size, committed.rect);
        let start = transform.viewport_rect.center() + egui::vec2(20.0, 20.0);
        let _ = ctx
            .run_pass(
                egui::RawInput {
                    screen_rect: Some(transform.viewport_rect),
                    events: vec![
                        egui::Event::PointerMoved(start),
                        egui::Event::PointerButton {
                            pos: start,
                            button: egui::PointerButton::Primary,
                            pressed: true,
                            modifiers: egui::Modifiers::NONE,
                        },
                    ],
                    ..Default::default()
                },
                |ctx| {
                    ctx.root_ui(|root| {
                        egui::CentralPanel::default().show(root, |ui| {
                            for _ in 0..2 {
                                app.handle_export_crop_pointer_with_holds(
                                    ui, &transform, 0, size, committed, handles, false, true,
                                );
                                assert!(
                                    app.export_crop_create_drag.is_some(),
                                    "a second pass over the same press must keep its owner"
                                );
                            }
                        });
                    });
                },
            )
            .drop_without_applying_deltas();
        let key = app.page_path_key(0).unwrap();
        assert_eq!(
            app.export_crop_db.as_ref().unwrap().get(&key),
            Some(committed)
        );
        run_crop_pointer_frame(
            &mut app,
            &ctx,
            &transform,
            size,
            committed,
            handles,
            vec![egui::Event::PointerMoved(start + egui::vec2(40.0, 40.0))],
            false,
            true,
        );
        assert_ne!(app.export_crop_for_idx(0, size), Some(committed));
        let _ = ctx
            .run_pass(
                egui::RawInput {
                    events: vec![egui::Event::Key {
                        key: egui::Key::Escape,
                        physical_key: None,
                        pressed: true,
                        repeat: false,
                        modifiers: egui::Modifiers::NONE,
                    }],
                    ..Default::default()
                },
                |ctx| {
                    app.handle_export_crop_keys(ctx, 0);
                },
            )
            .drop_without_applying_deltas();
        assert_eq!(app.export_crop_for_idx(0, size), Some(committed));
        assert_eq!(
            app.export_crop_db.as_ref().unwrap().get(&key),
            Some(committed)
        );
    }

    #[test]
    fn crop_drag_active_touch_does_not_finish_from_mouse_button_level() {
        let (mut app, ctx, transform, initial, draft) = start_test_crop_drag(true, true);
        let size = [100, 100];
        let key = app.page_path_key(0).unwrap();
        ctx.data_mut(|data| {
            data.insert_temp(egui::Id::unique("overlay_test_primary_level"), false);
        });
        let point = transform.viewport_rect.center() + egui::vec2(60.0, 40.0);
        run_crop_pointer_frame(
            &mut app,
            &ctx,
            &transform,
            size,
            draft,
            handle_points(&transform, size, draft.rect),
            vec![
                egui::Event::Touch {
                    device_id: egui::TouchDeviceId(1),
                    id: egui::TouchId(1),
                    phase: egui::TouchPhase::Start,
                    pos: point,
                    force: None,
                },
                egui::Event::PointerMoved(point),
            ],
            false,
            true,
        );
        assert!(
            app.export_crop_create_drag.is_some(),
            "an active touch must keep the logical primary drag"
        );
        assert_eq!(app.export_crop_db.as_ref().unwrap().get(&key), initial);
        app.finish_export_crop_drag(OverlayDragEnd::Escape);
        assert_eq!(app.export_crop_for_idx(0, size), initial);
        assert_eq!(app.export_crop_db.as_ref().unwrap().get(&key), initial);
    }

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
