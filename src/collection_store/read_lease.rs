//! UI-owned collection reads share one identity, lifecycle, and retry-backoff contract.
//!
//! The lease is deliberately separate from the database actor. Only an explicit owner
//! transition or a real terminal result retires a UI intent; elapsed time does not change actor
//! health or start a replacement actor.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const FAST_POLL: Duration = Duration::from_millis(50);
const MEDIUM_POLL: Duration = Duration::from_millis(200);
const SLOW_POLL: Duration = Duration::from_secs(1);
const COMPLETION_POLL: Duration = Duration::from_millis(50);

static NEXT_REQUEST_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PollStep {
    Fast,
    Medium,
    Slow,
}

impl PollStep {
    fn delay(self) -> Duration {
        match self {
            Self::Fast => FAST_POLL,
            Self::Medium => MEDIUM_POLL,
            Self::Slow => SLOW_POLL,
        }
    }

    fn advance(self) -> Self {
        match self {
            Self::Fast => Self::Medium,
            Self::Medium | Self::Slow => Self::Slow,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct CollectionReadScope {
    owner: &'static str,
    context: Option<u64>,
    surface_generation: Option<u64>,
}

impl CollectionReadScope {
    pub(crate) const fn app_global(owner: &'static str) -> Self {
        Self {
            owner,
            context: None,
            surface_generation: None,
        }
    }

    pub(crate) const fn viewer(owner: &'static str, context: u64, surface_generation: u64) -> Self {
        Self {
            owner,
            context: Some(context),
            surface_generation: Some(surface_generation),
        }
    }
}

#[derive(Clone, Debug)]
struct ActiveTiming {
    wall_started_at: Instant,
    paused_total: Duration,
    next_poll_at: Instant,
    poll_step: PollStep,
    phase: &'static str,
}

#[derive(Clone, Debug)]
enum CollectionReadLeaseState {
    Dormant,
    Active(ActiveTiming),
    Paused {
        timing: ActiveTiming,
        paused_at: Instant,
    },
    Terminal {
        phase: &'static str,
        outcome: &'static str,
    },
}

#[derive(Debug)]
struct CollectionReadLeaseInner {
    request_id: u64,
    scope: CollectionReadScope,
    state: CollectionReadLeaseState,
}

/// The single lifetime owner for one collection read intent.
///
/// Clones share the same identity and phase state. Only `fresh_dormant` creates a new operation
/// template. PDF password input pauses active-time telemetry without changing the request ID.
#[derive(Clone, Debug)]
pub(crate) struct CollectionReadLease {
    inner: Arc<Mutex<CollectionReadLeaseInner>>,
}

impl PartialEq for CollectionReadLease {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.inner, &other.inner)
    }
}

impl Eq for CollectionReadLease {}

impl Drop for CollectionReadLease {
    fn drop(&mut self) {
        if Arc::strong_count(&self.inner) != 1 {
            return;
        }
        let inner = self
            .inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if matches!(
            inner.state,
            CollectionReadLeaseState::Active(_) | CollectionReadLeaseState::Paused { .. }
        ) {
            emit(&inner, Instant::now(), "retired");
        }
    }
}

impl CollectionReadLease {
    pub(crate) fn new(scope: CollectionReadScope, now: Instant, phase: &'static str) -> Self {
        let inner = CollectionReadLeaseInner {
            request_id: NEXT_REQUEST_ID.fetch_add(1, Ordering::Relaxed).max(1),
            scope,
            state: CollectionReadLeaseState::Active(active_timing(now, phase)),
        };
        emit(&inner, now, "begin");
        Self {
            inner: Arc::new(Mutex::new(inner)),
        }
    }

    pub(crate) fn dormant(scope: CollectionReadScope) -> Self {
        Self {
            inner: Arc::new(Mutex::new(CollectionReadLeaseInner {
                request_id: 0,
                scope,
                state: CollectionReadLeaseState::Dormant,
            })),
        }
    }

    /// Make a template for a user-visible operation distinct from this lease.
    pub(crate) fn fresh_dormant(&self) -> Self {
        let inner = self
            .inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        Self::dormant(inner.scope)
    }

    pub(crate) fn activate(&mut self, now: Instant, phase: &'static str) {
        let mut inner = self
            .inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if !matches!(inner.state, CollectionReadLeaseState::Dormant) {
            return;
        }
        inner.request_id = NEXT_REQUEST_ID.fetch_add(1, Ordering::Relaxed).max(1);
        inner.state = CollectionReadLeaseState::Active(active_timing(now, phase));
        emit(&inner, now, "begin");
    }

    pub(crate) fn bind_viewer(&mut self, context: u64, surface_generation: u64) {
        let mut inner = self
            .inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        match (inner.scope.context, inner.scope.surface_generation) {
            (None, None) => {
                inner.scope.context = Some(context);
                inner.scope.surface_generation = Some(surface_generation);
            }
            (Some(bound_context), Some(bound_surface)) => {
                debug_assert_eq!(bound_context, context);
                debug_assert_eq!(bound_surface, surface_generation);
            }
            _ => debug_assert!(
                false,
                "collection read scope must bind context and surface together"
            ),
        }
    }

    pub(crate) fn request_id(&self) -> u64 {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .request_id
    }

    #[cfg(test)]
    pub(crate) fn phase(&self) -> &'static str {
        let inner = self
            .inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        match &inner.state {
            CollectionReadLeaseState::Dormant => "dormant",
            CollectionReadLeaseState::Active(timing)
            | CollectionReadLeaseState::Paused { timing, .. } => timing.phase,
            CollectionReadLeaseState::Terminal { phase, .. } => phase,
        }
    }

    #[cfg(test)]
    pub(crate) fn is_paused(&self) -> bool {
        matches!(
            self.inner
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .state,
            CollectionReadLeaseState::Paused { .. }
        )
    }

    pub(crate) fn is_due(&self, now: Instant) -> bool {
        let inner = self
            .inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        matches!(&inner.state, CollectionReadLeaseState::Active(timing) if now >= timing.next_poll_at)
    }

    pub(crate) fn poll_delay(&self, now: Instant) -> Option<Duration> {
        let inner = self
            .inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        match &inner.state {
            CollectionReadLeaseState::Active(timing) => {
                Some(timing.next_poll_at.saturating_duration_since(now))
            }
            CollectionReadLeaseState::Dormant => Some(COMPLETION_POLL),
            CollectionReadLeaseState::Paused { .. } | CollectionReadLeaseState::Terminal { .. } => {
                None
            }
        }
    }

    /// A request that has already entered an actor/worker does not use retry backoff. The
    /// receiver remains cheap to probe and keeps the established completion latency.
    pub(crate) fn completion_poll_delay(&self) -> Option<Duration> {
        let inner = self
            .inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        match &inner.state {
            CollectionReadLeaseState::Active(_) => Some(COMPLETION_POLL),
            CollectionReadLeaseState::Dormant => Some(Duration::ZERO),
            CollectionReadLeaseState::Paused { .. } | CollectionReadLeaseState::Terminal { .. } => {
                None
            }
        }
    }

    pub(crate) fn phase_progress(&mut self, now: Instant, phase: &'static str) {
        self.activate(now, phase);
        let mut inner = self
            .inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let CollectionReadLeaseState::Active(timing) = &mut inner.state {
            timing.phase = phase;
            timing.poll_step = PollStep::Medium;
            timing.next_poll_at = now + FAST_POLL;
            emit(&inner, now, "phase");
        }
    }

    /// Schedule the next observation or retry. Admission and worker creation must pass `is_due`,
    /// so unrelated high-frequency frames cannot bypass the backoff.
    pub(crate) fn defer(&mut self, now: Instant, phase: &'static str) {
        self.activate(now, phase);
        let mut inner = self
            .inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let CollectionReadLeaseState::Active(timing) = &mut inner.state else {
            return;
        };
        if timing.phase != phase {
            timing.phase = phase;
            timing.poll_step = PollStep::Medium;
            timing.next_poll_at = now + FAST_POLL;
            emit(&inner, now, "phase");
            return;
        }
        if now < timing.next_poll_at {
            return;
        }
        let delay = timing.poll_step.delay();
        timing.poll_step = timing.poll_step.advance();
        timing.next_poll_at = now + delay;
        emit(&inner, now, "wait");
    }

    pub(crate) fn pause(&mut self, now: Instant, phase: &'static str) {
        self.activate(now, phase);
        let mut inner = self
            .inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let state = std::mem::replace(&mut inner.state, CollectionReadLeaseState::Dormant);
        let (state, changed) = match state {
            CollectionReadLeaseState::Active(mut timing) => {
                timing.phase = phase;
                (
                    CollectionReadLeaseState::Paused {
                        timing,
                        paused_at: now,
                    },
                    true,
                )
            }
            other => (other, false),
        };
        inner.state = state;
        if changed {
            emit(&inner, now, "pause");
        }
    }

    pub(crate) fn resume(&mut self, now: Instant, phase: &'static str) {
        let mut inner = self
            .inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let state = std::mem::replace(&mut inner.state, CollectionReadLeaseState::Dormant);
        let (state, changed) = match state {
            CollectionReadLeaseState::Paused {
                mut timing,
                paused_at,
            } => {
                let paused = now.saturating_duration_since(paused_at);
                timing.paused_total += paused;
                timing.phase = phase;
                timing.poll_step = PollStep::Medium;
                timing.next_poll_at = now + FAST_POLL;
                (CollectionReadLeaseState::Active(timing), true)
            }
            other => (other, false),
        };
        inner.state = state;
        if changed {
            emit(&inner, now, "resume");
        }
    }

    pub(crate) fn finish(&mut self, now: Instant, terminal: &'static str) {
        let mut inner = self
            .inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let phase = match &inner.state {
            CollectionReadLeaseState::Active(timing)
            | CollectionReadLeaseState::Paused { timing, .. } => timing.phase,
            CollectionReadLeaseState::Dormant | CollectionReadLeaseState::Terminal { .. } => return,
        };
        emit(&inner, now, terminal);
        inner.state = CollectionReadLeaseState::Terminal {
            phase,
            outcome: terminal,
        };
    }

    pub(crate) fn active_elapsed(&self, now: Instant) -> Duration {
        let inner = self
            .inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        active_elapsed(&inner.state, now)
    }

    pub(crate) fn wall_elapsed(&self, now: Instant) -> Duration {
        let inner = self
            .inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        wall_elapsed(&inner.state, now)
    }

    #[cfg(test)]
    pub(crate) fn next_poll_at_for_test(&self) -> Option<Instant> {
        let inner = self
            .inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        match &inner.state {
            CollectionReadLeaseState::Active(timing) => Some(timing.next_poll_at),
            _ => None,
        }
    }

    #[cfg(test)]
    pub(crate) fn force_due_for_test(&mut self, now: Instant) {
        let mut inner = self
            .inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let CollectionReadLeaseState::Active(timing) = &mut inner.state {
            timing.next_poll_at = now;
        }
    }

    #[cfg(test)]
    pub(crate) fn set_next_poll_at_for_test(&mut self, at: Instant) {
        let mut inner = self
            .inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let CollectionReadLeaseState::Active(timing) = &mut inner.state {
            timing.next_poll_at = at;
        }
    }
}

fn active_timing(now: Instant, phase: &'static str) -> ActiveTiming {
    ActiveTiming {
        wall_started_at: now,
        paused_total: Duration::ZERO,
        next_poll_at: now,
        poll_step: PollStep::Fast,
        phase,
    }
}

fn active_elapsed(state: &CollectionReadLeaseState, now: Instant) -> Duration {
    match state {
        CollectionReadLeaseState::Active(timing) => now
            .saturating_duration_since(timing.wall_started_at)
            .saturating_sub(timing.paused_total),
        CollectionReadLeaseState::Paused { timing, paused_at } => paused_at
            .saturating_duration_since(timing.wall_started_at)
            .saturating_sub(timing.paused_total),
        CollectionReadLeaseState::Dormant | CollectionReadLeaseState::Terminal { .. } => {
            Duration::ZERO
        }
    }
}

fn wall_elapsed(state: &CollectionReadLeaseState, now: Instant) -> Duration {
    match state {
        CollectionReadLeaseState::Active(timing)
        | CollectionReadLeaseState::Paused { timing, .. } => {
            now.saturating_duration_since(timing.wall_started_at)
        }
        CollectionReadLeaseState::Dormant | CollectionReadLeaseState::Terminal { .. } => {
            Duration::ZERO
        }
    }
}

fn emit(inner: &CollectionReadLeaseInner, now: Instant, outcome: &'static str) {
    if !crate::perf::is_enabled() || matches!(inner.state, CollectionReadLeaseState::Dormant) {
        return;
    }
    let phase = match &inner.state {
        CollectionReadLeaseState::Dormant => "dormant",
        CollectionReadLeaseState::Active(timing)
        | CollectionReadLeaseState::Paused { timing, .. } => timing.phase,
        CollectionReadLeaseState::Terminal { phase, outcome } => {
            let _ = outcome;
            phase
        }
    };
    let mut fields = vec![
        ("owner", serde_json::Value::from(inner.scope.owner)),
        ("request_id", serde_json::Value::from(inner.request_id)),
        ("phase", serde_json::Value::from(phase)),
        (
            "active_ms",
            serde_json::Value::from(active_elapsed(&inner.state, now).as_secs_f64() * 1000.0),
        ),
        (
            "wall_ms",
            serde_json::Value::from(wall_elapsed(&inner.state, now).as_secs_f64() * 1000.0),
        ),
        ("outcome", serde_json::Value::from(outcome)),
    ];
    if let Some(context) = inner.scope.context {
        fields.push(("context", serde_json::Value::from(context)));
    }
    if let Some(surface_generation) = inner.scope.surface_generation {
        fields.push((
            "surface_generation",
            serde_json::Value::from(surface_generation),
        ));
    }
    crate::perf::event("collection", "read_lease", None, 0, &fields);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_progression_and_phase_reset_keep_one_identity_without_a_time_limit() {
        let start = Instant::now();
        let mut lease =
            CollectionReadLease::new(CollectionReadScope::app_global("test"), start, "starting");
        let request_id = lease.request_id();
        lease.defer(start, "starting");
        assert_eq!(lease.poll_delay(start), Some(FAST_POLL));
        lease.defer(start + FAST_POLL, "starting");
        assert_eq!(lease.poll_delay(start + FAST_POLL), Some(MEDIUM_POLL));
        lease.defer(start + FAST_POLL + MEDIUM_POLL, "starting");
        assert_eq!(
            lease.poll_delay(start + FAST_POLL + MEDIUM_POLL),
            Some(SLOW_POLL)
        );
        assert_eq!(lease.completion_poll_delay(), Some(FAST_POLL));
        assert!(!lease.is_due(start + FAST_POLL + MEDIUM_POLL));
        lease.phase_progress(start + Duration::from_secs(2), "snapshot");
        assert_eq!(
            lease.poll_delay(start + Duration::from_secs(2)),
            Some(FAST_POLL)
        );
        assert_eq!(lease.request_id(), request_id);
        assert!(lease.is_due(start + Duration::from_secs(24 * 60 * 60)));
        assert_eq!(lease.phase(), "snapshot");
        assert_eq!(lease.request_id(), request_id);
    }

    #[test]
    fn password_pause_excludes_input_time_without_changing_identity() {
        let start = Instant::now();
        let mut lease =
            CollectionReadLease::new(CollectionReadScope::app_global("test"), start, "preflight");
        let request_id = lease.request_id();
        lease.pause(start + Duration::from_secs(4), "password_input");
        assert!(lease.is_paused());
        assert_eq!(lease.phase(), "password_input");
        assert_eq!(
            lease.active_elapsed(start + Duration::from_secs(40)),
            Duration::from_secs(4)
        );
        assert_eq!(
            lease.wall_elapsed(start + Duration::from_secs(40)),
            Duration::from_secs(40)
        );
        lease.resume(start + Duration::from_secs(40), "password_preflight");
        assert!(!lease.is_paused());
        assert_eq!(lease.phase(), "password_preflight");
        assert_eq!(lease.request_id(), request_id);
        assert_eq!(
            lease.active_elapsed(start + Duration::from_secs(45)),
            Duration::from_secs(9)
        );
    }

    #[test]
    fn clones_share_identity_and_explicit_fresh_template_does_not() {
        let start = Instant::now();
        let mut original = CollectionReadLease::new(
            CollectionReadScope::viewer("navigation", 7, 11),
            start,
            "admission",
        );
        let mut continuation = original.clone();
        continuation.defer(start, "admission");
        assert_eq!(continuation.request_id(), original.request_id());
        assert_eq!(continuation.poll_delay(start), original.poll_delay(start));

        let mut fresh = original.fresh_dormant();
        assert_eq!(fresh.request_id(), 0);
        fresh.activate(start + Duration::from_secs(30), "admission");
        assert_ne!(fresh.request_id(), original.request_id());
        original.finish(start + Duration::from_secs(2), "cancelled");
        assert!(continuation.poll_delay(start).is_none());
    }
}
