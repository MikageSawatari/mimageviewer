//! Windows の既知フォルダとドライブ一覧を取得する小さなヘルパー。
//!
//! Desktop / Pictures / Downloads は OneDrive や管理者設定によるリダイレクトを
//! 正しく扱うため Known Folder API を優先し、失敗時だけ USERPROFILE 配下へ
//! フォールバックする。

use std::path::{Path, PathBuf};

use crate::settings::{Settings, StartupFolderMode};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QuickLocation {
    pub label: &'static str,
    pub path: PathBuf,
}

/// フォルダバーの「場所▼」とリモート Home が共有する、表示順を含む列挙結果。
///
/// 表示条件・区切り・既知フォルダの重複除去はこの型の生成時に確定し、各 UI は
/// 描画と遷移だけを担当する。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LocationMenuEntry {
    DriveList,
    ReadingHistory,
    Bookmarks,
    Rating { stars: Vec<u8> },
    Bookshelf,
    Separator,
    QuickLocation(QuickLocation),
    DriveRoot(PathBuf),
}

pub fn location_menu_entries(settings: &Settings) -> Vec<LocationMenuEntry> {
    let desktop = settings.show_location_desktop.then(desktop_dir).flatten();
    let pictures = settings.show_location_pictures.then(pictures_dir).flatten();
    let downloads = settings
        .show_location_downloads
        .then(downloads_dir)
        .flatten();
    let drives = settings
        .show_location_drive_roots
        .then(available_drives)
        .unwrap_or_default();
    location_menu_entries_from(settings, desktop, pictures, downloads, drives)
}

fn location_menu_entries_from(
    settings: &Settings,
    desktop: Option<PathBuf>,
    pictures: Option<PathBuf>,
    downloads: Option<PathBuf>,
    drives: Vec<PathBuf>,
) -> Vec<LocationMenuEntry> {
    let mut entries = Vec::new();
    if settings.show_location_drive_list {
        entries.push(LocationMenuEntry::DriveList);
    }
    if settings.show_location_reading_history {
        entries.push(LocationMenuEntry::ReadingHistory);
    }
    // 本体にはブックマーク専用の非表示設定がない。
    entries.push(LocationMenuEntry::Bookmarks);
    if settings.show_location_rating {
        entries.push(LocationMenuEntry::Rating {
            stars: (1..=5).collect(),
        });
    }
    if settings.show_location_bookshelf {
        entries.push(LocationMenuEntry::Bookshelf);
    }

    let mut quick_locations = Vec::new();
    if settings.show_location_desktop {
        push_unique_location(&mut quick_locations, "デスクトップ", desktop);
    }
    if settings.show_location_pictures {
        push_unique_location(&mut quick_locations, "ピクチャ", pictures);
    }
    if settings.show_location_downloads {
        push_unique_location(&mut quick_locations, "ダウンロード", downloads);
    }
    if !quick_locations.is_empty() {
        entries.push(LocationMenuEntry::Separator);
        entries.extend(
            quick_locations
                .into_iter()
                .map(LocationMenuEntry::QuickLocation),
        );
    }

    if settings.show_location_drive_roots && !drives.is_empty() {
        entries.push(LocationMenuEntry::Separator);
        entries.extend(drives.into_iter().map(LocationMenuEntry::DriveRoot));
    }
    entries
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

pub fn startup_folder(
    mode: StartupFolderMode,
    last_folder: Option<&Path>,
    specific_folder: Option<&Path>,
) -> Option<PathBuf> {
    match mode {
        StartupFolderMode::Previous => last_folder
            .and_then(resolve_startup_last_folder)
            .or_else(desktop_dir),
        StartupFolderMode::Desktop => {
            desktop_dir().or_else(|| last_folder.and_then(resolve_startup_last_folder))
        }
        StartupFolderMode::Specific => specific_folder
            .and_then(resolve_startup_last_folder)
            .or_else(desktop_dir)
            .or_else(|| last_folder.and_then(resolve_startup_last_folder)),
        StartupFolderMode::Drives | StartupFolderMode::ReadingHistory => None,
    }
}

/// 起動フォルダが、前回終了時にいた場所**そのもの**か。
///
/// カーソルの復元は「同じ場所を開き直した」ときだけ意味を持つ。`Previous` 以外のモードにも
/// `last_folder` へ落ちてくる経路があるので、**モードではなく解決結果**で判定する。
///
/// [`resolve_startup_last_folder`] は消えたフォルダを**祖先へ遡上**して返すので、比較相手は
/// 解決後ではなく `last_folder` そのもの。遡上した先へ前のカーソル名を持ち込まないため。
pub fn startup_folder_is_last_folder(resolved: &Path, last_folder: Option<&Path>) -> bool {
    last_folder.is_some_and(|last| crate::folder_tree::path_eq(last, resolved))
}

/// 起動時にカーソルを戻す項目名。戻さないときは `None`。
///
/// 保存側 (`App::cursor_name_to_persist`) と対になる判断で、**同じ
/// [`startup_folder_is_last_folder`] を通す**。片方だけ条件が増えると、保存はするのに
/// 復元しない (またはその逆) という噛み合わない状態になる。
/// 戻す項目名と、分かっていればその行位置。
///
/// 行位置が `None` なのは「一番上の行にいた」ではなく**分からない**。`Some(0)` は本当に
/// 一番上にいたという意味なので、この 2 つを同じ値にしない。分からないときは
/// `apply_scroll_to_selected` が従来どおり見える最小限だけ動かす。
pub fn startup_cursor_hint(
    settings: &Settings,
    resolved_folder: &Path,
) -> Option<(String, Option<u32>)> {
    if !settings.restore_last_cursor {
        return None;
    }
    if !startup_folder_is_last_folder(resolved_folder, settings.last_folder.as_deref()) {
        return None;
    }
    let name = settings
        .last_cursor_name
        .as_ref()
        .filter(|name| !name.is_empty())
        .cloned()?;
    Some((name, settings.last_cursor_rows_above))
}

/// 起動時の last_folder を復元する。フォルダ (または仮想フォルダ ZIP/PDF) がそのまま
/// 開けるならそれを、消えていたら**直近の存在する親フォルダ**を返す。どの親も辿れない
/// (ドライブ自体が無い等) ときだけ None で、呼び出し側が Desktop にフォールバックする。
///
/// 末端サブフォルダだけ削除されたケース (例: `D:\Photos\2024\Jan` の `Jan` だけ消えた)
/// で、いきなり Desktop へ飛ばさず `D:\Photos\2024` を開くための祖先遡上。
fn resolve_startup_last_folder(path: &Path) -> Option<PathBuf> {
    crate::folder_tree::resolve_openable_path(path)
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
    use super::{
        LocationMenuEntry, QuickLocation, location_menu_entries_from, push_unique_location,
        startup_cursor_hint, startup_folder, startup_folder_is_last_folder,
    };
    use crate::settings::{Settings, StartupFolderMode};
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

    #[test]
    fn location_menu_entries_follow_core_order_and_separators() {
        let settings = Settings::default();
        let entries = location_menu_entries_from(
            &settings,
            Some(PathBuf::from(r"C:\Users\me\Desktop")),
            Some(PathBuf::from(r"C:\Users\me\Pictures")),
            Some(PathBuf::from(r"C:\Users\me\Downloads")),
            vec![PathBuf::from(r"C:\"), PathBuf::from(r"D:\")],
        );
        assert_eq!(
            entries,
            vec![
                LocationMenuEntry::DriveList,
                LocationMenuEntry::ReadingHistory,
                LocationMenuEntry::Bookmarks,
                LocationMenuEntry::Rating {
                    stars: vec![1, 2, 3, 4, 5],
                },
                LocationMenuEntry::Bookshelf,
                LocationMenuEntry::Separator,
                LocationMenuEntry::QuickLocation(QuickLocation {
                    label: "デスクトップ",
                    path: PathBuf::from(r"C:\Users\me\Desktop"),
                }),
                LocationMenuEntry::QuickLocation(QuickLocation {
                    label: "ピクチャ",
                    path: PathBuf::from(r"C:\Users\me\Pictures"),
                }),
                LocationMenuEntry::QuickLocation(QuickLocation {
                    label: "ダウンロード",
                    path: PathBuf::from(r"C:\Users\me\Downloads"),
                }),
                LocationMenuEntry::Separator,
                LocationMenuEntry::DriveRoot(PathBuf::from(r"C:\")),
                LocationMenuEntry::DriveRoot(PathBuf::from(r"D:\")),
            ]
        );
    }

    #[test]
    fn location_menu_entries_apply_settings_and_skip_missing_or_duplicate_folders() {
        let mut settings = Settings::default();
        settings.show_location_drive_list = false;
        settings.show_location_reading_history = false;
        settings.show_location_rating = false;
        settings.show_location_bookshelf = false;
        settings.show_location_downloads = true;
        settings.show_location_drive_roots = false;
        let duplicate = PathBuf::from(r"C:\Users\me\Desktop");
        let entries = location_menu_entries_from(
            &settings,
            Some(duplicate.clone()),
            None,
            Some(duplicate.clone()),
            vec![PathBuf::from(r"C:\")],
        );
        assert_eq!(
            entries,
            vec![
                LocationMenuEntry::Bookmarks,
                LocationMenuEntry::Separator,
                LocationMenuEntry::QuickLocation(QuickLocation {
                    label: "デスクトップ",
                    path: duplicate,
                }),
            ]
        );
    }

    #[test]
    fn every_location_setting_removes_its_shared_menu_entry() {
        type DisableSetting = fn(&mut Settings);
        type MatchesEntry = fn(&LocationMenuEntry) -> bool;

        let cases: [(&str, DisableSetting, MatchesEntry); 8] = [
            (
                "show_location_drive_list",
                |settings| settings.show_location_drive_list = false,
                |entry| matches!(entry, LocationMenuEntry::DriveList),
            ),
            (
                "show_location_reading_history",
                |settings| settings.show_location_reading_history = false,
                |entry| matches!(entry, LocationMenuEntry::ReadingHistory),
            ),
            (
                "show_location_rating",
                |settings| settings.show_location_rating = false,
                |entry| matches!(entry, LocationMenuEntry::Rating { .. }),
            ),
            (
                "show_location_bookshelf",
                |settings| settings.show_location_bookshelf = false,
                |entry| matches!(entry, LocationMenuEntry::Bookshelf),
            ),
            (
                "show_location_desktop",
                |settings| settings.show_location_desktop = false,
                |entry| matches!(entry, LocationMenuEntry::QuickLocation(location) if location.label == "デスクトップ"),
            ),
            (
                "show_location_pictures",
                |settings| settings.show_location_pictures = false,
                |entry| matches!(entry, LocationMenuEntry::QuickLocation(location) if location.label == "ピクチャ"),
            ),
            (
                "show_location_downloads",
                |settings| settings.show_location_downloads = false,
                |entry| matches!(entry, LocationMenuEntry::QuickLocation(location) if location.label == "ダウンロード"),
            ),
            (
                "show_location_drive_roots",
                |settings| settings.show_location_drive_roots = false,
                |entry| matches!(entry, LocationMenuEntry::DriveRoot(_)),
            ),
        ];

        for (name, disable, matches_entry) in cases {
            let mut settings = Settings::default();
            let baseline = location_menu_entries_from(
                &settings,
                Some(PathBuf::from(r"C:\Users\me\Desktop")),
                Some(PathBuf::from(r"C:\Users\me\Pictures")),
                Some(PathBuf::from(r"C:\Users\me\Downloads")),
                vec![PathBuf::from(r"C:\")],
            );
            assert!(
                baseline.iter().any(matches_entry),
                "{name} must have a baseline entry"
            );

            disable(&mut settings);
            let hidden = location_menu_entries_from(
                &settings,
                Some(PathBuf::from(r"C:\Users\me\Desktop")),
                Some(PathBuf::from(r"C:\Users\me\Pictures")),
                Some(PathBuf::from(r"C:\Users\me\Downloads")),
                vec![PathBuf::from(r"C:\")],
            );
            assert!(
                !hidden.iter().any(matches_entry),
                "{name} must hide the shared entry"
            );
        }
    }

    #[test]
    fn startup_folder_specific_uses_existing_folder() {
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let chosen = startup_folder(StartupFolderMode::Specific, None, Some(tmp.path()))
            .expect("existing specific folder should be returned");
        assert_eq!(chosen, tmp.path());
    }

    #[test]
    fn startup_folder_drives_is_virtual_and_returns_none() {
        let tmp = tempfile::TempDir::new().expect("tempdir");
        assert_eq!(
            startup_folder(
                StartupFolderMode::Drives,
                Some(tmp.path()),
                Some(tmp.path())
            ),
            None
        );
    }

    /// 復元側の判断。設定 OFF・別の場所・名前が無い、のどれでも `None`。
    #[test]
    fn the_startup_cursor_hint_needs_the_setting_and_the_same_place() {
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let folder = tmp.path().join("Photos");
        std::fs::create_dir_all(&folder).expect("mkdir");

        let mut settings = Settings::default();
        assert!(
            settings.restore_last_cursor,
            "既定 ON。旧設定を読んだ利用者も serde default で ON になる"
        );
        settings.last_folder = Some(folder.clone());
        settings.last_cursor_name = Some("b.jpg".to_string());
        settings.last_cursor_rows_above = Some(3);
        assert_eq!(
            startup_cursor_hint(&settings, &folder),
            Some(("b.jpg".to_string(), Some(3)))
        );

        // 行位置だけ分からない場合。**`Some(0)` にしない** — 0 は「一番上の行にいた」で、
        // 分からないのとは別。分からないときは従来どおり見える最小限だけ動かす。
        settings.last_cursor_rows_above = None;
        assert_eq!(
            startup_cursor_hint(&settings, &folder),
            Some(("b.jpg".to_string(), None))
        );
        settings.last_cursor_rows_above = Some(0);
        assert_eq!(
            startup_cursor_hint(&settings, &folder),
            Some(("b.jpg".to_string(), Some(0)))
        );

        settings.restore_last_cursor = false;
        assert_eq!(startup_cursor_hint(&settings, &folder), None);
        settings.restore_last_cursor = true;

        // 別の場所を開くなら、前回の名前は無関係。
        assert_eq!(startup_cursor_hint(&settings, tmp.path()), None);

        // 保存側が「対にならない」と判断して破棄した後。
        settings.last_cursor_name = None;
        assert_eq!(startup_cursor_hint(&settings, &folder), None);

        // 空文字は名前として使えない (items のどれにも一致しないので実害は無いが、
        // 「復元した」と誤解する状態を作らない)。
        settings.last_cursor_name = Some(String::new());
        assert_eq!(startup_cursor_hint(&settings, &folder), None);
    }

    /// 前回いた場所をそのまま開いたときだけ true。カーソル復元の可否はこれで決まる。
    #[test]
    fn the_cursor_belongs_to_the_place_we_actually_reopened() {
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let folder = tmp.path().join("Photos");
        std::fs::create_dir_all(&folder).expect("mkdir");

        assert!(startup_folder_is_last_folder(&folder, Some(&folder)));
        // Windows の大文字小文字非区別。`path_eq` に委ねているのでここは確認だけ。
        let shouted = std::path::PathBuf::from(folder.to_string_lossy().to_uppercase());
        assert!(startup_folder_is_last_folder(&shouted, Some(&folder)));
        // 初回起動 (last_folder が無い) では復元する相手がいない。
        assert!(!startup_folder_is_last_folder(&folder, None));
        // 別の場所を開いたなら、前回のカーソル名は使えない。
        assert!(!startup_folder_is_last_folder(tmp.path(), Some(&folder)));
    }

    /// 末端フォルダが消えて**祖先へ遡上**したときは復元しない。
    ///
    /// `resolve_startup_last_folder` は開けない場所を親へ辿るので、素朴に「起動フォルダが
    /// 決まったなら前回の続き」と扱うと、消えたフォルダのカーソル名を親フォルダへ
    /// 持ち込むことになる。判定を解決**後**ではなく `last_folder` そのものと突き合わせて
    /// いるのはこのため。
    #[test]
    fn walking_up_to_a_surviving_ancestor_does_not_carry_the_cursor() {
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let parent = tmp.path().join("Photos");
        std::fs::create_dir_all(&parent).expect("mkdir");
        let gone = parent.join("2024-Jan");

        let resolved = startup_folder(StartupFolderMode::Previous, Some(&gone), None)
            .expect("親へ遡上して開けるはず");
        assert_eq!(resolved, parent, "この前提が崩れたらテストの意味が変わる");
        assert!(!startup_folder_is_last_folder(&resolved, Some(&gone)));
    }

    #[test]
    fn startup_folder_reading_history_is_virtual_and_returns_none() {
        let tmp = tempfile::TempDir::new().expect("tempdir");
        assert_eq!(
            startup_folder(
                StartupFolderMode::ReadingHistory,
                Some(tmp.path()),
                Some(tmp.path())
            ),
            None
        );
    }
}
