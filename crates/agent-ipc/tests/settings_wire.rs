use mimageviewer_agent_ipc::*;
use serde_json::{Value, json};

#[test]
fn settings_commands_and_payloads_roundtrip_without_protocol_change() {
    assert_eq!(AGENT_PROTOCOL_VERSION, 6);
    assert_eq!(MIN_ADAPTER_PROTOCOL_VERSION, 3);
    for command in [
        Command::GetSettings {
            query: None,
            keys: None,
            page: None,
        },
        Command::GetSettings {
            query: Some("wheel zoom".into()),
            keys: Some(vec!["fullscreen_wheel_action".into()]),
            page: Some(2),
        },
        Command::SetSettings {
            changes: json!({"fullscreen_wheel_action":"Zoom", "save_search_input_history":false})
                .as_object()
                .unwrap()
                .clone(),
        },
    ] {
        let request = Request { id: 1, command };
        let mut bytes = Vec::new();
        write_frame(&mut bytes, &request).unwrap();
        assert_eq!(
            read_frame::<_, Request>(&mut bytes.as_slice()).unwrap(),
            Some(request)
        );
    }
    let omitted: Command = serde_json::from_value(json!({"type":"get_settings"})).unwrap();
    assert_eq!(
        omitted,
        Command::GetSettings {
            query: None,
            keys: None,
            page: None
        }
    );
    for payload in [
        Payload::Settings(serde_json::from_value(json!({"items":[
            {"key":"a", "label":"Setting", "value":true, "overridden_by_favorite":false},
            {"key":"b", "label":"Setting", "value":2, "overridden_by_favorite":false, "choices":null},
            {"key":"c", "label":"Setting", "value":"Zoom", "overridden_by_favorite":true, "choices":[{"value":"Zoom", "label":"Setting"}]},
            {"id":"a".repeat(64),"error":"too_long"}
        ],"page":1,"pages":1,"more":false})).unwrap()),
        Payload::SettingsWrite(serde_json::from_value(json!({"changed":[{"key":"a","label":"Setting","before":1,"after":2,"overridden_by_favorite":false,"rows":[{"item":"Override","before":"Value","after":"Value"}]}],"unchanged":[]})).unwrap()),
    ] {
        let response = Response { id: 2, result: Ok(payload) };
        let mut bytes = Vec::new();
        write_frame(&mut bytes, &response).unwrap();
        assert_eq!(read_frame::<_, Response>(&mut bytes.as_slice()).unwrap(), Some(response.clone()));
        assert!(mcp_content_chars(&response).unwrap() <= MAX_PAGE_CHARS);
    }
}

#[test]
fn settings_choices_distinguish_omitted_null_and_values_after_typed_roundtrip() {
    let row = json!({"key":"key", "label":"Setting", "value":1, "overridden_by_favorite":false});
    for choices in [
        None,
        Some(Value::Null),
        Some(json!([{"value":1,"label":"Setting","unknown":"discard"}])),
    ] {
        let mut input = row.clone();
        if let Some(choices) = &choices {
            input["choices"] = choices.clone();
        }
        input["unknown"] = json!("discard");
        let typed: SettingRow = serde_json::from_value(input).unwrap();
        let output = serde_json::to_value(&typed).unwrap();
        assert_eq!(output.get("choices").is_some(), choices.is_some());
        if choices == Some(Value::Null) {
            assert_eq!(typed.choices, Some(None));
        }
        assert!(!output.to_string().contains("discard"));
        assert_eq!(serde_json::from_value::<SettingRow>(output).unwrap(), typed);
    }
}
