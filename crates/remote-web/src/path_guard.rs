use std::path::{Component, Path, PathBuf};

#[derive(Debug)]
pub enum ResolveError {
    InvalidRelativePath,
    Unavailable,
    EscapesFavorite,
}

pub fn resolve_existing(root: &Path, relative: &str) -> Result<PathBuf, ResolveError> {
    validate_relative(relative)?;

    let canonical_root = std::fs::canonicalize(root).map_err(|_| ResolveError::Unavailable)?;
    let candidate = if relative.is_empty() {
        canonical_root.clone()
    } else {
        canonical_root.join(relative)
    };
    let canonical_candidate =
        std::fs::canonicalize(candidate).map_err(|_| ResolveError::Unavailable)?;

    if !path_starts_with(&canonical_candidate, &canonical_root) {
        return Err(ResolveError::EscapesFavorite);
    }
    Ok(canonical_candidate)
}

fn validate_relative(relative: &str) -> Result<(), ResolveError> {
    if relative.contains('\0') || looks_like_windows_absolute_or_drive_path(relative) {
        return Err(ResolveError::InvalidRelativePath);
    }

    for component in Path::new(relative).components() {
        match component {
            Component::Prefix(_) | Component::RootDir | Component::ParentDir => {
                return Err(ResolveError::InvalidRelativePath);
            }
            Component::CurDir | Component::Normal(_) => {}
        }
    }

    // `Path::components` follows the host platform. Keep Windows separators and
    // drive-relative syntax unsafe even when these tests run on another host.
    if relative.split(['/', '\\']).any(|segment| segment == "..") {
        return Err(ResolveError::InvalidRelativePath);
    }
    Ok(())
}

fn looks_like_windows_absolute_or_drive_path(relative: &str) -> bool {
    let bytes = relative.as_bytes();
    relative.starts_with(['/', '\\'])
        || (bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':')
}

#[cfg(windows)]
fn path_starts_with(path: &Path, root: &Path) -> bool {
    let mut path_components = path.components();
    for root_component in root.components() {
        let Some(path_component) = path_components.next() else {
            return false;
        };
        if !component_eq_ignore_ascii_case(path_component, root_component) {
            return false;
        }
    }
    true
}

#[cfg(windows)]
fn component_eq_ignore_ascii_case(left: Component<'_>, right: Component<'_>) -> bool {
    left.as_os_str()
        .to_string_lossy()
        .eq_ignore_ascii_case(&right.as_os_str().to_string_lossy())
}

#[cfg(not(windows))]
fn path_starts_with(path: &Path, root: &Path) -> bool {
    path.starts_with(root)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn resolves_existing_path_beneath_root() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("favorite");
        fs::create_dir_all(root.join("album")).unwrap();
        fs::write(root.join("album").join("page.jpg"), b"x").unwrap();

        let resolved = resolve_existing(&root, "album/page.jpg").unwrap();
        assert_eq!(
            resolved,
            fs::canonicalize(root.join("album").join("page.jpg")).unwrap()
        );
        assert_eq!(
            resolve_existing(&root, "").unwrap(),
            fs::canonicalize(root).unwrap()
        );
    }

    #[test]
    fn rejects_parent_components_and_normalized_escape() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("favorite");
        fs::create_dir_all(root.join("inside")).unwrap();

        assert!(matches!(
            resolve_existing(&root, "../outside"),
            Err(ResolveError::InvalidRelativePath)
        ));
        assert!(matches!(
            resolve_existing(&root, "inside/../../outside"),
            Err(ResolveError::InvalidRelativePath)
        ));
        assert!(matches!(
            resolve_existing(&root, r"inside\..\..\outside"),
            Err(ResolveError::InvalidRelativePath)
        ));
    }

    #[test]
    fn rejects_absolute_rooted_and_drive_paths() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        for path in [
            "/etc/passwd",
            r"\Windows\System32",
            r"C:\Windows",
            r"C:relative",
            r"\\server\share\file",
        ] {
            assert!(
                matches!(
                    resolve_existing(root, path),
                    Err(ResolveError::InvalidRelativePath)
                ),
                "{path:?} should be rejected"
            );
        }
    }

    #[test]
    fn rejects_link_that_escapes_root() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("favorite");
        let outside = temp.path().join("outside");
        fs::create_dir_all(&root).unwrap();
        fs::create_dir_all(&outside).unwrap();
        fs::write(outside.join("secret.jpg"), b"secret").unwrap();
        let link = root.join("escape");

        if let Err(error) = make_dir_symlink(&outside, &link) {
            if error.kind() == std::io::ErrorKind::PermissionDenied {
                eprintln!("symlink creation is unavailable; link-escape assertion skipped");
                return;
            }
            panic!("failed to create test symlink: {error}");
        }

        assert!(matches!(
            resolve_existing(&root, "escape/secret.jpg"),
            Err(ResolveError::EscapesFavorite)
        ));
    }

    #[cfg(windows)]
    fn make_dir_symlink(target: &Path, link: &Path) -> std::io::Result<()> {
        match std::os::windows::fs::symlink_dir(target, link) {
            Ok(()) => Ok(()),
            Err(error) if error.raw_os_error() == Some(1314) => {
                // Creating a directory symlink needs SeCreateSymbolicLinkPrivilege on
                // Windows without Developer Mode. A junction exercises the same
                // canonicalization escape boundary and needs no special privilege.
                let status = std::process::Command::new("cmd")
                    .args(["/d", "/c", "mklink", "/J"])
                    .arg(link)
                    .arg(target)
                    .status()?;
                if status.success() { Ok(()) } else { Err(error) }
            }
            Err(error) => Err(error),
        }
    }

    #[cfg(unix)]
    fn make_dir_symlink(target: &Path, link: &Path) -> std::io::Result<()> {
        std::os::unix::fs::symlink(target, link)
    }
}
