//! Application-level egui keyboard focus policy.
//!
//! egui resolves `Tab` into a focus direction during `Context::begin_pass`,
//! before application shortcut code runs. mimageviewer also uses `Tab` as a
//! configurable shortcut, so non-text passes must cancel that already-decided
//! traversal before the first focusable widget is registered.

use std::sync::Arc;

const INSTALLATION_ID: &str = "miv_tab_shortcut_focus_policy_installed";
const TEXT_EDIT_ACTIVE_ID: &str = "miv_tab_shortcut_text_edit_active";

fn text_edit_active_id(viewport_id: egui::ViewportId) -> egui::Id {
    egui::Id::new((TEXT_EDIT_ACTIVE_ID, viewport_id))
}

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

fn focused_widget_is_text_edit(ctx: &egui::Context) -> bool {
    let focused_id = ctx.memory(|memory| memory.focused());
    focused_id.is_some_and(|id| egui::TextEdit::load_state(ctx, id).is_some())
}

/// Cancel focus traversal that egui already derived from a claimed `Tab`.
pub(crate) fn cancel_tab_focus_traversal(ctx: &egui::Context) {
    ctx.memory_mut(|memory| memory.move_focus(egui::FocusDirection::None));
}

/// Install mimageviewer's `Tab` shortcut policy on an egui context.
///
/// `PlatformOutput::ime` is egui's public, widget-specific signal that it sets
/// iff a mutable `TextEdit` is currently being edited. We sample it at the end
/// of each viewport pass and use that viewport-local previous-pass value at the
/// next begin-pass callback. `TextEdit::load_state` covers the one-pass boundary
/// where `request_focus` ran after the widget was drawn and `ime` is not set
/// yet. Together they distinguish real text editing from focus on buttons,
/// sliders, or full-screen click surfaces without weakening the keymap's
/// general `wants_keyboard_input()` gate.
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
            let viewport_id = ctx.viewport_id();
            let text_edit_was_active = ctx.data_mut(|data| {
                data.get_temp::<bool>(text_edit_active_id(viewport_id))
                    .unwrap_or(false)
            }) || focused_widget_is_text_edit(ctx);
            if !text_edit_was_active {
                cancel_tab_focus_traversal(ctx);
            }
        }),
    );

    ctx.on_end_pass(
        "miv_tab_shortcut_text_edit_sample",
        Arc::new(|ctx| {
            let text_edit_active = ctx.output(|output| output.ime.is_some());
            let viewport_id = ctx.viewport_id();
            ctx.data_mut(|data| {
                data.insert_temp(text_edit_active_id(viewport_id), text_edit_active);
            });
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
    fn focused_text_edit_keeps_tab_traversal_text_and_ime_input() {
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
        assert_eq!(ctx.memory(|memory| memory.focused()), Some(second_id));

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
        assert_eq!(second, "xあ");
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
        assert_eq!(ctx.memory(|memory| memory.focused()), Some(second_id));
    }
}
