// Reveals native_top_panorama with one real MouseMove, then performs one real
// left-button Down/Up through the external helper. The App must report the
// exact normal TogglePanorama handler transition before this script succeeds.

fn root_host_ready(s) {
    for window in s.windows {
        if window.role == "root" && window.host_ready {
            return true;
        }
    }
    false
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

log("native-top-panorama-click: wait for root grid and open the synthetic video");
wait_until(|s|
    !s.is_fullscreen
        && s.items_len == 1
        && s.pending_thumbs == 0
        && root_host_ready(s),
    30000
);
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

let observed = click_native_top_panorama(30000);
let moved = observed.mouse;
let target = observed.target;
let click = observed.click;
let down = click.down;
let up = click.up;

log("native-top-panorama-click: gesture=" + click.gesture_id
    + " target=" + target.token
    + " down=" + down.token
    + " up=" + up.token
    + " receiver=" + up.receiver_hwnd
    + " zoom=" + click.app_video_zoom_scale);

if target.name != "native_top_panorama"
        || !target.enabled
        || !target.senses_click
        || target.classification != "non_panorama"
        || target.panorama_pose_present
        || target.video_zoom_present {
    fail("native_top_panorama was not the enabled non-panorama None-zoom click target");
}
if target.owner_hwnd != moved.owner_hwnd
        || target.presenter_hwnd != moved.presenter_hwnd
        || target.source_epoch != moved.source_epoch
        || target.generation != moved.generation {
    fail("the named target did not belong to the exact hover owner/source/host");
}
if moved.requested_client_x != moved.actual_client_x
        || moved.requested_client_y != moved.actual_client_y {
    fail("the hover WndProc receipt did not preserve the requested point");
}
if down.token == 0 || up.token == 0 || down.token == up.token {
    fail("Down and Up did not have distinct nonzero delivery tokens");
}
if down.receiver_hwnd != target.presenter_hwnd || up.receiver_hwnd != target.presenter_hwnd {
    fail("Down or Up reached a receiver other than the prepared presenter");
}
if down.receiver_process_id <= 0
        || down.receiver_process_id != up.receiver_process_id
        || down.receiver_thread_id <= 0
        || down.receiver_thread_id != up.receiver_thread_id {
    fail("Down and Up did not preserve the exact receiver process/thread identity");
}
if down.actual_client_x != target.client_x
        || down.actual_client_y != target.client_y
        || up.actual_client_x != target.client_x
        || up.actual_client_y != target.client_y {
    fail("Down or Up did not reach the prepared control center");
}
if down.actual_screen_x != up.actual_screen_x || down.actual_screen_y != up.actual_screen_y {
    fail("Down and Up screen points differed");
}
if click.app_video_zoom_scale != 1.0 || click.app_panorama_active {
    fail("the normal TogglePanorama handler did not apply None -> Some(1.0)");
}
if !click.pointer_released || !click.drag_latches_released {
    fail("Up completed without releasing the pointer and drag latches");
}
if !contains_window(snapshot(), video) {
    fail("the selected detached video identity changed during the click");
}
let current = selected_target();
if current.target_mode != "targeted" || !current.current {
    fail("the exact selected detached video owner is no longer current");
}
log("native-top-panorama-click: exact Down/Up, Response command, and App effect completed");
