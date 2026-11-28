use mimageviewer_agent_ipc as current;
use serde_json::json;
mod v8_wire;

#[test]
fn frozen_v8_hello_and_response_projection_remain_compatible_with_v9() {
    let request = v8_wire::Request {
        id: 1,
        command: v8_wire::Command::Hello {
            version: 8,
            client_name: "fixture".into(),
            client_version: "1".into(),
            request_kind: v8_wire::RequestKind::Tool,
        },
    };
    let mut bytes = Vec::new();
    v8_wire::write_frame(&mut bytes, &request).unwrap();
    assert_eq!(bytes, include_bytes!("v8_wire/hello.frame"));
    assert_eq!(v8_wire::AGENT_PROTOCOL_VERSION, 8);
    assert_eq!(current::AGENT_PROTOCOL_VERSION, 9);
    assert_eq!(current::MIN_ADAPTER_PROTOCOL_VERSION, 3);
    assert!(current::supports_adapter_version(8));
    let parsed: current::Request = current::read_frame(&mut bytes.as_slice()).unwrap().unwrap();
    assert!(matches!(
        parsed.command,
        current::Command::Hello { version: 8, .. }
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
        (
            "operation_script",
            json!({"type":"operation_script","data":{"source":"fn slots() { [()] }","sha256":"c".repeat(64),"builtin":true,"enabled_at_admission":true,"load_state_at_admission":"ready","failure_reason_at_admission":null,"part":1,"parts":1,"message":"fixture","future":true}}),
        ),
        (
            "operation_script",
            json!({"type":"operation_script","data":{"sha256":"d".repeat(64),"enabled_at_admission":true,"load_state_at_admission":"failed","failure_reason_at_admission":"line 2","before_lines":2,"after_lines":3,"message":"fixture","future":true}}),
        ),
    ] {
        let mut bytes = Vec::new();
        current::write_frame(&mut bytes, &json!({"id":2,"result":{"Ok":payload}})).unwrap();
        let response: v8_wire::Response =
            v8_wire::read_frame(&mut bytes.as_slice()).unwrap().unwrap();
        let result = v8_wire::output::tool_result(name, response.result);
        assert_eq!(result["isError"], false, "{name}: {result}");
        let body = &result["structuredContent"];
        assert!(!body.to_string().contains("future"));
        let tools = v8_wire::schema::tools();
        assert!(v8_wire::schema_check::accepts_schema(
            &tools.iter().find(|tool| tool["name"] == name).unwrap()["outputSchema"],
            body
        ));
        assert!(v8_wire::mcp_content_chars(body).unwrap() <= v8_wire::MAX_PAGE_CHARS);
        assert!(
            json!({"jsonrpc":"2.0","id":"\0".repeat(256),"result":result})
                .to_string()
                .chars()
                .count()
                <= v8_wire::MAX_MCP_RESPONSE_CHARS
        );
    }
}

#[test]
fn frozen_v8_adapter_has_no_sound_chain_and_keeps_old_budgets() {
    assert_eq!(current::MAX_FRAME_BYTES, v8_wire::MAX_FRAME_BYTES);
    assert_eq!(current::MAX_PAGE_CHARS, v8_wire::MAX_PAGE_CHARS);
    assert_eq!(current::MAX_ITEM_CHARS, v8_wire::MAX_ITEM_CHARS);
    assert_eq!(v8_wire::schema::tools().len(), 30);
    assert!(
        v8_wire::schema::tools()
            .iter()
            .all(|tool| tool["name"] != "sound_chain")
    );
    assert!(serde_json::from_value::<v8_wire::Command>(json!({"type":"sound_chain"})).is_err());
    assert!(
        serde_json::from_value::<v8_wire::Payload>(json!({"type":"sound_chain","data":{}}))
            .is_err()
    );
    for side in [256, 512, 513, 1568] {
        let old = v8_wire::thumbnail_budget(8, side);
        let retained = current::thumbnail_budget(8, side);
        assert_eq!(old.max_images, retained.max_images);
        assert_eq!(old.max_image_bytes, retained.max_image_bytes);
        assert_eq!(old.max_total_bytes, retained.max_total_bytes);
    }
}
