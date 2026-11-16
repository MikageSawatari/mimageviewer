//! VLM / ASR 共通のプロセス、実行ファイルと length-framed IPC。
use crate::job_process::{self, JobChild, KillHandle, StdinMode};
use serde_json::Value;
use std::ffi::OsString;
use std::io::{self, BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
#[cfg(not(feature = "portable"))]
#[path = "vlm_worker_extract.rs"]
mod extract;
pub(crate) const MAX_FRAME: usize = 16 * 1024 * 1024;
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkerError {
    Timeout,
    Unavailable,
    Fatal(String),
    Protocol(String),
    Io(String),
}

impl std::fmt::Display for WorkerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "AI worker: {self:?}")
    }
}
impl std::error::Error for WorkerError {}
impl From<io::Error> for WorkerError {
    fn from(error: io::Error) -> Self {
        Self::Io(error.to_string())
    }
}

pub(crate) struct Frame {
    pub header: Value,
    pub bytes: Vec<u8>,
}

pub(crate) fn encode_frame(header: &Value, bytes: &[u8]) -> Result<Vec<u8>, WorkerError> {
    let json = serde_json::to_vec(header).map_err(|e| WorkerError::Protocol(e.to_string()))?;
    let len = 4usize
        .checked_add(json.len())
        .and_then(|n| n.checked_add(bytes.len()))
        .filter(|n| *n <= MAX_FRAME)
        .ok_or_else(|| WorkerError::Protocol("frame too large".into()))?;
    let mut frame = Vec::with_capacity(len + 4);
    frame.extend_from_slice(&(len as u32).to_le_bytes());
    frame.extend_from_slice(&(json.len() as u32).to_le_bytes());
    frame.extend_from_slice(&json);
    frame.extend_from_slice(bytes);
    Ok(frame)
}

pub(crate) fn read_frame(reader: &mut impl Read) -> Result<Frame, WorkerError> {
    let mut prefix = [0; 4];
    reader.read_exact(&mut prefix)?;
    let len = u32::from_le_bytes(prefix) as usize;
    if !(4..=MAX_FRAME).contains(&len) {
        return Err(WorkerError::Protocol("invalid frame length".into()));
    }
    let mut body = vec![0; len];
    reader.read_exact(&mut body)?;
    let json_len = u32::from_le_bytes(body[..4].try_into().unwrap()) as usize;
    if json_len == 0 || json_len > len - 4 {
        return Err(WorkerError::Protocol("invalid JSON length".into()));
    }
    let header: Value = serde_json::from_slice(&body[4..4 + json_len])
        .map_err(|e| WorkerError::Protocol(e.to_string()))?;
    if !header.is_object() {
        return Err(WorkerError::Protocol("JSON object required".into()));
    }
    Ok(Frame {
        header,
        bytes: body[4 + json_len..].to_vec(),
    })
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
pub(crate) enum WorkerExecutableSource {
    Override(PathBuf),
    Bundled,
    Embedded,
}

// Keep the precedence independent of environment mutation and filesystem I/O so
// both build flavors and release's ignored override are covered in one test.
pub(crate) fn worker_executable_source(
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

/// 起動と stderr の所有を共有する。各役割の stdin/stdout と状態は独立。
pub(crate) fn spawn_pipes(
    executable: &Path,
    args: &[OsString],
    role: &'static str,
) -> io::Result<(JobChild, KillHandle, std::fs::File, std::fs::File)> {
    let mut child = job_process::spawn(
        executable,
        args,
        &std::env::vars_os().collect::<Vec<_>>(),
        StdinMode::Pipe,
    )?;
    let stdin = child.take_stdin()?;
    let stdout = child.take_stdout()?;
    let stderr = child.take_stderr()?;
    let kill = child.kill_handle();
    std::thread::Builder::new()
        .name(format!("{role}-stderr"))
        .spawn(move || {
            let mut reader = BufReader::new(stderr);
            loop {
                let mut chunk = Vec::new();
                match reader.by_ref().take(8192).read_until(b'\n', &mut chunk) {
                    Ok(0) | Err(_) => break,
                    Ok(_) => crate::logger::log(format!(
                        "{role}_worker: {}",
                        String::from_utf8_lossy(&chunk).trim_end()
                    )),
                }
            }
        })?;
    Ok((child, kill, stdin, stdout))
}
