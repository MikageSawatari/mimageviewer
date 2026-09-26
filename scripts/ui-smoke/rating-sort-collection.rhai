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

collection_sort_smoke("open_seeded_collection");
wait_until(|s| s.collection_root_visible && s.collection_id == expected_id
    && s.collection_order_mode == "Manual" && s.collection_standard_sort == "FileName"
    && s.collection_revision == 1 && s.item_names == expected_names, 30000);
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
