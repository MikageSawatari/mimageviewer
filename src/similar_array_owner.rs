//! One store, one worker: base I/O, immutable publication, acknowledgements and history pruning.
//! Configuration belongs to the scheduler; it never cancels this store's initialization.

use std::path::PathBuf;
use std::sync::{
    Arc, Condvar, Mutex,
    atomic::{AtomicU64, Ordering},
    mpsc::{self, Receiver, Sender},
};
use std::time::Instant;

use crate::similar_db::{SimilarDb, StoreWatermark};
use crate::similar_search_array::{self as array, BasePersistence, SearchSnapshot};

static NEXT_OWNER: AtomicU64 = AtomicU64::new(1);

/// Only the store worker can construct product base-I/O authority.
pub(crate) struct BaseAccess(());
impl BaseAccess {
    /// Only disposable fixtures and standalone dev-tools copies may bypass the running owner.
    #[cfg(any(test, feature = "dev-tools"))]
    pub(crate) fn fixture() -> Self {
        Self(())
    }
}

#[derive(Clone, Debug)]
pub(crate) struct PublishedArray {
    pub(crate) snapshot: Arc<SearchSnapshot>,
    pub(crate) revision: u64,
}
impl PublishedArray {
    fn stamp(&self) -> StoreWatermark {
        StoreWatermark {
            store_id: self.snapshot.base.store_id,
            through_change_seq: self.snapshot.applied_seq,
        }
    }
    pub(crate) fn reaches(&self, goal: StoreWatermark) -> bool {
        let stamp = self.stamp();
        stamp.store_id == goal.store_id && stamp.through_change_seq >= goal.through_change_seq
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ArrayDemand {
    LoadIfPresent,
    LoadOrCreate,
    RefreshIfPresent,
    RefreshOrCreate,
}
impl ArrayDemand {
    fn creates(self) -> bool {
        matches!(self, Self::LoadOrCreate | Self::RefreshOrCreate)
    }
    fn refreshes(self) -> bool {
        matches!(self, Self::RefreshIfPresent | Self::RefreshOrCreate)
    }
    fn merge(self, other: Self) -> Self {
        match (
            self.creates() || other.creates(),
            self.refreshes() || other.refreshes(),
        ) {
            (false, false) => Self::LoadIfPresent,
            (true, false) => Self::LoadOrCreate,
            (false, true) => Self::RefreshIfPresent,
            (true, true) => Self::RefreshOrCreate,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ArrayOperation {
    Demand(ArrayDemand),
    Compact,
    Worker,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ArrayFailure {
    pub(crate) operation: ArrayOperation,
    pub(crate) detail: String,
}
impl std::fmt::Display for ArrayFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{:?}: {}。mImageViewer を再起動してください。",
            self.operation, self.detail
        )
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ArrayAck {
    Published(StoreWatermark),
    StoreChanged {
        expected: StoreWatermark,
        observed: StoreWatermark,
    },
    Failed(ArrayFailure),
    Stopped,
}
#[derive(Clone, Debug)]
pub(crate) enum OwnerTerminal {
    Stopped,
    Failed(ArrayFailure),
}

enum ArrayView {
    Uninitialized,
    Missing,
    Published(PublishedArray),
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PendingDemand {
    None,
    Some(ArrayDemand),
}
impl PendingDemand {
    fn add(&mut self, demand: ArrayDemand) {
        *self = Self::Some(match *self {
            Self::None => demand,
            Self::Some(old) => old.merge(demand),
        });
    }
}
#[derive(Clone, Copy)]
enum ArrayWork {
    Idle,
    Queued {
        operation: ArrayOperation,
        pending: PendingDemand,
    },
    Executing {
        operation: ArrayOperation,
        pending: PendingDemand,
    },
}
struct AckRequest {
    id: u64,
    goal: StoreWatermark,
    reply: Sender<ArrayAck>,
}
struct OpenArray {
    view: ArrayView,
    work: ArrayWork,
    acknowledgements: Vec<AckRequest>,
}
enum ArrayOwnerState {
    Open(OpenArray),
    Faulted {
        published: Option<PublishedArray>,
        failure: ArrayFailure,
    },
    Closing {
        published: Option<PublishedArray>,
    },
    Closed(OwnerTerminal),
}
impl ArrayOwnerState {
    fn published(&self) -> Option<&PublishedArray> {
        match self {
            Self::Open(OpenArray {
                view: ArrayView::Published(published),
                ..
            }) => Some(published),
            Self::Faulted { published, .. } | Self::Closing { published } => published.as_ref(),
            _ => None,
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) enum ArrayObservation {
    Uninitialized {
        busy: bool,
    },
    Missing,
    Published(PublishedArray),
    Faulted {
        published: Option<PublishedArray>,
        failure: ArrayFailure,
    },
    Closing,
    Closed(OwnerTerminal),
}
impl ArrayObservation {
    pub(crate) fn published(&self) -> Option<&PublishedArray> {
        match self {
            Self::Published(value) => Some(value),
            Self::Faulted { published, .. } => published.as_ref(),
            _ => None,
        }
    }
    pub(crate) fn failure(&self) -> Option<&ArrayFailure> {
        match self {
            Self::Faulted { failure, .. } | Self::Closed(OwnerTerminal::Failed(failure)) => {
                Some(failure)
            }
            _ => None,
        }
    }
}

struct Shared {
    state: Mutex<ArrayOwnerState>,
    wake: Condvar,
    data_dir: PathBuf,
    notify: Box<dyn Fn() + Send + Sync>,
    id: u64,
    next_ack: AtomicU64,
    elapsed_us: AtomicU64,
    operation_id: AtomicU64,
    #[cfg(test)]
    probe: Mutex<Option<Arc<dyn Fn(OwnerPhase) + Send + Sync>>>,
    #[cfg(test)]
    operations: AtomicU64,
    #[cfg(test)]
    injected_failure: Mutex<Option<OwnerPhase>>,
}
/// The non-clone creation/close authority lives in the manager; everyone else receives a handle.
pub(crate) struct SimilarArrayOwner {
    handle: SimilarArrayHandle,
}
#[derive(Clone)]
pub(crate) struct SimilarArrayHandle {
    shared: Arc<Shared>,
}

impl SimilarArrayOwner {
    pub(crate) fn new(data_dir: PathBuf, notify: impl Fn() + Send + Sync + 'static) -> Self {
        let shared = Arc::new(Shared {
            state: Mutex::new(ArrayOwnerState::Open(OpenArray {
                view: ArrayView::Uninitialized,
                work: ArrayWork::Idle,
                acknowledgements: Vec::new(),
            })),
            wake: Condvar::new(),
            data_dir,
            notify: Box::new(notify),
            id: NEXT_OWNER.fetch_add(1, Ordering::Relaxed),
            next_ack: AtomicU64::new(1),
            elapsed_us: AtomicU64::new(0),
            operation_id: AtomicU64::new(0),
            #[cfg(test)]
            probe: Mutex::new(None),
            #[cfg(test)]
            operations: AtomicU64::new(0),
            #[cfg(test)]
            injected_failure: Mutex::new(None),
        });
        let worker = Arc::clone(&shared);
        if let Err(error) = std::thread::Builder::new()
            .name("similar-array-owner".into())
            .spawn(move || {
                let result =
                    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| worker.run()));
                worker.close_worker(if result.is_ok() {
                    OwnerTerminal::Stopped
                } else {
                    OwnerTerminal::Failed(ArrayFailure {
                        operation: worker.active_operation(),
                        detail: "array owner worker panicked".into(),
                    })
                });
            })
        {
            let failure = ArrayFailure {
                operation: ArrayOperation::Worker,
                detail: format!("array owner worker start failed: {error}"),
            };
            crate::logger::log(format!(
                "similar array owner: owner_id={} terminal=Failed detail={failure}",
                shared.id
            ));
            *shared.state.lock().unwrap_or_else(|e| e.into_inner()) =
                ArrayOwnerState::Closed(OwnerTerminal::Failed(failure));
        }
        Self {
            handle: SimilarArrayHandle { shared },
        }
    }
    pub(crate) fn handle(&self) -> SimilarArrayHandle {
        self.handle.clone()
    }
}
impl Drop for SimilarArrayOwner {
    fn drop(&mut self) {
        self.handle.close();
    }
}

impl SimilarArrayHandle {
    pub(crate) fn observe(&self) -> ArrayObservation {
        self.with_observation(Clone::clone)
    }
    /// The closure may take the query-scope lock, in this order. It must do only short memory work.
    pub(crate) fn with_observation<T>(&self, read: impl FnOnce(&ArrayObservation) -> T) -> T {
        let result = {
            let state = self.shared.state.lock().unwrap_or_else(|e| e.into_inner());
            let observation = match &*state {
                ArrayOwnerState::Open(open) => match &open.view {
                    ArrayView::Uninitialized => ArrayObservation::Uninitialized {
                        busy: !matches!(open.work, ArrayWork::Idle),
                    },
                    ArrayView::Missing => ArrayObservation::Missing,
                    ArrayView::Published(value) => ArrayObservation::Published(value.clone()),
                },
                ArrayOwnerState::Faulted { published, failure } => ArrayObservation::Faulted {
                    published: published.clone(),
                    failure: failure.clone(),
                },
                ArrayOwnerState::Closing { .. } => ArrayObservation::Closing,
                ArrayOwnerState::Closed(terminal) => ArrayObservation::Closed(terminal.clone()),
            };
            read(&observation)
        };
        #[cfg(test)]
        self.shared.probe(OwnerPhase::AfterObservation);
        result
    }
    pub(crate) fn ensure(&self, demand: ArrayDemand) {
        self.shared.enqueue(demand);
    }
    pub(crate) fn refresh(&self) {
        self.ensure(ArrayDemand::RefreshIfPresent);
    }
    pub(crate) fn load_elapsed_us(&self) -> u64 {
        self.shared.elapsed_us.load(Ordering::Relaxed)
    }
    pub(crate) fn request_through(&self, goal: StoreWatermark) -> Receiver<ArrayAck> {
        let (reply, receiver) = mpsc::channel();
        let immediate = {
            let mut state = self.shared.state.lock().unwrap_or_else(|e| e.into_inner());
            // Close takes precedence even when its retained snapshot already reaches the goal.
            match &*state {
                ArrayOwnerState::Closing { .. }
                | ArrayOwnerState::Closed(OwnerTerminal::Stopped) => Some(ArrayAck::Stopped),
                ArrayOwnerState::Closed(OwnerTerminal::Failed(failure)) => {
                    Some(ArrayAck::Failed(failure.clone()))
                }
                _ if state.published().is_some_and(|value| value.reaches(goal)) => {
                    Some(ArrayAck::Published(state.published().unwrap().stamp()))
                }
                ArrayOwnerState::Faulted { failure, .. } => Some(ArrayAck::Failed(failure.clone())),
                ArrayOwnerState::Open(_) => None,
            }
            .or_else(|| {
                if let ArrayOwnerState::Open(open) = &mut *state {
                    open.acknowledgements.push(AckRequest {
                        id: self.shared.next_ack.fetch_add(1, Ordering::Relaxed),
                        goal,
                        reply: reply.clone(),
                    });
                    queue(open, ArrayDemand::RefreshOrCreate);
                }
                None
            })
        };
        if let Some(result) = immediate {
            self.shared.reply(reply, goal, result);
        }
        self.shared.wake.notify_one();
        receiver
    }
    pub(crate) fn close(&self) {
        let (old, replies) = {
            let mut state = self.shared.state.lock().unwrap_or_else(|e| e.into_inner());
            if matches!(
                *state,
                ArrayOwnerState::Closing { .. } | ArrayOwnerState::Closed(_)
            ) {
                return;
            }
            let mut old =
                std::mem::replace(&mut *state, ArrayOwnerState::Closing { published: None });
            // Transfer the owner's reference; the caller must not retire a published array.
            let published = match &mut old {
                ArrayOwnerState::Open(open) => {
                    match std::mem::replace(&mut open.view, ArrayView::Uninitialized) {
                        ArrayView::Published(published) => Some(published),
                        _ => None,
                    }
                }
                ArrayOwnerState::Faulted { published, .. } => published.take(),
                _ => unreachable!("closing and closed states returned above"),
            };
            *state = ArrayOwnerState::Closing { published };
            let replies = take_waiters(&mut old);
            (old, replies)
        };
        #[cfg(test)]
        self.shared.probe(OwnerPhase::AfterCloseTransition);
        self.shared.send_all(replies, ArrayAck::Stopped);
        drop(old);
        self.shared.wake.notify_all();
        (self.shared.notify)();
    }
    #[cfg(test)]
    pub(crate) fn wait_closed_for_test(&self) {
        let state = self.shared.state.lock().unwrap_or_else(|e| e.into_inner());
        let (state, timeout) = self
            .shared
            .wake
            .wait_timeout_while(state, std::time::Duration::from_secs(20), |state| {
                !matches!(state, ArrayOwnerState::Closed(_))
            })
            .unwrap();
        assert!(
            !timeout.timed_out() && matches!(*state, ArrayOwnerState::Closed(_)),
            "array owner did not close"
        );
    }
    #[cfg(test)]
    pub(crate) fn set_probe(&self, probe: Arc<dyn Fn(OwnerPhase) + Send + Sync>) {
        *self.shared.probe.lock().unwrap() = Some(probe);
    }
    #[cfg(test)]
    pub(crate) fn operation_count(&self) -> u64 {
        self.shared.operations.load(Ordering::Acquire)
    }
    #[cfg(test)]
    pub(crate) fn wait_idle_for_test(&self) {
        let state = self.shared.state.lock().unwrap();
        let (state, timeout) = self.shared.wake.wait_timeout_while(state, std::time::Duration::from_secs(20), |state| matches!(state, ArrayOwnerState::Open(open) if !matches!(open.work, ArrayWork::Idle))).unwrap();
        assert!(!timeout.timed_out(), "owner did not settle");
        assert!(
            matches!(*state, ArrayOwnerState::Open(_)),
            "owner faulted or closed"
        );
    }
    #[cfg(test)]
    pub(crate) fn fail_at_for_test(&self, phase: OwnerPhase) {
        *self.shared.injected_failure.lock().unwrap() = Some(phase);
    }
    #[cfg(test)]
    pub(crate) fn install_for_test(&self, observation: ArrayObservation) {
        let next = match observation {
            ArrayObservation::Published(value) => ArrayOwnerState::Open(OpenArray {
                view: ArrayView::Published(value),
                work: ArrayWork::Idle,
                acknowledgements: Vec::new(),
            }),
            ArrayObservation::Missing => ArrayOwnerState::Open(OpenArray {
                view: ArrayView::Missing,
                work: ArrayWork::Idle,
                acknowledgements: Vec::new(),
            }),
            ArrayObservation::Uninitialized { busy } => ArrayOwnerState::Open(OpenArray {
                view: ArrayView::Uninitialized,
                work: if busy {
                    ArrayWork::Executing {
                        operation: ArrayOperation::Demand(ArrayDemand::LoadIfPresent),
                        pending: PendingDemand::None,
                    }
                } else {
                    ArrayWork::Idle
                },
                acknowledgements: Vec::new(),
            }),
            ArrayObservation::Faulted { published, failure } => {
                ArrayOwnerState::Faulted { published, failure }
            }
            ArrayObservation::Closed(terminal) => ArrayOwnerState::Closed(terminal),
            _ => panic!("unsupported fixture state"),
        };
        let old = std::mem::replace(&mut *self.shared.state.lock().unwrap(), next);
        drop(old);
        (self.shared.notify)();
        self.shared.wake.notify_all();
    }
}

fn queue(open: &mut OpenArray, demand: ArrayDemand) {
    if !demand.refreshes() && matches!(open.view, ArrayView::Published(_)) {
        return;
    }
    // Repeated ensures join an executing load; create escalation must survive a Missing result.
    match &mut open.work {
        ArrayWork::Idle => {
            open.work = ArrayWork::Queued {
                operation: ArrayOperation::Demand(demand),
                pending: PendingDemand::None,
            }
        }
        ArrayWork::Queued {
            operation: ArrayOperation::Demand(current),
            ..
        } => *current = current.merge(demand),
        ArrayWork::Executing {
            operation: ArrayOperation::Demand(current),
            ..
        } if !demand.refreshes() && (!demand.creates() || current.creates()) => {}
        ArrayWork::Queued { pending, .. } | ArrayWork::Executing { pending, .. } => {
            pending.add(demand)
        }
    }
}
fn take_waiters(state: &mut ArrayOwnerState) -> Vec<AckRequest> {
    match state {
        ArrayOwnerState::Open(open) => std::mem::take(&mut open.acknowledgements),
        _ => Vec::new(),
    }
}

enum OperationResult {
    Missing,
    Ready {
        db: SimilarDb,
        snapshot: Arc<SearchSnapshot>,
        persistence: BasePersistence,
        read: Option<(StoreWatermark, u64)>,
        follow_refresh: bool,
        diagnostic: String,
    },
}

impl Shared {
    fn active_operation(&self) -> ArrayOperation {
        let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        match &*state {
            ArrayOwnerState::Open(open) => match open.work {
                ArrayWork::Executing { operation, .. } | ArrayWork::Queued { operation, .. } => {
                    operation
                }
                ArrayWork::Idle => ArrayOperation::Worker,
            },
            _ => ArrayOperation::Worker,
        }
    }
    fn enqueue(&self, demand: ArrayDemand) {
        {
            let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            if let ArrayOwnerState::Open(open) = &mut *state {
                queue(open, demand);
            }
        }
        self.wake.notify_one();
    }
    fn reply(&self, reply: Sender<ArrayAck>, goal: StoreWatermark, result: ArrayAck) {
        if !matches!(result, ArrayAck::Published(_)) {
            let observed = self
                .state
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .published()
                .map(PublishedArray::stamp);
            crate::logger::log(format!(
                "similar array ack: owner_id={} operation_id={} required={goal:?} observed={observed:?} terminal={result:?}",
                self.id,
                self.operation_id.load(Ordering::Relaxed)
            ));
        }
        let _ = reply.send(result);
    }
    fn send_all(&self, requests: Vec<AckRequest>, result: ArrayAck) {
        for request in requests {
            self.reply(request.reply, request.goal, result.clone());
        }
    }
    fn run(&self) {
        let access = BaseAccess(());
        let mut operation_id = 0u64;
        loop {
            let (operation, snapshot) = {
                let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
                loop {
                    match &mut *state {
                        ArrayOwnerState::Closing { .. } | ArrayOwnerState::Closed(_) => return,
                        ArrayOwnerState::Open(open) => {
                            if let ArrayWork::Queued { operation, pending } = open.work {
                                let snapshot = match &open.view {
                                    ArrayView::Published(value) => {
                                        Some(Arc::clone(&value.snapshot))
                                    }
                                    _ => None,
                                };
                                open.work = ArrayWork::Executing { operation, pending };
                                break (operation, snapshot);
                            }
                        }
                        ArrayOwnerState::Faulted { .. } => {}
                    }
                    state = self.wake.wait(state).unwrap_or_else(|e| e.into_inner());
                }
            };
            operation_id += 1;
            self.operation_id.store(operation_id, Ordering::Relaxed);
            #[cfg(test)]
            self.operations.fetch_add(1, Ordering::AcqRel);
            let started = Instant::now();
            crate::logger::log(format!(
                "similar array operation: owner_id={} operation_id={operation_id} operation={operation:?} terminal=Start",
                self.id
            ));
            let result = self.execute(&access, operation, snapshot);
            match result {
                Ok(result) => {
                    if let Err(detail) = self.complete(operation, result, started) {
                        self.fault(operation, detail);
                    }
                }
                Err(detail) => self.fault(operation, detail),
            }
            let observation = {
                let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
                match &*state {
                    ArrayOwnerState::Closing { .. } => "Stopped".to_owned(),
                    ArrayOwnerState::Faulted { published, failure } => format!(
                        "Failed observed={:?} detail={failure}",
                        published.as_ref().map(PublishedArray::stamp)
                    ),
                    ArrayOwnerState::Open(OpenArray {
                        view: ArrayView::Missing,
                        ..
                    }) => "Missing".to_owned(),
                    _ => format!(
                        "Completed observed={:?}",
                        state.published().map(PublishedArray::stamp)
                    ),
                }
            };
            crate::logger::log(format!(
                "similar array operation: owner_id={} operation_id={operation_id} operation={operation:?} terminal={observation} elapsed_ms={:.1}",
                self.id,
                started.elapsed().as_secs_f64() * 1000.0
            ));
        }
    }
    fn execute(
        &self,
        access: &BaseAccess,
        operation: ArrayOperation,
        snapshot: Option<Arc<SearchSnapshot>>,
    ) -> Result<OperationResult, String> {
        let db_path = SimilarDb::db_path_at(&self.data_dir);
        let path = array::base_path(&self.data_dir);
        if let ArrayOperation::Demand(demand) = operation {
            if !demand.creates() && !db_path.is_file() {
                return Ok(OperationResult::Missing);
            }
        }
        let db = SimilarDb::open_at(&db_path)
            .map_err(|error| format!("array DB open failed: {error}"))?;
        match (operation, snapshot) {
            (ArrayOperation::Compact, Some(snapshot)) => {
                let base = Arc::new(array::compacted_base(&snapshot));
                array::write_compacted_for_owner(access, &path, &base)?;
                let cutoff = StoreWatermark {
                    store_id: base.store_id,
                    through_change_seq: base.applied_seq,
                };
                let next = array::snapshot_on_new_base(&snapshot, base)
                    .map_err(|_| "compaction store/cutoff mismatch".to_owned())?;
                Ok(OperationResult::Ready {
                    db,
                    snapshot: Arc::new(next),
                    persistence: BasePersistence::Written(cutoff),
                    read: None,
                    follow_refresh: true,
                    diagnostic: "compaction".into(),
                })
            }
            (ArrayOperation::Demand(_), None) => {
                if let Err(error) = array::retire_legacy_sidecar(&self.data_dir) {
                    crate::logger::log(error);
                }
                let loaded = array::load_for_owner(access, &db, &path)?;
                let diagnostic = format!(
                    "memory load: source={:?} rows={} rebuild_reason={:?}",
                    loaded.source,
                    loaded.snapshot.record_count(),
                    loaded.rejected
                );
                Ok(OperationResult::Ready {
                    db,
                    snapshot: Arc::new(loaded.snapshot),
                    persistence: loaded.persistence,
                    read: None,
                    follow_refresh: true,
                    diagnostic,
                })
            }
            (ArrayOperation::Demand(_), Some(snapshot)) => {
                #[cfg(test)]
                self.checkpoint(OwnerPhase::BeforeRead)?;
                let ack_limit = {
                    let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
                    match &*state {
                        ArrayOwnerState::Open(open) => {
                            open.acknowledgements.last().map_or(0, |ack| ack.id)
                        }
                        _ => return Err("owner closing".into()),
                    }
                };
                let (observed, batch) = db
                    .load_array_changes(snapshot.applied_seq)
                    .map_err(|error| format!("array change read failed: {error}"))?;
                #[cfg(test)]
                self.probe(OwnerPhase::AfterRead);
                let next = if observed.store_id == snapshot.base.store_id {
                    array::apply_change_batch(&snapshot, batch)
                } else {
                    Err(array::MissingHistory)
                };
                let (next, persistence, rebuilt) = match next {
                    Ok(Some(next)) => (Arc::new(next), BasePersistence::Reused, false),
                    Ok(None) => (Arc::clone(&snapshot), BasePersistence::Reused, false),
                    Err(_) => {
                        let next = array::rebuild_for_owner(access, &db, &path)?;
                        let cutoff = StoreWatermark {
                            store_id: next.base.store_id,
                            through_change_seq: next.base.applied_seq,
                        };
                        (Arc::new(next), BasePersistence::Written(cutoff), true)
                    }
                };
                // A rebuild takes a new read transaction; its newer stamp is authoritative.
                let observed = if rebuilt {
                    StoreWatermark {
                        store_id: next.base.store_id,
                        through_change_seq: next.applied_seq,
                    }
                } else {
                    observed
                };
                let diagnostic = format!(
                    "array update: changes={} rebuilt={rebuilt}",
                    next.applied_seq.saturating_sub(snapshot.applied_seq)
                );
                Ok(OperationResult::Ready {
                    db,
                    snapshot: next,
                    persistence,
                    read: Some((observed, ack_limit)),
                    follow_refresh: false,
                    diagnostic,
                })
            }
            _ => Err("compaction without a published array".into()),
        }
    }
    fn complete(
        &self,
        operation: ArrayOperation,
        result: OperationResult,
        started: Instant,
    ) -> Result<(), String> {
        #[cfg(test)]
        self.checkpoint(OwnerPhase::BeforePublish)?;
        let mut replies = Vec::new();
        let old_view = {
            let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            let ArrayOwnerState::Open(open) = &mut *state else {
                return Ok(());
            };
            let (next_view, read) = match &result {
                OperationResult::Missing => (
                    (!matches!(open.view, ArrayView::Missing)).then_some(ArrayView::Missing),
                    None,
                ),
                OperationResult::Ready { snapshot, read, .. } => {
                    let current = match &open.view {
                        ArrayView::Published(current) => Some(current),
                        _ => None,
                    };
                    let changed =
                        current.is_none_or(|current| !Arc::ptr_eq(&current.snapshot, snapshot));
                    let next_view = changed.then(|| {
                        ArrayView::Published(PublishedArray {
                            snapshot: Arc::clone(snapshot),
                            revision: current.map_or(1, |value| value.revision + 1),
                        })
                    });
                    (next_view, *read)
                }
            };
            let old_view = next_view.map(|view| std::mem::replace(&mut open.view, view));
            let published = match &open.view {
                ArrayView::Published(value) => Some(value),
                _ => None,
            };
            let mut pending_acks = Vec::new();
            for request in std::mem::take(&mut open.acknowledgements) {
                let terminal = if let Some(published) =
                    published.filter(|value| value.reaches(request.goal))
                {
                    Some(ArrayAck::Published(published.stamp()))
                } else if let Some((observed, limit)) =
                    read.filter(|(_, limit)| request.id <= *limit)
                {
                    let _ = limit;
                    if request.goal.store_id != observed.store_id {
                        Some(ArrayAck::StoreChanged {
                            expected: request.goal,
                            observed,
                        })
                    } else if request.goal.through_change_seq > observed.through_change_seq {
                        Some(ArrayAck::Failed(ArrayFailure {
                            operation,
                            detail: format!(
                                "uncommitted array goal: required={:?} observed={observed:?}",
                                request.goal
                            ),
                        }))
                    } else {
                        None
                    }
                } else {
                    None
                };
                if let Some(terminal) = terminal {
                    replies.push((request, terminal));
                } else {
                    pending_acks.push(request);
                }
            }
            open.acknowledgements = pending_acks;
            old_view
        };
        if matches!(&result, OperationResult::Ready { diagnostic, .. } if diagnostic.starts_with("memory load"))
        {
            self.elapsed_us.store(
                u64::try_from(started.elapsed().as_micros()).unwrap_or(u64::MAX),
                Ordering::Relaxed,
            );
        }
        for (request, terminal) in replies {
            self.reply(request.reply, request.goal, terminal);
        }
        let changed = old_view.is_some();
        drop(old_view); // large old arrays are retired on the worker, outside the mutex.
        #[cfg(test)]
        self.probe(OwnerPhase::AfterPublish);
        if changed {
            (self.notify)();
        }
        if let OperationResult::Ready {
            db,
            snapshot,
            persistence,
            diagnostic,
            ..
        } = &result
        {
            if changed {
                crate::logger::log(format!(
                    "similar {diagnostic} adopted_seq={} elapsed_ms={:.1}",
                    snapshot.applied_seq,
                    started.elapsed().as_secs_f64() * 1000.0
                ));
            }
            if let BasePersistence::Written(cutoff) = persistence {
                #[cfg(test)]
                self.checkpoint(OwnerPhase::BeforePrune)?;
                // Close may omit not-yet-started pruning. Do not hold the state mutex during I/O.
                if matches!(
                    *self.state.lock().unwrap_or_else(|e| e.into_inner()),
                    ArrayOwnerState::Open(_)
                ) && !db
                    .prune_array_history(*cutoff)
                    .map_err(|error| format!("array history prune failed: {error}"))?
                {
                    return Err(format!(
                        "array history prune StoreChanged: cutoff={cutoff:?}"
                    ));
                }
            }
        }
        #[cfg(test)]
        self.probe(OwnerPhase::BeforeIdle);
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if let ArrayOwnerState::Open(open) = &mut *state {
            let ArrayWork::Executing {
                operation: active,
                mut pending,
            } = open.work
            else {
                unreachable!("worker owns the executing operation")
            };
            debug_assert_eq!(active, operation);
            if matches!(
                &result,
                OperationResult::Ready {
                    follow_refresh: true,
                    ..
                }
            ) {
                pending.add(ArrayDemand::RefreshIfPresent);
            }
            open.work = if !matches!(operation, ArrayOperation::Compact)
                && matches!(&open.view, ArrayView::Published(value) if value.snapshot.should_compact())
            {
                ArrayWork::Queued {
                    operation: ArrayOperation::Compact,
                    pending,
                }
            } else {
                match pending {
                    PendingDemand::None => ArrayWork::Idle,
                    PendingDemand::Some(demand) => ArrayWork::Queued {
                        operation: ArrayOperation::Demand(demand),
                        pending: PendingDemand::None,
                    },
                }
            };
        }
        drop(state);
        self.wake.notify_all();
        Ok(())
    }
    fn fault(&self, operation: ArrayOperation, detail: String) {
        let failure = ArrayFailure { operation, detail };
        let (old, requests) = {
            let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            if !matches!(*state, ArrayOwnerState::Open(_)) {
                return;
            }
            let published = state.published().cloned();
            let mut old = std::mem::replace(
                &mut *state,
                ArrayOwnerState::Faulted {
                    published,
                    failure: failure.clone(),
                },
            );
            let requests = take_waiters(&mut old);
            (old, requests)
        };
        drop(old);
        self.send_all(requests, ArrayAck::Failed(failure));
        (self.notify)();
        #[cfg(test)]
        self.probe(OwnerPhase::AfterFault);
    }
    fn close_worker(&self, terminal: OwnerTerminal) {
        crate::logger::log(format!(
            "similar array owner: owner_id={} operation_id={} terminal={terminal:?}",
            self.id,
            self.operation_id.load(Ordering::Relaxed)
        ));
        // Drop all arrays/requests before Closed: no worker-held DB, file or buffer can resume I/O.
        let mut old = {
            let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            std::mem::replace(&mut *state, ArrayOwnerState::Closing { published: None })
        };
        let requests = take_waiters(&mut old);
        drop(old);
        let result = match &terminal {
            OwnerTerminal::Stopped => ArrayAck::Stopped,
            OwnerTerminal::Failed(failure) => ArrayAck::Failed(failure.clone()),
        };
        self.send_all(requests, result);
        *self.state.lock().unwrap_or_else(|e| e.into_inner()) = ArrayOwnerState::Closed(terminal);
        self.wake.notify_all();
        (self.notify)();
    }
    #[cfg(test)]
    fn probe(&self, phase: OwnerPhase) {
        let probe = self.probe.lock().unwrap().clone();
        if let Some(probe) = probe {
            probe(phase);
        }
    }
    #[cfg(test)]
    fn checkpoint(&self, phase: OwnerPhase) -> Result<(), String> {
        self.probe(phase);
        if *self.injected_failure.lock().unwrap() == Some(phase) {
            Err(format!("injected array {phase:?} failure"))
        } else {
            Ok(())
        }
    }
}

#[cfg(test)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum OwnerPhase {
    AfterObservation,
    BeforeRead,
    AfterRead,
    BeforePublish,
    AfterPublish,
    BeforePrune,
    BeforeIdle,
    AfterFault,
    AfterCloseTransition,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn receive(receiver: Receiver<ArrayAck>) -> ArrayAck {
        receiver.recv_timeout(Duration::from_secs(20)).unwrap()
    }
    fn close(owner: &SimilarArrayOwner) {
        owner.handle.close();
        owner.handle.wait_closed_for_test();
    }
    fn put(db: &SimilarDb, key: &str) {
        db.upsert_loose_item(&crate::similar_db::StoredItem {
            item_key: key.into(),
            kind: crate::similar_db::ItemKind::Image,
            container_key: None,
            page_index: None,
            mtime: 10,
            file_size: 20,
            hash_version: crate::similar_db::current_hash_version(),
            pdq256: [7; 32],
            quality: 50,
            width: 100,
            height: 80,
            format: 1,
        })
        .unwrap();
    }
    #[test]
    fn demand_merge_is_commutative_and_preserves_both_strengths() {
        use ArrayDemand::*;
        for a in [
            LoadIfPresent,
            LoadOrCreate,
            RefreshIfPresent,
            RefreshOrCreate,
        ] {
            for b in [
                LoadIfPresent,
                LoadOrCreate,
                RefreshIfPresent,
                RefreshOrCreate,
            ] {
                assert_eq!(a.merge(b), b.merge(a));
                assert_eq!(a.merge(b).creates(), a.creates() || b.creates());
                assert_eq!(a.merge(b).refreshes(), a.refreshes() || b.refreshes());
            }
        }
    }
    #[test]
    fn idle_constructor_performs_no_io_and_close_rejects_late_ack() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("absent");
        let owner = SimilarArrayOwner::new(path.clone(), || {});
        assert!(!path.exists());
        close(&owner);
        let goal = StoreWatermark {
            store_id: [1; 16],
            through_change_seq: 0,
        };
        assert_eq!(
            receive(owner.handle.request_through(goal)),
            ArrayAck::Stopped
        );
        owner.handle.ensure(ArrayDemand::LoadOrCreate);
        assert!(!path.exists());
        assert_eq!(owner.handle.operation_count(), 0);
    }
    #[test]
    fn missing_noncreate_load_retains_late_create_and_ack() {
        let temp = tempfile::tempdir().unwrap();
        let owner = SimilarArrayOwner::new(temp.path().to_path_buf(), || {});
        let (entered, entered_rx) = mpsc::channel();
        let (resume, resume_rx) = mpsc::channel();
        let resume_rx = Mutex::new(resume_rx);
        let once = std::sync::atomic::AtomicBool::new(false);
        owner.handle.set_probe(Arc::new(move |phase| {
            if phase == OwnerPhase::BeforePublish && !once.swap(true, Ordering::AcqRel) {
                entered.send(()).unwrap();
                resume_rx
                    .lock()
                    .unwrap()
                    .recv_timeout(Duration::from_secs(20))
                    .unwrap();
            }
        }));
        owner.handle.ensure(ArrayDemand::LoadIfPresent);
        entered_rx.recv_timeout(Duration::from_secs(20)).unwrap();
        assert!(!SimilarDb::db_path_at(temp.path()).exists());
        let db = SimilarDb::open_at(&SimilarDb::db_path_at(temp.path())).unwrap();
        put(&db, "c:/a.jpg");
        let goal = db.change_watermark().unwrap();
        owner.handle.ensure(ArrayDemand::LoadOrCreate);
        let ack = owner.handle.request_through(goal);
        resume.send(()).unwrap();
        assert!(matches!(receive(ack), ArrayAck::Published(value) if value == goal));
        close(&owner);
    }
    #[test]
    fn publication_ack_is_atomic_and_invalid_goals_do_not_fault_owner() {
        let temp = tempfile::tempdir().unwrap();
        let db = SimilarDb::open_at(&SimilarDb::db_path_at(temp.path())).unwrap();
        put(&db, "c:/a.jpg");
        let goal = db.change_watermark().unwrap();
        let owner = SimilarArrayOwner::new(temp.path().to_path_buf(), || {});
        assert_eq!(
            receive(owner.handle.request_through(goal)),
            ArrayAck::Published(goal)
        );
        let wrong = StoreWatermark {
            store_id: [0; 16],
            ..goal
        };
        assert!(
            matches!(receive(owner.handle.request_through(wrong)), ArrayAck::StoreChanged { expected, observed } if expected == wrong && observed == goal)
        );
        let future = StoreWatermark {
            through_change_seq: goal.through_change_seq + 1,
            ..goal
        };
        assert!(matches!(
            receive(owner.handle.request_through(future)),
            ArrayAck::Failed(_)
        ));
        assert!(matches!(
            owner.handle.observe(),
            ArrayObservation::Published(_)
        ));
        assert_eq!(
            receive(owner.handle.request_through(goal)),
            ArrayAck::Published(goal)
        );
        close(&owner);
    }

    fn pause_once(handle: &SimilarArrayHandle, target: OwnerPhase) -> (Receiver<()>, Sender<()>) {
        let (entered, entered_rx) = mpsc::channel();
        let (resume, resume_rx) = mpsc::channel();
        let resume_rx = Mutex::new(resume_rx);
        let once = std::sync::atomic::AtomicBool::new(false);
        handle.set_probe(Arc::new(move |phase| {
            if phase == target && !once.swap(true, Ordering::AcqRel) {
                entered.send(()).unwrap();
                resume_rx
                    .lock()
                    .unwrap()
                    .recv_timeout(Duration::from_secs(20))
                    .expect("release owner barrier");
            }
        }));
        (entered_rx, resume)
    }
    fn entered(receiver: &Receiver<()>) {
        receiver.recv_timeout(Duration::from_secs(20)).unwrap();
    }
    fn wait_failure(handle: &SimilarArrayHandle, notifications: &Receiver<()>) -> ArrayFailure {
        loop {
            if let Some(failure) = handle.observe().failure() {
                return failure.clone();
            }
            entered(notifications);
        }
    }
    fn initialized(temp: &tempfile::TempDir) -> (SimilarDb, SimilarArrayOwner) {
        let db = SimilarDb::open_at(&SimilarDb::db_path_at(temp.path())).unwrap();
        put(&db, "c:/initial.jpg");
        let owner = SimilarArrayOwner::new(temp.path().to_path_buf(), || {});
        assert_eq!(
            receive(owner.handle.request_through(db.change_watermark().unwrap())),
            ArrayAck::Published(db.change_watermark().unwrap())
        );
        owner.handle.wait_idle_for_test();
        (db, owner)
    }

    #[test]
    fn refresh_and_ack_arriving_across_read_publication_and_idle_are_not_lost() {
        for phase in [
            OwnerPhase::BeforeRead,
            OwnerPhase::AfterRead,
            OwnerPhase::BeforePublish,
            OwnerPhase::AfterPublish,
            OwnerPhase::BeforeIdle,
        ] {
            let temp = tempfile::tempdir().unwrap();
            let (db, owner) = initialized(&temp);
            put(&db, "c:/first.jpg");
            let first = db.change_watermark().unwrap();
            let (blocked, release) = pause_once(&owner.handle, phase);
            let first_ack = owner.handle.request_through(first);
            entered(&blocked);
            put(&db, "c:/late.jpg");
            let late = db.change_watermark().unwrap();
            owner.handle.refresh();
            let late_ack = owner.handle.request_through(late);
            release.send(()).unwrap();
            assert!(
                matches!(receive(first_ack), ArrayAck::Published(stamp) if stamp.through_change_seq >= first.through_change_seq)
            );
            assert_eq!(receive(late_ack), ArrayAck::Published(late), "{phase:?}");
            owner.handle.wait_idle_for_test();
            assert_eq!(owner.handle.observe().published().unwrap().stamp(), late);
            close(&owner);
        }
    }

    #[test]
    fn empty_refresh_and_repeated_ensure_preserve_revision_and_do_not_notify_readers() {
        let temp = tempfile::tempdir().unwrap();
        let db = SimilarDb::open_at(&SimilarDb::db_path_at(temp.path())).unwrap();
        let notifications = Arc::new(AtomicU64::new(0));
        let observed = Arc::clone(&notifications);
        let owner = SimilarArrayOwner::new(temp.path().to_path_buf(), move || {
            observed.fetch_add(1, Ordering::AcqRel);
        });
        receive(owner.handle.request_through(db.change_watermark().unwrap()));
        owner.handle.wait_idle_for_test();
        let before = owner.handle.observe().published().unwrap().clone();
        let notified = notifications.load(Ordering::Acquire);
        for _ in 0..8 {
            owner.handle.refresh();
            owner.handle.ensure(ArrayDemand::LoadOrCreate);
        }
        owner.handle.wait_idle_for_test();
        let after = owner.handle.observe().published().unwrap().clone();
        assert_eq!(before.revision, after.revision);
        assert!(Arc::ptr_eq(&before.snapshot, &after.snapshot));
        assert_eq!(
            notifications.load(Ordering::Acquire),
            notified,
            "no extra book soft refresh"
        );
        let operations = owner.handle.operation_count();
        for _ in 0..8 {
            owner.handle.ensure(ArrayDemand::LoadIfPresent);
        }
        owner.handle.wait_idle_for_test();
        assert_eq!(
            owner.handle.operation_count(),
            operations,
            "idle ensures do not queue work"
        );
        close(&owner);
    }

    #[test]
    fn initial_replace_then_adopt_then_prune_and_close_can_keep_unpruned_history() {
        let temp = tempfile::tempdir().unwrap();
        let db = SimilarDb::open_at(&SimilarDb::db_path_at(temp.path())).unwrap();
        put(&db, "c:/initial.jpg");
        let goal = db.change_watermark().unwrap();
        let owner = SimilarArrayOwner::new(temp.path().to_path_buf(), || {});
        let (blocked, release) = pause_once(&owner.handle, OwnerPhase::BeforePrune);
        let ack = owner.handle.request_through(goal);
        entered(&blocked);
        assert!(
            array::base_path(temp.path()).is_file(),
            "disk replace has completed"
        );
        assert_eq!(
            owner.handle.observe().published().unwrap().stamp(),
            goal,
            "memory adopted before prune"
        );
        assert_eq!(receive(ack), ArrayAck::Published(goal));
        assert_eq!(
            db.load_item_changes_after(0).unwrap().changes.len(),
            1,
            "not pruned yet"
        );
        owner.handle.close();
        assert_eq!(
            receive(owner.handle.request_through(goal)),
            ArrayAck::Stopped
        );
        release.send(()).unwrap();
        owner.handle.wait_closed_for_test();
        assert_eq!(
            db.load_item_changes_after(0).unwrap().changes.len(),
            1,
            "close omitted unstarted prune"
        );
    }

    #[test]
    fn compaction_is_serial_and_retains_commits_after_the_persisted_cutoff() {
        let temp = tempfile::tempdir().unwrap();
        let (db, owner) = initialized(&temp);
        let old = owner.handle.observe().published().unwrap().clone();
        for n in 0..1024 {
            put(&db, &format!("c:/change-{n}.jpg"));
        }
        let cutoff = db.change_watermark().unwrap();
        let (blocked, blocked_rx) = mpsc::channel();
        let (resume, resume_rx) = mpsc::channel();
        let resume_rx = Mutex::new(resume_rx);
        let writes = Arc::new(AtomicU64::new(0));
        let count = Arc::clone(&writes);
        let _guard = array::startup_investigation_probe::install(
            array::base_path(temp.path()),
            Arc::new(move |phase| {
                if phase == array::startup_investigation_probe::Phase::BeforeReplace {
                    assert_eq!(
                        count.fetch_add(1, Ordering::AcqRel),
                        0,
                        "one compaction writer"
                    );
                    blocked.send(()).unwrap();
                    resume_rx
                        .lock()
                        .unwrap()
                        .recv_timeout(Duration::from_secs(20))
                        .expect("release compaction barrier");
                }
            }),
        );
        assert_eq!(
            receive(owner.handle.request_through(cutoff)),
            ArrayAck::Published(cutoff)
        );
        entered(&blocked_rx);
        // Already-published goals do not wait for compaction.
        assert_eq!(
            receive(owner.handle.request_through(cutoff)),
            ArrayAck::Published(cutoff)
        );
        put(&db, "c:/after-cutoff.jpg");
        let late = db.change_watermark().unwrap();
        let started = Instant::now();
        let ack = owner.handle.request_through(late);
        for _ in 0..32 {
            owner.handle.refresh();
        }
        assert!(matches!(ack.try_recv(), Err(mpsc::TryRecvError::Empty)));
        resume.send(()).unwrap();
        assert_eq!(receive(ack), ArrayAck::Published(late));
        eprintln!(
            "owner_compaction_new_ack_wait_ms={:.3}",
            started.elapsed().as_secs_f64() * 1000.0
        );
        owner.handle.wait_idle_for_test();
        let current = owner.handle.observe().published().unwrap().clone();
        assert_eq!(current.snapshot.base.applied_seq, cutoff.through_change_seq);
        assert_eq!(current.snapshot.delta.len(), 1);
        assert!(!Arc::ptr_eq(&current.snapshot.base, &old.snapshot.base));
        let history = db.load_item_changes_after(0).unwrap();
        assert_eq!(history.changes.len(), 1);
        assert_eq!(history.changes[0].seq, late.through_change_seq);
        assert_eq!(writes.load(Ordering::Acquire), 1);
        close(&owner);
    }

    #[test]
    fn missing_history_rebuild_keeps_late_refresh_and_prunes_only_its_base() {
        let temp = tempfile::tempdir().unwrap();
        let (db, owner) = initialized(&temp);
        put(&db, "c:/missing.jpg");
        let cutoff = db.change_watermark().unwrap();
        db.prune_item_changes_through(cutoff.through_change_seq)
            .unwrap();
        let (blocked, release) = pause_once(&owner.handle, OwnerPhase::BeforePublish);
        owner.handle.refresh();
        entered(&blocked);
        put(&db, "c:/late.jpg");
        let late = db.change_watermark().unwrap();
        let ack = owner.handle.request_through(late);
        owner.handle.refresh();
        release.send(()).unwrap();
        assert_eq!(receive(ack), ArrayAck::Published(late));
        owner.handle.wait_idle_for_test();
        let published = owner.handle.observe().published().unwrap().clone();
        assert_eq!(
            published.snapshot.base.applied_seq,
            cutoff.through_change_seq
        );
        assert_eq!(published.snapshot.record_count(), 3);
        assert_eq!(db.load_item_changes_after(0).unwrap().changes.len(), 1);
        close(&owner);
    }

    #[test]
    fn read_or_prune_failure_is_terminal_keeps_data_and_only_a_new_owner_recovers() {
        for phase in [OwnerPhase::BeforeRead, OwnerPhase::BeforePrune] {
            let temp = tempfile::tempdir().unwrap();
            let db = SimilarDb::open_at(&SimilarDb::db_path_at(temp.path())).unwrap();
            put(&db, "c:/initial.jpg");
            let goal = db.change_watermark().unwrap();
            let (notify, notifications) = mpsc::channel();
            let owner = SimilarArrayOwner::new(temp.path().to_path_buf(), move || {
                let _ = notify.send(());
            });
            if phase == OwnerPhase::BeforeRead {
                receive(owner.handle.request_through(goal));
                owner.handle.wait_idle_for_test();
                put(&db, "c:/late.jpg");
            }
            owner.handle.fail_at_for_test(phase);
            let current = db.change_watermark().unwrap();
            let ack = receive(owner.handle.request_through(current));
            let failure = wait_failure(&owner.handle, &notifications);
            if phase == OwnerPhase::BeforeRead {
                assert_eq!(ack, ArrayAck::Failed(failure.clone()));
            } else {
                assert_eq!(ack, ArrayAck::Published(current));
            }
            let published = owner.handle.observe().published().unwrap().clone();
            assert_eq!(
                receive(owner.handle.request_through(published.stamp())),
                ArrayAck::Published(published.stamp())
            );
            let unreachable = StoreWatermark {
                through_change_seq: current.through_change_seq + 1,
                ..current
            };
            let operations = owner.handle.operation_count();
            for _ in 0..8 {
                owner.handle.ensure(ArrayDemand::LoadOrCreate);
                owner.handle.refresh();
                assert_eq!(
                    receive(owner.handle.request_through(unreachable)),
                    ArrayAck::Failed(failure.clone())
                );
            }
            assert_eq!(owner.handle.operation_count(), operations);
            assert_eq!(
                db.load_base_search_rows(crate::similar_db::current_hash_version())
                    .unwrap()
                    .records
                    .len(),
                if phase == OwnerPhase::BeforeRead {
                    2
                } else {
                    1
                }
            );
            assert!(!db.load_item_changes_after(0).unwrap().changes.is_empty());
            close(&owner);
            let reopened = SimilarArrayOwner::new(temp.path().to_path_buf(), || {});
            assert_eq!(
                receive(reopened.handle.request_through(current)),
                ArrayAck::Published(current)
            );
            close(&reopened);
        }
    }

    #[test]
    fn failed_replace_and_worker_panic_notify_waiters_without_retries() {
        for panic_worker in [false, true] {
            let temp = tempfile::tempdir().unwrap();
            let db = SimilarDb::open_at(&SimilarDb::db_path_at(temp.path())).unwrap();
            put(&db, "c:/initial.jpg");
            let goal = db.change_watermark().unwrap();
            if !panic_worker {
                std::fs::create_dir(array::base_path(temp.path())).unwrap();
            }
            let (notify, notifications) = mpsc::channel();
            let owner = SimilarArrayOwner::new(temp.path().to_path_buf(), move || {
                let _ = notify.send(());
            });
            if panic_worker {
                owner.handle.set_probe(Arc::new(|phase| {
                    if phase == OwnerPhase::BeforePublish {
                        panic!("injected owner exit");
                    }
                }));
            }
            assert!(matches!(
                receive(owner.handle.request_through(goal)),
                ArrayAck::Failed(_)
            ));
            let failure = wait_failure(&owner.handle, &notifications);
            assert!(failure.detail.contains(if panic_worker {
                "panicked"
            } else {
                "base publish failed"
            }));
            assert_eq!(db.load_item_changes_after(0).unwrap().changes.len(), 1);
            let operations = owner.handle.operation_count();
            owner.handle.refresh();
            owner.handle.ensure(ArrayDemand::LoadOrCreate);
            assert!(matches!(
                receive(owner.handle.request_through(goal)),
                ArrayAck::Failed(_)
            ));
            assert_eq!(owner.handle.operation_count(), operations);
            close(&owner);
        }
    }

    #[test]
    fn shutdown_during_load_refresh_or_rebuild_never_adopts_or_prunes_after_closed() {
        for initial in [false, true] {
            for phase in [
                OwnerPhase::BeforeRead,
                OwnerPhase::AfterRead,
                OwnerPhase::BeforePublish,
            ] {
                if initial && phase != OwnerPhase::BeforePublish {
                    continue;
                }
                let temp = tempfile::tempdir().unwrap();
                let db = SimilarDb::open_at(&SimilarDb::db_path_at(temp.path())).unwrap();
                put(&db, "c:/initial.jpg");
                let owner = SimilarArrayOwner::new(temp.path().to_path_buf(), || {});
                if !initial {
                    receive(owner.handle.request_through(db.change_watermark().unwrap()));
                    owner.handle.wait_idle_for_test();
                    put(&db, "c:/late.jpg");
                }
                let (blocked, release) = pause_once(&owner.handle, phase);
                let goal = db.change_watermark().unwrap();
                let ack = owner.handle.request_through(goal);
                entered(&blocked);
                owner.handle.close();
                assert_eq!(receive(ack), ArrayAck::Stopped);
                assert_eq!(
                    receive(owner.handle.request_through(goal)),
                    ArrayAck::Stopped
                );
                release.send(()).unwrap();
                owner.handle.wait_closed_for_test();
                let operations = owner.handle.operation_count();
                owner.handle.refresh();
                owner.handle.ensure(ArrayDemand::LoadOrCreate);
                assert_eq!(owner.handle.operation_count(), operations);
                assert!(matches!(
                    owner.handle.observe(),
                    ArrayObservation::Closed(OwnerTerminal::Stopped)
                ));
                assert!(!db.load_item_changes_after(0).unwrap().changes.is_empty());
                let reopened = SimilarArrayOwner::new(temp.path().to_path_buf(), || {});
                assert_eq!(
                    receive(reopened.handle.request_through(goal)),
                    ArrayAck::Published(goal)
                );
                close(&reopened);
            }
        }
    }

    #[test]
    fn failure_and_reopen_of_one_store_leave_sibling_publication_ack_and_history_unchanged() {
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        let (db_b, owner_b) = initialized(&b);
        let before = owner_b.handle.observe().published().unwrap().clone();
        let base_bytes = std::fs::read(array::base_path(b.path())).unwrap();
        let changes = db_b.load_item_changes_after(0).unwrap();
        let db_a = SimilarDb::open_at(&SimilarDb::db_path_at(a.path())).unwrap();
        put(&db_a, "c:/a.jpg");
        let owner_a = SimilarArrayOwner::new(a.path().to_path_buf(), || {});
        owner_a.handle.fail_at_for_test(OwnerPhase::BeforePublish);
        assert!(matches!(
            receive(
                owner_a
                    .handle
                    .request_through(db_a.change_watermark().unwrap())
            ),
            ArrayAck::Failed(_)
        ));
        close(&owner_a);
        let reopened = SimilarArrayOwner::new(a.path().to_path_buf(), || {});
        assert!(matches!(
            receive(
                reopened
                    .handle
                    .request_through(db_a.change_watermark().unwrap())
            ),
            ArrayAck::Published(_)
        ));
        close(&reopened);
        let after = owner_b.handle.observe().published().unwrap().clone();
        assert_eq!(before.revision, after.revision);
        assert!(Arc::ptr_eq(&before.snapshot, &after.snapshot));
        assert_eq!(
            receive(owner_b.handle.request_through(before.stamp())),
            ArrayAck::Published(before.stamp())
        );
        assert_eq!(
            std::fs::read(array::base_path(b.path())).unwrap(),
            base_bytes
        );
        assert_eq!(db_b.load_item_changes_after(0).unwrap(), changes);
        close(&owner_b);
    }

    #[test]
    fn close_during_rebuild_or_compaction_replace_keeps_history_and_reopens_cleanly() {
        for compact in [false, true] {
            let temp = tempfile::tempdir().unwrap();
            let (db, owner) = initialized(&temp);
            let before = owner.handle.observe().published().unwrap().clone();
            for n in 0..if compact { 1024 } else { 1 } {
                put(&db, &format!("c:/change-{n}.jpg"));
            }
            if !compact {
                db.prune_item_changes_through(db.change_watermark().unwrap().through_change_seq)
                    .unwrap();
                put(&db, "c:/after-gap.jpg");
            }
            let goal = db.change_watermark().unwrap();
            let history = db.load_item_changes_after(0).unwrap();
            let (blocked, blocked_rx) = mpsc::channel();
            let (resume, resume_rx) = mpsc::channel();
            let resume_rx = Mutex::new(resume_rx);
            let replacements = Arc::new(AtomicU64::new(0));
            let count = Arc::clone(&replacements);
            let _guard = array::startup_investigation_probe::install(
                array::base_path(temp.path()),
                Arc::new(move |phase| {
                    if phase == array::startup_investigation_probe::Phase::BeforeReplace {
                        assert_eq!(count.fetch_add(1, Ordering::AcqRel), 0);
                        blocked.send(()).unwrap();
                        resume_rx
                            .lock()
                            .unwrap()
                            .recv_timeout(Duration::from_secs(20))
                            .expect("release closing base writer");
                    }
                }),
            );
            let ack = owner.handle.request_through(goal);
            entered(&blocked_rx);
            let published = owner.handle.observe().published().unwrap().clone();
            if compact {
                assert_eq!(receive(ack), ArrayAck::Published(goal));
            } else {
                assert_eq!(published.revision, before.revision);
                drop(ack);
            }
            owner.handle.close();
            assert_eq!(
                receive(owner.handle.request_through(goal)),
                ArrayAck::Stopped
            );
            resume.send(()).unwrap();
            owner.handle.wait_closed_for_test();
            assert_eq!(
                db.load_item_changes_after(0).unwrap(),
                history,
                "no prune after close"
            );
            assert_eq!(replacements.load(Ordering::Acquire), 1);
            // No later old-worker operation can touch this path after Closed.
            drop(_guard);
            let reopened = SimilarArrayOwner::new(temp.path().to_path_buf(), || {});
            assert_eq!(
                receive(reopened.handle.request_through(goal)),
                ArrayAck::Published(goal)
            );
            reopened.handle.wait_idle_for_test();
            assert_eq!(
                reopened
                    .handle
                    .observe()
                    .published()
                    .unwrap()
                    .snapshot
                    .base
                    .applied_seq,
                goal.through_change_seq
            );
            close(&reopened);
        }
    }
}
