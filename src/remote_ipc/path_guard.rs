use std::path::{Path, PathBuf};

use mimageviewer_ipc::{RemoteAddress, RemoteSubresource, validate_absolute_path};

#[derive(Debug, PartialEq, Eq)]
pub(super) enum ResolveError {
    InvalidPath,
    Unavailable,
}

#[derive(Debug, PartialEq, Eq)]
pub(super) struct ResolvedPath {
    /// 実ファイルを開くための canonical path。
    pub canonical: PathBuf,
    /// mIV の path key と公開住所に使う canonical path。
    /// Windows では filesystem API が付ける extended prefix を通常表記へ戻す。
    pub logical: PathBuf,
}

/// 絶対パスを検証し、実在する対象を canonical path へ正規化する。
pub(super) fn resolve_existing(path: &str) -> Result<ResolvedPath, ResolveError> {
    // 絶対パス + NUL 拒否の正本は mimageviewer-ipc。ここに述語を複製しない。
    validate_absolute_path(path).map_err(|_| ResolveError::InvalidPath)?;
    let canonical = std::fs::canonicalize(path).map_err(|_| ResolveError::Unavailable)?;
    let logical = logical_path_from_canonical(&canonical);
    Ok(ResolvedPath { logical, canonical })
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

/// 画素生成経路が使った canonical path と subresource から公開用 identity を再構成する。
pub(super) fn page_identity_from_resolved(
    resolved: &ResolvedPath,
    subresource: &RemoteSubresource,
) -> RemoteAddress {
    RemoteAddress {
        path: resolved.logical.to_string_lossy().into_owned(),
        subresource: subresource.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_any_existing_absolute_path_and_canonicalizes_it() {
        let temp = tempfile::tempdir().unwrap();
        let page = temp.path().join("outside.jpg");
        std::fs::write(&page, b"page").unwrap();

        let resolved = resolve_existing(page.to_string_lossy().as_ref()).unwrap();
        assert_eq!(resolved.canonical, std::fs::canonicalize(&page).unwrap());
        assert_eq!(
            resolved.logical,
            logical_path_from_canonical(&resolved.canonical)
        );
        assert!(!resolved.logical.to_string_lossy().starts_with(r"\\?\"));
    }

    #[test]
    fn rejects_missing_nul_and_relative_paths() {
        let temp = tempfile::tempdir().unwrap();
        let missing = temp.path().join("missing.jpg");
        assert_eq!(
            resolve_existing(missing.to_string_lossy().as_ref()),
            Err(ResolveError::Unavailable)
        );
        assert_eq!(
            resolve_existing("relative.jpg"),
            Err(ResolveError::InvalidPath)
        );
        assert_eq!(
            resolve_existing("invalid\0path.jpg"),
            Err(ResolveError::InvalidPath)
        );
    }

    #[test]
    fn page_identity_uses_the_canonical_absolute_path() {
        let temp = tempfile::tempdir().unwrap();
        let pdf = temp.path().join("book.pdf");
        std::fs::write(&pdf, b"pdf").unwrap();
        let resolved = resolve_existing(pdf.to_string_lossy().as_ref()).unwrap();
        let address =
            page_identity_from_resolved(&resolved, &RemoteSubresource::PdfPage { page_number: 2 });
        assert!(Path::new(&address.path).is_absolute());
        assert_eq!(address.path, resolved.logical.to_string_lossy());
    }
}
