use super::*;

fn view() -> ViewState {
    ViewState {
        view: ViewKind::SearchResults,
        folder: Some("C:/pics".into()),
        fullscreen: true,
        item: Some(ViewItem {
            path: "C:/pics/book.zip".into(),
            subresource: Some("inner.zip/page.png".into()),
            name: "page.png".into(),
            kind: ItemKind::Image,
        }),
        search: ViewSearch {
            item_index: true,
            favorite: Some(ViewFavorite {
                name: "pics".into(),
                path: "C:/pics".into(),
            }),
            favorite_root: true,
        },
    }
}

#[test]
fn get_view_state_arguments_and_structured_output_match_registered_schema() {
    let (name, command) = parse_tool(&json!({"name":"get_view_state","arguments":{}})).unwrap();
    assert_eq!(name, "get_view_state");
    assert_eq!(command, Command::GetViewState);
    let tool = schema::tools()
        .into_iter()
        .find(|t| t["name"] == name)
        .unwrap();
    assert_eq!(tool["annotations"]["readOnlyHint"], true);
    for bad in [
        json!({"path":"C:/pics"}),
        json!({"cursor":"x"}),
        json!({"id":1}),
        json!(null),
    ] {
        assert!(parse_tool(&json!({"name":name,"arguments":bad})).is_err());
    }
    let expected = view();
    let result = tool_result(name, Ok(Payload::ViewState(expected.clone())));
    assert_eq!(result["isError"], false);
    assert_eq!(result["structuredContent"], json!(expected));
    assert!(accepts_schema(
        &tool["outputSchema"],
        &result["structuredContent"]
    ));
    assert_eq!(
        serde_json::from_str::<Value>(result["content"][0]["text"].as_str().unwrap()).unwrap(),
        result["structuredContent"]
    );
    assert!(mcp_content_chars(&expected).unwrap() <= MAX_PAGE_CHARS);
    let envelope = success(json!("\0".repeat(256)), result, true);
    assert!(response_non_image_chars(&envelope) <= MAX_MCP_RESPONSE_CHARS);
    assert_eq!(
        serde_json::from_str::<Value>(&serialize_response(&envelope, ResponseKind::Data)).unwrap(),
        envelope
    );
    assert_eq!(tool_result(name, Ok(Payload::Cancelled))["isError"], true);
}

#[test]
fn get_view_state_transports_read_or_permission_refusal_on_all_mcp_versions() {
    for version in VERSIONS {
        for disabled in [false, true] {
            let mut h = Harness::new(Arc::new(Fixture { disabled }));
            if version != MODERN {
                h.send(json!({"jsonrpc":"2.0","id":0,"method":"initialize","params":{"protocolVersion":version,"capabilities":{},"clientInfo":{"name":"fixture","version":"1"}}}));
                h.receive();
                h.send(json!({"jsonrpc":"2.0","method":"notifications/initialized"}));
            }
            h.send(version_request(
                version,
                1,
                "tools/call",
                json!({"name":"get_view_state","arguments":{}}),
            ));
            let reply = h.receive();
            assert_eq!(reply["result"]["isError"], disabled, "{reply}");
            if disabled {
                assert_eq!(
                    reply["result"]["structuredContent"]["error"]["code"],
                    "disabled"
                );
            } else {
                assert_eq!(reply["result"]["structuredContent"]["view"], "folder");
            }
            h.close();
        }
    }
}

#[test]
fn guide_permissions_and_favorite_index_flags_survive_typed_mcp_projection() {
    let result = tool_result(
        "miv_status",
        Ok(Payload::Status(StatusPayload {
            app_version: "fixture".into(),
            permission: PermissionSnapshot {
                level: AgentLevel::ReadOnly,
                current_view: true,
                settings: true,
                ..Default::default()
            },
            search_scope: vec![],
            search_scope_truncated: false,
            fts_ready: false,
            remote_active: false,
        })),
    );
    assert_eq!(
        result["structuredContent"]["permission"]["current_view"],
        true
    );
    assert_eq!(result["structuredContent"]["permission"]["settings"], true);
    for flags in [Some((true, false)), Some((false, true)), None] {
        let value = LibraryItem {
            path: "C:/pics".into(),
            subresource: None,
            name: "pics".into(),
            kind: ItemKind::Folder,
            id: None,
            page: None,
            added_at: None,
            last_read_at: None,
            rated_at: None,
            rating: None,
            item_index: flags.map(|f| f.0),
            container_index: flags.map(|f| f.1),
        };
        let result = tool_result(
            "list_library",
            Ok(Payload::Library(LibraryPage {
                items: vec![value.into()],
                cursor: None,
                truncated: false,
            })),
        );
        let row = &result["structuredContent"]["items"][0];
        match flags {
            Some((item, container)) => {
                assert_eq!(row["item_index"], item);
                assert_eq!(row["container_index"], container);
            }
            None => {
                assert!(row.get("item_index").is_none());
                assert!(row.get("container_index").is_none());
            }
        }
        let schema = schema::tools()
            .into_iter()
            .find(|t| t["name"] == "list_library")
            .unwrap();
        assert!(accepts_schema(
            &schema["outputSchema"],
            &result["structuredContent"]
        ));
    }
}
