use super::tests::Fixture;
use super::*;

fn start(
    connection: &Arc<Connection>,
    id: u64,
    path: &Path,
) -> (JoinHandle<()>, crossbeam_channel::Receiver<Reply>) {
    let (send, receive) = crossbeam_channel::unbounded();
    let worker = connection
        .request(
            Request {
                id,
                command: Command::OpenInViewer {
                    path: path.to_string_lossy().into_owned(),
                },
            },
            send,
        )
        .unwrap();
    (worker, receive)
}

#[test]
fn claimed_open_survives_deadline_cancel_and_permission_change_before_wire_send() {
    let mut fixture = Fixture::new();
    fixture.set_level(AgentLevel::ReadAndOpen);
    let now = Arc::new(AtomicUsize::new(0));
    let clock = now.clone();
    let base = Instant::now();
    Arc::get_mut(&mut fixture.handlers).unwrap().read_hooks = Some(Arc::new(ReadHooks {
        now: Arc::new(move || base + Duration::from_secs(clock.load(Ordering::Acquire) as u64)),
        execute: Box::new(|command, _| match command {
            Command::Hello { version, .. } => Ok(Payload::Hello { version }),
            _ => panic!("only handshake uses the read controller in this test"),
        }),
    }));
    let connection = fixture.connection();
    let queue = fixture.handlers.open.install(egui::Context::default());
    let (ready, queued) = std::sync::mpsc::channel();
    fixture.handlers.open.set_enqueue_observer(ready);
    let (worker, replies) = start(&connection, 101, fixture._dir.path());
    queued.recv_timeout(Duration::from_secs(10)).unwrap();
    let request = queue.try_recv().unwrap();
    assert!(request.try_claim());
    now.store(21, Ordering::Release);
    connection.disconnect();
    assert!(matches!(
        replies.recv_timeout(Duration::from_millis(75)),
        Err(crossbeam_channel::RecvTimeoutError::Timeout)
    ));
    request.complete(open::UiOpenOutcome::Admitted);
    let reply = replies.recv_timeout(Duration::from_secs(10)).unwrap();
    worker.join().unwrap();
    fixture.set_level(AgentLevel::ReadOnly);
    assert!(matches!(
        reply.wire().result,
        Ok(Payload::Opened(OpenResult {
            status: OpenStatus::Accepted,
            ..
        }))
    ));
}

#[test]
fn readonly_open_never_enters_ui_queue() {
    let fixture = Fixture::new();
    let connection = fixture.connection();
    let queue = fixture.handlers.open.install(egui::Context::default());
    let response = fixture.call(
        &connection,
        101,
        Command::OpenInViewer {
            path: fixture._dir.path().to_string_lossy().into_owned(),
        },
    );
    assert_eq!(response.result.unwrap_err().code, ErrorCode::Disabled);
    assert!(queue.try_recv().is_err());
}

#[test]
fn second_connection_open_is_busy_until_first_ui_reply() {
    let fixture = Fixture::new();
    fixture.set_level(AgentLevel::ReadAndOpen);
    let first = fixture.connection();
    let second = fixture.connection();
    let queue = fixture.handlers.open.install(egui::Context::default());
    let (ready, queued) = std::sync::mpsc::channel();
    fixture.handlers.open.set_enqueue_observer(ready);
    let (worker, replies) = start(&first, 101, fixture._dir.path());
    queued.recv_timeout(Duration::from_secs(10)).unwrap();
    let response = fixture.call(
        &second,
        102,
        Command::OpenInViewer {
            path: fixture._dir.path().to_string_lossy().into_owned(),
        },
    );
    assert_eq!(response.result.unwrap_err().code, ErrorCode::Busy);
    let request = queue.try_recv().unwrap();
    assert!(request.try_claim());
    request.complete(open::UiOpenOutcome::Admitted);
    assert!(matches!(
        replies
            .recv_timeout(Duration::from_secs(10))
            .unwrap()
            .wire()
            .result,
        Ok(Payload::Opened(_))
    ));
    worker.join().unwrap();
}

#[test]
fn open_validation_deadline_cancels_pending_and_retains_permits_until_worker_exit() {
    let mut fixture = Fixture::new();
    fixture.set_level(AgentLevel::ReadAndOpen);
    let now = Arc::new(AtomicUsize::new(0));
    let clock = now.clone();
    let base = Instant::now();
    Arc::get_mut(&mut fixture.handlers).unwrap().read_hooks = Some(Arc::new(ReadHooks {
        now: Arc::new(move || base + Duration::from_secs(clock.load(Ordering::Acquire) as u64)),
        execute: Box::new(|command, _| match command {
            Command::Hello { version, .. } => Ok(Payload::Hello { version }),
            _ => panic!("only handshake uses the read controller in this test"),
        }),
    }));
    let connection = fixture.connection();
    let queue = fixture.handlers.open.install(egui::Context::default());
    let (entered, started) = crossbeam_channel::unbounded();
    let (release, gate) = crossbeam_channel::unbounded();
    fixture
        .handlers
        .open
        .set_validation_hook(Arc::new(move |_| {
            entered.send(()).unwrap();
            gate.recv_timeout(Duration::from_secs(10)).unwrap();
        }));
    let (worker, replies) = start(&connection, 101, fixture._dir.path());
    started.recv_timeout(Duration::from_secs(10)).unwrap();
    now.store(20, Ordering::Release);
    assert_eq!(
        replies
            .recv_timeout(Duration::from_secs(10))
            .unwrap()
            .wire()
            .result
            .unwrap_err()
            .code,
        ErrorCode::Timeout
    );
    assert_eq!(fixture.handlers.active.load(Ordering::Acquire), 1);
    assert_eq!(
        fixture
            .handlers
            .open
            .execute(
                fixture._dir.path().to_string_lossy().into_owned(),
                &AtomicBool::new(false)
            )
            .unwrap_err()
            .code,
        ErrorCode::Busy
    );
    release.send(()).unwrap();
    worker.join().unwrap();
    assert_eq!(fixture.handlers.active.load(Ordering::Acquire), 0);
    assert!(queue.try_recv().is_err());
}
