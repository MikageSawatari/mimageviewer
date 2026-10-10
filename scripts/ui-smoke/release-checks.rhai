// Sequential suite recipe. All paths are generated literals from the verified manifest.
const CASE_ID = "__CASE_ID__";
const CASE_DIRECTORY = "__CASE_DIRECTORY__";
const STATIC_PATH = "__STATIC_PATH__";
const MANY_PATH = "__MANY_PATH__";
const VIDEO_CHILD = "__VIDEO_CHILD__";
const VIDEO_SOURCE = "__VIDEO_SOURCE__";
const VIDEO_PARENT = "__VIDEO_PARENT__";
const SEARCH_TERM = "__SEARCH_TERM__";
const SEARCH_HITS = __SEARCH_HITS__;
const CACHE_PATHS = __CACHE_PATHS__;
const NAVIGATION_PATHS = __NAVIGATION_PATHS__;
const INCLUDE_FIRST_AI = __INCLUDE_FIRST_AI__;

fn root_selectable(s) {
    for w in s.windows {
        if w.role == "root" && w.host_ready { return true; }
    }
    false
}

// Win32 canonical paths can carry a verbatim prefix and backslash separators.
// Normalize only for comparing observed physical paths to generated literals.
fn comparable_path(path) {
    path.replace("\\", "/");
    if path.starts_with("//?/") { path = path.sub_string(4); }
    path.to_lower()
}
fn settled(path) {
    let last = #{};
    for attempt in 0..1800 {
        let state = release_check_observe();
        if comparable_path(state.current_folder_path) == comparable_path(path)
            && state.loading_complete && state.index_settled && state.interaction_ready {
            return state;
        }
        last = state;
        sleep(100);
    }
    fail("release-check readiness timeout for " + path + ": observed=" + last.current_folder_path
        + " generation=" + last.items_generation + " interaction_ready=" + last.interaction_ready
        + " loading_complete=" + last.loading_complete + " index_settled=" + last.index_settled);
}
fn focused() {
    for attempt in 0..200 {
        if release_check_observe().focused { return; }
        sleep(100);
    }
    fail("environment: exact root foreground was not acquired");
}
fn search(term, expected_hits) {
    focused();
    let before = release_check_observe();
    tap_key("G", "ctrl");
    for attempt in 0..300 {
        let state = release_check_observe();
        if state.search_active && state.search_focused { break; }
        if attempt == 299 { fail("environment: Ctrl+G search did not focus"); }
        sleep(100);
    }
    release_check_text(term);
    for attempt in 0..1800 {
        let state = release_check_observe();
        if state.search_query == term && state.search_done && !state.search_searching
            && state.interaction_ready {
            return #{ ctrl_g_receipt: true, text_receipt: true, completed: true,
                start_t: before.perf_t, end_t: state.perf_t, expected_query: term, actual_query: state.search_query,
                expected_hits: expected_hits, actual_hits: state.search_hits };
        }
        sleep(100);
    }
    fail("environment: fixture search did not settle");
}

release_check_begin(CASE_ID, CASE_DIRECTORY);
// host_ready joins the real context/HWND/backend identity used by select_root.
// host_incarnation belongs to detached hosts and is always absent on root.
// This passive startup wait adds no repaint or input to the measured idle window.
wait_until(|s| root_selectable(s), 30000);
select_root();
for attempt in 0..1800 {
    let state = release_check_observe();
    if state.index_settled && state.interaction_ready { break; }
    if attempt == 1799 { fail("environment: initial index scans did not settle"); }
    sleep(100);
}
let evidence = #{ case_id: CASE_ID, pid: release_check_observe().pid };
if CASE_ID == "seed-profile" {
    for path in CACHE_PATHS {
        release_check_open(path);
        settled(path);
        release_check_seed("cache");
    }
    release_check_open(VIDEO_CHILD);
    settled(VIDEO_CHILD);
    release_check_pin(VIDEO_SOURCE);
    // DB persistence precedes the ordinary pin-triggered child reload. Wait for
    // its dirty/classification/sidecar owners before admitting another open.
    settled(VIDEO_CHILD);
    release_check_open(VIDEO_PARENT);
    settled(VIDEO_PARENT);
    release_check_seed("cache");
    evidence.search = search(SEARCH_TERM, SEARCH_HITS);
    release_check_seed("search");
} else if CASE_ID == "first-ai" {
    release_check_open(STATIC_PATH);
    settled(STATIC_PATH);
    focused();
    evidence.first_ai = release_check_first_ai(STATIC_PATH);
} else if CASE_ID.starts_with("perf-") {
    // Preserve the initial startup lf_scan; do not turn it into a second open.
    if comparable_path(release_check_observe().current_folder_path) != comparable_path(MANY_PATH) {
        fail("environment: initial perf folder does not match generated many-folder fixture");
    }
    settled(MANY_PATH);
    evidence.folder_open = #{ completed: true };
    release_check_open(NAVIGATION_PATHS[0]);
    settled(NAVIGATION_PATHS[0]);
    let navigation = [];
    for step in 1..6 {
        focused();
        let before = release_check_observe();
        tap_key("Down", "ctrl");
        let after = settled(NAVIGATION_PATHS[step]);
        navigation.push(#{ receipt: true, before_generation: before.items_generation,
            after_generation: after.items_generation, before_path: before.current_folder_path,
            after_path: after.current_folder_path, start_t: before.perf_t, end_t: after.perf_t });
    }
    evidence.navigation = navigation;
    evidence.search = search(SEARCH_TERM, SEARCH_HITS);
} else {
    let path = if CASE_ID == "video-pin-background" { VIDEO_PARENT }
        else if CASE_ID.starts_with("tray-") { MANY_PATH } else { STATIC_PATH };
    release_check_open(path);
    if CASE_ID == "tray-close-while-loading" {
        release_check_close("while-loading");
    } else {
        settled(path);
        if CASE_ID == "tray-close-after-loading" { release_check_close("after-loading"); }
    }
    // One worker sleep includes warmup, actual sample, and completion margins.
    // pause waits read-only for sampler-finished.json after writing its real end clock.
    release_check_pause(CASE_DIRECTORY, 35);
}
release_check_evidence(CASE_DIRECTORY, evidence);
release_check_quit();
