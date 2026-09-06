//! お気に入り配下の「別バージョン」索引ジョブと遅延ロード線形検索。

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};
use std::fs::{File, OpenOptions};
use std::io::{BufReader, BufWriter, Read, Seek, SeekFrom, Write};
use std::path::{Component, Path, PathBuf, Prefix};
use std::sync::{
    Arc, Condvar, Mutex, OnceLock, RwLock, Weak,
    atomic::{AtomicBool, AtomicU64, Ordering},
};
use std::time::Duration;

use image::GenericImageView;
use sha2::{Digest, Sha256};

use crate::dupe::{self, Algo, Sig};
use crate::similar_db::{
    CompactSearchRow as CompactIndexRecord, CompactSearchRows, CompletedIndexStats, ContainerKind,
    Freshness, ItemKind, SearchContentStamp, SearchRow, SimilarDb, StoredItem,
    current_hash_version,
};
use crate::similar_image::{
    PDF_RENDER_LONG_EDGE, ProxySource, SimilarImageFormat, proxy_from_source,
};

/// §9.5 の単体画像帯。いずれも実測済みで、索引ジョブの採否には使わない。
pub const NEARLY_IDENTICAL_MAX_DISTANCE: u32 = 8;
pub const OTHER_VERSION_MAX_DISTANCE: u32 = 48;
/// §19.4 の本単位パラメータ。単体画像帯とは共有しない。
pub const BOOK_RADIUS: u32 = 32;
pub const BOOK_COVERAGE: f32 = 0.5;
pub const BOOK_MIN_MATCHED_PAGES: u32 = 3;
pub const BOOK_MAX_BOOKS_PER_PAGE: u32 = 8;
pub const BOOK_MIN_QUALITY: u8 = 1;

// 2026-09-05 の HDD 実測では 16 並列が E: 102.7 MB/s / D: 150.8 MB/s でピーク、
// 32 並列では 85.2 / 111.7 MB/s へ低下した。複数ドライブが同時に走っても各 16 が
// 合計 32 にならないよう、ボリューム単位はピークの一段手前である 8 に制限する。
const INDEX_GLOBAL_OUTSTANDING_LIMIT: usize = 16;
const INDEX_PER_VOLUME_OUTSTANDING_LIMIT: usize = 8;
// 操作中も差分照合を完全には止めず、既存 ActivityGate の状態で新規開始を 1 本へ絞る。
const INDEX_ACTIVE_GLOBAL_OUTSTANDING_LIMIT: usize = 1;
const INDEX_ACTIVE_PER_VOLUME_OUTSTANDING_LIMIT: usize = 1;
const INDEX_LIMIT_RECHECK: Duration = Duration::from_millis(50);

const COMPACT_SIDECAR_FILE: &str = "similar.compact";
const COMPACT_SIDECAR_MAGIC: [u8; 8] = *b"MIVSIMC1";
const COMPACT_SIDECAR_FORMAT_VERSION: u32 = 1;
const COMPACT_SIDECAR_HEADER_LEN: usize = 104;
const COMPACT_SIDECAR_RECORD_LEN: usize = 44;
const COMPACT_SIDECAR_IO_RECORDS: usize = 16 * 1024;
static COMPACT_SIDECAR_TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IndexStage {
    Opening,
    Scanning,
    Pruning,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct IndexReport {
    pub discovered: u64,
    pub processed: u64,
    pub indexed: u64,
    pub unchanged: u64,
    pub removed: u64,
    pub containers_completed: u64,
    pub password_required_pdfs: u64,
    pub corrupt_containers: u64,
    pub zero_page_containers: u64,
    pub decode_failures: u64,
    pub io_failures: u64,
    pub errors: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunningProgress {
    pub stage: IndexStage,
    pub current_path: Option<PathBuf>,
    pub report: IndexReport,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum IndexProgress {
    Idle,
    Running(RunningProgress),
    Complete(IndexReport),
    Cancelled(IndexReport),
    Failed(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MatchBand {
    NearlyIdentical,
    OtherVersion,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QueryHit {
    pub row_id: u32,
    pub item_key: String,
    pub kind: ItemKind,
    pub container_key: Option<String>,
    pub page_index: Option<u32>,
    pub distance: u32,
    pub band: MatchBand,
    pub mtime: i64,
    pub file_size: i64,
    pub width: u32,
    pub height: u32,
    pub format: SimilarImageFormat,
    pub origin_width: u32,
    pub origin_height: u32,
    /// 問い合わせ worker が一度だけ実在パスへ戻した表示・遷移先。
    /// 消失済みなら正規化 key から組み立てた fallback を保持する。
    pub target: Option<SimilarItemTarget>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ItemQuery {
    NoIndex,
    Preparing,
    Ready(Vec<QueryHit>),
    Featureless,
    NotIndexed,
    Failed(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IndexSummary {
    pub completed_at_unix_secs: i64,
    pub registered_items: u64,
    pub password_required_pdfs: u64,
    pub corrupt_containers: u64,
    pub zero_page_containers: u64,
    pub decode_failures: u64,
    pub io_failures: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum IndexSummaryStatus {
    NoIndex,
    Preparing,
    Ready(IndexSummary),
    Failed(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SimilarItemTarget {
    File(PathBuf),
    ZipPage {
        zip_path: PathBuf,
        entry_name: String,
    },
    PdfPage {
        pdf_path: PathBuf,
        page_num: u32,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub struct BookRelationHit {
    pub other_container_key: String,
    pub pair: dupe::book::BookPair,
}

#[derive(Clone, Debug, PartialEq)]
pub enum BookQuery {
    Preparing,
    Ready(Vec<BookRelationHit>),
    Featureless,
    NotIndexed,
    NotBook,
    Failed(String),
}

pub struct SimilarIndexManager {
    data_dir: PathBuf,
    progress: Arc<Mutex<IndexProgress>>,
    memory: Arc<Mutex<MemoryState>>,
    summary: Arc<Mutex<SummaryState>>,
    memory_epoch: Arc<AtomicU64>,
    item_query: Arc<Mutex<ItemQueryCache>>,
    book_query: Arc<Mutex<BookQueryState>>,
    enabled_roots: Arc<RwLock<Vec<String>>>,
    scheduler: Arc<SimilarIndexScheduler>,
}

impl SimilarIndexManager {
    /// DB を開かない軽量 constructor。起動時 I/O を増やさない。
    pub fn new(data_dir: PathBuf) -> Self {
        let progress = Arc::new(Mutex::new(IndexProgress::Idle));
        let memory = Arc::new(Mutex::new(MemoryState::Unloaded));
        let summary = Arc::new(Mutex::new(SummaryState::Unloaded));
        let memory_epoch = Arc::new(AtomicU64::new(0));
        let item_query = Arc::new(Mutex::new(ItemQueryCache::default()));
        let book_query = Arc::new(Mutex::new(BookQueryState::Idle));
        let enabled_roots = Arc::new(RwLock::new(Vec::new()));
        let scheduler = Arc::new(SimilarIndexScheduler {
            data_dir: data_dir.clone(),
            progress: Arc::clone(&progress),
            memory: Arc::clone(&memory),
            summary: Arc::clone(&summary),
            memory_epoch: Arc::clone(&memory_epoch),
            item_query: Arc::clone(&item_query),
            book_query: Arc::clone(&book_query),
            prefill_db: Arc::new(Mutex::new(None)),
            enabled_roots: Arc::clone(&enabled_roots),
            active_cancel: Mutex::new(None),
            state: Mutex::new(SchedulerState::default()),
        });
        Self {
            data_dir,
            progress,
            memory,
            summary,
            memory_epoch,
            item_query,
            book_query,
            enabled_roots,
            scheduler,
        }
    }

    /// `auto_index_similar` が有効なお気に入りを、索引の完全な対象 snapshot として反映する。
    /// I/O は scheduler worker 内だけで行い、この呼び出しは UI スレッドをブロックしない。
    pub fn configure(
        &self,
        favorites: &[crate::settings::FavoriteEntry],
        pdf_passwords: crate::pdf_passwords::PdfPasswordStore,
        activity_gate: Option<Arc<crate::activity_gate::ActivityGate>>,
    ) {
        let roots: Vec<PathBuf> = favorites
            .iter()
            .filter(|favorite| favorite.auto_index_similar)
            .map(|favorite| favorite.path.clone())
            .collect();
        let should_load = !roots.is_empty();
        self.scheduler
            .configure(roots, pdf_passwords, activity_gate);
        if should_load {
            start_memory_load(
                &self.data_dir,
                &self.memory,
                &self.memory_epoch,
                &self.item_query,
                MemoryLoadTrigger::Configure,
            );
        }
    }

    /// favorite を OFF にしたとき、その範囲だけを worker 上で即時削除する。
    /// 後続の全走査が中断・失敗しても、OFF にした範囲の行を残さない。
    pub fn purge_disabled_favorite(&self, root: &Path) {
        self.scheduler.queue_purge(root.to_path_buf());
    }

    /// メタデータ索引 supervisor の watcher から再照合を要求する軽量 notifier。
    pub fn notifier(&self) -> SimilarIndexNotifier {
        SimilarIndexNotifier {
            scheduler: Arc::downgrade(&self.scheduler),
        }
    }

    pub fn progress(&self) -> IndexProgress {
        self.progress
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// 同じ表示項目と同じメモリ snapshot への照会は Arc ごと再利用する。
    /// 線形走査と SQLite point lookup は worker 上だけで行う。
    pub fn query_item(&self, item_key: &str) -> Arc<ItemQuery> {
        if !self.item_is_enabled(item_key) {
            return Arc::new(ItemQuery::NotIndexed);
        }
        let running = matches!(self.progress(), IndexProgress::Running(_));
        let index = {
            let state = self.memory.lock().unwrap_or_else(|e| e.into_inner());
            match &*state {
                MemoryState::Unloaded => {
                    if running {
                        return Arc::new(ItemQuery::Preparing);
                    }
                    drop(state);
                    start_memory_load(
                        &self.data_dir,
                        &self.memory,
                        &self.memory_epoch,
                        &self.item_query,
                        MemoryLoadTrigger::QueryFallback,
                    );
                    return Arc::new(ItemQuery::Preparing);
                }
                MemoryState::Missing if running => return Arc::new(ItemQuery::Preparing),
                MemoryState::Missing => return Arc::new(ItemQuery::NoIndex),
                MemoryState::Loading => return Arc::new(ItemQuery::Preparing),
                MemoryState::Failed(error) => return Arc::new(ItemQuery::Failed(error.clone())),
                MemoryState::Ready(index) => Arc::clone(index),
            }
        };
        let epoch = self.memory_epoch.load(Ordering::Acquire);
        if let Some(result) = cached_item_query(&self.item_query, item_key, epoch) {
            if running && matches!(result.as_ref(), ItemQuery::NotIndexed) {
                return Arc::new(ItemQuery::Preparing);
            }
            return result;
        }

        let preparing = Arc::new(ItemQuery::Preparing);
        store_cached_item_query(&self.item_query, item_key, epoch, Arc::clone(&preparing));
        let cache = Arc::clone(&self.item_query);
        let epoch_guard = Arc::clone(&self.memory_epoch);
        let enabled_roots = Arc::clone(&self.enabled_roots);
        let db_path = SimilarDb::db_path_at(&self.data_dir);
        let query_key = item_key.to_owned();
        let spawn_result = std::thread::Builder::new()
            .name("similar-item-query".to_owned())
            .spawn(move || {
                let mut result = SimilarDb::open_at(&db_path)
                    .map_err(db_error)
                    .map_or_else(ItemQuery::Failed, |db| {
                        query_item_ready(&db, &index, &query_key)
                    });
                if let ItemQuery::Ready(hits) = &mut result {
                    let roots = enabled_roots.read().unwrap_or_else(|e| e.into_inner());
                    hits.retain(|hit| key_is_under_any(&hit.item_key, &roots));
                }
                if running && matches!(result, ItemQuery::NotIndexed) {
                    result = ItemQuery::Preparing;
                }
                if epoch_guard.load(Ordering::Acquire) == epoch {
                    replace_cached_item_query_if_current(
                        &cache,
                        &query_key,
                        epoch,
                        Arc::new(result),
                    );
                }
            });
        if let Err(error) = spawn_result {
            let failed = Arc::new(ItemQuery::Failed(format!(
                "similar item query worker start failed: {error}"
            )));
            store_cached_item_query(&self.item_query, item_key, epoch, Arc::clone(&failed));
            return failed;
        }
        preparing
    }

    /// 走査中に Complete 済みの旧 snapshot を表示しているかを UI へ伝える。
    pub fn query_results_are_stale(&self) -> bool {
        matches!(self.progress(), IndexProgress::Running(_))
            && matches!(
                &*self.memory.lock().unwrap_or_else(|e| e.into_inner()),
                MemoryState::Ready(_)
            )
    }

    /// お気に入り編集の状態表示用集計。署名本体は読まず、DB I/O は専用 worker で行う。
    pub fn summary(&self) -> IndexSummaryStatus {
        let mut state = self.summary.lock().unwrap_or_else(|e| e.into_inner());
        match &*state {
            SummaryState::Unloaded => {
                *state = SummaryState::Loading;
                let db_path = SimilarDb::db_path_at(&self.data_dir);
                let state = Arc::clone(&self.summary);
                std::thread::spawn(move || {
                    let loaded = if !db_path.is_file() {
                        Ok(None)
                    } else {
                        SimilarDb::open_at(&db_path)
                            .and_then(|db| db.load_index_summary(current_hash_version()))
                    };
                    *state.lock().unwrap_or_else(|e| e.into_inner()) = match loaded {
                        Ok(Some(summary)) => SummaryState::Ready(IndexSummary {
                            completed_at_unix_secs: summary.completed_at_unix_secs,
                            registered_items: summary.registered_items,
                            password_required_pdfs: summary.stats.password_required_pdfs,
                            corrupt_containers: summary.stats.corrupt_containers,
                            zero_page_containers: summary.stats.zero_page_containers,
                            decode_failures: summary.stats.decode_failures,
                            io_failures: summary.stats.io_failures,
                        }),
                        Ok(None) => SummaryState::Missing,
                        Err(error) => SummaryState::Failed(format!(
                            "similar index summary load failed: {error}"
                        )),
                    };
                });
                IndexSummaryStatus::Preparing
            }
            SummaryState::Loading => IndexSummaryStatus::Preparing,
            SummaryState::Missing => IndexSummaryStatus::NoIndex,
            SummaryState::Ready(summary) => IndexSummaryStatus::Ready(*summary),
            SummaryState::Failed(error) => IndexSummaryStatus::Failed(error.clone()),
        }
    }

    pub fn query_book(&self, item_key: &str) -> BookQuery {
        if !self.item_is_enabled(item_key) {
            return BookQuery::NotIndexed;
        }
        let running = matches!(self.progress(), IndexProgress::Running(_));
        let memory = self.memory.lock().unwrap_or_else(|e| e.into_inner());
        match &*memory {
            MemoryState::Unloaded => {
                if running {
                    return BookQuery::Preparing;
                }
                drop(memory);
                start_memory_load(
                    &self.data_dir,
                    &self.memory,
                    &self.memory_epoch,
                    &self.item_query,
                    MemoryLoadTrigger::BookQueryFallback,
                );
                return BookQuery::Preparing;
            }
            MemoryState::Missing if running => return BookQuery::Preparing,
            MemoryState::Missing => return BookQuery::NotIndexed,
            MemoryState::Loading => return BookQuery::Preparing,
            MemoryState::Failed(error) => return BookQuery::Failed(error.clone()),
            MemoryState::Ready(_) => {}
        };
        drop(memory);
        let mut query = self.book_query.lock().unwrap_or_else(|e| e.into_inner());
        match &*query {
            BookQueryState::Loading { item_key: active } if active == item_key => {
                return BookQuery::Preparing;
            }
            BookQueryState::Ready {
                item_key: active,
                result,
            } if active == item_key => return result.clone(),
            _ => {}
        }
        *query = BookQueryState::Loading {
            item_key: item_key.to_owned(),
        };
        let query_state = Arc::clone(&self.book_query);
        let enabled_roots = Arc::clone(&self.enabled_roots);
        let query_key = item_key.to_owned();
        let epoch = self.memory_epoch.load(Ordering::Acquire);
        let epoch_guard = Arc::clone(&self.memory_epoch);
        let db_path = SimilarDb::db_path_at(&self.data_dir);
        std::thread::spawn(move || {
            // 本単位 UI は後続 step。常駐 index を膨らませず、要求された時だけ詳細表を読む。
            let mut result = SimilarDb::open_at(&db_path)
                .and_then(|db| db.load_search_rows(current_hash_version()))
                .map(MemoryIndex::detailed_from_rows)
                .map_or_else(
                    |error| BookQuery::Failed(db_error(error)),
                    |details| query_book_ready(&details, &query_key),
                );
            if let BookQuery::Ready(hits) = &mut result {
                let roots = enabled_roots.read().unwrap_or_else(|e| e.into_inner());
                hits.retain(|hit| key_is_under_any(&hit.other_container_key, &roots));
            }
            if epoch_guard.load(Ordering::Acquire) == epoch {
                let mut state = query_state.lock().unwrap_or_else(|e| e.into_inner());
                if matches!(
                    &*state,
                    BookQueryState::Loading { item_key } if item_key == &query_key
                ) {
                    *state = BookQueryState::Ready {
                        item_key: query_key,
                        result,
                    };
                }
            }
        });
        BookQuery::Preparing
    }

    fn item_is_enabled(&self, item_key: &str) -> bool {
        key_is_under_any(
            item_key,
            &self.enabled_roots.read().unwrap_or_else(|e| e.into_inner()),
        )
    }
}

fn start_memory_load(
    data_dir: &Path,
    memory: &Arc<Mutex<MemoryState>>,
    memory_epoch: &Arc<AtomicU64>,
    item_query: &Arc<Mutex<ItemQueryCache>>,
    trigger: MemoryLoadTrigger,
) {
    let epoch = memory_epoch.load(Ordering::Acquire);
    {
        let mut state = memory.lock().unwrap_or_else(|e| e.into_inner());
        if !matches!(*state, MemoryState::Unloaded) {
            return;
        }
        *state = MemoryState::Loading;
    }
    let db_path = SimilarDb::db_path_at(data_dir);
    let sidecar_path = compact_sidecar_path(data_dir);
    let state = Arc::clone(memory);
    let epoch_guard = Arc::clone(memory_epoch);
    let cache = Arc::clone(item_query);
    let spawn_result = std::thread::Builder::new()
        .name("similar-index-load".to_owned())
        .spawn(move || {
            let started = std::time::Instant::now();
            let loaded = (if !trigger.create_if_missing() && !db_path.is_file() {
                Ok(None)
            } else {
                SimilarDb::open_at(&db_path)
                    .map_err(db_error)
                    .and_then(|db| MemoryIndex::load_cached(&db, &sidecar_path))
                    .map(Some)
            })
            .map(|loaded| {
                loaded.map(|loaded| {
                    crate::logger::log(format!(
                        "similar memory load: trigger={trigger:?} source={:?} rows={} elapsed_ms={:.1}",
                        loaded.source,
                        loaded.index.records.len(),
                        started.elapsed().as_secs_f64() * 1000.0
                    ));
                    Arc::new(loaded.index)
                })
            })
            .map_err(|error| format!("similar index load failed: {error}"));
            let mut state = state.lock().unwrap_or_else(|e| e.into_inner());
            if epoch_guard.load(Ordering::Acquire) != epoch {
                return;
            }
            *state = match loaded {
                Ok(Some(index)) => MemoryState::Ready(index),
                Ok(None) => MemoryState::Missing,
                Err(error) => MemoryState::Failed(error),
            };
            drop(state);
            cache.lock().unwrap_or_else(|e| e.into_inner()).entry = None;
            epoch_guard.fetch_add(1, Ordering::AcqRel);
        });
    if let Err(error) = spawn_result {
        *memory.lock().unwrap_or_else(|e| e.into_inner()) =
            MemoryState::Failed(format!("similar index load worker start failed: {error}"));
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MemoryLoadTrigger {
    Configure,
    IndexRunOpen,
    IndexRunFinished,
    QueryFallback,
    BookQueryFallback,
}

impl MemoryLoadTrigger {
    fn create_if_missing(self) -> bool {
        matches!(
            self,
            Self::Configure | Self::IndexRunOpen | Self::IndexRunFinished
        )
    }
}

impl Drop for SimilarIndexManager {
    fn drop(&mut self) {
        self.scheduler.shutdown();
    }
}

#[derive(Clone)]
struct SchedulerConfig {
    roots: Vec<PathBuf>,
    pdf_passwords: crate::pdf_passwords::PdfPasswordStore,
    activity_gate: Option<Arc<crate::activity_gate::ActivityGate>>,
}

#[derive(Default)]
struct SchedulerState {
    config: Option<SchedulerConfig>,
    pending_purge_roots: BTreeSet<PathBuf>,
    revision: u64,
    worker_running: bool,
    shutdown: bool,
}

struct SimilarIndexScheduler {
    data_dir: PathBuf,
    progress: Arc<Mutex<IndexProgress>>,
    memory: Arc<Mutex<MemoryState>>,
    summary: Arc<Mutex<SummaryState>>,
    memory_epoch: Arc<AtomicU64>,
    item_query: Arc<Mutex<ItemQueryCache>>,
    book_query: Arc<Mutex<BookQueryState>>,
    prefill_db: Arc<Mutex<Option<Arc<SimilarDb>>>>,
    enabled_roots: Arc<RwLock<Vec<String>>>,
    active_cancel: Mutex<Option<Arc<AtomicBool>>>,
    state: Mutex<SchedulerState>,
}

/// 既存の favorite watcher が所有する通知口。ファイル監視は増やさない。
#[derive(Clone)]
pub struct SimilarIndexNotifier {
    scheduler: Weak<SimilarIndexScheduler>,
}

impl SimilarIndexNotifier {
    pub fn request_reconcile(&self) {
        if let Some(scheduler) = self.scheduler.upgrade() {
            scheduler.request_reconcile();
        }
    }
}

impl SimilarIndexScheduler {
    fn configure(
        self: &Arc<Self>,
        mut roots: Vec<PathBuf>,
        pdf_passwords: crate::pdf_passwords::PdfPasswordStore,
        activity_gate: Option<Arc<crate::activity_gate::ActivityGate>>,
    ) {
        roots.sort();
        roots.dedup();
        let normalized = roots
            .iter()
            .map(|root| crate::search_index_db::normalize_path(root))
            .collect::<Vec<_>>();
        *self
            .enabled_roots
            .write()
            .unwrap_or_else(|e| e.into_inner()) = normalized;

        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if state.shutdown {
            return;
        }
        let had_roots = state
            .config
            .as_ref()
            .is_some_and(|config| !config.roots.is_empty());
        let roots_changed = state
            .config
            .as_ref()
            .is_none_or(|config| config.roots != roots);
        let has_roots = !roots.is_empty();
        state.config = Some(SchedulerConfig {
            roots,
            pdf_passwords,
            activity_gate,
        });
        if !roots_changed {
            return;
        }
        self.retain_loaded_snapshot_during_run();
        // 対象変更時だけ現在の旧 snapshot 走査を止める。watcher 通知は coalesce し、
        // 進行中の一巡を完了させてから最新状態をもう一度照合する。
        if let Some(cancel) = self
            .active_cancel
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
        {
            cancel.store(true, Ordering::Relaxed);
        }
        state.revision = state.revision.wrapping_add(1);
        let should_start = !state.worker_running && (had_roots || has_roots);
        if should_start {
            state.worker_running = true;
        }
        drop(state);
        if should_start {
            self.spawn_worker();
        }
    }

    fn request_reconcile(self: &Arc<Self>) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if state.shutdown
            || state
                .config
                .as_ref()
                .is_none_or(|config| config.roots.is_empty())
        {
            return;
        }
        state.revision = state.revision.wrapping_add(1);
        let should_start = !state.worker_running;
        if should_start {
            state.worker_running = true;
        }
        drop(state);
        if should_start {
            self.spawn_worker();
        }
    }

    fn queue_purge(self: &Arc<Self>, root: PathBuf) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if state.shutdown {
            return;
        }
        state.pending_purge_roots.insert(root);
        state.revision = state.revision.wrapping_add(1);
        if let Some(cancel) = self
            .active_cancel
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
        {
            cancel.store(true, Ordering::Relaxed);
        }
        let should_start = !state.worker_running && state.config.is_some();
        if should_start {
            state.worker_running = true;
        }
        drop(state);
        if should_start {
            self.spawn_worker();
        }
    }

    fn spawn_worker(self: &Arc<Self>) {
        let scheduler = Arc::clone(self);
        if let Err(error) = std::thread::Builder::new()
            .name("similar-index".to_owned())
            .spawn(move || scheduler.worker_loop())
        {
            self.state
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .worker_running = false;
            *self.progress.lock().unwrap_or_else(|e| e.into_inner()) =
                IndexProgress::Failed(format!("similar index worker start failed: {error}"));
        }
    }

    fn worker_loop(self: Arc<Self>) {
        let db_path = SimilarDb::db_path_at(&self.data_dir);
        let db = match SimilarDb::open_at(&db_path) {
            Ok(db) => Arc::new(db),
            Err(error) => {
                self.finish_worker(IndexProgress::Failed(format!(
                    "similar.db open failed: {error}"
                )));
                return;
            }
        };
        *self.prefill_db.lock().unwrap_or_else(|e| e.into_inner()) = Some(Arc::clone(&db));
        register_prefill_db(&db, &self.enabled_roots);
        // configure と DB 作成が競合しても、実行中の worker が必ず eager load を開始する。
        // この呼び出しは既に Loading / Ready なら no-op で、パネル照会には依存しない。
        start_memory_load(
            &self.data_dir,
            &self.memory,
            &self.memory_epoch,
            &self.item_query,
            MemoryLoadTrigger::IndexRunOpen,
        );

        loop {
            let (revision, config, cancel, purge_roots) = {
                let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
                if state.shutdown {
                    return;
                }
                let Some(config) = state.config.clone() else {
                    drop(state);
                    self.finish_worker(IndexProgress::Idle);
                    return;
                };
                let cancel = Arc::new(AtomicBool::new(false));
                *self.active_cancel.lock().unwrap_or_else(|e| e.into_inner()) =
                    Some(Arc::clone(&cancel));
                let purge_roots = std::mem::take(&mut state.pending_purge_roots);
                (state.revision, config, cancel, purge_roots)
            };
            *self.progress.lock().unwrap_or_else(|e| e.into_inner()) =
                IndexProgress::Running(RunningProgress {
                    stage: IndexStage::Opening,
                    current_path: None,
                    report: IndexReport::default(),
                });
            let keep_roots = config
                .roots
                .iter()
                .map(|root| crate::search_index_db::normalize_path(root))
                .collect::<Vec<_>>();
            let purge_roots = purge_roots
                .iter()
                .map(|root| crate::search_index_db::normalize_path(root))
                .collect::<Vec<_>>();
            let outcome = db
                .purge_roots_except(&purge_roots, &keep_roots)
                .map_err(|error| format!("disabled favorite purge failed: {error}"))
                .and_then(|purged| {
                    run_index_job(
                        &db,
                        &config.roots,
                        &config.pdf_passwords,
                        config.activity_gate.as_deref(),
                        &cancel,
                        &self.progress,
                    )
                    .map(|mut report| {
                        report.removed = report.removed.saturating_add(purged as u64);
                        report
                    })
                });
            let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            if state.shutdown {
                return;
            }
            let dirty = state.revision != revision;
            if dirty {
                drop(state);
                continue;
            }
            let next = match outcome {
                Ok(report) if cancel.load(Ordering::Relaxed) => IndexProgress::Cancelled(report),
                Ok(report) => IndexProgress::Complete(report),
                Err(error) => IndexProgress::Failed(error),
            };
            state.worker_running = false;
            *self.active_cancel.lock().unwrap_or_else(|e| e.into_inner()) = None;
            drop(state);
            // coalesce された中間 pass では旧 snapshot を保持する。最後の pass が全 in-flight
            // を回収した後だけ捨てるため、更新中の照会が SQLite 全件 reload を繰り返さない。
            self.invalidate_loaded_state();
            *self.progress.lock().unwrap_or_else(|e| e.into_inner()) = next;
            start_memory_load(
                &self.data_dir,
                &self.memory,
                &self.memory_epoch,
                &self.item_query,
                MemoryLoadTrigger::IndexRunFinished,
            );
            return;
        }
    }

    fn retain_loaded_snapshot_during_run(&self) {
        self.memory_epoch.fetch_add(1, Ordering::AcqRel);
        retain_ready_memory_or_unload(&mut self.memory.lock().unwrap_or_else(|e| e.into_inner()));
        *self.book_query.lock().unwrap_or_else(|e| e.into_inner()) = BookQueryState::Idle;
    }

    fn invalidate_loaded_state(&self) {
        *self.memory.lock().unwrap_or_else(|e| e.into_inner()) = MemoryState::Unloaded;
        self.memory_epoch.fetch_add(1, Ordering::AcqRel);
        *self.summary.lock().unwrap_or_else(|e| e.into_inner()) = SummaryState::Unloaded;
        *self.book_query.lock().unwrap_or_else(|e| e.into_inner()) = BookQueryState::Idle;
    }

    fn finish_worker(&self, progress: IndexProgress) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        *self.active_cancel.lock().unwrap_or_else(|e| e.into_inner()) = None;
        state.worker_running = false;
        *self.progress.lock().unwrap_or_else(|e| e.into_inner()) = progress;
    }

    fn shutdown(&self) {
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .shutdown = true;
        if let Some(cancel) = self
            .active_cancel
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
        {
            cancel.store(true, Ordering::Relaxed);
        }
    }
}

fn key_is_under_any(key: &str, roots: &[String]) -> bool {
    crate::similar_db::key_is_under_any(key, roots)
}

enum MemoryState {
    Unloaded,
    Loading,
    Missing,
    Ready(Arc<MemoryIndex>),
    Failed(String),
}

enum SummaryState {
    Unloaded,
    Loading,
    Missing,
    Ready(IndexSummary),
    Failed(String),
}

enum BookQueryState {
    Idle,
    Loading { item_key: String },
    Ready { item_key: String, result: BookQuery },
}

#[derive(Default)]
struct ItemQueryCache {
    entry: Option<CachedItemQuery>,
}

struct CachedItemQuery {
    item_key: String,
    memory_epoch: u64,
    result: Arc<ItemQuery>,
}

fn cached_item_query(
    cache: &Mutex<ItemQueryCache>,
    item_key: &str,
    memory_epoch: u64,
) -> Option<Arc<ItemQuery>> {
    cache
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .entry
        .as_ref()
        .filter(|entry| entry.item_key == item_key && entry.memory_epoch == memory_epoch)
        .map(|entry| Arc::clone(&entry.result))
}

fn store_cached_item_query(
    cache: &Mutex<ItemQueryCache>,
    item_key: &str,
    memory_epoch: u64,
    result: Arc<ItemQuery>,
) {
    cache.lock().unwrap_or_else(|e| e.into_inner()).entry = Some(CachedItemQuery {
        item_key: item_key.to_owned(),
        memory_epoch,
        result,
    });
}

fn replace_cached_item_query_if_current(
    cache: &Mutex<ItemQueryCache>,
    item_key: &str,
    memory_epoch: u64,
    result: Arc<ItemQuery>,
) {
    let mut cache = cache.lock().unwrap_or_else(|e| e.into_inner());
    let Some(entry) = cache.entry.as_mut() else {
        return;
    };
    if entry.item_key == item_key && entry.memory_epoch == memory_epoch {
        entry.result = result;
    }
}

fn retain_ready_memory_or_unload(state: &mut MemoryState) {
    if !matches!(state, MemoryState::Ready(_)) {
        *state = MemoryState::Unloaded;
    }
}

struct MemoryIndex {
    /// 線形照合と origin 候補探索に必要な値だけを保持する。
    /// sidecar 上は padding のない 44-byte/件で、key hash 順に並ぶ。
    records: Vec<CompactIndexRecord>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MemoryLoadSource {
    Sidecar,
    Sqlite,
}

struct LoadedMemoryIndex {
    index: MemoryIndex,
    source: MemoryLoadSource,
}

#[derive(Clone, Copy)]
struct CompactSidecarHeader {
    hash_version: i64,
    proxy_version: u32,
    row_count: u64,
    body_len: u64,
    stamp: SearchContentStamp,
    body_sha256: [u8; 32],
}

struct DetailedMemoryIndex {
    signatures: Vec<([u8; 32], u32)>,
    rows: HashMap<u32, StoredItem>,
    row_for_key: HashMap<String, u32>,
    book_ids: BTreeMap<String, u32>,
}

impl MemoryIndex {
    #[cfg(test)]
    fn load(db: &SimilarDb) -> rusqlite::Result<Self> {
        db.load_compact_search_rows(current_hash_version(), stable_item_key_hash)
            .map(Self::from_compact_rows)
    }

    fn load_cached(db: &SimilarDb, sidecar_path: &Path) -> Result<LoadedMemoryIndex, String> {
        let expected_stamp = db.search_content_stamp().map_err(db_error)?;
        match read_compact_sidecar(sidecar_path, expected_stamp) {
            Ok(index) => {
                // 読んでいる間に writer が公開集合を変えていないことを再確認する。
                // header 一致だけで採用すると、read 中に世代が進んだ sidecar を公開し得る。
                if db.search_content_stamp().map_err(db_error)? == expected_stamp {
                    return Ok(LoadedMemoryIndex {
                        index,
                        source: MemoryLoadSource::Sidecar,
                    });
                }
                crate::logger::log(
                    "similar compact sidecar ignored: database generation changed during read",
                );
            }
            Err(error) => crate::logger::log(format!(
                "similar compact sidecar ignored ({}): {error}",
                sidecar_path.display()
            )),
        }

        // sidecar は派生物なので、欠損・不一致・破損のどれも DB snapshot へ戻す。
        let compact = db
            .load_compact_search_rows(current_hash_version(), stable_item_key_hash)
            .map_err(db_error)?;
        let stamp = compact.stamp;
        let index = Self::from_compact_rows(compact);
        // 書き終わるまで DB が同じ世代ならだけ publish する。失敗しても今回の
        // メモリ snapshot は利用でき、次回も SQLite fallback になるだけである。
        if let Err(error) = write_compact_sidecar_if_current(db, sidecar_path, stamp, &index) {
            crate::logger::log(format!(
                "similar compact sidecar rebuild skipped ({}): {error}",
                sidecar_path.display()
            ));
        }
        Ok(LoadedMemoryIndex {
            index,
            source: MemoryLoadSource::Sqlite,
        })
    }

    fn from_compact_rows(compact: CompactSearchRows) -> Self {
        let mut records = compact.records;
        records.sort_unstable_by_key(|record| (record.key_hash, record.row_id));
        Self { records }
    }

    #[cfg(test)]
    fn from_rows(rows: Vec<SearchRow>) -> Self {
        let mut records = rows
            .into_iter()
            .map(|row| CompactIndexRecord {
                signature: row.item.pdq256,
                row_id: row.row_id,
                key_hash: stable_item_key_hash(&row.item.item_key),
            })
            .collect::<Vec<_>>();
        records.sort_unstable_by_key(|record| (record.key_hash, record.row_id));
        Self { records }
    }

    fn candidate_records_for_key(&self, item_key: &str) -> &[CompactIndexRecord] {
        let hash = stable_item_key_hash(item_key);
        let start = self
            .records
            .partition_point(|record| record.key_hash < hash);
        let end = self
            .records
            .partition_point(|record| record.key_hash <= hash);
        &self.records[start..end]
    }

    fn detailed_from_rows(rows: Vec<SearchRow>) -> DetailedMemoryIndex {
        DetailedMemoryIndex::from_rows(rows)
    }
}

/// プロセスごとに seed が変わる `RandomState` は永続 sidecar に使えない。
/// 形式 version でアルゴリズムを固定した FNV-1a とし、候補取得後は必ず SQLite の
/// `item_key` 実値を比較するため、64-bit collision が別画像へ解決されることはない。
fn stable_item_key_hash(item_key: &str) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for byte in item_key.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

fn compact_sidecar_path(data_dir: &Path) -> PathBuf {
    data_dir.join(COMPACT_SIDECAR_FILE)
}

impl CompactSidecarHeader {
    fn encode(self) -> [u8; COMPACT_SIDECAR_HEADER_LEN] {
        let mut bytes = [0u8; COMPACT_SIDECAR_HEADER_LEN];
        bytes[0..8].copy_from_slice(&COMPACT_SIDECAR_MAGIC);
        bytes[8..12].copy_from_slice(&COMPACT_SIDECAR_FORMAT_VERSION.to_le_bytes());
        bytes[12..16].copy_from_slice(&(COMPACT_SIDECAR_HEADER_LEN as u32).to_le_bytes());
        bytes[16..24].copy_from_slice(&self.hash_version.to_le_bytes());
        bytes[24..28].copy_from_slice(&self.proxy_version.to_le_bytes());
        bytes[28..32].copy_from_slice(&(COMPACT_SIDECAR_RECORD_LEN as u32).to_le_bytes());
        bytes[32..40].copy_from_slice(&self.row_count.to_le_bytes());
        bytes[40..48].copy_from_slice(&self.body_len.to_le_bytes());
        bytes[48..64].copy_from_slice(&self.stamp.store_id);
        bytes[64..72].copy_from_slice(&self.stamp.generation.to_le_bytes());
        bytes[72..104].copy_from_slice(&self.body_sha256);
        bytes
    }

    fn decode(bytes: &[u8; COMPACT_SIDECAR_HEADER_LEN]) -> Result<Self, String> {
        if bytes[0..8] != COMPACT_SIDECAR_MAGIC {
            return Err("compact sidecar magic mismatch".to_owned());
        }
        let format_version = u32::from_le_bytes(bytes[8..12].try_into().unwrap());
        let header_len = u32::from_le_bytes(bytes[12..16].try_into().unwrap());
        let record_len = u32::from_le_bytes(bytes[28..32].try_into().unwrap());
        if format_version != COMPACT_SIDECAR_FORMAT_VERSION
            || header_len != COMPACT_SIDECAR_HEADER_LEN as u32
            || record_len != COMPACT_SIDECAR_RECORD_LEN as u32
        {
            return Err("compact sidecar format mismatch".to_owned());
        }
        let mut store_id = [0u8; 16];
        store_id.copy_from_slice(&bytes[48..64]);
        let mut body_sha256 = [0u8; 32];
        body_sha256.copy_from_slice(&bytes[72..104]);
        Ok(Self {
            hash_version: i64::from_le_bytes(bytes[16..24].try_into().unwrap()),
            proxy_version: u32::from_le_bytes(bytes[24..28].try_into().unwrap()),
            row_count: u64::from_le_bytes(bytes[32..40].try_into().unwrap()),
            body_len: u64::from_le_bytes(bytes[40..48].try_into().unwrap()),
            stamp: SearchContentStamp {
                store_id,
                generation: u64::from_le_bytes(bytes[64..72].try_into().unwrap()),
            },
            body_sha256,
        })
    }
}

fn encode_compact_record(record: CompactIndexRecord) -> [u8; COMPACT_SIDECAR_RECORD_LEN] {
    let mut bytes = [0u8; COMPACT_SIDECAR_RECORD_LEN];
    bytes[0..32].copy_from_slice(&record.signature);
    bytes[32..36].copy_from_slice(&record.row_id.to_le_bytes());
    bytes[36..44].copy_from_slice(&record.key_hash.to_le_bytes());
    bytes
}

fn read_compact_sidecar(
    path: &Path,
    expected_stamp: SearchContentStamp,
) -> Result<MemoryIndex, String> {
    let file = File::open(path).map_err(|error| format!("compact sidecar open failed: {error}"))?;
    let file_len = file
        .metadata()
        .map_err(|error| format!("compact sidecar metadata failed: {error}"))?
        .len();
    let mut reader = BufReader::new(file);
    let mut header_bytes = [0u8; COMPACT_SIDECAR_HEADER_LEN];
    reader
        .read_exact(&mut header_bytes)
        .map_err(|error| format!("compact sidecar header is short: {error}"))?;
    let header = CompactSidecarHeader::decode(&header_bytes)?;
    let expected_body_len = header
        .row_count
        .checked_mul(COMPACT_SIDECAR_RECORD_LEN as u64)
        .ok_or_else(|| "compact sidecar row count overflow".to_owned())?;
    let expected_file_len = (COMPACT_SIDECAR_HEADER_LEN as u64)
        .checked_add(expected_body_len)
        .ok_or_else(|| "compact sidecar file length overflow".to_owned())?;
    if header.hash_version != current_hash_version()
        || header.proxy_version != dupe::PROXY_VERSION
        || header.stamp != expected_stamp
        || header.body_len != expected_body_len
        || file_len != expected_file_len
    {
        return Err("compact sidecar stamp or length mismatch".to_owned());
    }
    let row_count = usize::try_from(header.row_count)
        .map_err(|_| "compact sidecar row count does not fit memory".to_owned())?;
    let mut records = Vec::with_capacity(row_count);
    let mut digest = Sha256::new();
    let mut body_buffer =
        Vec::with_capacity(COMPACT_SIDECAR_IO_RECORDS.saturating_mul(COMPACT_SIDECAR_RECORD_LEN));
    let mut previous_order = None;
    let mut remaining = row_count;
    while remaining > 0 {
        let chunk_records = remaining.min(COMPACT_SIDECAR_IO_RECORDS);
        body_buffer.resize(chunk_records * COMPACT_SIDECAR_RECORD_LEN, 0);
        reader
            .read_exact(&mut body_buffer)
            .map_err(|error| format!("compact sidecar body is short: {error}"))?;
        digest.update(&body_buffer);
        for record_bytes in body_buffer.chunks_exact(COMPACT_SIDECAR_RECORD_LEN) {
            let mut signature = [0u8; 32];
            signature.copy_from_slice(&record_bytes[0..32]);
            let record = CompactIndexRecord {
                signature,
                row_id: u32::from_le_bytes(record_bytes[32..36].try_into().unwrap()),
                key_hash: u64::from_le_bytes(record_bytes[36..44].try_into().unwrap()),
            };
            if record.row_id == 0
                || previous_order
                    .is_some_and(|previous| previous > (record.key_hash, record.row_id))
            {
                return Err("compact sidecar record order is invalid".to_owned());
            }
            previous_order = Some((record.key_hash, record.row_id));
            records.push(record);
        }
        remaining -= chunk_records;
    }
    let actual_digest: [u8; 32] = digest.finalize().into();
    if actual_digest != header.body_sha256 {
        return Err("compact sidecar checksum mismatch".to_owned());
    }
    Ok(MemoryIndex { records })
}

fn write_compact_sidecar_if_current(
    db: &SimilarDb,
    path: &Path,
    stamp: SearchContentStamp,
    index: &MemoryIndex,
) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| "compact sidecar has no parent directory".to_owned())?;
    std::fs::create_dir_all(parent)
        .map_err(|error| format!("compact sidecar directory create failed: {error}"))?;
    let sequence = COMPACT_SIDECAR_TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let temp_path = parent.join(format!(
        ".{COMPACT_SIDECAR_FILE}.{}.{}.tmp",
        std::process::id(),
        sequence
    ));
    let write_result = (|| {
        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp_path)
            .map_err(|error| format!("compact sidecar temp create failed: {error}"))?;
        let mut writer = BufWriter::new(file);
        writer
            .write_all(&[0u8; COMPACT_SIDECAR_HEADER_LEN])
            .map_err(|error| format!("compact sidecar header reserve failed: {error}"))?;
        let mut digest = Sha256::new();
        let mut body_buffer = Vec::with_capacity(
            COMPACT_SIDECAR_IO_RECORDS.saturating_mul(COMPACT_SIDECAR_RECORD_LEN),
        );
        for records in index.records.chunks(COMPACT_SIDECAR_IO_RECORDS) {
            body_buffer.clear();
            for record in records {
                body_buffer.extend_from_slice(&encode_compact_record(*record));
            }
            writer
                .write_all(&body_buffer)
                .map_err(|error| format!("compact sidecar body write failed: {error}"))?;
            digest.update(&body_buffer);
        }
        let row_count = u64::try_from(index.records.len())
            .map_err(|_| "compact sidecar row count does not fit u64".to_owned())?;
        let body_len = row_count
            .checked_mul(COMPACT_SIDECAR_RECORD_LEN as u64)
            .ok_or_else(|| "compact sidecar body length overflow".to_owned())?;
        let header = CompactSidecarHeader {
            hash_version: current_hash_version(),
            proxy_version: dupe::PROXY_VERSION,
            row_count,
            body_len,
            stamp,
            body_sha256: digest.finalize().into(),
        };
        writer
            .seek(SeekFrom::Start(0))
            .and_then(|_| writer.write_all(&header.encode()))
            .and_then(|_| writer.flush())
            .map_err(|error| format!("compact sidecar header publish failed: {error}"))?;
        writer
            .get_ref()
            .sync_all()
            .map_err(|error| format!("compact sidecar sync failed: {error}"))?;

        if db.search_content_stamp().map_err(db_error)? != stamp {
            return Err("compact sidecar source generation changed while writing".to_owned());
        }
        match std::fs::remove_file(path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(format!("compact sidecar old file remove failed: {error}"));
            }
        }
        std::fs::rename(&temp_path, path)
            .map_err(|error| format!("compact sidecar rename failed: {error}"))?;
        Ok(())
    })();
    if write_result.is_err() {
        let _ = std::fs::remove_file(&temp_path);
    }
    write_result
}

impl DetailedMemoryIndex {
    fn from_rows(rows: Vec<SearchRow>) -> Self {
        let mut signatures = Vec::with_capacity(rows.len());
        let mut by_id = HashMap::with_capacity(rows.len());
        let mut row_for_key = HashMap::with_capacity(rows.len());
        let mut container_keys = BTreeSet::new();
        for row in rows {
            signatures.push((row.item.pdq256, row.row_id));
            row_for_key.insert(row.item.item_key.clone(), row.row_id);
            if let Some(container) = &row.item.container_key {
                container_keys.insert(container.clone());
            }
            by_id.insert(row.row_id, row.item);
        }
        let book_ids = container_keys
            .into_iter()
            .enumerate()
            .filter_map(|(index, key)| u32::try_from(index + 1).ok().map(|id| (key, id)))
            .collect();
        Self {
            signatures,
            rows: by_id,
            row_for_key,
            book_ids,
        }
    }
}

fn query_item_ready(db: &SimilarDb, index: &MemoryIndex, item_key: &str) -> ItemQuery {
    query_item_ready_with(index, item_key, |row_id| {
        db.load_item_by_row_id(row_id, current_hash_version())
            .map_err(db_error)
    })
}

fn query_item_ready_with(
    index: &MemoryIndex,
    item_key: &str,
    mut load_row: impl FnMut(u32) -> Result<Option<SearchRow>, String>,
) -> ItemQuery {
    let mut origin_row = None;
    let mut origin_signature = None;
    for record in index.candidate_records_for_key(item_key) {
        match load_row(record.row_id) {
            Ok(Some(row)) if row.item.item_key == item_key => {
                origin_row = Some(row);
                origin_signature = Some(record.signature);
                break;
            }
            Ok(_) => {}
            Err(error) => return ItemQuery::Failed(error),
        }
    }
    let Some(origin_row) = origin_row else {
        return ItemQuery::NotIndexed;
    };
    let origin_id = origin_row.row_id;
    let origin = origin_row.item;
    if origin_signature != Some(origin.pdq256) {
        return ItemQuery::NotIndexed;
    }
    if origin.quality == 0 {
        return ItemQuery::Featureless;
    }
    let mut candidates = Vec::new();
    for record in &index.records {
        if record.row_id == origin_id {
            continue;
        }
        let distance = hamming256(&origin.pdq256, &record.signature);
        let Some(band) = match_band(distance) else {
            continue;
        };
        candidates.push((record.row_id, record.signature, distance, band));
    }
    let mut hits = Vec::with_capacity(candidates.len());
    for (row_id, signature, distance, band) in candidates {
        let item = match load_row(row_id) {
            Ok(Some(row)) if row.item.pdq256 == signature => row.item,
            Ok(_) => continue,
            Err(error) => return ItemQuery::Failed(error),
        };
        let target = resolved_target_for_item(&item);
        hits.push(QueryHit {
            row_id,
            item_key: item.item_key.clone(),
            kind: item.kind,
            container_key: item.container_key.clone(),
            page_index: item.page_index,
            distance,
            band,
            mtime: item.mtime,
            file_size: item.file_size,
            width: item.width,
            height: item.height,
            format: SimilarImageFormat::from_i64(item.format),
            origin_width: origin.width,
            origin_height: origin.height,
            target,
        });
    }
    hits.sort_by(|left, right| {
        left.distance
            .cmp(&right.distance)
            .then_with(|| left.item_key.cmp(&right.item_key))
    });
    ItemQuery::Ready(hits)
}

pub const fn match_band(distance: u32) -> Option<MatchBand> {
    if distance <= NEARLY_IDENTICAL_MAX_DISTANCE {
        Some(MatchBand::NearlyIdentical)
    } else if distance <= OTHER_VERSION_MAX_DISTANCE {
        Some(MatchBand::OtherVersion)
    } else {
        None
    }
}

pub fn target_for_hit(hit: &QueryHit) -> Option<&SimilarItemTarget> {
    hit.target.as_ref()
}

fn resolved_target_for_item(item: &StoredItem) -> Option<SimilarItemTarget> {
    match item.kind {
        ItemKind::Image => Some(SimilarItemTarget::File(recover_existing_path(Path::new(
            &item.item_key,
        )))),
        ItemKind::ZipPage => {
            let (zip_path, entry_name) = item
                .item_key
                .split_once(crate::search_norm::ZIP_ENTRY_SEP)?;
            Some(SimilarItemTarget::ZipPage {
                zip_path: recover_existing_path(Path::new(zip_path)),
                entry_name: entry_name.to_owned(),
            })
        }
        ItemKind::PdfPage => {
            let (pdf_path, page) = item
                .item_key
                .split_once(crate::search_norm::ZIP_ENTRY_SEP)?;
            let page_num = page.strip_prefix("pdf:")?.parse().ok()?;
            Some(SimilarItemTarget::PdfPage {
                pdf_path: recover_existing_path(Path::new(pdf_path)),
                page_num,
            })
        }
    }
}

fn recover_existing_path(path: &Path) -> PathBuf {
    std::fs::canonicalize(path)
        .map(strip_windows_verbatim_prefix)
        .unwrap_or_else(|_| path.to_path_buf())
}

fn strip_windows_verbatim_prefix(path: PathBuf) -> PathBuf {
    #[cfg(windows)]
    {
        let text = path.to_string_lossy();
        if let Some(rest) = text.strip_prefix(r"\\?\UNC\") {
            return PathBuf::from(format!(r"\\{rest}"));
        }
        if let Some(rest) = text.strip_prefix(r"\\?\") {
            return PathBuf::from(rest);
        }
    }
    path
}

fn query_book_ready(index: &DetailedMemoryIndex, item_key: &str) -> BookQuery {
    let Some(origin_id) = index.row_for_key.get(item_key).copied() else {
        return BookQuery::NotIndexed;
    };
    let origin = &index.rows[&origin_id];
    let Some(origin_key) = origin.container_key.as_deref() else {
        return BookQuery::NotBook;
    };
    let Some(&origin_book) = index.book_ids.get(origin_key) else {
        return BookQuery::NotBook;
    };
    let pages_by_book = book_pages(index);
    let Some(origin_pages) = pages_by_book.get(&origin_book) else {
        return BookQuery::NotBook;
    };
    if origin_pages
        .iter()
        .all(|page| page.quality < BOOK_MIN_QUALITY)
    {
        return BookQuery::Featureless;
    }

    // 現在の本の各ページだけを全署名へ線形照合する。全 book pair は作らない。
    let mut candidates = BTreeSet::new();
    for page in origin_pages {
        if page.quality < BOOK_MIN_QUALITY {
            continue;
        }
        let Sig::Bits(bits) = &page.sig else {
            continue;
        };
        let signature: &[u8; 32] = match bits.as_ref().try_into() {
            Ok(signature) => signature,
            Err(_) => continue,
        };
        for (other, row_id) in &index.signatures {
            if hamming256(signature, other) > BOOK_RADIUS {
                continue;
            }
            let Some(container_key) = index.rows[row_id].container_key.as_deref() else {
                continue;
            };
            let Some(&book) = index.book_ids.get(container_key) else {
                continue;
            };
            if book != origin_book {
                candidates.insert(book);
            }
        }
    }

    let params = dupe::book::Params {
        radius: BOOK_RADIUS,
        max_books_per_page: BOOK_MAX_BOOKS_PER_PAGE,
        min_quality: BOOK_MIN_QUALITY,
        coverage_threshold: BOOK_COVERAGE,
        min_matched_pages: BOOK_MIN_MATCHED_PAGES,
    };
    let key_for_book = index
        .book_ids
        .iter()
        .map(|(key, id)| (*id, key.clone()))
        .collect::<HashMap<_, _>>();
    let mut hits = Vec::new();
    for candidate in candidates {
        let mut corpus = [origin_book, candidate]
            .into_iter()
            .filter_map(|book| pages_by_book.get(&book))
            .flatten()
            .cloned()
            .collect::<Vec<_>>();
        let target_len = corpus.len();
        let mut seen_pages = corpus
            .iter()
            .map(|page| (page.book, page.index))
            .collect::<HashSet<_>>();
        // K=8 の共通ページ除外に必要な「実際に近いページ」だけを context に加える。
        // 近傍 book の全ページを展開しないため、全件 book pair sweep にはならない。
        for target_index in 0..target_len {
            let target_page = corpus[target_index].clone();
            add_page_neighborhood(index, &target_page, &mut seen_pages, &mut corpus);
        }
        match dupe::book::classify_pair(&corpus, params, origin_book, candidate) {
            Ok(pair) => hits.push(BookRelationHit {
                other_container_key: key_for_book[&candidate].clone(),
                pair,
            }),
            Err(error) => return BookQuery::Failed(error.to_string()),
        }
    }
    hits.sort_by(|left, right| left.other_container_key.cmp(&right.other_container_key));
    BookQuery::Ready(hits)
}

fn book_pages(index: &DetailedMemoryIndex) -> HashMap<u32, Vec<dupe::book::BookPage>> {
    let mut books = HashMap::<u32, Vec<dupe::book::BookPage>>::new();
    for item in index.rows.values() {
        let (Some(container), Some(page_index)) = (&item.container_key, item.page_index) else {
            continue;
        };
        let Some(&book) = index.book_ids.get(container) else {
            continue;
        };
        books.entry(book).or_default().push(dupe::book::BookPage {
            book,
            index: page_index,
            quality: item.quality,
            sig: Sig::Bits(Box::new(item.pdq256)),
        });
    }
    for pages in books.values_mut() {
        pages.sort_by_key(|page| page.index);
    }
    books
}

fn add_page_neighborhood(
    index: &DetailedMemoryIndex,
    page: &dupe::book::BookPage,
    seen_pages: &mut HashSet<(u32, u32)>,
    corpus: &mut Vec<dupe::book::BookPage>,
) {
    if page.quality < BOOK_MIN_QUALITY {
        return;
    }
    let Sig::Bits(bits) = &page.sig else {
        return;
    };
    let Ok(signature) = <&[u8; 32]>::try_from(bits.as_ref()) else {
        return;
    };
    let mut neighborhood = BTreeSet::from([page.book]);
    for (other, row_id) in &index.signatures {
        if hamming256(signature, other) > BOOK_RADIUS {
            continue;
        }
        let Some(container) = index.rows[row_id].container_key.as_deref() else {
            continue;
        };
        let Some(&book) = index.book_ids.get(container) else {
            continue;
        };
        neighborhood.insert(book);
        if let Some(page_index) = index.rows[row_id].page_index
            && seen_pages.insert((book, page_index))
        {
            corpus.push(dupe::book::BookPage {
                book,
                index: page_index,
                quality: index.rows[row_id].quality,
                sig: Sig::Bits(Box::new(*other)),
            });
        }
        if neighborhood.len() as u32 > BOOK_MAX_BOOKS_PER_PAGE {
            break;
        }
    }
}

fn hamming256(left: &[u8; 32], right: &[u8; 32]) -> u32 {
    left.iter()
        .zip(right)
        .map(|(left, right)| (left ^ right).count_ones())
        .sum()
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum VolumeKey {
    Drive(u8),
    Unc(String, String),
    Other(String),
}

fn volume_key(path: &Path) -> VolumeKey {
    let Some(component) = path.components().next() else {
        return VolumeKey::Other(String::new());
    };
    let Component::Prefix(prefix) = component else {
        return VolumeKey::Other(String::new());
    };
    match prefix.kind() {
        Prefix::Disk(letter) | Prefix::VerbatimDisk(letter) => {
            VolumeKey::Drive(letter.to_ascii_uppercase())
        }
        Prefix::UNC(server, share) | Prefix::VerbatimUNC(server, share) => VolumeKey::Unc(
            server.to_string_lossy().to_lowercase(),
            share.to_string_lossy().to_lowercase(),
        ),
        other => VolumeKey::Other(format!("{other:?}").to_lowercase()),
    }
}

#[derive(Clone, Copy)]
struct ConcurrencyLimits {
    global: usize,
    per_volume: usize,
}

fn current_concurrency_limits(
    activity_gate: Option<&crate::activity_gate::ActivityGate>,
) -> ConcurrencyLimits {
    match activity_gate {
        Some(gate) if gate.is_paused() => ConcurrencyLimits {
            global: 0,
            per_volume: 0,
        },
        Some(gate) if !gate.is_idle() => ConcurrencyLimits {
            global: INDEX_ACTIVE_GLOBAL_OUTSTANDING_LIMIT,
            per_volume: INDEX_ACTIVE_PER_VOLUME_OUTSTANDING_LIMIT,
        },
        _ => ConcurrencyLimits {
            global: INDEX_GLOBAL_OUTSTANDING_LIMIT,
            per_volume: INDEX_PER_VOLUME_OUTSTANDING_LIMIT,
        },
    }
}

struct TaggedWork<T> {
    volume: VolumeKey,
    task: T,
}

impl<T> TaggedWork<T> {
    fn new(volume: VolumeKey, task: T) -> Self {
        Self { volume, task }
    }
}

struct WorkQueueState<T> {
    pending: VecDeque<TaggedWork<T>>,
    in_flight: usize,
    in_flight_by_volume: HashMap<VolumeKey, usize>,
}

struct BoundedWorkQueue<T> {
    state: Mutex<WorkQueueState<T>>,
    changed: Condvar,
}

impl<T> BoundedWorkQueue<T> {
    fn new(initial: Vec<TaggedWork<T>>) -> Self {
        Self {
            state: Mutex::new(WorkQueueState {
                pending: initial.into(),
                in_flight: 0,
                in_flight_by_volume: HashMap::new(),
            }),
            changed: Condvar::new(),
        }
    }

    fn take<'a>(
        &'a self,
        activity_gate: Option<&crate::activity_gate::ActivityGate>,
        cancel: &AtomicBool,
    ) -> Option<WorkLease<'a, T>> {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        loop {
            if cancel.load(Ordering::Relaxed) {
                let discarded_pending = !state.pending.is_empty();
                state.pending.clear();
                if discarded_pending {
                    self.changed.notify_all();
                }
                if state.in_flight == 0 {
                    return None;
                }
            } else {
                let limits = current_concurrency_limits(activity_gate);
                if state.in_flight < limits.global
                    && let Some(index) = state.pending.iter().position(|work| {
                        state
                            .in_flight_by_volume
                            .get(&work.volume)
                            .copied()
                            .unwrap_or(0)
                            < limits.per_volume
                    })
                {
                    let work = state.pending.remove(index).expect("work index disappeared");
                    state.in_flight += 1;
                    *state
                        .in_flight_by_volume
                        .entry(work.volume.clone())
                        .or_default() += 1;
                    return Some(WorkLease {
                        queue: self,
                        volume: work.volume,
                        task: Some(work.task),
                        finished: false,
                    });
                }
                if state.pending.is_empty() && state.in_flight == 0 {
                    return None;
                }
            }
            let (next, _) = self
                .changed
                .wait_timeout(state, INDEX_LIMIT_RECHECK)
                .unwrap_or_else(|e| e.into_inner());
            state = next;
        }
    }

    fn finish(&self, volume: &VolumeKey, children: Vec<TaggedWork<T>>) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        debug_assert!(state.in_flight > 0);
        state.in_flight = state.in_flight.saturating_sub(1);
        let remove_volume = if let Some(count) = state.in_flight_by_volume.get_mut(volume) {
            debug_assert!(*count > 0);
            *count = count.saturating_sub(1);
            *count == 0
        } else {
            false
        };
        if remove_volume {
            state.in_flight_by_volume.remove(volume);
        }
        state.pending.extend(children);
        self.changed.notify_all();
    }
}

struct WorkLease<'a, T> {
    queue: &'a BoundedWorkQueue<T>,
    volume: VolumeKey,
    task: Option<T>,
    finished: bool,
}

impl<T> WorkLease<'_, T> {
    fn take_task(&mut self) -> T {
        self.task.take().expect("work task already taken")
    }

    fn finish(mut self, children: Vec<TaggedWork<T>>) {
        self.queue.finish(&self.volume, children);
        self.finished = true;
    }
}

impl<T> Drop for WorkLease<'_, T> {
    fn drop(&mut self) {
        if !self.finished {
            self.queue.finish(&self.volume, Vec::new());
        }
    }
}

#[derive(Default)]
struct ScanAggregateState {
    report: IndexReport,
    seen_items: HashSet<String>,
    seen_containers: HashSet<String>,
    visited_dirs: HashSet<String>,
    prune_safe: bool,
}

struct ScanAggregate<'a> {
    state: Mutex<ScanAggregateState>,
    progress: &'a Arc<Mutex<IndexProgress>>,
}

impl<'a> ScanAggregate<'a> {
    fn new(progress: &'a Arc<Mutex<IndexProgress>>) -> Self {
        Self {
            state: Mutex::new(ScanAggregateState {
                prune_safe: true,
                ..ScanAggregateState::default()
            }),
            progress,
        }
    }

    fn mark_directory_visited(&self, directory_key: String) -> bool {
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .visited_dirs
            .insert(directory_key)
    }

    fn merge(
        &self,
        report: &mut IndexReport,
        seen_items: &mut HashSet<String>,
        seen_containers: &mut HashSet<String>,
        prune_safe: bool,
    ) {
        let published = {
            let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            state.report.discovered = state.report.discovered.saturating_add(report.discovered);
            let processed_room = state
                .report
                .discovered
                .saturating_sub(state.report.processed);
            debug_assert!(
                report.processed <= processed_room,
                "similar index processed count exceeded discovered count"
            );
            state.report.processed = state
                .report
                .processed
                .saturating_add(report.processed.min(processed_room));
            state.report.indexed = state.report.indexed.saturating_add(report.indexed);
            state.report.unchanged = state.report.unchanged.saturating_add(report.unchanged);
            state.report.removed = state.report.removed.saturating_add(report.removed);
            state.report.containers_completed = state
                .report
                .containers_completed
                .saturating_add(report.containers_completed);
            state.report.password_required_pdfs = state
                .report
                .password_required_pdfs
                .saturating_add(report.password_required_pdfs);
            state.report.corrupt_containers = state
                .report
                .corrupt_containers
                .saturating_add(report.corrupt_containers);
            state.report.zero_page_containers = state
                .report
                .zero_page_containers
                .saturating_add(report.zero_page_containers);
            state.report.decode_failures = state
                .report
                .decode_failures
                .saturating_add(report.decode_failures);
            state.report.io_failures = state.report.io_failures.saturating_add(report.io_failures);
            state.report.errors.append(&mut report.errors);
            state.seen_items.extend(seen_items.drain());
            state.seen_containers.extend(seen_containers.drain());
            state.prune_safe &= prune_safe;
            report.discovered = 0;
            report.processed = 0;
            report.indexed = 0;
            report.unchanged = 0;
            report.removed = 0;
            report.containers_completed = 0;
            report.password_required_pdfs = 0;
            report.corrupt_containers = 0;
            report.zero_page_containers = 0;
            report.decode_failures = 0;
            report.io_failures = 0;
            state.report.clone()
        };
        publish_report(self.progress, &published);
    }

    fn snapshot(&self) -> ScanAggregateState {
        let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        ScanAggregateState {
            report: state.report.clone(),
            seen_items: state.seen_items.clone(),
            seen_containers: state.seen_containers.clone(),
            visited_dirs: HashSet::new(),
            prune_safe: state.prune_safe,
        }
    }
}

enum ScanWork {
    Directory(PathBuf),
    LooseImage {
        candidate: FileCandidate,
        page_index: Option<u32>,
    },
    ImageBook {
        directory: PathBuf,
        container_key: String,
        images: Vec<FileCandidate>,
    },
    Zip(FileCandidate),
    Pdf(FileCandidate),
}

impl ScanWork {
    fn path(&self) -> &Path {
        match self {
            Self::Directory(path) => path,
            Self::LooseImage { candidate, .. } | Self::Zip(candidate) | Self::Pdf(candidate) => {
                &candidate.path
            }
            Self::ImageBook { directory, .. } => directory,
        }
    }

    fn tagged(self) -> TaggedWork<Self> {
        TaggedWork::new(volume_key(self.path()), self)
    }
}

fn run_index_job(
    db: &SimilarDb,
    roots: &[PathBuf],
    pdf_passwords: &crate::pdf_passwords::PdfPasswordStore,
    activity_gate: Option<&crate::activity_gate::ActivityGate>,
    cancel: &Arc<AtomicBool>,
    progress: &Arc<Mutex<IndexProgress>>,
) -> Result<IndexReport, String> {
    db.cleanup_incomplete()
        .map_err(|error| format!("incomplete generation cleanup failed: {error}"))?;
    set_stage(progress, IndexStage::Scanning, None);
    let aggregate = ScanAggregate::new(progress);
    let initial = roots
        .iter()
        .cloned()
        .map(ScanWork::Directory)
        .map(ScanWork::tagged)
        .collect();
    let queue = BoundedWorkQueue::new(initial);
    let worker_result = std::thread::scope(|scope| {
        let mut workers = Vec::with_capacity(INDEX_GLOBAL_OUTSTANDING_LIMIT);
        for _ in 0..INDEX_GLOBAL_OUTSTANDING_LIMIT {
            workers.push(scope.spawn(|| {
                scan_worker_loop(
                    &queue,
                    &aggregate,
                    db,
                    pdf_passwords,
                    activity_gate,
                    cancel,
                    progress,
                )
            }));
        }
        for worker in workers {
            if worker.join().is_err() {
                return Err("similar index scan worker panicked".to_owned());
            }
        }
        Ok(())
    });
    if let Err(error) = worker_result {
        db.cleanup_incomplete()
            .map_err(|cleanup| format!("{error}; generation cleanup failed: {cleanup}"))?;
        return Err(error);
    }
    let mut aggregate = aggregate.snapshot();
    if cancel.load(Ordering::Relaxed) {
        db.cleanup_incomplete()
            .map_err(|error| format!("cancel cleanup failed: {error}"))?;
        return Ok(aggregate.report);
    }
    set_stage(progress, IndexStage::Pruning, None);
    if aggregate.prune_safe {
        aggregate.report.removed =
            db.prune_except_seen(&aggregate.seen_items, &aggregate.seen_containers)
                .map_err(|error| format!("stale row prune failed: {error}"))? as u64;
    }
    publish_report(progress, &aggregate.report);
    let completed_at_unix_secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| i64::try_from(duration.as_secs()).unwrap_or(i64::MAX))
        .unwrap_or(0);
    db.record_completed_index(
        current_hash_version(),
        completed_at_unix_secs,
        CompletedIndexStats {
            password_required_pdfs: aggregate.report.password_required_pdfs,
            corrupt_containers: aggregate.report.corrupt_containers,
            zero_page_containers: aggregate.report.zero_page_containers,
            decode_failures: aggregate.report.decode_failures,
            io_failures: aggregate.report.io_failures,
        },
    )
    .map_err(|error| format!("index summary publish failed: {error}"))?;
    Ok(aggregate.report)
}

struct ScanContext<'a> {
    db: &'a SimilarDb,
    pdf_passwords: &'a crate::pdf_passwords::PdfPasswordStore,
    cancel: &'a Arc<AtomicBool>,
    aggregate: &'a ScanAggregate<'a>,
    report: IndexReport,
    seen_items: HashSet<String>,
    seen_containers: HashSet<String>,
    /// 走査漏れと削除を区別できない I/O failure が 1 件でもあれば prune しない。
    prune_safe: bool,
}

#[derive(Clone)]
struct FileCandidate {
    path: PathBuf,
    mtime: i64,
    file_size: i64,
}

fn scan_worker_loop(
    queue: &BoundedWorkQueue<ScanWork>,
    aggregate: &ScanAggregate<'_>,
    db: &SimilarDb,
    pdf_passwords: &crate::pdf_passwords::PdfPasswordStore,
    activity_gate: Option<&crate::activity_gate::ActivityGate>,
    cancel: &Arc<AtomicBool>,
    progress: &Arc<Mutex<IndexProgress>>,
) {
    while let Some(mut lease) = queue.take(activity_gate, cancel.as_ref()) {
        let work = lease.take_task();
        let path = work.path().to_path_buf();
        set_stage(progress, IndexStage::Scanning, Some(path.clone()));
        let mut context = ScanContext {
            db,
            pdf_passwords,
            cancel,
            aggregate,
            report: IndexReport::default(),
            seen_items: HashSet::new(),
            seen_containers: HashSet::new(),
            prune_safe: true,
        };
        let result = if context.cancelled() {
            Ok(Vec::new())
        } else {
            match work {
                ScanWork::Directory(directory) => context.discover_directory(&directory),
                ScanWork::LooseImage {
                    candidate,
                    page_index,
                } => context
                    .process_loose_image(&candidate, page_index)
                    .map(|()| Vec::new()),
                ScanWork::ImageBook {
                    directory,
                    container_key,
                    images,
                } => context
                    .process_image_book(&directory, &container_key, &images)
                    .map(|()| Vec::new()),
                ScanWork::Zip(candidate) => context.process_zip(&candidate).map(|()| Vec::new()),
                ScanWork::Pdf(candidate) => context.process_pdf(&candidate).map(|()| Vec::new()),
            }
        };
        let children = match result {
            Ok(children) => children,
            Err(error) => {
                context.io_error(&path, error);
                Vec::new()
            }
        };
        context.publish();
        lease.finish(children.into_iter().map(ScanWork::tagged).collect());
    }
}

impl ScanContext<'_> {
    fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }

    fn publish(&mut self) {
        self.aggregate.merge(
            &mut self.report,
            &mut self.seen_items,
            &mut self.seen_containers,
            self.prune_safe,
        );
    }

    fn discover_directory(&mut self, directory: &Path) -> Result<Vec<ScanWork>, String> {
        if self.cancelled() {
            return Ok(Vec::new());
        }
        let directory_key = crate::search_index_db::normalize_path(directory);
        if !self.aggregate.mark_directory_visited(directory_key.clone()) {
            return Ok(Vec::new());
        }
        let entries = std::fs::read_dir(directory)
            .map_err(|error| format!("read_dir {}: {error}", directory.display()))?;
        let mut subdirectories = Vec::new();
        let mut images = Vec::new();
        let mut zips = Vec::new();
        let mut pdfs = Vec::new();
        let mut all_media = Vec::new();
        let mut has_container = false;

        for entry in entries {
            if self.cancelled() {
                return Ok(Vec::new());
            }
            let entry = match entry {
                Ok(entry) => entry,
                Err(error) => {
                    self.prune_safe = false;
                    self.report.io_failures += 1;
                    self.report
                        .errors
                        .push(format!("directory entry {}: {error}", directory.display()));
                    continue;
                }
            };
            if crate::fs_entry::is_internal_app_entry_name(&entry.file_name()) {
                continue;
            }
            let path = entry.path();
            if crate::folder_tree::is_apple_double(&path) {
                continue;
            }
            let file_type = match entry.file_type() {
                Ok(file_type) => file_type,
                Err(error) => {
                    self.io_error(&path, format!("file_type: {error}"));
                    continue;
                }
            };
            if file_type.is_symlink() {
                continue;
            }
            if file_type.is_dir() {
                has_container = true;
                subdirectories.push(path);
                continue;
            }
            if !file_type.is_file() {
                continue;
            }
            let extension = extension_lower(&path);
            let recognized_image = crate::folder_tree::is_recognized_image_ext(&extension);
            let is_zip = crate::folder_tree::is_zip_extension(&extension);
            let is_pdf = crate::folder_tree::is_pdf_extension(&extension);
            let is_convertible = crate::folder_tree::is_convertible_archive_path(&path);
            let media_kind = if recognized_image {
                Some(crate::app::folder_scan::ScanMediaKind::Image)
            } else if crate::folder_tree::SUPPORTED_VIDEO_EXTENSIONS.contains(&extension.as_str()) {
                Some(crate::app::folder_scan::ScanMediaKind::Video)
            } else if crate::folder_tree::is_audio_ext(&extension) {
                Some(crate::app::folder_scan::ScanMediaKind::Audio)
            } else {
                None
            };
            if !(recognized_image || is_zip || is_pdf || media_kind.is_some() || is_convertible) {
                continue;
            }
            let metadata = match entry.metadata() {
                Ok(metadata) => metadata,
                Err(error) => {
                    self.io_error(&path, format!("metadata: {error}"));
                    continue;
                }
            };
            let mtime = crate::ui_helpers::mtime_secs(&metadata);
            let file_size = match i64::try_from(metadata.len()) {
                Ok(size) => size,
                Err(_) => {
                    self.io_error(&path, "file size exceeds SQLite INTEGER".to_owned());
                    continue;
                }
            };
            if let Some(kind) = media_kind {
                all_media.push((path.clone(), kind, mtime, file_size));
            }
            let candidate = FileCandidate {
                path,
                mtime,
                file_size,
            };
            if recognized_image {
                images.push(candidate);
            } else if is_zip {
                has_container = true;
                zips.push(candidate);
            } else if is_pdf {
                has_container = true;
                pdfs.push(candidate);
            } else if is_convertible {
                has_container = true;
            }
        }

        images.sort_by(|left, right| {
            let left = left
                .path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("");
            let right = right
                .path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("");
            crate::filename_sort::compare_file_names(left, right)
        });
        let is_book =
            crate::app::folder_scan::is_image_only_book_contents(has_container, &all_media);
        let mut work = Vec::new();
        if is_book {
            work.push(ScanWork::ImageBook {
                directory: directory.to_path_buf(),
                container_key: directory_key,
                images,
            });
        } else {
            for (page_index, candidate) in images.into_iter().enumerate() {
                work.push(ScanWork::LooseImage {
                    candidate,
                    page_index: u32::try_from(page_index).ok(),
                });
            }
        }
        work.extend(zips.into_iter().map(ScanWork::Zip));
        work.extend(pdfs.into_iter().map(ScanWork::Pdf));
        for child in subdirectories {
            work.push(ScanWork::Directory(child));
        }
        self.publish();
        Ok(work)
    }

    fn process_loose_image(
        &mut self,
        candidate: &FileCandidate,
        page_index: Option<u32>,
    ) -> Result<(), String> {
        self.report.discovered += 1;
        let key = crate::search_index_db::normalize_path(&candidate.path);
        self.seen_items.insert(key.clone());
        let existing = self
            .db
            .load_item(&key, current_hash_version())
            .map_err(db_error)?;
        if existing.as_ref().is_some_and(|existing| {
            existing.item.container_key.is_none()
                && existing.item.page_index == page_index
                && item_metadata_matches(&existing.item, candidate)
        }) {
            self.report.unchanged += 1;
            self.report.processed += 1;
            return Ok(());
        }
        match self.build_file_item(&key, ItemKind::Image, None, page_index, candidate) {
            Ok(item) => {
                self.db.upsert_loose_item(&item).map_err(db_error)?;
                self.report.indexed += 1;
            }
            Err(error) => self.decode_error(&candidate.path, error),
        }
        self.report.processed += 1;
        self.publish();
        Ok(())
    }

    fn process_image_book(
        &mut self,
        directory: &Path,
        container_key: &str,
        images: &[FileCandidate],
    ) -> Result<(), String> {
        self.report.discovered += images.len() as u64;
        self.seen_containers.insert(container_key.to_owned());
        let metadata = std::fs::metadata(directory)
            .map_err(|error| format!("metadata {}: {error}", directory.display()))?;
        let mtime = crate::ui_helpers::mtime_secs(&metadata);
        let file_size = i64::try_from(metadata.len()).map_err(|_| {
            format!(
                "directory size exceeds SQLite INTEGER: {}",
                directory.display()
            )
        })?;
        let page_count = u32::try_from(images.len())
            .map_err(|_| format!("too many pages in {}", directory.display()))?;
        let container_current = self
            .db
            .container_freshness(
                container_key,
                mtime,
                file_size,
                page_count,
                current_hash_version(),
            )
            .map_err(db_error)?
            == Freshness::Current;
        let mut every_page_current = container_current;
        if every_page_current {
            for (index, candidate) in images.iter().enumerate() {
                let key = crate::search_index_db::normalize_path(&candidate.path);
                let existing = self
                    .db
                    .load_item(&key, current_hash_version())
                    .map_err(db_error)?;
                if !existing.as_ref().is_some_and(|existing| {
                    existing.item.container_key.as_deref() == Some(container_key)
                        && existing.item.page_index == Some(index as u32)
                        && item_metadata_matches(&existing.item, candidate)
                }) {
                    every_page_current = false;
                    break;
                }
            }
        }
        if every_page_current {
            let keys = self
                .db
                .item_keys_for_container(container_key)
                .map_err(db_error)?;
            self.report.unchanged += keys.len() as u64;
            self.report.processed += keys.len() as u64;
            self.seen_items.extend(keys);
            return Ok(());
        }
        let generation = self
            .db
            .begin_container_build(
                container_key,
                ContainerKind::ImageFolder,
                page_count,
                mtime,
                file_size,
            )
            .map_err(db_error)?;
        for (index, candidate) in images.iter().enumerate() {
            if self.cancelled() {
                return Ok(());
            }
            let key = crate::search_index_db::normalize_path(&candidate.path);
            self.seen_items.insert(key.clone());
            let item = match self.build_file_item(
                &key,
                ItemKind::Image,
                Some(container_key),
                Some(index as u32),
                candidate,
            ) {
                Ok(item) => item,
                Err(error) => {
                    if self.cancelled() {
                        return Ok(());
                    }
                    self.decode_error(&candidate.path, error);
                    self.db
                        .fail_container(container_key, generation)
                        .map_err(db_error)?;
                    return Ok(());
                }
            };
            self.db.stage_item(generation, &item).map_err(db_error)?;
            self.report.processed += 1;
            self.publish();
        }
        self.db
            .complete_container(container_key, generation)
            .map_err(db_error)?;
        self.report.indexed += images.len() as u64;
        self.report.containers_completed += 1;
        Ok(())
    }

    fn process_zip(&mut self, candidate: &FileCandidate) -> Result<(), String> {
        let container_key = crate::search_index_db::normalize_path(&candidate.path);
        self.seen_containers.insert(container_key.clone());
        let entries = match crate::zip_loader::enumerate_image_entries(&candidate.path) {
            Ok(entries) => entries,
            Err(error) => {
                self.db
                    .record_container_failure(
                        &container_key,
                        ContainerKind::Zip,
                        candidate.mtime,
                        candidate.file_size,
                    )
                    .map_err(db_error)?;
                self.container_error(&candidate.path, error.to_string(), false);
                return Ok(());
            }
        };
        self.report.discovered += entries.len() as u64;
        let page_count = u32::try_from(entries.len())
            .map_err(|_| format!("too many ZIP pages: {}", candidate.path.display()))?;
        if page_count == 0 {
            self.report.zero_page_containers += 1;
            self.report
                .errors
                .push(format!("zero-page ZIP: {}", candidate.path.display()));
            let generation = self
                .db
                .begin_container_build(
                    &container_key,
                    ContainerKind::Zip,
                    0,
                    candidate.mtime,
                    candidate.file_size,
                )
                .map_err(db_error)?;
            self.db
                .fail_container(&container_key, generation)
                .map_err(db_error)?;
            return Ok(());
        }
        if self
            .db
            .container_freshness(
                &container_key,
                candidate.mtime,
                candidate.file_size,
                page_count,
                current_hash_version(),
            )
            .map_err(db_error)?
            == Freshness::Current
        {
            let keys = self
                .db
                .item_keys_for_container(&container_key)
                .map_err(db_error)?;
            self.report.unchanged += keys.len() as u64;
            self.report.processed += keys.len() as u64;
            self.seen_items.extend(keys);
            return Ok(());
        }
        let generation = self
            .db
            .begin_container_build(
                &container_key,
                ContainerKind::Zip,
                page_count,
                candidate.mtime,
                candidate.file_size,
            )
            .map_err(db_error)?;
        for (index, entry) in entries.iter().enumerate() {
            if self.cancelled() {
                return Ok(());
            }
            let key = crate::search_norm::zip_entry_key(&container_key, &entry.entry_name);
            self.seen_items.insert(key.clone());
            let item = match self.build_encoded_item(
                &key,
                ItemKind::ZipPage,
                &container_key,
                index as u32,
                // Thumbnail requests carry the source ZIP's freshness values.
                // Use that same source identity here so their canonical Proxy
                // prefill can actually be consumed by the complete scanner.
                candidate,
                &entry.entry_name,
            ) {
                Ok(item) => item,
                Err(error) => {
                    if self.cancelled() {
                        return Ok(());
                    }
                    self.decode_error(&candidate.path, error);
                    self.db
                        .fail_container(&container_key, generation)
                        .map_err(db_error)?;
                    return Ok(());
                }
            };
            self.db.stage_item(generation, &item).map_err(db_error)?;
            self.report.processed += 1;
            self.publish();
        }
        self.db
            .complete_container(&container_key, generation)
            .map_err(db_error)?;
        self.report.indexed += entries.len() as u64;
        self.report.containers_completed += 1;
        Ok(())
    }

    fn process_pdf(&mut self, candidate: &FileCandidate) -> Result<(), String> {
        let container_key = crate::search_index_db::normalize_path(&candidate.path);
        self.seen_containers.insert(container_key.clone());
        let password = self.pdf_passwords.get(&candidate.path);
        let pages = match crate::pdf_loader::enumerate_pages_with_cancel(
            &candidate.path,
            password.as_deref(),
            Some(Arc::clone(self.cancel)),
        ) {
            Ok(pages) => pages,
            Err(error) => {
                if self.cancelled() {
                    return Ok(());
                }
                self.db
                    .record_container_failure(
                        &container_key,
                        ContainerKind::Pdf,
                        candidate.mtime,
                        candidate.file_size,
                    )
                    .map_err(db_error)?;
                self.container_error(
                    &candidate.path,
                    error.to_string(),
                    pdf_password_required(&error),
                );
                return Ok(());
            }
        };
        self.report.discovered += pages.len() as u64;
        let page_count = u32::try_from(pages.len())
            .map_err(|_| format!("too many PDF pages: {}", candidate.path.display()))?;
        if page_count == 0 {
            self.report.zero_page_containers += 1;
            self.report
                .errors
                .push(format!("zero-page PDF: {}", candidate.path.display()));
            let generation = self
                .db
                .begin_container_build(
                    &container_key,
                    ContainerKind::Pdf,
                    0,
                    candidate.mtime,
                    candidate.file_size,
                )
                .map_err(db_error)?;
            self.db
                .fail_container(&container_key, generation)
                .map_err(db_error)?;
            return Ok(());
        }
        if self
            .db
            .container_freshness(
                &container_key,
                candidate.mtime,
                candidate.file_size,
                page_count,
                current_hash_version(),
            )
            .map_err(db_error)?
            == Freshness::Current
        {
            let keys = self
                .db
                .item_keys_for_container(&container_key)
                .map_err(db_error)?;
            self.report.unchanged += keys.len() as u64;
            self.report.processed += keys.len() as u64;
            self.seen_items.extend(keys);
            return Ok(());
        }
        let generation = self
            .db
            .begin_container_build(
                &container_key,
                ContainerKind::Pdf,
                page_count,
                candidate.mtime,
                candidate.file_size,
            )
            .map_err(db_error)?;
        for page in &pages {
            if self.cancelled() {
                return Ok(());
            }
            let key = pdf_page_key(&container_key, page.page_num);
            self.seen_items.insert(key.clone());
            let item = match self.build_pdf_item(
                &key,
                &container_key,
                candidate,
                page.page_num,
                password.as_deref(),
            ) {
                Ok(item) => item,
                Err(error) => {
                    if self.cancelled() {
                        return Ok(());
                    }
                    self.decode_error(&candidate.path, error);
                    self.db
                        .fail_container(&container_key, generation)
                        .map_err(db_error)?;
                    return Ok(());
                }
            };
            self.db.stage_item(generation, &item).map_err(db_error)?;
            self.report.processed += 1;
            self.publish();
        }
        self.db
            .complete_container(&container_key, generation)
            .map_err(db_error)?;
        self.report.indexed += pages.len() as u64;
        self.report.containers_completed += 1;
        Ok(())
    }

    fn build_file_item(
        &self,
        key: &str,
        kind: ItemKind,
        container_key: Option<&str>,
        page_index: Option<u32>,
        candidate: &FileCandidate,
    ) -> Result<StoredItem, String> {
        if let Some(existing) = self
            .db
            .load_item(key, current_hash_version())
            .map_err(db_error)?
            .filter(|existing| item_metadata_matches(&existing.item, candidate))
        {
            return Ok(with_identity(
                existing.item,
                kind,
                container_key,
                page_index,
            ));
        }
        if let Some(prefill) = self.load_prefill(key, candidate)? {
            return Ok(with_identity(prefill, kind, container_key, page_index));
        }
        let canonical = proxy_from_source(
            ProxySource::File {
                path: &candidate.path,
                verified_bytes: None,
            },
            Some(self.cancel),
        )
        .map_err(|error| error.to_string())?;
        stored_from_proxy(key, kind, container_key, page_index, candidate, canonical)
    }

    fn build_encoded_item(
        &self,
        key: &str,
        kind: ItemKind,
        container_key: &str,
        page_index: u32,
        candidate: &FileCandidate,
        entry_name: &str,
    ) -> Result<StoredItem, String> {
        if let Some(prefill) = self.load_prefill(key, candidate)? {
            return Ok(with_identity(
                prefill,
                kind,
                Some(container_key),
                Some(page_index),
            ));
        }
        let bytes = crate::zip_loader::read_entry_bytes_cancellable(
            &candidate.path,
            entry_name,
            self.cancel,
        )
        .map_err(|error| error.to_string())?;
        let canonical = proxy_from_source(
            ProxySource::Encoded {
                filename_hint: entry_name,
                bytes: &bytes,
            },
            Some(self.cancel),
        )
        .map_err(|error| error.to_string())?;
        stored_from_proxy(
            key,
            kind,
            Some(container_key),
            Some(page_index),
            candidate,
            canonical,
        )
    }

    fn build_pdf_item(
        &self,
        key: &str,
        container_key: &str,
        candidate: &FileCandidate,
        page_num: u32,
        password: Option<&str>,
    ) -> Result<StoredItem, String> {
        if let Some(prefill) = self.load_prefill(key, candidate)? {
            return Ok(with_identity(
                prefill,
                ItemKind::PdfPage,
                Some(container_key),
                Some(page_num),
            ));
        }
        let render = crate::pdf_loader::render_page(
            &candidate.path,
            page_num,
            PDF_RENDER_LONG_EDGE,
            password,
            Some(Arc::clone(self.cancel)),
            crate::pdf_loader::JobPriority::Normal,
            0,
            crate::pdf_loader::CancelWaitPolicy::AbortOnCancel,
        )
        .map_err(|error| error.to_string())?;
        let dims = render.image.dimensions();
        let canonical = proxy_from_source(
            ProxySource::Raster {
                image: &render.image,
                source_dims: dims,
                format: SimilarImageFormat::Pdf,
            },
            Some(self.cancel),
        )
        .map_err(|error| error.to_string())?;
        stored_from_proxy(
            key,
            ItemKind::PdfPage,
            Some(container_key),
            Some(page_num),
            candidate,
            canonical,
        )
    }

    fn load_prefill(
        &self,
        key: &str,
        candidate: &FileCandidate,
    ) -> Result<Option<StoredItem>, String> {
        self.db
            .load_prefill(
                key,
                candidate.mtime,
                candidate.file_size,
                current_hash_version(),
            )
            .map_err(db_error)
    }

    fn decode_error(&mut self, path: &Path, error: String) {
        self.report.decode_failures += 1;
        self.report
            .errors
            .push(format!("decode {}: {error}", path.display()));
    }

    fn container_error(&mut self, path: &Path, error: String, password_required: bool) {
        if password_required {
            self.report.password_required_pdfs += 1;
        } else if !self.cancelled() {
            self.report.corrupt_containers += 1;
        }
        self.report
            .errors
            .push(format!("container {}: {error}", path.display()));
    }

    fn io_error(&mut self, path: &Path, error: String) {
        self.prune_safe = false;
        self.report.io_failures += 1;
        self.report
            .errors
            .push(format!("I/O {}: {error}", path.display()));
        self.publish();
    }
}

fn stored_from_proxy(
    key: &str,
    kind: ItemKind,
    container_key: Option<&str>,
    page_index: Option<u32>,
    candidate: &FileCandidate,
    canonical: crate::similar_image::CanonicalProxy,
) -> Result<StoredItem, String> {
    let signature = dupe::compute(Algo::Pdq256, &canonical.proxy);
    let Sig::Bits(bits) = signature.sig else {
        return Err("PDQ-256 returned a non-bit signature".to_owned());
    };
    let pdq256: [u8; 32] = bits
        .as_ref()
        .try_into()
        .map_err(|_| "PDQ-256 returned a non-256-bit signature".to_owned())?;
    Ok(StoredItem {
        item_key: key.to_owned(),
        kind,
        container_key: container_key.map(str::to_owned),
        page_index,
        mtime: candidate.mtime,
        file_size: candidate.file_size,
        hash_version: current_hash_version(),
        pdq256,
        quality: signature.quality,
        width: canonical.source_dims.0,
        height: canonical.source_dims.1,
        format: canonical.format as i64,
    })
}

fn with_identity(
    mut item: StoredItem,
    kind: ItemKind,
    container_key: Option<&str>,
    page_index: Option<u32>,
) -> StoredItem {
    item.kind = kind;
    item.container_key = container_key.map(str::to_owned);
    item.page_index = page_index;
    item
}

fn item_metadata_matches(item: &StoredItem, candidate: &FileCandidate) -> bool {
    item.hash_version == current_hash_version()
        && item.mtime == candidate.mtime
        && item.file_size == candidate.file_size
}

fn set_stage(
    progress: &Arc<Mutex<IndexProgress>>,
    stage: IndexStage,
    current_path: Option<PathBuf>,
) {
    let mut progress = progress.lock().unwrap_or_else(|e| e.into_inner());
    let report = match &*progress {
        IndexProgress::Running(running) => running.report.clone(),
        _ => IndexReport::default(),
    };
    *progress = IndexProgress::Running(RunningProgress {
        stage,
        current_path,
        report,
    });
}

fn publish_report(progress: &Arc<Mutex<IndexProgress>>, report: &IndexReport) {
    if let IndexProgress::Running(running) =
        &mut *progress.lock().unwrap_or_else(|e| e.into_inner())
    {
        running.report = report.clone();
    }
}

fn db_error(error: rusqlite::Error) -> String {
    format!("similar.db: {error}")
}

fn extension_lower(path: &Path) -> String {
    path.extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
}

fn pdf_password_required(error: &std::io::Error) -> bool {
    error.kind() == std::io::ErrorKind::PermissionDenied
        || error.to_string().to_ascii_lowercase().contains("password")
}

pub fn item_key_for_file(path: &Path) -> String {
    crate::search_index_db::normalize_path(path)
}

pub fn item_key_for_zip_page(zip_path: &Path, entry_name: &str) -> String {
    crate::search_norm::zip_entry_key(
        &crate::search_index_db::normalize_path(zip_path),
        entry_name,
    )
}

pub fn item_key_for_pdf_page(pdf_path: &Path, page_num: u32) -> String {
    pdf_page_key(&crate::search_index_db::normalize_path(pdf_path), page_num)
}

fn pdf_page_key(normalized_pdf_path: &str, page_num: u32) -> String {
    format!(
        "{normalized_pdf_path}{}pdf:{page_num}",
        crate::search_norm::ZIP_ENTRY_SEP
    )
}

struct PrefillRegistration {
    db: Weak<SimilarDb>,
    enabled_roots: Weak<RwLock<Vec<String>>>,
}

static PREFILL_DB: OnceLock<RwLock<Option<PrefillRegistration>>> = OnceLock::new();

fn register_prefill_db(db: &Arc<SimilarDb>, enabled_roots: &Arc<RwLock<Vec<String>>>) {
    *PREFILL_DB
        .get_or_init(|| RwLock::new(None))
        .write()
        .unwrap_or_else(|e| e.into_inner()) = Some(PrefillRegistration {
        db: Arc::downgrade(db),
        enabled_roots: Arc::downgrade(enabled_roots),
    });
}

#[cfg(test)]
pub(crate) fn register_prefill_db_for_test(db: &Arc<SimilarDb>) {
    // decode target 不変条件のテスト用。scope owner を保持しないため prefill 自体は無効。
    register_prefill_db(db, &Arc::new(RwLock::new(Vec::new())));
}

fn prefill_target() -> Option<(Arc<SimilarDb>, Arc<RwLock<Vec<String>>>)> {
    let registration = PREFILL_DB.get()?.read().unwrap_or_else(|e| e.into_inner());
    let registration = registration.as_ref()?;
    let enabled_roots = registration.enabled_roots.upgrade()?;
    Some((registration.db.upgrade()?, enabled_roots))
}

/// 保存済み thumbnail ではなく、元 source の oriented decode buffer を再利用する。
pub(crate) fn offer_thumbnail_raster(
    path: &Path,
    zip_entry: Option<&str>,
    pdf_page: Option<u32>,
    mtime: i64,
    file_size: i64,
    image: &image::DynamicImage,
    source_dims: (u32, u32),
) {
    let Some((db, enabled_roots)) = prefill_target() else {
        return;
    };
    // scope の read lock を put 完了まで保持する。OFF 切替側は write lock の取得後に
    // prune を予約するため、無効化済み root が prefill で後から復活しない。
    let roots = enabled_roots.read().unwrap_or_else(|e| e.into_inner());
    let normalized_path = crate::search_index_db::normalize_path(path);
    if !key_is_under_any(&normalized_path, &roots) {
        return;
    }
    let (item_key, kind, format) = if let Some(entry) = zip_entry {
        (
            item_key_for_zip_page(path, entry),
            ItemKind::ZipPage,
            SimilarImageFormat::from_filename(entry),
        )
    } else if let Some(page) = pdf_page {
        (
            item_key_for_pdf_page(path, page),
            ItemKind::PdfPage,
            SimilarImageFormat::Pdf,
        )
    } else {
        (
            item_key_for_file(path),
            ItemKind::Image,
            SimilarImageFormat::from_filename(path.to_string_lossy().as_ref()),
        )
    };
    if !raster_is_large_enough_for_canonical_proxy(format, image.dimensions(), source_dims) {
        return;
    }
    let canonical = match proxy_from_source(
        ProxySource::Raster {
            image,
            source_dims,
            format,
        },
        None,
    ) {
        Ok(canonical) => canonical,
        Err(error) => {
            crate::logger::log(format!(
                "similar prefill proxy failed for {item_key}: {error}"
            ));
            return;
        }
    };
    let candidate = FileCandidate {
        path: path.to_path_buf(),
        mtime,
        file_size,
    };
    match stored_from_proxy(&item_key, kind, None, pdf_page, &candidate, canonical) {
        Ok(item) => {
            if let Err(error) = db.put_prefill(&item) {
                crate::logger::log(format!(
                    "similar prefill write failed for {item_key}: {error}"
                ));
            }
        }
        Err(error) => {
            crate::logger::log(format!(
                "similar prefill signature failed for {item_key}: {error}"
            ));
        }
    }
}

fn raster_is_large_enough_for_canonical_proxy(
    format: SimilarImageFormat,
    decoded_dims: (u32, u32),
    source_dims: (u32, u32),
) -> bool {
    let decoded_long_edge = decoded_dims.0.max(decoded_dims.1);
    let source_long_edge = source_dims.0.max(source_dims.1);
    let required_long_edge = match format {
        SimilarImageFormat::Jpeg => {
            source_long_edge.min(crate::similar_image::JPEG_DCT_TARGET_EDGE)
        }
        SimilarImageFormat::Pdf => PDF_RENDER_LONG_EDGE,
        _ => source_long_edge,
    };
    decoded_long_edge >= required_long_edge
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    #[cfg(windows)]
    fn current_working_set_bytes() -> usize {
        use std::ffi::c_void;

        #[repr(C)]
        struct ProcessMemoryCounters {
            cb: u32,
            page_fault_count: u32,
            peak_working_set_size: usize,
            working_set_size: usize,
            quota_peak_paged_pool_usage: usize,
            quota_paged_pool_usage: usize,
            quota_peak_non_paged_pool_usage: usize,
            quota_non_paged_pool_usage: usize,
            pagefile_usage: usize,
            peak_pagefile_usage: usize,
        }

        #[link(name = "kernel32")]
        unsafe extern "system" {
            fn GetCurrentProcess() -> *mut c_void;
        }
        #[link(name = "psapi")]
        unsafe extern "system" {
            fn GetProcessMemoryInfo(
                process: *mut c_void,
                counters: *mut ProcessMemoryCounters,
                size: u32,
            ) -> i32;
        }

        let mut counters = ProcessMemoryCounters {
            cb: std::mem::size_of::<ProcessMemoryCounters>() as u32,
            page_fault_count: 0,
            peak_working_set_size: 0,
            working_set_size: 0,
            quota_peak_paged_pool_usage: 0,
            quota_paged_pool_usage: 0,
            quota_peak_non_paged_pool_usage: 0,
            quota_non_paged_pool_usage: 0,
            pagefile_usage: 0,
            peak_pagefile_usage: 0,
        };
        let ok = unsafe {
            GetProcessMemoryInfo(
                GetCurrentProcess(),
                &mut counters,
                std::mem::size_of::<ProcessMemoryCounters>() as u32,
            )
        };
        assert_ne!(ok, 0, "GetProcessMemoryInfo failed");
        counters.working_set_size
    }

    #[cfg(not(windows))]
    fn current_working_set_bytes() -> usize {
        0
    }

    #[test]
    #[ignore = "manual measurement against a caller-selected similar.db"]
    fn measure_real_store_load_and_uncached_query() {
        let db_path = std::env::var_os("MIV_SIMILAR_BENCH_DB")
            .map(PathBuf::from)
            .expect("set MIV_SIMILAR_BENCH_DB");
        let mode = std::env::var("MIV_SIMILAR_BENCH_LOAD").unwrap_or_else(|_| "sqlite".to_owned());
        let sidecar = db_path.with_extension("compact");
        if mode == "rebuild" {
            match std::fs::remove_file(&sidecar) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => panic!("sidecar reset failed: {error}"),
            }
        }
        let resident_before = current_working_set_bytes();
        let load_started = std::time::Instant::now();
        let db = SimilarDb::open_at(&db_path).unwrap();
        let (index, load_source) = match mode.as_str() {
            "sqlite" => (MemoryIndex::load(&db).unwrap(), MemoryLoadSource::Sqlite),
            "rebuild" | "sidecar" => {
                let loaded = MemoryIndex::load_cached(&db, &sidecar).unwrap();
                (loaded.index, loaded.source)
            }
            other => panic!("unknown MIV_SIMILAR_BENCH_LOAD mode: {other}"),
        };
        let row_count = index.records.len();
        let load_ms = load_started.elapsed().as_secs_f64() * 1000.0;
        let resident_after = current_working_set_bytes();
        let Some(origin_key) = std::env::var("MIV_SIMILAR_BENCH_ORIGIN").ok() else {
            eprintln!(
                "similar_load_measurement rows={row_count} mode={mode} source={load_source:?} load_ms={load_ms:.3} resident_before_bytes={resident_before} resident_after_bytes={resident_after} resident_delta_bytes={} sidecar_bytes={}",
                resident_after.saturating_sub(resident_before),
                std::fs::metadata(sidecar).map_or(0, |metadata| metadata.len())
            );
            return;
        };
        let mut query_ms = Vec::new();
        let mut hit_count = 0;
        for _ in 0..3 {
            let started = std::time::Instant::now();
            let result = std::hint::black_box(query_item_ready(&db, &index, &origin_key));
            query_ms.push(started.elapsed().as_secs_f64() * 1000.0);
            hit_count = match result {
                ItemQuery::Ready(hits) => hits.len(),
                other => panic!("unexpected benchmark result: {other:?}"),
            };
        }
        let cache = Mutex::new(ItemQueryCache::default());
        let cache_miss_started = std::time::Instant::now();
        let cached = Arc::new(query_item_ready(&db, &index, &origin_key));
        store_cached_item_query(&cache, &origin_key, 7, Arc::clone(&cached));
        let cache_miss_ms = cache_miss_started.elapsed().as_secs_f64() * 1000.0;
        let mut cache_hit_ms = Vec::new();
        for _ in 0..3 {
            let started = std::time::Instant::now();
            let again = std::hint::black_box(
                cached_item_query(&cache, &origin_key, 7).expect("cache hit missing"),
            );
            cache_hit_ms.push(started.elapsed().as_secs_f64() * 1000.0);
            assert!(Arc::ptr_eq(&cached, &again));
        }
        eprintln!(
            "similar_query_measurement rows={row_count} mode={mode} source={load_source:?} load_ms={load_ms:.3} resident_before_bytes={resident_before} resident_after_bytes={resident_after} resident_delta_bytes={} query_ms={query_ms:?} cache_miss_ms={cache_miss_ms:.3} cache_hit_ms={cache_hit_ms:?} hits={hit_count} origin={origin_key:?} sidecar_bytes={}",
            resident_after.saturating_sub(resident_before),
            std::fs::metadata(sidecar).map_or(0, |metadata| metadata.len())
        );
    }

    #[derive(Default)]
    struct StubConcurrency {
        current_global: usize,
        peak_global: usize,
        current_by_volume: HashMap<VolumeKey, usize>,
        peak_by_volume: HashMap<VolumeKey, usize>,
    }

    #[test]
    fn bounded_stub_source_respects_global_and_per_volume_caps() {
        let drive_e = VolumeKey::Drive(b'E');
        let drive_d = VolumeKey::Drive(b'D');
        let initial = (0..64)
            .map(|index| {
                let volume = if index % 2 == 0 {
                    drive_e.clone()
                } else {
                    drive_d.clone()
                };
                TaggedWork::new(volume, index)
            })
            .collect();
        let queue = BoundedWorkQueue::new(initial);
        let cancel = AtomicBool::new(false);
        let observed = Arc::new((Mutex::new(StubConcurrency::default()), Condvar::new()));
        let release = Arc::new((Mutex::new(false), Condvar::new()));
        let violations = AtomicUsize::new(0);

        std::thread::scope(|scope| {
            let mut workers = Vec::new();
            // 実運用の 16 worker より多い stub worker を競合させ、queue 自体の cap を検証する。
            for _ in 0..32 {
                let queue = &queue;
                let cancel = &cancel;
                let observed = Arc::clone(&observed);
                let release = Arc::clone(&release);
                let violations = &violations;
                workers.push(scope.spawn(move || {
                    while let Some(mut lease) = queue.take(None, cancel) {
                        let _item = lease.take_task();
                        {
                            let (state, changed) = &*observed;
                            let mut state = state.lock().unwrap();
                            state.current_global += 1;
                            state.peak_global = state.peak_global.max(state.current_global);
                            let current = state
                                .current_by_volume
                                .entry(lease.volume.clone())
                                .or_default();
                            *current += 1;
                            let current = *current;
                            state
                                .peak_by_volume
                                .entry(lease.volume.clone())
                                .and_modify(|peak| *peak = (*peak).max(current))
                                .or_insert(current);
                            if state.current_global > INDEX_GLOBAL_OUTSTANDING_LIMIT
                                || current > INDEX_PER_VOLUME_OUTSTANDING_LIMIT
                            {
                                violations.fetch_add(1, Ordering::Relaxed);
                            }
                            changed.notify_all();
                        }
                        let (released, changed) = &*release;
                        let mut released = released.lock().unwrap();
                        while !*released {
                            released = changed.wait(released).unwrap();
                        }
                        drop(released);
                        {
                            let (state, _) = &*observed;
                            let mut state = state.lock().unwrap();
                            state.current_global -= 1;
                            *state
                                .current_by_volume
                                .get_mut(&lease.volume)
                                .expect("stub volume count missing") -= 1;
                        }
                        lease.finish(Vec::new());
                    }
                }));
            }

            let deadline = std::time::Instant::now() + Duration::from_secs(5);
            let (state, changed) = &*observed;
            let mut state = state.lock().unwrap();
            while state.peak_global < INDEX_GLOBAL_OUTSTANDING_LIMIT {
                let now = std::time::Instant::now();
                assert!(now < deadline, "stub workers did not fill the global cap");
                let (next, _) = changed.wait_timeout(state, deadline - now).unwrap();
                state = next;
            }
            assert_eq!(state.peak_global, INDEX_GLOBAL_OUTSTANDING_LIMIT);
            assert_eq!(state.peak_by_volume.get(&drive_e), Some(&8));
            assert_eq!(state.peak_by_volume.get(&drive_d), Some(&8));
            drop(state);
            let (released, changed) = &*release;
            *released.lock().unwrap() = true;
            changed.notify_all();

            for worker in workers {
                worker.join().unwrap();
            }
        });
        assert_eq!(violations.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn cancellation_waits_for_in_flight_stub_work_before_queue_completion() {
        let queue = BoundedWorkQueue::new(vec![
            TaggedWork::new(VolumeKey::Drive(b'E'), 1),
            TaggedWork::new(VolumeKey::Drive(b'E'), 2),
        ]);
        let cancel = AtomicBool::new(false);
        let mut in_flight = queue.take(None, &cancel).expect("first stub work");
        assert_eq!(in_flight.take_task(), 1);
        cancel.store(true, Ordering::Relaxed);
        std::thread::scope(|scope| {
            let (tx, rx) = std::sync::mpsc::channel();
            let queue = &queue;
            let cancel = &cancel;
            let waiter = scope.spawn(move || {
                let next = queue.take(None, cancel);
                tx.send(next.is_none()).unwrap();
            });
            let deadline = std::time::Instant::now() + Duration::from_secs(1);
            let mut state = queue.state.lock().unwrap();
            while !state.pending.is_empty() {
                let now = std::time::Instant::now();
                assert!(
                    now < deadline,
                    "cancelled waiter did not discard pending work"
                );
                let (next, _) = queue.changed.wait_timeout(state, deadline - now).unwrap();
                state = next;
            }
            assert_eq!(state.in_flight, 1);
            drop(state);
            assert!(
                matches!(rx.try_recv(), Err(std::sync::mpsc::TryRecvError::Empty)),
                "cancelled queue must not finish while work is still in flight"
            );
            in_flight.finish(Vec::new());
            assert!(
                rx.recv_timeout(Duration::from_secs(1)).unwrap(),
                "cancelled queue should finish after the last lease is released"
            );
            waiter.join().unwrap();
        });
    }

    #[cfg(windows)]
    #[test]
    fn volume_key_uses_drive_or_unc_server_and_share() {
        assert_eq!(
            volume_key(Path::new(r"e:\library\page.jpg")),
            VolumeKey::Drive(b'E')
        );
        assert_eq!(
            volume_key(Path::new(r"\\Server\Share\book\page.jpg")),
            VolumeKey::Unc("server".to_owned(), "share".to_owned())
        );
    }

    #[test]
    fn activity_gate_reduces_new_work_to_one_overall_and_per_volume() {
        let gate = crate::activity_gate::ActivityGate::new(10_000);
        gate.bump();
        let active = current_concurrency_limits(Some(&gate));
        assert_eq!(active.global, 1);
        assert_eq!(active.per_volume, 1);
        gate.set_paused(true);
        let paused = current_concurrency_limits(Some(&gate));
        assert_eq!(paused.global, 0);
        assert_eq!(paused.per_volume, 0);
    }

    #[test]
    fn concurrent_scan_keeps_container_publish_and_progress_counts_complete() {
        let root = tempfile::tempdir().unwrap();
        for book in ["book-a", "book-b"] {
            let directory = root.path().join(book);
            std::fs::create_dir(&directory).unwrap();
            for page in 0..3 {
                let image = image::DynamicImage::new_rgb8(16 + page, 16 + page);
                image
                    .save(directory.join(format!("{page:02}.png")))
                    .unwrap();
            }
        }
        let db = SimilarDb::open_in_memory().unwrap();
        let cancel = Arc::new(AtomicBool::new(false));
        let progress = Arc::new(Mutex::new(IndexProgress::Idle));
        let passwords = crate::pdf_passwords::PdfPasswordStore::empty_for_test();
        let report = run_index_job(
            &db,
            &[root.path().to_path_buf()],
            &passwords,
            None,
            &cancel,
            &progress,
        )
        .unwrap();
        assert_eq!(report.discovered, 6);
        assert_eq!(report.processed, 6);
        assert_eq!(report.indexed, 6);
        assert_eq!(report.containers_completed, 2);
        assert!(report.processed <= report.discovered);
        let containers = db.load_complete_containers().unwrap();
        assert_eq!(containers.len(), 2);
        assert!(
            containers
                .iter()
                .all(|container| container.page_count == Some(3))
        );
    }

    fn row(id: u32, key: &str, signature: [u8; 32], quality: u8) -> SearchRow {
        SearchRow {
            row_id: id,
            item: StoredItem {
                item_key: key.to_owned(),
                kind: ItemKind::Image,
                container_key: None,
                page_index: None,
                mtime: 1,
                file_size: 2,
                hash_version: current_hash_version(),
                pdq256: signature,
                quality,
                width: 10,
                height: 10,
                format: SimilarImageFormat::Png as i64,
            },
        }
    }

    fn query_test_rows(index: &MemoryIndex, rows: &[SearchRow], item_key: &str) -> ItemQuery {
        query_item_ready_with(index, item_key, |row_id| {
            Ok(rows.iter().find(|row| row.row_id == row_id).cloned())
        })
    }

    #[test]
    fn featureless_origin_is_typed_not_an_empty_result() {
        let rows = vec![row(1, "blank", [0; 32], 0)];
        let index = MemoryIndex::from_rows(rows.clone());
        assert_eq!(
            query_test_rows(&index, &rows, "blank"),
            ItemQuery::Featureless
        );
    }

    #[test]
    fn item_query_cache_changes_only_with_origin_or_memory_epoch() {
        let cache = Mutex::new(ItemQueryCache::default());
        let first = Arc::new(ItemQuery::Ready(Vec::new()));
        store_cached_item_query(&cache, "origin-a", 4, Arc::clone(&first));
        let same = cached_item_query(&cache, "origin-a", 4).unwrap();
        assert!(Arc::ptr_eq(&first, &same));
        assert!(cached_item_query(&cache, "origin-b", 4).is_none());
        assert!(cached_item_query(&cache, "origin-a", 5).is_none());
    }

    #[test]
    fn stale_item_query_worker_cannot_replace_a_newer_origin() {
        let cache = Mutex::new(ItemQueryCache::default());
        store_cached_item_query(
            &cache,
            "origin-b",
            7,
            Arc::new(ItemQuery::Ready(Vec::new())),
        );
        replace_cached_item_query_if_current(
            &cache,
            "origin-a",
            7,
            Arc::new(ItemQuery::Featureless),
        );

        assert!(matches!(
            cached_item_query(&cache, "origin-b", 7).as_deref(),
            Some(ItemQuery::Ready(_))
        ));
        assert!(cached_item_query(&cache, "origin-a", 7).is_none());
    }

    #[test]
    fn active_run_keeps_only_a_complete_loaded_snapshot() {
        let ready = Arc::new(MemoryIndex::from_rows(vec![row(1, "origin", [0; 32], 1)]));
        let mut state = MemoryState::Ready(Arc::clone(&ready));
        retain_ready_memory_or_unload(&mut state);
        let MemoryState::Ready(retained) = state else {
            panic!("complete snapshot was dropped");
        };
        assert!(Arc::ptr_eq(&ready, &retained));

        let mut loading = MemoryState::Loading;
        retain_ready_memory_or_unload(&mut loading);
        assert!(matches!(loading, MemoryState::Unloaded));
    }

    #[test]
    fn stale_indicator_requires_a_running_job_and_a_loaded_snapshot() {
        let manager = SimilarIndexManager::new(PathBuf::from("unused-test-data-dir"));
        *manager.progress.lock().unwrap() = IndexProgress::Running(RunningProgress {
            stage: IndexStage::Scanning,
            current_path: None,
            report: IndexReport::default(),
        });
        assert!(!manager.query_results_are_stale());

        *manager.memory.lock().unwrap() = MemoryState::Ready(Arc::new(MemoryIndex::from_rows(
            vec![row(1, "origin", [0; 32], 1)],
        )));
        assert!(manager.query_results_are_stale());
        *manager.progress.lock().unwrap() = IndexProgress::Idle;
        assert!(!manager.query_results_are_stale());
    }

    #[test]
    fn thumbnail_prefill_accepts_only_an_already_canonical_sized_raster() {
        assert!(!raster_is_large_enough_for_canonical_proxy(
            SimilarImageFormat::Jpeg,
            (750, 500),
            (3000, 2000),
        ));
        assert!(raster_is_large_enough_for_canonical_proxy(
            SimilarImageFormat::Jpeg,
            (2250, 1500),
            (3000, 2000),
        ));
        assert!(raster_is_large_enough_for_canonical_proxy(
            SimilarImageFormat::Jpeg,
            (1000, 700),
            (1000, 700),
        ));
        assert!(!raster_is_large_enough_for_canonical_proxy(
            SimilarImageFormat::Pdf,
            (512, 384),
            (512, 384),
        ));
        assert!(raster_is_large_enough_for_canonical_proxy(
            SimilarImageFormat::Pdf,
            (1024, 768),
            (1024, 768),
        ));
    }

    #[test]
    fn favorite_scope_matches_descendants_but_not_prefix_siblings() {
        let roots = vec!["c:/library/keep".to_owned()];
        assert!(key_is_under_any("c:/library/keep", &roots));
        assert!(key_is_under_any("c:/library/keep/page.jpg", &roots));
        assert!(!key_is_under_any("c:/library/keep-old/page.jpg", &roots));
        assert!(!key_is_under_any("c:/library/other/page.jpg", &roots));
    }

    #[test]
    fn linear_query_uses_the_two_measured_bands() {
        let mut nearly = [0u8; 32];
        nearly[0] = 0xff;
        let mut version = [0u8; 32];
        for byte in version.iter_mut().take(2) {
            *byte = 0xff;
        }
        let unrelated = [0xff; 32];
        let rows = vec![
            row(1, "origin", [0; 32], 1),
            row(2, "near", nearly, 1),
            row(3, "version", version, 1),
            row(4, "far", unrelated, 1),
        ];
        let index = MemoryIndex::from_rows(rows.clone());
        let ItemQuery::Ready(hits) = query_test_rows(&index, &rows, "origin") else {
            panic!("expected ready query");
        };
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].band, MatchBand::NearlyIdentical);
        assert_eq!(hits[0].distance, 8);
        assert_eq!(hits[1].band, MatchBand::OtherVersion);
        assert_eq!(hits[1].distance, 16);
    }

    #[test]
    fn origin_hash_candidate_is_confirmed_against_the_stored_key() {
        let rows = vec![row(1, "actual-key", [0; 32], 1)];
        let mut index = MemoryIndex::from_rows(rows.clone());
        let requested = "different-key-with-the-same-hash-candidate";
        index.records[0].key_hash = stable_item_key_hash(requested);

        assert_eq!(
            query_test_rows(&index, &rows, requested),
            ItemQuery::NotIndexed
        );
    }

    #[test]
    fn item_query_runs_off_thread_and_publishes_the_cached_result() {
        let temp = tempfile::tempdir().unwrap();
        let db_path = SimilarDb::db_path_at(temp.path());
        let db = SimilarDb::open_at(&db_path).unwrap();
        let origin = row(1, "c:/library/origin.png", [0; 32], 1).item;
        let mut near_signature = [0; 32];
        near_signature[0] = 0xff;
        let near = row(2, "c:/library/near.png", near_signature, 1).item;
        db.upsert_loose_item(&origin).unwrap();
        db.upsert_loose_item(&near).unwrap();
        let index = Arc::new(MemoryIndex::load(&db).unwrap());

        let manager = SimilarIndexManager::new(temp.path().to_path_buf());
        *manager.memory.lock().unwrap() = MemoryState::Ready(index);
        *manager.enabled_roots.write().unwrap() = vec!["c:/library".to_owned()];
        assert_eq!(*manager.query_item(&origin.item_key), ItemQuery::Preparing);

        for _ in 0..100 {
            let result = manager.query_item(&origin.item_key);
            if let ItemQuery::Ready(hits) = result.as_ref() {
                assert_eq!(hits.len(), 1);
                assert_eq!(hits[0].item_key, near.item_key);
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!("item query worker did not publish its result");
    }

    #[test]
    fn compact_sidecar_round_trips_and_deletion_is_harmless() {
        let temp = tempfile::tempdir().unwrap();
        let db = SimilarDb::open_at(&SimilarDb::db_path_at(temp.path())).unwrap();
        db.upsert_loose_item(&row(1, "a", [1; 32], 1).item).unwrap();
        db.upsert_loose_item(&row(2, "b", [2; 32], 1).item).unwrap();
        let sidecar = compact_sidecar_path(temp.path());

        let first = MemoryIndex::load_cached(&db, &sidecar).unwrap();
        assert_eq!(first.source, MemoryLoadSource::Sqlite);
        assert!(sidecar.is_file());
        let second = MemoryIndex::load_cached(&db, &sidecar).unwrap();
        assert_eq!(second.source, MemoryLoadSource::Sidecar);
        assert_eq!(first.index.records, second.index.records);

        std::fs::remove_file(&sidecar).unwrap();
        let after_delete = MemoryIndex::load_cached(&db, &sidecar).unwrap();
        assert_eq!(after_delete.source, MemoryLoadSource::Sqlite);
        assert_eq!(after_delete.index.records, first.index.records);
        assert!(sidecar.is_file());
    }

    #[test]
    fn compact_sidecar_header_stamps_every_compatibility_boundary() {
        let temp = tempfile::tempdir().unwrap();
        let db = SimilarDb::open_at(&SimilarDb::db_path_at(temp.path())).unwrap();
        db.upsert_loose_item(&row(1, "a", [1; 32], 1).item).unwrap();
        let sidecar = compact_sidecar_path(temp.path());
        MemoryIndex::load_cached(&db, &sidecar).unwrap();

        let mut file = File::open(&sidecar).unwrap();
        let mut bytes = [0u8; COMPACT_SIDECAR_HEADER_LEN];
        file.read_exact(&mut bytes).unwrap();
        let header = CompactSidecarHeader::decode(&bytes).unwrap();
        assert_eq!(header.hash_version, current_hash_version());
        assert_eq!(header.proxy_version, dupe::PROXY_VERSION);
        assert_eq!(header.row_count, 1);
        assert_eq!(header.body_len, COMPACT_SIDECAR_RECORD_LEN as u64);
        assert_eq!(header.stamp, db.search_content_stamp().unwrap());
        assert_ne!(header.body_sha256, [0; 32]);

        let wrong_generation = SearchContentStamp {
            generation: header.stamp.generation + 1,
            ..header.stamp
        };
        assert!(read_compact_sidecar(&sidecar, wrong_generation).is_err());
    }

    #[test]
    fn compact_sidecar_rejects_short_corrupt_and_stale_content() {
        let temp = tempfile::tempdir().unwrap();
        let db = SimilarDb::open_at(&SimilarDb::db_path_at(temp.path())).unwrap();
        db.upsert_loose_item(&row(1, "a", [1; 32], 1).item).unwrap();
        let sidecar = compact_sidecar_path(temp.path());
        MemoryIndex::load_cached(&db, &sidecar).unwrap();

        OpenOptions::new()
            .write(true)
            .open(&sidecar)
            .unwrap()
            .set_len((COMPACT_SIDECAR_HEADER_LEN + 3) as u64)
            .unwrap();
        let repaired = MemoryIndex::load_cached(&db, &sidecar).unwrap();
        assert_eq!(repaired.source, MemoryLoadSource::Sqlite);
        assert_eq!(repaired.index.records.len(), 1);
        assert_eq!(
            std::fs::metadata(&sidecar).unwrap().len(),
            (COMPACT_SIDECAR_HEADER_LEN + COMPACT_SIDECAR_RECORD_LEN) as u64
        );

        let mut bytes = std::fs::read(&sidecar).unwrap();
        bytes[COMPACT_SIDECAR_HEADER_LEN] ^= 0xff;
        std::fs::write(&sidecar, bytes).unwrap();
        let checksum_repaired = MemoryIndex::load_cached(&db, &sidecar).unwrap();
        assert_eq!(checksum_repaired.source, MemoryLoadSource::Sqlite);

        db.upsert_loose_item(&row(2, "b", [2; 32], 1).item).unwrap();
        let stale_rebuilt = MemoryIndex::load_cached(&db, &sidecar).unwrap();
        assert_eq!(stale_rebuilt.source, MemoryLoadSource::Sqlite);
        assert_eq!(stale_rebuilt.index.records.len(), 2);
        assert_eq!(
            MemoryIndex::load_cached(&db, &sidecar).unwrap().source,
            MemoryLoadSource::Sidecar
        );
    }

    #[test]
    fn compact_sidecar_rejects_a_different_database_with_the_same_generation() {
        let temp = tempfile::tempdir().unwrap();
        let db_path = SimilarDb::db_path_at(temp.path());
        let sidecar = compact_sidecar_path(temp.path());
        {
            let db = SimilarDb::open_at(&db_path).unwrap();
            db.upsert_loose_item(&row(1, "old", [1; 32], 1).item)
                .unwrap();
            MemoryIndex::load_cached(&db, &sidecar).unwrap();
        }
        std::fs::remove_file(&db_path).unwrap();
        for suffix in ["-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{}{}", db_path.display(), suffix));
        }
        let replacement = SimilarDb::open_at(&db_path).unwrap();
        replacement
            .upsert_loose_item(&row(1, "new", [2; 32], 1).item)
            .unwrap();
        let loaded = MemoryIndex::load_cached(&replacement, &sidecar).unwrap();
        assert_eq!(loaded.source, MemoryLoadSource::Sqlite);
        assert_eq!(loaded.index.records.len(), 1);
        assert_eq!(
            query_test_rows(&loaded.index, &[row(1, "new", [2; 32], 1)], "old"),
            ItemQuery::NotIndexed
        );
    }

    #[test]
    fn enabling_a_favorite_starts_memory_load_before_any_panel_query() {
        let temp = tempfile::tempdir().unwrap();
        let library = temp.path().join("library");
        std::fs::create_dir(&library).unwrap();
        let mut favorite = crate::settings::FavoriteEntry::new("library".to_owned(), library);
        favorite.auto_index_similar = true;
        let manager = SimilarIndexManager::new(temp.path().to_path_buf());

        manager.configure(
            &[favorite],
            crate::pdf_passwords::PdfPasswordStore::empty_for_test(),
            None,
        );

        assert!(!matches!(
            *manager.memory.lock().unwrap(),
            MemoryState::Unloaded
        ));
    }

    #[test]
    fn index_worker_open_also_starts_memory_load_without_a_panel_query() {
        let temp = tempfile::tempdir().unwrap();
        let library = temp.path().join("library");
        std::fs::create_dir(&library).unwrap();
        let manager = SimilarIndexManager::new(temp.path().to_path_buf());

        // Manager::configure 側の eager 呼び出しを通さず、scheduler の実行開始だけを使う。
        manager.scheduler.configure(
            vec![library],
            crate::pdf_passwords::PdfPasswordStore::empty_for_test(),
            None,
        );

        for _ in 0..100 {
            if !matches!(*manager.memory.lock().unwrap(), MemoryState::Unloaded) {
                return;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        panic!("index worker DB open did not start the eager memory load");
    }

    #[test]
    fn panel_query_during_a_run_does_not_start_the_eager_load() {
        let manager = SimilarIndexManager::new(PathBuf::from("unused-test-data-dir"));
        *manager.enabled_roots.write().unwrap() = vec!["c:/library".to_owned()];
        *manager.progress.lock().unwrap() = IndexProgress::Running(RunningProgress {
            stage: IndexStage::Scanning,
            current_path: None,
            report: IndexReport::default(),
        });

        assert_eq!(
            manager.query_item("c:/library/page.jpg").as_ref(),
            &ItemQuery::Preparing
        );
        assert!(matches!(
            *manager.memory.lock().unwrap(),
            MemoryState::Unloaded
        ));
    }

    #[test]
    fn missing_store_remains_a_distinct_no_index_result() {
        let temp = tempfile::tempdir().unwrap();
        let manager = SimilarIndexManager::new(temp.path().to_path_buf());
        *manager.enabled_roots.write().unwrap() = vec!["c:/library".to_owned()];
        assert_eq!(
            manager.query_item("c:/library/page.jpg").as_ref(),
            &ItemQuery::Preparing
        );
        for _ in 0..100 {
            if manager.query_item("c:/library/page.jpg").as_ref() == &ItemQuery::NoIndex {
                return;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        panic!("missing store did not publish the typed NoIndex result");
    }

    #[cfg(windows)]
    #[test]
    fn query_target_recovers_real_case_once_and_falls_back_when_missing() {
        let temp = tempfile::tempdir().unwrap();
        let directory = temp.path().join("MixedCaseFolder");
        std::fs::create_dir(&directory).unwrap();
        let actual = directory.join("ActualName.JpG");
        std::fs::write(&actual, b"not decoded by this test").unwrap();
        let normalized = crate::search_index_db::normalize_path(&actual);
        let mut item = row(1, &normalized, [0; 32], 1).item;

        let Some(SimilarItemTarget::File(resolved)) = resolved_target_for_item(&item) else {
            panic!("expected file target");
        };
        assert_eq!(resolved.file_name().unwrap(), "ActualName.JpG");
        assert_eq!(
            resolved.parent().unwrap().file_name().unwrap(),
            "MixedCaseFolder"
        );

        let zip = directory.join("ArchiveBook.ZIP");
        std::fs::write(&zip, b"not opened by this test").unwrap();
        item.kind = ItemKind::ZipPage;
        item.item_key = item_key_for_zip_page(&zip, "Page01.JPG");
        let Some(SimilarItemTarget::ZipPage { zip_path, .. }) = resolved_target_for_item(&item)
        else {
            panic!("expected zip target");
        };
        assert_eq!(zip_path.file_name().unwrap(), "ArchiveBook.ZIP");

        let pdf = directory.join("PrintedBook.PDF");
        std::fs::write(&pdf, b"not opened by this test").unwrap();
        item.kind = ItemKind::PdfPage;
        item.item_key = item_key_for_pdf_page(&pdf, 4);
        let Some(SimilarItemTarget::PdfPage { pdf_path, page_num }) =
            resolved_target_for_item(&item)
        else {
            panic!("expected pdf target");
        };
        assert_eq!(pdf_path.file_name().unwrap(), "PrintedBook.PDF");
        assert_eq!(page_num, 4);

        std::fs::remove_file(&actual).unwrap();
        item.kind = ItemKind::Image;
        item.item_key = normalized.clone();
        assert_eq!(
            resolved_target_for_item(&item),
            Some(SimilarItemTarget::File(PathBuf::from(normalized)))
        );
    }

    #[test]
    fn measured_band_boundaries_are_exact() {
        assert_eq!(match_band(0), Some(MatchBand::NearlyIdentical));
        assert_eq!(match_band(8), Some(MatchBand::NearlyIdentical));
        assert_eq!(match_band(9), Some(MatchBand::OtherVersion));
        assert_eq!(match_band(48), Some(MatchBand::OtherVersion));
        assert_eq!(match_band(49), None);
    }

    #[test]
    fn fullscreen_folder_setting_is_not_part_of_database_freshness() {
        let db = SimilarDb::open_in_memory().unwrap();
        let item = row(1, "a", [1; 32], 1).item;
        db.upsert_loose_item(&item).unwrap();
        let mut settings = crate::settings::Settings::default();
        let before = db.load_search_rows(current_hash_version()).unwrap();
        settings.auto_fullscreen_image_folders = !settings.auto_fullscreen_image_folders;
        let after = db.load_search_rows(current_hash_version()).unwrap();
        assert_eq!(before, after);
    }

    #[test]
    fn differential_reuses_only_an_unchanged_file_signature() {
        let db = SimilarDb::open_in_memory().unwrap();
        let existing = row(1, "missing.png", [7; 32], 10).item;
        db.upsert_loose_item(&existing).unwrap();
        let cancel = Arc::new(AtomicBool::new(false));
        let progress = Arc::new(Mutex::new(IndexProgress::Idle));
        let aggregate = ScanAggregate::new(&progress);
        let passwords = crate::pdf_passwords::PdfPasswordStore::empty_for_test();
        let context = ScanContext {
            db: &db,
            pdf_passwords: &passwords,
            cancel: &cancel,
            aggregate: &aggregate,
            report: IndexReport::default(),
            seen_items: HashSet::new(),
            seen_containers: HashSet::new(),
            prune_safe: true,
        };
        let unchanged = FileCandidate {
            path: PathBuf::from("this-file-does-not-exist.png"),
            mtime: existing.mtime,
            file_size: existing.file_size,
        };
        let reused = context
            .build_file_item(
                &existing.item_key,
                ItemKind::Image,
                Some("book"),
                Some(0),
                &unchanged,
            )
            .unwrap();
        assert_eq!(reused.pdq256, existing.pdq256);

        let changed = FileCandidate {
            file_size: unchanged.file_size + 1,
            ..unchanged
        };
        assert!(
            context
                .build_file_item(
                    &existing.item_key,
                    ItemKind::Image,
                    Some("book"),
                    Some(0),
                    &changed,
                )
                .is_err()
        );
    }

    #[test]
    fn book_query_calls_dupe_book_with_measured_product_params() {
        let mut rows = Vec::new();
        let mut row_id = 1;
        for (container, marker) in [("book-a", 0u8), ("book-b", 0u8)] {
            for page in 0..3 {
                let mut item = row(
                    row_id,
                    &format!("{container}/{page}"),
                    [marker.wrapping_add(page as u8); 32],
                    10,
                );
                item.item.container_key = Some(container.to_owned());
                item.item.page_index = Some(page);
                rows.push(item);
                row_id += 1;
            }
        }
        let index = DetailedMemoryIndex::from_rows(rows);
        let BookQuery::Ready(relations) = query_book_ready(&index, "book-a/0") else {
            panic!("expected a book result");
        };
        assert_eq!(relations.len(), 1);
        assert_eq!(relations[0].pair.relation, dupe::book::Relation::Same);
        assert_eq!(relations[0].pair.matched, BOOK_MIN_MATCHED_PAGES);
    }
}
