//! Connection-scoped page job identities and cancellation state.
//!
//! This module deliberately knows nothing about the heavy queue. The registry is the source of
//! truth for demand, lifetime, and monotonic priority; stage 3 will mirror that priority into the
//! queue while holding the wiring layer's critical section.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

pub(super) type PageJobConnectionId = u64;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(super) struct PageJobId(String);

impl PageJobId {
    pub(super) fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<String> for PageJobId {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl From<&str> for PageJobId {
    fn from(value: &str) -> Self {
        Self(value.to_owned())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(super) struct DisplayRequestId(String);

impl DisplayRequestId {
    pub(super) fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<String> for DisplayRequestId {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl From<&str> for DisplayRequestId {
    fn from(value: &str) -> Self {
        Self(value.to_owned())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum PageJobPriority {
    Prefetch,
    Foreground,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum PageJobCancelCause {
    NoDemand,
    SessionInvalidated,
    ConnectionClosed,
    ServiceStopping,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum RegisterPageJobError {
    DuplicateJob,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum PromotePageJobResult {
    Promoted,
    AlreadyForeground,
    AlreadyReleased { cause: PageJobCancelCause },
    UnknownJob,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ReleasePageJobResult {
    Released,
    AlreadyReleased { cause: PageJobCancelCause },
    UnknownJob,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum FinishPageJobResult {
    Finished,
    AlreadyReleased { cause: PageJobCancelCause },
    UnknownJob,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct CancelPageJobsResult {
    pub(super) released: usize,
    pub(super) already_released: usize,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct PageJobRegistrySnapshot {
    pub(super) prefetch_active: usize,
    pub(super) foreground_active: usize,
    pub(super) prefetch_released: usize,
    pub(super) foreground_released: usize,
    pub(super) distinct_display_requests: usize,
}

impl PageJobRegistrySnapshot {
    pub(super) fn active(&self) -> usize {
        self.prefetch_active + self.foreground_active
    }

    pub(super) fn released(&self) -> usize {
        self.prefetch_released + self.foreground_released
    }

    pub(super) fn total(&self) -> usize {
        self.active() + self.released()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PageJobState {
    Active,
    Released(PageJobCancelCause),
}

struct PageJobRecord {
    display_request_id: DisplayRequestId,
    priority: PageJobPriority,
    state: PageJobState,
    cancel: Arc<AtomicBool>,
}

#[derive(Default)]
struct PageJobRegistryState {
    jobs_by_connection: HashMap<PageJobConnectionId, HashMap<PageJobId, PageJobRecord>>,
}

#[derive(Default)]
pub(super) struct PageJobRegistry {
    inner: Mutex<PageJobRegistryState>,
}

impl PageJobRegistry {
    pub(super) fn new() -> Self {
        Self::default()
    }

    pub(super) fn register(
        &self,
        connection_id: PageJobConnectionId,
        job_id: PageJobId,
        display_request_id: DisplayRequestId,
        priority: PageJobPriority,
    ) -> Result<Arc<AtomicBool>, RegisterPageJobError> {
        let mut state = self.inner.lock().unwrap_or_else(|error| error.into_inner());
        let jobs = state.jobs_by_connection.entry(connection_id).or_default();
        if jobs.contains_key(&job_id) {
            return Err(RegisterPageJobError::DuplicateJob);
        }
        let cancel = Arc::new(AtomicBool::new(false));
        jobs.insert(
            job_id,
            PageJobRecord {
                display_request_id,
                priority,
                state: PageJobState::Active,
                cancel: Arc::clone(&cancel),
            },
        );
        Ok(cancel)
    }

    pub(super) fn promote(
        &self,
        connection_id: PageJobConnectionId,
        job_id: &PageJobId,
    ) -> PromotePageJobResult {
        let mut state = self.inner.lock().unwrap_or_else(|error| error.into_inner());
        let Some(job) = state
            .jobs_by_connection
            .get_mut(&connection_id)
            .and_then(|jobs| jobs.get_mut(job_id))
        else {
            return PromotePageJobResult::UnknownJob;
        };
        if let PageJobState::Released(cause) = job.state {
            return PromotePageJobResult::AlreadyReleased { cause };
        }
        match job.priority {
            PageJobPriority::Prefetch => {
                job.priority = PageJobPriority::Foreground;
                PromotePageJobResult::Promoted
            }
            PageJobPriority::Foreground => PromotePageJobResult::AlreadyForeground,
        }
    }

    pub(super) fn release(
        &self,
        connection_id: PageJobConnectionId,
        job_id: &PageJobId,
        cause: PageJobCancelCause,
    ) -> ReleasePageJobResult {
        let mut state = self.inner.lock().unwrap_or_else(|error| error.into_inner());
        let Some(job) = state
            .jobs_by_connection
            .get_mut(&connection_id)
            .and_then(|jobs| jobs.get_mut(job_id))
        else {
            return ReleasePageJobResult::UnknownJob;
        };
        match job.state {
            PageJobState::Active => {
                job.state = PageJobState::Released(cause);
                job.cancel.store(true, Ordering::Release);
                ReleasePageJobResult::Released
            }
            PageJobState::Released(cause) => ReleasePageJobResult::AlreadyReleased { cause },
        }
    }

    pub(super) fn display_request_id(
        &self,
        connection_id: PageJobConnectionId,
        job_id: &PageJobId,
    ) -> Option<DisplayRequestId> {
        let state = self.inner.lock().unwrap_or_else(|error| error.into_inner());
        state
            .jobs_by_connection
            .get(&connection_id)
            .and_then(|jobs| jobs.get(job_id))
            .map(|job| job.display_request_id.clone())
    }

    pub(super) fn finish(
        &self,
        connection_id: PageJobConnectionId,
        job_id: &PageJobId,
    ) -> FinishPageJobResult {
        let mut state = self.inner.lock().unwrap_or_else(|error| error.into_inner());
        let Some(jobs) = state.jobs_by_connection.get_mut(&connection_id) else {
            return FinishPageJobResult::UnknownJob;
        };
        let Some(job) = jobs.remove(job_id) else {
            return FinishPageJobResult::UnknownJob;
        };
        let connection_is_empty = jobs.is_empty();
        if connection_is_empty {
            state.jobs_by_connection.remove(&connection_id);
        }
        match job.state {
            PageJobState::Active => FinishPageJobResult::Finished,
            PageJobState::Released(cause) => FinishPageJobResult::AlreadyReleased { cause },
        }
    }

    /// Terminal for one connection: cancel everything it owns and forget it. Nobody will call
    /// `finish` for work whose connection is gone (its queued payloads are pruned, and an
    /// in-flight render holds its own cancel token), so keeping the records would leak one entry
    /// per reconnect. A later `finish` from a render that was already running answers
    /// `UnknownJob`, which is the expected result here and not a fault.
    ///
    /// Session invalidation is different: the connection survives and its jobs still report
    /// completion, so that path uses `cancel_all` and keeps the records.
    pub(super) fn close_connection(
        &self,
        connection_id: PageJobConnectionId,
        cause: PageJobCancelCause,
    ) -> CancelPageJobsResult {
        let mut state = self.inner.lock().unwrap_or_else(|error| error.into_inner());
        let Some(mut jobs) = state.jobs_by_connection.remove(&connection_id) else {
            return CancelPageJobsResult::default();
        };
        cancel_jobs(jobs.values_mut(), cause)
    }

    pub(super) fn cancel_all(&self, cause: PageJobCancelCause) -> CancelPageJobsResult {
        let mut state = self.inner.lock().unwrap_or_else(|error| error.into_inner());
        cancel_jobs(
            state
                .jobs_by_connection
                .values_mut()
                .flat_map(|jobs| jobs.values_mut()),
            cause,
        )
    }

    pub(super) fn snapshot(&self) -> PageJobRegistrySnapshot {
        let state = self.inner.lock().unwrap_or_else(|error| error.into_inner());
        let mut snapshot = PageJobRegistrySnapshot::default();
        let mut display_request_ids = HashSet::new();
        for (connection_id, jobs) in &state.jobs_by_connection {
            for job in jobs.values() {
                display_request_ids.insert((*connection_id, &job.display_request_id));
                match (job.priority, job.state) {
                    (PageJobPriority::Prefetch, PageJobState::Active) => {
                        snapshot.prefetch_active += 1
                    }
                    (PageJobPriority::Foreground, PageJobState::Active) => {
                        snapshot.foreground_active += 1
                    }
                    (PageJobPriority::Prefetch, PageJobState::Released(_)) => {
                        snapshot.prefetch_released += 1
                    }
                    (PageJobPriority::Foreground, PageJobState::Released(_)) => {
                        snapshot.foreground_released += 1
                    }
                }
            }
        }
        snapshot.distinct_display_requests = display_request_ids.len();
        snapshot
    }
}

fn cancel_jobs<'a>(
    jobs: impl Iterator<Item = &'a mut PageJobRecord>,
    cause: PageJobCancelCause,
) -> CancelPageJobsResult {
    let mut result = CancelPageJobsResult::default();
    for job in jobs {
        match job.state {
            PageJobState::Active => {
                job.state = PageJobState::Released(cause);
                job.cancel.store(true, Ordering::Release);
                result.released += 1;
            }
            PageJobState::Released(_) => result.already_released += 1,
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn register(
        registry: &PageJobRegistry,
        connection_id: PageJobConnectionId,
        job_id: &str,
        priority: PageJobPriority,
    ) -> Arc<AtomicBool> {
        registry
            .register(
                connection_id,
                job_id.into(),
                format!("display-{job_id}").into(),
                priority,
            )
            .unwrap()
    }

    #[test]
    fn priority_promotes_once_and_never_downgrades() {
        let registry = PageJobRegistry::new();
        register(&registry, 1, "page-1", PageJobPriority::Prefetch);
        assert_eq!(
            registry.promote(1, &"page-1".into()),
            PromotePageJobResult::Promoted
        );
        assert_eq!(
            registry.promote(1, &"page-1".into()),
            PromotePageJobResult::AlreadyForeground
        );
        assert_eq!(
            registry.promote(1, &"missing".into()),
            PromotePageJobResult::UnknownJob
        );
        assert_eq!(registry.snapshot().foreground_active, 1);
    }

    #[test]
    fn duplicate_register_is_typed_and_scoped_to_the_connection() {
        let registry = PageJobRegistry::new();
        let first = register(&registry, 1, "shared", PageJobPriority::Prefetch);
        assert!(matches!(
            registry.register(
                1,
                "shared".into(),
                "display-duplicate".into(),
                PageJobPriority::Foreground,
            ),
            Err(RegisterPageJobError::DuplicateJob)
        ));
        let second = register(&registry, 2, "shared", PageJobPriority::Foreground);
        assert!(!Arc::ptr_eq(&first, &second));
        assert_eq!(registry.snapshot().total(), 2);
    }

    #[test]
    fn display_request_identity_is_observable_and_counted_distinctly() {
        let registry = PageJobRegistry::new();
        registry
            .register(
                1,
                "page-a".into(),
                "display-a".into(),
                PageJobPriority::Prefetch,
            )
            .unwrap();
        registry
            .register(
                1,
                "page-b".into(),
                "display-a".into(),
                PageJobPriority::Foreground,
            )
            .unwrap();
        registry
            .register(
                2,
                "page-a".into(),
                "display-b".into(),
                PageJobPriority::Foreground,
            )
            .unwrap();

        assert_eq!(
            registry.display_request_id(1, &"page-a".into()),
            Some("display-a".into())
        );
        assert_eq!(
            registry.display_request_id(2, &"page-a".into()),
            Some("display-b".into())
        );
        assert_eq!(registry.display_request_id(1, &"missing".into()), None);
        assert_eq!(registry.snapshot().distinct_display_requests, 2);

        registry.release(1, &"page-a".into(), PageJobCancelCause::NoDemand);
        assert_eq!(
            registry.display_request_id(1, &"page-a".into()),
            Some("display-a".into())
        );
    }

    #[test]
    fn release_is_idempotent_and_finish_reports_the_release() {
        let registry = PageJobRegistry::new();
        let cancel = register(&registry, 1, "page-1", PageJobPriority::Prefetch);
        assert_eq!(
            registry.release(1, &"page-1".into(), PageJobCancelCause::NoDemand),
            ReleasePageJobResult::Released
        );
        assert!(cancel.load(Ordering::Acquire));
        assert_eq!(
            registry.release(1, &"page-1".into(), PageJobCancelCause::ServiceStopping),
            ReleasePageJobResult::AlreadyReleased {
                cause: PageJobCancelCause::NoDemand
            }
        );
        assert_eq!(
            registry.promote(1, &"page-1".into()),
            PromotePageJobResult::AlreadyReleased {
                cause: PageJobCancelCause::NoDemand
            }
        );
        assert_eq!(
            registry.finish(1, &"page-1".into()),
            FinishPageJobResult::AlreadyReleased {
                cause: PageJobCancelCause::NoDemand
            }
        );
        assert_eq!(
            registry.finish(1, &"page-1".into()),
            FinishPageJobResult::UnknownJob
        );
    }

    #[test]
    fn close_connection_never_changes_another_connection() {
        let registry = PageJobRegistry::new();
        let first_a = register(&registry, 1, "a", PageJobPriority::Prefetch);
        let first_b = register(&registry, 1, "b", PageJobPriority::Foreground);
        let second = register(&registry, 2, "a", PageJobPriority::Prefetch);
        assert_eq!(
            registry.close_connection(1, PageJobCancelCause::ConnectionClosed),
            CancelPageJobsResult {
                released: 2,
                already_released: 0
            }
        );
        assert!(first_a.load(Ordering::Acquire));
        assert!(first_b.load(Ordering::Acquire));
        assert!(!second.load(Ordering::Acquire));
        assert_eq!(
            registry.promote(2, &"a".into()),
            PromotePageJobResult::Promoted
        );
        assert_eq!(
            registry.snapshot(),
            PageJobRegistrySnapshot {
                prefetch_active: 0,
                foreground_active: 1,
                prefetch_released: 0,
                foreground_released: 0,
                distinct_display_requests: 1,
            }
        );
    }

    /// A reconnecting reader must not leave one record per closed connection behind. Nobody
    /// calls `finish` for work whose connection is gone, so closing has to be terminal.
    #[test]
    fn close_connection_forgets_the_connection() {
        let registry = PageJobRegistry::new();
        for connection_id in 0..8 {
            register(
                &registry,
                connection_id,
                "page",
                PageJobPriority::Foreground,
            );
            assert_eq!(
                registry.close_connection(connection_id, PageJobCancelCause::ConnectionClosed),
                CancelPageJobsResult {
                    released: 1,
                    already_released: 0
                }
            );
        }
        assert_eq!(registry.snapshot(), PageJobRegistrySnapshot::default());
        // The render that was already running still reports completion. After a close that is
        // expected, not a fault.
        assert_eq!(
            registry.finish(7, &"page".into()),
            FinishPageJobResult::UnknownJob
        );
        assert_eq!(
            registry.close_connection(7, PageJobCancelCause::ConnectionClosed),
            CancelPageJobsResult::default()
        );
    }

    #[test]
    fn cancel_all_and_unknown_operations_are_typed() {
        let registry = PageJobRegistry::new();
        register(&registry, 1, "a", PageJobPriority::Prefetch);
        register(&registry, 2, "b", PageJobPriority::Foreground);
        assert_eq!(
            registry.cancel_all(PageJobCancelCause::SessionInvalidated),
            CancelPageJobsResult {
                released: 2,
                already_released: 0
            }
        );
        assert_eq!(
            registry.cancel_all(PageJobCancelCause::ServiceStopping),
            CancelPageJobsResult {
                released: 0,
                already_released: 2
            }
        );
        assert_eq!(
            registry.release(9, &"missing".into(), PageJobCancelCause::NoDemand),
            ReleasePageJobResult::UnknownJob
        );
        assert_eq!(
            registry.finish(9, &"missing".into()),
            FinishPageJobResult::UnknownJob
        );
    }

    #[test]
    fn successful_finish_removes_the_record() {
        let registry = PageJobRegistry::new();
        register(&registry, 1, "page-1", PageJobPriority::Foreground);
        assert_eq!(
            registry.finish(1, &"page-1".into()),
            FinishPageJobResult::Finished
        );
        assert_eq!(registry.snapshot(), PageJobRegistrySnapshot::default());
    }
}
