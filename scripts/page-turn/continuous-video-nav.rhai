// 1.77 repro: in continuous reading, the cross-axis key navigates onto a video
// where the wheel would have skipped it, and the report says you cannot get back.
//
// Run against a folder whose middle item is a video, e.g.
// C:\tmp\miv-continuous-video (images p01..p11, video p12, images p13..p18).
//
// The harness has no snapshot() to branch on, so every check is a wait_until
// whose timeout IS the assertion. Each one is separate, so the failure names
// which half broke.

log("continuous-video: wait for the grid to settle");
wait_until(|s|
    s.target_registered
        && s.target_rendered
        && s.focused
        && !s.is_fullscreen
        && !s.modal_open
        && !s.popup_open,
    60000
);
// `pending_thumbs == 0` is also true before the folder has requested anything
// (backlog 1.70), so give the grid a moment rather than trusting it alone.
sleep(1500);

log("continuous-video: open the first page in fullscreen");
tap_key("Enter");
wait_until(
    |s| s.is_fullscreen && s.target_rendered && s.focused && s.current_is_still_image,
    30000,
);

// tap_key only speaks navigation keys, so cycle the reading flow by action.
log("continuous-video: turn on continuous reading");
run_action("FsReadingFlowCycle");
wait_until(|s| s.continuous_reading, 10000);

// Walk forward with the cross-axis key. In vertical continuous reading that
// axis is left/right. The video sits at the 12th item, so 11 steps reach it.
log("continuous-video: step right toward the video");
let steps = 0;
while steps < 11 {
    tap_key("Right");
    sleep(250);
    steps += 1;
}

// The cross-axis key must walk past the video without entering it. Before the
// fix this landed on the video, and the way back was not the key that got you
// there: the video context has no plain Left or Right at all.
log("continuous-video: check - we walked past the video without entering it");
wait_until(|s| s.is_fullscreen && s.current_is_still_image, 8000);

// Walk the rest of the folder and back, to be sure the skip did not strand us.
log("continuous-video: walk to the end and back");
let more = 0;
while more < 6 {
    tap_key("Right");
    sleep(200);
    more += 1;
}
wait_until(|s| s.is_fullscreen && s.current_is_still_image, 8000);

let back = 0;
while back < 17 {
    tap_key("Left");
    sleep(200);
    back += 1;
}
wait_until(|s| s.is_fullscreen && s.current_is_still_image, 8000);
wait_until(|s| s.continuous_reading, 5000);

log("continuous-video: the video never took the view, in either direction");
