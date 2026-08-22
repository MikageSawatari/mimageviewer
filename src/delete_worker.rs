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
//! - **メタ purge は worker 末尾で 1 回**: 各チャンクの Shell 成功 path と削除前 PDF path
//!   を蓄積し、キャンセル時も recycle 済みの成功分だけをまとめて hard purge する。

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
        /// SHA-256 キーの pdf_passwords を worker 側で purge した実 PDF path。
        /// UI は disk I/O なしで in-memory store から同じ hash を除く。
        purged_pdf_password_paths: Vec<PathBuf>,
        /// 最終 purge 失敗が永続 journal に保存され、後続 retry が必要。
        purge_deferred: bool,
    },
    /// 全バッチが終わった (キャンセル含む)。`canceled` が true ならユーザーが途中で止めた。
    Done {
        canceled: bool,
        store_mutations: crate::rename_key_migration::StoreMutationEffects,
    },
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
    pub purged_pdf_password_paths: Vec<PathBuf>,
    pub purge_deferred: bool,
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
            purged_pdf_password_paths: Vec::new(),
            purge_deferred: false,
        });
        let _ = tx.send(DeleteMsg::Done {
            canceled: false,
            store_mutations: Default::default(),
        });
    }

    DeletePending {
        cancel,
        rx,
        total,
        succeeded: Vec::new(),
        failed: Vec::new(),
        purged_pdf_password_paths: Vec::new(),
        purge_deferred: false,
    }
}

fn run_worker(
    paths: Vec<PathBuf>,
    hwnd: isize,
    cancel: Arc<AtomicBool>,
    tx: mpsc::Sender<DeleteMsg>,
) {
    let data_dir = crate::data_dir::get();
    let purge_data_dir = data_dir.clone();
    let has_pdf_passwords = crate::pdf_passwords::PdfPasswordStore::has_entries_at(&data_dir);
    run_worker_with_recycler(
        paths,
        hwnd,
        cancel,
        tx,
        has_pdf_passwords,
        recycle_chunk,
        collect_pdf_paths_for_delete,
        |succeeded, pdf_paths| {
            crate::rename_key_migration::purge_removed_paths_at(
                &purge_data_dir,
                succeeded,
                pdf_paths,
            )
        },
        |succeeded, pdf_paths| {
            crate::metadata_cleanup::journal_failed_delete_purge(&data_dir, succeeded, pdf_paths)
        },
    );
}

fn run_worker_with_recycler<F, C, P, J>(
    paths: Vec<PathBuf>,
    hwnd: isize,
    cancel: Arc<AtomicBool>,
    tx: mpsc::Sender<DeleteMsg>,
    collect_pdf_paths: bool,
    mut recycle: F,
    mut collect_pdfs: C,
    mut purge: P,
    mut journal_failure: J,
) where
    F: FnMut(isize, &[PathBuf]) -> DeleteChunkOutcome,
    C: FnMut(&[PathBuf], &AtomicBool) -> Vec<PathBuf>,
    P: FnMut(&[PathBuf], &[PathBuf]) -> crate::rename_key_migration::PurgeReport,
    J: FnMut(&[PathBuf], &[PathBuf]) -> bool,
{
    let mut succeeded_for_purge = Vec::new();
    let mut pdf_paths_for_purge = Vec::new();
    let mut canceled = false;
    let mut receiver_open = true;

    for chunk in paths.chunks(FILE_OPERATION_CHUNK_SIZE) {
        if cancel.load(Ordering::Relaxed) {
            canceled = true;
            break;
        }

        let t0 = std::time::Instant::now();
        // pdf_passwords.json は path hash しか持たず prefix 逆引きできないため、フォルダが
        // Shell で消える前に配下 PDF path を列挙する。保存行が無い場合は走査自体を省く。
        // reparse directory は辿らない。
        let pdf_candidates = if collect_pdf_paths {
            collect_pdfs(chunk, &cancel)
        } else {
            Vec::new()
        };
        if cancel.load(Ordering::Relaxed) {
            canceled = true;
            break;
        }
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
        let purged_pdf_password_paths = pdf_candidates
            .into_iter()
            .filter(|path| path_matches_removed(path, &outcome.succeeded))
            .collect::<Vec<_>>();
        succeeded_for_purge.extend(outcome.succeeded.iter().cloned());
        pdf_paths_for_purge.extend(purged_pdf_password_paths.iter().cloned());
        if tx
            .send(DeleteMsg::Batch {
                succeeded: outcome.succeeded,
                failed: outcome.failed,
                purged_pdf_password_paths,
                purge_deferred: false,
            })
            .is_err()
        {
            receiver_open = false;
            break;
        }
        if cancel.load(Ordering::Relaxed) {
            canceled = true;
            break;
        }
    }
    pdf_paths_for_purge.sort();
    pdf_paths_for_purge.dedup();
    let mut purge_deferred = false;
    let mut store_mutations = crate::rename_key_migration::StoreMutationEffects::default();
    if !succeeded_for_purge.is_empty() {
        let purge_started = std::time::Instant::now();
        let mut attempts = 1usize;
        let mut report = purge(&succeeded_for_purge, &pdf_paths_for_purge);
        store_mutations.merge(report.store_mutations);
        let mut rows = report.rows;
        let mut db_open_count = report.db_open_count;
        for retry in 1..=3 {
            if report.errors.is_empty() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(50 * retry));
            report = purge(&succeeded_for_purge, &pdf_paths_for_purge);
            store_mutations.merge(report.store_mutations);
            attempts += 1;
            rows += report.rows;
            db_open_count += report.db_open_count;
        }
        let purge_ms = purge_started.elapsed().as_secs_f64() * 1000.0;
        crate::logger::log(format!(
            "[delete] metadata hard purge rows={rows} removed={} errors={} attempts={attempts} db_opens={db_open_count} ms={purge_ms:.1}",
            succeeded_for_purge.len(),
            report.errors.len()
        ));
        if crate::perf::is_enabled() {
            crate::perf::event(
                "delete",
                "metadata_purge",
                Some("worker_tail"),
                0,
                &[
                    ("ms", serde_json::Value::from(purge_ms)),
                    (
                        "removed",
                        serde_json::Value::from(succeeded_for_purge.len() as u64),
                    ),
                    (
                        "pdf_paths",
                        serde_json::Value::from(pdf_paths_for_purge.len() as u64),
                    ),
                    ("attempts", serde_json::Value::from(attempts as u64)),
                    ("db_opens", serde_json::Value::from(db_open_count as u64)),
                    ("rows", serde_json::Value::from(rows as u64)),
                    (
                        "errors",
                        serde_json::Value::from(report.errors.len() as u64),
                    ),
                ],
            );
        }
        if !report.errors.is_empty() {
            purge_deferred = journal_failure(&succeeded_for_purge, &pdf_paths_for_purge);
            crate::logger::log(format!(
                "[delete] metadata hard purge deferred journaled={purge_deferred}"
            ));
        }
        for error in report.errors {
            crate::logger::log(format!("[delete] metadata hard purge failed: {error}"));
        }
    }

    if receiver_open && purge_deferred {
        receiver_open = tx
            .send(DeleteMsg::Batch {
                succeeded: Vec::new(),
                failed: Vec::new(),
                purged_pdf_password_paths: Vec::new(),
                purge_deferred: true,
            })
            .is_ok();
    }
    if receiver_open {
        let _ = tx.send(DeleteMsg::Done {
            canceled: canceled || cancel.load(Ordering::Relaxed),
            store_mutations,
        });
    }
}

fn path_matches_removed(path: &std::path::Path, removed: &[PathBuf]) -> bool {
    let key = crate::adjustment_db::normalize_path(path);
    removed.iter().any(|removed_path| {
        let removed_key = crate::adjustment_db::normalize_path(removed_path);
        key == removed_key
            || key.starts_with(&format!("{removed_key}/"))
            || key.starts_with(&format!("{removed_key}::"))
    })
}

/// pdf_passwords.json は path の SHA-256 hash しか保持しないため、フォルダ削除の prefix
/// purge に必要な PDF path を削除前に集める。I/O は delete worker 上で行い、junction / symlink
/// directory は辿らない。
fn collect_pdf_paths_for_delete(paths: &[PathBuf], cancel: &AtomicBool) -> Vec<PathBuf> {
    let mut pdfs = Vec::new();
    let mut stack = paths.to_vec();
    while let Some(path) = stack.pop() {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        if path
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("pdf"))
        {
            pdfs.push(path.clone());
        }
        let Ok(entries) = std::fs::read_dir(&path) else {
            continue;
        };
        for entry in entries.flatten() {
            if cancel.load(Ordering::Relaxed) {
                break;
            }
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            match crate::fs_entry::classify_dir_entry(&entry, &file_type) {
                crate::fs_entry::DirEntryKind::Directory => stack.push(entry.path()),
                crate::fs_entry::DirEntryKind::File => {
                    let child = entry.path();
                    if child
                        .extension()
                        .is_some_and(|extension| extension.eq_ignore_ascii_case("pdf"))
                    {
                        pdfs.push(child);
                    }
                }
                crate::fs_entry::DirEntryKind::ReparseDirectory
                | crate::fs_entry::DirEntryKind::Other => {}
            }
        }
    }
    pdfs.sort();
    pdfs.dedup();
    pdfs
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
            DeleteMsg::Batch {
                succeeded, failed, ..
            } => (succeeded.len(), failed.len()),
            DeleteMsg::Done { .. } => panic!("expected Batch"),
        }
    }

    fn done_canceled(msg: &DeleteMsg) -> bool {
        match msg {
            DeleteMsg::Done { canceled, .. } => *canceled,
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

        run_worker_with_recycler(
            paths,
            0,
            cancel,
            tx,
            false,
            |_hwnd, chunk| {
                calls += 1;
                failed_outcome(chunk, false)
            },
            |_paths, _cancel| Vec::new(),
            |_succeeded, _pdf_paths| crate::rename_key_migration::PurgeReport::default(),
            |_succeeded, _pdf_paths| false,
        );

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

        run_worker_with_recycler(
            paths,
            0,
            cancel,
            tx,
            false,
            |_hwnd, chunk| {
                calls += 1;
                cancel_for_recycler.store(true, Ordering::Relaxed);
                failed_outcome(chunk, false)
            },
            |_paths, _cancel| Vec::new(),
            |_succeeded, _pdf_paths| crate::rename_key_migration::PurgeReport::default(),
            |_succeeded, _pdf_paths| false,
        );

        let messages: Vec<_> = rx.try_iter().collect();
        assert_eq!(calls, 1, "mIV cancel should stop before the next chunk");
        assert_eq!(messages.len(), 2);
        assert_eq!(batch_counts(&messages[0]), (0, FILE_OPERATION_CHUNK_SIZE));
        assert!(done_canceled(&messages[1]));
    }

    #[test]
    fn worker_purges_all_succeeded_chunks_once_at_tail() {
        let paths = dummy_paths(FILE_OPERATION_CHUNK_SIZE * 2 + 1);
        let expected = paths.clone();
        let (tx, rx) = mpsc::channel();
        let mut recycle_calls = 0usize;
        let mut purge_calls = 0usize;
        let mut purged = Vec::new();

        run_worker_with_recycler(
            paths,
            0,
            Arc::new(AtomicBool::new(false)),
            tx,
            false,
            |_hwnd, chunk| {
                recycle_calls += 1;
                DeleteChunkOutcome {
                    succeeded: chunk.to_vec(),
                    failed: Vec::new(),
                    shell_aborted: false,
                    retryable: false,
                }
            },
            |_paths, _cancel| Vec::new(),
            |removed, _pdf_paths| {
                purge_calls += 1;
                purged.extend_from_slice(removed);
                crate::rename_key_migration::PurgeReport::default()
            },
            |_succeeded, _pdf_paths| false,
        );

        assert_eq!(recycle_calls, 3);
        assert_eq!(purge_calls, 1, "purge must not scale with chunk count");
        assert_eq!(purged, expected);
        let messages = rx.try_iter().collect::<Vec<_>>();
        assert_eq!(messages.len(), 4);
        assert!(!done_canceled(&messages[3]));
    }

    #[test]
    fn cancellation_purges_only_already_recycled_successes() {
        let paths = dummy_paths(FILE_OPERATION_CHUNK_SIZE + 1);
        let expected = paths[..FILE_OPERATION_CHUNK_SIZE].to_vec();
        let cancel = Arc::new(AtomicBool::new(false));
        let cancel_for_recycler = Arc::clone(&cancel);
        let (tx, rx) = mpsc::channel();
        let mut recycle_calls = 0usize;
        let mut purge_calls = 0usize;
        let mut purged = Vec::new();

        run_worker_with_recycler(
            paths,
            0,
            cancel,
            tx,
            false,
            |_hwnd, chunk| {
                recycle_calls += 1;
                cancel_for_recycler.store(true, Ordering::Relaxed);
                DeleteChunkOutcome {
                    succeeded: chunk.to_vec(),
                    failed: Vec::new(),
                    shell_aborted: false,
                    retryable: false,
                }
            },
            |_paths, _cancel| Vec::new(),
            |removed, _pdf_paths| {
                purge_calls += 1;
                purged.extend_from_slice(removed);
                crate::rename_key_migration::PurgeReport::default()
            },
            |_succeeded, _pdf_paths| false,
        );

        assert_eq!(recycle_calls, 1);
        assert_eq!(purge_calls, 1);
        assert_eq!(purged, expected);
        let messages = rx.try_iter().collect::<Vec<_>>();
        assert_eq!(messages.len(), 2);
        assert_eq!(batch_counts(&messages[0]), (FILE_OPERATION_CHUNK_SIZE, 0));
        assert!(done_canceled(&messages[1]));
    }

    #[test]
    fn empty_pdf_password_store_skips_pdf_tree_collection() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(temp.path().join("pdf_passwords.json"), b"{}").unwrap();
        let has_pdf_passwords = crate::pdf_passwords::PdfPasswordStore::has_entries_at(temp.path());
        assert!(!has_pdf_passwords);
        let root = temp.path().join("folder");
        std::fs::create_dir(&root).unwrap();
        std::fs::write(root.join("nested.pdf"), b"pdf").unwrap();
        let (tx, _rx) = mpsc::channel();
        let mut collect_calls = 0usize;
        let mut purge_pdf_paths = Vec::new();

        run_worker_with_recycler(
            vec![root],
            0,
            Arc::new(AtomicBool::new(false)),
            tx,
            has_pdf_passwords,
            |_hwnd, chunk| DeleteChunkOutcome {
                succeeded: chunk.to_vec(),
                failed: Vec::new(),
                shell_aborted: false,
                retryable: false,
            },
            |paths, cancel| {
                collect_calls += 1;
                collect_pdf_paths_for_delete(paths, cancel)
            },
            |_removed, pdf_paths| {
                purge_pdf_paths.extend_from_slice(pdf_paths);
                crate::rename_key_migration::PurgeReport::default()
            },
            |_succeeded, _pdf_paths| false,
        );

        assert_eq!(collect_calls, 0, "read_dir traversal must be skipped");
        assert!(purge_pdf_paths.is_empty());
        std::fs::write(
            temp.path().join("pdf_passwords.json"),
            br#"{"hash":"cipher"}"#,
        )
        .unwrap();
        assert!(crate::pdf_passwords::PdfPasswordStore::has_entries_at(
            temp.path()
        ));
    }

    #[test]
    fn pdf_candidates_include_nested_files_without_neighbor_prefixes() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("book");
        std::fs::create_dir_all(root.join("nested")).unwrap();
        let top = root.join("top.pdf");
        let nested = root.join("nested").join("inside.PDF");
        std::fs::write(&top, b"pdf").unwrap();
        std::fs::write(&nested, b"pdf").unwrap();
        std::fs::write(root.join("keep.jpg"), b"jpg").unwrap();

        let mut found =
            collect_pdf_paths_for_delete(std::slice::from_ref(&root), &AtomicBool::new(false));
        found.sort();
        let mut expected = vec![top, nested];
        expected.sort();
        assert_eq!(found, expected);
        assert!(path_matches_removed(&found[0], std::slice::from_ref(&root)));
        assert!(!path_matches_removed(
            &temp.path().join("book2").join("other.pdf"),
            std::slice::from_ref(&root)
        ));
    }

    #[test]
    fn worker_hard_purges_only_shell_succeeded_paths() {
        let temp = tempfile::tempdir().unwrap();
        let succeeded = PathBuf::from(r"C:\pics\gone.jpg");
        let failed = PathBuf::from(r"C:\pics\keep.jpg");
        let db = crate::rating_db::RatingDb::open_at(temp.path().join("rating.db")).unwrap();
        let succeeded_key = crate::adjustment_db::normalize_path(&succeeded);
        let failed_key = crate::adjustment_db::normalize_path(&failed);
        db.set(&succeeded_key, 5).unwrap();
        db.set(&failed_key, 4).unwrap();

        let (tx, rx) = mpsc::channel();
        run_worker_with_recycler(
            vec![succeeded.clone(), failed.clone()],
            0,
            Arc::new(AtomicBool::new(false)),
            tx,
            false,
            |_hwnd, _chunk| DeleteChunkOutcome {
                succeeded: vec![succeeded.clone()],
                failed: vec![(failed.clone(), "simulated failure".to_string())],
                shell_aborted: false,
                retryable: false,
            },
            |_paths, _cancel| Vec::new(),
            |removed, pdf_paths| {
                crate::rename_key_migration::purge_removed_paths_at(temp.path(), removed, pdf_paths)
            },
            |_succeeded, _pdf_paths| false,
        );

        let messages = rx.try_iter().collect::<Vec<_>>();
        assert_eq!(batch_counts(&messages[0]), (1, 1));
        assert_eq!(db.get(&succeeded_key), 0);
        assert_eq!(db.get(&failed_key), 4);
    }

    #[test]
    fn final_purge_failure_is_persisted_for_idle_retry() {
        let temp = tempfile::tempdir().unwrap();
        let data_dir = temp.path().join("data");
        let files = temp.path().join("files");
        std::fs::create_dir(&data_dir).unwrap();
        std::fs::create_dir(&files).unwrap();
        let removed = files.join("gone.jpg");
        let (tx, rx) = mpsc::channel();
        let mut purge_calls = 0usize;
        let journal_data_dir = data_dir.clone();

        run_worker_with_recycler(
            vec![removed.clone()],
            0,
            Arc::new(AtomicBool::new(false)),
            tx,
            false,
            |_hwnd, _chunk| DeleteChunkOutcome {
                succeeded: vec![removed.clone()],
                failed: Vec::new(),
                shell_aborted: false,
                retryable: false,
            },
            |_paths, _cancel| Vec::new(),
            |_removed, _pdf_paths| {
                purge_calls += 1;
                crate::rename_key_migration::PurgeReport {
                    rows: 0,
                    db_open_count: 0,
                    errors: vec!["simulated database lock".into()],
                    store_mutations: Default::default(),
                }
            },
            |removed, pdf_paths| {
                crate::metadata_cleanup::journal_failed_delete_purge(
                    &journal_data_dir,
                    removed,
                    pdf_paths,
                )
            },
        );

        assert_eq!(purge_calls, 4, "initial attempt plus three retries");
        let messages = rx.try_iter().collect::<Vec<_>>();
        assert!(messages.iter().any(|message| matches!(
            message,
            DeleteMsg::Batch {
                purge_deferred: true,
                ..
            }
        )));
        assert!(
            data_dir
                .join(crate::metadata_cleanup::DELETE_PURGE_JOURNAL_FILE)
                .exists()
        );
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
