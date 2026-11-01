use super::*;

fn open_activation(request: &UiOpenRequest) -> &ActivationOpenPath {
    match &request.kind {
        UiRequestKind::Open(activation) => activation,
        _ => panic!("expected the unchanged Open request"),
    }
}

#[test]
fn not_installed_returns_not_ready_and_releases_permit() {
    let controller = OpenController::new(None);
    let cancel = AtomicBool::new(false);
    for _ in 0..2 {
        assert_eq!(
            controller
                .execute("C:\\sample.jpg".into(), &cancel)
                .unwrap_err()
                .code,
            ErrorCode::NotReady,
        );
    }
}

#[test]
fn malformed_paths_never_enter_ui_queue() {
    let controller = OpenController::new(None);
    let queue = controller.install(egui::Context::default());
    let cancel = AtomicBool::new(false);
    for path in [
        "relative.jpg",
        "C:\\images\\..\\sample.jpg",
        "C:\\images\\.\\sample.jpg",
        "C:\\images\\nul\0.jpg",
        "https://example.invalid/a.jpg",
    ] {
        let error = controller.execute(path.into(), &cancel).unwrap_err();
        assert_eq!(error.code, ErrorCode::BadRequest);
        assert!(!error.message.contains(path));
        assert!(matches!(queue.try_recv(), Err(mpsc::TryRecvError::Empty)));
    }
}

// The input contract intentionally accepts Windows absolute paths only. These
// tests use disposable real files to exercise the worker's existence check.
#[cfg(windows)]
mod filesystem {
    use super::*;
    use crate::remote_ipc::session::RemoteControlPhase;
    use mimageviewer_ipc::{SessionAcquireRequest, SessionConnectionKind, SessionPeerInfo};

    fn path(dir: &tempfile::TempDir) -> String {
        dir.path().to_string_lossy().into_owned()
    }

    fn installed(
        session: Option<SessionHandle>,
    ) -> (Arc<OpenController>, mpsc::Receiver<UiOpenRequest>) {
        let controller = OpenController::new(session);
        let queue = controller.install(egui::Context::default());
        (controller, queue)
    }

    fn start(
        controller: &Arc<OpenController>,
        path: String,
        cancel: &Arc<AtomicBool>,
        timeout: Duration,
    ) -> std::thread::JoinHandle<Result<OpenResult, AgentError>> {
        let controller = Arc::clone(controller);
        let cancel = Arc::clone(cancel);
        std::thread::spawn(move || controller.execute_with_timeout(path, &cancel, timeout))
    }

    fn acquire(session: &SessionHandle) {
        session.acquire(SessionAcquireRequest {
            client_id: "agent-open-test".into(),
            peer: SessionPeerInfo {
                connection_kind: SessionConnectionKind::Direct,
                device_name: None,
            },
        });
    }

    #[test]
    fn nonexistent_path_is_bad_request_without_parent_fallback() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir
            .path()
            .join("missing.jpg")
            .to_string_lossy()
            .into_owned();
        let (controller, queue) = installed(None);
        let error = controller
            .execute(missing.clone(), &AtomicBool::new(false))
            .unwrap_err();
        assert_eq!(error.code, ErrorCode::BadRequest);
        assert!(!error.message.contains(&missing));
        assert!(matches!(queue.try_recv(), Err(mpsc::TryRecvError::Empty)));
    }

    #[test]
    fn existing_file_and_directory_reach_ui_and_return_admitted_result() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("sample.jpg");
        std::fs::write(&file, b"fixture").unwrap();
        let (controller, queue) = installed(None);
        let cancel = Arc::new(AtomicBool::new(false));
        for requested in [path(&dir), file.to_string_lossy().into_owned()] {
            let worker = start(
                &controller,
                requested.clone(),
                &cancel,
                Duration::from_secs(2),
            );
            let request = queue.recv_timeout(Duration::from_secs(2)).unwrap();
            assert_eq!(open_activation(&request).path.to_string_lossy(), requested);
            assert_eq!(
                open_activation(&request).acquisition_sequence_at_receive,
                None
            );
            assert!(!open_activation(&request).remote_owned_at_receive);
            assert!(request.try_claim());
            request.complete(UiOpenOutcome::Admitted);
            let result = worker.join().unwrap().unwrap();
            assert_eq!(result.status, OpenStatus::Accepted);
            assert_eq!(result.message, "mIV で開く処理を始めました");
        }
    }

    #[test]
    fn pending_and_claimed_open_keep_the_single_slot_until_reply() {
        let dir = tempfile::tempdir().unwrap();
        let (controller, queue) = installed(None);
        let cancel = Arc::new(AtomicBool::new(false));
        let worker = start(&controller, path(&dir), &cancel, Duration::from_secs(2));
        let request = queue.recv_timeout(Duration::from_secs(2)).unwrap();
        assert_eq!(
            controller.execute(path(&dir), &cancel).unwrap_err().code,
            ErrorCode::Busy
        );
        assert!(request.try_claim());
        assert_eq!(
            controller.execute(path(&dir), &cancel).unwrap_err().code,
            ErrorCode::Busy
        );
        request.complete(UiOpenOutcome::Refused(AgentError::new(
            ErrorCode::Busy,
            "モーダル表示中は開けません",
        )));
        assert_eq!(
            worker.join().unwrap().unwrap_err().message,
            "モーダル表示中は開けません"
        );
        let worker = start(&controller, path(&dir), &cancel, Duration::from_secs(2));
        let request = queue.recv_timeout(Duration::from_secs(2)).unwrap();
        assert!(request.try_claim());
        request.complete(UiOpenOutcome::Admitted);
        assert!(worker.join().unwrap().is_ok());
    }

    #[test]
    fn cancellation_before_enqueue_never_reaches_ui() {
        let dir = tempfile::tempdir().unwrap();
        let (controller, queue) = installed(None);
        assert_eq!(
            controller
                .execute(path(&dir), &AtomicBool::new(true))
                .unwrap_err()
                .code,
            ErrorCode::Cancelled
        );
        assert!(matches!(queue.try_recv(), Err(mpsc::TryRecvError::Empty)));
    }

    #[test]
    fn cancelled_pending_request_is_skipped_when_ui_later_receives_it() {
        let dir = tempfile::tempdir().unwrap();
        let (controller, queue) = installed(None);
        let cancel = Arc::new(AtomicBool::new(false));
        let worker = start(&controller, path(&dir), &cancel, Duration::from_secs(2));
        let request = queue.recv_timeout(Duration::from_secs(2)).unwrap();
        cancel.store(true, Ordering::Release);
        assert_eq!(
            worker.join().unwrap().unwrap_err().code,
            ErrorCode::Cancelled
        );
        assert!(!request.try_claim());
    }

    #[test]
    fn externally_cancelled_dispatch_ends_wait_without_waiting_for_ui() {
        let dir = tempfile::tempdir().unwrap();
        let (controller, queue) = installed(None);
        let dispatch = Arc::new(UiRequestDispatch::pending());
        let worker_dispatch = Arc::clone(&dispatch);
        let worker_controller = Arc::clone(&controller);
        let requested = path(&dir);
        let (done, result) = mpsc::channel();
        let worker = std::thread::spawn(move || {
            done.send(worker_controller.execute_with_dispatch(
                requested,
                &AtomicBool::new(false),
                worker_dispatch,
            ))
            .unwrap();
        });
        let request = queue.recv_timeout(Duration::from_secs(2)).unwrap();
        assert!(dispatch.cancel_pending());
        // Keep the UI reply sender alive: external cancellation must not be
        // mistaken for a claim and wait on this unanswered request.
        assert_eq!(
            result
                .recv_timeout(Duration::from_secs(2))
                .unwrap()
                .unwrap_err()
                .code,
            ErrorCode::Cancelled
        );
        assert!(!request.try_claim());
        worker.join().unwrap();
    }

    #[test]
    fn externally_cancelled_validation_never_enqueues() {
        let dir = tempfile::tempdir().unwrap();
        let (controller, queue) = installed(None);
        let dispatch = Arc::new(UiRequestDispatch::pending());
        let (entered, entered_rx) = mpsc::channel();
        let (release, release_rx) = mpsc::channel();
        let release_rx = Mutex::new(release_rx);
        controller.set_validation_hook(Arc::new(move |_| {
            entered.send(()).unwrap();
            release_rx.lock().unwrap().recv().unwrap();
        }));
        let worker_controller = Arc::clone(&controller);
        let worker_dispatch = Arc::clone(&dispatch);
        let requested = path(&dir);
        let worker = std::thread::spawn(move || {
            worker_controller.execute_with_dispatch(
                requested,
                &AtomicBool::new(false),
                worker_dispatch,
            )
        });
        entered_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        assert!(dispatch.cancel_pending());
        release.send(()).unwrap();
        assert_eq!(
            worker.join().unwrap().unwrap_err().code,
            ErrorCode::Cancelled
        );
        assert!(matches!(queue.try_recv(), Err(mpsc::TryRecvError::Empty)));
    }

    #[test]
    fn timed_out_pending_request_is_skipped_when_ui_later_receives_it() {
        let dir = tempfile::tempdir().unwrap();
        let (controller, queue) = installed(None);
        let error = controller
            .execute_with_timeout(
                path(&dir),
                &AtomicBool::new(false),
                Duration::from_millis(1),
            )
            .unwrap_err();
        assert_eq!(error.code, ErrorCode::Timeout);
        assert_eq!(error.message, "mIV が応答しません");
        assert!(!queue.try_recv().unwrap().try_claim());
    }

    #[test]
    fn claimed_request_waits_past_deadline_and_cancel_for_ui_reply() {
        let dir = tempfile::tempdir().unwrap();
        let (controller, queue) = installed(None);
        let cancel = Arc::new(AtomicBool::new(false));
        let controller_worker = Arc::clone(&controller);
        let worker_cancel = Arc::clone(&cancel);
        let (done, result) = mpsc::channel();
        let requested = path(&dir);
        let worker = std::thread::spawn(move || {
            let value = controller_worker.execute_with_timeout(
                requested,
                &worker_cancel,
                Duration::from_millis(100),
            );
            done.send(value).unwrap();
        });
        let request = queue.recv_timeout(Duration::from_secs(2)).unwrap();
        assert!(request.try_claim());
        cancel.store(true, Ordering::Release);
        assert!(matches!(
            result.recv_timeout(Duration::from_millis(150)),
            Err(mpsc::RecvTimeoutError::Timeout)
        ));
        assert_eq!(
            controller
                .execute(path(&dir), &AtomicBool::new(false))
                .unwrap_err()
                .code,
            ErrorCode::Busy
        );
        request.complete(UiOpenOutcome::Admitted);
        assert_eq!(
            result
                .recv_timeout(Duration::from_secs(2))
                .unwrap()
                .unwrap()
                .status,
            OpenStatus::Accepted
        );
        worker.join().unwrap();
    }

    #[test]
    fn every_remote_phase_refuses_before_enqueue() {
        let dir = tempfile::tempdir().unwrap();
        for phase in [
            RemoteControlPhase::AcquiringRemote,
            RemoteControlPhase::RemoteActive,
            RemoteControlPhase::DrainingRemote,
        ] {
            let session = SessionHandle::new();
            acquire(&session);
            let generation = session.snapshot().generation;
            match phase {
                RemoteControlPhase::RemoteActive => assert!(session.finish_acquire(generation)),
                RemoteControlPhase::DrainingRemote => {
                    assert!(session.abort_acquire_barrier(generation))
                }
                _ => {}
            }
            assert_eq!(session.snapshot().phase, phase);
            let (controller, queue) = installed(Some(session.clone()));
            assert_eq!(
                controller
                    .execute(path(&dir), &AtomicBool::new(false))
                    .unwrap_err()
                    .code,
                ErrorCode::Busy
            );
            assert_eq!(session.snapshot().phase, phase);
            assert!(matches!(queue.try_recv(), Err(mpsc::TryRecvError::Empty)));
        }
    }

    #[test]
    fn receive_captures_same_session_sequence_without_mutating_it() {
        let dir = tempfile::tempdir().unwrap();
        let session = SessionHandle::new();
        acquire(&session);
        let generation = session.snapshot().generation;
        assert!(session.abort_acquire_barrier(generation));
        assert!(session.complete_app_drain(generation));
        let before = session.snapshot();
        assert!(before.acquisition_sequence > 0);
        assert_eq!(before.phase, RemoteControlPhase::Local);
        let (controller, queue) = installed(Some(session.clone()));
        let worker = start(
            &controller,
            path(&dir),
            &Arc::new(AtomicBool::new(false)),
            Duration::from_secs(2),
        );
        let request = queue.recv_timeout(Duration::from_secs(2)).unwrap();
        assert_eq!(
            open_activation(&request).acquisition_sequence_at_receive,
            Some(before.acquisition_sequence)
        );
        assert!(!open_activation(&request).remote_owned_at_receive);
        assert!(request.try_claim());
        request.complete(UiOpenOutcome::Admitted);
        assert!(worker.join().unwrap().is_ok());
        let after = session.snapshot();
        assert_eq!(after.phase, before.phase);
        assert_eq!(after.generation, before.generation);
        assert_eq!(after.acquisition_sequence, before.acquisition_sequence);
        assert_eq!(
            after.control_return_sequence,
            before.control_return_sequence
        );
        assert!(after.active.is_none());
    }
}
