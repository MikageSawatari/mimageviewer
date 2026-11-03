//! Native foreground calls, with diagnostics at the process boundary.

use windows::Win32::Foundation::HWND;
use windows::Win32::UI::WindowsAndMessaging::{
    AllowSetForegroundWindow, GetForegroundWindow, GetWindowThreadProcessId, IsIconic, IsWindow,
    IsWindowVisible, SetForegroundWindow,
};

#[derive(Debug)]
struct Foreground {
    hwnd: u64,
    pid: u32,
}

fn foreground() -> Foreground {
    let hwnd = unsafe { GetForegroundWindow() };
    let mut pid = 0;
    unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
    Foreground {
        hwnd: hwnd.0 as u64,
        pid,
    }
}

pub(super) fn allow_host(pid: u32) {
    let before = foreground();
    let result = unsafe { AllowSetForegroundWindow(pid) };
    let after = foreground();
    crate::logger::log(format!(
        "[VST3 GUI] AllowSetForegroundWindow target_pid={pid} result={result:?} before_hwnd={:#x} before_pid={} after_hwnd={:#x} after_pid={}",
        before.hwnd, before.pid, after.hwnd, after.pid,
    ));
}

#[derive(Debug, PartialEq, Eq)]
enum ManualPresentation {
    ActivateVisible,
    ShowViaHost,
}

fn manual_presentation(valid: bool, visible: bool, iconic: bool) -> ManualPresentation {
    if valid && visible && !iconic {
        ManualPresentation::ActivateVisible
    } else {
        ManualPresentation::ShowViaHost
    }
}

/// UI-thread only. Return whether this was an existing, visible editor action,
/// independently of foreground success. Never retry through the host on failure.
pub(crate) fn activate_visible_editor(raw: u64, expected_pid: u32) -> bool {
    let hwnd = HWND(raw as *mut _);
    let mut pid = 0;
    unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
    let action = manual_presentation(
        raw != 0 && pid == expected_pid && unsafe { IsWindow(Some(hwnd)) }.as_bool(),
        unsafe { IsWindowVisible(hwnd) }.as_bool(),
        unsafe { IsIconic(hwnd) }.as_bool(),
    );
    if action == ManualPresentation::ShowViaHost {
        return false;
    }
    let before = foreground();
    let result = unsafe { SetForegroundWindow(hwnd) }.as_bool();
    let after = foreground();
    crate::logger::log(format!(
        "[EffeTune GUI] SetForegroundWindow target_hwnd={raw:#x} target_pid={pid} result={result} before_hwnd={:#x} before_pid={} after_hwnd={:#x} after_pid={}",
        before.hwnd, before.pid, after.hwnd, after.pid,
    ));
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn effetune_manual_presentation_only_activates_an_existing_visible_restored_editor() {
        for valid in [false, true] {
            for visible in [false, true] {
                for iconic in [false, true] {
                    assert_eq!(
                        manual_presentation(valid, visible, iconic),
                        if valid && visible && !iconic {
                            ManualPresentation::ActivateVisible
                        } else {
                            ManualPresentation::ShowViaHost
                        },
                    );
                }
            }
        }
    }
}
