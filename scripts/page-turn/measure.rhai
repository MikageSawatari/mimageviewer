// Section 1.58 page-turn measurement: hold the page-turn key on a page-sized
// book and let the perf log record what the burst cost.
//
// This asserts almost nothing on purpose. It is a measurement, not a gate: the
// invariant checks live in analyze_perf.py against the spec. Run it against a
// fixture from generate_measure_fixture.py, or against a real book prepared with
// `page-turn-smoke.ps1 -Setup`.

log("measure: wait for the grid to settle");
wait_until(|s|
    s.target_registered
        && s.target_rendered
        && s.focused
        && !s.is_fullscreen
        && s.pending_thumbs == 0
        && !s.modal_open
        && !s.popup_open,
    60000
);

tap_key("Right");
tap_key("Enter");

log("measure: wait for fullscreen page-turn preconditions");
wait_until(|s|
    s.is_fullscreen
        && s.target_rendered
        && s.focused
        && s.current_is_still_image
        && s.fullscreen_raw_key_permit
        && !s.continuous_reading
        && !s.modal_open
        && !s.popup_open
        && s.has_next_page,
    30000
);

// Let the first page finish settling, so the burst measures steady-state page
// turning rather than the cost of arriving in fullscreen.
sleep(1500);

set_repeat(250, 30);
log("measure: hold Right for 6 seconds");
hold_key("Right", 6000);

// Let the last page materialize, so the tail of the burst is in the log.
sleep(1500);
log("measure: done");
