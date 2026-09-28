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

/// UUID cache failure must not suppress the established folder/catalog/tile maintenance.
/// When it fails, a combined Auto-aspect row count is unknown, not zero.
pub(crate) enum CollectionAspectOutcome {
    Applied(crate::auto_aspect_cache::CollectionAutoAspectMaintenanceStats),
    Failed(String),
}

impl CollectionAspectOutcome {
    pub(crate) fn combined_entries(&self, folder_entries: usize) -> Option<usize> {
        match self {
            Self::Applied(stats) => Some(folder_entries + stats.remaining),
            Self::Failed(_) => None,
        }
    }

    pub(crate) fn combined_deleted(&self, folder_deleted: usize) -> usize {
        folder_deleted
            + match self {
                Self::Applied(stats) => stats.deleted,
                Self::Failed(_) => 0,
            }
    }

    pub(crate) fn error(&self) -> Option<&str> {
        match self {
            Self::Applied(_) => None,
            Self::Failed(error) => Some(error),
        }
    }
}

/// ワーカーから UI に返す結果。
///
/// `tile_thumb_*` フィールドは動画タイル モード キャッシュ
/// (`video_tile_thumbs.db`) の削除/サイズ情報。`DeleteAll` / `DeleteFolder` 経路
/// では catalog (静止画 + 動画グリッド) と一緒に削除する。
pub(crate) enum CacheMaintResult {
    Error(String),
    Stats {
        folders: usize,
        bytes: u64,
        /// 動画タイル サムネ DB のサイズ (WAL/SHM 込み)。tile cache が無効なら 0。
        tile_thumb_bytes: u64,
        /// サムネイル比率 Auto モードのフォルダ別確定値キャッシュ件数。
        auto_aspect_entries: usize,
        collection_aspect: CollectionAspectOutcome,
    },
    DeleteOldDone {
        deleted: usize,
        new_stats: (usize, u64),
        /// 併せて削除した Auto 比率キャッシュ行数。
        auto_aspect_deleted: usize,
        /// 削除後の Auto 比率キャッシュ行数。
        auto_aspect_entries: usize,
        collection_aspect: CollectionAspectOutcome,
    },
    /// すべて削除完了。`tile_thumb` は動画タイル DB に対する処理結果。
    DeleteAllDone {
        tile_thumb: TileThumbOutcome,
        /// 併せて削除した Auto 比率キャッシュ行数。
        auto_aspect_deleted: usize,
        /// 削除後の Auto 比率キャッシュ行数。
        auto_aspect_entries: usize,
        collection_aspect: CollectionAspectOutcome,
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
        collection_aspect: CollectionAspectOutcome,
    },
}

pub(crate) struct CacheMaintPending {
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

#[derive(Debug, Clone)]
pub enum EpubMaintTask {
    LoadRows,
    DeleteSelected { generation_ids: Vec<i64> },
    DeleteAll,
    DeleteMissingSources,
}

pub struct EpubMaintResult {
    pub entries: Vec<crate::epub_cache::CurrentGenerationEntry>,
    pub deleted: usize,
    /// Includes files unlinked before a failed database commit.
    pub physically_removed: Vec<crate::epub_cache::GenerationRow>,
    pub failures: Vec<(PathBuf, String)>,
    pub error: Option<String>,
    /// Keep the read/admission boundary until the UI has invalidated its memory caches.
    pub(crate) barriers: Vec<(
        crate::pdf_loader::EpubRangeLease,
        crate::pdf_loader::EpubDocumentDeleteGuard,
    )>,
}

pub struct EpubMaintPending {
    pub task: EpubMaintTask,
    pub rx: mpsc::Receiver<EpubMaintResult>,
}

pub fn spawn_epub(task: EpubMaintTask, data_dir: PathBuf) -> EpubMaintPending {
    let (tx, rx) = mpsc::channel();
    let fallback = tx.clone();
    let pending_task = task.clone();
    let spawn = std::thread::Builder::new()
        .name("epub-cache-maint".into())
        .spawn(move || {
            let result = (|| -> Result<_, String> {
                crate::pdf_loader::epub_conversion_guard().map_err(|error| format!("{error:?}"))?;
                let mut cache = crate::epub_cache::EpubCache::open_at(&data_dir)
                    .map_err(|error| format!("{error:?}"))?;
                Ok(run_epub_task(
                    &mut cache,
                    &task,
                    crate::pdf_loader::release_epub_document_for_delete,
                    |cache, id| cache.delete_generation_now(id),
                ))
            })();
            let message = match result {
                Ok(result) => result,
                Err(error) => EpubMaintResult {
                    entries: Vec::new(),
                    deleted: 0,
                    physically_removed: Vec::new(),
                    failures: Vec::new(),
                    error: Some(error),
                    barriers: Vec::new(),
                },
            };
            let _ = tx.send(message);
        });
    if let Err(error) = spawn {
        let _ = fallback.send(EpubMaintResult {
            entries: Vec::new(),
            deleted: 0,
            physically_removed: Vec::new(),
            failures: Vec::new(),
            error: Some(format!("worker を開始できません: {error}")),
            barriers: Vec::new(),
        });
    }
    EpubMaintPending {
        task: pending_task,
        rx,
    }
}

fn run_epub_task(
    cache: &mut crate::epub_cache::EpubCache,
    task: &EpubMaintTask,
    release_document: impl Fn(
        &std::path::Path,
        std::time::Instant,
    ) -> Result<crate::pdf_loader::EpubDocumentDeleteGuard, String>,
    delete_generation: impl Fn(
        &mut crate::epub_cache::EpubCache,
        i64,
    ) -> Result<
        crate::epub_cache::ImmediateDeleteOutcome,
        crate::epub_cache::CacheError,
    >,
) -> EpubMaintResult {
    use crate::epub_cache::ImmediateDeleteOutcome;
    let mut result = EpubMaintResult {
        entries: Vec::new(),
        deleted: 0,
        physically_removed: Vec::new(),
        failures: Vec::new(),
        error: None,
        barriers: Vec::new(),
    };
    let initial_rows = match cache.list_current() {
        Ok(rows) => rows,
        Err(error) => {
            result.error = Some(format!("一覧を読み込めませんでした: {error:?}"));
            return result;
        }
    };
    let selected_ids = match task {
        EpubMaintTask::DeleteSelected { generation_ids } => generation_ids
            .iter()
            .copied()
            .collect::<std::collections::HashSet<_>>(
        ),
        _ => std::collections::HashSet::new(),
    };
    for entry in &initial_rows {
        let row = &entry.generation;
        let selected = match task {
            EpubMaintTask::LoadRows => false,
            EpubMaintTask::DeleteSelected { .. } => selected_ids.contains(&row.generation_id),
            EpubMaintTask::DeleteAll => true,
            EpubMaintTask::DeleteMissingSources => match source_definitely_missing(&row.src_path) {
                Ok(missing) => missing,
                Err(error) => {
                    result.failures.push((
                        row.src_path.clone(),
                        format!("元ファイルを確認できませんでした: {error}"),
                    ));
                    false
                }
            },
        };
        if !selected {
            continue;
        }
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        let _read_boundary = match crate::pdf_loader::acquire_epub_delete_coverage(&row.src_path) {
            Ok(lease) => lease,
            Err(reason) => {
                result.failures.push((row.src_path.clone(), reason));
                continue;
            }
        };
        let mut worker_boundary = match release_document(&row.pdf_file, deadline) {
            Ok(guard) => guard,
            Err(reason) => {
                result.failures.push((row.src_path.clone(), reason));
                continue;
            }
        };
        let removed_before = result.physically_removed.len();
        match delete_generation(cache, row.generation_id) {
            Ok(ImmediateDeleteOutcome::NotCurrent) => {}
            Ok(ImmediateDeleteOutcome::Deleted(deleted)) => {
                crate::pdf_loader::invalidate_epub_pin_under_delete(
                    &deleted.src_path,
                    deleted.generation_id,
                );
                result.deleted += 1;
                result.physically_removed.push(deleted);
            }
            Ok(ImmediateDeleteOutcome::Failed {
                generation,
                file_removed,
                error,
            }) => {
                crate::logger::log(format!(
                    "epub cache delete failed source={} generation={} file_removed={file_removed} error={error:?}",
                    generation.src_path.display(),
                    generation.generation_id
                ));
                if file_removed {
                    crate::pdf_loader::invalidate_epub_pin_under_delete(
                        &generation.src_path,
                        generation.generation_id,
                    );
                    result.physically_removed.push(generation.clone());
                    result.failures.push((
                        generation.src_path,
                        "ファイルは削除されましたが、管理情報を更新できませんでした。再度開くときに確認します".into(),
                    ));
                } else {
                    result
                        .failures
                        .push((generation.src_path, epub_delete_error_message(&error)));
                }
            }
            Err(error) => {
                crate::logger::log(format!(
                    "epub cache delete could not start source={} generation={} error={error:?}",
                    row.src_path.display(),
                    row.generation_id
                ));
                result.failures.push((
                    row.src_path.clone(),
                    "管理情報を読み込めず、削除できませんでした".into(),
                ));
            }
        }
        if result.physically_removed.len() > removed_before {
            worker_boundary.mark_file_removed();
            result.barriers.push((_read_boundary, worker_boundary));
        }
    }
    match cache.list_current() {
        Ok(entries) => result.entries = entries,
        Err(error) => result.error = Some(format!("一覧を更新できませんでした: {error:?}")),
    }
    result
}

/// A missing file is safe to classify only after a containing directory can
/// be reached. A disconnected network share can otherwise look like NotFound.
fn source_definitely_missing(path: &std::path::Path) -> std::io::Result<bool> {
    match std::fs::metadata(path) {
        Ok(_) => Ok(false),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let mut parent = path.parent();
            while let Some(directory) = parent {
                match std::fs::metadata(directory) {
                    Ok(meta) if meta.is_dir() => return Ok(true),
                    Ok(_) => return Err(std::io::Error::other("parent is not a directory")),
                    Err(parent_error) if parent_error.kind() == std::io::ErrorKind::NotFound => {
                        parent = directory.parent();
                    }
                    Err(parent_error) => return Err(parent_error),
                }
            }
            Err(error)
        }
        Err(error) => Err(error),
    }
}

fn epub_delete_error_message(error: &crate::epub_cache::CacheError) -> String {
    match error {
        crate::epub_cache::CacheError::Io(_) => {
            "使用中またはアクセスできないため削除できませんでした".into()
        }
        crate::epub_cache::CacheError::Sql(_) => {
            "管理情報を更新できず、削除できませんでした".into()
        }
        crate::epub_cache::CacheError::UnsafePath(_) => {
            "保存先を安全に確認できず、削除できませんでした".into()
        }
        crate::epub_cache::CacheError::InvalidState(_) => {
            "対象の状態が変わったため削除できませんでした".into()
        }
    }
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
pub(crate) fn spawn(
    task: CacheMaintTask,
    cache_dir: PathBuf,
    video_tile_cache: Option<Arc<TileThumbCache>>,
    collection_reply: Result<
        crate::auto_aspect_cache::CollectionAutoAspectMaintenanceReply,
        String,
    >,
) -> CacheMaintPending {
    let cancel = Arc::new(AtomicBool::new(false));
    let (tx, rx) = mpsc::channel();
    let tx_worker = tx.clone();
    let task_clone = task.clone();
    let spawn_result = std::thread::Builder::new()
        .name("cache-maint".into())
        .spawn(move || {
            // Collection UUID cache work was admitted on the UI side. Wait here, not in UI,
            // before reporting a successful combined clear. Failure leaves the other caches usable.
            let collection_aspect = match collection_reply
                .and_then(|reply| reply.recv().map_err(|_| "Collection Auto 比率 cache worker が停止しました".to_owned()))
                .and_then(|result| result)
            {
                Ok(stats) => CollectionAspectOutcome::Applied(stats),
                Err(error) => CollectionAspectOutcome::Failed(error),
            };
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
                        collection_aspect,
                    }
                }
                CacheMaintTask::DeleteOld { days } => {
                    let deleted = crate::catalog::delete_old_cache(&cache_dir, days);
                    let (auto_aspect_deleted, auto_aspect_entries) = auto_aspect_delete_old(days);
                    let new_stats = crate::catalog::cache_stats(&cache_dir);
                    CacheMaintResult::DeleteOldDone {
                        deleted,
                        new_stats,
                        auto_aspect_deleted,
                        auto_aspect_entries,
                        collection_aspect,
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
                        collection_aspect,
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
                        collection_aspect,
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

#[cfg(test)]
mod tests {
    use super::*;

    struct TestSource(crate::epub_cache::SourceState);

    impl crate::epub_cache::SourceGuard for TestSource {
        fn state(&self) -> std::io::Result<crate::epub_cache::SourceState> {
            Ok(self.0)
        }
    }

    fn publish_test_epub(
        cache: &mut crate::epub_cache::EpubCache,
        source: &std::path::Path,
    ) -> crate::epub_cache::GenerationRow {
        use crate::epub_cache::{CONVERTER_OUTPUT_VERSION, GenerationRow};
        std::fs::write(source, b"test epub").unwrap();
        let state = crate::epub_cache::source_state(&std::fs::metadata(source).unwrap());
        let reserved = cache.reserve_output(source).unwrap();
        std::fs::create_dir_all(reserved.final_path().parent().unwrap()).unwrap();
        std::fs::write(reserved.final_path(), b"%PDF-1.4\n").unwrap();
        let row = GenerationRow {
            generation_id: reserved.generation_id(),
            src_path_key: crate::epub_cache::src_key(source),
            src_path: source.to_owned(),
            src_state: state,
            src_sha256: "full".into(),
            src_head_hash: "head".into(),
            pdf_file: reserved.final_path().to_owned(),
            pdf_size: 9,
            page_count: 1,
            direction: "rtl".into(),
            profile: "test".into(),
            created_at: 1,
            output_version: CONVERTER_OUTPUT_VERSION,
        };
        cache.publish(&row, &TestSource(state)).unwrap();
        row
    }

    #[test]
    fn epub_manager_selected_all_and_missing_delete_immediately_with_partial_failure() {
        let temp = tempfile::tempdir().unwrap();
        let mut cache = crate::epub_cache::EpubCache::open_at(temp.path()).unwrap();
        let a = publish_test_epub(&mut cache, &temp.path().join("a.epub"));
        let b = publish_test_epub(&mut cache, &temp.path().join("b.epub"));
        let c = publish_test_epub(&mut cache, &temp.path().join("c.epub"));
        let busy = crate::pdf_loader::try_acquire_epub_read_lease(&b.src_path).unwrap();
        let selected = run_epub_task(
            &mut cache,
            &EpubMaintTask::DeleteSelected {
                generation_ids: vec![a.generation_id, b.generation_id],
            },
            crate::pdf_loader::block_epub_document_for_test,
            |cache, id| cache.delete_generation_now(id),
        );
        assert_eq!(selected.deleted, 1);
        assert_eq!(selected.failures.len(), 1);
        assert!(!a.pdf_file.exists());
        assert!(b.pdf_file.exists());
        assert!(c.pdf_file.exists());

        std::fs::remove_file(&c.src_path).unwrap();
        let missing = run_epub_task(
            &mut cache,
            &EpubMaintTask::DeleteMissingSources,
            crate::pdf_loader::block_epub_document_for_test,
            |cache, id| cache.delete_generation_now(id),
        );
        assert_eq!(missing.deleted, 1);
        assert!(!c.pdf_file.exists());
        drop(busy);
        let all = run_epub_task(
            &mut cache,
            &EpubMaintTask::DeleteAll,
            crate::pdf_loader::block_epub_document_for_test,
            |cache, id| cache.delete_generation_now(id),
        );
        assert_eq!(all.deleted, 1);
        assert!(!b.pdf_file.exists());
        assert!(all.entries.is_empty());
    }

    #[test]
    fn epub_manager_commit_failure_invalidates_fixed_generation() {
        let temp = tempfile::tempdir().unwrap();
        let mut cache = crate::epub_cache::EpubCache::open_at(temp.path()).unwrap();
        let row = publish_test_epub(&mut cache, &temp.path().join("book.epub"));
        let _pin =
            crate::pdf_loader::pin_epub_for_test(&row.src_path, row.generation_id, row.pdf_size);
        assert!(crate::pdf_loader::pinned_epub_target(&row.src_path).is_some());
        let result = run_epub_task(
            &mut cache,
            &EpubMaintTask::DeleteAll,
            crate::pdf_loader::block_epub_document_for_test,
            |cache, id| cache.delete_generation_with_commit_failure_for_test(id),
        );
        assert_eq!(result.deleted, 0);
        assert_eq!(result.physically_removed.len(), 1);
        assert_eq!(result.failures.len(), 1);
        assert!(!row.pdf_file.exists());
        let mut app = crate::app::setup_app_for_test();
        let removed_key = format!("{}::page_0", crate::epub_cache::src_key(&row.src_path));
        let unrelated_key = "c:/books/other.epub::page_0".to_owned();
        let pixels = std::sync::Arc::new(egui::ColorImage::new([1, 1], vec![egui::Color32::BLACK]));
        for item_key in [removed_key.clone(), unrelated_key.clone()] {
            app.retained_final_ai_cache.insert(
                crate::app::RetainedFinalAiKey {
                    item_key: item_key.clone(),
                    edit_size: [1, 1],
                    color_ai_hash: 1,
                    bg: 0,
                },
                crate::app::RetainedFinalAiEntry {
                    pixels: std::sync::Arc::clone(&pixels),
                    used_upscale: false,
                    bytes: 4,
                    last_used: 0,
                },
            );
            app.retained_pdf_page_cache.insert(
                crate::app::RetainedPdfPageKey { item_key },
                crate::app::RetainedPdfPageEntry {
                    kind: crate::app::RetainedPdfPageCacheKind::Raster {
                        pixels: std::sync::Arc::clone(&pixels),
                        source_dims: [1, 1],
                        render_long_edge: 1,
                    },
                    bytes: 4,
                    last_used: 0,
                },
            );
        }
        app.invalidate_removed_epub_generations(&result.physically_removed);
        assert!(
            !app.retained_final_ai_cache
                .keys()
                .any(|key| key.item_key == removed_key)
        );
        assert!(
            !app.retained_pdf_page_cache
                .keys()
                .any(|key| key.item_key == removed_key)
        );
        assert!(
            app.retained_final_ai_cache
                .keys()
                .any(|key| key.item_key == unrelated_key)
        );
        assert!(
            app.retained_pdf_page_cache
                .keys()
                .any(|key| key.item_key == unrelated_key)
        );
        drop(result);
        assert!(crate::pdf_loader::pinned_epub_target(&row.src_path).is_none());
    }

    #[test]
    fn collection_actor_failure_preserves_folder_maintenance_and_reports_unknown_total() {
        let _app = crate::app::setup_app_for_test();
        let temp = tempfile::tempdir().unwrap();
        let folder = temp.path().join("book");
        let db_path = crate::catalog::db_path_for(temp.path(), &folder);
        std::fs::create_dir_all(db_path.parent().unwrap()).unwrap();
        std::fs::write(&db_path, b"cached").unwrap();
        let pending = spawn(
            CacheMaintTask::DeleteFolder {
                folder,
                auto_aspect_folder: None,
            },
            temp.path().to_path_buf(),
            None,
            Err("collection actor unavailable".into()),
        );
        let result = pending
            .rx
            .recv_timeout(std::time::Duration::from_secs(1))
            .unwrap();
        assert!(!db_path.exists(), "folder cache deletion must still run");
        assert!(matches!(
            result,
            CacheMaintResult::DeleteFolderDone {
                existed: true,
                collection_aspect: CollectionAspectOutcome::Failed(_),
                ..
            }
        ));

        let pending = spawn(
            CacheMaintTask::DeleteAll,
            temp.path().to_path_buf(),
            None,
            Err("collection actor unavailable".into()),
        );
        assert!(matches!(
            pending.rx.recv_timeout(std::time::Duration::from_secs(1)).unwrap(),
            CacheMaintResult::DeleteAllDone {
                collection_aspect: CollectionAspectOutcome::Failed(message), ..
            } if message.contains("collection actor unavailable")
        ));
    }
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
