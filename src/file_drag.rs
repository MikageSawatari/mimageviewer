//! ファイル D&D (ドラッグでコピー送出) の COM 経路。
//!
//! グリッドのサムネイルを掴んで、エクスプローラや他アプリへファイル / フォルダを
//! ドラッグ＆ドロップでコピーするための Windows シェル連携。設計の全体像は
//! `docs/file-drag-drop-design.md` を参照。
//!
//! 方針: 自前で `IDataObject` を実装せず、シェルが用意済みのものを借りる。
//! パス → PIDL → `IShellItemArray` → `BindToHandler(BHID_DataObject)` で
//! 完成済み `IDataObject` を得て `SHDoDragDrop` に渡す。`SHDoDragDrop` は
//! 既定の `IDropSource` とドラッグ画像も自動で提供する。
//!
//! `start_file_drag` は `SHDoDragDrop` が戻る (ドロップ完了 or キャンセル) まで
//! ブロックする。UI スレッドから、かつマウスボタン押下中に呼ぶこと。

use std::path::PathBuf;

/// `start_file_drag` の結果。呼び出し側はこれを見て、ポインタリセットの要否・
/// 失敗トーストの要否・ログ出力内容を判断する (`docs/file-drag-drop-design.md` §5.1)。
#[derive(Debug, Clone)]
pub struct DragOutcome {
    /// `SHDoDragDrop` を実際に呼んだか。true のときだけドラッグ後のポインタ
    /// リセットが要る (到達しなければ winit が通常どおり WM_LBUTTONUP を受ける)。
    /// `SHDoDragDrop` が HRESULT エラーを返した場合でも「呼んだ = モーダル
    /// ループに入った」ので true。
    pub started: bool,
    /// `SHParseDisplayName` に失敗したパス数 (0 が正常)。>0 ならトーストで明示する。
    pub failed_paths: usize,
    /// `SHDoDragDrop` の結果 DROPEFFECT の生ビット。`started == true` かつ成功時のみ
    /// `Some`。1 (`DROPEFFECT_COPY`) ならコピー成立、0 (`DROPEFFECT_NONE`) なら
    /// キャンセル。
    pub effect: Option<u32>,
    /// COM 各ステップで失敗した場合のエラー。正常時は `None`。
    pub error: Option<FileDragError>,
}

impl DragOutcome {
    /// ドラッグを開始しなかった (空入力 / 非 Windows) 場合の結果。
    fn not_started() -> Self {
        Self {
            started: false,
            failed_paths: 0,
            effect: None,
            error: None,
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
    /// `SHDoDragDrop` 自体が HRESULT エラーを返した (モーダルループには入っている)。
    DoDragDrop(i32),
}

/// 指定パス群の OLE ドラッグ＆ドロップ (コピー) を開始する。
///
/// `SHDoDragDrop` が戻る (ドロップ完了 or キャンセル) までブロックする。
/// UI スレッドから、かつマウスボタンが押下中に呼ぶこと。
#[cfg(windows)]
pub fn start_file_drag(hwnd: isize, paths: &[PathBuf]) -> DragOutcome {
    use std::os::windows::ffi::OsStrExt;
    use windows::Win32::Foundation::HWND;
    use windows::Win32::System::Com::{CoTaskMemFree, IBindCtx, IDataObject};
    use windows::Win32::System::Ole::{DROPEFFECT_COPY, IDropSource};
    use windows::Win32::UI::Shell::Common::ITEMIDLIST;
    use windows::Win32::UI::Shell::{
        BHID_DataObject, SHCreateShellItemArrayFromIDLists, SHDoDragDrop, SHParseDisplayName,
    };
    use windows::core::PCWSTR;

    if paths.is_empty() {
        return DragOutcome::not_started();
    }

    // COM 初期化はしない: winit が UI スレッドを STA 初期化済み
    // (docs/file-drag-drop-design.md §6.3)。この関数は UI スレッドからのみ呼ばれる。

    // 1. 各パスを PIDL に変換する。
    let mut pidls: Vec<*const ITEMIDLIST> = Vec::with_capacity(paths.len());
    let mut failed_paths = 0usize;
    for path in paths {
        let wide: Vec<u16> = path
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        let mut pidl: *mut ITEMIDLIST = std::ptr::null_mut();
        let parsed = unsafe {
            SHParseDisplayName(PCWSTR(wide.as_ptr()), None::<&IBindCtx>, &mut pidl, 0, None)
        };
        if parsed.is_ok() && !pidl.is_null() {
            pidls.push(pidl as *const ITEMIDLIST);
        } else {
            failed_paths += 1;
            if !pidl.is_null() {
                // 異常系の保険: 失敗扱いでも非 null なら解放する。
                unsafe { CoTaskMemFree(Some(pidl as *const core::ffi::c_void)) };
            }
        }
    }

    let free_pidls = |pidls: &[*const ITEMIDLIST]| {
        for &p in pidls {
            unsafe { CoTaskMemFree(Some(p as *const core::ffi::c_void)) };
        }
    };

    if pidls.is_empty() {
        crate::logger::log("file_drag: all paths failed SHParseDisplayName");
        return DragOutcome {
            started: false,
            failed_paths,
            effect: None,
            error: Some(FileDragError::AllPathsUnresolved),
        };
    }

    // 2. PIDL 配列から IShellItemArray を作る。
    let array = match unsafe { SHCreateShellItemArrayFromIDLists(&pidls) } {
        Ok(a) => a,
        Err(e) => {
            free_pidls(&pidls);
            crate::logger::log(format!(
                "file_drag: SHCreateShellItemArrayFromIDLists failed: {e}"
            ));
            return DragOutcome {
                started: false,
                failed_paths,
                effect: None,
                error: Some(FileDragError::ShellArrayCreate(e.code().0)),
            };
        }
    };
    // 配列が PIDL をコピー済みなので、ここで元の PIDL を解放してよい。
    free_pidls(&pidls);

    // 3. シェル完成済みの IDataObject を借りる。
    let data: IDataObject =
        match unsafe { array.BindToHandler(None::<&IBindCtx>, &BHID_DataObject) } {
            Ok(d) => d,
            Err(e) => {
                crate::logger::log(format!("file_drag: BindToHandler failed: {e}"));
                return DragOutcome {
                    started: false,
                    failed_paths,
                    effect: None,
                    error: Some(FileDragError::BindToHandler(e.code().0)),
                };
            }
        };

    // 4. SHDoDragDrop (モーダルブロック)。pdsrc は None でシェル既定の
    //    IDropSource が使われる (docs/file-drag-drop-design.md §4.3)。
    let hwnd = HWND(hwnd as *mut core::ffi::c_void);
    crate::logger::log(format!(
        "file_drag: SHDoDragDrop start ({} item(s))",
        paths.len()
    ));
    match unsafe { SHDoDragDrop(Some(hwnd), &data, None::<&IDropSource>, DROPEFFECT_COPY) } {
        Ok(effect) => {
            crate::logger::log(format!("file_drag: SHDoDragDrop done effect={}", effect.0));
            DragOutcome {
                started: true,
                failed_paths,
                effect: Some(effect.0),
                error: None,
            }
        }
        Err(e) => {
            crate::logger::log(format!("file_drag: SHDoDragDrop failed: {e}"));
            DragOutcome {
                started: true,
                failed_paths,
                effect: None,
                error: Some(FileDragError::DoDragDrop(e.code().0)),
            }
        }
    }
}

/// 非 Windows ビルド用の空実装。他のプラットフォーム分岐に揃えるためのスタブ。
#[cfg(not(windows))]
pub fn start_file_drag(_hwnd: isize, _paths: &[PathBuf]) -> DragOutcome {
    DragOutcome::not_started()
}
