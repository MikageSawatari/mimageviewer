//! Clock shared by every native viewport and every allocation of its window.
use std::sync::OnceLock;
use web_time::Instant;

static START: OnceLock<Instant> = OnceLock::new();

/// Current process input time in seconds. Integrations sample this alongside
/// their pass clock; button annotations remain absolute when input is queued.
pub fn now() -> f64 {
    at(Instant::now())
}

/// Sample at the same instant used by an integration's pass clock.
pub fn at(instant: Instant) -> f64 {
    instant
        .duration_since(*START.get_or_init(|| instant))
        .as_secs_f64()
}
