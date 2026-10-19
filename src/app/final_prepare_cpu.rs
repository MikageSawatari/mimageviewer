//! App-wide limits for speculative final/comic CPU preparation.
//!
//! Admission never waits for capacity. The mutex protects only small accounting
//! records and service addresses, never work, pixels, or repaint callbacks.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};

use super::ViewerContextId;
use crate::ui_fullscreen::SpreadPageOccurrence;

const MAX_UNITS: usize = 2;
const MAX_BYTES: u64 = 1024 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(super) struct FinalPrepareUnit {
    pub(super) context: ViewerContextId,
    pub(super) items_generation: u64,
    pub(super) pages: Vec<SpreadPageOccurrence>,
}

#[derive(Clone)]
pub(super) struct FinalPrepareWake {
    pub(super) ctx: egui::Context,
    pub(super) viewport: egui::ViewportId,
}

impl FinalPrepareWake {
    fn request(&self) {
        self.ctx.request_repaint_of(egui::ViewportId::ROOT);
        if self.viewport != egui::ViewportId::ROOT {
            self.ctx.request_repaint_of(self.viewport);
        }
    }
}

#[derive(Clone)]
pub(super) struct FinalPrepareCpuBudget {
    inner: Arc<BudgetInner>,
}

struct BudgetInner {
    state: Mutex<BudgetState>,
    max_units: usize,
    max_bytes: u64,
}

#[derive(Default)]
enum Execution {
    #[default]
    Available,
    Running,
}

#[derive(Default)]
struct BudgetState {
    execution: Execution,
    /// Independent reservations in each unit, not the number of Arc clones.
    units: HashMap<FinalPrepareUnit, usize>,
    bytes: u64,
    /// One existing service address per context; no queued request or work.
    waiters: HashMap<ViewerContextId, FinalPrepareWake>,
}

impl BudgetInner {
    fn state(&self) -> MutexGuard<'_, BudgetState> {
        self.state.lock().unwrap_or_else(|error| error.into_inner())
    }

    fn can_reserve(&self, state: &BudgetState, unit: &FinalPrepareUnit, bytes: u64) -> bool {
        let unit_available = state.units.contains_key(unit)
            || (state.units.len() < self.max_units
                && !state.units.keys().any(|key| key.context == unit.context));
        unit_available && bytes <= self.max_bytes.saturating_sub(state.bytes)
    }
}

impl Default for FinalPrepareCpuBudget {
    fn default() -> Self {
        Self::new(MAX_UNITS, MAX_BYTES)
    }
}

impl FinalPrepareCpuBudget {
    fn new(max_units: usize, max_bytes: u64) -> Self {
        Self {
            inner: Arc::new(BudgetInner {
                state: Mutex::new(BudgetState::default()),
                max_units,
                max_bytes,
            }),
        }
    }

    /// Admit both resources atomically for a new speculative worker. Failed
    /// admission creates neither lease: acquiring one and returning it when
    /// the other is unavailable would wake this same service repeatedly.
    /// The worker owns the permit until real completion, and its request/
    /// output retains the reservation until registration or discard.
    pub(super) fn try_start(
        &self,
        unit: FinalPrepareUnit,
        bytes: u64,
        wake: FinalPrepareWake,
    ) -> Option<(FinalPreparePermit, Arc<FinalPrepareReservation>)> {
        let mut state = self.inner.state();
        if matches!(state.execution, Execution::Running)
            || !self.inner.can_reserve(&state, &unit, bytes)
        {
            state.waiters.insert(unit.context, wake);
            return None;
        }
        state.execution = Execution::Running;
        *state.units.entry(unit.clone()).or_default() += 1;
        state.bytes += bytes;
        Some((
            FinalPreparePermit {
                inner: Arc::clone(&self.inner),
                wake,
            },
            Arc::new(FinalPrepareReservation {
                inner: Arc::clone(&self.inner),
                unit,
                bytes,
            }),
        ))
    }

    /// Reserve output bytes before spawning. A separate call always accounts
    /// separate bytes, even for the same unit (e.g. its other page or comic).
    /// Share an existing reservation Arc for shared pixels; cloning that Arc
    /// does not reserve again. The caller includes canonical and distinct
    /// upload pixels in `bytes` and retains the lease through pending/channel/
    /// CpuReady until those outputs are registered or discarded.
    #[cfg(test)]
    pub(super) fn try_reserve(
        &self,
        unit: FinalPrepareUnit,
        bytes: u64,
        wake: FinalPrepareWake,
    ) -> Option<Arc<FinalPrepareReservation>> {
        let mut state = self.inner.state();
        if !self.inner.can_reserve(&state, &unit, bytes) {
            // Test availability and register its wake under the same lock as
            // release, so a release cannot fall between those two operations.
            state.waiters.insert(unit.context, wake);
            return None;
        }
        *state.units.entry(unit.clone()).or_default() += 1;
        state.bytes += bytes;
        Some(Arc::new(FinalPrepareReservation {
            inner: Arc::clone(&self.inner),
            unit,
            bytes,
        }))
    }

    /// Move this non-Clone permit into the worker closure before spawning.
    /// Cancel, map removal, receiver drop, park, and synchronous takeover do
    /// not release it; only closure completion/drop does.
    #[cfg(test)]
    pub(super) fn try_acquire(
        &self,
        wake: FinalPrepareWake,
        context: ViewerContextId,
    ) -> Option<FinalPreparePermit> {
        let mut state = self.inner.state();
        match state.execution {
            Execution::Available => {
                state.execution = Execution::Running;
                Some(FinalPreparePermit {
                    inner: Arc::clone(&self.inner),
                    wake,
                })
            }
            Execution::Running => {
                state.waiters.insert(context, wake);
                None
            }
        }
    }

    #[cfg(test)]
    pub(super) fn with_limits(max_units: usize, max_bytes: u64) -> Self {
        assert!(max_units <= MAX_UNITS && max_bytes <= MAX_BYTES);
        Self::new(max_units, max_bytes)
    }

    #[cfg(test)]
    pub(super) fn snapshot(&self) -> FinalPrepareCpuSnapshot {
        let state = self.inner.state();
        FinalPrepareCpuSnapshot {
            running: usize::from(matches!(state.execution, Execution::Running)),
            units: state.units.len(),
            bytes: state.bytes,
        }
    }
}

pub(super) struct FinalPrepareReservation {
    inner: Arc<BudgetInner>,
    unit: FinalPrepareUnit,
    bytes: u64,
}

impl std::fmt::Debug for FinalPrepareReservation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FinalPrepareReservation")
            .field("unit", &self.unit)
            .field("bytes", &self.bytes)
            .finish_non_exhaustive()
    }
}

impl Drop for FinalPrepareReservation {
    fn drop(&mut self) {
        let waiters = {
            let mut state = self.inner.state();
            let count = state.units.get_mut(&self.unit).expect("reserved unit");
            *count -= 1;
            if *count == 0 {
                state.units.remove(&self.unit);
            }
            state.bytes -= self.bytes;
            std::mem::take(&mut state.waiters)
        };
        for wake in waiters.into_values() {
            wake.request();
        }
    }
}

pub(super) struct FinalPreparePermit {
    inner: Arc<BudgetInner>,
    wake: FinalPrepareWake,
}

impl Drop for FinalPreparePermit {
    fn drop(&mut self) {
        let waiters = {
            let mut state = self.inner.state();
            state.execution = Execution::Available;
            std::mem::take(&mut state.waiters)
        };
        for wake in waiters.into_values() {
            wake.request();
        }
        // The result may have reached its channel without another input/tick.
        self.wake.request();
    }
}

#[cfg(test)]
#[derive(Debug, PartialEq, Eq)]
pub(super) struct FinalPrepareCpuSnapshot {
    pub(super) running: usize,
    pub(super) units: usize,
    pub(super) bytes: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unit(context: u64, page: usize) -> FinalPrepareUnit {
        FinalPrepareUnit {
            context: ViewerContextId::for_test(context),
            items_generation: 7,
            pages: vec![SpreadPageOccurrence::navigation(page, page)],
        }
    }

    fn wake() -> FinalPrepareWake {
        FinalPrepareWake {
            ctx: egui::Context::default(),
            viewport: egui::ViewportId::ROOT,
        }
    }

    fn snapshot(running: usize, units: usize, bytes: u64) -> FinalPrepareCpuSnapshot {
        FinalPrepareCpuSnapshot {
            running,
            units,
            bytes,
        }
    }

    #[test]
    fn final_prepare_cpu_pair_reservations_share_unit_and_arc_clones_share_bytes() {
        let budget = FinalPrepareCpuBudget::with_limits(2, 100);
        let mut pair = unit(1, 2);
        pair.pages.push(SpreadPageOccurrence::navigation(3, 2));
        let left = budget.try_reserve(pair.clone(), 30, wake()).unwrap();
        let right = budget.try_reserve(pair.clone(), 40, wake()).unwrap();
        let pending = Arc::clone(&left);
        let channel = Arc::clone(&left);
        assert_eq!(budget.snapshot(), snapshot(0, 1, 70));
        assert!(budget.try_reserve(pair, 31, wake()).is_none());
        drop(left);
        drop(pending);
        assert_eq!(budget.snapshot(), snapshot(0, 1, 70));
        drop(channel);
        assert_eq!(budget.snapshot(), snapshot(0, 1, 40));
        drop(right);
        assert_eq!(budget.snapshot(), snapshot(0, 0, 0));
    }

    #[test]
    fn final_prepare_cpu_context_global_and_byte_limits_are_independent() {
        let budget = FinalPrepareCpuBudget::with_limits(2, 100);
        let first = budget.try_reserve(unit(1, 0), 40, wake()).unwrap();
        assert!(budget.try_reserve(unit(1, 1), 1, wake()).is_none());
        // Even replacing the same page in another items generation is a new unit.
        let mut new_generation = unit(1, 0);
        new_generation.items_generation += 1;
        assert!(budget.try_reserve(new_generation, 1, wake()).is_none());
        assert!(budget.try_reserve(unit(2, 0), 61, wake()).is_none());
        assert!(budget.try_reserve(unit(2, 0), u64::MAX, wake()).is_none());
        let second = budget.try_reserve(unit(2, 0), 60, wake()).unwrap();
        assert!(budget.try_reserve(unit(3, 0), 0, wake()).is_none());
        assert_eq!(budget.snapshot(), snapshot(0, 2, 100));
        drop(first);
        let third = budget.try_reserve(unit(3, 0), 40, wake()).unwrap();
        assert_eq!(budget.snapshot(), snapshot(0, 2, 100));
        drop((second, third));
        assert_eq!(budget.snapshot(), snapshot(0, 0, 0));
    }

    #[test]
    fn final_prepare_cpu_cancel_map_and_receiver_drop_keep_worker_ownership() {
        let budget = FinalPrepareCpuBudget::with_limits(2, 100);
        let a = unit(1, 0);
        let lease = budget.try_reserve(a.clone(), 40, wake()).unwrap();
        let permit = budget.try_acquire(wake(), a.context).unwrap();
        let pending = Arc::clone(&lease);
        let (tx, rx) = std::sync::mpsc::channel();
        let worker = move || {
            let _permit = permit;
            let _ = tx.send(lease);
        };
        // Simulated cancellation, pending-map removal and receiver drop.
        drop(pending);
        drop(rx);
        assert_eq!(budget.snapshot(), snapshot(1, 1, 40));
        assert!(budget.try_acquire(wake(), unit(2, 0).context).is_none());
        assert!(budget.try_reserve(unit(1, 1), 1, wake()).is_none());
        worker();
        assert_eq!(budget.snapshot(), snapshot(0, 0, 0));
        let _next = budget.try_acquire(wake(), unit(2, 0).context).unwrap();
        assert_eq!(budget.snapshot().running, 1);
    }

    #[test]
    fn final_prepare_cpu_channel_keeps_result_reservation_after_worker_finishes() {
        let budget = FinalPrepareCpuBudget::with_limits(2, 100);
        let lease = budget.try_reserve(unit(1, 0), 40, wake()).unwrap();
        let permit = budget.try_acquire(wake(), unit(1, 0).context).unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        tx.send(lease).unwrap();
        drop(permit);
        assert_eq!(budget.snapshot(), snapshot(0, 1, 40));
        let cpu_ready = rx.try_recv().unwrap();
        drop(rx);
        assert_eq!(budget.snapshot(), snapshot(0, 1, 40));
        drop(cpu_ready);
        assert_eq!(budget.snapshot(), snapshot(0, 0, 0));
    }

    #[test]
    fn final_prepare_cpu_atomic_start_does_not_self_wake_while_worker_is_busy() {
        let budget = FinalPrepareCpuBudget::with_limits(2, 100);
        let (worker, reservation) = budget.try_start(unit(1, 0), 40, wake()).unwrap();
        let waiter = FinalPrepareWake {
            ctx: egui::Context::default(),
            viewport: egui::ViewportId::from_hash_of("atomic-start-waiter"),
        };
        let calls = Arc::new(Mutex::new(Vec::new()));
        waiter.ctx.set_repaint_observer({
            let calls = Arc::clone(&calls);
            move |info| calls.lock().unwrap().push(info.viewport_id)
        });
        // Capacity is available, but no temporary reservation is acquired and
        // dropped on permit failure, so an idle service receives no self-wake.
        for _ in 0..3 {
            assert!(budget.try_start(unit(2, 0), 40, waiter.clone()).is_none());
            assert_eq!(budget.snapshot(), snapshot(1, 1, 40));
        }
        assert!(calls.lock().unwrap().is_empty());
        assert_eq!(budget.inner.state().waiters.len(), 1);
        drop(worker);
        assert_eq!(
            *calls.lock().unwrap(),
            [egui::ViewportId::ROOT, waiter.viewport]
        );
        let next = budget.try_start(unit(2, 0), 40, waiter).unwrap();
        assert_eq!(budget.snapshot(), snapshot(1, 2, 80));
        drop((next, reservation));
        assert_eq!(budget.snapshot(), snapshot(0, 0, 0));
    }

    #[test]
    fn final_prepare_cpu_atomic_start_does_not_self_wake_while_capacity_is_busy() {
        let budget = FinalPrepareCpuBudget::with_limits(1, 40);
        let reservation = budget.try_reserve(unit(1, 0), 40, wake()).unwrap();
        let waiter = wake();
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        waiter.ctx.set_repaint_observer({
            let calls = Arc::clone(&calls);
            move |_| {
                calls.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            }
        });
        // The worker slot is free, but admission must not acquire/drop a
        // permit (and its own completion wake) on reservation failure.
        for _ in 0..3 {
            assert!(budget.try_start(unit(2, 0), 40, waiter.clone()).is_none());
            assert_eq!(budget.snapshot(), snapshot(0, 1, 40));
        }
        assert_eq!(calls.load(std::sync::atomic::Ordering::Relaxed), 0);
        drop(reservation);
        assert_eq!(calls.load(std::sync::atomic::Ordering::Relaxed), 1);
        let _next = budget.try_start(unit(2, 0), 40, waiter).unwrap();
        assert_eq!(budget.snapshot(), snapshot(1, 1, 40));
    }

    #[test]
    fn final_prepare_cpu_panic_and_rejected_spawn_release_closure_resources() {
        let budget = FinalPrepareCpuBudget::with_limits(2, 100);
        let lease = budget.try_reserve(unit(1, 0), 40, wake()).unwrap();
        let permit = budget.try_acquire(wake(), unit(1, 0).context).unwrap();
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
            let _lease = lease;
            let _permit = permit;
            panic!("simulated preparation panic");
        }));
        assert!(result.is_err());
        assert_eq!(budget.snapshot(), snapshot(0, 0, 0));

        fn reject_spawn(_worker: impl FnOnce() + Send + 'static) -> std::io::Result<()> {
            Err(std::io::Error::other("simulated spawn rejection"))
        }
        let lease = budget.try_reserve(unit(1, 0), 40, wake()).unwrap();
        let permit = budget.try_acquire(wake(), unit(1, 0).context).unwrap();
        let result = reject_spawn(move || {
            let _lease = lease;
            let _permit = permit;
        });
        assert!(result.is_err());
        assert_eq!(budget.snapshot(), snapshot(0, 0, 0));
    }

    #[test]
    fn final_prepare_cpu_permit_release_wakes_blocked_service_without_input() {
        let budget = FinalPrepareCpuBudget::with_limits(2, 100);
        let owner = wake();
        let waiter = FinalPrepareWake {
            ctx: egui::Context::default(),
            viewport: egui::ViewportId::from_hash_of("cpu-waiter"),
        };
        let calls = Arc::new(Mutex::new(Vec::new()));
        waiter.ctx.set_request_repaint_callback({
            let calls = Arc::clone(&calls);
            let budget = budget.clone();
            move |info| {
                // Re-entering the accounting lock proves callbacks are outside it.
                assert_eq!(budget.snapshot().running, 0);
                calls.lock().unwrap().push(info.viewport_id);
            }
        });
        let owner_called = Arc::new(std::sync::atomic::AtomicBool::new(false));
        owner.ctx.set_request_repaint_callback({
            let owner_called = Arc::clone(&owner_called);
            move |_| owner_called.store(true, std::sync::atomic::Ordering::Relaxed)
        });
        let worker = budget.try_acquire(owner, unit(1, 0).context).unwrap();
        assert!(
            budget
                .try_acquire(waiter.clone(), unit(2, 0).context)
                .is_none()
        );
        assert!(calls.lock().unwrap().is_empty());
        // No timer, input, or poll occurs between registration and completion.
        drop(worker);
        assert_eq!(
            *calls.lock().unwrap(),
            [egui::ViewportId::ROOT, waiter.viewport]
        );
        assert!(owner_called.load(std::sync::atomic::Ordering::Relaxed));
        let _next = budget.try_acquire(waiter, unit(2, 0).context).unwrap();
    }

    #[test]
    fn final_prepare_cpu_reservation_release_wakes_latest_service_for_context() {
        let budget = FinalPrepareCpuBudget::with_limits(1, 40);
        let reservation = budget.try_reserve(unit(1, 0), 40, wake()).unwrap();
        let old_wake = wake();
        let old_called = Arc::new(std::sync::atomic::AtomicBool::new(false));
        old_wake.ctx.set_request_repaint_callback({
            let old_called = Arc::clone(&old_called);
            move |_| old_called.store(true, std::sync::atomic::Ordering::Relaxed)
        });
        assert!(
            budget
                .try_reserve(unit(2, 0), 40, old_wake.clone())
                .is_none()
        );
        let new_wake = FinalPrepareWake {
            ctx: egui::Context::default(),
            viewport: egui::ViewportId::from_hash_of("new-binding"),
        };
        let calls = Arc::new(Mutex::new(Vec::new()));
        new_wake.ctx.set_request_repaint_callback({
            let budget = budget.clone();
            let calls = Arc::clone(&calls);
            move |info| {
                assert_eq!(budget.snapshot(), snapshot(0, 0, 0));
                calls.lock().unwrap().push(info.viewport_id);
            }
        });
        assert!(
            budget
                .try_reserve(unit(2, 0), 40, new_wake.clone())
                .is_none()
        );
        assert_eq!(budget.inner.state().waiters.len(), 1);
        drop(reservation);
        assert_eq!(
            *calls.lock().unwrap(),
            [egui::ViewportId::ROOT, new_wake.viewport]
        );
        assert!(!old_called.load(std::sync::atomic::Ordering::Relaxed));
        let _ready = budget.try_reserve(unit(2, 0), 40, new_wake).unwrap();
    }
}
