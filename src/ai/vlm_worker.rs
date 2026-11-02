//! Local caption transport. All methods except `kill_handle`, `is_usable` and
//! `shutdown` belong on the caption job's worker thread, never the UI thread.
//! No UI, database, image decoder, retries or automatic worker restarts live here.

use std::ffi::OsString;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

pub use crate::job_process::KillHandle;
use crate::job_process::{self, JobChild, StdinMode};

pub const PROTOCOL_VERSION: u32 = 2;
pub const PROMPT_VERSION: i64 = 1;
pub const MIN_VRAM_BYTES: u64 = 15 * 1024 * 1024 * 1024 / 2;
#[cfg(not(feature = "portable"))]
#[path = "vlm_worker_extract.rs"]
mod extract;
const MAX_FRAME: usize = 16 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VlmError {
    Timeout,
    Unavailable,
    Fatal(String),
    Protocol(String),
    Io(String),
}

impl std::fmt::Display for VlmError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "VLM worker: {self:?}")
    }
}
impl std::error::Error for VlmError {}
impl From<io::Error> for VlmError {
    fn from(error: io::Error) -> Self {
        Self::Io(error.to_string())
    }
}

#[derive(Clone)]
struct Timeouts {
    hello: Duration,
    load: Duration,
    first_caption: Duration,
    caption: Duration,
}
impl Default for Timeouts {
    fn default() -> Self {
        Self {
            hello: Duration::from_secs(30),
            load: Duration::from_secs(120),
            first_caption: Duration::from_secs(120),
            caption: Duration::from_secs(60),
        }
    }
}

#[derive(Debug, Serialize)]
pub struct LoadRequest {
    pub model_path: PathBuf,
    pub mmproj_path: PathBuf,
    pub device_luid: String,
    pub cpu_threads: u32,
}

#[derive(Debug, Deserialize)]
pub struct LoadResult {
    pub model_load_s: f64,
    pub device: Device,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct Adapter {
    pub name: String,
    pub luid: String,
    pub dedicated_bytes: u64,
    pub software: bool,
}

#[derive(Clone, Debug, Deserialize)]
pub struct Device {
    pub name: String,
    pub luid: String,
    pub physical_index: u32,
    pub dxgi: Adapter,
}

#[derive(Debug, Deserialize)]
pub struct Validation {
    pub encoding_valid: bool,
    pub ended_on_eog: bool,
    pub json_complete: bool,
    pub field_types: bool,
    pub limits: bool,
    pub token_limit: bool,
    pub ok: bool,
}

#[derive(Debug, Deserialize)]
pub struct CaptionResult {
    pub content: String,
    pub finish_reason: String,
    pub caption: String,
    pub text: String,
    pub keywords: Vec<String>,
    pub validation: Validation,
    pub generated_tokens: u32,
    pub wall_s: f64,
}

impl CaptionResult {
    /// Validate the three fields again at the Rust ownership boundary (§6.5).
    fn valid(&self) -> bool {
        #[derive(Deserialize)]
        struct Fields {
            caption: String,
            text: String,
            keywords: Vec<String>,
        }
        let Ok(fields) = serde_json::from_str::<Fields>(&self.content) else {
            return false;
        };
        let v = &self.validation;
        self.finish_reason == "stop"
            && fields.caption == self.caption
            && fields.text == self.text
            && fields.keywords == self.keywords
            && v.encoding_valid
            && v.ended_on_eog
            && v.json_complete
            && v.field_types
            && v.limits
            && v.token_limit
            && v.ok
            && self.caption.chars().count() <= 400
            && self.text.chars().count() <= 400
            && self.keywords.len() <= 20
            && self.keywords.iter().all(|word| word.chars().count() <= 32)
            && self.generated_tokens <= 800
            && self.wall_s.is_finite()
            && self.wall_s >= 0.0
    }
}

#[derive(Debug)]
pub enum CaptionOutcome {
    Ok(CaptionResult),
    ImageFailure { message: String },
}

enum State {
    New,
    Unloaded,
    Loaded { first_caption: bool },
    Dead,
}

struct Frame {
    header: Value,
    bytes: Vec<u8>,
}

fn encode_frame(header: &Value, bytes: &[u8]) -> Result<Vec<u8>, VlmError> {
    let json = serde_json::to_vec(header).map_err(|e| VlmError::Protocol(e.to_string()))?;
    let len = 4usize
        .checked_add(json.len())
        .and_then(|n| n.checked_add(bytes.len()))
        .filter(|n| *n <= MAX_FRAME)
        .ok_or_else(|| VlmError::Protocol("frame too large".into()))?;
    let mut frame = Vec::with_capacity(len + 4);
    frame.extend_from_slice(&(len as u32).to_le_bytes());
    frame.extend_from_slice(&(json.len() as u32).to_le_bytes());
    frame.extend_from_slice(&json);
    frame.extend_from_slice(bytes);
    Ok(frame)
}

fn read_frame(reader: &mut impl Read) -> Result<Frame, VlmError> {
    let mut prefix = [0; 4];
    reader.read_exact(&mut prefix)?;
    let len = u32::from_le_bytes(prefix) as usize;
    if !(4..=MAX_FRAME).contains(&len) {
        return Err(VlmError::Protocol("invalid frame length".into()));
    }
    let mut body = vec![0; len];
    reader.read_exact(&mut body)?;
    let json_len = u32::from_le_bytes(body[..4].try_into().unwrap()) as usize;
    if json_len == 0 || json_len > len - 4 {
        return Err(VlmError::Protocol("invalid JSON length".into()));
    }
    let header: Value = serde_json::from_slice(&body[4..4 + json_len])
        .map_err(|e| VlmError::Protocol(e.to_string()))?;
    if !header.is_object() {
        return Err(VlmError::Protocol("JSON object required".into()));
    }
    Ok(Frame {
        header,
        bytes: body[4 + json_len..].to_vec(),
    })
}

/// Own one process for one caption job. The V1-3 owner uses `start_at`, publishes
/// `kill_handle()` to the App, then calls `hello`, `load` once, and `caption`
/// serially on its worker thread. This makes even the hello wait interruptible
/// by application exit. `spawn_at` is a convenience that performs hello as well.
///
/// V1-3's owner retains `kill_handle()` before load/caption and calls `kill()`
/// immediately on application exit. Normal user cancellation instead waits for
/// the current call to return, discards its result and calls `shutdown()`.
/// Shutdown/drop kill before detaching I/O threads; they never join on the UI.
/// Any transport/protocol/fatal error permanently retires this client. Only
/// `ImageFailure` permits the next image. No call retries or restarts a process.
pub struct VlmWorkerClient {
    _child: JobChild,
    kill: KillHandle,
    writer: Option<mpsc::SyncSender<Vec<u8>>>,
    replies: mpsc::Receiver<Result<Frame, VlmError>>,
    state: State,
    timeouts: Timeouts,
    pub source_identity: String,
}

impl VlmWorkerClient {
    pub fn spawn() -> Result<Self, VlmError> {
        Self::spawn_at(&worker_executable()?)
    }

    pub fn spawn_at(executable: &Path) -> Result<Self, VlmError> {
        Self::spawn_with(
            executable,
            &[OsString::from("--serve")],
            Timeouts::default(),
        )
    }

    fn spawn_with(
        executable: &Path,
        args: &[OsString],
        timeouts: Timeouts,
    ) -> Result<Self, VlmError> {
        let mut client = Self::start_with(executable, args, timeouts)?;
        client.hello()?;
        Ok(client)
    }

    /// Start the process and I/O threads without waiting for hello. Publish the
    /// kill handle to the App before calling any request method on the job thread.
    pub fn start_at(executable: &Path) -> Result<Self, VlmError> {
        Self::start_with(
            executable,
            &[OsString::from("--serve")],
            Timeouts::default(),
        )
    }

    fn start_with(
        executable: &Path,
        args: &[OsString],
        timeouts: Timeouts,
    ) -> Result<Self, VlmError> {
        let mut child = job_process::spawn(
            executable,
            args,
            &std::env::vars_os().collect::<Vec<_>>(),
            StdinMode::Pipe,
        )?;
        let mut stdin = child.take_stdin()?;
        let mut stdout = child.take_stdout()?;
        let stderr = child.take_stderr()?;
        let kill = child.kill_handle();
        let (writer_tx, writer_rx) = mpsc::sync_channel::<Vec<u8>>(1);
        let (reply_tx, replies) = mpsc::channel();
        let errors = reply_tx.clone();
        let writer_kill = kill.clone();
        std::thread::Builder::new()
            .name("vlm-stdin".into())
            .spawn(move || {
                while let Ok(frame) = writer_rx.recv() {
                    if let Err(error) = stdin.write_all(&frame).and_then(|_| stdin.flush()) {
                        let _ = writer_kill.kill();
                        let _ = errors.send(Err(error.into()));
                        break;
                    }
                }
            })?;
        let reader_kill = kill.clone();
        std::thread::Builder::new()
            .name("vlm-stdout".into())
            .spawn(move || {
                loop {
                    let frame = read_frame(&mut stdout);
                    let failed = frame.is_err();
                    if failed {
                        let _ = reader_kill.kill();
                    }
                    if reply_tx.send(frame).is_err() || failed {
                        break;
                    }
                }
            })?;
        std::thread::Builder::new()
            .name("vlm-stderr".into())
            .spawn(move || {
                // Bound each log chunk even if a damaged worker never writes a newline.
                let mut reader = BufReader::new(stderr);
                loop {
                    let mut chunk = Vec::new();
                    match reader.by_ref().take(8192).read_until(b'\n', &mut chunk) {
                        Ok(0) | Err(_) => break,
                        Ok(_) => crate::logger::log(format!(
                            "vlm_worker: {}",
                            String::from_utf8_lossy(&chunk).trim_end()
                        )),
                    }
                }
            })?;
        Ok(Self {
            _child: child,
            kill,
            writer: Some(writer_tx),
            replies,
            state: State::New,
            timeouts,
            source_identity: String::new(),
        })
    }

    /// Negotiate protocol version and retain source identity. No model is loaded.
    pub fn hello(&mut self) -> Result<(), VlmError> {
        if !matches!(self.state, State::New) {
            return Err(VlmError::Unavailable);
        }
        let hello = self.exchange(json!({"type":"hello"}), &[], "hello", self.timeouts.hello)?;
        if hello["protocol_version"].as_u64() != Some(PROTOCOL_VERSION as u64)
            || hello["prompt_version"].as_i64() != Some(PROMPT_VERSION)
            || hello["source_identity"].as_str().is_none_or(str::is_empty)
        {
            return self.fail(VlmError::Protocol("incompatible hello".into()));
        }
        self.source_identity = hello["source_identity"].as_str().unwrap().to_owned();
        self.state = State::Unloaded;
        Ok(())
    }

    pub fn kill_handle(&self) -> KillHandle {
        self.kill.clone()
    }

    pub fn is_usable(&self) -> bool {
        !matches!(self.state, State::Dead) && !self.kill.is_killed()
    }

    fn fail<T>(&mut self, error: VlmError) -> Result<T, VlmError> {
        self.state = State::Dead;
        let _ = self.kill.kill();
        self.writer.take();
        Err(error)
    }

    fn exchange(
        &mut self,
        header: Value,
        bytes: &[u8],
        expected: &str,
        timeout: Duration,
    ) -> Result<Value, VlmError> {
        if !self.is_usable() {
            return self.fail(VlmError::Unavailable);
        }
        let frame = match encode_frame(&header, bytes) {
            Ok(frame) => frame,
            Err(e) => return self.fail(e),
        };
        if self.writer.as_ref().unwrap().try_send(frame).is_err() {
            return self.fail(VlmError::Unavailable);
        }
        let frame = match self.replies.recv_timeout(timeout) {
            Ok(Ok(frame)) => frame,
            Ok(Err(e)) => return self.fail(e),
            Err(mpsc::RecvTimeoutError::Timeout) => return self.fail(VlmError::Timeout),
            Err(mpsc::RecvTimeoutError::Disconnected) => return self.fail(VlmError::Unavailable),
        };
        if frame.header["type"].as_str() != Some(expected) || !frame.bytes.is_empty() {
            return self.fail(VlmError::Protocol(
                "unexpected response type or binary payload".into(),
            ));
        }
        Ok(frame.header)
    }

    pub fn load(&mut self, request: &LoadRequest) -> Result<LoadResult, VlmError> {
        if !matches!(self.state, State::Unloaded) {
            return Err(VlmError::Unavailable);
        }
        let mut header = match serde_json::to_value(request) {
            Ok(header) => header,
            Err(e) => return self.fail(VlmError::Protocol(e.to_string())),
        };
        header["type"] = json!("load");
        let response = self.exchange(header, &[], "load", self.timeouts.load)?;
        if response["status"] == "fatal" {
            return self.fail(VlmError::Fatal(error_message(&response)));
        }
        if response["status"] != "ok" {
            return self.fail(VlmError::Protocol("invalid load status".into()));
        }
        let result: LoadResult = match serde_json::from_value(response) {
            Ok(r) => r,
            Err(e) => return self.fail(VlmError::Protocol(e.to_string())),
        };
        if !result.model_load_s.is_finite()
            || result.model_load_s < 0.0
            || result.device.luid != result.device.dxgi.luid
            || (!request.device_luid.is_empty()
                && !request
                    .device_luid
                    .eq_ignore_ascii_case(&result.device.luid))
        {
            return self.fail(VlmError::Protocol(
                "invalid loaded device or duration".into(),
            ));
        }
        self.state = State::Loaded {
            first_caption: true,
        };
        Ok(result)
    }

    pub fn caption(
        &mut self,
        request_id: u64,
        width: u32,
        height: u32,
        rgb: Vec<u8>,
    ) -> Result<CaptionOutcome, VlmError> {
        let first = match self.state {
            State::Loaded { first_caption } => first_caption,
            _ => return Err(VlmError::Unavailable),
        };
        if width == 0
            || height == 0
            || width > 1024
            || height > 1024
            || rgb.len() != width as usize * height as usize * 3
        {
            return self.fail(VlmError::Protocol("invalid RGB dimensions/length".into()));
        }
        let timeout = if first {
            self.timeouts.first_caption
        } else {
            self.timeouts.caption
        };
        let response = self.exchange(
            json!({"type":"caption","request_id":request_id,"width":width,"height":height}),
            &rgb,
            "caption",
            timeout,
        )?;
        if response["request_id"].as_u64() != Some(request_id) {
            return self.fail(VlmError::Protocol("request id mismatch".into()));
        }
        self.state = State::Loaded {
            first_caption: false,
        };
        match response["status"].as_str() {
            Some("fatal") => self.fail(VlmError::Fatal(error_message(&response))),
            Some("image_failure") => Ok(CaptionOutcome::ImageFailure {
                message: error_message(&response),
            }),
            Some("ok") => {
                let result: CaptionResult = match serde_json::from_value(response) {
                    Ok(r) => r,
                    Err(e) => return self.fail(VlmError::Protocol(e.to_string())),
                };
                if !result.valid() {
                    return Ok(CaptionOutcome::ImageFailure {
                        message: "caption validation failed".into(),
                    });
                }
                Ok(CaptionOutcome::Ok(result))
            }
            _ => self.fail(VlmError::Protocol("invalid caption status".into())),
        }
    }

    /// Call after the in-flight image finishes for normal cancellation. Does not
    /// wait for an acknowledgement or process/thread cleanup. App exit may call
    /// the retained kill handle directly without waiting for that image.
    pub fn shutdown(&mut self) {
        if self.is_usable() {
            if let Ok(frame) = encode_frame(&json!({"type":"shutdown"}), &[]) {
                if let Some(writer) = &self.writer {
                    let _ = writer.try_send(frame);
                }
            }
        }
        let _ = self.fail::<()>(VlmError::Unavailable);
    }
}
impl Drop for VlmWorkerClient {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn error_message(header: &Value) -> String {
    header["error"]
        .as_str()
        .unwrap_or("worker reported a failure")
        .to_owned()
}

/// Resolve/extract only on a worker thread. Release builds ignore overrides.
pub fn worker_executable() -> io::Result<PathBuf> {
    let override_path = if cfg!(debug_assertions) {
        std::env::var_os("MIV_VLM_WORKER")
    } else {
        None
    };
    match worker_executable_source(
        cfg!(debug_assertions),
        cfg!(feature = "portable"),
        override_path,
    ) {
        WorkerExecutableSource::Override(path) => Ok(path),
        #[cfg(feature = "portable")]
        WorkerExecutableSource::Bundled => {
            crate::native_assets::bundled("mimageviewer-vlm.exe").map_err(io::Error::other)
        }
        #[cfg(not(feature = "portable"))]
        WorkerExecutableSource::Embedded => extract::worker_executable(),
        _ => unreachable!("worker source must match the compile-time portable feature"),
    }
}

#[derive(Debug, PartialEq, Eq)]
enum WorkerExecutableSource {
    Override(PathBuf),
    Bundled,
    Embedded,
}

// Keep the precedence independent of environment mutation and filesystem I/O so
// both build flavors and release's ignored override are covered in one test.
fn worker_executable_source(
    debug: bool,
    portable: bool,
    override_path: Option<OsString>,
) -> WorkerExecutableSource {
    if debug && let Some(path) = override_path {
        return WorkerExecutableSource::Override(path.into());
    }
    if portable {
        WorkerExecutableSource::Bundled
    } else {
        WorkerExecutableSource::Embedded
    }
}

/// One immutable decision for this process, evaluated lazily on a worker thread.
/// The largest dedicated-memory DXGI adapter is selected even if it cannot run
/// Vulkan; a smaller adapter is never silently substituted. Pass its LUID to load.
#[derive(Clone, Debug)]
pub struct GpuDecision {
    pub device: Option<Adapter>,
    pub vram_total_bytes: u64,
    pub vulkan_available: bool,
}

impl GpuDecision {
    pub fn is_eligible(&self) -> bool {
        self.vulkan_available && self.vram_total_bytes >= MIN_VRAM_BYTES
    }
}

/// Must be called off the UI thread. Both success and failure are cached until
/// application exit. This uses dedicated total VRAM, never free VRAM or gpu_info.
pub fn detect_gpu() -> Result<GpuDecision, VlmError> {
    static DECISION: std::sync::OnceLock<Result<GpuDecision, VlmError>> =
        std::sync::OnceLock::new();
    DECISION
        .get_or_init(|| detect_gpu_at(&worker_executable()?, Duration::from_secs(30)))
        .clone()
}

fn select_gpu(inventory: &Value) -> Result<GpuDecision, VlmError> {
    let adapters: Vec<Adapter> = serde_json::from_value(inventory["dxgi"].clone())
        .map_err(|e| VlmError::Protocol(e.to_string()))?;
    let vulkan = inventory["vulkan"]
        .as_array()
        .ok_or_else(|| VlmError::Protocol("missing Vulkan inventory".into()))?;
    // Preserve enumeration order on ties, matching worker selection.
    let device = adapters
        .into_iter()
        .filter(|a| !a.software && a.dedicated_bytes > 0)
        .reduce(|best, next| {
            if next.dedicated_bytes > best.dedicated_bytes {
                next
            } else {
                best
            }
        });
    let vulkan_available = device.as_ref().is_some_and(|device| {
        vulkan.iter().any(|v| {
            v["luid_valid"] == true
                && v["luid"].as_str() == Some(device.luid.as_str())
                && matches!(v["mapping"].as_str(), Some("pci-id" | "unique-description"))
        })
    });
    Ok(GpuDecision {
        vram_total_bytes: device.as_ref().map_or(0, |a| a.dedicated_bytes),
        device,
        vulkan_available,
    })
}

fn detect_gpu_at(executable: &Path, timeout: Duration) -> Result<GpuDecision, VlmError> {
    use std::os::windows::io::AsRawHandle as _;
    use windows::Win32::Foundation::{HANDLE, WAIT_OBJECT_0};
    use windows::Win32::System::Threading::WaitForSingleObject;
    let start = std::time::Instant::now();
    let mut child = job_process::spawn(
        executable,
        &[OsString::from("--list-devices")],
        &std::env::vars_os().collect::<Vec<_>>(),
        StdinMode::Null,
    )?;
    let stdout = child.take_stdout()?;
    let stderr = child.take_stderr()?;
    let (tx, rx) = mpsc::channel();
    std::thread::Builder::new()
        .name("vlm-device-stdout".into())
        .spawn(move || {
            let mut bytes = Vec::new();
            let result = stdout
                .take(MAX_FRAME as u64 + 1)
                .read_to_end(&mut bytes)
                .map(|_| bytes);
            let _ = tx.send(result);
        })?;
    std::thread::Builder::new()
        .name("vlm-device-stderr".into())
        .spawn(move || {
            let mut reader = BufReader::new(stderr);
            loop {
                let mut chunk = Vec::new();
                match reader.by_ref().take(8192).read_until(b'\n', &mut chunk) {
                    Ok(0) | Err(_) => break,
                    Ok(_) => crate::logger::log(format!(
                        "vlm_device: {}",
                        String::from_utf8_lossy(&chunk).trim_end()
                    )),
                }
            }
        })?;
    let bytes = rx.recv_timeout(timeout).map_err(|e| match e {
        mpsc::RecvTimeoutError::Timeout => VlmError::Timeout,
        mpsc::RecvTimeoutError::Disconnected => VlmError::Unavailable,
    })??;
    if bytes.len() > MAX_FRAME {
        return Err(VlmError::Protocol("device inventory too large".into()));
    }
    let remaining = timeout
        .saturating_sub(start.elapsed())
        .as_millis()
        .min(u32::MAX as u128 - 1) as u32;
    if unsafe { WaitForSingleObject(HANDLE(child.process_handle().as_raw_handle()), remaining) }
        != WAIT_OBJECT_0
    {
        return Err(VlmError::Timeout);
    }
    if child.exit_code()? != 0 {
        return Err(VlmError::Fatal("device probe failed".into()));
    }
    select_gpu(&serde_json::from_slice(&bytes).map_err(|e| VlmError::Protocol(e.to_string()))?)
}

#[cfg(test)]
#[path = "vlm_worker_tests.rs"]
mod tests;
