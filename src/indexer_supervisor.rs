//! Indexer Supervisor (docs/search-expansion-design.md §3 アーキテクチャ図)。
//!
//! お気に入り単位で以下を統括するバックグラウンドスレッド:
//!
//! 1. 起動時に初期スキャン (Walker による 3-way diff) を実行
//! 2. FS watcher (notify-rs) を起動し、debounce 済みの変更イベントを受信
//! 3. 変更イベントを ingest_worker に流して二段整合性プロトコルで反映
//! 4. キャンセル・一時停止の受信
//! 5. 進捗統計の報告 (UI からの問い合わせ)
//!
//! ## 1 Supervisor = 1 お気に入り
//!
//! 複数お気に入りの場合は複数の Supervisor を起動する。`GlobalIoSemaphore` で
//! 全 Supervisor の I/O 同時実行を抑える。
//!
//! ## ライフサイクル
//!
//! ```text
//!   App::update で起動 ──┐
//!                         ▼
//!   IndexerSupervisor::spawn()
//!     1. FsWatcher 起動
//!     2. バックグラウンドスレッドで scan_loop 実行
//!   ...
//!   スレッドは:
//!     - 初期スキャン (1 回) → ingest
//!     - 以降は watcher イベントを待ち、debounce 済み変更を小刻みに ingest
//!   ...
//!   SupervisorHandle::stop() → cancel フラグ + watcher drop + thread join
//! ```

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Instant;
#[cfg(test)]
use std::time::Duration;

use crossbeam_channel::{bounded, select, Receiver, Sender};
use uuid::Uuid;

use crate::fts_index::FtsIndex;
use crate::fts_meta::FtsMetaDb;
use crate::ingest_worker::{IngestSession, IngestStats};
use crate::io_semaphore::{GlobalIoSemaphore, IoPriority};
use crate::search_walker::{self, CandidateFile, ScanParams};
use crate::search_watcher::{ChangeKind, DebouncedChange, FsWatcher, OVERFLOW_MARKER_PATH};

/// Supervisor が UI に返す進捗・状態スナップショット。
#[derive(Clone, Debug, Default)]
pub struct SupervisorStats {
    pub initial_scan_done: bool,
    pub ingested_ok: usize,
    pub ingested_failed: usize,
    pub deleted: usize,
    /// 最後のアクティビティから経過した時間 (インデックス管理ダイアログの "アイドル" 表示用)
    pub last_activity_ms_ago: Option<u64>,
    pub overflowed: bool,
    /// 初期スキャンの所要時間 (初回のみ記録, 以降の手動再構築では更新しない)
    pub initial_scan_duration_ms: Option<u64>,
    /// 直近のフル再スキャン所要時間 (initial_scan 含む, 更新あり)
    pub last_scan_duration_ms: Option<u64>,
    /// 直近のスキャンで走査された候補ファイル総数
    pub last_scan_total_scanned: usize,
    /// 直近のスキャン診断統計 (read_dir 失敗など)
    pub last_scan_diag: crate::search_walker::ScanDiag,
}

/// UI → Supervisor へのコマンド。
pub enum SupervisorCommand {
    /// 完全再スキャン (初期スキャンと同じ動作を手動トリガ)
    FullRescan,
    /// 停止 (drop で代替可能、明示コマンドも用意)
    Stop,
}

/// Supervisor のハンドル。Drop で自動停止する。
pub struct SupervisorHandle {
    pub favorite_id: Uuid,
    cmd_tx: Sender<SupervisorCommand>,
    cancel: Arc<AtomicBool>,
    stats: Arc<Mutex<SupervisorStats>>,
    thread: Option<JoinHandle<()>>,
}

impl SupervisorHandle {
    /// スナップショット取得 (短時間のロック)。
    pub fn snapshot_stats(&self) -> SupervisorStats {
        self.stats.lock().unwrap().clone()
    }

    /// 完全再スキャン要求 (インデックス管理ダイアログの「今すぐ再構築」で使用)。
    ///
    /// **非ブロッキング** (Codex round-10 Should-fix #1): `try_send` を使うため、
    /// cmd_tx は bounded(4) だがキューがフルなら silently drop する。
    /// これは長時間スキャン中に UI スレッドから連打された場合の UI freeze を防ぐため。
    /// "coalescing" 挙動: 同じ FullRescan が既にキューにあるなら追加リクエストは冗長なので
    /// 落としてよい (Supervisor は次のイベントで reconcile する)。
    pub fn request_full_rescan(&self) {
        match self.cmd_tx.try_send(SupervisorCommand::FullRescan) {
            Ok(_) => {}
            Err(crossbeam_channel::TrySendError::Full(_)) => {
                // キューにもう溜まっている → no-op (既にスキャン要求が届く予定)
                crate::logger::log(format!(
                    "indexer[{}]: request_full_rescan coalesced (queue full)",
                    self.favorite_id
                ));
            }
            Err(crossbeam_channel::TrySendError::Disconnected(_)) => {
                // supervisor は既に終了している
            }
        }
    }

    /// 明示停止。
    pub fn stop(self) {
        // drop で停止するので、ここでは明示 move させるだけ
        drop(self);
    }
}

impl Drop for SupervisorHandle {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::SeqCst);
        let _ = self.cmd_tx.send(SupervisorCommand::Stop);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// Supervisor を起動するための構築パラメータ。
pub struct SupervisorParams {
    pub favorite_id: Uuid,
    pub favorite_root: PathBuf,
    /// metadata インデックスが有効か (auto_index_metadata)。
    /// false の場合、Supervisor は起動しない (呼び出し側が spawn を呼ばない想定)。
    pub enable_metadata_index: bool,
}

/// Supervisor を起動する。`Arc<FtsMetaDb>` と `Arc<FtsIndex>` はアプリ全体で 1 本を共有する。
///
/// - `meta_db` と `fts` はどちらも内部で Mutex または同期機構を持つのでスレッド跨ぎで使える
/// - `io_sem` は全 Supervisor 共通で 1 つ
pub fn spawn(
    params: SupervisorParams,
    meta_db: Arc<FtsMetaDb>,
    fts: Arc<FtsIndex>,
    io_sem: Arc<GlobalIoSemaphore>,
) -> SupervisorHandle {
    assert!(
        params.enable_metadata_index,
        "Supervisor は metadata index 有効時のみ起動する"
    );

    let cancel = Arc::new(AtomicBool::new(false));
    let stats = Arc::new(Mutex::new(SupervisorStats::default()));
    let (cmd_tx, cmd_rx) = bounded::<SupervisorCommand>(4);
    let (change_tx, change_rx) = crossbeam_channel::unbounded::<DebouncedChange>();

    let fav_id = params.favorite_id;
    let root = params.favorite_root.clone();
    let cancel_cl = Arc::clone(&cancel);
    let stats_cl = Arc::clone(&stats);

    let thread = std::thread::Builder::new()
        .name(format!("indexer-{}", fav_id.as_simple()))
        .spawn(move || {
            supervisor_loop(
                fav_id,
                root,
                meta_db,
                fts,
                io_sem,
                cancel_cl,
                stats_cl,
                cmd_rx,
                change_tx,
                change_rx,
            );
        })
        .expect("failed to spawn indexer supervisor");

    SupervisorHandle {
        favorite_id: fav_id,
        cmd_tx,
        cancel,
        stats,
        thread: Some(thread),
    }
}

/// Supervisor バックグラウンドループの本体。
#[allow(clippy::too_many_arguments)]
fn supervisor_loop(
    favorite_id: Uuid,
    favorite_root: PathBuf,
    meta_db: Arc<FtsMetaDb>,
    fts: Arc<FtsIndex>,
    io_sem: Arc<GlobalIoSemaphore>,
    cancel: Arc<AtomicBool>,
    stats: Arc<Mutex<SupervisorStats>>,
    cmd_rx: Receiver<SupervisorCommand>,
    change_tx: Sender<DebouncedChange>,
    change_rx: Receiver<DebouncedChange>,
) {
    let session = IngestSession::new(favorite_id, favorite_root.clone(), &meta_db, &fts);

    // Tantivy writer は Supervisor の生存期間中 1 本を使い続ける (Tantivy 推奨)
    let mut writer = match fts.writer() {
        Ok(w) => w,
        Err(e) => {
            crate::logger::log(format!(
                "indexer[{favorite_id}]: writer init failed: {e}"
            ));
            return;
        }
    };

    // 1. Watcher 起動 (drop で停止)。失敗しても初期スキャンは動かす。
    let watcher = FsWatcher::start(favorite_id, &favorite_root, change_tx.clone()).ok();
    if watcher.is_none() {
        crate::logger::log(format!(
            "indexer[{favorite_id}]: FsWatcher start failed (will still run initial scan)"
        ));
    }

    // 2. 初期スキャン実行 (cancel は Arc のまま渡す — walker 途中で shutdown 可能に)
    run_initial_scan(
        favorite_id,
        &favorite_root,
        &session,
        &mut writer,
        &io_sem,
        Arc::clone(&cancel),
        &stats,
    );
    mark_activity(&stats);
    stats.lock().unwrap().initial_scan_done = true;

    // 3. 以降は watcher イベント + cmd を select で受信するループ
    loop {
        if cancel.load(Ordering::SeqCst) {
            break;
        }
        select! {
            recv(cmd_rx) -> msg => {
                match msg {
                    Ok(SupervisorCommand::Stop) => break,
                    Ok(SupervisorCommand::FullRescan) => {
                        run_initial_scan(
                            favorite_id,
                            &favorite_root,
                            &session,
                            &mut writer,
                            &io_sem,
                            Arc::clone(&cancel),
                            &stats,
                        );
                        mark_activity(&stats);
                    }
                    Err(_) => break, // Sender dropped
                }
            }
            recv(change_rx) -> msg => {
                match msg {
                    Ok(DebouncedChange { favorite_id: fid, path, kind }) => {
                        // 他お気に入りのイベントが来ることは無いが念のため
                        if fid != favorite_id {
                            continue;
                        }
                        // overflow マーカー → 全再スキャン
                        if path.to_string_lossy() == OVERFLOW_MARKER_PATH {
                            crate::logger::log(format!(
                                "indexer[{favorite_id}]: watcher overflow, running full rescan"
                            ));
                            stats.lock().unwrap().overflowed = true;
                            run_initial_scan(
                                favorite_id,
                                &favorite_root,
                                &session,
                                &mut writer,
                                &io_sem,
                                Arc::clone(&cancel),
                                &stats,
                            );
                            mark_activity(&stats);
                            continue;
                        }
                        apply_single_change(
                            &session,
                            &mut writer,
                            &io_sem,
                            &cancel,
                            &stats,
                            path,
                            kind,
                        );
                        mark_activity(&stats);
                    }
                    Err(_) => break, // watcher ended
                }
            }
        }
    }

    // 終了時に念のため commit (最後のバッチが残っていた場合)
    if let Err(e) = writer.commit() {
        crate::logger::log(format!("indexer[{favorite_id}]: final commit failed: {e}"));
    }
    drop(watcher); // 明示的に FsWatcher を drop
}

fn run_initial_scan(
    favorite_id: Uuid,
    favorite_root: &std::path::Path,
    session: &IngestSession,
    writer: &mut tantivy::IndexWriter,
    io_sem: &GlobalIoSemaphore,
    cancel: Arc<AtomicBool>,
    stats: &Mutex<SupervisorStats>,
) {
    // 所要時間計測: walker + ingest を含むフル scan の時間を拾う
    // (初期スキャンは supervisor 起動後 1 度のみ "initial"、以降の FullRescan /
    //  watcher overflow は last_scan_duration_ms のみ更新する)。
    let is_initial = !stats.lock().unwrap().initial_scan_done;
    let t_start = Instant::now();

    // walker にも supervisor と同じ Arc<AtomicBool> を渡す (Codex 6 回目指摘 #4)。
    // 旧実装は `Arc::new(AtomicBool::new(cancel.load()))` でスナップショットを渡しており、
    // 長時間 walk 中に SupervisorHandle::drop() が cancel を立てても伝わらなかった。
    let scan = match search_walker::scan(
        ScanParams {
            favorite_id,
            root: favorite_root.to_path_buf(),
            cancel: Arc::clone(&cancel),
        },
        session.meta_db,
        io_sem,
        IoPriority::Low,
    ) {
        Ok(r) => r,
        Err(e) => {
            crate::logger::log(format!(
                "indexer[{favorite_id}]: walker scan failed: {e}"
            ));
            return;
        }
    };
    let total_scanned = scan.total_scanned;
    let diag = scan.diag;
    // ingest_worker 側は `&AtomicBool` を取るのでここで unwrap
    let ingest_stats = match session.apply(
        scan.to_ingest,
        scan.to_delete,
        writer,
        io_sem,
        IoPriority::Low,
        &cancel,
    ) {
        Ok(s) => s,
        Err(e) => {
            crate::logger::log(format!(
                "indexer[{favorite_id}]: ingest apply failed: {e}"
            ));
            return;
        }
    };
    let dur_ms = t_start.elapsed().as_millis() as u64;
    crate::logger::log(format!(
        "indexer[{favorite_id}]: {} scan done in {dur_ms} ms \
         (scanned={total_scanned}, read_dir_err={}, file_type_err={}, metadata_err={}, depth_hits={})",
        if is_initial { "initial" } else { "rescan" },
        diag.read_dir_errors,
        diag.file_type_errors,
        diag.metadata_errors,
        diag.depth_limit_hits,
    ));
    update_stats(stats, &ingest_stats);
    {
        let mut s = stats.lock().unwrap();
        s.last_scan_duration_ms = Some(dur_ms);
        s.last_scan_total_scanned = total_scanned;
        s.last_scan_diag = diag;
        if is_initial {
            s.initial_scan_duration_ms = Some(dur_ms);
        }
    }
}

fn apply_single_change(
    session: &IngestSession,
    writer: &mut tantivy::IndexWriter,
    io_sem: &GlobalIoSemaphore,
    cancel: &AtomicBool,
    stats: &Mutex<SupervisorStats>,
    path: PathBuf,
    kind: ChangeKind,
) {
    // watcher から来るのは abs path。walker と同じ正規化で key を作る。
    let key = crate::search_index_db::normalize_path(&path);

    match kind {
        ChangeKind::Remove => {
            let ingest_stats = match session.apply(
                vec![],
                vec![key],
                writer,
                io_sem,
                IoPriority::Normal,
                cancel,
            ) {
                Ok(s) => s,
                Err(e) => {
                    crate::logger::log(format!(
                        "indexer[{}]: delete apply failed: {e}",
                        session.favorite_id
                    ));
                    return;
                }
            };
            update_stats(stats, &ingest_stats);
        }
        ChangeKind::Upsert => {
            // Walker と違って単一 path に対する apply → CandidateFile を直接組み立てる。
            // ファイル種別・mtime/size は現時点の FS から取得。
            let Some(cand) = build_candidate_from_path(&path, key) else {
                return;
            };
            let ingest_stats = match session.apply(
                vec![cand],
                vec![],
                writer,
                io_sem,
                IoPriority::Normal,
                cancel,
            ) {
                Ok(s) => s,
                Err(e) => {
                    crate::logger::log(format!(
                        "indexer[{}]: upsert apply failed: {e}",
                        session.favorite_id
                    ));
                    return;
                }
            };
            update_stats(stats, &ingest_stats);
        }
    }
}

fn build_candidate_from_path(abs_path: &std::path::Path, key: String) -> Option<CandidateFile> {
    let metadata = std::fs::metadata(abs_path).ok()?;
    if !metadata.is_file() {
        return None;
    }
    let mtime = metadata
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let file_size = metadata.len() as i64;
    let ext = abs_path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let kind = if ext == "zip" {
        search_walker::CandidateKind::Zip
    } else if ext == "pdf" {
        search_walker::CandidateKind::Pdf
    } else if crate::folder_tree::is_recognized_image_ext(&ext) {
        search_walker::CandidateKind::Image
    } else {
        // 非画像は無視 (typography 違い、notify が .tmp 等を拾う場合も)
        return None;
    };
    // Apple Double 除外
    if crate::folder_tree::is_apple_double(abs_path) {
        return None;
    }
    Some(CandidateFile {
        abs_path: abs_path.to_path_buf(),
        key,
        kind,
        mtime,
        file_size,
    })
}

fn update_stats(stats: &Mutex<SupervisorStats>, s: &IngestStats) {
    let mut lock = stats.lock().unwrap();
    lock.ingested_ok = lock.ingested_ok.saturating_add(s.ingested_ok);
    lock.ingested_failed = lock.ingested_failed.saturating_add(s.ingested_failed);
    lock.deleted = lock.deleted.saturating_add(s.deleted);
}

fn mark_activity(stats: &Mutex<SupervisorStats>) {
    // 本当は `last_activity: Option<Instant>` を持ちたいが、Instant は Clone だが
    // 現在時刻との差分を外部で取る方が柔軟。v1 では単純にゼロクリア方式で十分。
    let mut lock = stats.lock().unwrap();
    lock.last_activity_ms_ago = Some(0);
}


// -----------------------------------------------------------------------
// tests
// -----------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::Path;
    use tempfile::TempDir;

    fn setup() -> (TempDir, Arc<FtsMetaDb>, Arc<FtsIndex>, Arc<GlobalIoSemaphore>) {
        let tmp = TempDir::new().unwrap();
        let meta = Arc::new(FtsMetaDb::open_at(&tmp.path().join("m.db")).unwrap());
        let fts = Arc::new(FtsIndex::open_at(&tmp.path().join("fts")).unwrap());
        let sem = Arc::new(GlobalIoSemaphore::new(2));
        (tmp, meta, fts, sem)
    }

    fn write_image(dir: &Path, name: &str) {
        fs::write(dir.join(name), b"pretend-image").unwrap();
    }

    /// supervisor の初期スキャンが走り、stats に結果が反映されることを確認する。
    /// watcher の E2E 動作は通常環境依存なので、stats を polling で待つ。
    #[test]
    fn initial_scan_populates_stats() {
        let (tmp, meta, fts, sem) = setup();
        let fav_root = tmp.path().join("photos");
        fs::create_dir_all(&fav_root).unwrap();
        write_image(&fav_root, "a.jpg");
        write_image(&fav_root, "b.jpg");
        write_image(&fav_root, "archive.zip");

        let fav_id = Uuid::new_v4();
        let handle = spawn(
            SupervisorParams {
                favorite_id: fav_id,
                favorite_root: fav_root.clone(),
                enable_metadata_index: true,
            },
            Arc::clone(&meta),
            Arc::clone(&fts),
            Arc::clone(&sem),
        );

        // 初期スキャン完了を最長 5 秒待つ
        let deadline = Instant::now() + Duration::from_secs(5);
        let stats = loop {
            let s = handle.snapshot_stats();
            if s.initial_scan_done && s.ingested_ok >= 3 {
                break s;
            }
            if Instant::now() >= deadline {
                break s;
            }
            std::thread::sleep(Duration::from_millis(50));
        };
        assert!(
            stats.initial_scan_done,
            "初期スキャン完了フラグが立たない (stats: {stats:?})"
        );
        assert!(
            stats.ingested_ok >= 3,
            "ingest ok >=3 のはず (actual: {:?})",
            stats
        );

        drop(handle);
    }

    #[test]
    fn drop_handle_stops_cleanly() {
        let (tmp, meta, fts, sem) = setup();
        let fav_root = tmp.path().join("p");
        fs::create_dir_all(&fav_root).unwrap();
        let handle = spawn(
            SupervisorParams {
                favorite_id: Uuid::new_v4(),
                favorite_root: fav_root,
                enable_metadata_index: true,
            },
            meta,
            fts,
            sem,
        );
        // ただ drop するだけで join しないと test が終わらないことを確認
        drop(handle);
    }

    #[test]
    fn drop_during_long_scan_cancels_cleanly() {
        // Codex 6 回目指摘 #4 回帰: 長時間の初期スキャン中でも drop で cancel が伝わる
        let (tmp, meta, fts, sem) = setup();
        let fav_root = tmp.path().join("long");
        fs::create_dir_all(&fav_root).unwrap();
        // スキャン時間を稼ぐため多めに画像を作る (数百件あれば supervisor スレッドが
        // 初期スキャン中に drop される確率が高くなる)
        for i in 0..500 {
            write_image(&fav_root, &format!("img_{:04}.jpg", i));
        }
        let fav_id = Uuid::new_v4();
        let handle = spawn(
            SupervisorParams {
                favorite_id: fav_id,
                favorite_root: fav_root,
                enable_metadata_index: true,
            },
            meta,
            fts,
            sem,
        );
        // 初期スキャンが走り始めた直後 (stats が populate されるより前に)
        // drop して cancel がちゃんと伝わることを確認。
        // ここで thread が "完了" しないと drop の join が 2 秒以上かかるため、test 自体がタイムアウト。
        let t0 = Instant::now();
        drop(handle);
        let elapsed = t0.elapsed();
        assert!(
            elapsed < Duration::from_secs(5),
            "drop で cancel が伝わらず join がタイムアウトした: {:?}",
            elapsed
        );
    }

    #[test]
    fn full_rescan_command_triggers_additional_work() {
        let (tmp, meta, fts, sem) = setup();
        let fav_root = tmp.path().join("r");
        fs::create_dir_all(&fav_root).unwrap();
        write_image(&fav_root, "x.jpg");

        let fav_id = Uuid::new_v4();
        let handle = spawn(
            SupervisorParams {
                favorite_id: fav_id,
                favorite_root: fav_root.clone(),
                enable_metadata_index: true,
            },
            Arc::clone(&meta),
            Arc::clone(&fts),
            Arc::clone(&sem),
        );

        // 初期スキャン待ち
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if handle.snapshot_stats().initial_scan_done {
                break;
            }
            if Instant::now() >= deadline {
                panic!("initial scan did not complete");
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        let s_before = handle.snapshot_stats();

        // ファイルを追加してから手動再スキャン
        write_image(&fav_root, "y.jpg");
        handle.request_full_rescan();

        // ingested_ok が増えるのを待つ
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let s = handle.snapshot_stats();
            if s.ingested_ok > s_before.ingested_ok {
                break;
            }
            if Instant::now() >= deadline {
                panic!("full rescan did not ingest new file");
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        drop(handle);
    }
}
