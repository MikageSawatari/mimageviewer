//! Application-level egui keyboard focus policy.
//!
//! egui resolves `Tab` into a focus direction during `Context::begin_pass`,
//! before application shortcut code runs. mimageviewer disables that traversal
//! application-wide by choice: `Tab` never moves keyboard focus, including
//! between text fields. This policy only cancels the focus direction; it leaves
//! the key event available to the focused widget, IME, and configurable keymap.
//!
//! This began as a fix for a systemic shortcut failure: `Tab` could move egui
//! focus onto a non-text widget, `wants_keyboard_input()` would remain true, and
//! every application shortcut would stop working. Selectively allowing traversal
//! while editing text then produced a disorienting one-hop move out of a field
//! before traversal stopped. IME candidate selection with `Tab` is a separate,
//! field-level focus lock and recovery mechanism in `crate::ime_focus`.

use std::sync::Arc;

const INSTALLATION_ID: &str = "miv_tab_shortcut_focus_policy_installed";

fn tab_pressed(ctx: &egui::Context) -> bool {
    ctx.input(|input| {
        input.events.iter().any(|event| {
            matches!(
                event,
                egui::Event::Key {
                    key: egui::Key::Tab,
                    pressed: true,
                    ..
                }
            )
        })
    })
}

/// Cancel focus traversal that egui already derived from `Tab`.
pub(crate) fn cancel_tab_focus_traversal(ctx: &egui::Context) {
    ctx.memory_mut(|memory| memory.move_focus(egui::FocusDirection::None));
}

/// Install mimageviewer's `Tab` shortcut policy on an egui context.
///
/// The callback runs before the first focusable widget is registered, resets the
/// focus direction for every `Tab` press, and deliberately does not consume the
/// event. The focused UI or the later keymap ownership boundary still receives
/// the same press.
pub fn install_tab_shortcut_focus_policy(ctx: &egui::Context) {
    let installation_id = egui::Id::new(INSTALLATION_ID);
    let already_installed = ctx.data_mut(|data| {
        if data.get_temp::<bool>(installation_id).unwrap_or(false) {
            true
        } else {
            data.insert_temp(installation_id, true);
            false
        }
    });
    if already_installed {
        return;
    }

    ctx.on_begin_pass(
        "miv_tab_shortcut_focus_policy",
        Arc::new(|ctx| {
            if !tab_pressed(ctx) {
                return;
            }
            cancel_tab_focus_traversal(ctx);
        }),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tab_event(repeat: bool, modifiers: egui::Modifiers) -> egui::Event {
        egui::Event::Key {
            key: egui::Key::Tab,
            physical_key: None,
            pressed: true,
            repeat,
            modifiers,
        }
    }

    fn raw_input(
        viewport_id: egui::ViewportId,
        child_viewport_id: Option<egui::ViewportId>,
        events: Vec<egui::Event>,
    ) -> egui::RawInput {
        let mut input = egui::RawInput {
            viewport_id,
            events,
            ..Default::default()
        };
        if let Some(child_viewport_id) = child_viewport_id {
            input.viewports.insert(
                child_viewport_id,
                egui::ViewportInfo {
                    parent: Some(egui::ViewportId::ROOT),
                    ..Default::default()
                },
            );
        }
        input
    }

    fn draw_focusable_surface(ctx: &egui::Context, id: egui::Id) {
        egui::CentralPanel::default().show(ctx, |ui| {
            let rect = egui::Rect::from_min_size(ui.min_rect().min, egui::vec2(32.0, 32.0));
            let _ = ui.interact(rect, id, egui::Sense::click_and_drag());
        });
    }

    fn draw_text_fields(
        ctx: &egui::Context,
        first_id: egui::Id,
        second_id: egui::Id,
        first: &mut String,
        second: &mut String,
        request_first_focus: bool,
    ) {
        egui::CentralPanel::default().show(ctx, |ui| {
            let first_response = ui.add(egui::TextEdit::singleline(first).id(first_id));
            let _ = ui.add(egui::TextEdit::singleline(second).id(second_id));
            if request_first_focus {
                first_response.request_focus();
            }
        });
    }

    #[test]
    fn tab_before_focusable_surface_does_not_create_keyboard_focus() {
        let ctx = egui::Context::default();
        install_tab_shortcut_focus_policy(&ctx);

        let _ = ctx.run(
            raw_input(
                egui::ViewportId::ROOT,
                None,
                vec![tab_event(false, egui::Modifiers::NONE)],
            ),
            |ctx| {
                assert!(ctx.input(|input| input.events.iter().any(|event| matches!(
                    event,
                    egui::Event::Key {
                        key: egui::Key::Tab,
                        pressed: true,
                        ..
                    }
                ))));
                draw_focusable_surface(ctx, egui::Id::new("fs_click_test"));
            },
        );

        assert!(!ctx.wants_keyboard_input());
    }

    #[test]
    fn tab_repeat_also_does_not_leak_into_focus_traversal() {
        let ctx = egui::Context::default();
        install_tab_shortcut_focus_policy(&ctx);

        let _ = ctx.run(
            raw_input(
                egui::ViewportId::ROOT,
                None,
                vec![tab_event(true, egui::Modifiers::NONE)],
            ),
            |ctx| draw_focusable_surface(ctx, egui::Id::new("fs_click_repeat_test")),
        );

        assert!(!ctx.wants_keyboard_input());
    }

    #[test]
    fn focused_text_edit_keeps_focus_and_accepts_text_and_ime_input_after_tab() {
        let ctx = egui::Context::default();
        install_tab_shortcut_focus_policy(&ctx);
        let first_id = egui::Id::new("tab_policy_first_text");
        let second_id = egui::Id::new("tab_policy_second_text");
        let mut first = String::new();
        let mut second = String::new();

        let _ = ctx.run(Default::default(), |ctx| {
            draw_text_fields(ctx, first_id, second_id, &mut first, &mut second, true);
        });
        assert_eq!(ctx.memory(|memory| memory.focused()), Some(first_id));

        let _ = ctx.run(
            raw_input(
                egui::ViewportId::ROOT,
                None,
                vec![tab_event(false, egui::Modifiers::NONE)],
            ),
            |ctx| {
                assert!(ctx.wants_keyboard_input());
                assert!(ctx.input(|input| input.events.iter().any(|event| matches!(
                    event,
                    egui::Event::Key {
                        key: egui::Key::Tab,
                        pressed: true,
                        ..
                    }
                ))));
                draw_text_fields(ctx, first_id, second_id, &mut first, &mut second, false);
            },
        );
        assert_eq!(ctx.memory(|memory| memory.focused()), Some(first_id));

        let _ = ctx.run(
            raw_input(
                egui::ViewportId::ROOT,
                None,
                vec![
                    egui::Event::Text("x".to_owned()),
                    egui::Event::Ime(egui::ImeEvent::Enabled),
                    egui::Event::Ime(egui::ImeEvent::Preedit("あ".to_owned())),
                    egui::Event::Ime(egui::ImeEvent::Commit("あ".to_owned())),
                ],
            ),
            |ctx| {
                draw_text_fields(ctx, first_id, second_id, &mut first, &mut second, false);
            },
        );
        assert_eq!(ctx.memory(|memory| memory.focused()), Some(first_id));
        assert_eq!(first, "xあ");
        assert!(second.is_empty());
    }

    #[test]
    fn tab_does_not_move_focus_between_two_text_fields() {
        let ctx = egui::Context::default();
        install_tab_shortcut_focus_policy(&ctx);
        let first_id = egui::Id::new("tab_policy_no_hop_first");
        let second_id = egui::Id::new("tab_policy_no_hop_second");
        let mut first = String::new();
        let mut second = String::new();

        let _ = ctx.run(Default::default(), |ctx| {
            draw_text_fields(ctx, first_id, second_id, &mut first, &mut second, true);
        });
        let _ = ctx.run(
            raw_input(
                egui::ViewportId::ROOT,
                None,
                vec![tab_event(false, egui::Modifiers::NONE)],
            ),
            |ctx| {
                draw_text_fields(ctx, first_id, second_id, &mut first, &mut second, false);
            },
        );

        assert_eq!(ctx.memory(|memory| memory.focused()), Some(first_id));
        assert!(!ctx.memory(|memory| memory.has_focus(second_id)));
    }

    #[test]
    fn root_text_edit_and_fullscreen_child_focus_policy_are_independent() {
        let ctx = egui::Context::default();
        install_tab_shortcut_focus_policy(&ctx);
        let child_id = egui::ViewportId::from_hash_of("tab_policy_fullscreen_child");
        let first_id = egui::Id::new("root_first_text");
        let second_id = egui::Id::new("root_second_text");
        let mut first = String::new();
        let mut second = String::new();

        let _ = ctx.run(
            raw_input(egui::ViewportId::ROOT, Some(child_id), Vec::new()),
            |ctx| {
                draw_text_fields(ctx, first_id, second_id, &mut first, &mut second, true);
            },
        );

        let _ = ctx.run(
            raw_input(
                child_id,
                Some(child_id),
                vec![tab_event(false, egui::Modifiers::SHIFT)],
            ),
            |ctx| draw_focusable_surface(ctx, egui::Id::new("fullscreen_child_surface")),
        );
        assert!(!ctx.wants_keyboard_input());

        let _ = ctx.run(
            raw_input(
                egui::ViewportId::ROOT,
                Some(child_id),
                vec![tab_event(false, egui::Modifiers::NONE)],
            ),
            |ctx| {
                draw_text_fields(ctx, first_id, second_id, &mut first, &mut second, false);
            },
        );
        assert_eq!(ctx.memory(|memory| memory.focused()), Some(first_id));
    }
}
