use super::tests::Fixture;
use super::*;

#[path = "../../crates/agent-ipc/tests/v3_wire/mod.rs"]
mod v3_wire;
#[path = "../../crates/agent-ipc/tests/v5_wire/mod.rs"]
mod v5_wire;

const V3_HELLO: &[u8] = include_bytes!("../../crates/agent-ipc/tests/v3_wire/hello.frame");
const V5_HELLO: &[u8] = include_bytes!("../../crates/agent-ipc/tests/v5_wire/hello.frame");

fn frozen_v5_call(
    fixture: &Fixture,
    connection: &Arc<Connection>,
    id: u64,
    command: v5_wire::Command,
    tool: &str,
) -> serde_json::Value {
    let mut bytes = Vec::new();
    v5_wire::write_frame(&mut bytes, &v5_wire::Request { id, command }).unwrap();
    let current: Request = read_frame(&mut bytes.as_slice()).unwrap().unwrap();
    let response = fixture.call(connection, current.id, current.command);
    assert!(response.result.is_ok(), "{tool}: {:?}", response.result);
    let mut encoded = Vec::new();
    write_frame(&mut encoded, &response).unwrap();
    let old: v5_wire::Response = v5_wire::read_frame(&mut encoded.as_slice())
        .unwrap()
        .unwrap();
    let result = v5_wire::output::tool_result_with_side(tool, old.result, 1568);
    assert_eq!(result["isError"], false, "{tool}: {result}");
    let tools = v5_wire::schema::tools();
    assert!(v5_wire::schema_check::accepts_schema(
        &tools.iter().find(|t| t["name"] == tool).unwrap()["outputSchema"],
        &result["structuredContent"]
    ));
    assert!(
        v5_wire::mcp_content_chars(&result["structuredContent"]).unwrap()
            <= v5_wire::MAX_PAGE_CHARS
    );
    result
}

#[test]
fn frozen_v5_adapter_reaches_v6_handlers_and_drops_only_additive_fields() {
    let fixture = Fixture::new();
    let folder = fixture._dir.path().join("v5-folder");
    std::fs::create_dir_all(folder.join("child")).unwrap();
    let image = folder.join("large.png");
    image::DynamicImage::new_rgb8(1700, 850)
        .save(&image)
        .unwrap();
    crate::settings_db::with_db_result(|db| {
        let mut settings = db.load_into_settings()?;
        settings.agent_show_current_view = true;
        settings.agent_allow_settings = true;
        settings.agent_show_thumbnails = true;
        let mut favorite =
            crate::settings::FavoriteEntry::new("v5 favorite".into(), folder.clone());
        favorite.auto_index_metadata = true;
        favorite.auto_index_structure = true;
        settings.favorites = vec![favorite];
        db.save_full(&settings)
    })
    .unwrap();
    let connection = Connection::new(fixture.handlers.clone());
    assert_eq!(
        handshake(&connection, V5_HELLO).result,
        Ok(Payload::Hello { version: 5 })
    );
    let status = frozen_v5_call(
        &fixture,
        &connection,
        2,
        v5_wire::Command::Status,
        "miv_status",
    );
    assert!(
        status["structuredContent"]["permission"]
            .get("current_view")
            .is_none()
    );
    assert!(
        status["structuredContent"]["permission"]
            .get("settings")
            .is_none()
    );
    assert_eq!(
        status["structuredContent"]["search_scope"][0],
        folder.to_string_lossy().as_ref()
    );
    let library = frozen_v5_call(
        &fixture,
        &connection,
        3,
        v5_wire::Command::ListLibrary {
            kind: v5_wire::LibraryKind::Favorites,
            stars: None,
            max_stars: None,
            limit: None,
            cursor: None,
        },
        "list_library",
    );
    assert!(
        library["structuredContent"]["items"][0]
            .get("item_index")
            .is_none()
    );
    assert!(
        library["structuredContent"]["items"][0]
            .get("container_index")
            .is_none()
    );
    let thumbnails = frozen_v5_call(
        &fixture,
        &connection,
        4,
        v5_wire::Command::GetThumbnail {
            items: vec![v5_wire::ItemRef {
                path: image.to_string_lossy().into_owned(),
                subresource: None,
            }],
            long_side: Some(1568),
            cursor: None,
        },
        "get_thumbnail",
    );
    assert_eq!(
        thumbnails["structuredContent"]["items"][0]["dimensions"]["width"],
        1568
    );
    assert_eq!(thumbnails["content"][1]["type"], "image");
}

fn handshake(connection: &Arc<Connection>, frame: &[u8]) -> Response {
    let (tx, rx) = crossbeam_channel::unbounded();
    connection.read_handshake(&mut &frame[..], tx).unwrap();
    rx.recv_timeout(Duration::from_secs(10)).unwrap().wire()
}

fn frame(value: serde_json::Value) -> Vec<u8> {
    let mut bytes = Vec::new();
    write_frame(&mut bytes, &value).unwrap();
    bytes
}

#[test]
fn shipped_v3_hello_frame_negotiates_and_reaches_handler_with_connection_version() {
    let mut f = Fixture::new();
    let (seen, versions) = crossbeam_channel::unbounded();
    Arc::get_mut(&mut f.handlers).unwrap().read_hooks = Some(Arc::new(ReadHooks {
        now: Arc::new(Instant::now),
        execute: Box::new(move |_, ctx| {
            seen.send(ctx.adapter_protocol_version).unwrap();
            Ok(Payload::Cancelled)
        }),
    }));
    let v3 = Connection::new(f.handlers.clone());
    let hello = handshake(&v3, V3_HELLO);
    let mut wire = vec![];
    write_frame(&mut wire, &hello).unwrap();
    let expected = br#"{"id":1,"result":{"Ok":{"type":"hello","data":{"version":3}}}}"#;
    assert_eq!(&wire[..4], &(expected.len() as u32).to_le_bytes());
    assert_eq!(&wire[4..], expected);
    assert!(f.call(&v3, 2, Command::Status).result.is_ok());
    assert_eq!(versions.recv_timeout(Duration::from_secs(10)).unwrap(), 3);

    let current = f.connection();
    assert!(f.call(&current, 2, Command::Status).result.is_ok());
    assert_eq!(
        versions.recv_timeout(Duration::from_secs(10)).unwrap(),
        AGENT_PROTOCOL_VERSION
    );
    let v4 = Connection::new(f.handlers.clone());
    assert_eq!(
        handshake(
            &v4,
            &frame(serde_json::json!({
                "id": 1, "command": { "type": "hello", "version": 4,
                "client_name": "legacy-v4", "client_version": "1", "request_kind": "tool" }
            }))
        )
        .result,
        Ok(Payload::Hello { version: 4 })
    );
    assert!(f.call(&v4, 2, Command::Status).result.is_ok());
    assert_eq!(versions.recv_timeout(Duration::from_secs(10)).unwrap(), 4);
    // No re-Hello can change a connection's version or attribution.
    assert_eq!(
        f.call(
            &v3,
            3,
            Command::Hello {
                version: 4,
                client_name: "replacement".into(),
                client_version: "2".into(),
                request_kind: RequestKind::Internal,
            }
        )
        .result
        .unwrap_err()
        .code,
        ErrorCode::BadRequest
    );
    assert!(f.call(&v3, 4, Command::Status).result.is_ok());
    assert_eq!(versions.recv_timeout(Duration::from_secs(10)).unwrap(), 3);
    v4.disconnect();
    assert!(f.call(&v3, 5, Command::Status).result.is_ok());
    assert_eq!(versions.recv_timeout(Duration::from_secs(10)).unwrap(), 3);
}

#[test]
fn unsupported_hello_is_read_without_business_types_and_never_reaches_handler() {
    let mut f = Fixture::new();
    Arc::get_mut(&mut f.handlers).unwrap().read_hooks = Some(Arc::new(ReadHooks {
        now: Arc::new(Instant::now),
        execute: Box::new(|_, _| panic!("unnegotiated request reached business handler")),
    }));
    for version in [0, 2, AGENT_PROTOCOL_VERSION + 1, u32::MAX] {
        let c = Connection::new(f.handlers.clone());
        // These fields cannot deserialize as today's Request. Only the fixed
        // envelope is meaningful until the adapter version has been accepted.
        let response = handshake(
            &c,
            &frame(serde_json::json!({
                "id": 77, "command": {"type":"hello", "version":version,
                    "client_name":{}, "request_kind":"future", "future_field":[1,2]}
            })),
        );
        assert_eq!(response.id, 77);
        assert_eq!(
            response.result,
            Ok(Payload::Hello {
                version: AGENT_PROTOCOL_VERSION
            })
        );
        for command in [
            Command::Status,
            Command::Cancel { request_id: 2 },
            Command::OpenInViewer {
                path: "C:/fixture.png".into(),
            },
            Command::CreateCollection {
                name: "fixture".into(),
            },
            Command::AddTags {
                items: vec![],
                tags: vec!["fixture".into()],
            },
        ] {
            assert_eq!(
                f.call(&c, 2, command).result.unwrap_err().code,
                ErrorCode::VersionMismatch
            );
        }
        // Rejection lasts for the connection; a second valid Hello cannot repair it.
        assert!(
            f.call(
                &c,
                3,
                Command::Hello {
                    version: AGENT_PROTOCOL_VERSION,
                    client_name: "test".into(),
                    client_version: "1".into(),
                    request_kind: RequestKind::Tool,
                }
            )
            .result
            .is_err()
        );
        assert_eq!(
            f.call(&c, 4, Command::Status).result.unwrap_err().code,
            ErrorCode::VersionMismatch
        );
        assert_eq!(f.handlers.active.load(Ordering::Acquire), 0);
        assert!(f.handlers.recent_apps.lock().unwrap().entries().is_empty());
    }
}

fn frozen_call(
    fixture: &Fixture,
    connection: &Arc<Connection>,
    id: u64,
    command: v3_wire::Command,
    tool: &str,
) -> serde_json::Value {
    let mut request = Vec::new();
    v3_wire::write_frame(&mut request, &v3_wire::Request { id, command }).unwrap();
    let current: Request = read_frame(&mut request.as_slice()).unwrap().unwrap();
    let response = fixture.call(connection, current.id, current.command);
    assert!(response.result.is_ok(), "{tool}: {:?}", response.result);
    let mut encoded = Vec::new();
    write_frame(&mut encoded, &response).unwrap();
    assert!(encoded.len() <= v3_wire::MAX_FRAME_BYTES + 4);
    let old: v3_wire::Response = v3_wire::read_frame(&mut encoded.as_slice())
        .unwrap()
        .unwrap();
    let output = v3_wire::output::tool_result(tool, old.result);
    assert_eq!(output["isError"], false, "{tool}: {output}");
    let tools = v3_wire::schema::tools();
    let schema = &tools.iter().find(|entry| entry["name"] == tool).unwrap()["outputSchema"];
    assert!(v3_wire::schema_check::accepts_schema(
        schema,
        &output["structuredContent"]
    ));
    output
}

#[test]
fn frozen_v3_v4_requests_reach_real_handlers_with_old_output_limits() {
    let fixture = Fixture::new();
    let folder = fixture._dir.path().join("folder");
    std::fs::create_dir_all(folder.join("child")).unwrap();
    let image = folder.join("page.png");
    image::DynamicImage::new_rgb8(700, 350)
        .save(&image)
        .unwrap();
    image::DynamicImage::new_rgb8(2, 2)
        .save(folder.join("child/nested.png"))
        .unwrap();
    crate::settings_db::with_db_result(|db| {
        let mut settings = db.load_into_settings()?;
        settings.agent_show_thumbnails = true;
        let mut favorite = crate::settings::FavoriteEntry::new("indexed".into(), folder.clone());
        favorite.auto_index_metadata = true;
        settings.favorites = vec![favorite];
        db.save_full(&settings)
    })
    .unwrap();
    // v4 retained these shipped v3 wire request/response families unchanged.
    for version in [3, 4] {
        let connection = Connection::new(fixture.handlers.clone());
        let mut hello = Vec::new();
        v3_wire::write_frame(
            &mut hello,
            &v3_wire::Request {
                id: 1,
                command: v3_wire::Command::Hello {
                    version,
                    client_name: "frozen".into(),
                    client_version: "1".into(),
                    request_kind: v3_wire::RequestKind::Tool,
                },
            },
        )
        .unwrap();
        assert_eq!(
            handshake(&connection, &hello).result,
            Ok(Payload::Hello { version })
        );
        let status = frozen_call(
            &fixture,
            &connection,
            2,
            v3_wire::Command::Status,
            "miv_status",
        );
        assert!(status["structuredContent"].get("search_scope").is_none());
        assert!(
            status["structuredContent"]
                .get("search_scope_truncated")
                .is_none()
        );
        let listed = frozen_call(
            &fixture,
            &connection,
            3,
            v3_wire::Command::ListFolder {
                path: folder.to_string_lossy().into_owned(),
                subresource: None,
                min_stars: None,
                max_stars: None,
                kinds: None,
                limit: None,
                cursor: None,
            },
            "list_folder",
        );
        let names: Vec<_> = listed["structuredContent"]["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|row| row["name"].as_str().unwrap())
            .collect();
        assert_eq!(names.len(), 2);
        assert!(names.contains(&"child") && names.contains(&"page.png"));
        let thumbnails = frozen_call(
            &fixture,
            &connection,
            4,
            v3_wire::Command::GetThumbnail {
                items: (0..5)
                    .map(|_| v3_wire::ItemRef {
                        path: image.to_string_lossy().into_owned(),
                        subresource: None,
                    })
                    .collect(),
                long_side: Some(1568),
                cursor: None,
            },
            "get_thumbnail",
        );
        assert_eq!(thumbnails["content"].as_array().unwrap().len(), 6);
        for row in thumbnails["structuredContent"]["items"].as_array().unwrap() {
            assert_eq!(row["dimensions"]["width"], 512);
            assert_eq!(row["dimensions"]["height"], 256);
        }
        assert!(thumbnails["structuredContent"]["cursor"].is_null());
    }
}

#[test]
fn business_before_hello_and_malformed_supported_hello_cannot_enter_handlers() {
    let f = Fixture::new();
    for command in [Command::Status, Command::Cancel { request_id: 1 }] {
        let c = Connection::new(f.handlers.clone());
        assert_eq!(
            f.call(&c, 1, command.clone()).result.unwrap_err().code,
            ErrorCode::BadRequest
        );
        let (tx, _rx) = crossbeam_channel::unbounded();
        let mut bytes = vec![];
        write_frame(&mut bytes, &Request { id: 1, command }).unwrap();
        assert!(c.read_handshake(&mut bytes.as_slice(), tx).is_err());
        assert_eq!(f.handlers.active.load(Ordering::Acquire), 0);
    }
    let c = Connection::new(f.handlers.clone());
    let bytes = frame(serde_json::json!({"id":1,"command":{"type":"hello","version":3}}));
    let (tx, _rx) = crossbeam_channel::unbounded();
    assert!(c.read_handshake(&mut bytes.as_slice(), tx).is_err());
    assert_eq!(
        f.call(&c, 2, Command::Status).result.unwrap_err().code,
        ErrorCode::BadRequest
    );
    assert!(f.handlers.recent_apps.lock().unwrap().entries().is_empty());
    let ctx = ReadContext::new(
        &f.handlers,
        permission().unwrap(),
        Arc::new(AtomicBool::new(false)),
    );
    let (result, _) = f.handlers.execute(
        Command::Hello {
            version: 3,
            client_name: "test".into(),
            client_version: "1".into(),
            request_kind: RequestKind::Tool,
        },
        &ctx,
    );
    assert_eq!(result.unwrap_err().code, ErrorCode::BadRequest);
}
