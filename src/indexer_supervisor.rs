//! Indexer Supervisor (docs/archive/search-metadata/search-expansion-design.md §3 アーキテクチャ図)。
//!
//! お気に入り単位で以下を統括するバックグラウンドスレッド:
//!
//! 1. 起動時に初期スキャン (Walker による 3-way diff) を実行
//! 2. FS watcher (notify-rs) を起動し、debounce 済みの変更イベントを受信
//! 3. 変更イベントを ingest_worker に流して Tantivy First 書き込み順序で反映
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
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, Sender, bounded, select};
use uuid::Uuid;

use crate::fts_index::FtsIndex;
use crate::fts_meta::FtsMetaDb;
use crate::indexer_progress::ProgressReporter;
/// Filesystem observation and committed writes for the last Full; startup markers are separate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FullScanOutcome {
    Complete,
    Incomplete,
    Failed,
    Stopped,
}
use crate::ingest_worker::{IngestSession, IngestStats};
use crate::io_semaphore::{GlobalIoSemaphore, IoPriority};
use crate::search_walker::{self, CandidateFile, ScanParams};
use crate::search_watcher::{ChangeKind, DebouncedChange, FsWatcher, OVERFLOW_MARKER_PATH};

const WATCH_RETRY_DELAYS: [Duration; 5] = [
    Duration::from_secs(1),
    Duration::from_secs(2),
    Duration::from_secs(4),
    Duration::from_secs(8),
    Duration::from_secs(16),
];
const WATCH_HEALTH_POLL: Duration = Duration::from_secs(1);

#[cfg(test)]
thread_local! {
    static FULL_SCAN_GATE: std::cell::RefCell<Option<(std::sync::mpsc::Sender<()>, std::sync::mpsc::Receiver<()>)>> = const { std::cell::RefCell::new(None) };
    static FULL_SCAN_AFTER_APPLY_GATE: std::cell::RefCell<Option<(std::sync::mpsc::Sender<()>, std::sync::mpsc::Receiver<()>)>> = const { std::cell::RefCell::new(None) };
    static FTS_SCAN_EXTENSIONS_OVERRIDE: std::cell::RefCell<Option<Vec<String>>> = const { std::cell::RefCell::new(None) };
}

/// Supervisor が UI に返す進捗・状態スナップショット。
#[derive(Clone, Debug, Default)]
pub struct SupervisorStats {
    pub initial_scan_done: bool,
    pub initial_scan_skipped: bool,
    pub last_full_outcome: Option<FullScanOutcome>,
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
    /// "今何してる" の短い説明 (UI のリアルタイム進捗表示用)。
    /// walker / ingest_worker が ProgressReporter 経由で書き込み、snapshot 時に
    /// 読み出される。scan 区間外は None。
    pub current_activity: Option<String>,
    /// 現在のカウントベース進捗 (削除フェーズ / 取込フェーズで更新)。`None` の間は
    /// 削除/取込 が走っていない (= 起動直後の探索フェーズ等)。
    pub eta: Option<crate::indexer_progress::EtaSnapshot>,
    /// アクティブスキャン中 (walker + ingest を含むフル scan 実行中) か。
    /// true の間は UI が「⏳ スキャン中」を表示する。false かつ `initial_scan_done=true`
    /// なら「✅ 監視中」(notify-rs イベント待ち)。
    pub in_full_scan: bool,
}

/// UI → Supervisor へのコマンド。
pub enum SupervisorCommand {
    /// 完全再スキャン (初期スキャンと同じ動作を手動トリガ)
    FullRescan,
    MetadataFullRescan,
    /// 停止 (drop で代替可能、明示コマンドも用意)
    Stop,
}

/// Lightweight view; cloning or dropping it never owns the worker join.
#[derive(Clone)]
pub struct SupervisorControl {
    pub favorite_id: Uuid,
    cmd_tx: Sender<SupervisorCommand>,
    cancel: Arc<AtomicBool>,
    stats: Arc<Mutex<SupervisorStats>>,
    progress: ProgressReporter,
    similar_watch: Option<(
        crate::similar_index::SimilarIndexNotifier,
        crate::similar_index::SimilarWatchRegistration,
    )>,
}

impl SupervisorControl {
    #[cfg(test)]
    pub(crate) fn same_instance(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.cancel, &other.cancel)
    }
    #[cfg(test)]
    pub(crate) fn is_cancelled(&self) -> bool {
        self.cancel.load(Ordering::SeqCst)
    }

    pub fn snapshot_stats(&self) -> SupervisorStats {
        let mut s = self.stats.lock().unwrap().clone();
        s.current_activity = self.progress.snapshot();
        s.eta = self.progress.snapshot_eta();
        s
    }

    pub fn request_full_rescan(&self) {
        self.request(SupervisorCommand::FullRescan);
    }
    pub fn request_metadata_full_rescan(&self) {
        self.request(SupervisorCommand::MetadataFullRescan);
    }

    fn request(&self, command: SupervisorCommand) {
        if !self.cancel.load(Ordering::SeqCst) {
            let _ = self.cmd_tx.try_send(command);
        }
    }

    pub fn signal_stop(&self) {
        self.cancel.store(true, Ordering::SeqCst);
        if let Some((notifier, registration)) = &self.similar_watch {
            notifier.revoke_watch(registration);
        }
        // Cancellation remains authoritative when the bounded notification queue is full.
        let _ = self.cmd_tx.try_send(SupervisorCommand::Stop);
    }
}

/// Sole worker owner; cloneable control views never join or stop on drop.
pub struct SupervisorHandle {
    control: SupervisorControl,
    thread: Option<JoinHandle<()>>,
    finished_rx: std::sync::mpsc::Receiver<()>,
}

impl std::ops::Deref for SupervisorHandle {
    type Target = SupervisorControl;
    fn deref(&self) -> &SupervisorControl {
        &self.control
    }
}

impl SupervisorHandle {
    pub fn control(&self) -> SupervisorControl {
        self.control.clone()
    }
    pub fn stop(self) {
        drop(self);
    }

    /// Worker-side reconfiguration waits here after signal_stop.
    pub fn join(mut self) {
        self.signal_stop();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }

    /// False means detached at the manager-wide shutdown deadline.
    pub fn join_until(mut self, deadline: Instant) -> bool {
        self.signal_stop();
        let Some(thread) = self.thread.take() else {
            return true;
        };
        join_thread_until(thread, &self.finished_rx, deadline).is_some()
    }
}

fn join_thread_until<T>(
    thread: JoinHandle<T>,
    finished_rx: &std::sync::mpsc::Receiver<()>,
    deadline: Instant,
) -> Option<std::thread::Result<T>> {
    let remaining = deadline.saturating_duration_since(Instant::now());
    match finished_rx.recv_timeout(remaining) {
        Ok(()) | Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => Some(thread.join()),
        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
            drop(thread);
            None
        }
    }
}

impl Drop for SupervisorHandle {
    fn drop(&mut self) {
        if let Some(t) = self.thread.take() {
            self.control.signal_stop();
            let _ = t.join();
        }
    }
}

/// Supervisor construction parameters.
pub struct SupervisorParams {
    pub favorite_id: Uuid,
    pub favorite_root: PathBuf,
    pub excluded_roots: Vec<PathBuf>,
    /// metadata インデックスが有効か (auto_index_metadata)。
    /// false でも similar_notifier があれば watcher の共有だけを担う。
    pub enable_metadata_index: bool,
    /// 起動時の reconciliation と印の検証を済ませた場合だけ初回を省く。
    pub skip_initial_scan: bool,
    /// 同じ watcher のイベントで別バージョン索引の再照合も要求する。
    pub similar_notifier: Option<crate::similar_index::SimilarIndexNotifier>,
}

/// Supervisor を起動する。`Arc<FtsMetaDb>` と `Arc<FtsIndex>` はアプリ全体で 1 本を共有する。
///
/// - `meta_db` と `fts` はどちらも内部で Mutex または同期機構を持つのでスレッド跨ぎで使える
/// - `io_sem` は全 Supervisor 共通で 1 つ
/// - `writer` は **全 supervisor で共有** する `Arc<FtsWriterDispatcher>`。
///   Tantivy は 1 Index につき IndexWriter 1 本制約なので、所有権を専用 dispatcher
///   スレッドに集約し、各利用者は `WriterPriority` 付きでジョブを submit する
///   (interactive job が長時間 background ingest に starve しないように)。
pub fn spawn(
    params: SupervisorParams,
    meta_db: Arc<FtsMetaDb>,
    fts: Arc<FtsIndex>,
    writer: Arc<crate::fts_writer_dispatcher::FtsWriterDispatcher>,
    io_sem: Arc<GlobalIoSemaphore>,
    activity_gate: Arc<crate::activity_gate::ActivityGate>,
) -> SupervisorHandle {
    assert!(
        params.enable_metadata_index || params.similar_notifier.is_some(),
        "Supervisor は少なくとも 1 種類の索引が有効なときだけ起動する"
    );

    let cancel = Arc::new(AtomicBool::new(false));
    let stats = Arc::new(Mutex::new(SupervisorStats::default()));
    let progress = ProgressReporter::new();
    let (cmd_tx, cmd_rx) = bounded::<SupervisorCommand>(4);
    let (change_tx, change_rx) = crossbeam_channel::unbounded::<DebouncedChange>();
    let (finished_tx, finished_rx) = std::sync::mpsc::channel();

    let fav_id = params.favorite_id;
    let root = params.favorite_root.clone();
    let excluded_roots = params.excluded_roots.clone();
    let enable_metadata_index = params.enable_metadata_index;
    let skip_initial_scan = params.skip_initial_scan;
    let similar_notifier = params.similar_notifier.clone();
    let similar_registration = similar_notifier
        .as_ref()
        .and_then(|notifier| notifier.begin_watch(fav_id, &root));
    let registration_on_spawn_failure = similar_registration.clone();
    let cancel_cl = Arc::clone(&cancel);
    let similar_watch = similar_notifier.clone().zip(similar_registration.clone());
    let stats_cl = Arc::clone(&stats);
    let progress_cl = progress.clone();

    let thread = std::thread::Builder::new()
        .name(format!("indexer-{}", fav_id.as_simple()))
        .spawn(move || {
            crate::logger::log(format!(
                "indexer[{fav_id}]: supervisor starting for {}",
                root.display()
            ));
            supervisor_loop(
                fav_id,
                root,
                excluded_roots,
                enable_metadata_index,
                skip_initial_scan,
                similar_notifier,
                similar_registration,
                meta_db,
                fts,
                writer,
                io_sem,
                activity_gate,
                cancel_cl,
                stats_cl,
                progress_cl,
                cmd_rx,
                change_tx,
                change_rx,
            );
            let _ = finished_tx.send(());
        });
    let thread = match thread {
        Ok(thread) => thread,
        Err(error) => {
            if let (Some(notifier), Some(registration)) = (
                params.similar_notifier.as_ref(),
                registration_on_spawn_failure.as_ref(),
            ) {
                notifier.watch_unavailable(
                    registration,
                    format!("supervisor worker start failed: {error}"),
                );
            }
            panic!("failed to spawn indexer supervisor: {error}");
        }
    };

    SupervisorHandle {
        control: SupervisorControl {
            favorite_id: fav_id,
            cmd_tx,
            cancel,
            stats,
            progress,
            similar_watch,
        },
        thread: Some(thread),
        finished_rx,
    }
}

/// Supervisor バックグラウンドループの本体。
#[allow(clippy::too_many_arguments)]
fn supervisor_loop(
    favorite_id: Uuid,
    favorite_root: PathBuf,
    excluded_roots: Vec<PathBuf>,
    enable_metadata_index: bool,
    skip_initial_scan: bool,
    similar_notifier: Option<crate::similar_index::SimilarIndexNotifier>,
    similar_registration: Option<crate::similar_index::SimilarWatchRegistration>,
    meta_db: Arc<FtsMetaDb>,
    fts: Arc<FtsIndex>,
    writer: Arc<crate::fts_writer_dispatcher::FtsWriterDispatcher>,
    io_sem: Arc<GlobalIoSemaphore>,
    activity_gate: Arc<crate::activity_gate::ActivityGate>,
    cancel: Arc<AtomicBool>,
    stats: Arc<Mutex<SupervisorStats>>,
    progress: ProgressReporter,
    cmd_rx: Receiver<SupervisorCommand>,
    change_tx: Sender<DebouncedChange>,
    change_rx: Receiver<DebouncedChange>,
) {
    let session = IngestSession::new(favorite_id, favorite_root.clone(), &meta_db, &fts)
        .with_activity_gate(&activity_gate);

    // 1. Watcher registration is a barrier for the similar index.  Publish its terminal
    // immediately after recursive registration, before the metadata scan can hold this thread.
    let mut watch_retry_attempt = 0usize;
    let mut watcher = match FsWatcher::start(favorite_id, &favorite_root, change_tx.clone()) {
        Ok(watcher) => {
            if let (Some(notifier), Some(registration)) =
                (similar_notifier.as_ref(), similar_registration.as_ref())
            {
                if !cancel.load(Ordering::SeqCst) {
                    notifier.watch_ready(registration);
                }
            }
            Some(watcher)
        }
        Err(error) => {
            crate::logger::log(format!(
                "indexer[{favorite_id}]: FsWatcher start failed (will still run initial scan): {error}"
            ));
            if let (Some(notifier), Some(registration)) =
                (similar_notifier.as_ref(), similar_registration.as_ref())
            {
                notifier.watch_unavailable(registration, error.to_string());
            }
            None
        }
    };
    let mut next_watch_retry = watcher
        .is_none()
        .then(|| Instant::now() + WATCH_RETRY_DELAYS[0]);

    // 2. 初期スキャン実行 (cancel は Arc のまま渡す — walker 途中で shutdown 可能に)
    if enable_metadata_index && !skip_initial_scan {
        run_initial_scan(
            favorite_id,
            &favorite_root,
            &session,
            &writer,
            &io_sem,
            &excluded_roots,
            Arc::clone(&cancel),
            &stats,
            &progress,
        );
        mark_activity(&stats);
    }
    let initial_scan_duration_ms = {
        let mut stats = stats.lock().unwrap();
        stats.initial_scan_done = true;
        stats.initial_scan_skipped = enable_metadata_index && skip_initial_scan;
        stats.initial_scan_duration_ms.unwrap_or_default()
    };
    crate::perf::event(
        "index",
        "initial_scan_done",
        None,
        0,
        &[
            ("index_kind", serde_json::Value::from("fts")),
            (
                "skipped",
                serde_json::Value::from(enable_metadata_index && skip_initial_scan),
            ),
            (
                "favorite_id",
                serde_json::Value::from(favorite_id.to_string()),
            ),
            (
                "duration_ms",
                serde_json::Value::from(initial_scan_duration_ms),
            ),
        ],
    );
    // スキャン完了後は "今の作業" を消す (UI が ⏳→✅ に切り替わるタイミング)
    progress.clear();

    // 3. 以降は watcher イベント + cmd を select で受信するループ
    loop {
        if cancel.load(Ordering::SeqCst) {
            break;
        }
        if watcher.as_ref().is_some_and(FsWatcher::is_stopped) {
            watcher.take();
            watch_retry_attempt = 0;
            next_watch_retry = Some(Instant::now() + WATCH_RETRY_DELAYS[0]);
            if let (Some(notifier), Some(registration)) =
                (similar_notifier.as_ref(), similar_registration.as_ref())
            {
                notifier.watch_unavailable(registration, "watch debounce worker ended");
            }
            crate::logger::log(format!(
                "indexer[{favorite_id}]: watch debounce worker ended; scheduling registration retry"
            ));
        }
        if watcher.is_none() && next_watch_retry.is_some_and(|deadline| Instant::now() >= deadline)
        {
            match FsWatcher::start(favorite_id, &favorite_root, change_tx.clone()) {
                Ok(next) => {
                    watcher = Some(next);
                    next_watch_retry = None;
                    watch_retry_attempt = 0;
                    // Registration only closes the OS-watcher gap. Repair metadata while the
                    // Similar side remains Unavailable, then publish Ready so both consumers
                    // cross the same gap barrier.
                    if enable_metadata_index {
                        run_initial_scan(
                            favorite_id,
                            &favorite_root,
                            &session,
                            &writer,
                            &io_sem,
                            &excluded_roots,
                            Arc::clone(&cancel),
                            &stats,
                            &progress,
                        );
                        mark_activity(&stats);
                        progress.clear();
                    }
                    if !cancel.load(Ordering::SeqCst) {
                        if let (Some(notifier), Some(registration)) =
                            (similar_notifier.as_ref(), similar_registration.as_ref())
                        {
                            notifier.watch_ready(registration);
                        }
                    }
                    crate::logger::log(format!(
                        "indexer[{favorite_id}]: FsWatcher registration recovered"
                    ));
                }
                Err(error) => {
                    watch_retry_attempt = watch_retry_attempt.saturating_add(1);
                    if let (Some(notifier), Some(registration)) =
                        (similar_notifier.as_ref(), similar_registration.as_ref())
                    {
                        notifier.watch_unavailable(registration, error.to_string());
                    }
                    next_watch_retry = WATCH_RETRY_DELAYS
                        .get(watch_retry_attempt)
                        .map(|delay| Instant::now() + *delay);
                    crate::logger::log(format!(
                        "indexer[{favorite_id}]: FsWatcher retry {} failed: {error}",
                        watch_retry_attempt + 1
                    ));
                }
            }
        }
        let retry_wait = next_watch_retry
            .map(|deadline| deadline.saturating_duration_since(Instant::now()))
            .unwrap_or(WATCH_HEALTH_POLL)
            .min(WATCH_HEALTH_POLL);
        select! {
            recv(cmd_rx) -> msg => {
                match msg {
                    Ok(SupervisorCommand::Stop) => break,
                    Ok(command @ (SupervisorCommand::FullRescan | SupervisorCommand::MetadataFullRescan)) => {
                        if cancel.load(Ordering::SeqCst) {
                            break;
                        }
                        if let (Some(notifier), Some(registration)) =
                            (similar_notifier.as_ref(), similar_registration.as_ref())
                        {
                            if matches!(command, SupervisorCommand::FullRescan) { notifier.request_full(registration); }
                        }
                        if enable_metadata_index {
                            run_initial_scan(
                                favorite_id,
                                &favorite_root,
                                &session,
                                &writer,
                                &io_sem,
                                &excluded_roots,
                                Arc::clone(&cancel),
                                &stats,
                                &progress,
                            );
                            mark_activity(&stats);
                        }
                        if watcher.is_none() && next_watch_retry.is_none() {
                            watch_retry_attempt = 0;
                            next_watch_retry = Some(Instant::now() + WATCH_RETRY_DELAYS[0]);
                        }
                        progress.clear();
                    }
                    Err(_) => break, // Sender dropped
                }
            }
            recv(change_rx) -> msg => {
                match msg {
                    Ok(change) => {
                        let Some(DebouncedChange { favorite_id: fid, path, kind }) =
                            accept_change_unless_cancelled(&cancel, change)
                        else {
                            break;
                        };
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
                            if let (Some(notifier), Some(registration)) =
                                (similar_notifier.as_ref(), similar_registration.as_ref())
                            {
                                notifier.request_overflow(registration);
                            }
                            if enable_metadata_index {
                                run_initial_scan(
                                    favorite_id,
                                    &favorite_root,
                                    &session,
                                    &writer,
                                    &io_sem,
                                    &excluded_roots,
                                    Arc::clone(&cancel),
                                    &stats,
                                    &progress,
                                );
                                mark_activity(&stats);
                            }
                            progress.clear();
                            continue;
                        }
                        let similar_path = path.clone();
                        if enable_metadata_index {
                            apply_single_change(
                                &session,
                                &writer,
                                &io_sem,
                                &excluded_roots,
                                &cancel,
                                &stats,
                                &progress,
                                path,
                                kind,
                            );
                            mark_activity(&stats);
                        }
                        if let (Some(notifier), Some(registration)) =
                            (similar_notifier.as_ref(), similar_registration.as_ref())
                        {
                            notifier.request_change(registration, similar_path, kind);
                        }
                        progress.clear();
                    }
                    Err(_) => {
                        if let (Some(notifier), Some(registration)) =
                            (similar_notifier.as_ref(), similar_registration.as_ref())
                        {
                            notifier.watch_unavailable(registration, "watch event channel ended");
                        }
                        break;
                    }
                }
            }
            default(retry_wait) => {}
        }
    }

    // 終了時 commit は IndexerManager::drop 側で 1 回やる (共有 writer のため)。
    drop(watcher); // 明示的に FsWatcher を drop
    crate::logger::log(format!("indexer[{favorite_id}]: supervisor exiting"));
}

fn accept_change_unless_cancelled(
    cancel: &AtomicBool,
    change: DebouncedChange,
) -> Option<DebouncedChange> {
    if cancel.load(Ordering::SeqCst) {
        None
    } else {
        Some(change)
    }
}

#[allow(clippy::too_many_arguments)]
fn run_initial_scan(
    favorite_id: Uuid,
    favorite_root: &std::path::Path,
    session: &IngestSession,
    writer: &crate::fts_writer_dispatcher::FtsWriterDispatcher,
    io_sem: &GlobalIoSemaphore,
    excluded_roots: &[PathBuf],
    cancel: Arc<AtomicBool>,
    stats: &Mutex<SupervisorStats>,
    progress: &ProgressReporter,
) {
    if cancel.load(Ordering::SeqCst) {
        stats.lock().unwrap().last_full_outcome = Some(FullScanOutcome::Stopped);
        return;
    }
    // Every early write/observation error remains Failed; only complete paths publish success.
    stats.lock().unwrap().last_full_outcome = Some(FullScanOutcome::Failed);
    let fingerprint = fts_scan_fingerprint(
        favorite_root,
        favorite_id,
        excluded_roots,
        &fts_scan_extensions(),
    );
    // 初回・手動・overflow・watch回復の全 Full で、別指紋の行変更より前に失効を確定する。
    if let Err(error) = session.meta_db.prepare_full_scan(
        &crate::metadata_ownership::root_key(favorite_root),
        &fingerprint,
    ) {
        crate::logger::log(format!(
            "indexer[{favorite_id}]: scan marker invalidation failed: {error}"
        ));
        return;
    }
    // 所要時間計測: walker + ingest を含むフル scan の時間を拾う
    // (初期スキャンは supervisor 起動後 1 度のみ "initial"、以降の FullRescan /
    //  watcher overflow は last_scan_duration_ms のみ更新する)。
    let is_initial = !stats.lock().unwrap().initial_scan_done;
    let scan_kind = if is_initial { "initial" } else { "rescan" };
    let t_start = Instant::now();

    // UI 向けに「アクティブスキャン中」フラグを立てる (snapshot 時に ⏳ 表示)。
    // walker / ingest の早期 return 経路でも確実に false に戻すため RAII ガードで管理。
    // 旧実装は walker scan エラー / ingest apply エラーで return すると in_full_scan=true
    // のまま放置され、UI に「⏳ スキャン中」が居残るバグがあった。
    stats.lock().unwrap().in_full_scan = true;
    struct InFullScanGuard<'a>(&'a Mutex<SupervisorStats>);
    impl Drop for InFullScanGuard<'_> {
        fn drop(&mut self) {
            if let Ok(mut s) = self.0.lock() {
                s.in_full_scan = false;
            }
        }
    }
    let _scan_guard = InFullScanGuard(stats);
    #[cfg(test)]
    FULL_SCAN_GATE.with(|gate| {
        if let Some((started, release)) = gate.borrow_mut().take() {
            let _ = started.send(());
            let _ = release.recv();
        }
    });

    crate::logger::log(format!(
        "indexer[{favorite_id}]: {scan_kind} scan starting (walker phase)"
    ));

    // walker にも supervisor と同じ Arc<AtomicBool> を渡す (Codex 6 回目指摘 #4)。
    // 旧実装は `Arc::new(AtomicBool::new(cancel.load()))` でスナップショットを渡しており、
    // 長時間 walk 中に SupervisorHandle::drop() が cancel を立てても伝わらなかった。
    let t_walk = Instant::now();
    let scan = match search_walker::scan(
        ScanParams {
            favorite_id,
            root: favorite_root.to_path_buf(),
            excluded_roots: excluded_roots.to_vec(),
            cancel: Arc::clone(&cancel),
            progress: Some(progress.clone()),
        },
        session.meta_db,
        io_sem,
        IoPriority::Low,
        session.activity_gate,
    ) {
        Ok(r) => r,
        Err(e) => {
            crate::logger::log(format!("indexer[{favorite_id}]: walker scan failed: {e}"));
            if cancel.load(Ordering::SeqCst) {
                stats.lock().unwrap().last_full_outcome = Some(FullScanOutcome::Stopped);
            }
            return;
        }
    };
    if cancel.load(Ordering::SeqCst) {
        stats.lock().unwrap().last_full_outcome = Some(FullScanOutcome::Stopped);
        return;
    }
    let walk_ms = t_walk.elapsed().as_millis() as u64;
    let total_scanned = scan.total_scanned;
    let diag = scan.diag;
    let completeness = scan.completeness;
    let ingest_n = scan.to_ingest.len();
    let delete_n = scan.to_delete.len();
    crate::logger::log(format!(
        "indexer[{favorite_id}]: walker done in {walk_ms} ms \
         (scanned={total_scanned}, to_ingest={ingest_n}, to_delete={delete_n}, \
          observation={completeness:?})"
    ));

    // ingest フェーズでのみ共有 writer を lock する。walker は lock 不要なので、
    // 複数お気に入りの walk は並列で走る。ingest だけ直列化される。
    // UI 透明性: lock 待機中は progress を「取込待ち」に更新して、ユーザーから見て
    // 「動いていない?」と誤解されないようにする。
    progress.set(format!(
        "取込待ち (他のインデクサが writer を使用中)... 候補 {ingest_n} 件"
    ));
    crate::logger::log(format!(
        "indexer[{favorite_id}]: acquiring writer for ingest..."
    ));
    let t_ingest = Instant::now();
    // session.apply は writer Mutex を内部で flush 境界ごとに lock/unlock し、待機中の
    // 他 lock 利用者 (タグ書き込み worker など) に取り合いの機会を与える (CLAUDE.md
    // 並行処理ガイダンス + docs/async-architecture.md §5.5)。supervisor 側は Mutex 参照を
    // そのまま渡すだけ。
    let ingest_stats = match session.apply(
        scan.to_ingest,
        scan.to_delete,
        writer,
        io_sem,
        IoPriority::Low,
        &cancel,
        Some(progress),
    ) {
        Ok(s) => s,
        Err(e) => {
            crate::logger::log(format!("indexer[{favorite_id}]: ingest apply failed: {e}"));
            return;
        }
    };
    let ingest_ms = t_ingest.elapsed().as_millis() as u64;
    #[cfg(test)]
    FULL_SCAN_AFTER_APPLY_GATE.with(|gate| {
        if let Some((started, release)) = gate.borrow_mut().take() {
            let _ = started.send(());
            let _ = release.recv();
        }
    });
    let dur_ms = t_start.elapsed().as_millis() as u64;
    crate::logger::log(format!(
        "indexer[{favorite_id}]: {scan_kind} scan done in {dur_ms} ms \
         (walker={walk_ms}ms, ingest={ingest_ms}ms, scanned={total_scanned}, \
          ingest_ok={}, ingest_failed={}, deleted={}, \
          read_dir_err={}, entry_err={}, file_type_err={}, classification_err={}, \
          metadata_err={}, depth_hits={}, observation={completeness:?})",
        ingest_stats.ingested_ok,
        ingest_stats.ingested_failed,
        ingest_stats.deleted,
        diag.read_dir_errors,
        diag.entry_errors,
        diag.file_type_errors,
        diag.classification_errors,
        diag.metadata_errors,
        diag.depth_limit_hits,
    ));
    update_stats(stats, &ingest_stats);
    let outcome = if ingest_stats.cancelled || cancel.load(Ordering::SeqCst) {
        FullScanOutcome::Stopped
    } else if ingest_stats.ingested_failed > 0 {
        FullScanOutcome::Failed
    } else if completeness == crate::search_walker::ObservationCompleteness::Complete {
        FullScanOutcome::Complete
    } else {
        FullScanOutcome::Incomplete
    };
    if outcome == FullScanOutcome::Complete {
        if let Err(e) = session.meta_db.mark_scanned_once(
            &crate::metadata_ownership::root_key(favorite_root),
            &fingerprint,
        ) {
            crate::logger::log(format!(
                "indexer[{favorite_id}]: scanned_once write failed: {e}"
            ));
        }
    }
    {
        let mut s = stats.lock().unwrap();
        s.last_full_outcome = Some(outcome);
        s.last_scan_duration_ms = Some(dur_ms);
        s.last_scan_total_scanned = total_scanned;
        s.last_scan_diag = diag;
        if is_initial {
            s.initial_scan_duration_ms = Some(dur_ms);
        }
        // in_full_scan のクリアは関数末尾の InFullScanGuard::drop で行う。
    }
}

/// 入力: 正規化 root、自分の UUID、入れ子を含む除外 root、INDEX_VERSION、
/// 走査対象拡張子集合 (Susie 申告分を含む)。順番・重複・ASCII 大文字小文字は同一視する。
pub(crate) fn fts_scan_fingerprint(
    root: &std::path::Path,
    id: Uuid,
    excluded: &[PathBuf],
    extensions: &[String],
) -> String {
    let mut excluded = excluded
        .iter()
        .map(|p| crate::metadata_ownership::root_key(p))
        .collect::<Vec<_>>();
    excluded.sort();
    excluded.dedup();
    let mut extensions = extensions
        .iter()
        .map(|e| e.to_ascii_lowercase())
        .collect::<Vec<_>>();
    extensions.sort();
    extensions.dedup();
    serde_json::to_string(&(
        crate::metadata_ownership::root_key(root),
        id.to_string(),
        excluded,
        crate::fts_meta::INDEX_VERSION,
        extensions,
    ))
    .expect("string fingerprint serialization")
}

pub(crate) fn fts_scan_extensions() -> Vec<String> {
    #[cfg(test)]
    if let Some(extensions) = FTS_SCAN_EXTENSIONS_OVERRIDE.with(|value| value.borrow().clone()) {
        return extensions;
    }
    crate::folder_tree::SUPPORTED_EXTENSIONS
        .iter()
        .chain(crate::folder_tree::SUPPORTED_VIDEO_EXTENSIONS)
        .chain(crate::folder_tree::SUPPORTED_AUDIO_EXTENSIONS)
        .chain(["pdf", "epub"].iter())
        .map(|e| (*e).to_owned())
        .chain(crate::susie_loader::get_pool().extensions())
        .filter(|e| !e.eq_ignore_ascii_case("zip"))
        .collect()
}

pub(crate) fn can_skip_initial_scan(
    meta: &FtsMetaDb,
    enabled: bool,
    must_scan: bool,
    root: &std::path::Path,
    id: Uuid,
    excluded: &[PathBuf],
) -> bool {
    if !enabled || must_scan || meta.tantivy_rebuild_pending().unwrap_or(true) {
        return false;
    }
    let fingerprint = fts_scan_fingerprint(root, id, excluded, &fts_scan_extensions());
    match meta.scanned_once(&crate::metadata_ownership::root_key(root)) {
        Ok(Some(marker)) => marker == fingerprint,
        Ok(None) => false,
        Err(e) => {
            crate::logger::log(format!("indexer[{id}]: scanned_once read failed: {e}"));
            false
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn apply_single_change(
    session: &IngestSession,
    writer: &crate::fts_writer_dispatcher::FtsWriterDispatcher,
    io_sem: &GlobalIoSemaphore,
    excluded_roots: &[PathBuf],
    cancel: &AtomicBool,
    stats: &Mutex<SupervisorStats>,
    progress: &ProgressReporter,
    path: PathBuf,
    kind: ChangeKind,
) {
    if cancel.load(Ordering::SeqCst) {
        return;
    }
    if crate::books::path_is_under_any(&path, excluded_roots) {
        return;
    }
    // watcher から来るのは abs path。walker と同じ正規化で key を作る。
    let key = crate::search_index_db::normalize_path(&path);

    // 外部メタデータサイドカー (*.json / *.txt) の変更は、それ自体をアイテム索引の doc に
    // するのではなく、**対応する兄弟画像の再 ingest** に変換する (docs §14-2)。
    // Upsert/Remove どちらでも画像を再 ingest すれば、build_per_source_for_file が
    // サイドカーを読み直す (Remove なら sidecar_text がクリアされる)。
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if ext == "json" || ext == "txt" {
        reingest_images_for_sidecar(session, writer, io_sem, cancel, stats, progress, &path);
        return;
    }

    match kind {
        ChangeKind::Remove => {
            let ingest_stats = match session.apply(
                vec![],
                vec![key],
                writer,
                io_sem,
                IoPriority::Normal,
                cancel,
                Some(progress),
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
            //
            // **保険のフォールバック** (docs/search-test-plan.md rename バグ):
            // candidate が作れない (= ファイルが存在しない) ケースは
            //   - rename 元 (search_watcher が From を Remove にマップし損ねた場合)
            //   - Upsert 直後に削除された race
            //   - ファイル種別が対象外 (非画像/PDF/動画 — ZIP もここに含む §3.2)
            // のいずれか。前 2 者では旧エントリを残すと索引がゴミになる。
            // 安全側に倒し、候補が作れなければ Remove 経路にフォールバックする。
            // (種別外は通常 DB に行が無く delete 空振りだが、ZIP は旧版が入れた
            //  行をこの経路で掃除できる)。
            let Some(cand) = build_candidate_from_path(&path, key.clone()) else {
                let ingest_stats = match session.apply(
                    vec![],
                    vec![key],
                    writer,
                    io_sem,
                    IoPriority::Normal,
                    cancel,
                    Some(progress),
                ) {
                    Ok(s) => s,
                    Err(e) => {
                        crate::logger::log(format!(
                            "indexer[{}]: upsert→remove fallback apply failed: {e}",
                            session.favorite_id
                        ));
                        return;
                    }
                };
                update_stats(stats, &ingest_stats);
                return;
            };
            let ingest_stats = match session.apply(
                vec![cand],
                vec![],
                writer,
                io_sem,
                IoPriority::Normal,
                cancel,
                Some(progress),
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

/// サイドカー (*.json / *.txt) の変更イベントを、対応する兄弟画像の再 ingest に変換する
/// (docs §14-2)。`images_for_sidecar` が `<full>`/`<stem>` 形式の命名規則を逆引きする。
/// 対応画像が無い (孤立サイドカー) なら何もしない。
#[allow(clippy::too_many_arguments)]
fn reingest_images_for_sidecar(
    session: &IngestSession,
    writer: &crate::fts_writer_dispatcher::FtsWriterDispatcher,
    io_sem: &GlobalIoSemaphore,
    cancel: &AtomicBool,
    stats: &Mutex<SupervisorStats>,
    progress: &ProgressReporter,
    sidecar_path: &std::path::Path,
) {
    for img in crate::external_metadata::images_for_sidecar(sidecar_path) {
        if cancel.load(std::sync::atomic::Ordering::Relaxed) {
            return;
        }
        let key = crate::search_index_db::normalize_path(&img);
        let Some(cand) = build_candidate_from_path(&img, key) else {
            continue;
        };
        match session.apply(
            vec![cand],
            vec![],
            writer,
            io_sem,
            IoPriority::Normal,
            cancel,
            Some(progress),
        ) {
            Ok(s) => update_stats(stats, &s),
            Err(e) => crate::logger::log(format!(
                "indexer[{}]: sidecar reingest apply failed: {e}",
                session.favorite_id
            )),
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
    // ZIP はアイテム索引の対象外 (§3.2)。ext==zip は None を返し、呼び出し側の
    // upsert→remove フォールバックで既存 ZIP doc が索引から掃除される。
    // `is_recognized_image_ext` は Susie プラグイン申告の拡張子も拾うため、
    // アーカイブ系 Susie プラグインが "zip" を申告しても確実に弾けるよう、
    // 拡張子分類より前に明示的に reject する (Codex P2)。
    if ext == "zip" {
        return None;
    }
    let kind = if crate::folder_tree::is_paged_document_path(abs_path) {
        search_walker::CandidateKind::Pdf
    } else if crate::folder_tree::SUPPORTED_VIDEO_EXTENSIONS.contains(&ext.as_str()) {
        search_walker::CandidateKind::Video
    } else if crate::folder_tree::is_audio_ext(&ext) {
        search_walker::CandidateKind::Audio
    } else if crate::folder_tree::is_recognized_image_ext(&ext) {
        search_walker::CandidateKind::Image
    } else {
        // 非対応ファイルは無視 (typography 違い / notify が .tmp 等を拾う場合)
        return None;
    };
    // Apple Double 除外
    if crate::folder_tree::is_apple_double(abs_path) {
        return None;
    }
    // 差分用署名: 画像はサイドカー (同名 .json/.txt) を織り込む (walker と同じロジック、
    // docs/sidecar-metadata-ingest.md §14-3/§14-4)。これで fts_meta にサイドカー込みの
    // 署名が保存され、次回起動の walker 3-way diff と整合する。
    let (diff_mtime, diff_size) = if kind == search_walker::CandidateKind::Image {
        match crate::external_metadata::sidecar_signature(abs_path) {
            Some(sig) => (mtime.max(sig.mtime), file_size + sig.fingerprint),
            None => (mtime, file_size),
        }
    } else {
        (mtime, file_size)
    };
    Some(CandidateFile {
        abs_path: abs_path.to_path_buf(),
        key,
        kind,
        mtime,
        file_size,
        diff_mtime,
        diff_size,
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

    fn setup() -> (
        TempDir,
        Arc<FtsMetaDb>,
        Arc<FtsIndex>,
        Arc<crate::fts_writer_dispatcher::FtsWriterDispatcher>,
        Arc<GlobalIoSemaphore>,
        Arc<crate::activity_gate::ActivityGate>,
    ) {
        let tmp = TempDir::new().unwrap();
        let meta = Arc::new(FtsMetaDb::open_at(&tmp.path().join("m.db")).unwrap());
        let fts = Arc::new(FtsIndex::open_at(&tmp.path().join("fts")).unwrap());
        let raw_writer = fts.writer().unwrap();
        let writer =
            crate::fts_writer_dispatcher::FtsWriterDispatcher::start(raw_writer, Arc::clone(&fts));
        let sem = Arc::new(GlobalIoSemaphore::new(2));
        // テストでは idle 閾値 0ms にして wait_until_idle を即抜けさせる
        let gate = Arc::new(crate::activity_gate::ActivityGate::new(0));
        (tmp, meta, fts, writer, sem, gate)
    }

    fn write_image(dir: &Path, name: &str) {
        fs::write(dir.join(name), b"pretend-image").unwrap();
    }

    #[test]
    fn scanned_once_fingerprint_and_startup_skip_guards() {
        let (_tmp, meta, _fts, _writer, _sem, _gate) = setup();
        let root = Path::new("C:/Images/");
        let id = Uuid::new_v4();
        let extensions = fts_scan_extensions();
        let fingerprint = fts_scan_fingerprint(root, id, &[], &extensions);
        assert!(!can_skip_initial_scan(&meta, true, false, root, id, &[]));
        meta.mark_scanned_once(&crate::metadata_ownership::root_key(root), &fingerprint)
            .unwrap();
        assert!(can_skip_initial_scan(&meta, true, false, root, id, &[]));
        assert!(!can_skip_initial_scan(&meta, false, false, root, id, &[]));
        assert!(!can_skip_initial_scan(&meta, true, true, root, id, &[]));
        assert!(!can_skip_initial_scan(
            &meta,
            true,
            false,
            root,
            Uuid::new_v4(),
            &[]
        ));
        assert!(!can_skip_initial_scan(
            &meta,
            true,
            false,
            root,
            id,
            &[root.join("nested")]
        ));
        assert_eq!(
            fingerprint,
            fts_scan_fingerprint(Path::new("c:/images"), id, &[], &extensions)
        );
        let mut added = extensions.clone();
        added.push("susie-extra".into());
        assert_ne!(fingerprint, fts_scan_fingerprint(root, id, &[], &added));
        let excluded = vec![root.join("A"), root.join("b")];
        let reordered = vec![root.join("B"), root.join("a"), root.join("b")];
        assert_eq!(
            fts_scan_fingerprint(root, id, &excluded, &extensions),
            fts_scan_fingerprint(root, id, &reordered, &extensions)
        );
        meta.request_item_index_rebuild().unwrap();
        assert!(!can_skip_initial_scan(&meta, true, false, root, id, &[]));
    }

    #[test]
    fn skipped_initial_still_accepts_metadata_full_check_while_paused() {
        let (tmp, meta, fts, writer, sem, gate) = setup();
        let root = tmp.path().join("reused");
        fs::create_dir(&root).unwrap();
        write_image(&root, "a.jpg");
        let id = Uuid::new_v4();
        let marker = fts_scan_fingerprint(&root, id, &[], &fts_scan_extensions());
        let key = crate::metadata_ownership::root_key(&root);
        meta.mark_scanned_once(&key, &marker).unwrap();
        gate.set_paused(true);
        let handle = spawn(
            SupervisorParams {
                favorite_id: id,
                favorite_root: root.clone(),
                excluded_roots: Vec::new(),
                enable_metadata_index: true,
                skip_initial_scan: true,
                similar_notifier: None,
            },
            meta.clone(),
            fts,
            writer,
            sem,
            gate.clone(),
        );
        let deadline = Instant::now() + Duration::from_secs(30);
        while !handle.snapshot_stats().initial_scan_done {
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(5));
        }
        let stats = handle.snapshot_stats();
        assert!(stats.initial_scan_skipped);
        assert_eq!(stats.last_full_outcome, None);
        assert_eq!(stats.ingested_ok, 0);
        handle.control().request_metadata_full_rescan();
        while !handle.snapshot_stats().in_full_scan {
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(meta.list_path_owners().unwrap().is_empty());
        gate.set_paused(false);
        while handle.snapshot_stats().last_full_outcome != Some(FullScanOutcome::Complete) {
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(handle.snapshot_stats().ingested_ok, 1);
        assert_eq!(
            meta.scanned_once(&key).unwrap().as_deref(),
            Some(marker.as_str())
        );
        drop(handle);
    }

    #[test]
    fn skipped_initial_preserves_watch_deltas_marker_and_overflow_full() {
        let (tmp, meta, fts, writer, sem, gate) = setup();
        let root = tmp.path().join("reused_events");
        fs::create_dir(&root).unwrap();
        write_image(&root, "a.jpg");
        let id = Uuid::new_v4();
        let key = crate::metadata_ownership::root_key(&root);
        let marker = fts_scan_fingerprint(&root, id, &[], &fts_scan_extensions());
        meta.mark_scanned_once(&key, &marker).unwrap();
        let stats = Arc::new(Mutex::new(SupervisorStats::default()));
        let cancel = Arc::new(AtomicBool::new(false));
        let (cmd_tx, cmd_rx) = bounded(4);
        let (change_tx, change_rx) = crossbeam_channel::unbounded();
        let worker_meta = meta.clone();
        let worker_stats = stats.clone();
        let worker_cancel = cancel.clone();
        let worker_root = root.clone();
        let watcher_tx = change_tx.clone();
        let worker = std::thread::spawn(move || {
            supervisor_loop(
                id,
                worker_root,
                Vec::new(),
                true,
                true,
                None,
                None,
                worker_meta,
                fts,
                writer,
                sem,
                gate,
                worker_cancel,
                worker_stats,
                ProgressReporter::new(),
                cmd_rx,
                watcher_tx,
                change_rx,
            )
        });
        let deadline = Instant::now() + Duration::from_secs(30);
        while !stats.lock().unwrap().initial_scan_done {
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(stats.lock().unwrap().last_full_outcome, None);
        change_tx
            .send(DebouncedChange {
                favorite_id: id,
                path: root.join("a.jpg"),
                kind: ChangeKind::Upsert,
            })
            .unwrap();
        while stats.lock().unwrap().ingested_ok == 0 {
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(
            meta.scanned_once(&key).unwrap().as_deref(),
            Some(marker.as_str())
        );
        let stale = crate::search_index_db::normalize_path(&root.join("missing.jpg"));
        meta.upsert_meta_ok(&stale, id, &root, crate::fts_index::IndexKind::Image, 1, 2)
            .unwrap();
        change_tx
            .send(DebouncedChange {
                favorite_id: id,
                path: PathBuf::from(OVERFLOW_MARKER_PATH),
                kind: ChangeKind::Upsert,
            })
            .unwrap();
        while stats.lock().unwrap().last_full_outcome != Some(FullScanOutcome::Complete) {
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(stats.lock().unwrap().overflowed);
        assert!(meta.get(&stale).unwrap().is_none());
        cancel.store(true, Ordering::SeqCst);
        cmd_tx.send(SupervisorCommand::Stop).unwrap();
        worker.join().unwrap();
        assert_eq!(
            meta.scanned_once(&key).unwrap().as_deref(),
            Some(marker.as_str())
        );
    }

    #[test]
    fn bounded_join_detaches_when_deadline_is_exhausted() {
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let (finished_tx, finished_rx) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            release_rx.recv().unwrap();
            finished_tx.send(()).unwrap();
        });

        assert!(join_thread_until(worker, &finished_rx, Instant::now()).is_none());
        release_tx.send(()).unwrap();
        finished_rx.recv_timeout(Duration::from_secs(1)).unwrap();
    }

    #[test]
    fn cancel_during_full_scan_clears_activity_and_reports_stopped() {
        let (tmp, meta, fts, writer, sem, _gate) = setup();
        let root = tmp.path().join("cancelled");
        fs::create_dir(&root).unwrap();
        write_image(&root, "untouched.jpg");
        let cancel = Arc::new(AtomicBool::new(false));
        let stats = Arc::new(Mutex::new(SupervisorStats::default()));
        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let worker_cancel = cancel.clone();
        let worker_stats = stats.clone();
        let worker_meta = meta.clone();
        let worker = std::thread::spawn(move || {
            FULL_SCAN_GATE.with(|gate| *gate.borrow_mut() = Some((started_tx, release_rx)));
            let id = Uuid::new_v4();
            let session = IngestSession::new(id, root.clone(), &worker_meta, &fts);
            run_initial_scan(
                id,
                &root,
                &session,
                &writer,
                &sem,
                &[],
                worker_cancel,
                &worker_stats,
                &ProgressReporter::new(),
            );
        });
        started_rx.recv_timeout(Duration::from_secs(10)).unwrap();
        assert!(stats.lock().unwrap().in_full_scan);
        cancel.store(true, Ordering::SeqCst);
        release_tx.send(()).unwrap();
        worker.join().unwrap();
        let stats = stats.lock().unwrap();
        assert!(!stats.in_full_scan);
        assert_eq!(stats.last_full_outcome, Some(FullScanOutcome::Stopped));
        assert!(meta.list_path_owners().unwrap().is_empty());
        let conn = rusqlite::Connection::open(tmp.path().join("m.db")).unwrap();
        assert_eq!(
            conn.query_row("SELECT count(*) FROM scanned_once", [], |r| r
                .get::<_, usize>(0))
                .unwrap(),
            0
        );
    }

    #[test]
    fn changed_extension_set_cancelled_full_then_reopened_original_set_does_not_skip() {
        cancelled_full_reopen_for_extension_set(true);
    }

    #[test]
    fn same_fingerprint_cancelled_full_preserves_marker_after_deletion_and_reopen() {
        cancelled_full_reopen_for_extension_set(false);
    }

    fn cancelled_full_reopen_for_extension_set(change_extensions: bool) {
        let (tmp, meta, fts, writer, sem, gate) = setup();
        let root = tmp.path().join("extension_set");
        fs::create_dir(&root).unwrap();
        let id = Uuid::new_v4();
        let key = crate::metadata_ownership::root_key(&root);
        let current_extensions = fts_scan_extensions();
        let mut original_extensions = current_extensions.clone();
        let file = if change_extensions {
            // 旧 Susie 申告集合だけにある形式。現在の実 walker は非対応として削除候補にする。
            let extension = "miv_s3_retired_extension";
            assert!(!current_extensions.iter().any(|value| value == extension));
            original_extensions.push(extension.to_owned());
            let path = root.join(format!("retired.{extension}"));
            fs::write(&path, b"formerly supported by a plugin").unwrap();
            path
        } else {
            // 同じ指紋の Full が、終了中に消えた通常画像を削除する許容ケース。
            root.join("missing.jpg")
        };
        let file_key = crate::search_index_db::normalize_path(&file);
        let original = fts_scan_fingerprint(&root, id, &[], &original_extensions);
        meta.upsert_meta_ok(
            &file_key,
            id,
            &root,
            crate::fts_index::IndexKind::Image,
            1,
            1,
        )
        .unwrap();
        meta.mark_scanned_once(&key, &original).unwrap();
        meta.mark_scanned_once("c:/unrelated", "untouched").unwrap();
        let (committed_tx, committed_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let stats = Arc::new(Mutex::new(SupervisorStats::default()));
        let worker_cancel = Arc::clone(&cancel);
        let worker_stats = Arc::clone(&stats);
        let worker_meta = Arc::clone(&meta);
        let worker_fts = Arc::clone(&fts);
        let worker_writer = Arc::clone(&writer);
        let worker_sem = Arc::clone(&sem);
        let worker_root = root.clone();
        let worker = std::thread::spawn(move || {
            FULL_SCAN_AFTER_APPLY_GATE
                .with(|gate| *gate.borrow_mut() = Some((committed_tx, release_rx)));
            let session = IngestSession::new(id, worker_root.clone(), &worker_meta, &worker_fts);
            run_initial_scan(
                id,
                &worker_root,
                &session,
                &worker_writer,
                &worker_sem,
                &[],
                worker_cancel,
                &worker_stats,
                &ProgressReporter::new(),
            );
        });
        // 実 walker と削除バッチが確定した地点で止め、完了判定前に通常の取消を送る。
        committed_rx.recv_timeout(Duration::from_secs(30)).unwrap();
        let removed = meta.get(&file_key).unwrap().is_none();
        let marker_after_delete = meta.scanned_once(&key).unwrap();
        cancel.store(true, Ordering::SeqCst);
        release_tx.send(()).unwrap();
        worker.join().unwrap();
        assert!(removed);
        assert_eq!(
            stats.lock().unwrap().last_full_outcome,
            Some(FullScanOutcome::Stopped)
        );
        assert_eq!(
            marker_after_delete.as_deref(),
            (!change_extensions).then_some(original.as_str())
        );
        drop(meta);
        let reopened = Arc::new(FtsMetaDb::open_at(&tmp.path().join("m.db")).unwrap());
        assert_eq!(
            reopened.scanned_once(&key).unwrap().as_deref(),
            (!change_extensions).then_some(original.as_str())
        );
        assert_eq!(
            reopened.scanned_once("c:/unrelated").unwrap().as_deref(),
            Some("untouched")
        );
        // 元の申告集合へ戻す。thread-local 注入なので実 Susie pool や他テストを変更しない。
        struct RestoreExtensions;
        impl Drop for RestoreExtensions {
            fn drop(&mut self) {
                FTS_SCAN_EXTENSIONS_OVERRIDE.with(|value| *value.borrow_mut() = None);
            }
        }
        FTS_SCAN_EXTENSIONS_OVERRIDE.with(|value| *value.borrow_mut() = Some(original_extensions));
        let _restore = RestoreExtensions;
        let skip = can_skip_initial_scan(&reopened, true, false, &root, id, &[]);
        assert_eq!(skip, !change_extensions);
        let handle = spawn(
            SupervisorParams {
                favorite_id: id,
                favorite_root: root,
                excluded_roots: vec![],
                enable_metadata_index: true,
                skip_initial_scan: skip,
                similar_notifier: None,
            },
            reopened,
            fts,
            writer,
            sem,
            gate,
        );
        let deadline = Instant::now() + Duration::from_secs(30);
        while !handle.snapshot_stats().initial_scan_done {
            assert!(Instant::now() < deadline, "reopened Full stalled");
            std::thread::yield_now();
        }
        assert_eq!(
            handle.snapshot_stats().initial_scan_skipped,
            !change_extensions
        );
        drop(handle);
    }

    #[test]
    fn fingerprint_invalidation_failure_aborts_before_any_full_row_mutation() {
        let (tmp, meta, fts, writer, sem, _gate) = setup();
        let root = tmp.path().join("invalidation_failure");
        fs::create_dir(&root).unwrap();
        let id = Uuid::new_v4();
        let file = crate::search_index_db::normalize_path(&root.join("missing.jpg"));
        let key = crate::metadata_ownership::root_key(&root);
        meta.upsert_meta_ok(&file, id, &root, crate::fts_index::IndexKind::Image, 1, 1)
            .unwrap();
        meta.mark_scanned_once(&key, "different fingerprint")
            .unwrap();
        let conn = rusqlite::Connection::open(tmp.path().join("m.db")).unwrap();
        conn.execute_batch("CREATE TRIGGER fail_marker_delete BEFORE DELETE ON scanned_once BEGIN SELECT RAISE(ABORT, 'injected'); END;").unwrap();
        let (submitted, release) = writer.test_gate_next_batch();
        // 失効が退行して提出された場合も、gateで待ち続けず後のassertionで失敗させる。
        drop(release);
        let stats = Mutex::new(SupervisorStats::default());
        let session = IngestSession::new(id, root.clone(), &meta, &fts);
        run_initial_scan(
            id,
            &root,
            &session,
            &writer,
            &sem,
            &[],
            Arc::new(AtomicBool::new(false)),
            &stats,
            &ProgressReporter::new(),
        );
        assert_eq!(
            stats.lock().unwrap().last_full_outcome,
            Some(FullScanOutcome::Failed)
        );
        assert!(meta.get(&file).unwrap().is_some());
        assert_eq!(
            meta.scanned_once(&key).unwrap().as_deref(),
            Some("different fingerprint")
        );
        assert!(
            submitted.try_recv().is_err(),
            "Full must not submit Tantivy mutations after preparation failure"
        );
    }

    #[test]
    fn control_stop_is_nonblocking_when_notification_queue_is_full() {
        let (tx, rx) = bounded(4);
        for _ in 0..4 {
            tx.try_send(SupervisorCommand::FullRescan).ok().unwrap();
        }
        let control = SupervisorControl {
            favorite_id: Uuid::new_v4(),
            cmd_tx: tx,
            cancel: Arc::new(AtomicBool::new(false)),
            stats: Arc::new(Mutex::new(SupervisorStats::default())),
            progress: ProgressReporter::new(),
            similar_watch: None,
        };
        let worker_control = control.clone();
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            worker_control.signal_stop();
            done_tx.send(()).unwrap();
        });
        done_rx.recv_timeout(Duration::from_secs(10)).unwrap();
        assert!(control.cancel.load(Ordering::SeqCst));
        assert_eq!(rx.len(), 4);
        worker.join().unwrap();
    }

    #[test]
    fn full_outcome_distinguishes_incomplete_reload_and_sqlite_failures() {
        let (tmp, meta, fts, writer, sem, _gate) = setup();
        let root = tmp.path().join("full_outcome");
        fs::create_dir(&root).unwrap();
        write_image(&root, "first.jpg");
        let id = Uuid::new_v4();
        let session = IngestSession::new(id, root.clone(), &meta, &fts);
        let stats = Mutex::new(SupervisorStats::default());
        let cancel = Arc::new(AtomicBool::new(false));
        let progress = ProgressReporter::new();
        writer.test_set_reload_failure(true);
        run_initial_scan(
            id,
            &root,
            &session,
            &writer,
            &sem,
            &[],
            cancel.clone(),
            &stats,
            &progress,
        );
        assert_eq!(
            stats.lock().unwrap().last_full_outcome,
            Some(FullScanOutcome::Failed)
        );
        let root_key = crate::metadata_ownership::root_key(&root);
        assert_eq!(meta.scanned_once(&root_key).unwrap(), None);
        writer.test_set_reload_failure(false);
        run_initial_scan(
            id,
            &root,
            &session,
            &writer,
            &sem,
            &[],
            cancel.clone(),
            &stats,
            &progress,
        );
        assert_eq!(
            stats.lock().unwrap().last_full_outcome,
            Some(FullScanOutcome::Complete)
        );
        assert!(meta.scanned_once(&root_key).unwrap().is_some());
        meta.clear_scanned_once_for_root(&root_key).unwrap();
        let db = rusqlite::Connection::open(tmp.path().join("m.db")).unwrap();
        db.execute_batch("CREATE TRIGGER fail_full BEFORE INSERT ON files BEGIN SELECT RAISE(FAIL, 'injected SQLite failure'); END;").unwrap();
        write_image(&root, "second.jpg");
        run_initial_scan(
            id,
            &root,
            &session,
            &writer,
            &sem,
            &[],
            cancel.clone(),
            &stats,
            &progress,
        );
        assert_eq!(
            stats.lock().unwrap().last_full_outcome,
            Some(FullScanOutcome::Failed)
        );
        assert_eq!(meta.scanned_once(&root_key).unwrap(), None);
        db.execute_batch("DROP TRIGGER fail_full;").unwrap();
        let missing = tmp.path().join("missing");
        let missing_session = IngestSession::new(id, missing.clone(), &meta, &fts);
        run_initial_scan(
            id,
            &missing,
            &missing_session,
            &writer,
            &sem,
            &[],
            cancel,
            &stats,
            &progress,
        );
        assert_eq!(
            stats.lock().unwrap().last_full_outcome,
            Some(FullScanOutcome::Incomplete)
        );
        assert_eq!(
            meta.scanned_once(&crate::metadata_ownership::root_key(&missing))
                .unwrap(),
            None
        );
        assert!(!stats.lock().unwrap().in_full_scan);
    }

    #[test]
    fn queued_change_is_discarded_after_cancel() {
        let (tx, rx) = crossbeam_channel::unbounded();
        let cancel = AtomicBool::new(false);
        tx.send(DebouncedChange {
            favorite_id: Uuid::new_v4(),
            path: PathBuf::from(OVERFLOW_MARKER_PATH),
            kind: ChangeKind::Upsert,
        })
        .unwrap();
        cancel.store(true, Ordering::SeqCst);

        let queued = rx.recv().unwrap();
        assert!(accept_change_unless_cancelled(&cancel, queued).is_none());
    }

    #[test]
    fn incremental_reconcile_watch_health_poll_is_shorter_than_retry_backoff() {
        assert!(WATCH_HEALTH_POLL <= Duration::from_secs(1));
        assert!(WATCH_HEALTH_POLL <= WATCH_RETRY_DELAYS[0]);
    }

    /// ZIP はアイテム索引の対象外なので、notify 差分追従の候補ビルダは ZIP に対し
    /// None を返す。呼び出し側はこれを upsert→remove フォールバックに流し、既存
    /// ZIP doc を索引から掃除する (§3.2、Codex P3)。
    #[test]
    fn build_candidate_from_path_rejects_zip() {
        let tmp = TempDir::new().unwrap();
        let zip = tmp.path().join("album.zip");
        fs::write(&zip, b"PK").unwrap();
        let zip_key = crate::search_index_db::normalize_path(&zip);
        assert!(
            build_candidate_from_path(&zip, zip_key).is_none(),
            "ZIP は候補にならない"
        );
        // 回帰確認: 画像は従来どおり候補になる
        let jpg = tmp.path().join("photo.jpg");
        fs::write(&jpg, b"x").unwrap();
        let jpg_key = crate::search_index_db::normalize_path(&jpg);
        assert!(
            build_candidate_from_path(&jpg, jpg_key).is_some(),
            "画像は候補になる"
        );

        // 回帰: watcher 差分でも共通の音声拡張子判定を通って Audio 候補になる。
        let audio = tmp.path().join("track.FLAC");
        fs::write(&audio, b"fake-flac").unwrap();
        let audio_key = crate::search_index_db::normalize_path(&audio);
        let candidate = build_candidate_from_path(&audio, audio_key).expect("音声は候補になる");
        assert_eq!(candidate.kind, search_walker::CandidateKind::Audio);
        let epub = tmp.path().join("book.EPUB");
        fs::write(&epub, b"epub").unwrap();
        let epub_key = crate::search_index_db::normalize_path(&epub);
        let candidate = build_candidate_from_path(&epub, epub_key).expect("EPUB は本の候補になる");
        assert_eq!(candidate.kind, search_walker::CandidateKind::Pdf);
    }

    /// supervisor の初期スキャンが走り、stats に結果が反映されることを確認する。
    /// watcher の E2E 動作は通常環境依存なので、stats を polling で待つ。
    #[test]
    fn initial_scan_populates_stats() {
        let (tmp, meta, fts, writer, sem, gate) = setup();
        let fav_root = tmp.path().join("photos");
        fs::create_dir_all(&fav_root).unwrap();
        write_image(&fav_root, "a.jpg");
        write_image(&fav_root, "b.jpg");
        write_image(&fav_root, "c.jpg");

        let fav_id = Uuid::new_v4();
        let handle = spawn(
            SupervisorParams {
                favorite_id: fav_id,
                favorite_root: fav_root.clone(),
                excluded_roots: Vec::new(),
                enable_metadata_index: true,
                skip_initial_scan: false,
                similar_notifier: None,
            },
            Arc::clone(&meta),
            Arc::clone(&fts),
            Arc::clone(&writer),
            Arc::clone(&sem),
            Arc::clone(&gate),
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
        let (tmp, meta, fts, writer, sem, gate) = setup();
        let fav_root = tmp.path().join("p");
        fs::create_dir_all(&fav_root).unwrap();
        let handle = spawn(
            SupervisorParams {
                favorite_id: Uuid::new_v4(),
                favorite_root: fav_root,
                excluded_roots: Vec::new(),
                enable_metadata_index: true,
                skip_initial_scan: false,
                similar_notifier: None,
            },
            meta,
            fts,
            writer,
            sem,
            gate,
        );
        // ただ drop するだけで join しないと test が終わらないことを確認
        drop(handle);
    }

    #[test]
    fn drop_during_long_scan_cancels_cleanly() {
        // Codex 6 回目指摘 #4 回帰: 長時間の初期スキャン中でも drop で cancel が伝わる
        let (tmp, meta, fts, writer, sem, gate) = setup();
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
                excluded_roots: Vec::new(),
                enable_metadata_index: true,
                skip_initial_scan: false,
                similar_notifier: None,
            },
            meta,
            fts,
            writer,
            sem,
            gate,
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

    /// 回帰テスト: 2 つのお気に入りに対応する supervisor を同時に走らせた場合、
    /// 共有 IndexWriter 経由で両方とも初期スキャンを完了できること。
    ///
    /// 2026-04 バグ: 共有 writer 化前は 2 つ目の supervisor が
    /// `fts.writer()` を呼んだ瞬間に LockBusy で return し、UI では永遠に
    /// ⏳ スキャン中 の表示になっていた。共有 writer 化でこの経路を塞いだ。
    #[test]
    fn two_supervisors_share_writer_and_both_finish() {
        let (tmp, meta, fts, writer, sem, gate) = setup();
        let root_a = tmp.path().join("a");
        let root_b = tmp.path().join("b");
        fs::create_dir_all(&root_a).unwrap();
        fs::create_dir_all(&root_b).unwrap();
        write_image(&root_a, "a1.jpg");
        write_image(&root_a, "a2.jpg");
        write_image(&root_b, "b1.jpg");
        write_image(&root_b, "b2.jpg");

        let fav_a = Uuid::new_v4();
        let fav_b = Uuid::new_v4();
        let handle_a = spawn(
            SupervisorParams {
                favorite_id: fav_a,
                favorite_root: root_a,
                excluded_roots: Vec::new(),
                enable_metadata_index: true,
                skip_initial_scan: false,
                similar_notifier: None,
            },
            Arc::clone(&meta),
            Arc::clone(&fts),
            Arc::clone(&writer),
            Arc::clone(&sem),
            Arc::clone(&gate),
        );
        let handle_b = spawn(
            SupervisorParams {
                favorite_id: fav_b,
                favorite_root: root_b,
                excluded_roots: Vec::new(),
                enable_metadata_index: true,
                skip_initial_scan: false,
                similar_notifier: None,
            },
            Arc::clone(&meta),
            Arc::clone(&fts),
            Arc::clone(&writer),
            Arc::clone(&sem),
            Arc::clone(&gate),
        );

        // 両方が initial_scan_done になることを 5 秒以内に確認
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let done_a = handle_a.snapshot_stats().initial_scan_done;
            let done_b = handle_b.snapshot_stats().initial_scan_done;
            if done_a && done_b {
                break;
            }
            if Instant::now() >= deadline {
                panic!("2 supervisor の initial scan が両方完了しない (a={done_a}, b={done_b})");
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(handle_a.snapshot_stats().ingested_ok >= 2);
        assert!(handle_b.snapshot_stats().ingested_ok >= 2);

        drop(handle_a);
        drop(handle_b);
    }

    #[test]
    fn full_rescan_command_triggers_additional_work() {
        let (tmp, meta, fts, writer, sem, gate) = setup();
        let fav_root = tmp.path().join("r");
        fs::create_dir_all(&fav_root).unwrap();
        write_image(&fav_root, "x.jpg");

        let fav_id = Uuid::new_v4();
        let handle = spawn(
            SupervisorParams {
                favorite_id: fav_id,
                favorite_root: fav_root.clone(),
                excluded_roots: Vec::new(),
                enable_metadata_index: true,
                skip_initial_scan: false,
                similar_notifier: None,
            },
            Arc::clone(&meta),
            Arc::clone(&fts),
            Arc::clone(&writer),
            Arc::clone(&sem),
            Arc::clone(&gate),
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

    /// `InFullScanGuard` の RAII 解放: 通常完了経路で `in_full_scan` が false に戻ること。
    /// RAII ベースなので関数のどの経路 (正常完了 / walker error / ingest error 早期 return /
    /// cancel) で抜けても guard は必ず drop される。本テストはその代表として完了経路を
    /// 固定し、`drop_during_long_scan_cancels_cleanly` が cancel 経路で join 完了を保証する。
    ///
    /// 0.8.x の修正前は walker / ingest 早期 return で `in_full_scan = true` が残置され、
    /// UI の「⏳ スキャン中」表示が居残るバグがあった。Drop ベースのガードへの置換が回帰しないよう守る。
    #[test]
    fn in_full_scan_resets_to_false_after_initial_scan_done() {
        let (tmp, meta, fts, writer, sem, gate) = setup();
        let fav_root = tmp.path().join("scan_guard");
        fs::create_dir_all(&fav_root).unwrap();
        for i in 0..5 {
            write_image(&fav_root, &format!("img_{i}.jpg"));
        }

        let fav_id = Uuid::new_v4();
        let handle = spawn(
            SupervisorParams {
                favorite_id: fav_id,
                favorite_root: fav_root.clone(),
                excluded_roots: Vec::new(),
                enable_metadata_index: true,
                skip_initial_scan: false,
                similar_notifier: None,
            },
            Arc::clone(&meta),
            Arc::clone(&fts),
            Arc::clone(&writer),
            Arc::clone(&sem),
            Arc::clone(&gate),
        );

        // 初期スキャン完了まで待つ
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let s = handle.snapshot_stats();
            if s.initial_scan_done {
                break;
            }
            if Instant::now() >= deadline {
                panic!("initial scan did not complete");
            }
            std::thread::sleep(Duration::from_millis(20));
        }

        // 完了直後は in_full_scan が drop されているはず。Loop 1 周で観測されない場合
        // (drop 直前の極小タイミング) は最大 500ms polling して true→false 遷移を許容する。
        let deadline = Instant::now() + Duration::from_millis(500);
        let mut final_state = handle.snapshot_stats().in_full_scan;
        while final_state {
            if Instant::now() >= deadline {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
            final_state = handle.snapshot_stats().in_full_scan;
        }
        assert!(
            !final_state,
            "scan 完了後に in_full_scan が false に戻っていない (RAII guard 退行)"
        );

        drop(handle);
    }

    /// cancel 経路でも RAII guard が drop されて `in_full_scan` が false に戻ること。
    /// `stats` Arc を handle drop の前に clone することで、handle 経由 API が無くても
    /// 直接観測する (Codex P3 指摘: drop_during_long_scan_cancels_cleanly は join
    /// 完了を保証するだけで in_full_scan の値は見ていなかった)。
    #[test]
    fn in_full_scan_resets_to_false_after_cancel() {
        let (tmp, meta, fts, writer, sem, gate) = setup();
        let fav_root = tmp.path().join("cancel_scan_guard");
        fs::create_dir_all(&fav_root).unwrap();
        for i in 0..50 {
            write_image(&fav_root, &format!("img_{:03}.jpg", i));
        }

        let handle = spawn(
            SupervisorParams {
                favorite_id: Uuid::new_v4(),
                favorite_root: fav_root,
                excluded_roots: Vec::new(),
                enable_metadata_index: true,
                skip_initial_scan: false,
                similar_notifier: None,
            },
            meta,
            fts,
            writer,
            sem,
            gate,
        );
        // handle drop 後も観測できるよう stats Arc を事前に clone。
        let stats_arc = Arc::clone(&handle.stats);

        std::thread::sleep(Duration::from_millis(20));

        let t0 = Instant::now();
        drop(handle);
        let join_elapsed = t0.elapsed();
        assert!(
            join_elapsed < Duration::from_secs(5),
            "cancel + join に時間がかかりすぎ: {:?}",
            join_elapsed
        );

        // join 完了 = supervisor thread 終了 = InFullScanGuard も drop 済み。
        let in_full_scan = stats_arc.lock().unwrap().in_full_scan;
        assert!(
            !in_full_scan,
            "cancel 経路でも RAII guard が drop されて in_full_scan=false に戻る"
        );
    }
}
