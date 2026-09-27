// Disposable, fully unattended always-on-top style smoke.

fn count_role(s, role) {
    let count = 0;
    for host in s.host_styles {
        if host.role == role { count += 1; }
    }
    count
}

fn count_presentation(s, role, presentation) {
    let count = 0;
    for host in s.host_styles {
        if host.role == role && host.presentation == presentation { count += 1; }
    }
    count
}

fn style(s, role) {
    for host in s.host_styles {
        if host.role == role { return host; }
    }
    fail("missing host style for " + role);
}

fn role_is_foreground(s, role) {
    for host in s.host_styles {
        if host.role == role && host.foreground_hwnd == host.hwnd { return true; }
    }
    false
}

fn expect_role(role, count, topmost) {
    wait_until(|s| {
        let seen = 0;
        for host in s.host_styles {
            if host.role == role {
                if host.topmost != topmost { return false; }
                seen += 1;
            }
        }
        seen >= count
    }, 30000);
}

fn styles(label) {
    let s = snapshot();
    log("always-on-top " + label + " desired=" + s.always_on_top
        + " visible=" + s.window_visible + " hosts=" + s.host_styles.to_debug());
}

log("always-on-top: root OFF/ON");
wait_until(|s| s.items_len == 3 && !s.is_fullscreen && count_role(s, "root") == 1, 30000);
expect_role("root", 1, false);
capture("aot-root-off");
always_on_top_smoke("on");
wait_until(|s| s.always_on_top, 10000);
expect_role("root", 1, true);
styles("root-on");
capture("aot-root-on");

// Measure first: a lost style is recorded for a targeted restore fix, rather
// than being hidden by an unconditional reassert in production code.
always_on_top_smoke("minimize_root");
wait_until(|s| style(s, "root").minimized, 15000);
styles("root-minimized");
always_on_top_smoke("restore_root");
wait_until(|s| !style(s, "root").minimized, 15000);
styles("root-restored");
capture("aot-root-restored");
always_on_top_smoke("off");
expect_role("root", 1, false);
always_on_top_smoke("on");
expect_role("root", 1, true);

log("always-on-top: switch to Details and move the real pointer over a preview cell");
select_root();
run_action("GridToggleDetailsView");
move_details_preview_pointer(15000);
wait_until(|s| count_role(s, "preview") == 1, 15000);
expect_role("preview", 1, true);
styles("hover-preview-on");
capture("aot-preview-on", "all");
always_on_top_smoke("off");
expect_role("preview", 1, true);
expect_role("root", 1, false);
styles("hover-preview-off");
always_on_top_smoke("on");
expect_role("root", 1, true);
select_root();
run_action("GridToggleDetailsView");
wait_until(|s| count_role(s, "preview") == 0, 15000);
styles("post-preview-close");

// Use the same root action path with OFF and ON. A foreground failure with OFF
// rules out the topmost level as the sole cause of the handoff failure.
log("always-on-top: still fullscreen activation with root OFF");
always_on_top_smoke("off");
expect_role("root", 1, false);
select_root();
run_action("GridMoveFirst");
run_action("GridOpenSelected");
wait_until(|s| s.is_fullscreen && count_role(s, "fullscreen") == 1, 30000);
expect_role("fullscreen", 1, false);
styles("still-fullscreen-off-before-focus");
wait_until(|s| role_is_foreground(s, "fullscreen"), 10000);
sleep(750);
let off_steady = snapshot();
if !off_steady.is_fullscreen || !role_is_foreground(off_steady, "fullscreen") {
    fail("OFF fullscreen closed or lost foreground after focus guard grace");
}
styles("still-fullscreen-off-stable");
always_on_top_smoke("close_fullscreen");
wait_until(|s| !s.is_fullscreen && count_role(s, "fullscreen") == 0, 30000);
always_on_top_smoke("on");
expect_role("root", 1, true);

log("always-on-top: separate still fullscreen host");
select_root();
run_action("GridMoveFirst");
run_action("GridOpenSelected");
wait_until(|s| s.is_fullscreen && count_role(s, "fullscreen") == 1, 30000);
styles("still-fullscreen-before-focus");
wait_until(|s| role_is_foreground(s, "fullscreen"), 10000);
wait_until(|s| s.target_registered && s.focused, 10000);
expect_role("fullscreen", 1, true);
sleep(750);
let on_steady = snapshot();
if !on_steady.is_fullscreen || !role_is_foreground(on_steady, "fullscreen") {
    fail("ON fullscreen closed or lost foreground after focus guard grace");
}
styles("still-fullscreen-on");
capture("aot-still-on", "all");
log("always-on-top: F12 linked detached handoff");
tap_key("F12");
wait_until(|s| count_role(s, "detached") == 1, 30000);
wait_until(|s| role_is_foreground(s, "detached") && s.target_registered && s.focused, 10000);
expect_role("detached", 1, true);
styles("f12-detached-on");
capture("aot-f12-detached", "all");
tap_key("F12");
wait_until(|s| count_role(s, "fullscreen") == 1 && count_role(s, "detached") == 0, 30000);
expect_role("fullscreen", 1, true);
always_on_top_smoke("off");
expect_role("root", 1, false);
expect_role("fullscreen", 1, false);
styles("still-fullscreen-off");
capture("aot-still-off", "all");
always_on_top_smoke("on");
expect_role("fullscreen", 1, true);
always_on_top_smoke("close_fullscreen");
wait_until(|s| !s.is_fullscreen, 30000);

log("always-on-top: native video fullscreen backdrop and HUD");
select_root();
run_action("GridMoveLast");
run_action("GridOpenSelected");
wait_until(|s| s.is_fullscreen && count_role(s, "presenter") == 1
    && count_role(s, "hud") == 1, 30000);
expect_role("root", 1, true);
// The egui fullscreen viewport is only a startup cover. Once the native
// presenter is ready, the App hides and retires that cover.
wait_until(|s| count_role(s, "fullscreen") == 0, 10000);
let native_on = snapshot();
let hud = style(native_on, "hud");
let presenter = style(native_on, "presenter");
if !hud.topmost || !hud.noactivate || hud.owner_hwnd != presenter.hwnd {
    fail("native fullscreen HUD lost its existing style or presenter owner");
}
styles("native-video-on-order-measurement");
capture("aot-native-on", "all");
always_on_top_smoke("off");
expect_role("root", 1, false);
wait_until(|s| count_role(s, "fullscreen") == 0
    && count_role(s, "presenter") == 1 && count_role(s, "hud") == 1, 10000);
let native_off = snapshot();
if !style(native_off, "hud").topmost || !style(native_off, "hud").noactivate {
    fail("native fullscreen HUD exception changed while OFF");
}
styles("native-video-off-order-measurement");
capture("aot-native-off", "all");
always_on_top_smoke("on");
expect_role("root", 1, true);
always_on_top_smoke("close_fullscreen");
wait_until(|s| !s.is_fullscreen, 30000);

log("always-on-top: independent detached windows");
always_on_top_smoke("enable_independent_windows");
select_root();
run_action("GridMoveFirst");
run_action("GridOpenSelected");
wait_until(|s| count_role(s, "detached") >= 1, 30000);
select_root();
run_action("GridMoveFirst");
wait_until(|s| s.target_registered && s.focused, 10000);
tap_key("Right");
run_action("GridOpenSelected");
wait_until(|s| count_role(s, "detached") >= 2
    && count_presentation(s, "detached", "passive_deferred_frozen") >= 1
    && count_presentation(s, "detached", "active_immediate") >= 1, 30000);
expect_role("detached", 2, true);
styles("detached-on");
capture("aot-detached-on", "all");
always_on_top_smoke("off");
expect_role("root", 1, false);
expect_role("detached", 2, false);
styles("detached-off");
capture("aot-detached-off", "all");
always_on_top_smoke("on");
expect_role("root", 1, true);
expect_role("detached", 2, true);

log("always-on-top: hiding to tray and restoring through the tray Open handler");
always_on_top_smoke("hide_to_tray");
wait_until(|s| !s.window_visible, 15000);
styles("tray-hidden");
always_on_top_smoke("restore_from_tray");
wait_until(|s| s.window_visible, 15000);
styles("tray-restored-measurement");
capture("aot-tray-restored", "all");
log("always-on-top: completed; compare restore style and native order logs before deciding on reasserts");
