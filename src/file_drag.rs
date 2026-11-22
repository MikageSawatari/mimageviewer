//! ファイル D&D (ドラッグでコピー送出) の COM 経路。
//!
//! グリッドのサムネイルを掴んで、エクスプローラや他アプリへファイル / フォルダを
//! ドラッグ＆ドロップでコピーするための Windows シェル連携。設計の全体像は
//! `docs/file-drag-drop-design.md` を参照。
//!
//! パス → PIDL → `IShellItemArray` → `BindToHandler(BHID_DataObject)` で Shell の
//! `IDataObject` を得る。送出だけを STA-bound な委譲ラッパーで包み、
//! `IDataObjectAsyncCapability` を提供する (§12)。共有の Shell 生成経路は変更しない。
//!
//! UI スレッド・マウス押下中に `SHDoDragDrop` を呼ぶ。ドラッグ操作の OLE モーダル
//! 区間は残る。非同期を選択して Drop から早期復帰する target では、その後の転送完了を
//! 待たない。非対応 target の同期処理と開始前の Shell 解決コストは残る。
//! 復帰後の GetData / SetData / EndOperation は通常の winit message loop が UI STA へ配送する。
//! 1ドラッグ1 owner とし App を参照せず、終了時に転送完了を待つ機構も追加しない。

use std::path::{Path, PathBuf};

#[cfg(windows)]
mod async_data_object;

#[cfg(all(windows, test))]
pub(crate) use async_data_object::tests::with_shell_drag_data_object;

/// `start_file_drag` の結果。呼び出し側はこれを見て、ポインタリセットの要否・
/// 失敗トーストの要否を判断する (`docs/file-drag-drop-design.md` §5.1)。
#[derive(Debug, Clone)]
pub struct DragOutcome {
    /// `SHDoDragDrop` を実際に呼んだか。true のときだけドラッグ後のポインタ
    /// リセットが要る (到達しなければ winit が通常どおり WM_LBUTTONUP を受ける)。
    /// `SHDoDragDrop` が HRESULT エラーを返した場合でも「呼んだ = モーダル
    /// ループに入った」ので true。
    pub started: bool,
    /// `SHParseDisplayName` に失敗したパス数 (0 が正常)。>0 ならトーストで明示する。
    pub failed_paths: usize,
    /// COM 各ステップで失敗した場合のエラー。正常時は `None`。
    pub error: Option<FileDragError>,
    /// ドロップが受理されたか (`SHDoDragDrop` の effect が NONE 以外)。
    /// 非同期転送の完了・成功を示すものではない。
    /// キャンセル (Esc / 無効ターゲット上で離す) のときは false。受理の診断値であり、
    /// 自分の窓への落下判定や元ファイルの削除には使わない。
    pub dropped: bool,
}

impl DragOutcome {
    /// ドラッグを開始しなかった (空入力 / 非 Windows) 場合の結果。
    fn not_started() -> Self {
        Self {
            started: false,
            failed_paths: 0,
            error: None,
            dropped: false,
        }
    }

    /// `SHDoDragDrop` へ到達する前に失敗した結果 (`started = false`)。
    fn failed_before_start(failed_paths: usize, error: FileDragError) -> Self {
        Self {
            started: false,
            failed_paths,
            error: Some(error),
            dropped: false,
        }
    }

    /// `SHDoDragDrop` を呼んだ後の結果 (`started = true`、`error` は HRESULT エラー時のみ)。
    fn after_modal(failed_paths: usize, error: Option<FileDragError>, dropped: bool) -> Self {
        Self {
            started: true,
            failed_paths,
            error,
            dropped,
        }
    }
}

/// `start_file_drag` の COM ステップ別エラー。どこで失敗したかを呼び出し側が
/// 区別できるようにする (主にログの切り分け用)。HRESULT は生の `i32` で保持し、
/// 型をプラットフォーム非依存に保つ。
#[derive(Debug, Clone)]
pub enum FileDragError {
    /// `SHParseDisplayName` が全パスで失敗した (ドラッグ対象が 1 件も作れない)。
    AllPathsUnresolved,
    /// `SHCreateShellItemArrayFromIDLists` が失敗した。
    ShellArrayCreate(i32),
    /// `IShellItemArray::BindToHandler(BHID_DataObject)` が失敗した。
    BindToHandler(i32),
    /// 送出専用 async capability の設定 / 復帰時の寿命処理が失敗した。
    AsyncCapability(i32),
    /// `SHDoDragDrop` 自体が HRESULT エラーを返した (モーダルループには入っている)。
    DoDragDrop(i32),
}

/// 実パス群を Shell の `IDataObject` へ変換する共有経路。
///
/// ドラッグ送出とファイル Copy/Cut のクリップボード送出を同じ
/// `SHCreateShellItemArrayFromIDLists` + `BHID_DataObject` 実装へ揃える。
/// エラー時の tuple 先頭は `SHParseDisplayName` に失敗したパス数。
#[cfg(windows)]
pub(crate) fn shell_data_object_for_paths(
    paths: &[PathBuf],
) -> Result<(windows::Win32::System::Com::IDataObject, usize), (usize, FileDragError)> {
    use windows::Win32::System::Com::{CoTaskMemFree, IBindCtx, IDataObject};
    use windows::Win32::UI::Shell::Common::ITEMIDLIST;
    use windows::Win32::UI::Shell::{
        BHID_DataObject, SHCreateShellItemArrayFromIDLists, SHParseDisplayName,
    };
    use windows::core::PCWSTR;

    let mut pidls: Vec<*const ITEMIDLIST> = Vec::with_capacity(paths.len());
    let mut failed_paths = 0usize;
    for path in paths {
        let wide = shell_parse_wide_path(path);
        let mut pidl: *mut ITEMIDLIST = std::ptr::null_mut();
        let parsed = unsafe {
            SHParseDisplayName(PCWSTR(wide.as_ptr()), None::<&IBindCtx>, &mut pidl, 0, None)
        };
        if parsed.is_ok() && !pidl.is_null() {
            pidls.push(pidl as *const ITEMIDLIST);
        } else {
            failed_paths += 1;
            if !pidl.is_null() {
                unsafe { CoTaskMemFree(Some(pidl as *const core::ffi::c_void)) };
            }
        }
    }

    let free_pidls = |pidls: &[*const ITEMIDLIST]| {
        for &pidl in pidls {
            unsafe { CoTaskMemFree(Some(pidl as *const core::ffi::c_void)) };
        }
    };

    if pidls.is_empty() {
        return Err((failed_paths, FileDragError::AllPathsUnresolved));
    }

    let array = match unsafe { SHCreateShellItemArrayFromIDLists(&pidls) } {
        Ok(array) => array,
        Err(error) => {
            free_pidls(&pidls);
            return Err((
                failed_paths,
                FileDragError::ShellArrayCreate(error.code().0),
            ));
        }
    };
    free_pidls(&pidls);
    let data: IDataObject = unsafe { array.BindToHandler(None::<&IBindCtx>, &BHID_DataObject) }
        .map_err(|error| (failed_paths, FileDragError::BindToHandler(error.code().0)))?;
    Ok((data, failed_paths))
}

/// `shell_data_object_for_paths` と同じ材料から `IShellItemArray` の方を返す。
///
/// パッケージ (Store) アプリの起動 (`IApplicationActivationManager::ActivateForFile`) は
/// `IDataObject` ではなく `IShellItemArray` を取る。PIDL 解決を 2 度書かないよう分ける。
#[cfg(windows)]
pub(crate) fn shell_item_array_for_paths(
    paths: &[PathBuf],
) -> Result<(windows::Win32::UI::Shell::IShellItemArray, usize), (usize, FileDragError)> {
    use windows::Win32::System::Com::{CoTaskMemFree, IBindCtx};
    use windows::Win32::UI::Shell::Common::ITEMIDLIST;
    use windows::Win32::UI::Shell::{SHCreateShellItemArrayFromIDLists, SHParseDisplayName};
    use windows::core::PCWSTR;

    let mut pidls: Vec<*const ITEMIDLIST> = Vec::with_capacity(paths.len());
    let mut failed_paths = 0usize;
    for path in paths {
        let wide = shell_parse_wide_path(path);
        let mut pidl: *mut ITEMIDLIST = std::ptr::null_mut();
        let parsed = unsafe {
            SHParseDisplayName(PCWSTR(wide.as_ptr()), None::<&IBindCtx>, &mut pidl, 0, None)
        };
        if parsed.is_ok() && !pidl.is_null() {
            pidls.push(pidl as *const ITEMIDLIST);
        } else {
            failed_paths += 1;
            if !pidl.is_null() {
                unsafe { CoTaskMemFree(Some(pidl as *const core::ffi::c_void)) };
            }
        }
    }
    let free_pidls = |pidls: &[*const ITEMIDLIST]| {
        for &pidl in pidls {
            unsafe { CoTaskMemFree(Some(pidl as *const core::ffi::c_void)) };
        }
    };
    if pidls.is_empty() {
        return Err((failed_paths, FileDragError::AllPathsUnresolved));
    }
    let array = match unsafe { SHCreateShellItemArrayFromIDLists(&pidls) } {
        Ok(array) => array,
        Err(error) => {
            free_pidls(&pidls);
            return Err((
                failed_paths,
                FileDragError::ShellArrayCreate(error.code().0),
            ));
        }
    };
    free_pidls(&pidls);
    Ok((array, failed_paths))
}

/// 指定パス群の OLE ドラッグ＆ドロップ (コピー) を開始する。
///
/// OLE モーダル区間はブロックする。非同期 target の転送完了は待たない。
/// UI スレッドから、かつマウスボタンが押下中に呼ぶこと。
#[cfg(windows)]
pub(crate) fn start_file_drag(
    hwnd: isize,
    paths: &[PathBuf],
    excluded_virtual: bool,
    shared: &crate::main_drop_target::SharedMainDrop,
) -> DragOutcome {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::System::Ole::{DROPEFFECT_COPY, IDropSource};
    use windows::Win32::UI::Shell::SHDoDragDrop;

    if paths.is_empty() {
        return DragOutcome::not_started();
    }

    // COM 初期化はしない: winit が UI スレッドを STA 初期化済み
    // (docs/file-drag-drop-design.md §6.3)。この関数は UI スレッドからのみ呼ばれる。

    let (data, failed_paths) = match shell_data_object_for_paths(paths) {
        Ok(result) => result,
        Err((failed_paths, error)) => {
            crate::logger::log(format!(
                "file_drag: could not build Shell IDataObject: {error:?}; failed_paths={failed_paths}"
            ));
            return DragOutcome::failed_before_start(failed_paths, error);
        }
    };
    let drag = match async_data_object::DragDataObject::new(data) {
        Ok(drag) => drag,
        Err(error) => {
            crate::logger::log(format!("file_drag: async setup failed: {error}"));
            return DragOutcome::failed_before_start(
                failed_paths,
                FileDragError::AsyncCapability(error.code().0),
            );
        }
    };

    // SHDoDragDrop (モーダルブロック)。pdsrc は None でシェル既定の
    //    IDropSource が使われる (docs/file-drag-drop-design.md §4.3)。
    let hwnd = HWND(hwnd as *mut core::ffi::c_void);
    crate::logger::log(format!(
        "file_drag: SHDoDragDrop start ({} item(s)) drag_id={}",
        paths.len(),
        drag.id,
    ));
    let started_at = std::time::Instant::now();
    let internal = crate::main_drop_target::InternalDrag {
        paths: paths.to_vec(),
        excluded_virtual,
        failed_paths,
    };
    let result = with_internal_drag(shared, internal, || unsafe {
        SHDoDragDrop(
            Some(hwnd),
            &drag.data,
            None::<&IDropSource>,
            DROPEFFECT_COPY,
        )
    });
    let elapsed_ms = started_at.elapsed().as_secs_f64() * 1000.0;
    // The target's Start/End state owns lifetime even when the modal result is NONE/error.
    let in_operation = drag.finish_modal();
    let async_error = in_operation.as_ref().err().map(|error| {
        crate::logger::log(format!(
            "file_drag: async cleanup failed drag_id={}: {error}",
            drag.id
        ));
        FileDragError::AsyncCapability(error.code().0)
    });
    let in_operation = in_operation.ok();
    match result {
        Ok(effect) => {
            crate::logger::log(format!(
                "file_drag: SHDoDragDrop done effect={} drag_id={} elapsed_ms={elapsed_ms:.3} in_operation={in_operation:?}",
                effect.0, drag.id,
            ));
            // effect が NONE (0) 以外ならドロップ成立。Esc キャンセルや無効ターゲット上で
            // 離した場合は NONE。
            DragOutcome::after_modal(failed_paths, async_error, effect.0 != 0)
        }
        Err(e) => {
            crate::logger::log(format!(
                "file_drag: SHDoDragDrop failed: {e} drag_id={} elapsed_ms={elapsed_ms:.3} in_operation={in_operation:?}",
                drag.id,
            ));
            DragOutcome::after_modal(
                failed_paths,
                Some(FileDragError::DoDragDrop(e.code().0)),
                false,
            )
        }
    }
}

fn with_internal_drag<R>(
    shared: &crate::main_drop_target::SharedMainDrop,
    internal: crate::main_drop_target::InternalDrag,
    modal: impl FnOnce() -> R,
) -> R {
    shared.borrow_mut().internal = Some(internal);
    let result = modal();
    shared.borrow_mut().internal = None;
    result
}

#[cfg(windows)]
fn shell_parse_wide_path(path: &Path) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;

    // Ctrl+G results use the normalized index key (`c:/...`). Win32 file APIs
    // accept '/', but SHParseDisplayName rejects it with E_INVALIDARG.
    path.as_os_str()
        .encode_wide()
        .map(|ch| if ch == b'/' as u16 { b'\\' as u16 } else { ch })
        .chain(std::iter::once(0))
        .collect()
}

/// 非 Windows ビルド用の空実装。他のプラットフォーム分岐に揃えるためのスタブ。
#[cfg(not(windows))]
pub(crate) fn start_file_drag(
    _hwnd: isize,
    _paths: &[PathBuf],
    _excluded_virtual: bool,
    _shared: &crate::main_drop_target::SharedMainDrop,
) -> DragOutcome {
    DragOutcome::not_started()
}

/// 一覧への外部ドロップを worker 上でコピーし、既存の完了通知へ写す。
/// 全パスをそのまま渡し、検証と実転送は整理先と同じ入口に任せる。
/// `transfer` は Windows の画面を出さずに worker の境界を検証するため差し替えられる。
pub(crate) fn copy_dropped_paths_blocking(
    hwnd: Option<isize>,
    paths: Vec<PathBuf>,
    destination: PathBuf,
    transfer: impl FnOnce(
        Option<isize>,
        crate::shell_file_ops::ShellTransferRequest,
    ) -> crate::shell_file_ops::ShellTransferResult,
) -> crate::ui_dialogs::context_menu::CopyOutcome {
    use crate::shell_file_ops::{ShellTransferOperation, ShellTransferRequest};
    use crate::ui_dialogs::context_menu::CopyOutcome;

    let attempted = paths.len();
    let request = ShellTransferRequest {
        operation: ShellTransferOperation::Copy,
        default_move_needs_volume_check: false,
        sources: paths,
        destination,
    };
    match transfer(hwnd, request) {
        Ok(result) => CopyOutcome {
            attempted,
            notice: result.notice(false),
            ..CopyOutcome::default()
        },
        Err(message) => CopyOutcome::all_failed(attempted, message),
    }
}

#[cfg(test)]
mod file_drop_worker_tests {
    use super::copy_dropped_paths_blocking;
    use crate::shell_file_ops::{ShellTransferOperation, ShellTransferOutcome};
    use std::path::PathBuf;

    #[test]
    fn native_drag_shared_payload_exists_only_inside_modal_call_including_error_return() {
        use crate::main_drop_target::{InternalDrag, MainDropShared};
        let shared = std::rc::Rc::new(std::cell::RefCell::new(MainDropShared::default()));
        for failed in [false, true] {
            let paths = vec![
                PathBuf::from(r"C:\all\first"),
                PathBuf::from(r"C:\all\unresolved"),
            ];
            let result = super::with_internal_drag(
                &shared,
                InternalDrag {
                    paths: paths.clone(),
                    excluded_virtual: true,
                    failed_paths: 1,
                },
                || {
                    let mut state = shared.borrow_mut(); // No borrow spans the modal call.
                    let source = state.internal.as_ref().unwrap();
                    assert_eq!(source.paths, paths);
                    assert!(source.excluded_virtual);
                    assert_eq!(source.failed_paths, 1);
                    state.admission = Ok(());
                    if failed { Err("modal error") } else { Ok(()) }
                },
            );
            assert_eq!(result.is_err(), failed);
            assert!(shared.borrow().internal.is_none());
        }
    }

    #[test]
    fn dropped_paths_worker_preserves_file_folder_order_destination_and_owner() {
        let temp = tempfile::tempdir().unwrap();
        let file = temp.path().join("file.txt");
        let folder = temp.path().join("folder");
        std::fs::write(&file, b"file contents").unwrap();
        std::fs::create_dir(&folder).unwrap();
        let paths = vec![file.clone(), folder.clone()];
        let destination = temp.path().join("destination");
        let outcome = copy_dropped_paths_blocking(
            Some(123),
            paths.clone(),
            destination.clone(),
            |hwnd, request| {
                assert_eq!(hwnd, Some(123));
                assert_eq!(request.operation, ShellTransferOperation::Copy);
                assert_eq!(request.sources, paths);
                assert_eq!(request.destination, destination);
                Ok(ShellTransferOutcome::test_outcome(0, Ok(false), 0))
            },
        );
        assert_eq!(outcome.attempted, 2);
        assert_eq!(outcome.failed, 0);
        assert!(outcome.first_errors.is_empty());
        assert!(outcome.notice.is_none());
        assert_eq!(std::fs::read(&file).unwrap(), b"file contents");
        assert!(folder.is_dir());
    }

    #[test]
    fn dropped_paths_worker_keeps_folder_only_request() {
        let temp = tempfile::tempdir().unwrap();
        let folder = temp.path().join("folder");
        std::fs::create_dir(&folder).unwrap();
        let outcome = copy_dropped_paths_blocking(
            None,
            vec![folder.clone()],
            temp.path().join("destination"),
            |hwnd, request| {
                assert_eq!(hwnd, None);
                assert_eq!(request.sources, vec![folder]);
                Ok(ShellTransferOutcome::test_outcome(0, Ok(false), 0))
            },
        );
        assert_eq!(outcome.attempted, 1);
        assert_eq!(outcome.failed, 0);
        assert!(outcome.notice.is_none());
    }

    #[test]
    fn dropped_paths_worker_reports_cancellation_without_claiming_failure() {
        let outcome = copy_dropped_paths_blocking(
            None,
            vec![PathBuf::from("file.txt"), PathBuf::from("folder")],
            PathBuf::from("destination"),
            |_, _| Ok(ShellTransferOutcome::test_outcome(0, Ok(true), 0)),
        );
        assert_eq!(outcome.attempted, 2);
        assert_eq!(outcome.failed, 0);
        assert!(outcome.first_errors.is_empty());
        assert_eq!(
            outcome.notice.as_deref(),
            Some("操作が中断されました。移動・コピーした項目はありません")
        );
    }

    #[test]
    fn dropped_paths_858_worker_uses_shared_interrupted_and_failed_notices() {
        for (perform, abort, processed, expected) in [
            (
                0,
                Ok(true),
                0,
                "操作が中断されました。移動・コピーした項目はありません",
            ),
            (
                0,
                Ok(true),
                1,
                "操作が中断されました。一部の項目は処理済みです。フォルダの内容を確認してください",
            ),
            (
                0x800704C7u32 as i32,
                Ok(false),
                0,
                "操作が中断されました。移動・コピーした項目はありません",
            ),
            (
                0x80070005u32 as i32,
                Ok(false),
                0,
                "操作を完了できませんでした。フォルダの内容を確認してください\nShell: 0x80070005、中断状態: 中断なし",
            ),
            (
                0x80070005u32 as i32,
                Ok(false),
                1,
                "操作を完了できませんでした。フォルダの内容を確認してください。一部の項目は処理済みです\nShell: 0x80070005、中断状態: 中断なし",
            ),
            (
                0,
                Err(0x80004005u32 as i32),
                0,
                "操作を完了できませんでした。フォルダの内容を確認してください\nShell: 0x00000000、中断状態: 照会失敗 0x80004005",
            ),
        ] {
            let outcome = copy_dropped_paths_blocking(
                None,
                vec!["file.txt".into(), "folder".into()],
                "destination".into(),
                |_, _| {
                    Ok(ShellTransferOutcome::test_outcome(
                        perform, abort, processed,
                    ))
                },
            );
            assert_eq!(outcome.attempted, 2);
            assert_eq!(outcome.notice.as_deref(), Some(expected));
            assert_eq!(
                outcome.failed, 0,
                "Shell callback counts include descendants, not requested-source failures"
            );
            assert!(outcome.first_errors.is_empty());
        }
    }

    #[test]
    fn dropped_paths_worker_preserves_error_as_first_failure() {
        let outcome = copy_dropped_paths_blocking(
            None,
            vec![PathBuf::from("file.txt"), PathBuf::from("folder")],
            PathBuf::from("destination"),
            |_, _| Err("Shell transfer failed".into()),
        );
        assert_eq!(outcome.attempted, 2);
        assert_eq!(outcome.failed, 2);
        assert_eq!(outcome.first_errors, vec!["Shell transfer failed"]);
        assert!(outcome.notice.is_none());
    }

    #[test]
    fn dropped_paths_worker_rejects_whole_request_for_self_or_descendant() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        let child = source.join("child");
        std::fs::create_dir_all(&child).unwrap();
        let file = temp.path().join("file.txt");
        std::fs::write(&file, b"file contents").unwrap();
        for destination in [source.clone(), child.clone()] {
            let outcome = copy_dropped_paths_blocking(
                None,
                vec![file.clone(), source.clone()],
                destination.clone(),
                crate::shell_file_ops::transfer_items_blocking,
            );
            assert_eq!(outcome.attempted, 2);
            assert_eq!(outcome.failed, 2);
            assert!(outcome.first_errors[0].contains("フォルダ自身またはその配下"));
            assert!(outcome.notice.is_none());
            assert!(!destination.join("file.txt").exists());
        }
        assert_eq!(std::fs::read(&file).unwrap(), b"file contents");
    }

    /// Windows may display native progress/error UI; run explicitly on the user's desktop.
    #[cfg(windows)]
    #[test]
    #[ignore = "Windows Shell UI/environment dependent; uses disposable temp data"]
    fn dropped_paths_worker_real_copy_file_and_folder() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        let destination = temp.path().join("destination");
        let folder = source.join("folder");
        std::fs::create_dir_all(folder.join("nested")).unwrap();
        std::fs::create_dir(&destination).unwrap();
        let file = source.join("file.txt");
        std::fs::write(&file, b"file contents").unwrap();
        std::fs::write(folder.join("nested/child.txt"), b"folder contents").unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        let worker_file = file.clone();
        let worker_folder = folder.clone();
        let worker_destination = destination.clone();
        std::thread::Builder::new()
            .name("file-drop-validate".into())
            .spawn(move || {
                let outcome = copy_dropped_paths_blocking(
                    None,
                    vec![worker_file, worker_folder],
                    worker_destination,
                    crate::shell_file_ops::transfer_items_blocking,
                );
                let _ = tx.send(outcome);
            })
            .unwrap();
        let outcome = rx
            .recv_timeout(std::time::Duration::from_secs(300))
            .expect("Shell operation did not finish within five minutes");
        assert_eq!(outcome.attempted, 2);
        assert_eq!(outcome.failed, 0, "{:?}", outcome.first_errors);
        assert!(outcome.notice.is_none());
        assert_eq!(
            std::fs::read(destination.join("file.txt")).unwrap(),
            b"file contents"
        );
        assert_eq!(
            std::fs::read(destination.join("folder/nested/child.txt")).unwrap(),
            b"folder contents"
        );
        assert_eq!(std::fs::read(&file).unwrap(), b"file contents");
        assert_eq!(
            std::fs::read(folder.join("nested/child.txt")).unwrap(),
            b"folder contents"
        );
    }
}

#[cfg(all(test, windows))]
mod shell_parse_path_tests {
    use super::shell_parse_wide_path;
    use std::path::Path;

    #[test]
    fn converts_forward_slashes_for_shell_parse() {
        let wide = shell_parse_wide_path(Path::new(
            r"g:/home/comfyui/eagle/ai/sd_image/2025-04-19.png",
        ));
        assert_eq!(wide.last().copied(), Some(0));
        let without_nul = &wide[..wide.len() - 1];
        assert_eq!(
            String::from_utf16(without_nul).unwrap(),
            r"g:\home\comfyui\eagle\ai\sd_image\2025-04-19.png"
        );
    }

    #[test]
    fn converts_normalized_unc_paths_for_shell_parse() {
        let wide = shell_parse_wide_path(Path::new(r"//server/share/folder/a.jpg"));
        let without_nul = &wide[..wide.len() - 1];
        assert_eq!(
            String::from_utf16(without_nul).unwrap(),
            r"\\server\share\folder\a.jpg"
        );
    }
}
