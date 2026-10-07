// Actual Places menu -> Rating -> three stars, double-click, fourth page, independent returns.
// No direct enter_rating_view / history actions. Failures are aggregated after all cases.

fn input_ready(s) {
    s.target_registered && s.focused && s.target_viewport == "ROOT"
}

fn grid_adopted(s, surface, path, names, generation) {
    !s.startup_open_pending && !s.is_fullscreen
        && !s.modal_open && !s.popup_open && !s.text_input_or_pending_focus && !s.ime_active
        && s.grid_surface == surface && s.current_folder_path == path
        && s.item_names == names && s.items_generation > generation
}

// Never request foreground focus. Physical input is optional in unattended runs.
fn physical_input_available() {
    try { wait_until(|s| input_ready(s), 1000); return true; }
    catch (error) { return false; }
}

fn pointer(label, kind) {
    if kind == "double" { double_click_widget(label); }
    else if kind == "hover" { hover_widget(label); }
    else { click_widget(label); }
    // Ack precedes the pointer edges. Wake ROOT and wait for later published frames.
    let frames = 3;
    if kind == "double" { frames = 5; }
    for step in 0..frames {
        let frame = snapshot().snapshot_frame;
        log("rating-folder-back: widget-frame label=" + label + " step=" + step);
        wait_until(|s| s.snapshot_frame > frame, 5000);
    }
}

fn click(label) { pointer(label, "single"); }

fn row_widget(index) {
    let s = snapshot();
    "grid-row:" + s.items_generation + ":" + index + ":" + s.item_names[index]
}

fn arrive(surface, path, names, generation, label) {
    try {
        wait_until(|s| grid_adopted(s, surface, path, names, generation), 30000);
        wait_until(|s| s.grid.generation == s.items_generation
            && s.grid.selected_index == s.selected_index, 30000);
    } catch (error) {
        let s = snapshot();
        log("rating-folder-back: adoption timeout " + label + " surface=" + s.grid_surface
            + " path=" + s.current_folder_path + " names=" + s.item_names.to_debug()
            + " generation=" + s.items_generation + " target=" + s.target_registered
            + " focused=" + s.focused);
        throw error;
    }
    log("rating-folder-back: adopted " + label + " surface=" + surface + " path=" + path
        + " generation=" + snapshot().items_generation);
}

fn rating_root(path, names) {
    click("places-menu");
    click("rating-menu");
    let generation = snapshot().items_generation;
    click("rating-three");
    arrive("Rating { stars: 3 }", path, names, generation, "rating-root");
    tall_aspect_settled("rating-root");
}

// 585 direct 3:4 images outweigh the fifteen 2:3 container representatives. With at least
// 31 real samples, even all fifteen container samples cannot overturn the 3:4 median later.
fn tall_aspect_settled(label) {
    wait_until(|s| tall_aspect_ready(s), 30000);
    log("rating-folder-back: tall aspect settled " + label + " grid=" + snapshot().grid.to_debug());
}

fn tall_aspect_ready(s) {
    let minimum = 31;
    if s.item_names.len < minimum { minimum = s.item_names.len; }
    s.grid.generation == s.items_generation
        && s.grid.aspect_auto && s.grid.aspect_label == "3:4"
        && s.grid.aspect_sample_count >= minimum
}

// Use the painted geometry, including the clip viewport, rather than assuming window size
// equals grid size. The fixture targets row30 in a sixty-row list, never the final screen.
fn mid_list_geometry(s, index) {
    if s.item_names.len != 600 || s.grid.columns != 10 || s.grid.cell_height <= 0.0 {
        return false;
    }
    let row = index / s.grid.columns;
    let visible_rows = s.grid.viewport_height / s.grid.cell_height;
    let target_center = (row + 0.5) * s.grid.cell_height;
    row >= 25 && row <= 35 && visible_rows >= 6.0 && visible_rows <= 8.0
        && s.grid.scroll_offset > 0.0
        && target_center >= s.grid.scroll_offset
        && target_center <= s.grid.scroll_offset + s.grid.viewport_height
        && s.grid.content_height - s.grid.scroll_offset - s.grid.viewport_height
            >= 10.0 * s.grid.cell_height
}

fn record(label, screenshot) {
    let s = snapshot();
    let name = "<none>";
    if s.selected_index >= 0 && s.selected_index < s.item_names.len {
        name = s.item_names[s.selected_index];
    }
    log("rating-folder-back: " + label + " surface=" + s.grid_surface
        + " path=" + s.current_folder_path + " generation=" + s.items_generation
        + " selected_index=" + s.selected_index + " selected_name=" + name
        + " grid=" + s.grid.to_debug());
    if s.grid.columns > 0 && s.grid.cell_height > 0.0 {
        log("rating-folder-back: geometry " + label + " items=" + s.item_names.len
            + " selected_row=" + s.selected_index / s.grid.columns
            + " viewport_rows=" + s.grid.viewport_height / s.grid.cell_height
            + " visible_cell_capacity=" + s.grid.viewport_height / s.grid.cell_height * s.grid.columns);
    }
    // Capture every return/error; setup checkpoints are logged to stay below the 64-shot limit.
    if screenshot { capture(label); }
    s
}

fn case_label(target, kind, roundtrip) {
    let label = "folder-" + kind;
    if target.ends_with("-book.zip") { label = "zip-" + kind; }
    if roundtrip { label += "-roundtrip"; }
    label
}

fn depart(kind, forward) {
    if kind != "toolbar" && !physical_input_available() { throw "SKIPPED no-focus"; }
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

fn returned(label, opened_key, opened_name, opened_index, generation, rating_path, rating_names) {
    arrive("Rating { stars: 3 }", rating_path, rating_names, generation, label);
    tall_aspect_settled(label);
    // Assert the same observation that was logged, rather than a later capture-time state.
    let s = record(label, true);
    let ok = s.selected_index == opened_index && s.grid.selected_key == opened_key
        && s.grid.selected_name == opened_name && s.grid.visible
        && mid_list_geometry(s, opened_index);
    log("rating-folder-back: result=" + ok + " case=" + label
        + " expected_key=" + opened_key + " expected_name=" + opened_name
        + " expected_index=" + opened_index);
    ok
}

fn fourth_page(path, pages, generation, label) {
    arrive("Folder", path, pages, generation, label);
    wait_until(|s| s.grid.generation == s.items_generation && s.grid.aspect_auto
        && s.grid.columns == 6 && s.grid.aspect_label == "2:3"
        && s.grid.aspect_sample_count >= pages.len, 30000);
    click(row_widget(3));
    wait_until(|s| s.selected_index == 3 && s.grid.generation == s.items_generation
        && s.grid.selected_index == 3 && s.grid.visible, 30000);
    let s = record(label, false);
    if s.selected_index != 3 || s.grid.selected_name != "page-04.png" || !s.grid.visible
        || s.grid.columns != 6 || s.grid.aspect_label != "2:3" {
        throw "fourth page was not selected in the adopted container: " + label;
    }
}

fn run_case(target, kind, roundtrip, library, rating_path, rating_names, pages) {
    let label = case_label(target, kind, roundtrip);
    log("rating-folder-back: begin " + label);
    rating_root(rating_path, rating_names);
    let index = snapshot().item_names.index_of(target);
    if index < 0 { throw "fixture row is missing: " + target; }
    let widget = row_widget(index);
    pointer(widget, "hover");
    let opened = record(label + "-before-open", false);
    log("rating-folder-back: setup target_index=" + index + " target_row=" + index / 10);
    if !mid_list_geometry(opened, index) {
        throw "fixture does not exercise a mid-list rating row with ten columns and about seventy visible cells";
    }
    let path = library + "\\" + target;
    let key = path;
    log("rating-folder-back: opening expected_key=" + key + " expected_index=" + index);
    // First pointer click selects; the second enters the production double-click owner.
    pointer(widget, "double");
    fourth_page(path, pages, opened.items_generation, label + "-fourth-page");
    let generation = snapshot().items_generation;
    depart(kind, false);
    let ok = returned(label + "-back", key, target, index, generation, rating_path, rating_names);
    if roundtrip {
        generation = snapshot().items_generation;
        depart(kind, true);
        // Preserve the reported fourth-page setup on the second departure too.
        fourth_page(path, pages, generation, label + "-forward-fourth-page");
        generation = snapshot().items_generation;
        depart(kind, false);
        let second = returned(label + "-second-back", key, target, index, generation, rating_path, rating_names);
        ok = ok && second;
    }
    ok
}

// Scenario entry: helpers above are also exercised by headless contract tests.
let rating_names = [];
for n in 1..601 {
    let number = n.to_string();
    while number.len < 4 { number = "0" + number; }
    if n >= 299 && n <= 310 { rating_names.push(number + "-folder"); }
    else if n >= 311 && n <= 313 { rating_names.push(number + "-book.zip"); }
    else { rating_names.push(number + "-image.png"); }
}
let pages = [];
for n in 1..9 { pages.push("page-0" + n + ".png"); }
wait_until(|s| !s.startup_open_pending && !s.is_fullscreen
    && s.grid_surface == "Folder" && s.item_names == rating_names, 30000);
let library = snapshot().current_folder_path;
if !library.ends_with("\\rating-folder-back\\library") { fail("unexpected fixture root: " + library); }
let rating_path = library;
rating_path.replace("\\rating-folder-back\\library", "\\__rating_view__");
let root = select_root();
if root.target_mode != "targeted" || !root.current { fail("Root input owner is not current"); }
let failures = 0;
let cases = 0;
let executed = 0;
let skipped = 0;
for target in ["0310-folder", "0311-book.zip"] {
    for kind in ["toolbar", "alt", "browser", "appcommand", "mouse", "backspace"] {
        let rounds = [false, true];
        if kind == "backspace" { rounds = [false]; }
        for roundtrip in rounds {
            cases += 1;
            let label = case_label(target, kind, roundtrip);
            if kind != "toolbar" && !physical_input_available() {
                skipped += 1;
                log("rating-folder-back: SKIPPED no-focus case=" + label);
                continue;
            }
            executed += 1;
            try {
                if !run_case(target, kind, roundtrip, library, rating_path, rating_names, pages) { failures += 1; }
            } catch (error) {
                if error.to_string().contains("SKIPPED no-focus") {
                    skipped += 1;
                    executed -= 1;
                    log("rating-folder-back: SKIPPED no-focus case=" + label);
                    continue;
                }
                failures += 1;
                let label = case_label(target, kind, roundtrip) + "-error-" + cases;
                log("rating-folder-back: FAIL " + label + " error=" + error.to_string());
                try { record(label, true); } catch (capture_error) {
                    log("rating-folder-back: artifact error=" + capture_error.to_string());
                }
            }
        }
    }
}
log("rating-folder-back: cases=" + cases + " executed=" + executed + " skipped=" + skipped + " failures=" + failures);
if failures > 0 { fail("rating-folder-back: " + failures + " of " + cases + " cases failed"); }
log("rating-folder-back: PASS executed=" + executed + " skipped=" + skipped);
