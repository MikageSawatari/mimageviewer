//! UI admission is separate from worker-only reads and FIFO writes.
use super::changes::{
    ChangeController, ChangeCounts, ChangeRecord, ChangeRecordId, ChangeRow, ChangeRowOutcome,
    ChangeRowResult,
};
use crate::filename_stack_script::{self, ScriptDocument};
use mimageviewer_agent_ipc::{
    AgentError, ErrorCode, MAX_PAGE_CHARS, Payload, StackScriptReadResult, StackScriptResult,
    StackScriptWriteResult, mcp_content_chars,
};

const MESSAGE: &str = "書く前に読み、戻せるようにその本文を持っておいてください。スタック表示を開き直すか、フォルダを読み直すと新しいスクリプトで分けます。";

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
    if source.is_some_and(|source| source.len() as u64 > filename_stack_script::MAX_SCRIPT_BYTES) {
        return Err(bad("スタック用スクリプトは 1 MiB 以下にしてください"));
    }
    Ok(())
}

fn check_budget(value: &impl serde::Serialize) -> Result<(), AgentError> {
    if mcp_content_chars(value).map_err(|_| super::internal())? > MAX_PAGE_CHARS {
        return Err(bad("スクリプトの応答が長すぎます"));
    }
    Ok(())
}

/// Split on Unicode scalar boundaries using the actual structured + text-copy
/// size. Maximal part-number widths bound the final part/parts metadata.
fn parts(document: &ScriptDocument, enabled: bool) -> Result<Vec<String>, AgentError> {
    let bounds = document
        .source
        .char_indices()
        .map(|(index, _)| index)
        .chain(std::iter::once(document.source.len()))
        .collect::<Vec<_>>();
    let mut result = Vec::new();
    let mut start = 0;
    while start + 1 < bounds.len() {
        // Bound each binary search independently of the total document length.
        let mut low = start;
        let mut high = (start + 8000).min(bounds.len() - 1);
        while low < high {
            let end = low + (high - low).div_ceil(2);
            let candidate = StackScriptReadResult {
                source: document.source[bounds[start]..bounds[end]].into(),
                sha256: document.sha256.clone(),
                builtin: document.builtin,
                enabled,
                part: u32::MAX,
                parts: u32::MAX,
                message: MESSAGE.into(),
            };
            if check_budget(&candidate).is_ok() {
                low = end;
            } else {
                high = end - 1;
            }
        }
        if low == start {
            return Err(bad("スクリプトの応答が長すぎます"));
        }
        result.push(document.source[bounds[start]..bounds[low]].into());
        start = low;
    }
    if result.is_empty() {
        result.push(String::new());
    }
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
    let parts = parts(document, enabled)?;
    let part = part.unwrap_or(1);
    let source = parts
        .get(part as usize - 1)
        .ok_or_else(|| bad("指定された part はありません"))?
        .clone();
    let result = StackScriptResult::Read(StackScriptReadResult {
        source,
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
        notice: if lines.is_some() {
            format!("{summary}。{MESSAGE}")
        } else {
            summary.clone()
        },
        summary,
        rows: vec![ChangeRow {
            item: "スタック用スクリプト".into(),
            before,
            after,
            result,
        }],
    });
}
