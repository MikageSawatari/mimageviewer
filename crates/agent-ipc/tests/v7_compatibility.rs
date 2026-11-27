use mimageviewer_agent_ipc as current;
use serde_json::json;
mod v7_wire;

#[test]
fn frozen_v7_hello_and_response_projection_remain_compatible_with_v8() {
    let request = v7_wire::Request {
        id: 1,
        command: v7_wire::Command::Hello {
            version: 7,
            client_name: "fixture".into(),
            client_version: "1".into(),
            request_kind: v7_wire::RequestKind::Tool,
        },
    };
    let mut bytes = Vec::new();
    v7_wire::write_frame(&mut bytes, &request).unwrap();
    assert_eq!(bytes, include_bytes!("v7_wire/hello.frame"));
    assert_eq!(v7_wire::AGENT_PROTOCOL_VERSION, 7);
    assert_eq!(current::AGENT_PROTOCOL_VERSION, 8);
    assert_eq!(current::MIN_ADAPTER_PROTOCOL_VERSION, 3);
    assert!(current::supports_adapter_version(7));
    let parsed: current::Request = current::read_frame(&mut bytes.as_slice()).unwrap().unwrap();
    assert!(matches!(
        parsed.command,
        current::Command::Hello { version: 7, .. }
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
            "get_operations",
            json!({"type":"operations","data":{"items":[{"slot":"gesture.image","label":"gestures","value":[],"value_label":[],"default":[],"future":true}],"page":1,"pages":1,"future":true}}),
        ),
        (
            "set_operations",
            json!({"type":"operations_write","data":{"changed":[{"name":"FsCopyImage","label":"copy","before":null,"after":["Ctrl+C"],"future":true}],"unchanged":[],"conflicts":[],"future":true}}),
        ),
        (
            "stack_script",
            json!({"type":"stack_script","data":{"source":"fn group(x) { x }","sha256":"a".repeat(64),"builtin":true,"enabled":false,"part":1,"parts":1,"message":"fixture","future":true}}),
        ),
        (
            "stack_script",
            json!({"type":"stack_script","data":{"sha256":"b".repeat(64),"enabled":true,"before_lines":2,"after_lines":3,"message":"fixture","future":true}}),
        ),
    ] {
        let mut bytes = Vec::new();
        current::write_frame(&mut bytes, &json!({"id":2,"result":{"Ok":payload}})).unwrap();
        let response: v7_wire::Response =
            v7_wire::read_frame(&mut bytes.as_slice()).unwrap().unwrap();
        let result = v7_wire::output::tool_result(name, response.result);
        assert_eq!(result["isError"], false, "{name}: {result}");
        let body = &result["structuredContent"];
        assert!(!body.to_string().contains("future"));
        let tools = v7_wire::schema::tools();
        assert!(v7_wire::schema_check::accepts_schema(
            &tools.iter().find(|tool| tool["name"] == name).unwrap()["outputSchema"],
            body
        ));
        assert!(v7_wire::mcp_content_chars(body).unwrap() <= v7_wire::MAX_PAGE_CHARS);
        assert!(
            json!({"jsonrpc":"2.0","id":"\0".repeat(256),"result":result})
                .to_string()
                .chars()
                .count()
                <= v7_wire::MAX_MCP_RESPONSE_CHARS
        );
    }
}

#[test]
fn frozen_v7_adapter_has_no_operation_script_and_keeps_old_budgets() {
    assert_eq!(current::MAX_FRAME_BYTES, v7_wire::MAX_FRAME_BYTES);
    assert_eq!(current::MAX_PAGE_CHARS, v7_wire::MAX_PAGE_CHARS);
    assert_eq!(current::MAX_ITEM_CHARS, v7_wire::MAX_ITEM_CHARS);
    assert_eq!(v7_wire::schema::tools().len(), 29);
    assert!(
        v7_wire::schema::tools()
            .iter()
            .all(|tool| tool["name"] != "operation_script")
    );
    assert!(
        serde_json::from_value::<v7_wire::Command>(json!({"type":"operation_script"})).is_err()
    );
    assert!(
        serde_json::from_value::<v7_wire::Payload>(json!({"type":"operation_script","data":{}}))
            .is_err()
    );
    for side in [256, 512, 513, 1568] {
        let old = v7_wire::thumbnail_budget(7, side);
        let retained = current::thumbnail_budget(7, side);
        assert_eq!(old.max_images, retained.max_images);
        assert_eq!(old.max_image_bytes, retained.max_image_bytes);
        assert_eq!(old.max_total_bytes, retained.max_total_bytes);
    }
}
