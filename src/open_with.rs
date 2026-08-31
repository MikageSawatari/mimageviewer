/// 拡張子に関連付けられたアプリケーションの列挙と起動。
///
/// Windows Shell API (`SHAssocEnumHandlers`) を使用して、
/// ファイル拡張子に対応するアプリ一覧を取得する。
use std::path::{Path, PathBuf};

/// アプリケーションハンドラ情報
#[derive(Clone, Debug)]
pub struct AppHandler {
    pub display_name: String,
    pub handler_id: String,
}

/// ファイル選択ダイアログで利用者が明示的に選んだ実行ファイル。
///
/// `AppHandler` と型を分け、関連付け識別子を実行ファイルパスとして扱えないようにする。
#[derive(Clone, Debug)]
pub struct PickedExecutable {
    pub display_name: String,
    pub executable: std::path::PathBuf,
}

/// 指定された拡張子に関連付けられたアプリケーション一覧を返す。
///
/// `extension` は `.jpg` のようにドット付きの拡張子。
/// エラー時は空の Vec を返す。
#[cfg(windows)]
pub fn enumerate_handlers(extension: &str) -> Vec<AppHandler> {
    match enumerate_handlers_inner(extension) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("enumerate_handlers failed for {extension}: {e}");
            Vec::new()
        }
    }
}

#[cfg(not(windows))]
pub fn enumerate_handlers(_extension: &str) -> Vec<AppHandler> {
    Vec::new()
}

#[cfg(windows)]
fn enumerate_handlers_inner(
    extension: &str,
) -> Result<Vec<AppHandler>, Box<dyn std::error::Error>> {
    use windows::Win32::System::Com::CoTaskMemFree;
    use windows::Win32::UI::Shell::{ASSOC_FILTER_RECOMMENDED, SHAssocEnumHandlers};
    use windows::core::PCWSTR;

    let _com = initialize_com_sta()?;

    let ext_wide: Vec<u16> = extension.encode_utf16().chain(std::iter::once(0)).collect();

    let enum_handlers =
        unsafe { SHAssocEnumHandlers(PCWSTR(ext_wide.as_ptr()), ASSOC_FILTER_RECOMMENDED)? };

    let mut result = Vec::new();
    let mut seen_ids = std::collections::HashSet::new();

    loop {
        let mut handlers: [Option<_>; 1] = [None];
        let mut fetched: u32 = 0;
        let hr = unsafe { enum_handlers.Next(&mut handlers, Some(&mut fetched)) };
        if hr.is_err() || fetched == 0 {
            break;
        }
        if let Some(handler) = handlers[0].take() {
            let display_name = unsafe { handler.GetUIName() }.ok().and_then(|value| {
                let text = unsafe { value.to_string() }.ok();
                unsafe { CoTaskMemFree(Some(value.0 as *const _)) };
                text
            });
            let handler_id = unsafe { handler.GetName() }.ok().and_then(|value| {
                let text = unsafe { value.to_string() }.ok();
                unsafe { CoTaskMemFree(Some(value.0 as *const _)) };
                text
            });

            if let (Some(display_name), Some(handler_id)) = (display_name, handler_id) {
                let key = handler_id.to_lowercase();
                if seen_ids.insert(key) {
                    result.push(AppHandler {
                        display_name,
                        handler_id,
                    });
                }
            }
        }
    }

    Ok(result)
}

/// ファイル選択ダイアログで .exe を選ばせ、(表示名, exeパス) を返す。
/// キャンセル時は None。
#[cfg(windows)]
pub fn pick_exe_dialog() -> Option<PickedExecutable> {
    use windows::Win32::UI::Controls::Dialogs::{
        GetOpenFileNameW, OFN_FILEMUSTEXIST, OFN_NOCHANGEDIR, OFN_PATHMUSTEXIST, OPENFILENAMEW,
    };
    use windows::core::PCWSTR;

    // フィルタ文字列: "実行ファイル (*.exe)\0*.exe\0\0"
    let filter: Vec<u16> = "実行ファイル (*.exe)\0*.exe\0\0".encode_utf16().collect();

    let mut file_buf = vec![0u16; 512];

    let mut ofn = OPENFILENAMEW::default();
    ofn.lStructSize = std::mem::size_of::<OPENFILENAMEW>() as u32;
    ofn.lpstrFilter = PCWSTR(filter.as_ptr());
    ofn.lpstrFile = windows::core::PWSTR(file_buf.as_mut_ptr());
    ofn.nMaxFile = file_buf.len() as u32;
    ofn.Flags = OFN_FILEMUSTEXIST | OFN_PATHMUSTEXIST | OFN_NOCHANGEDIR;

    let ok = unsafe { GetOpenFileNameW(&mut ofn) };
    if !ok.as_bool() {
        return None;
    }

    let path_str = String::from_utf16_lossy(
        &file_buf[..file_buf
            .iter()
            .position(|&c| c == 0)
            .unwrap_or(file_buf.len())],
    );
    let path = std::path::Path::new(&path_str);

    // 表示名: exe のファイル名からステムを取得
    let display_name = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("Unknown")
        .to_string();

    Some(PickedExecutable {
        display_name,
        executable: path.to_path_buf(),
    })
}

#[cfg(not(windows))]
pub fn pick_exe_dialog() -> Option<PickedExecutable> {
    None
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum AssociationLaunchError {
    NoTargets,
    MissingExtension,
    NonUnicodeExtension,
    HandlerNotFound,
    Shell(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct PackageFullNameParts {
    name: String,
    publisher_id: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct WindowsAppsHandlerIdentity {
    package_name: String,
    publisher_id: String,
    relative_path: String,
}

impl WindowsAppsHandlerIdentity {
    fn matches(&self, other: &Self) -> bool {
        self.package_name.eq_ignore_ascii_case(&other.package_name)
            && self.publisher_id.eq_ignore_ascii_case(&other.publisher_id)
            && self
                .relative_path
                .eq_ignore_ascii_case(&other.relative_path)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct AssociationHandlerMatch {
    index: usize,
    needs_writeback: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct AssociationInvokeOutcome {
    pub refreshed_handler_id: Option<String>,
}

/// Package full name の更新で変わらない Name と PublisherId を取り出す。
fn split_package_full_name(package_full_name: &str) -> Option<PackageFullNameParts> {
    let (name_version_arch, publisher_id) = package_full_name.rsplit_once("__")?;
    let (name_version, architecture) = name_version_arch.rsplit_once('_')?;
    let (name, version) = name_version.rsplit_once('_')?;
    if name.is_empty() || version.is_empty() || architecture.is_empty() || publisher_id.is_empty() {
        return None;
    }
    Some(PackageFullNameParts {
        name: name.to_string(),
        publisher_id: publisher_id.to_string(),
    })
}

/// WindowsApps 配下の handler ID を、更新で変わらない package identity と exe 相対パスへ正規化する。
fn normalize_windows_apps_handler_id(handler_id: &str) -> Option<WindowsAppsHandlerIdentity> {
    let components: Vec<_> = handler_id
        .split(['\\', '/'])
        .filter(|component| !component.is_empty())
        .collect();
    let windows_apps_index = components
        .iter()
        .position(|component| component.eq_ignore_ascii_case("WindowsApps"))?;
    let package_full_name = *components.get(windows_apps_index + 1)?;
    let remaining = components.get(windows_apps_index + 2..)?;
    if remaining.is_empty() {
        return None;
    }
    let package = split_package_full_name(package_full_name)?;
    Some(WindowsAppsHandlerIdentity {
        package_name: package.name,
        publisher_id: package.publisher_id,
        relative_path: remaining.join("\\"),
    })
}

fn association_extension(path: &Path) -> Result<String, AssociationLaunchError> {
    let extension = path
        .extension()
        .ok_or(AssociationLaunchError::MissingExtension)?
        .to_str()
        .ok_or(AssociationLaunchError::NonUnicodeExtension)?;
    if extension.is_empty() {
        return Err(AssociationLaunchError::MissingExtension);
    }
    Ok(format!(".{extension}"))
}

/// Batch 起動でも、関連付けハンドラの列挙には先頭対象の拡張子だけを使う。
///
/// 対象順は外部ツール側で「現在項目を先頭」に解決済みという契約であり、ここで並べ替えない。
fn association_extension_for_paths(paths: &[PathBuf]) -> Result<String, AssociationLaunchError> {
    let first = paths.first().ok_or(AssociationLaunchError::NoTargets)?;
    association_extension(first)
}

/// `shell_data_object_for_paths` は一部の PIDL 解決に失敗しても、解決できた対象の
/// `IDataObject` を返せる。関連付け Batch は部分実行せず、1 件でも失敗したら Invoke 前に止める。
fn ensure_all_association_paths_resolved(
    failed_paths: usize,
) -> Result<(), AssociationLaunchError> {
    if failed_paths == 0 {
        Ok(())
    } else {
        Err(AssociationLaunchError::Shell(format!(
            "対象ファイルを Shell に渡せません (解決失敗 {failed_paths} 件)"
        )))
    }
}

fn find_association_handler(
    expected_id: &str,
    expected_display_name: &str,
    candidates: &[AppHandler],
) -> Result<AssociationHandlerMatch, AssociationLaunchError> {
    if let Some(index) = candidates
        .iter()
        .position(|candidate| candidate.handler_id == expected_id)
    {
        return Ok(AssociationHandlerMatch {
            index,
            needs_writeback: false,
        });
    }

    if let Some(expected_package) = normalize_windows_apps_handler_id(expected_id)
        && let Some(index) = candidates.iter().position(|candidate| {
            normalize_windows_apps_handler_id(&candidate.handler_id)
                .is_some_and(|candidate_package| expected_package.matches(&candidate_package))
        })
    {
        return Ok(AssociationHandlerMatch {
            index,
            needs_writeback: true,
        });
    }

    if !expected_display_name.is_empty()
        && let Some(index) = candidates
            .iter()
            .position(|candidate| candidate.display_name == expected_display_name)
    {
        return Ok(AssociationHandlerMatch {
            index,
            needs_writeback: true,
        });
    }

    Err(AssociationLaunchError::HandlerNotFound)
}

fn association_launch_error_message(error: &AssociationLaunchError) -> String {
    match error {
        AssociationLaunchError::NoTargets => {
            "関連付けアプリへ渡す対象ファイルがありません".to_string()
        }
        AssociationLaunchError::MissingExtension => {
            "関連付けアプリを探すための拡張子がありません".to_string()
        }
        AssociationLaunchError::NonUnicodeExtension => {
            "関連付けアプリを探せない拡張子です".to_string()
        }
        AssociationLaunchError::HandlerNotFound => "関連付けアプリが見つかりません".to_string(),
        AssociationLaunchError::Shell(detail) => detail.clone(),
    }
}

#[cfg(windows)]
struct ComApartmentGuard {
    uninitialize: bool,
}

#[cfg(windows)]
impl Drop for ComApartmentGuard {
    fn drop(&mut self) {
        if self.uninitialize {
            unsafe { windows::Win32::System::Com::CoUninitialize() };
        }
    }
}

#[cfg(windows)]
fn initialize_com_sta() -> Result<ComApartmentGuard, windows::core::Error> {
    use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx};

    let result = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
    result.ok()?;
    Ok(ComApartmentGuard { uninitialize: true })
}

/// 保存済み handler ID を先頭対象の拡張子で引き直し、Shell に対象全件を一度に渡す。
///
/// Association 固有の COM ポインタと `IAssocHandler::Invoke` はこの関数から外へ出さない。
/// 呼び出し元は external-tool launch worker なので、列挙・PIDL 解決・Invoke は UI thread
/// では実行されない。
#[cfg(windows)]
fn invoke_association_handler_inner(
    extension: &str,
    expected_id: &str,
    expected_display_name: &str,
    file_paths: &[PathBuf],
) -> Result<AssociationInvokeOutcome, AssociationLaunchError> {
    use windows::Win32::System::Com::CoTaskMemFree;
    use windows::Win32::UI::Shell::{ASSOC_FILTER_NONE, SHAssocEnumHandlers};
    use windows::core::PCWSTR;

    let _com = initialize_com_sta().map_err(|error| {
        AssociationLaunchError::Shell(format!("COM を初期化できません: {error}"))
    })?;
    let extension_wide: Vec<u16> = extension.encode_utf16().chain(std::iter::once(0)).collect();
    let enum_handlers =
        unsafe { SHAssocEnumHandlers(PCWSTR(extension_wide.as_ptr()), ASSOC_FILTER_NONE) }
            .map_err(|error| {
                AssociationLaunchError::Shell(format!("関連付けアプリを列挙できません: {error}"))
            })?;

    let mut handlers = Vec::new();
    let mut candidates = Vec::new();
    loop {
        let mut next = [None];
        let mut fetched = 0u32;
        unsafe { enum_handlers.Next(&mut next, Some(&mut fetched)) }.map_err(|error| {
            AssociationLaunchError::Shell(format!("関連付けアプリの列挙に失敗しました: {error}"))
        })?;
        if fetched == 0 {
            break;
        }
        let Some(handler) = next[0].take() else {
            continue;
        };
        let Ok(name) = (unsafe { handler.GetName() }) else {
            continue;
        };
        let handler_id = unsafe { name.to_string() };
        unsafe { CoTaskMemFree(Some(name.0 as *const _)) };
        if let Ok(handler_id) = handler_id {
            let display_name = unsafe { handler.GetUIName() }
                .ok()
                .and_then(|value| {
                    let text = unsafe { value.to_string() }.ok();
                    unsafe { CoTaskMemFree(Some(value.0 as *const _)) };
                    text
                })
                .unwrap_or_default();
            candidates.push(AppHandler {
                display_name,
                handler_id,
            });
            handlers.push(handler);
        }
    }

    let matched = find_association_handler(expected_id, expected_display_name, &candidates)?;
    let refreshed_handler_id = matched
        .needs_writeback
        .then(|| candidates[matched.index].handler_id.clone());
    let (data, failed_paths) = crate::file_drag::shell_data_object_for_paths(file_paths).map_err(
        |(failed_paths, error)| {
            AssociationLaunchError::Shell(format!(
                "対象ファイルを Shell に渡せません: {error:?} (解決失敗 {failed_paths} 件)"
            ))
        },
    )?;
    ensure_all_association_paths_resolved(failed_paths)?;
    unsafe { handlers[matched.index].Invoke(&data) }.map_err(|error| {
        AssociationLaunchError::Shell(format!("関連付けアプリを起動できません: {error}"))
    })?;
    Ok(AssociationInvokeOutcome {
        refreshed_handler_id,
    })
}

#[cfg(windows)]
pub(crate) fn invoke_association_handler_for_paths(
    handler_id: &str,
    display_name: &str,
    file_paths: &[PathBuf],
) -> Result<AssociationInvokeOutcome, String> {
    let extension = association_extension_for_paths(file_paths)
        .map_err(|error| association_launch_error_message(&error))?;
    invoke_association_handler_inner(&extension, handler_id, display_name, file_paths)
        .map_err(|error| association_launch_error_message(&error))
}

#[cfg(not(windows))]
pub(crate) fn invoke_association_handler_for_paths(
    _handler_id: &str,
    _display_name: &str,
    file_paths: &[PathBuf],
) -> Result<AssociationInvokeOutcome, String> {
    if file_paths.is_empty() {
        return Err(association_launch_error_message(
            &AssociationLaunchError::NoTargets,
        ));
    }
    Err("関連付けアプリの起動は Windows でのみ利用できます".to_string())
}

/// 既存の単一ファイル起動経路向け compatibility wrapper。
pub(crate) fn invoke_association_handler(
    handler_id: &str,
    display_name: &str,
    file_path: &Path,
) -> Result<AssociationInvokeOutcome, String> {
    invoke_association_handler_for_paths(handler_id, display_name, &[file_path.to_path_buf()])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn association_extension_is_pure_and_keeps_the_dot() {
        assert_eq!(
            association_extension(std::path::Path::new(r"C:\images\page.JPG")),
            Ok(".JPG".to_string())
        );
        assert_eq!(
            association_extension(std::path::Path::new(r"C:\images\page")),
            Err(AssociationLaunchError::MissingExtension)
        );
    }

    #[test]
    fn association_batch_uses_only_the_first_path_for_handler_lookup() {
        let paths = vec![
            PathBuf::from(r"C:\images\primary.JPG"),
            PathBuf::from(r"C:\images\secondary.png"),
            PathBuf::from(r"C:\images\without-extension"),
        ];

        assert_eq!(
            association_extension_for_paths(&paths),
            Ok(".JPG".to_string())
        );

        let first_has_no_extension = vec![
            PathBuf::from(r"C:\images\without-extension"),
            PathBuf::from(r"C:\images\secondary.png"),
        ];
        assert_eq!(
            association_extension_for_paths(&first_has_no_extension),
            Err(AssociationLaunchError::MissingExtension),
            "後続 path の拡張子へ暗黙にフォールバックしない"
        );
    }

    #[test]
    fn association_batch_rejects_an_empty_target_list() {
        assert_eq!(
            association_extension_for_paths(&[]),
            Err(AssociationLaunchError::NoTargets)
        );
        assert_eq!(
            association_launch_error_message(&AssociationLaunchError::NoTargets),
            "関連付けアプリへ渡す対象ファイルがありません"
        );
    }

    #[test]
    fn association_batch_rejects_any_unresolved_shell_path() {
        assert_eq!(ensure_all_association_paths_resolved(0), Ok(()));
        assert_eq!(
            ensure_all_association_paths_resolved(2),
            Err(AssociationLaunchError::Shell(
                "対象ファイルを Shell に渡せません (解決失敗 2 件)".to_string()
            ))
        );
    }

    #[test]
    fn handler_id_selection_reports_a_user_visible_not_found_error() {
        let candidates = vec![
            handler("フォト", "Photos.App"),
            handler("ペイント", "Paint.App"),
        ];
        assert_eq!(
            find_association_handler("Paint.App", "ペイント", &candidates),
            Ok(AssociationHandlerMatch {
                index: 1,
                needs_writeback: false,
            })
        );
        let error = find_association_handler("Removed.App", "削除済み", &candidates).unwrap_err();
        assert_eq!(error, AssociationLaunchError::HandlerNotFound);
        assert_eq!(
            association_launch_error_message(&error),
            "関連付けアプリが見つかりません"
        );
    }

    fn handler(display_name: &str, handler_id: &str) -> AppHandler {
        AppHandler {
            display_name: display_name.to_string(),
            handler_id: handler_id.to_string(),
        }
    }

    #[test]
    fn package_full_name_split_keeps_name_and_publisher_id() {
        assert_eq!(
            split_package_full_name("Microsoft.Paint_11.2605.81.0_x64__8wekyb3d8bbwe"),
            Some(PackageFullNameParts {
                name: "Microsoft.Paint".to_string(),
                publisher_id: "8wekyb3d8bbwe".to_string(),
            })
        );
        assert_eq!(
            split_package_full_name("Microsoft.Paint_11.2605.81.0_x64_8wekyb3d8bbwe"),
            None
        );
        assert_eq!(
            split_package_full_name("Microsoft.Paint_x64__8wekyb3d8bbwe"),
            None
        );
    }

    #[test]
    fn windows_apps_handler_normalization_keeps_the_relative_executable_path() {
        assert_eq!(
            normalize_windows_apps_handler_id(
                r"C:\Program Files\WindowsApps\Microsoft.Paint_11.2605.81.0_x64__8wekyb3d8bbwe\PaintApp\mspaint.exe"
            ),
            Some(WindowsAppsHandlerIdentity {
                package_name: "Microsoft.Paint".to_string(),
                publisher_id: "8wekyb3d8bbwe".to_string(),
                relative_path: r"PaintApp\mspaint.exe".to_string(),
            })
        );
        assert_eq!(
            normalize_windows_apps_handler_id(
                r"C:\Program Files\Microsoft.Paint_11.2605.81.0_x64__8wekyb3d8bbwe\PaintApp\mspaint.exe"
            ),
            None
        );
    }

    #[test]
    fn handler_resolution_uses_exact_then_package_then_display_name() {
        const OLD_PAINT: &str = r"C:\Program Files\WindowsApps\Microsoft.Paint_11.2603.251.0_x64__8wekyb3d8bbwe\PaintApp\mspaint.exe";
        const CURRENT_PAINT: &str = r"C:\Program Files\WindowsApps\Microsoft.Paint_11.2605.81.0_x64__8wekyb3d8bbwe\PaintApp\mspaint.exe";
        let candidates = vec![
            handler("同じ表示名", "DisplayOnly.App"),
            handler("別の表示名", "Exact.App"),
            handler("ペイント", CURRENT_PAINT),
            handler("表示名フォールバック", "Changed.App"),
        ];

        assert_eq!(
            find_association_handler("Exact.App", "同じ表示名", &candidates),
            Ok(AssociationHandlerMatch {
                index: 1,
                needs_writeback: false,
            })
        );
        assert_eq!(
            find_association_handler(OLD_PAINT, "ペイント", &candidates),
            Ok(AssociationHandlerMatch {
                index: 2,
                needs_writeback: true,
            })
        );
        assert_eq!(
            find_association_handler("Removed.App", "表示名フォールバック", &candidates),
            Ok(AssociationHandlerMatch {
                index: 3,
                needs_writeback: true,
            })
        );
        assert_eq!(
            find_association_handler("Removed.App", "見つからない", &candidates),
            Err(AssociationLaunchError::HandlerNotFound)
        );
    }

    #[test]
    fn package_resolution_does_not_match_another_executable_in_the_same_package() {
        const OLD_PAINT: &str = r"C:\Program Files\WindowsApps\Microsoft.Paint_11.2603.251.0_x64__8wekyb3d8bbwe\PaintApp\mspaint.exe";
        const CURRENT_HELPER: &str = r"C:\Program Files\WindowsApps\Microsoft.Paint_11.2605.81.0_x64__8wekyb3d8bbwe\PaintApp\PaintStudio.View.exe";
        let candidates = vec![handler("別のアプリ", CURRENT_HELPER)];

        assert_eq!(
            find_association_handler(OLD_PAINT, "ペイント", &candidates),
            Err(AssociationLaunchError::HandlerNotFound)
        );
    }
}
