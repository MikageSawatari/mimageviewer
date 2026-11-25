//! r11 main-context event detection and synchronous execution in the owning pass.

use std::{
    collections::VecDeque,
    time::{Duration, Instant},
};

use super::*;
use crate::operation_script::ScriptEvent;

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct EventIdentity {
    pub container: Option<ScriptContainer>,
    pub context: ScriptContext,
    pub page: Option<EventPageIdentity>,
    /// Cursor only; checked items are intentionally outside the select event.
    pub selected: Option<ScriptItemIdentity>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ScriptContainer {
    pub(super) path: PathBuf,
    pub(super) kind: &'static str,
}

impl ScriptContainer {
    fn from_item(item: &ScriptItemIdentity) -> Option<Self> {
        let folder = |path: &std::path::Path| {
            path.parent().map(|parent| Self {
                path: parent.to_path_buf(),
                kind: "folder",
            })
        };
        let archive = |path: &std::path::Path| Self {
            path: path.to_path_buf(),
            kind: if path
                .extension()
                .and_then(|ext| ext.to_str())
                .is_some_and(|ext| ext.eq_ignore_ascii_case("epub"))
            {
                "epub"
            } else if path
                .extension()
                .and_then(|ext| ext.to_str())
                .is_some_and(|ext| ext.eq_ignore_ascii_case("pdf"))
            {
                "pdf"
            } else {
                "zip"
            },
        };
        match item {
            ScriptItemIdentity::ZipImage(path, entry) => {
                let parts = crate::zip_loader::split_nested_zip_path(entry);
                let path = if parts.len() > 1 {
                    // Entry names are literal archive identity, never a host path.
                    // join() would replace the outer archive for / or drive prefixes.
                    let mut logical = path.as_os_str().to_owned();
                    logical.push("/");
                    logical.push(parts[..parts.len() - 1].join("/"));
                    PathBuf::from(logical)
                } else {
                    path.clone()
                };
                Some(archive(&path))
            }
            ScriptItemIdentity::PdfPage(path, _) => Some(archive(path)),
            ScriptItemIdentity::File("image" | "video" | "audio", path) => folder(path),
            // Container rows, stacks and placeholders are not viewer pages.
            ScriptItemIdentity::File(_, _)
            | ScriptItemIdentity::Stack(_, _)
            | ScriptItemIdentity::ZipDirectory(_, _) => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct EventPageIdentity {
    container: Option<ScriptContainer>,
    scalars: ScriptPageScalars,
    unit_source: ScriptUnitSourceInputs,
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
        let mut events = Vec::new();
        if current.context != ScriptContext::Grid
            && current.container.is_some()
            && previous.and_then(|previous| previous.container.as_ref())
                != current.container.as_ref()
        {
            events.push(ScriptEvent::Open);
        }
        if previous.map(|previous| previous.context) != Some(current.context) {
            events.push(ScriptEvent::View);
        }
        if current.context != ScriptContext::Grid
            && current.page.is_some()
            && previous.and_then(|previous| previous.page.as_ref()) != current.page.as_ref()
        {
            events.push(ScriptEvent::Page);
        }
        if current.context == ScriptContext::Grid
            && previous.and_then(|previous| previous.selected.as_ref()) != current.selected.as_ref()
        {
            events.push(ScriptEvent::Select);
        }
        self.finish_event(current);
        events
    }

    /// Call immediately after an event invocation, including rejection/failure.
    /// Key invocations never call this: their changes belong to the next comparison.
    pub(super) fn finish_event(&mut self, mut current: EventIdentity) {
        if current.context == ScriptContext::Grid {
            current.container = None;
            current.page = None;
        } else {
            current.selected = self
                .baseline
                .as_ref()
                .and_then(|previous| previous.selected.clone());
        }
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
    pub(crate) fn operation_script_has_events(&self) -> bool {
        self.settings.operation_script_enabled
            && matches!(&self.operation_script.state, OperationScriptState::Ready(script) if script.has_on_event)
    }
    pub(crate) fn collect_operation_script_event_identity(
        &self,
        surface: ScriptInputSurface,
    ) -> EventIdentity {
        let viewer = surface == ScriptInputSurface::Viewer;
        let container = viewer
            .then(|| self.operation_script_viewer_container())
            .flatten();
        EventIdentity {
            container: container.clone(),
            context: self.script_context(surface),
            page: viewer.then(|| EventPageIdentity {
                container,
                scalars: self.operation_script_page_scalars(),
                unit_source: self.operation_script_unit_source(),
            }),
            selected: (!viewer)
                .then(|| {
                    self.selected
                        .and_then(|idx| self.items.get(idx))
                        .map(item_identity)
                })
                .flatten(),
        }
    }

    pub(super) fn operation_script_viewer_container(&self) -> Option<ScriptContainer> {
        self.fullscreen_idx
            .and_then(|idx| self.items.get(idx))
            .map(item_identity)
            .as_ref()
            .and_then(ScriptContainer::from_item)
    }

    /// Live owning Context only; ROOT frame numbering is shared by all viewports.
    pub(crate) fn begin_operation_script_event_detection(
        &mut self,
        ctx: &egui::Context,
        surface: ScriptInputSurface,
    ) -> bool {
        // OFF / no receiver never read keyboard ownership or build an identity.
        if !self.operation_script_has_events() {
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

    pub(crate) fn detect_operation_script_events(
        &mut self,
        ctx: &egui::Context,
        surface: ScriptInputSurface,
    ) {
        if !self.begin_operation_script_event_detection(ctx, surface) {
            return;
        }
        self.detect_admitted_operation_script_events(ctx, surface, Instant::now());
    }

    #[cfg(test)]
    pub(super) fn detect_operation_script_events_at(
        &mut self,
        ctx: &egui::Context,
        surface: ScriptInputSurface,
        now: Instant,
    ) {
        if self.begin_operation_script_event_detection(ctx, surface) {
            self.detect_admitted_operation_script_events(ctx, surface, now);
        }
    }

    fn detect_admitted_operation_script_events(
        &mut self,
        ctx: &egui::Context,
        surface: ScriptInputSurface,
        now: Instant,
    ) {
        let started = crate::perf::is_enabled().then(Instant::now);
        let identity = self.collect_operation_script_event_identity(surface);
        let events = self.operation_script_events.observe(identity);
        if let Some(started) = started {
            crate::perf::event(
                "script",
                "event_detect",
                None,
                0,
                &[
                    (
                        "ms",
                        serde_json::json!(started.elapsed().as_secs_f64() * 1000.0),
                    ),
                    (
                        "events",
                        serde_json::json!(
                            events.iter().map(|event| event.name()).collect::<Vec<_>>()
                        ),
                    ),
                ],
            );
        }
        if events.is_empty() {
            return;
        }
        match self.operation_script_events.admit_execution(now) {
            EventAdmission::Drop { notify } => {
                if notify {
                    self.show_feedback_warning(
                        "イベントの実行回数が多いためスクリプトを一時的に省略しました".into(),
                    );
                }
            }
            EventAdmission::Execute => {
                self.run_operation_script_event(ctx, surface, events);
                let identity = self.collect_operation_script_event_identity(surface);
                self.operation_script_events.finish_event(identity);
            }
        }
    }

    fn run_operation_script_event(
        &mut self,
        ctx: &egui::Context,
        surface: ScriptInputSurface,
        events: Vec<ScriptEvent>,
    ) {
        let native_video =
            surface == ScriptInputSurface::Viewer && self.operation_script_uses_native_video();
        let mut snapshot = self.collect_operation_script_snapshot(surface);
        snapshot.events = events;
        let result = match &self.operation_script.state {
            OperationScriptState::Ready(script) => script.evaluate_event(&snapshot),
            _ => return,
        }
        .and_then(|output| {
            validate_output(
                output,
                ValidationContext {
                    context: snapshot.context,
                    invocation: ScriptInvocation::Event,
                    navigation_already_accepted: false,
                },
            )
            .map_err(|error| format!("操作の指定を確認してください ({error:?})"))
        });
        let output = match result {
            Ok(output) => output,
            Err(error) => {
                self.operation_script_event_failure(error);
                return;
            }
        };
        for operation in output.operations() {
            if !matches!(self.operation_script_eligibility(ctx, surface), ScriptEligibility::Available(context) if context == snapshot.context)
            {
                self.operation_script_event_failure("操作の対象が変わりました");
                return;
            }
            let result =
                self.execute_operation_script_action(ctx, surface, operation.action, native_video);
            if result.rejected() || result.needs_postprocessing() {
                self.operation_script_event_failure(format!(
                    "『{}』を実行できませんでした",
                    operation.action.ini_name()
                ));
                return;
            }
        }
        if let Some(notice) = output.notice() {
            self.show_feedback_toast(notice.to_owned());
        }
    }

    fn operation_script_event_failure(&mut self, reason: impl std::fmt::Display) {
        let message = format!("スクリプトのイベント: {reason}");
        crate::logger::log(&message);
        self.show_feedback_warning(message);
    }
}
