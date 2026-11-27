use super::{MouseNavPresses, PendingMouseNavPresses};

#[test]
fn browser_pending_repeat_only_preserves_navigation_count() {
    let pending = PendingMouseNavPresses::new();
    pending.record_keydown(1 << 30);
    pending.record_keydown(1 << 30);
    assert_eq!(
        pending.take(),
        MouseNavPresses {
            count: 2,
            has_non_repeat: false,
        }
    );
}

#[test]
fn browser_pending_initial_press_survives_mixed_repeat_batch() {
    for initial_first in [false, true] {
        let pending = PendingMouseNavPresses::new();
        pending.record_keydown(if initial_first { 0 } else { 1 << 30 });
        pending.record_keydown(if initial_first { 1 << 30 } else { 0 });
        assert_eq!(
            pending.take(),
            MouseNavPresses {
                count: 2,
                has_non_repeat: true,
            }
        );
    }
}

#[test]
fn browser_pending_drain_retires_count_and_initial_press_together() {
    let pending = PendingMouseNavPresses::new();
    pending.record_keydown(0);
    assert!(pending.take().has_non_repeat);
    assert_eq!(pending.take(), MouseNavPresses::default());
    pending.record_keydown(1 << 30);
    let next = pending.take();
    assert_eq!(next.count, 1);
    assert!(!next.has_non_repeat);
}

#[test]
fn browser_pending_direction_and_appcommand_evidence_stay_separate() {
    let back = PendingMouseNavPresses::new();
    let forward = PendingMouseNavPresses::new();
    // AppCommand carries no repeat flag and is an initial-press source.
    back.record(false);
    forward.record_keydown(1 << 30);
    assert!(back.take().has_non_repeat);
    let next = forward.take();
    assert_eq!(next.count, 1);
    assert!(!next.has_non_repeat);
    assert_eq!(back.take(), MouseNavPresses::default());
}

#[test]
fn browser_pending_count_keeps_existing_u32_wrapping() {
    use std::sync::atomic::Ordering;
    let pending = PendingMouseNavPresses::new();
    pending.0.store(u64::from(u32::MAX), Ordering::Release);
    pending.record_keydown(1 << 30);
    assert_eq!(pending.take(), MouseNavPresses::default());
}
