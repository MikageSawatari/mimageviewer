//! Instance-local barriers for deterministic cancellation composition tests.

use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
    mpsc::{self, Receiver, Sender},
};
use std::time::Duration;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum BookQueryTestPhase {
    MihPostingScan,
    SqlProgress,
    DirectCandidateSlot,
}

#[derive(Debug)]
pub(crate) struct BookQueryTestPhaseProbe {
    phase: BookQueryTestPhase,
    armed: AtomicBool,
    fired: AtomicBool,
    reached: Sender<()>,
    resume: Mutex<Receiver<()>>,
}

pub(crate) struct BookQueryTestPhaseControl {
    reached: Receiver<()>,
    resume: Option<Sender<()>>,
}

impl BookQueryTestPhaseProbe {
    pub(crate) fn new(phase: BookQueryTestPhase) -> (Arc<Self>, BookQueryTestPhaseControl) {
        let (reached_tx, reached_rx) = mpsc::channel();
        let (resume_tx, resume_rx) = mpsc::channel();
        (
            Arc::new(Self {
                phase,
                armed: AtomicBool::new(false),
                fired: AtomicBool::new(false),
                reached: reached_tx,
                resume: Mutex::new(resume_rx),
            }),
            BookQueryTestPhaseControl {
                reached: reached_rx,
                resume: Some(resume_tx),
            },
        )
    }

    /// Metadata is the first read in the request transaction. SQL probes stay dormant until this
    /// point, while MIH and direct-loop probes are thereby guaranteed to observe body work.
    pub(crate) fn arm_after_metadata(&self) {
        self.armed.store(true, Ordering::Release);
    }

    pub(crate) fn checkpoint(&self, phase: BookQueryTestPhase) {
        if self.phase != phase
            || !self.armed.load(Ordering::Acquire)
            || self
                .fired
                .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                .is_err()
        {
            return;
        }
        if self.reached.send(()).is_err() {
            return;
        }
        // Disconnecting the control also releases a worker when a test assertion unwinds.
        let _ = self
            .resume
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .recv();
    }
}

impl BookQueryTestPhaseControl {
    pub(crate) fn wait_reached(&self) {
        self.reached
            .recv_timeout(Duration::from_secs(5))
            .expect("book query did not reach the armed cancellation phase");
    }

    pub(crate) fn release(&mut self) {
        if let Some(resume) = self.resume.take() {
            let _ = resume.send(());
        }
    }
}
