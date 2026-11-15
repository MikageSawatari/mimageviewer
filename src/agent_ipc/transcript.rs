//! 保存済み文字起こしのworker専用read。行snapshotは要求内だけで保持する。
//! 保存値・start_secs・返す区間は、すべてmIVの再生位置と同じメディア時刻。
use super::*;
use crate::transcript_db::{FileSignature, TranscriptDb, TranscriptStatus};

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct TranscriptCursor {
    version: u32,
    tool: String,
    permission_epoch: u64,
    args_digest: String,
    rev: i64,
    next: usize,
    start_secs: f64,
}

fn bad_cursor() -> AgentError {
    error(
        ErrorCode::BadRequest,
        "文字起こしが変わったか続きが無効です。最初から取り直してください",
    )
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn unhex(value: &str) -> Result<Vec<u8>, AgentError> {
    if value.len() > 8192 || !value.len().is_multiple_of(2) || !value.is_ascii() {
        return Err(bad_cursor());
    }
    (0..value.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&value[index..index + 2], 16).map_err(|_| bad_cursor()))
        .collect()
}

fn encode(data: TranscriptCursor) -> Result<String, AgentError> {
    // cursorはAIアプリが書き換えられる値なので、認可にも対象の決定にも使わない (FolderCursorと同じ)。
    // - 許可: 要求ごとにadmission snapshotの `transcripts` を見る (handler冒頭と受付)。送信直前はepochを再照合する。
    // - 対象: 読む行は常に要求の `item` から決める。cursorの引数digestが要求の対象・開始秒と一致しなければ拒否するので、
    //   別の項目の続きとしては使えない。
    // - 位置: 書き換えても、許可された同じ行の別の区間を読むだけになる。rev・範囲外は取り直しを求める。
    // このため署名・プロセス固有の鍵・cursor台帳は持たない (独立レビュー区切り4 P3、状態と依存を減らすため)。
    Ok(hex(&serde_json::to_vec(&data).map_err(|_| internal())?))
}

fn decode(
    value: &str,
    key: &str,
    epoch: u64,
    start: Option<f64>,
) -> Result<TranscriptCursor, AgentError> {
    let data: TranscriptCursor =
        serde_json::from_slice(&unhex(value)?).map_err(|_| bad_cursor())?;
    if data.version != 1
        || data.tool != "get_transcript"
        || data.rev <= 0
        || data.next == 0
        || !valid_start(data.start_secs)
        || start.is_some_and(|start| start != data.start_secs)
        || data.args_digest != wire_digest(&(key, data.start_secs)).map_err(|_| internal())?
    {
        return Err(bad_cursor());
    }
    if data.permission_epoch != epoch {
        return Err(error(
            ErrorCode::PermissionChanged,
            "許可が変わりました。最初からやり直してください",
        ));
    }
    Ok(data)
}

fn valid_start(value: f64) -> bool {
    value.is_finite() && value >= 0.0
}

fn fits(value: &(impl Serialize + ?Sized), budget: usize) -> Result<bool, AgentError> {
    mcp_content_chars(value)
        .map(|count| count <= budget)
        .map_err(|_| internal())
}

/// 巨大な1区間だけは本文を文字境界で切って印を付ける。時刻と次の区間位置は変えない。
fn shrink_segment(segment: &mut TranscriptSegmentInfo) -> Result<(), AgentError> {
    if fits(segment, MAX_ITEM_CHARS)? {
        return Ok(());
    }
    let original = std::mem::take(&mut segment.text.text);
    let boundaries: Vec<_> = original
        .char_indices()
        .map(|(index, _)| index)
        .take(MAX_ITEM_CHARS + 1)
        .collect();
    segment.text.truncated = true;
    let (mut low, mut high) = (0, boundaries.len());
    while low + 1 < high {
        let mid = (low + high) / 2;
        segment.text.text = original[..boundaries[mid]].to_owned();
        if fits(segment, MAX_ITEM_CHARS)? {
            low = mid;
        } else {
            high = mid;
        }
    }
    segment.text.text = original[..boundaries[low]].to_owned();
    Ok(())
}

impl Handlers {
    pub(super) fn get_transcript(
        &self,
        item: ItemRef,
        start_secs: Option<f64>,
        cursor: Option<String>,
        ctx: &ReadContext,
    ) -> Result<TranscriptReadResult, AgentError> {
        // 受付だけに依存しない。DB/statを行う前に同じadmission snapshotで拒否する。
        if !ctx.granted.0.transcripts {
            return Err(error(ErrorCode::Disabled, TRANSCRIPT_PERMISSION_MESSAGE));
        }
        if start_secs.is_some_and(|value| !valid_start(value)) {
            return Err(error(
                ErrorCode::BadRequest,
                "開始時刻は0以上の有限な秒で指定してください",
            ));
        }
        let path = input_path::validate(&item.path)?;
        let key = TranscriptDb::normalize_path(path);
        let continuation = cursor
            .as_deref()
            .map(|cursor| decode(cursor, &key, ctx.granted.1, start_secs))
            .transpose()?;
        ctx.checkpoint()?;
        let metadata = std::fs::metadata(path).ok();
        let kind = item_kind_from_extension(
            &path
                .extension()
                .and_then(|ext| ext.to_str())
                .unwrap_or("")
                .to_ascii_lowercase(),
        );
        if item.subresource.is_some()
            || metadata.as_ref().is_some_and(std::fs::Metadata::is_dir)
            || !matches!(kind, ItemKind::Video | ItemKind::Audio)
        {
            return if continuation.is_some() {
                Err(bad_cursor())
            } else {
                Ok(TranscriptReadResult::NotApplicable)
            };
        }
        if !metadata.as_ref().is_some_and(std::fs::Metadata::is_file) {
            return if continuation.is_some() {
                Err(bad_cursor())
            } else {
                Ok(TranscriptReadResult::None)
            };
        }
        let row = (|| {
            let Some(db) = TranscriptDb::open_read_only(TranscriptDb::db_path())? else {
                return Ok(None);
            };
            db.get(&key)
        })()
        .map_err(|err: rusqlite::Error| {
            log::error!("agent_ipc: transcript read failed: {err}");
            internal()
        })?;
        ctx.checkpoint()?;
        let Some(row) = row else {
            return if continuation.is_some() {
                Err(bad_cursor())
            } else {
                Ok(TranscriptReadResult::None)
            };
        };
        if continuation
            .as_ref()
            .is_some_and(|cursor| cursor.rev != row.rev || cursor.next >= row.segments.len())
        {
            return Err(bad_cursor());
        }
        if row.status == TranscriptStatus::Failed {
            return Ok(TranscriptReadResult::Failed);
        }
        let start = continuation
            .as_ref()
            .map_or(start_secs.unwrap_or(0.0), |cursor| cursor.start_secs);
        let start = if start == 0.0 { 0.0 } else { start };
        let args_digest = wire_digest(&(key.as_str(), start)).map_err(|_| internal())?;
        let first = continuation.as_ref().map_or_else(
            || {
                row.segments
                    .partition_point(|segment| segment.1 as f64 <= start * 1000.0)
            },
            |cursor| cursor.next,
        );
        let info = item_info::transcript_info(
            &row,
            metadata.as_ref().and_then(FileSignature::from_metadata),
        );
        let mut page = TranscriptPage {
            info,
            segments: Vec::new(),
            cursor: None,
            truncated: false,
        };
        if !fits(&TranscriptReadResult::Present(page.clone()), MAX_PAGE_CHARS)? {
            log::error!("agent_ipc: transcript metadata exceeds response budget");
            return Err(internal());
        }
        for index in first..row.segments.len() {
            ctx.checkpoint()?;
            let segment = &row.segments[index];
            let mut value = TranscriptSegmentInfo {
                start_secs: segment.0 as f64 / 1000.0,
                end_secs: segment.1 as f64 / 1000.0,
                text: TextValue {
                    text: segment.2.clone(),
                    truncated: false,
                },
            };
            shrink_segment(&mut value)?;
            page.segments.push(value);
            page.cursor = (index + 1 < row.segments.len())
                .then(|| {
                    encode(TranscriptCursor {
                        version: 1,
                        tool: "get_transcript".into(),
                        permission_epoch: ctx.granted.1,
                        args_digest: args_digest.clone(),
                        rev: row.rev,
                        next: index + 1,
                        start_secs: start,
                    })
                })
                .transpose()?;
            page.truncated =
                page.cursor.is_some() || page.segments.iter().any(|segment| segment.text.truncated);
            if !fits(&TranscriptReadResult::Present(page.clone()), MAX_PAGE_CHARS)? {
                page.segments.pop();
                if page.segments.is_empty() {
                    log::error!(
                        "agent_ipc: transcript metadata and segment exceed response budget"
                    );
                    return Err(internal());
                }
                page.cursor = Some(encode(TranscriptCursor {
                    version: 1,
                    tool: "get_transcript".into(),
                    permission_epoch: ctx.granted.1,
                    args_digest: args_digest.clone(),
                    rev: row.rev,
                    next: index,
                    start_secs: start,
                })?);
                page.truncated = true;
                break;
            }
            ctx.publish(Payload::Transcript(TranscriptReadResult::Present(
                page.clone(),
            )));
            if ctx.expired() {
                break;
            }
        }
        ctx.checkpoint()?;
        Ok(TranscriptReadResult::Present(page))
    }
}

#[cfg(test)]
#[path = "transcript_tests.rs"]
mod tests;
