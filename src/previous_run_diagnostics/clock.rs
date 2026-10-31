//! One monotonic origin for normal checkpoints and the preopened panic sink.
//! No correspondence between separately sampled Instant/QPC clocks is needed.
use std::time::Instant;
use windows_sys::Win32::System::Performance::{QueryPerformanceCounter, QueryPerformanceFrequency};

#[derive(Clone, Copy)]
pub(super) enum RunClock {
    Qpc(QpcOrigin),
    // If QPC initialization fails, normal scheduling may still use Instant.
    // The fatal sink remains unavailable rather than mixing clock domains.
    Unavailable(Instant),
}

impl RunClock {
    pub(super) fn new() -> Self {
        let mut frequency = 0;
        if unsafe { QueryPerformanceFrequency(&mut frequency) } != 0
            && frequency > 0
            && let Some(ticks) = qpc_ticks()
        {
            Self::Qpc(QpcOrigin {
                frequency: frequency as u64,
                ticks,
            })
        } else {
            Self::Unavailable(Instant::now())
        }
    }

    pub(super) fn elapsed_ms(self) -> u64 {
        match self {
            Self::Qpc(origin) => origin.elapsed_ms().unwrap_or(0),
            Self::Unavailable(origin) => origin.elapsed().as_millis().min(u64::MAX as u128) as u64,
        }
    }

    pub(super) fn qpc(self) -> Option<QpcOrigin> {
        match self {
            Self::Qpc(origin) => Some(origin),
            Self::Unavailable(_) => None,
        }
    }

    #[cfg(test)]
    pub(super) fn from_ticks(frequency: u64, ticks: u64) -> Self {
        assert_ne!(frequency, 0);
        Self::Qpc(QpcOrigin { frequency, ticks })
    }
}

#[derive(Clone, Copy)]
pub(super) struct QpcOrigin {
    frequency: u64,
    ticks: u64,
}
impl QpcOrigin {
    pub(super) fn elapsed_ms(self) -> Option<u64> {
        self.elapsed_at(qpc_ticks()?)
    }

    pub(super) fn elapsed_at(self, ticks: u64) -> Option<u64> {
        let ticks = ticks.checked_sub(self.ticks)?;
        Some((u128::from(ticks) * 1000 / u128::from(self.frequency)).min(u64::MAX as u128) as u64)
    }
}

fn qpc_ticks() -> Option<u64> {
    let mut ticks = 0;
    (unsafe { QueryPerformanceCounter(&mut ticks) } != 0 && ticks >= 0).then_some(ticks as u64)
}
