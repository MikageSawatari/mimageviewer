//! Actual close-mode handler driven by physical button times across stalled passes.
use super::*;

fn edge(pos: egui::Pos2, pressed: bool, time: f64) -> Vec<egui::Event> {
    vec![
        egui::Event::PointerMoved(pos),
        egui::Event::PointerButtonTime {
            button: egui::PointerButton::Primary,
            pressed,
            time,
        },
        button(pos, pressed, egui::Modifiers::NONE),
    ]
}

fn timed_click(press: f64, release: f64) -> Vec<egui::Event> {
    let mut events = edge(canvas_pos(), true, press);
    events.extend(edge(canvas_pos(), false, release));
    events
}

fn canvas() -> Canvas {
    let mut canvas = Canvas::new();
    canvas
        .ctx
        .options_mut(|o| o.input_options.max_double_click_delay = 0.5);
    assert_eq!(canvas.merge(vec![]), (false, false));
    canvas
}

fn merge(canvas: &mut Canvas, pass: f64, events: Vec<egui::Event>) -> (bool, bool) {
    canvas.time = pass - 0.04;
    canvas.merge(events)
}

#[test]
fn close_mode_919_timestamped_stall_between_clicks_closes_on_second_click() {
    let _input = crate::key_input::lock_test_input();
    let mut canvas = canvas();
    assert_eq!(
        merge(&mut canvas, 1.08, timed_click(1.0, 1.08)),
        (false, false)
    );
    assert_eq!(merge(&mut canvas, 1.1, vec![]), (false, false));
    assert_eq!(
        merge(&mut canvas, 1.8, timed_click(1.25, 1.33)),
        (true, false)
    );
    assert_eq!(canvas.raw_click_counts(), [2]);
}

#[test]
fn close_mode_919_timestamped_stall_during_press_remains_click_and_closes() {
    let _input = crate::key_input::lock_test_input();
    let mut canvas = canvas();
    assert_eq!(
        merge(&mut canvas, 1.0, edge(canvas_pos(), true, 1.0)),
        (false, false)
    );
    assert_eq!(merge(&mut canvas, 1.02, vec![]), (false, false));
    assert_eq!(
        merge(&mut canvas, 1.92, edge(canvas_pos(), false, 1.08)),
        (false, false)
    );
    assert_eq!(canvas.raw_click_counts(), [1]);
    assert_eq!(
        merge(&mut canvas, 2.0, edge(canvas_pos(), true, 1.25)),
        (false, false)
    );
    assert_eq!(
        merge(&mut canvas, 2.08, edge(canvas_pos(), false, 1.33)),
        (true, false)
    );
    assert_eq!(canvas.raw_click_counts(), [2]);
}

#[test]
fn close_mode_919_timestamped_slow_clicks_in_same_pass_do_not_close() {
    let _input = crate::key_input::lock_test_input();
    let mut canvas = canvas();
    let mut events = timed_click(1.0, 1.08);
    events.extend(timed_click(2.0, 2.08));
    assert_eq!(merge(&mut canvas, 2.8, events), (false, false));
    assert_eq!(canvas.raw_click_counts(), [1, 1]);
}

#[test]
fn close_mode_919_timestamped_opening_pair_does_not_carry_into_viewer() {
    let _input = crate::key_input::lock_test_input();
    let mut canvas = canvas();
    // The opening double-click was adjudicated by egui outside the canvas owner.
    canvas
        .ctx
        .run_ui(
            egui::RawInput {
                screen_rect: Some(viewport()),
                time: Some(1.33),
                input_clock: Some(1.33),
                events: timed_click(1.0, 1.08)
                    .into_iter()
                    .chain(timed_click(1.25, 1.33))
                    .collect(),
                ..Default::default()
            },
            |_| {},
        )
        .drop_without_applying_deltas();
    assert_eq!(merge(&mut canvas, 1.34, vec![]), (false, false));
    assert_eq!(
        merge(&mut canvas, 2.0, timed_click(1.4, 1.48)),
        (false, false)
    );
    assert_eq!(canvas.raw_click_counts(), [3]);
    assert_eq!(canvas.app.fs_front_pointer.clicks.current_accepted, 1);
}

#[test]
fn close_mode_919_timestamped_reset_pair_then_close_pair() {
    let _input = crate::key_input::lock_test_input();
    let mut canvas = canvas();
    canvas.app.fs_zoom = 2.0;
    assert_eq!(
        merge(&mut canvas, 1.08, timed_click(1.0, 1.08)),
        (false, false)
    );
    assert_eq!(
        merge(&mut canvas, 1.8, timed_click(1.25, 1.33)),
        (false, false)
    );
    assert_eq!(canvas.app.fs_zoom, 1.0);
    assert_eq!(
        merge(&mut canvas, 1.9, timed_click(1.4, 1.48)),
        (false, false)
    );
    assert_eq!(
        merge(&mut canvas, 2.0, timed_click(1.55, 1.63)),
        (true, false)
    );
}
