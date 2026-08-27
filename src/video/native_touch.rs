use crate::touch_input::{
    TapZoneGeometry, TouchCommand, TouchOwner, TouchPhase, TouchRecognizer, TouchSample,
};

pub(crate) const MAX_OWNED_TOUCH_POINTERS: usize = 16;
const PRIMARY_CANCEL_DISTANCE_POINTS: f32 = 1024.0;
pub(crate) const VIDEO_TAP_SEEK_SECS: f64 = 5.0;

/// Native HWND that originated a video-overlay input stream.
///
/// This type lives in the platform-neutral adapter module so source-aware
/// touch classification remains covered by non-Windows unit tests.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum NativeVideoWindowSource {
    Presenter,
    Hud,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NativeVideoTouchPhase {
    Start,
    Move,
    End,
    Cancel,
}

impl From<NativeVideoTouchPhase> for TouchPhase {
    fn from(value: NativeVideoTouchPhase) -> Self {
        match value {
            NativeVideoTouchPhase::Start => Self::Start,
            NativeVideoTouchPhase::Move => Self::Move,
            NativeVideoTouchPhase::End => Self::End,
            NativeVideoTouchPhase::Cancel => Self::Cancel,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NativeVideoTouchEvent {
    pub(crate) source: NativeVideoWindowSource,
    pub pointer_id: u32,
    /// Source-HWND client coordinate in physical pixels. Presenter and HUD
    /// mirror the same client geometry, so both join the existing conversion.
    pub x: i32,
    /// Source-HWND client coordinate in physical pixels.
    pub y: i32,
    pub phase: NativeVideoTouchPhase,
    /// This stream began while the presenter was not active, so it must not
    /// reach an overlay control. The gesture itself is still recognized; see
    /// `native_touch_is_activation_tap`.
    pub suppress_widget_primary: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum NativePointerTypeProbe {
    Touch,
    NonTouch,
    Failed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum NativeTouchPassReason {
    Disabled,
    NonTouch,
    PointerTypeQueryFailed,
    UnregisteredPointerId,
    CapacityExceeded,
}

impl NativeTouchPassReason {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Disabled => "disabled",
            Self::NonTouch => "non_pt_touch",
            Self::PointerTypeQueryFailed => "pointer_type_query_failed",
            Self::UnregisteredPointerId => "unregistered_pointer_id",
            Self::CapacityExceeded => "capacity_exceeded",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum NativeTouchOwnershipDecision {
    Owned,
    Passed(NativeTouchPassReason),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct OwnedTouchPointer {
    id: u32,
    last_client_position: Option<[i32; 2]>,
    suppress_widget_primary: bool,
}

/// Pure, bounded ownership state for one native input HWND.
///
/// A pointer id can enter this set only at `WM_POINTERDOWN`. Follow-up
/// messages never claim an unregistered id, preserving whole-stream Win32
/// ownership.
#[derive(Clone, Debug)]
pub(crate) struct NativeTouchOwnership {
    pointers: Vec<OwnedTouchPointer>,
    capacity: usize,
}

impl Default for NativeTouchOwnership {
    fn default() -> Self {
        Self::with_capacity(MAX_OWNED_TOUCH_POINTERS)
    }
}

impl NativeTouchOwnership {
    pub(crate) fn with_capacity(capacity: usize) -> Self {
        Self {
            pointers: Vec::with_capacity(capacity),
            capacity,
        }
    }

    pub(crate) fn begin(
        &mut self,
        pointer_id: u32,
        enabled: bool,
        probe: NativePointerTypeProbe,
    ) -> NativeTouchOwnershipDecision {
        if !enabled {
            return NativeTouchOwnershipDecision::Passed(NativeTouchPassReason::Disabled);
        }
        match probe {
            NativePointerTypeProbe::NonTouch => {
                return NativeTouchOwnershipDecision::Passed(NativeTouchPassReason::NonTouch);
            }
            NativePointerTypeProbe::Failed => {
                return NativeTouchOwnershipDecision::Passed(
                    NativeTouchPassReason::PointerTypeQueryFailed,
                );
            }
            NativePointerTypeProbe::Touch => {}
        }
        if self.contains(pointer_id) {
            return NativeTouchOwnershipDecision::Owned;
        }
        if self.pointers.len() >= self.capacity {
            return NativeTouchOwnershipDecision::Passed(NativeTouchPassReason::CapacityExceeded);
        }
        self.pointers.push(OwnedTouchPointer {
            id: pointer_id,
            last_client_position: None,
            suppress_widget_primary: false,
        });
        NativeTouchOwnershipDecision::Owned
    }

    pub(crate) fn followup(&self, pointer_id: u32) -> NativeTouchOwnershipDecision {
        if self.contains(pointer_id) {
            NativeTouchOwnershipDecision::Owned
        } else {
            NativeTouchOwnershipDecision::Passed(NativeTouchPassReason::UnregisteredPointerId)
        }
    }

    pub(crate) fn record_client_position(&mut self, pointer_id: u32, x: i32, y: i32) {
        if let Some(pointer) = self
            .pointers
            .iter_mut()
            .find(|pointer| pointer.id == pointer_id)
        {
            pointer.last_client_position = Some([x, y]);
        }
    }

    pub(crate) fn last_client_position(&self, pointer_id: u32) -> Option<[i32; 2]> {
        self.pointers
            .iter()
            .find(|pointer| pointer.id == pointer_id)
            .and_then(|pointer| pointer.last_client_position)
    }

    pub(crate) fn mark_suppress_widget_primary(&mut self, pointer_id: u32) {
        if let Some(pointer) = self
            .pointers
            .iter_mut()
            .find(|pointer| pointer.id == pointer_id)
        {
            pointer.suppress_widget_primary = true;
        }
    }

    pub(crate) fn suppresses_widget_primary(&self, pointer_id: u32) -> bool {
        self.pointers
            .iter()
            .find(|pointer| pointer.id == pointer_id)
            .is_some_and(|pointer| pointer.suppress_widget_primary)
    }

    pub(crate) fn has_suppressed_widget_stream(&self) -> bool {
        self.pointers
            .iter()
            .any(|pointer| pointer.suppress_widget_primary)
    }

    pub(crate) fn release(&mut self, pointer_id: u32) -> bool {
        let Some(index) = self
            .pointers
            .iter()
            .position(|pointer| pointer.id == pointer_id)
        else {
            return false;
        };
        self.pointers.swap_remove(index);
        true
    }

    pub(crate) fn clear(&mut self) {
        self.pointers.clear();
    }

    pub(crate) fn contains(&self, pointer_id: u32) -> bool {
        self.pointers.iter().any(|pointer| pointer.id == pointer_id)
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.pointers.is_empty()
    }

    pub(crate) fn first_pointer_id(&self) -> Option<u32> {
        self.pointers.first().map(|pointer| pointer.id)
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.pointers.len()
    }
}

/// Shared client-pixel to egui-point conversion for native mouse and touch.
pub(crate) fn native_client_pixels_to_points(
    client_pixels: [i32; 2],
    pixels_per_point: f32,
) -> egui::Pos2 {
    egui::pos2(
        client_pixels[0] as f32 / pixels_per_point,
        client_pixels[1] as f32 / pixels_per_point,
    )
}

/// Pure model of `ScreenToClient` followed by the shared point conversion.
///
/// The wndproc uses the real HWND-aware `ScreenToClient`; this helper makes
/// the coordinate contract testable without a Win32 desktop.
#[cfg(test)]
pub(crate) fn native_pointer_screen_pixels_to_points(
    screen_pixels: [i32; 2],
    client_origin_screen_pixels: [i32; 2],
    pixels_per_point: f32,
) -> egui::Pos2 {
    native_client_pixels_to_points(
        [
            screen_pixels[0] - client_origin_screen_pixels[0],
            screen_pixels[1] - client_origin_screen_pixels[1],
        ],
        pixels_per_point,
    )
}

pub(crate) fn native_touch_command_toggles_chrome_without_video_gestures(
    command: TouchCommand,
) -> bool {
    matches!(
        command,
        TouchCommand::ToggleChrome | TouchCommand::PageSide { .. }
    )
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum NativeVideoTouchCommand {
    ToggleChrome,
    SeekRelative {
        delta_secs: f64,
    },
    PanoramaDrag {
        delta_points: egui::Vec2,
        viewport_height_points: f32,
    },
    LearnAndShowChrome,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
enum PanoramaTouchOwnership {
    #[default]
    Inactive,
    Pending {
        pointer_id: u32,
        start: egui::Pos2,
    },
    Dragging {
        pointer_id: u32,
        last: egui::Pos2,
    },
    Cancelled,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PhysicalTapSide {
    Left,
    Right,
}

impl PhysicalTapSide {
    fn from_left(left: bool) -> Self {
        if left { Self::Left } else { Self::Right }
    }

    fn seek_delta_secs(self) -> f64 {
        match self {
            Self::Left => -VIDEO_TAP_SEEK_SECS,
            Self::Right => VIDEO_TAP_SEEK_SECS,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum TouchLearningState {
    #[default]
    Unknown,
    Unlearned,
    Learned,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FirstRunHelpPhase {
    AwaitingInitialRelease,
    Ready,
}

pub(crate) fn native_touch_mouse_discard_decision(
    gestures_enabled: bool,
    source_query_succeeded: bool,
    source_is_touch: bool,
) -> bool {
    gestures_enabled && source_query_succeeded && source_is_touch
}

/// Whether this stream began while the presenter was not the active window.
///
/// Mouse eats the whole activating click (`MA_ACTIVATEANDEAT`) because the
/// presenter body binds it to play/pause. Touch binds the presenter body only
/// to revealing chrome, which has no side effect, and someone who tapped an
/// inactive window plainly intends to operate it next. So the gesture is
/// recognized normally and only the synthetic press that could reach an
/// overlay control is withheld (2026-08-08, real-device judgement).
pub(crate) fn native_touch_is_activation_tap(
    is_child_window: bool,
    foreground_is_current_process: bool,
    presenter_is_foreground: bool,
) -> bool {
    if is_child_window {
        !foreground_is_current_process
    } else {
        !presenter_is_foreground
    }
}

pub(crate) fn native_touch_should_request_focus_claim(
    first_owned_stream: bool,
    is_child_window: bool,
    foreground_is_current_process: bool,
    presenter_is_foreground: bool,
    presenter_has_thread_focus: bool,
) -> bool {
    first_owned_stream
        && (native_touch_is_activation_tap(
            is_child_window,
            foreground_is_current_process,
            presenter_is_foreground,
        ) || !presenter_has_thread_focus)
}

pub(crate) fn native_touch_followup_phase(
    cancelled: bool,
    pointer_up: bool,
) -> NativeVideoTouchPhase {
    if cancelled {
        NativeVideoTouchPhase::Cancel
    } else if pointer_up {
        NativeVideoTouchPhase::End
    } else {
        NativeVideoTouchPhase::Move
    }
}

pub(crate) struct NativeTouchAdapter {
    recognizer: TouchRecognizer,
    primary_id: Option<u32>,
    primary_start_pos: Option<egui::Pos2>,
    primary_pressed: bool,
    /// The primary contact began on an inactive presenter, so no synthetic
    /// press is emitted for it. The gesture still runs.
    primary_withholds_press: bool,
    pending_commands: Vec<NativeVideoTouchCommand>,
    chrome_latched: bool,
    video_gestures_enabled: bool,
    learning_state: TouchLearningState,
    first_run_help_phase: Option<FirstRunHelpPhase>,
    panorama_active: bool,
    panorama_ownership: PanoramaTouchOwnership,
}

impl Default for NativeTouchAdapter {
    fn default() -> Self {
        Self {
            recognizer: TouchRecognizer::default(),
            primary_id: None,
            primary_start_pos: None,
            primary_pressed: false,
            primary_withholds_press: false,
            pending_commands: Vec::new(),
            chrome_latched: false,
            video_gestures_enabled: true,
            learning_state: TouchLearningState::Unknown,
            first_run_help_phase: None,
            panorama_active: false,
            panorama_ownership: PanoramaTouchOwnership::Inactive,
        }
    }
}

pub(crate) struct NativeTouchAdapterOutput {
    pub(crate) pos: egui::Pos2,
    pub(crate) egui_events: Vec<egui::Event>,
}

pub(crate) struct NativeTouchAdapterReset {
    pub(crate) changed: bool,
    pub(crate) egui_events: Vec<egui::Event>,
}

impl NativeTouchAdapter {
    pub(crate) fn configure_panorama(&mut self, active: bool) -> bool {
        if self.panorama_active == active {
            return false;
        }
        self.panorama_active = active;
        self.panorama_ownership = PanoramaTouchOwnership::Inactive;
        true
    }

    /// Applies the App-owned learned snapshot without allowing a stale false
    /// snapshot to resurrect help after this adapter emitted the learn event.
    /// Returns whether the visible help state changed.
    pub(crate) fn configure_video_gestures(
        &mut self,
        enabled: bool,
        learned_snapshot: Option<bool>,
    ) -> bool {
        let was_visible = self.first_run_help_visible();
        if self.video_gestures_enabled != enabled {
            self.video_gestures_enabled = enabled;
            self.first_run_help_phase = None;
        }
        if enabled {
            match learned_snapshot {
                Some(true) => {
                    self.learning_state = TouchLearningState::Learned;
                    self.first_run_help_phase = None;
                }
                Some(false) if self.learning_state != TouchLearningState::Learned => {
                    self.learning_state = TouchLearningState::Unlearned;
                }
                _ => {}
            }
        }
        was_visible != self.first_run_help_visible()
    }

    pub(crate) fn handle_event(
        &mut self,
        event: NativeVideoTouchEvent,
        geometry: &TapZoneGeometry,
        now_ms: u64,
        pixels_per_point: f32,
    ) -> NativeTouchAdapterOutput {
        let pos = native_client_pixels_to_points([event.x, event.y], pixels_per_point);
        if event.phase == NativeVideoTouchPhase::Start {
            if self.video_gestures_enabled
                && self.learning_state == TouchLearningState::Unlearned
                && self.first_run_help_phase.is_none()
                && !self.recognizer.is_active()
            {
                self.first_run_help_phase = Some(FirstRunHelpPhase::AwaitingInitialRelease);
            }
        }
        if event.phase == NativeVideoTouchPhase::Start
            && self.primary_id.is_none()
            && !self.recognizer.is_active()
        {
            self.primary_id = Some(event.pointer_id);
            self.primary_start_pos = Some(pos);
            self.primary_pressed = false;
            self.primary_withholds_press = event.suppress_widget_primary;
            if self.panorama_active && event.source == NativeVideoWindowSource::Presenter {
                self.panorama_ownership = PanoramaTouchOwnership::Pending {
                    pointer_id: event.pointer_id,
                    start: pos,
                };
            }
        }
        let is_primary = self.primary_id == Some(event.pointer_id);
        let sample = TouchSample {
            id: u64::from(event.pointer_id),
            pos,
            phase: event.phase.into(),
            now_ms,
        };
        // One typed choke point decides origin semantics. A pointer can reach
        // the HUD HWND only after the OS hit-test selected its interactive
        // region, so HUD starts bypass presenter geometry classification.
        let help_visible = self.first_run_help_visible();
        let commands = match (help_visible, event.source) {
            (true, _) => {
                let mut help_geometry = geometry.clone();
                help_geometry.excluded.clear();
                self.recognizer.handle_sample(&help_geometry, sample)
            }
            (false, NativeVideoWindowSource::Presenter) => {
                self.recognizer.handle_sample(geometry, sample)
            }
            (false, NativeVideoWindowSource::Hud) => self
                .recognizer
                .handle_widget_passthrough_sample(geometry, sample),
        };
        self.consume_touch_commands(commands);
        self.consume_panorama_touch(event, pos, geometry.surface.height());

        let suppress_primary = self.recognizer.should_suppress_primary();
        let mut egui_events = Vec::new();
        let cancelled_existing_press =
            suppress_primary && self.cancel_primary_press(pos, &mut egui_events);

        if is_primary && !cancelled_existing_press {
            match event.phase {
                NativeVideoTouchPhase::Start => {
                    if !suppress_primary
                        && !self.primary_withholds_press
                        && self.recognizer.owner() == TouchOwner::WidgetPassthrough
                    {
                        self.press_primary_at(pos, &mut egui_events);
                    } else {
                        egui_events.push(egui::Event::PointerMoved(pos));
                    }
                }
                NativeVideoTouchPhase::Move | NativeVideoTouchPhase::End => {
                    if !suppress_primary
                        && !self.primary_withholds_press
                        && !self.primary_pressed
                        && matches!(
                            self.recognizer.owner(),
                            TouchOwner::WidgetPassthrough | TouchOwner::ViewerPointerPassthrough
                        )
                    {
                        let start = self.primary_start_pos.unwrap_or(pos);
                        self.press_primary_at(start, &mut egui_events);
                    }
                    egui_events.push(egui::Event::PointerMoved(pos));
                    if event.phase == NativeVideoTouchPhase::End {
                        if self.primary_pressed && !suppress_primary {
                            self.release_primary_at(pos, &mut egui_events);
                        } else {
                            egui_events.push(egui::Event::PointerGone);
                        }
                    }
                }
                NativeVideoTouchPhase::Cancel => {
                    if !self.cancel_primary_press(pos, &mut egui_events) {
                        egui_events.push(egui::Event::PointerGone);
                    }
                }
            }
        }

        if !self.recognizer.is_active() {
            if self.primary_pressed {
                self.cancel_primary_press(pos, &mut egui_events);
            }
            self.primary_id = None;
            self.primary_start_pos = None;
            self.primary_pressed = false;
            self.primary_withholds_press = false;
        }

        NativeTouchAdapterOutput { pos, egui_events }
    }

    fn consume_panorama_touch(
        &mut self,
        event: NativeVideoTouchEvent,
        pos: egui::Pos2,
        viewport_height_points: f32,
    ) {
        if !self.panorama_active || event.source != NativeVideoWindowSource::Presenter {
            return;
        }
        if self.recognizer.owner() == TouchOwner::Pinch {
            self.panorama_ownership = PanoramaTouchOwnership::Cancelled;
            return;
        }
        match (self.panorama_ownership, event.phase) {
            (
                PanoramaTouchOwnership::Pending { pointer_id, start },
                NativeVideoTouchPhase::Move | NativeVideoTouchPhase::End,
            ) if pointer_id == event.pointer_id => {
                let delta = pos - start;
                if delta.length_sq()
                    > crate::touch_input::TAP_MAX_DISTANCE_PT
                        * crate::touch_input::TAP_MAX_DISTANCE_PT
                {
                    if delta.length_sq() > 0.0 {
                        self.pending_commands
                            .push(NativeVideoTouchCommand::PanoramaDrag {
                                delta_points: delta,
                                viewport_height_points,
                            });
                    }
                    self.panorama_ownership = PanoramaTouchOwnership::Dragging {
                        pointer_id,
                        last: pos,
                    };
                }
            }
            (
                PanoramaTouchOwnership::Dragging { pointer_id, last },
                NativeVideoTouchPhase::Move | NativeVideoTouchPhase::End,
            ) if pointer_id == event.pointer_id => {
                let delta = pos - last;
                if delta.length_sq() > 0.0 {
                    self.pending_commands
                        .push(NativeVideoTouchCommand::PanoramaDrag {
                            delta_points: delta,
                            viewport_height_points,
                        });
                }
                self.panorama_ownership = PanoramaTouchOwnership::Dragging {
                    pointer_id,
                    last: pos,
                };
            }
            (_, NativeVideoTouchPhase::Cancel) => {
                self.panorama_ownership = PanoramaTouchOwnership::Cancelled;
            }
            _ => {}
        }
        if !self.recognizer.is_active() {
            self.panorama_ownership = PanoramaTouchOwnership::Inactive;
        }
    }

    fn consume_touch_commands(&mut self, commands: Vec<TouchCommand>) {
        if let Some(phase) = self.first_run_help_phase {
            if phase == FirstRunHelpPhase::Ready
                && commands.iter().any(|command| {
                    matches!(
                        command,
                        TouchCommand::ToggleChrome | TouchCommand::PageSide { .. }
                    )
                })
            {
                self.learning_state = TouchLearningState::Learned;
                self.first_run_help_phase = None;
                self.pending_commands
                    .push(NativeVideoTouchCommand::LearnAndShowChrome);
            } else if phase == FirstRunHelpPhase::AwaitingInitialRelease
                && !self.recognizer.is_active()
            {
                self.first_run_help_phase = Some(FirstRunHelpPhase::Ready);
            }
            return;
        }

        for command in commands {
            match command {
                TouchCommand::PageSide { left } if self.video_gestures_enabled => {
                    let side = PhysicalTapSide::from_left(left);
                    self.pending_commands
                        .push(NativeVideoTouchCommand::SeekRelative {
                            delta_secs: side.seek_delta_secs(),
                        });
                }
                TouchCommand::ToggleChrome => {
                    self.pending_commands
                        .push(NativeVideoTouchCommand::ToggleChrome);
                }
                command if native_touch_command_toggles_chrome_without_video_gestures(command) => {
                    self.pending_commands
                        .push(NativeVideoTouchCommand::ToggleChrome);
                }
                _ => {}
            }
        }
    }

    fn press_primary_at(&mut self, pos: egui::Pos2, events: &mut Vec<egui::Event>) {
        events.push(egui::Event::PointerMoved(pos));
        events.push(primary_button_event(pos, true));
        self.primary_pressed = true;
    }

    /// Completes a synthetic primary click and ends its pointer stream.
    ///
    /// Native touch has no later mouse-leave event that can terminate the
    /// emulated pointer. Leaving the last touch position live would turn the
    /// distance to the next tap into egui's per-pass pointer delta, which a
    /// ScrollArea can consume as a drag on that next press.
    fn release_primary_at(&mut self, pos: egui::Pos2, events: &mut Vec<egui::Event>) {
        debug_assert!(self.primary_pressed);
        events.push(primary_button_event(pos, false));
        events.push(egui::Event::PointerGone);
        self.primary_pressed = false;
    }

    /// Cancels a synthetic primary press without completing a click.
    ///
    /// `egui::Event::PointerGone` only clears the latest pointer position; it
    /// deliberately does not release `PointerState::down`. Therefore every
    /// live synthetic press must first move beyond egui's click-distance
    /// threshold and then emit a real button-up before `PointerGone`.
    fn cancel_primary_press(
        &mut self,
        fallback_pos: egui::Pos2,
        events: &mut Vec<egui::Event>,
    ) -> bool {
        if !self.primary_pressed {
            return false;
        }
        let press_pos = self.primary_start_pos.unwrap_or(fallback_pos);
        let cancel_pos = press_pos + egui::vec2(PRIMARY_CANCEL_DISTANCE_POINTS, 0.0);
        events.push(egui::Event::PointerMoved(cancel_pos));
        self.release_primary_at(cancel_pos, events);
        true
    }

    /// Commands are destructive-read output. `egui::Context::run` may invoke
    /// its UI closure for multiple passes, but later passes cannot replay a
    /// command already taken here.
    pub(crate) fn take_commands(&mut self) -> Vec<NativeVideoTouchCommand> {
        std::mem::take(&mut self.pending_commands)
    }

    pub(crate) fn first_run_help_visible(&self) -> bool {
        self.video_gestures_enabled && self.first_run_help_phase.is_some()
    }

    pub(crate) fn chrome_latched(&self) -> bool {
        self.chrome_latched
    }

    pub(crate) fn toggle_chrome(&mut self) {
        self.chrome_latched = !self.chrome_latched;
    }

    pub(crate) fn show_chrome(&mut self) {
        self.chrome_latched = true;
    }

    pub(crate) fn reset_for_source_swap(&mut self) -> NativeTouchAdapterReset {
        let changed = self.chrome_latched
            || self.primary_id.is_some()
            || self.primary_pressed
            || self.recognizer.is_active()
            || !self.pending_commands.is_empty()
            || self.first_run_help_phase.is_some();
        let mut egui_events = Vec::new();
        self.cancel_primary_press(
            self.primary_start_pos.unwrap_or(egui::Pos2::ZERO),
            &mut egui_events,
        );
        *self = Self::default();
        NativeTouchAdapterReset {
            changed,
            egui_events,
        }
    }
}

fn primary_button_event(pos: egui::Pos2, pressed: bool) -> egui::Event {
    egui::Event::PointerButton {
        pos,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: egui::Modifiers::NONE,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn geometry() -> TapZoneGeometry {
        TapZoneGeometry {
            surface: egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1000.0, 800.0)),
            excluded: Vec::new(),
            behavior: crate::touch_input::TouchSurfaceBehavior::Viewer {
                accepts_pinch: false,
                tap_zones: true,
            },
        }
    }

    fn panorama_geometry() -> TapZoneGeometry {
        TapZoneGeometry {
            behavior: crate::touch_input::TouchSurfaceBehavior::Viewer {
                accepts_pinch: true,
                tap_zones: true,
            },
            ..geometry()
        }
    }

    fn event(
        pointer_id: u32,
        x: i32,
        y: i32,
        phase: NativeVideoTouchPhase,
    ) -> NativeVideoTouchEvent {
        NativeVideoTouchEvent {
            source: NativeVideoWindowSource::Presenter,
            pointer_id,
            x,
            y,
            phase,
            suppress_widget_primary: false,
        }
    }

    fn activation_event(
        pointer_id: u32,
        x: i32,
        y: i32,
        phase: NativeVideoTouchPhase,
    ) -> NativeVideoTouchEvent {
        NativeVideoTouchEvent {
            suppress_widget_primary: true,
            ..event(pointer_id, x, y, phase)
        }
    }

    fn hud_event(
        pointer_id: u32,
        x: i32,
        y: i32,
        phase: NativeVideoTouchPhase,
    ) -> NativeVideoTouchEvent {
        NativeVideoTouchEvent {
            source: NativeVideoWindowSource::Hud,
            ..event(pointer_id, x, y, phase)
        }
    }

    fn has_primary_button(events: &[egui::Event], pressed: bool) -> bool {
        events.iter().any(|event| {
            matches!(
                event,
                egui::Event::PointerButton {
                    button: egui::PointerButton::Primary,
                    pressed: actual,
                    ..
                } if *actual == pressed
            )
        })
    }

    fn pointer_delta_after_events(ctx: &egui::Context, events: Vec<egui::Event>) -> egui::Vec2 {
        let mut delta = None;
        let _ = ctx.run(
            egui::RawInput {
                events,
                ..Default::default()
            },
            |ctx| {
                if delta.is_none() {
                    delta = Some(ctx.input(|input| input.pointer.delta()));
                }
            },
        );
        delta.unwrap()
    }

    fn tap(
        adapter: &mut NativeTouchAdapter,
        event_factory: impl Fn(u32, i32, i32, NativeVideoTouchPhase) -> NativeVideoTouchEvent,
        pointer_id: u32,
        x: i32,
        y: i32,
        start_ms: u64,
    ) -> Vec<NativeVideoTouchCommand> {
        adapter.handle_event(
            event_factory(pointer_id, x, y, NativeVideoTouchPhase::Start),
            &geometry(),
            start_ms,
            1.0,
        );
        adapter.handle_event(
            event_factory(pointer_id, x, y, NativeVideoTouchPhase::End),
            &geometry(),
            start_ms + 50,
            1.0,
        );
        adapter.take_commands()
    }

    #[test]
    fn ownership_claims_down_and_releases_on_up() {
        let mut state = NativeTouchOwnership::default();
        assert_eq!(
            state.begin(10, true, NativePointerTypeProbe::Touch),
            NativeTouchOwnershipDecision::Owned
        );
        assert_eq!(state.followup(10), NativeTouchOwnershipDecision::Owned);
        assert!(state.release(10));
        assert_eq!(
            state.followup(10),
            NativeTouchOwnershipDecision::Passed(NativeTouchPassReason::UnregisteredPointerId)
        );
    }

    #[test]
    fn cancel_and_capture_changed_release_ownership() {
        let mut state = NativeTouchOwnership::default();
        state.begin(1, true, NativePointerTypeProbe::Touch);
        assert!(state.release(1)); // POINTER_FLAG_CANCELED
        state.begin(2, true, NativePointerTypeProbe::Touch);
        assert!(state.release(2)); // WM_POINTERCAPTURECHANGED
        assert_eq!(state.len(), 0);
    }

    #[test]
    fn non_touch_failed_probe_and_unseen_update_are_passed() {
        let mut state = NativeTouchOwnership::default();
        assert_eq!(
            state.begin(1, true, NativePointerTypeProbe::NonTouch),
            NativeTouchOwnershipDecision::Passed(NativeTouchPassReason::NonTouch)
        );
        assert_eq!(
            state.begin(2, true, NativePointerTypeProbe::Failed),
            NativeTouchOwnershipDecision::Passed(NativeTouchPassReason::PointerTypeQueryFailed)
        );
        assert_eq!(
            state.followup(3),
            NativeTouchOwnershipDecision::Passed(NativeTouchPassReason::UnregisteredPointerId)
        );
        assert_eq!(state.len(), 0);
    }

    #[test]
    fn interleaved_streams_are_independent() {
        let mut state = NativeTouchOwnership::default();
        state.begin(7, true, NativePointerTypeProbe::Touch);
        state.begin(42, true, NativePointerTypeProbe::Touch);
        assert_eq!(state.followup(7), NativeTouchOwnershipDecision::Owned);
        assert_eq!(state.followup(42), NativeTouchOwnershipDecision::Owned);
        state.release(7);
        assert_eq!(state.followup(42), NativeTouchOwnershipDecision::Owned);
        state.release(42);
        assert_eq!(state.len(), 0);
    }

    #[test]
    fn followup_does_not_cross_per_hwnd_ownership_sets() {
        let mut presenter = NativeTouchOwnership::default();
        let hud = NativeTouchOwnership::default();
        presenter.begin(77, true, NativePointerTypeProbe::Touch);

        assert_eq!(presenter.followup(77), NativeTouchOwnershipDecision::Owned);
        assert_eq!(
            hud.followup(77),
            NativeTouchOwnershipDecision::Passed(NativeTouchPassReason::UnregisteredPointerId)
        );
    }

    #[test]
    fn ownership_has_a_hard_capacity() {
        let mut state = NativeTouchOwnership::with_capacity(2);
        state.begin(1, true, NativePointerTypeProbe::Touch);
        state.begin(2, true, NativePointerTypeProbe::Touch);
        assert_eq!(
            state.begin(3, true, NativePointerTypeProbe::Touch),
            NativeTouchOwnershipDecision::Passed(NativeTouchPassReason::CapacityExceeded)
        );
        assert_eq!(state.len(), 2);
    }

    #[test]
    fn kill_switch_claims_no_touch_streams() {
        let mut state = NativeTouchOwnership::default();
        assert_eq!(
            state.begin(4, false, NativePointerTypeProbe::Touch),
            NativeTouchOwnershipDecision::Passed(NativeTouchPassReason::Disabled)
        );
        assert_eq!(state.len(), 0);
    }

    #[test]
    fn activation_tap_and_window_destroy_clear_are_per_hwnd_state() {
        let mut state = NativeTouchOwnership::default();
        state.begin(11, true, NativePointerTypeProbe::Touch);
        state.mark_suppress_widget_primary(11);
        assert!(state.suppresses_widget_primary(11));
        assert!(state.has_suppressed_widget_stream());
        state.begin(12, true, NativePointerTypeProbe::Touch);
        state.mark_suppress_widget_primary(12);
        state.release(11);
        assert!(state.has_suppressed_widget_stream());
        state.release(12);
        assert!(!state.has_suppressed_widget_stream());
        state.begin(13, true, NativePointerTypeProbe::Touch);
        state.clear();
        assert_eq!(state.len(), 0);
    }

    #[test]
    fn promoted_mouse_filter_is_fail_open() {
        assert!(native_touch_mouse_discard_decision(true, true, true));
        assert!(!native_touch_mouse_discard_decision(false, true, true));
        assert!(!native_touch_mouse_discard_decision(true, false, true));
        assert!(!native_touch_mouse_discard_decision(true, true, false));
    }

    #[test]
    fn activation_tap_matches_existing_child_and_popup_activation_policy() {
        assert!(!native_touch_is_activation_tap(true, true, false));
        assert!(native_touch_is_activation_tap(true, false, false));
        assert!(!native_touch_is_activation_tap(false, true, true));
        assert!(native_touch_is_activation_tap(false, true, false));
        assert!(native_touch_is_activation_tap(false, false, false));
    }

    #[test]
    fn focus_claim_is_limited_to_first_stream_and_missing_activation_or_focus() {
        assert!(!native_touch_should_request_focus_claim(
            true, true, true, false, true,
        ));
        assert!(native_touch_should_request_focus_claim(
            true, true, true, false, false,
        ));
        assert!(native_touch_should_request_focus_claim(
            true, true, false, false, false,
        ));
        assert!(!native_touch_should_request_focus_claim(
            true, false, true, true, true,
        ));
        assert!(native_touch_should_request_focus_claim(
            true, false, true, false, false,
        ));
        assert!(!native_touch_should_request_focus_claim(
            false, false, false, false, false,
        ));
    }

    #[test]
    fn cancelled_flag_wins_over_update_and_up_phase() {
        assert_eq!(
            native_touch_followup_phase(true, false),
            NativeVideoTouchPhase::Cancel
        );
        assert_eq!(
            native_touch_followup_phase(true, true),
            NativeVideoTouchPhase::Cancel
        );
        assert_eq!(
            native_touch_followup_phase(false, true),
            NativeVideoTouchPhase::End
        );
        assert_eq!(
            native_touch_followup_phase(false, false),
            NativeVideoTouchPhase::Move
        );
    }

    #[test]
    fn screen_client_point_conversion_covers_common_dpi_scales() {
        let screen = [550, 390];
        let origin = [100, 90];
        for (ppp, expected) in [
            (1.0, egui::pos2(450.0, 300.0)),
            (1.5, egui::pos2(300.0, 200.0)),
            (2.0, egui::pos2(225.0, 150.0)),
        ] {
            assert_eq!(
                native_pointer_screen_pixels_to_points(screen, origin, ppp),
                expected
            );
        }
    }

    #[test]
    fn center_and_audio_only_side_commands_map_to_chrome_toggle() {
        assert!(native_touch_command_toggles_chrome_without_video_gestures(
            TouchCommand::ToggleChrome
        ));
        assert!(native_touch_command_toggles_chrome_without_video_gestures(
            TouchCommand::PageSide { left: true }
        ));
    }

    #[test]
    fn viewer_tap_suppresses_primary_press_and_release() {
        let mut adapter = NativeTouchAdapter::default();
        let start = adapter.handle_event(
            event(1, 500, 400, NativeVideoTouchPhase::Start),
            &geometry(),
            0,
            1.0,
        );
        let end = adapter.handle_event(
            event(1, 500, 400, NativeVideoTouchPhase::End),
            &geometry(),
            100,
            1.0,
        );
        assert!(!has_primary_button(&start.egui_events, true));
        assert!(!has_primary_button(&end.egui_events, false));
        assert_eq!(
            adapter.take_commands(),
            vec![NativeVideoTouchCommand::ToggleChrome]
        );
        assert!(adapter.take_commands().is_empty());
    }

    #[test]
    fn activation_tap_still_reveals_chrome() {
        let mut adapter = NativeTouchAdapter::default();
        adapter.handle_event(
            activation_event(1, 500, 400, NativeVideoTouchPhase::Start),
            &geometry(),
            0,
            1.0,
        );
        adapter.handle_event(
            activation_event(1, 500, 400, NativeVideoTouchPhase::End),
            &geometry(),
            100,
            1.0,
        );
        assert_eq!(
            adapter.take_commands(),
            vec![NativeVideoTouchCommand::ToggleChrome]
        );
    }

    #[test]
    fn activation_tap_never_reaches_an_overlay_control() {
        let mut geometry = geometry();
        geometry.excluded.push(egui::Rect::from_min_max(
            egui::Pos2::ZERO,
            egui::pos2(200.0, 100.0),
        ));
        let mut adapter = NativeTouchAdapter::default();
        let start = adapter.handle_event(
            activation_event(1, 100, 50, NativeVideoTouchPhase::Start),
            &geometry,
            0,
            1.0,
        );
        let end = adapter.handle_event(
            activation_event(1, 100, 50, NativeVideoTouchPhase::End),
            &geometry,
            100,
            1.0,
        );
        assert!(!has_primary_button(&start.egui_events, true));
        assert!(!has_primary_button(&end.egui_events, false));
    }

    #[test]
    fn press_withholding_does_not_leak_into_the_next_gesture() {
        let mut geometry = geometry();
        geometry.excluded.push(egui::Rect::from_min_max(
            egui::Pos2::ZERO,
            egui::pos2(200.0, 100.0),
        ));
        let mut adapter = NativeTouchAdapter::default();
        adapter.handle_event(
            activation_event(1, 100, 50, NativeVideoTouchPhase::Start),
            &geometry,
            0,
            1.0,
        );
        adapter.handle_event(
            activation_event(1, 100, 50, NativeVideoTouchPhase::End),
            &geometry,
            100,
            1.0,
        );
        let start = adapter.handle_event(
            event(2, 100, 50, NativeVideoTouchPhase::Start),
            &geometry,
            200,
            1.0,
        );
        assert!(has_primary_button(&start.egui_events, true));
    }

    #[test]
    fn widget_passthrough_keeps_primary_press_and_release() {
        let mut geometry = geometry();
        geometry.excluded.push(egui::Rect::from_min_max(
            egui::Pos2::ZERO,
            egui::pos2(200.0, 100.0),
        ));
        let mut adapter = NativeTouchAdapter::default();
        let start = adapter.handle_event(
            event(1, 100, 50, NativeVideoTouchPhase::Start),
            &geometry,
            0,
            1.0,
        );
        let end = adapter.handle_event(
            event(1, 100, 50, NativeVideoTouchPhase::End),
            &geometry,
            100,
            1.0,
        );
        assert!(has_primary_button(&start.egui_events, true));
        assert!(has_primary_button(&end.egui_events, false));
        let released = end
            .egui_events
            .iter()
            .position(|event| has_primary_button(std::slice::from_ref(event), false))
            .unwrap();
        let gone = end
            .egui_events
            .iter()
            .position(|event| matches!(event, egui::Event::PointerGone))
            .unwrap();
        assert!(released < gone);
        assert!(adapter.take_commands().is_empty());
    }

    #[test]
    fn completed_hud_tap_clears_pointer_position_before_next_tap() {
        let mut adapter = NativeTouchAdapter::default();
        let ctx = egui::Context::default();

        let first_start = adapter.handle_event(
            hud_event(1, 500, 100, NativeVideoTouchPhase::Start),
            &geometry(),
            0,
            1.0,
        );
        assert_eq!(
            pointer_delta_after_events(&ctx, first_start.egui_events),
            egui::Vec2::ZERO
        );
        let first_end = adapter.handle_event(
            hud_event(1, 500, 100, NativeVideoTouchPhase::End),
            &geometry(),
            50,
            1.0,
        );
        let _ = pointer_delta_after_events(&ctx, first_end.egui_events);

        let second_start = adapter.handle_event(
            hud_event(2, 500, 700, NativeVideoTouchPhase::Start),
            &geometry(),
            100,
            1.0,
        );
        assert_eq!(
            pointer_delta_after_events(&ctx, second_start.egui_events),
            egui::Vec2::ZERO
        );
    }

    #[test]
    fn hud_origin_is_widget_passthrough_without_presenter_exclusion_geometry() {
        let mut adapter = NativeTouchAdapter::default();
        let start = adapter.handle_event(
            hud_event(1, 500, 400, NativeVideoTouchPhase::Start),
            &geometry(),
            0,
            1.0,
        );
        let end = adapter.handle_event(
            hud_event(1, 500, 400, NativeVideoTouchPhase::End),
            &geometry(),
            100,
            1.0,
        );

        assert!(has_primary_button(&start.egui_events, true));
        assert!(has_primary_button(&end.egui_events, false));
        assert!(adapter.take_commands().is_empty());
    }

    #[test]
    fn simultaneous_hud_and_presenter_contacts_have_one_primary_owner() {
        let mut adapter = NativeTouchAdapter::default();
        let outputs = [
            (hud_event(1, 500, 400, NativeVideoTouchPhase::Start), 0),
            (event(2, 600, 400, NativeVideoTouchPhase::Start), 10),
            (event(2, 600, 400, NativeVideoTouchPhase::End), 20),
            (hud_event(1, 500, 400, NativeVideoTouchPhase::End), 30),
        ]
        .into_iter()
        .map(|(event, now_ms)| adapter.handle_event(event, &geometry(), now_ms, 1.0))
        .collect::<Vec<_>>();

        let presses = outputs
            .iter()
            .flat_map(|output| output.egui_events.iter())
            .filter(|event| {
                matches!(
                    event,
                    egui::Event::PointerButton {
                        button: egui::PointerButton::Primary,
                        pressed: true,
                        ..
                    }
                )
            })
            .count();
        let releases = outputs
            .iter()
            .flat_map(|output| output.egui_events.iter())
            .filter(|event| {
                matches!(
                    event,
                    egui::Event::PointerButton {
                        button: egui::PointerButton::Primary,
                        pressed: false,
                        ..
                    }
                )
            })
            .count();
        assert_eq!(presses, 1);
        assert_eq!(releases, 1);
        assert!(adapter.take_commands().is_empty());
    }

    #[test]
    fn hud_capture_loss_cancel_releases_primary_before_pointer_gone() {
        let mut adapter = NativeTouchAdapter::default();
        let start = adapter.handle_event(
            hud_event(1, 500, 400, NativeVideoTouchPhase::Start),
            &geometry(),
            0,
            1.0,
        );
        assert!(has_primary_button(&start.egui_events, true));

        let cancel = adapter.handle_event(
            hud_event(1, 500, 400, NativeVideoTouchPhase::Cancel),
            &geometry(),
            10,
            1.0,
        );
        let moved = cancel
            .egui_events
            .iter()
            .position(|event| matches!(event, egui::Event::PointerMoved(pos) if pos.x >= 500.0 + PRIMARY_CANCEL_DISTANCE_POINTS))
            .expect("capture loss must move beyond click distance");
        let released = cancel
            .egui_events
            .iter()
            .position(|event| has_primary_button(std::slice::from_ref(event), false))
            .expect("capture loss must release the synthetic primary");
        let gone = cancel
            .egui_events
            .iter()
            .position(|event| matches!(event, egui::Event::PointerGone))
            .expect("capture loss must end with PointerGone");
        assert!(moved < released && released < gone);
    }

    #[test]
    fn cancel_releases_live_primary_after_click_cancelling_move() {
        let mut geometry = geometry();
        geometry.excluded.push(egui::Rect::from_min_max(
            egui::Pos2::ZERO,
            egui::pos2(200.0, 100.0),
        ));
        let mut adapter = NativeTouchAdapter::default();
        let start = adapter.handle_event(
            event(1, 100, 50, NativeVideoTouchPhase::Start),
            &geometry,
            0,
            1.0,
        );
        assert!(has_primary_button(&start.egui_events, true));

        let cancel = adapter.handle_event(
            event(1, 100, 50, NativeVideoTouchPhase::Cancel),
            &geometry,
            10,
            1.0,
        );
        let moved = cancel
            .egui_events
            .iter()
            .position(|event| matches!(event, egui::Event::PointerMoved(pos) if pos.x >= 100.0 + PRIMARY_CANCEL_DISTANCE_POINTS))
            .expect("cancel must move beyond click distance");
        let released = cancel
            .egui_events
            .iter()
            .position(|event| {
                matches!(
                    event,
                    egui::Event::PointerButton {
                        button: egui::PointerButton::Primary,
                        pressed: false,
                        ..
                    }
                )
            })
            .expect("cancel must release the synthetic primary");
        let gone = cancel
            .egui_events
            .iter()
            .position(|event| matches!(event, egui::Event::PointerGone))
            .expect("cancel must end with PointerGone");
        assert!(moved < released);
        assert!(released < gone);
    }

    #[test]
    fn side_taps_seek_once_without_changing_chrome_latch() {
        for chrome_visible in [false, true] {
            for (x, delta_secs) in [(100, -VIDEO_TAP_SEEK_SECS), (900, VIDEO_TAP_SEEK_SECS)] {
                let mut adapter = NativeTouchAdapter::default();
                if chrome_visible {
                    adapter.show_chrome();
                }

                assert_eq!(
                    tap(&mut adapter, event, 1, x, 400, 0),
                    vec![NativeVideoTouchCommand::SeekRelative { delta_secs }]
                );
                assert_eq!(adapter.chrome_latched(), chrome_visible);
            }
        }
    }

    #[test]
    fn every_repeated_side_tap_emits_exactly_one_seek() {
        let mut adapter = NativeTouchAdapter::default();
        for (index, start_ms) in [0, 150, 300, 450].into_iter().enumerate() {
            assert_eq!(
                tap(&mut adapter, event, index as u32 + 1, 900, 400, start_ms),
                vec![NativeVideoTouchCommand::SeekRelative {
                    delta_secs: VIDEO_TAP_SEEK_SECS,
                }]
            );
        }
    }

    #[test]
    fn panorama_drag_latches_and_applies_the_first_full_motion() {
        let mut adapter = NativeTouchAdapter::default();
        adapter.configure_panorama(true);
        let geometry = panorama_geometry();
        adapter.handle_event(
            event(1, 100, 400, NativeVideoTouchPhase::Start),
            &geometry,
            0,
            1.0,
        );
        adapter.handle_event(
            event(1, 113, 400, NativeVideoTouchPhase::Move),
            &geometry,
            10,
            1.0,
        );
        assert_eq!(
            adapter.take_commands(),
            vec![NativeVideoTouchCommand::PanoramaDrag {
                delta_points: egui::vec2(13.0, 0.0),
                viewport_height_points: 800.0,
            }],
            "the ownership frame must include motion from DOWN"
        );

        adapter.handle_event(
            event(1, 100, 400, NativeVideoTouchPhase::Move),
            &geometry,
            20,
            1.0,
        );
        assert_eq!(
            adapter.take_commands(),
            vec![NativeVideoTouchCommand::PanoramaDrag {
                delta_points: egui::vec2(-13.0, 0.0),
                viewport_height_points: 800.0,
            }]
        );
        adapter.handle_event(
            event(1, 100, 400, NativeVideoTouchPhase::End),
            &geometry,
            30,
            1.0,
        );
        assert!(
            adapter.take_commands().is_empty(),
            "returning to the start must not turn the latched drag into a tap"
        );
    }

    #[test]
    fn panorama_second_contact_cancels_the_pending_tap() {
        let mut adapter = NativeTouchAdapter::default();
        adapter.configure_panorama(true);
        let geometry = panorama_geometry();
        adapter.handle_event(
            event(1, 100, 400, NativeVideoTouchPhase::Start),
            &geometry,
            0,
            1.0,
        );
        adapter.handle_event(
            event(2, 200, 400, NativeVideoTouchPhase::Start),
            &geometry,
            10,
            1.0,
        );
        adapter.handle_event(
            event(2, 200, 400, NativeVideoTouchPhase::End),
            &geometry,
            20,
            1.0,
        );
        adapter.handle_event(
            event(1, 100, 400, NativeVideoTouchPhase::End),
            &geometry,
            30,
            1.0,
        );
        assert!(
            adapter.take_commands().is_empty(),
            "a second contact must cancel both the pending tap and panorama drag"
        );
    }

    #[test]
    fn center_and_hud_taps_never_seek() {
        let mut center = NativeTouchAdapter::default();
        let center_commands = [0, 150, 300]
            .into_iter()
            .enumerate()
            .flat_map(|(index, start_ms)| {
                tap(&mut center, event, index as u32 + 1, 500, 400, start_ms)
            })
            .collect::<Vec<_>>();
        assert_eq!(
            center_commands,
            vec![
                NativeVideoTouchCommand::ToggleChrome,
                NativeVideoTouchCommand::ToggleChrome,
                NativeVideoTouchCommand::ToggleChrome,
            ]
        );

        let mut hud = NativeTouchAdapter::default();
        let hud_commands = [0, 150, 300]
            .into_iter()
            .enumerate()
            .flat_map(|(index, start_ms)| {
                tap(&mut hud, hud_event, index as u32 + 1, 100, 400, start_ms)
            })
            .collect::<Vec<_>>();
        assert!(hud_commands.is_empty());
    }

    #[test]
    fn audio_only_side_tap_maps_to_chrome_toggle() {
        let mut adapter = NativeTouchAdapter::default();
        adapter.configure_video_gestures(false, None);

        assert_eq!(
            tap(&mut adapter, event, 1, 100, 400, 0),
            vec![NativeVideoTouchCommand::ToggleChrome]
        );
    }

    #[test]
    fn first_run_help_consumes_discovery_then_learns_and_shows_chrome() {
        let mut adapter = NativeTouchAdapter::default();
        adapter.configure_video_gestures(true, Some(false));

        assert!(tap(&mut adapter, event, 1, 100, 400, 0).is_empty());
        assert!(adapter.first_run_help_visible());
        assert_eq!(
            tap(&mut adapter, hud_event, 2, 100, 400, 150),
            vec![NativeVideoTouchCommand::LearnAndShowChrome]
        );
        assert!(!adapter.first_run_help_visible());
        assert_eq!(
            tap(&mut adapter, event, 3, 100, 400, 300),
            vec![NativeVideoTouchCommand::SeekRelative {
                delta_secs: -VIDEO_TAP_SEEK_SECS,
            }]
        );
    }

    #[test]
    fn learned_snapshot_suppresses_first_run_help() {
        let mut adapter = NativeTouchAdapter::default();
        adapter.configure_video_gestures(true, Some(true));
        assert_eq!(
            tap(&mut adapter, event, 1, 500, 400, 0),
            vec![NativeVideoTouchCommand::ToggleChrome]
        );
        assert!(!adapter.first_run_help_visible());
    }

    #[test]
    fn still_help_learning_does_not_suppress_video_help() {
        let mut settings = crate::settings::Settings {
            touch_still_chrome_learned: true,
            ..Default::default()
        };
        assert!(!settings.touch_video_chrome_learned);

        let mut adapter = NativeTouchAdapter::default();
        adapter.configure_video_gestures(true, Some(settings.touch_video_chrome_learned));
        assert!(tap(&mut adapter, event, 1, 100, 400, 0).is_empty());
        assert!(adapter.first_run_help_visible());

        settings.touch_video_chrome_learned = true;
        adapter.configure_video_gestures(true, Some(settings.touch_video_chrome_learned));
        assert!(!adapter.first_run_help_visible());
    }

    #[test]
    fn physical_left_is_negative_and_physical_right_is_positive_without_reading_direction() {
        // The native adapter consumes physical PageSide values directly. Reading direction is
        // deliberately not an input to this mapping.
        let mut left = NativeTouchAdapter::default();
        assert_eq!(
            tap(&mut left, event, 1, 100, 400, 0),
            vec![NativeVideoTouchCommand::SeekRelative { delta_secs: -5.0 }]
        );

        let mut right = NativeTouchAdapter::default();
        assert_eq!(
            tap(&mut right, event, 1, 900, 400, 0),
            vec![NativeVideoTouchCommand::SeekRelative { delta_secs: 5.0 }]
        );
    }

    #[test]
    fn source_swap_resets_the_actual_native_touch_session() {
        let mut geometry = geometry();
        geometry.excluded.push(egui::Rect::from_min_max(
            egui::Pos2::ZERO,
            egui::pos2(200.0, 100.0),
        ));
        let mut adapter = NativeTouchAdapter::default();
        adapter.toggle_chrome();
        let start = adapter.handle_event(
            event(1, 100, 50, NativeVideoTouchPhase::Start),
            &geometry,
            0,
            1.0,
        );
        assert!(has_primary_button(&start.egui_events, true));
        assert!(adapter.chrome_latched());
        let reset = adapter.reset_for_source_swap();
        assert!(reset.changed);
        assert!(has_primary_button(&reset.egui_events, false));
        assert!(!adapter.chrome_latched());
        assert!(adapter.take_commands().is_empty());
    }
}
