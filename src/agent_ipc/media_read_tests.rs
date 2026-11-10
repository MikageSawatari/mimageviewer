use super::tests::Fixture;
use super::*;

fn thumbnail_command() -> Command {
    Command::GetThumbnail {
        items: vec![ItemRef {
            path: "C:/fixture/page.png".into(),
            subresource: None,
        }],
        long_side: None,
        cursor: None,
    }
}

fn thumbnail_prefix() -> Payload {
    Payload::Thumbnails(ThumbnailResponse {
        page: ThumbnailPage {
            items: vec![
                ThumbnailItem {
                    item: ItemRef {
                        path: "C:/fixture/page.png".into(),
                        subresource: None,
                    },
                    status: ThumbnailStatus::Ok,
                    dimensions: Some(Dimensions {
                        width: 1,
                        height: 1,
                        unit: DimensionUnit::Pixels,
                    }),
                }
                .into(),
            ],
            cursor: Some(encode_cursor(
                "get_thumbnail",
                serde_json::json!(["fixture"]),
                1,
                0,
            )),
            truncated: true,
        },
        images: vec![ThumbnailImage {
            item_index: 0,
            webp: b"private image bytes".to_vec(),
        }],
    })
}

fn immediate(command: Command, ctx: &ReadContext) -> Result<Payload, Command> {
    match command {
        Command::Hello { version, .. } => Ok(Payload::Hello { version }),
        Command::Status => Ok(Payload::Status(StatusPayload {
            app_version: "fixture".into(),
            permission: ctx.granted.0,
            fts_ready: false,
            remote_active: false,
        })),
        command => Err(command),
    }
}

fn set_thumbnails(allowed: bool) {
    crate::settings_db::with_db_result(|db| {
        let mut settings = db.load_into_settings()?;
        settings.agent_show_thumbnails = allowed;
        db.save_full(&settings)
    })
    .unwrap();
}

fn begin(
    connection: &Arc<Connection>,
    id: u64,
    command: Command,
) -> (JoinHandle<()>, crossbeam_channel::Receiver<Reply>) {
    let (send, recv) = crossbeam_channel::unbounded();
    (
        connection.request(Request { id, command }, send).unwrap(),
        recv,
    )
}

fn receive(recv: &crossbeam_channel::Receiver<Reply>) -> Reply {
    recv.recv_timeout(Duration::from_secs(10))
        .expect("media controller reply")
}

#[test]
fn thumbnail_admission_and_wire_recheck_never_release_revoked_images() {
    for change_during_work in [true, false] {
        let mut fixture = Fixture::new();
        let (started_send, started) = crossbeam_channel::unbounded();
        let (release, gate) = crossbeam_channel::unbounded();
        let reads = Arc::new(AtomicUsize::new(0));
        let worker_reads = reads.clone();
        Arc::get_mut(&mut fixture.handlers).unwrap().read_hooks = Some(Arc::new(ReadHooks {
            now: Arc::new(Instant::now),
            execute: Box::new(move |command, ctx| {
                let command = match immediate(command, ctx) {
                    Ok(result) => return Ok(result),
                    Err(command) => command,
                };
                assert!(matches!(command, Command::GetThumbnail { .. }));
                worker_reads.fetch_add(1, Ordering::AcqRel);
                started_send.send(()).unwrap();
                gate.recv_timeout(Duration::from_secs(10)).unwrap();
                Ok(thumbnail_prefix())
            }),
        }));
        let connection = fixture.connection();
        assert_eq!(
            fixture
                .call(&connection, 1, thumbnail_command())
                .result
                .unwrap_err()
                .code,
            ErrorCode::Disabled
        );
        assert_eq!(reads.load(Ordering::Acquire), 0);
        assert_eq!(fixture.handlers.active.load(Ordering::Acquire), 0);
        set_thumbnails(true);
        let (worker, replies) = begin(&connection, 2, thumbnail_command());
        started.recv_timeout(Duration::from_secs(10)).unwrap();
        if change_during_work {
            set_thumbnails(false);
        }
        release.send(()).unwrap();
        let reply = receive(&replies);
        worker.join().unwrap();
        assert!(matches!(reply.response.result, Ok(Payload::Thumbnails(_))));
        if !change_during_work {
            set_thumbnails(false);
        }
        let response = reply.wire();
        assert_eq!(
            response.result.as_ref().unwrap_err().code,
            ErrorCode::PermissionChanged
        );
        let json = serde_json::to_string(&response).unwrap();
        assert!(!json.contains("images") && !json.contains("webp"));
        assert_eq!(
            fixture
                .call(&connection, 3, thumbnail_command())
                .result
                .unwrap_err()
                .code,
            ErrorCode::Disabled
        );
        assert_eq!(reads.load(Ordering::Acquire), 1);
    }
}

#[test]
fn thumbnail_cursor_revocation_is_disabled_at_admission_before_epoch_validation() {
    let mut fixture = Fixture::new();
    set_thumbnails(true);
    let reads = Arc::new(AtomicUsize::new(0));
    let worker_reads = reads.clone();
    let args = serde_json::json!(["fixture"]);
    let worker_args = args.clone();
    Arc::get_mut(&mut fixture.handlers).unwrap().read_hooks = Some(Arc::new(ReadHooks {
        now: Arc::new(Instant::now),
        execute: Box::new(move |command, ctx| {
            assert!(matches!(
                command,
                Command::GetThumbnail { cursor: None, .. }
            ));
            worker_reads.fetch_add(1, Ordering::AcqRel);
            let Payload::Thumbnails(mut response) = thumbnail_prefix() else {
                unreachable!()
            };
            response.page.cursor = Some(encode_cursor(
                "get_thumbnail",
                worker_args.clone(),
                1,
                ctx.granted.1,
            ));
            Ok(Payload::Thumbnails(response))
        }),
    }));
    let connection = fixture.connection();
    let Payload::Thumbnails(first) = fixture
        .call(&connection, 1, thumbnail_command())
        .result
        .unwrap()
    else {
        panic!("expected thumbnail cursor")
    };
    let cursor = first.page.cursor.expect("fixture has another page");
    assert_eq!(
        decode_cursor(
            Some(&cursor),
            "get_thumbnail",
            &args,
            permission().unwrap().1
        )
        .unwrap(),
        1
    );

    set_thumbnails(false);
    let granted = permission().unwrap();
    assert_eq!(granted.0.level, AgentLevel::ReadOnly);
    assert_eq!(
        decode_cursor(Some(&cursor), "get_thumbnail", &args, granted.1)
            .unwrap_err()
            .code,
        ErrorCode::PermissionChanged
    );
    let Command::GetThumbnail {
        items, long_side, ..
    } = thumbnail_command()
    else {
        unreachable!()
    };
    let (send, recv) = crossbeam_channel::unbounded();
    let worker = connection.request(
        Request {
            id: 2,
            command: Command::GetThumbnail {
                items,
                long_side,
                cursor: Some(cursor),
            },
        },
        send,
    );
    assert!(
        worker.is_none(),
        "revoked tool must be rejected before starting work"
    );
    assert_eq!(
        receive(&recv).wire().result.unwrap_err().code,
        ErrorCode::Disabled
    );
    assert_eq!(reads.load(Ordering::Acquire), 1);
    assert_eq!(fixture.handlers.active.load(Ordering::Acquire), 0);
}

#[test]
fn media_deadline_keeps_both_slots_until_actual_reads_finish() {
    for has_prefix in [true, false] {
        let mut fixture = Fixture::new();
        set_thumbnails(true);
        let base = Instant::now();
        let seconds = Arc::new(AtomicUsize::new(0));
        let clock = seconds.clone();
        let (started_send, started) = crossbeam_channel::unbounded();
        let (release, gate) = crossbeam_channel::unbounded();
        Arc::get_mut(&mut fixture.handlers).unwrap().read_hooks = Some(Arc::new(ReadHooks {
            now: Arc::new(move || base + Duration::from_secs(clock.load(Ordering::Acquire) as u64)),
            execute: Box::new(move |command, ctx| {
                let command = match immediate(command, ctx) {
                    Ok(result) => return Ok(result),
                    Err(command) => command,
                };
                match command {
                    Command::GetThumbnail { .. } if has_prefix => ctx.publish(thumbnail_prefix()),
                    Command::GetThumbnail { .. } | Command::ListFolder { .. } => {}
                    _ => panic!("unexpected media request"),
                }
                started_send.send(ctx.cancel.clone()).unwrap();
                gate.recv_timeout(Duration::from_secs(10)).unwrap();
                ctx.checkpoint()?;
                Ok(thumbnail_prefix())
            }),
        }));
        let a = fixture.connection();
        let b = fixture.connection();
        let c = fixture.connection();
        let (worker_a, replies_a) = begin(&a, 10, thumbnail_command());
        let cancel_a = started.recv_timeout(Duration::from_secs(10)).unwrap();
        let (worker_b, replies_b) = begin(
            &b,
            20,
            Command::ListFolder {
                min_stars: None,
                max_stars: None,
                kinds: None,
                path: "C:/fixture".into(),
                subresource: None,
                limit: None,
                cursor: None,
            },
        );
        let cancel_b = started.recv_timeout(Duration::from_secs(10)).unwrap();
        seconds.store(20, Ordering::Release);
        let result = receive(&replies_a).wire().result;
        if has_prefix {
            let Payload::Thumbnails(response) = result.unwrap() else {
                panic!("thumbnail prefix")
            };
            assert_eq!(response.page.items.len(), 1);
            assert_eq!(response.images.len(), 1);
            assert_eq!(
                decode_cursor(
                    response.page.cursor.as_deref(),
                    "get_thumbnail",
                    &serde_json::json!(["fixture"]),
                    0
                )
                .unwrap(),
                1
            );
            assert!(response.page.truncated);
        } else {
            assert_eq!(result.unwrap_err().code, ErrorCode::Timeout);
        }
        assert_eq!(
            receive(&replies_b).wire().result.unwrap_err().code,
            ErrorCode::Timeout
        );
        assert!(cancel_a.load(Ordering::Acquire) && cancel_b.load(Ordering::Acquire));
        assert_eq!(fixture.handlers.active.load(Ordering::Acquire), 2);
        assert_eq!(
            fixture
                .call(&c, 30, Command::Status)
                .result
                .unwrap_err()
                .code,
            ErrorCode::Busy
        );
        assert!(!worker_a.is_finished() && !worker_b.is_finished());
        release.send(()).unwrap();
        release.send(()).unwrap();
        worker_a.join().unwrap();
        worker_b.join().unwrap();
        assert_eq!(fixture.handlers.active.load(Ordering::Acquire), 0);
    }
}

#[test]
fn shared_composition_snapshot_remains_readable_after_admission_closes() {
    let fixture = Fixture::new();
    let readers = &fixture.handlers.readers;
    assert!(readers.book_composition().is_none());
    let (service, owner) = crate::book_composition::BookCompositionService::spawn_at(
        fixture._dir.path().to_path_buf(),
        None,
    )
    .unwrap();
    let scope =
        crate::book_scope::BookScopeKey::document(fixture._dir.path().join("book.pdf")).unwrap();
    assert!(
        service
            .prepare(scope, crate::book_composition::PageDomain::Document)
            .recv_timeout(Duration::from_secs(10))
            .unwrap()
            .status
            .is_ok()
    );
    readers.install_book_composition(service.clone());
    let borrowed = readers.book_composition().unwrap();
    assert!(Arc::ptr_eq(&borrowed.snapshot(), &service.snapshot()));
    service.close_admission();
    assert!(borrowed.current_candidate_snapshot().is_none());
    assert!(borrowed.snapshot().status.is_ok());
    owner.shutdown_and_join();
    assert!(
        readers
            .book_composition()
            .unwrap()
            .snapshot()
            .status
            .is_ok()
    );
}
