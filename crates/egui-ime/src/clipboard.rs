//! Pure clipboard shortcut classification shared by the two input bridges.
//!
//! Hosts retain Process/IME, text focus and press/release ownership, clipboard
//! I/O, newline normalization and suppression of a recognized empty paste.

/// A recognized shortcut, before the host performs clipboard I/O or delivery.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClipboardCommand {
    Cut,
    Copy,
    Paste,
}

fn letter_command(key: egui::Key) -> Option<ClipboardCommand> {
    match key {
        egui::Key::X => Some(ClipboardCommand::Cut),
        egui::Key::C => Some(ClipboardCommand::Copy),
        egui::Key::V => Some(ClipboardCommand::Paste),
        _ => None,
    }
}

/// Preserve egui-winit 0.33.3 shortcut precedence and platform compatibility.
///
/// Command letters accept Shift and Alt. Dedicated clipboard keys work without
/// modifiers; Windows also retains Shift+Delete, Ctrl+Insert and Shift+Insert.
/// Ctrl+Shift+Insert is Copy because the original bridge checks Copy first.
/// Call only after the host has excluded IME-processed keys and releases.
pub fn winit_clipboard_command(
    modifiers: egui::Modifiers,
    key: egui::Key,
) -> Option<ClipboardCommand> {
    if key == egui::Key::Cut
        || (cfg!(target_os = "windows") && modifiers.shift && key == egui::Key::Delete)
    {
        Some(ClipboardCommand::Cut)
    } else if key == egui::Key::Copy
        || (cfg!(target_os = "windows") && modifiers.ctrl && key == egui::Key::Insert)
    {
        Some(ClipboardCommand::Copy)
    } else if key == egui::Key::Paste
        || (cfg!(target_os = "windows") && modifiers.shift && key == egui::Key::Insert)
    {
        Some(ClipboardCommand::Paste)
    } else if modifiers.command {
        letter_command(key)
    } else {
        None
    }
}

/// Preserve native Windows overlay shortcuts: Ctrl+C/X/V, with Shift allowed.
///
/// Alt, dedicated clipboard keys and Insert/Delete are intentionally outside
/// this bridge's existing shortcuts. The host applies its text-focus/IME gate,
/// suppresses recognized presses and releases, and performs I/O on presses only.
pub fn native_windows_clipboard_command(
    modifiers: egui::Modifiers,
    key: egui::Key,
) -> Option<ClipboardCommand> {
    if modifiers.ctrl && !modifiers.alt {
        letter_command(key)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clipboard_shortcut_modifier_truth_table_preserves_bridge_differences() {
        // Include independent ctrl/command/mac_cmd flags: the native bridge uses
        // ctrl while winit uses command, including on non-Windows platforms.
        for bits in 0..32 {
            let modifiers = egui::Modifiers {
                alt: bits & 1 != 0,
                ctrl: bits & 2 != 0,
                shift: bits & 4 != 0,
                mac_cmd: bits & 8 != 0,
                command: bits & 16 != 0,
            };
            for (key, command) in [
                (egui::Key::X, ClipboardCommand::Cut),
                (egui::Key::C, ClipboardCommand::Copy),
                (egui::Key::V, ClipboardCommand::Paste),
            ] {
                assert_eq!(
                    winit_clipboard_command(modifiers, key),
                    modifiers.command.then_some(command),
                    "winit {modifiers:?} {key:?}",
                );
                assert_eq!(
                    native_windows_clipboard_command(modifiers, key),
                    (modifiers.ctrl && !modifiers.alt).then_some(command),
                    "native {modifiers:?} {key:?}",
                );
            }
            for (key, expected) in [
                (egui::Key::Cut, Some(ClipboardCommand::Cut)),
                (egui::Key::Copy, Some(ClipboardCommand::Copy)),
                (egui::Key::Paste, Some(ClipboardCommand::Paste)),
                (
                    egui::Key::Delete,
                    (cfg!(target_os = "windows") && modifiers.shift)
                        .then_some(ClipboardCommand::Cut),
                ),
                (
                    egui::Key::Insert,
                    if !cfg!(target_os = "windows") {
                        None
                    } else if modifiers.ctrl {
                        Some(ClipboardCommand::Copy)
                    } else {
                        modifiers.shift.then_some(ClipboardCommand::Paste)
                    },
                ),
                (egui::Key::A, None),
                (egui::Key::Enter, None),
            ] {
                assert_eq!(
                    winit_clipboard_command(modifiers, key),
                    expected,
                    "winit {modifiers:?} {key:?}",
                );
                assert_eq!(
                    native_windows_clipboard_command(modifiers, key),
                    None,
                    "native {modifiers:?} {key:?}",
                );
            }
        }
    }
}
