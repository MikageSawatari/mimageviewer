// Real Windows clipboard and real OS Ctrl+V. Run only after interactive approval.
// Content/server/file inspection is owned by the Rhai worker; app state is projected.
fn monitors(on) {
    clipboard_capture_smoke(if on { "monitors_on" } else { "monitors_off" });
    wait_until(|s| s.clipboard_capture.image_enabled == on
        && s.clipboard_capture.html_enabled == on
        && (!on || s.clipboard_capture.monitor_ready), 30000);
}

fn wait_files(manual_count, capture_count) {
    wait_until(|s| { let f = clipboard_files();
        f.manual_count == manual_count && f.capture_count == capture_count }, 30000);
    log("clipboard files: " + clipboard_files().to_debug());
}

fn settled(sequence) {
    wait_until(|s| s.clipboard_capture.automatic_sequence == sequence, 30000);
}

fn no_popup_since(before, label) {
    let c = snapshot().clipboard_capture;
    if c.popup_visible || c.popup_shows != before.popup_shows
        || c.popup_requests != before.popup_requests || c.save_requests != before.save_requests || c.dialog_open {
        fail(label + " unexpectedly displayed popup/dialog");
    }
}

wait_until(|s| !s.startup_open_pending && !s.is_fullscreen && s.grid_surface == "Folder"
    && s.item_names == ["seed.png"] && s.target_registered && s.focused, 30000);
select_root();
monitors(false);
let current_folder = snapshot().current_folder_path;
wait_files(1, 0);

clipboard_fixture("html");
clipboard_paste();
wait_until(|s| s.clipboard_capture.dialog_open && s.clipboard_capture.fetch_complete
    && s.clipboard_capture.selected_count == 1, 30000);
if snapshot().clipboard_capture.dialog_destination != current_folder {
    fail("Ctrl+V HTML destination differs from current folder");
}
capture("clipboard-html-selection");
click_widget("選んだ 1 枚を保存");
wait_until(|s| !s.clipboard_capture.dialog_open, 30000);
wait_files(2, 0);

clipboard_fixture("png");
clipboard_paste();
wait_files(3, 0);
if clipboard_files().manual_matching_png != 3 {
    fail("manual HTML/PNG saved bytes differ from loopback PNG fixture");
}
capture("clipboard-manual-png");

clipboard_fixture("hdrop");
clipboard_paste();
wait_files(4, 0);
if !clipboard_files().shell_copy_matches {
    fail("CF_HDROP did not copy source via existing Shell paste");
}
capture("clipboard-shell-copy");

monitors(true);
let before = snapshot().clipboard_capture;
let sequence = clipboard_fixture("paragraph");
settled(sequence);
no_popup_since(before, "paragraph HTML");
wait_files(4, 0);
capture("clipboard-paragraph-ignored");

let before = snapshot().clipboard_capture;
let sequence = clipboard_fixture("office");
settled(sequence);
no_popup_since(before, "Office composite");
if snapshot().clipboard_capture.open_count != before.open_count {
    fail("mIV opened Office clipboard content");
}
wait_files(4, 0);
capture("clipboard-office-ignored");

let before = snapshot().clipboard_capture;
let result = clipboard_excel_write(20);
if result.successful_second_opens != 20 { fail("Excel second OpenClipboard failure"); }
settled(result.sequence);
no_popup_since(before, "Excel two-phase writer");
if snapshot().clipboard_capture.open_count != before.open_count {
    fail("mIV opened an Excel two-phase Office clipboard");
}
wait_files(4, 0);
log("Excel two-phase writer: " + result.to_debug());
capture("clipboard-excel-two-phase");

let sequence = clipboard_fixture("png");
settled(sequence);
wait_files(4, 1);
if clipboard_files().capture_matching_png != 1 {
    fail("S1 PNG was not saved exactly once with original bytes");
}
capture("clipboard-s1-png");
monitors(false);
log("ClipboardCapture passed; real clipboard was overwritten with disposable PNG content.");
