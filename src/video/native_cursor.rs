//! Pump-owned cursor routing and auto-hide state for native video windows.

use std::time::{Duration, Instant};

use super::native_window::NativeVideoWindowSource;
use super::native_window_host::NativeCursorIcon;

pub(crate) fn cursor_move_is_activity(
    previous: Option<(i32, i32)>,
    position: (i32, i32),
    hidden: bool,
) -> bool {
    previous.map_or(!hidden, |previous| previous != position)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CursorActivity {
    Move([i32; 2]),
    Explicit([i32; 2]),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CursorInputOwnership {
    Unknown,
    Owned,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CursorRoutingState {
    Unknown,
    Presenter,
    Hud,
    CapturedPresenter,
    CapturedHud,
}

impl CursorRoutingState {
    pub(crate) fn input_ownership(self) -> CursorInputOwnership {
        if self == Self::Unknown {
            CursorInputOwnership::Unknown
        } else {
            CursorInputOwnership::Owned
        }
    }

    pub(crate) fn source(self) -> Option<NativeVideoWindowSource> {
        match self {
            Self::Unknown => None,
            Self::Presenter | Self::CapturedPresenter => Some(NativeVideoWindowSource::Presenter),
            Self::Hud | Self::CapturedHud => Some(NativeVideoWindowSource::Hud),
        }
    }

    fn from_source(source: NativeVideoWindowSource, captured: bool) -> Self {
        match (source, captured) {
            (NativeVideoWindowSource::Presenter, false) => Self::Presenter,
            (NativeVideoWindowSource::Hud, false) => Self::Hud,
            (NativeVideoWindowSource::Presenter, true) => Self::CapturedPresenter,
            (NativeVideoWindowSource::Hud, true) => Self::CapturedHud,
        }
    }

    fn is_captured(self) -> bool {
        matches!(self, Self::CapturedPresenter | Self::CapturedHud)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CursorRoutingEventKind {
    Move([i32; 2]),
    Explicit {
        position: [i32; 2],
        establishes_target: bool,
    },
    Leave,
    CaptureLost,
    TrackingFailed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct CursorRoutingEvent {
    pub(crate) sequence: u64,
    pub(crate) source: NativeVideoWindowSource,
    pub(crate) kind: CursorRoutingEventKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct CursorRoutingBatchResult {
    pub(crate) state: CursorRoutingState,
    pub(crate) activity: Option<CursorActivity>,
}

/// Reduces every cursor-related edge drained in one pump turn to one ownership
/// decision. A presenter/HUD leave followed by the other role's zero-delta move
/// therefore never publishes a transient unowned state to the auto-hide reducer.
pub(crate) fn reduce_cursor_routing_batch(
    current: CursorRoutingState,
    events: &[CursorRoutingEvent],
    local_capture: Option<NativeVideoWindowSource>,
) -> CursorRoutingBatchResult {
    let mut routed = current.source().map(|source| (0_u64, source));
    let mut presenter_invalidated = None::<u64>;
    let mut hud_invalidated = None::<u64>;
    let mut latest_position = None::<(u64, [i32; 2])>;
    let mut explicit_activity = false;

    for event in events {
        match event.kind {
            CursorRoutingEventKind::Move(position) => {
                routed = Some((event.sequence, event.source));
                if latest_position.is_none_or(|(sequence, _)| event.sequence >= sequence) {
                    latest_position = Some((event.sequence, position));
                }
            }
            CursorRoutingEventKind::Explicit {
                position,
                establishes_target,
            } => {
                explicit_activity = true;
                if latest_position.is_none_or(|(sequence, _)| event.sequence >= sequence) {
                    latest_position = Some((event.sequence, position));
                }
                if establishes_target {
                    routed = Some((event.sequence, event.source));
                }
            }
            CursorRoutingEventKind::Leave
            | CursorRoutingEventKind::CaptureLost
            | CursorRoutingEventKind::TrackingFailed => {
                let invalidated = match event.source {
                    NativeVideoWindowSource::Presenter => &mut presenter_invalidated,
                    NativeVideoWindowSource::Hud => &mut hud_invalidated,
                };
                *invalidated =
                    Some(invalidated.map_or(event.sequence, |prior| prior.max(event.sequence)));
            }
        }
    }

    let state = if let Some(source) = local_capture {
        CursorRoutingState::from_source(source, true)
    } else if current.is_captured() && routed.is_some_and(|(sequence, _)| sequence == 0) {
        // GetCapture is the pump-thread fact for our own presenter/HUD capture.
        // If it vanished without a newer target message, ownership is unknown.
        CursorRoutingState::Unknown
    } else {
        routed.map_or(CursorRoutingState::Unknown, |(routed_sequence, source)| {
            let invalidated_sequence = match source {
                NativeVideoWindowSource::Presenter => presenter_invalidated,
                NativeVideoWindowSource::Hud => hud_invalidated,
            };
            if invalidated_sequence.is_some_and(|sequence| sequence >= routed_sequence) {
                CursorRoutingState::Unknown
            } else {
                CursorRoutingState::from_source(source, false)
            }
        })
    };

    let activity = latest_position.map(|(_, position)| {
        if explicit_activity {
            CursorActivity::Explicit(position)
        } else {
            CursorActivity::Move(position)
        }
    });

    CursorRoutingBatchResult { state, activity }
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct CursorAutoHideReducer {
    input_owned: CursorInputOwnership,
    last_position: Option<[i32; 2]>,
    last_activity: Option<Instant>,
    hidden: bool,
    desired_icon: NativeCursorIcon,
    auto_hide_allowed: bool,
    hide_delay: Duration,
    applied_icon: Option<NativeCursorIcon>,
}

impl CursorAutoHideReducer {
    pub(crate) fn new(hide_delay_secs: f32) -> Self {
        let clamped = crate::settings::clamp_fullscreen_cursor_hide_delay_secs(hide_delay_secs);
        Self {
            input_owned: CursorInputOwnership::Unknown,
            last_position: None,
            last_activity: None,
            hidden: false,
            desired_icon: NativeCursorIcon::Arrow,
            auto_hide_allowed: false,
            hide_delay: Duration::from_secs_f32(clamped),
            applied_icon: None,
        }
    }

    pub(crate) fn apply_input_batch(
        &mut self,
        input_owned: CursorInputOwnership,
        activity: Option<CursorActivity>,
        now: Instant,
    ) -> Option<NativeCursorIcon> {
        let ownership_changed = self.input_owned != input_owned;
        self.input_owned = input_owned;
        if input_owned == CursorInputOwnership::Unknown {
            if let Some(activity) = activity {
                self.last_position = Some(activity.position());
            }
            self.last_activity = Some(now);
            self.hidden = false;
            self.applied_icon = None;
            // Once ownership is lost, an external window's WM_SETCURSOR owns the
            // icon. Never overwrite its custom cursor with IDC_ARROW here.
            return None;
        }

        if ownership_changed {
            self.last_activity = Some(now);
            self.hidden = false;
            self.applied_icon = None;
        }

        let active = match activity {
            Some(CursorActivity::Move(position)) => {
                let active = cursor_move_is_activity(
                    self.last_position
                        .map(|position| (position[0], position[1])),
                    (position[0], position[1]),
                    self.hidden,
                );
                self.last_position = Some(position);
                active
            }
            Some(CursorActivity::Explicit(position)) => {
                self.last_position = Some(position);
                true
            }
            None => false,
        };
        if active {
            self.last_activity = Some(now);
            self.hidden = false;
            self.applied_icon = None;
        }
        self.resolve(now, ownership_changed || active)
    }

    pub(crate) fn record_external_activity(&mut self, now: Instant) -> Option<NativeCursorIcon> {
        self.last_activity = Some(now);
        self.hidden = false;
        self.applied_icon = None;
        self.resolve(now, true)
    }

    pub(crate) fn set_render_policy(
        &mut self,
        desired_icon: NativeCursorIcon,
        auto_hide_allowed: bool,
        now: Instant,
    ) -> Option<NativeCursorIcon> {
        let changed =
            self.desired_icon != desired_icon || self.auto_hide_allowed != auto_hide_allowed;
        if !auto_hide_allowed {
            // Keep the countdown parked while interactive UI remains visible,
            // without invalidating the applied icon or calling SetCursor again.
            self.last_activity = Some(now);
            self.hidden = false;
        }
        if !changed {
            return None;
        }
        self.desired_icon = desired_icon;
        self.auto_hide_allowed = auto_hide_allowed;
        self.applied_icon = None;
        self.resolve(now, true)
    }

    pub(crate) fn reset_for_transition(&mut self, now: Instant) -> Option<NativeCursorIcon> {
        self.input_owned = CursorInputOwnership::Unknown;
        self.last_position = None;
        self.last_activity = Some(now);
        self.hidden = false;
        self.desired_icon = NativeCursorIcon::Arrow;
        self.auto_hide_allowed = false;
        self.applied_icon = None;
        None
    }

    pub(crate) fn tick(&mut self, now: Instant) -> Option<NativeCursorIcon> {
        self.resolve(now, false)
    }

    pub(crate) fn input_owned(self) -> bool {
        self.input_owned == CursorInputOwnership::Owned
    }

    #[cfg(test)]
    pub(crate) fn input_ownership(self) -> CursorInputOwnership {
        self.input_owned
    }

    pub(crate) fn hidden(self) -> bool {
        self.hidden
    }

    pub(crate) fn last_activity(self) -> Option<Instant> {
        self.last_activity
    }

    fn resolve(&mut self, now: Instant, force: bool) -> Option<NativeCursorIcon> {
        if self.input_owned != CursorInputOwnership::Owned {
            return None;
        }
        let timed_out = self.auto_hide_allowed
            && self
                .last_activity
                .is_some_and(|last| now.saturating_duration_since(last) >= self.hide_delay);
        let icon = if self.desired_icon == NativeCursorIcon::Hidden || timed_out {
            NativeCursorIcon::Hidden
        } else {
            self.desired_icon
        };
        self.hidden = icon == NativeCursorIcon::Hidden;
        if force || self.applied_icon != Some(icon) {
            self.applied_icon = Some(icon);
            Some(icon)
        } else {
            None
        }
    }
}

impl CursorActivity {
    fn position(self) -> [i32; 2] {
        match self {
            Self::Move(position) | Self::Explicit(position) => position,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(
        sequence: u64,
        source: NativeVideoWindowSource,
        kind: CursorRoutingEventKind,
    ) -> CursorRoutingEvent {
        CursorRoutingEvent {
            sequence,
            source,
            kind,
        }
    }

    fn hidden_reducer(now: Instant) -> CursorAutoHideReducer {
        let mut reducer = CursorAutoHideReducer::new(0.1);
        assert_eq!(
            reducer.apply_input_batch(
                CursorInputOwnership::Owned,
                Some(CursorActivity::Move([50, 50])),
                now,
            ),
            Some(NativeCursorIcon::Arrow)
        );
        assert_eq!(
            reducer.set_render_policy(NativeCursorIcon::Arrow, true, now),
            Some(NativeCursorIcon::Arrow)
        );
        assert_eq!(
            reducer.tick(now + Duration::from_millis(150)),
            Some(NativeCursorIcon::Hidden)
        );
        reducer
    }

    #[test]
    fn presenter_to_hud_handoff_preserves_hidden_cursor_and_activity_clock() {
        let now = Instant::now();
        for events in [
            [
                event(
                    1,
                    NativeVideoWindowSource::Presenter,
                    CursorRoutingEventKind::Leave,
                ),
                event(
                    2,
                    NativeVideoWindowSource::Hud,
                    CursorRoutingEventKind::Move([50, 50]),
                ),
            ],
            [
                event(
                    1,
                    NativeVideoWindowSource::Hud,
                    CursorRoutingEventKind::Move([50, 50]),
                ),
                event(
                    2,
                    NativeVideoWindowSource::Presenter,
                    CursorRoutingEventKind::Leave,
                ),
            ],
        ] {
            let mut reducer = hidden_reducer(now);
            let last_activity = reducer.last_activity();
            let batch = reduce_cursor_routing_batch(CursorRoutingState::Presenter, &events, None);
            assert_eq!(batch.state, CursorRoutingState::Hud);
            assert_eq!(
                reducer.apply_input_batch(
                    batch.state.input_ownership(),
                    batch.activity,
                    now + Duration::from_millis(151),
                ),
                None
            );
            assert!(reducer.hidden());
            assert_eq!(reducer.last_activity(), last_activity);
        }
    }

    #[test]
    fn hud_to_presenter_handoff_preserves_hidden_cursor_and_activity_clock() {
        let now = Instant::now();
        for events in [
            [
                event(
                    1,
                    NativeVideoWindowSource::Hud,
                    CursorRoutingEventKind::Leave,
                ),
                event(
                    2,
                    NativeVideoWindowSource::Presenter,
                    CursorRoutingEventKind::Move([50, 50]),
                ),
            ],
            [
                event(
                    1,
                    NativeVideoWindowSource::Presenter,
                    CursorRoutingEventKind::Move([50, 50]),
                ),
                event(
                    2,
                    NativeVideoWindowSource::Hud,
                    CursorRoutingEventKind::Leave,
                ),
            ],
        ] {
            let mut reducer = hidden_reducer(now);
            let last_activity = reducer.last_activity();
            let batch = reduce_cursor_routing_batch(CursorRoutingState::Hud, &events, None);
            assert_eq!(batch.state, CursorRoutingState::Presenter);
            assert_eq!(
                reducer.apply_input_batch(
                    batch.state.input_ownership(),
                    batch.activity,
                    now + Duration::from_millis(151),
                ),
                None
            );
            assert!(reducer.hidden());
            assert_eq!(reducer.last_activity(), last_activity);
        }
    }

    #[test]
    fn captured_presenter_and_hud_own_out_of_bounds_drag() {
        for (current, source, expected) in [
            (
                CursorRoutingState::Presenter,
                NativeVideoWindowSource::Presenter,
                CursorRoutingState::CapturedPresenter,
            ),
            (
                CursorRoutingState::Hud,
                NativeVideoWindowSource::Hud,
                CursorRoutingState::CapturedHud,
            ),
        ] {
            let batch = reduce_cursor_routing_batch(
                current,
                &[event(1, source, CursorRoutingEventKind::Move([-500, 900]))],
                Some(source),
            );
            assert_eq!(batch.state, expected);
            assert_eq!(batch.state.input_ownership(), CursorInputOwnership::Owned);
        }
    }

    #[test]
    fn self_release_capture_returns_to_unknown_even_when_button_up_is_in_same_drain() {
        for events in [
            [
                event(
                    1,
                    NativeVideoWindowSource::Presenter,
                    CursorRoutingEventKind::CaptureLost,
                ),
                event(
                    2,
                    NativeVideoWindowSource::Presenter,
                    CursorRoutingEventKind::Explicit {
                        position: [50, 50],
                        establishes_target: false,
                    },
                ),
            ],
            [
                event(
                    1,
                    NativeVideoWindowSource::Presenter,
                    CursorRoutingEventKind::Explicit {
                        position: [50, 50],
                        establishes_target: false,
                    },
                ),
                event(
                    2,
                    NativeVideoWindowSource::Presenter,
                    CursorRoutingEventKind::CaptureLost,
                ),
            ],
        ] {
            let batch =
                reduce_cursor_routing_batch(CursorRoutingState::CapturedPresenter, &events, None);
            assert_eq!(batch.state, CursorRoutingState::Unknown);
        }
    }

    #[test]
    fn external_capture_loss_without_local_capture_returns_to_unknown() {
        let batch = reduce_cursor_routing_batch(
            CursorRoutingState::CapturedHud,
            &[event(
                1,
                NativeVideoWindowSource::Hud,
                CursorRoutingEventKind::CaptureLost,
            )],
            None,
        );
        assert_eq!(batch.state, CursorRoutingState::Unknown);
    }

    #[test]
    fn tracking_failure_returns_to_unknown() {
        let batch = reduce_cursor_routing_batch(
            CursorRoutingState::Hud,
            &[event(
                1,
                NativeVideoWindowSource::Hud,
                CursorRoutingEventKind::TrackingFailed,
            )],
            None,
        );
        assert_eq!(batch.state, CursorRoutingState::Unknown);
    }

    #[test]
    fn startup_unknown_is_seeded_by_zero_delta_local_move() {
        let now = Instant::now();
        let mut reducer = CursorAutoHideReducer::new(0.1);
        let batch = reduce_cursor_routing_batch(
            CursorRoutingState::Unknown,
            &[event(
                1,
                NativeVideoWindowSource::Presenter,
                CursorRoutingEventKind::Move([50, 50]),
            )],
            None,
        );
        assert_eq!(batch.state, CursorRoutingState::Presenter);
        assert_eq!(
            reducer.apply_input_batch(batch.state.input_ownership(), batch.activity, now),
            Some(NativeCursorIcon::Arrow)
        );
        assert_eq!(reducer.last_activity(), Some(now));
    }

    #[test]
    fn reset_for_transition_returns_input_ownership_to_unknown_without_cursor_write() {
        let now = Instant::now();
        let mut reducer = hidden_reducer(now);
        assert_eq!(
            reducer.reset_for_transition(now + Duration::from_millis(151)),
            None
        );
        assert_eq!(reducer.input_ownership(), CursorInputOwnership::Unknown);
        assert!(!reducer.hidden());
    }

    #[test]
    fn placement_hide_and_close_each_reset_to_unknown() {
        let now = Instant::now();
        let mut reducer = CursorAutoHideReducer::new(1.0);
        for step in 0..3 {
            let at = now + Duration::from_millis(step * 10);
            let _ = reducer.apply_input_batch(CursorInputOwnership::Owned, None, at);
            assert!(reducer.input_owned());
            assert_eq!(reducer.reset_for_transition(at), None);
            assert_eq!(reducer.input_ownership(), CursorInputOwnership::Unknown);
        }
    }

    #[test]
    fn unchanged_render_policy_does_not_reapply_cursor() {
        let now = Instant::now();
        let mut reducer = CursorAutoHideReducer::new(1.0);
        assert_eq!(
            reducer.apply_input_batch(CursorInputOwnership::Owned, None, now),
            Some(NativeCursorIcon::Arrow)
        );
        assert_eq!(
            reducer.set_render_policy(NativeCursorIcon::Hand, false, now),
            Some(NativeCursorIcon::Hand)
        );
        assert_eq!(
            reducer.set_render_policy(
                NativeCursorIcon::Hand,
                false,
                now + Duration::from_millis(1)
            ),
            None
        );
        assert_eq!(
            reducer.last_activity(),
            Some(now + Duration::from_millis(1))
        );
    }

    #[test]
    fn unknown_never_hides_or_restores_an_external_cursor() {
        let now = Instant::now();
        let mut reducer = hidden_reducer(now);
        assert_eq!(
            reducer.apply_input_batch(
                CursorInputOwnership::Unknown,
                None,
                now + Duration::from_millis(151),
            ),
            None
        );
        assert_eq!(reducer.tick(now + Duration::from_secs(1)), None);
        assert!(!reducer.hidden());
    }
}
