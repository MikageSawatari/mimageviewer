// Queue-injected PadEvents use the physical consumer and normal UI handlers.
// gilrs/OS reads are deliberately outside this scenario's coverage.

fn root_window(s) {
    for window in s.windows {
        if window.role == "root" { return window; }
    }
    fail("GamepadBasic has no root content snapshot");
}

fn wait_dump(ready, label) {
    try {
        wait_until(ready, 15000);
    } catch (error) {
        let s = snapshot();
        log("gamepad-basic: " + label + " gamepad=" + s.gamepad.to_debug()
            + " path=" + s.current_folder_path + " selected=" + s.selected_index
            + " fullscreen=" + s.is_fullscreen + " windows=" + s.windows.to_debug());
        throw error;
    }
}

wait_dump(|s| s.gamepad.enabled && s.gamepad.foreground && s.gamepad.reader == "script"
    && !s.startup_open_pending && !s.is_fullscreen && s.grid_surface == "Folder"
    && s.item_names == ["01-red.png", "02-green.png", "03-blue.png"]
    && s.pending_thumbs == 0, "startup");
select_root();
let initial = snapshot();
let sub = initial.current_folder_path;
if !sub.ends_with("\\library\\sub") { fail("unexpected fixture startup path: " + sub); }
let library = sub.sub_string(0, sub.len - 4);
let expected_index = initial.selected_index + 1;
if expected_index < 0 || expected_index >= initial.items_len {
    fail("fixture selection cannot move right: " + initial.selected_index);
}
capture("gamepad-start");

pad_tap("right");
wait_dump(|s| !s.is_fullscreen && s.current_folder_path == sub
    && s.selected_index == expected_index
    && root_window(s).selected_item_identity != "", "dpad-right");
let expected_identity = root_window(snapshot()).selected_item_identity;
capture("gamepad-right");

pad_tap("a");
wait_dump(|s| {
    let window = root_window(s);
    s.is_fullscreen && s.current_folder_path == sub
        && window.page_index == expected_index && window.item_identity == expected_identity
        && window.selected_item_identity == expected_identity && window.page_ready
}, "a-fullscreen");
// Dedicated main fullscreen has no root-owned paint projection. Capture its
// actual viewport as well; page identity and cache readiness remain assertions.
capture("gamepad-fullscreen", "all");

pad_tap("b");
wait_dump(|s| !s.is_fullscreen && s.grid_surface == "Folder"
    && s.current_folder_path == sub && s.selected_index == expected_index, "b-grid");
capture("gamepad-grid");

// The ring commits on X release while Up is still held. Preserve this order.
pad_press("x");
pad_press("up");
pad_release("x");
pad_release("up");
wait_dump(|s| !s.is_fullscreen && !s.startup_open_pending
    && s.grid_surface == "Folder" && s.current_folder_path == library
    && s.item_names == ["sub"], "x-ring-parent");
capture("gamepad-parent");

pad_neutral();
pad_disconnect();
wait_dump(|s| s.gamepad.injected == s.gamepad.drained && s.gamepad.discarded == 0,
    "final-drain");
log("gamepad-basic: passed " + snapshot().gamepad.to_debug());
