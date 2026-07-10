//! ゴミ箱移動のバックグラウンドワーカー。
//!
//! ## 設計
//!
//! 別スレッドで Windows Shell の `IFileOperation` をチャンク単位で呼び、結果を
//! `DeleteMsg` として `mpsc::Receiver` 経由で UI に返す。
//!
//! - **チャンク単位で実行**: 旧 `SHFileOperationW` の複数パス一括呼び出しは一部のパス
//!   条件下で `result == 0` を返しつつ実際には削除しない症状が再現したため採用しない。
//!   Vista+ の後継 API である `IFileOperation` に対象をまとめて予約し、
//!   `PerformOperations` をチャンクごとに 1 回だけ呼ぶ。
//! - **チャンク = cancel / UI 進捗粒度**: チャンク完了ごとに `DeleteMsg::Batch` を送り、
//!   チャンク間で mIV 側キャンセルを受け付ける。Shell 側の中断はチャンク内の
//!   失敗として扱い、未処理チャンクを捨てない。

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;

/// 1 回の `IFileOperation::PerformOperations` にまとめる最大件数。
/// 大きいほど Shell の固定費を畳めるが、mIV 側キャンセルはチャンク間でしか効かない。
const FILE_OPERATION_CHUNK_SIZE: usize = 100;

/// viewer close 後の decoder teardown を待つための Shell 再試行上限。
const DELETE_RETRY_LIMIT: usize = 5;
const DELETE_RETRY_BACKOFF: std::time::Duration = std::time::Duration::from_millis(200);

/// ワーカーから UI への進捗通知。
#[derive(Debug)]
pub enum DeleteMsg {
    /// 1 バッチ分の結果。`succeeded` と `failed` の和は必ずバッチに含めた件数に一致する。
    /// UI はこれを集計して進捗表示を更新するだけで、items への反映は `Done` 受信後に行う。
    Batch {
        succeeded: Vec<PathBuf>,
        failed: Vec<(PathBuf, String)>,
    },
    /// 全バッチが終わった (キャンセル含む)。`canceled` が true ならユーザーが途中で止めた。
    Done { canceled: bool },
}

/// UI スレッドが保持する worker ハンドル。
pub struct DeletePending {
    pub cancel: Arc<AtomicBool>,
    pub rx: mpsc::Receiver<DeleteMsg>,
    /// 削除要求を出した時点の総件数。進捗表示の分母。
    pub total: usize,
    /// これまでに成功扱いになったパス (items から抜く対象)。
    pub succeeded: Vec<PathBuf>,
    /// これまでに失敗したパスとエラーメッセージ (トースト / ログ通知用)。
    pub failed: Vec<(PathBuf, String)>,
}

impl DeletePending {
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }

    /// 処理済み件数 (成功 + 失敗)。進捗バーの分子。
    pub fn processed(&self) -> usize {
        self.succeeded.len() + self.failed.len()
    }
}

/// 削除ワーカーを spawn する。`paths` のファイル / フォルダをゴミ箱に移動し、進捗を返す。
pub fn spawn(paths: Vec<PathBuf>, hwnd: Option<isize>) -> DeletePending {
    let total = paths.len();
    let cancel = Arc::new(AtomicBool::new(false));
    let (tx, rx) = mpsc::channel();
    let paths_for_error = paths.clone();

    let cancel_worker = Arc::clone(&cancel);
    let tx_worker = tx.clone();
    let spawn_result = std::thread::Builder::new()
        .name("delete-worker".into())
        .spawn(move || {
            run_worker(paths, hwnd.unwrap_or_default(), cancel_worker, tx_worker);
        });
    if let Err(e) = spawn_result {
        let message = format!("削除 worker を開始できません: {e}");
        crate::logger::log(format!("[delete] worker spawn failed: {e}"));
        let failed = paths_for_error
            .into_iter()
            .map(|path| (path, message.clone()))
            .collect();
        let _ = tx.send(DeleteMsg::Batch {
            succeeded: Vec::new(),
            failed,
        });
        let _ = tx.send(DeleteMsg::Done { canceled: false });
    }

    DeletePending {
        cancel,
        rx,
        total,
        succeeded: Vec::new(),
        failed: Vec::new(),
    }
}

fn run_worker(
    paths: Vec<PathBuf>,
    hwnd: isize,
    cancel: Arc<AtomicBool>,
    tx: mpsc::Sender<DeleteMsg>,
) {
    run_worker_with_recycler(paths, hwnd, cancel, tx, recycle_chunk);
}

fn run_worker_with_recycler<F>(
    paths: Vec<PathBuf>,
    hwnd: isize,
    cancel: Arc<AtomicBool>,
    tx: mpsc::Sender<DeleteMsg>,
    mut recycle: F,
) where
    F: FnMut(isize, &[PathBuf]) -> DeleteChunkOutcome,
{
    for chunk in paths.chunks(FILE_OPERATION_CHUNK_SIZE) {
        if cancel.load(Ordering::Relaxed) {
            let _ = tx.send(DeleteMsg::Done { canceled: true });
            return;
        }

        let t0 = std::time::Instant::now();
        let outcome = recycle_chunk_with_retry(hwnd, chunk, &cancel, &mut recycle);
        if crate::perf::is_enabled() {
            crate::perf::event(
                "delete",
                "shell_chunk",
                Some("ifileoperation"),
                0,
                &[
                    (
                        "ms",
                        serde_json::Value::from(t0.elapsed().as_secs_f64() * 1000.0),
                    ),
                    ("count", serde_json::Value::from(chunk.len() as u64)),
                    (
                        "succeeded",
                        serde_json::Value::from(outcome.succeeded.len() as u64),
                    ),
                    (
                        "failed",
                        serde_json::Value::from(outcome.failed.len() as u64),
                    ),
                    (
                        "shell_aborted",
                        serde_json::Value::from(outcome.shell_aborted),
                    ),
                ],
            );
        }

        for (path, msg) in &outcome.failed {
            crate::logger::log(format!("[delete] failed: {}: {msg}", path.display()));
        }
        if tx
            .send(DeleteMsg::Batch {
                succeeded: outcome.succeeded,
                failed: outcome.failed,
            })
            .is_err()
        {
            return;
        }
        if cancel.load(Ordering::Relaxed) {
            let _ = tx.send(DeleteMsg::Done { canceled: true });
            return;
        }
    }
    let _ = tx.send(DeleteMsg::Done {
        canceled: cancel.load(Ordering::Relaxed),
    });
}

struct DeleteChunkOutcome {
    succeeded: Vec<PathBuf>,
    failed: Vec<(PathBuf, String)>,
    shell_aborted: bool,
    retryable: bool,
}

impl DeleteChunkOutcome {
    fn all_failed(paths: &[PathBuf], message: String) -> Self {
        Self {
            succeeded: Vec::new(),
            failed: paths
                .iter()
                .cloned()
                .map(|path| (path, message.clone()))
                .collect(),
            shell_aborted: false,
            retryable: false,
        }
    }
}

fn recycle_chunk_with_retry<F>(
    hwnd: isize,
    paths: &[PathBuf],
    cancel: &AtomicBool,
    recycle: &mut F,
) -> DeleteChunkOutcome
where
    F: FnMut(isize, &[PathBuf]) -> DeleteChunkOutcome,
{
    let mut pending = paths.to_vec();
    let mut succeeded = Vec::new();
    for retry in 0..=DELETE_RETRY_LIMIT {
        let outcome = recycle(hwnd, &pending);
        succeeded.extend(outcome.succeeded);
        if outcome.failed.is_empty() {
            return DeleteChunkOutcome {
                succeeded,
                failed: Vec::new(),
                shell_aborted: outcome.shell_aborted,
                retryable: false,
            };
        }
        if !outcome.retryable || retry == DELETE_RETRY_LIMIT || cancel.load(Ordering::Relaxed) {
            return DeleteChunkOutcome {
                succeeded,
                failed: outcome.failed,
                shell_aborted: outcome.shell_aborted,
                retryable: outcome.retryable,
            };
        }
        if wait_retry_backoff(cancel) {
            return DeleteChunkOutcome {
                succeeded,
                failed: outcome.failed,
                shell_aborted: outcome.shell_aborted,
                retryable: true,
            };
        }
        // 成功済み項目は再投入せず、残った失敗項目だけを新しい IFileOperation で試す。
        pending = outcome.failed.into_iter().map(|(path, _)| path).collect();
    }
    unreachable!()
}

fn wait_retry_backoff(cancel: &AtomicBool) -> bool {
    let deadline = std::time::Instant::now() + DELETE_RETRY_BACKOFF;
    while std::time::Instant::now() < deadline {
        if cancel.load(Ordering::Relaxed) {
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    cancel.load(Ordering::Relaxed)
}

fn is_retryable_shell_failure(shell_aborted: bool, hresult: Option<i32>) -> bool {
    const WIN32_SHARING: i32 = 0x8007_0020_u32 as i32;
    const WIN32_LOCK: i32 = 0x8007_0021_u32 as i32;
    const COPYENGINE_SHARING_SRC: i32 = 0x8027_0027_u32 as i32;
    const COPYENGINE_SHARING_DEST: i32 = 0x8027_0028_u32 as i32;
    shell_aborted
        || matches!(
            hresult,
            Some(WIN32_SHARING | WIN32_LOCK | COPYENGINE_SHARING_SRC | COPYENGINE_SHARING_DEST)
        )
}

/// `IFileOperation` (削除) のフラグ。**FOF_WANTNUKEWARNING を必ず含める**こと。
/// 含めないと、ゴミ箱に入れられない対象 (容量超過 / ゴミ箱無効ボリューム / リムーバブル /
/// ネットワーク共有) で完全削除へフォールバックする際の警告が出ない構成へ退行しうる。
/// mIV が削除確認 / 進捗 UI を持つため通常の Shell UI は抑制するが、
/// `FOF_WANTNUKEWARNING` は `FOF_NOCONFIRMATION` を部分的に上書きし、
/// ゴミ箱へ入らない対象が完全削除される前の警告を残す。
#[cfg(windows)]
fn recycle_flags() -> windows::Win32::UI::Shell::FILEOPERATION_FLAGS {
    use windows::Win32::UI::Shell::{
        FOF_ALLOWUNDO, FOF_NOCONFIRMATION, FOF_NOERRORUI, FOF_SILENT, FOF_WANTNUKEWARNING,
        FOFX_ADDUNDORECORD, FOFX_RECYCLEONDELETE,
    };
    FOF_ALLOWUNDO
        | FOFX_RECYCLEONDELETE
        | FOFX_ADDUNDORECORD
        | FOF_WANTNUKEWARNING
        | FOF_NOCONFIRMATION
        | FOF_NOERRORUI
        | FOF_SILENT
}

/// 複数パス (ファイルまたはフォルダ) を 1 回の `IFileOperation` で削除する。
#[cfg(windows)]
fn recycle_chunk(hwnd: isize, paths: &[PathBuf]) -> DeleteChunkOutcome {
    use windows::Win32::Foundation::{HWND, RPC_E_CHANGED_MODE};
    use windows::Win32::System::Com::{
        CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx,
        CoUninitialize,
    };
    use windows::Win32::UI::Shell::{
        FileOperation, IFileOperation, IFileOperationProgressSink, IShellItem,
        SHCreateItemFromParsingName,
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

    let _com = match ComStaGuard::new() {
        Ok(guard) => guard,
        Err(err) => return DeleteChunkOutcome::all_failed(paths, err),
    };
    let op: IFileOperation = match unsafe {
        CoCreateInstance(&FileOperation, None::<&IUnknown>, CLSCTX_INPROC_SERVER)
    } {
        Ok(op) => op,
        Err(e) => {
            return DeleteChunkOutcome::all_failed(
                paths,
                format!("IFileOperation を作成できません: {e}"),
            );
        }
    };

    if hwnd != 0
        && let Err(e) = unsafe { op.SetOwnerWindow(HWND(hwnd as *mut core::ffi::c_void)) }
    {
        return DeleteChunkOutcome::all_failed(
            paths,
            format!("Shell 操作の owner window を設定できません: {e}"),
        );
    }
    if let Err(e) = unsafe { op.SetOperationFlags(recycle_flags()) } {
        return DeleteChunkOutcome::all_failed(
            paths,
            format!("Shell 操作フラグを設定できません: {e}"),
        );
    }

    let mut succeeded = Vec::new();
    let mut failed = Vec::new();
    let mut scheduled = Vec::new();
    for path in paths {
        let wide = wide_null_path(path);
        let item: IShellItem = match unsafe {
            SHCreateItemFromParsingName(
                PCWSTR(wide.as_ptr()),
                None::<&windows::Win32::System::Com::IBindCtx>,
            )
        } {
            Ok(item) => item,
            Err(e) => {
                if path_is_gone(path) {
                    succeeded.push(path.clone());
                } else {
                    failed.push((path.clone(), format!("対象を開けません: {e}")));
                }
                continue;
            }
        };
        if let Err(e) = unsafe { op.DeleteItem(&item, None::<&IFileOperationProgressSink>) } {
            if path_is_gone(path) {
                succeeded.push(path.clone());
            } else {
                failed.push((path.clone(), format!("削除を予約できません: {e}")));
            }
            continue;
        }
        scheduled.push(path.clone());
    }

    if scheduled.is_empty() {
        return DeleteChunkOutcome {
            succeeded,
            failed,
            shell_aborted: false,
            retryable: false,
        };
    }

    let perform_error = unsafe { op.PerformOperations() }.err();
    let shell_aborted = unsafe { op.GetAnyOperationsAborted() }
        .map(|v| v.as_bool())
        .unwrap_or(false);
    let retryable = is_retryable_shell_failure(
        shell_aborted,
        perform_error.as_ref().map(|error| error.code().0),
    );

    for path in scheduled {
        if path_is_gone(&path) {
            succeeded.push(path);
        } else {
            let msg = if let Some(e) = &perform_error {
                format!("削除に失敗しました: {e}")
            } else if shell_aborted {
                "Shell 操作が中断されました".to_string()
            } else {
                "削除後も対象が残っています".to_string()
            };
            failed.push((path, msg));
        }
    }

    DeleteChunkOutcome {
        succeeded,
        failed,
        shell_aborted,
        retryable,
    }
}

#[cfg(not(windows))]
fn recycle_chunk(_hwnd: isize, paths: &[PathBuf]) -> DeleteChunkOutcome {
    DeleteChunkOutcome::all_failed(
        paths,
        "recycle bin not supported on this platform".to_string(),
    )
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
fn path_is_gone(path: &std::path::Path) -> bool {
    path.try_exists().map(|exists| !exists).unwrap_or(false)
}

#[cfg(test)]
mod worker_tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    fn dummy_paths(count: usize) -> Vec<PathBuf> {
        (0..count)
            .map(|idx| PathBuf::from(format!(r"C:\tmp\delete-test-{idx}.jpg")))
            .collect()
    }

    fn failed_outcome(paths: &[PathBuf], shell_aborted: bool) -> DeleteChunkOutcome {
        DeleteChunkOutcome {
            succeeded: Vec::new(),
            failed: paths
                .iter()
                .cloned()
                .map(|path| (path, "simulated failure".to_string()))
                .collect(),
            shell_aborted,
            retryable: shell_aborted,
        }
    }

    fn batch_counts(msg: &DeleteMsg) -> (usize, usize) {
        match msg {
            DeleteMsg::Batch { succeeded, failed } => (succeeded.len(), failed.len()),
            DeleteMsg::Done { .. } => panic!("expected Batch"),
        }
    }

    fn done_canceled(msg: &DeleteMsg) -> bool {
        match msg {
            DeleteMsg::Done { canceled } => *canceled,
            DeleteMsg::Batch { .. } => panic!("expected Done"),
        }
    }

    #[test]
    fn retry_decision_accepts_abort_and_sharing_hresult() {
        assert!(is_retryable_shell_failure(true, None));
        assert!(is_retryable_shell_failure(
            false,
            Some(0x8007_0020_u32 as i32)
        ));
        assert!(is_retryable_shell_failure(
            false,
            Some(0x8027_0027_u32 as i32)
        ));
        assert!(!is_retryable_shell_failure(false, None));
        assert!(!is_retryable_shell_failure(
            false,
            Some(0x8007_0005_u32 as i32)
        ));
    }

    #[test]
    fn retry_requeues_only_failed_paths() {
        let paths = dummy_paths(2);
        let cancel = AtomicBool::new(false);
        let mut calls = 0;
        let outcome = recycle_chunk_with_retry(0, &paths, &cancel, &mut |_hwnd, chunk| {
            calls += 1;
            if calls == 1 {
                DeleteChunkOutcome {
                    succeeded: vec![chunk[0].clone()],
                    failed: vec![(chunk[1].clone(), String::new())],
                    shell_aborted: true,
                    retryable: true,
                }
            } else {
                DeleteChunkOutcome {
                    succeeded: chunk.to_vec(),
                    failed: Vec::new(),
                    shell_aborted: false,
                    retryable: false,
                }
            }
        });
        assert_eq!(calls, 2);
        assert_eq!(outcome.succeeded, paths);
        assert!(outcome.failed.is_empty());
    }

    #[test]
    fn worker_continues_after_failed_chunk() {
        let paths = dummy_paths(FILE_OPERATION_CHUNK_SIZE + 1);
        let cancel = Arc::new(AtomicBool::new(false));
        let (tx, rx) = mpsc::channel();
        let mut calls = 0usize;

        run_worker_with_recycler(paths, 0, cancel, tx, |_hwnd, chunk| {
            calls += 1;
            failed_outcome(chunk, false)
        });

        let messages: Vec<_> = rx.try_iter().collect();
        assert_eq!(calls, 2, "shell_aborted chunk must not stop later chunks");
        assert_eq!(messages.len(), 3);
        assert_eq!(batch_counts(&messages[0]), (0, FILE_OPERATION_CHUNK_SIZE));
        assert_eq!(batch_counts(&messages[1]), (0, 1));
        assert!(!done_canceled(&messages[2]));
    }

    #[test]
    fn worker_stops_after_miv_cancel_at_chunk_boundary() {
        let paths = dummy_paths(FILE_OPERATION_CHUNK_SIZE + 1);
        let cancel = Arc::new(AtomicBool::new(false));
        let cancel_for_recycler = Arc::clone(&cancel);
        let (tx, rx) = mpsc::channel();
        let mut calls = 0usize;

        run_worker_with_recycler(paths, 0, cancel, tx, |_hwnd, chunk| {
            calls += 1;
            cancel_for_recycler.store(true, Ordering::Relaxed);
            failed_outcome(chunk, false)
        });

        let messages: Vec<_> = rx.try_iter().collect();
        assert_eq!(calls, 1, "mIV cancel should stop before the next chunk");
        assert_eq!(messages.len(), 2);
        assert_eq!(batch_counts(&messages[0]), (0, FILE_OPERATION_CHUNK_SIZE));
        assert!(done_canceled(&messages[1]));
    }
}

#[cfg(all(test, windows))]
mod tests {
    use windows::Win32::UI::Shell::{
        FOF_NOCONFIRMATION, FOF_NOERRORUI, FOF_SILENT, FOF_WANTNUKEWARNING, FOFX_RECYCLEONDELETE,
    };

    #[test]
    fn recycle_flags_include_nuke_warning() {
        // DI-1 回帰ガード: FOF_WANTNUKEWARNING が外れると、ゴミ箱不可時に無言で完全削除
        // されるようになってしまう。削除フラグから外さないこと。
        assert_ne!(
            super::recycle_flags().0 & FOF_WANTNUKEWARNING.0,
            0,
            "FOF_WANTNUKEWARNING must stay set so non-recyclable targets prompt before permanent delete"
        );
    }

    #[test]
    fn recycle_flags_request_recycle_with_miv_owned_ui() {
        let flags = super::recycle_flags().0;
        assert_ne!(
            flags & FOFX_RECYCLEONDELETE.0,
            0,
            "IFileOperation delete must target the recycle bin"
        );
        let miv_owned_ui_flags = FOF_NOCONFIRMATION.0 | FOF_NOERRORUI.0 | FOF_SILENT.0;
        assert_eq!(
            flags & miv_owned_ui_flags,
            miv_owned_ui_flags,
            "mIV owns delete confirmation/progress UI; Shell UI should stay quiet except the nuke warning"
        );
    }
}
