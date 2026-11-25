//! Hidden production HWND handler tests. No application launch, focus changes,
//! or global input injection; the GETMESSAGE hook belongs only to this thread.
use super::super::native_window_host::NativeWindowHost;
use super::*;
use std::cell::Cell;
use windows::Win32::UI::WindowsAndMessaging::{
    CWPSTRUCT, CallNextHookEx, HHOOK, SendMessageW, SetWindowsHookExW, UnhookWindowsHookEx,
    WH_CALLWNDPROC, WH_GETMESSAGE, WM_APP, WM_SIZE,
};

const MARKER: u32 = WM_APP + 0x364;

struct Fixture {
    host: NativeWindowHost,
    receiver: NativeWindowEventReceiver,
    _pump_receiver: NativeWindowEventReceiver,
    overflow: Arc<AtomicBool>,
}

impl Fixture {
    fn new(with_hud: bool) -> Self {
        let overflow = Arc::new(AtomicBool::new(false));
        let (pump, pump_receiver) = native_window_event_route(1024, Arc::clone(&overflow));
        let (render, receiver) = native_window_event_route(1024, Arc::clone(&overflow));
        let sink =
            |source| NativeVideoWindowEventSink::new(1, 1, source, pump.clone(), render.clone());
        let host = NativeWindowHost::create_hidden_for_geometry_test(
            1,
            sink(NativeVideoWindowSource::Presenter),
            sink(NativeVideoWindowSource::Hud),
            with_hud,
        );
        let fixture = Self {
            host,
            receiver,
            _pump_receiver: pump_receiver,
            overflow,
        };
        fixture.drain();
        fixture
    }

    fn hwnd(&self) -> HWND {
        self.host.hwnd()
    }

    fn drain(&self) -> Vec<NativeVideoWindowEventEnvelope> {
        assert!(!self.overflow.load(Ordering::Acquire), "test sink overflow");
        self.receiver.drain()
    }

    fn keys(&self) -> Vec<NativeVideoKeyEvent> {
        self.drain()
            .into_iter()
            .filter_map(|envelope| match envelope.event {
                NativeVideoWindowEvent::KeyDown(key) => Some(key),
                _ => None,
            })
            .collect()
    }

    fn post_key(&self, message: u32, vk: usize, repeat: bool) {
        let scan = if vk == 0xA6 { 0x6A } else { 0x69 };
        let mut bits = 1 | (scan << 16) | (1 << 24);
        if repeat {
            bits |= 1 << 30;
        }
        if matches!(message, WM_KEYUP | WM_SYSKEYUP) {
            bits |= (1_u32 << 31) as isize | (1 << 30);
        }
        unsafe {
            PostMessageW(Some(self.hwnd()), message, WPARAM(vk), LPARAM(bits)).unwrap();
        }
    }

    fn press(&self, vk: usize) {
        self.post_key(WM_KEYDOWN, vk, false);
        assert!(!pump_thread_messages_inner(None));
        let keys = self.keys();
        assert_eq!(keys.len(), 1);
        assert_eq!(keys[0].virtual_key, vk as u32);
        assert!(keys[0].extended);
        assert_eq!(keys[0].scan_code, if vk == 0xA6 { 0x6A } else { 0x69 });
    }
}

#[derive(Clone, Copy)]
struct Command {
    receiver: HWND,
    source: HWND,
    word: u16,
    nested: bool,
}

impl Command {
    fn back(hwnd: HWND) -> Self {
        Self {
            receiver: hwnd,
            source: hwnd,
            word: 1,
            nested: false,
        }
    }
    fn lparam(self) -> LPARAM {
        LPARAM((u32::from(self.word) << 16) as isize)
    }
    fn send(self) -> LRESULT {
        unsafe {
            SendMessageW(
                self.receiver,
                WM_APPCOMMAND,
                Some(WPARAM(self.source.0 as usize)),
                Some(self.lparam()),
            )
        }
    }
}

#[derive(Clone, Copy, Default)]
struct HookResult {
    markers: usize,
    commands: usize,
    result: isize,
}

thread_local! {
    static HOOK_COMMAND: Cell<Option<Command>> = const { Cell::new(None) };
    static NESTED_COMMAND: Cell<Option<Command>> = const { Cell::new(None) };
    static HOOK_RESULT: Cell<HookResult> = const { Cell::new(HookResult { markers: 0, commands: 0, result: 0 }) };
}

fn record_command(command: Command) {
    let result = command.send();
    let mut observed = HOOK_RESULT.get();
    observed.commands += 1;
    observed.result = result.0;
    HOOK_RESULT.set(observed);
}

unsafe extern "system" fn getmessage_hook(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    // Do not assert/panic across a USER32 callback. All observations are checked
    // after the real pump has returned and the hook has been removed.
    if code >= 0 && wparam.0 == PM_REMOVE.0 as usize && lparam.0 != 0 {
        let msg = unsafe { &*(lparam.0 as *const MSG) };
        if msg.message == MARKER
            && let Some(command) = HOOK_COMMAND.take()
        {
            let mut observed = HOOK_RESULT.get();
            observed.markers += 1;
            HOOK_RESULT.set(observed);
            if command.nested {
                NESTED_COMMAND.set(Some(command));
                let position = WINDOWPOS {
                    hwnd: command.receiver,
                    cx: 320,
                    cy: 180,
                    flags: SWP_NOMOVE | SWP_NOACTIVATE,
                    ..Default::default()
                };
                unsafe {
                    SendMessageW(
                        command.receiver,
                        WM_WINDOWPOSCHANGED,
                        Some(WPARAM(0)),
                        Some(LPARAM((&position as *const WINDOWPOS) as isize)),
                    );
                }
                NESTED_COMMAND.set(None);
            } else {
                record_command(command);
            }
        }
    }
    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}

unsafe extern "system" fn callwndproc_hook(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code >= 0 && lparam.0 != 0 {
        let call = unsafe { &*(lparam.0 as *const CWPSTRUCT) };
        if call.message == WM_SIZE
            && NESTED_COMMAND
                .get()
                .is_some_and(|command| command.receiver == call.hwnd)
            && let Some(command) = NESTED_COMMAND.take()
        {
            record_command(command);
        }
    }
    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}

struct Hook(HHOOK);
impl Drop for Hook {
    fn drop(&mut self) {
        unsafe {
            let _ = UnhookWindowsHookEx(self.0);
        }
    }
}

fn during_peek(command: Command) {
    let get_hook = Hook(unsafe {
        SetWindowsHookExW(
            WH_GETMESSAGE,
            Some(getmessage_hook),
            None,
            GetCurrentThreadId(),
        )
        .expect("thread GETMESSAGE hook")
    });
    let nested_hook = command.nested.then(|| {
        Hook(unsafe {
            SetWindowsHookExW(
                WH_CALLWNDPROC,
                Some(callwndproc_hook),
                None,
                GetCurrentThreadId(),
            )
            .expect("thread CALLWNDPROC hook")
        })
    });
    HOOK_RESULT.set(HookResult::default());
    HOOK_COMMAND.set(Some(command));
    unsafe {
        PostMessageW(Some(command.receiver), MARKER, WPARAM(0), LPARAM(0)).unwrap();
    }
    let quit = pump_thread_messages_inner(None);
    let unconsumed = HOOK_COMMAND.take();
    let observed = HOOK_RESULT.get();
    drop(nested_hook);
    drop(get_hook);
    assert!(!quit);
    assert!(unconsumed.is_none(), "marker was not observed");
    assert_eq!(observed.markers, 1);
    assert_eq!(
        observed.commands, 1,
        "nested command must reach the presenter"
    );
    assert_eq!(observed.result, 1, "WM_APPCOMMAND must be handled");
}

fn assert_synthesized(keys: &[NativeVideoKeyEvent], vk: u32, label: &str) {
    assert_eq!(keys.len(), 1);
    assert_eq!(keys[0].virtual_key, vk);
    assert_eq!(keys[0].scan_code, 0);
    assert!(!keys[0].extended);
    assert!(!keys[0].repeat);
    let crate::mouse_seek_debug::NativeVideoInputOrigin::Win32AppCommand { delivery, .. } =
        keys[0].receipt.origin
    else {
        panic!("AppCommand receipt");
    };
    assert_eq!(delivery.paired_receipt, None);
    match label {
        "during_peek" => {
            assert_eq!(
                delivery.pump_frame,
                Some(crate::mouse_seek_debug::NativePumpFrameKind::Peek)
            );
            assert_eq!(delivery.in_send_flags & 0xF, 0);
            assert_eq!(delivery.outer, None);
        }
        "queued" => {
            assert!(delivery.dispatching);
            assert_eq!(
                delivery.pump_frame,
                Some(crate::mouse_seek_debug::NativePumpFrameKind::Dispatch)
            );
        }
        "unknown" => {
            assert!(!delivery.dispatching);
            assert_eq!(delivery.pump_frame, None);
            assert_eq!(delivery.outer, None);
        }
        "sent_cross_thread" => assert_ne!(delivery.in_send_flags & 0xF, 0),
        "nested" => assert_eq!(
            delivery.outer.map(|outer| outer.message),
            Some(WM_WINDOWPOSCHANGED)
        ),
        _ => panic!("unexpected expected delivery"),
    }
}

// These four regressions count both the real key and its echo. The previous
// presenter handler emits a second KeyDown for every echo.
#[test]
fn browser_key_echo_back_inside_real_peek_is_suppressed() {
    let fixture = Fixture::new(false);
    fixture.post_key(WM_KEYDOWN, 0xA6, false);
    during_peek(Command::back(fixture.hwnd()));
    let keys = fixture.keys();
    assert_eq!(keys.len(), 1);
    assert_eq!(keys[0].scan_code, 0x6A);
    assert!(keys[0].extended);
}

#[test]
fn browser_key_echo_forward_inside_real_peek_is_suppressed() {
    let fixture = Fixture::new(false);
    fixture.post_key(WM_KEYDOWN, 0xA7, false);
    during_peek(Command {
        word: 2,
        ..Command::back(fixture.hwnd())
    });
    let keys = fixture.keys();
    assert_eq!(keys.len(), 1);
    assert_eq!(keys[0].virtual_key, 0xA7);
    assert_eq!(keys[0].scan_code, 0x69);
}

#[test]
fn browser_key_echo_repeat_presses_each_own_one_echo() {
    for message in [WM_KEYDOWN, WM_SYSKEYDOWN] {
        let fixture = Fixture::new(false);
        fixture.post_key(message, 0xA6, false);
        fixture.post_key(message, 0xA6, true);
        during_peek(Command::back(fixture.hwnd()));
        during_peek(Command::back(fixture.hwnd()));
        let keys = fixture.keys();
        assert_eq!(keys.len(), 2);
        assert!(!keys[0].repeat);
        assert!(keys[1].repeat);
    }
}

#[test]
fn browser_key_echo_after_dispatched_keyup_is_suppressed() {
    for (down, up) in [(WM_KEYDOWN, WM_KEYUP), (WM_SYSKEYDOWN, WM_SYSKEYUP)] {
        let fixture = Fixture::new(false);
        fixture.post_key(down, 0xA6, false);
        fixture.post_key(up, 0xA6, false);
        during_peek(Command::back(fixture.hwnd()));
        let events = fixture.drain();
        assert_eq!(
            events
                .iter()
                .filter(|e| matches!(e.event, NativeVideoWindowEvent::KeyDown(_)))
                .count(),
            1
        );
        assert_eq!(
            events
                .iter()
                .filter(|e| matches!(e.event, NativeVideoWindowEvent::KeyUp(_)))
                .count(),
            1
        );
    }
}

#[test]
fn browser_key_echo_two_nonrepeat_presses_each_own_one_echo() {
    let fixture = Fixture::new(false);
    fixture.post_key(WM_KEYDOWN, 0xA6, false);
    fixture.post_key(WM_KEYDOWN, 0xA6, false);
    during_peek(Command::back(fixture.hwnd()));
    during_peek(Command::back(fixture.hwnd()));
    let keys = fixture.keys();
    assert_eq!(keys.len(), 2);
    assert!(keys.iter().all(|key| !key.repeat && key.scan_code == 0x6A));
}

#[test]
fn browser_key_echo_no_prior_press_and_keyup_only_route_once() {
    for keyup_only in [false, true] {
        let fixture = Fixture::new(false);
        if keyup_only {
            fixture.post_key(WM_KEYUP, 0xA6, false);
        }
        during_peek(Command::back(fixture.hwnd()));
        assert_synthesized(&fixture.keys(), 0xA6, "during_peek");
    }
}

#[test]
fn browser_key_echo_mouse_and_oem_devices_route_once() {
    for device in [0x8000, 0x1000] {
        for prior_press in [false, true] {
            let fixture = Fixture::new(false);
            if prior_press {
                fixture.press(0xA6);
            }
            during_peek(Command {
                word: device | 1,
                ..Command::back(fixture.hwnd())
            });
            assert_synthesized(&fixture.keys(), 0xA6, "during_peek");
        }
    }
}

#[test]
fn browser_key_echo_queued_appcommand_routes_once() {
    let fixture = Fixture::new(false);
    fixture.press(0xA6);
    let command = Command::back(fixture.hwnd());
    unsafe {
        PostMessageW(
            Some(command.receiver),
            WM_APPCOMMAND,
            WPARAM(command.source.0 as usize),
            command.lparam(),
        )
        .unwrap();
    }
    assert!(!pump_thread_messages_inner(None));
    assert_synthesized(&fixture.keys(), 0xA6, "queued");
}

#[test]
fn browser_key_echo_same_thread_send_outside_pump_routes_once() {
    let fixture = Fixture::new(false);
    fixture.press(0xA6);
    assert_eq!(Command::back(fixture.hwnd()).send(), LRESULT(1));
    assert_synthesized(&fixture.keys(), 0xA6, "unknown");
}

#[test]
fn browser_key_echo_cross_thread_send_routes_once_while_owner_pumps() {
    let fixture = Fixture::new(false);
    fixture.press(0xA6);
    let hwnd = fixture.hwnd().0 as usize;
    let (tx, rx) = std::sync::mpsc::channel();
    let sender = std::thread::spawn(move || {
        // Bound a failing test without ever joining a sender blocked on USER32.
        use windows::Win32::UI::WindowsAndMessaging::{
            SMTO_ABORTIFHUNG, SMTO_BLOCK, SendMessageTimeoutW,
        };
        let mut result = 0;
        let sent = unsafe {
            SendMessageTimeoutW(
                HWND(hwnd as *mut _),
                WM_APPCOMMAND,
                WPARAM(hwnd),
                LPARAM(1 << 16),
                SMTO_ABORTIFHUNG | SMTO_BLOCK,
                5000,
                Some(&mut result),
            )
        };
        let _ = tx.send((sent.0 != 0, result));
    });
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let (sent, result) = loop {
        assert!(!pump_thread_messages_inner(None));
        match rx.try_recv() {
            Ok(result) => break result,
            Err(std::sync::mpsc::TryRecvError::Empty) => {}
            Err(error) => panic!("sender disconnected: {error}"),
        }
        assert!(
            std::time::Instant::now() < deadline,
            "cross-thread send timed out"
        );
        sleep_until_message(1);
    };
    sender.join().unwrap();
    assert!(sent);
    assert_eq!(result, 1);
    assert_synthesized(&fixture.keys(), 0xA6, "sent_cross_thread");
}

#[test]
fn browser_key_echo_nested_in_presenter_wndproc_routes_once() {
    let fixture = Fixture::new(false);
    fixture.press(0xA6);
    // DefWindowProc(WM_WINDOWPOSCHANGED) synchronously sends WM_SIZE. The
    // CALLWNDPROC hook sends the command while that presenter scope is alive,
    // all inside the GETMESSAGE hook / real PeekMessageW call.
    during_peek(Command {
        nested: true,
        ..Command::back(fixture.hwnd())
    });
    assert_synthesized(&fixture.keys(), 0xA6, "nested");
}

#[test]
fn browser_key_echo_opposite_direction_routes_once() {
    let fixture = Fixture::new(false);
    fixture.press(0xA6);
    during_peek(Command {
        word: 2,
        ..Command::back(fixture.hwnd())
    });
    assert_synthesized(&fixture.keys(), 0xA7, "during_peek");
}

#[test]
fn browser_key_echo_different_source_hwnd_routes_once() {
    let fixture = Fixture::new(true);
    fixture.press(0xA6);
    during_peek(Command {
        source: HWND(fixture.host.hud_hwnd() as *mut _),
        ..Command::back(fixture.hwnd())
    });
    assert_synthesized(&fixture.keys(), 0xA6, "during_peek");
}

#[test]
fn browser_key_echo_different_presenter_hwnd_routes_once() {
    let first = Fixture::new(false);
    let second = Fixture::new(false);
    first.press(0xA6);
    during_peek(Command::back(second.hwnd()));
    assert_synthesized(&second.keys(), 0xA6, "during_peek");
    assert!(first.keys().is_empty());
}

#[test]
fn browser_key_echo_raw_xbutton_down_up_still_emit_one_press() {
    for button in [1, 2] {
        let fixture = Fixture::new(false);
        unsafe {
            assert_eq!(
                SendMessageW(
                    fixture.hwnd(),
                    WM_XBUTTONDOWN,
                    Some(WPARAM(button << 16)),
                    Some(LPARAM(0))
                ),
                LRESULT(1)
            );
            assert_eq!(
                SendMessageW(
                    fixture.hwnd(),
                    WM_XBUTTONUP,
                    Some(WPARAM(button << 16)),
                    Some(LPARAM(0))
                ),
                LRESULT(1)
            );
        }
        let buttons: Vec<_> = fixture
            .drain()
            .into_iter()
            .filter_map(|e| match e.event {
                NativeVideoWindowEvent::MouseButton(button) => Some(button),
                _ => None,
            })
            .collect();
        assert_eq!(buttons.len(), 2);
        assert!(buttons[0].down);
        assert!(!buttons[1].down);
        assert_eq!(
            buttons[0].button,
            if button == 1 {
                NativeVideoMouseButton::Extra1
            } else {
                NativeVideoMouseButton::Extra2
            }
        );
    }
}

#[test]
fn browser_key_echo_cap_drops_oldest_and_suppresses_exactly_64() {
    for (vk, word) in [(0xA6, 1), (0xA7, 2)] {
        let fixture = Fixture::new(false);
        for _ in 0..65 {
            fixture.post_key(WM_KEYDOWN, vk, false);
        }
        assert!(!pump_thread_messages_inner(None));
        let keys = fixture.keys();
        assert_eq!(keys.len(), 65);
        let index = (vk - 0xA6) as usize;
        let pending = &window_state(fixture.hwnd())
            .unwrap()
            .unpaired_browser_presses[index];
        assert_eq!(pending.len(), 64);
        assert_eq!(
            pending.front().copied(),
            Some(keys[1].receipt.input_receipt_id)
        );
        assert_eq!(
            pending.back().copied(),
            Some(keys[64].receipt.input_receipt_id)
        );
        for paired in keys.iter().skip(1) {
            assert_eq!(
                window_state(fixture.hwnd())
                    .unwrap()
                    .unpaired_browser_presses[index]
                    .front()
                    .copied(),
                Some(paired.receipt.input_receipt_id)
            );
            during_peek(Command {
                word,
                ..Command::back(fixture.hwnd())
            });
        }
        assert!(fixture.keys().is_empty());
        during_peek(Command {
            word,
            ..Command::back(fixture.hwnd())
        });
        assert_synthesized(&fixture.keys(), vk as u32, "during_peek");
    }
}

#[test]
fn browser_key_echo_fifo_dies_with_presenter_hwnd() {
    let first = Fixture::new(false);
    first.press(0xA6);
    let destroyed = first.hwnd();
    drop(first);
    assert!(!unsafe { IsWindow(Some(destroyed)) }.as_bool());
    let replacement = Fixture::new(false);
    during_peek(Command::back(replacement.hwnd()));
    assert_synthesized(&replacement.keys(), 0xA6, "during_peek");
}

#[test]
fn browser_key_echo_hud_appcommand_after_presenter_press_routes_once() {
    let fixture = Fixture::new(true);
    fixture.press(0xA6);
    let hud = HWND(fixture.host.hud_hwnd() as *mut _);
    during_peek(Command::back(hud));
    let events = fixture.drain();
    let keys: Vec<_> = events
        .iter()
        .filter_map(|e| match &e.event {
            NativeVideoWindowEvent::KeyDown(key) => {
                assert_eq!(e.source, NativeVideoWindowSource::Hud);
                Some(*key)
            }
            _ => None,
        })
        .collect();
    assert_synthesized(&keys, 0xA6, "during_peek");
}
