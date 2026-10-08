//! Core-process startup memory diagnostics. Never changes initialization order.

use std::time::Duration;

const STARTUP_WINDOW: Duration = Duration::from_secs(60);

/// The UI may call this at a startup marker: one memory query, no enumeration
/// or blocking work. Periodic queries run on the sampler thread instead.
pub(crate) fn emit(stage: &str, kind: &str) {
    if !super::is_enabled() {
        return;
    }
    #[cfg(windows)]
    match snapshot() {
        Ok(memory) => super::event(
            "process_memory",
            kind,
            None,
            0,
            &[
                ("stage", stage.into()),
                ("pid", std::process::id().into()),
                ("private_bytes", memory.PrivateUsage.into()),
                ("working_set_bytes", memory.WorkingSetSize.into()),
                ("peak_working_set_bytes", memory.PeakWorkingSetSize.into()),
                ("pagefile_bytes", memory.PagefileUsage.into()),
            ],
        ),
        Err(error) => super::event(
            "process_memory",
            "query_failed",
            None,
            0,
            &[("stage", stage.into()), ("error", error.into())],
        ),
    }
    #[cfg(not(windows))]
    let _ = (stage, kind);
}

#[cfg(windows)]
fn snapshot() -> Result<windows_sys::Win32::System::ProcessStatus::PROCESS_MEMORY_COUNTERS_EX, u32>
{
    use windows_sys::Win32::System::ProcessStatus::{
        GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS, PROCESS_MEMORY_COUNTERS_EX,
    };
    use windows_sys::Win32::System::Threading::GetCurrentProcess;

    // All fields are integers. The EX structure extends the base structure and
    // the API uses cb to select it. GetCurrentProcess returns a borrowed pseudo handle.
    unsafe {
        let mut memory: PROCESS_MEMORY_COUNTERS_EX = std::mem::zeroed();
        memory.cb = std::mem::size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32;
        if GetProcessMemoryInfo(
            GetCurrentProcess(),
            &mut memory as *mut _ as *mut PROCESS_MEMORY_COUNTERS,
            memory.cb,
        ) == 0
        {
            return Err(windows_sys::Win32::Foundation::GetLastError());
        }
        Ok(memory)
    }
}

/// Skip missed ticks rather than producing a burst after a delayed sample.
fn until_next_sample(elapsed: Duration) -> Option<Duration> {
    if elapsed >= STARTUP_WINDOW {
        return None;
    }
    Some(Duration::from_secs(elapsed.as_secs() + 1) - elapsed)
}

pub(super) fn start_sampler() {
    if !super::is_enabled() {
        return;
    }
    let Some(start) = super::program_start() else {
        return;
    };
    if until_next_sample(start.elapsed()).is_none() {
        return;
    }
    #[cfg(windows)]
    if let Err(error) = std::thread::Builder::new()
        .name("startup-memory-sampler".into())
        .spawn(move || {
            while let Some(delay) = until_next_sample(start.elapsed()) {
                std::thread::sleep(delay);
                if start.elapsed() >= STARTUP_WINDOW {
                    break;
                }
                emit("startup_sampler", "sample");
            }
        })
    {
        crate::logger::log(format!("perf: memory sampler spawn failed: {error}"));
    }
}

/// Captures both success and early error returns without changing their paths.
pub(crate) struct Span(&'static str);

pub(crate) fn span(stage: &'static str) -> Option<Span> {
    if !super::is_enabled() {
        return None;
    }
    let start = super::program_start()?;
    if start.elapsed() >= STARTUP_WINDOW {
        return None;
    }
    emit(stage, "begin");
    Some(Span(stage))
}

impl Drop for Span {
    fn drop(&mut self) {
        emit(self.0, "end");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sampler_skips_missed_ticks_and_stops_at_sixty_seconds() {
        assert_eq!(
            until_next_sample(Duration::ZERO),
            Some(Duration::from_secs(1))
        );
        assert_eq!(
            until_next_sample(Duration::from_millis(42_750)),
            Some(Duration::from_millis(250))
        );
        assert_eq!(
            until_next_sample(Duration::from_millis(59_999)),
            Some(Duration::from_millis(1))
        );
        assert_eq!(until_next_sample(Duration::from_secs(60)), None);
        assert_eq!(until_next_sample(Duration::from_secs(600)), None);
    }

    #[test]
    #[cfg(windows)]
    fn extended_counters_read_the_current_test_process() {
        let memory = snapshot().expect("current process memory query");
        assert_eq!(memory.cb as usize, std::mem::size_of_val(&memory));
        assert!(memory.PrivateUsage > 0);
        assert!(memory.WorkingSetSize > 0);
        assert!(memory.PeakWorkingSetSize >= memory.WorkingSetSize);
        assert!(memory.PagefileUsage > 0);
    }
}
