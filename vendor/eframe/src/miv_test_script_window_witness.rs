//! mImageViewer-only diagnostics for native viewport callback ownership.
//!
//! The application-side detached-window manager records logical host claims,
//! but those claims cannot distinguish two live `winit::Window` allocations
//! that reused the same HWND. This module gives each actual allocation a
//! process-unique token and exposes the witness only while eframe is invoking
//! application code for that window.

use std::{
    cell::RefCell,
    marker::PhantomData,
    rc::Rc,
    sync::{
        Arc, Mutex, OnceLock, Weak,
        atomic::{AtomicU64, Ordering},
    },
};

use raw_window_handle::{HasWindowHandle as _, RawWindowHandle};
use winit::window::Window;

/// Copyable proof of the native window allocation behind one viewport callback.
#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
pub struct WindowWitness {
    viewport_id: egui::ViewportId,
    hwnd: u64,
    token: u64,
}

impl WindowWitness {
    #[inline]
    pub fn viewport_id(self) -> egui::ViewportId {
        self.viewport_id
    }

    #[inline]
    pub fn hwnd(self) -> u64 {
        self.hwnd
    }

    #[inline]
    pub fn token(self) -> u64 {
        self.token
    }
}

enum WindowAllocation {
    Native(Weak<Window>),
    Fixture(Weak<()>),
}

impl WindowAllocation {
    fn is_alive(&self) -> bool {
        match self {
            Self::Native(window) => window.strong_count() != 0,
            Self::Fixture(instance) => instance.strong_count() != 0,
        }
    }

    fn is_native(&self, window: &Weak<Window>) -> bool {
        matches!(self, Self::Native(existing) if Weak::ptr_eq(existing, window))
    }

    fn is_fixture(&self, instance: &Weak<()>) -> bool {
        matches!(self, Self::Fixture(existing) if Weak::ptr_eq(existing, instance))
    }
}

struct WindowRecord {
    context: egui::Context,
    allocation: WindowAllocation,
    witness: WindowWitness,
}

#[derive(Clone)]
struct ActiveWindow {
    context: egui::Context,
    witness: WindowWitness,
}

static RECORDS: OnceLock<Mutex<Vec<WindowRecord>>> = OnceLock::new();
static NEXT_TOKEN: AtomicU64 = AtomicU64::new(1);

thread_local! {
    static ACTIVE_WINDOW: RefCell<Option<ActiveWindow>> = const { RefCell::new(None) };
}

fn records() -> &'static Mutex<Vec<WindowRecord>> {
    RECORDS.get_or_init(|| Mutex::new(Vec::new()))
}

fn next_token() -> u64 {
    NEXT_TOKEN
        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
            current.checked_add(1)
        })
        .expect("mImageViewer native window witness token space exhausted")
}

fn hwnd(window: &Window) -> Option<u64> {
    let handle = window.window_handle().ok()?.as_raw();
    let RawWindowHandle::Win32(handle) = handle else {
        return None;
    };
    let value = handle.hwnd.get() as usize as u64;
    (value != 0).then_some(value)
}

fn observe_native(
    context: &egui::Context,
    viewport_id: egui::ViewportId,
    window: &Arc<Window>,
) -> Option<WindowWitness> {
    let hwnd = hwnd(window)?;
    let weak = Arc::downgrade(window);
    let mut records = records().lock().expect("window witness registry poisoned");
    records.retain(|record| record.allocation.is_alive());
    if let Some(record) = records.iter().find(|record| {
        record.context == *context
            && record.witness.viewport_id == viewport_id
            && record.witness.hwnd == hwnd
            && record.allocation.is_native(&weak)
    }) {
        return Some(record.witness);
    }
    records.retain(|record| {
        !(record.context == *context && record.witness.viewport_id == viewport_id)
    });
    let witness = WindowWitness {
        viewport_id,
        hwnd,
        token: next_token(),
    };
    records.push(WindowRecord {
        context: context.clone(),
        allocation: WindowAllocation::Native(weak),
        witness,
    });
    Some(witness)
}

fn observe_fixture(
    context: &egui::Context,
    viewport_id: egui::ViewportId,
    hwnd: u64,
    instance: &Arc<()>,
) -> WindowWitness {
    let weak = Arc::downgrade(instance);
    let mut records = records().lock().expect("window witness registry poisoned");
    records.retain(|record| record.allocation.is_alive());
    if let Some(record) = records.iter().find(|record| {
        record.context == *context
            && record.witness.viewport_id == viewport_id
            && record.witness.hwnd == hwnd
            && record.allocation.is_fixture(&weak)
    }) {
        return record.witness;
    }
    records.retain(|record| {
        !(record.context == *context && record.witness.viewport_id == viewport_id)
    });
    let witness = WindowWitness {
        viewport_id,
        hwnd,
        token: next_token(),
    };
    records.push(WindowRecord {
        context: context.clone(),
        allocation: WindowAllocation::Fixture(weak),
        witness,
    });
    witness
}

/// RAII owner of the callback-local witness. Nested immediate viewports restore
/// the exact parent witness, including an explicitly empty parent scope.
pub struct ActiveWindowGuard {
    previous: Option<ActiveWindow>,
    _thread_bound: PhantomData<Rc<()>>,
}

impl ActiveWindowGuard {
    fn enter(context: &egui::Context, witness: Option<WindowWitness>) -> Self {
        let current = witness.map(|witness| ActiveWindow {
            context: context.clone(),
            witness,
        });
        let previous = ACTIVE_WINDOW.with(|active| active.replace(current));
        Self {
            previous,
            _thread_bound: PhantomData,
        }
    }
}

impl Drop for ActiveWindowGuard {
    fn drop(&mut self) {
        let previous = self.previous.take();
        ACTIVE_WINDOW.with(|active| {
            active.replace(previous);
        });
    }
}

pub(crate) fn enter_native_window(
    context: &egui::Context,
    viewport_id: egui::ViewportId,
    window: Option<&Arc<Window>>,
) -> ActiveWindowGuard {
    let witness = window.and_then(|window| observe_native(context, viewport_id, window));
    ActiveWindowGuard::enter(context, witness)
}

/// Return the witness for `viewport_id` in the egui Context that owns the
/// current backend callback. Calls outside a backend callback intentionally
/// return `None`.
pub fn latest(viewport_id: egui::ViewportId) -> Option<WindowWitness> {
    let context =
        ACTIVE_WINDOW.with(|active| active.borrow().as_ref().map(|a| a.context.clone()))?;
    let mut records = records().lock().expect("window witness registry poisoned");
    records.retain(|record| record.allocation.is_alive());
    records
        .iter()
        .find(|record| record.context == context && record.witness.viewport_id == viewport_id)
        .map(|record| record.witness)
}

/// Return the exact window witness for the callback currently invoking user code.
pub fn active() -> Option<WindowWitness> {
    ACTIVE_WINDOW.with(|active| active.borrow().as_ref().map(|active| active.witness))
}

/// Check an exact process-unique allocation witness without relying on the
/// callback-local [`active`] / [`latest`] scope.
///
/// This lookup is deliberately read-only. A worker must not prune dead records
/// (which would drop their `egui::Context` on that worker) or upgrade the weak
/// allocation (which could move the final `Window` drop off the event-loop
/// thread).
pub fn is_current(
    viewport_id: egui::ViewportId,
    hwnd: u64,
    token: u64,
) -> Result<bool, &'static str> {
    let records = records()
        .lock()
        .map_err(|_| "window witness registry poisoned")?;
    Ok(records.iter().any(|record| {
        record.witness.viewport_id == viewport_id
            && record.witness.hwnd == hwnd
            && record.witness.token == token
            && record.allocation.is_alive()
    }))
}

/// Feature-only fixture for application-side ownership tests. It follows the
/// same Context/viewport registry and nested TLS rules without constructing an
/// operating-system window.
#[doc(hidden)]
pub struct WindowWitnessFixture {
    instance: Arc<()>,
}

impl Default for WindowWitnessFixture {
    fn default() -> Self {
        Self::new()
    }
}

impl WindowWitnessFixture {
    pub fn new() -> Self {
        Self {
            instance: Arc::new(()),
        }
    }

    pub fn enter(
        &self,
        context: &egui::Context,
        viewport_id: egui::ViewportId,
        hwnd: u64,
    ) -> ActiveWindowGuard {
        let witness = observe_fixture(context, viewport_id, hwnd, &self.instance);
        ActiveWindowGuard::enter(context, Some(witness))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_are_stable_per_instance_and_never_reused_for_a_replacement() {
        let context = egui::Context::default();
        let viewport = egui::ViewportId::from_hash_of("witness-token");
        let first = WindowWitnessFixture::new();
        let first_token = {
            let _scope = first.enter(&context, viewport, 0x101);
            active().unwrap().token()
        };
        let same_token = {
            let _scope = first.enter(&context, viewport, 0x101);
            active().unwrap().token()
        };
        let replacement = WindowWitnessFixture::new();
        let replacement_token = {
            let _scope = replacement.enter(&context, viewport, 0x101);
            active().unwrap().token()
        };

        assert_eq!(same_token, first_token);
        assert_ne!(replacement_token, first_token);
    }

    #[test]
    fn registry_is_context_scoped_and_nested_scope_restores_parent() {
        let first_context = egui::Context::default();
        let second_context = egui::Context::default();
        let viewport = egui::ViewportId::from_hash_of("shared-viewport-id");
        let first = WindowWitnessFixture::new();
        let second = WindowWitnessFixture::new();

        let first_scope = first.enter(&first_context, viewport, 0x201);
        let first_witness = active().unwrap();
        assert_eq!(latest(viewport), Some(first_witness));
        {
            let _second_scope = second.enter(&second_context, viewport, 0x202);
            let second_witness = active().unwrap();
            assert_ne!(second_witness.token(), first_witness.token());
            assert_eq!(latest(viewport), Some(second_witness));
        }
        assert_eq!(active(), Some(first_witness));
        assert_eq!(latest(viewport), Some(first_witness));
        drop(first_scope);
        assert_eq!(active(), None);
        assert_eq!(latest(viewport), None);
    }

    #[test]
    fn explicitly_empty_scope_hides_and_then_restores_the_parent() {
        let context = egui::Context::default();
        let viewport = egui::ViewportId::from_hash_of("empty-nested-scope");
        let fixture = WindowWitnessFixture::new();

        assert_eq!(active(), None);
        {
            let _outer_empty = ActiveWindowGuard::enter(&context, None);
            assert_eq!(active(), None);
            {
                let _inner = fixture.enter(&context, viewport, 0x301);
                assert!(active().is_some());
                assert!(latest(viewport).is_some());
            }
            assert_eq!(active(), None);
            assert_eq!(latest(viewport), None);
        }
        assert_eq!(active(), None);

        let _parent = fixture.enter(&context, viewport, 0x301);
        let parent = active();
        {
            let _inner_empty = ActiveWindowGuard::enter(&context, None);
            assert_eq!(active(), None);
            assert_eq!(latest(viewport), None);
        }
        assert_eq!(active(), parent);
        assert_eq!(latest(viewport), parent);
    }

    #[test]
    fn exact_liveness_is_available_outside_callback_tls_without_retargeting() {
        let context = egui::Context::default();
        let viewport = egui::ViewportId::from_hash_of("worker-liveness");
        let first = WindowWitnessFixture::new();
        let first_witness = {
            let _scope = first.enter(&context, viewport, 0x401);
            active().unwrap()
        };
        assert_eq!(active(), None);
        assert_eq!(
            is_current(
                first_witness.viewport_id(),
                first_witness.hwnd(),
                first_witness.token()
            ),
            Ok(true)
        );
        assert_eq!(
            is_current(
                egui::ViewportId::from_hash_of("wrong-viewport"),
                first_witness.hwnd(),
                first_witness.token()
            ),
            Ok(false)
        );
        assert_eq!(
            is_current(
                first_witness.viewport_id(),
                first_witness.hwnd() + 1,
                first_witness.token()
            ),
            Ok(false)
        );
        assert_eq!(
            is_current(
                first_witness.viewport_id(),
                first_witness.hwnd(),
                first_witness.token() + 1
            ),
            Ok(false)
        );

        let replacement = WindowWitnessFixture::new();
        let replacement_witness = {
            let _scope = replacement.enter(&context, viewport, 0x401);
            active().unwrap()
        };
        assert_eq!(
            is_current(
                first_witness.viewport_id(),
                first_witness.hwnd(),
                first_witness.token()
            ),
            Ok(false),
            "the old token must not retarget to a replacement with the same HWND"
        );
        assert_eq!(
            is_current(
                replacement_witness.viewport_id(),
                replacement_witness.hwnd(),
                replacement_witness.token()
            ),
            Ok(true)
        );
        drop(replacement);
        assert_eq!(
            is_current(
                replacement_witness.viewport_id(),
                replacement_witness.hwnd(),
                replacement_witness.token()
            ),
            Ok(false)
        );
    }
}
