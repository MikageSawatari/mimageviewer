//! Root startup geometry: prepare a normal restore size, then optionally maximize after show.

use crate::settings::{StartupWindowState, resolve_startup_maximized};

/// Native pixels, captured from the actual root HWND after its DPI is known.
#[derive(Clone, Copy, Debug)]
pub(crate) struct StartupWindowBounds {
    pub work_area: egui::Rect,
    pub outer_rect: egui::Rect,
    pub client_size: egui::Vec2,
    pub native_pixels_per_point: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct StartupWindowPlacement {
    /// OS-DPI logical geometry, independent of the application's UI zoom.
    pub size: egui::Vec2,
    pub min_size: egui::Vec2,
    pub position: egui::Pos2,
}

pub(crate) fn fit_startup_window(
    size: [f32; 2],
    min_size: [f32; 2],
    bounds: StartupWindowBounds,
) -> StartupWindowPlacement {
    let scale = bounds.native_pixels_per_point;
    let decoration = (bounds.outer_rect.size() - bounds.client_size).max(egui::Vec2::ZERO);
    let available = (bounds.work_area.size() - decoration)
        .floor()
        .max(egui::Vec2::splat(1.0));
    // Keep ordinary native pixel rounding for a fitting restore size. Only the
    // available work-area cap rounds inward; avoid a one-pixel drift on each start.
    let client = (egui::vec2(size[0], size[1]) * scale)
        .round()
        .min(available);
    let outer_size = client + decoration;
    let position = bounds
        .outer_rect
        .min
        .max(bounds.work_area.min)
        .min((bounds.work_area.max - outer_size).max(bounds.work_area.min));
    StartupWindowPlacement {
        size: client / scale,
        min_size: egui::vec2(min_size[0], min_size[1]).min(available / scale),
        position: (position.to_vec2() / scale).to_pos2(),
    }
}

/// Fit the actual hidden root before its first egui input/paint. The ordinary
/// WM_SIZE event updates winit/eframe's surface; do not show, focus or maximize.
/// Keep the original desired normal size for the subsequent DPI-aware viewport
/// reapply, which uses this same work-area calculation.
#[cfg(windows)]
pub(crate) fn fit_created_startup_window(hwnd_raw: isize, size: [f32; 2]) {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::WindowsAndMessaging::{SWP_NOACTIVATE, SWP_NOZORDER, SetWindowPos};
    let Some(bounds) = crate::monitor::startup_window_bounds(hwnd_raw) else {
        crate::logger::log("[viewport] cannot query created root work area".to_owned());
        return;
    };
    let fit = fit_startup_window(size, crate::MIN_INNER_SIZE, bounds);
    let scale = bounds.native_pixels_per_point;
    let decoration = (bounds.outer_rect.size() - bounds.client_size).max(egui::Vec2::ZERO);
    let target = egui::Rect::from_min_size(
        (fit.position.to_vec2() * scale).to_pos2(),
        fit.size * scale + decoration,
    );
    if target == bounds.outer_rect {
        return;
    }
    if let Err(error) = unsafe {
        SetWindowPos(
            HWND(hwnd_raw as *mut _),
            None,
            target.left().round() as i32,
            target.top().round() as i32,
            target.width().round() as i32,
            target.height().round() as i32,
            SWP_NOACTIVATE | SWP_NOZORDER,
        )
    } {
        crate::logger::log(format!(
            "[viewport] created root work area fit failed: {error}"
        ));
        return;
    }
    // Explicit SetWindowPos may be smaller than the interactive builder minimum.
    // Record the real result rather than claiming the requested size was applied.
    if let Some(actual) = crate::monitor::startup_window_bounds(hwnd_raw) {
        crate::logger::log(format!(
            "[viewport] created root work area fit: target={target:?} actual={:?} client={:?} work={:?} contained={}",
            actual.outer_rect,
            actual.client_size,
            actual.work_area,
            actual
                .work_area
                .expand(0.01)
                .contains_rect(actual.outer_rect),
        ));
    }
}

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
    fn startup_normal_geometry_fits_work_area_including_decorations() {
        for scale in [1.0, 1.25, 1.5, 2.0] {
            for origin in [egui::pos2(0.0, 0.0), egui::pos2(-1920.0, -1080.0)] {
                // Also cover taskbars on the left/top, and too-small native minima.
                for work_size in [egui::vec2(1520.0, 775.0), egui::vec2(1093.0, 574.0)] {
                    let work =
                        egui::Rect::from_min_size(origin + egui::vec2(48.0, 40.0), work_size);
                    let decoration = egui::vec2(16.0, 39.0) * scale;
                    let bounds = StartupWindowBounds {
                        work_area: work,
                        outer_rect: egui::Rect::from_min_size(
                            origin + egui::vec2(60.0, 40.0),
                            egui::vec2(1280.0, 800.0) * scale + decoration,
                        ),
                        client_size: egui::vec2(1280.0, 800.0) * scale,
                        native_pixels_per_point: scale,
                    };
                    let fit = fit_startup_window(SIZE, crate::MIN_INNER_SIZE, bounds);
                    let outer = egui::Rect::from_min_size(
                        (fit.position.to_vec2() * scale).to_pos2(),
                        fit.size * scale + decoration,
                    );
                    assert!(
                        work.expand(0.01).contains_rect(outer),
                        "{scale}: {outer:?} in {work:?}"
                    );
                    assert!(fit.min_size.x <= fit.size.x && fit.min_size.y <= fit.size.y);
                    // Viewport commands include UI zoom, but native geometry must not.
                    for zoom in [1.0, 2.0] {
                        let command = fit.size / zoom;
                        assert!((command * (scale * zoom) - fit.size * scale).length() < 0.01);
                    }
                }
            }
        }
    }

    #[test]
    fn startup_normal_geometry_preserves_fitting_large_monitor_placement() {
        let bounds = StartupWindowBounds {
            work_area: egui::Rect::from_min_size(
                egui::pos2(-3840.0, 0.0),
                egui::vec2(3840.0, 2080.0),
            ),
            outer_rect: egui::Rect::from_min_size(
                egui::pos2(-3000.0, 100.0),
                egui::vec2(1936.0, 1239.0),
            ),
            client_size: egui::vec2(1920.0, 1200.0),
            native_pixels_per_point: 1.5,
        };
        let fit = fit_startup_window(SIZE, crate::MIN_INNER_SIZE, bounds);
        assert_eq!(fit.size, egui::vec2(1280.0, 800.0));
        assert_eq!(fit.min_size, egui::vec2(640.0, 580.0));
        assert_eq!(fit.position, egui::pos2(-2000.0, 100.0 / 1.5));
    }

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
