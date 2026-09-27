// Disposable A/F/G and Collection C/B/D history regression.
// Every Back, Forward, and parent step uses the synthetic input timeline,
// which enters the normal keymap and viewport routing path.

let collection_id = "80f58851-997b-4b80-90bc-f50bb1d2523e";
let collection_surface = "Collection(" + collection_id + ")";
let rating_surface = "Rating { stars: 1 }";

fn state(s) {
    "surface=" + s.grid_surface + " path=" + s.current_folder_path
        + " collection=" + s.collection_id + " names=" + s.item_names.to_debug()
        + " generation=" + s.items_generation;
}

fn arrive(surface, path, names, label) {
    try {
        wait_until(|s| !s.startup_open_pending && !s.is_fullscreen
            && s.grid_surface == surface && s.current_folder_path == path
            && s.item_names == names, 30000);
    } catch (error) {
        log("folder-history: " + label + " timed out: " + state(snapshot()));
        throw error;
    }
    capture(label);
    log("folder-history: " + label + " " + state(snapshot()));
}

fn arrive_collection_root(surface, id, label) {
    try {
        wait_until(|s| !s.startup_open_pending && !s.is_fullscreen
            && s.grid_surface == surface && s.collection_root_visible
            && s.collection_id == id && s.item_names == ["B"], 30000);
    } catch (error) {
        log("folder-history: " + label + " timed out: " + state(snapshot()));
        throw error;
    }
    capture(label);
    log("folder-history: " + label + " " + state(snapshot()));
}

wait_until(|s| !s.startup_open_pending && s.grid_surface == "Folder"
    && s.item_names == ["B", "F"], 30000);
let a = snapshot().current_folder_path;
if a == "" { fail("startup A has no physical path"); }
let f = a + "\\F";
let g = f + "\\G";
let b = a + "\\B";
let d = b + "\\D";
select_root();
arrive("Folder", a, ["B", "F"], "a-start");

folder_history_smoke("open_rating_one");
wait_until(|s| s.grid_surface == rating_surface && s.item_names == ["F"], 30000);
let rating_path = snapshot().current_folder_path;
if rating_path == "" || rating_path == a {
    fail("Rating root did not adopt its synthetic path: " + state(snapshot()));
}
arrive(rating_surface, rating_path, ["F"], "rating-one");
run_action("GridMoveFirst");
run_action("GridOpenSelected");
arrive("Folder", f, ["G", "z-page.png"], "rating-child-f");
run_action("GridMoveFirst");
run_action("GridOpenSelected");
arrive("Folder", g, ["g-page.png"], "rating-descendant-g");
tap_key("Left", "alt");
arrive("Folder", f, ["G", "z-page.png"], "rating-back-f");
tap_key("Left", "alt");
arrive(rating_surface, rating_path, ["F"], "rating-back-root");
tap_key("Right", "alt");
arrive("Folder", f, ["G", "z-page.png"], "rating-forward-f");
tap_key("Backspace");
arrive(rating_surface, rating_path, ["F"], "rating-bs-root");
tap_key("Backspace");
arrive("Folder", a, ["B", "F"], "rating-bs-a");

folder_history_smoke("open_seeded_collection");
arrive_collection_root(collection_surface, collection_id, "collection-root-c");
run_action("GridMoveFirst");
run_action("GridOpenSelected");
arrive(collection_surface, b, ["D", "z-page.png"], "collection-child-b");
run_action("GridMoveFirst");
run_action("GridOpenSelected");
arrive(collection_surface, d, ["d-page.png"], "collection-descendant-d");
tap_key("Left", "alt");
arrive(collection_surface, b, ["D", "z-page.png"], "collection-back-b");
tap_key("Left", "alt");
arrive_collection_root(collection_surface, collection_id, "collection-back-c");
tap_key("Right", "alt");
arrive(collection_surface, b, ["D", "z-page.png"], "collection-forward-b");
tap_key("Backspace");
arrive_collection_root(collection_surface, collection_id, "collection-bs-c");
log("folder-history: PASS");
