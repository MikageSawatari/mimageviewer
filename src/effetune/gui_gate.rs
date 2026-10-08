//! Transport of suppression sources. Requested GUI visibility belongs to the host.
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use windows::Win32::Foundation::{
    CloseHandle, ERROR_ALREADY_EXISTS, GetLastError, HANDLE, INVALID_HANDLE_VALUE,
};
use windows::Win32::System::Memory::{
    CreateFileMappingW, FILE_MAP_ALL_ACCESS, MEMORY_MAPPED_VIEW_ADDRESS, MapViewOfFile,
    PAGE_READWRITE, UnmapViewOfFile,
};
use windows::core::HSTRING;

const AUTO_BITS: u32 = 5;
const AUTO_MASK: u64 = (1 << AUTO_BITS) - 1;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AutoSuppression {
    SettingOff = 1,
    RootHidden = 2,
    Fullscreen = 4,
    Minimized = 8,
    RemoteBlocked = 16,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct AutoPresentationSnapshot {
    pub revision: u64,
    pub allowed: bool,
}

#[derive(Clone, Debug)]
pub(crate) struct AutoPresentationReader(Arc<GuiGate>);

impl AutoPresentationReader {
    pub(crate) fn snapshot(&self) -> AutoPresentationSnapshot {
        self.0.auto_snapshot()
    }
}

// Must match GuiGateState in the host. Both processes are x64 and use aligned,
// lock-free 64-bit loads/stores; the mapping never contains a process pointer.
#[repr(C)]
struct GateState {
    magic: u64,
    version: u64,
    minimized_sequence: AtomicU64,
    remote: AtomicU64,
    keep_visible_when_minimized: AtomicU64,
    auto_presentation: AtomicU64,
}

#[derive(Debug)]
pub(crate) struct GuiGate {
    handle: HANDLE,
    view: MEMORY_MAPPED_VIEW_ADDRESS,
    name: String,
    notify: Option<std::sync::mpsc::Sender<super::HostCommand>>,
}
unsafe impl Send for GuiGate {}
unsafe impl Sync for GuiGate {}

impl GuiGate {
    #[cfg(test)]
    pub(crate) fn create_for_test() -> Arc<Self> {
        Self::create(None).expect("test presentation gate")
    }
    pub(super) fn create(
        notify: Option<std::sync::mpsc::Sender<super::HostCommand>>,
    ) -> Result<Arc<Self>, String> {
        let name = format!("miv-effetune-gate-{}", uuid::Uuid::new_v4());
        let handle = unsafe {
            CreateFileMappingW(
                INVALID_HANDLE_VALUE,
                None,
                PAGE_READWRITE,
                0,
                std::mem::size_of::<GateState>() as u32,
                &HSTRING::from(&name),
            )
        }
        .map_err(|error| error.to_string())?;
        if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
            let _ = unsafe { CloseHandle(handle) };
            return Err("presentation gate name already exists".into());
        }
        let view = unsafe {
            MapViewOfFile(
                handle,
                FILE_MAP_ALL_ACCESS,
                0,
                0,
                std::mem::size_of::<GateState>(),
            )
        };
        if view.Value.is_null() {
            let _ = unsafe { CloseHandle(handle) };
            return Err("presentation gate mapping failed".into());
        }
        unsafe {
            view.Value.cast::<GateState>().write(GateState {
                magic: 0x4d49_5647_4154_4501,
                version: 3,
                minimized_sequence: AtomicU64::new(0),
                remote: AtomicU64::new(0),
                keep_visible_when_minimized: AtomicU64::new(0),
                // Canonical owners publish their initial facts before a player
                // receives its read-only reader. Fail closed while unbound.
                auto_presentation: AtomicU64::new(
                    AutoSuppression::SettingOff as u64 | AutoSuppression::RootHidden as u64,
                ),
            })
        };
        Ok(Arc::new(Self {
            handle,
            view,
            name,
            notify,
        }))
    }
    fn state(&self) -> &GateState {
        unsafe { &*self.view.Value.cast::<GateState>() }
    }
    pub(crate) fn name(&self) -> &str {
        &self.name
    }
    pub(crate) fn auto_reader(self: &Arc<Self>) -> AutoPresentationReader {
        AutoPresentationReader(Arc::clone(self))
    }
    pub(crate) fn auto_snapshot(&self) -> AutoPresentationSnapshot {
        let value = self.state().auto_presentation.load(Ordering::Acquire);
        AutoPresentationSnapshot {
            revision: value >> AUTO_BITS,
            allowed: value & AUTO_MASK == 0,
        }
    }
    pub(crate) fn set_auto_factor(&self, factor: AutoSuppression, blocked: bool) {
        self.update_auto_factor(factor, blocked, false);
    }
    pub(crate) fn invalidate_auto(&self) {
        let _ = self.state().auto_presentation.fetch_update(
            Ordering::AcqRel,
            Ordering::Acquire,
            |value| Some(value + (1 << AUTO_BITS)),
        );
    }
    fn update_auto_factor(&self, factor: AutoSuppression, blocked: bool, force: bool) {
        let atomic = &self.state().auto_presentation;
        let bit = factor as u64;
        let mut previous = atomic.load(Ordering::Acquire);
        loop {
            let bits = if blocked {
                previous | bit
            } else {
                previous & !bit
            } & AUTO_MASK;
            if !force && bits == previous & AUTO_MASK {
                return;
            }
            let next = ((previous >> AUTO_BITS) + 1) << AUTO_BITS | bits;
            match atomic.compare_exchange_weak(previous, next, Ordering::AcqRel, Ordering::Acquire)
            {
                Ok(_) => return,
                Err(actual) => previous = actual,
            }
        }
    }
    pub(crate) fn minimized_sequence(&self) -> u64 {
        self.state().minimized_sequence.load(Ordering::Acquire)
    }
    pub(crate) fn set_keep_visible_when_minimized(&self, keep_visible: bool) {
        let previous = self
            .state()
            .keep_visible_when_minimized
            .swap(u64::from(keep_visible), Ordering::AcqRel);
        if previous != u64::from(keep_visible) {
            if let Some(notify) = &self.notify {
                let _ = notify.send(super::HostCommand::ReconcileVisibility);
            }
        }
    }
    #[cfg(test)]
    pub(crate) fn keep_visible_when_minimized(&self) -> bool {
        self.state()
            .keep_visible_when_minimized
            .load(Ordering::Acquire)
            != 0
    }
    pub(crate) fn note_minimized(&self) {
        self.update_auto_factor(AutoSuppression::Minimized, true, true);
        self.state()
            .minimized_sequence
            .fetch_add(1, Ordering::AcqRel);
    }
    pub(crate) fn remote(&self) -> u64 {
        self.state().remote.load(Ordering::Acquire)
    }
    pub(crate) fn publish_remote(&self, sequence: Option<u64>, blocks: bool) {
        self.publish_remote_with_revision(sequence, blocks, false);
    }
    pub(crate) fn publish_remote_with_revision(
        &self,
        sequence: Option<u64>,
        blocks: bool,
        force: bool,
    ) {
        self.update_auto_factor(AutoSuppression::RemoteBlocked, blocks, force);
        self.state().remote.store(
            Self::remote_token(sequence) | u64::from(blocks),
            Ordering::Release,
        );
        if sequence.is_some() {
            if let Some(notify) = &self.notify {
                let _ = notify.send(super::HostCommand::ReconcileVisibility);
            }
        }
    }
    pub(crate) fn remote_token(sequence: Option<u64>) -> u64 {
        sequence.map_or(0, |sequence| (sequence << 2) | 2)
    }
}
impl Drop for GuiGate {
    fn drop(&mut self) {
        unsafe {
            let _ = UnmapViewOfFile(self.view);
            let _ = CloseHandle(self.handle);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn eligible_gate() -> Arc<GuiGate> {
        let gate = GuiGate::create_for_test();
        gate.set_auto_factor(AutoSuppression::SettingOff, false);
        gate.set_auto_factor(AutoSuppression::RootHidden, false);
        gate
    }

    #[test]
    fn auto_projection_invalidates_every_suppression_round_trip() {
        for factor in [
            AutoSuppression::SettingOff,
            AutoSuppression::RootHidden,
            AutoSuppression::Fullscreen,
            AutoSuppression::Minimized,
            AutoSuppression::RemoteBlocked,
        ] {
            let gate = eligible_gate();
            let reader = gate.auto_reader();
            let success = reader.snapshot();
            assert!(success.allowed);
            gate.set_auto_factor(factor, true);
            assert!(!reader.snapshot().allowed);
            gate.set_auto_factor(factor, false);
            assert!(reader.snapshot().allowed);
            assert_ne!(success, reader.snapshot());
        }
    }

    #[test]
    fn auto_projection_preserves_other_owners_and_forced_event_revisions() {
        let gate = eligible_gate();
        gate.set_auto_factor(AutoSuppression::Fullscreen, true);
        gate.set_auto_factor(AutoSuppression::SettingOff, true);
        gate.set_auto_factor(AutoSuppression::SettingOff, false);
        assert!(!gate.auto_snapshot().allowed);
        gate.set_auto_factor(AutoSuppression::Fullscreen, false);
        let initial = gate.auto_snapshot();
        gate.note_minimized();
        let minimized = gate.auto_snapshot();
        gate.note_minimized();
        assert!(gate.auto_snapshot().revision > minimized.revision);
        gate.set_auto_factor(AutoSuppression::Minimized, false);
        gate.publish_remote_with_revision(Some(1), true, true);
        let remote = gate.auto_snapshot();
        gate.publish_remote_with_revision(Some(2), true, true);
        assert!(gate.auto_snapshot().revision > remote.revision);
        gate.publish_remote_with_revision(Some(2), false, false);
        assert!(gate.auto_snapshot().allowed);
        assert!(gate.auto_snapshot().revision > initial.revision);
        let before = gate.auto_snapshot();
        gate.invalidate_auto();
        assert!(gate.auto_snapshot().allowed);
        assert!(gate.auto_snapshot().revision > before.revision);
    }

    #[test]
    fn concurrent_auto_owners_do_not_clear_each_others_suppression() {
        let gate = eligible_gate();
        let workers: Vec<_> = [
            AutoSuppression::SettingOff,
            AutoSuppression::RootHidden,
            AutoSuppression::Fullscreen,
            AutoSuppression::Minimized,
            AutoSuppression::RemoteBlocked,
        ]
        .into_iter()
        .map(|factor| {
            let gate = Arc::clone(&gate);
            std::thread::spawn(move || {
                for _ in 0..1000 {
                    gate.set_auto_factor(factor, true);
                    gate.set_auto_factor(factor, false);
                }
                gate.set_auto_factor(factor, true);
            })
        })
        .collect();
        for worker in workers {
            worker.join().unwrap();
        }
        for factor in [
            AutoSuppression::SettingOff,
            AutoSuppression::RootHidden,
            AutoSuppression::Fullscreen,
            AutoSuppression::Minimized,
        ] {
            gate.set_auto_factor(factor, false);
            assert!(!gate.auto_snapshot().allowed);
        }
        gate.set_auto_factor(AutoSuppression::RemoteBlocked, false);
        assert!(gate.auto_snapshot().allowed);
    }
    #[test]
    fn source_epochs_survive_completed_suppression_intervals() {
        assert_eq!(std::mem::size_of::<GateState>(), 48);
        assert_eq!(std::mem::offset_of!(GateState, auto_presentation), 40);
        assert_eq!(
            std::mem::offset_of!(GateState, keep_visible_when_minimized),
            32
        );
        let gate = GuiGate::create(None).unwrap();
        // Host extract_string_field uses raw contents: the actual mapping name
        // must not require JSON unescaping (notably a namespace backslash).
        let encoded = serde_json::to_string(gate.name()).unwrap();
        assert_eq!(&encoded[1..encoded.len() - 1], gate.name());
        gate.publish_remote(Some(0), false);
        let permit = (gate.minimized_sequence(), gate.remote());
        gate.note_minimized();
        assert_ne!(permit.0, gate.minimized_sequence());
        gate.publish_remote(Some(1), true);
        assert_eq!(gate.remote() & 1, 1);
        gate.publish_remote(Some(1), false);
        assert_ne!(permit.1, gate.remote());
    }

    #[test]
    fn minimize_policy_notifies_only_changes_and_preserves_permit_epochs() {
        let (tx, rx) = std::sync::mpsc::channel();
        let gate = GuiGate::create(Some(tx)).unwrap();
        assert_eq!(gate.state().version, 3);
        assert!(!gate.keep_visible_when_minimized());
        gate.publish_remote(Some(3), true);
        let _ = rx.try_recv().unwrap();
        let epochs = (gate.minimized_sequence(), gate.remote());
        for keep_visible in [true, false] {
            gate.set_keep_visible_when_minimized(keep_visible);
            assert_eq!(gate.keep_visible_when_minimized(), keep_visible);
            assert!(matches!(
                rx.try_recv(),
                Ok(super::super::HostCommand::ReconcileVisibility)
            ));
            gate.set_keep_visible_when_minimized(keep_visible);
            assert!(rx.try_recv().is_err());
            assert_eq!((gate.minimized_sequence(), gate.remote()), epochs);
        }
        gate.set_keep_visible_when_minimized(true);
        gate.note_minimized();
        assert_ne!(gate.minimized_sequence(), epochs.0);
    }
}
