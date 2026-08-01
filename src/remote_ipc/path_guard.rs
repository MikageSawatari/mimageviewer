use std::path::{Component, Path, PathBuf};

use uuid::Uuid;

use crate::settings::FavoriteEntry;

#[derive(Debug, PartialEq, Eq)]
pub(super) enum ResolveError {
    InvalidFavoriteId,
    FavoriteNotFound,
    InvalidRelativePath,
    Unavailable,
    EscapesFavorite,
}

#[derive(Debug, PartialEq, Eq)]
pub(super) struct ResolvedFavoritePath {
    /// お気に入り境界。フォルダ代表の再帰先や pin もこの内側に限る。
    pub canonical_root: PathBuf,
    /// 実ファイルを開くための canonical path。
    pub canonical: PathBuf,
    /// mIV の catalog キーを既存の見かけのパスと揃えるための logical path。
    pub logical: PathBuf,
}

#[derive(Debug, PartialEq, Eq)]
pub(super) struct FavoriteRelativePath {
    pub favorite_id: String,
    pub relative_path: String,
}

/// 既存の絶対 path を、最も深く一致するお気に入り root と相対 path に写像する。
/// canonical path 同士で比較するため junction / symlink で root 外へ出た項目は返さない。
pub(super) fn map_existing_to_favorite(
    favorites: &[FavoriteEntry],
    candidate: &Path,
) -> Option<FavoriteRelativePath> {
    let canonical = std::fs::canonicalize(candidate).ok()?;
    let mut best: Option<(usize, &FavoriteEntry, PathBuf)> = None;
    for favorite in favorites {
        let root = std::fs::canonicalize(&favorite.path).ok()?;
        if !path_starts_with(&canonical, &root) {
            continue;
        }
        let depth = root.components().count();
        if best
            .as_ref()
            .is_none_or(|(best_depth, _, _)| depth > *best_depth)
        {
            best = Some((depth, favorite, root));
        }
    }
    let (_, favorite, root) = best?;
    let relative = components_after_root(&canonical, &root)?;
    let relative_path = relative
        .components()
        .filter_map(|component| match component {
            Component::Normal(value) => Some(value.to_string_lossy().into_owned()),
            Component::CurDir => None,
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("/");
    Some(FavoriteRelativePath {
        favorite_id: favorite.id.to_string(),
        relative_path,
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
    favorites: &[FavoriteEntry],
    favorite_id: &str,
    relative: &str,
) -> Result<ResolvedFavoritePath, ResolveError> {
    let favorite_id = Uuid::parse_str(favorite_id).map_err(|_| ResolveError::InvalidFavoriteId)?;
    let favorite = favorites
        .iter()
        .find(|favorite| favorite.id == favorite_id)
        .ok_or(ResolveError::FavoriteNotFound)?;
    validate_relative(relative)?;

    let canonical_root =
        std::fs::canonicalize(&favorite.path).map_err(|_| ResolveError::Unavailable)?;
    let logical = logical_favorite_path(&favorite.path, relative);
    let canonical = canonicalize_within(&canonical_root, &logical)?;
    Ok(ResolvedFavoritePath {
        canonical_root,
        canonical,
        logical,
    })
}

/// favorite root と検証済み相対 path の論理キーを組み立てる。
/// ファイルシステムへ触れないため UI thread の永続キー導出でも共有できる。
pub(super) fn logical_favorite_path(favorite_root: &Path, relative: &str) -> PathBuf {
    if relative.is_empty() {
        favorite_root.to_path_buf()
    } else {
        favorite_root.join(relative)
    }
}

pub(super) fn canonicalize_within(
    canonical_root: &Path,
    candidate: &Path,
) -> Result<PathBuf, ResolveError> {
    let canonical = std::fs::canonicalize(candidate).map_err(|_| ResolveError::Unavailable)?;
    if !path_starts_with(&canonical, canonical_root) {
        return Err(ResolveError::EscapesFavorite);
    }
    Ok(canonical)
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

    #[test]
    fn resolves_only_registered_favorite_relative_paths() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("favorite");
        std::fs::create_dir_all(root.join("album")).unwrap();
        std::fs::write(root.join("album/page.jpg"), b"x").unwrap();
        let favorite = favorite(&root);

        let resolved = resolve_existing(
            std::slice::from_ref(&favorite),
            &favorite.id.to_string(),
            "album/page.jpg",
        )
        .unwrap();
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

        assert_eq!(
            resolve_existing(&[favorite.clone()], &Uuid::new_v4().to_string(), ""),
            Err(ResolveError::FavoriteNotFound)
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
                resolve_existing(&[favorite.clone()], &favorite.id.to_string(), relative),
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
        assert_eq!(
            resolve_existing(
                &[favorite.clone()],
                &favorite.id.to_string(),
                "escape/secret.jpg"
            ),
            Err(ResolveError::EscapesFavorite)
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

        let mapped =
            map_existing_to_favorite(std::slice::from_ref(&favorite), &nested.join("page.jpg"))
                .unwrap();
        assert_eq!(mapped.favorite_id, favorite.id.to_string());
        assert_eq!(mapped.relative_path, "album/page.jpg");
        assert!(!Path::new(&mapped.relative_path).is_absolute());
        assert!(map_existing_to_favorite(&[favorite], &outside).is_none());
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
