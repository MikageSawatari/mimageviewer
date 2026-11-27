//! §1.919 investigation: the real close-mode handler when a UI stall changes which
//! egui pass a physical button edge is delivered in. egui-winit stamps every event
//! of a pass with the pass start time, so the pass times below are exactly what
//! the handler sees. `forum-919-real-window` (target/investigation) confirms the
//! delivery model with real Win32 messages.
use super::*;

/// Windows' default double-click time (GetDoubleClickTime = 500 ms).
const WINDOWS_DEFAULT_DELAY: f64 = 0.5;

fn stalled_canvas() -> Canvas {
    let canvas = Canvas::new();
    canvas
        .ctx
        .options_mut(|o| o.input_options.max_double_click_delay = WINDOWS_DEFAULT_DELAY);
    canvas
}

fn press(pos: egui::Pos2) -> Vec<egui::Event> {
    vec![
        egui::Event::PointerMoved(pos),
        button(pos, true, egui::Modifiers::NONE),
    ]
}

fn release(pos: egui::Pos2) -> Vec<egui::Event> {
    vec![button(pos, false, egui::Modifiers::NONE)]
}

impl Canvas {
    /// Run one pass whose egui time is `time` (the pass start).
    fn merge_at(&mut self, time: f64, events: Vec<egui::Event>) -> (bool, bool) {
        self.time = time - 0.04;
        self.merge(events)
    }
}

#[test]
fn close_mode_919_idle_ui_double_click_closes() {
    let _input = crate::key_input::lock_test_input();
    let mut canvas = stalled_canvas();
    let pos = canvas_pos();
    assert_eq!(canvas.merge_at(1.00, press(pos)), (false, false));
    assert_eq!(canvas.merge_at(1.08, release(pos)), (false, false));
    assert_eq!(canvas.raw_click_counts(), [1]);
    assert_eq!(canvas.merge_at(1.25, press(pos)), (false, false));
    assert_eq!(canvas.merge_at(1.33, release(pos)), (true, false));
    assert_eq!(canvas.raw_click_counts(), [2]);
}

/// (a) The pass after click 1 stalls for 700 ms and click 2 (physically 250 ms
/// after click 1) is delivered with the next pass. The pair is lost and a third
/// click is needed (the >>906 symptom).
#[test]
fn close_mode_919_stall_between_clicks_loses_the_pair_on_pass_time() {
    let _input = crate::key_input::lock_test_input();
    let mut canvas = stalled_canvas();
    let pos = canvas_pos();
    assert_eq!(canvas.merge_at(1.00, press(pos)), (false, false));
    assert_eq!(canvas.merge_at(1.08, release(pos)), (false, false));
    // 1.10: a pass begins and blocks the UI thread until 1.80.
    assert_eq!(canvas.merge_at(1.10, vec![]), (false, false));
    let mut second = press(pos);
    second.extend(release(pos));
    assert_eq!(canvas.merge_at(1.80, second), (false, false));
    assert_eq!(canvas.raw_click_counts(), [1]);
    // A third physical click inside the delay closes.
    assert_eq!(canvas.merge_at(1.95, press(pos)), (false, false));
    assert_eq!(canvas.merge_at(2.03, release(pos)), (true, false));
    assert_eq!(canvas.raw_click_counts(), [2]);
}

/// (b) The press is delivered, then a 900 ms stall delays the release (physically
/// 80 ms later). egui sees a 900 ms hold, so the release is not a click at all.
#[test]
fn close_mode_919_stall_inside_a_press_turns_the_click_into_a_hold() {
    let _input = crate::key_input::lock_test_input();
    let mut canvas = stalled_canvas();
    let pos = canvas_pos();
    assert_eq!(canvas.merge_at(1.00, press(pos)), (false, false));
    assert_eq!(canvas.merge_at(1.02, vec![]), (false, false));
    assert_eq!(canvas.merge_at(1.92, release(pos)), (false, false));
    assert_eq!(canvas.raw_click_counts(), Vec::<u32>::new());
    // The physical second click of that double click now only counts as 1.
    assert_eq!(canvas.merge_at(2.00, press(pos)), (false, false));
    assert_eq!(canvas.merge_at(2.08, release(pos)), (false, false));
    assert_eq!(canvas.raw_click_counts(), [1]);
}

/// (c) Both clicks arrive during one stall and are delivered in one pass: egui
/// gives both the same time and the pair closes.
#[test]
fn close_mode_919_both_clicks_in_one_stalled_batch_close() {
    let _input = crate::key_input::lock_test_input();
    let mut canvas = stalled_canvas();
    let pos = canvas_pos();
    assert_eq!(canvas.merge_at(1.00, vec![]), (false, false));
    assert_eq!(canvas.merge_at(1.70, double_click(pos)), (true, false));
    assert_eq!(canvas.raw_click_counts(), [1, 2]);
}
