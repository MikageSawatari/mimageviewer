use super::tests::Fixture;
use super::*;

fn gated_commands() -> Vec<(Command, &'static str)> {
    let item = ItemRef {
        path: "C:/missing.mp4".into(),
        subresource: None,
    };
    vec![
        (
            Command::GetThumbnail {
                items: vec![item.clone()],
                long_side: None,
                cursor: None,
            },
            THUMBNAIL_PERMISSION_MESSAGE,
        ),
        (
            Command::GetTranscript {
                item: item.clone(),
                start_secs: None,
                cursor: None,
            },
            TRANSCRIPT_PERMISSION_MESSAGE,
        ),
        (
            Command::OpenInViewer {
                path: item.path.clone(),
            },
            OPEN_PERMISSION_MESSAGE,
        ),
        (
            Command::CreateCollection { name: "new".into() },
            ORGANIZE_PERMISSION_MESSAGE,
        ),
        (
            Command::RenameCollection {
                id: "id".into(),
                name: "new".into(),
            },
            ORGANIZE_PERMISSION_MESSAGE,
        ),
        (
            Command::AddToCollection {
                id: "id".into(),
                items: vec![item.clone()],
            },
            ORGANIZE_PERMISSION_MESSAGE,
        ),
        (
            Command::RemoveFromCollection {
                id: "id".into(),
                items: vec![item.clone()],
            },
            ORGANIZE_PERMISSION_MESSAGE,
        ),
        (
            Command::AddTags {
                items: vec![item.clone()],
                tags: vec!["tag".into()],
            },
            ORGANIZE_PERMISSION_MESSAGE,
        ),
        (
            Command::RemoveTags {
                items: vec![item.clone()],
                tags: vec!["tag".into()],
            },
            ORGANIZE_PERMISSION_MESSAGE,
        ),
        (
            Command::SetRating {
                items: vec![item],
                stars: 3,
            },
            ORGANIZE_PERMISSION_MESSAGE,
        ),
    ]
}

#[test]
fn core_admission_names_required_permission_before_any_tool_work() {
    let fixture = Fixture::new();
    let connection = fixture.connection();
    for (index, (command, message)) in gated_commands().into_iter().enumerate() {
        let response = fixture.call(&connection, index as u64 + 1, command);
        let denied = response.result.unwrap_err();
        assert_eq!(denied.code, ErrorCode::Disabled);
        assert_eq!(denied.message, message);
        assert!(message.contains("AI 連携…"));
    }
    fixture.set_level(AgentLevel::Disabled);
    for (index, (command, _)) in gated_commands().into_iter().enumerate() {
        let denied = fixture
            .call(&connection, index as u64 + 100, command)
            .result
            .unwrap_err();
        assert_eq!(denied.code, ErrorCode::Disabled);
        assert_eq!(denied.message, DISABLED_PERMISSION_MESSAGE);
    }
    let denied = fixture
        .call(&connection, 200, Command::Status)
        .result
        .unwrap_err();
    assert_eq!(denied.message, DISABLED_PERMISSION_MESSAGE);
}

#[test]
fn core_permission_matrix_keeps_gps_and_captions_as_content_permissions() {
    for level in [
        AgentLevel::ReadOnly,
        AgentLevel::ReadAndOpen,
        AgentLevel::Organize,
    ] {
        for bits in 0..16 {
            let snapshot = PermissionSnapshot {
                level,
                thumbnails: bits & 1 != 0,
                gps: bits & 2 != 0,
                ai_captions: bits & 4 != 0,
                transcripts: bits & 8 != 0,
            };
            for (command, message) in gated_commands() {
                let allowed = match command {
                    Command::GetThumbnail { .. } => snapshot.thumbnails,
                    Command::GetTranscript { .. } => snapshot.transcripts,
                    Command::OpenInViewer { .. } => level >= AgentLevel::ReadAndOpen,
                    _ => level >= AgentLevel::Organize,
                };
                let result = check_tool_permission(snapshot, &command);
                assert_eq!(result.is_ok(), allowed);
                if !allowed {
                    assert_eq!(result.unwrap_err().message, message);
                }
            }
            assert!(check_tool_permission(snapshot, &Command::Status).is_ok());
            assert!(
                check_tool_permission(
                    snapshot,
                    &Command::GetItemInfo {
                        items: vec![],
                        limit: None,
                        cursor: None
                    }
                )
                .is_ok()
            );
        }
    }
}

#[test]
fn core_same_connection_accepts_immediately_after_enabling_without_discovery() {
    let fixture = Fixture::new();
    let connection = fixture.connection();
    fixture.set_level(AgentLevel::Disabled);
    assert_eq!(
        fixture
            .call(&connection, 1, Command::Status)
            .result
            .unwrap_err()
            .code,
        ErrorCode::Disabled
    );
    fixture.set_level(AgentLevel::ReadOnly);
    assert!(matches!(
        fixture.call(&connection, 2, Command::Status).result,
        Ok(Payload::Status(_))
    ));
    let commands = gated_commands();
    for (index, (command, _)) in commands.iter().take(2).enumerate() {
        assert_eq!(
            fixture
                .call(&connection, 10 + index as u64, command.clone())
                .result
                .unwrap_err()
                .code,
            ErrorCode::Disabled
        );
    }
    crate::settings_db::with_db_result(|db| {
        let mut settings = db.load_into_settings()?;
        settings.agent_show_thumbnails = true;
        settings.agent_show_transcripts = true;
        db.save_full(&settings)
    })
    .unwrap();
    // 存在しないファイルでも、許可後はhandlerへ届いて項目の状態として返る。
    for (index, (command, _)) in commands.into_iter().take(2).enumerate() {
        assert!(
            fixture
                .call(&connection, 20 + index as u64, command)
                .result
                .is_ok()
        );
    }
    // Openも同じconnectionのまま許可を上げ、既存UI受付の完了まで検査する。
    fixture.set_level(AgentLevel::ReadAndOpen);
    let queue = fixture.handlers.open.install(egui::Context::default());
    let (enqueued_tx, enqueued_rx) = std::sync::mpsc::channel();
    fixture.handlers.open.set_enqueue_observer(enqueued_tx);
    let (send, replies) = crossbeam_channel::unbounded();
    let worker = connection
        .request(
            Request {
                id: 29,
                command: Command::OpenInViewer {
                    path: fixture._dir.path().to_string_lossy().into_owned(),
                },
            },
            send,
        )
        .unwrap();
    enqueued_rx.recv_timeout(Duration::from_secs(10)).unwrap();
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

    let create = Command::CreateCollection {
        name: "after grant".into(),
    };
    assert_eq!(
        fixture
            .call(&connection, 30, create.clone())
            .result
            .unwrap_err()
            .message,
        ORGANIZE_PERMISSION_MESSAGE
    );
    fixture.set_level(AgentLevel::Organize);
    let _notices = fixture.handlers.changes.install(egui::Context::default());
    fixture.handlers.changes.set_ready(true);
    let result = fixture.call(&connection, 31, create).result;
    assert!(
        matches!(result, Ok(Payload::CollectionWrite(_))),
        "{result:?}"
    );
}
