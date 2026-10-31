//! Agent inputs must use the grid's spelling, so edit keys and pixels identify
//! the same item. Long-name expansion deliberately does not resolve links.
use std::path::Path;

use super::{AgentError, ErrorCode, error, internal};

const MESSAGE: &str = "パスは mIV の一覧と同じ表記で指定してください";

pub(super) fn validate(path: &str) -> Result<&Path, AgentError> {
    validate_with(path, long_name)
}

pub(super) fn cursor_key(path: &str) -> Result<String, AgentError> {
    validate(path).map(key)
}

/// Only separator and case differences are equivalent among admitted inputs.
/// In particular, do not resolve dot components or strip namespace prefixes.
pub(super) fn key(path: &Path) -> String {
    let key = path.to_string_lossy().replace('/', "\\").to_lowercase();
    if key.len() > 3 {
        key.trim_end_matches('\\').to_owned()
    } else {
        key
    }
}

fn invalid() -> AgentError {
    error(ErrorCode::BadRequest, MESSAGE)
}

fn validate_with(
    path: &str,
    lookup: impl FnOnce(&str) -> std::io::Result<Option<String>>,
) -> Result<&Path, AgentError> {
    let spelling = path.replace('/', "\\");
    let bytes = spelling.as_bytes();
    let rest = if bytes.len() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && bytes[2] == b'\\'
    {
        &spelling[3..]
    } else if let Some(unc) = spelling.strip_prefix("\\\\") {
        // A normal UNC path needs both a server and share. This also excludes
        // Win32 device/verbatim prefixes (their first component is . or ?).
        let mut components = unc.split('\\');
        if components.next().is_none_or(str::is_empty)
            || components.next().is_none_or(str::is_empty)
        {
            return Err(invalid());
        }
        unc
    } else {
        return Err(invalid());
    };
    let components = rest.strip_suffix('\\').unwrap_or(rest);
    for component in components.split('\\') {
        // The empty remainder is the drive root, not a repeated separator.
        if component.is_empty() && rest.is_empty() {
            continue;
        }
        if component.is_empty()
            || component.ends_with(['.', ' '])
            || component
                .chars()
                .any(|c| c.is_control() || matches!(c, ':' | '<' | '>' | '"' | '|' | '?' | '*'))
        {
            return Err(invalid());
        }
    }
    if let Some(long) = lookup(path).map_err(|_| internal())? {
        if !same_spelling(&spelling, &long.replace('/', "\\")) {
            return Err(invalid());
        }
    }
    Ok(Path::new(path))
}

#[cfg(windows)]
fn same_spelling(left: &str, right: &str) -> bool {
    use windows::Win32::Globalization::{CSTR_EQUAL, CompareStringOrdinal};
    let left = left.encode_utf16().collect::<Vec<_>>();
    let right = right.encode_utf16().collect::<Vec<_>>();
    unsafe { CompareStringOrdinal(&left, &right, true) == CSTR_EQUAL }
}

#[cfg(not(windows))]
fn same_spelling(left: &str, right: &str) -> bool {
    left.to_lowercase() == right.to_lowercase()
}

#[cfg(windows)]
fn long_name(path: &str) -> std::io::Result<Option<String>> {
    use windows::Win32::Storage::FileSystem::GetLongPathNameW;
    use windows::core::PCWSTR;
    let wide = path.encode_utf16().chain(Some(0)).collect::<Vec<_>>();
    let needed = unsafe { GetLongPathNameW(PCWSTR(wide.as_ptr()), None) };
    if needed == 0 {
        return missing_or_error();
    }
    let mut buffer = vec![0; needed as usize];
    let length = unsafe { GetLongPathNameW(PCWSTR(wide.as_ptr()), Some(&mut buffer)) };
    if length == 0 {
        return missing_or_error();
    }
    if length as usize >= buffer.len() {
        return Err(std::io::Error::other("long path changed during validation"));
    }
    String::from_utf16(&buffer[..length as usize])
        .map(Some)
        .map_err(|_| std::io::Error::other("invalid long path encoding"))
}

#[cfg(windows)]
fn missing_or_error() -> std::io::Result<Option<String>> {
    let error = std::io::Error::last_os_error();
    if matches!(error.raw_os_error(), Some(2 | 3)) {
        // Preserve each tool's normal not-found result. Never canonicalize the
        // nearest existing parent or follow a junction/symlink instead.
        Ok(None)
    } else {
        Err(error)
    }
}

#[cfg(not(windows))]
fn long_name(_: &str) -> std::io::Result<Option<String>> {
    Ok(None)
}

#[cfg(test)]
#[path = "input_path_tests.rs"]
mod tests;
