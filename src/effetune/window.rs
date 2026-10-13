//! Main-window events used only by the unowned EffeTune editor.
//! No IPC or plugin work runs in the native window procedure.

use std::cell::Cell;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, mpsc};

use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::ScreenToClient;
use windows::Win32::UI::Input::Pointer::{
    GetPointerDeviceRects, GetPointerFrameInfoHistory, POINTER_FLAG_DOWN, POINTER_FLAG_PRIMARY,
    POINTER_FLAG_UP, POINTER_INFO,
};
use windows::Win32::UI::Input::Touch::{
    GetTouchInputInfo, HTOUCHINPUT, TOUCHEVENTF_DOWN, TOUCHEVENTF_PRIMARY, TOUCHEVENTF_UP,
    TOUCHINPUT,
};
use windows::Win32::UI::Input::{
    GetCurrentInputMessageSource, IMDT_MOUSE, IMDT_UNAVAILABLE, INPUT_MESSAGE_SOURCE,
};
use windows::Win32::UI::Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass};
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, GetForegroundWindow, GetMessageExtraInfo, GetMessagePos, GetMessageTime, HHOOK,
    HTCLIENT, IsIconic, IsWindowVisible, MSG, PM_REMOVE, SetWindowsHookExW, UnhookWindowsHookEx,
    WA_ACTIVE, WA_CLICKACTIVE, WA_INACTIVE, WH_GETMESSAGE, WM_ACTIVATE, WM_CANCELMODE,
    WM_LBUTTONDBLCLK, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MBUTTONDBLCLK, WM_MBUTTONDOWN,
    WM_MOUSEACTIVATE, WM_NCDESTROY, WM_NCLBUTTONDBLCLK, WM_NCLBUTTONDOWN, WM_NCMBUTTONDBLCLK,
    WM_NCMBUTTONDOWN, WM_NCRBUTTONDBLCLK, WM_NCRBUTTONDOWN, WM_NCXBUTTONDBLCLK, WM_NCXBUTTONDOWN,
    WM_POINTERACTIVATE, WM_POINTERDOWN, WM_POINTERUP, WM_POINTERUPDATE, WM_RBUTTONDBLCLK,
    WM_RBUTTONDOWN, WM_SHOWWINDOW, WM_SIZE, WM_TOUCH, WM_WINDOWPOSCHANGED, WM_XBUTTONDBLCLK,
    WM_XBUTTONDOWN,
};

use super::{HostCommand, gui_gate::GuiGate};

const SUBCLASS_ID: usize = 0x4546_4645;

mod press_capture;
mod press_journal;
use press_capture::{CaptureWorker, PacketKey};
pub(crate) use press_capture::{ForegroundFact, ForegroundRelation};
use press_journal::InputOwner;

thread_local! {
    // Only this UI thread calls the registration's hook/subclass. The subclass
    // owns the Arc until NCDESTROY and retires the hook before releasing it.
    static INPUT_OBSERVER: Cell<*const MainWindowObserver> = const { Cell::new(std::ptr::null()) };
}

enum InputRegistration {
    Uninstalled,
    Unavailable {
        hwnd: u64,
    },
    Installed {
        hwnd: u64,
        hook: usize,
        capture: CaptureWorker,
    },
}

impl Drop for InputRegistration {
    fn drop(&mut self) {
        if let Self::Installed { hook, .. } = self {
            unsafe {
                let _ = UnhookWindowsHookEx(HHOOK(*hook as *mut _));
            }
            // CaptureWorker::drop signals stop; no UI join.
        }
    }
}

impl InputRegistration {
    fn hwnd(&self) -> Option<u64> {
        match self {
            Self::Installed { hwnd, .. } | Self::Unavailable { hwnd } => Some(*hwnd),
            Self::Uninstalled => None,
        }
    }
}

fn known_window(hwnd: u64) -> ForegroundFact {
    if hwnd == 0 {
        ForegroundFact::Unknown
    } else {
        ForegroundFact::Window(hwnd)
    }
}

fn client_location_bits(hwnd: HWND, x: f64, y: f64) -> Option<[u64; 2]> {
    // Same operation order and floor/fract convention as winit's Windows
    // pointer producer. These are packet location bits, not a HWND heuristic.
    let mut point = POINT {
        x: x.floor() as i32,
        y: y.floor() as i32,
    };
    if !unsafe { ScreenToClient(hwnd, &mut point) }.as_bool() {
        return None;
    }
    Some([
        (point.x as f64 + x.fract()).to_bits(),
        (point.y as f64 + y.fract()).to_bits(),
    ])
}

fn pointer_location_bits(hwnd: HWND, info: &POINTER_INFO) -> Option<[u64; 2]> {
    let mut device = RECT::default();
    let mut display = RECT::default();
    unsafe { GetPointerDeviceRects(info.sourceDevice, &mut device, &mut display) }.ok()?;
    let ratio_x = (display.right - display.left) as f64 / (device.right - device.left) as f64;
    let ratio_y = (display.bottom - display.top) as f64 / (device.bottom - device.top) as f64;
    let x = display.left as f64 + info.ptHimetricLocation.x as f64 * ratio_x;
    let y = display.top as f64 + info.ptHimetricLocation.y as f64 * ratio_y;
    client_location_bits(hwnd, x, y)
}

fn legacy_touch_location_bits(hwnd: HWND, input: &TOUCHINPUT) -> Option<[u64; 2]> {
    let mut point = POINT {
        x: input.x / 100,
        y: input.y / 100,
    };
    if !unsafe { ScreenToClient(hwnd, &mut point) }.as_bool() {
        return None;
    }
    Some([
        (point.x as f64 + (input.x % 100) as f64 / 100.0).to_bits(),
        (point.y as f64 + (input.y % 100) as f64 / 100.0).to_bits(),
    ])
}

fn current_packet_key() -> PacketKey {
    let point = unsafe { GetMessagePos() };
    PacketKey {
        time: unsafe { GetMessageTime() } as u32,
        x: (point as i16) as i32,
        y: ((point >> 16) as i16) as i32,
        extra: unsafe { GetMessageExtraInfo() }.0 as usize,
    }
}

fn promoted_mouse() -> bool {
    // Microsoft's documented pen/touch mouse-promotion signature, not a timing
    // heuristic. Touch contacts retain their own native producer.
    (unsafe { GetMessageExtraInfo() }.0 as u32 & 0xffff_ff00) == 0xff51_5700
}

unsafe extern "system" fn input_queue_hook(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code == 0 {
        INPUT_OBSERVER.with(|slot| {
            let pointer = slot.get();
            if !pointer.is_null() {
                let observer = unsafe { &*pointer };
                let msg = unsafe { &*(lparam.0 as *const MSG) };
                observer.drain_capture();
                let main = match &*observer.registration.lock().unwrap() {
                    InputRegistration::Installed { hwnd, .. } => *hwnd,
                    _ => 0,
                };
                let down = (wparam.0 == PM_REMOVE.0 as usize
                    && msg.hwnd.0 as u64 == main
                    && matches!(msg.message, WM_LBUTTONDOWN | WM_LBUTTONDBLCLK))
                .then(|| PacketKey {
                    time: msg.time,
                    x: msg.pt.x,
                    y: msg.pt.y,
                    // Read extra info only at genuine input dispatch, not from
                    // an ambient accessor while GetMessage is still returning.
                    extra: 0,
                });
                observer.input.lock().unwrap().retrieved(down);
            }
        });
    }
    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}

#[derive(Debug, Default, Clone, Copy)]
enum ClickForeground {
    #[default]
    None,
    Activating(u64),
    Pressed(u64),
    Released(u64),
    // Consumed or cancelled gestures cannot be reopened by a late activation.
    Finished,
}

// Diagnostic observations only; these do not supply missing foreground facts or
// alter the click reducer. Enable before startup with MIV_EFFETUNE_INPUT_TRACE=1.
#[derive(Clone, Copy, PartialEq, Eq)]
struct ObservedHwnd(u64);

impl std::fmt::Debug for ObservedHwnd {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "0x{:x}", self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ObservedActivation {
    Inactive,
    Active,
    ClickActive,
    Other(u16),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ObservedLeftButton {
    Down,
    DoubleClick,
    Up,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NativeInputMessage {
    MouseActivate {
        top_level: ObservedHwnd,
        hit_test: u16,
        button_message: u16,
    },
    Activate {
        activation: ObservedActivation,
        minimized: bool,
        other: ObservedHwnd,
    },
    LeftButton {
        button: ObservedLeftButton,
        keys: u16,
        client_point: (i16, i16),
    },
    CancelMode,
}

impl NativeInputMessage {
    fn decode(message: u32, wparam: usize, lparam: isize) -> Option<Self> {
        Some(match message {
            WM_MOUSEACTIVATE => Self::MouseActivate {
                top_level: ObservedHwnd(wparam as u64),
                hit_test: lparam as u16,
                button_message: ((lparam as u32 >> 16) & 0xffff) as u16,
            },
            WM_ACTIVATE => Self::Activate {
                activation: match (wparam & 0xffff) as u32 {
                    WA_INACTIVE => ObservedActivation::Inactive,
                    WA_ACTIVE => ObservedActivation::Active,
                    WA_CLICKACTIVE => ObservedActivation::ClickActive,
                    other => ObservedActivation::Other(other as u16),
                },
                minimized: (wparam >> 16) & 0xffff != 0,
                other: ObservedHwnd(lparam as u64),
            },
            WM_LBUTTONDOWN | WM_LBUTTONDBLCLK | WM_LBUTTONUP => Self::LeftButton {
                button: match message {
                    WM_LBUTTONDOWN => ObservedLeftButton::Down,
                    WM_LBUTTONDBLCLK => ObservedLeftButton::DoubleClick,
                    _ => ObservedLeftButton::Up,
                },
                keys: wparam as u16,
                client_point: (lparam as i16, ((lparam as u32 >> 16) & 0xffff) as i16),
            },
            WM_CANCELMODE => Self::CancelMode,
            _ => return None,
        })
    }
}

enum ClickEvent {
    Activating(u64),
    ClickActivated(u64),
    Down(u64),
    Up,
    Cancel,
}

impl ClickForeground {
    fn event(&mut self, event: ClickEvent) {
        match event {
            ClickEvent::Activating(before) => *self = Self::Activating(before),
            // Cross-input-queue WM_ACTIVATE is asynchronous. It must not
            // downgrade a press (or its release awaiting egui's click consumer).
            ClickEvent::ClickActivated(before) if matches!(self, Self::None) => {
                *self = Self::Activating(before);
            }
            ClickEvent::ClickActivated(_) => {}
            ClickEvent::Down(foreground) => {
                let before = match *self {
                    Self::Activating(before) | Self::Pressed(before) => before,
                    Self::None | Self::Released(_) | Self::Finished => foreground,
                };
                *self = Self::Pressed(before);
            }
            ClickEvent::Up => {
                if let Self::Pressed(before) = *self {
                    *self = Self::Released(before);
                } else if matches!(self, Self::Activating(_)) {
                    *self = Self::Finished;
                }
            }
            ClickEvent::Cancel => *self = Self::Finished,
        }
    }

    fn take(&mut self, foreground: u64, pointer_click: bool) -> u64 {
        match std::mem::replace(self, Self::Finished) {
            Self::Pressed(before) | Self::Released(before) if pointer_click => before,
            _ => foreground,
        }
    }
}

fn mouse_click_event(
    message: u32,
    wparam: usize,
    lparam: isize,
    foreground: impl FnOnce() -> u64,
) -> Option<ClickEvent> {
    Some(match message {
        WM_MOUSEACTIVATE
            if matches!(
                (lparam as u32 >> 16) & 0xffff,
                WM_LBUTTONDOWN | WM_LBUTTONDBLCLK
            ) && (lparam as u32 & 0xffff) == HTCLIENT =>
        {
            ClickEvent::Activating(foreground())
        }
        WM_LBUTTONDOWN | WM_LBUTTONDBLCLK => ClickEvent::Down(foreground()),
        WM_LBUTTONUP => ClickEvent::Up,
        WM_ACTIVATE if wparam & 0xffff == WA_CLICKACTIVE as usize => {
            ClickEvent::ClickActivated(lparam as u64)
        }
        WM_ACTIVATE if wparam & 0xffff == WA_INACTIVE as usize => ClickEvent::Cancel,
        WM_CANCELMODE | WM_MOUSEACTIVATE | WM_NCLBUTTONDOWN | WM_NCLBUTTONDBLCLK
        | WM_RBUTTONDOWN | WM_RBUTTONDBLCLK | WM_MBUTTONDOWN | WM_MBUTTONDBLCLK
        | WM_XBUTTONDOWN | WM_XBUTTONDBLCLK | WM_NCRBUTTONDOWN | WM_NCRBUTTONDBLCLK
        | WM_NCMBUTTONDOWN | WM_NCMBUTTONDBLCLK | WM_NCXBUTTONDOWN | WM_NCXBUTTONDBLCLK => {
            ClickEvent::Cancel
        }
        _ => return None,
    })
}

pub(super) struct MainWindowObserver {
    notify: mpsc::Sender<HostCommand>,
    click: Mutex<ClickForeground>,
    input: Mutex<InputOwner>,
    registration: Mutex<InputRegistration>,
    failure_reported: AtomicBool,
    gate: Result<Arc<GuiGate>, String>,
    input_trace: bool,
    trace_sequence: AtomicU64,
}

impl MainWindowObserver {
    pub fn new(notify: mpsc::Sender<HostCommand>) -> Self {
        Self {
            gate: GuiGate::create(Some(notify.clone())),
            notify,
            click: Mutex::new(ClickForeground::default()),
            input: Mutex::new(InputOwner::default()),
            registration: Mutex::new(InputRegistration::Uninstalled),
            failure_reported: AtomicBool::new(false),
            input_trace: std::env::var("MIV_EFFETUNE_INPUT_TRACE").ok().as_deref() == Some("1"),
            trace_sequence: AtomicU64::new(0),
        }
    }

    pub fn gate(&self) -> Result<Arc<GuiGate>, String> {
        self.gate.clone()
    }

    fn notify_visibility(&self) {
        let _ = self.notify.send(HostCommand::ReconcileVisibility);
    }
    pub fn install(self: &Arc<Self>, hwnd: u64) {
        {
            let mut registration = self.registration.lock().unwrap();
            if registration.hwnd() == Some(hwnd) {
                return; // Also failed capture install: no retry or extra Arc.
            }
            if let Some(previous) = registration.hwnd() {
                // This registration owns exactly one subclass Arc. Retire it
                // through ownership instead of a new common-controls API.
                if unsafe {
                    RemoveWindowSubclass(HWND(previous as *mut _), Some(subclass), SUBCLASS_ID)
                }
                .as_bool()
                {
                    unsafe {
                        drop(Arc::from_raw(Arc::as_ptr(self)));
                    }
                }
            }
            *registration = InputRegistration::Uninstalled;
        }
        *self.input.lock().unwrap() = InputOwner::default();
        *self.click.lock().unwrap() = ClickForeground::default();
        self.failure_reported.store(false, Ordering::Relaxed);
        if let Ok(gate) = &self.gate {
            let hwnd = HWND(hwnd as *mut _);
            gate.set_auto_factor(
                super::gui_gate::AutoSuppression::RootHidden,
                !unsafe { IsWindowVisible(hwnd) }.as_bool(),
            );
            gate.set_auto_factor(
                super::gui_gate::AutoSuppression::Minimized,
                unsafe { IsIconic(hwnd) }.as_bool(),
            );
        }
        // The subclass owns one strong reference until WM_NCDESTROY.
        let state = Arc::into_raw(Arc::clone(self));
        let ok = unsafe {
            SetWindowSubclass(
                HWND(hwnd as *mut _),
                Some(subclass),
                SUBCLASS_ID,
                state as usize,
            )
            .as_bool()
        };
        if !ok {
            unsafe { drop(Arc::from_raw(state)) };
            crate::logger::log("[EffeTune] main window observer install failed");
        } else {
            INPUT_OBSERVER.with(|slot| slot.set(state));
            *self.registration.lock().unwrap() = InputRegistration::Unavailable { hwnd };
            let started = press_capture::start(hwnd).and_then(|capture| {
                unsafe {
                    SetWindowsHookExW(
                        WH_GETMESSAGE,
                        Some(input_queue_hook),
                        None,
                        windows::Win32::System::Threading::GetCurrentThreadId(),
                    )
                }
                .map(|hook| InputRegistration::Installed {
                    hwnd,
                    hook: hook.0 as usize,
                    capture,
                })
                .map_err(|error| format!("input queue hook: {error}"))
            });
            match started {
                Ok(registration) => *self.registration.lock().unwrap() = registration,
                Err(error) => {
                    crate::logger::log(format!("[EffeTune] input observer unavailable: {error}"))
                }
            }
        }
        if ok && self.input_trace {
            crate::logger::log(format!(
                "[EffeTune input] installed hwnd={:?} foreground={:?}",
                ObservedHwnd(hwnd),
                ObservedHwnd(unsafe { GetForegroundWindow() }.0 as u64)
            ));
        }
    }

    pub fn minimize_sequence(&self) -> u64 {
        self.gate
            .as_ref()
            .map_or(0, |gate| gate.minimized_sequence())
    }

    fn drain_capture(&self) {
        let registration = self.registration.lock().unwrap();
        if let InputRegistration::Installed { capture, .. } = &*registration {
            let mut input = self.input.lock().unwrap();
            if capture.failed() {
                input.journal.invalidate();
            }
            while let Ok(record) = capture.try_recv() {
                input.journal.insert(record);
            }
        }
    }

    pub fn begin_input(&self, raw: &egui::RawInput) {
        if raw.viewport_id != egui::ViewportId::ROOT {
            return;
        }
        self.drain_capture();
        self.input.lock().unwrap().begin_input(&raw.events);
    }

    pub fn native_input_events(&self, source: eframe::NativeInputSource, events: &[egui::Event]) {
        self.input.lock().unwrap().input_events(source, events);
    }

    pub fn take_failure_notice(&self) -> bool {
        let registration = self.registration.lock().unwrap();
        let failed = match &*registration {
            InputRegistration::Unavailable { .. } => true,
            InputRegistration::Installed { capture, .. } => {
                capture.failed() || self.input.lock().unwrap().journal.lost()
            }
            InputRegistration::Uninstalled => false,
        };
        failed && !self.failure_reported.swap(true, Ordering::Relaxed)
    }

    fn touch_down(&self, id: u64, primary: bool) {
        let current = unsafe { GetForegroundWindow() }.0 as u64;
        let before = if primary {
            let mut click = self.click.lock().unwrap();
            click.event(ClickEvent::Down(current));
            match *click {
                ClickForeground::Pressed(before) => before,
                _ => 0,
            }
        } else {
            current
        };
        self.input
            .lock()
            .unwrap()
            .touch_down(id, known_window(before));
    }

    fn touch_capture_failed(&self) {
        let mut input = self.input.lock().unwrap();
        let first_failure = !input.journal.lost();
        input.invalidate_capture();
        if first_failure {
            crate::logger::log(
                "[EffeTune] native contact capture unavailable; input provenance is unknown until restart",
            );
        }
    }

    pub fn click_foreground(
        &self,
        pointer_click: bool,
        release_index: Option<usize>,
    ) -> ForegroundFact {
        let foreground = unsafe { GetForegroundWindow() }.0 as u64;
        let captured = if pointer_click {
            self.input.lock().unwrap().clicked(release_index)
        } else {
            self.click.lock().unwrap().event(ClickEvent::Cancel);
            self.input.lock().unwrap().cancel();
            known_window(foreground)
        };
        crate::logger::log(format!(
            "[EffeTune GUI] button input pointer={pointer_click} release={release_index:?} captured={captured:?} current=0x{foreground:x}"
        ));
        captured
    }
}

unsafe extern "system" fn subclass(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _id: usize,
    data: usize,
) -> LRESULT {
    let observer = unsafe { &*(data as *const MainWindowObserver) };
    let observation = NativeInputMessage::decode(message, wparam.0, lparam.0)
        .map(|decoded| (decoded, unsafe { GetForegroundWindow() }.0 as u64));
    let trace = if observer.input_trace {
        observation.map(|(decoded, foreground)| {
            let sequence = observer.trace_sequence.fetch_add(1, Ordering::Relaxed) + 1;
            let click = *observer.click.lock().unwrap();
            crate::logger::log(format!(
                "[EffeTune input] seq={sequence} phase=Enter hwnd={:?} message=0x{message:x} decoded={decoded:?} wparam=0x{:x} lparam=0x{:x} foreground={:?} click={click:?}",
                ObservedHwnd(hwnd.0 as u64), wparam.0, lparam.0 as usize, ObservedHwnd(foreground)
            ));
            sequence
        })
    } else {
        None
    };
    if let Some(event) = mouse_click_event(message, wparam.0, lparam.0, || {
        observation.map_or_else(
            || unsafe { GetForegroundWindow() }.0 as u64,
            |(_, foreground)| foreground,
        )
    }) {
        if matches!(event, ClickEvent::Cancel) {
            observer.input.lock().unwrap().cancel();
        }
        if !matches!(event, ClickEvent::Down(_) | ClickEvent::Up) {
            observer.click.lock().unwrap().event(event);
        }
    }
    match message {
        WM_LBUTTONDOWN | WM_LBUTTONDBLCLK => {
            let mut source = INPUT_MESSAGE_SOURCE::default();
            let input_source = unsafe { GetCurrentInputMessageSource(&mut source) }.is_ok()
                && source.deviceType != IMDT_UNAVAILABLE;
            let real_mouse = input_source && source.deviceType == IMDT_MOUSE;
            let promoted = if input_source && promoted_mouse() {
                let mut click = observer.click.lock().unwrap();
                click.event(ClickEvent::Down(unsafe { GetForegroundWindow() }.0 as u64));
                let before = match *click {
                    ClickForeground::Pressed(before) => before,
                    _ => 0,
                };
                Some(known_window(before))
            } else {
                observer.click.lock().unwrap().event(ClickEvent::Cancel);
                None
            };
            observer
                .input
                .lock()
                .unwrap()
                .mouse_down(current_packet_key(), real_mouse, promoted);
        }
        WM_LBUTTONUP => {
            let mut source = INPUT_MESSAGE_SOURCE::default();
            let genuine_input = unsafe { GetCurrentInputMessageSource(&mut source) }.is_ok()
                && source.deviceType != IMDT_UNAVAILABLE;
            observer.input.lock().unwrap().mouse_up(genuine_input);
            if promoted_mouse() {
                observer.click.lock().unwrap().event(ClickEvent::Up);
            }
        }
        WM_POINTERACTIVATE => observer.click.lock().unwrap().event(ClickEvent::Activating(
            unsafe { GetForegroundWindow() }.0 as u64,
        )),
        WM_POINTERDOWN | WM_POINTERUPDATE | WM_POINTERUP => {
            // Mirror winit's contact/history producer, but do not consume it or
            // call SkipPointerFrameMessages. PRIMARY is not egui's contact owner.
            let id = (wparam.0 & 0xffff) as u32;
            let (mut rows, mut columns) = (0_u32, 0_u32);
            if unsafe { GetPointerFrameInfoHistory(id, &mut rows, &mut columns, None) }.is_ok() {
                if let Some(count) = rows.checked_mul(columns).filter(|count| *count <= 4096) {
                    let mut history = vec![POINTER_INFO::default(); count as usize];
                    let capacity = (rows, columns);
                    if unsafe {
                        GetPointerFrameInfoHistory(
                            id,
                            &mut rows,
                            &mut columns,
                            Some(history.as_mut_ptr()),
                        )
                    }
                    .is_ok()
                        && rows <= capacity.0
                        && columns <= capacity.1
                    {
                        for info in history[..(rows * columns) as usize].iter().rev() {
                            if info.pointerFlags & POINTER_FLAG_DOWN == POINTER_FLAG_DOWN {
                                observer.touch_down(
                                    info.pointerId as u64,
                                    info.pointerFlags & POINTER_FLAG_PRIMARY
                                        == POINTER_FLAG_PRIMARY,
                                );
                            } else if info.pointerFlags & POINTER_FLAG_UP == POINTER_FLAG_UP {
                                if let Some(location) = pointer_location_bits(hwnd, info) {
                                    observer
                                        .input
                                        .lock()
                                        .unwrap()
                                        .touch_up_at(info.pointerId as u64, location);
                                } else {
                                    observer.touch_capture_failed();
                                }
                                if info.pointerFlags & POINTER_FLAG_PRIMARY == POINTER_FLAG_PRIMARY
                                {
                                    observer.click.lock().unwrap().event(ClickEvent::Up);
                                }
                            }
                        }
                    } else {
                        observer.touch_capture_failed();
                    }
                } else {
                    observer.touch_capture_failed();
                }
            } else {
                observer.touch_capture_failed();
            }
        }
        WM_TOUCH => {
            // Peek only. The downstream winit procedure consumes/closes this handle.
            let mut inputs = vec![TOUCHINPUT::default(); wparam.0 & 0xffff];
            if unsafe {
                GetTouchInputInfo(
                    HTOUCHINPUT(lparam.0 as *mut _),
                    &mut inputs,
                    std::mem::size_of::<TOUCHINPUT>() as i32,
                )
            }
            .is_ok()
            {
                for input in &inputs {
                    let primary = input.dwFlags & TOUCHEVENTF_PRIMARY == TOUCHEVENTF_PRIMARY;
                    if input.dwFlags & TOUCHEVENTF_DOWN == TOUCHEVENTF_DOWN {
                        observer.touch_down(input.dwID as u64, primary);
                    } else if input.dwFlags & TOUCHEVENTF_UP == TOUCHEVENTF_UP {
                        if let Some(location) = legacy_touch_location_bits(hwnd, input) {
                            observer
                                .input
                                .lock()
                                .unwrap()
                                .touch_up_at(input.dwID as u64, location);
                        } else {
                            observer.touch_capture_failed();
                        }
                        if primary {
                            observer.click.lock().unwrap().event(ClickEvent::Up);
                        }
                    }
                }
            } else {
                observer.touch_capture_failed();
            }
        }
        WM_SIZE => {
            if wparam.0 == windows::Win32::UI::WindowsAndMessaging::SIZE_MINIMIZED as usize {
                if let Ok(gate) = &observer.gate {
                    gate.note_minimized();
                }
            } else if let Ok(gate) = &observer.gate {
                gate.set_auto_factor(
                    super::gui_gate::AutoSuppression::Minimized,
                    unsafe { IsIconic(hwnd) }.as_bool(),
                );
            }
            let result = unsafe { DefSubclassProc(hwnd, message, wparam, lparam) };
            observer.notify_visibility();
            return result;
        }
        WM_SHOWWINDOW | WM_WINDOWPOSCHANGED => {
            if let Ok(gate) = &observer.gate {
                gate.set_auto_factor(
                    super::gui_gate::AutoSuppression::RootHidden,
                    !unsafe { IsWindowVisible(hwnd) }.as_bool(),
                );
            }
        }
        WM_NCDESTROY => unsafe {
            let mut registration = observer.registration.lock().unwrap();
            if matches!(&*registration,
                InputRegistration::Installed { hwnd: installed, .. }
                | InputRegistration::Unavailable { hwnd: installed }
                if *installed == hwnd.0 as u64)
            {
                if let Ok(gate) = &observer.gate {
                    gate.set_auto_factor(super::gui_gate::AutoSuppression::RootHidden, true);
                }
                INPUT_OBSERVER.with(|slot| {
                    if slot.get() == data as *const MainWindowObserver {
                        slot.set(std::ptr::null());
                    }
                });
                *registration = InputRegistration::Uninstalled;
            }
            drop(registration);
            let _ = RemoveWindowSubclass(hwnd, Some(subclass), SUBCLASS_ID);
            let result = DefSubclassProc(hwnd, message, wparam, lparam);
            drop(Arc::from_raw(data as *const MainWindowObserver));
            return result;
        },
        _ => {}
    }
    let result = unsafe { DefSubclassProc(hwnd, message, wparam, lparam) };
    if let Some(sequence) = trace {
        let click = *observer.click.lock().unwrap();
        crate::logger::log(format!(
            "[EffeTune input] seq={sequence} phase=Return hwnd={:?} message=0x{message:x} result={} foreground={:?} click={click:?}",
            ObservedHwnd(hwnd.0 as u64),
            result.0,
            ObservedHwnd(unsafe { GetForegroundWindow() }.0 as u64)
        ));
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::effetune::{GuiButtonAction, gui_button_action};

    #[test]
    fn keyboard_consumer_cancels_the_observers_still_pressed_mouse_fact() {
        let (sender, _receiver) = mpsc::channel();
        let observer = MainWindowObserver::new(sender);
        let key = PacketKey {
            time: 1,
            x: 2,
            y: 3,
            extra: 0,
        };
        {
            let mut input = observer.input.lock().unwrap();
            input.journal.insert(press_capture::CaptureRecord {
                key,
                foreground: ForegroundFact::Window(20),
            });
            input.retrieved(Some(key));
            input.mouse_down(key, true, None);
        }
        let _current_keyboard_foreground = observer.click_foreground(false, None);
        let release = egui::Event::PointerButton {
            pos: egui::pos2(2.0, 3.0),
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: egui::Modifiers::NONE,
        };
        let mut input = observer.input.lock().unwrap();
        input.mouse_up(true);
        input.input_events(
            eframe::NativeInputSource::PrimaryMouseRelease,
            std::slice::from_ref(&release),
        );
        input.begin_input(&[release]);
        assert!(matches!(input.clicked(Some(0)), ForegroundFact::Unknown));
    }

    #[test]
    fn native_input_trace_decodes_activation_hwnds_and_null_without_substitution() {
        assert_eq!(
            NativeInputMessage::decode(
                WM_MOUSEACTIVATE,
                0x16f2a5a,
                ((WM_LBUTTONDOWN << 16) | HTCLIENT) as isize,
            ),
            Some(NativeInputMessage::MouseActivate {
                top_level: ObservedHwnd(0x16f2a5a),
                hit_test: HTCLIENT as u16,
                button_message: WM_LBUTTONDOWN as u16,
            })
        );
        for (kind, decoded) in [
            (WA_INACTIVE, ObservedActivation::Inactive),
            (WA_ACTIVE, ObservedActivation::Active),
            (WA_CLICKACTIVE, ObservedActivation::ClickActive),
        ] {
            for other in [0, 0x51b244e] {
                assert_eq!(
                    NativeInputMessage::decode(WM_ACTIVATE, (kind | (1 << 16)) as usize, other),
                    Some(NativeInputMessage::Activate {
                        activation: decoded,
                        minimized: true,
                        other: ObservedHwnd(other as u64),
                    })
                );
            }
        }
        assert_eq!(format!("{:?}", ObservedHwnd(0)), "0x0");
        assert_eq!(format!("{:?}", ObservedHwnd(0x51b244e)), "0x51b244e");
    }

    #[test]
    fn native_input_trace_decodes_signed_client_points_and_ignores_frame_traffic() {
        for (message, button) in [
            (WM_LBUTTONDOWN, ObservedLeftButton::Down),
            (WM_LBUTTONDBLCLK, ObservedLeftButton::DoubleClick),
            (WM_LBUTTONUP, ObservedLeftButton::Up),
        ] {
            let packed = (u32::from((-5_i16) as u16) << 16) | u32::from((-10_i16) as u16);
            assert_eq!(
                NativeInputMessage::decode(message, 9, packed as isize),
                Some(NativeInputMessage::LeftButton {
                    button,
                    keys: 9,
                    client_point: (-10, -5),
                })
            );
        }
        assert_eq!(NativeInputMessage::decode(WM_SIZE, 0, 0), None);
        assert_eq!(NativeInputMessage::decode(WM_WINDOWPOSCHANGED, 0, 0), None);
    }

    fn native(
        click: &mut ClickForeground,
        message: u32,
        wparam: usize,
        lparam: isize,
        foreground: u64,
    ) {
        if let Some(event) = mouse_click_event(message, wparam, lparam, || foreground) {
            click.event(event);
        }
    }

    // Fake HWND topology: 10 main, 20 exact outer editor, 21 plugin child,
    // 30/40 nested owned popups, 50 unrelated generic VST, 60 external app.
    fn button_action(click: &mut ClickForeground, pointer: bool, visible: bool) -> GuiButtonAction {
        let foreground = click.take(10, pointer);
        let in_front = crate::video::dsp::editor_owns_foreground(
            20,
            foreground,
            |window| if window == 21 { 20 } else { window },
            |window| match window {
                40 => 30,
                30 => 20,
                _ => 0,
            },
        );
        gui_button_action(visible, in_front)
    }

    #[test]
    fn delayed_native_activation_after_down_or_up_cannot_change_hide_to_activate() {
        for before in [20, 21, 30, 40, 10, 50, 60] {
            for activation_after_up in [false, true] {
                let mut click = ClickForeground::default();
                native(
                    &mut click,
                    WM_MOUSEACTIVATE,
                    10,
                    ((WM_LBUTTONDOWN << 16) | HTCLIENT) as isize,
                    before,
                );
                native(&mut click, WM_LBUTTONDOWN, 0, 0, 10);
                if activation_after_up {
                    native(&mut click, WM_LBUTTONUP, 0, 0, 10);
                }
                // WM_ACTIVATE's lParam is deliberately different from the
                // sampled global foreground; that valid press owns the fact.
                native(&mut click, WM_ACTIVATE, WA_CLICKACTIVE as usize, 60, 10);
                native(&mut click, WM_LBUTTONUP, 0, 0, 10);
                assert_eq!(
                    button_action(&mut click, true, true),
                    if [20, 21, 30, 40].contains(&before) {
                        GuiButtonAction::Hide
                    } else {
                        GuiButtonAction::Activate
                    }
                );
            }
        }
    }

    #[test]
    fn native_double_click_activation_captures_editor_before_main_is_raised() {
        let mut click = ClickForeground::default();
        native(
            &mut click,
            WM_MOUSEACTIVATE,
            10,
            ((WM_LBUTTONDBLCLK << 16) | HTCLIENT) as isize,
            20,
        );
        native(&mut click, WM_LBUTTONDBLCLK, 0, 0, 10);
        native(&mut click, WM_LBUTTONUP, 0, 0, 10);
        assert_eq!(button_action(&mut click, true, true), GuiButtonAction::Hide);
    }

    #[test]
    fn native_click_activation_before_down_uses_deactivated_window_as_fallback() {
        let mut click = ClickForeground::default();
        native(&mut click, WM_ACTIVATE, WA_CLICKACTIVE as usize, 20, 10);
        native(&mut click, WM_LBUTTONDOWN, 0, 0, 10);
        native(&mut click, WM_LBUTTONUP, 0, 0, 10);
        assert_eq!(button_action(&mut click, true, true), GuiButtonAction::Hide);
    }

    #[test]
    fn finished_gesture_cannot_be_reopened_by_late_activation() {
        for finish in 0..4 {
            let mut click = ClickForeground::default();
            click.event(ClickEvent::Activating(20));
            if finish != 3 {
                native(&mut click, WM_LBUTTONDOWN, 0, 0, 10);
            }
            native(&mut click, WM_LBUTTONUP, 0, 0, 10);
            match finish {
                0 => assert_eq!(button_action(&mut click, true, true), GuiButtonAction::Hide),
                1 => assert_eq!(
                    button_action(&mut click, false, true),
                    GuiButtonAction::Activate
                ),
                2 => native(&mut click, WM_CANCELMODE, 0, 0, 10),
                _ => {} // Activation without a primary down ended on UP.
            }
            native(&mut click, WM_ACTIVATE, WA_CLICKACTIVE as usize, 20, 10);
            native(&mut click, WM_LBUTTONDOWN, 0, 0, 10);
            native(&mut click, WM_LBUTTONUP, 0, 0, 10);
            assert_eq!(
                button_action(&mut click, true, true),
                GuiButtonAction::Activate
            );

            // A genuine new activation still owns the next editor-front click.
            native(
                &mut click,
                WM_MOUSEACTIVATE,
                10,
                ((WM_LBUTTONDOWN << 16) | HTCLIENT) as isize,
                20,
            );
            native(&mut click, WM_LBUTTONDOWN, 0, 0, 10);
            native(&mut click, WM_LBUTTONUP, 0, 0, 10);
            assert_eq!(button_action(&mut click, true, true), GuiButtonAction::Hide);
        }
    }

    #[test]
    fn primary_pointer_legacy_touch_and_promoted_down_share_one_gesture() {
        let mut click = ClickForeground::default();
        click.event(ClickEvent::Activating(20)); // POINTERACTIVATE
        click.event(ClickEvent::Down(10)); // primary POINTERDOWN / TOUCH DOWN
        native(&mut click, WM_LBUTTONDOWN, 0, 0, 10); // if promoted by input provider
        click.event(ClickEvent::Up);
        native(&mut click, WM_LBUTTONUP, 0, 0, 10);
        assert_eq!(button_action(&mut click, true, true), GuiButtonAction::Hide);

        // An unused gesture must not contaminate a later genuine press.
        click.event(ClickEvent::Activating(20));
        click.event(ClickEvent::Down(10));
        click.event(ClickEvent::Up);
        click.event(ClickEvent::Down(10));
        click.event(ClickEvent::Up);
        assert_eq!(
            button_action(&mut click, true, true),
            GuiButtonAction::Activate
        );
    }

    #[test]
    fn cancelled_nonprimary_nonclient_and_deactivated_gestures_do_not_hide() {
        for (message, wparam, lparam) in [
            (WM_CANCELMODE, 0, 0),
            (WM_RBUTTONDOWN, 0, 0),
            (WM_NCLBUTTONDOWN, 0, 0),
            (WM_MOUSEACTIVATE, 0, ((WM_LBUTTONDOWN << 16) | 2) as isize),
            (WM_ACTIVATE, WA_INACTIVE as usize, 60),
        ] {
            let mut click = ClickForeground::default();
            click.event(ClickEvent::Activating(20));
            click.event(ClickEvent::Down(10));
            native(&mut click, message, wparam, lparam, 10);
            assert_eq!(
                button_action(&mut click, true, true),
                GuiButtonAction::Activate
            );
        }
    }

    #[test]
    fn hidden_editor_shows_and_keyboard_uses_current_foreground() {
        let mut click = ClickForeground::default();
        click.event(ClickEvent::Activating(20));
        click.event(ClickEvent::Down(10));
        click.event(ClickEvent::Up);
        assert_eq!(
            button_action(&mut click, false, true),
            GuiButtonAction::Activate
        );
        assert_eq!(
            button_action(&mut click, true, false),
            GuiButtonAction::Show
        );
    }

    #[test]
    fn size_notification_only_queues_work_without_a_bridge_reference() {
        let (tx, rx) = mpsc::channel();
        let observer = MainWindowObserver::new(tx);
        let before = observer.minimize_sequence();
        observer.gate().unwrap().note_minimized();
        observer.notify_visibility();
        assert_eq!(observer.minimize_sequence(), before + 1);
        assert!(matches!(
            rx.try_recv(),
            Ok(HostCommand::ReconcileVisibility)
        ));
        // The native notification type carries neither a bridge nor an IPC command.
        let native = include_str!("window.rs")
            .split("WM_SIZE => {")
            .nth(1)
            .unwrap()
            .split("WM_NCDESTROY")
            .next()
            .unwrap();
        assert!(!native.contains("bridge"));
        assert!(!native.contains("lock("));
        assert!(!native.contains("send_value"));
    }

    #[test]
    fn activation_click_keeps_foreground_before_main_is_raised() {
        let mut click = ClickForeground::default();
        click.event(ClickEvent::Activating(20));
        click.event(ClickEvent::Down(10));
        assert_eq!(click.take(10, true), 20);
        assert_eq!(click.take(10, true), 10);
    }

    #[test]
    fn next_press_replaces_an_unconsumed_background_click() {
        let mut click = ClickForeground::default();
        click.event(ClickEvent::Activating(20));
        click.event(ClickEvent::Down(10));
        click.event(ClickEvent::Up);
        click.event(ClickEvent::Down(10));
        assert_eq!(click.take(10, true), 10);
    }

    #[test]
    fn keyboard_action_discards_old_pointer_snapshot() {
        let mut click = ClickForeground::default();
        click.event(ClickEvent::Activating(20));
        click.event(ClickEvent::Down(10));
        assert_eq!(click.take(10, false), 10);
        assert_eq!(click.take(10, true), 10);
    }

    #[test]
    fn touch_activation_and_cross_input_presses_replace_completed_mouse_click() {
        let mut click = ClickForeground::default();
        click.event(ClickEvent::Activating(20));
        click.event(ClickEvent::Down(10)); // old mouse press, unused by toolbar
        click.event(ClickEvent::Up);
        click.event(ClickEvent::Down(10)); // primary touch press while main is already active
        assert_eq!(click.take(10, true), 10);
        click.event(ClickEvent::Activating(20)); // WM_POINTERACTIVATE / legacy click activation
        click.event(ClickEvent::Down(10)); // primary POINTERDOWN / TOUCH down
        assert_eq!(click.take(10, true), 20);
    }
}
