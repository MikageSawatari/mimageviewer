//! Filesystem identity for migration admission. Run on the startup worker or
//! the early diagnostic route, before opening logs/settings at a supplied path.

use std::ffi::{OsStr, OsString};
use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};

pub(super) fn same_location(left: &Path, right: &Path) -> Result<bool, String> {
    equal_component(resolved(left)?.as_os_str(), resolved(right)?.as_os_str())
}

pub(super) fn overlapping_locations(left: &Path, right: &Path) -> Result<bool, String> {
    let left = resolved(left)?;
    let right = resolved(right)?;
    Ok(is_prefix(&left, &right)? || is_prefix(&right, &left)?)
}

fn is_prefix(parent: &Path, child: &Path) -> Result<bool, String> {
    let mut child = child.components();
    for parent in parent.components() {
        let Some(child) = child.next() else {
            return Ok(false);
        };
        if !equal_component(parent.as_os_str(), child.as_os_str())? {
            return Ok(false);
        }
    }
    Ok(true)
}

fn resolved(path: &Path) -> Result<PathBuf, String> {
    let absolute =
        std::path::absolute(path).map_err(|error| path_error("make absolute", path, error))?;
    require_filesystem_namespace(&absolute)?;
    let mut ancestor = absolute.as_path();
    let mut missing = Vec::<OsString>::new();
    let canonical = loop {
        // Preserve the original spelling until the kernel resolves it. In a
        // verbatim Windows path, junction/.. must not be collapsed before open.
        match fs::canonicalize(ancestor) {
            Ok(canonical) => break canonical,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                match fs::symlink_metadata(ancestor) {
                    // A broken existing reparse point is not a missing child.
                    Ok(_) => return Err(path_error("canonicalize existing path", ancestor, error)),
                    Err(inspect) if inspect.kind() == io::ErrorKind::NotFound => {}
                    Err(inspect) => {
                        return Err(path_error("inspect existing ancestor", ancestor, inspect));
                    }
                }
                match ancestor.components().next_back() {
                    Some(Component::Normal(name)) => missing.push(name.to_os_string()),
                    Some(Component::CurDir) => {}
                    Some(Component::ParentDir) => {
                        return Err(format!(
                            "migration path has an unresolved parent traversal: {}",
                            path.display()
                        ));
                    }
                    _ => {
                        return Err(format!(
                            "migration path has no existing ancestor: {}",
                            path.display()
                        ));
                    }
                }
                ancestor = ancestor
                    .parent()
                    .ok_or_else(|| format!("migration path has no parent: {}", path.display()))?;
            }
            Err(error) => {
                return Err(path_error(
                    "canonicalize existing ancestor",
                    ancestor,
                    error,
                ));
            }
        }
    };
    // Existing symlinks/junctions and short-name spellings are resolved here.
    // A broken link or inaccessible ancestor is an error, never "different".
    if !missing.is_empty()
        && !fs::metadata(&canonical)
            .map_err(|error| path_error("inspect canonical ancestor", &canonical, error))?
            .is_dir()
    {
        return Err(format!(
            "migration path ancestor is not a directory: {}",
            canonical.display()
        ));
    }
    let mut result = lexical_normalize(&canonical)?;
    for name in missing.into_iter().rev() {
        result.push(name);
    }
    lexical_normalize(&result)
}

fn require_filesystem_namespace(path: &Path) -> Result<(), String> {
    #[cfg(windows)]
    {
        use std::path::Prefix;
        if let Some(Component::Prefix(prefix)) = path.components().next()
            && matches!(prefix.kind(), Prefix::Verbatim(_) | Prefix::DeviceNS(_))
        {
            return Err(format!(
                "unsupported migration filesystem namespace: {}",
                path.display()
            ));
        }
    }
    #[cfg(not(windows))]
    let _ = path;
    Ok(())
}

fn lexical_normalize(path: &Path) -> Result<PathBuf, String> {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                // All callers pass absolute paths; never pop the root/prefix.
                if matches!(
                    normalized.components().next_back(),
                    Some(Component::Normal(_))
                ) {
                    normalized.pop();
                }
            }
            #[cfg(windows)]
            Component::Prefix(prefix) => {
                use std::path::Prefix;
                match prefix.kind() {
                    Prefix::VerbatimDisk(drive) => {
                        normalized.push(format!("{}:", char::from(drive)));
                    }
                    Prefix::VerbatimUNC(server, share) => {
                        let mut ordinary = OsString::from(r"\\");
                        ordinary.push(server);
                        ordinary.push(r"\");
                        ordinary.push(share);
                        normalized.push(ordinary);
                    }
                    Prefix::Verbatim(_) | Prefix::DeviceNS(_) => {
                        return Err(format!(
                            "unsupported migration filesystem namespace: {}",
                            path.display()
                        ));
                    }
                    _ => normalized.push(prefix.as_os_str()),
                }
            }
            other => normalized.push(other.as_os_str()),
        }
    }
    Ok(normalized)
}

fn equal_component(left: &OsStr, right: &OsStr) -> Result<bool, String> {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        use windows::Win32::Globalization::{CSTR_EQUAL, CompareStringOrdinal};
        if left == right {
            return Ok(true);
        }
        let left: Vec<u16> = left.encode_wide().collect();
        let right: Vec<u16> = right.encode_wide().collect();
        if left.len() > i32::MAX as usize || right.len() > i32::MAX as usize {
            return Err("migration path exceeds ordinal comparison length".into());
        }
        let compared = unsafe { CompareStringOrdinal(&left, &right, true) };
        if compared.0 == 0 {
            return Err(format!(
                "compare migration path: {}",
                io::Error::last_os_error()
            ));
        }
        Ok(compared == CSTR_EQUAL)
    }
    #[cfg(not(windows))]
    {
        Ok(left == right)
    }
}

fn path_error(stage: &str, path: &Path, error: io::Error) -> String {
    format!("{stage} {}: {error}", path.display())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn existing_and_missing_descendant_spellings_resolve_to_same_location() {
        let fixture = tempfile::tempdir().unwrap();
        let existing = fixture.path().join("existing");
        fs::create_dir(&existing).unwrap();
        fs::create_dir(existing.join("present")).unwrap();
        let missing = existing.join("future").join("child");
        assert!(same_location(&missing, &existing.join(".").join("future").join("child")).unwrap());
        assert!(
            same_location(
                &missing,
                &existing
                    .join("present")
                    .join("..")
                    .join("future")
                    .join("child")
            )
            .unwrap()
        );
        assert!(overlapping_locations(&existing, &missing).unwrap());
        assert!(overlapping_locations(&missing, &existing).unwrap());
        assert!(!same_location(&existing, &missing).unwrap());
        assert!(
            !overlapping_locations(&existing, &fixture.path().join("existing-sibling")).unwrap()
        );
    }

    #[test]
    fn missing_parent_components_follow_native_resolution_without_creating_data() {
        let fixture = tempfile::tempdir().unwrap();
        // canonicalize supplies the verbatim prefix on Windows, preserving the
        // missing/.. spelling through std::path::absolute. Native resolution
        // may accept that parent despite "missing" not existing.
        let canonical = fs::canonicalize(fixture.path()).unwrap();
        let ambiguous = canonical.join("missing").join("..").join("child");
        match fs::canonicalize(ambiguous.parent().unwrap()) {
            Ok(native_parent) => {
                assert!(same_location(&ambiguous, &native_parent.join("child")).unwrap());
                assert!(overlapping_locations(&ambiguous, &native_parent).unwrap());
            }
            Err(_) => {
                assert!(same_location(&ambiguous, &canonical.join("child")).is_err());
                assert!(overlapping_locations(&ambiguous, &canonical).is_err());
            }
        }
        assert!(!canonical.join("missing").exists());
        assert!(!canonical.join("child").exists());
    }

    #[test]
    fn existing_parent_traversal_is_resolved_before_comparison() {
        let fixture = tempfile::tempdir().unwrap();
        fs::create_dir(fixture.path().join("present")).unwrap();
        let canonical = fs::canonicalize(fixture.path()).unwrap();
        let traversed = canonical.join("present").join("..");
        if fs::canonicalize(&traversed).is_ok() {
            assert!(same_location(&traversed, &canonical).unwrap());
            assert!(overlapping_locations(&traversed.join("future"), &canonical).unwrap());
        } else {
            // Some native namespace/filesystem combinations reject verbatim
            // parent components. Lexical normalization must not admit them.
            assert!(same_location(&traversed, &canonical).is_err());
            assert!(overlapping_locations(&traversed.join("future"), &canonical).is_err());
        }
    }

    #[test]
    fn unexpected_io_errors_are_not_treated_as_distinct_paths() {
        let fixture = tempfile::tempdir().unwrap();
        let file = fixture.path().join("file");
        fs::write(&file, b"fixture").unwrap();
        assert!(same_location(&file.join("child"), fixture.path()).is_err());
        assert!(overlapping_locations(&file.join("child"), fixture.path()).is_err());
    }

    #[cfg(windows)]
    #[test]
    fn windows_verbatim_case_and_dot_aliases_cannot_bypass_profile_comparison() {
        let fixture = tempfile::tempdir().unwrap();
        let existing = fixture.path().join("MixedCase");
        fs::create_dir(&existing).unwrap();
        let canonical = fs::canonicalize(&existing).unwrap();
        assert!(same_location(&existing, &canonical).unwrap());
        assert!(same_location(&existing, &existing.join(".")).unwrap());
        assert!(same_location(&existing, &existing.join("unused").join("..")).unwrap());
        let upper = PathBuf::from(existing.to_string_lossy().to_uppercase());
        assert!(same_location(&existing, &upper).unwrap());
        assert!(overlapping_locations(&canonical, &upper.join("not-created")).unwrap());
        let future = existing.join("not-created");
        let verbatim_future = canonical.join("not-created");
        assert!(same_location(&future, &verbatim_future).unwrap());
    }

    #[cfg(windows)]
    #[test]
    fn windows_unc_verbatim_normalization_is_lexical_and_contacts_no_share() {
        assert_eq!(
            lexical_normalize(Path::new(r"\\?\UNC\server\share\first\..\child")).unwrap(),
            PathBuf::from(r"\\server\share\child")
        );
        assert!(equal_component(OsStr::new("MixedCase"), OsStr::new("MIXEDCASE")).unwrap());
        assert!(lexical_normalize(Path::new(r"\\.\pipe\untrusted")).is_err());
    }
}
