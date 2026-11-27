use mimageviewer_agent_ipc::*;
use serde_json::{Value, json};

fn request(value: Value) -> SoundChainRequest {
    parse_sound_chain_request(value).unwrap()
}

#[test]
fn sound_chain_v9_actions_round_trip_through_command_frames() {
    assert_eq!(AGENT_PROTOCOL_VERSION, 9);
    assert_eq!(MIN_ADAPTER_PROTOCOL_VERSION, 3);
    for arguments in [
        json!({"action":"lock"}),
        json!({"action":"unlock","lock":"token"}),
        json!({"action":"get"}),
        json!({"action":"get","pipeline":"B","offset":32}),
        json!({"action":"get","pipeline":"A","ids":[1,2]}),
        json!({"action":"apply","lock":"token","revision":"token:1","pipeline":"B",
            "effects":[{"id":1},{"id":2,"type":"Gain","enabled":false,
                "parameters":{"gain":3,"nested":[{"on":true}]},
                "inputBus":0,"outputBus":4,"channel":"All"},{"type":"Gain"}]}),
        json!({"action":"apply","lock":"token","revision":"token:1","pipeline":"B","effects":[]}),
        json!({"action":"select","lock":"token","revision":"token:1","use":"B","bypass":false}),
        json!({"action":"undo","lock":"token"}),
        json!({"action":"save_preset","lock":"token","pipeline":"B","name":"猫の調整"}),
        json!({"action":"save_preset","lock":"token","pipeline":"A","name":"MCP: 猫","replace":true}),
    ] {
        let typed = request(arguments.clone());
        assert_eq!(typed.is_read(), arguments["action"] == "get");
        let mut command = arguments;
        command["type"] = json!("sound_chain");
        assert_eq!(
            serde_json::from_value::<Command>(command).unwrap(),
            Command::SoundChain(typed.clone())
        );
        let envelope = Request {
            id: 17,
            command: Command::SoundChain(typed),
        };
        let mut bytes = Vec::new();
        write_frame(&mut bytes, &envelope).unwrap();
        assert_eq!(
            read_frame::<_, Request>(&mut bytes.as_slice()).unwrap(),
            Some(envelope)
        );
    }
}

#[test]
fn sound_chain_rejects_unknown_action_fields_and_wrong_json_types() {
    for value in [
        json!({"action":"future"}),
        json!({"action":"lock","name":"unexpected"}),
        json!({"action":"get","revision":"unexpected"}),
        json!({"action":"unlock"}),
        json!({"action":"get","pipeline":"C"}),
        json!({"action":"get","offset":-1}),
        json!({"action":"get","pipeline":"A","ids":[1.5]}),
        json!({"action":"save_preset","lock":"x","pipeline":"B","name":"preset","replace":"yes"}),
        json!({"action":"apply","lock":"x","revision":"x:0","pipeline":"B","effects":{}}),
        json!({"action":"apply","lock":"x","revision":"x:0","pipeline":"B","effects":[{"type":"Gain","name":"unexpected"}]}),
        json!({"action":"apply","lock":"x","revision":"x:0","pipeline":"B","effects":[{"type":"Gain","parameters":[]}]}),
        json!({"action":"apply","lock":"x","revision":"x:0","pipeline":"B","effects":[{"type":"Gain","enabled":1}]}),
        json!({"action":"apply","lock":"x","revision":"x:0","pipeline":"B","effects":[{"type":"Gain","inputBus":256}]}),
    ] {
        assert!(parse_sound_chain_request(value.clone()).is_err(), "{value}");
    }
    for bytes in [
        br#"{"action":"lock","action":"unlock"}"#.as_slice(),
        br#"{"action":"apply","effects":[{"parameters":{"gain":1,"gain":2}}]}"#,
    ] {
        assert!(parse_unique_json(bytes).is_err());
    }
}

#[test]
fn sound_chain_validates_cross_fields_and_bounded_collections() {
    for value in [
        json!({"action":"unlock","lock":""}),
        json!({"action":"get","offset":1}),
        json!({"action":"get","ids":[1]}),
        json!({"action":"get","pipeline":"B","ids":[]}),
        json!({"action":"get","pipeline":"B","ids":[0]}),
        json!({"action":"get","pipeline":"B","ids":[1,1]}),
        json!({"action":"get","pipeline":"B","ids":[1,2],"offset":1}),
        json!({"action":"get","pipeline":"B","ids":[1,2,3,4,5,6,7,8,9]}),
        json!({"action":"apply","lock":"x","revision":"","pipeline":"B","effects":[]}),
        json!({"action":"apply","lock":"x","revision":"x:1","pipeline":"B","effects":[{}]}),
        json!({"action":"apply","lock":"x","revision":"x:1","pipeline":"B","effects":[{"id":0}]}),
        json!({"action":"apply","lock":"x","revision":"x:1","pipeline":"B","effects":[{"id":1},{"id":1}]}),
        json!({"action":"apply","lock":"x","revision":"x:1","pipeline":"B","effects":[{"id":1,"enabled":true}]}),
        json!({"action":"apply","lock":"x","revision":"x:1","pipeline":"B","effects":[{"type":"Gain","inputBus":5}]}),
        json!({"action":"select","lock":"x","revision":"x:1"}),
        json!({"action":"select","lock":"x","revision":"","bypass":true}),
    ] {
        assert!(parse_sound_chain_request(value.clone()).is_err(), "{value}");
    }
    let mut apply = json!({"action":"apply","lock":"x","revision":"x:1","pipeline":"B",
        "effects":vec![json!({"type":"Gain"}); MAX_SOUND_STAGES]});
    assert!(parse_sound_chain_request(apply.clone()).is_ok());
    apply["effects"]
        .as_array_mut()
        .unwrap()
        .push(json!({"type":"Gain"}));
    assert!(parse_sound_chain_request(apply).is_err());
    let parameters: serde_json::Map<String, Value> = (0..MAX_SOUND_PARAMETERS)
        .map(|index| (format!("key{index}"), json!(true)))
        .collect();
    let mut apply = json!({"action":"apply","lock":"x","revision":"x:1","pipeline":"B",
        "effects":[{"type":"Gain","parameters":parameters}]});
    assert!(parse_sound_chain_request(apply.clone()).is_ok());
    apply["effects"][0]["parameters"]["overflow"] = json!(true);
    assert!(parse_sound_chain_request(apply).is_err());
    for (key, accepted) in [("猫".repeat(64), true), ("猫".repeat(65), false)] {
        let parameters = serde_json::Map::from_iter([(key, json!(true))]);
        let apply = json!({"action":"apply","lock":"x","revision":"x:1","pipeline":"B",
            "effects":[{"type":"Gain","parameters":parameters}]});
        assert_eq!(parse_sound_chain_request(apply).is_ok(), accepted);
    }
}

#[test]
fn sound_chain_preset_names_include_the_automatic_prefix_in_the_limit() {
    for (name, accepted) in [
        ("猫".repeat(59), true),
        ("猫".repeat(60), false),
        (format!("MCP: {}", "猫".repeat(59)), true),
        (format!("MCP: {}", "猫".repeat(60)), false),
        ("".into(), false),
        ("MCP: ".into(), false),
        ("__proto__".into(), false),
        ("constructor".into(), false),
        ("prototype".into(), false),
        ("name\0tail".into(), false),
        ("  猫  ".into(), true),
    ] {
        assert_eq!(
            parse_sound_chain_request(json!({"action":"save_preset","lock":"x",
                "pipeline":"B","name":name}))
            .is_ok(),
            accepted,
            "{name:?}"
        );
    }
}

fn state() -> Value {
    json!({"revision":null,"locked":false,"masterBypass":false,"currentPipeline":"A",
        "countA":1,"countB":0,"latency_secs":0.01,"latency_remaining_secs":1.99,
        "note":"読み直し直後は値が合う前の場合があります"})
}

#[test]
fn sound_chain_typed_get_projection_preserves_missing_null_and_paged_shapes() {
    for mut value in [
        state(),
        {
            let mut value = state();
            value["pipeline"] = json!("A");
            value["offset"] = json!(0);
            value["next_offset"] = Value::Null;
            value["effects"] = json!([]);
            value
        },
        {
            let mut value = state();
            value["pipeline"] = json!("B");
            value["offset"] = json!(32);
            value["next_offset"] = json!(64);
            value["effects"] = json!([]);
            value
        },
        {
            let mut value = state();
            value["pipeline"] = json!("A");
            value["effects"] = json!([{"id":1,"type":"Gain","type_truncated":false,
                "name":"Gain","name_truncated":false,"enabled":true,"inputBus":0,
                "outputBus":0,"channel":"All","too_long":true,"note":"画面で編集してください"}]);
            value
        },
        {
            let mut value = state();
            value["pipeline"] = json!("A");
            value["effects"] = json!([{"id":1,"type":"Gain","type_truncated":false,
                "name":"Gain","name_truncated":false,"enabled":true,"inputBus":0,
                "outputBus":0,"channel":"All","parameters":{"gain":2,"future_parameter":true}}]);
            value
        },
    ] {
        let expected = value.clone();
        value["future_state"] = json!("discard");
        if let Some(row) = value.pointer_mut("/effects/0") {
            row["future_row"] = json!("discard");
        }
        let typed: SoundChainState = serde_json::from_value(value).unwrap();
        assert_eq!(serde_json::to_value(&typed).unwrap(), expected);
        assert!(mcp_content_chars(&typed).unwrap() <= MAX_PAGE_CHARS);
    }
}

#[test]
fn sound_chain_get_pages_keep_typed_projection_within_the_shared_output_budget() {
    for (rows, parameters) in [(32, false), (8, true)] {
        let mut value = state();
        value["revision"] = json!("token:123");
        value["locked"] = json!(true);
        value["countA"] = json!(128);
        value["pipeline"] = json!("A");
        value["effects"] = Value::Array(
            (1..=rows)
                .map(|id| {
                    let mut row = json!({"id":id,"type":"未".repeat(40),"type_truncated":true,
                "name":"猫".repeat(40),"name_truncated":true,"enabled":true,
                "inputBus":4,"outputBus":4,"channel":"All"});
                    if parameters {
                        row["parameters"] = json!({"message":"\\\"".repeat(300)});
                    }
                    row
                })
                .collect(),
        );
        if !parameters {
            value["offset"] = json!(0);
            value["next_offset"] = json!(32);
        }
        let response: SoundChainResponse = serde_json::from_value(json!({
            "outcome":{"status":"state","state":value},"persistence_warning":null
        }))
        .unwrap();
        assert!(mcp_content_chars(&response).unwrap() <= MAX_PAGE_CHARS);
        let SoundChainOutcome::State { state } = &response.outcome else {
            unreachable!()
        };
        for row in state.effects.as_ref().unwrap() {
            assert!(mcp_content_chars(row).unwrap() <= MAX_ITEM_CHARS);
        }
        let projected = serde_json::to_value(&response).unwrap();
        let result = json!({"content":[{"type":"text","text":projected.to_string()}],
            "structuredContent":projected,"isError":false});
        assert!(
            json!({"jsonrpc":"2.0","id":"\0".repeat(256),"result":result})
                .to_string()
                .chars()
                .count()
                <= MAX_MCP_RESPONSE_CHARS
        );
    }
}

#[test]
fn sound_chain_outcomes_and_command_specific_errors_are_typed_and_round_trip() {
    for outcome in [
        json!({"status":"locked","lock":"token","revision":"token:0"}),
        json!({"status":"released","already_released":true}),
        json!({"status":"state","state":state()}),
        json!({"status":"accepted","revision":"token:1","counts":[1,2],"can_undo":true,
            "warnings":[{"id":1,"key":"future_key","key_truncated":false}],"warning_count":1,"note":"受け付けました"}),
        json!({"status":"unchanged","revision":"token:1"}),
        json!({"status":"undone","revision":"token:2"}),
        json!({"status":"preset_saved","name":"MCP: 猫","replaced":true,"revision":"token:2","note":"保存しました"}),
        json!({"status":"pending","message":"まだ終わっていません"}),
    ] {
        let mut input = json!({"outcome":outcome,"persistence_warning":null});
        let expected = input.clone();
        input["future_response"] = json!("discard");
        input["outcome"]["future_outcome"] = json!("discard");
        let typed: SoundChainResponse = serde_json::from_value(input).unwrap();
        assert_eq!(serde_json::to_value(&typed).unwrap(), expected);
        let response = Response {
            id: 19,
            result: Ok(Payload::SoundChain(Ok(typed))),
        };
        let mut bytes = Vec::new();
        write_frame(&mut bytes, &response).unwrap();
        assert_eq!(
            read_frame::<_, Response>(&mut bytes.as_slice()).unwrap(),
            Some(response)
        );
    }
    for code in [
        SoundChainErrorCode::Busy,
        SoundChainErrorCode::Unlocked,
        SoundChainErrorCode::LockMismatch,
        SoundChainErrorCode::Changed,
        SoundChainErrorCode::NothingToUndo,
        SoundChainErrorCode::Aborted,
        SoundChainErrorCode::Invalid,
        SoundChainErrorCode::Unavailable,
    ] {
        assert_eq!(serde_json::to_value(&code).unwrap(), json!(code.as_str()));
        let response = Response {
            id: 20,
            result: Ok(Payload::SoundChain(Err(SoundChainError {
                code,
                message: "失敗しました".into(),
            }))),
        };
        let mut bytes = Vec::new();
        write_frame(&mut bytes, &response).unwrap();
        assert_eq!(
            read_frame::<_, Response>(&mut bytes.as_slice()).unwrap(),
            Some(response)
        );
    }
    assert!(serde_json::from_value::<ErrorCode>(json!("unlocked")).is_err());
    assert!(serde_json::from_value::<ErrorCode>(json!("lock_mismatch")).is_err());
    assert!(serde_json::from_value::<ErrorCode>(json!("nothing_to_undo")).is_err());
}
