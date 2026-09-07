// Multi-window PDF smoke scenario.
//
// This script intentionally names the S1 APIs before their Rust implementation
// lands. scripts/ui-smoke.ps1 must not run this scenario until S1 is complete.

log("multi-window-pdf: wait for the root grid");
select_root();
wait_until(|s|
    !s.is_fullscreen
        && s.target_registered
        && s.focused
        && s.items_len == 2
        && s.pending_thumbs == 0,
    30000
);

run_action("GridMoveFirst");
run_action("GridOpenSelected");
wait_until(|s|
    s.windows.len == 1
        && s.windows[0].media_kind == "pdf"
        && s.windows[0].paint_matches_current_page
        && s.windows[0].paint_revision > 0,
    30000
);

select_root();
run_action("GridMoveLast");
run_action("GridOpenSelected");
wait_until(|s|
    s.windows.len == 2
        && s.windows[0].window_id != s.windows[1].window_id
        && s.windows[0].context_serial != s.windows[1].context_serial
        && s.windows[0].item_identity != s.windows[1].item_identity
        && s.windows[0].paint_matches_current_page
        && s.windows[1].paint_matches_current_page,
    30000
);

let before = snapshot().windows;
let first = before[0];
let second = before[1];
select_window(first.window_id, first.context_serial);
run_action("FsPageNext");
wait_until(|s|
    s.windows[0].context_serial == first.context_serial
        && s.windows[0].page_index != first.page_index
        && s.windows[0].paint_matches_current_page
        && s.windows[0].paint_revision > first.paint_revision
        && s.windows[1].context_serial == second.context_serial
        && s.windows[1].page_index == second.page_index
        && s.windows[1].items_generation == second.items_generation,
    30000
);

log("multi-window-pdf: done");
