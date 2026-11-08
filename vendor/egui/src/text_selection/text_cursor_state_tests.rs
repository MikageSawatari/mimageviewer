//! §1.691: real TextEdit pointer routing, without a window or native input.
use super::*;
use crate::{Context, Event, Id, Modifiers, PointerButton, Pos2, RawInput, TextEdit, pos2, vec2};

struct Field {
    ctx: Context,
    id: Id,
    text: String,
    output: Option<crate::text_edit::TextEditOutput>,
    time: f64,
    dragged: bool,
}

impl Field {
    fn new() -> Self {
        let mut field = Self {
            ctx: Context::default(),
            id: Id::unique("section1691"),
            // Include multi-byte characters: pointer containment uses char boundaries.
            text: "aébc def ghi".to_owned(),
            output: None,
            time: 0.0,
            dragged: false,
        };
        field.frame(vec![], Modifiers::NONE);
        field
            .ctx
            .memory_mut(|memory| memory.request_focus(field.id));
        field.frame(vec![], Modifiers::NONE);
        field.select(CCursorRange::two(CCursor::new(3), CCursor::new(6)));
        field
    }

    fn frame(&mut self, events: Vec<Event>, modifiers: Modifiers) {
        self.time += 1.0; // Deterministic single clicks; no wall-clock sleeps.
        let output = self.ctx.run_ui(
            RawInput {
                time: Some(self.time),
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(400.0, 100.0))),
                events: std::iter::once(Event::ModifiersChanged(modifiers))
                    .chain(events)
                    .collect(),
                ..Default::default()
            },
            |ui| {
                let output = TextEdit::singleline(&mut self.text)
                    .id(self.id)
                    .desired_width(300.0)
                    .show(ui);
                self.dragged = ui.is_being_dragged(self.id);
                self.output = Some(output);
            },
        );
        output.drop_without_applying_deltas();
    }

    fn select(&mut self, range: CCursorRange) {
        let mut state = TextEdit::load_state(&self.ctx, self.id).unwrap();
        state.cursor.set_char_range(Some(range));
        // Also prove that the early return retains granular selection state.
        state.cursor.granular_drag = Some(GranularDragSelect {
            granularity: SelectGranularity::Word,
            anchor: range,
        });
        state.store(&self.ctx, self.id);
    }

    fn cursor(&self) -> TextCursorState {
        TextEdit::load_state(&self.ctx, self.id).unwrap().cursor
    }

    fn position(&self, index: usize) -> Pos2 {
        let output = self.output.as_ref().unwrap();
        output.galley_pos
            + output
                .galley
                .pos_from_cursor(CCursor::new(index))
                .center()
                .to_vec2()
    }

    fn press(&mut self, index: usize, button: PointerButton, modifiers: Modifiers) -> Pos2 {
        let pos = self.position(index);
        self.frame(vec![Event::PointerMoved(pos)], modifiers);
        self.frame(vec![pointer(pos, button, true, modifiers)], modifiers);
        pos
    }

    fn assert_range(&self, primary: usize, secondary: usize) {
        let range = self.cursor().char_range().unwrap();
        assert_eq!(
            (range.primary.index.0, range.secondary.index.0),
            (primary, secondary)
        );
    }
}

fn pointer(pos: Pos2, button: PointerButton, pressed: bool, modifiers: Modifiers) -> Event {
    Event::PointerButton {
        pos,
        button,
        pressed,
        modifiers,
    }
}

fn assert_preserved(button: PointerButton, modifiers: Modifiers) {
    // Inclusive boundaries, both range directions, and all interior boundaries.
    for reversed in [false, true] {
        for index in 3..=6 {
            let mut field = Field::new();
            if reversed {
                field.select(CCursorRange::two(CCursor::new(6), CCursor::new(3)));
            }
            let before = field.cursor();
            let pos = field.press(index, button, modifiers);
            let after = field.cursor();
            assert_eq!(after.char_range(), before.char_range());
            assert_eq!(after.granular_drag, before.granular_drag);
            // Releasing the same button must not subsequently collapse the range.
            field.frame(vec![pointer(pos, button, false, modifiers)], modifiers);
            assert_eq!(field.cursor().char_range(), before.char_range());
            assert_eq!(field.cursor().granular_drag, before.granular_drag);
        }
    }
}

#[test]
fn section1691_secondary_press_inside_preserves_selection() {
    assert_preserved(PointerButton::Secondary, Modifiers::NONE);
}

#[test]
fn section1691_middle_press_inside_preserves_selection() {
    assert_preserved(PointerButton::Middle, Modifiers::NONE);
}

#[test]
fn section1691_extra_press_inside_preserves_selection() {
    for button in [PointerButton::Extra1, PointerButton::Extra2] {
        assert_preserved(button, Modifiers::NONE);
    }
}

#[test]
fn section1691_shift_secondary_inside_preserves_selection() {
    assert_preserved(PointerButton::Secondary, Modifiers::SHIFT);
}

#[test]
fn section1691_secondary_press_outside_moves_caret() {
    for index in [2, 7] {
        let mut field = Field::new();
        field.press(index, PointerButton::Secondary, Modifiers::NONE);
        field.assert_range(index, index);
        assert!(field.cursor().granular_drag.is_none());
        assert!(field.ctx.memory(|memory| memory.has_focus(field.id)));
        // Pasting still inserts at the right-clicked boundary.
        field.frame(vec![Event::Paste("X".to_owned())], Modifiers::NONE);
        let mut expected = "aébc def ghi".chars().collect::<Vec<_>>();
        expected.insert(index, 'X');
        assert_eq!(field.text, expected.into_iter().collect::<String>());
    }
}

#[test]
fn section1691_secondary_without_selection_moves_caret() {
    let mut field = Field::new();
    field.select(CCursorRange::one(CCursor::new(3)));
    field.press(4, PointerButton::Secondary, Modifiers::NONE);
    field.assert_range(4, 4);
}

#[test]
fn section1691_primary_press_and_drag_unchanged() {
    let mut field = Field::new();
    field.press(4, PointerButton::Primary, Modifiers::NONE);
    field.assert_range(4, 4);
    assert!(field.cursor().granular_drag.is_none());
    let pos = field.position(9);
    field.frame(vec![Event::PointerMoved(pos)], Modifiers::NONE);
    assert!(field.dragged);
    field.assert_range(9, 4);
}

#[test]
fn section1691_primary_and_secondary_pressed_together_move_caret() {
    let mut field = Field::new();
    let pos = field.position(4);
    field.frame(vec![Event::PointerMoved(pos)], Modifiers::NONE);
    field.frame(
        vec![
            pointer(pos, PointerButton::Primary, true, Modifiers::NONE),
            pointer(pos, PointerButton::Secondary, true, Modifiers::NONE),
        ],
        Modifiers::NONE,
    );
    field.assert_range(4, 4);
    assert!(field.cursor().granular_drag.is_none());
}

#[test]
fn section1691_shift_press_outside_still_extends() {
    for button in [PointerButton::Primary, PointerButton::Secondary] {
        let mut field = Field::new();
        // Plain selection: Shift extension keeps the secondary endpoint fixed.
        let mut state = TextEdit::load_state(&field.ctx, field.id).unwrap();
        state.cursor.granular_drag = None;
        state.store(&field.ctx, field.id);
        field.press(8, button, Modifiers::SHIFT);
        field.assert_range(8, 3);
    }
}

#[test]
fn section1691_secondary_only_drag_preserves_selection() {
    let mut field = Field::new();
    let before = field.cursor();
    field.press(4, PointerButton::Secondary, Modifiers::NONE);
    let pos = field.position(10);
    field.frame(vec![Event::PointerMoved(pos)], Modifiers::NONE);
    // click_and_drag widgets can acquire drag capture from ANY pointer button.
    assert!(
        field.dragged,
        "secondary-only drag must reach pointer_interaction"
    );
    assert!(!field.ctx.input(|i| i.pointer.primary_down()));
    assert_eq!(field.cursor().char_range(), before.char_range());
    assert_eq!(field.cursor().granular_drag, before.granular_drag);
}

#[test]
fn section1691_ignored_press_reports_no_interaction_or_state_change() {
    let ctx = Context::default();
    let pos = pos2(40.0, 20.0);
    let mut cursor = TextCursorState::from(CCursorRange::two(CCursor::new(3), CCursor::new(6)));
    cursor.granular_drag = Some(GranularDragSelect {
        granularity: SelectGranularity::Line,
        anchor: cursor.char_range().unwrap(),
    });
    let before = cursor;
    // Use a real registered response, while calling the state boundary directly
    // to observe its bool (TextEdit uses it for focus/cursor-purpose changes).
    for events in [
        vec![Event::PointerMoved(pos)],
        vec![pointer(
            pos,
            PointerButton::Secondary,
            true,
            Modifiers::NONE,
        )],
    ] {
        let output = ctx.run_ui(
            RawInput {
                events,
                ..Default::default()
            },
            |ui| {
                let (_, response) =
                    ui.allocate_exact_size(vec2(200.0, 40.0), crate::Sense::click_and_drag());
                if ui.input(|i| i.pointer.any_pressed()) {
                    assert!(response.hovered());
                    assert!(!ui.input(|i| i.pointer.primary_pressed()));
                }
                let galley = ui.painter().layout_no_wrap(
                    "abcdefgh".to_owned(),
                    crate::FontId::default(),
                    crate::Color32::WHITE,
                );
                assert!(!cursor.pointer_interaction(
                    ui,
                    &response,
                    CCursor::new(4),
                    &galley,
                    false
                ));
            },
        );
        output.drop_without_applying_deltas();
    }
    assert_eq!(cursor.char_range(), before.char_range());
    assert_eq!(cursor.granular_drag, before.granular_drag);
}
