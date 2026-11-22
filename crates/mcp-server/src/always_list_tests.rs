use super::*;
use std::sync::atomic::{AtomicBool, AtomicUsize};

fn connect(backend: Arc<dyn Backend>, version: &str) -> Harness {
    let harness = Harness::new(backend);
    if version != MODERN {
        harness.send(json!({"jsonrpc":"2.0","id":0,"method":"initialize","params":{
            "protocolVersion":version,"capabilities":{},"clientInfo":{"name":"fixture","version":"1"}
        }}));
        let initialized = harness.receive();
        assert!(
            initialized["result"]["capabilities"]["tools"]
                .get("listChanged")
                .is_none()
        );
        harness.send(json!({"jsonrpc":"2.0","method":"notifications/initialized"}));
    }
    harness
}

fn request(version: &str, id: i64, method: &str, params: Value) -> Value {
    if version == MODERN {
        modern(id, method, params)
    } else {
        json!({"jsonrpc":"2.0","id":id,"method":method,"params":params})
    }
}

#[test]
fn every_permission_combination_has_identical_full_discovery_without_core_queries() {
    struct BackendState(Mutex<PermissionSnapshot>, AtomicUsize);
    impl Backend for BackendState {
        fn call(
            &self,
            _: Command,
            _: &ClientIdentity,
            _: RequestKind,
            _: Arc<CallControl>,
        ) -> BackendResult {
            self.1.fetch_add(1, Ordering::Relaxed);
            panic!(
                "discovery must not query core: {:?}",
                self.0.lock().unwrap()
            );
        }
    }
    let expected = json!(schema::tools());
    assert_eq!(expected.as_array().unwrap().len(), 25);
    for version in VERSIONS {
        let backend = Arc::new(BackendState(
            Mutex::new(PermissionSnapshot::default()),
            AtomicUsize::new(0),
        ));
        let mut h = connect(backend.clone(), version);
        let mut id = 0;
        for level in [
            AgentLevel::Disabled,
            AgentLevel::ReadOnly,
            AgentLevel::ReadAndOpen,
            AgentLevel::Organize,
        ] {
            for bits in 0..64 {
                *backend.0.lock().unwrap() = PermissionSnapshot {
                    current_view: bits & 16 != 0,
                    settings: bits & 32 != 0,

                    level,
                    thumbnails: bits & 1 != 0,
                    gps: bits & 2 != 0,
                    ai_captions: bits & 4 != 0,
                    transcripts: bits & 8 != 0,
                };
                id += 1;
                h.send(request(version, id, "tools/list", json!({})));
                let reply = h.receive();
                assert_eq!(reply["id"], id);
                assert_eq!(reply["result"]["tools"], expected);
            }
        }
        assert_eq!(backend.1.load(Ordering::Relaxed), 0);
        h.close();
        assert!(
            h.output.try_recv().is_err(),
            "no list_changed notifications"
        );
    }
}

#[test]
fn core_availability_and_errors_never_change_discovery_or_generate_notices() {
    struct Availability(AtomicUsize);
    impl Backend for Availability {
        fn call(
            &self,
            _: Command,
            _: &ClientIdentity,
            _: RequestKind,
            _: Arc<CallControl>,
        ) -> BackendResult {
            match self.0.load(Ordering::Acquire) {
                0 => Err(BackendError::Unreachable),
                1 => Err(AgentError::new(ErrorCode::Disabled, "mIV の AI 連携が無効です").into()),
                2 => {
                    Err(AgentError::new(ErrorCode::PermissionChanged, "許可が変わりました").into())
                }
                _ => Ok(Payload::Transcript(TranscriptReadResult::None)),
            }
        }
    }
    for version in VERSIONS {
        let backend = Arc::new(Availability(AtomicUsize::new(0)));
        let mut h = connect(backend.clone(), version);
        for state in 0..4 {
            backend.0.store(state, Ordering::Release);
            h.send(request(version, 1, "tools/list", json!({})));
            assert_eq!(h.receive()["result"]["tools"], json!(schema::tools()));
            h.send(request(
                version,
                2,
                "tools/call",
                json!({"name":"get_transcript","arguments":{"item":{"path":"C:/sample.mp4"}}}),
            ));
            let reply = h.receive();
            assert_eq!(reply["id"], 2);
            assert_eq!(reply["result"]["isError"], state != 3);
            if state != 3 {
                assert_eq!(
                    reply["result"]["structuredContent"]["error"]["code"],
                    ["not_ready", "disabled", "permission_changed"][state]
                );
            }
        }
        h.close();
    }
}

#[test]
fn permission_errors_and_enable_apply_on_same_connection_without_relisting() {
    struct Gate(AtomicBool, Mutex<Vec<Command>>);
    impl Backend for Gate {
        fn call(
            &self,
            command: Command,
            _: &ClientIdentity,
            kind: RequestKind,
            _: Arc<CallControl>,
        ) -> BackendResult {
            assert_eq!(kind, RequestKind::Tool);
            self.1.lock().unwrap().push(command.clone());
            if !self.0.load(Ordering::Acquire) {
                return Err(AgentError::new(
                    ErrorCode::Disabled,
                    "「文字起こしを AI に見せる」を ON にしてください",
                )
                .into());
            }
            assert!(matches!(command, Command::GetTranscript { .. }));
            Ok(Payload::Transcript(TranscriptReadResult::None))
        }
    }
    for version in VERSIONS {
        let backend = Arc::new(Gate(AtomicBool::new(false), Mutex::new(vec![])));
        let mut h = connect(backend.clone(), version);
        h.send(request(version, 1, "tools/list", json!({})));
        assert_eq!(h.receive()["result"]["tools"], json!(schema::tools()));
        for (id, permitted) in [(2, false), (3, true), (4, false)] {
            backend.0.store(permitted, Ordering::Release);
            h.send(request(
                version,
                id,
                "tools/call",
                json!({"name":"get_transcript","arguments":{"item":{"path":"C:/sample.mp4"}}}),
            ));
            let reply = h.receive();
            assert_eq!(reply["id"], id);
            assert_eq!(reply["result"]["isError"], !permitted);
            if !permitted {
                assert!(
                    reply["result"]["content"][0]["text"]
                        .as_str()
                        .unwrap()
                        .contains("文字起こしを AI に見せる")
                );
            }
        }
        assert_eq!(backend.1.lock().unwrap().len(), 3);
        h.close();
    }
}

#[test]
fn each_tool_description_names_its_miv_permission() {
    for tool in schema::tools() {
        let name = tool["name"].as_str().unwrap();
        let description = tool["description"].as_str().unwrap();
        let prefix = match name {
            "search_help" | "get_help" => "help offline.",
            "get_view_state" => "Needs mIV Read + current-view sharing.",
            "get_settings" | "set_settings" => "Needs mIV Read + settings access.",
            "get_transcript" => "Needs mIV Read + transcript sharing.",
            "get_thumbnail" => "Needs mIV Read + thumbnail sharing.",
            "open_in_viewer" => "Needs mIV Read and Open.",
            "create_collection"
            | "rename_collection"
            | "add_to_collection"
            | "remove_from_collection"
            | "add_tags"
            | "remove_tags"
            | "set_rating" => "Needs mIV Organize.",
            _ => "Needs mIV Read.",
        };
        assert!(description.contains(prefix), "{name}");
    }
}
