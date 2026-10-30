use super::*;

struct Fixture {
    handlers: Arc<Handlers>,
    runtime: Option<crate::collection_store::CollectionStoreRuntime>,
    _dir: crate::settings_db::DataDirOverrideGuard,
}
impl Fixture {
    fn new() -> Self {
        let dir = crate::settings_db::DataDirOverrideGuard::new();
        let settings = crate::settings::Settings {
            agent_level: AgentLevel::ReadOnly,
            ..Default::default()
        };
        let db = crate::settings_db::SettingsDb::create_new(dir.path()).unwrap();
        db.save_full(&settings).unwrap();
        drop(db);
        let runtime = crate::collection_store::CollectionStoreRuntime::start_at(
            dir.path().join("collection.db"),
        )
        .unwrap();
        let handlers = Arc::new(Handlers {
            readers: crate::remote_ipc::SharedReaders::open(&settings).unwrap(),
            producer: Some(
                crate::collection_store::CollectionRemoteProducerControl::new(runtime.client()),
            ),
            session: None,
            active: AtomicUsize::new(0),
        });
        Self {
            _dir: dir,
            runtime: Some(runtime),
            handlers,
        }
    }
    fn set_level(&self, agent_level: AgentLevel) {
        crate::settings_db::with_db_result(|db| {
            let mut s = db.load_into_settings()?;
            s.agent_level = agent_level;
            db.save_full(&s)
        })
        .unwrap();
    }
    fn call(&self, connection: &Arc<Connection>, id: u64, command: Command) -> Response {
        let (tx, rx) = crossbeam_channel::unbounded();
        let worker = connection.request(Request { id, command }, tx);
        let response = rx.recv_timeout(Duration::from_secs(10)).unwrap().wire();
        if let Some(worker) = worker {
            worker.join().unwrap();
        }
        response
    }
    fn connection(&self) -> Arc<Connection> {
        let c = Connection::new(self.handlers.clone());
        assert!(
            self.call(
                &c,
                0,
                Command::Hello {
                    version: AGENT_PROTOCOL_VERSION,
                    client_name: "test".into()
                }
            )
            .result
            .is_ok()
        );
        c
    }

    fn seed_index(&self) {
        use crate::fts_index::{Container, FtsIndex, IndexDoc, IndexKind, upsert_doc};
        let favorite =
            crate::settings::FavoriteEntry::new("fixture".into(), crate::data_dir::get());
        crate::settings_db::with_db_result(|db| {
            let mut s = db.load_into_settings()?;
            let mut favorite = favorite.clone();
            favorite.auto_index_metadata = true;
            s.favorites = vec![favorite];
            db.save_full(&s)
        })
        .unwrap();
        let fts = FtsIndex::open_at(&crate::data_dir::get().join("fts_index")).unwrap();
        {
            let mut writer = fts.writer().unwrap();
            for i in 0..4 {
                let doc = IndexDoc {
                    path: format!("C:/fixture/cat{i}.png"),
                    container: Container::Fs,
                    zip_entry: String::new(),
                    favorite_id: favorite.id,
                    kind: IndexKind::Image,
                    mtime: 0,
                    file_size: 0,
                    norms: crate::ingest_text::PerSourceText {
                        name: format!("cat{i}"),
                        ..Default::default()
                    },
                };
                upsert_doc(&writer, fts.fields(), &doc).unwrap();
            }
            writer.commit().unwrap();
            fts.reload_reader().unwrap();
        }
        *self.handlers.readers.fts.write().unwrap() = Some(Arc::new(fts));
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.handlers.producer.as_ref().unwrap().close_and_drain();
        self.runtime.take().unwrap().shutdown_and_join();
    }
}

#[test]
fn permission_is_live_and_handshake_is_strict() {
    let f = Fixture::new();
    let c = Connection::new(f.handlers.clone());
    assert_eq!(
        f.call(&c, 1, Command::Status).result.unwrap_err().code,
        ErrorCode::BadRequest
    );
    assert_eq!(
        f.call(
            &c,
            2,
            Command::Hello {
                version: 999,
                client_name: "test".into()
            }
        )
        .result
        .unwrap_err()
        .code,
        ErrorCode::VersionMismatch
    );
    let c = f.connection();
    assert!(f.call(&c, 3, Command::Status).result.is_ok());
    f.set_level(AgentLevel::Disabled);
    for command in [
        Command::Status,
        Command::ListTags {
            choice: TagChoice::All,
        },
        Command::Cancel { request_id: 3 },
    ] {
        assert_eq!(
            f.call(&c, 4, command).result.unwrap_err().code,
            ErrorCode::Disabled
        );
    }
    f.set_level(AgentLevel::ReadOnly);
    assert!(f.call(&c, 5, Command::Status).result.is_ok());
    assert_eq!(
        f.call(
            &c,
            6,
            Command::SearchItems {
                query: "猫猫".into(),
                kind: None,
                limit: None,
                cursor: None
            }
        )
        .result
        .unwrap_err()
        .code,
        ErrorCode::NotReady
    );
}

#[test]
fn admission_cancel_disconnect_and_slot_lifetime() {
    let f = Fixture::new();
    let a = f.connection();
    let b = f.connection();
    let c = f.connection();
    // Hold real admission owners without relying on Tantivy timing.
    let (pa, flag) = a.acquire(10).unwrap();
    let (pb, _) = b.acquire(20).unwrap();
    assert_eq!(
        f.call(&a, 11, Command::Status).result.unwrap_err().code,
        ErrorCode::Busy
    );
    assert_eq!(
        f.call(&c, 21, Command::Status).result.unwrap_err().code,
        ErrorCode::Busy
    );
    f.call(&c, 22, Command::Cancel { request_id: 10 });
    assert!(!flag.load(Ordering::Acquire));
    f.call(&a, 23, Command::Cancel { request_id: 10 });
    assert!(flag.load(Ordering::Acquire));
    assert_eq!(f.handlers.active.load(Ordering::Acquire), 2);
    b.disconnect();
    assert!(
        b.running
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .cancel
            .load(Ordering::Acquire)
    );
    drop(pa);
    assert!(f.call(&c, 24, Command::Status).result.is_ok());
    drop(pb);
    assert_eq!(f.handlers.active.load(Ordering::Acquire), 0);
}

#[test]
fn deadline_returns_partial_without_releasing_the_running_worker_slot() {
    let f = Fixture::new();
    let a = f.connection();
    let b = f.connection();
    let (permit, cancel) = a.acquire(10).unwrap();
    let (other, _) = b.acquire(11).unwrap();
    let (tx, rx) = crossbeam_channel::unbounded();
    tx.send(crate::global_search::SearchStreamEvent::Batch {
        hits: vec![crate::global_search::GlobalHit {
            path: "C:/cat.png".into(),
            score: 1.,
            stars: 0,
            mtime: 0,
            file_size: Some(0),
        }],
        scanned_candidates: 1,
        valid_hits: 1,
        visibility_notice: None,
    })
    .unwrap();
    let start = Instant::now();
    let calls = AtomicUsize::new(0);
    let (items, truncated) =
        collect_search(&rx, &cancel, 50, start + Duration::from_secs(20), || {
            if calls.fetch_add(1, Ordering::Relaxed) < 2 {
                start
            } else {
                start + Duration::from_secs(20)
            }
        })
        .unwrap();
    assert_eq!(items.len(), 1);
    assert!(truncated);
    assert_eq!(f.handlers.active.load(Ordering::Acquire), 2);
    assert_eq!(
        f.call(&a, 12, Command::Status).result.unwrap_err().code,
        ErrorCode::Busy
    );
    cancel.store(true, Ordering::Release);
    assert_eq!(
        collect_search(&rx, &cancel, 50, start + Duration::from_secs(20), || start)
            .unwrap_err()
            .code,
        ErrorCode::Cancelled
    );
    drop(permit);
    drop(other);
}

#[test]
fn cursor_continuation_and_character_budget() {
    let f = Fixture::new();
    let items: Vec<_> = (0..8)
        .map(|i| Item {
            path: format!("C:\\data\\{i}.png"),
            subresource: None,
            name: format!("{i}{}", "猫".repeat(5000)),
            kind: ItemKind::Image,
            tags: vec![],
            rating: 0,
        })
        .collect();
    let args = serde_json::json!(["猫", null, "path"]);
    let one = page(items.clone(), 0, 5, false, "items_by_tag", args.clone()).unwrap();
    assert!(one.truncated);
    assert!(json_chars(&one) <= MAX_PAGE_CHARS);
    let offset = decode_cursor(one.cursor.as_deref(), "items_by_tag", &args).unwrap();
    let two = page(items, offset, 5, false, "items_by_tag", args.clone()).unwrap();
    assert_ne!(one.items[0].path, two.items[0].path);
    assert_eq!(
        decode_cursor(one.cursor.as_deref(), "search_items", &args)
            .unwrap_err()
            .message,
        "最初からやり直してください"
    );
    let bad = encode_cursor("items_by_tag", args.clone(), 1);
    assert!(decode_cursor(Some(&bad), "items_by_tag", &serde_json::json!([])).is_err());
    drop(f);
}

#[test]
fn queued_results_are_revoked_even_after_reenable_and_unrelated_saves_are_allowed() {
    let f = Fixture::new();
    let (_, epoch) = permission().unwrap();
    let reply = || Reply {
        response: Response {
            id: 1,
            result: Ok(Payload::Items(ItemPage {
                items: vec![],
                cursor: None,
                truncated: false,
            })),
        },
        permission_epoch: epoch,
    };
    crate::settings_db::with_db_result(|db| {
        let mut s = db.load_into_settings()?;
        s.notification_duration = crate::settings::Settings::default().notification_duration;
        db.save_full(&s)
    })
    .unwrap();
    assert!(reply().wire().result.is_ok());
    f.set_level(AgentLevel::Disabled);
    f.set_level(AgentLevel::ReadOnly);
    assert_eq!(reply().wire().result.unwrap_err().code, ErrorCode::Disabled);
    assert_ne!(epoch, permission().unwrap().1);
}

#[test]
fn fts_fixture_search_uses_live_favorites_and_has_cursor() {
    let f = Fixture::new();
    f.seed_index();
    let c = f.connection();
    let Payload::Items(first) = f
        .call(
            &c,
            1,
            Command::SearchItems {
                query: "cat".into(),
                kind: Some(ItemKind::Image),
                limit: Some(2),
                cursor: None,
            },
        )
        .result
        .unwrap()
    else {
        panic!()
    };
    assert_eq!(first.items.len(), 2);
    assert!(first.truncated);
    let Payload::Items(second) = f
        .call(
            &c,
            2,
            Command::SearchItems {
                query: "cat".into(),
                kind: Some(ItemKind::Image),
                limit: Some(2),
                cursor: first.cursor,
            },
        )
        .result
        .unwrap()
    else {
        panic!()
    };
    assert_eq!(second.items.len(), 2);
    assert!(
        first
            .items
            .iter()
            .all(|a| second.items.iter().all(|b| a.path != b.path))
    );
    let Payload::Items(other) = f
        .call(
            &c,
            3,
            Command::SearchItems {
                query: "cat".into(),
                kind: Some(ItemKind::Other),
                limit: None,
                cursor: None,
            },
        )
        .result
        .unwrap()
    else {
        panic!()
    };
    assert!(other.items.is_empty());
}

/// This one test launches the MCP console adapter only, never the product core.
/// It is kept separate because this implementation brief prohibits all product exe launches.
#[cfg(windows)]
#[test]
#[ignore = "requires explicit approval to launch the MCP exe against disposable fixtures"]
fn stdio_exe_uses_real_core_fixture_handlers() {
    use serde_json::json;
    use std::io::{BufRead, Write};
    let f = Fixture::new();
    f.seed_index();
    let path = crate::data_dir::get().join("cat.png");
    std::fs::write(&path, []).unwrap();
    let mut tags = crate::tags_db::TagsDb::open().unwrap();
    tags.set_item_tags(
        &crate::tags_db::item_key_for_path(&path),
        ["cat"],
        crate::tags_db::source::EDIT,
    )
    .unwrap();
    f.runtime
        .as_ref()
        .unwrap()
        .client()
        .create_collection("fixture collection".into())
        .unwrap()
        .recv()
        .unwrap()
        .unwrap();
    // The real core handler, Connection admission, wire codec and pipe transport are used.
    let server = super::pipe::fixture_server(f.handlers.clone()).unwrap();
    let exe = std::env::current_exe()
        .unwrap()
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("mimageviewer-mcp.exe");
    struct ChildGuard(std::process::Child);
    impl Drop for ChildGuard {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    let mut child = ChildGuard(
        std::process::Command::new(exe)
            .args(["--data-dir"])
            .arg(crate::data_dir::get())
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap(),
    );
    let mut input = child.0.stdin.take().unwrap();
    let mut output = std::io::BufReader::new(child.0.stdout.take().unwrap());
    let (tx, rx) = crossbeam_channel::bounded(8);
    let reader = std::thread::spawn(move || {
        loop {
            let mut line = String::new();
            if output.read_line(&mut line).unwrap() == 0 {
                break;
            }
            if tx.send(line).is_err() {
                break;
            }
        }
    });
    let meta = json!({"io.modelcontextprotocol/protocolVersion":"2026-07-28","io.modelcontextprotocol/clientCapabilities":{}});
    for (id, name, arguments) in [
        (1, "miv_status", json!({})),
        (2, "list_tags", json!({"choice":"all"})),
        (3, "items_by_tag", json!({"tag":"cat"})),
        (4, "list_collections", json!({})),
        (5, "search_items", json!({"query":"cat","limit":2})),
    ] {
        writeln!(input, "{}", json!({"jsonrpc":"2.0","id":id,"method":"tools/call","params":{"name":name,"arguments":arguments,"_meta":meta}})).unwrap();
        input.flush().unwrap();
        let line = rx.recv_timeout(Duration::from_secs(10)).unwrap();
        let result: serde_json::Value = serde_json::from_str(&line).unwrap();
        assert_eq!(result["id"], id);
        assert_eq!(result["result"]["isError"], false, "{result}");
        let structured = &result["result"]["structuredContent"];
        let text: serde_json::Value =
            serde_json::from_str(result["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(structured, &text);
        assert!(line.chars().count() <= MAX_MCP_RESPONSE_CHARS);
        match name {
            "miv_status" => assert_eq!(structured["fts_ready"], true),
            "list_tags" => assert_eq!(structured["tags"][0]["name"], "cat"),
            "items_by_tag" => assert_eq!(structured["items"][0]["tags"][0], "#cat"),
            "list_collections" => {
                assert_eq!(structured["collections"][0]["name"], "fixture collection")
            }
            "search_items" => assert_eq!(structured["items"].as_array().unwrap().len(), 2),
            _ => unreachable!(),
        }
    }
    drop(input);
    let deadline = Instant::now() + Duration::from_millis(300);
    loop {
        if let Some(status) = child.0.try_wait().unwrap() {
            assert!(status.success());
            break;
        }
        if Instant::now() >= deadline {
            let _ = child.0.kill();
            panic!("MCP exe failed to exit within 300 ms after EOF");
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    reader.join().unwrap();
    drop(server);
}

#[test]
fn tags_use_the_projection_and_real_rating_database() {
    let f = Fixture::new();
    let root = crate::data_dir::get();
    let path = root.join("test.png");
    std::fs::write(&path, []).unwrap();
    let mut db = crate::tags_db::TagsDb::open().unwrap();
    db.set_item_tags(
        &crate::tags_db::item_key_for_path(&path),
        ["cat"],
        crate::tags_db::source::EDIT,
    )
    .unwrap();
    let ratings = crate::rating_db::RatingDb::open().unwrap();
    ratings
        .set(&crate::adjustment_db::normalize_path(&path), 4)
        .unwrap();
    let actor = f.runtime.as_ref().unwrap().client();
    let collection = actor
        .create_collection("fixture collection".into())
        .unwrap()
        .recv()
        .unwrap()
        .unwrap();
    actor
        .add_batch(
            collection.definition.id,
            collection.definition.revision,
            vec![
                crate::collection_store::CollectionRegistration::from_trusted_path(
                    &path,
                    crate::collection_store::CollectionResolvedKind::Image,
                )
                .unwrap(),
            ],
        )
        .unwrap()
        .recv()
        .unwrap()
        .unwrap();
    let c = f.connection();
    let Payload::Items(page) = f
        .call(
            &c,
            1,
            Command::ItemsByTag {
                tag: "cat".into(),
                kind: None,
                limit: None,
                cursor: None,
            },
        )
        .result
        .unwrap()
    else {
        panic!()
    };
    assert_eq!(page.items.len(), 1);
    assert_eq!(page.items[0].rating, 4);
    assert_eq!(page.items[0].tags, ["#cat"]);
    let Payload::Tags(page) = f
        .call(
            &c,
            2,
            Command::ListTags {
                choice: TagChoice::All,
            },
        )
        .result
        .unwrap()
    else {
        panic!()
    };
    assert_eq!(page.tags[0].name, "cat");
    assert_eq!(page.tags[0].count, 1);
    let Payload::Collections(page) = f.call(&c, 3, Command::ListCollections).result.unwrap() else {
        panic!()
    };
    assert_eq!(page.collections[0].name, "fixture collection");
    assert_eq!(page.collections[0].count, 1);
}
