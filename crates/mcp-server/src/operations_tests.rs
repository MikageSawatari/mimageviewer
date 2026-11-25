use super::*;

fn operations_page() -> OperationsPage {
    serde_json::from_value(json!({"items":[
        {"action":"FsCopyImage","label":"copy image","context":"FsImage","context_label":"image fullscreen","trigger":"press","keys":[],"default":[],"customized":false,"inputs":["ring.image.up"],"screens":["image_fullscreen"],"screen_labels":["image fullscreen"],"discard":"unknown"},
        {"id":"a".repeat(64),"error":"too_long"}
    ],"page":1,"pages":1,"reserved":[],"note":"choose a screen","discard":"unknown"})).unwrap()
}

fn operations_write() -> OperationsWriteResult {
    serde_json::from_value(json!({"changed":[{"name":"FsCopyImage","label":"copy","before":null,"after":["Ctrl+C"],"discard":true}],"unchanged":[],"conflicts":[{"key":"Ctrl+C","kind":"overlap","action":"FsCopyImage","other":"FsToggleMetadata","discard":true}]})).unwrap()
}

#[test]
fn operations_arguments_reject_wrong_views_types_counts_and_unknown_fields() {
    for args in [
        json!({}),
        json!({"key":"Ctrl+C","screen":"image_fullscreen"}),
        json!({"view":"inputs","slots":"gesture"}),
        json!({"view":"choices","slot":"ring.image.up"}),
        json!({"view":"conflicts","actions":["FsCopyImage"],"key":"Ctrl+C"}),
    ] {
        assert!(matches!(
            parse_tool(&json!({"name":"get_operations","arguments":args}))
                .unwrap()
                .1,
            Command::GetOperations(_)
        ));
    }
    for args in [
        json!({"view":"bad"}),
        json!({"view":"inputs","actions":["FsCopyImage"]}),
        json!({"context":"Grid","screen":"grid"}),
        json!({"view":"conflicts","key":"Ctrl+C"}),
        json!({"page":0}),
        json!({"page":-1}),
        json!({"page":1.5}),
        json!({"actions":[]}),
        json!({"query":"x".repeat(201)}),
        json!({"cursor":"next"}),
    ] {
        assert!(
            parse_tool(&json!({"name":"get_operations","arguments":args})).is_err(),
            "{args}"
        );
    }
    for args in [
        json!({"keys":{"FsCopyImage":["F13","F13"]}}),
        json!({"inputs":{"gesture.image":[{"pattern":"DR","action":"none"},{"pattern":"DR","action":"copy_files"}]}}),
        json!({"keys":{"FsCopyImage":null},"inputs":{"right_drag.image":null}}),
    ] {
        let (_, command) = parse_tool(&json!({"name":"set_operations","arguments":args})).unwrap();
        let Command::SetOperations(edits) = command else {
            panic!("wrong command");
        };
        assert_eq!(
            serde_json::to_value(edits).unwrap()["keys"],
            args.get("keys").cloned().unwrap_or(Value::Null)
        );
    }
    let mut keys = serde_json::Map::new();
    for index in 0..MAX_SETTINGS_KEYS {
        keys.insert(format!("action{index}"), Value::Null);
    }
    assert!(parse_tool(&json!({"name":"set_operations","arguments":{"keys":keys}})).is_ok());
    for args in [
        json!({}),
        json!({"keys":{},"inputs":null}),
        json!({"keys":keys,"inputs":{"option.gamepad_enabled":false}}),
        json!({"keys":[]}),
        json!({"keys":{"FsCopyImage":null},"confirm":false}),
    ] {
        assert!(parse_tool(&json!({"name":"set_operations","arguments":args})).is_err());
    }
}

#[test]
fn operations_typed_projection_and_final_envelopes_fit_all_versions() {
    for (name, payload) in [
        ("get_operations", Payload::Operations(operations_page())),
        (
            "set_operations",
            Payload::OperationsWrite(operations_write()),
        ),
    ] {
        let result = tool_result(name, Ok(payload));
        assert_eq!(result["isError"], false);
        assert!(!result.to_string().contains("discard"));
        let tools = schema::tools();
        let schema = &tools.iter().find(|tool| tool["name"] == name).unwrap()["outputSchema"];
        assert!(accepts_schema(schema, &result["structuredContent"]));
        assert!(!accepts_schema(schema, &json!({})));
        assert_eq!(
            serde_json::from_str::<Value>(result["content"][0]["text"].as_str().unwrap()).unwrap(),
            result["structuredContent"]
        );
        for modern in [false, true] {
            let envelope = success(json!("\0".repeat(256)), result.clone(), modern);
            assert!(response_non_image_chars(&envelope) <= MAX_MCP_RESPONSE_CHARS);
        }
        assert_eq!(tool_result(name, Ok(Payload::Cancelled))["isError"], true);
    }
    assert_eq!(
        tool_result(
            "get_operations",
            Ok(Payload::OperationsWrite(operations_write()))
        )["isError"],
        true
    );
    assert_eq!(
        tool_result("set_operations", Ok(Payload::Operations(operations_page())))["isError"],
        true
    );
}

#[test]
fn operations_discovery_keeps_annotations_safety_guidance_and_pair_budget() {
    let tools = schema::tools();
    let pair: Vec<_> = tools
        .iter()
        .filter(|tool| {
            matches!(
                tool["name"].as_str(),
                Some("get_operations" | "set_operations")
            )
        })
        .collect();
    assert_eq!(pair.len(), 2);
    let chars: usize = pair
        .iter()
        .map(|tool| tool.to_string().chars().count())
        .sum();
    assert!(chars <= 3_400, "operations pair: {chars}");
    for tool in &pair {
        assert_eq!(
            tool["annotations"]["readOnlyHint"],
            tool["name"] == "get_operations"
        );
        assert_eq!(tool["annotations"]["destructiveHint"], false);
        assert_eq!(tool["annotations"]["idempotentHint"], true);
        assert!(tool["description"].as_str().unwrap().encode_utf16().count() <= 2_048);
    }
    let read = pair
        .iter()
        .find(|tool| tool["name"] == "get_operations")
        .unwrap()["description"]
        .as_str()
        .unwrap();
    for text in [
        "screen+key",
        "too_long",
        "unknown",
        "get_help",
        "gamepad X",
        "fullscreen_primary_click",
    ] {
        assert!(read.contains(text), "{text}");
    }
    let write = pair
        .iter()
        .find(|tool| tool["name"] == "set_operations")
        .unwrap()["description"]
        .as_str()
        .unwrap();
    for text in [
        "intended screen",
        "unrequested screens",
        "simulate conflicts",
        "never block",
        "cancel",
        "no universal winner",
        "first wins",
    ] {
        assert!(write.contains(text), "{text}");
    }
    assert!(INSTRUCTIONS.contains("get_operations"));
}

#[test]
fn operations_tools_work_on_every_mcp_version_and_permission_failure() {
    struct OperationsBackend {
        disabled: bool,
    }
    impl Backend for OperationsBackend {
        fn call(
            &self,
            command: Command,
            _: &ClientIdentity,
            _: RequestKind,
            _: Arc<CallControl>,
        ) -> BackendResult {
            if self.disabled {
                return Err(AgentError::new(
                    ErrorCode::Disabled,
                    "Settings permission is disabled",
                )
                .into());
            }
            Ok(match command {
                Command::GetOperations(_) => Payload::Operations(operations_page()),
                Command::SetOperations(_) => Payload::OperationsWrite(operations_write()),
                _ => panic!("wrong command"),
            })
        }
    }
    for version in VERSIONS {
        for disabled in [false, true] {
            let mut h = Harness::new(Arc::new(OperationsBackend { disabled }));
            if version != MODERN {
                h.send(json!({"jsonrpc":"2.0","id":0,"method":"initialize","params":{"protocolVersion":version,"capabilities":{},"clientInfo":{"name":"fixture","version":"1"}}}));
                h.receive();
                h.send(json!({"jsonrpc":"2.0","method":"notifications/initialized"}));
            }
            h.send(version_request(version, 1, "tools/list", json!({})));
            let list = h.receive();
            for (name, args) in [
                ("get_operations", json!({"key":"Ctrl+C"})),
                ("set_operations", json!({"keys":{"FsCopyImage":["Ctrl+C"]}})),
            ] {
                assert!(
                    list["result"]["tools"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .any(|tool| tool["name"] == name)
                );
                h.send(version_request(
                    version,
                    2,
                    "tools/call",
                    json!({"name":name,"arguments":args}),
                ));
                let reply = h.receive();
                assert_eq!(reply["result"]["isError"], disabled);
                if disabled {
                    assert_eq!(
                        reply["result"]["structuredContent"]["error"]["code"],
                        "disabled"
                    );
                }
            }
            h.close();
        }
    }
}

#[test]
fn operations_stdio_rejects_duplicate_addresses_before_backend() {
    let (send, _) = mpsc::sync_channel(1);
    struct NoCall;
    impl Backend for NoCall {
        fn call(
            &self,
            _: Command,
            _: &ClientIdentity,
            _: RequestKind,
            _: Arc<CallControl>,
        ) -> BackendResult {
            panic!("duplicate addresses must be rejected before the backend")
        }
    }
    let backend: Arc<dyn Backend> = Arc::new(NoCall);
    let mut session = Session::Fresh;
    let owner: ActiveOwner = Arc::new(Mutex::new(None));
    let mut serial = 0;
    let reply = handle_line(br#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"set_operations","arguments":{"keys":{"FsCopyImage":[],"FsCopyImage":null}}}}"#,
        &mut session,&owner,&backend,&send,&mut serial).unwrap();
    assert_eq!(reply.value["error"]["code"], -32700);
}
