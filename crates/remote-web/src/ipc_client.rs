use std::collections::HashMap;
use std::fmt;
use std::io::{Read, Write};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

use mimageviewer_ipc::{
    ClientHello, ClientMessage, CollectionError, CollectionErrorCode, CollectionKind,
    CollectionPayload, CollectionRequest, CollectionResponse, FrameError, HomePayload, HomeRequest,
    HomeResponse, MAX_CONTROL_FRAME_BYTES, MAX_RESPONSE_FRAME_BYTES, PIPE_NAME, PROTOCOL_VERSION,
    RequestId, ServerMessage, ThumbnailError, ThumbnailErrorCode, ThumbnailRequest,
    ThumbnailResponse, read_frame, write_frame,
};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(1);
const RESPONSE_TIMEOUT: Duration = Duration::from_secs(120);
const MAX_RETRIES: u32 = 2;

pub struct ThumbnailClient {
    pipe_name: String,
    connection: Mutex<Option<Arc<Connection>>>,
    next_request_id: AtomicU64,
    next_connection_id: AtomicU64,
}

pub struct ThumbnailSuccess {
    pub bytes: Vec<u8>,
    pub retry_count: u32,
    pub retry_statuses: Vec<String>,
    pub connection_id: u64,
}

pub struct IpcSuccess<T> {
    pub value: T,
    pub retry_count: u32,
    pub retry_statuses: Vec<String>,
    pub connection_id: u64,
}

pub struct ClientFailure {
    pub error: ClientError,
    pub retry_count: u32,
    pub retry_statuses: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct ProtocolFailure {
    pub stage: &'static str,
    pub kind: &'static str,
    pub os_error: Option<i32>,
    detail: String,
}

impl ProtocolFailure {
    #[cfg(test)]
    pub(crate) fn new(
        stage: &'static str,
        kind: &'static str,
        os_error: Option<i32>,
        detail: impl Into<String>,
    ) -> Self {
        protocol_failure(stage, kind, os_error, detail)
    }
}

#[derive(Clone, Debug)]
pub enum ClientError {
    Unavailable(ProtocolFailure),
    VersionMismatch { client: u32, server: u32 },
    Protocol(ProtocolFailure),
    Remote(ThumbnailError),
    CollectionRemote(CollectionError),
}

impl ClientError {
    pub fn is_transient(&self) -> bool {
        matches!(self, Self::Unavailable(_) | Self::Protocol(_))
            || matches!(self, Self::Remote(error) if error.code == ThumbnailErrorCode::Busy)
            || matches!(self, Self::CollectionRemote(error) if error.code == CollectionErrorCode::Busy)
    }

    pub fn ipc_status(&self) -> String {
        match self {
            Self::Unavailable(failure) | Self::Protocol(failure) => {
                format!("{}_{}", failure.stage, failure.kind)
            }
            Self::VersionMismatch { .. } => "protocol_version_mismatch".to_owned(),
            Self::Remote(error) => match error.code {
                ThumbnailErrorCode::BadRequest => "miv_bad_request",
                ThumbnailErrorCode::FavoriteNotFound => "miv_favorite_not_found",
                ThumbnailErrorCode::PathRejected => "miv_path_rejected",
                ThumbnailErrorCode::NotFound => "miv_not_found",
                ThumbnailErrorCode::Unsupported => "miv_unsupported",
                ThumbnailErrorCode::GenerationFailed => "miv_generation_failed",
                ThumbnailErrorCode::Busy => "miv_busy",
                ThumbnailErrorCode::Internal => "miv_internal",
            }
            .to_owned(),
            Self::CollectionRemote(error) => match error.code {
                CollectionErrorCode::BadRequest => "miv_collection_bad_request",
                CollectionErrorCode::NotFound => "miv_collection_not_found",
                CollectionErrorCode::Busy => "miv_collection_busy",
                CollectionErrorCode::Internal => "miv_collection_internal",
            }
            .to_owned(),
        }
    }

    pub fn protocol_failure(&self) -> Option<&ProtocolFailure> {
        match self {
            Self::Unavailable(failure) | Self::Protocol(failure) => Some(failure),
            _ => None,
        }
    }
}

impl fmt::Display for ClientError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unavailable(error) => write!(
                f,
                "mIV 本体の IPC に接続できません ({}:{}: {})",
                error.stage, error.kind, error.detail
            ),
            Self::VersionMismatch { client, server } => write!(
                f,
                "IPC プロトコル版が一致しません (remote-web={client}, mIV={server})"
            ),
            Self::Protocol(error) => write!(
                f,
                "IPC 通信に失敗しました ({}:{}: {})",
                error.stage, error.kind, error.detail
            ),
            Self::Remote(error) => write!(f, "mIV 本体が要求を拒否しました: {}", error.message),
            Self::CollectionRemote(error) => {
                write!(
                    f,
                    "mIV 本体が集約ビュー要求を拒否しました: {}",
                    error.message
                )
            }
        }
    }
}

impl ThumbnailClient {
    pub fn new() -> Self {
        Self {
            pipe_name: PIPE_NAME.to_owned(),
            connection: Mutex::new(None),
            next_request_id: AtomicU64::new(1),
            next_connection_id: AtomicU64::new(1),
        }
    }

    pub fn probe(&self) -> Result<(), ClientError> {
        self.get_connection().map(|_| ())
    }

    pub fn thumbnail(
        &self,
        favorite_id: &str,
        relative_path: &str,
        target_px: u32,
    ) -> Result<ThumbnailSuccess, ClientFailure> {
        let request = ThumbnailRequest {
            favorite_id: favorite_id.to_owned(),
            relative_path: relative_path.to_owned(),
            target_px,
        };
        run_with_retry(|| {
            let connection = self.get_connection()?;
            let connection_id = connection.id;
            let id = self.next_request_id.fetch_add(1, Ordering::Relaxed);
            match connection.request(
                id,
                ClientMessage::Thumbnail {
                    id,
                    request: request.clone(),
                },
            ) {
                Ok(ServerMessage::Thumbnail {
                    response: ThumbnailResponse::Success { webp_bytes },
                    ..
                }) => Ok((webp_bytes, connection_id)),
                Ok(ServerMessage::Thumbnail {
                    response: ThumbnailResponse::Error(error),
                    ..
                }) => Err(ClientError::Remote(error)),
                Ok(_) => Err(ClientError::Protocol(protocol_failure(
                    "response_route",
                    "response_type_mismatch",
                    None,
                    "thumbnail request received another response type",
                ))),
                Err(error) => {
                    self.invalidate_connection(connection_id);
                    Err(ClientError::Protocol(error))
                }
            }
        })
        .map(
            |RetrySuccess {
                 value: (bytes, connection_id),
                 retry_count,
                 retry_statuses,
             }| ThumbnailSuccess {
                bytes,
                retry_count,
                retry_statuses,
                connection_id,
            },
        )
        .map_err(|failure| ClientFailure {
            error: failure.error,
            retry_count: failure.retry_count,
            retry_statuses: failure.retry_statuses,
        })
    }

    pub fn home(&self) -> Result<IpcSuccess<HomePayload>, ClientFailure> {
        self.collection_request(|id| ClientMessage::Home {
            id,
            request: HomeRequest,
        })
        .and_then(|success| match success.value {
            ServerMessage::Home {
                response: HomeResponse::Success(payload),
                ..
            } => Ok(IpcSuccess {
                value: payload,
                retry_count: success.retry_count,
                retry_statuses: success.retry_statuses,
                connection_id: success.connection_id,
            }),
            ServerMessage::Home {
                response: HomeResponse::Error(error),
                ..
            } => Err(ClientFailure {
                error: ClientError::CollectionRemote(error),
                retry_count: success.retry_count,
                retry_statuses: success.retry_statuses,
            }),
            _ => Err(ClientFailure {
                error: ClientError::Protocol(protocol_failure(
                    "response_route",
                    "response_type_mismatch",
                    None,
                    "home request received another response type",
                )),
                retry_count: success.retry_count,
                retry_statuses: success.retry_statuses,
            }),
        })
    }

    pub fn collection(
        &self,
        kind: CollectionKind,
    ) -> Result<IpcSuccess<CollectionPayload>, ClientFailure> {
        self.collection_request(|id| ClientMessage::Collection {
            id,
            request: CollectionRequest { kind: kind.clone() },
        })
        .and_then(|success| match success.value {
            ServerMessage::Collection {
                response: CollectionResponse::Success(payload),
                ..
            } => Ok(IpcSuccess {
                value: payload,
                retry_count: success.retry_count,
                retry_statuses: success.retry_statuses,
                connection_id: success.connection_id,
            }),
            ServerMessage::Collection {
                response: CollectionResponse::Error(error),
                ..
            } => Err(ClientFailure {
                error: ClientError::CollectionRemote(error),
                retry_count: success.retry_count,
                retry_statuses: success.retry_statuses,
            }),
            _ => Err(ClientFailure {
                error: ClientError::Protocol(protocol_failure(
                    "response_route",
                    "response_type_mismatch",
                    None,
                    "collection request received another response type",
                )),
                retry_count: success.retry_count,
                retry_statuses: success.retry_statuses,
            }),
        })
    }

    fn collection_request(
        &self,
        request: impl Fn(RequestId) -> ClientMessage,
    ) -> Result<IpcSuccess<ServerMessage>, ClientFailure> {
        run_with_retry(|| {
            let connection = self.get_connection()?;
            let connection_id = connection.id;
            let id = self.next_request_id.fetch_add(1, Ordering::Relaxed);
            match connection.request(id, request(id)) {
                Ok(response) => Ok((response, connection_id)),
                Err(error) => {
                    self.invalidate_connection(connection_id);
                    Err(ClientError::Protocol(error))
                }
            }
        })
        .map(
            |RetrySuccess {
                 value: (value, connection_id),
                 retry_count,
                 retry_statuses,
             }| IpcSuccess {
                value,
                retry_count,
                retry_statuses,
                connection_id,
            },
        )
        .map_err(|failure| ClientFailure {
            error: failure.error,
            retry_count: failure.retry_count,
            retry_statuses: failure.retry_statuses,
        })
    }

    fn get_connection(&self) -> Result<Arc<Connection>, ClientError> {
        let mut slot = self
            .connection
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if let Some(connection) = slot.as_ref()
            && !connection.is_broken()
        {
            return Ok(Arc::clone(connection));
        }

        let mut pipe = PipeStream::connect(&self.pipe_name, CONNECT_TIMEOUT)
            .map_err(|error| ClientError::Unavailable(io_failure("connect", error)))?;
        handshake(&mut pipe)?;
        let connection_id = self.next_connection_id.fetch_add(1, Ordering::Relaxed);
        let connection = Connection::start(connection_id, pipe).map_err(ClientError::Protocol)?;
        *slot = Some(Arc::clone(&connection));
        Ok(connection)
    }

    fn invalidate_connection(&self, connection_id: u64) {
        let mut slot = self
            .connection
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if slot
            .as_ref()
            .is_some_and(|connection| connection.id == connection_id)
        {
            *slot = None;
        }
    }
}

fn handshake(pipe: &mut PipeStream) -> Result<(), ClientError> {
    write_frame(pipe, &ClientHello::current())
        .map_err(|error| ClientError::Protocol(frame_failure("handshake_write", error)))?;
    let hello: mimageviewer_ipc::ServerHello = read_frame(pipe, MAX_CONTROL_FRAME_BYTES)
        .map_err(|error| ClientError::Protocol(frame_failure("handshake_read", error)))?;
    if !hello.accepted || hello.protocol_version != PROTOCOL_VERSION {
        return Err(ClientError::VersionMismatch {
            client: PROTOCOL_VERSION,
            server: hello.protocol_version,
        });
    }
    Ok(())
}

type PendingReply = mpsc::SyncSender<Result<ServerMessage, ProtocolFailure>>;

#[derive(Default)]
struct PendingRequests {
    entries: Mutex<HashMap<RequestId, PendingReply>>,
}

impl PendingRequests {
    fn register(
        &self,
        id: RequestId,
        reply: PendingReply,
        broken: &AtomicBool,
    ) -> Result<(), ProtocolFailure> {
        let mut entries = self
            .entries
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if broken.load(Ordering::Acquire) {
            return Err(protocol_failure(
                "request_write",
                "connection_closed",
                None,
                "connection already closed",
            ));
        }
        entries.insert(id, reply);
        Ok(())
    }

    fn remove(&self, id: RequestId) -> Option<PendingReply> {
        self.entries
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .remove(&id)
    }

    fn resolve(&self, id: RequestId, response: ServerMessage) -> bool {
        self.remove(id)
            .is_some_and(|reply| reply.send(Ok(response)).is_ok())
    }

    fn fail_all(&self, failure: ProtocolFailure) {
        let pending = {
            let mut entries = self
                .entries
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            entries.drain().map(|(_, reply)| reply).collect::<Vec<_>>()
        };
        for reply in pending {
            let _ = reply.send(Err(failure.clone()));
        }
    }
}

struct Connection {
    id: u64,
    writer: Mutex<PipeStream>,
    cancel_pipe: PipeStream,
    pending: PendingRequests,
    broken: AtomicBool,
}

impl Connection {
    fn start(id: u64, pipe: PipeStream) -> Result<Arc<Self>, ProtocolFailure> {
        let connection = Arc::new(Self {
            id,
            writer: Mutex::new(pipe.clone()),
            cancel_pipe: pipe.clone(),
            pending: PendingRequests::default(),
            broken: AtomicBool::new(false),
        });
        let reader_connection = Arc::clone(&connection);
        std::thread::Builder::new()
            .name("remote-ipc-reader".to_owned())
            .spawn(move || reader_connection.reader_loop(pipe))
            .map_err(|error| {
                protocol_failure(
                    "reader_spawn",
                    "thread",
                    None,
                    format!("response reader thread: {error}"),
                )
            })?;
        Ok(connection)
    }

    fn is_broken(&self) -> bool {
        self.broken.load(Ordering::Acquire)
    }

    fn request(
        &self,
        id: RequestId,
        request: ClientMessage,
    ) -> Result<ServerMessage, ProtocolFailure> {
        let (reply_tx, reply_rx) = mpsc::sync_channel(1);
        self.pending.register(id, reply_tx, &self.broken)?;
        let write_result = {
            let mut writer = self
                .writer
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            write_frame(&mut *writer, &request)
        };
        if let Err(error) = write_result {
            self.pending.remove(id);
            let failure = frame_failure("request_write", error);
            self.fail(failure.clone(), true);
            return Err(failure);
        }
        match reply_rx.recv_timeout(RESPONSE_TIMEOUT) {
            Ok(result) => result,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                self.pending.remove(id);
                let failure = protocol_failure(
                    "response_read",
                    "timeout",
                    None,
                    "thumbnail response timed out",
                );
                self.fail(failure.clone(), true);
                Err(failure)
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => Err(protocol_failure(
                "response_route",
                "disconnected",
                None,
                "response dispatcher stopped",
            )),
        }
    }

    fn reader_loop(&self, mut pipe: PipeStream) {
        loop {
            let message: ServerMessage = match read_frame(&mut pipe, MAX_RESPONSE_FRAME_BYTES) {
                Ok(message) => message,
                Err(error) => {
                    self.fail(frame_failure("response_read", error), false);
                    return;
                }
            };
            let id = message.id();
            if !self.pending.resolve(id, message) {
                self.fail(
                    protocol_failure(
                        "response_route",
                        "unknown_request_id",
                        None,
                        format!("response id {id} has no pending request"),
                    ),
                    true,
                );
                return;
            }
        }
    }

    fn fail(&self, failure: ProtocolFailure, cancel_io: bool) {
        if self.broken.swap(true, Ordering::AcqRel) {
            return;
        }
        self.pending.fail_all(failure);
        if cancel_io {
            self.cancel_pipe.cancel_io();
        }
    }
}

struct RetrySuccess<T> {
    value: T,
    retry_count: u32,
    retry_statuses: Vec<String>,
}

#[derive(Debug)]
struct RetryFailure {
    error: ClientError,
    retry_count: u32,
    retry_statuses: Vec<String>,
}

fn run_with_retry<T>(
    mut operation: impl FnMut() -> Result<T, ClientError>,
) -> Result<RetrySuccess<T>, RetryFailure> {
    let mut retry_count = 0;
    let mut retry_statuses = Vec::new();
    loop {
        match operation() {
            Ok(value) => {
                return Ok(RetrySuccess {
                    value,
                    retry_count,
                    retry_statuses,
                });
            }
            Err(error) if error.is_transient() && retry_count < MAX_RETRIES => {
                retry_statuses.push(error.ipc_status());
                std::thread::sleep(retry_delay(retry_count));
                retry_count += 1;
            }
            Err(error) => {
                return Err(RetryFailure {
                    error,
                    retry_count,
                    retry_statuses,
                });
            }
        }
    }
}

fn retry_delay(retry_count: u32) -> Duration {
    Duration::from_millis(25_u64.saturating_mul(1_u64 << retry_count.min(6)))
}

fn frame_failure(stage: &'static str, error: FrameError) -> ProtocolFailure {
    match error {
        FrameError::Io(error) => io_failure(stage, error),
        FrameError::TooLarge { length, maximum } => protocol_failure(
            stage,
            "frame_too_large",
            None,
            format!("{length} > {maximum}"),
        ),
        FrameError::Encode(error) => protocol_failure(stage, "encode", None, error),
        FrameError::Decode(error) => protocol_failure(stage, "decode", None, error),
    }
}

fn io_failure(stage: &'static str, error: std::io::Error) -> ProtocolFailure {
    let kind = match error.kind() {
        std::io::ErrorKind::NotFound => "not_found",
        std::io::ErrorKind::PermissionDenied => "permission_denied",
        std::io::ErrorKind::TimedOut => "timeout",
        std::io::ErrorKind::UnexpectedEof => "unexpected_eof",
        std::io::ErrorKind::BrokenPipe => "broken_pipe",
        std::io::ErrorKind::ConnectionReset => "connection_reset",
        std::io::ErrorKind::WouldBlock => "busy",
        _ => "io",
    };
    protocol_failure(stage, kind, error.raw_os_error(), error.to_string())
}

fn protocol_failure(
    stage: &'static str,
    kind: &'static str,
    os_error: Option<i32>,
    detail: impl Into<String>,
) -> ProtocolFailure {
    ProtocolFailure {
        stage,
        kind,
        os_error,
        detail: detail.into(),
    }
}

#[cfg(windows)]
struct PipeHandle {
    handle: windows::Win32::Foundation::HANDLE,
}

#[cfg(windows)]
unsafe impl Send for PipeHandle {}
#[cfg(windows)]
unsafe impl Sync for PipeHandle {}

#[cfg(windows)]
impl Drop for PipeHandle {
    fn drop(&mut self) {
        unsafe {
            let _ = windows::Win32::Foundation::CloseHandle(self.handle);
        }
    }
}

#[cfg(windows)]
#[derive(Clone)]
struct PipeStream {
    inner: Arc<PipeHandle>,
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
                Ok(handle) => {
                    return Ok(Self {
                        inner: Arc::new(PipeHandle { handle }),
                    });
                }
                Err(_) => {
                    let error = unsafe { GetLastError() };
                    if error == ERROR_PIPE_BUSY && Instant::now() < deadline {
                        unsafe {
                            let _ = WaitNamedPipeW(PCWSTR(name.as_ptr()), 50);
                        }
                        continue;
                    }
                    if error == ERROR_FILE_NOT_FOUND && Instant::now() < deadline {
                        std::thread::sleep(Duration::from_millis(20));
                        continue;
                    }
                    return Err(std::io::Error::from_raw_os_error(error.0 as i32));
                }
            }
        }
    }

    fn cancel_io(&self) {
        unsafe {
            let _ = windows::Win32::System::IO::CancelIoEx(self.inner.handle, None);
        }
    }
}

#[cfg(not(windows))]
#[derive(Clone)]
struct PipeStream;

#[cfg(not(windows))]
impl PipeStream {
    fn connect(_pipe_name: &str, _timeout: Duration) -> Result<Self, std::io::Error> {
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "Windows named pipes are unavailable",
        ))
    }

    fn cancel_io(&self) {}
}

#[cfg(windows)]
impl Read for PipeStream {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        use windows::Win32::Storage::FileSystem::ReadFile;
        let mut read = 0_u32;
        unsafe { ReadFile(self.inner.handle, Some(buffer), Some(&mut read), None) }
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
        unsafe { WriteFile(self.inner.handle, Some(buffer), Some(&mut written), None) }
            .map_err(|_| std::io::Error::last_os_error())?;
        Ok(written as usize)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        use windows::Win32::Storage::FileSystem::FlushFileBuffers;
        unsafe { FlushFileBuffers(self.inner.handle) }.map_err(|_| std::io::Error::last_os_error())
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn responses_are_routed_by_id_even_when_they_arrive_out_of_order() {
        let pending = PendingRequests::default();
        let broken = AtomicBool::new(false);
        let (first_tx, first_rx) = mpsc::sync_channel(1);
        let (second_tx, second_rx) = mpsc::sync_channel(1);
        pending.register(10, first_tx, &broken).unwrap();
        pending.register(20, second_tx, &broken).unwrap();

        assert!(pending.resolve(
            20,
            ServerMessage::Thumbnail {
                id: 20,
                response: ThumbnailResponse::Success {
                    webp_bytes: vec![2]
                }
            }
        ));
        assert!(pending.resolve(
            10,
            ServerMessage::Thumbnail {
                id: 10,
                response: ThumbnailResponse::Success {
                    webp_bytes: vec![1]
                }
            }
        ));
        assert!(matches!(
            first_rx.recv().unwrap(),
            Ok(ServerMessage::Thumbnail {
                response: ThumbnailResponse::Success { webp_bytes }, ..
            }) if webp_bytes == vec![1]
        ));
        assert!(matches!(
            second_rx.recv().unwrap(),
            Ok(ServerMessage::Thumbnail {
                response: ThumbnailResponse::Success { webp_bytes }, ..
            }) if webp_bytes == vec![2]
        ));
    }

    #[test]
    fn connection_loss_is_retried_and_the_next_connection_can_succeed() {
        let mut calls = 0;
        let result = run_with_retry(|| {
            calls += 1;
            if calls == 1 {
                Err(ClientError::Protocol(protocol_failure(
                    "response_read",
                    "broken_pipe",
                    Some(109),
                    "pipe ended",
                )))
            } else {
                Ok(7)
            }
        })
        .unwrap();
        assert_eq!(result.value, 7);
        assert_eq!(result.retry_count, 1);
        assert_eq!(result.retry_statuses, ["response_read_broken_pipe"]);
        assert_eq!(calls, 2);
    }

    #[test]
    fn permanent_remote_errors_are_not_retried() {
        let mut calls = 0;
        let result = run_with_retry(|| {
            calls += 1;
            Err::<(), _>(ClientError::Remote(ThumbnailError::new(
                ThumbnailErrorCode::NotFound,
                "missing",
            )))
        });
        assert!(result.is_err());
        assert_eq!(calls, 1);
    }

    #[cfg(windows)]
    #[test]
    #[ignore = "managed test sandbox denies local named-pipe connections (os error 5)"]
    fn one_pipe_routes_two_concurrent_requests_with_reversed_responses() {
        use windows::Win32::Foundation::{CloseHandle, ERROR_PIPE_CONNECTED, GetLastError, HANDLE};
        use windows::Win32::Storage::FileSystem::{
            FlushFileBuffers, PIPE_ACCESS_DUPLEX, ReadFile, WriteFile,
        };
        use windows::Win32::System::Pipes::{
            ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, NAMED_PIPE_MODE,
            PIPE_READMODE_BYTE, PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE, PIPE_WAIT,
        };
        use windows::core::PCWSTR;

        struct TestServerStream(HANDLE);
        unsafe impl Send for TestServerStream {}
        impl Read for TestServerStream {
            fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
                let mut read = 0;
                unsafe { ReadFile(self.0, Some(buffer), Some(&mut read), None) }
                    .map_err(|_| std::io::Error::last_os_error())?;
                Ok(read as usize)
            }
        }
        impl Write for TestServerStream {
            fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
                let mut written = 0;
                unsafe { WriteFile(self.0, Some(buffer), Some(&mut written), None) }
                    .map_err(|_| std::io::Error::last_os_error())?;
                Ok(written as usize)
            }
            fn flush(&mut self) -> std::io::Result<()> {
                unsafe { FlushFileBuffers(self.0) }.map_err(|_| std::io::Error::last_os_error())
            }
        }
        impl Drop for TestServerStream {
            fn drop(&mut self) {
                unsafe {
                    let _ = DisconnectNamedPipe(self.0);
                    let _ = CloseHandle(self.0);
                }
            }
        }

        let pipe_name = format!(
            r"\\.\pipe\mimageviewer-remote-test-{}-{}",
            std::process::id(),
            Instant::now().elapsed().as_nanos()
        );
        let server_name = pipe_name.clone();
        let (ready_tx, ready_rx) = mpsc::sync_channel(1);
        let server = std::thread::spawn(move || {
            let wide: Vec<u16> = server_name.encode_utf16().chain([0]).collect();
            let mode = NAMED_PIPE_MODE(
                PIPE_TYPE_BYTE.0
                    | PIPE_READMODE_BYTE.0
                    | PIPE_WAIT.0
                    | PIPE_REJECT_REMOTE_CLIENTS.0,
            );
            let handle = unsafe {
                CreateNamedPipeW(
                    PCWSTR(wide.as_ptr()),
                    PIPE_ACCESS_DUPLEX,
                    mode,
                    1,
                    64 * 1024,
                    64 * 1024,
                    0,
                    None,
                )
            };
            if handle.is_invalid() {
                let _ = ready_tx.send(Err(std::io::Error::last_os_error()));
                return;
            }
            let _ = ready_tx.send(Ok(()));
            let connected = unsafe {
                ConnectNamedPipe(handle, None).is_ok() || GetLastError() == ERROR_PIPE_CONNECTED
            };
            assert!(connected);
            let mut pipe = TestServerStream(handle);
            let hello: ClientHello = read_frame(&mut pipe, MAX_CONTROL_FRAME_BYTES).unwrap();
            write_frame(
                &mut pipe,
                &mimageviewer_ipc::negotiate(hello.protocol_version),
            )
            .unwrap();
            let first: ClientMessage = read_frame(&mut pipe, MAX_CONTROL_FRAME_BYTES).unwrap();
            let second: ClientMessage = read_frame(&mut pipe, MAX_CONTROL_FRAME_BYTES).unwrap();
            for message in [second, first] {
                let ClientMessage::Thumbnail { id, request } = message else {
                    panic!("unexpected request type")
                };
                let marker = request.relative_path.as_bytes()[0];
                write_frame(
                    &mut pipe,
                    &ServerMessage::Thumbnail {
                        id,
                        response: ThumbnailResponse::Success {
                            webp_bytes: vec![marker],
                        },
                    },
                )
                .unwrap();
            }
        });
        ready_rx
            .recv_timeout(Duration::from_secs(2))
            .unwrap()
            .unwrap();

        let client = Arc::new(ThumbnailClient {
            pipe_name,
            connection: Mutex::new(None),
            next_request_id: AtomicU64::new(1),
            next_connection_id: AtomicU64::new(1),
        });
        let first_client = Arc::clone(&client);
        let first = std::thread::spawn(move || {
            first_client
                .thumbnail("00000000-0000-0000-0000-000000000000", "a.jpg", 128)
                .map(|result| result.bytes)
                .map_err(|failure| failure.error.to_string())
        });
        let second_client = Arc::clone(&client);
        let second = std::thread::spawn(move || {
            second_client
                .thumbnail("00000000-0000-0000-0000-000000000000", "b.jpg", 128)
                .map(|result| result.bytes)
                .map_err(|failure| failure.error.to_string())
        });
        assert_eq!(first.join().unwrap().unwrap(), vec![b'a']);
        assert_eq!(second.join().unwrap().unwrap(), vec![b'b']);
        server.join().unwrap();
    }
}
