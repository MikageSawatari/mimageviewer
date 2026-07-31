use std::io::{Read, Write};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

use mimageviewer_ipc::{
    ClientHello, ClientMessage, CollectionError, CollectionErrorCode, CollectionResponse,
    HomeResponse, MAX_CONTROL_FRAME_BYTES, PIPE_NAME, PROTOCOL_VERSION, ServerMessage,
    ThumbnailError, ThumbnailErrorCode, ThumbnailResponse, negotiate, read_frame, write_frame,
};
use windows::Win32::Foundation::{
    CloseHandle, ERROR_FILE_NOT_FOUND, ERROR_IO_PENDING, ERROR_PIPE_BUSY, ERROR_PIPE_CONNECTED,
    GetLastError, HANDLE,
};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_FLAG_FIRST_PIPE_INSTANCE, FILE_FLAG_OVERLAPPED,
    FILE_FLAGS_AND_ATTRIBUTES, FILE_GENERIC_READ, FILE_GENERIC_WRITE, FILE_SHARE_READ,
    FILE_SHARE_WRITE, OPEN_EXISTING, PIPE_ACCESS_DUPLEX, ReadFile, WriteFile,
};
use windows::Win32::System::IO::{GetOverlappedResult, OVERLAPPED};
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
        enqueued_at: Instant,
    },
    Stop,
}

struct QueueMetrics {
    name: &'static str,
    queued: AtomicUsize,
    active: AtomicUsize,
}

impl QueueMetrics {
    fn new(name: &'static str) -> Self {
        Self {
            name,
            queued: AtomicUsize::new(0),
            active: AtomicUsize::new(0),
        }
    }

    fn reserve(&self) {
        self.queued.fetch_add(1, Ordering::AcqRel);
    }

    fn rollback(&self) {
        self.queued.fetch_sub(1, Ordering::AcqRel);
    }

    fn started(&self) -> (usize, usize) {
        let queued = self.queued.fetch_sub(1, Ordering::AcqRel) - 1;
        let active = self.active.fetch_add(1, Ordering::AcqRel) + 1;
        (queued, active)
    }

    fn finished(&self) -> (usize, usize) {
        let active = self.active.fetch_sub(1, Ordering::AcqRel) - 1;
        (self.queued.load(Ordering::Acquire), active)
    }

    fn snapshot(&self) -> (usize, usize) {
        (
            self.queued.load(Ordering::Acquire),
            self.active.load(Ordering::Acquire),
        )
    }
}

struct ConnectionLifecycle {
    id: u64,
    started_at: Instant,
}

impl Drop for ConnectionLifecycle {
    fn drop(&mut self) {
        crate::logger::log(format!(
            "remote_ipc: connection_disconnected connection_id={} duration_ms={:.1}",
            self.id,
            self.started_at.elapsed().as_secs_f64() * 1000.0
        ));
    }
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
        let heavy_metrics = Arc::new(QueueMetrics::new("heavy"));
        let home_metrics = Arc::new(QueueMetrics::new("home"));
        let home_collection_engine = Arc::clone(&collection_engine);
        let home_worker_metrics = Arc::clone(&home_metrics);
        let home_worker = std::thread::Builder::new()
            .name("remote-home".to_owned())
            .spawn(move || {
                home_worker_loop(home_work_rx, &home_collection_engine, &home_worker_metrics)
            })
            .map_err(|error| format!("remote IPC home worker を開始できません: {error}"))?;
        let mut workers = Vec::with_capacity(worker_count);
        for index in 0..worker_count {
            let work_rx = Arc::clone(&heavy_work_rx);
            let thumbnail_engine = Arc::clone(&thumbnail_engine);
            let collection_engine = Arc::clone(&collection_engine);
            let worker_metrics = Arc::clone(&heavy_metrics);
            workers.push(
                std::thread::Builder::new()
                    .name(format!("remote-thumb-{index}"))
                    .spawn(move || {
                        worker_loop(
                            &work_rx,
                            &thumbnail_engine,
                            &collection_engine,
                            &worker_metrics,
                            index,
                        )
                    })
                    .map_err(|error| format!("remote IPC worker を開始できません: {error}"))?,
            );
        }

        let stop = Arc::new(AtomicBool::new(false));
        let next_connection_id = Arc::new(AtomicU64::new(1));
        let mut listeners = Vec::with_capacity(ACCEPTOR_COUNT);
        for (index, initial_pipe) in initial_pipes.into_iter().enumerate() {
            let listener_stop = Arc::clone(&stop);
            let listener_heavy_tx = heavy_work_tx.clone();
            let listener_home_tx = home_work_tx.clone();
            let listener_heavy_metrics = Arc::clone(&heavy_metrics);
            let listener_home_metrics = Arc::clone(&home_metrics);
            let listener_next_connection_id = Arc::clone(&next_connection_id);
            match std::thread::Builder::new()
                .name(format!("remote-ipc-listener-{index}"))
                .spawn(move || {
                    acceptor_loop(
                        listener_stop,
                        listener_heavy_tx,
                        listener_home_tx,
                        listener_heavy_metrics,
                        listener_home_metrics,
                        listener_next_connection_id,
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
    metrics: &QueueMetrics,
    worker_index: usize,
) {
    crate::logger::log(format!(
        "remote_ipc: worker_started queue={} worker={worker_index}",
        metrics.name
    ));
    let context = WorkerContext::open();
    loop {
        let work = {
            let receiver = work_rx.lock().unwrap_or_else(|error| error.into_inner());
            receiver.recv()
        };
        match work {
            Ok(Work::Request {
                message,
                reply,
                enqueued_at,
            }) => execute_work(
                message,
                reply,
                enqueued_at,
                metrics,
                &format!("heavy-{worker_index}"),
                |message| match message {
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
                },
            ),
            Ok(Work::Stop) | Err(_) => break,
        }
    }
    crate::logger::log(format!(
        "remote_ipc: worker_stopped queue={} worker={worker_index}",
        metrics.name
    ));
}

fn home_worker_loop(
    work_rx: mpsc::Receiver<Work>,
    collection_engine: &CollectionEngine,
    metrics: &QueueMetrics,
) {
    crate::logger::log("remote_ipc: worker_started queue=home worker=home-0".to_owned());
    loop {
        match work_rx.recv() {
            Ok(Work::Request {
                message,
                reply,
                enqueued_at,
            }) => {
                execute_work(
                    message,
                    reply,
                    enqueued_at,
                    metrics,
                    "home-0",
                    |message| match message {
                        ClientMessage::Home { id, .. } => ServerMessage::Home {
                            id,
                            response: collection_engine.home(),
                        },
                        other => service_stopped_response(&other),
                    },
                )
            }
            Ok(Work::Stop) | Err(_) => break,
        }
    }
    crate::logger::log("remote_ipc: worker_stopped queue=home worker=home-0".to_owned());
}

fn execute_work(
    message: ClientMessage,
    reply: mpsc::Sender<ServerMessage>,
    enqueued_at: Instant,
    metrics: &QueueMetrics,
    worker: &str,
    handler: impl FnOnce(ClientMessage) -> ServerMessage,
) {
    let request_id = message.id();
    let request_kind = request_kind(&message);
    let (queued, active) = metrics.started();
    crate::logger::log(format!(
        "remote_ipc: worker_start request_id={request_id} kind={request_kind} queue={} worker={worker} queue_wait_ms={:.1} queued={queued} active={active}",
        metrics.name,
        enqueued_at.elapsed().as_secs_f64() * 1000.0
    ));
    let started_at = Instant::now();
    let response = handler(message);
    let outcome = response_outcome(&response);
    let reply_ok = reply.send(response).is_ok();
    let (queued, active) = metrics.finished();
    crate::logger::log(format!(
        "remote_ipc: worker_complete request_id={request_id} kind={request_kind} queue={} worker={worker} outcome={outcome} duration_ms={:.1} reply_ok={reply_ok} queued={queued} active={active}",
        metrics.name,
        started_at.elapsed().as_secs_f64() * 1000.0
    ));
}

fn request_kind(message: &ClientMessage) -> &'static str {
    match message {
        ClientMessage::Thumbnail { .. } => "thumbnail",
        ClientMessage::Home { .. } => "home",
        ClientMessage::Collection { .. } => "collection",
    }
}

fn response_outcome(response: &ServerMessage) -> &'static str {
    match response {
        ServerMessage::Thumbnail {
            response: ThumbnailResponse::Success { .. },
            ..
        }
        | ServerMessage::Home {
            response: HomeResponse::Success(_),
            ..
        }
        | ServerMessage::Collection {
            response: CollectionResponse::Success(_),
            ..
        } => "ok",
        ServerMessage::Thumbnail {
            response: ThumbnailResponse::Error(_),
            ..
        }
        | ServerMessage::Home {
            response: HomeResponse::Error(_),
            ..
        }
        | ServerMessage::Collection {
            response: CollectionResponse::Error(_),
            ..
        } => "error",
    }
}

fn acceptor_loop(
    stop: Arc<AtomicBool>,
    heavy_work_tx: mpsc::SyncSender<Work>,
    home_work_tx: mpsc::SyncSender<Work>,
    heavy_metrics: Arc<QueueMetrics>,
    home_metrics: Arc<QueueMetrics>,
    next_connection_id: Arc<AtomicU64>,
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
        let connected = match connect_server_pipe(&pipe) {
            Ok(()) => true,
            Err(error) => {
                crate::logger::log(format!(
                    "remote_ipc: stage=accept_connect acceptor={index} os_error={:?} error={error}",
                    error.raw_os_error()
                ));
                false
            }
        };
        if !connected {
            continue;
        }
        if stop.load(Ordering::Acquire) {
            break;
        }
        let connection_id = next_connection_id.fetch_add(1, Ordering::Relaxed);
        crate::logger::log(format!(
            "remote_ipc: connection_accepted connection_id={connection_id} acceptor={index}"
        ));
        let heavy_work_tx = heavy_work_tx.clone();
        let home_work_tx = home_work_tx.clone();
        let heavy_metrics = Arc::clone(&heavy_metrics);
        let home_metrics = Arc::clone(&home_metrics);
        if let Err(error) = std::thread::Builder::new()
            .name(format!("remote-ipc-connection-{connection_id}"))
            .spawn(move || {
                handle_connection(
                    connection_id,
                    pipe,
                    heavy_work_tx,
                    home_work_tx,
                    heavy_metrics,
                    home_metrics,
                )
            })
        {
            crate::logger::log(format!(
                "remote_ipc: stage=connection_spawn connection_id={connection_id} error={error}"
            ));
        }
        // この接続を処理する thread を起こした直後に次の instance を作る。
        // 他の acceptor も並行して待機しているため、再接続 burst に空白を作らない。
    }
    crate::logger::log(format!("remote_ipc: listener exiting acceptor={index}"));
}

fn handle_connection(
    connection_id: u64,
    mut pipe: PipeStream,
    heavy_work_tx: mpsc::SyncSender<Work>,
    home_work_tx: mpsc::SyncSender<Work>,
    heavy_metrics: Arc<QueueMetrics>,
    home_metrics: Arc<QueueMetrics>,
) {
    let _lifecycle = ConnectionLifecycle {
        id: connection_id,
        started_at: Instant::now(),
    };
    let hello: ClientHello = match read_frame(&mut pipe, MAX_CONTROL_FRAME_BYTES) {
        Ok(hello) => hello,
        Err(error) => {
            crate::logger::log(format!(
                "remote_ipc: stage=handshake_read connection_id={connection_id} error_kind={} error={error}",
                frame_error_kind(&error)
            ));
            return;
        }
    };
    let response = negotiate(hello.protocol_version);
    if let Err(error) = write_frame(&mut pipe, &response) {
        crate::logger::log(format!(
            "remote_ipc: stage=handshake_write connection_id={connection_id} error_kind={} error={error}",
            frame_error_kind(&error)
        ));
        return;
    }
    if !response.accepted {
        crate::logger::log(format!(
            "remote_ipc: protocol mismatch rejected connection_id={connection_id} client={} server={}",
            hello.protocol_version, response.protocol_version
        ));
        return;
    }
    crate::logger::log(format!(
        "remote_ipc: handshake_accepted connection_id={connection_id} protocol={}",
        response.protocol_version
    ));

    let (reply_tx, reply_rx) = mpsc::channel::<ServerMessage>();
    let mut response_pipe = pipe.clone();
    let writer = match std::thread::Builder::new()
        .name("remote-ipc-writer".to_owned())
        .spawn(move || {
            for response in reply_rx {
                if let Err(error) = write_frame(&mut response_pipe, &response) {
                    crate::logger::log(format!(
                        "remote_ipc: stage=response_write connection_id={connection_id} request_id={} error_kind={} error={error}",
                        response.id(),
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
                    "remote_ipc: stage=request_read connection_id={connection_id} error_kind={} error={error}",
                    frame_error_kind(&error)
                ));
                break;
            }
        };
        let request_id = message.id();
        let kind = request_kind(&message);
        crate::logger::log(format!(
            "remote_ipc: request_received connection_id={connection_id} request_id={request_id} kind={kind}"
        ));
        // Home は専用 worker へ分離する。重い queue が満杯でも connection reader を
        // 塞がず、後続 Home を読めるよう Busy を明示応答する。
        let (work_tx, metrics) = if matches!(message, ClientMessage::Home { .. }) {
            (&home_work_tx, &home_metrics)
        } else {
            (&heavy_work_tx, &heavy_metrics)
        };
        match enqueue_work(work_tx, metrics, message, reply_tx.clone()) {
            Ok(()) => {
                let (queued, active) = metrics.snapshot();
                crate::logger::log(format!(
                    "remote_ipc: request_enqueued connection_id={connection_id} request_id={request_id} kind={kind} queue={} queued={queued} active={active}",
                    metrics.name
                ));
            }
            Err(mpsc::TrySendError::Full(Work::Request { message, reply, .. })) => {
                let (queued, active) = metrics.snapshot();
                crate::logger::log(format!(
                    "remote_ipc: queue_full connection_id={connection_id} request_id={request_id} kind={kind} queue={} queued={queued} active={active}",
                    metrics.name
                ));
                let _ = reply.send(queue_busy_response(&message));
            }
            Err(mpsc::TrySendError::Disconnected(Work::Request { message, reply, .. })) => {
                let (queued, active) = metrics.snapshot();
                crate::logger::log(format!(
                    "remote_ipc: queue_stopped connection_id={connection_id} request_id={request_id} kind={kind} queue={} queued={queued} active={active}",
                    metrics.name
                ));
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

fn enqueue_work(
    work_tx: &mpsc::SyncSender<Work>,
    metrics: &QueueMetrics,
    message: ClientMessage,
    reply: mpsc::Sender<ServerMessage>,
) -> Result<(), mpsc::TrySendError<Work>> {
    metrics.reserve();
    let result = work_tx.try_send(Work::Request {
        message,
        reply,
        enqueued_at: Instant::now(),
    });
    if result.is_err() {
        metrics.rollback();
    }
    result
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
    let access = server_pipe_access(first);
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

fn server_pipe_access(first: bool) -> FILE_FLAGS_AND_ATTRIBUTES {
    let mut flags = PIPE_ACCESS_DUPLEX.0 | FILE_FLAG_OVERLAPPED.0;
    if first {
        flags |= FILE_FLAG_FIRST_PIPE_INSTANCE.0;
    }
    FILE_FLAGS_AND_ATTRIBUTES(flags)
}

fn connect_server_pipe(pipe: &PipeStream) -> Result<(), std::io::Error> {
    let event = OverlappedEvent::new()?;
    let mut overlapped = OVERLAPPED {
        hEvent: event.0,
        ..Default::default()
    };
    let started = unsafe { ConnectNamedPipe(pipe.handle(), Some(&mut overlapped)) };
    if started.is_err() {
        let error = unsafe { GetLastError() };
        if error == ERROR_PIPE_CONNECTED {
            return Ok(());
        }
        if error != ERROR_IO_PENDING {
            return Err(std::io::Error::from_raw_os_error(error.0 as i32));
        }
    }
    complete_overlapped(pipe.handle(), &overlapped, started).map(|_| ())
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

struct OverlappedEvent(HANDLE);

impl OverlappedEvent {
    fn new() -> std::io::Result<Self> {
        use windows::Win32::System::Threading::CreateEventW;

        unsafe { CreateEventW(None, true, false, PCWSTR::null()) }
            .map(Self)
            .map_err(|_| std::io::Error::last_os_error())
    }
}

impl Drop for OverlappedEvent {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

fn complete_overlapped(
    handle: HANDLE,
    overlapped: &OVERLAPPED,
    started: windows::core::Result<()>,
) -> std::io::Result<u32> {
    if started.is_err() {
        let error = unsafe { GetLastError() };
        if error != ERROR_IO_PENDING {
            return Err(std::io::Error::from_raw_os_error(error.0 as i32));
        }
    }
    let mut transferred = 0_u32;
    unsafe { GetOverlappedResult(handle, overlapped, &mut transferred, true) }
        .map_err(|_| std::io::Error::last_os_error())?;
    Ok(transferred)
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
        let event = OverlappedEvent::new()?;
        let mut overlapped = OVERLAPPED {
            hEvent: event.0,
            ..Default::default()
        };
        let started = unsafe { ReadFile(self.handle(), Some(buffer), None, Some(&mut overlapped)) };
        complete_overlapped(self.handle(), &overlapped, started).map(|read| read as usize)
    }
}

impl Write for PipeStream {
    fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
        let event = OverlappedEvent::new()?;
        let mut overlapped = OVERLAPPED {
            hEvent: event.0,
            ..Default::default()
        };
        let started =
            unsafe { WriteFile(self.handle(), Some(buffer), None, Some(&mut overlapped)) };
        complete_overlapped(self.handle(), &overlapped, started).map(|written| written as usize)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        // Persistent pipe では overlapped WriteFile の完了が framing の境界になる。
        // FlushFileBuffers は同じ handle の pending read と直列化するため使わない。
        Ok(())
    }
}

fn wide_nul(value: &str) -> Vec<u16> {
    value.encode_utf16().chain([0]).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overlapped_transport_and_home_queue_complete_round_trip() {
        // Persistent connection は reader と writer が同じ duplex handle を同時利用する。
        // 同期 handle に戻すと handshake 後の pending read が request write を塞ぐ。
        assert_ne!(
            server_pipe_access(false).0 & FILE_FLAG_OVERLAPPED.0,
            0,
            "persistent pipe の同時 read/write には overlapped handle が必要"
        );

        // 実 named pipe に依存せず、本番と同じ dispatcher helper -> Home queue ->
        // worker -> request-id 付き response の往復を固定する。
        let (work_tx, work_rx) = mpsc::sync_channel(2);
        let metrics = Arc::new(QueueMetrics::new("home-test"));
        let worker_metrics = Arc::clone(&metrics);
        let collection_engine = CollectionEngine::new(crate::settings::Settings::default());
        let worker = std::thread::spawn(move || {
            home_worker_loop(work_rx, &collection_engine, &worker_metrics)
        });
        let (reply_tx, reply_rx) = mpsc::channel();
        enqueue_work(
            &work_tx,
            &metrics,
            ClientMessage::Home {
                id: 73,
                request: mimageviewer_ipc::HomeRequest,
            },
            reply_tx,
        )
        .expect("Home request must enter its dedicated queue");

        let response = reply_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("handshake 後の最初の要求が worker を通って応答すること");
        assert!(matches!(
            response,
            ServerMessage::Home {
                id: 73,
                response: HomeResponse::Success(_),
            }
        ));
        work_tx.send(Work::Stop).unwrap();
        worker.join().unwrap();
        assert_eq!(metrics.snapshot(), (0, 0));
    }

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
