use std::ffi::c_void;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicPtr, AtomicU64, Ordering};

use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::ScreenToClient;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::{
    AttachThreadInput, GetCurrentProcessId, GetCurrentThreadId,
};
use windows::Win32::UI::Controls::WM_MOUSELEAVE;
use windows::Win32::UI::Input::Ime::{
    GCS_COMPSTR, GCS_RESULTSTR, IME_COMPOSITION_STRING, ISC_SHOWUICOMPOSITIONWINDOW,
    ImmGetCompositionStringW, ImmGetContext, ImmReleaseContext,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetFocus, GetKeyState, ReleaseCapture, SetCapture, TME_LEAVE, TRACKMOUSEEVENT, TrackMouseEvent,
    VK_CONTROL, VK_MENU, VK_SHIFT,
};
use windows::Win32::UI::Input::Pointer::{
    GetPointerInfo, GetPointerType, POINTER_FLAG_CANCELED, POINTER_INFO,
};
use windows::Win32::UI::Input::{GetCurrentInputMessageSource, IMDT_TOUCH, INPUT_MESSAGE_SOURCE};
use windows::Win32::UI::Shell::{DefSubclassProc, SetWindowSubclass};
use windows::Win32::UI::WindowsAndMessaging::{
    AdjustWindowRectEx, CREATESTRUCTW, CS_DBLCLKS, CS_HREDRAW, CS_VREDRAW, CW_USEDEFAULT,
    CreateWindowExW, DefWindowProcW, DispatchMessageW, GWL_STYLE, GWLP_USERDATA, GetClientRect,
    GetForegroundWindow, GetParent, GetWindowLongPtrW, GetWindowRect, GetWindowThreadProcessId,
    HTCLIENT, HWND_TOP, IDC_ARROW, IsWindow, IsWindowVisible, IsZoomed, LoadCursorW, MA_ACTIVATE,
    MA_ACTIVATEANDEAT, MSG, PM_REMOVE, POINTER_INPUT_TYPE, PT_TOUCH, PeekMessageW, PostMessageW,
    PostQuitMessage, RegisterClassW, SC_MINIMIZE, SW_HIDE, SW_SHOW, SW_SHOWNOACTIVATE,
    SWP_ASYNCWINDOWPOS, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOOWNERZORDER, SWP_NOSIZE, SWP_NOZORDER,
    SWP_SHOWWINDOW, SetWindowLongPtrW, TranslateMessage, WINDOW_EX_STYLE, WINDOWPOS, WM_APPCOMMAND,
    WM_CANCELMODE, WM_CAPTURECHANGED, WM_CHAR, WM_CLOSE, WM_DESTROY, WM_IME_COMPOSITION,
    WM_IME_ENDCOMPOSITION, WM_IME_SETCONTEXT, WM_IME_STARTCOMPOSITION, WM_KEYDOWN, WM_KEYUP,
    WM_LBUTTONDBLCLK, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MBUTTONDBLCLK, WM_MBUTTONDOWN, WM_MBUTTONUP,
    WM_MOUSEACTIVATE, WM_MOUSEMOVE, WM_MOUSEWHEEL, WM_NCCREATE, WM_NCDESTROY, WM_NULL,
    WM_POINTERCAPTURECHANGED, WM_POINTERDOWN, WM_POINTERENTER, WM_POINTERLEAVE, WM_POINTERUP,
    WM_POINTERUPDATE, WM_RBUTTONDBLCLK, WM_RBUTTONDOWN, WM_RBUTTONUP, WM_SETCURSOR, WM_SIZE,
    WM_SYSCOMMAND, WM_SYSKEYDOWN, WM_SYSKEYUP, WM_WINDOWPOSCHANGED, WM_XBUTTONDBLCLK,
    WM_XBUTTONDOWN, WM_XBUTTONUP, WNDCLASSW, WS_CHILD, WS_CLIPCHILDREN, WS_CLIPSIBLINGS,
    WS_EX_NOREDIRECTIONBITMAP, WS_OVERLAPPEDWINDOW, WS_POPUP, WS_VISIBLE,
};
use windows::core::w;

pub(crate) use super::native_touch::NativeVideoWindowSource;
use super::native_touch::{
    NativePointerTypeProbe, NativeTouchOwnership, NativeTouchOwnershipDecision,
    native_touch_followup_phase, native_touch_is_activation_tap,
    native_touch_mouse_discard_decision, native_touch_should_request_focus_claim,
};
pub use super::native_touch::{NativeVideoTouchEvent, NativeVideoTouchPhase};
use crate::touch_debug::{TouchDebugWindow, log_win32_message};

#[derive(Clone, Copy, Debug)]
pub struct NativeVideoKeyEvent {
    pub virtual_key: u32,
    pub scan_code: u16,
    pub extended: bool,
    pub shift: bool,
    pub ctrl: bool,
    pub alt: bool,
    pub repeat: bool,
}

#[derive(Clone, Debug)]
pub enum NativeVideoImeEvent {
    Enabled,
    Preedit(String),
    Commit(String),
    Disabled,
}

impl NativeVideoWindowSource {
    fn touch_debug_window(self) -> TouchDebugWindow {
        match self {
            Self::Presenter => TouchDebugWindow::Presenter,
            Self::Hud => TouchDebugWindow::Hud,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NativeCursorOwnershipEdge {
    Leave,
    CaptureLost,
    TrackingFailed,
}

#[derive(Clone, Debug)]
pub enum NativeVideoWindowEvent {
    /// 通常のウィンドウ操作 (`×` / Alt+F4 / taskbar close) による close request。
    /// App 側で viewer session close に接続し、stale `fullscreen_idx` を残さないために使う。
    /// `generation` は close を出した HWND の placement 世代 (`WindowState.generation`)。
    /// placement switch で旧 HWND が teardown されたあとに遅れて届く stale close を
    /// App 側が現世代と比較して棄却するために焼き込む。
    CloseRequested {
        generation: u64,
    },
    KeyDown(NativeVideoKeyEvent),
    KeyUp(NativeVideoKeyEvent),
    Text(char),
    Ime(NativeVideoImeEvent),
    MouseMove(NativeVideoMouseEvent),
    MouseButton(NativeVideoMouseButtonEvent),
    MouseWheel(NativeVideoMouseWheelEvent),
    MouseLeave,
    Touch(NativeVideoTouchEvent),
    /// Cursor ownership edge for the pump-owned router. This is distinct from
    /// the generic `MouseLeave` consumed by egui pointer state.
    CursorOwnership(NativeCursorOwnershipEdge),
    /// presenter HWND の `WM_WINDOWPOSCHANGED` で発火。HUD overlay HWND を
    /// presenter のジオメトリに追従させるために pump thread が消費する。
    /// UI 側には転送しない。
    GeometryChanged {
        x: i32,
        y: i32,
        w: u32,
        h: u32,
        maximized: bool,
    },
    /// HUD overlay HWND の `WM_DPICHANGED` で発火。`suggested_rect` は
    /// `WM_DPICHANGED` の lparam で渡される新 DPI 用 RECT。
    /// pump/render route が pixels_per_point 更新 + resize + 次フレーム region 再計算に使う。
    DpiChanged {
        dpi: u32,
        suggested_rect: RECT,
    },
    /// HUD overlay HWND の `WM_WINDOWPOSCHANGING` で「自分より前に別 window が
    /// 割り込みそう」を検知したときに送る raise 要求。pump が typed raise task に
    /// 内部変換する。これは best-effort safety net。
    RequestRaiseHud,
    /// HUD button-down requested a presenter foreground/focus handoff. The
    /// wndproc only enqueues this; the pump applies USER32 focus work after
    /// message dispatch returns.
    RequestFocusClaim,
    /// The OS lifetime of this HWND ended. The pump combines this with the
    /// stamped epoch and closes or loses the host without a render ack.
    Destroyed,
}

/// Generation-stamped event sent from a presenter/HUD wndproc.
#[derive(Clone, Debug)]
pub(crate) struct NativeVideoWindowEventEnvelope {
    pub(crate) sequence: u64,
    pub(crate) epoch: u64,
    pub(crate) generation: u64,
    pub(crate) source: NativeVideoWindowSource,
    pub(crate) event: NativeVideoWindowEvent,
}

const WINDOW_EVENT_LATEST_MOUSE_MOVE: usize = 0;
const WINDOW_EVENT_LATEST_GEOMETRY: usize = 1;
const WINDOW_EVENT_LATEST_DPI: usize = 2;
const WINDOW_EVENT_LATEST_RAISE: usize = 3;
const WINDOW_EVENT_LATEST_SLOTS: usize = 4;

fn native_window_event_latest_slot(event: &NativeVideoWindowEvent) -> Option<usize> {
    match event {
        NativeVideoWindowEvent::MouseMove(_) => Some(WINDOW_EVENT_LATEST_MOUSE_MOVE),
        NativeVideoWindowEvent::GeometryChanged { .. } => Some(WINDOW_EVENT_LATEST_GEOMETRY),
        NativeVideoWindowEvent::DpiChanged { .. } => Some(WINDOW_EVENT_LATEST_DPI),
        NativeVideoWindowEvent::RequestRaiseHud => Some(WINDOW_EVENT_LATEST_RAISE),
        NativeVideoWindowEvent::CloseRequested { .. }
        | NativeVideoWindowEvent::KeyDown(_)
        | NativeVideoWindowEvent::KeyUp(_)
        | NativeVideoWindowEvent::Text(_)
        | NativeVideoWindowEvent::Ime(_)
        | NativeVideoWindowEvent::MouseButton(_)
        | NativeVideoWindowEvent::MouseWheel(_)
        | NativeVideoWindowEvent::MouseLeave
        | NativeVideoWindowEvent::Touch(_)
        | NativeVideoWindowEvent::CursorOwnership(_)
        | NativeVideoWindowEvent::RequestFocusClaim
        | NativeVideoWindowEvent::Destroyed => None,
    }
}

struct NativeWindowEventRouteShared {
    next_sequence: AtomicU64,
    latest: Vec<LatestWindowEventSlot>,
    overflow_fault: Arc<AtomicBool>,
}

/// Lock-free single-value mailbox. Producer and consumer each take ownership
/// with one atomic swap, so a wndproc never waits for render-side draining.
struct LatestWindowEventSlot {
    value: AtomicPtr<NativeVideoWindowEventEnvelope>,
}

impl LatestWindowEventSlot {
    fn empty() -> Self {
        Self {
            value: AtomicPtr::new(std::ptr::null_mut()),
        }
    }

    fn publish(&self, event: NativeVideoWindowEventEnvelope) {
        let next = Box::into_raw(Box::new(event));
        let prior = self.value.swap(next, Ordering::AcqRel);
        if !prior.is_null() {
            unsafe { drop(Box::from_raw(prior)) };
        }
    }

    fn take(&self) -> Option<NativeVideoWindowEventEnvelope> {
        let value = self.value.swap(std::ptr::null_mut(), Ordering::AcqRel);
        (!value.is_null()).then(|| unsafe { *Box::from_raw(value) })
    }
}

impl Drop for LatestWindowEventSlot {
    fn drop(&mut self) {
        let value = *self.value.get_mut();
        if !value.is_null() {
            unsafe { drop(Box::from_raw(value)) };
        }
    }
}

/// Non-blocking bounded sender used directly by presenter/HUD wndprocs.
///
/// Mouse move, geometry, DPI, and HUD raise are latest-value slots. Close,
/// key, text, IME, button, wheel, touch, leave, and destroy use a bounded lossless
/// path. A full path raises an explicit session fault instead of blocking the
/// HWND owner or silently dropping input.
#[derive(Clone)]
pub(crate) struct NativeWindowEventRoute {
    lossless_tx: crossbeam_channel::Sender<NativeVideoWindowEventEnvelope>,
    shared: Arc<NativeWindowEventRouteShared>,
}

pub(crate) struct NativeWindowEventReceiver {
    lossless_rx: crossbeam_channel::Receiver<NativeVideoWindowEventEnvelope>,
    shared: Arc<NativeWindowEventRouteShared>,
}

pub(crate) fn native_window_event_route(
    lossless_capacity: usize,
    overflow_fault: Arc<AtomicBool>,
) -> (NativeWindowEventRoute, NativeWindowEventReceiver) {
    let (lossless_tx, lossless_rx) = crossbeam_channel::bounded(lossless_capacity);
    let shared = Arc::new(NativeWindowEventRouteShared {
        next_sequence: AtomicU64::new(1),
        latest: (0..WINDOW_EVENT_LATEST_SLOTS)
            .map(|_| LatestWindowEventSlot::empty())
            .collect(),
        overflow_fault,
    });
    (
        NativeWindowEventRoute {
            lossless_tx,
            shared: Arc::clone(&shared),
        },
        NativeWindowEventReceiver {
            lossless_rx,
            shared,
        },
    )
}

impl NativeWindowEventRoute {
    fn send(&self, mut envelope: NativeVideoWindowEventEnvelope) {
        envelope.sequence = self.shared.next_sequence.fetch_add(1, Ordering::Relaxed);
        if let Some(slot) = native_window_event_latest_slot(&envelope.event) {
            self.shared.latest[slot].publish(envelope);
            return;
        }
        match self.lossless_tx.try_send(envelope) {
            Ok(()) => {}
            Err(crossbeam_channel::TrySendError::Full(_))
            | Err(crossbeam_channel::TrySendError::Disconnected(_)) => {
                self.shared.overflow_fault.store(true, Ordering::Release)
            }
        }
    }
}

impl NativeWindowEventReceiver {
    pub(crate) fn drain(&self) -> Vec<NativeVideoWindowEventEnvelope> {
        let mut events: Vec<_> = self.lossless_rx.try_iter().collect();
        events.extend(self.shared.latest.iter().filter_map(|slot| slot.take()));
        events.sort_unstable_by_key(|event| event.sequence);
        events
    }
}

/// Per-HWND endpoint stored in `GWLP_USERDATA`. WndProc only decodes and
/// enqueues; the pump and render routes perform work after dispatch returns.
#[derive(Clone)]
pub(crate) struct NativeVideoWindowEventSink {
    epoch: u64,
    generation: u64,
    source: NativeVideoWindowSource,
    pump_route: NativeWindowEventRoute,
    render_route: NativeWindowEventRoute,
}

impl NativeVideoWindowEventSink {
    pub(crate) fn new(
        epoch: u64,
        generation: u64,
        source: NativeVideoWindowSource,
        pump_route: NativeWindowEventRoute,
        render_route: NativeWindowEventRoute,
    ) -> Self {
        Self {
            epoch,
            generation,
            source,
            pump_route,
            render_route,
        }
    }

    pub(crate) fn send(&self, event: NativeVideoWindowEvent) {
        let envelope = NativeVideoWindowEventEnvelope {
            sequence: 0,
            epoch: self.epoch,
            generation: self.generation,
            source: self.source,
            event,
        };
        if matches!(
            envelope.event,
            NativeVideoWindowEvent::CloseRequested { .. }
                | NativeVideoWindowEvent::GeometryChanged { .. }
                | NativeVideoWindowEvent::DpiChanged { .. }
                | NativeVideoWindowEvent::RequestRaiseHud
                | NativeVideoWindowEvent::RequestFocusClaim
                | NativeVideoWindowEvent::Destroyed
                | NativeVideoWindowEvent::MouseMove(_)
                | NativeVideoWindowEvent::MouseButton(_)
                | NativeVideoWindowEvent::MouseWheel(_)
                | NativeVideoWindowEvent::Touch(_)
                | NativeVideoWindowEvent::CursorOwnership(_)
        ) {
            self.pump_route.send(envelope.clone());
        }
        if matches!(
            envelope.event,
            NativeVideoWindowEvent::KeyDown(_)
                | NativeVideoWindowEvent::KeyUp(_)
                | NativeVideoWindowEvent::Text(_)
                | NativeVideoWindowEvent::Ime(_)
                | NativeVideoWindowEvent::MouseMove(_)
                | NativeVideoWindowEvent::MouseButton(_)
                | NativeVideoWindowEvent::MouseWheel(_)
                | NativeVideoWindowEvent::MouseLeave
                | NativeVideoWindowEvent::Touch(_)
                | NativeVideoWindowEvent::GeometryChanged { .. }
                | NativeVideoWindowEvent::DpiChanged { .. }
        ) {
            self.render_route.send(envelope);
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NativeVideoMouseButton {
    Left,
    Right,
    Middle,
    Extra1,
    Extra2,
}

#[derive(Clone, Copy, Debug)]
pub struct NativeVideoMouseEvent {
    pub x: i32,
    pub y: i32,
    pub shift: bool,
    pub ctrl: bool,
}

#[derive(Clone, Copy, Debug)]
pub struct NativeVideoMouseButtonEvent {
    pub button: NativeVideoMouseButton,
    pub down: bool,
    pub double_click: bool,
    pub x: i32,
    pub y: i32,
    pub shift: bool,
    pub ctrl: bool,
}

#[derive(Clone, Copy, Debug)]
pub struct NativeVideoMouseWheelEvent {
    pub delta: i16,
    pub x: i32,
    pub y: i32,
    pub shift: bool,
    pub ctrl: bool,
}

#[derive(Clone, Copy, Debug)]
pub enum NativeVideoWindowMode {
    Windowed {
        width: u32,
        height: u32,
    },
    /// 通常 top-level window。`rect.left/top` は希望 outer position、
    /// `rect.right/bottom` は希望 client size から導いた右下 (screen coords)。
    WindowedAt {
        rect: RECT,
    },
    Borderless {
        rect: RECT,
    },
    /// メインウィンドウのクライアント領域に重ねる子ウィンドウ (`WS_CHILD`)。
    /// `rect` は親 (= `NativeVideoWindowConfig.owner_hwnd`) のクライアント座標。
    /// in-window 動画再生 (Phase 0) で使う。
    Child {
        rect: RECT,
    },
}

pub struct NativeVideoWindowConfig {
    pub mode: NativeVideoWindowMode,
    pub owner_hwnd: u64,
    /// `false` の場合、HWND は hidden で作成し、呼び出し側が DComp 初期化後に
    /// `show_and_raise` で表示する。native fullscreen presenter の透明期間を避けるため。
    pub initially_visible: bool,
    pub activate_on_show: bool,
    pub close_on_escape: bool,
    pub(crate) event_sink: Option<NativeVideoWindowEventSink>,
    /// この HWND を生成した presenter placement 世代。placement switch で
    /// window を rebuild するたびに presenter 側で +1 され、`WM_CLOSE` が
    /// `CloseRequested { generation }` に焼き込む。App 側は「現世代より古い
    /// close」を stale として棄却する (旧 HWND teardown 由来の遅延 close 対策)。
    pub generation: u64,
}

impl NativeVideoWindowConfig {
    pub fn test_windowed(width: u32, height: u32) -> Self {
        Self {
            mode: NativeVideoWindowMode::Windowed { width, height },
            owner_hwnd: 0,
            initially_visible: true,
            activate_on_show: true,
            close_on_escape: true,
            event_sink: None,
            generation: 0,
        }
    }
}

pub(crate) struct NativeVideoWindow {
    hwnd: HWND,
    generation: u64,
    owner_thread: std::thread::ThreadId,
    _not_send_or_sync: std::marker::PhantomData<std::rc::Rc<()>>,
}

struct WindowState {
    close_on_escape: bool,
    event_sink: Option<NativeVideoWindowEventSink>,
    ime_preediting: bool,
    /// Whole-stream `PT_TOUCH` ownership for this presenter HWND. The HUD has
    /// an independent owner set in its own per-HWND `WindowState`.
    touch_ownership: NativeTouchOwnership,
    /// `NativeVideoWindowConfig.generation` の焼き込み。`WM_CLOSE` で
    /// `CloseRequested { generation }` を stamp するために保持する。
    generation: u64,
}

/// T31 (Codex P2 / 2026-05-16): クラス登録を 1 回に集約し、戻り値を見て
/// `ERROR_CLASS_ALREADY_EXISTS` 以外のエラーは propagation する。旧コードは
/// `RegisterClassW` を `create()` ごとに呼んで戻り値を捨てていたため、
/// (a) 多重起動時の競合ログが拾えない (b) 真のエラーが見過ごされる、の 2 問題
/// があった。`OnceLock<Result<(), String>>` でアトミック初期化する。
fn register_native_video_window_class() -> Result<(), String> {
    use std::sync::OnceLock;
    static REGISTER_RESULT: OnceLock<Result<(), String>> = OnceLock::new();
    REGISTER_RESULT
        .get_or_init(|| unsafe {
            let hmodule = GetModuleHandleW(None)
                .map_err(|e| format!("GetModuleHandleW for native video window class: {e:?}"))?;
            let hinstance = HINSTANCE(hmodule.0);
            let cursor = LoadCursorW(None, IDC_ARROW).ok();
            let wc = WNDCLASSW {
                style: CS_HREDRAW | CS_VREDRAW | CS_DBLCLKS,
                lpfnWndProc: Some(wnd_proc),
                hInstance: hinstance,
                hCursor: cursor.unwrap_or_default(),
                lpszClassName: w!("mIVNativeVideoWindow"),
                ..Default::default()
            };
            if RegisterClassW(&wc) == 0 {
                let err = std::io::Error::last_os_error();
                // ERROR_CLASS_ALREADY_EXISTS = 1410 は同一プロセス内で他経路から
                // 同名クラスが先に登録された場合に出る。挙動上は OK。
                if err.raw_os_error() != Some(1410) {
                    return Err(format!("RegisterClassW NativeVideoWindow: {err:?}"));
                }
            }
            Ok(())
        })
        .clone()
}

/// in-window モードの presenter child HWND。main window のサブクラスプロシージャが
/// 親リサイズ時にこの HWND を同期リサイズする。pump thread が child publish 時に
/// 登録し、host destroy 時に 0 へクリアする。
static IN_WINDOW_VIDEO_CHILD: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// pump thread から in-window child HWND を登録 / 解除する (`0` で解除)。
pub fn set_in_window_video_child(hwnd: u64) {
    IN_WINDOW_VIDEO_CHILD.store(hwnd, std::sync::atomic::Ordering::SeqCst);
}

const IN_WINDOW_RESIZE_SUBCLASS_ID: usize = 0x6D69_7631; // "miv1"

/// main window をサブクラス化し、`WM_SIZE` のたびに in-window presenter child を
/// 親クライアント領域へリサイズする。最大化 / 復元 / リサイズドラッグのすべてで
/// child が親に追従する (pump の periodic reflow と併用し、親の `WM_SIZE` を
/// 直接フックするので取りこぼし・遅延がない)。同じ `(proc, id)` の再登録は
/// `SetWindowSubclass` 側で冪等。
pub fn install_in_window_resize_subclass(main_hwnd: u64) -> bool {
    if main_hwnd == 0 {
        return false;
    }
    unsafe {
        SetWindowSubclass(
            HWND(main_hwnd as *mut _),
            Some(in_window_resize_subclass_proc),
            IN_WINDOW_RESIZE_SUBCLASS_ID,
            0,
        )
        .as_bool()
    }
}

fn detached_window_debug_enabled() -> bool {
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var_os("MIV_DETACHED_WINDOW_DEBUG").is_some())
}

/// main window サブクラスプロシージャ。`WM_SIZE` を受けたら登録済みの in-window
/// child を親クライアント領域へリサイズする。`SWP_ASYNCWINDOWPOS` で UI スレッドを
/// ブロックせずに child owner pump へ要求を post する。それ以外のメッセージは素通し。
unsafe extern "system" fn in_window_resize_subclass_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _id: usize,
    _ref_data: usize,
) -> LRESULT {
    if msg == WM_SIZE {
        let child = IN_WINDOW_VIDEO_CHILD.load(std::sync::atomic::Ordering::SeqCst);
        if child != 0 {
            let child_hwnd = HWND(child as *mut _);
            unsafe {
                // handle 再利用への防御 (Codex P2): child が生きていて、かつその親が
                // いま resize されている `hwnd` であるときだけリサイズする。
                let is_our_child = IsWindow(Some(child_hwnd)).as_bool()
                    && GetParent(child_hwnd).is_ok_and(|p| p == hwnd);
                if is_our_child {
                    let mut rc = RECT::default();
                    if GetClientRect(hwnd, &mut rc).is_ok() {
                        let w = (rc.right - rc.left).max(1);
                        let h = (rc.bottom - rc.top).max(1);
                        if detached_window_debug_enabled() {
                            crate::logger::log(format!(
                                "[detached-window-debug] placement_trace \
                                 source=in_window_resize_subclass \
                                 event=native_set_window_pos hwnd=0x{:x} parent=0x{:x} \
                                 pos=(0,0) size={}x{}",
                                child_hwnd.0 as usize, hwnd.0 as usize, w, h
                            ));
                        }
                        let _ = crate::presentation_observer::set_window_pos(
                            child_hwnd,
                            None,
                            0,
                            0,
                            w,
                            h,
                            SWP_NOACTIVATE | SWP_NOZORDER | SWP_ASYNCWINDOWPOS,
                            crate::presentation_observer::WindowRole::Presenter,
                            "in_window_resize_subclass",
                        );
                    }
                }
            }
        }
    }
    unsafe { DefSubclassProc(hwnd, msg, wparam, lparam) }
}

impl NativeVideoWindow {
    #[track_caller]
    fn assert_owner_thread(&self) {
        assert_eq!(
            std::thread::current().id(),
            self.owner_thread,
            "NativeVideoWindow operation ran on a non-owner thread"
        );
    }

    pub(crate) fn create(config: NativeVideoWindowConfig) -> Result<Self, String> {
        register_native_video_window_class()?;
        if matches!(config.mode, NativeVideoWindowMode::Child { .. }) && config.owner_hwnd == 0 {
            return Err(
                "Child native video window requires a parent HWND, but owner_hwnd is 0".to_string(),
            );
        }
        unsafe {
            let hmodule = GetModuleHandleW(None).map_err(|e| format!("GetModuleHandleW: {e:?}"))?;
            let hinstance = HINSTANCE(hmodule.0);

            let (ex_style, style, x, y, width, height, raise_on_show) = match config.mode {
                NativeVideoWindowMode::Windowed { width, height } => {
                    let mut style = WS_OVERLAPPEDWINDOW;
                    if config.initially_visible {
                        style |= WS_VISIBLE;
                    }
                    let ex_style = WINDOW_EX_STYLE::default();
                    let mut rect = RECT {
                        left: 0,
                        top: 0,
                        right: width as i32,
                        bottom: height as i32,
                    };
                    AdjustWindowRectEx(&mut rect, style, false, ex_style)
                        .map_err(|e| format!("AdjustWindowRectEx: {e:?}"))?;
                    (
                        ex_style,
                        style,
                        CW_USEDEFAULT,
                        CW_USEDEFAULT,
                        rect.right - rect.left,
                        rect.bottom - rect.top,
                        false,
                    )
                }
                NativeVideoWindowMode::WindowedAt { rect } => {
                    let mut style = WS_OVERLAPPEDWINDOW;
                    if config.initially_visible {
                        style |= WS_VISIBLE;
                    }
                    let ex_style = WINDOW_EX_STYLE::default();
                    let client_w = (rect.right - rect.left).max(1);
                    let client_h = (rect.bottom - rect.top).max(1);
                    let mut outer = RECT {
                        left: 0,
                        top: 0,
                        right: client_w,
                        bottom: client_h,
                    };
                    AdjustWindowRectEx(&mut outer, style, false, ex_style)
                        .map_err(|e| format!("AdjustWindowRectEx(WindowedAt): {e:?}"))?;
                    (
                        ex_style,
                        style,
                        rect.left,
                        rect.top,
                        outer.right - outer.left,
                        outer.bottom - outer.top,
                        false,
                    )
                }
                NativeVideoWindowMode::Borderless { rect } => {
                    let mut style = WS_POPUP | WS_CLIPSIBLINGS | WS_CLIPCHILDREN;
                    if config.initially_visible {
                        style |= WS_VISIBLE;
                    }
                    let ex_style = WS_EX_NOREDIRECTIONBITMAP;
                    (
                        ex_style,
                        style,
                        rect.left,
                        rect.top,
                        rect.right - rect.left,
                        rect.bottom - rect.top,
                        true,
                    )
                }
                NativeVideoWindowMode::Child { rect } => {
                    // in-window 再生: presenter HWND を main HWND の子にする。
                    // 親クライアント座標で配置し、親に自動クリップ・自動移動させる。
                    let mut style = WS_CHILD | WS_CLIPSIBLINGS | WS_CLIPCHILDREN;
                    if config.initially_visible {
                        style |= WS_VISIBLE;
                    }
                    // 子には WS_EX_NOREDIRECTIONBITMAP を付けない (Chromium の child
                    // compositor window と同じ。DComp target は子 HWND でも作れる)。
                    let ex_style = WINDOW_EX_STYLE::default();
                    (
                        ex_style,
                        style,
                        rect.left,
                        rect.top,
                        rect.right - rect.left,
                        rect.bottom - rect.top,
                        false,
                    )
                }
            };

            let state = Box::new(WindowState {
                close_on_escape: config.close_on_escape,
                event_sink: config.event_sink,
                ime_preediting: false,
                touch_ownership: NativeTouchOwnership::default(),
                generation: config.generation,
            });
            let state_ptr = Box::into_raw(state);
            let owner_hwnd = if config.owner_hwnd != 0 {
                Some(HWND(config.owner_hwnd as *mut _))
            } else {
                None
            };
            let hwnd = match CreateWindowExW(
                ex_style,
                w!("mIVNativeVideoWindow"),
                w!("mIV Native Video Window"),
                style,
                x,
                y,
                width,
                height,
                owner_hwnd,
                None,
                Some(hinstance),
                Some(state_ptr.cast()),
            ) {
                Ok(hwnd) => hwnd,
                Err(err) => {
                    let _ = Box::from_raw(state_ptr);
                    return Err(format!("CreateWindowExW: {err:?}"));
                }
            };
            crate::presentation_observer::register(
                crate::presentation_observer::WindowRole::Presenter,
                hwnd.0 as usize as u64,
            );
            crate::dwm_transitions::disable_transitions_for_window(hwnd);
            if config.initially_visible {
                let _ = if config.activate_on_show {
                    crate::presentation_observer::show_window(
                        hwnd,
                        SW_SHOW,
                        crate::presentation_observer::WindowRole::Presenter,
                        "NativeVideoWindow::create",
                    )
                } else {
                    crate::presentation_observer::show_window(
                        hwnd,
                        SW_SHOWNOACTIVATE,
                        crate::presentation_observer::WindowRole::Presenter,
                        "NativeVideoWindow::create",
                    )
                };
            }
            if config.initially_visible && raise_on_show && config.activate_on_show {
                bring_hwnd_to_front(hwnd);
                log_window_state("created", hwnd);
            } else if !config.initially_visible {
                log_window_state("created-hidden", hwnd);
            }
            Ok(Self {
                hwnd,
                generation: config.generation,
                owner_thread: std::thread::current().id(),
                _not_send_or_sync: std::marker::PhantomData,
            })
        }
    }

    pub(crate) fn hwnd(&self) -> HWND {
        self.assert_owner_thread();
        self.hwnd
    }

    pub(crate) fn generation(&self) -> u64 {
        self.assert_owner_thread();
        self.generation
    }

    pub(crate) fn show_and_raise(&self) -> bool {
        self.assert_owner_thread();
        if self.hwnd.0.is_null() {
            return false;
        }
        let t0 = std::time::Instant::now();
        unsafe {
            if !IsWindow(Some(self.hwnd)).as_bool() {
                return false;
            }
            crate::dwm_transitions::disable_transitions_for_window(self.hwnd);
        }
        let dwm_ms = t0.elapsed().as_secs_f64() * 1000.0;
        let show_t0 = std::time::Instant::now();
        unsafe {
            let _ = crate::presentation_observer::show_window(
                self.hwnd,
                SW_SHOW,
                crate::presentation_observer::WindowRole::Presenter,
                "NativeVideoWindow::show_and_raise",
            );
        }
        let show_ms = show_t0.elapsed().as_secs_f64() * 1000.0;
        let raise_t0 = std::time::Instant::now();
        let raised = bring_hwnd_to_front(self.hwnd);
        let raise_ms = raise_t0.elapsed().as_secs_f64() * 1000.0;
        crate::logger::log(format!(
            "[native-video] show_and_raise dwm={dwm_ms:.1}ms show_window={show_ms:.1} \
             set_window_pos={raise_ms:.1}"
        ));
        log_window_state("shown", self.hwnd);
        raised
    }

    pub(crate) fn show_no_activate(&self) -> bool {
        self.assert_owner_thread();
        if self.hwnd.0.is_null() {
            return false;
        }
        let t0 = std::time::Instant::now();
        unsafe {
            if !IsWindow(Some(self.hwnd)).as_bool() {
                return false;
            }
            crate::dwm_transitions::disable_transitions_for_window(self.hwnd);
        }
        let dwm_ms = t0.elapsed().as_secs_f64() * 1000.0;
        let show_t0 = std::time::Instant::now();
        unsafe {
            let _ = crate::presentation_observer::show_window(
                self.hwnd,
                SW_SHOWNOACTIVATE,
                crate::presentation_observer::WindowRole::Presenter,
                "NativeVideoWindow::show_no_activate",
            );
        }
        let show_ms = show_t0.elapsed().as_secs_f64() * 1000.0;
        let swp_t0 = std::time::Instant::now();
        unsafe {
            let _ = crate::presentation_observer::set_window_pos(
                self.hwnd,
                Some(HWND_TOP),
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_SHOWWINDOW,
                crate::presentation_observer::WindowRole::Presenter,
                "NativeVideoWindow::show_no_activate",
            );
        }
        crate::logger::log(format!(
            "[native-video] show_no_activate dwm={dwm_ms:.1}ms show_window={show_ms:.1} \
             set_window_pos={:.1}",
            swp_t0.elapsed().as_secs_f64() * 1000.0,
        ));
        log_window_state("shown-noactivate", self.hwnd);
        true
    }

    /// ウィンドウを非表示にする (`SW_HIDE`)。破棄はせず、後で `show_and_raise` /
    /// `show_no_activate` で再表示できる (Inc 7 hidden presenter: 動画→音声モード中は
    /// presenter ウィンドウを hide して egui 音楽ビューを見せる)。
    pub(crate) fn hide(&self) -> bool {
        self.assert_owner_thread();
        if self.hwnd.0.is_null() {
            return false;
        }
        unsafe {
            if !IsWindow(Some(self.hwnd)).as_bool() {
                return false;
            }
            crate::dwm_transitions::disable_transitions_for_window(self.hwnd);
            let _ = crate::presentation_observer::show_window(
                self.hwnd,
                SW_HIDE,
                crate::presentation_observer::WindowRole::Presenter,
                "NativeVideoWindow::hide",
            );
        }
        log_window_state("hidden", self.hwnd);
        true
    }

    pub(crate) fn destroy(&mut self) {
        self.assert_owner_thread();
        if self.hwnd.0.is_null() {
            return;
        }
        unsafe {
            if IsWindow(Some(self.hwnd)).as_bool() {
                // in-window child の global 登録を DestroyWindow の **前** に解除する。
                // 「DestroyWindow 後・解除前」の窓で main の WM_SIZE が来ると、再利用
                // された handle を subclass が掴む恐れがあるため (Codex P2)。CAS で
                // 「自分の HWND のときだけ」0 にする (fullscreen window では global は
                // 別値/0 なので no-op)。
                let _ = IN_WINDOW_VIDEO_CHILD.compare_exchange(
                    self.hwnd.0 as u64,
                    0,
                    std::sync::atomic::Ordering::SeqCst,
                    std::sync::atomic::Ordering::SeqCst,
                );
                let _ = crate::presentation_observer::destroy_window(
                    self.hwnd,
                    crate::presentation_observer::WindowRole::Presenter,
                    "NativeVideoWindow::destroy",
                );
            }
        }
        crate::presentation_observer::unregister(
            crate::presentation_observer::WindowRole::Presenter,
            self.hwnd.0 as usize as u64,
        );
        self.hwnd = HWND::default();
    }
}

pub fn bring_to_front(hwnd_raw: u64) -> bool {
    if hwnd_raw == 0 {
        return false;
    }
    unsafe {
        let hwnd = HWND(hwnd_raw as *mut _);
        if !IsWindow(Some(hwnd)).as_bool() {
            return false;
        }
        bring_hwnd_to_front(hwnd)
    }
}

/// HWND owner pump のメッセージループを即時に起こすため、
/// 良性の `WM_NULL` を post する (Inc 7 hidden presenter: hide/show コマンドを
/// アイドル中の presenter に素早く反映させる)。`WM_NULL` は wndproc で `DefWindowProcW`
/// に落ちるだけなので副作用は無い。
pub fn post_wake(hwnd_raw: u64) {
    if hwnd_raw == 0 {
        return;
    }
    unsafe {
        let hwnd = HWND(hwnd_raw as *mut _);
        if !IsWindow(Some(hwnd)).as_bool() {
            return;
        }
        let _ = PostMessageW(Some(hwnd), WM_NULL, WPARAM(0), LPARAM(0));
    }
}

pub fn minimize_window(hwnd_raw: u64) -> bool {
    if hwnd_raw == 0 {
        return false;
    }
    unsafe {
        let hwnd = HWND(hwnd_raw as *mut _);
        if !IsWindow(Some(hwnd)).as_bool() {
            return false;
        }
        let ok = PostMessageW(
            Some(hwnd),
            WM_SYSCOMMAND,
            WPARAM(SC_MINIMIZE as usize),
            LPARAM(0),
        )
        .is_ok();
        log_window_state("minimize-posted", hwnd);
        ok
    }
}

pub fn foreground_belongs_to_current_process() -> bool {
    unsafe {
        let foreground = GetForegroundWindow();
        if foreground.0.is_null() {
            return true;
        }
        let mut foreground_pid = 0_u32;
        let _ = GetWindowThreadProcessId(foreground, Some(&mut foreground_pid));
        foreground_pid == 0 || foreground_pid == GetCurrentProcessId()
    }
}

pub fn foreground_hwnd() -> u64 {
    unsafe { GetForegroundWindow().0 as u64 }
}

pub fn thread_focus_hwnd() -> u64 {
    unsafe { GetFocus().0 as u64 }
}

/// `foreground_belongs_to_current_process` の保守的版。
/// foreground=null / pid=0 の不確定ケースは false を返す
/// (= 「mIV が前面と確信できない」場合は奪還しない)。
pub fn foreground_belongs_to_current_process_strict() -> bool {
    hwnd_belongs_to_current_process_strict(unsafe { GetForegroundWindow() })
}

fn hwnd_belongs_to_current_process_strict(hwnd: HWND) -> bool {
    unsafe {
        if hwnd.0.is_null() {
            return false;
        }
        let mut foreground_pid = 0_u32;
        let _ = GetWindowThreadProcessId(hwnd, Some(&mut foreground_pid));
        foreground_pid != 0 && foreground_pid == GetCurrentProcessId()
    }
}

pub fn is_window_alive(hwnd_raw: u64) -> bool {
    if hwnd_raw == 0 {
        return false;
    }
    unsafe { IsWindow(Some(HWND(hwnd_raw as *mut _))).as_bool() }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AttachSpanSamplePoint {
    BeforeAttach,
    AfterAttach,
    AfterFocus,
    AfterDetach,
}

impl AttachSpanSamplePoint {
    const fn as_str(self) -> &'static str {
        match self {
            Self::BeforeAttach => "before_attach",
            Self::AfterAttach => "after_attach",
            Self::AfterFocus => "after_focus",
            Self::AfterDetach => "after_detach",
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct AttachSpanSample {
    point: AttachSpanSamplePoint,
    modifiers: crate::modifier_probe::SidedModifierSnapshot,
}

#[derive(Debug)]
struct AttachSpanProbe {
    enabled: bool,
    this_tid: u32,
    foreground_tid: u32,
    partner_is_miv_ui_thread: bool,
    samples: Vec<AttachSpanSample>,
}

impl AttachSpanProbe {
    fn new(
        enabled: bool,
        this_tid: u32,
        foreground_tid: u32,
        partner_is_miv_ui_thread: bool,
    ) -> Self {
        Self {
            enabled,
            this_tid,
            foreground_tid,
            partner_is_miv_ui_thread,
            samples: Vec::new(),
        }
    }

    fn record(&mut self, point: AttachSpanSamplePoint) {
        self.record_with(point, crate::modifier_probe::sample_sided_modifier_snapshot);
    }

    fn record_with(
        &mut self,
        point: AttachSpanSamplePoint,
        sample: impl FnOnce() -> crate::modifier_probe::SidedModifierSnapshot,
    ) {
        if !self.enabled {
            return;
        }
        self.samples.push(AttachSpanSample {
            point,
            modifiers: sample(),
        });
    }

    fn into_fields(
        self,
        attach_ok: bool,
        detach_ok: bool,
    ) -> Option<Vec<(&'static str, serde_json::Value)>> {
        if !self.enabled {
            return None;
        }
        let samples = self
            .samples
            .into_iter()
            .map(|sample| {
                let serde_json::Value::Object(mut fields) = sample.modifiers.into_value() else {
                    unreachable!("sided modifier snapshot must serialize as an object");
                };
                fields.insert(
                    "point".to_owned(),
                    serde_json::Value::from(sample.point.as_str()),
                );
                serde_json::Value::Object(fields)
            })
            .collect();
        Some(vec![
            ("this_tid", serde_json::Value::from(self.this_tid)),
            (
                "foreground_tid",
                serde_json::Value::from(self.foreground_tid),
            ),
            (
                "partner_is_miv_ui_thread",
                serde_json::Value::from(self.partner_is_miv_ui_thread),
            ),
            ("attach_ok", serde_json::Value::from(attach_ok)),
            ("detach_ok", serde_json::Value::from(detach_ok)),
            ("samples", serde_json::Value::Array(samples)),
        ])
    }

    fn emit(self, attach_ok: bool, detach_ok: bool) {
        let Some(fields) = self.into_fields(attach_ok, detach_ok) else {
            return;
        };
        crate::perf::event(
            "native_presenter",
            "attach_thread_input_probe",
            None,
            0,
            &fields,
        );
    }
}

/// `hwnd_raw` が child window なら、現在の親 HWND を返す。
///
/// HWND の生存だけでは、detached viewer host の再生成後も presenter child が旧 host の
/// 子として残っている状態を区別できない。呼び出し側はこの実 OS 関係を現在の registry
/// owner と比較し、必要なときだけ placement switch を行う。
pub fn window_parent(hwnd_raw: u64) -> Option<u64> {
    if hwnd_raw == 0 {
        return None;
    }
    unsafe {
        GetParent(HWND(hwnd_raw as *mut _))
            .ok()
            .map(|parent| parent.0 as usize as u64)
            .filter(|parent| *parent != 0)
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct ForegroundClaimReport {
    pub foreground_hwnd: u64,
    pub post_foreground_hwnd: u64,
    pub target_hwnd: u64,
    pub attach_thread_input_ok: bool,
    pub set_foreground_ok: bool,
    pub set_active_ok: bool,
    pub set_focus_ok: bool,
}

/// `SetForegroundWindow` cooperative ルールを掻い潜って HWND を最前面に上げる。
/// Alt+Tab で他アプリが foreground を持っている状態でも安定して動く best-effort。
/// 詳細ログ用に各 API 結果を返す。
///
/// `target_hwnd_raw == 0` または `!IsWindow(target)` の場合は何もせず、
/// 全 ok=false の report を返す (共通 utility なので呼び出し側で stale HWND を
/// 渡しても安全に no-op になる)。
pub fn claim_foreground(target_hwnd_raw: u64) -> ForegroundClaimReport {
    if target_hwnd_raw == 0 || !is_window_alive(target_hwnd_raw) {
        return ForegroundClaimReport {
            target_hwnd: target_hwnd_raw,
            ..ForegroundClaimReport::default()
        };
    }
    unsafe {
        let target = HWND(target_hwnd_raw as *mut c_void);
        let foreground = GetForegroundWindow();
        let this_tid = GetCurrentThreadId();
        let foreground_tid = if !foreground.0.is_null() {
            GetWindowThreadProcessId(foreground, None)
        } else {
            0
        };
        let ui_thread_id = crate::modifier_probe::ui_thread_id();
        let mut attach_probe = AttachSpanProbe::new(
            crate::perf::is_enabled(),
            this_tid,
            foreground_tid,
            ui_thread_id != 0 && foreground_tid == ui_thread_id,
        );
        attach_probe.record(AttachSpanSamplePoint::BeforeAttach);
        let attached = foreground_tid != 0
            && foreground_tid != this_tid
            && AttachThreadInput(this_tid, foreground_tid, true).as_bool();
        if attached {
            attach_probe.record(AttachSpanSamplePoint::AfterAttach);
        }
        let set_foreground_ok = crate::presentation_observer::set_foreground_window(
            target,
            crate::presentation_observer::WindowRole::Presenter,
            "claim_foreground",
        );
        let set_active_ok = crate::presentation_observer::set_active_window(
            target,
            crate::presentation_observer::WindowRole::Presenter,
            "claim_foreground",
        )
        .is_ok();
        let set_focus_ok = crate::presentation_observer::set_focus(
            Some(target),
            crate::presentation_observer::WindowRole::Presenter,
            "claim_foreground",
        )
        .is_ok();
        attach_probe.record(AttachSpanSamplePoint::AfterFocus);
        let post_foreground = GetForegroundWindow();
        let detach_ok = attached && AttachThreadInput(this_tid, foreground_tid, false).as_bool();
        attach_probe.record(AttachSpanSamplePoint::AfterDetach);
        attach_probe.emit(attached, detach_ok);
        ForegroundClaimReport {
            foreground_hwnd: foreground.0 as u64,
            post_foreground_hwnd: post_foreground.0 as u64,
            target_hwnd: target.0 as u64,
            attach_thread_input_ok: attached,
            set_foreground_ok,
            set_active_ok,
            set_focus_ok,
        }
    }
}

fn bring_hwnd_to_front(hwnd: HWND) -> bool {
    unsafe {
        let flags = SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_NOOWNERZORDER | SWP_SHOWWINDOW;
        crate::presentation_observer::set_window_pos(
            hwnd,
            Some(HWND_TOP),
            0,
            0,
            0,
            0,
            flags,
            crate::presentation_observer::WindowRole::Presenter,
            "bring_hwnd_to_front",
        )
        .is_ok()
    }
}

pub fn log_state(hwnd_raw: u64, label: &str) {
    if hwnd_raw == 0 {
        return;
    }
    unsafe {
        let hwnd = HWND(hwnd_raw as *mut _);
        if IsWindow(Some(hwnd)).as_bool() {
            log_window_state(label, hwnd);
        }
    }
}

fn log_window_state(label: &str, hwnd: HWND) {
    unsafe {
        let mut rect = RECT::default();
        let rect_ok = GetWindowRect(hwnd, &mut rect).is_ok();
        let visible = IsWindowVisible(hwnd).as_bool();
        let foreground = GetForegroundWindow();
        crate::logger::log(format!(
            "[native-video] window {label}: hwnd=0x{:x} visible={} rect_ok={} rect=({},{} {}x{}) foreground=0x{:x}",
            hwnd.0 as usize,
            visible,
            rect_ok,
            rect.left,
            rect.top,
            rect.right - rect.left,
            rect.bottom - rect.top,
            foreground.0 as usize
        ));
        crate::perf::event(
            "native_presenter",
            "window_state",
            None,
            0,
            &[
                ("label", serde_json::Value::from(label)),
                ("hwnd", serde_json::Value::from(hwnd.0 as usize as u64)),
                ("visible", serde_json::Value::from(visible)),
                ("rect_ok", serde_json::Value::from(rect_ok)),
                ("left", serde_json::Value::from(rect.left)),
                ("top", serde_json::Value::from(rect.top)),
                ("width", serde_json::Value::from(rect.right - rect.left)),
                ("height", serde_json::Value::from(rect.bottom - rect.top)),
                (
                    "foreground",
                    serde_json::Value::from(foreground.0 as usize as u64),
                ),
            ],
        );
    }
}

impl Drop for NativeVideoWindow {
    fn drop(&mut self) {
        self.destroy();
    }
}

pub fn pump_thread_messages() -> bool {
    pump_thread_messages_inner(None)
}

pub(crate) fn pump_thread_messages_with_health(
    health: &super::native_window_health::NativeWindowHealth,
) -> bool {
    pump_thread_messages_inner(Some(health))
}

fn pump_thread_messages_inner(
    health: Option<&super::native_window_health::NativeWindowHealth>,
) -> bool {
    let mut quit = false;
    unsafe {
        let mut msg = MSG::default();
        while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
            if msg.message == super::native_window_health::NATIVE_WINDOW_HEALTH_PING {
                if let Some(health) = health {
                    health.acknowledge_pump_ping(msg.lParam.0 as u64, msg.wParam.0 as u64);
                }
                continue;
            }
            if msg.message == windows::Win32::UI::WindowsAndMessaging::WM_QUIT {
                quit = true;
                break;
            }
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
            if let Some(health) = health {
                health.record_message_dispatched();
            }
        }
    }
    quit
}

/// Ends the dedicated pump loop after its typed Shutdown reducer has destroyed
/// every pump-owned HWND. Individual HWND destruction never calls this.
pub(crate) fn post_typed_pump_quit() {
    unsafe { PostQuitMessage(0) };
}

/// `thread::sleep` の message 対応版。最大 `ms` ミリ秒待つが、呼び出しスレッドの
/// メッセージキューに何か届いた時点でタイムアウト前でも即座に返る。これにより
/// presenter スレッドのアイドル待機中に来た `WM_WINDOWPOSCHANGED` (リサイズ) 等を
/// 1 アイドル周期ぶん待たずに拾える。返ったあとは呼び出し側が通常どおりループ先頭で
/// `pump_thread_messages` する前提 (この関数自体はメッセージを除去しない)。
pub fn sleep_until_message(ms: u32) {
    use windows::Win32::UI::WindowsAndMessaging::{
        MWMO_INPUTAVAILABLE, MsgWaitForMultipleObjectsEx, QS_ALLINPUT,
    };
    unsafe {
        let _ = MsgWaitForMultipleObjectsEx(None, ms, QS_ALLINPUT, MWMO_INPUTAVAILABLE);
    }
}

fn native_touch_gestures_disabled() -> bool {
    static DISABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *DISABLED.get_or_init(|| std::env::var_os("MIV_DISABLE_TOUCH_GESTURES").is_some())
}

fn pointer_type_probe(pointer_id: u32) -> NativePointerTypeProbe {
    let mut pointer_type = POINTER_INPUT_TYPE::default();
    match unsafe { GetPointerType(pointer_id, &mut pointer_type) } {
        Ok(()) if pointer_type == PT_TOUCH => NativePointerTypeProbe::Touch,
        Ok(()) => NativePointerTypeProbe::NonTouch,
        Err(_) => NativePointerTypeProbe::Failed,
    }
}

#[derive(Clone, Copy)]
struct PointerClientInfo {
    x: i32,
    y: i32,
    cancelled: bool,
}

fn pointer_client_info(hwnd: HWND, pointer_id: u32) -> Option<PointerClientInfo> {
    let mut info = POINTER_INFO::default();
    unsafe { GetPointerInfo(pointer_id, &mut info) }.ok()?;
    let mut client = info.ptPixelLocation;
    // `ptPixelLocation` is in physical screen pixels. ScreenToClient is HWND
    // based, so the conversion remains valid when a presenter spans monitors.
    if !unsafe { ScreenToClient(hwnd, &mut client) }.as_bool() {
        return None;
    }
    Some(PointerClientInfo {
        x: client.x,
        y: client.y,
        cancelled: info.pointerFlags & POINTER_FLAG_CANCELED == POINTER_FLAG_CANCELED,
    })
}

fn send_touch_event(
    sink: Option<&NativeVideoWindowEventSink>,
    source: NativeVideoWindowSource,
    pointer_id: u32,
    position: [i32; 2],
    phase: NativeVideoTouchPhase,
    suppress_widget_primary: bool,
) {
    if let Some(sink) = sink {
        sink.send(NativeVideoWindowEvent::Touch(NativeVideoTouchEvent {
            source,
            pointer_id,
            x: position[0],
            y: position[1],
            phase,
            suppress_widget_primary,
        }));
    }
}

fn log_touch_ownership(
    source: NativeVideoWindowSource,
    pointer_id: u32,
    decision: NativeTouchOwnershipDecision,
) {
    match decision {
        NativeTouchOwnershipDecision::Owned => crate::touch_debug::log_native_touch_ownership(
            source.touch_debug_window(),
            pointer_id,
            true,
            "pt_touch_owned",
        ),
        NativeTouchOwnershipDecision::Passed(reason) => {
            crate::touch_debug::log_native_touch_ownership(
                source.touch_debug_window(),
                pointer_id,
                false,
                reason.label(),
            )
        }
    }
}

#[derive(Clone, Copy)]
struct NativeTouchPointerPolicy {
    source: NativeVideoWindowSource,
    suppress_widget_primary: bool,
    request_focus_claim: bool,
}

fn is_native_touch_pointer_message(msg: u32) -> bool {
    matches!(
        msg,
        WM_POINTERDOWN
            | WM_POINTERUPDATE
            | WM_POINTERUP
            | WM_POINTERCAPTURECHANGED
            | WM_POINTERENTER
            | WM_POINTERLEAVE
    )
}

fn handle_presenter_pointer_message(hwnd: HWND, msg: u32, wparam: WPARAM) -> Option<LRESULT> {
    if !is_native_touch_pointer_message(msg) {
        return None;
    }
    let state = window_state_mut(hwnd)?;
    if msg != WM_POINTERDOWN {
        let WindowState {
            event_sink,
            touch_ownership,
            ..
        } = state;
        return handle_native_touch_pointer_message(
            hwnd,
            msg,
            wparam,
            touch_ownership,
            event_sink.as_ref(),
            NativeTouchPointerPolicy {
                source: NativeVideoWindowSource::Presenter,
                suppress_widget_primary: false,
                request_focus_claim: false,
            },
        );
    }
    let first_owned_stream = state.touch_ownership.is_empty();
    let is_child = unsafe { (GetWindowLongPtrW(hwnd, GWL_STYLE) as u32 & WS_CHILD.0) != 0 };
    let foreground = unsafe { GetForegroundWindow() };
    let foreground_is_current_process = hwnd_belongs_to_current_process_strict(foreground);
    let presenter_is_foreground = foreground == hwnd;
    let presenter_has_thread_focus = unsafe { GetFocus() } == hwnd;
    let suppress_widget_primary = state.touch_ownership.has_suppressed_widget_stream()
        || native_touch_is_activation_tap(
            is_child,
            foreground_is_current_process,
            presenter_is_foreground,
        );
    let request_focus_claim = native_touch_should_request_focus_claim(
        first_owned_stream,
        is_child,
        foreground_is_current_process,
        presenter_is_foreground,
        presenter_has_thread_focus,
    );
    let WindowState {
        event_sink,
        touch_ownership,
        ..
    } = state;
    handle_native_touch_pointer_message(
        hwnd,
        msg,
        wparam,
        touch_ownership,
        event_sink.as_ref(),
        NativeTouchPointerPolicy {
            source: NativeVideoWindowSource::Presenter,
            suppress_widget_primary,
            request_focus_claim,
        },
    )
}

pub(crate) fn handle_hud_pointer_message(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    ownership: &mut NativeTouchOwnership,
    sink: &NativeVideoWindowEventSink,
) -> Option<LRESULT> {
    handle_native_touch_pointer_message(
        hwnd,
        msg,
        wparam,
        ownership,
        Some(sink),
        NativeTouchPointerPolicy {
            source: NativeVideoWindowSource::Hud,
            suppress_widget_primary: false,
            request_focus_claim: true,
        },
    )
}

fn handle_native_touch_pointer_message(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    ownership: &mut NativeTouchOwnership,
    sink: Option<&NativeVideoWindowEventSink>,
    policy: NativeTouchPointerPolicy,
) -> Option<LRESULT> {
    if !is_native_touch_pointer_message(msg) {
        return None;
    }
    let pointer_id = (wparam.0 & 0xffff) as u32;
    if msg != WM_POINTERDOWN {
        return handle_owned_pointer_followup(ownership, sink, policy, hwnd, msg, pointer_id);
    }

    let enabled = !native_touch_gestures_disabled();
    let probe = if enabled {
        pointer_type_probe(pointer_id)
    } else {
        NativePointerTypeProbe::Touch
    };
    let decision = ownership.begin(pointer_id, enabled, probe);
    log_touch_ownership(policy.source, pointer_id, decision);
    if !matches!(decision, NativeTouchOwnershipDecision::Owned) {
        return None;
    }
    if policy.suppress_widget_primary {
        ownership.mark_suppress_widget_primary(pointer_id);
    }

    if let Some(info) = pointer_client_info(hwnd, pointer_id) {
        ownership.record_client_position(pointer_id, info.x, info.y);
        let phase = if info.cancelled {
            NativeVideoTouchPhase::Cancel
        } else {
            NativeVideoTouchPhase::Start
        };
        send_touch_event(
            sink,
            policy.source,
            pointer_id,
            [info.x, info.y],
            phase,
            policy.suppress_widget_primary,
        );
        if info.cancelled {
            ownership.release(pointer_id);
        }
    }
    if policy.request_focus_claim
        && ownership.contains(pointer_id)
        && let Some(sink) = sink
    {
        sink.send(NativeVideoWindowEvent::RequestFocusClaim);
    }
    Some(LRESULT(0))
}

fn handle_owned_pointer_followup(
    ownership: &mut NativeTouchOwnership,
    sink: Option<&NativeVideoWindowEventSink>,
    policy: NativeTouchPointerPolicy,
    hwnd: HWND,
    msg: u32,
    pointer_id: u32,
) -> Option<LRESULT> {
    let decision = ownership.followup(pointer_id);
    log_touch_ownership(policy.source, pointer_id, decision);
    if !matches!(decision, NativeTouchOwnershipDecision::Owned) {
        return None;
    }

    if msg == WM_POINTERCAPTURECHANGED {
        // No Cancel/capture-loss message was observed during the Phase 1
        // hardware gate. Keep this defensive path and its diagnostic live.
        let suppress_widget_primary = ownership.suppresses_widget_primary(pointer_id);
        if let Some(position) = ownership.last_client_position(pointer_id) {
            send_touch_event(
                sink,
                policy.source,
                pointer_id,
                position,
                NativeVideoTouchPhase::Cancel,
                suppress_widget_primary,
            );
        }
        ownership.release(pointer_id);
        return Some(LRESULT(0));
    }
    if matches!(msg, WM_POINTERENTER | WM_POINTERLEAVE) {
        return Some(LRESULT(0));
    }

    let Some(info) = pointer_client_info(hwnd, pointer_id) else {
        if msg == WM_POINTERUP {
            let suppress_widget_primary = ownership.suppresses_widget_primary(pointer_id);
            if let Some(position) = ownership.last_client_position(pointer_id) {
                send_touch_event(
                    sink,
                    policy.source,
                    pointer_id,
                    position,
                    NativeVideoTouchPhase::Cancel,
                    suppress_widget_primary,
                );
            }
            ownership.release(pointer_id);
        }
        return Some(LRESULT(0));
    };
    ownership.record_client_position(pointer_id, info.x, info.y);
    // An activation tap is delivered like any other gesture; only its
    // synthetic primary press is withheld downstream.
    let suppress_widget_primary = ownership.suppresses_widget_primary(pointer_id);
    let phase = native_touch_followup_phase(info.cancelled, msg == WM_POINTERUP);
    send_touch_event(
        sink,
        policy.source,
        pointer_id,
        [info.x, info.y],
        phase,
        suppress_widget_primary,
    );
    if info.cancelled || msg == WM_POINTERUP {
        ownership.release(pointer_id);
    }
    Some(LRESULT(0))
}

pub(crate) fn cancel_hud_touch_streams(
    ownership: &mut NativeTouchOwnership,
    sink: &NativeVideoWindowEventSink,
) {
    while let Some(pointer_id) = ownership.first_pointer_id() {
        let suppress_widget_primary = ownership.suppresses_widget_primary(pointer_id);
        if let Some(position) = ownership.last_client_position(pointer_id) {
            send_touch_event(
                Some(sink),
                NativeVideoWindowSource::Hud,
                pointer_id,
                position,
                NativeVideoTouchPhase::Cancel,
                suppress_widget_primary,
            );
        }
        ownership.release(pointer_id);
    }
}

pub(crate) fn should_discard_promoted_touch_mouse(
    msg: u32,
    source_window: NativeVideoWindowSource,
) -> bool {
    let mut source = INPUT_MESSAGE_SOURCE::default();
    let query_succeeded = unsafe { GetCurrentInputMessageSource(&mut source) }.is_ok();
    let discard = native_touch_mouse_discard_decision(
        !native_touch_gestures_disabled(),
        query_succeeded,
        source.deviceType == IMDT_TOUCH,
    );
    if discard {
        crate::touch_debug::log_native_touch_mouse_discard(source_window.touch_debug_window(), msg);
    }
    discard
}

unsafe extern "system" fn wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    log_win32_message(TouchDebugWindow::Presenter, hwnd, msg, wparam, lparam);
    if let Some(result) = handle_presenter_pointer_message(hwnd, msg, wparam) {
        return result;
    }
    match msg {
        WM_NCCREATE => {
            let createstruct = lparam.0 as *const CREATESTRUCTW;
            if !createstruct.is_null() {
                let state = unsafe { (*createstruct).lpCreateParams } as *mut WindowState;
                if !state.is_null() {
                    unsafe {
                        let _ = SetWindowLongPtrW(hwnd, GWLP_USERDATA, state as isize);
                    }
                }
            }
            unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
        }
        WM_MOUSEACTIVATE => {
            // 他アプリから戻ってきたときの左クリックは「フォーカスを戻すためのクリック」
            // とみなし、再生 toggle に作用させない。MA_ACTIVATEANDEAT はアクティブ化を
            // 引き起こしたマウス down メッセージ (= WM_LBUTTONDOWN) を Windows が破棄
            // する動作で、結果として App 経路 (handle_native_video_mouse_button) と
            // egui overlay 経路 (overlay_draw.rs primary_clicked) のどちらの click
            // 判定も成立しなくなる。WM_LBUTTONUP 単独は届く可能性があるが、App 側は
            // 対応する down 記録が無ければ無視するし、egui も down 抜きの up を
            // click 扱いしないので副作用は出ない。
            //
            // 画像フルスクリーン (ui_fullscreen.rs の fs_primary_suppression)
            // と同じく左クリックのみ抑制し、右/中ボタンによるアクティブ化は通常通り
            // 通す (右クリック = フルスクリーン終了がそのまま走るのは画像側挙動と
            // 整合する)。LOWORD(lparam) == HTCLIENT で「クライアント領域上のクリック」
            // だけを対象にし、test_windowed (debug 用ウィンドウモード) で title bar や
            // resize 枠をクリックして戻るときの操作まで食べないようにする。
            // WM_LBUTTONDBLCLK は通常 WM_LBUTTONDOWN の後に来るので trigger になる
            // ことは稀だが、念のため同等扱い。
            let hit_test = (lparam.0 & 0xFFFF) as u32;
            let trigger_msg = ((lparam.0 >> 16) & 0xFFFF) as u32;
            let is_left = trigger_msg == WM_LBUTTONDOWN || trigger_msg == WM_LBUTTONDBLCLK;
            let is_child = unsafe { (GetWindowLongPtrW(hwnd, GWL_STYLE) as u32 & WS_CHILD.0) != 0 };
            let want_eat = hit_test == HTCLIENT as u32 && is_left;
            // popup (フルスクリーン) はトップレベルウィンドウなので、WM_MOUSEACTIVATE は
            // 「非アクティブ状態へのクリック」= フォーカス復帰クリックのときだけ届く。
            // 従来どおり HTCLIENT 上の左クリックを無条件で ANDEAT する。
            //
            // in-window モードの child window は親 (main window) が別スレッドのため
            // WM_MOUSEACTIVATE が毎クリック届く。無条件 ANDEAT だと通常の再生クリック
            // まで全部食われるので、「この WM_MOUSEACTIVATE 時点の foreground が mIV
            // プロセスのものだと確証できないとき」だけ ANDEAT する。実機ログ (2026-05)
            // で確認した挙動:
            //   - 真の復帰クリック: アクティブ化遷移中で GetForegroundWindow()==NULL
            //   - mIV が既に前面での通常クリック: foreground は有効な mIV の HWND
            // foreground_belongs_to_current_process_strict() は NULL / pid0 を「ours で
            // ない」と判定するので、復帰クリックだけを正しく ANDEAT できる (NULL を
            // 「ours」とみなす非 strict 版だと復帰クリックを取りこぼす)。
            let foreground_is_ours = foreground_belongs_to_current_process_strict();
            let eat = if is_child {
                want_eat && !foreground_is_ours
            } else {
                want_eat
            };
            if eat {
                LRESULT(MA_ACTIVATEANDEAT as isize)
            } else {
                LRESULT(MA_ACTIVATE as isize)
            }
        }
        WM_KEYDOWN | WM_SYSKEYDOWN => {
            if let Some(sink) = window_state(hwnd).and_then(|s| s.event_sink.as_ref()) {
                let key = native_key_event(wparam, lparam);
                crate::key_debug::record_native_video_key(key, true);
                sink.send(NativeVideoWindowEvent::KeyDown(key));
            }
            if wparam.0 as u32 == 0x1B && window_state(hwnd).is_some_and(|s| s.close_on_escape) {
                unsafe {
                    let _ = crate::presentation_observer::destroy_window(
                        hwnd,
                        crate::presentation_observer::WindowRole::Presenter,
                        "presenter_wndproc_escape",
                    );
                }
                return LRESULT(0);
            }
            unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
        }
        WM_KEYUP | WM_SYSKEYUP => {
            if let Some(sink) = window_state(hwnd).and_then(|s| s.event_sink.as_ref()) {
                let key = native_key_event(wparam, lparam);
                crate::key_debug::record_native_video_key(key, false);
                sink.send(NativeVideoWindowEvent::KeyUp(key));
            }
            unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
        }
        WM_APPCOMMAND => {
            // Mouse driver が進む/戻るボタンを APPCOMMAND_BROWSER_BACKWARD/FORWARD で
            // 送ってくる経路 (Chrome / Explorer 等の標準ナビ経路) を受ける。
            // HIWORD(lparam) の下 12 bit が AppCommand コード。
            //  1 = APPCOMMAND_BROWSER_BACKWARD (= 戻る = Ctrl+↑ 相当)
            //  2 = APPCOMMAND_BROWSER_FORWARD  (= 進む = Ctrl+↓ 相当)
            let cmd_word = ((lparam.0 >> 16) & 0xFFFF) as u32;
            let app_command = cmd_word & 0xFFF;
            let synth_vk = match app_command {
                1 => Some(0xA6_u32), // VK_BROWSER_BACK
                2 => Some(0xA7_u32), // VK_BROWSER_FORWARD
                _ => None,
            };
            if let Some(vk) = synth_vk
                && let Some(sink) = window_state(hwnd).and_then(|s| s.event_sink.as_ref())
            {
                let key = NativeVideoKeyEvent {
                    virtual_key: vk,
                    scan_code: 0,
                    extended: false,
                    shift: false,
                    ctrl: false,
                    alt: false,
                    repeat: false,
                };
                crate::key_debug::record_native_video_key(key, true);
                sink.send(NativeVideoWindowEvent::KeyDown(key));
                // WM_APPCOMMAND の規約: 処理した場合 TRUE を返す。
                return LRESULT(1);
            }
            unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
        }
        WM_CHAR => {
            if let Some(ch) = char::from_u32(wparam.0 as u32)
                && !ch.is_control()
                && let Some(sink) = window_state(hwnd).and_then(|s| s.event_sink.as_ref())
            {
                sink.send(NativeVideoWindowEvent::Text(ch));
            }
            LRESULT(0)
        }
        WM_IME_STARTCOMPOSITION => {
            if let Some(state) = window_state_mut(hwnd) {
                state.ime_preediting = false;
                send_ime_event(state, NativeVideoImeEvent::Enabled);
            }
            unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
        }
        WM_IME_COMPOSITION => {
            if let Some(state) = window_state_mut(hwnd) {
                let flags = lparam.0 as u32;
                if flags == 0 {
                    state.ime_preediting = false;
                    send_ime_event(state, NativeVideoImeEvent::Preedit(String::new()));
                }
                if (flags & GCS_RESULTSTR.0) != 0
                    && let Some(text) = ime_composition_string(hwnd, GCS_RESULTSTR)
                {
                    state.ime_preediting = false;
                    send_ime_event(state, NativeVideoImeEvent::Preedit(String::new()));
                    if !text.is_empty() {
                        send_ime_event(state, NativeVideoImeEvent::Commit(text));
                    }
                }
                if (flags & GCS_COMPSTR.0) != 0
                    && let Some(text) = ime_composition_string(hwnd, GCS_COMPSTR)
                {
                    state.ime_preediting = true;
                    send_ime_event(state, NativeVideoImeEvent::Preedit(text));
                }
                // egui draws the preedit text itself; suppress the default IME
                // composition window that otherwise appears at the top-left.
                LRESULT(0)
            } else {
                unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
            }
        }
        WM_IME_ENDCOMPOSITION => {
            if let Some(state) = window_state_mut(hwnd) {
                if state.ime_preediting
                    && let Some(text) = ime_composition_string(hwnd, GCS_RESULTSTR)
                {
                    send_ime_event(state, NativeVideoImeEvent::Preedit(String::new()));
                    if !text.is_empty() {
                        send_ime_event(state, NativeVideoImeEvent::Commit(text));
                    }
                }
                state.ime_preediting = false;
                send_ime_event(state, NativeVideoImeEvent::Disabled);
            }
            unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
        }
        WM_IME_SETCONTEXT => {
            let lparam = LPARAM(lparam.0 & !(ISC_SHOWUICOMPOSITIONWINDOW as isize));
            unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
        }
        // Keep this policy paired with the HUD `WM_SETCURSOR` branch in
        // `native_window_host/hud_window.rs` (2026-06-06 decision). The pump-owned
        // reducer is the only code that applies the presenter cursor. Calling
        // `DefWindowProcW` here would restore the class arrow on every mouse move,
        // overwriting both egui's non-arrow cursor and `SetCursor(None)` from auto-hide.
        // Do not call `SetCursor` here either: returning handled preserves exactly the
        // icon (including hidden) that the reducer last applied.
        WM_SETCURSOR => {
            let hit_test = signed_low_word(lparam.0);
            let trigger_message = ((lparam.0 as u32 >> 16) & 0xffff) as u16;
            let result = LRESULT(1);
            super::cursor_debug::log(format_args!(
                "layer=win32 event=WM_SETCURSOR window=presenter hwnd=0x{:016X} cursor_hwnd=0x{:016X} hit_test={hit_test} trigger_message=0x{trigger_message:04X} handler=explicit returned={}",
                hwnd.0 as usize as u64, wparam.0 as u64, result.0,
            ));
            result
        }
        WM_MOUSEMOVE => {
            super::cursor_debug::log(format_args!(
                "layer=win32 event=WM_MOUSEMOVE window=presenter hwnd=0x{:016X} client_px=({}, {})",
                hwnd.0 as usize as u64,
                signed_low_word(lparam.0),
                signed_high_word(lparam.0),
            ));
            if should_discard_promoted_touch_mouse(msg, NativeVideoWindowSource::Presenter) {
                return LRESULT(0);
            }
            if let Some(sink) = window_state(hwnd).and_then(|s| s.event_sink.as_ref()) {
                // CP9 実機 debug: presenter wndproc 経由の mouse は HUD region 外 (= 穴)
                // のときに来るので、これが頻発しているなら HUD region に問題がある。
                // 100ms 周期 rate limit で log。
                if std::env::var_os("MIV_HUD_DEBUG").is_some() {
                    static LAST_LOG_MS: std::sync::atomic::AtomicI64 =
                        std::sync::atomic::AtomicI64::new(0);
                    let now_ms = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_millis() as i64)
                        .unwrap_or(0);
                    let last = LAST_LOG_MS.load(std::sync::atomic::Ordering::Relaxed);
                    if now_ms - last >= 100 {
                        LAST_LOG_MS.store(now_ms, std::sync::atomic::Ordering::Relaxed);
                        let evt = native_mouse_event(wparam, lparam);
                        crate::logger::log(format!(
                            "[HUD-DEBUG] presenter WM_MOUSEMOVE x={} y={}",
                            evt.x, evt.y
                        ));
                    }
                }
                sink.send(NativeVideoWindowEvent::MouseMove(native_mouse_event(
                    wparam, lparam,
                )));
                if !track_mouse_leave(hwnd) {
                    sink.send(NativeVideoWindowEvent::CursorOwnership(
                        NativeCursorOwnershipEdge::TrackingFailed,
                    ));
                }
            }
            unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
        }
        WM_LBUTTONDOWN | WM_LBUTTONUP | WM_RBUTTONDOWN | WM_RBUTTONUP | WM_MBUTTONDOWN
        | WM_MBUTTONUP | WM_LBUTTONDBLCLK | WM_RBUTTONDBLCLK | WM_MBUTTONDBLCLK
        | WM_XBUTTONDOWN | WM_XBUTTONUP | WM_XBUTTONDBLCLK => {
            if should_discard_promoted_touch_mouse(msg, NativeVideoWindowSource::Presenter) {
                return LRESULT(0);
            }
            if mouse_message_is_down(msg) {
                unsafe {
                    let _ = SetCapture(hwnd);
                }
            } else {
                unsafe {
                    let _ = ReleaseCapture();
                }
            }
            if let Some(sink) = window_state(hwnd).and_then(|s| s.event_sink.as_ref()) {
                sink.send(NativeVideoWindowEvent::MouseButton(
                    native_mouse_button_event(msg, wparam, lparam),
                ));
            }
            // WM_XBUTTONUP に対して DefWindowProc に流すと、Windows が
            // APPCOMMAND_BROWSER_BACKWARD/FORWARD を合成して WM_APPCOMMAND を再送する
            // ([MS docs: Mouse Input Overview](https://learn.microsoft.com/en-us/windows/win32/inputdev/about-mouse-input))。
            // 進む/戻るは既に MouseButton(Extra1/Extra2) で処理しているので、その後
            // WM_APPCOMMAND を本ファイル下の handler が再度拾うと 1 押下 = 2 ナビになる。
            // TRUE (= 処理済み) を返して APPCOMMAND 合成を抑止 (Codex 2 周目 P2)。
            // APPCOMMAND 経路は driver / AHK が WM_APPCOMMAND を直接送る場合のみに限定する。
            if msg == WM_XBUTTONUP {
                return LRESULT(1);
            }
            unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
        }
        WM_MOUSEWHEEL => {
            if should_discard_promoted_touch_mouse(msg, NativeVideoWindowSource::Presenter) {
                return LRESULT(0);
            }
            if let Some(sink) = window_state(hwnd).and_then(|s| s.event_sink.as_ref()) {
                sink.send(NativeVideoWindowEvent::MouseWheel(
                    native_mouse_wheel_event(hwnd, wparam, lparam),
                ));
            }
            unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
        }
        WM_MOUSELEAVE => {
            if should_discard_promoted_touch_mouse(msg, NativeVideoWindowSource::Presenter) {
                return LRESULT(0);
            }
            // WndProc は decode/enqueue のみ。generic leave は egui pointer 用、source-stamped
            // ownership edge は pump-owned cursor router 用として別々に送る。
            if let Some(sink) = window_state(hwnd).and_then(|s| s.event_sink.as_ref()) {
                sink.send(NativeVideoWindowEvent::MouseLeave);
                sink.send(NativeVideoWindowEvent::CursorOwnership(
                    NativeCursorOwnershipEdge::Leave,
                ));
            }
            unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
        }
        WM_CAPTURECHANGED | WM_CANCELMODE => {
            if let Some(sink) = window_state(hwnd).and_then(|s| s.event_sink.as_ref()) {
                sink.send(NativeVideoWindowEvent::CursorOwnership(
                    NativeCursorOwnershipEdge::CaptureLost,
                ));
            }
            unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
        }
        WM_WINDOWPOSCHANGED => {
            // CP8: presenter HWND の位置 / サイズが変わったら HUD HWND に mirror する。
            // `lparam` は `WINDOWPOS*`。
            //
            // **Codex CP8 P1 反映**: `SWP_NOMOVE` / `SWP_NOSIZE` が立っているとき、その field
            // (x/y or cx/cy) は無視される値で、bogus。z-order 変更だけの `SetWindowPos(...,
            // SWP_NOMOVE | SWP_NOSIZE, ...)` でも `WM_WINDOWPOSCHANGED` が発火するため、
            // フラグを見ずに値を信じると HUD geometry / presenter.resize が bogus で走る。
            //
            // 対策:
            //   - `SWP_NOMOVE && SWP_NOSIZE` 両方なら event 発火せず skip。
            //   - どちらかだけ立っているなら `GetWindowRect(hwnd)` で現在値を取り直す。
            //
            // borderless `WS_POPUP` なので window cx/cy = client サイズ前提。
            if lparam.0 != 0 {
                let wp = lparam.0 as *const WINDOWPOS;
                if !wp.is_null() {
                    let (flags_value, mut x, mut y, mut w, mut h) = unsafe {
                        let p = &*wp;
                        (p.flags.0, p.x, p.y, p.cx, p.cy)
                    };
                    let no_move = (flags_value & SWP_NOMOVE.0) != 0;
                    let no_size = (flags_value & SWP_NOSIZE.0) != 0;
                    if !(no_move && no_size) {
                        if no_move || no_size {
                            // 片方だけ bogus なので `GetWindowRect` で現在値を取り直す。
                            let mut rc = windows::Win32::Foundation::RECT::default();
                            if unsafe { GetWindowRect(hwnd, &mut rc) }.is_ok() {
                                if no_move {
                                    x = rc.left;
                                    y = rc.top;
                                }
                                if no_size {
                                    w = rc.right - rc.left;
                                    h = rc.bottom - rc.top;
                                }
                            }
                        }
                        let mut client = windows::Win32::Foundation::RECT::default();
                        let (w_u32, h_u32) = if unsafe { GetClientRect(hwnd, &mut client) }.is_ok()
                        {
                            (
                                (client.right - client.left).max(1) as u32,
                                (client.bottom - client.top).max(1) as u32,
                            )
                        } else {
                            (w.max(1) as u32, h.max(1) as u32)
                        };
                        if let Some(sink) = window_state(hwnd).and_then(|s| s.event_sink.as_ref()) {
                            sink.send(NativeVideoWindowEvent::GeometryChanged {
                                x,
                                y,
                                w: w_u32,
                                h: h_u32,
                                maximized: unsafe { IsZoomed(hwnd).as_bool() },
                            });
                        }
                    }
                    // 両方 NoMove + NoSize は z-order だけの変更 → event 発火しない。
                }
            }
            unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
        }
        WM_CLOSE => {
            if let Some(state) = window_state(hwnd)
                && let Some(sink) = state.event_sink.as_ref()
            {
                sink.send(NativeVideoWindowEvent::CloseRequested {
                    generation: state.generation,
                });
            }
            unsafe {
                let _ = crate::presentation_observer::destroy_window(
                    hwnd,
                    crate::presentation_observer::WindowRole::Presenter,
                    "presenter_wndproc_close",
                );
            }
            LRESULT(0)
        }
        WM_DESTROY => {
            if let Some(sink) = window_state(hwnd).and_then(|s| s.event_sink.as_ref()) {
                sink.send(NativeVideoWindowEvent::Destroyed);
            }
            LRESULT(0)
        }
        WM_NCDESTROY => {
            let ptr = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *mut WindowState;
            if !ptr.is_null() {
                unsafe {
                    (*ptr).touch_ownership.clear();
                    let _ = SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
                    let _ = Box::from_raw(ptr);
                }
            }
            unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

fn window_state(hwnd: HWND) -> Option<&'static WindowState> {
    let ptr = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *const WindowState;
    if ptr.is_null() {
        None
    } else {
        Some(unsafe { &*ptr })
    }
}

fn window_state_mut(hwnd: HWND) -> Option<&'static mut WindowState> {
    let ptr = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *mut WindowState;
    if ptr.is_null() {
        None
    } else {
        Some(unsafe { &mut *ptr })
    }
}

fn send_ime_event(state: &WindowState, event: NativeVideoImeEvent) {
    if let Some(sink) = state.event_sink.as_ref() {
        sink.send(NativeVideoWindowEvent::Ime(event));
    }
}

fn ime_composition_string(hwnd: HWND, mode: IME_COMPOSITION_STRING) -> Option<String> {
    unsafe {
        let himc = ImmGetContext(hwnd);
        if himc.0.is_null() {
            return None;
        }
        let byte_len = ImmGetCompositionStringW(himc, mode, None, 0);
        let result = if byte_len < 0 {
            None
        } else if byte_len == 0 {
            Some(String::new())
        } else {
            let mut buf = vec![0_u16; byte_len as usize / 2];
            let read = ImmGetCompositionStringW(
                himc,
                mode,
                Some(buf.as_mut_ptr().cast::<c_void>()),
                byte_len as u32,
            );
            if read < 0 {
                None
            } else {
                buf.truncate(read as usize / 2);
                Some(String::from_utf16_lossy(&buf))
            }
        };
        let _ = ImmReleaseContext(hwnd, himc);
        result
    }
}

fn native_key_event(wparam: WPARAM, lparam: LPARAM) -> NativeVideoKeyEvent {
    // The native presenter WndProc is a separate input route and is explicitly
    // outside the app-side synthetic timeline.
    let shift = unsafe { GetKeyState(VK_SHIFT.0 as i32) } < 0;
    let ctrl = unsafe { GetKeyState(VK_CONTROL.0 as i32) } < 0;
    let alt = unsafe { GetKeyState(VK_MENU.0 as i32) } < 0;
    let raw = lparam.0 as u64;
    NativeVideoKeyEvent {
        virtual_key: wparam.0 as u32,
        scan_code: ((raw >> 16) & 0xff) as u16,
        extended: (raw & (1 << 24)) != 0,
        shift,
        ctrl,
        alt,
        repeat: ((lparam.0 as u64) & (1 << 30)) != 0,
    }
}

fn native_mouse_event(wparam: WPARAM, lparam: LPARAM) -> NativeVideoMouseEvent {
    NativeVideoMouseEvent {
        x: signed_low_word(lparam.0),
        y: signed_high_word(lparam.0),
        shift: mouse_shift(wparam),
        ctrl: mouse_ctrl(wparam),
    }
}

fn native_mouse_button_event(
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> NativeVideoMouseButtonEvent {
    let button = match msg {
        WM_RBUTTONDOWN | WM_RBUTTONUP | WM_RBUTTONDBLCLK => NativeVideoMouseButton::Right,
        WM_MBUTTONDOWN | WM_MBUTTONUP | WM_MBUTTONDBLCLK => NativeVideoMouseButton::Middle,
        WM_XBUTTONDOWN | WM_XBUTTONUP | WM_XBUTTONDBLCLK => {
            // HIWORD(wparam): XBUTTON1=1 (back), XBUTTON2=2 (forward)
            match ((wparam.0 >> 16) & 0xFFFF) as u16 {
                2 => NativeVideoMouseButton::Extra2,
                _ => NativeVideoMouseButton::Extra1,
            }
        }
        _ => NativeVideoMouseButton::Left,
    };
    NativeVideoMouseButtonEvent {
        button,
        down: mouse_message_is_down(msg),
        double_click: matches!(
            msg,
            WM_LBUTTONDBLCLK | WM_RBUTTONDBLCLK | WM_MBUTTONDBLCLK | WM_XBUTTONDBLCLK
        ),
        x: signed_low_word(lparam.0),
        y: signed_high_word(lparam.0),
        shift: mouse_shift(wparam),
        ctrl: mouse_ctrl(wparam),
    }
}

fn native_mouse_wheel_event(
    hwnd: HWND,
    wparam: WPARAM,
    lparam: LPARAM,
) -> NativeVideoMouseWheelEvent {
    let mut point = POINT {
        x: signed_low_word(lparam.0),
        y: signed_high_word(lparam.0),
    };
    unsafe {
        let _ = ScreenToClient(hwnd, &mut point);
    }
    NativeVideoMouseWheelEvent {
        delta: signed_high_word(wparam.0 as isize) as i16,
        x: point.x,
        y: point.y,
        shift: mouse_shift(wparam),
        ctrl: mouse_ctrl(wparam),
    }
}

fn mouse_message_is_down(msg: u32) -> bool {
    matches!(
        msg,
        WM_LBUTTONDOWN
            | WM_RBUTTONDOWN
            | WM_MBUTTONDOWN
            | WM_XBUTTONDOWN
            | WM_LBUTTONDBLCLK
            | WM_RBUTTONDBLCLK
            | WM_MBUTTONDBLCLK
            | WM_XBUTTONDBLCLK
    )
}

fn mouse_shift(wparam: WPARAM) -> bool {
    (wparam.0 & 0x0004) != 0
}

fn mouse_ctrl(wparam: WPARAM) -> bool {
    (wparam.0 & 0x0008) != 0
}

fn track_mouse_leave(hwnd: HWND) -> bool {
    let mut track = TRACKMOUSEEVENT {
        cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
        dwFlags: TME_LEAVE,
        hwndTrack: hwnd,
        dwHoverTime: 0,
    };
    unsafe { TrackMouseEvent(&mut track).is_ok() }
}

fn signed_low_word(value: isize) -> i32 {
    ((value as u32 & 0xffff) as i16) as i32
}

fn signed_high_word(value: isize) -> i32 {
    (((value as u32 >> 16) & 0xffff) as i16) as i32
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Mutex, OnceLock};

    use windows::Win32::UI::WindowsAndMessaging::{
        DestroyWindow, GetCursorPos, HWND_TOPMOST, SWP_NOACTIVATE, SWP_SHOWWINDOW,
        SetForegroundWindow, SetWindowPos, ShowWindow, WS_EX_TOOLWINDOW,
    };

    static CURSOR_WINDOW_TEST_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

    struct TestTopLevelWindow(HWND);

    impl Drop for TestTopLevelWindow {
        fn drop(&mut self) {
            unsafe {
                let _ = DestroyWindow(self.0);
            }
        }
    }

    struct ForegroundRestore(HWND);

    impl Drop for ForegroundRestore {
        fn drop(&mut self) {
            unsafe {
                if !self.0.0.is_null() && IsWindow(Some(self.0)).as_bool() {
                    let _ = SetForegroundWindow(self.0);
                }
            }
        }
    }

    fn wait_for_cursor_window_event(
        receiver: &NativeWindowEventReceiver,
        description: &str,
        mut predicate: impl FnMut(&NativeVideoWindowEventEnvelope) -> bool,
    ) -> NativeVideoWindowEventEnvelope {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        loop {
            let _ = pump_thread_messages();
            if let Some(event) = receiver.drain().into_iter().find(|event| predicate(event)) {
                return event;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "timed out waiting for {description}"
            );
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
    }

    fn key(virtual_key: u32) -> NativeVideoWindowEvent {
        NativeVideoWindowEvent::KeyDown(NativeVideoKeyEvent {
            virtual_key,
            scan_code: 0,
            extended: false,
            shift: false,
            ctrl: false,
            alt: false,
            repeat: false,
        })
    }

    #[test]
    fn presenter_wndproc_handles_setcursor_without_defaulting_to_class_arrow() {
        let trigger = ((WM_MOUSEMOVE as isize) << 16) | HTCLIENT as isize;
        let result = unsafe { wnd_proc(HWND::default(), WM_SETCURSOR, WPARAM(0), LPARAM(trigger)) };

        assert_eq!(result, LRESULT(1));
    }

    fn attach_span_sample_points(fields: &[(&'static str, serde_json::Value)]) -> Vec<String> {
        fields
            .iter()
            .find_map(|(name, value)| (*name == "samples").then_some(value))
            .and_then(serde_json::Value::as_array)
            .expect("attach span samples field")
            .iter()
            .map(|sample| {
                sample
                    .get("point")
                    .and_then(serde_json::Value::as_str)
                    .expect("attach span sample point")
                    .to_owned()
            })
            .collect()
    }

    #[test]
    fn claim_foreground_modifier_probe_records_four_samples_after_successful_attach() {
        let mut probe = AttachSpanProbe::new(true, 10, 20, true);
        for point in [
            AttachSpanSamplePoint::BeforeAttach,
            AttachSpanSamplePoint::AfterAttach,
            AttachSpanSamplePoint::AfterFocus,
            AttachSpanSamplePoint::AfterDetach,
        ] {
            probe.record_with(point, crate::modifier_probe::SidedModifierSnapshot::default);
        }

        let fields = probe
            .into_fields(true, true)
            .expect("enabled probe must emit fields");
        assert_eq!(
            attach_span_sample_points(&fields),
            [
                "before_attach",
                "after_attach",
                "after_focus",
                "after_detach"
            ]
        );
        assert_eq!(
            fields
                .iter()
                .find_map(|(name, value)| (*name == "this_tid").then_some(value)),
            Some(&serde_json::Value::from(10))
        );
        assert_eq!(
            fields
                .iter()
                .find_map(|(name, value)| (*name == "foreground_tid").then_some(value)),
            Some(&serde_json::Value::from(20))
        );
        assert_eq!(
            fields.iter().find_map(|(name, value)| {
                (*name == "partner_is_miv_ui_thread").then_some(value)
            }),
            Some(&serde_json::Value::Bool(true))
        );
        assert_eq!(
            fields
                .iter()
                .find_map(|(name, value)| (*name == "attach_ok").then_some(value)),
            Some(&serde_json::Value::Bool(true))
        );
        assert_eq!(
            fields
                .iter()
                .find_map(|(name, value)| (*name == "detach_ok").then_some(value)),
            Some(&serde_json::Value::Bool(true))
        );
    }

    #[test]
    fn claim_foreground_modifier_probe_omits_after_attach_when_attach_is_unsuccessful() {
        let mut probe = AttachSpanProbe::new(true, 10, 30, false);
        for point in [
            AttachSpanSamplePoint::BeforeAttach,
            AttachSpanSamplePoint::AfterFocus,
            AttachSpanSamplePoint::AfterDetach,
        ] {
            probe.record_with(point, crate::modifier_probe::SidedModifierSnapshot::default);
        }

        let fields = probe
            .into_fields(false, false)
            .expect("enabled probe must emit fields");
        assert_eq!(
            attach_span_sample_points(&fields),
            ["before_attach", "after_focus", "after_detach"]
        );
    }

    #[test]
    fn claim_foreground_modifier_probe_is_silent_when_perf_is_disabled() {
        let mut sampled = 0;
        let mut probe = AttachSpanProbe::new(false, 10, 20, true);
        for point in [
            AttachSpanSamplePoint::BeforeAttach,
            AttachSpanSamplePoint::AfterAttach,
            AttachSpanSamplePoint::AfterFocus,
            AttachSpanSamplePoint::AfterDetach,
        ] {
            probe.record_with(point, || {
                sampled += 1;
                crate::modifier_probe::SidedModifierSnapshot::default()
            });
        }

        assert_eq!(sampled, 0);
        assert!(probe.into_fields(true, true).is_none());
    }

    #[test]
    fn window_event_sink_coalesces_mouse_and_keeps_key_ime_lossless() {
        let overflow = Arc::new(AtomicBool::new(false));
        let (pump_route, pump_rx) = native_window_event_route(8, Arc::clone(&overflow));
        let (render_route, render_rx) = native_window_event_route(8, Arc::clone(&overflow));
        let sink = NativeVideoWindowEventSink::new(
            7,
            7,
            NativeVideoWindowSource::Presenter,
            pump_route,
            render_route,
        );

        sink.send(key(0x41));
        sink.send(NativeVideoWindowEvent::Ime(NativeVideoImeEvent::Commit(
            "日本語".to_string(),
        )));
        sink.send(NativeVideoWindowEvent::MouseMove(NativeVideoMouseEvent {
            x: 1,
            y: 2,
            shift: false,
            ctrl: false,
        }));
        sink.send(NativeVideoWindowEvent::MouseMove(NativeVideoMouseEvent {
            x: 30,
            y: 40,
            shift: false,
            ctrl: false,
        }));
        sink.send(NativeVideoWindowEvent::RequestFocusClaim);
        sink.send(NativeVideoWindowEvent::CloseRequested { generation: 7 });

        let pump = pump_rx.drain();
        assert_eq!(pump.len(), 3);
        assert!(matches!(
            pump[0].event,
            NativeVideoWindowEvent::MouseMove(NativeVideoMouseEvent { x: 30, y: 40, .. })
        ));
        assert_eq!(pump[0].source, NativeVideoWindowSource::Presenter);
        assert!(matches!(
            pump[1].event,
            NativeVideoWindowEvent::RequestFocusClaim
        ));
        assert!(matches!(
            pump[2].event,
            NativeVideoWindowEvent::CloseRequested { generation: 7 }
        ));

        let render = render_rx.drain();
        assert_eq!(render.len(), 3);
        assert!(matches!(
            render[0].event,
            NativeVideoWindowEvent::KeyDown(_)
        ));
        assert!(matches!(
            render[1].event,
            NativeVideoWindowEvent::Ime(NativeVideoImeEvent::Commit(_))
        ));
        assert!(matches!(
            render[2].event,
            NativeVideoWindowEvent::MouseMove(NativeVideoMouseEvent { x: 30, y: 40, .. })
        ));
        assert!(!overflow.load(Ordering::Acquire));
    }

    #[test]
    fn touch_events_are_lossless_and_route_to_pump_and_render() {
        let overflow = Arc::new(AtomicBool::new(false));
        let (pump_route, pump_rx) = native_window_event_route(8, Arc::clone(&overflow));
        let (render_route, render_rx) = native_window_event_route(8, Arc::clone(&overflow));
        let sink = NativeVideoWindowEventSink::new(
            7,
            7,
            NativeVideoWindowSource::Presenter,
            pump_route,
            render_route,
        );
        for phase in [
            NativeVideoTouchPhase::Start,
            NativeVideoTouchPhase::Move,
            NativeVideoTouchPhase::End,
        ] {
            sink.send(NativeVideoWindowEvent::Touch(NativeVideoTouchEvent {
                source: NativeVideoWindowSource::Presenter,
                pointer_id: 42,
                x: 100,
                y: 200,
                phase,
                suppress_widget_primary: false,
            }));
        }

        let phases = |events: Vec<NativeVideoWindowEventEnvelope>| {
            events
                .into_iter()
                .map(|event| match event.event {
                    NativeVideoWindowEvent::Touch(touch) => {
                        assert_eq!(touch.source, NativeVideoWindowSource::Presenter);
                        touch.phase
                    }
                    other => panic!("unexpected event: {other:?}"),
                })
                .collect::<Vec<_>>()
        };
        let expected = vec![
            NativeVideoTouchPhase::Start,
            NativeVideoTouchPhase::Move,
            NativeVideoTouchPhase::End,
        ];
        assert_eq!(phases(pump_rx.drain()), expected);
        assert_eq!(phases(render_rx.drain()), expected);
        assert!(!overflow.load(Ordering::Acquire));
    }

    #[test]
    fn hud_lifecycle_cleanup_cancels_and_releases_all_owned_streams() {
        let overflow = Arc::new(AtomicBool::new(false));
        let (pump_route, pump_rx) = native_window_event_route(8, Arc::clone(&overflow));
        let (render_route, render_rx) = native_window_event_route(8, Arc::clone(&overflow));
        let sink = NativeVideoWindowEventSink::new(
            7,
            7,
            NativeVideoWindowSource::Hud,
            pump_route,
            render_route,
        );
        let mut ownership = NativeTouchOwnership::default();
        for (pointer_id, position) in [(11, [100, 200]), (12, [300, 400])] {
            assert_eq!(
                ownership.begin(pointer_id, true, NativePointerTypeProbe::Touch),
                NativeTouchOwnershipDecision::Owned
            );
            ownership.record_client_position(pointer_id, position[0], position[1]);
        }

        cancel_hud_touch_streams(&mut ownership, &sink);

        assert!(ownership.is_empty());
        let assert_cancels = |events: Vec<NativeVideoWindowEventEnvelope>| {
            assert_eq!(events.len(), 2);
            for event in events {
                assert_eq!(event.source, NativeVideoWindowSource::Hud);
                match event.event {
                    NativeVideoWindowEvent::Touch(touch) => {
                        assert_eq!(touch.source, NativeVideoWindowSource::Hud);
                        assert_eq!(touch.phase, NativeVideoTouchPhase::Cancel);
                    }
                    other => panic!("unexpected event: {other:?}"),
                }
            }
        };
        assert_cancels(pump_rx.drain());
        assert_cancels(render_rx.drain());
        assert!(!overflow.load(Ordering::Acquire));
    }

    #[test]
    fn lossless_window_event_overflow_is_an_explicit_session_fault() {
        let overflow = Arc::new(AtomicBool::new(false));
        let (route, _rx) = native_window_event_route(1, Arc::clone(&overflow));
        route.send(NativeVideoWindowEventEnvelope {
            sequence: 0,
            epoch: 1,
            generation: 1,
            source: NativeVideoWindowSource::Presenter,
            event: key(0x41),
        });
        route.send(NativeVideoWindowEventEnvelope {
            sequence: 0,
            epoch: 1,
            generation: 1,
            source: NativeVideoWindowSource::Presenter,
            event: key(0x42),
        });
        assert!(overflow.load(Ordering::Acquire));
    }

    #[test]
    #[ignore = "requires an interactive Windows input desktop; run explicitly"]
    fn stationary_cursor_routes_separate_top_level_window_show_and_hide() {
        let _guard = CURSOR_WINDOW_TEST_LOCK
            .get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap();
        let _foreground_restore = ForegroundRestore(unsafe { GetForegroundWindow() });
        let mut cursor = POINT::default();
        if let Err(error) = unsafe { GetCursorPos(&mut cursor) } {
            eprintln!("skipping interactive cursor-window test: GetCursorPos failed: {error:?}");
            return;
        }
        let rect = RECT {
            left: cursor.x - 32,
            top: cursor.y - 32,
            right: cursor.x + 32,
            bottom: cursor.y + 32,
        };
        let hmodule = unsafe { GetModuleHandleW(None).expect("GetModuleHandleW") };
        let cover = TestTopLevelWindow(
            unsafe {
                CreateWindowExW(
                    WS_EX_TOOLWINDOW,
                    w!("STATIC"),
                    w!("mIV cursor ownership cover"),
                    WS_POPUP,
                    rect.left,
                    rect.top,
                    rect.right - rect.left,
                    rect.bottom - rect.top,
                    None,
                    None,
                    Some(HINSTANCE(hmodule.0)),
                    None,
                )
            }
            .expect("create cover test window"),
        );
        unsafe {
            SetWindowPos(
                cover.0,
                Some(HWND_TOPMOST),
                rect.left,
                rect.top,
                rect.right - rect.left,
                rect.bottom - rect.top,
                SWP_SHOWWINDOW,
            )
            .expect("show initial cover test window");
        }
        let overflow = Arc::new(AtomicBool::new(false));
        let (pump_route, pump_rx) = native_window_event_route(16, Arc::clone(&overflow));
        let (render_route, _render_rx) = native_window_event_route(16, Arc::clone(&overflow));
        let sink = NativeVideoWindowEventSink::new(
            41,
            41,
            NativeVideoWindowSource::Presenter,
            pump_route,
            render_route,
        );
        let presenter = NativeVideoWindow::create(NativeVideoWindowConfig {
            mode: NativeVideoWindowMode::Borderless { rect },
            owner_hwnd: 0,
            initially_visible: false,
            activate_on_show: false,
            close_on_escape: false,
            event_sink: Some(sink),
            generation: 41,
        })
        .expect("create presenter test window");
        assert!(presenter.show_no_activate());
        unsafe {
            SetWindowPos(
                presenter.hwnd(),
                Some(HWND_TOPMOST),
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_SHOWWINDOW,
            )
            .expect("raise presenter test window");
        }
        wait_for_cursor_window_event(&pump_rx, "presenter show seed", |event| {
            event.source == NativeVideoWindowSource::Presenter
                && matches!(event.event, NativeVideoWindowEvent::MouseMove(_))
        });

        unsafe {
            SetWindowPos(
                cover.0,
                Some(HWND_TOPMOST),
                rect.left,
                rect.top,
                rect.right - rect.left,
                rect.bottom - rect.top,
                SWP_SHOWWINDOW,
            )
            .expect("show cover test window");
        }
        wait_for_cursor_window_event(&pump_rx, "presenter leave after cover show", |event| {
            event.source == NativeVideoWindowSource::Presenter
                && matches!(
                    event.event,
                    NativeVideoWindowEvent::CursorOwnership(NativeCursorOwnershipEdge::Leave)
                )
        });

        unsafe {
            let _ = ShowWindow(cover.0, SW_HIDE);
        }
        assert!(!unsafe { IsWindowVisible(cover.0).as_bool() });
        assert!(!overflow.load(Ordering::Acquire));
    }
}
