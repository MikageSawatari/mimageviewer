// Disposable always-on-top style smoke. The only human step is clicking the
// tray icon's Open command after the script hides the window.

fn count_role(s, role) {
    let count = 0;
    for host in s.host_styles {
        if host.role == role { count += 1; }
    }
    count
}

fn style(s, role) {
    for host in s.host_styles {
        if host.role == role { return host; }
    }
    fail("missing host style for " + role);
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

log("always-on-top: separate still fullscreen host");
select_root();
run_action("GridMoveFirst");
run_action("GridOpenSelected");
wait_until(|s| s.is_fullscreen && count_role(s, "fullscreen") == 1, 30000);
expect_role("fullscreen", 1, true);
styles("still-fullscreen-on");
capture("aot-still-on", "all");
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
    && count_role(s, "fullscreen") == 1, 30000);
expect_role("root", 1, true);
expect_role("fullscreen", 1, true);
wait_until(|s| count_role(s, "hud") == 1, 15000);
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
expect_role("fullscreen", 1, false);
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
tap_key("Right");
run_action("GridOpenSelected");
wait_until(|s| count_role(s, "detached") >= 2, 30000);
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

log("always-on-top: hiding to tray; use tray Open within 60 seconds");
always_on_top_smoke("hide_to_tray");
wait_until(|s| !s.window_visible, 15000);
styles("tray-hidden");
wait_until(|s| s.window_visible, 60000);
styles("tray-restored-measurement");
capture("aot-tray-restored", "all");
log("always-on-top: completed; compare restore style and native order logs before deciding on reasserts");
