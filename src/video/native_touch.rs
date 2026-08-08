use crate::touch_input::{
    TapZoneGeometry, TouchCommand, TouchOwner, TouchPhase, TouchRecognizer, TouchSample,
};

pub(crate) const MAX_OWNED_TOUCH_POINTERS: usize = 16;
const PRIMARY_CANCEL_DISTANCE_POINTS: f32 = 1024.0;

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
    pub pointer_id: u32,
    /// Presenter client coordinate in physical pixels.
    pub x: i32,
    /// Presenter client coordinate in physical pixels.
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
    activation_tap: bool,
}

/// Pure, bounded ownership state for one presenter HWND.
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
            activation_tap: false,
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

    pub(crate) fn mark_activation_tap(&mut self, pointer_id: u32) {
        if let Some(pointer) = self
            .pointers
            .iter_mut()
            .find(|pointer| pointer.id == pointer_id)
        {
            pointer.activation_tap = true;
        }
    }

    pub(crate) fn is_activation_tap(&self, pointer_id: u32) -> bool {
        self.pointers
            .iter()
            .find(|pointer| pointer.id == pointer_id)
            .is_some_and(|pointer| pointer.activation_tap)
    }

    pub(crate) fn has_activation_tap_stream(&self) -> bool {
        self.pointers.iter().any(|pointer| pointer.activation_tap)
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

pub(crate) fn native_touch_command_toggles_chrome(command: TouchCommand) -> bool {
    matches!(
        command,
        TouchCommand::ToggleChrome | TouchCommand::PageSide { .. }
    )
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

#[derive(Default)]
pub(crate) struct NativeTouchAdapter {
    recognizer: TouchRecognizer,
    primary_id: Option<u32>,
    primary_start_pos: Option<egui::Pos2>,
    primary_pressed: bool,
    /// The primary contact began on an inactive presenter, so no synthetic
    /// press is emitted for it. The gesture still runs.
    primary_withholds_press: bool,
    pending_commands: Vec<TouchCommand>,
    chrome_latched: bool,
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
    pub(crate) fn handle_event(
        &mut self,
        event: NativeVideoTouchEvent,
        geometry: &TapZoneGeometry,
        now_ms: u64,
        pixels_per_point: f32,
    ) -> NativeTouchAdapterOutput {
        let pos = native_client_pixels_to_points([event.x, event.y], pixels_per_point);
        if event.phase == NativeVideoTouchPhase::Start
            && self.primary_id.is_none()
            && !self.recognizer.is_active()
        {
            self.primary_id = Some(event.pointer_id);
            self.primary_start_pos = Some(pos);
            self.primary_pressed = false;
            self.primary_withholds_press = event.suppress_widget_primary;
        }
        let is_primary = self.primary_id == Some(event.pointer_id);
        let commands = self.recognizer.handle_sample(
            geometry,
            TouchSample {
                id: u64::from(event.pointer_id),
                pos,
                phase: event.phase.into(),
                now_ms,
            },
        );
        self.pending_commands.extend(commands);

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
                            egui_events.push(primary_button_event(pos, false));
                            self.primary_pressed = false;
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

    fn press_primary_at(&mut self, pos: egui::Pos2, events: &mut Vec<egui::Event>) {
        events.push(egui::Event::PointerMoved(pos));
        events.push(primary_button_event(pos, true));
        self.primary_pressed = true;
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
        events.push(primary_button_event(cancel_pos, false));
        events.push(egui::Event::PointerGone);
        self.primary_pressed = false;
        true
    }

    /// Commands are destructive-read output. `egui::Context::run` may invoke
    /// its UI closure for multiple passes, but later passes cannot replay a
    /// command already taken here.
    pub(crate) fn take_commands(&mut self) -> Vec<TouchCommand> {
        std::mem::take(&mut self.pending_commands)
    }

    pub(crate) fn chrome_latched(&self) -> bool {
        self.chrome_latched
    }

    pub(crate) fn toggle_chrome(&mut self) {
        self.chrome_latched = !self.chrome_latched;
    }

    pub(crate) fn reset_for_source_swap(&mut self) -> NativeTouchAdapterReset {
        let changed = self.chrome_latched
            || self.primary_id.is_some()
            || self.primary_pressed
            || self.recognizer.is_active()
            || !self.pending_commands.is_empty();
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
                accepts_edge_swipe: true,
            },
        }
    }

    fn event(
        pointer_id: u32,
        x: i32,
        y: i32,
        phase: NativeVideoTouchPhase,
    ) -> NativeVideoTouchEvent {
        NativeVideoTouchEvent {
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
        state.mark_activation_tap(11);
        assert!(state.is_activation_tap(11));
        assert!(state.has_activation_tap_stream());
        state.begin(12, true, NativePointerTypeProbe::Touch);
        state.mark_activation_tap(12);
        state.release(11);
        assert!(state.has_activation_tap_stream());
        state.release(12);
        assert!(!state.has_activation_tap_stream());
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
    fn both_tap_commands_map_to_chrome_toggle() {
        assert!(native_touch_command_toggles_chrome(
            TouchCommand::ToggleChrome
        ));
        assert!(native_touch_command_toggles_chrome(
            TouchCommand::PageSide { left: true }
        ));
        assert!(!native_touch_command_toggles_chrome(
            TouchCommand::OpenSidePanel { left: true }
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
        assert_eq!(adapter.take_commands(), vec![TouchCommand::ToggleChrome]);
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
        assert_eq!(adapter.take_commands(), vec![TouchCommand::ToggleChrome]);
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
        assert!(adapter.take_commands().is_empty());
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
