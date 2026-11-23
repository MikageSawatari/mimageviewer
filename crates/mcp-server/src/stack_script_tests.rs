use super::*;

fn stack_result(write: bool) -> StackScriptResult {
    serde_json::from_value(if write {
        json!({"sha256":"b".repeat(64),"enabled":true,"before_lines":4,"after_lines":5,"message":"スタック表示を開き直すか、フォルダを読み直してください","source":"discard old text","unknown":"discard"})
    } else {
        json!({"source":"fn group(x) { x } // 猫🦊\n\"\u{0000}","sha256":"a".repeat(64),"builtin":true,"enabled":false,"part":1,"parts":1,"message":"スタック表示を開き直すか、フォルダを読み直してください","unknown":"discard"})
    }).unwrap()
}

#[test]
fn stack_script_arguments_are_strict_and_guard_hashes_before_ipc() {
    let tool = schema::tools()
        .into_iter()
        .find(|t| t["name"] == "stack_script")
        .unwrap();
    for (args, source, part, hash) in [
        (json!({}), None, None, None),
        (
            json!({"source":null,"part":null,"expected_sha256":null}),
            None,
            None,
            None,
        ),
        (json!({"part":1}), None, Some(1), None),
        (
            json!({"part":2,"expected_sha256":"A".repeat(64)}),
            None,
            Some(2),
            Some("A".repeat(64)),
        ),
        (
            json!({"source":"","expected_sha256":"b".repeat(64)}),
            Some(String::new()),
            None,
            Some("b".repeat(64)),
        ),
        (
            json!({"source":"猫🦊","part":1,"expected_sha256":"a".repeat(64)}),
            Some("猫🦊".into()),
            Some(1),
            Some("a".repeat(64)),
        ),
    ] {
        assert!(accepts_schema(&tool["inputSchema"], &args));
        assert_eq!(
            parse_tool(&json!({"name":"stack_script","arguments":args})).unwrap(),
            (
                "stack_script",
                Command::StackScript {
                    source,
                    part,
                    expected_sha256: hash
                }
            )
        );
    }
    for args in [
        json!(null),
        json!({"source":false}),
        json!({"part":0}),
        json!({"part":-1}),
        json!({"part":1.5}),
        json!({"expected_sha256":0}),
        json!({"extra":1}),
        json!({"source":"x","expected_sha256":"a".repeat(64),"reset":true}),
    ] {
        assert!(!accepts_schema(&tool["inputSchema"], &args), "{args}");
        assert!(parse_tool(&json!({"name":"stack_script","arguments":args})).is_err());
    }
    // Cross-field conditions and UTF-8 bytes are checked at runtime as well as core admission.
    for args in [
        json!({"source":""}),
        json!({"source":"x","expected_sha256":null}),
        json!({"part":2}),
        json!({"expected_sha256":"g".repeat(64)}),
        json!({"expected_sha256":"a".repeat(63)}),
        json!({"expected_sha256":"a".repeat(65)}),
        json!({"source":"猫".repeat(349526),"expected_sha256":"a".repeat(64)}),
    ] {
        assert!(parse_tool(&json!({"name":"stack_script","arguments":args})).is_err());
    }
}

#[test]
fn stack_script_typed_responses_match_schema_and_final_envelope_budget() {
    let tool = schema::tools()
        .into_iter()
        .find(|t| t["name"] == "stack_script")
        .unwrap();
    for write in [false, true] {
        let result = tool_result(
            "stack_script",
            Ok(Payload::StackScript(stack_result(write))),
        );
        let body = &result["structuredContent"];
        assert_eq!(result["isError"], false);
        assert_eq!(body.get("source").is_none(), write);
        assert!(!body.to_string().contains("discard"));
        assert!(accepts_schema(&tool["outputSchema"], body));
        let required = if write {
            vec![
                "sha256",
                "enabled",
                "before_lines",
                "after_lines",
                "message",
            ]
        } else {
            vec![
                "source", "sha256", "builtin", "enabled", "part", "parts", "message",
            ]
        };
        for field in required {
            let mut missing = body.clone();
            missing.as_object_mut().unwrap().remove(field);
            assert!(
                serde_json::from_value::<StackScriptResult>(missing).is_err(),
                "missing {field}"
            );
            let mut wrong = body.clone();
            wrong[field] = Value::Null;
            assert!(
                serde_json::from_value::<StackScriptResult>(wrong).is_err(),
                "wrong {field}"
            );
        }
        assert_eq!(
            serde_json::from_str::<Value>(result["content"][0]["text"].as_str().unwrap()).unwrap(),
            *body
        );
        for modern in [false, true] {
            let envelope = success(json!("\0".repeat(256)), result.clone(), modern);
            assert!(response_non_image_chars(&envelope) <= MAX_MCP_RESPONSE_CHARS);
            assert_eq!(
                serde_json::from_str::<Value>(&serialize_response(&envelope, ResponseKind::Data))
                    .unwrap(),
                envelope
            );
        }
    }
    assert_eq!(
        tool_result("set_settings", Ok(Payload::StackScript(stack_result(true))))["isError"],
        true
    );
    assert_eq!(
        tool_result("stack_script", Ok(Payload::Cancelled))["isError"],
        true
    );
    assert_eq!(tool["outputSchema"]["anyOf"][0], json!({}));
    assert_eq!(
        tool["annotations"],
        json!({"readOnlyHint":false,"destructiveHint":false,"idempotentHint":true,"openWorldHint":false})
    );
    let description = tool["description"].as_str().unwrap();
    for text in [
        "retain old source",
        "before writing",
        "complete source",
        "Reopen stack view or reload folder",
        "Needs mIV Read + settings access.",
    ] {
        assert!(description.contains(text), "{text}");
    }
}

#[test]
fn stack_script_all_mcp_versions_discover_and_transport_reads_writes_and_refusals() {
    struct ScriptBackend;
    impl Backend for ScriptBackend {
        fn call(
            &self,
            command: Command,
            _: &ClientIdentity,
            _: RequestKind,
            _: Arc<CallControl>,
        ) -> BackendResult {
            let Command::StackScript {
                source,
                expected_sha256,
                ..
            } = command
            else {
                panic!("wrong command")
            };
            if expected_sha256.as_deref() == Some("0".repeat(64).as_str()) {
                return Err(AgentError::new(
                    ErrorCode::Conflict,
                    "読んだあとに変わっています。読み直してください",
                )
                .into());
            }
            Ok(Payload::StackScript(stack_result(source.is_some())))
        }
    }
    for version in VERSIONS {
        let mut h = Harness::new(Arc::new(ScriptBackend));
        if version != MODERN {
            h.send(json!({"jsonrpc":"2.0","id":0,"method":"initialize","params":{"protocolVersion":version,"capabilities":{},"clientInfo":{"name":"fixture","version":"1"}}}));
            h.receive();
            h.send(json!({"jsonrpc":"2.0","method":"notifications/initialized"}));
        }
        h.send(version_request(version, 1, "tools/list", json!({})));
        let reply = h.receive();
        assert!(
            reply["result"]["tools"]
                .as_array()
                .unwrap()
                .iter()
                .any(|t| t["name"] == "stack_script")
        );
        for (id, args, error) in [
            (2, json!({}), false),
            (
                3,
                json!({"source":"fn group(x) { x }","expected_sha256":"a".repeat(64)}),
                false,
            ),
            (
                4,
                json!({"source":"fn group(x) { x }","expected_sha256":"0".repeat(64)}),
                true,
            ),
            (5, json!({"part":2,"expected_sha256":"0".repeat(64)}), true),
        ] {
            h.send(version_request(
                version,
                id,
                "tools/call",
                json!({"name":"stack_script","arguments":args}),
            ));
            let reply = h.receive();
            assert_eq!(reply["result"]["isError"], error, "{reply}");
            assert!(response_non_image_chars(&reply) <= MAX_MCP_RESPONSE_CHARS);
            if error {
                assert_eq!(
                    reply["result"]["structuredContent"]["error"]["code"],
                    "conflict"
                );
            }
        }
        h.send(version_request(
            version,
            6,
            "tools/call",
            json!({"name":"stack_script","arguments":{"source":"x"}}),
        ));
        assert_eq!(h.receive()["error"]["code"], -32602);
        h.close();
    }
}
