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
