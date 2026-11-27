use super::*;
use crate::gamepad::{PadAxis, PadButton, PadEvent};
use crate::ring_shortcut::RingShortcutContext;

impl Harness {
    /// Real physical handoff + production semantic dispatcher. Omit only the
    /// navigation tail, so a replacement worker cannot complete before capture.
    fn pad(&mut self, events: &[PadEvent]) {
        let _foreground = crate::test_native_focus::scoped(1, 1, true);
        self.env.settings.gamepad_enabled = true;
        for event in events {
            self.env.gamepad.push_for_test(*event);
        }
        self.ctx
            .run_pass(
                egui::RawInput {
                    focused: true,
                    ..Default::default()
                },
                |ctx| {
                    self.env.input_update_begin(ctx);
                    assert!(self.env.input_root_keyboard(ctx).0.is_none());
                    // These are the existing admission predicates. Outcome's
                    // dispatch_allowed field is private to gamepad_input.
                    assert!(!self.env.sidecar_restore_active());
                    assert!(!self.env.common_modal_dialog_open());
                    assert!(!self.env.remote_session_blocks_local_control());
                    assert!(!self.env.shortcuts_blocked_by_text_input(ctx));
                    assert!(!ctx.any_popup_open());
                    let batch = self.env.sample_gamepad_input_with_logic_for_test(ctx);
                    let outcome = self.env.dispatch_gamepad_batch(ctx, batch);
                    assert!(outcome.nav.is_none());
                },
            )
            .drop_without_applying_deltas();
    }

    fn tap_pad(&mut self, button: PadButton) {
        self.pad(&[PadEvent::ButtonPressed(button)]);
        self.pad(&[PadEvent::ButtonReleased(button)]);
    }

    fn open_picker(&mut self) {
        self.tap_pad(PadButton::West);
        assert_eq!(
            self.env
                .ring_picker
                .as_ref()
                .expect("X opens picker")
                .context,
            RingShortcutContext::Grid
        );
    }

    fn picker_row(&mut self, row: usize) {
        for _ in 0..5 {
            if self.env.ring_picker.as_ref().unwrap().row == row {
                return;
            }
            self.tap_pad(PadButton::DPadDown);
        }
        assert_eq!(
            self.env.ring_picker.as_ref().unwrap().row,
            row,
            "row navigation must be admitted"
        );
    }
}

fn media_fixture() -> Harness {
    let mut h = Harness::images();
    let folder = h.env.tmp.path().join("duration-media");
    std::fs::create_dir(&folder).unwrap();
    // Small valid PCM files; the duration cache is seeded exactly as duration_762.
    // No media is played and no native presenter is created by these tests.
    for name in ["A.wav", "B.wav", "C.wav"] {
        write_wav(&folder.join(name));
    }
    h.env.settings.sort_order = SortOrder::FileName;
    h.env.load_folder_and_finish_for_test(folder);
    h.settle();
    assert_eq!(h.names(), ["A.wav", "B.wav", "C.wav"]);
    for (idx, duration) in [(0, 20.0), (1, 30.0), (2, 10.0)] {
        let key = h.env.details_lazy_cache_key(idx).unwrap();
        let (source_mtime, source_size) = h.env.image_metas[idx].unwrap();
        h.env.details_lazy_meta.insert(
            key,
            DetailsLazyMeta {
                source_mtime,
                source_size,
                media: DetailsMediaMeta::Read(DetailsVideoProbe {
                    duration_secs: Some(duration),
                    dims: None,
                    codec: None,
                }),
                ..Default::default()
            },
        );
    }
    h
}

#[test]
fn sort_hold_838_picker_descending_replaces_ascending_request_and_order() {
    let mut h = media_fixture();
    let a = request_sort(&mut h.env, SortOrder::DurationAsc);
    let generation = h.env.items_generation;
    h.open_picker();
    h.picker_row(1); // GridSortOrder
    // DurationDesc immediately follows DurationAsc in the released picker.
    h.pad(&[PadEvent::ButtonPressed(PadButton::DPadRight)]);
    assert_eq!(
        h.env.ring_picker.as_ref().unwrap().grid_sort_choice,
        crate::app::FolderSortChoice::Standard(SortOrder::DurationDesc)
    );
    assert_eq!(h.env.settings.sort_order, SortOrder::DurationDesc);
    let b = HeldScan::capture(&mut h.env);
    a.assert_retired();
    let FolderOpenScanPurpose::CurrentViewOrderRefresh { order, .. } =
        &h.env.folder_pane_open_pending.as_ref().unwrap().purpose
    else {
        panic!("replacement sort")
    };
    assert_eq!(order.sort_order, SortOrder::DurationDesc);
    h.pad(&[PadEvent::ButtonReleased(PadButton::DPadRight)]);
    assert_eq!(
        h.env.items_generation, generation,
        "A must not be installed"
    );
    // Leave the picker open: the production adoption/restore path closes it.
    b.release();
    h.settle();
    assert_eq!(h.env.settings.sort_order, SortOrder::DurationDesc);
    assert_eq!(h.names(), ["B.wav", "A.wav", "C.wav"]);
    assert!(h.env.ring_picker.is_none());
    assert!(h.env.grid_item_input_allowed());
}

#[test]
fn sort_hold_838_picker_can_open_during_sort_wait() {
    let mut h = Harness::images();
    let _held = request_sort(&mut h.env, SortOrder::DurationAsc);
    h.open_picker();
    assert!(h.env.folder_pane_open_pending.is_some());
}

#[test]
fn sort_worker_849_picker_name_change_uses_same_worker_wait() {
    let mut h = Harness::images();
    h.open_picker();
    h.picker_row(1);
    // FileNameDesc -> Numeric is a real non-duration picker edit.
    let rows = h.env.items.clone();
    let generation = h.env.items_generation;
    h.pad(&[PadEvent::ButtonPressed(PadButton::DPadRight)]);
    assert_eq!(h.env.settings.sort_order, SortOrder::Numeric);
    let held = HeldScan::capture(&mut h.env);
    assert!(h.env.view_order_refresh_scan_pending());
    assert!(!h.env.grid_item_input_allowed());
    h.pad(&[PadEvent::ButtonReleased(PadButton::DPadRight)]);
    assert_eq!(h.env.items, rows);
    assert_eq!(h.env.items_generation, generation);
    held.release();
    h.settle();
    assert_eq!(h.names(), ["B.png", "C.png", "Z.wav"]);
    assert_eq!(h.env.selected, Some(1));
    assert!(h.env.grid_item_input_allowed());
}

#[test]
fn sort_hold_838_left_stick_still_moves_folder_tree_during_sort_wait() {
    let mut h = Harness::images();
    let folder = h.env.current_folder.clone().unwrap();
    let leaf = folder.join("tree-leaf");
    std::fs::create_dir(&leaf).unwrap();
    h.env.settings.folder_tree_pane_visible = true;
    h.env.sync_folder_pane_state(&h.ctx);
    h.env.folder_pane.set_cursor(leaf.clone());
    h.env.folder_pane.set_focus_tree();
    assert_eq!(h.env.folder_pane.cursor_nav_target_if_moved(), Some(leaf));
    let selected = h.env.selected;
    // Restore ends the physical session and requires a neutral crossing. The
    // analog gate reads physical axes (already changed by drain), so the first
    // non-neutral event cannot itself release that gate. A real neutral poll
    // also clears left_stick_next_step; no elapsed-time sleep is needed.
    h.pad(&[]);
    assert!(!h.env.gamepad.state.directional_neutral_required());
    assert!(h.env.folder_pane_blocks_grid_keyboard());
    let _held = request_sort(&mut h.env, SortOrder::DurationAsc);
    assert!(h.env.folder_pane_blocks_grid_keyboard());
    h.pad(&[PadEvent::AxisChanged(PadAxis::LeftX, -0.9)]);
    h.pad(&[PadEvent::AxisChanged(PadAxis::LeftX, 0.0)]);
    // Left on an unexpanded leaf moves to its parent, which is the active folder.
    assert_eq!(h.env.folder_pane.cursor_nav_target_if_moved(), None);
    assert_eq!(h.env.selected, selected, "tree input must not leak to grid");
    assert!(h.env.folder_pane_open_pending.is_some());
}

#[test]
fn sort_hold_838_picker_non_item_previews_remain_available() {
    let mut h = Harness::images();
    let _held = request_sort(&mut h.env, SortOrder::DurationAsc);
    h.open_picker();
    let cols = h.env.settings.grid_cols;
    h.tap_pad(PadButton::DPadRight);
    assert_eq!(h.env.settings.grid_cols, cols + 1);
    h.picker_row(2); // GridThumbAspect
    let old_aspect = (
        h.env.settings.thumb_aspect_auto,
        h.env.settings.thumb_aspect,
    );
    h.tap_pad(PadButton::DPadRight);
    assert_ne!(
        (
            h.env.settings.thumb_aspect_auto,
            h.env.settings.thumb_aspect
        ),
        old_aspect
    );
    h.picker_row(4); // ContainerRating
    let key = crate::adjustment_db::normalize_path(h.env.current_folder.as_ref().unwrap());
    let before = h.env.rating_db.as_ref().unwrap().get(&key);
    h.tap_pad(PadButton::DPadRight);
    assert_eq!(
        h.env.rating_db.as_ref().unwrap().get(&key),
        (before + 1) % 6
    );
    assert!(h.env.folder_pane_open_pending.is_some());
}

fn blocked_item_rating(list: bool, session_end: bool) {
    let mut h = Harness::images();
    h.open_picker();
    h.picker_row(3); // ItemRating
    let before = h.env.ring_picker.as_ref().unwrap().clone();
    assert_eq!(
        before.original.item_rating_records.len(),
        1,
        "real eligible item"
    );
    let key = before.original.item_rating_records[0].path_key.clone();
    let rating = h.env.rating_db.as_ref().unwrap().get(&key);
    let generation = h.env.rating_session_write_generation;
    let undo = h.env.meta_undo.undo_len();
    if list {
        h.tap_pad(PadButton::South); // Open list before the wait; selecting later is the input under test.
        assert!(h.env.ring_picker.as_ref().unwrap().drill.is_some());
    }
    let _held = request_sort(&mut h.env, SortOrder::DurationAsc);
    if list {
        h.tap_pad(PadButton::DPadDown);
        h.tap_pad(PadButton::South); // Goes through set_ring_picker_row_value_selection.
    } else {
        h.tap_pad(PadButton::DPadRight); // Goes through change_ring_picker_value.
    }
    let after_input = h.env.ring_picker.as_ref().unwrap().clone();
    let rating_after_input = h.env.rating_db.as_ref().unwrap().get(&key);
    if session_end {
        h.pad(&[PadEvent::Disconnected]);
    } else {
        h.tap_pad(PadButton::East); // Ordinary confirmation, not the no-commit modal terminal.
    }
    assert!(h.env.ring_picker.is_none());
    assert_eq!(
        h.env.rating_db.as_ref().unwrap().get(&key),
        rating,
        "confirmation cannot write a blocked draft"
    );
    assert_eq!(
        rating_after_input, rating,
        "preview cannot write during the wait"
    );
    assert_eq!(
        after_input.item_rating, before.item_rating,
        "blocked input cannot change the draft"
    );
    assert_eq!(
        after_input.dirty_rows, before.dirty_rows,
        "blocked input cannot dirty the row"
    );
    assert_eq!(h.env.rating_session_write_generation, generation);
    assert_eq!(h.env.meta_undo.undo_len(), undo);
}

#[test]
fn sort_hold_838_item_rating_direction_and_confirm_do_not_write() {
    blocked_item_rating(false, false);
}

#[test]
fn sort_hold_838_item_rating_direction_and_session_end_do_not_write() {
    blocked_item_rating(false, true);
}

#[test]
fn sort_hold_838_item_rating_list_and_confirm_do_not_write() {
    blocked_item_rating(true, false);
}

#[test]
fn sort_hold_838_item_rating_list_and_session_end_do_not_write() {
    blocked_item_rating(true, true);
}

fn earlier_preview_remains_undoable(session_end: bool) {
    let mut h = Harness::images();
    h.open_picker();
    h.picker_row(3);
    let key = h
        .env
        .ring_picker
        .as_ref()
        .unwrap()
        .original
        .item_rating_records[0]
        .path_key
        .clone();
    let before = h.env.rating_db.as_ref().unwrap().get(&key);
    h.tap_pad(PadButton::DPadRight);
    let preview = h.env.rating_db.as_ref().unwrap().get(&key);
    assert_ne!(
        preview, before,
        "successful durable preview before the wait"
    );
    let undo = h.env.meta_undo.undo_len();
    let _held = request_sort(&mut h.env, SortOrder::DurationAsc);
    if session_end {
        h.pad(&[PadEvent::Disconnected]);
    } else {
        h.tap_pad(PadButton::East);
    }
    assert!(h.env.ring_picker.is_none());
    assert_eq!(h.env.rating_db.as_ref().unwrap().get(&key), preview);
    assert_eq!(h.env.meta_undo.undo_len(), undo + 1);
    // Exercise the recorded Undo recipe, not a blocked Ctrl+Z input. Ordinary
    // physical-list adoption already clears Undo; preserving it past adoption
    // would be a separate change to the released reload behavior.
    h.env.apply_meta_undo();
    assert_eq!(h.env.rating_db.as_ref().unwrap().get(&key), before);
    assert_eq!(h.env.meta_undo.redo_len(), 1);
}

#[test]
fn sort_hold_838_pre_wait_rating_preview_confirm_preserves_undo() {
    earlier_preview_remains_undoable(false);
}

#[test]
fn sort_hold_838_pre_wait_rating_preview_session_end_preserves_undo() {
    earlier_preview_remains_undoable(true);
}

fn direct_replacement_blocks_item_rating(list: bool, session_end: bool) {
    let mut h = Harness::images();
    let sort = request_sort(&mut h.env, SortOrder::DurationAsc);
    h.open_picker(); // Opened under the refresh-only exception, before replacement.
    h.picker_row(3); // ItemRating
    let before = h.env.ring_picker.as_ref().unwrap().clone();
    assert_eq!(before.context, RingShortcutContext::Grid);
    assert_eq!(
        before.original.item_rating_records.len(),
        1,
        "eligible item"
    );
    let key = before.original.item_rating_records[0].path_key.clone();
    let rating = h.env.rating_db.as_ref().unwrap().get(&key);
    let write_generation = h.env.rating_session_write_generation;
    let undo = h.env.meta_undo.undo_len();
    let source = h.env.current_folder.clone();
    let rows = h.env.items.clone();
    let generation = h.env.items_generation;
    let destination = h.env.tmp.path().join("rating-direct-destination");
    std::fs::create_dir(&destination).unwrap();
    h.env.open_direct_navigation_target_at_stage(
        destination,
        None,
        OpenRequestOwner::Navigation,
        None,
        None,
        StartupListIntent::ExplicitList,
        MainListRequestStage::Admission,
        DirectoryScanRoute::MainWorker,
    );
    let direct = HeldScan::capture(&mut h.env);
    sort.assert_retired();
    assert!(h.env.direct_navigation_scan_pending());
    assert!(!h.env.view_order_refresh_scan_pending());
    assert!(!h.env.grid_item_input_allowed());
    let retained = h
        .env
        .ring_picker
        .as_ref()
        .expect("direct admission keeps picker");
    assert_eq!(retained.context, before.context);
    assert_eq!(retained.owner, before.owner);
    assert_eq!(retained.anchor, before.anchor);

    if list {
        h.tap_pad(PadButton::South);
        assert!(h.env.ring_picker.as_ref().unwrap().drill.is_some());
        h.tap_pad(PadButton::DPadDown);
        assert_ne!(
            h.env
                .ring_picker
                .as_ref()
                .unwrap()
                .drill
                .as_ref()
                .unwrap()
                .selected,
            usize::from(before.item_rating),
            "the list must actually select a different candidate",
        );
        h.tap_pad(PadButton::South);
    } else {
        h.tap_pad(PadButton::DPadRight);
    }
    let after_input = h.env.ring_picker.as_ref().unwrap().clone();
    let rating_after_input = h.env.rating_db.as_ref().unwrap().get(&key);
    if session_end {
        h.pad(&[PadEvent::Disconnected]);
    } else {
        h.tap_pad(PadButton::East);
    }
    assert!(
        h.env.ring_picker.is_none(),
        "ordinary terminal must still close picker"
    );
    assert!(h.env.direct_navigation_scan_pending());
    assert!(!direct.cancel.load(Ordering::Relaxed));
    assert_eq!(h.env.current_folder, source);
    assert_eq!(h.env.items, rows);
    assert_eq!(h.env.items_generation, generation);
    // Keep both terminals reachable before checking the draft: in the reviewed
    // implementation preview is blocked, but finalize writes the changed draft.
    assert_eq!(
        h.env.rating_db.as_ref().unwrap().get(&key),
        rating,
        "direct-wait confirmation cannot write an old-list rating"
    );
    assert_eq!(
        rating_after_input, rating,
        "direct-wait preview cannot write"
    );
    assert_eq!(
        after_input.item_rating, before.item_rating,
        "blocked draft must stay unchanged"
    );
    assert_eq!(
        after_input.dirty_rows, before.dirty_rows,
        "blocked row must not become dirty"
    );
    assert_eq!(h.env.rating_session_write_generation, write_generation);
    assert_eq!(h.env.meta_undo.undo_len(), undo);
}

#[test]
fn sort_hold_838_direct_replacement_item_rating_direction_and_confirm_do_not_write() {
    direct_replacement_blocks_item_rating(false, false);
}

#[test]
fn sort_hold_838_direct_replacement_item_rating_direction_and_session_end_do_not_write() {
    direct_replacement_blocks_item_rating(false, true);
}

#[test]
fn sort_hold_838_direct_replacement_item_rating_list_and_confirm_do_not_write() {
    direct_replacement_blocks_item_rating(true, false);
}

#[test]
fn sort_hold_838_direct_replacement_item_rating_list_and_session_end_do_not_write() {
    direct_replacement_blocks_item_rating(true, true);
}
