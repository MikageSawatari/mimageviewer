use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use mimageviewer_ipc::{
    PagePayload, RemoteAiCancelResponse, RemoteAiJobError, RemoteAiJobErrorCode,
    RemoteAiJobSnapshot, RemoteAiJobState, RemoteAiPageOutcome, RemoteAiPageOutcomeState,
    RemoteAiProgress, RemoteAiProgressPhase, RemoteAiRecoverableResponse, RemoteAiResultResponse,
    RemoteAiStartRequest, RemoteAiStartResponse, RemoteAiStateResponse, RemoteAiTerminalCode,
    RemoteAiTerminalDetail,
};

use super::session::SessionOperation;

pub(super) struct ContainerRemoteAiExecutor {
    engine: Arc<super::container::ContainerEngine>,
}

impl ContainerRemoteAiExecutor {
    pub(super) fn new(engine: Arc<super::container::ContainerEngine>) -> Self {
        Self { engine }
    }
}

impl RemoteAiExecutor for ContainerRemoteAiExecutor {
    fn execute(
        &self,
        request: &RemoteAiStartRequest,
        progress: &dyn RemoteAiProgressSink,
        cancel: &Arc<AtomicBool>,
    ) -> RemoteAiExecutionOutcome {
        self.engine.execute_remote_ai(request, progress, cancel)
    }
}

pub(crate) const TERMINAL_RETENTION: Duration = Duration::from_secs(10 * 60);
const TOMBSTONE_LIMIT: usize = 256;

pub(crate) trait RemoteAiProgressSink: Send + Sync {
    fn update(&self, state: RemoteAiJobState, progress: Option<RemoteAiProgress>);
}

pub(crate) trait RemoteAiExecutor: Send + Sync {
    fn execute(
        &self,
        request: &RemoteAiStartRequest,
        progress: &dyn RemoteAiProgressSink,
        cancel: &Arc<AtomicBool>,
    ) -> RemoteAiExecutionOutcome;
}

pub(crate) enum RemoteAiPageExecutionOutcome {
    Ready(PagePayload),
    NotApplicable {
        code: RemoteAiTerminalCode,
        message: String,
    },
}

pub(crate) enum RemoteAiExecutionOutcome {
    Completed(Vec<RemoteAiPageExecutionOutcome>),
    Superseded(String),
    Failed(String),
}

#[derive(Clone, Copy)]
pub(crate) enum RemoteAiDrainCause {
    DiscardedByHost,
    BackgroundExpired,
    Superseded,
}

/// A remote job's drain participation. The underlying session operation stays live until the
/// executor has acknowledged cancellation or produced a terminal result.
struct RemoteAiJobLease {
    operation: Option<SessionOperation>,
}

impl RemoteAiJobLease {
    fn new(operation: SessionOperation) -> Self {
        Self {
            operation: Some(operation),
        }
    }

    fn wait_until_active(&self) -> Result<(), mimageviewer_ipc::SessionResponse> {
        self.operation
            .as_ref()
            .expect("job lease is live")
            .wait_until_active()
    }

    fn started(&self) {
        self.operation
            .as_ref()
            .expect("job lease is live")
            .started();
    }

    fn finish(mut self, success: bool) {
        if let Some(operation) = self.operation.take() {
            operation.finish(success);
        }
    }
}

struct JobEntry {
    owner: String,
    request: RemoteAiStartRequest,
    snapshot: RemoteAiJobSnapshot,
    cancel: Arc<AtomicBool>,
    requested_terminal: Option<(RemoteAiJobState, RemoteAiTerminalDetail)>,
    results: Vec<Option<PagePayload>>,
    terminal_at: Option<Duration>,
}

struct Tombstone {
    owner: String,
    job_id: String,
    terminal_code: Option<RemoteAiTerminalCode>,
}

#[derive(Default)]
struct RegistryState {
    next_sequence: u64,
    jobs: HashMap<String, JobEntry>,
    order: VecDeque<String>,
    tombstones: VecDeque<Tombstone>,
}

pub(crate) struct RemoteAiJobRegistry {
    inner: Mutex<RegistryState>,
    executor: Arc<dyn RemoteAiExecutor>,
    origin: Instant,
}

impl RemoteAiJobRegistry {
    pub(crate) fn new(executor: Arc<dyn RemoteAiExecutor>) -> Arc<Self> {
        Arc::new(Self {
            inner: Mutex::new(RegistryState::default()),
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

    pub(crate) fn has_nonterminal_jobs(&self) -> bool {
        self.inner
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .jobs
            .values()
            .any(|job| !job.snapshot.state.is_terminal())
    }

    pub(crate) fn on_session_drain(&self, cause: RemoteAiDrainCause) {
        let (state, code, message) = match cause {
            RemoteAiDrainCause::DiscardedByHost => (
                RemoteAiJobState::DiscardedByHost,
                RemoteAiTerminalCode::DiscardedByHost,
                "PC 側で接続が終了されたため AI 処理を中止しました",
            ),
            RemoteAiDrainCause::BackgroundExpired => (
                RemoteAiJobState::BackgroundExpired,
                RemoteAiTerminalCode::BackgroundExpired,
                "バックグラウンド保持時間を超えたため AI 処理を中止しました",
            ),
            RemoteAiDrainCause::Superseded => (
                RemoteAiJobState::Superseded,
                RemoteAiTerminalCode::Superseded,
                "別の端末へ操作権が移ったため AI 処理を中止しました",
            ),
        };
        self.request_cancel_all(state, code, message);
    }

    pub(crate) fn start(
        self: &Arc<Self>,
        owner: String,
        request: RemoteAiStartRequest,
        accept_before_unix_ms: u64,
        operation: SessionOperation,
    ) -> RemoteAiStartResponse {
        if let Err(error) = validate_start_request(&request) {
            return RemoteAiStartResponse::Error(error);
        }
        if Self::unix_ms() > accept_before_unix_ms {
            return RemoteAiStartResponse::Error(RemoteAiJobError::new(
                RemoteAiJobErrorCode::StartExpired,
                "remote AI start was not admitted within two seconds",
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
            return RemoteAiStartResponse::Accepted(existing.snapshot.clone());
        }
        for job in state
            .jobs
            .values_mut()
            .filter(|job| job.owner == owner && !job.snapshot.state.is_terminal())
        {
            request_cancel(
                job,
                RemoteAiJobState::Superseded,
                RemoteAiTerminalCode::Superseded,
                "新しく表示したページの AI 処理に切り替えました",
            );
        }
        state.next_sequence = state.next_sequence.wrapping_add(1).max(1);
        let job_id = format!("{generation}-{}", state.next_sequence);
        let snapshot = RemoteAiJobSnapshot {
            job_id: job_id.clone(),
            request_id: request.request_id.clone(),
            state: RemoteAiJobState::WaitingForLocalDrain,
            progress: Some(waiting_progress(request.pages.len())),
            terminal: None,
            page_count: request.pages.len() as u32,
            page_outcomes: (0..request.pages.len())
                .map(|page_index| RemoteAiPageOutcome {
                    page_index: page_index as u32,
                    state: RemoteAiPageOutcomeState::Pending,
                    terminal: None,
                })
                .collect(),
            created_unix_ms: unix_ms,
            updated_unix_ms: unix_ms,
        };
        state.order.push_back(job_id.clone());
        state.jobs.insert(
            job_id.clone(),
            JobEntry {
                owner,
                request: request.clone(),
                snapshot: snapshot.clone(),
                cancel,
                requested_terminal: None,
                results: Vec::new(),
                terminal_at: None,
            },
        );
        drop(state);

        self.spawn_job(job_id, request, RemoteAiJobLease::new(operation));
        RemoteAiStartResponse::Accepted(snapshot)
    }

    fn spawn_job(
        self: &Arc<Self>,
        job_id: String,
        request: RemoteAiStartRequest,
        lease: RemoteAiJobLease,
    ) {
        let registry = Arc::downgrade(self);
        let executor = Arc::clone(&self.executor);
        let thread_job_id = job_id.clone();
        let spawn = std::thread::Builder::new()
            .name(format!("remote-ai-{}", job_id))
            .spawn(move || {
                if lease.wait_until_active().is_err() {
                    if let Some(registry) = registry.upgrade() {
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
                let reporter = RegistryProgressSink {
                    registry: Arc::downgrade(&registry),
                    job_id: thread_job_id.clone(),
                };
                reporter.update(
                    RemoteAiJobState::PreparingSource,
                    Some(progress_for(
                        RemoteAiProgressPhase::PreparingSource,
                        0,
                        request.pages.len(),
                        0,
                        1,
                        None,
                    )),
                );
                let cancel = registry.cancel_for(&thread_job_id);
                let outcome = match cancel {
                    Some(cancel) => executor.execute(&request, &reporter, &cancel),
                    None => {
                        RemoteAiExecutionOutcome::Failed("AI 処理を開始できませんでした".to_owned())
                    }
                };
                let success = matches!(outcome, RemoteAiExecutionOutcome::Completed(_));
                registry.complete(&thread_job_id, outcome);
                lease.finish(success);
            });
        if let Err(error) = spawn {
            self.complete(
                &job_id,
                RemoteAiExecutionOutcome::Failed({
                    crate::logger::log(format!(
                        "remote_ipc: remote AI thread start failed: {error}"
                    ));
                    "AI 処理を開始できませんでした".to_owned()
                }),
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
            set_terminal(job, terminal_state, terminal, self.now());
        } else {
            set_terminal(
                job,
                RemoteAiJobState::Failed,
                terminal_detail(
                    RemoteAiTerminalCode::ExecutionFailed,
                    "AI 処理を開始できませんでした",
                    None,
                ),
                self.now(),
            );
        }
    }

    fn complete(&self, job_id: &str, outcome: RemoteAiExecutionOutcome) {
        let mut state = self.inner.lock().unwrap_or_else(|error| error.into_inner());
        let Some(job) = state.jobs.get_mut(job_id) else {
            return;
        };
        let now = self.now();
        if let Some((terminal_state, terminal)) = job.requested_terminal.take() {
            set_terminal(job, terminal_state, terminal, now);
            return;
        }
        match outcome {
            RemoteAiExecutionOutcome::Completed(outcomes) => {
                if outcomes.len() != job.request.pages.len() {
                    set_terminal(
                        job,
                        RemoteAiJobState::Failed,
                        terminal_detail(
                            RemoteAiTerminalCode::ExecutionFailed,
                            "AI 処理を完了できませんでした",
                            None,
                        ),
                        now,
                    );
                } else {
                    job.results.clear();
                    job.snapshot.page_outcomes.clear();
                    for (page_index, outcome) in outcomes.into_iter().enumerate() {
                        match outcome {
                            RemoteAiPageExecutionOutcome::Ready(result) => {
                                job.results.push(Some(result));
                                job.snapshot.page_outcomes.push(RemoteAiPageOutcome {
                                    page_index: page_index as u32,
                                    state: RemoteAiPageOutcomeState::Ready,
                                    terminal: None,
                                });
                            }
                            RemoteAiPageExecutionOutcome::NotApplicable { code, message } => {
                                job.results.push(None);
                                job.snapshot.page_outcomes.push(RemoteAiPageOutcome {
                                    page_index: page_index as u32,
                                    state: RemoteAiPageOutcomeState::NotApplicable,
                                    terminal: Some(terminal_detail(
                                        code,
                                        message,
                                        Some(page_index),
                                    )),
                                });
                            }
                        }
                    }
                    set_terminal_ready(job, now);
                }
            }
            RemoteAiExecutionOutcome::Superseded(message) => set_terminal(
                job,
                RemoteAiJobState::Superseded,
                terminal_detail(RemoteAiTerminalCode::SourceChanged, message, None),
                now,
            ),
            RemoteAiExecutionOutcome::Failed(message) => set_terminal(
                job,
                RemoteAiJobState::Failed,
                terminal_detail(RemoteAiTerminalCode::ExecutionFailed, message, None),
                now,
            ),
        }
    }

    pub(crate) fn state(&self, owner: &str, job_id: &str) -> RemoteAiStateResponse {
        let mut state = self.inner.lock().unwrap_or_else(|error| error.into_inner());
        self.prune_locked(&mut state, self.now());
        match lookup_job(&state, owner, job_id) {
            Ok(job) => RemoteAiStateResponse::Success(job.snapshot.clone()),
            Err(error) => RemoteAiStateResponse::Error(error),
        }
    }

    pub(crate) fn recoverable(&self, owner: &str) -> RemoteAiRecoverableResponse {
        let mut state = self.inner.lock().unwrap_or_else(|error| error.into_inner());
        self.prune_locked(&mut state, self.now());
        let snapshots = state
            .order
            .iter()
            .filter_map(|job_id| state.jobs.get(job_id))
            .filter(|job| job.owner == owner)
            .map(|job| job.snapshot.clone())
            .collect();
        RemoteAiRecoverableResponse::Success(snapshots)
    }

    pub(crate) fn cancel(&self, owner: &str, job_id: &str) -> RemoteAiCancelResponse {
        let mut state = self.inner.lock().unwrap_or_else(|error| error.into_inner());
        self.prune_locked(&mut state, self.now());
        let Some(job) = state.jobs.get_mut(job_id) else {
            return RemoteAiCancelResponse::Error(missing_job_error(&state, owner, job_id));
        };
        if job.owner != owner {
            return RemoteAiCancelResponse::Error(forbidden_error());
        }
        if !job.snapshot.state.is_terminal() {
            request_cancel(
                job,
                RemoteAiJobState::CancelledByUser,
                RemoteAiTerminalCode::CancelledByUser,
                "利用者が AI 処理を取り消しました",
            );
        }
        RemoteAiCancelResponse::Success(job.snapshot.clone())
    }

    pub(crate) fn result(
        &self,
        owner: &str,
        job_id: &str,
        page_index: usize,
    ) -> RemoteAiResultResponse {
        let mut state = self.inner.lock().unwrap_or_else(|error| error.into_inner());
        self.prune_locked(&mut state, self.now());
        let job = match lookup_job(&state, owner, job_id) {
            Ok(job) => job,
            Err(error) => return RemoteAiResultResponse::Error(error),
        };
        if page_index >= job.request.pages.len() {
            return RemoteAiResultResponse::Error(RemoteAiJobError::new(
                RemoteAiJobErrorCode::PageOutOfRange,
                "page index is outside this job's page group",
            ));
        }
        if job.snapshot.state != RemoteAiJobState::Ready {
            return RemoteAiResultResponse::Error(RemoteAiJobError::new(
                RemoteAiJobErrorCode::NotReady,
                "AI result is not ready",
            ));
        }
        match job.results.get(page_index).and_then(Clone::clone) {
            Some(result) => RemoteAiResultResponse::Success(result),
            None => {
                let terminal_code = job
                    .snapshot
                    .page_outcomes
                    .get(page_index)
                    .and_then(|outcome| outcome.terminal.as_ref())
                    .map(|terminal| terminal.code);
                let mut error = RemoteAiJobError::new(
                    RemoteAiJobErrorCode::PageNotApplicable,
                    "このページは AI 処理の対象外です",
                );
                error.terminal_code = terminal_code;
                RemoteAiResultResponse::Error(error)
            }
        }
    }

    fn request_cancel_all(
        &self,
        terminal_state: RemoteAiJobState,
        code: RemoteAiTerminalCode,
        message: &str,
    ) {
        let mut state = self.inner.lock().unwrap_or_else(|error| error.into_inner());
        for job in state
            .jobs
            .values_mut()
            .filter(|job| !job.snapshot.state.is_terminal())
        {
            request_cancel(job, terminal_state, code, message);
        }
    }

    fn prune_locked(&self, state: &mut RegistryState, now: Duration) {
        let expired = state
            .order
            .iter()
            .filter_map(|job_id| {
                state.jobs.get(job_id).and_then(|job| {
                    job.terminal_at
                        .filter(|terminal_at| {
                            now.saturating_sub(*terminal_at) >= TERMINAL_RETENTION
                        })
                        .map(|_| job_id.clone())
                })
            })
            .collect::<Vec<_>>();
        for job_id in expired {
            if let Some(job) = state.jobs.remove(&job_id) {
                state.tombstones.push_back(Tombstone {
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

    #[cfg(test)]
    fn expire_terminals_for_test(&self) {
        let mut state = self.inner.lock().unwrap_or_else(|error| error.into_inner());
        for job in state
            .jobs
            .values_mut()
            .filter(|job| job.terminal_at.is_some())
        {
            job.terminal_at = Some(Duration::ZERO);
        }
        self.prune_locked(&mut state, TERMINAL_RETENTION);
    }
}

struct RegistryProgressSink {
    registry: Weak<RemoteAiJobRegistry>,
    job_id: String,
}

impl RemoteAiProgressSink for RegistryProgressSink {
    fn update(&self, state: RemoteAiJobState, progress: Option<RemoteAiProgress>) {
        let Some(registry) = self.registry.upgrade() else {
            return;
        };
        let mut registry_state = registry
            .inner
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let Some(job) = registry_state.jobs.get_mut(&self.job_id) else {
            return;
        };
        if job.requested_terminal.is_some() || job.snapshot.state.is_terminal() {
            return;
        }
        job.snapshot.state = state;
        job.snapshot.progress = progress;
        job.snapshot.updated_unix_ms = RemoteAiJobRegistry::unix_ms();
    }
}

fn validate_start_request(request: &RemoteAiStartRequest) -> Result<(), RemoteAiJobError> {
    if request.request_id.is_empty() || request.request_id.len() > 128 {
        return Err(RemoteAiJobError::new(
            RemoteAiJobErrorCode::BadRequest,
            "request_id must contain 1 to 128 bytes",
        ));
    }
    if !(1..=2).contains(&request.pages.len()) {
        return Err(RemoteAiJobError::new(
            RemoteAiJobErrorCode::BadRequest,
            "an AI page group must contain one or two pages",
        ));
    }
    for page in &request.pages {
        page.address.validate_syntax().map_err(|_| {
            RemoteAiJobError::new(RemoteAiJobErrorCode::BadRequest, "invalid page address")
        })?;
        if page.target_px == 0 || page.target_px > crate::pdf_loader::PDF_RENDER_MAX_LONG_PX {
            return Err(RemoteAiJobError::new(
                RemoteAiJobErrorCode::BadRequest,
                "target_px is outside the supported range",
            ));
        }
    }
    Ok(())
}

fn waiting_progress(page_count: usize) -> RemoteAiProgress {
    progress_for(
        RemoteAiProgressPhase::WaitingForLocalDrain,
        0,
        page_count,
        0,
        1,
        None,
    )
}

pub(crate) fn progress_for(
    phase: RemoteAiProgressPhase,
    page_index: usize,
    page_count: usize,
    stage_index: usize,
    stage_count: usize,
    tiles: Option<(usize, usize)>,
) -> RemoteAiProgress {
    RemoteAiProgress {
        phase,
        page_index: page_index as u32,
        page_count: page_count as u32,
        stage_index: stage_index as u32,
        stage_count: stage_count as u32,
        completed_tiles: tiles.map(|tiles| tiles.0 as u32),
        total_tiles: tiles.map(|tiles| tiles.1 as u32),
    }
}

fn terminal_detail(
    code: RemoteAiTerminalCode,
    message: impl Into<String>,
    page_index: Option<usize>,
) -> RemoteAiTerminalDetail {
    RemoteAiTerminalDetail {
        code,
        message: message.into(),
        page_index: page_index.map(|index| index as u32),
    }
}

fn set_terminal_ready(job: &mut JobEntry, now: Duration) {
    job.snapshot.state = RemoteAiJobState::Ready;
    job.snapshot.progress = None;
    job.snapshot.terminal = None;
    job.snapshot.updated_unix_ms = RemoteAiJobRegistry::unix_ms();
    job.terminal_at = Some(now);
}

fn set_terminal(
    job: &mut JobEntry,
    state: RemoteAiJobState,
    terminal: RemoteAiTerminalDetail,
    now: Duration,
) {
    job.snapshot.state = state;
    job.snapshot.progress = None;
    job.snapshot.terminal = Some(terminal);
    job.snapshot.updated_unix_ms = RemoteAiJobRegistry::unix_ms();
    job.terminal_at = Some(now);
    job.results.clear();
}

fn request_cancel(
    job: &mut JobEntry,
    state: RemoteAiJobState,
    code: RemoteAiTerminalCode,
    message: impl Into<String>,
) {
    if job.snapshot.state.is_terminal() || job.requested_terminal.is_some() {
        return;
    }
    job.requested_terminal = Some((state, terminal_detail(code, message, None)));
    job.snapshot.state = RemoteAiJobState::Cancelling;
    job.snapshot.progress = Some(progress_for(
        RemoteAiProgressPhase::Cancelling,
        0,
        job.request.pages.len(),
        0,
        1,
        None,
    ));
    job.snapshot.updated_unix_ms = RemoteAiJobRegistry::unix_ms();
    job.cancel.store(true, Ordering::Release);
}

fn lookup_job<'a>(
    state: &'a RegistryState,
    owner: &str,
    job_id: &str,
) -> Result<&'a JobEntry, RemoteAiJobError> {
    if let Some(job) = state.jobs.get(job_id) {
        return if job.owner == owner {
            Ok(job)
        } else {
            Err(forbidden_error())
        };
    }
    Err(missing_job_error(state, owner, job_id))
}

fn missing_job_error(state: &RegistryState, owner: &str, job_id: &str) -> RemoteAiJobError {
    if let Some(tombstone) = state
        .tombstones
        .iter()
        .find(|entry| entry.owner == owner && entry.job_id == job_id)
    {
        let mut error = RemoteAiJobError::new(
            RemoteAiJobErrorCode::JobGone,
            "AI job terminal retention has expired",
        );
        error.terminal_code = tombstone.terminal_code;
        return error;
    }
    RemoteAiJobError::new(RemoteAiJobErrorCode::NotFound, "AI job was not found")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    struct FakeCall {
        request_id: String,
        cancel: Arc<AtomicBool>,
        complete: mpsc::Sender<FakeCompletion>,
    }

    enum FakeCompletion {
        Ready,
        Failed(&'static str),
        NotApplicable(RemoteAiTerminalCode),
        Mixed(RemoteAiTerminalCode),
    }

    struct ControlledExecutor {
        calls: mpsc::Sender<FakeCall>,
    }

    impl RemoteAiExecutor for ControlledExecutor {
        fn execute(
            &self,
            request: &RemoteAiStartRequest,
            _progress: &dyn RemoteAiProgressSink,
            cancel: &Arc<AtomicBool>,
        ) -> RemoteAiExecutionOutcome {
            let (complete, wait) = mpsc::channel();
            self.calls
                .send(FakeCall {
                    request_id: request.request_id.clone(),
                    cancel: Arc::clone(cancel),
                    complete,
                })
                .expect("test call receiver");
            match wait.recv().expect("test completion") {
                FakeCompletion::Ready => RemoteAiExecutionOutcome::Completed(
                    request
                        .pages
                        .iter()
                        .map(|_| RemoteAiPageExecutionOutcome::Ready(fake_page_payload()))
                        .collect(),
                ),
                FakeCompletion::Failed(message) => {
                    RemoteAiExecutionOutcome::Failed(message.to_owned())
                }
                FakeCompletion::NotApplicable(code) => RemoteAiExecutionOutcome::Completed(
                    request
                        .pages
                        .iter()
                        .map(|_| RemoteAiPageExecutionOutcome::NotApplicable {
                            code,
                            message: "not applicable in registry test".to_owned(),
                        })
                        .collect(),
                ),
                FakeCompletion::Mixed(code) => RemoteAiExecutionOutcome::Completed(
                    request
                        .pages
                        .iter()
                        .enumerate()
                        .map(|(index, _)| {
                            if index == 0 {
                                RemoteAiPageExecutionOutcome::Ready(fake_page_payload())
                            } else {
                                RemoteAiPageExecutionOutcome::NotApplicable {
                                    code,
                                    message: "not applicable in registry test".to_owned(),
                                }
                            }
                        })
                        .collect(),
                ),
            }
        }
    }

    fn fake_page_payload() -> PagePayload {
        PagePayload {
            bytes: vec![1, 2, 3],
            content_type: "image/jpeg".to_owned(),
            width: 1,
            height: 1,
            identity: mimageviewer_ipc::RemoteAddress::file(
                "30d6c167-7148-4f3e-9a5a-21c5fd31ecb2",
                "image.png",
            ),
        }
    }

    fn peer() -> mimageviewer_ipc::SessionPeerInfo {
        mimageviewer_ipc::SessionPeerInfo {
            connection_kind: mimageviewer_ipc::SessionConnectionKind::Direct,
            device_name: Some("test phone".to_owned()),
        }
    }

    fn request(request_id: &str) -> RemoteAiStartRequest {
        RemoteAiStartRequest {
            request_id: request_id.to_owned(),
            pages: vec![mimageviewer_ipc::RemoteAiPageRequest {
                address: mimageviewer_ipc::RemoteAddress::file(
                    "30d6c167-7148-4f3e-9a5a-21c5fd31ecb2",
                    "image.png",
                ),
                target_px: 1024,
                render_context: None,
            }],
        }
    }

    fn fixture() -> (
        super::super::session::SessionHandle,
        Arc<RemoteAiJobRegistry>,
        mpsc::Receiver<FakeCall>,
    ) {
        let (calls, call_rx) = mpsc::channel();
        let registry = RemoteAiJobRegistry::new(Arc::new(ControlledExecutor { calls }));
        let session = super::super::session::SessionHandle::new();
        session.install_ai_jobs(&registry);
        assert_eq!(
            session
                .acquire(mimageviewer_ipc::SessionAcquireRequest {
                    client_id: "client".to_owned(),
                    peer: peer(),
                })
                .status,
            mimageviewer_ipc::SessionStatus::Active
        );
        let generation = session.snapshot().generation;
        assert!(session.finish_acquire(generation));
        (session, registry, call_rx)
    }

    fn start(
        session: &super::super::session::SessionHandle,
        registry: &Arc<RemoteAiJobRegistry>,
        request_id: &str,
    ) -> RemoteAiJobSnapshot {
        start_with_request(session, registry, request(request_id))
    }

    fn start_with_request(
        session: &super::super::session::SessionHandle,
        registry: &Arc<RemoteAiJobRegistry>,
        request: RemoteAiStartRequest,
    ) -> RemoteAiJobSnapshot {
        let owner = session.owner_for_test("client");
        let operation = session
            .begin_operation(&owner, "remote AI test".to_owned())
            .unwrap();
        match registry.start("client".to_owned(), request, u64::MAX, operation) {
            RemoteAiStartResponse::Accepted(snapshot) => snapshot,
            RemoteAiStartResponse::Error(error) => panic!("start failed: {error:?}"),
        }
    }

    fn wait_for_state(
        registry: &RemoteAiJobRegistry,
        job_id: &str,
        expected: RemoteAiJobState,
    ) -> RemoteAiJobSnapshot {
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            let snapshot = match registry.state("client", job_id) {
                RemoteAiStateResponse::Success(snapshot) => snapshot,
                RemoteAiStateResponse::Error(error) => panic!("state failed: {error:?}"),
            };
            if snapshot.state == expected {
                return snapshot;
            }
            assert!(
                Instant::now() < deadline,
                "last state: {:?}",
                snapshot.state
            );
            std::thread::yield_now();
        }
    }

    #[test]
    fn ready_result_is_retained_then_becomes_typed_gone_tombstone() {
        let (session, registry, calls) = fixture();
        let job = start(&session, &registry, "ready");
        let call = calls.recv_timeout(Duration::from_secs(1)).unwrap();
        assert_eq!(call.request_id, "ready");
        call.complete.send(FakeCompletion::Ready).unwrap();
        wait_for_state(&registry, &job.job_id, RemoteAiJobState::Ready);
        assert!(matches!(
            registry.result("client", &job.job_id, 0),
            RemoteAiResultResponse::Success(PagePayload { bytes, .. }) if bytes == vec![1, 2, 3]
        ));

        registry.expire_terminals_for_test();
        assert!(matches!(
            registry.state("client", &job.job_id),
            RemoteAiStateResponse::Error(RemoteAiJobError {
                code: RemoteAiJobErrorCode::JobGone,
                ..
            })
        ));
    }

    #[test]
    fn disconnect_drain_waits_for_executor_acknowledgement() {
        let (session, registry, calls) = fixture();
        let job = start(&session, &registry, "disconnect");
        let call = calls.recv_timeout(Duration::from_secs(1)).unwrap();
        let generation = session.snapshot().generation;

        session.local_disconnect();
        assert!(call.cancel.load(Ordering::Acquire));
        assert_eq!(
            wait_for_state(&registry, &job.job_id, RemoteAiJobState::Cancelling).state,
            RemoteAiJobState::Cancelling
        );
        assert!(!session.complete_app_drain(generation));
        assert_eq!(
            session.snapshot().phase,
            super::super::session::RemoteControlPhase::DrainingRemote
        );

        call.complete.send(FakeCompletion::Ready).unwrap();
        let terminal = wait_for_state(&registry, &job.job_id, RemoteAiJobState::DiscardedByHost);
        assert_eq!(
            terminal.terminal.unwrap().code,
            RemoteAiTerminalCode::DiscardedByHost
        );
        let deadline = Instant::now() + Duration::from_secs(2);
        while session.snapshot().phase != super::super::session::RemoteControlPhase::Local {
            assert!(Instant::now() < deadline);
            std::thread::yield_now();
        }
    }

    #[test]
    fn explicit_cancel_and_supersede_are_distinct_terminal_results() {
        let (session, registry, calls) = fixture();
        let cancelled = start(&session, &registry, "cancel");
        let cancelled_call = calls.recv_timeout(Duration::from_secs(1)).unwrap();
        assert!(matches!(
            registry.cancel("client", &cancelled.job_id),
            RemoteAiCancelResponse::Success(RemoteAiJobSnapshot {
                state: RemoteAiJobState::Cancelling,
                ..
            })
        ));
        assert!(cancelled_call.cancel.load(Ordering::Acquire));
        cancelled_call.complete.send(FakeCompletion::Ready).unwrap();
        wait_for_state(
            &registry,
            &cancelled.job_id,
            RemoteAiJobState::CancelledByUser,
        );

        let old = start(&session, &registry, "old");
        let old_call = calls.recv_timeout(Duration::from_secs(1)).unwrap();
        let new = start(&session, &registry, "new");
        let new_call = calls.recv_timeout(Duration::from_secs(1)).unwrap();
        assert!(old_call.cancel.load(Ordering::Acquire));
        old_call.complete.send(FakeCompletion::Ready).unwrap();
        new_call.complete.send(FakeCompletion::Ready).unwrap();
        wait_for_state(&registry, &old.job_id, RemoteAiJobState::Superseded);
        wait_for_state(&registry, &new.job_id, RemoteAiJobState::Ready);
    }

    #[test]
    fn recoverable_query_keeps_the_same_nonterminal_job_identity() {
        let (session, registry, calls) = fixture();
        let job = start(&session, &registry, "recover");
        let call = calls.recv_timeout(Duration::from_secs(1)).unwrap();
        let recovered = match registry.recoverable("client") {
            RemoteAiRecoverableResponse::Success(jobs) => jobs,
            RemoteAiRecoverableResponse::Error(error) => panic!("recover failed: {error:?}"),
        };
        assert_eq!(recovered.len(), 1);
        assert_eq!(recovered[0].job_id, job.job_id);
        call.complete
            .send(FakeCompletion::Failed("expected failure"))
            .unwrap();
        wait_for_state(&registry, &job.job_id, RemoteAiJobState::Failed);
    }

    #[test]
    fn all_not_applicable_pages_complete_as_ready_with_typed_page_outcomes() {
        let (session, registry, calls) = fixture();
        for (index, code) in [
            RemoteAiTerminalCode::VectorPdf,
            RemoteAiTerminalCode::SizeGate,
            RemoteAiTerminalCode::AnimatedGif,
            RemoteAiTerminalCode::AnimatedApng,
            RemoteAiTerminalCode::AnimatedWebp,
        ]
        .into_iter()
        .enumerate()
        {
            let job = start(&session, &registry, &format!("not-applicable-{index}"));
            calls
                .recv_timeout(Duration::from_secs(1))
                .unwrap()
                .complete
                .send(FakeCompletion::NotApplicable(code))
                .unwrap();
            let completed = wait_for_state(&registry, &job.job_id, RemoteAiJobState::Ready);
            assert!(completed.terminal.is_none());
            assert_eq!(completed.page_outcomes.len(), 1);
            assert_eq!(
                completed.page_outcomes[0].state,
                RemoteAiPageOutcomeState::NotApplicable
            );
            assert_eq!(
                completed.page_outcomes[0]
                    .terminal
                    .as_ref()
                    .map(|terminal| terminal.code),
                Some(code)
            );
            assert!(matches!(
                registry.result("client", &job.job_id, 0),
                RemoteAiResultResponse::Error(RemoteAiJobError {
                    code: RemoteAiJobErrorCode::PageNotApplicable,
                    terminal_code: Some(result_code),
                    ..
                }) if result_code == code
            ));
        }
    }

    #[test]
    fn mixed_ready_and_not_applicable_pages_publish_only_the_ready_result() {
        let (session, registry, calls) = fixture();
        let mut mixed_request = request("mixed-page-outcomes");
        mixed_request
            .pages
            .push(mimageviewer_ipc::RemoteAiPageRequest {
                address: mimageviewer_ipc::RemoteAddress::file(
                    "30d6c167-7148-4f3e-9a5a-21c5fd31ecb2",
                    "vector.pdf",
                ),
                target_px: 1024,
                render_context: None,
            });
        let job = start_with_request(&session, &registry, mixed_request);
        calls
            .recv_timeout(Duration::from_secs(1))
            .unwrap()
            .complete
            .send(FakeCompletion::Mixed(RemoteAiTerminalCode::VectorPdf))
            .unwrap();

        let completed = wait_for_state(&registry, &job.job_id, RemoteAiJobState::Ready);
        assert!(completed.terminal.is_none());
        assert_eq!(completed.page_outcomes.len(), 2);
        assert_eq!(
            completed.page_outcomes[0].state,
            RemoteAiPageOutcomeState::Ready
        );
        assert_eq!(
            completed.page_outcomes[1].state,
            RemoteAiPageOutcomeState::NotApplicable
        );
        assert!(matches!(
            registry.result("client", &job.job_id, 0),
            RemoteAiResultResponse::Success(PagePayload { bytes, .. }) if bytes == vec![1, 2, 3]
        ));
        assert!(matches!(
            registry.result("client", &job.job_id, 1),
            RemoteAiResultResponse::Error(RemoteAiJobError {
                code: RemoteAiJobErrorCode::PageNotApplicable,
                terminal_code: Some(RemoteAiTerminalCode::VectorPdf),
                ..
            })
        ));
    }

    #[test]
    fn waiting_for_local_drain_dispatches_only_after_acquire_finishes() {
        let (calls, call_rx) = mpsc::channel();
        let registry = RemoteAiJobRegistry::new(Arc::new(ControlledExecutor { calls }));
        let session = super::super::session::SessionHandle::new();
        session.install_ai_jobs(&registry);
        session.acquire(mimageviewer_ipc::SessionAcquireRequest {
            client_id: "client".to_owned(),
            peer: peer(),
        });
        let job = start(&session, &registry, "waiting");
        assert_eq!(job.state, RemoteAiJobState::WaitingForLocalDrain);
        assert!(matches!(
            call_rx.recv_timeout(Duration::from_millis(50)),
            Err(mpsc::RecvTimeoutError::Timeout)
        ));

        assert!(session.finish_acquire(session.snapshot().generation));
        let call = call_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        call.complete.send(FakeCompletion::Ready).unwrap();
        wait_for_state(&registry, &job.job_id, RemoteAiJobState::Ready);
    }

    #[test]
    fn expired_start_is_rejected_before_registry_insert_or_executor_dispatch() {
        let (session, registry, calls) = fixture();
        let owner = session.owner_for_test("client");
        let operation = session
            .begin_operation(&owner, "expired remote AI test".to_owned())
            .unwrap();
        assert!(matches!(
            registry.start("client".to_owned(), request("expired"), 0, operation),
            RemoteAiStartResponse::Error(RemoteAiJobError {
                code: RemoteAiJobErrorCode::StartExpired,
                ..
            })
        ));
        assert!(matches!(calls.try_recv(), Err(mpsc::TryRecvError::Empty)));
        assert!(matches!(
            registry.recoverable("client"),
            RemoteAiRecoverableResponse::Success(jobs) if jobs.is_empty()
        ));
    }

    #[test]
    fn background_expiry_keeps_typed_terminal_until_executor_ack() {
        let (session, registry, calls) = fixture();
        let job = start(&session, &registry, "background-expiry");
        let call = calls.recv_timeout(Duration::from_secs(1)).unwrap();
        registry.on_session_drain(RemoteAiDrainCause::BackgroundExpired);
        assert!(call.cancel.load(Ordering::Acquire));
        wait_for_state(&registry, &job.job_id, RemoteAiJobState::Cancelling);

        call.complete.send(FakeCompletion::Ready).unwrap();
        let terminal = wait_for_state(&registry, &job.job_id, RemoteAiJobState::BackgroundExpired);
        assert_eq!(
            terminal.terminal.unwrap().code,
            RemoteAiTerminalCode::BackgroundExpired
        );
    }

    #[test]
    fn terminal_retention_expires_at_ten_minute_boundary() {
        let (session, registry, calls) = fixture();
        let job = start(&session, &registry, "retention-boundary");
        let call = calls.recv_timeout(Duration::from_secs(1)).unwrap();
        call.complete.send(FakeCompletion::Ready).unwrap();
        wait_for_state(&registry, &job.job_id, RemoteAiJobState::Ready);

        let mut state = registry
            .inner
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let terminal_at = state.jobs[&job.job_id].terminal_at.unwrap();
        registry.prune_locked(
            &mut state,
            terminal_at + TERMINAL_RETENTION - Duration::from_millis(1),
        );
        assert!(state.jobs.contains_key(&job.job_id));
        registry.prune_locked(&mut state, terminal_at + TERMINAL_RETENTION);
        assert!(!state.jobs.contains_key(&job.job_id));
        assert!(
            state
                .tombstones
                .iter()
                .any(|tombstone| tombstone.job_id == job.job_id)
        );
    }

    #[test]
    fn repeated_result_reads_return_stored_bytes_without_reexecuting() {
        let (session, registry, calls) = fixture();
        let job = start(&session, &registry, "stored-result");
        let call = calls.recv_timeout(Duration::from_secs(1)).unwrap();
        call.complete.send(FakeCompletion::Ready).unwrap();
        wait_for_state(&registry, &job.job_id, RemoteAiJobState::Ready);

        for _ in 0..2 {
            assert!(matches!(
                registry.result("client", &job.job_id, 0),
                RemoteAiResultResponse::Success(PagePayload { bytes, .. })
                    if bytes == vec![1, 2, 3]
            ));
        }
        assert!(matches!(calls.try_recv(), Err(mpsc::TryRecvError::Empty)));
    }
}

fn forbidden_error() -> RemoteAiJobError {
    RemoteAiJobError::new(
        RemoteAiJobErrorCode::Forbidden,
        "AI job belongs to a different remote client",
    )
}
