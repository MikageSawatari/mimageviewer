//! Set MIV_FOCUS_DEBUG=1 before startup to trace ROOT input/focus (4096 lines max).
//! Read events in order, then compare focus_start/end and keyboard_owners (actual returns).
//! primary_hits are egui's previous-pass press targets; compare address_prev/address rects and gates.
//! win32_activation records come from the existing main HWND subclass; no text/path contents are logged.

use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use egui::{Context, Event, Id, PointerButton};
use serde_json::{Value, json};

use crate::keyboard_input::KeyboardOwner;

const TRACE_LINE_LIMIT: u32 = 4096;
static ENABLED: OnceLock<bool> = OnceLock::new();
static LINES: AtomicU32 = AtomicU32::new(0);
static CAP_REPORTED: AtomicBool = AtomicBool::new(false);

pub(crate) fn is_enabled() -> bool {
    *ENABLED.get_or_init(|| std::env::var("MIV_FOCUS_DEBUG").is_ok_and(|value| value == "1"))
}

/// Installed before eframe begins the first pass. When disabled there are no
/// callbacks, snapshots, context lookups, or allocations, only the cached flag.
pub(crate) fn install(ctx: &Context) {
    if !is_enabled() {
        return;
    }
    if ctx.plugin_opt::<FocusTrace>().is_none() {
        ctx.add_plugin(FocusTrace::default());
    }
}

/// Observe returned owners, never invoke the owner decision for diagnostics.
pub(crate) fn record_keyboard_owner(ctx: &Context, owner: KeyboardOwner) {
    if !is_enabled() {
        return;
    }
    if ctx.viewport_id() != egui::ViewportId::ROOT {
        return;
    }
    let _ = ctx.with_plugin(|trace: &mut FocusTrace| {
        if let Some(pass) = &mut trace.pass
            && pass.number == ctx.cumulative_pass_nr()
            && pass.owners.last() != Some(&owner)
        {
            pass.owners.push(owner);
        }
    });
}

#[derive(Default)]
struct FocusTrace {
    pass: Option<RootPass>,
    previous_focus: Option<Option<Id>>,
    previous_raw_focused: Option<bool>,
}

struct RootPass {
    number: u64,
    raw_focused: bool,
    interesting_input: bool,
    events: Vec<Value>,
    focus_start: Option<Id>,
    owners: Vec<KeyboardOwner>,
    primary_hits: Vec<Value>,
    address_prev: Value,
    address: Value,
}

impl egui::Plugin for FocusTrace {
    fn debug_name(&self) -> &'static str {
        "mIV focus trace"
    }

    fn on_begin_pass(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx();
        if ctx.viewport_id() != egui::ViewportId::ROOT || CAP_REPORTED.load(Ordering::Relaxed) {
            return;
        }
        let (raw_focused, interesting_input, events) = ctx.input(|input| {
            (
                input.raw.focused,
                input.raw.events.iter().any(|event| {
                    matches!(
                        event,
                        Event::PointerButton { .. }
                            | Event::PointerButtonReleased { .. }
                            | Event::Key { .. }
                            | Event::WindowFocused(_)
                    )
                }),
                input.raw.events.iter().map(format_event).collect(),
            )
        });
        let primary_hits = ctx
            .pointer_button_targets()
            .into_iter()
            .filter(|(_, button, pressed, _, _)| *button == PointerButton::Primary && *pressed)
            .map(|(pos, _, _, id, allowed)| {
                let widget = ctx.viewport(|viewport| {
                    id.and_then(|id| viewport.prev_pass.widgets.get(id).copied())
                });
                json!({
                    "pos": [pos.x, pos.y], "id": format_id(id), "allowed": allowed,
                    "enabled": widget.map(|widget| widget.enabled),
                    "sense_click": widget.map(|widget| widget.sense.senses_click()),
                    "rect": widget.map(|widget| format_rect(widget.rect)),
                    "interact_rect": widget.map(|widget| format_rect(widget.interact_rect)),
                })
            })
            .collect();
        let address_prev = ctx.viewport(|viewport| {
            format_address(viewport.prev_pass.widgets.get(address_id()).copied())
        });
        self.pass = Some(RootPass {
            number: ctx.cumulative_pass_nr(),
            raw_focused,
            interesting_input,
            events,
            focus_start: ctx.memory(|memory| memory.focused()),
            owners: Vec::new(),
            primary_hits,
            address_prev,
            address: format_address(None),
        });
    }

    fn on_end_pass(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx();
        if ctx.viewport_id() == egui::ViewportId::ROOT
            && let Some(pass) = &mut self.pass
        {
            pass.address = ctx.viewport(|viewport| {
                format_address(viewport.this_pass.widgets.get(address_id()).copied())
            });
        }
    }

    fn output_hook(&mut self, ctx: &Context, _output: &mut egui::FullOutput) {
        // A child output_hook resumes the parent viewport before it is called.
        // Use ROOT's completed-pass count, not ctx.viewport_id(), to avoid
        // finishing the ROOT record when its immediate fullscreen child ends.
        if !self.pass.as_ref().is_some_and(|pass| {
            ctx.cumulative_pass_nr_for(egui::ViewportId::ROOT) == pass.number + 1
        }) {
            return;
        }
        let Some(pass) = self.pass.take() else {
            return;
        };
        // This runs after Memory::end_pass too, so disappearance of a focused
        // widget at end-pass is observable, not just focus changes inside App::ui.
        let focus_end = ctx.memory(|memory| memory.focused());
        let focus_changed = pass.focus_start != focus_end
            || self
                .previous_focus
                .is_some_and(|previous| previous != pass.focus_start)
            || self
                .previous_raw_focused
                .is_some_and(|previous| previous != pass.raw_focused);
        if pass.interesting_input || focus_changed {
            emit(format_line(&pass, focus_end));
        }
        self.previous_focus = Some(focus_end);
        self.previous_raw_focused = Some(pass.raw_focused);
    }
}

fn address_id() -> Id {
    Id::unique("folder_address_input")
}

fn format_id(id: Option<Id>) -> Option<String> {
    id.map(|id| format!("0x{:016x}", id.value()))
}

fn format_rect(rect: egui::Rect) -> [f32; 4] {
    [rect.min.x, rect.min.y, rect.max.x, rect.max.y]
}

fn format_address(widget: Option<egui::WidgetRect>) -> Value {
    json!({
        "id": format_id(Some(address_id())), "drawn": widget.is_some(),
        "enabled": widget.map(|widget| widget.enabled),
        "rect": widget.map(|widget| format_rect(widget.rect)),
        "interact_rect": widget.map(|widget| format_rect(widget.interact_rect)),
    })
}

fn format_modifiers(modifiers: egui::Modifiers) -> Value {
    json!({
        "alt": modifiers.alt, "ctrl": modifiers.ctrl, "shift": modifiers.shift,
        "mac_cmd": modifiers.mac_cmd, "command": modifiers.command,
    })
}

fn non_printable_key(key: egui::Key) -> bool {
    use egui::Key;
    (Key::F1..=Key::F35).contains(&key)
        || matches!(
            key,
            Key::Escape
                | Key::Enter
                | Key::Tab
                | Key::Backspace
                | Key::Insert
                | Key::Delete
                | Key::ArrowUp
                | Key::ArrowDown
                | Key::ArrowLeft
                | Key::ArrowRight
                | Key::Home
                | Key::End
                | Key::PageUp
                | Key::PageDown
                | Key::Copy
                | Key::Cut
                | Key::Paste
                | Key::BrowserBack
                | Key::BrowserForward
                | Key::BrowserRefresh
                | Key::BrowserSearch
                | Key::BrowserHome
                | Key::BrowserFavorites
                | Key::BrowserStop
        )
}

fn diagnostic_chord(key: egui::Key, modifiers: egui::Modifiers) -> bool {
    use egui::Key;
    !modifiers.alt
        && (modifiers.ctrl || modifiers.command || modifiers.mac_cmd)
        && matches!(
            key,
            Key::A | Key::C | Key::V | Key::X | Key::Z | Key::Y | Key::F | Key::L
        )
}

fn format_event(event: &Event) -> Value {
    match event {
        Event::Key {
            key,
            physical_key,
            pressed,
            repeat,
            modifiers,
        } => {
            let show_key = diagnostic_chord(*key, *modifiers) || non_printable_key(*key);
            // Only the explicit diagnostic chords may reveal printable keys.
            // Alt character entry and remapped physical keys stay redacted too.
            let key_name = if show_key {
                format!("{key:?}")
            } else {
                "printable".into()
            };
            let physical_name = physical_key.map(|physical| {
                if show_key
                    && (non_printable_key(physical) || diagnostic_chord(physical, *modifiers))
                {
                    format!("{physical:?}")
                } else {
                    "printable".into()
                }
            });
            json!({
                "kind": "Key", "key": key_name, "physical_key": physical_name,
                "pressed": pressed, "repeat": repeat, "modifiers": format_modifiers(*modifiers),
            })
        }
        Event::PointerButton {
            pos,
            button,
            pressed,
            modifiers,
        } => json!({
            "kind": "PointerButton", "button": format!("{button:?}"), "pressed": pressed,
            "pos": [pos.x, pos.y], "modifiers": format_modifiers(*modifiers),
        }),
        Event::PointerButtonReleased { button, modifiers } => json!({
            "kind": "PointerButtonReleased", "button": format!("{button:?}"),
            "pressed": false, "pos": null, "modifiers": format_modifiers(*modifiers),
        }),
        Event::PointerMoved(pos) => json!({ "kind": "PointerMoved", "pos": [pos.x, pos.y] }),
        Event::ModifiersChanged(modifiers) => json!({
            "kind": "ModifiersChanged", "modifiers": format_modifiers(*modifiers),
        }),
        Event::WindowFocused(focused) => json!({ "kind": "WindowFocused", "focused": focused }),
        // Preserve order/kind without writing clipboard, text, or IME contents.
        other => json!({ "kind": match other {
            Event::Copy => "Copy",
            Event::Cut => "Cut",
            Event::Paste(_) => "Paste",
            Event::PasteImage(_) => "PasteImage",
            Event::MiddleClickPaste { .. } => "MiddleClickPaste",
            Event::Text(_) => "Text",
            Event::Ime(_) => "Ime",
            Event::PointerInputProvenance(_) => "PointerInputProvenance",
            Event::PointerButtonTime { .. } => "PointerButtonTime",
            Event::MouseMoved(_) => "MouseMoved",
            Event::PointerGone => "PointerGone",
            Event::Zoom(_) => "Zoom",
            Event::Rotate(_) => "Rotate",
            Event::Touch { .. } => "Touch",
            Event::MouseWheel { .. } => "MouseWheel",
            _ => "Other",
        }}),
    }
}

fn format_line(pass: &RootPass, focus_end: Option<Id>) -> String {
    format!(
        "[focus-trace] {}",
        json!({
            "kind": "root_pass", "pass": pass.number, "raw_focused": pass.raw_focused,
            "events": pass.events, "focus_start": format_id(pass.focus_start),
            "focus_end": format_id(focus_end),
            "keyboard_owners": pass.owners.iter().map(|owner| format!("{owner:?}")).collect::<Vec<_>>(),
            "primary_hits": pass.primary_hits, "address_prev": pass.address_prev, "address": pass.address,
        })
    )
}

#[cfg(windows)]
pub(crate) fn log_main_activation(
    hwnd: u64,
    message: u32,
    wparam: usize,
    lparam: isize,
    result: isize,
) {
    // Caller checks the cached flag and the existing subclass's ROOT registration.
    emit(format_main_activation(
        hwnd, message, wparam, lparam, result,
    ));
}

#[cfg(windows)]
fn format_main_activation(
    hwnd: u64,
    message: u32,
    wparam: usize,
    lparam: isize,
    result: isize,
) -> String {
    let mouse_activate_result = (message == 0x0021).then(|| match result {
        1 => "MA_ACTIVATE",
        2 => "MA_ACTIVATEANDEAT",
        3 => "MA_NOACTIVATE",
        4 => "MA_NOACTIVATEANDEAT",
        _ => "unknown",
    });
    format!(
        "[focus-trace] {}",
        json!({
            "kind": "win32_activation", "source": "main_key_input_subclass",
            "hwnd": format!("0x{hwnd:x}"),
            "message": if message == 0x0006 { "WM_ACTIVATE" } else { "WM_MOUSEACTIVATE" },
            "wparam": format!("0x{wparam:x}"), "lparam": format!("0x{lparam:x}"),
            "result": result, "mouse_activate_result": mouse_activate_result,
        })
    )
}

fn emit(line: String) {
    match LINES.try_update(Ordering::AcqRel, Ordering::Acquire, |count| {
        (count < TRACE_LINE_LIMIT).then_some(count + 1)
    }) {
        Ok(_) => crate::logger::log(line),
        Err(_) if !CAP_REPORTED.swap(true, Ordering::AcqRel) => crate::logger::log(format!(
            "[focus-trace] trace_capped data_line_limit={TRACE_LINE_LIMIT} further_records=dropped"
        )),
        Err(_) => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key_fields(
        key: egui::Key,
        physical_key: Option<egui::Key>,
        modifiers: egui::Modifiers,
    ) -> Value {
        format_event(&Event::Key {
            key,
            physical_key,
            pressed: true,
            repeat: false,
            modifiers,
        })
    }

    #[test]
    fn focus_trace_redacts_both_key_identities_for_printable_and_shift_typing() {
        for modifiers in [egui::Modifiers::NONE, egui::Modifiers::SHIFT] {
            for key in [
                egui::Key::A,
                egui::Key::Num7,
                egui::Key::Backslash,
                egui::Key::Space,
            ] {
                for physical in [key, egui::Key::Escape] {
                    let fields = key_fields(key, Some(physical), modifiers);
                    assert_eq!(fields["kind"], "Key");
                    assert_eq!(fields["key"], "printable");
                    assert_eq!(fields["physical_key"], "printable");
                    assert_eq!(fields["pressed"], true);
                    assert_eq!(fields["modifiers"], format_modifiers(modifiers));
                }
                let fields = key_fields(key, None, modifiers);
                assert_eq!(fields["key"], "printable");
                assert_eq!(fields["physical_key"], Value::Null);
            }
        }
        // A non-printable logical key must not expose a printable physical key.
        let fields = key_fields(egui::Key::Enter, Some(egui::Key::A), egui::Modifiers::NONE);
        assert_eq!(fields["key"], "Enter");
        assert_eq!(fields["physical_key"], "printable");
    }

    #[test]
    fn focus_trace_keeps_navigation_function_browser_keys_and_shortcut_chords() {
        use egui::Key;
        for key in [
            Key::Escape,
            Key::Enter,
            Key::Tab,
            Key::Backspace,
            Key::Delete,
            Key::ArrowLeft,
            Key::ArrowRight,
            Key::ArrowUp,
            Key::ArrowDown,
            Key::Home,
            Key::End,
            Key::PageUp,
            Key::PageDown,
            Key::F1,
            Key::F12,
            Key::F35,
            Key::BrowserBack,
            Key::BrowserForward,
            Key::BrowserRefresh,
            Key::BrowserSearch,
            Key::BrowserHome,
            Key::BrowserFavorites,
            Key::BrowserStop,
        ] {
            let fields = key_fields(key, Some(key), egui::Modifiers::NONE);
            assert_eq!(fields["key"], format!("{key:?}"));
            assert_eq!(fields["physical_key"], format!("{key:?}"));
        }
        for modifiers in [
            egui::Modifiers::CTRL,
            egui::Modifiers {
                command: true,
                ..egui::Modifiers::NONE
            },
            egui::Modifiers {
                mac_cmd: true,
                ..egui::Modifiers::NONE
            },
        ] {
            for key in [
                Key::A,
                Key::C,
                Key::V,
                Key::X,
                Key::Z,
                Key::Y,
                Key::F,
                Key::L,
            ] {
                let fields = key_fields(key, Some(key), modifiers);
                assert_eq!(fields["key"], format!("{key:?}"));
                assert_eq!(fields["physical_key"], format!("{key:?}"));
            }
        }
    }

    #[test]
    fn focus_trace_redacts_alt_numpad_character_entry() {
        // egui represents both numpad and main-row digits as Num0..Num9.
        for key in [
            egui::Key::Num0,
            egui::Key::Num1,
            egui::Key::Num2,
            egui::Key::Num3,
            egui::Key::Num4,
            egui::Key::Num5,
            egui::Key::Num6,
            egui::Key::Num7,
            egui::Key::Num8,
            egui::Key::Num9,
        ] {
            let fields = key_fields(key, Some(key), egui::Modifiers::ALT);
            assert_eq!(fields["key"], "printable");
            assert_eq!(fields["physical_key"], "printable");
        }
    }

    #[test]
    fn focus_trace_redacts_non_diagnostic_printable_chords_and_shift_letters() {
        for modifiers in [
            egui::Modifiers::SHIFT,
            egui::Modifiers::ALT,
            egui::Modifiers {
                ctrl: true,
                alt: true,
                ..egui::Modifiers::NONE
            },
            egui::Modifiers {
                command: true,
                alt: true,
                ..egui::Modifiers::NONE
            },
        ] {
            let fields = key_fields(egui::Key::A, Some(egui::Key::A), modifiers);
            assert_eq!(fields["key"], "printable");
            assert_eq!(fields["physical_key"], "printable");
        }
        for key in [egui::Key::B, egui::Key::Num1, egui::Key::Backslash] {
            let fields = key_fields(key, Some(egui::Key::A), egui::Modifiers::CTRL);
            assert_eq!(fields["key"], "printable");
            assert_eq!(fields["physical_key"], "printable");
        }
        let fields = key_fields(egui::Key::A, Some(egui::Key::Num1), egui::Modifiers::CTRL);
        assert_eq!(fields["key"], "A");
        assert_eq!(fields["physical_key"], "printable");
    }

    #[cfg(windows)]
    #[test]
    fn focus_trace_mouse_activation_includes_downstream_result_and_eaten_click_label() {
        for (result, label) in [
            (1, "MA_ACTIVATE"),
            (2, "MA_ACTIVATEANDEAT"),
            (3, "MA_NOACTIVATE"),
            (4, "MA_NOACTIVATEANDEAT"),
            (99, "unknown"),
        ] {
            let line = format_main_activation(0x1234, 0x0021, 0x5678, 0x02010001, result);
            let fields: Value =
                serde_json::from_str(line.strip_prefix("[focus-trace] ").unwrap()).unwrap();
            assert_eq!(fields["message"], "WM_MOUSEACTIVATE");
            assert_eq!(fields["hwnd"], "0x1234");
            assert_eq!(fields["wparam"], "0x5678");
            assert_eq!(fields["lparam"], "0x2010001");
            assert_eq!(fields["result"], json!(result));
            assert_eq!(fields["mouse_activate_result"], label);
        }
        let line = format_main_activation(0x1234, 0x0006, 1, 0, 0);
        let fields: Value =
            serde_json::from_str(line.strip_prefix("[focus-trace] ").unwrap()).unwrap();
        assert_eq!(fields["message"], "WM_ACTIVATE");
        assert_eq!(fields["result"], 0);
        assert_eq!(fields["mouse_activate_result"], Value::Null);
    }

    #[test]
    fn focus_trace_line_preserves_event_order_focus_owner_hits_and_address() {
        let pos = egui::pos2(500.0, 110.0);
        let rect = egui::Rect::from_min_max(egui::pos2(431.0, 96.0), egui::pos2(1342.0, 121.0));
        let widget = egui::WidgetRect {
            id: address_id(),
            parent_id: Id::NULL,
            layer_id: egui::LayerId::background(),
            rect,
            interact_rect: rect,
            sense: egui::Sense::click_and_drag(),
            enabled: true,
            visible: true,
        };
        let pass = RootPass {
            number: 42,
            raw_focused: true,
            interesting_input: true,
            events: [
                Event::WindowFocused(true),
                Event::PointerButton {
                    pos,
                    button: PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::NONE,
                },
                Event::Key {
                    key: egui::Key::A,
                    physical_key: Some(egui::Key::A),
                    pressed: true,
                    repeat: false,
                    modifiers: egui::Modifiers {
                        ctrl: true,
                        command: true,
                        ..egui::Modifiers::NONE
                    },
                },
                Event::Paste("private clipboard path".into()),
                Event::Text("private text".into()),
            ]
            .iter()
            .map(format_event)
            .collect(),
            focus_start: None,
            owners: vec![KeyboardOwner::Unclaimed],
            primary_hits: vec![json!({ "id": format_id(Some(address_id())),
                "enabled": false, "sense_click": true, "allowed": false, "pos": [500.0, 110.0] })],
            address_prev: format_address(None),
            address: format_address(Some(widget)),
        };
        let line = format_line(&pass, Some(address_id()));
        assert!(!line.contains('\n'));
        assert!(!line.contains("private"));
        let fields: Value =
            serde_json::from_str(line.strip_prefix("[focus-trace] ").unwrap()).unwrap();
        assert_eq!(fields["kind"], "root_pass");
        assert_eq!(fields["pass"], 42);
        assert_eq!(fields["raw_focused"], true);
        assert_eq!(fields["focus_start"], Value::Null);
        assert_eq!(fields["focus_end"], json!(format_id(Some(address_id()))));
        assert_eq!(fields["keyboard_owners"], json!(["Unclaimed"]));
        assert_eq!(fields["events"][0]["kind"], "WindowFocused");
        assert_eq!(fields["events"][1]["kind"], "PointerButton");
        assert_eq!(fields["events"][1]["button"], "Primary");
        assert_eq!(fields["events"][1]["pressed"], true);
        assert_eq!(fields["events"][1]["pos"], json!([500.0, 110.0]));
        assert_eq!(fields["events"][2]["key"], "A");
        assert_eq!(fields["events"][2]["modifiers"]["ctrl"], true);
        assert_eq!(fields["events"][2]["modifiers"]["command"], true);
        assert_eq!(fields["events"][3], json!({ "kind": "Paste" }));
        assert_eq!(fields["events"][4], json!({ "kind": "Text" }));
        assert_eq!(fields["primary_hits"], json!(pass.primary_hits));
        assert_eq!(fields["address_prev"]["drawn"], false);
        assert_eq!(fields["address"]["drawn"], true);
        assert_eq!(fields["address"]["enabled"], true);
        assert_eq!(fields["address"]["rect"], json!(format_rect(rect)));
        assert_eq!(fields["address"]["interact_rect"], json!(format_rect(rect)));
        assert_eq!(fields["address"], pass.address);
    }
}
