//! Process-wide admission control for fullscreen page loads.
//!
//! The execution budget is global, while each request remains owned by the
//! `ViewerContextBundle::fs_pending` entry that holds its ticket. A cancelled
//! running request keeps its permit until the worker actually exits.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Instant;

pub(crate) const FS_PAGE_LOAD_TOTAL_PERMITS: usize = 6;
pub(crate) const FS_PAGE_LOAD_HIGH_RESERVED_PERMITS: usize = 2;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FsPageLoadPriority {
    Normal,
    High,
}

struct SchedulerInner {
    state: Mutex<SchedulerState>,
    changed: Condvar,
}

pub(crate) struct FsPageLoadScheduler {
    inner: Arc<SchedulerInner>,
}

impl FsPageLoadScheduler {
    pub(crate) fn new() -> Self {
        Self::with_limits(
            FS_PAGE_LOAD_TOTAL_PERMITS,
            FS_PAGE_LOAD_HIGH_RESERVED_PERMITS,
        )
    }

    fn with_limits(total: usize, high_reserved: usize) -> Self {
        assert!(total >= 1, "page-load permits must be non-zero");
        assert!(
            high_reserved < total,
            "at least one normal page-load permit must remain"
        );
        Self {
            inner: Arc::new(SchedulerInner {
                state: Mutex::new(SchedulerState {
                    total,
                    normal_limit: total - high_reserved,
                    next_request_id: 1,
                    next_enqueue_order: 1,
                    requests: HashMap::new(),
                }),
                changed: Condvar::new(),
            }),
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn request(
        &self,
        owner_context: u64,
        idx: usize,
        priority: FsPageLoadPriority,
        contract: FsPageLoadContract,
        perf_key: Option<String>,
        perf_seq: u64,
    ) -> FsPageLoadTicket {
        self.request_with_cancel(
            owner_context,
            idx,
            priority,
            contract,
            perf_key,
            perf_seq,
            Arc::new(AtomicBool::new(false)),
        )
    }

    pub(crate) fn supersede_waiting_for_latest_seek(&self, owner_context: u64) {
        let (superseded, stats) = {
            let mut state = self.inner.state.lock().unwrap();
            let superseded = state.remove_waiting_superseded_by_seek(owner_context, None);
            let stats = state.stats();
            self.inner.changed.notify_all();
            (superseded, stats)
        };
        for request in superseded {
            emit_scheduler_event(
                "scheduler_cancel_waiting",
                &request.perf,
                request.priority,
                request.contract,
                stats,
                None,
            );
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn request_with_cancel(
        &self,
        owner_context: u64,
        idx: usize,
        priority: FsPageLoadPriority,
        contract: FsPageLoadContract,
        perf_key: Option<String>,
        perf_seq: u64,
        cancel: Arc<AtomicBool>,
    ) -> FsPageLoadTicket {
        let perf = PerfIdentity {
            key: perf_key,
            seq: perf_seq,
            idx,
            owner_context,
        };
        let (request_id, superseded, stats) = {
            let mut state = self.inner.state.lock().unwrap();
            let request_id = state.next_request_id;
            state.next_request_id = state
                .next_request_id
                .checked_add(1)
                .expect("page-load request id exhausted");
            let enqueue_order = state.next_enqueue_order;
            state.next_enqueue_order = state
                .next_enqueue_order
                .checked_add(1)
                .expect("page-load enqueue order exhausted");
            let superseded = if contract == FsPageLoadContract::LatestSeek {
                state.remove_waiting_superseded_by_seek(owner_context, None)
            } else {
                Vec::new()
            };
            state.requests.insert(
                request_id,
                RequestRecord {
                    priority,
                    contract,
                    phase: RequestPhase::Waiting,
                    enqueue_order,
                    cancel: Arc::clone(&cancel),
                    cancel_started_at: None,
                    perf: perf.clone(),
                },
            );
            let stats = state.stats();
            self.inner.changed.notify_all();
            (request_id, superseded, stats)
        };
        for request in superseded {
            emit_scheduler_event(
                "scheduler_cancel_waiting",
                &request.perf,
                request.priority,
                request.contract,
                stats,
                None,
            );
        }
        emit_scheduler_event("scheduler_enqueue", &perf, priority, contract, stats, None);
        FsPageLoadTicket {
            inner: Arc::clone(&self.inner),
            request_id,
            cancel,
            armed: true,
        }
    }

    #[cfg(test)]
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn request_with_cancel_for_test(
        &self,
        owner_context: u64,
        idx: usize,
        priority: FsPageLoadPriority,
        contract: FsPageLoadContract,
        cancel: Arc<AtomicBool>,
    ) -> FsPageLoadTicket {
        self.request_with_cancel(owner_context, idx, priority, contract, None, 0, cancel)
    }

    #[cfg(test)]
    pub(crate) fn stats(&self) -> FsPageLoadSchedulerStats {
        self.inner.state.lock().unwrap().stats()
    }
}

pub(crate) struct FsPageLoadTicket {
    inner: Arc<SchedulerInner>,
    request_id: u64,
    cancel: Arc<AtomicBool>,
    armed: bool,
}

impl FsPageLoadTicket {
    pub(crate) fn waiter(&self) -> FsPageLoadWaiter {
        FsPageLoadWaiter {
            inner: Arc::clone(&self.inner),
            request_id: self.request_id,
            cancel: Arc::clone(&self.cancel),
            abandon_on_drop: true,
        }
    }

    pub(crate) fn cancel_token(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.cancel)
    }

    pub(crate) fn cancel(&self) {
        cancel_request(&self.inner, self.request_id, "scheduler_cancel");
    }

    pub(crate) fn promote_to_high(&self, contract: FsPageLoadContract) -> bool {
        let (event, superseded, stats, was_waiting) = {
            let mut state = self.inner.state.lock().unwrap();
            let Some(request) = state.requests.get_mut(&self.request_id) else {
                return false;
            };
            let was_waiting = request.phase == RequestPhase::Waiting;
            let changed = was_waiting
                && (request.priority != FsPageLoadPriority::High || request.contract != contract);
            if was_waiting {
                request.priority = FsPageLoadPriority::High;
                request.contract = contract;
            }
            let perf = request.perf.clone();
            let priority = request.priority;
            let effective_contract = request.contract;
            let superseded = if was_waiting && contract == FsPageLoadContract::LatestSeek {
                state.remove_waiting_superseded_by_seek(perf.owner_context, Some(self.request_id))
            } else {
                Vec::new()
            };
            let stats = state.stats();
            self.inner.changed.notify_all();
            (
                changed.then_some((perf, priority, effective_contract)),
                superseded,
                stats,
                was_waiting,
            )
        };
        for request in superseded {
            emit_scheduler_event(
                "scheduler_cancel_waiting",
                &request.perf,
                request.priority,
                request.contract,
                stats,
                None,
            );
        }
        if let Some((perf, priority, effective_contract)) = event {
            emit_scheduler_event(
                "scheduler_promote",
                &perf,
                priority,
                effective_contract,
                stats,
                None,
            );
        }
        was_waiting
    }

    /// Stop Drop from interpreting an observed terminal result as cancellation.
    /// The worker permit remains the sole owner of execution completion.
    pub(crate) fn disarm(&mut self) {
        self.armed = false;
    }

    #[cfg(test)]
    pub(crate) fn is_cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }
}

impl Drop for FsPageLoadTicket {
    fn drop(&mut self) {
        if self.armed {
            self.cancel();
        }
    }
}

pub(crate) struct FsPageLoadWaiter {
    inner: Arc<SchedulerInner>,
    request_id: u64,
    cancel: Arc<AtomicBool>,
    abandon_on_drop: bool,
}

impl FsPageLoadWaiter {
    pub(crate) fn acquire_cancellable(mut self) -> Option<FsPageLoadPermit> {
        let (perf, priority, contract, stats) = {
            let mut state = self.inner.state.lock().unwrap();
            loop {
                let Some(_request) = state.requests.get(&self.request_id) else {
                    self.abandon_on_drop = false;
                    return None;
                };
                if self.cancel.load(Ordering::Relaxed) {
                    let request = state.requests.remove(&self.request_id).unwrap();
                    let stats = state.stats();
                    self.inner.changed.notify_all();
                    self.abandon_on_drop = false;
                    drop(state);
                    emit_scheduler_event(
                        "scheduler_cancel_waiting",
                        &request.perf,
                        request.priority,
                        request.contract,
                        stats,
                        None,
                    );
                    return None;
                }
                if state.can_acquire(self.request_id) {
                    break;
                }
                state = self.inner.changed.wait(state).unwrap();
            }
            let request = state.requests.get_mut(&self.request_id).unwrap();
            request.phase = RequestPhase::Running;
            let perf = request.perf.clone();
            let priority = request.priority;
            let contract = request.contract;
            let stats = state.stats();
            self.abandon_on_drop = false;
            // Leaving the waiting set changes who the head is, so the next waiter may
            // now be admissible. Without this the budget collapses to one: a waiter
            // that already parked would sleep until a permit is dropped, even while
            // capacity is free.
            self.inner.changed.notify_all();
            (perf, priority, contract, stats)
        };
        emit_scheduler_event("scheduler_acquire", &perf, priority, contract, stats, None);
        Some(FsPageLoadPermit {
            inner: Arc::clone(&self.inner),
            request_id: self.request_id,
        })
    }
}

impl Drop for FsPageLoadWaiter {
    fn drop(&mut self) {
        if self.abandon_on_drop {
            cancel_request(&self.inner, self.request_id, "scheduler_abandon_waiter");
        }
    }
}

pub(crate) struct FsPageLoadPermit {
    inner: Arc<SchedulerInner>,
    request_id: u64,
}

impl Drop for FsPageLoadPermit {
    fn drop(&mut self) {
        let Some((request, stats, cancel_to_finish_ms)) = ({
            let mut state = self.inner.state.lock().unwrap();
            let request = state.requests.remove(&self.request_id);
            let cancel_to_finish_ms = request
                .as_ref()
                .and_then(|request| request.cancel_started_at)
                .map(|started_at| started_at.elapsed().as_secs_f64() * 1000.0);
            let stats = state.stats();
            self.inner.changed.notify_all();
            request.map(|request| (request, stats, cancel_to_finish_ms))
        }) else {
            return;
        };
        emit_scheduler_event(
            "scheduler_finish",
            &request.perf,
            request.priority,
            request.contract,
            stats,
            cancel_to_finish_ms,
        );
    }
}

fn cancel_request(inner: &SchedulerInner, request_id: u64, kind: &'static str) {
    let event = {
        let mut state = inner.state.lock().unwrap();
        let Some(request) = state.requests.get_mut(&request_id) else {
            return;
        };
        request.cancel.store(true, Ordering::Relaxed);
        if request.phase == RequestPhase::Waiting {
            let request = state.requests.remove(&request_id).unwrap();
            let stats = state.stats();
            inner.changed.notify_all();
            Some((request.perf, request.priority, request.contract, stats))
        } else {
            if request.phase == RequestPhase::Running {
                request.phase = RequestPhase::Cancelling;
                request.cancel_started_at = Some(Instant::now());
            }
            let perf = request.perf.clone();
            let priority = request.priority;
            let contract = request.contract;
            let stats = state.stats();
            inner.changed.notify_all();
            Some((perf, priority, contract, stats))
        }
    };
    if let Some((perf, priority, contract, stats)) = event {
        emit_scheduler_event(kind, &perf, priority, contract, stats, None);
    }
}

fn emit_scheduler_event(
    kind: &'static str,
    perf: &PerfIdentity,
    priority: FsPageLoadPriority,
    contract: FsPageLoadContract,
    stats: FsPageLoadSchedulerStats,
    cancel_to_finish_ms: Option<f64>,
) {
    if !crate::perf::is_enabled() {
        return;
    }
    let mut fields = vec![
        ("idx", serde_json::Value::from(perf.idx)),
        ("owner_context", serde_json::Value::from(perf.owner_context)),
        ("priority", serde_json::Value::from(priority.perf_label())),
        ("contract", serde_json::Value::from(contract.perf_label())),
        ("waiting", serde_json::Value::from(stats.waiting)),
        ("running", serde_json::Value::from(stats.running)),
        ("cancelling", serde_json::Value::from(stats.cancelling)),
        (
            "total_limit",
            serde_json::Value::from(FS_PAGE_LOAD_TOTAL_PERMITS),
        ),
        (
            "normal_limit",
            serde_json::Value::from(
                FS_PAGE_LOAD_TOTAL_PERMITS - FS_PAGE_LOAD_HIGH_RESERVED_PERMITS,
            ),
        ),
    ];
    if let Some(ms) = cancel_to_finish_ms {
        fields.push(("cancel_to_finish_ms", serde_json::Value::from(ms)));
    }
    crate::perf::event("fs", kind, perf.key.as_deref(), perf.seq, &fields);
}

impl FsPageLoadPriority {
    const fn perf_label(self) -> &'static str {
        match self {
            Self::Normal => "normal",
            Self::High => "high",
        }
    }
}

/// Admission contract carried by each request.
///
/// Sequential navigation never supersedes an accepted target. A direct seek
/// makes older waiting work from the same viewer obsolete, so those requests
/// are removed before they can acquire a permit or read source bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FsPageLoadContract {
    Sequential,
    LatestSeek,
}

impl FsPageLoadContract {
    const fn perf_label(self) -> &'static str {
        match self {
            Self::Sequential => "sequential",
            Self::LatestSeek => "latest_seek",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RequestPhase {
    Waiting,
    Running,
    Cancelling,
}

#[derive(Clone)]
struct PerfIdentity {
    key: Option<String>,
    seq: u64,
    idx: usize,
    owner_context: u64,
}

struct RequestRecord {
    priority: FsPageLoadPriority,
    contract: FsPageLoadContract,
    phase: RequestPhase,
    enqueue_order: u64,
    cancel: Arc<AtomicBool>,
    cancel_started_at: Option<Instant>,
    perf: PerfIdentity,
}

struct SchedulerState {
    total: usize,
    normal_limit: usize,
    next_request_id: u64,
    next_enqueue_order: u64,
    requests: HashMap<u64, RequestRecord>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct FsPageLoadSchedulerStats {
    pub(crate) waiting: usize,
    pub(crate) running: usize,
    pub(crate) cancelling: usize,
    pub(crate) running_normal: usize,
}

impl SchedulerState {
    fn stats(&self) -> FsPageLoadSchedulerStats {
        let mut stats = FsPageLoadSchedulerStats::default();
        for request in self.requests.values() {
            match request.phase {
                RequestPhase::Waiting => stats.waiting += 1,
                RequestPhase::Running => stats.running += 1,
                RequestPhase::Cancelling => stats.cancelling += 1,
            }
            if request.priority == FsPageLoadPriority::Normal
                && matches!(
                    request.phase,
                    RequestPhase::Running | RequestPhase::Cancelling
                )
            {
                stats.running_normal += 1;
            }
        }
        stats
    }

    fn can_acquire(&self, request_id: u64) -> bool {
        let Some(request) = self.requests.get(&request_id) else {
            return false;
        };
        if request.phase != RequestPhase::Waiting {
            return false;
        }
        let stats = self.stats();
        if stats.running + stats.cancelling >= self.total {
            return false;
        }
        if request.priority == FsPageLoadPriority::Normal
            && stats.running_normal >= self.normal_limit
        {
            return false;
        }
        self.requests
            .iter()
            .filter(|(_, candidate)| candidate.phase == RequestPhase::Waiting)
            .min_by_key(|(_, candidate)| {
                (
                    match candidate.priority {
                        FsPageLoadPriority::High => 0_u8,
                        FsPageLoadPriority::Normal => 1_u8,
                    },
                    candidate.enqueue_order,
                )
            })
            .map(|(&id, _)| id)
            == Some(request_id)
    }

    fn remove_waiting_superseded_by_seek(
        &mut self,
        owner_context: u64,
        except_request_id: Option<u64>,
    ) -> Vec<RequestRecord> {
        let superseded = self
            .requests
            .iter()
            .filter_map(|(&id, request)| {
                (Some(id) != except_request_id
                    && request.perf.owner_context == owner_context
                    && request.phase == RequestPhase::Waiting)
                    .then_some(id)
            })
            .collect::<Vec<_>>();
        superseded
            .into_iter()
            .filter_map(|id| {
                let request = self.requests.remove(&id)?;
                request.cancel.store(true, Ordering::Relaxed);
                Some(request)
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;
    use std::sync::mpsc;
    use std::time::Duration;

    fn wait_until(mut predicate: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !predicate() {
            assert!(Instant::now() < deadline, "condition timed out");
            std::thread::yield_now();
        }
    }

    fn test_ticket(
        scheduler: &FsPageLoadScheduler,
        owner: u64,
        idx: usize,
        priority: FsPageLoadPriority,
        contract: FsPageLoadContract,
    ) -> FsPageLoadTicket {
        scheduler.request(owner, idx, priority, contract, None, 0)
    }

    #[test]
    fn slow_read_and_decode_stay_within_process_budget() {
        let scheduler = FsPageLoadScheduler::with_limits(3, 1);
        let gate = Arc::new((Mutex::new(false), Condvar::new()));
        let active = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));
        let mut tickets = Vec::new();
        let mut workers = Vec::new();

        for idx in 0_usize..12 {
            let priority = if idx < 2 {
                FsPageLoadPriority::High
            } else {
                FsPageLoadPriority::Normal
            };
            let ticket = test_ticket(
                &scheduler,
                (idx % 2) as u64,
                idx,
                priority,
                FsPageLoadContract::Sequential,
            );
            let waiter = ticket.waiter();
            let gate = Arc::clone(&gate);
            let active = Arc::clone(&active);
            let peak = Arc::clone(&peak);
            workers.push(std::thread::spawn(move || {
                let _permit = waiter.acquire_cancellable().expect("request admitted");
                let now = active.fetch_add(1, Ordering::SeqCst) + 1;
                peak.fetch_max(now, Ordering::SeqCst);
                let (open, changed) = &*gate;
                let mut open = open.lock().unwrap();
                while !*open {
                    open = changed.wait(open).unwrap();
                }
                active.fetch_sub(1, Ordering::SeqCst);
            }));
            tickets.push(ticket);
        }

        wait_until(|| {
            let stats = scheduler.stats();
            stats.running + stats.cancelling == 3
        });
        let stats = scheduler.stats();
        assert_eq!(stats.running, 3);
        assert!(stats.running_normal <= 2);
        assert!(peak.load(Ordering::SeqCst) <= 3);

        let (open, changed) = &*gate;
        *open.lock().unwrap() = true;
        changed.notify_all();
        for worker in workers {
            worker.join().unwrap();
        }
        assert_eq!(peak.load(Ordering::SeqCst), 3);
        assert_eq!(scheduler.stats(), FsPageLoadSchedulerStats::default());
        drop(tickets);
    }

    #[test]
    fn parked_waiter_wakes_when_the_head_starts_running() {
        // A waiter that parked while another request was ahead of it must still be
        // admitted once that request starts running and capacity is free. Without a
        // wake-up on the Waiting -> Running transition the budget collapses to one.
        let scheduler = FsPageLoadScheduler::with_limits(3, 0);
        let head = test_ticket(
            &scheduler,
            1,
            0,
            FsPageLoadPriority::High,
            FsPageLoadContract::Sequential,
        );
        let follower = test_ticket(
            &scheduler,
            1,
            1,
            FsPageLoadPriority::Normal,
            FsPageLoadContract::Sequential,
        );
        let follower_waiter = follower.waiter();
        let (admitted_tx, admitted_rx) = mpsc::channel();
        let worker = std::thread::spawn(move || {
            let _permit = follower_waiter.acquire_cancellable().expect("admitted");
            admitted_tx.send(()).unwrap();
            std::thread::sleep(Duration::from_millis(200));
        });
        // Let the follower park behind the head. No further requests arrive after this.
        std::thread::sleep(Duration::from_millis(150));
        assert!(matches!(
            admitted_rx.try_recv(),
            Err(mpsc::TryRecvError::Empty)
        ));

        let head_permit = head.waiter().acquire_cancellable().expect("head admitted");
        admitted_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("parked follower must be admitted once the head starts running");
        drop(head_permit);
        worker.join().unwrap();
        drop((head, follower));
    }

    #[test]
    fn cancel_does_not_release_permit_until_worker_finishes() {
        let scheduler = FsPageLoadScheduler::with_limits(1, 0);
        let first = test_ticket(
            &scheduler,
            1,
            0,
            FsPageLoadPriority::High,
            FsPageLoadContract::Sequential,
        );
        let first_permit = first.waiter().acquire_cancellable().unwrap();
        first.cancel();
        assert_eq!(
            scheduler.stats(),
            FsPageLoadSchedulerStats {
                cancelling: 1,
                ..Default::default()
            }
        );

        let second = test_ticket(
            &scheduler,
            1,
            1,
            FsPageLoadPriority::High,
            FsPageLoadContract::Sequential,
        );
        let second_waiter = second.waiter();
        let (started_tx, started_rx) = mpsc::channel();
        let worker = std::thread::spawn(move || {
            let _permit = second_waiter.acquire_cancellable().unwrap();
            started_tx.send(()).unwrap();
        });
        wait_until(|| scheduler.stats().waiting == 1);
        assert!(matches!(
            started_rx.try_recv(),
            Err(mpsc::TryRecvError::Empty)
        ));

        drop(first_permit);
        started_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("returned permit admits next request");
        worker.join().unwrap();
        drop((first, second));
    }

    #[test]
    fn cancelled_waiter_reads_zero_bytes() {
        let scheduler = FsPageLoadScheduler::with_limits(1, 0);
        let blocker = test_ticket(
            &scheduler,
            1,
            0,
            FsPageLoadPriority::High,
            FsPageLoadContract::Sequential,
        );
        let blocker_permit = blocker.waiter().acquire_cancellable().unwrap();
        let pending = test_ticket(
            &scheduler,
            1,
            1,
            FsPageLoadPriority::Normal,
            FsPageLoadContract::Sequential,
        );
        let waiter = pending.waiter();
        let bytes_read = Arc::new(AtomicUsize::new(0));
        let worker_bytes_read = Arc::clone(&bytes_read);
        let worker = std::thread::spawn(move || {
            if let Some(_permit) = waiter.acquire_cancellable() {
                worker_bytes_read.fetch_add(1, Ordering::SeqCst);
            }
        });
        wait_until(|| scheduler.stats().waiting == 1);
        pending.cancel();
        worker.join().unwrap();
        assert_eq!(bytes_read.load(Ordering::SeqCst), 0);
        assert_eq!(scheduler.stats().running, 1);
        drop(blocker_permit);
        drop((blocker, pending));
    }

    #[test]
    fn permit_drop_returns_budget_on_error() {
        let scheduler = FsPageLoadScheduler::with_limits(1, 0);
        let failed = test_ticket(
            &scheduler,
            1,
            0,
            FsPageLoadPriority::High,
            FsPageLoadContract::Sequential,
        );
        let result = (|| -> Result<(), &'static str> {
            let _permit = failed.waiter().acquire_cancellable().unwrap();
            Err("decode failed")
        })();
        assert_eq!(result, Err("decode failed"));
        assert_eq!(scheduler.stats(), FsPageLoadSchedulerStats::default());

        let next = test_ticket(
            &scheduler,
            1,
            1,
            FsPageLoadPriority::Normal,
            FsPageLoadContract::Sequential,
        );
        assert!(next.waiter().acquire_cancellable().is_some());
        drop((failed, next));
    }

    #[test]
    fn waiting_prefetch_is_promoted_without_cancel_or_restart() {
        let scheduler = FsPageLoadScheduler::with_limits(1, 0);
        let blocker = test_ticket(
            &scheduler,
            1,
            0,
            FsPageLoadPriority::High,
            FsPageLoadContract::Sequential,
        );
        let blocker_permit = blocker.waiter().acquire_cancellable().unwrap();
        let older = test_ticket(
            &scheduler,
            1,
            1,
            FsPageLoadPriority::Normal,
            FsPageLoadContract::Sequential,
        );
        let promoted = test_ticket(
            &scheduler,
            1,
            2,
            FsPageLoadPriority::Normal,
            FsPageLoadContract::Sequential,
        );
        promoted.promote_to_high(FsPageLoadContract::Sequential);
        assert!(!promoted.is_cancelled());

        let (order_tx, order_rx) = mpsc::channel();
        let mut workers = Vec::new();
        for (idx, waiter) in [(1, older.waiter()), (2, promoted.waiter())] {
            let order_tx = order_tx.clone();
            workers.push(std::thread::spawn(move || {
                let _permit = waiter.acquire_cancellable().unwrap();
                order_tx.send(idx).unwrap();
            }));
        }
        drop(order_tx);
        wait_until(|| scheduler.stats().waiting == 2);
        drop(blocker_permit);
        assert_eq!(order_rx.recv_timeout(Duration::from_secs(5)).unwrap(), 2);
        for worker in workers {
            worker.join().unwrap();
        }
        drop((blocker, older, promoted));
    }

    #[test]
    fn latest_seek_supersedes_only_waiting_requests_in_same_viewer() {
        let scheduler = FsPageLoadScheduler::with_limits(1, 0);
        let blocker = test_ticket(
            &scheduler,
            9,
            0,
            FsPageLoadPriority::High,
            FsPageLoadContract::Sequential,
        );
        let blocker_permit = blocker.waiter().acquire_cancellable().unwrap();
        let old_same_viewer = test_ticket(
            &scheduler,
            10,
            1,
            FsPageLoadPriority::Normal,
            FsPageLoadContract::Sequential,
        );
        let sibling_viewer = test_ticket(
            &scheduler,
            20,
            2,
            FsPageLoadPriority::Normal,
            FsPageLoadContract::Sequential,
        );
        let latest = test_ticket(
            &scheduler,
            10,
            99,
            FsPageLoadPriority::High,
            FsPageLoadContract::LatestSeek,
        );

        assert!(old_same_viewer.is_cancelled());
        assert!(!sibling_viewer.is_cancelled());
        assert!(!latest.is_cancelled());
        assert_eq!(scheduler.stats().waiting, 2);
        drop((old_same_viewer, sibling_viewer, latest));
        drop(blocker_permit);
        drop(blocker);
    }

    #[test]
    fn sequential_burst_keeps_every_accepted_target() {
        let scheduler = FsPageLoadScheduler::with_limits(1, 0);
        let blocker = test_ticket(
            &scheduler,
            1,
            0,
            FsPageLoadPriority::High,
            FsPageLoadContract::Sequential,
        );
        let blocker_permit = blocker.waiter().acquire_cancellable().unwrap();
        let requests = (1..=6)
            .map(|idx| {
                test_ticket(
                    &scheduler,
                    1,
                    idx,
                    FsPageLoadPriority::Normal,
                    FsPageLoadContract::Sequential,
                )
            })
            .collect::<Vec<_>>();
        assert_eq!(scheduler.stats().waiting, requests.len());
        assert!(requests.iter().all(|request| !request.is_cancelled()));
        drop(requests);
        drop(blocker_permit);
        drop(blocker);
    }

    #[test]
    fn cancelling_one_running_viewer_does_not_cancel_the_other() {
        let scheduler = FsPageLoadScheduler::with_limits(2, 0);
        let first = test_ticket(
            &scheduler,
            100,
            0,
            FsPageLoadPriority::High,
            FsPageLoadContract::Sequential,
        );
        let second = test_ticket(
            &scheduler,
            200,
            0,
            FsPageLoadPriority::High,
            FsPageLoadContract::Sequential,
        );
        let first_permit = first.waiter().acquire_cancellable().unwrap();
        let second_permit = second.waiter().acquire_cancellable().unwrap();
        first.cancel();

        assert!(first.is_cancelled());
        assert!(!second.is_cancelled());
        assert_eq!(scheduler.stats().cancelling, 1);
        assert_eq!(scheduler.stats().running, 1);
        drop((first_permit, second_permit));
        assert_eq!(scheduler.stats(), FsPageLoadSchedulerStats::default());
        drop((first, second));
    }
}
