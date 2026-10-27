// Reference measurement only. Run on an explicitly approved disposable portable.
// No D3 readiness wait: missing preparations remain in the measured denominator.
// Page turns go through key input: run_action("FsPage*") moves without the page-turn animation.
// The reading direction is read from the main fullscreen before F12; the root snapshot no longer
// reports the detached context's spread mode afterwards.
fn prev_key(rtl) { if rtl { "Right" } else { "Left" } }
fn next_key(rtl) { if rtl { "Left" } else { "Right" } }
fn turn(key) { wait_until(|s| s.target_registered, 10000); tap_key(key); }
fn middle_grid(index) {
    wait_until(|s| s.target_registered && s.target_rendered
        && !s.is_fullscreen && s.items_len >= 16 && s.pending_thumbs == 0
        && !s.modal_open && !s.popup_open, 60000);
    select_root();
    run_action("GridMoveFirst");
    wait_until(|s| s.focused && s.selected_index == 0, 10000);
    for i in 0..index {
        let before = snapshot().selected_index;
        tap_key("Right");
        wait_until(|s| s.selected_index > before, 10000);
    }
    if snapshot().selected_index != index { fail("middle selection differs from fixture index"); }
}
fn detached(s) {
    let result = [];
    for window in s.windows {
        if window.role == "detached" && window.host_ready { result.push(window); }
    }
    result
}
fn normal_painted(window) {
    // paint evidence is recorded only for single-image draws, so spreads wait on page/viewport readiness.
    window.page_ready && window.viewport_rendered
}
fn open_main() {
    run_action("GridOpenSelected");
    wait_until(|s| s.is_fullscreen && s.target_rendered && s.current_is_still_image
        && s.fullscreen_raw_key_permit && !s.modal_open && !s.popup_open, 30000);
    select_root();
}
fn open_f12() {
    tap_key("F12");
    wait_until(|s| {
        let windows = detached(s);
        windows.len == 1 && normal_painted(windows[0])
    }, 30000);
    let window = detached(snapshot())[0];
    select_window(window.window_id,window.context_serial);
    log("back: selected window=" + window.window_id + " context=" + window.context_serial
        + " page=" + window.page_index);
    window
}

middle_grid(0);
open_main();
sleep(2500);
// Read after the first page settles; right after open the snapshot can still report Single.
let rtl = snapshot().spread_mode.starts_with("Rtl");
// Both profiles (wrap false / true) keep speculative endpoints Boundary.
turn(prev_key(rtl));
sleep(2500);
tap_key("End");
sleep(2500);
turn(next_key(rtl));
sleep(2500);
log("back-boundary-wrap: done");
