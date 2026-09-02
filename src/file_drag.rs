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
//!
//! **モーダルブロックの不可避性 (review #8 メモ)**: `SHDoDragDrop` は OLE2 の
//! STA (Single-Threaded Apartment) モデルに従って HWND 所有スレッド上で独自の
//! メッセージループを回す。`IDataObject` は呼び出しスレッドのアパートに bound
//! されるため、別スレッドへ移して実行することは仕様上できない (= drag は UI
//! スレッドをブロックする以外の選択肢が無い)。
//!
//! ユーザー影響の軽減策:
//!   - **presenter / 音声 / decoder スレッドは独立**して動き続けるので、ドラッグ
//!     中も動画は再生され続け、音飛びも起きない。`App::update` 側の poll が止まる
//!     だけで、worker 自体は backlog を貯めながら作業を継続している。
//!   - SHDoDragDrop 復帰直後に `ctx.request_repaint()` を呼ぶ (呼び出し側で実施) と、
//!     次フレームで `poll_pdf_render` / `poll_file_drop_pending` /
//!     `poll_global_search_events` などが backlog をまとめて処理する。
//!   - 将来的な改善案: カスタム `IDropSource` を実装して `GiveFeedback` callback
//!     から mIV の worker poll を周期駆動する。ただし `IDropSource` 内から `App`
//!     (= `&mut self`) を参照する経路を作る必要があり、現時点では未実装。

use std::path::{Path, PathBuf};

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
    /// ドロップが実際に成立したか (`SHDoDragDrop` の effect が NONE 以外)。
    /// キャンセル (Esc / 無効ターゲット上で離す) のときは false。呼び出し側が
    /// 「mIV 自身のウィンドウへ落ちた」判定をするのに使う。
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
/// `SHDoDragDrop` が戻る (ドロップ完了 or キャンセル) までブロックする。
/// UI スレッドから、かつマウスボタンが押下中に呼ぶこと。
#[cfg(windows)]
pub fn start_file_drag(hwnd: isize, paths: &[PathBuf]) -> DragOutcome {
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

    // SHDoDragDrop (モーダルブロック)。pdsrc は None でシェル既定の
    //    IDropSource が使われる (docs/file-drag-drop-design.md §4.3)。
    let hwnd = HWND(hwnd as *mut core::ffi::c_void);
    crate::logger::log(format!(
        "file_drag: SHDoDragDrop start ({} item(s))",
        paths.len()
    ));
    match unsafe { SHDoDragDrop(Some(hwnd), &data, None::<&IDropSource>, DROPEFFECT_COPY) } {
        Ok(effect) => {
            crate::logger::log(format!("file_drag: SHDoDragDrop done effect={}", effect.0));
            // effect が NONE (0) 以外ならドロップ成立。Esc キャンセルや無効ターゲット上で
            // 離した場合は NONE。
            DragOutcome::after_modal(failed_paths, None, effect.0 != 0)
        }
        Err(e) => {
            crate::logger::log(format!("file_drag: SHDoDragDrop failed: {e}"));
            DragOutcome::after_modal(
                failed_paths,
                Some(FileDragError::DoDragDrop(e.code().0)),
                false,
            )
        }
    }
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
pub fn start_file_drag(_hwnd: isize, _paths: &[PathBuf]) -> DragOutcome {
    DragOutcome::not_started()
}

/// ディレクトリ `src` を `dest` 直下へコピーすると無限再帰になるかを判定する。
///
/// `Copy-Item -Recurse` は `src` のツリーを走査しつつ `dest/basename(src)` へ複製する。
/// コピー先 `dest/basename(src)` が `src` 自身または `src` 配下にあると、生成された
/// ばかりのフォルダを再び走査対象に拾い `dest/.../basename(src)/...` が無限に増殖する
/// (例: `C:\A\B` 表示中に `C:\A` をドロップ → `C:\A\B\A\B\A...`)。
///
/// 実パスを `canonicalize` で正規化 (失敗時はそのまま) してから判定する。エクスプローラ
/// → mIV のドロップ受け取り (`App::handle_external_file_drop`) で使う。
pub fn dir_copy_would_recurse(src: &Path, dest: &Path) -> bool {
    let canon = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    copy_target_inside_src(&canon(src), &canon(dest))
}

/// `dir_copy_would_recurse` の純粋判定部 (正規化済みパス前提、ユニットテスト用)。
/// コピー先が `src` 自身または `src` 配下なら `true`。
fn copy_target_inside_src(src: &Path, dest: &Path) -> bool {
    // コピー先パス。通常のディレクトリは `dest/basename(src)`。`src` がルート
    // (`C:\` や `\\server\share`、`file_name()` == None) のときは basename が無く
    // コピー先を一意化できないため、`dest` 自体をコピー先領域とみなす — `dest` が
    // ルート配下 (= 同じドライブ / 共有) なら `Copy-Item -Recurse` は生成中の
    // フォルダを再走査して無限再帰する。
    let target = match src.file_name() {
        Some(name) => dest.join(name),
        None => dest.to_path_buf(),
    };
    // Windows はパス大小無視。両辺を小文字化し、コンポーネント単位で前方一致を見る
    // (`Path::starts_with` はコンポーネント境界で判定するので `C:\AB` は `C:\A` 配下に
    // ならない)。
    let lower = |p: &Path| PathBuf::from(p.to_string_lossy().to_lowercase());
    lower(&target).starts_with(lower(src))
}

/// 外部ドロップされたパス群を「コピー対象 (ファイル)」と「skip したフォルダ数」に分ける。
///
/// **フォルダのドロップ受け取りは v1.1.0 で一旦無効化**したため、ディレクトリは全て
/// 除外する (同名衝突の無確認上書き・再帰コピーのデータ破壊リスクのため。将来 Explorer
/// 相当の衝突解決と合わせて再導入する)。`is_dir` を注入可能にしてユニットテストしやすく
/// する (実利用では `|p| p.is_dir()`)。`App::handle_external_file_drop` の worker で使う。
pub fn partition_dropped_paths(
    paths: Vec<PathBuf>,
    mut is_dir: impl FnMut(&Path) -> bool,
) -> (Vec<PathBuf>, usize) {
    let mut files = Vec::with_capacity(paths.len());
    let mut folders_skipped = 0usize;
    for p in paths {
        if is_dir(&p) {
            folders_skipped += 1;
        } else {
            files.push(p);
        }
    }
    (files, folders_skipped)
}

#[cfg(test)]
mod partition_dropped_paths_tests {
    use super::partition_dropped_paths;
    use std::path::PathBuf;

    #[test]
    fn skips_all_directories_keeps_files_in_order() {
        let paths = vec![
            PathBuf::from(r"C:\books\a.jpg"),
            PathBuf::from(r"C:\books\sub"),
            PathBuf::from(r"C:\books\b.png"),
            PathBuf::from(r"C:\books\dir2"),
        ];
        let dirs = [r"C:\books\sub", r"C:\books\dir2"];
        let (files, skipped) =
            partition_dropped_paths(paths, |p| dirs.contains(&p.to_string_lossy().as_ref()));
        assert_eq!(skipped, 2, "両ディレクトリを skip する");
        assert_eq!(
            files,
            vec![
                PathBuf::from(r"C:\books\a.jpg"),
                PathBuf::from(r"C:\books\b.png"),
            ],
            "ファイルだけが順序を保って残る"
        );
    }

    #[test]
    fn all_files_pass_through() {
        let paths = vec![PathBuf::from("a.jpg"), PathBuf::from("b.png")];
        let (files, skipped) = partition_dropped_paths(paths.clone(), |_| false);
        assert_eq!(skipped, 0);
        assert_eq!(files, paths);
    }

    #[test]
    fn all_folders_yield_empty_with_count() {
        let paths = vec![PathBuf::from("d1"), PathBuf::from("d2")];
        let (files, skipped) = partition_dropped_paths(paths, |_| true);
        assert!(files.is_empty());
        assert_eq!(skipped, 2);
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

#[cfg(all(test, windows))]
mod recurse_guard_tests {
    use super::copy_target_inside_src;
    use std::path::Path;

    #[test]
    fn dest_under_src_is_recursive() {
        // C:\A\B 表示中に C:\A をドロップ → コピー先 C:\A\B\A は src 配下。
        assert!(copy_target_inside_src(
            Path::new(r"C:\A"),
            Path::new(r"C:\A\B")
        ));
    }

    #[test]
    fn dropping_folder_onto_itself_is_recursive() {
        assert!(copy_target_inside_src(
            Path::new(r"C:\X\sub"),
            Path::new(r"C:\X\sub")
        ));
    }

    #[test]
    fn dropping_direct_child_back_is_recursive() {
        // C:\X 表示中にその子 C:\X\sub をドロップ → コピー先が src 自身。
        assert!(copy_target_inside_src(
            Path::new(r"C:\X\sub"),
            Path::new(r"C:\X")
        ));
    }

    #[test]
    fn unrelated_folders_are_safe() {
        assert!(!copy_target_inside_src(
            Path::new(r"C:\Y\photos"),
            Path::new(r"C:\X")
        ));
    }

    #[test]
    fn deeper_descendant_is_safe() {
        // C:\X 表示中に C:\X\a\sub をドロップ → コピー先 C:\X\sub は src 配下でない。
        assert!(!copy_target_inside_src(
            Path::new(r"C:\X\a\sub"),
            Path::new(r"C:\X")
        ));
    }

    #[test]
    fn case_insensitive_detection() {
        assert!(copy_target_inside_src(
            Path::new(r"C:\A"),
            Path::new(r"c:\a\b")
        ));
    }

    #[test]
    fn component_boundary_prefix_is_safe() {
        // C:\ABC は C:\A の子ではない (コンポーネント境界)。
        assert!(!copy_target_inside_src(
            Path::new(r"C:\A"),
            Path::new(r"C:\ABC")
        ));
    }

    #[test]
    fn drive_root_into_subdir_is_recursive() {
        // C:\A 表示中に ドライブルート C:\ をドロップ。file_name() == None でも
        // dest が src 配下なので拒否されること。
        assert!(copy_target_inside_src(
            Path::new(r"C:\"),
            Path::new(r"C:\A")
        ));
    }

    #[test]
    fn unc_share_root_into_subdir_is_recursive() {
        // \\server\share 表示中に 共有ルート \\server\share をドロップ。
        assert!(copy_target_inside_src(
            Path::new(r"\\server\share"),
            Path::new(r"\\server\share\A")
        ));
    }

    #[test]
    fn drive_root_into_other_drive_is_safe() {
        // C:\ を D:\X へ → 別ドライブなので自己再帰しない (巨大コピーではあるが
        // 再帰ガードの対象外)。
        assert!(!copy_target_inside_src(
            Path::new(r"C:\"),
            Path::new(r"D:\X")
        ));
    }
}
