use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, RwLock, RwLockReadGuard, RwLockWriteGuard};
use std::time::Instant;

static OVERLAY_GPU_SERVICE: OnceLock<OverlayGpuService> = OnceLock::new();

pub(super) fn overlay_gpu_service() -> &'static OverlayGpuService {
    OVERLAY_GPU_SERVICE.get_or_init(OverlayGpuService::new)
}

/// Process-owned cache manager for the native egui overlay's wgpu stack.
///
/// The instance and every healthy device epoch remain strongly owned for the
/// process lifetime. Surfaces and egui renderers remain presenter-owned.
///
/// On sharing and the [`DeviceEpoch`] gate: mIV has exactly one live media session at a
/// time, so this is NOT about two videos playing together. Two `native-video-render`
/// threads can still overlap because `Drop for NativeVideoOutput` does not block on the
/// render-thread join -- it sets cancel and spawns a joiner -- so an outgoing thread can
/// submit while an incoming one configures. That window did not matter while every overlay
/// owned its own device; it does now.
pub(super) struct OverlayGpuService {
    instance: wgpu::Instance,
    epochs: Mutex<Vec<Arc<DeviceEpoch>>>,
    next_generation: AtomicU64,
}

impl OverlayGpuService {
    fn new() -> Self {
        Self {
            instance: wgpu::Instance::new(&wgpu::InstanceDescriptor {
                backends: wgpu::Backends::DX12,
                ..Default::default()
            }),
            epochs: Mutex::new(Vec::new()),
            next_generation: AtomicU64::new(1),
        }
    }

    pub(super) fn create_composition_surface(
        &'static self,
        composition_visual: *mut core::ffi::c_void,
    ) -> Result<wgpu::Surface<'static>, String> {
        // SAFETY: The caller passes the COM visual owned by its NativeEguiOverlay.
        // The resulting Surface is dropped before that overlay releases the visual.
        unsafe {
            self.instance
                .create_surface_unsafe(wgpu::SurfaceTargetUnsafe::CompositionVisual(
                    composition_visual,
                ))
                .map_err(|e| format!("wgpu CompositionVisual surface: {e:?}"))
        }
    }

    pub(super) fn select_or_create_epoch(
        &self,
        surface: &wgpu::Surface<'_>,
    ) -> Result<DeviceEpochSelection, String> {
        // Keep selection and creation in one critical section. Otherwise an outgoing and
        // an incoming render thread (see the type doc: teardown does not block) could both
        // miss and create duplicate devices for the same adapter.
        let mut epochs = lock_unpoisoned(&self.epochs);
        if let Some(index) = find_reusable_epoch_index(
            epochs.as_slice(),
            |epoch| epoch.is_lost(),
            |epoch| epoch.adapter.is_surface_supported(surface),
        ) {
            return Ok(DeviceEpochSelection {
                epoch: Arc::clone(&epochs[index]),
                adapter_ms: 0.0,
                device_ms: 0.0,
                reused: true,
            });
        }

        let adapter_t0 = Instant::now();
        let adapter =
            pollster::block_on(self.instance.request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                force_fallback_adapter: false,
                compatible_surface: Some(surface),
            }))
            .map_err(|e| format!("wgpu request_adapter for DComp overlay: {e:?}"))?;
        let adapter_ms = adapter_t0.elapsed().as_secs_f64() * 1000.0;

        let generation = self.next_generation.fetch_add(1, Ordering::Relaxed);
        let loss_latch = Arc::new(DeviceLossLatch::new(generation));
        let device_t0 = Instant::now();
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("mIV native egui overlay"),
            ..Default::default()
        }))
        .map_err(|e| format!("wgpu request_device for DComp overlay: {e:?}"))?;
        let device_ms = device_t0.elapsed().as_secs_f64() * 1000.0;

        // Register immediately after creation. This callback is intentionally only a
        // one-way fact publication: no wgpu call, recreation, retry, or logging.
        let callback_latch = Arc::clone(&loss_latch);
        device.set_device_lost_callback(move |_reason, _message| {
            callback_latch.latch_if_generation(generation);
        });

        let epoch = Arc::new(DeviceEpoch {
            generation,
            adapter,
            device,
            queue,
            gate: OverlayGpuGate::default(),
            loss_latch,
        });
        epochs.push(Arc::clone(&epoch));
        Ok(DeviceEpochSelection {
            epoch,
            adapter_ms,
            device_ms,
            reused: false,
        })
    }
}

pub(super) struct DeviceEpochSelection {
    pub(super) epoch: Arc<DeviceEpoch>,
    pub(super) adapter_ms: f64,
    pub(super) device_ms: f64,
    pub(super) reused: bool,
}

pub(super) struct DeviceEpoch {
    generation: u64,
    adapter: wgpu::Adapter,
    device: wgpu::Device,
    queue: wgpu::Queue,
    gate: OverlayGpuGate,
    loss_latch: Arc<DeviceLossLatch>,
}

impl DeviceEpoch {
    pub(super) fn generation(&self) -> u64 {
        self.generation
    }

    pub(super) fn adapter(&self) -> &wgpu::Adapter {
        &self.adapter
    }

    pub(super) fn device(&self) -> &wgpu::Device {
        &self.device
    }

    pub(super) fn queue(&self) -> &wgpu::Queue {
        &self.queue
    }

    pub(super) fn configure_guard(&self) -> RwLockWriteGuard<'_, ()> {
        self.gate.configure()
    }

    pub(super) fn submission_guard(&self) -> RwLockReadGuard<'_, ()> {
        self.gate.submission()
    }

    pub(super) fn is_lost(&self) -> bool {
        self.loss_latch.is_latched_for(self.generation)
    }

    pub(super) fn ensure_alive(&self, operation: &str) -> Result<(), String> {
        if self.is_lost() {
            Err(format!(
                "wgpu overlay device epoch {} was lost before {operation}",
                self.generation
            ))
        } else {
            Ok(())
        }
    }
}

#[derive(Default)]
struct OverlayGpuGate {
    inner: RwLock<()>,
}

impl OverlayGpuGate {
    fn configure(&self) -> RwLockWriteGuard<'_, ()> {
        self.inner
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn submission(&self) -> RwLockReadGuard<'_, ()> {
        self.inner
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

struct DeviceLossLatch {
    generation: u64,
    lost_generation: AtomicU64,
}

impl DeviceLossLatch {
    fn new(generation: u64) -> Self {
        Self {
            generation,
            lost_generation: AtomicU64::new(0),
        }
    }

    fn latch_if_generation(&self, callback_generation: u64) -> bool {
        if callback_generation != self.generation {
            return false;
        }
        self.lost_generation
            .compare_exchange(0, callback_generation, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
    }

    fn is_latched_for(&self, generation: u64) -> bool {
        self.lost_generation.load(Ordering::Acquire) == generation
    }
}

fn find_reusable_epoch_index<T>(
    epochs: &[T],
    mut is_lost: impl FnMut(&T) -> bool,
    mut is_compatible: impl FnMut(&T) -> bool,
) -> Option<usize> {
    epochs
        .iter()
        .position(|epoch| !is_lost(epoch) && is_compatible(epoch))
}

fn lock_unpoisoned<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone, Copy)]
    struct EpochCandidate {
        lost: bool,
        compatible: bool,
    }

    #[test]
    fn epoch_selection_skips_lost_and_reuses_healthy_compatible_epoch() {
        let epochs = [
            EpochCandidate {
                lost: true,
                compatible: true,
            },
            EpochCandidate {
                lost: false,
                compatible: false,
            },
            EpochCandidate {
                lost: false,
                compatible: true,
            },
        ];

        assert_eq!(
            find_reusable_epoch_index(&epochs, |epoch| epoch.lost, |epoch| epoch.compatible),
            Some(2)
        );
        assert_eq!(
            find_reusable_epoch_index(&epochs[..2], |epoch| epoch.lost, |epoch| epoch.compatible),
            None
        );
    }

    #[test]
    fn late_old_generation_loss_cannot_latch_successor() {
        let old_generation = 41;
        let successor_generation = 42;
        let successor = DeviceLossLatch::new(successor_generation);

        assert!(!successor.latch_if_generation(old_generation));
        assert!(!successor.is_latched_for(successor_generation));
        assert!(successor.latch_if_generation(successor_generation));
        assert!(successor.is_latched_for(successor_generation));
    }

    #[test]
    fn configure_and_submission_guards_are_mutually_exclusive() {
        let gate = OverlayGpuGate::default();

        let submission = gate.submission();
        assert!(gate.inner.try_write().is_err());
        drop(submission);

        let configure = gate.configure();
        assert!(gate.inner.try_read().is_err());
        drop(configure);

        assert!(gate.inner.try_read().is_ok());
        assert!(gate.inner.try_write().is_ok());
    }
}
