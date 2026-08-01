//! Pump-owned cursor auto-hide reducer for native video windows.

use std::time::{Duration, Instant};

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

#[derive(Clone, Copy, Debug)]
pub(crate) struct CursorAutoHideReducer {
    input_owned: bool,
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
            input_owned: false,
            last_position: None,
            last_activity: None,
            hidden: false,
            desired_icon: NativeCursorIcon::Arrow,
            auto_hide_allowed: false,
            hide_delay: Duration::from_secs_f32(clamped),
            applied_icon: None,
        }
    }

    pub(crate) fn observe_input_target(
        &mut self,
        input_owned: bool,
        position: Option<[i32; 2]>,
        now: Instant,
    ) -> Option<NativeCursorIcon> {
        let ownership_changed = self.input_owned != input_owned;
        self.input_owned = input_owned;
        if !input_owned {
            self.last_position = position;
            self.last_activity = Some(now);
            self.applied_icon = None;
            return self.release_hidden_cursor();
        }
        if ownership_changed {
            self.last_activity = Some(now);
            self.hidden = false;
            self.applied_icon = None;
        } else if let Some(position) = position
            && self
                .last_position
                .is_some_and(|previous| previous != position)
        {
            self.last_activity = Some(now);
            self.hidden = false;
        }
        self.last_position = position;
        self.resolve(now, ownership_changed)
    }

    pub(crate) fn record_activity(
        &mut self,
        activity: CursorActivity,
        now: Instant,
    ) -> Option<NativeCursorIcon> {
        self.input_owned = true;
        let (position, active) = match activity {
            CursorActivity::Move(position) => (
                position,
                cursor_move_is_activity(
                    self.last_position
                        .map(|position| (position[0], position[1])),
                    (position[0], position[1]),
                    self.hidden,
                ),
            ),
            CursorActivity::Explicit(position) => (position, true),
        };
        self.last_position = Some(position);
        if active {
            self.last_activity = Some(now);
            self.hidden = false;
            self.applied_icon = None;
        }
        self.resolve(now, active)
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
        self.desired_icon = desired_icon;
        self.auto_hide_allowed = auto_hide_allowed;
        if !auto_hide_allowed {
            self.last_activity = Some(now);
            self.hidden = false;
        }
        self.applied_icon = None;
        self.resolve(now, true)
    }

    pub(crate) fn reset_for_transition(&mut self, now: Instant) -> Option<NativeCursorIcon> {
        self.last_position = None;
        self.last_activity = Some(now);
        self.hidden = false;
        self.desired_icon = NativeCursorIcon::Arrow;
        self.auto_hide_allowed = false;
        self.applied_icon = None;
        self.input_owned.then_some(NativeCursorIcon::Arrow)
    }

    pub(crate) fn tick(&mut self, now: Instant) -> Option<NativeCursorIcon> {
        self.resolve(now, false)
    }

    pub(crate) fn input_owned(self) -> bool {
        self.input_owned
    }
    pub(crate) fn hidden(self) -> bool {
        self.hidden
    }
    pub(crate) fn last_activity(self) -> Option<Instant> {
        self.last_activity
    }

    fn resolve(&mut self, now: Instant, force: bool) -> Option<NativeCursorIcon> {
        if !self.input_owned {
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

    fn release_hidden_cursor(&mut self) -> Option<NativeCursorIcon> {
        if self.hidden {
            self.hidden = false;
            Some(NativeCursorIcon::Arrow)
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hidden_reducer(now: Instant) -> CursorAutoHideReducer {
        let mut reducer = CursorAutoHideReducer::new(0.1);
        assert_eq!(
            reducer.observe_input_target(true, Some([50, 50]), now),
            Some(NativeCursorIcon::Arrow)
        );
        let _ = reducer.set_render_policy(NativeCursorIcon::Arrow, true, now);
        assert_eq!(
            reducer.tick(now + Duration::from_millis(150)),
            Some(NativeCursorIcon::Hidden)
        );
        reducer
    }

    #[test]
    fn covered_presenter_releases_auto_hidden_cursor() {
        let now = Instant::now();
        let mut reducer = hidden_reducer(now);
        assert_eq!(
            reducer.observe_input_target(false, Some([50, 50]), now + Duration::from_millis(151)),
            Some(NativeCursorIcon::Arrow)
        );
        assert!(!reducer.hidden());
        assert!(!reducer.input_owned());
    }

    #[test]
    fn unknown_input_target_never_falls_back_to_hiding() {
        let now = Instant::now();
        let mut reducer = hidden_reducer(now);
        assert_eq!(
            reducer.observe_input_target(false, None, now + Duration::from_millis(151)),
            Some(NativeCursorIcon::Arrow)
        );
        assert!(!reducer.hidden());
    }

    #[test]
    fn placement_or_owner_transition_resets_auto_hide_state() {
        let now = Instant::now();
        let mut reducer = hidden_reducer(now);
        assert_eq!(
            reducer.reset_for_transition(now + Duration::from_millis(151)),
            Some(NativeCursorIcon::Arrow)
        );
        assert!(!reducer.hidden());
        assert_eq!(
            reducer.last_activity(),
            Some(now + Duration::from_millis(151))
        );
    }

    #[test]
    fn zero_delta_move_does_not_restore_hidden_cursor_but_real_move_does() {
        let now = Instant::now();
        let mut reducer = hidden_reducer(now);
        assert_eq!(
            reducer.record_activity(
                CursorActivity::Move([50, 50]),
                now + Duration::from_millis(151)
            ),
            None
        );
        assert!(reducer.hidden());
        assert_eq!(
            reducer.record_activity(
                CursorActivity::Move([51, 50]),
                now + Duration::from_millis(152)
            ),
            Some(NativeCursorIcon::Arrow)
        );
        assert!(!reducer.hidden());
    }
}
