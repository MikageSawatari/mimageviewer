//! DXGI/Vulkan の列挙だけを共有し、使用可否は役割ごとに判断する。
use super::worker_transport::{MAX_FRAME, WorkerError, worker_executable};
use crate::job_process::{self, StdinMode};
use serde::Deserialize;
use serde_json::Value;
use std::ffi::OsString;
use std::io::{BufRead, BufReader, Read};
use std::path::Path;
use std::sync::mpsc;
use std::time::Duration;
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

/// 列挙の結果のみ process 共通。役割の client / model / state は共有しない。
pub(crate) fn inventory() -> Result<Option<Value>, WorkerError> {
    static INVENTORY: std::sync::OnceLock<Result<Option<Value>, WorkerError>> =
        std::sync::OnceLock::new();
    INVENTORY
        .get_or_init(|| {
            if !system_vulkan_loader_available() {
                return Ok(None);
            }
            probe_inventory(&worker_executable()?, Duration::from_secs(30)).map(Some)
        })
        .clone()
}
pub(crate) fn system_vulkan_loader_available() -> bool {
    use windows::Win32::Foundation::FreeLibrary;
    use windows::Win32::System::LibraryLoader::{LOAD_LIBRARY_SEARCH_SYSTEM32, LoadLibraryExW};
    let Some(path) = system_vulkan_loader_path() else {
        return false;
    };
    // Use an absolute path as well as SYSTEM32: a bare name can reuse an
    // already-loaded app-local module before the directory search runs.
    let Ok(module) = (unsafe {
        LoadLibraryExW(
            windows::core::PCWSTR(path.as_ptr()),
            None,
            LOAD_LIBRARY_SEARCH_SYSTEM32,
        )
    }) else {
        return false;
    };
    let _ = unsafe { FreeLibrary(module) };
    true
}

pub(crate) fn system_vulkan_loader_path() -> Option<Vec<u16>> {
    use windows::Win32::System::SystemInformation::GetSystemDirectoryW;
    let mut directory = vec![0u16; 32768];
    let length = unsafe { GetSystemDirectoryW(Some(&mut directory)) } as usize;
    if length == 0 || length >= directory.len() {
        return None;
    }
    directory.truncate(length);
    directory.extend("\\vulkan-1.dll".encode_utf16());
    directory.push(0);
    Some(directory)
}

pub(crate) fn probe_inventory(executable: &Path, timeout: Duration) -> Result<Value, WorkerError> {
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
        mpsc::RecvTimeoutError::Timeout => WorkerError::Timeout,
        mpsc::RecvTimeoutError::Disconnected => WorkerError::Unavailable,
    })??;
    if bytes.len() > MAX_FRAME {
        return Err(WorkerError::Protocol("device inventory too large".into()));
    }
    let remaining = timeout
        .saturating_sub(start.elapsed())
        .as_millis()
        .min(u32::MAX as u128 - 1) as u32;
    if unsafe { WaitForSingleObject(HANDLE(child.process_handle().as_raw_handle()), remaining) }
        != WAIT_OBJECT_0
    {
        return Err(WorkerError::Timeout);
    }
    if child.exit_code()? != 0 {
        return Err(WorkerError::Fatal("device probe failed".into()));
    }
    serde_json::from_slice(&bytes).map_err(|e| WorkerError::Protocol(e.to_string()))
}
