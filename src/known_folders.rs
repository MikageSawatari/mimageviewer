//! Windows の既知フォルダとドライブ一覧を取得する小さなヘルパー。
//!
//! Desktop / Pictures / Downloads は OneDrive や管理者設定によるリダイレクトを
//! 正しく扱うため Known Folder API を優先し、失敗時だけ USERPROFILE 配下へ
//! フォールバックする。

use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QuickLocation {
    pub label: &'static str,
    pub path: PathBuf,
}

pub fn desktop_dir() -> Option<PathBuf> {
    known_folder_or_userprofile("Desktop", KnownFolder::Desktop)
}

pub fn pictures_dir() -> Option<PathBuf> {
    known_folder_or_userprofile("Pictures", KnownFolder::Pictures)
}

pub fn downloads_dir() -> Option<PathBuf> {
    known_folder_or_userprofile("Downloads", KnownFolder::Downloads)
}

pub fn quick_locations() -> Vec<QuickLocation> {
    let mut locations = Vec::new();
    push_unique_location(&mut locations, "デスクトップ", desktop_dir());
    push_unique_location(&mut locations, "ピクチャ", pictures_dir());
    push_unique_location(&mut locations, "ダウンロード", downloads_dir());
    locations
}

pub fn startup_folder(last_folder: Option<&Path>) -> Option<PathBuf> {
    last_folder
        .and_then(resolve_startup_last_folder)
        .or_else(desktop_dir)
}

fn resolve_startup_last_folder(path: &Path) -> Option<PathBuf> {
    if path.is_dir() {
        return Some(path.to_path_buf());
    }
    if path.is_file() && crate::folder_tree::is_virtual_folder(path) {
        return Some(path.to_path_buf());
    }
    None
}

fn push_unique_location(
    locations: &mut Vec<QuickLocation>,
    label: &'static str,
    path: Option<PathBuf>,
) {
    let Some(path) = path else {
        return;
    };
    if locations
        .iter()
        .any(|existing| crate::folder_tree::path_eq(&existing.path, &path))
    {
        return;
    }
    locations.push(QuickLocation { label, path });
}

#[derive(Clone, Copy)]
enum KnownFolder {
    Desktop,
    Pictures,
    Downloads,
}

fn known_folder_or_userprofile(fallback_name: &str, folder: KnownFolder) -> Option<PathBuf> {
    platform_known_folder(folder)
        .filter(|p| p.is_dir())
        .or_else(|| userprofile_child(fallback_name).filter(|p| p.is_dir()))
}

fn userprofile_child(name: &str) -> Option<PathBuf> {
    std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(|p| PathBuf::from(p).join(name))
}

#[cfg(windows)]
fn platform_known_folder(folder: KnownFolder) -> Option<PathBuf> {
    use windows::Win32::System::Com::CoTaskMemFree;
    use windows::Win32::UI::Shell::{
        FOLDERID_Desktop, FOLDERID_Downloads, FOLDERID_Pictures, KF_FLAG_DEFAULT,
        SHGetKnownFolderPath,
    };

    let folder_id = match folder {
        KnownFolder::Desktop => &FOLDERID_Desktop,
        KnownFolder::Pictures => &FOLDERID_Pictures,
        KnownFolder::Downloads => &FOLDERID_Downloads,
    };

    unsafe {
        let pwstr = SHGetKnownFolderPath(folder_id, KF_FLAG_DEFAULT, None).ok()?;
        let path = pwstr.to_string().ok().map(PathBuf::from);
        CoTaskMemFree(Some(pwstr.0 as *const core::ffi::c_void));
        path
    }
}

#[cfg(not(windows))]
fn platform_known_folder(folder: KnownFolder) -> Option<PathBuf> {
    let name = match folder {
        KnownFolder::Desktop => "Desktop",
        KnownFolder::Pictures => "Pictures",
        KnownFolder::Downloads => "Downloads",
    };
    userprofile_child(name)
}

#[cfg(windows)]
pub fn available_drives() -> Vec<PathBuf> {
    use windows::Win32::Storage::FileSystem::{GetDriveTypeW, GetLogicalDrives};
    use windows::core::PCWSTR;

    const DRIVE_UNKNOWN: u32 = 0;
    const DRIVE_NO_ROOT_DIR: u32 = 1;

    let mask = unsafe { GetLogicalDrives() };
    let mut drives = Vec::new();
    for i in 0..26u32 {
        if (mask & (1 << i)) == 0 {
            continue;
        }
        let letter = (b'A' + i as u8) as char;
        let root = format!("{letter}:\\");
        let wide: Vec<u16> = root.encode_utf16().chain(std::iter::once(0)).collect();
        let drive_type = unsafe { GetDriveTypeW(PCWSTR(wide.as_ptr())) };
        if drive_type == DRIVE_UNKNOWN || drive_type == DRIVE_NO_ROOT_DIR {
            continue;
        }
        drives.push(PathBuf::from(root));
    }
    drives
}

#[cfg(not(windows))]
pub fn available_drives() -> Vec<PathBuf> {
    vec![PathBuf::from("/")]
}

#[cfg(test)]
mod tests {
    use super::{QuickLocation, push_unique_location};
    use std::path::PathBuf;

    #[test]
    fn quick_locations_skip_duplicates() {
        let mut locations = vec![QuickLocation {
            label: "デスクトップ",
            path: PathBuf::from(r"C:\Users\me\Desktop"),
        }];
        push_unique_location(
            &mut locations,
            "ダウンロード",
            Some(PathBuf::from(r"C:\Users\me\Desktop")),
        );
        assert_eq!(locations.len(), 1);
    }
}
