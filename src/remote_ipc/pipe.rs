use std::io::{Read, Write};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

use mimageviewer_ipc::{
    ClientHello, ClientMessage, CollectionError, CollectionErrorCode, CollectionResponse,
    ContainerResponse, HomeResponse, MAX_CONTROL_FRAME_BYTES, MediaError, MediaErrorCode,
    PIPE_NAME, PROTOCOL_VERSION, PagePriority, PageResponse, RemoteWriteError,
    RemoteWriteErrorCode, RemoteWriteRequest, RemoteWriteResponse, ServerMessage, SessionResponse,
    SessionStatus, ThumbnailError, ThumbnailErrorCode, ThumbnailResponse, negotiate, read_frame,
    write_frame,
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
use super::container::ContainerEngine;
use super::session::{SessionHandle, SessionOperation, SessionRuntime, UiWriteOutcome};
use super::thumbnail::{ThumbnailEngine, WorkerContext};

const PIPE_BUFFER_BYTES: u32 = 64 * 1024;
const PIPE_MAX_INSTANCES: u32 = 16;
/// 再接続が集中しても待機中 instance を切らさないため、常時この本数を accept 待ちにする。
const ACCEPTOR_COUNT: usize = 4;
const HEAVY_WORK_QUEUE_CAPACITY: usize = 16;
const HOME_WORK_QUEUE_CAPACITY: usize = 8;
const WRITE_WORK_QUEUE_CAPACITY: usize = 16;

enum Work {
    Request {
        message: ClientMessage,
        reply: mpsc::Sender<ServerMessage>,
        enqueued_at: Instant,
        _prefetch_permit: Option<PrefetchPermit>,
        session_operation: SessionOperation,
    },
    Stop,
}

struct PrefetchPermit {
    in_flight: Arc<AtomicUsize>,
}

impl Drop for PrefetchPermit {
    fn drop(&mut self) {
        self.in_flight.fetch_sub(1, Ordering::AcqRel);
    }
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
    write_worker: Option<std::thread::JoinHandle<()>>,
    heavy_work_tx: mpsc::SyncSender<Work>,
    home_work_tx: mpsc::SyncSender<Work>,
    write_work_tx: mpsc::SyncSender<Work>,
    session_runtime: SessionRuntime,
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

        let session_runtime = SessionRuntime::start()?;
        let session_handle = session_runtime.handle();
        let configured_worker_count = settings.parallelism.thread_count();
        let worker_count = remote_heavy_worker_count(configured_worker_count);
        let thumbnail_engine = Arc::new(ThumbnailEngine::new(settings.clone()));
        let container_engine = Arc::new(ContainerEngine::new(settings.clone()));
        let collection_engine = Arc::new(CollectionEngine::new(settings));
        let (heavy_work_tx, heavy_work_rx) = mpsc::sync_channel::<Work>(HEAVY_WORK_QUEUE_CAPACITY);
        let heavy_work_rx = Arc::new(Mutex::new(heavy_work_rx));
        let (home_work_tx, home_work_rx) = mpsc::sync_channel::<Work>(HOME_WORK_QUEUE_CAPACITY);
        let (write_work_tx, write_work_rx) = mpsc::sync_channel::<Work>(WRITE_WORK_QUEUE_CAPACITY);
        let heavy_metrics = Arc::new(QueueMetrics::new("heavy"));
        let home_metrics = Arc::new(QueueMetrics::new("home"));
        let write_metrics = Arc::new(QueueMetrics::new("write"));
        let prefetch_in_flight = Arc::new(AtomicUsize::new(0));
        let home_collection_engine = Arc::clone(&collection_engine);
        let home_worker_metrics = Arc::clone(&home_metrics);
        let home_worker = std::thread::Builder::new()
            .name("remote-home".to_owned())
            .spawn(move || {
                home_worker_loop(home_work_rx, &home_collection_engine, &home_worker_metrics)
            })
            .map_err(|error| format!("remote IPC home worker を開始できません: {error}"))?;
        let write_container_engine = Arc::clone(&container_engine);
        let write_session = session_handle.clone();
        let write_worker_metrics = Arc::clone(&write_metrics);
        let write_worker = std::thread::Builder::new()
            .name("remote-write".to_owned())
            .spawn(move || {
                write_worker_loop(
                    write_work_rx,
                    &write_container_engine,
                    &write_session,
                    &write_worker_metrics,
                )
            })
            .map_err(|error| format!("remote IPC write worker を開始できません: {error}"))?;
        let mut workers = Vec::with_capacity(worker_count);
        for index in 0..worker_count {
            let work_rx = Arc::clone(&heavy_work_rx);
            let thumbnail_engine = Arc::clone(&thumbnail_engine);
            let container_engine = Arc::clone(&container_engine);
            let collection_engine = Arc::clone(&collection_engine);
            let worker_metrics = Arc::clone(&heavy_metrics);
            workers.push(
                std::thread::Builder::new()
                    .name(format!("remote-thumb-{index}"))
                    .spawn(move || {
                        worker_loop(
                            &work_rx,
                            &thumbnail_engine,
                            &container_engine,
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
            let listener_write_tx = write_work_tx.clone();
            let listener_heavy_metrics = Arc::clone(&heavy_metrics);
            let listener_home_metrics = Arc::clone(&home_metrics);
            let listener_write_metrics = Arc::clone(&write_metrics);
            let listener_next_connection_id = Arc::clone(&next_connection_id);
            let listener_prefetch_in_flight = Arc::clone(&prefetch_in_flight);
            let listener_session = session_handle.clone();
            match std::thread::Builder::new()
                .name(format!("remote-ipc-listener-{index}"))
                .spawn(move || {
                    acceptor_loop(
                        listener_stop,
                        listener_heavy_tx,
                        listener_home_tx,
                        listener_write_tx,
                        listener_heavy_metrics,
                        listener_home_metrics,
                        listener_write_metrics,
                        listener_next_connection_id,
                        listener_prefetch_in_flight,
                        worker_count,
                        listener_session,
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
                    let _ = write_work_tx.send(Work::Stop);
                    for worker in workers {
                        let _ = worker.join();
                    }
                    let _ = home_worker.join();
                    let _ = write_worker.join();
                    return Err(format!("remote IPC listener を開始できません: {error}"));
                }
            }
        }

        crate::logger::log(format!(
            "remote_ipc: listening pipe={PIPE_NAME} protocol={PROTOCOL_VERSION} heavy_workers={worker_count} configured_workers={configured_worker_count} home_workers=1 write_workers=1 prefetch_limit={} acceptors={ACCEPTOR_COUNT} multiplexed=true",
            usize::from(worker_count >= 2)
        ));
        Ok(Self {
            stop,
            listeners,
            workers,
            home_worker: Some(home_worker),
            write_worker: Some(write_worker),
            heavy_work_tx,
            home_work_tx,
            write_work_tx,
            session_runtime,
        })
    }

    pub(super) fn session_handle(&self) -> SessionHandle {
        self.session_runtime.handle()
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
        let _ = self.write_work_tx.send(Work::Stop);
        for worker in self.workers.drain(..) {
            let _ = worker.join();
        }
        if let Some(worker) = self.home_worker.take() {
            let _ = worker.join();
        }
        if let Some(worker) = self.write_worker.take() {
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
    container_engine: &ContainerEngine,
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
                _prefetch_permit,
                session_operation,
            }) => execute_work(
                message,
                reply,
                enqueued_at,
                metrics,
                &format!("heavy-{worker_index}"),
                session_operation,
                |message| match message {
                    ClientMessage::Thumbnail { id, request, .. } => ServerMessage::Thumbnail {
                        id,
                        response: thumbnail_engine.handle(request, &context, container_engine),
                    },
                    ClientMessage::Home { id, .. } => ServerMessage::Home {
                        id,
                        response: collection_engine.home(),
                    },
                    ClientMessage::Collection { id, request, .. } => ServerMessage::Collection {
                        id,
                        response: collection_engine.collection(request),
                    },
                    ClientMessage::Container { id, request, .. } => ServerMessage::Container {
                        id,
                        response: container_engine.container(request),
                    },
                    ClientMessage::Page { id, request, .. } => ServerMessage::Page {
                        id,
                        response: container_engine.page(request, &context),
                    },
                    ClientMessage::Write { id, .. } => ServerMessage::Write {
                        id,
                        response: RemoteWriteResponse::Error(RemoteWriteError::new(
                            RemoteWriteErrorCode::Internal,
                            "write request was routed to a heavy worker",
                        )),
                    },
                    ClientMessage::RemoteWebConnectionInfo { id, .. } => {
                        ServerMessage::RemoteWebConnectionInfo {
                            id,
                            accepted: false,
                            message: "connection information was routed to a worker".to_owned(),
                        }
                    }
                    ClientMessage::SessionAcquire { id, .. }
                    | ClientMessage::SessionPing { id, .. }
                    | ClientMessage::SessionActivity { id, .. } => ServerMessage::Session {
                        id,
                        response: session_response(
                            SessionStatus::NotAcquired,
                            "session control request was routed to a worker",
                        ),
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
                _prefetch_permit,
                session_operation,
            }) => execute_work(
                message,
                reply,
                enqueued_at,
                metrics,
                "home-0",
                session_operation,
                |message| match message {
                    ClientMessage::Home { id, .. } => ServerMessage::Home {
                        id,
                        response: collection_engine.home(),
                    },
                    other => service_stopped_response(&other),
                },
            ),
            Ok(Work::Stop) | Err(_) => break,
        }
    }
    crate::logger::log("remote_ipc: worker_stopped queue=home worker=home-0".to_owned());
}

fn write_worker_loop(
    work_rx: mpsc::Receiver<Work>,
    container_engine: &ContainerEngine,
    session: &SessionHandle,
    metrics: &QueueMetrics,
) {
    crate::logger::log("remote_ipc: worker_started queue=write worker=write-0".to_owned());
    while let Ok(work) = work_rx.recv() {
        let Work::Request {
            message,
            reply,
            enqueued_at,
            session_operation,
            ..
        } = work
        else {
            break;
        };
        let request_id = message.id();
        let (queued, active) = metrics.started();
        crate::logger::log(format!(
            "remote_ipc: worker_start request_id={request_id} kind=write queue=write worker=write-0 queue_wait_ms={:.1} queued={queued} active={active}",
            enqueued_at.elapsed().as_secs_f64() * 1000.0
        ));
        let started_at = Instant::now();
        let response = match message {
            ClientMessage::Write {
                id, mut request, ..
            } => {
                if let Err(error) = container_engine.validate_write_request(&mut request) {
                    session_operation.started();
                    session_operation.finish(false);
                    ServerMessage::Write {
                        id,
                        response: RemoteWriteResponse::Error(error),
                    }
                } else {
                    match session.submit_write(request, session_operation) {
                        UiWriteOutcome::Write(response) => ServerMessage::Write { id, response },
                        UiWriteOutcome::Session(response) => {
                            ServerMessage::Session { id, response }
                        }
                    }
                }
            }
            other => {
                session_operation.started();
                session_operation.finish(false);
                service_stopped_response(&other)
            }
        };
        let outcome = response_outcome(&response);
        let reply_ok = reply.send(response).is_ok();
        let (queued, active) = metrics.finished();
        crate::logger::log(format!(
            "remote_ipc: worker_finish request_id={request_id} kind=write queue=write worker=write-0 outcome={outcome} duration_ms={:.1} reply_ok={reply_ok} queued={queued} active={active}",
            started_at.elapsed().as_secs_f64() * 1000.0
        ));
    }
    crate::logger::log("remote_ipc: worker_stopped queue=write worker=write-0".to_owned());
}

fn execute_work(
    message: ClientMessage,
    reply: mpsc::Sender<ServerMessage>,
    enqueued_at: Instant,
    metrics: &QueueMetrics,
    worker: &str,
    session_operation: SessionOperation,
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
    session_operation.started();
    let response = handler(message);
    let ownership = session_operation.ownership_response();
    let response = if ownership.status == SessionStatus::Active {
        response
    } else {
        ServerMessage::Session {
            id: request_id,
            response: ownership,
        }
    };
    let outcome = response_outcome(&response);
    session_operation.finish(outcome == "ok");
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
        ClientMessage::RemoteWebConnectionInfo { .. } => "connection_info",
        ClientMessage::SessionAcquire { .. } => "session_acquire",
        ClientMessage::SessionPing { .. } => "session_ping",
        ClientMessage::SessionActivity { .. } => "session_activity",
        ClientMessage::Thumbnail { .. } => "thumbnail",
        ClientMessage::Home { .. } => "home",
        ClientMessage::Collection { .. } => "collection",
        ClientMessage::Container { .. } => "container",
        ClientMessage::Page { request, .. } => match request.priority {
            PagePriority::Foreground => "page_foreground",
            PagePriority::Prefetch => "page_prefetch",
        },
        ClientMessage::Write { .. } => "write",
    }
}

fn message_client_id(message: &ClientMessage) -> Option<&str> {
    match message {
        ClientMessage::RemoteWebConnectionInfo { .. } => None,
        ClientMessage::Thumbnail { client_id, .. }
        | ClientMessage::Home { client_id, .. }
        | ClientMessage::Collection { client_id, .. }
        | ClientMessage::Container { client_id, .. }
        | ClientMessage::Page { client_id, .. }
        | ClientMessage::Write { client_id, .. }
        | ClientMessage::SessionActivity { client_id, .. } => Some(client_id),
        ClientMessage::SessionAcquire { request, .. } => Some(&request.client_id),
        ClientMessage::SessionPing { request, .. } => Some(&request.client_id),
    }
}

fn operation_description(message: &ClientMessage) -> String {
    match message {
        ClientMessage::RemoteWebConnectionInfo { .. } => "接続情報を更新中".to_owned(),
        ClientMessage::Thumbnail { request, .. } => match request.address.subresource {
            mimageviewer_ipc::RemoteSubresource::PdfPage { page_number } => {
                format!("PDF {} ページ目のサムネイルを生成中", page_number + 1)
            }
            mimageviewer_ipc::RemoteSubresource::ZipEntry { .. } => {
                "ZIP ページのサムネイルを生成中".to_owned()
            }
            _ => "サムネイルを生成中".to_owned(),
        },
        ClientMessage::Home { .. } => "ホームを読み込み中".to_owned(),
        ClientMessage::Collection { .. } => "集約ビューを読み込み中".to_owned(),
        ClientMessage::Container { .. } => "コンテナを列挙中".to_owned(),
        ClientMessage::Page { request, .. } => match request.address.subresource {
            mimageviewer_ipc::RemoteSubresource::PdfPage { page_number } => {
                format!("PDF {} ページ目をレンダリング中", page_number + 1)
            }
            mimageviewer_ipc::RemoteSubresource::ZipEntry { .. } => {
                "ZIP ページをレンダリング中".to_owned()
            }
            _ => "ページをレンダリング中".to_owned(),
        },
        ClientMessage::Write { request, .. } => match request {
            RemoteWriteRequest::SetSpread { .. } => "見開き設定を書き込み中",
            RemoteWriteRequest::RecordReadingProgress { .. } => "読書位置を記録中",
            RemoteWriteRequest::SetRating { .. } => "レーティングを書き込み中",
            RemoteWriteRequest::SetBookmark { .. } => "ブックマークを書き込み中",
            RemoteWriteRequest::GetItemState { .. } => "ページ情報を確認中",
        }
        .to_owned(),
        ClientMessage::SessionAcquire { .. }
        | ClientMessage::SessionPing { .. }
        | ClientMessage::SessionActivity { .. } => "接続を確認中".to_owned(),
    }
}

fn session_response(status: SessionStatus, message: impl Into<String>) -> SessionResponse {
    SessionResponse {
        status,
        message: message.into(),
    }
}

fn response_outcome(response: &ServerMessage) -> &'static str {
    match response {
        ServerMessage::RemoteWebConnectionInfo { accepted: true, .. }
        | ServerMessage::Session {
            response:
                SessionResponse {
                    status: SessionStatus::Active,
                    ..
                },
            ..
        }
        | ServerMessage::Thumbnail {
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
        }
        | ServerMessage::Container {
            response: ContainerResponse::Success(_),
            ..
        }
        | ServerMessage::Page {
            response: PageResponse::Success(_),
            ..
        }
        | ServerMessage::Write {
            response: RemoteWriteResponse::Success(_),
            ..
        } => "ok",
        ServerMessage::RemoteWebConnectionInfo { .. }
        | ServerMessage::Session { .. }
        | ServerMessage::Thumbnail {
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
        }
        | ServerMessage::Container {
            response: ContainerResponse::Error(_),
            ..
        }
        | ServerMessage::Page {
            response: PageResponse::Error(_),
            ..
        }
        | ServerMessage::Write {
            response: RemoteWriteResponse::Error(_),
            ..
        } => "error",
    }
}

fn acceptor_loop(
    stop: Arc<AtomicBool>,
    heavy_work_tx: mpsc::SyncSender<Work>,
    home_work_tx: mpsc::SyncSender<Work>,
    write_work_tx: mpsc::SyncSender<Work>,
    heavy_metrics: Arc<QueueMetrics>,
    home_metrics: Arc<QueueMetrics>,
    write_metrics: Arc<QueueMetrics>,
    next_connection_id: Arc<AtomicU64>,
    prefetch_in_flight: Arc<AtomicUsize>,
    heavy_worker_count: usize,
    session: SessionHandle,
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
        let write_work_tx = write_work_tx.clone();
        let heavy_metrics = Arc::clone(&heavy_metrics);
        let home_metrics = Arc::clone(&home_metrics);
        let write_metrics = Arc::clone(&write_metrics);
        let prefetch_in_flight = Arc::clone(&prefetch_in_flight);
        let session = session.clone();
        if let Err(error) = std::thread::Builder::new()
            .name(format!("remote-ipc-connection-{connection_id}"))
            .spawn(move || {
                handle_connection(
                    connection_id,
                    pipe,
                    heavy_work_tx,
                    home_work_tx,
                    write_work_tx,
                    heavy_metrics,
                    home_metrics,
                    write_metrics,
                    prefetch_in_flight,
                    heavy_worker_count,
                    session,
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
    write_work_tx: mpsc::SyncSender<Work>,
    heavy_metrics: Arc<QueueMetrics>,
    home_metrics: Arc<QueueMetrics>,
    write_metrics: Arc<QueueMetrics>,
    prefetch_in_flight: Arc<AtomicUsize>,
    heavy_worker_count: usize,
    session: SessionHandle,
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
    // handshake 済み接続を接続情報の受信前から可視化する。これにより UI は
    // 「remote-web 自体が未接続」と「接続済みだが URL 未着」を区別できる。
    session.remote_web_connected(connection_id);

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
        match &message {
            ClientMessage::RemoteWebConnectionInfo { id, info } => {
                let accepted = session.announce_remote_web(connection_id, info.clone());
                crate::logger::log(format!(
                    "remote_ipc: connection_info connection_id={connection_id} accepted={accepted} tailscale_serve={:?} pin_configured={}",
                    info.tailscale_serve, info.pin_configured
                ));
                let message = if accepted {
                    "remote-web connection information accepted"
                } else {
                    "remote-web connection URL was rejected"
                };
                let _ = reply_tx.send(ServerMessage::RemoteWebConnectionInfo {
                    id: *id,
                    accepted,
                    message: message.to_owned(),
                });
                continue;
            }
            ClientMessage::SessionAcquire { id, request } => {
                let response = session.acquire(request.clone());
                let _ = reply_tx.send(ServerMessage::Session { id: *id, response });
                continue;
            }
            ClientMessage::SessionPing { id, request } => {
                let response = session.ping(request);
                let _ = reply_tx.send(ServerMessage::Session { id: *id, response });
                continue;
            }
            ClientMessage::SessionActivity { id, client_id } => {
                let response =
                    match session.begin_operation(client_id, "API 要求を処理中".to_owned()) {
                        Ok(operation) => {
                            operation.started();
                            operation.finish(true);
                            SessionResponse::active()
                        }
                        Err(response) => response,
                    };
                let _ = reply_tx.send(ServerMessage::Session { id: *id, response });
                continue;
            }
            _ => {}
        }
        let client_id =
            message_client_id(&message).expect("non-control remote IPC requests carry a client id");
        let session_operation =
            match session.begin_operation(client_id, operation_description(&message)) {
                Ok(operation) => operation,
                Err(response) => {
                    let _ = reply_tx.send(ServerMessage::Session {
                        id: request_id,
                        response,
                    });
                    continue;
                }
            };
        let prefetch_permit = if matches!(
            message,
            ClientMessage::Page {
                request: mimageviewer_ipc::PageRequest {
                    priority: PagePriority::Prefetch,
                    ..
                },
                ..
            }
        ) {
            let (queued, active) = heavy_metrics.snapshot();
            match try_acquire_prefetch(&prefetch_in_flight, heavy_worker_count, queued, active) {
                Some(permit) => Some(permit),
                None => {
                    crate::logger::log(format!(
                        "remote_ipc: prefetch_busy connection_id={connection_id} request_id={request_id} heavy_workers={heavy_worker_count} queued={queued} active={active}"
                    ));
                    let _ = reply_tx.send(queue_busy_response(&message));
                    drop(session_operation);
                    continue;
                }
            }
        } else {
            None
        };
        // Home は専用 worker へ分離する。重い queue が満杯でも connection reader を
        // 塞がず、後続 Home を読めるよう Busy を明示応答する。
        let (work_tx, metrics) = if matches!(message, ClientMessage::Home { .. }) {
            (&home_work_tx, &home_metrics)
        } else if matches!(message, ClientMessage::Write { .. }) {
            (&write_work_tx, &write_metrics)
        } else {
            (&heavy_work_tx, &heavy_metrics)
        };
        match enqueue_work(
            work_tx,
            metrics,
            message,
            reply_tx.clone(),
            prefetch_permit,
            session_operation,
        ) {
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
    session.remote_web_disconnected(connection_id);
    drop(reply_tx);
    let _ = writer.join();
}

fn enqueue_work(
    work_tx: &mpsc::SyncSender<Work>,
    metrics: &QueueMetrics,
    message: ClientMessage,
    reply: mpsc::Sender<ServerMessage>,
    prefetch_permit: Option<PrefetchPermit>,
    session_operation: SessionOperation,
) -> Result<(), mpsc::TrySendError<Work>> {
    metrics.reserve();
    let result = work_tx.try_send(Work::Request {
        message,
        reply,
        enqueued_at: Instant::now(),
        _prefetch_permit: prefetch_permit,
        session_operation,
    });
    if result.is_err() {
        metrics.rollback();
    }
    result
}

fn try_acquire_prefetch(
    in_flight: &Arc<AtomicUsize>,
    heavy_worker_count: usize,
    queued: usize,
    active: usize,
) -> Option<PrefetchPermit> {
    // prefetch 開始後も foreground 用 worker を 1 本空ける。queue が既にある時も
    // FIFO の前後関係で表示要求を遅らせ得るため受け付けない。
    if heavy_worker_count < 2 || queued > 0 || active >= heavy_worker_count - 1 {
        return None;
    }
    in_flight
        .compare_exchange(0, 1, Ordering::AcqRel, Ordering::Acquire)
        .ok()
        .map(|_| PrefetchPermit {
            in_flight: Arc::clone(in_flight),
        })
}

fn service_stopped_response(message: &ClientMessage) -> ServerMessage {
    match message {
        ClientMessage::RemoteWebConnectionInfo { id, .. } => {
            ServerMessage::RemoteWebConnectionInfo {
                id: *id,
                accepted: false,
                message: "mIV 本体のリモートサービスが停止しています".to_owned(),
            }
        }
        ClientMessage::SessionAcquire { id, .. }
        | ClientMessage::SessionPing { id, .. }
        | ClientMessage::SessionActivity { id, .. } => ServerMessage::Session {
            id: *id,
            response: session_response(
                SessionStatus::NotAcquired,
                "mIV 本体のリモートサービスが停止しています",
            ),
        },
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
        ClientMessage::Container { id, .. } => ServerMessage::Container {
            id: *id,
            response: ContainerResponse::Error(MediaError::new(
                MediaErrorCode::Internal,
                "mIV 本体のコンテナワーカーが停止しています",
            )),
        },
        ClientMessage::Page { id, .. } => ServerMessage::Page {
            id: *id,
            response: PageResponse::Error(MediaError::new(
                MediaErrorCode::Internal,
                "mIV 本体のページワーカーが停止しています",
            )),
        },
        ClientMessage::Write { id, .. } => ServerMessage::Write {
            id: *id,
            response: RemoteWriteResponse::Error(RemoteWriteError::new(
                RemoteWriteErrorCode::Internal,
                "mIV 本体の書き込みワーカーが停止しています",
            )),
        },
    }
}

fn queue_busy_response(message: &ClientMessage) -> ServerMessage {
    match message {
        ClientMessage::RemoteWebConnectionInfo { id, .. } => {
            ServerMessage::RemoteWebConnectionInfo {
                id: *id,
                accepted: false,
                message: "mIV 本体のリモートサービスが混み合っています".to_owned(),
            }
        }
        ClientMessage::SessionAcquire { id, .. }
        | ClientMessage::SessionPing { id, .. }
        | ClientMessage::SessionActivity { id, .. } => ServerMessage::Session {
            id: *id,
            response: session_response(
                SessionStatus::NotAcquired,
                "mIV 本体のリモートサービスが混み合っています",
            ),
        },
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
        ClientMessage::Container { id, .. } => ServerMessage::Container {
            id: *id,
            response: ContainerResponse::Error(MediaError::new(
                MediaErrorCode::Busy,
                "mIV 本体のリモートコンテナ queue が混み合っています",
            )),
        },
        ClientMessage::Page { id, .. } => ServerMessage::Page {
            id: *id,
            response: PageResponse::Error(MediaError::new(
                MediaErrorCode::Busy,
                "mIV 本体のリモートページ queue が混み合っています",
            )),
        },
        ClientMessage::Write { id, .. } => ServerMessage::Write {
            id: *id,
            response: RemoteWriteResponse::Error(RemoteWriteError::new(
                RemoteWriteErrorCode::Busy,
                "mIV 本体のリモート書き込み queue が混み合っています",
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
        let session = SessionHandle::new();
        session.acquire(mimageviewer_ipc::SessionAcquireRequest {
            client_id: "test-client".to_owned(),
            peer: mimageviewer_ipc::SessionPeerInfo {
                connection_kind: mimageviewer_ipc::SessionConnectionKind::Unknown,
                device_name: None,
            },
        });
        let session_operation = session
            .begin_operation("test-client", "ホームを読み込み中".to_owned())
            .unwrap();
        enqueue_work(
            &work_tx,
            &metrics,
            ClientMessage::Home {
                id: 73,
                client_id: "test-client".to_owned(),
                request: mimageviewer_ipc::HomeRequest,
            },
            reply_tx,
            None,
            session_operation,
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
    fn prefetch_uses_at_most_one_remote_worker_and_is_disabled_with_one_worker() {
        let in_flight = Arc::new(AtomicUsize::new(0));
        assert!(try_acquire_prefetch(&in_flight, 1, 0, 0).is_none());
        assert!(try_acquire_prefetch(&in_flight, 2, 1, 0).is_none());
        assert!(try_acquire_prefetch(&in_flight, 2, 0, 1).is_none());
        let first = try_acquire_prefetch(&in_flight, 2, 0, 0).expect("first prefetch permit");
        assert!(try_acquire_prefetch(&in_flight, 2, 0, 0).is_none());
        drop(first);
        assert!(try_acquire_prefetch(&in_flight, 2, 0, 0).is_some());
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
            client_id: "test-client".to_owned(),
            request: mimageviewer_ipc::ThumbnailRequest {
                address: mimageviewer_ipc::RemoteAddress::file(
                    "00000000-0000-0000-0000-000000000000",
                    "page.jpg",
                ),
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
