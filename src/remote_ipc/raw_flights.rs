//! Remote-only RAW development shared by pages and AI jobs.
//!
//! The map entry describes who can still use a result. `outstanding` describes
//! executor work, including a cancelled job after its key has been replaced.
//! These are deliberately separate accounting domains.

use crate::raw::{
    RawBrightness, RawDevelopExecutor, RawDevelopOutput, RawDevelopScale, RawError, RawOwnedSource,
    RawPriority, RawTicket,
};
use std::collections::{HashMap, VecDeque};
use std::fs::Metadata;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

pub(super) const REMOTE_RAW_FLIGHT_LIMIT: usize = 6;
const CANCEL_POLL: Duration = Duration::from_millis(50);

pub(super) type DevelopedRaw = RawDevelopOutput;
type DevelopResult = Result<Arc<DevelopedRaw>, RawError>;
type Completion = Box<dyn FnOnce(Result<RawDevelopOutput, RawError>) + Send + 'static>;

/// Exact source identity. Neither output quality nor requested pixel size
/// changes the full-size LibRaw raster.
#[derive(Clone, Debug, Hash, PartialEq, Eq)]
pub(super) struct RemoteRawIdentity {
    pub(super) normalized_path: String,
    pub(super) mtime_100ns: i64,
    pub(super) file_size: u64,
    pub(super) zip_entry: Option<String>,
    pub(super) zip_dir_prefix: Option<String>,
    pub(super) brightness: RawBrightness,
}

impl RemoteRawIdentity {
    pub(super) fn new(
        canonical_path: &Path,
        metadata: &Metadata,
        zip_entry: Option<&str>,
        zip_dir_prefix: Option<&str>,
        brightness: RawBrightness,
    ) -> Self {
        Self {
            normalized_path: crate::adjustment_db::normalize_path(canonical_path),
            mtime_100ns: remote_raw_mtime_100ns(metadata),
            file_size: metadata.len(),
            zip_entry: zip_entry.map(str::to_owned),
            zip_dir_prefix: zip_dir_prefix.map(str::to_owned),
            brightness,
        }
    }
}

/// Windows metadata exposes the unrounded FILETIME used by the source key.
#[cfg(windows)]
pub(super) fn remote_raw_mtime_100ns(metadata: &Metadata) -> i64 {
    use std::os::windows::fs::MetadataExt;
    i64::try_from(metadata.last_write_time()).unwrap_or(i64::MAX)
}

#[cfg(not(windows))]
pub(super) fn remote_raw_mtime_100ns(metadata: &Metadata) -> i64 {
    const UNIX_EPOCH_IN_FILETIME_TICKS: i128 = 116_444_736_000_000_000;
    let nanos = metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |duration| duration.as_nanos() as i128);
    i64::try_from(UNIX_EPOCH_IN_FILETIME_TICKS + nanos / 100).unwrap_or(i64::MAX)
}

#[derive(Clone, Copy, Debug)]
pub(super) struct RemoteRawFlightPolicy {
    pub(super) submit_priority: RawPriority,
    pub(super) max_outstanding: usize,
    pub(super) max_cached_entries: usize,
    pub(super) max_cached_bytes: usize,
}

impl RemoteRawFlightPolicy {
    pub(super) fn s2b() -> Self {
        Self {
            submit_priority: RawPriority::High,
            max_outstanding: REMOTE_RAW_FLIGHT_LIMIT,
            max_cached_entries: 1,
            max_cached_bytes: usize::MAX,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum RemoteRawFlightError {
    Raw(RawError),
    Capacity,
    Cancelled,
}

trait FlightTicket: Send + Sync {
    fn cancel(&self);
    fn promote_to_high(&self);
}

impl FlightTicket for RawTicket {
    fn cancel(&self) {
        RawTicket::cancel(self);
    }

    fn promote_to_high(&self) {
        RawTicket::promote_to_high(self);
    }
}

trait FlightSubmitter: Send + Sync {
    fn submit(
        &self,
        source: RawOwnedSource,
        brightness: RawBrightness,
        priority: RawPriority,
        complete: Completion,
    ) -> Result<Arc<dyn FlightTicket>, RawError>;
}

impl FlightSubmitter for RawDevelopExecutor {
    fn submit(
        &self,
        source: RawOwnedSource,
        brightness: RawBrightness,
        priority: RawPriority,
        complete: Completion,
    ) -> Result<Arc<dyn FlightTicket>, RawError> {
        self.submit_with_completion(
            source,
            RawDevelopScale::Full,
            brightness,
            priority,
            complete,
        )
        .map(|ticket| Arc::new(ticket) as Arc<dyn FlightTicket>)
    }
}

enum FlightState {
    Submitting {
        id: u64,
        waiters: usize,
        cancel_requested: bool,
    },
    InFlight {
        id: u64,
        ticket: Arc<dyn FlightTicket>,
        waiters: usize,
    },
    Cancelling {
        id: u64,
    },
    Done {
        id: u64,
        result: DevelopResult,
        waiters: usize,
    },
}

impl FlightState {
    fn id(&self) -> u64 {
        match self {
            Self::Submitting { id, .. }
            | Self::InFlight { id, .. }
            | Self::Cancelling { id }
            | Self::Done { id, .. } => *id,
        }
    }
}

struct CacheEntry {
    identity: RemoteRawIdentity,
    raster: Arc<DevelopedRaw>,
    bytes: usize,
}

struct FlightWork {
    _identity: Option<RemoteRawIdentity>,
    source_cancel: Arc<AtomicBool>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum OwnerLifecycle {
    Running,
    Stopped,
}

struct State {
    lifecycle: OwnerLifecycle,
    next_id: u64,
    entries: HashMap<RemoteRawIdentity, FlightState>,
    outstanding: HashMap<u64, FlightWork>,
    cache: VecDeque<CacheEntry>,
    cached_bytes: usize,
    next_ai_wait_id: u64,
    ai_capacity_waiters: HashMap<String, u64>,
    ai_latest_request: HashMap<String, u64>,
}

impl State {
    fn cache_lookup(&mut self, identity: &RemoteRawIdentity) -> Option<Arc<DevelopedRaw>> {
        let index = self
            .cache
            .iter()
            .position(|entry| &entry.identity == identity)?;
        let entry = self.cache.remove(index).expect("cache index exists");
        let result = Arc::clone(&entry.raster);
        self.cache.push_front(entry);
        Some(result)
    }

    fn cache_insert(
        &mut self,
        identity: RemoteRawIdentity,
        raster: Arc<DevelopedRaw>,
        policy: RemoteRawFlightPolicy,
    ) {
        if policy.max_cached_entries == 0 {
            return;
        }
        let bytes = raster.image.as_bytes().len();
        if bytes > policy.max_cached_bytes {
            return;
        }
        if let Some(index) = self
            .cache
            .iter()
            .position(|entry| entry.identity == identity)
        {
            let replaced = self.cache.remove(index).expect("cache index exists");
            self.cached_bytes -= replaced.bytes;
        }
        self.cached_bytes = self.cached_bytes.saturating_add(bytes);
        self.cache.push_front(CacheEntry {
            identity,
            raster,
            bytes,
        });
        while self.cache.len() > policy.max_cached_entries
            || self.cached_bytes > policy.max_cached_bytes
        {
            let removed = self.cache.pop_back().expect("cache exceeds policy");
            self.cached_bytes -= removed.bytes;
        }
    }

    fn reserve(&mut self, identity: &RemoteRawIdentity) -> u64 {
        let id = self.next_id;
        self.next_id = self
            .next_id
            .checked_add(1)
            .expect("RAW flight id exhausted");
        self.entries.insert(
            identity.clone(),
            FlightState::Submitting {
                id,
                waiters: 1,
                cancel_requested: false,
            },
        );
        self.outstanding.insert(
            id,
            FlightWork {
                _identity: Some(identity.clone()),
                source_cancel: Arc::new(AtomicBool::new(false)),
            },
        );
        id
    }

    fn reserve_probe(&mut self) -> (u64, Arc<AtomicBool>) {
        let id = self.next_id;
        self.next_id = self
            .next_id
            .checked_add(1)
            .expect("RAW flight id exhausted");
        let source_cancel = Arc::new(AtomicBool::new(false));
        self.outstanding.insert(
            id,
            FlightWork {
                _identity: None,
                source_cancel: Arc::clone(&source_cancel),
            },
        );
        (id, source_cancel)
    }
}

pub(super) struct RemoteRawFlights {
    submitter: Arc<dyn FlightSubmitter>,
    policy: RemoteRawFlightPolicy,
    state: Mutex<State>,
    changed: Condvar,
}

enum Join {
    Ready(DevelopResult),
    Participant { id: u64, submit: bool },
    Capacity,
}

struct ParticipantLease {
    flights: Arc<RemoteRawFlights>,
    identity: RemoteRawIdentity,
    id: u64,
}

impl Drop for ParticipantLease {
    fn drop(&mut self) {
        self.flights.leave(&self.identity, self.id);
    }
}

struct AiCapacityLease {
    flights: Arc<RemoteRawFlights>,
    owner: String,
    id: u64,
}

struct AiRequestLease {
    flights: Arc<RemoteRawFlights>,
    owner: String,
    id: u64,
}

#[derive(Clone, Copy)]
pub(super) enum RawProbeDemand<'a> {
    Page,
    Ai { owner: &'a str },
}

struct CapacityProbeLease {
    flights: Arc<RemoteRawFlights>,
    id: u64,
}

impl Drop for CapacityProbeLease {
    fn drop(&mut self) {
        let mut state = self.flights.state.lock().unwrap_or_else(|e| e.into_inner());
        state.outstanding.remove(&self.id);
        self.flights.changed.notify_all();
    }
}

impl Drop for AiRequestLease {
    fn drop(&mut self) {
        let mut state = self.flights.state.lock().unwrap_or_else(|e| e.into_inner());
        if state.ai_latest_request.get(&self.owner) == Some(&self.id) {
            state.ai_latest_request.remove(&self.owner);
        }
    }
}

impl Drop for AiCapacityLease {
    fn drop(&mut self) {
        let mut state = self.flights.state.lock().unwrap_or_else(|e| e.into_inner());
        if state.ai_capacity_waiters.get(&self.owner) == Some(&self.id) {
            state.ai_capacity_waiters.remove(&self.owner);
        }
    }
}

impl RemoteRawFlights {
    pub(super) fn new(
        executor: Arc<RawDevelopExecutor>,
        policy: RemoteRawFlightPolicy,
    ) -> Arc<Self> {
        Self::new_with_submitter(executor, policy)
    }

    fn new_with_submitter(
        submitter: Arc<dyn FlightSubmitter>,
        policy: RemoteRawFlightPolicy,
    ) -> Arc<Self> {
        assert!(policy.max_outstanding > 0);
        Arc::new(Self {
            submitter,
            policy,
            state: Mutex::new(State {
                lifecycle: OwnerLifecycle::Running,
                next_id: 1,
                entries: HashMap::new(),
                outstanding: HashMap::new(),
                cache: VecDeque::new(),
                cached_bytes: 0,
                next_ai_wait_id: 1,
                ai_capacity_waiters: HashMap::new(),
                ai_latest_request: HashMap::new(),
            }),
            changed: Condvar::new(),
        })
    }

    pub(super) fn lookup(&self, identity: &RemoteRawIdentity) -> Option<Arc<DevelopedRaw>> {
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .cache_lookup(identity)
    }

    pub(super) fn clear_cache(&self) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.cache.clear();
        state.cached_bytes = 0;
    }

    /// Terminal service stop. The current server's workers and AI threads may
    /// still hold this Arc; a new service starts with a new flight owner.
    pub(super) fn stop(&self) {
        let tickets = {
            let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            if state.lifecycle == OwnerLifecycle::Stopped {
                return;
            }
            state.lifecycle = OwnerLifecycle::Stopped;
            state.cache.clear();
            state.cached_bytes = 0;
            state.ai_capacity_waiters.clear();
            state.ai_latest_request.clear();
            for work in state.outstanding.values() {
                work.source_cancel.store(true, Ordering::Release);
            }
            let mut tickets = Vec::new();
            for entry in state.entries.values_mut() {
                match entry {
                    FlightState::Submitting {
                        cancel_requested, ..
                    } => *cancel_requested = true,
                    FlightState::InFlight { ticket, .. } => tickets.push(Arc::clone(ticket)),
                    FlightState::Cancelling { .. } | FlightState::Done { .. } => {}
                }
            }
            self.changed.notify_all();
            tickets
        };
        // A queued cancellation invokes the completion callback synchronously.
        for ticket in tickets {
            ticket.cancel();
        }
    }

    fn is_stopped(&self) -> bool {
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .lifecycle
            == OwnerLifecycle::Stopped
    }

    pub(super) fn develop_page<F>(
        self: &Arc<Self>,
        identity: RemoteRawIdentity,
        source_factory: F,
        cancel: &AtomicBool,
    ) -> Result<Arc<DevelopedRaw>, RemoteRawFlightError>
    where
        F: FnOnce(&Arc<AtomicBool>) -> Result<RawOwnedSource, RawError> + Send + 'static,
    {
        self.develop(
            identity,
            source_factory,
            cancel,
            None,
            self.policy.submit_priority,
        )
    }

    pub(super) fn develop_ai<F>(
        self: &Arc<Self>,
        identity: RemoteRawIdentity,
        owner: &str,
        source_factory: F,
        cancel: &AtomicBool,
    ) -> Result<Arc<DevelopedRaw>, RemoteRawFlightError>
    where
        F: FnOnce(&Arc<AtomicBool>) -> Result<RawOwnedSource, RawError> + Send + 'static,
    {
        if cancel.load(Ordering::Acquire) {
            return Err(RemoteRawFlightError::Cancelled);
        }
        // Claim newest-wins order at invocation, before any work that might
        // delay entry to the capacity queue.
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if state.lifecycle == OwnerLifecycle::Stopped || cancel.load(Ordering::Acquire) {
            return Err(RemoteRawFlightError::Cancelled);
        }
        let request_id = state.next_ai_wait_id;
        state.next_ai_wait_id = state
            .next_ai_wait_id
            .checked_add(1)
            .expect("AI request id exhausted");
        state.ai_latest_request.insert(owner.to_owned(), request_id);
        if state.ai_capacity_waiters.remove(owner).is_some() {
            self.changed.notify_all();
        }
        drop(state);
        let _request_lease = AiRequestLease {
            flights: Arc::clone(self),
            owner: owner.to_owned(),
            id: request_id,
        };
        self.develop(
            identity,
            source_factory,
            cancel,
            Some((owner, request_id)),
            RawPriority::High,
        )
    }

    /// A bounded source-selection operation that must inspect archive bytes
    /// before its final RAW identity is known. It occupies one outstanding id
    /// for the entire operation, so AI never extracts a nested ZIP while
    /// waiting for capacity. The resulting entry name is kept, not its bytes.
    pub(super) fn with_capacity_probe<T>(
        self: &Arc<Self>,
        demand: RawProbeDemand<'_>,
        cancel: &AtomicBool,
        action: impl FnOnce(&Arc<AtomicBool>) -> T,
    ) -> Result<T, RemoteRawFlightError> {
        if cancel.load(Ordering::Acquire) {
            return Err(RemoteRawFlightError::Cancelled);
        }
        let ai_request = if let RawProbeDemand::Ai { owner } = demand {
            let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            if state.lifecycle == OwnerLifecycle::Stopped || cancel.load(Ordering::Acquire) {
                return Err(RemoteRawFlightError::Cancelled);
            }
            let id = state.next_ai_wait_id;
            state.next_ai_wait_id = state
                .next_ai_wait_id
                .checked_add(1)
                .expect("AI request id exhausted");
            state.ai_latest_request.insert(owner.to_owned(), id);
            if state.ai_capacity_waiters.remove(owner).is_some() {
                self.changed.notify_all();
            }
            Some(AiRequestLease {
                flights: Arc::clone(self),
                owner: owner.to_owned(),
                id,
            })
        } else {
            None
        };
        let mut ai_wait: Option<AiCapacityLease> = None;
        let (lease, source_cancel) = loop {
            if cancel.load(Ordering::Acquire) {
                return Err(RemoteRawFlightError::Cancelled);
            }
            let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            if state.lifecycle == OwnerLifecycle::Stopped
                || ai_request.as_ref().is_some_and(|request| {
                    state.ai_latest_request.get(&request.owner) != Some(&request.id)
                })
                || ai_wait.as_ref().is_some_and(|wait| {
                    state.ai_capacity_waiters.get(&wait.owner) != Some(&wait.id)
                })
            {
                return Err(RemoteRawFlightError::Cancelled);
            }
            if state.outstanding.len() < self.policy.max_outstanding {
                if cancel.load(Ordering::Acquire) {
                    return Err(RemoteRawFlightError::Cancelled);
                }
                let (id, token) = state.reserve_probe();
                break (
                    CapacityProbeLease {
                        flights: Arc::clone(self),
                        id,
                    },
                    token,
                );
            }
            let RawProbeDemand::Ai { owner } = demand else {
                return Err(RemoteRawFlightError::Capacity);
            };
            if ai_wait.is_none() {
                let id = state.next_ai_wait_id;
                state.next_ai_wait_id = state
                    .next_ai_wait_id
                    .checked_add(1)
                    .expect("AI capacity wait id exhausted");
                state.ai_capacity_waiters.insert(owner.to_owned(), id);
                ai_wait = Some(AiCapacityLease {
                    flights: Arc::clone(self),
                    owner: owner.to_owned(),
                    id,
                });
                self.changed.notify_all();
            }
            let (next, _) = self
                .changed
                .wait_timeout(state, CANCEL_POLL)
                .unwrap_or_else(|e| e.into_inner());
            drop(next);
        };
        drop(ai_wait);
        let result = action(&source_cancel);
        drop(lease);
        if cancel.load(Ordering::Acquire)
            || self.is_stopped()
            || ai_request.as_ref().is_some_and(|request| {
                self.state
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .ai_latest_request
                    .get(&request.owner)
                    != Some(&request.id)
            })
        {
            Err(RemoteRawFlightError::Cancelled)
        } else {
            Ok(result)
        }
    }

    /// The same flight owner accepts a later, lower-priority S2c prefetch
    /// without creating another cache or outstanding-work accounting domain.
    pub(super) fn develop_with_priority<F>(
        self: &Arc<Self>,
        identity: RemoteRawIdentity,
        source_factory: F,
        cancel: &AtomicBool,
        priority: RawPriority,
    ) -> Result<Arc<DevelopedRaw>, RemoteRawFlightError>
    where
        F: FnOnce(&Arc<AtomicBool>) -> Result<RawOwnedSource, RawError> + Send + 'static,
    {
        self.develop(identity, source_factory, cancel, None, priority)
    }

    fn develop<F>(
        self: &Arc<Self>,
        identity: RemoteRawIdentity,
        source_factory: F,
        cancel: &AtomicBool,
        ai_owner: Option<(&str, u64)>,
        priority: RawPriority,
    ) -> Result<Arc<DevelopedRaw>, RemoteRawFlightError>
    where
        F: FnOnce(&Arc<AtomicBool>) -> Result<RawOwnedSource, RawError> + Send + 'static,
    {
        let mut ai_wait: Option<AiCapacityLease> = None;
        let join = loop {
            if cancel.load(Ordering::Acquire) {
                return Err(RemoteRawFlightError::Cancelled);
            }
            let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            if state.lifecycle == OwnerLifecycle::Stopped {
                return Err(RemoteRawFlightError::Cancelled);
            }
            if let Some((owner, request_id)) = ai_owner
                && state.ai_latest_request.get(owner) != Some(&request_id)
            {
                return Err(RemoteRawFlightError::Cancelled);
            }
            if let Some(wait) = &ai_wait
                && state.ai_capacity_waiters.get(&wait.owner) != Some(&wait.id)
            {
                return Err(RemoteRawFlightError::Cancelled);
            }
            if let Some(raster) = state.cache_lookup(&identity) {
                break Join::Ready(Ok(raster));
            }
            let mut promote = None;
            let joined = match state.entries.get_mut(&identity) {
                Some(FlightState::Submitting {
                    id,
                    waiters,
                    cancel_requested,
                }) => {
                    *waiters += 1;
                    *cancel_requested = false;
                    Some(Join::Participant {
                        id: *id,
                        submit: false,
                    })
                }
                Some(FlightState::InFlight {
                    id,
                    ticket,
                    waiters,
                }) => {
                    *waiters += 1;
                    if priority == RawPriority::High {
                        promote = Some(Arc::clone(ticket));
                    }
                    Some(Join::Participant {
                        id: *id,
                        submit: false,
                    })
                }
                Some(FlightState::Done { result, .. }) => Some(Join::Ready(result.clone())),
                Some(FlightState::Cancelling { .. }) | None => None,
            };
            if let Some(joined) = joined {
                drop(state);
                if let Some(ticket) = promote {
                    ticket.promote_to_high();
                }
                break joined;
            }
            // The key can be Cancelling while its old flight still occupies a
            // slot. Capacity is checked for every replacement as well.
            if state.outstanding.len() < self.policy.max_outstanding {
                if cancel.load(Ordering::Acquire) {
                    return Err(RemoteRawFlightError::Cancelled);
                }
                if let Some((owner, request_id)) = ai_owner
                    && state.ai_latest_request.get(owner) != Some(&request_id)
                {
                    return Err(RemoteRawFlightError::Cancelled);
                }
                if let Some(wait) = &ai_wait
                    && state.ai_capacity_waiters.get(&wait.owner) != Some(&wait.id)
                {
                    return Err(RemoteRawFlightError::Cancelled);
                }
                let id = state.reserve(&identity);
                break Join::Participant { id, submit: true };
            }
            let Some((owner, _)) = ai_owner else {
                break Join::Capacity;
            };
            if ai_wait.is_none() {
                let id = state.next_ai_wait_id;
                state.next_ai_wait_id = state
                    .next_ai_wait_id
                    .checked_add(1)
                    .expect("AI capacity wait id exhausted");
                state.ai_capacity_waiters.insert(owner.to_owned(), id);
                ai_wait = Some(AiCapacityLease {
                    flights: Arc::clone(self),
                    owner: owner.to_owned(),
                    id,
                });
                self.changed.notify_all();
            }
            let (next, _) = self
                .changed
                .wait_timeout(state, CANCEL_POLL)
                .unwrap_or_else(|e| e.into_inner());
            drop(next);
        };
        drop(ai_wait);
        match join {
            Join::Ready(result) => result.map_err(RemoteRawFlightError::Raw),
            Join::Capacity => Err(RemoteRawFlightError::Capacity),
            Join::Participant { id, submit } => {
                let lease = ParticipantLease {
                    flights: Arc::clone(self),
                    identity: identity.clone(),
                    id,
                };
                if submit {
                    self.start_source_worker(identity.clone(), id, source_factory, priority);
                }
                let result = self.wait_for_result(&identity, id, cancel);
                drop(lease);
                result
            }
        }
    }

    fn start_source_worker<F>(
        self: &Arc<Self>,
        identity: RemoteRawIdentity,
        id: u64,
        source_factory: F,
        priority: RawPriority,
    ) where
        F: FnOnce(&Arc<AtomicBool>) -> Result<RawOwnedSource, RawError> + Send + 'static,
    {
        let worker = Arc::clone(self);
        let worker_identity = identity.clone();
        let spawned = std::thread::Builder::new()
            .name("remote-raw-source".to_owned())
            .spawn(move || {
                worker.resolve_and_submit(worker_identity, id, source_factory, priority)
            });
        if let Err(error) = spawned {
            self.complete(
                &identity,
                id,
                Err(RawError::Io(format!("RAW source worker: {error}"))),
            );
        }
    }

    fn resolve_and_submit<F>(
        self: &Arc<Self>,
        identity: RemoteRawIdentity,
        id: u64,
        source_factory: F,
        priority: RawPriority,
    ) where
        F: FnOnce(&Arc<AtomicBool>) -> Result<RawOwnedSource, RawError>,
    {
        if self.is_stopped() {
            self.complete(&identity, id, Err(RawError::Cancelled));
            return;
        }
        let source_cancel = {
            let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            Arc::clone(
                &state
                    .outstanding
                    .get(&id)
                    .expect("reserved RAW flight exists until submit finishes")
                    .source_cancel,
            )
        };
        // Capacity was reserved before this can read ZIP source bytes.
        let resolved = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            source_factory(&source_cancel)
        }))
        .unwrap_or_else(|_| Err(RawError::Io("RAW source worker panicked".to_owned())));
        let source = match resolved {
            Ok(source) => source,
            Err(error) => {
                self.complete(&identity, id, Err(error));
                return;
            }
        };
        let has_waiters = {
            let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            state.lifecycle == OwnerLifecycle::Running
                && matches!(
                    state.entries.get(&identity),
                    Some(FlightState::Submitting {
                        id: current,
                        waiters,
                        cancel_requested: false,
                    }) if *current == id && *waiters > 0
                )
        };
        if !has_waiters {
            self.complete(&identity, id, Err(RawError::Cancelled));
            return;
        }
        let flights = Arc::clone(self);
        let completed_identity = identity.clone();
        let submitted = self.submitter.submit(
            source,
            identity.brightness,
            priority,
            Box::new(move |result| flights.complete(&completed_identity, id, result)),
        );
        match submitted {
            Ok(ticket) => self.attach_ticket(&identity, id, ticket),
            Err(error) => self.complete(&identity, id, Err(error)),
        }
    }

    fn attach_ticket(&self, identity: &RemoteRawIdentity, id: u64, ticket: Arc<dyn FlightTicket>) {
        let cancel_ticket = {
            let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            match state.entries.remove(identity) {
                Some(FlightState::Submitting {
                    id: current,
                    waiters,
                    cancel_requested,
                }) if current == id => {
                    if cancel_requested || waiters == 0 {
                        state
                            .entries
                            .insert(identity.clone(), FlightState::Cancelling { id });
                        true
                    } else {
                        state.entries.insert(
                            identity.clone(),
                            FlightState::InFlight {
                                id,
                                ticket: Arc::clone(&ticket),
                                waiters,
                            },
                        );
                        false
                    }
                }
                other => {
                    // An immediate callback can complete before submit returns.
                    if let Some(other) = other {
                        state.entries.insert(identity.clone(), other);
                    }
                    false
                }
            }
        };
        if cancel_ticket {
            ticket.cancel();
        }
    }

    fn complete(
        &self,
        identity: &RemoteRawIdentity,
        id: u64,
        result: Result<RawDevelopOutput, RawError>,
    ) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if state.outstanding.remove(&id).is_none() {
            return;
        }
        if state.lifecycle == OwnerLifecycle::Stopped {
            if state
                .entries
                .get(identity)
                .is_some_and(|entry| entry.id() == id)
            {
                state.entries.remove(identity);
            }
            self.changed.notify_all();
            return;
        }
        match state.entries.remove(identity) {
            Some(other) if other.id() != id => {
                state.entries.insert(identity.clone(), other);
            }
            Some(FlightState::Submitting { waiters, .. })
            | Some(FlightState::InFlight { waiters, .. }) => {
                if waiters > 0 {
                    let result = result.map(Arc::new);
                    if let Ok(raster) = &result {
                        state.cache_insert(identity.clone(), Arc::clone(raster), self.policy);
                    }
                    state.entries.insert(
                        identity.clone(),
                        FlightState::Done {
                            id,
                            result,
                            waiters,
                        },
                    );
                }
            }
            Some(FlightState::Cancelling { .. }) | None => {}
            Some(other) => {
                state.entries.insert(identity.clone(), other);
            }
        }
        // Also wakes AI capacity waiters when a replaced, cancelled flight ends.
        self.changed.notify_all();
    }

    fn wait_for_result(
        &self,
        identity: &RemoteRawIdentity,
        id: u64,
        cancel: &AtomicBool,
    ) -> Result<Arc<DevelopedRaw>, RemoteRawFlightError> {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        loop {
            if state.lifecycle == OwnerLifecycle::Stopped {
                return Err(RemoteRawFlightError::Cancelled);
            }
            if cancel.load(Ordering::Acquire) {
                return Err(RemoteRawFlightError::Cancelled);
            }
            if let Some(FlightState::Done {
                id: current,
                result,
                ..
            }) = state.entries.get(identity)
                && *current == id
            {
                return result.clone().map_err(RemoteRawFlightError::Raw);
            }
            let (next, _) = self
                .changed
                .wait_timeout(state, CANCEL_POLL)
                .unwrap_or_else(|e| e.into_inner());
            state = next;
        }
    }

    fn leave(&self, identity: &RemoteRawIdentity, id: u64) {
        let cancel_ticket = {
            let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            let Some(current) = state.entries.remove(identity) else {
                return;
            };
            if current.id() != id {
                state.entries.insert(identity.clone(), current);
                return;
            }
            match current {
                FlightState::Submitting {
                    id,
                    waiters,
                    cancel_requested,
                } => {
                    debug_assert!(waiters > 0);
                    let waiters = waiters - 1;
                    state.entries.insert(
                        identity.clone(),
                        FlightState::Submitting {
                            id,
                            waiters,
                            cancel_requested: cancel_requested || waiters == 0,
                        },
                    );
                    None
                }
                FlightState::InFlight {
                    id,
                    ticket,
                    waiters,
                } => {
                    debug_assert!(waiters > 0);
                    if waiters == 1 {
                        let cancel_ticket = Arc::clone(&ticket);
                        state
                            .entries
                            .insert(identity.clone(), FlightState::Cancelling { id });
                        Some(cancel_ticket)
                    } else {
                        state.entries.insert(
                            identity.clone(),
                            FlightState::InFlight {
                                id,
                                ticket,
                                waiters: waiters - 1,
                            },
                        );
                        None
                    }
                }
                FlightState::Done {
                    id,
                    result,
                    waiters,
                } => {
                    debug_assert!(waiters > 0);
                    if waiters > 1 {
                        state.entries.insert(
                            identity.clone(),
                            FlightState::Done {
                                id,
                                result,
                                waiters: waiters - 1,
                            },
                        );
                    }
                    None
                }
                FlightState::Cancelling { id } => {
                    state
                        .entries
                        .insert(identity.clone(), FlightState::Cancelling { id });
                    None
                }
            }
        };
        // A queued executor cancellation invokes its callback synchronously.
        if let Some(ticket) = cancel_ticket {
            ticket.cancel();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::raw::AppliedBrightness;
    use image::{DynamicImage, GenericImageView};
    use std::sync::atomic::AtomicUsize;
    use std::sync::mpsc;
    use std::thread;
    use std::time::Instant;

    fn identity(name: &str) -> RemoteRawIdentity {
        RemoteRawIdentity {
            normalized_path: name.to_owned(),
            mtime_100ns: 1,
            file_size: 10,
            zip_entry: None,
            zip_dir_prefix: None,
            brightness: RawBrightness::None,
        }
    }

    fn output() -> RawDevelopOutput {
        RawDevelopOutput {
            image: DynamicImage::new_rgb8(2, 3),
            brightness: AppliedBrightness::None,
        }
    }

    fn source(_: &Arc<AtomicBool>) -> Result<RawOwnedSource, RawError> {
        Ok(RawOwnedSource::Bytes(Arc::from(&b"test"[..])))
    }

    #[derive(Default)]
    struct FakeState {
        next_id: u64,
        submitted: Vec<u64>,
        priorities: Vec<RawPriority>,
        completions: HashMap<u64, Completion>,
        cancelled: Vec<u64>,
        promoted: Vec<u64>,
        sync_cancel: bool,
        instant: Option<Result<(), RawError>>,
        reject: Option<RawError>,
    }

    #[derive(Default)]
    struct FakeSubmitter {
        state: Mutex<FakeState>,
        changed: Condvar,
    }

    struct FakeTicket {
        owner: Arc<FakeSubmitter>,
        id: u64,
    }

    impl FlightTicket for FakeTicket {
        fn cancel(&self) {
            let callback = {
                let mut state = self.owner.state.lock().unwrap();
                state.cancelled.push(self.id);
                if state.sync_cancel {
                    state.completions.remove(&self.id)
                } else {
                    None
                }
            };
            if let Some(callback) = callback {
                callback(Err(RawError::Cancelled));
            }
        }

        fn promote_to_high(&self) {
            self.owner.state.lock().unwrap().promoted.push(self.id);
            self.owner.changed.notify_all();
        }
    }

    impl FlightSubmitter for Arc<FakeSubmitter> {
        fn submit(
            &self,
            _source: RawOwnedSource,
            _brightness: RawBrightness,
            priority: RawPriority,
            complete: Completion,
        ) -> Result<Arc<dyn FlightTicket>, RawError> {
            let (id, instant, reject) = {
                let mut state = self.state.lock().unwrap();
                let id = state.next_id + 1;
                state.next_id = id;
                state.submitted.push(id);
                state.priorities.push(priority);
                let instant = state.instant.clone();
                let reject = state.reject.clone();
                if instant.is_none() && reject.is_none() {
                    state.completions.insert(id, complete);
                    self.changed.notify_all();
                    return Ok(Arc::new(FakeTicket {
                        owner: Arc::clone(self),
                        id,
                    }));
                }
                self.changed.notify_all();
                (id, instant, reject)
            };
            if let Some(error) = reject {
                return Err(error);
            }
            complete(instant.expect("instant result").map(|_| output()));
            Ok(Arc::new(FakeTicket {
                owner: Arc::clone(self),
                id,
            }))
        }
    }

    impl FakeSubmitter {
        fn finish(&self, id: u64, result: Result<RawDevelopOutput, RawError>) {
            let callback = self.state.lock().unwrap().completions.remove(&id).unwrap();
            callback(result);
        }

        fn wait_for_submits(&self, count: usize) {
            let deadline = Instant::now() + Duration::from_secs(5);
            let mut state = self.state.lock().unwrap();
            while state.submitted.len() < count {
                assert!(Instant::now() < deadline, "submit did not arrive");
                let (next, _) = self.changed.wait_timeout(state, CANCEL_POLL).unwrap();
                state = next;
            }
        }
    }

    fn flights(fake: &Arc<FakeSubmitter>) -> Arc<RemoteRawFlights> {
        RemoteRawFlights::new_with_submitter(
            Arc::new(Arc::clone(fake)),
            RemoteRawFlightPolicy::s2b(),
        )
    }

    fn wait_for_waiters(flights: &RemoteRawFlights, key: &RemoteRawIdentity, wanted: usize) {
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut state = flights.state.lock().unwrap();
        loop {
            let actual = match state.entries.get(key) {
                Some(FlightState::Submitting { waiters, .. })
                | Some(FlightState::InFlight { waiters, .. })
                | Some(FlightState::Done { waiters, .. }) => *waiters,
                Some(FlightState::Cancelling { .. }) | None => 0,
            };
            if actual == wanted {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "waiter count did not reach {wanted}"
            );
            let (next, _) = flights.changed.wait_timeout(state, CANCEL_POLL).unwrap();
            state = next;
        }
    }

    fn wait_for_inflight(flights: &RemoteRawFlights, key: &RemoteRawIdentity) {
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut state = flights.state.lock().unwrap();
        while !matches!(state.entries.get(key), Some(FlightState::InFlight { .. })) {
            assert!(Instant::now() < deadline, "RAW ticket did not arrive");
            let (next, _) = flights.changed.wait_timeout(state, CANCEL_POLL).unwrap();
            state = next;
        }
    }

    #[test]
    fn ai_capacity_probe_does_not_start_archive_selection_while_full() {
        use std::io::Write;

        let temp = tempfile::tempdir().unwrap();
        let archive = temp.path().join("ai-capacity-nested.zip");
        let nested = {
            let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
            writer
                .start_file("page.dng", zip::write::SimpleFileOptions::default())
                .unwrap();
            writer.write_all(b"RAW source bytes").unwrap();
            writer.finish().unwrap().into_inner()
        };
        let mut writer = zip::ZipWriter::new(std::fs::File::create(&archive).unwrap());
        writer
            .start_file("inner.zip", zip::write::SimpleFileOptions::default())
            .unwrap();
        writer.write_all(&nested).unwrap();
        writer.finish().unwrap();

        let fake = Arc::new(FakeSubmitter::default());
        let flights = flights(&fake);
        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel::<()>();
        let release_rx = Arc::new(Mutex::new(release_rx));
        let mut holders = Vec::new();
        for _ in 0..REMOTE_RAW_FLIGHT_LIMIT {
            let flights = Arc::clone(&flights);
            let entered = entered_tx.clone();
            let release = Arc::clone(&release_rx);
            holders.push(thread::spawn(move || {
                flights
                    .with_capacity_probe(RawProbeDemand::Page, &AtomicBool::new(false), |_| {
                        entered.send(()).unwrap();
                        release.lock().unwrap().recv().unwrap();
                    })
                    .unwrap();
            }));
        }
        for _ in 0..REMOTE_RAW_FLIGHT_LIMIT {
            entered_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        }
        let cancel = Arc::new(AtomicBool::new(false));
        let selections = Arc::new(AtomicUsize::new(0));
        let ai = {
            let flights = Arc::clone(&flights);
            let cancel = Arc::clone(&cancel);
            let selections = Arc::clone(&selections);
            let archive = archive.clone();
            thread::spawn(move || {
                flights.with_capacity_probe(
                    RawProbeDemand::Ai { owner: "phone" },
                    &cancel,
                    |stop| {
                        selections.fetch_add(1, Ordering::SeqCst);
                        crate::zip_loader::read_first_image_bytes_cancellable_with_stop(
                            &archive,
                            &cancel,
                            Some(stop),
                        )
                    },
                )
            })
        };
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut state = flights.state.lock().unwrap();
        while !state.ai_capacity_waiters.contains_key("phone") {
            assert!(Instant::now() < deadline, "AI did not wait for capacity");
            let (next, _) = flights.changed.wait_timeout(state, CANCEL_POLL).unwrap();
            state = next;
        }
        drop(state);
        assert_eq!(selections.load(Ordering::SeqCst), 0);
        assert!(!crate::zip_loader::nested_cache_contains(
            &archive,
            "inner.zip"
        ));
        cancel.store(true, Ordering::Release);
        assert!(matches!(
            ai.join().unwrap(),
            Err(RemoteRawFlightError::Cancelled)
        ));
        assert_eq!(selections.load(Ordering::SeqCst), 0);
        assert!(!crate::zip_loader::nested_cache_contains(
            &archive,
            "inner.zip"
        ));
        for _ in 0..REMOTE_RAW_FLIGHT_LIMIT {
            release_tx.send(()).unwrap();
        }
        for holder in holders {
            holder.join().unwrap();
        }
        assert!(flights.state.lock().unwrap().outstanding.is_empty());
    }

    #[test]
    fn identity_separates_precise_stamp_zip_context_and_brightness() {
        let original = identity("a.raw");
        let mut changed = original.clone();
        changed.mtime_100ns += 1;
        assert_ne!(original, changed);
        changed = original.clone();
        changed.zip_entry = Some("one.raw".into());
        assert_ne!(original, changed);
        changed = original.clone();
        changed.zip_dir_prefix = Some("nested/".into());
        assert_ne!(original, changed);
        changed = original.clone();
        changed.brightness = RawBrightness::MatchPreview;
        assert_ne!(original, changed);
    }

    #[test]
    fn immediate_completion_and_error_before_ticket_are_delivered() {
        let fake = Arc::new(FakeSubmitter::default());
        fake.state.lock().unwrap().instant = Some(Ok(()));
        let flights = flights(&fake);
        let key = identity("instant.raw");
        let result = flights
            .develop_page(key.clone(), source, &AtomicBool::new(false))
            .unwrap();
        assert!(Arc::ptr_eq(&result, &flights.lookup(&key).unwrap()));
        assert_eq!(flights.state.lock().unwrap().outstanding.len(), 0);
        fake.state.lock().unwrap().instant = Some(Err(RawError::Corrupt("bad".into())));
        assert!(matches!(
            flights.develop_page(identity("error.raw"), source, &AtomicBool::new(false)),
            Err(RemoteRawFlightError::Raw(RawError::Corrupt(message))) if message == "bad"
        ));
        assert_eq!(flights.state.lock().unwrap().outstanding.len(), 0);
    }

    #[test]
    fn submit_rejection_and_source_failure_release_capacity() {
        let fake = Arc::new(FakeSubmitter::default());
        fake.state.lock().unwrap().reject = Some(RawError::Cancelled);
        let flights = flights(&fake);
        assert!(matches!(
            flights.develop_page(identity("a"), source, &AtomicBool::new(false)),
            Err(RemoteRawFlightError::Raw(RawError::Cancelled))
        ));
        assert!(matches!(
            flights.develop_page(
                identity("b"),
                |_| Err(RawError::Io("read".into())),
                &AtomicBool::new(false),
            ),
            Err(RemoteRawFlightError::Raw(RawError::Io(message))) if message == "read"
        ));
        assert!(flights.state.lock().unwrap().outstanding.is_empty());
    }

    #[test]
    fn cancelled_last_waiter_replaces_flight_and_old_completion_cannot_publish() {
        let fake = Arc::new(FakeSubmitter::default());
        let flights = flights(&fake);
        let key = identity("same.raw");
        let first_cancel = Arc::new(AtomicBool::new(false));
        let first = {
            let flights = Arc::clone(&flights);
            let key = key.clone();
            let cancel = Arc::clone(&first_cancel);
            thread::spawn(move || flights.develop_page(key, source, &cancel))
        };
        fake.wait_for_submits(1);
        wait_for_inflight(&flights, &key);
        first_cancel.store(true, Ordering::Release);
        assert!(matches!(
            first.join().unwrap(),
            Err(RemoteRawFlightError::Cancelled)
        ));
        assert!(matches!(
            flights.state.lock().unwrap().entries.get(&key),
            Some(FlightState::Cancelling { .. })
        ));
        let second = {
            let flights = Arc::clone(&flights);
            let key = key.clone();
            thread::spawn(move || flights.develop_page(key, source, &AtomicBool::new(false)))
        };
        fake.wait_for_submits(2);
        fake.finish(1, Ok(output()));
        assert!(flights.lookup(&key).is_none());
        fake.finish(2, Ok(output()));
        assert_eq!(second.join().unwrap().unwrap().image.height(), 3);
        assert!(flights.lookup(&key).is_some());
    }

    #[test]
    fn queued_cancel_callback_is_safe_and_releases_outstanding() {
        let fake = Arc::new(FakeSubmitter::default());
        fake.state.lock().unwrap().sync_cancel = true;
        let flights = flights(&fake);
        let cancel = Arc::new(AtomicBool::new(false));
        let waiter = {
            let flights = Arc::clone(&flights);
            let cancel = Arc::clone(&cancel);
            thread::spawn(move || flights.develop_page(identity("queued"), source, &cancel))
        };
        fake.wait_for_submits(1);
        wait_for_inflight(&flights, &identity("queued"));
        cancel.store(true, Ordering::Release);
        assert!(matches!(
            waiter.join().unwrap(),
            Err(RemoteRawFlightError::Cancelled)
        ));
        assert_eq!(flights.state.lock().unwrap().outstanding.len(), 0);
    }

    #[test]
    fn submitting_cancel_can_be_revoked_by_join_before_ticket_arrives() {
        let fake = Arc::new(FakeSubmitter::default());
        let flights = flights(&fake);
        let key = identity("submitting.raw");
        let id = flights.state.lock().unwrap().reserve(&key);
        let lease = ParticipantLease {
            flights: Arc::clone(&flights),
            identity: key.clone(),
            id,
        };
        drop(lease);
        {
            let mut state = flights.state.lock().unwrap();
            let Some(FlightState::Submitting {
                waiters,
                cancel_requested,
                ..
            }) = state.entries.get_mut(&key)
            else {
                panic!("expected Submitting");
            };
            assert_eq!(*waiters, 0);
            assert!(*cancel_requested);
            // This is the Submitting join cell. No ticket exists yet.
            *waiters += 1;
            *cancel_requested = false;
        }
        let ticket = Arc::new(FakeTicket {
            owner: Arc::clone(&fake),
            id: 42,
        });
        flights.attach_ticket(&key, id, ticket);
        assert!(matches!(
            flights.state.lock().unwrap().entries.get(&key),
            Some(FlightState::InFlight { waiters: 1, .. })
        ));
        assert!(fake.state.lock().unwrap().cancelled.is_empty());
        flights.complete(&key, id, Ok(output()));
        assert!(matches!(
            flights.state.lock().unwrap().entries.get(&key),
            Some(FlightState::Done { waiters: 1, .. })
        ));
        flights.leave(&key, id);
        assert!(!flights.state.lock().unwrap().entries.contains_key(&key));
    }

    #[test]
    fn done_join_reads_result_without_extending_participant_lifetime() {
        let fake = Arc::new(FakeSubmitter::default());
        let flights = flights(&fake);
        let key = identity("done.raw");
        let result = Arc::new(output());
        flights.state.lock().unwrap().entries.insert(
            key.clone(),
            FlightState::Done {
                id: 1,
                result: Ok(Arc::clone(&result)),
                waiters: 1,
            },
        );
        let joined = flights
            .develop_page(
                key.clone(),
                |_| panic!("Done must not submit"),
                &AtomicBool::new(false),
            )
            .unwrap();
        assert!(Arc::ptr_eq(&result, &joined));
        assert!(matches!(
            flights.state.lock().unwrap().entries.get(&key),
            Some(FlightState::Done { waiters: 1, .. })
        ));
        flights.leave(&key, 1);
        assert!(flights.state.lock().unwrap().entries.is_empty());
    }

    #[test]
    fn cache_replacement_does_not_invalidate_result_pins() {
        let fake = Arc::new(FakeSubmitter::default());
        fake.state.lock().unwrap().instant = Some(Ok(()));
        let flights = flights(&fake);
        let first_key = identity("first.raw");
        let first = flights
            .develop_page(first_key.clone(), source, &AtomicBool::new(false))
            .unwrap();
        let second_key = identity("second.raw");
        let second = flights
            .develop_page(second_key.clone(), source, &AtomicBool::new(false))
            .unwrap();
        assert!(!Arc::ptr_eq(&first, &second));
        assert!(flights.lookup(&first_key).is_none());
        assert!(Arc::ptr_eq(&second, &flights.lookup(&second_key).unwrap()));
        assert_eq!(first.image.dimensions(), (2, 3));
    }

    #[test]
    fn page_and_ai_join_one_flight_and_promote_low_priority_outside_lock() {
        let fake = Arc::new(FakeSubmitter::default());
        let flights = flights(&fake);
        let key = identity("shared.raw");
        let page = {
            let flights = Arc::clone(&flights);
            let key = key.clone();
            thread::spawn(move || {
                flights.develop_with_priority(
                    key,
                    source,
                    &AtomicBool::new(false),
                    RawPriority::Background,
                )
            })
        };
        fake.wait_for_submits(1);
        wait_for_inflight(&flights, &key);
        let ai = {
            let flights = Arc::clone(&flights);
            let key = key.clone();
            thread::spawn(move || {
                flights.develop_ai(
                    key,
                    "owner",
                    |_| panic!("joined AI must not read source"),
                    &AtomicBool::new(false),
                )
            })
        };
        wait_for_waiters(&flights, &key, 2);
        {
            let deadline = Instant::now() + Duration::from_secs(5);
            let mut state = fake.state.lock().unwrap();
            while state.promoted.is_empty() {
                assert!(Instant::now() < deadline, "join did not promote ticket");
                let (next, _) = fake.changed.wait_timeout(state, CANCEL_POLL).unwrap();
                state = next;
            }
            assert_eq!(state.submitted, [1]);
            assert_eq!(state.priorities, [RawPriority::Background]);
            assert_eq!(state.promoted, [1]);
        }
        fake.finish(1, Ok(output()));
        let page = page.join().unwrap().unwrap();
        let ai = ai.join().unwrap().unwrap();
        assert!(Arc::ptr_eq(&page, &ai));
        assert_eq!(flights.state.lock().unwrap().outstanding.len(), 0);
    }

    #[test]
    fn submitting_source_join_after_owner_cancel_revokes_cancel_request() {
        let fake = Arc::new(FakeSubmitter::default());
        let flights = flights(&fake);
        let key = identity("zip-entry.raw");
        let owner_cancel = Arc::new(AtomicBool::new(false));
        let (source_started, source_started_rx) = mpsc::channel();
        let (release_source, release_source_rx) = mpsc::channel();
        let owner = {
            let flights = Arc::clone(&flights);
            let key = key.clone();
            let cancel = Arc::clone(&owner_cancel);
            thread::spawn(move || {
                flights.develop_page(
                    key,
                    move |flight_cancel| {
                        source_started.send(()).unwrap();
                        release_source_rx.recv().unwrap();
                        assert!(
                            !flight_cancel.load(Ordering::Acquire),
                            "participant cancellation must not cancel shared ZIP source"
                        );
                        source(flight_cancel)
                    },
                    &cancel,
                )
            })
        };
        source_started_rx
            .recv_timeout(Duration::from_secs(5))
            .unwrap();
        owner_cancel.store(true, Ordering::Release);
        assert!(matches!(
            owner.join().unwrap(),
            Err(RemoteRawFlightError::Cancelled)
        ));
        assert!(matches!(
            flights.state.lock().unwrap().entries.get(&key),
            Some(FlightState::Submitting {
                waiters: 0,
                cancel_requested: true,
                ..
            })
        ));
        let joined = {
            let flights = Arc::clone(&flights);
            let key = key.clone();
            thread::spawn(move || {
                flights.develop_page(
                    key,
                    |_| panic!("joined waiter must not read ZIP source"),
                    &AtomicBool::new(false),
                )
            })
        };
        wait_for_waiters(&flights, &key, 1);
        assert!(matches!(
            flights.state.lock().unwrap().entries.get(&key),
            Some(FlightState::Submitting {
                cancel_requested: false,
                ..
            })
        ));
        release_source.send(()).unwrap();
        fake.wait_for_submits(1);
        assert!(fake.state.lock().unwrap().cancelled.is_empty());
        fake.finish(1, Ok(output()));
        assert_eq!(joined.join().unwrap().unwrap().image.height(), 3);
        assert!(flights.lookup(&key).is_some());
    }

    #[test]
    fn lone_submitting_waiter_cancels_before_gated_source_read_finishes() {
        let fake = Arc::new(FakeSubmitter::default());
        let flights = flights(&fake);
        let key = identity("lone-zip.raw");
        let cancel = Arc::new(AtomicBool::new(false));
        let (source_started, source_started_rx) = mpsc::channel();
        let (release_source, release_source_rx) = mpsc::channel();
        let waiter = {
            let flights = Arc::clone(&flights);
            let key = key.clone();
            let cancel = Arc::clone(&cancel);
            thread::spawn(move || {
                flights.develop_page(
                    key,
                    move |flight_cancel| {
                        source_started.send(()).unwrap();
                        release_source_rx.recv().unwrap();
                        source(flight_cancel)
                    },
                    &cancel,
                )
            })
        };
        source_started_rx
            .recv_timeout(Duration::from_secs(5))
            .unwrap();
        cancel.store(true, Ordering::Release);
        assert!(matches!(
            waiter.join().unwrap(),
            Err(RemoteRawFlightError::Cancelled)
        ));
        assert!(matches!(
            flights.state.lock().unwrap().entries.get(&key),
            Some(FlightState::Submitting {
                waiters: 0,
                cancel_requested: true,
                ..
            })
        ));
        release_source.send(()).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut state = flights.state.lock().unwrap();
        while !state.outstanding.is_empty() {
            assert!(Instant::now() < deadline, "source worker did not finish");
            let (next, _) = flights.changed.wait_timeout(state, CANCEL_POLL).unwrap();
            state = next;
        }
        assert!(fake.state.lock().unwrap().submitted.is_empty());
    }

    #[test]
    fn service_stop_cancels_in_progress_source_read_without_submitting() {
        let fake = Arc::new(FakeSubmitter::default());
        let flights = flights(&fake);
        let (source_token_tx, source_token_rx) = mpsc::channel();
        let (release_source, release_source_rx) = mpsc::channel();
        let page = {
            let flights = Arc::clone(&flights);
            thread::spawn(move || {
                flights.develop_page(
                    identity("stopping-source.raw"),
                    move |flight_cancel| {
                        source_token_tx.send(Arc::clone(flight_cancel)).unwrap();
                        release_source_rx.recv().unwrap();
                        if flight_cancel.load(Ordering::Acquire) {
                            Err(RawError::Cancelled)
                        } else {
                            source(flight_cancel)
                        }
                    },
                    &AtomicBool::new(false),
                )
            })
        };
        let token = source_token_rx
            .recv_timeout(Duration::from_secs(5))
            .unwrap();
        assert!(!token.load(Ordering::Acquire));
        flights.stop();
        assert!(token.load(Ordering::Acquire));
        assert!(matches!(
            page.join().unwrap(),
            Err(RemoteRawFlightError::Cancelled)
        ));
        release_source.send(()).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut state = flights.state.lock().unwrap();
        while !state.outstanding.is_empty() {
            assert!(
                Instant::now() < deadline,
                "stopped source worker did not finish"
            );
            let (next, _) = flights.changed.wait_timeout(state, CANCEL_POLL).unwrap();
            state = next;
        }
        drop(state);
        assert!(fake.state.lock().unwrap().submitted.is_empty());
    }

    #[test]
    fn repeated_same_key_cancel_recreate_near_limit_never_exceeds_six() {
        let fake = Arc::new(FakeSubmitter::default());
        let flights = flights(&fake);
        let mut filler = Vec::new();
        for index in 0..4 {
            let cancel = Arc::new(AtomicBool::new(false));
            let worker = {
                let flights = Arc::clone(&flights);
                let cancel = Arc::clone(&cancel);
                thread::spawn(move || {
                    flights.develop_page(identity(&format!("filler-{index}")), source, &cancel)
                })
            };
            filler.push((cancel, worker));
        }
        fake.wait_for_submits(4);
        let key = identity("churn.raw");
        for generation in 0..4 {
            if generation >= 2 {
                assert!(matches!(
                    flights.develop_page(key.clone(), source, &AtomicBool::new(false)),
                    Err(RemoteRawFlightError::Capacity)
                ));
                assert_eq!(
                    flights.state.lock().unwrap().outstanding.len(),
                    REMOTE_RAW_FLIGHT_LIMIT
                );
                fake.finish(3 + generation as u64, Err(RawError::Cancelled));
            }
            let cancel = Arc::new(AtomicBool::new(false));
            let worker = {
                let flights = Arc::clone(&flights);
                let key = key.clone();
                let cancel = Arc::clone(&cancel);
                thread::spawn(move || flights.develop_page(key, source, &cancel))
            };
            fake.wait_for_submits(5 + generation);
            wait_for_inflight(&flights, &key);
            assert!(flights.state.lock().unwrap().outstanding.len() <= REMOTE_RAW_FLIGHT_LIMIT);
            cancel.store(true, Ordering::Release);
            assert!(matches!(
                worker.join().unwrap(),
                Err(RemoteRawFlightError::Cancelled)
            ));
        }
        fake.finish(7, Err(RawError::Cancelled));
        fake.finish(8, Err(RawError::Cancelled));
        for (cancel, worker) in filler {
            cancel.store(true, Ordering::Release);
            assert!(matches!(
                worker.join().unwrap(),
                Err(RemoteRawFlightError::Cancelled)
            ));
        }
        for id in 1..=4 {
            fake.finish(id, Err(RawError::Cancelled));
        }
        assert!(flights.state.lock().unwrap().outstanding.is_empty());
    }

    #[test]
    fn running_flight_cancels_only_after_last_page_and_ai_participant_leaves() {
        let fake = Arc::new(FakeSubmitter::default());
        let flights = flights(&fake);
        let key = identity("two-participants.raw");
        let page_cancel = Arc::new(AtomicBool::new(false));
        let page = {
            let flights = Arc::clone(&flights);
            let key = key.clone();
            let cancel = Arc::clone(&page_cancel);
            thread::spawn(move || flights.develop_page(key, source, &cancel))
        };
        fake.wait_for_submits(1);
        wait_for_inflight(&flights, &key);
        let ai_cancel = Arc::new(AtomicBool::new(false));
        let ai = {
            let flights = Arc::clone(&flights);
            let key = key.clone();
            let cancel = Arc::clone(&ai_cancel);
            thread::spawn(move || {
                flights.develop_ai(
                    key,
                    "owner",
                    |_| panic!("AI must join running flight"),
                    &cancel,
                )
            })
        };
        wait_for_waiters(&flights, &key, 2);
        page_cancel.store(true, Ordering::Release);
        assert!(matches!(
            page.join().unwrap(),
            Err(RemoteRawFlightError::Cancelled)
        ));
        assert!(fake.state.lock().unwrap().cancelled.is_empty());
        ai_cancel.store(true, Ordering::Release);
        assert!(matches!(
            ai.join().unwrap(),
            Err(RemoteRawFlightError::Cancelled)
        ));
        assert!(fake.state.lock().unwrap().cancelled.contains(&1));
        fake.finish(1, Err(RawError::Cancelled));
        assert!(flights.state.lock().unwrap().outstanding.is_empty());
    }

    #[test]
    fn ai_capacity_wait_wakes_on_drain_and_connection_close_flags() {
        for _reason in ["drain", "connection-close"] {
            let fake = Arc::new(FakeSubmitter::default());
            let flights = RemoteRawFlights::new_with_submitter(
                Arc::new(Arc::clone(&fake)),
                RemoteRawFlightPolicy {
                    max_outstanding: 1,
                    ..RemoteRawFlightPolicy::s2b()
                },
            );
            let occupied = identity("occupied.raw");
            let id = flights.state.lock().unwrap().reserve(&occupied);
            let cancel = Arc::new(AtomicBool::new(false));
            let reads = Arc::new(AtomicUsize::new(0));
            let ai = {
                let flights = Arc::clone(&flights);
                let cancel = Arc::clone(&cancel);
                let reads = Arc::clone(&reads);
                thread::spawn(move || {
                    flights.develop_ai(
                        identity("waiting.raw"),
                        "owner",
                        move |flight_cancel| {
                            reads.fetch_add(1, Ordering::SeqCst);
                            source(flight_cancel)
                        },
                        &cancel,
                    )
                })
            };
            let deadline = Instant::now() + Duration::from_secs(5);
            let mut state = flights.state.lock().unwrap();
            while !state.ai_capacity_waiters.contains_key("owner") {
                assert!(Instant::now() < deadline, "AI capacity wait did not arrive");
                let (next, _) = flights.changed.wait_timeout(state, CANCEL_POLL).unwrap();
                state = next;
            }
            drop(state);
            cancel.store(true, Ordering::Release);
            assert!(matches!(
                ai.join().unwrap(),
                Err(RemoteRawFlightError::Cancelled)
            ));
            assert_eq!(reads.load(Ordering::SeqCst), 0);
            assert!(flights.state.lock().unwrap().ai_capacity_waiters.is_empty());
            flights.complete(&occupied, id, Err(RawError::Cancelled));
        }
    }

    #[test]
    fn newer_ai_capacity_waiter_supersedes_old_owner_slot() {
        let fake = Arc::new(FakeSubmitter::default());
        let flights = RemoteRawFlights::new_with_submitter(
            Arc::new(Arc::clone(&fake)),
            RemoteRawFlightPolicy {
                max_outstanding: 1,
                ..RemoteRawFlightPolicy::s2b()
            },
        );
        let occupied = {
            let flights = Arc::clone(&flights);
            thread::spawn(move || {
                flights.develop_page(identity("occupied"), source, &AtomicBool::new(false))
            })
        };
        fake.wait_for_submits(1);
        wait_for_inflight(&flights, &identity("occupied"));
        let reads = Arc::new(AtomicUsize::new(0));
        let old = {
            let flights = Arc::clone(&flights);
            let reads = Arc::clone(&reads);
            thread::spawn(move || {
                flights.develop_ai(
                    identity("old-ai"),
                    "same-owner",
                    move |flight_cancel| {
                        reads.fetch_add(1, Ordering::SeqCst);
                        source(flight_cancel)
                    },
                    &AtomicBool::new(false),
                )
            })
        };
        let old_slot = {
            let deadline = Instant::now() + Duration::from_secs(5);
            let mut state = flights.state.lock().unwrap();
            loop {
                if let Some(&id) = state.ai_capacity_waiters.get("same-owner") {
                    break id;
                }
                assert!(
                    Instant::now() < deadline,
                    "old AI capacity wait did not arrive"
                );
                let (next, _) = flights.changed.wait_timeout(state, CANCEL_POLL).unwrap();
                state = next;
            }
        };
        let newest = {
            let flights = Arc::clone(&flights);
            let reads = Arc::clone(&reads);
            thread::spawn(move || {
                flights.develop_ai(
                    identity("new-ai"),
                    "same-owner",
                    move |flight_cancel| {
                        reads.fetch_add(1, Ordering::SeqCst);
                        source(flight_cancel)
                    },
                    &AtomicBool::new(false),
                )
            })
        };
        assert!(matches!(
            old.join().unwrap(),
            Err(RemoteRawFlightError::Cancelled)
        ));
        let newest_slot = {
            let deadline = Instant::now() + Duration::from_secs(5);
            let mut state = flights.state.lock().unwrap();
            loop {
                if let Some(&id) = state.ai_capacity_waiters.get("same-owner")
                    && id != old_slot
                {
                    break id;
                }
                assert!(
                    Instant::now() < deadline,
                    "new AI capacity wait did not arrive"
                );
                let (next, _) = flights.changed.wait_timeout(state, CANCEL_POLL).unwrap();
                state = next;
            }
        };
        assert_ne!(old_slot, newest_slot);
        assert_eq!(reads.load(Ordering::SeqCst), 0);
        fake.finish(1, Ok(output()));
        occupied.join().unwrap().unwrap();
        fake.wait_for_submits(2);
        fake.finish(2, Ok(output()));
        assert!(newest.join().unwrap().is_ok());
        assert_eq!(reads.load(Ordering::SeqCst), 1);
        assert!(flights.state.lock().unwrap().ai_latest_request.is_empty());
    }

    #[test]
    fn service_stop_wakes_page_and_ai_and_rejects_late_publication() {
        let fake = Arc::new(FakeSubmitter::default());
        let flights = RemoteRawFlights::new_with_submitter(
            Arc::new(Arc::clone(&fake)),
            RemoteRawFlightPolicy {
                max_outstanding: 1,
                ..RemoteRawFlightPolicy::s2b()
            },
        );
        let key = identity("running.raw");
        let page = {
            let flights = Arc::clone(&flights);
            let key = key.clone();
            thread::spawn(move || flights.develop_page(key, source, &AtomicBool::new(false)))
        };
        fake.wait_for_submits(1);
        wait_for_inflight(&flights, &key);
        let reads = Arc::new(AtomicUsize::new(0));
        let ai = {
            let flights = Arc::clone(&flights);
            let reads = Arc::clone(&reads);
            thread::spawn(move || {
                flights.develop_ai(
                    identity("waiting.raw"),
                    "owner",
                    move |flight_cancel| {
                        reads.fetch_add(1, Ordering::SeqCst);
                        source(flight_cancel)
                    },
                    &AtomicBool::new(false),
                )
            })
        };
        {
            let deadline = Instant::now() + Duration::from_secs(5);
            let mut state = flights.state.lock().unwrap();
            while !state.ai_capacity_waiters.contains_key("owner") {
                assert!(Instant::now() < deadline, "AI capacity wait did not arrive");
                let (next, _) = flights.changed.wait_timeout(state, CANCEL_POLL).unwrap();
                state = next;
            }
        }
        flights.stop();
        flights.stop();
        assert!(matches!(
            page.join().unwrap(),
            Err(RemoteRawFlightError::Cancelled)
        ));
        assert!(matches!(
            ai.join().unwrap(),
            Err(RemoteRawFlightError::Cancelled)
        ));
        assert_eq!(reads.load(Ordering::SeqCst), 0);
        assert!(fake.state.lock().unwrap().cancelled.contains(&1));
        fake.finish(1, Ok(output()));
        assert!(flights.lookup(&key).is_none());
        assert!(flights.state.lock().unwrap().outstanding.is_empty());
        assert!(matches!(
            flights.develop_page(
                key,
                |_| panic!("stopped owner must not read"),
                &AtomicBool::new(false)
            ),
            Err(RemoteRawFlightError::Cancelled)
        ));
    }

    #[test]
    fn capacity_counts_cancelled_old_ids_and_ai_waits_without_reading_source() {
        let fake = Arc::new(FakeSubmitter::default());
        let flights = flights(&fake);
        let mut waiters = Vec::new();
        let mut cancels = Vec::new();
        for index in 0..REMOTE_RAW_FLIGHT_LIMIT {
            let cancel = Arc::new(AtomicBool::new(false));
            let worker = {
                let flights = Arc::clone(&flights);
                let cancel = Arc::clone(&cancel);
                thread::spawn(move || {
                    flights.develop_page(identity(&format!("{index}.raw")), source, &cancel)
                })
            };
            cancels.push(cancel);
            waiters.push(worker);
        }
        fake.wait_for_submits(REMOTE_RAW_FLIGHT_LIMIT);
        wait_for_inflight(&flights, &identity("0.raw"));
        assert!(matches!(
            flights.develop_page(identity("overflow"), source, &AtomicBool::new(false)),
            Err(RemoteRawFlightError::Capacity)
        ));
        cancels[0].store(true, Ordering::Release);
        assert!(matches!(
            waiters.remove(0).join().unwrap(),
            Err(RemoteRawFlightError::Cancelled)
        ));
        assert!(
            matches!(
                flights.develop_page(identity("0.raw"), source, &AtomicBool::new(false)),
                Err(RemoteRawFlightError::Capacity)
            ),
            "replacement must count the cancelling old flight"
        );
        let reads = Arc::new(AtomicUsize::new(0));
        let ai_cancel = Arc::new(AtomicBool::new(false));
        let ai = {
            let flights = Arc::clone(&flights);
            let reads = Arc::clone(&reads);
            let cancel = Arc::clone(&ai_cancel);
            thread::spawn(move || {
                flights.develop_ai(
                    identity("ai.raw"),
                    "owner",
                    move |flight_cancel| {
                        reads.fetch_add(1, Ordering::SeqCst);
                        source(flight_cancel)
                    },
                    &cancel,
                )
            })
        };
        thread::yield_now();
        assert_eq!(reads.load(Ordering::SeqCst), 0);
        ai_cancel.store(true, Ordering::Release);
        assert!(matches!(
            ai.join().unwrap(),
            Err(RemoteRawFlightError::Cancelled)
        ));
        assert_eq!(reads.load(Ordering::SeqCst), 0);
        for cancel in cancels.iter().skip(1) {
            cancel.store(true, Ordering::Release);
        }
        for waiter in waiters {
            assert!(matches!(
                waiter.join().unwrap(),
                Err(RemoteRawFlightError::Cancelled)
            ));
        }
        for id in 1..=REMOTE_RAW_FLIGHT_LIMIT as u64 {
            fake.finish(id, Err(RawError::Cancelled));
        }
        assert!(flights.state.lock().unwrap().outstanding.is_empty());
    }
}
