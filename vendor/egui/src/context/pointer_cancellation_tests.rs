use crate::{
    Context, Event, Id, Modifiers, PointerButton, RawInput, Rect, Sense, ViewportId, pos2,
};

#[derive(Clone, Copy, Debug)]
struct ResponseState {
    clicked: bool,
    dragged: bool,
    stopped: bool,
    primary_down: bool,
}

fn button(button: PointerButton, pressed: bool) -> Event {
    Event::PointerButton {
        pos: pos2(40.0, 30.0),
        button,
        pressed,
        modifiers: Modifiers::NONE,
    }
}

fn frame(ctx: &Context, time: f64, events: Vec<Event>, sense: Sense) -> ResponseState {
    let mut response = None;
    ctx.run_ui(
        RawInput {
            time: Some(time),
            events,
            focused: true,
            ..Default::default()
        },
        |ui| {
            let widget = ui.interact(
                Rect::from_min_max(pos2(10.0, 10.0), pos2(110.0, 60.0)),
                Id::unique("owned"),
                sense,
            );
            response = Some(ResponseState {
                clicked: widget.clicked(),
                dragged: widget.dragged(),
                stopped: widget.drag_stopped(),
                primary_down: ui.input(|input| input.pointer.primary_down()),
            });
        },
    )
    .drop_without_applying_deltas();
    response.unwrap()
}

#[test]
fn pointer_cancellation_ends_short_click_and_drag_without_committing_either() {
    for drag in [false, true] {
        for cancel in [false, true] {
            let ctx = Context::default();
            let sense = Sense::click_and_drag();
            frame(&ctx, 0.9, Vec::new(), sense);
            frame(&ctx, 1.0, vec![button(PointerButton::Primary, true)], sense);
            if drag {
                let moved = frame(
                    &ctx,
                    1.005,
                    vec![Event::PointerMoved(pos2(85.0, 30.0))],
                    sense,
                );
                assert!(moved.dragged);
            }
            let events = if cancel {
                ctx.cancel_pointer_button_for(
                    ctx.viewport_registration(ViewportId::ROOT).unwrap(),
                    PointerButton::Primary,
                    Id::unique("owned"),
                );
                Vec::new()
            } else {
                vec![button(PointerButton::Primary, false)]
            };
            let finished = frame(&ctx, 1.01, events, sense);
            assert!(!finished.primary_down);
            assert!(!finished.dragged);
            assert_eq!(finished.clicked, !cancel && !drag);
            assert_eq!(finished.stopped, !cancel && drag);
        }
    }
}

#[test]
fn pointer_cancellation_keeps_other_button_and_other_widget_capture() {
    let ctx = Context::default();
    let sense = Sense::drag();
    frame(&ctx, 0.9, Vec::new(), sense);
    let secondary = frame(
        &ctx,
        1.0,
        vec![button(PointerButton::Secondary, true)],
        sense,
    );
    assert!(secondary.dragged);
    frame(
        &ctx,
        1.005,
        vec![button(PointerButton::Primary, true)],
        sense,
    );
    let registration = ctx.viewport_registration(ViewportId::ROOT).unwrap();
    ctx.cancel_pointer_button_for(
        registration,
        PointerButton::Primary,
        Id::unique("different-owner"),
    );
    let retained = frame(&ctx, 1.01, Vec::new(), sense);
    assert!(!retained.primary_down);
    assert!(retained.dragged);
    assert!(!retained.stopped);
    assert!(ctx.input(|input| input.pointer.secondary_down()));
    let state = ctx.interaction_snapshot(Clone::clone);
    assert_eq!(state.dragged, Some(Id::unique("owned")));
}

#[test]
fn positionless_release_ends_hold_without_hover_or_click_and_next_press_works() {
    for button in [PointerButton::Primary, PointerButton::Secondary] {
        let ctx = Context::default();
        let sense = Sense::click_and_drag();
        frame(&ctx, 0.9, Vec::new(), sense);
        frame(&ctx, 1.0, vec![self::button(button, true)], sense);
        let released = frame(
            &ctx,
            2.5,
            vec![
                Event::PointerGone,
                Event::PointerButtonReleased {
                    button,
                    modifiers: Modifiers::NONE,
                },
            ],
            sense,
        );
        assert!(!released.clicked);
        assert!(!ctx.input(|input| input.pointer.button_down(button)));
        assert_eq!(ctx.input(|input| input.pointer.hover_pos()), None);
        assert_eq!(ctx.input(|input| input.pointer.press_origin()), None);
        assert_eq!(
            ctx.input(|input| input
                .pointer
                .released_buttons_with_click_counts()
                .collect::<Vec<_>>()),
            vec![(button, None)]
        );
        let next = frame(
            &ctx,
            3.0,
            vec![
                self::button(PointerButton::Primary, true),
                self::button(PointerButton::Primary, false),
            ],
            sense,
        );
        assert!(next.clicked);
    }
}

#[test]
fn ordered_release_facts_keep_unknown_termination_separate_from_later_points() {
    let ctx = Context::default();
    let actual = pos2(40.0, 30.0);
    let later = pos2(70.0, 40.0);
    frame(
        &ctx,
        1.0,
        vec![button(PointerButton::Primary, true)],
        Sense::click_and_drag(),
    );
    frame(
        &ctx,
        1.1,
        vec![
            Event::PointerGone,
            Event::PointerButtonReleased {
                button: PointerButton::Primary,
                modifiers: Modifiers::NONE,
            },
            Event::PointerMoved(later),
            button(PointerButton::Secondary, true),
            button(PointerButton::Secondary, false),
        ],
        Sense::click_and_drag(),
    );
    assert_eq!(
        ctx.input(|i| i.pointer.button_releases().collect::<Vec<_>>()),
        vec![
            (PointerButton::Primary, None),
            (PointerButton::Secondary, Some(actual))
        ]
    );
    assert!(ctx.input(|i| i.pointer.primary_released()));
    assert_eq!(
        ctx.input(|i| i.pointer.button_release_position(PointerButton::Primary)),
        None
    );
    assert_eq!(
        ctx.input(|i| i.pointer.button_release_position(PointerButton::Secondary)),
        Some(actual)
    );
}

#[test]
fn unknown_drag_stop_has_no_operation_coordinate_but_does_end_the_drag() {
    let ctx = Context::default();
    let sense = Sense::drag();
    frame(&ctx, 0.9, Vec::new(), sense);
    assert!(frame(&ctx, 1.0, vec![button(PointerButton::Primary, true)], sense).dragged);
    let end = frame(
        &ctx,
        1.1,
        vec![
            Event::PointerGone,
            Event::PointerButtonReleased {
                button: PointerButton::Primary,
                modifiers: Modifiers::NONE,
            },
            Event::PointerMoved(pos2(40.0, 30.0)),
        ],
        sense,
    );
    assert!(end.stopped);
    assert!(!end.dragged && !end.clicked && !end.primary_down);
    assert_eq!(
        ctx.read_response(Id::unique("owned"))
            .unwrap()
            .interact_pointer_pos(),
        None
    );
}

#[test]
fn unknown_stop_and_same_widget_pending_press_have_separate_positions() {
    let ctx = Context::default();
    let sense = Sense::click_and_drag();
    frame(&ctx, 0.9, Vec::new(), sense);
    frame(&ctx, 1.0, vec![button(PointerButton::Primary, true)], sense);
    assert!(
        frame(
            &ctx,
            1.1,
            vec![Event::PointerMoved(pos2(85.0, 30.0))],
            sense
        )
        .dragged
    );
    frame(
        &ctx,
        1.2,
        vec![
            Event::PointerGone,
            Event::PointerButtonReleased {
                button: PointerButton::Primary,
                modifiers: Modifiers::NONE,
            },
            button(PointerButton::Primary, true),
        ],
        sense,
    );
    let pending = ctx.read_response(Id::unique("owned")).unwrap();
    assert!(pending.is_pointer_button_down_on());
    assert!(!pending.dragged() && !pending.drag_started() && pending.drag_stopped());
    assert_eq!(pending.interact_pointer_pos(), Some(pos2(40.0, 30.0)));
    assert_eq!(pending.drag_stopped_pos(), None);
    assert!(
        frame(
            &ctx,
            1.3,
            vec![button(PointerButton::Primary, false)],
            sense
        )
        .clicked
    );
}

#[test]
fn old_drag_stop_and_new_completed_click_keep_their_own_release_positions() {
    for known_old_end in [false, true] {
        let ctx = Context::default();
        let sense = Sense::click_and_drag();
        frame(&ctx, 0.9, Vec::new(), sense);
        frame(&ctx, 1.0, vec![button(PointerButton::Primary, true)], sense);
        let old_end = pos2(85.0, 30.0);
        assert!(frame(&ctx, 1.1, vec![Event::PointerMoved(old_end)], sense).dragged);
        let old_release = if known_old_end {
            Event::PointerButton {
                pos: old_end,
                button: PointerButton::Primary,
                pressed: false,
                modifiers: Modifiers::NONE,
            }
        } else {
            Event::PointerButtonReleased {
                button: PointerButton::Primary,
                modifiers: Modifiers::NONE,
            }
        };
        frame(
            &ctx,
            1.2,
            vec![
                Event::PointerGone,
                old_release,
                button(PointerButton::Primary, true),
                button(PointerButton::Primary, false),
                Event::PointerMoved(pos2(90.0, 40.0)),
            ],
            sense,
        );
        let response = ctx.read_response(Id::unique("owned")).unwrap();
        assert!(response.clicked() && response.drag_stopped());
        assert!(!response.dragged() && !response.is_pointer_button_down_on());
        assert_eq!(response.interact_pointer_pos(), Some(pos2(40.0, 30.0)));
        assert_eq!(
            ctx.input(|input| input.pointer.button_click_position(PointerButton::Primary)),
            Some(pos2(40.0, 30.0))
        );
        assert_eq!(
            response.drag_stopped_pos(),
            known_old_end.then_some(old_end)
        );
        assert_eq!(ctx.pointer_latest_pos(), Some(pos2(90.0, 40.0)));
    }
}

#[test]
fn completed_click_position_is_not_replaced_by_a_new_held_press() {
    let ctx = Context::default();
    let sense = Sense::click_and_drag();
    frame(&ctx, 0.9, Vec::new(), sense);
    frame(
        &ctx,
        1.0,
        vec![
            button(PointerButton::Primary, true),
            button(PointerButton::Primary, false),
            Event::PointerButton {
                pos: pos2(70.0, 30.0),
                button: PointerButton::Primary,
                pressed: true,
                modifiers: Modifiers::NONE,
            },
            Event::PointerMoved(pos2(90.0, 30.0)),
        ],
        sense,
    );
    let response = ctx.read_response(Id::unique("owned")).unwrap();
    assert!(response.clicked() && response.is_pointer_button_down_on());
    assert_eq!(response.interact_pointer_pos(), Some(pos2(90.0, 30.0)));
    assert_eq!(
        ctx.input(|input| input.pointer.button_click_position(PointerButton::Primary)),
        Some(pos2(40.0, 30.0))
    );
    assert_eq!(
        ctx.input(|input| input.pointer.press_origin()),
        Some(pos2(70.0, 30.0))
    );
}

#[test]
fn escape_drag_stop_does_not_borrow_a_fresh_completed_click_position() {
    let ctx = Context::default();
    let sense = Sense::click_and_drag();
    frame(&ctx, 0.9, Vec::new(), sense);
    frame(&ctx, 1.0, vec![button(PointerButton::Primary, true)], sense);
    assert!(
        frame(
            &ctx,
            1.1,
            vec![Event::PointerMoved(pos2(85.0, 30.0))],
            sense
        )
        .dragged
    );
    frame(
        &ctx,
        1.2,
        vec![
            Event::Key {
                key: crate::Key::Escape,
                physical_key: Some(crate::Key::Escape),
                pressed: true,
                repeat: false,
                modifiers: Modifiers::NONE,
            },
            Event::PointerButton {
                pos: pos2(85.0, 30.0),
                button: PointerButton::Primary,
                pressed: false,
                modifiers: Modifiers::NONE,
            },
            button(PointerButton::Primary, true),
            button(PointerButton::Primary, false),
            Event::PointerMoved(pos2(90.0, 40.0)),
        ],
        sense,
    );
    let response = ctx.read_response(Id::unique("owned")).unwrap();
    assert!(response.clicked() && response.drag_stopped());
    assert!(!response.dragged() && !response.is_pointer_button_down_on());
    assert_eq!(
        response.drag_stopped_pos(),
        None,
        "Escape ended the old drag before the fresh click"
    );
    assert_eq!(response.interact_pointer_pos(), Some(pos2(40.0, 30.0)));
    assert_eq!(
        ctx.input(|input| input.pointer.button_click_position(PointerButton::Primary)),
        Some(pos2(40.0, 30.0))
    );
}

#[test]
fn programmatic_drag_stop_and_replacement_do_not_reuse_an_earlier_release_position() {
    for replacement in [false, true] {
        let ctx = Context::default();
        let sense = Sense::drag();
        let owner = Id::unique("owned");
        frame(&ctx, 0.9, Vec::new(), sense);
        assert!(frame(&ctx, 1.0, vec![button(PointerButton::Primary, true)], sense).dragged);
        frame(
            &ctx,
            1.1,
            vec![button(PointerButton::Primary, false)],
            sense,
        );
        assert_eq!(
            ctx.read_response(owner).unwrap().drag_stopped_pos(),
            Some(pos2(40.0, 30.0))
        );
        ctx.set_dragged_id(owner);
        if replacement {
            ctx.set_dragged_id(Id::unique("replacement"));
        } else {
            ctx.stop_dragging();
        }
        assert_eq!(ctx.drag_stopped_id(), Some(owner));
        assert_eq!(
            ctx.read_response(owner).unwrap().drag_stopped_pos(),
            None,
            "a non-pointer terminal has no operation point"
        );
    }
}
