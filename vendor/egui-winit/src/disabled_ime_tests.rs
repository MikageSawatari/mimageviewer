//! No Window/EventLoop: use State's real producer, shared plugin and TextEdit.

use super::*;
use winit::event::Ime;

struct NoDisplay;
impl HasDisplayHandle for NoDisplay {
    fn display_handle(
        &self,
    ) -> Result<raw_window_handle::DisplayHandle<'_>, raw_window_handle::HandleError> {
        Err(raw_window_handle::HandleError::Unavailable)
    }
}

struct Field {
    ctx: egui::Context,
    state: State,
    id: egui::Id,
    text: String,
}

impl Field {
    fn new(ctx: egui::Context, viewport: ViewportId) -> Self {
        ctx.set_embed_viewports(false);
        egui_ime::install_ime_input_policy(&ctx);
        let mut state = State::new(ctx.clone(), viewport, &NoDisplay, None, None, None);
        state.egui_input.focused = true;
        let mut field = Self {
            ctx,
            state,
            id: egui::Id::unique(("disabled-producer-field", viewport)),
            text: "元".into(),
        };
        field.pass(&[], true);
        field
    }

    fn pass(&mut self, events: &[Ime], focus: bool) -> usize {
        for event in events {
            self.state.on_ime(event);
        }
        // take_egui_input requires a real Window for its geometry. Drain only
        // State's produced input here; conversion and normalization stay real.
        let mut input = self.state.egui_input.take();
        input.viewport_id = self.state.viewport_id;
        for viewport in [ViewportId::ROOT, ViewportId::from_hash_of("sibling")] {
            input.viewports.entry(viewport).or_default();
        }
        let mut empty_preedits = 0;
        let output = self.ctx.run_ui(input, |ui| {
            if ui.ctx().viewport_id() == ViewportId::ROOT {
                // RawInput metadata alone does not keep a child registered.
                // Preserve the sibling's real lifetime across root passes.
                ui.ctx().show_viewport_deferred(
                    ViewportId::from_hash_of("sibling"),
                    egui::ViewportBuilder::default(),
                    |_, _| {},
                );
            }
            empty_preedits = ui.input(|i| {
                i.events.iter().filter(|event| matches!(event,
                    egui::Event::Ime(egui::ImeEvent::Preedit { text, .. }) if text.is_empty()
                )).count()
            });
            let response = ui.add(egui::TextEdit::multiline(&mut self.text).id(self.id));
            if focus {
                response.request_focus();
            }
        });
        // Headless test: all output (including textures) deliberately ends here.
        output.drop_without_applying_deltas();
        empty_preedits
    }

    fn preedit(&mut self) {
        self.pass(&[Ime::Preedit("あ".into(), Some((0, 3)))], false);
        assert_eq!(self.text, "元あ");
        assert!(egui_ime::ime_input_active_in_viewport(
            &self.ctx,
            self.state.viewport_id
        ));
    }
}

#[test]
fn disabled_producer_cancels_preedit_same_or_later_pass() {
    for same_pass in [false, true] {
        let mut field = Field::new(egui::Context::default(), ViewportId::ROOT);
        let count = if same_pass {
            field.pass(
                &[Ime::Preedit("あ".into(), Some((0, 3))), Ime::Disabled],
                false,
            )
        } else {
            field.preedit();
            field.pass(&[Ime::Disabled], false)
        };
        assert_eq!(count, 1);
        assert_eq!(field.text, "元");
        assert!(!egui_ime::ime_input_active(&field.ctx));
    }
}

#[test]
fn disabled_producer_terminates_unfocused_composition() {
    let mut field = Field::new(egui::Context::default(), ViewportId::ROOT);
    field.preedit();
    field.state.on_focused(false);
    field.pass(&[], false);
    assert!(!egui_ime::ime_input_active(&field.ctx));
    assert_eq!(field.pass(&[Ime::Disabled], false), 1);
    assert_eq!(field.text, "元");
    assert_eq!(field.pass(&[Ime::Disabled], false), 0);
}

#[test]
fn disabled_producer_preserves_committed_text_and_deduplicates_terminals() {
    for commit in [false, true] {
        let mut field = Field::new(egui::Context::default(), ViewportId::ROOT);
        field.preedit();
        let expected = if commit {
            field.pass(&[Ime::Commit("確定".into())], false);
            "元確定"
        } else {
            "元"
        };
        assert_eq!(
            field.pass(
                &[
                    Ime::Disabled,
                    Ime::Disabled,
                    Ime::Commit(String::new()),
                    Ime::Disabled
                ],
                false
            ),
            usize::from(!commit)
        );
        assert_eq!(field.text, expected);
        assert_eq!(
            field.pass(&[Ime::Disabled, Ime::Commit(String::new())], false),
            0
        );
        assert_eq!(field.text, expected);
    }
}

#[test]
fn disabled_producer_without_composition_preserves_normal_selection() {
    let mut field = Field::new(egui::Context::default(), ViewportId::ROOT);
    let mut state = egui::TextEdit::load_state(&field.ctx, field.id).unwrap();
    state
        .cursor
        .set_char_range(Some(egui::text::CCursorRange::two(
            egui::text::CCursor::new(0),
            egui::text::CCursor::new(1),
        )));
    state.store(&field.ctx, field.id);
    assert_eq!(field.pass(&[Ime::Disabled], false), 0);
    assert_eq!(field.text, "元");
    assert_eq!(
        egui::TextEdit::load_state(&field.ctx, field.id)
            .unwrap()
            .cursor
            .char_range()
            .unwrap(),
        egui::text::CCursorRange::two(egui::text::CCursor::new(0), egui::text::CCursor::new(1))
    );
}

#[test]
fn disabled_producer_isolates_sibling_state_and_viewport() {
    let ctx = egui::Context::default();
    let mut root = Field::new(ctx.clone(), ViewportId::ROOT);
    let mut sibling = Field::new(ctx, ViewportId::from_hash_of("sibling"));
    root.preedit();
    sibling.preedit();
    assert_eq!(root.pass(&[Ime::Disabled], false), 1);
    assert_eq!(root.text, "元");
    assert!(sibling.state.egui_input.events.is_empty());
    sibling.pass(&[], false);
    assert_eq!(sibling.text, "元あ");
    assert!(egui_ime::ime_input_active_in_viewport(
        &sibling.ctx,
        sibling.state.viewport_id
    ));
    assert_eq!(sibling.pass(&[Ime::Disabled], false), 1);
    assert_eq!(sibling.text, "元");
}

#[test]
fn disabled_producer_leaves_following_enter_escape_available() {
    for key in [egui::Key::Enter, egui::Key::Escape] {
        let mut field = Field::new(egui::Context::default(), ViewportId::ROOT);
        field.preedit();
        field.pass(&[Ime::Disabled], false);
        let (logical, physical) = if key == egui::Key::Enter {
            (
                winit::keyboard::NamedKey::Enter,
                winit::keyboard::KeyCode::Enter,
            )
        } else {
            (
                winit::keyboard::NamedKey::Escape,
                winit::keyboard::KeyCode::Escape,
            )
        };
        assert!(field.state.on_keyboard_input_parts(
            &winit::keyboard::Key::Named(logical),
            winit::keyboard::PhysicalKey::Code(physical),
            None,
            ElementState::Pressed,
            false,
        ));
        field.pass(&[], false);
        assert!(!egui_ime::ime_input_active(&field.ctx));
        if key == egui::Key::Enter {
            assert_eq!(field.text, "元\n");
        } else {
            assert_eq!(field.text, "元");
            assert!(field.ctx.memory(|m| m.focused()).is_none());
        }
    }
}

#[test]
#[allow(deprecated, reason = "Enabled/Disabled compatibility event fixture")]
fn ime_producer_keeps_notifications_and_unicode_active_range() {
    let mut field = Field::new(egui::Context::default(), ViewportId::ROOT);
    field.state.on_ime(&Ime::Enabled);
    assert_eq!(
        field.state.egui_input.events,
        if cfg!(target_os = "linux") {
            vec![]
        } else {
            vec![egui::Event::Ime(egui::ImeEvent::Enabled)]
        }
    );
    field.state.egui_input.events.clear();
    field
        .state
        .on_ime(&Ime::Preedit("aあ語".into(), Some((1, 7))));
    field
        .state
        .on_ime(&Ime::Preedit("aあ語".into(), Some((2, 7))));
    field.state.on_ime(&Ime::Preedit("aあ語".into(), None));
    field.state.on_ime(&Ime::Disabled);
    assert_eq!(
        field.state.egui_input.events,
        vec![
            egui::Event::Ime(egui::ImeEvent::Preedit {
                text: "aあ語".into(),
                active_range_chars: Some(1..3)
            }),
            egui::Event::Ime(egui::ImeEvent::Preedit {
                text: "aあ語".into(),
                active_range_chars: None
            }),
            egui::Event::Ime(egui::ImeEvent::Preedit {
                text: "aあ語".into(),
                active_range_chars: None
            }),
            egui::Event::Ime(egui::ImeEvent::Disabled),
        ]
    );
}

#[cfg(target_os = "windows")]
#[test]
fn disabled_producer_does_not_end_physical_key_holds() {
    use winit::keyboard::{Key, KeyCode, NamedKey, PhysicalKey};
    let mut field = Field::new(egui::Context::default(), ViewportId::ROOT);
    field.state.allow_ime = true;
    for (logical, code, expected) in [
        (NamedKey::Process, KeyCode::Enter, KeyHold::Hidden),
        (NamedKey::Escape, KeyCode::Escape, KeyHold::Forwarded),
    ] {
        field.state.on_keyboard_input_parts(
            &Key::Named(logical),
            PhysicalKey::Code(code),
            None,
            ElementState::Pressed,
            false,
        );
        assert_eq!(
            field.state.key_holds.get(&PhysicalKey::Code(code)),
            Some(&expected)
        );
    }
    let holds = field.state.key_holds.clone();
    field.state.on_ime(&Ime::Disabled);
    assert_eq!(field.state.key_holds, holds);
    field.state.allow_ime = false;
    assert!(!field.state.on_keyboard_input_parts(
        &Key::Named(NamedKey::Enter),
        PhysicalKey::Code(KeyCode::Enter),
        None,
        ElementState::Released,
        false
    ));
    assert!(field.state.on_keyboard_input_parts(
        &Key::Named(NamedKey::Process),
        PhysicalKey::Code(KeyCode::Escape),
        None,
        ElementState::Released,
        false
    ));
    assert!(field.state.key_holds.is_empty());
}

#[test]
fn state_modifier_events_stay_before_edges_and_keep_clipboard_shift() {
    use winit::keyboard::{Key, KeyCode, ModifiersState, PhysicalKey};
    let mut field = Field::new(egui::Context::default(), ViewportId::ROOT);
    field
        .state
        .on_modifiers_changed(ModifiersState::CONTROL | ModifiersState::SHIFT);
    let command = field.state.modifiers;
    field.state.on_keyboard_input_parts(
        &Key::Character("c".into()),
        PhysicalKey::Code(KeyCode::KeyC),
        Some("c"),
        ElementState::Pressed,
        false,
    );
    field.state.on_modifiers_changed(ModifiersState::empty());
    field.state.on_keyboard_input_parts(
        &Key::Character("c".into()),
        PhysicalKey::Code(KeyCode::KeyC),
        None,
        ElementState::Released,
        false,
    );
    assert_eq!(field.state.egui_input.events.len(), 4);
    assert_eq!(
        field.state.egui_input.events[0],
        egui::Event::ModifiersChanged(command)
    );
    assert_eq!(field.state.egui_input.events[1], egui::Event::Copy);
    assert_eq!(
        field.state.egui_input.events[2],
        egui::Event::ModifiersChanged(egui::Modifiers::NONE)
    );
    assert!(matches!(
        field.state.egui_input.events[3],
        egui::Event::Key {
            pressed: false,
            modifiers: egui::Modifiers::NONE,
            ..
        }
    ));
    field.state.on_modifiers_changed(ModifiersState::ALT);
    field.state.on_focused(false);
    assert_eq!(field.state.modifiers, egui::Modifiers::NONE);
}

#[test]
fn clipboard_switch_and_image_paste_keep_upstream_037_behavior() {
    use winit::keyboard::{Key, KeyCode, PhysicalKey};
    let mut field = Field::new(egui::Context::default(), ViewportId::ROOT);
    field
        .state
        .on_modifiers_changed(winit::keyboard::ModifiersState::CONTROL);
    field.state.egui_input.events.clear();
    field.state.set_clipboard_shortcuts(false);
    assert!(field.state.on_keyboard_input_parts(
        &Key::Character("v".into()),
        PhysicalKey::Code(KeyCode::KeyV),
        Some("v"),
        ElementState::Pressed,
        false
    ));
    assert!(matches!(
        field.state.egui_input.events.as_slice(),
        [egui::Event::Key {
            key: egui::Key::V,
            ..
        }]
    ));
    field.state.egui_input.events.clear();
    let image = egui::ColorImage::new([1, 1], vec![egui::Color32::RED]);
    let expected = image.clone();
    assert!(append_keyboard_input(
        &mut field.state.egui_input,
        field.state.modifiers,
        &Key::Character("ж".into()),
        PhysicalKey::Code(KeyCode::KeyV),
        Some("v"),
        ElementState::Pressed,
        false,
        #[cfg(target_os = "windows")]
        true,
        #[cfg(target_os = "windows")]
        &mut field.state.key_holds,
        true,
        || Some(PasteContent::Image(image)),
    ));
    let [egui::Event::PasteImage(image)] = field.state.egui_input.events.as_slice() else {
        panic!("expected only image paste")
    };
    assert_eq!(image.as_ref(), &expected);
}
