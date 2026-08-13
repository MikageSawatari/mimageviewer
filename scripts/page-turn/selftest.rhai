log("self-test: wait for isolated grid");
wait_until(|s|
    s.target_registered
        && s.target_rendered
        && s.focused
        && !s.is_fullscreen
        && s.items_len > 0
        && s.pending_thumbs == 0
        && !s.modal_open
        && !s.popup_open
        && !s.ime_active
        && !s.text_input_or_pending_focus,
    20000
);

tap_key("Right");
tap_key("Enter");

log("self-test: wait for fullscreen page-turn preconditions");
wait_until(|s|
    s.is_fullscreen
        && s.target_registered
        && s.target_rendered
        && s.focused
        && s.current_is_still_image
        && !s.music_view_active
        && !s.modal_open
        && !s.context_menu_open
        && !s.popup_open
        && !s.ime_active
        && !s.text_input_or_pending_focus
        && !s.overlay_edit_active
        && !s.capture_region_selection
        && s.fullscreen_raw_key_permit
        && !s.continuous_reading
        && s.spread_mode == "Single"
        && s.has_previous_page
        && s.has_next_page
        && s.fs_idx == 1,
    20000
);

set_repeat(250, 30);
log("self-test: hold Right for 5 seconds");
hold_key("Right", 5000);
wait_until(|s| s.fs_idx >= 5, 10000);
log("self-test: page progress established");
