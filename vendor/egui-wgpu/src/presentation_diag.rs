//! mIV presentation observer bridge (backlog 1.139).
//!
//! This is temporary instrumentation. The backend emits allocation-free stage markers and the
//! application decides whether a presentation transition is active before retaining them.

use std::sync::OnceLock;

/// One marker inside immediate-viewport window/surface setup.
#[derive(Clone, Copy, Debug)]
pub struct Event {
    pub stage: &'static str,
    pub phase: &'static str,
    pub viewport: u64,
    pub arg0: u64,
    pub arg1: u64,
}

type Sink = fn(Event);

static SINK: OnceLock<Sink> = OnceLock::new();

/// Install the application-side sink once during startup.
pub fn set_sink(sink: Sink) {
    let _ = SINK.set(sink);
}

/// Emit a marker without allocating or writing to disk.
pub fn emit(
    stage: &'static str,
    phase: &'static str,
    viewport: u64,
    arg0: u64,
    arg1: u64,
) {
    if let Some(sink) = SINK.get() {
        sink(Event {
            stage,
            phase,
            viewport,
            arg0,
            arg1,
        });
    }
}

/// Ensures a begun backend stage receives an end marker on every return path.
pub struct Scope {
    stage: &'static str,
    viewport: u64,
    arg0: u64,
    arg1: u64,
}

impl Scope {
    pub fn begin(stage: &'static str, viewport: u64, arg0: u64, arg1: u64) -> Self {
        emit(stage, "begin", viewport, arg0, arg1);
        Self {
            stage,
            viewport,
            arg0,
            arg1,
        }
    }
}

impl Drop for Scope {
    fn drop(&mut self) {
        emit(
            self.stage,
            "end",
            self.viewport,
            self.arg0,
            self.arg1,
        );
    }
}
