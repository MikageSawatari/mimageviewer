use super::*;
use crate::{FontId, Key, Modal, Modifiers, RawInput, Sense, vec2};

#[derive(Clone, Copy)]
enum Location {
    Background,
    Modal,
    Nonmodal,
}

fn check_label_input(location: Location, events: Vec<Event>, dragging: bool, expected: usize) {
    let ctx = Context::default();
    let modal_id = Id::unique("label input modal");
    let raw_input = |events| RawInput {
        screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(900.0, 700.0))),
        events,
        ..Default::default()
    };
    if !matches!(location, Location::Nonmodal) {
        let output = ctx.run_ui(raw_input(vec![]), |ui| {
            Modal::new(modal_id).show(ui.ctx(), |_| {});
        });
        output.drop_without_applying_deltas();
    }

    let mut checked = false;
    let output = ctx.run_ui(raw_input(events), |ui| {
        let mut check = |ui: &mut Ui| {
            let (_, response) = ui.allocate_exact_size(vec2(180.0, 60.0), Sense::click_and_drag());
            let mut galley = ui.painter().layout_no_wrap(
                "abc".into(),
                FontId::monospace(16.0),
                crate::Color32::WHITE,
            );
            let transform = TSTransform::from_translation(response.rect.min.to_vec2());
            let mut state = ViewportLabelSelectionState {
                selection: Some(CurrentSelection {
                    layer_id: response.layer_id,
                    primary: WidgetTextCursor::new(
                        response.id,
                        CCursor::new(1),
                        transform,
                        &galley,
                    ),
                    secondary: WidgetTextCursor::new(
                        response.id,
                        CCursor::new(0),
                        transform,
                        &galley,
                    ),
                }),
                selection_bbox_this_frame: transform
                    * Rect::from_min_size(Pos2::ZERO, galley.size()),
                is_dragging: dragging,
                ..Default::default()
            };
            state.on_begin_pass(ui);
            let painted = state.on_label(ui, &response, response.rect.min, &mut galley);
            state.on_end_pass(ui);
            let selection = state
                .selection
                .expect("the existing selection stays visible");
            assert_eq!(selection.primary.ccursor.index.0, expected);
            assert_eq!(selection.secondary.ccursor.index.0, 0);
            assert!(
                !painted.is_empty(),
                "existing selection must still be painted"
            );
            if matches!(location, Location::Background) {
                assert!(
                    !state.is_dragging,
                    "a modal must end the captured background gesture"
                );
                assert!(ctx.pass_state(|state| state.scroll_target.iter().all(Option::is_none)));
            }
            checked = true;
        };
        match location {
            Location::Background => {
                check(ui);
                Modal::new(modal_id).show(ui.ctx(), |_| {});
            }
            Location::Modal => {
                Modal::new(modal_id).show(ui.ctx(), |ui| check(ui));
            }
            Location::Nonmodal => check(ui),
        }
    });
    output.drop_without_applying_deltas();
    assert!(checked);
}

#[test]
fn modal_ends_background_label_capture_without_clearing_selection() {
    check_label_input(
        Location::Background,
        vec![Event::PointerMoved(Pos2::new(20.0, 650.0))],
        true,
        1,
    );
}

#[test]
fn captured_label_drag_still_extends_outside_label_in_modal() {
    check_label_input(
        Location::Modal,
        vec![Event::PointerMoved(Pos2::new(20.0, 650.0))],
        true,
        3,
    );
}

#[test]
fn captured_label_drag_without_modal_still_extends_outside_label() {
    check_label_input(
        Location::Nonmodal,
        vec![Event::PointerMoved(Pos2::new(20.0, 650.0))],
        true,
        3,
    );
}

#[test]
fn modal_blocks_background_label_keyboard_selection_but_allows_modal_selection() {
    let event = Event::Key {
        key: Key::ArrowRight,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: Modifiers::SHIFT,
    };
    check_label_input(Location::Background, vec![event.clone()], false, 1);
    check_label_input(Location::Modal, vec![event], false, 2);
}

#[test]
fn modal_escape_does_not_clear_background_label_selection() {
    check_label_input(
        Location::Background,
        vec![Event::Key {
            key: Key::Escape,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: Modifiers::NONE,
        }],
        false,
        1,
    );
}

#[test]
fn modal_plain_drag_replaces_blocked_anchor_and_nonmodal_drag_keeps_granularity() {
    for clicks in [2, 3] {
        for modal in [false, true] {
            let ctx = Context::default();
            let mut time = 1.0;
            let mut frame = |events, modal| {
                time += 0.05;
                let mut labels = [None, None];
                let output = ctx.run_ui(
                    RawInput {
                        screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(900.0, 700.0))),
                        time: Some(time),
                        events,
                        ..Default::default()
                    },
                    |ui| {
                        let mut label = |ui: &mut Ui, index, text: &str| {
                            let response = ui.add(
                                crate::Label::new(
                                    crate::RichText::new(text).font(FontId::monospace(16.0)),
                                )
                                .selectable(true),
                            );
                            labels[index] = Some((response.id, response.layer_id, response.rect));
                        };
                        label(ui, 0, "alpha beta gamma");
                        if modal {
                            Modal::new(Id::unique("anchor replacement modal")).show(
                                ui.ctx(),
                                |ui| {
                                    label(ui, 1, "C:/folder/image.png");
                                },
                            );
                        }
                    },
                );
                output.drop_without_applying_deltas();
                labels
            };
            frame(vec![], false);
            let background = frame(vec![], false)[0].unwrap();
            let start = background.2.left_center() + vec2(16.0, 0.0);
            let edge = |pos, pressed| Event::PointerButton {
                pos,
                button: crate::PointerButton::Primary,
                pressed,
                modifiers: Modifiers::NONE,
            };
            for click in 0..clicks {
                frame(vec![Event::PointerMoved(start), edge(start, true)], false);
                if modal || click + 1 < clicks {
                    frame(vec![edge(start, false)], false);
                }
            }
            let plugin = ctx.plugin::<LabelSelectionState>();
            assert!(
                plugin.lock().states[&crate::ViewportId::ROOT]
                    .granular_drag
                    .is_some(),
                "fixture must retain the actual word/line anchor"
            );
            if !modal {
                // Continue the held double/triple-click gesture into the next word.
                let to = background.2.left_center() + vec2(background.2.width() * 8.5 / 16.0, 0.0);
                frame(vec![Event::PointerMoved(to)], false);
                frame(vec![edge(to, false)], false);
                for _ in 0..3 {
                    frame(vec![], false);
                    let plugin = plugin.lock();
                    let state = &plugin.states[&crate::ViewportId::ROOT];
                    let selection = state
                        .selection
                        .expect("nonmodal word/line selection must survive");
                    assert_eq!(selection.primary.widget_id, background.0);
                    assert_eq!(selection.secondary.widget_id, background.0);
                    assert_eq!(selection.secondary.ccursor.index.0, 0);
                    assert_eq!(
                        selection.primary.ccursor.index.0,
                        if clicks == 2 { 10 } else { 16 }
                    );
                    assert_eq!(
                        state.granular_drag.unwrap().granularity,
                        if clicks == 2 {
                            SelectGranularity::Word
                        } else {
                            SelectGranularity::Line
                        }
                    );
                }
                continue;
            }
            frame(vec![], true);
            let foreground = frame(vec![], true)[1].unwrap();
            let from = foreground.2.left_center() + vec2(12.0, 0.0);
            let to = from + vec2(65.0, 0.0);
            frame(vec![Event::PointerMoved(from), edge(from, true)], true);
            frame(vec![Event::PointerMoved(to)], true);
            frame(vec![edge(to, false)], true);
            for _ in 0..3 {
                frame(vec![], true);
                let plugin = plugin.lock();
                let state = &plugin.states[&crate::ViewportId::ROOT];
                let selection = state
                    .selection
                    .expect("the new drag selection must survive later passes");
                assert_eq!(selection.layer_id, foreground.1);
                assert_eq!(selection.primary.widget_id, foreground.0);
                assert_eq!(selection.secondary.widget_id, foreground.0);
                assert!(selection.primary.ccursor.index > selection.secondary.ccursor.index);
                assert!(selection.primary.ccursor.index.0 <= "C:/folder/image.png".len());
                assert!(
                    state.granular_drag.is_none(),
                    "plain drag must not inherit the background anchor"
                );
            }
        }
    }
}
