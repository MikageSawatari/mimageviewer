// Rating snapshot order and Preferences commit in a disposable real app.
// The runner seeds rating.db and opens the three-image fixture in RatingDesc.

fn names(s, expected) {
    s.item_names == expected
}

fn wait_rating(check, label) {
    try {
        wait_until(check, 30000);
    } catch (error) {
        let s = snapshot();
        log("rating-sort: " + label + " timed out: names=" + s.item_names.to_debug()
            + " ratings=" + s.item_ratings.to_debug()
            + " generation=" + s.items_generation
            + " load_requests=" + s.folder_load_requests
            + " smart_busy=" + s.smart_folder_busy);
        throw error;
    }
}

wait_rating(|s| s.sort_order == "RatingDesc" && s.items_len == 3
    && names(s, ["02-unrated.png", "03-two.png", "01-one.png"]), "initial order");
select_root();
run_action("GridMoveLast");
let initial = snapshot();
run_action("RatingItem5");
wait_rating(|s| s.item_ratings.len == 3 && s.item_ratings[2] == 5,
    "rating write");
let after_write = snapshot();
if after_write.items_generation != initial.items_generation
    || after_write.item_names != initial.item_names {
    fail("rating write moved rows before refresh");
}

click_widget("最新の情報に更新");
wait_rating(|s| s.items_generation > initial.items_generation
    && names(s, ["01-one.png", "02-unrated.png", "03-two.png"]),
    "sort refresh");
let refreshed = snapshot();
if refreshed.items_generation != initial.items_generation + 1 {
    fail("sort refresh installed more than one folder load");
}
if refreshed.folder_load_requests != initial.folder_load_requests + 1 {
    fail("sort refresh issued a duplicate folder load request");
}
sleep(500);
let after_refresh_pause = snapshot();
if after_refresh_pause.items_generation != refreshed.items_generation
    || after_refresh_pause.folder_load_requests != refreshed.folder_load_requests {
    fail("sort refresh issued another folder load request");
}

rating_sort_smoke("open_thumbnail_preferences");
wait_rating(|s| s.preferences_open && s.preferences_page == "Thumbnail",
    "Preferences thumbnail page");
click_widget("すべての評価より下");
wait_rating(|s| s.preferences_draft_unrated_position == "BelowAll",
    "Preferences unrated radio");
let before_ok = snapshot().items_generation;
click_widget("OK");
wait_rating(|s| !s.preferences_open
    && s.rating_sort_unrated_position == "BelowAll"
    && names(s, ["01-one.png", "03-two.png", "02-unrated.png"]),
    "Preferences OK and order");
if snapshot().items_generation != before_ok + 1 {
    fail("Preferences OK did not apply exactly one rating-sort reload");
}

rating_sort_smoke("open_first_smart_folder");
wait_rating(|s| s.smart_folder_name == "RatingSortSmoke"
    && s.smart_folder_root_visible && s.smart_folder_session_phase == "Root"
    && !s.smart_folder_busy && s.items_len == 3
    && names(s, ["01-one.png", "03-two.png", "02-unrated.png"]),
    "Smart Folder settlement");
let settled = snapshot();
sleep(3000);
let after_settle = snapshot();
if after_settle.smart_folder_busy || !after_settle.smart_folder_root_visible
    || after_settle.smart_folder_session_phase != "Root"
    || !names(after_settle, ["01-one.png", "03-two.png", "02-unrated.png"])
    || after_settle.items_generation != settled.items_generation {
    fail("Smart Folder repeated prepare after settling");
}
log("rating-sort: PASS");
