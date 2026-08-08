use std::path::PathBuf;

use mimageviewer_ipc::validate_absolute_path;

#[derive(Debug)]
pub enum ResolveError {
    InvalidPath,
    Unavailable,
}

/// 絶対パスを検証し、実在する対象を canonical path へ正規化する。
pub fn resolve_existing(path: &str) -> Result<PathBuf, ResolveError> {
    // 絶対パス + NUL 拒否の正本は mimageviewer-ipc。ここに述語を複製しない。
    validate_absolute_path(path).map_err(|_| ResolveError::InvalidPath)?;
    std::fs::canonicalize(path).map_err(|_| ResolveError::Unavailable)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_existing_absolute_paths_outside_favorites() {
        let temp = tempfile::tempdir().unwrap();
        let page = temp.path().join("page.jpg");
        std::fs::write(&page, b"page").unwrap();
        assert_eq!(
            resolve_existing(page.to_string_lossy().as_ref()).unwrap(),
            std::fs::canonicalize(page).unwrap()
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
}
