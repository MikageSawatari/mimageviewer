//! Main-window events used only by the unowned EffeTune editor.
//! No IPC or plugin work runs in the native window procedure.

use std::sync::{Arc, mpsc};

use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::UI::Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass};
use windows::Win32::UI::WindowsAndMessaging::{
    IsIconic, IsWindowVisible, WM_NCDESTROY, WM_SHOWWINDOW, WM_SIZE, WM_WINDOWPOSCHANGED,
};

use super::{HostCommand, gui_gate::GuiGate};

const SUBCLASS_ID: usize = 0x4546_4645;

pub(super) struct MainWindowObserver {
    notify: mpsc::Sender<HostCommand>,
    gate: Result<Arc<GuiGate>, String>,
}

impl MainWindowObserver {
    pub fn new(notify: mpsc::Sender<HostCommand>) -> Self {
        Self {
            gate: GuiGate::create(Some(notify.clone())),
            notify,
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
}
