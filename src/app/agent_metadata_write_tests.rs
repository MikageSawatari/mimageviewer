//! Headless coverage of the real App write admission and queued CAS boundary.
use super::*;
use crate::agent_ipc::metadata_write::{PreparedRatings, PreparedTags, PreparedWrite};
use crate::agent_ipc::open::{OpenController, UiRequestKind};
use crate::app::agent_rating_write::PreparedRatingTarget;
use crate::app::tests::phase_c_support::{AppTestEnv, setup_app};
use crate::rating_db::{RatingItemKind, RatingMeta};
use crate::ui_request_dispatch::UiRequestDispatch;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::time::Duration;

fn install(app: &mut App, ctx: &egui::Context) -> Arc<OpenController> {
    app.initialized = true;
    app.indexer_init = IndexerInit::Unavailable;
    app.settings.agent_level = AgentLevel::Organize;
    app.install_agent_changes(crate::agent_ipc::changes::ChangeController::new(), ctx);
    let controller = OpenController::new(app.remote_session_handle().cloned());
    app.install_agent_open(controller.clone(), ctx);
    controller
}

fn prepared(path: &Path) -> PreparedWrite {
    PreparedWrite::Ratings(PreparedRatings {
        items: vec![PreparedItem {
            input: mimageviewer_agent_ipc::ItemRef {
                path: path.to_string_lossy().into_owned(),
                subresource: None,
            },
            target: Ok(PreparedRatingTarget {
                key: crate::adjustment_db::normalize_path(path),
                legacy_key: None,
                source: path.to_path_buf(),
                meta: RatingMeta::new(RatingItemKind::Image).with_source_path(path),
                xmp_target: None,
                compiled_book_page: false,
            }),
        }],
        stars: 4,
        client_name: "admission fixture".into(),
    })
}

fn request_kind(write: PreparedWrite) -> UiRequestKind {
    match write {
        PreparedWrite::Tags(write) => UiRequestKind::TagWrite(write),
        PreparedWrite::Ratings(write) => UiRequestKind::RatingWrite(write),
    }
}

fn append_receipt(app: &AppTestEnv) -> crate::book_append_undo::BookAppendUndoReceipt {
    let root = app.tmp.path().join("append-books");
    let folder = root.join("book");
    std::fs::create_dir_all(&folder).unwrap();
    let mut builder =
        crate::book_append_undo::AppendReceiptBuilder::begin(&root, &folder, "book".into())
            .unwrap();
    let page = folder.join("page.png");
    std::fs::write(&page, b"fixture").unwrap();
    builder.record(&page).unwrap();
    crate::book_append_undo::BookAppendUndoReceipt {
        files: builder.finish().unwrap(),
        operation_id: 1,
        source_context: app.projected_viewer_context_id(),
        accepted_revision: app.book_append_undo_revision,
    }
}

fn document_modal(app: &mut AppTestEnv, ctx: &egui::Context) {
    use crate::clipboard_capture::{
        CaptureIntent, SelectionSnapshot, data::CaptureTimestamp, html::HtmlCapture,
    };
    let snapshot = Arc::new(SelectionSnapshot {
        token: 1,
        html_epoch: 0,
        html: HtmlCapture {
            page_url: "https://example.invalid/agent-write-test".into(),
            candidates: vec!["data:image/png;base64,AA==".into()],
            omitted: 0,
        },
        intent: CaptureIntent::Manual {
            destination: app.tmp.path().join("paste"),
        },
        timestamp: CaptureTimestamp {
            stamp: "20261101-120000-000".into(),
            month: "2026-11".into(),
        },
    });
    app.handle_capture_selection(snapshot, ctx, |_, _| {
        panic!("manual fixture must not activate a window")
    });
}

#[test]
fn agent_write_every_busy_precheck_refuses_without_mutation() {
    for variant in 0..15 {
        let mut app = setup_app();
        let ctx = egui::Context::default();
        install(&mut app, &ctx);
        let path = app.tmp.path().join("page.png");
        let write = request_kind(prepared(&path));
        let receipt = append_receipt(&app);
        app.book_append_undo_receipt = Some(receipt.clone());
        let mut book_sender = None;
        let mut save_completion = None;
        let expected = match variant {
            0 => {
                app.initialized = false;
                WriteRefusal::Startup
            }
            1 => {
                app.indexer_init = IndexerInit::NotStarted;
                WriteRefusal::Startup
            }
            2 => {
                let (_, rx) = mpsc::channel();
                app.tag_maintenance_rx = Some(rx);
                WriteRefusal::TagMaintenance
            }
            3 => {
                book_sender = Some(app.hold_book_structure_for_test());
                WriteRefusal::BookOperation
            }
            4 => {
                let (_, rx) = mpsc::channel();
                app.book_op_pending = Some(crate::books::BookOpPending {
                    rx,
                    intent: crate::books::BookOpIntent::UndoAppend { receipt },
                });
                WriteRefusal::AppendUndo
            }
            5 => {
                app.current_folder = Some(app.tmp.path().to_path_buf());
                app.items_are_drive_list = false;
                app.current_folder_last_mtime = Some(std::time::SystemTime::UNIX_EPOCH);
                assert!(app.metadata_transfer_target().is_some());
                app.open_metadata_export_dialog();
                assert!(app.metadata_transfer.is_some());
                WriteRefusal::MetadataTransfer
            }
            6 => {
                app.current_folder = Some(app.tmp.path().to_path_buf());
                app.items_are_drive_list = false;
                app.current_folder_last_mtime = Some(std::time::SystemTime::UNIX_EPOCH);
                assert!(app.metadata_transfer_target().is_some());
                app.open_metadata_import_dialog();
                assert!(app.metadata_transfer.is_some());
                WriteRefusal::MetadataTransfer
            }
            7 => {
                let job = crate::rename_key_migration::PathMigrationJob::shell_rename(
                    path.clone(),
                    path.with_extension("jpg"),
                    false,
                );
                app.rename_migration_queue.push_back(job);
                WriteRefusal::RenameMigration
            }
            8 => {
                let job = crate::rename_key_migration::PathMigrationJob::shell_rename(
                    path.clone(),
                    path.with_extension("jpg"),
                    false,
                );
                app.rename_migration_boot_retry.push(job);
                WriteRefusal::RenameMigration
            }
            9 => {
                app.activate_sidecar_restore_modal_for_test(path.clone());
                WriteRefusal::SidecarRestore
            }
            10 => {
                app.composition_editor_operations.owner = Some(
                    crate::app::book_composition_editor::CompositionEditorOwner {
                        context_id: app.projected_viewer_context_id(),
                        scope: crate::book_scope::BookScopeKey::document(
                            &path.with_extension("cbz"),
                        )
                        .unwrap(),
                        request_id: 1,
                    },
                );
                WriteRefusal::Composition
            }
            11 => {
                save_completion = Some(app.sent_composition_save_for_lifecycle_test());
                WriteRefusal::Composition
            }
            12 => {
                document_modal(&mut app, &ctx);
                WriteRefusal::DocumentModal
            }
            13 => {
                let job = crate::rename_key_migration::PathMigrationJob::shell_rename(
                    path.clone(),
                    path.with_extension("jpg"),
                    false,
                );
                let (_, rx) = mpsc::channel();
                app.rename_migration_in_flight =
                    Some(crate::app::RenameMigrationInFlight::Generic { job, rx });
                WriteRefusal::RenameMigration
            }
            14 => {
                let pending = crate::metadata_cleanup::spawn_scan(
                    app.tmp.path().to_path_buf(),
                    app.tmp.path().join("books"),
                );
                pending.rx.recv_timeout(Duration::from_secs(10)).unwrap();
                app.metadata_cleanup_pending = Some(pending);
                WriteRefusal::MetadataTransfer
            }
            _ => unreachable!(),
        };
        let revision = app.book_append_undo_revision;
        let generation = app.rating_session_write_generation;
        let undo = app.meta_undo.undo_len();
        let toast = app
            .fs_feedback_toast
            .as_ref()
            .map(|(text, _, _)| text.clone());
        assert_eq!(
            app.agent_metadata_precheck(true),
            Some(expected),
            "variant {variant}"
        );
        assert_eq!(
            app.agent_metadata_precheck(false),
            if variant == 2 { None } else { Some(expected) }
        );
        // Tag maintenance blocks tags only. Every other reason also blocks ratings.
        let tag_write;
        let checked = if variant == 2 {
            tag_write = UiRequestKind::TagWrite(PreparedTags {
                items: Vec::new(),
                tags: vec!["tag".into()],
                add: true,
                client_name: "fixture".into(),
            });
            &tag_write
        } else {
            &write
        };
        let UiOpenOutcome::Refused(error) = app.admit_agent_metadata_write(checked) else {
            panic!("busy state must refuse variant {variant}")
        };
        assert_eq!(
            error.code,
            if expected == WriteRefusal::Startup {
                ErrorCode::NotReady
            } else {
                ErrorCode::Busy
            }
        );
        assert_eq!(app.rating_session_write_generation, generation);
        assert_eq!(app.book_append_undo_revision, revision);
        assert!(app.book_append_undo_receipt.is_some());
        assert_eq!(app.meta_undo.undo_len(), undo);
        assert_eq!(
            app.fs_feedback_toast
                .as_ref()
                .map(|(text, _, _)| text.clone()),
            toast
        );
        assert!(
            app.agent_changes
                .as_ref()
                .unwrap()
                .controller
                .records()
                .lock()
                .unwrap()
                .entries()
                .is_empty()
        );
        assert_eq!(
            app.rating_db
                .as_ref()
                .unwrap()
                .get(&crate::adjustment_db::normalize_path(&path)),
            0
        );
        // Retire fixtures through plain ownership drops; never run a native UI.
        app.book_op_pending = None;
        app.book_structural_op = None;
        app.rename_migration_queue.clear();
        app.rename_migration_boot_retry.clear();
        app.rename_migration_in_flight = None;
        app.metadata_cleanup_pending = None;
        app.composition_editor_operations.owner = None;
        if let Some(completion) = save_completion {
            completion(Ok(()));
        }
        drop(book_sender);
    }
}

struct QueuedWrite {
    cancel: Arc<AtomicBool>,
    dispatch: Arc<UiRequestDispatch>,
    worker: std::thread::JoinHandle<Result<UiOpenOutcome, AgentError>>,
}
fn enqueue(
    controller: &Arc<OpenController>,
    write: PreparedWrite,
    timeout: Duration,
) -> QueuedWrite {
    let (notify, ready) = mpsc::channel();
    controller.set_enqueue_observer(notify);
    let cancel = Arc::new(AtomicBool::new(false));
    let dispatch = Arc::new(UiRequestDispatch::pending());
    let (thread_controller, thread_cancel, thread_dispatch) =
        (controller.clone(), cancel.clone(), dispatch.clone());
    let worker = std::thread::spawn(move || {
        thread_controller.execute_prepared_for_test(write, &thread_cancel, thread_dispatch, timeout)
    });
    ready.recv_timeout(Duration::from_secs(10)).unwrap();
    QueuedWrite {
        cancel,
        dispatch,
        worker,
    }
}

#[test]
fn agent_write_queued_request_rechecks_permission_and_remote_acquisition() {
    for remote in [false, true] {
        let mut app = setup_app();
        let session = crate::remote_ipc::session::SessionHandle::new();
        app.set_remote_session_handle(session.clone());
        let ctx = egui::Context::default();
        let controller = install(&mut app, &ctx);
        let path = app.tmp.path().join("page.png");
        let queued = enqueue(&controller, prepared(&path), Duration::from_secs(5));
        if remote {
            session.acquire(mimageviewer_ipc::SessionAcquireRequest {
                client_id: "agent-write-admission".into(),
                peer: mimageviewer_ipc::SessionPeerInfo {
                    connection_kind: mimageviewer_ipc::SessionConnectionKind::Direct,
                    device_name: None,
                },
            });
        } else {
            app.settings.agent_level = AgentLevel::ReadAndOpen;
        }
        app.poll_agent_open_requests(&ctx);
        let UiOpenOutcome::Refused(error) = queued.worker.join().unwrap().unwrap() else {
            panic!("must recheck at UI consumption")
        };
        assert_eq!(
            error.code,
            if remote {
                ErrorCode::Busy
            } else {
                ErrorCode::Disabled
            }
        );
        assert_eq!(app.rating_session_write_generation, 0);
        assert_eq!(app.meta_undo.undo_len(), 0);
    }
}

#[test]
fn agent_write_cancelled_or_expired_queue_never_writes_later() {
    for cancelled in [false, true] {
        let mut app = setup_app();
        let ctx = egui::Context::default();
        let controller = install(&mut app, &ctx);
        let path = app.tmp.path().join("page.png");
        let queued = enqueue(
            &controller,
            prepared(&path),
            if cancelled {
                Duration::from_secs(5)
            } else {
                Duration::from_millis(50)
            },
        );
        if cancelled {
            queued.cancel.store(true, Ordering::Release);
        }
        let error = match queued.worker.join().unwrap() {
            Err(error) => error,
            _ => panic!("unclaimed queue must stop"),
        };
        assert_eq!(
            error.code,
            if cancelled {
                ErrorCode::Cancelled
            } else {
                ErrorCode::Timeout
            }
        );
        app.poll_agent_open_requests(&ctx);
        assert_eq!(app.rating_session_write_generation, 0);
        assert!(
            app.agent_changes
                .as_ref()
                .unwrap()
                .controller
                .records()
                .lock()
                .unwrap()
                .entries()
                .is_empty()
        );
    }
}

#[test]
fn agent_write_claimed_operation_finishes_after_cancellation_and_timeout() {
    let mut app = setup_app();
    let ctx = egui::Context::default();
    let controller = install(&mut app, &ctx);
    let path = app.tmp.path().join("page.png");
    let queued = enqueue(&controller, prepared(&path), Duration::from_millis(250));
    let request = app
        .agent_open_requests
        .as_ref()
        .unwrap()
        .try_recv()
        .unwrap();
    assert!(request.try_claim());
    queued.cancel.store(true, Ordering::Release);
    assert!(!queued.dispatch.cancel_pending());
    std::thread::sleep(Duration::from_millis(300));
    assert!(
        !queued.worker.is_finished(),
        "claimed worker must await the accepted operation"
    );
    let result = app.admit_agent_metadata_write(&request.kind);
    request.complete(result);
    assert!(matches!(
        queued.worker.join().unwrap().unwrap(),
        UiOpenOutcome::Written(Ok(Payload::RatingWrite(_)))
    ));
    assert_eq!(
        app.rating_db
            .as_ref()
            .unwrap()
            .get(&crate::adjustment_db::normalize_path(&path)),
        4
    );
    assert_eq!(app.meta_undo.undo_len(), 0);
    assert_eq!(
        app.agent_changes
            .as_ref()
            .unwrap()
            .controller
            .records()
            .lock()
            .unwrap()
            .entries()
            .len(),
        1
    );
}

#[test]
fn agent_write_and_open_share_the_same_single_request_permit() {
    let mut app = setup_app();
    let ctx = egui::Context::default();
    let controller = install(&mut app, &ctx);
    let path = app.tmp.path().join("page.png");
    let queued = enqueue(&controller, prepared(&path), Duration::from_secs(5));
    let error = controller
        .execute(
            app.tmp.path().to_string_lossy().into_owned(),
            &AtomicBool::new(false),
        )
        .unwrap_err();
    assert_eq!(error.code, ErrorCode::Busy);
    let other = PreparedWrite::Tags(PreparedTags {
        items: Vec::new(),
        tags: vec!["tag".into()],
        add: true,
        client_name: "fixture".into(),
    });
    let other_result = controller.execute_prepared_for_test(
        other,
        &AtomicBool::new(false),
        Arc::new(UiRequestDispatch::pending()),
        Duration::from_secs(5),
    );
    assert!(matches!(
        other_result,
        Err(AgentError {
            code: ErrorCode::Busy,
            ..
        })
    ));
    queued.cancel.store(true, Ordering::Release);
    assert!(matches!(
        queued.worker.join().unwrap(),
        Err(AgentError {
            code: ErrorCode::Cancelled,
            ..
        })
    ));
    app.poll_agent_open_requests(&ctx);
    assert_eq!(app.rating_session_write_generation, 0);
}

#[test]
fn agent_tag_admission_real_writer_finishes_changed_noop_and_failed_records() {
    use crate::agent_ipc::changes::{ChangeRowOutcome, ChangeRowResult};
    let mut app = setup_app();
    let ctx = egui::Context::default();
    install(&mut app, &ctx);
    let path = app.tmp.path().join("agent-tag-record.png");
    std::fs::write(&path, b"fixture").unwrap();
    let key = crate::tags_db::item_key_for_path(&path);
    let mut db = crate::tags_db::TagsDb::open().unwrap();
    db.apply_item_tag_delta(&key, &["original".into()], true)
        .unwrap();
    // Disposable DB triggers fail the real writer deterministically, after
    // it has read the existing tags. No product failure hook is required.
    let connection = rusqlite::Connection::open(app.tmp.path().join("tags.db")).unwrap();
    connection
        .execute_batch(
            "CREATE TRIGGER fail_agent_fixture BEFORE INSERT ON item_tags
         WHEN NEW.tag = 'blocked'
         BEGIN SELECT RAISE(ABORT, 'agent fixture rejection'); END;
         CREATE TRIGGER fail_agent_remove_fixture BEFORE DELETE ON item_tags
         WHEN OLD.tag = 'original'
         BEGIN SELECT RAISE(ABORT, 'agent fixture rejection'); END;",
        )
        .unwrap();
    drop(connection);
    let undo_before = app.meta_undo.undo_len();
    let cases = [
        ("extra", true, Some(ChangeRowOutcome::Changed)),
        ("extra", true, Some(ChangeRowOutcome::Unchanged)),
        ("blocked", true, None),
        ("extra", false, Some(ChangeRowOutcome::Changed)),
        ("extra", false, Some(ChangeRowOutcome::Unchanged)),
        ("original", false, None),
    ];
    for (iteration, (tag, add, expected)) in cases.into_iter().enumerate() {
        let before = db.display_tags_for_item(&key);
        let request = UiRequestKind::TagWrite(PreparedTags {
            items: vec![
                PreparedItem {
                    input: mimageviewer_agent_ipc::ItemRef {
                        path: "invalid input".into(),
                        subresource: None,
                    },
                    target: Err(MetadataSkipReason::InvalidPath),
                },
                PreparedItem {
                    input: mimageviewer_agent_ipc::ItemRef {
                        path: path.to_string_lossy().into_owned(),
                        subresource: None,
                    },
                    target: Ok(crate::tag_ops::TagTarget {
                        path: path.clone(),
                        tag_sidecar: None,
                    }),
                },
            ],
            tags: vec![tag.into()],
            add,
            client_name: "real tag admission fixture".into(),
        });
        let UiOpenOutcome::Written(Ok(Payload::TagWrite(reply))) =
            app.admit_agent_metadata_write(&request)
        else {
            panic!("tag write must be admitted")
        };
        assert!(
            reply
                .outcomes
                .contains(&MetadataItemOutcome::Queued { indices: vec![1] })
        );
        let controller = app.agent_changes.as_ref().unwrap().controller.clone();
        let pending = controller.records().lock().unwrap().entries()[0].clone();
        assert_eq!(pending.tool, if add { "add_tags" } else { "remove_tags" });
        assert_eq!(pending.rows[1].result, ChangeRowResult::Pending);
        assert!(pending.rows[1].before.is_empty());
        assert!(pending.rows[1].after.is_empty());
        assert_eq!(pending.counts.queued, 1);
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        loop {
            app.poll_tag_write_results();
            if !app
                .tag_write_handle
                .as_ref()
                .unwrap()
                .has_unconsumed_batch()
            {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "real tag writer completion"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
        let after = db.display_tags_for_item(&key);
        let completed = controller.records().lock().unwrap().entries()[0].clone();
        assert_eq!(completed.id, pending.id);
        assert_eq!(completed.tool, if add { "add_tags" } else { "remove_tags" });
        assert_eq!(completed.counts.queued, 0);
        assert_eq!(completed.counts.skipped, 1);
        assert_eq!(completed.rows[1].before, before.join(", "));
        assert_eq!(completed.rows[1].after, after.join(", "));
        match expected {
            Some(ChangeRowOutcome::Changed) => {
                assert_eq!(
                    completed.rows[1].result,
                    ChangeRowResult::Done(ChangeRowOutcome::Changed)
                );
                assert_eq!(completed.counts.changed, 1);
                assert_ne!(before, after);
            }
            Some(ChangeRowOutcome::Unchanged) => {
                assert_eq!(
                    completed.rows[1].result,
                    ChangeRowResult::Done(ChangeRowOutcome::Unchanged)
                );
                assert_eq!(completed.counts.unchanged, 1);
                assert_eq!(before, after);
            }
            None => {
                assert!(
                    matches!(&completed.rows[1].result, ChangeRowResult::Failed(error) if error.contains("agent fixture rejection"))
                );
                assert_eq!(completed.counts.failed, 1);
                assert_eq!(before, after);
            }
            Some(ChangeRowOutcome::Text(_)) => unreachable!(),
        }
        assert_eq!(app.tags_cache.get(&key), Some(&after));
        assert_eq!(app.meta_undo.undo_len(), undo_before);
        // Adoption mutates the current record, never an already adopted UI snapshot.
        assert_eq!(pending.rows[1].result, ChangeRowResult::Pending);
        assert_eq!(
            controller.records().lock().unwrap().entries().len(),
            iteration + 1
        );
    }
}
