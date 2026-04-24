//! ゴミ箱移動のバックグラウンドワーカー。
//!
//! ## 設計
//!
//! 別スレッドで `SHFileOperationW` をファイル単位で呼び、結果を `DeleteMsg` として
//! `mpsc::Receiver` 経由で UI に返す。
//!
//! - **1 ファイルずつ実行**: `SHFileOperationW` の複数パス一括呼び出しは一部のパス
//!   条件下で `result == 0` を返しつつ実際には削除しない症状が再現したため採用せず
//!   (2026-04 の v0.8.1 検証で判明)。perf 差は syscall overhead のみで 10-20ms/file。
//! - **バッチ = UI 進捗粒度**: 10 件ごとに `DeleteMsg::Batch` を送り進捗更新する
//!   (100-200ms おき)。

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;

/// 1 メッセージにまとめる処理件数。UI 進捗更新の粒度。
/// 小さいほど進捗がなめらかに動くが mpsc オーバーヘッドが増える。10 は
/// 「進捗 100-200ms おきに更新 / メッセージ数 ~100」の折衷。
const BATCH_SIZE: usize = 10;

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

/// 削除ワーカーを spawn する。`paths` のファイルをゴミ箱に移動し、進捗を返す。
pub fn spawn(paths: Vec<PathBuf>) -> DeletePending {
    let total = paths.len();
    let cancel = Arc::new(AtomicBool::new(false));
    let (tx, rx) = mpsc::channel();

    let cancel_worker = Arc::clone(&cancel);
    std::thread::Builder::new()
        .name("delete-worker".into())
        .spawn(move || {
            run_worker(paths, cancel_worker, tx);
        })
        .expect("failed to spawn delete worker");

    DeletePending {
        cancel,
        rx,
        total,
        succeeded: Vec::new(),
        failed: Vec::new(),
    }
}

fn run_worker(paths: Vec<PathBuf>, cancel: Arc<AtomicBool>, tx: mpsc::Sender<DeleteMsg>) {
    for chunk in paths.chunks(BATCH_SIZE) {
        if cancel.load(Ordering::Relaxed) {
            let _ = tx.send(DeleteMsg::Done { canceled: true });
            return;
        }

        let mut succeeded = Vec::with_capacity(chunk.len());
        let mut failed = Vec::new();
        for p in chunk {
            if cancel.load(Ordering::Relaxed) {
                let _ = tx.send(DeleteMsg::Batch { succeeded, failed });
                let _ = tx.send(DeleteMsg::Done { canceled: true });
                return;
            }
            match recycle_one(p) {
                Ok(()) => succeeded.push(p.clone()),
                Err(msg) => {
                    crate::logger::log(format!("[delete] failed: {}: {msg}", p.display()));
                    failed.push((p.clone(), msg));
                }
            }
        }
        if tx.send(DeleteMsg::Batch { succeeded, failed }).is_err() {
            return;
        }
    }
    let _ = tx.send(DeleteMsg::Done {
        canceled: cancel.load(Ordering::Relaxed),
    });
}

/// 単一パスを `SHFileOperationW` で削除する。
#[cfg(windows)]
fn recycle_one(path: &std::path::Path) -> Result<(), String> {
    use std::os::windows::ffi::OsStrExt;
    use windows::Win32::UI::Shell::{
        FO_DELETE, FOF_ALLOWUNDO, FOF_NOCONFIRMATION, FOF_NOERRORUI, FOF_SILENT, SHFILEOPSTRUCTW,
        SHFileOperationW,
    };

    let wide: Vec<u16> = path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .chain(std::iter::once(0))
        .collect();
    let flags =
        (FOF_ALLOWUNDO.0 | FOF_NOCONFIRMATION.0 | FOF_SILENT.0 | FOF_NOERRORUI.0) as u16;
    let mut op = SHFILEOPSTRUCTW {
        wFunc: FO_DELETE,
        pFrom: windows::core::PCWSTR(wide.as_ptr()),
        fFlags: flags,
        ..Default::default()
    };
    let result = unsafe { SHFileOperationW(&mut op) };
    if result == 0 && !op.fAnyOperationsAborted.as_bool() {
        Ok(())
    } else {
        Err(format!("SHFileOperationW failed: code={result}"))
    }
}

#[cfg(not(windows))]
fn recycle_one(_path: &std::path::Path) -> Result<(), String> {
    Err("recycle bin not supported on this platform".into())
}
