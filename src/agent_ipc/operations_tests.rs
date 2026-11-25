use super::open::{UiOpenOutcome, UiRequestKind};
use super::tests::Fixture;
use super::*;
use serde_json::json;

fn save_access(enabled: bool) {
    crate::settings_db::with_db_result(|db| {
        let mut settings = db.load_into_settings()?;
        settings.agent_allow_settings = enabled;
        db.save_full(&settings)
    })
    .unwrap();
}

fn read() -> Command {
    Command::GetOperations(OperationsQuery::default())
}
fn write() -> Command {
    Command::SetOperations(
        serde_json::from_value(json!({"keys":{"FsCopyImage":["Ctrl+C"]}})).unwrap(),
    )
}
fn page() -> OperationsPage {
    OperationsPage {
        items: vec![],
        page: 1,
        pages: 1,
        reserved: vec![],
        note: None,
    }
}
fn result() -> Payload {
    Payload::OperationsWrite(OperationsWriteResult {
        changed: vec![OperationChange {
            name: "FsCopyImage".into(),
            label: "copy".into(),
            before: serde_json::Value::Null,
            after: json!(["Ctrl+C"]),
        }],
        unchanged: vec![],
        conflicts: vec![],
    })
}

fn start(
    connection: &Arc<Connection>,
    command: Command,
) -> (JoinHandle<()>, crossbeam_channel::Receiver<Reply>) {
    let (sender, replies) = crossbeam_channel::unbounded();
    (
        connection
            .request(Request { id: 401, command }, sender)
            .unwrap(),
        replies,
    )
}

#[test]
fn operations_permission_flag_refuses_reads_and_writes_before_worker() {
    let fixture = Fixture::new();
    let ui = fixture.handlers.open.install(egui::Context::default());
    let connection = fixture.connection();
    for command in [read(), write()] {
        let (sender, replies) = crossbeam_channel::unbounded();
        assert!(
            connection
                .request(Request { id: 400, command }, sender)
                .is_none()
        );
        let error = replies
            .recv_timeout(Duration::from_secs(10))
            .unwrap()
            .wire()
            .result
            .unwrap_err();
        assert_eq!(error.code, ErrorCode::Disabled);
        assert_eq!(error.message, SETTINGS_PERMISSION_MESSAGE);
        assert!(ui.try_recv().is_err());
        assert_eq!(fixture.handlers.active.load(Ordering::Acquire), 0);
    }
}

#[test]
fn operations_read_uses_ui_owner_and_rechecks_epoch_before_wire() {
    let fixture = Fixture::new();
    save_access(true);
    let ui = fixture.handlers.open.install(egui::Context::default());
    let connection = fixture.connection();
    let (worker, replies) = start(&connection, read());
    let request = ui.recv_timeout(Duration::from_secs(10)).unwrap();
    assert!(matches!(&request.kind, UiRequestKind::GetOperations(_)));
    assert!(request.try_claim());
    request.complete(UiOpenOutcome::Operations(page()));
    let reply = replies.recv_timeout(Duration::from_secs(10)).unwrap();
    worker.join().unwrap();
    assert!(matches!(reply.policy, ReplyPolicy::Read(_)));
    assert_eq!(reply.response.result, Ok(Payload::Operations(page())));
    save_access(false);
    assert_eq!(
        reply.wire().result.unwrap_err().code,
        ErrorCode::PermissionChanged
    );
}

#[test]
fn operations_claimed_write_keeps_actual_reply_after_cancel_and_permission_revocation() {
    let fixture = Fixture::new();
    save_access(true);
    let ui = fixture.handlers.open.install(egui::Context::default());
    let connection = fixture.connection();
    assert_eq!(permission().unwrap().0.level, AgentLevel::ReadOnly);
    let (worker, replies) = start(&connection, write());
    let request = ui.recv_timeout(Duration::from_secs(10)).unwrap();
    assert!(
        matches!(&request.kind,UiRequestKind::SetOperations { client_name,.. } if client_name == "test")
    );
    assert!(request.try_claim());
    save_access(false);
    assert!(
        fixture
            .call(&connection, 402, Command::Cancel { request_id: 401 })
            .result
            .is_ok()
    );
    assert!(replies.try_recv().is_err());
    request.complete(UiOpenOutcome::Written(Ok(result())));
    let reply = replies.recv_timeout(Duration::from_secs(10)).unwrap();
    worker.join().unwrap();
    assert!(matches!(reply.policy, ReplyPolicy::AcceptedOperation));
    assert_eq!(reply.wire().result, Ok(result()));
    assert!(replies.try_recv().is_err());
}

#[test]
fn operations_pending_cancel_prevents_late_commit_and_shares_existing_ui_slot() {
    let fixture = Fixture::new();
    save_access(true);
    let ui = fixture.handlers.open.install(egui::Context::default());
    let connection = fixture.connection();
    let second = fixture.connection();
    let (worker, replies) = start(&connection, write());
    let request = ui.recv_timeout(Duration::from_secs(10)).unwrap();
    assert_eq!(
        fixture
            .call(
                &second,
                402,
                Command::GetSettings {
                    query: None,
                    keys: None,
                    page: None
                }
            )
            .result
            .unwrap_err()
            .code,
        ErrorCode::Busy
    );
    assert!(
        fixture
            .call(&connection, 403, Command::Cancel { request_id: 401 })
            .result
            .is_ok()
    );
    let reply = replies.recv_timeout(Duration::from_secs(10)).unwrap();
    worker.join().unwrap();
    assert_eq!(reply.wire().result.unwrap_err().code, ErrorCode::Cancelled);
    assert!(!request.try_claim());
    request.complete(UiOpenOutcome::Written(Ok(result())));
    assert!(ui.try_recv().is_err());
}
