// Hold the page key one way, then the other, and let the log say what each direction did.
//
// Reported 2026-08-14: holding Down stays at full quality and crawls, while holding Up drops to
// thumbnail quality and flies. Measured from the reporter's session at 9 pages/s against 19 over
// the same 58 pages, with the pass-through stand-in used 0 times forward and 53 times back.
//
// The point of running it here is that the reason label in the log cannot distinguish "the fast
// path declined a stand-in it did not need" from "the stand-in never appeared", so the question
// needs the `fs/navigation_target_phase` probe rather than another round trip to the reporter.
//
// Both directions cover the same pages at the same repeat rate, so anything that differs is a
// property of the direction and not of the folder or the pace.

log("hold-both: wait for the grid to settle");
wait_until(|s|
    s.target_registered
        && s.target_rendered
        && s.focused
        && !s.is_fullscreen
        && s.items_len > 0
        && s.pending_thumbs == 0
        && !s.modal_open
        && !s.popup_open,
    60000
);

log("hold-both: open the first image");
tap_key("Enter");
wait_until(|s|
    s.is_fullscreen
        && s.target_rendered
        && s.focused
        && s.fullscreen_raw_key_permit
        && !s.modal_open
        && !s.popup_open,
    30000
);

// Let the first page settle, so the burst measures paging rather than arrival.
sleep(2000);

// Well above what either direction can sustain, so the measured rate is the viewer's ceiling
// rather than the repeat rate. At the reporter's pace both directions were press-limited and
// looked equally fast, which hid the difference the reporter could see: 9 pages/s one way
// against 19 the other.
set_repeat(250, 40);

let before_forward = snapshot().fs_idx;
log("hold-both: forward (Down)");
hold_key("Down", 8000);
sleep(2500);
let after_forward = snapshot().fs_idx;
log("hold-both: forward travelled " + (after_forward - before_forward) + " pages");

log("hold-both: back (Up)");
hold_key("Up", 8000);
sleep(2500);
let after_back = snapshot().fs_idx;
log("hold-both: back travelled " + (after_forward - after_back) + " pages");

// A scenario that quietly does not run is worse than one that fails: an early return from the
// hold once let a script sail through every assertion and report success after 45ms of nothing.
if after_forward - before_forward < 5 {
    fail("the forward hold did not turn pages: " + before_forward + " -> " + after_forward);
}
if after_forward - after_back < 5 {
    fail("the back hold did not turn pages: " + after_forward + " -> " + after_back);
}

log("hold-both: done");
