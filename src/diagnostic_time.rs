//! Numeric local dates for allowlisted diagnostics. Resolve the time zone on a
//! normal worker, never while drawing a notice. No paths or zone names escape.
use windows::Win32::Foundation::{FILETIME, SYSTEMTIME};
use windows::Win32::System::Time::{
    FileTimeToSystemTime, SystemTimeToFileTime, SystemTimeToTzSpecificLocalTimeEx,
};

const UNIX_EPOCH: u64 = 116_444_736_000_000_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct LocalDateTime {
    year: u16,
    month: u16,
    day: u16,
    hour: u16,
    minute: u16,
    second: u16,
    offset_minutes: i32,
}

fn file_time(ticks: u64) -> FILETIME {
    FILETIME {
        dwLowDateTime: ticks as u32,
        dwHighDateTime: (ticks >> 32) as u32,
    }
}
fn system_time(ticks: u64) -> Option<SYSTEMTIME> {
    let mut time = SYSTEMTIME::default();
    unsafe {
        FileTimeToSystemTime(&file_time(ticks), &mut time).ok()?;
    }
    Some(time)
}
impl LocalDateTime {
    pub(crate) fn from_utc_ms(ms: u64) -> Option<Self> {
        let ticks = ms.checked_mul(10_000)?.checked_add(UNIX_EPOCH)?;
        let utc = system_time(ticks)?;
        let mut local = SYSTEMTIME::default();
        unsafe {
            SystemTimeToTzSpecificLocalTimeEx(None, &utc, &mut local).ok()?;
        }
        let mut local_ticks = FILETIME::default();
        unsafe {
            SystemTimeToFileTime(&local, &mut local_ticks).ok()?;
        }
        let local_ticks =
            (u64::from(local_ticks.dwHighDateTime) << 32) | u64::from(local_ticks.dwLowDateTime);
        let offset = (i128::from(local_ticks) - i128::from(ticks)) / 600_000_000;
        Self::from_system(local, i32::try_from(offset).ok()?)
    }
    fn from_system(time: SYSTEMTIME, offset_minutes: i32) -> Option<Self> {
        if time.wYear > 9999 || offset_minutes.unsigned_abs() > 24 * 60 {
            return None;
        }
        Some(Self {
            year: time.wYear,
            month: time.wMonth,
            day: time.wDay,
            hour: time.wHour,
            minute: time.wMinute,
            second: time.wSecond,
            offset_minutes,
        })
    }
    pub(crate) fn now() -> Option<Self> {
        let ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .ok()?
            .as_millis();
        Self::from_utc_ms(ms.try_into().ok()?)
    }
    /// Fixed offset fixtures don't depend on the machine's time zone or DST.
    pub(crate) fn with_offset(ms: u64, offset_minutes: i32) -> Option<Self> {
        let ticks = i128::from(ms) * 10_000
            + i128::from(UNIX_EPOCH)
            + i128::from(offset_minutes) * 600_000_000;
        Self::from_system(system_time(ticks.try_into().ok()?)?, offset_minutes)
    }
}
impl std::fmt::Display for LocalDateTime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let sign = if self.offset_minutes < 0 { '-' } else { '+' };
        let minutes = self.offset_minutes.unsigned_abs();
        write!(
            f,
            "{:04}-{:02}-{:02} {:02}:{:02}:{:02} (UTC{sign}{:02}:{:02})",
            self.year,
            self.month,
            self.day,
            self.hour,
            self.minute,
            self.second,
            minutes / 60,
            minutes % 60
        )
    }
}
pub(crate) fn display(value: Option<LocalDateTime>) -> String {
    value.map_or_else(|| "時刻不明".into(), |value| value.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn diagnostic_time_local_dates_have_explicit_offsets_and_date_rollover() {
        assert_eq!(
            LocalDateTime::with_offset(0, 540).unwrap().to_string(),
            "1970-01-01 09:00:00 (UTC+09:00)"
        );
        assert_eq!(
            LocalDateTime::with_offset(0, -300).unwrap().to_string(),
            "1969-12-31 19:00:00 (UTC-05:00)"
        );
        assert_eq!(
            LocalDateTime::with_offset(0, 330).unwrap().to_string(),
            "1970-01-01 05:30:00 (UTC+05:30)"
        );
        assert!(LocalDateTime::from_utc_ms(u64::MAX).is_none());
        assert!(LocalDateTime::with_offset(u64::MAX, 540).is_none());
        assert_eq!(display(None), "時刻不明");
    }
    #[test]
    fn diagnostic_time_system_zone_conversion_preserves_the_instant() {
        for ms in [1_767_225_600_123, 1_782_864_000_789, 1_793_355_825_000] {
            let local = LocalDateTime::from_utc_ms(ms).unwrap();
            assert_eq!(
                Some(local),
                LocalDateTime::with_offset(ms, local.offset_minutes)
            );
        }
    }
}
