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

pub(crate) use egui_ime::cancel_tab_focus_traversal;
pub use egui_ime::install_tab_shortcut_focus_policy;

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

    fn draw_focusable_surface(ui: &mut egui::Ui, id: egui::Id) {
        egui::CentralPanel::default().show(ui, |ui| {
            let rect = egui::Rect::from_min_size(ui.min_rect().min, egui::vec2(32.0, 32.0));
            let _ = ui.interact(rect, id, egui::Sense::click_and_drag());
        });
    }

    fn draw_text_fields(
        ui: &mut egui::Ui,
        first_id: egui::Id,
        second_id: egui::Id,
        first: &mut String,
        second: &mut String,
        request_first_focus: bool,
    ) {
        egui::CentralPanel::default().show(ui, |ui| {
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

        ctx.run_ui(
            raw_input(
                egui::ViewportId::ROOT,
                None,
                vec![tab_event(false, egui::Modifiers::NONE)],
            ),
            |ui| {
                let ctx = &ui.ctx().clone();
                assert!(ctx.input(|input| input.events.iter().any(|event| matches!(
                    event,
                    egui::Event::Key {
                        key: egui::Key::Tab,
                        pressed: true,
                        ..
                    }
                ))));
                draw_focusable_surface(ui, egui::Id::unique("fs_click_test"));
            },
        )
        .drop_without_applying_deltas();

        assert!(!ctx.egui_wants_keyboard_input());
    }

    #[test]
    fn tab_repeat_also_does_not_leak_into_focus_traversal() {
        let ctx = egui::Context::default();
        install_tab_shortcut_focus_policy(&ctx);

        ctx.run_ui(
            raw_input(
                egui::ViewportId::ROOT,
                None,
                vec![tab_event(true, egui::Modifiers::NONE)],
            ),
            |ui| draw_focusable_surface(ui, egui::Id::unique("fs_click_repeat_test")),
        )
        .drop_without_applying_deltas();

        assert!(!ctx.egui_wants_keyboard_input());
    }

    #[test]
    #[expect(
        deprecated,
        reason = "Exercise retained IME compatibility notifications and cancellation ordering"
    )]
    fn focused_text_edit_keeps_focus_and_accepts_text_and_ime_input_after_tab() {
        let ctx = egui::Context::default();
        install_tab_shortcut_focus_policy(&ctx);
        let first_id = egui::Id::unique("tab_policy_first_text");
        let second_id = egui::Id::unique("tab_policy_second_text");
        let mut first = String::new();
        let mut second = String::new();

        ctx.run_ui(Default::default(), |ui| {
            draw_text_fields(ui, first_id, second_id, &mut first, &mut second, true);
        })
        .drop_without_applying_deltas();
        assert_eq!(ctx.memory(|memory| memory.focused()), Some(first_id));

        ctx.run_ui(
            raw_input(
                egui::ViewportId::ROOT,
                None,
                vec![tab_event(false, egui::Modifiers::NONE)],
            ),
            |ui| {
                let ctx = &ui.ctx().clone();
                assert!(ctx.egui_wants_keyboard_input());
                assert!(ctx.input(|input| input.events.iter().any(|event| matches!(
                    event,
                    egui::Event::Key {
                        key: egui::Key::Tab,
                        pressed: true,
                        ..
                    }
                ))));
                draw_text_fields(ui, first_id, second_id, &mut first, &mut second, false);
            },
        )
        .drop_without_applying_deltas();
        assert_eq!(ctx.memory(|memory| memory.focused()), Some(first_id));

        ctx.run_ui(
            raw_input(
                egui::ViewportId::ROOT,
                None,
                vec![
                    egui::Event::Text("x".to_owned()),
                    egui::Event::Ime(egui::ImeEvent::Enabled),
                    egui::Event::Ime(egui::ImeEvent::Preedit {
                        text: "あ".to_owned(),
                        active_range_chars: None,
                    }),
                    egui::Event::Ime(egui::ImeEvent::Commit("あ".to_owned())),
                ],
            ),
            |ui| {
                draw_text_fields(ui, first_id, second_id, &mut first, &mut second, false);
            },
        )
        .drop_without_applying_deltas();
        assert_eq!(ctx.memory(|memory| memory.focused()), Some(first_id));
        assert_eq!(first, "xあ");
        assert!(second.is_empty());
    }

    #[test]
    fn tab_does_not_move_focus_between_two_text_fields() {
        let ctx = egui::Context::default();
        install_tab_shortcut_focus_policy(&ctx);
        let first_id = egui::Id::unique("tab_policy_no_hop_first");
        let second_id = egui::Id::unique("tab_policy_no_hop_second");
        let mut first = String::new();
        let mut second = String::new();

        ctx.run_ui(Default::default(), |ui| {
            draw_text_fields(ui, first_id, second_id, &mut first, &mut second, true);
        })
        .drop_without_applying_deltas();
        ctx.run_ui(
            raw_input(
                egui::ViewportId::ROOT,
                None,
                vec![tab_event(false, egui::Modifiers::NONE)],
            ),
            |ui| {
                draw_text_fields(ui, first_id, second_id, &mut first, &mut second, false);
            },
        )
        .drop_without_applying_deltas();

        assert_eq!(ctx.memory(|memory| memory.focused()), Some(first_id));
        assert!(!ctx.memory(|memory| memory.has_focus(second_id)));
    }

    #[test]
    fn root_text_edit_and_fullscreen_child_focus_policy_are_independent() {
        let ctx = egui::Context::default();
        install_tab_shortcut_focus_policy(&ctx);
        let child_id = egui::ViewportId::from_hash_of("tab_policy_fullscreen_child");
        let first_id = egui::Id::unique("root_first_text");
        let second_id = egui::Id::unique("root_second_text");
        let mut first = String::new();
        let mut second = String::new();

        ctx.run_ui(
            raw_input(egui::ViewportId::ROOT, Some(child_id), Vec::new()),
            |ui| {
                draw_text_fields(ui, first_id, second_id, &mut first, &mut second, true);
            },
        )
        .drop_without_applying_deltas();

        ctx.run_ui(
            raw_input(
                child_id,
                Some(child_id),
                vec![tab_event(false, egui::Modifiers::SHIFT)],
            ),
            |ui| draw_focusable_surface(ui, egui::Id::unique("fullscreen_child_surface")),
        )
        .drop_without_applying_deltas();
        assert!(!ctx.egui_wants_keyboard_input());

        ctx.run_ui(
            raw_input(
                egui::ViewportId::ROOT,
                Some(child_id),
                vec![tab_event(false, egui::Modifiers::NONE)],
            ),
            |ui| {
                draw_text_fields(ui, first_id, second_id, &mut first, &mut second, false);
            },
        )
        .drop_without_applying_deltas();
        assert_eq!(ctx.memory(|memory| memory.focused()), Some(first_id));
    }
}
