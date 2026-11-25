//! Main-context event ownership. UI detection is intentionally not connected yet:
//! reader-order and adopted-location publication need a complete cheap identity key.
//! See docs/operation-script-stage4-handoff.md before adding a call site.

use std::{
    collections::VecDeque,
    time::{Duration, Instant},
};

use super::*;
use crate::operation_script::ScriptEvent;

#[derive(Clone, Debug, PartialEq)]
pub(super) struct EventIdentity {
    /// A completed, adopted logical location, never an in-flight current_folder.
    pub location: ScriptLocation,
    pub context: ScriptContext,
    pub page: Option<ScriptPageIdentity>,
    /// Cursor only; checked items are intentionally outside the select event.
    pub selected: Option<ScriptItemIdentity>,
}

/// App-common main state; never exchanged with a viewer-context bundle.
/// Skipped passes retain their baseline; over-limit events are discarded, never queued.
pub(crate) struct OperationScriptEvents {
    baseline: Option<EventIdentity>,
    detected_root_frame: u64,
    executions: VecDeque<Instant>,
    limit_notified: bool,
}

impl Default for OperationScriptEvents {
    fn default() -> Self {
        Self {
            baseline: None,
            detected_root_frame: u64::MAX,
            executions: VecDeque::new(),
            limit_notified: false,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum EventAdmission {
    Execute,
    Drop { notify: bool },
}

impl OperationScriptEvents {
    pub(super) fn begin_frame(&mut self, root_frame: u64) -> bool {
        if self.detected_root_frame == root_frame {
            return false;
        }
        // Record even if comparison finds no changes, before running any script.
        self.detected_root_frame = root_frame;
        true
    }

    pub(super) fn observe(&mut self, current: EventIdentity) -> Vec<ScriptEvent> {
        let previous = self.baseline.as_ref();
        let started = crate::perf::is_enabled().then(Instant::now);
        let mut events = Vec::new();
        if previous.is_none_or(|previous| previous.location != current.location) {
            events.push(ScriptEvent::Open);
        }
        if previous.map_or(ScriptContext::Grid, |previous| previous.context) != current.context {
            events.push(ScriptEvent::View);
        }
        if current.context != ScriptContext::Grid
            && current.page.is_some()
            && previous.and_then(|previous| previous.page.as_ref()) != current.page.as_ref()
        {
            events.push(ScriptEvent::Page);
        }
        if current.context == ScriptContext::Grid
            && previous.map_or(current.selected.is_some(), |previous| {
                (&previous.location, &previous.selected) != (&current.location, &current.selected)
            })
        {
            events.push(ScriptEvent::Select);
        }
        self.baseline = Some(current);
        if let Some(started) = started {
            crate::perf::event(
                "script",
                "event_compare",
                None,
                0,
                &[
                    (
                        "ms",
                        serde_json::json!(started.elapsed().as_secs_f64() * 1000.0),
                    ),
                    ("events", serde_json::json!(events.len())),
                ],
            );
        }
        events
    }

    /// Call immediately after an event invocation, including rejection/failure.
    /// Key invocations never call this: their changes belong to the next comparison.
    pub(super) fn finish_event(&mut self, current: EventIdentity) {
        self.baseline = Some(current);
    }

    pub(super) fn admit_execution(&mut self, now: Instant) -> EventAdmission {
        while self.executions.front().is_some_and(|previous| {
            now.saturating_duration_since(*previous) >= Duration::from_secs(1)
        }) {
            self.executions.pop_front();
        }
        if self.executions.len() == 10 {
            let notify = !self.limit_notified;
            self.limit_notified = true;
            return EventAdmission::Drop { notify };
        }
        self.executions.push_back(now);
        EventAdmission::Execute
    }
}

impl App {
    /// The future owner call sites must pass their live viewport's Context.
    /// Only readiness/eligibility/frame ownership are implemented here; obtaining
    /// the O(N) identity is blocked pending the adoption-owner decision.
    pub(crate) fn begin_operation_script_event_detection(
        &mut self,
        ctx: &egui::Context,
        surface: ScriptInputSurface,
    ) -> bool {
        // OFF / no receiver never read keyboard ownership or build an identity.
        if !self.settings.operation_script_enabled
            || !matches!(&self.operation_script.state, OperationScriptState::Ready(script) if script.has_on_event)
        {
            return false;
        }
        if !matches!(
            self.operation_script_eligibility(ctx, surface),
            ScriptEligibility::Available(_)
        ) {
            return false;
        }
        self.operation_script_events
            .begin_frame(ctx.cumulative_frame_nr_for(egui::ViewportId::ROOT))
    }
}
