//! Shared capture filenames and Mark of the Web, independent of image decoding and UI.
//! Filesystem writes and browser-host destination checks belong on caller-owned workers.

use std::path::{Path, PathBuf};

mod destination;
pub use destination::is_browser_host_destination;

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct CaptureOrigin {
    pub page_url: Option<String>,
    /// URL written to HostUrl. Browser captures supply the final response URL here.
    pub image_url: Option<String>,
}

impl CaptureOrigin {
    /// Keep the clicked image URL separate at the caller: MOTW records the final URL.
    /// URL sanitization remains at the shared MOTW boundary, as for clipboard captures.
    pub fn from_browser_urls(page_url: impl Into<String>, final_url: impl Into<String>) -> Self {
        Self {
            page_url: Some(page_url.into()),
            image_url: Some(final_url.into()),
        }
    }

    pub fn domain(&self) -> Option<String> {
        self.page_url
            .as_deref()
            .or(self.image_url.as_deref())
            .and_then(|value| url::Url::parse(value).ok())
            .and_then(|value| value.host_str().map(str::to_owned))
    }
}

pub fn sanitize_url(value: &str) -> Option<String> {
    // Reject literal control characters instead of allowing URL parsing to
    // erase line breaks supplied for a Zone.Identifier injection.
    if value.chars().any(char::is_control) {
        return None;
    }
    let mut parsed = url::Url::parse(value).ok()?;
    if !matches!(parsed.scheme(), "http" | "https") || parsed.host_str().is_none() {
        return None;
    }
    parsed.set_username("").ok()?;
    parsed.set_password(None).ok()?;
    parsed.set_fragment(None);
    Some(parsed.to_string())
}

#[derive(Debug, Clone)]
pub struct CaptureTimestamp {
    pub stamp: String,
    pub month: String,
}

impl CaptureTimestamp {
    pub fn now() -> Self {
        #[cfg(windows)]
        {
            let time = unsafe { windows::Win32::System::SystemInformation::GetLocalTime() };
            Self {
                stamp: format!(
                    "{:04}{:02}{:02}-{:02}{:02}{:02}-{:03}",
                    time.wYear,
                    time.wMonth,
                    time.wDay,
                    time.wHour,
                    time.wMinute,
                    time.wSecond,
                    time.wMilliseconds
                ),
                month: format!("{:04}-{:02}", time.wYear, time.wMonth),
            }
        }
        #[cfg(not(windows))]
        {
            // Production is Windows-only; deterministic UTC fallback for other
            // targets uses the UNIX date without a platform time dependency.
            let elapsed = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default();
            let days = elapsed.as_secs() / 86400;
            let z = days as i64 + 719468;
            let era = z / 146097;
            let doe = z - era * 146097;
            let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
            let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
            let mp = (5 * doy + 2) / 153;
            let day = doy - (153 * mp + 2) / 5 + 1;
            let month = mp + if mp < 10 { 3 } else { -9 };
            let year = yoe + era * 400 + i64::from(month <= 2);
            let seconds = elapsed.as_secs() % 86400;
            Self {
                stamp: format!(
                    "{year:04}{month:02}{day:02}-{:02}{:02}{:02}-{:03}",
                    seconds / 3600,
                    seconds / 60 % 60,
                    seconds % 60,
                    elapsed.subsec_millis()
                ),
                month: format!("{year:04}-{month:02}"),
            }
        }
    }
}

pub fn capture_filename(
    timestamp: &CaptureTimestamp,
    domain: Option<&str>,
    index: usize,
    total: usize,
    collision: usize,
    extension: &str,
) -> String {
    let suffix = if collision > 1 {
        format!("-{collision}")
    } else {
        String::new()
    };
    let domain = domain
        .map(|value| {
            value
                .chars()
                .map(|c| {
                    if c == '_' || c.is_control() || "<>:\"/\\|?*".contains(c) {
                        '-'
                    } else {
                        c
                    }
                })
                .collect::<String>()
        })
        .filter(|value| !value.is_empty())
        .map(|value| format!("-{value}"))
        .unwrap_or_default();
    let digits = total.max(1).to_string().len().max(2);
    format!(
        "{}{suffix}{domain}_{index:0digits$}.{extension}",
        timestamp.stamp
    )
}

pub fn motw_contents(origin: &CaptureOrigin) -> Option<String> {
    let page = origin.page_url.as_deref().and_then(sanitize_url);
    let image = origin.image_url.as_deref().and_then(sanitize_url);
    if page.is_none() && image.is_none() {
        return None;
    }
    let mut result = "[ZoneTransfer]\r\nZoneId=3\r\n".to_owned();
    if let Some(value) = page {
        result.push_str(&format!("ReferrerUrl={value}\r\n"));
    }
    if let Some(value) = image {
        result.push_str(&format!("HostUrl={value}\r\n"));
    }
    Some(result)
}

pub fn write_motw(path: &Path, origin: &CaptureOrigin) -> Result<(), String> {
    #[cfg(windows)]
    if let Some(contents) = motw_contents(origin) {
        let mut ads = path.as_os_str().to_os_string();
        ads.push(":Zone.Identifier");
        std::fs::write(PathBuf::from(ads), contents)
            .map_err(|error| format!("出どころを記録できませんでした: {error}"))?;
    }
    #[cfg(not(windows))]
    let _ = (path, origin);
    Ok(())
}

#[cfg(test)]
mod tests;
