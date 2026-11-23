//! §1.849: watch re-registration and queued refreshes follow the existing sort owner.
use super::*;
use crate::app::book_append_refresh::{FolderRefreshCause, FolderRefreshRequest};

fn watch_stamp(h: &Harness) -> (u64, u64) {
    let watch = h
        .env
        .current_folder_watch
        .as_ref()
        .expect("registered watch");
    (watch.epoch, watch.projection_key)
}

fn queue_refresh(h: &mut Harness, cause: FolderRefreshCause) {
    let folder = h.env.current_folder.clone().unwrap();
    h.env
        .retain_folder_refresh(FolderRefreshRequest { folder, cause });
}

fn assert_cause(actual: &FolderRefreshCause, expected: &FolderRefreshCause) {
    assert_eq!(
        std::mem::discriminant(actual),
        std::mem::discriminant(expected)
    );
    if let (FolderRefreshCause::BookAppend(actual), FolderRefreshCause::BookAppend(expected)) =
        (actual, expected)
    {
        assert_eq!(actual.operation(), expected.operation());
        assert_eq!(actual.files.root, expected.files.root);
        assert_eq!(actual.files.folder, expected.files.folder);
        assert_eq!(actual.files.book_name, expected.files.book_name);
        assert_eq!(actual.files.composition, expected.files.composition);
        assert_eq!(actual.files.created.len(), expected.files.created.len());
        for (actual, expected) in actual.files.created.iter().zip(&expected.files.created) {
            assert_eq!(actual.path, expected.path);
        }
    }
}

fn assert_queued(h: &Harness, expected: &FolderRefreshCause) {
    let request = h
        .env
        .folder_refresh_pending
        .as_ref()
        .expect("retained refresh");
    assert_eq!(Some(&request.folder), h.env.current_folder.as_ref());
    assert_cause(&request.cause, expected);
}

fn assert_watch_held(h: &mut Harness, old: (u64, u64)) {
    for _ in 0..3 {
        h.env.poll_current_folder_watch(&h.ctx);
        h.env.consume_folder_refresh_pending(&h.ctx);
        assert_eq!(watch_stamp(h), old, "sort keeps the old subscription");
        assert!(
            h.env.external_rescan_pending.is_none(),
            "no competing scan starts"
        );
        assert!(h.env.folder_pane_open_pending.is_some());
    }
}

fn adopt_sort_without_watch(h: &mut Harness, held: HeldScan) {
    let generation = h.env.items_generation;
    held.release();
    let ready = mutation_refresh_wait_for_order_scan(&mut h.env);
    mutation_refresh_resolve_order_scan(&mut h.env, ready);
    h.env.finish_page_identity_preparation_for_test();
    assert!(h.env.folder_pane_open_pending.is_none());
    assert!(h.env.items_generation > generation, "the sort was adopted");
}

fn assert_watch_rebuilt(h: &mut Harness, old: (u64, u64)) {
    let folder = h.env.current_folder.clone().unwrap();
    let key = h.env.current_folder_projection_key(&folder);
    assert_ne!(key, old.1, "fixture must change the projection key");
    h.env.poll_current_folder_watch(&h.ctx);
    let (epoch, actual_key) = watch_stamp(h);
    assert_ne!(
        epoch, old.0,
        "a new subscription starts after the sort wait"
    );
    assert_eq!(actual_key, key);
    let scan = h
        .env
        .external_rescan_pending
        .as_ref()
        .expect("confirmation scan");
    assert_eq!(scan.folder, folder);
    assert_eq!(scan.items_generation, h.env.items_generation);
    let proof = scan.proof.as_ref().expect("ordinary observation proof");
    assert_eq!(proof.projection_key, key);
    assert_eq!(proof.subscription.map(|(epoch, _)| epoch), Some(epoch));
}

#[test]
fn sort_worker_849_watch_rebuild_waits_for_sort_adoption() {
    for order in [
        SortOrder::FileName,
        SortOrder::DateAsc,
        SortOrder::DurationAsc,
    ] {
        let mut h = Harness::images();
        let old = watch_stamp(&h);
        assert!(h.env.external_rescan_pending.is_none());
        let held = request_sort(&mut h.env, order);
        assert_watch_held(&mut h, old);
        assert!(h.env.folder_refresh_pending.is_none());
        adopt_sort_without_watch(&mut h, held);
        assert_eq!(watch_stamp(&h), old);
        assert_watch_rebuilt(&mut h, old);
    }
}

#[test]
fn sort_worker_849_missing_watch_registration_waits_for_sort() {
    let mut h = Harness::images();
    let old = watch_stamp(&h);
    h.env.retire_passive_folder_observation();
    let held = request_sort(&mut h.env, SortOrder::FileName);
    h.env.poll_current_folder_watch(&h.ctx);
    assert!(h.env.current_folder_watch.is_none());
    assert!(h.env.external_rescan_pending.is_none());
    adopt_sort_without_watch(&mut h, held);
    assert_watch_rebuilt(&mut h, old);
}

#[test]
fn sort_worker_849_deferred_refresh_causes_survive_sort_adoption() {
    for cause in [
        FolderRefreshCause::External,
        FolderRefreshCause::Operation,
        FolderRefreshCause::BookAppendExternal,
    ] {
        let mut h = Harness::images();
        let old = watch_stamp(&h);
        let held = request_sort(&mut h.env, SortOrder::FileName);
        queue_refresh(&mut h, cause.clone());
        assert_watch_held(&mut h, old);
        assert_queued(&h, &cause);
        adopt_sort_without_watch(&mut h, held);
        assert_queued(&h, &cause);
        assert_watch_rebuilt(&mut h, old);
        h.env.consume_folder_refresh_pending(&h.ctx);
        assert!(h.env.folder_refresh_pending.is_none());
        assert_cause(
            &h.env.external_rescan_pending.as_ref().unwrap().cause,
            &cause,
        );
    }
}

#[test]
fn sort_worker_849_watch_and_refresh_follow_latest_sort_replacement() {
    let mut h = Harness::images();
    let old = watch_stamp(&h);
    let first = request_sort(&mut h.env, SortOrder::FileName);
    queue_refresh(&mut h, FolderRefreshCause::Operation);
    assert_watch_held(&mut h, old);
    let last = request_sort(&mut h.env, SortOrder::DateAsc);
    first.assert_retired();
    assert_watch_held(&mut h, old);
    assert_queued(&h, &FolderRefreshCause::Operation);
    adopt_sort_without_watch(&mut h, last);
    assert_watch_rebuilt(&mut h, old);
    h.env.consume_folder_refresh_pending(&h.ctx);
    assert!(h.env.folder_refresh_pending.is_none());
    assert_cause(
        &h.env.external_rescan_pending.as_ref().unwrap().cause,
        &FolderRefreshCause::Operation,
    );
}

#[test]
fn sort_worker_849_watch_and_refresh_resume_after_navigation_replacement() {
    let mut h = Harness::images();
    let destination = h.env.tmp.path().join("watch-destination");
    std::fs::create_dir(&destination).unwrap();
    let old = watch_stamp(&h);
    let held = request_sort(&mut h.env, SortOrder::FileName);
    queue_refresh(&mut h, FolderRefreshCause::Operation);
    assert_watch_held(&mut h, old);
    h.env.open_direct_navigation_target_at_stage(
        destination,
        None,
        OpenRequestOwner::Navigation,
        None,
        None,
        StartupListIntent::ExplicitList,
        MainListRequestStage::Admission,
        DirectoryScanRoute::Worker,
    );
    let direct = HeldScan::capture(&mut h.env);
    held.assert_retired();
    assert!(h.env.direct_navigation_scan_pending());
    let folder = h.env.current_folder.clone().unwrap();
    assert!(!h.env.current_view_order_refresh_scans_folder(&folder));
    assert_watch_rebuilt(&mut h, old);
    h.env.consume_folder_refresh_pending(&h.ctx);
    assert!(h.env.folder_refresh_pending.is_none());
    assert_cause(
        &h.env.external_rescan_pending.as_ref().unwrap().cause,
        &FolderRefreshCause::Operation,
    );
    drop(direct);
}

fn refresh_after_retirement(retire: impl FnOnce(&mut Harness, HeldScan)) {
    let mut h = Harness::images();
    let old = watch_stamp(&h);
    let held = request_sort(&mut h.env, SortOrder::FileName);
    queue_refresh(&mut h, FolderRefreshCause::Operation);
    assert_watch_held(&mut h, old);
    retire(&mut h, held);
    assert!(h.env.folder_pane_open_pending.is_none());
    assert_queued(&h, &FolderRefreshCause::Operation);
    assert_watch_rebuilt(&mut h, old);
    h.env.consume_folder_refresh_pending(&h.ctx);
    assert!(h.env.folder_refresh_pending.is_none());
    assert_cause(
        &h.env.external_rescan_pending.as_ref().unwrap().cause,
        &FolderRefreshCause::Operation,
    );
}

#[test]
fn sort_worker_849_watch_and_refresh_resume_after_sort_cancel() {
    refresh_after_retirement(|h, held| {
        h.env
            .cancel_folder_pane_open(PaneOpenRestoreExit::Abandoned);
        held.assert_retired();
    });
}

#[test]
fn sort_worker_849_watch_and_refresh_resume_after_sort_disconnect() {
    refresh_after_retirement(|h, held| {
        drop(held);
        assert!(h.env.poll_folder_pane_open(&h.ctx).is_none());
    });
}

#[test]
fn sort_worker_849_sort_wait_does_not_retain_other_folder_request() {
    let mut h = Harness::images();
    let old = watch_stamp(&h);
    let _held = request_sort(&mut h.env, SortOrder::FileName);
    let other = h.env.tmp.path().join("other-folder");
    assert!(!h.env.current_view_order_refresh_scans_folder(&other));
    h.env.retain_folder_refresh(FolderRefreshRequest {
        folder: other,
        cause: FolderRefreshCause::Operation,
    });
    h.env.consume_folder_refresh_pending(&h.ctx);
    assert!(
        h.env.folder_refresh_pending.is_none(),
        "old unrelated request is still discarded"
    );
    assert_watch_held(&mut h, old);
}

#[test]
fn sort_worker_849_queued_append_receipt_survives_sort_wait() {
    let mut h = Harness::images();
    let folder = h.env.current_folder.clone().unwrap();
    // A real non-listed output makes a nonempty, verifiable receipt while keeping
    // the three-row fixture. Register a fresh watch only after the write, so late
    // callbacks from this setup cannot compete with the sort under test.
    h.env.retire_passive_folder_observation();
    let mut builder = crate::book_append_undo::AppendReceiptBuilder::begin(
        folder.parent().unwrap(),
        &folder,
        "sort-hold-838".into(),
    )
    .unwrap();
    let output = folder.join("appended-note.txt");
    std::fs::write(&output, b"append receipt fixture").unwrap();
    builder.record(&output).unwrap();
    let files = builder.finish().unwrap();
    h.settle();
    assert_eq!(h.names(), ["C.png", "B.png", "Z.wav"]);
    let receipt = crate::book_append_undo::BookAppendUndoReceipt {
        files,
        operation_id: 1849,
        source_context: h.env.projected_viewer_context_id(),
        accepted_revision: h.env.book_append_undo_revision,
    };
    h.env.book_append_undo_receipt = Some(receipt.clone());
    let cause = FolderRefreshCause::BookAppend(receipt);
    let old = watch_stamp(&h);
    let held = request_sort(&mut h.env, SortOrder::FileName);
    queue_refresh(&mut h, cause.clone());
    assert_watch_held(&mut h, old);
    assert_queued(&h, &cause);
    adopt_sort_without_watch(&mut h, held);
    assert_queued(&h, &cause);
    assert_watch_rebuilt(&mut h, old);
    h.env.consume_folder_refresh_pending(&h.ctx);
    assert!(h.env.folder_refresh_pending.is_none());
    // Ordinary sort adoption already retires the Undo receipt. Existing consumption
    // must still deliver its general refresh instead of losing it or reviving Undo.
    assert_cause(
        &h.env.external_rescan_pending.as_ref().unwrap().cause,
        &FolderRefreshCause::BookAppendExternal,
    );
}

#[test]
fn sort_worker_849_inflight_registration_scan_defers_retry_until_sort_adoption() {
    let mut h = Harness::images();
    // Start the real initial registration/confirmation owner again. Hold only
    // its receive boundary, just like HeldScan; neither proof nor result is fabricated.
    h.env.retire_passive_folder_observation();
    h.env.poll_current_folder_watch(&h.ctx);
    let old = watch_stamp(&h);
    let pending = h
        .env
        .external_rescan_pending
        .as_mut()
        .expect("initial scan");
    let result = pending
        .rx
        .recv_timeout(DEADLINE)
        .expect("real observation completion");
    assert!(result.is_some());
    let old_cancel = pending.cancel.clone();
    let (tx, rx) = mpsc::channel();
    pending.rx = rx;
    let held = request_sort(&mut h.env, SortOrder::FileName);
    h.env.poll_current_folder_watch(&h.ctx);
    assert_eq!(watch_stamp(&h), old);
    let pending = h.env.external_rescan_pending.as_ref().unwrap();
    assert!(Arc::ptr_eq(&pending.cancel, &old_cancel));
    assert!(!pending.restart_requested);
    tx.send(result).unwrap();
    let generation = h.env.items_generation;
    h.env.poll_external_rescan(&h.ctx);
    assert!(
        h.env.external_rescan_pending.is_none(),
        "stale initial result was dropped"
    );
    assert_eq!(h.env.items_generation, generation);
    assert_queued(&h, &FolderRefreshCause::External);
    assert_watch_held(&mut h, old);
    assert_queued(&h, &FolderRefreshCause::External);
    adopt_sort_without_watch(&mut h, held);
    assert_watch_rebuilt(&mut h, old);
    h.env.consume_folder_refresh_pending(&h.ctx);
    assert!(h.env.folder_refresh_pending.is_none());
    assert!(h.env.external_rescan_pending.is_some());
}
