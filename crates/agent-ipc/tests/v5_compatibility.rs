use mimageviewer_agent_ipc as current;
use serde_json::json;
mod v5_wire;

#[test]
fn frozen_v5_hello_keeps_exact_frame_and_is_still_accepted_by_v6_core() {
    let request = v5_wire::Request {
        id: 1,
        command: v5_wire::Command::Hello {
            version: 5,
            client_name: "fixture".into(),
            client_version: "1".into(),
            request_kind: v5_wire::RequestKind::Tool,
        },
    };
    let mut bytes = Vec::new();
    v5_wire::write_frame(&mut bytes, &request).unwrap();
    assert_eq!(bytes, include_bytes!("v5_wire/hello.frame"));
    assert_eq!(v5_wire::AGENT_PROTOCOL_VERSION, 5);
    assert_eq!(current::AGENT_PROTOCOL_VERSION, 6);
    assert_eq!(current::MIN_ADAPTER_PROTOCOL_VERSION, 3);
    assert!(current::supports_adapter_version(5));
    assert!(
        !current::check_version(5).is_ok(),
        "new adapter still requires the current core version"
    );
    let decoded: current::Request = current::read_frame(&mut bytes.as_slice()).unwrap().unwrap();
    assert!(matches!(
        decoded.command,
        current::Command::Hello { version: 5, .. }
    ));
}

#[test]
fn frozen_v5_adapter_ignores_new_permission_and_favorite_fields() {
    for (name, payload) in [
        (
            "miv_status",
            json!({"type":"status","data":{"app_version":"fixture","search_scope":[],"search_scope_truncated":false,"permission":{"level":"read_only","thumbnails":false,"gps":false,"ai_captions":false,"transcripts":false,"current_view":true,"settings":true},"fts_ready":false,"remote_active":false}}),
        ),
        (
            "list_library",
            json!({"type":"library","data":{"items":[{"path":"C:/pics","subresource":null,"name":"pics","kind":"folder","id":"id","page":null,"added_at":null,"last_read_at":null,"rated_at":null,"rating":null,"item_index":true,"container_index":false}],"cursor":null,"truncated":false}}),
        ),
    ] {
        let mut bytes = Vec::new();
        current::write_frame(&mut bytes, &json!({"id":2,"result":{"Ok":payload}})).unwrap();
        let old: v5_wire::Response = v5_wire::read_frame(&mut bytes.as_slice()).unwrap().unwrap();
        let result = v5_wire::output::tool_result(name, old.result);
        assert_eq!(result["isError"], false);
        let body = &result["structuredContent"];
        assert!(body.pointer("/permission/current_view").is_none());
        assert!(body.pointer("/permission/settings").is_none());
        assert!(body.pointer("/items/0/item_index").is_none());
        assert!(body.pointer("/items/0/container_index").is_none());
        let tools = v5_wire::schema::tools();
        let tool = tools.iter().find(|t| t["name"] == name).unwrap();
        assert!(v5_wire::schema_check::accepts_schema(
            &tool["outputSchema"],
            body
        ));
        assert!(v5_wire::mcp_content_chars(body).unwrap() <= v5_wire::MAX_PAGE_CHARS);
        assert!(
            json!({"jsonrpc":"2.0","id":"\0".repeat(256),"result":result})
                .to_string()
                .chars()
                .count()
                <= v5_wire::MAX_MCP_RESPONSE_CHARS
        );
    }
    let old: current::PermissionSnapshot = serde_json::from_value(json!({"level":"read_only","thumbnails":false,"gps":false,"ai_captions":false,"transcripts":false})).unwrap();
    assert!(!old.current_view && !old.settings);
}

#[test]
fn frozen_v5_output_budgets_and_large_thumbnail_contract_are_preserved() {
    assert_eq!(current::MAX_FRAME_BYTES, v5_wire::MAX_FRAME_BYTES);
    assert_eq!(current::MAX_PAGE_CHARS, v5_wire::MAX_PAGE_CHARS);
    assert_eq!(current::MAX_ITEM_CHARS, v5_wire::MAX_ITEM_CHARS);
    for side in [256, 512, 513, 1568] {
        let old = v5_wire::thumbnail_budget(5, side);
        let retained = current::thumbnail_budget(5, side);
        assert_eq!(old.max_images, retained.max_images);
        assert_eq!(old.max_image_bytes, retained.max_image_bytes);
        assert_eq!(old.max_total_bytes, retained.max_total_bytes);
    }
}

#[test]
fn frozen_v5_adapter_never_registers_new_settings_commands() {
    for tool in v5_wire::schema::tools() {
        assert!(!matches!(
            tool["name"].as_str(),
            Some("get_settings" | "set_settings" | "set_favorite")
        ));
    }
}

#[test]
fn favorite_commands_and_payloads_are_only_decodable_by_v6_types() {
    let command = current::Command::SetFavorite {
        path: "C:/pics".into(),
        name: None,
        item_index: Some(true),
        container_index: None,
    };
    let value = serde_json::to_value(&command).unwrap();
    assert!(serde_json::from_value::<v5_wire::Command>(value).is_err());
    let payload = current::Payload::FavoriteWrite(current::FavoriteWriteResult {
        path: "C:/pics".into(),
        name: "pics".into(),
        added: false,
        before: Some(current::FavoriteIndexState {
            item_index: false,
            container_index: false,
        }),
        after: current::FavoriteIndexState {
            item_index: true,
            container_index: false,
        },
        message: None,
    });
    assert!(
        serde_json::from_value::<v5_wire::Payload>(serde_json::to_value(payload).unwrap()).is_err()
    );
}
