//! サムネイルキャッシュ管理ダイアログから走らせる重い I/O をバックグラウンド化する。
//!
//! `catalog::cache_stats` / `delete_old_cache` / `delete_all_cache` はキャッシュ配下を
//! `read_dir` + `metadata` + `remove_file` で舐めるので、キャッシュ DB が数千フォルダ
//! 規模になると UI スレッドで秒オーダーのブロックが出る。本モジュールは各操作を
//! 別スレッドで走らせ、結果を `mpsc::Receiver` で UI に返す。
//!
//! UI 側は `CacheMaintPending` を保持している間ボタンを無効化して「処理中…」表示にし、
//! `poll_cache_maint_pending` が `CacheMaintResult` を受けたら stats / result を反映する。

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::mpsc;

/// 管理ダイアログで走らせる操作種別。
#[derive(Debug, Clone)]
pub enum CacheMaintTask {
    /// キャッシュ配下の (.db ファイル数, 合計バイト数) を集計する。
    Stats,
    /// 最終更新から `days` 日以上前の .db を削除。
    DeleteOld { days: u64 },
    /// .db をすべて削除。
    DeleteAll,
    /// 指定フォルダに対応する 1 ファイルの .db を削除。
    DeleteFolder { folder: PathBuf },
}

/// ワーカーから UI に返す結果。
pub enum CacheMaintResult {
    Stats {
        folders: usize,
        bytes: u64,
    },
    DeleteOldDone {
        deleted: usize,
        new_stats: (usize, u64),
    },
    DeleteAllDone,
    DeleteFolderDone {
        existed: bool,
        folder_name: String,
        new_stats: (usize, u64),
    },
}

pub struct CacheMaintPending {
    pub task: CacheMaintTask,
    pub rx: mpsc::Receiver<CacheMaintResult>,
    pub cancel: Arc<AtomicBool>,
}

// ─────────────────────────────────────────────────────────────────────────
// 変換済みアーカイブキャッシュ (ArchiveCacheDb) 側のワーカー
// ─────────────────────────────────────────────────────────────────────────

/// 変換済みアーカイブキャッシュダイアログで走らせる操作。
#[derive(Debug, Clone)]
pub enum ArchiveMaintTask {
    /// DB 全件ロード + 各 src_path の exists チェック + total_size 集計。
    /// ダイアログ表示 / 再読込 / 各種削除後の再ロードに使う。
    LoadRows,
    /// 指定 src_path のエントリと対応するキャッシュ ZIP を削除。
    DeleteSelected { src_paths: Vec<std::path::PathBuf> },
    /// 元ファイル消失エントリを一括削除。
    DeleteMissing,
    /// 全件削除 + キャッシュディレクトリ掃除。
    DeleteAll,
}

pub enum ArchiveMaintResult {
    Rows {
        entries: Vec<crate::archive_cache::ArchiveCacheEntry>,
        total_bytes: u64,
    },
    DeletedSelected {
        removed: usize,
    },
    DeletedMissing {
        removed: usize,
    },
    DeletedAll {
        removed: usize,
    },
    Error(String),
}

pub struct ArchiveMaintPending {
    pub task: ArchiveMaintTask,
    pub rx: mpsc::Receiver<ArchiveMaintResult>,
}

pub fn spawn_archive(
    task: ArchiveMaintTask,
    db: Arc<crate::archive_cache::ArchiveCacheDb>,
) -> ArchiveMaintPending {
    let (tx, rx) = mpsc::channel();
    let task_clone = task.clone();
    std::thread::Builder::new()
        .name("archive-cache-maint".into())
        .spawn(move || {
            let result = match task_clone {
                ArchiveMaintTask::LoadRows => match db.list_all() {
                    Ok(entries) => {
                        let total_bytes = db.total_size().unwrap_or(0);
                        ArchiveMaintResult::Rows {
                            entries,
                            total_bytes,
                        }
                    }
                    Err(e) => ArchiveMaintResult::Error(format!("list_all failed: {e}")),
                },
                ArchiveMaintTask::DeleteSelected { src_paths } => {
                    let mut removed = 0;
                    for p in &src_paths {
                        if db.delete_entry(p).is_ok() {
                            removed += 1;
                        }
                    }
                    ArchiveMaintResult::DeletedSelected { removed }
                }
                ArchiveMaintTask::DeleteMissing => match db.delete_missing_originals() {
                    Ok(n) => ArchiveMaintResult::DeletedMissing { removed: n },
                    Err(e) => ArchiveMaintResult::Error(format!("delete_missing failed: {e}")),
                },
                ArchiveMaintTask::DeleteAll => match db.clear_all() {
                    Ok(n) => ArchiveMaintResult::DeletedAll { removed: n },
                    Err(e) => ArchiveMaintResult::Error(format!("clear_all failed: {e}")),
                },
            };
            let _ = tx.send(result);
        })
        .expect("failed to spawn archive-cache-maint worker");
    ArchiveMaintPending { task, rx }
}

/// 指定タスクを別スレッドで実行し、ハンドルを返す。
pub fn spawn(task: CacheMaintTask, cache_dir: PathBuf) -> CacheMaintPending {
    let cancel = Arc::new(AtomicBool::new(false));
    let (tx, rx) = mpsc::channel();
    let task_clone = task.clone();
    std::thread::Builder::new()
        .name("cache-maint".into())
        .spawn(move || {
            let result = match task_clone {
                CacheMaintTask::Stats => {
                    let (folders, bytes) = crate::catalog::cache_stats(&cache_dir);
                    CacheMaintResult::Stats { folders, bytes }
                }
                CacheMaintTask::DeleteOld { days } => {
                    let deleted = crate::catalog::delete_old_cache(&cache_dir, days);
                    let new_stats = crate::catalog::cache_stats(&cache_dir);
                    CacheMaintResult::DeleteOldDone {
                        deleted,
                        new_stats,
                    }
                }
                CacheMaintTask::DeleteAll => {
                    crate::catalog::delete_all_cache(&cache_dir);
                    CacheMaintResult::DeleteAllDone
                }
                CacheMaintTask::DeleteFolder { folder } => {
                    let db_path = crate::catalog::db_path_for(&cache_dir, &folder);
                    let folder_name = folder
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or("?")
                        .to_string();
                    let existed = db_path.exists();
                    if existed {
                        let _ = std::fs::remove_file(&db_path);
                    }
                    let new_stats = crate::catalog::cache_stats(&cache_dir);
                    CacheMaintResult::DeleteFolderDone {
                        existed,
                        folder_name,
                        new_stats,
                    }
                }
            };
            let _ = tx.send(result);
        })
        .expect("failed to spawn cache-maint worker");
    CacheMaintPending { task, rx, cancel }
}
