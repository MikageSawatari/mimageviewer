use std::time::{Duration, Instant};

use eframe::egui;

#[cfg(windows)]
use super::ViewerPresentation;
use super::{App, FolderNavMode, GridContainerOpenMode};
use crate::adjustment::PostFilter;
use crate::folder_pane::{FolderPaneCommand, FolderPaneTreeKey};
use crate::gamepad::{GamepadInputState, PadAxis, PadButton, PadEvent, WestReleaseOutcome};
use crate::grid_item::GridItem;
use crate::keymap::KeyAction;
use crate::ring_shortcut::{
    GamepadFavoritePickerState, GamepadLocationEntry, GamepadLocationNav,
    GamepadLocationPickerState, GamepadVideoMarkerPickerState, MOUSE_FLICK_MOVE_THRESHOLD_PX,
    MOUSE_FLICK_NEUTRAL_RADIUS_PX, MOUSE_GESTURE_STEP_THRESHOLD_PX, MouseButtonSlot,
    MouseFlickOutcome, MouseFlickState, MouseGestureDirection, MouseGestureState, PickerListMode,
    PickerListState, RightDragContext, RightDragMode, RingActionId, RingDirection,
    RingPickerAnchor, RingPickerOriginalState, RingPickerRowId, RingPickerState,
    RingShortcutContext, format_mouse_gesture_pattern, mouse_flick_guide_delay,
    mouse_flick_menu_delay, mouse_gesture_direction_from_delta,
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
const GAMEPAD_LIST_VISIBLE_ROWS: usize = 12;
const RING_STICK_COMMIT_THRESHOLD: f32 = 0.50;
const RING_STICK_HYSTERESIS_DEGREES: f32 = 8.0;

fn request_ring_overlay_repaint(ctx: &egui::Context) {
    ctx.request_repaint();
    if ctx.viewport_id() != egui::ViewportId::ROOT {
        ctx.request_repaint_of(egui::ViewportId::ROOT);
    }
}

fn request_ring_overlay_repaint_after(ctx: &egui::Context, duration: Duration) {
    ctx.request_repaint_after(duration);
    if ctx.viewport_id() != egui::ViewportId::ROOT {
        ctx.request_repaint_after_for(duration, egui::ViewportId::ROOT);
    }
}

fn ring_location_action_for_key_action(action: KeyAction) -> Option<RingActionId> {
    if let Some(slot) = action.favorite_slot_number() {
        return RingActionId::favorite_slot_action(slot);
    }
    if let Some(letter) = action.drive_letter() {
        return RingActionId::drive_action(letter);
    }
    Some(match action {
        KeyAction::GridOpenLocationDriveList => RingActionId::OpenLocationDriveList,
        KeyAction::GridOpenLocationReadingHistory => RingActionId::OpenLocationReadingHistory,
        KeyAction::GridOpenLocationRating1 => RingActionId::OpenLocationRating1,
        KeyAction::GridOpenLocationRating2 => RingActionId::OpenLocationRating2,
        KeyAction::GridOpenLocationRating3 => RingActionId::OpenLocationRating3,
        KeyAction::GridOpenLocationRating4 => RingActionId::OpenLocationRating4,
        KeyAction::GridOpenLocationRating5 => RingActionId::OpenLocationRating5,
        KeyAction::GridOpenLocationBooksRoot => RingActionId::OpenLocationBooksRoot,
        KeyAction::GridOpenLocationDesktop => RingActionId::OpenLocationDesktop,
        KeyAction::GridOpenLocationPictures => RingActionId::OpenLocationPictures,
        KeyAction::GridOpenLocationDownloads => RingActionId::OpenLocationDownloads,
        _ => return None,
    })
}

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
    RingPickerRowId::ReadingDirection,
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
    RingPickerRowId::ItemRating,
    RingPickerRowId::ContainerRating,
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
const POST_FILTER_GROUP_UTILITY: &[PostFilter] = &[
    PostFilter::Sharpen,
    PostFilter::Downscale2x,
    PostFilter::Downscale4x,
];

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
        if self.ring_picker.is_some() || !self.gamepad_state.west_ring_active() {
            return;
        }
        let context = self.current_ring_shortcut_context();
        let selected = self.gamepad_state.west_ring_direction();
        let painter = ui.painter();
        let center = full_rect.center();
        let radius = ring_guide_radius_for_rect(full_rect);
        let slots = &self.settings.ring_shortcuts.profile(context).slots;
        draw_ring_guide_donut(painter, center, radius, selected, |direction| {
            slots
                .get(direction.slot_index())
                .cloned()
                .unwrap_or_default()
                .label_for_context(context)
        });
    }

    /// `surface_context` identifies which window is drawing: `Grid` for the main
    /// window's grid, or the active fullscreen context for the viewer window. With a
    /// detached viewer open, the main window and the viewer are two separate, live
    /// windows that share the single `mouse_ring_flick` state; only the window the
    /// ring belongs to should render it, otherwise the ring appears in both windows.
    pub(crate) fn draw_mouse_ring_flick_overlay(
        &self,
        ui: &mut egui::Ui,
        full_rect: egui::Rect,
        surface_context: RingShortcutContext,
    ) {
        if !self
            .settings
            .ring_shortcuts
            .mouse_ring_enabled(surface_context)
            || !self.settings.ring_shortcuts.mouse_ring_help_visible
            || self.ring_picker.is_some()
        {
            return;
        }
        let Some(flick) = self.mouse_ring_flick.as_ref() else {
            return;
        };
        if flick.context != surface_context {
            return;
        }
        if !flick.guide_visible() {
            return;
        }

        let context = flick.context;
        let selected = mouse_flick_direction(flick);
        let radius = ring_guide_radius_for_rect(full_rect);
        let center = flick.start_pos;
        let painter = ui.painter();
        let slots = &self.settings.ring_shortcuts.profile(context).slots;
        draw_ring_guide_donut(painter, center, radius, selected, |direction| {
            slots
                .get(direction.slot_index())
                .cloned()
                .unwrap_or_default()
                .label_for_context(context)
        });
    }

    pub(crate) fn current_right_drag_context(&self) -> RightDragContext {
        if self.is_overlay_edit_mode_active() {
            RightDragContext::EditMode
        } else if let Some(fs_idx) = self.fullscreen_idx {
            if self.fullscreen_uses_video_ring_context(fs_idx) {
                RightDragContext::VideoFullscreen
            } else {
                RightDragContext::ImageFullscreen
            }
        } else {
            RightDragContext::Grid
        }
    }

    pub(crate) fn draw_mouse_gesture_overlay(
        &self,
        ui: &mut egui::Ui,
        full_rect: egui::Rect,
        surface_context: RightDragContext,
    ) {
        if self
            .settings
            .ring_shortcuts
            .right_drag_mode(surface_context)
            != RightDragMode::MouseGesture
            || !self.settings.ring_shortcuts.mouse_gesture_help_visible
            || self.ring_picker.is_some()
        {
            return;
        }
        let Some(gesture) = self.mouse_gesture.as_ref() else {
            return;
        };
        if gesture.context != surface_context || !gesture.guide_visible() {
            return;
        }
        let profile = self
            .settings
            .ring_shortcuts
            .mouse_gesture_profile(surface_context);
        let painter = ui.painter();
        let margin = 16.0;
        let usable_w = (full_rect.width() - margin * 2.0).max(120.0);
        let usable_h = (full_rect.height() - margin * 2.0).max(120.0);
        let row_h = 32.0;
        let rows = profile.bindings.len().max(1);
        let panel_w = (full_rect.width() * 0.60).clamp(340.0, 560.0).min(usable_w);
        let panel_h = (96.0 + row_h * rows as f32).min(usable_h);
        let rect = egui::Rect::from_center_size(full_rect.center(), egui::vec2(panel_w, panel_h));
        painter.rect_filled(rect, 8.0, egui::Color32::from_black_alpha(220));
        painter.rect_stroke(
            rect,
            8.0,
            egui::Stroke::new(1.0, egui::Color32::from_white_alpha(90)),
            egui::StrokeKind::Inside,
        );
        let current = if gesture.pattern.is_empty() {
            "-".to_string()
        } else {
            format_mouse_gesture_pattern(&gesture.pattern)
        };
        painter.text(
            rect.min + egui::vec2(14.0, 14.0),
            egui::Align2::LEFT_TOP,
            format!("マウスジェスチャ / 入力中: {current}"),
            egui::FontId::proportional(17.0),
            egui::Color32::WHITE,
        );
        let row_left = rect.min.x + 14.0;
        let row_right = rect.max.x - 14.0;
        let mut y = rect.min.y + 46.0;
        let action_context = surface_context.gesture_action_context();
        let selected_row = profile.binding_index_for_pattern(&gesture.pattern);
        if profile.bindings.is_empty() {
            painter.text(
                egui::pos2(row_left + 10.0, y + row_h * 0.5),
                egui::Align2::LEFT_CENTER,
                "未登録",
                egui::FontId::proportional(14.5),
                egui::Color32::from_gray(205),
            );
            painter.text(
                egui::pos2(row_right - 10.0, y + row_h * 0.5),
                egui::Align2::RIGHT_CENTER,
                "操作カスタマイズで追加",
                egui::FontId::proportional(14.5),
                egui::Color32::from_gray(230),
            );
        } else {
            let available_h = (panel_h - 96.0).max(row_h);
            let visible_rows = ((available_h / row_h).floor() as usize)
                .max(1)
                .min(profile.bindings.len().max(1));
            let focus_row = selected_row.unwrap_or(0);
            let start = focus_row
                .saturating_sub(visible_rows / 2)
                .min(profile.bindings.len().saturating_sub(visible_rows));
            let end = (start + visible_rows).min(profile.bindings.len());
            for (idx, binding) in profile.bindings.iter().enumerate().take(end).skip(start) {
                let selected = selected_row == Some(idx);
                let row_rect = egui::Rect::from_min_size(
                    egui::pos2(row_left, y),
                    egui::vec2((row_right - row_left).max(1.0), row_h),
                );
                painter.rect_filled(
                    row_rect,
                    5.0,
                    if selected {
                        egui::Color32::from_rgb(56, 94, 138)
                    } else {
                        egui::Color32::TRANSPARENT
                    },
                );
                let pattern = format_mouse_gesture_pattern(&binding.pattern);
                let label = binding.action.label_for_context(action_context);
                painter.text(
                    egui::pos2(row_rect.min.x + 10.0, row_rect.center().y),
                    egui::Align2::LEFT_CENTER,
                    pattern,
                    egui::FontId::proportional(14.5),
                    egui::Color32::from_white_alpha(if selected { 245 } else { 205 }),
                );
                painter.text(
                    egui::pos2(row_rect.max.x - 10.0, row_rect.center().y),
                    egui::Align2::RIGHT_CENTER,
                    label,
                    egui::FontId::proportional(14.5),
                    egui::Color32::WHITE,
                );
                y += row_h;
            }
        }
        painter.text(
            egui::pos2(rect.min.x + 14.0, rect.max.y - 20.0),
            egui::Align2::LEFT_CENTER,
            "右ドラッグで軌跡を入力、離すと実行 / 短押しは従来操作",
            egui::FontId::proportional(12.0),
            egui::Color32::from_gray(190),
        );
    }
    pub(crate) fn start_mouse_ring_flick(
        &mut self,
        ctx: &egui::Context,
        context: RingShortcutContext,
        pos: egui::Pos2,
        grid_target_idx: Option<usize>,
    ) {
        if !self.settings.ring_shortcuts.mouse_ring_enabled(context) || self.ring_picker.is_some() {
            return;
        }
        if self.mouse_ring_flick.is_some() {
            return;
        }
        self.mouse_ring_flick = Some(MouseFlickState::new(context, Instant::now(), pos));
        self.mouse_ring_grid_target_idx = grid_target_idx;
        self.mouse_ring_suppress_context_menu_once = false;
        self.sync_native_video_ring_guide_overlay(ctx);
        request_ring_overlay_repaint_after(ctx, mouse_flick_guide_delay());
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
            // This entry point is the grid's per-frame ring updater (always called with
            // `RingShortcutContext::Grid`). While a fullscreen viewer lives in a separate
            // viewport (normal F11 or the detached window), the main window keeps rendering
            // the grid behind it, so this runs every frame even though the grid is not the
            // active surface. Do NOT cancel a flick that belongs to the currently-active
            // ring context — doing so destroyed the fullscreen flick one frame after it was
            // created, so the overlay only flashed for a single frame and never reached
            // `guide_visible()`. Only drop a genuinely stale flick whose context is no
            // longer the active one.
            if existing.context == self.current_ring_shortcut_context() {
                return MouseFlickOutcome::None;
            }
            self.cancel_mouse_ring_flick();
            self.clear_native_video_ring_guide_overlay(ctx);
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
        self.update_mouse_ring_flick_at(
            ctx,
            context,
            pointer_pos,
            secondary_down,
            secondary_released,
        )
    }

    pub(crate) fn update_native_mouse_ring_flick(
        &mut self,
        ctx: &egui::Context,
        context: RingShortcutContext,
        pos: egui::Pos2,
        secondary_down: bool,
        secondary_released: bool,
    ) -> MouseFlickOutcome {
        self.update_mouse_ring_flick_with_pos(ctx, context, pos, secondary_down, secondary_released)
    }

    pub(crate) fn update_mouse_ring_flick_with_pos(
        &mut self,
        ctx: &egui::Context,
        context: RingShortcutContext,
        pos: egui::Pos2,
        secondary_down: bool,
        secondary_released: bool,
    ) -> MouseFlickOutcome {
        self.update_mouse_ring_flick_at(ctx, context, pos, secondary_down, secondary_released)
    }

    fn update_mouse_ring_flick_at(
        &mut self,
        ctx: &egui::Context,
        context: RingShortcutContext,
        pointer_pos: egui::Pos2,
        secondary_down: bool,
        secondary_released: bool,
    ) -> MouseFlickOutcome {
        let (moved, elapsed, armed, direction) = {
            let Some(flick) = self.mouse_ring_flick.as_mut() else {
                return MouseFlickOutcome::None;
            };
            // The flick belongs to another surface (e.g. the detached viewer's per-frame
            // update runs with `secondary_down=false` while the user is right-dragging the
            // main window's grid). Leave it alone — otherwise the `!secondary_down` branch
            // below would cancel the other window's live flick.
            if flick.context != context {
                return MouseFlickOutcome::None;
            }
            flick.current_pos = pointer_pos;
            if flick.moved() >= MOUSE_FLICK_MOVE_THRESHOLD_PX {
                flick.armed = true;
            }
            (
                flick.moved(),
                flick.elapsed(),
                flick.armed,
                mouse_flick_direction(flick),
            )
        };

        if secondary_released {
            let long_press = elapsed >= mouse_flick_menu_delay();
            let ring_visible = armed || long_press;
            self.mouse_ring_flick = None;
            self.clear_native_video_ring_guide_overlay(ctx);
            if let Some(direction) = direction {
                self.mouse_ring_grid_target_idx = None;
                self.mouse_ring_suppress_context_menu_once = true;
                if let Some(nav) =
                    self.trigger_ring_shortcut_action(ctx, context, direction, "mouse-flick")
                {
                    self.mouse_ring_nav = Some(nav);
                }
                request_ring_overlay_repaint(ctx);
                return MouseFlickOutcome::Fired;
            }
            if moved < MOUSE_FLICK_MOVE_THRESHOLD_PX {
                self.mouse_ring_suppress_context_menu_once = true;
                request_ring_overlay_repaint(ctx);
                return if long_press {
                    self.mouse_ring_grid_target_idx = None;
                    MouseFlickOutcome::Cancelled
                } else {
                    MouseFlickOutcome::ShortTap
                };
            }
            if ring_visible {
                self.mouse_ring_grid_target_idx = None;
                self.mouse_ring_suppress_context_menu_once = true;
                request_ring_overlay_repaint(ctx);
                return MouseFlickOutcome::Cancelled;
            }
            self.mouse_ring_grid_target_idx = None;
            request_ring_overlay_repaint(ctx);
            return MouseFlickOutcome::None;
        }

        if !secondary_down {
            self.cancel_mouse_ring_flick();
            self.clear_native_video_ring_guide_overlay(ctx);
            return MouseFlickOutcome::None;
        }

        if !armed && moved < MOUSE_FLICK_NEUTRAL_RADIUS_PX && elapsed >= mouse_flick_menu_delay() {
            self.sync_native_video_ring_guide_overlay(ctx);
            self.request_mouse_ring_flick_repaint(ctx);
            return MouseFlickOutcome::None;
        }

        self.sync_native_video_ring_guide_overlay(ctx);
        self.request_mouse_ring_flick_repaint(ctx);
        MouseFlickOutcome::None
    }

    pub(crate) fn start_mouse_gesture(
        &mut self,
        ctx: &egui::Context,
        context: RightDragContext,
        pos: egui::Pos2,
        grid_target_idx: Option<usize>,
    ) {
        if self.settings.ring_shortcuts.right_drag_mode(context) != RightDragMode::MouseGesture
            || self.ring_picker.is_some()
            || self.mouse_gesture.is_some()
        {
            return;
        }
        self.mouse_gesture = Some(MouseGestureState::new(context, Instant::now(), pos));
        self.mouse_gesture_grid_target_idx = grid_target_idx;
        self.mouse_ring_suppress_context_menu_once = false;
        if context == RightDragContext::VideoFullscreen {
            self.sync_native_video_mouse_gesture_overlay(ctx);
        }
        request_ring_overlay_repaint_after(ctx, mouse_flick_guide_delay());
    }

    pub(crate) fn update_mouse_gesture(
        &mut self,
        ctx: &egui::Context,
        context: RightDragContext,
    ) -> MouseFlickOutcome {
        let Some(existing) = self.mouse_gesture.as_ref() else {
            return MouseFlickOutcome::None;
        };
        if existing.context != context {
            if existing.context == self.current_right_drag_context() {
                return MouseFlickOutcome::None;
            }
            let old_context = existing.context;
            self.cancel_mouse_gesture();
            if old_context == RightDragContext::VideoFullscreen {
                self.clear_native_video_mouse_gesture_overlay(ctx);
            }
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
        self.update_mouse_gesture_with_pos(
            ctx,
            context,
            pointer_pos,
            secondary_down,
            secondary_released,
        )
    }

    pub(crate) fn update_native_mouse_gesture(
        &mut self,
        ctx: &egui::Context,
        context: RightDragContext,
        pos: egui::Pos2,
        secondary_down: bool,
        secondary_released: bool,
    ) -> MouseFlickOutcome {
        self.update_mouse_gesture_with_pos(ctx, context, pos, secondary_down, secondary_released)
    }

    pub(crate) fn update_mouse_gesture_with_pos(
        &mut self,
        ctx: &egui::Context,
        context: RightDragContext,
        pointer_pos: egui::Pos2,
        secondary_down: bool,
        secondary_released: bool,
    ) -> MouseFlickOutcome {
        let (moved, elapsed, pattern, armed) = {
            let Some(gesture) = self.mouse_gesture.as_mut() else {
                return MouseFlickOutcome::None;
            };
            if gesture.context != context {
                return MouseFlickOutcome::None;
            }
            gesture.current_pos = pointer_pos;
            let step_delta = pointer_pos - gesture.last_step_pos;
            if step_delta.length() >= MOUSE_GESTURE_STEP_THRESHOLD_PX {
                if let Some(direction) = mouse_gesture_direction_from_delta(step_delta) {
                    if gesture.pattern.last().copied() != Some(direction)
                        && gesture.pattern.len() < crate::ring_shortcut::MOUSE_GESTURE_MAX_STROKES
                    {
                        gesture.pattern.push(direction);
                        gesture.armed = true;
                    }
                    gesture.last_step_pos = pointer_pos;
                }
            }
            (
                gesture.moved(),
                gesture.elapsed(),
                gesture.pattern.clone(),
                gesture.armed,
            )
        };

        if secondary_released {
            self.mouse_gesture = None;
            if context == RightDragContext::VideoFullscreen {
                self.clear_native_video_mouse_gesture_overlay(ctx);
            }
            if !pattern.is_empty() {
                self.mouse_gesture_grid_target_idx = None;
                self.mouse_ring_suppress_context_menu_once = true;
                if let Some(nav) = self.trigger_mouse_gesture_action(ctx, context, &pattern) {
                    self.mouse_ring_nav = Some(nav);
                }
                request_ring_overlay_repaint(ctx);
                return MouseFlickOutcome::Fired;
            }
            if moved < MOUSE_FLICK_MOVE_THRESHOLD_PX {
                self.mouse_ring_suppress_context_menu_once = true;
                request_ring_overlay_repaint(ctx);
                return if elapsed >= mouse_flick_menu_delay() {
                    self.mouse_gesture_grid_target_idx = None;
                    MouseFlickOutcome::Cancelled
                } else {
                    MouseFlickOutcome::ShortTap
                };
            }
            if armed || elapsed >= mouse_flick_menu_delay() {
                self.mouse_gesture_grid_target_idx = None;
                self.mouse_ring_suppress_context_menu_once = true;
                request_ring_overlay_repaint(ctx);
                return MouseFlickOutcome::Cancelled;
            }
            self.mouse_gesture_grid_target_idx = None;
            request_ring_overlay_repaint(ctx);
            return MouseFlickOutcome::None;
        }

        if !secondary_down {
            self.cancel_mouse_gesture();
            if context == RightDragContext::VideoFullscreen {
                self.clear_native_video_mouse_gesture_overlay(ctx);
            }
            return MouseFlickOutcome::None;
        }
        if context == RightDragContext::VideoFullscreen {
            self.sync_native_video_mouse_gesture_overlay(ctx);
        }
        self.request_mouse_gesture_repaint(ctx);
        MouseFlickOutcome::None
    }

    pub(crate) fn cancel_mouse_gesture(&mut self) {
        self.mouse_gesture = None;
        self.mouse_gesture_grid_target_idx = None;
    }

    fn request_mouse_gesture_repaint(&self, ctx: &egui::Context) {
        let Some(gesture) = self.mouse_gesture.as_ref() else {
            return;
        };
        let elapsed = gesture.elapsed();
        if elapsed < mouse_flick_guide_delay() {
            request_ring_overlay_repaint_after(ctx, mouse_flick_guide_delay() - elapsed);
        } else if !gesture.armed && elapsed < mouse_flick_menu_delay() {
            request_ring_overlay_repaint_after(ctx, mouse_flick_menu_delay() - elapsed);
        } else {
            request_ring_overlay_repaint_after(ctx, GAMEPAD_REPAINT_INTERVAL);
        }
    }

    fn trigger_mouse_gesture_action(
        &mut self,
        ctx: &egui::Context,
        context: RightDragContext,
        pattern: &[MouseGestureDirection],
    ) -> Option<AddressBarNav> {
        let action_context = context.gesture_action_context();
        let action = self
            .settings
            .ring_shortcuts
            .mouse_gesture_profile(context)
            .action_for_pattern(pattern);
        let pattern_label = format_mouse_gesture_pattern(pattern);
        if !action.is_valid_for_context(action_context) {
            crate::logger::log(format!(
                "mouse gesture ignored invalid action={} context={context:?}",
                action.as_str()
            ));
            return None;
        }
        if matches!(action, RingActionId::None) {
            self.show_feedback_toast(format!("[Gesture: {pattern_label} なし]"));
            return None;
        }
        self.show_feedback_toast(format!(
            "[Gesture: {pattern_label} {}]",
            action.label_for_context(action_context)
        ));
        self.apply_ring_action(ctx, action_context, action, "mouse-gesture")
    }
    pub(crate) fn mouse_ring_context_menu_suppressed(&self, ctx: &egui::Context) -> bool {
        if self.mouse_ring_suppress_context_menu_once {
            return true;
        }
        if let Some(flick) = self.mouse_ring_flick.as_ref() {
            if !self
                .settings
                .ring_shortcuts
                .mouse_ring_enabled(flick.context)
            {
                return false;
            }
            if flick.armed {
                return true;
            }
            return ctx.input(|i| {
                i.pointer
                    .interact_pos()
                    .or_else(|| i.pointer.latest_pos())
                    .is_some_and(|pos| {
                        pos.distance(flick.start_pos) >= MOUSE_FLICK_MOVE_THRESHOLD_PX
                    })
            });
        }
        if let Some(gesture) = self.mouse_gesture.as_ref() {
            if self
                .settings
                .ring_shortcuts
                .right_drag_mode(gesture.context)
                != RightDragMode::MouseGesture
            {
                return false;
            }
            if gesture.armed {
                return true;
            }
            return ctx.input(|i| {
                i.pointer
                    .interact_pos()
                    .or_else(|| i.pointer.latest_pos())
                    .is_some_and(|pos| {
                        pos.distance(gesture.start_pos) >= MOUSE_FLICK_MOVE_THRESHOLD_PX
                    })
            });
        }
        false
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
        self.cancel_mouse_gesture();
    }

    pub(crate) fn request_mouse_ring_flick_repaint(&self, ctx: &egui::Context) {
        let Some(flick) = self.mouse_ring_flick.as_ref() else {
            return;
        };
        let elapsed = flick.elapsed();
        if elapsed < mouse_flick_guide_delay() {
            request_ring_overlay_repaint_after(ctx, mouse_flick_guide_delay() - elapsed);
        } else if !flick.armed && elapsed < mouse_flick_menu_delay() {
            request_ring_overlay_repaint_after(ctx, mouse_flick_menu_delay() - elapsed);
        } else if flick.armed {
            request_ring_overlay_repaint_after(ctx, GAMEPAD_REPAINT_INTERVAL);
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
            let count = drill
                .map(|list| self.picker_list_len(picker, list))
                .unwrap_or(0)
                .max(1);
            (104.0 + row_h * count as f32).min(full_rect.height() - 32.0)
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
                            egui::RichText::new(format!("ピッカー / {}", picker.context.label()))
                                .size(17.0)
                                .color(egui::Color32::WHITE),
                        );
                        ui.add_space(6.0);
                        if let Some(drill) = drill {
                            self.draw_gamepad_picker_list(ui, picker, drill);
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
                                egui::RichText::new("上下:選択  左右:変更  A:一覧  B/X:確定")
                                    .size(12.0)
                                    .color(egui::Color32::from_white_alpha(190)),
                            );
                        }
                    });
            });
    }

    pub(crate) fn draw_gamepad_favorite_picker_overlay(
        &self,
        ui: &mut egui::Ui,
        full_rect: egui::Rect,
    ) {
        let Some(picker) = self.gamepad_favorite_picker.as_ref() else {
            return;
        };
        let painter = ui.painter();
        painter.rect_filled(full_rect, 0.0, egui::Color32::from_black_alpha(120));

        let row_h = 34.0;
        let visible_rows = self
            .settings
            .favorites
            .len()
            .min(GAMEPAD_LIST_VISIBLE_ROWS)
            .max(1);
        let panel_w = (full_rect.width() * 0.62).clamp(360.0, 640.0);
        let panel_h = 92.0 + row_h * visible_rows as f32;
        let panel_rect =
            egui::Rect::from_center_size(full_rect.center(), egui::vec2(panel_w, panel_h));
        let selected = picker
            .selected
            .min(self.settings.favorites.len().saturating_sub(1));
        let start = picker
            .scroll_top
            .min(self.settings.favorites.len().saturating_sub(visible_rows));
        let end = (start + visible_rows).min(self.settings.favorites.len());

        egui::Area::new(egui::Id::new("gamepad_favorite_picker_overlay"))
            .order(egui::Order::Foreground)
            .fixed_pos(panel_rect.min)
            .show(ui.ctx(), |ui| {
                ui.set_min_size(panel_rect.size());
                egui::Frame::new()
                    .fill(egui::Color32::from_black_alpha(224))
                    .stroke(egui::Stroke::new(1.0, egui::Color32::from_white_alpha(90)))
                    .corner_radius(egui::CornerRadius::same(8))
                    .inner_margin(egui::Margin::same(14))
                    .show(ui, |ui| {
                        ui.set_width(panel_w - 28.0);
                        ui.label(
                            egui::RichText::new("お気に入り")
                                .size(17.0)
                                .color(egui::Color32::WHITE),
                        );
                        ui.add_space(6.0);
                        let mut first_row_rect = None;
                        let mut last_row_rect = None;
                        let has_scrollbar = self.settings.favorites.len() > visible_rows;
                        let scrollbar_gutter = if has_scrollbar { 18.0 } else { 0.0 };
                        for idx in start..end {
                            let fav = &self.settings.favorites[idx];
                            let is_selected = idx == selected;
                            let (rect, _) = ui.allocate_exact_size(
                                egui::vec2(ui.available_width(), row_h),
                                egui::Sense::hover(),
                            );
                            first_row_rect.get_or_insert(rect);
                            last_row_rect = Some(rect);
                            let row_rect = egui::Rect::from_min_max(
                                rect.min,
                                egui::pos2(
                                    (rect.max.x - scrollbar_gutter).max(rect.min.x),
                                    rect.max.y,
                                ),
                            );
                            ui.painter().rect_filled(
                                row_rect,
                                5.0,
                                if is_selected {
                                    egui::Color32::from_rgb(56, 94, 138)
                                } else {
                                    egui::Color32::TRANSPARENT
                                },
                            );
                            ui.painter().text(
                                egui::pos2(row_rect.min.x + 10.0, row_rect.center().y),
                                egui::Align2::LEFT_CENTER,
                                truncate_ring_overlay_label(&fav.name, 22),
                                egui::FontId::proportional(if is_selected { 15.5 } else { 14.5 }),
                                egui::Color32::from_white_alpha(if is_selected {
                                    255
                                } else {
                                    220
                                }),
                            );
                            let value_right = row_rect.max.x - 10.0;
                            ui.painter().text(
                                egui::pos2(value_right, row_rect.center().y),
                                egui::Align2::RIGHT_CENTER,
                                truncate_ring_overlay_label(&fav.path.display().to_string(), 34),
                                egui::FontId::proportional(12.5),
                                egui::Color32::from_white_alpha(if is_selected {
                                    235
                                } else {
                                    180
                                }),
                            );
                        }
                        if let (Some(first), Some(last)) = (first_row_rect, last_row_rect) {
                            draw_picker_scrollbar(
                                ui.painter(),
                                egui::Rect::from_min_max(first.min, last.max),
                                self.settings.favorites.len(),
                                visible_rows,
                                start,
                            );
                        }
                        ui.add_space(8.0);
                        ui.label(
                            egui::RichText::new("上下:選択  A:移動  B/Start:閉じる")
                                .size(12.0)
                                .color(egui::Color32::from_white_alpha(190)),
                        );
                    });
            });
    }

    pub(crate) fn draw_gamepad_video_marker_picker_overlay(
        &mut self,
        ui: &mut egui::Ui,
        full_rect: egui::Rect,
    ) {
        let Some(picker) = self.gamepad_video_marker_picker.clone() else {
            return;
        };
        let Some(fs_idx) = self.fullscreen_idx else {
            return;
        };
        if !self.current_fullscreen_is_video(fs_idx) {
            return;
        }
        let markers = self.collect_video_nav_markers(fs_idx);
        if markers.is_empty() {
            return;
        }
        let painter = ui.painter();
        painter.rect_filled(full_rect, 0.0, egui::Color32::from_black_alpha(120));

        let row_h = 34.0;
        let visible_rows = markers.len().min(GAMEPAD_LIST_VISIBLE_ROWS).max(1);
        let panel_w = (full_rect.width() * 0.62).clamp(360.0, 640.0);
        let panel_h = 92.0 + row_h * visible_rows as f32;
        let panel_rect =
            egui::Rect::from_center_size(full_rect.center(), egui::vec2(panel_w, panel_h));
        let selected = picker.selected.min(markers.len().saturating_sub(1));
        let start = picker
            .scroll_top
            .min(markers.len().saturating_sub(visible_rows));
        let end = (start + visible_rows).min(markers.len());

        egui::Area::new(egui::Id::new("gamepad_video_marker_picker_overlay"))
            .order(egui::Order::Foreground)
            .fixed_pos(panel_rect.min)
            .show(ui.ctx(), |ui| {
                ui.set_min_size(panel_rect.size());
                egui::Frame::new()
                    .fill(egui::Color32::from_black_alpha(224))
                    .stroke(egui::Stroke::new(1.0, egui::Color32::from_white_alpha(90)))
                    .corner_radius(egui::CornerRadius::same(8))
                    .inner_margin(egui::Margin::same(14))
                    .show(ui, |ui| {
                        ui.set_width(panel_w - 28.0);
                        ui.label(
                            egui::RichText::new("ブックマーク / チャプター")
                                .size(17.0)
                                .color(egui::Color32::WHITE),
                        );
                        ui.add_space(6.0);
                        let mut first_row_rect = None;
                        let mut last_row_rect = None;
                        let has_scrollbar = markers.len() > visible_rows;
                        let scrollbar_gutter = if has_scrollbar { 18.0 } else { 0.0 };
                        for idx in start..end {
                            let marker = &markers[idx];
                            let is_selected = idx == selected;
                            let (rect, _) = ui.allocate_exact_size(
                                egui::vec2(ui.available_width(), row_h),
                                egui::Sense::hover(),
                            );
                            first_row_rect.get_or_insert(rect);
                            last_row_rect = Some(rect);
                            let row_rect = egui::Rect::from_min_max(
                                rect.min,
                                egui::pos2(
                                    (rect.max.x - scrollbar_gutter).max(rect.min.x),
                                    rect.max.y,
                                ),
                            );
                            ui.painter().rect_filled(
                                row_rect,
                                5.0,
                                if is_selected {
                                    egui::Color32::from_rgb(56, 94, 138)
                                } else {
                                    egui::Color32::TRANSPARENT
                                },
                            );
                            ui.painter().text(
                                egui::pos2(row_rect.min.x + 10.0, row_rect.center().y),
                                egui::Align2::LEFT_CENTER,
                                video_marker_primary_label(marker),
                                egui::FontId::proportional(if is_selected { 15.5 } else { 14.5 }),
                                egui::Color32::from_white_alpha(if is_selected {
                                    255
                                } else {
                                    220
                                }),
                            );
                            ui.painter().text(
                                egui::pos2(row_rect.max.x - 10.0, row_rect.center().y),
                                egui::Align2::RIGHT_CENTER,
                                truncate_ring_overlay_label(
                                    &video_marker_secondary_label(marker),
                                    34,
                                ),
                                egui::FontId::proportional(12.5),
                                egui::Color32::from_white_alpha(if is_selected {
                                    235
                                } else {
                                    180
                                }),
                            );
                        }
                        if let (Some(first), Some(last)) = (first_row_rect, last_row_rect) {
                            draw_picker_scrollbar(
                                ui.painter(),
                                egui::Rect::from_min_max(first.min, last.max),
                                markers.len(),
                                visible_rows,
                                start,
                            );
                        }
                        ui.add_space(8.0);
                        ui.label(
                            egui::RichText::new("上下:選択  A:移動  B/Select:閉じる")
                                .size(12.0)
                                .color(egui::Color32::from_white_alpha(190)),
                        );
                    });
            });
    }

    pub(crate) fn draw_gamepad_location_picker_overlay(
        &self,
        ui: &mut egui::Ui,
        full_rect: egui::Rect,
    ) {
        let Some(picker) = self.gamepad_location_picker.as_ref() else {
            return;
        };
        let painter = ui.painter();
        painter.rect_filled(full_rect, 0.0, egui::Color32::from_black_alpha(120));

        let row_h = 34.0;
        let visible_rows = picker.entries.len().min(GAMEPAD_LIST_VISIBLE_ROWS).max(1);
        let panel_w = (full_rect.width() * 0.62).clamp(360.0, 640.0);
        let panel_h = 92.0 + row_h * visible_rows as f32;
        let panel_rect =
            egui::Rect::from_center_size(full_rect.center(), egui::vec2(panel_w, panel_h));
        let selected = picker.selected.min(picker.entries.len().saturating_sub(1));
        let start = picker
            .scroll_top
            .min(picker.entries.len().saturating_sub(visible_rows));
        let end = (start + visible_rows).min(picker.entries.len());

        egui::Area::new(egui::Id::new("gamepad_location_picker_overlay"))
            .order(egui::Order::Foreground)
            .fixed_pos(panel_rect.min)
            .show(ui.ctx(), |ui| {
                ui.set_min_size(panel_rect.size());
                egui::Frame::new()
                    .fill(egui::Color32::from_black_alpha(224))
                    .stroke(egui::Stroke::new(1.0, egui::Color32::from_white_alpha(90)))
                    .corner_radius(egui::CornerRadius::same(8))
                    .inner_margin(egui::Margin::same(14))
                    .show(ui, |ui| {
                        ui.set_width(panel_w - 28.0);
                        ui.label(
                            egui::RichText::new("場所")
                                .size(17.0)
                                .color(egui::Color32::WHITE),
                        );
                        ui.add_space(6.0);
                        let mut first_row_rect = None;
                        let mut last_row_rect = None;
                        let has_scrollbar = picker.entries.len() > visible_rows;
                        let scrollbar_gutter = if has_scrollbar { 18.0 } else { 0.0 };
                        for idx in start..end {
                            let entry = &picker.entries[idx];
                            let is_selected = idx == selected;
                            let (rect, _) = ui.allocate_exact_size(
                                egui::vec2(ui.available_width(), row_h),
                                egui::Sense::hover(),
                            );
                            first_row_rect.get_or_insert(rect);
                            last_row_rect = Some(rect);
                            let row_rect = egui::Rect::from_min_max(
                                rect.min,
                                egui::pos2(
                                    (rect.max.x - scrollbar_gutter).max(rect.min.x),
                                    rect.max.y,
                                ),
                            );
                            ui.painter().rect_filled(
                                row_rect,
                                5.0,
                                if is_selected {
                                    egui::Color32::from_rgb(56, 94, 138)
                                } else {
                                    egui::Color32::TRANSPARENT
                                },
                            );
                            ui.painter().text(
                                egui::pos2(row_rect.min.x + 10.0, row_rect.center().y),
                                egui::Align2::LEFT_CENTER,
                                truncate_ring_overlay_label(&entry.label, 22),
                                egui::FontId::proportional(if is_selected { 15.5 } else { 14.5 }),
                                egui::Color32::from_white_alpha(if is_selected {
                                    255
                                } else {
                                    220
                                }),
                            );
                            ui.painter().text(
                                egui::pos2(row_rect.max.x - 10.0, row_rect.center().y),
                                egui::Align2::RIGHT_CENTER,
                                truncate_ring_overlay_label(&entry.value, 34),
                                egui::FontId::proportional(12.5),
                                egui::Color32::from_white_alpha(if is_selected {
                                    235
                                } else {
                                    180
                                }),
                            );
                        }
                        if let (Some(first), Some(last)) = (first_row_rect, last_row_rect) {
                            draw_picker_scrollbar(
                                ui.painter(),
                                egui::Rect::from_min_max(first.min, last.max),
                                picker.entries.len(),
                                visible_rows,
                                start,
                            );
                        }
                        ui.add_space(8.0);
                        ui.label(
                            egui::RichText::new("上下:選択  A:移動  B/Select:閉じる")
                                .size(12.0)
                                .color(egui::Color32::from_white_alpha(190)),
                        );
                    });
            });
    }

    fn draw_gamepad_picker_list(
        &self,
        ui: &mut egui::Ui,
        picker: &RingPickerState,
        list: PickerListState,
    ) {
        let title = self.picker_list_title(list);
        let items = self.picker_list_items(picker, list);
        ui.label(
            egui::RichText::new(title)
                .size(14.0)
                .color(egui::Color32::from_white_alpha(200)),
        );
        ui.add_space(8.0);
        let row_h = 32.0;
        let selected = list.selected.min(items.len().saturating_sub(1));
        for (idx, item) in items.iter().enumerate() {
            let is_selected = idx == selected;
            let (rect, _) = ui.allocate_exact_size(
                egui::vec2(ui.available_width(), row_h),
                egui::Sense::hover(),
            );
            ui.painter().rect_filled(
                rect,
                5.0,
                if is_selected {
                    egui::Color32::from_rgb(56, 94, 138)
                } else {
                    egui::Color32::TRANSPARENT
                },
            );
            ui.painter().text(
                egui::pos2(rect.min.x + 10.0, rect.center().y),
                egui::Align2::LEFT_CENTER,
                item,
                egui::FontId::proportional(if is_selected { 15.5 } else { 14.5 }),
                egui::Color32::from_white_alpha(if is_selected { 255 } else { 215 }),
            );
        }
        ui.add_space(8.0);
        ui.label(
            egui::RichText::new("上下:選択  A:選択  B/X:確定して閉じる")
                .size(12.0)
                .color(egui::Color32::from_white_alpha(190)),
        );
    }

    fn picker_list_len(&self, picker: &RingPickerState, list: PickerListState) -> usize {
        picker_list_len_for_state(picker, list, self.settings.ai_feature_mode)
    }

    fn picker_list_title(&self, list: PickerListState) -> &'static str {
        match list.mode {
            PickerListMode::PostFilterGroup => "ポストフィルタ: グループ",
            PickerListMode::PostFilterItem { .. } => "ポストフィルタ: 項目",
            PickerListMode::RowValues(row) => row.label(),
        }
    }

    fn picker_list_items(&self, picker: &RingPickerState, list: PickerListState) -> Vec<String> {
        match list.mode {
            PickerListMode::PostFilterGroup => POST_FILTER_GROUPS
                .iter()
                .map(|group| group.label.to_string())
                .collect(),
            PickerListMode::PostFilterItem { group } => POST_FILTER_GROUPS
                .get(group)
                .map(|group| {
                    group
                        .filters
                        .iter()
                        .map(|filter| filter.display_label().to_string())
                        .collect()
                })
                .unwrap_or_default(),
            PickerListMode::RowValues(row) => {
                picker_row_value_labels(picker, row, self.settings.ai_feature_mode)
            }
        }
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
            RingPickerRowId::ReadingDirection => picker.reading_direction.label().to_string(),
            RingPickerRowId::FitMode => picker.fit_mode.label().to_string(),
            RingPickerRowId::PostFilter => picker.post_filter.display_label().to_string(),
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
                    if self.gamepad_state.set_button_down(button, true, now) {
                        actions.push(PadAction {
                            button,
                            kind: PadActionKind::Press,
                        });
                    }
                }
                PadEvent::ButtonReleased(button) => {
                    saw_input_event = true;
                    if self.gamepad_state.set_button_down(button, false, now) {
                        actions.push(PadAction {
                            button,
                            kind: PadActionKind::Release,
                        });
                    }
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
            self.consume_gamepad_directional_neutral_gate(now);
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
            || self.gamepad_favorite_picker.is_some()
            || self.gamepad_location_picker.is_some()
            || self.gamepad_video_marker_picker.is_some()
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
        if self.gamepad_video_marker_picker.is_some() {
            self.dispatch_gamepad_video_marker_picker_button(ctx, action);
            return None;
        }
        if self.gamepad_favorite_picker.is_some() {
            return self.dispatch_gamepad_favorite_picker_button(ctx, action);
        }
        if self.gamepad_location_picker.is_some() {
            return self.dispatch_gamepad_location_picker_button(ctx, action);
        }
        if self.ring_picker.is_some() {
            self.dispatch_gamepad_picker_button(ctx, action);
            return None;
        }
        if self.gamepad_state.directional_neutral_required()
            && (button_dir(action.button).is_some() || action.button == PadButton::West)
        {
            return None;
        }
        match action.kind {
            PadActionKind::Release if action.button == PadButton::West => {
                if let Some(direction) = self.current_ring_gamepad_direction() {
                    self.gamepad_state.mark_west_ring_direction(direction);
                }
                self.finish_gamepad_west_release(ctx)
            }
            PadActionKind::Release if action.button == PadButton::North => {
                if !self.gamepad_state.y_modifier_used() {
                    self.handle_gamepad_y_tap(ctx);
                }
                None
            }
            PadActionKind::Release => None,
            PadActionKind::Press | PadActionKind::Repeat => {
                if self.gamepad_state.west_ring_active() {
                    if let Some(dir) = button_dir(action.button) {
                        let direction = self
                            .current_ring_gamepad_direction()
                            .unwrap_or_else(|| ring_direction_from_pad_dir(dir));
                        self.gamepad_state.mark_west_ring_direction(direction);
                        self.sync_native_video_ring_guide_overlay(ctx);
                        ctx.request_repaint();
                    } else if action.button == PadButton::East
                        && action.kind == PadActionKind::Press
                    {
                        self.gamepad_state.cancel_west_ring();
                        self.clear_native_video_ring_guide_overlay(ctx);
                        ctx.request_repaint();
                    }
                    return None;
                }
                if let Some(dir) = button_dir(action.button) {
                    if self.fullscreen_idx.is_none()
                        && self.handle_gamepad_folder_tree_direction(dir)
                    {
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
                    PadButton::West if action.kind == PadActionKind::Press => {
                        self.sync_native_video_ring_guide_overlay(ctx);
                        None
                    }
                    PadButton::Select if action.kind == PadActionKind::Press => {
                        self.handle_gamepad_select(ctx)
                    }
                    PadButton::Start if action.kind == PadActionKind::Press => {
                        self.handle_gamepad_start(ctx)
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
        if self.gamepad_video_marker_picker.is_some() {
            return self.dispatch_gamepad_video_marker_picker_analog(ctx, now);
        }
        if self.gamepad_favorite_picker.is_some() {
            return self.dispatch_gamepad_favorite_picker_analog(ctx, now);
        }
        if self.gamepad_location_picker.is_some() {
            return self.dispatch_gamepad_location_picker_analog(ctx, now);
        }
        if self.ring_picker.is_some() {
            return self.dispatch_gamepad_picker_analog(ctx, now);
        }
        if self.consume_gamepad_directional_neutral_gate(now) {
            return false;
        }
        if self.gamepad_state.west_ring_active() {
            if let Some(direction) = self.current_ring_gamepad_direction() {
                self.gamepad_state.mark_west_ring_direction(direction);
            }
            self.sync_native_video_ring_guide_overlay(ctx);
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

    fn gamepad_directional_neutral(&self) -> bool {
        !self.gamepad_state.dpad_direction_down()
            && stick_pair(&self.gamepad_state, PadAxis::LeftX, PadAxis::LeftY).length_sq() == 0.0
    }

    fn consume_gamepad_directional_neutral_gate(&mut self, now: Instant) -> bool {
        if !self.gamepad_state.directional_neutral_required() {
            return false;
        }
        let _ = self.gamepad_state.analog_dt(false, now);
        let _ = self
            .gamepad_state
            .left_stick_step_due(false, now, STICK_STEP_INTERVAL);
        if self.gamepad_directional_neutral() {
            self.gamepad_state.clear_directional_neutral_required();
            false
        } else {
            true
        }
    }

    fn current_ring_gamepad_direction(&self) -> Option<RingDirection> {
        ring_direction_from_dpad_buttons(&self.gamepad_state).or_else(|| {
            let stick = stick_pair(&self.gamepad_state, PadAxis::LeftX, PadAxis::LeftY);
            ring_direction_from_stick_with_hysteresis(
                stick,
                self.gamepad_state.west_ring_direction(),
            )
        })
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

        let lt = self.gamepad_state.trigger_value(true);
        let rt = self.gamepad_state.trigger_value(false);
        let trigger_delta = rt - lt;
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
        let lt = self.gamepad_state.trigger_value(true);
        let rt = self.gamepad_state.trigger_value(false);
        let trigger_delta = rt - lt;
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
        self.handle_gamepad_grid_accept(ctx)
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
        self.clear_native_video_ring_guide_overlay(ctx);
        match self.gamepad_state.finish_west_release() {
            WestReleaseOutcome::Picker => {
                self.open_gamepad_ring_picker(ctx);
                None
            }
            WestReleaseOutcome::Ring(direction) => {
                self.gamepad_state.require_directional_neutral();
                self.trigger_gamepad_ring_action(ctx, direction)
            }
            WestReleaseOutcome::Suppressed => None,
        }
    }

    fn open_gamepad_ring_picker(&mut self, ctx: &egui::Context) {
        let context = self.current_ring_shortcut_context();
        let mut picker = self.build_ring_picker_state(context);
        picker.clamp_row(picker_rows_for_context(context).len());
        self.gamepad_favorite_picker = None;
        self.gamepad_location_picker = None;
        self.gamepad_video_marker_picker = None;
        self.ring_picker = Some(picker);
        self.clear_native_video_ring_guide_overlay(ctx);
        ctx.request_repaint();
        self.sync_native_video_picker_overlay(ctx);
        self.maybe_show_x_picker_hint(ctx, context);
    }

    fn build_ring_picker_state(&mut self, context: RingShortcutContext) -> RingPickerState {
        let fs_idx = self.fullscreen_idx;
        let params = fs_idx.map(|idx| self.effective_params(idx).clone());
        let item_rating_records = self.current_picker_item_rating_records(context);
        let item_rating = if item_rating_records.len() == 1 {
            item_rating_records[0].1
        } else {
            0
        };
        let container_rating = self.current_folder_rating();
        let grid_cols = self.settings.grid_cols.clamp(
            crate::settings::MIN_GRID_COLS,
            crate::settings::MAX_GRID_COLS,
        );
        let sort_order = self.settings.sort_order;
        let thumb_aspect_auto = self.settings.thumb_aspect_auto;
        let thumb_aspect = self.settings.thumb_aspect;
        let spread_mode = self.spread_mode;
        let reading_flow = self.reading_flow;
        let reading_direction = self.reading_direction;
        let fit_mode = self
            .settings
            .fullscreen_fit_mode
            .effective_for_flow(self.reading_flow);
        let post_filter = params
            .as_ref()
            .map(|p| p.post_filter)
            .unwrap_or(PostFilter::None);
        let upscale_model_key = params.and_then(|p| p.upscale_model);
        let video_volume = self.settings.video_volume;
        let video_playback_speed = self.video_playback_speed;
        let video_continuous_mode = self.video_continuous_mode;
        let original = RingPickerOriginalState {
            grid_cols,
            sort_order,
            thumb_aspect_auto,
            thumb_aspect,
            item_rating_records,
            container_rating,
            spread_mode,
            reading_flow,
            reading_direction,
            fit_mode,
            post_filter,
            upscale_model_key: upscale_model_key.clone(),
            video_volume,
            video_playback_speed,
            video_continuous_mode,
        };
        RingPickerState {
            context,
            anchor: self.current_ring_picker_anchor(context),
            original,
            row: 0,
            dirty_rows: Vec::new(),
            x_close_armed: false,
            drill: None,
            grid_cols,
            sort_order,
            thumb_aspect_auto,
            thumb_aspect,
            item_rating,
            container_rating,
            spread_mode,
            reading_flow,
            reading_direction,
            fit_mode,
            post_filter,
            upscale_model_key,
            video_volume,
            video_playback_speed,
            video_continuous_mode,
        }
    }

    fn current_ring_picker_anchor(&self, context: RingShortcutContext) -> RingPickerAnchor {
        let folder = self.effective_folder();
        let item_key = match context {
            RingShortcutContext::Grid => None,
            RingShortcutContext::ImageFullscreen | RingShortcutContext::VideoFullscreen => self
                .fullscreen_idx
                .and_then(|idx| self.items.get(idx))
                .map(GridItem::perf_key),
        };
        RingPickerAnchor { folder, item_key }
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
            "ピッカーを開きました。メタデータ表示は画像リングの右上スロットから使えます。"
                .to_string(),
            X_PICKER_HINT_TOAST_SECS,
        );
        ctx.request_repaint();
    }

    fn current_picker_item_rating_records(
        &mut self,
        context: RingShortcutContext,
    ) -> Vec<(usize, u8)> {
        match context {
            RingShortcutContext::Grid => {
                let targets = self.ratable_targets();
                targets
                    .into_iter()
                    .map(|idx| (idx, self.get_rating(idx)))
                    .collect()
            }
            RingShortcutContext::ImageFullscreen | RingShortcutContext::VideoFullscreen => self
                .fullscreen_idx
                .map(|idx| vec![(idx, self.get_rating(idx))])
                .unwrap_or_default(),
        }
    }

    fn dispatch_gamepad_picker_button(&mut self, ctx: &egui::Context, action: PadAction) {
        if self.ring_picker_is_stale() {
            self.close_stale_ring_picker(ctx);
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
                            self.confirm_ring_picker_list(ctx);
                        } else if self.enter_ring_picker_list_for_current_row(ctx) {
                        } else {
                            ctx.request_repaint();
                        }
                    }
                    PadButton::East if action.kind == PadActionKind::Press => {
                        self.commit_ring_picker(ctx);
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
        if self.ring_picker_is_stale() {
            self.close_stale_ring_picker(ctx);
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

    fn dispatch_gamepad_video_marker_picker_button(
        &mut self,
        ctx: &egui::Context,
        action: PadAction,
    ) {
        match action.kind {
            PadActionKind::Release => {}
            PadActionKind::Press | PadActionKind::Repeat => {
                if let Some(dir) = button_dir(action.button) {
                    if matches!(dir, PadDir::Up | PadDir::Down) {
                        self.move_gamepad_video_marker_picker(ctx, dir);
                    }
                    return;
                }
                match action.button {
                    PadButton::South if action.kind == PadActionKind::Press => {
                        self.confirm_gamepad_video_marker_picker(ctx);
                    }
                    PadButton::East | PadButton::Select if action.kind == PadActionKind::Press => {
                        self.gamepad_video_marker_picker = None;
                        self.clear_native_video_picker_overlay(ctx);
                        self.gamepad_state.require_directional_neutral();
                        ctx.request_repaint();
                    }
                    _ => {}
                }
            }
        }
    }

    fn dispatch_gamepad_video_marker_picker_analog(
        &mut self,
        ctx: &egui::Context,
        now: Instant,
    ) -> bool {
        let stick = stick_pair(&self.gamepad_state, PadAxis::LeftX, PadAxis::LeftY);
        let stick_dir = dominant_stick_dir(stick);
        let active = stick_dir.is_some();
        let due = self
            .gamepad_state
            .left_stick_step_due(active, now, STICK_STEP_INTERVAL);
        if due && let Some(dir @ (PadDir::Up | PadDir::Down)) = stick_dir {
            self.move_gamepad_video_marker_picker(ctx, dir);
        }
        active
    }

    fn move_gamepad_video_marker_picker(&mut self, ctx: &egui::Context, dir: PadDir) {
        let Some(fs_idx) = self.fullscreen_idx else {
            self.gamepad_video_marker_picker = None;
            self.clear_native_video_picker_overlay(ctx);
            self.gamepad_state.require_directional_neutral();
            ctx.request_repaint();
            return;
        };
        let len = self.collect_video_nav_markers(fs_idx).len();
        if len == 0 {
            self.gamepad_video_marker_picker = None;
            self.clear_native_video_picker_overlay(ctx);
            self.gamepad_state.require_directional_neutral();
            ctx.request_repaint();
            return;
        }
        if let Some(picker) = self.gamepad_video_marker_picker.as_mut() {
            let delta = if dir == PadDir::Down { 1 } else { -1 };
            picker.selected = cycle_index(len, picker.selected, delta);
            update_video_marker_picker_scroll(picker, len);
            ctx.request_repaint();
            self.sync_native_video_marker_picker_overlay(ctx);
        }
    }

    fn confirm_gamepad_video_marker_picker(&mut self, ctx: &egui::Context) {
        let Some(fs_idx) = self.fullscreen_idx else {
            self.gamepad_video_marker_picker = None;
            self.clear_native_video_picker_overlay(ctx);
            self.gamepad_state.require_directional_neutral();
            ctx.request_repaint();
            return;
        };
        let markers = self.collect_video_nav_markers(fs_idx);
        let selected = self
            .gamepad_video_marker_picker
            .as_ref()
            .map(|picker| picker.selected)
            .unwrap_or(0)
            .min(markers.len().saturating_sub(1));
        let Some(marker) = markers.get(selected).cloned() else {
            self.gamepad_video_marker_picker = None;
            self.clear_native_video_picker_overlay(ctx);
            self.gamepad_state.require_directional_neutral();
            ctx.request_repaint();
            return;
        };
        self.gamepad_video_marker_picker = None;
        self.clear_native_video_picker_overlay(ctx);
        self.gamepad_state.require_directional_neutral();
        if let Some(player) = self.fs_video_player(fs_idx) {
            player.seek(marker.pts);
        }
        #[cfg(windows)]
        {
            self.apply_loop_mode_to_player(fs_idx);
            self.maybe_start_normalize_scan_for_play_intent(fs_idx);
        }
        self.show_feedback_toast(video_marker_seek_toast(&marker));
        ctx.request_repaint();
    }

    fn dispatch_gamepad_favorite_picker_button(
        &mut self,
        ctx: &egui::Context,
        action: PadAction,
    ) -> Option<AddressBarNav> {
        match action.kind {
            PadActionKind::Release => None,
            PadActionKind::Press | PadActionKind::Repeat => {
                if let Some(dir) = button_dir(action.button) {
                    if matches!(dir, PadDir::Up | PadDir::Down) {
                        self.move_gamepad_favorite_picker(ctx, dir);
                    }
                    return None;
                }
                match action.button {
                    PadButton::South if action.kind == PadActionKind::Press => {
                        self.confirm_gamepad_favorite_picker(ctx)
                    }
                    PadButton::East | PadButton::Start if action.kind == PadActionKind::Press => {
                        self.gamepad_favorite_picker = None;
                        self.clear_native_video_picker_overlay(ctx);
                        self.gamepad_state.require_directional_neutral();
                        ctx.request_repaint();
                        None
                    }
                    _ => None,
                }
            }
        }
    }

    fn dispatch_gamepad_favorite_picker_analog(
        &mut self,
        ctx: &egui::Context,
        now: Instant,
    ) -> bool {
        let stick = stick_pair(&self.gamepad_state, PadAxis::LeftX, PadAxis::LeftY);
        let stick_dir = dominant_stick_dir(stick);
        let active = stick_dir.is_some();
        let due = self
            .gamepad_state
            .left_stick_step_due(active, now, STICK_STEP_INTERVAL);
        if due && let Some(dir @ (PadDir::Up | PadDir::Down)) = stick_dir {
            self.move_gamepad_favorite_picker(ctx, dir);
        }
        active
    }

    fn move_gamepad_favorite_picker(&mut self, ctx: &egui::Context, dir: PadDir) {
        let len = self.settings.favorites.len();
        if len == 0 {
            self.gamepad_favorite_picker = None;
            self.clear_native_video_picker_overlay(ctx);
            self.gamepad_state.require_directional_neutral();
            ctx.request_repaint();
            return;
        }
        if let Some(picker) = self.gamepad_favorite_picker.as_mut() {
            let delta = if dir == PadDir::Down { 1 } else { -1 };
            picker.selected = cycle_index(len, picker.selected, delta);
            update_favorite_picker_scroll(picker, len);
            ctx.request_repaint();
            self.sync_native_video_favorite_picker_overlay(ctx);
        }
    }

    fn confirm_gamepad_favorite_picker(&mut self, ctx: &egui::Context) -> Option<AddressBarNav> {
        let selected = self
            .gamepad_favorite_picker
            .as_ref()
            .map(|picker| picker.selected)
            .unwrap_or(0)
            .min(self.settings.favorites.len().saturating_sub(1));
        let target = self.settings.favorites.get(selected)?.path.clone();
        self.gamepad_favorite_picker = None;
        self.clear_native_video_picker_overlay(ctx);
        self.gamepad_state.require_directional_neutral();
        if self.fullscreen_idx.is_some() {
            self.close_fullscreen();
        }
        self.bump_input_seq(
            "gamepad_favorite_nav",
            Some(&format!("path={}", target.display())),
        );
        ctx.request_repaint();
        Some(AddressBarNav::Direct(target))
    }

    fn dispatch_gamepad_location_picker_button(
        &mut self,
        ctx: &egui::Context,
        action: PadAction,
    ) -> Option<AddressBarNav> {
        match action.kind {
            PadActionKind::Release => None,
            PadActionKind::Press | PadActionKind::Repeat => {
                if let Some(dir) = button_dir(action.button) {
                    if matches!(dir, PadDir::Up | PadDir::Down) {
                        self.move_gamepad_location_picker(ctx, dir);
                    }
                    return None;
                }
                match action.button {
                    PadButton::South if action.kind == PadActionKind::Press => {
                        self.confirm_gamepad_location_picker(ctx)
                    }
                    PadButton::East | PadButton::Select if action.kind == PadActionKind::Press => {
                        self.gamepad_location_picker = None;
                        self.gamepad_state.require_directional_neutral();
                        ctx.request_repaint();
                        None
                    }
                    _ => None,
                }
            }
        }
    }

    fn dispatch_gamepad_location_picker_analog(
        &mut self,
        ctx: &egui::Context,
        now: Instant,
    ) -> bool {
        let stick = stick_pair(&self.gamepad_state, PadAxis::LeftX, PadAxis::LeftY);
        let stick_dir = dominant_stick_dir(stick);
        let active = stick_dir.is_some();
        let due = self
            .gamepad_state
            .left_stick_step_due(active, now, STICK_STEP_INTERVAL);
        if due && let Some(dir @ (PadDir::Up | PadDir::Down)) = stick_dir {
            self.move_gamepad_location_picker(ctx, dir);
        }
        active
    }

    fn move_gamepad_location_picker(&mut self, ctx: &egui::Context, dir: PadDir) {
        let Some(picker) = self.gamepad_location_picker.as_mut() else {
            return;
        };
        let len = picker.entries.len();
        if len == 0 {
            self.gamepad_location_picker = None;
            self.gamepad_state.require_directional_neutral();
            ctx.request_repaint();
            return;
        }
        let delta = if dir == PadDir::Down { 1 } else { -1 };
        picker.selected = cycle_index(len, picker.selected, delta);
        update_location_picker_scroll(picker);
        ctx.request_repaint();
    }

    fn confirm_gamepad_location_picker(&mut self, ctx: &egui::Context) -> Option<AddressBarNav> {
        let selected = self
            .gamepad_location_picker
            .as_ref()
            .map(|picker| picker.selected)
            .unwrap_or(0);
        let nav = self
            .gamepad_location_picker
            .as_ref()
            .and_then(|picker| picker.entries.get(selected))
            .map(|entry| entry.nav.clone());
        self.gamepad_location_picker = None;
        self.gamepad_state.require_directional_neutral();
        ctx.request_repaint();

        let Some(nav) = nav else {
            return None;
        };
        let nav = match nav {
            GamepadLocationNav::DriveList => AddressBarNav::DriveList(None),
            GamepadLocationNav::ReadingHistory => AddressBarNav::ReadingHistory,
            GamepadLocationNav::RatingView(stars) => {
                self.bump_input_seq("gamepad_rating_view", Some(&stars.to_string()));
                self.enter_rating_view(stars);
                return None;
            }
            GamepadLocationNav::BooksRoot => AddressBarNav::BooksRoot,
            GamepadLocationNav::Direct(path) => {
                let Some(resolved) = crate::folder_tree::resolve_openable_path(&path) else {
                    self.show_feedback_toast(format!("場所が見つかりません: {}", path.display()));
                    return None;
                };
                AddressBarNav::Direct(resolved)
            }
        };
        self.bump_input_seq("gamepad_location_nav", Some(&format!("{nav:?}")));
        Some(nav)
    }

    fn ring_picker_is_stale(&self) -> bool {
        self.ring_picker.as_ref().is_some_and(|picker| {
            picker.context != self.current_ring_shortcut_context()
                || picker.anchor != self.current_ring_picker_anchor(picker.context)
        })
    }

    fn close_stale_ring_picker(&mut self, ctx: &egui::Context) {
        if self.ring_picker.take().is_some() {
            self.clear_native_video_picker_overlay(ctx);
            self.gamepad_state.cancel_west_ring();
            self.gamepad_state.require_directional_neutral();
            ctx.request_repaint();
        }
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
            self.handle_picker_list_direction(ctx, dir);
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
                    self.change_ring_picker_value(ctx, row, delta);
                    if let Some(picker) = self.ring_picker.as_mut() {
                        picker.x_close_armed = false;
                    }
                    ctx.request_repaint();
                    self.sync_native_video_picker_overlay(ctx);
                }
            }
        }
    }

    fn change_ring_picker_value(&mut self, ctx: &egui::Context, row: RingPickerRowId, delta: i32) {
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
            RingPickerRowId::ReadingDirection => {
                const VALUES: &[ReadingDirection] = &[ReadingDirection::Ltr, ReadingDirection::Rtl];
                picker.reading_direction = cycle_value(VALUES, picker.reading_direction, delta);
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
            RingPickerRowId::PostFilter => {
                picker.post_filter = cycle_value(PostFilter::ALL, picker.post_filter, delta);
                mark_picker_dirty(picker, row);
            }
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
        self.preview_ring_picker_row(ctx, row);
    }

    fn preview_ring_picker_row(&mut self, ctx: &egui::Context, row: RingPickerRowId) {
        let Some(picker) = self.ring_picker.clone() else {
            return;
        };
        match picker.context {
            RingShortcutContext::Grid => self.preview_grid_picker_row(&picker, row),
            RingShortcutContext::ImageFullscreen => {
                if let Some(fs_idx) = self.fullscreen_idx {
                    self.preview_image_picker_row(ctx, fs_idx, &picker, row);
                }
            }
            RingShortcutContext::VideoFullscreen => {
                if let Some(fs_idx) = self.fullscreen_idx {
                    self.preview_video_picker_row(ctx, fs_idx, &picker, row);
                }
            }
        }
    }

    fn preview_grid_picker_row(&mut self, picker: &RingPickerState, row: RingPickerRowId) {
        match row {
            RingPickerRowId::GridColumns if self.settings.grid_cols != picker.grid_cols => {
                self.set_grid_view_mode(GridViewMode::Thumbnail);
                self.settings.grid_cols = picker.grid_cols;
                self.scroll_to_selected = true;
                self.settings.save();
            }
            RingPickerRowId::GridSortOrder if self.settings.sort_order != picker.sort_order => {
                self.apply_grid_picker_sort_order(picker.sort_order);
            }
            RingPickerRowId::GridThumbAspect
                if self.settings.thumb_aspect_auto != picker.thumb_aspect_auto
                    || self.settings.thumb_aspect != picker.thumb_aspect =>
            {
                self.apply_picker_thumb_aspect(picker.thumb_aspect_auto, picker.thumb_aspect);
            }
            RingPickerRowId::ItemRating => {
                self.preview_picker_item_rating(picker, picker.item_rating);
            }
            RingPickerRowId::ContainerRating
                if self.current_folder_rating() != picker.container_rating =>
            {
                self.preview_current_folder_rating(picker.container_rating);
            }
            _ => {}
        }
    }

    fn preview_image_picker_row(
        &mut self,
        ctx: &egui::Context,
        fs_idx: usize,
        picker: &RingPickerState,
        row: RingPickerRowId,
    ) {
        let layout_supported = self.vertical_reading_supported_idx(fs_idx);
        match row {
            RingPickerRowId::SpreadMode
                if layout_supported && self.spread_mode != picker.spread_mode =>
            {
                self.apply_fullscreen_spread_mode(ctx, fs_idx, picker.spread_mode);
            }
            RingPickerRowId::ReadingFlow
                if layout_supported && self.reading_flow != picker.reading_flow =>
            {
                self.set_reading_flow_for_fullscreen(ctx, fs_idx, picker.reading_flow);
            }
            RingPickerRowId::ReadingDirection
                if layout_supported && self.reading_direction != picker.reading_direction =>
            {
                self.set_reading_direction_for_fullscreen(ctx, fs_idx, picker.reading_direction);
            }
            RingPickerRowId::FitMode if layout_supported => {
                let fit = picker.fit_mode.effective_for_flow(self.reading_flow);
                if self
                    .settings
                    .fullscreen_fit_mode
                    .effective_for_flow(self.reading_flow)
                    != fit
                {
                    self.set_fullscreen_fit_mode_for_current(ctx, fs_idx, fit);
                }
            }
            RingPickerRowId::ItemRating => {
                self.preview_picker_item_rating(picker, picker.item_rating);
            }
            RingPickerRowId::ContainerRating
                if self.current_folder_rating() != picker.container_rating =>
            {
                self.preview_current_folder_rating(picker.container_rating);
            }
            RingPickerRowId::PostFilter
                if self.reading_flow.is_paged()
                    && self.effective_params(fs_idx).post_filter != picker.post_filter =>
            {
                self.preview_picker_post_filter(fs_idx, picker.post_filter);
            }
            // UpscaleModel remains commit-only to avoid expensive redraws per repeat.
            _ => {}
        }
    }

    fn preview_video_picker_row(
        &mut self,
        ctx: &egui::Context,
        fs_idx: usize,
        picker: &RingPickerState,
        row: RingPickerRowId,
    ) {
        match row {
            RingPickerRowId::VideoVolume
                if (self.settings.video_volume - picker.video_volume).abs() > 1.0e-9 =>
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
            RingPickerRowId::VideoPlaybackSpeed
                if (self.video_playback_speed - picker.video_playback_speed).abs() > 1.0e-9 =>
            {
                #[cfg(windows)]
                self.handle_video_playback_speed_command(ctx, fs_idx, picker.video_playback_speed);
                #[cfg(not(windows))]
                {
                    let speed =
                        crate::video::clock::clamp_playback_speed(picker.video_playback_speed);
                    self.video_playback_speed = speed;
                    self.settings.video_playback_speed = speed;
                    self.settings.save();
                }
            }
            RingPickerRowId::VideoContinuousMode
                if self.video_continuous_mode != picker.video_continuous_mode =>
            {
                self.set_video_continuous_mode_common(ctx, fs_idx, picker.video_continuous_mode);
            }
            RingPickerRowId::ItemRating => {
                self.preview_picker_item_rating(picker, picker.item_rating);
            }
            RingPickerRowId::ContainerRating
                if self.current_folder_rating() != picker.container_rating =>
            {
                self.preview_current_folder_rating(picker.container_rating);
            }
            _ => {}
        }
    }

    fn preview_picker_item_rating(&mut self, picker: &RingPickerState, stars: u8) {
        if picker.original.item_rating_records.is_empty() {
            return;
        }
        let mut touched = Vec::with_capacity(picker.original.item_rating_records.len());
        for &(idx, _) in &picker.original.item_rating_records {
            self.set_rating(idx, stars);
            touched.push(idx);
        }
        if self.global_search.active {
            self.refresh_global_search_hit_stars(&touched);
        }
        if self.global_search.active && self.items_are_global_search_view {
            self.rebuild_items_from_global_search();
        } else {
            self.rebuild_visible_indices();
        }
    }

    fn enter_ring_picker_list_for_current_row(&mut self, ctx: &egui::Context) -> bool {
        let ai_mode = self.settings.ai_feature_mode;
        let Some(picker) = self.ring_picker.as_mut() else {
            return false;
        };
        let Some(&row) = picker_rows_for_context(picker.context).get(picker.current_row()) else {
            return false;
        };
        let list = if row == RingPickerRowId::PostFilter {
            PickerListState {
                mode: PickerListMode::PostFilterGroup,
                selected: post_filter_group_index(picker.post_filter),
            }
        } else if picker_row_supports_value_list(row) {
            PickerListState {
                mode: PickerListMode::RowValues(row),
                selected: picker_row_value_index(picker, row, ai_mode),
            }
        } else {
            return false;
        };
        picker.drill = Some(list);
        picker.x_close_armed = false;
        ctx.request_repaint();
        self.sync_native_video_picker_overlay(ctx);
        true
    }

    fn confirm_ring_picker_list(&mut self, ctx: &egui::Context) {
        let mut post_filter = None;
        let mut row_selection = None;
        let mut needs_sync = false;
        if let Some(picker) = self.ring_picker.as_mut()
            && let Some(list) = picker.drill
        {
            match list.mode {
                PickerListMode::PostFilterGroup => {
                    let group = list
                        .selected
                        .min(POST_FILTER_GROUPS.len().saturating_sub(1));
                    let item = post_filter_item_index_in_group(picker.post_filter, group);
                    picker.drill = Some(PickerListState {
                        mode: PickerListMode::PostFilterItem { group },
                        selected: item,
                    });
                    needs_sync = true;
                }
                PickerListMode::PostFilterItem { group } => {
                    post_filter = POST_FILTER_GROUPS
                        .get(group)
                        .and_then(|g| g.filters.get(list.selected))
                        .copied();
                    picker.drill = None;
                    needs_sync = true;
                }
                PickerListMode::RowValues(row) => {
                    row_selection = Some((row, list.selected));
                    picker.drill = None;
                    needs_sync = true;
                }
            }
            picker.x_close_armed = false;
        }
        if let Some(filter) = post_filter {
            self.set_ring_picker_post_filter_selection(ctx, filter);
        }
        if let Some((row, index)) = row_selection {
            self.set_ring_picker_row_value_selection(ctx, row, index);
        } else if needs_sync {
            ctx.request_repaint();
            self.sync_native_video_picker_overlay(ctx);
        }
    }

    fn handle_picker_list_direction(&mut self, ctx: &egui::Context, dir: PadDir) {
        if !matches!(dir, PadDir::Up | PadDir::Down) {
            return;
        }
        let ai_mode = self.settings.ai_feature_mode;
        let mut needs_sync = false;
        if let Some(picker) = self.ring_picker.as_mut()
            && let Some(mut list) = picker.drill
        {
            let len = picker_list_len_for_state(picker, list, ai_mode);
            if len > 0 {
                let delta = if dir == PadDir::Down { 1 } else { -1 };
                list.selected = cycle_index(len, list.selected, delta);
                picker.drill = Some(list);
                picker.x_close_armed = false;
                needs_sync = true;
            }
        }
        if needs_sync {
            ctx.request_repaint();
            self.sync_native_video_picker_overlay(ctx);
        }
    }

    fn set_ring_picker_post_filter_selection(&mut self, ctx: &egui::Context, filter: PostFilter) {
        let mut changed = false;
        if let Some(picker) = self.ring_picker.as_mut() {
            if picker.post_filter != filter {
                picker.post_filter = filter;
                mark_picker_dirty(picker, RingPickerRowId::PostFilter);
                changed = true;
            }
            picker.x_close_armed = false;
        }
        if changed {
            self.preview_ring_picker_row(ctx, RingPickerRowId::PostFilter);
        }
        ctx.request_repaint();
        self.sync_native_video_picker_overlay(ctx);
    }

    fn set_ring_picker_row_value_selection(
        &mut self,
        ctx: &egui::Context,
        row: RingPickerRowId,
        index: usize,
    ) {
        let ai_mode = self.settings.ai_feature_mode;
        let mut changed = false;
        if let Some(picker) = self.ring_picker.as_mut() {
            match row {
                RingPickerRowId::ItemRating => {
                    let value = index.min(5) as u8;
                    if picker.item_rating != value {
                        picker.item_rating = value;
                        changed = true;
                    }
                }
                RingPickerRowId::ContainerRating => {
                    let value = index.min(5) as u8;
                    if picker.container_rating != value {
                        picker.container_rating = value;
                        changed = true;
                    }
                }
                RingPickerRowId::SpreadMode => {
                    if let Some(&value) = SpreadMode::all().get(index)
                        && picker.spread_mode != value
                    {
                        picker.spread_mode = value;
                        changed = true;
                    }
                }
                RingPickerRowId::ReadingFlow => {
                    if let Some(&value) = ReadingFlow::all().get(index)
                        && picker.reading_flow != value
                    {
                        picker.reading_flow = value;
                        picker.fit_mode = picker.fit_mode.effective_for_flow(picker.reading_flow);
                        changed = true;
                    }
                }
                RingPickerRowId::ReadingDirection => {
                    const VALUES: &[ReadingDirection] =
                        &[ReadingDirection::Ltr, ReadingDirection::Rtl];
                    if let Some(&value) = VALUES.get(index)
                        && picker.reading_direction != value
                    {
                        picker.reading_direction = value;
                        changed = true;
                    }
                }
                RingPickerRowId::FitMode => {
                    if let Some(&value) =
                        FullscreenFitMode::selectable_for_flow(picker.reading_flow).get(index)
                    {
                        let value = value.effective_for_flow(picker.reading_flow);
                        if picker.fit_mode.effective_for_flow(picker.reading_flow) != value {
                            picker.fit_mode = value;
                            changed = true;
                        }
                    }
                }
                RingPickerRowId::UpscaleModel => {
                    let items = crate::adjustment::upscale_menu_items_for_mode(ai_mode);
                    if let Some((_, key)) = items.get(index)
                        && picker.upscale_model_key.as_deref() != *key
                    {
                        picker.upscale_model_key = key.map(|s| s.to_string());
                        changed = true;
                    }
                }
                RingPickerRowId::VideoPlaybackSpeed => {
                    if let Some(&value) = crate::video::clock::PLAYBACK_SPEED_CHOICES.get(index)
                        && (picker.video_playback_speed - value).abs() > 1.0e-9
                    {
                        picker.video_playback_speed = value;
                        changed = true;
                    }
                }
                RingPickerRowId::VideoContinuousMode => {
                    if let Some(&value) = video_continuous_mode_values().get(index)
                        && picker.video_continuous_mode != value
                    {
                        picker.video_continuous_mode = value;
                        changed = true;
                    }
                }
                _ => {}
            }
            if changed {
                mark_picker_dirty(picker, row);
            }
            picker.x_close_armed = false;
        }
        if changed {
            self.preview_ring_picker_row(ctx, row);
        }
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

    fn sync_native_video_favorite_picker_overlay(&mut self, ctx: &egui::Context) {
        #[cfg(windows)]
        {
            let overlay = (self.current_ring_shortcut_context()
                == RingShortcutContext::VideoFullscreen)
                .then(|| {
                    self.gamepad_favorite_picker
                        .as_ref()
                        .map(|picker| self.native_video_favorite_picker_overlay(picker))
                })
                .flatten();
            self.set_native_video_ring_picker_overlay(overlay);
            self.request_native_video_hud_repaint(ctx);
        }
        #[cfg(not(windows))]
        let _ = ctx;
    }

    fn sync_native_video_marker_picker_overlay(&mut self, ctx: &egui::Context) {
        #[cfg(windows)]
        {
            let picker = self.gamepad_video_marker_picker.clone();
            let overlay = (self.current_ring_shortcut_context()
                == RingShortcutContext::VideoFullscreen)
                .then(|| picker.map(|picker| self.native_video_marker_picker_overlay(&picker)))
                .flatten();
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

    pub(crate) fn sync_native_video_ring_guide_overlay(&mut self, ctx: &egui::Context) {
        #[cfg(windows)]
        {
            let context = self.current_ring_shortcut_context();
            // 音楽ビュー (音声ファイル or Inc 7 動画→音声モード) は VideoFullscreen context だが
            // native video presenter/HUD が無い / hidden なので、native ガイドは出さず egui
            // オーバーレイ (draw_mouse_ring_flick_overlay) が音楽ビュー上に直接リングを描く。
            // ここでは何も出さない (None で既存ガイドもクリア)。実 native 動画のときだけ HUD に描く。
            let music_view = self
                .fullscreen_idx
                .is_some_and(|idx| self.fs_music_view_active(idx));
            let overlay = (context == RingShortcutContext::VideoFullscreen && !music_view)
                .then(|| self.native_video_ring_guide_overlay(context))
                .flatten();
            self.set_native_video_ring_guide_overlay(overlay);
            self.request_native_video_hud_repaint(ctx);
        }
        #[cfg(not(windows))]
        let _ = ctx;
    }

    fn clear_native_video_ring_guide_overlay(&mut self, ctx: &egui::Context) {
        #[cfg(windows)]
        {
            self.set_native_video_ring_guide_overlay(None);
            self.request_native_video_hud_repaint(ctx);
        }
        #[cfg(not(windows))]
        let _ = ctx;
    }

    fn sync_native_video_mouse_gesture_overlay(&mut self, ctx: &egui::Context) {
        #[cfg(windows)]
        {
            // 音楽ビュー (音声ファイル or 動画→音声モード) は native HUD が無い / hidden なので
            // native ジェスチャガイドは出さず、egui オーバーレイ (draw_mouse_gesture_overlay) に
            // 任せる (ring guide と同じ)。
            let music_view = self
                .fullscreen_idx
                .is_some_and(|idx| self.fs_music_view_active(idx));
            if !self.settings.ring_shortcuts.mouse_gesture_help_visible || music_view {
                self.set_native_video_ring_picker_overlay(None);
                self.request_native_video_hud_repaint(ctx);
                return;
            }
            let overlay = self.native_video_mouse_gesture_overlay();
            self.set_native_video_ring_picker_overlay(overlay);
            self.request_native_video_hud_repaint(ctx);
        }
        #[cfg(not(windows))]
        let _ = ctx;
    }

    fn clear_native_video_mouse_gesture_overlay(&mut self, ctx: &egui::Context) {
        #[cfg(windows)]
        {
            self.set_native_video_ring_picker_overlay(None);
            self.request_native_video_hud_repaint(ctx);
        }
        #[cfg(not(windows))]
        let _ = ctx;
    }

    #[cfg(windows)]
    fn native_video_mouse_gesture_overlay(
        &self,
    ) -> Option<crate::video::native_presenter::NativeOverlayRingPicker> {
        let gesture = self.mouse_gesture.as_ref()?;
        if gesture.context != RightDragContext::VideoFullscreen || !gesture.guide_visible() {
            return None;
        }
        let profile = self
            .settings
            .ring_shortcuts
            .mouse_gesture_profile(RightDragContext::VideoFullscreen);
        let action_context = RightDragContext::VideoFullscreen.gesture_action_context();
        let mut rows: Vec<_> = profile
            .bindings
            .iter()
            .map(
                |binding| crate::video::native_presenter::NativeOverlayRingPickerRow {
                    label: format_mouse_gesture_pattern(&binding.pattern),
                    value: binding.action.label_for_context(action_context).to_string(),
                },
            )
            .collect();
        if rows.is_empty() {
            rows.push(crate::video::native_presenter::NativeOverlayRingPickerRow {
                label: "未登録".to_string(),
                value: "環境設定で追加".to_string(),
            });
        }
        let selected_row = profile.binding_index_for_pattern(&gesture.pattern);
        let current = if gesture.pattern.is_empty() {
            "-".to_string()
        } else {
            format_mouse_gesture_pattern(&gesture.pattern)
        };
        Some(crate::video::native_presenter::NativeOverlayRingPicker {
            title: format!("マウスジェスチャ / 入力中: {current}"),
            rows,
            selected_row,
            footer: "右ドラッグで軌跡を入力、離すと実行 / 短押しは閉じる".to_string(),
            drill: None,
        })
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
            crate::video::native_presenter::NativeOverlayRingPickerDrill {
                title: self.picker_list_title(drill).to_string(),
                items: self.picker_list_items(picker, drill),
                selected: drill.selected,
                footer: "上下:選択  A:選択  B/X:確定して閉じる".to_string(),
            }
        });
        crate::video::native_presenter::NativeOverlayRingPicker {
            title: format!("ピッカー / {}", picker.context.label()),
            rows,
            selected_row: Some(picker.current_row()),
            footer: "上下:選択  左右:変更  A:一覧  B/X:確定".to_string(),
            drill,
        }
    }

    #[cfg(windows)]
    fn native_video_favorite_picker_overlay(
        &self,
        picker: &GamepadFavoritePickerState,
    ) -> crate::video::native_presenter::NativeOverlayRingPicker {
        let rows = self
            .settings
            .favorites
            .iter()
            .map(
                |favorite| crate::video::native_presenter::NativeOverlayRingPickerRow {
                    label: favorite.name.clone(),
                    value: favorite.path.display().to_string(),
                },
            )
            .collect();
        crate::video::native_presenter::NativeOverlayRingPicker {
            title: "お気に入り".to_string(),
            rows,
            selected_row: Some(picker.selected),
            footer: "上下:選択  A:移動  B/Start:閉じる".to_string(),
            drill: None,
        }
    }

    #[cfg(windows)]
    fn native_video_marker_picker_overlay(
        &mut self,
        picker: &GamepadVideoMarkerPickerState,
    ) -> crate::video::native_presenter::NativeOverlayRingPicker {
        let markers = self
            .fullscreen_idx
            .map(|fs_idx| self.collect_video_nav_markers(fs_idx))
            .unwrap_or_default();
        let rows = markers
            .iter()
            .map(
                |marker| crate::video::native_presenter::NativeOverlayRingPickerRow {
                    label: video_marker_primary_label(marker),
                    value: video_marker_secondary_label(marker),
                },
            )
            .collect();
        crate::video::native_presenter::NativeOverlayRingPicker {
            title: "ブックマーク / チャプター".to_string(),
            rows,
            selected_row: Some(picker.selected),
            footer: "上下:選択  A:移動  B/Select:閉じる".to_string(),
            drill: None,
        }
    }

    #[cfg(windows)]
    fn native_video_ring_guide_overlay(
        &self,
        context: RingShortcutContext,
    ) -> Option<crate::video::native_presenter::NativeOverlayRingGuide> {
        if context != RingShortcutContext::VideoFullscreen || self.ring_picker.is_some() {
            return None;
        }
        let (selected, heading, detail, center_client_px) = if self.gamepad_state.west_ring_active()
        {
            let selected = self.gamepad_state.west_ring_direction();
            let (heading, detail) = ring_guide_heading_detail(
                &self.settings.ring_shortcuts.profile(context).slots,
                context,
                selected,
                "X",
                "方向なしで離すとピッカー",
            );
            (selected, heading, detail, None)
        } else if self.settings.ring_shortcuts.mouse_ring_enabled(context) {
            if !self.settings.ring_shortcuts.mouse_ring_help_visible {
                return None;
            }
            let flick = self.mouse_ring_flick.as_ref()?;
            if flick.context != context || !flick.guide_visible() {
                return None;
            }
            let selected = mouse_flick_direction(flick);
            let (heading, detail) = ring_guide_heading_detail(
                &self.settings.ring_shortcuts.profile(context).slots,
                context,
                selected,
                "右ドラッグ",
                "中央で離すと取消",
            );
            (selected, heading, detail, Some(flick.start_pos))
        } else {
            return None;
        };
        Some(crate::video::native_presenter::NativeOverlayRingGuide {
            heading,
            detail,
            selected_slot: selected.map(RingDirection::slot_index),
            center_client_px,
            slots: RingDirection::all()
                .iter()
                .map(|&direction| {
                    let action = self
                        .settings
                        .ring_shortcuts
                        .profile(context)
                        .slots
                        .get(direction.slot_index())
                        .cloned()
                        .unwrap_or_default();
                    crate::video::native_presenter::NativeOverlayRingGuideSlot {
                        short_label: ring_direction_short_label(direction).to_string(),
                        action_label: action.label_for_context(context).to_string(),
                    }
                })
                .collect(),
        })
    }

    fn commit_ring_picker(&mut self, ctx: &egui::Context) {
        let Some(picker) = self.ring_picker.take() else {
            return;
        };
        if picker.context == RingShortcutContext::VideoFullscreen {
            self.clear_native_video_picker_overlay(ctx);
        }
        self.gamepad_state.cancel_west_ring();
        self.gamepad_state.require_directional_neutral();
        self.commit_live_picker_undo(&picker);
        self.apply_ring_picker_state(ctx, picker);
        ctx.request_repaint();
    }

    fn commit_live_picker_undo(&mut self, picker: &RingPickerState) {
        if picker.dirty_rows.contains(&RingPickerRowId::ItemRating) {
            let records: Vec<(usize, u8, u8)> = picker
                .original
                .item_rating_records
                .iter()
                .filter_map(|&(idx, before)| {
                    (before != picker.item_rating).then_some((idx, before, picker.item_rating))
                })
                .collect();
            if !records.is_empty() {
                let summary = if records.len() > 1 {
                    if picker.item_rating == 0 {
                        format!("★解除を {} 件に適用", records.len())
                    } else {
                        format!("★{} を {} 件に付与", picker.item_rating, records.len())
                    }
                } else if picker.item_rating == 0 {
                    "★解除".to_string()
                } else {
                    format!("★{}", picker.item_rating)
                };
                self.capture_rating_undo(records, summary);
            }
        }
        if picker
            .dirty_rows
            .contains(&RingPickerRowId::ContainerRating)
            && picker.original.container_rating != picker.container_rating
        {
            self.capture_container_rating_undo(
                picker.original.container_rating,
                picker.container_rating,
            );
        }
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
            self.apply_grid_picker_sort_order(picker.sort_order);
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

    fn apply_grid_picker_sort_order(&mut self, sort_order: SortOrder) {
        if self.settings.sort_order == sort_order {
            return;
        }
        self.settings.sort_order = sort_order;
        self.settings.save();
        self.apply_sort_change_reload();
        self.scroll_to_selected = true;
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
            if picker
                .dirty_rows
                .contains(&RingPickerRowId::ReadingDirection)
                && self.reading_direction != picker.reading_direction
            {
                self.set_reading_direction_for_fullscreen(ctx, fs_idx, picker.reading_direction);
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
                if self.effective_params(fs_idx).post_filter != picker.original.post_filter {
                    self.preview_picker_post_filter(fs_idx, picker.original.post_filter);
                }
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

    fn preview_picker_post_filter(&mut self, fs_idx: usize, next: PostFilter) {
        let scope = self.resolve_adjust_scope(fs_idx);
        let old_params = self.effective_params(fs_idx).clone();
        if old_params.post_filter == next {
            return;
        }
        let mut params = old_params.clone();
        params.post_filter = next;
        match scope {
            crate::ui_fullscreen::AdjustScope::PageOverride => {
                self.adjustment_page_params.insert(fs_idx, params.clone());
            }
            crate::ui_fullscreen::AdjustScope::FavoriteDefault(id) => {
                self.adjustment_favorite_params.insert(id, params.clone());
            }
            crate::ui_fullscreen::AdjustScope::Global => {
                self.settings.global_preset = params.clone();
            }
        }
        self.clear_caches_for_param_change(fs_idx, &old_params, &params);
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
                app.clear_caches_for_param_change(fs_idx, &old_params, &params);
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
        self.trigger_ring_shortcut_action(ctx, context, direction, "gamepad-ring")
    }

    pub(crate) fn trigger_ring_shortcut_action(
        &mut self,
        ctx: &egui::Context,
        context: RingShortcutContext,
        direction: RingDirection,
        source: &'static str,
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
        self.apply_ring_action(ctx, context, action, source)
    }

    pub(crate) fn current_ring_shortcut_context(&self) -> RingShortcutContext {
        if let Some(fs_idx) = self.fullscreen_idx {
            if self.fullscreen_uses_video_ring_context(fs_idx) {
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
        self.apply_mouse_button(
            ctx,
            if forward {
                MouseButtonSlot::Forward
            } else {
                MouseButtonSlot::Back
            },
            source,
        )
    }

    pub(crate) fn update_mouse_middle_short_click(
        &mut self,
        ctx: &egui::Context,
        allow_start: bool,
    ) -> bool {
        let (is_down, is_pressed, is_released, current_pos) = ctx.input(|i| {
            (
                i.pointer.button_down(egui::PointerButton::Middle),
                i.pointer.button_pressed(egui::PointerButton::Middle),
                i.pointer.button_released(egui::PointerButton::Middle),
                i.pointer.interact_pos(),
            )
        });

        if is_pressed {
            self.mouse_middle_click_start = if allow_start {
                current_pos.map(|pos| (pos, Instant::now(), false))
            } else {
                None
            };
        }
        if is_down || is_released {
            if let Some((start, _, cancelled)) = self.mouse_middle_click_start.as_mut() {
                if let Some(pos) = current_pos {
                    if pos.distance(*start) > crate::ui_fullscreen::MIDDLE_DRAG_THRESHOLD_PX {
                        *cancelled = true;
                    }
                }
            }
        }
        if is_released {
            return matches!(
                self.mouse_middle_click_start.take(),
                Some((_, started_at, false))
                    if started_at.elapsed() <= Duration::from_millis(500)
            );
        }
        if !is_down && !is_pressed {
            self.mouse_middle_click_start = None;
        }
        false
    }

    pub(crate) fn apply_mouse_button(
        &mut self,
        ctx: &egui::Context,
        slot: MouseButtonSlot,
        source: &'static str,
    ) -> Option<AddressBarNav> {
        let context = self.current_ring_shortcut_context();
        let action = self
            .settings
            .ring_shortcuts
            .mouse_button_profile(context)
            .action(slot);
        if !action.is_valid_for_mouse_button_context(context) {
            crate::logger::log(format!(
                "[input-nav] source={source} ignored invalid mouse button action={} context={context:?}",
                action.as_str()
            ));
            return None;
        }
        if matches!(action, RingActionId::None | RingActionId::Unknown(_)) {
            return None;
        }
        crate::logger::log(format!(
            "[input-nav] source={source} mouse_button={} action={} context={context:?}",
            slot.log_name(),
            action.as_str()
        ));
        self.apply_ring_action(ctx, context, action, source)
    }

    fn apply_folder_history_nav(
        &mut self,
        forward: bool,
        source: &'static str,
    ) -> Option<AddressBarNav> {
        if self.block_gamepad_grid_folder_nav_for_detached_foreground(
            source,
            if forward {
                "history_forward"
            } else {
                "history_back"
            },
        ) {
            return None;
        }
        if self.is_snapshot_active() || self.items_are_drive_list {
            return None;
        }
        let nav = if forward {
            AddressBarNav::HistoryForward
        } else {
            AddressBarNav::HistoryBack
        };
        crate::logger::log(format!(
            "[input-nav] source={source} action=folder_history_{}",
            if forward { "forward" } else { "back" }
        ));
        Some(nav)
    }

    fn location_navigation_blocked(&mut self) -> bool {
        if self.is_snapshot_active() {
            self.show_feedback_toast(
                "スナップショット中は他のフォルダに移動できません".to_string(),
            );
            return true;
        }
        if self.global_search.active || self.favsearch.active || self.tag_view.active {
            self.show_feedback_toast("検索中は場所ジャンプを使用できません".to_string());
            return true;
        }
        false
    }

    fn apply_ring_favorite_slot(
        &mut self,
        ctx: &egui::Context,
        slot: usize,
        source: &'static str,
    ) -> Option<AddressBarNav> {
        if self.is_snapshot_active() {
            self.show_feedback_toast(
                "スナップショット中は他のフォルダに移動できません".to_string(),
            );
            return None;
        }
        let Some(target) = self
            .settings
            .favorites
            .get(slot.saturating_sub(1))
            .map(|favorite| favorite.path.clone())
        else {
            self.show_feedback_toast(format!("お気に入り {slot} は未登録です"));
            return None;
        };
        if self.fullscreen_idx.is_some() {
            self.close_fullscreen();
        }
        self.bump_input_seq(source, Some(&format!("favorite_slot={slot}")));
        ctx.request_repaint();
        Some(AddressBarNav::Direct(target))
    }

    fn apply_ring_drive_letter(
        &mut self,
        letter: char,
        source: &'static str,
    ) -> Option<AddressBarNav> {
        if self.location_navigation_blocked() {
            return None;
        }
        let path = super::drive_root_path_for_letter(letter)?;
        let Some(resolved) = crate::folder_tree::resolve_openable_path(&path) else {
            self.show_feedback_toast(format!("ドライブが見つかりません: {}", path.display()));
            return None;
        };
        if self.fullscreen_idx.is_some() {
            self.close_fullscreen();
        }
        self.bump_input_seq(source, Some(&format!("drive={}", path.display())));
        Some(AddressBarNav::Direct(resolved))
    }

    fn apply_current_drive_root(&mut self, source: &'static str) -> Option<AddressBarNav> {
        if self.location_navigation_blocked() {
            return None;
        }
        let Some(path) = self.current_location_root() else {
            self.show_feedback_toast("現在位置のルートディレクトリが見つかりません".to_string());
            return None;
        };
        let Some(resolved) = crate::folder_tree::resolve_openable_path(&path) else {
            self.show_feedback_toast(format!(
                "ルートディレクトリが見つかりません: {}",
                path.display()
            ));
            return None;
        };
        if self.fullscreen_idx.is_some() {
            self.close_fullscreen();
        }
        self.bump_input_seq(
            source,
            Some(&format!("current_drive_root={}", path.display())),
        );
        Some(AddressBarNav::Direct(resolved))
    }

    fn apply_switch_drive_letter(
        &mut self,
        letter: char,
        source: &'static str,
    ) -> Option<AddressBarNav> {
        if self.location_navigation_blocked() {
            return None;
        }
        let root = super::drive_root_path_for_letter(letter)?;
        let drive_key = super::drive_current_key_for_letter(letter)?;
        let remembered = self.active_drive_current_dir(letter);
        let remembered_resolved = remembered.as_ref().and_then(|path| {
            let resolved = crate::folder_tree::resolve_openable_path(path)?;
            (super::drive_current_key_for_path(&resolved) == Some(drive_key.clone()))
                .then_some(resolved)
        });
        let target = if let Some(path) = remembered_resolved {
            path
        } else {
            if let Some(path) = remembered.as_ref() {
                self.show_feedback_toast(format!(
                    "{} の最後の場所が見つかりません。ルートへ移動します: {}",
                    drive_key,
                    path.display()
                ));
            }
            let Some(resolved_root) = crate::folder_tree::resolve_openable_path(&root) else {
                self.show_feedback_toast(format!("ドライブが見つかりません: {}", root.display()));
                return None;
            };
            resolved_root
        };
        if self.fullscreen_idx.is_some() {
            self.close_fullscreen();
        }
        self.bump_input_seq(
            source,
            Some(&format!(
                "switch_drive={} target={}",
                drive_key,
                target.display()
            )),
        );
        Some(AddressBarNav::Direct(target))
    }

    fn apply_ring_location_action(
        &mut self,
        ctx: &egui::Context,
        action: &RingActionId,
        source: &'static str,
    ) -> Option<AddressBarNav> {
        if self.location_navigation_blocked() {
            return None;
        }
        if let Some(stars) = action.location_rating_stars() {
            self.bump_input_seq(source, Some(&format!("rating_view={stars}")));
            self.enter_rating_view(stars);
            return None;
        }
        let nav = match action {
            RingActionId::OpenLocationDriveList => AddressBarNav::DriveList(None),
            RingActionId::OpenLocationReadingHistory => AddressBarNav::ReadingHistory,
            RingActionId::OpenLocationBooksRoot => AddressBarNav::BooksRoot,
            RingActionId::OpenLocationDesktop => {
                self.quick_location_nav(crate::known_folders::desktop_dir(), "デスクトップ")?
            }
            RingActionId::OpenLocationPictures => {
                self.quick_location_nav(crate::known_folders::pictures_dir(), "ピクチャ")?
            }
            RingActionId::OpenLocationDownloads => {
                self.quick_location_nav(crate::known_folders::downloads_dir(), "ダウンロード")?
            }
            _ => return None,
        };
        if self.fullscreen_idx.is_some() {
            self.close_fullscreen();
        }
        self.bump_input_seq(source, Some(&format!("{nav:?}")));
        ctx.request_repaint();
        Some(nav)
    }

    pub(crate) fn apply_location_navigation_key_action(
        &mut self,
        ctx: &egui::Context,
        action: KeyAction,
        source: &'static str,
    ) -> Option<AddressBarNav> {
        if let Some(slot) = action.favorite_slot_number() {
            return self.apply_ring_favorite_slot(ctx, slot, source);
        }
        if let Some(letter) = action.drive_letter() {
            return self.apply_ring_drive_letter(letter, source);
        }
        if action == KeyAction::GridOpenCurrentDriveRoot {
            return self.apply_current_drive_root(source);
        }
        if let Some(letter) = action.switch_drive_letter() {
            return self.apply_switch_drive_letter(letter, source);
        }
        if let Some(ring_action) = ring_location_action_for_key_action(action) {
            return self.apply_ring_location_action(ctx, &ring_action, source);
        }
        match action {
            KeyAction::GridFavoritePrev => self.apply_favorite_cycle_nav(ctx, false, source),
            KeyAction::GridFavoriteNext => self.apply_favorite_cycle_nav(ctx, true, source),
            _ => None,
        }
    }

    pub(crate) fn apply_pinned_tag_key_action(&mut self, action: KeyAction, source: &'static str) {
        let Some(slot) = action.pinned_tag_slot_number() else {
            return;
        };
        let Some(name) = self
            .settings
            .tags
            .iter()
            .filter(|tag| tag.show_shortcut)
            .nth(slot.saturating_sub(1))
            .map(|tag| tag.name.clone())
        else {
            self.show_feedback_toast(format!("ピン留めタグ {slot} は未登録です"));
            return;
        };
        self.bump_input_seq(source, Some(&format!("pinned_tag_slot={slot}")));
        self.request_tag_toggle_for_selection(&name);
    }

    fn apply_favorite_cycle_nav(
        &mut self,
        ctx: &egui::Context,
        forward: bool,
        source: &'static str,
    ) -> Option<AddressBarNav> {
        if self.is_snapshot_active() {
            self.show_feedback_toast(
                "スナップショット中は他のフォルダに移動できません".to_string(),
            );
            return None;
        }
        let len = self.settings.favorites.len();
        if len == 0 {
            self.show_feedback_toast("お気に入りが登録されていません".to_string());
            return None;
        }
        let current = self.current_gamepad_favorite_index();
        let next = match (current, forward) {
            (Some(idx), true) => (idx + 1) % len,
            (Some(idx), false) => (idx + len - 1) % len,
            (None, true) => 0,
            (None, false) => len - 1,
        };
        self.apply_ring_favorite_slot(ctx, next + 1, source)
    }

    fn quick_location_nav(
        &mut self,
        path: Option<std::path::PathBuf>,
        label: &'static str,
    ) -> Option<AddressBarNav> {
        let Some(path) = path else {
            self.show_feedback_toast(format!("{label} が見つかりません"));
            return None;
        };
        let Some(resolved) = crate::folder_tree::resolve_openable_path(&path) else {
            self.show_feedback_toast(format!("場所が見つかりません: {}", path.display()));
            return None;
        };
        Some(AddressBarNav::Direct(resolved))
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
            if self.block_gamepad_grid_folder_nav_for_detached_foreground(
                source,
                if forward { "tree_forward" } else { "tree_back" },
            ) {
                return;
            }
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
        } else if self.items_are_subfolder_expansion_view {
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
            if self.block_gamepad_grid_folder_nav_for_detached_foreground(
                source,
                if forward {
                    "sibling_forward"
                } else {
                    "sibling_back"
                },
            ) {
                return;
            }
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
            } else if self.items_are_subfolder_expansion_view {
                self.cancel_pending_folder_nav();
            } else if let Some(cur) = self.effective_folder() {
                self.start_folder_nav(cur, forward, FolderNavMode::SiblingGrid);
            }
        }
    }

    fn block_gamepad_grid_folder_nav_for_detached_foreground(
        &mut self,
        source: &'static str,
        detail: &'static str,
    ) -> bool {
        #[cfg(windows)]
        if self.active_detached_viewer_has_foreground() {
            self.bump_input_seq(source, Some(detail));
            self.cancel_pending_folder_nav();
            self.show_feedback_toast(
                Self::nav_noop_title(crate::ui_fullscreen::FsNavNoOpReason::DetachedIndependent)
                    .to_string(),
            );
            return true;
        }

        let _ = (source, detail);
        false
    }

    fn apply_ring_action(
        &mut self,
        ctx: &egui::Context,
        context: RingShortcutContext,
        action: RingActionId,
        source: &'static str,
    ) -> Option<AddressBarNav> {
        if let Some(slot) = action.favorite_slot_number() {
            return self.apply_ring_favorite_slot(ctx, slot, source);
        }
        if let Some(letter) = action.drive_letter() {
            return self.apply_ring_drive_letter(letter, source);
        }
        if action.location_rating_stars().is_some()
            || matches!(
                action,
                RingActionId::OpenLocationDriveList
                    | RingActionId::OpenLocationReadingHistory
                    | RingActionId::OpenLocationBooksRoot
                    | RingActionId::OpenLocationDesktop
                    | RingActionId::OpenLocationPictures
                    | RingActionId::OpenLocationDownloads
            )
        {
            return self.apply_ring_location_action(ctx, &action, source);
        }

        match action {
            RingActionId::None | RingActionId::Unknown(_) => None,
            RingActionId::ToggleDetachedViewer => {
                self.toggle_detached_viewer_mode();
                None
            }
            RingActionId::ToggleWindowMode => {
                self.apply_ring_toggle_window_mode(ctx, context);
                None
            }
            RingActionId::ToggleMaximize => {
                self.toggle_main_window_maximized(ctx);
                None
            }
            RingActionId::MinimizeWindow => {
                self.apply_ring_minimize_window(ctx, context);
                None
            }
            RingActionId::CloseFullscreen
                if matches!(
                    context,
                    RingShortcutContext::ImageFullscreen | RingShortcutContext::VideoFullscreen
                ) =>
            {
                self.apply_ring_close_fullscreen(ctx, context);
                None
            }
            RingActionId::CycleFavorite => self.handle_gamepad_start(ctx),
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
            RingActionId::GridOpenSelectedAsPage if context == RingShortcutContext::Grid => self
                .open_selected_grid_container_with_mode(
                    ctx,
                    GridContainerOpenMode::PageFullscreen,
                    source,
                ),
            RingActionId::GridOpenSelectedAsList if context == RingShortcutContext::Grid => self
                .open_selected_grid_container_with_mode(
                    ctx,
                    GridContainerOpenMode::PageList,
                    source,
                ),
            RingActionId::GridColumnCount1 if context == RingShortcutContext::Grid => {
                self.apply_ring_grid_column_count(1);
                None
            }
            RingActionId::GridColumnCount2 if context == RingShortcutContext::Grid => {
                self.apply_ring_grid_column_count(2);
                None
            }
            RingActionId::GridColumnCount3 if context == RingShortcutContext::Grid => {
                self.apply_ring_grid_column_count(3);
                None
            }
            RingActionId::GridColumnCount4 if context == RingShortcutContext::Grid => {
                self.apply_ring_grid_column_count(4);
                None
            }
            RingActionId::GridColumnCount5 if context == RingShortcutContext::Grid => {
                self.apply_ring_grid_column_count(5);
                None
            }
            RingActionId::GridColumnCount6 if context == RingShortcutContext::Grid => {
                self.apply_ring_grid_column_count(6);
                None
            }
            RingActionId::GridColumnCount7 if context == RingShortcutContext::Grid => {
                self.apply_ring_grid_column_count(7);
                None
            }
            RingActionId::GridColumnCount8 if context == RingShortcutContext::Grid => {
                self.apply_ring_grid_column_count(8);
                None
            }
            RingActionId::GridColumnCount9 if context == RingShortcutContext::Grid => {
                self.apply_ring_grid_column_count(9);
                None
            }
            RingActionId::GridColumnCount10 if context == RingShortcutContext::Grid => {
                self.apply_ring_grid_column_count(10);
                None
            }
            RingActionId::GridHistoryBack => self.apply_folder_history_nav(false, source),
            RingActionId::GridHistoryForward => self.apply_folder_history_nav(true, source),
            RingActionId::GridParentFolder if context == RingShortcutContext::Grid => {
                self.handle_gamepad_grid_back()
            }
            RingActionId::TreeFolderPrev => {
                self.apply_tree_folder_nav(ctx, false, source);
                None
            }
            RingActionId::TreeFolderNext => {
                self.apply_tree_folder_nav(ctx, true, source);
                None
            }
            RingActionId::SiblingFolderPrev => {
                self.apply_sibling_folder_nav(ctx, false, source);
                None
            }
            RingActionId::SiblingFolderNext => {
                self.apply_sibling_folder_nav(ctx, true, source);
                None
            }
            RingActionId::ImageHome if context == RingShortcutContext::ImageFullscreen => {
                if let Some(fs_idx) = self.fullscreen_idx {
                    self.handle_fullscreen_boundary_jump(ctx, fs_idx, false, source);
                }
                None
            }
            RingActionId::ImageEnd if context == RingShortcutContext::ImageFullscreen => {
                if let Some(fs_idx) = self.fullscreen_idx {
                    self.handle_fullscreen_boundary_jump(ctx, fs_idx, true, source);
                }
                None
            }
            RingActionId::ImageSpreadShiftLeft
                if context == RingShortcutContext::ImageFullscreen =>
            {
                let dir = self.visual_spread_shift_dir(false);
                self.apply_ring_image_spread_shift(ctx, dir, source);
                None
            }
            RingActionId::ImageSpreadShiftRight
                if context == RingShortcutContext::ImageFullscreen =>
            {
                let dir = self.visual_spread_shift_dir(true);
                self.apply_ring_image_spread_shift(ctx, dir, source);
                None
            }
            RingActionId::ImageSpreadShiftPrev
                if context == RingShortcutContext::ImageFullscreen =>
            {
                self.apply_ring_image_spread_shift(ctx, -1, source);
                None
            }
            RingActionId::ImageSpreadShiftNext
                if context == RingShortcutContext::ImageFullscreen =>
            {
                self.apply_ring_image_spread_shift(ctx, 1, source);
                None
            }
            RingActionId::ImageRotateLeft if context == RingShortcutContext::ImageFullscreen => {
                if let Some(fs_idx) = self.fullscreen_idx {
                    if self.current_fullscreen_spread_is_double(fs_idx) {
                        self.show_feedback_toast("見開き表示中は回転できません".to_string());
                    } else {
                        self.rotate_image_ccw(fs_idx);
                    }
                }
                None
            }
            RingActionId::ImageRotateRight if context == RingShortcutContext::ImageFullscreen => {
                if let Some(fs_idx) = self.fullscreen_idx {
                    if self.current_fullscreen_spread_is_double(fs_idx) {
                        self.show_feedback_toast("見開き表示中は回転できません".to_string());
                    } else {
                        self.rotate_image_cw(fs_idx);
                    }
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
                if let Some(fs_idx) = self.fullscreen_idx {
                    if self.current_fullscreen_spread_is_double(fs_idx) {
                        self.show_feedback_toast(
                            "見開き表示中はメタデータ表示を切り替えできません".to_string(),
                        );
                    } else {
                        self.show_metadata_panel = !self.show_metadata_panel;
                        self.metadata_panel_hover_active = false;
                    }
                }
                None
            }
            RingActionId::ImageSlideshow if context == RingShortcutContext::ImageFullscreen => {
                self.toggle_ring_slideshow();
                None
            }
            RingActionId::ImageZoomMode if context == RingShortcutContext::ImageFullscreen => {
                if let Some(fs_idx) = self.fullscreen_idx {
                    self.toggle_fs_zoom_mode_action(ctx, fs_idx);
                }
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
            RingActionId::ImageCopyToClipboard
                if context == RingShortcutContext::ImageFullscreen =>
            {
                if let Some(fs_idx) = self.fullscreen_idx
                    && !self.copy_item_image_to_clipboard(fs_idx)
                {
                    self.show_feedback_toast("この項目は画像コピーに対応していません".to_string());
                }
                None
            }
            RingActionId::ImageOpenFolder if context == RingShortcutContext::ImageFullscreen => {
                if let Some(fs_idx) = self.fullscreen_idx
                    && !self.open_item_folder_in_explorer(fs_idx)
                {
                    self.show_feedback_toast("フォルダを開けません".to_string());
                }
                None
            }
            RingActionId::ImageCopyPath if context == RingShortcutContext::ImageFullscreen => {
                if let Some(fs_idx) = self.fullscreen_idx
                    && !self.copy_item_path_to_clipboard(ctx, fs_idx)
                {
                    self.show_feedback_toast("パスをコピーできません".to_string());
                }
                None
            }
            RingActionId::ImageCopyFileName if context == RingShortcutContext::ImageFullscreen => {
                if let Some(fs_idx) = self.fullscreen_idx
                    && !self.copy_item_file_name_to_clipboard(ctx, fs_idx)
                {
                    self.show_feedback_toast("ファイル名をコピーできません".to_string());
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
                    // 音楽ビュー (音声ファイル / 動画→音声モード) は loop target 再計算を伴う
                    // 音楽用 helper を通す。native helper だと music_bookmarks / loop_target が
                    // stale になりブックマークループ判定が崩れる (Codex P2)。
                    if self.fs_music_view_active(fs_idx) {
                        self.cycle_music_loop_mode(ctx, fs_idx);
                    } else {
                        self.cycle_native_video_loop_common(ctx, fs_idx);
                    }
                }
                None
            }
            RingActionId::VideoBookmark if context == RingShortcutContext::VideoFullscreen => {
                // 音楽ビューは music_bookmarks が正本。native bookmark 経路は音楽パネル /
                // タイムラインへ反映されないので音楽 helper に分岐する (Codex P2)。
                if let Some(fs_idx) = self.fullscreen_idx
                    && self.fs_music_view_active(fs_idx)
                {
                    self.add_music_bookmark_at_current(fs_idx);
                } else {
                    self.add_ring_video_bookmark(ctx);
                }
                None
            }
            RingActionId::VideoMarkerPrev if context == RingShortcutContext::VideoFullscreen => {
                if let Some(fs_idx) = self.fullscreen_idx {
                    if self.fs_music_view_active(fs_idx) {
                        self.music_marker_jump(fs_idx, false);
                    } else {
                        self.jump_ring_video_marker(fs_idx, false);
                    }
                }
                None
            }
            RingActionId::VideoMarkerNext if context == RingShortcutContext::VideoFullscreen => {
                if let Some(fs_idx) = self.fullscreen_idx {
                    if self.fs_music_view_active(fs_idx) {
                        self.music_marker_jump(fs_idx, true);
                    } else {
                        self.jump_ring_video_marker(fs_idx, true);
                    }
                }
                None
            }
            RingActionId::VideoTileMode if context == RingShortcutContext::VideoFullscreen => {
                // タイル表示は native presenter 前提。音楽ビュー (音声 / 動画→音声モード) では
                // presenter が無い / hidden なので無効化する (Codex P3)。
                if self
                    .fullscreen_idx
                    .is_some_and(|i| self.fs_music_view_active(i))
                {
                    self.show_feedback_toast("タイル表示は動画のみ対応です".to_string());
                } else {
                    self.toggle_ring_video_tile_mode(ctx);
                }
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

    fn apply_ring_grid_column_count(&mut self, cols: usize) {
        self.set_grid_view_mode(crate::settings::GridViewMode::Thumbnail);
        if cols != self.settings.grid_cols {
            self.settings.grid_cols = cols;
            self.settings.save();
        }
    }

    fn apply_ring_toggle_window_mode(&mut self, ctx: &egui::Context, context: RingShortcutContext) {
        match context {
            RingShortcutContext::ImageFullscreen => {
                #[cfg(windows)]
                {
                    if self.viewer_session_is_detached() {
                        self.toggle_detached_viewer_borderless_fullscreen(ctx);
                    } else {
                        self.toggle_still_window_mode();
                        ctx.request_repaint_of(egui::ViewportId::ROOT);
                    }
                }
                #[cfg(not(windows))]
                {
                    let _ = ctx;
                    self.show_feedback_toast("この操作は Windows でのみ利用できます".to_string());
                }
            }
            RingShortcutContext::VideoFullscreen => {
                self.toggle_video_window_mode_for_input(ctx);
            }
            RingShortcutContext::Grid => {}
        }
    }

    fn apply_ring_close_fullscreen(&mut self, ctx: &egui::Context, context: RingShortcutContext) {
        match context {
            RingShortcutContext::ImageFullscreen => {
                let detached = self.viewer_session_is_detached();
                self.handle_fullscreen_close_request();
                if !detached {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                }
                ctx.request_repaint();
            }
            RingShortcutContext::VideoFullscreen => {
                self.close_fullscreen();
                ctx.request_repaint();
            }
            RingShortcutContext::Grid => {}
        }
    }

    fn apply_ring_minimize_window(&mut self, ctx: &egui::Context, context: RingShortcutContext) {
        #[cfg(windows)]
        if let Some(hwnd) = self.ring_minimize_target_hwnd(context) {
            crate::logger::log(format!(
                "[input-nav] minimize_window context={context:?} hwnd=0x{hwnd:x}"
            ));
            let _ = crate::video::native_window::minimize_window(hwnd);
            ctx.request_repaint();
            return;
        }

        let target_viewport = match context {
            RingShortcutContext::Grid => egui::ViewportId::ROOT,
            RingShortcutContext::ImageFullscreen => self.fullscreen_ring_action_viewport_id(ctx),
            RingShortcutContext::VideoFullscreen => self.fullscreen_ring_action_viewport_id(ctx),
        };
        ctx.send_viewport_cmd_to(target_viewport, egui::ViewportCommand::Minimized(true));
        ctx.request_repaint();
    }

    #[cfg(windows)]
    fn ring_minimize_target_hwnd(&self, context: RingShortcutContext) -> Option<u64> {
        let live_main = || {
            self.main_hwnd.and_then(|hwnd| {
                let hwnd = hwnd as u64;
                crate::video::native_window::is_window_alive(hwnd).then_some(hwnd)
            })
        };
        match context {
            RingShortcutContext::Grid => live_main(),
            RingShortcutContext::ImageFullscreen | RingShortcutContext::VideoFullscreen => {
                if matches!(self.viewer_presentation, ViewerPresentation::DetachedWindow)
                    || self.active_detached_viewer_context.is_some()
                {
                    if let Some(hwnd) = self.detached_viewer_host_hwnd_alive() {
                        return Some(hwnd);
                    }
                }
                if matches!(self.viewer_presentation, ViewerPresentation::MainWindow)
                    || context == RingShortcutContext::VideoFullscreen
                {
                    return live_main();
                }
                None
            }
        }
    }

    fn fullscreen_ring_action_viewport_id(&self, ctx: &egui::Context) -> egui::ViewportId {
        if ctx.viewport_id() != egui::ViewportId::ROOT {
            return ctx.viewport_id();
        }
        self.fullscreen_viewport_id()
    }

    fn visual_spread_shift_dir(&self, right: bool) -> i32 {
        let rtl = self.spread_mode.is_rtl();
        match (right, rtl) {
            (true, false) | (false, true) => 1,
            (true, true) | (false, false) => -1,
        }
    }

    fn apply_ring_image_spread_shift(
        &mut self,
        ctx: &egui::Context,
        dir: i32,
        source: &'static str,
    ) {
        let Some(fs_idx) = self.fullscreen_idx else {
            return;
        };
        let mut action = crate::ui_fullscreen::FsKeyAction::default();
        self.queue_spread_shift_action(fs_idx, dir, &mut action);
        if action.nav_delta != 0 || action.jump_to.is_some() {
            self.bump_input_seq(source, Some(&format!("spread_shift_dir={dir}")));
            self.handle_fs_navigation(
                ctx,
                false,
                false,
                None,
                None,
                None,
                action.nav_delta,
                action.jump_to,
                fs_idx,
            );
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
            RingShortcutContext::VideoFullscreen => {
                // 音楽ビュー (音声ファイル / 動画→音声モード) は代表サムネのフレームピン対象外。
                // 音声パスに video pin メタデータを残さない / 音声モード中の held フレームを
                // 誤ってピンしない (Codex P3)。ピンしたいときは動画表示に戻ってから行う。
                if self
                    .fullscreen_idx
                    .is_some_and(|i| self.fs_music_view_active(i))
                {
                    self.show_feedback_toast("代表サムネのピン留めは動画のみ対応です".to_string());
                } else {
                    self.pin_ring_video_frame(ctx);
                }
            }
        }
    }

    fn toggle_selected_grid_check(&mut self) {
        let Some(idx) = self.selected else {
            return;
        };
        if self.checked.contains(&idx) {
            self.checked.remove(&idx);
        } else if self.grid_item_can_be_checked(idx) {
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
            self.slideshow_scroll_anim = None;
            self.slideshow_scroll_range_cache = None;
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

    fn handle_gamepad_select(&mut self, ctx: &egui::Context) -> Option<AddressBarNav> {
        let Some(fs_idx) = self.fullscreen_idx else {
            self.open_gamepad_location_picker(ctx);
            return None;
        };
        if self.current_fullscreen_is_video(fs_idx) {
            self.open_gamepad_video_marker_picker(ctx, fs_idx);
            return None;
        }
        // 見開きモード (キーボードのショートカット 1〜5) を巡回トグルする。
        // Single → Ltr → LtrCover → Rtl → RtlCover → Single … の順。
        // 連結方式 (reading_flow) の切替は key_6 / 別操作の役割であり、Select ではない。
        // 対応アイテム (画像/ZIP画像/PDFページ) のときだけ。動画/非対応アイテム上で
        // 切り替えると、見えないモードがフォルダに永続化されてナビや各モードキーが壊れる。
        if !self.vertical_reading_supported_idx(fs_idx) {
            return None;
        }
        let next = self.spread_mode.next_in_spread_cycle();
        self.apply_fullscreen_spread_mode(ctx, fs_idx, next);
        self.show_feedback_toast(format!("[Pad:{}]", next.label()));
        None
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
                self.handle_gamepad_still_y_direction(ctx, fs_idx, dir);
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

    fn handle_gamepad_still_y_direction(
        &mut self,
        ctx: &egui::Context,
        fs_idx: usize,
        dir: PadDir,
    ) {
        match dir {
            PadDir::Up => {
                self.handle_fullscreen_boundary_jump(ctx, fs_idx, false, "gamepad_y_home")
            }
            PadDir::Down => {
                self.handle_fullscreen_boundary_jump(ctx, fs_idx, true, "gamepad_y_end")
            }
            PadDir::Left | PadDir::Right => self.handle_gamepad_spread_nudge(ctx, fs_idx, dir),
        }
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
        let mut action = crate::ui_fullscreen::FsKeyAction::default();
        self.queue_spread_shift_action(fs_idx, nudge_dir, &mut action);
        if action.nav_delta != 0 || action.jump_to.is_some() {
            self.bump_input_seq(
                "gamepad_fs_nudge",
                Some(&format!("spread_shift_dir={nudge_dir}")),
            );
            self.handle_fs_navigation(
                ctx,
                false,
                false,
                None,
                None,
                None,
                action.nav_delta,
                action.jump_to,
                fs_idx,
            );
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

    fn handle_gamepad_start(&mut self, ctx: &egui::Context) -> Option<AddressBarNav> {
        if self.is_snapshot_active() {
            self.show_feedback_toast(
                "スナップショット中は他のフォルダに移動できません".to_string(),
            );
            return None;
        }
        if self.settings.favorites.is_empty() {
            self.show_feedback_toast("お気に入りが登録されていません".to_string());
            return None;
        }
        let selected = self.current_gamepad_favorite_index().unwrap_or(0);
        let mut picker = GamepadFavoritePickerState {
            selected,
            scroll_top: selected.saturating_sub(5),
        };
        update_favorite_picker_scroll(&mut picker, self.settings.favorites.len());
        self.ring_picker = None;
        self.gamepad_location_picker = None;
        self.gamepad_video_marker_picker = None;
        self.gamepad_favorite_picker = Some(picker);
        ctx.request_repaint();
        self.sync_native_video_favorite_picker_overlay(ctx);
        None
    }

    fn open_gamepad_video_marker_picker(&mut self, ctx: &egui::Context, fs_idx: usize) {
        let markers = self.collect_video_nav_markers(fs_idx);
        if markers.is_empty() {
            self.show_feedback_toast("ブックマーク / チャプターがありません".to_string());
            return;
        }
        let current = self
            .fs_video_player(fs_idx)
            .map(|player| player.position())
            .unwrap_or(0.0);
        let selected = markers
            .iter()
            .rposition(|marker| marker.pts <= current + 0.5)
            .unwrap_or(0);
        let mut picker = GamepadVideoMarkerPickerState {
            selected,
            scroll_top: selected.saturating_sub(5),
        };
        update_video_marker_picker_scroll(&mut picker, markers.len());
        self.ring_picker = None;
        self.gamepad_favorite_picker = None;
        self.gamepad_location_picker = None;
        self.gamepad_video_marker_picker = Some(picker);
        ctx.request_repaint();
        self.sync_native_video_marker_picker_overlay(ctx);
    }

    fn open_gamepad_location_picker(&mut self, ctx: &egui::Context) {
        if self.is_snapshot_active() {
            self.show_feedback_toast(
                "スナップショット中は他のフォルダに移動できません".to_string(),
            );
            return;
        }
        if self.global_search.active || self.favsearch.active || self.tag_view.active {
            self.show_feedback_toast("検索中は場所リストを開けません".to_string());
            return;
        }
        let rating_counts = self.rating_counts();
        let entries = self.build_gamepad_location_entries(rating_counts);
        if entries.is_empty() {
            self.show_feedback_toast("移動できる場所がありません".to_string());
            return;
        }
        let selected = self.current_gamepad_location_index(&entries).unwrap_or(0);
        let mut picker = GamepadLocationPickerState {
            selected,
            scroll_top: selected.saturating_sub(5),
            entries,
        };
        update_location_picker_scroll(&mut picker);
        self.ring_picker = None;
        self.gamepad_favorite_picker = None;
        self.gamepad_video_marker_picker = None;
        self.gamepad_location_picker = Some(picker);
        ctx.request_repaint();
    }

    pub(crate) fn build_gamepad_location_entries(
        &self,
        rating_counts: Option<[usize; 6]>,
    ) -> Vec<GamepadLocationEntry> {
        let book_root = self.book_root_path();
        let mut entries = vec![
            GamepadLocationEntry {
                label: "ドライブ一覧".to_string(),
                value: "接続ドライブ".to_string(),
                nav: GamepadLocationNav::DriveList,
            },
            GamepadLocationEntry {
                label: "読書履歴".to_string(),
                value: "最近読んだ本".to_string(),
                nav: GamepadLocationNav::ReadingHistory,
            },
        ];

        for stars in 1..=5u8 {
            let value = rating_counts
                .map(|counts| format!("{} 件", counts[stars as usize]))
                .unwrap_or_else(|| "レーティング一覧".to_string());
            entries.push(GamepadLocationEntry {
                label: format!("★{stars} レーティング一覧"),
                value,
                nav: GamepadLocationNav::RatingView(stars),
            });
        }

        entries.push(GamepadLocationEntry {
            label: "本棚フォルダ".to_string(),
            value: book_root.display().to_string(),
            nav: GamepadLocationNav::BooksRoot,
        });

        for location in crate::known_folders::quick_locations() {
            let value = location.path.display().to_string();
            entries.push(GamepadLocationEntry {
                label: location.label.to_string(),
                value,
                nav: GamepadLocationNav::Direct(location.path),
            });
        }
        for drive in crate::known_folders::available_drives() {
            let label = drive.display().to_string();
            entries.push(GamepadLocationEntry {
                label: label.clone(),
                value: "ドライブ".to_string(),
                nav: GamepadLocationNav::Direct(drive),
            });
        }
        entries
    }

    pub(crate) fn current_gamepad_location_index(
        &self,
        entries: &[GamepadLocationEntry],
    ) -> Option<usize> {
        let effective_folder = self.effective_folder();
        let book_root = self.book_root_path();
        entries.iter().position(|entry| match &entry.nav {
            GamepadLocationNav::DriveList => self.items_are_drive_list,
            GamepadLocationNav::ReadingHistory => self.items_are_reading_history_view,
            GamepadLocationNav::RatingView(stars) => {
                self.items_are_rating_view && self.rating_view_stars == *stars
            }
            GamepadLocationNav::BooksRoot => effective_folder
                .as_ref()
                .is_some_and(|current| crate::folder_tree::path_eq(current, &book_root)),
            GamepadLocationNav::Direct(path) => effective_folder
                .as_ref()
                .is_some_and(|current| crate::folder_tree::path_eq(current, path)),
        })
    }

    fn current_gamepad_favorite_index(&self) -> Option<usize> {
        let current_favorite_id = self.effective_folder().and_then(|path| {
            self.find_nearest_favorite(&path)
                .map(|favorite| favorite.id)
        });
        current_favorite_id.and_then(|id| {
            self.settings
                .favorites
                .iter()
                .position(|favorite| favorite.id == id)
        })
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
        if self.block_gamepad_grid_folder_nav_for_detached_foreground(
            "gamepad_grid_folder_nav",
            if forward { "forward" } else { "backward" },
        ) {
            return;
        }
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
        } else if self.items_are_subfolder_expansion_view {
            self.cancel_pending_folder_nav();
        } else if self.zip_nav_handle_ctrl_updown(forward) {
            // ネスト ZIP 内: ツリーを DFS で前後のノードへ (#4 改)。端では false → 下で ZIP を抜ける。
        } else if let Some(cur) = self.effective_folder() {
            self.start_folder_nav(cur, forward, FolderNavMode::Grid);
        }
    }

    fn handle_gamepad_grid_accept(&mut self, ctx: &egui::Context) -> Option<AddressBarNav> {
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
        if !self.guard_reading_history_open(idx) {
            return None;
        }
        // 読書履歴ビューから本を開く場合は、閉じたときに読書履歴へ戻れるよう予約する。
        self.note_reading_history_open(idx);
        // ファイル名スタックの集約グリッドでメディアセルを開いたら、フラット読書フルスクリーンへ
        // (スタック/単独画像/動画を直接開く)。コンテナは false で通常ナビへ流れる。
        if self.stack_try_open_from_grid(idx, false) {
            return None;
        }
        #[cfg(windows)]
        if self.open_grid_container_in_detached_book_context(ctx, idx) {
            return None;
        }
        let item = self.items.get(idx).cloned();
        match item {
            Some(GridItem::Folder(p)) | Some(GridItem::ZipFile(p)) | Some(GridItem::PdfFile(p)) => {
                if self.should_auto_fullscreen_grid_container(idx) {
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
            | Some(GridItem::Video(_))
            | Some(GridItem::Audio(_)) => {
                self.bump_input_seq_for_item("gamepad_grid_open", idx);
                self.fs_open_intent_from_grid = true;
                self.open_fullscreen(idx);
                None
            }
            Some(GridItem::ConvertibleArchive { path, format }) => {
                let auto_fs = self.settings.effective_auto_fullscreen_zip_pdf();
                self.maybe_suppress_rating_filter_for_opened_container(idx);
                self.maybe_suppress_facet_filter_for_opened_container(idx);
                if self.settings.archive_file_handling_ignores_convertible() {
                    self.show_feedback_toast(
                        "設定により RAR / 7z / LZH アーカイブを無視しています".into(),
                    );
                } else if let Some(cached) = self.try_archive_cache_lookup(&path) {
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
            Some(GridItem::ZipDir {
                zip_path,
                dir_prefix,
                ..
            }) if self.items_are_rating_view => {
                self.open_rating_view_zipdir(zip_path, dir_prefix);
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
            // ファイル名スタック (v2.0.0): 集約グリッドのセルは上の stack_try_open_from_grid で
            // 既に処理済み (フラットフルスクリーンへ)。ここに来るのは非スタックモードのみで
            // Stack セルは存在しないが、網羅性のため no-op を置く。
            Some(GridItem::Stack { .. }) => None,
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

    /// リング / 右ドラッグの context を `VideoFullscreen` にすべき面か。
    /// 動画そのもの (`current_fullscreen_is_video`) に加えて、**音楽ビュー**
    /// (音声ファイル or 動画→音声モード、`fs_music_view_active`) も動画（メディア）リング
    /// = 再生/一時停止・シーク・マーカー・ミュート・ループ を使う。音楽ビューで画像リング
    /// (回転/コピー/ズーム 等) が出ても意味を成さないため (実機 FB 2026-07)。
    fn fullscreen_uses_video_ring_context(&self, fs_idx: usize) -> bool {
        self.current_fullscreen_is_video(fs_idx) || self.fs_music_view_active(fs_idx)
    }

    fn jump_ring_video_marker(&mut self, fs_idx: usize, next: bool) {
        #[cfg(windows)]
        self.jump_native_video_marker(fs_idx, next);
        #[cfg(not(windows))]
        let _ = (fs_idx, next);
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
            scan_code: 0,
            extended: false,
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

fn ring_direction_from_dpad_buttons(state: &GamepadInputState) -> Option<RingDirection> {
    let up = state.button_down(PadButton::DPadUp);
    let down = state.button_down(PadButton::DPadDown);
    let left = state.button_down(PadButton::DPadLeft);
    let right = state.button_down(PadButton::DPadRight);
    let vertical = match (up, down) {
        (true, false) => Some(PadDir::Up),
        (false, true) => Some(PadDir::Down),
        _ => None,
    };
    let horizontal = match (left, right) {
        (true, false) => Some(PadDir::Left),
        (false, true) => Some(PadDir::Right),
        _ => None,
    };
    match (vertical, horizontal) {
        (Some(PadDir::Up), Some(PadDir::Right)) => Some(RingDirection::UpRight),
        (Some(PadDir::Down), Some(PadDir::Right)) => Some(RingDirection::DownRight),
        (Some(PadDir::Down), Some(PadDir::Left)) => Some(RingDirection::DownLeft),
        (Some(PadDir::Up), Some(PadDir::Left)) => Some(RingDirection::UpLeft),
        (Some(dir), None) | (None, Some(dir)) => Some(ring_direction_from_pad_dir(dir)),
        _ => None,
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

fn ring_guide_radius_for_rect(rect: egui::Rect) -> f32 {
    (rect.width().min(rect.height()) * 0.20).clamp(144.0, 164.0)
}

fn draw_ring_guide_donut<'a>(
    painter: &egui::Painter,
    center: egui::Pos2,
    radius: f32,
    selected: Option<RingDirection>,
    action_label: impl Fn(RingDirection) -> &'a str,
) {
    let inner_radius = MOUSE_FLICK_NEUTRAL_RADIUS_PX.max(62.0);
    let outer_radius = radius + 46.0;
    let label_radius = (inner_radius + outer_radius) * 0.5;

    for &direction in RingDirection::all() {
        let is_selected = selected == Some(direction);
        let fill = if is_selected {
            egui::Color32::from_rgba_unmultiplied(72, 126, 190, 218)
        } else {
            egui::Color32::from_rgba_unmultiplied(0, 0, 0, 118)
        };
        let stroke = if is_selected {
            egui::Stroke::new(2.0, egui::Color32::WHITE)
        } else {
            egui::Stroke::new(1.0, egui::Color32::from_white_alpha(120))
        };
        draw_annular_segment(
            painter,
            center,
            inner_radius,
            outer_radius,
            direction,
            fill,
            stroke,
        );

        let label_center = center + ring_direction_unit(direction) * label_radius;
        let text = truncate_ring_overlay_label(action_label(direction), 11);
        draw_ring_segment_label(painter, label_center, &text, is_selected);
    }
}

fn draw_annular_segment(
    painter: &egui::Painter,
    center: egui::Pos2,
    inner_radius: f32,
    outer_radius: f32,
    direction: RingDirection,
    fill: egui::Color32,
    stroke: egui::Stroke,
) {
    let mid = ring_direction_angle_rad(direction);
    let half = std::f32::consts::PI / 8.0;
    let start = mid - half;
    let end = mid + half;
    let steps = 8;
    let mut outer_points = Vec::with_capacity(steps + 1);
    let mut inner_points = Vec::with_capacity(steps + 1);
    for i in 0..=steps {
        let t = start + (end - start) * i as f32 / steps as f32;
        outer_points.push(ring_point(center, outer_radius, t));
        inner_points.push(ring_point(center, inner_radius, t));
    }
    for i in 0..steps {
        painter.add(egui::Shape::convex_polygon(
            vec![
                outer_points[i],
                outer_points[i + 1],
                inner_points[i + 1],
                inner_points[i],
            ],
            fill,
            egui::Stroke::NONE,
        ));
    }
    painter.add(egui::Shape::line(outer_points.clone(), stroke));
    painter.add(egui::Shape::line(inner_points.clone(), stroke));
    painter.line_segment([outer_points[0], inner_points[0]], stroke);
    painter.line_segment(
        [*outer_points.last().unwrap(), *inner_points.last().unwrap()],
        stroke,
    );
}

fn draw_ring_segment_label(
    painter: &egui::Painter,
    center: egui::Pos2,
    text: &str,
    selected: bool,
) {
    let font = egui::FontId::proportional(if selected { 14.5 } else { 13.5 });
    let text_color = egui::Color32::WHITE;
    let galley = painter.layout_no_wrap(text.to_string(), font, text_color);
    let padding = egui::vec2(8.0, 4.0);
    let size = galley.size() + padding * 2.0;
    let rect = egui::Rect::from_center_size(center, size);
    let fill = if selected {
        egui::Color32::from_rgba_unmultiplied(22, 44, 72, 232)
    } else {
        egui::Color32::from_rgba_unmultiplied(0, 0, 0, 214)
    };
    painter.rect_filled(rect, 4.0, fill);
    painter.rect_stroke(
        rect,
        4.0,
        egui::Stroke::new(
            if selected { 1.4 } else { 1.0 },
            egui::Color32::from_white_alpha(if selected { 210 } else { 150 }),
        ),
        egui::StrokeKind::Outside,
    );
    painter.galley(rect.min + padding, galley, text_color);
}

fn ring_direction_angle_rad(direction: RingDirection) -> f32 {
    match direction {
        RingDirection::Up => -std::f32::consts::FRAC_PI_2,
        RingDirection::UpRight => -std::f32::consts::FRAC_PI_4,
        RingDirection::Right => 0.0,
        RingDirection::DownRight => std::f32::consts::FRAC_PI_4,
        RingDirection::Down => std::f32::consts::FRAC_PI_2,
        RingDirection::DownLeft => std::f32::consts::FRAC_PI_4 * 3.0,
        RingDirection::Left => std::f32::consts::PI,
        RingDirection::UpLeft => -std::f32::consts::FRAC_PI_4 * 3.0,
    }
}

fn ring_point(center: egui::Pos2, radius: f32, angle: f32) -> egui::Pos2 {
    center + egui::vec2(angle.cos() * radius, angle.sin() * radius)
}

fn truncate_ring_overlay_label(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let keep = max_chars.saturating_sub(3).max(1);
    let mut out: String = text.chars().take(keep).collect();
    out.push_str("...");
    out
}

fn draw_picker_scrollbar(
    painter: &egui::Painter,
    rows_rect: egui::Rect,
    total_rows: usize,
    visible_rows: usize,
    scroll_top: usize,
) {
    if total_rows <= visible_rows || visible_rows == 0 {
        return;
    }
    let track = egui::Rect::from_min_max(
        egui::pos2(rows_rect.max.x - 8.0, rows_rect.min.y + 4.0),
        egui::pos2(rows_rect.max.x - 3.0, rows_rect.max.y - 4.0),
    );
    if track.height() <= 4.0 {
        return;
    }
    painter.rect_filled(track, 2.5, egui::Color32::from_white_alpha(48));
    let ratio = visible_rows as f32 / total_rows as f32;
    let thumb_h = (track.height() * ratio).clamp(18.0, track.height());
    let max_scroll = total_rows.saturating_sub(visible_rows).max(1) as f32;
    let t = (scroll_top as f32 / max_scroll).clamp(0.0, 1.0);
    let thumb_top = track.min.y + (track.height() - thumb_h) * t;
    let thumb = egui::Rect::from_min_max(
        egui::pos2(track.min.x, thumb_top),
        egui::pos2(track.max.x, thumb_top + thumb_h),
    );
    painter.rect_filled(thumb, 2.5, egui::Color32::from_white_alpha(180));
}

fn video_marker_kind_label(kind: crate::ui_fullscreen::NavMarkerKind) -> &'static str {
    match kind {
        crate::ui_fullscreen::NavMarkerKind::Chapter => "チャプター",
        crate::ui_fullscreen::NavMarkerKind::Bookmark => "ブックマーク",
        crate::ui_fullscreen::NavMarkerKind::Pin => "ピン",
    }
}

fn video_marker_primary_label(marker: &crate::ui_fullscreen::NavMarker) -> String {
    format!(
        "{} {}",
        crate::ui_helpers::format_hms(marker.pts),
        video_marker_kind_label(marker.kind)
    )
}

fn video_marker_secondary_label(marker: &crate::ui_fullscreen::NavMarker) -> String {
    marker
        .title
        .as_deref()
        .filter(|title| !title.is_empty())
        .unwrap_or("")
        .to_string()
}

fn video_marker_seek_toast(marker: &crate::ui_fullscreen::NavMarker) -> String {
    let title = video_marker_secondary_label(marker);
    if title.is_empty() {
        video_marker_primary_label(marker)
    } else {
        format!("{}: {}", video_marker_primary_label(marker), title)
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

fn ring_guide_heading_detail(
    slots: &[RingActionId],
    context: RingShortcutContext,
    selected: Option<RingDirection>,
    fallback_heading: &str,
    fallback_detail: &str,
) -> (String, String) {
    if let Some(direction) = selected {
        let action = slots
            .get(direction.slot_index())
            .cloned()
            .unwrap_or_default();
        (
            ring_direction_short_label(direction).to_string(),
            action.label_for_context(context).to_string(),
        )
    } else {
        (fallback_heading.to_string(), fallback_detail.to_string())
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
    if stick.length_sq() < RING_STICK_COMMIT_THRESHOLD * RING_STICK_COMMIT_THRESHOLD {
        return None;
    }
    let degrees = stick.y.atan2(stick.x).to_degrees();
    Some(ring_direction_from_angle_degrees(degrees))
}

fn ring_direction_from_stick_with_hysteresis(
    stick: egui::Vec2,
    previous: Option<RingDirection>,
) -> Option<RingDirection> {
    if stick.length_sq() < RING_STICK_COMMIT_THRESHOLD * RING_STICK_COMMIT_THRESHOLD {
        return None;
    }
    let degrees = stick.y.atan2(stick.x).to_degrees();
    if let Some(previous) = previous {
        let sticky_width = 22.5 + RING_STICK_HYSTERESIS_DEGREES;
        if angular_distance_deg(degrees, ring_direction_input_angle_deg(previous)) <= sticky_width {
            return Some(previous);
        }
    }
    Some(ring_direction_from_angle_degrees(degrees))
}

fn ring_direction_from_angle_degrees(degrees: f32) -> RingDirection {
    if (-22.5..22.5).contains(&degrees) {
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
    }
}

fn ring_direction_input_angle_deg(direction: RingDirection) -> f32 {
    match direction {
        RingDirection::Right => 0.0,
        RingDirection::UpRight => 45.0,
        RingDirection::Up => 90.0,
        RingDirection::UpLeft => 135.0,
        RingDirection::Left => 180.0,
        RingDirection::DownLeft => -135.0,
        RingDirection::Down => -90.0,
        RingDirection::DownRight => -45.0,
    }
}

fn angular_distance_deg(a: f32, b: f32) -> f32 {
    ((a - b + 180.0).rem_euclid(360.0) - 180.0).abs()
}

fn mouse_flick_direction(flick: &MouseFlickState) -> Option<RingDirection> {
    let delta = flick.current_pos - flick.start_pos;
    if delta.length() < MOUSE_FLICK_NEUTRAL_RADIUS_PX {
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

fn update_favorite_picker_scroll(picker: &mut GamepadFavoritePickerState, len: usize) {
    if len == 0 {
        picker.selected = 0;
        picker.scroll_top = 0;
        return;
    }
    let visible = len.min(GAMEPAD_LIST_VISIBLE_ROWS).max(1);
    picker.selected = picker.selected.min(len - 1);
    if picker.selected < picker.scroll_top {
        picker.scroll_top = picker.selected;
    } else if picker.selected >= picker.scroll_top + visible {
        picker.scroll_top = picker.selected + 1 - visible;
    }
    picker.scroll_top = picker.scroll_top.min(len.saturating_sub(visible));
}

fn update_video_marker_picker_scroll(picker: &mut GamepadVideoMarkerPickerState, len: usize) {
    if len == 0 {
        picker.selected = 0;
        picker.scroll_top = 0;
        return;
    }
    let visible = len.min(GAMEPAD_LIST_VISIBLE_ROWS).max(1);
    picker.selected = picker.selected.min(len - 1);
    if picker.selected < picker.scroll_top {
        picker.scroll_top = picker.selected;
    } else if picker.selected >= picker.scroll_top + visible {
        picker.scroll_top = picker.selected + 1 - visible;
    }
    picker.scroll_top = picker.scroll_top.min(len.saturating_sub(visible));
}

fn update_location_picker_scroll(picker: &mut GamepadLocationPickerState) {
    let len = picker.entries.len();
    if len == 0 {
        picker.selected = 0;
        picker.scroll_top = 0;
        return;
    }
    let visible = len.min(GAMEPAD_LIST_VISIBLE_ROWS).max(1);
    picker.selected = picker.selected.min(len - 1);
    if picker.selected < picker.scroll_top {
        picker.scroll_top = picker.selected;
    } else if picker.selected >= picker.scroll_top + visible {
        picker.scroll_top = picker.selected + 1 - visible;
    }
    picker.scroll_top = picker.scroll_top.min(len.saturating_sub(visible));
}

fn cycle_rating(current: u8, delta: i32) -> u8 {
    cycle_index(6, current.min(5) as usize, delta) as u8
}

fn video_continuous_mode_values() -> &'static [VideoContinuousMode] {
    &[
        VideoContinuousMode::Off,
        VideoContinuousMode::Continuous,
        VideoContinuousMode::ContinuousLoop,
    ]
}

fn picker_row_supports_value_list(row: RingPickerRowId) -> bool {
    matches!(
        row,
        RingPickerRowId::ItemRating
            | RingPickerRowId::ContainerRating
            | RingPickerRowId::SpreadMode
            | RingPickerRowId::ReadingFlow
            | RingPickerRowId::ReadingDirection
            | RingPickerRowId::FitMode
            | RingPickerRowId::UpscaleModel
            | RingPickerRowId::VideoPlaybackSpeed
            | RingPickerRowId::VideoContinuousMode
    )
}

fn picker_list_len_for_state(
    picker: &RingPickerState,
    list: PickerListState,
    ai_mode: crate::settings::AiFeatureMode,
) -> usize {
    match list.mode {
        PickerListMode::PostFilterGroup => POST_FILTER_GROUPS.len(),
        PickerListMode::PostFilterItem { group } => POST_FILTER_GROUPS
            .get(group)
            .map(|g| g.filters.len())
            .unwrap_or(0),
        PickerListMode::RowValues(row) => picker_row_value_len(picker, row, ai_mode),
    }
}

fn picker_row_value_len(
    picker: &RingPickerState,
    row: RingPickerRowId,
    ai_mode: crate::settings::AiFeatureMode,
) -> usize {
    match row {
        RingPickerRowId::ItemRating | RingPickerRowId::ContainerRating => 6,
        RingPickerRowId::SpreadMode => SpreadMode::all().len(),
        RingPickerRowId::ReadingFlow => ReadingFlow::all().len(),
        RingPickerRowId::ReadingDirection => 2,
        RingPickerRowId::FitMode => {
            FullscreenFitMode::selectable_for_flow(picker.reading_flow).len()
        }
        RingPickerRowId::UpscaleModel => {
            crate::adjustment::upscale_menu_items_for_mode(ai_mode).len()
        }
        RingPickerRowId::VideoPlaybackSpeed => crate::video::clock::PLAYBACK_SPEED_CHOICES.len(),
        RingPickerRowId::VideoContinuousMode => video_continuous_mode_values().len(),
        _ => 0,
    }
}

fn picker_row_value_labels(
    picker: &RingPickerState,
    row: RingPickerRowId,
    ai_mode: crate::settings::AiFeatureMode,
) -> Vec<String> {
    match row {
        RingPickerRowId::ItemRating | RingPickerRowId::ContainerRating => {
            (0..=5).map(rating_label).collect()
        }
        RingPickerRowId::SpreadMode => SpreadMode::all()
            .iter()
            .map(|mode| mode.label().to_string())
            .collect(),
        RingPickerRowId::ReadingFlow => ReadingFlow::all()
            .iter()
            .map(|flow| flow.label().to_string())
            .collect(),
        RingPickerRowId::ReadingDirection => [ReadingDirection::Ltr, ReadingDirection::Rtl]
            .iter()
            .map(|direction| direction.label().to_string())
            .collect(),
        RingPickerRowId::FitMode => FullscreenFitMode::selectable_for_flow(picker.reading_flow)
            .iter()
            .map(|mode| mode.label().to_string())
            .collect(),
        RingPickerRowId::UpscaleModel => crate::adjustment::upscale_menu_items_for_mode(ai_mode)
            .into_iter()
            .map(|(label, _)| label.to_string())
            .collect(),
        RingPickerRowId::VideoPlaybackSpeed => crate::video::clock::PLAYBACK_SPEED_CHOICES
            .iter()
            .map(|&speed| crate::video::clock::format_playback_speed(speed))
            .collect(),
        RingPickerRowId::VideoContinuousMode => video_continuous_mode_values()
            .iter()
            .map(|mode| mode.label().to_string())
            .collect(),
        _ => Vec::new(),
    }
}

fn picker_row_value_index(
    picker: &RingPickerState,
    row: RingPickerRowId,
    ai_mode: crate::settings::AiFeatureMode,
) -> usize {
    match row {
        RingPickerRowId::ItemRating => picker.item_rating.min(5) as usize,
        RingPickerRowId::ContainerRating => picker.container_rating.min(5) as usize,
        RingPickerRowId::SpreadMode => SpreadMode::all()
            .iter()
            .position(|&mode| mode == picker.spread_mode)
            .unwrap_or(0),
        RingPickerRowId::ReadingFlow => ReadingFlow::all()
            .iter()
            .position(|&flow| flow == picker.reading_flow)
            .unwrap_or(0),
        RingPickerRowId::ReadingDirection => [ReadingDirection::Ltr, ReadingDirection::Rtl]
            .iter()
            .position(|&direction| direction == picker.reading_direction)
            .unwrap_or(0),
        RingPickerRowId::FitMode => {
            let values = FullscreenFitMode::selectable_for_flow(picker.reading_flow);
            let current = picker.fit_mode.effective_for_flow(picker.reading_flow);
            values.iter().position(|&mode| mode == current).unwrap_or(0)
        }
        RingPickerRowId::UpscaleModel => crate::adjustment::upscale_menu_items_for_mode(ai_mode)
            .iter()
            .position(|(_, key)| *key == picker.upscale_model_key.as_deref())
            .unwrap_or(0),
        RingPickerRowId::VideoPlaybackSpeed => crate::video::clock::PLAYBACK_SPEED_CHOICES
            .iter()
            .enumerate()
            .min_by(|(_, a), (_, b)| {
                (*a - picker.video_playback_speed)
                    .abs()
                    .partial_cmp(&(*b - picker.video_playback_speed).abs())
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .map(|(idx, _)| idx)
            .unwrap_or(0),
        RingPickerRowId::VideoContinuousMode => video_continuous_mode_values()
            .iter()
            .position(|&mode| mode == picker.video_continuous_mode)
            .unwrap_or(0),
        _ => 0,
    }
}

fn post_filter_group_index(filter: PostFilter) -> usize {
    POST_FILTER_GROUPS
        .iter()
        .position(|group| group.filters.contains(&filter))
        .unwrap_or(0)
}

fn post_filter_item_index_in_group(filter: PostFilter, group: usize) -> usize {
    POST_FILTER_GROUPS
        .get(group)
        .and_then(|group| group.filters.iter().position(|&f| f == filter))
        .unwrap_or(0)
}

fn rating_label(stars: u8) -> String {
    let stars = stars.min(5) as usize;
    format!("{}{}", "★".repeat(stars), "☆".repeat(5 - stars))
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

#[cfg(test)]
mod tests {
    use super::{
        POST_FILTER_GROUPS, PadDir, continuous_reading_stick_axis, cycle_rating,
        cycle_video_playback_speed, gamepad_grid_nav_target_pos, mouse_flick_direction,
        picker_rows_for_context, post_filter_group_index, post_filter_item_index_in_group,
        rating_label, ring_direction_from_dpad_buttons, ring_direction_from_stick,
        ring_direction_from_stick_with_hysteresis,
    };
    use crate::adjustment::PostFilter;
    use crate::gamepad::{GamepadInputState, PadButton};
    use crate::ring_shortcut::RingShortcutContext;
    use crate::ring_shortcut::{
        MOUSE_FLICK_MOVE_THRESHOLD_PX, MOUSE_FLICK_NEUTRAL_RADIUS_PX, MouseFlickState,
        RingDirection,
    };
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
        assert_eq!(ring_direction_from_stick(egui::vec2(0.49, 0.0)), None);
        assert_eq!(
            ring_direction_from_stick(egui::vec2(0.50, 0.0)),
            Some(RingDirection::Right)
        );
    }

    #[test]
    fn ring_stick_hysteresis_keeps_previous_sector_near_boundary() {
        assert_eq!(
            ring_direction_from_stick_with_hysteresis(
                egui::vec2(25.0_f32.to_radians().cos(), 25.0_f32.to_radians().sin()),
                Some(RingDirection::Right)
            ),
            Some(RingDirection::Right)
        );
        assert_eq!(
            ring_direction_from_stick_with_hysteresis(
                egui::vec2(32.0_f32.to_radians().cos(), 32.0_f32.to_radians().sin()),
                Some(RingDirection::Right)
            ),
            Some(RingDirection::UpRight)
        );
    }

    #[test]
    fn mouse_flick_converts_screen_y_to_ring_direction() {
        let mut flick = MouseFlickState::new(
            RingShortcutContext::Grid,
            std::time::Instant::now(),
            egui::pos2(100.0, 100.0),
        );
        flick.current_pos = egui::pos2(140.0, 60.0);
        assert_eq!(mouse_flick_direction(&flick), Some(RingDirection::UpRight));

        flick.current_pos = egui::pos2(140.0, 140.0);
        assert_eq!(
            mouse_flick_direction(&flick),
            Some(RingDirection::DownRight)
        );

        flick.current_pos = egui::pos2(110.0, 110.0);
        assert_eq!(mouse_flick_direction(&flick), None);
    }

    #[test]
    fn mouse_flick_keeps_center_neutral_after_move_threshold() {
        let mut flick = MouseFlickState::new(
            RingShortcutContext::ImageFullscreen,
            std::time::Instant::now(),
            egui::pos2(100.0, 100.0),
        );
        flick.current_pos = egui::pos2(100.0 + MOUSE_FLICK_MOVE_THRESHOLD_PX + 8.0, 100.0);
        assert_eq!(mouse_flick_direction(&flick), None);

        flick.current_pos = egui::pos2(100.0 + MOUSE_FLICK_NEUTRAL_RADIUS_PX + 1.0, 100.0);
        assert_eq!(mouse_flick_direction(&flick), Some(RingDirection::Right));
    }

    #[test]
    fn picker_rows_are_context_specific() {
        assert_eq!(picker_rows_for_context(RingShortcutContext::Grid).len(), 5);
        assert_eq!(
            picker_rows_for_context(RingShortcutContext::ImageFullscreen).len(),
            8
        );
        assert!(
            picker_rows_for_context(RingShortcutContext::ImageFullscreen)
                .contains(&crate::ring_shortcut::RingPickerRowId::ReadingDirection)
        );
        assert_eq!(
            picker_rows_for_context(RingShortcutContext::VideoFullscreen).len(),
            5
        );
        assert!(
            picker_rows_for_context(RingShortcutContext::VideoFullscreen)
                .contains(&crate::ring_shortcut::RingPickerRowId::ItemRating)
        );
        assert!(
            picker_rows_for_context(RingShortcutContext::VideoFullscreen)
                .contains(&crate::ring_shortcut::RingPickerRowId::ContainerRating)
        );
    }

    #[test]
    fn rating_picker_wraps_clear_and_five_stars() {
        assert_eq!(cycle_rating(0, -1), 5);
        assert_eq!(cycle_rating(5, 1), 0);
        assert_eq!(cycle_rating(2, 1), 3);
    }

    #[test]
    fn rating_label_always_uses_five_stars() {
        assert_eq!(rating_label(0), "☆☆☆☆☆");
        assert_eq!(rating_label(2), "★★☆☆☆");
        assert_eq!(rating_label(7), "★★★★★");
    }

    #[test]
    fn dpad_ring_direction_combines_cardinals() {
        let now = std::time::Instant::now();
        let mut state = GamepadInputState::default();
        state.set_button_down(PadButton::DPadUp, true, now);
        state.set_button_down(PadButton::DPadRight, true, now);
        assert_eq!(
            ring_direction_from_dpad_buttons(&state),
            Some(RingDirection::UpRight)
        );

        state.set_button_down(PadButton::DPadRight, false, now);
        assert_eq!(
            ring_direction_from_dpad_buttons(&state),
            Some(RingDirection::Up)
        );

        state.set_button_down(PadButton::DPadDown, true, now);
        assert_eq!(ring_direction_from_dpad_buttons(&state), None);
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

            let group = post_filter_group_index(filter);
            let item = post_filter_item_index_in_group(filter, group);
            assert_eq!(POST_FILTER_GROUPS[group].filters[item], filter);
        }
    }
}
