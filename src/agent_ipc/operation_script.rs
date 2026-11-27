//! UI admission snapshots and bounded responses; file work belongs to the loader FIFO.
use super::changes::{
    ChangeController, ChangeCounts, ChangeRecord, ChangeRecordId, ChangeRow, ChangeRowOutcome,
    ChangeRowResult,
};
use crate::operation_script::{ScriptDocument, ScriptWriteOutcome};
use mimageviewer_agent_ipc::{
    AgentError, ErrorCode, MAX_OPERATION_SCRIPT_FAILURE_REASON_CHARS,
    MAX_OPERATION_SCRIPT_SOURCE_BYTES, MAX_PAGE_CHARS, OperationScriptLoadState,
    OperationScriptReadResult, OperationScriptResult, OperationScriptWriteResult, Payload,
    mcp_content_chars,
};

const MESSAGE: &str = "有効化は利用者が環境設定の「操作のスクリプト」で行います。有効なら書いた内容を自動で使い、OFF なら有効にすると使えます。スロットのキーへの割り当ては set_operations の「スクリプト操作 N」です。仕様は search_help の「操作のスクリプト」を参照してください。";

#[derive(Clone, Debug)]
pub(crate) struct AdmissionSnapshot {
    pub enabled_at_admission: bool,
    pub load_state_at_admission: OperationScriptLoadState,
    pub failure_reason_at_admission: Option<String>,
}

impl AdmissionSnapshot {
    pub(crate) fn capture(
        enabled: bool,
        state: &crate::operation_script::OperationScriptState,
    ) -> Self {
        use crate::operation_script::OperationScriptState;
        let (load_state_at_admission, reason) = match state {
            OperationScriptState::Disabled => (OperationScriptLoadState::Disabled, None),
            OperationScriptState::Loading(_) => (OperationScriptLoadState::Loading, None),
            OperationScriptState::Ready(_) => (OperationScriptLoadState::Ready, None),
            OperationScriptState::Failed(reason) => {
                (OperationScriptLoadState::Failed, Some(reason))
            }
        };
        Self {
            enabled_at_admission: enabled,
            load_state_at_admission,
            failure_reason_at_admission: reason.map(|reason| {
                reason
                    .chars()
                    .take(MAX_OPERATION_SCRIPT_FAILURE_REASON_CHARS)
                    .collect()
            }),
        }
    }
}

fn bad(message: &str) -> AgentError {
    super::error(ErrorCode::BadRequest, message)
}

pub(super) fn validate_arguments(
    source: Option<&str>,
    part: Option<u32>,
    expected: Option<&str>,
) -> Result<(), AgentError> {
    if part == Some(0) {
        return Err(bad("part は 1 以上で指定してください"));
    }
    if expected
        .is_some_and(|hash| hash.len() != 64 || !hash.bytes().all(|byte| byte.is_ascii_hexdigit()))
    {
        return Err(bad(
            "expected_sha256 は SHA-256 の 64 桁の値を指定してください",
        ));
    }
    if (source.is_some() || part.unwrap_or(1) > 1) && expected.is_none() {
        return Err(bad(
            "書き込みと 2 つ目以降の part には expected_sha256 が必要です。最初から読み直してください",
        ));
    }
    if source.is_some_and(|source| source.len() > MAX_OPERATION_SCRIPT_SOURCE_BYTES) {
        return Err(bad(
            "AI から書き込む操作のスクリプトは UTF-8 で 256 KiB 以下にしてください",
        ));
    }
    Ok(())
}

fn check_budget(value: &impl serde::Serialize) -> Result<(), AgentError> {
    if mcp_content_chars(value).map_err(|_| super::internal())? > MAX_PAGE_CHARS {
        return Err(bad("スクリプトの応答が長すぎます"));
    }
    Ok(())
}

fn parts(document: &ScriptDocument) -> Result<Vec<std::ops::Range<usize>>, AgentError> {
    // Reserve the widest state and 500 maximally escaped scalars, including
    // the JSON text copy. Neither state nor failure reason can move a boundary.
    let bound = OperationScriptReadResult {
        source: String::new(),
        sha256: document.sha256.clone(),
        builtin: false,
        enabled_at_admission: false,
        load_state_at_admission: OperationScriptLoadState::Disabled,
        failure_reason_at_admission: Some("\0".repeat(MAX_OPERATION_SCRIPT_FAILURE_REASON_CHARS)),
        part: u32::MAX,
        parts: u32::MAX,
        message: MESSAGE.into(),
    };
    let available = MAX_PAGE_CHARS
        .checked_sub(mcp_content_chars(&bound).map_err(|_| super::internal())?)
        .ok_or_else(|| bad("スクリプトの応答が長すぎます"))?;
    let (mut result, mut start, mut used) = (Vec::new(), 0, 0);
    for (index, scalar) in document.source.char_indices() {
        let cost = match scalar {
            '"' | '\\' => 6,
            '\n' | '\r' | '\t' | '\u{0008}' | '\u{000c}' => 5,
            '\u{0000}'..='\u{001f}' => 13,
            _ => 2,
        };
        if cost > available {
            return Err(bad("スクリプトの応答が長すぎます"));
        }
        if used + cost > available {
            result.push(start..index);
            start = index;
            used = 0;
        }
        used += cost;
    }
    result.push(start..document.source.len());
    Ok(result)
}

pub(super) fn read_result(
    document: &ScriptDocument,
    snapshot: &AdmissionSnapshot,
    part: Option<u32>,
    expected: Option<&str>,
) -> Result<OperationScriptResult, AgentError> {
    validate_arguments(None, part, expected)?;
    if expected.is_some_and(|expected| !expected.eq_ignore_ascii_case(&document.sha256)) {
        return Err(bad(
            "読んでいる途中で変わりました。最初から読み直してください",
        ));
    }
    let ranges = parts(document)?;
    let part = part.unwrap_or(1);
    let range = ranges
        .get(part as usize - 1)
        .ok_or_else(|| bad("指定された part はありません"))?;
    let result = OperationScriptResult::Read(OperationScriptReadResult {
        source: document.source[range.clone()].into(),
        sha256: document.sha256.clone(),
        builtin: document.builtin,
        enabled_at_admission: snapshot.enabled_at_admission,
        load_state_at_admission: snapshot.load_state_at_admission.clone(),
        failure_reason_at_admission: snapshot.failure_reason_at_admission.clone(),
        part,
        parts: ranges.len() as u32,
        message: MESSAGE.into(),
    });
    check_budget(&result)?;
    Ok(result)
}

pub(super) fn complete_write(
    outcome: Result<ScriptWriteOutcome, String>,
    snapshot: &AdmissionSnapshot,
    client: &str,
    changes: &ChangeController,
) -> Result<Payload, AgentError> {
    let outcome = match outcome {
        Ok(outcome) => outcome,
        Err(message) => {
            publish(changes, client, None);
            // Compiler diagnostics can contain source fragments; log only the category.
            crate::logger::log("agent_ipc: tool=operation_script write_failed");
            return Err(bad(&message));
        }
    };
    let result = OperationScriptWriteResult {
        sha256: outcome.sha256,
        enabled_at_admission: snapshot.enabled_at_admission,
        load_state_at_admission: snapshot.load_state_at_admission.clone(),
        failure_reason_at_admission: snapshot.failure_reason_at_admission.clone(),
        before_lines: outcome.before_lines,
        after_lines: outcome.after_lines,
        message: MESSAGE.into(),
    };
    publish(
        changes,
        client,
        Some((result.before_lines, result.after_lines)),
    );
    crate::logger::log(format!(
        "agent_ipc: tool=operation_script lines={}->{}",
        result.before_lines, result.after_lines
    ));
    Ok(Payload::OperationScript(OperationScriptResult::Write(
        result,
    )))
}

/// Prove the final write response fits before admitting a side effect.
pub(super) fn check_write_budget() -> Result<(), AgentError> {
    check_budget(&OperationScriptWriteResult {
        sha256: "f".repeat(64),
        enabled_at_admission: false,
        load_state_at_admission: OperationScriptLoadState::Disabled,
        failure_reason_at_admission: Some("\0".repeat(MAX_OPERATION_SCRIPT_FAILURE_REASON_CHARS)),
        before_lines: usize::MAX,
        after_lines: usize::MAX,
        message: MESSAGE.into(),
    })
}

fn publish(changes: &ChangeController, client: &str, lines: Option<(usize, usize)>) {
    let summary = lines.map_or_else(
        || "操作のスクリプトを書き込めませんでした".into(),
        |(before, after)| format!("操作のスクリプトを変更 ({before} 行 → {after} 行)"),
    );
    let (before, after, result) = match lines {
        Some((before, after)) => (
            format!("{before} 行"),
            format!("{after} 行"),
            ChangeRowResult::Done(ChangeRowOutcome::Changed),
        ),
        None => (
            String::new(),
            String::new(),
            ChangeRowResult::Failed("書き込みを完了できませんでした".into()),
        ),
    };
    changes.publish(ChangeRecord {
        id: ChangeRecordId::default(),
        at: std::time::SystemTime::now(),
        client_name: client.into(),
        tool: "operation_script".into(),
        collection: None,
        counts: ChangeCounts {
            changed: usize::from(lines.is_some()),
            failed: usize::from(lines.is_none()),
            ..Default::default()
        },
        notice: summary.clone(),
        summary,
        rows: vec![ChangeRow {
            item: "操作のスクリプト".into(),
            before,
            after,
            result,
        }],
    });
}
