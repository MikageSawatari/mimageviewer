//! お気に入り配下の「別バージョン」索引ジョブと遅延ロード線形検索。

use std::collections::{BTreeSet, HashMap, HashSet, VecDeque};
use std::path::{Component, Path, PathBuf, Prefix};
use std::sync::{
    Arc, Condvar, Mutex, OnceLock, RwLock, Weak,
    atomic::{AtomicBool, AtomicU64, Ordering},
};
use std::time::Duration;

use crate::dupe::{self, Algo, Sig};
use crate::similar_db::{
    CandidateIdentity, CompletedIndexStats, ContainerKind, Freshness, ItemKind, SearchRow,
    SimilarDb, StoredItem, current_hash_version,
};
use crate::similar_image::{
    PDF_RENDER_LONG_EDGE, ProxySource, SimilarImageFormat, proxy_from_source,
};
use crate::similar_search_array::{self, SearchRecord, SearchSnapshot};
use image::GenericImageView;

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
    pub item_id: u64,
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

/// ページ帯の 1 コマ。**削除判断の証拠ではなく、移動のための地図** (§9.3)。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BookPageState {
    /// 対応が取れ、距離も「ほぼ同一」の帯に入る。
    Strong,
    /// 対応は取れたが距離が離れている (別解像度・修正あり)。
    Weak,
    /// 相手に対応ページが無い。
    Unmatched,
    /// 採点対象外。featureless か、どの本にもあるページ。**分母からも外れている**ので、
    /// 「一致しなかった」と同じ色で塗ってはいけない。
    Excluded,
}

#[derive(Clone, Debug, PartialEq)]
pub struct BookPageMatch {
    pub state: BookPageState,
    /// 相手の本での対応ページ。押すとそこへ移動する。
    pub other_page_index: Option<u32>,
    /// 移動先。`items` に無い場所も開けるよう、照会側で解決しておく。
    pub other_target: Option<SimilarItemTarget>,
    pub other_item_key: Option<String>,
    /// サムネイルのキャッシュ判定に使う。帯にホバーしたページを出すため。
    pub other_mtime: i64,
    pub other_file_size: i64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct BookRelationHit {
    pub other_container_key: String,
    pub pair: dupe::book::BookPair,
    /// 起点の本のページ順に並んだ帯。長さは起点の本のページ数と一致する。
    pub pages: Vec<BookPageMatch>,
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
            array_update: Mutex::new(ArrayUpdateState::default()),
            compaction_running: AtomicBool::new(false),
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
                Some(Arc::downgrade(&self.scheduler)),
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
                        Some(Arc::downgrade(&self.scheduler)),
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

    /// `container_key` で引く。**ページではなく本で引くこと** — ページごとに引き直すと、
    /// 本を読み進めるあいだ 1 ページごとに数秒の照会が走る。
    pub fn query_book(&self, container_key: &str) -> Arc<BookQuery> {
        let item_key = container_key;
        if !self.item_is_enabled(item_key) {
            return Arc::new(BookQuery::NotIndexed);
        }
        let running = matches!(self.progress(), IndexProgress::Running(_));
        let memory = self.memory.lock().unwrap_or_else(|e| e.into_inner());
        match &*memory {
            MemoryState::Unloaded => {
                if running {
                    return Arc::new(BookQuery::Preparing);
                }
                drop(memory);
                start_memory_load(
                    &self.data_dir,
                    &self.memory,
                    &self.memory_epoch,
                    &self.item_query,
                    MemoryLoadTrigger::BookQueryFallback,
                    Some(Arc::downgrade(&self.scheduler)),
                );
                return Arc::new(BookQuery::Preparing);
            }
            MemoryState::Missing if running => return Arc::new(BookQuery::Preparing),
            MemoryState::Missing => return Arc::new(BookQuery::NotIndexed),
            MemoryState::Loading => return Arc::new(BookQuery::Preparing),
            MemoryState::Failed(error) => return Arc::new(BookQuery::Failed(error.clone())),
            MemoryState::Ready(_) => {}
        };
        let MemoryState::Ready(snapshot) = &*memory else {
            unreachable!("every other memory state returned above");
        };
        let snapshot = Arc::clone(snapshot);
        drop(memory);
        let mut query = self.book_query.lock().unwrap_or_else(|e| e.into_inner());
        match &*query {
            BookQueryState::Loading { item_key: active } if active == item_key => {
                return Arc::new(BookQuery::Preparing);
            }
            BookQueryState::Ready {
                item_key: active,
                result,
            } if active == item_key => return Arc::clone(result),
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
            // 常駐配列で候補を出し、identity は SQLite から引く。照会のために全行を読み直さない。
            let mut result = SimilarDb::open_at(&db_path).map_or_else(
                |error| BookQuery::Failed(db_error(error)),
                |db| query_book_ready(&db, &snapshot, &query_key),
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
                        result: Arc::new(result),
                    };
                }
            }
        });
        Arc::new(BookQuery::Preparing)
    }

    fn item_is_enabled(&self, item_key: &str) -> bool {
        key_is_under_any(
            item_key,
            &self.enabled_roots.read().unwrap_or_else(|e| e.into_inner()),
        )
    }
}

/// 書庫のエントリ順で振られた `page_index` を、閲覧側と同じページ順へ振り直す。
///
/// 本単位の対応付けは順序が揃っていることを前提にする。署名は変わらないので**再デコードは
/// 起きず**、振り直しは保存済みの item_key だけで完結する。一度成功したら記録して二度と
/// 走らない。失敗しても索引そのものは使えるので、記録せずに次の起動へ回す。
fn repair_page_order_if_stale(db: &SimilarDb) {
    let stored = match db.page_order_version() {
        Ok(version) => version,
        Err(error) => {
            crate::logger::log(format!("similar page order version unreadable: {error}"));
            return;
        }
    };
    if stored >= crate::similar_db::PAGE_ORDER_VERSION {
        return;
    }
    let started = std::time::Instant::now();
    match db.renumber_container_pages(crate::similar_db::PAGE_ORDER_VERSION, |left, right| {
        compare_book_pages(left, right)
    }) {
        Ok(containers) => crate::logger::log(format!(
            "similar page order repaired: containers={containers} from_version={stored} elapsed_ms={:.1}",
            started.elapsed().as_secs_f64() * 1000.0
        )),
        Err(error) => crate::logger::log(format!("similar page order repair failed: {error}")),
    }
}

fn start_memory_load(
    data_dir: &Path,
    memory: &Arc<Mutex<MemoryState>>,
    memory_epoch: &Arc<AtomicU64>,
    item_query: &Arc<Mutex<ItemQueryCache>>,
    trigger: MemoryLoadTrigger,
    scheduler: Option<Weak<SimilarIndexScheduler>>,
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
    let base_path = similar_search_array::base_path(data_dir);
    let legacy_sidecar_dir = data_dir.to_path_buf();
    let state = Arc::clone(memory);
    let epoch_guard = Arc::clone(memory_epoch);
    let cache = Arc::clone(item_query);
    let spawn_result = std::thread::Builder::new()
        .name("similar-index-load".to_owned())
        .spawn(move || {
            let started = std::time::Instant::now();
            if let Err(error) = similar_search_array::retire_legacy_sidecar(&legacy_sidecar_dir) {
                crate::logger::log(error);
            }
            let loaded = (if !trigger.create_if_missing() && !db_path.is_file() {
                Ok(None)
            } else {
                SimilarDb::open_at(&db_path)
                    .map_err(db_error)
                    .and_then(|db| similar_search_array::load_or_rebuild(&db, &base_path))
                    .map(Some)
            })
            .map(|loaded| {
                loaded.map(|loaded| {
                    crate::logger::log(format!(
                        "similar memory load: trigger={trigger:?} source={:?} rows={} elapsed_ms={:.1}",
                        loaded.source,
                        loaded.snapshot.record_count(),
                        started.elapsed().as_secs_f64() * 1000.0
                    ));
                    if let Some(reason) = loaded.rejected {
                        crate::logger::log(format!("similar base rebuilt: {reason}"));
                    }
                    Arc::new(loaded.snapshot)
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
            if let Some(scheduler) = scheduler.and_then(|scheduler| scheduler.upgrade()) {
                scheduler.request_array_refresh();
            }
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
    QueryFallback,
    BookQueryFallback,
}

impl MemoryLoadTrigger {
    fn create_if_missing(self) -> bool {
        matches!(self, Self::Configure | Self::IndexRunOpen)
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

#[derive(Default)]
struct ArrayUpdateState {
    requested: bool,
    running: bool,
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
    array_update: Mutex<ArrayUpdateState>,
    compaction_running: AtomicBool,
}

/// 既存の favorite watcher が所有する通知口。ファイル監視は増やさない。
#[derive(Clone)]
pub struct SimilarIndexNotifier {
    scheduler: Weak<SimilarIndexScheduler>,
}

#[derive(Clone)]
struct ArrayRefreshNotifier {
    scheduler: Weak<SimilarIndexScheduler>,
}

impl ArrayRefreshNotifier {
    fn request(&self) {
        if let Some(scheduler) = self.scheduler.upgrade() {
            scheduler.request_array_refresh();
        }
    }
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
        repair_page_order_if_stale(&db);
        // configure と DB 作成が競合しても、実行中の worker が必ず eager load を開始する。
        // この呼び出しは既に Loading / Ready なら no-op で、パネル照会には依存しない。
        start_memory_load(
            &self.data_dir,
            &self.memory,
            &self.memory_epoch,
            &self.item_query,
            MemoryLoadTrigger::IndexRunOpen,
            Some(Arc::downgrade(&self)),
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
                    if purged > 0 {
                        self.request_array_refresh();
                    }
                    let array_refresh = ArrayRefreshNotifier {
                        scheduler: Arc::downgrade(&self),
                    };
                    run_index_job(
                        &db,
                        &config.roots,
                        &config.pdf_passwords,
                        config.activity_gate.as_deref(),
                        &cancel,
                        &self.progress,
                        &array_refresh,
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
            *self.summary.lock().unwrap_or_else(|e| e.into_inner()) = SummaryState::Unloaded;
            *self.book_query.lock().unwrap_or_else(|e| e.into_inner()) = BookQueryState::Idle;
            *self.progress.lock().unwrap_or_else(|e| e.into_inner()) = next;
            self.request_array_refresh();
            return;
        }
    }

    fn retain_loaded_snapshot_during_run(&self) {
        self.memory_epoch.fetch_add(1, Ordering::AcqRel);
        retain_ready_memory_or_unload(&mut self.memory.lock().unwrap_or_else(|e| e.into_inner()));
        *self.book_query.lock().unwrap_or_else(|e| e.into_inner()) = BookQueryState::Idle;
    }

    fn request_array_refresh(self: &Arc<Self>) {
        let should_start = {
            let mut state = self.array_update.lock().unwrap_or_else(|e| e.into_inner());
            state.requested = true;
            if state.running {
                false
            } else {
                state.running = true;
                true
            }
        };
        if should_start {
            let scheduler = Arc::clone(self);
            if let Err(error) = std::thread::Builder::new()
                .name("similar-array-update".to_owned())
                .spawn(move || scheduler.array_update_loop())
            {
                self.array_update
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .running = false;
                crate::logger::log(format!("similar array update worker start failed: {error}"));
            }
        }
    }

    fn array_update_loop(self: Arc<Self>) {
        let db_path = SimilarDb::db_path_at(&self.data_dir);
        let base_path = similar_search_array::base_path(&self.data_dir);
        loop {
            self.array_update
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .requested = false;
            let snapshot = {
                let memory = self.memory.lock().unwrap_or_else(|e| e.into_inner());
                match &*memory {
                    MemoryState::Ready(snapshot) => Some(Arc::clone(snapshot)),
                    _ => None,
                }
            };
            if let Some(snapshot) = snapshot {
                let started = std::time::Instant::now();
                let result = SimilarDb::open_at(&db_path)
                    .map_err(db_error)
                    .and_then(|db| {
                        let batch = db
                            .load_item_changes_after(snapshot.applied_seq)
                            .map_err(db_error)?;
                        match similar_search_array::apply_change_batch(&snapshot, batch) {
                            Ok(Some(next)) => Ok((db, next, false)),
                            Ok(None) => Ok((
                                db,
                                SearchSnapshot {
                                    base: Arc::clone(&snapshot.base),
                                    delta: Arc::clone(&snapshot.delta),
                                    superseded: Arc::clone(&snapshot.superseded),
                                    applied_seq: snapshot.applied_seq,
                                },
                                false,
                            )),
                            Err(_) => similar_search_array::rebuild_from_sqlite(&db, &base_path)
                                .map(|next| (db, next, true)),
                        }
                    });
                match result {
                    Ok((db, next, rebuilt)) => {
                        let changed = next.applied_seq != snapshot.applied_seq || rebuilt;
                        let next = Arc::new(next);
                        let active = if changed {
                            if self.publish_snapshot_if_current(&snapshot, Arc::clone(&next)) {
                                if rebuilt
                                    && let Err(error) = db
                                        .prune_item_changes_through(next.base.applied_seq)
                                        .map_err(db_error)
                                {
                                    crate::logger::log(format!(
                                        "similar array rebuilt history prune failed: {error}"
                                    ));
                                }
                                Some(Arc::clone(&next))
                            } else {
                                self.array_update
                                    .lock()
                                    .unwrap_or_else(|e| e.into_inner())
                                    .requested = true;
                                None
                            }
                        } else {
                            Some(Arc::clone(&snapshot))
                        };
                        if changed && active.is_some() {
                            crate::logger::log(format!(
                                "similar array update: changes={} rebuilt={} elapsed_ms={:.1}",
                                next.applied_seq.saturating_sub(snapshot.applied_seq),
                                rebuilt,
                                started.elapsed().as_secs_f64() * 1000.0
                            ));
                        }
                        if let Some(active) = active
                            && active.should_compact()
                        {
                            self.start_compaction(db, active);
                        }
                    }
                    Err(error) => {
                        crate::logger::log(format!("similar array update failed: {error}"))
                    }
                }
            }
            let mut state = self.array_update.lock().unwrap_or_else(|e| e.into_inner());
            if state.requested {
                continue;
            }
            state.running = false;
            return;
        }
    }

    fn publish_snapshot_if_current(
        &self,
        expected: &Arc<SearchSnapshot>,
        next: Arc<SearchSnapshot>,
    ) -> bool {
        let published = {
            let mut memory = self.memory.lock().unwrap_or_else(|e| e.into_inner());
            match &*memory {
                MemoryState::Ready(current) if Arc::ptr_eq(current, expected) => {
                    *memory = MemoryState::Ready(next);
                    true
                }
                _ => false,
            }
        };
        if published {
            self.memory_epoch.fetch_add(1, Ordering::AcqRel);
            self.item_query
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .entry = None;
            *self.book_query.lock().unwrap_or_else(|e| e.into_inner()) = BookQueryState::Idle;
        }
        published
    }

    fn start_compaction(self: &Arc<Self>, db: SimilarDb, snapshot: Arc<SearchSnapshot>) {
        if self
            .compaction_running
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return;
        }
        let scheduler = Arc::clone(self);
        let base_path = similar_search_array::base_path(&self.data_dir);
        let spawn = std::thread::Builder::new()
            .name("similar-array-compact".to_owned())
            .spawn(move || {
                let started = std::time::Instant::now();
                let base = Arc::new(similar_search_array::compacted_base(&snapshot));
                let result = similar_search_array::write_compacted_base(&base_path, &base);
                match result {
                    Ok(()) => {
                        // DB 更新が同時に進んでいても cutoff より新しい delta を残して公開する。
                        // CAS に負けたら最新 snapshot でもう一度組み直す。公開できるまでは、
                        // 新 base の復旧に必要な item_change を削除しない。
                        let mut published = false;
                        loop {
                            let current = {
                                let memory = scheduler
                                    .memory
                                    .lock()
                                    .unwrap_or_else(|e| e.into_inner());
                                match &*memory {
                                    MemoryState::Ready(current) => Some(Arc::clone(current)),
                                    _ => None,
                                }
                            };
                            let Some(current) = current else { break };
                            let Ok(next) = similar_search_array::snapshot_on_new_base(
                                &current,
                                Arc::clone(&base),
                            ) else {
                                break;
                            };
                            if scheduler.publish_snapshot_if_current(&current, Arc::new(next)) {
                                published = true;
                                break;
                            }
                            std::thread::yield_now();
                        }
                        if published {
                            match db.prune_item_changes_through(base.applied_seq) {
                                Ok(_) => crate::logger::log(format!(
                                    "similar array compaction: rows={} through_seq={} elapsed_ms={:.1}",
                                    snapshot.record_count(),
                                    snapshot.applied_seq,
                                    started.elapsed().as_secs_f64() * 1000.0
                                )),
                                Err(error) => crate::logger::log(format!(
                                    "similar array compaction history prune failed: {}",
                                    db_error(error)
                                )),
                            }
                        } else {
                            crate::logger::log(
                                "similar array compaction deferred publication; history retained",
                            );
                        }
                    }
                    Err(error) => crate::logger::log(format!("similar array compaction failed: {error}")),
                }
                scheduler.compaction_running.store(false, Ordering::Release);
                scheduler.request_array_refresh();
            });
        if let Err(error) = spawn {
            self.compaction_running.store(false, Ordering::Release);
            crate::logger::log(format!(
                "similar array compaction worker start failed: {error}"
            ));
        }
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
    Ready(Arc<SearchSnapshot>),
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
    Loading {
        item_key: String,
    },
    Ready {
        item_key: String,
        result: Arc<BookQuery>,
    },
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

fn query_item_ready(db: &SimilarDb, snapshot: &SearchSnapshot, item_key: &str) -> ItemQuery {
    let origin = match db.load_item(item_key, current_hash_version()) {
        Ok(Some(row)) => row,
        Ok(None) => return ItemQuery::NotIndexed,
        Err(error) => return ItemQuery::Failed(db_error(error)),
    };
    let origin_id = origin.item_id;
    let origin_signature = origin.item.pdq256;
    let mut candidates = Vec::new();
    let mut consider = |record: &SearchRecord| {
        if record.item_id == origin_id || record.quality == 0 {
            return;
        }
        if match_band(hamming256(&origin_signature, &record.signature)).is_some() {
            candidates.push(CandidateIdentity {
                item_id: record.item_id,
                revision: record.revision,
                signature: record.signature,
            });
        }
    };
    for (index, record) in snapshot.base.records.iter().enumerate() {
        if !snapshot.base_record_is_superseded(index) {
            consider(record);
        }
    }
    for entry in snapshot.delta.iter() {
        if let Some(record) = &entry.record {
            consider(record);
        }
    }
    // origin と候補の identity / eligibility / signature / 表示値を一つの read transaction
    // で確定する。上の配列走査は候補提案にしか使わない。
    let verified = match db.verify_item_candidates(item_key, current_hash_version(), &candidates) {
        Ok(Some(verified)) => verified,
        Ok(None) => return ItemQuery::NotIndexed,
        Err(error) => return ItemQuery::Failed(db_error(error)),
    };
    let origin = verified.origin.item;
    if origin.quality == 0 {
        return ItemQuery::Featureless;
    }
    let mut hits = Vec::with_capacity(verified.candidates.len());
    for row in verified.candidates {
        let distance = hamming256(&origin.pdq256, &row.item.pdq256);
        let Some(band) = match_band(distance) else {
            continue;
        };
        let item_id = row.item_id;
        let item = row.item;
        let target = resolved_target_for_item(&item);
        hits.push(QueryHit {
            item_id,
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

fn query_book_ready(db: &SimilarDb, snapshot: &SearchSnapshot, container_key: &str) -> BookQuery {
    let hash_version = current_hash_version();
    let origin_container = container_key.to_owned();
    let origin_pages = match db.load_book_pages(&origin_container, hash_version) {
        Ok(pages) => pages,
        Err(error) => return BookQuery::Failed(db_error(error)),
    };
    if origin_pages.is_empty() {
        return BookQuery::NotBook;
    }
    if origin_pages
        .iter()
        .all(|row| row.item.quality < BOOK_MIN_QUALITY)
    {
        return BookQuery::Featureless;
    }

    let matches = match collect_book_page_matches(db, snapshot, &origin_pages, hash_version) {
        Ok(matches) => matches,
        Err(error) => return BookQuery::Failed(error),
    };

    // 候補の本は、よくあるページ由来の一致からは作らない。表紙や白ページで全ての本が
    // 互いに候補になるのを防ぐ。
    let mut matched_pages_per_book: HashMap<&str, u32> = HashMap::new();
    for page in &matches.pages {
        if page.origin_is_common {
            continue;
        }
        for row in &page.matched {
            if row.container_key != origin_container {
                *matched_pages_per_book
                    .entry(row.container_key.as_str())
                    .or_default() += 1;
            }
        }
    }
    let candidates = matched_pages_per_book
        .into_iter()
        .filter(|(_, count)| *count >= BOOK_MIN_MATCHED_PAGES)
        .map(|(key, _)| key.to_owned())
        .collect::<BTreeSet<_>>();

    let params = dupe::book::Params {
        radius: BOOK_RADIUS,
        max_books_per_page: BOOK_MAX_BOOKS_PER_PAGE,
        min_quality: BOOK_MIN_QUALITY,
        coverage_threshold: BOOK_COVERAGE,
        min_matched_pages: BOOK_MIN_MATCHED_PAGES,
    };

    let candidates = candidates.into_iter().collect::<Vec<_>>();
    let mut candidate_pages = Vec::with_capacity(candidates.len());
    for key in &candidates {
        match db.load_book_pages(key, hash_version) {
            Ok(pages) => candidate_pages.push(pages),
            Err(error) => return BookQuery::Failed(db_error(error)),
        }
    }
    // 相手の本のページも同じように全体へ当てる。これを省くと相手側のよくあるページを
    // 見落とし、分母が大きくなって被覆率が過小に出る。候補ごとに走査すると配列を
    // 候補の数だけ読み直すことになるので、ここで 1 回にまとめる。
    let borrowed = candidate_pages
        .iter()
        .map(|pages| pages.as_slice())
        .collect::<Vec<_>>();
    let candidate_matches =
        match collect_book_page_matches_for(db, snapshot, &borrowed, hash_version) {
            Ok(matches) => matches,
            Err(error) => return BookQuery::Failed(error),
        };

    let mut hits = Vec::new();
    for ((candidate_key, pages), candidate_matches) in candidates
        .into_iter()
        .zip(candidate_pages.iter())
        .zip(candidate_matches.iter())
    {
        if pages.is_empty() {
            continue;
        }
        let mut corpus = BookCorpusBuilder::new(&origin_container, &candidate_key);
        corpus.add_book(BOOK_ORIGIN, &origin_pages);
        corpus.add_book(BOOK_CANDIDATE, pages);
        corpus.add_context(&matches);
        corpus.add_context(candidate_matches);

        match dupe::book::classify_pair(&corpus.pages, params, BOOK_ORIGIN, BOOK_CANDIDATE) {
            Ok(pair) => {
                let strip = build_page_strip(&origin_pages, pages, &matches, &pair);
                hits.push(BookRelationHit {
                    other_container_key: candidate_key,
                    pair,
                    pages: strip,
                })
            }
            Err(error) => return BookQuery::Failed(error.to_string()),
        }
    }
    hits.sort_by(|left, right| left.other_container_key.cmp(&right.other_container_key));
    BookQuery::Ready(hits)
}

const BOOK_ORIGIN: u32 = 1;
const BOOK_CANDIDATE: u32 = 2;

/// `dupe::book` に渡す corpus を組む。
///
/// 「よくあるページ」の判定は corpus の中だけで行われるため、比べる 2 冊のページに加えて
/// 「その周辺に何冊いるか」を数えられるだけの文脈ページを入れる必要がある。冊数が上限を
/// 超えたと分かればよいので、近傍の全ページは展開しない。
struct BookCorpusBuilder<'a> {
    origin_key: &'a str,
    candidate_key: &'a str,
    pages: Vec<dupe::book::BookPage>,
    seen: HashSet<(u32, u32)>,
    context_books: HashMap<String, u32>,
    next_context_book: u32,
}

impl<'a> BookCorpusBuilder<'a> {
    fn new(origin_key: &'a str, candidate_key: &'a str) -> Self {
        Self {
            origin_key,
            candidate_key,
            pages: Vec::new(),
            seen: HashSet::new(),
            context_books: HashMap::new(),
            next_context_book: BOOK_CANDIDATE + 1,
        }
    }

    fn add_book(&mut self, book: u32, rows: &[crate::similar_db::SearchRow]) {
        for row in rows {
            let Some(page_index) = row.item.page_index else {
                continue;
            };
            if self.seen.insert((book, page_index)) {
                self.pages.push(dupe::book::BookPage {
                    book,
                    index: page_index,
                    quality: row.item.quality,
                    sig: Sig::Bits(Box::new(row.item.pdq256)),
                });
            }
        }
    }

    fn add_context(&mut self, matches: &BookMatchSet) {
        for page in &matches.pages {
            for row in &page.matched {
                if row.container_key == self.origin_key || row.container_key == self.candidate_key {
                    continue;
                }
                let book = match self.context_books.get(&row.container_key) {
                    Some(book) => *book,
                    None => {
                        let book = self.next_context_book;
                        self.next_context_book += 1;
                        self.context_books.insert(row.container_key.clone(), book);
                        book
                    }
                };
                if self.seen.insert((book, row.page_index)) {
                    self.pages.push(dupe::book::BookPage {
                        book,
                        index: row.page_index,
                        quality: row.quality,
                        sig: Sig::Bits(Box::new(row.signature)),
                    });
                }
            }
        }
    }
}

/// 起点の本のページ帯を組む。
///
/// 対応が取れたページは `alignment` に (起点ページ, 相手ページ) として並ぶ。距離は
/// `alignment` に載っていないので、両方の署名から測り直して「ほぼ同一」と「別バージョン」を
/// 分ける。単体画像の帯 (§9.5) と同じ切り方にして、2 か所で違う基準を持たない。
fn build_page_strip(
    origin_pages: &[crate::similar_db::SearchRow],
    candidate_pages: &[crate::similar_db::SearchRow],
    origin_matches: &BookMatchSet,
    pair: &dupe::book::BookPair,
) -> Vec<BookPageMatch> {
    let by_origin_page = origin_pages
        .iter()
        .enumerate()
        .filter_map(|(slot, row)| row.item.page_index.map(|page| (page, slot)))
        .collect::<HashMap<_, _>>();
    let candidate_by_page = candidate_pages
        .iter()
        .filter_map(|row| row.item.page_index.map(|page| (page, row)))
        .collect::<HashMap<_, _>>();

    let mut strip = origin_pages
        .iter()
        .enumerate()
        .map(|(slot, row)| {
            let excluded = row.item.quality < BOOK_MIN_QUALITY
                || origin_matches
                    .pages
                    .get(slot)
                    .is_some_and(|page| page.origin_is_common);
            BookPageMatch {
                state: if excluded {
                    BookPageState::Excluded
                } else {
                    BookPageState::Unmatched
                },
                other_page_index: None,
                other_target: None,
                other_item_key: None,
                other_mtime: 0,
                other_file_size: 0,
            }
        })
        .collect::<Vec<_>>();

    // 同じ本のページはコンテナが同じなので、移動先の解決は本の中で 1 度で足りる。
    let mut resolved_targets: HashMap<u64, Option<SimilarItemTarget>> = HashMap::new();
    for &(origin_page, other_page) in &pair.alignment {
        let Some(&slot) = by_origin_page.get(&origin_page) else {
            continue;
        };
        let Some(other) = candidate_by_page.get(&other_page) else {
            continue;
        };
        let distance = hamming256(&origin_pages[slot].item.pdq256, &other.item.pdq256);
        let target = resolved_targets
            .entry(other.item_id)
            .or_insert_with(|| resolved_target_for_item(&other.item))
            .clone();
        strip[slot] = BookPageMatch {
            state: match match_band(distance) {
                Some(MatchBand::NearlyIdentical) => BookPageState::Strong,
                _ => BookPageState::Weak,
            },
            other_page_index: Some(other_page),
            other_target: target,
            other_item_key: Some(other.item.item_key.clone()),
            other_mtime: other.item.mtime,
            other_file_size: other.item.file_size,
        };
    }
    strip
}

/// 1 ページぶんの一致。`origin_is_common` は「相手の本が多すぎて候補作りに使えない」印。
struct BookPageMatches {
    origin_is_common: bool,
    matched: Vec<ResolvedPage>,
}

struct ResolvedPage {
    container_key: String,
    page_index: u32,
    quality: u8,
    signature: [u8; 32],
}

struct BookMatchSet {
    pages: Vec<BookPageMatches>,
}

/// 1 ページが半径内に持てる一致の上限。
///
/// これを超えるページは、どの数え方でも distinctive ではない。上限に達したページは
/// 候補作りから外すが、集めた分は文脈として残す。
const BOOK_PAGE_MATCH_LIMIT: usize = 256;

/// 走査を分割する単位。小さすぎると合流の費用が勝ち、大きすぎると尻尾で遊ぶ。
const BOOK_SCAN_CHUNK: usize = 32_768;

/// 何冊ぶんかのページを、常駐配列への **1 回の走査**でまとめて照合する。
///
/// 本ごとに走査すると、比較回数が同じでも配列 (222 MB) を本の数だけ読み直すことになる。
/// 実測では候補 7 冊で 10.4 分かかっていた。署名は総ページ数 × 32 byte しかないので、
/// 記録側を 1 回流して内側で全ページと比べ、chunk ごとに並列化する。
fn scan_snapshot_for_pages(snapshot: &SearchSnapshot, signatures: &[[u8; 32]]) -> Vec<Vec<u64>> {
    use rayon::prelude::*;

    if signatures.is_empty() {
        return Vec::new();
    }
    let consider = |record: &crate::similar_search_array::SearchRecord,
                    per_page: &mut Vec<Vec<u64>>| {
        if record.quality == 0 {
            return;
        }
        for (page, signature) in signatures.iter().enumerate() {
            if hamming256_within(signature, &record.signature, BOOK_RADIUS).is_some() {
                let bucket = &mut per_page[page];
                // 上限に達したページは「よくあるページ」として扱うので、それ以上は集めない。
                if bucket.len() <= BOOK_PAGE_MATCH_LIMIT {
                    bucket.push(record.item_id);
                }
            }
        }
    };

    let records = &snapshot.base.records;
    let mut per_page = records
        .par_chunks(BOOK_SCAN_CHUNK)
        .enumerate()
        .map(|(chunk, slice)| {
            let mut local = vec![Vec::new(); signatures.len()];
            let start = chunk * BOOK_SCAN_CHUNK;
            for (offset, record) in slice.iter().enumerate() {
                if !snapshot.base_record_is_superseded(start + offset) {
                    consider(record, &mut local);
                }
            }
            local
        })
        .reduce(
            || vec![Vec::new(); signatures.len()],
            |mut left, right| {
                for (bucket, extra) in left.iter_mut().zip(right) {
                    if bucket.len() <= BOOK_PAGE_MATCH_LIMIT {
                        bucket.extend(extra);
                    }
                }
                left
            },
        );
    for entry in snapshot.delta.iter() {
        if let Some(record) = &entry.record {
            consider(record, &mut per_page);
        }
    }
    per_page
}

/// 本のページ群を照合し、identity を SQLite で確定する。
///
/// `pages` は 1 冊ぶん。候補の本をまとめて調べるときは [`collect_book_page_matches_for`] を
/// 使って走査を 1 回にまとめる。
fn collect_book_page_matches(
    db: &SimilarDb,
    snapshot: &SearchSnapshot,
    pages: &[crate::similar_db::SearchRow],
    hash_version: i64,
) -> Result<BookMatchSet, String> {
    let mut out = collect_book_page_matches_for(db, snapshot, &[pages], hash_version)?;
    Ok(out.remove(0))
}

/// 何冊ぶんかを 1 回の走査で照合する。返り値は入力と同じ並び。
fn collect_book_page_matches_for(
    db: &SimilarDb,
    snapshot: &SearchSnapshot,
    books: &[&[crate::similar_db::SearchRow]],
    hash_version: i64,
) -> Result<Vec<BookMatchSet>, String> {
    // 走査対象は quality のあるページだけ。どの本のどのページかは添字で持ち帰る。
    let mut owners = Vec::new();
    let mut signatures = Vec::new();
    for (book, pages) in books.iter().enumerate() {
        for (page, row) in pages.iter().enumerate() {
            if row.item.quality >= BOOK_MIN_QUALITY {
                owners.push((book, page));
                signatures.push(row.item.pdq256);
            }
        }
    }
    let scanned = scan_snapshot_for_pages(snapshot, &signatures);

    // 提案された item_id を一つの読み取り snapshot で identity に変える。配列は候補しか出さない。
    let mut wanted = scanned.iter().flatten().copied().collect::<Vec<_>>();
    wanted.sort_unstable();
    wanted.dedup();
    let resolved = db
        .resolve_pages_by_item_id(&wanted, hash_version)
        .map_err(db_error)?;
    let by_id = resolved
        .into_iter()
        .filter_map(|row| {
            let container = row.item.container_key?;
            let page_index = row.item.page_index?;
            Some((
                row.item_id,
                ResolvedPage {
                    container_key: container,
                    page_index,
                    quality: row.item.quality,
                    signature: row.item.pdq256,
                },
            ))
        })
        .collect::<HashMap<_, _>>();

    let mut out = books
        .iter()
        .map(|pages| BookMatchSet {
            pages: pages
                .iter()
                .map(|_| BookPageMatches {
                    origin_is_common: false,
                    matched: Vec::new(),
                })
                .collect(),
        })
        .collect::<Vec<_>>();
    for ((book, page), ids) in owners.into_iter().zip(scanned) {
        let overflowed = ids.len() > BOOK_PAGE_MATCH_LIMIT;
        let signature = books[book][page].item.pdq256;
        let matched = ids
            .into_iter()
            .filter_map(|id| by_id.get(&id))
            // DB 側の署名が配列と食い違っていた候補はここで落ちる。距離は DB の値で決める。
            .filter(|page| hamming256_within(&signature, &page.signature, BOOK_RADIUS).is_some())
            .map(|page| ResolvedPage {
                container_key: page.container_key.clone(),
                page_index: page.page_index,
                quality: page.quality,
                signature: page.signature,
            })
            .collect::<Vec<_>>();
        let distinct_books = matched
            .iter()
            .map(|page| page.container_key.as_str())
            .collect::<HashSet<_>>()
            .len() as u32;
        out[book].pages[page] = BookPageMatches {
            origin_is_common: overflowed || distinct_books > BOOK_MAX_BOOKS_PER_PAGE,
            matched,
        };
    }
    Ok(out)
}

fn hamming256(left: &[u8; 32], right: &[u8; 32]) -> u32 {
    left.iter()
        .zip(right)
        .map(|(left, right)| (left ^ right).count_ones())
        .sum()
}

/// 半径を超えると分かった時点で打ち切る距離判定。
///
/// 本単位の照会は 1 ページごとに全署名を見るので、比較回数が「ページ数 × 全行数」になる。
/// 無関係な 2 枚は 256 bit のうち 128 bit 前後が違うため、多くは最初の 64 bit で超える。
/// 半径内のときだけ距離を返す。
fn hamming256_within(left: &[u8; 32], right: &[u8; 32], radius: u32) -> Option<u32> {
    let mut distance = 0u32;
    for offset in (0..32).step_by(8) {
        let left = u64::from_le_bytes(left[offset..offset + 8].try_into().expect("8 bytes"));
        let right = u64::from_le_bytes(right[offset..offset + 8].try_into().expect("8 bytes"));
        distance += (left ^ right).count_ones();
        if distance > radius {
            return None;
        }
    }
    Some(distance)
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
    array_refresh: &ArrayRefreshNotifier,
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
                    array_refresh,
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
        if aggregate.report.removed > 0 {
            array_refresh.request();
        }
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
    array_refresh: &'a ArrayRefreshNotifier,
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
    array_refresh: &ArrayRefreshNotifier,
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
            array_refresh,
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
                if self.db.upsert_loose_item(&item).map_err(db_error)? {
                    self.array_refresh.request();
                }
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
        self.array_refresh.request();
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
        // 書庫のエントリ順ではなく、閲覧側と同じページ順に並べてから採番する。
        // ここが展開済みフォルダと食い違うと、本単位の対応付けが崩れる。
        let mut entries = entries;
        entries.sort_by(|left, right| compare_book_pages(&left.entry_name, &right.entry_name));
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
        self.array_refresh.request();
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
        self.array_refresh.request();
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

/// 本の中でページが並ぶ順を決める鍵。
///
/// `page_index` は本単位の照合で「対応が連続した区間になっているか」を見るために使う。
/// したがって**同じ中身なら、ZIP でも展開済みフォルダでも同じ順**でなければならない。
/// 実店では ZIP だけが書庫のエントリ順で採番されており、同じ作品の ZIP 版と展開版を
/// 比べると 373 ページ一致するはずのものが 35 ページしか揃わなかった (順序がばらばらだと
/// 単調な対応付けは最長増加部分列の長さまでしか伸びない)。
///
/// 閲覧側の本のページ順 ([`crate::app::BOOK_READING_PAGE_ORDER`]) と同じく、数値を考慮した
/// ファイル名順で固定する。フォルダの直下しか本にならないので、ディレクトリを先に比べて
/// から名前を比べれば、平らな ZIP と展開済みフォルダは同じ並びになる。
pub fn book_page_order_key(name_within_container: &str) -> (Vec<String>, String) {
    let normalized = name_within_container.replace('\\', "/");
    let mut parts = normalized.split('/').collect::<Vec<_>>();
    let base = parts.pop().unwrap_or("").to_owned();
    (parts.into_iter().map(str::to_owned).collect(), base)
}

/// 同じ本の 2 ページを並べる。[`book_page_order_key`] の鍵どうしを比べる。
pub fn compare_book_pages(left: &str, right: &str) -> std::cmp::Ordering {
    let (left_dirs, left_base) = book_page_order_key(left);
    let (right_dirs, right_base) = book_page_order_key(right);
    for (left, right) in left_dirs.iter().zip(&right_dirs) {
        let ordering = crate::filename_sort::compare_file_names(left, right);
        if ordering != std::cmp::Ordering::Equal {
            return ordering;
        }
    }
    left_dirs
        .len()
        .cmp(&right_dirs.len())
        .then_with(|| crate::filename_sort::compare_file_names(&left_base, &right_base))
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

    /// 本単位の照会を実店で測る。`MIV_SIMILAR_BENCH_DB` に store、
    /// `MIV_SIMILAR_BENCH_CONTAINER` に本のコンテナキーを渡す。
    ///
    /// 起点ページの読み出し、常駐配列の走査、SQLite での identity 確定、分類までを通す。
    /// 旧実装はここで全行を文字列込みで読み直しており、4,629,375 行で 24 秒 / 7.7 GB を
    /// 使ったうえ照会が終わらなかった。
    #[test]
    #[ignore = "manual measurement of the book query against a caller-selected similar.db"]
    fn measure_real_store_book_query() {
        let db_path = std::env::var_os("MIV_SIMILAR_BENCH_DB")
            .map(PathBuf::from)
            .expect("set MIV_SIMILAR_BENCH_DB");
        // コンテナキーを受け取って先頭ページを自分で引く。item_key の区切りは不可視の
        // `\x1f` なので、環境変数へ手で書き写すと必ず取り違える。
        let container_key =
            std::env::var("MIV_SIMILAR_BENCH_CONTAINER").expect("MIV_SIMILAR_BENCH_CONTAINER");

        let resident_before = current_working_set_bytes();
        let db = SimilarDb::open_at(&db_path).unwrap();
        let load_started = std::time::Instant::now();
        let loaded = similar_search_array::load_or_rebuild(
            &db,
            &similar_search_array::base_path(db_path.parent().expect("store has a parent")),
        )
        .unwrap();
        let load_ms = load_started.elapsed().as_secs_f64() * 1000.0;
        let snapshot = loaded.snapshot;
        let rows = snapshot.record_count();
        let resident_loaded = current_working_set_bytes();

        if std::env::var_os("MIV_SIMILAR_BENCH_REPAIR").is_some() {
            let started = std::time::Instant::now();
            let containers = db
                .renumber_container_pages(crate::similar_db::PAGE_ORDER_VERSION, |left, right| {
                    compare_book_pages(left, right)
                })
                .unwrap();
            eprintln!(
                "similar_page_order_repair containers={containers} elapsed_ms={:.1}",
                started.elapsed().as_secs_f64() * 1000.0
            );
        }
        let pages = db
            .load_book_pages(&container_key, current_hash_version())
            .unwrap();
        let origin_key = pages
            .iter()
            .min_by_key(|row| row.item.page_index.unwrap_or(u32::MAX))
            .map(|row| row.item.item_key.clone())
            .expect("no page found for MIV_SIMILAR_BENCH_CONTAINER");

        let query_started = std::time::Instant::now();
        let result = query_book_ready(&db, &snapshot, &origin_key);
        let query_ms = query_started.elapsed().as_secs_f64() * 1000.0;
        let resident_after = current_working_set_bytes();
        let summary = match &result {
            BookQuery::Ready(hits) => format!("Ready({})", hits.len()),
            other => format!("{other:?}"),
        };
        eprintln!(
            "similar_book_query_measurement rows={rows} pages={} load_ms={load_ms:.1} query_ms={query_ms:.1} resident_before_bytes={resident_before} resident_loaded_bytes={resident_loaded} resident_after_bytes={resident_after} query_delta_bytes={} result={summary}",
            pages.len(),
            resident_after.saturating_sub(resident_loaded)
        );
        if let BookQuery::Ready(hits) = &result {
            for hit in hits.iter().take(10) {
                eprintln!(
                    "  other={} relation={:?} matched={} distinctive_a={} distinctive_b={} coverage_a={:.3} coverage_b={:.3} aligned={}",
                    hit.other_container_key,
                    hit.pair.relation,
                    hit.pair.matched,
                    hit.pair.distinctive_a,
                    hit.pair.distinctive_b,
                    hit.pair.coverage_a,
                    hit.pair.coverage_b,
                    hit.pair.alignment.len()
                );
            }
        }
    }

    #[test]
    #[ignore = "manual measurement against a caller-selected similar.db"]
    fn measure_real_store_load_and_uncached_query() {
        let db_path = std::env::var_os("MIV_SIMILAR_BENCH_DB")
            .map(PathBuf::from)
            .expect("set MIV_SIMILAR_BENCH_DB");
        let mode = std::env::var("MIV_SIMILAR_BENCH_LOAD").unwrap_or_else(|_| "base".to_owned());
        let base = db_path.with_file_name("similar.base");
        if mode == "rebuild" {
            match std::fs::remove_file(&base) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => panic!("base reset failed: {error}"),
            }
        }
        let resident_before = current_working_set_bytes();
        let load_started = std::time::Instant::now();
        let db = SimilarDb::open_at(&db_path).unwrap();
        let loaded = match mode.as_str() {
            "rebuild" | "base" => similar_search_array::load_or_rebuild(&db, &base).unwrap(),
            other => panic!("unknown MIV_SIMILAR_BENCH_LOAD mode: {other}"),
        };
        let index = loaded.snapshot;
        let load_source = loaded.source;
        let row_count = index.record_count();
        let load_ms = load_started.elapsed().as_secs_f64() * 1000.0;
        let resident_after = current_working_set_bytes();
        let Some(origin_key) = std::env::var("MIV_SIMILAR_BENCH_ORIGIN").ok() else {
            eprintln!(
                "similar_load_measurement rows={row_count} mode={mode} source={load_source:?} load_ms={load_ms:.3} resident_before_bytes={resident_before} resident_after_bytes={resident_after} resident_delta_bytes={} base_bytes={}",
                resident_after.saturating_sub(resident_before),
                std::fs::metadata(base).map_or(0, |metadata| metadata.len())
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
            "similar_query_measurement rows={row_count} mode={mode} source={load_source:?} load_ms={load_ms:.3} resident_before_bytes={resident_before} resident_after_bytes={resident_after} resident_delta_bytes={} query_ms={query_ms:?} cache_miss_ms={cache_miss_ms:.3} cache_hit_ms={cache_hit_ms:?} hits={hit_count} origin={origin_key:?} base_bytes={}",
            resident_after.saturating_sub(resident_before),
            std::fs::metadata(base).map_or(0, |metadata| metadata.len())
        );
    }

    #[test]
    #[ignore = "manual 4,627,166-row synthetic measurement for duplicate-detection plan §21"]
    fn measure_incremental_array_at_reference_cardinality() {
        const ROWS: u64 = 4_627_166;

        let temp = tempfile::tempdir().unwrap();
        let db_path = SimilarDb::db_path_at(temp.path());
        drop(SimilarDb::open_at(&db_path).unwrap());
        let setup_started = std::time::Instant::now();
        let mut conn = rusqlite::Connection::open(&db_path).unwrap();
        conn.execute_batch("PRAGMA synchronous=OFF;").unwrap();
        let transaction = conn.transaction().unwrap();
        transaction
            .execute(
                "WITH RECURSIVE ids(value) AS (
                   VALUES(1) UNION ALL SELECT value + 1 FROM ids WHERE value < ?1
                 )
                 INSERT INTO item
                   (revision, item_key, kind, container_key, page_index, mtime, file_size,
                    hash_version, pdq256, quality, width, height, format)
                 SELECT 1, printf('c:/library/item-%d.png', value), 0, NULL, NULL, 1, 1,
                        ?2, randomblob(32), 50, 100, 100, 1
                 FROM ids",
                rusqlite::params![i64::try_from(ROWS).unwrap(), current_hash_version()],
            )
            .unwrap();
        transaction.commit().unwrap();
        drop(conn);
        let setup_ms = setup_started.elapsed().as_secs_f64() * 1000.0;

        let db = SimilarDb::open_at(&db_path).unwrap();
        let base_path = similar_search_array::base_path(temp.path());
        let resident_before = current_working_set_bytes();
        let missing_started = std::time::Instant::now();
        let missing = similar_search_array::load_or_rebuild(&db, &base_path).unwrap();
        let missing_ms = missing_started.elapsed().as_secs_f64() * 1000.0;
        let resident_loaded = current_working_set_bytes();
        assert_eq!(missing.snapshot.record_count(), ROWS as usize);
        assert_eq!(missing.source, similar_search_array::LoadSource::Sqlite);
        drop(missing);

        let existing_started = std::time::Instant::now();
        let existing = similar_search_array::load_or_rebuild(&db, &base_path).unwrap();
        let existing_ms = existing_started.elapsed().as_secs_f64() * 1000.0;
        assert_eq!(existing.snapshot.record_count(), ROWS as usize);
        assert_eq!(existing.source, similar_search_array::LoadSource::BaseFile);

        let manager = SimilarIndexManager::new(temp.path().to_path_buf());
        *manager.memory.lock().unwrap() = MemoryState::Ready(Arc::new(existing.snapshot));
        *manager.enabled_roots.write().unwrap() = vec!["c:/library".to_owned()];
        let origin = db
            .load_item("c:/library/item-1.png", current_hash_version())
            .unwrap()
            .unwrap()
            .item;
        let mut near = origin.clone();
        near.item_key = "c:/library/new.png".to_owned();
        near.pdq256[0] ^= 1;
        let add_started = std::time::Instant::now();
        db.upsert_loose_item(&near).unwrap();
        manager.scheduler.request_array_refresh();
        let mut add_to_query_ms = None;
        for _ in 0..2_000 {
            let result = manager.query_item(&origin.item_key);
            if let ItemQuery::Ready(hits) = result.as_ref()
                && hits.iter().any(|hit| hit.item_key == near.item_key)
            {
                add_to_query_ms = Some(add_started.elapsed().as_secs_f64() * 1000.0);
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        let add_to_query_ms = add_to_query_ms.expect("incremental add did not reach query");
        let current = {
            let memory = manager.memory.lock().unwrap();
            let MemoryState::Ready(current) = &*memory else {
                panic!("snapshot disappeared");
            };
            Arc::clone(current)
        };
        assert_eq!(current.delta.len(), 1);

        let compact_started = std::time::Instant::now();
        let compacted = similar_search_array::compacted_base(&current);
        similar_search_array::write_compacted_base(&base_path, &compacted).unwrap();
        let compact_ms = compact_started.elapsed().as_secs_f64() * 1000.0;
        let resident_after = current_working_set_bytes();
        eprintln!(
            "similar_incremental_reference rows={ROWS} setup_ms={setup_ms:.3} base_missing_ms={missing_ms:.3} base_existing_ms={existing_ms:.3} add_to_query_ms={add_to_query_ms:.3} compact_ms={compact_ms:.3} resident_before_bytes={resident_before} resident_loaded_bytes={resident_loaded} resident_after_bytes={resident_after} resident_load_delta_bytes={} base_bytes={} compact_every_changes={}",
            resident_loaded.saturating_sub(resident_before),
            std::fs::metadata(&base_path).unwrap().len(),
            current.compaction_threshold(),
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
            &ArrayRefreshNotifier {
                scheduler: Weak::new(),
            },
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

    fn row(id: u64, key: &str, signature: [u8; 32], quality: u8) -> SearchRow {
        SearchRow {
            item_id: id,
            revision: 1,
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

    fn snapshot_from_rows(rows: &[SearchRow]) -> SearchSnapshot {
        SearchSnapshot::from_base(crate::similar_search_array::BaseArray {
            records: rows
                .iter()
                .map(|row| SearchRecord {
                    item_id: row.item_id,
                    signature: row.item.pdq256,
                    quality: row.item.quality,
                    revision: row.revision,
                })
                .collect(),
            store_id: [1; 16],
            applied_seq: 0,
        })
    }

    fn query_test_rows(rows: &[SearchRow], item_key: &str) -> ItemQuery {
        let db = SimilarDb::open_in_memory().unwrap();
        for row in rows {
            db.upsert_loose_item(&row.item).unwrap();
        }
        let base = db.load_base_search_rows(current_hash_version()).unwrap();
        let snapshot = SearchSnapshot::from_base(crate::similar_search_array::BaseArray {
            records: base.records.into_boxed_slice(),
            store_id: base.store_id,
            applied_seq: base.applied_seq,
        });
        query_item_ready(&db, &snapshot, item_key)
    }

    #[test]
    fn featureless_origin_is_typed_not_an_empty_result() {
        let rows = vec![row(1, "blank", [0; 32], 0)];
        assert_eq!(query_test_rows(&rows, "blank"), ItemQuery::Featureless);
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
        let ready = Arc::new(snapshot_from_rows(&[row(1, "origin", [0; 32], 1)]));
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

        *manager.memory.lock().unwrap() = MemoryState::Ready(Arc::new(snapshot_from_rows(&[row(
            1, "origin", [0; 32], 1,
        )])));
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
        let ItemQuery::Ready(hits) = query_test_rows(&rows, "origin") else {
            panic!("expected ready query");
        };
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].band, MatchBand::NearlyIdentical);
        assert_eq!(hits[0].distance, 8);
        assert_eq!(hits[1].band, MatchBand::OtherVersion);
        assert_eq!(hits[1].distance, 16);
    }

    #[test]
    fn origin_absent_from_array_is_loaded_by_item_key() {
        let db = SimilarDb::open_in_memory().unwrap();
        let near = row(1, "near", [0; 32], 1).item;
        db.upsert_loose_item(&near).unwrap();
        let base = db.load_base_search_rows(current_hash_version()).unwrap();
        let snapshot = SearchSnapshot::from_base(crate::similar_search_array::BaseArray {
            records: base.records.into_boxed_slice(),
            store_id: base.store_id,
            applied_seq: base.applied_seq,
        });
        let mut origin_signature = [0u8; 32];
        origin_signature[0] = 1;
        db.upsert_loose_item(&row(2, "new-origin", origin_signature, 1).item)
            .unwrap();
        let ItemQuery::Ready(hits) = query_item_ready(&db, &snapshot, "new-origin") else {
            panic!("new origin should be read directly from SQLite");
        };
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].item_key, "near");
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
        let base = db.load_base_search_rows(current_hash_version()).unwrap();
        let index = Arc::new(SearchSnapshot::from_base(
            crate::similar_search_array::BaseArray {
                records: base.records.into_boxed_slice(),
                store_id: base.store_id,
                applied_seq: base.applied_seq,
            },
        ));

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
    fn stale_candidate_is_dropped_but_a_later_valid_candidate_survives() {
        let db = SimilarDb::open_in_memory().unwrap();
        let origin = row(1, "origin", [0; 32], 1).item;
        let stale = row(2, "stale", [1; 32], 1).item;
        let valid = row(3, "valid", [2; 32], 1).item;
        for item in [&origin, &stale, &valid] {
            db.upsert_loose_item(item).unwrap();
        }
        let base = db.load_base_search_rows(current_hash_version()).unwrap();
        let snapshot = SearchSnapshot::from_base(crate::similar_search_array::BaseArray {
            records: base.records.into_boxed_slice(),
            store_id: base.store_id,
            applied_seq: base.applied_seq,
        });
        let mut changed = stale.clone();
        changed.file_size += 1;
        changed.pdq256 = [0xff; 32];
        db.upsert_loose_item(&changed).unwrap();

        let ItemQuery::Ready(hits) = query_item_ready(&db, &snapshot, "origin") else {
            panic!("expected ready query");
        };
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].item_key, "valid");
    }

    #[test]
    fn one_committed_add_reaches_the_next_query_without_rebuilding_base() {
        let temp = tempfile::tempdir().unwrap();
        let db_path = SimilarDb::db_path_at(temp.path());
        let db = SimilarDb::open_at(&db_path).unwrap();
        let origin = row(1, "c:/library/origin.png", [0; 32], 1).item;
        db.upsert_loose_item(&origin).unwrap();
        let loaded = similar_search_array::load_or_rebuild(
            &db,
            &similar_search_array::base_path(temp.path()),
        )
        .unwrap();
        let original_base = Arc::clone(&loaded.snapshot.base);

        let manager = SimilarIndexManager::new(temp.path().to_path_buf());
        *manager.memory.lock().unwrap() = MemoryState::Ready(Arc::new(loaded.snapshot));
        *manager.enabled_roots.write().unwrap() = vec!["c:/library".to_owned()];

        let mut near_signature = [0u8; 32];
        near_signature[0] = 1;
        let near = row(2, "c:/library/new.png", near_signature, 1).item;
        let started = std::time::Instant::now();
        db.upsert_loose_item(&near).unwrap();
        manager.scheduler.request_array_refresh();

        for _ in 0..500 {
            let result = manager.query_item(&origin.item_key);
            if let ItemQuery::Ready(hits) = result.as_ref()
                && hits.iter().any(|hit| hit.item_key == near.item_key)
            {
                let elapsed = started.elapsed();
                let current = manager.memory.lock().unwrap();
                let MemoryState::Ready(current) = &*current else {
                    panic!("snapshot disappeared");
                };
                assert!(Arc::ptr_eq(&original_base, &current.base));
                assert_eq!(current.delta.len(), 1);
                eprintln!(
                    "similar_incremental_add_to_query_ms={:.3}",
                    elapsed.as_secs_f64() * 1000.0
                );
                assert!(elapsed < Duration::from_secs(2));
                return;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        panic!("incremental add did not reach the next query");
    }

    #[test]
    fn missing_history_keeps_ready_snapshot_until_rebuild_is_published() {
        let temp = tempfile::tempdir().unwrap();
        let db_path = SimilarDb::db_path_at(temp.path());
        let db = SimilarDb::open_at(&db_path).unwrap();
        let origin = row(1, "c:/library/origin.png", [0; 32], 1).item;
        db.upsert_loose_item(&origin).unwrap();
        let loaded = similar_search_array::load_or_rebuild(
            &db,
            &similar_search_array::base_path(temp.path()),
        )
        .unwrap();
        let old_base = Arc::clone(&loaded.snapshot.base);

        let missed = row(2, "c:/library/missed.png", [1; 32], 1).item;
        db.upsert_loose_item(&missed).unwrap();
        let missed_seq = db.load_item_changes_after(0).unwrap().latest_seq;
        db.prune_item_changes_through(missed_seq).unwrap();
        let newest = row(3, "c:/library/newest.png", [2; 32], 1).item;
        db.upsert_loose_item(&newest).unwrap();
        let latest_seq = db.load_item_changes_after(0).unwrap().latest_seq;

        let manager = SimilarIndexManager::new(temp.path().to_path_buf());
        *manager.memory.lock().unwrap() = MemoryState::Ready(Arc::new(loaded.snapshot));
        manager.scheduler.request_array_refresh();

        for _ in 0..500 {
            let state = manager.memory.lock().unwrap();
            let MemoryState::Ready(current) = &*state else {
                panic!("history recovery must retain a usable snapshot");
            };
            if current.applied_seq == latest_seq {
                assert!(!Arc::ptr_eq(&old_base, &current.base));
                assert_eq!(current.base.records.len(), 3);
                return;
            }
            assert!(Arc::ptr_eq(&old_base, &current.base));
            drop(state);
            std::thread::sleep(Duration::from_millis(5));
        }
        panic!("missing history did not rebuild and publish a current base");
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
            array_refresh: &ArrayRefreshNotifier {
                scheduler: Weak::new(),
            },
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

    /// 2 冊分のページを公開済みコンテナとして書き、本単位の照会が実際の経路
    /// (常駐配列で候補、SQLite で identity) を通ることを確かめる。
    fn publish_book(db: &SimilarDb, container_key: &str, pages: &[StoredItem]) {
        let generation = db
            .begin_container_build(
                container_key,
                ContainerKind::ImageFolder,
                pages.len() as u32,
                1,
                1,
            )
            .unwrap();
        for page in pages {
            db.stage_item(generation, page).unwrap();
        }
        db.complete_container(container_key, generation).unwrap();
    }

    fn book_page(container: &str, page: u32, marker: u8) -> StoredItem {
        StoredItem {
            item_key: format!("{container}/{page}"),
            kind: ItemKind::Image,
            container_key: Some(container.to_owned()),
            page_index: Some(page),
            mtime: 1,
            file_size: 1,
            hash_version: current_hash_version(),
            pdq256: [marker; 32],
            quality: 10,
            width: 100,
            height: 100,
            format: 1,
        }
    }

    #[test]
    fn book_query_calls_dupe_book_with_measured_product_params() {
        let db = SimilarDb::open_in_memory().unwrap();
        for container in ["book-a", "book-b"] {
            let pages = (0..3)
                .map(|page| book_page(container, page, page as u8))
                .collect::<Vec<_>>();
            publish_book(&db, container, &pages);
        }
        let base = db.load_base_search_rows(current_hash_version()).unwrap();
        let snapshot = SearchSnapshot::from_base(crate::similar_search_array::BaseArray {
            records: base.records.into_boxed_slice(),
            store_id: base.store_id,
            applied_seq: base.applied_seq,
        });

        let BookQuery::Ready(relations) = query_book_ready(&db, &snapshot, "book-a") else {
            panic!("expected a book result");
        };
        assert_eq!(relations.len(), 1);
        assert_eq!(relations[0].other_container_key, "book-b");
        assert_eq!(relations[0].pair.relation, dupe::book::Relation::Same);
        assert_eq!(relations[0].pair.matched, BOOK_MIN_MATCHED_PAGES);
    }

    /// 本の関係はページではなく本で引く。同じ本のどのページからでも同じ照会になり、
    /// 読み進めても引き直しが起きないこと。ページで引くと 1 ページごとに数秒待たされる。
    #[test]
    fn every_page_of_a_book_asks_the_same_question() {
        use crate::grid_item::GridItem;

        let zip = PathBuf::from(r"C:\books\a.zip");
        let first = GridItem::ZipImage {
            zip_path: zip.clone(),
            entry_name: "001.jpg".to_owned(),
        };
        let last = GridItem::ZipImage {
            zip_path: zip.clone(),
            entry_name: "400.jpg".to_owned(),
        };
        assert_eq!(
            crate::app::similar_index_container_key(&first),
            crate::app::similar_index_container_key(&last)
        );
        assert_ne!(
            crate::app::similar_index_item_key(&first),
            crate::app::similar_index_item_key(&last),
            "the pages themselves stay distinct; only the book key is shared"
        );

        // 画像は親フォルダで引く。本として索引されていなければ照会側が「本ではない」と答える。
        let page = GridItem::Image(PathBuf::from(r"C:\books\b\003.png"));
        assert_eq!(
            crate::app::similar_index_container_key(&page).as_deref(),
            Some("c:/books/b")
        );
    }

    /// コンテナに属さない画像は本ではない。単体画像の照会と結果を混ぜない。
    #[test]
    fn a_loose_image_is_not_a_book() {
        let db = SimilarDb::open_in_memory().unwrap();
        let mut loose = book_page("ignored", 0, 7);
        loose.item_key = "c:/loose.png".to_owned();
        loose.container_key = None;
        loose.page_index = None;
        db.upsert_loose_item(&loose).unwrap();
        let base = db.load_base_search_rows(current_hash_version()).unwrap();
        let snapshot = SearchSnapshot::from_base(crate::similar_search_array::BaseArray {
            records: base.records.into_boxed_slice(),
            store_id: base.store_id,
            applied_seq: base.applied_seq,
        });
        assert_eq!(
            query_book_ready(&db, &snapshot, "c:/loose"),
            BookQuery::NotBook
        );
    }

    /// 全ページが featureless な本は、0 件ではなく「判定できない」として返る。
    #[test]
    fn a_book_of_featureless_pages_is_typed_not_empty() {
        let db = SimilarDb::open_in_memory().unwrap();
        let pages = (0..3)
            .map(|page| {
                let mut page = book_page("blank", page, 0);
                page.quality = 0;
                page
            })
            .collect::<Vec<_>>();
        publish_book(&db, "blank", &pages);
        let base = db.load_base_search_rows(current_hash_version()).unwrap();
        let snapshot = SearchSnapshot::from_base(crate::similar_search_array::BaseArray {
            records: base.records.into_boxed_slice(),
            store_id: base.store_id,
            applied_seq: base.applied_seq,
        });
        assert_eq!(
            query_book_ready(&db, &snapshot, "blank"),
            BookQuery::Featureless
        );
    }

    /// 平らな ZIP と、それを展開したフォルダが同じ並びになること。
    ///
    /// ここが食い違うと単調な対応付けが崩れ、373 ページ揃うはずの 2 冊が 35 ページしか
    /// 一致しない (実店で観測)。
    #[test]
    fn an_archive_and_its_extracted_folder_order_pages_the_same() {
        let names = ["10.jpg", "2.jpg", "1.jpg", "p03.png"];
        let mut archive = names.to_vec();
        archive.sort_by(|left, right| compare_book_pages(left, right));
        assert_eq!(archive, ["1.jpg", "2.jpg", "10.jpg", "p03.png"]);

        // 展開済みフォルダ側は索引が file_name で並べている。同じ結果になること。
        let mut folder = names.to_vec();
        folder.sort_by(|left, right| crate::filename_sort::compare_file_names(left, right));
        assert_eq!(archive, folder);
    }

    /// 入れ子のあるアーカイブは、ディレクトリを先に見てから名前を見る。同じ名前のページが
    /// 別のフォルダにあっても、並びが混ざらない。
    #[test]
    fn nested_archive_pages_sort_by_directory_then_name() {
        let mut names = vec![
            "vol2/1.jpg",
            "vol10/1.jpg",
            "vol1/10.jpg",
            "vol1/2.jpg",
            "cover.jpg",
        ];
        names.sort_by(|left, right| compare_book_pages(left, right));
        assert_eq!(
            names,
            [
                "cover.jpg",
                "vol1/2.jpg",
                "vol1/10.jpg",
                "vol2/1.jpg",
                "vol10/1.jpg"
            ]
        );
    }

    /// 書庫のエントリ順で保存された古い索引を、再デコードせずに振り直す。
    #[test]
    fn a_stale_archive_page_order_is_repaired_without_rehashing() {
        let db = SimilarDb::open_in_memory().unwrap();
        let container = "c:/books/a.zip";
        let entries = ["10.jpg", "2.jpg", "1.jpg"];
        let generation = db
            .begin_container_build(container, ContainerKind::Zip, entries.len() as u32, 1, 1)
            .unwrap();
        // 書庫のエントリ順のまま採番された状態を作る。
        for (page, entry) in entries.iter().enumerate() {
            let mut item = book_page(container, page as u32, page as u8);
            item.item_key = format!("{container}/{entry}");
            db.stage_item(generation, &item).unwrap();
        }
        db.complete_container(container, generation).unwrap();

        let before = db
            .load_book_pages(container, current_hash_version())
            .unwrap()
            .into_iter()
            .map(|row| row.item.item_key)
            .collect::<Vec<_>>();
        assert_eq!(
            before,
            [
                "c:/books/a.zip/10.jpg",
                "c:/books/a.zip/2.jpg",
                "c:/books/a.zip/1.jpg"
            ],
            "fixture reproduces the archive order the indexer used to store"
        );
        let signatures_before = db
            .load_book_pages(container, current_hash_version())
            .unwrap()
            .into_iter()
            .map(|row| row.item.pdq256)
            .collect::<Vec<_>>();

        assert_eq!(
            db.renumber_container_pages(crate::similar_db::PAGE_ORDER_VERSION, |left, right| {
                compare_book_pages(left, right)
            })
            .unwrap(),
            1
        );

        let after = db
            .load_book_pages(container, current_hash_version())
            .unwrap();
        assert_eq!(
            after
                .iter()
                .map(|row| row.item.item_key.as_str())
                .collect::<Vec<_>>(),
            [
                "c:/books/a.zip/1.jpg",
                "c:/books/a.zip/2.jpg",
                "c:/books/a.zip/10.jpg"
            ]
        );
        // 署名は振り直しの対象ではない。ここが変わっていたら再デコードが起きている。
        let mut signatures_after = after.iter().map(|row| row.item.pdq256).collect::<Vec<_>>();
        signatures_after.sort();
        let mut expected = signatures_before;
        expected.sort();
        assert_eq!(signatures_after, expected);
        assert_eq!(
            db.page_order_version().unwrap(),
            crate::similar_db::PAGE_ORDER_VERSION
        );
    }

    /// 半径を超えたと分かった時点で打ち切っても、半径内の距離は完全一致する。
    #[test]
    fn the_early_exit_distance_agrees_with_the_full_one() {
        let mut left = [0u8; 32];
        let mut right = [0u8; 32];
        for bit in 0..=64u32 {
            for index in 0..32 {
                right[index] = 0;
            }
            let mut remaining = bit;
            let mut index = 0;
            while remaining >= 8 {
                right[index] = 0xff;
                remaining -= 8;
                index += 1;
            }
            right[index] = (1u16 << remaining).wrapping_sub(1) as u8;
            let full = hamming256(&left, &right);
            assert_eq!(full, bit, "fixture builds the intended distance");
            assert_eq!(hamming256_within(&left, &right, 64), Some(full));
            if bit > 0 {
                assert_eq!(hamming256_within(&left, &right, bit - 1), None);
            }
            assert_eq!(hamming256_within(&left, &right, bit), Some(full));
        }
        left[31] = 0xff;
        assert_eq!(hamming256_within(&left, &left, 0), Some(0));
    }
}
