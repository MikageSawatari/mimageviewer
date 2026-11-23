//! Worker-only comparison of a capture destination with the owned host directory.

use std::ffi::{OsStr, OsString};
use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};

/// Whether `destination` is the managed `browser_host_dir` itself or below it.
///
/// The caller supplies its actual host directory; this crate never reads profile settings
/// or environment variables. Existing ancestors are resolved (including junctions and
/// short names), then missing suffixes are appended without creating directories.
/// Dot components left by `absolute` are rejected before resolving any ancestor:
/// appending a month to a verbatim path can otherwise normalize it differently.
/// Windows comparison is ordinal and case insensitive, at component boundaries.
///
/// This performs filesystem I/O: call on a worker, before publication or saving. An
/// inaccessible path, broken link or unsupported namespace is an error, not permission
/// to save. As with ordinary path-based writes, it does not pin ancestors against a
/// concurrent filesystem change by another process.
pub fn is_browser_host_destination(
    destination: &Path,
    browser_host_dir: &Path,
) -> Result<bool, String> {
    let destination = resolved_destination(destination)?;
    let host = resolved_destination(browser_host_dir)?;
    let mut destination = destination.components();
    for component in host.components() {
        let Some(candidate) = destination.next() else {
            return Ok(false);
        };
        if !equal_component(component.as_os_str(), candidate.as_os_str())? {
            return Ok(false);
        }
    }
    Ok(true)
}

fn resolved_destination(path: &Path) -> Result<PathBuf, String> {
    if path.as_os_str().is_empty() {
        return Err("capture destination is empty".into());
    }
    let absolute = std::path::absolute(path).map_err(|error| error.to_string())?;
    if absolute
        .components()
        .any(|component| matches!(component, Component::CurDir | Component::ParentDir))
    {
        return Err("capture destination contains unresolved dot components".into());
    }
    require_filesystem_namespace(&absolute)?;
    let mut ancestor = absolute.as_path();
    let mut suffix = Vec::<OsString>::new();
    let canonical = loop {
        match fs::canonicalize(ancestor) {
            Ok(canonical) => break canonical,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                match fs::symlink_metadata(ancestor) {
                    // A dangling link is an existing entry, not an absent directory.
                    Ok(_) => return Err(error.to_string()),
                    Err(inspect) if inspect.kind() == io::ErrorKind::NotFound => {}
                    Err(inspect) => return Err(inspect.to_string()),
                }
                match ancestor.components().next_back() {
                    Some(Component::Normal(name)) => suffix.push(name.to_os_string()),
                    _ => return Err(error.to_string()),
                }
                ancestor = ancestor.parent().ok_or_else(|| error.to_string())?;
            }
            Err(error) => return Err(error.to_string()),
        }
    };
    if !suffix.is_empty()
        && !fs::metadata(&canonical)
            .map_err(|error| error.to_string())?
            .is_dir()
    {
        return Err("capture destination ancestor is not a directory".into());
    }
    let mut resolved = ordinary_path(&canonical)?;
    for component in suffix.into_iter().rev() {
        resolved.push(component);
    }
    Ok(resolved)
}

fn require_filesystem_namespace(path: &Path) -> Result<(), String> {
    #[cfg(windows)]
    if let Some(Component::Prefix(prefix)) = path.components().next()
        && matches!(
            prefix.kind(),
            std::path::Prefix::Verbatim(_) | std::path::Prefix::DeviceNS(_)
        )
    {
        return Err("unsupported capture destination namespace".into());
    }
    let _ = path;
    Ok(())
}

fn ordinary_path(path: &Path) -> Result<PathBuf, String> {
    require_filesystem_namespace(path)?;
    let mut result = PathBuf::new();
    for component in path.components() {
        match component {
            #[cfg(windows)]
            Component::Prefix(prefix) => match prefix.kind() {
                std::path::Prefix::VerbatimDisk(drive) => {
                    result.push(format!("{}:", char::from(drive)));
                }
                std::path::Prefix::VerbatimUNC(server, share) => {
                    let mut unc = OsString::from(r"\\");
                    unc.push(server);
                    unc.push(r"\");
                    unc.push(share);
                    result.push(unc);
                }
                _ => result.push(prefix.as_os_str()),
            },
            other => result.push(other.as_os_str()),
        }
    }
    Ok(result)
}

fn equal_component(left: &OsStr, right: &OsStr) -> Result<bool, String> {
    if left == right {
        return Ok(true);
    }
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        use windows::Win32::Globalization::{CSTR_EQUAL, CompareStringOrdinal};
        let left: Vec<u16> = left.encode_wide().collect();
        let right: Vec<u16> = right.encode_wide().collect();
        if left.len() > i32::MAX as usize || right.len() > i32::MAX as usize {
            return Err("capture destination component is too long".into());
        }
        let result = unsafe { CompareStringOrdinal(&left, &right, true) };
        if result.0 == 0 {
            return Err(io::Error::last_os_error().to_string());
        }
        Ok(result == CSTR_EQUAL)
    }
    #[cfg(not(windows))]
    Ok(false)
}
