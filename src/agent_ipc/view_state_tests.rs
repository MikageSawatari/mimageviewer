use super::tests::Fixture;
use super::*;

fn snapshot() -> ViewState {
    ViewState {
        view: ViewKind::Folder,
        folder: Some("C:/fixture".into()),
        fullscreen: false,
        item: None,
        search: ViewSearch {
            item_index: false,
            favorite: None,
            favorite_root: false,
        },
    }
}

fn save_flags(view: bool, settings: bool) {
    crate::settings_db::with_db_result(|db| {
        let mut saved = db.load_into_settings()?;
        saved.agent_show_current_view = view;
        saved.agent_allow_settings = settings;
        db.save_full(&saved)
    })
    .unwrap();
}

#[test]
fn get_view_state_permission_off_refuses_before_work_and_status_reports_both_flags() {
    let mut fixture = Fixture::new();
    Arc::get_mut(&mut fixture.handlers).unwrap().read_hooks = Some(Arc::new(ReadHooks {
        now: Arc::new(Instant::now),
        execute: Box::new(|command, _| {
            assert!(matches!(command, Command::GetViewState));
            Ok(Payload::ViewState(snapshot()))
        }),
    }));
    let connection = fixture.connection();
    let denied = fixture
        .call(&connection, 1, Command::GetViewState)
        .result
        .unwrap_err();
    assert_eq!(denied.code, ErrorCode::Disabled);
    assert_eq!(denied.message, CURRENT_VIEW_PERMISSION_MESSAGE);
    save_flags(true, false);
    assert!(matches!(
        fixture.call(&connection, 2, Command::GetViewState).result,
        Ok(Payload::ViewState(_))
    ));
    fixture.set_level(AgentLevel::Disabled);
    assert_eq!(
        fixture
            .call(&connection, 3, Command::GetViewState)
            .result
            .unwrap_err()
            .code,
        ErrorCode::Disabled
    );
    drop(connection);
    // Restore the ordinary status handler after the bounded test hook is gone.
    Arc::get_mut(&mut fixture.handlers).unwrap().read_hooks = None;
    fixture.set_level(AgentLevel::ReadOnly);
    save_flags(true, true);
    let connection = fixture.connection();
    let Payload::Status(status) = fixture
        .call(&connection, 4, Command::Status)
        .result
        .unwrap()
    else {
        panic!("status")
    };
    assert!(status.permission.current_view && status.permission.settings);
}

#[test]
fn get_view_state_reply_rejects_a_permission_epoch_change_before_emission() {
    let mut fixture = Fixture::new();
    save_flags(true, false);
    let granted = permission().unwrap();
    let (entered, waiting) = crossbeam_channel::bounded(1);
    let (release, resume) = crossbeam_channel::bounded(1);
    Arc::get_mut(&mut fixture.handlers).unwrap().read_hooks = Some(Arc::new(ReadHooks {
        now: Arc::new(Instant::now),
        execute: Box::new(move |command, _| {
            assert!(matches!(command, Command::GetViewState));
            entered.send(()).unwrap();
            resume.recv().unwrap();
            Ok(Payload::ViewState(snapshot()))
        }),
    }));
    let connection = fixture.connection();
    let (send, receive) = crossbeam_channel::unbounded();
    let worker = connection
        .request(
            Request {
                id: 9,
                command: Command::GetViewState,
            },
            send,
        )
        .unwrap();
    waiting.recv_timeout(Duration::from_secs(10)).unwrap();
    // Even changing the independent settings flag invalidates the old grant.
    save_flags(true, true);
    assert_ne!(permission().unwrap().1, granted.1);
    release.send(()).unwrap();
    let reply = receive.recv_timeout(Duration::from_secs(10)).unwrap();
    worker.join().unwrap();
    assert_eq!(
        reply.wire().result.unwrap_err().code,
        ErrorCode::PermissionChanged
    );
}
