//! 名前索引 (Ctrl+S 検索用 `search_index_db`) のスーパーバイザ。
//!
//! ## 責務
//!
//! お気に入りで `auto_index_structure = true` の間、以下を 1 スレッドで担当する:
//!
//! 1. 起動時の初期バルクスキャン (`name_bulk_indexer::run_bulk_name_index`)
//! 2. FS 監視 (`FsWatcher` = notify-rs) を張り続け、debounce 済みイベントを受信
//! 3. 変更された path の parent フォルダを再走査して `search_index_db::upsert_children`
//!    で差分反映
//! 4. `NameIndexStats` を UI に snapshot 可能な形で公開
//!
//! ## メタ索引 Supervisor との違い
//!
//! - 書き込み先が SQLite (`SearchIndexDb`) なので Tantivy writer 制約がない →
//!   **複数お気に入りの name supervisor は真に並列で動ける**
//! - Ingest フェーズが軽量 (upsert_children = `INSERT OR REPLACE`) なので、メタ側の
//!   ように `writer.lock()` 直前に「取込待ち」状態を出す必要はない
//!
//! ## 既存の `name_bulk_indexer` との関係
//!
//! `name_bulk_indexer::run_bulk_name_index` をワンショット bulk の実装本体として
//! 再利用する。旧来 `spawn_bulk` で thread を投げっぱなしにしていた経路は
//! `NameIndexSupervisor` で長期スレッド化した形に置き換わる。
//!
//! 2026-04 ユーザー指摘: 旧 `name_bulk_indexer::spawn_bulk` は初期 1 回だけ走って
//! 終了していたため、「✅ 索引あり」表示はスナップショット時点の静的状態を示すに
//! 過ぎず、その後 FS に追加されたフォルダ/ZIP/PDF は Ctrl+S 検索にヒットしなかった。
//! この module が FsWatcher を握って差分追従する責務を担う。

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Instant;

use crossbeam_channel::{Receiver, Sender, bounded, select};
use uuid::Uuid;

use crate::folder_tree::is_apple_double;
use crate::indexer_progress::ProgressReporter;
use crate::name_bulk_indexer::{classify_name_index_kind, run_bulk_name_index};
use crate::search_index_db::{IndexEntry, SearchIndexDb};
use crate::search_watcher::{ChangeKind, DebouncedChange, FsWatcher, OVERFLOW_MARKER_PATH};

/// UI が snapshot するステータス。
#[derive(Clone, Debug, Default)]
pub struct NameIndexStats {
    /// 初期バルクスキャンが完了したか
    pub initial_scan_done: bool,
    /// フル scan (初期 or 手動再構築) を実行中か
    pub in_full_scan: bool,
    /// 初期バルクで書き込まれたエントリ件数 (参考)
    pub initial_entries_written: usize,
    /// watcher イベントで適用した更新回数 (参考)
    pub updates_applied: usize,
    /// 最新の `progress` 情報 (snapshot 時に合成される)
    pub current_activity: Option<String>,
}

pub enum NameIndexCommand {
    Stop,
    #[allow(dead_code)] // 将来の手動再構築ボタン用
    FullRescan,
}

pub struct NameIndexSupervisorHandle {
    pub favorite_id: Uuid,
    cmd_tx: Sender<NameIndexCommand>,
    cancel: Arc<AtomicBool>,
    stats: Arc<Mutex<NameIndexStats>>,
    progress: ProgressReporter,
    thread: Option<JoinHandle<()>>,
}

impl NameIndexSupervisorHandle {
    pub fn snapshot_stats(&self) -> NameIndexStats {
        let mut s = self.stats.lock().unwrap().clone();
        s.current_activity = self.progress.snapshot();
        s
    }

    /// cancel シグナルだけ送り、thread join は待たない。
    /// `IndexerManager::drop` と同じ「全員 signal_stop → drain」パターン用。
    pub fn signal_stop(&self) {
        self.cancel.store(true, Ordering::SeqCst);
        let _ = self.cmd_tx.send(NameIndexCommand::Stop);
    }
}

impl Drop for NameIndexSupervisorHandle {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::SeqCst);
        let _ = self.cmd_tx.send(NameIndexCommand::Stop);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// 1 お気に入り分の name index supervisor を起動する。
///
/// `activity_gate` が `Some` のとき、bulk scan は UI 操作中に自動で待機する (2026-04 F)。
/// テスト・レガシー経路で指定不要なら `None` を渡す。
pub fn spawn(
    favorite_id: Uuid,
    favorite_root: PathBuf,
    db: Arc<SearchIndexDb>,
    activity_gate: Option<Arc<crate::activity_gate::ActivityGate>>,
) -> NameIndexSupervisorHandle {
    let cancel = Arc::new(AtomicBool::new(false));
    let stats = Arc::new(Mutex::new(NameIndexStats::default()));
    let progress = ProgressReporter::new();
    let (cmd_tx, cmd_rx) = bounded::<NameIndexCommand>(4);
    let (change_tx, change_rx) = crossbeam_channel::unbounded::<DebouncedChange>();

    let cancel_cl = Arc::clone(&cancel);
    let stats_cl = Arc::clone(&stats);
    let progress_cl = progress.clone();
    let root_cl = favorite_root.clone();

    crate::logger::log(format!(
        "name_index[{favorite_id}]: supervisor starting for {}",
        favorite_root.display()
    ));

    let thread = std::thread::Builder::new()
        .name(format!("name-index-{}", favorite_id.as_simple()))
        .spawn(move || {
            supervisor_loop(
                favorite_id,
                root_cl,
                db,
                activity_gate,
                cancel_cl,
                stats_cl,
                progress_cl,
                cmd_rx,
                change_tx,
                change_rx,
            );
        })
        .expect("failed to spawn name index supervisor");

    NameIndexSupervisorHandle {
        favorite_id,
        cmd_tx,
        cancel,
        stats,
        progress,
        thread: Some(thread),
    }
}

#[allow(clippy::too_many_arguments)]
fn supervisor_loop(
    favorite_id: Uuid,
    favorite_root: PathBuf,
    db: Arc<SearchIndexDb>,
    activity_gate: Option<Arc<crate::activity_gate::ActivityGate>>,
    cancel: Arc<AtomicBool>,
    stats: Arc<Mutex<NameIndexStats>>,
    progress: ProgressReporter,
    cmd_rx: Receiver<NameIndexCommand>,
    change_tx: Sender<DebouncedChange>,
    change_rx: Receiver<DebouncedChange>,
) {
    // 1. FsWatcher を先に起動して変更を取りこぼさない
    //    (初期 bulk 中に変更が起きても change_rx にたまる)
    let watcher = FsWatcher::start(favorite_id, &favorite_root, change_tx.clone()).ok();
    if watcher.is_none() {
        crate::logger::log(format!(
            "name_index[{favorite_id}]: FsWatcher start failed (will still run initial bulk)"
        ));
    }

    // 2. 初期バルク
    run_full_scan(
        favorite_id,
        &favorite_root,
        &db,
        activity_gate.as_deref(),
        &cancel,
        &stats,
        &progress,
    );

    // 3. watcher イベント + cmd を select で受信するループ
    loop {
        if cancel.load(Ordering::SeqCst) {
            break;
        }
        select! {
            recv(cmd_rx) -> msg => {
                match msg {
                    Ok(NameIndexCommand::Stop) => break,
                    Ok(NameIndexCommand::FullRescan) => {
                        run_full_scan(
                            favorite_id,
                            &favorite_root,
                            &db,
                            activity_gate.as_deref(),
                            &cancel,
                            &stats,
                            &progress,
                        );
                    }
                    Err(_) => break,
                }
            }
            recv(change_rx) -> msg => {
                match msg {
                    Ok(DebouncedChange { favorite_id: fid, path, kind }) => {
                        if fid != favorite_id { continue; }
                        if path.to_string_lossy() == OVERFLOW_MARKER_PATH {
                            // overflow → フル再スキャン
                            crate::logger::log(format!(
                                "name_index[{favorite_id}]: watcher overflow, running full rescan"
                            ));
                            run_full_scan(
                                favorite_id,
                                &favorite_root,
                                &db,
                                activity_gate.as_deref(),
                                &cancel,
                                &stats,
                                &progress,
                            );
                            continue;
                        }
                        apply_single_change(&favorite_root, &db, &path, kind, &progress, &stats);
                    }
                    Err(_) => break,
                }
            }
        }
    }

    drop(watcher);
    crate::logger::log(format!("name_index[{favorite_id}]: supervisor exiting"));
}

/// 初期 bulk / 手動再構築 / overflow で呼ばれる「フル scan」経路。
#[allow(clippy::too_many_arguments)]
fn run_full_scan(
    favorite_id: Uuid,
    favorite_root: &Path,
    db: &SearchIndexDb,
    activity_gate: Option<&crate::activity_gate::ActivityGate>,
    cancel: &AtomicBool,
    stats: &Mutex<NameIndexStats>,
    progress: &ProgressReporter,
) {
    let is_initial = !stats.lock().unwrap().initial_scan_done;
    stats.lock().unwrap().in_full_scan = true;
    let t0 = Instant::now();

    crate::logger::log(format!(
        "name_index[{favorite_id}]: {} scan starting",
        if is_initial { "initial" } else { "rescan" }
    ));

    let summary = run_bulk_name_index(favorite_root, db, activity_gate, cancel, Some(progress));

    let dur_ms = t0.elapsed().as_millis() as u64;
    crate::logger::log(format!(
        "name_index[{favorite_id}]: {} scan done in {dur_ms} ms (folders={}, entries={}, cancelled={})",
        if is_initial { "initial" } else { "rescan" },
        summary.folders_visited,
        summary.entries_written,
        summary.cancelled,
    ));

    {
        let mut s = stats.lock().unwrap();
        s.in_full_scan = false;
        if is_initial {
            s.initial_scan_done = true;
            s.initial_entries_written = summary.entries_written;
        }
    }
    progress.clear();
}

/// 1 つの変更イベントを適用する。
/// 変更があった path の **親フォルダ** を読み直して `upsert_children` で差分反映する
/// (Add/Remove/Rename を統一的に扱えるため)。
fn apply_single_change(
    favorite_root: &Path,
    db: &SearchIndexDb,
    changed_path: &Path,
    _kind: ChangeKind,
    progress: &ProgressReporter,
    stats: &Mutex<NameIndexStats>,
) {
    let Some(parent) = changed_path.parent() else {
        return;
    };
    progress.set(format!("更新: {}", parent.display()));

    let mut children: Vec<IndexEntry> = Vec::new();
    match std::fs::read_dir(parent) {
        Ok(entries) => {
            for entry in entries.flatten() {
                let p = entry.path();
                if is_apple_double(&p) {
                    continue;
                }
                let Ok(ft) = entry.file_type() else { continue };
                let Some(kind) = classify_name_index_kind(&p, &ft) else {
                    continue;
                };
                let name = p
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("")
                    .to_string();
                children.push(IndexEntry {
                    path: p,
                    display_name: name,
                    kind,
                    mtime: 0,
                });
            }
        }
        Err(_) => {
            // parent が削除されたケース: 空の children で upsert_children を呼び、その
            // フォルダ配下の行を一掃する。
        }
    }

    if let Err(e) = db.upsert_children(favorite_root, parent, &children) {
        crate::logger::log(format!(
            "name_index: upsert_children failed for {}: {e}",
            parent.display()
        ));
    } else {
        stats.lock().unwrap().updates_applied += 1;
    }
    progress.clear();
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::time::Duration;
    use tempfile::TempDir;

    /// 初期スキャン完了 + stats 反映を確認する最小 smoke test。
    /// watcher の E2E は OS 依存なので固定秒で polling する。
    #[test]
    fn initial_scan_populates_stats() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().join("fav");
        fs::create_dir_all(root.join("sub")).unwrap();
        fs::write(root.join("a.zip"), b"").unwrap();
        fs::write(root.join("b.pdf"), b"").unwrap();

        let db = Arc::new(SearchIndexDb::open_in_memory().unwrap());
        let fav_id = Uuid::new_v4();
        let handle = spawn(fav_id, root.clone(), Arc::clone(&db), None);

        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let s = handle.snapshot_stats();
            if s.initial_scan_done && s.initial_entries_written >= 3 {
                break;
            }
            if Instant::now() >= deadline {
                panic!("initial scan did not complete: {s:?}");
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        drop(handle);
    }

    #[test]
    fn drop_handle_stops_cleanly() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().join("fav");
        fs::create_dir_all(&root).unwrap();
        let db = Arc::new(SearchIndexDb::open_in_memory().unwrap());
        let handle = spawn(Uuid::new_v4(), root, db, None);
        // drop で無限待ちしないこと
        let t0 = Instant::now();
        drop(handle);
        assert!(
            t0.elapsed() < Duration::from_secs(5),
            "drop took too long ({:?})",
            t0.elapsed()
        );
    }
}
