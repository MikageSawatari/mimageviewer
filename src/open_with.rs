/// 拡張子に関連付けられたアプリケーションの列挙と起動。
///
/// Windows Shell API (`SHAssocEnumHandlers`) を使用して、
/// ファイル拡張子に対応するアプリ一覧を取得する。

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
    MissingExtension,
    NonUnicodeExtension,
    HandlerNotFound,
    Shell(String),
}

fn association_extension(path: &std::path::Path) -> Result<String, AssociationLaunchError> {
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

fn find_association_handler_index(
    expected_id: &str,
    candidate_ids: &[String],
) -> Result<usize, AssociationLaunchError> {
    candidate_ids
        .iter()
        .position(|candidate| candidate == expected_id)
        .ok_or(AssociationLaunchError::HandlerNotFound)
}

fn association_launch_error_message(error: &AssociationLaunchError) -> String {
    match error {
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

/// 保存済み handler ID を対象拡張子で引き直し、Shell に単一ファイルを起動させる。
///
/// Association 固有の COM ポインタと `IAssocHandler::Invoke` はこの関数から外へ出さない。
/// 呼び出し元は external-tool launch worker なので、列挙・PIDL 解決・Invoke は UI thread
/// では実行されない。
#[cfg(windows)]
fn invoke_association_handler_inner(
    extension: &str,
    expected_id: &str,
    file_path: &std::path::Path,
) -> Result<(), AssociationLaunchError> {
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
    let mut candidate_ids = Vec::new();
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
            candidate_ids.push(handler_id);
            handlers.push(handler);
        }
    }

    let index = find_association_handler_index(expected_id, &candidate_ids)?;
    let (data, failed_paths) =
        crate::file_drag::shell_data_object_for_paths(&[file_path.to_path_buf()]).map_err(
            |(failed_paths, error)| {
                AssociationLaunchError::Shell(format!(
                    "対象ファイルを Shell に渡せません: {error:?} (解決失敗 {failed_paths} 件)"
                ))
            },
        )?;
    if failed_paths != 0 {
        return Err(AssociationLaunchError::Shell(format!(
            "対象ファイルを Shell に渡せません (解決失敗 {failed_paths} 件)"
        )));
    }
    unsafe { handlers[index].Invoke(&data) }.map_err(|error| {
        AssociationLaunchError::Shell(format!("関連付けアプリを起動できません: {error}"))
    })
}

#[cfg(windows)]
pub(crate) fn invoke_association_handler(
    handler_id: &str,
    file_path: &std::path::Path,
) -> Result<(), String> {
    let extension = association_extension(file_path)
        .map_err(|error| association_launch_error_message(&error))?;
    invoke_association_handler_inner(&extension, handler_id, file_path)
        .map_err(|error| association_launch_error_message(&error))
}

#[cfg(not(windows))]
pub(crate) fn invoke_association_handler(
    _handler_id: &str,
    _file_path: &std::path::Path,
) -> Result<(), String> {
    Err("関連付けアプリの起動は Windows でのみ利用できます".to_string())
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
    fn handler_id_selection_reports_a_user_visible_not_found_error() {
        let candidates = vec!["Photos.App".to_string(), "Paint.App".to_string()];
        assert_eq!(
            find_association_handler_index("Paint.App", &candidates),
            Ok(1)
        );
        let error = find_association_handler_index("Removed.App", &candidates).unwrap_err();
        assert_eq!(error, AssociationLaunchError::HandlerNotFound);
        assert_eq!(
            association_launch_error_message(&error),
            "関連付けアプリが見つかりません"
        );
    }
}
