//! Opt-in native-video cursor diagnostics.
//!
//! `MIV_CURSOR_DEBUG=1` enables high-volume, order-preserving diagnostics in
//! the normal `mimageviewer.log`. The logger supplies the monotonic timestamp
//! and thread id; this module adds a process-wide sequence number so events
//! that share the same displayed millisecond remain unambiguous.

use std::fmt;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};

static ENABLED: OnceLock<bool> = OnceLock::new();
static SEQUENCE: AtomicU64 = AtomicU64::new(1);

pub(crate) fn enabled() -> bool {
    *ENABLED.get_or_init(|| std::env::var_os("MIV_CURSOR_DEBUG").is_some())
}

pub(crate) fn log(args: fmt::Arguments<'_>) {
    if !enabled() {
        return;
    }
    let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
    crate::logger::log(format!("[CURSOR-DEBUG][seq={sequence:08}] {args}"));
}
