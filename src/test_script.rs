//! Opt-in Rhai runner for isolated in-process application tests.
//!
//! The worker evaluates scripts and sends typed commands only. Synthetic input
//! is materialized by `key_input`'s ROOT plugin, while App/UI state publication,
//! direct `KeyAction` delivery, failure classification, and shutdown stay on
//! the UI thread.

#![cfg_attr(all(test, not(feature = "test-script")), allow(dead_code))]

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock, RwLock, mpsc};
use std::time::{Duration, Instant};

use rhai::{Dynamic, Engine, EvalAltResult, FnPtr, ImmutableString, Map, NativeCallContext};

#[cfg(test)]
use crate::key_input::SyntheticKeyCommandKind;
use crate::key_input::{
    SyntheticInputIssue, SyntheticKeyCommand, SyntheticModifiers, SyntheticNavigationKey,
};
use crate::keymap::{KeyAction, KeyTrigger};

const MAX_SCRIPT_BYTES: u64 = 1024 * 1024;
const WAIT_POLL_INTERVAL: Duration = Duration::from_millis(10);
const EXIT_NOT_SET: i32 = -1;
const EXIT_SCRIPT_FAILURE: i32 = 1;
const EXIT_ENVIRONMENT_FAILURE: i32 = 2;
// App-owned workers get two seconds to join during normal shutdown. Six
// seconds leaves additional scheduling margin while still firing before the
// fixed wgpu device-drop timeout observed on contended Windows GPU systems.
const SHUTDOWN_WATCHDOG_GRACE: Duration = Duration::from_secs(6);

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct KeymapLevelObservation {
    pub(crate) frame_nr: u64,
    pub(crate) key: String,
    pub(crate) hold_ids: Vec<u64>,
    pub(crate) held: bool,
}

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
pub(crate) enum TestScriptPaintSourceKind {
    CatalogThumbnail,
    FullOrProcessed,
}

impl TestScriptPaintSourceKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::CatalogThumbnail => "catalog_thumbnail",
            Self::FullOrProcessed => "full_or_processed",
        }
    }

    fn preference(self) -> u8 {
        match self {
            Self::CatalogThumbnail => 0,
            Self::FullOrProcessed => 1,
        }
    }
}

/// Identity captured by the producer that selected the texture later painted.
///
/// This is diagnostic evidence only. It must travel with the selected resource;
/// reconstructing it from the current cache would incorrectly relabel a frozen
/// thumbnail after a full-resolution entry arrives.
#[derive(Clone, Debug, Hash, PartialEq, Eq)]
pub(crate) struct TestScriptContentProof {
    pub(crate) context_serial: u64,
    pub(crate) items_generation: u64,
    pub(crate) page_index: usize,
    pub(crate) item_identity: String,
    pub(crate) source_texture_id: egui::TextureId,
    pub(crate) source_kind: TestScriptPaintSourceKind,
}

/// Existing window lifetime owners represented without inventing a shared ID space.
///
/// The root content owner is the registry's main context. Detached host incarnations
/// come from `DetachedWindowManager`; they are not a second test-owned epoch. Both
/// variants also carry the eframe backend token for the exact live `winit::Window`
/// allocation, because an HWND alone can be reused while an older host is still alive.
/// Residence is deliberately absent because Mounted/AtRest is only the storage
/// location of the same logical viewer and host.
#[derive(Clone, Debug, Hash, PartialEq, Eq)]
pub(crate) enum TestScriptWindowIdentity {
    Root {
        context_serial: u64,
        hwnd: u64,
        backend_token: u64,
    },
    Detached {
        window_id: u64,
        context_serial: u64,
        viewport_id: egui::ViewportId,
        host_incarnation: u64,
        hwnd: u64,
        backend_token: u64,
    },
}

impl TestScriptWindowIdentity {
    pub(crate) fn role(&self) -> &'static str {
        match self {
            Self::Root { .. } => "root",
            Self::Detached { .. } => "detached",
        }
    }

    pub(crate) fn window_id(&self) -> Option<u64> {
        match self {
            Self::Root { .. } => None,
            Self::Detached { window_id, .. } => Some(*window_id),
        }
    }

    pub(crate) fn context_serial(&self) -> u64 {
        match self {
            Self::Root { context_serial, .. } | Self::Detached { context_serial, .. } => {
                *context_serial
            }
        }
    }

    pub(crate) fn viewport_id(&self) -> egui::ViewportId {
        match self {
            Self::Root { .. } => egui::ViewportId::ROOT,
            Self::Detached { viewport_id, .. } => *viewport_id,
        }
    }

    pub(crate) fn host_incarnation(&self) -> Option<u64> {
        match self {
            Self::Root { .. } => None,
            Self::Detached {
                host_incarnation, ..
            } => Some(*host_incarnation),
        }
    }

    pub(crate) fn hwnd(&self) -> u64 {
        match self {
            Self::Root { hwnd, .. } | Self::Detached { hwnd, .. } => *hwnd,
        }
    }

    pub(crate) fn backend_token(&self) -> u64 {
        match self {
            Self::Root { backend_token, .. } | Self::Detached { backend_token, .. } => {
                *backend_token
            }
        }
    }

    pub(crate) fn matches_backend_witness(
        &self,
        witness: eframe::miv_test_script_window_witness::WindowWitness,
    ) -> bool {
        self.viewport_id() == witness.viewport_id()
            && self.hwnd() == witness.hwnd()
            && self.backend_token() == witness.token()
    }

    pub(crate) fn describe(&self) -> String {
        match self {
            Self::Root {
                context_serial,
                hwnd,
                backend_token,
            } => format!("root/context={context_serial}/hwnd=0x{hwnd:x}/backend={backend_token}"),
            Self::Detached {
                window_id,
                context_serial,
                viewport_id,
                host_incarnation,
                hwnd,
                backend_token,
            } => format!(
                "detached/window={window_id}/context={context_serial}/viewport={viewport_id:?}/host={host_incarnation}/hwnd=0x{hwnd:x}/backend={backend_token}"
            ),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
enum TestScriptActionSelection {
    #[default]
    LegacyImplicit,
    Targeted(TestScriptWindowIdentity),
}

impl TestScriptActionSelection {
    fn mode(&self) -> &'static str {
        match self {
            Self::LegacyImplicit => "legacy_implicit",
            Self::Targeted(_) => "targeted",
        }
    }
}

#[derive(Clone, Debug, Hash, PartialEq, Eq)]
struct TestScriptPaintEvidenceKey {
    owner: TestScriptWindowIdentity,
    content: TestScriptContentProof,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct TestScriptWindowSnapshot {
    pub(crate) identity: Option<TestScriptWindowIdentity>,
    pub(crate) role: String,
    pub(crate) window_id: Option<u64>,
    pub(crate) context_serial: u64,
    pub(crate) viewport_id: egui::ViewportId,
    pub(crate) host_incarnation: Option<u64>,
    pub(crate) hwnd: Option<u64>,
    pub(crate) backend_token: Option<u64>,
    pub(crate) residence: String,
    pub(crate) media_kind: String,
    pub(crate) page_index: Option<usize>,
    pub(crate) items_generation: u64,
    pub(crate) item_identity: String,
    pub(crate) page_ready: bool,
    pub(crate) viewport_rendered: bool,
    pub(crate) viewport_revision: u64,
    pub(crate) paint_matches_current_page: bool,
    pub(crate) full_texture_painted: bool,
    pub(crate) paint_source: String,
    pub(crate) paint_source_texture: String,
    pub(crate) painted_page_index: Option<usize>,
    pub(crate) paint_revision: u64,
}

impl TestScriptWindowSnapshot {
    fn current_content_matches(&self, proof: &TestScriptContentProof) -> bool {
        self.context_serial == proof.context_serial
            && self.items_generation == proof.items_generation
            && self.page_index == Some(proof.page_index)
            && self.item_identity == proof.item_identity
    }

    fn accepts_owner(&self, owner: &TestScriptWindowIdentity) -> bool {
        self.identity.as_ref() == Some(owner)
    }

    fn to_rhai_map(&self) -> Map {
        let mut map = Map::new();
        map.insert("role".into(), self.role.clone().into());
        map.insert(
            "window_id".into(),
            self.window_id
                .map(|value| Dynamic::from(saturating_rhai_int(value)))
                .unwrap_or(Dynamic::UNIT),
        );
        map.insert(
            "context_serial".into(),
            saturating_rhai_int(self.context_serial).into(),
        );
        map.insert("viewport".into(), format!("{:?}", self.viewport_id).into());
        map.insert(
            "host_incarnation".into(),
            self.host_incarnation
                .map(|value| Dynamic::from(saturating_rhai_int(value)))
                .unwrap_or(Dynamic::UNIT),
        );
        map.insert(
            "hwnd".into(),
            self.hwnd
                .map(|value| Dynamic::from(format!("0x{value:x}")))
                .unwrap_or(Dynamic::UNIT),
        );
        map.insert(
            "backend_token".into(),
            self.backend_token
                .map(|value| Dynamic::from(saturating_rhai_int(value)))
                .unwrap_or(Dynamic::UNIT),
        );
        map.insert("host_ready".into(), self.identity.is_some().into());
        map.insert("residence".into(), self.residence.clone().into());
        map.insert("media_kind".into(), self.media_kind.clone().into());
        map.insert(
            "page_index".into(),
            self.page_index
                .map_or(-1, |value| i64::try_from(value).unwrap_or(i64::MAX))
                .into(),
        );
        map.insert(
            "items_generation".into(),
            saturating_rhai_int(self.items_generation).into(),
        );
        map.insert("item_identity".into(), self.item_identity.clone().into());
        map.insert("page_ready".into(), self.page_ready.into());
        map.insert("viewport_rendered".into(), self.viewport_rendered.into());
        map.insert(
            "viewport_revision".into(),
            saturating_rhai_int(self.viewport_revision).into(),
        );
        map.insert(
            "paint_matches_current_page".into(),
            self.paint_matches_current_page.into(),
        );
        map.insert(
            "full_texture_painted".into(),
            self.full_texture_painted.into(),
        );
        map.insert("paint_source".into(), self.paint_source.clone().into());
        map.insert(
            "paint_source_texture".into(),
            self.paint_source_texture.clone().into(),
        );
        map.insert(
            "painted_page_index".into(),
            self.painted_page_index
                .map_or(-1, |value| i64::try_from(value).unwrap_or(i64::MAX))
                .into(),
        );
        map.insert(
            "paint_revision".into(),
            saturating_rhai_int(self.paint_revision).into(),
        );
        map
    }
}

fn saturating_rhai_int(value: u64) -> rhai::INT {
    i64::try_from(value).unwrap_or(i64::MAX)
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct TestScriptSnapshot {
    pub(crate) is_fullscreen: bool,
    pub(crate) fs_idx: i64,
    pub(crate) items_generation: i64,
    pub(crate) focused: bool,
    pub(crate) target_viewport: String,
    pub(crate) target_registered: bool,
    pub(crate) target_rendered: bool,
    /// 現在のフォルダに並んでいる item 数。
    ///
    /// `pending_thumbs == 0` だけでは「全部終わった」と「まだ何も始まっていない」を
    /// 区別できない。落ち着いたことを待つ条件には `items_len > 0` を併せて使う。
    pub(crate) items_len: i64,
    pub(crate) pending_thumbs: i64,
    pub(crate) spread_mode: String,
    pub(crate) continuous_reading: bool,
    pub(crate) current_is_still_image: bool,
    pub(crate) music_view_active: bool,
    pub(crate) modal_open: bool,
    pub(crate) context_menu_open: bool,
    pub(crate) popup_open: bool,
    pub(crate) ime_active: bool,
    pub(crate) text_input_or_pending_focus: bool,
    pub(crate) overlay_edit_active: bool,
    pub(crate) capture_region_selection: bool,
    pub(crate) fullscreen_raw_key_permit: bool,
    pub(crate) has_previous_page: bool,
    pub(crate) has_next_page: bool,
    /// Consecutive frames whose page-turn decision deferred texture uploads. A number that keeps
    /// climbing while nothing is being pressed is the livelock, not slow loading.
    /// Which continuation the viewer is in: `paged`, `vertical` or `horizontal`. `continuous_reading`
    /// says only that it is not paged, and the two continuations lay pages out differently enough
    /// that a scenario reproducing a layout-dependent failure has to name the one it means.
    pub(crate) reading_flow: String,
    pub(crate) upload_deferral_streak: i64,
    /// Why the page has no stand-in to show, or empty. See `PassthroughUnavailable`.
    pub(crate) passthrough_unavailable: String,
    pub(crate) keymap_level_observations: Vec<KeymapLevelObservation>,
    pub(crate) windows: Vec<TestScriptWindowSnapshot>,
}

impl Default for TestScriptSnapshot {
    fn default() -> Self {
        Self {
            is_fullscreen: false,
            fs_idx: -1,
            items_generation: 0,
            focused: false,
            target_viewport: "unregistered".to_string(),
            target_registered: false,
            target_rendered: false,
            items_len: 0,
            pending_thumbs: 0,
            spread_mode: "Single".to_string(),
            continuous_reading: false,
            current_is_still_image: false,
            music_view_active: false,
            modal_open: false,
            context_menu_open: false,
            popup_open: false,
            ime_active: false,
            text_input_or_pending_focus: false,
            overlay_edit_active: false,
            capture_region_selection: false,
            fullscreen_raw_key_permit: false,
            has_previous_page: false,
            has_next_page: false,
            reading_flow: "paged".to_string(),
            upload_deferral_streak: 0,
            passthrough_unavailable: String::new(),
            keymap_level_observations: Vec::new(),
            windows: Vec::new(),
        }
    }
}

impl TestScriptSnapshot {
    fn to_rhai_map(&self) -> Map {
        let mut map = Map::new();
        macro_rules! insert {
            ($field:ident) => {
                map.insert(
                    stringify!($field).into(),
                    Dynamic::from(self.$field.clone()),
                );
            };
        }
        insert!(is_fullscreen);
        insert!(fs_idx);
        insert!(items_generation);
        insert!(focused);
        insert!(target_viewport);
        insert!(target_registered);
        insert!(target_rendered);
        insert!(items_len);
        insert!(pending_thumbs);
        insert!(spread_mode);
        insert!(continuous_reading);
        insert!(current_is_still_image);
        insert!(music_view_active);
        insert!(modal_open);
        insert!(context_menu_open);
        insert!(popup_open);
        insert!(ime_active);
        insert!(text_input_or_pending_focus);
        insert!(overlay_edit_active);
        insert!(capture_region_selection);
        insert!(fullscreen_raw_key_permit);
        insert!(has_previous_page);
        insert!(has_next_page);
        insert!(reading_flow);
        insert!(upload_deferral_streak);
        insert!(passthrough_unavailable);
        map.insert(
            "windows".into(),
            self.windows
                .iter()
                .map(|window| Dynamic::from_map(window.to_rhai_map()))
                .collect::<rhai::Array>()
                .into(),
        );
        map
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ScriptOutcomeKind {
    Success,
    ScriptFailure,
    EnvironmentFailure,
}

impl ScriptOutcomeKind {
    fn exit_code(self) -> i32 {
        match self {
            Self::Success => 0,
            Self::ScriptFailure => EXIT_SCRIPT_FAILURE,
            Self::EnvironmentFailure => EXIT_ENVIRONMENT_FAILURE,
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Success => "success",
            Self::ScriptFailure => "script_failure",
            Self::EnvironmentFailure => "environment_failure",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ScriptOutcome {
    kind: ScriptOutcomeKind,
    message: String,
}

impl ScriptOutcome {
    fn success() -> Self {
        Self {
            kind: ScriptOutcomeKind::Success,
            message: "script completed".to_string(),
        }
    }

    fn script_failure(message: impl Into<String>) -> Self {
        Self {
            kind: ScriptOutcomeKind::ScriptFailure,
            message: message.into(),
        }
    }

    fn environment_failure(message: impl Into<String>) -> Self {
        Self {
            kind: ScriptOutcomeKind::EnvironmentFailure,
            message: message.into(),
        }
    }

    fn override_with_environment_failure(&mut self, message: impl Into<String>) {
        self.kind = ScriptOutcomeKind::EnvironmentFailure;
        self.message = message.into();
    }
}

#[derive(Debug)]
struct PreconditionTrace {
    name: &'static str,
    satisfied: bool,
    timeout_ms: Option<u64>,
    elapsed_ms: u64,
    target_registered: Option<bool>,
    focused: Option<bool>,
}

#[derive(Debug)]
enum UiCommand {
    Key(SyntheticKeyCommand),
    Cancel(Instant),
    SetRepeat {
        delay: Duration,
        hz: f64,
    },
    RunAction {
        action: KeyAction,
        selection: TestScriptActionSelection,
        applied: mpsc::SyncSender<Result<(), String>>,
    },
    Log(String),
    Precondition(PreconditionTrace),
    Finished(ScriptOutcome),
}

#[derive(Default)]
struct InterruptState {
    failure: Mutex<Option<String>>,
    changed: Condvar,
}

impl InterruptState {
    fn fail(&self, message: impl Into<String>) {
        let Ok(mut guard) = self.failure.lock() else {
            return;
        };
        if guard.is_none() {
            *guard = Some(message.into());
            self.changed.notify_all();
        }
    }

    fn check(&self) -> Result<(), String> {
        self.failure
            .lock()
            .map_err(|_| "script interrupt state is poisoned".to_string())?
            .clone()
            .map_or(Ok(()), Err)
    }

    fn failure_message(&self) -> Option<String> {
        self.failure.lock().ok().and_then(|guard| guard.clone())
    }

    /// Sleep for `duration`, returning early only if the run has actually failed.
    ///
    /// The loop is the point. `wait_timeout` returns on spurious wakeups as well as on notify, so
    /// taking the first wakeup as "the wait is over" makes a hold last however long the condvar
    /// felt like: a twenty-second burst returned in 45ms, the script sailed past its assertions
    /// because nothing had happened yet, and the run reported success. A scenario that silently
    /// does not run is worse than one that fails.
    fn wait(&self, duration: Duration) -> Result<(), String> {
        let deadline = std::time::Instant::now() + duration;
        let mut guard = self
            .failure
            .lock()
            .map_err(|_| "script interrupt state is poisoned".to_string())?;
        loop {
            if let Some(message) = guard.as_ref() {
                return Err(message.clone());
            }
            let now = std::time::Instant::now();
            if now >= deadline {
                return Ok(());
            }
            let (next, _) = self
                .changed
                .wait_timeout(guard, deadline - now)
                .map_err(|_| "script interrupt state is poisoned".to_string())?;
            guard = next;
        }
    }
}

#[derive(Clone)]
struct RunnerBridge {
    tx: mpsc::Sender<UiCommand>,
    snapshot: Arc<RwLock<TestScriptSnapshot>>,
    interrupt: Arc<InterruptState>,
    wake: Arc<dyn Fn() + Send + Sync>,
    next_hold_id: Arc<AtomicU64>,
    action_selection: Arc<Mutex<TestScriptActionSelection>>,
}

impl RunnerBridge {
    fn send(&self, command: UiCommand) -> Result<(), String> {
        self.interrupt.check()?;
        self.send_unchecked(command)
    }

    fn send_unchecked(&self, command: UiCommand) -> Result<(), String> {
        self.tx
            .send(command)
            .map_err(|_| "test-script UI command channel disconnected".to_string())?;
        (self.wake)();
        Ok(())
    }

    fn latest_snapshot(&self) -> Result<TestScriptSnapshot, String> {
        self.snapshot
            .read()
            .map(|snapshot| snapshot.clone())
            .map_err(|_| "test-script snapshot is poisoned".to_string())
    }

    fn require_key_target(&self) -> Result<(), String> {
        let snapshot = self.latest_snapshot()?;
        let satisfied = snapshot.target_registered && snapshot.focused;
        let _ = self.send_unchecked(UiCommand::Precondition(PreconditionTrace {
            name: "key_target",
            satisfied,
            timeout_ms: None,
            elapsed_ms: 0,
            target_registered: Some(snapshot.target_registered),
            focused: Some(snapshot.focused),
        }));
        if !snapshot.target_registered {
            return Err(
                "synthetic key target is not registered; wait_until(|s| s.target_registered, timeout_ms) before sending input"
                    .to_string(),
            );
        }
        if !snapshot.focused {
            return Err(
                "synthetic key target is not focused; wait_until(|s| s.focused, timeout_ms) before sending input"
                    .to_string(),
            );
        }
        Ok(())
    }

    fn allocate_hold_id(&self) -> u64 {
        self.next_hold_id.fetch_add(1, Ordering::Relaxed) + 1
    }

    fn action_selection(&self) -> Result<TestScriptActionSelection, String> {
        self.action_selection
            .lock()
            .map(|selection| selection.clone())
            .map_err(|_| "test-script action selection is poisoned".to_string())
    }

    fn select_root(&self) -> Result<Map, String> {
        let snapshot = self.latest_snapshot()?;
        let window = snapshot
            .windows
            .iter()
            .find(|window| window.role == "root")
            .ok_or_else(|| "select_root could not find the root window".to_string())?;
        self.select_window_snapshot(window)
    }

    fn select_window(&self, window_id: u64, context_serial: u64) -> Result<Map, String> {
        let snapshot = self.latest_snapshot()?;
        let window = snapshot
            .windows
            .iter()
            .find(|window| {
                window.role == "detached"
                    && window.window_id == Some(window_id)
                    && window.context_serial == context_serial
            })
            .ok_or_else(|| {
                format!(
                    "select_window target is not current: window={window_id} context={context_serial}"
                )
            })?;
        self.select_window_snapshot(window)
    }

    fn select_window_snapshot(&self, window: &TestScriptWindowSnapshot) -> Result<Map, String> {
        let identity = window
            .identity
            .clone()
            .ok_or_else(|| format!("select_{} target has no current host identity", window.role))?;
        let mut selection = self
            .action_selection
            .lock()
            .map_err(|_| "test-script action selection is poisoned".to_string())?;
        *selection = TestScriptActionSelection::Targeted(identity.clone());
        drop(selection);

        let mut selected = window.to_rhai_map();
        selected.insert("target_mode".into(), Dynamic::from("targeted"));
        selected.insert("current".into(), Dynamic::from(true));
        let _ = self.send_unchecked(UiCommand::Log(format!(
            "action target selected mode=targeted owner={}",
            identity.describe()
        )));
        Ok(selected)
    }

    fn selected_target(&self) -> Result<Map, String> {
        let selection = self.action_selection()?;
        let snapshot = self.latest_snapshot()?;
        let mut selected = Map::new();
        selected.insert("target_mode".into(), Dynamic::from(selection.mode()));
        match selection {
            TestScriptActionSelection::LegacyImplicit => {
                selected.insert("current".into(), Dynamic::from(true));
                selected.insert("role".into(), Dynamic::from("implicit"));
            }
            TestScriptActionSelection::Targeted(identity) => {
                let current = snapshot
                    .windows
                    .iter()
                    .any(|window| window.identity.as_ref() == Some(&identity));
                selected.insert("current".into(), Dynamic::from(current));
                selected.insert("role".into(), Dynamic::from(identity.role()));
                selected.insert(
                    "context_serial".into(),
                    Dynamic::from(saturating_rhai_int(identity.context_serial())),
                );
                selected.insert(
                    "window_id".into(),
                    identity
                        .window_id()
                        .map(|value| Dynamic::from(saturating_rhai_int(value)))
                        .unwrap_or(Dynamic::UNIT),
                );
            }
        }
        Ok(selected)
    }
}

fn emit_perf_step(message: &str) {
    if !crate::perf::is_enabled() {
        return;
    }
    crate::perf::event(
        "test_script",
        "step",
        None,
        0,
        &[("message", serde_json::Value::from(message))],
    );
}

fn emit_perf_precondition(trace: &PreconditionTrace) {
    if !crate::perf::is_enabled() {
        return;
    }
    let mut extras = vec![
        ("name", serde_json::Value::from(trace.name)),
        ("satisfied", serde_json::Value::from(trace.satisfied)),
        ("elapsed_ms", serde_json::Value::from(trace.elapsed_ms)),
    ];
    if let Some(timeout_ms) = trace.timeout_ms {
        extras.push(("timeout_ms", serde_json::Value::from(timeout_ms)));
    }
    if let Some(target_registered) = trace.target_registered {
        extras.push((
            "target_registered",
            serde_json::Value::from(target_registered),
        ));
    }
    if let Some(focused) = trace.focused {
        extras.push(("focused", serde_json::Value::from(focused)));
    }
    crate::perf::event("test_script", "precondition", None, 0, &extras);
}

fn emit_perf_fail(outcome: &ScriptOutcome) {
    if outcome.kind == ScriptOutcomeKind::Success || !crate::perf::is_enabled() {
        return;
    }
    crate::perf::event(
        "test_script",
        "fail",
        None,
        0,
        &[
            (
                "failure_kind",
                serde_json::Value::from(outcome.kind.as_str()),
            ),
            (
                "exit_code",
                serde_json::Value::from(outcome.kind.exit_code()),
            ),
            ("message", serde_json::Value::from(outcome.message.as_str())),
        ],
    );
}

fn emit_perf_level_reads(observations: &[KeymapLevelObservation]) {
    if !crate::perf::is_enabled() {
        return;
    }
    for observation in observations {
        for hold_id in &observation.hold_ids {
            crate::perf::event(
                "test_script",
                "level_read",
                Some(&observation.key),
                0,
                &[
                    ("hold_id", serde_json::Value::from(*hold_id)),
                    ("held", serde_json::Value::from(observation.held)),
                    ("frame_nr", serde_json::Value::from(observation.frame_nr)),
                    ("reader", serde_json::Value::from("Keymap::key_held_chord")),
                ],
            );
        }
    }
}

fn rhai_error(message: impl Into<String>) -> Box<EvalAltResult> {
    EvalAltResult::ErrorRuntime(Dynamic::from(message.into()), rhai::Position::NONE).into()
}

fn checked_duration(ms: rhai::INT, argument: &str) -> Result<Duration, Box<EvalAltResult>> {
    let ms =
        u64::try_from(ms).map_err(|_| rhai_error(format!("{argument} must be zero or greater")))?;
    Ok(Duration::from_millis(ms))
}

fn checked_u64(value: rhai::INT, argument: &str) -> Result<u64, Box<EvalAltResult>> {
    u64::try_from(value).map_err(|_| rhai_error(format!("{argument} must be zero or greater")))
}

fn parse_navigation_key(name: &str) -> Result<SyntheticNavigationKey, Box<EvalAltResult>> {
    let key = match name.trim().to_ascii_lowercase().as_str() {
        "right" | "arrowright" => SyntheticNavigationKey::Right,
        "left" | "arrowleft" => SyntheticNavigationKey::Left,
        "up" | "arrowup" => SyntheticNavigationKey::Up,
        "down" | "arrowdown" => SyntheticNavigationKey::Down,
        "pageup" => SyntheticNavigationKey::PageUp,
        "pagedown" => SyntheticNavigationKey::PageDown,
        "home" => SyntheticNavigationKey::Home,
        "end" => SyntheticNavigationKey::End,
        "enter" => SyntheticNavigationKey::Enter,
        "escape" | "esc" => SyntheticNavigationKey::Escape,
        _ => {
            return Err(rhai_error(format!(
                "unsupported synthetic navigation key: {name}"
            )));
        }
    };
    Ok(key)
}

/// Parse a modifier spec such as `"ctrl"`, `"ctrl+shift"` or `""`.
///
/// Ctrl is what makes folder navigation (Ctrl+Up/Down) expressible, and the timeline already
/// answers `GetAsyncKeyState` for the modifier VKs from the held set, so a scripted Ctrl chord
/// reaches both the keymap's OS-level reads and egui's event modifiers - the same two
/// representations a physical press produces.
fn parse_modifiers(spec: &str) -> Result<SyntheticModifiers, Box<EvalAltResult>> {
    let mut modifiers = SyntheticModifiers::default();
    let spec = spec.trim();
    if spec.is_empty() || spec.eq_ignore_ascii_case("none") {
        return Ok(modifiers);
    }
    for part in spec.split(['+', ',']) {
        match part.trim().to_ascii_lowercase().as_str() {
            "" => {}
            "ctrl" | "control" => modifiers.ctrl = true,
            "shift" => modifiers.shift = true,
            "alt" => modifiers.alt = true,
            other => {
                return Err(rhai_error(format!(
                    "unsupported synthetic modifier: {other} (in {spec:?})"
                )));
            }
        }
    }
    Ok(modifiers)
}

fn hold_key_impl(
    bridge: &RunnerBridge,
    name: &str,
    modifiers: SyntheticModifiers,
    ms: rhai::INT,
) -> Result<(), Box<EvalAltResult>> {
    let key = parse_navigation_key(name)?;
    let duration = checked_duration(ms, "hold_key ms")?;
    bridge.require_key_target().map_err(rhai_error)?;
    let hold_id = bridge.allocate_hold_id();
    bridge
        .send(UiCommand::Key(
            SyntheticKeyCommand::down(Instant::now(), key, modifiers).with_hold_id(hold_id),
        ))
        .map_err(rhai_error)?;
    if let Err(error) = wait_interruptibly(&bridge.interrupt, duration) {
        let _ = bridge.send_unchecked(UiCommand::Key(
            SyntheticKeyCommand::up(Instant::now(), key).with_hold_id(hold_id),
        ));
        return Err(error);
    }
    bridge
        .send(UiCommand::Key(
            SyntheticKeyCommand::up(Instant::now(), key).with_hold_id(hold_id),
        ))
        .map_err(rhai_error)
}

fn tap_key_impl(
    bridge: &RunnerBridge,
    name: &str,
    modifiers: SyntheticModifiers,
) -> Result<(), Box<EvalAltResult>> {
    let key = parse_navigation_key(name)?;
    bridge.require_key_target().map_err(rhai_error)?;
    bridge
        .send(UiCommand::Key(SyntheticKeyCommand::down(
            Instant::now(),
            key,
            modifiers,
        )))
        .map_err(rhai_error)?;
    bridge
        .send(UiCommand::Key(SyntheticKeyCommand::up(Instant::now(), key)))
        .map_err(rhai_error)
}

fn parse_action(name: &str) -> Result<KeyAction, Box<EvalAltResult>> {
    let action = KeyAction::from_ini_name(name)
        .ok_or_else(|| rhai_error(format!("unknown KeyAction ini name: {name}")))?;
    if action.trigger() != KeyTrigger::Press {
        return Err(rhai_error(format!(
            "run_action only accepts one-shot Press actions: {name}"
        )));
    }
    Ok(action)
}

fn wait_interruptibly(
    interrupt: &InterruptState,
    duration: Duration,
) -> Result<(), Box<EvalAltResult>> {
    interrupt.wait(duration).map_err(rhai_error)
}

fn register_runner_api(engine: &mut Engine, bridge: RunnerBridge) {
    let select_root_bridge = bridge.clone();
    engine.register_fn("select_root", move || -> Result<Map, Box<EvalAltResult>> {
        select_root_bridge.select_root().map_err(rhai_error)
    });

    let select_window_bridge = bridge.clone();
    engine.register_fn(
        "select_window",
        move |window_id: rhai::INT, context_serial: rhai::INT| -> Result<Map, Box<EvalAltResult>> {
            select_window_bridge
                .select_window(
                    checked_u64(window_id, "select_window window_id")?,
                    checked_u64(context_serial, "select_window context_serial")?,
                )
                .map_err(rhai_error)
        },
    );

    let selected_target_bridge = bridge.clone();
    engine.register_fn(
        "selected_target",
        move || -> Result<Map, Box<EvalAltResult>> {
            selected_target_bridge.selected_target().map_err(rhai_error)
        },
    );

    let hold_bridge = bridge.clone();
    engine.register_fn(
        "hold_key",
        move |name: ImmutableString, ms: rhai::INT| -> Result<(), Box<EvalAltResult>> {
            hold_key_impl(&hold_bridge, &name, SyntheticModifiers::default(), ms)
        },
    );

    // `hold_key("Down", "ctrl", 3000)` - folder navigation is a Ctrl chord, so without this
    // overload the harness cannot express the input that both of the v3.0.0 fullscreen defects
    // start from.
    let hold_mod_bridge = bridge.clone();
    engine.register_fn(
        "hold_key",
        move |name: ImmutableString,
              modifiers: ImmutableString,
              ms: rhai::INT|
              -> Result<(), Box<EvalAltResult>> {
            let modifiers = parse_modifiers(&modifiers)?;
            hold_key_impl(&hold_mod_bridge, &name, modifiers, ms)
        },
    );

    let tap_bridge = bridge.clone();
    engine.register_fn(
        "tap_key",
        move |name: ImmutableString| -> Result<(), Box<EvalAltResult>> {
            tap_key_impl(&tap_bridge, &name, SyntheticModifiers::default())
        },
    );

    let tap_mod_bridge = bridge.clone();
    engine.register_fn(
        "tap_key",
        move |name: ImmutableString,
              modifiers: ImmutableString|
              -> Result<(), Box<EvalAltResult>> {
            let modifiers = parse_modifiers(&modifiers)?;
            tap_key_impl(&tap_mod_bridge, &name, modifiers)
        },
    );

    let release_bridge = bridge.clone();
    engine.register_fn(
        "release_key",
        move |name: ImmutableString| -> Result<(), Box<EvalAltResult>> {
            let key = parse_navigation_key(&name)?;
            release_bridge
                .send(UiCommand::Key(SyntheticKeyCommand::up(Instant::now(), key)))
                .map_err(rhai_error)
        },
    );

    let action_bridge = bridge.clone();
    engine.register_fn(
        "run_action",
        move |name: ImmutableString| -> Result<(), Box<EvalAltResult>> {
            let action = parse_action(&name)?;
            let selection = action_bridge.action_selection().map_err(rhai_error)?;
            let (applied_tx, applied_rx) = mpsc::sync_channel(1);
            action_bridge
                .send(UiCommand::RunAction {
                    action,
                    selection,
                    applied: applied_tx,
                })
                .map_err(rhai_error)?;
            loop {
                action_bridge.interrupt.check().map_err(rhai_error)?;
                match applied_rx.recv_timeout(WAIT_POLL_INTERVAL) {
                    Ok(Ok(())) => return Ok(()),
                    Ok(Err(message)) => return Err(rhai_error(message)),
                    Err(mpsc::RecvTimeoutError::Timeout) => {}
                    Err(mpsc::RecvTimeoutError::Disconnected) => {
                        return Err(rhai_error("run_action apply acknowledgement disconnected"));
                    }
                }
            }
        },
    );

    let sleep_bridge = bridge.clone();
    engine.register_fn(
        "sleep",
        move |ms: rhai::INT| -> Result<(), Box<EvalAltResult>> {
            wait_interruptibly(&sleep_bridge.interrupt, checked_duration(ms, "sleep ms")?)
        },
    );

    let wait_bridge = bridge.clone();
    engine.register_fn(
        "wait_until",
        move |ctx: NativeCallContext,
              condition: FnPtr,
              timeout_ms: rhai::INT|
              -> Result<(), Box<EvalAltResult>> {
            let timeout = checked_duration(timeout_ms, "wait_until timeout_ms")?;
            let started = Instant::now();
            loop {
                wait_bridge.interrupt.check().map_err(rhai_error)?;
                let snapshot = wait_bridge.latest_snapshot().map_err(rhai_error)?;
                if condition.call_within_context::<bool>(&ctx, (snapshot.to_rhai_map(),))? {
                    wait_bridge
                        .send(UiCommand::Precondition(PreconditionTrace {
                            name: "wait_until",
                            satisfied: true,
                            timeout_ms: Some(timeout.as_millis() as u64),
                            elapsed_ms: started.elapsed().as_millis().min(u128::from(u64::MAX))
                                as u64,
                            target_registered: Some(snapshot.target_registered),
                            focused: Some(snapshot.focused),
                        }))
                        .map_err(rhai_error)?;
                    return Ok(());
                }
                let elapsed = started.elapsed();
                if elapsed >= timeout {
                    let _ =
                        wait_bridge.send_unchecked(UiCommand::Precondition(PreconditionTrace {
                            name: "wait_until",
                            satisfied: false,
                            timeout_ms: Some(timeout.as_millis() as u64),
                            elapsed_ms: elapsed.as_millis().min(u128::from(u64::MAX)) as u64,
                            target_registered: Some(snapshot.target_registered),
                            focused: Some(snapshot.focused),
                        }));
                    return Err(rhai_error(format!(
                        "wait_until timed out after {} ms",
                        timeout.as_millis()
                    )));
                }
                wait_interruptibly(
                    &wait_bridge.interrupt,
                    WAIT_POLL_INTERVAL.min(timeout.saturating_sub(elapsed)),
                )?;
            }
        },
    );

    // 条件を待つのではなく、いまの状態をそのまま読む。`wait_until` は真になるまで
    // 待つので、「どちらに転んだか」で分岐するシナリオが書けなかった。
    let snapshot_bridge = bridge.clone();
    engine.register_fn("snapshot", move || -> Result<Map, Box<EvalAltResult>> {
        let snapshot = snapshot_bridge.latest_snapshot().map_err(rhai_error)?;
        Ok(snapshot.to_rhai_map())
    });

    let repeat_float_bridge = bridge.clone();
    engine.register_fn(
        "set_repeat",
        move |delay_ms: rhai::INT, hz: rhai::FLOAT| -> Result<(), Box<EvalAltResult>> {
            let delay = checked_duration(delay_ms, "set_repeat delay_ms")?;
            if !hz.is_finite() || hz <= 0.0 {
                return Err(rhai_error(
                    "set_repeat hz must be finite and greater than zero",
                ));
            }
            repeat_float_bridge
                .send(UiCommand::SetRepeat { delay, hz })
                .map_err(rhai_error)
        },
    );

    let repeat_int_bridge = bridge.clone();
    engine.register_fn(
        "set_repeat",
        move |delay_ms: rhai::INT, hz: rhai::INT| -> Result<(), Box<EvalAltResult>> {
            let delay = checked_duration(delay_ms, "set_repeat delay_ms")?;
            if hz <= 0 {
                return Err(rhai_error("set_repeat hz must be greater than zero"));
            }
            repeat_int_bridge
                .send(UiCommand::SetRepeat {
                    delay,
                    hz: hz as f64,
                })
                .map_err(rhai_error)
        },
    );

    let log_bridge = bridge.clone();
    engine.register_fn(
        "log",
        move |message: ImmutableString| -> Result<(), Box<EvalAltResult>> {
            log_bridge
                .send(UiCommand::Log(message.to_string()))
                .map_err(rhai_error)
        },
    );

    engine.register_fn(
        "fail",
        move |message: ImmutableString| -> Result<(), Box<EvalAltResult>> {
            Err(rhai_error(format!("script fail: {message}")))
        },
    );
}

fn build_engine(bridge: RunnerBridge) -> Engine {
    let mut engine = Engine::new();
    engine.set_max_operations(100_000_000);
    engine.set_max_call_levels(64);
    engine.set_max_expr_depths(64, 64);
    engine.set_max_string_size(16 * 1024 * 1024);
    engine.set_max_array_size(1_000_000);
    engine.set_max_map_size(1_000_000);
    engine.disable_symbol("eval");
    engine.disable_symbol("import");

    let interrupt = Arc::clone(&bridge.interrupt);
    engine.on_progress(move |operations| {
        if operations & 0xFFFF == 0
            && let Err(message) = interrupt.check()
        {
            Some(Dynamic::from(message))
        } else {
            None
        }
    });
    register_runner_api(&mut engine, bridge);
    engine
}

fn evaluate_source(source: &str, bridge: RunnerBridge) -> ScriptOutcome {
    let engine = build_engine(bridge.clone());
    match engine.eval::<Dynamic>(source) {
        Ok(_) => bridge
            .interrupt
            .failure_message()
            .map(ScriptOutcome::environment_failure)
            .unwrap_or_else(ScriptOutcome::success),
        Err(error) => bridge
            .interrupt
            .failure_message()
            .map(ScriptOutcome::environment_failure)
            .unwrap_or_else(|| ScriptOutcome::script_failure(error.to_string())),
    }
}

fn load_script(path: &Path) -> Result<String, String> {
    let metadata = std::fs::metadata(path)
        .map_err(|error| format!("failed to inspect test script {}: {error}", path.display()))?;
    if !metadata.is_file() {
        return Err(format!("test script is not a file: {}", path.display()));
    }
    if metadata.len() > MAX_SCRIPT_BYTES {
        return Err(format!(
            "test script exceeds {} bytes: {}",
            MAX_SCRIPT_BYTES,
            path.display()
        ));
    }
    std::fs::read_to_string(path)
        .map_err(|error| format!("failed to read test script {}: {error}", path.display()))
}

fn finish_worker(bridge: &RunnerBridge, outcome: ScriptOutcome) {
    let _ = bridge.send_unchecked(UiCommand::Cancel(Instant::now()));
    let _ = bridge.send_unchecked(UiCommand::Finished(outcome));
}

fn spawn_script_path(path: PathBuf, bridge: RunnerBridge) -> Result<(), String> {
    std::thread::Builder::new()
        .name("test-script-runner".to_string())
        .spawn(move || {
            let outcome = match load_script(&path) {
                Ok(source) => evaluate_source(&source, bridge.clone()),
                Err(error) => ScriptOutcome::script_failure(error),
            };
            finish_worker(&bridge, outcome);
        })
        .map(|_| ())
        .map_err(|error| format!("failed to spawn test-script runner: {error}"))
}

#[cfg(test)]
fn spawn_script_source(source: String, bridge: RunnerBridge) -> Result<(), String> {
    std::thread::Builder::new()
        .name("test-script-runner-test".to_string())
        .spawn(move || {
            let outcome = evaluate_source(&source, bridge.clone());
            finish_worker(&bridge, outcome);
        })
        .map(|_| ())
        .map_err(|error| format!("failed to spawn test-script runner: {error}"))
}

struct PendingAction {
    action: KeyAction,
    dispatch: PendingActionDispatch,
    // `None` means a non-consuming pressed_action peek already acknowledged
    // the command. Keep the entry until the frame ends so later peeks observe
    // the same press, just like an egui input event.
    applied: Option<mpsc::SyncSender<Result<(), String>>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum PendingActionDispatch {
    LegacyImplicit,
    Targeted {
        owner: TestScriptWindowIdentity,
        phase: TargetedActionPhase,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TargetedActionPhase {
    AwaitingDetachedOwner,
    AwaitingPass,
}

#[derive(Clone, Debug)]
struct TestScriptActionPassObservation {
    pass: u64,
    owner: Option<TestScriptWindowIdentity>,
    eligible: bool,
}

fn joined_window_snapshots(
    authoritative: &[TestScriptWindowSnapshot],
    viewport_observations: &HashMap<TestScriptWindowIdentity, u64>,
    paint_observations: &HashMap<TestScriptPaintEvidenceKey, u64>,
) -> Vec<TestScriptWindowSnapshot> {
    authoritative
        .iter()
        .cloned()
        .map(|mut window| {
            if let Some(revision) = window
                .identity
                .as_ref()
                .and_then(|identity| viewport_observations.get(identity))
            {
                window.viewport_rendered = true;
                window.viewport_revision = *revision;
            }
            let best = paint_observations
                .iter()
                .filter(|(key, _)| {
                    window.accepts_owner(&key.owner) && window.current_content_matches(&key.content)
                })
                .max_by_key(|(key, revision)| (key.content.source_kind.preference(), **revision));
            if let Some((key, revision)) = best {
                window.paint_matches_current_page = true;
                window.full_texture_painted =
                    key.content.source_kind == TestScriptPaintSourceKind::FullOrProcessed;
                window.paint_source = key.content.source_kind.as_str().to_string();
                window.paint_source_texture = format!("{:?}", key.content.source_texture_id);
                window.painted_page_index = Some(key.content.page_index);
                window.paint_revision = *revision;
            }
            window
        })
        .collect()
}

struct FinishState {
    outcome: ScriptOutcome,
    started_frame: u64,
    newer_frames: u8,
}

struct UiRuntime {
    rx: mpsc::Receiver<UiCommand>,
    snapshot: Arc<RwLock<TestScriptSnapshot>>,
    interrupt: Arc<InterruptState>,
    pending_actions: VecDeque<PendingAction>,
    last_frame: Option<u64>,
    finish: Option<FinishState>,
    cancel_requested: bool,
    authoritative_windows: Vec<TestScriptWindowSnapshot>,
    viewport_observations: HashMap<TestScriptWindowIdentity, u64>,
    paint_observations: HashMap<TestScriptPaintEvidenceKey, u64>,
    next_observation_revision: u64,
}

impl UiRuntime {
    fn new(
        rx: mpsc::Receiver<UiCommand>,
        snapshot: Arc<RwLock<TestScriptSnapshot>>,
        interrupt: Arc<InterruptState>,
    ) -> Self {
        Self {
            rx,
            snapshot,
            interrupt,
            pending_actions: VecDeque::new(),
            last_frame: None,
            finish: None,
            cancel_requested: false,
            authoritative_windows: Vec::new(),
            viewport_observations: HashMap::new(),
            paint_observations: HashMap::new(),
            next_observation_revision: 0,
        }
    }

    fn replace_authoritative_windows(&mut self, windows: Vec<TestScriptWindowSnapshot>) {
        self.authoritative_windows = windows;
        let mut retained_actions = VecDeque::with_capacity(self.pending_actions.len());
        while let Some(mut pending) = self.pending_actions.pop_front() {
            let stale_owner = match &pending.dispatch {
                PendingActionDispatch::LegacyImplicit => None,
                PendingActionDispatch::Targeted { owner, .. } => (!self
                    .authoritative_windows
                    .iter()
                    .any(|window| window.identity.as_ref() == Some(owner)))
                .then(|| owner.clone()),
            };
            if let Some(owner) = stale_owner {
                let message = format!(
                    "run_action target is no longer current: {}",
                    owner.describe()
                );
                if let Some(applied) = pending.applied.take() {
                    let _ = applied.send(Err(message.clone()));
                }
                crate::logger::log(format!(
                    "[test-script] action rejected target_mode=targeted owner={} reason=stale",
                    owner.describe()
                ));
            } else {
                retained_actions.push_back(pending);
            }
        }
        self.pending_actions = retained_actions;
        self.viewport_observations.retain(|identity, _| {
            self.authoritative_windows
                .iter()
                .any(|window| window.accepts_owner(identity))
        });
        self.paint_observations.retain(|key, _| {
            self.authoritative_windows.iter().any(|window| {
                window.accepts_owner(&key.owner) && window.current_content_matches(&key.content)
            })
        });
    }

    fn joined_windows(&self) -> Vec<TestScriptWindowSnapshot> {
        joined_window_snapshots(
            &self.authoritative_windows,
            &self.viewport_observations,
            &self.paint_observations,
        )
    }

    fn publish_snapshot(&mut self, mut snapshot: TestScriptSnapshot) -> Result<(), String> {
        self.replace_authoritative_windows(std::mem::take(&mut snapshot.windows));
        snapshot.windows = self.joined_windows();
        self.snapshot
            .write()
            .map(|mut published| *published = snapshot)
            .map_err(|_| "test-script snapshot is poisoned".to_string())
    }

    fn publish_windows(&mut self, windows: Vec<TestScriptWindowSnapshot>) -> Result<(), String> {
        self.replace_authoritative_windows(windows);
        let joined = self.joined_windows();
        self.snapshot
            .write()
            .map(|mut published| published.windows = joined)
            .map_err(|_| "test-script snapshot is poisoned".to_string())
    }

    fn publish_window_frame(
        &mut self,
        owner: TestScriptWindowIdentity,
        content: Option<TestScriptContentProof>,
    ) -> Result<bool, String> {
        let owner_is_current = self
            .authoritative_windows
            .iter()
            .any(|window| window.accepts_owner(&owner));
        if !owner_is_current {
            return Ok(false);
        }
        self.next_observation_revision = self.next_observation_revision.wrapping_add(1).max(1);
        let revision = self.next_observation_revision;
        self.viewport_observations.insert(owner.clone(), revision);
        if let Some(content) = content
            && self.authoritative_windows.iter().any(|window| {
                window.accepts_owner(&owner) && window.current_content_matches(&content)
            })
        {
            let existing_preference = self
                .paint_observations
                .iter()
                .filter(|(key, _)| key.owner == owner)
                .map(|(key, _)| key.content.source_kind.preference())
                .max();
            if existing_preference
                .is_none_or(|preference| content.source_kind.preference() >= preference)
            {
                // One current-content observation per exact owner is enough. A new
                // processed texture must not grow this table forever, while a late
                // thumbnail callback must not replace evidence that a full source was
                // already painted for the same current page.
                self.paint_observations.retain(|key, _| key.owner != owner);
                self.paint_observations
                    .insert(TestScriptPaintEvidenceKey { owner, content }, revision);
            }
        }
        let joined = self.joined_windows();
        self.snapshot
            .write()
            .map(|mut published| published.windows = joined)
            .map_err(|_| "test-script snapshot is poisoned".to_string())?;
        Ok(true)
    }

    fn begin_finish(&mut self, mut outcome: ScriptOutcome, frame: u64) {
        if let Some(existing) = self.finish.as_mut() {
            if outcome.kind == ScriptOutcomeKind::EnvironmentFailure {
                existing
                    .outcome
                    .override_with_environment_failure(outcome.message);
            }
            return;
        }
        if let Some(environment_failure) = self.interrupt.failure_message() {
            outcome = ScriptOutcome::environment_failure(environment_failure);
        }
        self.release_pending_actions("script finished before run_action was consumed");
        self.finish = Some(FinishState {
            outcome,
            started_frame: frame,
            newer_frames: 0,
        });
    }

    fn fail_environment(&mut self, message: String, frame: u64) {
        self.interrupt.fail(message.clone());
        self.release_pending_actions(&message);
        self.begin_finish(ScriptOutcome::environment_failure(message), frame);
    }

    fn request_cancel(&mut self) -> bool {
        if self.cancel_requested {
            return true;
        }
        self.cancel_requested = crate::key_input::cancel_synthetic_input(Instant::now());
        self.cancel_requested
    }

    fn release_pending_actions(&mut self, message: &str) {
        for mut pending in self.pending_actions.drain(..) {
            if let Some(applied) = pending.applied.take() {
                let _ = applied.send(Err(message.to_string()));
            }
        }
    }

    fn queue_action(
        &mut self,
        action: KeyAction,
        selection: TestScriptActionSelection,
        applied: mpsc::SyncSender<Result<(), String>>,
    ) -> Option<egui::ViewportId> {
        match selection {
            TestScriptActionSelection::LegacyImplicit => {
                self.pending_actions.push_back(PendingAction {
                    action,
                    dispatch: PendingActionDispatch::LegacyImplicit,
                    applied: Some(applied),
                });
                crate::logger::log(format!(
                    "[test-script] run_action action={} target_mode=legacy_implicit",
                    action.ini_name()
                ));
                None
            }
            TestScriptActionSelection::Targeted(owner) => {
                let Some(window) = self
                    .authoritative_windows
                    .iter()
                    .find(|window| window.identity.as_ref() == Some(&owner))
                else {
                    let message = format!(
                        "run_action target is no longer current: {}",
                        owner.describe()
                    );
                    let _ = applied.send(Err(message));
                    return None;
                };
                let phase = match (&owner, window.residence.as_str()) {
                    (TestScriptWindowIdentity::Root { .. }, "mounted" | "at_rest") => {
                        TargetedActionPhase::AwaitingPass
                    }
                    (TestScriptWindowIdentity::Detached { .. }, "mounted" | "at_rest") => {
                        TargetedActionPhase::AwaitingDetachedOwner
                    }
                    _ => {
                        let message = format!(
                            "run_action target cannot accept input: {} residence={}",
                            owner.describe(),
                            window.residence
                        );
                        let _ = applied.send(Err(message));
                        return None;
                    }
                };
                let focus =
                    (phase == TargetedActionPhase::AwaitingPass).then(|| owner.viewport_id());
                crate::logger::log(format!(
                    "[test-script] run_action action={} target_mode=targeted owner={} phase={phase:?}",
                    action.ini_name(),
                    owner.describe()
                ));
                self.pending_actions.push_back(PendingAction {
                    action,
                    dispatch: PendingActionDispatch::Targeted { owner, phase },
                    applied: Some(applied),
                });
                focus
            }
        }
    }

    fn pending_targeted_detached_owner(&self) -> Option<TestScriptWindowIdentity> {
        self.pending_actions
            .iter()
            .find_map(|pending| match &pending.dispatch {
                PendingActionDispatch::Targeted {
                    owner,
                    phase: TargetedActionPhase::AwaitingDetachedOwner,
                } => Some(owner.clone()),
                PendingActionDispatch::LegacyImplicit
                | PendingActionDispatch::Targeted {
                    phase: TargetedActionPhase::AwaitingPass,
                    ..
                } => None,
            })
    }

    fn finish_targeted_detached_owner(
        &mut self,
        owner: &TestScriptWindowIdentity,
        result: Result<(), String>,
    ) {
        let Some(index) = self.pending_actions.iter().position(|pending| {
            matches!(
                &pending.dispatch,
                PendingActionDispatch::Targeted {
                    owner: pending_owner,
                    phase: TargetedActionPhase::AwaitingDetachedOwner,
                } if pending_owner == owner
            )
        }) else {
            return;
        };
        match result {
            Ok(()) => {
                if let PendingActionDispatch::Targeted { phase, .. } =
                    &mut self.pending_actions[index].dispatch
                {
                    *phase = TargetedActionPhase::AwaitingPass;
                }
                crate::logger::log(format!(
                    "[test-script] action target ready owner={}",
                    owner.describe()
                ));
            }
            Err(message) => {
                let mut pending = self.pending_actions.remove(index).expect("index exists");
                if let Some(applied) = pending.applied.take() {
                    let _ = applied.send(Err(message.clone()));
                }
                crate::logger::log(format!(
                    "[test-script] action target resolution failed owner={} error={message}",
                    owner.describe()
                ));
            }
        }
    }

    fn expire_unconsumed_legacy_actions(&mut self, frame: u64) {
        if self.pending_actions.is_empty() {
            return;
        }
        let mut retained = VecDeque::with_capacity(self.pending_actions.len());
        let mut unconsumed = Vec::new();
        while let Some(pending) = self.pending_actions.pop_front() {
            match pending.dispatch {
                PendingActionDispatch::LegacyImplicit => {
                    if pending.applied.is_some() {
                        unconsumed.push(pending);
                    }
                }
                PendingActionDispatch::Targeted { .. } => retained.push_back(pending),
            }
        }
        self.pending_actions = retained;
        if unconsumed.is_empty() {
            return;
        }
        let names = unconsumed
            .iter()
            .map(|pending| pending.action.ini_name())
            .collect::<Vec<_>>()
            .join(", ");
        let message = format!("run_action was not consumed in its UI frame: {names}");
        for mut pending in unconsumed {
            if let Some(applied) = pending.applied.take() {
                let _ = applied.send(Err(message.clone()));
            }
        }
        self.fail_environment(message, frame);
    }

    fn finish_target_pass(&mut self, owner: &TestScriptWindowIdentity, eligible: bool, frame: u64) {
        let mut retained = VecDeque::with_capacity(self.pending_actions.len());
        let mut unconsumed = Vec::new();
        while let Some(pending) = self.pending_actions.pop_front() {
            let belongs_to_pass = matches!(
                &pending.dispatch,
                PendingActionDispatch::Targeted {
                    owner: pending_owner,
                    phase: TargetedActionPhase::AwaitingPass,
                } if pending_owner == owner
            );
            if belongs_to_pass && (eligible || pending.applied.is_none()) {
                if eligible && pending.applied.is_some() {
                    unconsumed.push(pending);
                }
            } else {
                retained.push_back(pending);
            }
        }
        self.pending_actions = retained;
        if unconsumed.is_empty() {
            return;
        }
        let names = unconsumed
            .iter()
            .map(|pending| pending.action.ini_name())
            .collect::<Vec<_>>()
            .join(", ");
        let message = format!(
            "run_action was not consumed in its target UI pass: owner={} actions={names}",
            owner.describe()
        );
        for mut pending in unconsumed {
            if let Some(applied) = pending.applied.take() {
                let _ = applied.send(Err(message.clone()));
            }
        }
        self.fail_environment(message, frame);
    }
}

fn runtime() -> &'static Mutex<Option<UiRuntime>> {
    static RUNTIME: OnceLock<Mutex<Option<UiRuntime>>> = OnceLock::new();
    RUNTIME.get_or_init(|| Mutex::new(None))
}

static PROCESS_EXIT_CODE: AtomicI32 = AtomicI32::new(EXIT_NOT_SET);
static SHUTDOWN_WATCHDOG_ARMED: AtomicBool = AtomicBool::new(false);

fn frame_key(ctx: &egui::Context) -> u64 {
    ctx.input(|input| input.time.to_bits())
}

fn describe_issue(issue: &SyntheticInputIssue) -> String {
    match issue {
        SyntheticInputIssue::WaitingForRouting(error) => {
            format!("synthetic routing target unavailable: {error:?}")
        }
        SyntheticInputIssue::WaitingForFocus(target) => format!(
            "synthetic routing target is not focused: viewport={:?} hwnd=0x{:x}",
            target.viewport, target.hwnd
        ),
        SyntheticInputIssue::FocusLost { viewport } => {
            format!("synthetic key hold lost focus: viewport={viewport:?}")
        }
        SyntheticInputIssue::TargetViewportNotRendered {
            viewport,
            raw_input_time,
            event_count,
        } => format!(
            "synthetic target viewport was not rendered in its outer frame: viewport={viewport:?} raw_input_time={raw_input_time:?} event_count={event_count}"
        ),
    }
}

pub(crate) fn start(path: PathBuf, ctx: &egui::Context) -> Result<(), String> {
    PROCESS_EXIT_CODE.store(EXIT_NOT_SET, Ordering::Release);
    let result = start_inner(path, ctx);
    if let Err(error) = &result {
        let outcome = ScriptOutcome::environment_failure(format!("runner start failed: {error}"));
        PROCESS_EXIT_CODE.store(EXIT_ENVIRONMENT_FAILURE, Ordering::Release);
        crate::logger::log(format!(
            "[test-script] finished kind=EnvironmentFailure exit_code={} message=runner start failed: {error}",
            EXIT_ENVIRONMENT_FAILURE
        ));
        emit_perf_fail(&outcome);
        let _ = crate::key_input::cancel_synthetic_input(Instant::now());
        crate::key_input::disarm_synthetic_input();
        // eframe has already created its wgpu Device before invoking the app
        // creator, so even creator failure needs the teardown watchdog.
        arm_shutdown_watchdog(EXIT_ENVIRONMENT_FAILURE, "runner-start-failure");
    }
    result
}

fn start_inner(path: PathBuf, ctx: &egui::Context) -> Result<(), String> {
    if !crate::key_input::arm_synthetic_input() {
        return Err("failed to arm synthetic input timeline".to_string());
    }
    // Synthetic routing intentionally obeys the production foreground/focus
    // rules. Request focus here so unattended runs can satisfy that precondition
    // without a host-side click or key injection.
    ctx.send_viewport_cmd_to(egui::ViewportId::ROOT, egui::ViewportCommand::Focus);

    let (tx, rx) = mpsc::channel();
    let snapshot = Arc::new(RwLock::new(TestScriptSnapshot::default()));
    let interrupt = Arc::new(InterruptState::default());
    let wake_ctx = ctx.clone();
    let bridge = RunnerBridge {
        tx,
        snapshot: Arc::clone(&snapshot),
        interrupt: Arc::clone(&interrupt),
        wake: Arc::new(move || {
            wake_ctx.request_repaint_of(egui::ViewportId::ROOT);
        }),
        next_hold_id: Arc::new(AtomicU64::new(0)),
        action_selection: Arc::new(Mutex::new(TestScriptActionSelection::LegacyImplicit)),
    };

    let mut guard = runtime()
        .lock()
        .map_err(|_| "test-script runtime is poisoned".to_string())?;
    if guard.is_some() {
        crate::key_input::disarm_synthetic_input();
        return Err("a test-script runtime is already active".to_string());
    }
    *guard = Some(UiRuntime::new(rx, snapshot, interrupt));
    drop(guard);

    if let Err(error) = spawn_script_path(path, bridge) {
        if let Ok(mut guard) = runtime().lock() {
            *guard = None;
        }
        crate::key_input::disarm_synthetic_input();
        return Err(error);
    }
    ctx.request_repaint_of(egui::ViewportId::ROOT);
    Ok(())
}

fn action_matches_owner(
    dispatch: &PendingActionDispatch,
    owner: Option<&TestScriptWindowIdentity>,
) -> bool {
    match dispatch {
        PendingActionDispatch::LegacyImplicit => true,
        PendingActionDispatch::Targeted {
            owner: target,
            phase: TargetedActionPhase::AwaitingPass,
        } => owner == Some(target),
        PendingActionDispatch::Targeted {
            phase: TargetedActionPhase::AwaitingDetachedOwner,
            ..
        } => false,
    }
}

fn consume_pending_action_from(
    pending_actions: &mut VecDeque<PendingAction>,
    owner: Option<&TestScriptWindowIdentity>,
    action: KeyAction,
) -> bool {
    let Some(index) = pending_actions.iter().position(|pending| {
        pending.action == action && action_matches_owner(&pending.dispatch, owner)
    }) else {
        return false;
    };
    let mut pending = pending_actions.remove(index).expect("index exists");
    if let Some(applied) = pending.applied.take() {
        let _ = applied.send(Ok(()));
    }
    true
}

fn peek_pending_action_from(
    pending_actions: &mut VecDeque<PendingAction>,
    owner: Option<&TestScriptWindowIdentity>,
    action: KeyAction,
) -> bool {
    let Some(pending) = pending_actions
        .iter_mut()
        .find(|pending| pending.action == action && action_matches_owner(&pending.dispatch, owner))
    else {
        return false;
    };
    if let Some(applied) = pending.applied.take() {
        let _ = applied.send(Ok(()));
    }
    true
}

fn action_pass_observation_id(viewport_id: egui::ViewportId) -> egui::Id {
    egui::Id::new("miv.test_script.action_pass_observation").with(viewport_id)
}

fn action_pass_observation(ctx: &egui::Context) -> Option<TestScriptActionPassObservation> {
    let pass = ctx.cumulative_pass_nr();
    let observation_id = action_pass_observation_id(ctx.viewport_id());
    ctx.data(|data| {
        data.get_temp::<TestScriptActionPassObservation>(observation_id)
            .filter(|observation| observation.pass == pass)
    })
}

fn action_pass_observation_for_active_backend(
    ctx: &egui::Context,
) -> Option<TestScriptActionPassObservation> {
    let observation = action_pass_observation(ctx)?;
    if observation.owner.as_ref().is_none_or(|owner| {
        eframe::miv_test_script_window_witness::active()
            .is_some_and(|witness| owner.matches_backend_witness(witness))
    }) {
        Some(observation)
    } else {
        None
    }
}

pub(crate) fn publish_action_pass_owner(
    ctx: &egui::Context,
    owner: Option<TestScriptWindowIdentity>,
) {
    let owner = owner.filter(|owner| {
        eframe::miv_test_script_window_witness::active()
            .is_some_and(|witness| owner.matches_backend_witness(witness))
    });
    let pass = ctx.cumulative_pass_nr();
    let observation_id = action_pass_observation_id(ctx.viewport_id());
    ctx.data_mut(|data| {
        let eligible = data
            .get_temp::<TestScriptActionPassObservation>(observation_id)
            .is_some_and(|observation| {
                observation.pass == pass && observation.owner == owner && observation.eligible
            });
        data.insert_temp(
            observation_id,
            TestScriptActionPassObservation {
                pass,
                owner,
                eligible,
            },
        );
    });
}

pub(crate) fn mark_action_pass_eligible(ctx: &egui::Context) {
    let Some(mut observation) = action_pass_observation_for_active_backend(ctx) else {
        return;
    };
    observation.eligible = true;
    let observation_id = action_pass_observation_id(ctx.viewport_id());
    ctx.data_mut(|data| data.insert_temp(observation_id, observation));
}

pub(crate) fn finish_action_pass(ctx: &egui::Context) {
    let Some(observation) = action_pass_observation_for_active_backend(ctx) else {
        return;
    };
    let Some(owner) = observation.owner else {
        return;
    };
    let Ok(mut guard) = runtime().lock() else {
        return;
    };
    let Some(runtime) = guard.as_mut() else {
        return;
    };
    runtime.finish_target_pass(&owner, observation.eligible, frame_key(ctx));
}

pub(crate) fn consume_pending_action(ctx: &egui::Context, action: KeyAction) -> bool {
    let owner =
        action_pass_observation_for_active_backend(ctx).and_then(|observation| observation.owner);
    let Ok(mut guard) = runtime().lock() else {
        return false;
    };
    let Some(runtime) = guard.as_mut() else {
        return false;
    };
    consume_pending_action_from(&mut runtime.pending_actions, owner.as_ref(), action)
}

pub(crate) fn peek_pending_action(ctx: &egui::Context, action: KeyAction) -> bool {
    let owner =
        action_pass_observation_for_active_backend(ctx).and_then(|observation| observation.owner);
    let Ok(mut guard) = runtime().lock() else {
        return false;
    };
    let Some(runtime) = guard.as_mut() else {
        return false;
    };
    peek_pending_action_from(&mut runtime.pending_actions, owner.as_ref(), action)
}

pub(crate) fn pending_targeted_detached_owner() -> Option<TestScriptWindowIdentity> {
    runtime()
        .lock()
        .ok()
        .and_then(|guard| guard.as_ref()?.pending_targeted_detached_owner())
}

pub(crate) fn finish_targeted_detached_owner(
    owner: &TestScriptWindowIdentity,
    result: Result<(), String>,
) {
    let Ok(mut guard) = runtime().lock() else {
        return;
    };
    let Some(runtime) = guard.as_mut() else {
        return;
    };
    runtime.finish_targeted_detached_owner(owner, result);
}

fn flush_exit_logs() {
    crate::perf::flush();
    crate::logger::flush();
}

fn arm_shutdown_watchdog(exit_code: i32, trigger: &'static str) {
    if SHUTDOWN_WATCHDOG_ARMED.swap(true, Ordering::AcqRel) {
        return;
    }
    crate::logger::log(format!(
        "[test-script] shutdown path=watchdog-armed trigger={trigger} grace_ms={}",
        SHUTDOWN_WATCHDOG_GRACE.as_millis()
    ));
    // Persist every event produced before Close even if third-party GPU
    // teardown wedges before the watchdog reaches its final flush.
    flush_exit_logs();

    let spawned = std::thread::Builder::new()
        .name("test-script-exit-watchdog".to_string())
        .spawn(move || {
            std::thread::sleep(SHUTDOWN_WATCHDOG_GRACE);
            // This is deliberately process-level containment, not a repair for
            // wgpu. Once eframe owns Device teardown there is no application
            // API with which to complete or cancel its stuck GPU submission.
            // The opt-in test harness must nevertheless expose the already
            // determined script result as the real process exit code.
            crate::logger::log(format!(
                "[test-script] shutdown path=forced-watchdog exit_code={exit_code}"
            ));
            flush_exit_logs();
            std::process::exit(exit_code);
        });
    if let Err(error) = spawned {
        crate::logger::log(format!(
            "[test-script] shutdown path=watchdog-spawn-failed exit_code={} error={error}",
            EXIT_ENVIRONMENT_FAILURE
        ));
        flush_exit_logs();
        std::process::exit(EXIT_ENVIRONMENT_FAILURE);
    }
}

pub(crate) fn ui_update(ctx: &egui::Context, snapshot: TestScriptSnapshot) -> bool {
    let frame = frame_key(ctx);
    let issues = crate::key_input::take_synthetic_input_issues(ctx);
    let Ok(mut guard) = runtime().lock() else {
        let outcome =
            ScriptOutcome::environment_failure("test-script runtime is poisoned".to_string());
        PROCESS_EXIT_CODE.store(EXIT_ENVIRONMENT_FAILURE, Ordering::Release);
        crate::logger::log(format!(
            "[test-script] finished kind=EnvironmentFailure exit_code={} message=test-script runtime is poisoned",
            EXIT_ENVIRONMENT_FAILURE
        ));
        emit_perf_fail(&outcome);
        let _ = crate::key_input::cancel_synthetic_input(Instant::now());
        crate::key_input::disarm_synthetic_input();
        arm_shutdown_watchdog(EXIT_ENVIRONMENT_FAILURE, "runtime-poisoned");
        return true;
    };
    let Some(runtime) = guard.as_mut() else {
        return false;
    };

    let new_frame = runtime.last_frame != Some(frame);
    if new_frame {
        if runtime.last_frame.is_some() {
            runtime.expire_unconsumed_legacy_actions(frame);
        }
        runtime.last_frame = Some(frame);
    }
    emit_perf_level_reads(&snapshot.keymap_level_observations);
    if let Err(error) = runtime.publish_snapshot(snapshot) {
        runtime.fail_environment(error, frame);
    }

    for issue in issues {
        runtime.fail_environment(describe_issue(&issue), frame);
    }

    while let Ok(command) = runtime.rx.try_recv() {
        match command {
            UiCommand::Key(command) => {
                if runtime.finish.is_none() && !crate::key_input::enqueue_synthetic_command(command)
                {
                    runtime.fail_environment(
                        "failed to enqueue synthetic key command".to_string(),
                        frame,
                    );
                }
            }
            UiCommand::Cancel(at) => {
                runtime.release_pending_actions("run_action was cancelled before consumption");
                if crate::key_input::cancel_synthetic_input(at) {
                    runtime.cancel_requested = true;
                } else {
                    runtime.fail_environment(
                        "failed to enqueue synthetic CancelAll".to_string(),
                        frame,
                    );
                }
            }
            UiCommand::SetRepeat { delay, hz } => {
                if runtime.finish.is_none() && !crate::key_input::set_synthetic_repeat(delay, hz) {
                    runtime.fail_environment(
                        "failed to apply synthetic repeat settings".to_string(),
                        frame,
                    );
                }
            }
            UiCommand::RunAction {
                action,
                selection,
                applied,
            } => {
                if runtime.finish.is_some() {
                    let _ = applied.send(Err("script is already finishing".to_string()));
                } else {
                    if let Some(viewport_id) = runtime.queue_action(action, selection, applied) {
                        ctx.send_viewport_cmd_to(viewport_id, egui::ViewportCommand::Focus);
                        ctx.request_repaint_of(viewport_id);
                    }
                }
            }
            UiCommand::Log(message) => {
                crate::logger::log(format!("[test-script] {message}"));
                emit_perf_step(&message);
            }
            UiCommand::Precondition(trace) => emit_perf_precondition(&trace),
            UiCommand::Finished(outcome) => runtime.begin_finish(outcome, frame),
        }
    }

    if runtime.finish.is_some() && !runtime.request_cancel() {
        runtime.fail_environment(
            "failed to request terminal synthetic cancellation".to_string(),
            frame,
        );
    }

    let mut close = false;
    if let Some(finish) = runtime.finish.as_mut() {
        if new_frame && frame != finish.started_frame {
            finish.newer_frames = finish.newer_frames.saturating_add(1);
        }
        if finish.newer_frames >= 2 && crate::key_input::synthetic_input_is_idle() {
            let outcome = finish.outcome.clone();
            PROCESS_EXIT_CODE.store(outcome.kind.exit_code(), Ordering::Release);
            crate::logger::log(format!(
                "[test-script] finished kind={:?} exit_code={} message={}",
                outcome.kind,
                outcome.kind.exit_code(),
                outcome.message
            ));
            emit_perf_fail(&outcome);
            crate::key_input::disarm_synthetic_input();
            arm_shutdown_watchdog(outcome.kind.exit_code(), "close-request");
            close = true;
        } else {
            ctx.request_repaint_of(egui::ViewportId::ROOT);
        }
    } else if !crate::key_input::synthetic_input_is_idle() {
        // A held key must keep advancing the deterministic repeat timeline even
        // while the application would otherwise sleep.
        ctx.request_repaint_of(egui::ViewportId::ROOT);
    }

    if close {
        *guard = None;
    }
    close
}

/// Replace the read-only detached-window table after a lifecycle phase that can
/// establish or retire an HWND claim. This never drives the lifecycle itself.
pub(crate) fn publish_window_snapshots(windows: Vec<TestScriptWindowSnapshot>) {
    let Ok(mut guard) = runtime().lock() else {
        return;
    };
    let Some(runtime) = guard.as_mut() else {
        return;
    };
    let _ = runtime.publish_windows(windows);
}

/// Record one viewport callback and, when supplied, the exact texture command
/// queued by that callback. The current table is checked before either record is
/// exposed, so a callback from a retired host cannot displace current evidence.
pub(crate) fn publish_window_frame(
    owner: TestScriptWindowIdentity,
    content: Option<TestScriptContentProof>,
) {
    let Ok(mut guard) = runtime().lock() else {
        return;
    };
    let Some(runtime) = guard.as_mut() else {
        return;
    };
    let _ = runtime.publish_window_frame(owner, content);
}

pub(crate) fn publish_fullscreen_input_state(
    ctx: &egui::Context,
    fs_idx: usize,
    focused: bool,
    target_rendered: bool,
    raw_key_permit: bool,
    ime_active: bool,
    text_input_or_pending_focus: bool,
    popup_open: bool,
) {
    let Ok(guard) = runtime().lock() else {
        return;
    };
    let Some(runtime) = guard.as_ref() else {
        return;
    };
    let Ok(mut snapshot) = runtime.snapshot.write() else {
        return;
    };
    if snapshot.fs_idx != fs_idx as i64 {
        return;
    }
    let viewport_label = if ctx.viewport_id() == egui::ViewportId::ROOT {
        "ROOT".to_string()
    } else {
        format!("{:?}", ctx.viewport_id())
    };
    if snapshot.target_viewport != viewport_label {
        return;
    }
    snapshot.focused = focused;
    snapshot.target_rendered = target_rendered;
    snapshot.fullscreen_raw_key_permit = raw_key_permit;
    snapshot.ime_active = ime_active;
    snapshot.text_input_or_pending_focus = text_input_or_pending_focus;
    snapshot.popup_open = popup_open;
}

pub(crate) fn on_app_exit() {
    let active = runtime().lock().ok().and_then(|mut guard| guard.take());
    let Some(mut runtime) = active else {
        return;
    };
    PROCESS_EXIT_CODE.store(EXIT_ENVIRONMENT_FAILURE, Ordering::Release);
    let outcome = ScriptOutcome::environment_failure(
        "application exited before the test script completed".to_string(),
    );
    runtime
        .interrupt
        .fail("application exited before the test script completed");
    runtime.release_pending_actions("application exited before run_action was consumed");
    let _ = crate::key_input::cancel_synthetic_input(Instant::now());
    crate::key_input::disarm_synthetic_input();
    crate::logger::log(format!(
        "[test-script] finished kind=EnvironmentFailure exit_code={} message=application exited before the test script completed",
        EXIT_ENVIRONMENT_FAILURE
    ));
    emit_perf_fail(&outcome);
    arm_shutdown_watchdog(EXIT_ENVIRONMENT_FAILURE, "premature-app-exit");
}

pub(crate) fn process_exit_code() -> Option<i32> {
    let code = PROCESS_EXIT_CODE.load(Ordering::Acquire);
    (code != EXIT_NOT_SET).then_some(code)
}

pub(crate) fn exit_after_run_native() -> ! {
    let exit_code = process_exit_code().unwrap_or(EXIT_ENVIRONMENT_FAILURE);
    crate::logger::log(format!(
        "[test-script] shutdown path=run-native-return exit_code={exit_code}"
    ));
    flush_exit_logs();
    std::process::exit(exit_code);
}

pub(crate) fn cli_script_path_from(args: &[std::ffi::OsString]) -> Result<Option<PathBuf>, String> {
    let mut script_path = None;
    let mut has_data_dir = false;
    let mut index = 1usize;
    while index < args.len() {
        let arg = &args[index];
        if arg == "--" {
            break;
        }
        if arg == "--test-script" || arg == "--data-dir" {
            let flag = arg.to_string_lossy();
            let Some(value) = args.get(index + 1) else {
                return Err(format!("{flag} requires a path value"));
            };
            if value.to_str().is_some_and(|value| value.starts_with("--")) {
                return Err(format!("{flag} requires a path value"));
            }
            if value.is_empty() {
                return Err(format!("{flag} requires a non-empty path value"));
            }
            if arg == "--test-script" {
                if script_path.is_some() {
                    return Err("--test-script may only be specified once".to_string());
                }
                script_path = Some(PathBuf::from(value));
            } else {
                has_data_dir = true;
            }
            index += 2;
            continue;
        }
        index += 1;
    }

    if script_path.is_some() && !has_data_dir {
        return Err(
            "--test-script requires an explicit --data-dir; refusing to use the normal application profile"
                .to_string(),
        );
    }
    Ok(script_path)
}

#[cfg(test)]
mod tests {
    use super::InterruptState;
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    /// A hold must last as long as it was asked to, even when something wakes the condvar.
    ///
    /// Without the loop this returned on the first notify and a twenty-second burst finished in
    /// 45ms, so the scenario never ran and the script reported success anyway.
    #[test]
    fn a_wait_is_not_ended_by_a_wakeup_that_is_not_a_failure() {
        let state = Arc::new(InterruptState::default());
        let waker = Arc::clone(&state);
        std::thread::spawn(move || {
            for _ in 0..10 {
                std::thread::sleep(Duration::from_millis(5));
                waker.changed.notify_all();
            }
        });
        let started = Instant::now();
        state
            .wait(Duration::from_millis(300))
            .expect("no failure was set");
        let elapsed = started.elapsed();
        assert!(
            elapsed >= Duration::from_millis(290),
            "the wait ended after {elapsed:?} instead of running its full 300ms"
        );
    }

    #[test]
    fn a_wait_ends_as_soon_as_the_run_fails() {
        let state = Arc::new(InterruptState::default());
        let failer = Arc::clone(&state);
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(30));
            failer.fail("the window lost focus");
        });
        let started = Instant::now();
        let error = state
            .wait(Duration::from_secs(30))
            .expect_err("a failure must interrupt the wait");
        assert_eq!(error, "the window lost focus");
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "a failure must not wait out the full duration"
        );
    }

    use super::*;
    use std::ffi::OsString;
    use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};

    fn args(values: &[&str]) -> Vec<OsString> {
        values.iter().map(OsString::from).collect()
    }

    fn runner_bridge(
        snapshot: TestScriptSnapshot,
    ) -> (RunnerBridge, mpsc::Receiver<UiCommand>, Arc<AtomicUsize>) {
        let (tx, rx) = mpsc::channel();
        let wakes = Arc::new(AtomicUsize::new(0));
        let wake_count = Arc::clone(&wakes);
        (
            RunnerBridge {
                tx,
                snapshot: Arc::new(RwLock::new(snapshot)),
                interrupt: Arc::new(InterruptState::default()),
                wake: Arc::new(move || {
                    wake_count.fetch_add(1, AtomicOrdering::Relaxed);
                }),
                next_hold_id: Arc::new(AtomicU64::new(0)),
                action_selection: Arc::new(Mutex::new(TestScriptActionSelection::LegacyImplicit)),
            },
            rx,
            wakes,
        )
    }

    fn ready_snapshot() -> TestScriptSnapshot {
        TestScriptSnapshot {
            focused: true,
            target_registered: true,
            target_viewport: "ROOT".to_string(),
            ..TestScriptSnapshot::default()
        }
    }

    fn receive_through_finished(rx: &mpsc::Receiver<UiCommand>) -> Vec<UiCommand> {
        let mut commands = Vec::new();
        loop {
            let command = rx
                .recv_timeout(Duration::from_secs(2))
                .expect("runner command");
            let finished = matches!(command, UiCommand::Finished(_));
            commands.push(command);
            if finished {
                return commands;
            }
        }
    }

    #[test]
    fn cli_requires_isolated_data_dir() {
        let parsed = cli_script_path_from(&args(&[
            "mimageviewer-core.exe",
            "--test-script",
            "smoke.rhai",
        ]));
        assert!(parsed.unwrap_err().contains("--data-dir"));
    }

    #[test]
    fn cli_returns_script_and_does_not_treat_its_value_as_a_path_argument() {
        let parsed = cli_script_path_from(&args(&[
            "mimageviewer-core.exe",
            "--data-dir",
            "sandbox",
            "--test-script",
            "smoke.rhai",
        ]))
        .unwrap();
        assert_eq!(parsed, Some(PathBuf::from("smoke.rhai")));
    }

    #[test]
    fn script_thread_translates_calls_to_typed_commands_and_wakes() {
        let (bridge, rx, wakes) = runner_bridge(ready_snapshot());
        spawn_script_source(
            r#"
                tap_key("Right");
                hold_key("Left", 1);
                release_key("Home");
                set_repeat(125, 20);
                log("hello");
            "#
            .to_string(),
            bridge,
        )
        .unwrap();
        let commands = receive_through_finished(&rx);
        let key_kinds = commands
            .iter()
            .filter_map(|command| match command {
                UiCommand::Key(command) => Some(command.kind),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(
            key_kinds,
            vec![
                SyntheticKeyCommandKind::Down {
                    key: SyntheticNavigationKey::Right,
                    modifiers: SyntheticModifiers::default(),
                },
                SyntheticKeyCommandKind::Up {
                    key: SyntheticNavigationKey::Right,
                },
                SyntheticKeyCommandKind::Down {
                    key: SyntheticNavigationKey::Left,
                    modifiers: SyntheticModifiers::default(),
                },
                SyntheticKeyCommandKind::Up {
                    key: SyntheticNavigationKey::Left,
                },
                SyntheticKeyCommandKind::Up {
                    key: SyntheticNavigationKey::Home,
                },
            ]
        );
        assert!(commands.iter().any(|command| matches!(
            command,
            UiCommand::SetRepeat { delay, hz }
                if *delay == Duration::from_millis(125) && (*hz - 20.0).abs() < f64::EPSILON
        )));
        assert!(
            commands
                .iter()
                .any(|command| matches!(command, UiCommand::Log(message) if message == "hello"))
        );
        assert!(
            commands
                .iter()
                .any(|command| matches!(command, UiCommand::Cancel(_)))
        );
        assert!(matches!(
            commands.last(),
            Some(UiCommand::Finished(ScriptOutcome {
                kind: ScriptOutcomeKind::Success,
                ..
            }))
        ));
        assert_eq!(wakes.load(AtomicOrdering::Relaxed), commands.len());
    }

    #[test]
    fn invalid_key_is_a_script_failure_instead_of_a_noop() {
        let (bridge, rx, _) = runner_bridge(ready_snapshot());
        spawn_script_source(r#"tap_key("A");"#.to_string(), bridge).unwrap();
        let commands = receive_through_finished(&rx);
        assert!(
            !commands
                .iter()
                .any(|command| matches!(command, UiCommand::Key(_)))
        );
        assert!(matches!(
            commands.last(),
            Some(UiCommand::Finished(ScriptOutcome {
                kind: ScriptOutcomeKind::ScriptFailure,
                message,
            })) if message.contains("unsupported synthetic navigation key")
        ));
    }

    #[test]
    fn hold_key_assigns_one_monotonic_id_to_its_down_and_up() {
        let (bridge, rx, _) = runner_bridge(ready_snapshot());
        spawn_script_source(
            r#"
                hold_key("Left", 0);
                hold_key("Right", 0);
            "#
            .to_string(),
            bridge,
        )
        .unwrap();
        let commands = receive_through_finished(&rx);
        let hold_ids = commands
            .iter()
            .filter_map(|command| match command {
                UiCommand::Key(command) => command.hold_id,
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(hold_ids, [1, 1, 2, 2]);
    }

    #[test]
    fn run_action_translates_ini_name_and_waits_for_ui_acknowledgement() {
        let (bridge, rx, wakes) = runner_bridge(ready_snapshot());
        let action = KeyAction::FsClose;
        spawn_script_source(format!(r#"run_action("{}");"#, action.ini_name()), bridge).unwrap();

        match rx.recv_timeout(Duration::from_secs(2)).unwrap() {
            UiCommand::RunAction {
                action: actual,
                selection,
                applied,
            } => {
                assert_eq!(actual, action);
                assert_eq!(selection, TestScriptActionSelection::LegacyImplicit);
                applied.send(Ok(())).unwrap();
            }
            command => panic!("unexpected command before action acknowledgement: {command:?}"),
        }

        let commands = receive_through_finished(&rx);
        assert!(
            commands
                .iter()
                .any(|command| matches!(command, UiCommand::Cancel(_)))
        );
        assert!(matches!(
            commands.last(),
            Some(UiCommand::Finished(ScriptOutcome {
                kind: ScriptOutcomeKind::Success,
                ..
            }))
        ));
        assert_eq!(
            wakes.load(AtomicOrdering::Relaxed),
            commands.len() + 1,
            "RunAction, Cancel, and Finished must each wake ROOT"
        );
    }

    #[test]
    fn selected_root_is_attached_to_run_action_as_an_exact_target() {
        let owner = TestScriptWindowIdentity::Root {
            context_serial: 31,
            hwnd: 0x3131,
            backend_token: 41,
        };
        let mut snapshot = ready_snapshot();
        snapshot.windows = vec![window_snapshot(owner.clone(), 1, 0, "root-item")];
        let (bridge, rx, _) = runner_bridge(snapshot);
        spawn_script_source(
            r#"
                let selected = select_root();
                if selected.target_mode != "targeted" || !selected.current {
                    fail("root target missing");
                }
                run_action("GridMoveFirst");
            "#
            .to_string(),
            bridge,
        )
        .unwrap();

        loop {
            match rx.recv_timeout(Duration::from_secs(2)).unwrap() {
                UiCommand::RunAction {
                    action,
                    selection,
                    applied,
                } => {
                    assert_eq!(action, KeyAction::GridMoveFirst);
                    assert_eq!(selection, TestScriptActionSelection::Targeted(owner));
                    applied.send(Ok(())).unwrap();
                    break;
                }
                UiCommand::Log(message) => assert!(message.contains("mode=targeted")),
                command => panic!("unexpected command before targeted action: {command:?}"),
            }
        }
        assert!(matches!(
            receive_through_finished(&rx).last(),
            Some(UiCommand::Finished(ScriptOutcome {
                kind: ScriptOutcomeKind::Success,
                ..
            }))
        ));
    }

    #[test]
    fn explicit_selection_fails_instead_of_reserving_a_missing_identity() {
        let (bridge, rx, _) = runner_bridge(ready_snapshot());
        spawn_script_source("select_root();".to_string(), bridge).unwrap();
        let commands = receive_through_finished(&rx);

        assert!(
            !commands
                .iter()
                .any(|command| matches!(command, UiCommand::RunAction { .. }))
        );
        assert!(matches!(
            commands.last(),
            Some(UiCommand::Finished(ScriptOutcome {
                kind: ScriptOutcomeKind::ScriptFailure,
                message,
            })) if message.contains("could not find the root window")
        ));
    }

    #[test]
    fn pending_action_peek_is_repeatable_within_the_ui_frame() {
        let action = KeyAction::FsClose;
        let (applied, acknowledgement) = mpsc::sync_channel(1);
        let mut pending = VecDeque::from([PendingAction {
            action,
            dispatch: PendingActionDispatch::LegacyImplicit,
            applied: Some(applied),
        }]);

        assert!(peek_pending_action_from(&mut pending, None, action));
        assert_eq!(
            acknowledgement
                .recv_timeout(Duration::from_secs(1))
                .unwrap(),
            Ok(())
        );
        assert!(peek_pending_action_from(&mut pending, None, action));
        assert!(matches!(
            acknowledgement.try_recv(),
            Err(mpsc::TryRecvError::Disconnected)
        ));
        assert!(consume_pending_action_from(&mut pending, None, action));
        assert!(pending.is_empty());
    }

    #[test]
    fn fail_api_finishes_nonzero() {
        let (bridge, rx, _) = runner_bridge(ready_snapshot());
        spawn_script_source(r#"fail("expected");"#.to_string(), bridge).unwrap();
        let commands = receive_through_finished(&rx);
        assert!(matches!(
            commands.last(),
            Some(UiCommand::Finished(ScriptOutcome {
                kind: ScriptOutcomeKind::ScriptFailure,
                message,
            })) if message.contains("expected")
        ));
    }

    #[test]
    fn wait_until_reads_published_snapshot_across_thread_boundary() {
        let (bridge, rx, _) = runner_bridge(TestScriptSnapshot::default());
        let snapshot = Arc::clone(&bridge.snapshot);
        spawn_script_source(
            "wait_until(|s| s.is_fullscreen && s.focused, 1000);".to_string(),
            bridge,
        )
        .unwrap();
        std::thread::sleep(Duration::from_millis(20));
        {
            let mut published = snapshot.write().unwrap();
            published.is_fullscreen = true;
            published.focused = true;
        }
        let commands = receive_through_finished(&rx);
        assert!(matches!(
            commands.last(),
            Some(UiCommand::Finished(ScriptOutcome {
                kind: ScriptOutcomeKind::Success,
                ..
            }))
        ));
    }

    #[test]
    fn issue_classification_names_environment_failures() {
        let viewport = egui::ViewportId::from_hash_of("missing-child");
        let issue = SyntheticInputIssue::TargetViewportNotRendered {
            viewport,
            raw_input_time: Some(1.5),
            event_count: 3,
        };
        let message = describe_issue(&issue);
        assert!(message.contains("not rendered"));
        assert!(message.contains("event_count=3"));
    }

    fn window_identity(
        window_id: u64,
        context_serial: u64,
        host_incarnation: u64,
    ) -> TestScriptWindowIdentity {
        window_identity_with_backend_token(
            window_id,
            context_serial,
            host_incarnation,
            0x2000 + host_incarnation,
        )
    }

    fn window_identity_with_backend_token(
        window_id: u64,
        context_serial: u64,
        host_incarnation: u64,
        backend_token: u64,
    ) -> TestScriptWindowIdentity {
        TestScriptWindowIdentity::Detached {
            window_id,
            context_serial,
            viewport_id: egui::ViewportId::from_hash_of(("test-window", window_id)),
            host_incarnation,
            hwnd: 0x1000 + host_incarnation,
            backend_token,
        }
    }

    fn root_identity(context_serial: u64, hwnd: u64) -> TestScriptWindowIdentity {
        TestScriptWindowIdentity::Root {
            context_serial,
            hwnd,
            backend_token: 0x3000 + context_serial,
        }
    }

    fn local_runtime() -> UiRuntime {
        let (_tx, rx) = mpsc::channel();
        UiRuntime::new(
            rx,
            Arc::new(RwLock::new(TestScriptSnapshot::default())),
            Arc::new(InterruptState::default()),
        )
    }

    #[test]
    fn targeted_action_waits_for_exact_owner_and_does_not_repeat_after_peek() {
        let owner = window_identity(7, 11, 14);
        let sibling = window_identity(8, 12, 15);
        let action = KeyAction::FsPageNext;
        let mut runtime = local_runtime();
        runtime
            .publish_windows(vec![
                window_snapshot(owner.clone(), 17, 2, "pdf::owner#2"),
                window_snapshot(sibling.clone(), 18, 4, "pdf::sibling#4"),
            ])
            .unwrap();
        let (applied, acknowledgement) = mpsc::sync_channel(1);

        assert_eq!(
            runtime.queue_action(
                action,
                TestScriptActionSelection::Targeted(owner.clone()),
                applied,
            ),
            None,
            "detached owner is resolved by App before a pass is eligible"
        );
        assert_eq!(
            runtime.pending_targeted_detached_owner(),
            Some(owner.clone())
        );
        assert!(!consume_pending_action_from(
            &mut runtime.pending_actions,
            Some(&owner),
            action
        ));
        runtime.finish_targeted_detached_owner(&owner, Ok(()));
        assert!(!peek_pending_action_from(
            &mut runtime.pending_actions,
            Some(&sibling),
            action
        ));
        runtime.finish_target_pass(&sibling, true, 1);
        runtime.finish_target_pass(&owner, false, 1);
        assert!(matches!(
            acknowledgement.try_recv(),
            Err(mpsc::TryRecvError::Empty)
        ));

        assert!(peek_pending_action_from(
            &mut runtime.pending_actions,
            Some(&owner),
            action
        ));
        assert_eq!(
            acknowledgement
                .recv_timeout(Duration::from_secs(1))
                .unwrap(),
            Ok(())
        );
        assert!(peek_pending_action_from(
            &mut runtime.pending_actions,
            Some(&owner),
            action
        ));
        runtime.finish_target_pass(&owner, false, 1);
        assert!(runtime.pending_actions.is_empty());
        assert!(!peek_pending_action_from(
            &mut runtime.pending_actions,
            Some(&owner),
            action
        ));
    }

    #[test]
    fn root_frame_expiry_does_not_expire_a_detached_target_before_its_pass() {
        let owner = window_identity(7, 11, 14);
        let action = KeyAction::FsClose;
        let mut runtime = local_runtime();
        runtime
            .publish_windows(vec![window_snapshot(owner.clone(), 17, 2, "pdf::owner#2")])
            .unwrap();
        let (applied, acknowledgement) = mpsc::sync_channel(1);
        runtime.queue_action(
            action,
            TestScriptActionSelection::Targeted(owner.clone()),
            applied,
        );

        runtime.expire_unconsumed_legacy_actions(2);
        assert_eq!(runtime.pending_actions.len(), 1);
        assert!(matches!(
            acknowledgement.try_recv(),
            Err(mpsc::TryRecvError::Empty)
        ));
        runtime.finish_targeted_detached_owner(&owner, Ok(()));
        runtime.finish_target_pass(&owner, true, 2);
        let error = acknowledgement
            .recv_timeout(Duration::from_secs(1))
            .unwrap()
            .unwrap_err();
        assert!(error.contains("target UI pass"));
    }

    #[test]
    fn stale_target_is_rejected_without_legacy_fallback() {
        let owner = window_identity(7, 11, 14);
        let sibling = window_identity(8, 12, 15);
        let action = KeyAction::FsClose;
        let mut runtime = local_runtime();
        runtime
            .publish_windows(vec![window_snapshot(owner.clone(), 17, 2, "pdf::owner#2")])
            .unwrap();
        let (applied, acknowledgement) = mpsc::sync_channel(1);
        runtime.queue_action(
            action,
            TestScriptActionSelection::Targeted(owner.clone()),
            applied,
        );

        runtime
            .publish_windows(vec![window_snapshot(
                sibling.clone(),
                18,
                4,
                "pdf::sibling#4",
            )])
            .unwrap();
        let error = acknowledgement
            .recv_timeout(Duration::from_secs(1))
            .unwrap()
            .unwrap_err();
        assert!(error.contains("no longer current"));
        assert!(!consume_pending_action_from(
            &mut runtime.pending_actions,
            Some(&sibling),
            action
        ));
    }

    #[test]
    fn backend_replacement_stales_target_without_a_manager_claim_change() {
        let old_owner = window_identity_with_backend_token(7, 11, 14, 101);
        let replacement_owner = window_identity_with_backend_token(7, 11, 14, 102);
        let action = KeyAction::FsClose;
        let mut runtime = local_runtime();
        runtime
            .publish_windows(vec![window_snapshot(
                old_owner.clone(),
                17,
                2,
                "pdf::owner#2",
            )])
            .unwrap();
        let (applied, acknowledgement) = mpsc::sync_channel(1);
        runtime.queue_action(
            action,
            TestScriptActionSelection::Targeted(old_owner),
            applied,
        );

        runtime
            .publish_windows(vec![window_snapshot(
                replacement_owner,
                17,
                2,
                "pdf::owner#2",
            )])
            .unwrap();

        let error = acknowledgement
            .recv_timeout(Duration::from_secs(1))
            .unwrap()
            .unwrap_err();
        assert!(error.contains("no longer current"));
        assert!(runtime.pending_actions.is_empty());
    }

    #[test]
    fn legacy_action_retains_first_matching_consumer_behavior() {
        let action = KeyAction::GridMoveFirst;
        let owner = window_identity(7, 11, 14);
        let mut runtime = local_runtime();
        let (applied, acknowledgement) = mpsc::sync_channel(1);
        runtime.queue_action(action, TestScriptActionSelection::LegacyImplicit, applied);

        assert!(consume_pending_action_from(
            &mut runtime.pending_actions,
            Some(&owner),
            action
        ));
        assert_eq!(
            acknowledgement
                .recv_timeout(Duration::from_secs(1))
                .unwrap(),
            Ok(())
        );
    }

    #[test]
    fn action_pass_observations_are_partitioned_by_viewport_at_the_same_pass() {
        let ctx = egui::Context::default();
        let root = root_identity(1, 0x100);
        let child = window_identity(7, 11, 14);
        let pass = 9;
        let root_observation = TestScriptActionPassObservation {
            pass,
            owner: Some(root.clone()),
            eligible: true,
        };
        let child_observation = TestScriptActionPassObservation {
            pass,
            owner: Some(child.clone()),
            eligible: false,
        };
        let child_viewport = child.viewport_id();
        ctx.data_mut(|data| {
            data.insert_temp(
                action_pass_observation_id(egui::ViewportId::ROOT),
                root_observation,
            );
            data.insert_temp(
                action_pass_observation_id(child_viewport),
                child_observation,
            );
        });

        ctx.data(|data| {
            assert_eq!(
                data.get_temp::<TestScriptActionPassObservation>(action_pass_observation_id(
                    egui::ViewportId::ROOT
                ))
                .and_then(|observation| observation.owner),
                Some(root)
            );
            assert_eq!(
                data.get_temp::<TestScriptActionPassObservation>(action_pass_observation_id(
                    child_viewport
                ))
                .and_then(|observation| observation.owner),
                Some(child)
            );
        });
    }

    #[test]
    fn action_pass_publication_and_eligibility_use_egui_data_without_reentry() {
        let ctx = egui::Context::default();
        let fixture = eframe::miv_test_script_window_witness::WindowWitnessFixture::new();
        let _scope = fixture.enter(&ctx, egui::ViewportId::ROOT, 0x100);
        let witness = eframe::miv_test_script_window_witness::active().unwrap();
        let owner = TestScriptWindowIdentity::Root {
            context_serial: 1,
            hwnd: 0x100,
            backend_token: witness.token(),
        };
        ctx.begin_pass(Default::default());

        publish_action_pass_owner(&ctx, Some(owner.clone()));
        mark_action_pass_eligible(&ctx);
        let observation = action_pass_observation(&ctx).expect("current pass observation");

        assert_eq!(observation.owner, Some(owner));
        assert!(observation.eligible);
        let _ = ctx.end_pass();
    }

    #[test]
    fn replacement_backend_with_the_same_hwnd_cannot_consume_an_old_target() {
        let ctx = egui::Context::default();
        let first = eframe::miv_test_script_window_witness::WindowWitnessFixture::new();
        let old_owner = {
            let _scope = first.enter(&ctx, egui::ViewportId::ROOT, 0x101);
            TestScriptWindowIdentity::Root {
                context_serial: 1,
                hwnd: 0x101,
                backend_token: eframe::miv_test_script_window_witness::active()
                    .unwrap()
                    .token(),
            }
        };
        let replacement = eframe::miv_test_script_window_witness::WindowWitnessFixture::new();
        let _scope = replacement.enter(&ctx, egui::ViewportId::ROOT, 0x101);
        ctx.begin_pass(Default::default());

        publish_action_pass_owner(&ctx, Some(old_owner.clone()));
        let observed_owner = action_pass_observation_for_active_backend(&ctx)
            .and_then(|observation| observation.owner);
        let (applied, _acknowledgement) = mpsc::sync_channel(1);
        let mut pending = VecDeque::from([PendingAction {
            action: KeyAction::GridToggleDetailsView,
            dispatch: PendingActionDispatch::Targeted {
                owner: old_owner,
                phase: TargetedActionPhase::AwaitingPass,
            },
            applied: Some(applied),
        }]);

        assert_eq!(observed_owner, None);
        assert!(!consume_pending_action_from(
            &mut pending,
            observed_owner.as_ref(),
            KeyAction::GridToggleDetailsView
        ));
        assert_eq!(pending.len(), 1);
        let _ = ctx.end_pass();
    }

    #[test]
    fn backend_witness_scope_is_context_scoped_nested_and_thread_local() {
        let first_context = egui::Context::default();
        let second_context = egui::Context::default();
        let viewport = egui::ViewportId::from_hash_of("same-viewport-separate-contexts");
        let first = eframe::miv_test_script_window_witness::WindowWitnessFixture::new();
        let second = eframe::miv_test_script_window_witness::WindowWitnessFixture::new();
        let first_scope = first.enter(&first_context, viewport, 0x201);
        let first_witness = eframe::miv_test_script_window_witness::active().unwrap();

        assert_eq!(
            eframe::miv_test_script_window_witness::latest(viewport),
            Some(first_witness)
        );
        std::thread::spawn(move || {
            assert_eq!(eframe::miv_test_script_window_witness::active(), None);
            assert_eq!(
                eframe::miv_test_script_window_witness::latest(viewport),
                None
            );
        })
        .join()
        .unwrap();
        {
            let _second_scope = second.enter(&second_context, viewport, 0x202);
            let second_witness = eframe::miv_test_script_window_witness::active().unwrap();
            assert_ne!(first_witness.token(), second_witness.token());
            assert_eq!(
                eframe::miv_test_script_window_witness::latest(viewport),
                Some(second_witness)
            );
        }

        assert_eq!(
            eframe::miv_test_script_window_witness::active(),
            Some(first_witness)
        );
        assert_eq!(
            eframe::miv_test_script_window_witness::latest(viewport),
            Some(first_witness)
        );
        drop(first_scope);
        assert_eq!(eframe::miv_test_script_window_witness::active(), None);
    }

    fn content_proof(
        context_serial: u64,
        generation: u64,
        page_index: usize,
        item: &str,
        texture: u64,
        source_kind: TestScriptPaintSourceKind,
    ) -> TestScriptContentProof {
        TestScriptContentProof {
            context_serial,
            items_generation: generation,
            page_index,
            item_identity: item.to_string(),
            source_texture_id: egui::TextureId::Managed(texture),
            source_kind,
        }
    }

    fn window_snapshot(
        identity: TestScriptWindowIdentity,
        generation: u64,
        page_index: usize,
        item: &str,
    ) -> TestScriptWindowSnapshot {
        TestScriptWindowSnapshot {
            identity: Some(identity.clone()),
            role: identity.role().to_string(),
            window_id: identity.window_id(),
            context_serial: identity.context_serial(),
            viewport_id: identity.viewport_id(),
            host_incarnation: identity.host_incarnation(),
            hwnd: Some(identity.hwnd()),
            backend_token: Some(identity.backend_token()),
            residence: "at_rest".to_string(),
            media_kind: "pdf".to_string(),
            page_index: Some(page_index),
            items_generation: generation,
            item_identity: item.to_string(),
            page_ready: true,
            viewport_rendered: false,
            viewport_revision: 0,
            paint_matches_current_page: false,
            full_texture_painted: false,
            paint_source: String::new(),
            paint_source_texture: String::new(),
            painted_page_index: None,
            paint_revision: 0,
        }
    }

    #[test]
    fn paint_evidence_requires_the_exact_owner_and_current_page_identity() {
        let owner = window_identity(7, 11, 13);
        let current = window_snapshot(owner.clone(), 17, 2, "pdf::current#2");
        let exact = content_proof(
            11,
            17,
            2,
            "pdf::current#2",
            19,
            TestScriptPaintSourceKind::FullOrProcessed,
        );
        let mut observations = HashMap::new();
        observations.insert(
            TestScriptPaintEvidenceKey {
                owner: owner.clone(),
                content: exact.clone(),
            },
            1,
        );
        assert!(
            joined_window_snapshots(&[current.clone()], &HashMap::new(), &observations)[0]
                .paint_matches_current_page
        );

        let stale_cases = [
            TestScriptPaintEvidenceKey {
                owner: window_identity(8, 11, 13),
                content: exact.clone(),
            },
            TestScriptPaintEvidenceKey {
                owner: window_identity(7, 12, 13),
                content: content_proof(
                    12,
                    17,
                    2,
                    "pdf::current#2",
                    19,
                    TestScriptPaintSourceKind::FullOrProcessed,
                ),
            },
            TestScriptPaintEvidenceKey {
                owner: window_identity(7, 11, 14),
                content: exact.clone(),
            },
            TestScriptPaintEvidenceKey {
                owner: owner.clone(),
                content: content_proof(
                    11,
                    18,
                    2,
                    "pdf::current#2",
                    19,
                    TestScriptPaintSourceKind::FullOrProcessed,
                ),
            },
            TestScriptPaintEvidenceKey {
                owner: owner.clone(),
                content: content_proof(
                    11,
                    17,
                    3,
                    "pdf::current#2",
                    19,
                    TestScriptPaintSourceKind::FullOrProcessed,
                ),
            },
            TestScriptPaintEvidenceKey {
                owner,
                content: content_proof(
                    11,
                    17,
                    2,
                    "pdf::other#2",
                    19,
                    TestScriptPaintSourceKind::FullOrProcessed,
                ),
            },
        ];
        for stale in stale_cases {
            let joined = joined_window_snapshots(
                &[current.clone()],
                &HashMap::new(),
                &HashMap::from([(stale, 2)]),
            );
            assert!(!joined[0].paint_matches_current_page);
        }
    }

    #[test]
    fn late_old_callback_cannot_replace_current_full_paint_evidence() {
        let current_owner = window_identity(7, 11, 14);
        let old_owner = window_identity(7, 11, 13);
        let window = window_snapshot(current_owner.clone(), 17, 2, "pdf::current#2");
        let full = content_proof(
            11,
            17,
            2,
            "pdf::current#2",
            20,
            TestScriptPaintSourceKind::FullOrProcessed,
        );
        let thumbnail = content_proof(
            11,
            17,
            2,
            "pdf::current#2",
            19,
            TestScriptPaintSourceKind::CatalogThumbnail,
        );
        let observations = HashMap::from([
            (
                TestScriptPaintEvidenceKey {
                    owner: current_owner.clone(),
                    content: full,
                },
                2,
            ),
            (
                TestScriptPaintEvidenceKey {
                    owner: old_owner,
                    content: thumbnail.clone(),
                },
                3,
            ),
            (
                TestScriptPaintEvidenceKey {
                    owner: current_owner,
                    content: thumbnail,
                },
                4,
            ),
        ]);

        let joined = joined_window_snapshots(&[window], &HashMap::new(), &observations);
        assert!(joined[0].full_texture_painted);
        assert_eq!(joined[0].paint_revision, 2);
        assert_eq!(joined[0].paint_source, "full_or_processed");
    }

    #[test]
    fn repeated_textures_keep_one_best_paint_observation_per_owner() {
        let (_tx, rx) = mpsc::channel();
        let snapshot = Arc::new(RwLock::new(TestScriptSnapshot::default()));
        let mut runtime = UiRuntime::new(
            rx,
            Arc::clone(&snapshot),
            Arc::new(InterruptState::default()),
        );
        let owner = window_identity(7, 11, 14);
        runtime
            .publish_windows(vec![window_snapshot(
                owner.clone(),
                17,
                2,
                "pdf::current#2",
            )])
            .unwrap();

        for texture in 100..164 {
            runtime
                .publish_window_frame(
                    owner.clone(),
                    Some(content_proof(
                        11,
                        17,
                        2,
                        "pdf::current#2",
                        texture,
                        TestScriptPaintSourceKind::CatalogThumbnail,
                    )),
                )
                .unwrap();
        }
        assert_eq!(runtime.paint_observations.len(), 1);

        runtime
            .publish_window_frame(
                owner.clone(),
                Some(content_proof(
                    11,
                    17,
                    2,
                    "pdf::current#2",
                    1000,
                    TestScriptPaintSourceKind::FullOrProcessed,
                )),
            )
            .unwrap();
        let full_revision = snapshot.read().unwrap().windows[0].paint_revision;
        runtime
            .publish_window_frame(
                owner,
                Some(content_proof(
                    11,
                    17,
                    2,
                    "pdf::current#2",
                    1001,
                    TestScriptPaintSourceKind::CatalogThumbnail,
                )),
            )
            .unwrap();

        assert_eq!(runtime.paint_observations.len(), 1);
        let published = snapshot.read().unwrap();
        assert!(published.windows[0].full_texture_painted);
        assert_eq!(published.windows[0].paint_revision, full_revision);
        assert!(published.windows[0].viewport_revision > full_revision);
    }

    #[test]
    fn table_change_prunes_stale_callbacks_without_clearing_current_evidence() {
        let (_tx, rx) = mpsc::channel();
        let snapshot = Arc::new(RwLock::new(TestScriptSnapshot::default()));
        let mut runtime = UiRuntime::new(
            rx,
            Arc::clone(&snapshot),
            Arc::new(InterruptState::default()),
        );
        let old_owner = window_identity(7, 11, 13);
        let current_owner = window_identity(7, 11, 14);
        let content = content_proof(
            11,
            17,
            2,
            "pdf::current#2",
            20,
            TestScriptPaintSourceKind::FullOrProcessed,
        );
        runtime
            .publish_windows(vec![window_snapshot(
                old_owner.clone(),
                17,
                2,
                "pdf::current#2",
            )])
            .unwrap();
        assert!(
            runtime
                .publish_window_frame(old_owner.clone(), Some(content.clone()))
                .unwrap()
        );

        runtime
            .publish_windows(vec![window_snapshot(
                current_owner.clone(),
                17,
                2,
                "pdf::current#2",
            )])
            .unwrap();
        assert!(
            runtime
                .publish_window_frame(current_owner, Some(content.clone()))
                .unwrap()
        );
        assert!(
            !runtime
                .publish_window_frame(old_owner, Some(content))
                .unwrap()
        );
        let published = snapshot.read().unwrap();
        assert!(published.windows[0].paint_matches_current_page);
        assert!(published.windows[0].full_texture_painted);
    }

    #[test]
    fn outcome_kinds_map_to_process_exit_codes() {
        assert_eq!(ScriptOutcomeKind::Success.exit_code(), 0);
        assert_ne!(ScriptOutcomeKind::ScriptFailure.exit_code(), 0);
        assert_ne!(ScriptOutcomeKind::EnvironmentFailure.exit_code(), 0);
    }
}
