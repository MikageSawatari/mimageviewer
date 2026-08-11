// Section 1.58 spread measurement on a real book. RTL uses Left to advance
// and Right to go back. Keeping both holds in one process exposes revisit
// decode behavior without changing the viewer settings between runs.

log("spread RTL: wait for the grid to settle");
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

log("spread RTL: wait for fullscreen page-turn preconditions");
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

run_action("FsSpreadRtl");
wait_until(|s| s.spread_mode == "Rtl", 10000);
sleep(1500);

set_repeat(250, 30);
log("spread RTL: hold Left (forward) for 5 seconds");
hold_key("Left", 5000);
sleep(1500);

log("spread RTL: hold Right (backward) for 5 seconds");
hold_key("Right", 5000);
sleep(1500);

log("spread RTL: done");
