//! Exclusive, block-scoped ownership of the two application DSP bridges.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Instant;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DspOwner {
    Local { pump_instance: u64, epoch: u64 },
    Remote { session: u64, generation: u64 },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DspRequest {
    pub owner: DspOwner,
    number: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HandoffWaitError {
    Cancelled,
    TimedOut,
}

#[derive(Default)]
struct State {
    next_request: u64,
    desired: Option<DspRequest>,
    granted: Option<DspRequest>,
    in_flight: usize,
    resetting: Option<DspRequest>,
    remote_owner: Option<DspRequest>,
    exiting: bool,
}

#[derive(Default)]
pub struct DspProcessingCoordinator {
    state: Mutex<State>,
    changed: Condvar,
}

impl DspProcessingCoordinator {
    /// Reservation only changes memory. The old block may finish under its existing permit.
    pub fn reserve(&self, owner: DspOwner) -> Option<DspRequest> {
        let mut state = self.state.lock().unwrap();
        if state.exiting {
            return None;
        }
        state.next_request = state.next_request.saturating_add(1);
        let request = DspRequest {
            owner,
            number: state.next_request,
        };
        state.desired = Some(request);
        state.granted = None;
        self.changed.notify_all();
        Some(request)
    }

    /// Cancel pending and granted ownership without waiting for an in-flight block.
    pub fn revoke(&self) {
        let mut state = self.state.lock().unwrap();
        state.next_request = state.next_request.saturating_add(1);
        state.desired = None;
        state.granted = None;
        self.changed.notify_all();
    }

    pub fn cancel(&self, request: DspRequest) {
        let mut state = self.state.lock().unwrap();
        if state.desired == Some(request) {
            state.desired = None;
        }
        if state.granted == Some(request) {
            state.granted = None;
        }
        if state.remote_owner == Some(request) {
            state.remote_owner = None;
        }
        self.changed.notify_all();
    }

    pub fn revoke_local(&self, pump_instance: u64) {
        let mut state = self.state.lock().unwrap();
        let belongs_to_pump = |request: DspRequest| matches!(request.owner, DspOwner::Local { pump_instance: id, .. } if id == pump_instance);
        if state.desired.is_some_and(belongs_to_pump) {
            state.desired = None;
        }
        if state.granted.is_some_and(belongs_to_pump) {
            state.granted = None;
        }
        self.changed.notify_all();
    }

    pub fn is_acquiring_local(&self, pump_instance: u64) -> bool {
        let state = self.state.lock().unwrap();
        matches!(state.desired, Some(DspRequest { owner: DspOwner::Local { pump_instance: id, .. }, .. }) if id == pump_instance)
            && state.granted != state.desired
    }

    pub fn is_granted(&self, request: DspRequest) -> bool {
        let state = self.state.lock().unwrap();
        state.granted == Some(request) && state.desired == Some(request)
    }

    pub fn is_desired(&self, request: DspRequest) -> bool {
        let state = self.state.lock().unwrap();
        !state.exiting && state.desired == Some(request)
    }

    pub fn is_latest_request(&self, request: DspRequest) -> bool {
        let state = self.state.lock().unwrap();
        !state.exiting && state.next_request == request.number
    }

    /// A displaced local claimant may keep playing dry when another local player won.
    /// Remote acquisition and exit keep every local player paused instead.
    pub fn local_dry_resume_allowed(&self) -> bool {
        let state = self.state.lock().unwrap();
        !state.exiting
            && !matches!(
                state.desired,
                Some(DspRequest {
                    owner: DspOwner::Remote { .. },
                    ..
                })
            )
            && state.remote_owner.is_none()
    }

    pub fn local_permit(self: &Arc<Self>, pump_instance: u64) -> Option<DspPermit> {
        let mut state = self.state.lock().unwrap();
        if state.exiting || state.granted != state.desired {
            return None;
        }
        if !matches!(state.granted, Some(DspRequest { owner: DspOwner::Local { pump_instance: id, .. }, .. }) if id == pump_instance)
        {
            return None;
        }
        state.in_flight += 1;
        Some(DspPermit {
            coordinator: Arc::clone(self),
        })
    }

    pub fn local_has_token(&self, pump_instance: u64) -> bool {
        let state = self.state.lock().unwrap();
        !state.exiting
            && state.granted == state.desired
            && matches!(state.granted, Some(DspRequest { owner: DspOwner::Local { pump_instance: id, .. }, .. }) if id == pump_instance)
    }

    pub fn permit(self: &Arc<Self>, request: DspRequest) -> Option<DspPermit> {
        let mut state = self.state.lock().unwrap();
        if state.granted != Some(request) || state.desired != Some(request) || state.exiting {
            return None;
        }
        state.in_flight += 1;
        Some(DspPermit {
            coordinator: Arc::clone(self),
        })
    }

    /// Wait off the UI thread until no old block or handoff reset owns the bridges.
    pub fn wait_handoff(
        self: &Arc<Self>,
        request: DspRequest,
        deadline: Instant,
        cancel: &AtomicBool,
    ) -> Result<DspHandoff, HandoffWaitError> {
        let mut state = self.state.lock().unwrap();
        loop {
            if cancel.load(Ordering::Acquire) || state.exiting || state.desired != Some(request) {
                return Err(HandoffWaitError::Cancelled);
            }
            if state.in_flight == 0 && state.resetting.is_none() && state.remote_owner.is_none() {
                state.resetting = Some(request);
                return Ok(DspHandoff {
                    coordinator: Arc::clone(self),
                    request,
                });
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                if state.desired == Some(request) {
                    state.desired = None;
                }
                self.changed.notify_all();
                return Err(HandoffWaitError::TimedOut);
            }
            let (next, _) = self.changed.wait_timeout(state, remaining).unwrap();
            state = next;
        }
    }

    /// Exit may wait for one bounded period. A timeout forbids querying either shared bridge.
    pub fn quiesce_for_exit(&self, deadline: Instant) -> bool {
        let mut state = self.state.lock().unwrap();
        state.exiting = true;
        state.next_request = state.next_request.saturating_add(1);
        state.desired = None;
        state.granted = None;
        self.changed.notify_all();
        while state.in_flight > 0 || state.resetting.is_some() || state.remote_owner.is_some() {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return false;
            }
            let (next, _) = self.changed.wait_timeout(state, remaining).unwrap();
            state = next;
        }
        true
    }
}

pub struct DspPermit {
    coordinator: Arc<DspProcessingCoordinator>,
}

impl Drop for DspPermit {
    fn drop(&mut self) {
        let mut state = self.coordinator.state.lock().unwrap();
        state.in_flight -= 1;
        self.coordinator.changed.notify_all();
    }
}

pub struct DspHandoff {
    coordinator: Arc<DspProcessingCoordinator>,
    request: DspRequest,
}

impl DspHandoff {
    /// Called only after reset and the local paused re-seek have completed.
    pub fn grant(&self) -> bool {
        let mut state = self.coordinator.state.lock().unwrap();
        if state.desired != Some(self.request)
            || state.resetting != Some(self.request)
            || state.exiting
        {
            return false;
        }
        state.granted = Some(self.request);
        if matches!(self.request.owner, DspOwner::Remote { .. }) {
            state.remote_owner = Some(self.request);
        }
        state.resetting = None;
        self.coordinator.changed.notify_all();
        true
    }

    pub fn request(&self) -> DspRequest {
        self.request
    }
}

impl Drop for DspHandoff {
    fn drop(&mut self) {
        let mut state = self.coordinator.state.lock().unwrap();
        if state.resetting == Some(self.request) {
            state.resetting = None;
        }
        self.coordinator.changed.notify_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::time::Duration;

    #[test]
    fn block_permit_fences_all_stages_and_output_commit() {
        let coordinator = Arc::new(DspProcessingCoordinator::default());
        let cancel = AtomicBool::new(false);
        let local = coordinator
            .reserve(DspOwner::Local {
                pump_instance: 1,
                epoch: 1,
            })
            .unwrap();
        coordinator
            .wait_handoff(local, Instant::now() + Duration::from_secs(1), &cancel)
            .unwrap()
            .grant();
        let block = coordinator.permit(local).unwrap();
        let remote = coordinator
            .reserve(DspOwner::Remote {
                session: 7,
                generation: 1,
            })
            .unwrap();
        assert!(coordinator.permit(local).is_none());
        let (tx, rx) = mpsc::channel();
        let worker = Arc::clone(&coordinator);
        let join = std::thread::spawn(move || {
            let cancel = AtomicBool::new(false);
            let handoff = worker
                .wait_handoff(remote, Instant::now() + Duration::from_secs(1), &cancel)
                .unwrap();
            tx.send(()).unwrap();
            assert!(handoff.grant());
        });
        assert!(rx.recv_timeout(Duration::from_millis(30)).is_err());
        drop(block);
        rx.recv_timeout(Duration::from_secs(1)).unwrap();
        join.join().unwrap();
        assert!(coordinator.permit(remote).is_some());
    }

    #[test]
    fn timed_out_request_cannot_gain_token_after_old_block_finishes() {
        let coordinator = Arc::new(DspProcessingCoordinator::default());
        let cancel = AtomicBool::new(false);
        let first = coordinator
            .reserve(DspOwner::Local {
                pump_instance: 1,
                epoch: 1,
            })
            .unwrap();
        assert!(
            coordinator
                .wait_handoff(first, Instant::now() + Duration::from_secs(1), &cancel)
                .unwrap()
                .grant()
        );
        let block = coordinator.permit(first).unwrap();
        let second = coordinator
            .reserve(DspOwner::Local {
                pump_instance: 2,
                epoch: 2,
            })
            .unwrap();
        assert_eq!(
            coordinator
                .wait_handoff(second, Instant::now() + Duration::from_millis(1), &cancel)
                .err(),
            Some(HandoffWaitError::TimedOut)
        );
        drop(block);
        assert!(!coordinator.is_granted(second));
        assert!(coordinator.permit(second).is_none());
    }

    #[test]
    fn acquiring_pump_blocks_dry_output_until_paused_reseek_and_grant() {
        let coordinator = Arc::new(DspProcessingCoordinator::default());
        let cancel = AtomicBool::new(false);
        let first = coordinator
            .reserve(DspOwner::Local {
                pump_instance: 1,
                epoch: 1,
            })
            .unwrap();
        assert!(
            coordinator
                .wait_handoff(first, Instant::now() + Duration::from_secs(1), &cancel)
                .unwrap()
                .grant()
        );
        let old_block = coordinator.permit(first).unwrap();
        let next = coordinator
            .reserve(DspOwner::Local {
                pump_instance: 2,
                epoch: 1,
            })
            .unwrap();
        assert!(coordinator.is_acquiring_local(2));
        assert!(coordinator.local_permit(2).is_none());
        assert!(coordinator.local_permit(1).is_none());
        drop(old_block);
        let handoff = coordinator
            .wait_handoff(next, Instant::now() + Duration::from_secs(1), &cancel)
            .unwrap();
        // The caller performs the paused re-seek while the pump remains acquiring.
        assert!(coordinator.is_acquiring_local(2));
        assert!(coordinator.local_permit(2).is_none());
        assert!(handoff.grant());
        assert!(!coordinator.is_acquiring_local(2));
        assert!(coordinator.local_permit(2).is_some());
    }

    #[test]
    fn latest_local_claimant_wins_after_remote_generation_returns() {
        let coordinator = Arc::new(DspProcessingCoordinator::default());
        let cancel = AtomicBool::new(false);
        let remote = coordinator
            .reserve(DspOwner::Remote {
                session: 9,
                generation: 3,
            })
            .unwrap();
        assert!(
            coordinator
                .wait_handoff(remote, Instant::now() + Duration::from_secs(1), &cancel)
                .unwrap()
                .grant()
        );
        let remote_block = coordinator.permit(remote).unwrap();
        assert!(!coordinator.local_dry_resume_allowed());
        let stale = coordinator
            .reserve(DspOwner::Local {
                pump_instance: 1,
                epoch: 1,
            })
            .unwrap();
        let latest = coordinator
            .reserve(DspOwner::Local {
                pump_instance: 2,
                epoch: 1,
            })
            .unwrap();
        assert_eq!(
            coordinator
                .wait_handoff(stale, Instant::now() + Duration::from_secs(1), &cancel)
                .err(),
            Some(HandoffWaitError::Cancelled)
        );
        drop(remote_block);
        coordinator.cancel(remote); // old generation worker releases its lease
        assert!(coordinator.local_dry_resume_allowed());
        let handoff = coordinator
            .wait_handoff(latest, Instant::now() + Duration::from_secs(1), &cancel)
            .unwrap();
        assert!(handoff.grant());
        assert!(coordinator.local_permit(1).is_none());
        assert!(coordinator.local_permit(2).is_some());
    }

    #[test]
    fn exit_quiescence_times_out_while_remote_worker_owns_bridge() {
        let coordinator = Arc::new(DspProcessingCoordinator::default());
        let cancel = AtomicBool::new(false);
        let remote = coordinator
            .reserve(DspOwner::Remote {
                session: 1,
                generation: 1,
            })
            .unwrap();
        assert!(
            coordinator
                .wait_handoff(remote, Instant::now() + Duration::from_secs(1), &cancel)
                .unwrap()
                .grant()
        );
        assert!(!coordinator.quiesce_for_exit(Instant::now() + Duration::from_millis(1)));
        assert!(!coordinator.local_dry_resume_allowed());
        assert!(coordinator.permit(remote).is_none());
        coordinator.cancel(remote);
        assert!(coordinator.quiesce_for_exit(Instant::now() + Duration::from_secs(1)));
    }
}
