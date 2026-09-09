// Real native mouse move smoke scenario.

fn root_host_ready(s) {
    for window in s.windows {
        if window.role == "root" && window.host_ready {
            return true;
        }
    }
    false
}

fn log_bootstrap_snapshot(s) {
    let root_count = 0;
    let ready_root_count = 0;
    for window in s.windows {
        if window.role == "root" {
            root_count += 1;
            if window.host_ready {
                ready_root_count += 1;
            }
        }
        log("native-mouse-move: bootstrap-window"
            + " role=" + window.role
            + " window_id=" + window.window_id
            + " context_serial=" + window.context_serial
            + " viewport=" + window.viewport
            + " host_incarnation=" + window.host_incarnation
            + " hwnd=" + window.hwnd
            + " backend_token=" + window.backend_token
            + " host_ready=" + window.host_ready
            + " residence=" + window.residence
            + " media_kind=" + window.media_kind
            + " page_index=" + window.page_index
            + " items_generation=" + window.items_generation);
    }
    log("native-mouse-move: bootstrap"
        + " is_fullscreen=" + s.is_fullscreen
        + " target_registered=" + s.target_registered
        + " focused=" + s.focused
        + " items_len=" + s.items_len
        + " pending_thumbs=" + s.pending_thumbs
        + " target_viewport=" + s.target_viewport
        + " target_rendered=" + s.target_rendered
        + " root_count=" + root_count
        + " ready_root_count=" + ready_root_count
        + " check_not_fullscreen=" + !s.is_fullscreen
        + " check_items_len_1=" + (s.items_len == 1)
        + " check_pending_zero=" + (s.pending_thumbs == 0)
        + " check_root_ready=" + (ready_root_count > 0));
}

fn detached_videos(s) {
    let videos = [];
    for window in s.windows {
        if window.role == "detached" && window.host_ready && window.media_kind == "video" {
            videos.push(window);
        }
    }
    videos
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

fn log_move(label, receipt) {
    log("native-mouse-move: " + label
        + " token=" + receipt.token
        + " owner=" + receipt.owner_hwnd
        + " presenter=" + receipt.presenter_hwnd
        + " source_epoch=" + receipt.source_epoch
        + " generation=" + receipt.generation
        + " requested=(" + receipt.requested_client_x + "," + receipt.requested_client_y + ")"
        + " actual=(" + receipt.actual_client_x + "," + receipt.actual_client_y + ")");
}

log("native-mouse-move: wait for root grid and open the synthetic video");
try {
    wait_until(|s|
        !s.is_fullscreen
            && s.items_len == 1
            && s.pending_thumbs == 0
            && root_host_ready(s),
        30000
    );
} catch (error) {
    log_bootstrap_snapshot(snapshot());
    throw;
}
log("native-mouse-move: bootstrap wait satisfied");
log_bootstrap_snapshot(snapshot());

let root = select_root();
if root.target_mode != "targeted" || !root.current {
    fail("select_root did not establish a current targeted owner");
}
run_action("GridMoveFirst");
run_action("GridOpenSelected");
wait_until(|s| detached_videos(s).len == 1, 30000);

let videos = detached_videos(snapshot());
if videos.len != 1 {
    fail("the synthetic video did not open in exactly one detached host");
}
let video = videos[0];
let selected = select_window(video.window_id, video.context_serial);
if selected.target_mode != "targeted" || !selected.current {
    fail("select_window did not establish the detached video owner");
}

let first = move_native_canvas(0.35, 0.40, 30000);
log_move("first", first);
let second = move_native_canvas(0.65, 0.60, 30000);
log_move("second", second);

if first.token == second.token {
    fail("the two native mouse steps reused a diagnostic token");
}
if first.owner_hwnd != second.owner_hwnd
        || first.presenter_hwnd != second.presenter_hwnd
        || first.source_epoch != second.source_epoch
        || first.generation != second.generation {
    fail("the native target identity changed between mouse moves");
}
if first.requested_client_x == second.requested_client_x
        && first.requested_client_y == second.requested_client_y {
    fail("the two requested native mouse points were identical");
}
if first.actual_client_x == second.actual_client_x
        && first.actual_client_y == second.actual_client_y {
    fail("the two actual WndProc mouse points were identical");
}
if !contains_window(snapshot(), video) {
    fail("the selected detached video identity changed during native mouse input");
}

let current = selected_target();
if current.target_mode != "targeted" || !current.current {
    fail("the exact selected detached video owner is no longer current");
}
log("native-mouse-move: both tagged moves reached pump and render routes");
