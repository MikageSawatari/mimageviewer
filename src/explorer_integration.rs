//! Windows Explorer integration helpers.
//!
//! The SendTo entry is a per-user `.lnk` shortcut stored in the shell SendTo
//! known folder.  Windows appends selected files to the shortcut target as
//! positional arguments, which then reuse the normal startup-open path.

use std::path::{Path, PathBuf};

pub const LAUNCHER_EXE_ENV_VAR: &str = "MIV_LAUNCHER_EXE_PATH";
pub const SEND_TO_SHORTCUT_NAME: &str = "mImageViewer.lnk";

#[derive(Clone, Debug)]
pub struct SendToShortcutStatus {
    pub send_to_dir: PathBuf,
    pub shortcut_path: PathBuf,
    pub expected_target: PathBuf,
    pub registered: bool,
    pub target: Option<PathBuf>,
    pub target_matches: bool,
}

pub fn app_executable_path() -> Result<PathBuf, String> {
    if let Some(path) = std::env::var_os(LAUNCHER_EXE_ENV_VAR)
        .filter(|p| !p.is_empty())
        .map(PathBuf::from)
    {
        return Ok(path);
    }
    std::env::current_exe().map_err(|e| format!("実行ファイルのパスを取得できませんでした: {e}"))
}

#[cfg(windows)]
pub fn send_to_shortcut_status() -> Result<SendToShortcutStatus, String> {
    let send_to_dir = send_to_dir()?;
    let shortcut_path = send_to_dir.join(SEND_TO_SHORTCUT_NAME);
    let expected_target = app_executable_path()?;
    let registered = shortcut_path.is_file();
    let target = if registered {
        read_shortcut_target(&shortcut_path).ok()
    } else {
        None
    };
    let target_matches = target
        .as_deref()
        .is_some_and(|target| path_eq(target, &expected_target));
    Ok(SendToShortcutStatus {
        send_to_dir,
        shortcut_path,
        expected_target,
        registered,
        target,
        target_matches,
    })
}

#[cfg(not(windows))]
pub fn send_to_shortcut_status() -> Result<SendToShortcutStatus, String> {
    Err("SendTo 連携は Windows 専用です。".to_string())
}

#[cfg(windows)]
pub fn register_send_to_shortcut() -> Result<SendToShortcutStatus, String> {
    let send_to_dir = send_to_dir()?;
    let shortcut_path = send_to_dir.join(SEND_TO_SHORTCUT_NAME);
    let target = app_executable_path()?;
    if !target.is_file() {
        return Err(format!(
            "登録先の実行ファイルが見つかりません: {}",
            target.display()
        ));
    }
    std::fs::create_dir_all(&send_to_dir)
        .map_err(|e| format!("SendTo フォルダを作成できませんでした: {e}"))?;
    create_shortcut(&shortcut_path, &target)?;
    send_to_shortcut_status()
}

#[cfg(not(windows))]
pub fn register_send_to_shortcut() -> Result<SendToShortcutStatus, String> {
    Err("SendTo 連携は Windows 専用です。".to_string())
}

pub fn unregister_send_to_shortcut() -> Result<SendToShortcutStatus, String> {
    let shortcut_path = send_to_shortcut_path()?;
    match std::fs::remove_file(&shortcut_path) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => {
            return Err(format!(
                "SendTo ショートカットを削除できませんでした: {} ({e})",
                shortcut_path.display()
            ));
        }
    }
    send_to_shortcut_status()
}

pub fn send_to_shortcut_path() -> Result<PathBuf, String> {
    Ok(send_to_dir()?.join(SEND_TO_SHORTCUT_NAME))
}

#[cfg(windows)]
pub fn send_to_dir() -> Result<PathBuf, String> {
    use windows::Win32::System::Com::CoTaskMemFree;
    use windows::Win32::UI::Shell::{FOLDERID_SendTo, KF_FLAG_DEFAULT, SHGetKnownFolderPath};

    unsafe {
        let pwstr = SHGetKnownFolderPath(&FOLDERID_SendTo, KF_FLAG_DEFAULT, None)
            .map_err(|e| format!("SendTo フォルダを取得できませんでした: {e:?}"))?;
        let path = pwstr
            .to_string()
            .map(PathBuf::from)
            .map_err(|e| format!("SendTo フォルダのパス変換に失敗しました: {e:?}"));
        CoTaskMemFree(Some(pwstr.0 as *const core::ffi::c_void));
        path
    }
}

#[cfg(not(windows))]
pub fn send_to_dir() -> Result<PathBuf, String> {
    Err("SendTo 連携は Windows 専用です。".to_string())
}

#[cfg(windows)]
fn create_shortcut(shortcut_path: &Path, target: &Path) -> Result<(), String> {
    use windows::Win32::System::Com::{
        CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, IPersistFile,
    };
    use windows::Win32::UI::Shell::{IShellLinkW, ShellLink};
    use windows::core::{Interface, PCWSTR};

    let _com = ComInitScope::init(COINIT_APARTMENTTHREADED);
    let link: IShellLinkW = unsafe { CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER) }
        .map_err(|e| {
            format!("ショートカット作成用 COM オブジェクトを作成できませんでした: {e:?}")
        })?;

    let target_wide = wide_nul(target);
    unsafe {
        link.SetPath(PCWSTR(target_wide.as_ptr()))
            .map_err(|e| format!("ショートカットの実行ファイルを設定できませんでした: {e:?}"))?;
        link.SetArguments(PCWSTR(wide_nul_str("").as_ptr()))
            .map_err(|e| format!("ショートカット引数を設定できませんでした: {e:?}"))?;
        link.SetDescription(PCWSTR(wide_nul_str("mImageViewer で開く").as_ptr()))
            .map_err(|e| format!("ショートカットの説明を設定できませんでした: {e:?}"))?;
        link.SetIconLocation(PCWSTR(target_wide.as_ptr()), 0)
            .map_err(|e| format!("ショートカットのアイコンを設定できませんでした: {e:?}"))?;
        if let Some(parent) = target.parent() {
            let working_dir = wide_nul(parent);
            link.SetWorkingDirectory(PCWSTR(working_dir.as_ptr()))
                .map_err(|e| format!("作業フォルダを設定できませんでした: {e:?}"))?;
        }
    }

    let persist: IPersistFile = link
        .cast()
        .map_err(|e| format!("IPersistFile を取得できませんでした: {e:?}"))?;
    let shortcut_wide = wide_nul(shortcut_path);
    unsafe {
        persist
            .Save(PCWSTR(shortcut_wide.as_ptr()), true)
            .map_err(|e| format!("ショートカットを保存できませんでした: {e:?}"))?;
    }
    Ok(())
}

#[cfg(windows)]
fn read_shortcut_target(shortcut_path: &Path) -> Result<PathBuf, String> {
    use windows::Win32::Storage::FileSystem::WIN32_FIND_DATAW;
    use windows::Win32::System::Com::{
        CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, IPersistFile, STGM_READ,
    };
    use windows::Win32::UI::Shell::{IShellLinkW, ShellLink};
    use windows::core::{Interface, PCWSTR};

    let _com = ComInitScope::init(COINIT_APARTMENTTHREADED);
    let link: IShellLinkW = unsafe { CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER) }
        .map_err(|e| {
            format!("ショートカット読み取り用 COM オブジェクトを作成できませんでした: {e:?}")
        })?;
    let persist: IPersistFile = link
        .cast()
        .map_err(|e| format!("IPersistFile を取得できませんでした: {e:?}"))?;
    let shortcut_wide = wide_nul(shortcut_path);
    unsafe {
        persist
            .Load(PCWSTR(shortcut_wide.as_ptr()), STGM_READ)
            .map_err(|e| format!("ショートカットを読み込めませんでした: {e:?}"))?;
    }

    let mut buf = vec![0_u16; 32_768];
    let mut find_data = WIN32_FIND_DATAW::default();
    unsafe {
        link.GetPath(&mut buf, &mut find_data, 0)
            .map_err(|e| format!("ショートカットのリンク先を取得できませんでした: {e:?}"))?;
    }
    let len = buf.iter().position(|&ch| ch == 0).unwrap_or(buf.len());
    if len == 0 {
        return Err("ショートカットのリンク先が空です。".to_string());
    }
    Ok(PathBuf::from(String::from_utf16_lossy(&buf[..len])))
}

#[cfg(windows)]
struct ComInitScope {
    needs_uninit: bool,
}

#[cfg(windows)]
impl ComInitScope {
    fn init(coinit: windows::Win32::System::Com::COINIT) -> Self {
        use windows::Win32::Foundation::S_OK;
        use windows::Win32::System::Com::CoInitializeEx;

        let hr = unsafe { CoInitializeEx(None, coinit) };
        Self {
            needs_uninit: hr == S_OK,
        }
    }
}

#[cfg(windows)]
impl Drop for ComInitScope {
    fn drop(&mut self) {
        if self.needs_uninit {
            unsafe {
                windows::Win32::System::Com::CoUninitialize();
            }
        }
    }
}

#[cfg(windows)]
fn wide_nul(path: &Path) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;

    path.as_os_str().encode_wide().chain([0]).collect()
}

#[cfg(windows)]
fn wide_nul_str(s: &str) -> Vec<u16> {
    s.encode_utf16().chain([0]).collect()
}

#[cfg(windows)]
fn path_eq(a: &Path, b: &Path) -> bool {
    a.to_string_lossy()
        .replace('/', "\\")
        .eq_ignore_ascii_case(&b.to_string_lossy().replace('/', "\\"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn send_to_shortcut_name_is_lnk() {
        assert_eq!(SEND_TO_SHORTCUT_NAME, "mImageViewer.lnk");
    }
}
