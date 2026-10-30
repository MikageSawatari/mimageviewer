//! Optional diagnostic job ownership on the normal inference worker thread.
//! This scope does not propagate to blender/native worker threads and is never
//! used from panic/native exception/device-lost handlers.

use crate::previous_run_diagnostics::checkpoint::{JobLease, JobUpdate};
use std::cell::RefCell;
use std::marker::PhantomData;
use std::rc::Rc;

thread_local! {
    static CURRENT_JOB: RefCell<Option<JobLease>> = const { RefCell::new(None) };
}

/// A scope must be dropped on the thread that installed it. Keeping only a
/// diagnostic lease here does not retain an image, Session, or inference job.
pub(crate) struct Guard {
    previous: Option<JobLease>,
    _same_thread: PhantomData<Rc<()>>,
}

/// An absent job explicitly masks an outer scope, so an unrelated request
/// cannot accidentally publish into the preceding request's record.
pub(crate) fn enter(job: Option<JobLease>) -> Guard {
    Guard {
        previous: CURRENT_JOB.with(|current| current.replace(job)),
        _same_thread: PhantomData,
    }
}

/// Publish one coherent update; the lease owns its immutable job identity.
/// No adopted TLS job means no action, and publication never waits for a lock.
pub(crate) fn publish(update: JobUpdate) {
    CURRENT_JOB.with(|current| {
        if let Some(job) = current.borrow().as_ref() {
            job.update(update);
        }
    });
}

impl Drop for Guard {
    fn drop(&mut self) {
        // Drop the outgoing lease after releasing the RefCell borrow. Its final
        // drop may publish an activity boundary, but never invalidates a peer.
        let outgoing = CURRENT_JOB.with(|current| current.replace(self.previous.take()));
        drop(outgoing);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::previous_run_diagnostics::checkpoint::{JobMetadata, JobStage};
    use crate::previous_run_diagnostics::test_support;
    use std::sync::Arc;
    use std::sync::atomic::AtomicBool;

    fn current_id() -> Option<u64> {
        CURRENT_JOB.with(|current| current.borrow().as_ref().map(|job| job.metadata().job_id))
    }

    #[test]
    fn absent_scope_and_unwind_leave_no_adopted_job() {
        assert!(CURRENT_JOB.with(|current| current.borrow().is_none()));
        publish(JobUpdate {
            stage: JobStage::ScaleDetect,
            ..JobUpdate::default()
        });
        let result = std::panic::catch_unwind(|| {
            let _outer = enter(None);
            let _inner = enter(None);
            publish(JobUpdate {
                stage: JobStage::TileRun,
                ..JobUpdate::default()
            });
            panic!("diagnostic scope fixture unwind");
        });
        assert!(result.is_err());
        assert!(CURRENT_JOB.with(|current| current.borrow().is_none()));
    }

    #[test]
    fn adopted_scope_restores_job_identity_masks_absent_requests_and_stays_thread_local() {
        let publisher = test_support::publisher();
        let first = publisher.begin_job(
            JobMetadata {
                job_id: 101,
                ..JobMetadata::default()
            },
            Arc::new(AtomicBool::new(false)),
        );
        let second = publisher.begin_job(
            JobMetadata {
                job_id: 202,
                ..JobMetadata::default()
            },
            Arc::new(AtomicBool::new(false)),
        );
        {
            let _outer = enter(Some(first.clone()));
            assert_eq!(current_id(), Some(101));
            std::thread::spawn(|| {
                assert_eq!(current_id(), None);
                publish(JobUpdate {
                    stage: JobStage::TileRun,
                    ..JobUpdate::default()
                });
            })
            .join()
            .unwrap();
            {
                let _masked = enter(None);
                assert_eq!(current_id(), None);
                {
                    let _inner = enter(Some(second.clone()));
                    assert_eq!(current_id(), Some(202));
                }
                assert_eq!(current_id(), None);
            }
            assert_eq!(current_id(), Some(101));
            assert!(
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    let _inner = enter(Some(second.clone()));
                    panic!("adopted diagnostic scope fixture unwind");
                }))
                .is_err()
            );
            assert_eq!(current_id(), Some(101));
        }
        assert_eq!(current_id(), None);
        drop((first, second));
        assert_eq!(publisher.active_count_for_test(), 0);
    }
}
