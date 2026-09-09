// S2 real-egui still-image strip/track smoke scenario.

const STRIP = "fullscreen_still_seek_strip_row";
const TRACK = "fullscreen_seek_track";
const WAIT_MS = 30000;
const FIXTURE_IMAGE_COUNT = 40;
// The disposable fixture pins still_seek_strip_height=large. The production
// drag handler quantizes the immutable press delta by this 152-point cell width.
const STRIP_CELL_WIDTH = 152.0;

fn strip_drag_positions(region, label, cell_width) {
    let width = region.right - region.left;
    if width <= 0.0 {
        fail(label + " strip row has no usable width");
    }
    let down_x = 0.05;
    let one_cell_x = down_x + cell_width / width;
    let two_cells_x = down_x + 2.0 * cell_width / width;
    let three_cells_x = down_x + 3.0 * cell_width / width;
    if one_cell_x <= down_x
            || two_cells_x <= one_cell_x
            || three_cells_x <= two_cells_x
            || three_cells_x > 0.95 {
        fail(label + " strip row is too narrow for three distinct fixture steps"
            + ": width=" + width
            + " final_x=" + three_cells_x);
    }
    log("still-strip-drag: " + label + " coordinates"
        + " width=" + width
        + " down=" + down_x
        + " one=" + one_cell_x
        + " two=" + two_cells_x
        + " three=" + three_cells_x);
    [down_x, one_cell_x, two_cells_x, three_cells_x]
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

// Mirror ReadingFlow::next: Paged -> Vertical -> Horizontal -> Paged.
fn next_reading_flow(flow) {
    if flow == "Paged" {
        return "Vertical";
    }
    if flow == "Vertical" {
        return "Horizontal";
    }
    if flow == "Horizontal" {
        return "Paged";
    }
    fail("unknown reading flow while computing the expected transition: " + flow);
}

// run_action acknowledges exact-owner consumption, before the handler effect is
// published. Advance only through fresh region revisions until that same owner
// exposes the requested mode, under one monotonic absolute timeout.
fn wait_for_region_mode(
    name,
    after_revision,
    expected_spread,
    expected_flow,
    expected_rtl,
    timeout_ms
) {
    if timeout_ms <= 0 {
        fail("region mode timeout must be positive");
    }
    let started = timestamp();
    let budget_ms = timeout_ms.to_float();
    let current_after = after_revision;
    let last_revision = after_revision;
    let last_spread = "<unobserved>";
    let last_flow = "<unobserved>";
    let last_rtl = false;
    while true {
        let elapsed_ms = started.elapsed * 1000.0;
        let remaining_ms = (budget_ms - elapsed_ms).to_int();
        if elapsed_ms >= budget_ms || remaining_ms <= 0 {
            fail("timed out waiting for exact-owner region mode"
                + ": name=" + name
                + " last_revision=" + last_revision
                + " last_spread=" + last_spread
                + " last_flow=" + last_flow
                + " last_rtl=" + last_rtl);
        }
        let observed = egui_pointer_region(name, current_after, remaining_ms);
        current_after = observed.revision;
        last_revision = observed.revision;
        last_spread = observed.spread_mode;
        last_flow = observed.reading_flow;
        last_rtl = observed.strip_rtl;
        if started.elapsed * 1000.0 >= budget_ms {
            fail("region mode arrived after its absolute deadline"
                + ": name=" + name
                + " last_revision=" + last_revision
                + " last_spread=" + last_spread
                + " last_flow=" + last_flow
                + " last_rtl=" + last_rtl);
        }
        if last_spread == expected_spread
                && last_flow == expected_flow
                && last_rtl == expected_rtl {
            return observed;
        }
    }
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
    let expected_flow = strip.reading_flow;
    let expected_rtl = strip.strip_rtl;
    run_action("FsSpreadSingle");
    strip = wait_for_region_mode(
        STRIP, before_revision, "Single", expected_flow, expected_rtl, WAIT_MS);
}
for mode_step in 0..3 {
    if strip.reading_flow == "Paged" {
        break;
    }
    let before_revision = strip.revision;
    let expected_flow = next_reading_flow(strip.reading_flow);
    run_action("FsReadingFlowCycle");
    strip = wait_for_region_mode(
        STRIP,
        before_revision,
        strip.spread_mode,
        expected_flow,
        strip.strip_rtl,
        WAIT_MS
    );
}
if strip.reading_flow != "Paged" {
    fail("FsReadingFlowCycle did not reach Paged");
}
if strip.strip_rtl {
    let before_revision = strip.revision;
    run_action("FsReadingDirectionToggle");
    strip = wait_for_region_mode(
        STRIP,
        before_revision,
        strip.spread_mode,
        strip.reading_flow,
        false,
        WAIT_MS
    );
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
if initial_center < 3 {
    fail("LTR fixture center has less than three pages of start-side margin");
}
let ltr_x = strip_drag_positions(strip, "LTR", STRIP_CELL_WIDTH);
log("still-strip-drag: LTR initial"
    + " revision=" + strip.revision
    + " geometry_token=" + strip.geometry_token
    + " page=" + page
    + " center=" + initial_center
    + " rect_left=" + strip.left
    + " rect_right=" + strip.right
    + " coordinate_left=" + strip.coordinate_left
    + " coordinate_right=" + strip.coordinate_right);

let down = egui_pointer_down(STRIP, strip.geometry_token, ltr_x[0], 0.50, WAIT_MS);
assert_strip_step(down, "pressed", page, "LTR down");
let moved = egui_pointer_move(ltr_x[1], 0.50, WAIT_MS);
assert_strip_step(moved, "strip_center", page, "LTR threshold move");
log("still-strip-drag: LTR threshold"
    + " effect_value=" + moved.effect_value
    + " after_revision=" + moved.after_revision
    + " page_before=" + moved.page_before
    + " page_after=" + moved.page_after);
if moved.effect_value != initial_center - 1 {
    fail("LTR one-cell move did not decrease strip center by one");
}
let painted = egui_pointer_region(STRIP, moved.after_revision, WAIT_MS);
assert_base_mode(painted, false, "LTR threshold paint");
assert_strip_paint(painted, moved.effect_value, page, "LTR threshold paint");
assert_sibling_unchanged(sibling, "LTR threshold move");

let continued = egui_pointer_move(ltr_x[2], 0.50, WAIT_MS);
assert_strip_step(continued, "strip_center", page, "LTR continued move");
log("still-strip-drag: LTR continued"
    + " effect_value=" + continued.effect_value
    + " after_revision=" + continued.after_revision
    + " page_before=" + continued.page_before
    + " page_after=" + continued.page_after
    + " painted_center=" + painted.strip_center
    + " painted_rect_left=" + painted.left
    + " painted_rect_right=" + painted.right
    + " painted_coordinate_left=" + painted.coordinate_left
    + " painted_coordinate_right=" + painted.coordinate_right);
if continued.effect_value != initial_center - 2 {
    fail("LTR two-cell move did not decrease strip center by two");
}
painted = egui_pointer_region(STRIP, continued.after_revision, WAIT_MS);
assert_strip_paint(painted, continued.effect_value, page, "LTR continued paint");

let released = egui_pointer_up(ltr_x[3], 0.50, WAIT_MS);
assert_strip_step(released, "released_strip_center", page, "LTR final-coordinate release");
if released.effect_value != initial_center - 3 {
    fail("LTR three-cell release did not decrease strip center by three");
}
painted = egui_pointer_region(STRIP, released.after_revision, WAIT_MS);
assert_strip_paint(painted, released.effect_value, page, "LTR release paint");
assert_sibling_unchanged(sibling, "LTR release");

// Use the existing production action to invert the effective strip direction, then repeat the
// same physical rightward gesture. RTL must move the logical center in the opposite direction.
run_action("FsReadingDirectionToggle");
strip = wait_for_region_mode(
    STRIP,
    painted.revision,
    painted.spread_mode,
    painted.reading_flow,
    true,
    WAIT_MS
);
assert_base_mode(strip, true, "RTL setup");
let rtl_initial_center = strip.strip_center;
if rtl_initial_center > FIXTURE_IMAGE_COUNT - 4 {
    fail("RTL fixture center has less than three pages of end-side margin");
}
let rtl_x = strip_drag_positions(strip, "RTL", STRIP_CELL_WIDTH);
down = egui_pointer_down(STRIP, strip.geometry_token, rtl_x[0], 0.50, WAIT_MS);
assert_strip_step(down, "pressed", page, "RTL down");
moved = egui_pointer_move(rtl_x[1], 0.50, WAIT_MS);
assert_strip_step(moved, "strip_center", page, "RTL threshold move");
if moved.effect_value != rtl_initial_center + 1 {
    fail("RTL one-cell move did not increase strip center by one");
}
painted = egui_pointer_region(STRIP, moved.after_revision, WAIT_MS);
assert_base_mode(painted, true, "RTL threshold paint");
assert_strip_paint(painted, moved.effect_value, page, "RTL threshold paint");

continued = egui_pointer_move(rtl_x[2], 0.50, WAIT_MS);
assert_strip_step(continued, "strip_center", page, "RTL continued move");
if continued.effect_value != rtl_initial_center + 2 {
    fail("RTL two-cell move did not increase strip center by two");
}
painted = egui_pointer_region(STRIP, continued.after_revision, WAIT_MS);
assert_strip_paint(painted, continued.effect_value, page, "RTL continued paint");

released = egui_pointer_up(rtl_x[3], 0.50, WAIT_MS);
assert_strip_step(released, "released_strip_center", page, "RTL final-coordinate release");
if released.effect_value != rtl_initial_center + 3 {
    fail("RTL three-cell release did not increase strip center by three");
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
