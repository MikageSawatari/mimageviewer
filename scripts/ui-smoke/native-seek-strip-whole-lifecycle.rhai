// Real native seek-strip lifecycle smoke scenario.
// Uses only tagged Windows MouseMove input and exact per-window test-script observations.

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

fn exact_window(s, expected) {
    for window in s.windows {
        if same_identity(window, expected) {
            return window;
        }
    }
    fail("the exact detached video owner disappeared");
}

fn strip_state_is(s, expected, state) {
    for window in s.windows {
        if same_identity(window, expected) {
            return window.seek_strip.state == state;
        }
    }
    false
}

fn strip_ready(s, expected) {
    for window in s.windows {
        if same_identity(window, expected) {
            let strip = window.seek_strip;
            return strip.state == "visible"
                && strip.mode == "thumbnails"
                && strip.span == "whole"
                && strip.receipt_present
                && strip.receipt_visible
                && strip.receipt_window_applied
                && strip.receipt_session_id == strip.session_id
                && strip.receipt_source_epoch == strip.source_epoch
                && strip.receipt_generation >= 0
                && strip.receipt_layout_revision == strip.layout_revision
                && strip.receipt_reported_count > 9
                && strip.visible_count == strip.receipt_reported_count
                && strip.axis_cell_count == strip.receipt_reported_count
                && strip.worker_instance_id > 0
                && strip.decoder_open_count > 0
                && strip.worker_status == "running"
                && strip.last_sent_request_id > 0
                && strip.last_finished_request_id == strip.last_sent_request_id;
        }
    }
    false
}

fn suspended_and_settled(s, expected) {
    for window in s.windows {
        if same_identity(window, expected) {
            let strip = window.seek_strip;
            return strip.state == "suspended"
                && strip.receipt_present
                && !strip.receipt_visible
                && strip.receipt_session_id == strip.session_id
                && strip.receipt_source_epoch == strip.source_epoch
                && strip.receipt_generation >= 0
                && strip.visible_count > 9
                && strip.axis_cell_count == strip.visible_count
                && strip.worker_instance_id > 0
                && strip.decoder_open_count > 0
                && strip.worker_status == "running"
                && strip.last_sent_request_id > 0
                && strip.last_finished_request_id == strip.last_sent_request_id;
        }
    }
    false
}

fn log_strip(label, strip) {
    log("native-seek-strip-whole-lifecycle: " + label
        + " state=" + strip.state
        + " session=" + strip.session_id
        + " source_epoch=" + strip.source_epoch
        + " generation=" + strip.receipt_generation
        + " revision=" + strip.layout_revision
        + " reported_count=" + strip.receipt_reported_count
        + " window_applied=" + strip.receipt_window_applied
        + " visible_count=" + strip.visible_count
        + " axis_count=" + strip.axis_cell_count
        + " request=" + strip.last_sent_request_id
        + " finished=" + strip.last_finished_request_id
        + " worker=" + strip.worker_instance_id
        + " decoder_opens=" + strip.decoder_open_count
        + " worker_status=" + strip.worker_status);
}

log("native-seek-strip-whole-lifecycle: wait for root grid and open the synthetic video");
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

// Force the same Suspended -> successful Visible transition that used to lose the real window.
let first_center = move_native_canvas(0.5, 0.5, 30000);
wait_until(|s| strip_state_is(s, video, "suspended"), 30000);
let first_hidden = exact_window(snapshot(), video).seek_strip;
log_strip("first-hidden", first_hidden);

let first_reveal = reveal_native_top_panorama(30000);
wait_until(|s| strip_ready(s, video), 30000);
let first_visible = exact_window(snapshot(), video).seek_strip;
log_strip("first-visible", first_visible);

if first_reveal.mouse.source_epoch != first_visible.source_epoch
        || first_reveal.mouse.generation != first_visible.receipt_generation {
    fail("the first Visible receipt did not belong to the exact MouseMove source/generation");
}
if first_reveal.target.name != "native_top_panorama"
        || !first_reveal.target.enabled
        || !first_reveal.target.senses_click {
    fail("the first HUD reveal did not observe the enabled native_top_panorama Response");
}

let second_center = move_native_canvas(0.5, 0.5, 30000);
if first_center.token == second_center.token {
    fail("the two center MouseMove steps reused a delivery token");
}
wait_until(|s| suspended_and_settled(s, video), 30000);
let stable_hidden = exact_window(snapshot(), video).seek_strip;
log_strip("stable-hidden", stable_hidden);

// A bounded quiet interval proves that Hidden itself does not issue another request.
sleep(750);
let quiet_hidden = exact_window(snapshot(), video).seek_strip;
if quiet_hidden.state != "suspended"
        || quiet_hidden.session_id != stable_hidden.session_id
        || quiet_hidden.source_epoch != stable_hidden.source_epoch
        || quiet_hidden.items_generation != stable_hidden.items_generation
        || quiet_hidden.worker_instance_id != stable_hidden.worker_instance_id
        || quiet_hidden.visible_count != stable_hidden.visible_count
        || quiet_hidden.axis_cell_count != stable_hidden.axis_cell_count
        || quiet_hidden.last_sent_request_id != stable_hidden.last_sent_request_id
        || quiet_hidden.decoder_open_count != stable_hidden.decoder_open_count
        || quiet_hidden.worker_status != "running" {
    fail("the Hidden interval changed the owner, geometry, worker, request, or decoder state");
}

let second_reveal = reveal_native_top_panorama(30000);
wait_until(|s| strip_ready(s, video), 30000);
let second_visible = exact_window(snapshot(), video).seek_strip;
log_strip("second-visible", second_visible);

if second_visible.session_id != stable_hidden.session_id
        || second_visible.worker_instance_id != stable_hidden.worker_instance_id {
    fail("HUD redisplay replaced the seek-strip resource session or worker");
}
if second_visible.decoder_open_count != stable_hidden.decoder_open_count {
    fail("HUD redisplay opened another seek-strip decoder");
}
if second_visible.receipt_reported_count != first_visible.receipt_reported_count
        || second_visible.visible_count != second_visible.receipt_reported_count
        || second_visible.axis_cell_count != second_visible.receipt_reported_count {
    fail("HUD redisplay did not preserve the exact whole-strip geometry");
}
if second_reveal.mouse.source_epoch != second_visible.source_epoch
        || second_reveal.mouse.generation != second_visible.receipt_generation {
    fail("the second Visible receipt did not belong to the exact MouseMove source/generation");
}
if !same_identity(exact_window(snapshot(), video), video) {
    fail("the detached video identity changed during seek-strip lifecycle observation");
}
let current = selected_target();
if current.target_mode != "targeted" || !current.current {
    fail("the exact selected detached video owner is no longer current");
}
log("native-seek-strip-whole-lifecycle: real whole geometry and HUD resource lifetime verified");
