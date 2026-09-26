// Disposable Collection root, real egui pointer input for the sort ComboBox.
// The runner checks collection.db again after the application exits.

let expected_id = "80f58851-997b-4b80-90bc-f50bb1d2523e";
let expected_names = ["01-one.png", "02-unrated.png", "03-two.png"];
let expected_rows = [
    "手動", "シャッフル（再選択で並べ直す）",
    "名前↑", "名前↓", "番号↑", "番号↓", "日付↑", "日付↓",
    "サイズ↑", "サイズ↓", "評価↑", "評価↓"
];
let reason = "コレクションでは評価順を使えません。手動の並べ替えで順番を変えられます";

fn collection_state(s) {
    "runtime=" + s.collection_runtime_phase
        + " error=" + s.collection_runtime_error
        + " seeded_id_present=" + s.collection_seeded_id_present.to_debug()
        + " surface=" + s.grid_surface
        + " startup_pending=" + s.startup_open_pending.to_debug()
        + " folder=" + s.current_folder_path
        + " names=" + s.item_names.to_debug()
        + " collection_id=" + s.collection_id
        + " revision=" + s.collection_revision.to_debug()
}

fn wait_collection(check, timeout_ms, label) {
    try {
        wait_until(check, timeout_ms);
    } catch (error) {
        log("collection-sort: " + label + " timed out: " + collection_state(snapshot()));
        throw error;
    }
}

// The runner also supplies this fixture as a startup path. Wait until that
// asynchronous folder open has installed its list before replacing it.
wait_collection(|s| s.collection_runtime_phase == "Ready"
    || s.collection_runtime_phase == "Failed", 10000, "Collection runtime");
let runtime = snapshot();
if runtime.collection_runtime_phase != "Ready" || !runtime.collection_seeded_id_present {
    fail("seeded Collection is unavailable: " + collection_state(runtime));
}
wait_collection(|s| !s.startup_open_pending && s.grid_surface == "Folder"
    && s.folder_load_requests >= 1 && s.item_names == expected_names,
    30000, "startup fixture");
log("collection-sort: startup fixture ready: " + collection_state(snapshot()));

collection_sort_smoke("open_seeded_collection");
wait_collection(|s| s.collection_root_visible && s.collection_id == expected_id
    && s.collection_order_mode == "Manual" && s.collection_standard_sort == "FileName"
    && s.collection_revision == 1 && s.item_names == expected_names,
    30000, "Collection root");
let before = snapshot();
log("collection-sort: root ready names=" + before.item_names.to_debug());

click_widget("collection_sort_combo");
wait_until(|s| s.collection_sort_popup.open && s.collection_sort_popup.rows.len == 12, 10000);
let popup = snapshot().collection_sort_popup;
if popup.needs_scrolling || !popup.within_screen {
    fail("Collection sort popup is clipped or needs scrolling: " + popup.to_debug());
}
for i in 0..expected_rows.len {
    let row = popup.rows[i];
    if row.label != expected_rows[i] || !row.visible
        || row.disabled != (i >= 10) {
        fail("Collection sort popup row " + i + " is wrong: " + row.to_debug());
    }
}

hover_sort_row("評価↑");
wait_until(|s| s.collection_sort_popup.open
    && s.collection_sort_popup.tooltip_open
    && s.collection_sort_popup.tooltip_text == reason, 10000);
click_sort_row("評価↑");
sleep(600);
let after = snapshot();
if !after.collection_root_visible || after.collection_id != before.collection_id
    || after.collection_order_mode != before.collection_order_mode
    || after.collection_standard_sort != before.collection_standard_sort
    || after.collection_revision != before.collection_revision
    || after.item_names != before.item_names
    || after.items_generation != before.items_generation {
    fail("disabled Collection rating click changed the view: before="
        + before.to_debug() + " after=" + after.to_debug());
}
log("collection-sort: PASS");
