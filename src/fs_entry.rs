//! `std::fs::DirEntry` classification helpers for filesystem listing paths.
//!
//! The hot listing paths must keep using `DirEntry::file_type()` instead of
//! `Path::is_dir()` / `is_file()`. On Windows, directory symlinks and junctions
//! are reported as reparse points, so they need a small extra classification
//! step before the UI/search walkers can treat them as folders.

use std::collections::HashSet;
use std::fs::{DirEntry, FileType};
use std::path::Path;

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
