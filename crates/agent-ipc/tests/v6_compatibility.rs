use mimageviewer_agent_ipc as current;
use serde_json::json;
mod v6_wire;

#[test]
fn frozen_v6_hello_and_response_projection_remain_compatible_with_v7() {
    let request = v6_wire::Request {
        id: 1,
        command: v6_wire::Command::Hello {
            version: 6,
            client_name: "fixture".into(),
            client_version: "1".into(),
            request_kind: v6_wire::RequestKind::Tool,
        },
    };
    let mut bytes = Vec::new();
    v6_wire::write_frame(&mut bytes, &request).unwrap();
    assert_eq!(bytes, include_bytes!("v6_wire/hello.frame"));
    assert!(current::supports_adapter_version(6));
    assert_eq!(current::MIN_ADAPTER_PROTOCOL_VERSION, 3);
    assert_eq!(v6_wire::AGENT_PROTOCOL_VERSION, 6);
    let parsed: current::Request = current::read_frame(&mut bytes.as_slice()).unwrap().unwrap();
    assert!(matches!(
        parsed.command,
        current::Command::Hello { version: 6, .. }
    ));
    for (name, payload) in [
        (
            "miv_status",
            json!({"type":"status","data":{"app_version":"fixture","search_scope":[],"search_scope_truncated":false,"permission":{"level":"read_only","thumbnails":false,"gps":false,"ai_captions":false,"transcripts":false,"current_view":true,"settings":true,"future":true},"fts_ready":false,"remote_active":false,"future":true}}),
        ),
        (
            "get_settings",
            json!({"type":"settings","data":{"items":[{"key":"fullscreen_wheel_action","label":"wheel","value":"Zoom","overridden_by_favorite":false,"choices":null,"future":true}],"page":1,"pages":1,"more":false,"future":true}}),
        ),
        (
            "set_settings",
            json!({"type":"settings_write","data":{"changed":[{"key":"fullscreen_wheel_action","label":"wheel","before":"PageTurn","after":"Zoom","overridden_by_favorite":false,"future":true}],"unchanged":[],"future":true}}),
        ),
    ] {
        let mut bytes = Vec::new();
        current::write_frame(&mut bytes, &json!({"id":2,"result":{"Ok":payload}})).unwrap();
        let response: v6_wire::Response =
            v6_wire::read_frame(&mut bytes.as_slice()).unwrap().unwrap();
        let result = v6_wire::output::tool_result(name, response.result);
        assert_eq!(result["isError"], false);
        let body = &result["structuredContent"];
        assert!(!body.to_string().contains("future"));
        let tools = v6_wire::schema::tools();
        assert!(v6_wire::schema_check::accepts_schema(
            &tools.iter().find(|tool| tool["name"] == name).unwrap()["outputSchema"],
            body
        ));
        assert!(v6_wire::mcp_content_chars(body).unwrap() <= v6_wire::MAX_PAGE_CHARS);
        assert!(
            json!({"jsonrpc":"2.0","id":"\0".repeat(256),"result":result})
                .to_string()
                .chars()
                .count()
                <= v6_wire::MAX_MCP_RESPONSE_CHARS
        );
    }
}

#[test]
fn frozen_v6_adapter_never_receives_operations_variants_and_keeps_all_old_budgets() {
    assert_eq!(current::MAX_FRAME_BYTES, v6_wire::MAX_FRAME_BYTES);
    assert_eq!(current::MAX_PAGE_CHARS, v6_wire::MAX_PAGE_CHARS);
    assert_eq!(current::MAX_ITEM_CHARS, v6_wire::MAX_ITEM_CHARS);
    for tool in v6_wire::schema::tools() {
        assert!(!matches!(
            tool["name"].as_str(),
            Some("get_operations" | "set_operations")
        ));
    }
    assert!(serde_json::from_value::<v6_wire::Command>(json!({"type":"get_operations"})).is_err());
    assert!(
        serde_json::from_value::<v6_wire::Command>(
            json!({"type":"set_operations","keys":{"FsCopyImage":null}})
        )
        .is_err()
    );
    for side in [256, 512, 513, 1568] {
        let old = v6_wire::thumbnail_budget(6, side);
        let retained = current::thumbnail_budget(6, side);
        assert_eq!(old.max_images, retained.max_images);
        assert_eq!(old.max_image_bytes, retained.max_image_bytes);
        assert_eq!(old.max_total_bytes, retained.max_total_bytes);
    }
}
