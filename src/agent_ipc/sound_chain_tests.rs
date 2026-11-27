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

fn pending_permit_case(read: bool, terminal: &str, client_end: &str) {
    use super::open::{UiOpenOutcome, UiRequestKind};
    use crate::effetune::sound_service::sound_job_with_fake_host;
    use crate::video::dsp::bridge::{SoundCommand, SoundReply};
    use base64::Engine;

    let fixture = tests::Fixture::new();
    crate::settings_db::with_db_result(|db| {
        let mut settings = db.load_into_settings()?;
        settings.agent_allow_settings = true;
        db.save_full(&settings)
    })
    .unwrap();
    let ui = fixture.handlers.open.install(egui::Context::default());
    let connection = fixture.connection();
    let other = fixture.connection();
    let third = fixture.connection();
    let (other_permit, _) = other.acquire(900).unwrap();
    let command = Command::SoundChain(if read {
        SoundChainRequest::Get {
            pipeline: None,
            offset: 0,
            ids: None,
        }
    } else {
        SoundChainRequest::Lock {}
    });
    let (send, receive) = crossbeam_channel::unbounded();
    let worker = connection
        .request(Request { id: 43, command }, send)
        .unwrap();
    let request = ui.recv_timeout(Duration::from_secs(5)).unwrap();
    assert!(request.try_claim());
    let mut receive = Some(receive);
    if client_end == "eof" {
        // The pipe's EOF path disconnects the connection and drops its reader.
        connection.disconnect();
        drop(receive.take());
    }
    let mut held = None;
    request.complete_with(|kind| {
        let UiRequestKind::SoundChain {
            request,
            client_name,
            permit,
        } = kind
        else {
            panic!()
        };
        assert!(permit.is_some(), "real agent admission must carry its slot");
        let permit = permit.map(|pending| pending.upgrade().unwrap());
        let (ticket, commands, replies, run) = sound_job_with_fake_host(
            request,
            client_name,
            Arc::clone(&fixture.handlers.changes),
            permit,
        );
        held = Some((commands, replies, run));
        UiOpenOutcome::SoundChainAdmitted(ticket.with_wait_budget(Duration::ZERO))
    });
    let (commands, host_replies, run) = held.unwrap();
    // The unstarted closure owns the same job as the FIFO. In queue_drop we
    // keep it queued through the response deadline, then discard that queue.
    enum HeldJob<F> {
        Queued(F),
        Running(std::thread::JoinHandle<bool>),
    }
    let held = if terminal == "queue_drop" {
        HeldJob::Queued(run)
    } else {
        HeldJob::Running(std::thread::spawn(run))
    };
    let first = match &held {
        HeldJob::Queued(_) => None,
        HeldJob::Running(_) => Some(commands.recv_timeout(Duration::from_secs(5)).unwrap()),
    };
    if let Some(receive) = receive.as_ref() {
        let reply = receive.recv_timeout(Duration::from_secs(5)).unwrap().wire();
        let Ok(Payload::SoundChain(Ok(response))) = reply.result else {
            panic!("expected Pending")
        };
        assert!(matches!(
            response.outcome,
            SoundChainOutcome::Pending { .. }
        ));
    }
    worker.join().unwrap();
    if client_end == "cancel" {
        assert!(
            fixture
                .call(&connection, 44, Command::Cancel { request_id: 43 })
                .result
                .is_ok()
        );
    } else if client_end == "disconnect" {
        connection.disconnect();
    }
    assert_eq!(fixture.handlers.active.load(Ordering::Acquire), 2);
    assert!(connection.running.lock().unwrap().is_some());
    let next_sound = || {
        Command::SoundChain(SoundChainRequest::Get {
            pipeline: None,
            offset: 0,
            ids: None,
        })
    };
    assert_eq!(
        fixture
            .call(&connection, 45, next_sound())
            .result
            .unwrap_err()
            .code,
        ErrorCode::Busy
    );
    assert_eq!(
        fixture
            .call(&third, 46, next_sound())
            .result
            .unwrap_err()
            .code,
        ErrorCode::Busy
    );
    assert!(
        fixture
            .handlers
            .changes
            .records()
            .lock()
            .unwrap()
            .entries()
            .is_empty()
    );
    assert!(
        ui.try_recv().is_err(),
        "Busy requests must not add FIFO work"
    );

    if terminal == "success" {
        let reply = |status: &str, state| SoundReply {
            request_id: 1,
            slot_id: 0,
            status: status.into(),
            lock: Some("test-lock".into()),
            generation: 0,
            state,
            guard: None,
        };
        if !read {
            assert!(matches!(first, Some(SoundCommand::Lock)));
            host_replies.send(Ok(reply("Locked", None))).unwrap();
        } else {
            assert!(matches!(first, Some(SoundCommand::Get)));
            host_replies
                .send(Ok(reply(
                    "State",
                    Some(base64::engine::general_purpose::STANDARD.encode(state(0, false))),
                )))
                .unwrap();
        }
        // Existing service persistence also fetches state after a successful get.
        assert!(matches!(
            commands.recv_timeout(Duration::from_secs(5)).unwrap(),
            SoundCommand::Get
        ));
        host_replies
            .send(Ok(reply(
                "State",
                Some(base64::engine::general_purpose::STANDARD.encode(state(0, false))),
            )))
            .unwrap();
    } else if terminal == "invalid" {
        host_replies
            .send(Err(SoundError::Invalid("fake rejection".into())))
            .unwrap();
    } else if terminal == "aborted" {
        host_replies.send(Err(SoundError::Aborted)).unwrap();
    } else {
        assert!(matches!(terminal, "host_eof" | "queue_drop"));
        drop(host_replies);
    }
    match held {
        HeldJob::Queued(job) => drop(job),
        HeldJob::Running(worker) => {
            assert_eq!(worker.join().unwrap(), terminal == "success");
        }
    }
    assert_eq!(fixture.handlers.active.load(Ordering::Acquire), 1);
    assert!(connection.running.lock().unwrap().is_none());
    assert!(fixture.call(&third, 47, Command::Status).result.is_ok());
    assert_eq!(
        fixture
            .handlers
            .changes
            .records()
            .lock()
            .unwrap()
            .entries()
            .len(),
        usize::from(terminal != "queue_drop" && !(terminal == "success" && read))
    );
    drop(other_permit);
    assert_eq!(fixture.handlers.active.load(Ordering::Acquire), 0);
}

#[test]
fn sound_chain_pending_keeps_permit_until_fake_host_terminal() {
    for read in [false, true] {
        for terminal in ["success", "invalid", "aborted", "host_eof", "queue_drop"] {
            pending_permit_case(read, terminal, "connected");
        }
    }
}

#[test]
fn sound_chain_pending_keeps_permit_after_cancel_disconnect_and_eof() {
    for read in [false, true] {
        for client_end in ["cancel", "disconnect", "eof"] {
            pending_permit_case(read, "aborted", client_end);
        }
    }
}

#[test]
fn sound_chain_refusal_and_other_metadata_tools_release_permits() {
    use super::open::{UiOpenOutcome, UiRequestKind};
    let fixture = tests::Fixture::new();
    crate::settings_db::with_db_result(|db| {
        let mut settings = db.load_into_settings()?;
        settings.agent_allow_settings = true;
        db.save_full(&settings)
    })
    .unwrap();
    let ui = fixture.handlers.open.install(egui::Context::default());
    let connection = fixture.connection();
    for sound in [true, false] {
        let command = if sound {
            Command::SoundChain(SoundChainRequest::Lock {})
        } else {
            Command::SetSettings {
                changes: json!({"fullscreen_wheel_action":"PageTurn"})
                    .as_object()
                    .unwrap()
                    .clone(),
            }
        };
        let (send, receive) = crossbeam_channel::unbounded();
        let worker = connection
            .request(Request { id: 51, command }, send)
            .unwrap();
        let request = ui.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(request.try_claim());
        assert_eq!(
            matches!(&request.kind, UiRequestKind::SoundChain { .. }),
            sound
        );
        assert_eq!(fixture.handlers.active.load(Ordering::Acquire), 1);
        request.complete(UiOpenOutcome::Refused(error(
            ErrorCode::Busy,
            "fake UI refusal",
        )));
        assert_eq!(
            receive
                .recv_timeout(Duration::from_secs(5))
                .unwrap()
                .wire()
                .result
                .unwrap_err()
                .code,
            ErrorCode::Busy
        );
        worker.join().unwrap();
        assert_eq!(fixture.handlers.active.load(Ordering::Acquire), 0);
        assert!(connection.running.lock().unwrap().is_none());
        assert!(
            fixture
                .call(&connection, 52, Command::Status)
                .result
                .is_ok()
        );
    }
}

#[test]
fn sound_chain_pending_load_holds_permit_until_rejection_or_controller_drop() {
    use super::open::{UiOpenOutcome, UiRequestKind};
    use crate::effetune::{EffetuneController, EffetuneFailure, EffetuneRuntime, LoadOrigin};

    for read in [false, true] {
        for reject_load in [false, true] {
            let fixture = tests::Fixture::new();
            crate::settings_db::with_db_result(|db| {
                let mut settings = db.load_into_settings()?;
                settings.agent_allow_settings = true;
                db.save_full(&settings)
            })
            .unwrap();
            let ui = fixture.handlers.open.install(egui::Context::default());
            let connection = fixture.connection();
            let mut controller = EffetuneController::new();
            // No actual load worker or host exists. Keep the job in the real
            // controller's pending-load queue without entering start_load.
            controller.runtime = EffetuneRuntime::Loading {
                origin: LoadOrigin::SoundAgent,
                open_gui_when_ready: None,
            };
            let command = Command::SoundChain(if read {
                SoundChainRequest::Get {
                    pipeline: None,
                    offset: 0,
                    ids: None,
                }
            } else {
                SoundChainRequest::Lock {}
            });
            let (send, receive) = crossbeam_channel::unbounded();
            let worker = connection
                .request(Request { id: 61, command }, send)
                .unwrap();
            let request = ui.recv_timeout(Duration::from_secs(5)).unwrap();
            assert!(request.try_claim());
            request.complete_with(|kind| {
                let UiRequestKind::SoundChain {
                    request,
                    client_name,
                    permit,
                } = kind
                else {
                    panic!()
                };
                assert!(permit.is_some());
                let permit = permit.map(|pending| pending.upgrade().unwrap());
                let ticket = controller.submit_sound(
                    request,
                    &client_name,
                    Arc::clone(&fixture.handlers.changes),
                    0.0,
                    permit,
                );
                UiOpenOutcome::SoundChainAdmitted(ticket.with_wait_budget(Duration::ZERO))
            });
            let reply = receive.recv_timeout(Duration::from_secs(5)).unwrap().wire();
            assert!(matches!(
                reply.result,
                Ok(Payload::SoundChain(Ok(SoundChainResponse {
                    outcome: SoundChainOutcome::Pending { .. },
                    ..
                })))
            ));
            worker.join().unwrap();
            assert_eq!(fixture.handlers.active.load(Ordering::Acquire), 1);
            assert_eq!(
                fixture
                    .call(&connection, 62, Command::Status)
                    .result
                    .unwrap_err()
                    .code,
                ErrorCode::Busy
            );
            if reject_load {
                controller.set_test_startup_completion(Err(EffetuneFailure::LoadFailed(
                    "fake load rejection".into(),
                )));
                assert!(controller.poll());
                assert!(matches!(controller.runtime, EffetuneRuntime::Failed(_)));
            }
            // Dropping the controller also drops pending jobs if mIV exits
            // before its load completion is delivered to the UI.
            drop(controller);
            assert_eq!(fixture.handlers.active.load(Ordering::Acquire), 0);
            assert!(connection.running.lock().unwrap().is_none());
            assert_eq!(
                fixture
                    .handlers
                    .changes
                    .records()
                    .lock()
                    .unwrap()
                    .entries()
                    .len(),
                usize::from(reject_load)
            );
            assert!(
                fixture
                    .call(&connection, 63, Command::Status)
                    .result
                    .is_ok()
            );
        }
    }
}

#[test]
fn sound_chain_unclaimed_cancel_eof_and_admission_deadline_release_queued_permits() {
    use super::open::UiRequestKind;

    for read in [false, true] {
        for end in ["cancel", "eof", "admission_deadline"] {
            let fixture = tests::Fixture::new();
            crate::settings_db::with_db_result(|db| {
                let mut settings = db.load_into_settings()?;
                settings.agent_allow_settings = true;
                db.save_full(&settings)
            })
            .unwrap();
            let ui = fixture.handlers.open.install(egui::Context::default());
            let (enqueued, enqueue_rx) = std::sync::mpsc::channel();
            fixture.handlers.open.set_enqueue_observer(enqueued);
            let first = fixture.connection();
            let second = fixture.connection();
            let third = fixture.connection();

            for connection in [&first, &second] {
                let command = Command::SoundChain(if read {
                    SoundChainRequest::Get {
                        pipeline: None,
                        offset: 0,
                        ids: None,
                    }
                } else {
                    SoundChainRequest::Lock {}
                });
                let (send, receive) = crossbeam_channel::unbounded();
                let worker = connection
                    .request(Request { id: 71, command }, send)
                    .unwrap();
                // Observe the enqueue without receiving the UI request. Both
                // stale entries must remain in the actual queue below.
                enqueue_rx.recv_timeout(Duration::from_secs(5)).unwrap();
                if end == "eof" {
                    connection.disconnect();
                    drop(receive);
                } else {
                    if end == "cancel" {
                        assert!(
                            fixture
                                .call(connection, 72, Command::Cancel { request_id: 71 })
                                .result
                                .is_ok()
                        );
                    }
                    // admission_deadline uses the real five-second UI budget,
                    // not the twenty-second outer tool deadline or a sleep.
                    let reply = receive.recv_timeout(Duration::from_secs(10)).unwrap();
                    assert!(matches!(reply.policy, ReplyPolicy::Rejected));
                    let error = reply.wire().result.unwrap_err();
                    assert_eq!(
                        error.code,
                        if end == "cancel" {
                            ErrorCode::Cancelled
                        } else {
                            ErrorCode::Timeout
                        }
                    );
                    if end == "admission_deadline" {
                        assert_eq!(error.message, "mIV が応答しません");
                    }
                }
                worker.join().unwrap();
                assert!(connection.running.lock().unwrap().is_none());
                assert_eq!(fixture.handlers.active.load(Ordering::Acquire), 0);
            }

            // Status is usable with both cancelled requests still queued.
            assert!(fixture.call(&third, 73, Command::Status).result.is_ok());
            let (first_replacement, _) = first.acquire(74).unwrap();
            let (second_replacement, _) = second.acquire(75).unwrap();
            assert_eq!(fixture.handlers.active.load(Ordering::Acquire), 2);
            for index in 0..2 {
                let stale = ui.recv_timeout(Duration::from_secs(5)).unwrap();
                let UiRequestKind::SoundChain {
                    permit: Some(pending),
                    ..
                } = &stale.kind
                else {
                    panic!("expected a queued sound request with a weak permit");
                };
                assert!(pending.upgrade().is_none());
                if index == 0 {
                    // The real UI skips a cancelled request at this CAS,
                    // before it can upgrade the permit or submit any job.
                    assert!(!stale.try_claim());
                }
                // Also cover discarding the other queued request outright.
                drop(stale);
                assert_eq!(fixture.handlers.active.load(Ordering::Acquire), 2);
                assert_eq!(first.running.lock().unwrap().as_ref().unwrap().id, 74);
                assert_eq!(second.running.lock().unwrap().as_ref().unwrap().id, 75);
            }
            assert!(ui.try_recv().is_err());
            assert!(
                fixture
                    .handlers
                    .changes
                    .records()
                    .lock()
                    .unwrap()
                    .entries()
                    .is_empty()
            );
            assert_eq!(
                fixture
                    .call(&third, 76, Command::Status)
                    .result
                    .unwrap_err()
                    .code,
                ErrorCode::Busy
            );
            drop(first_replacement);
            drop(second_replacement);
            assert_eq!(fixture.handlers.active.load(Ordering::Acquire), 0);
            assert!(fixture.call(&third, 77, Command::Status).result.is_ok());
        }
    }
}
