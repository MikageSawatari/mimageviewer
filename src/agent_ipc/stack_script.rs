//! UI admission is separate from worker-only reads and FIFO writes.
use super::changes::{
    ChangeController, ChangeCounts, ChangeRecord, ChangeRecordId, ChangeRow, ChangeRowOutcome,
    ChangeRowResult,
};
use crate::filename_stack_script::{self, ScriptDocument};
use mimageviewer_agent_ipc::{
    AgentError, ErrorCode, MAX_PAGE_CHARS, MAX_STACK_SCRIPT_SOURCE_BYTES, Payload,
    StackScriptReadResult, StackScriptResult, StackScriptWriteResult, mcp_content_chars,
};

const MESSAGE: &str = "書く前に読み、戻せるようにその本文を持っておいてください。256 KiB を超える本文はエディタから戻してください。スタック表示を開き直すか、フォルダを読み直すと新しいスクリプトで分けます。";

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
    if source.is_some_and(|source| source.len() > MAX_STACK_SCRIPT_SOURCE_BYTES) {
        return Err(bad(
            "AI から書き込むスタック用スクリプトは UTF-8 で 256 KiB 以下にしてください",
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

/// Partition by the exact JSON + escaped-text-copy cost of each scalar.
/// Widest metadata keeps boundaries independent of the enabled/builtin state:
/// those can change between parts without changing the source hash.
fn parts(document: &ScriptDocument) -> Result<Vec<std::ops::Range<usize>>, AgentError> {
    let bound = StackScriptReadResult {
        source: String::new(),
        sha256: document.sha256.clone(),
        builtin: false,
        enabled: false,
        part: u32::MAX,
        parts: u32::MAX,
        message: MESSAGE.into(),
    };
    let available = MAX_PAGE_CHARS
        .checked_sub(mcp_content_chars(&bound).map_err(|_| super::internal())?)
        .ok_or_else(|| bad("スクリプトの応答が長すぎます"))?;
    let mut result = Vec::new();
    let mut start = 0;
    let mut used = 0;
    for (index, scalar) in document.source.char_indices() {
        // serde_json escapes quotes/backslashes and ASCII controls. Count both
        // serialized source and that serialized string's escaped text copy.
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
    enabled: bool,
    part: Option<u32>,
    expected: Option<&str>,
) -> Result<StackScriptResult, AgentError> {
    validate_arguments(None, part, expected)?;
    if expected.is_some_and(|expected| !expected.eq_ignore_ascii_case(&document.sha256)) {
        return Err(bad(
            "読んでいる途中で変わりました。最初から読み直してください",
        ));
    }
    let parts = parts(document)?;
    let part = part.unwrap_or(1);
    let source = parts
        .get(part as usize - 1)
        .ok_or_else(|| bad("指定された part はありません"))?;
    let result = StackScriptResult::Read(StackScriptReadResult {
        source: document.source[source.clone()].into(),
        sha256: document.sha256.clone(),
        builtin: document.builtin,
        enabled,
        part,
        parts: parts.len() as u32,
        message: MESSAGE.into(),
    });
    check_budget(&result)?;
    Ok(result)
}

pub(super) fn execute(
    source: Option<String>,
    part: Option<u32>,
    expected: Option<String>,
    enabled: bool,
    client: &str,
    changes: &ChangeController,
) -> Result<Payload, AgentError> {
    validate_arguments(source.as_deref(), part, expected.as_deref())?;
    if let Some(source) = source {
        // Worst-case line-count widths prove the final response fits before any
        // job changes the file. Never place source text in records or logs.
        let bound = StackScriptWriteResult {
            sha256: "f".repeat(64),
            enabled,
            before_lines: usize::MAX,
            after_lines: usize::MAX,
            message: MESSAGE.into(),
        };
        check_budget(&bound)?;
        let outcome =
            filename_stack_script::write_script(source, expected.unwrap().to_ascii_lowercase());
        complete_write(outcome, enabled, client, changes)
    } else {
        let document = filename_stack_script::read_script_document()
            .map_err(|_| super::error(ErrorCode::Internal, "スクリプトを読み込めませんでした"))?;
        read_result(&document, enabled, part, expected.as_deref()).map(Payload::StackScript)
    }
}

pub(super) fn complete_write(
    outcome: Result<filename_stack_script::ScriptWriteOutcome, String>,
    enabled: bool,
    client: &str,
    changes: &ChangeController,
) -> Result<Payload, AgentError> {
    let outcome = match outcome {
        Ok(outcome) => outcome,
        Err(message) => {
            publish(changes, client, None);
            return Err(bad(&message));
        }
    };
    let result = StackScriptWriteResult {
        sha256: outcome.sha256,
        enabled,
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
        "agent_ipc: tool=stack_script lines={}->{}",
        result.before_lines, result.after_lines
    ));
    Ok(Payload::StackScript(StackScriptResult::Write(result)))
}

fn publish(changes: &ChangeController, client: &str, lines: Option<(usize, usize)>) {
    let summary = lines.map_or_else(
        || "スタック用スクリプトを書き込めませんでした".into(),
        |(before, after)| format!("スタック用スクリプトを変更 ({before} 行 → {after} 行)"),
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
        tool: "stack_script".into(),
        collection: None,
        counts: ChangeCounts {
            changed: usize::from(lines.is_some()),
            failed: usize::from(lines.is_none()),
            ..Default::default()
        },
        // 通知は設計 §6.9 の 1 行だけ。MESSAGE は AI への案内なので応答にだけ入れる。
        notice: summary.clone(),
        summary,
        rows: vec![ChangeRow {
            item: "スタック用スクリプト".into(),
            before,
            after,
            result,
        }],
    });
}
