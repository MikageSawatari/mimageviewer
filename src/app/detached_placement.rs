//! Detached placement is physical from the runtime owner to the HWND boundary.

use super::App;
use super::detached_window_manager::{
    GeometryNotification, HostPlacement, PlacementOutcome, ShowRequest,
};
use crate::detached_window_geometry::{self as geometry, PhysicalRect, PhysicalWindowPlacement};
use eframe::egui;

enum HostObservation {
    Maximized,
    Normal(PhysicalRect),
}

impl App {
    pub(crate) fn invalidate_detached_geometry_on_visible_commit(
        &mut self,
        viewport_id: egui::ViewportId,
    ) {
        for window_id in self.detached_window_manager.ids() {
            if Self::detached_image_window_viewport_id(window_id) == viewport_id {
                self.detached_window_manager
                    .invalidate_geometry_notification(window_id);
                break;
            }
        }
    }

    pub(super) fn default_detached_viewer_window_placement(&self) -> PhysicalWindowPlacement {
        let main_outer = self
            .main_hwnd
            .and_then(|hwnd| geometry::read_outer_rect(hwnd as isize))
            .unwrap_or(PhysicalRect {
                left: 32,
                top: 32,
                right: 992,
                bottom: 752,
            });
        if let Some(monitor) = geometry::monitor_for_rect(main_outer) {
            geometry::default_placement(
                main_outer,
                monitor,
                geometry::decorated_frame(monitor.dpi).unwrap_or_default(),
            )
        } else {
            crate::logger::log("[detached-placement] no monitor available for default seed");
            PhysicalWindowPlacement {
                outer: main_outer,
                maximized: false,
            }
        }
    }

    pub(crate) fn detached_viewer_window_placement(&self) -> PhysicalWindowPlacement {
        if let Some(placement) = self.settings.detached_viewer_window_physical_placement {
            return if Self::detached_window_runtime_placement_is_usable(placement) {
                placement
            } else {
                self.default_detached_viewer_window_placement()
            };
        }
        let monitors = geometry::monitors();
        let frames: Vec<_> = monitors
            .iter()
            .map(|monitor| geometry::decorated_frame(monitor.dpi).unwrap_or_default())
            .collect();
        self.settings
            .detached_viewer_window_placement
            .and_then(|legacy| geometry::migrate_legacy_placement(legacy, &monitors, &frames))
            .filter(|placement| Self::detached_window_runtime_placement_is_usable(*placement))
            .unwrap_or_else(|| self.default_detached_viewer_window_placement())
    }

    /// A newly created HWND never chooses the borderless target.
    pub(crate) fn detached_viewer_borderless_target_rect(&self) -> PhysicalRect {
        let intent = self
            .detached_viewer_restore_placement
            .unwrap_or_else(|| self.active_detached_viewer_current_placement())
            .outer;
        geometry::borderless_target_for(intent).unwrap_or(intent)
    }

    pub(crate) fn detached_window_host_is_placed(&self, window_id: u64) -> bool {
        self.detached_window_manager
            .runtime(window_id)
            .is_some_and(|runtime| {
                matches!(runtime.host_placement, HostPlacement::Placed { incarnation, .. }
                if incarnation == runtime.host_incarnation && runtime.hwnd != 0)
            })
    }

    fn dispatch_detached_show(
        &mut self,
        ctx: &egui::Context,
        window_id: u64,
        request: ShowRequest,
    ) {
        let viewport_id = Self::detached_image_window_viewport_id(window_id);
        if request.focus {
            ctx.send_viewport_cmd_to(viewport_id, egui::ViewportCommand::Minimized(false));
        }
        let maximized = self
            .detached_window_manager
            .runtime(window_id)
            .map(|_| self.detached_window_placement_target(window_id).maximized)
            .or(request.maximized);
        if let Some(maximized) = maximized {
            self.send_detached_viewport_visible_commit(ctx, viewport_id, maximized);
        } else {
            ctx.send_viewport_cmd_to(viewport_id, egui::ViewportCommand::Visible(true));
        }
        if request.focus {
            ctx.send_viewport_cmd_to(viewport_id, egui::ViewportCommand::Focus);
        }
    }

    /// Root-side visibility and the associated focus/maximization share one request owner.
    pub(crate) fn request_detached_window_visibility(
        &mut self,
        ctx: &egui::Context,
        window_id: u64,
        visible: bool,
        maximized: Option<bool>,
        focus: bool,
    ) {
        if !visible {
            self.detached_window_manager.clear_show_request(window_id);
            ctx.send_viewport_cmd_to(
                Self::detached_image_window_viewport_id(window_id),
                egui::ViewportCommand::Visible(false),
            );
            return;
        }
        // A late transition effect must not unhide an already-placed host while
        // the application is resident in the tray. Keep the same request owner.
        if !self.window_visible && self.detached_window_host_is_placed(window_id) {
            self.detached_window_manager
                .mark_host_unplaced(window_id, false);
        }
        let maximized = if self.detached_window_manager.runtime(window_id).is_some() {
            maximized.map(|_| self.detached_window_placement_target(window_id).maximized)
        } else {
            maximized
        };
        if let Some(request) = self
            .detached_window_manager
            .request_show(window_id, ShowRequest { maximized, focus })
        {
            self.dispatch_detached_show(ctx, window_id, request);
        }
    }

    /// The owner, rather than a delayed visibility request, determines window mode.
    fn detached_window_placement_target(&self, window_id: u64) -> PhysicalWindowPlacement {
        let placement = self.detached_window_seed_placement(window_id);
        if self.active_detached_hwnd_window_id() != Some(window_id) {
            return placement;
        }
        if let Some(transition) = self.detached_viewer_borderless_transition {
            return PhysicalWindowPlacement {
                outer: transition.target_outer,
                maximized: !transition.target_borderless
                    && self
                        .detached_viewer_restore_placement
                        .unwrap_or(placement)
                        .maximized,
            };
        }
        if self.detached_viewer_borderless_fullscreen {
            PhysicalWindowPlacement {
                outer: self.detached_viewer_borderless_target_rect(),
                maximized: false,
            }
        } else {
            placement
        }
    }

    pub(super) fn apply_detached_window_host_placement(
        &mut self,
        ctx: &egui::Context,
        window_id: u64,
    ) {
        let active = self.active_detached_hwnd_window_id() == Some(window_id);
        if let Some(transition) = self
            .detached_viewer_borderless_transition
            .filter(|_| active)
        {
            if !self.detached_window_host_is_placed(window_id) {
                // A replacement created during AwaitUnzoom has not received the
                // old host's mode command. Decorations preserve the outer rect.
                ctx.send_viewport_cmd_to(
                    Self::detached_image_window_viewport_id(window_id),
                    egui::ViewportCommand::Decorations(!transition.target_borderless),
                );
            }
            self.apply_detached_window_target(
                ctx,
                window_id,
                self.detached_window_placement_target(window_id),
                false,
            );
            return;
        }
        let borderless = active && self.detached_viewer_borderless_fullscreen;
        let placement = self.detached_window_placement_target(window_id);
        self.apply_detached_window_target(ctx, window_id, placement, !borderless);
    }

    /// Returns true after an application attempt, including a reported Mismatch.
    pub(super) fn apply_detached_window_target(
        &mut self,
        ctx: &egui::Context,
        window_id: u64,
        target: PhysicalWindowPlacement,
        ordinary_maximized_adoption: bool,
    ) -> bool {
        if self.detached_window_host_is_placed(window_id) {
            return true;
        }
        let Some(claim) = self.detached_window_manager.host_claim_alive(window_id) else {
            return false;
        };
        let awaiting = matches!(
            self.detached_window_manager.host_placement(window_id),
            Some(HostPlacement::Unplaced {
                awaiting_unzoom: true,
                ..
            })
        );
        let zoomed = self.detached_host_zoomed(window_id, claim.hwnd);
        // SW_RESTORE / SW_MAXIMIZE also show and activate a window. Never advance this
        // sequence while the root is hidden to tray, even if the unzoom has completed.
        let pending_show = matches!(
            self.detached_window_manager.host_placement(window_id),
            Some(HostPlacement::Unplaced {
                show_request: Some(_),
                ..
            })
        );
        if !self.window_visible && (awaiting || zoomed || pending_show) {
            return false;
        }
        // SW_RESTORE also makes a hidden window visible. Only an explicit show
        // owner may start that sequence for a hidden, already-maximized host.
        let host_visible = self.detached_host_visible(window_id, claim.hwnd);
        if zoomed && !host_visible && !pending_show {
            return false;
        }
        let same_normal_placement = ordinary_maximized_adoption
            && target.maximized
            && zoomed
            && geometry::monitor_for_window(claim.hwnd as isize)
                .zip(geometry::monitor_for_rect(target.outer))
                .is_some_and(|(actual, desired)| actual.id == desired.id)
            && geometry::read_normal_outer_rect(claim.hwnd as isize) == Some(target.outer);
        if zoomed && !same_normal_placement {
            if !awaiting {
                self.detached_window_manager
                    .mark_host_unplaced(window_id, true);
                ctx.send_viewport_cmd_to(
                    Self::detached_image_window_viewport_id(window_id),
                    egui::ViewportCommand::Maximized(false),
                );
            }
            return false;
        }
        let outcome = if same_normal_placement {
            PlacementOutcome::Exact
        } else {
            match self.apply_detached_host_rect(window_id, claim.hwnd, target.outer) {
                Ok(applied) => {
                    crate::logger::log(format!(
                        "[detached-placement] window_id={window_id} incarnation={} target={:?} outcome=Exact moved={}",
                        claim.incarnation, target.outer, applied.moved
                    ));
                    PlacementOutcome::Exact
                }
                Err(mismatch) => {
                    crate::logger::log(format!(
                        "[detached-placement] window_id={window_id} incarnation={} outcome=Mismatch {mismatch:?}",
                        claim.incarnation
                    ));
                    self.show_feedback_toast(
                        "別ウィンドウを指定の位置に置けませんでした".to_string(),
                    );
                    PlacementOutcome::Mismatch
                }
            }
        };
        let request = self
            .detached_window_manager
            .mark_host_placed(claim, outcome)
            .map(|request| ShowRequest {
                maximized: request.maximized.map(|_| target.maximized),
                ..request
            });
        if target.maximized
            && !same_normal_placement
            && self.window_visible
            && host_visible
            && request.is_none_or(|request| request.maximized.is_none())
        {
            ctx.send_viewport_cmd_to(
                Self::detached_image_window_viewport_id(window_id),
                egui::ViewportCommand::Maximized(true),
            );
        }
        if let Some(request) = request {
            self.dispatch_detached_show(ctx, window_id, request);
        }
        true
    }

    pub(crate) fn advance_detached_window_host_placements(&mut self, ctx: &egui::Context) {
        let ids: Vec<_> = self.detached_window_manager.ids();
        for window_id in ids {
            // F11 owns its captured target and mode command ordering until Settle.
            if self.active_detached_hwnd_window_id() == Some(window_id)
                && self.detached_viewer_borderless_transition.is_some()
            {
                continue;
            }
            self.apply_detached_window_host_placement(ctx, window_id);
        }
    }

    pub(super) fn apply_detached_viewer_borderless_target(
        &mut self,
        ctx: &egui::Context,
        transition: super::DetachedViewerBorderlessTransition,
        begin: bool,
    ) -> bool {
        let Some(window_id) = self.active_detached_hwnd_window_id() else {
            return false;
        };
        let viewport_id = Self::detached_image_window_viewport_id(window_id);
        let restore = self
            .detached_viewer_restore_placement
            .unwrap_or_else(|| self.active_detached_viewer_current_placement());
        if begin {
            ctx.send_viewport_cmd_to(
                viewport_id,
                egui::ViewportCommand::Decorations(!transition.target_borderless),
            );
            self.detached_window_manager
                .mark_host_unplaced(window_id, false);
        }
        let target = PhysicalWindowPlacement {
            outer: transition.target_outer,
            maximized: !transition.target_borderless && restore.maximized,
        };
        if !self.apply_detached_window_target(ctx, window_id, target, false) {
            return false;
        }
        self.write_detached_viewer_borderless_state(
            transition.target_borderless,
            transition.target_borderless.then_some(restore),
            "App::apply_detached_viewer_borderless_target",
            "f11_physical_applied",
        );
        if !transition.target_borderless {
            self.set_detached_window_runtime_placement(window_id, restore, "borderless_restore");
        } else if self
            .video_presentation_transition
            .z_order_recovery_permitted()
        {
            self.request_detached_window_visibility(ctx, window_id, true, None, true);
        }
        self.sync_mounted_native_chrome_policy();
        ctx.request_repaint();
        true
    }

    /// The OS commands queued later in this pass must not be observed half-committed.
    /// In particular an unzoom/apply/remaximize sequence preserves the maximize intent
    /// until the backend has executed its commands at the end of the root pass.
    pub(crate) fn record_detached_host_placements_before_commands(&mut self, ctx: &egui::Context) {
        if !self.window_visible {
            return;
        }
        for window_id in self.detached_window_manager.ids() {
            if !self.detached_host_observation_is_eligible(window_id) {
                continue;
            }
            // Eframe supplies metadata for every viewport in root input. A child
            // callback wakes root after its info changes; compare before IsWindow.
            let notification = ctx.input(|input| {
                input
                    .raw
                    .viewports
                    .get(&Self::detached_image_window_viewport_id(window_id))
                    .and_then(GeometryNotification::from_viewport)
            });
            let Some(notification) = notification else {
                continue;
            };
            if !self
                .detached_window_manager
                .geometry_notification_changed(window_id, notification)
            {
                continue;
            }
            if notification.minimized {
                self.detached_window_manager
                    .record_geometry_notification(window_id, notification);
                continue;
            }
            self.observe_detached_window_host_placement_notified(
                window_id,
                "root_hwnd_observation",
                Some(notification),
            );
        }
    }

    fn detached_host_observation_is_eligible(&self, window_id: u64) -> bool {
        self.detached_window_host_is_placed(window_id)
            && !(self.active_detached_hwnd_window_id() == Some(window_id)
                && (self.detached_viewer_borderless_fullscreen
                    || self.detached_viewer_borderless_transition.is_some()))
    }

    /// Decisions refresh only normal geometry, preserving the runtime mode intent.
    /// Root's first-pass observation is the sole reader that updates maximization.
    /// Closing has already captured its final placement before teardown can erase
    /// the borderless state. Unplaced hosts retain the intended runtime target.
    pub(crate) fn capture_detached_window_placement_at_decision(
        &mut self,
        window_id: u64,
        source: &'static str,
    ) -> Option<PhysicalRect> {
        if self.detached_window_state(window_id) == Some(super::DetachedWindowState::Closing) {
            return None;
        }
        let hwnd = self.detached_window_hwnd_alive(window_id)?;
        let outer = self.detached_host_outer(window_id, hwnd);
        if self.detached_host_observation_is_eligible(window_id) {
            #[cfg(test)]
            OBSERVATION_READS.with(|reads| *reads.borrow_mut().entry(window_id).or_default() += 1);
            let zoomed = self.detached_host_zoomed(window_id, hwnd);
            let normal_outer = if zoomed {
                self.detached_host_normal_outer(window_id, hwnd)
            } else {
                outer
            };
            if let Some(normal_outer) = normal_outer {
                let previous = self
                    .detached_window_manager
                    .placement(window_id)
                    .unwrap_or_else(|| self.detached_window_seed_placement(window_id));
                self.record_detached_normal_placement(
                    window_id,
                    hwnd,
                    PhysicalWindowPlacement {
                        outer: normal_outer,
                        ..previous
                    },
                    zoomed,
                    source,
                );
            }
        }
        outer
    }

    pub(crate) fn capture_active_detached_placement_at_decision(
        &mut self,
        source: &'static str,
    ) -> PhysicalWindowPlacement {
        if let Some(window_id) = self.active_detached_hwnd_window_id() {
            self.capture_detached_window_placement_at_decision(window_id, source);
        }
        self.active_detached_viewer_current_placement()
    }

    /// Egui frames only announce an observation. Desktop coordinates come from the HWND.
    pub(crate) fn observe_detached_window_host_placement(
        &mut self,
        window_id: u64,
        source: &'static str,
    ) -> Option<PhysicalWindowPlacement> {
        self.observe_detached_window_host_placement_notified(window_id, source, None)
    }

    fn observe_detached_window_host_placement_notified(
        &mut self,
        window_id: u64,
        source: &'static str,
        notification: Option<GeometryNotification>,
    ) -> Option<PhysicalWindowPlacement> {
        if !self.detached_host_observation_is_eligible(window_id) {
            return None;
        }
        if let Some(notification) = notification {
            // A hidden host must not be polled again on idle frames. The visible
            // commit invalidates this notification so the shown host is sampled.
            self.detached_window_manager
                .record_geometry_notification(window_id, notification);
        }
        let hwnd = self.detached_window_hwnd_alive(window_id)?;
        let observation = self.read_detached_host_observation(window_id, hwnd)?;
        self.record_detached_host_observation(window_id, hwnd, observation, source)
    }

    fn record_detached_host_observation(
        &mut self,
        window_id: u64,
        hwnd: u64,
        observation: HostObservation,
        source: &'static str,
    ) -> Option<PhysicalWindowPlacement> {
        // This is the already-validated physical owner, not a fresh seed lookup
        // (which would enumerate monitors on every unchanged observation).
        let previous = self
            .detached_window_manager
            .placement(window_id)
            .unwrap_or_else(|| self.detached_window_seed_placement(window_id));
        if matches!(observation, HostObservation::Maximized) {
            if previous.maximized {
                return None;
            }
            self.set_detached_window_runtime_placement(
                window_id,
                PhysicalWindowPlacement {
                    maximized: true,
                    ..previous
                },
                source,
            );
            return None;
        }
        let HostObservation::Normal(outer) = observation else {
            unreachable!()
        };
        let candidate = PhysicalWindowPlacement {
            outer,
            maximized: false,
        };
        self.record_detached_normal_placement(window_id, hwnd, candidate, false, source)
    }

    fn record_detached_normal_placement(
        &mut self,
        window_id: u64,
        hwnd: u64,
        candidate: PhysicalWindowPlacement,
        zoomed: bool,
        source: &'static str,
    ) -> Option<PhysicalWindowPlacement> {
        let previous = self
            .detached_window_manager
            .placement(window_id)
            .unwrap_or_else(|| self.detached_window_seed_placement(window_id));
        let outer = candidate.outer;
        // An unchanged physical rect needs neither monitor facts nor client/frame
        // dimensions. A maximized->normal change still records its mode below.
        if candidate == previous {
            return None;
        }
        if !Self::detached_window_runtime_placement_is_usable(candidate) {
            return None;
        }
        let monitor = geometry::monitor_for_rect(outer);
        let ppp = monitor.map_or(1.0, |monitor| monitor.scale());
        // A zoomed HWND's measured client belongs to the maximized outer, not
        // rcNormalPosition. The default-size defense must compare normal sizes.
        let candidate_client = if zoomed {
            monitor
                .and_then(|monitor| geometry::decorated_frame(monitor.dpi))
                .map(|frame| [outer.width() - frame.width, outer.height() - frame.height])
        } else {
            self.detached_host_client_size(window_id, hwnd)
        };
        let Some(candidate_client) = candidate_client else {
            crate::logger::log(format!(
                "[detached-placement] client observation unavailable window_id={window_id}"
            ));
            return None;
        };
        let previous_frame = geometry::monitor_for_rect(previous.outer)
            .and_then(|monitor| geometry::decorated_frame(monitor.dpi));
        let Some(previous_frame) = previous_frame else {
            crate::logger::log(format!(
                "[detached-placement] previous frame extent unavailable window_id={window_id}"
            ));
            return None;
        };
        let previous_client = [
            previous.outer.width() - previous_frame.width,
            previous.outer.height() - previous_frame.height,
        ];
        let rejected = if self.active_detached_hwnd_window_id() == Some(window_id) {
            Self::detached_active_placement_update_looks_like_default_viewport(
                previous,
                candidate,
                previous_client,
                candidate_client,
                ppp,
            )
        } else {
            Self::detached_passive_placement_update_looks_like_default_viewport(
                previous,
                candidate,
                previous_client,
                candidate_client,
                ppp,
                false,
            )
        };
        if rejected {
            crate::logger::log(format!(
                "[detached-placement] rejected default viewport observation window_id={window_id} source={source} previous={previous:?} candidate={candidate:?}"
            ));
            self.detached_window_manager
                .mark_host_unplaced(window_id, false);
            return Some(previous);
        }
        self.set_detached_window_runtime_placement(window_id, candidate, source);
        None
    }

    fn detached_host_outer(&self, _window_id: u64, hwnd: u64) -> Option<PhysicalRect> {
        #[cfg(test)]
        if let Some((outer, _)) = host_geometry_for_test(_window_id) {
            return Some(outer);
        }
        geometry::read_outer_rect(hwnd as isize)
    }

    fn detached_host_normal_outer(&self, _window_id: u64, hwnd: u64) -> Option<PhysicalRect> {
        #[cfg(test)]
        if host_geometry_for_test(_window_id).is_some() {
            return HOST_NORMAL_OUTER.with(|rects| rects.borrow().get(&_window_id).copied());
        }
        geometry::read_normal_outer_rect(hwnd as isize)
    }

    fn detached_host_visible(&self, window_id: u64, hwnd: u64) -> bool {
        #[cfg(test)]
        if host_geometry_for_test(window_id).is_some() {
            return HOST_VISIBILITY
                .with(|visibility| visibility.borrow().get(&window_id).copied().unwrap_or(true));
        }
        let _ = window_id;
        use windows::Win32::Foundation::HWND;
        use windows::Win32::UI::WindowsAndMessaging::IsWindowVisible;
        unsafe { IsWindowVisible(HWND(hwnd as *mut _)).as_bool() }
    }

    fn read_detached_host_observation(&self, window_id: u64, hwnd: u64) -> Option<HostObservation> {
        #[cfg(test)]
        OBSERVATION_READS.with(|reads| *reads.borrow_mut().entry(window_id).or_default() += 1);
        if !self.detached_host_visible(window_id, hwnd) {
            return None;
        }
        #[cfg(test)]
        let fake = host_geometry_for_test(window_id).is_some();
        #[cfg(not(test))]
        let fake = false;
        if !fake {
            use windows::Win32::Foundation::HWND;
            use windows::Win32::UI::WindowsAndMessaging::IsIconic;
            if unsafe { IsIconic(HWND(hwnd as *mut _)).as_bool() } {
                return None;
            }
        }
        if self.detached_host_zoomed(window_id, hwnd) {
            Some(HostObservation::Maximized)
        } else {
            self.detached_host_outer(window_id, hwnd)
                .map(HostObservation::Normal)
        }
    }

    fn detached_host_client_size(&self, window_id: u64, hwnd: u64) -> Option<[i32; 2]> {
        #[cfg(test)]
        if let Some((outer, _)) = host_geometry_for_test(window_id) {
            return Some([outer.width(), outer.height()]);
        }
        let _ = window_id;
        geometry::read_client_size(hwnd as isize)
    }

    fn detached_host_zoomed(&self, _window_id: u64, hwnd: u64) -> bool {
        #[cfg(test)]
        if let Some((_, zoomed)) = host_geometry_for_test(_window_id) {
            return zoomed;
        }
        geometry::read_zoomed(hwnd as isize)
    }

    fn apply_detached_host_rect(
        &self,
        _window_id: u64,
        hwnd: u64,
        target: PhysicalRect,
    ) -> Result<geometry::PlacementApplied, geometry::PlacementMismatch> {
        #[cfg(test)]
        if let Some((outer, zoomed)) = host_geometry_for_test(_window_id) {
            assert!(!zoomed, "physical placement must wait for actual unzoom");
            set_host_geometry_for_test(_window_id, target, false);
            return Ok(geometry::PlacementApplied {
                moved: outer != target,
            });
        }
        geometry::apply_outer_rect(hwnd as isize, target)
    }

    #[cfg(test)]
    pub(crate) fn set_detached_window_host_for_test(&mut self, window_id: u64, hwnd: u64) {
        let mut alive = self.detached_window_manager.registered_hwnds();
        alive.insert(hwnd);
        self.detached_window_manager.set_live_hwnds_for_test(alive);
        self.detached_window_hwnd_set(window_id, hwnd);
    }
}

#[cfg(test)]
thread_local! {
    static HOST_GEOMETRY: std::cell::RefCell<std::collections::HashMap<u64, (PhysicalRect, bool)>> =
        std::cell::RefCell::new(std::collections::HashMap::new());
    static HOST_VISIBILITY: std::cell::RefCell<std::collections::HashMap<u64, bool>> = Default::default();
    static HOST_NORMAL_OUTER: std::cell::RefCell<std::collections::HashMap<u64, PhysicalRect>> = Default::default();
    static OBSERVATION_READS: std::cell::RefCell<std::collections::HashMap<u64, u64>> = Default::default();
}

#[cfg(test)]
pub(crate) fn host_geometry_for_test(window_id: u64) -> Option<(PhysicalRect, bool)> {
    HOST_GEOMETRY.with(|hosts| hosts.borrow().get(&window_id).copied())
}

#[cfg(test)]
pub(crate) fn set_host_geometry_for_test(window_id: u64, outer: PhysicalRect, zoomed: bool) {
    HOST_GEOMETRY.with(|hosts| hosts.borrow_mut().insert(window_id, (outer, zoomed)));
}

/// A thread-local HWND observation fixture. Its scope restores nested fixtures on panic too.
#[cfg(test)]
pub(crate) fn with_host_geometry_for_test<R>(
    window_id: u64,
    outer: PhysicalRect,
    zoomed: bool,
    run: impl FnOnce() -> R,
) -> R {
    struct Restore(u64, Option<(PhysicalRect, bool)>);
    impl Drop for Restore {
        fn drop(&mut self) {
            HOST_GEOMETRY.with(|hosts| {
                let mut hosts = hosts.borrow_mut();
                if let Some(previous) = self.1 {
                    hosts.insert(self.0, previous);
                } else {
                    hosts.remove(&self.0);
                }
            });
        }
    }
    let previous = host_geometry_for_test(window_id);
    let _restore = Restore(window_id, previous);
    set_host_geometry_for_test(window_id, outer, zoomed);
    run()
}

#[cfg(test)]
fn with_host_visibility_for_test<R>(window_id: u64, visible: bool, run: impl FnOnce() -> R) -> R {
    struct Restore(u64, Option<bool>);
    impl Drop for Restore {
        fn drop(&mut self) {
            HOST_VISIBILITY.with(|visibility| {
                let mut visibility = visibility.borrow_mut();
                if let Some(previous) = self.1 {
                    visibility.insert(self.0, previous);
                } else {
                    visibility.remove(&self.0);
                }
            });
        }
    }
    let previous =
        HOST_VISIBILITY.with(|visibility| visibility.borrow_mut().insert(window_id, visible));
    let _restore = Restore(window_id, previous);
    run()
}

#[cfg(test)]
fn with_host_normal_outer_for_test<R>(
    window_id: u64,
    outer: PhysicalRect,
    run: impl FnOnce() -> R,
) -> R {
    struct Restore(u64, Option<PhysicalRect>);
    impl Drop for Restore {
        fn drop(&mut self) {
            HOST_NORMAL_OUTER.with(|rects| {
                let mut rects = rects.borrow_mut();
                if let Some(previous) = self.1 {
                    rects.insert(self.0, previous);
                } else {
                    rects.remove(&self.0);
                }
            });
        }
    }
    let previous = HOST_NORMAL_OUTER.with(|rects| rects.borrow_mut().insert(window_id, outer));
    let _restore = Restore(window_id, previous);
    run()
}

#[cfg(test)]
mod tests {
    use super::super::DetachedWindowState;
    use super::super::tests::phase_c_support::setup_app;
    use super::*;

    fn context(viewport_id: egui::ViewportId) -> egui::Context {
        let ctx = egui::Context::default();
        ctx.set_embed_viewports(false);
        ctx.begin_pass(Default::default());
        ctx.show_viewport_deferred(viewport_id, Default::default(), |_, _| {});
        ctx
    }

    fn observe_frame(app: &mut App, ctx: &egui::Context, info: Option<egui::ViewportInfo>) {
        let mut input = egui::RawInput::default();
        if let Some(info) = info {
            input
                .viewports
                .insert(App::detached_image_window_viewport_id(12), info);
        }
        ctx.begin_pass(input);
        app.record_detached_host_placements_before_commands(ctx);
        ctx.end_pass().drop_without_applying_deltas();
    }

    fn observation_reads() -> u64 {
        OBSERVATION_READS.with(|reads| reads.borrow().get(&12).copied().unwrap_or(0))
    }

    #[test]
    fn hidden_maximized_seed_during_native_transition_waits_for_visible_commit() {
        let mut app = setup_app();
        app.window_visible = true;
        let mut target = app.default_detached_viewer_window_placement();
        target.maximized = true;
        app.set_detached_window_runtime_placement(12, target, "test");
        app.video_presentation_transition.request_transition(
            super::super::ViewerPresentation::DetachedWindow,
            true,
            false,
            0,
            None,
            None,
        );
        assert!(app.video_presentation_transition.is_transitioning());
        app.detached_window_manager
            .set_live_hwnds_for_test([0x1234]);
        let id = App::detached_image_window_viewport_id(12);
        let ctx = context(id);
        with_host_geometry_for_test(12, target.outer, false, || {
            with_host_visibility_for_test(12, false, || {
                app.detached_window_hwnd_set_for_reusable_lease(&ctx, 12, 0x1234, "hidden_native")
                    .unwrap();
                assert!(app.detached_window_host_is_placed(12));
                assert_eq!(app.detached_window_runtime_placement(12), Some(target));
            });
        });
        let output = ctx.end_pass();
        assert!(
            output.viewport_output[&id]
                .commands
                .iter()
                .all(|command| !matches!(
                    command,
                    egui::ViewportCommand::Visible(_)
                        | egui::ViewportCommand::Maximized(_)
                        | egui::ViewportCommand::Focus
                        | egui::ViewportCommand::Minimized(_)
                ))
        );
        output.drop_without_applying_deltas();
        ctx.begin_pass(Default::default());
        ctx.show_viewport_deferred(id, Default::default(), |_, _| {});
        // Even a focus-only explicit show uses the retained authoritative max intent.
        app.request_detached_window_visibility(&ctx, 12, true, None, true);
        let output = ctx.end_pass();
        let commands = &output.viewport_output[&id].commands;
        let maximize = commands
            .iter()
            .position(|c| matches!(c, egui::ViewportCommand::Maximized(true)))
            .unwrap();
        let visible = commands
            .iter()
            .position(|c| matches!(c, egui::ViewportCommand::Visible(true)))
            .unwrap();
        assert!(maximize < visible);
        assert!(
            commands
                .iter()
                .any(|c| matches!(c, egui::ViewportCommand::Focus))
        );
        output.drop_without_applying_deltas();
    }

    #[test]
    fn hidden_host_never_unzooms_or_remaximizes_without_a_show_owner() {
        for zoomed in [false, true] {
            let mut app = setup_app();
            app.window_visible = true;
            let mut target = app.default_detached_viewer_window_placement();
            target.maximized = true;
            app.set_detached_window_runtime_placement(12, target, "test");
            app.set_detached_window_host_for_test(12, 0x1234);
            let id = App::detached_image_window_viewport_id(12);
            let ctx = context(id);
            with_host_geometry_for_test(12, target.outer, zoomed, || {
                with_host_visibility_for_test(12, false, || {
                    app.apply_detached_window_target(&ctx, 12, target, false);
                    assert_eq!(app.detached_window_runtime_placement(12), Some(target));
                });
            });
            let output = ctx.end_pass();
            assert!(
                output.viewport_output[&id].commands.is_empty(),
                "zoomed={zoomed}"
            );
            output.drop_without_applying_deltas();
        }
    }

    #[test]
    fn hiding_an_existing_window_after_unzoom_defers_remaximization_to_show() {
        for ordinary_adoption in [false, true] {
            let mut app = setup_app();
            app.window_visible = true;
            let mut target = app.default_detached_viewer_window_placement();
            target.maximized = true;
            app.set_detached_window_runtime_placement(12, target, "test");
            app.set_detached_window_host_for_test(12, 0x1234);
            let id = App::detached_image_window_viewport_id(12);
            let ctx = context(id);
            with_host_geometry_for_test(12, target.outer, true, || {
                assert!(!app.apply_detached_window_target(&ctx, 12, target, ordinary_adoption));
                set_host_geometry_for_test(12, target.outer, false);
                with_host_visibility_for_test(12, false, || {
                    assert!(app.apply_detached_window_target(&ctx, 12, target, ordinary_adoption));
                });
                assert!(app.detached_window_runtime_placement(12).unwrap().maximized);
            });
            let output = ctx.end_pass();
            let commands = &output.viewport_output[&id].commands;
            assert!(
                commands
                    .iter()
                    .any(|c| matches!(c, egui::ViewportCommand::Maximized(false)))
            );
            assert!(commands.iter().all(|c| !matches!(
                c,
                egui::ViewportCommand::Maximized(true)
                    | egui::ViewportCommand::Visible(true)
                    | egui::ViewportCommand::Focus
            )));
            output.drop_without_applying_deltas();
            ctx.begin_pass(Default::default());
            ctx.show_viewport_deferred(id, Default::default(), |_, _| {});
            app.request_detached_window_visibility(&ctx, 12, true, None, false);
            let output = ctx.end_pass();
            assert!(
                output.viewport_output[&id]
                    .commands
                    .iter()
                    .any(|c| matches!(c, egui::ViewportCommand::Maximized(true)))
            );
            output.drop_without_applying_deltas();
        }
    }

    #[test]
    fn f11_captures_live_outer_before_any_geometry_notification() {
        for maximized in [false, true] {
            let mut app = setup_app();
            app.window_visible = true;
            app.fullscreen_idx = Some(0);
            app.viewer_presentation = super::super::ViewerPresentation::DetachedWindow;
            app.set_detached_window_binding_for_test(Some(12));
            app.begin_active_detached_session(12, super::super::DetachedSource::Image);
            let previous = app.default_detached_viewer_window_placement();
            app.set_detached_window_runtime_placement(12, previous, "test");
            app.set_detached_window_host_for_test(12, 0x1234);
            let claim = app.detached_window_manager.host_claim_alive(12).unwrap();
            app.detached_window_manager
                .mark_host_placed(claim, PlacementOutcome::Exact);
            let ctx = egui::Context::default();
            let info = egui::ViewportInfo {
                outer_rect: Some(egui::Rect::from_min_size(
                    egui::pos2(80.0, 80.0),
                    egui::vec2(960.0, 720.0),
                )),
                maximized: Some(false),
                ..Default::default()
            };
            with_host_geometry_for_test(12, previous.outer, false, || {
                observe_frame(&mut app, &ctx, Some(info.clone()));
                let moved = PhysicalRect {
                    left: previous.outer.left + 90,
                    right: previous.outer.right + 90,
                    ..previous.outer
                };
                set_host_geometry_for_test(12, moved, maximized);
                // The host moved, but egui still reports the previous frame.
                let mut input = egui::RawInput::default();
                input
                    .viewports
                    .insert(App::detached_image_window_viewport_id(12), info.clone());
                ctx.begin_pass(input);
                app.record_detached_host_placements_before_commands(&ctx);
                assert!(app.viewer_session_is_detached());
                assert_eq!(app.detached_window_manager.placement(12), Some(previous));
                let normal_outer = PhysicalRect {
                    top: moved.top + 40,
                    bottom: moved.bottom + 40,
                    ..moved
                };
                with_host_normal_outer_for_test(12, normal_outer, || {
                    app.toggle_detached_viewer_borderless_fullscreen(&ctx);
                });
                assert_eq!(
                    app.detached_viewer_borderless_transition
                        .unwrap()
                        .target_outer,
                    geometry::borderless_target_for(moved).unwrap()
                );
                let restore = app.detached_viewer_restore_placement.unwrap();
                assert_eq!(restore.maximized, previous.maximized);
                assert_eq!(restore.outer, if maximized { normal_outer } else { moved });
                ctx.end_pass().drop_without_applying_deltas();
            });
        }
    }

    #[test]
    fn close_captures_last_move_before_notification_and_preserves_maximized_normal_rect() {
        for (terminal_retirement, maximized, intended_maximized) in
            [false, true].into_iter().flat_map(|retire| {
                [false, true]
                    .into_iter()
                    .flat_map(move |zoomed| [false, true].map(|intent| (retire, zoomed, intent)))
            })
        {
            let mut app = setup_app();
            let previous = PhysicalWindowPlacement {
                maximized: intended_maximized,
                ..app.default_detached_viewer_window_placement()
            };
            app.set_detached_window_runtime_placement(12, previous, "test");
            app.set_detached_window_host_for_test(12, 0x1234);
            let claim = app.detached_window_manager.host_claim_alive(12).unwrap();
            app.detached_window_manager
                .mark_host_placed(claim, PlacementOutcome::Exact);
            let moved = PhysicalRect {
                left: previous.outer.left + 120,
                right: previous.outer.right + 120,
                ..previous.outer
            };
            let actual_outer = if maximized {
                geometry::borderless_target_for(moved).unwrap()
            } else {
                moved
            };
            with_host_geometry_for_test(12, actual_outer, maximized, || {
                with_host_normal_outer_for_test(12, moved, || {
                    // Hidden hosts also retain their final normal geometry.
                    with_host_visibility_for_test(12, false, || {
                        if terminal_retirement {
                            app.retire_terminal_detached_viewport_identity(12, "test_close");
                        } else {
                            app.remove_detached_window_runtime(12, "test_remove");
                        }
                    });
                });
            });
            assert_eq!(
                app.settings.detached_viewer_window_physical_placement,
                Some(PhysicalWindowPlacement {
                    outer: moved,
                    maximized: intended_maximized,
                })
            );
        }
    }

    #[test]
    fn next_root_observation_updates_os_restore_intent_after_decision_geometry_only() {
        let mut app = setup_app();
        app.window_visible = true;
        let previous = PhysicalWindowPlacement {
            maximized: true,
            ..app.default_detached_viewer_window_placement()
        };
        app.set_detached_window_runtime_placement(12, previous, "test");
        app.set_detached_window_host_for_test(12, 0x1234);
        let claim = app.detached_window_manager.host_claim_alive(12).unwrap();
        app.detached_window_manager
            .mark_host_placed(claim, PlacementOutcome::Exact);
        let ctx = egui::Context::default();
        let maximized_outer = geometry::borderless_target_for(previous.outer).unwrap();
        let mut info = egui::ViewportInfo {
            outer_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1920.0, 1080.0),
            )),
            maximized: Some(true),
            ..Default::default()
        };
        with_host_geometry_for_test(12, maximized_outer, true, || {
            observe_frame(&mut app, &ctx, Some(info.clone()));
            let normal_outer = PhysicalRect {
                left: previous.outer.left + 70,
                right: previous.outer.right + 70,
                ..previous.outer
            };
            set_host_geometry_for_test(12, normal_outer, false);
            app.capture_detached_window_placement_at_decision(12, "before_next_observation");
            assert_eq!(
                app.detached_window_manager.placement(12),
                Some(PhysicalWindowPlacement {
                    outer: normal_outer,
                    maximized: true,
                })
            );
            info.maximized = Some(false);
            info.outer_rect = Some(egui::Rect::from_min_size(
                egui::pos2(80.0, 80.0),
                egui::vec2(960.0, 720.0),
            ));
            observe_frame(&mut app, &ctx, Some(info));
            assert_eq!(
                app.detached_window_manager.placement(12),
                Some(PhysicalWindowPlacement {
                    outer: normal_outer,
                    maximized: false,
                })
            );
        });
    }

    #[test]
    fn closing_freezes_captured_normal_placement_before_borderless_teardown() {
        let mut app = setup_app();
        app.set_detached_window_binding_for_test(Some(12));
        app.begin_active_detached_session(12, super::super::DetachedSource::Image);
        let restore = app.default_detached_viewer_window_placement();
        app.set_detached_window_runtime_placement(12, restore, "test");
        app.set_detached_window_host_for_test(12, 0x1234);
        let claim = app.detached_window_manager.host_claim_alive(12).unwrap();
        app.detached_window_manager
            .mark_host_placed(claim, PlacementOutcome::Exact);
        app.write_detached_viewer_borderless_state(true, Some(restore), "test", "test");
        let fullscreen = geometry::borderless_target_for(restore.outer).unwrap();
        with_host_geometry_for_test(12, fullscreen, false, || {
            app.transition_detached_window_state(12, DetachedWindowState::Closing, "test");
            app.write_detached_viewer_borderless_state(false, None, "test", "teardown");
            app.clear_detached_window_hwnd_for_window_id(12);
            app.remove_detached_window_runtime(12, "test");
        });
        assert_eq!(
            app.settings.detached_viewer_window_physical_placement,
            Some(restore)
        );
    }

    #[test]
    fn rehost_and_new_window_seed_capture_the_outgoing_live_owner() {
        let mut app = setup_app();
        app.set_detached_window_binding_for_test(Some(12));
        app.begin_active_detached_session(12, super::super::DetachedSource::Image);
        let previous = app.default_detached_viewer_window_placement();
        app.set_detached_window_runtime_placement(12, previous, "test");
        app.set_detached_window_host_for_test(12, 0x1234);
        let claim = app.detached_window_manager.host_claim_alive(12).unwrap();
        app.detached_window_manager
            .mark_host_placed(claim, PlacementOutcome::Exact);
        let moved = PhysicalRect {
            left: previous.outer.left + 120,
            right: previous.outer.right + 120,
            ..previous.outer
        };
        with_host_geometry_for_test(12, moved, false, || {
            let seed = app.capture_active_detached_placement_at_decision("new_window_seed");
            assert_eq!(seed.outer, moved);
            let moved_again = PhysicalRect {
                top: moved.top + 50,
                bottom: moved.bottom + 50,
                ..moved
            };
            set_host_geometry_for_test(12, moved_again, false);
            app.clear_detached_window_hwnd_for_window_id(12);
            assert_eq!(
                app.detached_window_manager.placement(12).unwrap().outer,
                moved_again
            );
        });
    }

    #[test]
    fn root_observation_reads_once_per_geometry_change_and_never_on_idle_or_focus() {
        let mut app = setup_app();
        app.window_visible = true;
        let target = app.default_detached_viewer_window_placement();
        app.set_detached_window_runtime_placement(12, target, "test");
        app.set_detached_window_host_for_test(12, 0x1234);
        let claim = app.detached_window_manager.host_claim_alive(12).unwrap();
        app.detached_window_manager
            .mark_host_placed(claim, PlacementOutcome::Exact);
        let ctx = egui::Context::default();
        let mut info = egui::ViewportInfo {
            outer_rect: Some(egui::Rect::from_min_size(
                egui::pos2(80.0, 80.0),
                egui::vec2(960.0, 720.0),
            )),
            minimized: Some(false),
            maximized: Some(false),
            native_pixels_per_point: Some(1.5),
            ..Default::default()
        };
        OBSERVATION_READS.with(|reads| reads.borrow_mut().clear());
        with_host_geometry_for_test(12, target.outer, false, || {
            observe_frame(&mut app, &ctx, None);
            assert_eq!(observation_reads(), 0);
            observe_frame(&mut app, &ctx, Some(info.clone()));
            assert_eq!(observation_reads(), 1);
            observe_frame(&mut app, &ctx, Some(info.clone()));
            info.focused = Some(true);
            info.title = Some("changed title".to_string());
            observe_frame(&mut app, &ctx, Some(info.clone()));
            assert_eq!(
                observation_reads(),
                1,
                "unchanged/focus-only frames must not read HWND"
            );
            info.minimized = Some(true);
            observe_frame(&mut app, &ctx, Some(info.clone()));
            assert_eq!(
                observation_reads(),
                1,
                "minimized metadata alone is sufficient"
            );
            info.minimized = Some(false);
            observe_frame(&mut app, &ctx, Some(info.clone()));
            assert_eq!(
                observation_reads(),
                2,
                "restore to the same rect is a new observation"
            );
            let moved = PhysicalRect {
                left: target.outer.left + 90,
                right: target.outer.right + 90,
                ..target.outer
            };
            set_host_geometry_for_test(12, moved, false);
            info.outer_rect = info
                .outer_rect
                .map(|rect| rect.translate(egui::vec2(60.0, 0.0)));
            observe_frame(&mut app, &ctx, Some(info.clone()));
            assert_eq!(
                observation_reads(),
                3,
                "one geometry notification reads one host snapshot"
            );
            assert_eq!(
                app.detached_window_runtime_placement(12).unwrap().outer,
                moved
            );
            observe_frame(&mut app, &ctx, Some(info));
            assert_eq!(observation_reads(), 3);
        });
    }

    #[test]
    fn unplaced_hidden_and_recreated_hosts_do_not_consume_geometry_notifications() {
        let mut app = setup_app();
        app.window_visible = true;
        let target = app.default_detached_viewer_window_placement();
        app.set_detached_window_runtime_placement(12, target, "test");
        app.set_detached_window_host_for_test(12, 0x1234);
        let ctx = egui::Context::default();
        let info = egui::ViewportInfo {
            outer_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(960.0, 720.0),
            )),
            ..Default::default()
        };
        OBSERVATION_READS.with(|reads| reads.borrow_mut().clear());
        with_host_geometry_for_test(12, target.outer, false, || {
            observe_frame(&mut app, &ctx, Some(info.clone()));
            assert_eq!(observation_reads(), 0);
            let claim = app.detached_window_manager.host_claim_alive(12).unwrap();
            app.detached_window_manager
                .mark_host_placed(claim, PlacementOutcome::Exact);
            app.window_visible = false;
            observe_frame(&mut app, &ctx, Some(info.clone()));
            assert_eq!(observation_reads(), 0);
            app.window_visible = true;
            with_host_visibility_for_test(12, false, || {
                observe_frame(&mut app, &ctx, Some(info.clone()));
                observe_frame(&mut app, &ctx, Some(info.clone()));
            });
            assert_eq!(observation_reads(), 1);
            ctx.begin_pass(Default::default());
            app.request_detached_window_visibility(&ctx, 12, true, None, false);
            ctx.end_pass().drop_without_applying_deltas();
            observe_frame(&mut app, &ctx, Some(info.clone()));
            assert_eq!(
                observation_reads(),
                2,
                "showing a previously hidden host still samples"
            );
            app.detached_window_hwnd_clear(12);
            assert_eq!(
                observation_reads(),
                3,
                "rehost captures before clearing HWND"
            );
            app.set_detached_window_host_for_test(12, 0x1234);
            let new_claim = app.detached_window_manager.host_claim_alive(12).unwrap();
            assert_ne!(claim.incarnation, new_claim.incarnation);
            app.detached_window_manager
                .mark_host_placed(new_claim, PlacementOutcome::Exact);
            observe_frame(&mut app, &ctx, Some(info));
            assert_eq!(
                observation_reads(),
                4,
                "reused HWND value has fresh notification metadata"
            );
        });
    }

    #[test]
    fn registration_places_before_releasing_combined_show_and_focus() {
        let mut app = setup_app();
        app.transition_detached_window_state(12, DetachedWindowState::Opening, "test");
        let target = app.default_detached_viewer_window_placement();
        app.set_detached_window_runtime_placement(12, target, "test");
        app.detached_window_manager
            .set_live_hwnds_for_test([0x1234]);
        let id = App::detached_image_window_viewport_id(12);
        let ctx = context(id);
        app.request_detached_window_visibility(&ctx, 12, true, Some(false), false);
        app.request_detached_window_visibility(&ctx, 12, true, None, true);
        assert!(matches!(
            app.detached_window_manager.host_placement(12),
            Some(HostPlacement::Unplaced {
                show_request: Some(ShowRequest { focus: true, .. }),
                ..
            })
        ));
        let mut initial = target.outer;
        initial.left += 50;
        initial.right += 50;
        with_host_geometry_for_test(12, initial, false, || {
            app.detached_window_hwnd_set_for_reusable_lease(&ctx, 12, 0x1234, "test_created")
                .unwrap();
            assert!(app.detached_window_host_is_placed(12));
            assert_eq!(host_geometry_for_test(12), Some((target.outer, false)));
        });
        let output = ctx.end_pass();
        let commands = &output.viewport_output[&id].commands;
        assert!(
            commands
                .iter()
                .any(|c| matches!(c, egui::ViewportCommand::Visible(true)))
        );
        assert!(
            commands
                .iter()
                .any(|c| matches!(c, egui::ViewportCommand::Focus))
        );
        output.drop_without_applying_deltas();
    }

    #[test]
    fn unplaced_observation_cannot_replace_seed_and_same_host_is_not_reapplied() {
        let mut app = setup_app();
        let target = app.default_detached_viewer_window_placement();
        app.set_detached_window_runtime_placement(12, target, "test");
        app.set_detached_window_host_for_test(12, 0x1234);
        let mut moved = target.outer;
        moved.left += 80;
        moved.right += 80;
        let ctx = context(App::detached_image_window_viewport_id(12));
        with_host_geometry_for_test(12, moved, false, || {
            app.observe_detached_window_host_placement(12, "unplaced");
            assert_eq!(app.detached_window_runtime_placement(12), Some(target));
            app.apply_detached_window_host_placement(&ctx, 12);
            assert_eq!(host_geometry_for_test(12), Some((target.outer, false)));
            set_host_geometry_for_test(12, moved, false);
            app.detached_window_hwnd_set_for_reusable_lease(&ctx, 12, 0x1234, "same_incarnation")
                .unwrap();
            assert_eq!(host_geometry_for_test(12), Some((moved, false)));
            app.observe_detached_window_host_placement(12, "placed");
            assert_eq!(
                app.detached_window_runtime_placement(12).unwrap().outer,
                moved
            );
            app.detached_window_hwnd_clear(12);
            app.detached_window_hwnd_set_for_reusable_lease(&ctx, 12, 0x1234, "new_incarnation")
                .unwrap();
            assert!(app.detached_window_host_is_placed(12));
        });
        ctx.end_pass().drop_without_applying_deltas();
    }

    #[test]
    fn unzoom_wait_is_an_os_fact_and_tray_pauses_the_whole_sequence() {
        let mut app = setup_app();
        app.window_visible = true;
        let mut target = app.default_detached_viewer_window_placement();
        target.maximized = true;
        app.set_detached_window_runtime_placement(12, target, "test");
        app.set_detached_window_host_for_test(12, 0x1234);
        let ctx = context(App::detached_image_window_viewport_id(12));
        let initial = PhysicalRect {
            right: target.outer.right + 200,
            ..target.outer
        };
        with_host_geometry_for_test(12, initial, true, || {
            assert!(!app.apply_detached_window_target(&ctx, 12, target, false));
            assert_eq!(host_geometry_for_test(12), Some((initial, true)));
            assert!(matches!(
                app.detached_window_manager.host_placement(12),
                Some(HostPlacement::Unplaced {
                    awaiting_unzoom: true,
                    ..
                })
            ));
            app.window_visible = false;
            set_host_geometry_for_test(12, initial, false);
            assert!(!app.apply_detached_window_target(&ctx, 12, target, false));
            app.record_detached_host_placements_before_commands(&ctx);
            assert_eq!(app.detached_window_runtime_placement(12), Some(target));
            assert_eq!(host_geometry_for_test(12), Some((initial, false)));
            app.window_visible = true;
            assert!(app.apply_detached_window_target(&ctx, 12, target, false));
            assert_eq!(host_geometry_for_test(12), Some((target.outer, false)));
            // The queued maximize has not run yet. No observation is made after
            // commands in this pass, so its false level cannot overwrite intent.
            assert!(app.detached_window_runtime_placement(12).unwrap().maximized);
        });
        let output = ctx.end_pass();
        let commands =
            &output.viewport_output[&App::detached_image_window_viewport_id(12)].commands;
        let unzoom = commands
            .iter()
            .position(|c| matches!(c, egui::ViewportCommand::Maximized(false)))
            .unwrap();
        let rezoom = commands
            .iter()
            .position(|c| matches!(c, egui::ViewportCommand::Maximized(true)))
            .unwrap();
        assert!(unzoom < rezoom);
        output.drop_without_applying_deltas();
    }

    #[test]
    fn f11_uses_its_captured_physical_target_even_when_runtime_moves() {
        let mut app = setup_app();
        app.window_visible = true;
        app.set_detached_window_binding_for_test(Some(12));
        app.begin_active_detached_session(12, super::super::DetachedSource::Image);
        let restore = app.default_detached_viewer_window_placement();
        app.set_detached_window_runtime_placement(12, restore, "test");
        app.set_detached_window_host_for_test(12, 0x1234);
        app.write_detached_viewer_borderless_state(false, Some(restore), "test", "test_restore");
        let target = geometry::borderless_target_for(restore.outer).unwrap();
        let transition = super::super::DetachedViewerBorderlessTransition {
            target_borderless: true,
            target_outer: target,
            phase: super::super::DetachedViewerBorderlessTransitionPhase::ApplyWindowMode,
        };
        let ctx = context(App::detached_image_window_viewport_id(12));
        with_host_geometry_for_test(12, restore.outer, true, || {
            assert!(!app.apply_detached_viewer_borderless_target(&ctx, transition, true));
            set_host_geometry_for_test(12, restore.outer, false);
            assert!(app.apply_detached_viewer_borderless_target(&ctx, transition, false));
            assert_eq!(host_geometry_for_test(12), Some((target, false)));
            assert_eq!(app.detached_viewer_restore_placement, Some(restore));
        });
        ctx.end_pass().drop_without_applying_deltas();
    }

    #[test]
    fn default_viewport_defence_compares_measured_client_including_at_150_percent() {
        let previous = PhysicalWindowPlacement {
            outer: PhysicalRect {
                left: 120,
                top: 100,
                right: 1944,
                bottom: 1358,
            },
            maximized: false,
        };
        let candidate = PhysicalWindowPlacement {
            outer: PhysicalRect {
                left: 600,
                top: 300,
                right: 1424,
                bottom: 958,
            },
            maximized: false,
        };
        for frame in [[24, 58], [0, 0], [50, 100]] {
            let candidate = PhysicalWindowPlacement {
                outer: PhysicalRect {
                    right: candidate.outer.left + 800 + frame[0],
                    bottom: candidate.outer.top + 600 + frame[1],
                    ..candidate.outer
                },
                ..candidate
            };
            assert!(
                App::detached_passive_placement_update_looks_like_default_viewport(
                    previous,
                    candidate,
                    [1800, 1200],
                    [800, 600],
                    1.5,
                    false
                )
            );
            assert!(
                App::detached_active_placement_update_looks_like_default_viewport(
                    previous,
                    candidate,
                    [1800, 1200],
                    [800, 600],
                    1.5
                )
            );
            assert!(
                !App::detached_passive_placement_update_looks_like_default_viewport(
                    previous,
                    candidate,
                    [1800, 1200],
                    [900, 700],
                    1.5,
                    false
                )
            );
            assert!(
                !App::detached_passive_placement_update_looks_like_default_viewport(
                    previous,
                    candidate,
                    [1800, 1200],
                    [800, 600],
                    1.5,
                    true
                )
            );
        }
    }

    #[test]
    fn tray_retains_late_show_focus_until_restored_even_for_an_already_placed_host() {
        let mut app = setup_app();
        let target = app.default_detached_viewer_window_placement();
        app.set_detached_window_runtime_placement(12, target, "test");
        app.set_detached_window_host_for_test(12, 0x1234);
        let ctx = context(App::detached_image_window_viewport_id(12));
        with_host_geometry_for_test(12, target.outer, false, || {
            app.apply_detached_window_host_placement(&ctx, 12);
            assert!(app.detached_window_host_is_placed(12));
            app.window_visible = false;
            app.request_detached_window_visibility(&ctx, 12, true, Some(false), true);
            app.apply_detached_window_host_placement(&ctx, 12);
            assert!(matches!(
                app.detached_window_manager.host_placement(12),
                Some(HostPlacement::Unplaced {
                    show_request: Some(ShowRequest { focus: true, .. }),
                    ..
                })
            ));
            app.window_visible = true;
            app.apply_detached_window_host_placement(&ctx, 12);
            assert!(app.detached_window_host_is_placed(12));
            assert_eq!(host_geometry_for_test(12), Some((target.outer, false)));
        });
        let output = ctx.end_pass();
        let commands =
            &output.viewport_output[&App::detached_image_window_viewport_id(12)].commands;
        assert_eq!(
            commands
                .iter()
                .filter(|c| matches!(c, egui::ViewportCommand::Visible(true)))
                .count(),
            1
        );
        assert_eq!(
            commands
                .iter()
                .filter(|c| matches!(c, egui::ViewportCommand::Focus))
                .count(),
            1
        );
        output.drop_without_applying_deltas();
    }

    #[test]
    fn replacement_host_during_f11_wait_receives_the_captured_target_on_registration() {
        let mut app = setup_app();
        app.window_visible = true;
        app.set_detached_window_binding_for_test(Some(12));
        app.begin_active_detached_session(12, super::super::DetachedSource::Image);
        let restore = app.default_detached_viewer_window_placement();
        app.set_detached_window_runtime_placement(12, restore, "test");
        let target = geometry::borderless_target_for(restore.outer).unwrap();
        app.detached_viewer_borderless_transition =
            Some(super::super::DetachedViewerBorderlessTransition {
                target_borderless: true,
                target_outer: target,
                phase: super::super::DetachedViewerBorderlessTransitionPhase::AwaitUnzoom,
            });
        app.set_detached_window_host_for_test(12, 0x1234);
        let ctx = context(App::detached_image_window_viewport_id(12));
        with_host_geometry_for_test(12, restore.outer, false, || {
            app.detached_window_hwnd_set_for_reusable_lease(&ctx, 12, 0x1234, "f11_replacement")
                .unwrap();
            assert_eq!(host_geometry_for_test(12), Some((target, false)));
            assert!(app.detached_window_host_is_placed(12));
        });
        let output = ctx.end_pass();
        assert!(
            output.viewport_output[&App::detached_image_window_viewport_id(12)]
                .commands
                .iter()
                .any(|c| matches!(c, egui::ViewportCommand::Decorations(false)))
        );
        output.drop_without_applying_deltas();
    }

    #[test]
    fn f11_tray_restore_never_replays_normal_maximize_over_the_captured_fullscreen_target() {
        for replace_host in [false, true] {
            let mut app = setup_app();
            app.window_visible = true;
            app.set_detached_window_binding_for_test(Some(12));
            app.begin_active_detached_session(12, super::super::DetachedSource::Image);
            let mut restore = app.default_detached_viewer_window_placement();
            restore.maximized = true;
            app.set_detached_window_runtime_placement(12, restore, "test");
            app.write_detached_viewer_borderless_state(false, Some(restore), "test", "restore");
            app.set_detached_window_host_for_test(12, 0x1234);
            let target = geometry::borderless_target_for(restore.outer).unwrap();
            let transition = super::super::DetachedViewerBorderlessTransition {
                target_borderless: true,
                target_outer: target,
                phase: super::super::DetachedViewerBorderlessTransitionPhase::AwaitUnzoom,
            };
            app.detached_viewer_borderless_transition = Some(transition);
            let ctx = context(App::detached_image_window_viewport_id(12));
            with_host_geometry_for_test(12, restore.outer, true, || {
                assert!(!app.apply_detached_viewer_borderless_target(&ctx, transition, true));
                app.request_detached_window_visibility(&ctx, 12, false, None, false);
                app.window_visible = false;
                set_host_geometry_for_test(12, restore.outer, false);
                if replace_host {
                    app.detached_window_hwnd_clear(12);
                    app.set_detached_window_host_for_test(12, 0x5678);
                    app.detached_window_hwnd_set_for_reusable_lease(
                        &ctx,
                        12,
                        0x5678,
                        "tray_rehost",
                    )
                    .unwrap();
                    assert!(app.detached_window_host_is_placed(12));
                }
                app.window_visible = true;
                // Tray restoration captured the old normal maximize intent.
                app.request_detached_window_visibility(&ctx, 12, true, Some(true), true);
                assert!(app.apply_detached_viewer_borderless_target(&ctx, transition, false));
                assert_eq!(host_geometry_for_test(12), Some((target, false)));
                assert!(app.detached_viewer_borderless_fullscreen);
                assert_eq!(app.detached_viewer_restore_placement, Some(restore));
            });
            let output = ctx.end_pass();
            let commands =
                &output.viewport_output[&App::detached_image_window_viewport_id(12)].commands;
            assert!(
                !commands
                    .iter()
                    .any(|c| matches!(c, egui::ViewportCommand::Maximized(true))),
                "normal maximize must never overwrite F11 target; replace_host={replace_host}"
            );
            assert!(
                commands
                    .iter()
                    .any(|c| matches!(c, egui::ViewportCommand::Visible(true)))
            );
            assert!(
                commands
                    .iter()
                    .any(|c| matches!(c, egui::ViewportCommand::Focus))
            );
            output.drop_without_applying_deltas();
        }
    }
}
