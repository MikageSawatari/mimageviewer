//! Test-only gate: reject ungrouped GPU creation before touching the driver.
//!
//! nextest 0.9.143 sets NEXTEST=1 and NEXTEST_TEST_GROUP (or @global).
//! Cargo's ordinary test harness does not set NEXTEST, so it stays unrestricted.

use std::ffi::OsStr;

#[track_caller]
pub(crate) fn assert_gpu_test_group() {
    let caller = std::panic::Location::caller();
    let result = std::panic::catch_unwind(|| {
        assert_membership(
            std::env::var_os("NEXTEST").as_deref(),
            std::env::var_os("NEXTEST_TEST_GROUP").as_deref(),
        );
    });
    if let Err(panic) = result {
        // Worker panics may be caught or their JoinHandle discarded. Fail the
        // test process itself so a background GPU creation cannot escape this
        // gate. This module is only included by test targets / cfg(test).
        if let Some(message) = panic.downcast_ref::<String>() {
            use std::io::Write as _;
            let _ = writeln!(std::io::stderr().lock(), "{message}\nGPU entry: {caller}");
        }
        std::process::exit(101);
    }
}

#[track_caller]
fn assert_membership(nextest: Option<&OsStr>, group: Option<&OsStr>) {
    if nextest != Some(OsStr::new("1")) {
        return;
    }
    assert_eq!(
        group,
        Some(OsStr::new("gpu")),
        "GPU device/session test is outside the gpu group: \
         add this test to the gpu group in .config/nextest.toml \
         (NEXTEST_TEST_GROUP={group:?})"
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gpu_membership_validation() {
        // Pure inputs: never mutate the process environment or create a device.
        assert_membership(None, None);
        assert_membership(None, Some(OsStr::new("@global")));
        assert_membership(Some(OsStr::new("0")), None);
        assert_membership(Some(OsStr::new("1")), Some(OsStr::new("gpu")));
        for group in [None, Some(OsStr::new("@global")), Some(OsStr::new("other"))] {
            let panic = std::panic::catch_unwind(|| {
                assert_membership(Some(OsStr::new("1")), group);
            })
            .expect_err("nextest must reject a missing or non-gpu group");
            let message = panic.downcast_ref::<String>().expect("assertion message");
            assert!(message.contains("gpu group in .config/nextest.toml"));
        }
    }
}
