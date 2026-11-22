//! Headless favorite permission, physical-folder preparation, and UI dispatch lifecycle.
use super::open::{UiOpenOutcome, UiRequestKind};
use super::tests::Fixture;
use super::*;
use std::path::PathBuf;

fn save_settings_access(enabled: bool) {
    crate::settings_db::with_db_result(|db| {
        let mut saved = db.load_into_settings()?;
        saved.agent_allow_settings = enabled;
        db.save_full(&saved)
    })
    .unwrap();
}

fn command(path: &Path) -> Command {
    Command::SetFavorite {
        path: path.to_string_lossy().into_owned(),
        name: Some("requested name".into()),
        item_index: None,
        container_index: Some(false),
    }
}

fn actual_result(path: &Path) -> Payload {
    // An existing favorite retains its actual name and any omitted flag.
    Payload::FavoriteWrite(FavoriteWriteResult {
        path: path.to_string_lossy().into_owned(),
        name: "existing name".into(),
        added: false,
        before: Some(FavoriteIndexState {
            item_index: true,
            container_index: true,
        }),
        after: FavoriteIndexState {
            item_index: true,
            container_index: false,
        },
        message: None,
    })
}

fn start(
    connection: &Arc<Connection>,
    command: Command,
) -> (JoinHandle<()>, crossbeam_channel::Receiver<Reply>) {
    let (send, replies) = crossbeam_channel::unbounded();
    let worker = connection
        .request(Request { id: 501, command }, send)
        .unwrap();
    (worker, replies)
}

fn finish(worker: JoinHandle<()>, replies: &crossbeam_channel::Receiver<Reply>) -> Reply {
    let reply = replies.recv_timeout(Duration::from_secs(10)).unwrap();
    worker.join().unwrap();
    assert!(
        replies.try_recv().is_err(),
        "one reply per favorite request"
    );
    reply
}

#[test]
fn favorite_settings_flag_refuses_before_path_worker_and_grant_changes_epoch() {
    let mut fixture = Fixture::new();
    Arc::get_mut(&mut fixture.handlers).unwrap().read_hooks = Some(Arc::new(ReadHooks {
        now: Arc::new(Instant::now),
        execute: Box::new(|_, _| panic!("favorite writes must use metadata_response")),
    }));
    let ui = fixture.handlers.open.install(egui::Context::default());
    let connection = fixture.connection();
    let denied = permission().unwrap();
    assert_eq!(denied.0.level, AgentLevel::ReadOnly);
    assert!(!denied.0.settings);
    let (send, replies) = crossbeam_channel::unbounded();
    assert!(
        connection
            .request(
                Request {
                    id: 500,
                    command: command(Path::new("relative-before-path-validation")),
                },
                send
            )
            .is_none(),
        "settings permission must refuse before spawning a path worker"
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
    save_settings_access(true);
    let granted = permission().unwrap();
    assert_eq!(granted.0.level, AgentLevel::ReadOnly);
    assert!(granted.0.settings);
    assert_ne!(denied.1, granted.1);
}

#[cfg(windows)]
#[test]
fn favorite_path_worker_rejects_relative_targets_without_ui_queue() {
    let fixture = Fixture::new();
    save_settings_access(true);
    let ui = fixture.handlers.open.install(egui::Context::default());
    let connection = fixture.connection();
    for path in [PathBuf::from("relative")] {
        let (worker, replies) = start(&connection, command(&path));
        let reply = finish(worker, &replies);
        assert!(matches!(reply.policy, ReplyPolicy::Rejected));
        assert_eq!(reply.wire().result.unwrap_err().code, ErrorCode::BadRequest);
        assert!(
            ui.try_recv().is_err(),
            "invalid physical folder must not reach UI"
        );
        assert_eq!(fixture.handlers.active.load(Ordering::Acquire), 0);
    }
}

#[cfg(windows)]
#[test]
fn favorite_path_worker_passes_unavailable_folder_results_to_ui_for_live_registration_check() {
    let fixture = Fixture::new();
    save_settings_access(true);
    let ui = fixture.handlers.open.install(egui::Context::default());
    let connection = fixture.connection();
    let file = fixture._dir.path().join("favorite-file.png");
    let archive = fixture._dir.path().join("favorite-file.zip");
    std::fs::write(&file, b"headless fixture; no decoding").unwrap();
    std::fs::write(&archive, b"PK headless fixture; no extraction").unwrap();
    for path in [fixture._dir.path().join("favorite-missing"), file, archive] {
        let (worker, replies) = start(&connection, command(&path));
        let request = ui.recv_timeout(Duration::from_secs(10)).unwrap();
        let UiRequestKind::SetFavorite(prepared) = &request.kind else {
            panic!("expected a favorite request");
        };
        assert!(crate::folder_tree::path_eq(&prepared.path, &path));
        let error = prepared.folder_check.clone().unwrap_err();
        assert_eq!(error.code, ErrorCode::BadRequest);
        assert!(request.try_claim());
        // UI refuses a new registration with this error; existing registrations
        // ignore it and can switch their indexes off (covered by App regression).
        request.complete(UiOpenOutcome::Refused(error));
        let reply = finish(worker, &replies);
        assert_eq!(reply.wire().result.unwrap_err().code, ErrorCode::BadRequest);
        assert_eq!(fixture.handlers.active.load(Ordering::Acquire), 0);
    }
}

#[cfg(windows)]
#[test]
fn favorite_read_only_worker_prepares_flags_and_claim_preserves_actual_result_after_revocation() {
    let mut fixture = Fixture::new();
    save_settings_access(true);
    Arc::get_mut(&mut fixture.handlers).unwrap().read_hooks = Some(Arc::new(ReadHooks {
        now: Arc::new(Instant::now),
        execute: Box::new(|_, _| panic!("favorite writes must use metadata_response")),
    }));
    let path = fixture._dir.path().join("favorite");
    std::fs::create_dir(&path).unwrap();
    let ui = fixture.handlers.open.install(egui::Context::default());
    let connection = fixture.connection();
    assert_eq!(permission().unwrap().0.level, AgentLevel::ReadOnly);
    let (worker, replies) = start(&connection, command(&path));
    let request = ui.recv_timeout(Duration::from_secs(10)).unwrap();
    match &request.kind {
        UiRequestKind::SetFavorite(prepared) => {
            assert!(prepared.folder_check.is_ok());
            assert_eq!(prepared.path, crate::path_key::normalize_input_path(&path));
            assert_eq!(prepared.name.as_deref(), Some("requested name"));
            assert_eq!(prepared.item_index, None);
            assert_eq!(prepared.container_index, Some(false));
            assert_eq!(prepared.client_name, "test");
        }
        _ => panic!("favorite write did not reach its UI owner with real preparation"),
    }
    assert!(request.try_claim());
    save_settings_access(false);
    assert!(
        fixture
            .call(&connection, 502, Command::Cancel { request_id: 501 })
            .result
            .is_ok()
    );
    assert!(
        replies.try_recv().is_err(),
        "claimed writes await actual UI readback"
    );
    assert_eq!(fixture.handlers.active.load(Ordering::Acquire), 1);
    let actual = actual_result(&path);
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

#[cfg(windows)]
#[test]
fn favorite_pending_cancel_shares_ui_slot_and_prevents_late_claim() {
    let fixture = Fixture::new();
    save_settings_access(true);
    let path = fixture._dir.path().join("favorite");
    std::fs::create_dir(&path).unwrap();
    let ui = fixture.handlers.open.install(egui::Context::default());
    let connection = fixture.connection();
    let second = fixture.connection();
    let (worker, replies) = start(&connection, command(&path));
    let request = ui.recv_timeout(Duration::from_secs(10)).unwrap();
    assert!(matches!(request.kind, UiRequestKind::SetFavorite(_)));
    assert_eq!(
        fixture
            .call(
                &second,
                503,
                Command::GetSettings {
                    query: None,
                    keys: None,
                    page: None,
                }
            )
            .result
            .unwrap_err()
            .code,
        ErrorCode::Busy
    );
    assert!(ui.try_recv().is_err());
    assert!(
        fixture
            .call(&connection, 504, Command::Cancel { request_id: 501 })
            .result
            .is_ok()
    );
    let reply = finish(worker, &replies);
    assert!(matches!(reply.policy, ReplyPolicy::Rejected));
    assert_eq!(reply.wire().result.unwrap_err().code, ErrorCode::Cancelled);
    assert!(
        !request.try_claim(),
        "cancelled favorite request must never commit late"
    );
    request.complete(UiOpenOutcome::Written(Ok(actual_result(&path))));
    assert!(ui.try_recv().is_err());
    assert_eq!(fixture.handlers.active.load(Ordering::Acquire), 0);
}
