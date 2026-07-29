//! Main-window Win32 key edge queue.
//!
//! egui flattens some physical keys (notably numpad digits and JIS-specific
//! keys).  The keymap still lets egui handle text/IME normally, but shortcut
//! matching reads key-down edges from this queue when the main window subclass
//! is installed.

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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KeyEdge {
    pub virtual_key: u32,
    pub scan_code: u16,
    pub extended: bool,
    pub pressed: bool,
    pub repeat: bool,
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
}

#[derive(Default)]
struct ReturnKeyState {
    main_down: bool,
    numpad_down: bool,
}

impl ReturnKeyState {
    fn apply_edge(&mut self, edge: KeyEdge) {
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

    fn clear(&mut self) {
        self.main_down = false;
        self.numpad_down = false;
    }
}

#[derive(Default)]
struct KeyInputState {
    installed_hwnds: Vec<u64>,
    pending: VecDeque<KeyEdge>,
    frame: Vec<KeyEdge>,
    frame_active: bool,
    frame_had_key_down: bool,
    return_keys: ReturnKeyState,
}

fn state() -> &'static Mutex<KeyInputState> {
    static STATE: OnceLock<Mutex<KeyInputState>> = OnceLock::new();
    STATE.get_or_init(|| Mutex::new(KeyInputState::default()))
}

pub fn install_main_window_subclass(hwnd_raw: u64) -> bool {
    install_window_subclass(hwnd_raw, "main")
}

pub fn install_viewport_window_subclass(hwnd_raw: u64) -> bool {
    install_window_subclass(hwnd_raw, "viewport")
}

fn install_window_subclass(hwnd_raw: u64, label: &'static str) -> bool {
    if hwnd_raw == 0 {
        return false;
    }
    if state()
        .lock()
        .map(|guard| guard.installed_hwnds.contains(&hwnd_raw))
        .unwrap_or(false)
    {
        return true;
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
    if ok {
        if let Ok(mut guard) = state().lock() {
            if !guard.installed_hwnds.contains(&hwnd_raw) {
                guard.installed_hwnds.push(hwnd_raw);
            }
        }
    } else {
        crate::logger::log(format!(
            "key-input: SetWindowSubclass failed label={label} hwnd=0x{hwnd_raw:x}"
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
        guard.frame_had_key_down = guard.frame.iter().any(|edge| edge.pressed);
        guard.frame_active = !guard.installed_hwnds.is_empty();
    }
}

pub fn is_frame_active() -> bool {
    state()
        .lock()
        .map(|guard| guard.frame_active)
        .unwrap_or(false)
}

pub fn frame_had_key_down() -> bool {
    state()
        .lock()
        .map(|guard| guard.frame_had_key_down)
        .unwrap_or(false)
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ConsumeKeyDownResult {
    pub matched_count: usize,
    pub triggered_count: usize,
}

pub fn consume_key_down_with_result<F>(allow_repeat: bool, mut predicate: F) -> ConsumeKeyDownResult
where
    F: FnMut(KeyEdge) -> bool,
{
    consume_key_down_inner(allow_repeat, false, &mut predicate)
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
    allow_repeat: bool,
    mut predicate: F,
) -> ConsumeKeyDownResult
where
    F: FnMut(KeyEdge) -> bool,
{
    consume_key_down_inner(allow_repeat, true, &mut predicate)
}

fn consume_key_down_inner<F>(
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
                if edge.pressed && predicate(edge) {
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

pub fn consume_key_down<F>(allow_repeat: bool, predicate: F) -> bool
where
    F: FnMut(KeyEdge) -> bool,
{
    consume_key_down_with_result(allow_repeat, predicate).triggered_count > 0
}

#[cfg(test)]
pub fn set_test_frame(edges: Vec<KeyEdge>) {
    if let Ok(mut guard) = state().lock() {
        guard.frame = edges;
        guard.frame_had_key_down = guard.frame.iter().any(|edge| edge.pressed);
        guard.frame_active = true;
    }
}

#[cfg(test)]
pub fn clear_test_frame() {
    if let Ok(mut guard) = state().lock() {
        guard.frame.clear();
        guard.frame_active = false;
        guard.frame_had_key_down = false;
    }
}

#[cfg(test)]
pub(crate) static TEST_INPUT_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

pub fn pressed_key_down<F>(predicate: F) -> bool
where
    F: Fn(KeyEdge) -> bool,
{
    state()
        .lock()
        .map(|guard| {
            guard
                .frame
                .iter()
                .any(|edge| edge.pressed && predicate(*edge))
        })
        .unwrap_or(false)
}

/// Consume all matching physical key edges from the current frame.
///
/// Unlike egui's `Event::Key`, the Win32 edge retains scan-code and extended-bit
/// information, so callers can distinguish main Enter from numpad Enter on both
/// key-down and key-up.
pub fn consume_key_edges<F>(mut predicate: F) -> (bool, bool)
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
                if predicate(edge) {
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

/// Return the physical held state for VK_RETURN, split by the WM_KEY* extended
/// bit (`false` = main Enter, `true` = numpad Enter).
pub fn return_key_held(extended: bool) -> bool {
    state()
        .lock()
        .map(|guard| guard.return_keys.is_down(extended))
        .unwrap_or(false)
}

fn push_edge(edge: KeyEdge) {
    crate::key_debug::record_raw_edge(crate::key_debug::KeyDebugSource::MainWin32, edge);
    if let Ok(mut guard) = state().lock() {
        guard.return_keys.apply_edge(edge);
        while guard.pending.len() >= MAX_PENDING_EVENTS {
            guard.pending.pop_front();
        }
        guard.pending.push_back(edge);
    }
}

fn key_state(vk: windows::Win32::UI::Input::KeyboardAndMouse::VIRTUAL_KEY) -> bool {
    unsafe { GetKeyState(vk.0 as i32) < 0 }
}

fn key_edge_from_message(msg: u32, wparam: WPARAM, lparam: LPARAM) -> Option<KeyEdge> {
    let pressed = matches!(msg, WM_KEYDOWN | WM_SYSKEYDOWN);
    if !pressed && !matches!(msg, WM_KEYUP | WM_SYSKEYUP) {
        return None;
    }
    let raw = lparam.0 as u64;
    Some(KeyEdge {
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
    if let Some(edge) = key_edge_from_message(msg, wparam, lparam) {
        push_edge(edge);
    } else if msg == WM_KILLFOCUS {
        if let Ok(mut guard) = state().lock() {
            // A key-up can be delivered to another HWND after focus moves. Do
            // not let an Enter flavor remain latched in a later frame.
            guard.return_keys.clear();
        }
    } else if msg == WM_NCDESTROY
        && let Ok(mut guard) = state().lock()
    {
        let hwnd_raw = hwnd.0 as u64;
        guard
            .installed_hwnds
            .retain(|installed| *installed != hwnd_raw);
        if guard.installed_hwnds.is_empty() {
            guard.pending.clear();
            guard.frame.clear();
            guard.frame_active = false;
            guard.frame_had_key_down = false;
            guard.return_keys.clear();
        }
    }
    unsafe { DefSubclassProc(hwnd, msg, wparam, lparam) }
}

#[cfg(test)]
mod tests {
    use super::{
        KeyEdge, ReturnKeyState, TEST_INPUT_LOCK, begin_frame, consume_key_down, pressed_key_down,
        set_test_frame,
    };

    fn return_edge(extended: bool, pressed: bool) -> KeyEdge {
        KeyEdge {
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

        state.apply_edge(return_edge(true, true));
        assert!(!state.is_down(false));
        assert!(state.is_down(true));

        state.apply_edge(return_edge(false, true));
        assert!(state.is_down(false));
        assert!(state.is_down(true));

        state.apply_edge(return_edge(true, false));
        assert!(state.is_down(false));
        assert!(!state.is_down(true));

        state.apply_edge(return_edge(false, false));
        assert!(!state.is_down(false));
        assert!(!state.is_down(true));
    }

    #[test]
    fn return_key_latch_clear_drops_stale_focus_state() {
        let mut state = ReturnKeyState::default();
        state.apply_edge(return_edge(false, true));
        state.apply_edge(return_edge(true, true));

        state.clear();

        assert!(!state.is_down(false));
        assert!(!state.is_down(true));
    }

    #[test]
    fn unconsumed_frame_edges_expire_at_next_begin_frame() {
        let _serial = TEST_INPUT_LOCK
            .get_or_init(|| std::sync::Mutex::new(()))
            .lock()
            .expect("key input test lock poisoned");
        set_test_frame(vec![
            KeyEdge {
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

        assert!(consume_key_down(true, |edge| edge.virtual_key == 0x28));
        assert!(pressed_key_down(|edge| edge.virtual_key == 0x28));

        begin_frame();

        assert!(!pressed_key_down(|edge| edge.virtual_key == 0x28));
    }
}
