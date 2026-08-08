use std::path::{Component, Path, PathBuf};

use mimageviewer_ipc::{RemoteAddress, RemoteSubresource};
use mimageviewer_registered_roots::ResolveError as RegisteredResolveError;
use uuid::Uuid;

use crate::settings::FavoriteEntry;

use super::live_favorites::AllowedRootsSnapshot;

#[derive(Debug, PartialEq, Eq)]
pub(super) enum ResolveError {
    InvalidRootId,
    RootNotFound,
    InvalidRelativePath,
    Unavailable,
    EscapesRoot,
}

#[derive(Debug, PartialEq, Eq)]
pub(super) struct ResolvedRootPath {
    /// 要求文字列ではなく、allowlist で実際に一致した UUID。
    pub root_id: String,
    /// パンくず先頭に使う、絶対 path を含まない root 表示名。
    pub root_name: String,
    /// logical path を相対化する root。
    pub logical_root: PathBuf,
    /// root 境界。フォルダ代表の再帰先や pin もこの内側に限る。
    pub canonical_root: PathBuf,
    /// 実ファイルを開くための canonical path。
    pub canonical: PathBuf,
    /// mIV の catalog キーを既存の見かけのパスと揃えるための logical path。
    pub logical: PathBuf,
}

#[derive(Debug, PartialEq, Eq)]
pub(super) struct RootRelativePath {
    pub root_id: String,
    pub relative_path: String,
}

/// 既存の絶対 path を、favorite 優先で許可済み root と相対 path に写像する。
/// root の種類と粒度はこの境界の外へ出さない。
pub(super) fn map_existing_to_root(
    allowed: &AllowedRootsSnapshot,
    candidate: &Path,
) -> Option<RootRelativePath> {
    let roots = resolve_existing_roots(allowed);
    map_existing_to_resolved_root(&roots, candidate)
}

struct ResolvedFavoriteRoot<'a> {
    favorite: &'a FavoriteEntry,
    canonical_root: PathBuf,
}

pub(super) struct ResolvedRoots<'a> {
    favorites: Vec<ResolvedFavoriteRoot<'a>>,
    registered: &'a mimageviewer_registered_roots::RegisteredRootsSnapshot,
}

/// 候補列を変換する前に favorite root を 1 回だけ解決する。
/// offline の root はその root だけを除外し、残りの allowlist 判定を継続する。
pub(super) fn resolve_existing_roots(allowed: &AllowedRootsSnapshot) -> ResolvedRoots<'_> {
    let favorites = allowed
        .favorites
        .iter()
        .filter_map(|favorite| {
            std::fs::canonicalize(&favorite.path)
                .ok()
                .map(|canonical_root| ResolvedFavoriteRoot {
                    favorite,
                    canonical_root,
                })
        })
        .collect();
    ResolvedRoots {
        favorites,
        registered: &allowed.registered,
    }
}

pub(super) fn map_existing_to_resolved_root(
    roots: &ResolvedRoots<'_>,
    candidate: &Path,
) -> Option<RootRelativePath> {
    let canonical = std::fs::canonicalize(candidate).ok()?;
    let mut best: Option<(usize, &ResolvedFavoriteRoot<'_>)> = None;
    for root in &roots.favorites {
        if !path_starts_with(&canonical, &root.canonical_root) {
            continue;
        }
        let depth = root.canonical_root.components().count();
        if best
            .as_ref()
            .is_none_or(|(best_depth, _)| depth > *best_depth)
        {
            best = Some((depth, root));
        }
    }
    if let Some((_, root)) = best {
        let relative = components_after_root(&canonical, &root.canonical_root)?;
        let relative_path = remote_relative_path(&relative)?;
        return Some(RootRelativePath {
            root_id: root.favorite.id.to_string(),
            relative_path,
        });
    }
    roots
        .registered
        .map_existing(candidate)
        .map(|mapped| RootRelativePath {
            root_id: mapped.root_id.to_string(),
            relative_path: mapped.relative_path,
        })
}

fn components_after_root(path: &Path, root: &Path) -> Option<PathBuf> {
    let mut path_components = path.components();
    for root_component in root.components() {
        let path_component = path_components.next()?;
        if !component_eq(path_component, root_component) {
            return None;
        }
    }
    Some(path_components.collect())
}

#[cfg(windows)]
fn component_eq(left: Component<'_>, right: Component<'_>) -> bool {
    left.as_os_str()
        .to_string_lossy()
        .eq_ignore_ascii_case(&right.as_os_str().to_string_lossy())
}

#[cfg(not(windows))]
fn component_eq(left: Component<'_>, right: Component<'_>) -> bool {
    left == right
}

pub(super) fn resolve_existing(
    allowed: &AllowedRootsSnapshot,
    root_id: &str,
    relative: &str,
) -> Result<ResolvedRootPath, ResolveError> {
    let root_id = Uuid::parse_str(root_id).map_err(|_| ResolveError::InvalidRootId)?;
    if let Some(favorite) = allowed
        .favorites
        .iter()
        .find(|favorite| favorite.id == root_id)
    {
        validate_relative(relative)?;
        let canonical_root =
            std::fs::canonicalize(&favorite.path).map_err(|_| ResolveError::Unavailable)?;
        let logical = logical_root_path(&favorite.path, relative);
        let canonical = canonicalize_within(&canonical_root, &logical)?;
        return Ok(ResolvedRootPath {
            root_id: favorite.id.to_string(),
            root_name: favorite.name.clone(),
            logical_root: favorite.path.clone(),
            canonical_root,
            canonical,
            logical,
        });
    }
    let resolved = allowed
        .registered
        .resolve_existing(root_id, relative)
        .map_err(map_registered_resolve_error)?;
    Ok(ResolvedRootPath {
        root_id: resolved.root_id.to_string(),
        root_name: resolved.root_name,
        logical_root: resolved.root_path,
        canonical_root: resolved.canonical_root,
        canonical: resolved.canonical,
        logical: resolved.logical,
    })
}

fn map_registered_resolve_error(error: RegisteredResolveError) -> ResolveError {
    match error {
        RegisteredResolveError::RootNotFound => ResolveError::RootNotFound,
        RegisteredResolveError::InvalidRelativePath
        | RegisteredResolveError::FileRootHasRelativePath => ResolveError::InvalidRelativePath,
        RegisteredResolveError::Unavailable => ResolveError::Unavailable,
        RegisteredResolveError::EscapesRoot => ResolveError::EscapesRoot,
    }
}

/// 画素生成経路が使った解決済み logical path と subresource から、公開用 identity を再構成する。
/// HTTP 要求の root/path をそのまま返さないため、この関数は `ResolvedRootPath` だけを入力にする。
pub(super) fn page_identity_from_resolved(
    resolved: &ResolvedRootPath,
    subresource: &RemoteSubresource,
) -> Option<RemoteAddress> {
    let relative = components_after_root(&resolved.logical, &resolved.logical_root)?;
    let mut segments = Vec::new();
    for component in relative.components() {
        match component {
            Component::Normal(value) => segments.push(value.to_string_lossy().into_owned()),
            Component::CurDir => {}
            Component::Prefix(_) | Component::RootDir | Component::ParentDir => return None,
        }
    }
    Some(RemoteAddress {
        root_id: resolved.root_id.clone(),
        relative_path: segments.join("/"),
        subresource: subresource.clone(),
    })
}

/// favorite root と検証済み相対 path の論理キーを組み立てる。
/// ファイルシステムへ触れないため UI thread の永続キー導出でも共有できる。
pub(super) fn logical_root_path(root: &Path, relative: &str) -> PathBuf {
    if relative.is_empty() {
        root.to_path_buf()
    } else {
        root.join(relative)
    }
}

pub(super) fn logical_favorite_path(favorite_root: &Path, relative: &str) -> PathBuf {
    logical_root_path(favorite_root, relative)
}

pub(super) fn canonicalize_within(
    canonical_root: &Path,
    candidate: &Path,
) -> Result<PathBuf, ResolveError> {
    let canonical = std::fs::canonicalize(candidate).map_err(|_| ResolveError::Unavailable)?;
    if !path_starts_with(&canonical, canonical_root) {
        return Err(ResolveError::EscapesRoot);
    }
    Ok(canonical)
}

fn remote_relative_path(path: &Path) -> Option<String> {
    let mut segments = Vec::new();
    for component in path.components() {
        match component {
            Component::Normal(value) => segments.push(value.to_string_lossy().into_owned()),
            Component::CurDir => {}
            Component::Prefix(_) | Component::RootDir | Component::ParentDir => return None,
        }
    }
    Some(segments.join("/"))
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
    // 非 Windows の unit test でも Windows 区切りの traversal を拒否する。
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
        if !path_component
            .as_os_str()
            .to_string_lossy()
            .eq_ignore_ascii_case(&root_component.as_os_str().to_string_lossy())
        {
            return false;
        }
    }
    true
}

#[cfg(not(windows))]
fn path_starts_with(path: &Path, root: &Path) -> bool {
    path.starts_with(root)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn favorite(root: &Path) -> FavoriteEntry {
        FavoriteEntry::new("test".to_owned(), root.to_path_buf())
    }

    fn allowed(favorites: Vec<FavoriteEntry>, registered: Vec<PathBuf>) -> AllowedRootsSnapshot {
        AllowedRootsSnapshot {
            favorites: std::sync::Arc::new(favorites),
            registered: mimageviewer_registered_roots::RegisteredRootsSnapshot::from_paths(
                registered,
            ),
        }
    }

    fn resolved_for_identity(relative: &str) -> ResolvedRootPath {
        let logical_root = PathBuf::from("favorite");
        let logical = logical_root_path(&logical_root, relative);
        ResolvedRootPath {
            root_id: "30d6c167-7148-4f3e-9a5a-21c5fd31ecb2".to_owned(),
            root_name: "favorite".to_owned(),
            logical_root: logical_root.clone(),
            canonical_root: logical_root.clone(),
            canonical: logical.clone(),
            logical,
        }
    }

    #[test]
    fn page_identity_distinguishes_documents_entries_and_files() {
        let pdf_page = RemoteSubresource::PdfPage { page_number: 1 };
        let first_pdf =
            page_identity_from_resolved(&resolved_for_identity("books/first.pdf"), &pdf_page)
                .unwrap();
        let same_pdf =
            page_identity_from_resolved(&resolved_for_identity("books/first.pdf"), &pdf_page)
                .unwrap();
        let other_pdf =
            page_identity_from_resolved(&resolved_for_identity("books/other.pdf"), &pdf_page)
                .unwrap();
        assert_eq!(first_pdf, same_pdf);
        assert_ne!(first_pdf, other_pdf);

        let archive = resolved_for_identity("books/first.zip");
        let first_entry = page_identity_from_resolved(
            &archive,
            &RemoteSubresource::ZipEntry {
                entry_name: "chapter/001.jpg".to_owned(),
            },
        )
        .unwrap();
        let other_entry = page_identity_from_resolved(
            &archive,
            &RemoteSubresource::ZipEntry {
                entry_name: "chapter/002.jpg".to_owned(),
            },
        )
        .unwrap();
        assert_ne!(first_entry, other_entry);

        let first_file = page_identity_from_resolved(
            &resolved_for_identity("pages/001.jpg"),
            &RemoteSubresource::File,
        )
        .unwrap();
        let other_file = page_identity_from_resolved(
            &resolved_for_identity("pages/002.jpg"),
            &RemoteSubresource::File,
        )
        .unwrap();
        assert_ne!(first_file, other_file);
        assert_eq!(first_file.relative_path, "pages/001.jpg");
    }

    #[test]
    fn resolves_only_registered_favorite_relative_paths() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("favorite");
        std::fs::create_dir_all(root.join("album")).unwrap();
        std::fs::write(root.join("album/page.jpg"), b"x").unwrap();
        let favorite = favorite(&root);
        let roots = allowed(vec![favorite.clone()], Vec::new());

        let resolved =
            resolve_existing(&roots, &favorite.id.to_string(), "album/page.jpg").unwrap();
        assert_eq!(
            resolved.canonical,
            std::fs::canonicalize(root.join("album/page.jpg")).unwrap()
        );
        assert_eq!(resolved.logical, root.join("album/page.jpg"));
    }

    #[test]
    fn rejects_unknown_favorite_and_root_escape_syntax() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("favorite");
        std::fs::create_dir_all(&root).unwrap();
        let favorite = favorite(&root);
        let roots = allowed(vec![favorite.clone()], Vec::new());

        assert_eq!(
            resolve_existing(&roots, &Uuid::new_v4().to_string(), ""),
            Err(ResolveError::RootNotFound)
        );
        for relative in [
            "../outside.jpg",
            r"album\..\..\outside.jpg",
            r"C:\Windows\secret.jpg",
            r"C:relative.jpg",
            r"\\server\share\secret.jpg",
            "/etc/passwd",
        ] {
            assert_eq!(
                resolve_existing(&roots, &favorite.id.to_string(), relative),
                Err(ResolveError::InvalidRelativePath),
                "{relative:?}"
            );
        }
    }

    #[test]
    fn rejects_link_that_resolves_outside_the_favorite() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("favorite");
        let outside = temp.path().join("outside");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(outside.join("secret.jpg"), b"secret").unwrap();
        let link = root.join("escape");
        if make_dir_link(&outside, &link).is_err() {
            eprintln!("directory links are unavailable; escape assertion skipped");
            return;
        }
        let favorite = favorite(&root);
        let roots = allowed(vec![favorite.clone()], Vec::new());
        assert_eq!(
            resolve_existing(&roots, &favorite.id.to_string(), "escape/secret.jpg"),
            Err(ResolveError::EscapesRoot)
        );
    }

    #[test]
    fn maps_only_existing_favorite_members_to_relative_paths() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("favorite");
        let nested = root.join("album");
        let outside = temp.path().join("outside.jpg");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::write(nested.join("page.jpg"), b"x").unwrap();
        std::fs::write(&outside, b"x").unwrap();
        let favorite = favorite(&root);
        let roots = allowed(vec![favorite.clone()], Vec::new());

        let mapped = map_existing_to_root(&roots, &nested.join("page.jpg")).unwrap();
        assert_eq!(mapped.root_id, favorite.id.to_string());
        assert_eq!(mapped.relative_path, "album/page.jpg");
        assert!(!Path::new(&mapped.relative_path).is_absolute());
        assert!(map_existing_to_root(&roots, &outside).is_none());
    }

    #[test]
    fn missing_favorite_root_does_not_hide_an_existing_favorite_member() {
        let temp = tempfile::tempdir().unwrap();
        let missing_root = temp.path().join("missing");
        let existing_root = temp.path().join("existing");
        let page = existing_root.join("album/page.jpg");
        std::fs::create_dir_all(page.parent().unwrap()).unwrap();
        std::fs::write(&page, b"page").unwrap();
        let missing = favorite(&missing_root);
        let existing = favorite(&existing_root);
        let roots = allowed(vec![missing, existing.clone()], Vec::new());

        let mapped = map_existing_to_root(&roots, &page).unwrap();

        assert_eq!(mapped.root_id, existing.id.to_string());
        assert_eq!(mapped.relative_path, "album/page.jpg");
    }

    #[test]
    fn registered_file_is_self_only_and_favorite_mapping_still_wins() {
        let temp = tempfile::tempdir().unwrap();
        let favorite_root = temp.path().join("favorite");
        let registered_file = favorite_root.join("book.zip");
        std::fs::create_dir(&favorite_root).unwrap();
        std::fs::write(&registered_file, b"zip").unwrap();
        let favorite = favorite(&favorite_root);
        let roots = allowed(vec![favorite.clone()], vec![registered_file.clone()]);

        let mapped = map_existing_to_root(&roots, &registered_file).unwrap();
        assert_eq!(mapped.root_id, favorite.id.to_string());
        assert_eq!(mapped.relative_path, "book.zip");

        let registered_only = allowed(Vec::new(), vec![registered_file.clone()]);
        let mapped = map_existing_to_root(&registered_only, &registered_file).unwrap();
        assert_eq!(
            mapped.root_id,
            mimageviewer_registered_roots::registered_root_id(&registered_file).to_string()
        );
        assert!(mapped.relative_path.is_empty());
        assert!(resolve_existing(&registered_only, &mapped.root_id, "").is_ok());
        assert_eq!(
            resolve_existing(&registered_only, &mapped.root_id, "sibling.jpg"),
            Err(ResolveError::InvalidRelativePath)
        );
    }

    #[test]
    fn registered_file_subresources_keep_the_empty_root_relative_path() {
        let temp = tempfile::tempdir().unwrap();
        let zip = temp.path().join("book.zip");
        let pdf = temp.path().join("book.pdf");
        std::fs::write(&zip, b"zip").unwrap();
        std::fs::write(&pdf, b"pdf").unwrap();
        let roots = allowed(Vec::new(), vec![zip.clone(), pdf.clone()]);
        let addresses = [
            RemoteAddress {
                root_id: mimageviewer_registered_roots::registered_root_id(&zip).to_string(),
                relative_path: String::new(),
                subresource: RemoteSubresource::ZipEntry {
                    entry_name: "chapter/001.jpg".to_owned(),
                },
            },
            RemoteAddress {
                root_id: mimageviewer_registered_roots::registered_root_id(&pdf).to_string(),
                relative_path: String::new(),
                subresource: RemoteSubresource::PdfPage { page_number: 0 },
            },
        ];

        for address in addresses {
            address.validate_syntax().unwrap();
            let resolved = resolve_existing(&roots, &address.root_id, &address.relative_path)
                .expect("registered container must resolve");
            assert_eq!(
                page_identity_from_resolved(&resolved, &address.subresource),
                Some(address)
            );
        }
    }

    #[cfg(windows)]
    fn make_dir_link(target: &Path, link: &Path) -> std::io::Result<()> {
        match std::os::windows::fs::symlink_dir(target, link) {
            Ok(()) => Ok(()),
            Err(error) if error.raw_os_error() == Some(1314) => {
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
    fn make_dir_link(target: &Path, link: &Path) -> std::io::Result<()> {
        std::os::unix::fs::symlink(target, link)
    }
}
