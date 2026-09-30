//! Session-owned declarations; one service worker identifies and reconciles them.
//! Detached waiters own leases, never page jobs or PC display state.
use super::raw_flights::{RemoteRawFlightError, RemoteRawFlights, RemoteRawIdentity};
use crate::raw::{RawError, RawOwnedSource};
use mimageviewer_ipc::{
    RawPrefetchWindowAck, RawPrefetchWindowRequest, RawPrefetchWindowStatus, RemoteAddress,
};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

const WAIT: Duration = Duration::from_millis(50);
const THREAD_LIMIT: usize = 6;
type SourceFactory =
    Arc<dyn Fn(&Arc<AtomicBool>) -> Result<RawOwnedSource, RawError> + Send + Sync>;

#[derive(Clone)]
pub(super) struct PrefetchSource {
    pub(super) identity: RemoteRawIdentity,
    pub(super) read: SourceFactory,
}

pub(super) trait PrefetchIdentifier: Send + Sync {
    fn identify(&self, address: &RemoteAddress, cancel: &Arc<AtomicBool>)
    -> Option<PrefetchSource>;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum WaiterState {
    Pending,
    Running,
    Retiring,
}

struct Waiter {
    source: PrefetchSource,
    state: WaiterState,
    cancel: Arc<AtomicBool>,
}

/// The sole owner of an acquired session's generation and desired set.
#[derive(Clone, Copy, PartialEq, Eq)]
enum WindowLifecycle {
    Active,
    Terminal,
}
#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum ServiceLifecycle {
    #[default]
    Running,
    Stopped,
}

struct RemoteRawPrefetchWindow {
    session_id: String,
    lifecycle: WindowLifecycle,
    accepted: u64,
    applied: u64,
    mailbox: Option<RawPrefetchWindowRequest>,
    identify_cancel: Arc<AtomicBool>,
    desired: HashMap<RemoteRawIdentity, PrefetchSource>,
    active: HashMap<RemoteRawIdentity, u64>,
}

#[derive(Default)]
struct State {
    lifecycle: ServiceLifecycle,
    window: Option<RemoteRawPrefetchWindow>,
    next_waiter: u64,
    // Retiring records outlive their session, and count until the thread exits.
    waiters: HashMap<u64, Waiter>,
}

impl State {
    fn retire(&mut self, id: u64) {
        if let Some(waiter) = self.waiters.get_mut(&id) {
            waiter.cancel.store(true, Ordering::Release);
            if waiter.state == WaiterState::Pending {
                self.waiters.remove(&id);
            } else {
                waiter.state = WaiterState::Retiring;
            }
        }
    }

    fn terminate(&mut self) {
        if let Some(window) = &mut self.window {
            window.lifecycle = WindowLifecycle::Terminal;
            window.identify_cancel.store(true, Ordering::Release);
            window.mailbox = None;
            window.desired.clear();
            window.active.clear();
        }
        for id in self.waiters.keys().copied().collect::<Vec<_>>() {
            self.retire(id);
        }
    }

    fn thread_count(&self) -> usize {
        self.waiters
            .values()
            .filter(|waiter| waiter.state != WaiterState::Pending)
            .count()
    }
}

pub(super) struct RemoteRawPrefetchRegistry {
    state: Mutex<State>,
    changed: Condvar,
    flights: Arc<RemoteRawFlights>,
    #[cfg(test)]
    fail_spawn: AtomicBool,
    #[cfg(test)]
    capacity_hook: Mutex<Option<Box<dyn FnOnce() + Send>>>,
}

impl RemoteRawPrefetchRegistry {
    pub(super) fn new(flights: Arc<RemoteRawFlights>) -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(State::default()),
            changed: Condvar::new(),
            flights,
            #[cfg(test)]
            fail_spawn: AtomicBool::new(false),
            #[cfg(test)]
            capacity_hook: Mutex::new(None),
        })
    }

    /// Called under the session state lock at acquisition. No flight operations.
    pub(super) fn acquire(&self, session_id: &str) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if state.lifecycle == ServiceLifecycle::Stopped {
            return;
        }
        state.terminate();
        state.window = Some(RemoteRawPrefetchWindow {
            session_id: session_id.to_owned(),
            lifecycle: WindowLifecycle::Active,
            accepted: 0,
            applied: 0,
            mailbox: None,
            identify_cancel: Arc::new(AtomicBool::new(false)),
            desired: HashMap::new(),
            active: HashMap::new(),
        });
        self.changed.notify_all();
    }

    /// Called under the session state lock by the shared begin_drain transition.
    pub(super) fn terminate(&self) {
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .terminate();
        self.changed.notify_all();
    }

    pub(super) fn stop(&self) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.lifecycle = ServiceLifecycle::Stopped;
        state.terminate();
        self.changed.notify_all();
    }

    /// Reader-thread admission: syntax only, no filesystem or flight operations.
    pub(super) fn declare(
        &self,
        session_id: &str,
        request: RawPrefetchWindowRequest,
    ) -> RawPrefetchWindowAck {
        let generation = request.window_generation;
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let status = match state.window.as_mut() {
            Some(window)
                if window.session_id == session_id
                    && window.lifecycle == WindowLifecycle::Active =>
            {
                if generation < window.accepted {
                    RawPrefetchWindowStatus::Stale
                } else if generation == window.accepted {
                    RawPrefetchWindowStatus::Duplicate
                } else {
                    window.accepted = generation;
                    window.mailbox = Some(request);
                    self.changed.notify_all();
                    RawPrefetchWindowStatus::Accepted
                }
            }
            _ => RawPrefetchWindowStatus::Terminal,
        };
        RawPrefetchWindowAck {
            window_generation: generation,
            status,
        }
    }

    fn reconcile(&self, session: &str, generation: u64, sources: Vec<PrefetchSource>) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let Some(window) = state.window.as_mut() else {
            return;
        };
        if window.lifecycle == WindowLifecycle::Terminal
            || window.session_id != session
            || window.accepted != generation
        {
            return;
        }
        if window.applied == generation {
            return;
        }
        window.applied = generation;
        window.desired = sources
            .into_iter()
            .map(|source| (source.identity.clone(), source))
            .collect();
        let removed: Vec<_> = window
            .active
            .iter()
            .filter(|(identity, _)| !window.desired.contains_key(*identity))
            .map(|(identity, id)| (identity.clone(), *id))
            .collect();
        for (identity, _) in &removed {
            window.active.remove(identity);
        }
        for (_, id) in removed {
            state.retire(id);
        }
        let added: Vec<_> = {
            let window = state.window.as_ref().expect("window exists");
            window
                .desired
                .values()
                .filter(|source| !window.active.contains_key(&source.identity))
                .cloned()
                .collect()
        };
        for source in added {
            state.next_waiter = state
                .next_waiter
                .checked_add(1)
                .expect("RAW waiter ID exhausted");
            let id = state.next_waiter;
            state
                .window
                .as_mut()
                .expect("window exists")
                .active
                .insert(source.identity.clone(), id);
            state.waiters.insert(
                id,
                Waiter {
                    source,
                    state: WaiterState::Pending,
                    cancel: Arc::new(AtomicBool::new(false)),
                },
            );
        }
    }

    fn pending(&self) -> Vec<u64> {
        let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if state.window.as_ref().is_none_or(|window| {
            window.lifecycle == WindowLifecycle::Terminal || window.mailbox.is_some()
        }) {
            return Vec::new();
        }
        state
            .waiters
            .iter()
            .filter(|(_, waiter)| waiter.state == WaiterState::Pending)
            .map(|(id, _)| *id)
            .collect()
    }

    fn reserve(&self, id: u64) -> Option<(PrefetchSource, Arc<AtomicBool>)> {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if state.lifecycle == ServiceLifecycle::Stopped || state.thread_count() >= THREAD_LIMIT {
            return None;
        }
        let waiter = state.waiters.get_mut(&id)?;
        if waiter.state != WaiterState::Pending {
            return None;
        }
        waiter.state = WaiterState::Running;
        Some((waiter.source.clone(), Arc::clone(&waiter.cancel)))
    }

    fn retry(&self, id: u64) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(waiter) = state.waiters.get_mut(&id) {
            if waiter.state == WaiterState::Running {
                waiter.state = WaiterState::Pending;
            } else {
                state.waiters.remove(&id);
            }
        }
        self.changed.notify_all();
    }

    fn finished(&self, id: u64) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let Some(waiter) = state.waiters.remove(&id) else {
            return;
        };
        if let Some(window) = &mut state.window
            && window.active.get(&waiter.source.identity) == Some(&id)
        {
            window.active.remove(&waiter.source.identity);
        }
        self.changed.notify_all();
    }

    fn run(self: &Arc<Self>, identifier: Arc<dyn PrefetchIdentifier>) {
        loop {
            let declaration = {
                let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
                if state.lifecycle == ServiceLifecycle::Stopped {
                    return;
                }
                state.window.as_mut().and_then(|window| {
                    window.mailbox.take().map(|request| {
                        (
                            window.session_id.clone(),
                            request,
                            Arc::clone(&window.identify_cancel),
                        )
                    })
                })
            };
            if let Some((session, request, cancel)) = declaration {
                let sources = request
                    .entries
                    .iter()
                    .filter_map(|address| identifier.identify(address, &cancel))
                    .filter(|source| self.flights.lookup(&source.identity).is_none())
                    .collect();
                self.reconcile(&session, request.window_generation, sources);
            }
            // Each wake retries actual admission, even if capacity was released
            // before Pending was recorded. No capacity epoch can lose the wake.
            for id in self.pending() {
                let Some((source, cancel)) = self.reserve(id) else {
                    continue;
                };
                let mut admission = match self.flights.try_prefetch(source.identity) {
                    Ok(admission) => admission,
                    Err(RemoteRawFlightError::Capacity) => {
                        #[cfg(test)]
                        if let Some(hook) = self.capacity_hook.lock().unwrap().take() {
                            hook();
                        }
                        self.retry(id);
                        continue;
                    }
                    Err(_) => {
                        self.finished(id);
                        continue;
                    }
                };
                #[cfg(test)]
                if self.fail_spawn.swap(false, Ordering::AcqRel) {
                    drop(admission);
                    self.retry(id);
                    continue;
                }
                admission.start(move |cancel| (source.read)(cancel));
                if cancel.load(Ordering::Acquire) {
                    drop(admission);
                    self.finished(id);
                    continue;
                }
                let registry = Arc::clone(self);
                let spawned = std::thread::Builder::new()
                    .name("remote-raw-prefetch-waiter".to_owned())
                    .spawn(move || {
                        struct Exit(Arc<RemoteRawPrefetchRegistry>, u64);
                        impl Drop for Exit {
                            fn drop(&mut self) {
                                self.0.finished(self.1);
                            }
                        }
                        let _exit = Exit(registry, id);
                        let _ = admission.wait(&cancel);
                    });
                if spawned.is_err() {
                    self.retry(id);
                }
            }
            let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            if state.lifecycle == ServiceLifecycle::Stopped {
                return;
            }
            if state
                .window
                .as_ref()
                .is_some_and(|window| window.mailbox.is_some())
            {
                continue;
            }
            if state
                .waiters
                .values()
                .any(|waiter| waiter.state == WaiterState::Pending)
            {
                drop(
                    self.changed
                        .wait_timeout(state, WAIT)
                        .unwrap_or_else(|e| e.into_inner()),
                );
            } else {
                drop(self.changed.wait(state).unwrap_or_else(|e| e.into_inner()));
            }
        }
    }
}

/// Ordered startup rollback / service shutdown owns the worker, not the session.
pub(super) struct RawPrefetchService {
    pub(super) registry: Arc<RemoteRawPrefetchRegistry>,
    worker: Option<std::thread::JoinHandle<()>>,
}

impl RawPrefetchService {
    pub(super) fn start(
        flights: Arc<RemoteRawFlights>,
        identifier: Arc<dyn PrefetchIdentifier>,
    ) -> Result<Self, String> {
        let registry = RemoteRawPrefetchRegistry::new(flights);
        let owner = Arc::clone(&registry);
        let worker = std::thread::Builder::new()
            .name("remote-raw-prefetch".to_owned())
            .spawn(move || owner.run(identifier))
            .map_err(|error| {
                registry.stop();
                format!("RAW prefetch worker: {error}")
            })?;
        Ok(Self {
            registry,
            worker: Some(worker),
        })
    }
}

impl Drop for RawPrefetchService {
    fn drop(&mut self) {
        self.registry.stop();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
        // A replacement service cannot reset its thread budget while waiters
        // from this one are still retiring. Waiters release their own lease on
        // cancellation without waiting for the executor job to finish.
        let mut state = self
            .registry
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        while !state.waiters.is_empty() {
            state = self
                .registry
                .changed
                .wait_timeout(state, WAIT)
                .unwrap_or_else(|e| e.into_inner())
                .0;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::raw_flights::tests::{
        FakeSubmitter, identity, output, source, wait_for_waiters,
    };
    use super::*;
    use std::sync::mpsc;
    use std::time::Instant;

    fn spec(name: &str) -> PrefetchSource {
        PrefetchSource {
            identity: identity(name),
            read: Arc::new(source),
        }
    }
    fn request(generation: u64, names: &[&str]) -> RawPrefetchWindowRequest {
        RawPrefetchWindowRequest {
            window_generation: generation,
            entries: names
                .iter()
                .map(|name| RemoteAddress::file(*name))
                .collect(),
        }
    }
    fn pending_ids(registry: &RemoteRawPrefetchRegistry) -> Vec<u64> {
        registry
            .state
            .lock()
            .unwrap()
            .waiters
            .iter()
            .filter(|(_, waiter)| waiter.state == WaiterState::Pending)
            .map(|(id, _)| *id)
            .collect()
    }
    fn registry() -> Arc<RemoteRawPrefetchRegistry> {
        RemoteRawPrefetchRegistry::new(Arc::new(FakeSubmitter::default()).s2c())
    }
    struct Identifier;
    impl PrefetchIdentifier for Identifier {
        fn identify(&self, address: &RemoteAddress, _: &Arc<AtomicBool>) -> Option<PrefetchSource> {
            Some(spec(&address.path))
        }
    }
    fn wait(registry: &RemoteRawPrefetchRegistry, predicate: impl Fn(&State) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut state = registry.state.lock().unwrap();
        while !predicate(&state) {
            assert!(Instant::now() < deadline, "window state did not arrive");
            state = registry.changed.wait_timeout(state, WAIT).unwrap().0;
        }
    }

    #[test]
    fn mailbox_preserves_11_after_10_and_duplicate_ack() {
        let registry = registry();
        registry.acquire("session");
        assert_eq!(
            registry.declare("session", request(11, &["new"])).status,
            RawPrefetchWindowStatus::Accepted
        );
        assert_eq!(
            registry.declare("session", request(10, &["old"])).status,
            RawPrefetchWindowStatus::Stale
        );
        assert_eq!(
            registry.declare("session", request(11, &[])).status,
            RawPrefetchWindowStatus::Duplicate
        );
        assert_eq!(
            registry
                .state
                .lock()
                .unwrap()
                .window
                .as_ref()
                .unwrap()
                .mailbox
                .as_ref()
                .unwrap()
                .entries[0]
                .path,
            "new"
        );
        registry.declare("session", request(12, &[]));
        assert!(
            registry
                .state
                .lock()
                .unwrap()
                .window
                .as_ref()
                .unwrap()
                .mailbox
                .as_ref()
                .unwrap()
                .entries
                .is_empty()
        );
    }

    #[test]
    fn overlapping_windows_keep_waiter_ids_cancel_outside_and_do_not_revive() {
        let registry = registry();
        registry.acquire("session");
        registry.declare("session", request(1, &["a", "b", "c"]));
        registry.reconcile("session", 1, vec![spec("a"), spec("b"), spec("c")]);
        let ids = registry
            .state
            .lock()
            .unwrap()
            .window
            .as_ref()
            .unwrap()
            .active
            .clone();
        registry.reconcile("session", 1, vec![spec("a"), spec("b"), spec("c")]);
        assert_eq!(
            registry
                .state
                .lock()
                .unwrap()
                .window
                .as_ref()
                .unwrap()
                .active,
            ids
        );
        let a = ids[&identity("a")];
        registry.reserve(a).unwrap();
        registry.finished(a);
        registry.reconcile("session", 1, vec![spec("a"), spec("b"), spec("c")]);
        assert!(
            !registry
                .state
                .lock()
                .unwrap()
                .window
                .as_ref()
                .unwrap()
                .active
                .contains_key(&identity("a"))
        );
        registry.declare("session", request(2, &["b", "c", "d"]));
        registry.reconcile("session", 2, vec![spec("b"), spec("c"), spec("d")]);
        let state = registry.state.lock().unwrap();
        let active = &state.window.as_ref().unwrap().active;
        assert_eq!(active[&identity("b")], ids[&identity("b")]);
        assert_eq!(active[&identity("c")], ids[&identity("c")]);
        assert!(!active.contains_key(&identity("a")));
        assert!(active.contains_key(&identity("d")));
    }

    #[test]
    fn retiring_counts_service_wide_before_spawn_and_old_completion_cannot_delete_new() {
        let registry = registry();
        registry.acquire("old");
        registry.declare("old", request(1, &[]));
        registry.reconcile("old", 1, vec![spec("a"), spec("b"), spec("c")]);
        let old = pending_ids(&registry);
        for id in &old {
            registry.reserve(*id).unwrap();
        }
        registry.terminate();
        registry.acquire("new");
        registry.declare("new", request(1, &[]));
        registry.reconcile("new", 1, vec![spec("a"), spec("b"), spec("c")]);
        let new = pending_ids(&registry);
        for id in &new {
            registry.reserve(*id).unwrap();
        }
        assert_eq!(registry.state.lock().unwrap().thread_count(), 6);
        registry.declare("new", request(2, &[]));
        registry.reconcile("new", 2, vec![spec("a"), spec("b"), spec("d")]);
        let pending = pending_ids(&registry);
        assert_eq!(pending.len(), 1);
        assert!(registry.reserve(pending[0]).is_none());
        registry.finished(old[0]);
        assert!(registry.reserve(pending[0]).is_some());
        for id in old.iter().skip(1) {
            registry.finished(*id);
        }
        let state = registry.state.lock().unwrap();
        let new_a = new
            .iter()
            .copied()
            .find(|id| {
                state
                    .waiters
                    .get(id)
                    .is_some_and(|waiter| waiter.source.identity == identity("a"))
            })
            .unwrap();
        assert_eq!(state.window.as_ref().unwrap().active[&identity("a")], new_a);
    }

    #[test]
    fn late_identification_does_not_overwrite_empty_or_terminal_declaration() {
        let registry = registry();
        registry.acquire("session");
        registry.declare("session", request(1, &["a"]));
        registry.declare("session", request(2, &[]));
        registry.reconcile("session", 1, vec![spec("a")]);
        assert!(registry.state.lock().unwrap().waiters.is_empty());
        registry.reconcile("session", 2, Vec::new());
        registry.terminate();
        registry.reconcile("session", 2, vec![spec("a")]);
        assert!(registry.state.lock().unwrap().waiters.is_empty());
        assert_eq!(
            registry.declare("session", request(3, &["a"])).status,
            RawPrefetchWindowStatus::Terminal
        );
    }

    #[test]
    fn worker_slow_identification_is_discarded_after_new_empty_mailbox() {
        struct Slow {
            entered: mpsc::Sender<()>,
            release: Mutex<mpsc::Receiver<()>>,
        }
        impl PrefetchIdentifier for Slow {
            fn identify(
                &self,
                address: &RemoteAddress,
                _: &Arc<AtomicBool>,
            ) -> Option<PrefetchSource> {
                self.entered.send(()).unwrap();
                self.release.lock().unwrap().recv().unwrap();
                Some(spec(&address.path))
            }
        }
        let fake = Arc::new(FakeSubmitter::default());
        let (entered, arrived) = mpsc::channel();
        let (release, blocked) = mpsc::channel();
        let service = RawPrefetchService::start(
            fake.s2c(),
            Arc::new(Slow {
                entered,
                release: Mutex::new(blocked),
            }),
        )
        .unwrap();
        service.registry.acquire("session");
        service.registry.declare("session", request(1, &["old"]));
        arrived.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(
            service.registry.declare("session", request(2, &[])).status,
            RawPrefetchWindowStatus::Accepted
        );
        release.send(()).unwrap();
        wait(&service.registry, |state| {
            state.window.as_ref().unwrap().applied == 2
        });
        assert_eq!(fake.submits(), 0);
        assert!(service.registry.state.lock().unwrap().waiters.is_empty());
    }

    #[test]
    fn reserved_slot_fast_completion_and_spawn_rollback_preserve_new_waiter_id() {
        let registry = registry();
        registry.acquire("session");
        registry.declare("session", request(1, &[]));
        registry.reconcile("session", 1, vec![spec("a")]);
        let first = pending_ids(&registry)[0];
        registry.reserve(first).unwrap();
        assert_eq!(registry.state.lock().unwrap().thread_count(), 1);
        registry.retry(first);
        assert_eq!(registry.state.lock().unwrap().thread_count(), 0);
        registry.reserve(first).unwrap();
        registry.declare("session", request(2, &[]));
        registry.reconcile("session", 2, vec![]);
        registry.declare("session", request(3, &[]));
        registry.reconcile("session", 3, vec![spec("a")]);
        let second = pending_ids(&registry)[0];
        assert_ne!(first, second);
        registry.reserve(second).unwrap();
        registry.finished(first);
        registry.finished(first);
        assert_eq!(
            registry
                .state
                .lock()
                .unwrap()
                .window
                .as_ref()
                .unwrap()
                .active[&identity("a")],
            second
        );
        registry.finished(second);
        registry.reconcile("session", 3, vec![spec("a")]);
        assert!(registry.state.lock().unwrap().waiters.is_empty());
    }

    #[test]
    fn service_worker_reconciles_overlap_without_redeveloping_and_caches_completion() {
        let fake = Arc::new(FakeSubmitter::default());
        let flights = fake.s2c();
        let service =
            RawPrefetchService::start(Arc::clone(&flights), Arc::new(Identifier)).unwrap();
        let registry = &service.registry;
        registry.acquire("session");
        registry.declare("session", request(1, &["a", "b", "c"]));
        fake.wait_for_submits(3);
        registry.declare("session", request(2, &["a", "b", "c"]));
        wait(registry, |state| {
            state.window.as_ref().unwrap().applied == 2
        });
        assert_eq!(fake.submits(), 3);
        for id in 1..=3 {
            fake.finish(id, Ok(output()));
        }
        wait(registry, |state| state.waiters.is_empty());
        assert!(flights.lookup(&identity("a")).is_some());
        registry.declare("session", request(3, &["a", "b", "c"]));
        wait(registry, |state| {
            state.window.as_ref().unwrap().applied == 3
        });
        assert_eq!(fake.submits(), 3);
    }

    #[test]
    fn pending_retries_actual_admission_when_capacity_freed_before_pending_record() {
        let fake = Arc::new(FakeSubmitter::default());
        let flights = fake.s2c();
        let held: Vec<_> = (0..3)
            .map(|index| {
                flights
                    .try_prefetch(identity(&format!("held-{index}")))
                    .unwrap()
            })
            .collect();
        let service =
            RawPrefetchService::start(Arc::clone(&flights), Arc::new(Identifier)).unwrap();
        let (released, arrived) = mpsc::channel();
        *service.registry.capacity_hook.lock().unwrap() = Some(Box::new(move || {
            drop(held);
            released.send(Instant::now()).unwrap();
        }));
        service.registry.acquire("session");
        service
            .registry
            .declare("session", request(1, &["pending"]));
        let started = arrived.recv_timeout(Duration::from_secs(5)).unwrap();
        fake.wait_for_submits(1);
        // The algorithm waits at most 50ms; allow scheduler noise in the witness.
        assert!(started.elapsed() < Duration::from_millis(250));
        fake.finish(1, Ok(output()));
        wait(&service.registry, |state| state.waiters.is_empty());
    }

    #[test]
    fn spawn_failure_releases_reserved_slot_and_retries_pending() {
        let fake = Arc::new(FakeSubmitter::default());
        let flights = fake.s2c();
        let service = RawPrefetchService::start(flights, Arc::new(Identifier)).unwrap();
        service.registry.fail_spawn.store(true, Ordering::Release);
        service.registry.acquire("session");
        service.registry.declare("session", request(1, &["a"]));
        wait(&service.registry, |_| {
            !service.registry.fail_spawn.load(Ordering::Acquire)
        });
        fake.wait_for_submits(1);
        fake.finish(1, Ok(output()));
        wait(&service.registry, |state| state.waiters.is_empty());
        assert_eq!(service.registry.state.lock().unwrap().thread_count(), 0);
    }

    #[test]
    fn window_joins_foreground_lease_survives_foreground_departure_and_window_cancel_preserves_foreground()
     {
        let fake = Arc::new(FakeSubmitter::default());
        let flights = fake.s2c();
        let cancel = Arc::new(AtomicBool::new(false));
        let foreground = {
            let flights = Arc::clone(&flights);
            let cancel = Arc::clone(&cancel);
            std::thread::spawn(move || flights.develop_page(identity("a"), source, &cancel))
        };
        fake.wait_for_submits(1);
        let service =
            RawPrefetchService::start(Arc::clone(&flights), Arc::new(Identifier)).unwrap();
        service.registry.acquire("session");
        service.registry.declare("session", request(1, &["a"]));
        wait(&service.registry, |state| {
            state
                .waiters
                .values()
                .any(|waiter| waiter.state == WaiterState::Running)
        });
        wait_for_waiters(&flights, &identity("a"), 2);
        cancel.store(true, Ordering::Release);
        assert!(foreground.join().unwrap().is_err());
        fake.finish(1, Ok(output()));
        wait(&service.registry, |state| state.waiters.is_empty());
        assert!(flights.lookup(&identity("a")).is_some());
        let foreground = {
            let flights = Arc::clone(&flights);
            std::thread::spawn(move || {
                flights.develop_ai(identity("b"), "ai-owner", source, &AtomicBool::new(false))
            })
        };
        fake.wait_for_submits(2);
        service.registry.declare("session", request(2, &["b"]));
        wait(&service.registry, |state| {
            state
                .waiters
                .values()
                .any(|waiter| waiter.state == WaiterState::Running)
        });
        wait_for_waiters(&flights, &identity("b"), 2);
        service.registry.terminate();
        fake.finish(2, Ok(output()));
        assert!(foreground.join().unwrap().is_ok());
    }

    #[test]
    fn service_stop_and_startup_rollback_are_terminal_and_new_session_resets_generation() {
        let registry = registry();
        registry.acquire("first");
        registry.declare("first", request(11, &["a"]));
        registry.terminate();
        registry.acquire("second");
        assert_eq!(
            registry.declare("second", request(1, &[])).status,
            RawPrefetchWindowStatus::Accepted
        );
        registry.stop();
        registry.acquire("third");
        assert_eq!(
            registry.declare("third", request(1, &[])).status,
            RawPrefetchWindowStatus::Terminal
        );
        let fake = Arc::new(FakeSubmitter::default());
        let service = RawPrefetchService::start(fake.s2c(), Arc::new(Identifier)).unwrap();
        let registry = Arc::clone(&service.registry);
        registry.acquire("rollback");
        drop(service);
        assert_eq!(
            registry.declare("rollback", request(1, &[])).status,
            RawPrefetchWindowStatus::Terminal
        );
    }

    #[test]
    fn service_shutdown_cancels_and_waits_for_all_detached_waiters_to_exit() {
        let fake = Arc::new(FakeSubmitter::default());
        let service = RawPrefetchService::start(fake.s2c(), Arc::new(Identifier)).unwrap();
        let registry = Arc::clone(&service.registry);
        registry.acquire("session");
        registry.declare("session", request(1, &["a"]));
        fake.wait_for_submits(1);
        drop(service);
        assert_eq!(registry.state.lock().unwrap().thread_count(), 0);
        assert_eq!(
            registry.declare("session", request(2, &[])).status,
            RawPrefetchWindowStatus::Terminal
        );
        fake.wait_for_cancels(1);
        assert_eq!(fake.cancellations(), [1]);
        fake.finish(1, Err(RawError::Cancelled));
    }
}
