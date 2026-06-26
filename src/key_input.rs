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
    WM_KEYDOWN, WM_KEYUP, WM_NCDESTROY, WM_SYSKEYDOWN, WM_SYSKEYUP,
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
struct KeyInputState {
    installed_hwnd: u64,
    pending: VecDeque<KeyEdge>,
    frame: Vec<KeyEdge>,
    frame_active: bool,
    frame_had_key_down: bool,
}

fn state() -> &'static Mutex<KeyInputState> {
    static STATE: OnceLock<Mutex<KeyInputState>> = OnceLock::new();
    STATE.get_or_init(|| Mutex::new(KeyInputState::default()))
}

pub fn install_main_window_subclass(hwnd_raw: u64) -> bool {
    if hwnd_raw == 0 {
        return false;
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
            guard.installed_hwnd = hwnd_raw;
        }
    } else {
        crate::logger::log(format!(
            "key-input: SetWindowSubclass failed hwnd=0x{hwnd_raw:x}"
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
        guard.frame_active = guard.installed_hwnd != 0;
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

pub fn consume_key_down<F>(allow_repeat: bool, mut predicate: F) -> bool
where
    F: FnMut(KeyEdge) -> bool,
{
    state()
        .lock()
        .map(|mut guard| {
            let Some(index) = guard.frame.iter().position(|edge| {
                edge.pressed && (allow_repeat || !edge.repeat) && predicate(*edge)
            }) else {
                return false;
            };
            guard.frame.remove(index);
            true
        })
        .unwrap_or(false)
}

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

fn push_edge(edge: KeyEdge) {
    if let Ok(mut guard) = state().lock() {
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
    } else if msg == WM_NCDESTROY
        && let Ok(mut guard) = state().lock()
        && guard.installed_hwnd == hwnd.0 as u64
    {
        guard.installed_hwnd = 0;
        guard.pending.clear();
        guard.frame.clear();
        guard.frame_active = false;
        guard.frame_had_key_down = false;
    }
    unsafe { DefSubclassProc(hwnd, msg, wparam, lparam) }
}
