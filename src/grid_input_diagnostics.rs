//! Bounded perf-log schema for main-grid pointer/open diagnostics.
//!
//! The trace starts from the physical primary press, before egui decides whether a click exists.
//! That boundary is intentional: a missing `clicked` / `double_clicked` signal is the case this
//! instrumentation must preserve rather than the condition that enables it.

use serde_json::Value;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum GridOpenBlockReason {
    ModalDialog,
    ContextMenu,
    BadgeHit,
    NativeDragDrop,
}

impl GridOpenBlockReason {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::ModalDialog => "modal_dialog",
            Self::ContextMenu => "context_menu",
            Self::BadgeHit => "badge_hit",
            Self::NativeDragDrop => "native_drag_drop",
        }
    }

    pub(crate) fn from_state(
        modal: bool,
        context_menu: bool,
        badge_hit: bool,
        drag_started: bool,
    ) -> Option<Self> {
        if modal {
            Some(Self::ModalDialog)
        } else if context_menu {
            Some(Self::ContextMenu)
        } else if badge_hit {
            Some(Self::BadgeHit)
        } else if drag_started {
            Some(Self::NativeDragDrop)
        } else {
            None
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum GridActivationIgnoredReason {
    ModalDialog,
    ContextMenu,
    BadgeHit,
    NativeDragDrop,
    SelectionOnly,
    ClickNotRecognized,
    NoCellSignal,
    ReadingHistoryGuard,
}

impl GridActivationIgnoredReason {
    fn as_str(self) -> &'static str {
        match self {
            Self::ModalDialog => "modal_dialog",
            Self::ContextMenu => "context_menu",
            Self::BadgeHit => "badge_hit",
            Self::NativeDragDrop => "native_drag_drop",
            Self::SelectionOnly => "selection_only",
            Self::ClickNotRecognized => "click_not_recognized",
            Self::NoCellSignal => "no_cell_signal",
            Self::ReadingHistoryGuard => "reading_history_guard",
        }
    }
}

impl From<GridOpenBlockReason> for GridActivationIgnoredReason {
    fn from(value: GridOpenBlockReason) -> Self {
        match value {
            GridOpenBlockReason::ModalDialog => Self::ModalDialog,
            GridOpenBlockReason::ContextMenu => Self::ContextMenu,
            GridOpenBlockReason::BadgeHit => Self::BadgeHit,
            GridOpenBlockReason::NativeDragDrop => Self::NativeDragDrop,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum GridActivationDiagnostic {
    Accepted,
    Ignored(GridActivationIgnoredReason),
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum GridPointerTarget {
    Cell {
        idx: usize,
        item_key: String,
        archive_key: Option<String>,
        item_kind: &'static str,
    },
    Background,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct GridPointerTrace {
    pub(crate) seq: u64,
    pub(crate) target: GridPointerTarget,
    pub(crate) current_folder: Option<String>,
    pub(crate) items_generation: u64,
    pub(crate) press_x: f32,
    pub(crate) press_y: f32,
    pub(crate) terminal_reported: bool,
}

impl GridPointerTrace {
    pub(crate) fn key(&self) -> Option<&str> {
        match &self.target {
            GridPointerTarget::Cell { item_key, .. } => Some(item_key),
            GridPointerTarget::Background => None,
        }
    }

    pub(crate) fn idx(&self) -> Option<usize> {
        match self.target {
            GridPointerTarget::Cell { idx, .. } => Some(idx),
            GridPointerTarget::Background => None,
        }
    }

    pub(crate) fn item_kind(&self) -> &'static str {
        match self.target {
            GridPointerTarget::Cell { item_kind, .. } => item_kind,
            GridPointerTarget::Background => "background",
        }
    }

    fn archive_key(&self) -> Option<&str> {
        match &self.target {
            GridPointerTarget::Cell { archive_key, .. } => archive_key.as_deref(),
            GridPointerTarget::Background => None,
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct GridCellSignal {
    pub(crate) first_click: bool,
    pub(crate) double_clicked: bool,
    pub(crate) time_since_last_click: f32,
    pub(crate) max_double_click_delay: f64,
    pub(crate) clicked_by_primary: bool,
    pub(crate) double_clicked_by_primary: bool,
    pub(crate) drag_started: bool,
    pub(crate) release_pos: Option<(f32, f32)>,
    pub(crate) block_reason: Option<GridOpenBlockReason>,
    pub(crate) activation: GridActivationDiagnostic,
}

#[derive(Clone, Debug)]
pub(crate) struct GridDiagnosticEvent {
    pub(crate) kind: &'static str,
    pub(crate) key: Option<String>,
    pub(crate) seq: u64,
    pub(crate) extras: Vec<(&'static str, Value)>,
}

impl GridDiagnosticEvent {
    pub(crate) fn emit(self) {
        crate::perf::event(
            "grid",
            self.kind,
            self.key.as_deref(),
            self.seq,
            &self.extras,
        );
    }
}

fn target_extras(trace: &GridPointerTrace) -> Vec<(&'static str, Value)> {
    vec![
        ("idx", trace.idx().map(Value::from).unwrap_or(Value::Null)),
        ("item_kind", Value::from(trace.item_kind())),
        (
            "archive_key",
            trace.archive_key().map(Value::from).unwrap_or(Value::Null),
        ),
        (
            "current_folder",
            trace
                .current_folder
                .as_deref()
                .map(Value::from)
                .unwrap_or(Value::Null),
        ),
        ("items_generation", Value::from(trace.items_generation)),
        ("press_x", Value::from(trace.press_x)),
        ("press_y", Value::from(trace.press_y)),
    ]
}

pub(crate) fn pointer_press_events(
    enabled: bool,
    trace: &GridPointerTrace,
) -> Vec<GridDiagnosticEvent> {
    if !enabled {
        return Vec::new();
    }
    vec![GridDiagnosticEvent {
        kind: "pointer_press",
        key: trace.key().map(str::to_owned),
        seq: trace.seq,
        extras: target_extras(trace),
    }]
}

pub(crate) fn cell_signal_events(
    enabled: bool,
    trace: &GridPointerTrace,
    signal: &GridCellSignal,
) -> Vec<GridDiagnosticEvent> {
    if !enabled {
        return Vec::new();
    }
    let mut signal_extras = target_extras(trace);
    signal_extras.extend([
        ("first_click", Value::from(signal.first_click)),
        ("double_clicked", Value::from(signal.double_clicked)),
        (
            "time_since_last_click",
            Value::from(signal.time_since_last_click),
        ),
        (
            "max_double_click_delay",
            Value::from(signal.max_double_click_delay),
        ),
        ("clicked_by_primary", Value::from(signal.clicked_by_primary)),
        (
            "double_clicked_by_primary",
            Value::from(signal.double_clicked_by_primary),
        ),
        ("drag_started", Value::from(signal.drag_started)),
        (
            "release_x",
            signal
                .release_pos
                .map(|(x, _)| Value::from(x))
                .unwrap_or(Value::Null),
        ),
        (
            "release_y",
            signal
                .release_pos
                .map(|(_, y)| Value::from(y))
                .unwrap_or(Value::Null),
        ),
    ]);

    let mut gate_extras = target_extras(trace);
    gate_extras.extend([
        ("allowed", Value::from(signal.block_reason.is_none())),
        (
            "block_reason",
            signal
                .block_reason
                .map(GridOpenBlockReason::as_str)
                .map(Value::from)
                .unwrap_or(Value::Null),
        ),
    ]);

    let mut activation_extras = target_extras(trace);
    let (accepted, ignored_reason) = match signal.activation {
        GridActivationDiagnostic::Accepted => (true, Value::Null),
        GridActivationDiagnostic::Ignored(reason) => (false, Value::from(reason.as_str())),
    };
    activation_extras.extend([
        ("accepted", Value::from(accepted)),
        ("ignored_reason", ignored_reason),
        ("owner", Value::from("main_grid")),
    ]);

    vec![
        GridDiagnosticEvent {
            kind: "cell_signal",
            key: trace.key().map(str::to_owned),
            seq: trace.seq,
            extras: signal_extras,
        },
        GridDiagnosticEvent {
            kind: "open_gate",
            key: trace.key().map(str::to_owned),
            seq: trace.seq,
            extras: gate_extras,
        },
        GridDiagnosticEvent {
            kind: "activation_request",
            key: trace.key().map(str::to_owned),
            seq: trace.seq,
            extras: activation_extras,
        },
    ]
}

pub(crate) fn pointer_release_events(
    enabled: bool,
    trace: &GridPointerTrace,
    release_pos: Option<(f32, f32)>,
) -> Vec<GridDiagnosticEvent> {
    if !enabled {
        return Vec::new();
    }
    let mut extras = target_extras(trace);
    extras.extend([
        (
            "release_x",
            release_pos
                .map(|(x, _)| Value::from(x))
                .unwrap_or(Value::Null),
        ),
        (
            "release_y",
            release_pos
                .map(|(_, y)| Value::from(y))
                .unwrap_or(Value::Null),
        ),
        ("cell_signal_observed", Value::from(trace.terminal_reported)),
    ]);
    vec![GridDiagnosticEvent {
        kind: "pointer_release",
        key: trace.key().map(str::to_owned),
        seq: trace.seq,
        extras,
    }]
}

pub(crate) fn emit_all(events: Vec<GridDiagnosticEvent>) {
    for event in events {
        event.emit();
    }
}

pub(crate) struct GridActivationDispatchGuard {
    event: Option<GridDiagnosticEvent>,
}

impl GridActivationDispatchGuard {
    pub(crate) fn new(trace: &GridPointerTrace) -> Self {
        let mut extras = target_extras(trace);
        extras.push(("owner", Value::from("main_grid")));
        Self {
            event: crate::perf::is_enabled().then(|| GridDiagnosticEvent {
                kind: "activation_dispatch_complete",
                key: trace.key().map(str::to_owned),
                seq: trace.seq,
                extras,
            }),
        }
    }
}

impl Drop for GridActivationDispatchGuard {
    fn drop(&mut self) {
        if let Some(event) = self.event.take() {
            event.emit();
        }
    }
}

pub(crate) fn emit_auto_aspect_switch(
    trace: Option<&GridPointerTrace>,
    current_folder: Option<&str>,
    items_generation: u64,
    old: &str,
    new: &str,
) {
    if !crate::perf::is_enabled() {
        return;
    }
    let (pointer_state, pointer_seq, idx, press_x, press_y) = trace.map_or(
        ("idle", 0, Value::Null, Value::Null, Value::Null),
        |trace| {
            (
                "active",
                trace.seq,
                trace.idx().map(Value::from).unwrap_or(Value::Null),
                Value::from(trace.press_x),
                Value::from(trace.press_y),
            )
        },
    );
    crate::perf::event(
        "grid",
        "auto_aspect_switch",
        current_folder,
        pointer_seq,
        &[
            ("old", Value::from(old)),
            ("new", Value::from(new)),
            ("items_generation", Value::from(items_generation)),
            ("pointer_state", Value::from(pointer_state)),
            ("pointer_idx", idx),
            ("pointer_press_x", press_x),
            ("pointer_press_y", press_y),
        ],
    );
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ArchiveAutoFullscreenPaintTrace {
    pub(crate) seq: u64,
    pub(crate) archive_key: String,
    pub(crate) items_generation: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cell_trace() -> GridPointerTrace {
        GridPointerTrace {
            seq: 77,
            target: GridPointerTarget::Cell {
                idx: 4,
                item_key: "archive::RAR::C:\\books\\sample.rar".to_string(),
                archive_key: Some("c:\\books\\sample.rar".to_string()),
                item_kind: "convertible_archive",
            },
            current_folder: Some("C:\\books".to_string()),
            items_generation: 12,
            press_x: 120.0,
            press_y: 240.0,
            terminal_reported: false,
        }
    }

    #[test]
    fn block_reason_is_a_closed_structured_value() {
        let signal = GridCellSignal {
            first_click: false,
            double_clicked: true,
            time_since_last_click: 0.228,
            max_double_click_delay: 0.5,
            clicked_by_primary: true,
            double_clicked_by_primary: true,
            drag_started: false,
            release_pos: Some((121.0, 241.0)),
            block_reason: Some(GridOpenBlockReason::BadgeHit),
            activation: GridActivationDiagnostic::Ignored(GridActivationIgnoredReason::BadgeHit),
        };
        let events = cell_signal_events(true, &cell_trace(), &signal);
        let gate = events
            .iter()
            .find(|event| event.kind == "open_gate")
            .unwrap();
        assert!(gate.extras.contains(&("allowed", Value::from(false))));
        assert!(
            gate.extras
                .contains(&("block_reason", Value::from("badge_hit")))
        );
    }

    #[test]
    fn missing_click_signal_keeps_the_press_correlation_id() {
        let trace = cell_trace();
        let signal = GridCellSignal {
            first_click: false,
            double_clicked: false,
            time_since_last_click: 0.649,
            max_double_click_delay: 0.5,
            clicked_by_primary: false,
            double_clicked_by_primary: false,
            drag_started: false,
            release_pos: Some((500.0, 500.0)),
            block_reason: None,
            activation: GridActivationDiagnostic::Ignored(
                GridActivationIgnoredReason::NoCellSignal,
            ),
        };
        let events = cell_signal_events(true, &trace, &signal);
        let activation = events
            .iter()
            .find(|event| event.kind == "activation_request")
            .unwrap();
        assert_eq!(activation.seq, 77);
        assert_eq!(activation.key.as_deref(), trace.key());
        assert!(
            activation
                .extras
                .contains(&("ignored_reason", Value::from("no_cell_signal")))
        );
    }

    #[test]
    fn cell_signal_always_reports_egui_click_observations() {
        let cases = [
            GridCellSignal {
                first_click: true,
                double_clicked: false,
                time_since_last_click: 0.228,
                max_double_click_delay: 0.5,
                clicked_by_primary: false,
                double_clicked_by_primary: false,
                drag_started: false,
                release_pos: Some((121.0, 241.0)),
                block_reason: None,
                activation: GridActivationDiagnostic::Ignored(
                    GridActivationIgnoredReason::SelectionOnly,
                ),
            },
            GridCellSignal {
                first_click: true,
                double_clicked: true,
                time_since_last_click: 0.4,
                max_double_click_delay: 0.5,
                clicked_by_primary: true,
                double_clicked_by_primary: true,
                drag_started: false,
                release_pos: Some((121.0, 241.0)),
                block_reason: None,
                activation: GridActivationDiagnostic::Accepted,
            },
        ];
        assert!(cases[0].first_click);
        assert!(!cases[0].clicked_by_primary);

        for signal in cases {
            let events = cell_signal_events(true, &cell_trace(), &signal);
            let cell_signal = events
                .iter()
                .find(|event| event.kind == "cell_signal")
                .unwrap();
            assert!(
                cell_signal
                    .extras
                    .contains(&("first_click", Value::from(signal.first_click)))
            );
            assert!(cell_signal.extras.contains(&(
                "time_since_last_click",
                Value::from(signal.time_since_last_click)
            )));
            assert!(cell_signal.extras.contains(&(
                "max_double_click_delay",
                Value::from(signal.max_double_click_delay)
            )));
            assert!(
                cell_signal
                    .extras
                    .contains(&("clicked_by_primary", Value::from(signal.clicked_by_primary),))
            );
            assert!(cell_signal.extras.contains(&(
                "double_clicked_by_primary",
                Value::from(signal.double_clicked_by_primary),
            )));
        }
    }

    #[test]
    fn perf_off_builds_no_grid_events() {
        let trace = cell_trace();
        assert!(pointer_press_events(false, &trace).is_empty());
        assert!(
            cell_signal_events(
                false,
                &trace,
                &GridCellSignal {
                    first_click: true,
                    double_clicked: false,
                    time_since_last_click: 0.2,
                    max_double_click_delay: 0.5,
                    clicked_by_primary: true,
                    double_clicked_by_primary: false,
                    drag_started: false,
                    release_pos: None,
                    block_reason: None,
                    activation: GridActivationDiagnostic::Ignored(
                        GridActivationIgnoredReason::SelectionOnly,
                    ),
                },
            )
            .is_empty()
        );
        assert!(pointer_release_events(false, &trace, None).is_empty());
    }
}
