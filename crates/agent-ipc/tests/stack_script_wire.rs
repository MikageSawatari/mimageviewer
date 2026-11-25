use mimageviewer_agent_ipc::*;
use serde_json::json;

#[test]
fn stack_script_v6_command_defaults_and_frames_are_additive() {
    assert_eq!(AGENT_PROTOCOL_VERSION, 7);
    assert_eq!(MIN_ADAPTER_PROTOCOL_VERSION, 3);
    for (fields, source, part, expected_sha256) in [
        (json!({}), None, None, None),
        (
            json!({"source":null,"part":null,"expected_sha256":null}),
            None,
            None,
            None,
        ),
        (
            json!({"part":2,"expected_sha256":"a".repeat(64)}),
            None,
            Some(2),
            Some("a".repeat(64)),
        ),
        (
            json!({"source":"fn group(x) { x } // 猫🦊","expected_sha256":"b".repeat(64)}),
            Some("fn group(x) { x } // 猫🦊".into()),
            None,
            Some("b".repeat(64)),
        ),
    ] {
        let mut wire = json!({"type":"stack_script"});
        wire.as_object_mut()
            .unwrap()
            .extend(fields.as_object().unwrap().clone());
        let command = Command::StackScript {
            source,
            part,
            expected_sha256,
        };
        assert_eq!(serde_json::from_value::<Command>(wire).unwrap(), command);
        let request = Request { id: 1, command };
        let mut bytes = Vec::new();
        write_frame(&mut bytes, &request).unwrap();
        assert_eq!(
            read_frame::<_, Request>(&mut bytes.as_slice()).unwrap(),
            Some(request)
        );
    }
}

#[test]
fn stack_script_response_projection_discards_unknown_fields_and_old_write_source() {
    for value in [
        json!({"source":"猫🦊\n", "sha256":"a".repeat(64),"builtin":true,"enabled":false,"part":1,"parts":2,"message":"開き直してください","unknown":"discard"}),
        json!({"sha256":"b".repeat(64),"enabled":true,"before_lines":20,"after_lines":30,"message":"開き直してください","source":"discard old source","unknown":"discard"}),
    ] {
        let typed: StackScriptResult = serde_json::from_value(value).unwrap();
        let projected = serde_json::to_value(&typed).unwrap();
        assert!(!projected.to_string().contains("discard"));
        if matches!(typed, StackScriptResult::Write(_)) {
            assert!(projected.get("source").is_none());
        }
        let response = Response {
            id: 2,
            result: Ok(Payload::StackScript(typed)),
        };
        let mut bytes = Vec::new();
        write_frame(&mut bytes, &response).unwrap();
        assert_eq!(
            read_frame::<_, Response>(&mut bytes.as_slice()).unwrap(),
            Some(response)
        );
    }
}
