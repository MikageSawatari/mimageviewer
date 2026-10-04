//! Root startup geometry: prepare a normal restore size, then optionally maximize after show.

use crate::settings::{StartupWindowState, resolve_startup_maximized};

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) enum StartupWindowGeometry {
    #[default]
    Complete,
    NormalSizePending {
        size: [f32; 2],
        maximize_after_show: bool,
    },
    AwaitingVisibleMaximize,
    /// Commands run after egui finishes the frame, including all its layout passes.
    MaximizeQueued {
        frame_nr: u64,
    },
}

impl StartupWindowGeometry {
    pub(crate) fn new(
        size: [f32; 2],
        startup_state: StartupWindowState,
        saved_maximized: bool,
        has_window_size_arg: bool,
    ) -> Self {
        Self::NormalSizePending {
            size,
            maximize_after_show: !has_window_size_arg
                && resolve_startup_maximized(startup_state, saved_maximized),
        }
    }

    pub(crate) fn begin_frame(&mut self, frame_nr: u64) {
        if matches!(*self, Self::MaximizeQueued { frame_nr: queued } if queued != frame_nr) {
            // The previous frame's native commands have run. Ordinary reports, including
            // an immediate user restore, own the state again; do not wait for a MAX ack.
            *self = Self::Complete;
        }
    }

    pub(crate) fn take_normal_size(&mut self) -> Option<[f32; 2]> {
        let Self::NormalSizePending {
            size,
            maximize_after_show,
        } = *self
        else {
            return None;
        };
        *self = if maximize_after_show {
            Self::AwaitingVisibleMaximize
        } else {
            Self::Complete
        };
        Some(size)
    }

    pub(crate) fn awaiting_visible_maximize(&self) -> bool {
        matches!(self, Self::AwaitingVisibleMaximize)
    }

    pub(crate) fn take_post_visible_maximize(
        &mut self,
        visible_commit_completed: bool,
        window_visible: bool,
        minimized: bool,
        frame_nr: u64,
    ) -> bool {
        if self.awaiting_visible_maximize()
            && visible_commit_completed
            && window_visible
            && !minimized
        {
            *self = Self::MaximizeQueued { frame_nr };
            true
        } else {
            false
        }
    }

    pub(crate) fn maximize_queued(&self) -> bool {
        matches!(self, Self::MaximizeQueued { .. })
    }

    pub(crate) fn maximized_to_save(&self, tracked_maximized: bool) -> bool {
        match self {
            Self::NormalSizePending {
                maximize_after_show,
                ..
            } => *maximize_after_show || tracked_maximized,
            Self::AwaitingVisibleMaximize | Self::MaximizeQueued { .. } => true,
            Self::Complete => tracked_maximized,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SIZE: [f32; 2] = [1280.0, 800.0];

    #[test]
    fn saved_maximized_starts_normal_and_maximizes_once_after_visible_commit() {
        let mut state =
            StartupWindowGeometry::new(SIZE, StartupWindowState::RememberLast, true, false);
        // Even native visibility from another producer is not the eframe commit receipt.
        assert!(!state.take_post_visible_maximize(false, true, false, 0));
        assert_eq!(state.take_normal_size(), Some(SIZE));
        assert!(!state.take_post_visible_maximize(false, true, false, 0));
        assert!(!state.take_post_visible_maximize(true, false, false, 0));
        assert!(state.take_post_visible_maximize(true, true, false, 1));
        assert!(!state.take_post_visible_maximize(true, true, false, 1));
        assert_eq!(state.take_normal_size(), None);
        state.begin_frame(2);
        // Restore/unmaximize must never replay the old startup InnerSize or MAX command.
        assert_eq!(state.take_normal_size(), None);
        assert!(!state.take_post_visible_maximize(true, true, false, 2));
    }

    #[test]
    fn startup_settings_and_window_size_override_decide_only_post_show_maximize() {
        for (setting, saved, expected) in [
            (StartupWindowState::RememberLast, false, false),
            (StartupWindowState::RememberLast, true, true),
            (StartupWindowState::Normal, false, false),
            (StartupWindowState::Normal, true, false),
            (StartupWindowState::Maximized, false, true),
            (StartupWindowState::Maximized, true, true),
        ] {
            for has_size in [false, true] {
                let mut state = StartupWindowGeometry::new(SIZE, setting, saved, has_size);
                assert_eq!(state.take_normal_size(), Some(SIZE));
                assert_eq!(
                    state.take_post_visible_maximize(true, true, false, 1),
                    expected && !has_size,
                    "setting={setting:?}, saved={saved}, CLI size={has_size}",
                );
            }
        }
    }

    #[test]
    fn tray_hide_or_minimize_before_maximize_preserves_the_request_until_restore() {
        let mut state =
            StartupWindowGeometry::new(SIZE, StartupWindowState::Maximized, false, false);
        state.take_normal_size();
        assert!(!state.take_post_visible_maximize(true, false, false, 1));
        assert!(state.maximized_to_save(false));
        assert!(!state.take_post_visible_maximize(true, true, true, 2));
        assert!(state.take_post_visible_maximize(true, true, false, 3));
    }

    #[test]
    fn exit_save_preserves_intent_during_startup_and_actual_state_after_command_dispatch() {
        let mut state =
            StartupWindowGeometry::new(SIZE, StartupWindowState::RememberLast, true, false);
        assert!(state.maximized_to_save(false));
        state.take_normal_size();
        assert!(state.maximized_to_save(false));
        state.take_post_visible_maximize(true, true, false, 7);
        // A repeated layout pass still reports the pre-command normal window.
        state.begin_frame(7);
        assert!(state.maximize_queued());
        assert!(state.maximized_to_save(false));
        state.begin_frame(8);
        assert!(!state.maximize_queued());
        assert!(state.maximized_to_save(true));
        assert!(
            !state.maximized_to_save(false),
            "user restore after maximize wins"
        );
        assert!(
            state.maximized_to_save(true),
            "minimize preserves the tracked max flag"
        );

        let mut normal = StartupWindowGeometry::new(SIZE, StartupWindowState::Normal, true, false);
        normal.take_normal_size();
        assert!(!normal.maximized_to_save(false));
    }
}
