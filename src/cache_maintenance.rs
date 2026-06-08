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

use crate::video::tile_thumb_cache::TileThumbCache;

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
    DeleteFolder {
        folder: PathBuf,
        /// Auto 比率キャッシュ側で削除するユーザー視点の対象。
        ///
        /// 通常は `folder` と同じだが、変換済み RAR/7z/LZH 閲覧中は `folder` が
        /// キャッシュ ZIP、Auto 比率キャッシュは元アーカイブパスで保存される。
        auto_aspect_folder: Option<PathBuf>,
    },
}

/// 動画タイル サムネ DB の削除アウトカム。
///
/// 通常経路では `clear_all` / `clear_for_folder` が削除行数を返すが、open に失敗して
/// `TileThumbCache` インスタンスが None だったとき / `clear_all` 自体が失敗したとき
/// (= DB 壊れ / ロック / I/O エラー) の fallback として、DB ファイルを物理削除する
/// 経路を区別する (Codex P2)。
#[derive(Debug, Clone)]
pub enum TileThumbOutcome {
    /// 通常経路: SQL の DELETE + VACUUM で `rows` 行を消した。
    Cleared { rows: usize },
    /// fallback: `video_tile_thumbs.db` / `-wal` / `-shm` を `remove_file` で消した。
    /// `files_removed` は実際に消えたファイル数 (0〜3、存在しなかったものは含まれない)。
    FilesErased { files_removed: usize },
    /// tile cache 経路が今回の処理対象ではない (例: `DeleteOld` や `DeleteFolder` で
    /// open 失敗時など、何もしないケース)。
    Untouched,
}

/// ワーカーから UI に返す結果。
///
/// `tile_thumb_*` フィールドは動画タイル モード キャッシュ
/// (`video_tile_thumbs.db`) の削除/サイズ情報。`DeleteAll` / `DeleteFolder` 経路
/// では catalog (静止画 + 動画グリッド) と一緒に削除する。
pub enum CacheMaintResult {
    Error(String),
    Stats {
        folders: usize,
        bytes: u64,
        /// 動画タイル サムネ DB のサイズ (WAL/SHM 込み)。tile cache が無効なら 0。
        tile_thumb_bytes: u64,
        /// サムネイル比率 Auto モードのフォルダ別確定値キャッシュ件数。
        auto_aspect_entries: usize,
    },
    DeleteOldDone {
        deleted: usize,
        new_stats: (usize, u64),
        /// 併せて削除した Auto 比率キャッシュ行数。
        auto_aspect_deleted: usize,
        /// 削除後の Auto 比率キャッシュ行数。
        auto_aspect_entries: usize,
    },
    /// すべて削除完了。`tile_thumb` は動画タイル DB に対する処理結果。
    DeleteAllDone {
        tile_thumb: TileThumbOutcome,
        /// 併せて削除した Auto 比率キャッシュ行数。
        auto_aspect_deleted: usize,
        /// 削除後の Auto 比率キャッシュ行数。
        auto_aspect_entries: usize,
    },
    DeleteFolderDone {
        existed: bool,
        folder_name: String,
        new_stats: (usize, u64),
        /// 当該フォルダ配下の動画タイル サムネに対する処理結果。
        tile_thumb: TileThumbOutcome,
        /// 当該フォルダの Auto 比率キャッシュ削除行数。
        auto_aspect_deleted: usize,
        /// 削除後の Auto 比率キャッシュ行数。
        auto_aspect_entries: usize,
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
    let tx_worker = tx.clone();
    let task_clone = task.clone();
    let spawn_result = std::thread::Builder::new()
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
            let _ = tx_worker.send(result);
        });
    if let Err(e) = spawn_result {
        crate::logger::log(format!("failed to spawn archive-cache-maint worker: {e}"));
        let _ = tx.send(ArchiveMaintResult::Error(format!(
            "worker を開始できません: {e}"
        )));
    }
    ArchiveMaintPending { task, rx }
}

/// 指定タスクを別スレッドで実行し、ハンドルを返す。
///
/// `video_tile_cache` を渡すと、`DeleteAll` / `DeleteFolder` の際に動画タイル サムネ
/// キャッシュ DB (`video_tile_thumbs.db`) も同時に削除する (= ユーザー UX で
/// 「サムネ削除」と一括で動かす)。`DeleteOld` は tile cache に「最終アクセス時刻」が
/// 無いため対象外。`Stats` 時は tile DB ファイル サイズも添えて返す。
pub fn spawn(
    task: CacheMaintTask,
    cache_dir: PathBuf,
    video_tile_cache: Option<Arc<TileThumbCache>>,
) -> CacheMaintPending {
    let cancel = Arc::new(AtomicBool::new(false));
    let (tx, rx) = mpsc::channel();
    let tx_worker = tx.clone();
    let task_clone = task.clone();
    let spawn_result = std::thread::Builder::new()
        .name("cache-maint".into())
        .spawn(move || {
            let result = match task_clone {
                CacheMaintTask::Stats => {
                    let (folders, bytes) = crate::catalog::cache_stats(&cache_dir);
                    let tile_thumb_bytes = TileThumbCache::db_size_bytes();
                    let auto_aspect_entries = auto_aspect_count();
                    CacheMaintResult::Stats {
                        folders,
                        bytes,
                        tile_thumb_bytes,
                        auto_aspect_entries,
                    }
                }
                CacheMaintTask::DeleteOld { days } => {
                    let deleted = crate::catalog::delete_old_cache(&cache_dir, days);
                    let (auto_aspect_deleted, auto_aspect_entries) =
                        auto_aspect_delete_old(days);
                    let new_stats = crate::catalog::cache_stats(&cache_dir);
                    CacheMaintResult::DeleteOldDone {
                        deleted,
                        new_stats,
                        auto_aspect_deleted,
                        auto_aspect_entries,
                    }
                }
                CacheMaintTask::DeleteAll => {
                    crate::catalog::delete_all_cache(&cache_dir);
                    // 通常経路: open 済みインスタンスがあれば clear_all (DELETE + VACUUM)。
                    // 失敗 / インスタンス None なら fallback で DB ファイルを物理削除する
                    // (Codex P2: DB 壊れ / ロックで「全削除」しても残らないように)。
                    let tile_thumb = match video_tile_cache.as_ref() {
                        Some(c) => match c.clear_all() {
                            Ok(rows) => TileThumbOutcome::Cleared { rows },
                            Err(e) => {
                                crate::logger::log(format!(
                                    "video_tile_cache.clear_all failed: {e} — falling back to file erase"
                                ));
                                let files_removed = TileThumbCache::erase_db_files();
                                TileThumbOutcome::FilesErased { files_removed }
                            }
                        },
                        None => {
                            let files_removed = TileThumbCache::erase_db_files();
                            TileThumbOutcome::FilesErased { files_removed }
                        }
                    };
                    let (auto_aspect_deleted, auto_aspect_entries) = auto_aspect_clear_all();
                    CacheMaintResult::DeleteAllDone {
                        tile_thumb,
                        auto_aspect_deleted,
                        auto_aspect_entries,
                    }
                }
                CacheMaintTask::DeleteFolder {
                    folder,
                    auto_aspect_folder,
                } => {
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
                    // フォルダ単位削除は prefix DELETE が必要なので、open 失敗時の
                    // fallback は無い (= DB 全消しで対応すべきケースではない)。Untouched
                    // を返して UI 側でメッセージを区別する。
                    let tile_thumb = match video_tile_cache
                        .as_ref()
                        .map(|c| c.clear_for_folder(&folder))
                    {
                        Some(Ok(rows)) => TileThumbOutcome::Cleared { rows },
                        Some(Err(e)) => {
                            crate::logger::log(format!(
                                "video_tile_cache.clear_for_folder failed: {e}"
                            ));
                            TileThumbOutcome::Untouched
                        }
                        None => TileThumbOutcome::Untouched,
                    };
                    let (auto_aspect_deleted, auto_aspect_entries) = auto_aspect_folder
                        .as_deref()
                        .map(auto_aspect_delete_folder)
                        .unwrap_or_else(|| (0, auto_aspect_count()));
                    let new_stats = crate::catalog::cache_stats(&cache_dir);
                    CacheMaintResult::DeleteFolderDone {
                        existed,
                        folder_name,
                        new_stats,
                        tile_thumb,
                        auto_aspect_deleted,
                        auto_aspect_entries,
                    }
                }
            };
            let _ = tx_worker.send(result);
        });
    if let Err(e) = spawn_result {
        crate::logger::log(format!("failed to spawn cache-maint worker: {e}"));
        let _ = tx.send(CacheMaintResult::Error(format!(
            "worker を開始できません: {e}"
        )));
    }
    CacheMaintPending { task, rx, cancel }
}

fn with_auto_aspect_db<T>(
    op: impl FnOnce(&crate::auto_aspect_cache::AutoAspectCacheDb) -> Result<T, rusqlite::Error>,
    fallback: T,
    label: &str,
) -> T {
    match crate::auto_aspect_cache::AutoAspectCacheDb::open().and_then(|db| op(&db)) {
        Ok(value) => value,
        Err(e) => {
            crate::logger::log(format!("auto_aspect_cache {label} failed: {e}"));
            fallback
        }
    }
}

fn auto_aspect_count() -> usize {
    with_auto_aspect_db(|db| Ok(db.count()), 0, "count")
}

fn auto_aspect_delete_old(days: u64) -> (usize, usize) {
    with_auto_aspect_db(
        |db| {
            let deleted = db.delete_older_than_days(days)?;
            Ok((deleted, db.count()))
        },
        (0, 0),
        "delete_old",
    )
}

fn auto_aspect_clear_all() -> (usize, usize) {
    with_auto_aspect_db(
        |db| {
            let deleted = db.clear_all()?;
            Ok((deleted, db.count()))
        },
        (0, 0),
        "clear_all",
    )
}

fn auto_aspect_delete_folder(folder: &std::path::Path) -> (usize, usize) {
    with_auto_aspect_db(
        |db| {
            let deleted = db.delete_for_folder(folder)?;
            Ok((deleted, db.count()))
        },
        (0, 0),
        "delete_for_folder",
    )
}
