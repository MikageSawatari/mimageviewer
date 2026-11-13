//! ローカル文字起こしの transport。UI / DB / ジョブ / FFmpeg はここに置かない。
//! `start_at` 後すぐ kill handle を公開し、hello / load / split は worker thread で行う。
//! split 後は抽出 thread が Input、結果を処理する thread が Output を所有する。

use std::ffi::OsString;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, mpsc};
use std::time::Duration;

use serde::Deserialize;
use serde_json::{Value, json};

use super::worker_gpu::{self, Adapter, Device};
pub use super::worker_transport::worker_executable;
use super::worker_transport::{Frame, WorkerError, encode_frame, read_frame, spawn_pipes};
use crate::job_process::JobChild;
pub use crate::job_process::KillHandle;

pub const PROTOCOL_VERSION: u32 = 1;
pub const SETTINGS_VERSION: u32 = 1;
pub const MAX_PCM_BYTES: usize = 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AsrError {
    Transport(WorkerError),
    Worker { code: String, message: String },
    Protocol(String),
    Unavailable,
}
impl std::fmt::Display for AsrError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "ASR worker: {self:?}")
    }
}
impl std::error::Error for AsrError {}
impl From<WorkerError> for AsrError {
    fn from(error: WorkerError) -> Self {
        Self::Transport(error)
    }
}
impl From<std::io::Error> for AsrError {
    fn from(error: std::io::Error) -> Self {
        Self::Transport(error.into())
    }
}

#[derive(Clone, Debug)]
pub enum GpuDetection {
    VulkanLoaderUnavailable,
    Device(GpuDecision),
}

#[derive(Clone, Debug)]
pub struct GpuDecision {
    pub device: Option<Adapter>,
    pub vulkan_available: bool,
}
impl GpuDecision {
    pub fn is_eligible(&self) -> bool {
        self.device.is_some() && self.vulkan_available
    }
}

/// UI thread では呼ばない。空き VRAM の測定は行わない。
pub fn detect_gpu() -> Result<GpuDetection, AsrError> {
    match worker_gpu::inventory()? {
        None => Ok(GpuDetection::VulkanLoaderUnavailable),
        Some(inventory) => select_gpu(&inventory).map(GpuDetection::Device),
    }
}

fn select_gpu(inventory: &Value) -> Result<GpuDecision, AsrError> {
    let adapters: Vec<Adapter> = serde_json::from_value(inventory["dxgi"].clone())
        .map_err(|e| AsrError::Protocol(e.to_string()))?;
    let vulkan = inventory["vulkan"]
        .as_array()
        .ok_or_else(|| AsrError::Protocol("missing Vulkan inventory".into()))?;
    // 同量なら最初の候補。専用メモリ 0 の内蔵 GPU も候補に残す。
    let device = adapters
        .into_iter()
        .filter(|a| !a.software)
        .reduce(|best, next| {
            if next.dedicated_bytes > best.dedicated_bytes {
                next
            } else {
                best
            }
        });
    let vulkan_available = device.as_ref().is_some_and(|adapter| {
        let mut matches = vulkan.iter().filter(|v| {
            v["luid_valid"] == true
                && v["luid"].as_str() == Some(adapter.luid.as_str())
                && matches!(v["mapping"].as_str(), Some("pci-id" | "unique-description"))
        });
        matches.next().is_some() && matches.next().is_none()
    });
    Ok(GpuDecision {
        device,
        vulkan_available,
    })
}

#[derive(Debug)]
pub struct LoadRequest {
    pub model_path: PathBuf,
    /// 列挙時に選んだidentityを保持する。変わり得るVulkanの番号は送らない。
    pub device_luid: String,
}
#[derive(Debug, Deserialize)]
pub struct LoadResult {
    pub model_load_s: f64,
    pub cpu_threads: u32,
    pub device: Device,
}

struct ProcessOwner {
    _child: JobChild,
    kill: KillHandle,
}
impl ProcessOwner {
    fn fail<T>(&self, error: AsrError) -> Result<T, AsrError> {
        crate::logger::log(format!("asr_worker: {error}"));
        let _ = self.kill.kill();
        Err(error)
    }
}

enum ClientState {
    New,
    Unloaded,
    Loaded,
}
pub struct AsrWorkerClient {
    owner: Option<Arc<ProcessOwner>>,
    stdin: Option<std::fs::File>,
    replies: Option<mpsc::Receiver<Result<Frame, WorkerError>>>,
    state: ClientState,
    pub source_identity: String,
}

impl AsrWorkerClient {
    pub fn start() -> Result<Self, AsrError> {
        Self::start_at(&worker_executable()?)
    }

    /// hello の前に停止権をジョブへ渡せるよう、起動だけを行う。
    pub fn start_at(executable: &Path) -> Result<Self, AsrError> {
        Self::start_with(executable, &[OsString::from("--asr-serve")])
    }

    fn start_with(executable: &Path, args: &[OsString]) -> Result<Self, AsrError> {
        let (child, kill, stdin, mut stdout) = spawn_pipes(executable, args, "asr")?;
        let owner = Arc::new(ProcessOwner {
            _child: child,
            kill: kill.clone(),
        });
        // bounded channel で遅い consumer にも結果の無制限蓄積を許さない。
        let (tx, replies) = mpsc::sync_channel(1);
        std::thread::Builder::new()
            .name("asr-stdout".into())
            .spawn(move || {
                loop {
                    let frame = read_frame(&mut stdout);
                    let failed = frame.is_err();
                    if failed {
                        let _ = kill.kill();
                    }
                    if tx.send(frame).is_err() || failed {
                        break;
                    }
                }
            })?;
        Ok(Self {
            owner: Some(owner),
            stdin: Some(stdin),
            replies: Some(replies),
            state: ClientState::New,
            source_identity: String::new(),
        })
    }

    pub fn kill_handle(&self) -> KillHandle {
        self.owner.as_ref().unwrap().kill.clone()
    }

    fn exchange(
        &mut self,
        header: Value,
        expected: &str,
        timeout: Duration,
    ) -> Result<Value, AsrError> {
        let result = (|| {
            let bytes = encode_frame(&header, &[])?;
            self.stdin.as_mut().unwrap().write_all(&bytes)?;
            let frame = self
                .replies
                .as_ref()
                .unwrap()
                .recv_timeout(timeout)
                .map_err(|e| {
                    AsrError::Transport(match e {
                        mpsc::RecvTimeoutError::Timeout => WorkerError::Timeout,
                        mpsc::RecvTimeoutError::Disconnected => WorkerError::Unavailable,
                    })
                })??;
            checked_header(frame, expected)
        })();
        match result {
            Ok(header) => Ok(header),
            Err(e) => self.owner.as_ref().unwrap().fail(e),
        }
    }

    pub fn hello(&mut self) -> Result<(), AsrError> {
        if !matches!(self.state, ClientState::New) {
            return Err(AsrError::Unavailable);
        }
        let header = self.exchange(json!({"type":"hello"}), "hello", Duration::from_secs(30))?;
        let identity = header["source_identity"].as_str().unwrap_or("");
        if header["role"] != "asr"
            || header["protocol_version"].as_u64() != Some(PROTOCOL_VERSION as u64)
            || header["settings_version"].as_u64() != Some(SETTINGS_VERSION as u64)
            || identity.len() != 64
            || !identity.bytes().all(|c| c.is_ascii_hexdigit())
        {
            return self
                .owner
                .as_ref()
                .unwrap()
                .fail(AsrError::Protocol("incompatible hello".into()));
        }
        self.source_identity = identity.to_owned();
        self.state = ClientState::Unloaded;
        Ok(())
    }

    pub fn load(&mut self, request: &LoadRequest) -> Result<LoadResult, AsrError> {
        if !matches!(self.state, ClientState::Unloaded) {
            return Err(AsrError::Unavailable);
        }
        if request.device_luid.len() != 17
            || !request.device_luid.bytes().enumerate().all(|(i, b)| {
                if i == 8 {
                    b == b':'
                } else {
                    b.is_ascii_hexdigit()
                }
            })
        {
            return self
                .owner
                .as_ref()
                .unwrap()
                .fail(AsrError::Protocol("invalid GPU LUID".into()));
        }
        let header = self.exchange(
            json!({"type":"load", "model_path":request.model_path,
            "device_luid":request.device_luid}),
            "load",
            Duration::from_secs(120),
        )?;
        let result: LoadResult = match serde_json::from_value(header) {
            Ok(result) => result,
            Err(e) => {
                return self
                    .owner
                    .as_ref()
                    .unwrap()
                    .fail(AsrError::Protocol(e.to_string()));
            }
        };
        if !result.model_load_s.is_finite()
            || result.model_load_s < 0.0
            || !(1..=8).contains(&result.cpu_threads)
            || !result
                .device
                .luid
                .eq_ignore_ascii_case(&request.device_luid)
            || !result
                .device
                .luid
                .eq_ignore_ascii_case(&result.device.dxgi.luid)
            || result.device.dxgi.software
        {
            return self
                .owner
                .as_ref()
                .unwrap()
                .fail(AsrError::Protocol("invalid load result".into()));
        }
        self.state = ClientState::Loaded;
        Ok(result)
    }

    /// モデルは保持したまま、両 endpoint で begin→end を繰り返せる。
    /// Input/Output のどちらかを捨てると worker を終了し、相手の pipe 待ちも解く。
    pub fn split(mut self) -> Result<(AsrInput, AsrOutput), AsrError> {
        if !matches!(self.state, ClientState::Loaded) {
            return Err(AsrError::Unavailable);
        }
        let owner = self.owner.take().unwrap();
        let (requests, expected_requests) = mpsc::channel();
        Ok((
            AsrInput {
                owner: owner.clone(),
                stdin: self.stdin.take().unwrap(),
                request: None,
                requests,
            },
            AsrOutput {
                owner,
                replies: self.replies.take().unwrap(),
                stream: None,
                expected_requests,
            },
        ))
    }
}
impl Drop for AsrWorkerClient {
    fn drop(&mut self) {
        if let Some(owner) = &self.owner {
            let _ = owner.kill.kill();
        }
    }
}

fn checked_header(frame: Frame, expected: &str) -> Result<Value, AsrError> {
    if !frame.bytes.is_empty() {
        return Err(AsrError::Protocol("unexpected binary response".into()));
    }
    let header = frame.header;
    if header["type"] == "error" || header["status"] == "fatal" {
        return Err(AsrError::Worker {
            code: header["code"]
                .as_str()
                .unwrap_or("worker_failure")
                .to_owned(),
            message: header["error"]
                .as_str()
                .unwrap_or("worker failure")
                .to_owned(),
        });
    }
    if header["type"].as_str() != Some(expected)
        || (matches!(expected, "load" | "begin") && header["status"] != "ok")
        || (expected == "end" && header["status"] != "ok")
    {
        return Err(AsrError::Protocol(
            "unexpected response type or status".into(),
        ));
    }
    Ok(header)
}

pub struct AsrInput {
    owner: Arc<ProcessOwner>,
    stdin: std::fs::File,
    request: Option<InputRequest>,
    requests: mpsc::Sender<RequestBoundary>,
}
struct InputRequest {
    id: u64,
    samples: u64,
}
#[derive(Debug, PartialEq, Eq)]
enum RequestBoundary {
    Begin(u64),
    End { id: u64, duration_ms: u64 },
}
impl AsrInput {
    pub fn kill_handle(&self) -> KillHandle {
        self.owner.kill.clone()
    }
    fn send(&mut self, header: Value, pcm: &[u8]) -> Result<(), AsrError> {
        if self.owner.kill.is_killed() {
            return Err(AsrError::Unavailable);
        }
        let result = encode_frame(&header, pcm)
            .map_err(AsrError::from)
            .and_then(|frame| self.stdin.write_all(&frame).map_err(AsrError::from));
        match result {
            Ok(()) => Ok(()),
            Err(e) => self.owner.fail(e),
        }
    }
    pub fn begin(&mut self, request_id: u64) -> Result<(), AsrError> {
        if self.request.is_some() {
            return self
                .owner
                .fail(AsrError::Protocol("begin before end".into()));
        }
        // 応答より先に要求番号を公開し、別 thread の受信側でも照合する。
        // pipe の背圧が要求数も制限する。制御 channel 自体では待たない。
        if self
            .requests
            .send(RequestBoundary::Begin(request_id))
            .is_err()
        {
            return self.owner.fail(AsrError::Unavailable);
        }
        self.send(json!({"type":"begin", "request_id":request_id}), &[])?;
        self.request = Some(InputRequest {
            id: request_id,
            samples: 0,
        });
        Ok(())
    }
    /// 16kHz / mono / s16le。呼出し元で chunk 化し、pipe の背圧で抽出を待たせる。
    pub fn write_pcm(&mut self, pcm: &[u8]) -> Result<(), AsrError> {
        let Some(request) = &self.request else {
            return Err(AsrError::Unavailable);
        };
        let id = request.id;
        if pcm.is_empty() || pcm.len() > MAX_PCM_BYTES || pcm.len() % 2 != 0 {
            return self
                .owner
                .fail(AsrError::Protocol("invalid PCM frame length".into()));
        }
        let samples = request
            .samples
            .checked_add((pcm.len() / 2) as u64)
            .ok_or_else(|| AsrError::Protocol("PCM sample count overflow".into()))?;
        self.send(json!({"type":"pcm", "request_id":id}), pcm)?;
        self.request.as_mut().unwrap().samples = samples;
        Ok(())
    }
    pub fn end(&mut self) -> Result<(), AsrError> {
        let Some(request) = &self.request else {
            return Err(AsrError::Unavailable);
        };
        let id = request.id;
        // end の応答を受ける前に、実際に送った全sampleの長さを公開する。
        if self
            .requests
            .send(RequestBoundary::End {
                id,
                duration_ms: request.samples / 16,
            })
            .is_err()
        {
            return self.owner.fail(AsrError::Unavailable);
        }
        self.send(json!({"type":"end", "request_id":id}), &[])?;
        self.request = None;
        Ok(())
    }
}
impl Drop for AsrInput {
    fn drop(&mut self) {
        let _ = self.owner.kill.kill();
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct Segment {
    pub start_ms: u64,
    pub end_ms: u64,
    pub text: String,
}
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct Window {
    pub request_id: u64,
    pub start_ms: u64,
    pub read_end_ms: u64,
    pub join_ms: u64,
    #[serde(rename = "final")]
    pub final_window: bool,
    pub segments: Vec<Segment>,
    /// 採用するtoken本文だけをtrim前に修復した破棄数。lookaheadと端の空白は数えない。
    /// 不一致まで一致したprefixは1断片で、不一致byteを再評価する。不正な先頭・単独の
    /// 継続byteは各1byte/1断片。overlong等の不正な2byte目も同じ再評価規則を使う。
    /// 窓末尾の未完prefixは全体で1断片。空になって消えた区間の破棄数も残す。
    pub utf8_discarded_bytes: u64,
    pub utf8_discarded_fragments: u64,
}
#[derive(Debug, PartialEq, Eq)]
pub enum AsrEvent {
    Begin {
        request_id: u64,
    },
    Progress {
        request_id: u64,
        processed_ms: u64,
    },
    Window(Window),
    End {
        request_id: u64,
        duration_ms: u64,
        /// この入力で公開した全Windowの合計。次のbeginでは0へ戻す。
        utf8_discarded_bytes: u64,
        utf8_discarded_fragments: u64,
    },
}
struct StreamState {
    id: u64,
    next_start: u64,
    last_segment: Option<(u64, u64)>,
    final_end: Option<u64>,
    utf8_discarded: DiscardCount,
}
#[derive(Default, PartialEq, Eq)]
struct DiscardCount {
    bytes: u64,
    fragments: u64,
}
impl DiscardCount {
    fn checked(bytes: u64, fragments: u64) -> Result<Self, AsrError> {
        if (bytes == 0) != (fragments == 0) || fragments > bytes {
            return Err(AsrError::Protocol("invalid UTF-8 discard counters".into()));
        }
        Ok(Self { bytes, fragments })
    }
    fn add(&mut self, count: Self) -> Result<(), AsrError> {
        self.bytes = self
            .bytes
            .checked_add(count.bytes)
            .ok_or_else(|| AsrError::Protocol("UTF-8 discard byte count overflow".into()))?;
        self.fragments = self
            .fragments
            .checked_add(count.fragments)
            .ok_or_else(|| AsrError::Protocol("UTF-8 discard fragment count overflow".into()))?;
        Ok(())
    }
}
pub struct AsrOutput {
    owner: Arc<ProcessOwner>,
    replies: mpsc::Receiver<Result<Frame, WorkerError>>,
    stream: Option<StreamState>,
    expected_requests: mpsc::Receiver<RequestBoundary>,
}
impl AsrOutput {
    pub fn kill_handle(&self) -> KillHandle {
        self.owner.kill.clone()
    }
    /// block は結果 thread だけ。中止は retained KillHandle を使い、命令を送らない。
    pub fn recv(&mut self) -> Result<AsrEvent, AsrError> {
        // EOF を見た I/O thread は pipe の相手を即時停止するが、その前に
        // queue へ届いた fatal 応答や framing エラーを先に受け取り、ログに残す。
        // 外部中止後の未保存の結果を捨てる判断はジョブの Stopping が所有する。
        let result = self
            .replies
            .recv()
            .map_err(|_| AsrError::Unavailable)
            .and_then(|frame| frame.map_err(AsrError::from))
            .and_then(|frame| self.accept(frame));
        match result {
            Ok(event) => Ok(event),
            Err(e) => self.owner.fail(e),
        }
    }
    fn accept(&mut self, frame: Frame) -> Result<AsrEvent, AsrError> {
        let kind = frame.header["type"].as_str().unwrap_or("").to_owned();
        let h = checked_header(frame, &kind)?;
        let number = |name: &str| {
            h[name]
                .as_u64()
                .ok_or_else(|| AsrError::Protocol(format!("missing {name}")))
        };
        let id = number("request_id")?;
        let bad = || AsrError::Protocol("invalid ASR event order/timestamps".into());
        if kind == "begin" {
            if self.stream.is_some()
                || self.expected_requests.try_recv().ok() != Some(RequestBoundary::Begin(id))
            {
                return Err(bad());
            }
            self.stream = Some(StreamState {
                id,
                next_start: 0,
                last_segment: None,
                final_end: None,
                utf8_discarded: DiscardCount::default(),
            });
            return Ok(AsrEvent::Begin { request_id: id });
        }
        let stream = self.stream.as_mut().ok_or_else(bad)?;
        if stream.id != id {
            return Err(bad());
        }
        match kind.as_str() {
            "progress" if stream.final_end.is_none() => Ok(AsrEvent::Progress {
                request_id: id,
                processed_ms: number("processed_ms")?,
            }),
            "window" if stream.final_end.is_none() => {
                let window: Window =
                    serde_json::from_value(h).map_err(|e| AsrError::Protocol(e.to_string()))?;
                if window.start_ms != stream.next_start
                    || window.read_end_ms < window.start_ms
                    || window.join_ms < window.start_ms
                    || window.join_ms > window.read_end_ms
                    || (!window.final_window && window.join_ms == window.start_ms)
                    || (window.final_window && window.join_ms != window.read_end_ms)
                {
                    return Err(bad());
                }
                for segment in &window.segments {
                    if segment.end_ms < segment.start_ms
                        || segment.start_ms < window.start_ms
                        || (!window.final_window && segment.start_ms >= window.join_ms)
                        || stream.last_segment.is_some_and(|(start, end)| {
                            segment.start_ms < start || segment.end_ms < end
                        })
                    {
                        return Err(bad());
                    }
                    stream.last_segment = Some((segment.start_ms, segment.end_ms));
                }
                stream.utf8_discarded.add(DiscardCount::checked(
                    window.utf8_discarded_bytes,
                    window.utf8_discarded_fragments,
                )?)?;
                stream.next_start = window.join_ms;
                if window.final_window {
                    stream.final_end = Some(window.read_end_ms);
                }
                Ok(AsrEvent::Window(window))
            }
            "end" => {
                let duration_ms = number("duration_ms")?;
                if self.expected_requests.try_recv().ok()
                    != Some(RequestBoundary::End { id, duration_ms })
                {
                    return Err(bad());
                }
                let utf8_discarded_bytes = number("utf8_discarded_bytes")?;
                let utf8_discarded_fragments = number("utf8_discarded_fragments")?;
                if stream.final_end != Some(duration_ms)
                    || stream.utf8_discarded
                        != DiscardCount::checked(utf8_discarded_bytes, utf8_discarded_fragments)?
                {
                    return Err(bad());
                }
                self.stream = None;
                Ok(AsrEvent::End {
                    request_id: id,
                    duration_ms,
                    utf8_discarded_bytes,
                    utf8_discarded_fragments,
                })
            }
            _ => Err(bad()),
        }
    }
}
impl Drop for AsrOutput {
    fn drop(&mut self) {
        let _ = self.owner.kill.kill();
    }
}

#[cfg(test)]
#[path = "asr_worker_tests.rs"]
mod tests;
