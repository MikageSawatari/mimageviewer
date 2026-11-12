//! Worker-side pad vocabulary and drain acknowledgement. UI admission stays on the
//! runtime's real producer path; no physical input or downstream state is replaced.
use std::collections::HashSet;

use super::*;
use crate::gamepad::{PadAxis, PadButton, PadEvent, ScriptPadAck, ScriptPadSnapshot};

const ACK_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Clone, Default)]
pub(super) struct WorkerState {
    held: Arc<Mutex<Held>>,
    injected: Arc<AtomicBool>,
}

#[derive(Default)]
struct Held {
    buttons: HashSet<PadButton>,
    axes: HashMap<PadAxis, f32>,
}

impl Held {
    fn observe(&mut self, events: &[PadEvent]) {
        for event in events {
            match *event {
                PadEvent::ButtonPressed(button) => {
                    self.buttons.insert(button);
                }
                PadEvent::ButtonReleased(button) => {
                    self.buttons.remove(&button);
                }
                PadEvent::AxisChanged(axis, value) => {
                    self.axes.insert(axis, value);
                }
                PadEvent::Disconnected => {
                    self.buttons.clear();
                    self.axes.clear();
                }
                PadEvent::Connected => {}
            }
        }
    }

    fn neutral(&self) -> Vec<PadEvent> {
        let mut buttons = self.buttons.iter().copied().collect::<Vec<_>>();
        buttons.sort_unstable();
        let mut events = buttons
            .into_iter()
            .map(PadEvent::ButtonReleased)
            .collect::<Vec<_>>();
        // Stable axis order also makes traces reproducible.
        for axis in [
            PadAxis::LeftX,
            PadAxis::LeftY,
            PadAxis::RightX,
            PadAxis::RightY,
            PadAxis::LeftTrigger,
            PadAxis::RightTrigger,
        ] {
            if self.axes.get(&axis).is_some_and(|value| *value != 0.0) {
                events.push(PadEvent::AxisChanged(axis, 0.0));
            }
        }
        events
    }
}

impl WorkerState {
    pub(super) fn injected_flag(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.injected)
    }
}

#[derive(Debug)]
pub(super) struct Request {
    events: Vec<PadEvent>,
    deadline: Instant,
    injected: Arc<AtomicBool>,
    reply: mpsc::SyncSender<Result<ScriptPadAck, String>>,
}

pub(super) fn handle_request(
    request: Request,
    finishing: bool,
    inject: &mut impl FnMut(&[PadEvent]) -> Result<ScriptPadAck, String>,
) {
    let result = if finishing {
        Err("script is already finishing".to_string())
    } else if Instant::now() >= request.deadline {
        Err("gamepad UI response deadline expired before injection".to_string())
    } else {
        inject(&request.events).inspect(|_| request.injected.store(true, Ordering::Release))
    };
    let _ = request.reply.send(result);
}

pub(super) fn snapshot_map(snapshot: &ScriptPadSnapshot) -> Map {
    let mut map = Map::new();
    map.insert("enabled".into(), snapshot.enabled.into());
    map.insert("foreground".into(), snapshot.foreground.into());
    map.insert("reader".into(), snapshot.reader.into());
    map.insert("injected".into(), (snapshot.injected as rhai::INT).into());
    map.insert("drained".into(), (snapshot.drained as rhai::INT).into());
    map.insert("discarded".into(), (snapshot.discarded as rhai::INT).into());
    map
}

fn diagnostic(bridge: &RunnerBridge, ack: Option<&ScriptPadAck>) -> String {
    let snapshot = bridge.snapshot.try_read();
    let state = snapshot.as_ref().ok().map(|snapshot| &snapshot.gamepad);
    let counts = ack.map(|ack| ack.counters.snapshot());
    format!(
        "enabled={} foreground={} reader={} injected={} drained={} discarded={}",
        state.map(|s| s.enabled).unwrap_or(false),
        state.map(|s| s.foreground).unwrap_or(false),
        state.map(|s| s.reader).unwrap_or("unavailable"),
        counts
            .as_ref()
            .map(|c| c.injected)
            .or_else(|| state.map(|s| s.injected))
            .unwrap_or(0),
        counts
            .as_ref()
            .map(|c| c.drained)
            .or_else(|| state.map(|s| s.drained))
            .unwrap_or(0),
        counts
            .as_ref()
            .map(|c| c.discarded)
            .or_else(|| state.map(|s| s.discarded))
            .unwrap_or(0)
    )
}

fn environment_failure(bridge: &RunnerBridge, message: &str, ack: Option<&ScriptPadAck>) -> String {
    let message = format!("gamepad {message}: {}", diagnostic(bridge, ack));
    bridge.interrupt.fail(message.clone());
    message
}

fn send(bridge: &RunnerBridge, events: Vec<PadEvent>, cleanup: bool) -> Result<(), String> {
    send_with_timeout(bridge, events, cleanup, ACK_TIMEOUT)
}

fn send_with_timeout(
    bridge: &RunnerBridge,
    events: Vec<PadEvent>,
    cleanup: bool,
    timeout: Duration,
) -> Result<(), String> {
    if events.is_empty() {
        return Ok(());
    }
    let deadline = Instant::now() + timeout;
    let (reply, response) = mpsc::sync_channel(1);
    let command = UiCommand::Gamepad(Request {
        events: events.clone(),
        deadline,
        injected: bridge.gamepad.injected_flag(),
        reply,
    });
    if cleanup {
        bridge.send_unchecked(command)?;
    } else {
        bridge.send(command)?;
    }
    let ack = loop {
        if !cleanup {
            bridge.interrupt.check()?;
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(environment_failure(
                bridge,
                "timed out after 5000 ms awaiting UI response",
                None,
            ));
        }
        match response.recv_timeout(remaining.min(WAIT_POLL_INTERVAL)) {
            Ok(result) => {
                if Instant::now() >= deadline {
                    return Err(environment_failure(
                        bridge,
                        "UI response exceeded acknowledgement deadline",
                        None,
                    ));
                }
                break result.map_err(|error| environment_failure(bridge, &error, None))?;
            }
            Err(mpsc::RecvTimeoutError::Timeout) => (bridge.wake)(),
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                return Err(environment_failure(
                    bridge,
                    "UI response disconnected",
                    None,
                ));
            }
        }
    };
    bridge.gamepad.held.lock().unwrap().observe(&events);
    loop {
        if !cleanup {
            bridge.interrupt.check()?;
        }
        let counts = ack.counters.snapshot();
        if counts.discarded != ack.discarded {
            return Err(environment_failure(
                bridge,
                "input was discarded before drain acknowledgement",
                Some(&ack),
            ));
        }
        if Instant::now() >= deadline {
            return Err(environment_failure(
                bridge,
                "timed out after 5000 ms awaiting drain acknowledgement",
                Some(&ack),
            ));
        }
        if counts.drained >= ack.injected {
            return Ok(());
        }
        (bridge.wake)();
        let wait = deadline
            .saturating_duration_since(Instant::now())
            .min(WAIT_POLL_INTERVAL);
        if cleanup {
            std::thread::sleep(wait);
        } else {
            bridge.interrupt.wait(wait)?;
        }
    }
}

fn button(name: &str) -> Result<PadButton, String> {
    match name.trim().to_ascii_lowercase().as_str() {
        "a" | "south" => Ok(PadButton::South),
        "b" | "east" => Ok(PadButton::East),
        "x" | "west" => Ok(PadButton::West),
        "y" | "north" => Ok(PadButton::North),
        "up" | "dpad_up" => Ok(PadButton::DPadUp),
        "down" | "dpad_down" => Ok(PadButton::DPadDown),
        "left" | "dpad_left" => Ok(PadButton::DPadLeft),
        "right" | "dpad_right" => Ok(PadButton::DPadRight),
        "lb" => Ok(PadButton::LeftShoulder),
        "rb" => Ok(PadButton::RightShoulder),
        "lt" => Ok(PadButton::LeftTrigger),
        "rt" => Ok(PadButton::RightTrigger),
        "back" | "select" => Ok(PadButton::Select),
        "start" => Ok(PadButton::Start),
        _ => Err(format!("unsupported gamepad button: {name}")),
    }
}

fn axis(name: &str) -> Result<PadAxis, String> {
    match name.trim().to_ascii_lowercase().as_str() {
        "left_x" => Ok(PadAxis::LeftX),
        "left_y" => Ok(PadAxis::LeftY),
        "right_x" => Ok(PadAxis::RightX),
        "right_y" => Ok(PadAxis::RightY),
        "lt" => Ok(PadAxis::LeftTrigger),
        "rt" => Ok(PadAxis::RightTrigger),
        _ => Err(format!("unsupported gamepad axis: {name}")),
    }
}

fn value(value: Dynamic, trigger: bool) -> Result<f32, String> {
    let numeric = if value.is::<rhai::INT>() {
        value.cast::<rhai::INT>() as f64
    } else if value.is::<rhai::FLOAT>() {
        value.cast::<rhai::FLOAT>()
    } else {
        return Err("gamepad axis value must be an integer or float".into());
    };
    let minimum = if trigger { 0.0 } else { -1.0 };
    if !numeric.is_finite() || numeric < minimum || numeric > 1.0 {
        return Err(format!(
            "gamepad axis value must be finite and in {minimum}..=1: {numeric}"
        ));
    }
    Ok(numeric as f32)
}

fn side(name: &str, trigger: bool) -> Result<(PadAxis, PadAxis), String> {
    match (name.trim().to_ascii_lowercase().as_str(), trigger) {
        ("left", false) => Ok((PadAxis::LeftX, PadAxis::LeftY)),
        ("right", false) => Ok((PadAxis::RightX, PadAxis::RightY)),
        ("left", true) => Ok((PadAxis::LeftTrigger, PadAxis::LeftTrigger)),
        ("right", true) => Ok((PadAxis::RightTrigger, PadAxis::RightTrigger)),
        _ => Err(format!(
            "unsupported gamepad side: {name}; expected left or right"
        )),
    }
}

fn trigger_events(axis: PadAxis, value: f32, pressed: bool) -> Vec<PadEvent> {
    let button = if axis == PadAxis::LeftTrigger {
        PadButton::LeftTrigger
    } else {
        PadButton::RightTrigger
    };
    let mut events = Vec::new();
    // gilrs 0.11.2 gamepad.rs 294-318: edge first, then ButtonChanged;
    // default hysteresis thresholds are 0.75 pressed / 0.65 released.
    if value >= 0.75 && !pressed {
        events.push(PadEvent::ButtonPressed(button));
    } else if value <= 0.65 && pressed {
        events.push(PadEvent::ButtonReleased(button));
    }
    events.push(PadEvent::AxisChanged(axis, value));
    events
}

fn hold(bridge: &RunnerBridge, name: &str, duration: Duration) -> Result<(), Box<EvalAltResult>> {
    let button = button(name).map_err(rhai_error)?;
    let release = || send(bridge, vec![PadEvent::ButtonReleased(button)], true);
    if let Err(error) = send(bridge, vec![PadEvent::ButtonPressed(button)], false) {
        let _ = release();
        return Err(rhai_error(error));
    }
    if let Err(error) = bridge.interrupt.wait(duration) {
        let _ = release();
        return Err(rhai_error(error));
    }
    release().map_err(rhai_error)
}

pub(super) fn register(engine: &mut Engine, bridge: RunnerBridge) {
    for (name, pressed) in [("pad_press", true), ("pad_release", false)] {
        let bridge = bridge.clone();
        engine.register_fn(
            name,
            move |name: ImmutableString| -> Result<(), Box<EvalAltResult>> {
                let button = button(&name).map_err(rhai_error)?;
                send(
                    &bridge,
                    vec![if pressed {
                        PadEvent::ButtonPressed(button)
                    } else {
                        PadEvent::ButtonReleased(button)
                    }],
                    false,
                )
                .map_err(rhai_error)
            },
        );
    }
    let tap = bridge.clone();
    engine.register_fn("pad_tap", move |name: ImmutableString| {
        hold(&tap, &name, Duration::ZERO)
    });
    let held = bridge.clone();
    engine.register_fn("pad_hold", move |name: ImmutableString, ms: rhai::INT| {
        hold(&held, &name, checked_duration(ms, "pad_hold ms")?)
    });
    let axes = bridge.clone();
    engine.register_fn(
        "pad_axis",
        move |name: ImmutableString, numeric: Dynamic| -> Result<(), Box<EvalAltResult>> {
            let axis = axis(&name).map_err(rhai_error)?;
            let value = value(
                numeric,
                matches!(axis, PadAxis::LeftTrigger | PadAxis::RightTrigger),
            )
            .map_err(rhai_error)?;
            send(&axes, vec![PadEvent::AxisChanged(axis, value)], false).map_err(rhai_error)
        },
    );
    let stick = bridge.clone();
    engine.register_fn(
        "pad_stick",
        move |name: ImmutableString, x: Dynamic, y: Dynamic| -> Result<(), Box<EvalAltResult>> {
            let (axis_x, axis_y) = side(&name, false).map_err(rhai_error)?;
            let x = value(x, false).map_err(rhai_error)?;
            let y = value(y, false).map_err(rhai_error)?;
            send(
                &stick,
                vec![
                    PadEvent::AxisChanged(axis_x, x),
                    PadEvent::AxisChanged(axis_y, y),
                ],
                false,
            )
            .map_err(rhai_error)
        },
    );
    let trigger = bridge.clone();
    engine.register_fn(
        "pad_trigger",
        move |name: ImmutableString, numeric: Dynamic| -> Result<(), Box<EvalAltResult>> {
            let (axis, _) = side(&name, true).map_err(rhai_error)?;
            let value = value(numeric, true).map_err(rhai_error)?;
            let button = if axis == PadAxis::LeftTrigger {
                PadButton::LeftTrigger
            } else {
                PadButton::RightTrigger
            };
            let pressed = trigger
                .gamepad
                .held
                .lock()
                .unwrap()
                .buttons
                .contains(&button);
            send(&trigger, trigger_events(axis, value, pressed), false).map_err(rhai_error)
        },
    );
    let neutral = bridge.clone();
    engine.register_fn("pad_neutral", move || -> Result<(), Box<EvalAltResult>> {
        let events = neutral.gamepad.held.lock().unwrap().neutral();
        send(&neutral, events, true).map_err(rhai_error)
    });
    for (name, event) in [
        ("pad_connect", PadEvent::Connected),
        ("pad_disconnect", PadEvent::Disconnected),
    ] {
        let bridge = bridge.clone();
        engine.register_fn(name, move || -> Result<(), Box<EvalAltResult>> {
            send(&bridge, vec![event], false).map_err(rhai_error)
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gamepad::GamepadRuntime;

    fn bridge() -> (RunnerBridge, mpsc::Receiver<UiCommand>) {
        let (tx, rx) = mpsc::channel();
        (
            RunnerBridge {
                tx,
                gamepad: WorkerState::default(),
                snapshot: Arc::new(RwLock::new(TestScriptSnapshot::default())),
                interrupt: Arc::new(InterruptState::default()),
                wake: Arc::new(|| {}),
                next_hold_id: Arc::new(AtomicU64::new(0)),
                action_selection: Arc::new(Mutex::new(TestScriptActionSelection::LegacyImplicit)),
                pointer_regions: pointer_input::new_shared_catalog(),
            },
            rx,
        )
    }

    fn request(rx: &mpsc::Receiver<UiCommand>) -> Request {
        match rx
            .recv_timeout(Duration::from_secs(2))
            .expect("gamepad request")
        {
            UiCommand::Gamepad(request) => request,
            command => panic!("expected gamepad request, got {command:?}"),
        }
    }

    #[test]
    fn names_ranges_and_trigger_hysteresis_follow_gilrs() {
        for (names, expected) in [
            (&["a", "SOUTH"][..], PadButton::South),
            (&["b", "east"][..], PadButton::East),
            (&["x", "west"][..], PadButton::West),
            (&["y", "north"][..], PadButton::North),
            (&["up", "dpad_up"][..], PadButton::DPadUp),
            (&["down", "dpad_down"][..], PadButton::DPadDown),
            (&["left", "dpad_left"][..], PadButton::DPadLeft),
            (&["right", "dpad_right"][..], PadButton::DPadRight),
            (&["lb"][..], PadButton::LeftShoulder),
            (&["rb"][..], PadButton::RightShoulder),
            (&["lt"][..], PadButton::LeftTrigger),
            (&["rt"][..], PadButton::RightTrigger),
            (&["back", "select"][..], PadButton::Select),
            (&["start"][..], PadButton::Start),
        ] {
            for name in names {
                assert_eq!(button(name).unwrap(), expected);
            }
        }
        assert!(button("unknown").is_err());
        for name in ["left_x", "LEFT_Y", "right_x", "right_y", "lt", "rt"] {
            assert!(axis(name).is_ok());
        }
        assert!(axis("x").is_err());
        assert_eq!(value((-1_i64).into(), false).unwrap(), -1.0);
        assert_eq!(value(1_i64.into(), true).unwrap(), 1.0);
        for invalid in [-1.01, 1.01, f64::INFINITY, f64::NAN] {
            assert!(value(invalid.into(), false).is_err());
        }
        assert!(value((-0.01_f64).into(), true).is_err());
        assert!(value("1".into(), true).is_err());
        assert!(side("middle", false).is_err());
        assert_eq!(
            trigger_events(PadAxis::LeftTrigger, 0.75, false),
            [
                PadEvent::ButtonPressed(PadButton::LeftTrigger),
                PadEvent::AxisChanged(PadAxis::LeftTrigger, 0.75)
            ]
        );
        assert_eq!(
            trigger_events(PadAxis::LeftTrigger, 0.70, true),
            [PadEvent::AxisChanged(PadAxis::LeftTrigger, 0.70)]
        );
        assert_eq!(
            trigger_events(PadAxis::LeftTrigger, 0.65, true),
            [
                PadEvent::ButtonReleased(PadButton::LeftTrigger),
                PadEvent::AxisChanged(PadAxis::LeftTrigger, 0.65)
            ]
        );
        assert_eq!(
            trigger_events(PadAxis::RightTrigger, 0.70, false),
            [PadEvent::AxisChanged(PadAxis::RightTrigger, 0.70)]
        );
    }

    #[test]
    fn worker_waits_for_direct_drain_and_tap_uses_distinct_drains_without_snapshot() {
        let _focus = crate::test_native_focus::scoped(1, 1, true);
        let ctx = egui::Context::default();
        let mut runtime = GamepadRuntime::new();
        let (bridge, rx) = bridge();
        let state = bridge.gamepad.clone();
        // An unavailable ROOT snapshot writer cannot prevent a successful ack.
        let snapshot = Arc::clone(&bridge.snapshot);
        let _snapshot_writer = snapshot.write().unwrap();
        let worker = std::thread::spawn(move || {
            hold(&bridge, "a", Duration::ZERO).map_err(|e| e.to_string())
        });
        let down = request(&rx);
        assert_eq!(down.events, [PadEvent::ButtonPressed(PadButton::South)]);
        handle_request(down, false, &mut |events| {
            runtime.inject_script_events(&ctx, true, events)
        });
        assert!(matches!(
            rx.recv_timeout(Duration::from_millis(30)),
            Err(mpsc::RecvTimeoutError::Timeout)
        ));
        assert!(!worker.is_finished());
        assert_eq!(runtime.drain(&ctx, true).len(), 1);
        let up = request(&rx);
        assert_eq!(up.events, [PadEvent::ButtonReleased(PadButton::South)]);
        handle_request(up, false, &mut |events| {
            runtime.inject_script_events(&ctx, true, events)
        });
        assert!(!worker.is_finished());
        assert_eq!(runtime.drain(&ctx, true).len(), 1);
        worker.join().unwrap().unwrap();
        assert!(state.injected.load(Ordering::Acquire));
        assert!(state.held.lock().unwrap().buttons.is_empty());
    }

    #[test]
    fn discarded_input_is_failure_and_ui_response_shares_deadline() {
        let _focus = crate::test_native_focus::scoped(1, 1, true);
        let ctx = egui::Context::default();
        let mut runtime = GamepadRuntime::new();
        let (bridge, rx) = bridge();
        let worker = std::thread::spawn(move || send(&bridge, vec![PadEvent::Connected], false));
        handle_request(request(&rx), false, &mut |events| {
            runtime.inject_script_events(&ctx, true, events)
        });
        assert!(runtime.drain(&ctx, false).is_empty());
        let error = worker.join().unwrap().unwrap_err();
        assert!(error.contains("discarded"), "{error}");
        assert!(
            error.contains("injected=1")
                && error.contains("drained=0")
                && error.contains("discarded=1")
        );

        let (bridge, rx) = self::bridge();
        let worker = std::thread::spawn(move || {
            send_with_timeout(
                &bridge,
                vec![PadEvent::Connected],
                false,
                Duration::from_millis(20),
            )
        });
        let pending = request(&rx);
        let error = worker.join().unwrap().unwrap_err();
        assert!(
            error.contains("UI response")
                && error.contains("enabled=")
                && error.contains("reader=")
        );
        let mut called = false;
        handle_request(pending, false, &mut |_| {
            called = true;
            unreachable!()
        });
        assert!(!called, "expired UI requests must never inject later");

        let (bridge, rx) = self::bridge();
        let worker = std::thread::spawn(move || {
            send_with_timeout(
                &bridge,
                vec![PadEvent::Connected],
                false,
                Duration::from_millis(100),
            )
        });
        handle_request(request(&rx), false, &mut |events| {
            runtime.inject_script_events(&ctx, true, events)
        });
        // Receiving the shared counters is not an ack until App drains them.
        let error = worker.join().unwrap().unwrap_err();
        assert!(error.contains("drain acknowledgement"), "{error}");
        assert!(
            error.contains("injected=2")
                && error.contains("drained=0")
                && error.contains("discarded=1"),
            "{error}"
        );
    }

    #[test]
    fn finish_guard_rejects_cleanup_and_interrupted_hold_still_releases() {
        let _focus = crate::test_native_focus::scoped(1, 1, true);
        let ctx = egui::Context::default();
        let mut runtime = GamepadRuntime::new();
        let (bridge, rx) = bridge();
        let interrupt = Arc::clone(&bridge.interrupt);
        let worker = std::thread::spawn(move || {
            hold(&bridge, "x", Duration::from_secs(60)).map_err(|e| e.to_string())
        });
        handle_request(request(&rx), false, &mut |events| {
            runtime.inject_script_events(&ctx, true, events)
        });
        runtime.drain(&ctx, true);
        interrupt.fail("interrupted hold");
        let release = request(&rx);
        assert_eq!(release.events, [PadEvent::ButtonReleased(PadButton::West)]);
        handle_request(release, false, &mut |events| {
            runtime.inject_script_events(&ctx, true, events)
        });
        runtime.drain(&ctx, true);
        assert!(
            worker
                .join()
                .unwrap()
                .unwrap_err()
                .contains("interrupted hold")
        );

        let (bridge, rx) = self::bridge();
        let worker = std::thread::spawn(move || send(&bridge, vec![PadEvent::Disconnected], true));
        let mut called = false;
        handle_request(request(&rx), true, &mut |_| {
            called = true;
            unreachable!()
        });
        assert!(!called);
        assert!(
            worker
                .join()
                .unwrap()
                .unwrap_err()
                .contains("already finishing")
        );
    }

    #[test]
    fn held_neutral_disconnect_and_all_rhai_commands_are_registered() {
        let _focus = crate::test_native_focus::scoped(1, 1, true);
        let ctx = egui::Context::default();
        let mut runtime = GamepadRuntime::new();
        let (bridge, rx) = bridge();
        let worker_bridge = bridge.clone();
        let worker = std::thread::spawn(move || {
            let engine = build_engine(worker_bridge);
            engine
                .eval::<()>(
                    r#"
                pad_connect(); pad_press("A"); pad_release("south"); pad_tap("b"); pad_hold("x", 0);
                pad_axis("left_x", -1); pad_stick("right", 0.5, 1); pad_trigger("left", 0.75);
                pad_trigger("left", 0.7); pad_neutral();
                pad_press("y"); pad_axis("left_y", 0.6); pad_disconnect();
            "#,
                )
                .map_err(|e| e.to_string())
        });
        while !worker.is_finished() {
            match rx.recv_timeout(Duration::from_millis(20)) {
                Ok(UiCommand::Gamepad(request)) => {
                    handle_request(request, false, &mut |events| {
                        runtime.inject_script_events(&ctx, true, events)
                    });
                    runtime.drain(&ctx, true);
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                other => panic!("unexpected command: {other:?}"),
            }
        }
        worker.join().unwrap().unwrap();
        assert!(bridge.gamepad.held.lock().unwrap().neutral().is_empty());
        let counts = runtime.script_snapshot(true);
        assert_eq!(counts.injected, counts.drained);
        assert_eq!(
            snapshot_map(&counts)["reader"].clone().cast::<String>(),
            "script"
        );
    }

    #[test]
    fn production_ui_dispatch_cleans_up_once_after_interrupt_already_started_finish() {
        let _serial = crate::key_input::lock_test_input();
        let _focus = crate::test_native_focus::scoped(1, 1, true);
        struct Reset;
        impl Drop for Reset {
            fn drop(&mut self) {
                runtime().lock().unwrap().take();
                crate::key_input::clear_test_synthetic_input();
            }
        }
        assert!(crate::key_input::arm_synthetic_input());
        let _reset = Reset;
        let ctx = egui::Context::default();
        let (bridge, rx) = bridge();
        let mut pad = GamepadRuntime::new();
        let (reply, _ack) = mpsc::sync_channel(1);
        handle_request(
            Request {
                events: vec![PadEvent::ButtonPressed(PadButton::South)],
                deadline: Instant::now() + ACK_TIMEOUT,
                injected: bridge.gamepad.injected_flag(),
                reply,
            },
            false,
            &mut |events| pad.inject_script_events(&ctx, true, events),
        );
        pad.drain(&ctx, true);

        let mut ui_runtime = UiRuntime::new(
            rx,
            Arc::clone(&bridge.snapshot),
            Arc::clone(&bridge.interrupt),
            Arc::clone(&bridge.pointer_regions),
        );
        ui_runtime.begin_finish(ScriptOutcome::success(), frame_key(&ctx));
        // This unit test exercises dispatch without arming the process-exit watchdog.
        ui_runtime.failure_capture_requested = true;
        bridge.interrupt.fail("gamepad acknowledgement timed out");
        let (reply, rejected) = mpsc::sync_channel(1);
        bridge
            .send_unchecked(UiCommand::Gamepad(Request {
                events: vec![PadEvent::Connected],
                deadline: Instant::now() + ACK_TIMEOUT,
                injected: bridge.gamepad.injected_flag(),
                reply,
            }))
            .unwrap();
        finish_worker(&bridge, ScriptOutcome::script_failure("interrupted"));
        assert!(runtime().lock().unwrap().replace(ui_runtime).is_none());
        let mut injected = Vec::new();
        assert!(!ui_update(&ctx, TestScriptSnapshot::default(), |events| {
            injected.extend_from_slice(events);
            pad.inject_script_events(&ctx, true, events)
        }));
        assert!(
            rejected
                .recv()
                .unwrap()
                .unwrap_err()
                .contains("already finishing")
        );
        assert_eq!(injected, [PadEvent::Disconnected]);
        assert_eq!(pad.drain(&ctx, true).len(), 1);
        assert!(!ui_update(&ctx, TestScriptSnapshot::default(), |events| {
            injected.extend_from_slice(events);
            pad.inject_script_events(&ctx, true, events)
        }));
        assert_eq!(
            injected,
            [PadEvent::Disconnected],
            "terminal cleanup must not repeat"
        );
    }

    #[test]
    fn ui_admission_refusal_is_environment_failure_but_invalid_name_is_script_failure() {
        let (bridge, rx) = bridge();
        let worker = std::thread::spawn(move || evaluate_source("pad_press(\"a\");", bridge));
        handle_request(request(&rx), false, &mut |_| {
            Err("environment: enabled=false; wait_until(|s| s.gamepad.foreground, ms)".into())
        });
        let outcome = worker.join().unwrap();
        assert_eq!(outcome.kind, ScriptOutcomeKind::EnvironmentFailure);
        assert!(
            outcome.message.contains("enabled=false") && outcome.message.contains("wait_until")
        );
        let (bridge, _) = self::bridge();
        let outcome = evaluate_source("pad_press(\"not-a-button\");", bridge);
        assert_eq!(outcome.kind, ScriptOutcomeKind::ScriptFailure);
    }

    #[test]
    fn gamepad_basic_scenario_compiles_with_runner_api() {
        let (bridge, _) = bridge();
        build_engine(bridge)
            .compile(include_str!("../../scripts/ui-smoke/gamepad-basic.rhai"))
            .unwrap();
    }
}
