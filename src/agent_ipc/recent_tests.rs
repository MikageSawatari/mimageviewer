use super::tests::Fixture;
use super::*;

fn hello(name: &str, version: &str, kind: RequestKind) -> Command {
    Command::Hello {
        version: AGENT_PROTOCOL_VERSION,
        client_name: name.into(),
        client_version: version.into(),
        request_kind: kind,
    }
}

#[test]
fn recent_apps_read_tool_wakes_root_after_releasing_record_lock() {
    let f = Fixture::new();
    let ctx = egui::Context::default();
    let notices = f.handlers.changes.install(ctx.clone());
    let recent_apps = f.handlers.recent_apps.clone();
    let (repaints, repaint_requests) = std::sync::mpsc::channel();
    ctx.set_repaint_observer(move |request| {
        let recent = recent_apps
            .try_lock()
            .expect("recent-apps lock must be released before repainting");
        assert_eq!(recent.entries().len(), 1);
        assert_eq!(recent.entries()[0].client_name, "test");
        repaints.send(request).unwrap();
    });
    let tool = f.connection();
    assert!(repaint_requests.try_recv().is_err());

    // Both installed states must refresh, including a later call by the same app.
    for (id, ready) in [(1, false), (2, true)] {
        f.handlers.changes.set_ready(ready);
        assert!(f.call(&tool, id, Command::Status).result.is_ok());
        let wake = repaint_requests.try_recv().expect("read tool must repaint");
        assert_eq!(wake.viewport_id, egui::ViewportId::ROOT);
        assert_eq!(wake.delay, Duration::ZERO);
        assert!(repaint_requests.try_recv().is_err());
        assert!(notices.try_recv().is_err());
        assert!(
            f.handlers
                .changes
                .records()
                .lock()
                .unwrap()
                .entries()
                .is_empty()
        );
    }
}

#[test]
fn recent_apps_unrecorded_requests_do_not_wake_ui() {
    let f = Fixture::new();
    let ctx = egui::Context::default();
    let _notices = f.handlers.changes.install(ctx.clone());
    let (repaints, repaint_requests) = std::sync::mpsc::channel();
    ctx.set_repaint_observer(move |request| {
        repaints.send(request).unwrap();
    });

    for level in [AgentLevel::ReadOnly, AgentLevel::Disabled] {
        f.set_level(level);
        let tool = f.connection();
        let cancel = f.call(&tool, 1, Command::Cancel { request_id: 20 });
        if level == AgentLevel::ReadOnly {
            assert!(cancel.result.is_ok());
        } else {
            assert_eq!(cancel.result.unwrap_err().code, ErrorCode::Disabled);
        }
        let internal = Connection::new(f.handlers.clone());
        assert!(
            f.call(&internal, 2, hello("internal", "1", RequestKind::Internal))
                .result
                .is_ok()
        );
        let status = f.call(&internal, 3, Command::Status);
        if level == AgentLevel::ReadOnly {
            assert!(status.result.is_ok());
        } else {
            assert_eq!(status.result.unwrap_err().code, ErrorCode::Disabled);
        }
        assert!(f.handlers.recent_apps.lock().unwrap().entries().is_empty());
        assert!(repaint_requests.try_recv().is_err());
    }
}

#[test]
fn recent_apps_exclude_handshake_cancel_internal_and_include_disabled_busy_tools() {
    let f = Fixture::new();
    f.set_level(AgentLevel::Disabled);
    let tool = Connection::new(f.handlers.clone());
    // No worker or admission slot, including while disabled.
    let (send, replies) = crossbeam_channel::unbounded();
    assert!(
        tool.request(
            Request {
                id: 0,
                command: hello("tool app", "2", RequestKind::Tool),
            },
            send
        )
        .is_none()
    );
    assert!(replies.recv().unwrap().wire().result.is_ok());
    assert_eq!(f.handlers.active.load(Ordering::Acquire), 0);
    assert!(f.handlers.recent_apps.lock().unwrap().entries().is_empty());
    assert_eq!(
        f.call(&tool, 1, Command::Cancel { request_id: 20 })
            .result
            .unwrap_err()
            .code,
        ErrorCode::Disabled
    );
    assert!(f.handlers.recent_apps.lock().unwrap().entries().is_empty());
    assert_eq!(
        f.call(&tool, 2, Command::Status).result.unwrap_err().code,
        ErrorCode::Disabled
    );
    {
        let recent = f.handlers.recent_apps.lock().unwrap();
        assert_eq!(recent.entries().len(), 1);
        assert_eq!(recent.entries()[0].client_name, "tool app");
        assert_eq!(recent.entries()[0].client_version, "2");
    }
    let before = f.handlers.recent_apps.lock().unwrap().entries().to_vec();
    let internal = Connection::new(f.handlers.clone());
    assert!(
        f.call(
            &internal,
            3,
            hello("internal app", "3", RequestKind::Internal)
        )
        .result
        .is_ok()
    );
    assert_eq!(
        f.call(&internal, 4, Command::Status)
            .result
            .unwrap_err()
            .code,
        ErrorCode::Disabled
    );
    assert_eq!(f.handlers.recent_apps.lock().unwrap().entries(), before);

    f.set_level(AgentLevel::ReadOnly);
    let (first, _) = tool.acquire(20).unwrap();
    let (second, _) = internal.acquire(21).unwrap();
    let busy = Connection::new(f.handlers.clone());
    assert!(
        f.call(&busy, 5, hello("busy app", "4", RequestKind::Tool))
            .result
            .is_ok()
    );
    assert_eq!(f.handlers.active.load(Ordering::Acquire), 2);
    assert_eq!(
        f.call(&busy, 6, Command::Status).result.unwrap_err().code,
        ErrorCode::Busy
    );
    let before = f.handlers.recent_apps.lock().unwrap().entries().to_vec();
    assert_eq!(before.len(), 2);
    assert_eq!(before[0].client_name, "busy app");
    assert_eq!(
        f.call(&internal, 7, Command::Status)
            .result
            .unwrap_err()
            .code,
        ErrorCode::Busy
    );
    assert!(
        f.call(&busy, 8, Command::Cancel { request_id: 20 })
            .result
            .is_ok()
    );
    assert_eq!(f.handlers.recent_apps.lock().unwrap().entries(), before);
    drop((first, second));
    assert!(f.call(&internal, 9, Command::Status).result.is_ok());
    assert_eq!(f.handlers.recent_apps.lock().unwrap().entries(), before);
    assert!(f.call(&busy, 10, Command::Status).result.is_ok());
    assert_eq!(f.handlers.recent_apps.lock().unwrap().entries().len(), 2);
}

#[test]
fn recent_apps_record_timeout_with_identity_captured_at_request_start() {
    let mut f = Fixture::new();
    let base = Instant::now();
    let elapsed = Arc::new(AtomicUsize::new(0));
    let clock = elapsed.clone();
    let (started_send, started) = crossbeam_channel::bounded(1);
    let (release, gate) = crossbeam_channel::bounded(1);
    Arc::get_mut(&mut f.handlers).unwrap().read_hooks = Some(Arc::new(ReadHooks {
        now: Arc::new(move || base + Duration::from_secs(clock.load(Ordering::Acquire) as u64)),
        execute: Box::new(move |_, _| {
            started_send.send(()).unwrap();
            gate.recv_timeout(Duration::from_secs(10)).unwrap();
            Err(cancelled())
        }),
    }));
    let c = Connection::new(f.handlers.clone());
    assert!(
        f.call(&c, 1, hello("original", "1", RequestKind::Tool))
            .result
            .is_ok()
    );
    let (send, replies) = crossbeam_channel::unbounded();
    let worker = c
        .request(
            Request {
                id: 2,
                command: Command::Status,
            },
            send,
        )
        .unwrap();
    started.recv_timeout(Duration::from_secs(10)).unwrap();
    assert!(
        f.call(&c, 3, hello("later", "2", RequestKind::Internal))
            .result
            .is_ok()
    );
    elapsed.store(20, Ordering::Release);
    let response = replies
        .recv_timeout(Duration::from_secs(10))
        .unwrap()
        .wire();
    // Release the gated worker before assertions so a failed assertion cannot
    // leave the fixture's worker waiting for its deadline.
    release.send(()).unwrap();
    worker.join().unwrap();
    assert_eq!(response.result.unwrap_err().code, ErrorCode::Timeout);
    let recent = f.handlers.recent_apps.lock().unwrap();
    assert_eq!(recent.entries().len(), 1);
    assert_eq!(recent.entries()[0].client_name, "original");
    assert_eq!(recent.entries()[0].client_version, "1");
}
