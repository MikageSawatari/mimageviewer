use super::tests::Fixture;
use super::*;
use std::io::Write;

fn item_payload() -> Payload {
    Payload::Items(ItemPage {
        items: vec![
            Item {
                path: "C:/fixture/prefix.png".into(),
                subresource: None,
                name: "prefix.png".into(),
                kind: ItemKind::Image,
                tags: Some(vec![]),
                container_tags: None,
                rating: 0,
            }
            .into(),
        ],
        cursor: Some(encode_cursor(
            "search_items",
            serde_json::json!(["fixture", "path"]),
            1,
            0,
        )),
        truncated: true,
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

fn start(
    connection: &Arc<Connection>,
    id: u64,
    command: Command,
) -> (JoinHandle<()>, crossbeam_channel::Receiver<Reply>) {
    let (send, recv) = crossbeam_channel::unbounded();
    let worker = connection
        .request(Request { id, command }, send)
        .expect("a valid read should start its actual controller");
    (worker, recv)
}
fn receive(recv: &crossbeam_channel::Receiver<Reply>) -> Reply {
    recv.recv_timeout(Duration::from_secs(10))
        .expect("controller response")
}
fn search() -> Command {
    Command::SearchItems {
        query: "fixture".into(),
        kind: None,
        limit: Some(1),
        cursor: None,
    }
}
fn collection() -> Command {
    Command::GetCollection {
        id: Some("fixture".into()),
        name: None,
        limit: Some(1),
        cursor: None,
    }
}

#[test]
fn real_controllers_return_advancing_prefix_or_timeout_and_retain_worker_permits() {
    for publish_prefix in [true, false] {
        let mut f = Fixture::new();
        let base = Instant::now();
        let seconds = Arc::new(AtomicUsize::new(0));
        let clock = seconds.clone();
        let (started_send, started) = crossbeam_channel::unbounded();
        let (release, gate) = crossbeam_channel::unbounded();
        Arc::get_mut(&mut f.handlers).unwrap().read_hooks = Some(Arc::new(ReadHooks {
            now: Arc::new(move || base + Duration::from_secs(clock.load(Ordering::Acquire) as u64)),
            execute: Box::new(move |command, ctx| {
                let command = match immediate(command, ctx) {
                    Ok(value) => return Ok(value),
                    Err(command) => command,
                };
                match command {
                    Command::SearchItems { .. } if publish_prefix => ctx.publish(item_payload()),
                    Command::SearchItems { .. } | Command::GetCollection { .. } => {}
                    _ => panic!("unexpected gated command"),
                }
                // ReadContext's deadline and optional progress already exist.
                started_send.send(ctx.cancel.clone()).unwrap();
                gate.recv_timeout(Duration::from_secs(10)).unwrap();
                ctx.checkpoint()?;
                Ok(item_payload())
            }),
        }));
        let a = f.connection();
        let b = f.connection();
        let c = f.connection();
        let (worker_a, replies_a) = start(&a, 10, search());
        let cancel_a = started.recv_timeout(Duration::from_secs(10)).unwrap();
        let (worker_b, replies_b) = start(&b, 20, collection());
        let cancel_b = started.recv_timeout(Duration::from_secs(10)).unwrap();
        seconds.store(20, Ordering::Release);
        let first = receive(&replies_a).wire();
        if publish_prefix {
            let Payload::Items(page) = first.result.unwrap() else {
                panic!("expected published prefix");
            };
            assert_eq!(page.items.len(), 1);
            assert!(page.truncated);
            assert_eq!(
                decode_cursor(
                    page.cursor.as_deref(),
                    "search_items",
                    &serde_json::json!(["fixture", "path"]),
                    0
                )
                .unwrap(),
                1
            );
        } else {
            assert_eq!(first.result.unwrap_err().code, ErrorCode::Timeout);
        }
        assert_eq!(
            receive(&replies_b).wire().result.unwrap_err().code,
            ErrorCode::Timeout
        );
        assert!(cancel_a.load(Ordering::Acquire));
        assert!(cancel_b.load(Ordering::Acquire));
        assert_eq!(f.handlers.active.load(Ordering::Acquire), 2);
        assert_eq!(
            f.call(&a, 11, Command::Status).result.unwrap_err().code,
            ErrorCode::Busy
        );
        assert_eq!(
            f.call(&c, 30, Command::Status).result.unwrap_err().code,
            ErrorCode::Busy
        );
        assert!(!worker_a.is_finished());
        assert!(!worker_b.is_finished());
        release.send(()).unwrap();
        release.send(()).unwrap();
        worker_a.join().unwrap();
        worker_b.join().unwrap();
        assert_eq!(f.handlers.active.load(Ordering::Acquire), 0);
        assert!(f.call(&c, 31, Command::Status).result.is_ok());
    }
}

#[test]
fn real_controller_cancel_and_disconnect_keep_slots_until_gated_reads_finish() {
    let mut f = Fixture::new();
    let base = Instant::now();
    let (started_send, started) = crossbeam_channel::unbounded();
    let (release, gate) = crossbeam_channel::unbounded();
    Arc::get_mut(&mut f.handlers).unwrap().read_hooks = Some(Arc::new(ReadHooks {
        now: Arc::new(move || base),
        execute: Box::new(move |command, ctx| {
            match immediate(command, ctx) {
                Ok(value) => return Ok(value),
                Err(_) => {}
            }
            started_send.send(ctx.cancel.clone()).unwrap();
            gate.recv_timeout(Duration::from_secs(10)).unwrap();
            ctx.checkpoint()?;
            Ok(item_payload())
        }),
    }));
    let a = f.connection();
    let b = f.connection();
    let c = f.connection();
    let (worker_a, replies_a) = start(&a, 10, search());
    let cancel_a = started.recv_timeout(Duration::from_secs(10)).unwrap();
    let (worker_b, replies_b) = start(&b, 20, collection());
    let cancel_b = started.recv_timeout(Duration::from_secs(10)).unwrap();
    // Another connection cannot cancel this identity.
    assert!(
        f.call(&c, 1, Command::Cancel { request_id: 10 })
            .result
            .is_ok()
    );
    assert!(!cancel_a.load(Ordering::Acquire));
    assert!(
        f.call(&a, 2, Command::Cancel { request_id: 10 })
            .result
            .is_ok()
    );
    assert_eq!(
        receive(&replies_a).wire().result.unwrap_err().code,
        ErrorCode::Cancelled
    );
    b.disconnect();
    assert_eq!(
        receive(&replies_b).wire().result.unwrap_err().code,
        ErrorCode::Cancelled
    );
    assert!(cancel_a.load(Ordering::Acquire));
    assert!(cancel_b.load(Ordering::Acquire));
    assert_eq!(f.handlers.active.load(Ordering::Acquire), 2);
    assert_eq!(
        f.call(&c, 3, Command::Status).result.unwrap_err().code,
        ErrorCode::Busy
    );
    release.send(()).unwrap();
    release.send(()).unwrap();
    worker_a.join().unwrap();
    worker_b.join().unwrap();
    assert_eq!(f.handlers.active.load(Ordering::Acquire), 0);
    assert!(f.call(&c, 4, Command::Status).result.is_ok());
}

fn set_snapshot(snapshot: PermissionSnapshot) {
    crate::settings_db::with_db_result(|db| {
        let mut settings = db.load_into_settings()?;
        settings.agent_level = snapshot.level;
        settings.agent_show_thumbnails = snapshot.thumbnails;
        settings.agent_show_gps = snapshot.gps;
        settings.agent_show_ai_captions = snapshot.ai_captions;
        db.save_full(&settings)
    })
    .unwrap();
}
fn revoke_or_cycle(which: usize, original: PermissionSnapshot) {
    match which {
        0 => set_snapshot(PermissionSnapshot {
            thumbnails: false,
            ..original
        }),
        1 => set_snapshot(PermissionSnapshot {
            gps: false,
            ..original
        }),
        2 => {
            set_snapshot(PermissionSnapshot {
                level: AgentLevel::Disabled,
                thumbnails: false,
                gps: false,
                ai_captions: false,
            });
            set_snapshot(original);
        }
        3 => set_snapshot(PermissionSnapshot {
            ai_captions: false,
            ..original
        }),
        _ => unreachable!(),
    }
}

#[test]
fn real_read_results_recheck_whole_snapshot_during_work_and_at_wire_boundary() {
    for change_before_worker_finishes in [true, false] {
        for field in 0..4 {
            let mut f = Fixture::new();
            let base = Instant::now();
            let original = PermissionSnapshot {
                level: AgentLevel::ReadOnly,
                thumbnails: true,
                gps: true,
                ai_captions: true,
            };
            set_snapshot(original);
            let granted_epoch = permission().unwrap().1;
            let (started_send, started) = crossbeam_channel::unbounded();
            let (release, gate) = crossbeam_channel::unbounded();
            Arc::get_mut(&mut f.handlers).unwrap().read_hooks = Some(Arc::new(ReadHooks {
                now: Arc::new(move || base),
                execute: Box::new(move |command, ctx| {
                    match immediate(command, ctx) {
                        Ok(value) => return Ok(value),
                        Err(_) => {}
                    }
                    started_send.send(ctx.granted.0).unwrap();
                    gate.recv_timeout(Duration::from_secs(10)).unwrap();
                    ctx.checkpoint()?;
                    Ok(item_payload())
                }),
            }));
            let c = f.connection();
            let (worker, replies) = start(&c, 10, search());
            assert_eq!(
                started.recv_timeout(Duration::from_secs(10)).unwrap(),
                original
            );
            if change_before_worker_finishes {
                revoke_or_cycle(field, original);
            }
            release.send(()).unwrap();
            let reply = receive(&replies);
            worker.join().unwrap();
            if !change_before_worker_finishes {
                revoke_or_cycle(field, original);
            }
            assert_ne!(permission().unwrap().1, granted_epoch);
            if field == 2 {
                assert_eq!(permission().unwrap().0, original);
            }
            assert_eq!(
                reply.wire().result.unwrap_err().code,
                ErrorCode::PermissionChanged
            );
            assert_eq!(f.handlers.active.load(Ordering::Acquire), 0);
        }
    }
}

#[test]
fn status_snapshot_staleness_is_distinct_from_actual_disable() {
    let f = Fixture::new();
    let c = f.connection();
    let original = permission().unwrap().0;
    for changed in [
        PermissionSnapshot {
            ai_captions: !original.ai_captions,
            ..original
        },
        PermissionSnapshot {
            gps: !original.gps,
            ..original
        },
        PermissionSnapshot {
            thumbnails: !original.thumbnails,
            ..original
        },
    ] {
        set_snapshot(original);
        let (worker, replies) = start(&c, 10, Command::Status);
        let reply = receive(&replies);
        worker.join().unwrap();
        assert!(matches!(reply.response.result, Ok(Payload::Status(_))));
        set_snapshot(changed);
        let stale = reply.wire().result.unwrap_err();
        assert_eq!(stale.code, ErrorCode::PermissionChanged);
        assert_ne!(stale.code, ErrorCode::Disabled);
        let Payload::Status(fresh) = f.call(&c, 11, Command::Status).result.unwrap() else {
            panic!("expected fresh permission snapshot");
        };
        assert_eq!(fresh.permission, changed);
    }
    set_snapshot(PermissionSnapshot {
        level: AgentLevel::Disabled,
        ..original
    });
    assert_eq!(
        f.call(&c, 12, Command::Status).result.unwrap_err().code,
        ErrorCode::Disabled
    );
}

fn zip_file(path: &Path, contents: &[u8]) {
    let mut zip = zip::ZipWriter::new(std::fs::File::create(path).unwrap());
    zip.start_file(
        "page.png",
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored),
    )
    .unwrap();
    zip.write_all(contents).unwrap();
    zip.finish().unwrap();
}
fn info_row(row: &PageRow<ItemInfo>) -> &ItemInfo {
    match row {
        PageRow::Value(value) => value,
        PageRow::TooLong(_) => panic!("small fixture row should fit"),
    }
}

#[test]
fn info_cursor_for_fifty_long_paths_resumes_without_reading_deleted_prefix() {
    let f = Fixture::new();
    let root = crate::data_dir::get();
    let items = (0..MAX_INFO_LIMIT)
        .map(|i| ItemRef {
            path: root
                .join(format!("{i:02}-{}-book.zip", "x".repeat(130)))
                .to_string_lossy()
                .into_owned(),
            subresource: Some("page.png".into()),
        })
        .collect::<Vec<_>>();
    assert!(serde_json::to_string(&items).unwrap().len() > 8192);
    zip_file(Path::new(&items[0].path), b"first");
    zip_file(Path::new(&items[1].path), b"second");
    let c = f.connection();
    let Payload::Info(first) = f
        .call(
            &c,
            1,
            Command::GetItemInfo {
                items: items.clone(),
                limit: Some(1),
                cursor: None,
            },
        )
        .result
        .unwrap()
    else {
        panic!("expected info page");
    };
    assert_eq!(first.items.len(), 1);
    assert!(first.truncated);
    assert_eq!(info_row(&first.items[0]).item, items[0]);
    assert!(matches!(
        info_row(&first.items[0]).size,
        InfoField::Present(5)
    ));
    let cursor = first.cursor.expect("another input item remains");
    assert!(cursor.len() < 8192);
    std::fs::remove_file(&items[0].path).unwrap();
    // Reading the old ZIP reference now fails. Successful continuation therefore
    // proves the prefix is skipped before metadata/ZIP reads, rather than after.
    assert_eq!(
        item_info::read(&items[0], permission().unwrap().0, &AtomicBool::new(false))
            .unwrap_err()
            .code,
        ErrorCode::BadRequest
    );
    let Payload::Info(second) = f
        .call(
            &c,
            2,
            Command::GetItemInfo {
                items: items.clone(),
                limit: Some(1),
                cursor: Some(cursor),
            },
        )
        .result
        .unwrap()
    else {
        panic!("expected next info page");
    };
    assert_eq!(second.items.len(), 1);
    assert_eq!(info_row(&second.items[0]).item, items[1]);
    assert!(matches!(
        info_row(&second.items[0]).size,
        InfoField::Present(6)
    ));
    assert!(
        second
            .cursor
            .as_ref()
            .is_some_and(|cursor| cursor.len() < 8192)
    );
}
