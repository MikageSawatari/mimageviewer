//! Windows Shell-backed file operations that need mIV-owned UI.

use std::path::PathBuf;
use std::sync::mpsc;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShellTransferOperation {
    Copy,
    Move,
}

#[derive(Debug, Clone)]
pub struct ShellTransferRequest {
    pub operation: ShellTransferOperation,
    pub sources: Vec<PathBuf>,
    pub destination: PathBuf,
}

#[derive(Debug, Clone, Copy)]
pub struct ShellTransferOutcome {
    pub aborted: bool,
}

pub type ShellTransferResult = Result<ShellTransferOutcome, String>;

/// The worker owns validation and all Shell calls; the UI only polls the terminal result.
pub fn transfer_items_async(
    hwnd: Option<isize>,
    request: ShellTransferRequest,
    wake_ctx: egui::Context,
) -> mpsc::Receiver<ShellTransferResult> {
    let (tx, rx) = mpsc::channel();
    let tx_on_spawn_error = tx.clone();
    let wake_on_spawn_error = wake_ctx.clone();
    let spawn_result = std::thread::Builder::new()
        .name("shell-transfer-worker".into())
        .spawn(move || {
            let result = validate_transfer_request(request)
                .and_then(|request| run_transfer_items(hwnd.unwrap_or_default(), request));
            let _ = tx.send(result);
            wake_ctx.request_repaint();
        });
    if let Err(e) = spawn_result {
        let _ = tx_on_spawn_error.send(Err(format!("ファイル整理 worker を開始できません: {e}")));
        wake_on_spawn_error.request_repaint();
    }
    rx
}

fn path_has_nul(path: &std::path::Path) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        path.as_os_str().encode_wide().any(|ch| ch == 0)
    }
    #[cfg(not(windows))]
    {
        path.as_os_str().as_encoded_bytes().contains(&0)
    }
}

// Call only with canonical paths. Component comparison avoids confusing siblings
// such as "photos" and "photos-old", and keeps drive/UNC prefixes in the comparison.
fn canonical_path_components(path: &std::path::Path) -> Vec<String> {
    path.components()
        .map(|component| component.as_os_str().to_string_lossy().to_lowercase())
        .collect()
}

fn validate_transfer_request(
    request: ShellTransferRequest,
) -> Result<ShellTransferRequest, String> {
    if request.sources.is_empty() {
        return Err("整理する対象がありません".into());
    }
    if !request.destination.is_absolute() || path_has_nul(&request.destination) {
        return Err("整理先には NUL を含まない絶対パスを指定してください".into());
    }
    let destination = std::fs::canonicalize(&request.destination).map_err(|e| {
        format!(
            "整理先を確認できません: {} ({e})",
            request.destination.display()
        )
    })?;
    if !std::fs::metadata(&destination)
        .map_err(|e| {
            format!(
                "整理先を確認できません: {} ({e})",
                request.destination.display()
            )
        })?
        .is_dir()
    {
        return Err(format!(
            "整理先は実フォルダではありません: {}",
            request.destination.display()
        ));
    }
    let destination_components = canonical_path_components(&destination);
    for source in &request.sources {
        if !source.is_absolute() || path_has_nul(source) {
            return Err(format!(
                "対象には NUL を含まない絶対パスが必要です: {}",
                source.display()
            ));
        }
        let canonical = std::fs::canonicalize(source)
            .map_err(|e| format!("対象を確認できません: {} ({e})", source.display()))?;
        let metadata = std::fs::metadata(&canonical)
            .map_err(|e| format!("対象を確認できません: {} ({e})", source.display()))?;
        if !metadata.is_dir() && !metadata.is_file() {
            return Err(format!(
                "対象は実ファイル／実フォルダではありません: {}",
                source.display()
            ));
        }
        if request.operation == ShellTransferOperation::Move {
            // Shell moves the selected item, including a junction itself. Resolve
            // its containing folder separately from the item's link target.
            if let Some(parent) = source.parent() {
                let parent = std::fs::canonicalize(parent).map_err(|e| {
                    format!(
                        "対象の親フォルダを確認できません: {} ({e})",
                        parent.display()
                    )
                })?;
                if canonical_path_components(&parent) == destination_components {
                    return Err(format!(
                        "同じフォルダなので移動できません: {}",
                        source.display()
                    ));
                }
            }
        }
        if metadata.is_dir()
            && destination_components.starts_with(&canonical_path_components(&canonical))
        {
            return Err(format!(
                "フォルダ自身またはその配下には移動／コピーできません: {}",
                source.display()
            ));
        }
    }
    // Resolve aliases for safety checks without changing the selected Shell item:
    // a junction's user-visible name and Windows' link handling remain Shell-owned.
    Ok(request)
}

#[cfg(windows)]
fn run_transfer_items(hwnd: isize, request: ShellTransferRequest) -> ShellTransferResult {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::System::Com::{
        CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx,
        CoUninitialize,
    };
    use windows::Win32::UI::Shell::{
        FOF_ALLOWUNDO, FOFX_ADDUNDORECORD, FileOperation, IFileOperation,
        IFileOperationProgressSink, IShellItem, SHCreateItemFromParsingName,
    };
    use windows::core::{IUnknown, PCWSTR};

    struct ComStaGuard;
    impl Drop for ComStaGuard {
        fn drop(&mut self) {
            unsafe { CoUninitialize() };
        }
    }
    unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) }
        .ok()
        .map_err(|e| format!("CoInitializeEx(STA) failed: {e}"))?;
    let _com = ComStaGuard;
    let destination_w = wide_null_path(&request.destination);
    let destination: IShellItem = unsafe {
        SHCreateItemFromParsingName(
            PCWSTR(destination_w.as_ptr()),
            None::<&windows::Win32::System::Com::IBindCtx>,
        )
    }
    .map_err(|e| format!("整理先を開けません: {e}"))?;
    let op: IFileOperation =
        unsafe { CoCreateInstance(&FileOperation, None::<&IUnknown>, CLSCTX_INPROC_SERVER) }
            .map_err(|e| format!("IFileOperation を作成できません: {e}"))?;
    if hwnd != 0 {
        unsafe { op.SetOwnerWindow(HWND(hwnd as *mut core::ffi::c_void)) }
            .map_err(|e| format!("Shell 操作の owner window を設定できません: {e}"))?;
    }
    // Keep Windows' conflict, error, cancellation and progress UI enabled.
    unsafe { op.SetOperationFlags(FOF_ALLOWUNDO | FOFX_ADDUNDORECORD) }
        .map_err(|e| format!("Shell 操作フラグを設定できません: {e}"))?;
    for source in &request.sources {
        let source_w = wide_null_path(source);
        let item: IShellItem = unsafe {
            SHCreateItemFromParsingName(
                PCWSTR(source_w.as_ptr()),
                None::<&windows::Win32::System::Com::IBindCtx>,
            )
        }
        .map_err(|e| format!("対象を開けません: {} ({e})", source.display()))?;
        match request.operation {
            ShellTransferOperation::Copy => unsafe {
                op.CopyItem(
                    &item,
                    &destination,
                    PCWSTR::null(),
                    None::<&IFileOperationProgressSink>,
                )
            },
            ShellTransferOperation::Move => unsafe {
                op.MoveItem(
                    &item,
                    &destination,
                    PCWSTR::null(),
                    None::<&IFileOperationProgressSink>,
                )
            },
        }
        .map_err(|e| format!("移動／コピーを予約できません: {} ({e})", source.display()))?;
    }
    unsafe { op.PerformOperations() }.map_err(|e| format!("移動／コピーを完了できません: {e}"))?;
    let aborted = unsafe { op.GetAnyOperationsAborted() }
        .map_err(|e| format!("Shell 操作の完了状態を確認できません: {e}"))?
        .as_bool();
    Ok(ShellTransferOutcome { aborted })
}

#[cfg(not(windows))]
fn run_transfer_items(_hwnd: isize, _request: ShellTransferRequest) -> ShellTransferResult {
    Err("移動／コピーは Windows Shell 経由でのみ利用できます".into())
}

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

#[cfg(test)]
mod transfer_tests {
    use super::*;

    fn request(
        operation: ShellTransferOperation,
        sources: Vec<PathBuf>,
        destination: PathBuf,
    ) -> ShellTransferRequest {
        ShellTransferRequest {
            operation,
            sources,
            destination,
        }
    }

    #[test]
    fn shell_transfer_path_validation_all_sources_and_same_parent() {
        let temp = tempfile::tempdir().unwrap();
        let folder = temp.path().join("folder");
        std::fs::create_dir(&folder).unwrap();
        let file = temp.path().join("file.txt");
        std::fs::write(&file, b"file").unwrap();
        let copy = validate_transfer_request(request(
            ShellTransferOperation::Copy,
            vec![file.clone(), folder.clone()],
            temp.path().to_path_buf(),
        ))
        .unwrap();
        assert_eq!(copy.sources.len(), 2);
        let another_parent = temp.path().join("other");
        std::fs::create_dir(&another_parent).unwrap();
        let another_file = another_parent.join("other.txt");
        std::fs::write(&another_file, b"other").unwrap();
        let error = validate_transfer_request(request(
            ShellTransferOperation::Move,
            vec![another_file, file],
            temp.path().to_path_buf(),
        ))
        .unwrap_err();
        assert!(error.contains("同じフォルダ"));
    }

    #[test]
    fn shell_transfer_path_validation_self_descendant_and_neighbor() {
        let temp = tempfile::tempdir().unwrap();
        let folder = temp.path().join("photos");
        let child = folder.join("child");
        let neighbor = temp.path().join("photos-old");
        std::fs::create_dir_all(&child).unwrap();
        std::fs::create_dir(&neighbor).unwrap();
        for operation in [ShellTransferOperation::Copy, ShellTransferOperation::Move] {
            for destination in [
                folder.clone(),
                child.clone(),
                child.join(".."),
                child.join("."),
            ] {
                let error = validate_transfer_request(request(
                    operation,
                    vec![folder.clone()],
                    destination,
                ))
                .unwrap_err();
                assert!(error.contains("フォルダ自身またはその配下"));
            }
            validate_transfer_request(request(operation, vec![folder.clone()], neighbor.clone()))
                .unwrap();
        }
    }

    #[test]
    fn shell_transfer_path_validation_missing_relative_nul_and_not_folder() {
        let temp = tempfile::tempdir().unwrap();
        let file = temp.path().join("file.txt");
        std::fs::write(&file, b"file").unwrap();
        for destination in [
            temp.path().join("missing"),
            PathBuf::from("relative"),
            temp.path().join("bad\0path"),
            file.clone(),
        ] {
            assert!(
                validate_transfer_request(request(
                    ShellTransferOperation::Copy,
                    vec![file.clone()],
                    destination
                ))
                .is_err()
            );
        }
        for source in [
            temp.path().join("missing"),
            PathBuf::from("relative"),
            temp.path().join("bad\0path"),
        ] {
            assert!(
                validate_transfer_request(request(
                    ShellTransferOperation::Copy,
                    vec![file.clone(), source],
                    temp.path().to_path_buf()
                ))
                .is_err()
            );
        }
        assert!(
            validate_transfer_request(request(
                ShellTransferOperation::Copy,
                vec![],
                temp.path().to_path_buf()
            ))
            .is_err()
        );
    }

    #[cfg(windows)]
    #[test]
    fn shell_transfer_path_components_drive_unc_case_and_separator() {
        for (left, right) in [
            (r"C:\PHOTOS\child", r"c:/photos/child/"),
            (
                r"\\server\share\PHOTOS\child",
                r"\\SERVER\SHARE\photos\child\",
            ),
        ] {
            assert_eq!(
                canonical_path_components(std::path::Path::new(left)),
                canonical_path_components(std::path::Path::new(right))
            );
        }
        let folder = canonical_path_components(std::path::Path::new(r"C:\photos"));
        for neighbor in [
            r"C:\photos-old",
            r"D:\photos\child",
            r"\\server\share\photos\child",
        ] {
            assert!(
                !canonical_path_components(std::path::Path::new(neighbor)).starts_with(&folder)
            );
        }
        let share = canonical_path_components(std::path::Path::new(r"\\server\share\photos"));
        assert!(
            !canonical_path_components(std::path::Path::new(r"\\server\share2\photos\child"))
                .starts_with(&share)
        );
    }

    #[cfg(windows)]
    #[test]
    fn shell_transfer_path_validation_resolves_junction() {
        let temp = tempfile::tempdir().unwrap();
        let folder = temp.path().join("source");
        let child = folder.join("child");
        let alias = temp.path().join("alias");
        std::fs::create_dir_all(&child).unwrap();
        let output = std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(&alias)
            .arg(&child)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "junction creation failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let copy = validate_transfer_request(request(
            ShellTransferOperation::Copy,
            vec![folder.clone()],
            alias.clone(),
        ));
        let moved = validate_transfer_request(request(
            ShellTransferOperation::Move,
            vec![folder],
            alias.clone(),
        ));
        let aliased_source = validate_transfer_request(request(
            ShellTransferOperation::Copy,
            vec![alias.clone()],
            temp.path().to_path_buf(),
        ));
        let same_parent_move = validate_transfer_request(request(
            ShellTransferOperation::Move,
            vec![alias.clone()],
            temp.path().to_path_buf(),
        ));
        let other = child.join("other.txt");
        std::fs::write(&other, b"other item").unwrap();
        validate_transfer_request(request(
            ShellTransferOperation::Move,
            vec![other.clone()],
            temp.path().to_path_buf(),
        ))
        .unwrap();
        let mixed_move = validate_transfer_request(request(
            ShellTransferOperation::Move,
            vec![other.clone(), alias.clone()],
            temp.path().to_path_buf(),
        ));
        let target_parent_move = validate_transfer_request(request(
            ShellTransferOperation::Move,
            vec![alias.clone()],
            child.parent().unwrap().to_path_buf(),
        ));
        for refusal in [same_parent_move, mixed_move] {
            let error = refusal.unwrap_err();
            assert!(error.contains("同じフォルダなので移動できません"));
            assert!(error.contains(&alias.display().to_string()));
        }
        assert_eq!(target_parent_move.unwrap().sources, vec![alias.clone()]);
        assert_eq!(std::fs::read(&other).unwrap(), b"other item");
        assert!(!temp.path().join("other.txt").exists());
        std::fs::remove_dir(&alias).unwrap();
        assert!(copy.unwrap_err().contains("フォルダ自身またはその配下"));
        assert!(moved.unwrap_err().contains("フォルダ自身またはその配下"));
        assert_eq!(aliased_source.unwrap().sources, vec![alias]);
    }

    /// Windows may display native progress/error UI; run explicitly on the user's desktop.
    #[cfg(windows)]
    #[test]
    #[ignore = "Windows Shell UI/environment dependent; uses disposable temp data"]
    fn shell_transfer_real_copy_and_move_file_and_folder() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        let copied = temp.path().join("copied");
        let moved = temp.path().join("moved");
        let folder = source.join("folder");
        std::fs::create_dir_all(folder.join("nested")).unwrap();
        std::fs::create_dir(&copied).unwrap();
        std::fs::create_dir(&moved).unwrap();
        let file = source.join("file.txt");
        std::fs::write(&file, b"file contents").unwrap();
        std::fs::write(folder.join("nested/child.txt"), b"folder contents").unwrap();
        for (operation, destination) in [
            (ShellTransferOperation::Copy, &copied),
            (ShellTransferOperation::Move, &moved),
        ] {
            let result = transfer_items_async(
                None,
                request(
                    operation,
                    vec![file.clone(), folder.clone()],
                    destination.clone(),
                ),
                egui::Context::default(),
            )
            .recv_timeout(std::time::Duration::from_secs(300))
            .expect("Shell operation did not finish within five minutes")
            .expect("Shell operation failed");
            assert!(!result.aborted);
            assert_eq!(
                std::fs::read(destination.join("file.txt")).unwrap(),
                b"file contents"
            );
            assert_eq!(
                std::fs::read(destination.join("folder/nested/child.txt")).unwrap(),
                b"folder contents"
            );
            assert_eq!(file.exists(), operation == ShellTransferOperation::Copy);
            assert_eq!(folder.exists(), operation == ShellTransferOperation::Copy);
        }
    }
}
