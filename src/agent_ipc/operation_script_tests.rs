use super::operation_script::{AdmissionSnapshot, read_result, validate_arguments};
use super::*;
use crate::operation_script::{
    DEFAULT_SCRIPT, OperationScriptState, ScriptDocument, ScriptWriteOutcome,
};
use sha2::{Digest, Sha256};

fn document(source: &str, builtin: bool) -> ScriptDocument {
    ScriptDocument {
        source: source.into(),
        sha256: format!("{:x}", Sha256::digest(source.as_bytes())),
        builtin,
    }
}

fn snapshot() -> AdmissionSnapshot {
    AdmissionSnapshot::capture(false, &OperationScriptState::Disabled)
}

fn final_budget(value: &OperationScriptResult) {
    assert!(mcp_content_chars(value).unwrap() <= MAX_PAGE_CHARS);
    let structured = serde_json::to_value(value).unwrap();
    let copy = serde_json::to_string(&structured).unwrap();
    let envelope = serde_json::json!({"jsonrpc":"2.0","id":"\0".repeat(256),"result":{
        "content":[{"type":"text","text":copy}],"structuredContent":structured,"isError":false,
        "resultType":"complete","_meta":{"io.modelcontextprotocol/serverInfo":{"name":"mimageviewer","version":env!("CARGO_PKG_VERSION")}}
    }});
    assert!(serde_json::to_string(&envelope).unwrap().chars().count() <= MAX_MCP_RESPONSE_CHARS);
}

#[test]
fn operation_script_parts_preserve_source_and_boundaries_across_admission_states_and_reasons() {
    for source in [
        String::new(),
        DEFAULT_SCRIPT.into(),
        "\"\\\n\0あ😀".repeat(12000),
        (0u8..=127).map(char::from).collect::<String>().repeat(200),
    ] {
        let document = document(&source, source == DEFAULT_SCRIPT);
        let OperationScriptResult::Read(first) =
            read_result(&document, &snapshot(), None, None).unwrap()
        else {
            panic!()
        };
        let mut joined = String::new();
        for part in 1..=first.parts {
            let mut current = snapshot();
            current.enabled_at_admission = part % 2 == 0;
            current.load_state_at_admission = match part % 4 {
                0 => OperationScriptLoadState::Disabled,
                1 => OperationScriptLoadState::Loading,
                2 => OperationScriptLoadState::Ready,
                _ => OperationScriptLoadState::Failed,
            };
            if current.load_state_at_admission == OperationScriptLoadState::Failed {
                current.failure_reason_at_admission =
                    Some("\0".repeat(MAX_OPERATION_SCRIPT_FAILURE_REASON_CHARS));
            }
            let result = read_result(&document, &current, Some(part), Some(&first.sha256)).unwrap();
            final_budget(&result);
            let OperationScriptResult::Read(result) = result else {
                panic!()
            };
            let OperationScriptResult::Read(baseline) =
                read_result(&document, &snapshot(), Some(part), Some(&first.sha256)).unwrap()
            else {
                panic!()
            };
            assert_eq!(result.source, baseline.source);
            assert_eq!(result.parts, first.parts);
            assert_eq!(result.sha256, document.sha256);
            assert_eq!(
                result.load_state_at_admission,
                current.load_state_at_admission
            );
            assert_eq!(
                result.failure_reason_at_admission,
                current.failure_reason_at_admission
            );
            joined.push_str(&result.source);
        }
        assert_eq!(joined, source);
        assert!(
            read_result(
                &document,
                &snapshot(),
                Some(first.parts + 1),
                Some(&first.sha256)
            )
            .is_err()
        );
    }
}

#[test]
fn operation_script_snapshot_truncates_failure_reason_by_scalars() {
    let reason = "あ😀\0".repeat(500);
    let snapshot = AdmissionSnapshot::capture(true, &OperationScriptState::Failed(reason.clone()));
    assert!(snapshot.enabled_at_admission);
    assert_eq!(
        snapshot.load_state_at_admission,
        OperationScriptLoadState::Failed
    );
    assert_eq!(
        snapshot.failure_reason_at_admission.unwrap(),
        reason
            .chars()
            .take(MAX_OPERATION_SCRIPT_FAILURE_REASON_CHARS)
            .collect::<String>()
    );
}

#[test]
fn operation_script_requires_hash_and_valid_part_and_checks_changed_document() {
    let document = document(&"// original\n".repeat(4000), false);
    assert!(validate_arguments(Some("fn slots() { [] }"), None, None).is_err());
    assert!(read_result(&document, &snapshot(), Some(2), None).is_err());
    assert!(validate_arguments(None, Some(0), None).is_err());
    for hash in ["", "not-a-hash", &"g".repeat(64)] {
        assert!(validate_arguments(None, None, Some(hash)).is_err());
    }
    let error = read_result(&document, &snapshot(), Some(2), Some(&"a".repeat(64))).unwrap_err();
    assert_eq!(error.code, ErrorCode::BadRequest);
    assert!(error.message.contains("最初から読み直して"));
    assert!(
        validate_arguments(
            Some(&"x".repeat(MAX_OPERATION_SCRIPT_SOURCE_BYTES)),
            None,
            Some(&document.sha256)
        )
        .is_ok()
    );
    assert!(operation_script::check_write_budget().is_ok());
}

#[test]
fn operation_script_ai_cap_refuses_utf8_bytes_before_ui_admission() {
    let fixture = tests::Fixture::new();
    crate::settings_db::with_db_result(|db| {
        let mut settings = db.load_into_settings()?;
        settings.agent_allow_settings = true;
        db.save_full(&settings)
    })
    .unwrap();
    let queue = fixture.handlers.open.install(egui::Context::default());
    let connection = fixture.connection();
    for source in [
        "x".repeat(MAX_OPERATION_SCRIPT_SOURCE_BYTES + 1),
        "あ".repeat(MAX_OPERATION_SCRIPT_SOURCE_BYTES / 3 + 1),
    ] {
        let (send, replies) = crossbeam_channel::unbounded();
        let worker = connection
            .request(
                Request {
                    id: 803,
                    command: Command::OperationScript {
                        source: Some(source),
                        part: None,
                        expected_sha256: Some("a".repeat(64)),
                    },
                },
                send,
            )
            .unwrap();
        let response = replies
            .recv_timeout(Duration::from_secs(10))
            .unwrap()
            .wire();
        worker.join().unwrap();
        assert_eq!(response.result.unwrap_err().code, ErrorCode::BadRequest);
        assert!(queue.try_recv().is_err());
        assert_eq!(fixture.handlers.active.load(Ordering::Acquire), 0);
    }
}

#[test]
fn operation_script_records_once_and_logs_neither_source_nor_diagnostics() {
    let fixture = tests::Fixture::new();
    let notices = fixture.handlers.changes.install(egui::Context::default());
    let payload = operation_script::complete_write(
        Ok(ScriptWriteOutcome {
            sha256: "f".repeat(64),
            before_lines: 3,
            after_lines: 8,
        }),
        &snapshot(),
        "script-client",
        &fixture.handlers.changes,
    )
    .unwrap();
    let Payload::OperationScript(result) = &payload else {
        panic!()
    };
    final_budget(result);
    let records = fixture.handlers.changes.records();
    let records = records.lock().unwrap();
    assert_eq!(records.entries().len(), 1);
    let record = &records.entries()[0];
    assert_eq!(record.notice, "操作のスクリプトを変更 (3 行 → 8 行)");
    assert_eq!(record.rows.len(), 1);
    assert_eq!(record.rows[0].before, "3 行");
    assert_eq!(record.rows[0].after, "8 行");
    assert_eq!(notices.try_recv().unwrap(), record.notice);
    drop(records);
    let sentinel = "PRIVATE_OPERATION_SCRIPT_BODY_SENTINEL";
    let error = operation_script::complete_write(
        Err(format!("invalid syntax {sentinel}")),
        &snapshot(),
        "script-client",
        &fixture.handlers.changes,
    )
    .unwrap_err();
    assert!(error.message.contains(sentinel)); // diagnostics go to the caller only
    assert_eq!(
        notices.try_recv().unwrap(),
        "操作のスクリプトを書き込めませんでした"
    );
    for record in fixture.handlers.changes.records().lock().unwrap().entries() {
        assert!(!format!("{record:?}").contains(sentinel));
    }
    let read_payload = Payload::OperationScript(
        read_result(&document(sentinel, false), &snapshot(), None, None).unwrap(),
    );
    assert!(
        !request_log_line("operation_script", Instant::now(), &Ok(read_payload)).contains(sentinel)
    );
    assert!(!request_log_line("operation_script", Instant::now(), &Err(error)).contains(sentinel));
}

#[test]
fn operation_script_permission_off_refuses_before_ui_or_file_work() {
    let fixture = tests::Fixture::new();
    let queue = fixture.handlers.open.install(egui::Context::default());
    let connection = fixture.connection();
    for source in [None, Some("fn slots() { [] }".into())] {
        let (send, replies) = crossbeam_channel::unbounded();
        assert!(
            connection
                .request(
                    Request {
                        id: 800,
                        command: Command::OperationScript {
                            source,
                            part: None,
                            expected_sha256: Some("f".repeat(64))
                        }
                    },
                    send
                )
                .is_none()
        );
        assert_eq!(
            replies
                .recv_timeout(Duration::from_secs(10))
                .unwrap()
                .wire()
                .result
                .unwrap_err()
                .code,
            ErrorCode::Disabled
        );
        assert!(queue.try_recv().is_err());
    }
}

#[test]
fn operation_script_read_response_rechecks_permission_epoch() {
    let fixture = tests::Fixture::new();
    let granted = permission().unwrap();
    let reply = Reply {
        response: Response {
            id: 801,
            result: Ok(Payload::OperationScript(
                read_result(&document(DEFAULT_SCRIPT, true), &snapshot(), None, None).unwrap(),
            )),
        },
        policy: ReplyPolicy::Read(granted),
    };
    fixture.set_level(AgentLevel::Disabled);
    assert!(reply.wire().result.is_err());
}

/// Release the injected stop and join before the disposable directory goes away,
/// including when a pipeline assertion fails.
struct PipelineLoader {
    loader: crate::operation_script::OperationScriptLoader,
    release: Option<std::sync::mpsc::Sender<()>>,
}
impl Drop for PipelineLoader {
    fn drop(&mut self) {
        if let Some(release) = self.release.take() {
            let _ = release.send(());
        }
        self.loader.shutdown_file_worker_for_test();
    }
}

#[test]
fn operation_script_claimed_pipeline_preserves_admission_snapshot_and_completes_after_cancel() {
    use super::open::{UiOpenOutcome, UiRequestKind};
    use crate::operation_script::{CompiledOperationScript, OperationScriptLoader};
    let fixture = tests::Fixture::new();
    crate::settings_db::with_db_result(|db| {
        let mut settings = db.load_into_settings()?;
        settings.agent_allow_settings = true;
        db.save_full(&settings)
    })
    .unwrap();
    let temp = tempfile::tempdir().unwrap();
    let path = crate::operation_script::script_path(temp.path());
    let ctx = egui::Context::default();
    let (entered, observed) = std::sync::mpsc::channel();
    let (release, released) = std::sync::mpsc::channel();
    let first = std::cell::Cell::new(true);
    let mut owner = PipelineLoader {
        loader: OperationScriptLoader::with_file_worker(
            path.clone(),
            |_| Ok(()),
            move || {
                if first.replace(false) {
                    let _ = entered.send(());
                    let _ = released.recv();
                }
            },
        ),
        release: Some(release),
    };
    owner.loader.state = OperationScriptState::Failed("先の読み込みの失敗".into());
    let ui = fixture.handlers.open.install(ctx.clone());
    let notices = fixture.handlers.changes.install(ctx.clone());
    let connection = fixture.connection();
    let source = "fn slots() { [] }";
    let (send, replies) = crossbeam_channel::unbounded();
    let worker = connection
        .request(
            Request {
                id: 810,
                command: Command::OperationScript {
                    source: Some(source.into()),
                    part: None,
                    expected_sha256: Some(document(DEFAULT_SCRIPT, true).sha256),
                },
            },
            send,
        )
        .unwrap();
    let request = ui.recv_timeout(Duration::from_secs(10)).unwrap();
    assert!(request.try_claim());
    let UiRequestKind::OperationScript(job) = &request.kind else {
        panic!()
    };
    let snapshot = AdmissionSnapshot::capture(true, &owner.loader.state);
    let queued = owner
        .loader
        .enqueue_agent_script(job.clone(), temp.path(), &ctx);
    request.complete(UiOpenOutcome::OperationScriptAdmitted { snapshot, queued });
    observed.recv_timeout(Duration::from_secs(10)).unwrap();
    assert!(!path.exists());
    assert!(replies.try_recv().is_err());
    // The outer cancellation cannot revoke file work that the UI has claimed.
    assert!(
        fixture
            .call(&connection, 811, Command::Cancel { request_id: 810 })
            .result
            .is_ok()
    );
    fixture.set_level(AgentLevel::Disabled);
    owner.release.take().unwrap().send(()).unwrap();
    let reply = replies.recv_timeout(Duration::from_secs(10)).unwrap();
    worker.join().unwrap();
    assert!(matches!(reply.policy, ReplyPolicy::AcceptedOperation));
    let Payload::OperationScript(OperationScriptResult::Write(result)) =
        reply.wire().result.unwrap()
    else {
        panic!()
    };
    assert!(result.enabled_at_admission);
    assert_eq!(
        result.load_state_at_admission,
        OperationScriptLoadState::Failed
    );
    assert_eq!(
        result.failure_reason_at_admission.as_deref(),
        Some("先の読み込みの失敗")
    );
    assert_eq!(result.sha256, document(source, false).sha256);
    assert_eq!(std::fs::read_to_string(&path).unwrap(), source);
    owner.loader.shutdown_file_worker_for_test();
    assert!(owner.loader.poll().is_empty());
    assert!(
        matches!(&owner.loader.state, OperationScriptState::Ready(script) if script.slots.is_empty())
    );
    assert_eq!(
        notices.try_recv().unwrap(),
        format!(
            "操作のスクリプトを変更 ({} 行 → 1 行)",
            DEFAULT_SCRIPT.lines().count()
        )
    );
    assert!(notices.try_recv().is_err());
    assert_eq!(
        fixture
            .handlers
            .changes
            .records()
            .lock()
            .unwrap()
            .entries()
            .len(),
        1
    );
    assert_eq!(fixture.handlers.active.load(Ordering::Acquire), 0);
    assert!(ui.try_recv().is_err());
    // A later read reports the state at its own admission, independently of
    // the successful write's earlier Failed snapshot.
    fixture.set_level(AgentLevel::ReadOnly);
    owner.loader.state =
        OperationScriptState::Ready(Box::new(CompiledOperationScript::compile(source).unwrap()));
    let (send, replies) = crossbeam_channel::unbounded();
    let worker = connection
        .request(
            Request {
                id: 812,
                command: Command::OperationScript {
                    source: None,
                    part: None,
                    expected_sha256: None,
                },
            },
            send,
        )
        .unwrap();
    let request = ui.recv_timeout(Duration::from_secs(10)).unwrap();
    assert!(request.try_claim());
    let UiRequestKind::OperationScript(job) = &request.kind else {
        panic!()
    };
    let snapshot = AdmissionSnapshot::capture(true, &owner.loader.state);
    // Use a fresh injected worker because the first was explicitly joined.
    owner.loader = OperationScriptLoader::with_file_worker(path, |_| Ok(()), || {});
    let queued = owner
        .loader
        .enqueue_agent_script(job.clone(), temp.path(), &ctx);
    request.complete(UiOpenOutcome::OperationScriptAdmitted { snapshot, queued });
    let reply = replies.recv_timeout(Duration::from_secs(10)).unwrap();
    worker.join().unwrap();
    assert!(matches!(reply.policy, ReplyPolicy::Read(_)));
    let Payload::OperationScript(OperationScriptResult::Read(result)) =
        reply.wire().result.unwrap()
    else {
        panic!()
    };
    assert_eq!(result.source, source);
    assert!(!result.builtin);
    assert_eq!(
        result.load_state_at_admission,
        OperationScriptLoadState::Ready
    );
    assert!(result.failure_reason_at_admission.is_none());
}
