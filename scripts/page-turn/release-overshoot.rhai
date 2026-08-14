// Let go of a held page key and see whether the viewer stays where it was left.
//
// Reported 2026-08-14: releasing the key advances one more page a short moment later, and that
// page arrives at full quality without passing through thumbnail quality first. The changelog
// promises the opposite - "the page you stop on returns to full quality" - so the page you stop
// on should be the page you were looking at.
//
// Measured behaviourally rather than from the log, because a rendering that completes after
// release is not the same thing as a move that happens after release, and the two are hard to
// tell apart in a trace. Here the viewer's own index is read at the moment of release and again
// once everything has settled: if they differ, it moved on its own.
//
// The hold stops well short of the end of the folder, so a boundary cannot mask the overshoot.

log("release-overshoot: wait for the grid");
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
sleep(2000);

set_repeat(250, 40);

// Three attempts: the reporter says it happens sometimes, not always.
for round in 0..3 {
    let start = snapshot().fs_idx;
    log("release-overshoot: round " + (round + 1) + " from idx=" + start);
    // Short enough not to reach the end of the folder. A hold that runs into the last page has
    // nowhere left to overshoot to, so it reports "no drift" whatever the viewer does - the first
    // version of this scenario passed three times that way, which is worse than failing.
    hold_key("Down", 1000);

    // Read the index as soon as the key is up, before anything settles.
    let at_release = snapshot().fs_idx;
    sleep(2500);
    let settled = snapshot().fs_idx;

    log("release-overshoot: round " + (round + 1) + " released at idx=" + at_release
        + ", settled at idx=" + settled + " (drift " + (settled - at_release) + ")");

    if at_release == start {
        fail("round " + (round + 1) + " did not turn any pages: still at idx=" + start);
    }
    if at_release >= snapshot().items_len - 2 {
        fail("round " + (round + 1) + " ran to the end of the folder (idx=" + at_release
            + " of " + snapshot().items_len + "): a hold that hits the boundary cannot overshoot, "
            + "so this round proves nothing. Shorten the hold or use a longer folder.");
    }
    if settled != at_release {
        fail("the viewer moved after the key was released: idx=" + at_release
            + " at release, idx=" + settled + " once settled");
    }

    // Walk back to the start so the next round has the whole folder ahead of it. Single taps,
    // because a hold here would be another burst and would muddy the next round's reading.
    while snapshot().fs_idx > 0 {
        tap_key("Up");
        sleep(120);
    }
    sleep(1500);
}

log("release-overshoot: done");
