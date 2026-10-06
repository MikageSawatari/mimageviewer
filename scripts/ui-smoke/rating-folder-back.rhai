// Actual Places menu -> Rating -> three stars, Enter, fourth page, independent returns.
// No direct enter_rating_view / history actions. Failures are aggregated after all cases.

fn click(label) {
    click_widget(label);
    // Widget acknowledgement precedes its Down/Up and the menu's resulting paint.
    // Log commands wake ROOT: an idle menu must not rely on unrelated repaint activity.
    for step in 0..3 {
        let frame = snapshot().snapshot_frame;
        log("rating-folder-back: widget-frame label=" + label + " step=" + step);
        wait_until(|s| s.snapshot_frame > frame, 5000);
    }
}

fn move_to(index) {
    let frame = snapshot().snapshot_frame;
    tap_key("Home");
    wait_until(|s| s.snapshot_frame > frame && s.selected_index == 0, 5000);
    for next in 1..(index + 1) {
        frame = snapshot().snapshot_frame;
        tap_key("Right");
        wait_until(|s| s.snapshot_frame > frame && s.selected_index == next, 5000);
    }
    wait_until(|s| s.grid.generation == s.items_generation
        && s.grid.selected_index == s.selected_index && s.grid.visible, 5000);
}

fn rating_root() {
    tap_key("Escape");
    click("places-menu");
    click("rating-menu");
    click("rating-three");
    wait_until(|s| !s.startup_open_pending && !s.is_fullscreen
        && s.grid_surface == "Rating { stars: 3 }" && s.items_len == 15, 10000);
}

fn record(label) {
    let s = snapshot();
    let name = "<none>";
    if s.selected_index >= 0 && s.selected_index < s.item_names.len {
        name = s.item_names[s.selected_index];
    }
    log("rating-folder-back: " + label + " surface=" + s.grid_surface
        + " path=" + s.current_folder_path + " generation=" + s.items_generation
        + " selected_index=" + s.selected_index + " selected_name=" + name
        + " grid=" + s.grid.to_debug());
    // ROOT capture writes screenshots and their identity manifest to run evidence.
    capture(label);
    s
}

fn depart(kind, forward) {
    if kind == "toolbar" {
        if forward { click("history-forward"); } else { click("history-back"); }
    } else if kind == "alt" {
        if forward { tap_key("Right", "alt"); } else { tap_key("Left", "alt"); }
    } else if kind == "browser" {
        if forward { root_history_input("browser_forward"); }
        else { root_history_input("browser_back"); }
    } else if kind == "appcommand" {
        if forward { root_history_input("appcommand_forward"); }
        else { root_history_input("appcommand_back"); }
    } else if kind == "mouse" {
        if forward { root_history_input("mouse_x2"); }
        else { root_history_input("mouse_x1"); }
    } else {
        tap_key("Backspace");
    }
}

fn returned(label, opened_key, opened_name, opened_index, generation) {
    wait_until(|s| !s.is_fullscreen && s.items_generation > generation
        && s.grid_surface == "Rating { stars: 3 }" && s.items_len == 15, 10000);
    wait_until(|s| s.grid.generation == s.items_generation
        && s.grid.selected_index == s.selected_index, 5000);
    // Assert the same observation that was logged, rather than a later capture-time state.
    let s = record(label);
    let ok = s.selected_index == opened_index && s.grid.selected_key == opened_key
        && s.grid.selected_name == opened_name && s.grid.visible
        && s.grid.scroll_offset > 0.0;
    log("rating-folder-back: result=" + ok + " case=" + label
        + " expected_key=" + opened_key + " expected_name=" + opened_name
        + " expected_index=" + opened_index);
    ok
}

fn run_case(target, kind, roundtrip) {
    rating_root();
    let index = snapshot().item_names.index_of(target);
    if index < 0 { throw "fixture row is missing: " + target; }
    move_to(index);
    let opened = snapshot();
    if opened.grid.scroll_offset <= 0.0
        || opened.grid.content_height <= opened.grid.viewport_height {
        throw "fixture does not exercise a scrolled rating list";
    }
    let key = opened.grid.selected_key;
    let label = target + "-" + kind;
    if roundtrip { label += "-roundtrip"; }
    record(label + "-before-open");
    tap_key("Enter");
    wait_until(|s| !s.is_fullscreen && s.items_generation > opened.items_generation
        && s.items_len == 8 && s.grid_surface != "Rating { stars: 3 }", 10000);
    move_to(3);
    record(label + "-fourth-page");
    let generation = snapshot().items_generation;
    depart(kind, false);
    let ok = returned(label + "-back", key, target, index, generation);
    if roundtrip {
        generation = snapshot().items_generation;
        depart(kind, true);
        wait_until(|s| !s.is_fullscreen && s.items_generation > generation
            && s.items_len == 8 && s.grid_surface != "Rating { stars: 3 }", 10000);
        // Preserve the reported fourth-page setup on the second departure too.
        move_to(3);
        record(label + "-forward-fourth-page");
        generation = snapshot().items_generation;
        depart(kind, false);
        let second = returned(label + "-second-back", key, target, index, generation);
        ok = ok && second;
    }
    ok
}

wait_until(|s| !s.startup_open_pending && !s.is_fullscreen && s.items_len == 15, 30000);
select_root();
let failures = 0;
let cases = 0;
for target in ["12-folder", "23-book.zip"] {
    for kind in ["toolbar", "alt", "browser", "appcommand", "mouse", "backspace"] {
        let rounds = [false, true];
        if kind == "backspace" { rounds = [false]; }
        for roundtrip in rounds {
            cases += 1;
            try {
                if !run_case(target, kind, roundtrip) { failures += 1; }
            } catch (error) {
                failures += 1;
                let label = target + "-" + kind + "-error-" + cases;
                log("rating-folder-back: FAIL " + label + " roundtrip=" + roundtrip
                    + " error=" + error.to_string());
                try { record(label); } catch (capture_error) {
                    log("rating-folder-back: artifact error=" + capture_error.to_string());
                }
            }
        }
    }
}
log("rating-folder-back: completed=" + cases + " failures=" + failures);
if failures > 0 { fail("rating-folder-back: " + failures + " of " + cases + " cases failed"); }
log("rating-folder-back: PASS");
