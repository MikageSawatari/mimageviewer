//! Process-wide TensorRT infer-worker lifecycle.
//!
//! The DirectML runtime, TensorRT worker process, retry policy, and UI notice have different
//! lifetimes.  This owner is the single source of truth for the worker side: every AI producer
//! asks it for a request route, while backend/pack/manual operations use explicit rearm methods.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex, OnceLock, Weak, mpsc};
use std::time::{Duration, Instant};

use ndarray::Array4;

use super::tensorrt_pack::PackStatus;
use super::trt_worker_pool::{TrtWorkerPool, WorkerStartError, WorkerStartFailureKind};
use super::{AiBackend, AiError, ModelKind};

const MAX_START_RETRIES: u32 = 1;
const MAX_INFER_DEATH_RECOVERIES: u32 = 3;
const START_RETRY_BACKOFF: Duration = Duration::from_secs(2);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TrtLifecycleRevision(u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TrtWorkerPoolId(u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TrtRequestedBackend {
    Local,
    TensorRt,
}

impl TrtRequestedBackend {
    fn from_backend(backend: AiBackend) -> Self {
        if backend == AiBackend::TensorRt {
            Self::TensorRt
        } else {
            Self::Local
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TrtDisabledReason {
    BackendNotRequested,
    PackUnavailable(PackStatus),
    PackUninstalling,
    AppRetired,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TrtLifecycleInternalFailure {
    TaskPanicked,
    ReaperUnavailable(String),
}

#[derive(Debug, Clone)]
pub enum TrtLifecycleFailure {
    Start(WorkerStartError),
    DiedDuringInfer {
        pool_id: TrtWorkerPoolId,
        detail: String,
    },
    Internal {
        kind: TrtLifecycleInternalFailure,
        detail: String,
    },
}

impl TrtLifecycleFailure {
    fn detail(&self) -> &str {
        match self {
            Self::Start(error) => &error.detail,
            Self::DiedDuringInfer { detail, .. } | Self::Internal { detail, .. } => detail,
        }
    }
}

#[derive(Debug, Clone)]
pub struct WorkerNotice {
    pub revision: TrtLifecycleRevision,
    pub kind: WorkerNoticeKind,
    pub detail: String,
}

#[cfg(test)]
impl WorkerNotice {
    pub(crate) fn for_test(kind: WorkerNoticeKind) -> Self {
        Self {
            revision: TrtLifecycleRevision(1),
            kind,
            detail: "test TensorRT notice".to_owned(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkerNoticeKind {
    SpawnFailed(WorkerStartFailureKind),
    DiedDuringInfer,
    LifecycleFailed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrtWorkerPhase {
    Disabled,
    Eligible,
    Starting,
    Attached,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TrtWorkerSnapshot {
    pub revision: TrtLifecycleRevision,
    pub phase: TrtWorkerPhase,
    pack_uninstalling: bool,
}

impl TrtWorkerSnapshot {
    pub fn is_starting(self) -> bool {
        self.phase == TrtWorkerPhase::Starting
    }

    pub fn is_attached(self) -> bool {
        self.phase == TrtWorkerPhase::Attached
    }

    pub fn is_pack_uninstalling(self) -> bool {
        self.phase == TrtWorkerPhase::Disabled && self.pack_uninstalling
    }

    #[cfg(test)]
    pub(crate) fn attached_for_projection_test() -> Self {
        Self {
            revision: TrtLifecycleRevision(1),
            phase: TrtWorkerPhase::Attached,
            pack_uninstalling: false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TrtRouteGeneration {
    DirectMl {
        revision: TrtLifecycleRevision,
    },
    Worker {
        revision: TrtLifecycleRevision,
        pool_id: TrtWorkerPoolId,
    },
}

impl TrtRouteGeneration {
    pub fn digest_bytes(self) -> [u8; 17] {
        let (tag, revision, pool_id) = match self {
            Self::DirectMl { revision } => (0, revision.0, 0),
            Self::Worker { revision, pool_id } => (1, revision.0, pool_id.0),
        };
        let mut bytes = [0; 17];
        bytes[0] = tag;
        bytes[1..9].copy_from_slice(&revision.to_le_bytes());
        bytes[9..17].copy_from_slice(&pool_id.to_le_bytes());
        bytes
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct TrtRecoveryBudget {
    start_retries_used: u32,
    infer_death_recoveries_used: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TrtStartCause {
    BackendEnabled,
    PackInstalled,
    LazyDemand,
    ManualRestart,
    StartupTransientRetry,
    InferDeathRetry,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct TrtStartTicket {
    revision: TrtLifecycleRevision,
    attempt_id: u64,
    requested_backend: TrtRequestedBackend,
    cause: TrtStartCause,
}

trait TrtWorkerEndpoint: Send + Sync {
    fn load_model(&self, kind: ModelKind) -> Result<u64, String>;
    fn infer(&self, kind: ModelKind, input: &Array4<f32>) -> Result<(Vec<i64>, Vec<f32>), String>;
    fn is_dead(&self) -> bool;
}

impl TrtWorkerEndpoint for TrtWorkerPool {
    fn load_model(&self, kind: ModelKind) -> Result<u64, String> {
        TrtWorkerPool::load_model(self, kind)
    }

    fn infer(&self, kind: ModelKind, input: &Array4<f32>) -> Result<(Vec<i64>, Vec<f32>), String> {
        TrtWorkerPool::infer(self, kind, input)
    }

    fn is_dead(&self) -> bool {
        TrtWorkerPool::is_dead(self)
    }
}

trait TrtPoolFactory: Send + Sync {
    fn pack_status(&self) -> PackStatus;
    fn start(
        &self,
        worker_exe: Option<&Path>,
    ) -> Result<Arc<dyn TrtWorkerEndpoint>, WorkerStartError>;
}

struct RealTrtPoolFactory;

impl TrtPoolFactory for RealTrtPoolFactory {
    fn pack_status(&self) -> PackStatus {
        super::tensorrt_pack::pack_status()
    }

    fn start(
        &self,
        worker_exe: Option<&Path>,
    ) -> Result<Arc<dyn TrtWorkerEndpoint>, WorkerStartError> {
        let pool = match worker_exe {
            Some(exe) => TrtWorkerPool::start_with_exe(exe)?,
            None => TrtWorkerPool::start()?,
        };
        Ok(Arc::new(pool))
    }
}

trait TrtTaskSpawner: Send + Sync {
    fn spawn(&self, name: &str, task: Box<dyn FnOnce() + Send>) -> std::io::Result<()>;
}

struct DetachedTrtTaskSpawner;

impl TrtTaskSpawner for DetachedTrtTaskSpawner {
    fn spawn(&self, name: &str, task: Box<dyn FnOnce() + Send>) -> std::io::Result<()> {
        std::thread::Builder::new()
            .name(name.to_owned())
            .spawn(task)
            .map(|_| ())
    }
}

#[derive(Default)]
struct TrtRetireGroupState {
    active: usize,
    closed: bool,
}

struct TrtRetireGroup {
    state: Mutex<TrtRetireGroupState>,
    changed: Condvar,
}

impl TrtRetireGroup {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(TrtRetireGroupState::default()),
            changed: Condvar::new(),
        })
    }

    fn begin(self: &Arc<Self>) -> Option<TrtRetireActivity> {
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        if state.closed {
            return None;
        }
        state.active += 1;
        Some(TrtRetireActivity {
            group: Arc::clone(self),
        })
    }

    fn close(self: Arc<Self>, revision: TrtLifecycleRevision) -> TrtRetireBarrier {
        self.state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .closed = true;
        self.changed.notify_all();
        TrtRetireBarrier {
            revision,
            group: self,
        }
    }
}

struct TrtRetireActivity {
    group: Arc<TrtRetireGroup>,
}

impl Drop for TrtRetireActivity {
    fn drop(&mut self) {
        let mut state = self
            .group
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        debug_assert!(state.active > 0);
        state.active = state.active.saturating_sub(1);
        let completed = state.closed && state.active == 0;
        drop(state);
        if completed {
            self.group.changed.notify_all();
        }
    }
}

pub struct TrtRetireBarrier {
    revision: TrtLifecycleRevision,
    group: Arc<TrtRetireGroup>,
}

impl TrtRetireBarrier {
    pub fn wait(&self) {
        let mut state = self
            .group
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        while !state.closed || state.active != 0 {
            state = self
                .group
                .changed
                .wait(state)
                .unwrap_or_else(|error| error.into_inner());
        }
    }
}

pub struct TrtPackUninstallPermit {
    owner: Weak<TrtWorkerLifecycleOwner>,
    revision: TrtLifecycleRevision,
    retire_barrier: TrtRetireBarrier,
}

impl TrtPackUninstallPermit {
    pub fn wait_for_retirement(&self) {
        debug_assert_eq!(self.revision, self.retire_barrier.revision);
        self.retire_barrier.wait();
    }

    pub fn is_current(&self) -> bool {
        self.owner
            .upgrade()
            .is_some_and(|owner| owner.pack_uninstall_is_current(self.revision))
    }

    pub fn complete(self) {
        if let Some(owner) = self.owner.upgrade() {
            owner.complete_pack_uninstall(self.revision);
        }
    }
}

struct TrtRetireJob {
    pool: Arc<dyn TrtWorkerEndpoint>,
    activity: TrtRetireActivity,
}

struct TrtAttachedPool {
    endpoint: Mutex<Option<Arc<dyn TrtWorkerEndpoint>>>,
    activity: Mutex<Option<TrtRetireActivity>>,
    retire_tx: mpsc::Sender<TrtRetireJob>,
}

impl TrtAttachedPool {
    fn new(
        endpoint: Arc<dyn TrtWorkerEndpoint>,
        activity: TrtRetireActivity,
        retire_tx: mpsc::Sender<TrtRetireJob>,
    ) -> Arc<Self> {
        Arc::new(Self {
            endpoint: Mutex::new(Some(endpoint)),
            activity: Mutex::new(Some(activity)),
            retire_tx,
        })
    }

    fn with_endpoint<T>(&self, apply: impl FnOnce(&dyn TrtWorkerEndpoint) -> T) -> T {
        let endpoint = self
            .endpoint
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        apply(endpoint.as_deref().expect("attached TRT endpoint"))
    }
}

impl TrtWorkerEndpoint for TrtAttachedPool {
    fn load_model(&self, kind: ModelKind) -> Result<u64, String> {
        self.with_endpoint(|endpoint| endpoint.load_model(kind))
    }

    fn infer(&self, kind: ModelKind, input: &Array4<f32>) -> Result<(Vec<i64>, Vec<f32>), String> {
        self.with_endpoint(|endpoint| endpoint.infer(kind, input))
    }

    fn is_dead(&self) -> bool {
        self.with_endpoint(|endpoint| endpoint.is_dead())
    }
}

impl Drop for TrtAttachedPool {
    fn drop(&mut self) {
        let endpoint = self
            .endpoint
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .take();
        let activity = self
            .activity
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .take();
        if let (Some(pool), Some(activity)) = (endpoint, activity) {
            retain_or_send_retire_job(&self.retire_tx, TrtRetireJob { pool, activity });
        }
    }
}

enum TrtWorkerState {
    Disabled {
        revision: TrtLifecycleRevision,
        reason: TrtDisabledReason,
    },
    Eligible {
        revision: TrtLifecycleRevision,
        budget: TrtRecoveryBudget,
    },
    Starting {
        ticket: TrtStartTicket,
        budget: TrtRecoveryBudget,
    },
    Attached {
        revision: TrtLifecycleRevision,
        pool_id: TrtWorkerPoolId,
        pool: Arc<TrtAttachedPool>,
        budget: TrtRecoveryBudget,
    },
    Failed {
        revision: TrtLifecycleRevision,
        failure: TrtLifecycleFailure,
    },
}

impl TrtWorkerState {
    fn revision(&self) -> TrtLifecycleRevision {
        match self {
            Self::Disabled { revision, .. }
            | Self::Eligible { revision, .. }
            | Self::Attached { revision, .. }
            | Self::Failed { revision, .. } => *revision,
            Self::Starting { ticket, .. } => ticket.revision,
        }
    }

    fn phase(&self) -> TrtWorkerPhase {
        match self {
            Self::Disabled { .. } => TrtWorkerPhase::Disabled,
            Self::Eligible { .. } => TrtWorkerPhase::Eligible,
            Self::Starting { .. } => TrtWorkerPhase::Starting,
            Self::Attached { .. } => TrtWorkerPhase::Attached,
            Self::Failed { .. } => TrtWorkerPhase::Failed,
        }
    }
}

struct TrtWorkerInner {
    requested_backend: TrtRequestedBackend,
    state: TrtWorkerState,
    retire_group: Arc<TrtRetireGroup>,
    next_revision: u64,
    next_attempt_id: u64,
    next_pool_id: u64,
    notice: Option<WorkerNotice>,
}

impl TrtWorkerInner {
    fn advance_revision(&mut self) -> (TrtLifecycleRevision, Arc<TrtRetireGroup>) {
        self.next_revision = self.next_revision.wrapping_add(1).max(1);
        let revision = TrtLifecycleRevision(self.next_revision);
        let retired_group = std::mem::replace(&mut self.retire_group, TrtRetireGroup::new());
        (revision, retired_group)
    }

    fn allocate_ticket(
        &mut self,
        revision: TrtLifecycleRevision,
        cause: TrtStartCause,
    ) -> TrtStartTicket {
        self.next_attempt_id = self.next_attempt_id.wrapping_add(1).max(1);
        TrtStartTicket {
            revision,
            attempt_id: self.next_attempt_id,
            requested_backend: TrtRequestedBackend::TensorRt,
            cause,
        }
    }

    fn allocate_pool_id(&mut self) -> TrtWorkerPoolId {
        self.next_pool_id = self.next_pool_id.wrapping_add(1).max(1);
        TrtWorkerPoolId(self.next_pool_id)
    }
}

pub struct TrtWorkerLifecycleOwner {
    inner: Mutex<TrtWorkerInner>,
    changed: Condvar,
    factory: Arc<dyn TrtPoolFactory>,
    task_spawner: Arc<dyn TrtTaskSpawner>,
    retry_backoff: Duration,
    worker_exe: Option<PathBuf>,
    retire_tx: Mutex<Option<mpsc::Sender<TrtRetireJob>>>,
}

impl TrtWorkerLifecycleOwner {
    pub fn new() -> Arc<Self> {
        Self::new_with_worker_exe(None)
    }

    pub fn new_for_diagnostic_worker(worker_exe: PathBuf) -> Arc<Self> {
        Self::new_with_worker_exe(Some(worker_exe))
    }

    fn new_with_worker_exe(worker_exe: Option<PathBuf>) -> Arc<Self> {
        Arc::new(Self {
            inner: Mutex::new(TrtWorkerInner {
                requested_backend: TrtRequestedBackend::Local,
                state: TrtWorkerState::Disabled {
                    revision: TrtLifecycleRevision(0),
                    reason: TrtDisabledReason::BackendNotRequested,
                },
                retire_group: TrtRetireGroup::new(),
                next_revision: 0,
                next_attempt_id: 0,
                next_pool_id: 0,
                notice: None,
            }),
            changed: Condvar::new(),
            factory: Arc::new(RealTrtPoolFactory),
            task_spawner: Arc::new(DetachedTrtTaskSpawner),
            retry_backoff: START_RETRY_BACKOFF,
            worker_exe,
            retire_tx: Mutex::new(None),
        })
    }

    #[cfg(test)]
    fn new_for_test(
        factory: Arc<dyn TrtPoolFactory>,
        task_spawner: Arc<dyn TrtTaskSpawner>,
    ) -> Arc<Self> {
        Arc::new(Self {
            inner: Mutex::new(TrtWorkerInner {
                requested_backend: TrtRequestedBackend::Local,
                state: TrtWorkerState::Disabled {
                    revision: TrtLifecycleRevision(0),
                    reason: TrtDisabledReason::BackendNotRequested,
                },
                retire_group: TrtRetireGroup::new(),
                next_revision: 0,
                next_attempt_id: 0,
                next_pool_id: 0,
                notice: None,
            }),
            changed: Condvar::new(),
            factory,
            task_spawner,
            retry_backoff: Duration::ZERO,
            worker_exe: None,
            retire_tx: Mutex::new(None),
        })
    }

    pub fn configure_at_app_start(&self, backend: AiBackend) {
        let requested = TrtRequestedBackend::from_backend(backend);
        let mut inner = self.inner.lock().unwrap_or_else(|error| error.into_inner());
        if inner.state.revision() != TrtLifecycleRevision(0) {
            return;
        }
        let (revision, retired_group) = inner.advance_revision();
        inner.requested_backend = requested;
        inner.notice = None;
        inner.state = if requested == TrtRequestedBackend::TensorRt {
            TrtWorkerState::Eligible {
                revision,
                budget: TrtRecoveryBudget::default(),
            }
        } else {
            TrtWorkerState::Disabled {
                revision,
                reason: TrtDisabledReason::BackendNotRequested,
            }
        };
        drop(inner);
        drop(retired_group.close(revision));
        self.changed.notify_all();
    }

    pub fn select_backend(self: &Arc<Self>, backend: AiBackend) {
        let requested = TrtRequestedBackend::from_backend(backend);
        let (ticket, retired, retired_group, revision) = {
            let mut inner = self.inner.lock().unwrap_or_else(|error| error.into_inner());
            if matches!(
                inner.state,
                TrtWorkerState::Disabled {
                    reason: TrtDisabledReason::AppRetired,
                    ..
                }
            ) {
                return;
            }
            if matches!(
                inner.state,
                TrtWorkerState::Disabled {
                    reason: TrtDisabledReason::PackUninstalling,
                    ..
                }
            ) {
                inner.requested_backend = requested;
                inner.notice = None;
                drop(inner);
                self.changed.notify_all();
                return;
            }
            let (revision, retired_group) = inner.advance_revision();
            inner.requested_backend = requested;
            inner.notice = None;
            let state = if requested == TrtRequestedBackend::TensorRt {
                let ticket = inner.allocate_ticket(revision, TrtStartCause::BackendEnabled);
                TrtWorkerState::Starting {
                    ticket,
                    budget: TrtRecoveryBudget::default(),
                }
            } else {
                TrtWorkerState::Disabled {
                    revision,
                    reason: TrtDisabledReason::BackendNotRequested,
                }
            };
            let old = std::mem::replace(&mut inner.state, state);
            let retired = Self::pool_from_state(old);
            let ticket = match inner.state {
                TrtWorkerState::Starting { ticket, .. } => Some(ticket),
                _ => None,
            };
            (ticket, retired, retired_group, revision)
        };
        drop(retired_group.close(revision));
        self.changed.notify_all();
        self.retire_pool(retired);
        if let Some(ticket) = ticket {
            self.launch_attempt(ticket, Duration::ZERO);
        }
    }

    pub fn pack_installed(self: &Arc<Self>) {
        let (ticket, retired, retired_group, revision) = {
            let mut inner = self.inner.lock().unwrap_or_else(|error| error.into_inner());
            if matches!(
                inner.state,
                TrtWorkerState::Disabled {
                    reason: TrtDisabledReason::AppRetired,
                    ..
                }
            ) {
                return;
            }
            if matches!(
                inner.state,
                TrtWorkerState::Disabled {
                    reason: TrtDisabledReason::PackUninstalling,
                    ..
                }
            ) {
                inner.requested_backend = TrtRequestedBackend::TensorRt;
                inner.notice = None;
                drop(inner);
                self.changed.notify_all();
                return;
            }
            let (revision, retired_group) = inner.advance_revision();
            inner.requested_backend = TrtRequestedBackend::TensorRt;
            inner.notice = None;
            let ticket = inner.allocate_ticket(revision, TrtStartCause::PackInstalled);
            let old = std::mem::replace(
                &mut inner.state,
                TrtWorkerState::Starting {
                    ticket,
                    budget: TrtRecoveryBudget::default(),
                },
            );
            (ticket, Self::pool_from_state(old), retired_group, revision)
        };
        drop(retired_group.close(revision));
        self.changed.notify_all();
        self.retire_pool(retired);
        self.launch_attempt(ticket, Duration::ZERO);
    }

    pub fn manual_restart(self: &Arc<Self>, expected_revision: TrtLifecycleRevision) -> bool {
        let ticket = {
            let mut inner = self.inner.lock().unwrap_or_else(|error| error.into_inner());
            if inner.requested_backend != TrtRequestedBackend::TensorRt
                || inner.state.revision() != expected_revision
                || !matches!(inner.state, TrtWorkerState::Failed { .. })
            {
                return false;
            }
            let (revision, retired_group) = inner.advance_revision();
            inner.notice = None;
            let ticket = inner.allocate_ticket(revision, TrtStartCause::ManualRestart);
            inner.state = TrtWorkerState::Starting {
                ticket,
                budget: TrtRecoveryBudget::default(),
            };
            (ticket, retired_group, revision)
        };
        let (ticket, retired_group, revision) = ticket;
        drop(retired_group.close(revision));
        self.changed.notify_all();
        self.launch_attempt(ticket, Duration::ZERO);
        true
    }

    pub fn retire(&self) {
        let (retired, retired_group, revision) = {
            let mut inner = self.inner.lock().unwrap_or_else(|error| error.into_inner());
            if matches!(
                inner.state,
                TrtWorkerState::Disabled {
                    reason: TrtDisabledReason::AppRetired,
                    ..
                }
            ) {
                return;
            }
            let (revision, retired_group) = inner.advance_revision();
            inner.requested_backend = TrtRequestedBackend::Local;
            inner.notice = None;
            let old = std::mem::replace(
                &mut inner.state,
                TrtWorkerState::Disabled {
                    revision,
                    reason: TrtDisabledReason::AppRetired,
                },
            );
            (Self::pool_from_state(old), retired_group, revision)
        };
        drop(retired_group.close(revision));
        self.changed.notify_all();
        self.retire_pool(retired);
    }

    pub fn begin_pack_uninstall(self: &Arc<Self>) -> Option<TrtPackUninstallPermit> {
        let (retired, retired_group, revision) = {
            let mut inner = self.inner.lock().unwrap_or_else(|error| error.into_inner());
            if matches!(
                inner.state,
                TrtWorkerState::Disabled {
                    reason: TrtDisabledReason::AppRetired,
                    ..
                }
            ) {
                return None;
            }
            if matches!(
                inner.state,
                TrtWorkerState::Disabled {
                    reason: TrtDisabledReason::PackUninstalling,
                    ..
                }
            ) {
                return None;
            }
            let (revision, retired_group) = inner.advance_revision();
            inner.requested_backend = TrtRequestedBackend::Local;
            inner.notice = None;
            let old = std::mem::replace(
                &mut inner.state,
                TrtWorkerState::Disabled {
                    revision,
                    reason: TrtDisabledReason::PackUninstalling,
                },
            );
            (Self::pool_from_state(old), retired_group, revision)
        };
        let retire_barrier = retired_group.close(revision);
        self.changed.notify_all();
        self.retire_pool(retired);
        Some(TrtPackUninstallPermit {
            owner: Arc::downgrade(self),
            revision,
            retire_barrier,
        })
    }

    fn pack_uninstall_is_current(&self, revision: TrtLifecycleRevision) -> bool {
        let inner = self.inner.lock().unwrap_or_else(|error| error.into_inner());
        inner.state.revision() == revision
            && matches!(
                inner.state,
                TrtWorkerState::Disabled {
                    reason: TrtDisabledReason::PackUninstalling,
                    ..
                }
            )
    }

    fn complete_pack_uninstall(&self, expected_revision: TrtLifecycleRevision) {
        let (retired_group, revision) = {
            let mut inner = self.inner.lock().unwrap_or_else(|error| error.into_inner());
            if inner.state.revision() != expected_revision
                || !matches!(
                    inner.state,
                    TrtWorkerState::Disabled {
                        reason: TrtDisabledReason::PackUninstalling,
                        ..
                    }
                )
            {
                return;
            }
            let requested = inner.requested_backend;
            let (revision, retired_group) = inner.advance_revision();
            inner.state = if requested == TrtRequestedBackend::TensorRt {
                TrtWorkerState::Eligible {
                    revision,
                    budget: TrtRecoveryBudget::default(),
                }
            } else {
                TrtWorkerState::Disabled {
                    revision,
                    reason: TrtDisabledReason::BackendNotRequested,
                }
            };
            (retired_group, revision)
        };
        drop(retired_group.close(revision));
        self.changed.notify_all();
    }

    pub fn snapshot(&self) -> TrtWorkerSnapshot {
        let inner = self.inner.lock().unwrap_or_else(|error| error.into_inner());
        TrtWorkerSnapshot {
            revision: inner.state.revision(),
            phase: inner.state.phase(),
            pack_uninstalling: matches!(
                inner.state,
                TrtWorkerState::Disabled {
                    reason: TrtDisabledReason::PackUninstalling,
                    ..
                }
            ),
        }
    }

    pub fn take_notice(&self) -> Option<WorkerNotice> {
        self.inner
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .notice
            .take()
    }

    pub fn route_generation(&self, kind: ModelKind) -> TrtRouteGeneration {
        let inner = self.inner.lock().unwrap_or_else(|error| error.into_inner());
        if model_uses_trt_worker(kind)
            && let TrtWorkerState::Attached {
                revision, pool_id, ..
            } = &inner.state
        {
            TrtRouteGeneration::Worker {
                revision: *revision,
                pool_id: *pool_id,
            }
        } else {
            TrtRouteGeneration::DirectMl {
                revision: inner.state.revision(),
            }
        }
    }

    pub fn route_or_begin(self: &Arc<Self>, kind: ModelKind) -> TrtInferenceRoute {
        if !model_uses_trt_worker(kind) {
            return TrtInferenceRoute::direct_ml();
        }
        let mut launch = None;
        let route = {
            let mut inner = self.inner.lock().unwrap_or_else(|error| error.into_inner());
            match &inner.state {
                TrtWorkerState::Attached {
                    revision,
                    pool_id,
                    pool,
                    ..
                } => TrtInferenceRoute::worker(
                    *revision,
                    *pool_id,
                    Arc::clone(pool),
                    Arc::downgrade(self),
                ),
                TrtWorkerState::Eligible { revision, budget } => {
                    let revision = *revision;
                    let budget = *budget;
                    let ticket = inner.allocate_ticket(revision, TrtStartCause::LazyDemand);
                    inner.state = TrtWorkerState::Starting { ticket, budget };
                    launch = Some(ticket);
                    TrtInferenceRoute::direct_ml()
                }
                TrtWorkerState::Disabled { .. }
                | TrtWorkerState::Starting { .. }
                | TrtWorkerState::Failed { .. } => TrtInferenceRoute::direct_ml(),
            }
        };
        if let Some(ticket) = launch {
            self.changed.notify_all();
            self.launch_attempt(ticket, Duration::ZERO);
        }
        route
    }

    pub fn wait_for_diagnostic_start(&self, timeout: Duration) -> Result<(), String> {
        let deadline = Instant::now() + timeout;
        let mut inner = self.inner.lock().unwrap_or_else(|error| error.into_inner());
        loop {
            match &inner.state {
                TrtWorkerState::Attached { .. } => return Ok(()),
                TrtWorkerState::Failed { failure, .. } => {
                    return Err(failure.detail().to_owned());
                }
                TrtWorkerState::Disabled { reason, .. } => {
                    return Err(format!("TensorRT worker disabled: {reason:?}"));
                }
                TrtWorkerState::Eligible { .. } | TrtWorkerState::Starting { .. } => {}
            }
            let now = Instant::now();
            if now >= deadline {
                return Err("TensorRT worker lifecycle wait timed out".to_owned());
            }
            let waited = self
                .changed
                .wait_timeout(inner, deadline.saturating_duration_since(now))
                .unwrap_or_else(|error| error.into_inner());
            inner = waited.0;
        }
    }

    fn launch_attempt(self: &Arc<Self>, ticket: TrtStartTicket, delay: Duration) {
        let Some(activity) = self.begin_ticket_activity(ticket) else {
            return;
        };
        let owner = Arc::clone(self);
        let task: Box<dyn FnOnce() + Send> = Box::new(move || {
            let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                owner.run_attempt(ticket, delay)
            }));
            match outcome {
                Ok(Some(outcome)) => owner.complete_attempt(ticket, outcome, activity),
                Ok(None) => {}
                Err(_) => owner.complete_attempt(
                    ticket,
                    TrtStartOutcome::Internal {
                        kind: TrtLifecycleInternalFailure::TaskPanicked,
                        detail: "TensorRT worker lifecycle task panicked".to_owned(),
                    },
                    activity,
                ),
            }
        });
        if let Err(error) = self.task_spawner.spawn("trt-worker-lifecycle", task) {
            // `task` and its activity were dropped by the failed spawn. Publish the typed failure
            // with a fresh activity so an uninstall barrier still covers the retry decision.
            let Some(activity) = self.begin_ticket_activity(ticket) else {
                return;
            };
            self.complete_attempt(
                ticket,
                TrtStartOutcome::StartFailed(WorkerStartError::new(
                    WorkerStartFailureKind::ProcessSpawn,
                    format!("TensorRT lifecycle task spawn failed: {error}"),
                )),
                activity,
            );
        }
    }

    fn begin_ticket_activity(&self, ticket: TrtStartTicket) -> Option<TrtRetireActivity> {
        let inner = self.inner.lock().unwrap_or_else(|error| error.into_inner());
        if inner.requested_backend != ticket.requested_backend
            || !Self::state_has_ticket(&inner.state, ticket)
        {
            return None;
        }
        inner.retire_group.begin()
    }

    fn run_attempt(
        self: &Arc<Self>,
        ticket: TrtStartTicket,
        delay: Duration,
    ) -> Option<TrtStartOutcome> {
        if !delay.is_zero() && !self.wait_for_ticket(ticket, delay) {
            return None;
        }
        if !self.ticket_is_current(ticket) {
            return None;
        }
        crate::logger::log(format!(
            "[AI] TRT worker lifecycle start cause={:?} revision={} attempt={}",
            ticket.cause, ticket.revision.0, ticket.attempt_id
        ));
        match self.factory.pack_status() {
            PackStatus::Valid => {}
            unavailable => {
                return Some(TrtStartOutcome::PackUnavailable(unavailable));
            }
        }
        let outcome = match self.factory.start(self.worker_exe.as_deref()) {
            Ok(pool) => match self.ensure_reaper() {
                Ok(()) => TrtStartOutcome::Started(pool),
                Err(detail) => {
                    drop(pool);
                    TrtStartOutcome::Internal {
                        kind: TrtLifecycleInternalFailure::ReaperUnavailable(detail.clone()),
                        detail,
                    }
                }
            },
            Err(error) => TrtStartOutcome::StartFailed(error),
        };
        Some(outcome)
    }

    fn wait_for_ticket(&self, ticket: TrtStartTicket, delay: Duration) -> bool {
        let deadline = Instant::now() + delay;
        let mut inner = self.inner.lock().unwrap_or_else(|error| error.into_inner());
        loop {
            if !Self::state_has_ticket(&inner.state, ticket)
                || inner.requested_backend != ticket.requested_backend
            {
                return false;
            }
            let now = Instant::now();
            if now >= deadline {
                return true;
            }
            let waited = self
                .changed
                .wait_timeout(inner, deadline.saturating_duration_since(now))
                .unwrap_or_else(|error| error.into_inner());
            inner = waited.0;
        }
    }

    fn ticket_is_current(&self, ticket: TrtStartTicket) -> bool {
        let inner = self.inner.lock().unwrap_or_else(|error| error.into_inner());
        inner.requested_backend == ticket.requested_backend
            && Self::state_has_ticket(&inner.state, ticket)
    }

    fn state_has_ticket(state: &TrtWorkerState, ticket: TrtStartTicket) -> bool {
        matches!(state, TrtWorkerState::Starting { ticket: current, .. } if *current == ticket)
    }

    fn complete_attempt(
        self: &Arc<Self>,
        ticket: TrtStartTicket,
        outcome: TrtStartOutcome,
        activity: TrtRetireActivity,
    ) {
        let mut retry = None;
        let mut stale_pool = None;
        {
            let mut inner = self.inner.lock().unwrap_or_else(|error| error.into_inner());
            let budget = match &inner.state {
                TrtWorkerState::Starting {
                    ticket: current,
                    budget,
                } if *current == ticket && inner.requested_backend == ticket.requested_backend => {
                    *budget
                }
                _ => {
                    if let TrtStartOutcome::Started(pool) = outcome {
                        stale_pool = Some((pool, activity));
                    }
                    drop(inner);
                    self.retire_endpoint(stale_pool);
                    return;
                }
            };

            match outcome {
                TrtStartOutcome::Started(pool) => {
                    let retire_tx = match self.reaper_sender() {
                        Ok(sender) => sender,
                        Err(detail) => {
                            drop(pool);
                            let failure = TrtLifecycleFailure::Internal {
                                kind: TrtLifecycleInternalFailure::ReaperUnavailable(
                                    detail.clone(),
                                ),
                                detail,
                            };
                            Self::publish_failed(&mut inner, ticket.revision, failure);
                            drop(inner);
                            self.changed.notify_all();
                            return;
                        }
                    };
                    let pool_id = inner.allocate_pool_id();
                    let mut budget = budget;
                    budget.start_retries_used = 0;
                    inner.state = TrtWorkerState::Attached {
                        revision: ticket.revision,
                        pool_id,
                        pool: TrtAttachedPool::new(pool, activity, retire_tx),
                        budget,
                    };
                    crate::logger::log(format!(
                        "[AI] TRT worker attached revision={} pool={}",
                        ticket.revision.0, pool_id.0
                    ));
                }
                TrtStartOutcome::PackUnavailable(status) => {
                    inner.state = TrtWorkerState::Disabled {
                        revision: ticket.revision,
                        reason: TrtDisabledReason::PackUnavailable(status.clone()),
                    };
                    crate::logger::log(format!(
                        "[AI] TensorRT pack unavailable ({status:?}); DirectML で動作"
                    ));
                }
                TrtStartOutcome::StartFailed(error)
                    if error.kind.is_automatic_retry_allowed()
                        && budget.start_retries_used < MAX_START_RETRIES =>
                {
                    let mut budget = budget;
                    budget.start_retries_used += 1;
                    let next = inner
                        .allocate_ticket(ticket.revision, TrtStartCause::StartupTransientRetry);
                    inner.state = TrtWorkerState::Starting {
                        ticket: next,
                        budget,
                    };
                    crate::logger::log(format!(
                        "[AI] TRT worker transient start failure; retry #{} / {}: {}",
                        budget.start_retries_used, MAX_START_RETRIES, error.detail
                    ));
                    retry = Some(next);
                }
                TrtStartOutcome::StartFailed(error) => {
                    crate::logger::log(format!(
                        "[AI] TRT worker spawn failed ({:?}): {}",
                        error.kind, error.detail
                    ));
                    let failure = TrtLifecycleFailure::Start(error);
                    Self::publish_failed(&mut inner, ticket.revision, failure);
                }
                TrtStartOutcome::Internal { kind, detail } => {
                    crate::logger::log(format!(
                        "[AI] TRT worker lifecycle internal failure ({kind:?}): {detail}"
                    ));
                    let failure = TrtLifecycleFailure::Internal { kind, detail };
                    Self::publish_failed(&mut inner, ticket.revision, failure);
                }
            }
        }
        self.changed.notify_all();
        if let Some(ticket) = retry {
            self.launch_attempt(ticket, self.retry_backoff);
        }
    }

    fn publish_failed(
        inner: &mut TrtWorkerInner,
        revision: TrtLifecycleRevision,
        failure: TrtLifecycleFailure,
    ) {
        let kind = match &failure {
            TrtLifecycleFailure::Start(error) => WorkerNoticeKind::SpawnFailed(error.kind),
            TrtLifecycleFailure::DiedDuringInfer { .. } => WorkerNoticeKind::DiedDuringInfer,
            TrtLifecycleFailure::Internal { .. } => WorkerNoticeKind::LifecycleFailed,
        };
        inner.notice = Some(WorkerNotice {
            revision,
            kind,
            detail: failure.detail().to_owned(),
        });
        inner.state = TrtWorkerState::Failed { revision, failure };
    }

    fn report_infer_success(&self, pool_id: TrtWorkerPoolId) {
        let mut inner = self.inner.lock().unwrap_or_else(|error| error.into_inner());
        if let TrtWorkerState::Attached {
            pool_id: current,
            budget,
            ..
        } = &mut inner.state
            && *current == pool_id
        {
            budget.infer_death_recoveries_used = 0;
        }
    }

    fn report_worker_died(self: &Arc<Self>, pool_id: TrtWorkerPoolId, detail: String) {
        let mut launch = None;
        let mut retired = None;
        {
            let mut inner = self.inner.lock().unwrap_or_else(|error| error.into_inner());
            let placeholder_revision = inner.state.revision();
            let old = std::mem::replace(
                &mut inner.state,
                TrtWorkerState::Disabled {
                    revision: placeholder_revision,
                    reason: TrtDisabledReason::BackendNotRequested,
                },
            );
            match old {
                TrtWorkerState::Attached {
                    revision,
                    pool_id: current,
                    pool,
                    mut budget,
                } if current == pool_id
                    && inner.requested_backend == TrtRequestedBackend::TensorRt =>
                {
                    retired = Some(pool);
                    if budget.infer_death_recoveries_used < MAX_INFER_DEATH_RECOVERIES {
                        budget.infer_death_recoveries_used += 1;
                        budget.start_retries_used = 0;
                        let ticket =
                            inner.allocate_ticket(revision, TrtStartCause::InferDeathRetry);
                        inner.state = TrtWorkerState::Starting { ticket, budget };
                        crate::logger::log(format!(
                            "[AI] TRT worker death pool={} recovery #{} / {}: {}",
                            pool_id.0,
                            budget.infer_death_recoveries_used,
                            MAX_INFER_DEATH_RECOVERIES,
                            detail
                        ));
                        launch = Some(ticket);
                    } else {
                        let failure = TrtLifecycleFailure::DiedDuringInfer { pool_id, detail };
                        Self::publish_failed(&mut inner, revision, failure);
                    }
                }
                state => {
                    inner.state = state;
                    crate::logger::log(format!(
                        "[AI] stale TRT worker death ignored pool={}",
                        pool_id.0
                    ));
                }
            }
        }
        self.changed.notify_all();
        self.retire_pool(retired);
        if let Some(ticket) = launch {
            self.launch_attempt(ticket, Duration::ZERO);
        }
    }

    fn ensure_reaper(&self) -> Result<(), String> {
        let mut slot = self
            .retire_tx
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if slot.is_some() {
            return Ok(());
        }
        let (tx, rx) = mpsc::channel::<TrtRetireJob>();
        std::thread::Builder::new()
            .name("trt-worker-reaper".to_owned())
            .spawn(move || {
                while let Ok(job) = rx.recv() {
                    drop(job.pool);
                    drop(job.activity);
                }
            })
            .map_err(|error| format!("TensorRT worker reaper spawn failed: {error}"))?;
        *slot = Some(tx);
        Ok(())
    }

    fn reaper_sender(&self) -> Result<mpsc::Sender<TrtRetireJob>, String> {
        self.retire_tx
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone()
            .ok_or_else(|| "TensorRT worker reaper sender missing".to_owned())
    }

    fn retire_pool(&self, pool: Option<Arc<TrtAttachedPool>>) {
        drop(pool);
    }

    fn retire_endpoint(&self, retired: Option<(Arc<dyn TrtWorkerEndpoint>, TrtRetireActivity)>) {
        let Some((pool, activity)) = retired else {
            return;
        };
        let sender = self
            .retire_tx
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone();
        if let Some(sender) = sender {
            match sender.send(TrtRetireJob { pool, activity }) {
                Ok(()) => return,
                Err(error) => {
                    retain_stranded_retire_job(error.0);
                    return;
                }
            }
        }
        retain_stranded_retire_job(TrtRetireJob { pool, activity });
    }

    fn pool_from_state(state: TrtWorkerState) -> Option<Arc<TrtAttachedPool>> {
        match state {
            TrtWorkerState::Attached { pool, .. } => Some(pool),
            _ => None,
        }
    }
}

fn retain_or_send_retire_job(sender: &mpsc::Sender<TrtRetireJob>, job: TrtRetireJob) {
    if let Err(error) = sender.send(job) {
        retain_stranded_retire_job(error.0);
    }
}

fn retain_stranded_retire_job(job: TrtRetireJob) {
    // An attached pool can only exist after the reaper was created. If its receiver dies,
    // process-lifetime retention is safer than running a blocking Child::drop on a UI/request
    // caller. The retirement barrier intentionally remains closed in this internal-failure case.
    crate::logger::log("[AI] TRT worker reaper unavailable; retaining orphan until exit");
    static STRANDED_JOBS: OnceLock<Mutex<Vec<TrtRetireJob>>> = OnceLock::new();
    STRANDED_JOBS
        .get_or_init(|| Mutex::new(Vec::new()))
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .push(job);
}

enum TrtStartOutcome {
    Started(Arc<dyn TrtWorkerEndpoint>),
    PackUnavailable(PackStatus),
    StartFailed(WorkerStartError),
    Internal {
        kind: TrtLifecycleInternalFailure,
        detail: String,
    },
}

pub struct TrtInferenceRoute {
    worker: Option<TrtWorkerLease>,
}

struct TrtWorkerLease {
    revision: TrtLifecycleRevision,
    pool_id: TrtWorkerPoolId,
    pool: Arc<TrtAttachedPool>,
    owner: Weak<TrtWorkerLifecycleOwner>,
}

impl TrtInferenceRoute {
    fn direct_ml() -> Self {
        Self { worker: None }
    }

    fn worker(
        revision: TrtLifecycleRevision,
        pool_id: TrtWorkerPoolId,
        pool: Arc<TrtAttachedPool>,
        owner: Weak<TrtWorkerLifecycleOwner>,
    ) -> Self {
        Self {
            worker: Some(TrtWorkerLease {
                revision,
                pool_id,
                pool,
                owner,
            }),
        }
    }

    pub fn uses_worker(&self) -> bool {
        self.worker.is_some()
    }

    pub fn effective_backend(&self) -> AiBackend {
        if self.uses_worker() {
            AiBackend::TensorRt
        } else {
            AiBackend::DirectMl
        }
    }

    pub fn fall_back_to_direct_ml(&mut self) {
        self.worker = None;
    }

    pub fn infer(
        &self,
        kind: ModelKind,
        input: &Array4<f32>,
    ) -> Result<(Vec<i64>, Vec<f32>), AiError> {
        let lease = self.worker.as_ref().ok_or_else(|| {
            AiError::Ort("TensorRT inference was requested for a DirectML route".to_owned())
        })?;
        if let Err(error) = lease.pool.load_model(kind) {
            lease.report_failure(format!("load_model({kind:?}): {error}"));
            return Err(AiError::Ort(format!(
                "worker.load_model({kind:?}): {error}"
            )));
        }
        match lease.pool.infer(kind, input) {
            Ok(output) => {
                if let Some(owner) = lease.owner.upgrade() {
                    owner.report_infer_success(lease.pool_id);
                }
                Ok(output)
            }
            Err(error) => {
                lease.report_failure(format!("infer({kind:?}): {error}"));
                Err(AiError::Ort(format!("worker.infer({kind:?}): {error}")))
            }
        }
    }

    #[cfg(test)]
    fn pool_id(&self) -> Option<TrtWorkerPoolId> {
        self.worker.as_ref().map(|worker| worker.pool_id)
    }
}

impl TrtWorkerLease {
    fn report_failure(&self, detail: String) {
        if !self.pool.is_dead() {
            return;
        }
        if let Some(owner) = self.owner.upgrade() {
            crate::logger::log(format!(
                "[AI] TRT worker died during infer revision={} pool={}: {}",
                self.revision.0, self.pool_id.0, detail
            ));
            owner.report_worker_died(self.pool_id, detail);
        }
    }
}

/// Canonical list of models shipped in the TensorRT engine pack and routed to the worker.
/// Distribution tooling derives its required engine directories from this list.
pub const TRT_WORKER_MODEL_KINDS: [ModelKind; 5] = [
    ModelKind::UpscaleRealEsrganX4Plus,
    ModelKind::UpscaleRealEsrganAnime6B,
    ModelKind::UpscaleRealCugan4x,
    ModelKind::UpscaleNmkdSiax4x,
    ModelKind::DenoiseRealplksr,
];

pub fn model_uses_trt_worker(kind: ModelKind) -> bool {
    TRT_WORKER_MODEL_KINDS.contains(&kind)
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    use super::*;

    struct FakeEndpoint {
        dead: AtomicBool,
        fail: AtomicBool,
    }

    impl FakeEndpoint {
        fn healthy() -> Arc<Self> {
            Arc::new(Self {
                dead: AtomicBool::new(false),
                fail: AtomicBool::new(false),
            })
        }

        fn kill(&self) {
            self.dead.store(true, Ordering::Release);
            self.fail.store(true, Ordering::Release);
        }
    }

    impl TrtWorkerEndpoint for FakeEndpoint {
        fn load_model(&self, _kind: ModelKind) -> Result<u64, String> {
            if self.fail.load(Ordering::Acquire) {
                Err("fake dead worker".to_owned())
            } else {
                Ok(0)
            }
        }

        fn infer(
            &self,
            _kind: ModelKind,
            _input: &Array4<f32>,
        ) -> Result<(Vec<i64>, Vec<f32>), String> {
            if self.fail.load(Ordering::Acquire) {
                Err("fake infer failure".to_owned())
            } else {
                Ok((vec![1, 3, 1, 1], vec![0.0; 3]))
            }
        }

        fn is_dead(&self) -> bool {
            self.dead.load(Ordering::Acquire)
        }
    }

    enum FactoryOutcome {
        Success(Arc<FakeEndpoint>),
        Failure(WorkerStartFailureKind),
        Panic,
    }

    struct FakeFactory {
        starts: AtomicUsize,
        outcomes: Mutex<VecDeque<FactoryOutcome>>,
    }

    impl FakeFactory {
        fn new(outcomes: impl IntoIterator<Item = FactoryOutcome>) -> Arc<Self> {
            Arc::new(Self {
                starts: AtomicUsize::new(0),
                outcomes: Mutex::new(outcomes.into_iter().collect()),
            })
        }
    }

    impl TrtPoolFactory for FakeFactory {
        fn pack_status(&self) -> PackStatus {
            PackStatus::Valid
        }

        fn start(
            &self,
            _worker_exe: Option<&Path>,
        ) -> Result<Arc<dyn TrtWorkerEndpoint>, WorkerStartError> {
            self.starts.fetch_add(1, Ordering::AcqRel);
            match self.outcomes.lock().unwrap().pop_front().unwrap() {
                FactoryOutcome::Success(endpoint) => Ok(endpoint),
                FactoryOutcome::Failure(kind) => {
                    Err(WorkerStartError::new(kind, format!("fake {kind:?}")))
                }
                FactoryOutcome::Panic => panic!("fake lifecycle panic"),
            }
        }
    }

    struct InlineSpawner {
        failures_remaining: AtomicUsize,
        calls: AtomicUsize,
    }

    impl InlineSpawner {
        fn success() -> Arc<Self> {
            Arc::new(Self {
                failures_remaining: AtomicUsize::new(0),
                calls: AtomicUsize::new(0),
            })
        }

        fn fail(count: usize) -> Arc<Self> {
            Arc::new(Self {
                failures_remaining: AtomicUsize::new(count),
                calls: AtomicUsize::new(0),
            })
        }
    }

    impl TrtTaskSpawner for InlineSpawner {
        fn spawn(&self, _name: &str, task: Box<dyn FnOnce() + Send>) -> std::io::Result<()> {
            self.calls.fetch_add(1, Ordering::AcqRel);
            if self
                .failures_remaining
                .fetch_update(Ordering::AcqRel, Ordering::Acquire, |remaining| {
                    remaining.checked_sub(1)
                })
                .is_ok()
            {
                Err(std::io::Error::other("fake task spawn failure"))
            } else {
                task();
                Ok(())
            }
        }
    }

    struct QueuedSpawner {
        calls: AtomicUsize,
        tasks: Mutex<VecDeque<Box<dyn FnOnce() + Send>>>,
    }

    impl QueuedSpawner {
        fn new() -> Arc<Self> {
            Arc::new(Self {
                calls: AtomicUsize::new(0),
                tasks: Mutex::new(VecDeque::new()),
            })
        }

        fn run_next(&self) {
            let task = self
                .tasks
                .lock()
                .unwrap()
                .pop_front()
                .expect("queued lifecycle task");
            task();
        }

        fn pending(&self) -> usize {
            self.tasks.lock().unwrap().len()
        }
    }

    impl TrtTaskSpawner for QueuedSpawner {
        fn spawn(&self, _name: &str, task: Box<dyn FnOnce() + Send>) -> std::io::Result<()> {
            self.calls.fetch_add(1, Ordering::AcqRel);
            self.tasks.lock().unwrap().push_back(task);
            Ok(())
        }
    }

    fn anime_kind() -> ModelKind {
        ModelKind::UpscaleRealEsrganAnime6B
    }

    #[test]
    fn deterministic_lazy_failure_is_terminal_for_all_later_demands() {
        let factory =
            FakeFactory::new([FactoryOutcome::Failure(WorkerStartFailureKind::RuntimeInit)]);
        let owner =
            TrtWorkerLifecycleOwner::new_for_test(factory.clone(), InlineSpawner::success());
        owner.configure_at_app_start(AiBackend::TensorRt);

        assert!(!owner.route_or_begin(anime_kind()).uses_worker());
        assert_eq!(owner.snapshot().phase, TrtWorkerPhase::Failed);
        for _ in 0..12 {
            assert!(!owner.route_or_begin(anime_kind()).uses_worker());
        }
        assert_eq!(factory.starts.load(Ordering::Acquire), 1);
        assert!(matches!(
            owner.take_notice().map(|notice| notice.kind),
            Some(WorkerNoticeKind::SpawnFailed(
                WorkerStartFailureKind::RuntimeInit
            ))
        ));
        assert!(owner.take_notice().is_none());
        assert_eq!(owner.snapshot().phase, TrtWorkerPhase::Failed);
    }

    #[test]
    fn transient_start_gets_exactly_one_retry() {
        let factory = FakeFactory::new([
            FactoryOutcome::Failure(WorkerStartFailureKind::Transport),
            FactoryOutcome::Failure(WorkerStartFailureKind::Transport),
        ]);
        let owner =
            TrtWorkerLifecycleOwner::new_for_test(factory.clone(), InlineSpawner::success());
        owner.configure_at_app_start(AiBackend::TensorRt);
        owner.route_or_begin(anime_kind());
        assert_eq!(factory.starts.load(Ordering::Acquire), 2);
        assert_eq!(owner.snapshot().phase, TrtWorkerPhase::Failed);
    }

    #[test]
    fn task_panic_is_deterministic_and_not_retried() {
        let factory = FakeFactory::new([FactoryOutcome::Panic]);
        let owner =
            TrtWorkerLifecycleOwner::new_for_test(factory.clone(), InlineSpawner::success());
        owner.configure_at_app_start(AiBackend::TensorRt);
        owner.route_or_begin(anime_kind());
        assert_eq!(factory.starts.load(Ordering::Acquire), 1);
        assert_eq!(owner.snapshot().phase, TrtWorkerPhase::Failed);
        assert!(matches!(
            owner.take_notice().map(|notice| notice.kind),
            Some(WorkerNoticeKind::LifecycleFailed)
        ));
    }

    #[test]
    fn os_task_spawn_failure_uses_the_one_transient_retry() {
        let factory = FakeFactory::new([]);
        let spawner = InlineSpawner::fail(2);
        let owner = TrtWorkerLifecycleOwner::new_for_test(factory, spawner.clone());
        owner.configure_at_app_start(AiBackend::TensorRt);
        owner.route_or_begin(anime_kind());
        assert_eq!(spawner.calls.load(Ordering::Acquire), 2);
        assert_eq!(owner.snapshot().phase, TrtWorkerPhase::Failed);
    }

    #[test]
    fn demand_api_cannot_rearm_failed_but_manual_restart_can() {
        let endpoint = FakeEndpoint::healthy();
        let factory = FakeFactory::new([
            FactoryOutcome::Failure(WorkerStartFailureKind::RuntimeInit),
            FactoryOutcome::Success(endpoint),
        ]);
        let owner =
            TrtWorkerLifecycleOwner::new_for_test(factory.clone(), InlineSpawner::success());
        owner.configure_at_app_start(AiBackend::TensorRt);
        owner.route_or_begin(anime_kind());
        let failed = owner.snapshot();
        for _ in 0..4 {
            owner.route_or_begin(anime_kind());
        }
        assert_eq!(owner.snapshot(), failed);
        assert_eq!(factory.starts.load(Ordering::Acquire), 1);
        assert!(owner.manual_restart(failed.revision));
        assert_eq!(owner.snapshot().phase, TrtWorkerPhase::Attached);
        assert_eq!(factory.starts.load(Ordering::Acquire), 2);
    }

    #[test]
    fn explicit_transition_clears_owner_notice_and_rejects_stale_banner_revision() {
        let factory =
            FakeFactory::new([FactoryOutcome::Failure(WorkerStartFailureKind::RuntimeInit)]);
        let owner =
            TrtWorkerLifecycleOwner::new_for_test(factory.clone(), InlineSpawner::success());
        owner.configure_at_app_start(AiBackend::TensorRt);
        owner.route_or_begin(anime_kind());
        let notice = owner.take_notice().expect("terminal failure notice");

        owner.select_backend(AiBackend::DirectMl);

        assert!(owner.take_notice().is_none());
        assert!(!owner.manual_restart(notice.revision));
        assert_eq!(owner.snapshot().phase, TrtWorkerPhase::Disabled);
        assert_eq!(factory.starts.load(Ordering::Acquire), 1);
    }

    #[test]
    fn successful_pack_install_rearm_clears_notice_and_invalidates_old_banner() {
        let factory = FakeFactory::new([
            FactoryOutcome::Failure(WorkerStartFailureKind::RuntimeInit),
            FactoryOutcome::Success(FakeEndpoint::healthy()),
        ]);
        let owner =
            TrtWorkerLifecycleOwner::new_for_test(factory.clone(), InlineSpawner::success());
        owner.configure_at_app_start(AiBackend::TensorRt);
        owner.route_or_begin(anime_kind());
        let notice = owner.take_notice().expect("terminal failure notice");

        owner.pack_installed();

        assert_eq!(owner.snapshot().phase, TrtWorkerPhase::Attached);
        assert!(owner.take_notice().is_none());
        assert!(!owner.manual_restart(notice.revision));
        assert_eq!(factory.starts.load(Ordering::Acquire), 2);
    }

    #[test]
    fn first_three_matching_deaths_restart_and_fourth_is_terminal() {
        let endpoints: Vec<_> = (0..4).map(|_| FakeEndpoint::healthy()).collect();
        let factory = FakeFactory::new(endpoints.iter().cloned().map(FactoryOutcome::Success));
        let owner = TrtWorkerLifecycleOwner::new_for_test(factory, InlineSpawner::success());
        owner.configure_at_app_start(AiBackend::TensorRt);
        owner.route_or_begin(anime_kind());

        for endpoint in endpoints.iter().take(3) {
            let route = owner.route_or_begin(anime_kind());
            assert!(route.uses_worker());
            endpoint.kill();
            let input = Array4::<f32>::zeros((1, 3, 1, 1));
            assert!(route.infer(anime_kind(), &input).is_err());
            assert_eq!(owner.snapshot().phase, TrtWorkerPhase::Attached);
        }

        let fourth = owner.route_or_begin(anime_kind());
        endpoints[3].kill();
        let input = Array4::<f32>::zeros((1, 3, 1, 1));
        assert!(fourth.infer(anime_kind(), &input).is_err());
        assert_eq!(owner.snapshot().phase, TrtWorkerPhase::Failed);
        assert!(matches!(
            owner.take_notice().map(|notice| notice.kind),
            Some(WorkerNoticeKind::DiedDuringInfer)
        ));
    }

    #[test]
    fn stale_old_pool_death_does_not_detach_the_new_pool() {
        let old = FakeEndpoint::healthy();
        let new = FakeEndpoint::healthy();
        let factory = FakeFactory::new([
            FactoryOutcome::Success(old.clone()),
            FactoryOutcome::Success(new),
        ]);
        let owner = TrtWorkerLifecycleOwner::new_for_test(factory, InlineSpawner::success());
        owner.configure_at_app_start(AiBackend::TensorRt);
        owner.route_or_begin(anime_kind());
        let old_route = owner.route_or_begin(anime_kind());
        let old_id = old_route.pool_id().unwrap();

        owner.select_backend(AiBackend::TensorRt);
        let new_route = owner.route_or_begin(anime_kind());
        assert_ne!(new_route.pool_id(), Some(old_id));
        old.kill();
        let input = Array4::<f32>::zeros((1, 3, 1, 1));
        assert!(old_route.infer(anime_kind(), &input).is_err());
        assert_eq!(owner.snapshot().phase, TrtWorkerPhase::Attached);
        assert_eq!(
            owner.route_or_begin(anime_kind()).pool_id(),
            new_route.pool_id()
        );
    }

    #[test]
    fn backend_change_rejects_stale_start_completion() {
        let old = FakeEndpoint::healthy();
        let new = FakeEndpoint::healthy();
        let factory = FakeFactory::new([FactoryOutcome::Success(new)]);
        let spawner = QueuedSpawner::new();
        let owner = TrtWorkerLifecycleOwner::new_for_test(factory.clone(), spawner.clone());
        owner.configure_at_app_start(AiBackend::TensorRt);

        let initial_route = owner.route_or_begin(anime_kind());
        assert!(!initial_route.uses_worker());
        let (old_ticket, old_activity) = {
            let inner = owner.inner.lock().unwrap();
            let ticket = match &inner.state {
                TrtWorkerState::Starting { ticket, .. } => *ticket,
                _ => panic!("lazy demand must publish a start ticket"),
            };
            (ticket, inner.retire_group.begin().unwrap())
        };
        let old_revision = owner.snapshot().revision;
        owner.select_backend(AiBackend::TensorRt);
        let new_revision = owner.snapshot().revision;
        assert_ne!(new_revision, old_revision);
        assert_eq!(spawner.pending(), 2);

        owner.ensure_reaper().unwrap();
        owner.complete_attempt(old_ticket, TrtStartOutcome::Started(old), old_activity);
        assert_eq!(owner.snapshot().phase, TrtWorkerPhase::Starting);
        assert_eq!(owner.snapshot().revision, new_revision);
        spawner.run_next();
        assert_eq!(owner.snapshot().phase, TrtWorkerPhase::Starting);
        assert_eq!(owner.snapshot().revision, new_revision);
        spawner.run_next();
        assert_eq!(owner.snapshot().phase, TrtWorkerPhase::Attached);
        assert_eq!(owner.snapshot().revision, new_revision);
        assert_eq!(factory.starts.load(Ordering::Acquire), 1);
    }

    #[test]
    fn backend_off_cancels_queued_start_and_transient_retry() {
        let factory =
            FakeFactory::new([FactoryOutcome::Failure(WorkerStartFailureKind::Transport)]);
        let spawner = QueuedSpawner::new();
        let owner = TrtWorkerLifecycleOwner::new_for_test(factory.clone(), spawner.clone());
        owner.configure_at_app_start(AiBackend::TensorRt);
        owner.route_or_begin(anime_kind());
        spawner.run_next();
        assert_eq!(factory.starts.load(Ordering::Acquire), 1);
        assert_eq!(owner.snapshot().phase, TrtWorkerPhase::Starting);
        assert_eq!(spawner.pending(), 1, "transient retry is queued");

        owner.select_backend(AiBackend::DirectMl);
        let disabled = owner.snapshot();
        spawner.run_next();
        assert_eq!(owner.snapshot(), disabled);
        assert_eq!(owner.snapshot().phase, TrtWorkerPhase::Disabled);
        assert_eq!(factory.starts.load(Ordering::Acquire), 1);
        assert!(owner.take_notice().is_none());
    }

    #[test]
    fn request_route_does_not_switch_after_late_attach() {
        let endpoint = FakeEndpoint::healthy();
        let factory = FakeFactory::new([FactoryOutcome::Success(endpoint)]);
        let spawner = QueuedSpawner::new();
        let owner = TrtWorkerLifecycleOwner::new_for_test(factory, spawner.clone());
        owner.configure_at_app_start(AiBackend::TensorRt);

        let route_before_attach = owner.route_or_begin(anime_kind());
        assert!(!route_before_attach.uses_worker());
        spawner.run_next();
        assert!(!route_before_attach.uses_worker());
        assert!(owner.route_or_begin(anime_kind()).uses_worker());
    }

    #[test]
    fn successful_inference_resets_the_infer_death_recovery_budget() {
        let endpoints: Vec<_> = (0..5).map(|_| FakeEndpoint::healthy()).collect();
        let factory = FakeFactory::new(endpoints.iter().cloned().map(FactoryOutcome::Success));
        let owner = TrtWorkerLifecycleOwner::new_for_test(factory, InlineSpawner::success());
        owner.configure_at_app_start(AiBackend::TensorRt);
        owner.route_or_begin(anime_kind());
        let input = Array4::<f32>::zeros((1, 3, 1, 1));

        let first = owner.route_or_begin(anime_kind());
        endpoints[0].kill();
        assert!(first.infer(anime_kind(), &input).is_err());
        let recovered = owner.route_or_begin(anime_kind());
        assert!(recovered.infer(anime_kind(), &input).is_ok());

        for endpoint in endpoints.iter().skip(1).take(3) {
            let route = owner.route_or_begin(anime_kind());
            endpoint.kill();
            assert!(route.infer(anime_kind(), &input).is_err());
            assert_eq!(owner.snapshot().phase, TrtWorkerPhase::Attached);
        }
        let fourth_after_success = owner.route_or_begin(anime_kind());
        endpoints[4].kill();
        assert!(fourth_after_success.infer(anime_kind(), &input).is_err());
        assert_eq!(owner.snapshot().phase, TrtWorkerPhase::Failed);
    }

    #[test]
    fn route_generation_is_read_only_and_changes_with_pool_identity() {
        let first = FakeEndpoint::healthy();
        let second = FakeEndpoint::healthy();
        let factory = FakeFactory::new([
            FactoryOutcome::Success(first),
            FactoryOutcome::Success(second),
        ]);
        let spawner = QueuedSpawner::new();
        let owner = TrtWorkerLifecycleOwner::new_for_test(factory.clone(), spawner.clone());
        owner.configure_at_app_start(AiBackend::TensorRt);

        let eligible = owner.route_generation(anime_kind());
        assert_eq!(factory.starts.load(Ordering::Acquire), 0);
        assert_eq!(spawner.pending(), 0);
        assert_eq!(owner.route_generation(anime_kind()), eligible);

        owner.route_or_begin(anime_kind());
        spawner.run_next();
        let attached = owner.route_generation(anime_kind());
        assert_ne!(attached.digest_bytes(), eligible.digest_bytes());
        owner.select_backend(AiBackend::TensorRt);
        spawner.run_next();
        let replaced = owner.route_generation(anime_kind());
        assert_ne!(replaced.digest_bytes(), attached.digest_bytes());
    }

    #[test]
    fn pack_uninstall_barrier_waits_for_start_task_and_route_lease_shutdown() {
        let endpoint = FakeEndpoint::healthy();
        let factory = FakeFactory::new([FactoryOutcome::Success(endpoint)]);
        let spawner = QueuedSpawner::new();
        let owner = TrtWorkerLifecycleOwner::new_for_test(factory, spawner.clone());
        owner.configure_at_app_start(AiBackend::TensorRt);
        owner.route_or_begin(anime_kind());
        let starting_permit = owner.begin_pack_uninstall().unwrap();
        assert!(owner.snapshot().is_pack_uninstalling());

        let (start_done_tx, start_done_rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            starting_permit.wait_for_retirement();
            start_done_tx.send(starting_permit).unwrap();
        });
        assert!(
            start_done_rx
                .recv_timeout(Duration::from_millis(20))
                .is_err(),
            "queued start activity must hold the uninstall barrier"
        );
        spawner.run_next();
        let starting_permit = start_done_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        starting_permit.complete();

        owner.select_backend(AiBackend::TensorRt);
        spawner.run_next();
        let route = owner.route_or_begin(anime_kind());
        assert!(route.uses_worker());
        let attached_permit = owner.begin_pack_uninstall().unwrap();
        let (route_done_tx, route_done_rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            attached_permit.wait_for_retirement();
            route_done_tx.send(attached_permit).unwrap();
        });
        assert!(
            route_done_rx
                .recv_timeout(Duration::from_millis(20))
                .is_err(),
            "an in-flight request lease must hold the uninstall barrier"
        );
        drop(route);
        let attached_permit = route_done_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        assert!(attached_permit.is_current());
        attached_permit.complete();
        assert_eq!(owner.snapshot().phase, TrtWorkerPhase::Disabled);
        assert!(!owner.snapshot().is_pack_uninstalling());
    }

    #[test]
    fn pack_rearm_is_deferred_until_uninstall_completes() {
        let endpoint = FakeEndpoint::healthy();
        let factory = FakeFactory::new([FactoryOutcome::Success(endpoint)]);
        let spawner = QueuedSpawner::new();
        let owner = TrtWorkerLifecycleOwner::new_for_test(factory, spawner.clone());
        owner.configure_at_app_start(AiBackend::TensorRt);

        let permit = owner.begin_pack_uninstall().unwrap();
        permit.wait_for_retirement();
        owner.pack_installed();
        owner.select_backend(AiBackend::TensorRt);
        assert!(owner.snapshot().is_pack_uninstalling());
        assert_eq!(spawner.pending(), 0);
        permit.complete();
        assert_eq!(owner.snapshot().phase, TrtWorkerPhase::Eligible);
        assert!(!owner.route_or_begin(anime_kind()).uses_worker());
        assert_eq!(spawner.pending(), 1);
        spawner.run_next();
        assert_eq!(owner.snapshot().phase, TrtWorkerPhase::Attached);
    }

    #[test]
    fn app_retired_is_absorbing_for_every_external_rearm() {
        let factory = FakeFactory::new([]);
        let spawner = QueuedSpawner::new();
        let owner = TrtWorkerLifecycleOwner::new_for_test(factory.clone(), spawner.clone());
        owner.configure_at_app_start(AiBackend::TensorRt);
        owner.retire();
        let retired = owner.snapshot();

        owner.select_backend(AiBackend::TensorRt);
        owner.pack_installed();
        assert!(!owner.manual_restart(retired.revision));
        assert!(owner.begin_pack_uninstall().is_none());
        assert!(!owner.route_or_begin(anime_kind()).uses_worker());
        assert_eq!(owner.snapshot(), retired);
        assert_eq!(spawner.pending(), 0);
        assert_eq!(factory.starts.load(Ordering::Acquire), 0);
    }

    #[test]
    fn models_outside_the_pack_never_start_the_worker() {
        let factory = FakeFactory::new([]);
        let owner =
            TrtWorkerLifecycleOwner::new_for_test(factory.clone(), InlineSpawner::success());
        owner.configure_at_app_start(AiBackend::TensorRt);
        for kind in [
            ModelKind::InpaintMiGan,
            ModelKind::SubjectMatte,
            ModelKind::UpscaleRealEsrGeneralV3,
        ] {
            assert!(!owner.route_or_begin(kind).uses_worker());
        }
        assert_eq!(factory.starts.load(Ordering::Acquire), 0);
        assert_eq!(owner.snapshot().phase, TrtWorkerPhase::Eligible);
    }

    #[test]
    fn every_canonical_pack_model_enters_the_same_demand_gate() {
        for kind in TRT_WORKER_MODEL_KINDS {
            let factory =
                FakeFactory::new([FactoryOutcome::Failure(WorkerStartFailureKind::RuntimeInit)]);
            let owner =
                TrtWorkerLifecycleOwner::new_for_test(factory.clone(), InlineSpawner::success());
            owner.configure_at_app_start(AiBackend::TensorRt);

            assert!(!owner.route_or_begin(kind).uses_worker(), "{kind:?}");
            assert_eq!(owner.snapshot().phase, TrtWorkerPhase::Failed, "{kind:?}");
            assert_eq!(factory.starts.load(Ordering::Acquire), 1, "{kind:?}");
        }
    }

    #[test]
    fn product_consumers_do_not_bypass_the_lifecycle_owner() {
        let product_sources = [
            ("runtime", include_str!("runtime.rs")),
            ("app", include_str!("../app.rs")),
            ("preferences", include_str!("../ui_dialogs/preferences.rs")),
            ("notice", include_str!("../ui_dialogs/trt_worker_notice.rs")),
            ("install", include_str!("../ui_dialogs/trt_install.rs")),
            ("remote", include_str!("../remote_ipc/container.rs")),
            ("books", include_str!("../books.rs")),
            ("materializer", include_str!("../materializer.rs")),
            ("video", include_str!("../video/upscale/job.rs")),
        ];
        for (name, source) in product_sources {
            for forbidden in [
                "TrtWorkerPool::start",
                "attach_worker_pool",
                "detach_worker_pool",
                "trt_restart_in_flight",
                "trt_auto_restart_attempts",
                "trt_spawn_restart_attempts",
            ] {
                assert!(
                    !source.contains(forbidden),
                    "{name} still bypasses lifecycle owner via {forbidden}"
                );
            }
        }
    }
}
