// Window switching uses the test-script activation request and viewport Focus.
// This scenario does not exercise OS click routing between windows.

// On a timeout, log every window's snapshot before failing, so the failed
// condition can be read from the evidence without rerunning.
fn wait_dump(ready, timeout_ms) {
    try {
        wait_until(ready, timeout_ms);
    } catch (error) {
        let s = snapshot();
        log("multi-window-stills: wait failed; is_fullscreen=" + s.is_fullscreen
            + " items_len=" + s.items_len + " windows=" + s.windows.to_debug());
        throw error;
    }
}

fn detached(s) {
    let windows = [];
    for window in s.windows {
        if window.role == "detached" && window.host_ready { windows.push(window); }
    }
    windows
}

fn by_id(s, id) {
    for window in s.windows {
        if window.role == "detached" && window.window_id == id { return window; }
    }
    ()
}

fn painted(window, expected_pages) {
    if !window.host_ready || window.page_index < 0 || window.paints.len != expected_pages {
        return false;
    }
    let current = false;
    let seen = [];
    for paint in window.paints {
        if paint.context_serial != window.context_serial
            || paint.items_generation != window.items_generation { return false; }
        for previous in seen { if previous == paint.page_index { return false; } }
        seen.push(paint.page_index);
        if paint.item_identity == window.item_identity
            && paint.page_index == window.page_index
            && paint.source == "full_or_processed" { current = true; }
    }
    current
}

fn final_current_paint(window) {
    for paint in window.paints {
        if paint.item_identity == window.item_identity
            && paint.page_index == window.page_index
            && paint.final_composite_complete { return true; }
    }
    false
}

fn contains_page(window, page_index) {
    for paint in window.paints {
        if paint.page_index == page_index { return true; }
    }
    false
}

fn same_numbers(a, b, tolerance) {
    if a.len != b.len { return false; }
    for i in 0..a.len {
        if (a[i] - b[i]).abs() > tolerance { return false; }
    }
    true
}

fn visible_clip(paint) {
    let min_x = paint.vertices[0][0];
    let min_y = paint.vertices[0][1];
    let max_x = min_x;
    let max_y = min_y;
    for vertex in paint.vertices {
        if vertex[0] < min_x { min_x = vertex[0]; }
        if vertex[1] < min_y { min_y = vertex[1]; }
        if vertex[0] > max_x { max_x = vertex[0]; }
        if vertex[1] > max_y { max_y = vertex[1]; }
    }
    if paint.clip[0] > min_x { min_x = paint.clip[0]; }
    if paint.clip[1] > min_y { min_y = paint.clip[1]; }
    if paint.clip[2] < max_x { max_x = paint.clip[2]; }
    if paint.clip[3] < max_y { max_y = paint.clip[3]; }
    [min_x, min_y, max_x, max_y]
}

fn same_paints(a, b) {
    if a.len != b.len { return false; }
    for i in 0..a.len {
        let x = a[i];
        let y = b[i];
        if x.pixels_per_point != y.pixels_per_point { return false; }
        let point_tolerance = 1.0 / x.pixels_per_point;
        if x.window_id != y.window_id
            || x.context_serial != y.context_serial
            || x.items_generation != y.items_generation
            || x.item_identity != y.item_identity
            || x.page_index != y.page_index
            || x.source_texture != y.source_texture
            || x.source != y.source
            || x.final_composite_complete != y.final_composite_complete
            || x.vertices.len != y.vertices.len
            || !same_numbers(visible_clip(x), visible_clip(y), point_tolerance)
            || !same_numbers(x.viewport_size, y.viewport_size, 0.01) {
            return false;
        }
        for vertex in 0..x.vertices.len {
            if (x.vertices[vertex][0] - y.vertices[vertex][0]).abs() > point_tolerance
                || (x.vertices[vertex][1] - y.vertices[vertex][1]).abs() > point_tolerance
                || (x.vertices[vertex][2] - y.vertices[vertex][2]).abs() > 0.0001
                || (x.vertices[vertex][3] - y.vertices[vertex][3]).abs() > 0.0001 {
                return false;
            }
        }
    }
    true
}

fn selected(window) {
    let target = select_window(window.window_id, window.context_serial);
    if target.target_mode != "targeted" || !target.current { fail("stale detached target"); }
}

log("multi-window-stills: waiting for root grid");
wait_dump(|s| !s.is_fullscreen && s.items_len == 2 && s.pending_thumbs == 0, 30000);
select_root();
run_action("GridMoveFirst");
run_action("GridOpenSelected");
wait_dump(|s| {
    let windows = detached(s);
    windows.len == 1 && painted(windows[0], 1) && windows[0].page_ready
        && windows[0].page_index == 0
        && windows[0].sidecar_imported && final_current_paint(windows[0])
}, 30000);
let folder = detached(snapshot())[0];
log("multi-window-stills: folder painted and sidecar imported");

select_root();
run_action("GridMoveLast");
run_action("GridOpenSelected");
wait_dump(|s| {
    if s.is_fullscreen || s.items_len != 1 { return false; }
    for window in s.windows {
        if window.role == "root" && window.sidecar_imported { return true; }
    }
    false
}, 30000);
log("multi-window-stills: ZIP folder sidecar imported");
select_root();
run_action("GridMoveFirst");
run_action("GridOpenSelected");
wait_dump(|s| {
    let windows = detached(s);
    if windows.len != 2 || by_id(s, folder.window_id) == () { return false; }
    let zip_loaded = false;
    for window in windows {
        if window.window_id != folder.window_id && window.sidecar_loaded
            && final_current_paint(window) { zip_loaded = true; }
    }
    zip_loaded && windows[0].context_serial != windows[1].context_serial
        && painted(windows[0], 1) && painted(windows[1], 1)
}, 30000);
let windows = detached(snapshot());
let first = by_id(snapshot(), folder.window_id);
let second = if windows[0].window_id == first.window_id { windows[1] } else { windows[0] };
if first.item_identity == second.item_identity || second.page_index != 0 {
    fail("folder and ZIP books did not open independently at page zero");
}
if !second.sidecar_loaded { fail("detached ZIP context did not load its sidecar"); }
log("multi-window-stills: ZIP painted");

selected(first);
run_action("FsJumpFirst");
wait_dump(|s| {
    let w = by_id(s, first.window_id);
    w != () && painted(w, 1) && w.page_ready && final_current_paint(w)
        && w.paints[0].revision > first.paints[0].revision
}, 30000);
let settling = by_id(snapshot(), first.window_id);
run_action("FsJumpFirst");
wait_dump(|s| {
    let w = by_id(s, first.window_id);
    w != () && painted(w, 1) && final_current_paint(w)
        && w.paints[0].revision > settling.paints[0].revision
        && same_paints(w.paints, settling.paints)
}, 30000);
let active = by_id(snapshot(), first.window_id);
if active.paints.len != 1 || active.paints[0].page_index != 0
    || !final_current_paint(active) || !same_paints(settling.paints, active.paints) {
    fail("folder endpoint did not paint one singleton page");
}
// LtrCover puts page zero in the right slot. Check the mesh, not its placement label.
let bounds = visible_clip(active.paints[0]);
let midpoint = active.paints[0].viewport_size[0] * 0.5;
let pixel = 1.0 / active.paints[0].pixels_per_point;
if bounds[0] >= bounds[2] || bounds[0] < midpoint - pixel
    || (bounds[0] + bounds[2]) * 0.5 <= midpoint + pixel {
    fail("I2: LtrCover endpoint singleton was not painted in the right slot: " + active.paints.to_debug());
}

selected(second);
run_action("FsJumpFirst");
wait_dump(|s| {
    let a = by_id(s, first.window_id);
    let b = by_id(s, second.window_id);
    a != () && b != () && painted(a, 1) && painted(b, 1)
        && a.paints[0].revision > active.paints[0].revision
        && b.paints[0].revision > second.paints[0].revision
}, 30000);
let parked = by_id(snapshot(), first.window_id);
if !same_paints(active.paints, parked.paints) {
    fail("I2: folder singleton paint changed when parked: active=" + active.paints.to_debug() + " parked=" + parked.paints.to_debug());
}

selected(first);
run_action("FsJumpFirst");
wait_dump(|s| {
    let w = by_id(s, first.window_id);
    w != () && painted(w, 1) && w.paints[0].revision > parked.paints[0].revision
}, 30000);
let returned = by_id(snapshot(), first.window_id);
if !same_paints(active.paints, returned.paints) { fail("I5: singleton paint changed on return"); }
let sibling = by_id(snapshot(), second.window_id);
run_action("FsPageNext");
wait_dump(|s| {
    let a = by_id(s, first.window_id);
    let b = by_id(s, second.window_id);
    a != () && b != () && a.page_index == 1 && painted(a, 2)
        && contains_page(a, 1) && contains_page(a, 2) && painted(b, 1)
        && a.paints[0].revision > returned.paints[0].revision
        && b.page_index == sibling.page_index
}, 30000);
let sibling_after = by_id(snapshot(), second.window_id);
if !same_paints(sibling.paints, sibling_after.paints) {
    fail("I3: sibling paint changed after folder page turn");
}

run_action("FsClose");
wait_dump(|s| by_id(s, first.window_id) == (), 30000);
selected(second);
run_action("FsClose");
wait_dump(|s| detached(s).len == 0 && !s.is_fullscreen, 30000);
log("multi-window-stills: done");
