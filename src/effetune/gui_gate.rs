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

// Must match GuiGateState in the host. Both processes are x64 and use aligned,
// lock-free 64-bit loads/stores; the mapping never contains a process pointer.
#[repr(C)]
struct GateState {
    magic: u64,
    version: u64,
    minimized_sequence: AtomicU64,
    remote: AtomicU64,
    keep_visible_when_minimized: AtomicU64,
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
                version: 2,
                minimized_sequence: AtomicU64::new(0),
                remote: AtomicU64::new(0),
                keep_visible_when_minimized: AtomicU64::new(0),
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
        self.state()
            .minimized_sequence
            .fetch_add(1, Ordering::AcqRel);
    }
    #[cfg(test)]
    pub(crate) fn remote(&self) -> u64 {
        self.state().remote.load(Ordering::Acquire)
    }
    pub(crate) fn publish_remote(&self, sequence: Option<u64>, blocks: bool) {
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
    #[test]
    fn source_epochs_survive_completed_suppression_intervals() {
        assert_eq!(std::mem::size_of::<GateState>(), 40);
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
        assert_eq!(gate.state().version, 2);
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
