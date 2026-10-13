//! Main-window events used only by the unowned EffeTune editor.
//! No IPC or plugin work runs in the native window procedure.

use std::sync::{Arc, Mutex, mpsc};

use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::UI::Input::Pointer::{GetPointerInfo, POINTER_FLAG_PRIMARY, POINTER_INFO};
use windows::Win32::UI::Input::Touch::{
    GetTouchInputInfo, HTOUCHINPUT, TOUCHEVENTF_DOWN, TOUCHEVENTF_PRIMARY, TOUCHEVENTF_UP,
    TOUCHINPUT,
};
use windows::Win32::UI::Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass};
use windows::Win32::UI::WindowsAndMessaging::{
    GetForegroundWindow, HTCLIENT, IsIconic, IsWindowVisible, WA_CLICKACTIVE, WA_INACTIVE,
    WM_ACTIVATE, WM_CANCELMODE, WM_LBUTTONDBLCLK, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MBUTTONDBLCLK,
    WM_MBUTTONDOWN, WM_MOUSEACTIVATE, WM_NCDESTROY, WM_NCLBUTTONDBLCLK, WM_NCLBUTTONDOWN,
    WM_NCMBUTTONDBLCLK, WM_NCMBUTTONDOWN, WM_NCRBUTTONDBLCLK, WM_NCRBUTTONDOWN, WM_NCXBUTTONDBLCLK,
    WM_NCXBUTTONDOWN, WM_POINTERACTIVATE, WM_POINTERDOWN, WM_POINTERUP, WM_RBUTTONDBLCLK,
    WM_RBUTTONDOWN, WM_SHOWWINDOW, WM_SIZE, WM_TOUCH, WM_WINDOWPOSCHANGED, WM_XBUTTONDBLCLK,
    WM_XBUTTONDOWN,
};

use super::{HostCommand, gui_gate::GuiGate};

const SUBCLASS_ID: usize = 0x4546_4645;

#[derive(Debug, Default)]
enum ClickForeground {
    #[default]
    None,
    Activating(u64),
    Pressed(u64),
    Released(u64),
    // Consumed or cancelled gestures cannot be reopened by a late activation.
    Finished,
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
        let mut click = self.click.lock().unwrap();
        let provenance = format!("{click:?}");
        let captured = click.take(foreground, pointer_click);
        drop(click);
        crate::logger::log(format!(
            "[EffeTune GUI] button input pointer={pointer_click} provenance={provenance} captured=0x{captured:x} current=0x{foreground:x}"
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
    if let Some(event) = mouse_click_event(
        message,
        wparam.0,
        lparam.0,
        || unsafe { GetForegroundWindow() }.0 as u64,
    ) {
        observer.click.lock().unwrap().event(event);
    }
    match message {
        WM_POINTERACTIVATE => observer.click.lock().unwrap().event(ClickEvent::Activating(
            unsafe { GetForegroundWindow() }.0 as u64,
        )),
        WM_POINTERDOWN | WM_POINTERUP => {
            let mut info = POINTER_INFO::default();
            if unsafe { GetPointerInfo((wparam.0 & 0xffff) as u32, &mut info) }.is_ok()
                && info.pointerFlags & POINTER_FLAG_PRIMARY == POINTER_FLAG_PRIMARY
            {
                observer
                    .click
                    .lock()
                    .unwrap()
                    .event(if message == WM_POINTERDOWN {
                        ClickEvent::Down(unsafe { GetForegroundWindow() }.0 as u64)
                    } else {
                        ClickEvent::Up
                    });
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
                for input in inputs
                    .iter()
                    .filter(|input| input.dwFlags & TOUCHEVENTF_PRIMARY == TOUCHEVENTF_PRIMARY)
                {
                    let event = if input.dwFlags & TOUCHEVENTF_DOWN == TOUCHEVENTF_DOWN {
                        ClickEvent::Down(unsafe { GetForegroundWindow() }.0 as u64)
                    } else if input.dwFlags & TOUCHEVENTF_UP == TOUCHEVENTF_UP {
                        ClickEvent::Up
                    } else {
                        continue;
                    };
                    observer.click.lock().unwrap().event(event);
                }
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
    use crate::effetune::{GuiButtonAction, gui_button_action};

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
