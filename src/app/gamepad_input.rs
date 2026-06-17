use std::path::PathBuf;
use std::time::{Duration, Instant};

use eframe::egui;

use super::{AdjustSpreadTarget, App, FolderNavMode};
use crate::adjustment::PostFilter;
use crate::folder_pane::{FolderPaneCommand, FolderPaneTreeKey};
use crate::gamepad::{GamepadInputState, PadAxis, PadButton, PadEvent, WestReleaseOutcome};
use crate::grid_item::GridItem;
use crate::ring_shortcut::{
    MOUSE_FLICK_MOVE_THRESHOLD_PX, MouseBackForwardActionId, MouseFlickOutcome, MouseFlickState,
    PostFilterDrillState, RingActionId, RingDirection, RingPickerRowId, RingPickerState,
    RingShortcutContext, WheelPairActionId, mouse_flick_guide_delay, mouse_flick_menu_delay,
};
use crate::settings::{
    FullscreenFitMode, GridViewMode, ReadingDirection, ReadingFlow, SortOrder, SpreadMode,
    ThumbAspect, format_video_volume_db, step_video_volume_by_fader_key_step,
};
use crate::ui_main::AddressBarNav;
use crate::video::VideoContinuousMode;

const BUTTON_REPEAT_INTERVAL: Duration = Duration::from_millis(95);
const SHOULDER_REPEAT_INTERVAL: Duration = Duration::from_millis(260);
const STICK_STEP_INTERVAL: Duration = Duration::from_millis(110);
const TRIGGER_STEP_INTERVAL: Duration = Duration::from_millis(150);
const DEADZONE: f32 = 0.25;
const TRIGGER_THRESHOLD: f32 = 0.35;
const PAN_SPEED_PX_PER_SEC: f32 = 720.0;
const RIGHT_STICK_ZOOM_MULTIPLIER: f32 = 2.0;
const GAMEPAD_REPAINT_INTERVAL: Duration = Duration::from_millis(16);
const X_PICKER_HINT_TOAST_SECS: f32 = 4.0;

const GRID_PICKER_ROWS: &[RingPickerRowId] = &[
    RingPickerRowId::GridColumns,
    RingPickerRowId::GridSortOrder,
    RingPickerRowId::GridThumbAspect,
    RingPickerRowId::ItemRating,
    RingPickerRowId::ContainerRating,
];

const IMAGE_PICKER_ROWS: &[RingPickerRowId] = &[
    RingPickerRowId::SpreadMode,
    RingPickerRowId::ReadingFlow,
    RingPickerRowId::FitMode,
    RingPickerRowId::ItemRating,
    RingPickerRowId::ContainerRating,
    RingPickerRowId::PostFilter,
    RingPickerRowId::UpscaleModel,
];

const VIDEO_PICKER_ROWS: &[RingPickerRowId] = &[
    RingPickerRowId::VideoVolume,
    RingPickerRowId::VideoPlaybackSpeed,
    RingPickerRowId::VideoContinuousMode,
];

struct PostFilterGroup {
    label: &'static str,
    filters: &'static [PostFilter],
}

const POST_FILTER_GROUP_BASIC: &[PostFilter] = &[PostFilter::None, PostFilter::Nearest];
const POST_FILTER_GROUP_CRT: &[PostFilter] = &[
    PostFilter::CrtSimple,
    PostFilter::CrtFull,
    PostFilter::CrtArcade,
];
const POST_FILTER_GROUP_RETRO: &[PostFilter] = &[
    PostFilter::Dither1bit,
    PostFilter::GameBoy,
    PostFilter::Pc98,
    PostFilter::GameGear,
    PostFilter::Famicom,
    PostFilter::MegaDrive,
    PostFilter::Msx2Plus,
    PostFilter::Sfc,
];
const POST_FILTER_GROUP_COMBO: &[PostFilter] = &[
    PostFilter::ComboFamicomCrt,
    PostFilter::ComboPc98Crt,
    PostFilter::ComboMsx2PlusCrt,
    PostFilter::ComboMegaDriveCrt,
    PostFilter::ComboSfcCrt,
];
const POST_FILTER_GROUP_MONO_TONE: &[PostFilter] = &[
    PostFilter::Sepia,
    PostFilter::MonoNeutral,
    PostFilter::MonoCool,
    PostFilter::MonoWarm,
    PostFilter::WarmTone,
    PostFilter::CoolTone,
];
const POST_FILTER_GROUP_FILM: &[PostFilter] = &[
    PostFilter::TealOrange,
    PostFilter::KodakPortra,
    PostFilter::FujiVelvia,
    PostFilter::BleachBypass,
    PostFilter::CrossProcess,
    PostFilter::Vintage,
];
const POST_FILTER_GROUP_ANALOG: &[PostFilter] = &[
    PostFilter::FilmGrain,
    PostFilter::Vignette,
    PostFilter::LightLeak,
    PostFilter::SoftFocus,
];
const POST_FILTER_GROUP_DRAWING: &[PostFilter] = &[
    PostFilter::Halftone,
    PostFilter::OilPaint,
    PostFilter::Sketch,
];
const POST_FILTER_GROUP_PSEUDO_COLOR: &[PostFilter] =
    &[PostFilter::PseudoColor4, PostFilter::PseudoColorSkin];
const POST_FILTER_GROUP_UTILITY: &[PostFilter] = &[PostFilter::Sharpen];

const POST_FILTER_GROUPS: &[PostFilterGroup] = &[
    PostFilterGroup {
        label: "基本",
        filters: POST_FILTER_GROUP_BASIC,
    },
    PostFilterGroup {
        label: "CRT",
        filters: POST_FILTER_GROUP_CRT,
    },
    PostFilterGroup {
        label: "レトロ機",
        filters: POST_FILTER_GROUP_RETRO,
    },
    PostFilterGroup {
        label: "CRT × レトロ機",
        filters: POST_FILTER_GROUP_COMBO,
    },
    PostFilterGroup {
        label: "モノ・トーン",
        filters: POST_FILTER_GROUP_MONO_TONE,
    },
    PostFilterGroup {
        label: "シネマ・フィルム",
        filters: POST_FILTER_GROUP_FILM,
    },
    PostFilterGroup {
        label: "アナログフィルム",
        filters: POST_FILTER_GROUP_ANALOG,
    },
    PostFilterGroup {
        label: "描画風",
        filters: POST_FILTER_GROUP_DRAWING,
    },
    PostFilterGroup {
        label: "漫画 疑似カラー",
        filters: POST_FILTER_GROUP_PSEUDO_COLOR,
    },
    PostFilterGroup {
        label: "実用",
        filters: POST_FILTER_GROUP_UTILITY,
    },
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PadActionKind {
    Press,
    Repeat,
    Release,
}

#[derive(Clone, Copy, Debug)]
struct PadAction {
    button: PadButton,
    kind: PadActionKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PadDir {
    Up,
    Down,
    Left,
    Right,
}

/// mIV のいずれかのウィンドウ (メイン / フルスクリーン) が OS の前面ウィンドウかどうか。
/// 前面ウィンドウの所有プロセスが自プロセスなら true。gilrs はフォーカス非依存で
/// グローバル入力を拾うため、別アプリ前面時にコントローラ入力で mIV が動かないようにする。
#[cfg(windows)]
fn app_window_is_foreground() -> bool {
    use windows::Win32::System::Threading::GetCurrentProcessId;
    use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId};
    unsafe {
        let hwnd = GetForegroundWindow();
        if hwnd.0.is_null() {
            return false;
        }
        let mut pid: u32 = 0;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        pid != 0 && pid == GetCurrentProcessId()
    }
}

#[cfg(not(windows))]
fn app_window_is_foreground() -> bool {
    true
}

impl App {
    pub(crate) fn draw_gamepad_ring_overlay(&self, ui: &mut egui::Ui, full_rect: egui::Rect) {
        if !self.settings.ring_shortcuts.gamepad_ring_enabled
            || self.ring_picker.is_some()
            || !self.gamepad_state.west_ring_active()
        {
            return;
        }
        let context = self.current_ring_shortcut_context();
        let selected = self.gamepad_state.west_ring_direction();
        let painter = ui.painter();
        let center = full_rect.center();
        let radius = (full_rect.width().min(full_rect.height()) * 0.18).clamp(72.0, 132.0);
        let backdrop_radius = radius + 48.0;
        painter.circle_filled(
            center,
            backdrop_radius,
            egui::Color32::from_black_alpha(128),
        );
        painter.circle_stroke(
            center,
            backdrop_radius,
            egui::Stroke::new(1.0, egui::Color32::from_white_alpha(56)),
        );

        for &direction in RingDirection::all() {
            let pos = center + ring_direction_unit(direction) * radius;
            let is_selected = selected == Some(direction);
            let fill = if is_selected {
                egui::Color32::from_rgb(72, 126, 190)
            } else {
                egui::Color32::from_black_alpha(170)
            };
            let stroke = if is_selected {
                egui::Stroke::new(2.0, egui::Color32::WHITE)
            } else {
                egui::Stroke::new(1.0, egui::Color32::from_white_alpha(120))
            };
            painter.circle_filled(pos, if is_selected { 25.0 } else { 21.0 }, fill);
            painter.circle_stroke(pos, if is_selected { 25.0 } else { 21.0 }, stroke);
            painter.text(
                pos,
                egui::Align2::CENTER_CENTER,
                ring_direction_short_label(direction),
                egui::FontId::proportional(if is_selected { 14.0 } else { 12.5 }),
                egui::Color32::WHITE,
            );
        }

        let center_rect = egui::Rect::from_center_size(center, egui::vec2(230.0, 56.0));
        painter.rect_filled(
            center_rect,
            8.0,
            egui::Color32::from_black_alpha(if selected.is_some() { 205 } else { 175 }),
        );
        painter.rect_stroke(
            center_rect,
            8.0,
            egui::Stroke::new(1.0, egui::Color32::from_white_alpha(110)),
            egui::StrokeKind::Outside,
        );
        let (heading, detail) = if let Some(direction) = selected {
            let action = self
                .settings
                .ring_shortcuts
                .profile(context)
                .slots
                .get(direction.slot_index())
                .cloned()
                .unwrap_or_default();
            (
                ring_direction_short_label(direction),
                action.label_for_context(context),
            )
        } else {
            ("X", "方向なしで離すとピッカー")
        };
        painter.text(
            center + egui::vec2(0.0, -11.0),
            egui::Align2::CENTER_CENTER,
            heading,
            egui::FontId::proportional(14.0),
            egui::Color32::from_white_alpha(220),
        );
        painter.text(
            center + egui::vec2(0.0, 11.0),
            egui::Align2::CENTER_CENTER,
            detail,
            egui::FontId::proportional(16.0),
            egui::Color32::WHITE,
        );
    }

    pub(crate) fn draw_mouse_ring_flick_overlay(&self, ui: &mut egui::Ui, full_rect: egui::Rect) {
        if !self.settings.ring_shortcuts.mouse_flick_enabled || self.ring_picker.is_some() {
            return;
        }
        let Some(flick) = self.mouse_ring_flick.as_ref() else {
            return;
        };
        if !flick.guide_visible() {
            return;
        }

        let context = flick.context;
        let selected = mouse_flick_direction(flick);
        let radius = 78.0;
        let margin = radius + 48.0;
        let min_x = full_rect.left() + margin;
        let max_x = full_rect.right() - margin;
        let min_y = full_rect.top() + margin;
        let max_y = full_rect.bottom() - margin;
        let center = egui::pos2(
            if min_x <= max_x {
                flick.start_pos.x.clamp(min_x, max_x)
            } else {
                full_rect.center().x
            },
            if min_y <= max_y {
                flick.start_pos.y.clamp(min_y, max_y)
            } else {
                full_rect.center().y
            },
        );
        let painter = ui.painter();
        painter.circle_filled(center, radius + 42.0, egui::Color32::from_black_alpha(118));
        painter.circle_stroke(
            center,
            radius + 42.0,
            egui::Stroke::new(1.0, egui::Color32::from_white_alpha(52)),
        );

        for &direction in RingDirection::all() {
            let pos = center + ring_direction_unit(direction) * radius;
            let is_selected = selected == Some(direction);
            let fill = if is_selected {
                egui::Color32::from_rgb(72, 126, 190)
            } else {
                egui::Color32::from_black_alpha(168)
            };
            let stroke = if is_selected {
                egui::Stroke::new(2.0, egui::Color32::WHITE)
            } else {
                egui::Stroke::new(1.0, egui::Color32::from_white_alpha(115))
            };
            painter.circle_filled(pos, if is_selected { 24.0 } else { 20.0 }, fill);
            painter.circle_stroke(pos, if is_selected { 24.0 } else { 20.0 }, stroke);
            painter.text(
                pos,
                egui::Align2::CENTER_CENTER,
                ring_direction_short_label(direction),
                egui::FontId::proportional(if is_selected { 13.5 } else { 12.0 }),
                egui::Color32::WHITE,
            );
        }

        let label = selected
            .map(|direction| {
                self.settings
                    .ring_shortcuts
                    .profile(context)
                    .slots
                    .get(direction.slot_index())
                    .cloned()
                    .unwrap_or_default()
                    .label_for_context(context)
            })
            .unwrap_or("中央で離すと取消");
        let label_rect = egui::Rect::from_center_size(center, egui::vec2(210.0, 50.0));
        painter.rect_filled(label_rect, 8.0, egui::Color32::from_black_alpha(198));
        painter.rect_stroke(
            label_rect,
            8.0,
            egui::Stroke::new(1.0, egui::Color32::from_white_alpha(105)),
            egui::StrokeKind::Outside,
        );
        painter.text(
            center,
            egui::Align2::CENTER_CENTER,
            label,
            egui::FontId::proportional(15.0),
            egui::Color32::WHITE,
        );
    }

    pub(crate) fn start_mouse_ring_flick(
        &mut self,
        ctx: &egui::Context,
        context: RingShortcutContext,
        pos: egui::Pos2,
        grid_target_idx: Option<usize>,
    ) {
        if !self.settings.ring_shortcuts.mouse_flick_enabled || self.ring_picker.is_some() {
            return;
        }
        if self.mouse_ring_flick.is_some() {
            return;
        }
        self.mouse_ring_flick = Some(MouseFlickState::new(context, Instant::now(), pos));
        self.mouse_ring_grid_target_idx = grid_target_idx;
        self.mouse_ring_suppress_context_menu_once = false;
        ctx.request_repaint_after(mouse_flick_guide_delay());
    }

    pub(crate) fn update_mouse_ring_flick(
        &mut self,
        ctx: &egui::Context,
        context: RingShortcutContext,
    ) -> MouseFlickOutcome {
        let Some(existing) = self.mouse_ring_flick.as_ref() else {
            return MouseFlickOutcome::None;
        };
        if existing.context != context {
            self.cancel_mouse_ring_flick();
            return MouseFlickOutcome::None;
        }

        let fallback_pos = existing.current_pos;
        let (secondary_down, secondary_released, pointer_pos) = ctx.input(|i| {
            (
                i.pointer.secondary_down(),
                i.pointer.secondary_released(),
                i.pointer
                    .interact_pos()
                    .or_else(|| i.pointer.latest_pos())
                    .unwrap_or(fallback_pos),
            )
        });

        let (moved, elapsed, current_pos, armed, direction) = {
            let Some(flick) = self.mouse_ring_flick.as_mut() else {
                return MouseFlickOutcome::None;
            };
            flick.current_pos = pointer_pos;
            if flick.moved() >= MOUSE_FLICK_MOVE_THRESHOLD_PX {
                flick.armed = true;
            }
            (
                flick.moved(),
                flick.elapsed(),
                flick.current_pos,
                flick.armed,
                mouse_flick_direction(flick),
            )
        };

        if secondary_released {
            let fired = armed || direction.is_some();
            self.mouse_ring_flick = None;
            self.mouse_ring_grid_target_idx = None;
            if fired {
                self.mouse_ring_suppress_context_menu_once = true;
                if let Some(direction) = direction
                    && let Some(nav) = self.trigger_ring_shortcut_action(ctx, context, direction)
                {
                    self.mouse_ring_nav = Some(nav);
                }
                ctx.request_repaint();
                return MouseFlickOutcome::Fired;
            }
            if moved < MOUSE_FLICK_MOVE_THRESHOLD_PX && elapsed < mouse_flick_menu_delay() {
                ctx.request_repaint();
                return MouseFlickOutcome::ShortTap;
            }
            ctx.request_repaint();
            return MouseFlickOutcome::None;
        }

        if !secondary_down {
            self.cancel_mouse_ring_flick();
            return MouseFlickOutcome::None;
        }

        if !armed && moved < MOUSE_FLICK_MOVE_THRESHOLD_PX && elapsed >= mouse_flick_menu_delay() {
            self.mouse_ring_flick = None;
            self.mouse_ring_suppress_context_menu_once = true;
            ctx.request_repaint();
            return MouseFlickOutcome::LongPressMenu(current_pos);
        }

        self.request_mouse_ring_flick_repaint(ctx);
        MouseFlickOutcome::None
    }

    pub(crate) fn mouse_ring_context_menu_suppressed(&self, ctx: &egui::Context) -> bool {
        if self.mouse_ring_suppress_context_menu_once {
            return true;
        }
        if !self.settings.ring_shortcuts.mouse_flick_enabled {
            return false;
        }
        let Some(flick) = self.mouse_ring_flick.as_ref() else {
            return false;
        };
        if flick.armed {
            return true;
        }
        ctx.input(|i| {
            i.pointer
                .interact_pos()
                .or_else(|| i.pointer.latest_pos())
                .is_some_and(|pos| pos.distance(flick.start_pos) >= MOUSE_FLICK_MOVE_THRESHOLD_PX)
        })
    }

    pub(crate) fn clear_mouse_ring_context_menu_suppression_if_idle(
        &mut self,
        ctx: &egui::Context,
    ) {
        let idle = ctx.input(|i| !i.pointer.secondary_down() && !i.pointer.secondary_released());
        if idle {
            self.mouse_ring_suppress_context_menu_once = false;
        }
    }

    pub(crate) fn cancel_mouse_ring_flick(&mut self) {
        self.mouse_ring_flick = None;
        self.mouse_ring_grid_target_idx = None;
    }

    pub(crate) fn request_mouse_ring_flick_repaint(&self, ctx: &egui::Context) {
        let Some(flick) = self.mouse_ring_flick.as_ref() else {
            return;
        };
        let elapsed = flick.elapsed();
        if elapsed < mouse_flick_guide_delay() {
            ctx.request_repaint_after(mouse_flick_guide_delay() - elapsed);
        } else if !flick.armed && elapsed < mouse_flick_menu_delay() {
            ctx.request_repaint_after(mouse_flick_menu_delay() - elapsed);
        } else if flick.armed {
            ctx.request_repaint_after(GAMEPAD_REPAINT_INTERVAL);
        }
    }

    pub(crate) fn draw_gamepad_picker_overlay(&self, ui: &mut egui::Ui, full_rect: egui::Rect) {
        let Some(picker) = self.ring_picker.as_ref() else {
            return;
        };
        let painter = ui.painter();
        painter.rect_filled(full_rect, 0.0, egui::Color32::from_black_alpha(120));

        let rows = picker_rows_for_context(picker.context);
        let row_h = 32.0;
        let drill = picker.drill;
        let panel_w = (full_rect.width() * 0.60).clamp(340.0, 560.0);
        let panel_h = if drill.is_some() {
            188.0
        } else {
            96.0 + row_h * rows.len() as f32
        };
        let panel_rect =
            egui::Rect::from_center_size(full_rect.center(), egui::vec2(panel_w, panel_h));

        egui::Area::new(egui::Id::new("gamepad_ring_picker_overlay"))
            .order(egui::Order::Foreground)
            .fixed_pos(panel_rect.min)
            .show(ui.ctx(), |ui| {
                ui.set_min_size(panel_rect.size());
                egui::Frame::new()
                    .fill(egui::Color32::from_black_alpha(220))
                    .stroke(egui::Stroke::new(1.0, egui::Color32::from_white_alpha(90)))
                    .corner_radius(egui::CornerRadius::same(8))
                    .inner_margin(egui::Margin::same(14))
                    .show(ui, |ui| {
                        ui.set_width(panel_w - 28.0);
                        ui.label(
                            egui::RichText::new(format!("X ピッカー / {}", picker.context.label()))
                                .size(17.0)
                                .color(egui::Color32::WHITE),
                        );
                        ui.add_space(6.0);
                        if let Some(drill) = drill {
                            self.draw_gamepad_post_filter_drill(ui, picker, drill);
                        } else {
                            for (idx, &row) in rows.iter().enumerate() {
                                let selected = idx == picker.current_row();
                                let (rect, _) = ui.allocate_exact_size(
                                    egui::vec2(ui.available_width(), row_h),
                                    egui::Sense::hover(),
                                );
                                let fill = if selected {
                                    egui::Color32::from_rgb(56, 94, 138)
                                } else {
                                    egui::Color32::from_black_alpha(0)
                                };
                                ui.painter().rect_filled(rect, 5.0, fill);
                                let label_pos = egui::pos2(rect.min.x + 10.0, rect.center().y);
                                let value_pos = egui::pos2(rect.max.x - 10.0, rect.center().y);
                                ui.painter().text(
                                    label_pos,
                                    egui::Align2::LEFT_CENTER,
                                    row.label(),
                                    egui::FontId::proportional(14.5),
                                    egui::Color32::from_white_alpha(if selected {
                                        245
                                    } else {
                                        205
                                    }),
                                );
                                ui.painter().text(
                                    value_pos,
                                    egui::Align2::RIGHT_CENTER,
                                    self.picker_value_text(picker, row),
                                    egui::FontId::proportional(14.5),
                                    egui::Color32::WHITE,
                                );
                            }
                            ui.add_space(8.0);
                            ui.label(
                                egui::RichText::new("方向:選択/変更  A:確定  B:取消  X:確定")
                                    .size(12.0)
                                    .color(egui::Color32::from_white_alpha(190)),
                            );
                        }
                    });
            });
    }

    fn draw_gamepad_post_filter_drill(
        &self,
        ui: &mut egui::Ui,
        picker: &RingPickerState,
        drill: PostFilterDrillState,
    ) {
        let group = POST_FILTER_GROUPS
            .get(drill.group)
            .unwrap_or(&POST_FILTER_GROUPS[0]);
        let filter = group
            .filters
            .get(drill.item)
            .copied()
            .unwrap_or(PostFilter::None);
        ui.label(
            egui::RichText::new("ポストフィルタ")
                .size(14.0)
                .color(egui::Color32::from_white_alpha(200)),
        );
        ui.add_space(8.0);
        ui.label(
            egui::RichText::new(format!(
                "{}/{}  {}",
                drill.group + 1,
                POST_FILTER_GROUPS.len(),
                group.label
            ))
            .size(16.0)
            .color(egui::Color32::WHITE),
        );
        ui.label(
            egui::RichText::new(format!(
                "{}/{}  {}",
                drill.item + 1,
                group.filters.len(),
                filter.display_label()
            ))
            .size(17.0)
            .color(egui::Color32::WHITE),
        );
        ui.add_space(10.0);
        ui.label(
            egui::RichText::new(format!("選択中: {}", picker.post_filter.display_label()))
                .size(13.0)
                .color(egui::Color32::from_white_alpha(210)),
        );
        ui.add_space(8.0);
        ui.label(
            egui::RichText::new("上下:グループ  左右:項目  A/B:戻る  X:確定")
                .size(12.0)
                .color(egui::Color32::from_white_alpha(190)),
        );
    }

    fn picker_value_text(&self, picker: &RingPickerState, row: RingPickerRowId) -> String {
        match row {
            RingPickerRowId::GridColumns => format!("{} 列", picker.grid_cols),
            RingPickerRowId::GridSortOrder => picker.sort_order.label().to_string(),
            RingPickerRowId::GridThumbAspect => {
                if picker.thumb_aspect_auto {
                    if let Some(current) = self.auto_aspect.current {
                        format!("自動 ({})", current.label())
                    } else {
                        "自動".to_string()
                    }
                } else {
                    picker.thumb_aspect.label().to_string()
                }
            }
            RingPickerRowId::ItemRating => rating_label(picker.item_rating),
            RingPickerRowId::ContainerRating => rating_label(picker.container_rating),
            RingPickerRowId::SpreadMode => picker.spread_mode.label().to_string(),
            RingPickerRowId::ReadingFlow => picker.reading_flow.label().to_string(),
            RingPickerRowId::FitMode => picker.fit_mode.label().to_string(),
            RingPickerRowId::PostFilter => {
                format!("{}  Aで選択", picker.post_filter.display_label())
            }
            RingPickerRowId::UpscaleModel => {
                crate::adjustment::upscale_model_label(picker.upscale_model_key.as_deref())
                    .to_string()
            }
            RingPickerRowId::VideoVolume => format_video_volume_db(picker.video_volume),
            RingPickerRowId::VideoPlaybackSpeed => {
                crate::video::clock::format_playback_speed(picker.video_playback_speed)
            }
            RingPickerRowId::VideoContinuousMode => {
                picker.video_continuous_mode.label().to_string()
            }
        }
    }

    pub(crate) fn handle_gamepad_input(&mut self, ctx: &egui::Context) -> Option<AddressBarNav> {
        let now = Instant::now();
        let events = self.gamepad.drain(ctx);
        let mut actions = Vec::new();
        let mut saw_input_event = false;

        for event in events {
            match event {
                PadEvent::ButtonPressed(button) => {
                    saw_input_event = true;
                    self.gamepad_state.set_button_down(button, true, now);
                    actions.push(PadAction {
                        button,
                        kind: PadActionKind::Press,
                    });
                }
                PadEvent::ButtonReleased(button) => {
                    saw_input_event = true;
                    self.gamepad_state.set_button_down(button, false, now);
                    actions.push(PadAction {
                        button,
                        kind: PadActionKind::Release,
                    });
                }
                PadEvent::AxisChanged(axis, value) => {
                    saw_input_event = true;
                    self.gamepad_state.set_axis(axis, value);
                }
                PadEvent::Connected => {
                    saw_input_event = true;
                }
                PadEvent::Disconnected => {
                    saw_input_event = true;
                    // 握ったまま切断すると保持状態が残りリピート/アナログが止まらない。
                    self.gamepad_state.clear();
                }
            }
        }

        for button in REPEAT_BUTTONS {
            let interval = repeat_interval_for_button(button);
            if self.gamepad_state.due_button_repeat(button, now, interval) {
                actions.push(PadAction {
                    button,
                    kind: PadActionKind::Repeat,
                });
            }
        }

        let dispatch_allowed = self.gamepad_dispatch_allowed(ctx);
        let mut nav = None;
        let mut dispatched = false;
        if dispatch_allowed {
            for action in actions {
                if let Some(next_nav) = self.dispatch_gamepad_button(ctx, action) {
                    nav = Some(next_nav);
                    dispatched = true;
                    break;
                }
                dispatched = true;
            }
            if self.dispatch_gamepad_analog(ctx, now) {
                dispatched = true;
            }
        } else {
            self.reset_gamepad_continuous_steps(now);
            // ブロック中に押されたボタンが解除後に遅れて発火しないよう抑止する。
            self.gamepad_state.suppress_pending_actions();
        }

        if saw_input_event || dispatched {
            self.activity_gate.bump();
        }

        if dispatch_allowed && self.gamepad_needs_repaint() {
            ctx.request_repaint_after(GAMEPAD_REPAINT_INTERVAL);
        }

        nav
    }

    fn gamepad_dispatch_allowed(&self, ctx: &egui::Context) -> bool {
        // gilrs はグローバル入力 (ウィンドウフォーカス非依存) なので、mIV が前面に
        // 無いときにバックグラウンドのコントローラ入力で操作されないよう OS の前面
        // プロセスでゲートする。フルスクリーンは別ウィンドウ (別ビューポート) なので
        // egui の viewport().focused (= メインビューポート) では正しく判定できない。
        if !app_window_is_foreground() {
            return false;
        }
        if self.ime_input_active() {
            return false;
        }
        if self.fullscreen_idx.is_some() {
            return !self.any_modal_dialog_open_for_fullscreen_keys()
                && self.fs_context_menu_idx.is_none()
                && !self.erase_mode
                && !self.conceal_mode
                && !self.local_adjust_mode
                && !self.export_crop_mode
                && !self.text_mode;
        }
        !self.shortcuts_blocked_by_text_input() && !ctx.is_popup_open()
    }

    fn reset_gamepad_continuous_steps(&mut self, now: Instant) {
        let _ = self.gamepad_state.analog_dt(false, now);
        let _ = self
            .gamepad_state
            .left_stick_step_due(false, now, STICK_STEP_INTERVAL);
        let _ = self
            .gamepad_state
            .trigger_step_due(false, now, TRIGGER_STEP_INTERVAL);
    }

    fn gamepad_needs_repaint(&self) -> bool {
        self.gamepad_state.repeat_active()
            || self.gamepad_analog_active()
            || self.gamepad_state.button_down(PadButton::North)
            || self.gamepad_state.button_down(PadButton::West)
            || self.ring_picker.is_some()
    }

    fn gamepad_analog_active(&self) -> bool {
        let left = stick_pair(&self.gamepad_state, PadAxis::LeftX, PadAxis::LeftY);
        let right = stick_pair(&self.gamepad_state, PadAxis::RightX, PadAxis::RightY);
        left.length_sq() > 0.0
            || right.length_sq() > 0.0
            || self.gamepad_state.trigger_value(true) > TRIGGER_THRESHOLD
            || self.gamepad_state.trigger_value(false) > TRIGGER_THRESHOLD
    }

    fn dispatch_gamepad_button(
        &mut self,
        ctx: &egui::Context,
        action: PadAction,
    ) -> Option<AddressBarNav> {
        if self.ring_picker.is_some() {
            self.dispatch_gamepad_picker_button(ctx, action);
            return None;
        }
        match action.kind {
            PadActionKind::Release if action.button == PadButton::West => {
                if self.settings.ring_shortcuts.gamepad_ring_enabled {
                    return self.finish_gamepad_west_release(ctx);
                }
                self.gamepad_state.finish_west_release();
                None
            }
            PadActionKind::Release if action.button == PadButton::North => {
                if !self.gamepad_state.y_modifier_used() {
                    self.handle_gamepad_y_tap(ctx);
                }
                None
            }
            PadActionKind::Release => None,
            PadActionKind::Press | PadActionKind::Repeat => {
                if self.settings.ring_shortcuts.gamepad_ring_enabled
                    && self.gamepad_state.west_ring_active()
                {
                    if let Some(dir) = button_dir(action.button) {
                        self.gamepad_state
                            .mark_west_ring_direction(ring_direction_from_pad_dir(dir));
                        ctx.request_repaint();
                    } else if action.button == PadButton::East
                        && action.kind == PadActionKind::Press
                    {
                        self.gamepad_state.cancel_west_ring();
                        ctx.request_repaint();
                    }
                    return None;
                }
                if let Some(dir) = button_dir(action.button) {
                    if self.handle_gamepad_folder_tree_direction(dir) {
                        return None;
                    }
                    self.handle_gamepad_direction(ctx, dir, action.kind == PadActionKind::Repeat);
                    return None;
                }
                match action.button {
                    PadButton::South if action.kind == PadActionKind::Press => {
                        self.handle_gamepad_accept(ctx)
                    }
                    PadButton::East if action.kind == PadActionKind::Press => {
                        self.handle_gamepad_back(ctx)
                    }
                    PadButton::West if action.kind == PadActionKind::Press => None,
                    PadButton::Select if action.kind == PadActionKind::Press => {
                        self.handle_gamepad_select(ctx);
                        None
                    }
                    PadButton::Start if action.kind == PadActionKind::Press => {
                        self.handle_gamepad_start()
                    }
                    PadButton::LeftShoulder => {
                        self.handle_gamepad_folder_nav(ctx, false);
                        None
                    }
                    PadButton::RightShoulder => {
                        self.handle_gamepad_folder_nav(ctx, true);
                        None
                    }
                    PadButton::North
                    | PadButton::Start
                    | PadButton::LeftTrigger
                    | PadButton::RightTrigger
                    | PadButton::South
                    | PadButton::East
                    | PadButton::West
                    | PadButton::Select => None,
                    PadButton::DPadUp
                    | PadButton::DPadDown
                    | PadButton::DPadLeft
                    | PadButton::DPadRight => None,
                }
            }
        }
    }

    fn dispatch_gamepad_analog(&mut self, ctx: &egui::Context, now: Instant) -> bool {
        if self.ring_picker.is_some() {
            return self.dispatch_gamepad_picker_analog(ctx, now);
        }
        if self.settings.ring_shortcuts.gamepad_ring_enabled
            && self.gamepad_state.west_ring_active()
        {
            let stick = stick_pair(&self.gamepad_state, PadAxis::LeftX, PadAxis::LeftY);
            if let Some(direction) = ring_direction_from_stick(stick) {
                self.gamepad_state.mark_west_ring_direction(direction);
            }
            return true;
        }
        let Some(fs_idx) = self.fullscreen_idx else {
            return self.dispatch_gamepad_grid_analog(now);
        };
        if self.current_fullscreen_is_video(fs_idx) {
            return self.dispatch_gamepad_video_analog(ctx, fs_idx, now);
        }
        self.dispatch_gamepad_still_analog(ctx, now)
    }

    fn dispatch_gamepad_grid_analog(&mut self, now: Instant) -> bool {
        let mut changed = false;
        let stick = stick_pair(&self.gamepad_state, PadAxis::LeftX, PadAxis::LeftY);
        let stick_dir = dominant_stick_dir(stick);
        let stick_active = stick_dir.is_some();
        if self.folder_pane_blocks_grid_keyboard() {
            if self
                .gamepad_state
                .left_stick_step_due(stick_active, now, STICK_STEP_INTERVAL)
                && let Some(dir) = stick_dir
            {
                changed = self.handle_gamepad_folder_tree_direction(dir);
            }
            let _ = self
                .gamepad_state
                .trigger_step_due(false, now, Duration::from_millis(70));
            let _ = self.gamepad_state.analog_dt(stick_active, now);
            return changed;
        }
        if self
            .gamepad_state
            .left_stick_step_due(stick_active, now, STICK_STEP_INTERVAL)
            && let Some(dir) = stick_dir
        {
            self.handle_gamepad_direction_for_grid(dir);
            changed = true;
        }

        let trigger_delta =
            self.gamepad_state.trigger_value(false) - self.gamepad_state.trigger_value(true);
        let trigger_active = trigger_delta.abs() > TRIGGER_THRESHOLD;
        if self
            .gamepad_state
            .trigger_step_due(trigger_active, now, Duration::from_millis(70))
        {
            self.scroll_gamepad_grid(trigger_delta.signum());
            changed = true;
        }

        let _ = self
            .gamepad_state
            .analog_dt(stick_active || trigger_active, now);
        changed
    }

    fn dispatch_gamepad_still_analog(&mut self, ctx: &egui::Context, now: Instant) -> bool {
        let left = stick_pair(&self.gamepad_state, PadAxis::LeftX, PadAxis::LeftY);
        let right = stick_pair(&self.gamepad_state, PadAxis::RightX, PadAxis::RightY);
        let lt = self.gamepad_state.trigger_value(true);
        let rt = self.gamepad_state.trigger_value(false);
        let pan_active = left.length_sq() > 0.0;
        let zoom_axis = (right.y * RIGHT_STICK_ZOOM_MULTIPLIER + rt - lt)
            .clamp(-RIGHT_STICK_ZOOM_MULTIPLIER, RIGHT_STICK_ZOOM_MULTIPLIER);
        let zoom_active = zoom_axis.abs() > 0.05;
        let active = pan_active || zoom_active;
        let dt = self.gamepad_state.analog_dt(active, now);
        if !active || dt <= 0.0 {
            return false;
        }

        let mut changed = false;
        if pan_active {
            // 連続読みのスクロールは、レンダラが実際に連続描画しているとき (continuous_active)
            // だけ。reading_flow だけで判定すると、解析(Z)/比較(X/C)/オーバーレイ編集で単ページに
            // フォールバックしているのにスティックがスクロールへ吸われ、ページ送りできなくなる。
            let continuous_active = self
                .fullscreen_idx
                .is_some_and(|idx| self.continuous_reading_active_for_idx(idx));
            if continuous_active
                && let Some(axis) =
                    continuous_reading_stick_axis(self.reading_flow, self.reading_direction, left)
            {
                let delta = axis * self.continuous_reading_gamepad_speed_px_per_sec(ctx) * dt;
                if delta.abs() > 0.5 {
                    self.scroll_vertical_reading_by(ctx, delta);
                    changed = true;
                }
            } else if continuous_active
                && (self.reading_flow.is_vertical() || self.reading_flow.is_horizontal())
            {
                if self.dispatch_gamepad_still_stick_step(ctx, now, left) {
                    changed = true;
                }
            } else if self.fs_zoom > 1.0 {
                let pan = egui::vec2(left.x, -left.y) * (PAN_SPEED_PX_PER_SEC * dt);
                if self.apply_gamepad_fullscreen_pan(pan) {
                    changed = true;
                }
            } else if self.dispatch_gamepad_still_stick_step(ctx, now, left) {
                changed = true;
            }
        }
        if zoom_active && self.apply_gamepad_fullscreen_zoom(zoom_axis, dt) {
            changed = true;
        }
        if changed {
            ctx.request_repaint();
        }
        changed
    }

    fn dispatch_gamepad_still_stick_step(
        &mut self,
        ctx: &egui::Context,
        now: Instant,
        stick: egui::Vec2,
    ) -> bool {
        let stick_dir = dominant_stick_dir(stick);
        let stick_active = stick_dir.is_some();
        if self
            .gamepad_state
            .left_stick_step_due(stick_active, now, STICK_STEP_INTERVAL)
            && let Some(fs_idx) = self.fullscreen_idx
            && let Some(dir) = stick_dir
        {
            self.handle_gamepad_still_direction(ctx, fs_idx, dir);
            true
        } else {
            false
        }
    }

    fn dispatch_gamepad_video_analog(
        &mut self,
        ctx: &egui::Context,
        fs_idx: usize,
        now: Instant,
    ) -> bool {
        let trigger_delta =
            self.gamepad_state.trigger_value(false) - self.gamepad_state.trigger_value(true);
        let trigger_active = trigger_delta.abs() > TRIGGER_THRESHOLD;
        let due = self
            .gamepad_state
            .trigger_step_due(trigger_active, now, TRIGGER_STEP_INTERVAL);
        let _ = self.gamepad_state.analog_dt(trigger_active, now);
        if !due {
            return false;
        }
        let vk = if trigger_delta > 0.0 { 0x27 } else { 0x25 };
        self.dispatch_native_video_key(ctx, fs_idx, vk, false, false, true);
        true
    }

    fn handle_gamepad_accept(&mut self, ctx: &egui::Context) -> Option<AddressBarNav> {
        if let Some(fs_idx) = self.fullscreen_idx {
            if self.current_fullscreen_is_video(fs_idx) {
                self.dispatch_native_video_key(ctx, fs_idx, 0x0D, false, false, false);
            } else {
                self.navigate_gamepad_still(ctx, fs_idx, 1);
            }
            return None;
        }
        self.handle_gamepad_grid_accept()
    }

    fn handle_gamepad_back(&mut self, ctx: &egui::Context) -> Option<AddressBarNav> {
        if let Some(fs_idx) = self.fullscreen_idx {
            if self.current_fullscreen_is_video(fs_idx) {
                self.dispatch_native_video_key(ctx, fs_idx, 0x1B, false, false, false);
            } else {
                self.bump_input_seq("gamepad_fs_close", None);
                self.handle_fs_navigation(ctx, true, false, None, None, None, 0, None, fs_idx);
            }
            return None;
        }
        self.handle_gamepad_grid_back()
    }

    fn finish_gamepad_west_release(&mut self, ctx: &egui::Context) -> Option<AddressBarNav> {
        match self.gamepad_state.finish_west_release() {
            WestReleaseOutcome::Picker => {
                self.open_gamepad_ring_picker(ctx);
                None
            }
            WestReleaseOutcome::Ring(direction) => self.trigger_gamepad_ring_action(ctx, direction),
            WestReleaseOutcome::Suppressed => None,
        }
    }

    fn open_gamepad_ring_picker(&mut self, ctx: &egui::Context) {
        let context = self.current_ring_shortcut_context();
        let mut picker = self.build_ring_picker_state(context);
        picker.clamp_row(picker_rows_for_context(context).len());
        self.ring_picker = Some(picker);
        ctx.request_repaint();
        self.sync_native_video_picker_overlay(ctx);
        self.maybe_show_x_picker_hint(ctx, context);
    }

    fn build_ring_picker_state(&mut self, context: RingShortcutContext) -> RingPickerState {
        let fs_idx = self.fullscreen_idx;
        let params = fs_idx.map(|idx| self.effective_params(idx).clone());
        let item_rating = self.current_picker_item_rating(context);
        let container_rating = self.current_folder_rating();
        RingPickerState {
            context,
            row: 0,
            dirty_rows: Vec::new(),
            x_close_armed: false,
            drill: None,
            grid_cols: self.settings.grid_cols.clamp(
                crate::settings::MIN_GRID_COLS,
                crate::settings::MAX_GRID_COLS,
            ),
            sort_order: self.settings.sort_order,
            thumb_aspect_auto: self.settings.thumb_aspect_auto,
            thumb_aspect: self.settings.thumb_aspect,
            item_rating,
            container_rating,
            spread_mode: self.spread_mode,
            reading_flow: self.reading_flow,
            fit_mode: self
                .settings
                .fullscreen_fit_mode
                .effective_for_flow(self.reading_flow),
            post_filter: params
                .as_ref()
                .map(|p| p.post_filter)
                .unwrap_or(PostFilter::None),
            upscale_model_key: params.and_then(|p| p.upscale_model),
            video_volume: self.settings.video_volume,
            video_playback_speed: self.video_playback_speed,
            video_continuous_mode: self.video_continuous_mode,
        }
    }

    fn maybe_show_x_picker_hint(&mut self, ctx: &egui::Context, context: RingShortcutContext) {
        if context != RingShortcutContext::ImageFullscreen
            || self.settings.ring_shortcuts.x_picker_hint_shown
        {
            return;
        }
        self.settings.ring_shortcuts.x_picker_hint_shown = true;
        self.settings.save();
        self.show_feedback_toast_with_duration(
            "X 単体はピッカーを開きます。メタデータ表示は画像リングの右上スロットから使えます。"
                .to_string(),
            X_PICKER_HINT_TOAST_SECS,
        );
        ctx.request_repaint();
    }

    fn current_picker_item_rating(&mut self, context: RingShortcutContext) -> u8 {
        match context {
            RingShortcutContext::Grid => {
                let targets = self.ratable_targets();
                if targets.len() == 1 {
                    self.get_rating(targets[0])
                } else {
                    0
                }
            }
            RingShortcutContext::ImageFullscreen | RingShortcutContext::VideoFullscreen => self
                .fullscreen_idx
                .map(|idx| self.get_rating(idx))
                .unwrap_or(0),
        }
    }

    fn dispatch_gamepad_picker_button(&mut self, ctx: &egui::Context, action: PadAction) {
        if self.ring_picker_context_is_stale() {
            self.clear_native_video_picker_overlay(ctx);
            self.ring_picker = None;
            return;
        }
        match action.kind {
            PadActionKind::Release if action.button == PadButton::West => {
                let close = self
                    .ring_picker
                    .as_ref()
                    .is_some_and(|picker| picker.x_close_armed);
                if close {
                    self.commit_ring_picker(ctx);
                }
            }
            PadActionKind::Release => {}
            PadActionKind::Press | PadActionKind::Repeat => {
                if let Some(dir) = button_dir(action.button) {
                    self.handle_ring_picker_direction(ctx, dir);
                    return;
                }
                match action.button {
                    PadButton::South if action.kind == PadActionKind::Press => {
                        if self.ring_picker_drill_active() {
                            self.exit_ring_picker_drill(ctx);
                        } else if self.current_ring_picker_row_kind()
                            == Some(RingPickerRowId::PostFilter)
                        {
                            self.enter_ring_picker_post_filter_drill(ctx);
                        } else {
                            self.commit_ring_picker(ctx);
                        }
                    }
                    PadButton::East if action.kind == PadActionKind::Press => {
                        if self.ring_picker_drill_active() {
                            self.exit_ring_picker_drill(ctx);
                        } else {
                            self.cancel_ring_picker(ctx);
                        }
                    }
                    PadButton::West if action.kind == PadActionKind::Press => {
                        if let Some(picker) = self.ring_picker.as_mut() {
                            picker.x_close_armed = true;
                        }
                        ctx.request_repaint();
                    }
                    PadButton::North
                    | PadButton::Start
                    | PadButton::LeftShoulder
                    | PadButton::RightShoulder
                    | PadButton::LeftTrigger
                    | PadButton::RightTrigger
                    | PadButton::Select
                    | PadButton::South
                    | PadButton::East
                    | PadButton::West
                    | PadButton::DPadUp
                    | PadButton::DPadDown
                    | PadButton::DPadLeft
                    | PadButton::DPadRight => {}
                }
            }
        }
    }

    fn dispatch_gamepad_picker_analog(&mut self, ctx: &egui::Context, now: Instant) -> bool {
        if self.ring_picker_context_is_stale() {
            self.clear_native_video_picker_overlay(ctx);
            self.ring_picker = None;
            return false;
        }
        let stick = stick_pair(&self.gamepad_state, PadAxis::LeftX, PadAxis::LeftY);
        let stick_dir = dominant_stick_dir(stick);
        let active = stick_dir.is_some();
        let due = self
            .gamepad_state
            .left_stick_step_due(active, now, STICK_STEP_INTERVAL);
        if due && let Some(dir) = stick_dir {
            self.handle_ring_picker_direction(ctx, dir);
        }
        active
    }

    fn ring_picker_context_is_stale(&self) -> bool {
        self.ring_picker
            .as_ref()
            .is_some_and(|picker| picker.context != self.current_ring_shortcut_context())
    }

    fn ring_picker_drill_active(&self) -> bool {
        self.ring_picker
            .as_ref()
            .is_some_and(|picker| picker.drill.is_some())
    }

    fn current_ring_picker_row_kind(&self) -> Option<RingPickerRowId> {
        let picker = self.ring_picker.as_ref()?;
        picker_rows_for_context(picker.context)
            .get(picker.current_row())
            .copied()
    }

    fn handle_ring_picker_direction(&mut self, ctx: &egui::Context, dir: PadDir) {
        if self.ring_picker_drill_active() {
            self.handle_post_filter_drill_direction(ctx, dir);
            return;
        }
        match dir {
            PadDir::Up | PadDir::Down => {
                let Some(picker) = self.ring_picker.as_mut() else {
                    return;
                };
                let rows = picker_rows_for_context(picker.context);
                let delta = if dir == PadDir::Down { 1 } else { -1 };
                picker.row = cycle_index(rows.len(), picker.row, delta);
                picker.x_close_armed = false;
                ctx.request_repaint();
                self.sync_native_video_picker_overlay(ctx);
            }
            PadDir::Left | PadDir::Right => {
                if let Some(row) = self.current_ring_picker_row_kind() {
                    let delta = if dir == PadDir::Right { 1 } else { -1 };
                    self.change_ring_picker_value(row, delta);
                    if let Some(picker) = self.ring_picker.as_mut() {
                        picker.x_close_armed = false;
                    }
                    ctx.request_repaint();
                    self.sync_native_video_picker_overlay(ctx);
                }
            }
        }
    }

    fn change_ring_picker_value(&mut self, row: RingPickerRowId, delta: i32) {
        let ai_mode = self.settings.ai_feature_mode;
        let Some(picker) = self.ring_picker.as_mut() else {
            return;
        };
        match row {
            RingPickerRowId::GridColumns => {
                let next = (picker.grid_cols as i32 + delta).clamp(
                    crate::settings::MIN_GRID_COLS as i32,
                    crate::settings::MAX_GRID_COLS as i32,
                );
                picker.grid_cols = next as usize;
                mark_picker_dirty(picker, row);
            }
            RingPickerRowId::GridSortOrder => {
                picker.sort_order = cycle_value(SortOrder::all(), picker.sort_order, delta);
                mark_picker_dirty(picker, row);
            }
            RingPickerRowId::GridThumbAspect => {
                let aspects = ThumbAspect::all();
                let current = if picker.thumb_aspect_auto {
                    0
                } else {
                    1 + aspects
                        .iter()
                        .position(|&a| a == picker.thumb_aspect)
                        .unwrap_or(0)
                };
                let next = cycle_index(1 + aspects.len(), current, delta);
                if next == 0 {
                    picker.thumb_aspect_auto = true;
                } else {
                    picker.thumb_aspect_auto = false;
                    picker.thumb_aspect = aspects[next - 1];
                }
                mark_picker_dirty(picker, row);
            }
            RingPickerRowId::ItemRating => {
                picker.item_rating = cycle_rating(picker.item_rating, delta);
                mark_picker_dirty(picker, row);
            }
            RingPickerRowId::ContainerRating => {
                picker.container_rating = cycle_rating(picker.container_rating, delta);
                mark_picker_dirty(picker, row);
            }
            RingPickerRowId::SpreadMode => {
                picker.spread_mode = cycle_value(SpreadMode::all(), picker.spread_mode, delta);
                mark_picker_dirty(picker, row);
            }
            RingPickerRowId::ReadingFlow => {
                picker.reading_flow = cycle_value(ReadingFlow::all(), picker.reading_flow, delta);
                picker.fit_mode = picker.fit_mode.effective_for_flow(picker.reading_flow);
                mark_picker_dirty(picker, row);
            }
            RingPickerRowId::FitMode => {
                picker.fit_mode = cycle_value(
                    FullscreenFitMode::selectable_for_flow(picker.reading_flow),
                    picker.fit_mode.effective_for_flow(picker.reading_flow),
                    delta,
                );
                mark_picker_dirty(picker, row);
            }
            RingPickerRowId::PostFilter => {}
            RingPickerRowId::UpscaleModel => {
                let items = crate::adjustment::upscale_menu_items_for_mode(ai_mode);
                if items.len() <= 1 {
                    return;
                }
                let current = items
                    .iter()
                    .position(|(_, key)| *key == picker.upscale_model_key.as_deref());
                let Some(current) = current else {
                    return;
                };
                let next = cycle_index(items.len(), current, delta);
                picker.upscale_model_key = items[next].1.map(|s| s.to_string());
                mark_picker_dirty(picker, row);
            }
            RingPickerRowId::VideoVolume => {
                picker.video_volume =
                    step_video_volume_by_fader_key_step(picker.video_volume, delta);
                mark_picker_dirty(picker, row);
            }
            RingPickerRowId::VideoPlaybackSpeed => {
                picker.video_playback_speed =
                    cycle_video_playback_speed(picker.video_playback_speed, delta);
                mark_picker_dirty(picker, row);
            }
            RingPickerRowId::VideoContinuousMode => {
                const MODES: &[VideoContinuousMode] = &[
                    VideoContinuousMode::Off,
                    VideoContinuousMode::Continuous,
                    VideoContinuousMode::ContinuousLoop,
                ];
                picker.video_continuous_mode =
                    cycle_value(MODES, picker.video_continuous_mode, delta);
                mark_picker_dirty(picker, row);
            }
        }
    }

    fn enter_ring_picker_post_filter_drill(&mut self, ctx: &egui::Context) {
        let Some(picker) = self.ring_picker.as_mut() else {
            return;
        };
        picker.drill = Some(drill_for_post_filter(picker.post_filter));
        picker.x_close_armed = false;
        ctx.request_repaint();
        self.sync_native_video_picker_overlay(ctx);
    }

    fn exit_ring_picker_drill(&mut self, ctx: &egui::Context) {
        if let Some(picker) = self.ring_picker.as_mut() {
            picker.drill = None;
            picker.x_close_armed = false;
        }
        ctx.request_repaint();
        self.sync_native_video_picker_overlay(ctx);
    }

    fn handle_post_filter_drill_direction(&mut self, ctx: &egui::Context, dir: PadDir) {
        let Some(picker) = self.ring_picker.as_mut() else {
            return;
        };
        let Some(mut drill) = picker.drill else {
            return;
        };
        match dir {
            PadDir::Up | PadDir::Down => {
                let delta = if dir == PadDir::Down { 1 } else { -1 };
                drill.group = cycle_index(POST_FILTER_GROUPS.len(), drill.group, delta);
                let len = POST_FILTER_GROUPS[drill.group].filters.len();
                drill.item = drill.item.min(len.saturating_sub(1));
            }
            PadDir::Left | PadDir::Right => {
                let delta = if dir == PadDir::Right { 1 } else { -1 };
                let len = POST_FILTER_GROUPS[drill.group].filters.len();
                drill.item = cycle_index(len, drill.item, delta);
            }
        }
        picker.post_filter = POST_FILTER_GROUPS[drill.group].filters[drill.item];
        picker.drill = Some(drill);
        mark_picker_dirty(picker, RingPickerRowId::PostFilter);
        picker.x_close_armed = false;
        ctx.request_repaint();
        self.sync_native_video_picker_overlay(ctx);
    }

    fn sync_native_video_picker_overlay(&mut self, ctx: &egui::Context) {
        #[cfg(windows)]
        {
            let overlay = self
                .ring_picker
                .as_ref()
                .filter(|picker| picker.context == RingShortcutContext::VideoFullscreen)
                .map(|picker| self.native_video_picker_overlay(picker));
            self.set_native_video_ring_picker_overlay(overlay);
            self.request_native_video_hud_repaint(ctx);
        }
        #[cfg(not(windows))]
        let _ = ctx;
    }

    fn clear_native_video_picker_overlay(&mut self, ctx: &egui::Context) {
        #[cfg(windows)]
        {
            self.set_native_video_ring_picker_overlay(None);
            self.request_native_video_hud_repaint(ctx);
        }
        #[cfg(not(windows))]
        let _ = ctx;
    }

    #[cfg(windows)]
    fn native_video_picker_overlay(
        &self,
        picker: &RingPickerState,
    ) -> crate::video::native_presenter::NativeOverlayRingPicker {
        let rows = picker_rows_for_context(picker.context)
            .iter()
            .map(
                |&row| crate::video::native_presenter::NativeOverlayRingPickerRow {
                    label: row.label().to_string(),
                    value: self.picker_value_text(picker, row),
                },
            )
            .collect();
        let drill = picker.drill.map(|drill| {
            let group = POST_FILTER_GROUPS
                .get(drill.group)
                .unwrap_or(&POST_FILTER_GROUPS[0]);
            let filter = group
                .filters
                .get(drill.item)
                .copied()
                .unwrap_or(PostFilter::None);
            crate::video::native_presenter::NativeOverlayRingPickerDrill {
                title: "ポストフィルタ".to_string(),
                group_line: format!(
                    "{}/{}  {}",
                    drill.group + 1,
                    POST_FILTER_GROUPS.len(),
                    group.label
                ),
                item_line: format!(
                    "{}/{}  {}",
                    drill.item + 1,
                    group.filters.len(),
                    filter.display_label()
                ),
                selected_line: format!("選択中: {}", picker.post_filter.display_label()),
                footer: "上下:グループ  左右:項目  A/B:戻る  X:確定".to_string(),
            }
        });
        crate::video::native_presenter::NativeOverlayRingPicker {
            title: format!("X ピッカー / {}", picker.context.label()),
            rows,
            selected_row: picker.current_row(),
            footer: "方向:選択/変更  A:確定  B:取消  X:確定".to_string(),
            drill,
        }
    }

    fn commit_ring_picker(&mut self, ctx: &egui::Context) {
        let Some(picker) = self.ring_picker.take() else {
            return;
        };
        if picker.context == RingShortcutContext::VideoFullscreen {
            self.clear_native_video_picker_overlay(ctx);
        }
        self.gamepad_state.cancel_west_ring();
        self.apply_ring_picker_state(ctx, picker);
        ctx.request_repaint();
    }

    fn cancel_ring_picker(&mut self, ctx: &egui::Context) {
        let was_video_picker = self
            .ring_picker
            .as_ref()
            .is_some_and(|picker| picker.context == RingShortcutContext::VideoFullscreen);
        self.ring_picker = None;
        if was_video_picker {
            self.clear_native_video_picker_overlay(ctx);
        }
        self.gamepad_state.cancel_west_ring();
        ctx.request_repaint();
    }

    fn apply_ring_picker_state(&mut self, ctx: &egui::Context, picker: RingPickerState) {
        match picker.context {
            RingShortcutContext::Grid => self.apply_grid_picker_state(picker),
            RingShortcutContext::ImageFullscreen => {
                if let Some(fs_idx) = self.fullscreen_idx {
                    self.apply_image_picker_state(ctx, fs_idx, picker);
                }
            }
            RingShortcutContext::VideoFullscreen => {
                if let Some(fs_idx) = self.fullscreen_idx {
                    self.apply_video_picker_state(ctx, fs_idx, picker);
                }
            }
        }
    }

    fn apply_grid_picker_state(&mut self, picker: RingPickerState) {
        let mut settings_changed = false;
        if picker.dirty_rows.contains(&RingPickerRowId::GridColumns)
            && self.settings.grid_cols != picker.grid_cols
        {
            self.set_grid_view_mode(GridViewMode::Thumbnail);
            self.settings.grid_cols = picker.grid_cols;
            self.scroll_to_selected = true;
            self.bump_input_seq(
                "ring_picker_grid_cols",
                Some(&format!("cols={}", picker.grid_cols)),
            );
            settings_changed = true;
        }
        if picker.dirty_rows.contains(&RingPickerRowId::GridSortOrder)
            && self.settings.sort_order != picker.sort_order
        {
            self.settings.sort_order = picker.sort_order;
            self.rebuild_visible_indices();
            settings_changed = true;
        }
        if picker
            .dirty_rows
            .contains(&RingPickerRowId::GridThumbAspect)
            && (self.settings.thumb_aspect_auto != picker.thumb_aspect_auto
                || self.settings.thumb_aspect != picker.thumb_aspect)
        {
            self.apply_picker_thumb_aspect(picker.thumb_aspect_auto, picker.thumb_aspect);
            settings_changed = false;
        }
        if settings_changed {
            self.settings.save();
        }
        if picker.dirty_rows.contains(&RingPickerRowId::ItemRating) {
            self.apply_rating_to_selection(picker.item_rating);
        }
        if picker
            .dirty_rows
            .contains(&RingPickerRowId::ContainerRating)
            && self.current_folder_rating() != picker.container_rating
            && self.set_current_folder_rating(picker.container_rating)
        {
            self.show_container_rating_toast(picker.container_rating);
        }
    }

    fn apply_picker_thumb_aspect(&mut self, auto: bool, aspect: ThumbAspect) {
        if auto {
            let was_off = !self.settings.thumb_aspect_auto;
            let prev_effective = self.effective_thumb_aspect();
            self.settings.thumb_aspect_auto = true;
            self.auto_aspect.reset_decision_only();
            if was_off {
                self.rebuild_auto_aspect_samples_from_loaded();
            }
            self.maybe_apply_auto_aspect(true);
            let new_effective = self.effective_thumb_aspect();
            if prev_effective != new_effective {
                self.fixup_scroll_for_aspect_change(new_effective);
            }
        } else {
            if self.settings.thumb_aspect_auto || self.settings.thumb_aspect != aspect {
                self.fixup_scroll_for_aspect_change(aspect);
            }
            self.settings.thumb_aspect_auto = false;
            self.settings.thumb_aspect = aspect;
        }
        self.settings.save();
    }

    fn apply_image_picker_state(
        &mut self,
        ctx: &egui::Context,
        fs_idx: usize,
        picker: RingPickerState,
    ) {
        if self.vertical_reading_supported_idx(fs_idx) {
            if picker.dirty_rows.contains(&RingPickerRowId::SpreadMode)
                && self.spread_mode != picker.spread_mode
            {
                self.apply_fullscreen_spread_mode(ctx, fs_idx, picker.spread_mode);
            }
            if picker.dirty_rows.contains(&RingPickerRowId::ReadingFlow)
                && self.reading_flow != picker.reading_flow
            {
                self.set_reading_flow_for_fullscreen(ctx, fs_idx, picker.reading_flow);
            }
            let fit = picker.fit_mode.effective_for_flow(self.reading_flow);
            if picker.dirty_rows.contains(&RingPickerRowId::FitMode)
                && self
                    .settings
                    .fullscreen_fit_mode
                    .effective_for_flow(self.reading_flow)
                    != fit
            {
                self.set_fullscreen_fit_mode_for_current(ctx, fs_idx, fit);
            }
        }
        if picker.dirty_rows.contains(&RingPickerRowId::ItemRating) {
            self.apply_fullscreen_picker_rating(fs_idx, picker.item_rating);
        }
        if picker
            .dirty_rows
            .contains(&RingPickerRowId::ContainerRating)
            && self.current_folder_rating() != picker.container_rating
            && self.set_current_folder_rating(picker.container_rating)
        {
            self.show_container_rating_toast(picker.container_rating);
        }
        if self.reading_flow.is_paged() {
            if picker.dirty_rows.contains(&RingPickerRowId::PostFilter) {
                self.apply_picker_post_filter(fs_idx, picker.post_filter);
            }
            if picker.dirty_rows.contains(&RingPickerRowId::UpscaleModel) {
                self.apply_picker_upscale_model(fs_idx, picker.upscale_model_key);
            }
        }
    }

    fn apply_video_picker_state(
        &mut self,
        ctx: &egui::Context,
        fs_idx: usize,
        picker: RingPickerState,
    ) {
        if picker.dirty_rows.contains(&RingPickerRowId::VideoVolume)
            && (self.settings.video_volume - picker.video_volume).abs() > 1.0e-9
        {
            #[cfg(windows)]
            self.handle_native_video_set_volume_command(ctx, fs_idx, picker.video_volume, true);
            #[cfg(not(windows))]
            {
                self.settings.video_volume =
                    crate::settings::clamp_video_volume(picker.video_volume);
                self.settings.save();
            }
        }
        if picker
            .dirty_rows
            .contains(&RingPickerRowId::VideoPlaybackSpeed)
            && (self.video_playback_speed - picker.video_playback_speed).abs() > 1.0e-9
        {
            #[cfg(windows)]
            self.handle_video_playback_speed_command(ctx, fs_idx, picker.video_playback_speed);
            #[cfg(not(windows))]
            {
                let speed = crate::video::clock::clamp_playback_speed(picker.video_playback_speed);
                self.video_playback_speed = speed;
                self.settings.video_playback_speed = speed;
                self.settings.save();
            }
        }
        if picker
            .dirty_rows
            .contains(&RingPickerRowId::VideoContinuousMode)
            && self.video_continuous_mode != picker.video_continuous_mode
        {
            self.set_video_continuous_mode_common(ctx, fs_idx, picker.video_continuous_mode);
        }
    }

    fn apply_fullscreen_picker_rating(&mut self, fs_idx: usize, stars: u8) {
        let before = self.rating_cache.get(&fs_idx).copied().unwrap_or(0);
        if before == stars {
            return;
        }
        let summary = if stars == 0 {
            "★解除".to_string()
        } else {
            format!("★{stars}")
        };
        self.capture_rating_undo(vec![(fs_idx, before, stars)], summary);
        self.set_rating(fs_idx, stars);
        self.rebuild_visible_indices();
        if stars == 0 {
            self.show_feedback_toast("[★解除]".to_string());
        } else {
            self.show_feedback_toast(format!("[{}]", "★".repeat(stars as usize)));
        }
    }

    fn apply_picker_post_filter(&mut self, fs_idx: usize, next: PostFilter) {
        let scope = self.resolve_adjust_scope(fs_idx);
        let old_params = self.effective_params(fs_idx).clone();
        if old_params.post_filter == next {
            return;
        }
        let mut params = old_params.clone();
        params.post_filter = next;
        self.show_feedback_toast(format!(
            "[Picker: {} / {}]",
            scope.label(),
            next.display_label()
        ));
        self.capture_adjust_full(
            format!("ポストフィルタ: {}", next.display_label()),
            |app| {
                app.write_params_for_scope(fs_idx, scope, params.clone());
                match scope {
                    crate::ui_fullscreen::AdjustScope::PageOverride => {
                        app.clear_caches_for_param_change(fs_idx, &old_params, &params)
                    }
                    crate::ui_fullscreen::AdjustScope::FavoriteDefault(_)
                    | crate::ui_fullscreen::AdjustScope::Global => {}
                }
            },
        );
    }

    fn apply_picker_upscale_model(&mut self, fs_idx: usize, next: Option<String>) {
        let old_params = self.effective_params(fs_idx).clone();
        if old_params.upscale_model == next {
            return;
        }
        let scope = self.resolve_adjust_scope(fs_idx);
        let label = crate::adjustment::upscale_model_label(next.as_deref());
        let mut params = old_params;
        params.upscale_model = next;
        self.show_feedback_toast(format!(
            "[Picker: {}アップスケール {}]",
            scope.label(),
            label
        ));
        self.capture_adjust_full(format!("AI アップスケール: {label}"), |app| {
            app.write_params_for_scope(fs_idx, scope, params);
            app.clear_all_adjustment_and_ai_caches(fs_idx);
        });
    }

    fn trigger_gamepad_ring_action(
        &mut self,
        ctx: &egui::Context,
        direction: RingDirection,
    ) -> Option<AddressBarNav> {
        let context = self.current_ring_shortcut_context();
        self.trigger_ring_shortcut_action(ctx, context, direction)
    }

    pub(crate) fn trigger_ring_shortcut_action(
        &mut self,
        ctx: &egui::Context,
        context: RingShortcutContext,
        direction: RingDirection,
    ) -> Option<AddressBarNav> {
        let action = self
            .settings
            .ring_shortcuts
            .profile(context)
            .slots
            .get(direction.slot_index())
            .cloned()
            .unwrap_or_default();
        if !action.is_valid_for_context(context) {
            crate::logger::log(format!(
                "ring shortcut ignored invalid action={} context={context:?}",
                action.as_str()
            ));
            return None;
        }
        if matches!(action, RingActionId::None) {
            self.show_feedback_toast(format!("[{}: なし]", direction.label()));
            return None;
        }
        self.apply_ring_action(ctx, context, action)
    }

    pub(crate) fn current_ring_shortcut_context(&self) -> RingShortcutContext {
        if let Some(fs_idx) = self.fullscreen_idx {
            if self.current_fullscreen_is_video(fs_idx) {
                RingShortcutContext::VideoFullscreen
            } else {
                RingShortcutContext::ImageFullscreen
            }
        } else {
            RingShortcutContext::Grid
        }
    }

    pub(crate) fn apply_mouse_back_forward_button(
        &mut self,
        ctx: &egui::Context,
        forward: bool,
        source: &'static str,
    ) -> Option<AddressBarNav> {
        match self
            .settings
            .ring_shortcuts
            .mouse_back_forward_action
            .effective()
        {
            MouseBackForwardActionId::FolderHistoryPrevNext => {
                crate::logger::log(format!(
                    "[input-nav] source={source} action=folder_history_{}",
                    if forward { "forward" } else { "back" }
                ));
                if self.is_snapshot_active() || self.items_are_drive_list {
                    return None;
                }
                Some(if forward {
                    AddressBarNav::HistoryForward
                } else {
                    AddressBarNav::HistoryBack
                })
            }
            MouseBackForwardActionId::TreeFolderPrevNext => {
                crate::logger::log(format!(
                    "[input-nav] source={source} action=tree_folder_{}",
                    if forward { "forward" } else { "back" }
                ));
                self.apply_tree_folder_nav(ctx, forward, source);
                None
            }
            MouseBackForwardActionId::None | MouseBackForwardActionId::Unknown(_) => None,
        }
    }

    pub(crate) fn apply_configured_wheel_pair(
        &mut self,
        ctx: &egui::Context,
        pair: WheelPairActionId,
        wheel_up: bool,
        image_rect: Option<egui::Rect>,
        source: &'static str,
    ) -> bool {
        let context = self.current_ring_shortcut_context();
        if !pair.is_valid_for_context(context) {
            return false;
        }
        match pair {
            WheelPairActionId::None | WheelPairActionId::Unknown(_) => false,
            WheelPairActionId::FolderHistoryPrevNext => {
                self.mouse_ring_nav = self.apply_mouse_history_nav(wheel_up, source);
                true
            }
            WheelPairActionId::TreeFolderPrevNext => {
                self.apply_tree_folder_nav(ctx, !wheel_up, source);
                true
            }
            WheelPairActionId::SiblingFolderPrevNext => {
                self.apply_sibling_folder_nav(ctx, !wheel_up, source);
                true
            }
            WheelPairActionId::PageJumpPrevNext => {
                if let Some(fs_idx) = self.fullscreen_idx
                    && context == RingShortcutContext::ImageFullscreen
                {
                    self.apply_wheel_page_jump(ctx, fs_idx, !wheel_up);
                    true
                } else {
                    false
                }
            }
            WheelPairActionId::ZoomInOut => {
                if context != RingShortcutContext::ImageFullscreen || self.analysis_mode {
                    return false;
                }
                let Some(rect) = image_rect else {
                    return false;
                };
                let wheel_y = if wheel_up { 120.0 } else { -120.0 };
                let mouse = ctx.input(|i| i.pointer.hover_pos());
                let changed = Self::apply_wheel_zoom(
                    &mut self.fs_zoom,
                    &mut self.fs_pan,
                    wheel_y,
                    mouse,
                    rect.center(),
                );
                if changed {
                    self.maybe_rerender_pdf(self.fs_zoom);
                }
                true
            }
            WheelPairActionId::VideoVolumeUpDown => {
                if context == RingShortcutContext::VideoFullscreen {
                    self.apply_video_wheel_volume(ctx, wheel_up);
                    true
                } else {
                    false
                }
            }
            WheelPairActionId::VideoMarkerPrevNext => {
                if let Some(fs_idx) = self.fullscreen_idx
                    && context == RingShortcutContext::VideoFullscreen
                {
                    self.jump_native_video_marker(fs_idx, !wheel_up);
                    self.request_native_video_hud_repaint(ctx);
                    true
                } else {
                    false
                }
            }
        }
    }

    pub(crate) fn apply_shift_alt_wheel_pair(
        &mut self,
        ctx: &egui::Context,
        wheel_y: f32,
        shift: bool,
        alt: bool,
        image_rect: Option<egui::Rect>,
        source: &'static str,
    ) -> bool {
        if wheel_y.abs() <= 0.5 {
            return false;
        }
        let pair = if shift {
            self.settings.ring_shortcuts.shift_wheel_pair.clone()
        } else if alt {
            self.settings.ring_shortcuts.alt_wheel_pair.clone()
        } else {
            return false;
        };
        self.apply_configured_wheel_pair(ctx, pair, wheel_y > 0.0, image_rect, source)
    }

    fn apply_mouse_history_nav(
        &mut self,
        wheel_up: bool,
        source: &'static str,
    ) -> Option<AddressBarNav> {
        if self.is_snapshot_active() || self.items_are_drive_list {
            return None;
        }
        let nav = if wheel_up {
            AddressBarNav::HistoryBack
        } else {
            AddressBarNav::HistoryForward
        };
        crate::logger::log(format!(
            "[input-nav] source={source} action=wheel_folder_history_{}",
            if wheel_up { "back" } else { "forward" }
        ));
        Some(nav)
    }

    fn apply_tree_folder_nav(&mut self, ctx: &egui::Context, forward: bool, source: &'static str) {
        if let Some(fs_idx) = self.fullscreen_idx {
            let native_toast = self.current_fullscreen_is_video(fs_idx);
            self.bump_input_seq(
                source,
                Some(if forward { "tree_forward" } else { "tree_back" }),
            );
            self.handle_fullscreen_ctrl_nav_context(ctx, fs_idx, forward, native_toast);
        } else {
            self.handle_grid_tree_folder_nav(forward, source);
        }
    }

    fn handle_grid_tree_folder_nav(&mut self, forward: bool, source: &'static str) {
        self.bump_input_seq(
            source,
            Some(if forward { "tree_forward" } else { "tree_back" }),
        );
        let in_local_search = self.show_search_bar;
        let in_favsearch = self.favsearch.active;
        let in_global_search = self.global_search.active;
        let in_global_search_drilled = in_global_search && self.global_search.drill.is_some();
        let in_tag_view = self.tag_view.active;

        if self.is_snapshot_active() {
            let _ = self.snapshot_navigate_grid(forward);
        } else if in_global_search_drilled {
            self.global_search_ctrl_nav(forward);
        } else if in_global_search {
        } else if in_local_search {
            self.cancel_pending_folder_nav();
        } else if in_favsearch {
            self.favsearch_ctrl_nav(forward);
        } else if in_tag_view {
            self.cancel_pending_folder_nav();
        } else if self.zip_nav_handle_ctrl_updown(forward) {
        } else if let Some(cur) = self.effective_folder() {
            self.start_folder_nav(cur, forward, FolderNavMode::Grid);
        }
    }

    fn apply_sibling_folder_nav(
        &mut self,
        ctx: &egui::Context,
        forward: bool,
        source: &'static str,
    ) {
        if let Some(fs_idx) = self.fullscreen_idx {
            self.bump_input_seq(
                source,
                Some(if forward {
                    "sibling_forward"
                } else {
                    "sibling_back"
                }),
            );
            let native_toast = self.current_fullscreen_is_video(fs_idx);
            self.handle_fullscreen_sibling_nav_context(ctx, fs_idx, forward, native_toast);
        } else {
            self.bump_input_seq(
                source,
                Some(if forward {
                    "sibling_forward"
                } else {
                    "sibling_back"
                }),
            );
            if self.is_snapshot_active() {
                let _ = self.snapshot_navigate_grid_page(forward);
            } else if self.global_search.active || self.favsearch.active || self.tag_view.active {
            } else if self.show_search_bar {
                self.cancel_pending_folder_nav();
            } else if let Some(cur) = self.effective_folder() {
                self.start_folder_nav(cur, forward, FolderNavMode::SiblingGrid);
            }
        }
    }

    fn apply_wheel_page_jump(&mut self, ctx: &egui::Context, fs_idx: usize, forward: bool) {
        if let Some(new_idx) = self.fullscreen_large_jump_target(fs_idx, forward) {
            self.open_fullscreen_from_fs_navigation(ctx, new_idx);
        } else {
            self.fs_boundary_hint = Some(crate::ui_fullscreen::FsBoundaryHint::Edge {
                at_end: forward,
                at: Instant::now(),
            });
        }
    }

    fn apply_video_wheel_volume(&mut self, ctx: &egui::Context, wheel_up: bool) {
        let Some(fs_idx) = self.fullscreen_idx else {
            return;
        };
        let delta = if wheel_up { 1 } else { -1 };
        let volume = self
            .fs_video_player(fs_idx)
            .map(|player| player.volume())
            .unwrap_or(self.settings.video_volume);
        let next = step_video_volume_by_fader_key_step(volume, delta);
        self.handle_native_video_set_volume_command(ctx, fs_idx, next, true);
        self.request_native_video_hud_repaint(ctx);
    }

    fn apply_ring_action(
        &mut self,
        ctx: &egui::Context,
        context: RingShortcutContext,
        action: RingActionId,
    ) -> Option<AddressBarNav> {
        match action {
            RingActionId::None | RingActionId::Unknown(_) => None,
            RingActionId::ToggleDetachedViewer => {
                self.toggle_detached_viewer_mode();
                None
            }
            RingActionId::CycleFavorite => self.handle_gamepad_start(),
            RingActionId::AddToBook => {
                self.apply_ring_add_to_book(ctx, context);
                None
            }
            RingActionId::PinRepresentativeThumb => {
                self.apply_ring_pin_representative_thumb(ctx, context);
                None
            }
            RingActionId::GridToggleDetails if context == RingShortcutContext::Grid => {
                self.toggle_grid_details_view();
                None
            }
            RingActionId::GridToggleSnapshotLock if context == RingShortcutContext::Grid => {
                if let Some(reason) = self.snapshot_button_disabled_reason() {
                    self.show_feedback_toast(reason.to_string());
                } else {
                    let label = self.infer_snapshot_source_label();
                    self.toggle_snapshot(label);
                }
                None
            }
            RingActionId::GridToggleCheck if context == RingShortcutContext::Grid => {
                self.toggle_selected_grid_check();
                None
            }
            RingActionId::GridSelectAll if context == RingShortcutContext::Grid => {
                for &idx in &self.visible_indices {
                    if self.items.get(idx).is_some_and(|it| it.is_checkable()) {
                        self.checked.insert(idx);
                    }
                }
                None
            }
            RingActionId::GridHistoryBack if context == RingShortcutContext::Grid => self
                .navigate_folder_history_back()
                .map(AddressBarNav::Direct),
            RingActionId::GridHistoryForward if context == RingShortcutContext::Grid => self
                .navigate_folder_history_forward()
                .map(AddressBarNav::Direct),
            RingActionId::GridParentFolder if context == RingShortcutContext::Grid => {
                self.handle_gamepad_grid_back()
            }
            RingActionId::ImageRotateLeft if context == RingShortcutContext::ImageFullscreen => {
                if let Some(fs_idx) = self.fullscreen_idx
                    && !self.current_fullscreen_spread_is_double(fs_idx)
                {
                    self.rotate_image_ccw(fs_idx);
                }
                None
            }
            RingActionId::ImageRotateRight if context == RingShortcutContext::ImageFullscreen => {
                if let Some(fs_idx) = self.fullscreen_idx
                    && !self.current_fullscreen_spread_is_double(fs_idx)
                {
                    self.rotate_image_cw(fs_idx);
                }
                None
            }
            RingActionId::ImageCapture if context == RingShortcutContext::ImageFullscreen => {
                if let Some(fs_idx) = self.fullscreen_idx {
                    self.save_image_capture_to_file(ctx, fs_idx);
                }
                None
            }
            RingActionId::ImageToggleMetadata
                if context == RingShortcutContext::ImageFullscreen =>
            {
                if let Some(fs_idx) = self.fullscreen_idx
                    && !self.current_fullscreen_spread_is_double(fs_idx)
                {
                    self.show_metadata_panel = !self.show_metadata_panel;
                    self.metadata_panel_hover_active = false;
                }
                None
            }
            RingActionId::ImageSlideshow if context == RingShortcutContext::ImageFullscreen => {
                self.toggle_ring_slideshow();
                None
            }
            RingActionId::ImagePixelGrid if context == RingShortcutContext::ImageFullscreen => {
                self.fs_pixel_grid_enabled = !self.fs_pixel_grid_enabled;
                self.show_feedback_toast(if self.fs_pixel_grid_enabled {
                    "[ピクセルグリッド ON]".to_string()
                } else {
                    "[ピクセルグリッド OFF]".to_string()
                });
                None
            }
            RingActionId::ImageBackgroundCycle
                if context == RingShortcutContext::ImageFullscreen =>
            {
                if let Some(fs_idx) = self.fullscreen_idx {
                    self.cycle_ring_transparent_background(fs_idx);
                }
                None
            }
            RingActionId::ImageComparePin if context == RingShortcutContext::ImageFullscreen => {
                if let Some(fs_idx) = self.fullscreen_idx {
                    self.toggle_compare_pin_from_current(ctx, fs_idx);
                }
                None
            }
            RingActionId::VideoCapture if context == RingShortcutContext::VideoFullscreen => {
                if let Some(fs_idx) = self.fullscreen_idx {
                    self.save_video_frame_to_file(ctx, fs_idx);
                }
                None
            }
            RingActionId::VideoMute if context == RingShortcutContext::VideoFullscreen => {
                if let Some(fs_idx) = self.fullscreen_idx
                    && self.toggle_video_session_mute_for_fs_idx(fs_idx)
                {
                    self.request_native_video_hud_repaint(ctx);
                }
                None
            }
            RingActionId::VideoLoop if context == RingShortcutContext::VideoFullscreen => {
                if let Some(fs_idx) = self.fullscreen_idx {
                    self.cycle_native_video_loop_common(ctx, fs_idx);
                }
                None
            }
            RingActionId::VideoBookmark if context == RingShortcutContext::VideoFullscreen => {
                self.add_ring_video_bookmark(ctx);
                None
            }
            RingActionId::VideoTileMode if context == RingShortcutContext::VideoFullscreen => {
                self.toggle_ring_video_tile_mode(ctx);
                None
            }
            RingActionId::VideoExternalPlayer
                if context == RingShortcutContext::VideoFullscreen =>
            {
                if let Some(fs_idx) = self.fullscreen_idx
                    && let Some(GridItem::Video(path)) = self.items.get(fs_idx)
                {
                    crate::ui_helpers::open_external_player(path);
                }
                None
            }
            _ => None,
        }
    }

    fn apply_ring_add_to_book(&mut self, ctx: &egui::Context, context: RingShortcutContext) {
        match context {
            RingShortcutContext::Grid => self.add_grid_selection_to_active_book(ctx),
            RingShortcutContext::ImageFullscreen => {
                if let Some(fs_idx) = self.fullscreen_idx {
                    self.add_fullscreen_image_to_active_book(ctx, fs_idx);
                }
            }
            RingShortcutContext::VideoFullscreen => {
                if let Some(fs_idx) = self.fullscreen_idx {
                    self.add_current_video_frame_to_active_book(ctx, fs_idx);
                }
            }
        }
    }

    fn apply_ring_pin_representative_thumb(
        &mut self,
        ctx: &egui::Context,
        context: RingShortcutContext,
    ) {
        match context {
            RingShortcutContext::Grid => self.toggle_folder_pin_from_selection(),
            RingShortcutContext::ImageFullscreen => {
                if let Some(fs_idx) = self.fullscreen_idx {
                    self.toggle_folder_pin_for_idx(fs_idx);
                }
            }
            RingShortcutContext::VideoFullscreen => self.pin_ring_video_frame(ctx),
        }
    }

    fn toggle_selected_grid_check(&mut self) {
        let Some(idx) = self.selected else {
            return;
        };
        if self.checked.contains(&idx) {
            self.checked.remove(&idx);
        } else if self.items.get(idx).is_some_and(|it| it.is_checkable()) {
            self.checked.insert(idx);
        }
    }

    fn current_fullscreen_spread_is_double(&mut self, fs_idx: usize) -> bool {
        matches!(
            self.resolve_spread_pair(fs_idx),
            crate::ui_fullscreen::SpreadPair::Double { .. }
        )
    }

    fn toggle_ring_slideshow(&mut self) {
        let Some(fs_idx) = self.fullscreen_idx else {
            return;
        };
        if self.slideshow_playing {
            self.slideshow_playing = false;
            self.slideshow_anchor_idx = None;
        } else if matches!(
            self.items.get(fs_idx),
            Some(GridItem::Image(_))
                | Some(GridItem::ZipImage { .. })
                | Some(GridItem::PdfPage { .. })
        ) {
            self.slideshow_playing = true;
            self.schedule_next_slideshow_from_now();
        }
    }

    fn cycle_ring_transparent_background(&mut self, fs_idx: usize) {
        let idxs: Vec<usize> = match self.resolve_spread_pair(fs_idx) {
            crate::ui_fullscreen::SpreadPair::Double { left, right } => vec![left, right],
            crate::ui_fullscreen::SpreadPair::Single => vec![fs_idx],
        };
        if !idxs.iter().any(|&i| self.fs_image_has_alpha(i)) {
            self.show_feedback_toast("透過画像ではないため背景は切り替えできません".to_string());
            return;
        }
        let modulo: u8 = if self.ai_upscale_enabled { 2 } else { 3 };
        self.fs_transparent_bg_mode = (self.fs_transparent_bg_mode + 1) % modulo;
        self.fs_transparent_bg_indicator_until =
            Some(std::time::Instant::now() + std::time::Duration::from_millis(1200));
        self.show_feedback_toast(
            crate::ui_fullscreen::transparent_bg_toast(self.fs_transparent_bg_mode).to_string(),
        );
        if self.ai_upscale_enabled {
            for idx in idxs {
                self.clear_adjustment_caches(idx);
            }
        }
    }

    fn pin_ring_video_frame(&mut self, ctx: &egui::Context) {
        #[cfg(windows)]
        if let Some(fs_idx) = self.fullscreen_idx {
            if self.video_tile_mode_active {
                let _ = self.handle_native_video_set_tile_pin_command(ctx, fs_idx);
            } else {
                let target = self
                    .fs_video_player(fs_idx)
                    .map(|p| p.position())
                    .unwrap_or(0.0);
                self.handle_native_video_set_pin_command(ctx, fs_idx, target);
            }
        }
        #[cfg(not(windows))]
        let _ = ctx;
    }

    fn add_ring_video_bookmark(&mut self, ctx: &egui::Context) {
        #[cfg(windows)]
        if let Some(fs_idx) = self.fullscreen_idx {
            self.add_native_video_bookmark(fs_idx, None);
            self.request_native_video_hud_repaint(ctx);
        }
        #[cfg(not(windows))]
        let _ = ctx;
    }

    fn toggle_ring_video_tile_mode(&mut self, ctx: &egui::Context) {
        #[cfg(windows)]
        if let Some(fs_idx) = self.fullscreen_idx {
            let screen = self.video_tile_layout_size(fs_idx, ctx);
            self.toggle_video_tile_mode(fs_idx, screen);
            self.sync_native_video_tile_overlay(ctx, fs_idx);
        }
        #[cfg(not(windows))]
        let _ = ctx;
    }

    fn handle_gamepad_select(&mut self, ctx: &egui::Context) {
        let Some(fs_idx) = self.fullscreen_idx else {
            return;
        };
        // 見開きモード (キーボードのショートカット 1〜5) を巡回トグルする。
        // Single → Ltr → LtrCover → Rtl → RtlCover → Single … の順。
        // 連結方式 (reading_flow) の切替は key_6 / 別操作の役割であり、Select ではない。
        // 対応アイテム (画像/ZIP画像/PDFページ) のときだけ。動画/非対応アイテム上で
        // 切り替えると、見えないモードがフォルダに永続化されてナビや各モードキーが壊れる。
        if !self.vertical_reading_supported_idx(fs_idx) {
            return;
        }
        let next = self.spread_mode.next_in_spread_cycle();
        self.apply_fullscreen_spread_mode(ctx, fs_idx, next);
        self.show_feedback_toast(format!("[Pad:{}]", next.label()));
    }

    fn handle_gamepad_y_tap(&mut self, ctx: &egui::Context) {
        let Some(fs_idx) = self.fullscreen_idx else {
            // 非フルスクリーン: Y でツリーをトグル。閉じる時はカーソルが別フォルダへ
            // 動いていれば Enter 相当でそこへ移動して閉じる
            // (`toggle_folder_tree_pane_from_key`)。
            if self.folder_pane_disabled() && !self.settings.folder_tree_pane_visible {
                // 非表示→表示しようとした: スナップショット中は移動不可なので拒否。
                self.show_feedback_toast(
                    "スナップショット中は他のフォルダに移動できません".to_string(),
                );
                return;
            }
            self.toggle_folder_tree_pane_from_key();
            return;
        };
        if self.current_fullscreen_is_video(fs_idx) {
            self.dispatch_native_video_key(ctx, fs_idx, 0x53, false, false, false);
        }
    }

    fn handle_gamepad_direction(&mut self, ctx: &egui::Context, dir: PadDir, repeat: bool) {
        if self.gamepad_state.button_down(PadButton::North) {
            self.gamepad_state.mark_y_modifier_used();
            if let Some(fs_idx) = self.fullscreen_idx
                && !self.current_fullscreen_is_video(fs_idx)
            {
                self.handle_gamepad_spread_nudge(ctx, fs_idx, dir);
            } else if let Some(fs_idx) = self.fullscreen_idx {
                self.handle_gamepad_video_y_direction(ctx, fs_idx, dir, repeat);
            } else {
                self.handle_gamepad_direction_for_grid(dir);
            }
            return;
        }

        if let Some(fs_idx) = self.fullscreen_idx {
            if self.current_fullscreen_is_video(fs_idx) {
                self.handle_gamepad_video_direction(ctx, fs_idx, dir, repeat);
            } else {
                self.handle_gamepad_still_direction(ctx, fs_idx, dir);
            }
        } else {
            self.handle_gamepad_direction_for_grid(dir);
        }
    }

    fn handle_gamepad_direction_for_grid(&mut self, dir: PadDir) {
        let display_order = self.current_grid_order().to_vec();
        let vi_len = display_order.len();
        if vi_len == 0 {
            return;
        }
        let cols = self.settings.grid_cols.max(1);
        let details_mode = self.settings.grid_view_mode == crate::settings::GridViewMode::Details;
        let cell_h = self.last_cell_h.max(1.0);
        let visible_rows = (self.last_viewport_h / cell_h).floor() as usize;
        let sel = self
            .selected
            .unwrap_or_else(|| display_order.first().copied().unwrap_or(0));
        let vis_pos = display_order
            .iter()
            .position(|&idx| idx == sel)
            .unwrap_or(0);
        let new_pos =
            gamepad_grid_nav_target_pos(vis_pos, vi_len, cols, visible_rows, details_mode, dir);
        let Some(new_sel) = display_order.get(new_pos).copied() else {
            return;
        };
        if self.selected != Some(new_sel) {
            self.selected = Some(new_sel);
            self.scroll_to_selected = true;
            self.update_last_selected_image();
            self.bump_input_seq("gamepad_grid_nav", Some(&format!("sel={new_sel}")));
        }
    }

    fn handle_gamepad_folder_tree_direction(&mut self, dir: PadDir) -> bool {
        if !self.folder_pane_blocks_grid_keyboard() {
            return false;
        }
        let key = match dir {
            PadDir::Up => FolderPaneTreeKey::Up,
            PadDir::Down => FolderPaneTreeKey::Down,
            PadDir::Left => FolderPaneTreeKey::Left,
            PadDir::Right => FolderPaneTreeKey::Right,
        };
        let _ = self
            .folder_pane
            .handle_tree_key(key, self.settings.sort_order);
        true
    }

    fn handle_gamepad_still_direction(&mut self, ctx: &egui::Context, fs_idx: usize, dir: PadDir) {
        // 連続読みのスクロールは、レンダラが連続描画しているときだけ。そうでなければ
        // (解析/比較/オーバーレイ編集中など) 下のページ送りへフォールバックさせる。
        let continuous_active = self.continuous_reading_active_for_idx(fs_idx);
        if continuous_active && self.reading_flow.is_vertical() {
            match dir {
                PadDir::Down => {
                    self.scroll_vertical_reading_step(ctx, 1.0);
                    return;
                }
                PadDir::Up => {
                    self.scroll_vertical_reading_step(ctx, -1.0);
                    return;
                }
                PadDir::Left | PadDir::Right => {}
            }
        } else if continuous_active && self.reading_flow.is_horizontal() {
            let axis_rtl = self.reading_direction == ReadingDirection::Rtl;
            match dir {
                PadDir::Right if !axis_rtl => {
                    self.scroll_vertical_reading_step(ctx, 1.0);
                    return;
                }
                PadDir::Left if axis_rtl => {
                    self.scroll_vertical_reading_step(ctx, 1.0);
                    return;
                }
                PadDir::Left if !axis_rtl => {
                    self.scroll_vertical_reading_step(ctx, -1.0);
                    return;
                }
                PadDir::Right if axis_rtl => {
                    self.scroll_vertical_reading_step(ctx, -1.0);
                    return;
                }
                PadDir::Up | PadDir::Down | PadDir::Left | PadDir::Right => {}
            }
        }
        let rtl = self.spread_mode.is_rtl();
        let base_delta = match dir {
            PadDir::Right if !rtl => 1,
            PadDir::Right => -1,
            PadDir::Left if !rtl => -1,
            PadDir::Left => 1,
            PadDir::Down => 1,
            PadDir::Up => -1,
        };
        self.navigate_gamepad_still(ctx, fs_idx, base_delta);
    }

    fn navigate_gamepad_still(&mut self, ctx: &egui::Context, fs_idx: usize, base_delta: i32) {
        let nav_delta = self.spread_nav_delta(base_delta);
        self.bump_input_seq("gamepad_fs_nav", Some(&format!("delta={nav_delta}")));
        self.handle_fs_navigation(ctx, false, false, None, None, None, nav_delta, None, fs_idx);
    }

    fn handle_gamepad_spread_nudge(&mut self, ctx: &egui::Context, fs_idx: usize, dir: PadDir) {
        let rtl = self.spread_mode.is_rtl();
        let nudge_dir = match dir {
            PadDir::Right if !rtl => Some(1),
            PadDir::Right => Some(-1),
            PadDir::Left if !rtl => Some(-1),
            PadDir::Left => Some(1),
            PadDir::Up | PadDir::Down => None,
        };
        let Some(nudge_dir) = nudge_dir else {
            return;
        };
        if self.spread_mode.is_spread() {
            if let Some((new_idx, new_mode)) = self.compute_spread_offset_nudge(fs_idx, nudge_dir) {
                self.spread_mode = new_mode;
                self.update_reading_direction_from_spread_mode(new_mode);
                self.persist_current_spread_mode();
                self.persist_current_reading_flow();
                self.adjust_spread_target = AdjustSpreadTarget::Left;
                self.bump_input_seq("gamepad_fs_nudge", Some(&format!("idx={new_idx}")));
                self.handle_fs_navigation(
                    ctx,
                    false,
                    false,
                    None,
                    None,
                    None,
                    0,
                    Some(new_idx),
                    fs_idx,
                );
                self.show_feedback_toast("見開きを1ページずらしました".to_string());
            }
        } else {
            self.navigate_gamepad_still(ctx, fs_idx, nudge_dir);
        }
    }

    fn handle_gamepad_video_y_direction(
        &mut self,
        ctx: &egui::Context,
        fs_idx: usize,
        dir: PadDir,
        repeat: bool,
    ) {
        match dir {
            PadDir::Left => self.dispatch_native_video_key(ctx, fs_idx, 0x4A, false, false, repeat),
            PadDir::Right => {
                self.dispatch_native_video_key(ctx, fs_idx, 0x4B, false, false, repeat)
            }
            PadDir::Up | PadDir::Down => {
                self.handle_gamepad_video_direction(ctx, fs_idx, dir, repeat)
            }
        }
    }

    fn handle_gamepad_video_direction(
        &mut self,
        ctx: &egui::Context,
        fs_idx: usize,
        dir: PadDir,
        repeat: bool,
    ) {
        let vk = match dir {
            PadDir::Left => 0x25,
            PadDir::Up => 0x26,
            PadDir::Right => 0x27,
            PadDir::Down => 0x28,
        };
        self.dispatch_native_video_key(ctx, fs_idx, vk, false, false, repeat);
    }

    fn handle_gamepad_start(&mut self) -> Option<AddressBarNav> {
        if self.is_snapshot_active() {
            self.show_feedback_toast(
                "スナップショット中は他のフォルダに移動できません".to_string(),
            );
            return None;
        }
        let target = self.next_gamepad_favorite_path()?;
        self.bump_input_seq(
            "gamepad_favorite_nav",
            Some(&format!("path={}", target.display())),
        );
        Some(AddressBarNav::Direct(target))
    }

    fn next_gamepad_favorite_path(&mut self) -> Option<PathBuf> {
        if self.settings.favorites.is_empty() {
            self.show_feedback_toast("お気に入りが登録されていません".to_string());
            return None;
        }

        let current_favorite_id = self.effective_folder().and_then(|path| {
            self.find_nearest_favorite(&path)
                .map(|favorite| favorite.id)
        });
        let current_index = current_favorite_id.and_then(|id| {
            self.settings
                .favorites
                .iter()
                .position(|favorite| favorite.id == id)
        });
        let next_index = current_index
            .map(|index| (index + 1) % self.settings.favorites.len())
            .unwrap_or(0);
        Some(self.settings.favorites[next_index].path.clone())
    }

    fn handle_gamepad_folder_nav(&mut self, ctx: &egui::Context, forward: bool) {
        if let Some(fs_idx) = self.fullscreen_idx {
            let native_toast = self.current_fullscreen_is_video(fs_idx);
            self.bump_input_seq(
                "gamepad_fs_folder_nav",
                Some(if forward { "forward" } else { "backward" }),
            );
            self.handle_fullscreen_ctrl_nav_context(ctx, fs_idx, forward, native_toast);
            return;
        }
        self.handle_gamepad_grid_folder_nav(forward);
    }

    fn handle_gamepad_grid_folder_nav(&mut self, forward: bool) {
        self.bump_input_seq(
            "gamepad_grid_folder_nav",
            Some(if forward { "forward" } else { "backward" }),
        );
        let in_local_search = self.show_search_bar;
        let in_favsearch = self.favsearch.active;
        let in_global_search = self.global_search.active;
        let in_global_search_drilled = in_global_search && self.global_search.drill.is_some();

        if self.is_snapshot_active() {
            let _ = self.snapshot_navigate_grid(forward);
        } else if in_global_search_drilled {
            self.global_search_ctrl_nav(forward);
        } else if in_global_search {
        } else if in_local_search {
            self.cancel_pending_folder_nav();
        } else if in_favsearch {
            self.favsearch_ctrl_nav(forward);
        } else if self.zip_nav_handle_ctrl_updown(forward) {
            // ネスト ZIP 内: ツリーを DFS で前後のノードへ (#4 改)。端では false → 下で ZIP を抜ける。
        } else if let Some(cur) = self.effective_folder() {
            self.start_folder_nav(cur, forward, FolderNavMode::Grid);
        }
    }

    fn handle_gamepad_grid_accept(&mut self) -> Option<AddressBarNav> {
        if self.folder_pane_blocks_grid_keyboard() {
            if let Some(FolderPaneCommand::Open(path)) = self
                .folder_pane
                .handle_tree_key(FolderPaneTreeKey::Enter, self.settings.sort_order)
            {
                self.settings.folder_tree_pane_visible = false;
                self.folder_pane.set_focus_grid();
                self.settings.save();
                // worker scan 経由で開く (UI スレッドの read_dir ブロック回避)。
                self.start_folder_pane_open(path);
                return None;
            }
            return None;
        }
        let idx = self.selected?;
        let item = self.items.get(idx).cloned();
        match item {
            Some(GridItem::Folder(p)) | Some(GridItem::ZipFile(p)) | Some(GridItem::PdfFile(p)) => {
                if self.settings.auto_fullscreen_zip_pdf
                    && matches!(
                        self.items.get(idx),
                        Some(GridItem::ZipFile(_)) | Some(GridItem::PdfFile(_))
                    )
                {
                    self.pending_auto_fs_open = true;
                }
                self.maybe_suppress_rating_filter_for_opened_container(idx);
                self.maybe_suppress_facet_filter_for_opened_container(idx);
                Some(AddressBarNav::Direct(p))
            }
            Some(GridItem::Image(_))
            | Some(GridItem::ZipImage { .. })
            | Some(GridItem::ZipSeparator { .. })
            | Some(GridItem::PdfPage { .. })
            | Some(GridItem::Video(_)) => {
                self.bump_input_seq_for_item("gamepad_grid_open", idx);
                self.fs_open_intent_from_grid = true;
                self.open_fullscreen(idx);
                None
            }
            Some(GridItem::ConvertibleArchive { path, format }) => {
                let auto_fs = self.settings.auto_fullscreen_zip_pdf;
                self.maybe_suppress_rating_filter_for_opened_container(idx);
                self.maybe_suppress_facet_filter_for_opened_container(idx);
                if let Some(cached) = self.try_archive_cache_lookup(&path) {
                    self.open_archive_via_cache(path, cached, auto_fs);
                } else {
                    self.request_archive_convert(path, format, auto_fs);
                }
                None
            }
            Some(GridItem::SearchContainer { path, kind, .. }) => {
                let is_zip = matches!(kind, crate::grid_item::SearchContainerKind::Zip);
                self.maybe_suppress_rating_filter_for_opened_container_path(&path);
                self.maybe_suppress_facet_filter_for_opened_container_path(&path);
                self.drill_into_container(path, is_zip);
                None
            }
            // ネスト ZIP ツリーの子コンテナへ降りる (Phase 3)。
            Some(GridItem::ZipDir { dir_prefix, .. }) => {
                // ★付きの本を絞り込み中に開くと中身が空表示になるのを防ぐ (Codex P2)。
                self.maybe_suppress_rating_filter_for_opened_zip_book(idx);
                self.maybe_suppress_facet_filter_for_opened_zip_book(idx);
                self.zip_nav_enter(&dir_prefix);
                None
            }
            None => None,
        }
    }

    pub(crate) fn handle_gamepad_grid_back(&mut self) -> Option<AddressBarNav> {
        if self.is_snapshot_active() && self.snapshot_return_to_list_view() {
            return None;
        }
        if self.global_search.active {
            if self.global_search.drill.is_some() {
                self.drill_back_one_level();
            }
            return None;
        }
        if self.favsearch.active {
            self.favsearch_back();
            return None;
        }
        if self.local_search_blocks_parent_nav() {
            self.cancel_pending_folder_nav();
            return None;
        }
        // ネスト ZIP ツリー内なら 1 階層戻る (ルートなら false → 親フォルダへ抜ける)。
        if self.zip_nav_back() {
            return None;
        }
        self.resolve_grid_parent_nav()
    }

    fn scroll_gamepad_grid(&mut self, direction: f32) {
        let cell_h = self.last_cell_h.max(1.0);
        let prev_offset = self.scroll_offset_y;
        self.scroll_offset_y = (self.scroll_offset_y + direction * cell_h * 2.0).max(0.0);
        self.scroll_offset_y = (self.scroll_offset_y / cell_h).round() * cell_h;
        if (self.scroll_offset_y - prev_offset).abs() > 0.5 {
            self.bump_input_seq(
                "gamepad_grid_scroll",
                Some(&format!("offset={:.0}", self.scroll_offset_y)),
            );
        }
    }

    fn current_fullscreen_is_video(&self, fs_idx: usize) -> bool {
        matches!(self.items.get(fs_idx), Some(GridItem::Video(_)))
    }

    #[cfg(windows)]
    fn dispatch_native_video_key(
        &mut self,
        ctx: &egui::Context,
        fs_idx: usize,
        virtual_key: u32,
        shift: bool,
        ctrl: bool,
        repeat: bool,
    ) {
        let key = crate::video::native_window::NativeVideoKeyEvent {
            virtual_key,
            shift,
            ctrl,
            alt: false,
            repeat,
        };
        self.handle_native_video_key_event(ctx, fs_idx, key);
    }

    #[cfg(not(windows))]
    fn dispatch_native_video_key(
        &mut self,
        _ctx: &egui::Context,
        _fs_idx: usize,
        _virtual_key: u32,
        _shift: bool,
        _ctrl: bool,
        _repeat: bool,
    ) {
    }
}

fn gamepad_grid_nav_target_pos(
    vis_pos: usize,
    vi_len: usize,
    cols: usize,
    visible_rows: usize,
    details_mode: bool,
    dir: PadDir,
) -> usize {
    if vi_len == 0 {
        return 0;
    }
    let last_pos = vi_len - 1;
    if details_mode {
        let page_items = visible_rows.max(1);
        match dir {
            PadDir::Right => (vis_pos + page_items).min(last_pos),
            PadDir::Left => vis_pos.saturating_sub(page_items),
            PadDir::Down => (vis_pos + 1).min(last_pos),
            PadDir::Up => vis_pos.saturating_sub(1),
        }
    } else {
        let cols = cols.max(1);
        match dir {
            PadDir::Right => (vis_pos + 1).min(last_pos),
            PadDir::Left => vis_pos.saturating_sub(1),
            PadDir::Down => (vis_pos + cols).min(last_pos),
            PadDir::Up => vis_pos.saturating_sub(cols),
        }
    }
}

const REPEAT_BUTTONS: [PadButton; 6] = [
    PadButton::DPadUp,
    PadButton::DPadDown,
    PadButton::DPadLeft,
    PadButton::DPadRight,
    PadButton::LeftShoulder,
    PadButton::RightShoulder,
];

fn repeat_interval_for_button(button: PadButton) -> Duration {
    match button {
        PadButton::LeftShoulder | PadButton::RightShoulder => SHOULDER_REPEAT_INTERVAL,
        _ => BUTTON_REPEAT_INTERVAL,
    }
}

fn button_dir(button: PadButton) -> Option<PadDir> {
    match button {
        PadButton::DPadUp => Some(PadDir::Up),
        PadButton::DPadDown => Some(PadDir::Down),
        PadButton::DPadLeft => Some(PadDir::Left),
        PadButton::DPadRight => Some(PadDir::Right),
        _ => None,
    }
}

fn ring_direction_from_pad_dir(dir: PadDir) -> RingDirection {
    match dir {
        PadDir::Up => RingDirection::Up,
        PadDir::Down => RingDirection::Down,
        PadDir::Left => RingDirection::Left,
        PadDir::Right => RingDirection::Right,
    }
}

fn ring_direction_unit(direction: RingDirection) -> egui::Vec2 {
    const D: f32 = std::f32::consts::FRAC_1_SQRT_2;
    match direction {
        RingDirection::Up => egui::vec2(0.0, -1.0),
        RingDirection::UpRight => egui::vec2(D, -D),
        RingDirection::Right => egui::vec2(1.0, 0.0),
        RingDirection::DownRight => egui::vec2(D, D),
        RingDirection::Down => egui::vec2(0.0, 1.0),
        RingDirection::DownLeft => egui::vec2(-D, D),
        RingDirection::Left => egui::vec2(-1.0, 0.0),
        RingDirection::UpLeft => egui::vec2(-D, -D),
    }
}

fn ring_direction_short_label(direction: RingDirection) -> &'static str {
    match direction {
        RingDirection::Up => "上",
        RingDirection::UpRight => "右上",
        RingDirection::Right => "右",
        RingDirection::DownRight => "右下",
        RingDirection::Down => "下",
        RingDirection::DownLeft => "左下",
        RingDirection::Left => "左",
        RingDirection::UpLeft => "左上",
    }
}

fn deadzone(value: f32) -> f32 {
    if value.abs() < DEADZONE { 0.0 } else { value }
}

fn stick_pair(state: &GamepadInputState, x_axis: PadAxis, y_axis: PadAxis) -> egui::Vec2 {
    egui::vec2(deadzone(state.axis(x_axis)), deadzone(state.axis(y_axis)))
}

fn dominant_stick_dir(stick: egui::Vec2) -> Option<PadDir> {
    if stick.length_sq() == 0.0 {
        return None;
    }
    if stick.x.abs() >= stick.y.abs() {
        if stick.x > 0.0 {
            Some(PadDir::Right)
        } else {
            Some(PadDir::Left)
        }
    } else if stick.y > 0.0 {
        Some(PadDir::Up)
    } else {
        Some(PadDir::Down)
    }
}

fn ring_direction_from_stick(stick: egui::Vec2) -> Option<RingDirection> {
    if stick.length_sq() == 0.0 {
        return None;
    }
    let degrees = stick.y.atan2(stick.x).to_degrees();
    Some(if (-22.5..22.5).contains(&degrees) {
        RingDirection::Right
    } else if (22.5..67.5).contains(&degrees) {
        RingDirection::UpRight
    } else if (67.5..112.5).contains(&degrees) {
        RingDirection::Up
    } else if (112.5..157.5).contains(&degrees) {
        RingDirection::UpLeft
    } else if degrees >= 157.5 || degrees < -157.5 {
        RingDirection::Left
    } else if (-157.5..-112.5).contains(&degrees) {
        RingDirection::DownLeft
    } else if (-112.5..-67.5).contains(&degrees) {
        RingDirection::Down
    } else {
        RingDirection::DownRight
    })
}

fn mouse_flick_direction(flick: &MouseFlickState) -> Option<RingDirection> {
    let delta = flick.current_pos - flick.start_pos;
    if delta.length() < MOUSE_FLICK_MOVE_THRESHOLD_PX {
        return None;
    }
    ring_direction_from_stick(egui::vec2(delta.x, -delta.y))
}

fn continuous_reading_stick_axis(
    reading_flow: ReadingFlow,
    reading_direction: ReadingDirection,
    stick: egui::Vec2,
) -> Option<f32> {
    let axis = if reading_flow.is_vertical() {
        -stick.y
    } else if reading_flow.is_horizontal() {
        if reading_direction == ReadingDirection::Rtl {
            -stick.x
        } else {
            stick.x
        }
    } else {
        return None;
    };
    (axis.abs() > 0.0).then_some(axis)
}

fn picker_rows_for_context(context: RingShortcutContext) -> &'static [RingPickerRowId] {
    match context {
        RingShortcutContext::Grid => GRID_PICKER_ROWS,
        RingShortcutContext::ImageFullscreen => IMAGE_PICKER_ROWS,
        RingShortcutContext::VideoFullscreen => VIDEO_PICKER_ROWS,
    }
}

fn cycle_index(len: usize, current: usize, delta: i32) -> usize {
    if len == 0 {
        return 0;
    }
    let current = current.min(len - 1) as i32;
    (current + delta).rem_euclid(len as i32) as usize
}

fn cycle_value<T: Copy + PartialEq>(values: &[T], current: T, delta: i32) -> T {
    if values.is_empty() {
        return current;
    }
    let current = values.iter().position(|&v| v == current).unwrap_or(0);
    values[cycle_index(values.len(), current, delta)]
}

fn cycle_rating(current: u8, delta: i32) -> u8 {
    cycle_index(6, current.min(5) as usize, delta) as u8
}

fn rating_label(stars: u8) -> String {
    let stars = stars.min(5);
    if stars == 0 {
        "なし".to_string()
    } else {
        format!("{} / 5", stars)
    }
}

fn mark_picker_dirty(picker: &mut RingPickerState, row: RingPickerRowId) {
    if !picker.dirty_rows.contains(&row) {
        picker.dirty_rows.push(row);
    }
}

fn cycle_video_playback_speed(current: f64, delta: i32) -> f64 {
    let values = &crate::video::clock::PLAYBACK_SPEED_CHOICES;
    let current = values
        .iter()
        .position(|&v| (v - current).abs() < 1.0e-6)
        .unwrap_or_else(|| {
            values
                .iter()
                .enumerate()
                .min_by(|(_, a), (_, b)| {
                    (*a - current)
                        .abs()
                        .partial_cmp(&(*b - current).abs())
                        .unwrap_or(std::cmp::Ordering::Equal)
                })
                .map(|(idx, _)| idx)
                .unwrap_or(0)
        });
    values[cycle_index(values.len(), current, delta)]
}

fn drill_for_post_filter(filter: PostFilter) -> PostFilterDrillState {
    for (group_idx, group) in POST_FILTER_GROUPS.iter().enumerate() {
        if let Some(item_idx) = group.filters.iter().position(|&f| f == filter) {
            return PostFilterDrillState {
                group: group_idx,
                item: item_idx,
            };
        }
    }
    PostFilterDrillState { group: 0, item: 0 }
}

#[cfg(test)]
mod tests {
    use super::{
        POST_FILTER_GROUPS, PadDir, continuous_reading_stick_axis, cycle_rating,
        cycle_video_playback_speed, drill_for_post_filter, gamepad_grid_nav_target_pos,
        mouse_flick_direction, picker_rows_for_context, ring_direction_from_stick,
    };
    use crate::adjustment::PostFilter;
    use crate::ring_shortcut::RingShortcutContext;
    use crate::ring_shortcut::{MouseFlickState, RingDirection};
    use crate::settings::{ReadingDirection, ReadingFlow};
    use eframe::egui;

    #[test]
    fn details_grid_gamepad_up_down_move_one_row() {
        assert_eq!(
            gamepad_grid_nav_target_pos(10, 30, 5, 12, true, PadDir::Down),
            11
        );
        assert_eq!(
            gamepad_grid_nav_target_pos(10, 30, 5, 12, true, PadDir::Up),
            9
        );
    }

    #[test]
    fn details_grid_gamepad_left_right_move_by_visible_rows() {
        assert_eq!(
            gamepad_grid_nav_target_pos(10, 30, 5, 8, true, PadDir::Right),
            18
        );
        assert_eq!(
            gamepad_grid_nav_target_pos(10, 30, 5, 8, true, PadDir::Left),
            2
        );
    }

    #[test]
    fn thumbnail_grid_gamepad_preserves_grid_geometry() {
        assert_eq!(
            gamepad_grid_nav_target_pos(5, 30, 5, 8, false, PadDir::Right),
            6
        );
        assert_eq!(
            gamepad_grid_nav_target_pos(5, 30, 5, 8, false, PadDir::Down),
            10
        );
    }

    #[test]
    fn vertical_reading_stick_down_scrolls_forward() {
        assert_eq!(
            continuous_reading_stick_axis(
                ReadingFlow::Vertical,
                ReadingDirection::Ltr,
                egui::vec2(0.0, -1.0),
            ),
            Some(1.0)
        );
        assert_eq!(
            continuous_reading_stick_axis(
                ReadingFlow::Vertical,
                ReadingDirection::Ltr,
                egui::vec2(0.0, 1.0),
            ),
            Some(-1.0)
        );
    }

    #[test]
    fn horizontal_reading_stick_respects_reading_direction() {
        assert_eq!(
            continuous_reading_stick_axis(
                ReadingFlow::Horizontal,
                ReadingDirection::Ltr,
                egui::vec2(1.0, 0.0),
            ),
            Some(1.0)
        );
        assert_eq!(
            continuous_reading_stick_axis(
                ReadingFlow::Horizontal,
                ReadingDirection::Rtl,
                egui::vec2(1.0, 0.0),
            ),
            Some(-1.0)
        );
    }

    #[test]
    fn paged_reading_has_no_continuous_axis() {
        assert_eq!(
            continuous_reading_stick_axis(
                ReadingFlow::Paged,
                ReadingDirection::Ltr,
                egui::vec2(1.0, 1.0),
            ),
            None
        );
    }

    #[test]
    fn ring_stick_uses_eight_way_sectors() {
        assert_eq!(
            ring_direction_from_stick(egui::vec2(1.0, 0.0)),
            Some(RingDirection::Right)
        );
        assert_eq!(
            ring_direction_from_stick(egui::vec2(1.0, 1.0)),
            Some(RingDirection::UpRight)
        );
        assert_eq!(
            ring_direction_from_stick(egui::vec2(0.0, -1.0)),
            Some(RingDirection::Down)
        );
        assert_eq!(ring_direction_from_stick(egui::vec2(0.0, 0.0)), None);
    }

    #[test]
    fn mouse_flick_converts_screen_y_to_ring_direction() {
        let mut flick = MouseFlickState::new(
            RingShortcutContext::Grid,
            std::time::Instant::now(),
            egui::pos2(100.0, 100.0),
        );
        flick.current_pos = egui::pos2(130.0, 70.0);
        assert_eq!(mouse_flick_direction(&flick), Some(RingDirection::UpRight));

        flick.current_pos = egui::pos2(130.0, 130.0);
        assert_eq!(
            mouse_flick_direction(&flick),
            Some(RingDirection::DownRight)
        );

        flick.current_pos = egui::pos2(110.0, 110.0);
        assert_eq!(mouse_flick_direction(&flick), None);
    }

    #[test]
    fn picker_rows_are_context_specific() {
        assert_eq!(picker_rows_for_context(RingShortcutContext::Grid).len(), 5);
        assert_eq!(
            picker_rows_for_context(RingShortcutContext::ImageFullscreen).len(),
            7
        );
        assert_eq!(
            picker_rows_for_context(RingShortcutContext::VideoFullscreen).len(),
            3
        );
    }

    #[test]
    fn rating_picker_wraps_clear_and_five_stars() {
        assert_eq!(cycle_rating(0, -1), 5);
        assert_eq!(cycle_rating(5, 1), 0);
        assert_eq!(cycle_rating(2, 1), 3);
    }

    #[test]
    fn video_speed_picker_uses_hud_choices() {
        assert_eq!(cycle_video_playback_speed(1.0, 1), 1.25);
        assert_eq!(cycle_video_playback_speed(0.5, -1), 3.0);
    }

    #[test]
    fn post_filter_drill_groups_cover_all_filters_once() {
        let grouped_count: usize = POST_FILTER_GROUPS.iter().map(|g| g.filters.len()).sum();
        assert_eq!(grouped_count, PostFilter::ALL.len());

        for &filter in PostFilter::ALL {
            let occurrences = POST_FILTER_GROUPS
                .iter()
                .flat_map(|g| g.filters.iter().copied())
                .filter(|&candidate| candidate == filter)
                .count();
            assert_eq!(occurrences, 1, "{filter:?}");

            let drill = drill_for_post_filter(filter);
            assert_eq!(POST_FILTER_GROUPS[drill.group].filters[drill.item], filter);
        }
    }
}
