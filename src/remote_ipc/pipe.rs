use std::io::{Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;

use mimageviewer_ipc::{
    ClientHello, ClientMessage, CollectionError, CollectionErrorCode, CollectionResponse,
    HomeResponse, MAX_CONTROL_FRAME_BYTES, PIPE_NAME, PROTOCOL_VERSION, ServerMessage,
    ThumbnailError, ThumbnailErrorCode, ThumbnailResponse, negotiate, read_frame, write_frame,
};
use windows::Win32::Foundation::{
    CloseHandle, ERROR_FILE_NOT_FOUND, ERROR_PIPE_BUSY, ERROR_PIPE_CONNECTED, GetLastError, HANDLE,
};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_FLAG_FIRST_PIPE_INSTANCE, FILE_FLAGS_AND_ATTRIBUTES,
    FILE_GENERIC_READ, FILE_GENERIC_WRITE, FILE_SHARE_READ, FILE_SHARE_WRITE, FlushFileBuffers,
    OPEN_EXISTING, PIPE_ACCESS_DUPLEX, ReadFile, WriteFile,
};
use windows::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, NAMED_PIPE_MODE, PIPE_READMODE_BYTE,
    PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE, PIPE_WAIT, WaitNamedPipeW,
};
use windows::core::PCWSTR;

use super::collections::CollectionEngine;
use super::thumbnail::{ThumbnailEngine, WorkerContext};

const PIPE_BUFFER_BYTES: u32 = 64 * 1024;
const PIPE_MAX_INSTANCES: u32 = 16;
/// 再接続が集中しても待機中 instance を切らさないため、常時この本数を accept 待ちにする。
const ACCEPTOR_COUNT: usize = 4;
const HEAVY_WORK_QUEUE_CAPACITY: usize = 16;
const HOME_WORK_QUEUE_CAPACITY: usize = 8;

enum Work {
    Request {
        message: ClientMessage,
        reply: mpsc::Sender<ServerMessage>,
    },
    Stop,
}

pub(super) struct ServerGuard {
    stop: Arc<AtomicBool>,
    listeners: Vec<std::thread::JoinHandle<()>>,
    workers: Vec<std::thread::JoinHandle<()>>,
    home_worker: Option<std::thread::JoinHandle<()>>,
    heavy_work_tx: mpsc::SyncSender<Work>,
    home_work_tx: mpsc::SyncSender<Work>,
}

impl ServerGuard {
    pub(super) fn start(settings: crate::settings::Settings) -> Result<Self, String> {
        // 最初の instance は同名サーバの二重起動検出も兼ねる。他の instance も
        // listener 開始前に作り、起動完了時点で複数本が必ず待機できる形にする。
        let mut initial_pipes = Vec::with_capacity(ACCEPTOR_COUNT);
        initial_pipes.push(create_server_pipe(true).map_err(|error| {
            format!(
                "remote IPC pipe を作成できません。同名サーバが既に存在する可能性があります: {error}"
            )
        })?);
        for _ in 1..ACCEPTOR_COUNT {
            initial_pipes.push(
                create_server_pipe(false).map_err(|error| {
                    format!("remote IPC pipe instance を作成できません: {error}")
                })?,
            );
        }

        let configured_worker_count = settings.parallelism.thread_count();
        let worker_count = remote_heavy_worker_count(configured_worker_count);
        let thumbnail_engine = Arc::new(ThumbnailEngine::new(settings.clone()));
        let collection_engine = Arc::new(CollectionEngine::new(settings));
        let (heavy_work_tx, heavy_work_rx) = mpsc::sync_channel::<Work>(HEAVY_WORK_QUEUE_CAPACITY);
        let heavy_work_rx = Arc::new(Mutex::new(heavy_work_rx));
        let (home_work_tx, home_work_rx) = mpsc::sync_channel::<Work>(HOME_WORK_QUEUE_CAPACITY);
        let home_collection_engine = Arc::clone(&collection_engine);
        let home_worker = std::thread::Builder::new()
            .name("remote-home".to_owned())
            .spawn(move || home_worker_loop(home_work_rx, &home_collection_engine))
            .map_err(|error| format!("remote IPC home worker を開始できません: {error}"))?;
        let mut workers = Vec::with_capacity(worker_count);
        for index in 0..worker_count {
            let work_rx = Arc::clone(&heavy_work_rx);
            let thumbnail_engine = Arc::clone(&thumbnail_engine);
            let collection_engine = Arc::clone(&collection_engine);
            workers.push(
                std::thread::Builder::new()
                    .name(format!("remote-thumb-{index}"))
                    .spawn(move || worker_loop(&work_rx, &thumbnail_engine, &collection_engine))
                    .map_err(|error| format!("remote IPC worker を開始できません: {error}"))?,
            );
        }

        let stop = Arc::new(AtomicBool::new(false));
        let mut listeners = Vec::with_capacity(ACCEPTOR_COUNT);
        for (index, initial_pipe) in initial_pipes.into_iter().enumerate() {
            let listener_stop = Arc::clone(&stop);
            let listener_heavy_tx = heavy_work_tx.clone();
            let listener_home_tx = home_work_tx.clone();
            match std::thread::Builder::new()
                .name(format!("remote-ipc-listener-{index}"))
                .spawn(move || {
                    acceptor_loop(
                        listener_stop,
                        listener_heavy_tx,
                        listener_home_tx,
                        initial_pipe,
                        index,
                    )
                }) {
                Ok(listener) => listeners.push(listener),
                Err(error) => {
                    stop.store(true, Ordering::Release);
                    for _ in 0..listeners.len() {
                        poke_listener();
                    }
                    for listener in listeners {
                        let _ = listener.join();
                    }
                    for _ in 0..worker_count {
                        let _ = heavy_work_tx.send(Work::Stop);
                    }
                    let _ = home_work_tx.send(Work::Stop);
                    for worker in workers {
                        let _ = worker.join();
                    }
                    let _ = home_worker.join();
                    return Err(format!("remote IPC listener を開始できません: {error}"));
                }
            }
        }

        crate::logger::log(format!(
            "remote_ipc: listening pipe={PIPE_NAME} protocol={PROTOCOL_VERSION} heavy_workers={worker_count} configured_workers={configured_worker_count} home_workers=1 acceptors={ACCEPTOR_COUNT} multiplexed=true"
        ));
        Ok(Self {
            stop,
            listeners,
            workers,
            home_worker: Some(home_worker),
            heavy_work_tx,
            home_work_tx,
        })
    }
}

impl Drop for ServerGuard {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        for _ in 0..self.listeners.len() {
            poke_listener();
        }
        for listener in self.listeners.drain(..) {
            let _ = listener.join();
        }
        for _ in 0..self.workers.len() {
            let _ = self.heavy_work_tx.send(Work::Stop);
        }
        let _ = self.home_work_tx.send(Work::Stop);
        for worker in self.workers.drain(..) {
            let _ = worker.join();
        }
        if let Some(worker) = self.home_worker.take() {
            let _ = worker.join();
        }
        crate::logger::log("remote_ipc: stopped".to_owned());
    }
}

fn remote_heavy_worker_count(configured_worker_count: usize) -> usize {
    // IPC decode がローカル表示用 worker と CPU / disk を奪い合わないよう、
    // 利用者設定の半分かつ最大 2 本だけを remote 専用にする。
    (configured_worker_count / 2).clamp(1, 2)
}

fn worker_loop(
    work_rx: &Mutex<mpsc::Receiver<Work>>,
    thumbnail_engine: &ThumbnailEngine,
    collection_engine: &CollectionEngine,
) {
    let context = WorkerContext::open();
    loop {
        let work = {
            let receiver = work_rx.lock().unwrap_or_else(|error| error.into_inner());
            receiver.recv()
        };
        match work {
            Ok(Work::Request { message, reply }) => {
                let response = match message {
                    ClientMessage::Thumbnail { id, request } => ServerMessage::Thumbnail {
                        id,
                        response: thumbnail_engine.handle(request, &context),
                    },
                    ClientMessage::Home { id, .. } => ServerMessage::Home {
                        id,
                        response: collection_engine.home(),
                    },
                    ClientMessage::Collection { id, request } => ServerMessage::Collection {
                        id,
                        response: collection_engine.collection(request),
                    },
                };
                let _ = reply.send(response);
            }
            Ok(Work::Stop) | Err(_) => break,
        }
    }
}

fn home_worker_loop(work_rx: mpsc::Receiver<Work>, collection_engine: &CollectionEngine) {
    loop {
        match work_rx.recv() {
            Ok(Work::Request { message, reply }) => {
                let response = match message {
                    ClientMessage::Home { id, .. } => ServerMessage::Home {
                        id,
                        response: collection_engine.home(),
                    },
                    other => service_stopped_response(&other),
                };
                let _ = reply.send(response);
            }
            Ok(Work::Stop) | Err(_) => break,
        }
    }
}

fn acceptor_loop(
    stop: Arc<AtomicBool>,
    heavy_work_tx: mpsc::SyncSender<Work>,
    home_work_tx: mpsc::SyncSender<Work>,
    initial_pipe: PipeStream,
    index: usize,
) {
    let mut next = Some(initial_pipe);
    loop {
        if stop.load(Ordering::Acquire) {
            break;
        }
        let pipe = match next.take() {
            Some(pipe) => pipe,
            None => match create_server_pipe(false) {
                Ok(pipe) => pipe,
                Err(error) => {
                    crate::logger::log(format!(
                        "remote_ipc: stage=accept_create acceptor={index} error={error}"
                    ));
                    break;
                }
            },
        };
        let connected = unsafe {
            match ConnectNamedPipe(pipe.handle(), None) {
                Ok(()) => true,
                Err(_) if GetLastError() == ERROR_PIPE_CONNECTED => true,
                Err(error) => {
                    crate::logger::log(format!(
                        "remote_ipc: stage=accept_connect acceptor={index} os_error={:?} error={error}",
                        GetLastError()
                    ));
                    false
                }
            }
        };
        if !connected {
            continue;
        }
        if stop.load(Ordering::Acquire) {
            break;
        }
        let heavy_work_tx = heavy_work_tx.clone();
        let home_work_tx = home_work_tx.clone();
        if let Err(error) = std::thread::Builder::new()
            .name("remote-ipc-connection".to_owned())
            .spawn(move || handle_connection(pipe, heavy_work_tx, home_work_tx))
        {
            crate::logger::log(format!("remote_ipc: stage=connection_spawn error={error}"));
        }
        // この接続を処理する thread を起こした直後に次の instance を作る。
        // 他の acceptor も並行して待機しているため、再接続 burst に空白を作らない。
    }
    crate::logger::log(format!("remote_ipc: listener exiting acceptor={index}"));
}

fn handle_connection(
    mut pipe: PipeStream,
    heavy_work_tx: mpsc::SyncSender<Work>,
    home_work_tx: mpsc::SyncSender<Work>,
) {
    let hello: ClientHello = match read_frame(&mut pipe, MAX_CONTROL_FRAME_BYTES) {
        Ok(hello) => hello,
        Err(error) => {
            crate::logger::log(format!(
                "remote_ipc: stage=handshake_read error_kind={} error={error}",
                frame_error_kind(&error)
            ));
            return;
        }
    };
    let response = negotiate(hello.protocol_version);
    if let Err(error) = write_frame(&mut pipe, &response) {
        crate::logger::log(format!(
            "remote_ipc: stage=handshake_write error_kind={} error={error}",
            frame_error_kind(&error)
        ));
        return;
    }
    if !response.accepted {
        crate::logger::log(format!(
            "remote_ipc: protocol mismatch rejected client={} server={}",
            hello.protocol_version, response.protocol_version
        ));
        return;
    }

    let (reply_tx, reply_rx) = mpsc::channel::<ServerMessage>();
    let mut response_pipe = pipe.clone();
    let writer = match std::thread::Builder::new()
        .name("remote-ipc-writer".to_owned())
        .spawn(move || {
            for response in reply_rx {
                if let Err(error) = write_frame(&mut response_pipe, &response) {
                    crate::logger::log(format!(
                        "remote_ipc: stage=response_write error_kind={} error={error}",
                        frame_error_kind(&error)
                    ));
                    break;
                }
            }
        }) {
        Ok(writer) => writer,
        Err(error) => {
            crate::logger::log(format!("remote_ipc: stage=writer_spawn error={error}"));
            return;
        }
    };

    loop {
        let message: ClientMessage = match read_frame(&mut pipe, MAX_CONTROL_FRAME_BYTES) {
            Ok(message) => message,
            Err(mimageviewer_ipc::FrameError::Io(error))
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::UnexpectedEof
                        | std::io::ErrorKind::BrokenPipe
                        | std::io::ErrorKind::ConnectionReset
                ) =>
            {
                break;
            }
            Err(error) => {
                crate::logger::log(format!(
                    "remote_ipc: stage=request_read error_kind={} error={error}",
                    frame_error_kind(&error)
                ));
                break;
            }
        };
        // Home は専用 worker へ分離する。重い queue が満杯でも connection reader を
        // 塞がず、後続 Home を読めるよう Busy を明示応答する。
        let work_tx = if matches!(message, ClientMessage::Home { .. }) {
            &home_work_tx
        } else {
            &heavy_work_tx
        };
        match work_tx.try_send(Work::Request {
            message,
            reply: reply_tx.clone(),
        }) {
            Ok(()) => {}
            Err(mpsc::TrySendError::Full(Work::Request { message, reply })) => {
                let _ = reply.send(queue_busy_response(&message));
            }
            Err(mpsc::TrySendError::Disconnected(Work::Request { message, reply })) => {
                let _ = reply.send(service_stopped_response(&message));
                break;
            }
            Err(mpsc::TrySendError::Full(Work::Stop)) => unreachable!(),
            Err(mpsc::TrySendError::Disconnected(Work::Stop)) => unreachable!(),
        }
    }
    drop(reply_tx);
    let _ = writer.join();
}

fn service_stopped_response(message: &ClientMessage) -> ServerMessage {
    match message {
        ClientMessage::Thumbnail { id, .. } => ServerMessage::Thumbnail {
            id: *id,
            response: ThumbnailResponse::Error(ThumbnailError::new(
                ThumbnailErrorCode::Internal,
                "mIV 本体のサムネイルワーカーが停止しています",
            )),
        },
        ClientMessage::Home { id, .. } => ServerMessage::Home {
            id: *id,
            response: HomeResponse::Error(CollectionError::new(
                CollectionErrorCode::Internal,
                "mIV 本体の集約ビューワーカーが停止しています",
            )),
        },
        ClientMessage::Collection { id, .. } => ServerMessage::Collection {
            id: *id,
            response: CollectionResponse::Error(CollectionError::new(
                CollectionErrorCode::Internal,
                "mIV 本体の集約ビューワーカーが停止しています",
            )),
        },
    }
}

fn queue_busy_response(message: &ClientMessage) -> ServerMessage {
    match message {
        ClientMessage::Thumbnail { id, .. } => ServerMessage::Thumbnail {
            id: *id,
            response: ThumbnailResponse::Error(ThumbnailError::new(
                ThumbnailErrorCode::Busy,
                "mIV 本体のリモートサムネイル queue が混み合っています",
            )),
        },
        ClientMessage::Home { id, .. } => ServerMessage::Home {
            id: *id,
            response: HomeResponse::Error(CollectionError::new(
                CollectionErrorCode::Busy,
                "mIV 本体のリモートホーム queue が混み合っています",
            )),
        },
        ClientMessage::Collection { id, .. } => ServerMessage::Collection {
            id: *id,
            response: CollectionResponse::Error(CollectionError::new(
                CollectionErrorCode::Busy,
                "mIV 本体のリモート集約ビュー queue が混み合っています",
            )),
        },
    }
}

fn frame_error_kind(error: &mimageviewer_ipc::FrameError) -> &'static str {
    match error {
        mimageviewer_ipc::FrameError::Io(error) => match error.kind() {
            std::io::ErrorKind::UnexpectedEof => "unexpected_eof",
            std::io::ErrorKind::BrokenPipe => "broken_pipe",
            std::io::ErrorKind::ConnectionReset => "connection_reset",
            std::io::ErrorKind::TimedOut => "timed_out",
            _ => "io",
        },
        mimageviewer_ipc::FrameError::TooLarge { .. } => "too_large",
        mimageviewer_ipc::FrameError::Encode(_) => "encode",
        mimageviewer_ipc::FrameError::Decode(_) => "decode",
    }
}

fn create_server_pipe(first: bool) -> Result<PipeStream, std::io::Error> {
    let name = wide_nul(PIPE_NAME);
    let mut access = PIPE_ACCESS_DUPLEX;
    if first {
        access = FILE_FLAGS_AND_ATTRIBUTES(access.0 | FILE_FLAG_FIRST_PIPE_INSTANCE.0);
    }
    let mode = NAMED_PIPE_MODE(
        PIPE_TYPE_BYTE.0 | PIPE_READMODE_BYTE.0 | PIPE_WAIT.0 | PIPE_REJECT_REMOTE_CLIENTS.0,
    );
    let handle = unsafe {
        CreateNamedPipeW(
            PCWSTR(name.as_ptr()),
            access,
            mode,
            PIPE_MAX_INSTANCES,
            PIPE_BUFFER_BYTES,
            PIPE_BUFFER_BYTES,
            0,
            None,
        )
    };
    if handle.is_invalid() {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(PipeStream::new(handle, true))
    }
}

fn poke_listener() {
    let _ = open_client_pipe(Duration::from_millis(100));
}

fn open_client_pipe(timeout: Duration) -> Result<PipeStream, std::io::Error> {
    let name = wide_nul(PIPE_NAME);
    let deadline = std::time::Instant::now() + timeout;
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
            Ok(handle) => return Ok(PipeStream::new(handle, false)),
            Err(_) => {
                let error = unsafe { GetLastError() };
                if error == ERROR_PIPE_BUSY && std::time::Instant::now() < deadline {
                    unsafe {
                        let _ = WaitNamedPipeW(PCWSTR(name.as_ptr()), 20);
                    }
                    continue;
                }
                if error == ERROR_FILE_NOT_FOUND && std::time::Instant::now() < deadline {
                    std::thread::sleep(Duration::from_millis(10));
                    continue;
                }
                return Err(std::io::Error::last_os_error());
            }
        }
    }
}

struct PipeHandle {
    handle: HANDLE,
    server_side: bool,
}

unsafe impl Send for PipeHandle {}
unsafe impl Sync for PipeHandle {}

impl Drop for PipeHandle {
    fn drop(&mut self) {
        unsafe {
            if self.server_side {
                let _ = DisconnectNamedPipe(self.handle);
            }
            let _ = CloseHandle(self.handle);
        }
    }
}

#[derive(Clone)]
struct PipeStream {
    inner: Arc<PipeHandle>,
}

impl PipeStream {
    fn new(handle: HANDLE, server_side: bool) -> Self {
        Self {
            inner: Arc::new(PipeHandle {
                handle,
                server_side,
            }),
        }
    }

    fn handle(&self) -> HANDLE {
        self.inner.handle
    }
}

impl Read for PipeStream {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        let mut read = 0_u32;
        unsafe { ReadFile(self.handle(), Some(buffer), Some(&mut read), None) }
            .map_err(|_| std::io::Error::last_os_error())?;
        Ok(read as usize)
    }
}

impl Write for PipeStream {
    fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
        let mut written = 0_u32;
        unsafe { WriteFile(self.handle(), Some(buffer), Some(&mut written), None) }
            .map_err(|_| std::io::Error::last_os_error())?;
        Ok(written as usize)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        unsafe { FlushFileBuffers(self.handle()) }.map_err(|_| std::io::Error::last_os_error())
    }
}

fn wide_nul(value: &str) -> Vec<u16> {
    value.encode_utf16().chain([0]).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_heavy_queue_rejects_immediately_and_home_lane_remains_available() {
        let (heavy_tx, _heavy_rx) = mpsc::sync_channel(1);
        heavy_tx.try_send(Work::Stop).unwrap();
        let started = std::time::Instant::now();
        assert!(matches!(
            heavy_tx.try_send(Work::Stop),
            Err(mpsc::TrySendError::Full(Work::Stop))
        ));
        assert!(started.elapsed() < Duration::from_millis(50));

        let (home_tx, home_rx) = mpsc::sync_channel(1);
        home_tx.try_send(Work::Stop).unwrap();
        assert!(matches!(home_rx.try_recv(), Ok(Work::Stop)));
    }

    #[test]
    fn queue_full_is_an_explicit_retryable_response() {
        let message = ClientMessage::Thumbnail {
            id: 42,
            request: mimageviewer_ipc::ThumbnailRequest {
                favorite_id: "00000000-0000-0000-0000-000000000000".to_owned(),
                relative_path: "page.jpg".to_owned(),
                target_px: 256,
            },
        };
        assert!(matches!(
            queue_busy_response(&message),
            ServerMessage::Thumbnail {
                id: 42,
                response: ThumbnailResponse::Error(ThumbnailError {
                    code: ThumbnailErrorCode::Busy,
                    ..
                }),
            }
        ));
    }

    #[test]
    fn remote_workers_leave_capacity_for_local_operations() {
        assert_eq!(remote_heavy_worker_count(1), 1);
        assert_eq!(remote_heavy_worker_count(2), 1);
        assert_eq!(remote_heavy_worker_count(4), 2);
        assert_eq!(remote_heavy_worker_count(8), 2);
        assert_eq!(remote_heavy_worker_count(64), 2);
    }
}
