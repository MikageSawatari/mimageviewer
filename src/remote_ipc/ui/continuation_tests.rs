//! Headless service/display boundary regressions, not a 0.37 logic integration test.
use super::super::session::RemoteControlPhase;
use super::*;

fn acquire(handle: &SessionHandle) {
    assert_eq!(
        handle
            .acquire(mimageviewer_ipc::SessionAcquireRequest {
                client_id: "continuation".into(),
                peer: mimageviewer_ipc::SessionPeerInfo {
                    connection_kind: SessionConnectionKind::Direct,
                    device_name: None,
                },
            })
            .status,
        SessionStatus::Active
    );
}

fn fixture() -> (crate::app::AppTestEnvForTest, SessionHandle, egui::Context) {
    let mut app = crate::app::setup_app_for_test();
    // These tests supply terminal workers explicitly, independently of startup I/O.
    drop(app.book_resume_writer.take());
    let handle = SessionHandle::new();
    app.set_remote_session_handle(handle.clone());
    (app, handle, egui::Context::default())
}

#[test]
fn egui037_continuation_remote_service_acquire_ack_and_return_do_not_consume_display() {
    let (mut app, handle, ctx) = fixture();
    let frame = ctx.cumulative_frame_nr();
    let lease = app.local_ai_activity_lease();
    acquire(&handle);
    for _ in 0..2 {
        app.service_remote_session(&ctx);
        assert_eq!(handle.snapshot().phase, RemoteControlPhase::AcquiringRemote);
    }
    drop(lease);
    app.service_remote_session(&ctx);
    assert_eq!(handle.snapshot().phase, RemoteControlPhase::RemoteActive);

    let owner = handle.owner_for_test("continuation");
    let operation = handle.begin_operation(&owner, "fake save".into()).unwrap();
    let (pending, reply) = ClaimedRemoteWrite::for_test(
        operation,
        RemoteWriteRequest::SetSortOrder {
            scope: mimageviewer_ipc::RemoteGridScope::Address {
                address: mimageviewer_ipc::RemoteAddress::file("C:/books"),
            },
            sort_order: "FileName".into(),
        },
    );
    let (worker, receiver) = std::sync::mpsc::channel();
    app.remote_session_ui
        .pending_reading_writes
        .push(PendingRemoteReadingWrite::ViewSetting { pending, receiver });
    app.service_remote_session(&ctx);
    assert!(reply.try_recv().is_err());
    handle.local_disconnect();
    app.service_remote_session(&ctx);
    assert_eq!(handle.snapshot().phase, RemoteControlPhase::DrainingRemote);
    worker.send(Ok(())).unwrap();
    app.service_remote_session(&ctx);
    assert!(matches!(
        reply.try_recv(),
        Ok(UiWriteOutcome::Write(RemoteWriteResponse::Success(_)))
    ));
    app.service_remote_session(&ctx);
    assert_eq!(handle.snapshot().phase, RemoteControlPhase::Local);
    let returned = handle.snapshot().control_return_sequence;
    assert!(app.remote_session_ui.pending_local_display.is_some());
    for _ in 0..3 {
        app.service_remote_session(&ctx);
    }
    assert_eq!(handle.snapshot().control_return_sequence, returned);
    assert_eq!(
        app.remote_session_ui
            .pending_local_display
            .as_ref()
            .unwrap()
            .control_return_sequence,
        returned
    );
    assert_eq!(ctx.cumulative_frame_nr(), frame);
    assert!(app.fullscreen_idx.is_none());
}

#[test]
fn egui037_continuation_remote_return_reacquire_late_ui_discards_old_slot() {
    for observe_reacquire in [false, true] {
        let (mut app, handle, ctx) = fixture();
        app.enter_reading_history();
        acquire(&handle);
        app.service_remote_session(&ctx);
        handle.local_disconnect();
        app.service_remote_session(&ctx);
        assert!(app.remote_session_ui.pending_local_display.is_some());
        let generation = app.items_generation;
        acquire(&handle);
        if observe_reacquire {
            app.service_remote_session(&ctx);
            assert!(app.remote_session_ui.pending_local_display.is_none());
        }
        app.continue_remote_local_display();
        assert!(app.remote_session_ui.pending_local_display.is_none());
        assert_eq!(app.items_generation, generation);
        assert!(app.fullscreen_idx.is_none());
    }
}

#[test]
fn egui037_continuation_remote_coalesced_return_preserves_acquire_owner() {
    let (mut app, handle, ctx) = fixture();
    acquire(&handle);
    app.service_remote_session(&ctx);
    handle.local_disconnect();
    assert!(handle.complete_app_drain(handle.snapshot().generation));
    acquire(&handle);
    app.service_remote_session(&ctx);
    assert_eq!(handle.snapshot().phase, RemoteControlPhase::RemoteActive);
    assert!(app.remote_session_ui.local_ai_lease.is_some());
    assert!(app.remote_session_ui.pending_local_display.is_none());
}

#[test]
fn egui037_continuation_remote_resume_response_uses_receiver_without_ui() {
    let (mut app, _handle, ctx) = fixture();
    let (claimed, response) =
        super::super::session::ClaimedBookResumeRead::for_test("C:/book.cbz".into());
    let (worker, receiver) = std::sync::mpsc::channel();
    app.remote_session_ui
        .pending_resume_reads
        .push(PendingRemoteResumeRead {
            claimed,
            cutoff: 7,
            receiver,
        });
    app.service_remote_session(&ctx);
    assert!(response.try_recv().is_err());
    worker
        .send(crate::book_resume_db::PositionReadReply {
            cutoff: 7,
            completed_through: 7,
            position: Some(crate::book_resume_db::ReadingPosition {
                page: 3,
                identity: None,
            }),
            status: Ok(()),
        })
        .unwrap();
    app.service_remote_session(&ctx);
    assert_eq!(
        response.try_recv().unwrap(),
        Some(crate::book_resume_db::ReadingPosition {
            page: 3,
            identity: None
        })
    );
    assert!(app.remote_session_ui.pending_resume_reads.is_empty());
    assert_eq!(ctx.cumulative_frame_nr(), 0);
}

#[test]
fn egui037_continuation_remote_shared_bookmark_router_consumes_each_reply_once() {
    let (mut app, handle, ctx) = fixture();
    acquire(&handle);
    app.service_remote_session(&ctx);
    let owner = handle.owner_for_test("continuation");
    let (pending, response) = ClaimedRemoteWrite::for_test(
        handle
            .begin_operation(&owner, "bookmark ACK".into())
            .unwrap(),
        RemoteWriteRequest::SetSortOrder {
            scope: mimageviewer_ipc::RemoteGridScope::Address {
                address: mimageviewer_ipc::RemoteAddress::file("C:/books"),
            },
            sort_order: "FileName".into(),
        },
    );
    app.remote_session_ui.pending_bookmark_writes.insert(
        12,
        PendingRemoteBookmarkWrite {
            claimed: pending,
            kind: PendingRemoteBookmarkWriteKind::SetTitle,
            container_path: "C:/books".into(),
        },
    );
    app.book_bookmark_pending_requests.extend([12, 13]);
    let (service, worker) =
        crate::book_bookmarks::BookBookmarkService::with_event_sender_for_test();
    app.book_bookmark_service = Some(service);
    worker
        .send(crate::book_bookmarks::BookBookmarkEvent::TitleUpdated {
            request_id: 12,
            result: Ok((42, None)),
        })
        .unwrap();
    worker
        .send(crate::book_bookmarks::BookBookmarkEvent::AllListed {
            request_id: 13,
            result: Ok(Vec::new()),
        })
        .unwrap();
    app.poll_book_bookmark_service_responses();
    assert!(matches!(
        response.try_recv(),
        Ok(UiWriteOutcome::Write(RemoteWriteResponse::Success(_)))
    ));
    assert!(app.remote_session_ui.pending_bookmark_writes.is_empty());
    assert!(!app.book_bookmark_pending_requests.contains(&12));
    assert!(!app.book_bookmark_pending_requests.contains(&13));
    app.poll_book_bookmark_service_responses();
    assert!(matches!(
        response.try_recv(),
        Err(std::sync::mpsc::TryRecvError::Disconnected)
    ));
}

#[test]
fn egui037_continuation_remote_normal_driver_retains_both_display_checkpoints() {
    // Exercise both old reload positions with a real synthetic-list install.
    for drain_before_poll in [false, true] {
        let (mut app, handle, ctx) = fixture();
        app.enter_reading_history();
        acquire(&handle);
        app.service_remote_session(&ctx);
        let before = app.items_generation;
        handle.local_disconnect();
        if drain_before_poll {
            assert!(handle.complete_app_drain(handle.snapshot().generation));
        }
        app.poll_remote_session(&ctx);
        assert!(
            app.items_generation > before,
            "normal return must actually reload its list"
        );
        let adopted = app.items_generation;
        assert!(app.remote_session_ui.pending_local_display.is_none());
        app.poll_remote_session(&ctx);
        assert_eq!(
            app.items_generation, adopted,
            "one return cannot reload twice"
        );
    }
    let source = include_str!("../ui.rs");
    let driver = source
        .split("pub(crate) fn poll_remote_session(")
        .nth(1)
        .unwrap()
        .split("/// Remote progress without")
        .next()
        .unwrap();
    let calls = [
        "service_remote_session_begin(ctx)",
        "continue_remote_local_display()",
        "service_remote_session_requests(ctx, &poll)",
        "continue_remote_local_display()",
        "service_remote_session_finish(ctx, &poll)",
    ];
    let mut remaining = driver;
    for call in calls {
        let position = remaining.find(call).expect(call);
        remaining = &remaining[position + call.len()..];
    }
    let phases = include_str!("../../app/update_phases.rs");
    assert!(
        phases
            .find("self.acknowledge_tray_resident_media_wake()")
            .unwrap()
            < phases.find("self.poll_remote_session(ctx)").unwrap()
    );
    assert!(
        phases.find("self.poll_remote_session(ctx)").unwrap()
            < phases.find("self.note_modal_block_transition()").unwrap()
    );
}

#[test]
fn egui037_continuation_remote_display_slot_is_consumed_once_after_return() {
    let (mut app, handle, ctx) = fixture();
    app.enter_reading_history();
    acquire(&handle);
    app.service_remote_session(&ctx);
    let before = app.items_generation;
    handle.local_disconnect();
    app.service_remote_session(&ctx);
    assert_eq!(
        app.items_generation, before,
        "service does not install the local list"
    );
    assert!(app.remote_session_ui.pending_local_display.is_some());
    app.continue_remote_local_display();
    assert!(
        app.items_generation > before,
        "UI actually adopts the reload"
    );
    let adopted = app.items_generation;
    assert!(app.remote_session_ui.pending_local_display.is_none());
    app.service_remote_session(&ctx);
    assert!(app.remote_session_ui.pending_local_display.is_none());
    app.continue_remote_local_display();
    assert_eq!(app.items_generation, adopted);
    assert_eq!(handle.snapshot().control_return_sequence, 1);
    assert!(app.fullscreen_idx.is_none());
}

#[test]
fn egui037_continuation_remote_old_return_identity_cannot_reload_after_later_return() {
    let (mut app, handle, ctx) = fixture();
    app.enter_reading_history();
    acquire(&handle);
    app.service_remote_session(&ctx);
    handle.local_disconnect();
    app.service_remote_session(&ctx);
    let old = app.remote_session_ui.pending_local_display.take().unwrap();
    acquire(&handle);
    app.service_remote_session(&ctx);
    handle.local_disconnect();
    app.service_remote_session(&ctx);
    let current = app.remote_session_ui.pending_local_display.take().unwrap();
    assert_ne!(old.acquisition_sequence, current.acquisition_sequence);
    assert_ne!(old.control_return_sequence, current.control_return_sequence);
    let before = app.items_generation;
    // A delayed consumer must not mistake a later Local phase for its old return.
    app.remote_session_ui.pending_local_display = Some(old);
    app.continue_remote_local_display();
    assert_eq!(app.items_generation, before);
    app.remote_session_ui.pending_local_display = Some(current);
    app.continue_remote_local_display();
    assert!(app.items_generation > before);
}
