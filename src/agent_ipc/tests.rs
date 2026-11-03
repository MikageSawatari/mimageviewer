use super::*;

fn row<T>(value: &PageRow<T>) -> &T {
    match value {
        PageRow::Value(value) => value,
        PageRow::TooLong(_) => panic!("fixture row must fit the item budget"),
    }
}

pub(super) struct Fixture {
    pub(super) handlers: Arc<Handlers>,
    pub(super) runtime: Option<crate::collection_store::CollectionStoreRuntime>,
    pub(super) _dir: crate::settings_db::DataDirOverrideGuard,
}
impl Fixture {
    pub(super) fn new() -> Self {
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
        match runtime
            .recv_event_timeout_for_test(Duration::from_secs(10))
            .expect("collection runtime startup event")
        {
            crate::collection_store::CollectionRuntimeEvent::Ready(_) => {}
            event => panic!("collection runtime did not start: {event:?}"),
        }
        let handlers = Arc::new(Handlers {
            readers: crate::remote_ipc::SharedReaders::open(&settings).unwrap(),
            producer: Some(
                crate::collection_store::CollectionRemoteProducerControl::new(runtime.client()),
            ),
            session: None,
            open: super::open::OpenController::new(None),
            recent_apps: Arc::new(Mutex::new(RecentApps::default())),
            changes: super::changes::ChangeController::new(),
            active: AtomicUsize::new(0),
            lane_log: Mutex::new(LaneLog::default()),
            search_hooks: None,
            read_hooks: None,
            write_hooks: None,
        });
        handlers.readers.install_collection_store(runtime.client());
        Self {
            _dir: dir,
            runtime: Some(runtime),
            handlers,
        }
    }
    pub(super) fn set_level(&self, agent_level: AgentLevel) {
        crate::settings_db::with_db_result(|db| {
            let mut s = db.load_into_settings()?;
            s.agent_level = agent_level;
            db.save_full(&s)
        })
        .unwrap();
    }
    pub(super) fn call(&self, connection: &Arc<Connection>, id: u64, command: Command) -> Response {
        let (tx, rx) = crossbeam_channel::unbounded();
        let worker = connection.request(Request { id, command }, tx);
        let response = rx.recv_timeout(Duration::from_secs(10)).unwrap().wire();
        if let Some(worker) = worker {
            worker.join().unwrap();
        }
        response
    }
    pub(super) fn connection(&self) -> Arc<Connection> {
        let c = Connection::new(self.handlers.clone());
        assert!(
            self.call(
                &c,
                0,
                Command::Hello {
                    version: AGENT_PROTOCOL_VERSION,
                    client_name: "test".into(),
                    client_version: "1".into(),
                    request_kind: RequestKind::Tool,
                }
            )
            .result
            .is_ok()
        );
        c
    }

    pub(super) fn seed_index(&self) {
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
                        ai_caption: "captionsecret".into(),
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
fn collection_starting_maps_to_not_ready() {
    let starting = collection_error(crate::collection_store::CollectionStoreError::Starting);
    assert_eq!(starting.code, ErrorCode::NotReady);
    assert_eq!(starting.message, "コレクションを準備しています…");
    let failure = collection_error(crate::collection_store::CollectionStoreError::Persistence(
        "fixture failure".into(),
    ));
    assert_eq!(failure.code, ErrorCode::Internal);
    assert_eq!(failure.message, "読み取りに失敗しました");
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
                client_name: "test".into(),
                client_version: "1".into(),
                request_kind: RequestKind::Tool,
            }
        )
        .result
        .unwrap_err()
        .code,
        ErrorCode::VersionMismatch
    );
    let c = f.connection();
    let Payload::Status(status) = f.call(&c, 3, Command::Status).result.unwrap() else {
        panic!("expected status")
    };
    assert_eq!(
        status.permission,
        PermissionSnapshot {
            level: AgentLevel::ReadOnly,
            thumbnails: false,
            gps: false,
            ai_captions: false,
        }
    );
    f.set_level(AgentLevel::Disabled);
    for command in [
        Command::Status,
        Command::ListTags {
            choice: TagChoice::All,
            limit: None,
            cursor: None,
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
fn cursor_continuation_and_character_budget() {
    let f = Fixture::new();
    let items: Vec<_> = (0..8)
        .map(|i| Item {
            path: format!("C:\\data\\{i}.png"),
            subresource: None,
            name: format!("{i}{}", "猫".repeat(5000)),
            kind: ItemKind::Image,
            tags: Some(vec![]),
            container_tags: None,
            rating: 0,
        })
        .collect();
    let args = serde_json::json!(["猫", null, "path"]);
    let one = page(items.clone(), 0, 5, false, "items_by_tag", args.clone(), 0).unwrap();
    assert!(one.truncated);
    assert!(mcp_content_chars(&one).unwrap() <= MAX_PAGE_CHARS);
    let offset = decode_cursor(one.cursor.as_deref(), "items_by_tag", &args, 0).unwrap();
    let two = page(items, offset, 5, false, "items_by_tag", args.clone(), 0).unwrap();
    assert_ne!(row(&one.items[0]).path, row(&two.items[0]).path);
    assert_eq!(
        decode_cursor(one.cursor.as_deref(), "search_items", &args, 0)
            .unwrap_err()
            .message,
        "最初からやり直してください"
    );
    let bad = encode_cursor("items_by_tag", args.clone(), 1, 0);
    assert!(decode_cursor(Some(&bad), "items_by_tag", &serde_json::json!([]), 0).is_err());
    drop(f);
}

#[test]
fn queued_results_are_revoked_even_after_reenable_and_unrelated_saves_are_allowed() {
    let f = Fixture::new();
    let grant = permission().unwrap();
    let reply = || Reply {
        response: Response {
            id: 1,
            result: Ok(Payload::Items(ItemPage {
                items: vec![],
                cursor: None,
                truncated: false,
            })),
        },
        policy: ReplyPolicy::Read(grant),
    };
    crate::settings_db::with_db_result(|db| {
        let mut s = db.load_into_settings()?;
        s.notification_duration = crate::settings::Settings::default().notification_duration;
        db.save_full(&s)
    })
    .unwrap();
    assert!(reply().wire().result.is_ok());
    f.set_level(AgentLevel::Disabled);
    assert_eq!(reply().wire().result.unwrap_err().code, ErrorCode::Disabled);
    f.set_level(AgentLevel::ReadOnly);
    assert_eq!(
        reply().wire().result.unwrap_err().code,
        ErrorCode::PermissionChanged
    );
    assert_ne!(grant.1, permission().unwrap().1);
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
            .all(|a| second.items.iter().all(|b| row(a).path != row(b).path))
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

#[test]
fn search_items_excludes_ai_captions_from_query_and_postfilter() {
    let f = Fixture::new();
    f.seed_index();
    let connection = f.connection();
    for (id, query, expected) in [
        (1, "captionsecret", 0),
        (2, "cat captionsecret", 0),
        (3, "cat -captionsecret", 4),
    ] {
        let Payload::Items(page) = f
            .call(
                &connection,
                id,
                Command::SearchItems {
                    query: query.into(),
                    kind: Some(ItemKind::Image),
                    limit: None,
                    cursor: None,
                },
            )
            .result
            .unwrap()
        else {
            panic!()
        };
        assert_eq!(page.items.len(), expected, "{query}");
    }
}

#[test]
fn search_items_allowed_ai_captions_match_images_and_books_in_query_and_postfilter() {
    let f = Fixture::new();
    seed_book_search_index(&f);
    crate::settings_db::with_db_result(|db| {
        let mut settings = db.load_into_settings()?;
        settings.agent_show_ai_captions = true;
        db.save_full(&settings)
    })
    .unwrap();
    let connection = f.connection();
    for (kind, query, expected) in [
        (Some(ItemKind::Image), "captionsecret", 4),
        (Some(ItemKind::Archive), "captionsecret", 5),
        (Some(ItemKind::Folder), "captionsecret", 3),
        (None, "cat captionsecret", 4),
        (None, "book captionsecret", 8),
        (None, "cat -captionsecret", 0),
        (None, "book -captionsecret", 0),
    ] {
        let Payload::Items(page) = f
            .call(
                &connection,
                1,
                Command::SearchItems {
                    query: query.into(),
                    kind,
                    limit: None,
                    cursor: None,
                },
            )
            .result
            .unwrap()
        else {
            panic!("expected search page")
        };
        assert_eq!(page.items.len(), expected, "{query} / {kind:?}");
    }
}

#[test]
fn continued_search_and_folder_reject_every_permission_change_from_admission_grant() {
    for field in ["captions", "gps", "thumbnails", "level"] {
        let f = Fixture::new();
        f.seed_index();
        let root = f._dir.path().join("folder");
        std::fs::create_dir(&root).unwrap();
        for name in ["a.png", "b.png"] {
            std::fs::write(root.join(name), b"fixture").unwrap();
        }
        let connection = f.connection();
        let Payload::Items(search) = f
            .call(
                &connection,
                1,
                Command::SearchItems {
                    query: "cat".into(),
                    kind: None,
                    limit: Some(1),
                    cursor: None,
                },
            )
            .result
            .unwrap()
        else {
            panic!()
        };
        let folder_command = |cursor| Command::ListFolder {
            path: root.to_string_lossy().into_owned(),
            subresource: None,
            limit: Some(1),
            cursor,
        };
        let Payload::Folder(folder) = f.call(&connection, 2, folder_command(None)).result.unwrap()
        else {
            panic!()
        };
        assert!(search.cursor.is_some() && folder.cursor.is_some());
        crate::settings_db::with_db_result(|db| {
            let mut settings = db.load_into_settings()?;
            match field {
                "captions" => settings.agent_show_ai_captions = true,
                "gps" => settings.agent_show_gps = true,
                "thumbnails" => settings.agent_show_thumbnails = true,
                "level" => settings.agent_level = AgentLevel::ReadAndOpen,
                _ => unreachable!(),
            }
            db.save_full(&settings)
        })
        .unwrap();
        for command in [
            Command::SearchItems {
                query: "cat".into(),
                kind: None,
                limit: Some(1),
                cursor: search.cursor,
            },
            folder_command(folder.cursor),
        ] {
            assert_eq!(
                f.call(&connection, 3, command).result.unwrap_err().code,
                ErrorCode::PermissionChanged,
                "{field}"
            );
        }
    }
}

#[test]
fn version_two_cursor_is_bad_request_even_when_permission_epoch_matches() {
    let args = serde_json::json!(["cat", null, "relevance"]);
    let legacy = serde_json::json!({
        "version": 2, "tool": "search_items", "args_digest": wire_digest(&args).unwrap(), "returned": 1,
    });
    let cursor = serde_json::to_vec(&legacy)
        .unwrap()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<String>();
    assert_eq!(
        decode_cursor(Some(&cursor), "search_items", &args, 0)
            .unwrap_err()
            .code,
        ErrorCode::BadRequest
    );
}

fn seed_book_search_index(f: &Fixture) {
    use crate::fts_index::{Container, IndexDoc, IndexKind, upsert_doc};
    f.seed_index();
    let favorite_id = f.handlers.readers.favorites().unwrap()[0].id;
    let fts = f.handlers.readers.fts.read().unwrap().clone().unwrap();
    let mut writer = fts.writer().unwrap();
    for (name, kind) in [
        ("book.zip", IndexKind::Zip),
        ("book.cbz", IndexKind::Zip),
        ("book.rar", IndexKind::Zip),
        ("book.7z", IndexKind::Zip),
        ("book.lzh", IndexKind::Zip),
        ("book-folder", IndexKind::Folder),
        ("book-folder.zip", IndexKind::Folder),
        ("book-folder.epub", IndexKind::Folder),
    ] {
        // No real directory exists at these paths: the stored kind owns result
        // classification, including folders whose names look like file types.
        let doc = IndexDoc {
            path: format!("C:/fixture/{name}"),
            container: Container::Fs,
            zip_entry: String::new(),
            favorite_id,
            kind,
            mtime: 0,
            file_size: 0,
            norms: crate::ingest_text::PerSourceText {
                name: name.into(),
                ai_caption: "captionsecret".into(),
                ..Default::default()
            },
        };
        upsert_doc(&writer, fts.fields(), &doc).unwrap();
    }
    writer.commit().unwrap();
    fts.reload_reader().unwrap();
}

#[test]
fn search_items_classifies_and_filters_captioned_books_by_stored_kind() {
    let f = Fixture::new();
    seed_book_search_index(&f);
    let connection = f.connection();
    for (id, kind, query, expected) in [
        (1, Some(ItemKind::Archive), "book", 5),
        (2, Some(ItemKind::Folder), "book", 3),
        (3, Some(ItemKind::Image), "book", 0),
        (4, None, "book", 8),
        (5, None, "captionsecret", 0),
        (6, None, "book -captionsecret", 8),
    ] {
        let Payload::Items(page) = f
            .call(
                &connection,
                id,
                Command::SearchItems {
                    query: query.into(),
                    kind,
                    limit: None,
                    cursor: None,
                },
            )
            .result
            .unwrap()
        else {
            panic!("expected item page")
        };
        assert_eq!(page.items.len(), expected, "{query} / {kind:?}");
        for item in &page.items {
            let item = row(item);
            let expected_kind = if item.name.starts_with("book-folder") {
                ItemKind::Folder
            } else {
                ItemKind::Archive
            };
            assert_eq!(item.kind, expected_kind, "{}", item.name);
            assert!(item.subresource.is_none());
            if let Some(kind) = kind {
                assert_eq!(item.kind, kind);
            }
        }
    }
}

#[test]
fn search_items_captures_ignore_archives_setting_and_keeps_zip_and_folders() {
    let f = Fixture::new();
    seed_book_search_index(&f);
    crate::settings_db::with_db_result(|db| {
        let mut settings = db.load_into_settings()?;
        settings.set_archive_file_handling(crate::settings::ArchiveFileHandling::Ignore);
        settings.epub_file_handling = crate::settings::EpubFileHandling::Ignore;
        db.save_full(&settings)
    })
    .unwrap();
    let connection = f.connection();
    for (id, kind, expected) in [
        (1, Some(ItemKind::Archive), 2),
        (2, Some(ItemKind::Folder), 3),
        (3, None, 5),
    ] {
        let Payload::Items(page) = f
            .call(
                &connection,
                id,
                Command::SearchItems {
                    query: "book".into(),
                    kind,
                    limit: None,
                    cursor: None,
                },
            )
            .result
            .unwrap()
        else {
            panic!("expected item page")
        };
        assert_eq!(page.items.len(), expected, "{kind:?}");
        assert!(page.items.iter().all(|item| {
            let item = row(item);
            item.kind == ItemKind::Folder
                || item.path.ends_with(".zip")
                || item.path.ends_with(".cbz")
        }));
    }
}

/// Cargo exposes CARGO_BIN_EXE only to the package that owns a binary. This
/// core-library test instead resolves the adapter from Cargo's executable
/// artifact, so custom target directories/profiles cannot select a stale guess.
#[cfg(windows)]
fn mcp_fixture_executable() -> std::path::PathBuf {
    if let Some(exe) = option_env!("CARGO_BIN_EXE_mimageviewer-mcp") {
        return exe.into();
    }
    let build = std::process::Command::new(env!("CARGO"))
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .args([
            "build",
            "-p",
            "mimageviewer-mcp",
            "--bin",
            "mimageviewer-mcp",
            "--message-format=json",
        ])
        .output()
        .expect("build the fixture-only MCP adapter");
    assert!(
        build.status.success(),
        "MCP fixture build failed: {}",
        String::from_utf8_lossy(&build.stderr)
    );
    String::from_utf8(build.stdout)
        .unwrap()
        .lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .find(|message| {
            message["reason"] == "compiler-artifact"
                && message["target"]["name"] == "mimageviewer-mcp"
                && message["executable"].is_string()
        })
        .and_then(|message| message["executable"].as_str().map(std::path::PathBuf::from))
        .expect("Cargo must report the MCP adapter's executable artifact")
}

/// Launches only the authorized console adapter against disposable core fixtures.
#[cfg(windows)]
#[test]
fn stdio_exe_uses_real_core_fixture_handlers() {
    use base64::Engine;
    use serde_json::json;
    use std::io::{BufRead, Write};
    let f = Fixture::new();
    f.seed_index();
    let folder = crate::data_dir::get().join("stdio-images");
    std::fs::create_dir(&folder).unwrap();
    let path = folder.join("cat.png");
    image::DynamicImage::new_rgb8(7, 5).save(&path).unwrap();
    crate::settings_db::with_db_result(|db| {
        let mut settings = db.load_into_settings()?;
        settings.agent_show_thumbnails = true;
        db.save_full(&settings)
    })
    .unwrap();
    let mut tags = crate::tags_db::TagsDb::open().unwrap();
    tags.set_item_tags(
        &crate::tags_db::item_key_for_path(&path),
        ["cat", "dog", "fox"],
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
    f.runtime
        .as_ref()
        .unwrap()
        .client()
        .create_collection("second collection".into())
        .unwrap()
        .recv()
        .unwrap()
        .unwrap();
    // The real core handler, Connection admission, wire codec and pipe transport are used.
    let server = super::pipe::fixture_server(f.handlers.clone()).unwrap();
    let exe = mcp_fixture_executable();
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
        (2, "list_tags", json!({"choice":"all","limit":1})),
        (3, "items_by_tag", json!({"tag":"cat"})),
        (4, "list_collections", json!({"limit":1})),
        (5, "search_items", json!({"query":"cat","limit":2})),
        (6, "list_folder", json!({"path":folder})),
        (7, "get_thumbnail", json!({"items":[{"path":path}]})),
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
            "list_folder" => {
                assert_eq!(structured["items"].as_array().unwrap().len(), 1);
                assert_eq!(structured["items"][0]["name"], "cat.png");
                assert_eq!(structured["items"][0]["kind"], "image");
                assert_eq!(structured["book_composition_applied"], false);
            }
            "get_thumbnail" => {
                assert_eq!(structured["items"][0]["status"], "ok");
                assert!(structured.get("images").is_none());
                let block = &result["result"]["content"][1];
                assert_eq!(block["type"], "image");
                assert_eq!(block["mimeType"], "image/webp");
                let webp = base64::engine::general_purpose::STANDARD
                    .decode(block["data"].as_str().unwrap())
                    .unwrap();
                let image =
                    image::load_from_memory_with_format(&webp, image::ImageFormat::WebP).unwrap();
                assert_eq!(
                    u64::from(image.width()),
                    structured["items"][0]["dimensions"]["width"]
                        .as_u64()
                        .unwrap()
                );
                assert_eq!(
                    u64::from(image.height()),
                    structured["items"][0]["dimensions"]["height"]
                        .as_u64()
                        .unwrap()
                );
            }
            _ => unreachable!(),
        }
        if matches!(name, "list_tags" | "list_collections") {
            let cursor = structured["cursor"]
                .as_str()
                .expect("fixture must have another page");
            let arguments = if name == "list_tags" {
                json!({"choice":"all","limit":1,"cursor":cursor})
            } else {
                json!({"limit":1,"cursor":cursor})
            };
            writeln!(input, "{}", json!({"jsonrpc":"2.0","id":100+id,"method":"tools/call","params":{"name":name,"arguments":arguments,"_meta":meta}})).unwrap();
            input.flush().unwrap();
            let line = rx.recv_timeout(Duration::from_secs(10)).unwrap();
            let next: serde_json::Value = serde_json::from_str(&line).unwrap();
            assert_eq!(next["id"], 100 + id);
            assert_eq!(next["result"]["isError"], false, "{next}");
            let rows = if name == "list_tags" {
                "tags"
            } else {
                "collections"
            };
            assert_eq!(
                next["result"]["structuredContent"][rows]
                    .as_array()
                    .unwrap()
                    .len(),
                1
            );
            assert_ne!(
                next["result"]["structuredContent"][rows][0]["name"],
                structured[rows][0]["name"]
            );
            let text: serde_json::Value =
                serde_json::from_str(next["result"]["content"][0]["text"].as_str().unwrap())
                    .unwrap();
            assert_eq!(next["result"]["structuredContent"], text);
            assert!(line.chars().count() <= MAX_MCP_RESPONSE_CHARS);
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
    assert_eq!(row(&page.items[0]).rating, 4);
    assert_eq!(row(&page.items[0]).tags, Some(vec!["#cat".into()]));
    let Payload::Tags(page) = f
        .call(
            &c,
            2,
            Command::ListTags {
                choice: TagChoice::All,
                limit: None,
                cursor: None,
            },
        )
        .result
        .unwrap()
    else {
        panic!()
    };
    assert_eq!(row(&page.tags[0]).name, "cat");
    assert_eq!(row(&page.tags[0]).count, 1);
    let Payload::Collections(page) = f
        .call(
            &c,
            3,
            Command::ListCollections {
                limit: None,
                cursor: None,
            },
        )
        .result
        .unwrap()
    else {
        panic!()
    };
    assert_eq!(row(&page.collections[0]).name, "fixture collection");
    assert_eq!(row(&page.collections[0]).count, 1);
}

#[test]
fn tag_and_collection_pages_continue_past_200_and_validate_cursors() {
    let f = Fixture::new();
    let path = crate::data_dir::get().join("catalog.png");
    std::fs::write(&path, []).unwrap();
    let mut db = crate::tags_db::TagsDb::open().unwrap();
    db.set_item_tags(
        &crate::tags_db::item_key_for_path(&path),
        (0..205).map(|i| format!("tag-{i:03}")),
        crate::tags_db::source::EDIT,
    )
    .unwrap();
    let actor = f.runtime.as_ref().unwrap().client();
    for i in 0..205 {
        actor
            .create_collection(format!("collection-{i:03}"))
            .unwrap()
            .recv()
            .unwrap()
            .unwrap();
    }
    let c = f.connection();
    let Payload::Tags(first) = f
        .call(
            &c,
            1,
            Command::ListTags {
                choice: TagChoice::All,
                limit: None,
                cursor: None,
            },
        )
        .result
        .unwrap()
    else {
        panic!()
    };
    assert_eq!(first.tags.len(), DEFAULT_LIMIT as usize);
    assert!(first.truncated);
    let tag_cursor = first.cursor.clone().unwrap();
    let Payload::Tags(second) = f
        .call(
            &c,
            2,
            Command::ListTags {
                choice: TagChoice::All,
                limit: Some(MAX_LIMIT),
                cursor: first.cursor,
            },
        )
        .result
        .unwrap()
    else {
        panic!()
    };
    let names = first
        .tags
        .iter()
        .chain(&second.tags)
        .map(|t| row(t).name.clone())
        .collect::<Vec<_>>();
    assert_eq!(
        names,
        (0..205).map(|i| format!("tag-{i:03}")).collect::<Vec<_>>()
    );
    assert!(!second.truncated);
    assert!(second.cursor.is_none());
    assert_eq!(
        f.call(
            &c,
            3,
            Command::ListTags {
                choice: TagChoice::Recent,
                limit: None,
                cursor: Some(tag_cursor.clone())
            }
        )
        .result
        .unwrap_err()
        .code,
        ErrorCode::BadRequest
    );
    assert_eq!(
        f.call(
            &c,
            4,
            Command::ListCollections {
                limit: None,
                cursor: Some(tag_cursor)
            }
        )
        .result
        .unwrap_err()
        .message,
        "最初からやり直してください"
    );

    let Payload::Collections(first) = f
        .call(
            &c,
            5,
            Command::ListCollections {
                limit: None,
                cursor: None,
            },
        )
        .result
        .unwrap()
    else {
        panic!()
    };
    assert_eq!(first.collections.len(), DEFAULT_LIMIT as usize);
    assert!(first.truncated);
    let Payload::Collections(second) = f
        .call(
            &c,
            6,
            Command::ListCollections {
                limit: Some(MAX_LIMIT),
                cursor: first.cursor,
            },
        )
        .result
        .unwrap()
    else {
        panic!()
    };
    let names = first
        .collections
        .iter()
        .chain(&second.collections)
        .map(|t| row(t).name.clone())
        .collect::<Vec<_>>();
    assert_eq!(
        names,
        (0..205)
            .map(|i| format!("collection-{i:03}"))
            .collect::<Vec<_>>()
    );
    assert!(second.collections.iter().all(|value| row(value).count == 0));
    assert!(!second.truncated);
    assert!(second.cursor.is_none());
    for limit in [0] {
        for command in [
            Command::ListTags {
                choice: TagChoice::All,
                limit: Some(limit),
                cursor: None,
            },
            Command::ListCollections {
                limit: Some(limit),
                cursor: None,
            },
        ] {
            assert_eq!(
                f.call(&c, 7, command).result.unwrap_err().code,
                ErrorCode::BadRequest
            );
        }
    }
    for command in [
        Command::ListTags {
            choice: TagChoice::All,
            limit: Some(MAX_LIMIT + 1),
            cursor: None,
        },
        Command::ListCollections {
            limit: Some(MAX_LIMIT + 1),
            cursor: None,
        },
    ] {
        let response = f.call(&c, 8, command);
        match response.result.unwrap() {
            Payload::Tags(page) => assert_eq!(page.tags.len(), MAX_LIMIT as usize),
            Payload::Collections(page) => assert_eq!(page.collections.len(), MAX_LIMIT as usize),
            _ => panic!("expected a clamped first page"),
        }
    }
}

#[test]
fn named_pages_use_actual_serialized_character_cap_and_returned_offset() {
    let args = serde_json::json!([TagChoice::All, "tag_menu_order"]);
    let tags = (0..20)
        .map(|i| Tag {
            name: format!("{i}{}", "猫\\\"".repeat(300)),
            count: 1,
        })
        .collect();
    let (tags, cursor, truncated) =
        bounded_rows(tags, 50, false, "list_tags", args.clone(), "tags", 0).unwrap();
    assert!(truncated);
    assert!(!tags.is_empty());
    assert!(tags.len() < 20);
    let page = TagPage {
        tags,
        cursor,
        truncated,
    };
    assert!(mcp_content_chars(&page).unwrap() <= MAX_PAGE_CHARS);
    assert_eq!(
        decode_cursor(page.cursor.as_deref(), "list_tags", &args, 0).unwrap(),
        50 + page.tags.len()
    );
    let args = serde_json::json!(["catalog_order"]);
    let rows = (0..20)
        .map(|i| Collection {
            id: format!("collection-{i}"),
            name: format!("{i}{}", "猫\\\"".repeat(300)),
            count: 5,
        })
        .collect();
    let (collections, cursor, truncated) = bounded_rows(
        rows,
        200,
        false,
        "list_collections",
        args.clone(),
        "collections",
        0,
    )
    .unwrap();
    let page = CollectionPage {
        collections,
        cursor,
        truncated,
    };
    assert!(page.truncated);
    assert!(!page.collections.is_empty());
    assert!(mcp_content_chars(&page).unwrap() <= MAX_PAGE_CHARS);
    assert_eq!(
        decode_cursor(page.cursor.as_deref(), "list_collections", &args, 0).unwrap(),
        200 + page.collections.len()
    );
    let (tags, cursor, _) = bounded_rows(
        vec![Tag {
            name: "猫".repeat(MAX_PAGE_CHARS),
            count: 1,
        }],
        0,
        false,
        "list_tags",
        serde_json::json!([]),
        "tags",
        0,
    )
    .unwrap();
    assert_eq!(tags.len(), 1);
    assert!(matches!(&tags[0], PageRow::TooLong(_)));
    assert!(
        cursor.is_none(),
        "the omitted row consumed the final input position"
    );
}
