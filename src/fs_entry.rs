//! `std::fs::DirEntry` classification helpers for filesystem listing paths.
//!
//! The hot listing paths must keep using `DirEntry::file_type()` instead of
//! `Path::is_dir()` / `is_file()`. On Windows, directory symlinks and junctions
//! are reported as reparse points, so they need a small extra classification
//! step before the UI/search walkers can treat them as folders.

use std::collections::HashSet;
use std::ffi::OsStr;
use std::fs::{DirEntry, FileType};
use std::path::Path;

pub const PORTABLE_METADATA_BUNDLE_DIRNAME: &str = "mimageviewer.meta.miv";

/// mIV自身が作る持ち運び用bundleは、Hidden属性や「隠しファイルを表示」の設定に
/// 依存せず、通常一覧・再帰ビュー・フォルダナビから常に除外する。
pub fn is_internal_app_entry_name(name: &OsStr) -> bool {
    let name = name.to_string_lossy();
    if name.eq_ignore_ascii_case(PORTABLE_METADATA_BUNDLE_DIRNAME) {
        return true;
    }

    let name = name.to_ascii_lowercase();
    name.starts_with(&format!(".{PORTABLE_METADATA_BUNDLE_DIRNAME}.")) && name.ends_with(".tmp")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DirEntryKind {
    Directory,
    ReparseDirectory,
    File,
    Other,
}

impl DirEntryKind {
    pub fn is_directory(self) -> bool {
        matches!(self, Self::Directory | Self::ReparseDirectory)
    }

    pub fn is_file(self) -> bool {
        matches!(self, Self::File)
    }
}

pub fn classify_dir_entry(entry: &DirEntry, file_type: &FileType) -> DirEntryKind {
    if file_type.is_dir() {
        return DirEntryKind::Directory;
    }
    if file_type.is_file() {
        return DirEntryKind::File;
    }
    classify_special_dir_entry(entry, file_type)
}

const FILE_ATTRIBUTE_HIDDEN: u32 = 0x2;
const FILE_ATTRIBUTE_SYSTEM: u32 = 0x4;

/// Windows のファイル属性から、一覧で除外すべきエントリかを判定する純関数。
///
/// Hidden + System は保護された OS ファイルとして常に隠し、Hidden のみは
/// show_hidden_files が false のときだけ隠す。
pub fn should_hide_dir_entry(attributes: u32, show_hidden_files: bool) -> bool {
    let hidden = attributes & FILE_ATTRIBUTE_HIDDEN != 0;
    let system = attributes & FILE_ATTRIBUTE_SYSTEM != 0;
    hidden && (system || !show_hidden_files)
}

/// DirEntry のキャッシュ済み情報だけを使って一覧の表示可否を判定する。
///
/// Windows の entry.metadata() は FindFirstFile / FindNextFile の結果を再利用するため、
/// エントリごとの追加 syscall は発生しない。属性を取得できない場合は従来どおり表示側へ倒す。
#[cfg(windows)]
pub fn should_hide_fs_entry(entry: &DirEntry, show_hidden_files: bool) -> bool {
    use std::os::windows::fs::MetadataExt;

    entry.metadata().ok().is_some_and(|metadata| {
        should_hide_dir_entry(metadata.file_attributes(), show_hidden_files)
    })
}

/// Unix 系では Windows 属性がないため、先頭 . を Hidden 相当として扱う。
#[cfg(not(windows))]
pub fn should_hide_fs_entry(entry: &DirEntry, show_hidden_files: bool) -> bool {
    !show_hidden_files && entry.file_name().to_string_lossy().starts_with('.')
}

#[cfg(windows)]
fn classify_special_dir_entry(entry: &DirEntry, file_type: &FileType) -> DirEntryKind {
    use std::os::windows::fs::{FileTypeExt, MetadataExt};

    if file_type.is_symlink_dir() {
        return DirEntryKind::ReparseDirectory;
    }
    if file_type.is_symlink_file() {
        return DirEntryKind::File;
    }

    const FILE_ATTRIBUTE_DIRECTORY: u32 = 0x10;
    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;

    let Ok(meta) = entry.metadata() else {
        return if file_type.is_symlink() {
            DirEntryKind::File
        } else {
            DirEntryKind::Other
        };
    };
    let attrs = meta.file_attributes();
    let is_reparse = file_type.is_symlink() || attrs & FILE_ATTRIBUTE_REPARSE_POINT != 0;
    if is_reparse && attrs & FILE_ATTRIBUTE_DIRECTORY != 0 {
        DirEntryKind::ReparseDirectory
    } else if is_reparse {
        DirEntryKind::File
    } else {
        DirEntryKind::Other
    }
}

#[cfg(not(windows))]
fn classify_special_dir_entry(_entry: &DirEntry, _file_type: &FileType) -> DirEntryKind {
    DirEntryKind::Other
}

pub fn directory_visit_key(path: &Path) -> String {
    let resolved = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    crate::path_key::normalize_keep_drive(&resolved)
}

pub fn mark_directory_visited(path: &Path, visited: &mut HashSet<String>) -> bool {
    visited.insert(directory_visit_key(path))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn classify_child(root: &Path, name: &str) -> Option<DirEntryKind> {
        for entry in std::fs::read_dir(root).ok()?.flatten() {
            if entry.file_name() == name {
                let ft = entry.file_type().ok()?;
                return Some(classify_dir_entry(&entry, &ft));
            }
        }
        None
    }

    #[test]
    fn portable_metadata_bundle_name_is_always_internal() {
        assert!(is_internal_app_entry_name(OsStr::new(
            "mimageviewer.meta.miv"
        )));
        assert!(is_internal_app_entry_name(OsStr::new(
            "MIMAGEVIEWER.META.MIV"
        )));
        assert!(is_internal_app_entry_name(OsStr::new(
            ".mimageviewer.meta.miv.1234.tmp"
        )));
        assert!(is_internal_app_entry_name(OsStr::new(
            ".MIMAGEVIEWER.META.MIV.1234.old.tmp"
        )));
        assert!(!is_internal_app_entry_name(OsStr::new(
            ".mimageviewer.meta.miv.tmp.jpg"
        )));
        assert!(!is_internal_app_entry_name(OsStr::new("photos")));
    }

    #[test]
    fn classifies_normal_directory_and_file() {
        let tmp = tempfile::TempDir::new().unwrap();
        std::fs::create_dir(tmp.path().join("dir")).unwrap();
        std::fs::write(tmp.path().join("file.jpg"), b"x").unwrap();

        assert!(
            classify_child(tmp.path(), "dir")
                .expect("dir")
                .is_directory()
        );
        assert!(
            classify_child(tmp.path(), "file.jpg")
                .expect("file")
                .is_file()
        );
    }

    #[test]
    fn hidden_system_entries_are_always_hidden() {
        let attributes = FILE_ATTRIBUTE_HIDDEN | FILE_ATTRIBUTE_SYSTEM;
        assert!(should_hide_dir_entry(attributes, false));
        assert!(should_hide_dir_entry(attributes, true));
    }

    #[test]
    fn hidden_only_entries_follow_show_hidden_setting() {
        assert!(should_hide_dir_entry(FILE_ATTRIBUTE_HIDDEN, false));
        assert!(!should_hide_dir_entry(FILE_ATTRIBUTE_HIDDEN, true));
    }

    #[test]
    fn normal_entries_are_always_visible() {
        assert!(!should_hide_dir_entry(0, false));
        assert!(!should_hide_dir_entry(0, true));
    }

    #[cfg(windows)]
    #[test]
    fn classifies_windows_directory_symlink_as_directory() {
        let tmp = tempfile::TempDir::new().unwrap();
        let target = tmp.path().join("target");
        let link = tmp.path().join("link");
        std::fs::create_dir(&target).unwrap();
        if std::os::windows::fs::symlink_dir(&target, &link).is_err() {
            return;
        }

        assert_eq!(
            classify_child(tmp.path(), "link"),
            Some(DirEntryKind::ReparseDirectory)
        );
    }
}
