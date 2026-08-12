use std::path::{Path, PathBuf};

use mimageviewer_ipc::{AddressError, validate_absolute_path};

#[derive(Debug, PartialEq, Eq)]
pub enum ResolveError {
    InvalidPath,
    NetworkPath,
    Unavailable,
}

#[derive(Debug, PartialEq, Eq)]
pub struct ResolvedPath {
    pub canonical: PathBuf,
    pub logical: PathBuf,
}

/// 絶対パスを検証し、実在する対象を canonical path へ正規化する。
pub fn resolve_existing(path: &str) -> Result<ResolvedPath, ResolveError> {
    resolve_existing_with(path, |candidate| std::fs::canonicalize(candidate))
}

fn resolve_existing_with(
    path: &str,
    canonicalize: impl FnOnce(&Path) -> std::io::Result<PathBuf>,
) -> Result<ResolvedPath, ResolveError> {
    // 絶対パス + NUL 拒否の正本は mimageviewer-ipc。ここに述語を複製しない。
    validate_absolute_path(path).map_err(resolve_syntax_error)?;
    let caller = Path::new(path);
    let canonical = canonicalize(caller).map_err(|_| ResolveError::Unavailable)?;
    let logical = logical_path_for_caller(caller, &canonical);
    Ok(ResolvedPath { canonical, logical })
}

fn resolve_syntax_error(error: AddressError) -> ResolveError {
    match error {
        AddressError::NetworkPath => ResolveError::NetworkPath,
        AddressError::InvalidPath | AddressError::InvalidZipPath => ResolveError::InvalidPath,
    }
}

fn logical_path_for_caller(caller: &Path, canonical: &Path) -> PathBuf {
    if canonical.to_string_lossy().starts_with(r"\\?\UNC\")
        && caller_is_drive_absolute(caller)
        && let Ok(absolute) = std::path::absolute(caller)
    {
        return logical_path_from_canonical(&absolute);
    }
    logical_path_from_canonical(canonical)
}

fn caller_is_drive_absolute(path: &Path) -> bool {
    let value = path.to_string_lossy();
    let bytes = value.as_bytes();
    bytes.len() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && matches!(bytes[2], b'/' | b'\\')
}

fn logical_path_from_canonical(canonical: &Path) -> PathBuf {
    let value = canonical.to_string_lossy();
    if let Some(rest) = value.strip_prefix(r"\\?\UNC\") {
        PathBuf::from(format!(r"\\{rest}"))
    } else if let Some(rest) = value.strip_prefix(r"\\?\") {
        PathBuf::from(rest)
    } else {
        canonical.to_path_buf()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_existing_absolute_paths_outside_favorites() {
        let temp = tempfile::tempdir().unwrap();
        let page = temp.path().join("page.jpg");
        std::fs::write(&page, b"page").unwrap();
        let resolved = resolve_existing(page.to_string_lossy().as_ref()).unwrap();
        assert_eq!(resolved.canonical, std::fs::canonicalize(&page).unwrap());
        assert_eq!(
            resolved.logical,
            logical_path_for_caller(&page, &resolved.canonical)
        );
    }

    #[test]
    fn rejects_relative_nul_and_missing_paths() {
        assert!(matches!(
            resolve_existing("relative.jpg"),
            Err(ResolveError::InvalidPath)
        ));
        assert!(matches!(
            resolve_existing("bad\0path.jpg"),
            Err(ResolveError::InvalidPath)
        ));
        let temp = tempfile::tempdir().unwrap();
        assert!(matches!(
            resolve_existing(temp.path().join("missing").to_string_lossy().as_ref()),
            Err(ResolveError::Unavailable)
        ));
    }

    #[test]
    fn rejects_network_namespace_before_canonicalize() {
        let canonicalize_called = std::cell::Cell::new(false);
        let result =
            resolve_existing_with(r"\\host-that-must-not-be-contacted\share\a.jpg", |_| {
                canonicalize_called.set(true);
                Ok(PathBuf::from(r"C:\unexpected"))
            });
        assert_eq!(result, Err(ResolveError::NetworkPath));
        assert!(!canonicalize_called.get());
    }

    #[cfg(windows)]
    #[test]
    fn drive_caller_stays_drive_shaped_when_canonical_is_unc() {
        assert_eq!(
            logical_path_for_caller(
                Path::new(r"Z:\photo\..\photo"),
                Path::new(r"\\?\UNC\nas\share\photo"),
            ),
            PathBuf::from(r"Z:\photo")
        );
        assert_eq!(
            logical_path_for_caller(Path::new(r"Z:\photo"), Path::new(r"\\?\Z:\photo"),),
            PathBuf::from(r"Z:\photo")
        );
    }
}
