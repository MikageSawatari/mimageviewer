//! Main HWND activation shared by tray, second-instance requests and capture popup.
//! Viewer routing and App restore synchronization remain with their existing owners.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ActivationState {
    TrayHidden,
    Minimized,
    Visible,
}

fn activation_state(hidden: bool, iconic: bool) -> ActivationState {
    if hidden {
        ActivationState::TrayHidden
    } else if iconic {
        ActivationState::Minimized
    } else {
        ActivationState::Visible
    }
}

/// Restores only the main native window. The caller publishes its UI event and App
/// retains `sync_after_restore`; this helper never switches viewer contexts.
pub(crate) fn activate_main_window(
    hwnd_raw: isize,
    placement_slot: &crate::tray::PlacementSlot,
    ctx: &eframe::egui::Context,
    reason: &'static str,
) -> ActivationState {
    #[cfg(windows)]
    {
        use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
        use windows::Win32::UI::WindowsAndMessaging::{
            IsIconic, IsWindowVisible, PostMessageW, SW_RESTORE, SW_SHOW, WM_PAINT,
        };
        let hwnd = HWND(hwnd_raw as *mut _);
        // Win32 can reenter window procedures or wait for the UI thread. Never
        // hold this slot's mutex while invoking any native window operation.
        let saved = placement_slot.lock().unwrap().take();
        let was_hidden = unsafe { !IsWindowVisible(hwnd).as_bool() };
        let state = activation_state(was_hidden, unsafe { IsIconic(hwnd).as_bool() });
        match state {
            ActivationState::TrayHidden => {
                if let Some(saved) = saved {
                    // Saved placement already includes the correct showCmd.
                    // Avoid SW_RESTORE here to preserve the tray flash/DPI fix.
                    crate::tray::restore_window_placement(hwnd_raw, &saved);
                } else {
                    // Existing behavior when GetWindowPlacement failed at hide:
                    // the HWND is still hidden and must be made visible.
                    crate::logger::log(format!("{reason}: hidden window has no saved placement"));
                    unsafe {
                        let _ = crate::presentation_observer::show_window(
                            hwnd,
                            SW_SHOW,
                            crate::presentation_observer::WindowRole::Main,
                            reason,
                        );
                    }
                }
            }
            ActivationState::Minimized => unsafe {
                let _ = crate::presentation_observer::show_window(
                    hwnd,
                    SW_RESTORE,
                    crate::presentation_observer::WindowRole::Main,
                    reason,
                );
            },
            ActivationState::Visible => {}
        }
        let foregrounded = unsafe {
            crate::presentation_observer::set_foreground_window(
                hwnd,
                crate::presentation_observer::WindowRole::Main,
                reason,
            )
        };
        if !foregrounded {
            crate::logger::log(format!("{reason}: SetForegroundWindow failed"));
        }
        if was_hidden
            && let Err(error) = unsafe { PostMessageW(Some(hwnd), WM_PAINT, WPARAM(0), LPARAM(0)) }
        {
            crate::logger::log(format!("{reason}: restore WM_PAINT post failed: {error:?}"));
        }
        ctx.request_repaint();
        state
    }
    #[cfg(not(windows))]
    {
        let _ = (hwnd_raw, placement_slot, reason);
        ctx.request_repaint();
        ActivationState::Visible
    }
}

#[cfg(test)]
mod tests {
    use super::{ActivationState, activation_state};

    #[test]
    fn main_activation_distinguishes_tray_minimized_and_visible() {
        assert_eq!(activation_state(true, false), ActivationState::TrayHidden);
        assert_eq!(activation_state(true, true), ActivationState::TrayHidden);
        assert_eq!(activation_state(false, true), ActivationState::Minimized);
        assert_eq!(activation_state(false, false), ActivationState::Visible);
    }
}
