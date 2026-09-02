use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};
use std::time::{Duration, Instant};

use crate::ring_shortcut::RingDirection;

#[cfg(not(test))]
use gilrs::{Axis, Button, EventType, Gilrs};

/// 入力イベントキューの上限。UI が長時間 drain しない (最小化 / 長 stall) と無制限に
/// 溜まってメモリスパイク / 復帰時ヒッチになるため上限を設ける。溢れたときは **最古** を
/// 捨てて最新を残す (= release / disconnect / 中立軸 などの重要イベントを失わない)。
#[cfg(not(test))]
const GAMEPAD_EVENT_CAP: usize = 256;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum PadButton {
    DPadUp,
    DPadDown,
    DPadLeft,
    DPadRight,
    South,
    East,
    West,
    North,
    LeftShoulder,
    RightShoulder,
    LeftTrigger,
    RightTrigger,
    Select,
    Start,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PadAxis {
    LeftX,
    LeftY,
    RightX,
    RightY,
    LeftTrigger,
    RightTrigger,
}

impl PadAxis {
    fn index(self) -> usize {
        match self {
            Self::LeftX => 0,
            Self::LeftY => 1,
            Self::RightX => 2,
            Self::RightY => 3,
            Self::LeftTrigger => 4,
            Self::RightTrigger => 5,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PadEvent {
    ButtonPressed(PadButton),
    ButtonReleased(PadButton),
    AxisChanged(PadAxis, f32),
    Connected,
    Disconnected,
}

pub struct GamepadRuntime {
    /// producer (gilrs スレッド) → consumer (UI スレッド) の共有キュー。mpsc を使わず
    /// `VecDeque` にしているのは、満杯時に **最古** を捨てて最新を残す (drop-oldest) ため。
    /// mpsc の sync_channel は満杯時に送ろうとした最新を捨ててしまい、release / disconnect /
    /// 中立軸を取りこぼして状態が固着しうる。
    queue: Arc<Mutex<VecDeque<PadEvent>>>,
    shutdown: Arc<AtomicBool>,
    #[cfg(not(test))]
    handle: Option<std::thread::JoinHandle<()>>,
    started: bool,
}

impl Default for GamepadRuntime {
    fn default() -> Self {
        Self::new()
    }
}

impl GamepadRuntime {
    pub fn new() -> Self {
        Self {
            queue: Arc::new(Mutex::new(VecDeque::new())),
            shutdown: Arc::new(AtomicBool::new(false)),
            #[cfg(not(test))]
            handle: None,
            started: false,
        }
    }

    /// この frame ぶんのイベントを取り出す。`enabled` が false のときは
    /// **デバイスを読むスレッドごと止める**。
    ///
    /// 読み捨てるだけでは足りない。gilrs スレッドは入力を拾うたびに
    /// `ctx.request_repaint()` を呼ぶので、スティックが少しずれているパッドが
    /// 挿さっているだけで UI が起き続ける。無効にした人が求めているのは
    /// 「反応しないこと」であって「反応しないまま起き続けること」ではない。
    pub fn drain(&mut self, ctx: &egui::Context, enabled: bool) -> Vec<PadEvent> {
        if !enabled {
            self.stop();
            return Vec::new();
        }
        self.ensure_started(ctx);
        let mut q = self.queue.lock().unwrap_or_else(|e| e.into_inner());
        q.drain(..).collect()
    }

    /// 設定から drain までの配線を見るテスト用。デバイス無しで 1 件流し込む。
    #[cfg(test)]
    pub(crate) fn push_for_test(&mut self, event: PadEvent) {
        self.queue
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push_back(event);
    }

    /// 読み取りスレッドを止め、溜まっていたイベントを捨てる。再度 `enabled` に
    /// なれば `ensure_started` が新しいスレッドを立てる。
    ///
    /// `shutdown` は次に生きるスレッドと共有しないよう、停止のたびに新しい
    /// フラグへ差し替える。使い回すと、再開したスレッドが前回の停止要求を
    /// 読んで即座に終了する。
    fn stop(&mut self) {
        if !self.started {
            return;
        }
        self.started = false;
        self.shutdown.store(true, Ordering::Relaxed);
        self.shutdown = Arc::new(AtomicBool::new(false));
        #[cfg(not(test))]
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
        let mut q = self.queue.lock().unwrap_or_else(|e| e.into_inner());
        q.clear();
    }

    #[cfg(test)]
    fn ensure_started(&mut self, _ctx: &egui::Context) {
        self.started = true;
    }

    #[cfg(not(test))]
    fn ensure_started(&mut self, ctx: &egui::Context) {
        if self.started {
            return;
        }
        self.started = true;
        let queue = Arc::clone(&self.queue);
        let shutdown = Arc::clone(&self.shutdown);
        let repaint_ctx = ctx.clone();
        self.handle = std::thread::Builder::new()
            .name("miv-gamepad".to_string())
            .spawn(move || {
                let mut gilrs = match Gilrs::new() {
                    Ok(gilrs) => gilrs,
                    Err(err) => {
                        crate::logger::log(format!("[gamepad] gilrs init failed: {err}"));
                        return;
                    }
                };

                while !shutdown.load(Ordering::Relaxed) {
                    let mut sent_any = false;
                    while let Some(event) = gilrs.next_event() {
                        if let Some(mapped) = map_gilrs_event(event.event) {
                            let mut q = queue.lock().unwrap_or_else(|e| e.into_inner());
                            // 満杯時は最古を捨てて最新を残す (release/disconnect/中立軸を守る)。
                            if q.len() >= GAMEPAD_EVENT_CAP {
                                q.pop_front();
                            }
                            q.push_back(mapped);
                            drop(q);
                            sent_any = true;
                        }
                    }
                    if sent_any {
                        repaint_ctx.request_repaint();
                        std::thread::sleep(Duration::from_millis(2));
                    } else {
                        std::thread::sleep(Duration::from_millis(16));
                    }
                }
            })
            .map_err(|err| {
                crate::logger::log(format!("[gamepad] thread spawn failed: {err}"));
                err
            })
            .ok();
    }
}

impl Drop for GamepadRuntime {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::Relaxed);
        #[cfg(not(test))]
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

#[cfg(not(test))]
fn map_gilrs_event(event: EventType) -> Option<PadEvent> {
    match event {
        EventType::ButtonPressed(button, _) => map_button(button).map(PadEvent::ButtonPressed),
        EventType::ButtonReleased(button, _) => map_button(button).map(PadEvent::ButtonReleased),
        EventType::ButtonChanged(button, value, _) => {
            let axis = match button {
                Button::LeftTrigger2 => Some(PadAxis::LeftTrigger),
                Button::RightTrigger2 => Some(PadAxis::RightTrigger),
                _ => None,
            };
            if let Some(axis) = axis {
                Some(PadEvent::AxisChanged(axis, value.clamp(0.0, 1.0)))
            } else {
                map_button(button).map(|button| {
                    if value >= 0.5 {
                        PadEvent::ButtonPressed(button)
                    } else {
                        PadEvent::ButtonReleased(button)
                    }
                })
            }
        }
        EventType::AxisChanged(axis, value, _) => {
            map_axis(axis).map(|axis| PadEvent::AxisChanged(axis, value.clamp(-1.0, 1.0)))
        }
        EventType::Connected => Some(PadEvent::Connected),
        EventType::Disconnected => Some(PadEvent::Disconnected),
        _ => None,
    }
}

#[cfg(not(test))]
fn map_button(button: Button) -> Option<PadButton> {
    match button {
        Button::DPadUp => Some(PadButton::DPadUp),
        Button::DPadDown => Some(PadButton::DPadDown),
        Button::DPadLeft => Some(PadButton::DPadLeft),
        Button::DPadRight => Some(PadButton::DPadRight),
        Button::South => Some(PadButton::South),
        Button::East => Some(PadButton::East),
        Button::West => Some(PadButton::West),
        Button::North => Some(PadButton::North),
        Button::LeftTrigger => Some(PadButton::LeftShoulder),
        Button::RightTrigger => Some(PadButton::RightShoulder),
        Button::LeftTrigger2 => Some(PadButton::LeftTrigger),
        Button::RightTrigger2 => Some(PadButton::RightTrigger),
        Button::Select => Some(PadButton::Select),
        Button::Start => Some(PadButton::Start),
        _ => None,
    }
}

#[cfg(not(test))]
fn map_axis(axis: Axis) -> Option<PadAxis> {
    match axis {
        Axis::LeftStickX => Some(PadAxis::LeftX),
        Axis::LeftStickY => Some(PadAxis::LeftY),
        Axis::RightStickX => Some(PadAxis::RightX),
        Axis::RightStickY => Some(PadAxis::RightY),
        Axis::LeftZ => Some(PadAxis::LeftTrigger),
        Axis::RightZ => Some(PadAxis::RightTrigger),
        _ => None,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TriggerRange {
    Unknown,
    MinusOneToOne,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WestReleaseOutcome {
    Picker,
    Ring(RingDirection),
    Suppressed,
}

pub struct GamepadInputState {
    buttons: HashSet<PadButton>,
    axes: [f32; 6],
    trigger_ranges: [TriggerRange; 2],
    repeat_next: HashMap<PadButton, Instant>,
    y_modifier_used: bool,
    west_ring_direction: Option<RingDirection>,
    west_tap_suppressed: bool,
    directional_neutral_required: bool,
    analog_last_tick: Option<Instant>,
    left_stick_next_step: Option<Instant>,
    trigger_next_step: Option<Instant>,
}

impl Default for GamepadInputState {
    fn default() -> Self {
        Self {
            buttons: HashSet::new(),
            axes: [0.0; 6],
            trigger_ranges: [TriggerRange::Unknown; 2],
            repeat_next: HashMap::new(),
            y_modifier_used: false,
            west_ring_direction: None,
            west_tap_suppressed: false,
            directional_neutral_required: false,
            analog_last_tick: None,
            left_stick_next_step: None,
            trigger_next_step: None,
        }
    }
}

impl GamepadInputState {
    pub fn set_button_down(&mut self, button: PadButton, down: bool, now: Instant) -> bool {
        let changed = if down {
            self.buttons.insert(button)
        } else {
            self.buttons.remove(&button)
        };
        if down && changed {
            if button == PadButton::North {
                self.y_modifier_used = false;
            }
            if button == PadButton::West {
                self.reset_west_ring_state();
            }
        } else if !down {
            self.repeat_next.remove(&button);
        }
        if down && changed && is_repeatable_button(button) {
            self.repeat_next
                .insert(button, now + Duration::from_millis(300));
        }
        changed
    }

    /// 保持中のボタン / 軸 / リピート / step タイマをすべてクリアする。
    /// コントローラ切断時に呼び、握ったまま切断 → リピート/アナログが止まらない
    /// 不具合を防ぐ。トリガーのレンジ較正 (`trigger_ranges`) は次接続でも有効なので残す。
    pub fn clear(&mut self) {
        self.buttons.clear();
        self.axes = [0.0; 6];
        self.repeat_next.clear();
        self.y_modifier_used = false;
        self.reset_west_ring_state();
        self.directional_neutral_required = false;
        self.analog_last_tick = None;
        self.left_stick_next_step = None;
        self.trigger_next_step = None;
    }

    /// ディスパッチがブロックされている間 (ダイアログ / IME / 編集モード) に押された
    /// ボタンが、ブロック解除後に遅れて発火するのを防ぐ。リピート予約をクリアし
    /// (= 解除後に保持中ボタンが勝手に連続発火しない)、保持中の Y は modifier 使用済み
    /// 扱いにして「離し = タップ」を抑止する。軸 / ボタン保持状態自体は残すので、
    /// 一瞬のポップアップ後もアナログ操作はそのまま継続できる。
    pub fn suppress_pending_actions(&mut self) {
        self.repeat_next.clear();
        if self.buttons.contains(&PadButton::North) {
            self.y_modifier_used = true;
        }
        if self.buttons.contains(&PadButton::West) {
            self.west_tap_suppressed = true;
            self.west_ring_direction = None;
            // X リング操作中にブロックされた場合、リング用に倒していたスティックが
            // ブロック解除後 (または X 離しがブロック中に処理された後) にそのまま通常
            // アナログ操作 (ページ移動 / シーク / グリッド移動) として発火しないよう、
            // ニュートラル通過を要求する (review-v2.3.0 hunt P2)。
            self.require_directional_neutral();
        }
    }

    pub fn set_axis(&mut self, axis: PadAxis, value: f32) {
        let value = value.clamp(-1.0, 1.0);
        if axis == PadAxis::LeftTrigger || axis == PadAxis::RightTrigger {
            let idx = if axis == PadAxis::LeftTrigger { 0 } else { 1 };
            if value < -0.25 {
                self.trigger_ranges[idx] = TriggerRange::MinusOneToOne;
            }
        }
        self.axes[axis.index()] = value;
    }

    pub fn button_down(&self, button: PadButton) -> bool {
        self.buttons.contains(&button)
    }

    pub fn axis(&self, axis: PadAxis) -> f32 {
        self.axes[axis.index()]
    }

    pub fn trigger_value(&self, left: bool) -> f32 {
        let button = if left {
            PadButton::LeftTrigger
        } else {
            PadButton::RightTrigger
        };
        let axis_value = self.trigger_axis_value(left);
        if self.button_down(button) {
            axis_value.max(1.0)
        } else {
            axis_value
        }
    }

    pub fn trigger_axis_value(&self, left: bool) -> f32 {
        let (axis, range) = if left {
            (PadAxis::LeftTrigger, self.trigger_ranges[0])
        } else {
            (PadAxis::RightTrigger, self.trigger_ranges[1])
        };
        match range {
            TriggerRange::Unknown => self.axis(axis).clamp(0.0, 1.0),
            TriggerRange::MinusOneToOne => ((self.axis(axis) + 1.0) * 0.5).clamp(0.0, 1.0),
        }
    }

    pub fn due_button_repeat(
        &mut self,
        button: PadButton,
        now: Instant,
        interval: Duration,
    ) -> bool {
        if !self.button_down(button) {
            self.repeat_next.remove(&button);
            return false;
        }
        let Some(next) = self.repeat_next.get_mut(&button) else {
            return false;
        };
        if now < *next {
            return false;
        }
        *next = now + interval;
        true
    }

    pub fn mark_y_modifier_used(&mut self) {
        if self.button_down(PadButton::North) {
            self.y_modifier_used = true;
        }
    }

    pub fn y_modifier_used(&self) -> bool {
        self.y_modifier_used
    }

    pub fn west_ring_active(&self) -> bool {
        self.button_down(PadButton::West) && !self.west_tap_suppressed
    }

    pub fn west_ring_direction(&self) -> Option<RingDirection> {
        if self.west_ring_active() {
            self.west_ring_direction
        } else {
            None
        }
    }

    pub fn mark_west_ring_direction(&mut self, direction: RingDirection) {
        if self.west_ring_active() {
            self.west_ring_direction = Some(direction);
        }
    }

    pub fn cancel_west_ring(&mut self) {
        if self.button_down(PadButton::West) {
            self.west_ring_direction = None;
            self.west_tap_suppressed = true;
            self.require_directional_neutral();
        }
    }

    pub fn require_directional_neutral(&mut self) {
        self.directional_neutral_required = true;
        self.left_stick_next_step = None;
        self.analog_last_tick = None;
        for button in DPAD_BUTTONS {
            self.repeat_next.remove(&button);
        }
    }

    pub fn clear_directional_neutral_required(&mut self) {
        self.directional_neutral_required = false;
    }

    pub fn directional_neutral_required(&self) -> bool {
        self.directional_neutral_required
    }

    pub fn dpad_direction_down(&self) -> bool {
        DPAD_BUTTONS
            .iter()
            .any(|button| self.buttons.contains(button))
    }

    pub fn finish_west_release(&mut self) -> WestReleaseOutcome {
        let outcome = if self.west_tap_suppressed {
            WestReleaseOutcome::Suppressed
        } else if let Some(direction) = self.west_ring_direction {
            WestReleaseOutcome::Ring(direction)
        } else {
            WestReleaseOutcome::Picker
        };
        self.reset_west_ring_state();
        outcome
    }

    pub fn analog_dt(&mut self, active: bool, now: Instant) -> f32 {
        if !active {
            self.analog_last_tick = None;
            return 0.0;
        }
        let dt = self
            .analog_last_tick
            .map(|last| now.saturating_duration_since(last).as_secs_f32())
            .unwrap_or(1.0 / 60.0)
            .clamp(0.0, 0.05);
        self.analog_last_tick = Some(now);
        dt
    }

    pub fn left_stick_step_due(&mut self, active: bool, now: Instant, interval: Duration) -> bool {
        step_due(&mut self.left_stick_next_step, active, now, interval)
    }

    pub fn trigger_step_due(&mut self, active: bool, now: Instant, interval: Duration) -> bool {
        step_due(&mut self.trigger_next_step, active, now, interval)
    }

    pub fn repeat_active(&self) -> bool {
        self.buttons
            .iter()
            .any(|&button| is_repeatable_button(button))
    }

    fn reset_west_ring_state(&mut self) {
        self.west_ring_direction = None;
        self.west_tap_suppressed = false;
    }
}

fn is_repeatable_button(button: PadButton) -> bool {
    matches!(
        button,
        PadButton::DPadUp
            | PadButton::DPadDown
            | PadButton::DPadLeft
            | PadButton::DPadRight
            | PadButton::LeftShoulder
            | PadButton::RightShoulder
    )
}

const DPAD_BUTTONS: [PadButton; 4] = [
    PadButton::DPadUp,
    PadButton::DPadDown,
    PadButton::DPadLeft,
    PadButton::DPadRight,
];

fn step_due(
    next_step: &mut Option<Instant>,
    active: bool,
    now: Instant,
    interval: Duration,
) -> bool {
    if !active {
        *next_step = None;
        return false;
    }
    match next_step {
        Some(next) if now >= *next => {
            *next = now + interval;
            true
        }
        Some(_) => false,
        None => {
            *next_step = Some(now + interval);
            true
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        GamepadInputState, GamepadRuntime, PadAxis, PadButton, PadEvent, WestReleaseOutcome,
    };
    use std::sync::atomic::Ordering;

    /// 無効にしたら、読むのをやめる。
    ///
    /// 読んだ結果を捨てるだけでは足りない: gilrs スレッドは入力のたびに repaint を
    /// 要求するので、スティックがずれているパッドが挿さっているだけで UI が起き続ける。
    #[test]
    fn disabling_the_pad_stops_reading_it_and_re_enabling_starts_again() {
        let ctx = egui::Context::default();
        let mut runtime = GamepadRuntime::new();

        assert!(runtime.drain(&ctx, true).is_empty());
        assert!(runtime.started, "有効なら読み取りを始める");

        // 停止前に届いていた分は配らない。無効にした後で古い入力が 1 回流れると、
        // 「切ったのに動いた」になる。
        runtime
            .queue
            .lock()
            .unwrap()
            .push_back(PadEvent::ButtonPressed(PadButton::South));
        assert!(runtime.drain(&ctx, false).is_empty());
        assert!(!runtime.started, "無効なら読み取りを止める");
        assert!(
            runtime.queue.lock().unwrap().is_empty(),
            "溜まっていた入力は捨てる"
        );
        assert!(
            !runtime.shutdown.load(Ordering::Relaxed),
            "停止フラグは次のスレッドと共有しない。使い回すと再開したスレッドが即座に終了する"
        );

        assert!(runtime.drain(&ctx, true).is_empty());
        assert!(runtime.started, "有効に戻したら読み取りを再開する");
    }

    use crate::ring_shortcut::RingDirection;
    use std::time::Instant;

    #[test]
    fn suppress_during_west_ring_requires_directional_neutral() {
        // review-v2.3.0 hunt P2: X リング中にディスパッチがブロックされたら、リング用に
        // 倒していたスティックがブロック解除後に通常アナログ操作へ漏れない (ニュートラル
        // 通過を要求する)。
        let mut state = GamepadInputState::default();
        let now = Instant::now();
        state.set_button_down(PadButton::West, true, now);
        state.set_axis(PadAxis::LeftX, 1.0);
        assert!(!state.directional_neutral_required());

        state.suppress_pending_actions();

        assert!(
            state.directional_neutral_required(),
            "ブロック中の suppress は neutral gate を立てる"
        );
        // West を保持していない通常ブロックでは gate を立てない (アナログ操作継続の設計)。
        let mut plain = GamepadInputState::default();
        plain.set_axis(PadAxis::LeftX, 1.0);
        plain.suppress_pending_actions();
        assert!(!plain.directional_neutral_required());
    }

    #[test]
    fn west_release_without_direction_opens_picker() {
        let mut state = GamepadInputState::default();
        let now = Instant::now();
        state.set_button_down(PadButton::West, true, now);
        state.set_button_down(PadButton::West, false, now);

        assert_eq!(state.finish_west_release(), WestReleaseOutcome::Picker);
    }

    #[test]
    fn west_release_with_direction_fires_ring() {
        let mut state = GamepadInputState::default();
        let now = Instant::now();
        state.set_button_down(PadButton::West, true, now);
        state.mark_west_ring_direction(RingDirection::UpRight);
        state.set_button_down(PadButton::West, false, now);

        assert_eq!(
            state.finish_west_release(),
            WestReleaseOutcome::Ring(RingDirection::UpRight)
        );
    }

    #[test]
    fn duplicate_west_press_does_not_reset_ring_direction() {
        let mut state = GamepadInputState::default();
        let now = Instant::now();
        assert!(state.set_button_down(PadButton::West, true, now));
        state.mark_west_ring_direction(RingDirection::DownLeft);

        assert!(!state.set_button_down(PadButton::West, true, now));
        state.set_button_down(PadButton::West, false, now);

        assert_eq!(
            state.finish_west_release(),
            WestReleaseOutcome::Ring(RingDirection::DownLeft)
        );
    }

    #[test]
    fn west_release_after_suppress_does_not_fire() {
        let mut state = GamepadInputState::default();
        let now = Instant::now();
        state.set_button_down(PadButton::West, true, now);
        state.mark_west_ring_direction(RingDirection::Right);
        state.suppress_pending_actions();
        state.set_button_down(PadButton::West, false, now);

        assert_eq!(state.finish_west_release(), WestReleaseOutcome::Suppressed);
    }

    #[test]
    fn cancel_west_ring_requires_directional_neutral() {
        let mut state = GamepadInputState::default();
        let now = Instant::now();
        state.set_button_down(PadButton::West, true, now);
        state.set_button_down(PadButton::DPadRight, true, now);
        state.mark_west_ring_direction(RingDirection::Right);

        state.cancel_west_ring();

        assert!(state.directional_neutral_required());
        assert!(state.dpad_direction_down());
        assert!(!state.due_button_repeat(
            PadButton::DPadRight,
            now + std::time::Duration::from_secs(1),
            std::time::Duration::from_millis(95)
        ));
        state.set_button_down(PadButton::West, false, now);
        assert_eq!(state.finish_west_release(), WestReleaseOutcome::Suppressed);
    }

    #[test]
    fn directional_neutral_gate_clears_dpad_repeats() {
        let mut state = GamepadInputState::default();
        let now = Instant::now();
        state.set_button_down(PadButton::DPadRight, true, now);

        state.require_directional_neutral();

        assert!(state.directional_neutral_required());
        assert!(state.dpad_direction_down());
        assert!(!state.due_button_repeat(
            PadButton::DPadRight,
            now + std::time::Duration::from_secs(1),
            std::time::Duration::from_millis(95)
        ));

        state.set_button_down(PadButton::DPadRight, false, now);
        assert!(!state.dpad_direction_down());
        state.clear_directional_neutral_required();
        assert!(!state.directional_neutral_required());
    }

    #[test]
    fn trigger_axis_value_excludes_synthetic_button_state() {
        let mut state = GamepadInputState::default();
        let now = Instant::now();

        state.set_axis(PadAxis::LeftTrigger, 0.75);
        assert_eq!(state.trigger_axis_value(true), 0.75);
        assert_eq!(state.trigger_value(true), 0.75);

        state.set_button_down(PadButton::LeftTrigger, true, now);
        state.set_axis(PadAxis::LeftTrigger, 0.0);
        assert_eq!(state.trigger_axis_value(true), 0.0);
        assert_eq!(state.trigger_value(true), 1.0);
    }
}
