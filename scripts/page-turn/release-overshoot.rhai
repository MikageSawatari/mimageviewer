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

    // Three readings, because two different things look alike from the outside.
    //
    // `at_release` is taken the instant the key is up. It routinely trails the app by a frame:
    // hold_key returns when it stops sending, not when the last repeat it already sent has been
    // processed, so a repeat emitted while the key was genuinely down can land a few ms later.
    // Reading only this and comparing it to the settled index reports that 4ms race as a bug -
    // which is what the first version of this scenario did, for a whole round of work.
    //
    // `drained` is taken after the in-flight repeats have had time to land. Movement after *that*
    // is the reported fault: presses that were queued while the app was behind, paid out once the
    // reader had already let go.
    let at_release = snapshot().fs_idx;
    sleep(250);
    let drained = snapshot().fs_idx;
    sleep(2500);
    let settled = snapshot().fs_idx;

    log("release-overshoot: round " + (round + 1) + " released at idx=" + at_release
        + ", drained at idx=" + drained + ", settled at idx=" + settled
        + " (in-flight " + (drained - at_release) + ", after-release " + (settled - drained) + ")");

    if at_release == start {
        fail("round " + (round + 1) + " did not turn any pages: still at idx=" + start);
    }
    if at_release >= snapshot().items_len - 2 {
        fail("round " + (round + 1) + " ran to the end of the folder (idx=" + at_release
            + " of " + snapshot().items_len + "): a hold that hits the boundary cannot overshoot, "
            + "so this round proves nothing. Shorten the hold or use a longer folder.");
    }
    if settled != drained {
        fail("the viewer kept moving well after the key was released: idx=" + drained
            + " once the in-flight repeats had landed, idx=" + settled + " a moment later");
    }
    // One frame of lag is the sampling race described above. More than that is a backlog of
    // presses being honoured after the reader stopped asking for them, which is the same fault
    // seen from the other end.
    if drained - at_release > 1 {
        fail("presses kept paying out after release: idx=" + at_release + " at release, idx="
            + drained + " once drained (" + (drained - at_release) + " pages the reader did not "
            + "ask to see)");
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
