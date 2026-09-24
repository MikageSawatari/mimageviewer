// Real detached-window key routing across direct RAR, ZIP, and CBR.
// The solid RAR between the first two books has no conversion-cache record.

fn wait_dump(ready, timeout_ms) {
    try {
        wait_until(ready, timeout_ms);
    } catch (error) {
        let s = snapshot();
        log("multi-window-rar-nav: wait failed; windows=" + s.windows.to_debug()
            + " items_len=" + s.items_len + " focused=" + s.focused);
        throw error;
    }
}

fn root_window(s) {
    for window in s.windows {
        if window.role == "root" { return window; }
    }
    ()
}

fn detached_window(s) {
    for window in s.windows {
        if window.role == "detached" && window.host_ready { return window; }
    }
    ()
}

fn by_id(s, id) {
    for window in s.windows {
        if window.role == "detached" && window.window_id == id { return window; }
    }
    ()
}

fn root_unchanged(s, baseline) {
    let root = root_window(s);
    root != () && root.context_serial == baseline.context_serial
        && root.items_generation == baseline.items_generation
        && root.selected_item_identity == baseline.selected_item_identity
        && root.item_identity == baseline.item_identity
}

fn current_paint_matches(window) {
    if !window.page_ready || !window.paint_matches_current_page
        || window.page_index < 0 || window.item_identity == "" { return false; }
    for paint in window.paints {
        if paint.context_serial == window.context_serial
            && paint.items_generation == window.items_generation
            && paint.item_identity == window.item_identity
            && paint.page_index == window.page_index {
            return true;
        }
    }
    false
}

fn landing_ready(s, baseline, id, serial, expected_name) {
    if !root_unchanged(s, baseline) { return false; }
    let window = by_id(s, id);
    window != () && window.context_serial == serial
        && window.item_identity.contains(expected_name)
        && current_paint_matches(window)
}

fn navigate(baseline, window, key, expected_name) {
    let target = select_window(window.window_id, window.context_serial);
    if target.target_mode != "targeted" || !target.current {
        fail("detached key target is stale before " + expected_name);
    }
    wait_dump(|s| s.target_registered && s.focused
        && by_id(s, window.window_id) != (), 10000);
    tap_key(key, "ctrl");
    wait_dump(|s| landing_ready(s, baseline, window.window_id,
        window.context_serial, expected_name), 30000);
    let landed = by_id(snapshot(), window.window_id);
    log("multi-window-rar-nav: landed " + expected_name
        + " item=" + landed.item_identity
        + " generation=" + landed.items_generation);
    landed
}

log("multi-window-rar-nav: waiting for root grid");
wait_dump(|s| !s.is_fullscreen && s.items_len == 4
    && s.pending_thumbs == 0 && root_window(s) != (), 30000);
select_root();
run_action("GridMoveFirst");
let before_open = root_window(snapshot());
if !before_open.selected_item_identity.contains("01-direct.rar") {
    fail("first grid item is not the direct RAR: " + before_open.to_debug());
}
run_action("GridOpenSelected");
wait_dump(|s| {
    let window = detached_window(s);
    window != () && window.item_identity.contains("01-direct.rar")
        && current_paint_matches(window)
        && root_unchanged(s, before_open)
}, 30000);

let first = detached_window(snapshot());
if first.window_id < 0 || first.context_serial < 0 {
    fail("detached RAR window has no stable identity");
}
log("multi-window-rar-nav: direct RAR painted");

let zip_forward = navigate(before_open, first, "Down", "06-control.zip");
let cbr_forward = navigate(before_open, zip_forward, "Down", "08-direct.cbr");
let zip_backward = navigate(before_open, cbr_forward, "Up", "06-control.zip");
let rar_backward = navigate(before_open, zip_backward, "Up", "01-direct.rar");
if rar_backward.window_id != first.window_id
    || rar_backward.context_serial != first.context_serial {
    fail("detached RAR navigation replaced its window or context");
}
log("multi-window-rar-nav: PASS");
