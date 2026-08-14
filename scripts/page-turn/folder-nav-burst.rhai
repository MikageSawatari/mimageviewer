// Hold Ctrl+Down across a folder of PDFs, then check that the viewer recovers.
//
// This reproduces a livelock seen on 2026-08-14: after a long burst of folder navigation the
// viewer sat on "loading" forever, spinning at 165fps, and only a restart cleared it. The page had
// decoded successfully - what never happened was the texture upload, because the page-turn
// sequence defers uploads until it presents something, and it had nothing to present.
//
// The burst is the reproduction; the assertion is the recovery. A slow load is fine and expected
// after several hundred navigations. What must not happen is `upload_deferral_streak` still
// climbing long after the key was released, which is the deferral and the sequence waiting on each
// other rather than on any real work.
//
// Run against a folder from generate_pdf_fixture.py, or a real folder of many PDFs.

log("folder-nav-burst: wait for the grid to settle");
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

// Enter on a PDF opens it as a folder of pages; a second Enter opens the page. Against a folder
// of loose images the first Enter already lands in fullscreen, so this asks rather than assumes.
tap_key("Enter");
sleep(2000);
if !snapshot().is_fullscreen {
    log("folder-nav-burst: entered the document, opening the page");
    tap_key("Enter");
}

log("folder-nav-burst: wait for fullscreen");
wait_until(|s|
    s.is_fullscreen
        && s.target_rendered
        && s.focused
        && s.fullscreen_raw_key_permit
        && !s.modal_open
        && !s.popup_open,
    30000
);

// Continuous reading, which is where the failure was reported. It matters structurally, not just
// as a preference: in paged mode the page-turn burst takes the ordinary path, while continuous
// reading is a blocker for it, and only the blocked path reached the state that livelocked.
// A post filter puts the final-effect pipeline in the path, which is what the pass-through
// rendition exists to cover for. Without it the page materializes directly and the sequence never
// enters the phase that defers uploads - a default profile cannot reach the reported state.
log("folder-nav-burst: enable a post filter so the final-effect pipeline runs");
run_action("FsPostFilterUpscaleAnime");
sleep(500);

log("folder-nav-burst: switch to continuous reading");
run_action("FsReadingFlowCycle");
wait_until(|s| s.continuous_reading, 10000);

// Let the first document settle, so the burst measures navigation rather than arrival.
sleep(2000);

// Faster than a comfortable reading pace and faster than a document can load, which is the point:
// the reported failure needed the reader to outrun the pipeline for hundreds of documents.
set_repeat(250, 30);

let before = snapshot().items_generation;

log("folder-nav-burst: hold Ctrl+Down for 20 seconds");
hold_key("Down", "ctrl", 20000);

// A scenario that quietly does not run is worse than one that fails: an early return from the
// hold once let this script sail through every assertion below and report success after 45ms of
// doing nothing. The list generation changes once per document, so this says the burst really did
// travel, and roughly how far.
let travelled = snapshot().items_generation - before;
log("folder-nav-burst: documents visited during the burst: " + travelled);
if travelled < 10 {
    fail("the burst did not navigate: only " + travelled
        + " documents in 20 seconds. The hold is not reaching the viewer.");
}

// Everything below happens with no key held. Any deferral that persists here is waiting on
// something that is not coming.
log("folder-nav-burst: released - watching for recovery");

// Generous: several hundred documents may be queued behind this, and a real PDF page can take a
// second on its own. The livelock does not resolve in any amount of time, so a long wait costs
// nothing when the viewer is healthy.
wait_until(|s| s.upload_deferral_streak == 0, 30000);

log("folder-nav-burst: uploads resumed");

// The viewer having stopped deferring is not the same as it having drawn the page, so give it a
// moment and confirm it is not stuck a second time on the page it settled on.
sleep(3000);
let after = snapshot();
if after.upload_deferral_streak > 0 {
    fail("uploads deferred again after settling: streak=" + after.upload_deferral_streak
        + " passthrough_unavailable=" + after.passthrough_unavailable);
}
if after.passthrough_unavailable != "" {
    fail("settled page still has nothing to show: " + after.passthrough_unavailable);
}

log("folder-nav-burst: done");
