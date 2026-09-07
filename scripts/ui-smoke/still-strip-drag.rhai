// S2 real-egui still-image strip/track smoke scenario.

const STRIP = "fullscreen_still_seek_strip_row";
const TRACK = "fullscreen_seek_track";
const WAIT_MS = 30000;

fn same_identity(actual, expected) {
    actual.role == expected.role
        && actual.window_id == expected.window_id
        && actual.context_serial == expected.context_serial
        && actual.viewport == expected.viewport
        && actual.host_incarnation == expected.host_incarnation
        && actual.hwnd == expected.hwnd
        && actual.backend_token == expected.backend_token
}

fn detached_windows(s) {
    let result = [];
    for window in s.windows {
        if window.role == "detached" && window.host_ready {
            result.push(window);
        }
    }
    result
}

fn root_ready(s) {
    for window in s.windows {
        if window.role == "root" && window.host_ready {
            return true;
        }
    }
    false
}

fn root_window(s) {
    for window in s.windows {
        if window.role == "root" && window.host_ready {
            return window;
        }
    }
    fail("root window disappeared");
}

fn root_unchanged(s, expected) {
    for window in s.windows {
        if window.role == "root" && same_identity(window, expected) {
            return window.items_generation == expected.items_generation;
        }
    }
    false
}

fn assert_root_unchanged(expected, label) {
    if !root_unchanged(snapshot(), expected) {
        fail(label + ": root identity/items generation changed");
    }
}

fn current_window(s, expected) {
    for window in s.windows {
        if same_identity(window, expected) {
            return window;
        }
    }
    fail("selected detached identity disappeared");
}

fn media_window(s, kind) {
    for window in s.windows {
        if window.role == "detached" && window.host_ready && window.media_kind == kind {
            return window;
        }
    }
    fail("detached media window disappeared: " + kind);
}

fn media_pair_ready(s, expected_sibling) {
    let windows = detached_windows(s);
    if windows.len != 2 {
        return false;
    }
    let image_count = 0;
    let pdf_count = 0;
    let image_serial = -1;
    let pdf_serial = -1;
    for window in windows {
        if window.media_kind == "image"
            && window.page_index == 0
            && window.full_texture_painted
            && window.paint_matches_current_page
        {
            image_count += 1;
            image_serial = window.context_serial;
        }
        if window.media_kind == "pdf" {
            pdf_count += 1;
            pdf_serial = window.context_serial;
        }
    }
    image_count == 1
        && pdf_count == 1
        && image_serial != pdf_serial
        && sibling_unchanged(s, expected_sibling)
}

fn target_page_ready(s, expected, page) {
    for window in s.windows {
        if same_identity(window, expected) {
            return window.page_index == page
                && window.items_generation == expected.items_generation
                && window.full_texture_painted
                && window.paint_matches_current_page;
        }
    }
    false
}

fn sibling_unchanged(s, expected) {
    for window in s.windows {
        if same_identity(window, expected) {
            return window.page_index == expected.page_index
                && window.items_generation == expected.items_generation;
        }
    }
    false
}

fn assert_sibling_unchanged(expected, label) {
    if !sibling_unchanged(snapshot(), expected) {
        fail(label + ": sibling identity/page/items generation changed");
    }
}

fn array_has(values, expected) {
    for value in values {
        if value == expected {
            return true;
        }
    }
    false
}

fn assert_targeted(selection, label) {
    if selection.target_mode != "targeted" || !selection.current {
        fail(label + ": exact targeted selection was not current");
    }
}

fn assert_base_mode(region, expected_rtl, label) {
    if region.spread_mode != "Single"
        || region.reading_flow != "Paged"
        || region.strip_rtl != expected_rtl
        || !region.strip_visible
        || !region.strip_locked
        || !region.bar_locked
    {
        fail(label
            + ": unexpected callback mode spread=" + region.spread_mode
            + " flow=" + region.reading_flow
            + " strip_rtl=" + region.strip_rtl
            + " strip_visible=" + region.strip_visible
            + " strip_locked=" + region.strip_locked
            + " bar_locked=" + region.bar_locked);
    }
}

fn assert_strip_paint(region, expected_center, expected_page, label) {
    if region.page_index != expected_page
        || region.strip_center != expected_center
        || !array_has(region.strip_cells, expected_center)
    {
        fail(label
            + ": handler/paint mismatch page=" + region.page_index
            + " center=" + region.strip_center
            + " expected_center=" + expected_center);
    }
}

fn assert_strip_step(step, effect, page, label) {
    if step.effect != effect
        || step.page_before != page
        || step.page_after != page
        || step.after_revision <= 0
    {
        fail(label
            + ": strip step changed page or reported wrong effect effect=" + step.effect
            + " before=" + step.page_before
            + " after=" + step.page_after);
    }
}

// Key-action completion proves the handler pass, whose geometry was captured before that handler.
// Step over that publication and return a later callback observation of the resulting mode.
fn settled_region_after_action(name, before_revision) {
    let action_show = egui_pointer_region(name, before_revision, WAIT_MS);
    egui_pointer_region(name, action_show.revision, WAIT_MS)
}

log("still-strip-drag: wait for image-book folder and PDF sibling");
wait_until(|s|
    !s.is_fullscreen
        && s.items_len == 2
        && s.pending_thumbs == 0
        && root_ready(s),
    WAIT_MS
);
let root = root_window(snapshot());

// Open the PDF first so it remains an independent AtRest context while the image-only folder is
// classified and opened as the active DetachedPhysical target.
assert_targeted(select_root(), "select root for PDF sibling");
run_action("GridMoveLast");
run_action("GridOpenSelected");
wait_until(|s| {
    let windows = detached_windows(s);
    windows.len == 1
        && windows[0].media_kind == "pdf"
        && windows[0].page_index == 0
        && windows[0].full_texture_painted
        && windows[0].paint_matches_current_page
}, WAIT_MS);
let sibling = media_window(snapshot(), "pdf");
assert_root_unchanged(root, "PDF sibling open");

assert_targeted(select_root(), "select root for image-book target");
run_action("GridMoveFirst");
run_action("GridOpenSelected");
wait_until(|s| media_pair_ready(s, sibling), WAIT_MS);
let opened = snapshot();
let target = media_window(opened, "image");
let current_sibling = media_window(opened, "pdf");
if !same_identity(current_sibling, sibling) {
    fail("opening the image book replaced the PDF sibling identity");
}
assert_root_unchanged(root, "image book open");
assert_targeted(select_window(target.window_id, target.context_serial), "select strip target");

let strip = egui_pointer_region(STRIP, 0, WAIT_MS);
if strip.spread_mode != "Single" {
    let before_revision = strip.revision;
    run_action("FsSpreadSingle");
    strip = settled_region_after_action(STRIP, before_revision);
}
for mode_step in 0..3 {
    if strip.reading_flow == "Paged" {
        break;
    }
    let before_revision = strip.revision;
    run_action("FsReadingFlowCycle");
    strip = settled_region_after_action(STRIP, before_revision);
}
if strip.reading_flow != "Paged" {
    fail("FsReadingFlowCycle did not reach Paged");
}
if strip.strip_rtl {
    let before_revision = strip.revision;
    run_action("FsReadingDirectionToggle");
    strip = settled_region_after_action(STRIP, before_revision);
}
assert_base_mode(strip, false, "initial LTR setup");
for page_step in 0..20 {
    run_action("FsPageNext");
    let expected_page = page_step + 1;
    wait_until(|s|
        target_page_ready(s, target, expected_page)
            && sibling_unchanged(s, sibling),
        WAIT_MS
    );
}
let target = current_window(snapshot(), target);
if target.page_index != 20 {
    fail("per-page setup did not reach image page 20");
}

strip = egui_pointer_region(STRIP, 0, WAIT_MS);
assert_base_mode(strip, false, "LTR setup");
let page = strip.page_index;
let initial_center = strip.strip_center;

let down = egui_pointer_down(STRIP, strip.geometry_token, 0.50, 0.50, WAIT_MS);
assert_strip_step(down, "pressed", page, "LTR down");
let moved = egui_pointer_move(0.62, 0.50, WAIT_MS);
assert_strip_step(moved, "strip_center", page, "LTR threshold move");
if moved.effect_value >= initial_center {
    fail("LTR rightward threshold move did not decrease strip center");
}
let painted = egui_pointer_region(STRIP, moved.after_revision, WAIT_MS);
assert_base_mode(painted, false, "LTR threshold paint");
assert_strip_paint(painted, moved.effect_value, page, "LTR threshold paint");
assert_sibling_unchanged(sibling, "LTR threshold move");

let continued = egui_pointer_move(0.75, 0.50, WAIT_MS);
assert_strip_step(continued, "strip_center", page, "LTR continued move");
if continued.effect_value >= moved.effect_value {
    fail("LTR continued move did not advance beyond threshold move");
}
painted = egui_pointer_region(STRIP, continued.after_revision, WAIT_MS);
assert_strip_paint(painted, continued.effect_value, page, "LTR continued paint");

let released = egui_pointer_up(0.90, 0.50, WAIT_MS);
assert_strip_step(released, "released_strip_center", page, "LTR final-coordinate release");
if released.effect_value >= continued.effect_value {
    fail("LTR release did not apply its distinct final coordinate");
}
painted = egui_pointer_region(STRIP, released.after_revision, WAIT_MS);
assert_strip_paint(painted, released.effect_value, page, "LTR release paint");
assert_sibling_unchanged(sibling, "LTR release");

// Use the existing production action to invert the effective strip direction, then repeat the
// same physical rightward gesture. RTL must move the logical center in the opposite direction.
run_action("FsReadingDirectionToggle");
strip = settled_region_after_action(STRIP, painted.revision);
assert_base_mode(strip, true, "RTL setup");
let rtl_initial_center = strip.strip_center;
down = egui_pointer_down(STRIP, strip.geometry_token, 0.50, 0.50, WAIT_MS);
assert_strip_step(down, "pressed", page, "RTL down");
moved = egui_pointer_move(0.62, 0.50, WAIT_MS);
assert_strip_step(moved, "strip_center", page, "RTL threshold move");
if moved.effect_value <= rtl_initial_center {
    fail("RTL rightward threshold move did not increase strip center");
}
painted = egui_pointer_region(STRIP, moved.after_revision, WAIT_MS);
assert_base_mode(painted, true, "RTL threshold paint");
assert_strip_paint(painted, moved.effect_value, page, "RTL threshold paint");

continued = egui_pointer_move(0.75, 0.50, WAIT_MS);
assert_strip_step(continued, "strip_center", page, "RTL continued move");
if continued.effect_value <= moved.effect_value {
    fail("RTL continued move did not advance opposite to LTR");
}
painted = egui_pointer_region(STRIP, continued.after_revision, WAIT_MS);
assert_strip_paint(painted, continued.effect_value, page, "RTL continued paint");

released = egui_pointer_up(0.90, 0.50, WAIT_MS);
assert_strip_step(released, "released_strip_center", page, "RTL final-coordinate release");
if released.effect_value <= continued.effect_value {
    fail("RTL release did not apply its distinct final coordinate");
}
painted = egui_pointer_region(STRIP, released.after_revision, WAIT_MS);
assert_strip_paint(painted, released.effect_value, page, "RTL release paint");
assert_sibling_unchanged(sibling, "RTL release");

// The strip only moves its column center. The track is the separate page-seek path.
let track = egui_pointer_region(TRACK, 0, WAIT_MS);
assert_base_mode(track, true, "track setup after RTL strip");
down = egui_pointer_down(TRACK, track.geometry_token, 0.50, 0.50, WAIT_MS);
if down.effect != "pressed" || down.after_revision <= 0 {
    fail("track Down did not reach the real track Response");
}
moved = egui_pointer_move(0.70, 0.50, WAIT_MS);
if moved.effect != "track_target"
    || moved.effect_value != moved.page_after
    || moved.after_revision <= down.after_revision
{
    fail("track move did not report its actual navigation landing");
}
let track_after_move = egui_pointer_region(TRACK, moved.after_revision, WAIT_MS);
assert_base_mode(track_after_move, true, "track repaint after move landing");
if track_after_move.page_index != moved.page_after {
    fail("track repaint did not reach the move landing before release");
}
released = egui_pointer_up(0.90, 0.50, WAIT_MS);
if released.effect != "released_track_target"
    || released.effect_value != released.page_after
    || released.page_before != moved.page_after
    || released.page_after == moved.page_after
    || released.page_after == page
    || released.after_revision <= moved.after_revision
{
    fail("track final-coordinate release did not seek to its reported page");
}
let track_after_release = egui_pointer_region(TRACK, released.after_revision, WAIT_MS);
assert_base_mode(track_after_release, true, "track repaint after release landing");
if track_after_release.page_index != released.page_after {
    fail("track repaint did not reach the release landing");
}
wait_until(|s| {
    let current = current_window(s, target);
    current.page_index == released.page_after
        && current.full_texture_painted
        && current.paint_matches_current_page
        && sibling_unchanged(s, sibling)
}, WAIT_MS);
assert_root_unchanged(root, "final strip/track state");

log("still-strip-drag: done");
