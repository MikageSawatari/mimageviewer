//! 文字起こしジョブの実行役。受付・待ち行列・終了判断はAppだけが所有する。
//! ファイル/DB/FFmpegとworker pipeの待ちはすべて背景threadで行う。

use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use super::asr_worker::{self, AsrEvent, AsrInput, AsrOutput, AsrWorkerClient, KillHandle};
use super::{transcript_audio, transcript_postprocess};
use crate::transcript_db::{
    FileSignature, SaveOutcome, SaveRequest, TranscriptDb, TranscriptResult, TranscriptRow,
    TranscriptSegment, TranscriptStatus,
};

/// 復号と後処理を合わせた版。どちらの規則を変えたときも同じ番号を上げる。
pub const CONFIG_VERSION: i64 = asr_worker::SETTINGS_VERSION as i64;

pub struct TranscriptWorkerConfig {
    pub worker_executable: Option<PathBuf>,
    pub load: asr_worker::LoadRequest,
    pub model_id: String,
    pub transcript_db_path: PathBuf,
}

/// 次の再生openと同じ、受付時のメモリ上のトラック選択をパスと一緒に保持する。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TranscriptTarget {
    pub path: PathBuf,
    pub saved_audio_track: Option<crate::video::SavedAudioTrackChoice>,
}
impl From<PathBuf> for TranscriptTarget {
    fn from(path: PathBuf) -> Self {
        Self {
            path,
            saved_audio_track: None,
        }
    }
}
impl From<&str> for TranscriptTarget {
    fn from(path: &str) -> Self {
        PathBuf::from(path).into()
    }
}

#[derive(Debug)]
pub enum TranscriptWorkerCommand {
    Item(TranscriptTarget),
    Finish,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TranscriptItemOutcome {
    Created,
    AlreadyCreated,
    NoAudio,
    Failed,
    /// failed行のcommitも表示を失効させる。既存okを保持した失敗とは区別する。
    FailedWritten,
    Discarded,
    Cancelled,
}

pub enum TranscriptWorkerEvent {
    Started(KillHandle),
    Progress { processed_ms: u64, duration_ms: u64 },
    ItemDone(TranscriptItemOutcome),
    Finished(Result<(), String>),
}

/// 取消とkill handleの公開を同じ境界で扱う。取消前の公開も、公開前の取消も漏らさない。
struct Control {
    cancel: Arc<AtomicBool>,
    kill: Mutex<Option<KillHandle>>,
}

impl Control {
    fn publish(&self, handle: KillHandle) -> bool {
        let mut slot = self.kill.lock().unwrap();
        if self.cancel.load(Ordering::Acquire) {
            drop(slot);
            let _ = handle.kill();
            return false;
        }
        *slot = Some(handle);
        true
    }

    fn stop(&self) {
        self.cancel.store(true, Ordering::Release);
        let handle = self.kill.lock().unwrap().clone();
        if let Some(handle) = handle {
            let _ = handle.kill();
        }
    }
}

pub struct TranscriptWorker {
    pub commands: Sender<TranscriptWorkerCommand>,
    pub events: Receiver<TranscriptWorkerEvent>,
    control: Arc<Control>,
}

impl TranscriptWorker {
    pub fn stop(&self) {
        self.control.stop();
        let _ = self.commands.send(TranscriptWorkerCommand::Finish);
    }

    pub fn stop_for_exit(&self) {
        self.stop();
    }

    #[cfg(test)]
    pub(crate) fn test_channels(
        cancel: Arc<AtomicBool>,
    ) -> (
        Self,
        Receiver<TranscriptWorkerCommand>,
        Sender<TranscriptWorkerEvent>,
    ) {
        let (commands, rx) = mpsc::channel();
        let (tx, events) = mpsc::channel();
        (
            Self {
                commands,
                events,
                control: Arc::new(Control {
                    cancel,
                    kill: Mutex::new(None),
                }),
            },
            rx,
            tx,
        )
    }
}

impl Drop for TranscriptWorker {
    fn drop(&mut self) {
        self.stop_for_exit();
    }
}

pub fn spawn_transcript_worker(
    config: TranscriptWorkerConfig,
    cancel: Arc<AtomicBool>,
    repaint: egui::Context,
) -> io::Result<TranscriptWorker> {
    let (commands, rx) = mpsc::channel();
    let (tx, events) = mpsc::channel();
    let control = Arc::new(Control {
        cancel,
        kill: Mutex::new(None),
    });
    let worker_control = Arc::clone(&control);
    std::thread::Builder::new()
        .name("transcript-job".into())
        .spawn(move || {
            let result = run(&config, &worker_control, rx, &tx, &repaint);
            // 取消によるpipeの終了は正常な中止。commit済みのItemDoneは既に送ってある。
            let result = if worker_control.cancel.load(Ordering::Acquire) {
                Ok(())
            } else {
                result
            };
            if let Err(error) = &result {
                crate::logger::log(format!("transcript_job: {error}"));
            }
            send(&tx, &repaint, TranscriptWorkerEvent::Finished(result));
        })?;
    Ok(TranscriptWorker {
        commands,
        events,
        control,
    })
}

fn send(
    events: &Sender<TranscriptWorkerEvent>,
    repaint: &egui::Context,
    event: TranscriptWorkerEvent,
) -> bool {
    let sent = events.send(event).is_ok();
    repaint.request_repaint();
    sent
}

fn clock() -> Option<Instant> {
    crate::perf::is_enabled().then(Instant::now)
}

fn stage(kind: &str, started: Option<Instant>, id: u64, extras: &[(&str, serde_json::Value)]) {
    let Some(started) = started else { return };
    let mut values = vec![
        (
            "elapsed_ms",
            serde_json::json!(started.elapsed().as_secs_f64() * 1000.0),
        ),
        ("request_id", serde_json::json!(id)),
    ];
    values.extend_from_slice(extras);
    crate::perf::event("transcript", kind, None, 0, &values);
}

fn run(
    config: &TranscriptWorkerConfig,
    control: &Control,
    commands: Receiver<TranscriptWorkerCommand>,
    events: &Sender<TranscriptWorkerEvent>,
    repaint: &egui::Context,
) -> Result<(), String> {
    if control.cancel.load(Ordering::Acquire) {
        return Ok(());
    }
    let executable = config
        .worker_executable
        .clone()
        .map(Ok)
        .unwrap_or_else(asr_worker::worker_executable)
        .map_err(|e| e.to_string())?;
    if control.cancel.load(Ordering::Acquire) {
        return Ok(());
    }
    let mut client = AsrWorkerClient::start_at(&executable).map_err(|e| e.to_string())?;
    let kill = client.kill_handle();
    if !control.publish(kill.clone()) {
        return Ok(());
    }
    if !send(events, repaint, TranscriptWorkerEvent::Started(kill)) {
        return Ok(());
    }
    client.hello().map_err(|e| e.to_string())?;
    if control.cancel.load(Ordering::Acquire) {
        return Ok(());
    }
    let started = clock();
    let loaded = client.load(&config.load);
    stage(
        "model_load",
        started,
        0,
        &[("success", serde_json::json!(loaded.is_ok()))],
    );
    loaded.map_err(|e| e.to_string())?;
    if control.cancel.load(Ordering::Acquire) {
        return Ok(());
    }
    let (mut input, mut output) = client.split().map_err(|e| e.to_string())?;
    let db = TranscriptDb::open_at(&config.transcript_db_path).map_err(|e| e.to_string())?;
    let mut request_id = 0;
    while let Ok(command) = commands.recv() {
        match command {
            TranscriptWorkerCommand::Finish => break,
            TranscriptWorkerCommand::Item(target) => {
                request_id += 1;
                let outcome = if control.cancel.load(Ordering::Acquire) {
                    TranscriptItemOutcome::Cancelled
                } else {
                    process_item(
                        config,
                        &db,
                        &control.cancel,
                        &target,
                        request_id,
                        &mut input,
                        &mut output,
                        events,
                        repaint,
                    )?
                };
                // commit後の取消を見てこの通知を捨ててはいけない。
                if !send(events, repaint, TranscriptWorkerEvent::ItemDone(outcome)) {
                    break;
                }
            }
        }
    }
    // 両pipe endpointのDropがJob Objectを終える。UI threadへjoinは持ち込まない。
    Ok(())
}

fn reusable(row: &TranscriptRow, signature: FileSignature, stream: usize, model_id: &str) -> bool {
    row.status == TranscriptStatus::Ok
        && row.signature == signature
        && row.audio_stream == Some(stream)
        && row.model_id == model_id
        && row.config_version == CONFIG_VERSION
}

fn process_item(
    config: &TranscriptWorkerConfig,
    db: &TranscriptDb,
    cancel: &Arc<AtomicBool>,
    target: &TranscriptTarget,
    request_id: u64,
    input: &mut AsrInput,
    output: &mut AsrOutput,
    events: &Sender<TranscriptWorkerEvent>,
    repaint: &egui::Context,
) -> Result<TranscriptItemOutcome, String> {
    let path = &target.path;
    let signature = match std::fs::metadata(path)
        .as_ref()
        .ok()
        .and_then(FileSignature::from_metadata)
    {
        Some(signature) => signature,
        None => {
            // 読めない署名を捏造して保存しない。既存okもそのまま残す。
            crate::logger::log("transcript_job: unavailable input signature");
            return Ok(TranscriptItemOutcome::Failed);
        }
    };
    let prepared = transcript_audio::prepare_with_saved(
        path,
        target.saved_audio_track.as_ref(),
        Arc::clone(cancel),
    );
    if cancel.load(Ordering::Acquire) {
        return Ok(TranscriptItemOutcome::Cancelled);
    }
    let prepared = match prepared {
        Ok(Some(prepared)) => prepared,
        Ok(None) => return Ok(TranscriptItemOutcome::NoAudio),
        Err(error) => {
            crate::logger::log(format!("transcript_job: audio prepare: {error}"));
            return save(
                config,
                db,
                cancel,
                path,
                signature,
                request_id,
                TranscriptResult::Failed,
            );
        }
    };
    let existing = db.get(&path.to_string_lossy()).map_err(|e| e.to_string())?;
    if existing
        .as_ref()
        .is_some_and(|row| reusable(row, signature, prepared.stream_index, &config.model_id))
    {
        return Ok(TranscriptItemOutcome::AlreadyCreated);
    }
    let stream_index = prepared.stream_index;
    let duration_ms = (prepared.duration_secs.max(0.0) * 1000.0) as u64;
    send(
        events,
        repaint,
        TranscriptWorkerEvent::Progress {
            processed_ms: 0,
            duration_ms,
        },
    );
    let started = clock();
    let inferred = infer(
        input,
        output,
        cancel,
        request_id,
        duration_ms,
        events,
        repaint,
        move |sink| {
            prepared
                .extract(cancel, sink)
                .map(|_| ())
                .map_err(|e| e.to_string())
        },
    );
    stage(
        "infer_file",
        started,
        request_id,
        &[("success", serde_json::json!(inferred.is_ok()))],
    );
    if cancel.load(Ordering::Acquire) {
        return Ok(TranscriptItemOutcome::Cancelled);
    }
    // wire/process/GPU失敗は型付きclient失敗のままジョブを終える。
    let inferred = inferred?;
    if let Some(error) = inferred.extraction_error {
        crate::logger::log(format!("transcript_job: audio extraction: {error}"));
        return save(
            config,
            db,
            cancel,
            path,
            signature,
            request_id,
            TranscriptResult::Failed,
        );
    }
    let (segments, removed_counts) = transcript_postprocess::clean(&inferred.segments);
    save(
        config,
        db,
        cancel,
        path,
        signature,
        request_id,
        TranscriptResult::Ok {
            segments,
            removed_counts,
            duration_ms: inferred.duration_ms,
            audio_stream: stream_index,
        },
    )
}

struct Inferred {
    segments: Vec<TranscriptSegment>,
    duration_ms: u64,
    extraction_error: Option<String>,
}

struct WriterAbort {
    kill: KillHandle,
    ended: bool,
}
impl Drop for WriterAbort {
    fn drop(&mut self) {
        // end送信前の異常終了では受信側を待たせない。回復せずジョブを終える。
        if !self.ended {
            let _ = self.kill.kill();
        }
    }
}

/// 抽出の失敗時もendまで読む。その入力の窓だけ捨て、Loadedに戻った同じモデルを次へ使う。
/// 送信と受信のどちらが失敗してもkillで相手のpipe待ちを解いてから、背景でjoinする。
fn infer(
    input: &mut AsrInput,
    output: &mut AsrOutput,
    cancel: &Arc<AtomicBool>,
    request_id: u64,
    duration_ms: u64,
    events: &Sender<TranscriptWorkerEvent>,
    repaint: &egui::Context,
    extract: impl FnOnce(
        &mut dyn FnMut(&[u8]) -> Result<(), transcript_audio::AudioError>,
    ) -> Result<(), String>
    + Send,
) -> Result<Inferred, String> {
    input.begin(request_id).map_err(|e| e.to_string())?;
    let kill = input.kill_handle();
    let result = std::thread::scope(|scope| {
        let writer_kill = kill.clone();
        let writer = scope.spawn(move || {
            let mut abort = WriterAbort {
                kill: writer_kill,
                ended: false,
            };
            let extracted = extract(&mut |pcm| {
                input
                    .write_pcm(pcm)
                    .map_err(|e| transcript_audio::AudioError::Sink(e.to_string()))
            });
            let ended = input.end().map_err(|e| e.to_string());
            abort.ended = ended.is_ok();
            (extracted, ended)
        });
        let mut segments = Vec::new();
        let mut window_started = clock();
        let read = loop {
            match output.recv() {
                Ok(AsrEvent::Begin { .. }) => {}
                Ok(AsrEvent::Progress { processed_ms, .. }) => {
                    if !cancel.load(Ordering::Acquire) {
                        send(
                            events,
                            repaint,
                            TranscriptWorkerEvent::Progress {
                                processed_ms,
                                duration_ms,
                            },
                        );
                    }
                }
                Ok(AsrEvent::Window(window)) => {
                    stage(
                        "infer_window",
                        window_started,
                        request_id,
                        &[
                            ("start_ms", serde_json::json!(window.start_ms)),
                            ("read_end_ms", serde_json::json!(window.read_end_ms)),
                            ("join_ms", serde_json::json!(window.join_ms)),
                            ("segments", serde_json::json!(window.segments.len())),
                            (
                                "utf8_discarded_bytes",
                                serde_json::json!(window.utf8_discarded_bytes),
                            ),
                            (
                                "utf8_discarded_fragments",
                                serde_json::json!(window.utf8_discarded_fragments),
                            ),
                        ],
                    );
                    window_started = clock();
                    if !cancel.load(Ordering::Acquire) {
                        segments.extend(
                            window
                                .segments
                                .into_iter()
                                .map(|s| (s.start_ms, s.end_ms, s.text)),
                        );
                    }
                }
                Ok(AsrEvent::End {
                    duration_ms,
                    utf8_discarded_bytes,
                    utf8_discarded_fragments,
                    ..
                }) => {
                    if !cancel.load(Ordering::Acquire) {
                        // containerの全長はhint。最後は実際に送ったPCM sample数へそろえる。
                        send(
                            events,
                            repaint,
                            TranscriptWorkerEvent::Progress {
                                processed_ms: duration_ms,
                                duration_ms,
                            },
                        );
                    }
                    crate::logger::log(format!(
                        "transcript_job: request={request_id} utf8_discarded_bytes={utf8_discarded_bytes} utf8_discarded_fragments={utf8_discarded_fragments}"
                    ));
                    crate::perf::event(
                        "transcript",
                        "utf8_discarded",
                        None,
                        0,
                        &[
                            ("request_id", serde_json::json!(request_id)),
                            (
                                "utf8_discarded_bytes",
                                serde_json::json!(utf8_discarded_bytes),
                            ),
                            (
                                "utf8_discarded_fragments",
                                serde_json::json!(utf8_discarded_fragments),
                            ),
                        ],
                    );
                    break Ok(duration_ms);
                }
                Err(error) => {
                    let _ = kill.kill();
                    break Err(error.to_string());
                }
            }
        };
        let written = writer
            .join()
            .map_err(|_| "音声の抽出threadが異常終了しました".to_owned())?;
        let duration_ms = read?;
        written.1?;
        Ok(Inferred {
            segments,
            duration_ms,
            extraction_error: written.0.err(),
        })
    });
    result
}

fn save(
    config: &TranscriptWorkerConfig,
    db: &TranscriptDb,
    cancel: &Arc<AtomicBool>,
    path: &Path,
    signature: FileSignature,
    request_id: u64,
    result: TranscriptResult,
) -> Result<TranscriptItemOutcome, String> {
    let failed = matches!(result, TranscriptResult::Failed);
    let started = clock();
    let saved = db.save_unless_cancelled(
        &SaveRequest {
            item_path: path.to_owned(),
            signature,
            model_id: config.model_id.clone(),
            config_version: CONFIG_VERSION,
            lang: "ja".into(),
            result,
        },
        cancel,
    );
    stage(
        "save",
        started,
        request_id,
        &[("success", serde_json::json!(saved.is_ok()))],
    );
    // commit後にはcancelを再検査しない。保存済み通知はStoppingでも必ず採用する。
    match saved.map_err(|e| e.to_string())? {
        None => Ok(TranscriptItemOutcome::Cancelled),
        Some(SaveOutcome::Discarded) => Ok(TranscriptItemOutcome::Discarded),
        Some(SaveOutcome::KeptExistingOk) => Ok(TranscriptItemOutcome::Failed),
        Some(SaveOutcome::Written) => Ok(if failed {
            TranscriptItemOutcome::FailedWritten
        } else {
            TranscriptItemOutcome::Created
        }),
    }
}

#[cfg(test)]
#[path = "transcript_job_worker_tests.rs"]
mod tests;
