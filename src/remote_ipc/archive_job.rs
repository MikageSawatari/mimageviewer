use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, Weak, mpsc};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use mimageviewer_ipc::{
    RemoteAddress, RemoteArchiveAccessMode, RemoteArchiveAwaitingInput,
    RemoteArchiveCancelResponse, RemoteArchiveConfirmRequest, RemoteArchiveImageSummary,
    RemoteArchiveInputResponse, RemoteArchiveJobError, RemoteArchiveJobErrorCode,
    RemoteArchiveJobSnapshot, RemoteArchiveJobState, RemoteArchiveOpenResult,
    RemoteArchivePasswordRequest, RemoteArchivePasswordResume, RemoteArchiveProgress,
    RemoteArchiveRecoverableResponse, RemoteArchiveResultResponse, RemoteArchiveStartRequest,
    RemoteArchiveStartResponse, RemoteArchiveStateResponse, RemoteArchiveTerminalCode,
    RemoteArchiveTerminalDetail, RemoteSubresource,
};

use super::long_job::{RemoteLongJobDrainCause, RemoteLongJobRegistry};
use super::session::{SessionHandle, SessionOperation};

const TERMINAL_RETENTION: Duration = Duration::from_secs(10 * 60);
const TOMBSTONE_LIMIT: usize = 256;
const MAX_PASSWORD_BYTES: usize = 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
struct SourceFingerprint {
    mtime: i64,
    size: i64,
}

/// Core-only load target. The public source stays stable while the backing can be a resolved RAR
/// volume or a cache ZIP. Deliberately do not implement Serialize for either type.
#[derive(Clone)]
pub(crate) struct RemoteArchiveOpenTarget {
    pub(crate) source: RemoteAddress,
    pub(crate) backing: RemoteArchiveBacking,
    source_fingerprint: SourceFingerprint,
}

#[derive(Clone)]
pub(crate) enum RemoteArchiveBacking {
    DirectRar { resolved_path: PathBuf },
    CachedZip { path: PathBuf, source_path: PathBuf },
}

impl RemoteArchiveOpenTarget {
    fn public_result(&self) -> RemoteArchiveOpenResult {
        let access = match &self.backing {
            RemoteArchiveBacking::DirectRar { .. } => RemoteArchiveAccessMode::DirectRar,
            RemoteArchiveBacking::CachedZip { .. } => RemoteArchiveAccessMode::CachedZip,
        };
        RemoteArchiveOpenResult {
            source: self.source.clone(),
            access,
        }
    }

    pub(crate) fn validated_backing_path(&self) -> Option<&Path> {
        let source_path = match &self.backing {
            RemoteArchiveBacking::DirectRar { resolved_path } => resolved_path,
            RemoteArchiveBacking::CachedZip { source_path, .. } => source_path,
        };
        let backing_path = match &self.backing {
            RemoteArchiveBacking::DirectRar { resolved_path } => resolved_path.as_path(),
            RemoteArchiveBacking::CachedZip { path, .. } => path.as_path(),
        };
        (source_fingerprint(source_path).ok().as_ref() == Some(&self.source_fingerprint)
            && backing_path.exists())
        .then_some(backing_path)
    }
}

pub(super) struct ContainerRemoteArchiveExecutor {
    engine: Arc<super::container::ContainerEngine>,
    session: SessionHandle,
}

impl ContainerRemoteArchiveExecutor {
    pub(super) fn new(
        engine: Arc<super::container::ContainerEngine>,
        session: SessionHandle,
    ) -> Self {
        Self { engine, session }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct CancelAwareExecutor {
        started: mpsc::Sender<()>,
        cancelled: mpsc::Sender<()>,
    }

    struct NoInputControl;

    impl RemoteArchiveJobControl for NoInputControl {
        fn update(&self, _state: RemoteArchiveJobState, _progress: Option<RemoteArchiveProgress>) {}

        fn await_confirmation(&self, _summary: RemoteArchiveImageSummary) -> Result<bool, ()> {
            panic!("cache-unavailable flow must not request confirmation")
        }

        fn await_password(
            &self,
            _resume: RemoteArchivePasswordResume,
            _bad_password: bool,
        ) -> Result<String, ()> {
            panic!("cache-unavailable flow must not request a password")
        }
    }

    impl RemoteArchiveExecutor for CancelAwareExecutor {
        fn execute(
            &self,
            _request: &RemoteArchiveStartRequest,
            control: &dyn RemoteArchiveJobControl,
            cancel: &Arc<AtomicBool>,
        ) -> RemoteArchiveExecutionOutcome {
            control.update(RemoteArchiveJobState::Converting, None);
            self.started.send(()).unwrap();
            let deadline = Instant::now() + Duration::from_secs(2);
            while !cancel.load(Ordering::Acquire) && Instant::now() < deadline {
                std::thread::yield_now();
            }
            self.cancelled.send(()).unwrap();
            cancelled_outcome()
        }
    }

    fn peer() -> mimageviewer_ipc::SessionPeerInfo {
        mimageviewer_ipc::SessionPeerInfo {
            connection_kind: mimageviewer_ipc::SessionConnectionKind::Direct,
            device_name: Some("archive test".to_owned()),
        }
    }

    #[test]
    fn owner_handoff_cancels_archive_job_without_blocking_acquire() {
        let (started, started_rx) = mpsc::channel();
        let (cancelled, cancelled_rx) = mpsc::channel();
        let registry =
            RemoteArchiveJobRegistry::new(Arc::new(CancelAwareExecutor { started, cancelled }));
        let session = SessionHandle::new();
        session.install_archive_jobs(&registry);
        assert_eq!(
            session
                .acquire(mimageviewer_ipc::SessionAcquireRequest {
                    client_id: "first".to_owned(),
                    peer: peer(),
                })
                .status,
            mimageviewer_ipc::SessionStatus::Active,
        );
        assert!(session.finish_acquire(session.snapshot().generation));
        let owner = session.owner_for_test("first");
        let operation = session
            .begin_operation(&owner, "archive test".to_owned())
            .unwrap();
        let response = registry.start(
            owner.client_id.clone(),
            RemoteArchiveStartRequest {
                request_id: "handoff".to_owned(),
                source: RemoteAddress::file("C:/Books/book.7z"),
            },
            u64::MAX,
            operation,
        );
        let snapshot = match response {
            RemoteArchiveStartResponse::Accepted(snapshot) => snapshot,
            RemoteArchiveStartResponse::Error(error) => panic!("start failed: {error:?}"),
        };
        started_rx.recv_timeout(Duration::from_secs(1)).unwrap();

        let before = Instant::now();
        let acquire = session.acquire(mimageviewer_ipc::SessionAcquireRequest {
            client_id: "second".to_owned(),
            peer: peer(),
        });
        assert_eq!(acquire.status, mimageviewer_ipc::SessionStatus::LocalInUse);
        assert!(before.elapsed() < Duration::from_millis(250));
        cancelled_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        let deadline = Instant::now() + Duration::from_secs(1);
        loop {
            let current = match registry.state("first", &snapshot.job_id) {
                RemoteArchiveStateResponse::Success(snapshot) => snapshot,
                RemoteArchiveStateResponse::Error(error) => panic!("state failed: {error:?}"),
            };
            if current.state == RemoteArchiveJobState::Superseded {
                break;
            }
            assert!(Instant::now() < deadline);
            std::thread::yield_now();
        }
    }

    #[test]
    fn public_result_never_contains_the_cache_backing_path() {
        let temp = tempfile::tempdir().unwrap();
        let source_path = temp.path().join("book.7z");
        let cache_path = temp.path().join("private-cache.zip");
        std::fs::write(&source_path, b"archive").unwrap();
        std::fs::write(&cache_path, b"zip").unwrap();
        let fingerprint = source_fingerprint(&source_path).unwrap();
        let source = RemoteAddress::file(source_path.to_string_lossy().into_owned());
        let target = cached_target(&source, &source_path, fingerprint, cache_path.clone());

        let public = target.public_result();
        assert_eq!(public.source, source);
        assert_eq!(public.access, RemoteArchiveAccessMode::CachedZip);
        let encoded = serde_json::to_string(&public).unwrap();
        assert!(!encoded.contains(cache_path.to_string_lossy().as_ref()));
    }

    #[test]
    fn missing_cache_db_is_an_explicit_conversion_error() {
        match cache_unavailable() {
            RemoteArchiveExecutionOutcome::Failed(code, message) => {
                assert_eq!(code, RemoteArchiveTerminalCode::CacheUnavailable);
                assert!(message.contains("変換できません"));
            }
            _ => panic!("missing cache must be terminal"),
        }
    }

    #[test]
    fn missing_cache_db_stops_convertible_format_before_scan_or_input() {
        let _data_dir = crate::data_dir::TestDataDirGuard::new();
        let temp = tempfile::tempdir().unwrap();
        let source_path = temp.path().join("book.7z");
        std::fs::write(&source_path, b"not scanned").unwrap();
        let session = SessionHandle::new();
        let engine = Arc::new(super::super::container::ContainerEngine::new(
            crate::settings::Settings::default(),
        ));
        let executor = ContainerRemoteArchiveExecutor::new(engine, session);
        let outcome = executor.execute(
            &RemoteArchiveStartRequest {
                request_id: "no-cache".to_owned(),
                source: RemoteAddress::file(source_path.to_string_lossy().into_owned()),
            },
            &NoInputControl,
            &Arc::new(AtomicBool::new(false)),
        );
        assert!(matches!(
            outcome,
            RemoteArchiveExecutionOutcome::Failed(RemoteArchiveTerminalCode::CacheUnavailable, _)
        ));
    }

    #[test]
    fn conversion_progress_high_water_never_moves_backwards() {
        let previous = RemoteArchiveProgress {
            files_done: 8,
            files_total: 10,
            bytes_written: 4_096,
        };
        let current = RemoteArchiveProgress {
            files_done: 2,
            files_total: 12,
            bytes_written: 1_024,
        };
        assert_eq!(
            monotonic_progress(previous, current),
            RemoteArchiveProgress {
                files_done: 8,
                files_total: 12,
                bytes_written: 4_096,
            }
        );
    }
}

fn source_fingerprint(path: &Path) -> std::io::Result<SourceFingerprint> {
    let metadata = std::fs::metadata(path)?;
    Ok(SourceFingerprint {
        mtime: crate::ui_helpers::mtime_secs(&metadata),
        size: metadata.len().min(i64::MAX as u64) as i64,
    })
}

fn cached_target(
    source: &RemoteAddress,
    source_path: &Path,
    source_fingerprint: SourceFingerprint,
    path: PathBuf,
) -> RemoteArchiveOpenTarget {
    RemoteArchiveOpenTarget {
        source: source.clone(),
        backing: RemoteArchiveBacking::CachedZip {
            path,
            source_path: source_path.to_path_buf(),
        },
        source_fingerprint,
    }
}

fn cached_outcome(
    source: &RemoteAddress,
    source_path: &Path,
    fingerprint: SourceFingerprint,
    path: PathBuf,
) -> RemoteArchiveExecutionOutcome {
    RemoteArchiveExecutionOutcome::Completed(cached_target(source, source_path, fingerprint, path))
}

fn public_summary(
    summary: crate::archive_converter::ArchiveImageSummary,
) -> RemoteArchiveImageSummary {
    RemoteArchiveImageSummary {
        image_count: summary.image_count,
        total_uncompressed_bytes: summary.total_uncompressed_bytes,
        nested_archive_count: summary.nested_archive_count,
    }
}

fn monotonic_progress(
    previous: RemoteArchiveProgress,
    current: RemoteArchiveProgress,
) -> RemoteArchiveProgress {
    RemoteArchiveProgress {
        files_done: previous.files_done.max(current.files_done),
        files_total: previous.files_total.max(current.files_total),
        bytes_written: previous.bytes_written.max(current.bytes_written),
    }
}

fn cancelled_outcome() -> RemoteArchiveExecutionOutcome {
    RemoteArchiveExecutionOutcome::Failed(
        RemoteArchiveTerminalCode::CancelledByUser,
        "アーカイブ操作はキャンセルされました".to_string(),
    )
}

fn scan_with_password_retry(
    path: &Path,
    format: crate::archive_converter::ArchiveFormat,
    password: &mut Option<String>,
    control: &dyn RemoteArchiveJobControl,
    cancel: &Arc<AtomicBool>,
) -> Result<crate::archive_converter::ArchiveImageSummary, RemoteArchiveExecutionOutcome> {
    loop {
        let result = crate::archive_converter::scan_summary_with_password_cancelable(
            path,
            format,
            password.as_deref(),
            cancel,
        );
        match result {
            Ok(summary) => return Ok(summary),
            Err(crate::archive_converter::ConvertError::Cancelled) => {
                return Err(cancelled_outcome());
            }
            Err(crate::archive_converter::ConvertError::PasswordUnsupported) => {
                return Err(password_unsupported());
            }
            Err(crate::archive_converter::ConvertError::PasswordRequired)
            | Err(crate::archive_converter::ConvertError::BadPassword) => {
                *password = match control
                    .await_password(RemoteArchivePasswordResume::Inspect, password.is_some())
                {
                    Ok(password) => Some(password),
                    Err(()) => return Err(cancelled_outcome()),
                };
            }
            Err(crate::archive_converter::ConvertError::NoImages) => {
                return Err(failed(
                    RemoteArchiveTerminalCode::NoImages,
                    "画像がありません",
                ));
            }
            Err(error) => return Err(conversion_failed(error)),
        }
    }
}

fn failed(
    code: RemoteArchiveTerminalCode,
    message: impl Into<String>,
) -> RemoteArchiveExecutionOutcome {
    RemoteArchiveExecutionOutcome::Failed(code, message.into())
}

fn source_changed() -> RemoteArchiveExecutionOutcome {
    RemoteArchiveExecutionOutcome::SourceChanged(
        "変換中に元のアーカイブが更新されました".to_string(),
    )
}

fn password_unsupported() -> RemoteArchiveExecutionOutcome {
    failed(
        RemoteArchiveTerminalCode::PasswordUnsupported,
        "この形式のパスワード付きアーカイブは変換できません",
    )
}

fn rar_inspection_failed() -> RemoteArchiveExecutionOutcome {
    failed(
        RemoteArchiveTerminalCode::ExecutionFailed,
        "RAR の内容を確認できませんでした",
    )
}

fn rar_first_volume_failed() -> RemoteArchiveExecutionOutcome {
    failed(
        RemoteArchiveTerminalCode::ExecutionFailed,
        "分割 RAR の先頭ボリュームを開けませんでした",
    )
}

fn cache_publish_failed() -> RemoteArchiveExecutionOutcome {
    failed(
        RemoteArchiveTerminalCode::CacheUnavailable,
        "変換キャッシュを保存できませんでした",
    )
}

fn cache_unavailable() -> RemoteArchiveExecutionOutcome {
    failed(
        RemoteArchiveTerminalCode::CacheUnavailable,
        "アーカイブ変換キャッシュを初期化できなかったため、この形式は変換できません",
    )
}

fn conversion_failed(
    error: crate::archive_converter::ConvertError,
) -> RemoteArchiveExecutionOutcome {
    crate::logger::log(format!("remote_archive: conversion failed: {error}"));
    failed(
        RemoteArchiveTerminalCode::ExecutionFailed,
        "アーカイブの変換に失敗しました",
    )
}

fn validate_archive_start(
    request: &RemoteArchiveStartRequest,
) -> Result<(), RemoteArchiveJobError> {
    if request.request_id.is_empty() || request.request_id.len() > 128 {
        return Err(RemoteArchiveJobError::new(
            RemoteArchiveJobErrorCode::BadRequest,
            "request_id must contain 1 to 128 bytes",
        ));
    }
    request.source.validate_syntax().map_err(|_| {
        RemoteArchiveJobError::new(
            RemoteArchiveJobErrorCode::BadRequest,
            "invalid archive address",
        )
    })?;
    if request.source.subresource != RemoteSubresource::File {
        return Err(RemoteArchiveJobError::new(
            RemoteArchiveJobErrorCode::BadRequest,
            "archive source must identify a file",
        ));
    }
    Ok(())
}

fn archive_terminal(
    code: RemoteArchiveTerminalCode,
    message: impl Into<String>,
) -> RemoteArchiveTerminalDetail {
    RemoteArchiveTerminalDetail {
        code,
        message: message.into(),
    }
}

fn set_archive_terminal(
    job: &mut ArchiveJobEntry,
    state: RemoteArchiveJobState,
    terminal: RemoteArchiveTerminalDetail,
    now: Duration,
) {
    job.snapshot.state = state;
    job.snapshot.progress = None;
    job.snapshot.awaiting_input = None;
    job.snapshot.terminal = Some(terminal);
    job.snapshot.updated_unix_ms = RemoteArchiveJobRegistry::unix_ms();
    job.result = None;
    job.terminal_at = Some(now);
}

fn request_archive_cancel(
    job: &mut ArchiveJobEntry,
    state: RemoteArchiveJobState,
    code: RemoteArchiveTerminalCode,
    message: impl Into<String>,
) {
    if job.snapshot.state.is_terminal() || job.requested_terminal.is_some() {
        return;
    }
    job.requested_terminal = Some((state, archive_terminal(code, message)));
    job.snapshot.state = RemoteArchiveJobState::Cancelling;
    job.snapshot.progress = None;
    job.snapshot.awaiting_input = None;
    job.snapshot.updated_unix_ms = RemoteArchiveJobRegistry::unix_ms();
    job.cancel.store(true, Ordering::Release);
    let _ = job.input.try_send(ArchiveInput::Cancel);
}

fn terminal_state_for(code: RemoteArchiveTerminalCode) -> RemoteArchiveJobState {
    match code {
        RemoteArchiveTerminalCode::DeclinedByUser => RemoteArchiveJobState::DeclinedByUser,
        RemoteArchiveTerminalCode::Superseded => RemoteArchiveJobState::Superseded,
        RemoteArchiveTerminalCode::CancelledByUser => RemoteArchiveJobState::CancelledByUser,
        RemoteArchiveTerminalCode::DiscardedByHost => RemoteArchiveJobState::DiscardedByHost,
        RemoteArchiveTerminalCode::BackgroundExpired => RemoteArchiveJobState::BackgroundExpired,
        _ => RemoteArchiveJobState::Failed,
    }
}

fn lookup_archive_job<'a>(
    state: &'a ArchiveRegistryState,
    owner: &str,
    job_id: &str,
) -> Result<&'a ArchiveJobEntry, RemoteArchiveJobError> {
    if let Some(job) = state.jobs.get(job_id) {
        return if job.owner == owner {
            Ok(job)
        } else {
            Err(archive_forbidden_error())
        };
    }
    Err(missing_archive_job_error(state, owner, job_id))
}

fn missing_archive_job_error(
    state: &ArchiveRegistryState,
    owner: &str,
    job_id: &str,
) -> RemoteArchiveJobError {
    if let Some(tombstone) = state
        .tombstones
        .iter()
        .find(|entry| entry.owner == owner && entry.job_id == job_id)
    {
        let mut error = RemoteArchiveJobError::new(
            RemoteArchiveJobErrorCode::JobGone,
            "archive job terminal retention has expired",
        );
        error.terminal_code = tombstone.terminal_code;
        return error;
    }
    RemoteArchiveJobError::new(
        RemoteArchiveJobErrorCode::NotFound,
        "archive job was not found",
    )
}

fn archive_forbidden_error() -> RemoteArchiveJobError {
    RemoteArchiveJobError::new(
        RemoteArchiveJobErrorCode::Forbidden,
        "archive job belongs to another client",
    )
}

fn archive_invalid_state() -> RemoteArchiveJobError {
    RemoteArchiveJobError::new(
        RemoteArchiveJobErrorCode::InvalidState,
        "archive job is not waiting for this input",
    )
}

fn archive_source_changed_error() -> RemoteArchiveJobError {
    let mut error = RemoteArchiveJobError::new(
        RemoteArchiveJobErrorCode::NotReady,
        "archive source or backing changed after preparation",
    );
    error.terminal_code = Some(RemoteArchiveTerminalCode::SourceChanged);
    error
}

fn archive_drain_terminal(
    cause: RemoteLongJobDrainCause,
) -> (
    RemoteArchiveJobState,
    RemoteArchiveTerminalCode,
    &'static str,
) {
    match cause {
        RemoteLongJobDrainCause::DiscardedByHost => (
            RemoteArchiveJobState::DiscardedByHost,
            RemoteArchiveTerminalCode::DiscardedByHost,
            "PC 側で接続が終了されたためアーカイブ操作を中止しました",
        ),
        RemoteLongJobDrainCause::BackgroundExpired => (
            RemoteArchiveJobState::BackgroundExpired,
            RemoteArchiveTerminalCode::BackgroundExpired,
            "バックグラウンド保持時間を超えたためアーカイブ操作を中止しました",
        ),
        RemoteLongJobDrainCause::Superseded => (
            RemoteArchiveJobState::Superseded,
            RemoteArchiveTerminalCode::Superseded,
            "別の端末へ操作権が移ったためアーカイブ操作を中止しました",
        ),
    }
}

pub(crate) trait RemoteArchiveJobControl: Send + Sync {
    fn update(&self, state: RemoteArchiveJobState, progress: Option<RemoteArchiveProgress>);
    fn await_confirmation(&self, summary: RemoteArchiveImageSummary) -> Result<bool, ()>;
    fn await_password(
        &self,
        resume: RemoteArchivePasswordResume,
        bad_password: bool,
    ) -> Result<String, ()>;
}

pub(crate) trait RemoteArchiveExecutor: Send + Sync {
    fn execute(
        &self,
        request: &RemoteArchiveStartRequest,
        control: &dyn RemoteArchiveJobControl,
        cancel: &Arc<AtomicBool>,
    ) -> RemoteArchiveExecutionOutcome;
}

pub(crate) enum RemoteArchiveExecutionOutcome {
    Completed(RemoteArchiveOpenTarget),
    Declined,
    SourceChanged(String),
    Failed(RemoteArchiveTerminalCode, String),
}

enum ArchiveInput {
    Confirmation(bool),
    Password(String),
    Cancel,
}

struct RemoteArchiveJobLease {
    operation: Option<SessionOperation>,
}

impl RemoteArchiveJobLease {
    fn new(operation: SessionOperation) -> Self {
        Self {
            operation: Some(operation),
        }
    }

    fn wait_until_active(&self) -> Result<(), mimageviewer_ipc::SessionResponse> {
        self.operation
            .as_ref()
            .expect("live archive lease")
            .wait_until_active()
    }

    fn started(&self) {
        self.operation
            .as_ref()
            .expect("live archive lease")
            .started();
    }

    fn drain_cause(&self) -> Option<RemoteLongJobDrainCause> {
        self.operation
            .as_ref()
            .expect("live archive lease")
            .long_job_drain_cause()
    }

    fn finish(mut self, success: bool) {
        if let Some(operation) = self.operation.take() {
            operation.finish(success);
        }
    }
}

struct ArchiveJobEntry {
    owner: String,
    request: RemoteArchiveStartRequest,
    snapshot: RemoteArchiveJobSnapshot,
    cancel: Arc<AtomicBool>,
    input: mpsc::SyncSender<ArchiveInput>,
    requested_terminal: Option<(RemoteArchiveJobState, RemoteArchiveTerminalDetail)>,
    progress_high_water: RemoteArchiveProgress,
    result: Option<RemoteArchiveOpenTarget>,
    terminal_at: Option<Duration>,
}

struct ArchiveTombstone {
    owner: String,
    job_id: String,
    terminal_code: Option<RemoteArchiveTerminalCode>,
}

#[derive(Default)]
struct ArchiveRegistryState {
    next_sequence: u64,
    jobs: HashMap<String, ArchiveJobEntry>,
    order: VecDeque<String>,
    tombstones: VecDeque<ArchiveTombstone>,
}

pub(crate) struct RemoteArchiveJobRegistry {
    inner: Mutex<ArchiveRegistryState>,
    executor: Arc<dyn RemoteArchiveExecutor>,
    origin: Instant,
}

impl RemoteArchiveJobRegistry {
    pub(crate) fn new(executor: Arc<dyn RemoteArchiveExecutor>) -> Arc<Self> {
        Arc::new(Self {
            inner: Mutex::new(ArchiveRegistryState::default()),
            executor,
            origin: Instant::now(),
        })
    }

    fn now(&self) -> Duration {
        self.origin.elapsed()
    }

    fn unix_ms() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
            .try_into()
            .unwrap_or(u64::MAX)
    }

    fn has_nonterminal_jobs_inner(&self) -> bool {
        self.inner
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .jobs
            .values()
            .any(|job| !job.snapshot.state.is_terminal())
    }

    pub(crate) fn start(
        self: &Arc<Self>,
        owner: String,
        request: RemoteArchiveStartRequest,
        accept_before_unix_ms: u64,
        operation: SessionOperation,
    ) -> RemoteArchiveStartResponse {
        if let Err(error) = validate_archive_start(&request) {
            return RemoteArchiveStartResponse::Error(error);
        }
        if Self::unix_ms() > accept_before_unix_ms {
            return RemoteArchiveStartResponse::Error(RemoteArchiveJobError::new(
                RemoteArchiveJobErrorCode::StartExpired,
                "archive start was not admitted within two seconds",
            ));
        }
        let now = self.now();
        let unix_ms = Self::unix_ms();
        let cancel = operation.cancel_flag();
        let generation = operation.generation();
        let mut state = self.inner.lock().unwrap_or_else(|error| error.into_inner());
        self.prune_locked(&mut state, now);
        if let Some(existing) = state
            .jobs
            .values()
            .find(|job| job.owner == owner && job.request.request_id == request.request_id)
        {
            return RemoteArchiveStartResponse::Accepted(existing.snapshot.clone());
        }
        for job in state
            .jobs
            .values_mut()
            .filter(|job| job.owner == owner && !job.snapshot.state.is_terminal())
        {
            request_archive_cancel(
                job,
                RemoteArchiveJobState::Superseded,
                RemoteArchiveTerminalCode::Superseded,
                "新しいアーカイブ操作へ切り替えました",
            );
        }
        state.next_sequence = state.next_sequence.wrapping_add(1).max(1);
        let job_id = format!("archive-{generation}-{}", state.next_sequence);
        let (input, input_rx) = mpsc::sync_channel(1);
        let snapshot = RemoteArchiveJobSnapshot {
            job_id: job_id.clone(),
            request_id: request.request_id.clone(),
            source: request.source.clone(),
            state: RemoteArchiveJobState::WaitingForLocalDrain,
            progress: None,
            awaiting_input: None,
            terminal: None,
            created_unix_ms: unix_ms,
            updated_unix_ms: unix_ms,
        };
        state.order.push_back(job_id.clone());
        state.jobs.insert(
            job_id.clone(),
            ArchiveJobEntry {
                owner,
                request: request.clone(),
                snapshot: snapshot.clone(),
                cancel,
                input,
                requested_terminal: None,
                progress_high_water: RemoteArchiveProgress::default(),
                result: None,
                terminal_at: None,
            },
        );
        drop(state);
        self.spawn_job(
            job_id,
            request,
            input_rx,
            RemoteArchiveJobLease::new(operation),
        );
        RemoteArchiveStartResponse::Accepted(snapshot)
    }

    fn spawn_job(
        self: &Arc<Self>,
        job_id: String,
        request: RemoteArchiveStartRequest,
        input: mpsc::Receiver<ArchiveInput>,
        lease: RemoteArchiveJobLease,
    ) {
        let registry = Arc::downgrade(self);
        let executor = Arc::clone(&self.executor);
        let thread_job_id = job_id.clone();
        let spawn = std::thread::Builder::new()
            .name(format!("remote-archive-{job_id}"))
            .spawn(move || {
                if lease.wait_until_active().is_err() {
                    if let Some(registry) = registry.upgrade() {
                        if let Some(cause) = lease.drain_cause() {
                            registry.on_session_drain_inner(cause);
                        }
                        registry.complete_cancelled_if_requested(&thread_job_id);
                    }
                    lease.finish(false);
                    return;
                }
                lease.started();
                let Some(registry) = registry.upgrade() else {
                    lease.finish(false);
                    return;
                };
                let control = RegistryArchiveControl {
                    registry: Arc::downgrade(&registry),
                    job_id: thread_job_id.clone(),
                    input: Mutex::new(input),
                };
                let cancel = registry.cancel_for(&thread_job_id);
                let outcome = match cancel {
                    Some(cancel) => executor.execute(&request, &control, &cancel),
                    None => failed(
                        RemoteArchiveTerminalCode::ExecutionFailed,
                        "アーカイブ操作を開始できませんでした",
                    ),
                };
                if let Some(cause) = lease.drain_cause() {
                    registry.on_session_drain_inner(cause);
                }
                let success = matches!(outcome, RemoteArchiveExecutionOutcome::Completed(_));
                registry.complete(&thread_job_id, outcome);
                lease.finish(success);
            });
        if let Err(error) = spawn {
            crate::logger::log(format!("remote_archive: thread start failed: {error}"));
            self.complete(
                &job_id,
                failed(
                    RemoteArchiveTerminalCode::ExecutionFailed,
                    "アーカイブ操作を開始できませんでした",
                ),
            );
        }
    }

    fn cancel_for(&self, job_id: &str) -> Option<Arc<AtomicBool>> {
        self.inner
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .jobs
            .get(job_id)
            .map(|job| Arc::clone(&job.cancel))
    }

    fn complete_cancelled_if_requested(&self, job_id: &str) {
        let mut state = self.inner.lock().unwrap_or_else(|error| error.into_inner());
        let Some(job) = state.jobs.get_mut(job_id) else {
            return;
        };
        if let Some((terminal_state, terminal)) = job.requested_terminal.take() {
            set_archive_terminal(job, terminal_state, terminal, self.now());
        } else {
            set_archive_terminal(
                job,
                RemoteArchiveJobState::Failed,
                archive_terminal(
                    RemoteArchiveTerminalCode::ExecutionFailed,
                    "アーカイブ操作を開始できませんでした",
                ),
                self.now(),
            );
        }
    }

    fn complete(&self, job_id: &str, outcome: RemoteArchiveExecutionOutcome) {
        let mut state = self.inner.lock().unwrap_or_else(|error| error.into_inner());
        let Some(job) = state.jobs.get_mut(job_id) else {
            return;
        };
        let now = self.now();
        if let Some((terminal_state, terminal)) = job.requested_terminal.take() {
            set_archive_terminal(job, terminal_state, terminal, now);
            return;
        }
        match outcome {
            RemoteArchiveExecutionOutcome::Completed(result) => {
                job.result = Some(result);
                job.snapshot.state = RemoteArchiveJobState::Ready;
                job.snapshot.progress = None;
                job.snapshot.awaiting_input = None;
                job.snapshot.terminal = None;
                job.snapshot.updated_unix_ms = Self::unix_ms();
                job.terminal_at = Some(now);
            }
            RemoteArchiveExecutionOutcome::Declined => set_archive_terminal(
                job,
                RemoteArchiveJobState::DeclinedByUser,
                archive_terminal(
                    RemoteArchiveTerminalCode::DeclinedByUser,
                    "変換しませんでした",
                ),
                now,
            ),
            RemoteArchiveExecutionOutcome::SourceChanged(message) => set_archive_terminal(
                job,
                RemoteArchiveJobState::Failed,
                archive_terminal(RemoteArchiveTerminalCode::SourceChanged, message),
                now,
            ),
            RemoteArchiveExecutionOutcome::Failed(code, message) => {
                let state = terminal_state_for(code);
                set_archive_terminal(job, state, archive_terminal(code, message), now);
            }
        }
    }

    pub(crate) fn state(&self, owner: &str, job_id: &str) -> RemoteArchiveStateResponse {
        let mut state = self.inner.lock().unwrap_or_else(|error| error.into_inner());
        self.prune_locked(&mut state, self.now());
        match lookup_archive_job(&state, owner, job_id) {
            Ok(job) => RemoteArchiveStateResponse::Success(job.snapshot.clone()),
            Err(error) => RemoteArchiveStateResponse::Error(error),
        }
    }

    pub(crate) fn recoverable(&self, owner: &str) -> RemoteArchiveRecoverableResponse {
        let mut state = self.inner.lock().unwrap_or_else(|error| error.into_inner());
        self.prune_locked(&mut state, self.now());
        RemoteArchiveRecoverableResponse::Success(
            state
                .order
                .iter()
                .filter_map(|job_id| state.jobs.get(job_id))
                .filter(|job| job.owner == owner)
                .map(|job| job.snapshot.clone())
                .collect(),
        )
    }

    pub(crate) fn cancel(&self, owner: &str, job_id: &str) -> RemoteArchiveCancelResponse {
        let mut state = self.inner.lock().unwrap_or_else(|error| error.into_inner());
        self.prune_locked(&mut state, self.now());
        let Some(job) = state.jobs.get_mut(job_id) else {
            return RemoteArchiveCancelResponse::Error(missing_archive_job_error(
                &state, owner, job_id,
            ));
        };
        if job.owner != owner {
            return RemoteArchiveCancelResponse::Error(archive_forbidden_error());
        }
        if !job.snapshot.state.is_terminal() {
            request_archive_cancel(
                job,
                RemoteArchiveJobState::CancelledByUser,
                RemoteArchiveTerminalCode::CancelledByUser,
                "利用者がアーカイブ操作を取り消しました",
            );
        }
        RemoteArchiveCancelResponse::Success(job.snapshot.clone())
    }

    pub(crate) fn confirm(
        &self,
        owner: &str,
        request: RemoteArchiveConfirmRequest,
    ) -> RemoteArchiveInputResponse {
        let mut state = self.inner.lock().unwrap_or_else(|error| error.into_inner());
        self.prune_locked(&mut state, self.now());
        let Some(job) = state.jobs.get_mut(&request.job_id) else {
            return RemoteArchiveInputResponse::Error(missing_archive_job_error(
                &state,
                owner,
                &request.job_id,
            ));
        };
        if job.owner != owner {
            return RemoteArchiveInputResponse::Error(archive_forbidden_error());
        }
        if !matches!(
            job.snapshot.awaiting_input,
            Some(RemoteArchiveAwaitingInput::Confirmation { .. })
        ) {
            return RemoteArchiveInputResponse::Error(archive_invalid_state());
        }
        if job
            .input
            .try_send(ArchiveInput::Confirmation(request.proceed))
            .is_err()
        {
            return RemoteArchiveInputResponse::Error(archive_invalid_state());
        }
        job.snapshot.awaiting_input = None;
        job.snapshot.state = RemoteArchiveJobState::Inspecting;
        job.snapshot.updated_unix_ms = Self::unix_ms();
        RemoteArchiveInputResponse::Success(job.snapshot.clone())
    }

    pub(crate) fn password(
        &self,
        owner: &str,
        request: RemoteArchivePasswordRequest,
    ) -> RemoteArchiveInputResponse {
        if request.password.is_empty()
            || request.password.len() > MAX_PASSWORD_BYTES
            || request.password.as_bytes().contains(&0)
        {
            return RemoteArchiveInputResponse::Error(RemoteArchiveJobError::new(
                RemoteArchiveJobErrorCode::BadRequest,
                "password must contain 1 to 1024 bytes and no NUL",
            ));
        }
        let mut state = self.inner.lock().unwrap_or_else(|error| error.into_inner());
        self.prune_locked(&mut state, self.now());
        let Some(job) = state.jobs.get_mut(&request.job_id) else {
            return RemoteArchiveInputResponse::Error(missing_archive_job_error(
                &state,
                owner,
                &request.job_id,
            ));
        };
        if job.owner != owner {
            return RemoteArchiveInputResponse::Error(archive_forbidden_error());
        }
        let resume = match job.snapshot.awaiting_input.as_ref() {
            Some(RemoteArchiveAwaitingInput::Password { resume, .. }) => *resume,
            _ => return RemoteArchiveInputResponse::Error(archive_invalid_state()),
        };
        if job
            .input
            .try_send(ArchiveInput::Password(request.password))
            .is_err()
        {
            return RemoteArchiveInputResponse::Error(archive_invalid_state());
        }
        job.snapshot.awaiting_input = None;
        job.snapshot.state = match resume {
            RemoteArchivePasswordResume::Inspect => RemoteArchiveJobState::Inspecting,
            RemoteArchivePasswordResume::Convert => RemoteArchiveJobState::WaitingForConversionSlot,
        };
        job.snapshot.updated_unix_ms = Self::unix_ms();
        RemoteArchiveInputResponse::Success(job.snapshot.clone())
    }

    pub(crate) fn result(&self, owner: &str, job_id: &str) -> RemoteArchiveResultResponse {
        let mut state = self.inner.lock().unwrap_or_else(|error| error.into_inner());
        self.prune_locked(&mut state, self.now());
        let job = match lookup_archive_job(&state, owner, job_id) {
            Ok(job) => job,
            Err(error) => return RemoteArchiveResultResponse::Error(error),
        };
        if job.snapshot.state != RemoteArchiveJobState::Ready {
            return RemoteArchiveResultResponse::Error(RemoteArchiveJobError::new(
                RemoteArchiveJobErrorCode::NotReady,
                "archive is not ready",
            ));
        }
        match job.result.as_ref() {
            Some(result) => RemoteArchiveResultResponse::Success(result.public_result()),
            None => RemoteArchiveResultResponse::Error(RemoteArchiveJobError::new(
                RemoteArchiveJobErrorCode::Internal,
                "archive backing is unavailable",
            )),
        }
    }

    fn request_cancel_all(
        &self,
        state_value: RemoteArchiveJobState,
        code: RemoteArchiveTerminalCode,
        message: &str,
    ) {
        let mut state = self.inner.lock().unwrap_or_else(|error| error.into_inner());
        for job in state
            .jobs
            .values_mut()
            .filter(|job| !job.snapshot.state.is_terminal())
        {
            request_archive_cancel(job, state_value, code, message);
        }
    }

    fn on_session_drain_inner(&self, cause: RemoteLongJobDrainCause) {
        let (state, code, message) = archive_drain_terminal(cause);
        self.request_cancel_all(state, code, message);
    }

    fn prune_locked(&self, state: &mut ArchiveRegistryState, now: Duration) {
        let expired: Vec<String> = state
            .order
            .iter()
            .filter_map(|job_id| {
                state.jobs.get(job_id).and_then(|job| {
                    job.terminal_at
                        .filter(|at| now.saturating_sub(*at) >= TERMINAL_RETENTION)
                        .map(|_| job_id.clone())
                })
            })
            .collect();
        for job_id in expired {
            if let Some(job) = state.jobs.remove(&job_id) {
                state.tombstones.push_back(ArchiveTombstone {
                    owner: job.owner,
                    job_id: job.snapshot.job_id,
                    terminal_code: job.snapshot.terminal.map(|terminal| terminal.code),
                });
            }
            state.order.retain(|ordered| ordered != &job_id);
        }
        while state.tombstones.len() > TOMBSTONE_LIMIT {
            state.tombstones.pop_front();
        }
    }

    pub(crate) fn open_target(
        &self,
        owner: &str,
        job_id: &str,
    ) -> Result<RemoteArchiveOpenTarget, RemoteArchiveJobError> {
        let state = self.inner.lock().unwrap_or_else(|error| error.into_inner());
        let job = lookup_archive_job(&state, owner, job_id)?;
        if job.snapshot.state != RemoteArchiveJobState::Ready {
            return Err(RemoteArchiveJobError::new(
                RemoteArchiveJobErrorCode::NotReady,
                "archive is not ready",
            ));
        }
        let result = job.result.clone().ok_or_else(|| {
            RemoteArchiveJobError::new(
                RemoteArchiveJobErrorCode::Internal,
                "archive backing is unavailable",
            )
        })?;
        drop(state);
        if result.validated_backing_path().is_none() {
            return Err(archive_source_changed_error());
        }
        Ok(result)
    }
}

impl RemoteLongJobRegistry for RemoteArchiveJobRegistry {
    fn has_nonterminal_jobs(&self) -> bool {
        self.has_nonterminal_jobs_inner()
    }

    fn on_session_drain(&self, cause: RemoteLongJobDrainCause) {
        self.on_session_drain_inner(cause);
    }
}

struct RegistryArchiveControl {
    registry: Weak<RemoteArchiveJobRegistry>,
    job_id: String,
    input: Mutex<mpsc::Receiver<ArchiveInput>>,
}

impl RegistryArchiveControl {
    fn receive(&self) -> Result<ArchiveInput, ()> {
        self.input
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .recv()
            .map_err(|_| ())
    }

    fn set_awaiting(&self, input: RemoteArchiveAwaitingInput) -> Result<(), ()> {
        let Some(registry) = self.registry.upgrade() else {
            return Err(());
        };
        let mut state = registry
            .inner
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let Some(job) = state.jobs.get_mut(&self.job_id) else {
            return Err(());
        };
        if job.requested_terminal.is_some() || job.snapshot.state.is_terminal() {
            return Err(());
        }
        job.snapshot.state = match &input {
            RemoteArchiveAwaitingInput::Confirmation { .. } => {
                RemoteArchiveJobState::AwaitingConfirmation
            }
            RemoteArchiveAwaitingInput::Password { .. } => RemoteArchiveJobState::AwaitingPassword,
        };
        job.snapshot.awaiting_input = Some(input);
        job.snapshot.progress = None;
        job.snapshot.updated_unix_ms = RemoteArchiveJobRegistry::unix_ms();
        Ok(())
    }
}

impl RemoteArchiveJobControl for RegistryArchiveControl {
    fn update(&self, state_value: RemoteArchiveJobState, progress: Option<RemoteArchiveProgress>) {
        let Some(registry) = self.registry.upgrade() else {
            return;
        };
        let mut state = registry
            .inner
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let Some(job) = state.jobs.get_mut(&self.job_id) else {
            return;
        };
        if job.requested_terminal.is_some() || job.snapshot.state.is_terminal() {
            return;
        }
        job.snapshot.state = state_value;
        job.snapshot.progress = progress.map(|progress| {
            let progress = monotonic_progress(job.progress_high_water, progress);
            job.progress_high_water = progress;
            progress
        });
        job.snapshot.awaiting_input = None;
        job.snapshot.updated_unix_ms = RemoteArchiveJobRegistry::unix_ms();
    }

    fn await_confirmation(&self, summary: RemoteArchiveImageSummary) -> Result<bool, ()> {
        self.set_awaiting(RemoteArchiveAwaitingInput::Confirmation { summary })?;
        match self.receive()? {
            ArchiveInput::Confirmation(proceed) => Ok(proceed),
            ArchiveInput::Cancel | ArchiveInput::Password(_) => Err(()),
        }
    }

    fn await_password(
        &self,
        resume: RemoteArchivePasswordResume,
        bad_password: bool,
    ) -> Result<String, ()> {
        self.set_awaiting(RemoteArchiveAwaitingInput::Password {
            resume,
            bad_password,
        })?;
        match self.receive()? {
            ArchiveInput::Password(password) => Ok(password),
            ArchiveInput::Cancel | ArchiveInput::Confirmation(_) => Err(()),
        }
    }
}

impl RemoteArchiveExecutor for ContainerRemoteArchiveExecutor {
    fn execute(
        &self,
        request: &RemoteArchiveStartRequest,
        control: &dyn RemoteArchiveJobControl,
        cancel: &Arc<AtomicBool>,
    ) -> RemoteArchiveExecutionOutcome {
        self.execute_inner(request, control, cancel)
    }
}

enum PreparedArchive {
    Summary(crate::archive_converter::ArchiveImageSummary),
    Target(RemoteArchiveOpenTarget),
}

impl ContainerRemoteArchiveExecutor {
    fn execute_inner(
        &self,
        request: &RemoteArchiveStartRequest,
        control: &dyn RemoteArchiveJobControl,
        cancel: &Arc<AtomicBool>,
    ) -> RemoteArchiveExecutionOutcome {
        control.update(RemoteArchiveJobState::Inspecting, None);
        let public_source = request.source.clone();
        let mut resolved = match super::path_guard::resolve_existing(&request.source.path) {
            Ok(resolved) => resolved,
            Err(super::path_guard::ResolveError::InvalidPath) => {
                return failed(
                    RemoteArchiveTerminalCode::UnsupportedFormat,
                    "アーカイブのパスが不正です",
                );
            }
            Err(super::path_guard::ResolveError::Unavailable) => {
                return failed(
                    RemoteArchiveTerminalCode::ExecutionFailed,
                    "アーカイブを開けませんでした",
                );
            }
        };
        if request.source.subresource != RemoteSubresource::File {
            return failed(
                RemoteArchiveTerminalCode::UnsupportedFormat,
                "アーカイブ内の項目を変換元にはできません",
            );
        }
        let Some(format) = resolved
            .logical
            .extension()
            .and_then(|extension| extension.to_str())
            .and_then(crate::archive_converter::ArchiveFormat::from_extension)
        else {
            return failed(
                RemoteArchiveTerminalCode::UnsupportedFormat,
                "この形式はアーカイブ変換の対象外です",
            );
        };
        let settings = match self.engine.settings_for_listing() {
            Ok(settings) => settings,
            Err(_) => {
                return failed(
                    RemoteArchiveTerminalCode::ExecutionFailed,
                    "最新のアーカイブ設定を読み込めませんでした",
                );
            }
        };
        if settings.archive_file_handling_ignores_convertible() {
            return failed(
                RemoteArchiveTerminalCode::IgnoredBySettings,
                "設定で変換対象アーカイブを表示しないよう指定されています",
            );
        }
        let mut fingerprint = match source_fingerprint(&resolved.canonical) {
            Ok(fingerprint) => fingerprint,
            Err(_) => {
                return failed(
                    RemoteArchiveTerminalCode::ExecutionFailed,
                    "アーカイブの更新情報を読み取れませんでした",
                );
            }
        };
        let cache_db = self.session.archive_cache_db();
        let mut fallback_cached = cache_db
            .as_ref()
            .and_then(|db| db.lookup(&resolved.canonical, fingerprint.mtime, fingerprint.size));
        let mut password = None;
        let summary = if format == crate::archive_converter::ArchiveFormat::Rar {
            match self.prepare_rar(
                &public_source,
                &mut resolved,
                &mut fingerprint,
                &mut fallback_cached,
                cache_db.as_ref(),
                &mut password,
                control,
                cancel,
            ) {
                Ok(PreparedArchive::Target(target)) => {
                    return RemoteArchiveExecutionOutcome::Completed(target);
                }
                Ok(PreparedArchive::Summary(summary)) => summary,
                Err(outcome) => return outcome,
            }
        } else {
            if let Some(path) = fallback_cached.take() {
                return cached_outcome(&public_source, &resolved.canonical, fingerprint, path);
            }
            if cache_db.is_none() {
                return cache_unavailable();
            }
            match scan_with_password_retry(
                &resolved.canonical,
                format,
                &mut password,
                control,
                cancel,
            ) {
                Ok(summary) => summary,
                Err(outcome) => return outcome,
            }
        };
        if summary.image_count == 0 && summary.nested_archive_count == 0 {
            return failed(
                RemoteArchiveTerminalCode::NoImages,
                "このアーカイブには画像ファイルが含まれていません",
            );
        }
        let Some(cache_db) = cache_db else {
            return cache_unavailable();
        };
        if !settings.archive_convert_suppresses_confirm() {
            match control.await_confirmation(public_summary(summary)) {
                Ok(true) => {}
                Ok(false) => return RemoteArchiveExecutionOutcome::Declined,
                Err(()) => return cancelled_outcome(),
            }
        }
        self.convert_to_cache(
            &public_source,
            &resolved,
            fingerprint,
            format,
            summary,
            password,
            cache_db,
            settings.archive_cache_max_bytes,
            control,
            cancel,
        )
    }

    fn prepare_rar(
        &self,
        public_source: &RemoteAddress,
        resolved: &mut super::path_guard::ResolvedPath,
        fingerprint: &mut SourceFingerprint,
        fallback_cached: &mut Option<PathBuf>,
        cache_db: Option<&Arc<crate::archive_cache::ArchiveCacheDb>>,
        password: &mut Option<String>,
        control: &dyn RemoteArchiveJobControl,
        cancel: &Arc<AtomicBool>,
    ) -> Result<PreparedArchive, RemoteArchiveExecutionOutcome> {
        let inspection =
            crate::rar_loader::inspect_for_direct_read_cancelable(&resolved.canonical, cancel);
        let inspection = match inspection {
            Ok(inspection) => inspection,
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {
                return Err(cancelled_outcome());
            }
            Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => {
                return self.prepare_password_rar(
                    public_source,
                    resolved,
                    fingerprint,
                    fallback_cached,
                    cache_db,
                    password,
                    control,
                    cancel,
                );
            }
            Err(_) => return Err(rar_inspection_failed()),
        };
        if inspection.resolved_path != resolved.canonical {
            *resolved = super::path_guard::resolve_existing(
                inspection.resolved_path.to_string_lossy().as_ref(),
            )
            .map_err(|_| rar_first_volume_failed())?;
            *fingerprint =
                source_fingerprint(&resolved.canonical).map_err(|_| rar_first_volume_failed())?;
            *fallback_cached = cache_db
                .and_then(|db| db.lookup(&resolved.canonical, fingerprint.mtime, fingerprint.size));
        }
        if inspection.decision == crate::rar_loader::RarDirectReadDecision::Direct {
            return Ok(PreparedArchive::Target(RemoteArchiveOpenTarget {
                source: public_source.clone(),
                backing: RemoteArchiveBacking::DirectRar {
                    resolved_path: resolved.canonical.clone(),
                },
                source_fingerprint: fingerprint.clone(),
            }));
        }
        if let Some(path) = fallback_cached.take() {
            return Ok(PreparedArchive::Target(cached_target(
                public_source,
                &resolved.canonical,
                fingerprint.clone(),
                path,
            )));
        }
        Ok(PreparedArchive::Summary(inspection.summary))
    }

    fn prepare_password_rar(
        &self,
        public_source: &RemoteAddress,
        resolved: &mut super::path_guard::ResolvedPath,
        fingerprint: &mut SourceFingerprint,
        fallback_cached: &mut Option<PathBuf>,
        cache_db: Option<&Arc<crate::archive_cache::ArchiveCacheDb>>,
        password: &mut Option<String>,
        control: &dyn RemoteArchiveJobControl,
        cancel: &Arc<AtomicBool>,
    ) -> Result<PreparedArchive, RemoteArchiveExecutionOutcome> {
        if let Some(path) = fallback_cached.take() {
            return Ok(PreparedArchive::Target(cached_target(
                public_source,
                &resolved.canonical,
                fingerprint.clone(),
                path,
            )));
        }
        if cache_db.is_none() {
            return Err(cache_unavailable());
        }
        let summary = scan_with_password_retry(
            &resolved.canonical,
            crate::archive_converter::ArchiveFormat::Rar,
            password,
            control,
            cancel,
        )?;
        let first_volume = crate::archive_converter::resolve_rar_source_path(
            &resolved.canonical,
            password.as_deref(),
        )
        .map_err(|_| rar_first_volume_failed())?;
        if first_volume != resolved.canonical {
            *resolved =
                super::path_guard::resolve_existing(first_volume.to_string_lossy().as_ref())
                    .map_err(|_| rar_first_volume_failed())?;
            *fingerprint =
                source_fingerprint(&resolved.canonical).map_err(|_| rar_first_volume_failed())?;
            *fallback_cached = cache_db
                .and_then(|db| db.lookup(&resolved.canonical, fingerprint.mtime, fingerprint.size));
            if let Some(path) = fallback_cached.take() {
                return Ok(PreparedArchive::Target(cached_target(
                    public_source,
                    &resolved.canonical,
                    fingerprint.clone(),
                    path,
                )));
            }
        }
        Ok(PreparedArchive::Summary(summary))
    }

    #[allow(clippy::too_many_arguments)]
    fn convert_to_cache(
        &self,
        public_source: &RemoteAddress,
        resolved: &super::path_guard::ResolvedPath,
        fingerprint: SourceFingerprint,
        format: crate::archive_converter::ArchiveFormat,
        summary: crate::archive_converter::ArchiveImageSummary,
        mut password: Option<String>,
        cache_db: Arc<crate::archive_cache::ArchiveCacheDb>,
        max_cache_bytes: u64,
        control: &dyn RemoteArchiveJobControl,
        cancel: &Arc<AtomicBool>,
    ) -> RemoteArchiveExecutionOutcome {
        loop {
            control.update(RemoteArchiveJobState::WaitingForConversionSlot, None);
            let convert_guard = cache_db.begin_convert();
            if cancel.load(Ordering::Relaxed) {
                return cancelled_outcome();
            }
            let current = match source_fingerprint(&resolved.canonical) {
                Ok(current) => current,
                Err(_) => return source_changed(),
            };
            if current != fingerprint {
                return source_changed();
            }
            if let Some(path) =
                cache_db.lookup(&resolved.canonical, fingerprint.mtime, fingerprint.size)
            {
                return cached_outcome(public_source, &resolved.canonical, fingerprint, path);
            }
            let destination = match cache_db.reserve_cache_zip_path(&resolved.canonical) {
                Ok(path) => path,
                Err(_) => return cache_publish_failed(),
            };
            control.update(
                RemoteArchiveJobState::Converting,
                Some(RemoteArchiveProgress {
                    files_done: 0,
                    files_total: u64::from(summary.image_count),
                    bytes_written: 0,
                }),
            );
            let progress = |value: crate::archive_converter::ConvertProgress| {
                control.update(
                    RemoteArchiveJobState::Converting,
                    Some(RemoteArchiveProgress {
                        files_done: u64::from(value.files_done),
                        files_total: u64::from(value.files_total),
                        bytes_written: value.bytes_written,
                    }),
                );
            };
            let converted = crate::archive_converter::convert_to_zip_with_password(
                &resolved.canonical,
                &destination,
                format,
                password.as_deref(),
                cancel,
                Some(&progress),
                crate::archive_converter::ConvertOptions::default(),
            );
            let converted = match converted {
                Ok(summary) => summary,
                Err(crate::archive_converter::ConvertError::Cancelled) => {
                    return cancelled_outcome();
                }
                Err(crate::archive_converter::ConvertError::PasswordRequired)
                | Err(crate::archive_converter::ConvertError::BadPassword) => {
                    drop(convert_guard);
                    password = match control
                        .await_password(RemoteArchivePasswordResume::Convert, password.is_some())
                    {
                        Ok(password) => Some(password),
                        Err(()) => return cancelled_outcome(),
                    };
                    continue;
                }
                Err(crate::archive_converter::ConvertError::PasswordUnsupported) => {
                    return password_unsupported();
                }
                Err(crate::archive_converter::ConvertError::NoImages) => {
                    return failed(RemoteArchiveTerminalCode::NoImages, "画像がありません");
                }
                Err(error) => return conversion_failed(error),
            };
            control.update(RemoteArchiveJobState::Finalizing, None);
            if source_fingerprint(&resolved.canonical).ok().as_ref() != Some(&fingerprint) {
                let _ = std::fs::remove_file(&destination);
                return source_changed();
            }
            let cached_size = match std::fs::metadata(&destination) {
                Ok(metadata) => metadata.len().min(i64::MAX as u64) as i64,
                Err(_) => {
                    let _ = std::fs::remove_file(&destination);
                    return cache_publish_failed();
                }
            };
            if cache_db
                .record(
                    &resolved.canonical,
                    fingerprint.mtime,
                    fingerprint.size,
                    format,
                    &destination,
                    cached_size,
                    converted.image_count,
                    password.is_some(),
                )
                .is_err()
            {
                let _ = std::fs::remove_file(&destination);
                return cache_publish_failed();
            }
            let prune = cache_db.prune_to_size_limit_locked(max_cache_bytes, &resolved.canonical);
            if let Err(error) = prune {
                crate::logger::log(format!("remote_archive: cache prune failed: {error}"));
            }
            drop(convert_guard);
            return cached_outcome(public_source, &resolved.canonical, fingerprint, destination);
        }
    }
}
