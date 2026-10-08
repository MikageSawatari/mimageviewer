// Reveals the native video top bar with one real MouseMove and observes the
// actual native_top_panorama Response. No click, wheel, key, or pan is sent.

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

log("native-top-panorama-hover: wait for root grid and open the synthetic video");
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

let observed = reveal_native_top_panorama(30000);
let moved = observed.mouse;
let target = observed.target;
log("native-top-panorama-hover: move"
    + " token=" + moved.token
    + " owner=" + moved.owner_hwnd
    + " presenter=" + moved.presenter_hwnd
    + " source_epoch=" + moved.source_epoch
    + " generation=" + moved.generation
    + " requested=(" + moved.requested_client_x + "," + moved.requested_client_y + ")"
    + " actual=(" + moved.actual_client_x + "," + moved.actual_client_y + ")");
log("native-top-panorama-hover: target"
    + " token=" + target.token
    + " classification=" + target.classification
    + " panorama_pose_present=" + target.panorama_pose_present
    + " video_zoom_present=" + target.video_zoom_present
    + " ppp=" + target.pixels_per_point
    + " client=(" + target.client_x + "," + target.client_y + ")"
    + " rect=(" + target.rect_min_x + "," + target.rect_min_y
    + ")-(" + target.rect_max_x + "," + target.rect_max_y + ")"
    + " interact=(" + target.interact_rect_min_x + "," + target.interact_rect_min_y
    + ")-(" + target.interact_rect_max_x + "," + target.interact_rect_max_y + ")"
    + " clip=(" + target.clip_rect_min_x + "," + target.clip_rect_min_y
    + ")-(" + target.clip_rect_max_x + "," + target.clip_rect_max_y + ")"
    + " layer=" + target.layer_id);

if moved.requested_client_x != moved.actual_client_x
        || moved.requested_client_y != moved.actual_client_y {
    fail("the top-hover WndProc receipt did not preserve the requested client point");
}
if moved.requested_client_y <= 0
        || moved.requested_client_y > 36.0 * target.pixels_per_point + 1.0 {
    fail("the MouseMove did not use the hidden 36pt top-hover activation contract");
}
if target.name != "native_top_panorama" || !target.enabled || !target.senses_click {
    fail("native_top_panorama was not observed as an enabled click Response");
}
if target.classification == "unknown" {
    fail("native_top_panorama metadata classification remained unknown");
}
if target.owner_hwnd != moved.owner_hwnd
        || target.presenter_hwnd != moved.presenter_hwnd
        || target.source_epoch != moved.source_epoch
        || target.generation != moved.generation {
    fail("the named target did not belong to the exact MouseMove owner/source/host");
}
if target.rect_min_x != target.interact_rect_min_x
        || target.rect_min_y != target.interact_rect_min_y
        || target.rect_max_x != target.interact_rect_max_x
        || target.rect_max_y != target.interact_rect_max_y {
    fail("native_top_panorama rect and interact_rect differ in the committed pass");
}
if target.rect_min_x < target.clip_rect_min_x
        || target.rect_min_y < target.clip_rect_min_y
        || target.rect_max_x > target.clip_rect_max_x
        || target.rect_max_y > target.clip_rect_max_y {
    fail("native_top_panorama escaped its actual UI clip rect");
}
let target_center_x = (target.rect_min_x + target.rect_max_x) * 0.5 * target.pixels_per_point;
let target_center_y = (target.rect_min_y + target.rect_max_y) * 0.5 * target.pixels_per_point;
if abs(target.client_x - target_center_x) > 1.0
        || abs(target.client_y - target_center_y) > 1.0 {
    fail("native_top_panorama physical client point does not match its observed DPI-scaled rect");
}
if !contains_window(snapshot(), video) {
    fail("the selected detached video identity changed during hover observation");
}
let current = selected_target();
if current.target_mode != "targeted" || !current.current {
    fail("the exact selected detached video owner is no longer current");
}
log("native-top-panorama-hover: tagged MouseMove reached pump/render and actual Response was observed");
