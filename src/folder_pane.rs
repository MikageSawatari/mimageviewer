//! Left-side filesystem folder tree pane state.
//!
//! This pane is intentionally separate from `folder_tree`: the latter is the
//! Ctrl+Up/Down DFS navigator and treats ZIP/PDF files as virtual folders.  The
//! pane here shows only real filesystem directories.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};

use crate::settings::{FolderTreeSortOrder, Settings};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct FolderPaneListingOptions {
    pub(crate) sort_order: FolderTreeSortOrder,
    pub(crate) show_hidden_files: bool,
}

impl FolderPaneListingOptions {
    pub(crate) fn from_settings(settings: &Settings) -> Self {
        Self {
            sort_order: settings.folder_tree_sort_order,
            show_hidden_files: settings.show_hidden_files,
        }
    }
}

impl Default for FolderPaneListingOptions {
    fn default() -> Self {
        Self {
            sort_order: FolderTreeSortOrder::default(),
            show_hidden_files: false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FolderPaneTreeKey {
    Up,
    Down,
    Left,
    Right,
    Enter,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum FolderPaneCommand {
    Open(PathBuf),
}

/// The last name activation in one pass replaces every earlier name activation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum FolderPaneActivation {
    Open(PathBuf),
    RefreshActive(PathBuf),
}

/// Diagnostic causes only; none of these participate in gesture adjudication.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FolderPaneGestureReset {
    FocusGrid,
    SuspendDisplay,
    ReloadForActive,
    SelectDrive,
    SelectedDriveRemoved,
    DisplayNotVisible,
    DisplayNotFresh,
    TreeFocusLost,
    TreeKeyPressed,
    ContinuationMainChromeBlocked { block_reason: Option<&'static str> },
    PointerWitnessMissing,
    PointerWitnessMismatch,
    ReleaseWitnessMissing,
    ReleaseButtonMismatch,
    ExtraPointerOrReleaseWitness,
    PaneDisabled,
    SingleMainChromeBlocked { block_reason: Option<&'static str> },
    ArrowMainChromeBlocked { block_reason: Option<&'static str> },
    ToggleMainChromeBlocked { block_reason: Option<&'static str> },
    NonPrimaryArrowActivation,
    NonPrimaryLabelActivation,
    InputCancelled,
    PressRejected,
    ReleaseRejected,
    TouchSingleCompleted,
    ArrowCompleted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FolderPaneGestureNotContinued {
    MixedInputSourceWhilePressed,
    ArrowWidgetHit,
    ArrowRectHit,
    InputSourceNotMouse,
    ContinueNotAllowed,
    DistanceExceeded,
    ReleaseTimeOutsideDoubleClickDelay,
    PreviousPressNotReleased,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FolderPaneGestureInputDiscard {
    NonRootViewport,
    PassAlreadyRecognized,
    WindowFocusLost,
    TouchCancelled,
    NonPrimaryButton,
    AmbiguousTouchBatch,
    SuppressedPointerEdge,
}

/// Diagnostic explanation of the UI-side continuation gate.
#[derive(Debug, Clone, Copy)]
pub(crate) enum FolderPaneGestureContinuationGate {
    MainChromeBlocked,
    LayerRejected,
    OutsideDisplayRect,
    HitRejected,
}

/// The sole owner of an ordered primary-pointer gesture in the tree.
#[derive(Debug, Default)]
enum FolderPaneLabelGesture {
    #[default]
    Idle,
    FirstPress {
        path: PathBuf,
        pos: egui::Pos2,
        source: FolderPanePointerSource,
    },
    AwaitingPress {
        anchor: FolderPaneLabelAnchor,
        time: f64,
    },
    PressAccepted(FolderPaneLabelAnchor),
    ArrowPress {
        path: PathBuf,
        source: FolderPanePointerSource,
    },
}

impl FolderPaneLabelGesture {
    fn diagnostic_target(&self) -> Option<(&'static str, &Path)> {
        match self {
            Self::Idle => None,
            Self::FirstPress { path, .. } => Some(("FirstPress", path)),
            Self::AwaitingPress { anchor, .. } => Some(("AwaitingPress", &anchor.path)),
            Self::PressAccepted(anchor) => Some(("PressAccepted", &anchor.path)),
            Self::ArrowPress { path, .. } => Some(("ArrowPress", path)),
        }
    }

    /// Mirrors the first rejecting branch in Press, using only pure comparisons.
    /// The actual transition below deliberately keeps its original conditions.
    fn not_continued(
        &self,
        source: FolderPanePointerSource,
        hit: &FolderPanePointerHit,
        arrow_rect: bool,
        continue_allowed: bool,
        pos: egui::Pos2,
        time: f64,
        max_dist: f32,
        max_delay: f64,
    ) -> Option<FolderPaneGestureNotContinued> {
        use FolderPaneGestureNotContinued::*;
        match self {
            Self::Idle => return None,
            Self::FirstPress {
                source: previous, ..
            }
            | Self::ArrowPress {
                source: previous, ..
            } if source != *previous => {
                return Some(MixedInputSourceWhilePressed);
            }
            Self::PressAccepted(_) if source != FolderPanePointerSource::Mouse => {
                return Some(MixedInputSourceWhilePressed);
            }
            _ => {}
        }
        if matches!(hit, FolderPanePointerHit::Arrow(_)) {
            return Some(ArrowWidgetHit);
        }
        if arrow_rect {
            return Some(ArrowRectHit);
        }
        if let Self::AwaitingPress {
            anchor,
            time: released,
        } = self
        {
            if source != FolderPanePointerSource::Mouse {
                Some(InputSourceNotMouse)
            } else if !continue_allowed {
                Some(ContinueNotAllowed)
            } else if !(pos.distance(anchor.pos) <= max_dist) {
                Some(DistanceExceeded)
            } else if !(0.0..=max_delay).contains(&(time - released)) {
                Some(ReleaseTimeOutsideDoubleClickDelay)
            } else {
                None
            }
        } else {
            Some(PreviousPressNotReleased)
        }
    }
}

fn log_folder_pane_gesture(
    kind: &str,
    reason: impl std::fmt::Debug,
    previous: &str,
    path: &Path,
    pass: Option<u64>,
    time: Option<f64>,
    details: std::fmt::Arguments<'_>,
) {
    let reason = format!("{reason:?}");
    let details = details.to_string();
    crate::logger::log(format!(
        "folder pane gesture {}: {} prev={} path={} pass={:?} t={:?} {}",
        kind.replace('_', " "),
        reason,
        previous,
        path.display(),
        pass,
        time,
        details,
    ));
    if crate::perf::is_enabled() {
        let path = path.to_string_lossy();
        crate::perf::event(
            "folder_pane",
            &format!("gesture_{kind}"),
            Some(path.as_ref()),
            0,
            &[
                ("reason", reason.into()),
                ("prev", previous.into()),
                ("pass", serde_json::json!(pass)),
                ("input_time", serde_json::json!(time)),
                ("details", details.into()),
            ],
        );
    }
}

#[derive(Debug)]
struct FolderPaneLabelAnchor {
    path: PathBuf,
    first_expanded: bool,
    pos: egui::Pos2,
    toggled: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FolderPanePointerSource {
    Mouse,
    Touch,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum FolderPanePointerHit {
    Label(PathBuf),
    Arrow(PathBuf),
    None,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum FolderPaneLabelResult {
    Single(PathBuf),
    Arrow(PathBuf),
    Toggle { path: PathBuf, expanded: bool },
}

/// Rebuilt at the tree drawing boundary; never used after a skipped frame.
pub(crate) struct FolderPaneLabelDisplay {
    pub pass: u64,
    pub layer: egui::LayerId,
    pub pane_rect: egui::Rect,
    pub rect: egui::Rect,
    pub rows: Vec<FolderPaneLabelWidgets>,
}

pub(crate) struct FolderPaneLabelWidgets {
    pub path: PathBuf,
    pub row_rect: egui::Rect,
    pub label: egui::Id,
    pub label_rect: egui::Rect,
    pub arrow: egui::Id,
    pub arrow_rect: egui::Rect,
}

#[derive(Default)]
struct FolderPaneLabelInputPass {
    pass: Option<u64>,
    results: Vec<FolderPaneLabelResult>,
    pointer_activations: HashSet<egui::Id>,
}

/// Witness acceptance, modal eligibility and touch provenance come from ROOT input.
pub(crate) enum FolderPaneLabelInput {
    Press {
        pos: egui::Pos2,
        hit: FolderPanePointerHit,
        start_allowed: bool,
        continue_allowed: bool,
        arrow_rect: bool,
        source: FolderPanePointerSource,
    },
    Release {
        hit: FolderPanePointerHit,
        start_allowed: bool,
        clicked: bool,
        source: FolderPanePointerSource,
    },
    Cancel,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FolderPaneRow {
    pub path: PathBuf,
    pub depth: usize,
    pub expanded: bool,
    pub loading: bool,
    pub has_children_or_unknown: bool,
    pub is_active: bool,
    pub is_cursor: bool,
    pub error: Option<String>,
}

/// Only a complete, successful enumeration can prove that a branch disappeared.
#[derive(Debug, Clone)]
struct CachedListing {
    children: Vec<PathBuf>,
    child_keys: HashSet<String>,
    membership: HashSet<String>,
    options: FolderPaneListingOptions,
}

impl CachedListing {
    fn new(
        children: Vec<PathBuf>,
        membership: HashSet<String>,
        options: FolderPaneListingOptions,
    ) -> Self {
        let child_keys = children.iter().map(|child| key_for(child)).collect();
        Self {
            children,
            child_keys,
            membership,
            options,
        }
    }
}

/// One index shared by the tree's egui temp data and its lifecycle owner.
/// Only drawing adds IDs; display suspension, drive changes and reload clear it.
#[derive(Clone, Default)]
pub(crate) struct FolderPaneRowIds(
    pub(crate) Arc<std::sync::Mutex<HashMap<egui::accesskit::NodeId, Arc<PathBuf>>>>,
);

impl FolderPaneRowIds {
    fn clear(&self) {
        self.0.lock().unwrap().clear();
    }
}

struct FolderPaneNode {
    path: PathBuf,
    cache: Option<CachedListing>,
    scan: NodeScan,
}

enum NodeScan {
    Unverified,
    Queued,
    Running(FolderPaneScanPending, FollowUp),
    Settled {
        options: FolderPaneListingOptions,
        error: Option<String>,
    },
}

#[derive(Clone, Copy)]
enum FollowUp {
    None,
    Recheck,
}

impl FolderPaneNode {
    fn placeholder(path: PathBuf) -> Self {
        Self {
            path,
            cache: None,
            scan: NodeScan::Unverified,
        }
    }

    fn children(&self) -> &[PathBuf] {
        self.cache
            .as_ref()
            .map_or(&[], |cache| cache.children.as_slice())
    }
}

struct FolderPaneScanPending {
    cancel: Arc<AtomicBool>,
    rx: mpsc::Receiver<Result<CachedListing, String>>,
    options: FolderPaneListingOptions,
}

impl Drop for FolderPaneScanPending {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
        // A completed listing may already be queued. Retire its payload off the UI thread.
        let rx = std::mem::replace(&mut self.rx, mpsc::channel().1);
        rayon::spawn(move || drop(rx));
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum PaneParticipation {
    Suspended,
    Displayed,
}

/// One cursor reveal, including listings adopted while the pane is opening.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum FolderPaneCursorScroll {
    #[default]
    Idle,
    Requested,
    FollowingRefresh,
}

#[derive(Clone, Default, PartialEq, Eq)]
struct FolderPanePosition {
    drive: Option<PathBuf>,
    cursor: Option<PathBuf>,
}

/// Focus routes input; this owner alone decides whether active changes may move
/// the displayed drive/cursor. A default root is not an explicit user choice.
enum FolderPaneSelection {
    FollowActive(FolderPanePosition),
    Explicit(FolderPanePosition),
    // Grid focus permits the next active change to take over, but F may still
    // commit this user-selected position until that change actually happens.
    ExplicitUntilActiveChange(FolderPanePosition),
}

impl FolderPaneSelection {
    fn position(&self) -> &FolderPanePosition {
        match self {
            Self::FollowActive(position)
            | Self::Explicit(position)
            | Self::ExplicitUntilActiveChange(position) => position,
        }
    }

    fn position_mut(&mut self) -> &mut FolderPanePosition {
        match self {
            Self::FollowActive(position)
            | Self::Explicit(position)
            | Self::ExplicitUntilActiveChange(position) => position,
        }
    }

    fn follows_active(&self) -> bool {
        !matches!(self, Self::Explicit(_))
    }

    fn has_explicit_position(&self) -> bool {
        !matches!(self, Self::FollowActive(_))
    }

    fn follow_next_active_change(&mut self) {
        if let Self::Explicit(position) = self {
            *self = Self::ExplicitUntilActiveChange(position.clone());
        }
    }

    fn make_explicit(&mut self) {
        if let Self::FollowActive(position) | Self::ExplicitUntilActiveChange(position) = self {
            *self = Self::Explicit(position.clone());
        }
    }
}

#[cfg(test)]
type TestScanBackend = Arc<
    dyn Fn(&Path, FolderPaneListingOptions, &AtomicBool) -> Result<CachedListing, String>
        + Send
        + Sync,
>;

pub(crate) struct FolderPaneState {
    pub has_focus: bool,
    pub(crate) cursor_scroll: FolderPaneCursorScroll,
    selection: FolderPaneSelection,
    active_path: Option<PathBuf>,
    active_key: Option<String>,
    drives: Vec<PathBuf>,
    nodes: HashMap<String, FolderPaneNode>,
    user_expanded: HashSet<String>,
    auto_expanded: HashSet<String>,
    user_collapsed: HashSet<String>,
    demand: HashSet<String>,
    participation: PaneParticipation,
    listing_options: FolderPaneListingOptions,
    last_drive_refresh: Option<std::time::Instant>,
    label_gesture: FolderPaneLabelGesture,
    pub(crate) row_ids: FolderPaneRowIds,
    pub(crate) label_display: Option<FolderPaneLabelDisplay>,
    label_input: FolderPaneLabelInputPass,
    // Latest ROOT input.time (also set by direct state tests), never read by
    // adjudication. Lifecycle resets outside recognition use this last sample.
    label_diagnostic_time: Option<f64>,
    #[cfg(test)]
    last_gesture_reset: Option<FolderPaneGestureReset>,
    #[cfg(test)]
    last_gesture_not_continued: Option<FolderPaneGestureNotContinued>,
    #[cfg(test)]
    scan_backend: Option<TestScanBackend>,
}

impl Default for FolderPaneState {
    fn default() -> Self {
        let drives = crate::known_folders::available_drives();
        Self {
            has_focus: false,
            cursor_scroll: FolderPaneCursorScroll::Idle,
            selection: FolderPaneSelection::FollowActive(FolderPanePosition {
                drive: drives.first().cloned(),
                cursor: None,
            }),
            drives,
            active_path: None,
            active_key: None,
            nodes: HashMap::new(),
            user_expanded: HashSet::new(),
            auto_expanded: HashSet::new(),
            user_collapsed: HashSet::new(),
            demand: HashSet::new(),
            participation: PaneParticipation::Suspended,
            listing_options: FolderPaneListingOptions::default(),
            last_drive_refresh: None,
            label_gesture: FolderPaneLabelGesture::Idle,
            row_ids: FolderPaneRowIds::default(),
            label_diagnostic_time: None,
            #[cfg(test)]
            last_gesture_reset: None,
            #[cfg(test)]
            last_gesture_not_continued: None,
            label_display: None,
            label_input: FolderPaneLabelInputPass::default(),
            #[cfg(test)]
            scan_backend: None,
        }
    }
}

impl FolderPaneState {
    #[track_caller]
    pub(crate) fn clear_label_click(&mut self, reason: FolderPaneGestureReset) {
        self.log_gesture_reset(reason);
        self.label_gesture = FolderPaneLabelGesture::Idle;
    }

    #[track_caller]
    fn log_gesture_reset(&mut self, reason: FolderPaneGestureReset) {
        if let Some((previous, path)) = self.label_gesture.diagnostic_target() {
            log_folder_pane_gesture(
                "reset",
                reason,
                previous,
                path,
                self.label_input.pass,
                self.label_diagnostic_time,
                format_args!(
                    "has_focus={} display_pass={:?} caller={}",
                    self.has_focus,
                    self.label_display.as_ref().map(|display| display.pass),
                    std::panic::Location::caller(),
                ),
            );
            #[cfg(test)]
            {
                self.last_gesture_reset = Some(reason);
            }
        }
    }

    pub(crate) fn set_label_diagnostic_time(&mut self, time: f64) {
        self.label_diagnostic_time = Some(time);
    }

    pub(crate) fn log_label_input_discard(
        &self,
        reason: FolderPaneGestureInputDiscard,
        pass: u64,
        time: f64,
        details: std::fmt::Arguments<'_>,
    ) {
        if let Some((previous, path)) = self.label_gesture.diagnostic_target() {
            log_folder_pane_gesture(
                "input_discard",
                reason,
                previous,
                path,
                Some(pass),
                Some(time),
                details,
            );
        }
    }

    pub(crate) fn log_label_continuation_gate(
        &self,
        reason: FolderPaneGestureContinuationGate,
        details: &str,
    ) {
        if let Some((previous, path)) = self.label_gesture.diagnostic_target() {
            log_folder_pane_gesture(
                "continuation_gate",
                reason,
                previous,
                path,
                self.label_input.pass,
                self.label_diagnostic_time,
                format_args!("{details}"),
            );
        }
    }

    pub(crate) fn label_gesture_requires_tree_focus(&self) -> bool {
        matches!(
            self.label_gesture,
            FolderPaneLabelGesture::AwaitingPress { .. } | FolderPaneLabelGesture::PressAccepted(_)
        )
    }

    #[track_caller]
    pub(crate) fn invalidate_label_display(&mut self, reason: FolderPaneGestureReset) {
        if matches!(
            reason,
            FolderPaneGestureReset::SuspendDisplay
                | FolderPaneGestureReset::SelectedDriveRemoved
                | FolderPaneGestureReset::SelectDrive
                | FolderPaneGestureReset::ReloadForActive
        ) {
            self.row_ids.clear();
        }
        self.clear_label_click(reason);
        self.label_display = None;
        self.label_input.results.clear();
        self.label_input.pointer_activations.clear();
    }

    pub(crate) fn begin_label_input(&mut self, pass: u64) -> bool {
        if self.label_input.pass == Some(pass) {
            return false;
        }
        self.label_input = FolderPaneLabelInputPass {
            pass: Some(pass),
            ..Default::default()
        };
        true
    }

    pub(crate) fn take_label_results(&mut self, pass: u64) -> Vec<FolderPaneLabelResult> {
        if self.label_input.pass == Some(pass) {
            std::mem::take(&mut self.label_input.results)
        } else {
            Vec::new()
        }
    }

    pub(crate) fn finish_label_input(&mut self) {
        self.label_input.results.clear();
        self.label_input.pointer_activations.clear();
    }

    pub(crate) fn record_label_pointer_activation(&mut self, widget: egui::Id) {
        self.label_input.pointer_activations.insert(widget);
    }

    pub(crate) fn label_pointer_activation_owned(&self, widget: egui::Id) -> bool {
        self.label_input.pointer_activations.contains(&widget)
    }

    /// Recognition only: expansion, focus, cursor and navigation belong to drawing.
    pub(crate) fn process_label_input(
        &mut self,
        input: FolderPaneLabelInput,
        time: f64,
        max_dist: f32,
        max_delay: f64,
    ) -> bool {
        use FolderPaneLabelGesture::*;
        use FolderPanePointerHit as Hit;
        self.label_diagnostic_time = Some(time);
        if let FolderPaneLabelInput::Press {
            pos,
            ref hit,
            source,
            arrow_rect,
            continue_allowed,
            start_allowed,
        } = input
            && let Some(reason) = self.label_gesture.not_continued(
                source,
                hit,
                arrow_rect,
                continue_allowed,
                pos,
                time,
                max_dist,
                max_delay,
            )
        {
            if let Some((previous, path)) = self.label_gesture.diagnostic_target() {
                let (distance, delay) = match &self.label_gesture {
                    AwaitingPress {
                        anchor,
                        time: released,
                    } => (Some(pos.distance(anchor.pos)), Some(time - released)),
                    _ => (None, None),
                };
                log_folder_pane_gesture(
                    "not_continued",
                    reason,
                    previous,
                    path,
                    self.label_input.pass,
                    Some(time),
                    format_args!(
                        "source={source:?} hit={hit:?} pos={pos:?} start_allowed={start_allowed} continue_allowed={continue_allowed} arrow_rect={arrow_rect} distance={distance:?} max_dist={max_dist} delay={delay:?} max_delay={max_delay}"
                    ),
                );
            }
            #[cfg(test)]
            {
                self.last_gesture_not_continued = Some(reason);
            }
        }
        let mut idle_reason = match &input {
            FolderPaneLabelInput::Cancel => FolderPaneGestureReset::InputCancelled,
            FolderPaneLabelInput::Press { .. } => FolderPaneGestureReset::PressRejected,
            FolderPaneLabelInput::Release { .. } => FolderPaneGestureReset::ReleaseRejected,
        };
        let input_details = self.label_gesture.diagnostic_target().map(|_| match &input {
            FolderPaneLabelInput::Cancel => "input=Cancel".to_string(),
            FolderPaneLabelInput::Press {
                pos, hit, source, start_allowed, continue_allowed, arrow_rect,
            } => format!(
                "input=Press source={source:?} hit={hit:?} pos={pos:?} start_allowed={start_allowed} continue_allowed={continue_allowed} arrow_rect={arrow_rect}"
            ),
            FolderPaneLabelInput::Release { hit, source, start_allowed, clicked } => format!(
                "input=Release source={source:?} hit={hit:?} start_allowed={start_allowed} clicked={clicked}"
            ),
        });
        let prior_results = self.label_input.results.len();
        let mut continuation_owned = false;
        let previous = std::mem::take(&mut self.label_gesture);
        let previous_target = previous
            .diagnostic_target()
            .map(|(kind, path)| (kind, path.to_path_buf()));
        self.label_gesture = match input {
            FolderPaneLabelInput::Cancel => Idle,
            FolderPaneLabelInput::Press {
                pos,
                hit,
                start_allowed,
                continue_allowed,
                arrow_rect,
                source,
            } => {
                // A second input source cannot acquire an unreleased press.
                // A completed mouse click may still be followed by an ordinary tap.
                let mixed_press = match &previous {
                    FirstPress {
                        source: pressed_source,
                        ..
                    }
                    | ArrowPress {
                        source: pressed_source,
                        ..
                    } => source != *pressed_source,
                    PressAccepted(_) => source != FolderPanePointerSource::Mouse,
                    _ => false,
                };
                // Arrow geometry also terminates a name gesture when disabled
                // widgets have no accepted witness during sidecar restore.
                if mixed_press {
                    Idle
                } else if let Hit::Arrow(path) = hit {
                    if start_allowed {
                        ArrowPress { path, source }
                    } else {
                        Idle
                    }
                } else if arrow_rect {
                    Idle
                } else if let AwaitingPress {
                    mut anchor,
                    time: released,
                } = previous
                    && source == FolderPanePointerSource::Mouse
                    && continue_allowed
                    && pos.distance(anchor.pos) <= max_dist
                    && (0.0..=max_delay).contains(&(time - released))
                {
                    continuation_owned = true;
                    if !anchor.toggled {
                        self.label_input
                            .results
                            .push(FolderPaneLabelResult::Toggle {
                                path: anchor.path.clone(),
                                expanded: !anchor.first_expanded,
                            });
                        log_folder_pane_gesture(
                            "toggle",
                            "SecondPressAccepted",
                            "AwaitingPress",
                            &anchor.path,
                            self.label_input.pass,
                            Some(time),
                            format_args!(
                                "expanded={} source={source:?} pos={pos:?} distance={} delay={}",
                                !anchor.first_expanded,
                                pos.distance(anchor.pos),
                                time - released,
                            ),
                        );
                        anchor.toggled = true;
                    } else {
                        log_folder_pane_gesture(
                            "continued",
                            "AlreadyToggled",
                            "AwaitingPress",
                            &anchor.path,
                            self.label_input.pass,
                            Some(time),
                            format_args!("source={source:?}"),
                        );
                    }
                    PressAccepted(anchor)
                } else if start_allowed && let Hit::Label(path) = hit {
                    FirstPress { path, pos, source }
                } else {
                    Idle
                }
            }
            FolderPaneLabelInput::Release {
                hit,
                start_allowed,
                clicked,
                source,
            } => match previous {
                PressAccepted(anchor) if source == FolderPanePointerSource::Mouse => {
                    continuation_owned = true;
                    AwaitingPress { anchor, time }
                }
                FirstPress {
                    path,
                    pos,
                    source: pressed_source,
                } if source == pressed_source
                    && clicked
                    && start_allowed
                    && matches!(&hit, Hit::Label(hit) if crate::folder_tree::path_eq(hit, &path)) =>
                {
                    let first_expanded = self.is_expanded_key(&key_for(&path));
                    self.label_input
                        .results
                        .push(FolderPaneLabelResult::Single(path.clone()));
                    log_folder_pane_gesture(
                        "single",
                        "FirstReleaseAccepted",
                        "FirstPress",
                        &path,
                        self.label_input.pass,
                        Some(time),
                        format_args!(
                            "source={source:?} pos={pos:?} first_expanded={first_expanded}"
                        ),
                    );
                    if source == FolderPanePointerSource::Touch {
                        idle_reason = FolderPaneGestureReset::TouchSingleCompleted;
                        Idle
                    } else {
                        AwaitingPress {
                            anchor: FolderPaneLabelAnchor {
                                path,
                                first_expanded,
                                pos,
                                toggled: false,
                            },
                            time,
                        }
                    }
                }
                ArrowPress {
                    path,
                    source: pressed_source,
                } if source == pressed_source
                    && clicked
                    && start_allowed
                    && matches!(&hit, Hit::Arrow(hit) if crate::folder_tree::path_eq(hit, &path)) =>
                {
                    self.label_input
                        .results
                        .push(FolderPaneLabelResult::Arrow(path));
                    idle_reason = FolderPaneGestureReset::ArrowCompleted;
                    Idle
                }
                _ => Idle,
            },
        };
        if matches!(self.label_gesture, Idle)
            && let Some((previous, path)) = previous_target
        {
            log_folder_pane_gesture(
                "reset",
                idle_reason,
                previous,
                &path,
                self.label_input.pass,
                Some(time),
                format_args!("{}", input_details.as_deref().unwrap_or_default()),
            );
            #[cfg(test)]
            {
                self.last_gesture_reset = Some(idle_reason);
            }
        }
        continuation_owned || self.label_input.results.len() > prior_results
    }

    /// The only expansion writer shared by arrows, tree keys and mouse gestures.
    /// It owns pane listings, never focus, cursor, reveal or viewer navigation.
    pub(crate) fn set_branch_expanded(&mut self, path: &Path, expanded: bool) {
        let key = key_for(path);
        if self.is_expanded_key(&key) == expanded {
            return;
        }
        if expanded {
            self.user_expanded.insert(key.clone());
            self.user_collapsed.remove(&key);
            self.ensure_node(path.to_path_buf());
            self.request_recheck(path);
        } else {
            self.user_expanded.remove(&key);
            if self.auto_expanded.contains(&key) {
                self.user_collapsed.insert(key.clone());
            }
            // The root stays demanded, but reopening it must recheck too.
            if let Some(node) = self.nodes.get_mut(&key) {
                node.scan = NodeScan::Unverified;
            }
        }
        self.reconcile_demand();
    }

    pub(crate) fn selected_drive(&self) -> Option<&Path> {
        self.selection.position().drive.as_deref()
    }

    fn cursor_path(&self) -> Option<&Path> {
        self.selection.position().cursor.as_deref()
    }

    fn clear_row_ids_on_drive_change(&self, drive: Option<&Path>) {
        if self.selected_drive().map(key_for) != drive.map(key_for) {
            self.row_ids.clear();
        }
    }

    fn resolve_follow_position(&mut self) {
        let drive = self
            .active_path
            .as_deref()
            .and_then(root_of_path)
            .or_else(|| self.selection.position().drive.clone())
            .or_else(|| self.drives.first().cloned());
        let cursor = self.active_path.clone().or_else(|| drive.clone());
        self.clear_row_ids_on_drive_change(drive.as_deref());
        self.selection = FolderPaneSelection::FollowActive(FolderPanePosition { drive, cursor });
    }
    pub(crate) fn drives(&self) -> &[PathBuf] {
        &self.drives
    }
    pub(crate) fn active_path(&self) -> Option<&Path> {
        self.active_path.as_deref()
    }
    pub(crate) fn has_pending(&self) -> bool {
        self.nodes
            .values()
            .any(|node| matches!(node.scan, NodeScan::Queued | NodeScan::Running(..)))
    }

    /// Only the actual main chrome drawing boundary resumes participation.
    pub(crate) fn resume_display(&mut self) {
        self.participation = PaneParticipation::Displayed;
    }

    #[track_caller]
    pub(crate) fn suspend_display(&mut self) {
        self.invalidate_label_display(FolderPaneGestureReset::SuspendDisplay);
        if self.participation == PaneParticipation::Suspended {
            return;
        }
        self.participation = PaneParticipation::Suspended;
        self.cancel_pending();
        self.demand.clear();
    }

    pub(crate) fn set_focus_tree(&mut self) {
        self.has_focus = true;
        if self.cursor_path().is_none() {
            self.selection.position_mut().cursor = self
                .active_path
                .clone()
                .or_else(|| self.selection.position().drive.clone());
        }
        self.cursor_scroll = FolderPaneCursorScroll::Requested;
    }

    pub(crate) fn set_focus_tree_at_active(&mut self) {
        self.resolve_follow_position();
        if let Some(active) = self.active_path.clone() {
            if let Some(root) = self.selected_drive() {
                for ancestor in ancestor_chain(root, &active) {
                    self.user_collapsed.remove(&key_for(&ancestor));
                }
            }
            self.reveal_active(false);
        }
        self.set_focus_tree();
    }

    #[track_caller]
    pub(crate) fn set_focus_grid(&mut self) {
        self.clear_label_click(FolderPaneGestureReset::FocusGrid);
        self.release_tree_keyboard_focus();
    }

    /// Apply a key already adjudicated before this pass's pointer sequence.
    pub(crate) fn release_tree_keyboard_focus(&mut self) {
        self.has_focus = false;
        // Preserve the shown cursor until the next location change, as before.
        self.selection.follow_next_active_change();
    }

    pub(crate) fn cursor_nav_target_if_moved(&self) -> Option<PathBuf> {
        if !self.selection.has_explicit_position() {
            return None;
        }
        let cursor = self.cursor_path()?;
        if self
            .active_path
            .as_deref()
            .is_some_and(|active| crate::folder_tree::path_eq(active, cursor))
        {
            return None;
        }
        Some(cursor.to_path_buf())
    }

    pub(crate) fn refresh_drives(&mut self) {
        let now = std::time::Instant::now();
        if self
            .last_drive_refresh
            .is_some_and(|last| now.duration_since(last) < std::time::Duration::from_millis(1500))
        {
            return;
        }
        self.last_drive_refresh = Some(now);
        let drives = crate::known_folders::available_drives();
        self.apply_drive_list(drives);
    }

    fn apply_drive_list(&mut self, drives: Vec<PathBuf>) {
        if drives == self.drives {
            return;
        }
        self.drives = drives;
        if self.selected_drive().is_some_and(|selected| {
            self.drives
                .iter()
                .any(|drive| crate::folder_tree::path_eq(drive, selected))
        }) {
            return;
        }
        self.invalidate_label_display(FolderPaneGestureReset::SelectedDriveRemoved);
        if !self.selection.has_explicit_position() && self.active_path.is_some() {
            self.resolve_follow_position();
        } else {
            let drive = self.drives.first().cloned();
            *self.selection.position_mut() = FolderPanePosition {
                cursor: drive.clone(),
                drive,
            };
        }
        self.cursor_scroll = FolderPaneCursorScroll::Requested;
    }

    #[track_caller]
    pub(crate) fn select_drive(&mut self, drive: PathBuf) {
        self.invalidate_label_display(FolderPaneGestureReset::SelectDrive);
        self.has_focus = true;
        // An actual selection, including the selected drive, is an explicit recheck.
        // Invalidate all participants, so an unchanged root also refreshes its open branches.
        self.cancel_pending();
        self.demand.clear();
        self.selection = FolderPaneSelection::Explicit(FolderPanePosition {
            drive: Some(drive.clone()),
            cursor: Some(drive.clone()),
        });
        self.cursor_scroll = FolderPaneCursorScroll::Requested;
        self.ensure_node(drive);
        self.reconcile_demand();
    }

    #[track_caller]
    pub(crate) fn reload_for_active(
        &mut self,
        active: Option<&Path>,
        options: FolderPaneListingOptions,
    ) {
        self.invalidate_label_display(FolderPaneGestureReset::ReloadForActive);
        self.cancel_pending();
        let retired = std::mem::take(&mut self.nodes);
        rayon::spawn(move || drop(retired));
        self.demand.clear();
        self.user_expanded.clear();
        self.auto_expanded.clear();
        self.user_collapsed.clear();
        self.active_key = None;
        self.listing_options = options;
        self.selection.follow_next_active_change();
        self.sync_to_active(active, options);
        self.resolve_follow_position();
        self.cursor_scroll = FolderPaneCursorScroll::Requested;
    }

    pub(crate) fn sync_to_active(
        &mut self,
        active: Option<&Path>,
        options: FolderPaneListingOptions,
    ) {
        self.refresh_drives();
        if self.listing_options != options {
            self.cancel_pending();
            self.listing_options = options;
        }
        let active = active.and_then(active_filesystem_folder);
        let key = active.as_ref().map(|path| key_for(path));
        let changed = self.active_key != key;
        if changed {
            self.auto_expanded.clear();
            self.user_collapsed.clear();
            self.active_key = key;
        }
        self.active_path = active;
        if self.selection.follows_active() && (changed || self.cursor_path().is_none()) {
            let previous = self.selection.position().clone();
            self.resolve_follow_position();
            if changed || self.selection.position() != &previous {
                self.cursor_scroll = FolderPaneCursorScroll::Requested;
            }
        }
        self.reveal_active(changed);
        if let Some(root) = self.selection.position().drive.clone() {
            self.ensure_node(root);
        }
        self.reconcile_demand();
        self.start_queued();
    }

    fn reveal_active(&mut self, changed: bool) {
        let Some(active) = self.active_path.clone() else {
            return;
        };
        let Some(root) = root_of_path(&active) else {
            return;
        };
        // The pair is resolved by active changes or explicit input, never by
        // this cache-reveal pass (including after focus returns to the grid).
        if self
            .selected_drive()
            .is_none_or(|drive| !crate::folder_tree::path_eq(drive, &root))
        {
            return;
        }
        self.ensure_node(root.clone());
        for pair in ancestor_chain(&root, &active).windows(2) {
            let parent_key = key_for(&pair[0]);
            self.auto_expanded.insert(parent_key.clone());
            if self.user_collapsed.contains(&parent_key) {
                break;
            }
            if let Some(cache) = self
                .nodes
                .get(&parent_key)
                .and_then(|node| node.cache.as_ref())
            {
                if !cache.membership.contains(&key_for(&pair[1])) {
                    // A NEW location may have been created after this parent's last scan.
                    // Retry this first contradiction once, never on same-location frame sync.
                    if changed {
                        self.request_recheck(&pair[0]);
                    }
                    break;
                }
                if !cache.child_keys.contains(&key_for(&pair[1])) {
                    break;
                }
            }
            self.ensure_node(pair[1].clone());
        }
    }

    /// Recheck only the existing displayed demand. Keep cached rows and every
    /// presentation/input owner intact, including a pending double-click gesture.
    pub(crate) fn recheck_displayed(&mut self) {
        let paths: Vec<_> = self
            .demand
            .iter()
            .filter_map(|key| self.nodes.get(key).map(|node| node.path.clone()))
            .collect();
        for path in paths {
            self.request_recheck(&path);
        }
    }

    /// A hint after main's accepted physical-folder observation. Never creates a node,
    /// opens a folder, or touches any viewer-owned queue/watch/generation.
    pub(crate) fn observe_accepted_folder(&mut self, folder: &Path) {
        self.request_recheck(folder);
    }

    fn request_recheck(&mut self, path: &Path) {
        let key = key_for(path);
        let Some(node) = self.nodes.get_mut(&key) else {
            return;
        };
        match &mut node.scan {
            NodeScan::Running(_, follow_up) => *follow_up = FollowUp::Recheck,
            NodeScan::Queued => {}
            _ => {
                node.scan = if self.demand.contains(&key) {
                    NodeScan::Queued
                } else {
                    NodeScan::Unverified
                }
            }
        }
        self.trace("recheck", path);
    }

    fn trace(&self, action: &str, path: &Path) {
        if crate::perf::is_enabled() {
            crate::perf::event("folder_pane", action, Some(&path.to_string_lossy()), 0, &[]);
        }
    }

    fn reachable(&self, path: &Path) -> bool {
        let Some(root) = self.selected_drive() else {
            return false;
        };
        if !path_is_same_or_descendant(path, root) {
            return false;
        }
        for pair in ancestor_chain(root, path).windows(2) {
            let key = key_for(&pair[0]);
            if !self.is_expanded_key(&key) {
                return false;
            }
            if let Some(cache) = self.nodes.get(&key).and_then(|node| node.cache.as_ref()) {
                if !cache.child_keys.contains(&key_for(&pair[1])) {
                    return false;
                }
            } else if !self.auto_expanded.contains(&key_for(&pair[1]))
                && self
                    .active_path
                    .as_deref()
                    .is_none_or(|active| !crate::folder_tree::path_eq(active, &pair[1]))
            {
                return false;
            }
        }
        true
    }

    fn reconcile_demand(&mut self) {
        if self.participation == PaneParticipation::Suspended {
            return;
        }
        let mut paths: Vec<_> = self
            .visible_rows()
            .into_iter()
            .filter(|row| row.expanded || row.depth == 0)
            .map(|row| row.path)
            .collect();
        // Unknown ancestors can still prefetch concurrently, within the existing depth bound.
        paths.extend(
            self.auto_expanded
                .iter()
                .filter_map(|key| self.nodes.get(key))
                .filter(|node| {
                    self.is_expanded_key(&key_for(&node.path)) && self.reachable(&node.path)
                })
                .map(|node| node.path.clone()),
        );
        let next: HashSet<_> = paths.iter().map(|path| key_for(path)).collect();
        for key in self.demand.difference(&next) {
            if let Some(node) = self.nodes.get_mut(key) {
                node.scan = NodeScan::Unverified;
            }
        }
        for path in paths {
            let key = key_for(&path);
            self.ensure_node(path);
            let node = self.nodes.get_mut(&key).unwrap();
            if !self.demand.contains(&key)
                || matches!(node.scan, NodeScan::Unverified)
                || matches!(node.scan, NodeScan::Settled { options, .. } if options != self.listing_options)
            {
                node.scan = NodeScan::Queued;
            }
        }
        self.demand = next;
    }

    fn start_queued(&mut self) {
        const START_BUDGET: usize = 4;
        let mut keys: Vec<_> = self
            .demand
            .iter()
            .filter(|key| {
                self.nodes
                    .get(*key)
                    .is_some_and(|node| matches!(node.scan, NodeScan::Queued))
            })
            .cloned()
            .collect();
        keys.sort_by_key(|key| self.nodes[key].path.components().count());
        for key in keys.into_iter().take(START_BUDGET) {
            let path = self.nodes[&key].path.clone();
            let options = self.listing_options;
            let (tx, rx) = mpsc::channel();
            let cancel = Arc::new(AtomicBool::new(false));
            let cancel_w = cancel.clone();
            let scan_path = path.clone();
            #[cfg(test)]
            let backend = self.scan_backend.clone();
            let spawned = std::thread::Builder::new()
                .name("folder-pane-scan".into())
                .spawn(move || {
                    #[cfg(test)]
                    let result = if let Some(backend) = backend {
                        backend(&scan_path, options, &cancel_w)
                    } else {
                        scan_complete_listing(&scan_path, options, Some(&cancel_w))
                            .map_err(|err| err.to_string())
                    };
                    #[cfg(not(test))]
                    let result = scan_complete_listing(&scan_path, options, Some(&cancel_w))
                        .map_err(|err| err.to_string());
                    if !cancel_w.load(Ordering::Relaxed) {
                        let _ = tx.send(result);
                    }
                });
            self.nodes.get_mut(&key).unwrap().scan = match spawned {
                Ok(_) => NodeScan::Running(
                    FolderPaneScanPending {
                        cancel,
                        rx,
                        options,
                    },
                    FollowUp::None,
                ),
                Err(err) => {
                    crate::logger::log(format!(
                        "folder pane scan spawn failed: {}: {err}",
                        path.display()
                    ));
                    NodeScan::Settled {
                        options,
                        error: Some(format!("列挙スレッドを開始できません: {err}")),
                    }
                }
            };
            self.trace("enqueue", &path);
        }
    }

    pub(crate) fn poll_pending(&mut self) -> bool {
        const APPLY_BUDGET: usize = 4;
        let mut keys: Vec<_> = self
            .demand
            .iter()
            .filter(|key| {
                self.nodes
                    .get(*key)
                    .is_some_and(|node| matches!(node.scan, NodeScan::Running(..)))
            })
            .cloned()
            .collect();
        keys.sort_by_key(|key| self.nodes[key].path.components().count());
        let mut applied = 0;
        let mut changed = false;
        for key in keys {
            if applied == APPLY_BUDGET {
                break;
            }
            let Some(node) = self.nodes.get_mut(&key) else {
                continue;
            };
            let NodeScan::Running(pending, follow_up) =
                std::mem::replace(&mut node.scan, NodeScan::Unverified)
            else {
                continue;
            };
            let result = match pending.rx.try_recv() {
                Ok(result) => result,
                Err(mpsc::TryRecvError::Empty) => {
                    node.scan = NodeScan::Running(pending, follow_up);
                    continue;
                }
                Err(mpsc::TryRecvError::Disconnected) => Err("列挙に失敗しました".into()),
            };
            applied += 1;
            if matches!(follow_up, FollowUp::Recheck) {
                node.scan = NodeScan::Queued;
                rayon::spawn(move || drop(result));
                continue;
            }
            let path = node.path.clone();
            if pending.options != self.listing_options || !self.demand.contains(&key) {
                rayon::spawn(move || drop(result));
                continue;
            }
            let error = match result {
                Ok(cache) => {
                    debug_assert_eq!(cache.options, pending.options);
                    let previous = self.nodes.get_mut(&key).unwrap().cache.replace(cache);
                    rayon::spawn(move || drop(previous));
                    self.prune_missing_children(&path);
                    let children = self.nodes[&key].children().to_vec();
                    self.repair_cursor_after_successful_refresh(&path, &children);
                    None
                }
                Err(error) => {
                    crate::logger::log(format!(
                        "folder pane scan failed: {}: {error}",
                        path.display()
                    ));
                    Some(error)
                }
            };
            self.nodes.get_mut(&key).unwrap().scan = NodeScan::Settled {
                options: pending.options,
                error,
            };
            self.trace("settled", &path);
            self.reveal_active(false);
            self.reconcile_demand();
            changed = true;
        }
        self.start_queued();
        changed
    }

    fn prune_missing_children(&mut self, parent: &Path) {
        let membership = &self.nodes[&key_for(parent)]
            .cache
            .as_ref()
            .unwrap()
            .membership;
        let removed: Vec<_> = self
            .nodes
            .iter()
            .filter_map(|(key, node)| {
                let child = node.path.ancestors().find(|candidate| {
                    candidate
                        .parent()
                        .is_some_and(|p| crate::folder_tree::path_eq(p, parent))
                })?;
                (!membership.contains(&key_for(child))).then_some(key.clone())
            })
            .collect();
        let mut retired = Vec::new();
        for key in removed {
            if let Some(mut node) = self.nodes.remove(&key) {
                node.scan = NodeScan::Unverified; // cancel/drop the private receiver before retirement
                retired.push(node);
            }
            self.user_expanded.remove(&key);
            self.auto_expanded.remove(&key);
            self.user_collapsed.remove(&key);
            self.demand.remove(&key);
        }
        if !retired.is_empty() {
            rayon::spawn(move || drop(retired));
        }
    }

    pub(crate) fn visible_rows(&self) -> Vec<FolderPaneRow> {
        self.visible_rows_indexed().0
    }

    /// Reuse DFS's normalized-key deduplication table as this draw's row index.
    /// Keys move into the table once; focus/accessibility lookups need no second
    /// normalization of the logical rows and no cache across passes.
    pub(crate) fn visible_rows_indexed(&self) -> (Vec<FolderPaneRow>, HashMap<String, usize>) {
        let mut rows = Vec::new();
        let mut row_indices = HashMap::new();
        if let Some(root) = self.selected_drive() {
            let active = self.active_path.as_deref().map(key_for);
            let cursor = self.cursor_path().map(key_for);
            self.push_visible_rows(
                root,
                0,
                &mut rows,
                &mut row_indices,
                active.as_deref(),
                cursor.as_deref(),
            );
        }
        (rows, row_indices)
    }

    pub(crate) fn set_cursor(&mut self, path: PathBuf) {
        let drive = self
            .selected_drive()
            .filter(|root| path_is_same_or_descendant(&path, root))
            .map(Path::to_path_buf)
            .or_else(|| root_of_path(&path));
        self.clear_row_ids_on_drive_change(drive.as_deref());
        self.selection = FolderPaneSelection::Explicit(FolderPanePosition {
            drive,
            cursor: Some(path),
        });
        self.cursor_scroll = FolderPaneCursorScroll::Requested;
    }

    pub(crate) fn handle_tree_key(&mut self, key: FolderPaneTreeKey) -> Option<FolderPaneCommand> {
        match key {
            FolderPaneTreeKey::Up => self.move_cursor(-1),
            FolderPaneTreeKey::Down => self.move_cursor(1),
            FolderPaneTreeKey::Left => self.collapse_cursor(),
            FolderPaneTreeKey::Right => self.expand_cursor(),
            FolderPaneTreeKey::Enter => {
                return self
                    .cursor_path()
                    .map(Path::to_path_buf)
                    .map(FolderPaneCommand::Open);
            }
        }
        None
    }

    fn move_cursor(&mut self, delta: isize) {
        let rows = self.visible_rows();
        if rows.is_empty() {
            return;
        }
        let current = self
            .cursor_path()
            .and_then(|cursor| {
                rows.iter()
                    .position(|row| crate::folder_tree::path_eq(&row.path, cursor))
            })
            .or_else(|| {
                self.active_path.as_ref().and_then(|active| {
                    rows.iter()
                        .position(|row| crate::folder_tree::path_eq(&row.path, active))
                })
            })
            .unwrap_or(0);
        let next = if delta.is_negative() {
            current.saturating_sub(delta.unsigned_abs())
        } else {
            (current + delta as usize).min(rows.len() - 1)
        };
        self.set_cursor(rows[next].path.clone());
    }

    fn collapse_cursor(&mut self) {
        let Some(cursor) = self.cursor_path().map(Path::to_path_buf) else {
            return;
        };
        self.selection.make_explicit();
        let key = key_for(&cursor);
        if self.is_expanded_key(&key) {
            self.set_branch_expanded(&cursor, false);
            self.cursor_scroll = FolderPaneCursorScroll::Requested;
        } else if let Some(parent) = cursor
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            self.set_cursor(parent.to_path_buf());
        }
    }

    fn expand_cursor(&mut self) {
        let Some(cursor) = self.cursor_path().map(Path::to_path_buf) else {
            return;
        };
        self.selection.make_explicit();
        let key = key_for(&cursor);
        if !self.is_expanded_key(&key) {
            self.set_branch_expanded(&cursor, true);
            self.cursor_scroll = FolderPaneCursorScroll::Requested;
        } else if let Some(child) = self
            .nodes
            .get(&key)
            .and_then(|node| node.children().first())
            .cloned()
        {
            self.set_cursor(child);
        }
    }

    fn push_visible_rows(
        &self,
        path: &Path,
        depth: usize,
        rows: &mut Vec<FolderPaneRow>,
        row_indices: &mut HashMap<String, usize>,
        active: Option<&str>,
        cursor: Option<&str>,
    ) {
        if depth > 64 {
            return;
        }
        let key = key_for(path);
        let expanded = self.is_expanded_key(&key);
        let node = self.nodes.get(&key);
        let is_active = active == Some(key.as_str());
        let is_cursor = cursor == Some(key.as_str());
        match row_indices.entry(key) {
            std::collections::hash_map::Entry::Occupied(_) => return,
            std::collections::hash_map::Entry::Vacant(entry) => {
                entry.insert(rows.len());
            }
        }
        let loading =
            node.is_some_and(|node| matches!(node.scan, NodeScan::Queued | NodeScan::Running(..)));
        let error = node.and_then(|node| match &node.scan {
            NodeScan::Settled { error, .. } => error.clone(),
            _ => None,
        });
        // No branch is watched: even a confirmed expanded empty must remain collapsible.
        let has_children_or_unknown = true;
        rows.push(FolderPaneRow {
            path: path.to_path_buf(),
            depth,
            expanded,
            loading,
            has_children_or_unknown,
            is_active,
            is_cursor,
            error,
        });
        if expanded && let Some(node) = node {
            for child in node.children() {
                self.push_visible_rows(child, depth + 1, rows, row_indices, active, cursor);
            }
        }
    }

    fn ensure_node(&mut self, path: PathBuf) {
        self.nodes
            .entry(key_for(&path))
            .or_insert_with(|| FolderPaneNode::placeholder(path));
    }
    fn is_expanded_key(&self, key: &str) -> bool {
        self.user_expanded.contains(key)
            || self.auto_expanded.contains(key) && !self.user_collapsed.contains(key)
    }
    fn cancel_pending(&mut self) {
        for node in self.nodes.values_mut() {
            node.scan = NodeScan::Unverified;
        }
    }

    fn repair_cursor_after_successful_refresh(&mut self, parent: &Path, children: &[PathBuf]) {
        let Some(cursor) = self.cursor_path() else {
            return;
        };
        if crate::folder_tree::path_eq(cursor, parent)
            || !path_is_same_or_descendant(cursor, parent)
        {
            return;
        }
        if children
            .iter()
            .any(|child| path_is_same_or_descendant(cursor, child))
        {
            return;
        }
        let active_visible = self.active_path.as_ref().is_some_and(|active| {
            self.visible_rows()
                .iter()
                .any(|row| crate::folder_tree::path_eq(&row.path, active))
        });
        self.selection.position_mut().cursor = if active_visible {
            self.active_path.clone()
        } else {
            self.selection.position().drive.clone()
        };
        self.cursor_scroll = FolderPaneCursorScroll::Requested;
    }
}

fn path_is_same_or_descendant(path: &Path, ancestor: &Path) -> bool {
    path.ancestors()
        .any(|candidate| crate::folder_tree::path_eq(candidate, ancestor))
}

#[cfg(test)]
pub(crate) fn scan_real_subfolders(
    path: &Path,
    sort_order: FolderTreeSortOrder,
    show_hidden_files: bool,
    cancel: Option<&AtomicBool>,
) -> std::io::Result<Vec<PathBuf>> {
    scan_complete_listing(
        path,
        FolderPaneListingOptions {
            sort_order,
            show_hidden_files,
        },
        cancel,
    )
    .map(|cache| cache.children)
}

fn scan_complete_listing(
    path: &Path,
    options: FolderPaneListingOptions,
    cancel: Option<&AtomicBool>,
) -> std::io::Result<CachedListing> {
    let perf_start = crate::perf::is_enabled().then(std::time::Instant::now);
    let mut stats = FolderPaneScanStats::default();
    let result = scan_real_subfolders_inner(
        path,
        options.sort_order,
        options.show_hidden_files,
        cancel,
        &mut stats,
    );
    if let Some(start) = perf_start {
        emit_folder_pane_scan_perf(path, options.sort_order, start, &stats, &result);
    }
    result
}

#[derive(Default)]
struct FolderPaneScanStats {
    entries_seen: usize,
    dirs_returned: usize,
    entry_errors: usize,
    file_type_errors: usize,
    mtime_errors: usize,
    canceled: bool,
}

fn scan_real_subfolders_inner(
    path: &Path,
    sort_order: FolderTreeSortOrder,
    show_hidden_files: bool,
    cancel: Option<&AtomicBool>,
    stats: &mut FolderPaneScanStats,
) -> std::io::Result<CachedListing> {
    let mut membership = HashSet::new();
    let mut dirs: Vec<(PathBuf, i64)> = Vec::new();
    let entries = std::fs::read_dir(path)?;
    let use_mtime = sort_order.uses_mtime();
    for entry in entries {
        if cancel.is_some_and(|c| c.load(Ordering::Relaxed)) {
            stats.canceled = true;
            return Err(std::io::Error::from(std::io::ErrorKind::Interrupted));
        }
        stats.entries_seen += 1;
        let entry = match entry {
            Ok(entry) => entry,
            Err(err) => {
                stats.entry_errors += 1;
                return Err(err);
            }
        };
        if crate::fs_entry::is_internal_app_entry_name(&entry.file_name()) {
            continue;
        }
        let file_type = match entry.file_type() {
            Ok(file_type) => file_type,
            Err(err) => {
                stats.file_type_errors += 1;
                return Err(err);
            }
        };
        if !crate::fs_entry::try_classify_dir_entry(&entry, &file_type)?.is_directory() {
            continue;
        }
        membership.insert(key_for(&entry.path()));
        if crate::fs_entry::should_hide_fs_entry(&entry, show_hidden_files) {
            continue;
        }
        let mtime = if use_mtime {
            match entry.metadata().and_then(|m| m.modified()) {
                Ok(t) => match t.duration_since(std::time::UNIX_EPOCH) {
                    Ok(d) => d.as_secs() as i64,
                    Err(_) => {
                        stats.mtime_errors += 1;
                        0
                    }
                },
                Err(_) => {
                    stats.mtime_errors += 1;
                    0
                }
            }
        } else {
            0
        };
        stats.dirs_returned += 1;
        dirs.push((entry.path(), mtime));
    }
    let mut keyed_dirs: Vec<_> = dirs
        .into_iter()
        .map(|(path, mtime)| {
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            let key = sort_order.name_key(name);
            (path, mtime, key)
        })
        .collect();
    keyed_dirs
        .sort_by(|(_, a_mt, ak), (_, b_mt, bk)| sort_order.compare_name_keys(ak, *a_mt, bk, *b_mt));
    if cancel.is_some_and(|c| c.load(Ordering::Relaxed)) {
        return Err(std::io::Error::from(std::io::ErrorKind::Interrupted));
    }
    Ok(CachedListing::new(
        keyed_dirs.into_iter().map(|(path, _, _)| path).collect(),
        membership,
        FolderPaneListingOptions {
            sort_order,
            show_hidden_files,
        },
    ))
}

fn emit_folder_pane_scan_perf(
    path: &Path,
    sort_order: FolderTreeSortOrder,
    start: std::time::Instant,
    stats: &FolderPaneScanStats,
    result: &std::io::Result<CachedListing>,
) {
    let ms = start.elapsed().as_secs_f64() * 1000.0;
    let use_mtime = sort_order.uses_mtime();
    let mut fields = vec![
        ("ms", serde_json::Value::from(ms)),
        ("entries_seen", serde_json::Value::from(stats.entries_seen)),
        (
            "dirs_returned",
            serde_json::Value::from(stats.dirs_returned),
        ),
        ("entry_errors", serde_json::Value::from(stats.entry_errors)),
        (
            "file_type_errors",
            serde_json::Value::from(stats.file_type_errors),
        ),
        ("mtime_errors", serde_json::Value::from(stats.mtime_errors)),
        ("canceled", serde_json::Value::from(stats.canceled)),
        ("use_mtime", serde_json::Value::from(use_mtime)),
        ("sort", serde_json::Value::from(format!("{sort_order:?}"))),
        ("ok", serde_json::Value::from(result.is_ok())),
    ];
    if let Err(err) = result {
        fields.push((
            "error_kind",
            serde_json::Value::from(format!("{:?}", err.kind())),
        ));
    }
    let key = path.to_string_lossy();
    crate::perf::event(
        "folder_pane",
        "scan_subfolders",
        Some(key.as_ref()),
        0,
        &fields,
    );
}

pub(crate) fn active_filesystem_folder(path: &Path) -> Option<PathBuf> {
    if path.as_os_str().is_empty() {
        return None;
    }
    // 字句判定のみ (filesystem syscall なし)。`sync_to_active` はペイン表示中 **毎フレーム**
    // これを呼ぶので、ここで `Path::is_dir()` / `is_file()` を叩くと切断ネットワークドライブ等で
    // 毎フレーム GetFileAttributes が SMB タイムアウトし UI が固まる
    // (docs/ui-responsiveness.md §4 の禁止事項)。
    //
    // 入力は `App::effective_folder()`、すなわち実ディレクトリ (current_folder) か
    // ZIP/PDF/変換アーカイブを仮想フォルダ source として開いたファイルパスのいずれか。
    // 後者は拡張子で判定して親ディレクトリを返し、それ以外はディレクトリとして扱う。
    if crate::folder_tree::is_open_as_container(path)
        || crate::folder_tree::is_convertible_archive_path(path)
    {
        return path.parent().map(Path::to_path_buf);
    }
    Some(path.to_path_buf())
}

pub(crate) fn folder_label(path: &Path) -> String {
    if is_root_like(path) {
        return path.display().to_string();
    }
    path.file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .map(str::to_owned)
        .unwrap_or_else(|| path.display().to_string())
}

pub(crate) fn drive_label(path: &Path) -> String {
    let display = path.display().to_string();
    if display.len() >= 2 && display.as_bytes().get(1) == Some(&b':') {
        display[..2].to_string()
    } else {
        display
    }
}

fn key_for(path: &Path) -> String {
    crate::path_key::normalize_keep_drive(path)
}

fn is_root_like(path: &Path) -> bool {
    path.parent().is_none()
        || path
            .parent()
            .is_some_and(|parent| parent.as_os_str().is_empty())
}

fn root_of_path(path: &Path) -> Option<PathBuf> {
    let raw = path.to_string_lossy();
    let bytes = raw.as_bytes();
    if bytes.len() >= 2 && bytes[1] == b':' && bytes[0].is_ascii_alphabetic() {
        let letter = (bytes[0] as char).to_ascii_uppercase();
        return Some(PathBuf::from(format!("{letter}:\\")));
    }
    if raw.starts_with('/') {
        return Some(PathBuf::from("/"));
    }
    path.ancestors().last().map(Path::to_path_buf)
}

fn ancestor_chain(root: &Path, path: &Path) -> Vec<PathBuf> {
    let mut chain = Vec::new();
    let mut current = Some(path);
    while let Some(cur) = current {
        chain.push(cur.to_path_buf());
        if crate::folder_tree::path_eq(cur, root) {
            break;
        }
        current = cur.parent();
    }
    chain.reverse();
    if chain
        .first()
        .is_none_or(|first| !crate::folder_tree::path_eq(first, root))
    {
        chain.insert(0, root.to_path_buf());
    }
    chain
}

#[cfg(test)]
#[path = "folder_pane/refresh_tests.rs"]
mod refresh_tests;

#[cfg(test)]
impl FolderPaneState {
    /// Keep host drive contents/permissions out of App tests, while scanning the
    /// disposable subtree with the production filesystem scanner and worker.
    pub(crate) fn test_scan_subtree(&mut self, root: PathBuf) {
        self.scan_backend = Some(Arc::new(move |path, options, cancel| {
            if path != root && path_is_same_or_descendant(&root, path) {
                let child = root
                    .ancestors()
                    .find(|ancestor| ancestor.parent() == Some(path))
                    .unwrap();
                let children = vec![child.to_path_buf()];
                let membership = children.iter().map(|child| key_for(child)).collect();
                Ok(CachedListing::new(children, membership, options))
            } else if path_is_same_or_descendant(path, &root) {
                scan_complete_listing(path, options, Some(cancel)).map_err(|err| err.to_string())
            } else {
                Ok(CachedListing::new(vec![], HashSet::new(), options))
            }
        }));
    }

    pub(crate) fn test_hold_next_scan(&mut self, held: PathBuf) -> mpsc::Sender<()> {
        let backend = self.scan_backend.take().expect("test subtree backend");
        let (tx, rx) = mpsc::channel();
        let gate = std::sync::Mutex::new(Some(rx));
        self.scan_backend = Some(Arc::new(move |path, options, cancel| {
            if crate::folder_tree::path_eq(path, &held)
                && let Some(rx) = gate.lock().unwrap().take()
            {
                // Test worker only. Dropping the sender also releases the worker.
                let _ = rx.recv();
            }
            backend(path, options, cancel)
        }));
        tx
    }

    pub(crate) fn test_tree(root: &Path, children: Vec<PathBuf>) -> Self {
        let mut state = Self::default();
        state.selection = FolderPaneSelection::Explicit(FolderPanePosition {
            drive: Some(root.to_path_buf()),
            cursor: None,
        });
        state.has_focus = true;
        state.last_drive_refresh = Some(std::time::Instant::now());
        state.resume_display();
        state.test_seed_listing(root, children, true);
        state
    }

    pub(crate) fn test_set_drive_choices(&mut self, drives: Vec<PathBuf>) {
        self.drives = drives;
    }

    pub(crate) fn test_set_default_drive(&mut self, drive: Option<PathBuf>) {
        self.selection = FolderPaneSelection::FollowActive(FolderPanePosition {
            drive,
            cursor: None,
        });
        self.last_drive_refresh = Some(std::time::Instant::now());
    }

    pub(crate) fn test_seed_listing(
        &mut self,
        path: &Path,
        children: Vec<PathBuf>,
        expanded: bool,
    ) {
        let key = key_for(path);
        let membership = children.iter().map(|child| key_for(child)).collect();
        let cache = CachedListing::new(children, membership, self.listing_options);
        self.nodes.insert(
            key.clone(),
            FolderPaneNode {
                path: path.to_path_buf(),
                cache: Some(cache),
                scan: NodeScan::Settled {
                    options: self.listing_options,
                    error: None,
                },
            },
        );
        if expanded {
            self.user_expanded.insert(key.clone());
        }
        if expanded
            || self
                .selected_drive()
                .is_some_and(|root| crate::folder_tree::path_eq(root, path))
        {
            self.demand.insert(key);
        }
    }

    pub(crate) fn test_children(&self, path: &Path) -> Vec<PathBuf> {
        self.nodes
            .get(&key_for(path))
            .map_or(Vec::new(), |node| node.children().to_vec())
    }

    pub(crate) fn test_unverified(&self, path: &Path) -> bool {
        self.nodes
            .get(&key_for(path))
            .is_some_and(|node| !matches!(node.scan, NodeScan::Settled { .. }))
    }

    pub(crate) fn test_settle(&mut self) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        while self.has_pending() {
            self.poll_pending();
            assert!(
                std::time::Instant::now() < deadline,
                "pane scans did not settle"
            );
            std::thread::yield_now();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn label_press(state: &mut FolderPaneState, path: &str, x: f32, time: f64) {
        pointer_press(
            state,
            FolderPanePointerHit::Label(p(path)),
            x,
            time,
            FolderPanePointerSource::Mouse,
            true,
            true,
            false,
        );
    }

    fn pointer_press(
        state: &mut FolderPaneState,
        hit: FolderPanePointerHit,
        x: f32,
        time: f64,
        source: FolderPanePointerSource,
        start_allowed: bool,
        continue_allowed: bool,
        arrow_rect: bool,
    ) -> bool {
        state.process_label_input(
            FolderPaneLabelInput::Press {
                pos: egui::pos2(x, 0.0),
                hit,
                start_allowed,
                continue_allowed,
                arrow_rect,
                source,
            },
            time,
            6.0,
            0.3,
        )
    }

    fn label_release(state: &mut FolderPaneState, path: &str, time: f64, clicked: bool) {
        pointer_release(
            state,
            FolderPanePointerHit::Label(p(path)),
            time,
            FolderPanePointerSource::Mouse,
            true,
            clicked,
        );
    }

    fn pointer_release(
        state: &mut FolderPaneState,
        hit: FolderPanePointerHit,
        time: f64,
        source: FolderPanePointerSource,
        start_allowed: bool,
        clicked: bool,
    ) -> bool {
        state.process_label_input(
            FolderPaneLabelInput::Release {
                hit,
                start_allowed,
                clicked,
                source,
            },
            time,
            6.0,
            0.3,
        )
    }

    #[test]
    fn label_click_first_release_uses_egui_adjudication_and_normalized_path() {
        for clicked in [false, true] {
            let mut state = FolderPaneState::default();
            label_press(&mut state, "C:/tree/branch", 0.0, 1.0);
            assert!(state.label_input.results.is_empty());
            assert!(!state.label_gesture_requires_tree_focus());
            label_release(&mut state, "c:/TREE/branch", 1.01, clicked);
            assert_eq!(state.label_input.results.len(), usize::from(clicked));
            assert_eq!(state.label_gesture_requires_tree_focus(), clicked);
        }
    }

    #[test]
    fn label_gesture_diagnostic_reports_double_click_delay_rejection() {
        for time in [1.0, 1.32] {
            let mut state = FolderPaneState::default();
            label_press(&mut state, "C:/a", 0.0, 1.0);
            label_release(&mut state, "C:/a", 1.01, true);
            label_press(&mut state, "C:/a", 0.0, time);
            assert_eq!(
                state.last_gesture_not_continued,
                Some(FolderPaneGestureNotContinued::ReleaseTimeOutsideDoubleClickDelay)
            );
            assert!(matches!(
                state.label_gesture,
                FolderPaneLabelGesture::FirstPress { .. }
            ));
            assert_eq!(
                state.label_input.results,
                vec![FolderPaneLabelResult::Single(p("C:/a"))]
            );
        }
    }

    #[test]
    fn label_gesture_diagnostic_reports_continuation_gate_rejection() {
        let mut state = FolderPaneState::default();
        label_press(&mut state, "C:/a", 0.0, 1.0);
        label_release(&mut state, "C:/a", 1.01, true);
        pointer_press(
            &mut state,
            FolderPanePointerHit::Label(p("C:/a")),
            0.0,
            1.1,
            FolderPanePointerSource::Mouse,
            true,
            false,
            false,
        );
        assert_eq!(
            state.last_gesture_not_continued,
            Some(FolderPaneGestureNotContinued::ContinueNotAllowed)
        );
        assert!(matches!(
            state.label_gesture,
            FolderPaneLabelGesture::FirstPress { .. }
        ));
        assert_eq!(
            state.label_input.results,
            vec![FolderPaneLabelResult::Single(p("C:/a"))]
        );
    }

    #[test]
    fn label_gesture_diagnostic_distinguishes_source_distance_and_arrow() {
        use FolderPaneGestureNotContinued::*;
        for (source, hit, x, arrow_rect, expected) in [
            (
                FolderPanePointerSource::Touch,
                FolderPanePointerHit::None,
                0.0,
                false,
                InputSourceNotMouse,
            ),
            (
                FolderPanePointerSource::Mouse,
                FolderPanePointerHit::None,
                7.0,
                false,
                DistanceExceeded,
            ),
            (
                FolderPanePointerSource::Mouse,
                FolderPanePointerHit::Arrow(p("C:/a")),
                0.0,
                false,
                ArrowWidgetHit,
            ),
            (
                FolderPanePointerSource::Mouse,
                FolderPanePointerHit::None,
                0.0,
                true,
                ArrowRectHit,
            ),
        ] {
            let mut state = FolderPaneState::default();
            label_press(&mut state, "C:/a", 0.0, 1.0);
            label_release(&mut state, "C:/a", 1.01, true);
            pointer_press(&mut state, hit, x, 1.1, source, true, true, arrow_rect);
            assert_eq!(state.last_gesture_not_continued, Some(expected));
            assert_eq!(
                state.label_input.results,
                vec![FolderPaneLabelResult::Single(p("C:/a"))]
            );
        }
    }

    #[test]
    fn label_gesture_diagnostic_focus_grid_resets_only_an_active_gesture() {
        let mut state = FolderPaneState::default();
        state.set_focus_grid();
        assert_eq!(state.last_gesture_reset, None);
        label_press(&mut state, "C:/a", 0.0, 1.0);
        label_release(&mut state, "C:/a", 1.01, true);
        state.set_focus_grid();
        assert_eq!(
            state.last_gesture_reset,
            Some(FolderPaneGestureReset::FocusGrid)
        );
        assert!(matches!(state.label_gesture, FolderPaneLabelGesture::Idle));
        // Clearing Idle must not publish another reset reason.
        state.clear_label_click(FolderPaneGestureReset::TreeKeyPressed);
        assert_eq!(
            state.last_gesture_reset,
            Some(FolderPaneGestureReset::FocusGrid)
        );
    }

    #[test]
    fn label_gesture_diagnostic_invalidation_retains_the_callers_reason() {
        let mut state = FolderPaneState::default();
        label_press(&mut state, "C:/a", 0.0, 1.0);
        state.invalidate_label_display(FolderPaneGestureReset::DisplayNotFresh);
        assert_eq!(
            state.last_gesture_reset,
            Some(FolderPaneGestureReset::DisplayNotFresh)
        );
        assert!(matches!(state.label_gesture, FolderPaneLabelGesture::Idle));
    }

    #[test]
    fn label_gesture_diagnostic_successful_continuation_has_no_rejection() {
        let mut state = FolderPaneState::default();
        label_press(&mut state, "C:/a", 0.0, 1.0);
        label_release(&mut state, "C:/a", 1.01, true);
        label_press(&mut state, "C:/a", 0.0, 1.1);
        assert_eq!(state.last_gesture_not_continued, None);
        assert_eq!(state.last_gesture_reset, None);
        assert!(matches!(
            state.label_gesture,
            FolderPaneLabelGesture::PressAccepted(_)
        ));
        assert_eq!(state.label_input.results.len(), 2);
    }

    #[test]
    fn label_click_second_press_owns_first_row_and_never_reissues_navigation() {
        let mut state = FolderPaneState::default();
        label_press(&mut state, "C:/a", 0.0, 1.0);
        label_release(&mut state, "C:/a", 1.01, true);
        label_press(&mut state, "C:/other", 4.0, 1.1);
        assert_eq!(
            state.label_input.results,
            vec![
                FolderPaneLabelResult::Single(p("C:/a")),
                FolderPaneLabelResult::Toggle {
                    path: p("C:/a"),
                    expanded: true
                },
            ]
        );
        // Even a dragged or long-held accepted continuation never becomes a single.
        label_release(&mut state, "C:/other", 9.0, false);
        for time in [9.2, 9.4, 9.6, 9.8] {
            label_press(&mut state, "C:/other", 4.0, time);
            label_release(&mut state, "C:/other", time + 0.01, true);
        }
        assert_eq!(state.label_input.results.len(), 2);
    }

    #[test]
    fn label_click_timeout_or_distance_starts_the_next_single() {
        for (x, time) in [(6.01, 1.1), (0.0, 1.311)] {
            let mut state = FolderPaneState::default();
            label_press(&mut state, "C:/a", 0.0, 1.0);
            label_release(&mut state, "C:/a", 1.01, true);
            label_press(&mut state, "C:/b", x, time);
            label_release(&mut state, "C:/b", time + 0.01, true);
            assert_eq!(
                state.label_input.results,
                vec![
                    FolderPaneLabelResult::Single(p("C:/a")),
                    FolderPaneLabelResult::Single(p("C:/b")),
                ]
            );
        }
    }

    #[test]
    fn label_click_arrow_and_cancel_break_only_the_gesture_not_prior_results() {
        for arrow in [false, true] {
            let mut state = FolderPaneState::default();
            label_press(&mut state, "C:/a", 0.0, 1.0);
            label_release(&mut state, "C:/a", 1.01, true);
            if arrow {
                pointer_press(
                    &mut state,
                    FolderPanePointerHit::Arrow(p("C:/a")),
                    0.0,
                    1.02,
                    FolderPanePointerSource::Mouse,
                    true,
                    true,
                    true,
                );
            } else {
                state.process_label_input(FolderPaneLabelInput::Cancel, 1.02, 6.0, 0.3);
            }
            label_press(&mut state, "C:/b", 0.0, 1.03);
            label_release(&mut state, "C:/b", 1.04, true);
            assert_eq!(
                state.label_input.results,
                vec![
                    FolderPaneLabelResult::Single(p("C:/a")),
                    FolderPaneLabelResult::Single(p("C:/b")),
                ]
            );
        }
    }

    #[test]
    fn label_click_release_rechecks_admission_target_and_source() {
        for (path, allowed, source) in [
            ("C:/a", false, FolderPanePointerSource::Mouse),
            ("C:/b", true, FolderPanePointerSource::Mouse),
            ("C:/a", true, FolderPanePointerSource::Touch),
        ] {
            let mut state = FolderPaneState::default();
            label_press(&mut state, "C:/a", 0.0, 1.0);
            pointer_release(
                &mut state,
                FolderPanePointerHit::Label(p(path)),
                1.01,
                source,
                allowed,
                true,
            );
            assert!(state.label_input.results.is_empty());
            assert!(matches!(state.label_gesture, FolderPaneLabelGesture::Idle));
        }
    }

    #[test]
    fn label_click_first_display_owns_target_without_mutating_pane_state() {
        for first in [false, true] {
            for current in [false, true] {
                let root = p("C:/");
                let a = p("C:/a");
                let mut state = FolderPaneState::test_tree(&root, vec![a.clone()]);
                if first {
                    state.user_expanded.insert(key_for(&a));
                }
                label_press(&mut state, "C:/a", 0.0, 1.0);
                label_release(&mut state, "C:/a", 1.01, true);
                if current {
                    state.user_expanded.insert(key_for(&a));
                } else {
                    state.user_expanded.remove(&key_for(&a));
                }
                let cursor = state.cursor_path().map(Path::to_path_buf);
                let focus = state.has_focus;
                let scroll = state.cursor_scroll;
                let demand = state.demand.clone();
                let nodes = state.nodes.len();
                label_press(&mut state, "C:/other", 4.0, 1.1);
                assert_eq!(
                    state.label_input.results.last(),
                    Some(&FolderPaneLabelResult::Toggle {
                        path: a.clone(),
                        expanded: !first,
                    })
                );
                assert_eq!(state.is_expanded_key(&key_for(&a)), current);
                assert_eq!(state.cursor_path(), cursor.as_deref());
                assert_eq!(state.has_focus, focus);
                assert_eq!(state.cursor_scroll, scroll);
                assert_eq!(state.demand, demand);
                assert_eq!(state.nodes.len(), nodes);
            }
        }
    }

    #[test]
    fn label_click_results_belong_only_to_the_input_pass_and_are_taken_once() {
        let mut state = FolderPaneState::default();
        assert!(state.begin_label_input(1));
        label_press(&mut state, "C:/a", 0.0, 1.0);
        label_release(&mut state, "C:/a", 1.01, true);
        assert!(!state.begin_label_input(1));
        assert!(state.take_label_results(0).is_empty());
        assert_eq!(state.take_label_results(1).len(), 1);
        assert!(state.take_label_results(1).is_empty());
        label_press(&mut state, "C:/b", 20.0, 1.1);
        label_release(&mut state, "C:/b", 1.11, true);
        assert!(state.begin_label_input(2));
        assert!(state.take_label_results(2).is_empty());
        label_press(&mut state, "C:/c", 40.0, 1.2);
        label_release(&mut state, "C:/c", 1.21, true);
        state.finish_label_input();
        assert!(state.take_label_results(2).is_empty());
    }

    #[test]
    fn label_click_arrow_results_preserve_edge_order_and_do_not_mutate_expansion() {
        let mut state = FolderPaneState::default();
        pointer_press(
            &mut state,
            FolderPanePointerHit::Arrow(p("C:/a")),
            0.0,
            1.0,
            FolderPanePointerSource::Mouse,
            true,
            true,
            true,
        );
        pointer_release(
            &mut state,
            FolderPanePointerHit::Arrow(p("c:/A")),
            1.01,
            FolderPanePointerSource::Mouse,
            true,
            true,
        );
        label_press(&mut state, "C:/a", 4.0, 1.02);
        label_release(&mut state, "C:/a", 1.03, true);
        label_press(&mut state, "C:/a", 4.0, 1.04);
        assert_eq!(
            state.label_input.results,
            vec![
                FolderPaneLabelResult::Arrow(p("C:/a")),
                FolderPaneLabelResult::Single(p("C:/a")),
                FolderPaneLabelResult::Toggle {
                    path: p("C:/a"),
                    expanded: true
                },
            ]
        );
        assert!(!state.is_expanded_key(&key_for(Path::new("C:/a"))));
    }

    #[test]
    fn label_click_arrow_release_requires_matching_accepted_click() {
        for (hit, source, allowed, clicked) in [
            (
                FolderPanePointerHit::Label(p("C:/a")),
                FolderPanePointerSource::Mouse,
                true,
                true,
            ),
            (
                FolderPanePointerHit::Arrow(p("C:/b")),
                FolderPanePointerSource::Mouse,
                true,
                true,
            ),
            (
                FolderPanePointerHit::Arrow(p("C:/a")),
                FolderPanePointerSource::Touch,
                true,
                true,
            ),
            (
                FolderPanePointerHit::Arrow(p("C:/a")),
                FolderPanePointerSource::Mouse,
                false,
                true,
            ),
            (
                FolderPanePointerHit::Arrow(p("C:/a")),
                FolderPanePointerSource::Mouse,
                true,
                false,
            ),
        ] {
            let mut state = FolderPaneState::default();
            pointer_press(
                &mut state,
                FolderPanePointerHit::Arrow(p("C:/a")),
                0.0,
                1.0,
                FolderPanePointerSource::Mouse,
                true,
                true,
                true,
            );
            pointer_release(&mut state, hit, 1.01, source, allowed, clicked);
            assert!(state.label_input.results.is_empty());
            assert!(matches!(state.label_gesture, FolderPaneLabelGesture::Idle));
        }
    }

    #[test]
    fn label_click_touch_taps_never_start_or_continue_a_double_gesture() {
        for arrow in [false, true] {
            let mut state = FolderPaneState::default();
            label_press(&mut state, "C:/a", 0.0, 1.0);
            label_release(&mut state, "C:/a", 1.01, true);
            for time in [1.1, 1.2] {
                let hit = if arrow {
                    FolderPanePointerHit::Arrow(p("C:/a"))
                } else {
                    FolderPanePointerHit::Label(p("C:/a"))
                };
                pointer_press(
                    &mut state,
                    hit.clone(),
                    0.0,
                    time,
                    FolderPanePointerSource::Touch,
                    true,
                    true,
                    arrow,
                );
                pointer_release(
                    &mut state,
                    hit,
                    time + 0.01,
                    FolderPanePointerSource::Touch,
                    true,
                    true,
                );
                assert!(matches!(state.label_gesture, FolderPaneLabelGesture::Idle));
            }
            assert_eq!(state.label_input.results.len(), 3);
            assert!(
                !state
                    .label_input
                    .results
                    .iter()
                    .any(|result| matches!(result, FolderPaneLabelResult::Toggle { .. }))
            );
        }
    }

    #[test]
    fn label_click_disabled_arrow_geometry_and_modal_admission_cancel_continuations() {
        for (hit, arrow_rect) in [
            (FolderPanePointerHit::None, true),
            (FolderPanePointerHit::Label(p("C:/a")), false),
        ] {
            let mut state = FolderPaneState::default();
            label_press(&mut state, "C:/a", 0.0, 1.0);
            label_release(&mut state, "C:/a", 1.01, true);
            pointer_press(
                &mut state,
                hit,
                0.0,
                1.1,
                FolderPanePointerSource::Mouse,
                false,
                arrow_rect,
                arrow_rect,
            );
            assert!(matches!(state.label_gesture, FolderPaneLabelGesture::Idle));
            assert_eq!(state.label_input.results.len(), 1);
        }
    }

    #[test]
    fn label_click_sidecar_continuation_accepts_without_new_click_admission() {
        let mut state = FolderPaneState::default();
        label_press(&mut state, "C:/a", 0.0, 1.0);
        label_release(&mut state, "C:/a", 1.01, true);
        pointer_press(
            &mut state,
            FolderPanePointerHit::None,
            0.0,
            1.1,
            FolderPanePointerSource::Mouse,
            false,
            true,
            false,
        );
        pointer_release(
            &mut state,
            FolderPanePointerHit::None,
            1.11,
            FolderPanePointerSource::Mouse,
            false,
            false,
        );
        assert_eq!(
            state.label_input.results.last(),
            Some(&FolderPaneLabelResult::Toggle {
                path: p("C:/a"),
                expanded: true,
            })
        );
        assert!(state.label_gesture_requires_tree_focus());
    }

    #[test]
    fn label_click_display_invalidation_discards_unapplied_results_and_gesture() {
        let mut state = FolderPaneState::default();
        label_press(&mut state, "C:/a", 0.0, 1.0);
        label_release(&mut state, "C:/a", 1.01, true);
        state.label_display = Some(FolderPaneLabelDisplay {
            pass: 1,
            layer: egui::LayerId::background(),
            pane_rect: egui::Rect::NOTHING,
            rect: egui::Rect::NOTHING,
            rows: vec![],
        });
        state.invalidate_label_display(FolderPaneGestureReset::DisplayNotFresh);
        assert!(state.label_display.is_none());
        assert!(state.label_input.results.is_empty());
        assert!(matches!(state.label_gesture, FolderPaneLabelGesture::Idle));
    }

    #[test]
    fn label_click_key_application_preserves_a_new_gesture_recognized_after_key_cancel() {
        let mut state = FolderPaneState::test_tree(Path::new("C:/"), vec![p("C:/a")]);
        label_press(&mut state, "C:/a", 0.0, 1.0);
        label_release(&mut state, "C:/a", 1.01, true);
        // The key cancels before the new pointer sequence, not during key application.
        state.clear_label_click(FolderPaneGestureReset::TreeKeyPressed);
        assert_eq!(
            state.label_input.results,
            vec![FolderPaneLabelResult::Single(p("C:/a"))]
        );
        state.label_input.results.clear();
        label_press(&mut state, "C:/a", 0.0, 1.1);
        label_release(&mut state, "C:/a", 1.11, true);
        state.handle_tree_key(FolderPaneTreeKey::Down);
        state.set_focus_tree();
        state.set_cursor(p("C:/a"));
        assert_eq!(
            state.label_input.results,
            vec![FolderPaneLabelResult::Single(p("C:/a"))]
        );
        assert!(state.label_gesture_requires_tree_focus());
    }

    #[test]
    fn label_click_shared_expansion_writer_preserves_selection_and_reveal() {
        let root = Path::new("C:/");
        let a = p("C:/a");
        let mut state = FolderPaneState::test_tree(root, vec![a.clone()]);
        state.auto_expanded.insert(key_for(&a));
        let cursor = state.cursor_path().map(Path::to_path_buf);
        let focus = state.has_focus;
        let scroll = state.cursor_scroll;
        for expanded in [false, true] {
            state.set_branch_expanded(&a, expanded);
            assert_eq!(state.is_expanded_key(&key_for(&a)), expanded);
            assert_eq!(state.cursor_path(), cursor.as_deref());
            assert_eq!(state.has_focus, focus);
            assert_eq!(state.cursor_scroll, scroll);
            let demand = state.demand.clone();
            state.set_branch_expanded(&a, expanded);
            assert_eq!(state.demand, demand);
        }
    }

    #[test]
    fn label_click_explicit_focus_loss_cancels_press_but_key_application_preserves_it() {
        for explicit_loss in [false, true] {
            let mut state = FolderPaneState::default();
            state.set_focus_tree();
            label_press(&mut state, "C:/a", 0.0, 1.0);
            if explicit_loss {
                state.set_focus_grid();
            } else {
                state.release_tree_keyboard_focus();
            }
            label_release(&mut state, "C:/a", 1.01, true);
            assert_eq!(state.label_input.results.len(), usize::from(!explicit_loss));
        }
    }

    #[test]
    fn label_click_hide_drive_switch_and_reload_retire_display_input() {
        for lifecycle in 0..3 {
            let mut state = FolderPaneState::default();
            state.last_drive_refresh = Some(std::time::Instant::now());
            label_press(&mut state, "C:/a", 0.0, 1.0);
            label_release(&mut state, "C:/a", 1.01, true);
            state.label_display = Some(FolderPaneLabelDisplay {
                pass: 1,
                layer: egui::LayerId::background(),
                pane_rect: egui::Rect::NOTHING,
                rect: egui::Rect::NOTHING,
                rows: vec![],
            });
            match lifecycle {
                0 => state.suspend_display(),
                1 => state.select_drive(p("C:/")),
                _ => state.reload_for_active(None, FolderPaneListingOptions::default()),
            }
            assert!(state.label_display.is_none());
            assert!(state.label_input.results.is_empty());
            assert!(matches!(state.label_gesture, FolderPaneLabelGesture::Idle));
        }
    }

    #[test]
    fn label_click_pointer_ownership_requires_commit_or_accepted_continuation() {
        let mut state = FolderPaneState::default();
        let hit = FolderPanePointerHit::Label(p("C:/a"));
        assert!(!pointer_press(
            &mut state,
            hit.clone(),
            0.0,
            1.0,
            FolderPanePointerSource::Mouse,
            true,
            true,
            false
        ));
        assert!(pointer_release(
            &mut state,
            hit.clone(),
            1.01,
            FolderPanePointerSource::Mouse,
            true,
            true
        ));
        for time in [1.1, 1.2] {
            assert!(pointer_press(
                &mut state,
                hit.clone(),
                0.0,
                time,
                FolderPanePointerSource::Mouse,
                true,
                true,
                false
            ));
            assert!(pointer_release(
                &mut state,
                hit.clone(),
                time + 0.01,
                FolderPanePointerSource::Mouse,
                true,
                false
            ));
        }
        assert_eq!(state.label_input.results.len(), 2);
        state.clear_label_click(FolderPaneGestureReset::TreeKeyPressed);
        assert!(!pointer_press(
            &mut state,
            hit.clone(),
            0.0,
            1.3,
            FolderPanePointerSource::Mouse,
            true,
            true,
            false
        ));
        assert!(!pointer_release(
            &mut state,
            hit,
            1.31,
            FolderPanePointerSource::Mouse,
            true,
            false
        ));
    }

    #[test]
    fn label_click_widget_activation_ownership_is_local_and_pass_scoped() {
        let mut state = FolderPaneState::default();
        let arrow = egui::Id::new("arrow");
        let label = egui::Id::new("label");
        state.begin_label_input(1);
        state.record_label_pointer_activation(arrow);
        assert!(state.label_pointer_activation_owned(arrow));
        assert!(!state.label_pointer_activation_owned(label));
        state.take_label_results(1);
        assert!(state.label_pointer_activation_owned(arrow));
        state.begin_label_input(2);
        assert!(!state.label_pointer_activation_owned(arrow));
        state.record_label_pointer_activation(label);
        state.finish_label_input();
        assert!(!state.label_pointer_activation_owned(label));
        state.record_label_pointer_activation(arrow);
        state.invalidate_label_display(FolderPaneGestureReset::DisplayNotFresh);
        assert!(!state.label_pointer_activation_owned(arrow));
    }

    #[test]
    fn label_click_unreleased_mixed_source_presses_cancel_across_passes() {
        for first_source in [
            FolderPanePointerSource::Mouse,
            FolderPanePointerSource::Touch,
        ] {
            let second_source = match first_source {
                FolderPanePointerSource::Mouse => FolderPanePointerSource::Touch,
                FolderPanePointerSource::Touch => FolderPanePointerSource::Mouse,
            };
            for first_arrow in [false, true] {
                for second_arrow in [false, true] {
                    let mut state = FolderPaneState::default();
                    let first_hit = if first_arrow {
                        FolderPanePointerHit::Arrow(p("C:/a"))
                    } else {
                        FolderPanePointerHit::Label(p("C:/a"))
                    };
                    let second_hit = if second_arrow {
                        FolderPanePointerHit::Arrow(p("C:/a"))
                    } else {
                        FolderPanePointerHit::Label(p("C:/a"))
                    };
                    state.begin_label_input(1);
                    assert!(!pointer_press(
                        &mut state,
                        first_hit.clone(),
                        0.0,
                        1.0,
                        first_source,
                        true,
                        true,
                        first_arrow
                    ));
                    state.begin_label_input(2);
                    assert!(!pointer_press(
                        &mut state,
                        second_hit.clone(),
                        0.0,
                        1.1,
                        second_source,
                        true,
                        true,
                        second_arrow
                    ));
                    assert!(matches!(state.label_gesture, FolderPaneLabelGesture::Idle));
                    assert!(!pointer_release(
                        &mut state,
                        second_hit,
                        1.11,
                        second_source,
                        true,
                        true
                    ));
                    assert!(!pointer_release(
                        &mut state,
                        first_hit,
                        1.12,
                        first_source,
                        true,
                        true
                    ));
                    assert!(state.label_input.results.is_empty());
                }
            }
        }
    }

    #[test]
    fn label_click_mixed_source_cancel_keeps_committed_result_prefix() {
        for pending_phase in 0..3 {
            for touch_arrow in [false, true] {
                for same_pass in [false, true] {
                    let mut state = FolderPaneState::default();
                    state.begin_label_input(1);
                    label_press(&mut state, "C:/a", 0.0, 1.0);
                    label_release(&mut state, "C:/a", 1.01, true);
                    match pending_phase {
                        0 => label_press(&mut state, "C:/b", 20.0, 1.1),
                        1 => {
                            pointer_press(
                                &mut state,
                                FolderPanePointerHit::Arrow(p("C:/b")),
                                20.0,
                                1.1,
                                FolderPanePointerSource::Mouse,
                                true,
                                true,
                                true,
                            );
                        }
                        _ => label_press(&mut state, "C:/a", 0.0, 1.1),
                    }
                    let mut expected = Vec::new();
                    if same_pass {
                        expected.push(FolderPaneLabelResult::Single(p("C:/a")));
                        if pending_phase == 2 {
                            expected.push(FolderPaneLabelResult::Toggle {
                                path: p("C:/a"),
                                expanded: true,
                            });
                        }
                    } else {
                        state.begin_label_input(2);
                    }
                    let touch_hit = if touch_arrow {
                        FolderPanePointerHit::Arrow(p("C:/b"))
                    } else {
                        FolderPanePointerHit::Label(p("C:/b"))
                    };
                    assert!(!pointer_press(
                        &mut state,
                        touch_hit.clone(),
                        0.0,
                        1.11,
                        FolderPanePointerSource::Touch,
                        true,
                        true,
                        touch_arrow
                    ));
                    assert!(matches!(state.label_gesture, FolderPaneLabelGesture::Idle));
                    assert!(!pointer_release(
                        &mut state,
                        touch_hit,
                        1.12,
                        FolderPanePointerSource::Touch,
                        true,
                        true
                    ));
                    assert!(!pointer_release(
                        &mut state,
                        FolderPanePointerHit::Label(p("C:/a")),
                        1.13,
                        FolderPanePointerSource::Mouse,
                        true,
                        true
                    ));
                    assert_eq!(state.label_input.results, expected);
                }
            }
        }
    }

    fn p(s: &str) -> PathBuf {
        PathBuf::from(s)
    }

    #[test]
    fn virtual_tree_row_flags_keep_case_and_separator_equivalence() {
        let root = p(r"C:\Data");
        let child = root.join("Pictures");
        let mut state = FolderPaneState::test_tree(&root, vec![child.clone()]);
        state.active_path = Some(p("c:/data/PICTURES"));
        state.set_cursor(p("C:/DATA/pictures"));
        let rows = state.visible_rows();
        assert_eq!(
            rows.iter().map(|row| &row.path).collect::<Vec<_>>(),
            vec![&root, &child]
        );
        for row in rows {
            assert_eq!(
                row.is_active,
                crate::folder_tree::path_eq(&row.path, state.active_path().unwrap())
            );
            assert_eq!(
                row.is_cursor,
                crate::folder_tree::path_eq(&row.path, state.cursor_path().unwrap())
            );
        }
    }

    #[test]
    fn virtual_tree_row_index_reuses_normalized_identity_and_first_duplicate() {
        let root = p(r"C:\Data");
        let first = root.join("Pictures");
        let duplicate = p("c:/DATA/PICTURES");
        let state = FolderPaneState::test_tree(&root, vec![first.clone(), duplicate]);
        let (rows, indices) = state.visible_rows_indexed();
        assert_eq!(
            rows.iter().map(|row| &row.path).collect::<Vec<_>>(),
            vec![&root, &first]
        );
        assert_eq!(indices.len(), rows.len());
        assert_eq!(indices.get(&key_for(&p("C:/data/pictures"))), Some(&1));
        for (index, row) in rows.iter().enumerate() {
            assert_eq!(indices.get(&key_for(&row.path)), Some(&index));
        }
    }

    #[test]
    fn virtual_tree_hidden_membership_does_not_reveal_or_reach_hidden_children() {
        let root = p(r"C:\");
        let hidden = root.join("hidden");
        let active = hidden.join("active");
        let mut state = FolderPaneState::test_tree(&root, vec![]);
        state.nodes.get_mut(&key_for(&root)).unwrap().cache = Some(CachedListing::new(
            vec![],
            [key_for(&hidden)].into_iter().collect(),
            state.listing_options,
        ));
        state.active_path = Some(active);
        state.reveal_active(false);
        assert!(!state.nodes.contains_key(&key_for(&hidden)));
        assert!(!state.reachable(&hidden));
        let cache = state.nodes[&key_for(&root)].cache.as_ref().unwrap();
        assert!(cache.membership.contains(&key_for(&hidden)));
        assert!(!cache.child_keys.contains(&key_for(&hidden)));
    }

    #[test]
    fn virtual_tree_row_id_index_lifetime_ignores_freshness_and_logical_removal() {
        for reset in [
            FolderPaneGestureReset::DisplayNotFresh,
            FolderPaneGestureReset::PaneDisabled,
            FolderPaneGestureReset::SuspendDisplay,
            FolderPaneGestureReset::SelectDrive,
            FolderPaneGestureReset::SelectedDriveRemoved,
            FolderPaneGestureReset::ReloadForActive,
        ] {
            let mut state = FolderPaneState::test_tree(&p(r"C:\"), vec![]);
            let id = egui::Id::unique("row lifetime").accesskit_id();
            state
                .row_ids
                .0
                .lock()
                .unwrap()
                .insert(id, Arc::new(p(r"C:\child")));
            state.test_seed_listing(&p("C:/"), vec![], true);
            assert!(state.row_ids.0.lock().unwrap().contains_key(&id));
            state.invalidate_label_display(reset);
            let retained = matches!(
                reset,
                FolderPaneGestureReset::DisplayNotFresh | FolderPaneGestureReset::PaneDisabled
            );
            assert_eq!(
                state.row_ids.0.lock().unwrap().contains_key(&id),
                retained,
                "{reset:?}"
            );
        }
    }

    #[test]
    fn virtual_tree_row_id_index_clears_on_explicit_and_followed_drive_changes() {
        let mut state = FolderPaneState::test_tree(&p(r"C:\"), vec![]);
        let id = egui::Id::unique("drive lifetime").accesskit_id();
        state
            .row_ids
            .0
            .lock()
            .unwrap()
            .insert(id, Arc::new(p(r"C:\child")));
        state.set_cursor(p("c:/CHILD"));
        assert!(state.row_ids.0.lock().unwrap().contains_key(&id));
        state.set_cursor(p(r"D:\child"));
        assert!(state.row_ids.0.lock().unwrap().is_empty());
        state
            .row_ids
            .0
            .lock()
            .unwrap()
            .insert(id, Arc::new(p(r"D:\child")));
        state.active_path = Some(p(r"C:\child"));
        state.resolve_follow_position();
        assert!(state.row_ids.0.lock().unwrap().is_empty());
    }

    #[test]
    fn active_virtual_folder_maps_to_parent() {
        assert_eq!(
            active_filesystem_folder(Path::new(r"C:\books\vol.zip")),
            Some(p(r"C:\books"))
        );
        assert_eq!(
            active_filesystem_folder(Path::new(r"D:\docs\scan.pdf")),
            Some(p(r"D:\docs"))
        );
    }

    #[test]
    fn scan_real_subfolders_excludes_files_and_virtual_containers() {
        let tmp = tempfile::TempDir::new().expect("tempdir");
        std::fs::create_dir(tmp.path().join("b")).unwrap();
        std::fs::create_dir(tmp.path().join("a")).unwrap();
        std::fs::write(tmp.path().join("book.zip"), b"not a real tree folder").unwrap();
        std::fs::write(tmp.path().join("doc.pdf"), b"pdf").unwrap();
        let dirs =
            scan_real_subfolders(tmp.path(), FolderTreeSortOrder::NameAsc, false, None).unwrap();
        let labels: Vec<_> = dirs.iter().map(|path| folder_label(path)).collect();
        assert_eq!(labels, vec!["a", "b"]);
    }

    #[test]
    fn scan_real_subfolders_excludes_portable_metadata_bundle_even_when_showing_hidden() {
        let tmp = tempfile::TempDir::new().expect("tempdir");
        std::fs::create_dir(tmp.path().join("visible")).unwrap();
        std::fs::create_dir(
            tmp.path()
                .join(crate::fs_entry::PORTABLE_METADATA_BUNDLE_DIRNAME),
        )
        .unwrap();
        let dirs =
            scan_real_subfolders(tmp.path(), FolderTreeSortOrder::NameAsc, true, None).unwrap();
        let labels: Vec<_> = dirs.iter().map(|path| folder_label(path)).collect();
        assert_eq!(labels, vec!["visible"]);
    }

    #[cfg(windows)]
    #[test]
    fn scan_real_subfolders_includes_windows_directory_symlink() {
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let target = tmp.path().join("target");
        let link = tmp.path().join("link");
        std::fs::create_dir(&target).unwrap();
        if std::os::windows::fs::symlink_dir(&target, &link).is_err() {
            return;
        }

        let dirs =
            scan_real_subfolders(tmp.path(), FolderTreeSortOrder::NameAsc, false, None).unwrap();
        let labels: Vec<_> = dirs.iter().map(|path| folder_label(path)).collect();
        assert_eq!(labels, vec!["link", "target"]);
    }
}
