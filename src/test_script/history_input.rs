//! Root-only posted Windows input for history-return smoke scenarios.
//!
//! Messages traverse the existing hook or winit/egui intake. This module never calls a
//! navigation handler, changes pending mouse counters, or sends desktop-wide input.
use std::time::{Duration, Instant};

use rhai::{Engine, EvalAltResult, ImmutableString};

use super::{RunnerBridge, TestScriptActionSelection, TestScriptWindowIdentity, rhai_error};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum HistoryInput {
    BrowserBack,
    BrowserForward,
    AppCommandBack,
    AppCommandForward,
    MouseX1,
    MouseX2,
}

impl HistoryInput {
    fn parse(name: &str) -> Result<Self, String> {
        match name {
            "browser_back" => Ok(Self::BrowserBack),
            "browser_forward" => Ok(Self::BrowserForward),
            "appcommand_back" => Ok(Self::AppCommandBack),
            "appcommand_forward" => Ok(Self::AppCommandForward),
            "mouse_x1" => Ok(Self::MouseX1),
            "mouse_x2" => Ok(Self::MouseX2),
            _ => Err(format!("unknown root history input: {name}")),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct PostedMessage {
    message: u32,
    wparam: usize,
    lparam: isize,
}

fn message_plan(input: HistoryInput, hwnd: u64, center: [i16; 2]) -> Vec<PostedMessage> {
    // Keep plans pure so their exact Win32 payloads can be checked without an OS window.
    let message = |message, wparam, lparam| PostedMessage {
        message,
        wparam,
        lparam,
    };
    match input {
        HistoryInput::BrowserBack | HistoryInput::BrowserForward => {
            let vk = if input == HistoryInput::BrowserBack {
                0xA6
            } else {
                0xA7
            };
            vec![
                message(0x0100, vk, 1), // WM_KEYDOWN, one press, no previous-down bit
                message(0x0101, vk, 0xC000_0001_u32 as isize), // WM_KEYUP
            ]
        }
        HistoryInput::AppCommandBack | HistoryInput::AppCommandForward => {
            let command = if input == HistoryInput::AppCommandBack {
                1
            } else {
                2
            };
            // WM_APPCOMMAND: HIWORD contains FAPPCOMMAND_MOUSE and command 1/2.
            vec![message(
                0x0319,
                hwnd as usize,
                ((0x8000_u32 | command) << 16) as isize,
            )]
        }
        HistoryInput::MouseX1 | HistoryInput::MouseX2 => {
            let button = if input == HistoryInput::MouseX1 { 1 } else { 2 };
            let down_mask = if input == HistoryInput::MouseX1 {
                0x20
            } else {
                0x40
            };
            let point = (center[0] as u16 as u32 | ((center[1] as u16 as u32) << 16)) as isize;
            vec![
                message(0x020B, (button << 16) | down_mask, point), // WM_XBUTTONDOWN
                message(0x020C, button << 16, point),               // WM_XBUTTONUP
            ]
        }
    }
}

fn with_root_target<T>(
    selection: TestScriptActionSelection,
    operation: impl FnOnce(TestScriptWindowIdentity) -> Result<T, String>,
) -> Result<T, String> {
    match selection {
        TestScriptActionSelection::Targeted(owner @ TestScriptWindowIdentity::Root { .. }) => {
            operation(owner)
        }
        TestScriptActionSelection::Targeted(owner) => Err(format!(
            "root_history_input requires the Root target; selected {}",
            owner.describe()
        )),
        TestScriptActionSelection::LegacyImplicit => {
            Err("root_history_input requires select_root first".to_string())
        }
    }
}

fn post_input(bridge: &RunnerBridge, input: HistoryInput) -> Result<(), String> {
    with_root_target(bridge.action_selection()?, |owner| {
        let deadline = Instant::now() + Duration::from_secs(5);
        bridge.validate_selected_owner_fresh(&owner, deadline)?;
        bridge.require_key_target()?;
        if bridge.latest_snapshot()?.target_viewport != "ROOT" {
            return Err("root_history_input requires the focused Root input target".into());
        }
        post_to_root(bridge, &owner, input, deadline)
    })
}

#[cfg(windows)]
fn post_to_root(
    bridge: &RunnerBridge,
    owner: &TestScriptWindowIdentity,
    input: HistoryInput,
    deadline: Instant,
) -> Result<(), String> {
    use windows::Win32::Foundation::{HWND, LPARAM, RECT, WPARAM};
    use windows::Win32::UI::WindowsAndMessaging::{
        GetClientRect, GetForegroundWindow, IsWindowVisible, PostMessageW,
    };

    let hwnd = HWND(owner.hwnd() as usize as *mut _);
    if unsafe { GetForegroundWindow() } != hwnd || !unsafe { IsWindowVisible(hwnd) }.as_bool() {
        return Err("root_history_input requires the visible Root window in the foreground".into());
    }
    let mut rect = RECT::default();
    unsafe { GetClientRect(hwnd, &mut rect) }
        .map_err(|error| format!("root history GetClientRect failed: {error}"))?;
    if rect.right <= rect.left || rect.bottom <= rect.top {
        return Err("root history client rectangle is empty".into());
    }
    let center = [
        i16::try_from((i64::from(rect.left) + i64::from(rect.right)) / 2)
            .map_err(|_| "root history client X coordinate is outside Win32 mouse range")?,
        i16::try_from((i64::from(rect.top) + i64::from(rect.bottom)) / 2)
            .map_err(|_| "root history client Y coordinate is outside Win32 mouse range")?,
    ];
    bridge.validate_selected_owner_cached(owner, deadline)?;
    if unsafe { GetForegroundWindow() } != hwnd {
        return Err("Root foreground owner changed before history input posting".into());
    }
    // Post (never SendMessage): WH_GETMESSAGE must observe Browser/APPCOMMAND messages.
    // winit 0.30's XBUTTON arms return ProcResult::Value, bypassing DefWindowProc; they
    // do not synthesize a second APPCOMMAND. Do not inject or suppress either route here.
    for message in message_plan(input, owner.hwnd(), center) {
        unsafe {
            PostMessageW(
                Some(hwnd),
                message.message,
                WPARAM(message.wparam),
                LPARAM(message.lparam),
            )
        }
        .map_err(|error| format!("root history PostMessageW failed: {error}"))?;
    }
    bridge.send(super::UiCommand::Log(format!(
        "root_history_input posted input={input:?} owner={}",
        owner.describe()
    )))?;
    (bridge.wake)();
    Ok(())
}

#[cfg(not(windows))]
fn post_to_root(
    _bridge: &RunnerBridge,
    _owner: &TestScriptWindowIdentity,
    _input: HistoryInput,
    _deadline: Instant,
) -> Result<(), String> {
    Err("root_history_input requires Windows".into())
}

pub(super) fn register(engine: &mut Engine, bridge: RunnerBridge) {
    engine.register_fn(
        "root_history_input",
        move |name: ImmutableString| -> Result<(), Box<EvalAltResult>> {
            let input = HistoryInput::parse(name.as_str()).map_err(rhai_error)?;
            // A refused case has posted no navigation. Leave the script able to record it
            // and attempt the remaining independent cases; never redirect to another HWND.
            post_input(&bridge, input).map_err(rhai_error)
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn supported_inputs_parse_without_aliases() {
        for (name, input) in [
            ("browser_back", HistoryInput::BrowserBack),
            ("browser_forward", HistoryInput::BrowserForward),
            ("appcommand_back", HistoryInput::AppCommandBack),
            ("appcommand_forward", HistoryInput::AppCommandForward),
            ("mouse_x1", HistoryInput::MouseX1),
            ("mouse_x2", HistoryInput::MouseX2),
        ] {
            assert_eq!(HistoryInput::parse(name), Ok(input));
        }
        for name in ["", "back", "BrowserBack", "mouse_x3"] {
            assert!(HistoryInput::parse(name).is_err());
        }
    }

    #[test]
    fn browser_and_appcommand_plans_press_once() {
        for (input, vk) in [
            (HistoryInput::BrowserBack, 0xA6),
            (HistoryInput::BrowserForward, 0xA7),
        ] {
            let plan = message_plan(input, 0x123, [20, 30]);
            assert_eq!(
                (plan[0].message, plan[0].wparam, plan[0].lparam),
                (0x100, vk, 1)
            );
            assert_eq!((plan[1].message, plan[1].wparam), (0x101, vk));
            assert_eq!(plan[1].lparam as u32, 0xC000_0001);
        }
        for (input, command) in [
            (HistoryInput::AppCommandBack, 1),
            (HistoryInput::AppCommandForward, 2),
        ] {
            let plan = message_plan(input, 0x123, [20, 30]);
            assert_eq!(plan.len(), 1);
            assert_eq!((plan[0].message, plan[0].wparam), (0x319, 0x123));
            assert_eq!((plan[0].lparam as u32 >> 16) & 0xFFF, command);
            assert_eq!((plan[0].lparam as u32 >> 16) & 0xF000, 0x8000);
        }
    }

    #[test]
    fn xbutton_plans_release_the_same_button_at_client_center() {
        for (input, button, mask) in [
            (HistoryInput::MouseX1, 1, 0x20),
            (HistoryInput::MouseX2, 2, 0x40),
        ] {
            let plan = message_plan(input, 0x123, [120, 240]);
            assert_eq!(plan.len(), 2);
            assert_eq!((plan[0].message, plan[1].message), (0x20B, 0x20C));
            assert_eq!(plan[0].wparam, (button << 16) | mask);
            assert_eq!(plan[1].wparam, button << 16);
            assert_eq!(plan[0].lparam, 120 | (240 << 16));
            assert_eq!(plan[0].lparam, plan[1].lparam);
        }
    }

    #[test]
    fn wrong_targets_fail_before_the_post_operation() {
        let detached = TestScriptWindowIdentity::Detached {
            window_id: 1,
            context_serial: 2,
            viewport_id: egui::ViewportId::from_hash_of("history-wrong-target"),
            host_incarnation: 3,
            hwnd: 4,
            backend_token: 5,
        };
        for selection in [
            TestScriptActionSelection::LegacyImplicit,
            TestScriptActionSelection::Targeted(detached),
        ] {
            let result: Result<(), String> = with_root_target(selection, |_| {
                panic!("wrong target must not reach native posting")
            });
            assert!(result.is_err());
        }
    }
}
