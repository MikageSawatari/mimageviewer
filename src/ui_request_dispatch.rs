//! A queued UI request has exactly one winner: the UI or its cancelling worker.
use std::sync::atomic::{AtomicU8, Ordering};

const UI_REQUEST_PENDING: u8 = 0;
const UI_REQUEST_CLAIMED: u8 = 1;
const UI_REQUEST_CANCELLED: u8 = 2;

pub(crate) struct UiRequestDispatch {
    state: AtomicU8,
}

impl UiRequestDispatch {
    pub(crate) fn pending() -> Self {
        Self {
            state: AtomicU8::new(UI_REQUEST_PENDING),
        }
    }

    pub(crate) fn try_claim(&self) -> bool {
        self.state
            .compare_exchange(
                UI_REQUEST_PENDING,
                UI_REQUEST_CLAIMED,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_ok()
    }

    pub(crate) fn cancel_pending(&self) -> bool {
        self.state
            .compare_exchange(
                UI_REQUEST_PENDING,
                UI_REQUEST_CANCELLED,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_ok()
    }

    pub(crate) fn is_cancelled(&self) -> bool {
        self.state.load(Ordering::Acquire) == UI_REQUEST_CANCELLED
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Barrier};

    #[test]
    fn claimed_request_cannot_be_cancelled_or_claimed_again() {
        let dispatch = UiRequestDispatch::pending();
        assert!(!dispatch.is_cancelled());
        assert!(dispatch.try_claim());
        assert!(!dispatch.is_cancelled());
        assert!(!dispatch.cancel_pending());
        assert!(!dispatch.try_claim());
    }

    #[test]
    fn cancelled_request_cannot_be_claimed_or_cancelled_again() {
        let dispatch = UiRequestDispatch::pending();
        assert!(dispatch.cancel_pending());
        assert!(dispatch.is_cancelled());
        assert!(!dispatch.try_claim());
        assert!(!dispatch.cancel_pending());
    }

    #[test]
    fn simultaneous_claim_and_cancel_have_one_winner() {
        for _ in 0..32 {
            let dispatch = Arc::new(UiRequestDispatch::pending());
            let barrier = Arc::new(Barrier::new(2));
            let worker_dispatch = Arc::clone(&dispatch);
            let worker_barrier = Arc::clone(&barrier);
            let worker = std::thread::spawn(move || {
                worker_barrier.wait();
                worker_dispatch.cancel_pending()
            });
            barrier.wait();
            let claimed = dispatch.try_claim();
            assert_ne!(claimed, worker.join().unwrap());
            assert!(!dispatch.try_claim());
            assert!(!dispatch.cancel_pending());
        }
    }
}
