use super::*;

fn favorite_write(added: bool) -> FavoriteWriteResult {
    serde_json::from_value(json!({
        "path":"C:/猫🦊", "name":"猫🦊", "added":added,
        "before":if added { Value::Null } else { json!({"item_index":false,"container_index":true,"unknown":"discard"}) },
        "after":{"item_index":true,"container_index":false,"unknown":"discard"},
        "message":if added { Some("新しく登録したお気に入りは AI から削除できません。削除するときは mIV の「お気に入り → 編集」を使ってください") } else { None },
        "unknown":"discard"
    })).unwrap()
}

#[test]
fn favorite_arguments_are_strict_and_preserve_optional_flags_and_text() {
    let tool = schema::tools()
        .into_iter()
        .find(|t| t["name"] == "set_favorite")
        .unwrap();
    for (args, expected) in [
        (
            json!({"path":"C:/猫🦊"}),
            Command::SetFavorite {
                path: "C:/猫🦊".into(),
                name: None,
                item_index: None,
                container_index: None,
            },
        ),
        (
            json!({"path":"C:/猫🦊","name":null,"item_index":null,"container_index":null}),
            Command::SetFavorite {
                path: "C:/猫🦊".into(),
                name: None,
                item_index: None,
                container_index: None,
            },
        ),
        (
            json!({"path":"C:/猫🦊","name":"名前🦊","item_index":false,"container_index":true}),
            Command::SetFavorite {
                path: "C:/猫🦊".into(),
                name: Some("名前🦊".into()),
                item_index: Some(false),
                container_index: Some(true),
            },
        ),
        // Path/name validity and existing-favorite semantics belong to core.
        (
            json!({"path":"relative","name":"","item_index":true}),
            Command::SetFavorite {
                path: "relative".into(),
                name: Some(String::new()),
                item_index: Some(true),
                container_index: None,
            },
        ),
    ] {
        assert!(accepts_schema(&tool["inputSchema"], &args));
        let (name, command) = parse_tool(&json!({"name":"set_favorite","arguments":args})).unwrap();
        assert_eq!(name, "set_favorite");
        assert_eq!(command, expected);
    }
    for args in [
        json!(null),
        json!({}),
        json!({"path":null}),
        json!({"path":1}),
        json!({"path":""}),
        json!({"path":"C:/pics","name":false}),
        json!({"path":"C:/pics","item_index":0}),
        json!({"path":"C:/pics","container_index":"false"}),
        json!({"path":"C:/pics","delete":true}),
    ] {
        assert!(!accepts_schema(&tool["inputSchema"], &args), "{args}");
        assert!(
            parse_tool(&json!({"name":"set_favorite","arguments":args})).is_err(),
            "{args}"
        );
    }
}

#[test]
fn favorite_typed_projection_preserves_required_null_and_fits_final_budget() {
    let tool = schema::tools()
        .into_iter()
        .find(|t| t["name"] == "set_favorite")
        .unwrap();
    for added in [false, true] {
        let result = tool_result(
            "set_favorite",
            Ok(Payload::FavoriteWrite(favorite_write(added))),
        );
        assert_eq!(result["isError"], false);
        let body = &result["structuredContent"];
        assert_eq!(body["added"], added);
        assert!(body.get("before").is_some());
        assert_eq!(body["before"].is_null(), added);
        assert_eq!(body.get("message").is_some(), added);
        if added {
            let message = body["message"].as_str().unwrap();
            assert!(message.contains("AI から削除できません"));
            assert!(message.contains("お気に入り → 編集"));
        }
        assert!(!body.to_string().contains("discard"));
        assert!(accepts_schema(&tool["outputSchema"], body));
        assert_eq!(
            serde_json::from_str::<Value>(result["content"][0]["text"].as_str().unwrap()).unwrap(),
            *body
        );
        assert!(mcp_content_chars(body).unwrap() <= MAX_PAGE_CHARS);
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
    assert!(!accepts_schema(&tool["outputSchema"], &json!({})));
    assert_eq!(
        tool_result("set_favorite", Ok(Payload::Cancelled))["isError"],
        true
    );
    assert_eq!(
        tool_result(
            "set_settings",
            Ok(Payload::FavoriteWrite(favorite_write(false)))
        )["isError"],
        true
    );
}

#[test]
fn favorite_discovery_and_calls_work_on_all_mcp_versions() {
    struct FavoriteBackend {
        disabled: bool,
    }
    impl Backend for FavoriteBackend {
        fn call(
            &self,
            command: Command,
            _: &ClientIdentity,
            _: RequestKind,
            _: Arc<CallControl>,
        ) -> BackendResult {
            assert_eq!(
                command,
                Command::SetFavorite {
                    path: "C:/猫🦊".into(),
                    name: None,
                    item_index: Some(true),
                    container_index: Some(false)
                }
            );
            if self.disabled {
                return Err(AgentError::new(
                    ErrorCode::Disabled,
                    "Settings permission is disabled",
                )
                .into());
            }
            Ok(Payload::FavoriteWrite(favorite_write(true)))
        }
    }
    for version in VERSIONS {
        for disabled in [false, true] {
            let mut h = Harness::new(Arc::new(FavoriteBackend { disabled }));
            if version != MODERN {
                h.send(json!({"jsonrpc":"2.0","id":0,"method":"initialize","params":{"protocolVersion":version,"capabilities":{},"clientInfo":{"name":"fixture","version":"1"}}}));
                h.receive();
                h.send(json!({"jsonrpc":"2.0","method":"notifications/initialized"}));
            }
            h.send(version_request(version, 1, "tools/list", json!({})));
            let listed = h.receive();
            assert!(
                listed["result"]["tools"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|t| t["name"] == "set_favorite")
            );
            h.send(version_request(version, 2, "tools/call", json!({"name":"set_favorite","arguments":{"path":"C:/猫🦊","item_index":true,"container_index":false}})));
            let reply = h.receive();
            assert_eq!(reply["result"]["isError"], disabled, "{reply}");
            if disabled {
                assert_eq!(
                    reply["result"]["structuredContent"]["error"]["code"],
                    "disabled"
                );
            } else {
                assert_eq!(reply["result"]["structuredContent"]["added"], true);
            }
            h.close();
        }
    }
}

#[test]
fn favorite_discovery_preserves_the_measured_draft_and_annotations() {
    let tool = schema::tools()
        .into_iter()
        .find(|t| t["name"] == "set_favorite")
        .unwrap();
    assert_eq!(serde_json::to_string(&tool).unwrap().chars().count(), 900);
    assert_eq!(
        tool["annotations"],
        json!({"readOnlyHint":false,"destructiveHint":false,"idempotentHint":true,"openWorldHint":false})
    );
    assert!(
        tool["description"]
            .as_str()
            .unwrap()
            .contains("AI cannot delete new favorites")
    );
    assert_eq!(
        tool["outputSchema"]["anyOf"][0]["required"],
        json!(["added", "before", "after"])
    );
}
