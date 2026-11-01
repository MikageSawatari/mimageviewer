//! Reject syntactic aliases, but retain producer-returned short names. Conceal
//! compares component names from the request; stored edit keys never become paths.
use std::path::{Path, PathBuf};

use super::{AgentError, ErrorCode, error, internal};

const MESSAGE: &str = "パスは mIV の一覧と同じ表記で指定してください";

pub(super) fn validate(path: &str) -> Result<&Path, AgentError> {
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
    Ok(Path::new(path))
}

pub(super) fn cursor_key(path: &str) -> Result<String, AgentError> {
    validate(path).map(key)
}

/// Cursor identity retains the accepted spelling; it does not merge short
/// names, resolve dot components, strip prefixes, or follow links.
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum NameForm {
    Long,
    Short,
}

pub(super) fn edit_spellings(path: &Path) -> Result<Vec<PathBuf>, AgentError> {
    edit_spellings_with(path, lookup_name)
}

pub(super) fn component_names(path: &Path) -> Result<Option<Vec<[String; 2]>>, AgentError> {
    // Only the original request is queried. A normalized edit key is lossy and
    // cannot be used as a filesystem path, including as a lookup-cache key.
    let Some(long) = lookup_name(path, NameForm::Long).map_err(|_| internal())? else {
        return Ok(None);
    };
    let Some(short) = lookup_name(path, NameForm::Short).map_err(|_| internal())? else {
        return Ok(None);
    };
    let long = crate::adjustment_db::normalize_path(&long);
    let short = crate::adjustment_db::normalize_path(&short);
    let long: Vec<_> = long.split('/').collect();
    let short: Vec<_> = short.split('/').collect();
    if long.len() != short.len() {
        return Err(internal());
    }
    Ok(Some(
        long.into_iter()
            .zip(short)
            .map(|(long, short)| [long.to_owned(), short.to_owned()])
            .collect(),
    ))
}

fn edit_spellings_with(
    path: &Path,
    mut lookup: impl FnMut(&Path, NameForm) -> std::io::Result<Option<PathBuf>>,
) -> Result<Vec<PathBuf>, AgentError> {
    let mut paths = vec![path.to_owned()];
    for form in [NameForm::Long, NameForm::Short] {
        if let Some(alternative) = lookup(path, form).map_err(|_| internal())? {
            if !paths
                .iter()
                .any(|existing| key(existing) == key(&alternative))
            {
                paths.push(alternative);
            }
        }
    }
    Ok(paths)
}

fn lookup_name(path: &Path, form: NameForm) -> std::io::Result<Option<PathBuf>> {
    #[cfg(test)]
    if let Some(result) =
        TEST_NAME_LOOKUP.with(|slot| slot.borrow().as_ref().map(|lookup| lookup(path, form)))
    {
        return result;
    }
    native_name(path, form)
}

#[cfg(windows)]
pub(super) fn native_name(path: &Path, form: NameForm) -> std::io::Result<Option<PathBuf>> {
    use std::os::windows::ffi::{OsStrExt, OsStringExt};
    use windows::Win32::Storage::FileSystem::{GetLongPathNameW, GetShortPathNameW};
    use windows::core::PCWSTR;
    let wide = path
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect::<Vec<_>>();
    let query = |buffer: Option<&mut [u16]>| unsafe {
        match form {
            NameForm::Long => GetLongPathNameW(PCWSTR(wide.as_ptr()), buffer),
            NameForm::Short => GetShortPathNameW(PCWSTR(wide.as_ptr()), buffer),
        }
    };
    let needed = query(None);
    if needed == 0 {
        return missing_or_error();
    }
    let mut buffer = vec![0; needed as usize];
    let length = query(Some(&mut buffer));
    if length == 0 {
        return missing_or_error();
    }
    if length as usize >= buffer.len() {
        return Err(std::io::Error::other("path spelling changed during lookup"));
    }
    Ok(Some(PathBuf::from(std::ffi::OsString::from_wide(
        &buffer[..length as usize],
    ))))
}

#[cfg(windows)]
fn missing_or_error() -> std::io::Result<Option<PathBuf>> {
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
pub(super) fn native_name(_: &Path, _: NameForm) -> std::io::Result<Option<PathBuf>> {
    Ok(None)
}

#[cfg(test)]
type NameLookup = dyn Fn(&Path, NameForm) -> std::io::Result<Option<PathBuf>>;

#[cfg(test)]
thread_local! {
    static TEST_NAME_LOOKUP: std::cell::RefCell<Option<Box<NameLookup>>> = const {
        std::cell::RefCell::new(None)
    };
}

#[cfg(test)]
pub(super) fn with_name_lookup<T>(
    lookup: impl Fn(&Path, NameForm) -> std::io::Result<Option<PathBuf>> + 'static,
    action: impl FnOnce() -> T,
) -> T {
    struct Restore(Option<Box<NameLookup>>);
    impl Drop for Restore {
        fn drop(&mut self) {
            TEST_NAME_LOOKUP.with(|slot| *slot.borrow_mut() = self.0.take());
        }
    }
    let _restore = Restore(TEST_NAME_LOOKUP.with(|slot| slot.replace(Some(Box::new(lookup)))));
    action()
}

#[cfg(test)]
#[path = "input_path_tests.rs"]
mod tests;
