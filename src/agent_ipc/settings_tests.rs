//! Headless settings permission, UI handoff, and accepted-write reply coverage.
use super::open::{UiOpenOutcome, UiRequestKind};
use super::tests::Fixture;
use super::*;
use serde_json::json;

fn save_settings_access(enabled: bool) {
    crate::settings_db::with_db_result(|db| {
        let mut saved = db.load_into_settings()?;
        saved.agent_allow_settings = enabled;
        db.save_full(&saved)
    })
    .unwrap();
}

fn read_command() -> Command {
    Command::GetSettings {
        query: None,
        keys: Some(vec!["fullscreen_wheel_action".into()]),
        page: None,
    }
}

fn changes() -> serde_json::Map<String, serde_json::Value> {
    json!({"fullscreen_wheel_action":"Zoom"})
        .as_object()
        .unwrap()
        .clone()
}

fn write_command() -> Command {
    Command::SetSettings { changes: changes() }
}

fn page() -> SettingsPage {
    SettingsPage {
        items: vec![
            SettingRow {
                key: "fullscreen_wheel_action".into(),
                label: "Wheel action".into(),
                value: json!("PageTurn"),
                overridden_by_favorite: true,
                choices: Some(None),
                template: None,
            }
            .into(),
        ],
        page: 1,
        pages: 1,
        more: false,
    }
}

fn actual_write_result() -> Payload {
    // The UI may retain or normalize a requested value. The adapter must return
    // the actual UI readback rather than synthesizing the requested "Zoom".
    Payload::SettingsWrite(SettingsWriteResult {
        changed: vec![],
        unchanged: vec![SettingChange {
            key: "fullscreen_wheel_action".into(),
            label: "Wheel action".into(),
            before: json!("PageTurn"),
            after: json!("PageTurn"),
            overridden_by_favorite: true,
            rows: vec![],
        }],
    })
}

fn start(
    connection: &Arc<Connection>,
    command: Command,
) -> (JoinHandle<()>, crossbeam_channel::Receiver<Reply>) {
    let (send, replies) = crossbeam_channel::unbounded();
    let worker = connection
        .request(Request { id: 401, command }, send)
        .unwrap();
    (worker, replies)
}

fn finish(worker: JoinHandle<()>, replies: &crossbeam_channel::Receiver<Reply>) -> Reply {
    let reply = replies.recv_timeout(Duration::from_secs(10)).unwrap();
    worker.join().unwrap();
    assert!(
        replies.try_recv().is_err(),
        "one reply per settings request"
    );
    reply
}

#[test]
fn settings_flag_refuses_both_commands_before_worker_and_read_only_grant_uses_epoch() {
    let mut fixture = Fixture::new();
    let read_calls = Arc::new(AtomicUsize::new(0));
    let observed = read_calls.clone();
    Arc::get_mut(&mut fixture.handlers).unwrap().read_hooks = Some(Arc::new(ReadHooks {
        now: Arc::new(Instant::now),
        execute: Box::new(move |command, _| {
            assert!(matches!(command, Command::GetSettings { .. }));
            observed.fetch_add(1, Ordering::AcqRel);
            Ok(Payload::Settings(page()))
        }),
    }));
    let ui = fixture.handlers.open.install(egui::Context::default());
    let connection = fixture.connection();
    let denied_grant = permission().unwrap();
    assert_eq!(denied_grant.0.level, AgentLevel::ReadOnly);
    assert!(!denied_grant.0.settings);
    for command in [read_command(), write_command()] {
        let (send, replies) = crossbeam_channel::unbounded();
        assert!(
            connection
                .request(Request { id: 400, command }, send)
                .is_none(),
            "the settings flag must refuse before spawning a worker"
        );
        let failure = replies
            .recv_timeout(Duration::from_secs(10))
            .unwrap()
            .wire()
            .result
            .unwrap_err();
        assert_eq!(failure.code, ErrorCode::Disabled);
        assert_eq!(failure.message, SETTINGS_PERMISSION_MESSAGE);
        assert!(ui.try_recv().is_err());
        assert_eq!(fixture.handlers.active.load(Ordering::Acquire), 0);
    }
    assert_eq!(read_calls.load(Ordering::Acquire), 0);
    save_settings_access(true);
    let granted = permission().unwrap();
    assert!(granted.0.settings);
    assert_eq!(granted.0.level, AgentLevel::ReadOnly);
    assert_ne!(granted.1, denied_grant.1);
    assert_eq!(
        fixture
            .call(&connection, 402, read_command())
            .result
            .unwrap(),
        Payload::Settings(page())
    );
    assert_eq!(read_calls.load(Ordering::Acquire), 1);
}

#[test]
fn settings_read_uses_ui_owner_and_rechecks_permission_epoch_before_wire() {
    let fixture = Fixture::new();
    save_settings_access(true);
    let ui = fixture.handlers.open.install(egui::Context::default());
    let connection = fixture.connection();
    let granted = permission().unwrap();
    let (worker, replies) = start(&connection, read_command());
    let request = ui.recv_timeout(Duration::from_secs(10)).unwrap();
    match &request.kind {
        UiRequestKind::GetSettings { query, keys, page } => {
            assert!(query.is_none() && page.is_none());
            assert_eq!(
                keys.as_deref().unwrap(),
                &["fullscreen_wheel_action".to_owned()]
            );
        }
        _ => panic!("settings read did not reach its UI owner"),
    }
    assert!(request.try_claim());
    request.complete(UiOpenOutcome::Settings(page()));
    let reply = finish(worker, &replies);
    assert!(matches!(reply.policy, ReplyPolicy::Read(_)));
    assert_eq!(reply.response.result, Ok(Payload::Settings(page())));
    save_settings_access(false);
    assert_ne!(permission().unwrap().1, granted.1);
    assert_eq!(
        reply.wire().result.unwrap_err().code,
        ErrorCode::PermissionChanged
    );
    assert_eq!(fixture.handlers.active.load(Ordering::Acquire), 0);
}

#[test]
fn settings_write_read_only_claim_keeps_actual_result_after_permission_revocation() {
    let mut fixture = Fixture::new();
    save_settings_access(true);
    Arc::get_mut(&mut fixture.handlers).unwrap().read_hooks = Some(Arc::new(ReadHooks {
        now: Arc::new(Instant::now),
        execute: Box::new(|_, _| panic!("settings writes must use metadata_response")),
    }));
    let ui = fixture.handlers.open.install(egui::Context::default());
    let connection = fixture.connection();
    assert_eq!(permission().unwrap().0.level, AgentLevel::ReadOnly);
    let (worker, replies) = start(&connection, write_command());
    let request = ui.recv_timeout(Duration::from_secs(10)).unwrap();
    match &request.kind {
        UiRequestKind::SetSettings {
            changes: prepared,
            client_name,
        } => {
            assert_eq!(prepared, &changes());
            assert_eq!(client_name, "test");
        }
        _ => panic!("settings write did not reach its UI owner"),
    }
    assert!(request.try_claim());
    save_settings_access(false);
    assert!(
        fixture
            .call(&connection, 403, Command::Cancel { request_id: 401 })
            .result
            .is_ok()
    );
    assert!(
        replies.try_recv().is_err(),
        "claimed writes await the UI result"
    );
    assert_eq!(fixture.handlers.active.load(Ordering::Acquire), 1);
    let actual = actual_write_result();
    request.complete(UiOpenOutcome::Written(Ok(actual.clone())));
    let reply = finish(worker, &replies);
    assert!(matches!(reply.policy, ReplyPolicy::AcceptedOperation));
    fixture.set_level(AgentLevel::Disabled);
    assert_eq!(reply.wire().result.unwrap(), actual);
    assert_eq!(fixture.handlers.active.load(Ordering::Acquire), 0);
    let recent = fixture.handlers.recent_apps.lock().unwrap();
    assert_eq!(recent.entries().len(), 1);
    assert_eq!(recent.entries()[0].client_name, "test");
}

#[test]
fn settings_pending_cancel_shares_ui_slot_and_prevents_late_commit() {
    let fixture = Fixture::new();
    save_settings_access(true);
    let ui = fixture.handlers.open.install(egui::Context::default());
    let connection = fixture.connection();
    let second = fixture.connection();
    let (worker, replies) = start(&connection, write_command());
    let request = ui.recv_timeout(Duration::from_secs(10)).unwrap();
    assert!(matches!(request.kind, UiRequestKind::SetSettings { .. }));
    assert_eq!(
        fixture
            .call(&second, 404, read_command())
            .result
            .unwrap_err()
            .code,
        ErrorCode::Busy
    );
    assert!(ui.try_recv().is_err());
    assert!(
        fixture
            .call(&connection, 405, Command::Cancel { request_id: 401 })
            .result
            .is_ok()
    );
    let reply = finish(worker, &replies);
    assert!(matches!(reply.policy, ReplyPolicy::Rejected));
    assert_eq!(reply.wire().result.unwrap_err().code, ErrorCode::Cancelled);
    assert!(
        !request.try_claim(),
        "cancelled settings must never commit late"
    );
    request.complete(UiOpenOutcome::Written(Ok(actual_write_result())));
    assert!(ui.try_recv().is_err());
    assert_eq!(fixture.handlers.active.load(Ordering::Acquire), 0);
}
