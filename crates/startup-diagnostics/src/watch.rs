use super::{Event, Lane, Outcome, Owner, Role, Stage, owner, unpack};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

const STATE_SHIFT: u32 = 62;
const VALUE_MASK: u64 = (1 << STATE_SHIFT) - 1;
const ACTIVE: u64 = 1 << STATE_SHIFT;
const SUSPENDED: u64 = 2 << STATE_SHIFT;
const RESERVED: u64 = 3 << STATE_SHIFT;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum WatchSlot {
    Launcher,
    CoreStartup,
    NormalPresent,
    IndexerInit,
    InitialDispatch,
}
impl WatchSlot {
    pub(crate) const ALL: [Self; 5] = [
        Self::Launcher,
        Self::CoreStartup,
        Self::NormalPresent,
        Self::IndexerInit,
        Self::InitialDispatch,
    ];
}
pub(crate) struct WatchState {
    clock: AtomicU64,
    child: AtomicU64,
}
impl WatchState {
    pub(crate) fn new() -> Self {
        Self {
            clock: AtomicU64::new(0),
            child: AtomicU64::new(0),
        }
    }
    fn reserve(&self) {
        self.clock.store(RESERVED, Ordering::Release);
    }
    fn begin(&self, now: u64) -> bool {
        self.clock
            .compare_exchange(RESERVED, ACTIVE | now, Ordering::AcqRel, Ordering::Relaxed)
            .is_ok()
    }
    fn suspend(&self, now: u64) {
        let previous = self.clock.load(Ordering::Acquire);
        if previous >> STATE_SHIFT == 1 {
            let elapsed = now.saturating_sub(previous & VALUE_MASK);
            let _ = self.clock.compare_exchange(
                previous,
                SUSPENDED | elapsed,
                Ordering::AcqRel,
                Ordering::Relaxed,
            );
        }
    }
    fn resume(&self, now: u64) {
        let previous = self.clock.load(Ordering::Acquire);
        if previous >> STATE_SHIFT == 2 {
            let start = now.saturating_sub(previous & VALUE_MASK);
            let _ = self.clock.compare_exchange(
                previous,
                ACTIVE | start,
                Ordering::AcqRel,
                Ordering::Relaxed,
            );
        }
    }
    pub(crate) fn retire(&self) -> bool {
        self.clock.swap(0, Ordering::AcqRel) != 0
    }
    fn elapsed(&self, now: u64) -> Option<u64> {
        let c = self.clock.load(Ordering::Acquire);
        (c >> STATE_SHIFT == 1).then(|| now.saturating_sub(c & VALUE_MASK))
    }
    fn effective_elapsed(&self, now: u64) -> u64 {
        let c = self.clock.load(Ordering::Acquire);
        match c >> STATE_SHIFT {
            1 => now.saturating_sub(c & VALUE_MASK),
            2 => c & VALUE_MASK,
            _ => 0,
        }
    }
    fn live(&self) -> bool {
        self.clock.load(Ordering::Acquire) != 0
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WatchHandle {
    pub(crate) slot: WatchSlot,
}
pub fn watch_handle(slot: WatchSlot) -> Option<WatchHandle> {
    owner()
        .filter(|o| o.watches[slot as usize].live())
        .map(|_| WatchHandle { slot })
}
/// Only the first request receives this capability. Remote retirement is
/// irreversible; fallback and redispatch cannot reserve or revive a slot.
pub fn first_dispatch_watch() -> Option<WatchHandle> {
    let o = owner()?;
    if o.first_dispatch_taken.swap(true, Ordering::AcqRel) {
        None
    } else {
        watch_handle(WatchSlot::InitialDispatch)
    }
}
impl WatchHandle {
    pub fn begin(self) {
        if let Some(o) = owner() {
            let now = o.now();
            if o.watches[self.slot as usize].begin(now) {
                watch_begin(o, self.slot, now);
            }
        }
    }
    pub fn suspend(self) {
        if let Some(o) = owner() {
            o.watches[self.slot as usize].suspend(o.now());
        }
    }
    pub fn resume(self) {
        if let Some(o) = owner() {
            o.watches[self.slot as usize].resume(o.now());
        }
    }
    pub fn retire(self, outcome: Outcome) {
        if let Some(o) = owner() {
            retire(o, self.slot, outcome);
        }
    }
    pub(crate) fn child(self, packed: u64) -> Option<(u64, u64)> {
        let o = owner()?;
        let slot = &o.watches[self.slot as usize];
        let (stage, _) = unpack(packed)?;
        // The packed child clock lives in the parent's effective-time
        // coordinate system, so intentional suspension excludes time for
        // both parent and nested children without changing their identities.
        let child = super::pack(stage, slot.effective_elapsed(o.now()));
        slot.live()
            .then(|| (child, slot.child.swap(child, Ordering::AcqRel)))
    }
    pub(crate) fn restore_child(self, packed: u64, parent: u64) {
        if let Some(o) = owner() {
            let _ = o.watches[self.slot as usize].child.compare_exchange(
                packed,
                parent,
                Ordering::AcqRel,
                Ordering::Relaxed,
            );
        }
    }
}
pub(crate) fn retire(o: &Owner, slot: WatchSlot, outcome: Outcome) {
    let now = o.now();
    let elapsed = o.watches[slot as usize].effective_elapsed(now);
    if o.watches[slot as usize].retire() {
        let mut e = Event::new(1, lane(slot), stage(slot), slot as u64, now)
            .named("watch.slot")
            .text("watch.end");
        e.outcome = outcome;
        e.elapsed = elapsed;
        o.publish(e);
    }
}
#[cfg(test)]
pub(crate) fn is_live(o: &Owner, slot: WatchSlot) -> bool {
    o.watches[slot as usize].live()
}
fn lane(slot: WatchSlot) -> Lane {
    match slot {
        WatchSlot::Launcher => Lane::Launcher,
        WatchSlot::IndexerInit => Lane::Indexer,
        WatchSlot::InitialDispatch => Lane::Navigation,
        _ => Lane::Core,
    }
}
fn stage(slot: WatchSlot) -> Stage {
    match slot {
        WatchSlot::Launcher | WatchSlot::CoreStartup => Stage::Entry,
        WatchSlot::NormalPresent => Stage::Present,
        WatchSlot::IndexerInit => Stage::IndexerInit,
        WatchSlot::InitialDispatch => Stage::InitialTargetResolve,
    }
}
pub(crate) fn initialize(o: &Owner, entry_us: u64) {
    let slots: &[WatchSlot] = if o.role == Role::Launcher {
        &[WatchSlot::Launcher]
    } else {
        &[
            WatchSlot::CoreStartup,
            WatchSlot::NormalPresent,
            WatchSlot::IndexerInit,
            WatchSlot::InitialDispatch,
        ]
    };
    for slot in slots {
        o.watches[*slot as usize].reserve();
    }
    let slot = if o.role == Role::Launcher {
        WatchSlot::Launcher
    } else {
        WatchSlot::CoreStartup
    };
    if o.watches[slot as usize].begin(entry_us) {
        watch_begin(o, slot, entry_us);
    }
}
fn watch_begin(o: &Owner, slot: WatchSlot, at: u64) {
    o.publish(
        Event::new(0, lane(slot), stage(slot), slot as u64, at)
            .named("watch.slot")
            .text("watch.begin"),
    );
}
/// This is an allowlist, not a scan of timeline lane current-stage values.
pub(crate) fn allowed(lane: Lane, stage: Stage, slot: WatchSlot) -> bool {
    if lane == Lane::Metadata
        || matches!(
            stage,
            Stage::MetadataCleanup
                | Stage::MetadataReconciliation
                | Stage::MetadataSupervisorSpawn
                | Stage::MetadataOrchestrationIdle
                | Stage::MetadataWatchRegistration
                | Stage::EnvironmentQuery
                | Stage::RunNative
        )
    {
        return false;
    }
    match slot {
        WatchSlot::Launcher => lane == Lane::Launcher,
        WatchSlot::CoreStartup => lane == Lane::Core,
        WatchSlot::NormalPresent => {
            lane == Lane::Core
                && matches!(
                    stage,
                    Stage::TextureDelivery
                        | Stage::Buffers
                        | Stage::SurfaceAcquire
                        | Stage::Encode
                        | Stage::Submit
                        | Stage::Present
                )
        }
        WatchSlot::IndexerInit => lane == Lane::Indexer,
        WatchSlot::InitialDispatch => {
            lane == Lane::Navigation
                && matches!(
                    stage,
                    Stage::InitialTargetResolve
                        | Stage::InitialTargetScan
                        | Stage::InitialTargetEnumerate
                        | Stage::InitialTargetAdopt
                )
        }
    }
}
pub(crate) fn spawn(o: Arc<Owner>) -> std::io::Result<()> {
    std::thread::Builder::new()
        .name("startup-watchdog".into())
        .spawn(move || {
            let mut poll = Poller::new();
            loop {
                if o.stopped() || !poll.poll(&o, o.now()) {
                    break;
                }
                // This is the independent observer's timed wait, never a UI wait
                // or a retry loop for acquiring the publisher's mutex.
                std::thread::park_timeout(Duration::from_millis(250));
            }
        })
        .map(|_| ())
}
struct ChildMask {
    identity: u64,
    mask: u8,
}
struct Poller {
    parent_masks: [u8; 5],
    children: [[ChildMask; 128]; 5],
    parent_only: bool,
}
impl Poller {
    fn new() -> Self {
        Self {
            parent_masks: [0; 5],
            parent_only: false,
            children: std::array::from_fn(|_| {
                std::array::from_fn(|_| ChildMask {
                    identity: 0,
                    mask: 0,
                })
            }),
        }
    }
    fn poll(&mut self, o: &Owner, now: u64) -> bool {
        let mut live = false;
        for slot in WatchSlot::ALL {
            let s = &o.watches[slot as usize];
            live |= s.live();
            let Some(elapsed) = s.elapsed(now) else {
                continue;
            };
            emit_due(
                o,
                slot,
                stage(slot),
                slot as u64,
                elapsed,
                now,
                &mut self.parent_masks[slot as usize],
                true,
            );
            if self.parent_only {
                continue;
            }
            let child = s.child.load(Ordering::Acquire);
            if let Some((stage, start)) = unpack(child) {
                let table = &mut self.children[slot as usize];
                let index = table
                    .iter()
                    .position(|e| e.identity == child)
                    .or_else(|| table.iter().position(|e| e.identity == 0));
                if let Some(index) = index {
                    table[index].identity = child;
                    emit_due(
                        o,
                        slot,
                        stage,
                        child,
                        elapsed.saturating_sub(start),
                        now,
                        &mut table[index].mask,
                        false,
                    );
                } else {
                    // Diagnostic fidelity has a fixed budget. Do not recycle identities
                    // (which would re-notify returning parents) or add recovery machinery.
                    self.parent_only = true;
                    o.publish(
                        Event::new(4, lane(slot), stage, slot as u64, now)
                            .named("watch.children.capacity_exceeded")
                            .text("capacity=128 per slot;watch=parent-only for remainder of run"),
                    );
                }
            }
        }
        live
    }
}
fn emit_due(
    o: &Owner,
    slot: WatchSlot,
    stage: Stage,
    id: u64,
    elapsed: u64,
    now: u64,
    mask: &mut u8,
    parent: bool,
) {
    for (i, threshold) in [5_000_000, 15_000_000, 30_000_000].into_iter().enumerate() {
        let bit = 1 << i;
        if elapsed >= threshold && *mask & bit == 0 {
            *mask |= bit;
            let mut e = Event::new(4, lane(slot), stage, id, now);
            e.elapsed = elapsed;
            e.parent = slot as u64;
            e.correlation = threshold;
            e = e.text(if parent {
                "parent.overdue"
            } else {
                "stage.overdue"
            });
            o.publish(e);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn child_capacity_boundary_reports_once_then_watches_only_parents() {
        let o = crate::tests::test_owner(Role::Core);
        initialize(&o, 0);
        let s = &o.watches[WatchSlot::CoreStartup as usize];
        let mut p = Poller::new();
        // Fill all 128 identities, including the exact last supported entry.
        for index in 0..128 {
            s.child
                .store(crate::pack(Stage::SettingsOpen, index), Ordering::Release);
            assert!(p.poll(&o, index));
        }
        assert!(!p.parent_only);
        let last = crate::pack(Stage::SettingsOpen, 127);
        p.poll(&o, 5_000_127);
        assert!(
            o.journal
                .lock()
                .unwrap()
                .history
                .iter()
                .any(|e| { e.message() == "stage.overdue" && e.id == last })
        );
        s.child
            .store(crate::pack(Stage::SettingsOpen, 128), Ordering::Release);
        p.poll(&o, 5_000_128);
        assert!(p.parent_only);
        // Even already-known children and a different slot stop child monitoring.
        s.child.store(last, Ordering::Release);
        let other = &o.watches[WatchSlot::IndexerInit as usize];
        other.begin(0);
        other
            .child
            .store(crate::pack(Stage::IndexerInit, 0), Ordering::Release);
        for now in [15_000_128, 30_000_128, 40_000_128] {
            assert!(p.poll(&o, now));
        }
        let j = o.journal.lock().unwrap();
        let capacity = j
            .history
            .iter()
            .filter(|e| e.name() == "watch.children.capacity_exceeded")
            .collect::<Vec<_>>();
        assert_eq!(capacity.len(), 1);
        assert!(capacity[0].message().contains("watch=parent-only"));
        assert_eq!(
            j.history
                .iter()
                .filter(|e| e.message() == "stage.overdue")
                .count(),
            1
        );
        for slot in [WatchSlot::CoreStartup, WatchSlot::IndexerInit] {
            let warnings = j
                .history
                .iter()
                .filter(|e| e.message() == "parent.overdue" && e.parent == slot as u64)
                .map(|e| e.correlation)
                .collect::<Vec<_>>();
            assert_eq!(warnings, [5_000_000, 15_000_000, 30_000_000]);
        }
        drop(j);
        for slot in WatchSlot::ALL {
            o.watches[slot as usize].retire();
        }
        assert!(!p.poll(&o, 50_000_000));
    }
    #[test]
    fn suspended_clock_retains_effective_elapsed_and_retirement_is_terminal() {
        let s = WatchState::new();
        s.reserve();
        s.begin(0);
        s.suspend(4_000_000);
        assert_eq!(s.elapsed(104_000_000), None);
        s.resume(104_000_000);
        assert_eq!(s.elapsed(105_000_000), Some(5_000_000));
        assert!(s.retire());
        s.begin(200_000_000);
        s.resume(200_000_000);
        assert!(!s.live());
    }
    #[test]
    fn reserved_resume_is_inert_and_zero_elapsed_pause_cannot_be_rebegun() {
        let s = WatchState::new();
        s.reserve();
        s.resume(100_000_000);
        assert_eq!(s.elapsed(200_000_000), None);
        assert!(s.begin(200_000_000));
        s.suspend(200_000_000);
        assert!(!s.begin(300_000_000));
        assert_eq!(s.elapsed(400_000_000), None);
        s.resume(400_000_000);
        assert_eq!(s.elapsed(405_000_000), Some(5_000_000));
    }
    #[test]
    fn parent_short_children_and_restoration_notify_each_threshold_once() {
        let o = crate::tests::test_owner(Role::Core);
        initialize(&o, 0);
        let s = &o.watches[WatchSlot::CoreStartup as usize];
        let mut p = Poller::new();
        for second in 0..=40 {
            s.child.store(
                crate::pack(Stage::SettingsOpen, second * 1_000_000),
                Ordering::Release,
            );
            assert!(p.poll(&o, second * 1_000_000));
        }
        let j = o.journal.lock().unwrap();
        let events = j
            .history
            .iter()
            .filter(|e| {
                e.message() == "parent.overdue" && e.parent == WatchSlot::CoreStartup as u64
            })
            .collect::<Vec<_>>();
        assert_eq!(events.len(), 3);
        assert_eq!(
            events.iter().map(|e| e.correlation).collect::<Vec<_>>(),
            vec![5_000_000, 15_000_000, 30_000_000]
        );
    }
    #[test]
    fn reserved_dispatch_prevents_exit_and_metadata_never_extends_watchdog() {
        let o = crate::tests::test_owner(Role::Core);
        initialize(&o, 0);
        for slot in [
            WatchSlot::CoreStartup,
            WatchSlot::NormalPresent,
            WatchSlot::IndexerInit,
        ] {
            o.watches[slot as usize].retire();
        }
        let mut p = Poller::new();
        assert!(p.poll(&o, 100_000_000));
        assert!(!allowed(
            Lane::Metadata,
            Stage::MetadataWatchRegistration,
            WatchSlot::IndexerInit
        ));
        o.watches[WatchSlot::InitialDispatch as usize].retire();
        assert!(!p.poll(&o, 140_000_000));
        o.watches[WatchSlot::InitialDispatch as usize].begin(140_000_000);
        assert!(!p.poll(&o, 180_000_000));
    }
    #[test]
    fn child_parent_return_does_not_reset_child_mask() {
        let o = crate::tests::test_owner(Role::Core);
        initialize(&o, 0);
        let mut p = Poller::new();
        let s = &o.watches[1];
        let parent = crate::pack(Stage::Settings, 0);
        s.child.store(parent, Ordering::Release);
        p.poll(&o, 5_000_000);
        s.child.store(
            crate::pack(Stage::SettingsOpen, 6_000_000),
            Ordering::Release,
        );
        p.poll(&o, 7_000_000);
        s.child.store(parent, Ordering::Release);
        p.poll(&o, 8_000_000);
        assert_eq!(
            o.journal
                .lock()
                .unwrap()
                .history
                .iter()
                .filter(|e| e.message() == "stage.overdue" && e.id == parent)
                .count(),
            1
        );
    }
    #[test]
    fn intentional_pause_excludes_hidden_time_from_parent_and_child() {
        let o = crate::tests::test_owner(Role::Core);
        initialize(&o, 0);
        let s = &o.watches[WatchSlot::NormalPresent as usize];
        s.begin(0);
        s.child
            .store(crate::pack(Stage::SurfaceAcquire, 0), Ordering::Release);
        let mut p = Poller::new();
        p.poll(&o, 4_000_000);
        s.suspend(4_000_000);
        p.poll(&o, 104_000_000);
        s.resume(104_000_000);
        p.poll(&o, 105_000_000);
        p.poll(&o, 106_000_000);
        let j = o.journal.lock().unwrap();
        let warnings = j
            .history
            .iter()
            .filter(|e| e.kind == 4 && e.parent == WatchSlot::NormalPresent as u64)
            .collect::<Vec<_>>();
        assert_eq!(warnings.len(), 2);
        assert!(warnings.iter().all(|e| e.correlation == 5_000_000));
    }
    #[test]
    fn inherited_core_watch_excludes_launcher_elapsed() {
        let o = crate::tests::test_owner(Role::Core);
        initialize(&o, 40_000_000);
        let mut p = Poller::new();
        p.poll(&o, 44_999_999);
        assert!(
            !o.journal
                .lock()
                .unwrap()
                .history
                .iter()
                .any(|e| e.kind == 4)
        );
        p.poll(&o, 45_000_000);
        assert_eq!(
            o.journal
                .lock()
                .unwrap()
                .history
                .iter()
                .filter(|e| e.kind == 4 && e.parent == WatchSlot::CoreStartup as u64)
                .count(),
            1
        );
    }
}
