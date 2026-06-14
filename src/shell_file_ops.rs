//! Windows Shell-backed file operations that need mIV-owned UI.

use std::path::PathBuf;
use std::sync::mpsc;

#[derive(Debug, Clone)]
pub struct ShellRenameOutcome {
    pub target: PathBuf,
    pub new_path: PathBuf,
    pub aborted: bool,
}

pub type ShellRenameResult = Result<ShellRenameOutcome, String>;

pub fn rename_item_async(
    hwnd: Option<isize>,
    target: PathBuf,
    new_name: String,
) -> mpsc::Receiver<ShellRenameResult> {
    let (tx, rx) = mpsc::channel();
    let spawn_target = target.clone();
    let spawn_name = new_name.clone();
    let tx_on_spawn_error = tx.clone();
    let spawn_result = std::thread::Builder::new()
        .name("shell-rename-worker".into())
        .spawn(move || {
            let result = run_rename_item(hwnd.unwrap_or_default(), spawn_target, spawn_name);
            let _ = tx.send(result);
        });
    if let Err(e) = spawn_result {
        let _ = tx_on_spawn_error.send(Err(format!("名前変更 worker を開始できません: {e}")));
    }
    rx
}

#[cfg(windows)]
fn run_rename_item(hwnd: isize, target: PathBuf, new_name: String) -> ShellRenameResult {
    use windows::Win32::Foundation::{HWND, RPC_E_CHANGED_MODE};
    use windows::Win32::System::Com::{
        CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx,
        CoUninitialize,
    };
    use windows::Win32::UI::Shell::{
        FOF_ALLOWUNDO, FOFX_ADDUNDORECORD, FileOperation, IFileOperation,
        IFileOperationProgressSink, IShellItem, SHCreateItemFromParsingName,
    };
    use windows::core::{IUnknown, PCWSTR};

    struct ComStaGuard {
        uninitialize: bool,
    }

    impl ComStaGuard {
        fn new() -> Result<Self, String> {
            let hr = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
            if hr.is_ok() {
                Ok(Self { uninitialize: true })
            } else if hr == RPC_E_CHANGED_MODE {
                Ok(Self {
                    uninitialize: false,
                })
            } else {
                Err(format!("CoInitializeEx(STA) failed: 0x{:08x}", hr.0))
            }
        }
    }

    impl Drop for ComStaGuard {
        fn drop(&mut self) {
            if self.uninitialize {
                unsafe { CoUninitialize() };
            }
        }
    }

    let _com = ComStaGuard::new()?;
    let target_w = wide_null_path(&target);
    let new_name_w = wide_null_str(&new_name);
    let item: IShellItem = unsafe {
        SHCreateItemFromParsingName(
            PCWSTR(target_w.as_ptr()),
            None::<&windows::Win32::System::Com::IBindCtx>,
        )
    }
    .map_err(|e| format!("対象を開けません: {e}"))?;
    let op: IFileOperation =
        unsafe { CoCreateInstance(&FileOperation, None::<&IUnknown>, CLSCTX_INPROC_SERVER) }
            .map_err(|e| format!("IFileOperation を作成できません: {e}"))?;

    if hwnd != 0 {
        unsafe { op.SetOwnerWindow(HWND(hwnd as *mut core::ffi::c_void)) }
            .map_err(|e| format!("Shell 操作の owner window を設定できません: {e}"))?;
    }
    unsafe { op.SetOperationFlags(FOF_ALLOWUNDO | FOFX_ADDUNDORECORD) }
        .map_err(|e| format!("Shell 操作フラグを設定できません: {e}"))?;
    unsafe {
        op.RenameItem(
            &item,
            PCWSTR(new_name_w.as_ptr()),
            None::<&IFileOperationProgressSink>,
        )
    }
    .map_err(|e| format!("名前変更を予約できません: {e}"))?;
    unsafe { op.PerformOperations() }.map_err(|e| format!("名前変更に失敗しました: {e}"))?;
    let aborted = unsafe { op.GetAnyOperationsAborted() }
        .map(|v| v.as_bool())
        .unwrap_or(false);

    let new_path = target
        .parent()
        .map(|parent| parent.join(&new_name))
        .unwrap_or_else(|| PathBuf::from(&new_name));
    if !aborted {
        let new_path_exists = new_path.try_exists().map_err(|e| {
            format!(
                "名前変更後の項目を確認できません: {} ({e})",
                new_path.display()
            )
        })?;
        if !new_path_exists {
            return Err(format!(
                "名前変更後の項目を確認できません: {}",
                new_path.display()
            ));
        }
        let target_still_exists = target.try_exists().map_err(|e| {
            format!(
                "名前変更前の項目を確認できません: {} ({e})",
                target.display()
            )
        })?;
        if target_still_exists && !crate::folder_tree::path_eq(&target, &new_path) {
            return Err(format!(
                "名前変更が完了していない可能性があります: {}",
                target.display()
            ));
        }
    }
    Ok(ShellRenameOutcome {
        target,
        new_path,
        aborted,
    })
}

#[cfg(not(windows))]
fn run_rename_item(_hwnd: isize, target: PathBuf, new_name: String) -> ShellRenameResult {
    let _ = (target, new_name);
    Err("名前変更は Windows Shell 経由でのみ利用できます".to_string())
}

#[cfg(windows)]
fn wide_null_path(path: &std::path::Path) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;

    path.as_os_str()
        .encode_wide()
        .map(|ch| if ch == b'/' as u16 { b'\\' as u16 } else { ch })
        .chain(std::iter::once(0))
        .collect()
}

#[cfg(windows)]
fn wide_null_str(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}
