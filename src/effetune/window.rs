//! Main-window events used only by the unowned EffeTune editor.
//! No IPC or plugin work runs in the native window procedure.

use std::sync::{Arc, Mutex, mpsc};

use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::UI::Input::Pointer::{GetPointerInfo, POINTER_FLAG_PRIMARY, POINTER_INFO};
use windows::Win32::UI::Input::Touch::{
    GetTouchInputInfo, HTOUCHINPUT, TOUCHEVENTF_DOWN, TOUCHEVENTF_PRIMARY, TOUCHINPUT,
};
use windows::Win32::UI::Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass};
use windows::Win32::UI::WindowsAndMessaging::{
    GetForegroundWindow, HTCLIENT, IsIconic, IsWindowVisible, WA_CLICKACTIVE, WA_INACTIVE,
    WM_ACTIVATE, WM_CANCELMODE, WM_LBUTTONDBLCLK, WM_LBUTTONDOWN, WM_MBUTTONDBLCLK, WM_MBUTTONDOWN,
    WM_MOUSEACTIVATE, WM_NCDESTROY, WM_NCLBUTTONDBLCLK, WM_NCLBUTTONDOWN, WM_NCMBUTTONDBLCLK,
    WM_NCMBUTTONDOWN, WM_NCRBUTTONDBLCLK, WM_NCRBUTTONDOWN, WM_NCXBUTTONDBLCLK, WM_NCXBUTTONDOWN,
    WM_POINTERACTIVATE, WM_POINTERDOWN, WM_RBUTTONDBLCLK, WM_RBUTTONDOWN, WM_SHOWWINDOW, WM_SIZE,
    WM_TOUCH, WM_WINDOWPOSCHANGED, WM_XBUTTONDBLCLK, WM_XBUTTONDOWN,
};

use super::{HostCommand, gui_gate::GuiGate};

const SUBCLASS_ID: usize = 0x4546_4645;

#[derive(Default)]
enum ClickForeground {
    #[default]
    None,
    Activating(u64),
    Pressed(u64),
}

impl ClickForeground {
    fn activating(&mut self, foreground: u64) {
        *self = Self::Activating(foreground);
    }

    fn pressed(&mut self, foreground: u64) {
        let before = match *self {
            Self::Activating(before) => before,
            _ => foreground,
        };
        *self = Self::Pressed(before);
    }

    fn take(&mut self, foreground: u64, pointer_click: bool) -> u64 {
        match std::mem::take(self) {
            Self::Pressed(before) if pointer_click => before,
            _ => foreground,
        }
    }
}

pub(super) struct MainWindowObserver {
    notify: mpsc::Sender<HostCommand>,
    click: Mutex<ClickForeground>,
    gate: Result<Arc<GuiGate>, String>,
}

impl MainWindowObserver {
    pub fn new(notify: mpsc::Sender<HostCommand>) -> Self {
        Self {
            gate: GuiGate::create(Some(notify.clone())),
            notify,
            click: Mutex::new(ClickForeground::default()),
        }
    }

    pub fn gate(&self) -> Result<Arc<GuiGate>, String> {
        self.gate.clone()
    }

    fn notify_visibility(&self) {
        let _ = self.notify.send(HostCommand::ReconcileVisibility);
    }
    pub fn install(self: &Arc<Self>, hwnd: u64) {
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
        }
    }

    pub fn minimize_sequence(&self) -> u64 {
        self.gate
            .as_ref()
            .map_or(0, |gate| gate.minimized_sequence())
    }

    pub fn click_foreground(&self, pointer_click: bool) -> u64 {
        let foreground = unsafe { GetForegroundWindow() }.0 as u64;
        self.click.lock().unwrap().take(foreground, pointer_click)
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
    match message {
        WM_MOUSEACTIVATE
            if (lparam.0 >> 16) as u32 == WM_LBUTTONDOWN
                && (lparam.0 as u32 & 0xffff) == HTCLIENT =>
        {
            observer
                .click
                .lock()
                .unwrap()
                .activating(unsafe { GetForegroundWindow() }.0 as u64);
        }
        WM_LBUTTONDOWN | WM_LBUTTONDBLCLK => observer
            .click
            .lock()
            .unwrap()
            .pressed(unsafe { GetForegroundWindow() }.0 as u64),
        WM_POINTERACTIVATE => observer
            .click
            .lock()
            .unwrap()
            .activating(unsafe { GetForegroundWindow() }.0 as u64),
        WM_POINTERDOWN => {
            let mut info = POINTER_INFO::default();
            if unsafe { GetPointerInfo((wparam.0 & 0xffff) as u32, &mut info) }.is_ok()
                && info.pointerFlags & POINTER_FLAG_PRIMARY == POINTER_FLAG_PRIMARY
            {
                observer
                    .click
                    .lock()
                    .unwrap()
                    .pressed(unsafe { GetForegroundWindow() }.0 as u64);
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
                && inputs.iter().any(|input| {
                    input.dwFlags & TOUCHEVENTF_PRIMARY == TOUCHEVENTF_PRIMARY
                        && input.dwFlags & TOUCHEVENTF_DOWN == TOUCHEVENTF_DOWN
                })
            {
                observer
                    .click
                    .lock()
                    .unwrap()
                    .pressed(unsafe { GetForegroundWindow() }.0 as u64);
            }
        }
        WM_ACTIVATE if wparam.0 & 0xffff == WA_CLICKACTIVE as usize => {
            let mut click = observer.click.lock().unwrap();
            if !matches!(*click, ClickForeground::Activating(_)) {
                click.activating(lparam.0 as u64);
            }
        }
        WM_ACTIVATE if wparam.0 & 0xffff == WA_INACTIVE as usize => {
            *observer.click.lock().unwrap() = ClickForeground::None
        }
        WM_CANCELMODE => *observer.click.lock().unwrap() = ClickForeground::None,
        WM_NCLBUTTONDOWN | WM_NCLBUTTONDBLCLK | WM_RBUTTONDOWN | WM_RBUTTONDBLCLK
        | WM_MBUTTONDOWN | WM_MBUTTONDBLCLK | WM_XBUTTONDOWN | WM_XBUTTONDBLCLK
        | WM_NCRBUTTONDOWN | WM_NCRBUTTONDBLCLK | WM_NCMBUTTONDOWN | WM_NCMBUTTONDBLCLK
        | WM_NCXBUTTONDOWN | WM_NCXBUTTONDBLCLK => {
            *observer.click.lock().unwrap() = ClickForeground::None
        }
        WM_MOUSEACTIVATE => *observer.click.lock().unwrap() = ClickForeground::None,
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
            if let Ok(gate) = &observer.gate {
                gate.set_auto_factor(super::gui_gate::AutoSuppression::RootHidden, true);
            }
            let _ = RemoveWindowSubclass(hwnd, Some(subclass), SUBCLASS_ID);
            drop(Arc::from_raw(data as *const MainWindowObserver));
        },
        _ => {}
    }
    unsafe { DefSubclassProc(hwnd, message, wparam, lparam) }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        click.activating(20);
        click.pressed(10);
        assert_eq!(click.take(10, true), 20);
        assert_eq!(click.take(10, true), 10);
    }

    #[test]
    fn next_press_replaces_an_unconsumed_background_click() {
        let mut click = ClickForeground::default();
        click.activating(20);
        click.pressed(10);
        click.pressed(10);
        assert_eq!(click.take(10, true), 10);
    }

    #[test]
    fn keyboard_action_discards_old_pointer_snapshot() {
        let mut click = ClickForeground::default();
        click.activating(20);
        click.pressed(10);
        assert_eq!(click.take(10, false), 10);
        assert_eq!(click.take(10, true), 10);
    }

    #[test]
    fn touch_activation_and_cross_input_presses_replace_completed_mouse_click() {
        let mut click = ClickForeground::default();
        click.activating(20);
        click.pressed(10); // old mouse press, unused by toolbar
        click.pressed(10); // primary touch press while main is already active
        assert_eq!(click.take(10, true), 10);
        click.activating(20); // WM_POINTERACTIVATE / legacy click activation
        click.pressed(10); // primary POINTERDOWN / TOUCH down
        assert_eq!(click.take(10, true), 20);
    }
}
