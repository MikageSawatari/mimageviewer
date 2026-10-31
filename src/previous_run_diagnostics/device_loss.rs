//! Per-device first-reason latch. Only the normal writer timestamps and saves it.
use super::RunIdentity;
use super::checkpoint::Stamp;
use serde::{Deserialize, Serialize};
use std::fs::{File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::Path;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, OnceLock};

// Existing bootstrap has at most a DX12 device and a Vulkan fallback device.
// Keep both identities; this does not implement device recreation/recovery.
const SOURCES: usize = 2;
pub(super) const FILE_NAME: &str = "device-loss-events.jsonl";
const MAX_BYTES: u64 = 4096;

pub(crate) struct DeviceLossLatch {
    id: u64,
    reason: AtomicU8,
}
impl DeviceLossLatch {
    /// Called inside wgpu's callback: no clock, message, I/O, wake, logger,
    /// resource operation, or lock. The first abnormal reason wins one CAS.
    pub(crate) fn observe(&self, reason: wgpu::DeviceLostReason) {
        if reason == wgpu::DeviceLostReason::Destroyed {
            return;
        }
        let _ = self
            .reason
            .compare_exchange(0, 1, Ordering::Relaxed, Ordering::Relaxed);
    }
}

pub(super) struct Registry {
    sources: [OnceLock<Arc<DeviceLossLatch>>; SOURCES],
    observed: AtomicU8,
}
impl Registry {
    pub(super) fn new() -> Self {
        Self {
            sources: std::array::from_fn(|_| OnceLock::new()),
            observed: AtomicU8::new(0),
        }
    }
    /// Only the normal root-device creation owner initializes these slots.
    pub(super) fn register(&self) -> Option<Arc<DeviceLossLatch>> {
        for (index, slot) in self.sources.iter().enumerate() {
            if slot.get().is_none() {
                let source = Arc::new(DeviceLossLatch {
                    id: index as u64 + 1,
                    reason: AtomicU8::new(0),
                });
                slot.set(source.clone()).ok()?;
                return Some(source);
            }
        }
        None
    }
    /// Normal event/maintenance paths may wake the writer; never the callback.
    pub(super) fn pending(&self) -> bool {
        let observed = self.observed.load(Ordering::Acquire);
        self.sources.iter().enumerate().any(|(index, source)| {
            observed & (1 << index) == 0
                && source
                    .get()
                    .is_some_and(|source| source.reason.load(Ordering::Relaxed) != 0)
        })
    }
    /// Sole writer consumes each source once, including when its write fails.
    pub(super) fn collect(&self, stamp: Stamp) -> Vec<DeviceObservation> {
        let mut result = Vec::new();
        for (index, slot) in self.sources.iter().enumerate() {
            if self.observed.load(Ordering::Relaxed) & (1 << index) != 0 {
                continue;
            }
            let Some(source) = slot.get() else {
                continue;
            };
            if source.reason.load(Ordering::Relaxed) == 0 {
                continue;
            }
            self.observed.fetch_or(1 << index, Ordering::Release);
            result.push(DeviceObservation {
                device_id: source.id,
                stamp,
                reason: DeviceReason::Unknown,
            });
        }
        result
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(super) enum DeviceReason {
    Unknown,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct DeviceObservation {
    pub device_id: u64,
    pub stamp: Stamp,
    pub reason: DeviceReason,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Entry {
    schema: u32,
    identity: RunIdentity,
    observation: DeviceObservation,
}

pub(super) struct Writer {
    file: File,
}
impl Writer {
    pub(super) fn open(directory: &Path) -> io::Result<Self> {
        Ok(Self {
            file: OpenOptions::new()
                .create(true)
                .append(true)
                .open(directory.join(FILE_NAME))?,
        })
    }
    pub(super) fn append(
        &mut self,
        identity: RunIdentity,
        observation: DeviceObservation,
    ) -> io::Result<()> {
        let mut bytes = serde_json::to_vec(&Entry {
            schema: 1,
            identity,
            observation,
        })?;
        if bytes.len() >= MAX_BYTES as usize / SOURCES {
            return Err(io::Error::other("device evidence exceeds bound"));
        }
        bytes.push(b'\n');
        if self.file.write(&bytes)? != bytes.len() {
            return Err(io::Error::new(
                io::ErrorKind::WriteZero,
                "short device evidence write",
            ));
        }
        Ok(())
    }
}
pub(super) fn read_observations(
    directory: &Path,
    identity: RunIdentity,
) -> io::Result<Vec<DeviceObservation>> {
    let file = match File::open(directory.join(FILE_NAME)) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error),
    };
    let mut bytes = Vec::new();
    file.take(MAX_BYTES + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_BYTES {
        return Err(io::Error::other("device evidence exceeds bound"));
    }
    let mut result = Vec::new();
    for line in bytes.split_inclusive(|byte| *byte == b'\n') {
        if !line.ends_with(b"\n") {
            continue;
        }
        let Ok(entry) = serde_json::from_slice::<Entry>(line) else {
            continue;
        };
        if entry.schema == 1
            && entry.identity == identity
            && (1..=SOURCES as u64).contains(&entry.observation.device_id)
            && !result
                .iter()
                .any(|prior: &DeviceObservation| prior.device_id == entry.observation.device_id)
        {
            result.push(entry.observation);
        }
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::previous_run_diagnostics::test_support;
    #[test]
    fn diagnostic_device_loss_keeps_first_reason_and_distinct_device_identities() {
        let registry = Registry::new();
        let old = registry.register().unwrap();
        let current = registry.register().unwrap();
        assert!(registry.register().is_none());
        old.observe(wgpu::DeviceLostReason::Destroyed);
        assert!(!registry.pending());
        current.observe(wgpu::DeviceLostReason::Unknown);
        current.observe(wgpu::DeviceLostReason::Destroyed);
        current.observe(wgpu::DeviceLostReason::Unknown);
        assert!(registry.pending());
        let stamp = Stamp {
            utc_ms: 100,
            run_elapsed_ms: 3,
        };
        let observations = registry.collect(stamp);
        assert_eq!(
            observations,
            [DeviceObservation {
                device_id: 2,
                stamp,
                reason: DeviceReason::Unknown
            }]
        );
        assert!(!registry.pending());
        old.observe(wgpu::DeviceLostReason::Unknown);
        assert_eq!(registry.collect(stamp)[0].device_id, 1);
        assert!(registry.collect(stamp).is_empty());
    }
    #[test]
    fn diagnostic_device_events_survive_checkpoint_replacement_and_reject_foreign_details() {
        let directory = tempfile::tempdir().unwrap();
        let identity = test_support::identity();
        let observation = DeviceObservation {
            device_id: 1,
            stamp: Stamp {
                utc_ms: 100,
                run_elapsed_ms: 3,
            },
            reason: DeviceReason::Unknown,
        };
        let mut writer = Writer::open(directory.path()).unwrap();
        writer.append(identity, observation).unwrap();
        std::fs::write(directory.path().join("state.json"), b"replacement").unwrap();
        assert_eq!(
            read_observations(directory.path(), identity).unwrap(),
            [observation]
        );
        let other = test_support::identity();
        assert!(
            read_observations(directory.path(), other)
                .unwrap()
                .is_empty()
        );
        let text = std::fs::read_to_string(directory.path().join(FILE_NAME)).unwrap();
        let extra = text.trim_end().strip_suffix('}').unwrap().to_owned()
            + ",\"message\":\"C:/private/secret.png\"}\n";
        std::fs::write(directory.path().join(FILE_NAME), extra).unwrap();
        assert!(
            read_observations(directory.path(), identity)
                .unwrap()
                .is_empty()
        );
        std::fs::write(directory.path().join(FILE_NAME), text.trim_end()).unwrap();
        assert!(
            read_observations(directory.path(), identity)
                .unwrap()
                .is_empty()
        );
    }
}
