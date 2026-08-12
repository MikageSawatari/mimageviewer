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

log("continuous-video: turn on continuous reading");
tap_key("6");
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

// CHECK 1: does the cross-axis key land on the video at all?
// A timeout here means the keys already skip videos and there is nothing to fix
// beyond writing that down.
log("continuous-video: check 1 - did we land on a non-still item?");
wait_until(|s| s.is_fullscreen && !s.current_is_still_image, 8000);
log("continuous-video: landed on a non-still item");

// CHECK 2: does the reverse key bring the still image back?
// A timeout here reproduces the report.
log("continuous-video: check 2 - can the reverse key return?");
tap_key("Left");
wait_until(|s| s.current_is_still_image, 10000);
log("continuous-video: returned to a still image");

// CHECK 3: is the continuous session still intact after the round trip?
log("continuous-video: check 3 - is continuous reading still on?");
wait_until(|s| s.continuous_reading, 5000);

log("continuous-video: round trip completed, the report did not reproduce here");
