// Isolated live regression for §1.198 wide-image idle-upgrade convergence.

const WAIT_MS = 30000;

fn same_identity(actual, expected) {
    actual.role == expected.role
        && actual.window_id == expected.window_id
        && actual.context_serial == expected.context_serial
        && actual.viewport == expected.viewport
        && actual.host_incarnation == expected.host_incarnation
        && actual.hwnd == expected.hwnd
        && actual.backend_token == expected.backend_token
}

fn root_ready(s) {
    for window in s.windows {
        if window.role == "root" && window.host_ready {
            return true;
        }
    }
    false
}

fn root_window(s) {
    for window in s.windows {
        if window.role == "root" && window.host_ready {
            return window;
        }
    }
    fail("idle198 root window disappeared");
}

fn root_unchanged(s, expected) {
    for window in s.windows {
        if window.role == "root" && same_identity(window, expected) {
            return window.items_generation == expected.items_generation;
        }
    }
    false
}

log("idle198:bootstrap:wait");
wait_until(|s|
    !s.is_fullscreen
        && s.items_len == 4
        && s.pending_thumbs == 0
        && root_ready(s),
    WAIT_MS
);

let initial_snapshot = snapshot();
let initial_root = root_window(initial_snapshot);
let selected = select_root();
if selected.target_mode != "targeted" || !selected.current {
    fail("idle198 select_root did not establish an exact targeted owner");
}
log("idle198:root"
    + " is_fullscreen=" + initial_snapshot.is_fullscreen
    + " items_len=" + initial_snapshot.items_len
    + " pending_thumbs=" + initial_snapshot.pending_thumbs
    + " target_mode=" + selected.target_mode
    + " target_current=" + selected.current
    + " context_serial=" + initial_root.context_serial
    + " viewport=" + initial_root.viewport
    + " host_incarnation=" + initial_root.host_incarnation
    + " hwnd=" + initial_root.hwnd
    + " backend_token=" + initial_root.backend_token
    + " items_generation=" + initial_root.items_generation);

run_action("GridColumnCount10");
wait_until(|s|
    !s.is_fullscreen
        && s.items_len == 4
        && s.pending_thumbs == 0
        && s.target_registered
        && s.target_viewport == "ROOT"
        && s.focused
        && root_unchanged(s, initial_root),
    WAIT_MS
);
log("idle198:small:begin");
sleep(8000);

run_action("GridColumnCount7");
log("idle198:374:begin");
sleep(30000);
log("idle198:374:end");

run_action("GridColumnCount6");
log("idle198:enlarged:begin");
sleep(30000);
log("idle198:enlarged:end");

// Leave enough time for the external runner to record an exact live/foreground
// sample after it observes the final measured marker. This is outside both
// measured idle windows.
sleep(1000);
wait_until(|s|
    !s.is_fullscreen
        && s.items_len == 4
        && s.pending_thumbs == 0
        && root_unchanged(s, initial_root),
    WAIT_MS
);
let final_snapshot = snapshot();
let final_root = root_window(final_snapshot);
let final_selection = selected_target();
if !same_identity(final_root, initial_root)
        || final_root.items_generation != initial_root.items_generation {
    fail("idle198 root identity or items generation changed");
}
if final_selection.target_mode != "targeted" || !final_selection.current {
    fail("idle198 exact targeted root selection became stale");
}
log("idle198:final"
    + " is_fullscreen=" + final_snapshot.is_fullscreen
    + " items_len=" + final_snapshot.items_len
    + " pending_thumbs=" + final_snapshot.pending_thumbs
    + " target_mode=" + final_selection.target_mode
    + " target_current=" + final_selection.current
    + " same_root=" + same_identity(final_root, initial_root)
    + " items_generation=" + final_root.items_generation);
log("idle198:done");
