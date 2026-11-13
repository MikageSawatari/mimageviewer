use super::tests::Fixture;
use super::*;

fn poll(fixture: &Fixture, version: u32) {
    // Match the adapter's fresh connection + Hello + Status for every poll.
    let connection = Connection::new(fixture.handlers.clone());
    let hello = fixture.call(
        &connection,
        1,
        Command::Hello {
            version,
            client_name: "private client identity".into(),
            client_version: "private version".into(),
            request_kind: RequestKind::Internal,
        },
    );
    if hello.result.is_ok() {
        fixture.call(&connection, 2, Command::Status);
    }
    connection.disconnect();
}

fn lines(fixture: &Fixture) -> Vec<String> {
    fixture.handlers.lane_log.lock().unwrap().lines.clone()
}

#[test]
fn search_input_history_is_private_in_agent_responses_and_logs() {
    let fixture = Fixture::new();
    fixture.seed_index();
    let books = fixture._dir.path().join("books");
    std::fs::create_dir(&books).unwrap();
    let sentinel = "miv-history-sentinel-756";
    crate::settings_db::with_db_result(|db| {
        let mut settings = db.load_into_settings()?;
        settings.book_root = Some(books.clone());
        settings
            .search_input_history
            .record(crate::ui_pinned_search::PinnedSearchKind::Local, sentinel);
        db.save_full(&settings)
    })
    .unwrap();
    assert_eq!(
        crate::settings_db::with_db_result(|db| db.load_into_settings())
            .unwrap()
            .search_input_history
            .len(),
        1
    );
    let collection = fixture
        .runtime
        .as_ref()
        .unwrap()
        .client()
        .create_collection("public collection".into())
        .unwrap()
        .recv_timeout(Duration::from_secs(10))
        .unwrap()
        .unwrap();
    let connection = fixture.connection();
    for (id, command) in [
        Command::Status,
        Command::ListLibrary {
            kind: LibraryKind::Bookshelf,
            stars: None,
            max_stars: None,
            limit: None,
            cursor: None,
        },
        Command::GetCollection {
            id: Some(collection.collection_id().to_string()),
            name: None,
            limit: None,
            cursor: None,
        },
        Command::SearchItems {
            query: "cat".into(),
            kind: Some(ItemKind::Image),
            limit: Some(2),
            cursor: None,
        },
        Command::ListFolder {
            path: books.to_string_lossy().into_owned(),
            subresource: None,
            min_stars: None,
            max_stars: None,
            kinds: None,
            limit: None,
            cursor: None,
        },
    ]
    .into_iter()
    .enumerate()
    {
        let response = fixture.call(&connection, id as u64 + 1, command);
        assert!(response.result.is_ok(), "{response:?}");
        let json = serde_json::to_string(&response).unwrap();
        assert!(!json.contains(sentinel));
        assert!(!json.contains("search_input_history"));
    }
    assert!(lines(&fixture).iter().all(|line| !line.contains(sentinel)));
    assert!(
        lines(&fixture)
            .iter()
            .all(|line| !line.contains("search_input_history"))
    );
}

#[test]
fn periodic_hello_and_status_stay_quiet_across_fresh_connections() {
    let fixture = Fixture::new();
    poll(&fixture, AGENT_PROTOCOL_VERSION);
    let first = lines(&fixture);
    assert_eq!(first.len(), 3);
    assert!(first[0].contains("tool=hello"));
    assert!(first[1].contains("state=permission_changed"));
    assert!(first[2].contains("tool=miv_status"));
    for _ in 0..20 {
        poll(&fixture, AGENT_PROTOCOL_VERSION);
    }
    assert_eq!(lines(&fixture), first);
    assert!(
        fixture
            .handlers
            .recent_apps
            .lock()
            .unwrap()
            .entries()
            .is_empty()
    );
    let connection = fixture.connection();
    for id in 10..12 {
        fixture.call(&connection, id, Command::Cancel { request_id: 0 });
    }
    assert_eq!(lines(&fixture).len(), first.len() + 2);
    // Actual tool attribution is retained even when Status logging is quiet.
    fixture.call(&connection, 20, Command::Status);
    assert_eq!(
        fixture.handlers.recent_apps.lock().unwrap().entries().len(),
        1
    );
    assert!(lines(&fixture).iter().all(|line| !line.contains("private")));
}

#[test]
fn polling_logs_permission_changes_errors_and_recovery_once() {
    let fixture = Fixture::new();
    poll(&fixture, AGENT_PROTOCOL_VERSION);
    let initial_count = lines(&fixture).len();
    fixture.set_level(AgentLevel::Disabled);
    poll(&fixture, AGENT_PROTOCOL_VERSION);
    let disabled = lines(&fixture);
    assert_eq!(disabled.len(), initial_count + 2);
    assert!(disabled.last().unwrap().contains("error=disabled"));
    for _ in 0..3 {
        poll(&fixture, AGENT_PROTOCOL_VERSION);
    }
    assert_eq!(lines(&fixture), disabled);
    fixture.set_level(AgentLevel::ReadOnly);
    poll(&fixture, AGENT_PROTOCOL_VERSION);
    let recovered = lines(&fixture);
    assert_eq!(recovered.len(), disabled.len() + 2);
    assert!(recovered.last().unwrap().contains("error=none"));
    crate::settings_db::with_db_result(|db| {
        let mut settings = db.load_into_settings()?;
        settings.agent_show_thumbnails = true;
        settings.agent_show_gps = true;
        db.save_full(&settings)
    })
    .unwrap();
    poll(&fixture, AGENT_PROTOCOL_VERSION);
    assert_eq!(lines(&fixture).len(), recovered.len() + 1);
    assert!(
        lines(&fixture)
            .last()
            .unwrap()
            .contains("state=permission_changed")
    );
    let count = lines(&fixture).len();
    poll(&fixture, AGENT_PROTOCOL_VERSION + 1);
    assert_eq!(lines(&fixture).len(), count + 1);
    assert!(
        lines(&fixture)
            .last()
            .unwrap()
            .contains("error=version_mismatch")
    );
    poll(&fixture, AGENT_PROTOCOL_VERSION + 1);
    assert_eq!(lines(&fixture).len(), count + 1);
    poll(&fixture, AGENT_PROTOCOL_VERSION);
    assert_eq!(lines(&fixture).len(), count + 2);
}

#[test]
fn polling_state_is_bounded_to_permission_epoch_and_error_kind() {
    let mut log = LaneLog::default();
    let snapshot = PermissionSnapshot {
        level: AgentLevel::ReadOnly,
        thumbnails: false,
        gps: false,
        ai_captions: false,
    };
    log.permission(&Ok((snapshot, 1)));
    log.permission(&Ok((snapshot, 1)));
    assert_eq!(log.lines.len(), 1);
    log.permission(&Ok((snapshot, 2)));
    assert_eq!(log.lines.len(), 2);
    let started = Instant::now();
    for ready in [false, true, false] {
        log.request(
            "miv_status",
            started,
            &Ok(Payload::Status(StatusPayload {
                app_version: format!("private status {ready}"),
                permission: snapshot,
                fts_ready: ready,
                remote_active: ready,
            })),
        );
    }
    assert_eq!(log.lines.len(), 3);
    for message in ["private path C:/private/a.png", "private query"] {
        log.request("miv_status", started, &Err(error(ErrorCode::Busy, message)));
    }
    assert_eq!(log.lines.len(), 4);
    log.request(
        "miv_status",
        started,
        &Err(error(ErrorCode::Internal, "private failure")),
    );
    assert_eq!(log.lines.len(), 5);
    assert!(log.lines.iter().all(|line| !line.contains("private")));
}

#[test]
fn normal_and_perf_emission_gate_quiets_polls_and_keeps_other_requests() {
    let mut log = LaneLog::default();
    let started = Instant::now();
    for (tool, payload) in [
        (
            "hello",
            Payload::Hello {
                version: AGENT_PROTOCOL_VERSION,
            },
        ),
        (
            "miv_status",
            Payload::Status(StatusPayload {
                app_version: "fixture".into(),
                permission: PermissionSnapshot::default(),
                fts_ready: false,
                remote_active: false,
            }),
        ),
        ("cancel", Payload::Cancelled),
    ] {
        for n in 0..20 {
            // This return value is the same gate used by Connection::record
            // for the real perf writer, without enabling process-global perf.
            assert_eq!(
                log.request(tool, started, &Ok(payload.clone())),
                n == 0 || tool == "cancel",
            );
        }
        assert!(log.request(
            tool,
            started,
            &Err(error(ErrorCode::Busy, "private message"))
        ));
        assert_eq!(
            log.request(
                tool,
                started,
                &Err(error(ErrorCode::Busy, "another private message"))
            ),
            tool == "cancel",
        );
        assert!(log.request(
            tool,
            started,
            &Err(error(ErrorCode::Internal, "private failure"))
        ));
        assert!(log.request(tool, started, &Ok(payload)));
    }
    assert_eq!(log.lines.len(), 4 + 4 + 24);
    assert!(log.lines.iter().all(|line| !line.contains("private")));
}
