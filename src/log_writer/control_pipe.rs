//! A local, current-user-only completion endpoint. It has no application-control
//! operations and does not use the UI or Remote session queue.
use super::*;
use std::os::windows::io::AsRawHandle;
use windows::Win32::Foundation::{
    CloseHandle, ERROR_IO_PENDING, ERROR_PIPE_CONNECTED, GetLastError, HANDLE, WAIT_OBJECT_0,
};
use windows::Win32::Storage::FileSystem::{
    BY_HANDLE_FILE_INFORMATION, FILE_FLAG_FIRST_PIPE_INSTANCE, FILE_FLAG_OVERLAPPED,
    GetFileInformationByHandle, PIPE_ACCESS_DUPLEX, ReadFile, WriteFile,
};
use windows::Win32::System::IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED};
use windows::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, NAMED_PIPE_MODE, PIPE_READMODE_BYTE,
    PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE, PIPE_WAIT,
};
use windows::Win32::System::Threading::{CreateEventW, INFINITE, WaitForSingleObject};
use windows::core::PCWSTR;

struct Handle(HANDLE);
impl Drop for Handle {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}
pub(super) fn start(service: Arc<Service>) {
    let _ = std::thread::Builder::new()
        .name("log-delivery-control".into())
        .spawn(move || {
            if let Err(error) = serve(&service) {
                service.fail(format!("log completion endpoint: {error}"));
            }
        });
}
fn finish(
    handle: HANDLE,
    op: &mut OVERLAPPED,
    result: windows::core::Result<()>,
    deadline: Option<Instant>,
) -> std::io::Result<u32> {
    if let Err(error) = result
        && unsafe { GetLastError() } != ERROR_IO_PENDING
    {
        return Err(std::io::Error::other(error.to_string()));
    }
    let timeout = deadline.map_or(INFINITE, |d| {
        d.saturating_duration_since(Instant::now())
            .as_millis()
            .min(u32::MAX as u128 - 1) as u32
    });
    let waited = unsafe { WaitForSingleObject(op.hEvent, timeout) };
    if waited != WAIT_OBJECT_0 {
        // The OVERLAPPED and borrowed buffer must remain alive until cancel has
        // completed. Cancellation here never waits on the log file owner.
        unsafe {
            let _ = CancelIoEx(handle, Some(op));
            let mut ignored = 0;
            let _ = GetOverlappedResult(handle, op, &mut ignored, true);
        }
        return Err(std::io::Error::new(
            std::io::ErrorKind::TimedOut,
            "log control I/O timeout",
        ));
    }
    let mut transferred = 0;
    unsafe { GetOverlappedResult(handle, op, &mut transferred, false) }
        .map_err(|e| std::io::Error::other(e.to_string()))?;
    Ok(transferred)
}
fn event() -> std::io::Result<Handle> {
    unsafe { CreateEventW(None, true, false, PCWSTR::null()) }
        .map(Handle)
        .map_err(|e| std::io::Error::other(e.to_string()))
}
fn read_exact(handle: HANDLE, mut bytes: &mut [u8], deadline: Instant) -> std::io::Result<()> {
    while !bytes.is_empty() {
        let event = event()?;
        let mut op = OVERLAPPED {
            hEvent: event.0,
            ..Default::default()
        };
        let result = unsafe { ReadFile(handle, Some(bytes), None, Some(&mut op)) };
        let count = finish(handle, &mut op, result, Some(deadline))? as usize;
        if count == 0 {
            return Err(std::io::ErrorKind::UnexpectedEof.into());
        }
        bytes = &mut bytes[count..];
    }
    Ok(())
}
fn write_all(handle: HANDLE, mut bytes: &[u8], deadline: Instant) -> std::io::Result<()> {
    while !bytes.is_empty() {
        let event = event()?;
        let mut op = OVERLAPPED {
            hEvent: event.0,
            ..Default::default()
        };
        let result = unsafe { WriteFile(handle, Some(bytes), None, Some(&mut op)) };
        let count = finish(handle, &mut op, result, Some(deadline))? as usize;
        if count == 0 {
            return Err(std::io::ErrorKind::WriteZero.into());
        }
        bytes = &bytes[count..];
    }
    Ok(())
}
fn serve(service: &Arc<Service>) -> std::io::Result<()> {
    let name: Vec<u16> = format!(
        "\\\\.\\pipe\\mimageviewer-log-{}-{}",
        std::process::id(),
        service.created
    )
    .encode_utf16()
    .chain([0])
    .collect();
    while service.is_running() {
        let mut security = crate::remote_ipc::current_user_pipe_security()?;
        let attrs = security.attributes();
        let raw = unsafe {
            CreateNamedPipeW(
                PCWSTR(name.as_ptr()),
                PIPE_ACCESS_DUPLEX | FILE_FLAG_OVERLAPPED | FILE_FLAG_FIRST_PIPE_INSTANCE,
                NAMED_PIPE_MODE(
                    PIPE_TYPE_BYTE.0
                        | PIPE_READMODE_BYTE.0
                        | PIPE_WAIT.0
                        | PIPE_REJECT_REMOTE_CLIENTS.0,
                ),
                1,
                8192,
                8192,
                0,
                Some(&attrs),
            )
        };
        if raw.is_invalid() {
            return Err(std::io::Error::last_os_error());
        }
        let pipe = Handle(raw);
        let event = event()?;
        let mut op = OVERLAPPED {
            hEvent: event.0,
            ..Default::default()
        };
        let result = unsafe { ConnectNamedPipe(pipe.0, Some(&mut op)) };
        if result.is_err() && unsafe { GetLastError() } == ERROR_PIPE_CONNECTED {
        } else {
            finish(pipe.0, &mut op, result, None)?;
        }
        let deadline = Instant::now() + BARRIER_WAIT;
        let result = (|| -> std::io::Result<()> {
            let mut len = [0; 4];
            read_exact(pipe.0, &mut len, deadline)?;
            let len = u32::from_le_bytes(len) as usize;
            if len == 0 || len > 4096 {
                return Err(std::io::ErrorKind::InvalidData.into());
            }
            let mut bytes = vec![0; len];
            read_exact(pipe.0, &mut bytes, deadline)?;
            let request: Value = serde_json::from_slice(&bytes)?;
            let response = handle_request(service, &request);
            let bytes = serde_json::to_vec(&response)?;
            write_all(pipe.0, &(bytes.len() as u32).to_le_bytes(), deadline)?;
            write_all(pipe.0, &bytes, deadline)
        })();
        let _ = result; // Malformed, timed-out, or disconnected local client only.
        unsafe {
            let _ = DisconnectNamedPipe(pipe.0);
        }
    }
    Ok(())
}
fn handle_request(service: &Arc<Service>, request: &Value) -> Value {
    let Some(id) = request["request_id"]
        .as_str()
        .filter(|s| !s.is_empty() && s.len() <= 128 && s.is_ascii())
    else {
        return json!({"error":"invalid request id"});
    };
    if request["schema"] != 1
        || request["pid"] != std::process::id()
        || request["process_start_unix_ms"] != service.created
        || request["session_id"] != service.session_id
    {
        return json!({"error":"session mismatch"});
    }
    let delivered = match service.barrier(id.into(), "measurement") {
        Ok(d) => d,
        Err(error) => return json!({"error":error}),
    };
    let Some(file) = delivered.perf else {
        return json!({"error":"perf disabled"});
    };
    let identity = match file_identity(&file.file) {
        Ok(id) => id,
        Err(error) => return json!({"error":error.to_string()}),
    };
    let mut response = serde_json::to_value(delivered.checkpoint).unwrap();
    response["perf_path"] = file.path.to_string_lossy().into_owned().into();
    response["perf_end_offset"] = file.len.into();
    response["perf_file_identity"] = identity.into();
    response
}
fn file_identity(file: &File) -> std::io::Result<String> {
    let mut info = BY_HANDLE_FILE_INFORMATION::default();
    unsafe { GetFileInformationByHandle(HANDLE(file.as_raw_handle()), &mut info) }
        .map_err(|e| std::io::Error::other(e.to_string()))?;
    let index = (u64::from(info.nFileIndexHigh) << 32) | u64::from(info.nFileIndexLow);
    Ok(format!("{}:{index}", info.dwVolumeSerialNumber))
}
