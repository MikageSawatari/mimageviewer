//! Per-viewport Win32 key edge queue.
//!
//! egui flattens some physical keys (notably numpad digits and JIS-specific
//! keys).  The keymap still lets egui handle text/IME normally, but shortcut
//! matching reads key-down edges from this queue when the target viewport's
//! HWND subclass is installed. Each edge is stamped with its source HWND and
//! registered `ViewportId` before it enters the queue.

use std::collections::VecDeque;
use std::sync::{Mutex, OnceLock};

use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::UI::Input::KeyboardAndMouse::{GetKeyState, VK_CONTROL, VK_MENU, VK_SHIFT};
use windows::Win32::UI::Shell::{DefSubclassProc, SetWindowSubclass};
use windows::Win32::UI::WindowsAndMessaging::{
    WM_KEYDOWN, WM_KEYUP, WM_KILLFOCUS, WM_NCDESTROY, WM_SYSKEYDOWN, WM_SYSKEYUP,
};

const MAIN_KEY_INPUT_SUBCLASS_ID: usize = 0x6D69_6B31; // "mik1"
const MAX_PENDING_EVENTS: usize = 256;
const MAX_LOGGED_UNREGISTERED_HWND: usize = 16;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KeyEdge {
    pub source_hwnd: u64,
    pub source_viewport: egui::ViewportId,
    pub virtual_key: u32,
    pub scan_code: u16,
    pub extended: bool,
    pub pressed: bool,
    pub repeat: bool,
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct RawKeyEdge {
    virtual_key: u32,
    scan_code: u16,
    extended: bool,
    pressed: bool,
    repeat: bool,
    ctrl: bool,
    shift: bool,
    alt: bool,
}

impl RawKeyEdge {
    fn with_source(self, source_hwnd: u64, source_viewport: egui::ViewportId) -> KeyEdge {
        KeyEdge {
            source_hwnd,
            source_viewport,
            virtual_key: self.virtual_key,
            scan_code: self.scan_code,
            extended: self.extended,
            pressed: self.pressed,
            repeat: self.repeat,
            ctrl: self.ctrl,
            shift: self.shift,
            alt: self.alt,
        }
    }
}

#[derive(Default)]
struct ReturnKeyState {
    main_down: bool,
    numpad_down: bool,
}

impl ReturnKeyState {
    fn apply_edge(&mut self, edge: &KeyEdge) {
        const VK_RETURN: u32 = 0x0D;
        if edge.virtual_key != VK_RETURN {
            return;
        }
        if edge.extended {
            self.numpad_down = edge.pressed;
        } else {
            self.main_down = edge.pressed;
        }
    }

    fn is_down(&self, extended: bool) -> bool {
        if extended {
            self.numpad_down
        } else {
            self.main_down
        }
    }

    #[cfg(test)]
    fn clear(&mut self) {
        self.main_down = false;
        self.numpad_down = false;
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct InstalledHwnd {
    hwnd_raw: u64,
    viewport: egui::ViewportId,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RegisterHwndResult {
    Inserted,
    AlreadyRegistered,
    ConflictingViewport(egui::ViewportId),
}

#[derive(Default)]
struct HwndViewportRegistry {
    entries: Vec<InstalledHwnd>,
}

impl HwndViewportRegistry {
    fn register(&mut self, hwnd_raw: u64, viewport: egui::ViewportId) -> RegisterHwndResult {
        if let Some(existing) = self.entries.iter().find(|entry| entry.hwnd_raw == hwnd_raw) {
            return if existing.viewport == viewport {
                RegisterHwndResult::AlreadyRegistered
            } else {
                RegisterHwndResult::ConflictingViewport(existing.viewport)
            };
        }
        self.entries.push(InstalledHwnd { hwnd_raw, viewport });
        RegisterHwndResult::Inserted
    }

    fn viewport_for_hwnd(&self, hwnd_raw: u64) -> Option<egui::ViewportId> {
        self.entries
            .iter()
            .find(|entry| entry.hwnd_raw == hwnd_raw)
            .map(|entry| entry.viewport)
    }

    fn contains_viewport(&self, viewport: egui::ViewportId) -> bool {
        self.entries.iter().any(|entry| entry.viewport == viewport)
    }

    fn remove(&mut self, hwnd_raw: u64) -> Option<egui::ViewportId> {
        let index = self
            .entries
            .iter()
            .position(|entry| entry.hwnd_raw == hwnd_raw)?;
        Some(self.entries.remove(index).viewport)
    }

    fn unique_viewports(&self) -> Vec<egui::ViewportId> {
        let mut viewports = Vec::new();
        for entry in &self.entries {
            if !viewports.contains(&entry.viewport) {
                viewports.push(entry.viewport);
            }
        }
        viewports
    }

    fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

struct ViewportReturnKeyState {
    viewport: egui::ViewportId,
    keys: ReturnKeyState,
}

#[derive(Default)]
struct ViewportReturnKeyStates {
    entries: Vec<ViewportReturnKeyState>,
}

impl ViewportReturnKeyStates {
    fn apply_edge(&mut self, viewport: egui::ViewportId, edge: &KeyEdge) {
        if let Some(entry) = self
            .entries
            .iter_mut()
            .find(|entry| entry.viewport == viewport)
        {
            entry.keys.apply_edge(edge);
            return;
        }
        let mut keys = ReturnKeyState::default();
        keys.apply_edge(edge);
        self.entries.push(ViewportReturnKeyState { viewport, keys });
    }

    fn is_down(&self, viewport: egui::ViewportId, extended: bool) -> bool {
        self.entries
            .iter()
            .find(|entry| entry.viewport == viewport)
            .is_some_and(|entry| entry.keys.is_down(extended))
    }

    fn clear_viewport(&mut self, viewport: egui::ViewportId) {
        self.entries.retain(|entry| entry.viewport != viewport);
    }

    fn clear(&mut self) {
        self.entries.clear();
    }
}

#[derive(Default)]
struct KeyInputState {
    installed_hwnds: HwndViewportRegistry,
    pending: VecDeque<KeyEdge>,
    frame: Vec<KeyEdge>,
    frame_active_viewports: Vec<egui::ViewportId>,
    return_keys: ViewportReturnKeyStates,
    logged_unregistered_hwnds: Vec<u64>,
}

impl KeyInputState {
    fn register_hwnd(&mut self, hwnd_raw: u64, viewport: egui::ViewportId) -> RegisterHwndResult {
        let result = self.installed_hwnds.register(hwnd_raw, viewport);
        if matches!(result, RegisterHwndResult::Inserted) {
            self.logged_unregistered_hwnds
                .retain(|logged| *logged != hwnd_raw);
        }
        result
    }

    fn unregister_hwnd(&mut self, hwnd_raw: u64) -> Option<egui::ViewportId> {
        let viewport = self.installed_hwnds.remove(hwnd_raw)?;
        // Edges are stamped with their source HWND. Once that HWND dies, do
        // not let an edge queued before WM_NCDESTROY reach a recreated
        // viewport that happens to reuse the same ViewportId.
        self.pending.retain(|edge| edge.source_hwnd != hwnd_raw);
        self.frame.retain(|edge| edge.source_hwnd != hwnd_raw);
        if !self.installed_hwnds.contains_viewport(viewport) {
            self.return_keys.clear_viewport(viewport);
        }
        if self.installed_hwnds.is_empty() {
            self.pending.clear();
            self.frame.clear();
            self.frame_active_viewports.clear();
            self.return_keys.clear();
        }
        Some(viewport)
    }

    fn enqueue_raw_edge(&mut self, hwnd_raw: u64, raw: RawKeyEdge) -> (KeyEdge, bool) {
        // The root HWND is installed before its subclass can publish input.
        // Missing registration is therefore an invariant violation. Route it
        // explicitly to ROOT for compatibility, but make the violation
        // observable instead of exposing the edge to every viewport.
        let source_viewport = self
            .installed_hwnds
            .viewport_for_hwnd(hwnd_raw)
            .unwrap_or(egui::ViewportId::ROOT);
        let edge = raw.with_source(hwnd_raw, source_viewport);
        self.return_keys.apply_edge(source_viewport, &edge);
        while self.pending.len() >= MAX_PENDING_EVENTS {
            self.pending.pop_front();
        }
        self.pending.push_back(edge);

        let unregistered = self.installed_hwnds.viewport_for_hwnd(hwnd_raw).is_none();
        let should_log = unregistered && !self.logged_unregistered_hwnds.contains(&hwnd_raw);
        if should_log {
            while self.logged_unregistered_hwnds.len() >= MAX_LOGGED_UNREGISTERED_HWND {
                self.logged_unregistered_hwnds.remove(0);
            }
            self.logged_unregistered_hwnds.push(hwnd_raw);
        }
        (edge, should_log)
    }

    fn routed_return_key_held(&self, viewport: egui::ViewportId, extended: bool) -> Option<bool> {
        self.frame_active_viewports
            .contains(&viewport)
            .then(|| self.return_keys.is_down(viewport, extended))
    }
}

fn state() -> &'static Mutex<KeyInputState> {
    static STATE: OnceLock<Mutex<KeyInputState>> = OnceLock::new();
    STATE.get_or_init(|| Mutex::new(KeyInputState::default()))
}

pub fn install_main_window_subclass(hwnd_raw: u64) -> bool {
    install_window_subclass(hwnd_raw, egui::ViewportId::ROOT, "main")
}

pub fn install_viewport_window_subclass(hwnd_raw: u64, viewport: egui::ViewportId) -> bool {
    install_window_subclass(hwnd_raw, viewport, "viewport")
}

fn install_window_subclass(hwnd_raw: u64, viewport: egui::ViewportId, label: &'static str) -> bool {
    if hwnd_raw == 0 {
        return false;
    }
    let registration = match state().lock() {
        Ok(mut guard) => guard.register_hwnd(hwnd_raw, viewport),
        Err(_) => return false,
    };
    match registration {
        RegisterHwndResult::AlreadyRegistered => return true,
        RegisterHwndResult::ConflictingViewport(existing) => {
            crate::logger::log(format!(
                "key-input: HWND registration conflict label={label} hwnd=0x{hwnd_raw:x} \
                 existing_viewport={existing:?} requested_viewport={viewport:?}"
            ));
            return false;
        }
        RegisterHwndResult::Inserted => {}
    }
    let hwnd = HWND(hwnd_raw as *mut _);
    let ok = unsafe {
        SetWindowSubclass(
            hwnd,
            Some(main_key_input_subclass_proc),
            MAIN_KEY_INPUT_SUBCLASS_ID,
            0,
        )
        .as_bool()
    };
    if !ok {
        if let Ok(mut guard) = state().lock() {
            guard.unregister_hwnd(hwnd_raw);
        }
        crate::logger::log(format!(
            "key-input: SetWindowSubclass failed label={label} hwnd=0x{hwnd_raw:x} \
             viewport={viewport:?}"
        ));
    }
    ok
}

pub fn begin_frame() {
    if let Ok(mut guard) = state().lock() {
        guard.frame.clear();
        while let Some(edge) = guard.pending.pop_front() {
            guard.frame.push(edge);
        }
        guard.frame_active_viewports = guard.installed_hwnds.unique_viewports();
        let edge_viewports: Vec<_> = guard
            .frame
            .iter()
            .map(|edge| edge.source_viewport)
            .collect();
        for viewport in edge_viewports {
            if !guard.frame_active_viewports.contains(&viewport) {
                guard.frame_active_viewports.push(viewport);
            }
        }
    }
}

pub fn is_frame_active(viewport: egui::ViewportId) -> bool {
    state()
        .lock()
        .map(|guard| guard.frame_active_viewports.contains(&viewport))
        .unwrap_or(false)
}

pub fn frame_had_key_down(viewport: egui::ViewportId) -> bool {
    state()
        .lock()
        .map(|guard| {
            guard
                .frame
                .iter()
                .any(|edge| edge.source_viewport == viewport && edge.pressed)
        })
        .unwrap_or(false)
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ConsumeKeyDownResult {
    pub matched_count: usize,
    pub triggered_count: usize,
}

pub fn consume_key_down_with_result<F>(
    viewport: egui::ViewportId,
    allow_repeat: bool,
    mut predicate: F,
) -> ConsumeKeyDownResult
where
    F: FnMut(KeyEdge) -> bool,
{
    consume_key_down_inner(viewport, allow_repeat, false, &mut predicate)
}

/// Consume every matching key-down edge from the current frame and return how
/// many action triggers they represent.
///
/// Non-repeat edges retain their physical cardinality. Auto-repeat edges keep
/// the historical per-frame behavior: when repeats are allowed they contribute
/// at most one trigger, and only when this frame has no matching physical press.
/// This prevents a long frame from turning accumulated OS repeats into delayed
/// navigation after the key is released.
pub fn consume_all_key_down_with_result<F>(
    viewport: egui::ViewportId,
    allow_repeat: bool,
    mut predicate: F,
) -> ConsumeKeyDownResult
where
    F: FnMut(KeyEdge) -> bool,
{
    consume_key_down_inner(viewport, allow_repeat, true, &mut predicate)
}

fn consume_key_down_inner<F>(
    viewport: egui::ViewportId,
    allow_repeat: bool,
    consume_all: bool,
    predicate: &mut F,
) -> ConsumeKeyDownResult
where
    F: FnMut(KeyEdge) -> bool,
{
    state()
        .lock()
        .map(|mut guard| {
            let mut result = ConsumeKeyDownResult::default();
            let mut physical_press_count = 0;
            let mut matched_repeat = false;
            let mut index = 0;
            while index < guard.frame.len() {
                let edge = guard.frame[index];
                if edge.source_viewport == viewport && edge.pressed && predicate(edge) {
                    result.matched_count += 1;
                    if edge.repeat {
                        matched_repeat = true;
                    } else {
                        physical_press_count += 1;
                    }
                    guard.frame.remove(index);
                    if !consume_all && allow_repeat {
                        break;
                    }
                } else {
                    index += 1;
                }
            }
            result.triggered_count = if consume_all {
                if physical_press_count > 0 {
                    physical_press_count
                } else {
                    usize::from(allow_repeat && matched_repeat)
                }
            } else {
                usize::from(physical_press_count > 0 || (allow_repeat && matched_repeat))
            };
            result
        })
        .unwrap_or_default()
}

pub fn consume_key_down<F>(viewport: egui::ViewportId, allow_repeat: bool, predicate: F) -> bool
where
    F: FnMut(KeyEdge) -> bool,
{
    consume_key_down_with_result(viewport, allow_repeat, predicate).triggered_count > 0
}

#[cfg(test)]
pub fn set_test_frame(edges: Vec<KeyEdge>) {
    set_test_frame_for_viewport(egui::ViewportId::ROOT, edges);
}

#[cfg(test)]
pub fn set_test_frame_for_viewport(viewport: egui::ViewportId, mut edges: Vec<KeyEdge>) {
    for edge in &mut edges {
        edge.source_viewport = viewport;
    }
    if let Ok(mut guard) = state().lock() {
        guard.frame = edges;
        guard.frame_active_viewports = vec![viewport];
    }
}

#[cfg(test)]
fn set_test_routed_frame(edges: Vec<KeyEdge>) {
    if let Ok(mut guard) = state().lock() {
        guard.frame_active_viewports.clear();
        for edge in &edges {
            if !guard.frame_active_viewports.contains(&edge.source_viewport) {
                guard.frame_active_viewports.push(edge.source_viewport);
            }
        }
        guard.frame = edges;
    }
}

#[cfg(test)]
pub fn clear_test_frame() {
    if let Ok(mut guard) = state().lock() {
        guard.frame.clear();
        guard.frame_active_viewports.clear();
        guard.return_keys.clear();
    }
}

#[cfg(test)]
pub fn set_test_return_key_state(viewport: egui::ViewportId, main_down: bool, numpad_down: bool) {
    if let Ok(mut guard) = state().lock() {
        guard.return_keys.clear_viewport(viewport);
        for (extended, pressed) in [(false, main_down), (true, numpad_down)] {
            if !pressed {
                continue;
            }
            let edge = KeyEdge {
                source_hwnd: 1,
                source_viewport: viewport,
                virtual_key: 0x0D,
                scan_code: 0x1C,
                extended,
                pressed: true,
                repeat: false,
                ctrl: false,
                shift: false,
                alt: false,
            };
            guard.return_keys.apply_edge(viewport, &edge);
        }
    }
}

#[cfg(test)]
pub(crate) static TEST_INPUT_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

pub fn pressed_key_down<F>(viewport: egui::ViewportId, predicate: F) -> bool
where
    F: Fn(KeyEdge) -> bool,
{
    state()
        .lock()
        .map(|guard| {
            guard
                .frame
                .iter()
                .any(|edge| edge.source_viewport == viewport && edge.pressed && predicate(*edge))
        })
        .unwrap_or(false)
}

/// Consume all matching physical key edges from the current frame.
///
/// Unlike egui's `Event::Key`, the Win32 edge retains scan-code and extended-bit
/// information, so callers can distinguish main Enter from numpad Enter on both
/// key-down and key-up.
pub fn consume_key_edges<F>(viewport: egui::ViewportId, mut predicate: F) -> (bool, bool)
where
    F: FnMut(KeyEdge) -> bool,
{
    state()
        .lock()
        .map(|mut guard| {
            let mut pressed = false;
            let mut released = false;
            let mut index = 0;
            while index < guard.frame.len() {
                let edge = guard.frame[index];
                if edge.source_viewport == viewport && predicate(edge) {
                    guard.frame.remove(index);
                    if edge.pressed {
                        if !edge.repeat {
                            pressed = true;
                        }
                    } else {
                        released = true;
                    }
                } else {
                    index += 1;
                }
            }
            (pressed, released)
        })
        .unwrap_or((false, false))
}

/// Return the source-routed physical held state for VK_RETURN, split by the
/// WM_KEY* extended bit (`false` = main Enter, `true` = numpad Enter).
///
/// `None` means this viewport has no subclass-routed input source in the
/// current frame, so callers must not infer a held key from process-global OS
/// state.
pub fn routed_return_key_held(viewport: egui::ViewportId, extended: bool) -> Option<bool> {
    state()
        .lock()
        .ok()
        .and_then(|guard| guard.routed_return_key_held(viewport, extended))
}

fn push_edge(hwnd_raw: u64, raw: RawKeyEdge) {
    let Ok(mut guard) = state().lock() else {
        return;
    };
    let (edge, should_log_unregistered) = guard.enqueue_raw_edge(hwnd_raw, raw);
    drop(guard);
    if should_log_unregistered {
        crate::logger::log(format!(
            "key-input: edge from unregistered HWND routed to ROOT hwnd=0x{hwnd_raw:x}"
        ));
    }
    crate::key_debug::record_raw_edge(crate::key_debug::KeyDebugSource::MainWin32, edge);
}

fn key_state(vk: windows::Win32::UI::Input::KeyboardAndMouse::VIRTUAL_KEY) -> bool {
    unsafe { GetKeyState(vk.0 as i32) < 0 }
}

fn key_edge_from_message(msg: u32, wparam: WPARAM, lparam: LPARAM) -> Option<RawKeyEdge> {
    let pressed = matches!(msg, WM_KEYDOWN | WM_SYSKEYDOWN);
    if !pressed && !matches!(msg, WM_KEYUP | WM_SYSKEYUP) {
        return None;
    }
    let raw = lparam.0 as u64;
    Some(RawKeyEdge {
        virtual_key: wparam.0 as u32,
        scan_code: ((raw >> 16) & 0xff) as u16,
        extended: (raw & (1 << 24)) != 0,
        pressed,
        repeat: (raw & (1 << 30)) != 0,
        ctrl: key_state(VK_CONTROL),
        shift: key_state(VK_SHIFT),
        alt: key_state(VK_MENU),
    })
}

unsafe extern "system" fn main_key_input_subclass_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _id: usize,
    _ref_data: usize,
) -> LRESULT {
    let hwnd_raw = hwnd.0 as u64;
    if let Some(edge) = key_edge_from_message(msg, wparam, lparam) {
        push_edge(hwnd_raw, edge);
    } else if msg == WM_KILLFOCUS {
        if let Ok(mut guard) = state().lock() {
            // A key-up can be delivered to another HWND after focus moves. Do
            // not let an Enter flavor remain latched in a later frame.
            let viewport = guard
                .installed_hwnds
                .viewport_for_hwnd(hwnd_raw)
                .unwrap_or(egui::ViewportId::ROOT);
            guard.return_keys.clear_viewport(viewport);
        }
    } else if msg == WM_NCDESTROY
        && let Ok(mut guard) = state().lock()
    {
        guard.unregister_hwnd(hwnd_raw);
    }
    unsafe { DefSubclassProc(hwnd, msg, wparam, lparam) }
}

#[cfg(test)]
mod tests {
    use super::{
        KeyEdge, KeyInputState, RawKeyEdge, RegisterHwndResult, ReturnKeyState, TEST_INPUT_LOCK,
        begin_frame, consume_key_down, pressed_key_down, set_test_frame, set_test_routed_frame,
    };

    fn raw_edge(virtual_key: u32, pressed: bool) -> RawKeyEdge {
        RawKeyEdge {
            virtual_key,
            scan_code: 0x1C,
            extended: false,
            pressed,
            repeat: false,
            ctrl: false,
            shift: false,
            alt: false,
        }
    }

    fn return_edge(extended: bool, pressed: bool) -> KeyEdge {
        KeyEdge {
            source_hwnd: 1,
            source_viewport: egui::ViewportId::ROOT,
            virtual_key: 0x0D,
            scan_code: 0x1C,
            extended,
            pressed,
            repeat: false,
            ctrl: false,
            shift: false,
            alt: false,
        }
    }

    #[test]
    fn return_key_latch_distinguishes_main_and_numpad_enter() {
        let mut state = ReturnKeyState::default();

        state.apply_edge(&return_edge(true, true));
        assert!(!state.is_down(false));
        assert!(state.is_down(true));

        state.apply_edge(&return_edge(false, true));
        assert!(state.is_down(false));
        assert!(state.is_down(true));

        state.apply_edge(&return_edge(true, false));
        assert!(state.is_down(false));
        assert!(!state.is_down(true));

        state.apply_edge(&return_edge(false, false));
        assert!(!state.is_down(false));
        assert!(!state.is_down(true));
    }

    #[test]
    fn return_key_latch_clear_drops_stale_focus_state() {
        let mut state = ReturnKeyState::default();
        state.apply_edge(&return_edge(false, true));
        state.apply_edge(&return_edge(true, true));

        state.clear();

        assert!(!state.is_down(false));
        assert!(!state.is_down(true));
    }

    #[test]
    fn routed_return_key_hold_requires_the_source_viewport_to_be_active() {
        let mut input = KeyInputState::default();
        let source = egui::ViewportId::from_hash_of(3_u64);
        let sibling = egui::ViewportId::from_hash_of(4_u64);
        let mut edge = return_edge(false, true);
        edge.source_viewport = source;
        input.return_keys.apply_edge(source, &edge);

        assert_eq!(input.routed_return_key_held(source, false), None);

        input.frame_active_viewports.push(source);
        assert_eq!(input.routed_return_key_held(source, false), Some(true));
        assert_eq!(input.routed_return_key_held(source, true), Some(false));
        assert_eq!(input.routed_return_key_held(sibling, false), None);
    }

    #[test]
    fn unconsumed_frame_edges_expire_at_next_begin_frame() {
        let _serial = TEST_INPUT_LOCK
            .get_or_init(|| std::sync::Mutex::new(()))
            .lock()
            .expect("key input test lock poisoned");
        set_test_frame(vec![
            KeyEdge {
                source_hwnd: 1,
                source_viewport: egui::ViewportId::ROOT,
                virtual_key: 0x28,
                scan_code: 0x50,
                extended: true,
                pressed: true,
                repeat: false,
                ctrl: true,
                shift: false,
                alt: false,
            },
            KeyEdge {
                source_hwnd: 1,
                source_viewport: egui::ViewportId::ROOT,
                virtual_key: 0x28,
                scan_code: 0x50,
                extended: true,
                pressed: true,
                repeat: false,
                ctrl: true,
                shift: false,
                alt: false,
            },
        ]);

        assert!(consume_key_down(egui::ViewportId::ROOT, true, |edge| {
            edge.virtual_key == 0x28
        }));
        assert!(pressed_key_down(egui::ViewportId::ROOT, |edge| {
            edge.virtual_key == 0x28
        }));

        begin_frame();

        assert!(!pressed_key_down(egui::ViewportId::ROOT, |edge| {
            edge.virtual_key == 0x28
        }));
    }

    #[test]
    fn different_viewport_cannot_consume_source_edge() {
        let _serial = TEST_INPUT_LOCK
            .get_or_init(|| std::sync::Mutex::new(()))
            .lock()
            .expect("key input test lock poisoned");
        let source = egui::ViewportId::from_hash_of("key-source");
        let sibling = egui::ViewportId::from_hash_of("key-sibling");
        let edge = raw_edge(0x25, true).with_source(0x101, source);
        set_test_routed_frame(vec![edge]);

        assert!(!consume_key_down(sibling, true, |_| true));
        assert!(pressed_key_down(source, |_| true));
        assert!(consume_key_down(source, true, |_| true));
    }

    #[test]
    fn hwnd_registration_and_removal_leave_no_stale_mapping_or_edge() {
        let mut input = KeyInputState::default();
        let viewport = egui::ViewportId::from_hash_of("registered-viewport");

        assert_eq!(
            input.register_hwnd(0x201, viewport),
            RegisterHwndResult::Inserted
        );
        assert_eq!(
            input.installed_hwnds.viewport_for_hwnd(0x201),
            Some(viewport)
        );
        input.enqueue_raw_edge(0x201, raw_edge(0x26, true));

        assert_eq!(input.unregister_hwnd(0x201), Some(viewport));
        assert_eq!(input.installed_hwnds.viewport_for_hwnd(0x201), None);
        assert!(input.pending.is_empty());

        let replacement = egui::ViewportId::from_hash_of("replacement-viewport");
        assert_eq!(
            input.register_hwnd(0x201, replacement),
            RegisterHwndResult::Inserted
        );
        assert_eq!(
            input.installed_hwnds.viewport_for_hwnd(0x201),
            Some(replacement)
        );
    }

    #[test]
    fn unregistered_hwnd_edge_is_explicitly_routed_to_root() {
        let mut input = KeyInputState::default();
        let (edge, should_log) = input.enqueue_raw_edge(0x301, raw_edge(0x27, true));

        assert_eq!(edge.source_hwnd, 0x301);
        assert_eq!(edge.source_viewport, egui::ViewportId::ROOT);
        assert!(should_log);
        assert_eq!(input.pending.pop_front(), Some(edge));
        let (_, should_log_again) = input.enqueue_raw_edge(0x301, raw_edge(0x27, false));
        assert!(!should_log_again, "one diagnostic per unregistered HWND");
    }
}
