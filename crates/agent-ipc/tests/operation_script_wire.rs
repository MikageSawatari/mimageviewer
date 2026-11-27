use mimageviewer_agent_ipc::*;
use serde_json::json;

#[test]
fn operation_script_admission_state_preserves_each_loader_state() {
    for state in [
        OperationScriptLoadState::Disabled,
        OperationScriptLoadState::Loading,
        OperationScriptLoadState::Ready,
        OperationScriptLoadState::Failed,
    ] {
        let wire = serde_json::to_value(state).unwrap();
        assert_eq!(
            serde_json::from_value::<OperationScriptLoadState>(wire).unwrap(),
            state
        );
    }
    assert!(serde_json::from_value::<OperationScriptLoadState>(json!("unknown")).is_err());
    assert_eq!(MAX_OPERATION_SCRIPT_SOURCE_BYTES, 256 * 1024);
    assert_eq!(MAX_OPERATION_SCRIPT_FAILURE_REASON_CHARS, 500);
}

#[test]
fn operation_script_v8_command_defaults_and_frames_are_additive() {
    assert_eq!(AGENT_PROTOCOL_VERSION, 8);
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
            json!({"source":"fn slots() { [()] } // 猫🦊","expected_sha256":"b".repeat(64)}),
            Some("fn slots() { [()] } // 猫🦊".into()),
            None,
            Some("b".repeat(64)),
        ),
    ] {
        let mut wire = json!({"type":"operation_script"});
        wire.as_object_mut()
            .unwrap()
            .extend(fields.as_object().unwrap().clone());
        let command = Command::OperationScript {
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
fn operation_script_response_projection_discards_unknown_fields_and_old_write_source() {
    for value in [
        json!({"source":"猫🦊\n", "sha256":"a".repeat(64),"builtin":true,"enabled_at_admission":false,"load_state_at_admission":"disabled","failure_reason_at_admission":null,"part":1,"parts":2,"message":"開き直してください","unknown":"discard"}),
        json!({"sha256":"b".repeat(64),"enabled_at_admission":true,"load_state_at_admission":"failed","failure_reason_at_admission":"line 2, column 3","before_lines":20,"after_lines":30,"message":"開き直してください","source":"discard old source","unknown":"discard"}),
    ] {
        let typed: OperationScriptResult = serde_json::from_value(value).unwrap();
        let projected = serde_json::to_value(&typed).unwrap();
        assert!(!projected.to_string().contains("discard"));
        if matches!(typed, OperationScriptResult::Write(_)) {
            assert!(projected.get("source").is_none());
        }
        let response = Response {
            id: 2,
            result: Ok(Payload::OperationScript(typed)),
        };
        let mut bytes = Vec::new();
        write_frame(&mut bytes, &response).unwrap();
        assert_eq!(
            read_frame::<_, Response>(&mut bytes.as_slice()).unwrap(),
            Some(response)
        );
    }
}
