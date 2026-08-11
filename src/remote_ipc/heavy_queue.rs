//! Bounded, three-lane blocking queue for remote heavy work.
//!
//! The queue owns ordering and worker reservation only. It does not infer page demand or cancel a
//! payload. Pruned and rejected payloads are always returned to the caller so stage 3 can complete
//! the client reply explicitly.

use std::collections::{HashMap, VecDeque};
use std::hash::Hash;
use std::sync::{Condvar, Mutex};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum HeavyQueueLane {
    Foreground,
    Interactive,
    Prefetch,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct HeavyQueueCapacities {
    pub(super) foreground: usize,
    pub(super) interactive: usize,
    pub(super) prefetch: usize,
}

impl HeavyQueueCapacities {
    fn for_lane(self, lane: HeavyQueueLane) -> usize {
        match lane {
            HeavyQueueLane::Foreground => self.foreground,
            HeavyQueueLane::Interactive => self.interactive,
            HeavyQueueLane::Prefetch => self.prefetch,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct HeavyQueueLaneSnapshot {
    pub(super) queued: usize,
    pub(super) active: usize,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct HeavyQueueSnapshot {
    pub(super) foreground: HeavyQueueLaneSnapshot,
    pub(super) interactive: HeavyQueueLaneSnapshot,
    pub(super) prefetch: HeavyQueueLaneSnapshot,
    pub(super) shutdown: bool,
}

impl HeavyQueueSnapshot {
    pub(super) fn queued(&self) -> usize {
        self.foreground.queued + self.interactive.queued + self.prefetch.queued
    }

    pub(super) fn active(&self) -> usize {
        self.foreground.active + self.interactive.active + self.prefetch.active
    }
}

#[derive(Debug)]
pub(super) struct HeavyQueueItem<K, T> {
    key: K,
    payload: T,
    lane: HeavyQueueLane,
}

impl<K, T> HeavyQueueItem<K, T> {
    pub(super) fn key(&self) -> &K {
        &self.key
    }

    pub(super) fn payload(&self) -> &T {
        &self.payload
    }

    pub(super) fn lane(&self) -> HeavyQueueLane {
        self.lane
    }

    pub(super) fn into_parts(self) -> (K, T, HeavyQueueLane) {
        (self.key, self.payload, self.lane)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum HeavyQueuePushErrorKind {
    LaneFull,
    DuplicateKey,
    Shutdown,
}

#[derive(Debug)]
pub(super) struct HeavyQueuePushError<K, T> {
    kind: HeavyQueuePushErrorKind,
    item: HeavyQueueItem<K, T>,
}

impl<K, T> HeavyQueuePushError<K, T> {
    pub(super) fn kind(&self) -> HeavyQueuePushErrorKind {
        self.kind
    }

    pub(super) fn into_item(self) -> HeavyQueueItem<K, T> {
        self.item
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum PromoteHeavyQueueResult {
    Promoted,
    AlreadyForeground,
    NotPrefetch { lane: HeavyQueueLane },
    Running { lane: HeavyQueueLane },
    UnknownKey,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum CompleteHeavyQueueResult {
    Completed { lane: HeavyQueueLane },
    UnknownKey,
}

struct HeavyQueueState<K, T> {
    foreground: VecDeque<HeavyQueueItem<K, T>>,
    interactive: VecDeque<HeavyQueueItem<K, T>>,
    prefetch: VecDeque<HeavyQueueItem<K, T>>,
    active: HashMap<K, HeavyQueueLane>,
    shutdown: bool,
}

impl<K, T> Default for HeavyQueueState<K, T> {
    fn default() -> Self {
        Self {
            foreground: VecDeque::new(),
            interactive: VecDeque::new(),
            prefetch: VecDeque::new(),
            active: HashMap::new(),
            shutdown: false,
        }
    }
}

pub(super) struct HeavyQueue<K, T> {
    worker_count: usize,
    capacities: HeavyQueueCapacities,
    state: Mutex<HeavyQueueState<K, T>>,
    ready: Condvar,
}

impl<K, T> HeavyQueue<K, T>
where
    K: Clone + Eq + Hash,
{
    pub(super) fn new(worker_count: usize, capacities: HeavyQueueCapacities) -> Self {
        assert_ne!(worker_count, 0);
        Self {
            worker_count,
            capacities,
            state: Mutex::new(HeavyQueueState::default()),
            ready: Condvar::new(),
        }
    }

    pub(super) fn push(
        &self,
        key: K,
        lane: HeavyQueueLane,
        payload: T,
    ) -> Result<(), HeavyQueuePushError<K, T>> {
        let item = HeavyQueueItem { key, payload, lane };
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        let error_kind = if state.shutdown {
            Some(HeavyQueuePushErrorKind::Shutdown)
        } else if state.active.contains_key(item.key()) || queued_contains(&state, item.key()) {
            Some(HeavyQueuePushErrorKind::DuplicateKey)
        } else if lane_len(&state, lane) >= self.capacities.for_lane(lane) {
            Some(HeavyQueuePushErrorKind::LaneFull)
        } else {
            None
        };
        if let Some(kind) = error_kind {
            return Err(HeavyQueuePushError { kind, item });
        }
        lane_mut(&mut state, lane).push_back(item);
        self.ready.notify_one();
        Ok(())
    }

    pub(super) fn pop(&self) -> Option<HeavyQueueItem<K, T>> {
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        loop {
            if state.shutdown {
                return None;
            }
            if let Some(item) = self.pop_ready(&mut state) {
                state.active.insert(item.key.clone(), item.lane);
                return Some(item);
            }
            state = self
                .ready
                .wait(state)
                .unwrap_or_else(|error| error.into_inner());
        }
    }

    fn pop_ready(&self, state: &mut HeavyQueueState<K, T>) -> Option<HeavyQueueItem<K, T>> {
        let active = state.active.len();
        if active >= self.worker_count {
            return None;
        }
        if let Some(item) = state.foreground.pop_front() {
            return Some(item);
        }
        if let Some(item) = state.interactive.pop_front() {
            return Some(item);
        }
        // Prefetch alone cannot occupy the last worker. This is not a dedicated foreground
        // reservation: foreground and interactive work may both claim that worker, and a later
        // foreground arrival can wait while interactive work keeps the pool full.
        if active < self.worker_count.saturating_sub(1) {
            return state.prefetch.pop_front();
        }
        None
    }

    pub(super) fn complete(&self, key: &K) -> CompleteHeavyQueueResult {
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        let Some(lane) = state.active.remove(key) else {
            return CompleteHeavyQueueResult::UnknownKey;
        };
        // A worker may be asleep only because the prefetch reservation was full.
        self.ready.notify_all();
        CompleteHeavyQueueResult::Completed { lane }
    }

    pub(super) fn promote(&self, key: &K) -> PromoteHeavyQueueResult {
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        if state.foreground.iter().any(|item| item.key() == key) {
            return PromoteHeavyQueueResult::AlreadyForeground;
        }
        if state.interactive.iter().any(|item| item.key() == key) {
            return PromoteHeavyQueueResult::NotPrefetch {
                lane: HeavyQueueLane::Interactive,
            };
        }
        if let Some(lane) = state.active.get(key).copied() {
            return PromoteHeavyQueueResult::Running { lane };
        }
        let Some(position) = state.prefetch.iter().position(|item| item.key() == key) else {
            return PromoteHeavyQueueResult::UnknownKey;
        };
        let mut item = state.prefetch.remove(position).unwrap();
        item.lane = HeavyQueueLane::Foreground;
        // Promotion is not new admission, so direct foreground push capacity does not reject it.
        state.foreground.push_back(item);
        self.ready.notify_all();
        PromoteHeavyQueueResult::Promoted
    }

    pub(super) fn prune(
        &self,
        mut should_prune: impl FnMut(&K, &T, HeavyQueueLane) -> bool,
    ) -> Vec<HeavyQueueItem<K, T>> {
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        let mut removed = Vec::new();
        prune_lane(
            &mut state.foreground,
            HeavyQueueLane::Foreground,
            &mut should_prune,
            &mut removed,
        );
        prune_lane(
            &mut state.interactive,
            HeavyQueueLane::Interactive,
            &mut should_prune,
            &mut removed,
        );
        prune_lane(
            &mut state.prefetch,
            HeavyQueueLane::Prefetch,
            &mut should_prune,
            &mut removed,
        );
        removed
    }

    pub(super) fn shutdown(&self) -> Vec<HeavyQueueItem<K, T>> {
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        if state.shutdown {
            return Vec::new();
        }
        state.shutdown = true;
        let mut removed = Vec::with_capacity(
            state.foreground.len() + state.interactive.len() + state.prefetch.len(),
        );
        removed.extend(state.foreground.drain(..));
        removed.extend(state.interactive.drain(..));
        removed.extend(state.prefetch.drain(..));
        self.ready.notify_all();
        removed
    }

    pub(super) fn snapshot(&self) -> HeavyQueueSnapshot {
        let state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        let mut snapshot = HeavyQueueSnapshot {
            foreground: HeavyQueueLaneSnapshot {
                queued: state.foreground.len(),
                active: 0,
            },
            interactive: HeavyQueueLaneSnapshot {
                queued: state.interactive.len(),
                active: 0,
            },
            prefetch: HeavyQueueLaneSnapshot {
                queued: state.prefetch.len(),
                active: 0,
            },
            shutdown: state.shutdown,
        };
        for lane in state.active.values() {
            match lane {
                HeavyQueueLane::Foreground => snapshot.foreground.active += 1,
                HeavyQueueLane::Interactive => snapshot.interactive.active += 1,
                HeavyQueueLane::Prefetch => snapshot.prefetch.active += 1,
            }
        }
        snapshot
    }
}

fn queued_contains<K, T>(state: &HeavyQueueState<K, T>, key: &K) -> bool
where
    K: Eq,
{
    state.foreground.iter().any(|item| item.key() == key)
        || state.interactive.iter().any(|item| item.key() == key)
        || state.prefetch.iter().any(|item| item.key() == key)
}

fn lane_len<K, T>(state: &HeavyQueueState<K, T>, lane: HeavyQueueLane) -> usize {
    match lane {
        HeavyQueueLane::Foreground => state.foreground.len(),
        HeavyQueueLane::Interactive => state.interactive.len(),
        HeavyQueueLane::Prefetch => state.prefetch.len(),
    }
}

fn lane_mut<K, T>(
    state: &mut HeavyQueueState<K, T>,
    lane: HeavyQueueLane,
) -> &mut VecDeque<HeavyQueueItem<K, T>> {
    match lane {
        HeavyQueueLane::Foreground => &mut state.foreground,
        HeavyQueueLane::Interactive => &mut state.interactive,
        HeavyQueueLane::Prefetch => &mut state.prefetch,
    }
}

fn prune_lane<K, T>(
    lane: &mut VecDeque<HeavyQueueItem<K, T>>,
    lane_kind: HeavyQueueLane,
    should_prune: &mut impl FnMut(&K, &T, HeavyQueueLane) -> bool,
    removed: &mut Vec<HeavyQueueItem<K, T>>,
) {
    let mut kept = VecDeque::with_capacity(lane.len());
    while let Some(item) = lane.pop_front() {
        if should_prune(item.key(), item.payload(), lane_kind) {
            removed.push(item);
        } else {
            kept.push_back(item);
        }
    }
    *lane = kept;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Barrier, mpsc};
    use std::time::Duration;

    /// The negative waits below only need to be long enough to observe "nothing happened yet",
    /// so they stay short. The positive waits must not fail merely because the machine is busy
    /// compiling, so they are generous: a passing test never spends this long.
    const WAKE_TIMEOUT: Duration = Duration::from_secs(10);

    fn capacities(each: usize) -> HeavyQueueCapacities {
        HeavyQueueCapacities {
            foreground: each,
            interactive: each,
            prefetch: each,
        }
    }

    #[test]
    fn lanes_are_priority_ordered_and_each_lane_is_fifo() {
        let queue = HeavyQueue::new(8, capacities(8));
        queue.push("p1", HeavyQueueLane::Prefetch, 1).unwrap();
        queue.push("i1", HeavyQueueLane::Interactive, 2).unwrap();
        queue.push("f1", HeavyQueueLane::Foreground, 3).unwrap();
        queue.push("p2", HeavyQueueLane::Prefetch, 4).unwrap();
        queue.push("i2", HeavyQueueLane::Interactive, 5).unwrap();
        queue.push("f2", HeavyQueueLane::Foreground, 6).unwrap();

        let popped = (0..6)
            .map(|_| queue.pop().unwrap().into_parts())
            .collect::<Vec<_>>();
        assert_eq!(
            popped,
            vec![
                ("f1", 3, HeavyQueueLane::Foreground),
                ("f2", 6, HeavyQueueLane::Foreground),
                ("i1", 2, HeavyQueueLane::Interactive),
                ("i2", 5, HeavyQueueLane::Interactive),
                ("p1", 1, HeavyQueueLane::Prefetch),
                ("p2", 4, HeavyQueueLane::Prefetch),
            ]
        );
    }

    #[test]
    fn foreground_admission_is_independent_from_a_full_prefetch_lane() {
        let queue = HeavyQueue::new(
            2,
            HeavyQueueCapacities {
                foreground: 1,
                interactive: 1,
                prefetch: 1,
            },
        );
        queue.push("p1", HeavyQueueLane::Prefetch, 1).unwrap();
        let error = queue.push("p2", HeavyQueueLane::Prefetch, 2).unwrap_err();
        assert_eq!(error.kind(), HeavyQueuePushErrorKind::LaneFull);
        assert_eq!(error.into_item().into_parts().1, 2);
        queue.push("f1", HeavyQueueLane::Foreground, 3).unwrap();
        assert_eq!(queue.snapshot().foreground.queued, 1);
    }

    #[test]
    fn prefetch_uses_only_n_minus_one_workers_and_completion_wakes_the_next() {
        let queue = Arc::new(HeavyQueue::new(3, capacities(4)));
        queue.push("p1", HeavyQueueLane::Prefetch, 1).unwrap();
        queue.push("p2", HeavyQueueLane::Prefetch, 2).unwrap();
        queue.push("p3", HeavyQueueLane::Prefetch, 3).unwrap();
        let first = queue.pop().unwrap();
        let second = queue.pop().unwrap();
        assert_eq!(queue.snapshot().prefetch.active, 2);

        let (tx, rx) = mpsc::channel();
        let worker_queue = Arc::clone(&queue);
        let worker = std::thread::spawn(move || tx.send(worker_queue.pop()).unwrap());
        assert!(matches!(
            rx.recv_timeout(Duration::from_millis(50)),
            Err(mpsc::RecvTimeoutError::Timeout)
        ));
        assert_eq!(
            queue.complete(first.key()),
            CompleteHeavyQueueResult::Completed {
                lane: HeavyQueueLane::Prefetch,
            }
        );
        let third = rx.recv_timeout(WAKE_TIMEOUT).unwrap().unwrap();
        assert_eq!(*third.key(), "p3");
        worker.join().unwrap();
        assert_eq!(*second.key(), "p2");
    }

    #[test]
    fn foreground_arrival_uses_the_worker_left_idle_by_prefetch() {
        let queue = Arc::new(HeavyQueue::new(3, capacities(4)));
        queue.push("p1", HeavyQueueLane::Prefetch, 1).unwrap();
        queue.push("p2", HeavyQueueLane::Prefetch, 2).unwrap();
        let _first = queue.pop().unwrap();
        let _second = queue.pop().unwrap();
        assert_eq!(queue.snapshot().active(), 2);

        let (tx, rx) = mpsc::channel();
        let worker_queue = Arc::clone(&queue);
        let worker = std::thread::spawn(move || tx.send(worker_queue.pop()).unwrap());
        assert!(matches!(
            rx.recv_timeout(Duration::from_millis(50)),
            Err(mpsc::RecvTimeoutError::Timeout)
        ));

        queue
            .push("foreground", HeavyQueueLane::Foreground, 3)
            .unwrap();
        let foreground = rx.recv_timeout(WAKE_TIMEOUT).unwrap().unwrap();
        assert_eq!(
            foreground.into_parts(),
            ("foreground", 3, HeavyQueueLane::Foreground)
        );
        worker.join().unwrap();
    }

    #[test]
    fn foreground_waits_when_interactive_work_already_fills_the_pool() {
        let queue = Arc::new(HeavyQueue::new(3, capacities(4)));
        queue.push("p1", HeavyQueueLane::Prefetch, 1).unwrap();
        queue.push("p2", HeavyQueueLane::Prefetch, 2).unwrap();
        let _first_prefetch = queue.pop().unwrap();
        let _second_prefetch = queue.pop().unwrap();

        queue
            .push("interactive", HeavyQueueLane::Interactive, 3)
            .unwrap();
        let interactive = queue.pop().unwrap();
        assert_eq!(interactive.lane(), HeavyQueueLane::Interactive);
        assert_eq!(queue.snapshot().active(), 3);

        queue
            .push("foreground", HeavyQueueLane::Foreground, 4)
            .unwrap();
        let (tx, rx) = mpsc::channel();
        let worker_queue = Arc::clone(&queue);
        let worker = std::thread::spawn(move || tx.send(worker_queue.pop()).unwrap());
        assert!(matches!(
            rx.recv_timeout(Duration::from_millis(50)),
            Err(mpsc::RecvTimeoutError::Timeout)
        ));

        assert_eq!(
            queue.complete(interactive.key()),
            CompleteHeavyQueueResult::Completed {
                lane: HeavyQueueLane::Interactive,
            }
        );
        let foreground = rx.recv_timeout(WAKE_TIMEOUT).unwrap().unwrap();
        assert_eq!(
            foreground.into_parts(),
            ("foreground", 4, HeavyQueueLane::Foreground)
        );
        worker.join().unwrap();
    }

    #[test]
    fn single_worker_pool_does_not_dispatch_prefetch() {
        let queue = Arc::new(HeavyQueue::new(1, capacities(1)));
        queue.push("prefetch", HeavyQueueLane::Prefetch, 1).unwrap();

        let (tx, rx) = mpsc::channel();
        let worker_queue = Arc::clone(&queue);
        let worker = std::thread::spawn(move || tx.send(worker_queue.pop()).unwrap());
        assert!(matches!(
            rx.recv_timeout(Duration::from_millis(50)),
            Err(mpsc::RecvTimeoutError::Timeout)
        ));
        assert_eq!(queue.snapshot().prefetch.queued, 1);

        let drained = queue.shutdown();
        assert_eq!(drained.len(), 1);
        assert!(rx.recv_timeout(WAKE_TIMEOUT).unwrap().is_none());
        worker.join().unwrap();
    }

    #[test]
    fn promote_moves_only_waiting_prefetch_and_every_noop_is_typed() {
        let queue = HeavyQueue::new(3, capacities(4));
        queue.push("page", HeavyQueueLane::Prefetch, 1).unwrap();
        assert_eq!(queue.promote(&"page"), PromoteHeavyQueueResult::Promoted);
        assert_eq!(
            queue.promote(&"page"),
            PromoteHeavyQueueResult::AlreadyForeground
        );
        let item = queue.pop().unwrap();
        assert_eq!(item.lane(), HeavyQueueLane::Foreground);
        assert_eq!(
            queue.promote(&"page"),
            PromoteHeavyQueueResult::Running {
                lane: HeavyQueueLane::Foreground,
            }
        );
        assert_eq!(
            queue.complete(&"page"),
            CompleteHeavyQueueResult::Completed {
                lane: HeavyQueueLane::Foreground,
            }
        );
        assert_eq!(queue.promote(&"page"), PromoteHeavyQueueResult::UnknownKey);
        assert_eq!(
            queue.complete(&"page"),
            CompleteHeavyQueueResult::UnknownKey
        );
    }

    #[test]
    fn prune_returns_every_removed_payload_to_the_caller() {
        let queue = HeavyQueue::new(3, capacities(4));
        queue.push("keep", HeavyQueueLane::Foreground, 1).unwrap();
        queue
            .push("drop-i", HeavyQueueLane::Interactive, 2)
            .unwrap();
        queue.push("drop-p", HeavyQueueLane::Prefetch, 3).unwrap();

        let removed = queue.prune(|key, _, _| key.starts_with("drop"));
        assert_eq!(
            removed
                .into_iter()
                .map(HeavyQueueItem::into_parts)
                .collect::<Vec<_>>(),
            vec![
                ("drop-i", 2, HeavyQueueLane::Interactive),
                ("drop-p", 3, HeavyQueueLane::Prefetch),
            ]
        );
        assert_eq!(queue.snapshot().queued(), 1);
    }

    #[test]
    fn shutdown_wakes_every_blocking_worker() {
        let queue = Arc::new(HeavyQueue::<usize, usize>::new(3, capacities(3)));
        let barrier = Arc::new(Barrier::new(4));
        let (tx, rx) = mpsc::channel();
        let mut workers = Vec::new();
        for _ in 0..3 {
            let worker_queue = Arc::clone(&queue);
            let worker_barrier = Arc::clone(&barrier);
            let worker_tx = tx.clone();
            workers.push(std::thread::spawn(move || {
                worker_barrier.wait();
                worker_tx.send(worker_queue.pop()).unwrap();
            }));
        }
        barrier.wait();
        assert!(queue.shutdown().is_empty());

        for _ in 0..3 {
            assert!(rx.recv_timeout(WAKE_TIMEOUT).unwrap().is_none());
        }
        for worker in workers {
            worker.join().unwrap();
        }
        assert!(queue.snapshot().shutdown);
    }

    #[test]
    fn snapshot_reports_queued_and_active_counts_by_lane() {
        let queue = HeavyQueue::new(4, capacities(4));
        queue.push("f1", HeavyQueueLane::Foreground, 1).unwrap();
        queue.push("f2", HeavyQueueLane::Foreground, 2).unwrap();
        queue.push("i1", HeavyQueueLane::Interactive, 3).unwrap();
        queue.push("p1", HeavyQueueLane::Prefetch, 4).unwrap();

        let active_foreground = queue.pop().unwrap();
        let active_foreground_2 = queue.pop().unwrap();
        let active_interactive = queue.pop().unwrap();
        let snapshot = queue.snapshot();
        assert_eq!(
            snapshot,
            HeavyQueueSnapshot {
                foreground: HeavyQueueLaneSnapshot {
                    queued: 0,
                    active: 2
                },
                interactive: HeavyQueueLaneSnapshot {
                    queued: 0,
                    active: 1
                },
                prefetch: HeavyQueueLaneSnapshot {
                    queued: 1,
                    active: 0
                },
                shutdown: false,
            }
        );
        assert_eq!(snapshot.queued(), 1);
        assert_eq!(snapshot.active(), 3);
        assert_eq!(active_foreground.lane(), HeavyQueueLane::Foreground);
        assert_eq!(active_foreground_2.lane(), HeavyQueueLane::Foreground);
        assert_eq!(active_interactive.lane(), HeavyQueueLane::Interactive);
    }

    #[test]
    fn duplicate_and_shutdown_pushes_return_the_payload() {
        let queue = HeavyQueue::new(2, capacities(2));
        queue.push("same", HeavyQueueLane::Foreground, 1).unwrap();
        let duplicate = queue.push("same", HeavyQueueLane::Prefetch, 2).unwrap_err();
        assert_eq!(duplicate.kind(), HeavyQueuePushErrorKind::DuplicateKey);
        assert_eq!(duplicate.into_item().into_parts().1, 2);

        let drained = queue.shutdown();
        assert_eq!(drained.len(), 1);
        let stopped = queue
            .push("new", HeavyQueueLane::Foreground, 3)
            .unwrap_err();
        assert_eq!(stopped.kind(), HeavyQueuePushErrorKind::Shutdown);
        assert_eq!(stopped.into_item().into_parts().1, 3);
    }
}
