use std::path::{Path, PathBuf};

use mimageviewer_ipc::{AddressError, RemoteAddress, RemoteSubresource, validate_absolute_path};

#[derive(Debug, PartialEq, Eq)]
pub(super) enum ResolveError {
    InvalidPath,
    NetworkPath,
    Unavailable,
}

#[derive(Debug, PartialEq, Eq)]
pub(super) struct ResolvedPath {
    /// 実ファイルを開くための canonical path。
    pub canonical: PathBuf,
    /// mIV の path key と公開住所に使う canonical path。
    /// Windows では filesystem API が付ける extended prefix を通常表記へ戻す。
    pub logical: PathBuf,
    /// 変換済みアーカイブや直接読み RAR の、Core 内だけで使う読み込み実体。
    /// identity / DB key / 公開住所には `logical` と `canonical` を使い続ける。
    archive_backing: Option<ResolvedArchiveBacking>,
}

#[derive(Debug, PartialEq, Eq)]
struct ResolvedArchiveBacking {
    canonical: PathBuf,
    logical: PathBuf,
}

impl ResolvedPath {
    pub(super) fn with_archive_backing(
        source_path: &Path,
        public_path: &str,
        backing_path: &Path,
    ) -> Result<Self, ResolveError> {
        validate_absolute_path(public_path).map_err(resolve_syntax_error)?;
        let canonical =
            std::fs::canonicalize(source_path).map_err(|_| ResolveError::Unavailable)?;
        let backing_canonical =
            std::fs::canonicalize(backing_path).map_err(|_| ResolveError::Unavailable)?;
        Ok(Self {
            canonical,
            logical: PathBuf::from(public_path),
            archive_backing: Some(ResolvedArchiveBacking {
                logical: logical_path_from_canonical(&backing_canonical),
                canonical: backing_canonical,
            }),
        })
    }

    pub(super) fn has_archive_backing(&self) -> bool {
        self.archive_backing.is_some()
    }

    pub(super) fn readable_canonical(&self) -> &Path {
        self.archive_backing
            .as_ref()
            .map_or(self.canonical.as_path(), |backing| {
                backing.canonical.as_path()
            })
    }

    pub(super) fn readable_logical(&self) -> &Path {
        self.archive_backing
            .as_ref()
            .map_or(self.logical.as_path(), |backing| backing.logical.as_path())
    }
}

/// 絶対パスを検証し、実在する対象を canonical path へ正規化する。
pub(super) fn resolve_existing(path: &str) -> Result<ResolvedPath, ResolveError> {
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
    Ok(ResolvedPath {
        logical,
        canonical,
        archive_backing: None,
    })
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

    #[test]
    fn archive_backing_is_used_only_for_reads() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("book.7z");
        let backing = temp.path().join("cache.zip");
        std::fs::write(&source, b"source").unwrap();
        std::fs::write(&backing, b"backing").unwrap();
        let public = source.to_string_lossy().into_owned();

        let resolved = ResolvedPath::with_archive_backing(&source, &public, &backing).unwrap();

        assert_eq!(resolved.logical, PathBuf::from(&public));
        assert_eq!(resolved.canonical, std::fs::canonicalize(&source).unwrap());
        assert_eq!(
            resolved.readable_canonical(),
            std::fs::canonicalize(&backing).unwrap().as_path()
        );
        assert_ne!(resolved.readable_canonical(), resolved.canonical.as_path());
        assert!(resolved.has_archive_backing());
        assert_eq!(
            page_identity_from_resolved(
                &resolved,
                &RemoteSubresource::ZipEntry {
                    entry_name: "page.jpg".to_owned(),
                }
            )
            .path,
            public
        );
    }
}
