use std::collections::HashMap;
use std::io::{Read, Write};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

use mimageviewer_ipc::{
    ClientHello, ClientMessage, CollectionError, CollectionErrorCode, CollectionResponse,
    ContainerResponse, FavoriteSearchResponse, FolderListResponse, HomeResponse,
    MAX_CONTROL_FRAME_BYTES, MediaError, MediaErrorCode, PIPE_NAME, PROTOCOL_VERSION, PagePriority,
    PageResponse, RemoteSessionIdentity, RemoteWriteError, RemoteWriteErrorCode,
    RemoteWriteRequest, RemoteWriteResponse, ServerMessage, SessionResponse, SessionStatus,
    TagBrowseResponse, TagItemsResponse, ThumbnailError, ThumbnailErrorCode, ThumbnailResponse,
    VideoStreamError, VideoStreamErrorCode, VideoStreamResult, VideoStreamThumbnailPayload,
    negotiate, read_frame, write_frame,
};
use windows::Win32::Foundation::{
    CloseHandle, ERROR_FILE_NOT_FOUND, ERROR_IO_PENDING, ERROR_PIPE_BUSY, ERROR_PIPE_CONNECTED,
    GENERIC_ALL, GetLastError, HANDLE,
};
use windows::Win32::Security::{
    ACCESS_ALLOWED_ACE, ACL, ACL_REVISION, AddAccessAllowedAce, GetLengthSid, GetTokenInformation,
    InitializeAcl, InitializeSecurityDescriptor, IsValidSid, PSECURITY_DESCRIPTOR,
    SECURITY_ATTRIBUTES, SECURITY_DESCRIPTOR, SetSecurityDescriptorDacl, TOKEN_QUERY, TOKEN_USER,
    TokenUser,
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
use windows::Win32::System::SystemServices::SECURITY_DESCRIPTOR_REVISION;
use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
use windows::core::PCWSTR;

use super::collections::CollectionEngine;
use super::container::ContainerEngine;
use super::heavy_queue::{
    CompleteHeavyQueueResult, HeavyQueue, HeavyQueueCapacities, HeavyQueueItem, HeavyQueueLane,
    HeavyQueuePushErrorKind, HeavyQueueSnapshot, PromoteHeavyQueueResult,
};
use super::page_jobs::{
    DisplayRequestId, PageJobCancelCause, PageJobId, PageJobPriority, PageJobRegistry,
    PromotePageJobResult, RegisterPageJobError, ReleasePageJobResult,
};
use super::session::{
    SessionHandle, SessionOperation, SessionRuntime, UiWriteOutcome, VideoStreamUiOutcome,
    VideoStreamUiRequest,
};
use super::thumbnail::{ThumbnailEngine, WorkerContext};
use super::video_stream::{VideoStreamEngine, VideoStreamStartBudget, VideoStreamStartStage};

const PIPE_BUFFER_BYTES: u32 = 64 * 1024;
const PIPE_MAX_INSTANCES: u32 = 16;
/// 再接続が集中しても待機中 instance を切らさないため、常時この本数を accept 待ちにする。
const ACCEPTOR_COUNT: usize = 4;
// remote-web admits at most four concurrent heavy IPC requests. Eight slots leave headroom for
// promotion/reconnect overlap without treating this queue as a bulk backlog; Interactive keeps
// the old capacity because direct IPC clients may mix thumbnails and container enumeration.
const HEAVY_WORK_QUEUE_CAPACITIES: HeavyQueueCapacities = HeavyQueueCapacities {
    foreground: 8,
    interactive: 16,
    prefetch: 8,
};
const HOME_WORK_QUEUE_CAPACITY: usize = 8;
const WRITE_WORK_QUEUE_CAPACITY: usize = 16;
const STREAM_WORK_QUEUE_CAPACITY: usize = 32;
const STREAM_WORKER_COUNT: usize = 4;

enum Work {
    Request {
        message: ClientMessage,
        reply: mpsc::Sender<ServerMessage>,
        enqueued_at: Instant,
        session_operation: SessionOperation,
        page_job: Option<PageJobWork>,
    },
    Stop,
}

struct PageJobWork {
    registry: Arc<PageJobRegistry>,
    connection_id: u64,
    job_id: PageJobId,
    cancel: Arc<AtomicBool>,
}

impl Drop for PageJobWork {
    fn drop(&mut self) {
        self.registry.finish(self.connection_id, &self.job_id);
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum WorkLane {
    Heavy,
    Home,
    Write,
    Stream,
}

type HeavyKey = (u64, u64);
type PageJobKey = (u64, PageJobId);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum HeavyEnqueueErrorKind {
    Cancelled,
    PrefetchUnavailableWithSingleWorker,
    LaneFull,
    DuplicateKey,
    Shutdown,
}

impl HeavyEnqueueErrorKind {
    fn log_reason(self) -> &'static str {
        match self {
            Self::Cancelled => "cancelled_before_enqueue",
            Self::PrefetchUnavailableWithSingleWorker => "prefetch_unavailable_with_single_worker",
            Self::LaneFull => "lane_full",
            Self::DuplicateKey => "duplicate_key",
            Self::Shutdown => "shutdown",
        }
    }
}

struct HeavyEnqueueError {
    kind: HeavyEnqueueErrorKind,
    lane: HeavyQueueLane,
    work: Work,
}

struct HeavyQueueWiring {
    queue: HeavyQueue<HeavyKey, Work>,
    registry: Arc<PageJobRegistry>,
    worker_count: usize,
    /// Glue-only mirror from the registry identity to the queue identity. Neither owner calls the
    /// other; every paired registry/queue transition is serialized by this mutex.
    page_keys: Mutex<HashMap<PageJobKey, HeavyKey>>,
}

impl HeavyQueueWiring {
    fn new(worker_count: usize, registry: Arc<PageJobRegistry>) -> Self {
        Self::new_with_capacities(worker_count, HEAVY_WORK_QUEUE_CAPACITIES, registry)
    }

    fn new_with_capacities(
        worker_count: usize,
        capacities: HeavyQueueCapacities,
        registry: Arc<PageJobRegistry>,
    ) -> Self {
        Self {
            queue: HeavyQueue::new(worker_count, capacities),
            registry,
            worker_count,
            page_keys: Mutex::new(HashMap::new()),
        }
    }

    fn enqueue(&self, connection_id: u64, work: Work) -> Result<HeavyQueueLane, HeavyEnqueueError> {
        self.enqueue_inner(connection_id, work)
    }

    fn enqueue_inner(
        &self,
        connection_id: u64,
        work: Work,
    ) -> Result<HeavyQueueLane, HeavyEnqueueError> {
        let (request_id, lane, page_key, cancelled) = heavy_work_metadata(&work);
        let key = (connection_id, request_id);
        let mut page_keys = self
            .page_keys
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if cancelled {
            return Err(HeavyEnqueueError {
                kind: HeavyEnqueueErrorKind::Cancelled,
                lane,
                work,
            });
        }
        if lane == HeavyQueueLane::Prefetch && self.worker_count == 1 {
            return Err(HeavyEnqueueError {
                kind: HeavyEnqueueErrorKind::PrefetchUnavailableWithSingleWorker,
                lane,
                work,
            });
        }
        self.push_locked(&mut page_keys, page_key, key, lane, work)
    }

    fn push_locked(
        &self,
        page_keys: &mut HashMap<PageJobKey, HeavyKey>,
        page_key: Option<PageJobKey>,
        key: HeavyKey,
        lane: HeavyQueueLane,
        work: Work,
    ) -> Result<HeavyQueueLane, HeavyEnqueueError> {
        match self.queue.push(key, lane, work) {
            Ok(()) => {
                if let Some(page_key) = page_key {
                    if let Some(previous_key) = page_keys.insert(page_key.clone(), key) {
                        crate::logger::log(format!(
                            "remote_ipc: page_queue_reconcile action=map_insert result=replaced connection_id={} job_id={} previous_request_id={} request_id={}",
                            page_key.0,
                            page_key.1.as_str(),
                            previous_key.1,
                            key.1,
                        ));
                    }
                }
                Ok(lane)
            }
            Err(error) => {
                let kind = match error.kind() {
                    HeavyQueuePushErrorKind::LaneFull => HeavyEnqueueErrorKind::LaneFull,
                    HeavyQueuePushErrorKind::DuplicateKey => HeavyEnqueueErrorKind::DuplicateKey,
                    HeavyQueuePushErrorKind::Shutdown => HeavyEnqueueErrorKind::Shutdown,
                };
                let (_, work, lane) = error.into_item().into_parts();
                Err(HeavyEnqueueError { kind, lane, work })
            }
        }
    }

    fn pop(&self) -> Option<HeavyQueueItem<HeavyKey, Work>> {
        self.queue.pop()
    }

    fn complete(&self, key: &HeavyKey, page_key: Option<&PageJobKey>) -> CompleteHeavyQueueResult {
        let mut page_keys = self
            .page_keys
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if let Some(page_key) = page_key
            && page_keys.get(page_key) == Some(key)
        {
            page_keys.remove(page_key);
        }
        self.queue.complete(key)
    }

    fn promote_page(
        &self,
        connection_id: u64,
        job_id: &PageJobId,
        display_request_id: DisplayRequestId,
    ) -> PromotePageJobResult {
        let display_request_present = !display_request_id.as_str().is_empty();
        let page_keys = self
            .page_keys
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let result = self
            .registry
            .promote(connection_id, job_id, display_request_id);
        if matches!(
            result,
            PromotePageJobResult::Promoted | PromotePageJobResult::AlreadyForeground
        ) && let Some(key) = page_keys.get(&(connection_id, job_id.clone()))
        {
            let queue_result = self.queue.promote(key);
            log_page_queue_promotion(
                connection_id,
                job_id,
                display_request_present,
                result,
                queue_result,
            );
        }
        result
    }

    fn release_page(
        &self,
        connection_id: u64,
        job_id: &PageJobId,
        cause: PageJobCancelCause,
    ) -> (ReleasePageJobResult, Vec<Work>) {
        let mut page_keys = self
            .page_keys
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let result = self.registry.release(connection_id, job_id, cause);
        let removed = if matches!(
            result,
            ReleasePageJobResult::Released | ReleasePageJobResult::AlreadyReleased { .. }
        ) {
            page_keys
                .get(&(connection_id, job_id.clone()))
                .copied()
                .map(|key| self.queue.prune(|queued_key, _, _| *queued_key == key))
                .unwrap_or_default()
        } else {
            Vec::new()
        };
        if !removed.is_empty() {
            page_keys.remove(&(connection_id, job_id.clone()));
        }
        (result, heavy_payloads(removed))
    }

    fn close_connection(&self, connection_id: u64) -> Vec<Work> {
        let mut page_keys = self
            .page_keys
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let result = self
            .registry
            .close_connection(connection_id, PageJobCancelCause::ConnectionClosed);
        let removed = self
            .queue
            .prune(|(queued_connection_id, _), _, _| *queued_connection_id == connection_id);
        page_keys.retain(|(job_connection_id, _), _| *job_connection_id != connection_id);
        log_connection_prune(
            connection_id,
            result.released,
            result.already_released,
            removed.len(),
        );
        heavy_payloads(removed)
    }

    fn stop(&self) -> Vec<Work> {
        let mut page_keys = self
            .page_keys
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        self.registry.stop(PageJobCancelCause::ServiceStopping);
        let removed = self.queue.shutdown();
        page_keys.clear();
        heavy_payloads(removed)
    }

    fn snapshot(&self) -> HeavyQueueSnapshot {
        self.queue.snapshot()
    }
}

struct HeavyCompletionGuard<'a> {
    wiring: &'a HeavyQueueWiring,
    key: HeavyKey,
    lane: HeavyQueueLane,
    page_key: Option<PageJobKey>,
    completed: bool,
}

impl<'a> HeavyCompletionGuard<'a> {
    fn new(
        wiring: &'a HeavyQueueWiring,
        key: HeavyKey,
        lane: HeavyQueueLane,
        page_key: Option<PageJobKey>,
    ) -> Self {
        Self {
            wiring,
            key,
            lane,
            page_key,
            completed: false,
        }
    }

    fn complete(&mut self) -> CompleteHeavyQueueResult {
        self.completed = true;
        self.wiring.complete(&self.key, self.page_key.as_ref())
    }
}

impl Drop for HeavyCompletionGuard<'_> {
    fn drop(&mut self) {
        if !self.completed {
            self.completed = true;
            let _ = self.wiring.complete(&self.key, self.page_key.as_ref());
        }
    }
}

fn heavy_work_metadata(work: &Work) -> (u64, HeavyQueueLane, Option<PageJobKey>, bool) {
    match work {
        Work::Request {
            message, page_job, ..
        } => (
            message.id(),
            heavy_queue_lane(message, page_job.as_ref()),
            page_job_key(page_job.as_ref()),
            page_job_cancelled(page_job.as_ref()),
        ),
        Work::Stop => unreachable!(),
    }
}

fn page_job_key(page_job: Option<&PageJobWork>) -> Option<PageJobKey> {
    page_job.map(|job| (job.connection_id, job.job_id.clone()))
}

fn page_job_cancelled(page_job: Option<&PageJobWork>) -> bool {
    page_job.is_some_and(|job| job.cancel.load(Ordering::Acquire))
}

fn heavy_queue_lane(message: &ClientMessage, page_job: Option<&PageJobWork>) -> HeavyQueueLane {
    match message {
        ClientMessage::Page { .. } => match effective_page_priority(page_job.unwrap()) {
            PagePriority::Foreground => HeavyQueueLane::Foreground,
            PagePriority::Prefetch => HeavyQueueLane::Prefetch,
        },
        _ => HeavyQueueLane::Interactive,
    }
}

fn heavy_payloads(items: Vec<HeavyQueueItem<HeavyKey, Work>>) -> Vec<Work> {
    items.into_iter().map(|item| item.into_parts().1).collect()
}

fn log_page_queue_promotion(
    connection_id: u64,
    job_id: &PageJobId,
    display_request_present: bool,
    registry: PromotePageJobResult,
    queue: PromoteHeavyQueueResult,
) {
    crate::logger::log(format!(
        "remote_ipc: page_queue_reconcile action=promote connection_id={connection_id} job_id={} display_request_present={display_request_present} registry={registry:?} queue={queue:?}",
        job_id.as_str()
    ));
}

fn log_connection_prune(
    connection_id: u64,
    released: usize,
    already_released: usize,
    pruned: usize,
) {
    crate::logger::log(format!(
        "remote_ipc: page_queue_reconcile action=close_connection connection_id={connection_id} registry_released={released} registry_already_released={already_released} queue_pruned={pruned}"
    ));
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

struct QueueLogState {
    queued: usize,
    active: usize,
    lane_fields: String,
}

enum ExecutionQueue<'a> {
    Metrics(&'a QueueMetrics),
    Heavy(HeavyCompletionGuard<'a>),
}

impl ExecutionQueue<'_> {
    fn name(&self) -> &'static str {
        match self {
            Self::Metrics(metrics) => metrics.name,
            Self::Heavy(_) => "heavy",
        }
    }

    fn started(&self) -> QueueLogState {
        match self {
            Self::Metrics(metrics) => {
                let (queued, active) = metrics.started();
                QueueLogState {
                    queued,
                    active,
                    lane_fields: String::new(),
                }
            }
            Self::Heavy(completion) => {
                heavy_queue_log_state(completion.wiring.snapshot(), Some(completion.lane))
            }
        }
    }

    fn finished(&mut self) -> QueueLogState {
        match self {
            Self::Metrics(metrics) => {
                let (queued, active) = metrics.finished();
                QueueLogState {
                    queued,
                    active,
                    lane_fields: String::new(),
                }
            }
            Self::Heavy(completion) => {
                if completion.complete() == CompleteHeavyQueueResult::UnknownKey {
                    crate::logger::log(format!(
                        "remote_ipc: page_queue_reconcile action=complete key={:?} queue=unknown_key",
                        completion.key
                    ));
                }
                heavy_queue_log_state(completion.wiring.snapshot(), Some(completion.lane))
            }
        }
    }
}

fn heavy_queue_log_state(
    snapshot: HeavyQueueSnapshot,
    lane: Option<HeavyQueueLane>,
) -> QueueLogState {
    let lane = lane.map(heavy_lane_name).unwrap_or("none");
    QueueLogState {
        queued: snapshot.queued(),
        active: snapshot.active(),
        lane_fields: format!(
            " lane={lane} foreground_queued={} foreground_active={} interactive_queued={} interactive_active={} prefetch_queued={} prefetch_active={}",
            snapshot.foreground.queued,
            snapshot.foreground.active,
            snapshot.interactive.queued,
            snapshot.interactive.active,
            snapshot.prefetch.queued,
            snapshot.prefetch.active,
        ),
    }
}

fn heavy_lane_name(lane: HeavyQueueLane) -> &'static str {
    match lane {
        HeavyQueueLane::Foreground => "foreground",
        HeavyQueueLane::Interactive => "interactive",
        HeavyQueueLane::Prefetch => "prefetch",
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
    stream_workers: Vec<std::thread::JoinHandle<()>>,
    home_worker: Option<std::thread::JoinHandle<()>>,
    write_worker: Option<std::thread::JoinHandle<()>>,
    heavy_queue: Arc<HeavyQueueWiring>,
    home_work_tx: mpsc::SyncSender<Work>,
    write_work_tx: mpsc::SyncSender<Work>,
    stream_work_tx: mpsc::SyncSender<Work>,
    session_runtime: SessionRuntime,
    _ai_jobs: Arc<super::ai_job::RemoteAiJobRegistry>,
    _archive_jobs: Arc<super::archive_job::RemoteArchiveJobRegistry>,
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
        let page_jobs = Arc::new(PageJobRegistry::new());
        session_handle.install_page_jobs(&page_jobs);
        let configured_worker_count = settings.parallelism.thread_count();
        let worker_count = remote_heavy_worker_count(configured_worker_count);
        let favorites = super::live_favorites::LiveFavorites::live(settings.favorites.clone())?;
        let thumbnail_engine = Arc::new(ThumbnailEngine::new(settings.clone()));
        let container_engine = Arc::new(ContainerEngine::new_with_session(
            settings.clone(),
            session_handle.clone(),
        ));
        let ai_executor = Arc::new(super::ai_job::ContainerRemoteAiExecutor::new(Arc::clone(
            &container_engine,
        )));
        let ai_jobs = super::ai_job::RemoteAiJobRegistry::new(ai_executor);
        session_handle.install_ai_jobs(&ai_jobs);
        let archive_executor = Arc::new(super::archive_job::ContainerRemoteArchiveExecutor::new(
            Arc::clone(&container_engine),
            session_handle.clone(),
        ));
        let archive_jobs = super::archive_job::RemoteArchiveJobRegistry::new(archive_executor);
        session_handle.install_archive_jobs(&archive_jobs);
        let video_stream_engine = Arc::new(VideoStreamEngine::new());
        let collection_engine = Arc::new(CollectionEngine::new_with_live_favorites(
            settings, favorites,
        ));
        let heavy_queue = Arc::new(HeavyQueueWiring::new(worker_count, Arc::clone(&page_jobs)));
        let (home_work_tx, home_work_rx) = mpsc::sync_channel::<Work>(HOME_WORK_QUEUE_CAPACITY);
        let (write_work_tx, write_work_rx) = mpsc::sync_channel::<Work>(WRITE_WORK_QUEUE_CAPACITY);
        let (stream_work_tx, stream_work_rx) =
            mpsc::sync_channel::<Work>(STREAM_WORK_QUEUE_CAPACITY);
        let stream_work_rx = Arc::new(Mutex::new(stream_work_rx));
        let home_metrics = Arc::new(QueueMetrics::new("home"));
        let write_metrics = Arc::new(QueueMetrics::new("write"));
        let stream_metrics = Arc::new(QueueMetrics::new("stream"));
        let home_collection_engine = Arc::clone(&collection_engine);
        let home_container_engine = Arc::clone(&container_engine);
        let home_worker_metrics = Arc::clone(&home_metrics);
        let home_worker = std::thread::Builder::new()
            .name("remote-home".to_owned())
            .spawn(move || {
                home_worker_loop(
                    home_work_rx,
                    &home_collection_engine,
                    &home_container_engine,
                    &home_worker_metrics,
                )
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
            let worker_queue = Arc::clone(&heavy_queue);
            let thumbnail_engine = Arc::clone(&thumbnail_engine);
            let container_engine = Arc::clone(&container_engine);
            let collection_engine = Arc::clone(&collection_engine);
            let session = session_handle.clone();
            workers.push(
                std::thread::Builder::new()
                    .name(format!("remote-thumb-{index}"))
                    .spawn(move || {
                        worker_loop(
                            &worker_queue,
                            &thumbnail_engine,
                            &container_engine,
                            &collection_engine,
                            &session,
                            index,
                        )
                    })
                    .map_err(|error| format!("remote IPC worker を開始できません: {error}"))?,
            );
        }
        let mut stream_workers = Vec::with_capacity(STREAM_WORKER_COUNT);
        for index in 0..STREAM_WORKER_COUNT {
            let work_rx = Arc::clone(&stream_work_rx);
            let engine = Arc::clone(&video_stream_engine);
            let session = session_handle.clone();
            let worker_metrics = Arc::clone(&stream_metrics);
            stream_workers.push(
                std::thread::Builder::new()
                    .name(format!("remote-stream-ipc-{index}"))
                    .spawn(move || {
                        stream_worker_loop(&work_rx, &engine, &session, &worker_metrics, index)
                    })
                    .map_err(|error| {
                        format!("remote IPC stream worker を開始できません: {error}")
                    })?,
            );
        }

        let stop = Arc::new(AtomicBool::new(false));
        let next_connection_id = Arc::new(AtomicU64::new(1));
        let mut listeners = Vec::with_capacity(ACCEPTOR_COUNT);
        for (index, initial_pipe) in initial_pipes.into_iter().enumerate() {
            let listener_stop = Arc::clone(&stop);
            let listener_heavy_queue = Arc::clone(&heavy_queue);
            let listener_home_tx = home_work_tx.clone();
            let listener_write_tx = write_work_tx.clone();
            let listener_stream_tx = stream_work_tx.clone();
            let listener_home_metrics = Arc::clone(&home_metrics);
            let listener_write_metrics = Arc::clone(&write_metrics);
            let listener_stream_metrics = Arc::clone(&stream_metrics);
            let listener_next_connection_id = Arc::clone(&next_connection_id);
            let listener_session = session_handle.clone();
            let listener_page_jobs = Arc::clone(&page_jobs);
            match std::thread::Builder::new()
                .name(format!("remote-ipc-listener-{index}"))
                .spawn(move || {
                    acceptor_loop(
                        listener_stop,
                        listener_heavy_queue,
                        listener_home_tx,
                        listener_write_tx,
                        listener_stream_tx,
                        listener_home_metrics,
                        listener_write_metrics,
                        listener_stream_metrics,
                        listener_next_connection_id,
                        listener_session,
                        listener_page_jobs,
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
                    respond_stopped_works(heavy_queue.stop(), "listener_start_failed");
                    let _ = home_work_tx.send(Work::Stop);
                    let _ = write_work_tx.send(Work::Stop);
                    for _ in 0..STREAM_WORKER_COUNT {
                        let _ = stream_work_tx.send(Work::Stop);
                    }
                    for worker in workers {
                        let _ = worker.join();
                    }
                    let _ = home_worker.join();
                    let _ = write_worker.join();
                    for worker in stream_workers {
                        let _ = worker.join();
                    }
                    return Err(format!("remote IPC listener を開始できません: {error}"));
                }
            }
        }

        crate::logger::log(format!(
            "remote_ipc: listening pipe={PIPE_NAME} protocol={PROTOCOL_VERSION} heavy_workers={worker_count} configured_workers={configured_worker_count} home_workers=1 write_workers=1 stream_workers={STREAM_WORKER_COUNT} heavy_foreground_capacity={} heavy_interactive_capacity={} heavy_prefetch_capacity={} acceptors={ACCEPTOR_COUNT} multiplexed=true",
            HEAVY_WORK_QUEUE_CAPACITIES.foreground,
            HEAVY_WORK_QUEUE_CAPACITIES.interactive,
            HEAVY_WORK_QUEUE_CAPACITIES.prefetch,
        ));
        Ok(Self {
            stop,
            listeners,
            workers,
            stream_workers,
            home_worker: Some(home_worker),
            write_worker: Some(write_worker),
            heavy_queue,
            home_work_tx,
            write_work_tx,
            stream_work_tx,
            session_runtime,
            _ai_jobs: ai_jobs,
            _archive_jobs: archive_jobs,
        })
    }

    pub(super) fn session_handle(&self) -> SessionHandle {
        self.session_runtime.handle()
    }
}

impl Drop for ServerGuard {
    fn drop(&mut self) {
        respond_stopped_works(self.heavy_queue.stop(), "service_stopping");
        self.stop.store(true, Ordering::Release);
        for _ in 0..self.listeners.len() {
            poke_listener();
        }
        for listener in self.listeners.drain(..) {
            let _ = listener.join();
        }
        let _ = self.home_work_tx.send(Work::Stop);
        let _ = self.write_work_tx.send(Work::Stop);
        for _ in 0..self.stream_workers.len() {
            let _ = self.stream_work_tx.send(Work::Stop);
        }
        for worker in self.workers.drain(..) {
            let _ = worker.join();
        }
        if let Some(worker) = self.home_worker.take() {
            let _ = worker.join();
        }
        if let Some(worker) = self.write_worker.take() {
            let _ = worker.join();
        }
        for worker in self.stream_workers.drain(..) {
            let _ = worker.join();
        }
        crate::logger::log("remote_ipc: stopped".to_owned());
    }
}

fn remote_heavy_worker_count(configured_worker_count: usize) -> usize {
    // 利用者の並列数設定をそのまま使う。かつては「ローカル表示用 worker と CPU / disk を
    // 奪い合わないよう」半分かつ最大 3 本に絞っていたが、奪い合う相手が居ない。
    // session は排他で、リモートが操作権を持つ間は本体の通常入力がロックされるため
    // (plan §2.2)、その間ローカルの表示 worker は動いていない。remote 専用の割引を
    // 持たないほうが、設定と実際の並列数が一致して保守しやすい。
    //
    // 上限が要る場面は remote-web 側が持つ (`MAX_CONCURRENT_PAGE_PREFETCH`)。
    // PDF は下流の worker プールが 4 本で頭打ちになるので、ここを増やしても
    // 実際に同時 raster される数はそれ以上には増えない。
    configured_worker_count.max(1)
}

fn worker_loop(
    work_queue: &HeavyQueueWiring,
    thumbnail_engine: &ThumbnailEngine,
    container_engine: &ContainerEngine,
    collection_engine: &CollectionEngine,
    session: &SessionHandle,
    worker_index: usize,
) {
    crate::logger::log(format!(
        "remote_ipc: worker_started queue=heavy worker={worker_index}"
    ));
    let context = WorkerContext::open();
    loop {
        let Some(item) = work_queue.pop() else {
            break;
        };
        let lane = item.lane();
        let (key, work, item_lane) = item.into_parts();
        debug_assert_eq!(lane, item_lane);
        match work {
            Work::Request {
                message,
                reply,
                enqueued_at,
                session_operation,
                page_job,
            } => execute_work(
                message,
                reply,
                enqueued_at,
                ExecutionQueue::Heavy(HeavyCompletionGuard::new(
                    work_queue,
                    key,
                    lane,
                    page_job_key(page_job.as_ref()),
                )),
                &format!("heavy-{worker_index}"),
                session_operation,
                page_job,
                |message, _session_cancel, page_job| match message {
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
                    ClientMessage::FavoriteSearch { id, request, .. } => {
                        ServerMessage::FavoriteSearch {
                            id,
                            response: collection_engine.favorite_search(request),
                        }
                    }
                    ClientMessage::TagBrowse { id, request, .. } => ServerMessage::TagBrowse {
                        id,
                        response: collection_engine.tag_browse(request),
                    },
                    ClientMessage::TagItems { id, request, .. } => ServerMessage::TagItems {
                        id,
                        response: collection_engine.tag_items(request),
                    },
                    ClientMessage::FolderList { id, request, .. } => ServerMessage::FolderList {
                        id,
                        response: container_engine.folder_list(request),
                    },
                    ClientMessage::Container { id, request, .. } => ServerMessage::Container {
                        id,
                        response: container_engine.container(request),
                    },
                    ClientMessage::Page {
                        id, mut request, ..
                    } => {
                        let page_job = page_job.expect("page work carries its registry lease");
                        request.priority = effective_page_priority(page_job);
                        ServerMessage::Page {
                            id,
                            response: container_engine.page_with_job_cancel(
                                request,
                                &context,
                                Arc::clone(&page_job.cancel),
                            ),
                        }
                    }
                    ClientMessage::VideoStreamJumpList {
                        id,
                        session: stream_session,
                        ..
                    } => ServerMessage::VideoStreamJumpList {
                        id,
                        response: match session.video_stream(stream_session) {
                            Ok(stream) => VideoStreamResult::Success(stream.jump_catalog.list()),
                            Err(error) => VideoStreamResult::Error(error),
                        },
                    },
                    ClientMessage::VideoStreamJumpThumbnail {
                        id,
                        session: stream_session,
                        token,
                        ..
                    } => ServerMessage::VideoStreamJumpThumbnail {
                        id,
                        response: if token.is_empty() || token.len() > 256 {
                            VideoStreamResult::Error(VideoStreamError::new(
                                VideoStreamErrorCode::BadRequest,
                                "ジャンプサムネイル token が不正です",
                            ))
                        } else {
                            match session.video_stream(stream_session) {
                                Ok(stream) => VideoStreamResult::Success(
                                    stream.jump_catalog.thumbnail(&token),
                                ),
                                Err(error) => VideoStreamResult::Error(error),
                            }
                        },
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
                    | ClientMessage::SessionRelease { id, .. }
                    | ClientMessage::SessionActivity { id, .. } => ServerMessage::Session {
                        id,
                        response: session_response(
                            SessionStatus::NotAcquired,
                            "session control request was routed to a worker",
                        ),
                    },
                    other @ (ClientMessage::VideoStreamStart { .. }
                    | ClientMessage::PageDemand { .. }
                    | ClientMessage::VideoStreamControl { .. }
                    | ClientMessage::VideoStreamSeek { .. }
                    | ClientMessage::VideoStreamThumbnail { .. }
                    | ClientMessage::VideoStreamPlaylist { .. }
                    | ClientMessage::VideoStreamSegment { .. }
                    | ClientMessage::VideoStreamState { .. }
                    | ClientMessage::VideoStreamStop { .. }
                    | ClientMessage::RemoteAiStart { .. }
                    | ClientMessage::RemoteAiState { .. }
                    | ClientMessage::RemoteAiRecoverable { .. }
                    | ClientMessage::RemoteAiCancel { .. }
                    | ClientMessage::RemoteAiResult { .. }
                    | ClientMessage::RemoteArchiveStart { .. }
                    | ClientMessage::RemoteArchiveState { .. }
                    | ClientMessage::RemoteArchiveRecoverable { .. }
                    | ClientMessage::RemoteArchiveCancel { .. }
                    | ClientMessage::RemoteArchiveConfirm { .. }
                    | ClientMessage::RemoteArchivePassword { .. }
                    | ClientMessage::RemoteArchiveResult { .. }) => {
                        service_stopped_response(&other)
                    }
                },
            ),
            Work::Stop => unreachable!(),
        }
    }
    crate::logger::log(format!(
        "remote_ipc: worker_stopped queue=heavy worker={worker_index}"
    ));
}

fn home_worker_loop(
    work_rx: mpsc::Receiver<Work>,
    collection_engine: &CollectionEngine,
    container_engine: &ContainerEngine,
    metrics: &QueueMetrics,
) {
    crate::logger::log("remote_ipc: worker_started queue=home worker=home-0".to_owned());
    loop {
        match work_rx.recv() {
            Ok(Work::Request {
                message,
                reply,
                enqueued_at,
                session_operation,
                page_job,
            }) => execute_work(
                message,
                reply,
                enqueued_at,
                ExecutionQueue::Metrics(metrics),
                "home-0",
                session_operation,
                page_job,
                |message, _cancel, _page_job| match message {
                    ClientMessage::Home { id, .. } => ServerMessage::Home {
                        id,
                        response: collection_engine.home(),
                    },
                    ClientMessage::FolderList { id, request, .. } => ServerMessage::FolderList {
                        id,
                        response: container_engine.folder_list(request),
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
        let response = if let Err(response) = session_operation.wait_until_active() {
            ServerMessage::Session {
                id: request_id,
                response,
            }
        } else {
            session_operation.started();
            match message {
                ClientMessage::Write {
                    id, mut request, ..
                } => {
                    if matches!(request, RemoteWriteRequest::ListBookBookmarks { .. }) {
                        let response = container_engine.book_bookmarks(&mut request);
                        finish_direct_write_request(id, session_operation, response)
                    } else if let Err(error) = container_engine.validate_write_request(&mut request)
                    {
                        session_operation.finish(false);
                        ServerMessage::Write {
                            id,
                            response: RemoteWriteResponse::Error(error),
                        }
                    } else {
                        match session.submit_write(request, session_operation) {
                            UiWriteOutcome::Write(response) => {
                                ServerMessage::Write { id, response }
                            }
                            UiWriteOutcome::Session(response) => {
                                ServerMessage::Session { id, response }
                            }
                        }
                    }
                }
                other => {
                    session_operation.finish(false);
                    service_stopped_response(&other)
                }
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

fn finish_direct_write_request(
    id: mimageviewer_ipc::RequestId,
    operation: SessionOperation,
    response: RemoteWriteResponse,
) -> ServerMessage {
    let ownership = operation.ownership_response();
    let response = if ownership.status == SessionStatus::Active {
        ServerMessage::Write { id, response }
    } else {
        ServerMessage::Session {
            id,
            response: ownership,
        }
    };
    operation.finish(response_outcome(&response) == "ok");
    response
}

fn stream_worker_loop(
    work_rx: &Mutex<mpsc::Receiver<Work>>,
    engine: &VideoStreamEngine,
    session: &SessionHandle,
    metrics: &QueueMetrics,
    worker_index: usize,
) {
    crate::logger::log(format!(
        "remote_ipc: worker_started queue=stream worker=stream-{worker_index}"
    ));
    loop {
        let work = {
            let receiver = work_rx.lock().unwrap_or_else(|error| error.into_inner());
            receiver.recv()
        };
        let Ok(Work::Request {
            message,
            reply,
            enqueued_at,
            session_operation,
            ..
        }) = work
        else {
            break;
        };
        let request_id = message.id();
        let kind = request_kind(&message);
        let (queued, active) = metrics.started();
        crate::logger::log(format!(
            "remote_ipc: worker_start request_id={request_id} kind={kind} queue=stream worker=stream-{worker_index} queue_wait_ms={:.1} queued={queued} active={active}",
            enqueued_at.elapsed().as_secs_f64() * 1000.0
        ));
        let started_at = Instant::now();
        let start_budget = matches!(&message, ClientMessage::VideoStreamStart { .. })
            .then(|| VideoStreamStartBudget::from_enqueued_at(enqueued_at));
        let activation = match start_budget {
            Some(budget) => session_operation.wait_until_active_for(budget.remaining()),
            None => session_operation.wait_until_active().map(|()| true),
        };
        let response = match activation {
            Err(response) => ServerMessage::Session {
                id: request_id,
                response,
            },
            Ok(false) => {
                session_operation.finish(false);
                ServerMessage::VideoStreamStart {
                    id: request_id,
                    response: VideoStreamResult::Error(
                        start_budget
                            .expect("only video start has a bounded activation wait")
                            .timeout_error(VideoStreamStartStage::Queue),
                    ),
                }
            }
            Ok(true) => {
                session_operation.started();
                execute_video_stream_request(
                    message,
                    engine,
                    session,
                    session_operation,
                    enqueued_at,
                )
            }
        };
        let outcome = response_outcome(&response);
        let reply_ok = reply.send(response).is_ok();
        let (queued, active) = metrics.finished();
        crate::logger::log(format!(
            "remote_ipc: worker_finish request_id={request_id} kind={kind} queue=stream worker=stream-{worker_index} outcome={outcome} duration_ms={:.1} reply_ok={reply_ok} queued={queued} active={active}",
            started_at.elapsed().as_secs_f64() * 1000.0
        ));
    }
    crate::logger::log(format!(
        "remote_ipc: worker_stopped queue=stream worker=stream-{worker_index}"
    ));
}

fn execute_video_stream_request(
    message: ClientMessage,
    engine: &VideoStreamEngine,
    session: &SessionHandle,
    operation: SessionOperation,
    enqueued_at: Instant,
) -> ServerMessage {
    match message {
        ClientMessage::VideoStreamStart {
            id,
            owner,
            address,
            quality,
        } => {
            let budget = VideoStreamStartBudget::from_enqueued_at(enqueued_at);
            if let Some(error) = budget.expired_error(VideoStreamStartStage::Queue) {
                operation.finish(false);
                return ServerMessage::VideoStreamStart {
                    id,
                    response: VideoStreamResult::Error(error),
                };
            }
            match engine.resolve_start_address(&address) {
                Ok(path) => match session.submit_video_stream(
                    VideoStreamUiRequest::Start {
                        owner,
                        path,
                        quality,
                        budget,
                    },
                    operation,
                ) {
                    VideoStreamUiOutcome::Started(stream) => ServerMessage::VideoStreamStart {
                        id,
                        response: engine.complete_start(stream, budget),
                    },
                    VideoStreamUiOutcome::Error(error) => ServerMessage::VideoStreamStart {
                        id,
                        response: VideoStreamResult::Error(error),
                    },
                    _ => ServerMessage::VideoStreamStart {
                        id,
                        response: unexpected_video_outcome("start"),
                    },
                },
                Err(error) => {
                    operation.finish(false);
                    ServerMessage::VideoStreamStart {
                        id,
                        response: VideoStreamResult::Error(error),
                    }
                }
            }
        }
        ClientMessage::VideoStreamControl {
            id,
            session: stream_session,
            action,
            ..
        } => match session.submit_video_stream(
            VideoStreamUiRequest::Control {
                session: stream_session,
                action,
            },
            operation,
        ) {
            VideoStreamUiOutcome::Controlled(response) => ServerMessage::VideoStreamControl {
                id,
                response: VideoStreamResult::Success(response),
            },
            VideoStreamUiOutcome::Error(error) => ServerMessage::VideoStreamControl {
                id,
                response: VideoStreamResult::Error(error),
            },
            _ => ServerMessage::VideoStreamControl {
                id,
                response: unexpected_video_outcome("control"),
            },
        },
        ClientMessage::VideoStreamSeek {
            id,
            session: stream_session,
            position_secs,
            ..
        } => match session.submit_video_stream(
            VideoStreamUiRequest::Seek {
                session: stream_session,
                position_secs,
            },
            operation,
        ) {
            VideoStreamUiOutcome::Seeked(generation) => ServerMessage::VideoStreamSeek {
                id,
                response: VideoStreamResult::Success(mimageviewer_ipc::VideoStreamSeekPayload {
                    generation: generation.0,
                }),
            },
            VideoStreamUiOutcome::Error(error) => ServerMessage::VideoStreamSeek {
                id,
                response: VideoStreamResult::Error(error),
            },
            _ => ServerMessage::VideoStreamSeek {
                id,
                response: unexpected_video_outcome("seek"),
            },
        },
        ClientMessage::VideoStreamThumbnail {
            id,
            session: stream_session,
            position_secs,
            ..
        } => match session.submit_video_stream(
            VideoStreamUiRequest::Thumbnail {
                session: stream_session,
                position_secs,
            },
            operation,
        ) {
            VideoStreamUiOutcome::ThumbnailPending => ServerMessage::VideoStreamThumbnail {
                id,
                response: VideoStreamResult::Success(VideoStreamThumbnailPayload::Pending),
            },
            VideoStreamUiOutcome::ThumbnailReady(thumbnail) => {
                ServerMessage::VideoStreamThumbnail {
                    id,
                    response: encode_video_stream_thumbnail(thumbnail),
                }
            }
            VideoStreamUiOutcome::ThumbnailCleared => ServerMessage::VideoStreamThumbnail {
                id,
                response: VideoStreamResult::Success(VideoStreamThumbnailPayload::Cleared),
            },
            VideoStreamUiOutcome::Error(error) => ServerMessage::VideoStreamThumbnail {
                id,
                response: VideoStreamResult::Error(error),
            },
            _ => ServerMessage::VideoStreamThumbnail {
                id,
                response: unexpected_video_outcome("thumbnail"),
            },
        },
        ClientMessage::VideoStreamStop {
            id,
            session: stream_session,
            ..
        } => match session.submit_video_stream(
            VideoStreamUiRequest::Stop {
                session: stream_session,
            },
            operation,
        ) {
            VideoStreamUiOutcome::Stopped => ServerMessage::VideoStreamStop {
                id,
                response: VideoStreamResult::Success(()),
            },
            VideoStreamUiOutcome::Error(error) => ServerMessage::VideoStreamStop {
                id,
                response: VideoStreamResult::Error(error),
            },
            _ => ServerMessage::VideoStreamStop {
                id,
                response: unexpected_video_outcome("stop"),
            },
        },
        ClientMessage::VideoStreamPlaylist {
            id,
            session: stream_session,
            generation,
            kind,
            ..
        } => finish_direct_video_request(
            id,
            operation,
            ServerMessage::VideoStreamPlaylist {
                id,
                response: engine.playlist(session, stream_session, generation, kind),
            },
        ),
        ClientMessage::VideoStreamSegment {
            id,
            session: stream_session,
            generation,
            index,
            ..
        } => finish_direct_video_request(
            id,
            operation,
            ServerMessage::VideoStreamSegment {
                id,
                response: engine.segment(session, stream_session, generation, index),
            },
        ),
        ClientMessage::VideoStreamState {
            id,
            session: stream_session,
            ..
        } => finish_direct_video_request(
            id,
            operation,
            ServerMessage::VideoStreamState {
                id,
                response: engine.state(session, stream_session),
            },
        ),
        other => {
            operation.finish(false);
            service_stopped_response(&other)
        }
    }
}

fn finish_direct_video_request(
    id: mimageviewer_ipc::RequestId,
    operation: SessionOperation,
    response: ServerMessage,
) -> ServerMessage {
    let ownership = operation.ownership_response();
    let response = if ownership.status == SessionStatus::Active {
        response
    } else {
        ServerMessage::Session {
            id,
            response: ownership,
        }
    };
    operation.finish(response_outcome(&response) == "ok");
    response
}

fn unexpected_video_outcome<T>(operation: &str) -> VideoStreamResult<T> {
    VideoStreamResult::Error(VideoStreamError::new(
        VideoStreamErrorCode::Internal,
        format!("動画 {operation} 応答の型が一致しません"),
    ))
}

fn encode_video_stream_thumbnail(
    thumbnail: crate::video::thumbnail::Thumbnail,
) -> VideoStreamResult<VideoStreamThumbnailPayload> {
    let expected_len = usize::try_from(thumbnail.width)
        .ok()
        .and_then(|width| {
            usize::try_from(thumbnail.height)
                .ok()
                .and_then(|height| width.checked_mul(height))
        })
        .and_then(|pixels| pixels.checked_mul(4));
    if expected_len != Some(thumbnail.rgba.len()) {
        return VideoStreamResult::Error(VideoStreamError::new(
            VideoStreamErrorCode::Internal,
            "seek thumbnail RGBA dimensions do not match its payload",
        ));
    }
    let webp_bytes =
        webp::Encoder::from_rgba(thumbnail.rgba.as_slice(), thumbnail.width, thumbnail.height)
            .encode(75.0)
            .to_vec();
    if webp_bytes.is_empty() {
        return VideoStreamResult::Error(VideoStreamError::new(
            VideoStreamErrorCode::Internal,
            "seek thumbnail WebP encoding returned an empty payload",
        ));
    }
    VideoStreamResult::Success(VideoStreamThumbnailPayload::Ready {
        actual_pts_secs: thumbnail.target_secs,
        width: thumbnail.width,
        height: thumbnail.height,
        webp_bytes,
    })
}

fn execute_work(
    message: ClientMessage,
    reply: mpsc::Sender<ServerMessage>,
    enqueued_at: Instant,
    mut queue: ExecutionQueue<'_>,
    worker: &str,
    session_operation: SessionOperation,
    page_job: Option<PageJobWork>,
    handler: impl FnOnce(ClientMessage, Arc<AtomicBool>, Option<&PageJobWork>) -> ServerMessage,
) {
    let request_id = message.id();
    let request_kind = match (&message, page_job.as_ref()) {
        (ClientMessage::Page { .. }, Some(page_job)) => match effective_page_priority(page_job) {
            PagePriority::Foreground => "page_foreground",
            PagePriority::Prefetch => "page_prefetch",
        },
        _ => request_kind(&message),
    };
    let queue_name = queue.name();
    let queue_state = queue.started();
    let queued = queue_state.queued;
    let active = queue_state.active;
    let lane_fields = &queue_state.lane_fields;
    let registry_fields = page_registry_log_fields(page_job.as_ref());
    crate::logger::log(format!(
        "remote_ipc: worker_start request_id={request_id} kind={request_kind} queue={queue_name} worker={worker} queue_wait_ms={:.1} queued={queued} active={active}{lane_fields}{registry_fields}",
        enqueued_at.elapsed().as_secs_f64() * 1000.0
    ));
    let started_at = Instant::now();
    let response = if page_job
        .as_ref()
        .is_some_and(|page_job| page_job.cancel.load(Ordering::Acquire))
    {
        cancelled_page_response(request_id)
    } else if let Err(response) = session_operation.wait_until_active() {
        if page_job
            .as_ref()
            .is_some_and(|page_job| page_job.cancel.load(Ordering::Acquire))
        {
            cancelled_page_response(request_id)
        } else {
            ServerMessage::Session {
                id: request_id,
                response,
            }
        }
    } else {
        session_operation.started();
        handler(message, session_operation.cancel_flag(), page_job.as_ref())
    };
    let ownership = session_operation.ownership_response();
    let response =
        if is_cancelled_page_response(&response) || ownership.status == SessionStatus::Active {
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
    let queue_state = queue.finished();
    let queued = queue_state.queued;
    let active = queue_state.active;
    let lane_fields = &queue_state.lane_fields;
    let registry_fields = page_registry_log_fields(page_job.as_ref());
    crate::logger::log(format!(
        "remote_ipc: worker_complete request_id={request_id} kind={request_kind} queue={queue_name} worker={worker} outcome={outcome} duration_ms={:.1} reply_ok={reply_ok} queued={queued} active={active}{lane_fields}{registry_fields}",
        started_at.elapsed().as_secs_f64() * 1000.0
    ));
}

fn effective_page_priority(page_job: &PageJobWork) -> PagePriority {
    match page_job
        .registry
        .priority(page_job.connection_id, &page_job.job_id)
    {
        Some(PageJobPriority::Foreground) => PagePriority::Foreground,
        Some(PageJobPriority::Prefetch) | None => PagePriority::Prefetch,
    }
}

fn cancelled_page_response(id: u64) -> ServerMessage {
    ServerMessage::Page {
        id,
        response: PageResponse::Error(MediaError::new(
            MediaErrorCode::Cancelled,
            "ページの表示需要がなくなったため処理を取り消しました",
        )),
    }
}

fn is_cancelled_page_response(response: &ServerMessage) -> bool {
    matches!(
        response,
        ServerMessage::Page {
            response: PageResponse::Error(MediaError {
                code: MediaErrorCode::Cancelled,
                ..
            }),
            ..
        }
    )
}

fn page_registry_log_fields(page_job: Option<&PageJobWork>) -> String {
    let Some(page_job) = page_job else {
        return String::new();
    };
    let snapshot = page_job.registry.snapshot();
    let display_request_present = page_job
        .registry
        .display_request_id(page_job.connection_id, &page_job.job_id)
        .is_some();
    format!(
        " page_jobs_active={} page_jobs_released={} page_jobs_prefetch_active={} page_jobs_foreground_active={} page_jobs_total={} page_display_request_present={display_request_present}",
        snapshot.active(),
        snapshot.released(),
        snapshot.prefetch_active,
        snapshot.foreground_active,
        snapshot.total(),
    )
}

fn request_kind(message: &ClientMessage) -> &'static str {
    match message {
        ClientMessage::RemoteWebConnectionInfo { .. } => "connection_info",
        ClientMessage::SessionAcquire { .. } => "session_acquire",
        ClientMessage::SessionPing { .. } => "session_ping",
        ClientMessage::SessionRelease { .. } => "session_release",
        ClientMessage::SessionActivity { .. } => "session_activity",
        ClientMessage::Thumbnail { .. } => "thumbnail",
        ClientMessage::Home { .. } => "home",
        ClientMessage::Collection { .. } => "collection",
        ClientMessage::FavoriteSearch { .. } => "favorite_search",
        ClientMessage::TagBrowse { .. } => "tag_browse",
        ClientMessage::TagItems { .. } => "tag_items",
        ClientMessage::FolderList { .. } => "folder_list",
        ClientMessage::Container { .. } => "container",
        ClientMessage::Page { request, .. } => match request.priority {
            PagePriority::Foreground => "page_foreground",
            PagePriority::Prefetch => "page_prefetch",
        },
        ClientMessage::PageDemand { .. } => "page_demand",
        ClientMessage::RemoteAiStart { .. } => "remote_ai_start",
        ClientMessage::RemoteAiState { .. } => "remote_ai_state",
        ClientMessage::RemoteAiRecoverable { .. } => "remote_ai_recoverable",
        ClientMessage::RemoteAiCancel { .. } => "remote_ai_cancel",
        ClientMessage::RemoteAiResult { .. } => "remote_ai_result",
        ClientMessage::RemoteArchiveStart { .. } => "remote_archive_start",
        ClientMessage::RemoteArchiveState { .. } => "remote_archive_state",
        ClientMessage::RemoteArchiveRecoverable { .. } => "remote_archive_recoverable",
        ClientMessage::RemoteArchiveCancel { .. } => "remote_archive_cancel",
        ClientMessage::RemoteArchiveConfirm { .. } => "remote_archive_confirm",
        ClientMessage::RemoteArchivePassword { .. } => "remote_archive_password",
        ClientMessage::RemoteArchiveResult { .. } => "remote_archive_result",
        ClientMessage::Write { .. } => "write",
        ClientMessage::VideoStreamStart { .. } => "video_stream_start",
        ClientMessage::VideoStreamControl { .. } => "video_stream_control",
        ClientMessage::VideoStreamSeek { .. } => "video_stream_seek",
        ClientMessage::VideoStreamThumbnail { .. } => "video_stream_thumbnail",
        ClientMessage::VideoStreamJumpList { .. } => "video_stream_jump_list",
        ClientMessage::VideoStreamJumpThumbnail { .. } => "video_stream_jump_thumbnail",
        ClientMessage::VideoStreamPlaylist { .. } => "video_stream_playlist",
        ClientMessage::VideoStreamSegment { .. } => "video_stream_segment",
        ClientMessage::VideoStreamState { .. } => "video_stream_state",
        ClientMessage::VideoStreamStop { .. } => "video_stream_stop",
    }
}

fn work_lane(message: &ClientMessage) -> WorkLane {
    match message {
        ClientMessage::Home { .. } | ClientMessage::FolderList { .. } => WorkLane::Home,
        ClientMessage::Write { .. } => WorkLane::Write,
        ClientMessage::VideoStreamStart { .. }
        | ClientMessage::VideoStreamControl { .. }
        | ClientMessage::VideoStreamSeek { .. }
        | ClientMessage::VideoStreamThumbnail { .. }
        | ClientMessage::VideoStreamPlaylist { .. }
        | ClientMessage::VideoStreamSegment { .. }
        | ClientMessage::VideoStreamState { .. }
        | ClientMessage::VideoStreamStop { .. } => WorkLane::Stream,
        ClientMessage::VideoStreamJumpList { .. }
        | ClientMessage::VideoStreamJumpThumbnail { .. } => WorkLane::Heavy,
        ClientMessage::RemoteAiStart { .. }
        | ClientMessage::RemoteAiState { .. }
        | ClientMessage::RemoteAiRecoverable { .. }
        | ClientMessage::RemoteAiCancel { .. }
        | ClientMessage::RemoteAiResult { .. } => WorkLane::Heavy,
        _ => WorkLane::Heavy,
    }
}

fn message_owner(message: &ClientMessage) -> Option<&RemoteSessionIdentity> {
    match message {
        ClientMessage::RemoteWebConnectionInfo { .. } | ClientMessage::SessionAcquire { .. } => {
            None
        }
        ClientMessage::SessionPing { request, .. } => Some(&request.owner),
        ClientMessage::SessionRelease { request, .. } => Some(&request.owner),
        ClientMessage::Thumbnail { owner, .. }
        | ClientMessage::Home { owner, .. }
        | ClientMessage::Collection { owner, .. }
        | ClientMessage::FavoriteSearch { owner, .. }
        | ClientMessage::TagBrowse { owner, .. }
        | ClientMessage::TagItems { owner, .. }
        | ClientMessage::Container { owner, .. }
        | ClientMessage::FolderList { owner, .. }
        | ClientMessage::Page { owner, .. }
        | ClientMessage::PageDemand { owner, .. }
        | ClientMessage::RemoteAiStart { owner, .. }
        | ClientMessage::RemoteAiState { owner, .. }
        | ClientMessage::RemoteAiRecoverable { owner, .. }
        | ClientMessage::RemoteAiCancel { owner, .. }
        | ClientMessage::RemoteAiResult { owner, .. }
        | ClientMessage::RemoteArchiveStart { owner, .. }
        | ClientMessage::RemoteArchiveState { owner, .. }
        | ClientMessage::RemoteArchiveRecoverable { owner, .. }
        | ClientMessage::RemoteArchiveCancel { owner, .. }
        | ClientMessage::RemoteArchiveConfirm { owner, .. }
        | ClientMessage::RemoteArchivePassword { owner, .. }
        | ClientMessage::RemoteArchiveResult { owner, .. }
        | ClientMessage::Write { owner, .. }
        | ClientMessage::VideoStreamStart { owner, .. }
        | ClientMessage::VideoStreamControl { owner, .. }
        | ClientMessage::VideoStreamSeek { owner, .. }
        | ClientMessage::VideoStreamThumbnail { owner, .. }
        | ClientMessage::VideoStreamJumpList { owner, .. }
        | ClientMessage::VideoStreamJumpThumbnail { owner, .. }
        | ClientMessage::VideoStreamPlaylist { owner, .. }
        | ClientMessage::VideoStreamSegment { owner, .. }
        | ClientMessage::VideoStreamState { owner, .. }
        | ClientMessage::VideoStreamStop { owner, .. }
        | ClientMessage::SessionActivity { owner, .. } => Some(owner),
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
        ClientMessage::FavoriteSearch { .. } => "お気に入りを検索中".to_owned(),
        ClientMessage::TagBrowse { .. } => "タグ一覧を読み込み中".to_owned(),
        ClientMessage::TagItems { .. } => "タグの項目を検索中".to_owned(),
        ClientMessage::Container { .. } => "コンテナを列挙中".to_owned(),
        ClientMessage::FolderList { .. } => "フォルダ一覧を読み込み中".to_owned(),
        ClientMessage::Page { request, .. } => match request.address.subresource {
            mimageviewer_ipc::RemoteSubresource::PdfPage { page_number } => {
                format!("PDF {} ページ目をレンダリング中", page_number + 1)
            }
            mimageviewer_ipc::RemoteSubresource::ZipEntry { .. } => {
                "ZIP ページをレンダリング中".to_owned()
            }
            _ => "ページをレンダリング中".to_owned(),
        },
        ClientMessage::PageDemand { .. } => "ページ表示の需要を更新中".to_owned(),
        ClientMessage::RemoteAiStart { .. } => "remote AI job を開始中".to_owned(),
        ClientMessage::RemoteAiState { .. } => "remote AI job の状態を確認中".to_owned(),
        ClientMessage::RemoteAiRecoverable { .. } => "復帰可能な remote AI job を確認中".to_owned(),
        ClientMessage::RemoteAiCancel { .. } => "remote AI job を取り消し中".to_owned(),
        ClientMessage::RemoteAiResult { .. } => "remote AI result を取得中".to_owned(),
        ClientMessage::RemoteArchiveStart { .. } => "アーカイブ準備を開始中".to_owned(),
        ClientMessage::RemoteArchiveState { .. } => "アーカイブ準備の状態を確認中".to_owned(),
        ClientMessage::RemoteArchiveRecoverable { .. } => {
            "復帰可能なアーカイブ操作を確認中".to_owned()
        }
        ClientMessage::RemoteArchiveCancel { .. } => "アーカイブ操作を取り消し中".to_owned(),
        ClientMessage::RemoteArchiveConfirm { .. } => "アーカイブ変換の確認を送信中".to_owned(),
        ClientMessage::RemoteArchivePassword { .. } => "アーカイブ認証情報を送信中".to_owned(),
        ClientMessage::RemoteArchiveResult { .. } => "アーカイブ準備結果を取得中".to_owned(),
        ClientMessage::Write { request, .. } => match request {
            RemoteWriteRequest::SetSpread { .. } => "見開き設定を書き込み中",
            RemoteWriteRequest::RecordReadingProgress { .. } => "読書位置を記録中",
            RemoteWriteRequest::SetRating { .. } => "レーティングを書き込み中",
            RemoteWriteRequest::SetBookmark { .. } => "ブックマークを書き込み中",
            RemoteWriteRequest::GetItemState { .. } => "ページ情報を確認中",
            RemoteWriteRequest::ListBookBookmarks { .. } => "ブックマーク一覧を確認中",
            RemoteWriteRequest::SetBookBookmarkTitle { .. } => "ブックマーク名を書き込み中",
            RemoteWriteRequest::RemoveBookBookmark { .. } => "ブックマークを削除中",
            RemoteWriteRequest::SetAdjustment { .. } => "画像補正を書き込み中",
            RemoteWriteRequest::GetAdjustmentState { .. } => "画像補正を確認中",
            RemoteWriteRequest::SetViewTrim { .. } => "表示トリムを書き込み中",
            RemoteWriteRequest::GetViewTrimState { .. } => "表示トリムを確認中",
            RemoteWriteRequest::SetSortOrder { .. } => "並べ替えを書き込み中",
        }
        .to_owned(),
        ClientMessage::VideoStreamStart { .. } => "動画ストリーミングを開始中".to_owned(),
        ClientMessage::VideoStreamControl { .. } => "動画ストリーミングを操作中".to_owned(),
        ClientMessage::VideoStreamSeek { .. } => "動画ストリーミングをシーク中".to_owned(),
        ClientMessage::VideoStreamThumbnail { .. } => "動画シークプレビューを取得中".to_owned(),
        ClientMessage::VideoStreamJumpList { .. } => "動画ジャンプ一覧を取得中".to_owned(),
        ClientMessage::VideoStreamJumpThumbnail { .. } => {
            "動画ジャンプサムネイルを取得中".to_owned()
        }
        ClientMessage::VideoStreamPlaylist { .. } => "動画プレイリストを取得中".to_owned(),
        ClientMessage::VideoStreamSegment { .. } => "動画セグメントを取得中".to_owned(),
        ClientMessage::VideoStreamState { .. } => "動画ストリーミング状態を取得中".to_owned(),
        ClientMessage::VideoStreamStop { .. } => "動画ストリーミングを停止中".to_owned(),
        ClientMessage::SessionAcquire { .. }
        | ClientMessage::SessionPing { .. }
        | ClientMessage::SessionRelease { .. }
        | ClientMessage::SessionActivity { .. } => "接続を確認中".to_owned(),
    }
}

fn session_response(status: SessionStatus, message: impl Into<String>) -> SessionResponse {
    SessionResponse {
        status,
        message: message.into(),
        session_id: None,
    }
}

fn ai_session_error(response: SessionResponse) -> mimageviewer_ipc::RemoteAiJobError {
    mimageviewer_ipc::RemoteAiJobError::new(
        mimageviewer_ipc::RemoteAiJobErrorCode::SessionClosing,
        response.message,
    )
}

fn ai_stopped_error() -> mimageviewer_ipc::RemoteAiJobError {
    mimageviewer_ipc::RemoteAiJobError::new(
        mimageviewer_ipc::RemoteAiJobErrorCode::Internal,
        "remote AI job registry is not available",
    )
}

fn ai_busy_error() -> mimageviewer_ipc::RemoteAiJobError {
    mimageviewer_ipc::RemoteAiJobError::new(
        mimageviewer_ipc::RemoteAiJobErrorCode::Internal,
        "remote AI queue is busy",
    )
}

fn ai_start_stopped() -> mimageviewer_ipc::RemoteAiStartResponse {
    mimageviewer_ipc::RemoteAiStartResponse::Error(ai_stopped_error())
}
fn ai_state_stopped() -> mimageviewer_ipc::RemoteAiStateResponse {
    mimageviewer_ipc::RemoteAiStateResponse::Error(ai_stopped_error())
}
fn ai_recoverable_stopped() -> mimageviewer_ipc::RemoteAiRecoverableResponse {
    mimageviewer_ipc::RemoteAiRecoverableResponse::Error(ai_stopped_error())
}
fn ai_cancel_stopped() -> mimageviewer_ipc::RemoteAiCancelResponse {
    mimageviewer_ipc::RemoteAiCancelResponse::Error(ai_stopped_error())
}
fn ai_result_stopped() -> mimageviewer_ipc::RemoteAiResultResponse {
    mimageviewer_ipc::RemoteAiResultResponse::Error(ai_stopped_error())
}

fn archive_session_error(response: SessionResponse) -> mimageviewer_ipc::RemoteArchiveJobError {
    mimageviewer_ipc::RemoteArchiveJobError::new(
        mimageviewer_ipc::RemoteArchiveJobErrorCode::SessionClosing,
        response.message,
    )
}

fn archive_stopped_error() -> mimageviewer_ipc::RemoteArchiveJobError {
    mimageviewer_ipc::RemoteArchiveJobError::new(
        mimageviewer_ipc::RemoteArchiveJobErrorCode::Internal,
        "remote archive job registry is not available",
    )
}

fn archive_busy_error() -> mimageviewer_ipc::RemoteArchiveJobError {
    mimageviewer_ipc::RemoteArchiveJobError::new(
        mimageviewer_ipc::RemoteArchiveJobErrorCode::Internal,
        "remote archive queue is busy",
    )
}

fn archive_start_stopped() -> mimageviewer_ipc::RemoteArchiveStartResponse {
    mimageviewer_ipc::RemoteArchiveStartResponse::Error(archive_stopped_error())
}
fn archive_state_stopped() -> mimageviewer_ipc::RemoteArchiveStateResponse {
    mimageviewer_ipc::RemoteArchiveStateResponse::Error(archive_stopped_error())
}
fn archive_recoverable_stopped() -> mimageviewer_ipc::RemoteArchiveRecoverableResponse {
    mimageviewer_ipc::RemoteArchiveRecoverableResponse::Error(archive_stopped_error())
}
fn archive_cancel_stopped() -> mimageviewer_ipc::RemoteArchiveCancelResponse {
    mimageviewer_ipc::RemoteArchiveCancelResponse::Error(archive_stopped_error())
}
fn archive_input_stopped() -> mimageviewer_ipc::RemoteArchiveInputResponse {
    mimageviewer_ipc::RemoteArchiveInputResponse::Error(archive_stopped_error())
}
fn archive_result_stopped() -> mimageviewer_ipc::RemoteArchiveResultResponse {
    mimageviewer_ipc::RemoteArchiveResultResponse::Error(archive_stopped_error())
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
        | ServerMessage::FavoriteSearch {
            response: FavoriteSearchResponse::Success(_),
            ..
        }
        | ServerMessage::TagBrowse {
            response: TagBrowseResponse::Success(_),
            ..
        }
        | ServerMessage::TagItems {
            response: TagItemsResponse::Success(_),
            ..
        }
        | ServerMessage::Container {
            response: ContainerResponse::Success(_),
            ..
        }
        | ServerMessage::FolderList {
            response: FolderListResponse::Success(_),
            ..
        }
        | ServerMessage::Page {
            response: PageResponse::Success(_),
            ..
        }
        | ServerMessage::PageDemand { .. }
        | ServerMessage::RemoteAiStart {
            response: mimageviewer_ipc::RemoteAiStartResponse::Accepted(_),
            ..
        }
        | ServerMessage::RemoteAiState {
            response: mimageviewer_ipc::RemoteAiStateResponse::Success(_),
            ..
        }
        | ServerMessage::RemoteAiRecoverable {
            response: mimageviewer_ipc::RemoteAiRecoverableResponse::Success(_),
            ..
        }
        | ServerMessage::RemoteAiCancel {
            response: mimageviewer_ipc::RemoteAiCancelResponse::Success(_),
            ..
        }
        | ServerMessage::RemoteAiResult {
            response: mimageviewer_ipc::RemoteAiResultResponse::Success(_),
            ..
        }
        | ServerMessage::RemoteArchiveStart {
            response: mimageviewer_ipc::RemoteArchiveStartResponse::Accepted(_),
            ..
        }
        | ServerMessage::RemoteArchiveState {
            response: mimageviewer_ipc::RemoteArchiveStateResponse::Success(_),
            ..
        }
        | ServerMessage::RemoteArchiveRecoverable {
            response: mimageviewer_ipc::RemoteArchiveRecoverableResponse::Success(_),
            ..
        }
        | ServerMessage::RemoteArchiveCancel {
            response: mimageviewer_ipc::RemoteArchiveCancelResponse::Success(_),
            ..
        }
        | ServerMessage::RemoteArchiveConfirm {
            response: mimageviewer_ipc::RemoteArchiveInputResponse::Success(_),
            ..
        }
        | ServerMessage::RemoteArchivePassword {
            response: mimageviewer_ipc::RemoteArchiveInputResponse::Success(_),
            ..
        }
        | ServerMessage::RemoteArchiveResult {
            response: mimageviewer_ipc::RemoteArchiveResultResponse::Success(_),
            ..
        }
        | ServerMessage::Write {
            response: RemoteWriteResponse::Success(_),
            ..
        }
        | ServerMessage::VideoStreamStart {
            response: VideoStreamResult::Success(_),
            ..
        }
        | ServerMessage::VideoStreamControl {
            response: VideoStreamResult::Success(_),
            ..
        }
        | ServerMessage::VideoStreamSeek {
            response: VideoStreamResult::Success(_),
            ..
        }
        | ServerMessage::VideoStreamThumbnail {
            response: VideoStreamResult::Success(_),
            ..
        }
        | ServerMessage::VideoStreamJumpList {
            response: VideoStreamResult::Success(_),
            ..
        }
        | ServerMessage::VideoStreamJumpThumbnail {
            response: VideoStreamResult::Success(_),
            ..
        }
        | ServerMessage::VideoStreamPlaylist {
            response: VideoStreamResult::Success(_),
            ..
        }
        | ServerMessage::VideoStreamSegment {
            response: VideoStreamResult::Success(_),
            ..
        }
        | ServerMessage::VideoStreamState {
            response: VideoStreamResult::Success(_),
            ..
        }
        | ServerMessage::VideoStreamStop {
            response: VideoStreamResult::Success(_),
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
        | ServerMessage::FavoriteSearch {
            response: FavoriteSearchResponse::Error(_),
            ..
        }
        | ServerMessage::TagBrowse {
            response: TagBrowseResponse::Error(_),
            ..
        }
        | ServerMessage::TagItems {
            response: TagItemsResponse::Error(_),
            ..
        }
        | ServerMessage::Container {
            response: ContainerResponse::Error(_),
            ..
        }
        | ServerMessage::FolderList {
            response: FolderListResponse::Error(_),
            ..
        }
        | ServerMessage::Page {
            response: PageResponse::Error(_),
            ..
        }
        | ServerMessage::RemoteAiStart {
            response: mimageviewer_ipc::RemoteAiStartResponse::Error(_),
            ..
        }
        | ServerMessage::RemoteAiState {
            response: mimageviewer_ipc::RemoteAiStateResponse::Error(_),
            ..
        }
        | ServerMessage::RemoteAiRecoverable {
            response: mimageviewer_ipc::RemoteAiRecoverableResponse::Error(_),
            ..
        }
        | ServerMessage::RemoteAiCancel {
            response: mimageviewer_ipc::RemoteAiCancelResponse::Error(_),
            ..
        }
        | ServerMessage::RemoteAiResult {
            response: mimageviewer_ipc::RemoteAiResultResponse::Error(_),
            ..
        }
        | ServerMessage::RemoteArchiveStart {
            response: mimageviewer_ipc::RemoteArchiveStartResponse::Error(_),
            ..
        }
        | ServerMessage::RemoteArchiveState {
            response: mimageviewer_ipc::RemoteArchiveStateResponse::Error(_),
            ..
        }
        | ServerMessage::RemoteArchiveRecoverable {
            response: mimageviewer_ipc::RemoteArchiveRecoverableResponse::Error(_),
            ..
        }
        | ServerMessage::RemoteArchiveCancel {
            response: mimageviewer_ipc::RemoteArchiveCancelResponse::Error(_),
            ..
        }
        | ServerMessage::RemoteArchiveConfirm {
            response: mimageviewer_ipc::RemoteArchiveInputResponse::Error(_),
            ..
        }
        | ServerMessage::RemoteArchivePassword {
            response: mimageviewer_ipc::RemoteArchiveInputResponse::Error(_),
            ..
        }
        | ServerMessage::RemoteArchiveResult {
            response: mimageviewer_ipc::RemoteArchiveResultResponse::Error(_),
            ..
        }
        | ServerMessage::Write {
            response: RemoteWriteResponse::Error(_),
            ..
        }
        | ServerMessage::VideoStreamStart {
            response: VideoStreamResult::Error(_),
            ..
        }
        | ServerMessage::VideoStreamControl {
            response: VideoStreamResult::Error(_),
            ..
        }
        | ServerMessage::VideoStreamSeek {
            response: VideoStreamResult::Error(_),
            ..
        }
        | ServerMessage::VideoStreamThumbnail {
            response: VideoStreamResult::Error(_),
            ..
        }
        | ServerMessage::VideoStreamJumpList {
            response: VideoStreamResult::Error(_),
            ..
        }
        | ServerMessage::VideoStreamJumpThumbnail {
            response: VideoStreamResult::Error(_),
            ..
        }
        | ServerMessage::VideoStreamPlaylist {
            response: VideoStreamResult::Error(_),
            ..
        }
        | ServerMessage::VideoStreamSegment {
            response: VideoStreamResult::Error(_),
            ..
        }
        | ServerMessage::VideoStreamState {
            response: VideoStreamResult::Error(_),
            ..
        }
        | ServerMessage::VideoStreamStop {
            response: VideoStreamResult::Error(_),
            ..
        } => "error",
    }
}

fn acceptor_loop(
    stop: Arc<AtomicBool>,
    heavy_queue: Arc<HeavyQueueWiring>,
    home_work_tx: mpsc::SyncSender<Work>,
    write_work_tx: mpsc::SyncSender<Work>,
    stream_work_tx: mpsc::SyncSender<Work>,
    home_metrics: Arc<QueueMetrics>,
    write_metrics: Arc<QueueMetrics>,
    stream_metrics: Arc<QueueMetrics>,
    next_connection_id: Arc<AtomicU64>,
    session: SessionHandle,
    page_jobs: Arc<PageJobRegistry>,
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
        let heavy_queue = Arc::clone(&heavy_queue);
        let home_work_tx = home_work_tx.clone();
        let write_work_tx = write_work_tx.clone();
        let stream_work_tx = stream_work_tx.clone();
        let home_metrics = Arc::clone(&home_metrics);
        let write_metrics = Arc::clone(&write_metrics);
        let stream_metrics = Arc::clone(&stream_metrics);
        let session = session.clone();
        let page_jobs = Arc::clone(&page_jobs);
        if let Err(error) = std::thread::Builder::new()
            .name(format!("remote-ipc-connection-{connection_id}"))
            .spawn(move || {
                handle_connection(
                    connection_id,
                    pipe,
                    heavy_queue,
                    home_work_tx,
                    write_work_tx,
                    stream_work_tx,
                    home_metrics,
                    write_metrics,
                    stream_metrics,
                    session,
                    page_jobs,
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
    heavy_queue: Arc<HeavyQueueWiring>,
    home_work_tx: mpsc::SyncSender<Work>,
    write_work_tx: mpsc::SyncSender<Work>,
    stream_work_tx: mpsc::SyncSender<Work>,
    home_metrics: Arc<QueueMetrics>,
    write_metrics: Arc<QueueMetrics>,
    stream_metrics: Arc<QueueMetrics>,
    session: SessionHandle,
    page_jobs: Arc<PageJobRegistry>,
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
                    "remote_ipc: connection_info connection_id={connection_id} accepted={accepted} tailscale_serve={:?}",
                    info.tailscale_serve
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
            ClientMessage::SessionRelease { id, request } => {
                let response = session.logout(&request.owner);
                let _ = reply_tx.send(ServerMessage::Session { id: *id, response });
                continue;
            }
            ClientMessage::SessionActivity { id, owner } => {
                let response = match session.begin_operation(owner, "API 要求を処理中".to_owned())
                {
                    Ok(operation) => match operation.wait_until_active() {
                        Ok(()) => {
                            operation.started();
                            let response = operation.ownership_response();
                            operation.finish(true);
                            response
                        }
                        Err(response) => response,
                    },
                    Err(response) => response,
                };
                let _ = reply_tx.send(ServerMessage::Session { id: *id, response });
                continue;
            }
            ClientMessage::PageDemand { id, owner, request } => {
                let response = match session
                    .begin_operation(owner, "ページ表示の需要を更新中".to_owned())
                {
                    Ok(operation) => match operation.wait_until_active() {
                        Ok(()) => {
                            operation.started();
                            let response = apply_page_demand(&heavy_queue, connection_id, request);
                            operation.finish(true);
                            ServerMessage::PageDemand { id: *id, response }
                        }
                        Err(response) => ServerMessage::Session { id: *id, response },
                    },
                    Err(response) => ServerMessage::Session { id: *id, response },
                };
                let _ = reply_tx.send(response);
                continue;
            }
            ClientMessage::RemoteAiStart {
                id,
                owner,
                request,
                accept_before_unix_ms,
            } => {
                session.note_long_job_client_seen(owner);
                let response = match session.begin_operation(owner, "remote AI job".to_owned()) {
                    Ok(operation) => session
                        .ai_job_registry()
                        .map(|jobs| {
                            jobs.start(
                                owner.client_id.clone(),
                                request.clone(),
                                *accept_before_unix_ms,
                                operation,
                            )
                        })
                        .unwrap_or_else(ai_start_stopped),
                    Err(session_response) => mimageviewer_ipc::RemoteAiStartResponse::Error(
                        ai_session_error(session_response),
                    ),
                };
                let _ = reply_tx.send(ServerMessage::RemoteAiStart { id: *id, response });
                continue;
            }
            ClientMessage::RemoteAiState { id, owner, job_id } => {
                let response = match session
                    .begin_operation(owner, "remote AI job の状態を確認中".to_owned())
                {
                    Ok(operation) => match operation.wait_until_active() {
                        Ok(()) => {
                            operation.started();
                            let response = session
                                .ai_job_registry()
                                .map(|jobs| jobs.state(&owner.client_id, job_id))
                                .unwrap_or_else(ai_state_stopped);
                            operation.finish(true);
                            response
                        }
                        Err(response) => mimageviewer_ipc::RemoteAiStateResponse::Error(
                            ai_session_error(response),
                        ),
                    },
                    Err(response) => {
                        mimageviewer_ipc::RemoteAiStateResponse::Error(ai_session_error(response))
                    }
                };
                let _ = reply_tx.send(ServerMessage::RemoteAiState { id: *id, response });
                continue;
            }
            ClientMessage::RemoteAiRecoverable { id, owner } => {
                let response = match session
                    .begin_operation(owner, "復帰可能な remote AI job を確認中".to_owned())
                {
                    Ok(operation) => match operation.wait_until_active() {
                        Ok(()) => {
                            operation.started();
                            let response = session
                                .ai_job_registry()
                                .map(|jobs| jobs.recoverable(&owner.client_id))
                                .unwrap_or_else(ai_recoverable_stopped);
                            operation.finish(true);
                            response
                        }
                        Err(response) => mimageviewer_ipc::RemoteAiRecoverableResponse::Error(
                            ai_session_error(response),
                        ),
                    },
                    Err(response) => mimageviewer_ipc::RemoteAiRecoverableResponse::Error(
                        ai_session_error(response),
                    ),
                };
                let _ = reply_tx.send(ServerMessage::RemoteAiRecoverable { id: *id, response });
                continue;
            }
            ClientMessage::RemoteAiCancel { id, owner, job_id } => {
                let response = match session
                    .begin_operation(owner, "remote AI job を取り消し中".to_owned())
                {
                    Ok(operation) => match operation.wait_until_active() {
                        Ok(()) => {
                            operation.started();
                            let response = session
                                .ai_job_registry()
                                .map(|jobs| jobs.cancel(&owner.client_id, job_id))
                                .unwrap_or_else(ai_cancel_stopped);
                            operation.finish(true);
                            response
                        }
                        Err(response) => mimageviewer_ipc::RemoteAiCancelResponse::Error(
                            ai_session_error(response),
                        ),
                    },
                    Err(response) => {
                        mimageviewer_ipc::RemoteAiCancelResponse::Error(ai_session_error(response))
                    }
                };
                let _ = reply_tx.send(ServerMessage::RemoteAiCancel { id: *id, response });
                continue;
            }
            ClientMessage::RemoteAiResult {
                id,
                owner,
                job_id,
                page_index,
            } => {
                let response = match session
                    .begin_operation(owner, "remote AI result を取得中".to_owned())
                {
                    Ok(operation) => match operation.wait_until_active() {
                        Ok(()) => {
                            operation.started();
                            let response = session
                                .ai_job_registry()
                                .map(|jobs| {
                                    jobs.result(&owner.client_id, job_id, *page_index as usize)
                                })
                                .unwrap_or_else(ai_result_stopped);
                            operation.finish(true);
                            response
                        }
                        Err(response) => mimageviewer_ipc::RemoteAiResultResponse::Error(
                            ai_session_error(response),
                        ),
                    },
                    Err(response) => {
                        mimageviewer_ipc::RemoteAiResultResponse::Error(ai_session_error(response))
                    }
                };
                let _ = reply_tx.send(ServerMessage::RemoteAiResult { id: *id, response });
                continue;
            }
            ClientMessage::RemoteArchiveStart {
                id,
                owner,
                request,
                accept_before_unix_ms,
            } => {
                session.note_long_job_client_seen(owner);
                let response = match session.begin_operation(owner, "remote archive job".to_owned())
                {
                    Ok(operation) => session
                        .archive_job_registry()
                        .map(|jobs| {
                            jobs.start(
                                owner.client_id.clone(),
                                request.clone(),
                                *accept_before_unix_ms,
                                operation,
                            )
                        })
                        .unwrap_or_else(archive_start_stopped),
                    Err(response) => mimageviewer_ipc::RemoteArchiveStartResponse::Error(
                        archive_session_error(response),
                    ),
                };
                let _ = reply_tx.send(ServerMessage::RemoteArchiveStart { id: *id, response });
                continue;
            }
            ClientMessage::RemoteArchiveState { id, owner, job_id } => {
                let response =
                    match session.begin_operation(owner, "remote archive state".to_owned()) {
                        Ok(operation) => match operation.wait_until_active() {
                            Ok(()) => {
                                operation.started();
                                let response = session
                                    .archive_job_registry()
                                    .map(|jobs| jobs.state(&owner.client_id, job_id))
                                    .unwrap_or_else(archive_state_stopped);
                                operation.finish(true);
                                response
                            }
                            Err(response) => mimageviewer_ipc::RemoteArchiveStateResponse::Error(
                                archive_session_error(response),
                            ),
                        },
                        Err(response) => mimageviewer_ipc::RemoteArchiveStateResponse::Error(
                            archive_session_error(response),
                        ),
                    };
                let _ = reply_tx.send(ServerMessage::RemoteArchiveState { id: *id, response });
                continue;
            }
            ClientMessage::RemoteArchiveRecoverable { id, owner } => {
                let response = match session
                    .begin_operation(owner, "remote archive recovery".to_owned())
                {
                    Ok(operation) => match operation.wait_until_active() {
                        Ok(()) => {
                            operation.started();
                            let response = session
                                .archive_job_registry()
                                .map(|jobs| jobs.recoverable(&owner.client_id))
                                .unwrap_or_else(archive_recoverable_stopped);
                            operation.finish(true);
                            response
                        }
                        Err(response) => mimageviewer_ipc::RemoteArchiveRecoverableResponse::Error(
                            archive_session_error(response),
                        ),
                    },
                    Err(response) => mimageviewer_ipc::RemoteArchiveRecoverableResponse::Error(
                        archive_session_error(response),
                    ),
                };
                let _ =
                    reply_tx.send(ServerMessage::RemoteArchiveRecoverable { id: *id, response });
                continue;
            }
            ClientMessage::RemoteArchiveCancel { id, owner, job_id } => {
                let response =
                    match session.begin_operation(owner, "remote archive cancel".to_owned()) {
                        Ok(operation) => match operation.wait_until_active() {
                            Ok(()) => {
                                operation.started();
                                let response = session
                                    .archive_job_registry()
                                    .map(|jobs| jobs.cancel(&owner.client_id, job_id))
                                    .unwrap_or_else(archive_cancel_stopped);
                                operation.finish(true);
                                response
                            }
                            Err(response) => mimageviewer_ipc::RemoteArchiveCancelResponse::Error(
                                archive_session_error(response),
                            ),
                        },
                        Err(response) => mimageviewer_ipc::RemoteArchiveCancelResponse::Error(
                            archive_session_error(response),
                        ),
                    };
                let _ = reply_tx.send(ServerMessage::RemoteArchiveCancel { id: *id, response });
                continue;
            }
            ClientMessage::RemoteArchiveConfirm { id, owner, request } => {
                let response = match session
                    .begin_operation(owner, "remote archive confirmation".to_owned())
                {
                    Ok(operation) => match operation.wait_until_active() {
                        Ok(()) => {
                            operation.started();
                            let response = session
                                .archive_job_registry()
                                .map(|jobs| jobs.confirm(&owner.client_id, request.clone()))
                                .unwrap_or_else(archive_input_stopped);
                            operation.finish(true);
                            response
                        }
                        Err(response) => mimageviewer_ipc::RemoteArchiveInputResponse::Error(
                            archive_session_error(response),
                        ),
                    },
                    Err(response) => mimageviewer_ipc::RemoteArchiveInputResponse::Error(
                        archive_session_error(response),
                    ),
                };
                let _ = reply_tx.send(ServerMessage::RemoteArchiveConfirm { id: *id, response });
                continue;
            }
            ClientMessage::RemoteArchivePassword { id, owner, request } => {
                let response =
                    match session.begin_operation(owner, "remote archive password".to_owned()) {
                        Ok(operation) => match operation.wait_until_active() {
                            Ok(()) => {
                                operation.started();
                                let response = session
                                    .archive_job_registry()
                                    .map(|jobs| jobs.password(&owner.client_id, request.clone()))
                                    .unwrap_or_else(archive_input_stopped);
                                operation.finish(true);
                                response
                            }
                            Err(response) => mimageviewer_ipc::RemoteArchiveInputResponse::Error(
                                archive_session_error(response),
                            ),
                        },
                        Err(response) => mimageviewer_ipc::RemoteArchiveInputResponse::Error(
                            archive_session_error(response),
                        ),
                    };
                let _ = reply_tx.send(ServerMessage::RemoteArchivePassword { id: *id, response });
                continue;
            }
            ClientMessage::RemoteArchiveResult { id, owner, job_id } => {
                let response =
                    match session.begin_operation(owner, "remote archive result".to_owned()) {
                        Ok(operation) => match operation.wait_until_active() {
                            Ok(()) => {
                                operation.started();
                                let response = session
                                    .archive_job_registry()
                                    .map(|jobs| jobs.result(&owner.client_id, job_id))
                                    .unwrap_or_else(archive_result_stopped);
                                operation.finish(true);
                                response
                            }
                            Err(response) => mimageviewer_ipc::RemoteArchiveResultResponse::Error(
                                archive_session_error(response),
                            ),
                        },
                        Err(response) => mimageviewer_ipc::RemoteArchiveResultResponse::Error(
                            archive_session_error(response),
                        ),
                    };
                let _ = reply_tx.send(ServerMessage::RemoteArchiveResult { id: *id, response });
                continue;
            }
            _ => {}
        }
        let owner =
            message_owner(&message).expect("non-control remote IPC requests carry an owner");
        let session_operation =
            match session.begin_operation(owner, operation_description(&message)) {
                Ok(operation) => operation,
                Err(response) => {
                    let _ = reply_tx.send(ServerMessage::Session {
                        id: request_id,
                        response,
                    });
                    continue;
                }
            };
        let page_job = if let ClientMessage::Page { request, .. } = &message {
            let job_id = PageJobId::from(request.job_id.clone());
            let priority = match request.priority {
                PagePriority::Foreground => PageJobPriority::Foreground,
                PagePriority::Prefetch => PageJobPriority::Prefetch,
            };
            let display_request_id = request
                .display_request_id
                .clone()
                .map(DisplayRequestId::from);
            let cancel = match page_jobs.register(
                connection_id,
                job_id.clone(),
                display_request_id,
                priority,
            ) {
                Ok(cancel) => cancel,
                Err(RegisterPageJobError::DuplicateJob) => {
                    let _ = reply_tx.send(ServerMessage::Page {
                        id: request_id,
                        response: PageResponse::Error(MediaError::new(
                            MediaErrorCode::BadRequest,
                            "page job identity is already registered",
                        )),
                    });
                    drop(session_operation);
                    continue;
                }
            };
            // A drain can win between begin_operation and registry registration. Mirror the
            // already-set session token into the sole render cancellation token in that race.
            if session_operation.cancel_flag().load(Ordering::Acquire) {
                page_jobs.release(
                    connection_id,
                    &job_id,
                    PageJobCancelCause::SessionInvalidated,
                );
            }
            let page_job = PageJobWork {
                registry: Arc::clone(&page_jobs),
                connection_id,
                job_id,
                cancel,
            };
            if page_job.cancel.load(Ordering::Acquire) {
                let _ = reply_tx.send(cancelled_page_response(request_id));
                drop(page_job);
                drop(session_operation);
                continue;
            }
            Some(page_job)
        } else {
            None
        };
        if work_lane(&message) == WorkLane::Heavy {
            let stopped = enqueue_heavy_connection_work(
                &heavy_queue,
                connection_id,
                request_id,
                kind,
                Work::Request {
                    message,
                    reply: reply_tx.clone(),
                    enqueued_at: Instant::now(),
                    session_operation,
                    page_job,
                },
            );
            if stopped {
                break;
            }
            continue;
        }
        // Home は専用 worker へ分離する。重い queue が満杯でも connection reader を
        // 塞がず、後続 Home を読めるよう Busy を明示応答する。
        let (work_tx, metrics) = match work_lane(&message) {
            WorkLane::Home => (&home_work_tx, &home_metrics),
            WorkLane::Write => (&write_work_tx, &write_metrics),
            WorkLane::Stream => (&stream_work_tx, &stream_metrics),
            WorkLane::Heavy => unreachable!(),
        };
        match enqueue_work(
            work_tx,
            metrics,
            message,
            reply_tx.clone(),
            session_operation,
            page_job,
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
    respond_cancelled_works(
        heavy_queue.close_connection(connection_id),
        "connection_closed",
    );
    session.remote_web_disconnected(connection_id);
    drop(reply_tx);
    let _ = writer.join();
}

fn enqueue_heavy_connection_work(
    wiring: &HeavyQueueWiring,
    connection_id: u64,
    request_id: u64,
    request_kind_name: &str,
    work: Work,
) -> bool {
    match wiring.enqueue(connection_id, work) {
        Ok(lane) => {
            let state = heavy_queue_log_state(wiring.snapshot(), Some(lane));
            crate::logger::log(format!(
                "remote_ipc: request_enqueued connection_id={connection_id} request_id={request_id} kind={request_kind_name} queue=heavy queued={} active={}{}",
                state.queued, state.active, state.lane_fields
            ));
            false
        }
        Err(error) => reject_heavy_connection_work(
            wiring,
            connection_id,
            request_id,
            request_kind_name,
            error,
        ),
    }
}

fn reject_heavy_connection_work(
    wiring: &HeavyQueueWiring,
    connection_id: u64,
    request_id: u64,
    request_kind_name: &str,
    error: HeavyEnqueueError,
) -> bool {
    let state = heavy_queue_log_state(wiring.snapshot(), Some(error.lane));
    let event = match error.kind {
        HeavyEnqueueErrorKind::LaneFull => "queue_full",
        HeavyEnqueueErrorKind::Shutdown => "queue_stopped",
        _ => "queue_rejected",
    };
    crate::logger::log(format!(
        "remote_ipc: {event} connection_id={connection_id} request_id={request_id} kind={request_kind_name} queue=heavy reason={} queued={} active={}{}",
        error.kind.log_reason(),
        state.queued,
        state.active,
        state.lane_fields
    ));
    let stopped = error.kind == HeavyEnqueueErrorKind::Shutdown;
    respond_rejected_heavy_work(error);
    stopped
}

fn respond_rejected_heavy_work(error: HeavyEnqueueError) {
    let Work::Request { message, reply, .. } = error.work else {
        unreachable!();
    };
    let response = match error.kind {
        HeavyEnqueueErrorKind::Cancelled => cancelled_work_response(&message),
        HeavyEnqueueErrorKind::PrefetchUnavailableWithSingleWorker
        | HeavyEnqueueErrorKind::LaneFull => queue_busy_response(&message),
        HeavyEnqueueErrorKind::DuplicateKey => duplicate_request_response(&message),
        HeavyEnqueueErrorKind::Shutdown => service_stopped_response(&message),
    };
    let _ = reply.send(response);
}

fn cancelled_work_response(message: &ClientMessage) -> ServerMessage {
    match message {
        ClientMessage::Page { id, .. } => cancelled_page_response(*id),
        _ => service_stopped_response(message),
    }
}

fn respond_cancelled_works(works: Vec<Work>, reason: &'static str) {
    respond_removed_works(works, reason, false);
}

fn respond_stopped_works(works: Vec<Work>, reason: &'static str) {
    respond_removed_works(works, reason, true);
}

fn respond_removed_works(works: Vec<Work>, reason: &'static str, stopped: bool) {
    for work in works {
        let Work::Request { message, reply, .. } = work else {
            unreachable!();
        };
        let request_id = message.id();
        let kind = request_kind(&message);
        let response = if stopped {
            service_stopped_response(&message)
        } else {
            cancelled_work_response(&message)
        };
        let outcome = response_outcome(&response);
        let reply_ok = reply.send(response).is_ok();
        crate::logger::log(format!(
            "remote_ipc: queue_pruned request_id={request_id} kind={kind} queue=heavy reason={reason} outcome={outcome} reply_ok={reply_ok}"
        ));
    }
}

fn apply_page_demand(
    wiring: &HeavyQueueWiring,
    connection_id: u64,
    request: &mimageviewer_ipc::PageDemandRequest,
) -> mimageviewer_ipc::PageDemandResponse {
    let promote = request
        .promote
        .iter()
        .map(|promotion| {
            let job_id = PageJobId::from(promotion.job.clone());
            // A promote may overtake the GET and is intentionally best-effort. Losing it only
            // leaves that request at its initial prefetch priority; release uses tombstones
            // because losing cancellation would violate ownership.
            let status = match wiring.promote_page(
                connection_id,
                &job_id,
                DisplayRequestId::from(promotion.display.clone()),
            ) {
                PromotePageJobResult::Promoted => {
                    mimageviewer_ipc::PageDemandPromoteStatus::Promoted
                }
                PromotePageJobResult::AlreadyForeground => {
                    mimageviewer_ipc::PageDemandPromoteStatus::AlreadyForeground
                }
                PromotePageJobResult::AlreadyReleased { cause } => {
                    mimageviewer_ipc::PageDemandPromoteStatus::AlreadyReleased {
                        cause: page_cancel_cause_to_ipc(cause),
                    }
                }
                PromotePageJobResult::UnknownJob => {
                    mimageviewer_ipc::PageDemandPromoteStatus::UnknownJob
                }
            };
            mimageviewer_ipc::PageDemandPromoteResult {
                job: promotion.job.clone(),
                status,
            }
        })
        .collect();
    let release = request
        .release
        .iter()
        .map(|release| {
            let job_id = PageJobId::from(release.job.clone());
            let (result, removed) = wiring.release_page(
                connection_id,
                &job_id,
                page_cancel_cause_from_ipc(release.cause),
            );
            respond_cancelled_works(removed, "release");
            let status = match result {
                ReleasePageJobResult::Released => {
                    mimageviewer_ipc::PageDemandReleaseStatus::Released
                }
                ReleasePageJobResult::AlreadyReleased { cause } => {
                    mimageviewer_ipc::PageDemandReleaseStatus::AlreadyReleased {
                        cause: page_cancel_cause_to_ipc(cause),
                    }
                }
                ReleasePageJobResult::Tombstoned => {
                    mimageviewer_ipc::PageDemandReleaseStatus::Tombstoned
                }
            };
            mimageviewer_ipc::PageDemandReleaseResult {
                job: release.job.clone(),
                status,
            }
        })
        .collect();
    let snapshot = wiring.registry.snapshot();
    crate::logger::log(format!(
        "remote_ipc: page_demand connection_id={connection_id} promote={} release={} page_jobs_active={} page_jobs_released={} page_jobs_prefetch_active={} page_jobs_foreground_active={}",
        request.promote.len(),
        request.release.len(),
        snapshot.active(),
        snapshot.released(),
        snapshot.prefetch_active,
        snapshot.foreground_active,
    ));
    mimageviewer_ipc::PageDemandResponse { promote, release }
}

fn page_cancel_cause_from_ipc(cause: mimageviewer_ipc::PageCancelCause) -> PageJobCancelCause {
    match cause {
        mimageviewer_ipc::PageCancelCause::NoDemand => PageJobCancelCause::NoDemand,
        mimageviewer_ipc::PageCancelCause::SessionInvalidated => {
            PageJobCancelCause::SessionInvalidated
        }
        mimageviewer_ipc::PageCancelCause::ContextReset => PageJobCancelCause::ContextReset,
        mimageviewer_ipc::PageCancelCause::ConnectionClosed => PageJobCancelCause::ConnectionClosed,
        mimageviewer_ipc::PageCancelCause::ServiceStopping => PageJobCancelCause::ServiceStopping,
    }
}

fn page_cancel_cause_to_ipc(cause: PageJobCancelCause) -> mimageviewer_ipc::PageCancelCause {
    match cause {
        PageJobCancelCause::NoDemand => mimageviewer_ipc::PageCancelCause::NoDemand,
        PageJobCancelCause::SessionInvalidated => {
            mimageviewer_ipc::PageCancelCause::SessionInvalidated
        }
        PageJobCancelCause::ContextReset => mimageviewer_ipc::PageCancelCause::ContextReset,
        PageJobCancelCause::ConnectionClosed => mimageviewer_ipc::PageCancelCause::ConnectionClosed,
        PageJobCancelCause::ServiceStopping => mimageviewer_ipc::PageCancelCause::ServiceStopping,
    }
}

fn enqueue_work(
    work_tx: &mpsc::SyncSender<Work>,
    metrics: &QueueMetrics,
    message: ClientMessage,
    reply: mpsc::Sender<ServerMessage>,
    session_operation: SessionOperation,
    page_job: Option<PageJobWork>,
) -> Result<(), mpsc::TrySendError<Work>> {
    metrics.reserve();
    let result = work_tx.try_send(Work::Request {
        message,
        reply,
        enqueued_at: Instant::now(),
        session_operation,
        page_job,
    });
    if result.is_err() {
        metrics.rollback();
    }
    result
}

fn service_stopped_response(message: &ClientMessage) -> ServerMessage {
    match message {
        ClientMessage::RemoteWebConnectionInfo { id, .. } => {
            ServerMessage::RemoteWebConnectionInfo {
                id: *id,
                accepted: false,
                message: "mIV 本体のリモート接続機能が停止しています".to_owned(),
            }
        }
        ClientMessage::SessionAcquire { id, .. }
        | ClientMessage::SessionPing { id, .. }
        | ClientMessage::SessionRelease { id, .. }
        | ClientMessage::SessionActivity { id, .. }
        | ClientMessage::PageDemand { id, .. } => ServerMessage::Session {
            id: *id,
            response: session_response(
                SessionStatus::NotAcquired,
                "mIV 本体のリモート接続機能が停止しています",
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
        ClientMessage::FavoriteSearch { id, .. } => ServerMessage::FavoriteSearch {
            id: *id,
            response: FavoriteSearchResponse::Error(CollectionError::new(
                CollectionErrorCode::Internal,
                "mIV 本体のお気に入り検索ワーカーが停止しています",
            )),
        },
        ClientMessage::TagBrowse { id, .. } => ServerMessage::TagBrowse {
            id: *id,
            response: TagBrowseResponse::Error(CollectionError::new(
                CollectionErrorCode::Internal,
                "mIV 本体のタグ一覧ワーカーが停止しています",
            )),
        },
        ClientMessage::TagItems { id, .. } => ServerMessage::TagItems {
            id: *id,
            response: TagItemsResponse::Error(CollectionError::new(
                CollectionErrorCode::Internal,
                "mIV 本体のタグ検索ワーカーが停止しています",
            )),
        },
        ClientMessage::Container { id, .. } => ServerMessage::Container {
            id: *id,
            response: ContainerResponse::Error(MediaError::new(
                MediaErrorCode::Internal,
                "mIV 本体のコンテナワーカーが停止しています",
            )),
        },
        ClientMessage::FolderList { id, .. } => ServerMessage::FolderList {
            id: *id,
            response: FolderListResponse::Error(MediaError::new(
                MediaErrorCode::Internal,
                "mIV 本体のフォルダ一覧ワーカーが停止しています",
            )),
        },
        ClientMessage::Page { id, .. } => ServerMessage::Page {
            id: *id,
            response: PageResponse::Error(MediaError::new(
                MediaErrorCode::Cancelled,
                "mIV 本体のページワーカー停止により処理を取り消しました",
            )),
        },
        ClientMessage::Write { id, .. } => ServerMessage::Write {
            id: *id,
            response: RemoteWriteResponse::Error(RemoteWriteError::new(
                RemoteWriteErrorCode::Internal,
                "mIV 本体の書き込みワーカーが停止しています",
            )),
        },
        ClientMessage::RemoteAiStart { id, .. } => ServerMessage::RemoteAiStart {
            id: *id,
            response: ai_start_stopped(),
        },
        ClientMessage::RemoteAiState { id, .. } => ServerMessage::RemoteAiState {
            id: *id,
            response: ai_state_stopped(),
        },
        ClientMessage::RemoteAiRecoverable { id, .. } => ServerMessage::RemoteAiRecoverable {
            id: *id,
            response: ai_recoverable_stopped(),
        },
        ClientMessage::RemoteAiCancel { id, .. } => ServerMessage::RemoteAiCancel {
            id: *id,
            response: ai_cancel_stopped(),
        },
        ClientMessage::RemoteAiResult { id, .. } => ServerMessage::RemoteAiResult {
            id: *id,
            response: ai_result_stopped(),
        },
        ClientMessage::RemoteArchiveStart { id, .. } => ServerMessage::RemoteArchiveStart {
            id: *id,
            response: archive_start_stopped(),
        },
        ClientMessage::RemoteArchiveState { id, .. } => ServerMessage::RemoteArchiveState {
            id: *id,
            response: archive_state_stopped(),
        },
        ClientMessage::RemoteArchiveRecoverable { id, .. } => {
            ServerMessage::RemoteArchiveRecoverable {
                id: *id,
                response: archive_recoverable_stopped(),
            }
        }
        ClientMessage::RemoteArchiveCancel { id, .. } => ServerMessage::RemoteArchiveCancel {
            id: *id,
            response: archive_cancel_stopped(),
        },
        ClientMessage::RemoteArchiveConfirm { id, .. } => ServerMessage::RemoteArchiveConfirm {
            id: *id,
            response: archive_input_stopped(),
        },
        ClientMessage::RemoteArchivePassword { id, .. } => ServerMessage::RemoteArchivePassword {
            id: *id,
            response: archive_input_stopped(),
        },
        ClientMessage::RemoteArchiveResult { id, .. } => ServerMessage::RemoteArchiveResult {
            id: *id,
            response: archive_result_stopped(),
        },
        ClientMessage::VideoStreamStart { id, .. } => ServerMessage::VideoStreamStart {
            id: *id,
            response: stopped_video_response(),
        },
        ClientMessage::VideoStreamControl { id, .. } => ServerMessage::VideoStreamControl {
            id: *id,
            response: stopped_video_response(),
        },
        ClientMessage::VideoStreamSeek { id, .. } => ServerMessage::VideoStreamSeek {
            id: *id,
            response: stopped_video_response(),
        },
        ClientMessage::VideoStreamThumbnail { id, .. } => ServerMessage::VideoStreamThumbnail {
            id: *id,
            response: stopped_video_response(),
        },
        ClientMessage::VideoStreamJumpList { id, .. } => ServerMessage::VideoStreamJumpList {
            id: *id,
            response: stopped_video_response(),
        },
        ClientMessage::VideoStreamJumpThumbnail { id, .. } => {
            ServerMessage::VideoStreamJumpThumbnail {
                id: *id,
                response: stopped_video_response(),
            }
        }
        ClientMessage::VideoStreamPlaylist { id, .. } => ServerMessage::VideoStreamPlaylist {
            id: *id,
            response: stopped_video_response(),
        },
        ClientMessage::VideoStreamSegment { id, .. } => ServerMessage::VideoStreamSegment {
            id: *id,
            response: stopped_video_response(),
        },
        ClientMessage::VideoStreamState { id, .. } => ServerMessage::VideoStreamState {
            id: *id,
            response: stopped_video_response(),
        },
        ClientMessage::VideoStreamStop { id, .. } => ServerMessage::VideoStreamStop {
            id: *id,
            response: stopped_video_response(),
        },
    }
}

fn duplicate_media_error() -> MediaError {
    MediaError::new(
        MediaErrorCode::BadRequest,
        "同じ接続内で request ID が重複しています",
    )
}

fn duplicate_collection_error() -> CollectionError {
    CollectionError::new(
        CollectionErrorCode::BadRequest,
        "同じ接続内で request ID が重複しています",
    )
}

fn duplicate_request_response(message: &ClientMessage) -> ServerMessage {
    match message {
        ClientMessage::Thumbnail { id, .. } => ServerMessage::Thumbnail {
            id: *id,
            response: ThumbnailResponse::Error(ThumbnailError::new(
                ThumbnailErrorCode::BadRequest,
                "同じ接続内で request ID が重複しています",
            )),
        },
        ClientMessage::Collection { id, .. } => ServerMessage::Collection {
            id: *id,
            response: CollectionResponse::Error(duplicate_collection_error()),
        },
        ClientMessage::FavoriteSearch { id, .. } => ServerMessage::FavoriteSearch {
            id: *id,
            response: FavoriteSearchResponse::Error(duplicate_collection_error()),
        },
        _ => duplicate_request_response_tail(message),
    }
}

fn duplicate_request_response_tail(message: &ClientMessage) -> ServerMessage {
    match message {
        ClientMessage::TagBrowse { id, .. } => ServerMessage::TagBrowse {
            id: *id,
            response: TagBrowseResponse::Error(duplicate_collection_error()),
        },
        ClientMessage::TagItems { id, .. } => ServerMessage::TagItems {
            id: *id,
            response: TagItemsResponse::Error(duplicate_collection_error()),
        },
        ClientMessage::Container { id, .. } => ServerMessage::Container {
            id: *id,
            response: ContainerResponse::Error(duplicate_media_error()),
        },
        ClientMessage::Page { id, .. } => ServerMessage::Page {
            id: *id,
            response: PageResponse::Error(duplicate_media_error()),
        },
        ClientMessage::VideoStreamJumpList { id, .. } => ServerMessage::VideoStreamJumpList {
            id: *id,
            response: VideoStreamResult::Error(duplicate_video_error()),
        },
        ClientMessage::VideoStreamJumpThumbnail { id, .. } => {
            ServerMessage::VideoStreamJumpThumbnail {
                id: *id,
                response: VideoStreamResult::Error(duplicate_video_error()),
            }
        }
        _ => service_stopped_response(message),
    }
}

fn duplicate_video_error() -> VideoStreamError {
    VideoStreamError::new(
        VideoStreamErrorCode::BadRequest,
        "同じ接続内で request ID が重複しています",
    )
}

fn queue_busy_response(message: &ClientMessage) -> ServerMessage {
    match message {
        ClientMessage::RemoteWebConnectionInfo { id, .. } => {
            ServerMessage::RemoteWebConnectionInfo {
                id: *id,
                accepted: false,
                message: "mIV 本体のリモート接続機能が混み合っています".to_owned(),
            }
        }
        ClientMessage::SessionAcquire { id, .. }
        | ClientMessage::SessionPing { id, .. }
        | ClientMessage::SessionRelease { id, .. }
        | ClientMessage::SessionActivity { id, .. }
        | ClientMessage::PageDemand { id, .. } => ServerMessage::Session {
            id: *id,
            response: session_response(
                SessionStatus::NotAcquired,
                "mIV 本体のリモート接続機能が混み合っています",
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
        ClientMessage::FavoriteSearch { id, .. } => ServerMessage::FavoriteSearch {
            id: *id,
            response: FavoriteSearchResponse::Error(CollectionError::new(
                CollectionErrorCode::Busy,
                "mIV 本体のお気に入り検索 queue が混み合っています",
            )),
        },
        ClientMessage::TagBrowse { id, .. } => ServerMessage::TagBrowse {
            id: *id,
            response: TagBrowseResponse::Error(CollectionError::new(
                CollectionErrorCode::Busy,
                "mIV 本体のタグ一覧 queue が混み合っています",
            )),
        },
        ClientMessage::TagItems { id, .. } => ServerMessage::TagItems {
            id: *id,
            response: TagItemsResponse::Error(CollectionError::new(
                CollectionErrorCode::Busy,
                "mIV 本体のタグ検索 queue が混み合っています",
            )),
        },
        ClientMessage::Container { id, .. } => ServerMessage::Container {
            id: *id,
            response: ContainerResponse::Error(MediaError::new(
                MediaErrorCode::Busy,
                "mIV 本体のリモートコンテナ queue が混み合っています",
            )),
        },
        ClientMessage::FolderList { id, .. } => ServerMessage::FolderList {
            id: *id,
            response: FolderListResponse::Error(MediaError::new(
                MediaErrorCode::Busy,
                "mIV 本体のリモートフォルダ一覧 queue が混み合っています",
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
        ClientMessage::RemoteAiStart { id, .. } => ServerMessage::RemoteAiStart {
            id: *id,
            response: mimageviewer_ipc::RemoteAiStartResponse::Error(ai_busy_error()),
        },
        ClientMessage::RemoteAiState { id, .. } => ServerMessage::RemoteAiState {
            id: *id,
            response: mimageviewer_ipc::RemoteAiStateResponse::Error(ai_busy_error()),
        },
        ClientMessage::RemoteAiRecoverable { id, .. } => ServerMessage::RemoteAiRecoverable {
            id: *id,
            response: mimageviewer_ipc::RemoteAiRecoverableResponse::Error(ai_busy_error()),
        },
        ClientMessage::RemoteAiCancel { id, .. } => ServerMessage::RemoteAiCancel {
            id: *id,
            response: mimageviewer_ipc::RemoteAiCancelResponse::Error(ai_busy_error()),
        },
        ClientMessage::RemoteAiResult { id, .. } => ServerMessage::RemoteAiResult {
            id: *id,
            response: mimageviewer_ipc::RemoteAiResultResponse::Error(ai_busy_error()),
        },
        ClientMessage::RemoteArchiveStart { id, .. } => ServerMessage::RemoteArchiveStart {
            id: *id,
            response: mimageviewer_ipc::RemoteArchiveStartResponse::Error(archive_busy_error()),
        },
        ClientMessage::RemoteArchiveState { id, .. } => ServerMessage::RemoteArchiveState {
            id: *id,
            response: mimageviewer_ipc::RemoteArchiveStateResponse::Error(archive_busy_error()),
        },
        ClientMessage::RemoteArchiveRecoverable { id, .. } => {
            ServerMessage::RemoteArchiveRecoverable {
                id: *id,
                response: mimageviewer_ipc::RemoteArchiveRecoverableResponse::Error(
                    archive_busy_error(),
                ),
            }
        }
        ClientMessage::RemoteArchiveCancel { id, .. } => ServerMessage::RemoteArchiveCancel {
            id: *id,
            response: mimageviewer_ipc::RemoteArchiveCancelResponse::Error(archive_busy_error()),
        },
        ClientMessage::RemoteArchiveConfirm { id, .. } => ServerMessage::RemoteArchiveConfirm {
            id: *id,
            response: mimageviewer_ipc::RemoteArchiveInputResponse::Error(archive_busy_error()),
        },
        ClientMessage::RemoteArchivePassword { id, .. } => ServerMessage::RemoteArchivePassword {
            id: *id,
            response: mimageviewer_ipc::RemoteArchiveInputResponse::Error(archive_busy_error()),
        },
        ClientMessage::RemoteArchiveResult { id, .. } => ServerMessage::RemoteArchiveResult {
            id: *id,
            response: mimageviewer_ipc::RemoteArchiveResultResponse::Error(archive_busy_error()),
        },
        ClientMessage::VideoStreamStart { id, .. } => ServerMessage::VideoStreamStart {
            id: *id,
            response: busy_video_response(),
        },
        ClientMessage::VideoStreamControl { id, .. } => ServerMessage::VideoStreamControl {
            id: *id,
            response: busy_video_response(),
        },
        ClientMessage::VideoStreamSeek { id, .. } => ServerMessage::VideoStreamSeek {
            id: *id,
            response: busy_video_response(),
        },
        ClientMessage::VideoStreamThumbnail { id, .. } => ServerMessage::VideoStreamThumbnail {
            id: *id,
            response: busy_video_response(),
        },
        ClientMessage::VideoStreamJumpList { id, .. } => ServerMessage::VideoStreamJumpList {
            id: *id,
            response: busy_video_response(),
        },
        ClientMessage::VideoStreamJumpThumbnail { id, .. } => {
            ServerMessage::VideoStreamJumpThumbnail {
                id: *id,
                response: busy_video_response(),
            }
        }
        ClientMessage::VideoStreamPlaylist { id, .. } => ServerMessage::VideoStreamPlaylist {
            id: *id,
            response: busy_video_response(),
        },
        ClientMessage::VideoStreamSegment { id, .. } => ServerMessage::VideoStreamSegment {
            id: *id,
            response: busy_video_response(),
        },
        ClientMessage::VideoStreamState { id, .. } => ServerMessage::VideoStreamState {
            id: *id,
            response: busy_video_response(),
        },
        ClientMessage::VideoStreamStop { id, .. } => ServerMessage::VideoStreamStop {
            id: *id,
            response: busy_video_response(),
        },
    }
}

fn stopped_video_response<T>() -> VideoStreamResult<T> {
    VideoStreamResult::Error(VideoStreamError::new(
        VideoStreamErrorCode::Internal,
        "mIV 本体の動画ストリーミングワーカーが停止しています",
    ))
}

fn busy_video_response<T>() -> VideoStreamResult<T> {
    VideoStreamResult::Error(VideoStreamError::new(
        VideoStreamErrorCode::Busy,
        "mIV 本体の動画ストリーミング queue が混み合っています",
    ))
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

/// `PIPE_REJECT_REMOTE_CLIENTS` はネットワーク経由だけを拒否する。既定 DACL はローカルの
/// Everyone に read を許すため、current user だけに full access を与える DACL を明示する。
struct CurrentUserPipeSecurity {
    _acl: Vec<u32>,
    descriptor: SECURITY_DESCRIPTOR,
}

impl CurrentUserPipeSecurity {
    fn attributes(&mut self) -> SECURITY_ATTRIBUTES {
        SECURITY_ATTRIBUTES {
            nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: (&mut self.descriptor as *mut SECURITY_DESCRIPTOR).cast(),
            bInheritHandle: false.into(),
        }
    }
}

fn current_user_pipe_security() -> Result<CurrentUserPipeSecurity, std::io::Error> {
    let mut token = HANDLE::default();
    unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) }
        .map_err(|_| std::io::Error::last_os_error())?;

    let result = (|| {
        let mut token_user_bytes = 0_u32;
        let _ = unsafe { GetTokenInformation(token, TokenUser, None, 0, &mut token_user_bytes) };
        if token_user_bytes < std::mem::size_of::<TOKEN_USER>() as u32 {
            return Err(std::io::Error::last_os_error());
        }
        let word_size = std::mem::size_of::<usize>();
        let mut token_user = vec![0_usize; (token_user_bytes as usize).div_ceil(word_size)];
        unsafe {
            GetTokenInformation(
                token,
                TokenUser,
                Some(token_user.as_mut_ptr().cast()),
                token_user_bytes,
                &mut token_user_bytes,
            )
        }
        .map_err(|_| std::io::Error::last_os_error())?;
        let token_user =
            unsafe { std::ptr::read_unaligned(token_user.as_ptr().cast::<TOKEN_USER>()) };
        let sid = token_user.User.Sid;
        if !unsafe { IsValidSid(sid) }.as_bool() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "current user SID is invalid",
            ));
        }

        let acl_bytes = std::mem::size_of::<ACL>() + std::mem::size_of::<ACCESS_ALLOWED_ACE>()
            - std::mem::size_of::<u32>()
            + unsafe { GetLengthSid(sid) } as usize;
        let mut acl = vec![0_u32; acl_bytes.div_ceil(std::mem::size_of::<u32>())];
        let acl_ptr = acl.as_mut_ptr().cast::<ACL>();
        unsafe { InitializeAcl(acl_ptr, (acl.len() * 4) as u32, ACL_REVISION) }
            .map_err(|_| std::io::Error::last_os_error())?;
        unsafe { AddAccessAllowedAce(acl_ptr, ACL_REVISION, GENERIC_ALL.0, sid) }
            .map_err(|_| std::io::Error::last_os_error())?;

        let mut descriptor = SECURITY_DESCRIPTOR::default();
        let descriptor_ptr =
            PSECURITY_DESCRIPTOR((&mut descriptor as *mut SECURITY_DESCRIPTOR).cast());
        unsafe { InitializeSecurityDescriptor(descriptor_ptr, SECURITY_DESCRIPTOR_REVISION) }
            .map_err(|_| std::io::Error::last_os_error())?;
        unsafe { SetSecurityDescriptorDacl(descriptor_ptr, true, Some(acl_ptr), false) }
            .map_err(|_| std::io::Error::last_os_error())?;
        Ok(CurrentUserPipeSecurity {
            _acl: acl,
            descriptor,
        })
    })();
    let _ = unsafe { CloseHandle(token) };
    result
}

fn create_server_pipe(first: bool) -> Result<PipeStream, std::io::Error> {
    let name = wide_nul(PIPE_NAME);
    let access = server_pipe_access(first);
    let mode = NAMED_PIPE_MODE(
        PIPE_TYPE_BYTE.0 | PIPE_READMODE_BYTE.0 | PIPE_WAIT.0 | PIPE_REJECT_REMOTE_CLIENTS.0,
    );
    let mut security = current_user_pipe_security()?;
    let security_attributes = security.attributes();
    let handle = unsafe {
        CreateNamedPipeW(
            PCWSTR(name.as_ptr()),
            access,
            mode,
            PIPE_MAX_INSTANCES,
            PIPE_BUFFER_BYTES,
            PIPE_BUFFER_BYTES,
            0,
            Some(&security_attributes),
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

    fn test_owner() -> RemoteSessionIdentity {
        RemoteSessionIdentity {
            client_id: "test-client".to_owned(),
            session_id: "0123456789abcdef0123456789abcdef".to_owned(),
        }
    }

    fn active_test_operation() -> SessionOperation {
        let session = SessionHandle::new();
        session.acquire(mimageviewer_ipc::SessionAcquireRequest {
            client_id: "queue-test".to_owned(),
            peer: mimageviewer_ipc::SessionPeerInfo {
                connection_kind: mimageviewer_ipc::SessionConnectionKind::Unknown,
                device_name: None,
            },
        });
        assert!(session.finish_acquire(session.snapshot().generation));
        let owner = session.owner_for_test("queue-test");
        session
            .begin_operation(&owner, "queue test".to_owned())
            .unwrap()
    }

    fn page_work(
        registry: &Arc<PageJobRegistry>,
        connection_id: u64,
        request_id: u64,
        job_id: &str,
        priority: PageJobPriority,
        reply: mpsc::Sender<ServerMessage>,
    ) -> Work {
        let cancel = registry
            .register(connection_id, job_id.into(), None, priority)
            .unwrap();
        Work::Request {
            message: ClientMessage::Page {
                id: request_id,
                owner: test_owner(),
                request: mimageviewer_ipc::PageRequest {
                    job_id: job_id.to_owned(),
                    display_request_id: None,
                    address: mimageviewer_ipc::RemoteAddress::file("C:/Pictures/page.jpg"),
                    target_px: 2048,
                    priority: match priority {
                        PageJobPriority::Foreground => PagePriority::Foreground,
                        PageJobPriority::Prefetch => PagePriority::Prefetch,
                    },
                    render_context: None,
                    adjustment_preview: None,
                },
            },
            reply,
            enqueued_at: Instant::now(),
            session_operation: active_test_operation(),
            page_job: Some(PageJobWork {
                registry: Arc::clone(registry),
                connection_id,
                job_id: job_id.into(),
                cancel,
            }),
        }
    }

    fn thumbnail_work(request_id: u64, reply: mpsc::Sender<ServerMessage>) -> Work {
        Work::Request {
            message: ClientMessage::Thumbnail {
                id: request_id,
                owner: test_owner(),
                request: mimageviewer_ipc::ThumbnailRequest {
                    address: mimageviewer_ipc::RemoteAddress::file("C:/Pictures/page.jpg"),
                    source_address: None,
                    target_px: 256,
                },
            },
            reply,
            enqueued_at: Instant::now(),
            session_operation: active_test_operation(),
            page_job: None,
        }
    }

    fn complete_heavy_item(
        wiring: &HeavyQueueWiring,
        item: HeavyQueueItem<HeavyKey, Work>,
    ) -> (HeavyQueueLane, Work) {
        let lane = item.lane();
        let (key, work, _) = item.into_parts();
        let page_key = match &work {
            Work::Request { page_job, .. } => page_job_key(page_job.as_ref()),
            Work::Stop => None,
        };
        assert!(matches!(
            wiring.complete(&key, page_key.as_ref()),
            CompleteHeavyQueueResult::Completed { .. }
        ));
        (lane, work)
    }

    #[test]
    fn remote_pipe_dacl_has_one_current_user_ace() {
        let security = current_user_pipe_security().unwrap();
        let acl = unsafe { &*security._acl.as_ptr().cast::<ACL>() };
        assert_eq!(acl.AceCount, 1);
    }

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
        let settings = crate::settings::Settings::default();
        let collection_engine = CollectionEngine::new(settings.clone());
        let container_engine = ContainerEngine::new(settings);
        let worker = std::thread::spawn(move || {
            home_worker_loop(
                work_rx,
                &collection_engine,
                &container_engine,
                &worker_metrics,
            )
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
        assert!(session.finish_acquire(session.snapshot().generation));
        let owner = session.owner_for_test("test-client");
        let session_operation = session
            .begin_operation(&owner, "ホームを読み込み中".to_owned())
            .unwrap();
        enqueue_work(
            &work_tx,
            &metrics,
            ClientMessage::Home {
                id: 73,
                owner,
                request: mimageviewer_ipc::HomeRequest,
            },
            reply_tx,
            session_operation,
            None,
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
    fn heavy_wiring_prioritizes_foreground_before_thumbnail_and_prefetch() {
        let registry = Arc::new(PageJobRegistry::new());
        let wiring = HeavyQueueWiring::new(3, Arc::clone(&registry));
        let (prefetch_tx, _prefetch_rx) = mpsc::channel();
        let (thumbnail_tx, _thumbnail_rx) = mpsc::channel();
        let (foreground_tx, _foreground_rx) = mpsc::channel();
        assert!(matches!(
            wiring.enqueue(
                1,
                page_work(
                    &registry,
                    1,
                    10,
                    "prefetch",
                    PageJobPriority::Prefetch,
                    prefetch_tx,
                ),
            ),
            Ok(HeavyQueueLane::Prefetch)
        ));
        assert!(matches!(
            wiring.enqueue(1, thumbnail_work(11, thumbnail_tx)),
            Ok(HeavyQueueLane::Interactive)
        ));
        assert!(matches!(
            wiring.enqueue(
                1,
                page_work(
                    &registry,
                    1,
                    12,
                    "foreground",
                    PageJobPriority::Foreground,
                    foreground_tx,
                ),
            ),
            Ok(HeavyQueueLane::Foreground)
        ));
        let lanes = (0..3)
            .map(|_| complete_heavy_item(&wiring, wiring.pop().unwrap()).0)
            .collect::<Vec<_>>();
        assert_eq!(
            lanes,
            vec![
                HeavyQueueLane::Foreground,
                HeavyQueueLane::Interactive,
                HeavyQueueLane::Prefetch,
            ]
        );
    }

    #[test]
    fn page_demand_promotion_moves_prefetch_ahead_of_waiting_interactive_work() {
        let registry = Arc::new(PageJobRegistry::new());
        let wiring = HeavyQueueWiring::new(2, Arc::clone(&registry));
        let (reply_tx, _reply_rx) = mpsc::channel();
        let (thumbnail_tx, _thumbnail_rx) = mpsc::channel();
        assert!(matches!(
            wiring.enqueue(
                7,
                page_work(
                    &registry,
                    7,
                    70,
                    "page-job",
                    PageJobPriority::Prefetch,
                    reply_tx,
                ),
            ),
            Ok(HeavyQueueLane::Prefetch)
        ));
        assert!(matches!(
            wiring.enqueue(7, thumbnail_work(71, thumbnail_tx)),
            Ok(HeavyQueueLane::Interactive)
        ));

        let promoted = apply_page_demand(
            &wiring,
            7,
            &mimageviewer_ipc::PageDemandRequest {
                promote: vec![mimageviewer_ipc::PageDemandPromotion {
                    job: "page-job".to_owned(),
                    display: "display-1".to_owned(),
                }],
                release: Vec::new(),
            },
        );
        assert_eq!(
            promoted.promote[0].status,
            mimageviewer_ipc::PageDemandPromoteStatus::Promoted
        );
        assert_eq!(
            wiring.snapshot().foreground.queued,
            1,
            "promotion must move the queue mirror before dispatch"
        );
        let promoted_item = wiring.pop().unwrap();
        assert_eq!(promoted_item.lane(), HeavyQueueLane::Foreground);
        let (_, work) = complete_heavy_item(&wiring, promoted_item);
        let Work::Request { page_job, .. } = &work else {
            unreachable!();
        };
        assert_eq!(
            effective_page_priority(page_job.as_ref().unwrap()),
            PagePriority::Foreground
        );
        let interactive_item = wiring.pop().unwrap();
        assert_eq!(interactive_item.lane(), HeavyQueueLane::Interactive);
        let _ = complete_heavy_item(&wiring, interactive_item);
    }

    #[test]
    fn dropped_heavy_completion_guard_releases_the_active_slot() {
        let registry = Arc::new(PageJobRegistry::new());
        let wiring = HeavyQueueWiring::new(1, registry);
        let (first_tx, _first_rx) = mpsc::channel();
        let (second_tx, _second_rx) = mpsc::channel();
        assert!(wiring.enqueue(1, thumbnail_work(72, first_tx)).is_ok());

        let first = wiring.pop().unwrap();
        let (key, work, lane) = first.into_parts();
        let completion = HeavyCompletionGuard::new(&wiring, key, lane, None);
        assert_eq!(wiring.snapshot().active(), 1);
        drop(completion);
        drop(work);
        assert_eq!(wiring.snapshot().active(), 0);

        assert!(wiring.enqueue(1, thumbnail_work(73, second_tx)).is_ok());
        let second = wiring.pop().unwrap();
        assert_eq!(second.lane(), HeavyQueueLane::Interactive);
        let _ = complete_heavy_item(&wiring, second);
    }

    #[test]
    fn page_demand_release_prunes_waiting_work_and_replies_cancelled() {
        let registry = Arc::new(PageJobRegistry::new());
        let wiring = HeavyQueueWiring::new(2, Arc::clone(&registry));
        let (reply_tx, reply_rx) = mpsc::channel();
        assert!(
            wiring
                .enqueue(
                    7,
                    page_work(
                        &registry,
                        7,
                        71,
                        "release-job",
                        PageJobPriority::Prefetch,
                        reply_tx,
                    ),
                )
                .is_ok()
        );
        let released = apply_page_demand(
            &wiring,
            7,
            &mimageviewer_ipc::PageDemandRequest {
                promote: Vec::new(),
                release: vec![mimageviewer_ipc::PageDemandRelease {
                    job: "release-job".to_owned(),
                    cause: mimageviewer_ipc::PageCancelCause::NoDemand,
                }],
            },
        );
        assert_eq!(
            released.release[0].status,
            mimageviewer_ipc::PageDemandReleaseStatus::Released
        );
        assert_eq!(wiring.snapshot().queued(), 0);
        assert!(is_cancelled_page_response(
            &reply_rx.recv_timeout(Duration::from_secs(1)).unwrap()
        ));
    }

    #[test]
    fn connection_prune_is_scoped_and_replies_to_its_waiting_page() {
        let registry = Arc::new(PageJobRegistry::new());
        let wiring = HeavyQueueWiring::new(2, Arc::clone(&registry));
        let (first_tx, first_rx) = mpsc::channel();
        let (second_tx, second_rx) = mpsc::channel();
        assert!(
            wiring
                .enqueue(
                    1,
                    page_work(
                        &registry,
                        1,
                        80,
                        "first",
                        PageJobPriority::Foreground,
                        first_tx,
                    ),
                )
                .is_ok()
        );
        assert!(
            wiring
                .enqueue(
                    2,
                    page_work(
                        &registry,
                        2,
                        81,
                        "second",
                        PageJobPriority::Foreground,
                        second_tx,
                    ),
                )
                .is_ok()
        );
        respond_cancelled_works(wiring.close_connection(1), "connection_test");
        assert!(is_cancelled_page_response(
            &first_rx.recv_timeout(Duration::from_secs(1)).unwrap()
        ));
        assert!(matches!(
            second_rx.try_recv(),
            Err(mpsc::TryRecvError::Empty)
        ));
        assert_eq!(wiring.snapshot().foreground.queued, 1);
        assert!(registry.priority(1, &PageJobId::from("first")).is_none());
        assert_eq!(
            registry.priority(2, &PageJobId::from("second")),
            Some(PageJobPriority::Foreground)
        );
        let _ = complete_heavy_item(&wiring, wiring.pop().unwrap());
    }

    #[test]
    fn single_worker_prefetch_is_rejected_immediately_with_busy() {
        let registry = Arc::new(PageJobRegistry::new());
        let wiring = HeavyQueueWiring::new(1, Arc::clone(&registry));
        let (reply_tx, reply_rx) = mpsc::channel();
        let work = page_work(
            &registry,
            1,
            90,
            "single-worker",
            PageJobPriority::Prefetch,
            reply_tx,
        );
        let started = Instant::now();
        let error = match wiring.enqueue(1, work) {
            Ok(_) => panic!("single-worker prefetch must not enter the queue"),
            Err(error) => error,
        };
        assert_eq!(
            error.kind,
            HeavyEnqueueErrorKind::PrefetchUnavailableWithSingleWorker
        );
        assert!(started.elapsed() < Duration::from_millis(50));
        respond_rejected_heavy_work(error);
        assert!(matches!(
            reply_rx.recv_timeout(Duration::from_secs(1)).unwrap(),
            ServerMessage::Page {
                response: PageResponse::Error(MediaError {
                    code: MediaErrorCode::Busy,
                    ..
                }),
                ..
            }
        ));
        assert_eq!(wiring.snapshot().queued(), 0);
    }

    #[test]
    fn full_prefetch_lane_does_not_block_foreground_admission() {
        let registry = Arc::new(PageJobRegistry::new());
        let wiring = HeavyQueueWiring::new_with_capacities(
            2,
            HeavyQueueCapacities {
                foreground: 1,
                interactive: 1,
                prefetch: 1,
            },
            Arc::clone(&registry),
        );
        let (first_tx, _first_rx) = mpsc::channel();
        let (full_tx, full_rx) = mpsc::channel();
        let (foreground_tx, _foreground_rx) = mpsc::channel();
        assert!(
            wiring
                .enqueue(
                    1,
                    page_work(
                        &registry,
                        1,
                        100,
                        "prefetch-1",
                        PageJobPriority::Prefetch,
                        first_tx,
                    ),
                )
                .is_ok()
        );
        let error = wiring
            .enqueue(
                1,
                page_work(
                    &registry,
                    1,
                    101,
                    "prefetch-2",
                    PageJobPriority::Prefetch,
                    full_tx,
                ),
            )
            .err()
            .unwrap();
        assert_eq!(error.kind, HeavyEnqueueErrorKind::LaneFull);
        respond_rejected_heavy_work(error);
        assert!(matches!(
            full_rx.recv_timeout(Duration::from_secs(1)).unwrap(),
            ServerMessage::Page {
                response: PageResponse::Error(MediaError {
                    code: MediaErrorCode::Busy,
                    ..
                }),
                ..
            }
        ));
        assert!(
            wiring
                .enqueue(
                    1,
                    page_work(
                        &registry,
                        1,
                        102,
                        "foreground",
                        PageJobPriority::Foreground,
                        foreground_tx,
                    ),
                )
                .is_ok()
        );
        assert_eq!(wiring.snapshot().foreground.queued, 1);
        respond_stopped_works(wiring.stop(), "test_cleanup");
    }

    #[test]
    fn duplicate_heavy_key_is_bad_request_not_busy() {
        let registry = Arc::new(PageJobRegistry::new());
        let wiring = HeavyQueueWiring::new(2, registry);
        let (first_tx, _first_rx) = mpsc::channel();
        let (duplicate_tx, duplicate_rx) = mpsc::channel();
        assert!(wiring.enqueue(1, thumbnail_work(110, first_tx)).is_ok());
        let error = wiring
            .enqueue(1, thumbnail_work(110, duplicate_tx))
            .err()
            .unwrap();
        assert_eq!(error.kind, HeavyEnqueueErrorKind::DuplicateKey);
        respond_rejected_heavy_work(error);
        assert!(matches!(
            duplicate_rx.recv_timeout(Duration::from_secs(1)).unwrap(),
            ServerMessage::Thumbnail {
                response: ThumbnailResponse::Error(ThumbnailError {
                    code: ThumbnailErrorCode::BadRequest,
                    ..
                }),
                ..
            }
        ));
        respond_stopped_works(wiring.stop(), "test_cleanup");
    }

    #[test]
    fn heavy_shutdown_replies_to_every_waiting_payload() {
        let registry = Arc::new(PageJobRegistry::new());
        let wiring = HeavyQueueWiring::new(2, Arc::clone(&registry));
        let (page_tx, page_rx) = mpsc::channel();
        let (thumbnail_tx, thumbnail_rx) = mpsc::channel();
        assert!(
            wiring
                .enqueue(
                    1,
                    page_work(
                        &registry,
                        1,
                        120,
                        "shutdown-page",
                        PageJobPriority::Foreground,
                        page_tx,
                    ),
                )
                .is_ok()
        );
        assert!(wiring.enqueue(1, thumbnail_work(121, thumbnail_tx)).is_ok());
        respond_stopped_works(wiring.stop(), "shutdown_test");
        assert!(is_cancelled_page_response(
            &page_rx.recv_timeout(Duration::from_secs(1)).unwrap()
        ));
        assert!(matches!(
            thumbnail_rx.recv_timeout(Duration::from_secs(1)).unwrap(),
            ServerMessage::Thumbnail {
                response: ThumbnailResponse::Error(ThumbnailError {
                    code: ThumbnailErrorCode::Internal,
                    ..
                }),
                ..
            }
        ));
        let snapshot = wiring.snapshot();
        assert!(snapshot.shutdown);
        assert_eq!(snapshot.queued(), 0);
    }

    #[test]
    fn heavy_log_state_keeps_totals_and_adds_every_lane_breakdown() {
        let state = heavy_queue_log_state(HeavyQueueSnapshot::default(), None);
        assert_eq!(state.queued, 0);
        assert_eq!(state.active, 0);
        for field in [
            "lane=none",
            "foreground_queued=0",
            "foreground_active=0",
            "interactive_queued=0",
            "interactive_active=0",
            "prefetch_queued=0",
            "prefetch_active=0",
        ] {
            assert!(state.lane_fields.contains(field), "missing {field}");
        }
    }

    #[test]
    fn stopped_page_work_has_a_typed_cancelled_response() {
        let response = service_stopped_response(&ClientMessage::Page {
            id: 91,
            owner: test_owner(),
            request: mimageviewer_ipc::PageRequest {
                job_id: "page-91".to_owned(),
                display_request_id: Some("display-91".to_owned()),
                address: mimageviewer_ipc::RemoteAddress::file("C:/Pictures/page.jpg"),
                target_px: 2048,
                priority: PagePriority::Foreground,
                render_context: None,
                adjustment_preview: None,
            },
        });
        assert!(is_cancelled_page_response(&response));
    }

    #[test]
    fn full_stream_queue_does_not_consume_thumbnail_or_list_lanes() {
        let segment = ClientMessage::VideoStreamSegment {
            id: 80,
            owner: test_owner(),
            session: 1,
            generation: 2,
            index: mimageviewer_ipc::VideoStreamSegmentIndex::Media { sequence: 3 },
        };
        let thumbnail = ClientMessage::Thumbnail {
            id: 81,
            owner: test_owner(),
            request: mimageviewer_ipc::ThumbnailRequest {
                address: mimageviewer_ipc::RemoteAddress::file("C:/Pictures/page.jpg"),
                source_address: None,
                target_px: 256,
            },
        };
        let home = ClientMessage::Home {
            id: 82,
            owner: test_owner(),
            request: mimageviewer_ipc::HomeRequest,
        };
        let folder_list = ClientMessage::FolderList {
            id: 83,
            owner: test_owner(),
            request: mimageviewer_ipc::FolderListRequest {
                address: mimageviewer_ipc::RemoteAddress::file("C:/Pictures/album"),
            },
        };
        let jump_list = ClientMessage::VideoStreamJumpList {
            id: 84,
            owner: test_owner(),
            session: 1,
        };
        assert_eq!(work_lane(&segment), WorkLane::Stream);
        assert_eq!(work_lane(&thumbnail), WorkLane::Heavy);
        assert_eq!(work_lane(&jump_list), WorkLane::Heavy);
        assert_eq!(work_lane(&home), WorkLane::Home);
        assert_eq!(work_lane(&folder_list), WorkLane::Home);

        let (stream_tx, _stream_rx) = mpsc::sync_channel(1);
        stream_tx.try_send(Work::Stop).unwrap();
        assert!(matches!(
            stream_tx.try_send(Work::Stop),
            Err(mpsc::TrySendError::Full(Work::Stop))
        ));
        let (home_tx, home_rx) = mpsc::sync_channel(1);
        home_tx.try_send(Work::Stop).unwrap();
        assert!(matches!(home_rx.try_recv(), Ok(Work::Stop)));
    }

    #[test]
    fn queue_full_is_an_explicit_retryable_response() {
        let message = ClientMessage::Thumbnail {
            id: 42,
            owner: test_owner(),
            request: mimageviewer_ipc::ThumbnailRequest {
                address: mimageviewer_ipc::RemoteAddress::file("C:/Pictures/page.jpg"),
                source_address: None,
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
    fn remote_workers_follow_the_configured_parallelism_without_a_discount() {
        // No remote-specific halving or ceiling: the number the user chose is the
        // number they get. The session is exclusive, so local display workers are
        // not running while a remote reader holds it.
        for configured in [1, 2, 4, 8, 64] {
            assert_eq!(remote_heavy_worker_count(configured), configured);
        }
        // A pool of zero would accept work nothing ever pops.
        assert_eq!(remote_heavy_worker_count(0), 1);
    }
}
