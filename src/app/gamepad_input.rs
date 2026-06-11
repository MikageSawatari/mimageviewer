use std::path::PathBuf;
use std::time::{Duration, Instant};

use eframe::egui;

use super::{AdjustSpreadTarget, App, FolderNavMode};
use crate::folder_pane::{FolderPaneCommand, FolderPaneTreeKey};
use crate::gamepad::{GamepadInputState, PadAxis, PadButton, PadEvent};
use crate::grid_item::GridItem;
use crate::settings::{ReadingDirection, ReadingFlow};
use crate::ui_main::AddressBarNav;

const BUTTON_REPEAT_INTERVAL: Duration = Duration::from_millis(95);
const SHOULDER_REPEAT_INTERVAL: Duration = Duration::from_millis(260);
const STICK_STEP_INTERVAL: Duration = Duration::from_millis(110);
const TRIGGER_STEP_INTERVAL: Duration = Duration::from_millis(150);
const DEADZONE: f32 = 0.25;
const TRIGGER_THRESHOLD: f32 = 0.35;
const PAN_SPEED_PX_PER_SEC: f32 = 720.0;
const RIGHT_STICK_ZOOM_MULTIPLIER: f32 = 2.0;
const GAMEPAD_REPAINT_INTERVAL: Duration = Duration::from_millis(16);

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
        match action.kind {
            PadActionKind::Release if action.button == PadButton::North => {
                if !self.gamepad_state.y_modifier_used() {
                    self.handle_gamepad_y_tap(ctx);
                }
                None
            }
            PadActionKind::Release => None,
            PadActionKind::Press | PadActionKind::Repeat => {
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
                    PadButton::West if action.kind == PadActionKind::Press => {
                        self.handle_gamepad_x(ctx);
                        None
                    }
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
                self.handle_fs_navigation(ctx, true, false, None, None, 0, None, fs_idx);
            }
            return None;
        }
        self.handle_gamepad_grid_back()
    }

    fn handle_gamepad_x(&mut self, _ctx: &egui::Context) {
        let Some(fs_idx) = self.fullscreen_idx else {
            return;
        };
        if self.current_fullscreen_is_video(fs_idx) {
            return;
        }
        let is_spread_double = matches!(
            self.resolve_spread_pair(fs_idx),
            crate::ui_fullscreen::SpreadPair::Double { .. }
        );
        if !is_spread_double {
            self.show_metadata_panel = !self.show_metadata_panel;
            self.metadata_panel_hover_active = false;
        }
    }

    fn handle_gamepad_select(&mut self, ctx: &egui::Context) {
        let Some(fs_idx) = self.fullscreen_idx else {
            return;
        };
        // キーボード key_6 と同条件: 連続読み対応アイテム (画像/ZIP画像/PDFページ) のときだけ
        // 連結方式を切り替える。動画/非対応アイテム上で切り替えると、見えない flow が
        // フォルダに永続化され、ナビや各モードキーが壊れる。supported 判定が動画も除外する。
        if !self.vertical_reading_supported_idx(fs_idx) {
            return;
        }
        let next = self.reading_flow.next();
        self.set_reading_flow_for_fullscreen(ctx, fs_idx, next);
        self.show_feedback_toast(format!("[Pad:{}]", next.label()));
    }

    fn handle_gamepad_y_tap(&mut self, ctx: &egui::Context) {
        let Some(fs_idx) = self.fullscreen_idx else {
            if self.folder_pane_disabled() {
                self.show_feedback_toast(
                    "スナップショット中は他のフォルダに移動できません".to_string(),
                );
                return;
            }
            self.set_folder_tree_pane_visible(true);
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
        self.handle_fs_navigation(ctx, false, false, None, None, nav_delta, None, fs_idx);
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
                self.handle_fs_navigation(ctx, false, false, None, None, 0, Some(new_idx), fs_idx);
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
                return Some(AddressBarNav::Direct(path));
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
                self.drill_into_container(path, is_zip);
                None
            }
            // ネスト ZIP ツリーの子コンテナへ降りる (Phase 3)。
            Some(GridItem::ZipDir { dir_prefix, .. }) => {
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

#[cfg(test)]
mod tests {
    use super::{PadDir, continuous_reading_stick_axis, gamepad_grid_nav_target_pos};
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
}
