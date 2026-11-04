//! パスをキーとして DB に保存するときの正規化ルール。
//!
//! ドライブ文字 (例: `C:`) を除外、小文字化、バックスラッシュ→スラッシュ統一。
//! USB / 外付け HDD のドライブレター変化で保存情報が失われないようにするため。
//!
//! ドライブ文字を保持したい場合 (お気に入り検索のスコープ判定など) は
//! この関数を使わず、呼び出し側で個別に正規化する。

use std::path::{Path, PathBuf};

/// ドライブルート (`C:\` など) または共有ルートとして扱うパスか。
///
/// ルート catalog とドライブ一覧 seed はこの判定を共有し、ドライブ直下の同名項目が
/// 別ドライブのキャッシュを拾わないようにする。
pub fn is_drive_or_share_root(path: &Path) -> bool {
    path.parent().is_none()
}

/// ドライブ文字を除いて小文字化・スラッシュ統一したパス文字列を返す。
pub fn normalize(path: &Path) -> String {
    let s = path.to_string_lossy();
    let no_drive = if s.len() >= 2 && s.chars().nth(1) == Some(':') {
        &s[2..]
    } else {
        &s
    };
    no_drive.to_lowercase().replace('\\', "/")
}

/// ドライブ文字を **保持** したまま小文字化・スラッシュ統一したパス文字列を返す。
/// rotation_db / rating_db / video_pins / video_bookmarks / video_tile_thumb_cache 等の
/// DB キー用に使う共通実装。`normalize` (= ドライブ文字除外、お気に入り検索用) とは
/// 別の規則であることに注意。
pub fn normalize_keep_drive(path: &Path) -> String {
    path.to_string_lossy().to_lowercase().replace('\\', "/")
}

/// DB キーと実ファイル列挙結果のように、表記が異なり得る 2 パスを
/// `normalize_keep_drive` と同じ規則で比較する。
pub fn eq_keep_drive(a: &Path, b: &Path) -> bool {
    normalize_keep_drive(a) == normalize_keep_drive(b)
}

/// Standardize an accepted physical path without I/O or changing its identity.
/// This is an input/display spelling, not a persistent key. Keep component casing,
/// relative/dot components and namespace paths intact; do not resolve symlinks.
pub(crate) fn normalize_input_path(path: &Path) -> PathBuf {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::{OsStrExt, OsStringExt};
        let mut units: Vec<u16> = path.as_os_str().encode_wide().collect();
        // Verbatim/device namespaces do not use ordinary Win32 separator rules.
        if units.starts_with(&[92, 92, 63, 92]) || units.starts_with(&[92, 92, 46, 92]) {
            return path.to_path_buf();
        }
        for unit in &mut units {
            if *unit == 47 {
                *unit = 92;
            }
        }
        if units.get(1) == Some(&58) && matches!(units[0], 97..=122) {
            units[0] -= 32;
        }
        PathBuf::from(std::ffi::OsString::from_wide(&units))
    }
    #[cfg(not(windows))]
    path.to_path_buf()
}

/// Compare physical path spellings without I/O or changing persistent DB keys.
/// Only verbatim drive and UNC paths alias ordinary paths; device/volume namespaces
/// do not. Resolve lexical dot components without allowing traversal above a root.
pub(crate) fn physical_path_spelling(path: &Path) -> Option<String> {
    let mut spelling = normalize_keep_drive(path);
    let drive_absolute = |value: &str| {
        let bytes = value.as_bytes();
        bytes.len() >= 3 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' && bytes[2] == b'/'
    };
    if let Some(rest) = spelling.strip_prefix("//?/") {
        spelling = if let Some(unc) = rest.strip_prefix("unc/") {
            format!("//{unc}")
        } else if drive_absolute(rest) {
            rest.to_owned()
        } else {
            return None;
        };
    }
    let (root, remainder) = if drive_absolute(&spelling) {
        (spelling[..3].to_owned(), &spelling[3..])
    } else if let Some(unc) = spelling.strip_prefix("//") {
        let (server, rest) = unc.trim_start_matches('/').split_once('/')?;
        let rest = rest.trim_start_matches('/');
        let (share, remainder) = rest.split_once('/').unwrap_or((rest, ""));
        if share.is_empty() {
            return None;
        }
        if [server, share]
            .iter()
            .any(|part| matches!(*part, "." | ".." | "?"))
        {
            return None;
        }
        (format!("//{server}/{share}"), remainder)
    } else if let Some(remainder) = spelling.strip_prefix('/') {
        ("/".to_owned(), remainder)
    } else {
        if spelling.contains(':') {
            return None;
        }
        (String::new(), spelling.as_str())
    };
    let mut components = Vec::new();
    for component in remainder.split('/') {
        match component {
            "" | "." => {}
            ".." => {
                components.pop()?;
            }
            _ => components.push(component),
        }
    }
    let tail = components.join("/");
    Some(
        if root.is_empty() || root.ends_with('/') || tail.is_empty() {
            format!("{root}{tail}")
        } else {
            format!("{root}/{tail}")
        },
    )
}

/// Component-boundary containment for ordinary/verbatim physical spellings.
pub(crate) fn physical_path_is_within(path: &Path, root: &Path) -> bool {
    let (Some(path), Some(root)) = (physical_path_spelling(path), physical_path_spelling(root))
    else {
        return false;
    };
    !root.is_empty()
        && (path == root
            || path
                .strip_prefix(&root)
                .is_some_and(|tail| root.ends_with('/') || tail.starts_with('/')))
}

/// Strict component-boundary descent using the same physical/logical source spellings.
/// Container callers pass the logical source, never an archive conversion cache path.
pub(crate) fn physical_path_is_strict_descendant(path: &Path, root: &Path) -> bool {
    physical_path_is_within(path, root)
        && physical_path_spelling(path) != physical_path_spelling(root)
}
/// Worker-operation-local display spelling resolver. Never share across requests:
/// a new operation must observe case-only renames rather than reuse old spellings.
/// Successful exact prefixes are cached (including leaves), at most 4096 entries.
/// Storage is proportional to the total length of those input/output prefixes.
/// Each uncached Windows component uses one exact name probe, no directory scan;
/// shared parents and repeated paths need no further probes within the operation.
#[derive(Default)]
pub(crate) struct ExistingPathCasingCache {
    #[cfg(windows)]
    prefixes: std::collections::HashMap<PathBuf, PathBuf>,
    #[cfg(all(windows, test))]
    probes: usize,
}

impl ExistingPathCasingCache {
    #[cfg(windows)]
    const MAX_PREFIXES: usize = 4096;

    /// Filesystem I/O: call only from a worker. Preserve drive/share roots and
    /// logical directory symlink/junction aliases; change only component casing.
    #[cfg(windows)]
    pub(crate) fn restore(&mut self, path: &Path) -> Option<PathBuf> {
        use std::path::{Component, Prefix};

        let input = normalize_input_path(path);
        let mut components = input.components();
        let Component::Prefix(prefix) = components.next()? else {
            return None;
        };
        if !matches!(
            prefix.kind(),
            Prefix::Disk(_)
                | Prefix::UNC(_, _)
                | Prefix::VerbatimDisk(_)
                | Prefix::VerbatimUNC(_, _)
        ) || components.next() != Some(Component::RootDir)
        {
            return None;
        }
        if self.prefixes.len() >= Self::MAX_PREFIXES {
            self.prefixes.clear();
        }
        let mut key = PathBuf::from(prefix.as_os_str());
        key.push(std::path::MAIN_SEPARATOR_STR);
        let mut spelled = key.clone();
        for component in components {
            let Component::Normal(name) = component else {
                return None;
            };
            // Exact paths, not lowercased keys: case-sensitive directories may
            // contain distinct entries whose names differ only by case.
            key.push(name);
            if let Some(cached) = self.prefixes.get(&key) {
                spelled.clone_from(cached);
                continue;
            }
            let query = spelled.join(name);
            #[cfg(test)]
            {
                self.probes += 1;
            }
            let actual = existing_component_name(&query)?;
            if !eq_keep_drive(Path::new(&actual), Path::new(name)) {
                // In particular, do not replace an 8.3 alias with a long name.
                return None;
            }
            spelled.push(actual);
            if self.prefixes.len() < Self::MAX_PREFIXES {
                self.prefixes.insert(key.clone(), spelled.clone());
            }
        }
        eq_keep_drive(&spelled, &input).then_some(spelled)
    }

    #[cfg(not(windows))]
    pub(crate) fn restore(&mut self, path: &Path) -> Option<PathBuf> {
        let canonical = std::fs::canonicalize(path).ok()?;
        eq_keep_drive(&canonical, path).then_some(canonical)
    }
}

#[cfg(windows)]
fn existing_component_name(path: &Path) -> Option<std::ffi::OsString> {
    use std::os::windows::ffi::{OsStrExt, OsStringExt};
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::Storage::FileSystem::{FindClose, FindFirstFileW, WIN32_FIND_DATAW};
    use windows::core::PCWSTR;

    struct FindHandle(HANDLE);
    impl Drop for FindHandle {
        fn drop(&mut self) {
            let _ = unsafe { FindClose(self.0) };
        }
    }

    // FindFirstFile interprets wildcards. Only the namespace prefix may contain
    // '?'; reject it (and NUL) in every ordinary component before calling Win32.
    if path.components().any(|component| {
        matches!(component, std::path::Component::Normal(name) if name.encode_wide().any(|unit| matches!(unit, 0 | 42 | 63)))
    }) {
        return None;
    }
    let mut wide = path.as_os_str().encode_wide().collect::<Vec<_>>();
    // Keep long paths supported without changing the displayed namespace.
    if !wide.starts_with(&[92, 92, 63, 92]) {
        wide = if wide.starts_with(&[92, 92]) {
            [vec![92, 92, 63, 92, 85, 78, 67, 92], wide[2..].to_vec()].concat()
        } else {
            [vec![92, 92, 63, 92], wide].concat()
        };
    }
    wide.push(0);
    let mut data = WIN32_FIND_DATAW::default();
    let _handle = FindHandle(unsafe { FindFirstFileW(PCWSTR(wide.as_ptr()), &mut data) }.ok()?);
    let length = data.cFileName.iter().position(|unit| *unit == 0)?;
    Some(std::ffi::OsString::from_wide(&data.cFileName[..length]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg(windows)]
    fn casing_cache_shares_exact_parent_probes_across_many_siblings() {
        let temp = tempfile::tempdir().unwrap();
        let parent = temp.path().join("MixedShelf").join("MixedChapter");
        std::fs::create_dir_all(&parent).unwrap();
        let mut cache = ExistingPathCasingCache::default();
        let mut first_probes = 0;
        for index in 0..128 {
            let actual = parent.join(format!("MixedPhoto{index:03}.JPG"));
            std::fs::write(&actual, b"image").unwrap();
            let input = PathBuf::from(normalize_keep_drive(&actual));
            assert_eq!(cache.restore(&input), Some(actual.clone()));
            if index == 0 {
                first_probes = cache.probes;
            }
            // Every later sibling needs one exact leaf query. Its ancestors
            // (and then its repeated full path) are entirely cache hits.
            assert_eq!(cache.probes, first_probes + index);
            let probes = cache.probes;
            assert_eq!(cache.restore(&input), Some(actual));
            assert_eq!(cache.probes, probes);
        }
    }

    #[test]
    #[cfg(windows)]
    fn fresh_casing_cache_observes_case_only_directory_and_file_renames() {
        let temp = tempfile::tempdir().unwrap();
        let directory = temp.path().join("MixedShelf");
        std::fs::create_dir(&directory).unwrap();
        let before = directory.join("MixedPhoto.JPG");
        std::fs::write(&before, b"image").unwrap();
        let input = PathBuf::from(normalize_keep_drive(&before));
        assert_eq!(
            ExistingPathCasingCache::default().restore(&input),
            Some(before.clone())
        );
        let after_directory = temp.path().join("MIXEDSHELF");
        std::fs::rename(&directory, &after_directory).unwrap();
        let after = after_directory.join("MIXEDPHOTO.JPG");
        std::fs::rename(after_directory.join("MixedPhoto.JPG"), &after).unwrap();
        assert_eq!(
            ExistingPathCasingCache::default().restore(&input),
            Some(after)
        );
    }

    #[test]
    #[cfg(windows)]
    fn casing_cache_rejects_wildcards_and_nonphysical_namespaces() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(temp.path().join("MixedPhoto.JPG"), b"image").unwrap();
        for pattern in ["*.JPG", "MixedPhot?.JPG"] {
            assert!(
                ExistingPathCasingCache::default()
                    .restore(&temp.path().join(pattern))
                    .is_none()
            );
        }
        for input in [
            r"\\.\C:\MixedPhoto.JPG",
            r"\\?\Volume{abc}\MixedPhoto.JPG",
            r"C:MixedPhoto.JPG",
            "relative.jpg",
        ] {
            assert!(
                ExistingPathCasingCache::default()
                    .restore(Path::new(input))
                    .is_none()
            );
        }
    }

    #[test]
    #[cfg(windows)]
    fn casing_cache_preserves_known_verbatim_prefix_and_roots() {
        let temp = tempfile::tempdir().unwrap();
        let actual = temp.path().join("MixedPhoto.JPG");
        std::fs::write(&actual, b"image").unwrap();
        let input = PathBuf::from(format!(
            r"\\?\{}",
            normalize_keep_drive(&actual).replace('/', r"\")
        ));
        let mut cache = ExistingPathCasingCache::default();
        let restored = cache.restore(&input).unwrap();
        assert!(eq_keep_drive(&restored, &input));
        // Drive spelling is preserved from the namespace prefix; every normal
        // component is spelled exactly as on disk.
        assert_eq!(restored.components().next(), input.components().next());
        assert_eq!(
            restored.components().skip(1).collect::<Vec<_>>(),
            actual.components().skip(1).collect::<Vec<_>>()
        );
        for root in [
            r"C:\",
            r"\\Server\Share\",
            r"\\?\C:\",
            r"\\?\UNC\Server\Share\",
        ] {
            let before = cache.probes;
            assert_eq!(cache.restore(Path::new(root)), Some(PathBuf::from(root)));
            assert_eq!(cache.probes, before);
        }
    }

    #[test]
    fn strict_descendant_uses_physical_source_spelling_and_component_boundaries() {
        let root = Path::new(r"C:\shelf");
        for child in [
            r"C:\shelf\chapter",
            r"c:/SHELF/book.zip",
            r"\\?\C:\shelf\book.pdf",
        ] {
            assert!(
                physical_path_is_strict_descendant(Path::new(child), root),
                "{child}"
            );
        }
        for other in [
            r"C:\shelf",
            r"c:/SHELF/",
            r"\\?\C:\shelf",
            r"C:\shelf-other\book.zip",
            r"D:\shelf\book.zip",
            r"C:\shelf\..\other",
        ] {
            assert!(
                !physical_path_is_strict_descendant(Path::new(other), root),
                "{other}"
            );
        }
    }

    #[cfg(windows)]
    #[test]
    fn input_spelling_preserves_identity_and_persistent_keys() {
        for (input, expected) in [
            (
                r"g:/home/comfyui/genshin_random\yaoyao",
                r"G:\home\comfyui\genshin_random\yaoyao",
            ),
            (r"c:Photos/../日本語", r"C:Photos\..\日本語"),
            (r"//Server/Share/Mixed\Path", r"\\Server\Share\Mixed\Path"),
            (r"relative/./Mixed\Path", r"relative\.\Mixed\Path"),
            (r"\\?\c:\literal/Path", r"\\?\c:\literal/Path"),
            (r"\\.\c:\literal/Path", r"\\.\c:\literal/Path"),
            ("", ""),
        ] {
            let path = Path::new(input);
            let accepted = normalize_input_path(path);
            assert_eq!(accepted.as_os_str(), std::ffi::OsStr::new(expected));
            assert_eq!(normalize(path), normalize(&accepted));
            assert_eq!(normalize_keep_drive(path), normalize_keep_drive(&accepted));
            assert_eq!(normalize_input_path(&accepted), accepted);
        }
        use std::os::windows::ffi::{OsStrExt, OsStringExt};
        let raw = std::ffi::OsString::from_wide(&[103, 58, 47, 0xd800]);
        let accepted = normalize_input_path(Path::new(&raw));
        assert_eq!(
            accepted.as_os_str().encode_wide().collect::<Vec<_>>(),
            [71, 58, 92, 0xd800]
        );
    }

    #[test]
    fn strips_drive_letter() {
        assert_eq!(normalize(Path::new(r"C:\Foo\Bar")), "/foo/bar");
        assert_eq!(
            normalize(Path::new(r"D:\Photos\IMG.jpg")),
            "/photos/img.jpg"
        );
    }

    #[test]
    fn no_drive_letter_passthrough() {
        assert_eq!(normalize(Path::new("/foo/bar")), "/foo/bar");
        assert_eq!(
            normalize(Path::new(r"\\server\share\file")),
            "//server/share/file"
        );
    }

    #[test]
    fn lowercases_and_unifies_slashes() {
        assert_eq!(
            normalize(Path::new(r"C:\Mixed/Slash\Path")),
            "/mixed/slash/path"
        );
    }

    #[test]
    fn keep_drive_equality_accepts_db_key_and_filesystem_spelling() {
        assert!(eq_keep_drive(
            Path::new("c:/media/bookmark.mp4"),
            Path::new(r"C:\Media\Bookmark.mp4")
        ));
        assert!(!eq_keep_drive(
            Path::new("c:/media/bookmark.mp4"),
            Path::new(r"D:\Media\Bookmark.mp4")
        ));
    }

    #[test]
    fn visibility_physical_path_containment_accepts_normal_and_known_verbatim_spellings() {
        for root in [r"C:\Data\archive_cache", r"\\?\C:\Data\archive_cache"] {
            for path in [
                r"c:\DATA\ARCHIVE_CACHE\hash\book.zip",
                "C:/data/archive_cache/hash/book.zip",
                r"\\?\C:\Data\archive_cache\hash\book.zip",
                r"C:\Data\archive_cache\.\hash\..\book.zip",
                r"C:\Data\archive_cache",
            ] {
                assert!(
                    physical_path_is_within(Path::new(path), Path::new(root)),
                    "{path}"
                );
            }
        }
        for root in [
            r"\\server\share\archive_cache",
            r"\\?\UNC\server\share\archive_cache",
        ] {
            for path in [
                r"\\SERVER\Share\archive_cache\book.zip",
                "//server/share/archive_cache/book.zip",
                r"\\?\unc\server\share\archive_cache\book.zip",
                r"\\server\share\archive_cache\a\..\book.zip",
            ] {
                assert!(
                    physical_path_is_within(Path::new(path), Path::new(root)),
                    "{path}"
                );
            }
        }
        assert!(physical_path_is_within(
            Path::new("C:/book.zip"),
            Path::new("C:/")
        ));
        assert!(physical_path_is_within(
            Path::new("/cache/book.zip"),
            Path::new("/")
        ));
        assert_eq!(
            physical_path_spelling(Path::new("//share-host/share/a")),
            Some("//share-host/share/a".into())
        );
    }

    #[test]
    fn visibility_physical_path_containment_rejects_siblings_traversal_and_other_namespaces() {
        let root = Path::new(r"C:\Data\archive_cache");
        for path in [
            r"C:\Data\archive_cache_old\book.zip",
            r"D:\Data\archive_cache\book.zip",
            r"C:\Data\archive_cache\..\outside.zip",
            r"\\?\C:\Data\archive_cache\a\..\..\outside.zip",
            r"C:\..\Data\archive_cache\book.zip",
            r"\\.\C:\Data\archive_cache\book.zip",
            r"\\?\Volume{abc}\Data\archive_cache\book.zip",
            r"\\?\GLOBALROOT\Device\Data\archive_cache\book.zip",
            r"\\?\C:Data\archive_cache\book.zip",
            r"C:Data\archive_cache\book.zip",
        ] {
            assert!(!physical_path_is_within(Path::new(path), root), "{path}");
        }
        let unc_root = Path::new(r"\\server\share\archive_cache");
        for path in [
            r"\\server\share_other\archive_cache\book.zip",
            r"\\server_other\share\archive_cache\book.zip",
            r"\\server\share\archive_cache_old\book.zip",
            r"\\server\share\archive_cache\..\outside.zip",
            r"\\?\UNC\server\share\..\archive_cache\book.zip",
        ] {
            assert!(
                !physical_path_is_within(Path::new(path), unc_root),
                "{path}"
            );
        }
        assert!(!physical_path_is_within(
            Path::new("/cache/book.zip"),
            Path::new(".")
        ));
        // This helper does not change either established persistent key rule.
        assert_eq!(
            normalize(Path::new(r"\\?\C:\Data\archive_cache")),
            "//?/c:/data/archive_cache"
        );
        assert_eq!(
            normalize_keep_drive(Path::new(r"\\?\UNC\server\share")),
            "//?/unc/server/share"
        );
    }

    #[test]
    fn drive_root_detection_matches_parent_boundary() {
        assert!(is_drive_or_share_root(Path::new(r"C:\")));
        assert!(!is_drive_or_share_root(Path::new(r"C:\Photos")));
    }
}
