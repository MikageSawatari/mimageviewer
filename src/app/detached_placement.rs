//! Detached placement is physical from the runtime owner to the HWND boundary.

use super::App;
use super::detached_window_manager::{HostPlacement, PlacementOutcome, ShowRequest};
use crate::detached_window_geometry::{self as geometry, PhysicalRect, PhysicalWindowPlacement};
use eframe::egui;

impl App {
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

    fn dispatch_detached_show(&self, ctx: &egui::Context, window_id: u64, request: ShowRequest) {
        let viewport_id = Self::detached_image_window_viewport_id(window_id);
        if request.focus {
            ctx.send_viewport_cmd_to(viewport_id, egui::ViewportCommand::Minimized(false));
        }
        if let Some(maximized) = request.maximized {
            Self::send_detached_viewport_visible_commit(ctx, viewport_id, maximized);
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
    pub(crate) fn record_detached_host_placements_before_commands(&mut self) {
        if !self.window_visible {
            return;
        }
        for window_id in self.detached_window_manager.ids() {
            let Some(hwnd) = self.detached_window_hwnd_alive(window_id) else {
                continue;
            };
            #[cfg(test)]
            let fake = host_geometry_for_test(window_id).is_some();
            #[cfg(not(test))]
            let fake = false;
            if !fake {
                use windows::Win32::Foundation::HWND;
                use windows::Win32::UI::WindowsAndMessaging::{IsIconic, IsWindowVisible};
                let hwnd = HWND(hwnd as *mut _);
                if unsafe { IsIconic(hwnd).as_bool() || !IsWindowVisible(hwnd).as_bool() } {
                    continue;
                }
            }
            self.observe_detached_window_host_placement(window_id, "root_hwnd_observation");
        }
    }

    /// Egui frames only announce an observation. Desktop coordinates come from the HWND.
    pub(crate) fn observe_detached_window_host_placement(
        &mut self,
        window_id: u64,
        source: &'static str,
    ) -> Option<PhysicalWindowPlacement> {
        if !self.detached_window_host_is_placed(window_id) {
            return None;
        }
        if self.active_detached_hwnd_window_id() == Some(window_id)
            && (self.detached_viewer_borderless_fullscreen
                || self.detached_viewer_borderless_transition.is_some())
        {
            return None;
        }
        let hwnd = self.detached_window_hwnd_alive(window_id)?;
        let previous = self.detached_window_seed_placement(window_id);
        if self.detached_host_zoomed(window_id, hwnd) {
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
        let outer = self.detached_host_outer(window_id, hwnd)?;
        let candidate = PhysicalWindowPlacement {
            outer,
            maximized: false,
        };
        if !Self::detached_window_runtime_placement_is_usable(candidate) {
            return None;
        }
        let ppp = geometry::monitor_for_rect(outer).map_or(1.0, |monitor| monitor.scale());
        let Some(candidate_client) = self.detached_host_client_size(window_id, hwnd) else {
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
            app.record_detached_host_placements_before_commands();
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
