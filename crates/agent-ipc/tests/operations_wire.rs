use mimageviewer_agent_ipc::*;
use serde_json::json;

#[test]
fn operations_wire_round_trips_new_commands_and_typed_responses() {
    assert_eq!(AGENT_PROTOCOL_VERSION, 9);
    for command in [
        json!({"type":"get_operations", "view":"keys", "key":"Ctrl+C", "screen":"image_fullscreen"}),
        json!({"type":"set_operations", "keys":{"FsCopyImage":["Ctrl+C"]}, "inputs":{"gesture.image":[{"pattern":"DR","action":"none"}]}}),
    ] {
        let request: Request = serde_json::from_value(json!({"id":2,"command":command})).unwrap();
        let mut bytes = Vec::new();
        write_frame(&mut bytes, &request).unwrap();
        let decoded: Request = read_frame(&mut bytes.as_slice()).unwrap().unwrap();
        assert_eq!(decoded, request);
    }
    for value in [
        json!({"type":"operations", "data":{"items":[{"slot":"gesture.image","label":"gestures","value":[{"pattern":"DR","action":"none"}],"value_label":["DR: none"],"default":[],"discard":"unknown"}],"page":1,"pages":1}}),
        json!({"type":"operations_write", "data":{"changed":[{"name":"FsCopyImage","label":"copy","before":null,"after":["Ctrl+C"],"discard":true}],"unchanged":[],"conflicts":[]}}),
    ] {
        let typed: Payload = serde_json::from_value(value).unwrap();
        assert!(
            !serde_json::to_value(typed)
                .unwrap()
                .to_string()
                .contains("discard")
        );
    }
}

#[test]
fn operations_query_validates_view_arguments_counts_and_pages() {
    for value in [
        json!({}),
        json!({"key":"Shift", "screen":"erase"}),
        json!({"query":"画像 コピー", "context":"Grid"}),
        json!({"view":"inputs","slots":"mouse.image"}),
        json!({"view":"choices","slot":"gesture.image","page":2}),
        json!({"view":"conflicts"}),
        json!({"view":"conflicts","actions":["FsCopyImage"],"key":"Ctrl+C"}),
    ] {
        serde_json::from_value::<OperationsQuery>(value)
            .unwrap()
            .validate()
            .unwrap();
    }
    for value in [
        json!({"view":"unknown"}),
        json!({"page":0}),
        json!({"actions":[]}),
        json!({"actions":vec!["a";33]}),
        json!({"query":"x".repeat(201)}),
        json!({"query":"copy","key":"Ctrl+C"}),
        json!({"query":"copy","actions":["FsCopyImage"]}),
        json!({"context":"Grid", "screen":"grid"}),
        json!({"view":"inputs","key":"Ctrl+C"}),
        json!({"view":"keys", "slots":"ring"}),
        json!({"view":"conflicts","key":"Ctrl+C"}),
        json!({"view":"conflicts","actions":["FsCopyImage"]}),
        json!({"view":"conflicts","actions":["FsCopyImage","FsClose"],"key":"Ctrl+C"}),
        json!({"view":"conflicts","screen":"image_fullscreen"}),
        json!({"view":"choices"}),
        json!({"view":"choices","slot":"gesture.image", "query":"copy"}),
    ] {
        assert!(
            serde_json::from_value::<OperationsQuery>(value.clone())
                .unwrap()
                .validate()
                .is_err(),
            "{value}"
        );
    }
    for value in [
        json!({"unknown":true}),
        json!({"page":-1}),
        json!({"page":1.5}),
        json!({"actions":[1]}),
    ] {
        assert!(serde_json::from_value::<OperationsQuery>(value).is_err());
    }
}

#[test]
fn operations_edits_validate_combined_limit_and_preserve_raw_values() {
    let mut keys = serde_json::Map::new();
    for index in 0..MAX_SETTINGS_KEYS {
        keys.insert(format!("action{index}"), json!(["F13", "F13"]));
    }
    let args = OperationEdits {
        keys: Some(keys),
        inputs: None,
    };
    args.validate().unwrap();
    let mut over = args.clone();
    over.inputs = Some(serde_json::from_value(json!({"option.gamepad_enabled":false})).unwrap());
    assert!(over.validate().is_err());
    assert!(OperationEdits::default().validate().is_err());
    assert_eq!(
        serde_json::from_value::<OperationEdits>(serde_json::to_value(&args).unwrap()).unwrap(),
        args
    );
    assert!(serde_json::from_value::<OperationEdits>(json!({"keys":[],"inputs":{}})).is_err());
    assert!(serde_json::from_value::<OperationEdits>(json!({"keys":{},"other":true})).is_err());
}

#[test]
fn operations_ipc_frame_rejects_duplicate_addresses_before_map_projection() {
    for bytes in [
        br#"{"id":1,"command":{"type":"set_operations","keys":{"FsCopyImage":[],"FsCopyImage":null}}}"#.as_slice(),
        br#"{"id":1,"command":{"type":"set_operations","inputs":{"gesture.image":[],"gesture.image":null}}}"#,
    ] {
        let mut frame = (bytes.len() as u32).to_le_bytes().to_vec(); frame.extend_from_slice(bytes);
        assert!(read_frame::<_, Request>(&mut frame.as_slice()).is_err());
    }
}
