//! Opt-in Rhai runner for isolated in-process application tests.
//!
//! The worker evaluates scripts and sends typed commands only. Synthetic input
//! is materialized by `key_input`'s ROOT plugin, while App/UI state publication,
//! direct `KeyAction` delivery, failure classification, and shutdown stay on
//! the UI thread.

#![cfg_attr(all(test, not(feature = "test-script")), allow(dead_code))]

use std::collections::VecDeque;
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
    pub(crate) upload_deferral_streak: i64,
    /// Why the page has no stand-in to show, or empty. See `PassthroughUnavailable`.
    pub(crate) passthrough_unavailable: String,
    pub(crate) keymap_level_observations: Vec<KeymapLevelObservation>,
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
            upload_deferral_streak: 0,
            passthrough_unavailable: String::new(),
            keymap_level_observations: Vec::new(),
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
        insert!(upload_deferral_streak);
        insert!(passthrough_unavailable);
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

    fn wait(&self, duration: Duration) -> Result<(), String> {
        let guard = self
            .failure
            .lock()
            .map_err(|_| "script interrupt state is poisoned".to_string())?;
        if let Some(message) = guard.as_ref() {
            return Err(message.clone());
        }
        let (guard, _) = self
            .changed
            .wait_timeout(guard, duration)
            .map_err(|_| "script interrupt state is poisoned".to_string())?;
        guard.as_ref().cloned().map_or(Ok(()), Err)
    }
}

#[derive(Clone)]
struct RunnerBridge {
    tx: mpsc::Sender<UiCommand>,
    snapshot: Arc<RwLock<TestScriptSnapshot>>,
    interrupt: Arc<InterruptState>,
    wake: Arc<dyn Fn() + Send + Sync>,
    next_hold_id: Arc<AtomicU64>,
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
            let (applied_tx, applied_rx) = mpsc::sync_channel(1);
            action_bridge
                .send(UiCommand::RunAction {
                    action,
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
    // `None` means a non-consuming pressed_action peek already acknowledged
    // the command. Keep the entry until the frame ends so later peeks observe
    // the same press, just like an egui input event.
    applied: Option<mpsc::SyncSender<Result<(), String>>>,
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
        }
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
        self.finish = Some(FinishState {
            outcome,
            started_frame: frame,
            newer_frames: 0,
        });
    }

    fn fail_environment(&mut self, message: String, frame: u64) {
        self.interrupt.fail(message.clone());
        self.begin_finish(ScriptOutcome::environment_failure(message), frame);
    }

    fn request_cancel(&mut self) -> bool {
        if self.cancel_requested {
            return true;
        }
        self.cancel_requested = crate::key_input::cancel_synthetic_input(Instant::now());
        self.cancel_requested
    }

    fn expire_unconsumed_actions(&mut self, frame: u64) {
        if self.pending_actions.is_empty() {
            return;
        }
        let unconsumed = self
            .pending_actions
            .drain(..)
            .filter(|pending| pending.applied.is_some())
            .collect::<Vec<_>>();
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

fn consume_pending_action_from(
    pending_actions: &mut VecDeque<PendingAction>,
    action: KeyAction,
) -> bool {
    let Some(index) = pending_actions
        .iter()
        .position(|pending| pending.action == action)
    else {
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
    action: KeyAction,
) -> bool {
    let Some(pending) = pending_actions
        .iter_mut()
        .find(|pending| pending.action == action)
    else {
        return false;
    };
    if let Some(applied) = pending.applied.take() {
        let _ = applied.send(Ok(()));
    }
    true
}

pub(crate) fn consume_pending_action(action: KeyAction) -> bool {
    let Ok(mut guard) = runtime().lock() else {
        return false;
    };
    let Some(runtime) = guard.as_mut() else {
        return false;
    };
    consume_pending_action_from(&mut runtime.pending_actions, action)
}

pub(crate) fn peek_pending_action(action: KeyAction) -> bool {
    let Ok(mut guard) = runtime().lock() else {
        return false;
    };
    let Some(runtime) = guard.as_mut() else {
        return false;
    };
    peek_pending_action_from(&mut runtime.pending_actions, action)
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
            runtime.expire_unconsumed_actions(frame);
        }
        runtime.last_frame = Some(frame);
    }
    emit_perf_level_reads(&snapshot.keymap_level_observations);
    if let Ok(mut published) = runtime.snapshot.write() {
        *published = snapshot;
    } else {
        runtime.fail_environment("test-script snapshot is poisoned".to_string(), frame);
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
            UiCommand::RunAction { action, applied } => {
                if runtime.finish.is_some() {
                    let _ = applied.send(Err("script is already finishing".to_string()));
                } else {
                    runtime.pending_actions.push_back(PendingAction {
                        action,
                        applied: Some(applied),
                    });
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
    let Some(runtime) = active else {
        return;
    };
    PROCESS_EXIT_CODE.store(EXIT_ENVIRONMENT_FAILURE, Ordering::Release);
    let outcome = ScriptOutcome::environment_failure(
        "application exited before the test script completed".to_string(),
    );
    runtime
        .interrupt
        .fail("application exited before the test script completed");
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
                applied,
            } => {
                assert_eq!(actual, action);
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
    fn pending_action_peek_is_repeatable_within_the_ui_frame() {
        let action = KeyAction::FsClose;
        let (applied, acknowledgement) = mpsc::sync_channel(1);
        let mut pending = VecDeque::from([PendingAction {
            action,
            applied: Some(applied),
        }]);

        assert!(peek_pending_action_from(&mut pending, action));
        assert_eq!(
            acknowledgement
                .recv_timeout(Duration::from_secs(1))
                .unwrap(),
            Ok(())
        );
        assert!(peek_pending_action_from(&mut pending, action));
        assert!(matches!(
            acknowledgement.try_recv(),
            Err(mpsc::TryRecvError::Disconnected)
        ));
        assert!(consume_pending_action_from(&mut pending, action));
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

    #[test]
    fn outcome_kinds_map_to_process_exit_codes() {
        assert_eq!(ScriptOutcomeKind::Success.exit_code(), 0);
        assert_ne!(ScriptOutcomeKind::ScriptFailure.exit_code(), 0);
        assert_ne!(ScriptOutcomeKind::EnvironmentFailure.exit_code(), 0);
    }
}
