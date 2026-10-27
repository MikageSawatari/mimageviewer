//! Observes winit waits without changing scheduling or rendering.

use std::{cell::Cell, sync::OnceLock, time::Instant};
use winit::event::StartCause;

/// The two start causes that carry a wait interval.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WaitCause {
    /// The wait was cancelled before its optional deadline.
    Cancelled,
    /// The requested resume time was reached.
    ResumeTimeReached,
}

/// A wait reported by winit, including evidence of intervening callbacks.
#[derive(Clone, Copy, Debug)]
pub struct WaitRecord {
    /// Why winit ended the wait.
    pub cause: WaitCause,
    /// winit's timestamp, taken after AboutToWait returns on Windows.
    pub start: Instant,
    /// Entry to new_events, before scheduling work.
    pub end: Instant,
    /// The optional WaitUntil deadline; elapsed time beyond it is not sleep.
    pub requested_resume: Option<Instant>,
    /// A callback (possibly inline painting) entered at or after start.
    pub callback_during_wait: bool,
}

type Hook = Box<dyn Fn(WaitRecord) + Send + Sync>;
static WITNESS: Witness = Witness(OnceLock::new());
thread_local! {
    // Callbacks and new_events run on the same event-loop thread. Keep other
    // event-loop threads from invalidating this thread's records.
    static LAST_CALLBACK: Cell<Option<Instant>> = const { Cell::new(None) };
}

struct Witness(OnceLock<Hook>);

impl Witness {
    fn install(&self, hook: Hook) -> bool {
        self.0.set(hook).is_ok()
    }

    fn callback_entry(&self) {
        if self.0.get().is_some() {
            LAST_CALLBACK.set(Some(Instant::now()));
        }
    }

    fn record(&self, cause: StartCause) {
        let Some(hook) = self.0.get() else { return };
        let end = Instant::now();
        if let Some(record) = wait_record(cause, end, LAST_CALLBACK.get()) {
            hook(record);
        }
    }
}

/// Installs the process-wide observer. Returns false if already installed.
pub fn install(hook: Hook) -> bool {
    WITNESS.install(hook)
}

pub(crate) fn callback_entry() {
    WITNESS.callback_entry();
}

pub(crate) fn record(cause: StartCause) {
    WITNESS.record(cause);
}

fn wait_record(cause: StartCause, end: Instant, callback: Option<Instant>) -> Option<WaitRecord> {
    let (cause, start, requested_resume) = match cause {
        StartCause::WaitCancelled {
            start,
            requested_resume,
        } => (WaitCause::Cancelled, start, requested_resume),
        StartCause::ResumeTimeReached {
            start,
            requested_resume,
        } => (WaitCause::ResumeTimeReached, start, Some(requested_resume)),
        StartCause::Init | StartCause::Poll => return None,
    };
    Some(WaitRecord {
        cause,
        start,
        end,
        requested_resume,
        callback_during_wait: callback.is_some_and(|stamp| stamp >= start),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        sync::{Arc, Mutex},
        time::Duration,
    };

    #[test]
    fn hook_dispatch_and_single_install() {
        let witness = Witness(OnceLock::new());
        let start = Instant::now();
        let cause = StartCause::WaitCancelled {
            start,
            requested_resume: None,
        };
        witness.callback_entry();
        witness.record(cause); // No hook: no timestamping or dispatch.
        let records = Arc::new(Mutex::new(Vec::new()));
        let sink = records.clone();
        assert!(witness.install(Box::new(move |r| sink.lock().unwrap().push(r))));
        assert!(!witness.install(Box::new(|_| panic!("second hook"))));
        witness.record(StartCause::Init);
        witness.record(StartCause::Poll);
        witness.record(cause);
        witness.callback_entry();
        witness.record(cause);
        let records = records.lock().unwrap();
        assert_eq!(records.len(), 2);
        assert!(!records[0].callback_during_wait);
        assert!(records[1].callback_during_wait);
        assert_eq!(records[0].start, start);
        assert!(records[0].end >= start);
    }

    #[test]
    fn causes_deadlines_and_callback_boundary() {
        let start = Instant::now();
        let end = start + Duration::from_millis(10);
        let deadline = end + Duration::from_millis(20);
        for requested_resume in [None, Some(deadline)] {
            let r = wait_record(
                StartCause::WaitCancelled {
                    start,
                    requested_resume,
                },
                end,
                None,
            )
            .unwrap();
            assert_eq!(r.cause, WaitCause::Cancelled);
            assert_eq!(r.requested_resume, requested_resume);
        }
        let cause = StartCause::ResumeTimeReached {
            start,
            requested_resume: deadline,
        };
        let r = wait_record(cause, end, Some(start)).unwrap();
        assert_eq!(r.cause, WaitCause::ResumeTimeReached);
        assert_eq!(r.requested_resume, Some(deadline));
        assert!(r.callback_during_wait);
        // Build the earlier callback without subtracting from an OS Instant.
        let later_start = start + Duration::from_millis(1);
        assert!(
            !wait_record(
                StartCause::WaitCancelled {
                    start: later_start,
                    requested_resume: None
                },
                end,
                Some(start)
            )
            .unwrap()
            .callback_during_wait
        );
    }
}
