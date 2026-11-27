use super::*;
use crate::emath::pos2;

fn edge(pressed: bool, stamp: Option<f64>) -> Vec<Event> {
    let mut events = Vec::new();
    if let Some(time) = stamp {
        events.push(Event::PointerButtonTime {
            button: PointerButton::Primary,
            pressed,
            time,
        });
    }
    events.push(Event::PointerButton {
        pos: pos2(20.0, 30.0),
        button: PointerButton::Primary,
        pressed,
        modifiers: Modifiers::NONE,
    });
    events
}

fn click(press: f64, release: f64) -> Vec<Event> {
    edge(true, Some(press))
        .into_iter()
        .chain(edge(false, Some(release)))
        .collect()
}

fn pass(pointer: PointerState, pass: f64, clock: Option<f64>, events: Vec<Event>) -> PointerState {
    pointer.begin_pass(
        pass,
        &RawInput {
            input_clock: clock,
            events,
            ..Default::default()
        },
        InputOptions {
            max_double_click_delay: 0.5,
            ..Default::default()
        },
    )
}

fn counts(pointer: &PointerState) -> Vec<u32> {
    pointer
        .released_buttons_with_click_counts()
        .filter_map(|(_, click)| click.map(|(_, count)| count))
        .collect()
}

#[test]
fn timestamp_919_stall_between_clicks_uses_actual_interval() {
    let pointer = pass(PointerState::default(), 1.08, Some(1.08), click(1.0, 1.08));
    let pointer = pass(pointer, 1.8, Some(1.8), click(1.25, 1.33));
    assert_eq!(counts(&pointer), [2]);
    assert_eq!(pointer.press_click_count(), 2);
}

#[test]
fn timestamp_919_delayed_release_uses_press_release_interval() {
    let pointer = pass(
        PointerState::default(),
        1.0,
        Some(1.0),
        edge(true, Some(1.0)),
    );
    let pointer = pass(pointer, 1.92, Some(1.92), edge(false, Some(1.08)));
    assert_eq!(counts(&pointer), [1]);
}

#[test]
fn timestamp_919_slow_clicks_batched_together_are_not_double() {
    let pointer = pass(
        PointerState::default(),
        3.0,
        Some(3.0),
        click(1.0, 1.08)
            .into_iter()
            .chain(click(2.0, 2.08))
            .collect(),
    );
    assert_eq!(counts(&pointer), [1, 1]);
}

#[test]
fn timestamp_919_annotation_allows_only_provenance_and_matching_button() {
    let marker = Event::PointerInputProvenance(crate::Id::new("timestamp proof"));
    let annotation = edge(true, Some(0.1)).remove(0);
    for (middle, valid) in [
        (vec![marker], true),
        (vec![Event::PointerMoved(pos2(20.0, 30.0))], false),
        (vec![Event::WindowFocused(true)], false),
    ] {
        let events = [vec![annotation.clone()], middle, edge(true, None)].concat();
        let pointer = pass(PointerState::default(), 1.0, Some(1.0), events);
        assert_eq!(
            pointer.button_edges().next().unwrap().time.input.is_some(),
            valid
        );
    }
    for (button, pressed) in [
        (PointerButton::Secondary, true),
        (PointerButton::Primary, false),
    ] {
        let events = [
            vec![Event::PointerButtonTime {
                button,
                pressed,
                time: 0.1,
            }],
            edge(true, None),
        ]
        .concat();
        let pointer = pass(PointerState::default(), 1.0, Some(1.0), events);
        assert_eq!(pointer.button_edges().next().unwrap().time.input, None);
    }
    let pointer = pass(PointerState::default(), 1.0, None, edge(true, Some(0.1)));
    assert_eq!(pointer.button_edges().next().unwrap().time.input, None);
}

#[test]
fn timestamp_919_translation_clamps_future_and_negative_hold_interval() {
    let pointer = pass(
        PointerState::default(),
        5.0,
        Some(10.0),
        edge(true, Some(11.0)),
    );
    assert_eq!(
        pointer.button_edges().next().unwrap().time,
        EdgeTime {
            pass: 5.0,
            input: Some(5.0)
        }
    );
    let pointer = pass(pointer, 5.1, Some(10.1), edge(false, Some(9.0)));
    assert_eq!(pointer.button_edges().next().unwrap().time.input, Some(5.0));
    assert_eq!(counts(&pointer), [1]);
}

#[test]
fn timestamp_919_mixed_click_pairs_use_pass_time_in_both_orders() {
    for first_known in [false, true] {
        let first = [
            edge(true, first_known.then_some(1.0)),
            edge(false, first_known.then_some(1.08)),
        ]
        .concat();
        let second = [
            edge(true, (!first_known).then_some(1.25)),
            edge(false, (!first_known).then_some(1.33)),
        ]
        .concat();
        let pointer = pass(PointerState::default(), 1.08, Some(1.08), first);
        let pointer = pass(pointer, 2.0, Some(2.0), second);
        assert_eq!(counts(&pointer), [1], "first_known={first_known}");
    }
}

#[test]
fn timestamp_919_mixed_press_release_uses_pass_duration_in_both_orders() {
    for press_known in [false, true] {
        let pointer = pass(
            PointerState::default(),
            1.0,
            Some(1.0),
            edge(true, press_known.then_some(1.0)),
        );
        let pointer = pass(
            pointer,
            2.0,
            Some(2.0),
            edge(false, (!press_known).then_some(1.08)),
        );
        assert_eq!(counts(&pointer), [], "press_known={press_known}");
    }
}

#[test]
fn timestamp_919_edges_preserve_order_clicks_and_cancel_removes_owned_edges() {
    let mut events = click(1.0, 1.08);
    events.extend(edge(true, Some(1.25)));
    events.push(Event::PointerButtonTime {
        button: PointerButton::Primary,
        pressed: false,
        time: 1.33,
    });
    events.push(Event::PointerButtonReleased {
        button: PointerButton::Primary,
        modifiers: Modifiers::NONE,
    });
    let mut pointer = pass(PointerState::default(), 2.0, Some(2.0), events);
    let edges: Vec<_> = pointer.button_edges().collect();
    assert_eq!(
        edges.iter().map(|edge| edge.pressed).collect::<Vec<_>>(),
        [true, false, true, false]
    );
    assert_eq!(edges[1].click.unwrap().1, 1);
    assert_eq!(edges[3].time.input, None);
    assert_eq!(edges[3].pos, None);
    assert_eq!(
        pointer
            .released_buttons_with_click_counts()
            .collect::<Vec<_>>(),
        [
            (PointerButton::Primary, edges[1].click),
            (PointerButton::Primary, None)
        ]
    );
    pointer.cancel_button(PointerButton::Primary);
    assert_eq!(pointer.button_edges().count(), 0);
}

#[test]
fn timestamp_919_pending_append_keeps_absolute_stamps_and_latest_clock() {
    let mut input = RawInput {
        time: Some(1.08),
        input_clock: Some(11.08),
        events: click(11.0, 11.08),
        ..Default::default()
    };
    input.append(RawInput {
        time: Some(3.0),
        input_clock: Some(13.0),
        events: click(12.0, 12.08),
        ..Default::default()
    });
    let taken = input.take();
    assert_eq!(taken.input_clock, Some(13.0));
    let pointer = PointerState::default().begin_pass(
        3.0,
        &taken,
        InputOptions {
            max_double_click_delay: 0.5,
            ..Default::default()
        },
    );
    assert_eq!(counts(&pointer), [1, 1]);
    let stamps: Vec<_> = pointer
        .button_edges()
        .map(|edge| edge.time.input.unwrap())
        .collect();
    assert!((stamps[0] - 1.0).abs() < 1e-10);
    assert!((stamps[2] - 2.0).abs() < 1e-10);
}

#[test]
fn timestamp_919_viewport_cancel_and_recreation_do_not_change_sibling_history() {
    use crate::{Context, ViewportId};
    let ctx = Context::default();
    let a = ViewportId::from_hash_of("timestamp-A");
    let b = ViewportId::from_hash_of("timestamp-B");
    let run = |viewport_id, pass, events| {
        let mut raw = RawInput {
            viewport_id,
            time: Some(pass),
            input_clock: Some(pass),
            events,
            ..Default::default()
        };
        for id in [a, b] {
            raw.viewports.insert(id, Default::default());
        }
        ctx.run_ui(raw, |_| {}).drop_without_applying_deltas();
    };
    run(a, 1.08, click(1.0, 1.08));
    run(b, 1.08, click(1.0, 1.08));
    let before = ctx.input_for(b, |input| input.pointer.button_edges().collect::<Vec<_>>());
    ctx.input_mut_for(a, |input| {
        input.pointer.cancel_button(PointerButton::Primary)
    });
    ctx.retire_viewport(a);
    run(a, 1.8, click(1.25, 1.33));
    assert_eq!(ctx.input_for(a, |input| counts(&input.pointer)), [1]);
    assert_eq!(
        ctx.input_for(b, |input| input.pointer.button_edges().collect::<Vec<_>>()),
        before
    );
    run(b, 1.8, click(1.25, 1.33));
    assert_eq!(ctx.input_for(b, |input| counts(&input.pointer)), [2]);
}

#[test]
fn timestamp_919_shared_interval_rule_preserves_sign_and_uses_whole_pair_clock() {
    assert_eq!(
        EdgeTime {
            pass: 9.0,
            input: Some(1.25)
        }
        .since(EdgeTime {
            pass: 1.0,
            input: Some(1.0)
        }),
        0.25
    );
    assert_eq!(
        EdgeTime {
            pass: 9.0,
            input: Some(1.25)
        }
        .since(EdgeTime::from(1.0)),
        8.0
    );
    assert_eq!(
        EdgeTime::from(9.0).since(EdgeTime {
            pass: 1.0,
            input: Some(1.0)
        }),
        8.0
    );
    assert_eq!(EdgeTime::from(1.0).since(EdgeTime::from(2.0)), -1.0);
    assert_eq!(
        EdgeTime {
            pass: 9.0,
            input: Some(1.0)
        }
        .since(EdgeTime {
            pass: 2.0,
            input: Some(1.25)
        }),
        -0.25
    );
    assert_eq!(
        EdgeTime {
            pass: 1.0,
            input: Some(0.5)
        }
        .since(EdgeTime::from(2.0)),
        -1.0
    );
    assert_eq!(
        EdgeTime::from(1.0).since(EdgeTime {
            pass: 2.0,
            input: Some(0.0)
        }),
        -1.0
    );
}
