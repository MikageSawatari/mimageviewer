//! Native input ownership follows a backend window allocation, independently of
//! the logical egui viewport and of a renderer's surface configuration.
//!
//! Queries and listeners share this registry. A backend retires its allocation
//! lease synchronously before replacing its window or publishing a successor.
//! Listener callbacks only install/remove native input resources; they must not
//! enter the application or start another backend window lifecycle operation.

use std::{
    collections::BTreeMap,
    sync::{Arc, Weak},
    thread::ThreadId,
};

use egui::mutex::Mutex;
use raw_window_handle::HasWindowHandle as _;

/// The native event source identifier supplied by winit.
pub use winit::window::WindowId as NativeWindowId;

/// A native allocation, rather than a surface resize or logical registration.
///
/// `window_id` is the source used by native event dispatch. On Windows it can be
/// reused after retirement, so copied values do not extend allocation lifetime.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NativeWindowAllocation {
    /// Logical viewport owning this window.
    pub viewport_id: egui::ViewportId,
    /// Logical viewport lifetime which created the native allocation.
    pub registration: egui::ViewportRegistration,
    /// Actual source identifier of this native window.
    pub window_id: NativeWindowId,
    /// Win32 HWND supplied by the owning winit window, when applicable.
    pub windows_hwnd: Option<isize>,
}

/// A non-owning witness to one exact backend allocation lifetime.
///
/// Unlike an HWND/WindowId or a copied descriptor, this identity is never reused
/// while a witness to the old allocation still exists. Keeping or cloning a
/// witness does not keep the input lease or native window owner alive.
#[derive(Clone, Debug)]
pub struct NativeWindowAllocationWitness {
    allocation: NativeWindowAllocation,
    lifetime: Weak<()>,
}

impl NativeWindowAllocationWitness {
    /// Descriptor of the allocation that created this witness.
    pub fn allocation(&self) -> NativeWindowAllocation {
        self.allocation
    }

    /// Whether the unique backend lease still owns this allocation.
    ///
    /// Explicit retirement invalidates witnesses synchronously, independently
    /// of the OS window's continued existence or other retained window Arcs.
    pub fn is_alive(&self) -> bool {
        self.lifetime.strong_count() != 0
    }
}

impl PartialEq for NativeWindowAllocationWitness {
    fn eq(&self, other: &Self) -> bool {
        Weak::ptr_eq(&self.lifetime, &other.lifetime)
    }
}

impl Eq for NativeWindowAllocationWitness {}

/// Native resources installed for exactly one allocation.
pub trait NativeWindowInputLease: Send + Sync {
    /// Remove the subclass, mappings, queued native input and gestures owned by
    /// this allocation. This runs before a successor allocation is published.
    fn retire(&mut self);
}

/// Native input subscriber. Callbacks run synchronously on the backend thread,
/// outside the registry and egui data locks, including immediate child creation.
pub trait NativeWindowInputListener: Send + Sync {
    /// Install input for this exact native source. An error is logged once by
    /// eframe; a subscriber can also enqueue user feedback in its input owner.
    fn install(
        &self,
        allocation: NativeWindowAllocation,
    ) -> Result<Box<dyn NativeWindowInputLease>, String>;
}

struct Entry {
    allocation: NativeWindowAllocation,
    witness: NativeWindowAllocationWitness,
    input: Option<Box<dyn NativeWindowInputLease>>,
}

struct Registry {
    backend_thread: ThreadId,
    listener: Option<Arc<dyn NativeWindowInputListener>>,
    allocations: BTreeMap<egui::ViewportId, Entry>,
}

type SharedRegistry = Arc<Mutex<Registry>>;

fn registry_id() -> egui::Id {
    egui::Id::unique("eframe native window input allocation registry")
}

fn existing_registry(ctx: &egui::Context) -> Option<SharedRegistry> {
    ctx.data(|data| data.get_temp::<SharedRegistry>(registry_id()))
}

fn registry(ctx: &egui::Context) -> SharedRegistry {
    ctx.data_mut(|data| {
        data.get_temp_mut_or_insert_with(registry_id(), || {
            Arc::new(Mutex::new(Registry {
                backend_thread: std::thread::current().id(),
                listener: None,
                allocations: BTreeMap::new(),
            }))
        })
        .clone()
    })
}

/// Return the backend's current allocation for a logical viewport.
/// A copy is only a lookup result; ownership remains with the backend lease.
pub fn native_window_allocation(
    ctx: &egui::Context,
    viewport_id: egui::ViewportId,
) -> Option<NativeWindowAllocation> {
    let registry = existing_registry(ctx)?;
    registry
        .lock()
        .allocations
        .get(&viewport_id)
        .map(|entry| entry.allocation)
}

/// Query the current backend allocation without extending its ownership.
/// Cached witnesses become invalid at explicit native retirement, even when a
/// later allocation reuses the exact same descriptor and HWND.
pub fn native_window_allocation_witness(
    ctx: &egui::Context,
    viewport_id: egui::ViewportId,
) -> Option<NativeWindowAllocationWitness> {
    let registry = existing_registry(ctx)?;
    registry
        .lock()
        .allocations
        .get(&viewport_id)
        .map(|entry| entry.witness.clone())
}

/// Attach the native input subscriber once on the backend thread.
/// Existing allocations (including ROOT before App creation) are replayed once.
pub fn set_native_window_input_listener(
    ctx: &egui::Context,
    listener: Arc<dyn NativeWindowInputListener>,
) -> Result<(), &'static str> {
    let registry = registry(ctx);
    let allocations = {
        let mut state = registry.lock();
        if state.backend_thread != std::thread::current().id() {
            return Err("native input listener must be attached on the backend thread");
        }
        if state.listener.is_some() {
            return Err("native input listener is already attached");
        }
        state.listener = Some(Arc::clone(&listener));
        state
            .allocations
            .values()
            .map(|entry| entry.allocation)
            .collect::<Vec<_>>()
    };
    for allocation in allocations {
        install_input(&registry, &*listener, allocation);
    }
    Ok(())
}

fn install_input(
    registry: &SharedRegistry,
    listener: &dyn NativeWindowInputListener,
    allocation: NativeWindowAllocation,
) {
    match listener.install(allocation) {
        Ok(input) => {
            let mut state = registry.lock();
            let entry = state
                .allocations
                .get_mut(&allocation.viewport_id)
                .expect("native listener must not reenter allocation lifecycle");
            assert_eq!(entry.allocation, allocation);
            assert!(entry.input.is_none());
            entry.input = Some(input);
        }
        Err(error) => log::error!(
            "Native input installation failed for viewport {:?}, source {:?}: {error}",
            allocation.viewport_id,
            allocation.window_id
        ),
    }
}

/// Unique backend owner of a published native allocation.
///
/// This lease is deliberately not Clone. Explicit retirement consumes its
/// responsibility; a later Drop cannot remove a replacement reusing the same
/// viewport, HWND or WindowId. Ending the logical/native owner retires input
/// even if another component still holds an Arc to the old winit window.
pub struct NativeWindowAllocationLease {
    owner: Option<(SharedRegistry, NativeWindowAllocation, Arc<()>)>,
}

impl NativeWindowAllocationLease {
    /// End this allocation once, synchronously, before publishing its successor.
    pub fn retire(&mut self) {
        let Some((registry, allocation, lifetime)) = self.owner.take() else {
            return;
        };
        // Only the unique backend lease holds a strong reference. Invalidate
        // cached host identities before native input teardown can run.
        drop(lifetime);
        let mut entry = {
            let mut state = registry.lock();
            assert_eq!(state.backend_thread, std::thread::current().id());
            let entry = state
                .allocations
                .remove(&allocation.viewport_id)
                .expect("native allocation lease must own its registry entry");
            assert_eq!(entry.allocation, allocation);
            entry
        };
        if let Some(input) = &mut entry.input {
            input.retire();
        }
    }
}

impl Drop for NativeWindowAllocationLease {
    fn drop(&mut self) {
        self.retire();
    }
}

/// Publish an allocation from a native backend and return its unique owner.
///
/// The caller must first retire the previous allocation and remove its native
/// event-dispatch maps. All publication/retirement runs on the backend thread.
/// The same producer can be used by a headless backend with synthetic handles;
/// a copied query value must never be republished by application UI code.
pub fn register_native_window_allocation(
    ctx: &egui::Context,
    allocation: NativeWindowAllocation,
) -> NativeWindowAllocationLease {
    let registry = registry(ctx);
    let lifetime = Arc::new(());
    let listener = {
        let mut state = registry.lock();
        assert_eq!(state.backend_thread, std::thread::current().id());
        assert!(
            !state.allocations.contains_key(&allocation.viewport_id),
            "retire the previous native allocation before publishing its successor"
        );
        assert_eq!(
            ctx.viewport_registration(allocation.viewport_id),
            Some(allocation.registration)
        );
        state.allocations.insert(
            allocation.viewport_id,
            Entry {
                allocation,
                witness: NativeWindowAllocationWitness {
                    allocation,
                    lifetime: Arc::downgrade(&lifetime),
                },
                input: None,
            },
        );
        state.listener.clone()
    };
    let lease = NativeWindowAllocationLease {
        owner: Some((Arc::clone(&registry), allocation, lifetime)),
    };
    if let Some(listener) = listener {
        install_input(&registry, &*listener, allocation);
    }
    lease
}

pub(crate) fn register_window(
    ctx: &egui::Context,
    viewport_id: egui::ViewportId,
    window: &winit::window::Window,
) -> NativeWindowAllocationLease {
    let windows_hwnd = window.window_handle().ok().and_then(|handle| {
        if let raw_window_handle::RawWindowHandle::Win32(handle) = handle.as_raw() {
            Some(handle.hwnd.get())
        } else {
            None
        }
    });
    register_native_window_allocation(
        ctx,
        NativeWindowAllocation {
            viewport_id,
            registration: ctx
                .viewport_registration(viewport_id)
                .expect("native window requires a logical viewport registration"),
            window_id: window.id(),
            windows_hwnd,
        },
    )
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum NativeWindowInputDiscardReason {
    UnmappedWindowId,
}

fn unmapped_primary_press_reason(
    viewport: Option<egui::ViewportId>,
    event: &winit::event::WindowEvent,
) -> Option<NativeWindowInputDiscardReason> {
    use winit::event::{ElementState, MouseButton, WindowEvent};
    (viewport.is_none()
        && matches!(
            event,
            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                button: MouseButton::Left,
                ..
            }
        ))
    .then_some(NativeWindowInputDiscardReason::UnmappedWindowId)
}

/// Observe the actual dispatch boundary; never infer a replacement viewport.
pub(crate) fn log_unmapped_primary_press(
    backend: &str,
    window: NativeWindowId,
    viewport: Option<egui::ViewportId>,
    event: &winit::event::WindowEvent,
) {
    if let Some(reason) = unmapped_primary_press_reason(viewport, event) {
        log::info!(
            target: "eframe::native_window_input::diagnostic",
            "[detached-activation] boundary=backend_input source=ClientClick decision={reason:?} backend={backend} window_id={window:?}"
        );
    }
}

/// Remove the native input owner and every old event source for this viewport.
/// Logical RawInput belongs to the viewport and is retained separately.
pub(crate) fn retire_window<K: Eq + std::hash::Hash, S: std::hash::BuildHasher>(
    input: &mut Option<NativeWindowAllocationLease>,
    viewport: egui::ViewportId,
    viewport_from_window: &mut std::collections::HashMap<K, egui::ViewportId, S>,
) {
    if let Some(mut input) = input.take() {
        input.retire();
    }
    viewport_from_window.retain(|_, id| *id != viewport);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_window_input_unmapped_diagnostic_is_only_a_primary_press() {
        use winit::event::{DeviceId, ElementState, MouseButton, WindowEvent};
        for state in [ElementState::Pressed, ElementState::Released] {
            for button in [MouseButton::Left, MouseButton::Right] {
                let event = WindowEvent::MouseInput {
                    device_id: DeviceId::dummy(),
                    state,
                    button,
                };
                assert_eq!(
                    unmapped_primary_press_reason(None, &event),
                    (state == ElementState::Pressed && button == MouseButton::Left)
                        .then_some(NativeWindowInputDiscardReason::UnmappedWindowId)
                );
                assert_eq!(
                    unmapped_primary_press_reason(Some(egui::ViewportId::ROOT), &event),
                    None
                );
            }
        }
        assert_eq!(
            unmapped_primary_press_reason(None, &WindowEvent::Focused(true)),
            None
        );
    }

    type Calls = Arc<Mutex<Vec<(&'static str, NativeWindowAllocation)>>>;

    struct Listener(Calls);
    struct InputLease(Calls, NativeWindowAllocation);

    impl NativeWindowInputListener for Listener {
        fn install(
            &self,
            allocation: NativeWindowAllocation,
        ) -> Result<Box<dyn NativeWindowInputLease>, String> {
            self.0.lock().push(("install", allocation));
            Ok(Box::new(InputLease(Arc::clone(&self.0), allocation)))
        }
    }

    impl NativeWindowInputLease for InputLease {
        fn retire(&mut self) {
            self.0.lock().push(("retire", self.1));
        }
    }

    fn allocation(
        ctx: &egui::Context,
        viewport_id: egui::ViewportId,
        source: u64,
    ) -> NativeWindowAllocation {
        NativeWindowAllocation {
            viewport_id,
            registration: ctx.viewport_registration(viewport_id).unwrap(),
            window_id: NativeWindowId::from(source),
            windows_hwnd: Some(source as isize),
        }
    }

    #[test]
    fn native_window_input_replays_root_once_and_registers_each_child_once() {
        let ctx = egui::Context::default();
        ctx.set_embed_viewports(false);
        let root = allocation(&ctx, egui::ViewportId::ROOT, 11);
        let root_lease = register_native_window_allocation(&ctx, root);
        let calls = Calls::default();
        set_native_window_input_listener(&ctx, Arc::new(Listener(Arc::clone(&calls)))).unwrap();
        assert!(set_native_window_input_listener(&ctx, Arc::new(Listener(calls.clone()))).is_err());
        let child_id = egui::ViewportId::from_hash_of("native input child");
        ctx.run_ui(Default::default(), |ui| {
            ui.ctx()
                .show_viewport_deferred(child_id, Default::default(), |_, _| {});
        })
        .drop_without_applying_deltas();
        let child = allocation(&ctx, child_id, 12);
        let child_lease = register_native_window_allocation(&ctx, child);
        assert_eq!(
            calls.lock().as_slice(),
            &[("install", root), ("install", child)]
        );
        drop(child_lease);
        assert!(native_window_allocation(&ctx, child_id).is_none());
        assert_eq!(native_window_allocation(&ctx, root.viewport_id), Some(root));
        drop(root_lease);
        assert!(native_window_allocation(&ctx, root.viewport_id).is_none());
        assert_eq!(
            calls.lock().as_slice(),
            &[
                ("install", root),
                ("install", child),
                ("retire", child),
                ("retire", root)
            ]
        );
    }

    #[test]
    fn native_window_input_same_hwnd_reuse_survives_retired_owner_late_drop() {
        let ctx = egui::Context::default();
        let calls = Calls::default();
        set_native_window_input_listener(&ctx, Arc::new(Listener(Arc::clone(&calls)))).unwrap();
        let allocation = allocation(&ctx, egui::ViewportId::ROOT, 21);
        let mut old = register_native_window_allocation(&ctx, allocation);
        let old_witness = native_window_allocation_witness(&ctx, allocation.viewport_id).unwrap();
        let old_cached = old_witness.clone();
        assert_eq!(old_witness, old_cached);
        assert!(old_witness.is_alive());
        assert_eq!(old_witness.lifetime.strong_count(), 1);
        old.retire();
        assert!(!old_witness.is_alive());
        assert!(!old_cached.is_alive());
        assert!(native_window_allocation_witness(&ctx, allocation.viewport_id).is_none());
        assert_eq!(
            calls.lock().as_slice(),
            &[("install", allocation), ("retire", allocation)]
        );
        let new = register_native_window_allocation(&ctx, allocation);
        let new_witness = native_window_allocation_witness(&ctx, allocation.viewport_id).unwrap();
        assert_eq!(old_witness.allocation(), new_witness.allocation());
        assert_ne!(old_witness, new_witness);
        assert!(new_witness.is_alive());
        assert!(!old_witness.is_alive());
        old.retire();
        drop(old);
        assert!(new_witness.is_alive());
        assert_eq!(
            native_window_allocation_witness(&ctx, allocation.viewport_id),
            Some(new_witness.clone())
        );
        assert_eq!(
            native_window_allocation(&ctx, allocation.viewport_id),
            Some(allocation)
        );
        assert_eq!(calls.lock().len(), 3);
        drop(new);
        assert!(!new_witness.is_alive());
        assert_eq!(calls.lock().len(), 4);
    }

    #[test]
    fn native_window_input_witness_invalidates_before_input_retirement() {
        struct Listener(Weak<egui::Context>);
        struct Input(NativeWindowAllocationWitness);
        impl NativeWindowInputListener for Listener {
            fn install(
                &self,
                allocation: NativeWindowAllocation,
            ) -> Result<Box<dyn NativeWindowInputLease>, String> {
                // Callback queries also prove that install runs outside the
                // registry/Context data locks. Weak avoids retaining Context.
                let ctx = self.0.upgrade().unwrap();
                let witness =
                    native_window_allocation_witness(&ctx, allocation.viewport_id).unwrap();
                assert!(witness.is_alive());
                Ok(Box::new(Input(witness)))
            }
        }
        impl NativeWindowInputLease for Input {
            fn retire(&mut self) {
                assert!(
                    !self.0.is_alive(),
                    "host caches must be invalid before input cleanup"
                );
            }
        }
        let ctx = Arc::new(egui::Context::default());
        set_native_window_input_listener(&ctx, Arc::new(Listener(Arc::downgrade(&ctx)))).unwrap();
        let lease =
            register_native_window_allocation(&ctx, allocation(&ctx, egui::ViewportId::ROOT, 25));
        let cached = native_window_allocation_witness(&ctx, egui::ViewportId::ROOT).unwrap();
        drop(lease);
        assert!(!cached.is_alive());
        assert!(native_window_allocation_witness(&ctx, egui::ViewportId::ROOT).is_none());
    }

    #[test]
    fn native_window_input_failed_install_keeps_physical_owner_without_retry() {
        struct Failed(Arc<Mutex<usize>>);
        impl NativeWindowInputListener for Failed {
            fn install(
                &self,
                _: NativeWindowAllocation,
            ) -> Result<Box<dyn NativeWindowInputLease>, String> {
                *self.0.lock() += 1;
                Err("controlled native input failure".into())
            }
        }
        let ctx = egui::Context::default();
        let count = Arc::new(Mutex::new(0));
        set_native_window_input_listener(&ctx, Arc::new(Failed(Arc::clone(&count)))).unwrap();
        let allocation = allocation(&ctx, egui::ViewportId::ROOT, 31);
        let lease = register_native_window_allocation(&ctx, allocation);
        for _ in 0..3 {
            assert_eq!(
                native_window_allocation(&ctx, allocation.viewport_id),
                Some(allocation)
            );
        }
        assert_eq!(*count.lock(), 1);
        drop(lease);
        assert!(native_window_allocation(&ctx, allocation.viewport_id).is_none());
    }
}
