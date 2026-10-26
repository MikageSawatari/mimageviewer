//! Headless input regression tests for the number beside a stepped slider.
use crate::{
    CentralPanel, Context, Event, Id, Key, Modifiers, PointerButton, Pos2, RawInput, Rect, Slider,
    emath::Numeric, pos2, vec2,
};
use core::ops::RangeInclusive;

enum WidgetKind {
    Slider(Option<f64>),
    LiveDragValue,
    CustomRoundedSlider,
    FixedDecimalsSlider,
}

struct InputTest<Num: Numeric> {
    ctx: Context,
    value: Num,
    range: RangeInclusive<Num>,
    kind: WidgetKind,
    rect: Rect,
    other_rect: Rect,
    other_clicked: bool,
    rail_id: Id,
    changed: bool,
    text: Vec<String>,
}

impl InputTest<u32> {
    fn new(value: u32) -> Self {
        Self::with_range(value, 0..=100, 5.0)
    }
}

impl<Num: Numeric> InputTest<Num> {
    fn with_range(value: Num, range: RangeInclusive<Num>, step: f64) -> Self {
        let mut test = Self {
            ctx: Context::default(),
            value,
            range,
            kind: WidgetKind::Slider(Some(step)),
            rect: Rect::NOTHING,
            other_rect: Rect::NOTHING,
            other_clicked: false,
            rail_id: Id::NULL,
            changed: false,
            text: vec![],
        };
        test.frame(vec![]);
        test
    }

    fn frame(&mut self, events: Vec<Event>) {
        let input = RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(400.0, 200.0))),
            events,
            ..Default::default()
        };
        let mut output = self.ctx.run_ui(input, |ui| {
            CentralPanel::default().show(ui, |ui| {
                let response = match self.kind {
                    WidgetKind::Slider(step) => {
                        let mut slider = Slider::new(&mut self.value, self.range.clone());
                        if let Some(step) = step {
                            slider = slider.step_by(step);
                        }
                        ui.add(slider)
                    }
                    WidgetKind::LiveDragValue => {
                        ui.add(crate::DragValue::new(&mut self.value).range(self.range.clone()))
                    }
                    WidgetKind::CustomRoundedSlider => ui.add(
                        Slider::from_get_set(0.0..=100.0, |new_value| {
                            if let Some(new_value) = new_value {
                                self.value = Num::from_f64((new_value * 2.0).round() / 2.0);
                            }
                            self.value.to_f64()
                        })
                        .step_by(0.1),
                    ),
                    WidgetKind::FixedDecimalsSlider => {
                        ui.add(Slider::new(&mut self.value, self.range.clone()).fixed_decimals(1))
                    }
                };
                self.rect = response.rect;
                if self.rail_id == Id::NULL {
                    self.rail_id = response.id;
                }
                self.changed = response.changed();
                let other = ui.button("Other focus");
                self.other_rect = other.rect;
                self.other_clicked = other.clicked();
            });
        });
        self.text = output
            .shapes
            .iter()
            .filter_map(|shape| {
                if let crate::Shape::Text(text) = &shape.shape {
                    Some(text.galley.text().to_owned())
                } else {
                    None
                }
            })
            .collect();
        output.textures_delta.clear();
    }

    fn click(&mut self, pos: Pos2) {
        self.frame(vec![
            Event::PointerMoved(pos),
            Event::PointerButton {
                pos,
                button: PointerButton::Primary,
                pressed: true,
                modifiers: Modifiers::NONE,
            },
        ]);
        self.frame(vec![Event::PointerButton {
            pos,
            button: PointerButton::Primary,
            pressed: false,
            modifiers: Modifiers::NONE,
        }]);
        self.frame(vec![]);
    }

    fn edit(&mut self) {
        self.click(pos2(self.rect.right() - 5.0, self.rect.center().y));
        assert!(self.ctx.memory(|mem| mem.focused().is_some()));
    }

    fn key(&mut self, key: Key) {
        self.frame(vec![
            Event::Key {
                key,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: Modifiers::NONE,
            },
            Event::Key {
                key,
                physical_key: None,
                pressed: false,
                repeat: false,
                modifiers: Modifiers::NONE,
            },
        ]);
    }

    fn type_text(&mut self, text: &str) {
        for digit in text.chars() {
            self.frame(vec![Event::Text(digit.to_string())]);
        }
    }

    fn assert_text(&self, text: &str) {
        assert!(
            self.text.iter().any(|painted| painted == text),
            "expected {text:?}, painted {:?}",
            self.text
        );
    }
}

#[test]
fn stepped_slider_accepts_digits_separately() {
    for (text, expected) in [("25", 25), ("50", 50), ("80", 80), ("100", 100)] {
        let mut test = InputTest::new(60);
        test.edit();
        let mut prefix = String::new();
        for digit in text.chars() {
            prefix.push(digit);
            test.type_text(&digit.to_string());
            let expected_partial = ((prefix.parse::<f64>().unwrap() / 5.0).round() * 5.0) as u32;
            assert_eq!(test.value, expected_partial, "live {prefix}");
            test.frame(vec![]);
            test.assert_text(&prefix);
        }
        test.frame(vec![]);
        test.assert_text(text);
        test.key(Key::Enter);
        test.frame(vec![]);
        assert_eq!(test.value, expected, "typed {text} one digit per frame");
    }
}

#[test]
fn stepped_slider_publishes_rounded_values_without_replacing_text() {
    let mut test = InputTest::new(60);
    test.edit();
    test.type_text("2");
    assert_eq!(
        test.value, 0,
        "live rounding matches egui 0.33; the preferences owner holds the draft"
    );
    assert!(test.changed);
    test.frame(vec![]);
    test.assert_text("2");
    test.type_text("5");
    assert_eq!(test.value, 25);
    assert!(test.changed);
    test.frame(vec![]);
    test.assert_text("25");
}

#[test]
fn stepped_slider_focus_loss_preserves_live_value() {
    let mut test = InputTest::new(60);
    test.edit();
    test.type_text("27");
    test.assert_text("27");
    assert_eq!(test.value, 25);
    test.click(test.other_rect.center());
    assert_eq!(test.value, 25);
}

#[test]
fn stepped_slider_value_is_live_before_an_apply_button_closes_the_ui() {
    let mut test = InputTest::new(60);
    test.edit();
    test.type_text("25");
    assert_eq!(test.value, 25);
    let pos = test.other_rect.center();
    test.frame(vec![
        Event::PointerMoved(pos),
        Event::PointerButton {
            pos,
            button: PointerButton::Primary,
            pressed: true,
            modifiers: Modifiers::NONE,
        },
    ]);
    test.frame(vec![Event::PointerButton {
        pos,
        button: PointerButton::Primary,
        pressed: false,
        modifiers: Modifiers::NONE,
    }]);
    assert!(test.other_clicked);
    assert_eq!(test.value, 25, "apply must see the edit in its click frame");
}

#[test]
fn stepped_slider_deletion_and_caret_edits() {
    let mut test = InputTest::new(60);
    test.edit();
    test.key(Key::Backspace);
    assert_eq!(test.value, 60);
    test.type_text("25");
    test.key(Key::ArrowLeft);
    test.key(Key::Delete);
    test.type_text("0");
    test.assert_text("20");
    test.key(Key::Home);
    test.key(Key::Delete);
    test.type_text("5");
    test.assert_text("50");
    assert_eq!(test.value, 50);
    test.key(Key::Enter);
    assert_eq!(test.value, 50);
}

#[test]
fn stepped_slider_escape_does_not_undo_live_values() {
    let mut test = InputTest::new(60);
    test.edit();
    test.type_text("2");
    test.key(Key::Escape);
    test.frame(vec![]);
    test.click(test.other_rect.center());
    assert_eq!(test.value, 0);
    test.edit();
    test.type_text("80");
    test.key(Key::Enter);
    assert_eq!(test.value, 80, "cancelled text must not survive reopening");
}

#[test]
fn stepped_slider_explicit_zero_endpoints_paste_and_clamp() {
    for (text, expected) in [("0", 0), ("100", 100), ("125", 100), ("-20", 0), ("27", 25)] {
        for paste in [false, true] {
            let mut test = InputTest::new(60);
            test.edit();
            if paste {
                test.frame(vec![Event::Paste(text.into())]);
            } else {
                test.type_text(text);
            }
            assert_eq!(test.value, expected);
            test.key(Key::Enter);
            test.frame(vec![]);
            assert_eq!(test.value, expected, "{text}, paste={paste}");
        }
    }
}

#[test]
fn stepped_slider_preserves_untouched_off_grid_value() {
    let mut test = InputTest::new(27);
    for _ in 0..3 {
        test.frame(vec![]);
    }
    assert_eq!(test.value, 27);
    test.edit();
    test.key(Key::Enter);
    test.frame(vec![]);
    assert_eq!(test.value, 27);
    test.edit();
    test.click(test.other_rect.center());
    assert_eq!(test.value, 27);
}

#[test]
fn stepped_slider_rail_and_number_arrows_keep_step() {
    let mut test = InputTest::new(50);
    test.ctx.memory_mut(|mem| mem.request_focus(test.rail_id));
    test.frame(vec![]); // Establish the rail's arrow-key focus lock before sending keys.
    test.key(Key::ArrowRight);
    assert_eq!(test.value, 55);
    test.key(Key::ArrowLeft);
    assert_eq!(test.value, 50);
    test.edit();
    test.key(Key::ArrowUp);
    assert_eq!(test.value, 55);
    test.key(Key::ArrowDown);
    assert_eq!(test.value, 50);
}

#[test]
fn stepped_slider_drag_keeps_step() {
    let mut test = InputTest::new(50);
    let start = pos2(test.rect.left() + 50.0, test.rect.center().y);
    test.frame(vec![
        Event::PointerMoved(start),
        Event::PointerButton {
            pos: start,
            button: PointerButton::Primary,
            pressed: true,
            modifiers: Modifiers::NONE,
        },
    ]);
    let mut values = vec![];
    for offset in [12.0, 18.0, 27.0, 36.0] {
        test.frame(vec![Event::PointerMoved(start + vec2(offset, 0.0))]);
        values.push(test.value);
    }
    assert!(values.windows(2).any(|pair| pair[0] != pair[1]));
    assert!(values.iter().all(|value| value % 5 == 0), "{values:?}");
    test.frame(vec![Event::PointerButton {
        pos: start + vec2(36.0, 0.0),
        button: PointerButton::Primary,
        pressed: false,
        modifiers: Modifiers::NONE,
    }]);
}

#[test]
fn stepped_slider_sibling_ranges_keep_partial_text() {
    for (initial, range, step, text, expected) in [
        (60.0_f64, 10.0..=100.0, 5.0, "25", 25.0), // Auto cache time
        (2.0, 0.5..=10.0, 0.5, "0.5", 0.5),        // Auto cache size
        (0.0, -4.0..=4.0, 0.25, "1.25", 1.25),     // Font vertical adjustment
        (1.0, 0.1..=4.0, 0.1, "0.8", 0.8),         // Colorize tone radius
    ] {
        let mut test = InputTest::with_range(initial, range, step);
        test.edit();
        test.type_text(text);
        test.assert_text(text);
        assert!((test.value - expected).abs() < 1e-10);
        test.key(Key::Enter);
        assert!(
            (test.value - expected).abs() < 1e-10,
            "{text}: {}",
            test.value
        );
    }
}

#[test]
fn unstepped_slider_keeps_live_updates() {
    let mut test = InputTest::with_range(60.0, 0.0..=100.0, 0.0);
    test.edit();
    test.type_text("2");
    assert_eq!(test.value, 2.0);
    assert!(test.changed);
    test.type_text("5");
    assert_eq!(test.value, 25.0);
}

#[test]
fn unstepped_slider_keeps_existing_decimal_rounding() {
    let ctx = Context::default();
    let mut value = 1.234_f64;
    let mut output = ctx.run_ui(RawInput::default(), |ui| {
        ui.add(Slider::new(&mut value, 0.0..=10.0).fixed_decimals(1));
    });
    output.textures_delta.clear();
    assert_eq!(value, 1.2);
}

#[test]
fn stepped_slider_external_change_invalidates_pending_text() {
    for blur in [false, true] {
        let mut test = InputTest::new(60);
        test.edit();
        test.type_text("25");
        test.value = 80; // e.g. a reset button or another control of the same value.
        test.frame(vec![]);
        test.assert_text("80");
        if blur {
            test.click(test.other_rect.center());
        } else {
            test.key(Key::Enter);
            test.frame(vec![]);
        }
        assert_eq!(test.value, 80);
    }
}

#[test]
fn stepped_slider_arrow_uses_live_value() {
    let mut test = InputTest::new(60);
    test.edit();
    test.type_text("2");
    test.key(Key::ArrowUp);
    assert_eq!(test.value, 5);
    test.frame(vec![]);
    test.assert_text("5");
    test.key(Key::Enter);
    test.frame(vec![]);
    assert_eq!(test.value, 5);
}

#[test]
fn integer_slider_implicit_step_keeps_live_updates() {
    let mut test = InputTest::new(60);
    test.kind = WidgetKind::Slider(None); // integer() supplies the implicit step of 1.
    test.frame(vec![]);
    test.edit();
    test.type_text("25");
    assert_eq!(test.value, 25);
    test.key(Key::Enter);
    assert_eq!(test.value, 25);
}

#[test]
fn standalone_drag_value_keeps_live_updates_and_external_change_detection() {
    let mut test = InputTest::new(60);
    test.kind = WidgetKind::LiveDragValue;
    test.frame(vec![]);
    test.edit();
    test.type_text("25");
    assert_eq!(test.value, 25);
    test.frame(vec![]);
    test.assert_text("25");
    test.value = 80;
    test.frame(vec![]);
    test.assert_text("80");
    test.key(Key::Enter);
    assert_eq!(test.value, 80);
}

#[test]
fn stepped_slider_off_grid_arrow_text_uses_stored_value() {
    let mut test = InputTest::new(27);
    test.edit();
    test.key(Key::ArrowUp);
    assert_eq!(test.value, 30);
    test.assert_text("30");
    test.frame(vec![]);
    test.assert_text("30");
    test.key(Key::ArrowDown);
    assert_eq!(test.value, 25);
    test.assert_text("25");
}

#[test]
fn stepped_slider_changed_tracks_effective_value() {
    let mut test = InputTest::new(25);
    test.edit();
    test.frame(vec![Event::Text("26".into())]);
    assert_eq!(test.value, 25);
    assert!(!test.changed);
    test.frame(vec![]);
    test.assert_text("26");
}

#[test]
fn slider_f32_and_custom_storage_rounding_preserve_text() {
    let mut float = InputTest::with_range(1.0_f32, 0.0..=2.0, 0.1);
    float.edit();
    float.type_text("1.23");
    float.frame(vec![]);
    float.assert_text("1.23");
    assert!((float.value - 1.2).abs() < 1e-6);

    let mut custom = InputTest::with_range(1.0_f64, 0.0..=100.0, 0.1);
    custom.kind = WidgetKind::CustomRoundedSlider;
    custom.frame(vec![]);
    custom.edit();
    custom.type_text("0.36");
    custom.frame(vec![]);
    custom.assert_text("0.36");
    assert_eq!(custom.value, 0.5);
    custom.value = 80.0;
    custom.frame(vec![]);
    custom.assert_text("80");
}

#[test]
fn slider_own_clamp_at_non_aligned_endpoint_preserves_text() {
    let mut test = InputTest::with_range(60_u32, 0..=98, 5.0);
    test.edit();
    test.type_text("98");
    test.frame(vec![]); // Existing Always clamping brings the rounded endpoint back into range.
    assert_eq!(test.value, 98);
    test.assert_text("98");
    test.type_text("0");
    test.frame(vec![]);
    test.assert_text("980");
    assert_eq!(test.value, 98);
}

#[test]
fn slider_without_step_preserves_text_across_decimal_rounding() {
    let mut test = InputTest::with_range(1.0_f64, 0.0..=100.0, 0.0);
    test.kind = WidgetKind::FixedDecimalsSlider;
    test.frame(vec![]);
    test.edit();
    test.type_text("1.234");
    test.frame(vec![]);
    test.assert_text("1.234");
    assert_eq!(test.value, 1.2);
    test.value = 3.5;
    test.frame(vec![]);
    test.assert_text("3.5");
}

#[test]
fn stepped_slider_accessibility_set_value_displays_stored_value() {
    let mut test = InputTest::new(27);
    test.edit();
    let target = test.ctx.memory(|mem| mem.focused().unwrap());
    test.frame(vec![Event::AccessKitActionRequest(
        crate::accesskit::ActionRequest {
            action: crate::accesskit::Action::SetValue,
            target_tree: crate::accesskit::TreeId::ROOT,
            target_node: target.accesskit_id(),
            data: Some(crate::accesskit::ActionData::NumericValue(32.0)),
        },
    )]);
    assert_eq!(test.value, 30);
    test.assert_text("30");
    test.frame(vec![]);
    test.assert_text("30");
}
