use std::fmt;
use std::io::{Read, Write};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use mimageviewer_ipc::{
    ClientHello, MAX_CONTROL_FRAME_BYTES, MAX_RESPONSE_FRAME_BYTES, PIPE_NAME, PROTOCOL_VERSION,
    ThumbnailError, ThumbnailRequest, ThumbnailResponse, read_frame, write_frame,
};

const CONNECT_TIMEOUT: Duration = Duration::from_millis(250);
const UNAVAILABLE_BACKOFF: Duration = Duration::from_secs(2);

pub struct ThumbnailClient {
    pipe_name: String,
    unavailable_since: Mutex<Option<Instant>>,
}

#[derive(Debug)]
pub enum ClientError {
    Unavailable,
    VersionMismatch { client: u32, server: u32 },
    Protocol(String),
    Remote(ThumbnailError),
}

impl fmt::Display for ClientError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unavailable => write!(f, "mIV 本体の IPC に接続できません"),
            Self::VersionMismatch { client, server } => write!(
                f,
                "IPC プロトコル版が一致しません (remote-web={client}, mIV={server})"
            ),
            Self::Protocol(error) => write!(f, "IPC 通信に失敗しました: {error}"),
            Self::Remote(error) => write!(f, "mIV 本体が要求を拒否しました: {}", error.message),
        }
    }
}

impl ThumbnailClient {
    pub fn new() -> Self {
        Self {
            pipe_name: PIPE_NAME.to_owned(),
            unavailable_since: Mutex::new(None),
        }
    }

    pub fn probe(&self) -> Result<(), ClientError> {
        let mut pipe = self.connect(true)?;
        handshake(&mut pipe)
    }

    pub fn thumbnail(
        &self,
        favorite_id: &str,
        relative_path: &str,
        target_px: u32,
    ) -> Result<Vec<u8>, ClientError> {
        let mut pipe = self.connect(false)?;
        handshake(&mut pipe)?;
        write_frame(
            &mut pipe,
            &ThumbnailRequest {
                favorite_id: favorite_id.to_owned(),
                relative_path: relative_path.to_owned(),
                target_px,
            },
        )
        .map_err(|error| ClientError::Protocol(error.to_string()))?;
        let response: ThumbnailResponse = read_frame(&mut pipe, MAX_RESPONSE_FRAME_BYTES)
            .map_err(|error| ClientError::Protocol(error.to_string()))?;
        match response {
            ThumbnailResponse::Success { webp_bytes } => Ok(webp_bytes),
            ThumbnailResponse::Error(error) => Err(ClientError::Remote(error)),
        }
    }

    fn connect(&self, bypass_backoff: bool) -> Result<PipeStream, ClientError> {
        if !bypass_backoff
            && self
                .unavailable_since
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .is_some_and(|at| at.elapsed() < UNAVAILABLE_BACKOFF)
        {
            return Err(ClientError::Unavailable);
        }
        match PipeStream::connect(&self.pipe_name, CONNECT_TIMEOUT) {
            Ok(pipe) => {
                *self
                    .unavailable_since
                    .lock()
                    .unwrap_or_else(|error| error.into_inner()) = None;
                Ok(pipe)
            }
            Err(_) => {
                *self
                    .unavailable_since
                    .lock()
                    .unwrap_or_else(|error| error.into_inner()) = Some(Instant::now());
                Err(ClientError::Unavailable)
            }
        }
    }
}

fn handshake(pipe: &mut PipeStream) -> Result<(), ClientError> {
    write_frame(pipe, &ClientHello::current())
        .map_err(|error| ClientError::Protocol(error.to_string()))?;
    let hello: mimageviewer_ipc::ServerHello = read_frame(pipe, MAX_CONTROL_FRAME_BYTES)
        .map_err(|error| ClientError::Protocol(error.to_string()))?;
    if !hello.accepted || hello.protocol_version != PROTOCOL_VERSION {
        return Err(ClientError::VersionMismatch {
            client: PROTOCOL_VERSION,
            server: hello.protocol_version,
        });
    }
    Ok(())
}

#[cfg(windows)]
struct PipeStream {
    handle: windows::Win32::Foundation::HANDLE,
}

#[cfg(windows)]
impl PipeStream {
    fn connect(pipe_name: &str, timeout: Duration) -> Result<Self, std::io::Error> {
        use windows::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_PIPE_BUSY, GetLastError};
        use windows::Win32::Storage::FileSystem::{
            CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_GENERIC_READ, FILE_GENERIC_WRITE,
            FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
        };
        use windows::Win32::System::Pipes::WaitNamedPipeW;
        use windows::core::PCWSTR;

        let name: Vec<u16> = pipe_name.encode_utf16().chain([0]).collect();
        let deadline = Instant::now() + timeout;
        loop {
            let handle = unsafe {
                CreateFileW(
                    PCWSTR(name.as_ptr()),
                    FILE_GENERIC_READ.0 | FILE_GENERIC_WRITE.0,
                    FILE_SHARE_READ | FILE_SHARE_WRITE,
                    None,
                    OPEN_EXISTING,
                    FILE_ATTRIBUTE_NORMAL,
                    None,
                )
            };
            match handle {
                Ok(handle) => return Ok(Self { handle }),
                Err(_) => {
                    let error = unsafe { GetLastError() };
                    if error == ERROR_PIPE_BUSY && Instant::now() < deadline {
                        unsafe {
                            let _ = WaitNamedPipeW(PCWSTR(name.as_ptr()), 20);
                        }
                        continue;
                    }
                    if error == ERROR_FILE_NOT_FOUND && Instant::now() < deadline {
                        std::thread::sleep(Duration::from_millis(10));
                        continue;
                    }
                    return Err(std::io::Error::last_os_error());
                }
            }
        }
    }
}

#[cfg(not(windows))]
struct PipeStream;

#[cfg(not(windows))]
impl PipeStream {
    fn connect(_pipe_name: &str, _timeout: Duration) -> Result<Self, std::io::Error> {
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "Windows named pipes are unavailable",
        ))
    }
}

#[cfg(windows)]
impl Read for PipeStream {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        use windows::Win32::Storage::FileSystem::ReadFile;
        let mut read = 0_u32;
        unsafe { ReadFile(self.handle, Some(buffer), Some(&mut read), None) }
            .map_err(|_| std::io::Error::last_os_error())?;
        Ok(read as usize)
    }
}

#[cfg(not(windows))]
impl Read for PipeStream {
    fn read(&mut self, _buffer: &mut [u8]) -> std::io::Result<usize> {
        Ok(0)
    }
}

#[cfg(windows)]
impl Write for PipeStream {
    fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
        use windows::Win32::Storage::FileSystem::WriteFile;
        let mut written = 0_u32;
        unsafe { WriteFile(self.handle, Some(buffer), Some(&mut written), None) }
            .map_err(|_| std::io::Error::last_os_error())?;
        Ok(written as usize)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[cfg(not(windows))]
impl Write for PipeStream {
    fn write(&mut self, _buffer: &[u8]) -> std::io::Result<usize> {
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "Windows named pipes are unavailable",
        ))
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[cfg(windows)]
impl Drop for PipeStream {
    fn drop(&mut self) {
        unsafe {
            let _ = windows::Win32::Foundation::CloseHandle(self.handle);
        }
    }
}
