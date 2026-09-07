// Multi-window PDF smoke scenario.

fn detached_windows(s) {
    let windows = [];
    for window in s.windows {
        if window.role == "detached" && window.host_ready {
            windows.push(window);
        }
    }
    windows
}

fn root_host_ready(s) {
    for window in s.windows {
        if window.role == "root" && window.host_ready {
            return true;
        }
    }
    false
}

fn same_identity(actual, expected) {
    actual.role == expected.role
        && actual.window_id == expected.window_id
        && actual.context_serial == expected.context_serial
        && actual.viewport == expected.viewport
        && actual.host_incarnation == expected.host_incarnation
        && actual.hwnd == expected.hwnd
        && actual.backend_token == expected.backend_token
}

fn contains_window(s, expected) {
    for window in s.windows {
        if same_identity(window, expected) {
            return true;
        }
    }
    false
}

fn contains_window_id(s, window_id) {
    for window in s.windows {
        if window.role == "detached" && window.window_id == window_id {
            return true;
        }
    }
    false
}

fn navigated_window_ready(s, expected) {
    for window in s.windows {
        if same_identity(window, expected) {
            return expected.page_index == 0
                && window.page_index == 1
                && window.items_generation == expected.items_generation
                && window.full_texture_painted
                && window.paint_matches_current_page
                && window.paint_revision > expected.paint_revision;
        }
    }
    false
}

fn sibling_unchanged(s, expected) {
    for window in s.windows {
        if same_identity(window, expected) {
            return window.page_index == expected.page_index
                && window.items_generation == expected.items_generation;
        }
    }
    false
}

log("multi-window-pdf: wait for the root grid and host identity");
wait_until(|s|
    !s.is_fullscreen
        && s.target_registered
        && s.focused
        && s.items_len == 2
        && s.pending_thumbs == 0
        && root_host_ready(s),
    30000
);

let root_target = select_root();
if root_target.target_mode != "targeted" || !root_target.current {
    fail("select_root did not establish a current targeted owner");
}
run_action("GridMoveFirst");
run_action("GridOpenSelected");
wait_until(|s| {
    let windows = detached_windows(s);
    windows.len == 1
        && windows[0].media_kind == "pdf"
        && windows[0].full_texture_painted
        && windows[0].paint_matches_current_page
        && windows[0].paint_revision > 0
}, 30000);

let root_target = select_root();
if root_target.target_mode != "targeted" || !root_target.current {
    fail("select_root did not remain available after the first detached open");
}
run_action("GridMoveLast");
run_action("GridOpenSelected");
wait_until(|s| {
    let windows = detached_windows(s);
    windows.len == 2
        && windows[0].window_id != windows[1].window_id
        && windows[0].context_serial != windows[1].context_serial
        && windows[0].item_identity != windows[1].item_identity
        && windows[0].page_index == 0
        && windows[1].page_index == 0
        && windows[0].full_texture_painted
        && windows[1].full_texture_painted
        && windows[0].paint_matches_current_page
        && windows[1].paint_matches_current_page
}, 30000);

let before = detached_windows(snapshot());
let first = before[0];
let second = before[1];
let first_target = select_window(first.window_id, first.context_serial);
if first_target.target_mode != "targeted" || !first_target.current {
    fail("select_window did not establish the first detached owner");
}
run_action("FsPageNext");
wait_until(|s|
    navigated_window_ready(s, first) && sibling_unchanged(s, second),
    30000
);

run_action("FsClose");
wait_until(|s|
    !contains_window_id(s, first.window_id) && sibling_unchanged(s, second),
    30000
);
let stale = selected_target();
if stale.target_mode != "targeted" || stale.current {
    fail("closed detached identity did not become stale");
}

log("multi-window-pdf: done");
