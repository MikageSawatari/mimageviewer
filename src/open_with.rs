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
    /// Windows が「おすすめ」に分類しているか (`IAssocHandler::IsRecommended`)。
    ///
    /// 表示順を決めるためだけの UI ヒントで、起動可否とは無関係。**候補を絞る条件に
    /// 使わない**: おすすめは利用者操作で変わる状態なので、これで母集団を切ると、
    /// 保存済みのツールが後から見つからなくなる。
    pub is_recommended: bool,
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
    use windows::Win32::UI::Shell::{ASSOC_FILTER_NONE, SHAssocEnumHandlers};
    use windows::core::PCWSTR;

    let _com = initialize_com_sta()?;

    let ext_wide: Vec<u16> = extension.encode_utf16().chain(std::iter::once(0)).collect();

    // `RECOMMENDED` で絞ると、Windows の「プログラムから開く」には出るアプリが mIV の
    // 一覧には出ない。利用者から見るとそれは mIV の不具合にしか見えない
    // (2026-09-01 利用者指摘)。**起動側の列挙と同じ `NONE` に揃え**、おすすめかどうかは
    // 属性として持って表示順にだけ使う。
    let enum_handlers =
        unsafe { SHAssocEnumHandlers(PCWSTR(ext_wide.as_ptr()), ASSOC_FILTER_NONE)? };

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
                    // `IsRecommended` は `Result` ではなく `HRESULT` を直接返す。
                    // おすすめでないときは **S_FALSE** で、これは失敗ではないので
                    // `is_ok()` では両者を区別できない。S_OK と厳密に比較する。
                    let is_recommended =
                        unsafe { handler.IsRecommended() } == windows::Win32::Foundation::S_OK;
                    result.push(AppHandler {
                        display_name,
                        handler_id,
                        is_recommended,
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

/// 起動経路 4 段をまたぐあいだの、**件ごとの**進み具合。
///
/// 起動できた対象を次の段へ渡すと二重に開き、まとめて成功として返すと失敗した対象が
/// 利用者から見えない (v3.5.0 レビュー F02)。どちらも起きないよう、次段へ渡す対象と
/// 打ち切り時の返し方をここが決める。
#[derive(Debug, Default)]
struct AssociationLaunchProgress {
    launched: Vec<PathBuf>,
    pending: Vec<(PathBuf, String)>,
}

impl AssociationLaunchProgress {
    fn new(file_paths: &[PathBuf]) -> Self {
        Self {
            launched: Vec::new(),
            pending: file_paths
                .iter()
                .map(|path| (path.clone(), String::new()))
                .collect(),
        }
    }

    /// 1 件ずつ叩く段の結果を取り込む。以降の段は失敗した分だけを見る。
    fn record_per_path(&mut self, outcome: ProgIdLaunchOutcome) {
        self.launched.extend(outcome.launched);
        self.pending = outcome.failed;
    }

    fn pending_paths(&self) -> Vec<PathBuf> {
        self.pending.iter().map(|(path, _)| path.clone()).collect()
    }

    fn all_launched(&self) -> bool {
        self.pending.is_empty()
    }

    /// 残りを起動できなかったときの返し方。**全滅なら `Err`、一部でも起動できていれば
    /// 失敗一覧付きの `Ok`。** 一部成功を `Err` にすると、起動済みの一時ファイルまで
    /// 未起動として扱われる。
    fn give_up(
        self,
        refreshed_handler_id: Option<String>,
        error: AssociationLaunchError,
    ) -> Result<AssociationInvokeOutcome, AssociationLaunchError> {
        if self.launched.is_empty() {
            return Err(error);
        }
        let message = association_launch_error_message(&error);
        Ok(AssociationInvokeOutcome {
            refreshed_handler_id,
            failed_paths: self
                .pending
                .into_iter()
                .map(|(path, _)| (path, message.clone()))
                .collect(),
        })
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
    /// 起動できなかった対象と理由。**空なら全件起動できた。**
    ///
    /// 複数ファイルを渡す経路 (`SelectionPolicy::Batch`) では、1 件でも起動できれば
    /// 「全件成功」として扱っていたので、失敗した対象が利用者に見えず、その一時ファイルも
    /// 起動済みとして手放していた (v3.5.0 レビュー F02)。件ごとの結果をここで返す。
    pub failed_paths: Vec<(PathBuf, String)>,
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
/// `invoke_association_handler_inner` と同じ条件 (`ASSOC_FILTER_NONE`、重複除去なし) で
/// 列挙して中身だけ返す調査用。起動側とピッカー側で列挙条件が違っていた件の確認に使う。
#[cfg(all(test, windows))]
pub(crate) fn enumerate_handlers_unfiltered(extension: &str) -> Vec<AppHandler> {
    use windows::Win32::System::Com::CoTaskMemFree;
    use windows::Win32::UI::Shell::{ASSOC_FILTER_NONE, SHAssocEnumHandlers};
    use windows::core::PCWSTR;

    let Ok(_com) = initialize_com_sta() else {
        return Vec::new();
    };
    let wide: Vec<u16> = extension.encode_utf16().chain(std::iter::once(0)).collect();
    let Ok(enum_handlers) =
        (unsafe { SHAssocEnumHandlers(PCWSTR(wide.as_ptr()), ASSOC_FILTER_NONE) })
    else {
        return Vec::new();
    };
    let mut out = Vec::new();
    loop {
        let mut next = [None];
        let mut fetched = 0u32;
        if unsafe { enum_handlers.Next(&mut next, Some(&mut fetched)) }.is_err() || fetched == 0 {
            break;
        }
        let Some(handler) = next[0].take() else {
            continue;
        };
        let handler_id = unsafe { handler.GetName() }.ok().and_then(|value| {
            let text = unsafe { value.to_string() }.ok();
            unsafe { CoTaskMemFree(Some(value.0 as *const _)) };
            text
        });
        let display_name = unsafe { handler.GetUIName() }
            .ok()
            .and_then(|value| {
                let text = unsafe { value.to_string() }.ok();
                unsafe { CoTaskMemFree(Some(value.0 as *const _)) };
                text
            })
            .unwrap_or_default();
        if let Some(handler_id) = handler_id {
            out.push(AppHandler {
                display_name,
                handler_id,
                is_recommended: false,
            });
        }
    }
    out
}

/// ハンドラ名が「そのまま起動できる実行ファイル」なら、そのパスを返す。
///
/// `IAssocHandler::GetName()` は、従来アプリでも Store アプリでも実 exe path を返すことが
/// ある一方 (`...\PaintApp\mspaint.exe`)、ProgID 相当の文字列だけのこともある
/// (`"フォト"` / `"TsubameViewer"`)。前者は shell を通さず直接起動できる。
fn executable_handler_path(handler_id: &str) -> Option<PathBuf> {
    let path = Path::new(handler_id);
    let is_exe = path
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("exe"));
    (is_exe && path.is_file()).then(|| path.to_path_buf())
}

/// 各 `IAssocHandler` を `IObjectWithAppUserModelID` / `IObjectWithProgID` へ QI して
/// 素性を出す調査用。`GetName()` が friendly name しか返さない packaged app でも、
/// ここから AUMID と正確な ProgID を取れるかを実機で確かめるために置く。
#[cfg(all(test, windows))]
pub(crate) fn dump_handler_identities(extension: &str) {
    use windows::Win32::System::Com::CoTaskMemFree;
    use windows::Win32::UI::Shell::{
        ASSOC_FILTER_NONE, IObjectWithAppUserModelID, IObjectWithProgID, SHAssocEnumHandlers,
    };
    use windows::core::{Interface, PCWSTR};

    let Ok(_com) = initialize_com_sta() else {
        return;
    };
    let wide: Vec<u16> = extension.encode_utf16().chain(std::iter::once(0)).collect();
    let Ok(enum_handlers) =
        (unsafe { SHAssocEnumHandlers(PCWSTR(wide.as_ptr()), ASSOC_FILTER_NONE) })
    else {
        return;
    };
    let read = |value: windows::core::PWSTR| {
        let text = unsafe { value.to_string() }.ok();
        unsafe { CoTaskMemFree(Some(value.0 as *const _)) };
        text
    };
    loop {
        let mut next = [None];
        let mut fetched = 0u32;
        if unsafe { enum_handlers.Next(&mut next, Some(&mut fetched)) }.is_err() || fetched == 0 {
            break;
        }
        let Some(handler) = next[0].take() else {
            continue;
        };
        let name = unsafe { handler.GetName() }.ok().and_then(read);
        let ui_name = unsafe { handler.GetUIName() }.ok().and_then(read);
        let aumid = handler
            .cast::<IObjectWithAppUserModelID>()
            .ok()
            .and_then(|object| unsafe { object.GetAppID() }.ok())
            .and_then(read);
        let prog_id = handler
            .cast::<IObjectWithProgID>()
            .ok()
            .and_then(|object| unsafe { object.GetProgID() }.ok())
            .and_then(read);
        println!("ui={ui_name:?} name={name:?} aumid={aumid:?} progid={prog_id:?}");
    }
}

/// パッケージ (Store) アプリを AppUserModelID で起動する。
///
/// `IAssocHandler::Invoke` は packaged app に対して S_OK を返しながら実際には起動せず、
/// shell が「アプリを選択」を出す (2026-08-31 実機、ペイントとフォトの両方)。
/// ハンドラ照合・データオブジェクト・`CreateInvoker`・STA 延命のいずれも原因ではなかった。
///
/// packaged app には専用の起動口がある。ハンドラ自身を `IObjectWithAppUserModelID` へ
/// QI すれば AUMID が取れるので、`IApplicationActivationManager::ActivateForFile` へ
/// `IShellItemArray` ごと渡す。実機の .jpg では packaged app 5 件すべてで AUMID が取れ、
/// 従来 exe のアプリでは 1 件も取れなかったので、これはそのまま両者の判別にもなる。
#[cfg(windows)]
fn activate_packaged_handler(
    handler: &windows::Win32::UI::Shell::IAssocHandler,
    file_paths: &[PathBuf],
) -> Option<bool> {
    use windows::Win32::System::Com::{CLSCTX_LOCAL_SERVER, CoCreateInstance};
    use windows::Win32::UI::Shell::{
        ApplicationActivationManager, IApplicationActivationManager, IObjectWithAppUserModelID,
    };
    use windows::core::{Interface, PCWSTR};

    let app_id = handler
        .cast::<IObjectWithAppUserModelID>()
        .ok()
        .and_then(|object| unsafe { object.GetAppID() }.ok())
        .and_then(|value| {
            let text = unsafe { value.to_string() }.ok();
            unsafe { windows::Win32::System::Com::CoTaskMemFree(Some(value.0 as *const _)) };
            text
        })
        .filter(|app_id| !app_id.is_empty())?;

    crate::logger::log(format!("open_with: packaged handler aumid={app_id:?}"));

    let manager: IApplicationActivationManager =
        match unsafe { CoCreateInstance(&ApplicationActivationManager, None, CLSCTX_LOCAL_SERVER) }
        {
            Ok(manager) => manager,
            Err(error) => {
                crate::logger::log(format!(
                    "open_with: CoCreateInstance(ApplicationActivationManager) failed: {error}"
                ));
                return Some(false);
            }
        };

    let (items, failed_paths) = match crate::file_drag::shell_item_array_for_paths(file_paths) {
        Ok(result) => result,
        Err((failed_paths, error)) => {
            crate::logger::log(format!(
                "open_with: shell item array failed: {error:?} (解決失敗 {failed_paths} 件)"
            ));
            return Some(false);
        }
    };
    if ensure_all_association_paths_resolved(failed_paths).is_err() {
        return Some(false);
    }

    let app_id_wide: Vec<u16> = app_id.encode_utf16().chain(std::iter::once(0)).collect();
    let verb_wide: Vec<u16> = "open".encode_utf16().chain(std::iter::once(0)).collect();
    let result = unsafe {
        manager.ActivateForFile(
            PCWSTR(app_id_wide.as_ptr()),
            &items,
            PCWSTR(verb_wide.as_ptr()),
        )
    };
    crate::logger::log(format!(
        "open_with: ActivateForFile result={:?} files={}",
        result.as_ref().map(|_| ()),
        file_paths.len()
    ));
    Some(result.is_ok())
}

/// 正確な ProgID を指定して `ShellExecuteExW` で開く。
///
/// AUMID 経由 (`ActivateForFile`) が contract 未対応 (0x80270254) を返す packaged app
/// 向けの次善手。ハンドラを `IObjectWithProgID` へ QI すると `AppX...` 形式の正確な
/// ProgID が取れるので、その登録済み `DelegateExecute` / broker を shell に処理させる。
/// `GetName()` の表示名 (`"フォト"`) からは引けないので、QI で取ることが要点。
///
/// worker thread から呼ぶので `SEE_MASK_NOASYNC` は必須。1 ファイルずつしか渡せない。
#[cfg(windows)]
fn shell_execute_with_progid(
    handler: &windows::Win32::UI::Shell::IAssocHandler,
    file_paths: &[PathBuf],
    owner_hwnd: Option<isize>,
) -> Option<ProgIdLaunchOutcome> {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::Shell::{
        IObjectWithProgID, SEE_MASK_CLASSNAME, SEE_MASK_NOASYNC, SHELLEXECUTEINFOW, ShellExecuteExW,
    };
    use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
    use windows::core::{Interface, PCWSTR};

    let prog_id = handler
        .cast::<IObjectWithProgID>()
        .ok()
        .and_then(|object| unsafe { object.GetProgID() }.ok())
        .and_then(|value| {
            let text = unsafe { value.to_string() }.ok();
            unsafe { windows::Win32::System::Com::CoTaskMemFree(Some(value.0 as *const _)) };
            text
        })
        .filter(|prog_id| !prog_id.is_empty())?;

    crate::logger::log(format!(
        "open_with: trying ShellExecuteEx progid={prog_id:?} files={}",
        file_paths.len()
    ));

    let prog_id_wide: Vec<u16> = prog_id.encode_utf16().chain(std::iter::once(0)).collect();
    let verb_wide: Vec<u16> = "open".encode_utf16().chain(std::iter::once(0)).collect();
    let mut outcome = ProgIdLaunchOutcome::default();
    for path in file_paths {
        let file_wide: Vec<u16> = path
            .as_os_str()
            .to_string_lossy()
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();
        let mut info = SHELLEXECUTEINFOW {
            cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
            fMask: SEE_MASK_CLASSNAME | SEE_MASK_NOASYNC,
            hwnd: HWND(owner_hwnd.unwrap_or(0) as *mut core::ffi::c_void),
            lpVerb: PCWSTR(verb_wide.as_ptr()),
            lpFile: PCWSTR(file_wide.as_ptr()),
            lpClass: PCWSTR(prog_id_wide.as_ptr()),
            nShow: SW_SHOWNORMAL.0,
            ..Default::default()
        };
        let result = unsafe { ShellExecuteExW(&mut info) };
        crate::logger::log(format!(
            "open_with: ShellExecuteEx progid result={:?} file={:?}",
            result.as_ref().map(|_| ()),
            path
        ));
        match result {
            Ok(()) => outcome.launched.push(path.clone()),
            Err(error) => outcome
                .failed
                .push((path.clone(), format!("ShellExecuteEx: {error}"))),
        }
    }
    Some(outcome)
}

/// ProgID 経路の**件ごとの**結果。
#[derive(Default)]
struct ProgIdLaunchOutcome {
    launched: Vec<PathBuf>,
    failed: Vec<(PathBuf, String)>,
}

// Windows 専用。呼ぶのは同名の `#[cfg(windows)]` ラッパーだけで、非 Windows 側の
// ラッパーはここへ来ない。gate が無いと `windows` crate も
// `file_drag::shell_data_object_for_paths` も非 Windows で解決できず、CI の
// ubuntu チェックだけが落ちる (Windows ローカルでは cfg(windows) が常に真のため)。
#[cfg(windows)]
fn invoke_association_handler_inner(
    extension: &str,
    expected_id: &str,
    expected_display_name: &str,
    file_paths: &[PathBuf],
    owner_hwnd: Option<isize>,
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
                // 起動側は候補の並び替えをしないので、この値は使わない。
                is_recommended: false,
            });
            handlers.push(handler);
        }
    }

    let matched = find_association_handler(expected_id, expected_display_name, &candidates)?;
    // 起動したのにアプリが出ず OS の「アプリを選択」が出る、という報告があった
    // (2026-08-31)。Invoke は Ok を返すので失敗ログに出ず、どのハンドラを掴んだのか
    // 外から見えなかった。どれに当てたかと Invoke の結果を残す。
    crate::logger::log(format!(
        "open_with: matched handler index={} id={:?} ui_name={:?} writeback={} (expected id={:?} name={:?}) candidates={}",
        matched.index,
        candidates[matched.index].handler_id,
        candidates[matched.index].display_name,
        matched.needs_writeback,
        expected_id,
        expected_display_name,
        candidates.len(),
    ));
    let refreshed_handler_id = matched
        .needs_writeback
        .then(|| candidates[matched.index].handler_id.clone());
    // 起動経路は 4 段。**どの段も、失敗したら次を試す**。試した経路と結果はログに残し、
    // 全部駄目だったときだけ利用者へ理由を返す。
    //
    // 順番は実測で決めた (2026-09-01、実機ログ)。
    // - ProgID + ShellExecuteEx はペイント / フォトの両方を 75〜95ms で起動できた
    // - AUMID + ActivateForFile は同じ 2 つとも 0x80270254 (contract 未対応) で、
    //   しかも失敗までに 1.0〜1.6 秒かかった。先に置くと毎回その分待たされる
    // - ProgID 経路は shell に登録済みの verb / DelegateExecute を処理させるので、
    //   exe を直接 spawn するより素性が良い (必要な引数や DDE を飛ばさない)
    //
    // **段をまたぐのは「まだ起動していない対象」だけ。** ProgID 経路は 1 件ずつ叩くので
    // 一部だけ失敗し得る。全体を再投入すると成功済みが二重に開き、全体を成功として返すと
    // 失敗した対象が利用者から見えない (v3.5.0 レビュー F02)。
    let mut progress = AssociationLaunchProgress::new(file_paths);
    let mut pending: Vec<PathBuf> = file_paths.to_vec();

    macro_rules! give_up {
        ($error:expr) => {{
            return progress.give_up(refreshed_handler_id, $error);
        }};
    }

    // 1. 正確な ProgID があれば shell に処理させる。`GetName()` の表示名 ("フォト") では
    //    引けないので、ハンドラを `IObjectWithProgID` へ QI して取る。
    if let Some(outcome) = shell_execute_with_progid(&handlers[matched.index], &pending, owner_hwnd)
    {
        progress.record_per_path(outcome);
        pending = progress.pending_paths();
        if progress.all_launched() {
            return Ok(AssociationInvokeOutcome {
                refreshed_handler_id,
                failed_paths: Vec::new(),
            });
        }
    }

    // 2. ハンドラ名が実在する .exe なら直接起動する。ハンドラ名は毎回列挙し直すので、
    //    Store アプリの更新でパスの版番号が変わっても追随する (`Executable` 保存とは違う)。
    if let Some(executable) = executable_handler_path(&candidates[matched.index].handler_id) {
        crate::logger::log(format!(
            "open_with: launching handler executable directly path={executable:?} files={}",
            pending.len()
        ));
        let mut command = std::process::Command::new(&executable);
        command
            .args(&pending)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        if let Err(error) = command.spawn() {
            give_up!(AssociationLaunchError::Shell(format!(
                "関連付けアプリを起動できません: {error}"
            )));
        }
        return Ok(AssociationInvokeOutcome {
            refreshed_handler_id,
            failed_paths: Vec::new(),
        });
    }

    // 3. packaged app 専用の起動口。実機の 2 つでは効かなかったが、file activation
    //    contract を要求する UWP handler にはこちらが正道 (docs)。ここまで落ちるのは
    //    ProgID も exe path も無い packaged app だけなので、待ち時間は普段は発生しない。
    if activate_packaged_handler(&handlers[matched.index], &pending).is_some_and(|ok| ok) {
        return Ok(AssociationInvokeOutcome {
            refreshed_handler_id,
            failed_paths: Vec::new(),
        });
    }

    // 4. 従来の経路。packaged app では S_OK を返しながら起動しないことが分かっている
    //    (2026-08-31 実機) が、従来アプリではこれで足りる。データオブジェクトは**この段でしか
    //    使わない**ので、ここまで来たときにだけ、残っている対象で組む。
    let (data, unresolved) = match crate::file_drag::shell_data_object_for_paths(&pending) {
        Ok(value) => value,
        Err((failed_paths, error)) => {
            give_up!(AssociationLaunchError::Shell(format!(
                "対象ファイルを Shell に渡せません: {error:?} (解決失敗 {failed_paths} 件)"
            )));
        }
    };
    if let Err(error) = ensure_all_association_paths_resolved(unresolved) {
        give_up!(error);
    }
    let invoke_result = unsafe { handlers[matched.index].Invoke(&data) };
    crate::logger::log(format!(
        "open_with: Invoke result={:?} files={}",
        invoke_result,
        pending.len()
    ));
    if let Err(error) = invoke_result {
        give_up!(AssociationLaunchError::Shell(format!(
            "関連付けアプリを起動できません: {error}"
        )));
    }
    Ok(AssociationInvokeOutcome {
        refreshed_handler_id,
        failed_paths: Vec::new(),
    })
}

#[cfg(windows)]
pub(crate) fn invoke_association_handler_for_paths(
    handler_id: &str,
    display_name: &str,
    file_paths: &[PathBuf],
    owner_hwnd: Option<isize>,
) -> Result<AssociationInvokeOutcome, String> {
    let extension = association_extension_for_paths(file_paths)
        .map_err(|error| association_launch_error_message(&error))?;
    invoke_association_handler_inner(&extension, handler_id, display_name, file_paths, owner_hwnd)
        .map_err(|error| association_launch_error_message(&error))
}

#[cfg(not(windows))]
pub(crate) fn invoke_association_handler_for_paths(
    _handler_id: &str,
    _display_name: &str,
    file_paths: &[PathBuf],
    _owner_hwnd: Option<isize>,
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
    owner_hwnd: Option<isize>,
) -> Result<AssociationInvokeOutcome, String> {
    invoke_association_handler_for_paths(
        handler_id,
        display_name,
        &[file_path.to_path_buf()],
        owner_hwnd,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 1 件ずつ叩く段で一部だけ失敗したら、**次の段へ回すのは失敗した分だけ**。
    ///
    /// 全体を再投入すると成功済みが二重に開く (F02)。
    #[test]
    fn only_the_targets_that_did_not_launch_go_on_to_the_next_stage() {
        let paths = vec![
            PathBuf::from(r"C:.png"),
            PathBuf::from(r"C:.png"),
            PathBuf::from(r"C:\c.png"),
        ];
        let mut progress = AssociationLaunchProgress::new(&paths);

        progress.record_per_path(ProgIdLaunchOutcome {
            launched: vec![paths[0].clone(), paths[2].clone()],
            failed: vec![(paths[1].clone(), "ShellExecuteEx: E_FAIL".to_string())],
        });

        assert!(!progress.all_launched());
        assert_eq!(progress.pending_paths(), vec![paths[1].clone()]);
    }

    /// 全件起動できたら、その先の段には進まない。
    #[test]
    fn a_fully_launched_batch_reports_no_failures() {
        let paths = vec![PathBuf::from(r"C:.png"), PathBuf::from(r"C:.png")];
        let mut progress = AssociationLaunchProgress::new(&paths);

        progress.record_per_path(ProgIdLaunchOutcome {
            launched: paths.clone(),
            failed: Vec::new(),
        });

        assert!(progress.all_launched());
        assert!(progress.pending_paths().is_empty());
    }

    /// 一部でも起動できていたら、残りは**失敗一覧付きの成功**として返す。
    ///
    /// ここを `Err` にすると、起動済みの対象まで未起動として扱われ、その一時ファイルが
    /// 掃除対象へ戻ってしまう。
    #[test]
    fn a_partly_launched_batch_returns_the_failures_instead_of_failing_outright() {
        let paths = vec![PathBuf::from(r"C:.png"), PathBuf::from(r"C:.png")];
        let mut progress = AssociationLaunchProgress::new(&paths);
        progress.record_per_path(ProgIdLaunchOutcome {
            launched: vec![paths[0].clone()],
            failed: vec![(paths[1].clone(), "ShellExecuteEx: E_FAIL".to_string())],
        });

        let outcome = progress
            .give_up(
                None,
                AssociationLaunchError::Shell("関連付けアプリを起動できません".to_string()),
            )
            .expect("一部でも起動できていれば全体は失敗にしない");

        assert_eq!(outcome.failed_paths.len(), 1);
        assert_eq!(outcome.failed_paths[0].0, paths[1]);
        assert!(!outcome.failed_paths[0].1.is_empty(), "理由を空にしない");
    }

    /// 1 件も起動できなければ、従来どおり全体の失敗。
    #[test]
    fn a_batch_that_launched_nothing_still_fails_as_a_whole() {
        let paths = vec![PathBuf::from(r"C:.png")];
        let mut progress = AssociationLaunchProgress::new(&paths);
        progress.record_per_path(ProgIdLaunchOutcome {
            launched: Vec::new(),
            failed: vec![(paths[0].clone(), "ShellExecuteEx: E_FAIL".to_string())],
        });

        let error = progress.give_up(None, AssociationLaunchError::HandlerNotFound);

        assert_eq!(error, Err(AssociationLaunchError::HandlerNotFound));
    }

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
            is_recommended: false,
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

#[cfg(all(test, windows))]
mod handler_dump_tests {
    /// 実機の関連付けハンドラを目視するための調査用テスト。
    /// `cargo test -p mimageviewer --lib handler_dump -- --ignored --nocapture` で走らせる。
    /// 環境依存なので通常のテスト実行からは外す。
    #[test]
    #[ignore]
    fn dump_handlers_for_jpg() {
        println!("--- picker view (canonical enumeration) ---");
        for handler in super::enumerate_handlers(".jpg") {
            println!(
                "recommended={} UIName={:?} | Name={:?}",
                handler.is_recommended, handler.display_name, handler.handler_id
            );
        }
        println!("--- identity QI (AUMID / ProgID) ---");
        super::dump_handler_identities(".jpg");
        println!("--- invoke view (NONE, raw order) ---");
        for (index, handler) in super::enumerate_handlers_unfiltered(".jpg")
            .into_iter()
            .enumerate()
        {
            println!(
                "[{index}] UIName={:?} | Name={:?}",
                handler.display_name, handler.handler_id
            );
        }
    }
}
