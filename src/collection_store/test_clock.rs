//! Collection read regression tests advance observation time instead of backdating an Instant.
//! Windows monotonic time starts at boot; subtracting a day can underflow after a reboot.

use std::cell::Cell;
use std::marker::PhantomData;
use std::rc::Rc;
use std::time::{Duration, Instant};

thread_local! {
    static OFFSET: Cell<Duration> = const { Cell::new(Duration::ZERO) };
}

pub(crate) struct TestReadClock {
    previous: Duration,
    // The scope belongs to the current test thread, including during unwinding.
    _thread: PhantomData<Rc<()>>,
}

impl TestReadClock {
    pub(crate) const LONG_ELAPSED: Duration = Duration::from_secs(24 * 60 * 60);

    pub(crate) fn long_elapsed_since(started: Instant) -> Self {
        let previous = OFFSET.with(|offset| {
            let previous = offset.get();
            offset.set(previous + Self::LONG_ELAPSED);
            previous
        });
        let scope = Self {
            previous,
            _thread: PhantomData,
        };
        assert!(Self::now().saturating_duration_since(started) >= Self::LONG_ELAPSED);
        scope
    }

    pub(crate) fn now() -> Instant {
        Instant::now() + OFFSET.with(Cell::get)
    }
}

impl Drop for TestReadClock {
    fn drop(&mut self) {
        OFFSET.with(|offset| offset.set(self.previous));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::collection_store::{CollectionReadLease, CollectionReadScope};

    #[test]
    fn long_elapsed_clock_keeps_real_lease_timing_and_is_thread_local() {
        let started = Instant::now();
        let lease = CollectionReadLease::new(
            CollectionReadScope::app_global("clock-test"),
            started,
            "actor",
        );
        let _scope = TestReadClock::long_elapsed_since(started);
        let now = TestReadClock::now();
        assert!(lease.wall_elapsed(now) >= TestReadClock::LONG_ELAPSED);
        assert!(lease.active_elapsed(now) >= TestReadClock::LONG_ELAPSED);
        assert!(lease.is_due(now));
        std::thread::spawn(|| OFFSET.with(|offset| assert_eq!(offset.get(), Duration::ZERO)))
            .join()
            .unwrap();
    }

    #[test]
    fn nested_clock_scope_restores_after_panic() {
        let _outer = TestReadClock::long_elapsed_since(Instant::now());
        let previous = OFFSET.with(Cell::get);
        let result = std::panic::catch_unwind(|| {
            let _inner = TestReadClock::long_elapsed_since(TestReadClock::now());
            assert_eq!(
                OFFSET.with(Cell::get),
                previous + TestReadClock::LONG_ELAPSED
            );
            panic!("exercise clock scope unwind");
        });
        assert!(result.is_err());
        assert_eq!(OFFSET.with(Cell::get), previous);
        drop(_outer);
        assert_eq!(OFFSET.with(Cell::get), Duration::ZERO);
    }

    #[test]
    fn collection_read_tests_do_not_backdate_monotonic_time() {
        for (name, source) in [
            ("collection_grid", include_str!("../app/collection_grid.rs")),
            (
                "collection_navigation",
                include_str!("../app/collection_navigation.rs"),
            ),
            ("collections", include_str!("../ui_dialogs/collections.rs")),
            (
                "saved_group_actions",
                include_str!("../app/saved_group_actions.rs"),
            ),
        ] {
            let compact: String = source.split_whitespace().collect();
            assert!(
                !compact.contains(".checked_sub(Duration::")
                    && !compact.contains(".checked_sub(std::time::Duration::"),
                "{name}: advance TestReadClock instead of subtracting durations from Instant"
            );
        }
    }
}
