// Disposable A/F/G and Collection C/B/D history regression, including ZIP/PDF books.
// Every Back, Forward, and parent step uses the synthetic input timeline,
// which enters the normal keymap and viewport routing path.

let collection_id = "80f58851-997b-4b80-90bc-f50bb1d2523e";
let collection_surface = "Collection(" + collection_id + ")";
let rating_surface = "Rating { stars: 1 }";

// Rating rows keep the rating store's normalized key (lowercase, '/'), so names and paths reached
// through the Rating view are lowercase (released behavior, backlog 1.297). This
// smoke checks history, not casing, so every comparison ignores case.
fn lower_names(names) { names.map(|n| n.to_lower()) }
fn norm_path(p) { let x = p.to_lower(); x.replace("/", "\\"); x }
fn same_path(a, b) { norm_path(a) == norm_path(b) }

fn state(s) {
    "surface=" + s.grid_surface + " path=" + s.current_folder_path
        + " collection=" + s.collection_id + " names=" + s.item_names.to_debug()
        + " generation=" + s.items_generation;
}

fn arrive(surface, path, names, label) {
    try {
        wait_until(|s| !s.startup_open_pending && !s.is_fullscreen
            && s.grid_surface == surface && same_path(s.current_folder_path, path)
            && lower_names(s.item_names) == lower_names(names), 30000);
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
wait_until(|s| s.grid_surface == rating_surface && lower_names(s.item_names) == ["f"], 30000);
let rating_path = snapshot().current_folder_path;
if rating_path == "" || same_path(rating_path, a) {
    fail("Rating root did not adopt its synthetic path: " + state(snapshot()));
}
arrive(rating_surface, rating_path, ["F"], "rating-one");
run_action("GridMoveFirst");
run_action("GridOpenSelected");
arrive("Folder", f, ["G", "x-book.pdf", "y-book.zip", "z-page.png"], "rating-child-f");
run_action("GridMoveFirst");
run_action("GridOpenSelected");
arrive("Folder", g, ["g-page.png"], "rating-descendant-g");
tap_key("Left", "alt");
arrive("Folder", f, ["G", "x-book.pdf", "y-book.zip", "z-page.png"], "rating-back-f");
tap_key("Left", "alt");
arrive(rating_surface, rating_path, ["F"], "rating-back-root");
tap_key("Right", "alt");
arrive("Folder", f, ["G", "x-book.pdf", "y-book.zip", "z-page.png"], "rating-forward-f");
run_action("GridMoveFirst");
tap_key("Right");
tap_key("Right");
run_action("GridOpenSelected");
let rating_zip = f + "\\y-book.zip";
arrive("Folder", rating_zip, ["chapter", "root.png"], "rating-zip-root");
run_action("GridMoveFirst");
run_action("GridOpenSelected");
arrive("Folder", rating_zip, ["first.png", "second.png"], "rating-zip-chapter");
tap_key("Backspace");
arrive("Folder", rating_zip, ["chapter", "root.png"], "rating-zip-bs-internal");
tap_key("Left", "alt");
arrive("Folder", f, ["G", "x-book.pdf", "y-book.zip", "z-page.png"], "rating-zip-back-outer");
tap_key("Right", "alt");
arrive("Folder", rating_zip, ["chapter", "root.png"], "rating-zip-forward-outer");
tap_key("Backspace");
arrive("Folder", f, ["G", "x-book.pdf", "y-book.zip", "z-page.png"], "rating-zip-bs-parent");
run_action("GridMoveFirst");
tap_key("Right");
run_action("GridOpenSelected");
let rating_pdf = f + "\\x-book.pdf";
arrive("Folder", rating_pdf, ["Page 1", "Page 2"], "rating-pdf-pages");
run_action("GridMoveFirst");
tap_key("Right");
arrive("Folder", rating_pdf, ["Page 1", "Page 2"], "rating-pdf-page-move");
tap_key("Left", "alt");
arrive("Folder", f, ["G", "x-book.pdf", "y-book.zip", "z-page.png"], "rating-pdf-back-outer");
tap_key("Right", "alt");
arrive("Folder", rating_pdf, ["Page 1", "Page 2"], "rating-pdf-forward-outer");
tap_key("Backspace");
arrive("Folder", f, ["G", "x-book.pdf", "y-book.zip", "z-page.png"], "rating-pdf-bs-parent");
tap_key("Backspace");
arrive(rating_surface, rating_path, ["F"], "rating-bs-root");
tap_key("Backspace");
arrive("Folder", a, ["B", "F"], "rating-bs-a");

// The earlier Rating PDF visit has populated F's catalog. Reopen it through an ordinary
// physical grid input, then read the durable placeholder-adoption event instead of racing the
// PDF worker's verification frame.
let warm_before = snapshot().pdf_warm_adoption_sequence;
run_action("GridMoveFirst");
tap_key("Right");
tap_key("Enter");
arrive("Folder", f, ["G", "x-book.pdf", "y-book.zip", "z-page.png"], "warm-direct-folder-f");
run_action("GridMoveFirst");
tap_key("Right");
tap_key("Enter");
wait_until(|s| s.pdf_warm_adoption_sequence > warm_before
    && s.pdf_warm_adoption_phase == "CommittedPlaceholder"
    && s.pdf_warm_adoption_path == f + "\\x-book.pdf", 30000);
arrive("Folder", f + "\\x-book.pdf", ["Page 1", "Page 2"], "warm-direct-pdf");
tap_key("Left", "alt");
arrive("Folder", f, ["G", "x-book.pdf", "y-book.zip", "z-page.png"], "warm-direct-back-f");
tap_key("Right", "alt");
arrive("Folder", f + "\\x-book.pdf", ["Page 1", "Page 2"], "warm-direct-forward-pdf");
tap_key("Backspace");
arrive("Folder", f, ["G", "x-book.pdf", "y-book.zip", "z-page.png"], "warm-direct-bs-f");
tap_key("Backspace");
arrive("Folder", a, ["B", "F"], "warm-direct-bs-a");

folder_history_smoke("open_seeded_collection");
arrive_collection_root(collection_surface, collection_id, "collection-root-c");
run_action("GridMoveFirst");
run_action("GridOpenSelected");
arrive(collection_surface, b, ["D", "x-book.pdf", "y-book.zip", "z-page.png"], "collection-child-b");
run_action("GridMoveFirst");
run_action("GridOpenSelected");
arrive(collection_surface, d, ["d-page.png"], "collection-descendant-d");
tap_key("Left", "alt");
arrive(collection_surface, b, ["D", "x-book.pdf", "y-book.zip", "z-page.png"], "collection-back-b");
tap_key("Left", "alt");
arrive_collection_root(collection_surface, collection_id, "collection-back-c");
tap_key("Right", "alt");
arrive(collection_surface, b, ["D", "x-book.pdf", "y-book.zip", "z-page.png"], "collection-forward-b");
run_action("GridMoveFirst");
tap_key("Right");
tap_key("Right");
run_action("GridOpenSelected");
let collection_zip = b + "\\y-book.zip";
arrive(collection_surface, collection_zip, ["chapter", "root.png"], "collection-zip-root");
run_action("GridMoveFirst");
run_action("GridOpenSelected");
arrive(collection_surface, collection_zip, ["first.png", "second.png"], "collection-zip-chapter");
tap_key("Backspace");
arrive(collection_surface, collection_zip, ["chapter", "root.png"], "collection-zip-bs-internal");
tap_key("Left", "alt");
arrive(collection_surface, b, ["D", "x-book.pdf", "y-book.zip", "z-page.png"], "collection-zip-back-outer");
tap_key("Right", "alt");
arrive(collection_surface, collection_zip, ["chapter", "root.png"], "collection-zip-forward-outer");
tap_key("Backspace");
arrive_collection_root(collection_surface, collection_id, "collection-zip-bs-root");
run_action("GridMoveFirst");
run_action("GridOpenSelected");
arrive(collection_surface, b, ["D", "x-book.pdf", "y-book.zip", "z-page.png"], "collection-reopen-b");
run_action("GridMoveFirst");
tap_key("Right");
run_action("GridOpenSelected");
let collection_pdf = b + "\\x-book.pdf";
arrive(collection_surface, collection_pdf, ["Page 1", "Page 2"], "collection-pdf-pages");
run_action("GridMoveFirst");
tap_key("Right");
arrive(collection_surface, collection_pdf, ["Page 1", "Page 2"], "collection-pdf-page-move");
tap_key("Left", "alt");
arrive(collection_surface, b, ["D", "x-book.pdf", "y-book.zip", "z-page.png"], "collection-pdf-back-outer");
tap_key("Right", "alt");
arrive(collection_surface, collection_pdf, ["Page 1", "Page 2"], "collection-pdf-forward-outer");
tap_key("Backspace");
arrive_collection_root(collection_surface, collection_id, "collection-pdf-bs-root");
run_action("GridMoveFirst");
run_action("GridOpenSelected");
arrive(collection_surface, b, ["D", "x-book.pdf", "y-book.zip", "z-page.png"], "collection-reopen-before-bs");
tap_key("Backspace");
arrive_collection_root(collection_surface, collection_id, "collection-bs-c");
log("folder-history: PASS");
