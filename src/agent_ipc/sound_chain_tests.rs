use super::*;
use crate::effetune::sound_document::{GetContext, GetInput, Pipeline};
use crate::effetune::sound_service::{
    SoundError, SoundOutcome, SoundRequest, SoundResponse, SoundTicket,
};
use serde_json::json;

fn response(outcome: SoundOutcome) -> SoundResponse {
    SoundResponse {
        outcome,
        persistence_warning: None,
    }
}

fn final_budget(value: &SoundChainResult) {
    assert!(mcp_content_chars(value).unwrap() <= MAX_PAGE_CHARS);
    let structured = serde_json::to_value(value).unwrap();
    let envelope = json!({"jsonrpc":"2.0","id":"\0".repeat(256),"result":{
        "content":[{"type":"text","text":structured.to_string()}],"structuredContent":structured,
        "isError":false,"resultType":"complete","_meta":{"io.modelcontextprotocol/serverInfo":{
            "name":"mimageviewer","version":env!("CARGO_PKG_VERSION")}}
    }});
    assert!(serde_json::to_string(&envelope).unwrap().chars().count() <= MAX_MCP_RESPONSE_CHARS);
}

fn state(count: usize, large: bool) -> Vec<u8> {
    let stages = (1..=count)
        .map(|id| {
            json!({
                "id":id,"type":"SectionPlugin","name":"\"\\\0あ😀".repeat(40),"enabled":true,
                "parameters":{"gain":id,"_uiHidden":true,
                    "curve":if large { "\"\\\0あ😀".repeat(8_000) } else { "x".into() }}
            })
        })
        .collect::<Vec<_>>();
    serde_json::to_vec(
        &json!({"formatVersion":1,"pipelineA":stages,"pipelineB":null,
        "masterBypass":false,"currentPipeline":"A"}),
    )
    .unwrap()
}

fn get(bytes: &[u8], input: GetInput) -> SoundChainResult {
    let state = crate::effetune::sound_document::get(
        bytes,
        &input,
        &GetContext {
            revision: Some("token:3".into()),
            latency_secs: 0.1,
            latency_remaining_secs: 1.9,
        },
    )
    .unwrap();
    let original = response(SoundOutcome::State { state });
    let expected = serde_json::to_value(&original).unwrap();
    let result = sound_chain::wire_result(Ok(original)).unwrap();
    // The typed projection must not add a null field per row or lose explicit
    // end-of-list null; page partitioning precedes this wire conversion.
    assert_eq!(serde_json::to_value(&result).unwrap()["Ok"], expected);
    final_budget(&result);
    result
}

#[test]
fn sound_chain_wire_get_128_stages_pages_advance_and_fit_final_mcp_budget() {
    let document = state(128, false);
    let mut offset = 0;
    let mut ids = Vec::new();
    loop {
        let result = get(
            &document,
            GetInput {
                pipeline: Some(Pipeline::A),
                offset,
                ids: None,
            },
        );
        let SoundChainOutcome::State { state } = result.unwrap().outcome else {
            panic!()
        };
        assert_eq!(state.revision.as_deref(), Some("token:3"));
        assert_eq!(state.count_a, 128);
        let rows = state.effects.unwrap();
        assert!(!rows.is_empty() && rows.len() <= 32);
        assert!(
            rows.iter()
                .all(|row| row.name_truncated && row.name.chars().count() == 40)
        );
        ids.extend(rows.into_iter().map(|row| row.id));
        match state.next_offset {
            Some(Some(next)) => {
                assert!(next > offset);
                offset = next;
            }
            Some(None) => break,
            None => panic!("list response must include next_offset"),
        }
    }
    assert_eq!(ids, (1..=128u32).collect::<Vec<_>>());
}

#[test]
fn sound_chain_wire_eight_values_fit_or_become_too_long_without_losing_ids() {
    for large in [false, true] {
        let result = get(
            &state(8, large),
            GetInput {
                pipeline: Some(Pipeline::A),
                offset: 0,
                ids: Some((1..=8).collect()),
            },
        );
        let SoundChainOutcome::State { state } = result.unwrap().outcome else {
            panic!()
        };
        assert!(state.next_offset.is_none());
        let rows = state.effects.unwrap();
        assert_eq!(rows.len(), 8);
        for (index, row) in rows.into_iter().enumerate() {
            assert_eq!(row.id, index as u32 + 1);
            if large {
                assert_eq!(row.too_long, Some(true));
                assert!(row.parameters.is_none());
                assert!(row.note.unwrap().contains("画面で編集"));
            } else {
                assert!(row.too_long.is_none());
                assert!(!row.parameters.unwrap().contains_key("_uiHidden"));
            }
        }
    }
}

#[test]
fn sound_chain_wire_maps_all_service_outcomes_and_error_codes() {
    for outcome in [
        SoundOutcome::Locked {
            lock: "token".into(),
            revision: "token:0".into(),
        },
        SoundOutcome::Released {
            already_released: false,
        },
        SoundOutcome::Accepted {
            revision: "token:1".into(),
            counts: [1, 2],
            can_undo: true,
            warnings: Vec::new(),
            warning_count: 0,
            note: "受け付けました".into(),
        },
        SoundOutcome::Unchanged {
            revision: "token:1".into(),
        },
        SoundOutcome::Undone {
            revision: "token:2".into(),
        },
        SoundOutcome::PresetSaved {
            name: "MCP: test".into(),
            replaced: false,
            revision: "token:2".into(),
            note: "保存".into(),
        },
        SoundOutcome::Pending {
            message: "まだ終わっていません".into(),
        },
    ] {
        let response = response(outcome);
        let expected = serde_json::to_value(&response).unwrap();
        let wire = sound_chain::wire_result(Ok(response)).unwrap();
        assert_eq!(serde_json::to_value(&wire).unwrap()["Ok"], expected);
        final_budget(&wire);
    }
    for (error, code) in [
        (SoundError::Busy, SoundChainErrorCode::Busy),
        (SoundError::Unlocked, SoundChainErrorCode::Unlocked),
        (SoundError::Changed, SoundChainErrorCode::Changed),
        (
            SoundError::NothingToUndo,
            SoundChainErrorCode::NothingToUndo,
        ),
        (SoundError::Aborted, SoundChainErrorCode::Aborted),
        (
            SoundError::Invalid("invalid".into()),
            SoundChainErrorCode::Invalid,
        ),
        (
            SoundError::Unavailable("unavailable".into()),
            SoundChainErrorCode::Unavailable,
        ),
    ] {
        let message = error.to_string();
        let wire = sound_chain::wire_result(Err(error)).unwrap();
        final_budget(&wire);
        let mapped = wire.unwrap_err();
        assert_eq!(mapped.code, code);
        assert_eq!(mapped.message, message);
    }
}

#[test]
fn sound_chain_permission_requires_settings_for_all_actions_and_preparation_is_worker_only() {
    for args in [
        json!({"action":"lock"}),
        json!({"action":"unlock","lock":"t"}),
        json!({"action":"get"}),
        json!({"action":"undo","lock":"t"}),
        json!({"action":"apply","lock":"t","revision":"t:0","pipeline":"B","effects":[]}),
        json!({"action":"select","lock":"t","revision":"t:0","use":"A"}),
        json!({"action":"save_preset","lock":"t","pipeline":"A","name":"test"}),
    ] {
        let args = parse_sound_chain_request(args).unwrap();
        for allowed in [false, true] {
            let snapshot = PermissionSnapshot {
                level: AgentLevel::ReadOnly,
                settings: allowed,
                current_view: false,
                thumbnails: false,
                gps: false,
                ai_captions: false,
                transcripts: false,
            };
            let result = check_tool_permission(snapshot, &Command::SoundChain(args.clone()));
            assert_eq!(result.is_ok(), allowed);
        }
        assert!(sound_chain::prepare_request(args).is_ok());
    }
    let invalid = SoundChainRequest::Get {
        pipeline: Some(SoundPipeline::A),
        offset: 0,
        ids: Some(vec![1; 9]),
    };
    assert_eq!(
        sound_chain::prepare_request(invalid).unwrap_err().code,
        ErrorCode::BadRequest
    );
}

#[test]
fn sound_chain_preparation_moves_every_edit_field_into_the_service_request() {
    let args =
        parse_sound_chain_request(json!({"action":"apply","lock":"token","revision":"token:3",
        "pipeline":"B","effects":[{"id":7,"type":"VolumePlugin","enabled":false,
            "inputBus":4,"outputBus":3,"channel":"78","parameters":{"gain":-6,"array":[1,true]}}]}))
        .unwrap();
    let SoundRequest::Apply {
        lock,
        revision,
        input,
    } = sound_chain::prepare_request(args).unwrap()
    else {
        panic!()
    };
    assert_eq!(lock, "token");
    assert_eq!(revision, "token:3");
    assert_eq!(input.pipeline, Pipeline::B);
    assert_eq!(input.effects.len(), 1);
    let effect = &input.effects[0];
    assert_eq!(effect.id, Some(7));
    assert_eq!(effect.effect_type.as_deref(), Some("VolumePlugin"));
    assert_eq!(effect.enabled, Some(false));
    assert_eq!(effect.input_bus, Some(4));
    assert_eq!(effect.output_bus, Some(3));
    assert_eq!(effect.channel.as_deref(), Some("78"));
    assert_eq!(
        effect.parameters.as_ref().unwrap()["array"],
        json!([1, true])
    );
}

#[test]
fn sound_chain_claimed_get_rechecks_permission_but_mutation_keeps_accepted_result() {
    use super::open::{UiOpenOutcome, UiRequestKind};
    for read in [false, true] {
        let fixture = tests::Fixture::new();
        crate::settings_db::with_db_result(|db| {
            let mut settings = db.load_into_settings()?;
            settings.agent_allow_settings = true;
            db.save_full(&settings)
        })
        .unwrap();
        let ui = fixture.handlers.open.install(egui::Context::default());
        let connection = fixture.connection();
        let args = if read {
            SoundChainRequest::Get {
                pipeline: None,
                offset: 0,
                ids: None,
            }
        } else {
            SoundChainRequest::Lock {}
        };
        let (send, receive) = crossbeam_channel::unbounded();
        let worker = connection
            .request(
                Request {
                    id: 43,
                    command: Command::SoundChain(args),
                },
                send,
            )
            .unwrap();
        let request = ui.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(request.try_claim());
        assert!(matches!(&request.kind, UiRequestKind::SoundChain { .. }));
        fixture.set_level(AgentLevel::Disabled);
        request.complete_with(|kind| {
            let UiRequestKind::SoundChain { request, .. } = kind else {
                panic!()
            };
            assert_eq!(matches!(request, SoundRequest::Get(_)), read);
            UiOpenOutcome::SoundChainAdmitted(SoundTicket::completed_for_test(Ok(response(
                SoundOutcome::Pending {
                    message: "pending fixture".into(),
                },
            ))))
        });
        let reply = receive.recv_timeout(Duration::from_secs(5)).unwrap();
        worker.join().unwrap();
        if read {
            assert!(matches!(reply.policy, ReplyPolicy::Read(_)));
            assert_eq!(reply.wire().result.unwrap_err().code, ErrorCode::Disabled);
        } else {
            assert!(matches!(reply.policy, ReplyPolicy::AcceptedOperation));
            assert!(matches!(
                reply.wire().result,
                Ok(Payload::SoundChain(Ok(_)))
            ));
        }
    }
}
