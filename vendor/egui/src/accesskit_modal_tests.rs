//! Widget-owned accessibility actions must respect the same modal boundary as pointer input.

use crate::{
    Context, DragValue, Event, Id, Modal, Panel, Pos2, RangeSlider, RawInput, Rect, Slider,
    TextEdit, Ui,
    accesskit::{Action, ActionData, ActionRequest, TextPosition, TextSelection, TreeId},
    text::{CCursor, CCursorRange},
    vec2,
};

fn request(id: Id, action: Action, data: Option<ActionData>) -> Event {
    Event::AccessKitActionRequest(ActionRequest {
        action,
        target_node: id.accesskit_id(),
        target_tree: TreeId::ROOT,
        data,
    })
}

fn input(events: Vec<Event>) -> RawInput {
    RawInput {
        screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(900.0, 700.0))),
        events,
        ..Default::default()
    }
}

struct Fields {
    text: String,
    text_id: Id,
    drag: f64,
    drag_id: Id,
    slider: f64,
    slider_id: Id,
    low: f64,
    high: f64,
    range_id: Id,
}

impl Fields {
    fn new(name: &str) -> Self {
        Self {
            text: "abc".into(),
            text_id: Id::unique(name),
            drag: 10.0,
            drag_id: Id::NULL,
            slider: 10.0,
            slider_id: Id::NULL,
            low: 10.0,
            high: 90.0,
            range_id: Id::NULL,
        }
    }

    fn show(&mut self, ui: &mut Ui) {
        ui.add(TextEdit::singleline(&mut self.text).id(self.text_id));
        self.drag_id = ui.add(DragValue::new(&mut self.drag).speed(1.0)).id;
        self.slider_id = ui
            .add(
                Slider::new(&mut self.slider, 0.0..=100.0)
                    .step_by(1.0)
                    .show_value(false),
            )
            .id;
        self.range_id = ui
            .add(RangeSlider::new(&mut self.low, &mut self.high, 0.0..=100.0).step_by(1.0))
            .id
            .with("low");
    }

    fn numeric_requests(&self, action: Action, data: Option<ActionData>) -> Vec<Event> {
        [self.drag_id, self.slider_id, self.range_id]
            .into_iter()
            .map(|id| request(id, action, data.clone()))
            .collect()
    }

    fn assert_numbers(&self, value: f64) {
        assert_eq!(self.drag, value, "DragValue");
        assert_eq!(self.slider, value, "Slider");
        assert_eq!(self.low, value, "RangeSlider low handle");
        assert_eq!(self.high, 90.0, "RangeSlider other handle");
    }
}

struct Fixture {
    ctx: Context,
    background: Fields,
    modal: Fields,
}

impl Fixture {
    fn new(modal_open: bool) -> Self {
        let mut fixture = Self {
            ctx: Context::default(),
            background: Fields::new("background text"),
            modal: Fields::new("modal text"),
        };
        // Establish widget IDs and the previous-pass modal boundary, as in a persistent dialog.
        fixture.frame(vec![], modal_open);
        fixture.frame(vec![], modal_open);
        fixture
    }

    fn frame(&mut self, events: Vec<Event>, modal_open: bool) {
        let output = self.ctx.run_ui(input(events), |ui| {
            self.background.show(ui);
            if modal_open {
                Modal::new(Id::unique("accesskit modal")).show(ui.ctx(), |ui| self.modal.show(ui));
            }
        });
        output.drop_without_applying_deltas();
    }

    fn assert_consumed(&self, id: Id, action: Action) {
        assert!(
            !self
                .ctx
                .input(|input| input.has_accesskit_action_request(id, action))
        );
    }
}

#[test]
fn modal_blocks_background_text_set_value_but_accepts_modal_value() {
    let mut fixture = Fixture::new(true);
    fixture.frame(
        vec![
            request(
                fixture.background.text_id,
                Action::SetValue,
                Some(ActionData::Value("blocked".into())),
            ),
            request(
                fixture.modal.text_id,
                Action::SetValue,
                Some(ActionData::Value("accepted".into())),
            ),
        ],
        true,
    );
    assert_eq!(fixture.background.text, "abc");
    assert_eq!(fixture.modal.text, "accepted");
    fixture.assert_consumed(fixture.background.text_id, Action::SetValue);
    // A rejected request cannot become a delayed edit after the dialog closes.
    fixture.frame(vec![], false);
    fixture.frame(vec![], false);
    assert_eq!(fixture.background.text, "abc");
}

#[test]
fn text_set_value_without_modal_remains_available_without_focus() {
    let mut fixture = Fixture::new(false);
    fixture.frame(
        vec![request(
            fixture.background.text_id,
            Action::SetValue,
            Some(ActionData::Value("accepted".into())),
        )],
        false,
    );
    assert_eq!(fixture.background.text, "accepted");
}

#[test]
fn first_modal_boundary_rejects_cached_background_focus_before_text_input() {
    let mut fixture = Fixture::new(false);
    fixture.frame(vec![], true);
    let id = fixture.background.text_id;
    fixture.frame(
        vec![request(id, Action::Focus, None), Event::Text("X".into())],
        true,
    );
    assert_eq!(fixture.background.text, "abc");
    assert!(!fixture.ctx.memory(|memory| memory.has_focus(id)));
    fixture.assert_consumed(id, Action::Focus);
}

#[test]
fn modal_focus_action_still_routes_text_input_to_modal_field() {
    let mut fixture = Fixture::new(false);
    fixture.frame(vec![], true);
    fixture.frame(vec![], true); // Register focus stops after the modal's initial sizing pass.
    let id = fixture.modal.text_id;
    let mut state = TextEdit::load_state(&fixture.ctx, id).unwrap();
    state
        .cursor
        .set_char_range(Some(CCursorRange::one(CCursor::new(1))));
    state.store(&fixture.ctx, id);
    fixture.frame(
        vec![request(id, Action::Focus, None), Event::Text("X".into())],
        true,
    );
    assert_eq!(fixture.modal.text, "aXbc");
    assert!(fixture.ctx.memory(|memory| memory.has_focus(id)));
}

#[test]
fn cached_focus_cannot_edit_a_hidden_field_reappearing_behind_modal() {
    for modal_open in [false, true] {
        let mut fixture = Fixture::new(false);
        let id = fixture.background.text_id;
        place_cursor(&fixture.ctx, id);
        // Keep its focus cache entry while omitting the field from the previous
        // widget tree. A previous-widget lookup cannot identify its layer now.
        let output = fixture.ctx.run_ui(input(vec![]), |ui| {
            if modal_open {
                Modal::new(Id::unique("accesskit modal"))
                    .show(ui.ctx(), |ui| fixture.modal.show(ui));
            }
        });
        output.drop_without_applying_deltas();
        let output = fixture.ctx.run_ui(
            input(vec![
                request(id, Action::Focus, None),
                Event::Text("X".into()),
            ]),
            |ui| {
                assert!(
                    ui.memory(|memory| memory.has_focus(id)),
                    "exercise cached focus before the field registers"
                );
                fixture.background.show(ui);
                if modal_open {
                    Modal::new(Id::unique("accesskit modal"))
                        .show(ui.ctx(), |ui| fixture.modal.show(ui));
                }
            },
        );
        output.drop_without_applying_deltas();
        assert_eq!(
            fixture.background.text,
            if modal_open { "abc" } else { "aXbc" }
        );
        assert_eq!(
            fixture.ctx.memory(|memory| memory.has_focus(id)),
            !modal_open
        );
        if modal_open {
            fixture.assert_consumed(id, Action::Focus);
        }
    }
}

fn selection_request(id: Id) -> Event {
    let position = |character_index| TextPosition {
        node: id.with(0_usize).with(0_usize).accesskit_id(),
        character_index,
    };
    request(
        id,
        Action::SetTextSelection,
        Some(ActionData::SetTextSelection(TextSelection {
            anchor: position(1),
            focus: position(2),
        })),
    )
}

fn place_cursor(ctx: &Context, id: Id) {
    let mut state = TextEdit::load_state(ctx, id).unwrap();
    state
        .cursor
        .set_char_range(Some(CCursorRange::one(CCursor::new(1))));
    state.store(ctx, id);
    ctx.memory_mut(|memory| memory.request_focus(id));
}

#[test]
fn modal_discards_background_text_selection_and_replacement() {
    let mut fixture = Fixture::new(true);
    let id = fixture.background.text_id;
    place_cursor(&fixture.ctx, id);
    fixture.frame(vec![selection_request(id)], true);
    fixture.assert_consumed(id, Action::SetTextSelection);
    let range = TextEdit::load_state(&fixture.ctx, id)
        .unwrap()
        .cursor
        .char_range()
        .unwrap();
    assert_eq!(range, CCursorRange::one(CCursor::new(1)));
    fixture.frame(
        vec![request(
            id,
            Action::ReplaceSelectedText,
            Some(ActionData::Value("X".into())),
        )],
        true,
    );
    fixture.assert_consumed(id, Action::ReplaceSelectedText);
    assert_eq!(fixture.background.text, "abc");
}

#[test]
fn modal_text_selection_and_replacement_remain_available() {
    let mut fixture = Fixture::new(true);
    let id = fixture.modal.text_id;
    place_cursor(&fixture.ctx, id);
    fixture.frame(vec![selection_request(id)], true);
    let range = TextEdit::load_state(&fixture.ctx, id)
        .unwrap()
        .cursor
        .char_range()
        .unwrap();
    assert_eq!(range.sorted_cursors().map(|cursor| cursor.index.0), [1, 2]);
    fixture.frame(
        vec![request(
            id,
            Action::ReplaceSelectedText,
            Some(ActionData::Value("X".into())),
        )],
        true,
    );
    assert_eq!(fixture.modal.text, "aXc");
}

#[test]
fn modal_blocks_numeric_actions_on_background_and_accepts_modal_actions() {
    let mut fixture = Fixture::new(true);
    for (action, data, expected) in [
        (Action::SetValue, Some(ActionData::NumericValue(40.0)), 40.0),
        (Action::Increment, None, 41.0),
        (Action::Decrement, None, 40.0),
    ] {
        let mut events = fixture.background.numeric_requests(action, data.clone());
        events.extend(fixture.modal.numeric_requests(action, data));
        fixture.frame(events, true);
        fixture.background.assert_numbers(10.0);
        fixture.modal.assert_numbers(expected);
        for id in [
            fixture.background.drag_id,
            fixture.background.slider_id,
            fixture.background.range_id,
        ] {
            fixture.assert_consumed(id, action);
        }
    }
}

#[test]
fn numeric_actions_without_modal_remain_available() {
    let mut fixture = Fixture::new(false);
    for (action, data, expected) in [
        (Action::SetValue, Some(ActionData::NumericValue(40.0)), 40.0),
        (Action::Increment, None, 41.0),
        (Action::Decrement, None, 40.0),
    ] {
        fixture.frame(fixture.background.numeric_requests(action, data), false);
        fixture.background.assert_numbers(expected);
    }
}

#[test]
fn modal_blocks_panel_resize_actions_and_accepts_modal_panel_resize() {
    for (action, data) in [
        (Action::SetValue, Some(ActionData::NumericValue(0.0))),
        (Action::Increment, None),
        (Action::Decrement, None),
    ] {
        let ctx = Context::default();
        let mut background = (Id::NULL, 0.0);
        let mut modal = (Id::NULL, 0.0);
        let mut frame = |events| {
            let output = ctx.run_ui(input(events), |ui| {
                let show_panel = |ui: &mut Ui, result: &mut (Id, f32)| {
                    result.0 = ui.make_persistent_id("resizable panel").with("__resize");
                    result.1 = Panel::left("resizable panel")
                        .resizable(true)
                        .default_size(120.0)
                        .min_size(20.0)
                        .show(ui, |ui| {
                            ui.take_available_space(); // A resizable panel needs contents that fill its width.
                            ui.label("Panel");
                        })
                        .response
                        .rect
                        .width();
                };
                show_panel(ui, &mut background);
                Modal::new(Id::unique("resize modal")).show(ui.ctx(), |ui| {
                    // A cached, automatically sized modal would clamp the panel to its old width.
                    ui.set_width(400.0);
                    ui.set_min_height(300.0);
                    show_panel(ui, &mut modal);
                });
            });
            output.drop_without_applying_deltas();
            (background, modal)
        };
        frame(vec![]);
        let (before_background, before_modal) = frame(vec![]);
        let (after_background, after_modal) = frame(vec![
            request(before_background.0, action, data.clone()),
            request(before_modal.0, action, data),
        ]);
        assert_eq!(after_background.1, before_background.1);
        match action {
            Action::SetValue | Action::Decrement => assert!(after_modal.1 < before_modal.1),
            Action::Increment => assert!(
                after_modal.1 > before_modal.1,
                "panel increment: before={}, after={}",
                before_modal.1,
                after_modal.1
            ),
            _ => unreachable!(),
        }
        assert!(
            !ctx.input(|input| input.has_accesskit_action_request(before_background.0, action))
        );
    }
}
