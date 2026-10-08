//! Read-only capture evidence for the opt-in diagnostic build.
//! Fixture writers are harness operations, not capture-reader opens.

use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

static OPEN_ATTEMPTS: AtomicU64 = AtomicU64::new(0);
static AUTOMATIC_SEQUENCE: AtomicU32 = AtomicU32::new(0);
static POPUP_SHOWS: AtomicU64 = AtomicU64::new(0);
static POPUP_REQUESTS: AtomicU64 = AtomicU64::new(0);
static SAVE_REQUESTS: AtomicU64 = AtomicU64::new(0);
static PASTES: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct CaptureDiagnostics {
    pub open_count: u64,
    pub automatic_sequence: u32,
    pub popup_shows: u64,
    pub popup_requests: u64,
    pub save_requests: u64,
    pub paste_count: u64,
}

pub(super) fn reader_open_attempt() {
    OPEN_ATTEMPTS.fetch_add(1, Ordering::Relaxed);
}

pub(super) fn automatic_completed(sequence: u32) {
    AUTOMATIC_SEQUENCE.store(sequence, Ordering::Release);
}

pub(super) fn popup_shown() {
    POPUP_SHOWS.fetch_add(1, Ordering::Relaxed);
}

pub(super) fn popup_requested() {
    POPUP_REQUESTS.fetch_add(1, Ordering::Relaxed);
}

pub(super) fn save_requested() {
    SAVE_REQUESTS.fetch_add(1, Ordering::Relaxed);
}

pub(crate) fn paste_dispatched() {
    PASTES.fetch_add(1, Ordering::Relaxed);
}

pub(crate) fn snapshot() -> CaptureDiagnostics {
    // Observe the completion fence before the counters it covers.
    let automatic_sequence = AUTOMATIC_SEQUENCE.load(Ordering::Acquire);
    CaptureDiagnostics {
        automatic_sequence,
        open_count: OPEN_ATTEMPTS.load(Ordering::Relaxed),
        popup_shows: POPUP_SHOWS.load(Ordering::Relaxed),
        popup_requests: POPUP_REQUESTS.load(Ordering::Relaxed),
        save_requests: SAVE_REQUESTS.load(Ordering::Relaxed),
        paste_count: PASTES.load(Ordering::Relaxed),
    }
}
