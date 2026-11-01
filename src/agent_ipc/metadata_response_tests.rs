//! Headless Connection -> preparation -> shared UI dispatch lifecycle coverage.
use super::open::{UiOpenOutcome, UiOpenRequest, UiRequestKind};
use super::tests::Fixture;
use super::*;

fn reference(path: &Path) -> ItemRef {
    ItemRef {
        path: path.to_string_lossy().into_owned(),
        subresource: None,
    }
}

fn commands(item: ItemRef) -> [Command; 3] {
    [
        Command::AddTags {
            items: vec![item.clone()],
            tags: vec!["猫".into()],
        },
        Command::RemoveTags {
            items: vec![item.clone()],
            tags: vec!["猫".into()],
        },
        Command::SetRating {
            items: vec![item],
            stars: 3,
        },
    ]
}

fn expected(command: &Command) -> Payload {
    let rating = matches!(command, Command::SetRating { .. });
    let result = MetadataWriteResult {
        outcomes: vec![if rating {
            MetadataItemOutcome::Changed { indices: vec![0] }
        } else {
            MetadataItemOutcome::Queued { indices: vec![0] }
        }],
    };
    if rating {
        Payload::RatingWrite(result)
    } else {
        Payload::TagWrite(result)
    }
}

fn clock(fixture: &mut Fixture) -> Arc<AtomicUsize> {
    let seconds = Arc::new(AtomicUsize::new(0));
    let current = seconds.clone();
    let base = Instant::now();
    Arc::get_mut(&mut fixture.handlers).unwrap().read_hooks = Some(Arc::new(ReadHooks {
        now: Arc::new(move || base + Duration::from_secs(current.load(Ordering::Acquire) as u64)),
        execute: Box::new(|_, _| panic!("metadata writes must not use the read executor")),
    }));
    seconds
}

fn start(
    connection: &Arc<Connection>,
    command: Command,
) -> (JoinHandle<()>, crossbeam_channel::Receiver<Reply>) {
    let (send, receive) = crossbeam_channel::unbounded();
    let worker = connection
        .request(Request { id: 201, command }, send)
        .unwrap();
    (worker, receive)
}

fn receive(replies: &crossbeam_channel::Receiver<Reply>, worker: JoinHandle<()>) -> Reply {
    let reply = replies.recv_timeout(Duration::from_secs(10)).unwrap();
    worker.join().unwrap();
    assert!(
        replies.try_recv().is_err(),
        "one reply per metadata request"
    );
    reply
}

#[test]
fn metadata_requires_organize_on_every_call_and_uninstalled_ui_is_not_ready() {
    let fixture = Fixture::new();
    let connection = fixture.connection();
    let item = reference(Path::new(r"C:\fixture\not-opened.png"));
    for level in [
        AgentLevel::Disabled,
        AgentLevel::ReadOnly,
        AgentLevel::ReadAndOpen,
    ] {
        fixture.set_level(level);
        for command in commands(item.clone()) {
            assert_eq!(
                fixture
                    .call(&connection, 100, command)
                    .result
                    .unwrap_err()
                    .code,
                ErrorCode::Disabled
            );
        }
    }
    fixture.set_level(AgentLevel::Organize);
    for command in commands(item.clone()) {
        assert_eq!(
            fixture
                .call(&connection, 101, command)
                .result
                .unwrap_err()
                .code,
            ErrorCode::NotReady
        );
    }
    let ui = fixture.handlers.open.install(egui::Context::default());
    fixture.set_level(AgentLevel::ReadAndOpen);
    for command in commands(item) {
        assert_eq!(
            fixture
                .call(&connection, 102, command)
                .result
                .unwrap_err()
                .code,
            ErrorCode::Disabled
        );
    }
    assert!(ui.try_recv().is_err());
    assert_eq!(fixture.handlers.active.load(Ordering::Acquire), 0);
}

#[test]
fn metadata_bad_tag_names_and_batch_limits_never_reach_ui() {
    let fixture = Fixture::new();
    fixture.set_level(AgentLevel::Organize);
    let ui = fixture.handlers.open.install(egui::Context::default());
    let connection = fixture.connection();
    let item = reference(Path::new(r"C:\fixture\never-inspected.png"));
    let mut invalid = vec![];
    for count in [0, MAX_WRITE_ITEMS + 1] {
        for command in commands(item.clone()) {
            invalid.push(match command {
                Command::AddTags { tags, .. } => Command::AddTags {
                    items: vec![item.clone(); count],
                    tags,
                },
                Command::RemoveTags { tags, .. } => Command::RemoveTags {
                    items: vec![item.clone(); count],
                    tags,
                },
                Command::SetRating { stars, .. } => Command::SetRating {
                    items: vec![item.clone(); count],
                    stars,
                },
                _ => unreachable!(),
            });
        }
    }
    for tags in [
        vec![],
        vec!["猫".into(); MAX_WRITE_TAGS + 1],
        vec!["valid".into(), " ".into()],
        vec!["a b".into()],
        vec!["猫".repeat(65)],
    ] {
        invalid.push(Command::AddTags {
            items: vec![item.clone()],
            tags: tags.clone(),
        });
        invalid.push(Command::RemoveTags {
            items: vec![item.clone()],
            tags,
        });
    }
    invalid.push(Command::SetRating {
        items: vec![item],
        stars: 6,
    });
    for command in invalid {
        let response = fixture.call(&connection, 100, command);
        assert_eq!(response.result.unwrap_err().code, ErrorCode::BadRequest);
        assert!(ui.try_recv().is_err());
        assert_eq!(fixture.handlers.active.load(Ordering::Acquire), 0);
    }
}

#[test]
fn metadata_log_names_counts_and_errors_never_include_paths_tags_or_error_details() {
    let secret = "PRIVATE_METADATA_VALUE";
    let item = ItemRef {
        path: format!("C:/{secret}.png"),
        subresource: Some(secret.into()),
    };
    for mut command in commands(item) {
        if let Command::AddTags { tags, .. } | Command::RemoveTags { tags, .. } = &mut command {
            *tags = vec![secret.into()];
        }
        let name = tool_name(&command);
        assert!(matches!(name, "add_tags" | "remove_tags" | "set_rating"));
        let success = Ok(expected(&command));
        assert_eq!(result_summary(&success), (1, "none"));
        let line = request_log_line(name, Instant::now(), &success);
        assert!(line.contains(&format!("tool={name}")));
        assert!(line.contains("count=1"));
        assert!(line.contains("outcome=accepted"));
        assert!(!line.contains(secret));
        assert!(!line.contains("indices"));
        for code in [
            ErrorCode::BadRequest,
            ErrorCode::Busy,
            ErrorCode::Cancelled,
            ErrorCode::Timeout,
            ErrorCode::PermissionChanged,
            ErrorCode::Internal,
        ] {
            let line = request_log_line(name, Instant::now(), &Err(error(code, secret)));
            assert!(line.contains("count=0"));
            assert!(line.contains("outcome=refused"));
            assert!(!line.contains(secret));
        }
    }
}

#[cfg(windows)]
mod filesystem {
    use super::*;

    fn image(fixture: &Fixture) -> ItemRef {
        let path = fixture._dir.path().join("metadata-connection.png");
        std::fs::write(&path, b"headless fixture; no decoding").unwrap();
        reference(&path)
    }

    fn check_kind(request: &UiOpenRequest, command: &Command) {
        match (&request.kind, command) {
            (UiRequestKind::TagWrite(prepared), Command::AddTags { .. }) => {
                assert!(prepared.add);
                assert_eq!(prepared.client_name, "test");
                assert!(prepared.items[0].target.is_ok());
            }
            (UiRequestKind::TagWrite(prepared), Command::RemoveTags { .. }) => {
                assert!(!prepared.add);
                assert_eq!(prepared.client_name, "test");
                assert!(prepared.items[0].target.is_ok());
            }
            (UiRequestKind::RatingWrite(prepared), Command::SetRating { stars, .. }) => {
                assert_eq!(prepared.stars, *stars);
                assert_eq!(prepared.client_name, "test");
                assert!(prepared.items[0].target.is_ok());
            }
            (UiRequestKind::Open(_), Command::OpenInViewer { .. }) => {}
            _ => panic!("prepared UI request kind differs from command"),
        }
    }

    #[test]
    fn metadata_connection_prepares_all_commands_and_preserves_refusal_errors() {
        let fixture = Fixture::new();
        fixture.set_level(AgentLevel::Organize);
        let ui = fixture.handlers.open.install(egui::Context::default());
        let connection = fixture.connection();
        for command in commands(image(&fixture)) {
            let (worker, replies) = start(&connection, command.clone());
            let request = ui.recv_timeout(Duration::from_secs(10)).unwrap();
            check_kind(&request, &command);
            assert!(request.try_claim());
            request.complete(UiOpenOutcome::Refused(error(
                ErrorCode::Busy,
                "UI write gate refused",
            )));
            let reply = receive(&replies, worker);
            assert!(matches!(reply.policy, ReplyPolicy::Rejected));
            let failure = reply.wire().result.unwrap_err();
            assert_eq!(failure.code, ErrorCode::Busy);
            assert_eq!(failure.message, "UI write gate refused");
            assert_eq!(fixture.handlers.active.load(Ordering::Acquire), 0);
        }
    }

    #[test]
    fn open_and_metadata_share_one_ui_slot_across_separate_connections() {
        let fixture = Fixture::new();
        fixture.set_level(AgentLevel::Organize);
        let ui = fixture.handlers.open.install(egui::Context::default());
        let first = fixture.connection();
        let second = fixture.connection();
        let item = image(&fixture);
        let metadata = commands(item.clone());
        let open = Command::OpenInViewer { path: item.path };
        for (held, blocked) in [
            (open.clone(), metadata[0].clone()),
            (metadata[0].clone(), metadata[2].clone()),
            (metadata[2].clone(), open),
        ] {
            let (worker, replies) = start(&first, held.clone());
            let request = ui.recv_timeout(Duration::from_secs(10)).unwrap();
            check_kind(&request, &held);
            assert_eq!(
                fixture.call(&second, 301, blocked).result.unwrap_err().code,
                ErrorCode::Busy
            );
            assert!(ui.try_recv().is_err());
            assert_eq!(fixture.handlers.active.load(Ordering::Acquire), 1);
            assert!(request.try_claim());
            request.complete(if matches!(held, Command::OpenInViewer { .. }) {
                UiOpenOutcome::Admitted
            } else {
                UiOpenOutcome::Written(Ok(expected(&held)))
            });
            assert!(receive(&replies, worker).wire().result.is_ok());
            assert_eq!(fixture.handlers.active.load(Ordering::Acquire), 0);
        }
    }

    #[test]
    fn pending_metadata_cancel_and_deadline_make_late_ui_claim_impossible() {
        for deadline in [false, true] {
            for rating in [false, true] {
                let mut fixture = Fixture::new();
                let seconds = clock(&mut fixture);
                fixture.set_level(AgentLevel::Organize);
                let ui = fixture.handlers.open.install(egui::Context::default());
                let connection = fixture.connection();
                let command = commands(image(&fixture))[if rating { 2 } else { 0 }].clone();
                let (worker, replies) = start(&connection, command);
                let request = ui.recv_timeout(Duration::from_secs(10)).unwrap();
                if deadline {
                    seconds.store(21, Ordering::Release);
                } else {
                    assert!(
                        fixture
                            .call(&connection, 202, Command::Cancel { request_id: 201 })
                            .result
                            .is_ok()
                    );
                }
                let reply = receive(&replies, worker);
                assert!(matches!(reply.policy, ReplyPolicy::Rejected));
                assert_eq!(
                    reply.wire().result.unwrap_err().code,
                    if deadline {
                        ErrorCode::Timeout
                    } else {
                        ErrorCode::Cancelled
                    }
                );
                assert!(!request.try_claim());
                request.complete(UiOpenOutcome::Written(Ok(Payload::Cancelled)));
                assert_eq!(fixture.handlers.active.load(Ordering::Acquire), 0);
                assert!(ui.try_recv().is_err());
            }
        }
    }

    #[test]
    fn claimed_metadata_keeps_written_result_and_original_client_after_all_outer_invalidation() {
        for rating in [false, true] {
            let mut fixture = Fixture::new();
            let seconds = clock(&mut fixture);
            fixture.set_level(AgentLevel::Organize);
            let ui = fixture.handlers.open.install(egui::Context::default());
            let connection = fixture.connection();
            let command = commands(image(&fixture))[if rating { 2 } else { 0 }].clone();
            let (worker, replies) = start(&connection, command.clone());
            let request = ui.recv_timeout(Duration::from_secs(10)).unwrap();
            assert!(request.try_claim());
            seconds.store(21, Ordering::Release);
            assert!(
                fixture
                    .call(&connection, 202, Command::Cancel { request_id: 201 })
                    .result
                    .is_ok()
            );
            connection.disconnect();
            fixture.call(
                &connection,
                203,
                Command::Hello {
                    version: AGENT_PROTOCOL_VERSION,
                    client_name: "later client".into(),
                    client_version: "2".into(),
                    request_kind: RequestKind::Tool,
                },
            );
            fixture.set_level(AgentLevel::ReadOnly);
            check_kind(&request, &command);
            assert!(matches!(
                replies.try_recv(),
                Err(crossbeam_channel::TryRecvError::Empty)
            ));
            assert_eq!(fixture.handlers.active.load(Ordering::Acquire), 1);
            let payload = expected(&command);
            request.complete(UiOpenOutcome::Written(Ok(payload.clone())));
            let reply = receive(&replies, worker);
            assert!(matches!(reply.policy, ReplyPolicy::AcceptedOperation));
            fixture.set_level(AgentLevel::Disabled);
            assert_eq!(reply.wire().result.unwrap(), payload);
            let recent = fixture.handlers.recent_apps.lock().unwrap();
            assert_eq!(recent.entries().len(), 1);
            assert_eq!(recent.entries()[0].client_name, "test");
            assert_eq!(fixture.handlers.active.load(Ordering::Acquire), 0);
        }
    }

    #[test]
    fn written_failure_is_an_accepted_operation_and_survives_permission_drop() {
        let fixture = Fixture::new();
        fixture.set_level(AgentLevel::Organize);
        let ui = fixture.handlers.open.install(egui::Context::default());
        let connection = fixture.connection();
        let command = commands(image(&fixture))[2].clone();
        let (worker, replies) = start(&connection, command);
        let request = ui.recv_timeout(Duration::from_secs(10)).unwrap();
        assert!(request.try_claim());
        request.complete(UiOpenOutcome::Written(Err(error(
            ErrorCode::Internal,
            "accepted write failed",
        ))));
        let reply = receive(&replies, worker);
        assert!(matches!(reply.policy, ReplyPolicy::AcceptedOperation));
        fixture.set_level(AgentLevel::Disabled);
        assert_eq!(reply.wire().result.unwrap_err().code, ErrorCode::Internal);
    }
}
