//! Typed, worker-only mIV Sound editing. CaptureQueue owns serialization and
//! persistence; host GUI owns the lock and its single undo generation.
use std::io::{Read, Write};
use std::path::Path;
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant, SystemTime};

use base64::Engine;
use serde::Serialize;
use serde_json::{Value, json};

use super::sound_document::{self, ApplyInput, GetContext, GetInput, Pipeline, SelectInput};
use crate::agent_ipc::changes::{
    ChangeController, ChangeCounts, ChangeRecord, ChangeRecordId, ChangeRow, ChangeRowOutcome,
    ChangeRowResult,
};
use crate::video::dsp::DspBridge;
use crate::video::dsp::bridge::{ConcurrentStateError, SoundCommand, SoundReply};

pub(crate) const RESPONSE_DEADLINE: Duration = Duration::from_secs(20);
const PRESET_LIMIT: u64 = 8 * 1024 * 1024;

#[derive(Clone, Debug)]
pub(crate) enum SoundRequest {
    Lock,
    Unlock {
        lock: String,
    },
    Get(GetInput),
    Apply {
        lock: String,
        revision: String,
        input: ApplyInput,
    },
    Select {
        lock: String,
        revision: String,
        input: SelectInput,
    },
    Undo {
        lock: String,
    },
    SavePreset {
        lock: String,
        pipeline: Pipeline,
        name: String,
        replace: bool,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub(crate) enum SoundOutcome {
    Locked {
        lock: String,
        revision: String,
    },
    Released {
        already_released: bool,
    },
    State {
        state: Value,
    },
    Accepted {
        revision: String,
        counts: [usize; 2],
        can_undo: bool,
        warnings: Vec<sound_document::ParameterWarning>,
        warning_count: usize,
        note: String,
    },
    Unchanged {
        revision: String,
    },
    Undone {
        revision: String,
    },
    PresetSaved {
        name: String,
        replaced: bool,
        revision: String,
        note: String,
    },
    Pending {
        message: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) enum SoundError {
    Busy,
    Unlocked,
    Changed,
    NothingToUndo,
    Aborted,
    Invalid(String),
    Unavailable(String),
}

impl std::fmt::Display for SoundError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Busy => f.write_str("ほかの操作でロック中です"),
            Self::Unlocked => f.write_str("利用者がロックを解除しました"),
            Self::Changed => f.write_str("状態が変わりました。get で確かめてください"),
            Self::NothingToUndo => f.write_str("戻せる変更がありません。窓で直してください"),
            Self::Aborted => f.write_str("音響調整との接続が切れ、処理を中断しました"),
            Self::Invalid(reason) | Self::Unavailable(reason) => f.write_str(reason),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub(crate) struct SoundResponse {
    pub outcome: SoundOutcome,
    pub persistence_warning: Option<String>,
}

/// The receiver may be dropped after the deadline. The queue still owns the
/// accepted job, persistence and notices, including results arriving late.
#[derive(Debug)]
pub(crate) struct SoundTicket {
    deadline: Instant,
    rx: mpsc::Receiver<Result<SoundResponse, SoundError>>,
}

impl SoundTicket {
    #[cfg(test)]
    pub(crate) fn completed_for_test(result: Result<SoundResponse, SoundError>) -> Self {
        let (tx, rx) = mpsc::channel();
        tx.send(result).unwrap();
        Self {
            deadline: Instant::now() + RESPONSE_DEADLINE,
            rx,
        }
    }

    /// Admission may follow time spent in the agent queue. Keep the common
    /// tool deadline without cancelling the accepted job or its late result.
    pub(crate) fn with_wait_budget(mut self, remaining: Duration) -> Self {
        self.deadline = self.deadline.min(Instant::now() + remaining);
        self
    }

    pub(crate) fn wait(self) -> Result<SoundResponse, SoundError> {
        match self
            .rx
            .recv_timeout(self.deadline.saturating_duration_since(Instant::now()))
        {
            Ok(result) => result,
            Err(mpsc::RecvTimeoutError::Timeout) => Ok(SoundResponse {
                outcome: SoundOutcome::Pending {
                    message: "まだ終わっていません。少し後に get で確かめてください".into(),
                },
                persistence_warning: None,
            }),
            Err(mpsc::RecvTimeoutError::Disconnected) => Err(SoundError::Aborted),
        }
    }
}

pub(super) struct SoundJob {
    pub request: SoundRequest,
    pub client: String,
    pub changes: Arc<ChangeController>,
    pub user_latency_secs: f64,
    reply: mpsc::Sender<Result<SoundResponse, SoundError>>,
}

pub(super) fn job(
    request: SoundRequest,
    client: &str,
    changes: Arc<ChangeController>,
    user_latency_secs: f64,
) -> (SoundJob, SoundTicket) {
    let (reply, rx) = mpsc::channel();
    (
        SoundJob {
            request,
            client: client.into(),
            changes,
            user_latency_secs,
            reply,
        },
        SoundTicket {
            rx,
            deadline: Instant::now() + RESPONSE_DEADLINE,
        },
    )
}

pub(super) fn reject(job: SoundJob, error: SoundError) {
    crate::logger::log(format!("[sound_chain] admission/load failed: {error}"));
    publish_change(
        &job.changes,
        &job.client,
        format!("mIV Sound: {error}"),
        Vec::new(),
    );
    let _ = job.reply.send(Err(error));
}

trait SoundHost {
    fn call(&self, command: SoundCommand) -> Result<SoundReply, SoundError>;
    fn latency_secs(&self) -> f64;
}

impl SoundHost for DspBridge {
    fn call(&self, command: SoundCommand) -> Result<SoundReply, SoundError> {
        self.sound_request(command)
            .map_err(bridge_error)?
            .recv()
            .map_err(|_| SoundError::Aborted)?
            .map_err(bridge_error)
    }
    fn latency_secs(&self) -> f64 {
        self.total_latency_samples() as f64 / self.sample_rate().max(1) as f64
    }
}

fn bridge_error(error: ConcurrentStateError) -> SoundError {
    match error {
        ConcurrentStateError::HostResponse(reason) => SoundError::Invalid(reason),
        ConcurrentStateError::HostExited | ConcurrentStateError::Interrupted(_) => {
            SoundError::Aborted
        }
    }
}

fn accepted(reply: SoundReply, statuses: &[&str]) -> Result<SoundReply, SoundError> {
    if statuses.contains(&reply.status.as_str()) {
        return Ok(reply);
    }
    Err(match reply.status.as_str() {
        "Busy" => SoundError::Busy,
        "Unlocked" => SoundError::Unlocked,
        "Changed" => SoundError::Changed,
        "NothingToUndo" => SoundError::NothingToUndo,
        _ => SoundError::Invalid("mIV Sound が要求を受け付けませんでした".into()),
    })
}

fn bytes(reply: &SoundReply) -> Result<Vec<u8>, SoundError> {
    base64::engine::general_purpose::STANDARD
        .decode(
            reply
                .state
                .as_deref()
                .ok_or_else(|| SoundError::Invalid("状態がありません".into()))?,
        )
        .map_err(|_| SoundError::Invalid("状態の形式が不正です".into()))
}

fn revision(reply: &SoundReply) -> Result<String, SoundError> {
    reply.revision().ok_or(SoundError::Unlocked)
}

fn check_revision(reply: &SoundReply, lock: &str, expected: &str) -> Result<(), SoundError> {
    if reply.lock.as_deref() != Some(lock) {
        return Err(SoundError::Unlocked);
    }
    if reply.revision().as_deref() != Some(expected) {
        return Err(SoundError::Changed);
    }
    Ok(())
}

fn budget<T: Serialize>(value: &T) -> Result<(), SoundError> {
    if mimageviewer_agent_ipc::mcp_content_chars(value)
        .map_err(|e| SoundError::Invalid(e.to_string()))?
        > mimageviewer_agent_ipc::MAX_PAGE_CHARS
    {
        return Err(SoundError::Invalid("応答が大きすぎます".into()));
    }
    Ok(())
}

fn read_optional(path: &Path, limit: u64) -> Result<Option<Vec<u8>>, SoundError> {
    let file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(SoundError::Invalid(format!("ファイルを読めません: {e}"))),
    };
    let mut data = Vec::new();
    file.take(limit + 1)
        .read_to_end(&mut data)
        .map_err(|e| SoundError::Invalid(format!("ファイルを読めません: {e}")))?;
    if data.len() as u64 > limit {
        return Err(SoundError::Invalid("ファイルが大きすぎます".into()));
    }
    Ok(Some(data))
}

fn check_startup(directory: &Path) -> Result<(), SoundError> {
    let config = read_optional(&directory.join("config.json"), PRESET_LIMIT)?;
    sound_document::check_pipeline_startup(config.as_deref()).map_err(SoundError::Invalid)
}

fn write_preset_temp(directory: &Path, data: &[u8]) -> Result<(), SoundError> {
    std::fs::create_dir_all(directory).map_err(|e| SoundError::Invalid(e.to_string()))?;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(directory.join("effetune_presets.json.miv-mcp.tmp"))
        .map_err(|e| SoundError::Invalid(e.to_string()))?;
    file.write_all(data)
        .and_then(|_| file.flush())
        .map_err(|e| SoundError::Invalid(format!("プリセットを書けません: {e}")))?;
    drop(file); // Required before sending sound_preset_commit, on Windows too.
    Ok(())
}

fn state_types(state: &[u8]) -> Value {
    let document: Value = serde_json::from_slice(state).unwrap_or(Value::Null);
    let types = |key: &str| {
        document[key]
            .as_array()
            .map(|rows| {
                rows.iter()
                    .map(|row| row["type"].as_str().unwrap_or("unknown").to_owned())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default()
    };
    json!({"A":types("pipelineA"),"B":types("pipelineB")})
}

struct Execution {
    outcome: SoundOutcome,
    notice: Option<String>,
    rows: Vec<ChangeRow>,
}

fn row(item: &str, before: impl Into<String>, after: impl Into<String>) -> ChangeRow {
    ChangeRow {
        item: item.into(),
        before: before.into(),
        after: after.into(),
        result: ChangeRowResult::Done(ChangeRowOutcome::Changed),
    }
}

fn execute(
    host: &impl SoundHost,
    request: &SoundRequest,
    plugin_dir: &Path,
    user_latency_secs: f64,
) -> Result<Execution, SoundError> {
    let mut rows = Vec::new();
    let (outcome, notice) = match request {
        SoundRequest::Lock => {
            let r = accepted(host.call(SoundCommand::Lock)?, &["Locked"])?;
            rows.push(row("mIV Sound のロック", "解除", "開始"));
            (
                SoundOutcome::Locked {
                    lock: r.lock.clone().ok_or(SoundError::Unlocked)?,
                    revision: revision(&r)?,
                },
                Some("AI アプリが mIV Sound の編集を始めました".into()),
            )
        }
        SoundRequest::Unlock { lock } => {
            let r = accepted(
                host.call(SoundCommand::Unlock { lock: lock.clone() })?,
                &["Released", "AlreadyReleased"],
            )?;
            let already_released = r.status == "AlreadyReleased";
            if !already_released {
                rows.push(row("mIV Sound のロック", "開始", "解除 (AI)"));
            }
            (
                SoundOutcome::Released { already_released },
                (!already_released).then(|| "AI アプリが mIV Sound のロックを解除しました".into()),
            )
        }
        SoundRequest::Get(input) => {
            let r = accepted(host.call(SoundCommand::Get)?, &["State"])?;
            let data = sound_document::get(
                &bytes(&r)?,
                input,
                &GetContext {
                    revision: r.revision(),
                    latency_secs: host.latency_secs(),
                    latency_remaining_secs: (super::composition::MAX_PLUGIN_LATENCY_SECS
                        - user_latency_secs)
                        .max(0.0),
                },
            )
            .map_err(SoundError::Invalid)?;
            (SoundOutcome::State { state: data }, None)
        }
        SoundRequest::Apply {
            lock,
            revision: expected,
            ..
        }
        | SoundRequest::Select {
            lock,
            revision: expected,
            ..
        } => {
            check_startup(plugin_dir)?;
            let r = accepted(host.call(SoundCommand::Get)?, &["State"])?;
            check_revision(&r, lock, expected)?;
            let current = bytes(&r)?;
            let prepared = match request {
                SoundRequest::Apply { input, .. } => sound_document::apply(&current, input),
                SoundRequest::Select { input, .. } => sound_document::select(&current, input),
                _ => unreachable!(),
            }
            .map_err(SoundError::Invalid)?;
            if prepared.unchanged {
                return Ok(Execution {
                    outcome: SoundOutcome::Unchanged {
                        revision: revision(&r)?,
                    },
                    notice: None,
                    rows,
                });
            }
            let next = r
                .generation
                .checked_add(1)
                .ok_or_else(|| SoundError::Invalid("世代の上限です".into()))?;
            let outcome = SoundOutcome::Accepted { revision: format!("{lock}:{next}"),
                counts: prepared.after_counts, can_undo: true,
                warnings: prepared.warnings.clone(),
                warning_count: prepared.warning_count,
                note: "受け付けました。数秒で mIV Sound が読み直します。使う側の残響などは一度切れます".into() };
            budget(&SoundResponse {
                outcome: outcome.clone(),
                persistence_warning: Some("状態を保存できませんでした".into()),
            })?;
            let replaced = accepted(
                host.call(SoundCommand::Replace {
                    lock: lock.clone(),
                    generation: r.generation,
                    state: base64::engine::general_purpose::STANDARD.encode(&prepared.bytes),
                })?,
                &["Replaced"],
            )?;
            // Record only counts, selection and bypass, never parameter values.
            let before: Value =
                serde_json::from_slice(&current).map_err(|e| SoundError::Invalid(e.to_string()))?;
            let after: Value = serde_json::from_slice(&prepared.bytes)
                .map_err(|e| SoundError::Invalid(e.to_string()))?;
            for (index, label) in ["A", "B"].into_iter().enumerate() {
                if prepared.before_counts[index] != prepared.after_counts[index]
                    || before[format!("pipeline{label}")] != after[format!("pipeline{label}")]
                {
                    rows.push(row(
                        &format!("mIV Sound の {label}"),
                        format!("{} 個", prepared.before_counts[index]),
                        format!("{} 個", prepared.after_counts[index]),
                    ));
                }
            }
            if before["currentPipeline"] != after["currentPipeline"] {
                rows.push(row(
                    "mIV Sound の使う側",
                    before["currentPipeline"].as_str().unwrap_or("A"),
                    after["currentPipeline"].as_str().unwrap_or("A"),
                ));
            }
            if before["masterBypass"] != after["masterBypass"] {
                rows.push(row(
                    "mIV Sound",
                    if before["masterBypass"] == true {
                        "オフ"
                    } else {
                        "オン"
                    },
                    if after["masterBypass"] == true {
                        "オフ"
                    } else {
                        "オン"
                    },
                ));
            }
            crate::logger::log(format!(
                "[sound_chain] replace before_current={} after_current={} before_counts={:?} after_counts={:?} before_types={} after_types={}",
                before["currentPipeline"].as_str().unwrap_or("unknown"),
                after["currentPipeline"].as_str().unwrap_or("unknown"),
                prepared.before_counts,
                prepared.after_counts,
                state_types(&current),
                state_types(&prepared.bytes)
            ));
            let notice = match request {
                SoundRequest::Apply { input, .. } => format!(
                    "AI アプリが mIV Sound の {:?} を組み直しました ({} 個のエフェクト)",
                    input.pipeline,
                    input.effects.len()
                ),
                _ => format!(
                    "AI アプリが mIV Sound の使う側を {}、全体を{}にしました",
                    after["currentPipeline"].as_str().unwrap_or("A"),
                    if after["masterBypass"] == true {
                        "オフ"
                    } else {
                        "オン"
                    }
                ),
            };
            let mut outcome = outcome;
            if let SoundOutcome::Accepted {
                revision: result_revision,
                ..
            } = &mut outcome
            {
                *result_revision = revision(&replaced)?;
            }
            (outcome, Some(notice))
        }
        SoundRequest::Undo { lock } => {
            check_startup(plugin_dir)?;
            let r = accepted(
                host.call(SoundCommand::Undo { lock: lock.clone() })?,
                &["Undone"],
            )?;
            rows.push(row("mIV Sound", "AI による変更", "取り消し"));
            (
                SoundOutcome::Undone {
                    revision: revision(&r)?,
                },
                Some("AI アプリが mIV Sound の変更を取り消しました".into()),
            )
        }
        SoundRequest::SavePreset {
            lock,
            pipeline,
            name,
            replace,
        } => {
            check_startup(plugin_dir)?;
            let r = accepted(host.call(SoundCommand::Get)?, &["State"])?;
            if r.lock.as_deref() != Some(lock) {
                return Err(SoundError::Unlocked);
            }
            let guard = r.guard.clone().ok_or(SoundError::Changed)?;
            let existing = read_optional(&plugin_dir.join("effetune_presets.json"), PRESET_LIMIT)?;
            let timestamp = preset_timestamp();
            let preset = sound_document::prepare_preset(
                &bytes(&r)?,
                existing.as_deref(),
                *pipeline,
                name,
                *replace,
                &timestamp,
            )
            .map_err(SoundError::Invalid)?;
            let mut outcome = SoundOutcome::PresetSaved {
                name: preset.name.clone(),
                replaced: preset.replaced,
                revision: revision(&r)?,
                note: "置き換えた古いプリセットは戻せません".into(),
            };
            let mut maximum_response = outcome.clone();
            if let SoundOutcome::PresetSaved { note, .. } = &mut maximum_response {
                note.push_str("。プリセットは保存しましたが、ファイルの保護を再開できませんでした");
            }
            budget(&SoundResponse {
                outcome: maximum_response,
                persistence_warning: Some("状態を保存できませんでした".into()),
            })?;
            write_preset_temp(plugin_dir, &preset.bytes)?;
            let committed = accepted(
                host.call(SoundCommand::PresetCommit {
                    lock: lock.clone(),
                    generation: r.generation,
                    guard,
                })?,
                &["Committed"],
            )?;
            rows.push(row(
                "mIV Sound のプリセット",
                if preset.replaced { "既存" } else { "なし" },
                preset.name.clone(),
            ));
            crate::logger::log(format!(
                "[sound_chain] save_preset name={} pipeline={pipeline:?} replaced={}",
                preset.name, preset.replaced
            ));
            let mut notice = format!(
                "AI アプリがプリセット『{}』を保存しました{}",
                preset.name,
                if preset.replaced {
                    " (置き換えました。古い内容は戻せません)"
                } else {
                    ""
                }
            );
            if committed.guard.is_none() {
                let warning = "プリセットは保存しましたが、ファイルの保護を再開できませんでした";
                crate::logger::log("[sound_chain] preset committed; host guard could not reopen");
                notice.push_str(&format!("。{warning}"));
                if let SoundOutcome::PresetSaved { note, .. } = &mut outcome {
                    note.push_str(&format!("。{warning}"));
                }
            }
            (outcome, Some(notice))
        }
    };
    Ok(Execution {
        outcome,
        notice,
        rows,
    })
}

pub(super) fn run(
    job: SoundJob,
    bridge: &DspBridge,
    plugin_dir: &Path,
    persist: impl FnOnce(&[u8]) -> Result<(), super::CaptureError>,
) {
    run_with_host(job, bridge, plugin_dir, persist);
}

fn run_with_host(
    job: SoundJob,
    bridge: &impl SoundHost,
    plugin_dir: &Path,
    persist: impl FnOnce(&[u8]) -> Result<(), super::CaptureError>,
) {
    let started = Instant::now();
    let name = match &job.request {
        SoundRequest::Lock => "lock",
        SoundRequest::Unlock { .. } => "unlock",
        SoundRequest::Get(_) => "get",
        SoundRequest::Apply { .. } => "apply",
        SoundRequest::Select { .. } => "select",
        SoundRequest::Undo { .. } => "undo",
        SoundRequest::SavePreset { .. } => "save_preset",
    };
    let result =
        execute(bridge, &job.request, plugin_dir, job.user_latency_secs).and_then(|execution| {
            // Fetch once after success, on this same queue; no second writer or
            // nested queue wait. A failed save cannot change an accepted outcome.
            let saved = bridge
                .call(SoundCommand::Get)
                .and_then(|r| accepted(r, &["State"]))
                .and_then(|r| bytes(&r))
                .map_err(|e| e.to_string())
                .and_then(|data| persist(&data).map_err(|e| e.to_string()));
            let warning = saved.err().map(|error| {
                crate::logger::log(format!("[sound_chain] state persistence: {error}"));
                "状態を保存できませんでした".to_owned()
            });
            if let Some(mut notice) = execution.notice {
                if let Some(warning) = &warning {
                    notice.push_str(&format!("。{warning}"));
                }
                publish_change(&job.changes, &job.client, notice, execution.rows);
            } else if let Some(warning) = &warning {
                publish_change(&job.changes, &job.client, warning.clone(), Vec::new());
            }
            let response = SoundResponse {
                outcome: execution.outcome,
                persistence_warning: warning,
            };
            budget(&response)?;
            Ok(response)
        });
    crate::logger::log(format!(
        "[sound_chain] action={name} result={} elapsed_ms={}",
        if result.is_ok() {
            "completed"
        } else {
            "failed"
        },
        started.elapsed().as_millis()
    ));
    if let Err(error) = &result {
        crate::logger::log(format!("[sound_chain] action={name} failed: {error}"));
        // Also surface failures that arrive after the caller's deadline.
        publish_change(
            &job.changes,
            &job.client,
            format!("mIV Sound: {error}"),
            Vec::new(),
        );
    }
    if crate::perf::is_enabled() {
        crate::perf::event(
            "sound",
            "request",
            None,
            0,
            &[
                ("action", json!(name)),
                ("elapsed_ms", json!(started.elapsed().as_millis() as u64)),
                ("success", json!(result.is_ok())),
            ],
        );
    }
    let _ = job.reply.send(result);
}

pub(super) fn publish_change(
    changes: &ChangeController,
    client: &str,
    notice: String,
    rows: Vec<ChangeRow>,
) {
    changes.publish(ChangeRecord {
        id: ChangeRecordId::default(),
        at: SystemTime::now(),
        client_name: client.into(),
        tool: "sound_chain".into(),
        collection: None,
        counts: ChangeCounts {
            changed: rows.len(),
            ..Default::default()
        },
        summary: notice.clone(),
        rows,
        notice,
    });
}

pub(super) fn user_released(changes: &ChangeController) {
    crate::logger::log("[sound_chain] lock released by user");
    publish_change(
        changes,
        "利用者",
        "mIV Sound のロックを解除しました (利用者)".into(),
        vec![row("mIV Sound のロック", "開始", "解除 (利用者)")],
    );
}

fn preset_timestamp() -> String {
    let now = unsafe { windows::Win32::System::SystemInformation::GetSystemTime() };
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{:03}Z",
        now.wYear, now.wMonth, now.wDay, now.wHour, now.wMinute, now.wSecond, now.wMilliseconds
    )
}

#[cfg(test)]
#[path = "sound_service_tests.rs"]
mod tests;
