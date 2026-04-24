//! ゴミ箱移動のバックグラウンドワーカー。
//!
//! ## 設計
//!
//! - 同期版は UI スレッドで `SHFileOperationW` を呼んでいたため、★1 画像を数千件
//!   一括削除するケース (AI 画像整理ワークフロー) で UI が数秒〜数十秒フリーズしていた。
//! - 本モジュールは別スレッドで削除を実行し、結果を `DeleteMsg` として
//!   `mpsc::Receiver` 経由で UI に返す。
//! - **バッチ戦略**: `SHFileOperationW` は NULL 区切りの複数パスを 1 コールで
//!   処理できるので、通常は 50 件まとめて 1 コールする (syscall コストを抑える)。
//!   バッチが失敗した場合だけ、そのバッチ内のファイルを 1 件ずつ個別に再試行して
//!   「一部は成功したが他は失敗」のケースを正確に把握する。
//! - **キャンセル粒度**: バッチ境界。実行中の `SHFileOperationW` は OS 側で
//!   中断できないので、キャンセルボタンは「次バッチ以降を止める」意味。UI 側でも
//!   その前提で文言を出す。
//! - **世代防御**: UI 側は開始時の `items_generation` をスナップショットして、
//!   完了時に現在値と比較する。不一致ならゴミ箱移動済みの結果を現在の items には
//!   適用しない (path 解決が自然に空振りする + 過剰な idx shift を防ぐ)。

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;

/// `SHFileOperationW` に 1 回で渡すパス数。失敗時はバッチ内を 1 件ずつ再試行するので、
/// 大きすぎると失敗時のフォールバックコストが増える。50 は実測ベースではなく
/// 「1 バッチ 50〜100ms 目安、キャンセル応答性も確保」のバランス。
const BATCH_SIZE: usize = 50;

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
    /// これまでに処理された件数 (成功 + 失敗)。分子。
    pub processed: usize,
    /// 開始時点の `items_generation` スナップショット。完了適用時の世代防御に使う。
    pub items_generation: u64,
}

impl DeletePending {
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

/// 削除ワーカーを spawn する。`paths` のファイルをゴミ箱に移動し、進捗を返す。
pub fn spawn(paths: Vec<PathBuf>, items_generation: u64) -> DeletePending {
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
        processed: 0,
        items_generation,
    }
}

fn run_worker(paths: Vec<PathBuf>, cancel: Arc<AtomicBool>, tx: mpsc::Sender<DeleteMsg>) {
    for chunk in paths.chunks(BATCH_SIZE) {
        if cancel.load(Ordering::Relaxed) {
            let _ = tx.send(DeleteMsg::Done { canceled: true });
            return;
        }

        let (succeeded, failed) = delete_batch(chunk);
        if tx
            .send(DeleteMsg::Batch { succeeded, failed })
            .is_err()
        {
            // UI 側の receiver が drop された (アプリ終了などの異常経路)。黙って終了。
            return;
        }
    }
    let _ = tx.send(DeleteMsg::Done {
        canceled: cancel.load(Ordering::Relaxed),
    });
}

/// 1 バッチを削除する。まず一括コールを試し、失敗したら 1 件ずつ再試行する。
fn delete_batch(chunk: &[PathBuf]) -> (Vec<PathBuf>, Vec<(PathBuf, String)>) {
    match recycle_many(chunk) {
        Ok(()) => (chunk.to_vec(), Vec::new()),
        Err(_) => {
            // バッチ失敗: 1 件ずつ実行して成功/失敗を精密に判定する。
            // SHFileOperationW は一部成功でも全体を失敗として返すことがあるので、
            // ここで個別確認しないと「一部は既にゴミ箱に入っているのに items に残る」
            // 不整合が起きる。
            let mut succeeded = Vec::new();
            let mut failed = Vec::new();
            for p in chunk {
                match recycle_one(p) {
                    Ok(()) => succeeded.push(p.clone()),
                    Err(msg) => failed.push((p.clone(), msg)),
                }
            }
            (succeeded, failed)
        }
    }
}

/// 複数パスを 1 回の `SHFileOperationW` でまとめて削除する。
#[cfg(windows)]
fn recycle_many(paths: &[PathBuf]) -> Result<(), String> {
    if paths.is_empty() {
        return Ok(());
    }
    use std::os::windows::ffi::OsStrExt;
    use windows::Win32::UI::Shell::{
        FO_DELETE, FOF_ALLOWUNDO, FOF_NOCONFIRMATION, FOF_NOERRORUI, FOF_SILENT, SHFILEOPSTRUCTW,
        SHFileOperationW,
    };

    // NULL 区切りで連結し、末尾は二重 NULL で終端。
    let mut wide: Vec<u16> = Vec::new();
    for p in paths {
        wide.extend(p.as_os_str().encode_wide());
        wide.push(0);
    }
    wide.push(0);

    // FOF_NOERRORUI: エラーダイアログを表示しない (バックグラウンドなので UI を出させない)
    let flags =
        (FOF_ALLOWUNDO.0 | FOF_NOCONFIRMATION.0 | FOF_SILENT.0 | FOF_NOERRORUI.0) as u16;
    let mut op = SHFILEOPSTRUCTW {
        wFunc: FO_DELETE,
        pFrom: windows::core::PCWSTR(wide.as_ptr()),
        fFlags: flags,
        ..Default::default()
    };
    let result = unsafe { SHFileOperationW(&mut op) };
    if result == 0 && op.fAnyOperationsAborted.as_bool() == false {
        Ok(())
    } else {
        Err(format!(
            "SHFileOperationW failed: code={result} aborted={}",
            op.fAnyOperationsAborted.as_bool()
        ))
    }
}

#[cfg(not(windows))]
fn recycle_many(_paths: &[PathBuf]) -> Result<(), String> {
    Err("recycle bin not supported on this platform".into())
}

/// 単一パスを `SHFileOperationW` で削除する (バッチ失敗時のフォールバック)。
#[cfg(windows)]
fn recycle_one(path: &PathBuf) -> Result<(), String> {
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
    if result == 0 && op.fAnyOperationsAborted.as_bool() == false {
        Ok(())
    } else {
        Err(format!("SHFileOperationW failed: code={result}"))
    }
}

#[cfg(not(windows))]
fn recycle_one(_path: &PathBuf) -> Result<(), String> {
    Err("recycle bin not supported on this platform".into())
}
