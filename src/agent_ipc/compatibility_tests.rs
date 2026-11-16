use super::tests::Fixture;
use super::*;

const V3_HELLO: &[u8] = include_bytes!("../../crates/agent-ipc/tests/v3_wire/hello.frame");

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

    let v4 = f.connection();
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
    for version in [0, 2, 5, u32::MAX] {
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
        assert_eq!(response.result, Ok(Payload::Hello { version: 4 }));
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
                    version: 4,
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
