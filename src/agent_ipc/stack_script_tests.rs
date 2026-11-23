use super::stack_script::{read_result, validate_arguments};
use super::*;
use crate::filename_stack_script::{DEFAULT_SCRIPT, ScriptDocument, ScriptWriteOutcome};
use sha2::{Digest, Sha256};

fn document(source: &str, builtin: bool) -> ScriptDocument {
    ScriptDocument {
        source: source.into(),
        sha256: format!("{:x}", Sha256::digest(source.as_bytes())),
        builtin,
    }
}

fn final_budget(value: &StackScriptResult) {
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
fn stack_script_parts_preserve_unicode_controls_and_whole_hash_in_final_budget() {
    for source in [
        String::new(),
        DEFAULT_SCRIPT.into(),
        "\"\\\n\0あ😀".repeat(12000),
        (0u8..=127).map(char::from).collect::<String>().repeat(200),
        "x".repeat(1024 * 1024),
    ] {
        let document = document(&source, source == DEFAULT_SCRIPT);
        let StackScriptResult::Read(first) = read_result(&document, true, None, None).unwrap()
        else {
            panic!()
        };
        assert_eq!(first.sha256, document.sha256);
        assert!(first.enabled);
        assert_eq!(first.builtin, document.builtin);
        let mut joined = String::new();
        for part in 1..=first.parts {
            let result = read_result(&document, true, Some(part), Some(&first.sha256)).unwrap();
            final_budget(&result);
            let StackScriptResult::Read(result) = result else {
                panic!()
            };
            assert_eq!(result.part, part);
            assert_eq!(result.parts, first.parts);
            assert_eq!(result.sha256, first.sha256);
            joined.push_str(&result.source);
        }
        assert_eq!(joined, source);
        assert!(read_result(&document, true, Some(first.parts + 1), Some(&first.sha256)).is_err());
    }
}

#[test]
fn stack_script_requires_hash_for_write_and_later_parts_and_refuses_changed_parts() {
    let original = document(&"// first\n".repeat(4000), false);
    assert!(validate_arguments(Some("fn group(files) { [] }"), None, None).is_err());
    assert!(read_result(&original, false, Some(2), None).is_err());
    assert!(validate_arguments(None, Some(0), None).is_err());
    for hash in ["", "not-a-hash", &"g".repeat(64)] {
        assert!(validate_arguments(None, None, Some(hash)).is_err());
    }
    assert!(
        validate_arguments(
            Some(&" ".repeat(MAX_STACK_SCRIPT_SOURCE_BYTES + 1)),
            None,
            Some(&original.sha256)
        )
        .is_err()
    );
    let changed = document(&"// changed\n".repeat(4000), false);
    let error = read_result(&changed, false, Some(2), Some(&original.sha256)).unwrap_err();
    assert_eq!(error.code, ErrorCode::BadRequest);
    assert!(error.message.contains("最初から読み直して"));
}

#[test]
fn stack_script_part_boundaries_ignore_metadata_changes_with_identical_source() {
    let source = "// same source\n".repeat(4000);
    let original = document(&source, false);
    let StackScriptResult::Read(first) = read_result(&original, true, None, None).unwrap() else {
        panic!()
    };
    assert!(first.parts > 1);
    let mut joined = String::new();
    for part in 1..=first.parts {
        let current = document(&source, part % 2 == 0);
        let StackScriptResult::Read(result) =
            read_result(&current, part % 2 != 0, Some(part), Some(&first.sha256)).unwrap()
        else {
            panic!()
        };
        assert_eq!(result.parts, first.parts);
        joined.push_str(&result.source);
    }
    assert_eq!(joined, source);
}

#[test]
fn stack_script_ai_cap_counts_utf8_bytes_and_refuses_before_ui_admission() {
    let hash = "a".repeat(64);
    assert!(
        validate_arguments(
            Some(&"x".repeat(MAX_STACK_SCRIPT_SOURCE_BYTES)),
            None,
            Some(&hash)
        )
        .is_ok()
    );
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
        "x".repeat(MAX_STACK_SCRIPT_SOURCE_BYTES + 1),
        "あ".repeat(MAX_STACK_SCRIPT_SOURCE_BYTES / 3 + 1),
    ] {
        let (send, replies) = crossbeam_channel::unbounded();
        let worker = connection
            .request(
                Request {
                    id: 702,
                    command: Command::StackScript {
                        source: Some(source),
                        part: None,
                        expected_sha256: Some(hash.clone()),
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
        let error = response.result.unwrap_err();
        assert_eq!(error.code, ErrorCode::BadRequest);
        assert!(error.message.contains("256 KiB"));
        assert!(queue.try_recv().is_err());
        assert_eq!(fixture.handlers.active.load(Ordering::Acquire), 0);
    }
}

#[test]
fn stack_script_records_and_logging_contain_lines_but_never_the_body() {
    let fixture = tests::Fixture::new();
    let ctx = egui::Context::default();
    let notices = fixture.handlers.changes.install(ctx);
    let payload = stack_script::complete_write(
        Ok(ScriptWriteOutcome {
            sha256: "f".repeat(64),
            before_lines: 3,
            after_lines: 8,
        }),
        false,
        "script-client",
        &fixture.handlers.changes,
    )
    .unwrap();
    let Payload::StackScript(result) = &payload else {
        panic!()
    };
    final_budget(result);
    let records = fixture.handlers.changes.records();
    let records = records.lock().unwrap();
    assert_eq!(records.entries().len(), 1);
    let record = &records.entries()[0];
    assert_eq!(record.notice, "スタック用スクリプトを変更 (3 行 → 8 行)");
    assert_eq!(record.rows[0].before, "3 行");
    assert_eq!(record.rows[0].after, "8 行");
    assert_eq!(notices.try_recv().unwrap(), record.notice);
    let Payload::StackScript(StackScriptResult::Write(write)) = &payload else {
        panic!()
    };
    assert!(write.message.contains("フォルダを読み直す"));
    assert!(!record.notice.contains("書く前に読み"));
    drop(records);
    let sentinel = "PRIVATE_SCRIPT_BODY_SENTINEL";
    stack_script::complete_write(
        Err(format!("invalid syntax {sentinel}")),
        true,
        "script-client",
        &fixture.handlers.changes,
    )
    .unwrap_err();
    assert!(!notices.try_recv().unwrap().contains(sentinel));
    for record in fixture.handlers.changes.records().lock().unwrap().entries() {
        assert!(!format!("{record:?}").contains(sentinel));
    }
    let read_payload =
        Payload::StackScript(read_result(&document(sentinel, false), false, None, None).unwrap());
    assert!(
        !request_log_line("stack_script", Instant::now(), &Ok(read_payload)).contains(sentinel)
    );
}

#[test]
fn stack_script_permission_off_refuses_before_ui_or_file_work() {
    let fixture = tests::Fixture::new();
    let queue = fixture.handlers.open.install(egui::Context::default());
    let connection = fixture.connection();
    for source in [None, Some("fn group(files) { [] }".into())] {
        let (send, replies) = crossbeam_channel::unbounded();
        assert!(
            connection
                .request(
                    Request {
                        id: 700,
                        command: Command::StackScript {
                            source,
                            part: None,
                            expected_sha256: Some("f".repeat(64)),
                        }
                    },
                    send
                )
                .is_none()
        );
        let response = replies
            .recv_timeout(Duration::from_secs(10))
            .unwrap()
            .wire();
        assert_eq!(response.result.unwrap_err().code, ErrorCode::Disabled);
        assert!(queue.try_recv().is_err());
    }
}

#[test]
fn stack_script_read_response_rechecks_permission_epoch() {
    let fixture = tests::Fixture::new();
    let granted = permission().unwrap();
    let payload = Payload::StackScript(
        read_result(&document(DEFAULT_SCRIPT, true), false, None, None).unwrap(),
    );
    let reply = Reply {
        response: Response {
            id: 701,
            result: Ok(payload),
        },
        policy: ReplyPolicy::Read(granted),
    };
    fixture.set_level(AgentLevel::Disabled);
    assert!(reply.wire().result.is_err());
}
