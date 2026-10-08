#[cfg(windows)]
pub(crate) fn ticks() -> u64 {
    let mut value = 0;
    // QPC is process-independent; unlike Instant's representation it can be
    // validated and handed across the launcher/core boundary.
    unsafe {
        windows_sys::Win32::System::Performance::QueryPerformanceCounter(&mut value);
    }
    value.max(0) as u64
}
#[cfg(windows)]
pub(crate) fn frequency() -> u64 {
    let mut value = 0;
    unsafe {
        windows_sys::Win32::System::Performance::QueryPerformanceFrequency(&mut value);
    }
    value.max(1) as u64
}
#[cfg(not(windows))]
pub(crate) fn ticks() -> u64 {
    use std::sync::OnceLock;
    use std::time::Instant;
    static START: OnceLock<Instant> = OnceLock::new();
    START
        .get_or_init(Instant::now)
        .elapsed()
        .as_nanos()
        .min(u64::MAX as u128) as u64
}
#[cfg(not(windows))]
pub(crate) fn frequency() -> u64 {
    1_000_000_000
}
pub(crate) fn relative_us(now: u64, origin: u64, frequency: u64) -> u64 {
    ((now.saturating_sub(origin) as u128) * 1_000_000 / (frequency.max(1) as u128))
        .min(super::TIME_MASK as u128) as u64
}
#[cfg(windows)]
pub(crate) fn thread_id() -> u32 {
    unsafe { windows_sys::Win32::System::Threading::GetCurrentThreadId() }
}
#[cfg(not(windows))]
pub(crate) fn thread_id() -> u32 {
    use std::hash::{Hash, Hasher};
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    std::thread::current().id().hash(&mut hash);
    (hash.finish() as u32).max(1)
}
