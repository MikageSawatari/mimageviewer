/// 拡張子に関連付けられたアプリケーションの列挙と起動。
///
/// Windows Shell API (`SHAssocEnumHandlers`) を使用して、
/// ファイル拡張子に対応するアプリ一覧を取得する。

/// アプリケーションハンドラ情報
#[derive(Clone, Debug)]
pub struct AppHandler {
    pub display_name: String,
    pub exe_path: String,
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
fn enumerate_handlers_inner(extension: &str) -> Result<Vec<AppHandler>, Box<dyn std::error::Error>> {
    use windows::Win32::UI::Shell::{SHAssocEnumHandlers, ASSOC_FILTER_RECOMMENDED};
    use windows::Win32::System::Com::{CoInitializeEx, CoTaskMemFree, COINIT_APARTMENTTHREADED};
    use windows::core::PCWSTR;

    // COM 初期化（既に初期化済みなら S_FALSE が返るだけで問題ない）
    let _ = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };

    let ext_wide: Vec<u16> = extension.encode_utf16().chain(std::iter::once(0)).collect();

    let enum_handlers = unsafe {
        SHAssocEnumHandlers(PCWSTR(ext_wide.as_ptr()), ASSOC_FILTER_RECOMMENDED)?
    };

    let mut result = Vec::new();
    let mut seen_paths = std::collections::HashSet::new();

    loop {
        let mut handlers: [Option<_>; 1] = [None];
        let mut fetched: u32 = 0;
        let hr = unsafe { enum_handlers.Next(&mut handlers, Some(&mut fetched)) };
        if hr.is_err() || fetched == 0 {
            break;
        }
        if let Some(handler) = handlers[0].take() {
            let ui_name = unsafe { handler.GetUIName() };
            let name = unsafe { handler.GetName() };

            if let (Ok(ui_name_pwstr), Ok(name_pwstr)) = (ui_name, name) {
                let display_name = unsafe { ui_name_pwstr.to_string() };
                let exe_path = unsafe { name_pwstr.to_string() };

                // PWSTR を解放
                unsafe {
                    CoTaskMemFree(Some(ui_name_pwstr.0 as *const _));
                    CoTaskMemFree(Some(name_pwstr.0 as *const _));
                }

                if let (Ok(display), Ok(exe)) = (display_name, exe_path) {
                    let key = exe.to_lowercase();
                    if seen_paths.insert(key) {
                        result.push(AppHandler {
                            display_name: display,
                            exe_path: exe,
                        });
                    }
                }
            }
        }
    }

    Ok(result)
}

/// ファイル選択ダイアログで .exe を選ばせ、(表示名, exeパス) を返す。
/// キャンセル時は None。
#[cfg(windows)]
pub fn pick_exe_dialog() -> Option<AppHandler> {
    use windows::Win32::UI::Controls::Dialogs::{
        GetOpenFileNameW, OPENFILENAMEW, OFN_FILEMUSTEXIST, OFN_PATHMUSTEXIST, OFN_NOCHANGEDIR,
    };
    use windows::core::PCWSTR;

    // フィルタ文字列: "実行ファイル (*.exe)\0*.exe\0\0"
    let filter: Vec<u16> = "実行ファイル (*.exe)\0*.exe\0\0"
        .encode_utf16()
        .collect();

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
        &file_buf[..file_buf.iter().position(|&c| c == 0).unwrap_or(file_buf.len())]
    );
    let path = std::path::Path::new(&path_str);

    // 表示名: exe のファイル名からステムを取得
    let display_name = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("Unknown")
        .to_string();

    Some(AppHandler {
        display_name,
        exe_path: path_str,
    })
}

#[cfg(not(windows))]
pub fn pick_exe_dialog() -> Option<AppHandler> {
    None
}

/// 指定したアプリケーションでファイルを開く。
pub fn launch_with_app(exe_path: &str, file_path: &std::path::Path) {
    let mut cmd = std::process::Command::new(exe_path);
    cmd.arg(file_path.as_os_str());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x08000000); // CREATE_NO_WINDOW
    }
    let _ = cmd.spawn();
}
