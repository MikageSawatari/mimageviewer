//! Optional mImageViewer startup milestones. This observer never changes window state.

use std::sync::OnceLock;

static OBSERVER: OnceLock<fn(&'static str, usize)> = OnceLock::new();

/// Install the process-wide observer before starting the native event loop.
pub fn install(observer: fn(&'static str, usize)) {
    let _ = OBSERVER.set(observer);
}

pub(crate) fn enabled() -> bool {
    OBSERVER.get().is_some()
}

pub(crate) fn emit(event: &'static str, hwnd: usize) {
    if let Some(observer) = OBSERVER.get() {
        observer(event, hwnd);
    }
}
