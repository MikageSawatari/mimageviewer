//! お気に入り配下の「別バージョン」索引ジョブと遅延ロード線形検索。

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};
use std::path::{Component, Path, PathBuf, Prefix};
use std::sync::{
    Arc, Condvar, Mutex, OnceLock, RwLock, Weak,
    atomic::{AtomicBool, AtomicU64, Ordering},
};
use std::time::Duration;

use crate::dupe::{self, Algo, Sig};
use crate::similar_book_engine::{EngineError as BookEngineError, SimilarBookQueryEngine};
use crate::similar_book_query::{
    BookQueryClient, BookQueryDispatchDecision, BookQueryExecutor, BookQueryPoll, BookQueryRequest,
    BookQueryRuntime, BookQueryTerminal,
};
use crate::similar_db::{
    CandidateIdentity, CompletedIndexStats, ContainerKind, Freshness, FullReconcileInventory,
    ItemKind, SimilarDb, StoredItem, current_hash_version,
};
use crate::similar_image::{
    PDF_RENDER_LONG_EDGE, ProxySource, SimilarImageFormat, proxy_from_source,
};
use crate::similar_search_array::{self, SearchRecord, SearchSnapshot};
use image::GenericImageView;
use uuid::Uuid;

/// Whether the alternate-version index is exposed and allowed to start.
///
/// The product constant is the single release switch.  Tests that exercise the
/// retained implementation pass `Enabled` explicitly instead of changing the
/// shipped policy.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SimilarFeatureCapability {
    Enabled,
    Paused,
}

impl SimilarFeatureCapability {
    pub(crate) const fn is_enabled(self) -> bool {
        matches!(self, Self::Enabled)
    }
}

pub(crate) const PRODUCT_SIMILAR_FEATURE_CAPABILITY: SimilarFeatureCapability =
    SimilarFeatureCapability::Paused;

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

fn next_similar_query_trace_id() -> u64 {
    static NEXT_ID: AtomicU64 = AtomicU64::new(1);
    NEXT_ID.fetch_add(1, Ordering::Relaxed).max(1)
}
const INDEX_PER_VOLUME_OUTSTANDING_LIMIT: usize = 8;
// 操作中も差分照合を完全には止めず、既存 ActivityGate の状態で新規開始を 1 本へ絞る。
const INDEX_ACTIVE_GLOBAL_OUTSTANDING_LIMIT: usize = 1;
const INDEX_ACTIVE_PER_VOLUME_OUTSTANDING_LIMIT: usize = 1;
const INDEX_LIMIT_RECHECK: Duration = Duration::from_millis(50);
const DIRTY_SCOPE_LIMIT: usize = 4096;

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
pub enum IndexDegradedReason {
    FilesystemObservationIncomplete,
    ArrayPublication(String),
    WatchUnavailable,
}

impl IndexDegradedReason {
    pub(crate) fn user_message(&self) -> String {
        match self {
            Self::FilesystemObservationIncomplete => {
                "ファイルの確認が完了しませんでした。既存の索引は保持されています".to_owned()
            }
            Self::ArrayPublication(detail) => {
                format!("検索用一覧への反映を確認できませんでした: {detail}")
            }
            Self::WatchUnavailable => {
                "一部フォルダーの監視を確認できません。索引は作成済みです".to_owned()
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum IndexProgress {
    Idle,
    Running(RunningProgress),
    AwaitingWatch(IndexReport),
    AwaitingArray(IndexReport),
    Complete(IndexReport),
    Cancelled(IndexReport),
    Degraded {
        report: IndexReport,
        reason: IndexDegradedReason,
    },
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
    /// 問い合わせ worker が一度だけ実在パスへ戻した表示・遷移先。
    /// 消失済みなら正規化 key から組み立てた fallback を保持する。
    pub target: Option<SimilarItemTarget>,
}

/// いま見ているページ自身。**候補と同じ形で出すために持つ。**
///
/// 以前は寸法だけを候補 1 件ごとに複製していた。同じ事実を件数分置くと、片方だけ更新される
/// 余地が残る。起点は 1 つしかないので 1 か所に置く。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OriginItem {
    pub item_key: String,
    pub kind: ItemKind,
    pub mtime: i64,
    pub file_size: i64,
    pub width: u32,
    pub height: u32,
    pub format: SimilarImageFormat,
    pub target: Option<SimilarItemTarget>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ItemMatches {
    pub origin: OriginItem,
    pub hits: Vec<QueryHit>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ItemQuery {
    NoIndex,
    Preparing,
    Ready(ItemMatches),
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

/// Per-origin-page state shared by every relation hit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BookPageBaseline {
    Unmatched,
    Excluded,
}

impl BookPageBaseline {
    pub(crate) fn state(self) -> BookPageState {
        match self {
            Self::Unmatched => BookPageState::Unmatched,
            Self::Excluded => BookPageState::Excluded,
        }
    }
}

/// State stored only for a page that has a concrete counterpart in one hit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BookPageMatchState {
    Strong,
    Weak,
}

impl BookPageMatchState {
    pub(crate) fn state(self) -> BookPageState {
        match self {
            Self::Strong => BookPageState::Strong,
            Self::Weak => BookPageState::Weak,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BookOriginPage {
    pub item_key: String,
    pub baseline: BookPageBaseline,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BookOrigin {
    pub pages: Box<[BookOriginPage]>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct BookPageMatch {
    /// Slot in [`BookOrigin::pages`] replaced by this match.
    pub origin_slot: usize,
    pub state: BookPageMatchState,
    /// 相手の本での対応ページ。押すとそこへ移動する。
    pub other_page_index: u32,
    /// 移動先。`items` に無い場所も開けるよう、照会側で解決しておく。
    pub other_target: Option<SimilarItemTarget>,
    pub other_item_key: String,
    /// サムネイルのキャッシュ判定に使う。帯にホバーしたページを出すため。
    pub other_mtime: i64,
    pub other_file_size: i64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct BookRelationHit {
    pub other_container_key: String,
    /// 相手の本の総ページ数。帯は**この本**のページで引くので、相手の長さは別に示さないと
    /// 帯が何を表しているのか読めない。
    pub other_page_count: u32,
    pub pair: dupe::book::BookPair,
    /// 起点ページ順の sparse override。対応が無いページは [`BookOrigin`] の baseline を使う。
    overrides: Box<[BookPageMatch]>,
}

/// 本の関係一式。
#[derive(Clone, Debug, PartialEq)]
pub struct BookRelations {
    /// Every hit borrows this one origin strip instead of copying its baseline pages.
    pub origin: Arc<BookOrigin>,
    pub hits: Vec<BookRelationHit>,
}

#[derive(Clone, Copy)]
pub(crate) struct BookStripPage<'a> {
    origin: &'a BookOriginPage,
    matched: Option<&'a BookPageMatch>,
}

impl BookStripPage<'_> {
    pub(crate) fn state(self) -> BookPageState {
        self.matched.map_or_else(
            || self.origin.baseline.state(),
            |matched| matched.state.state(),
        )
    }
}

impl<'a> BookStripPage<'a> {
    pub(crate) fn match_override(self) -> Option<&'a BookPageMatch> {
        self.matched
    }
}

/// Borrowed dense projection of one sparse relation strip.
#[derive(Clone, Copy)]
pub(crate) struct BookStripView<'a> {
    origin: &'a BookOrigin,
    hit: &'a BookRelationHit,
}

impl<'a> BookStripView<'a> {
    pub(crate) fn new(origin: &'a BookOrigin, hit: &'a BookRelationHit) -> Self {
        Self { origin, hit }
    }

    pub(crate) fn len(self) -> usize {
        self.origin.pages.len()
    }

    pub(crate) fn is_empty(self) -> bool {
        self.origin.pages.is_empty()
    }

    pub(crate) fn get(self, slot: usize) -> Option<BookStripPage<'a>> {
        let origin = self.origin.pages.get(slot)?;
        let matched = self
            .hit
            .overrides
            .binary_search_by_key(&slot, |entry| entry.origin_slot)
            .ok()
            .and_then(|index| self.hit.overrides.get(index));
        Some(BookStripPage { origin, matched })
    }

    pub(crate) fn baseline_state(self, slot: usize) -> Option<BookPageState> {
        self.origin
            .pages
            .get(slot)
            .map(|page| page.baseline.state())
    }

    pub(crate) fn overrides(self) -> &'a [BookPageMatch] {
        &self.hit.overrides
    }

    pub(crate) fn first_target(self) -> Option<&'a BookPageMatch> {
        self.hit
            .overrides
            .iter()
            .find(|entry| entry.other_target.is_some())
    }
}

impl BookRelationHit {
    pub(crate) fn new(
        other_container_key: String,
        other_page_count: u32,
        pair: dupe::book::BookPair,
        mut overrides: Vec<BookPageMatch>,
        origin_len: usize,
    ) -> Result<Self, String> {
        overrides.sort_by_key(|entry| entry.origin_slot);
        let mut previous = None;
        for entry in &overrides {
            if entry.origin_slot >= origin_len {
                return Err(format!(
                    "book strip override {} is outside origin length {origin_len}",
                    entry.origin_slot
                ));
            }
            if previous == Some(entry.origin_slot) {
                return Err(format!(
                    "book strip contains duplicate override {}",
                    entry.origin_slot
                ));
            }
            previous = Some(entry.origin_slot);
        }
        Ok(Self {
            other_container_key,
            other_page_count,
            pair,
            overrides: overrides.into_boxed_slice(),
        })
    }

    pub(crate) fn overrides(&self) -> &[BookPageMatch] {
        &self.overrides
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum BookQuery {
    Preparing,
    Ready(BookRelations),
    Featureless,
    NotIndexed,
    NotBook,
    Failed(String),
}

/// Move-only viewer identity for the global similar-book query owner.
///
/// The generic scheduler binding stays private so public callers cannot forge or share its
/// internal client id.
#[derive(Default)]
pub struct SimilarBookQueryClient(BookQueryClient<Arc<BookQuery>>);

impl SimilarBookQueryClient {
    pub fn new() -> Self {
        Self::default()
    }

    fn inner(&self) -> &BookQueryClient<Arc<BookQuery>> {
        &self.0
    }

    pub(crate) fn retain(&self) {
        self.0.retain();
    }

    pub(crate) fn withdraw(&self) {
        self.0.withdraw();
    }

    #[cfg(test)]
    pub(crate) fn demand_snapshot_for_test(
        &self,
    ) -> crate::similar_book_query::BookQueryDemandSnapshot {
        self.0.demand_snapshot_for_test()
    }
}

pub struct SimilarIndexManager {
    data_dir: PathBuf,
    progress: Arc<Mutex<IndexProgress>>,
    memory: Arc<Mutex<MemoryState>>,
    summary: Arc<Mutex<SummaryState>>,
    memory_epoch: Arc<AtomicU64>,
    item_query: Arc<Mutex<ItemQueryCache>>,
    enabled_roots: Arc<RwLock<Vec<String>>>,
    scheduler: Arc<SimilarIndexScheduler>,
    #[cfg(test)]
    item_query_test_hook: Mutex<Option<ItemQueryTestHook>>,
}

#[cfg(test)]
struct ItemQueryTestHook {
    started: std::sync::mpsc::Sender<()>,
    resume: std::sync::mpsc::Receiver<()>,
    completed: std::sync::mpsc::Sender<()>,
}

impl SimilarIndexManager {
    pub(crate) fn new_if_enabled(
        capability: SimilarFeatureCapability,
        data_dir: PathBuf,
        notify_book_query_change: impl Fn() + Send + Sync + 'static,
    ) -> Option<Self> {
        capability
            .is_enabled()
            .then(|| Self::new_with_book_query_notifier(data_dir, notify_book_query_change))
    }

    /// DB を開かない軽量 constructor。起動時 I/O を増やさない。
    pub fn new(data_dir: PathBuf) -> Self {
        Self::new_with_book_query_notifier(data_dir, || {})
    }

    pub(crate) fn new_with_book_query_notifier(
        data_dir: PathBuf,
        notify_book_query_change: impl Fn() + Send + Sync + 'static,
    ) -> Self {
        let progress = Arc::new(Mutex::new(IndexProgress::Idle));
        let memory = Arc::new(Mutex::new(MemoryState::Unloaded));
        let summary = Arc::new(Mutex::new(SummaryState::Unloaded));
        let memory_epoch = Arc::new(AtomicU64::new(0));
        let item_query = Arc::new(Mutex::new(ItemQueryCache::default()));
        let enabled_roots = Arc::new(RwLock::new(Vec::new()));
        let scheduler = Arc::new_cyclic(|scheduler| {
            let runtime_scheduler = scheduler.clone();
            let probe_scheduler = scheduler.clone();
            let book_query = BookQueryExecutor::with_dispatch_gate(
                move || {
                    // The old book scan pool lowered all query CPU work. The new executor owns
                    // the whole query, so lower its one worker when each runtime is initialized.
                    lower_current_thread_priority();
                    Ok(Box::new(SchedulerBookQueryRuntime {
                        scheduler: runtime_scheduler.clone(),
                        engine: BookEngineSlot::Vacant,
                    }))
                },
                notify_book_query_change,
                move || {
                    probe_scheduler.upgrade().map_or_else(
                        || {
                            BookQueryDispatchDecision::Complete(BookQueryTerminal::Failed(
                                "similar book query scheduler is unavailable".to_owned(),
                            ))
                        },
                        |scheduler: Arc<SimilarIndexScheduler>| {
                            scheduler.book_query_dispatch_decision()
                        },
                    )
                },
            );
            SimilarIndexScheduler {
                data_dir: data_dir.clone(),
                progress: Arc::clone(&progress),
                memory: Arc::clone(&memory),
                summary: Arc::clone(&summary),
                memory_epoch: Arc::clone(&memory_epoch),
                item_query: Arc::clone(&item_query),
                book_query,
                #[cfg(test)]
                book_query_phase_probe: Mutex::new(None),
                known_book_store: Mutex::new(ObservedBookStore::Unobserved),
                prefill_db: Arc::new(Mutex::new(None)),
                enabled_roots: Arc::clone(&enabled_roots),
                state: Mutex::new(SchedulerState::default()),
                array_update: Mutex::new(ArrayUpdateState::default()),
                array_changed: Condvar::new(),
                compaction_running: AtomicBool::new(false),
                #[cfg(test)]
                full_jobs_started: AtomicU64::new(0),
                #[cfg(test)]
                delta_jobs_started: AtomicU64::new(0),
            }
        });
        Self {
            data_dir,
            progress,
            memory,
            summary,
            memory_epoch,
            item_query,
            enabled_roots,
            scheduler,
            #[cfg(test)]
            item_query_test_hook: Mutex::new(None),
        }
    }

    #[cfg(test)]
    fn set_book_query_test_phase_probe(
        &self,
        probe: Arc<crate::similar_book_query_test_probe::BookQueryTestPhaseProbe>,
    ) {
        *self
            .scheduler
            .book_query_phase_probe
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = Some(probe);
    }

    /// `auto_index_similar` が有効なお気に入りを、索引の完全な対象 snapshot として反映する。
    /// I/O は scheduler worker 内だけで行い、この呼び出しは UI スレッドをブロックしない。
    pub fn configure(
        &self,
        favorites: &[crate::settings::FavoriteEntry],
        pdf_passwords: crate::pdf_passwords::PdfPasswordStore,
        activity_gate: Option<Arc<crate::activity_gate::ActivityGate>>,
        excluded_roots: Vec<PathBuf>,
    ) {
        let roots: Vec<ConfiguredRoot> = favorites
            .iter()
            .filter(|favorite| favorite.auto_index_similar)
            .map(|favorite| ConfiguredRoot {
                favorite_id: favorite.id,
                key: crate::search_index_db::normalize_path(&favorite.path),
                path: favorite.path.clone(),
            })
            .collect();
        let should_load = !roots.is_empty();
        self.scheduler
            .configure(roots, pdf_passwords, activity_gate, excluded_roots);
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

    #[cfg(test)]
    pub(crate) fn reconcile_job_counts_for_test(&self) -> (u64, u64) {
        (
            self.scheduler.full_jobs_started.load(Ordering::Acquire),
            self.scheduler.delta_jobs_started.load(Ordering::Acquire),
        )
    }

    /// 同じ表示項目と同じメモリ snapshot への照会は Arc ごと再利用する。
    /// 線形走査と SQLite point lookup は worker 上だけで行う。
    pub fn query_item(&self, item_key: &str) -> Arc<ItemQuery> {
        if !self.item_is_enabled(item_key) {
            return Arc::new(ItemQuery::NotIndexed);
        }
        let running = matches!(
            self.progress(),
            IndexProgress::Running(_)
                | IndexProgress::AwaitingWatch(_)
                | IndexProgress::AwaitingArray(_)
        );
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
        #[cfg(test)]
        let test_hook = self
            .item_query_test_hook
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take();
        let spawn_result = std::thread::Builder::new()
            .name("similar-item-query".to_owned())
            .spawn(move || {
                let perf_started = crate::perf::is_enabled().then(std::time::Instant::now);
                let cpu_started = perf_started.and_then(|_| current_thread_cpu_100ns());
                let trace_id = perf_started.map_or(0, |_| next_similar_query_trace_id());
                if perf_started.is_some() {
                    crate::perf::event(
                        "similar_item",
                        "start",
                        Some(&query_key),
                        trace_id,
                        &[
                            ("memory_epoch", serde_json::Value::from(epoch)),
                            (
                                "snapshot_records",
                                serde_json::Value::from(index.record_count() as u64),
                            ),
                            (
                                "priority",
                                serde_json::Value::from(current_thread_priority()),
                            ),
                        ],
                    );
                }
                #[cfg(test)]
                if let Some(hook) = test_hook.as_ref() {
                    let _ = hook.started.send(());
                    let _ = hook.resume.recv();
                }
                let mut result = SimilarDb::open_at(&db_path)
                    .map_err(db_error)
                    .map_or_else(ItemQuery::Failed, |db| {
                        query_item_ready(&db, &index, &query_key)
                    });
                if let ItemQuery::Ready(ItemMatches { hits, .. }) = &mut result {
                    let roots = enabled_roots.read().unwrap_or_else(|e| e.into_inner());
                    hits.retain(|hit| key_is_under_any(&hit.item_key, &roots));
                }
                let terminal = item_query_terminal_label(&result);
                let hit_count = perf_started.map_or(0, |_| match &result {
                    ItemQuery::Ready(matches) => matches.hits.len(),
                    _ => 0,
                });
                let active = if epoch_guard.load(Ordering::Acquire) == epoch {
                    replace_cached_item_query_if_current(
                        &cache,
                        &query_key,
                        epoch,
                        Arc::new(result),
                    )
                } else {
                    false
                };
                if let Some(started) = perf_started {
                    let cpu_ms = cpu_started
                        .zip(current_thread_cpu_100ns())
                        .map(|(before, after)| after.saturating_sub(before) as f64 / 10_000.0);
                    crate::perf::event(
                        "similar_item",
                        "end",
                        Some(&query_key),
                        trace_id,
                        &[
                            ("memory_epoch", serde_json::Value::from(epoch)),
                            (
                                "wall_ms",
                                serde_json::Value::from(started.elapsed().as_secs_f64() * 1000.0),
                            ),
                            (
                                "thread_cpu_ms",
                                cpu_ms.map_or(serde_json::Value::Null, serde_json::Value::from),
                            ),
                            (
                                "priority",
                                serde_json::Value::from(current_thread_priority()),
                            ),
                            ("active", serde_json::Value::from(active)),
                            ("stale", serde_json::Value::from(!active)),
                            ("terminal", serde_json::Value::from(terminal)),
                            ("hit_count", serde_json::Value::from(hit_count as u64)),
                        ],
                    );
                }
                #[cfg(test)]
                if let Some(hook) = test_hook {
                    let _ = hook.completed.send(());
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
        matches!(
            self.progress(),
            IndexProgress::Running(_)
                | IndexProgress::AwaitingWatch(_)
                | IndexProgress::AwaitingArray(_)
        ) && matches!(
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
    pub fn query_book(
        &self,
        client: &SimilarBookQueryClient,
        container_key: &str,
    ) -> Arc<BookQuery> {
        let result = match self
            .scheduler
            .book_query
            .query(client.inner(), container_key)
        {
            BookQueryPoll::Preparing => Arc::new(BookQuery::Preparing),
            BookQueryPoll::Ready(result) => result,
            BookQueryPoll::Failed(error) => Arc::new(BookQuery::Failed(error)),
            BookQueryPoll::Withdrawn => Arc::new(BookQuery::NotIndexed),
            BookQueryPoll::ExecutorStopped => Arc::new(BookQuery::Failed(
                "similar book query executor stopped".to_owned(),
            )),
        };
        // Keep the completed NotIndexed value in the owner. During an active index run only the
        // presentation is Preparing; the next terminal publisher schedules a soft refresh.
        if matches!(result.as_ref(), BookQuery::NotIndexed)
            && self.item_is_enabled(container_key)
            && self.scheduler.worker_is_running()
        {
            return Arc::new(BookQuery::Preparing);
        }
        result
    }

    pub(crate) fn withdraw_book_query(&self, client: &SimilarBookQueryClient) {
        self.scheduler.book_query.withdraw(client.inner());
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
    let worker_scheduler = scheduler.clone();
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
            cache.lock().unwrap_or_else(|e| e.into_inner()).entries.clear();
            epoch_guard.fetch_add(1, Ordering::AcqRel);
            if let Some(scheduler) = worker_scheduler.and_then(|scheduler| scheduler.upgrade()) {
                scheduler.request_array_refresh();
                scheduler.book_query.soft_refresh();
            }
        });
    if let Err(error) = spawn_result {
        {
            *memory.lock().unwrap_or_else(|e| e.into_inner()) =
                MemoryState::Failed(format!("similar index load worker start failed: {error}"));
        }
        if let Some(scheduler) = scheduler.and_then(|scheduler| scheduler.upgrade()) {
            scheduler.book_query.soft_refresh();
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MemoryLoadTrigger {
    Configure,
    IndexRunOpen,
    QueryFallback,
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
    roots: Vec<ConfiguredRoot>,
    excluded_roots: Vec<PathBuf>,
    excluded_root_keys: Vec<String>,
    pdf_passwords: crate::pdf_passwords::PdfPasswordStore,
    password_revision: u64,
    activity_gate: Option<Arc<crate::activity_gate::ActivityGate>>,
}

impl SchedulerConfig {
    fn scan_roots(&self) -> Vec<PathBuf> {
        let mut roots = self
            .roots
            .iter()
            .map(|root| root.path.clone())
            .collect::<Vec<_>>();
        roots.sort();
        roots.dedup();
        roots
    }

    fn normalized_roots(&self) -> Vec<String> {
        let mut roots = self
            .roots
            .iter()
            .map(|root| root.key.clone())
            .collect::<Vec<_>>();
        roots.sort();
        roots.dedup();
        roots
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ConfiguredRoot {
    favorite_id: Uuid,
    path: PathBuf,
    key: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum WatchHealth {
    Pending,
    Ready,
    Unavailable,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct RootWatchState {
    root_key: String,
    registration_generation: u64,
    health: WatchHealth,
    gap_epoch: u64,
    repaired_gap_epoch: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SimilarWatchRegistration {
    favorite_id: Uuid,
    root_key: String,
    registration_generation: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FullReason {
    Initial,
    Reconfigure,
    Overflow,
    WatchRecovery,
    Manual,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct FullIntent {
    config_epoch: u64,
    required_gap_epoch: u64,
    reason: FullReason,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum DirtyScope {
    DirectoryContents(PathBuf),
    Subtree(PathBuf),
    RemovedPrefix(PathBuf),
    RootRepair(PathBuf),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ChangedPathObservation {
    File,
    Directory,
    Missing,
    Unknown,
}

impl DirtyScope {
    fn path(&self) -> &Path {
        match self {
            Self::DirectoryContents(path)
            | Self::Subtree(path)
            | Self::RemovedPrefix(path)
            | Self::RootRepair(path) => path,
        }
    }

    fn normalized_identity(&self) -> (u8, String) {
        let kind = match self {
            Self::DirectoryContents(_) => 0,
            Self::Subtree(_) => 1,
            Self::RemovedPrefix(_) => 2,
            Self::RootRepair(_) => 3,
        };
        (kind, crate::search_index_db::normalize_path(self.path()))
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct DirtyScopeSet {
    latest_by_scope: BTreeMap<DirtyScope, u64>,
    normalized_identities: HashMap<(u8, String), DirtyScope>,
    covering_scopes: BTreeMap<String, DirtyScope>,
}

impl DirtyScopeSet {
    fn insert(&mut self, scope: DirtyScope, event_seq: u64) {
        let identity = scope.normalized_identity();
        if let Some(existing) = self.normalized_identities.get(&identity).cloned() {
            self.latest_by_scope
                .entry(existing)
                .and_modify(|latest| *latest = (*latest).max(event_seq));
            return;
        }

        // A RootRepair is the bounded, lossless replacement for every scope below a
        // configured root.  An older Subtree for the same root must not swallow it:
        // Subtree deliberately leaves destructive RemovedPrefix scopes independent,
        // while RootRepair absorbs them too.
        let allow_subtree_cover = matches!(
            scope,
            DirtyScope::DirectoryContents(_) | DirtyScope::Subtree(_)
        );
        if let Some(existing) = self.covering_scope(&identity.1, allow_subtree_cover) {
            self.latest_by_scope
                .entry(existing)
                .and_modify(|latest| *latest = (*latest).max(event_seq));
            return;
        }

        let mut latest = event_seq;
        if matches!(scope, DirtyScope::Subtree(_) | DirtyScope::RootRepair(_)) {
            let root_repair = matches!(scope, DirtyScope::RootRepair(_));
            let dominated = self
                .normalized_identities
                .iter()
                .filter(|((kind, key), _)| {
                    (root_repair || *kind != 2)
                        && (key == &identity.1
                            || key_is_under_any(key, std::slice::from_ref(&identity.1)))
                })
                .map(|(_, existing)| existing.clone())
                .collect::<Vec<_>>();
            for existing in dominated {
                if let Some(absorbed) = self.latest_by_scope.remove(&existing) {
                    latest = latest.max(absorbed);
                }
            }
            self.rebuild_indexes();
        }
        self.latest_by_scope.insert(scope.clone(), latest);
        self.normalized_identities
            .insert(identity.clone(), scope.clone());
        if matches!(scope, DirtyScope::Subtree(_) | DirtyScope::RootRepair(_)) {
            self.covering_scopes.insert(identity.1, scope);
        }
    }

    fn retain_after(&mut self, absorbed_through: u64) {
        self.latest_by_scope
            .retain(|_, latest| *latest > absorbed_through);
        self.rebuild_indexes();
    }

    fn merge(&mut self, other: Self) {
        for (scope, latest) in other.latest_by_scope {
            self.insert(scope, latest);
        }
    }

    fn is_empty(&self) -> bool {
        self.latest_by_scope.is_empty()
    }

    fn scopes(&self) -> impl Iterator<Item = &DirtyScope> {
        self.latest_by_scope.keys()
    }

    fn len(&self) -> usize {
        self.latest_by_scope.len()
    }

    fn covering_scope(&self, key: &str, allow_subtree: bool) -> Option<DirtyScope> {
        let mut candidate = Some(key);
        while let Some(current) = candidate {
            if let Some(scope) = self.covering_scopes.get(current)
                && (matches!(scope, DirtyScope::RootRepair(_))
                    || allow_subtree && matches!(scope, DirtyScope::Subtree(_)))
            {
                return Some(scope.clone());
            }
            candidate = normalized_parent(current);
        }
        None
    }

    fn coalesce_root(&mut self, root: PathBuf, event_seq: u64) {
        self.insert(DirtyScope::RootRepair(root), event_seq);
    }

    fn has_scope_under(&self, root: &Path) -> bool {
        let root_key = crate::search_index_db::normalize_path(root);
        self.normalized_identities
            .keys()
            .any(|(_, key)| key_is_under_any(key, std::slice::from_ref(&root_key)))
    }

    fn rebuild_indexes(&mut self) {
        self.normalized_identities.clear();
        self.covering_scopes.clear();
        for scope in self.latest_by_scope.keys() {
            let identity = scope.normalized_identity();
            self.normalized_identities
                .insert(identity.clone(), scope.clone());
            if matches!(scope, DirtyScope::Subtree(_) | DirtyScope::RootRepair(_)) {
                self.covering_scopes.insert(identity.1, scope.clone());
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ReconcileJobKind {
    Full(FullIntent),
    Delta,
    Purge,
}

#[derive(Clone, Debug)]
struct RunningReconcileJob {
    kind: ReconcileJobKind,
    config_epoch: u64,
    start_event_seq: u64,
    repairs_watch_gap: bool,
    cancel: Arc<AtomicBool>,
}

struct ReconcileJobPlan {
    running: RunningReconcileJob,
    config: SchedulerConfig,
    purge_roots: BTreeSet<PathBuf>,
    dirty: DirtyScopeSet,
}

#[derive(Debug)]
enum SchedulerPhase {
    Idle,
    Starting,
    Running(RunningReconcileJob),
    AwaitingArray(RunningReconcileJob),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SuccessfulJobDisposition {
    Complete,
    MoreWork,
    AwaitingWatch,
    DegradedWatch,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum InterruptedJobDisposition {
    RestartWorker,
    AwaitingWatch,
    Stopped,
}

#[derive(Debug, PartialEq, Eq)]
enum ArrayAckWaitError {
    Cancelled,
    Failed(String),
}

impl SchedulerPhase {
    fn is_worker_active(&self) -> bool {
        !matches!(self, Self::Idle)
    }

    fn cancel(&self) {
        let cancel = match self {
            Self::Running(job) | Self::AwaitingArray(job) => Some(&job.cancel),
            Self::Idle | Self::Starting => None,
        };
        if let Some(cancel) = cancel {
            let _ = cancel.compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire);
        }
    }
}

struct SchedulerState {
    desired_config: Option<SchedulerConfig>,
    config_epoch: u64,
    next_watch_generation: u64,
    next_gap_epoch: u64,
    next_event_seq: u64,
    watch_by_root: BTreeMap<Uuid, RootWatchState>,
    pending_full: Option<FullIntent>,
    dirty: DirtyScopeSet,
    pending_purge_roots: BTreeSet<PathBuf>,
    phase: SchedulerPhase,
    shutdown: bool,
}

impl Default for SchedulerState {
    fn default() -> Self {
        Self {
            desired_config: None,
            config_epoch: 0,
            next_watch_generation: 0,
            next_gap_epoch: 0,
            next_event_seq: 0,
            watch_by_root: BTreeMap::new(),
            pending_full: None,
            dirty: DirtyScopeSet::default(),
            pending_purge_roots: BTreeSet::new(),
            phase: SchedulerPhase::Idle,
            shutdown: false,
        }
    }
}

impl SchedulerState {
    fn merge_full_intent(&mut self, intent: FullIntent) {
        let should_replace = self.pending_full.is_none_or(|pending| {
            intent.config_epoch > pending.config_epoch
                || intent.config_epoch == pending.config_epoch
                    && intent.required_gap_epoch > pending.required_gap_epoch
        });
        if should_replace {
            self.pending_full = Some(intent);
        }
    }

    fn bound_dirty_scopes(&mut self, preferred_root: Option<PathBuf>) {
        if self.dirty.len() <= DIRTY_SCOPE_LIMIT {
            return;
        }
        let latest = self.next_event_seq;
        let mut roots = self
            .desired_config
            .as_ref()
            .map(|config| config.roots.clone())
            .unwrap_or_default();
        if let Some(preferred_root) = preferred_root {
            roots.sort_by_key(|root| (root.path != preferred_root, root.key.clone()));
        }
        for root in roots {
            if self.dirty.len() <= DIRTY_SCOPE_LIMIT {
                break;
            }
            if self.dirty.has_scope_under(&root.path) {
                self.dirty.coalesce_root(root.path, latest);
            }
        }
    }

    fn watches_are_terminal(&self) -> bool {
        let Some(config) = self.desired_config.as_ref() else {
            return true;
        };
        config.roots.iter().all(|root| {
            self.watch_by_root
                .get(&root.favorite_id)
                .is_some_and(|watch| watch.health != WatchHealth::Pending)
        })
    }

    fn has_unavailable_watch(&self) -> bool {
        self.watch_by_root
            .values()
            .any(|watch| watch.health == WatchHealth::Unavailable)
    }

    fn has_runnable_work(&self) -> bool {
        if self.shutdown || self.phase.is_worker_active() || self.desired_config.is_none() {
            return false;
        }
        if !self.pending_purge_roots.is_empty() {
            return true;
        }
        if self.pending_full.is_some() {
            return self.watches_are_terminal();
        }
        !self.dirty.is_empty()
    }

    fn reserve_worker_if_runnable(&mut self) -> bool {
        if self.has_runnable_work() {
            self.phase = SchedulerPhase::Starting;
            true
        } else {
            false
        }
    }

    fn take_next_job(&mut self) -> Option<ReconcileJobPlan> {
        if self.shutdown {
            return None;
        }
        let config = self.desired_config.clone()?;
        let cancel = Arc::new(AtomicBool::new(false));
        let start_event_seq = self.next_event_seq;
        let (kind, purge_roots, dirty) = if !self.pending_purge_roots.is_empty() {
            (
                ReconcileJobKind::Purge,
                std::mem::take(&mut self.pending_purge_roots),
                DirtyScopeSet::default(),
            )
        } else if self.pending_full.is_some() && self.watches_are_terminal() {
            let intent = self.pending_full.take().expect("pending full disappeared");
            (
                ReconcileJobKind::Full(intent),
                BTreeSet::new(),
                DirtyScopeSet::default(),
            )
        } else if self.pending_full.is_none() && !self.dirty.is_empty() {
            (
                ReconcileJobKind::Delta,
                BTreeSet::new(),
                std::mem::take(&mut self.dirty),
            )
        } else {
            self.phase = SchedulerPhase::Idle;
            return None;
        };
        let repairs_watch_gap = match kind {
            ReconcileJobKind::Full(intent) => self.watch_by_root.values().all(|watch| {
                watch.health == WatchHealth::Ready && watch.gap_epoch <= intent.required_gap_epoch
            }),
            ReconcileJobKind::Delta | ReconcileJobKind::Purge => false,
        };
        let running = RunningReconcileJob {
            kind,
            config_epoch: self.config_epoch,
            start_event_seq,
            repairs_watch_gap,
            cancel,
        };
        self.phase = SchedulerPhase::Running(running.clone());
        Some(ReconcileJobPlan {
            running,
            config,
            purge_roots,
            dirty,
        })
    }

    fn restore_unfinished_job(&mut self, plan: &ReconcileJobPlan, purge_committed: bool) {
        if !purge_committed {
            self.pending_purge_roots
                .extend(plan.purge_roots.iter().cloned());
        }
        if self.config_epoch != plan.running.config_epoch {
            return;
        }
        match plan.running.kind {
            ReconcileJobKind::Full(intent) => self.merge_full_intent(intent),
            ReconcileJobKind::Delta => {
                self.dirty.merge(plan.dirty.clone());
                self.bound_dirty_scopes(None);
            }
            ReconcileJobKind::Purge => {}
        }
    }

    fn finish_successful_job(&mut self, plan: &ReconcileJobPlan) -> SuccessfulJobDisposition {
        if let ReconcileJobKind::Full(intent) = plan.running.kind {
            self.dirty.retain_after(plan.running.start_event_seq);
            for watch in self.watch_by_root.values_mut() {
                if plan.running.repairs_watch_gap && watch.gap_epoch <= intent.required_gap_epoch {
                    watch.repaired_gap_epoch = watch.gap_epoch;
                }
            }
        }
        if self.pending_full.is_some() && !self.watches_are_terminal() {
            SuccessfulJobDisposition::AwaitingWatch
        } else if !self.pending_purge_roots.is_empty()
            || self.pending_full.is_some()
            || !self.dirty.is_empty()
        {
            SuccessfulJobDisposition::MoreWork
        } else if self.has_unavailable_watch() {
            SuccessfulJobDisposition::DegradedWatch
        } else {
            SuccessfulJobDisposition::Complete
        }
    }

    fn settle_interrupted_job(
        &mut self,
        plan: &ReconcileJobPlan,
        purge_committed: bool,
    ) -> InterruptedJobDisposition {
        self.restore_unfinished_job(plan, purge_committed);
        self.phase = SchedulerPhase::Idle;
        if self.shutdown {
            InterruptedJobDisposition::Stopped
        } else if self.has_runnable_work() {
            self.phase = SchedulerPhase::Starting;
            InterruptedJobDisposition::RestartWorker
        } else if self.pending_full.is_some() && !self.watches_are_terminal() {
            InterruptedJobDisposition::AwaitingWatch
        } else {
            InterruptedJobDisposition::Stopped
        }
    }
}

#[derive(Default)]
struct ArrayUpdateState {
    requested: bool,
    running: bool,
    required_through: Option<crate::similar_db::StoreWatermark>,
    last_error: Option<String>,
}

struct SimilarIndexScheduler {
    data_dir: PathBuf,
    progress: Arc<Mutex<IndexProgress>>,
    memory: Arc<Mutex<MemoryState>>,
    summary: Arc<Mutex<SummaryState>>,
    memory_epoch: Arc<AtomicU64>,
    item_query: Arc<Mutex<ItemQueryCache>>,
    book_query: BookQueryExecutor<Arc<BookQuery>>,
    #[cfg(test)]
    book_query_phase_probe:
        Mutex<Option<Arc<crate::similar_book_query_test_probe::BookQueryTestPhaseProbe>>>,
    known_book_store: Mutex<ObservedBookStore>,
    prefill_db: Arc<Mutex<Option<Arc<SimilarDb>>>>,
    enabled_roots: Arc<RwLock<Vec<String>>>,
    state: Mutex<SchedulerState>,
    array_update: Mutex<ArrayUpdateState>,
    array_changed: Condvar,
    compaction_running: AtomicBool,
    #[cfg(test)]
    full_jobs_started: AtomicU64,
    #[cfg(test)]
    delta_jobs_started: AtomicU64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ObservedBookStore {
    Unobserved,
    Known([u8; 16]),
}

enum BookEngineSlot {
    Vacant,
    Ready(SimilarBookQueryEngine),
}

struct SchedulerBookQueryRuntime {
    scheduler: Weak<SimilarIndexScheduler>,
    engine: BookEngineSlot,
}

impl BookQueryRuntime<Arc<BookQuery>> for SchedulerBookQueryRuntime {
    fn execute(
        &mut self,
        request: BookQueryRequest,
        cancel: Arc<AtomicBool>,
    ) -> BookQueryTerminal<Arc<BookQuery>> {
        let perf_started = crate::perf::is_enabled().then(std::time::Instant::now);
        let cpu_started = perf_started.and_then(|_| current_thread_cpu_100ns());
        let trace_id = perf_started.map_or(0, |_| next_similar_query_trace_id());
        if perf_started.is_some() {
            crate::perf::event(
                "similar_book",
                "start",
                Some(request.container_key()),
                trace_id,
                &[(
                    "priority",
                    serde_json::Value::from(current_thread_priority()),
                )],
            );
        }
        let terminal = self.execute_inner(&request, Arc::clone(&cancel));
        if let Some(started) = perf_started {
            let (hit_count, override_count) = book_query_result_counts(&terminal);
            let cancelled = cancel.load(Ordering::Acquire);
            let cpu_ms = cpu_started
                .zip(current_thread_cpu_100ns())
                .map(|(before, after)| after.saturating_sub(before) as f64 / 10_000.0);
            crate::perf::event(
                "similar_book",
                "end",
                Some(request.container_key()),
                trace_id,
                &[
                    (
                        "wall_ms",
                        serde_json::Value::from(started.elapsed().as_secs_f64() * 1000.0),
                    ),
                    (
                        "thread_cpu_ms",
                        cpu_ms.map_or(serde_json::Value::Null, serde_json::Value::from),
                    ),
                    (
                        "priority",
                        serde_json::Value::from(current_thread_priority()),
                    ),
                    ("cancelled", serde_json::Value::from(cancelled)),
                    ("active", serde_json::Value::from(!cancelled)),
                    ("stale", serde_json::Value::from(cancelled)),
                    (
                        "terminal",
                        serde_json::Value::from(book_query_terminal_label(&terminal)),
                    ),
                    ("hit_count", serde_json::Value::from(hit_count as u64)),
                    (
                        "override_count",
                        serde_json::Value::from(override_count as u64),
                    ),
                ],
            );
        }
        terminal
    }
}

impl SchedulerBookQueryRuntime {
    fn execute_inner(
        &mut self,
        request: &BookQueryRequest,
        cancel: Arc<AtomicBool>,
    ) -> BookQueryTerminal<Arc<BookQuery>> {
        let Some(scheduler) = self.scheduler.upgrade() else {
            return BookQueryTerminal::Failed(
                "similar book query scheduler is unavailable".to_owned(),
            );
        };
        let shared_snapshot = {
            let memory = scheduler.memory.lock().unwrap_or_else(|e| e.into_inner());
            match &*memory {
                MemoryState::Ready(snapshot) => Some(Arc::clone(snapshot)),
                MemoryState::Unloaded
                | MemoryState::Loading
                | MemoryState::Missing
                | MemoryState::Failed(_) => None,
            }
        };
        let roots = scheduler
            .enabled_roots
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        let db_path = SimilarDb::db_path_at(&scheduler.data_dir);

        if matches!(self.engine, BookEngineSlot::Vacant) {
            match SimilarBookQueryEngine::open_at(&db_path) {
                Ok(Some(engine)) => self.engine = BookEngineSlot::Ready(engine),
                Ok(None) => {
                    return BookQueryTerminal::Ready(Arc::new(BookQuery::NotIndexed));
                }
                Err(error) => {
                    return BookQueryTerminal::Ready(Arc::new(BookQuery::Failed(format!(
                        "similar book query database open failed: {error}"
                    ))));
                }
            }
        }

        let BookEngineSlot::Ready(engine) = &mut self.engine else {
            unreachable!("book query engine slot did not become ready")
        };
        #[cfg(test)]
        if let Some(probe) = scheduler
            .book_query_phase_probe
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone()
        {
            engine.set_test_phase_probe(probe);
        }
        let candidates = shared_snapshot.into_iter().collect::<Vec<_>>();
        let result = engine.query(request.container_key(), &roots, &candidates, cancel);
        match result {
            Ok(observation) => {
                scheduler.observe_book_store(observation.metadata.store_id);
                match observation.outcome {
                    Ok(query) => BookQueryTerminal::Ready(Arc::new(query)),
                    Err(error) => BookQueryTerminal::Ready(Arc::new(BookQuery::Failed(
                        book_engine_error(error),
                    ))),
                }
            }
            Err(error) => BookQueryTerminal::Ready(Arc::new(BookQuery::Failed(format!(
                "similar book query database read failed: {error}"
            )))),
        }
    }
}

fn item_query_terminal_label(query: &ItemQuery) -> &'static str {
    match query {
        ItemQuery::Preparing => "preparing",
        ItemQuery::Ready(_) => "ready",
        ItemQuery::Featureless => "featureless",
        ItemQuery::NotIndexed => "not_indexed",
        ItemQuery::NoIndex => "no_index",
        ItemQuery::Failed(_) => "failed",
    }
}

fn book_query_terminal_label(terminal: &BookQueryTerminal<Arc<BookQuery>>) -> &'static str {
    match terminal {
        BookQueryTerminal::Ready(query) => match query.as_ref() {
            BookQuery::Preparing => "preparing",
            BookQuery::Ready(_) => "ready",
            BookQuery::Featureless => "featureless",
            BookQuery::NotIndexed => "not_indexed",
            BookQuery::NotBook => "not_book",
            BookQuery::Failed(_) => "failed",
        },
        BookQueryTerminal::Failed(_) => "executor_failed",
    }
}

fn book_query_result_counts(terminal: &BookQueryTerminal<Arc<BookQuery>>) -> (usize, usize) {
    let BookQueryTerminal::Ready(query) = terminal else {
        return (0, 0);
    };
    let BookQuery::Ready(relations) = query.as_ref() else {
        return (0, 0);
    };
    (
        relations.hits.len(),
        relations.hits.iter().map(|hit| hit.overrides().len()).sum(),
    )
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
    /// Reserve one watcher generation before its thread starts.  Every terminal and event call
    /// must carry the returned token; a late supervisor can then never mutate a reconfigured root.
    pub fn begin_watch(&self, favorite_id: Uuid, root: &Path) -> Option<SimilarWatchRegistration> {
        self.scheduler.upgrade()?.begin_watch(favorite_id, root)
    }

    pub fn watch_ready(&self, registration: &SimilarWatchRegistration) {
        if let Some(scheduler) = self.scheduler.upgrade() {
            scheduler.set_watch_health(registration, WatchHealth::Ready, None);
        }
    }

    pub fn watch_unavailable(
        &self,
        registration: &SimilarWatchRegistration,
        reason: impl Into<String>,
    ) {
        if let Some(scheduler) = self.scheduler.upgrade() {
            scheduler.set_watch_health(registration, WatchHealth::Unavailable, Some(reason.into()));
        }
    }

    pub fn request_change(
        &self,
        registration: &SimilarWatchRegistration,
        path: PathBuf,
        kind: crate::search_watcher::ChangeKind,
    ) {
        if let Some(scheduler) = self.scheduler.upgrade() {
            scheduler.request_change(registration, path, kind);
        }
    }

    pub fn request_full(&self, registration: &SimilarWatchRegistration) {
        if let Some(scheduler) = self.scheduler.upgrade() {
            scheduler.request_full(registration, FullReason::Manual);
        }
    }

    pub fn request_overflow(&self, registration: &SimilarWatchRegistration) {
        if let Some(scheduler) = self.scheduler.upgrade() {
            scheduler.request_full(registration, FullReason::Overflow);
        }
    }

    /// Startup can fail before any supervisor is constructed.  Convert every still-pending root
    /// into a finite terminal so the initial filesystem snapshot can run, while keeping progress
    /// explicitly degraded rather than claiming a reliable watcher.
    pub fn finish_watch_bootstrap(&self) {
        if let Some(scheduler) = self.scheduler.upgrade() {
            scheduler.finish_watch_bootstrap();
        }
    }
}

impl SimilarIndexScheduler {
    fn book_query_dispatch_decision(&self) -> BookQueryDispatchDecision<Arc<BookQuery>> {
        let (shutdown, worker_running) = {
            let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            (state.shutdown, state.phase.is_worker_active())
        };
        if shutdown {
            return BookQueryDispatchDecision::Complete(BookQueryTerminal::Failed(
                "similar book query scheduler stopped".to_owned(),
            ));
        }
        let memory = self.memory.lock().unwrap_or_else(|e| e.into_inner());
        match &*memory {
            MemoryState::Loading => BookQueryDispatchDecision::Wait,
            MemoryState::Unloaded if worker_running => BookQueryDispatchDecision::Wait,
            MemoryState::Unloaded
            | MemoryState::Missing
            | MemoryState::Failed(_)
            | MemoryState::Ready(_) => BookQueryDispatchDecision::Run,
        }
    }

    fn worker_is_running(&self) -> bool {
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .phase
            .is_worker_active()
    }

    fn observe_book_store(&self, store_id: [u8; 16]) {
        let changed = {
            let mut observed = self
                .known_book_store
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            match *observed {
                ObservedBookStore::Unobserved => {
                    *observed = ObservedBookStore::Known(store_id);
                    false
                }
                ObservedBookStore::Known(current) if current == store_id => false,
                ObservedBookStore::Known(_) => {
                    *observed = ObservedBookStore::Known(store_id);
                    true
                }
            }
        };
        if changed {
            self.book_query.hard_invalidate();
        }
    }

    fn configure(
        self: &Arc<Self>,
        mut roots: Vec<ConfiguredRoot>,
        pdf_passwords: crate::pdf_passwords::PdfPasswordStore,
        activity_gate: Option<Arc<crate::activity_gate::ActivityGate>>,
        mut excluded_roots: Vec<PathBuf>,
    ) {
        roots.sort_by(|left, right| {
            left.key
                .cmp(&right.key)
                .then_with(|| left.favorite_id.cmp(&right.favorite_id))
        });
        roots.dedup_by(|left, right| left.favorite_id == right.favorite_id);
        excluded_roots.sort();
        excluded_roots.dedup();
        let mut normalized = roots
            .iter()
            .map(|root| root.key.clone())
            .collect::<Vec<_>>();
        normalized.sort();
        normalized.dedup();
        let mut excluded_root_keys = excluded_roots
            .iter()
            .map(|root| crate::search_index_db::normalize_path(root))
            .collect::<Vec<_>>();
        excluded_root_keys.sort();
        excluded_root_keys.dedup();
        *self
            .enabled_roots
            .write()
            .unwrap_or_else(|e| e.into_inner()) = normalized;

        let password_revision = pdf_passwords.configuration_revision();
        let should_start = {
            let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            if state.shutdown {
                return;
            }
            let previous = state.desired_config.clone();
            let roots_changed = previous.as_ref().is_none_or(|config| {
                config.roots != roots || config.excluded_roots != excluded_roots
            });
            let password_changed = previous
                .as_ref()
                .is_none_or(|config| config.password_revision != password_revision);
            let previous_root_paths = previous
                .as_ref()
                .map(|config| {
                    config
                        .roots
                        .iter()
                        .map(|root| root.path.clone())
                        .collect::<BTreeSet<_>>()
                })
                .unwrap_or_default();
            let next_root_paths = roots
                .iter()
                .map(|root| root.path.clone())
                .collect::<BTreeSet<_>>();
            state.desired_config = Some(SchedulerConfig {
                roots,
                excluded_roots,
                excluded_root_keys,
                pdf_passwords,
                password_revision,
                activity_gate,
            });
            if !roots_changed && !password_changed {
                return;
            }
            state.config_epoch = state.config_epoch.wrapping_add(1).max(1);
            let config_epoch = state.config_epoch;
            state
                .pending_purge_roots
                .extend(previous_root_paths.difference(&next_root_paths).cloned());
            if roots_changed {
                let old_watches = std::mem::take(&mut state.watch_by_root);
                let desired_roots = state
                    .desired_config
                    .as_ref()
                    .map(|config| config.roots.clone())
                    .unwrap_or_default();
                for root in desired_roots {
                    let retained = old_watches
                        .get(&root.favorite_id)
                        .filter(|watch| watch.root_key == root.key);
                    let watch = if let Some(retained) = retained {
                        retained.clone()
                    } else {
                        RootWatchState {
                            root_key: root.key,
                            // Generation zero is the unclaimed configuration placeholder. The
                            // first supervisor may claim it without creating a watch gap; every
                            // later registration replaces a real lifecycle and must repair one.
                            registration_generation: 0,
                            health: WatchHealth::Pending,
                            gap_epoch: 0,
                            repaired_gap_epoch: 0,
                        }
                    };
                    state.watch_by_root.insert(root.favorite_id, watch);
                }
            }
            let reason = if previous.is_none() {
                FullReason::Initial
            } else {
                FullReason::Reconfigure
            };
            if !next_root_paths.is_empty() || !previous_root_paths.is_empty() {
                let required_gap_epoch = state.next_gap_epoch;
                state.merge_full_intent(FullIntent {
                    config_epoch,
                    required_gap_epoch,
                    reason,
                });
            }
            state.phase.cancel();
            state.reserve_worker_if_runnable()
        };
        self.retain_loaded_snapshot_during_run();
        self.book_query.hard_invalidate();
        if should_start {
            self.spawn_worker();
        }
    }

    fn begin_watch(
        self: &Arc<Self>,
        favorite_id: Uuid,
        root: &Path,
    ) -> Option<SimilarWatchRegistration> {
        let root_key = crate::search_index_db::normalize_path(root);
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if state.shutdown
            || !state
                .desired_config
                .as_ref()?
                .roots
                .iter()
                .any(|configured| {
                    configured.favorite_id == favorite_id && configured.key == root_key
                })
        {
            return None;
        }
        let previous = state.watch_by_root.get(&favorite_id).cloned()?;
        let replacement = previous.registration_generation != 0;
        if replacement {
            state.next_gap_epoch = state.next_gap_epoch.wrapping_add(1).max(1);
        }
        let gap_epoch = if replacement {
            state.next_gap_epoch
        } else {
            previous.gap_epoch
        };
        let repaired_gap_epoch = previous.repaired_gap_epoch;
        if replacement {
            let config_epoch = state.config_epoch;
            state.merge_full_intent(FullIntent {
                config_epoch,
                required_gap_epoch: gap_epoch,
                reason: FullReason::WatchRecovery,
            });
            state.phase.cancel();
        }
        state.next_watch_generation = state.next_watch_generation.wrapping_add(1).max(1);
        let generation = state.next_watch_generation;
        state.watch_by_root.insert(
            favorite_id,
            RootWatchState {
                root_key: root_key.clone(),
                registration_generation: generation,
                health: WatchHealth::Pending,
                gap_epoch,
                repaired_gap_epoch,
            },
        );
        let registration = SimilarWatchRegistration {
            favorite_id,
            root_key,
            registration_generation: generation,
        };
        drop(state);
        if replacement {
            self.mark_awaiting_watch_if_terminal();
        }
        Some(registration)
    }

    fn mark_awaiting_watch_if_terminal(&self) {
        let mut progress = self.progress.lock().unwrap_or_else(|e| e.into_inner());
        let report = match &*progress {
            IndexProgress::Complete(report)
            | IndexProgress::Cancelled(report)
            | IndexProgress::AwaitingWatch(report)
            | IndexProgress::AwaitingArray(report) => Some(report.clone()),
            IndexProgress::Degraded { report, .. } => Some(report.clone()),
            IndexProgress::Idle | IndexProgress::Running(_) | IndexProgress::Failed(_) => None,
        };
        if let Some(report) = report {
            *progress = IndexProgress::AwaitingWatch(report);
        }
    }

    fn registration_is_current(
        state: &SchedulerState,
        registration: &SimilarWatchRegistration,
    ) -> bool {
        state
            .watch_by_root
            .get(&registration.favorite_id)
            .is_some_and(|watch| {
                watch.root_key == registration.root_key
                    && watch.registration_generation == registration.registration_generation
            })
    }

    fn set_watch_health(
        self: &Arc<Self>,
        registration: &SimilarWatchRegistration,
        health: WatchHealth,
        reason: Option<String>,
    ) {
        let should_start = {
            let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            if state.shutdown || !Self::registration_is_current(&state, registration) {
                return;
            }
            let previous = state
                .watch_by_root
                .get(&registration.favorite_id)
                .map(|watch| watch.health)
                .expect("validated watch disappeared");
            if previous == health {
                return;
            }
            if health == WatchHealth::Unavailable {
                let (gap_epoch, repaired_gap_epoch) = state
                    .watch_by_root
                    .get(&registration.favorite_id)
                    .map(|watch| (watch.gap_epoch, watch.repaired_gap_epoch))
                    .expect("validated watch disappeared");
                let gap_epoch = if gap_epoch <= repaired_gap_epoch {
                    state.next_gap_epoch = state.next_gap_epoch.wrapping_add(1).max(1);
                    state.next_gap_epoch
                } else {
                    gap_epoch
                };
                if let Some(watch) = state.watch_by_root.get_mut(&registration.favorite_id) {
                    watch.health = health;
                    watch.gap_epoch = gap_epoch;
                }
                let config_epoch = state.config_epoch;
                state.merge_full_intent(FullIntent {
                    config_epoch,
                    required_gap_epoch: gap_epoch,
                    reason: FullReason::WatchRecovery,
                });
                if let Some(reason) = reason {
                    crate::logger::log(format!(
                        "similar watcher unavailable favorite={} root={} reason={reason}",
                        registration.favorite_id, registration.root_key
                    ));
                }
            } else {
                let recovering_gap = state
                    .watch_by_root
                    .get(&registration.favorite_id)
                    .filter(|watch| watch.gap_epoch > watch.repaired_gap_epoch)
                    .map_or(0, |watch| watch.gap_epoch);
                if let Some(watch) = state.watch_by_root.get_mut(&registration.favorite_id) {
                    watch.health = health;
                }
                if recovering_gap > 0 {
                    let config_epoch = state.config_epoch;
                    state.merge_full_intent(FullIntent {
                        config_epoch,
                        required_gap_epoch: recovering_gap,
                        reason: FullReason::WatchRecovery,
                    });
                }
            }
            state.reserve_worker_if_runnable()
        };
        if should_start {
            self.spawn_worker();
        }
    }

    fn finish_watch_bootstrap(self: &Arc<Self>) {
        let registrations = {
            let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            state
                .watch_by_root
                .iter()
                .filter(|(_, watch)| watch.health == WatchHealth::Pending)
                .map(|(favorite_id, watch)| SimilarWatchRegistration {
                    favorite_id: *favorite_id,
                    root_key: watch.root_key.clone(),
                    registration_generation: watch.registration_generation,
                })
                .collect::<Vec<_>>()
        };
        for registration in registrations {
            self.set_watch_health(
                &registration,
                WatchHealth::Unavailable,
                Some("watch supervisor was not constructed".to_owned()),
            );
        }
    }

    fn request_change(
        self: &Arc<Self>,
        registration: &SimilarWatchRegistration,
        path: PathBuf,
        kind: crate::search_watcher::ChangeKind,
    ) {
        {
            let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            if state.shutdown
                || !Self::registration_is_current(&state, registration)
                || state
                    .watch_by_root
                    .get(&registration.favorite_id)
                    .is_none_or(|watch| watch.health != WatchHealth::Ready)
            {
                return;
            }
        }
        // Filesystem metadata may block on a sleeping disk or UNC share. Observe it before the
        // coordinator lock so UI-side progress/configuration reads never wait behind filesystem
        // I/O. The worker validates existence again before any destructive publication.
        let upsert_observation = (kind == crate::search_watcher::ChangeKind::Upsert).then(|| {
            match std::fs::metadata(&path) {
                Ok(metadata) if metadata.is_dir() => ChangedPathObservation::Directory,
                Ok(_) => ChangedPathObservation::File,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    ChangedPathObservation::Missing
                }
                Err(_) => ChangedPathObservation::Unknown,
            }
        });
        self.request_change_observed(registration, path, kind, upsert_observation);
    }

    fn request_change_observed(
        self: &Arc<Self>,
        registration: &SimilarWatchRegistration,
        path: PathBuf,
        kind: crate::search_watcher::ChangeKind,
        upsert_observation: Option<ChangedPathObservation>,
    ) {
        let path_key = crate::search_index_db::normalize_path(&path);
        let should_start = {
            let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            if state.shutdown || !Self::registration_is_current(&state, registration) {
                return;
            }
            if state
                .watch_by_root
                .get(&registration.favorite_id)
                .is_none_or(|watch| watch.health != WatchHealth::Ready)
            {
                return;
            }
            let Some(config) = state.desired_config.as_ref() else {
                return;
            };
            if !key_is_under_any(&path_key, std::slice::from_ref(&registration.root_key))
                || key_is_under_any(&path_key, &config.excluded_root_keys)
            {
                return;
            }
            let dirty_root = config
                .roots
                .iter()
                .find(|root| root.favorite_id == registration.favorite_id)
                .map(|root| root.path.clone());
            state.next_event_seq = state.next_event_seq.wrapping_add(1).max(1);
            let event_seq = state.next_event_seq;
            if let Some(parent) = path.parent() {
                state.dirty.insert(
                    DirtyScope::DirectoryContents(parent.to_path_buf()),
                    event_seq,
                );
            }
            match (kind, upsert_observation) {
                (
                    crate::search_watcher::ChangeKind::Upsert,
                    Some(ChangedPathObservation::Directory),
                ) => {
                    state.dirty.insert(DirtyScope::Subtree(path), event_seq);
                }
                (
                    crate::search_watcher::ChangeKind::Upsert,
                    Some(ChangedPathObservation::Missing),
                ) => {
                    // Windows RenameMode::Both is delivered as two Upserts by the shared
                    // watcher. The old side has disappeared by this boundary and therefore
                    // owns a destructive prefix observation as well as its parent's contents.
                    state
                        .dirty
                        .insert(DirtyScope::RemovedPrefix(path), event_seq);
                }
                (crate::search_watcher::ChangeKind::Upsert, Some(ChangedPathObservation::File)) => {
                    // The parent contents scope performs the observation. A permission or
                    // transient metadata failure must never be reclassified as a deletion.
                }
                (
                    crate::search_watcher::ChangeKind::Upsert,
                    Some(ChangedPathObservation::Unknown),
                ) => {
                    // The worker retries the observation recursively. It must not reduce an
                    // unknown directory to its parent's non-recursive contents.
                    state.dirty.insert(DirtyScope::Subtree(path), event_seq);
                }
                (crate::search_watcher::ChangeKind::Upsert, None) => {
                    debug_assert!(false, "Upsert requires a filesystem observation");
                }
                (crate::search_watcher::ChangeKind::Remove, None) => {
                    state
                        .dirty
                        .insert(DirtyScope::RemovedPrefix(path), event_seq);
                }
                (crate::search_watcher::ChangeKind::Remove, Some(_)) => {
                    debug_assert!(false, "Remove unexpectedly observed as Upsert");
                }
            }
            state.bound_dirty_scopes(dirty_root);
            state.reserve_worker_if_runnable()
        };
        if should_start {
            self.spawn_worker();
        }
    }

    fn request_full(self: &Arc<Self>, registration: &SimilarWatchRegistration, reason: FullReason) {
        let should_start = {
            let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            if state.shutdown || !Self::registration_is_current(&state, registration) {
                return;
            }
            if reason == FullReason::Overflow {
                state.next_gap_epoch = state.next_gap_epoch.wrapping_add(1).max(1);
                let gap_epoch = state.next_gap_epoch;
                if let Some(watch) = state.watch_by_root.get_mut(&registration.favorite_id) {
                    watch.gap_epoch = gap_epoch;
                }
            }
            let config_epoch = state.config_epoch;
            let required_gap_epoch = state.next_gap_epoch;
            state.merge_full_intent(FullIntent {
                config_epoch,
                required_gap_epoch,
                reason,
            });
            state.phase.cancel();
            state.reserve_worker_if_runnable()
        };
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
            {
                self.state.lock().unwrap_or_else(|e| e.into_inner()).phase = SchedulerPhase::Idle;
            }
            *self.progress.lock().unwrap_or_else(|e| e.into_inner()) =
                IndexProgress::Failed(format!("similar index worker start failed: {error}"));
            self.book_query.soft_refresh();
        }
    }

    fn worker_loop(self: Arc<Self>) {
        let db = match self.db_for_worker() {
            Ok(db) => db,
            Err(error) => {
                self.finish_worker(IndexProgress::Failed(format!(
                    "similar.db open failed: {error}"
                )));
                return;
            }
        };
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
            let Some(plan) = self
                .state
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .take_next_job()
            else {
                return;
            };
            if !self.job_is_current(&plan.running) {
                self.restore_unfinished_plan(&plan, false);
                if !self.continue_worker_if_runnable() {
                    return;
                }
                continue;
            }
            #[cfg(test)]
            match plan.running.kind {
                ReconcileJobKind::Full(_) => {
                    self.full_jobs_started.fetch_add(1, Ordering::AcqRel);
                }
                ReconcileJobKind::Delta => {
                    self.delta_jobs_started.fetch_add(1, Ordering::AcqRel);
                }
                ReconcileJobKind::Purge => {}
            }
            *self.progress.lock().unwrap_or_else(|e| e.into_inner()) =
                IndexProgress::Running(RunningProgress {
                    stage: IndexStage::Opening,
                    current_path: None,
                    report: IndexReport::default(),
                });
            let keep_roots = plan.config.normalized_roots();
            let purge_roots = plan
                .purge_roots
                .iter()
                .map(|root| crate::search_index_db::normalize_path(root))
                .collect::<Vec<_>>();
            let purge_result = db
                .purge_roots_except_if(&purge_roots, &keep_roots, || {
                    self.job_is_current(&plan.running)
                })
                .map_err(|error| format!("disabled favorite purge failed: {error}"));
            let purge_committed = matches!(
                &purge_result,
                Ok(crate::similar_db::ConditionalCommit::Committed(_))
            );
            let outcome = purge_result.and_then(|purge| {
                let purged = match purge {
                    crate::similar_db::ConditionalCommit::Committed(removed) => removed,
                    crate::similar_db::ConditionalCommit::Skipped => {
                        return Ok(ScanJobOutcome {
                            report: IndexReport::default(),
                            prune_safe: false,
                        });
                    }
                };
                let array_refresh = ArrayRefreshNotifier {
                    scheduler: Arc::downgrade(&self),
                };
                let scan = match plan.running.kind {
                    ReconcileJobKind::Full(_) => run_index_job(
                        &db,
                        &plan.config.scan_roots(),
                        &plan.config.excluded_root_keys,
                        &plan.config.pdf_passwords,
                        plan.config.activity_gate.as_deref(),
                        &plan.running.cancel,
                        &self.progress,
                        &array_refresh,
                    ),
                    ReconcileJobKind::Delta => run_delta_index_job(
                        &db,
                        &plan.dirty,
                        &plan.config,
                        &plan.running.cancel,
                        &self.progress,
                        &array_refresh,
                    ),
                    ReconcileJobKind::Purge => Ok(ScanJobOutcome {
                        report: IndexReport::default(),
                        prune_safe: true,
                    }),
                }?;
                let mut report = scan.report;
                report.removed = report.removed.saturating_add(purged as u64);
                Ok(ScanJobOutcome {
                    report,
                    prune_safe: scan.prune_safe,
                })
            });

            let cancelled = plan.running.cancel.load(Ordering::Acquire);
            let current_epoch = self
                .state
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .config_epoch;
            let stale = current_epoch != plan.running.config_epoch;
            if cancelled || stale {
                let report = outcome
                    .as_ref()
                    .map(|outcome| outcome.report.clone())
                    .unwrap_or_default();
                match self.settle_interrupted_plan(&plan, purge_committed, report) {
                    InterruptedJobDisposition::RestartWorker => continue,
                    InterruptedJobDisposition::AwaitingWatch
                    | InterruptedJobDisposition::Stopped => {
                        self.book_query.soft_refresh();
                        return;
                    }
                }
            }

            let outcome = match outcome {
                Ok(outcome) if outcome.prune_safe => outcome,
                Ok(outcome) => {
                    self.restore_unfinished_plan(&plan, purge_committed);
                    *self.progress.lock().unwrap_or_else(|e| e.into_inner()) =
                        IndexProgress::Degraded {
                            report: outcome.report,
                            reason: IndexDegradedReason::FilesystemObservationIncomplete,
                        };
                    self.stop_worker();
                    self.book_query.soft_refresh();
                    return;
                }
                Err(error) => {
                    self.restore_unfinished_plan(&plan, purge_committed);
                    *self.progress.lock().unwrap_or_else(|e| e.into_inner()) =
                        IndexProgress::Failed(error);
                    self.stop_worker();
                    self.book_query.soft_refresh();
                    return;
                }
            };

            let watermark = match db.change_watermark() {
                Ok(watermark) => watermark,
                Err(error) => {
                    self.restore_unfinished_plan(&plan, purge_committed);
                    *self.progress.lock().unwrap_or_else(|e| e.into_inner()) =
                        IndexProgress::Failed(format!(
                            "similar index publication watermark failed: {error}"
                        ));
                    self.stop_worker();
                    return;
                }
            };
            {
                let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
                if state.shutdown {
                    return;
                }
                state.phase = SchedulerPhase::AwaitingArray(plan.running.clone());
            }
            *self.progress.lock().unwrap_or_else(|e| e.into_inner()) =
                IndexProgress::AwaitingArray(outcome.report.clone());
            self.request_array_refresh_through(watermark);
            if let Err(error) = self.wait_for_array_ack(watermark, &plan.running.cancel) {
                match error {
                    ArrayAckWaitError::Cancelled => {
                        match self.settle_interrupted_plan(&plan, purge_committed, outcome.report) {
                            InterruptedJobDisposition::RestartWorker => continue,
                            InterruptedJobDisposition::AwaitingWatch
                            | InterruptedJobDisposition::Stopped => {
                                self.book_query.soft_refresh();
                                return;
                            }
                        }
                    }
                    ArrayAckWaitError::Failed(error) => {
                        self.restore_unfinished_plan(&plan, purge_committed);
                        *self.progress.lock().unwrap_or_else(|e| e.into_inner()) =
                            IndexProgress::Degraded {
                                report: outcome.report,
                                reason: IndexDegradedReason::ArrayPublication(error),
                            };
                        self.stop_worker();
                        self.book_query.soft_refresh();
                        return;
                    }
                }
            }

            if !self.job_is_current(&plan.running) {
                match self.settle_interrupted_plan(&plan, purge_committed, outcome.report) {
                    InterruptedJobDisposition::RestartWorker => continue,
                    InterruptedJobDisposition::AwaitingWatch
                    | InterruptedJobDisposition::Stopped => {
                        self.book_query.soft_refresh();
                        return;
                    }
                }
            }

            let disposition = self.finish_successful_plan(&plan);
            *self.summary.lock().unwrap_or_else(|e| e.into_inner()) = SummaryState::Unloaded;
            *self.progress.lock().unwrap_or_else(|e| e.into_inner()) = match disposition {
                SuccessfulJobDisposition::DegradedWatch => IndexProgress::Degraded {
                    report: outcome.report,
                    reason: IndexDegradedReason::WatchUnavailable,
                },
                SuccessfulJobDisposition::AwaitingWatch => {
                    IndexProgress::AwaitingWatch(outcome.report)
                }
                SuccessfulJobDisposition::MoreWork => IndexProgress::Running(RunningProgress {
                    stage: IndexStage::Opening,
                    current_path: None,
                    report: outcome.report,
                }),
                SuccessfulJobDisposition::Complete => IndexProgress::Complete(outcome.report),
            };
            self.book_query.soft_refresh();
            if !self.continue_worker_if_runnable() {
                return;
            }
        }
    }

    fn job_is_current(&self, job: &RunningReconcileJob) -> bool {
        let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        !state.shutdown
            && state.config_epoch == job.config_epoch
            && !job.cancel.load(Ordering::Acquire)
    }

    fn restore_unfinished_plan(&self, plan: &ReconcileJobPlan, purge_committed: bool) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.restore_unfinished_job(plan, purge_committed);
    }

    fn finish_successful_plan(&self, plan: &ReconcileJobPlan) -> SuccessfulJobDisposition {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.finish_successful_job(plan)
    }

    fn settle_interrupted_plan(
        &self,
        plan: &ReconcileJobPlan,
        purge_committed: bool,
        report: IndexReport,
    ) -> InterruptedJobDisposition {
        let disposition = self
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .settle_interrupted_job(plan, purge_committed);
        *self.progress.lock().unwrap_or_else(|e| e.into_inner()) = match disposition {
            InterruptedJobDisposition::RestartWorker => IndexProgress::Running(RunningProgress {
                stage: IndexStage::Opening,
                current_path: None,
                report,
            }),
            InterruptedJobDisposition::AwaitingWatch => IndexProgress::AwaitingWatch(report),
            InterruptedJobDisposition::Stopped => IndexProgress::Cancelled(report),
        };
        disposition
    }

    fn continue_worker_if_runnable(&self) -> bool {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.phase = SchedulerPhase::Idle;
        if state.has_runnable_work() {
            state.phase = SchedulerPhase::Starting;
            true
        } else {
            false
        }
    }

    fn stop_worker(&self) {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).phase = SchedulerPhase::Idle;
    }

    /// Index schedulerが存続する間、scan/purgeとthumbnail prefillは同じSQLite mutex ownerを
    /// 使う。runごとに開き直すと、旧runのprefillと新runのpurgeが別connectionで競合する。
    fn db_for_worker(&self) -> rusqlite::Result<Arc<SimilarDb>> {
        let mut owner = self.prefill_db.lock().unwrap_or_else(|e| e.into_inner());
        let db = if let Some(db) = owner.as_ref() {
            Arc::clone(db)
        } else {
            let db = Arc::new(SimilarDb::open_at(&SimilarDb::db_path_at(&self.data_dir))?);
            *owner = Some(Arc::clone(&db));
            db
        };
        drop(owner);
        // The global decode callback registration always carries the matching DB/scope pair.
        register_prefill_db(&db, &self.enabled_roots);
        Ok(db)
    }

    fn retain_loaded_snapshot_during_run(&self) {
        self.memory_epoch.fetch_add(1, Ordering::AcqRel);
        retain_ready_memory_or_unload(&mut self.memory.lock().unwrap_or_else(|e| e.into_inner()));
    }

    fn request_array_refresh(self: &Arc<Self>) {
        self.start_array_refresh(None);
    }

    fn request_array_refresh_through(
        self: &Arc<Self>,
        watermark: crate::similar_db::StoreWatermark,
    ) {
        self.start_array_refresh(Some(watermark));
    }

    fn start_array_refresh(self: &Arc<Self>, watermark: Option<crate::similar_db::StoreWatermark>) {
        let should_start = {
            let mut state = self.array_update.lock().unwrap_or_else(|e| e.into_inner());
            state.requested = true;
            state.last_error = None;
            if let Some(watermark) = watermark {
                state.required_through = Some(match state.required_through {
                    Some(current) if current.store_id == watermark.store_id => {
                        crate::similar_db::StoreWatermark {
                            store_id: current.store_id,
                            through_change_seq: current
                                .through_change_seq
                                .max(watermark.through_change_seq),
                        }
                    }
                    _ => watermark,
                });
            }
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
                {
                    let mut state = self.array_update.lock().unwrap_or_else(|e| e.into_inner());
                    state.running = false;
                    state.last_error =
                        Some(format!("similar array update worker start failed: {error}"));
                }
                crate::logger::log(format!("similar array update worker start failed: {error}"));
                self.array_changed.notify_all();
                self.book_query.soft_refresh();
            }
        }
    }

    fn snapshot_reaches(
        snapshot: &SearchSnapshot,
        watermark: crate::similar_db::StoreWatermark,
    ) -> bool {
        snapshot.base.store_id == watermark.store_id
            && snapshot.applied_seq >= watermark.through_change_seq
    }

    fn wait_for_array_ack(
        &self,
        watermark: crate::similar_db::StoreWatermark,
        cancel: &AtomicBool,
    ) -> Result<(), ArrayAckWaitError> {
        loop {
            if cancel.load(Ordering::Acquire)
                || self
                    .state
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .shutdown
            {
                return Err(ArrayAckWaitError::Cancelled);
            }
            let awaiting_initial_load = {
                let memory = self.memory.lock().unwrap_or_else(|e| e.into_inner());
                match &*memory {
                    MemoryState::Ready(snapshot) if Self::snapshot_reaches(snapshot, watermark) => {
                        return Ok(());
                    }
                    MemoryState::Failed(error) => {
                        return Err(ArrayAckWaitError::Failed(error.clone()));
                    }
                    MemoryState::Missing => {
                        return Err(ArrayAckWaitError::Failed(
                            "similar array publication source is missing".to_owned(),
                        ));
                    }
                    MemoryState::Loading => true,
                    MemoryState::Unloaded | MemoryState::Ready(_) => false,
                }
            };
            let state = self.array_update.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(error) = state.last_error.clone() {
                return Err(ArrayAckWaitError::Failed(error));
            }
            if !awaiting_initial_load && !state.running && !state.requested {
                return Err(ArrayAckWaitError::Failed(
                    "similar array publication worker stopped before the requested watermark"
                        .to_owned(),
                ));
            }
            let _ = self
                .array_changed
                .wait_timeout(state, Duration::from_millis(100))
                .unwrap_or_else(|e| e.into_inner());
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
                        if active
                            .as_ref()
                            .is_some_and(|active| active.should_compact())
                        {
                            self.start_compaction(
                                db,
                                Arc::clone(active.as_ref().expect("checked active snapshot")),
                            );
                        }
                        if let Some(active) = active {
                            let mut state =
                                self.array_update.lock().unwrap_or_else(|e| e.into_inner());
                            if state
                                .required_through
                                .is_some_and(|required| Self::snapshot_reaches(&active, required))
                            {
                                state.required_through = None;
                            }
                            state.last_error = None;
                            drop(state);
                            self.array_changed.notify_all();
                        }
                    }
                    Err(error) => {
                        crate::logger::log(format!("similar array update failed: {error}"));
                        self.array_update
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .last_error = Some(error);
                        self.array_changed.notify_all();
                    }
                }
            }
            let mut state = self.array_update.lock().unwrap_or_else(|e| e.into_inner());
            if state.requested {
                continue;
            }
            state.running = false;
            drop(state);
            self.array_changed.notify_all();
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
                .entries
                .clear();
            self.book_query.soft_refresh();
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
        self.state.lock().unwrap_or_else(|e| e.into_inner()).phase = SchedulerPhase::Idle;
        *self.progress.lock().unwrap_or_else(|e| e.into_inner()) = progress;
        self.book_query.soft_refresh();
    }

    fn shutdown(&self) {
        {
            let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            state.shutdown = true;
            state.phase.cancel();
        }
        self.array_changed.notify_all();
        self.book_query.shutdown();
    }
}

fn key_is_under_any(key: &str, roots: &[String]) -> bool {
    crate::similar_db::key_is_under_any(key, roots)
}

fn normalized_parent(key: &str) -> Option<&str> {
    let trimmed = key.trim_end_matches('/');
    let separator = trimmed.rfind('/')?;
    (separator > 2).then_some(&trimmed[..separator])
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

/// 単体照会の結果置き場。
///
/// **1 枠では足りない。** 見開きは 2 ページを同じフレームで引くので、1 枠だと後の照会が前の
/// 照会を追い出し、返ってきた結果も捨てられる。どちらも永久に「読み込み中」のまま、毎フレーム
/// worker を 2 本ずつ起こし続ける状態になる。
#[derive(Default)]
struct ItemQueryCache {
    entries: Vec<CachedItemQuery>,
}

/// 同時に覚えておく照会の数。見開きの 2 ページに、行き来したときの数ページ分の余裕を足す。
const ITEM_QUERY_CACHE_LIMIT: usize = 6;

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
        .entries
        .iter()
        .find(|entry| entry.item_key == item_key && entry.memory_epoch == memory_epoch)
        .map(|entry| Arc::clone(&entry.result))
}

fn store_cached_item_query(
    cache: &Mutex<ItemQueryCache>,
    item_key: &str,
    memory_epoch: u64,
    result: Arc<ItemQuery>,
) {
    let mut cache = cache.lock().unwrap_or_else(|e| e.into_inner());
    cache
        .entries
        .retain(|entry| entry.item_key != item_key && entry.memory_epoch == memory_epoch);
    cache.entries.push(CachedItemQuery {
        item_key: item_key.to_owned(),
        memory_epoch,
        result,
    });
    let overflow = cache.entries.len().saturating_sub(ITEM_QUERY_CACHE_LIMIT);
    cache.entries.drain(..overflow);
}

fn replace_cached_item_query_if_current(
    cache: &Mutex<ItemQueryCache>,
    item_key: &str,
    memory_epoch: u64,
    result: Arc<ItemQuery>,
) -> bool {
    let mut cache = cache.lock().unwrap_or_else(|e| e.into_inner());
    let Some(entry) = cache
        .entries
        .iter_mut()
        .find(|entry| entry.item_key == item_key && entry.memory_epoch == memory_epoch)
    else {
        return false;
    };
    entry.result = result;
    true
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
            target,
        });
    }
    hits.sort_by(|left, right| {
        left.distance
            .cmp(&right.distance)
            .then_with(|| left.item_key.cmp(&right.item_key))
    });
    let origin_target = resolved_target_for_item(&origin);
    ItemQuery::Ready(ItemMatches {
        origin: OriginItem {
            item_key: origin.item_key,
            kind: origin.kind,
            mtime: origin.mtime,
            file_size: origin.file_size,
            width: origin.width,
            height: origin.height,
            format: SimilarImageFormat::from_i64(origin.format),
            target: origin_target,
        },
        hits,
    })
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
    query_book_ready_inner(db, snapshot, container_key, false)
}

/// `scan_candidate_pages` は計測専用。製品経路は常に `false` で呼ぶ。
fn query_book_ready_inner(
    db: &SimilarDb,
    snapshot: &SearchSnapshot,
    container_key: &str,
    scan_candidate_pages: bool,
) -> BookQuery {
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
    let origin = Arc::new(build_book_origin(&origin_pages, &matches));

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
    // **候補側のページは全体へ当てない。**
    //
    // 当てれば「相手の本にあり、蔵書全体でありふれたページ」を分母から外せるが、費用が
    // 見合わない。実店 39 冊で測ると、走査ありは 123.6 秒、なしは 24.8 秒。差が出たのは
    // 9 冊で、いずれも `distinctive_b` が数 % 増えて被覆率が 2〜4% 下がるだけ。**判定が
    // 変わった本は 0 冊。**
    //
    // 構造的にもそうなる。この走査だけが拾うのは「起点側に近いページが無い、相手の
    // ありふれたページ」で、起点側にあれば起点の走査が同じ文脈を拾う。そして起点に無い
    // ページは被覆率 B の分母にしか効かないので、**誤差は必ず被覆率を低く見せる向き**に
    // 出る。含有関係を過大に言うことはない。
    //
    // 代わりに、起点の走査で見つけた文脈だけを両方の本に使う。
    let candidate_matches: Vec<BookMatchSet> = if scan_candidate_pages {
        match collect_book_page_matches_for(db, snapshot, &borrowed, hash_version) {
            Ok(matches) => matches,
            Err(error) => return BookQuery::Failed(error),
        }
    } else {
        borrowed
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
            .collect()
    };

    let origin_page_slots = book_origin_page_slots(&origin_pages);
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
                let overrides =
                    build_page_strip_overrides(&origin_pages, &origin_page_slots, pages, &pair);
                let hit = match BookRelationHit::new(
                    candidate_key,
                    pages.len() as u32,
                    pair,
                    overrides,
                    origin.pages.len(),
                ) {
                    Ok(hit) => hit,
                    Err(error) => return BookQuery::Failed(error),
                };
                hits.push(hit)
            }
            Err(error) => return BookQuery::Failed(error.to_string()),
        }
    }
    hits.sort_by(|left, right| left.other_container_key.cmp(&right.other_container_key));
    BookQuery::Ready(BookRelations { origin, hits })
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

fn build_book_origin(
    origin_pages: &[crate::similar_db::SearchRow],
    origin_matches: &BookMatchSet,
) -> BookOrigin {
    BookOrigin {
        pages: origin_pages
            .iter()
            .enumerate()
            .map(|(slot, row)| {
                let excluded = row.item.quality < BOOK_MIN_QUALITY
                    || origin_matches
                        .pages
                        .get(slot)
                        .is_some_and(|page| page.origin_is_common);
                BookOriginPage {
                    item_key: row.item.item_key.clone(),
                    baseline: if excluded {
                        BookPageBaseline::Excluded
                    } else {
                        BookPageBaseline::Unmatched
                    },
                }
            })
            .collect::<Vec<_>>()
            .into_boxed_slice(),
    }
}

/// 起点の本のページ帯に重ねる sparse match を組む。
///
/// 対応が取れたページは `alignment` に (起点ページ, 相手ページ) として並ぶ。距離は
/// `alignment` に載っていないので、両方の署名から測り直して「ほぼ同一」と「別バージョン」を
/// 分ける。単体画像の帯 (§9.5) と同じ切り方にして、2 か所で違う基準を持たない。
pub(crate) fn book_origin_page_slots(
    origin_pages: &[crate::similar_db::SearchRow],
) -> HashMap<u32, usize> {
    origin_pages
        .iter()
        .enumerate()
        .filter_map(|(slot, row)| row.item.page_index.map(|page| (page, slot)))
        .collect()
}

pub(crate) fn build_page_strip_overrides(
    origin_pages: &[crate::similar_db::SearchRow],
    by_origin_page: &HashMap<u32, usize>,
    candidate_pages: &[crate::similar_db::SearchRow],
    pair: &dupe::book::BookPair,
) -> Vec<BookPageMatch> {
    let candidate_by_page = candidate_pages
        .iter()
        .filter_map(|row| row.item.page_index.map(|page| (page, row)))
        .collect::<HashMap<_, _>>();

    // 同じ本のページはコンテナが同じなので、移動先の解決は本の中で 1 度で足りる。
    let mut resolved_targets: HashMap<u64, Option<SimilarItemTarget>> = HashMap::new();
    let mut overrides = Vec::with_capacity(pair.alignment.len());
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
        overrides.push(BookPageMatch {
            origin_slot: slot,
            state: match match_band(distance) {
                Some(MatchBand::NearlyIdentical) => BookPageMatchState::Strong,
                _ => BookPageMatchState::Weak,
            },
            other_page_index: other_page,
            other_target: target,
            other_item_key: other.item.item_key.clone(),
            other_mtime: other.item.mtime,
            other_file_size: other.item.file_size,
        });
    }
    overrides
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

/// 本単位の走査に使う専用プール。
///
/// **rayon の共有プールを使わない。** 共有プールは全コアを取るので、数秒続くこの走査が
/// 走っている間、他所の音声が途切れる (利用者環境で実際に起きた)。パネルを開いただけで
/// 機械が持っていかれるのは、答えが数秒早いことと引き合わない。
///
/// スレッド数を絞ったうえで、優先度も下げる。数を絞るだけでは、詰まっているときに走査が
/// 前面のスレッドと同じ土俵で競ってしまう。
fn book_scan_pool() -> Option<&'static rayon::ThreadPool> {
    static POOL: OnceLock<Option<rayon::ThreadPool>> = OnceLock::new();
    POOL.get_or_init(|| {
        // 2 コア分は空けておく。1 つは音声、1 つは UI と OS のために残す。**上限は置かない** —
        // 24 論理プロセッサの機械で 8 に絞ったところ、所要時間が 3.4 秒から 6.5 秒になった。
        // 前面へ譲る役目は優先度が担うので、数はコアを空けるためだけに使う。
        let threads = std::thread::available_parallelism()
            .map(|cores| cores.get().saturating_sub(2).max(1))
            .unwrap_or(2);
        match rayon::ThreadPoolBuilder::new()
            .num_threads(threads)
            .thread_name(|index| format!("similar-book-scan-{index}"))
            .start_handler(|_| lower_current_thread_priority())
            .build()
        {
            Ok(pool) => Some(pool),
            Err(error) => {
                crate::logger::log(format!("similar book scan pool: {error}"));
                None
            }
        }
    })
    .as_ref()
}

/// 走査スレッドを前面より下の優先度にする。
///
/// `THREAD_MODE_BACKGROUND_BEGIN` は I/O まで大きく絞るので使わない。ここで読むのは
/// 在メモリの配列で、利用者はパネルの答えを待っている。CPU の順番だけを譲る。
pub(crate) fn lower_current_thread_priority() {
    #[cfg(windows)]
    unsafe {
        use windows::Win32::System::Threading::{
            GetCurrentThread, SetThreadPriority, THREAD_PRIORITY_BELOW_NORMAL,
        };
        if SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_BELOW_NORMAL).is_err() {
            crate::logger::log("similar book scan: SetThreadPriority(BelowNormal) failed");
        }
    }
}

#[cfg(windows)]
fn current_thread_priority() -> i32 {
    unsafe {
        use windows::Win32::System::Threading::{GetCurrentThread, GetThreadPriority};
        GetThreadPriority(GetCurrentThread())
    }
}

#[cfg(not(windows))]
fn current_thread_priority() -> i32 {
    0
}

#[cfg(windows)]
fn current_thread_cpu_100ns() -> Option<u64> {
    use windows::Win32::Foundation::FILETIME;
    use windows::Win32::System::Threading::{GetCurrentThread, GetThreadTimes};
    let mut creation = FILETIME::default();
    let mut exit = FILETIME::default();
    let mut kernel = FILETIME::default();
    let mut user = FILETIME::default();
    unsafe {
        GetThreadTimes(
            GetCurrentThread(),
            &mut creation,
            &mut exit,
            &mut kernel,
            &mut user,
        )
        .ok()?;
    }
    let ticks =
        |value: FILETIME| (u64::from(value.dwHighDateTime) << 32) | u64::from(value.dwLowDateTime);
    Some(ticks(kernel).saturating_add(ticks(user)))
}

#[cfg(not(windows))]
fn current_thread_cpu_100ns() -> Option<u64> {
    None
}

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
    let scan = || {
        records
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
            )
    };
    let mut per_page = match book_scan_pool() {
        Some(pool) => pool.install(scan),
        None => scan(),
    };
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

/// Waits until the existing activity policy permits at least one new scan unit.
///
/// A full inventory load is itself new scan work: it can lock and read millions of DB rows before
/// the first filesystem work lease exists.  Reuse the same 0/1/full concurrency decision as the
/// work queue, before taking the DB mutex.  In particular, ordinary foreground activity keeps the
/// established one-worker allowance while an explicit pause remains a strict zero-worker barrier.
fn wait_for_full_inventory_start(
    activity_gate: Option<&crate::activity_gate::ActivityGate>,
    cancel: &AtomicBool,
) -> bool {
    loop {
        if cancel.load(Ordering::Acquire) {
            return false;
        }
        if current_concurrency_limits(activity_gate).global > 0 {
            return true;
        }
        std::thread::sleep(INDEX_LIMIT_RECHECK);
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

    fn merge(&self, report: &mut IndexReport, local_seen: &mut ScanLocalSeen, prune_safe: bool) {
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
            if let ScanLocalSeen::Delta { items, containers } = local_seen {
                state.seen_items.extend(items.drain());
                state.seen_containers.extend(containers.drain());
            }
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
    DirectoryContents(PathBuf),
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
            Self::Directory(path) | Self::DirectoryContents(path) => path,
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

struct ScanJobOutcome {
    report: IndexReport,
    prune_safe: bool,
}

#[derive(Default)]
struct DeltaPublication {
    loose_items: Mutex<Vec<StoredItem>>,
    completed_containers: Mutex<Vec<(String, u64)>>,
}

#[derive(Clone, Copy)]
enum ScanPass<'a> {
    Full(&'a FullReconcileInventory),
    Delta(&'a DeltaPublication),
}

enum ScanLocalSeen {
    Full,
    Delta {
        items: HashSet<String>,
        containers: HashSet<String>,
    },
}

impl DeltaPublication {
    fn stage_loose(&self, item: StoredItem) {
        self.loose_items
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .push(item);
    }

    fn stage_container(&self, container_key: String, generation: u64) {
        self.completed_containers
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .push((container_key, generation));
    }

    fn into_parts(self) -> (Vec<StoredItem>, Vec<(String, u64)>) {
        (
            self.loose_items
                .into_inner()
                .unwrap_or_else(|error| error.into_inner()),
            self.completed_containers
                .into_inner()
                .unwrap_or_else(|error| error.into_inner()),
        )
    }
}

fn run_index_job(
    db: &SimilarDb,
    roots: &[PathBuf],
    excluded_root_keys: &[String],
    pdf_passwords: &crate::pdf_passwords::PdfPasswordStore,
    activity_gate: Option<&crate::activity_gate::ActivityGate>,
    cancel: &Arc<AtomicBool>,
    progress: &Arc<Mutex<IndexProgress>>,
    array_refresh: &ArrayRefreshNotifier,
) -> Result<ScanJobOutcome, String> {
    if !wait_for_full_inventory_start(activity_gate, cancel) {
        return Ok(ScanJobOutcome {
            report: IndexReport::default(),
            prune_safe: false,
        });
    }
    db.cleanup_incomplete()
        .map_err(|error| format!("incomplete generation cleanup failed: {error}"))?;
    set_stage(progress, IndexStage::Opening, None);
    let Some(inventory) = db
        .load_full_reconcile_inventory(current_hash_version(), || !cancel.load(Ordering::Acquire))
        .map_err(|error| format!("full reconcile inventory load failed: {error}"))?
    else {
        return Ok(ScanJobOutcome {
            report: IndexReport::default(),
            prune_safe: false,
        });
    };
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
                    excluded_root_keys,
                    pdf_passwords,
                    activity_gate,
                    cancel,
                    progress,
                    array_refresh,
                    ScanPass::Full(&inventory),
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
        return Ok(ScanJobOutcome {
            report: aggregate.report,
            prune_safe: false,
        });
    }
    set_stage(progress, IndexStage::Pruning, None);
    if aggregate.prune_safe {
        let completed_at_unix_secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| i64::try_from(duration.as_secs()).unwrap_or(i64::MAX))
            .unwrap_or(0);
        let (removed, _) = db
            .finalize_full_reconcile_inventory_if(
                inventory,
                current_hash_version(),
                completed_at_unix_secs,
                CompletedIndexStats {
                    password_required_pdfs: aggregate.report.password_required_pdfs,
                    corrupt_containers: aggregate.report.corrupt_containers,
                    zero_page_containers: aggregate.report.zero_page_containers,
                    decode_failures: aggregate.report.decode_failures,
                    io_failures: aggregate.report.io_failures,
                },
                || !cancel.load(Ordering::Acquire),
            )
            .map_err(|error| format!("full reconcile publish failed: {error}"))?;
        aggregate.report.removed = removed as u64;
        if aggregate.report.removed > 0 {
            array_refresh.request();
        }
    }
    publish_report(progress, &aggregate.report);
    Ok(ScanJobOutcome {
        report: aggregate.report,
        prune_safe: aggregate.prune_safe,
    })
}

fn run_delta_index_job(
    db: &SimilarDb,
    dirty: &DirtyScopeSet,
    config: &SchedulerConfig,
    cancel: &Arc<AtomicBool>,
    progress: &Arc<Mutex<IndexProgress>>,
    _array_refresh: &ArrayRefreshNotifier,
) -> Result<ScanJobOutcome, String> {
    db.cleanup_incomplete()
        .map_err(|error| format!("incomplete generation cleanup failed: {error}"))?;
    set_stage(progress, IndexStage::Scanning, None);
    let root_keys = config.normalized_roots();
    let mut active_scopes = Vec::new();
    let mut initial = Vec::new();
    for scope in dirty.scopes() {
        let path = match scope {
            DirtyScope::DirectoryContents(path)
            | DirtyScope::Subtree(path)
            | DirtyScope::RemovedPrefix(path)
            | DirtyScope::RootRepair(path) => path,
        };
        let key = crate::search_index_db::normalize_path(path);
        if !key_is_under_any(&key, &root_keys) || key_is_under_any(&key, &config.excluded_root_keys)
        {
            continue;
        }
        let metadata = std::fs::metadata(path);
        match (scope, metadata) {
            (DirtyScope::DirectoryContents(path), Ok(metadata)) if metadata.is_dir() => {
                initial.push(ScanWork::DirectoryContents(path.clone()).tagged());
                active_scopes.push(scope.clone());
            }
            (DirtyScope::Subtree(path), Ok(metadata)) if metadata.is_dir() => {
                initial.push(ScanWork::Directory(path.clone()).tagged());
                active_scopes.push(scope.clone());
            }
            (DirtyScope::Subtree(path), Err(error))
                if error.kind() == std::io::ErrorKind::NotFound =>
            {
                // The producer could only classify this path conservatively.  A confirmed
                // disappearance at the worker boundary owns the whole former subtree,
                // including nested rows that the paired parent scan cannot see.
                active_scopes.push(DirtyScope::RemovedPrefix(path.clone()));
            }
            (DirtyScope::RootRepair(path), Ok(metadata)) if metadata.is_dir() => {
                initial.push(ScanWork::Directory(path.clone()).tagged());
                active_scopes.push(scope.clone());
            }
            (DirtyScope::RootRepair(path), Err(error))
                if error.kind() == std::io::ErrorKind::NotFound =>
            {
                active_scopes.push(DirtyScope::RemovedPrefix(path.clone()));
            }
            (DirtyScope::RootRepair(path), Ok(_)) => {
                crate::logger::log(format!(
                    "similar delta observation incomplete: repair root is not a directory: {}",
                    path.display()
                ));
                return Ok(ScanJobOutcome {
                    report: IndexReport::default(),
                    prune_safe: false,
                });
            }
            (DirtyScope::RemovedPrefix(_), Err(error))
                if error.kind() == std::io::ErrorKind::NotFound =>
            {
                active_scopes.push(scope.clone());
            }
            (
                DirtyScope::DirectoryContents(_)
                | DirtyScope::Subtree(_)
                | DirtyScope::RootRepair(_),
                Err(error),
            ) if error.kind() != std::io::ErrorKind::NotFound => {
                crate::logger::log(format!(
                    "similar delta observation incomplete for {}: {error}",
                    path.display()
                ));
                return Ok(ScanJobOutcome {
                    report: IndexReport::default(),
                    prune_safe: false,
                });
            }
            (DirtyScope::RemovedPrefix(_), Err(error)) => {
                crate::logger::log(format!(
                    "similar delta removal observation incomplete for {}: {error}",
                    path.display()
                ));
                return Ok(ScanJobOutcome {
                    report: IndexReport::default(),
                    prune_safe: false,
                });
            }
            (DirtyScope::RootRepair(_), Err(error)) => {
                debug_assert_ne!(error.kind(), std::io::ErrorKind::NotFound);
                return Ok(ScanJobOutcome {
                    report: IndexReport::default(),
                    prune_safe: false,
                });
            }
            (DirtyScope::DirectoryContents(_) | DirtyScope::Subtree(_), _)
            | (DirtyScope::RemovedPrefix(_), Ok(_)) => {
                // A vanished scan scope is covered by the paired parent/removal observation;
                // a remove followed by re-create keeps the current filesystem object.
            }
        }
    }

    let aggregate = ScanAggregate::new(progress);
    let publication = DeltaPublication::default();
    let queue = BoundedWorkQueue::new(initial);
    let deferred_refresh = ArrayRefreshNotifier {
        scheduler: Weak::new(),
    };
    let worker_result = std::thread::scope(|scope| {
        let mut workers = Vec::with_capacity(INDEX_GLOBAL_OUTSTANDING_LIMIT);
        for _ in 0..INDEX_GLOBAL_OUTSTANDING_LIMIT {
            workers.push(scope.spawn(|| {
                scan_worker_loop(
                    &queue,
                    &aggregate,
                    db,
                    &config.excluded_root_keys,
                    &config.pdf_passwords,
                    config.activity_gate.as_deref(),
                    cancel,
                    progress,
                    &deferred_refresh,
                    ScanPass::Delta(&publication),
                )
            }));
        }
        for worker in workers {
            if worker.join().is_err() {
                return Err("similar index delta worker panicked".to_owned());
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
        return Ok(ScanJobOutcome {
            report: aggregate.report,
            prune_safe: false,
        });
    }
    set_stage(progress, IndexStage::Pruning, None);
    if aggregate.prune_safe {
        let mut directory_contents = Vec::new();
        let mut subtrees = Vec::new();
        let mut removed_prefixes = Vec::new();
        for scope in &active_scopes {
            let (target, path) = match scope {
                DirtyScope::DirectoryContents(path) => (&mut directory_contents, path),
                DirtyScope::Subtree(path) | DirtyScope::RootRepair(path) => (&mut subtrees, path),
                DirtyScope::RemovedPrefix(path) => (&mut removed_prefixes, path),
            };
            target.push(crate::search_index_db::normalize_path(path));
        }
        let completed_at_unix_secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| i64::try_from(duration.as_secs()).unwrap_or(i64::MAX))
            .unwrap_or(0);
        let (loose_items, completed_containers) = publication.into_parts();
        aggregate.report.removed = match db.publish_delta_reconcile_if(
            &loose_items,
            &completed_containers,
            &directory_contents,
            &subtrees,
            &removed_prefixes,
            &aggregate.seen_items,
            &aggregate.seen_containers,
            current_hash_version(),
            completed_at_unix_secs,
            || !cancel.load(Ordering::Acquire),
        ) {
            Ok(removed) => removed as u64,
            Err(error) => {
                db.cleanup_incomplete().map_err(|cleanup| {
                    format!("delta publish failed: {error}; generation cleanup failed: {cleanup}")
                })?;
                return Err(format!("delta publish failed: {error}"));
            }
        };
    } else {
        db.cleanup_incomplete()
            .map_err(|error| format!("incomplete delta cleanup failed: {error}"))?;
    }
    publish_report(progress, &aggregate.report);
    Ok(ScanJobOutcome {
        report: aggregate.report,
        prune_safe: aggregate.prune_safe,
    })
}

struct ScanContext<'a> {
    db: &'a SimilarDb,
    excluded_root_keys: &'a [String],
    pdf_passwords: &'a crate::pdf_passwords::PdfPasswordStore,
    cancel: &'a Arc<AtomicBool>,
    aggregate: &'a ScanAggregate<'a>,
    report: IndexReport,
    local_seen: ScanLocalSeen,
    /// 走査漏れと削除を区別できない I/O failure が 1 件でもあれば prune しない。
    prune_safe: bool,
    array_refresh: &'a ArrayRefreshNotifier,
    pass: ScanPass<'a>,
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
    excluded_root_keys: &[String],
    pdf_passwords: &crate::pdf_passwords::PdfPasswordStore,
    activity_gate: Option<&crate::activity_gate::ActivityGate>,
    cancel: &Arc<AtomicBool>,
    progress: &Arc<Mutex<IndexProgress>>,
    array_refresh: &ArrayRefreshNotifier,
    pass: ScanPass<'_>,
) {
    while let Some(mut lease) = queue.take(activity_gate, cancel.as_ref()) {
        let work = lease.take_task();
        let path = work.path().to_path_buf();
        set_stage(progress, IndexStage::Scanning, Some(path.clone()));
        let mut context = ScanContext {
            db,
            excluded_root_keys,
            pdf_passwords,
            cancel,
            aggregate,
            report: IndexReport::default(),
            local_seen: match pass {
                ScanPass::Full(_) => ScanLocalSeen::Full,
                ScanPass::Delta(_) => ScanLocalSeen::Delta {
                    items: HashSet::new(),
                    containers: HashSet::new(),
                },
            },
            prune_safe: true,
            array_refresh,
            pass,
        };
        let result = if context.cancelled() {
            Ok(Vec::new())
        } else {
            match work {
                ScanWork::Directory(directory) => context.discover_directory(&directory, true),
                ScanWork::DirectoryContents(directory) => {
                    context.discover_directory(&directory, false)
                }
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
        self.aggregate
            .merge(&mut self.report, &mut self.local_seen, self.prune_safe);
    }

    fn mark_item_seen(&mut self, item_key: &str) {
        match (&self.pass, &mut self.local_seen) {
            (ScanPass::Full(inventory), ScanLocalSeen::Full) => {
                inventory.mark_item(item_key);
            }
            (ScanPass::Delta(_), ScanLocalSeen::Delta { items, .. }) => {
                items.insert(item_key.to_owned());
            }
            _ => unreachable!("scan pass and local seen owner must match"),
        }
    }

    fn mark_container_seen(&mut self, container_key: &str) {
        match (&self.pass, &mut self.local_seen) {
            (ScanPass::Full(inventory), ScanLocalSeen::Full) => {
                inventory.observe_container(container_key);
            }
            (ScanPass::Delta(_), ScanLocalSeen::Delta { containers, .. }) => {
                containers.insert(container_key.to_owned());
            }
            _ => unreachable!("scan pass and local seen owner must match"),
        }
    }

    fn item_is_exact_current(
        &mut self,
        item_key: &str,
        owner: Option<&str>,
        page_index: Option<u32>,
        candidate: &FileCandidate,
    ) -> Result<bool, String> {
        match self.pass {
            ScanPass::Full(inventory) => Ok(inventory
                .observe_item(
                    item_key,
                    owner,
                    page_index,
                    candidate.mtime,
                    candidate.file_size,
                )
                .exact_current),
            ScanPass::Delta(_) => {
                self.mark_item_seen(item_key);
                Ok(self
                    .db
                    .load_item(item_key, current_hash_version())
                    .map_err(db_error)?
                    .is_some_and(|existing| {
                        existing.item.container_key.as_deref() == owner
                            && existing.item.page_index == page_index
                            && item_metadata_matches(&existing.item, candidate)
                    }))
            }
        }
    }

    fn reusable_current_item(
        &self,
        item_key: &str,
        owner: Option<&str>,
        page_index: Option<u32>,
        candidate: &FileCandidate,
    ) -> Result<Option<StoredItem>, String> {
        if let ScanPass::Full(inventory) = self.pass
            && !inventory
                .observe_item(
                    item_key,
                    owner,
                    page_index,
                    candidate.mtime,
                    candidate.file_size,
                )
                .reusable_current_row
        {
            return Ok(None);
        }
        Ok(self
            .db
            .load_item(item_key, current_hash_version())
            .map_err(db_error)?
            .filter(|existing| item_metadata_matches(&existing.item, candidate))
            .map(|existing| existing.item))
    }

    fn container_observation(
        &self,
        container_key: &str,
        mtime: i64,
        file_size: i64,
        page_count: u32,
    ) -> Result<crate::similar_db::FullContainerObservation, String> {
        match self.pass {
            ScanPass::Full(inventory) => Ok(inventory.container_observation(
                container_key,
                mtime,
                file_size,
                page_count,
                current_hash_version(),
            )),
            ScanPass::Delta(_) => {
                let freshness = self
                    .db
                    .container_freshness(
                        container_key,
                        mtime,
                        file_size,
                        page_count,
                        current_hash_version(),
                    )
                    .map_err(db_error)?;
                Ok(crate::similar_db::FullContainerObservation {
                    freshness,
                    member_count: 0,
                })
            }
        }
    }

    fn preserve_current_container(&mut self, container_key: &str) -> Result<u64, String> {
        match self.pass {
            ScanPass::Full(inventory) => {
                Ok(u64::from(inventory.container_member_count(container_key)))
            }
            ScanPass::Delta(_) => {
                let keys = self
                    .db
                    .item_keys_for_container(container_key)
                    .map_err(db_error)?;
                let count = keys.len() as u64;
                let ScanLocalSeen::Delta { items, .. } = &mut self.local_seen else {
                    unreachable!("delta pass must own local seen sets")
                };
                items.extend(keys);
                Ok(count)
            }
        }
    }

    fn publish_loose_item(&self, item: StoredItem) -> Result<(), String> {
        match self.pass {
            ScanPass::Delta(publication) => {
                publication.stage_loose(item);
                Ok(())
            }
            ScanPass::Full(_) => {
                if self
                    .db
                    .upsert_loose_item_if(&item, || !self.cancelled())
                    .map_err(db_error)?
                {
                    self.array_refresh.request();
                }
                Ok(())
            }
        }
    }

    fn finish_container(&self, container_key: &str, generation: u64) -> Result<(), String> {
        match self.pass {
            ScanPass::Delta(publication) => {
                publication.stage_container(container_key.to_owned(), generation);
                Ok(())
            }
            ScanPass::Full(_) => {
                self.db
                    .complete_container_if(container_key, generation, || !self.cancelled())
                    .map_err(db_error)?;
                self.array_refresh.request();
                Ok(())
            }
        }
    }

    fn discover_directory(
        &mut self,
        directory: &Path,
        recursive: bool,
    ) -> Result<Vec<ScanWork>, String> {
        if self.cancelled() {
            return Ok(Vec::new());
        }
        let directory_key = crate::search_index_db::normalize_path(directory);
        if key_is_under_any(&directory_key, self.excluded_root_keys) {
            return Ok(Vec::new());
        }
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
        if recursive {
            for child in subdirectories {
                work.push(ScanWork::Directory(child));
            }
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
        if self.item_is_exact_current(&key, None, page_index, candidate)? {
            self.report.unchanged += 1;
            self.report.processed += 1;
            return Ok(());
        }
        match self.build_file_item(&key, ItemKind::Image, None, page_index, candidate) {
            Ok(item) => {
                self.publish_loose_item(item)?;
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
        self.mark_container_seen(container_key);
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
            .container_observation(container_key, mtime, file_size, page_count)?
            .freshness
            == Freshness::Current;
        let mut every_page_current = container_current;
        if every_page_current {
            for (index, candidate) in images.iter().enumerate() {
                let key = crate::search_index_db::normalize_path(&candidate.path);
                if !self.item_is_exact_current(
                    &key,
                    Some(container_key),
                    Some(index as u32),
                    candidate,
                )? {
                    every_page_current = false;
                    break;
                }
            }
        }
        if every_page_current {
            let count = self.preserve_current_container(container_key)?;
            self.report.unchanged += count;
            self.report.processed += count;
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
            self.mark_item_seen(&key);
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
        self.finish_container(container_key, generation)?;
        self.report.indexed += images.len() as u64;
        self.report.containers_completed += 1;
        Ok(())
    }

    fn process_zip(&mut self, candidate: &FileCandidate) -> Result<(), String> {
        let container_key = crate::search_index_db::normalize_path(&candidate.path);
        self.mark_container_seen(&container_key);
        let entries = match crate::zip_loader::enumerate_image_entries(&candidate.path) {
            Ok(entries) => entries,
            Err(error) => {
                if self.cancelled() {
                    return Ok(());
                }
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
            if self.cancelled() {
                return Ok(());
            }
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
            .container_observation(
                &container_key,
                candidate.mtime,
                candidate.file_size,
                page_count,
            )?
            .freshness
            == Freshness::Current
        {
            let count = self.preserve_current_container(&container_key)?;
            self.report.unchanged += count;
            self.report.processed += count;
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
            self.mark_item_seen(&key);
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
        self.finish_container(&container_key, generation)?;
        self.report.indexed += entries.len() as u64;
        self.report.containers_completed += 1;
        Ok(())
    }

    fn process_pdf(&mut self, candidate: &FileCandidate) -> Result<(), String> {
        let container_key = crate::search_index_db::normalize_path(&candidate.path);
        self.mark_container_seen(&container_key);
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
            if self.cancelled() {
                return Ok(());
            }
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
            .container_observation(
                &container_key,
                candidate.mtime,
                candidate.file_size,
                page_count,
            )?
            .freshness
            == Freshness::Current
        {
            let count = self.preserve_current_container(&container_key)?;
            self.report.unchanged += count;
            self.report.processed += count;
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
            self.mark_item_seen(&key);
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
        self.finish_container(&container_key, generation)?;
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
        if let Some(existing) =
            self.reusable_current_item(key, container_key, page_index, candidate)?
        {
            return Ok(with_identity(existing, kind, container_key, page_index));
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

fn book_engine_error(error: BookEngineError) -> String {
    error.to_string()
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

fn thumbnail_prefill_scope_allows(
    enabled_roots: &RwLock<Vec<String>>,
    normalized_path: &str,
) -> bool {
    let roots = enabled_roots.read().unwrap_or_else(|e| e.into_inner());
    key_is_under_any(normalized_path, &roots)
}

struct PreparedThumbnailPrefill {
    normalized_path: String,
    item: StoredItem,
}

#[derive(Debug)]
struct ThumbnailPrefillPrepareError {
    stage: &'static str,
    item_key: String,
    message: String,
}

#[allow(clippy::too_many_arguments)]
fn prepare_thumbnail_prefill(
    enabled_roots: &RwLock<Vec<String>>,
    path: &Path,
    zip_entry: Option<&str>,
    pdf_page: Option<u32>,
    mtime: i64,
    file_size: i64,
    image: &image::DynamicImage,
    source_dims: (u32, u32),
) -> Result<Option<PreparedThumbnailPrefill>, ThumbnailPrefillPrepareError> {
    let normalized_path = crate::search_index_db::normalize_path(path);
    // Fast rejection only. This helper returns an owned value, so its scope guard cannot survive
    // the decode/signature work or the later DB wait.
    if !thumbnail_prefill_scope_allows(enabled_roots, &normalized_path) {
        return Ok(None);
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
        return Ok(None);
    }
    let canonical = proxy_from_source(
        ProxySource::Raster {
            image,
            source_dims,
            format,
        },
        None,
    )
    .map_err(|error| ThumbnailPrefillPrepareError {
        stage: "proxy",
        item_key: item_key.clone(),
        message: error.to_string(),
    })?;
    let candidate = FileCandidate {
        path: path.to_path_buf(),
        mtime,
        file_size,
    };
    let item = stored_from_proxy(&item_key, kind, None, pdf_page, &candidate, canonical).map_err(
        |error| ThumbnailPrefillPrepareError {
            stage: "signature",
            item_key,
            message: error.to_string(),
        },
    )?;
    Ok(Some(PreparedThumbnailPrefill {
        normalized_path,
        item,
    }))
}

fn store_offered_thumbnail_prefill(
    db: &SimilarDb,
    enabled_roots: &RwLock<Vec<String>>,
    prepared: &PreparedThumbnailPrefill,
) -> rusqlite::Result<bool> {
    db.put_prefill_if(&prepared.item, || {
        thumbnail_prefill_scope_allows(enabled_roots, &prepared.normalized_path)
    })
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
    offer_thumbnail_raster_with_capability(
        PRODUCT_SIMILAR_FEATURE_CAPABILITY,
        path,
        zip_entry,
        pdf_page,
        mtime,
        file_size,
        image,
        source_dims,
    );
}

#[allow(clippy::too_many_arguments)]
fn offer_thumbnail_raster_with_capability(
    capability: SimilarFeatureCapability,
    path: &Path,
    zip_entry: Option<&str>,
    pdf_page: Option<u32>,
    mtime: i64,
    file_size: i64,
    image: &image::DynamicImage,
    source_dims: (u32, u32),
) {
    offer_thumbnail_raster_with_target_resolver(
        capability,
        path,
        zip_entry,
        pdf_page,
        mtime,
        file_size,
        image,
        source_dims,
        prefill_target,
    );
}

#[allow(clippy::too_many_arguments)]
fn offer_thumbnail_raster_with_target_resolver(
    capability: SimilarFeatureCapability,
    path: &Path,
    zip_entry: Option<&str>,
    pdf_page: Option<u32>,
    mtime: i64,
    file_size: i64,
    image: &image::DynamicImage,
    source_dims: (u32, u32),
    resolve_target: impl FnOnce() -> Option<(Arc<SimilarDb>, Arc<RwLock<Vec<String>>>)>,
) {
    // This direct product gate is intentional.  A stale process-global test registration (or a
    // future accidental manager construction) must not turn ordinary thumbnail decoding into
    // similar-index signature work or DB writes while the feature is paused.
    if !capability.is_enabled() {
        return;
    }
    let Some((db, enabled_roots)) = resolve_target() else {
        return;
    };
    let prepared = match prepare_thumbnail_prefill(
        &enabled_roots,
        path,
        zip_entry,
        pdf_page,
        mtime,
        file_size,
        image,
        source_dims,
    ) {
        Ok(Some(prepared)) => prepared,
        Ok(None) => return,
        Err(error) => {
            crate::logger::log(format!(
                "similar prefill {} failed for {}: {}",
                error.stage, error.item_key, error.message
            ));
            return;
        }
    };
    if let Err(error) = store_offered_thumbnail_prefill(&db, &enabled_roots, &prepared) {
        crate::logger::log(format!(
            "similar prefill write failed for {}: {error}",
            prepared.item.item_key
        ));
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

    /// 候補側の走査が結果を変えるのかを、多数の本で突き合わせる。
    ///
    /// この走査は照会時間の大半を占める。**変えないなら払う理由がない。** 1 プロセスで
    /// 両方を回すので、配列の読み込みは 1 回で済む。
    #[test]
    #[ignore = "manual sweep over a caller-selected similar.db"]
    fn sweep_whether_the_candidate_scan_changes_any_answer() {
        let db_path = std::env::var_os("MIV_SIMILAR_BENCH_DB")
            .map(PathBuf::from)
            .expect("set MIV_SIMILAR_BENCH_DB");
        let books: usize = std::env::var("MIV_SIMILAR_SWEEP_BOOKS")
            .ok()
            .and_then(|value| value.parse().ok())
            .unwrap_or(20);

        let db = SimilarDb::open_at(&db_path).unwrap();
        let loaded = similar_search_array::load_or_rebuild(
            &db,
            &similar_search_array::base_path(db_path.parent().expect("store has a parent")),
        )
        .unwrap();
        let snapshot = loaded.snapshot;

        let containers = db.load_complete_containers().unwrap();
        let stride = (containers.len() / books.max(1)).max(1);
        let mut checked = 0usize;
        let mut differing = 0usize;
        let mut relation_changed = 0usize;
        let mut full_ms = 0.0f64;
        let mut skip_ms = 0.0f64;
        for container in containers.iter().step_by(stride).take(books) {
            let started = std::time::Instant::now();
            let full = query_book_ready_inner(&db, &snapshot, &container.container_key, true);
            full_ms += started.elapsed().as_secs_f64() * 1000.0;
            let started = std::time::Instant::now();
            let skip = query_book_ready_inner(&db, &snapshot, &container.container_key, false);
            skip_ms += started.elapsed().as_secs_f64() * 1000.0;
            let BookQuery::Ready(full) = &full else {
                continue;
            };
            let BookQuery::Ready(skip) = &skip else {
                panic!("one side answered and the other did not");
            };
            if full.hits.is_empty() {
                continue;
            }
            checked += 1;
            if full
                .hits
                .iter()
                .zip(&skip.hits)
                .any(|(a, b)| a.pair.relation != b.pair.relation)
            {
                relation_changed += 1;
                eprintln!("  RELATION CHANGED: {}", container.container_key);
            }
            if full.hits != skip.hits {
                differing += 1;
                eprintln!("  differs: {}", container.container_key);
                for (a, b) in full.hits.iter().zip(&skip.hits) {
                    if a != b {
                        eprintln!(
                            "    other={} distinctive_b {} -> {} coverage_b {:.3} -> {:.3}",
                            a.other_container_key,
                            a.pair.distinctive_b,
                            b.pair.distinctive_b,
                            a.pair.coverage_b,
                            b.pair.coverage_b
                        );
                    }
                }
            }
        }
        eprintln!(
            "similar_candidate_scan_sweep books_with_hits={checked} differing={differing} relation_changed={relation_changed} full_ms={full_ms:.0} skip_ms={skip_ms:.0}"
        );
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

        let query_started = std::time::Instant::now();
        let result = query_book_ready(&db, &snapshot, &container_key);
        let query_ms = query_started.elapsed().as_secs_f64() * 1000.0;
        let resident_after = current_working_set_bytes();
        let summary = match &result {
            BookQuery::Ready(relations) => format!("Ready({})", relations.hits.len()),
            other => format!("{other:?}"),
        };
        eprintln!(
            "similar_book_query_measurement rows={rows} pages={} load_ms={load_ms:.1} query_ms={query_ms:.1} resident_before_bytes={resident_before} resident_loaded_bytes={resident_loaded} resident_after_bytes={resident_after} query_delta_bytes={} result={summary}",
            pages.len(),
            resident_after.saturating_sub(resident_loaded)
        );
        if let BookQuery::Ready(relations) = &result {
            for hit in relations.hits.iter().take(10) {
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
                ItemQuery::Ready(matches) => matches.hits.len(),
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
            if let ItemQuery::Ready(ItemMatches { hits, .. }) = result.as_ref()
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
    fn full_reconcile_inventory_obeys_pause_before_loading_and_cancel() {
        let passwords = crate::pdf_passwords::PdfPasswordStore::empty_for_test();

        let paused_db = SimilarDb::open_in_memory().unwrap();
        let paused_gate = crate::activity_gate::ActivityGate::new(10_000);
        paused_gate.set_paused(true);
        let paused_cancel = Arc::new(AtomicBool::new(false));
        let paused_progress = Arc::new(Mutex::new(IndexProgress::Idle));
        std::thread::scope(|scope| {
            let (started_tx, started_rx) = std::sync::mpsc::channel();
            let db = &paused_db;
            let gate = &paused_gate;
            let cancel = &paused_cancel;
            let progress = &paused_progress;
            let passwords = &passwords;
            let worker = scope.spawn(move || {
                started_tx.send(()).unwrap();
                run_index_job(
                    db,
                    &[],
                    &[],
                    passwords,
                    Some(gate),
                    cancel,
                    progress,
                    &ArrayRefreshNotifier {
                        scheduler: Weak::new(),
                    },
                )
                .unwrap()
            });
            started_rx.recv().unwrap();
            std::thread::sleep(INDEX_LIMIT_RECHECK * 3);
            assert_eq!(
                paused_db.full_inventory_load_count(),
                0,
                "an explicit pause must stop the inventory before it locks the DB"
            );
            paused_gate.set_paused(false);
            assert!(worker.join().unwrap().prune_safe);
        });
        assert_eq!(paused_db.full_inventory_load_count(), 1);

        let active_db = SimilarDb::open_in_memory().unwrap();
        let active_gate = crate::activity_gate::ActivityGate::new(10_000);
        active_gate.bump();
        assert!(
            run_index_job(
                &active_db,
                &[],
                &[],
                &passwords,
                Some(&active_gate),
                &Arc::new(AtomicBool::new(false)),
                &Arc::new(Mutex::new(IndexProgress::Idle)),
                &ArrayRefreshNotifier {
                    scheduler: Weak::new(),
                },
            )
            .unwrap()
            .prune_safe,
            "ordinary activity keeps the established one-worker allowance"
        );
        assert_eq!(active_db.full_inventory_load_count(), 1);

        let cancelled_db = SimilarDb::open_in_memory().unwrap();
        let cancelled_gate = crate::activity_gate::ActivityGate::new(10_000);
        cancelled_gate.set_paused(true);
        let cancelled = Arc::new(AtomicBool::new(false));
        let cancelled_progress = Arc::new(Mutex::new(IndexProgress::Idle));
        std::thread::scope(|scope| {
            let (started_tx, started_rx) = std::sync::mpsc::channel();
            let db = &cancelled_db;
            let gate = &cancelled_gate;
            let cancel = &cancelled;
            let progress = &cancelled_progress;
            let passwords = &passwords;
            let worker = scope.spawn(move || {
                started_tx.send(()).unwrap();
                run_index_job(
                    db,
                    &[],
                    &[],
                    passwords,
                    Some(gate),
                    cancel,
                    progress,
                    &ArrayRefreshNotifier {
                        scheduler: Weak::new(),
                    },
                )
                .unwrap()
            });
            started_rx.recv().unwrap();
            std::thread::sleep(INDEX_LIMIT_RECHECK * 2);
            cancelled.store(true, Ordering::Release);
            assert!(!worker.join().unwrap().prune_safe);
        });
        assert_eq!(
            cancelled_db.full_inventory_load_count(),
            0,
            "cancellation while paused must not enter the inventory loader"
        );
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
            &[],
            &passwords,
            None,
            &cancel,
            &progress,
            &ArrayRefreshNotifier {
                scheduler: Weak::new(),
            },
        )
        .unwrap()
        .report;
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

    use crate::similar_db::SearchRow;

    /// 候補が 0 件の結果。identity だけを比べるテスト用。
    fn empty_matches() -> ItemMatches {
        ItemMatches {
            origin: OriginItem {
                item_key: "origin".to_owned(),
                kind: ItemKind::Image,
                mtime: 0,
                file_size: 0,
                width: 0,
                height: 0,
                format: SimilarImageFormat::Other,
                target: None,
            },
            hits: Vec::new(),
        }
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
        let first = Arc::new(ItemQuery::Ready(empty_matches()));
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
            Arc::new(ItemQuery::Ready(empty_matches())),
        );
        assert!(!replace_cached_item_query_if_current(
            &cache,
            "origin-a",
            7,
            Arc::new(ItemQuery::Featureless),
        ));

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
    fn paused_capability_does_not_construct_the_optional_service_owner() {
        let temp = tempfile::tempdir().unwrap();
        let data_dir = temp.path().join("product-data");

        let manager = SimilarIndexManager::new_if_enabled(
            SimilarFeatureCapability::Paused,
            data_dir.clone(),
            || {},
        );

        assert!(manager.is_none());
        assert!(!data_dir.exists());
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
    fn paused_product_prefill_rejects_before_resolving_or_touching_the_store() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("already-decoded.png");
        let image = image::DynamicImage::new_rgba8(1024, 1024);
        let resolver_calls = std::sync::atomic::AtomicUsize::new(0);

        offer_thumbnail_raster_with_target_resolver(
            SimilarFeatureCapability::Paused,
            &path,
            None,
            None,
            10,
            20,
            &image,
            (1024, 1024),
            || {
                resolver_calls.fetch_add(1, Ordering::SeqCst);
                panic!("paused product prefill must not resolve a stale process-global target")
            },
        );

        assert_eq!(resolver_calls.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn running_projection_does_not_replace_the_cached_not_indexed_result() {
        let temp = tempfile::tempdir().unwrap();
        let db = SimilarDb::open_at(&SimilarDb::db_path_at(temp.path())).unwrap();
        let base = db.load_base_search_rows(current_hash_version()).unwrap();
        let snapshot = SearchSnapshot::from_base(crate::similar_search_array::BaseArray {
            records: base.records.into_boxed_slice(),
            store_id: base.store_id,
            applied_seq: base.applied_seq,
        });
        let manager = SimilarIndexManager::new(temp.path().to_path_buf());
        *manager.memory.lock().unwrap() = MemoryState::Ready(Arc::new(snapshot));
        *manager.enabled_roots.write().unwrap() = vec!["c:/library".to_owned()];
        let epoch = manager.memory_epoch.load(Ordering::Acquire);
        let run_query = |key: &str, terminal_before_completion: Option<IndexProgress>| {
            *manager.progress.lock().unwrap() = IndexProgress::Running(RunningProgress {
                stage: IndexStage::Scanning,
                current_path: None,
                report: IndexReport::default(),
            });
            let (started_tx, started_rx) = std::sync::mpsc::channel();
            let (resume_tx, resume_rx) = std::sync::mpsc::channel();
            let (completed_tx, completed_rx) = std::sync::mpsc::channel();
            *manager.item_query_test_hook.lock().unwrap() = Some(ItemQueryTestHook {
                started: started_tx,
                resume: resume_rx,
                completed: completed_tx,
            });

            assert_eq!(manager.query_item(key).as_ref(), &ItemQuery::Preparing);
            started_rx
                .recv_timeout(Duration::from_secs(1))
                .expect("query worker reached the deterministic barrier");
            if let Some(terminal) = terminal_before_completion.clone() {
                *manager.progress.lock().unwrap() = terminal;
            }
            resume_tx.send(()).unwrap();
            completed_rx
                .recv_timeout(Duration::from_secs(1))
                .expect("query worker published its terminal cache entry");
            let cached = cached_item_query(&manager.item_query, key, epoch)
                .expect("terminal query result is cached");
            assert_eq!(cached.as_ref(), &ItemQuery::NotIndexed);

            if terminal_before_completion.is_none() {
                assert_eq!(manager.query_item(key).as_ref(), &ItemQuery::Preparing);
                *manager.progress.lock().unwrap() = IndexProgress::Idle;
            }
            let projected = manager.query_item(key);
            assert_eq!(projected.as_ref(), &ItemQuery::NotIndexed);
            assert!(Arc::ptr_eq(&cached, &projected));
        };

        run_query("c:/library/running.png", None);
        run_query(
            "c:/library/complete.png",
            Some(IndexProgress::Complete(IndexReport::default())),
        );
        run_query(
            "c:/library/cancelled.png",
            Some(IndexProgress::Cancelled(IndexReport::default())),
        );
        run_query(
            "c:/library/failed.png",
            Some(IndexProgress::Failed("injected terminal".to_owned())),
        );
    }

    #[test]
    fn scheduler_reuses_one_db_owner_across_runs_and_purge_sees_old_prefill() {
        let temp = tempfile::tempdir().unwrap();
        let manager = SimilarIndexManager::new(temp.path().to_path_buf());
        let old_run_db = manager.scheduler.db_for_worker().unwrap();
        let prefill = row(1, "c:/library/old.png", [7; 32], 10).item;
        old_run_db.put_prefill(&prefill).unwrap();

        let new_run_db = manager.scheduler.db_for_worker().unwrap();
        assert!(Arc::ptr_eq(&old_run_db, &new_run_db));
        assert_eq!(
            new_run_db
                .purge_roots_except(&["c:/library".to_owned()], &[])
                .unwrap(),
            1
        );
        assert!(
            old_run_db
                .load_prefill(
                    &prefill.item_key,
                    prefill.mtime,
                    prefill.file_size,
                    prefill.hash_version,
                )
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn off_scope_update_completes_while_prefill_holds_db_then_rejects_insert() {
        let temp = tempfile::tempdir().unwrap();
        let manager = SimilarIndexManager::new(temp.path().to_path_buf());
        *manager.enabled_roots.write().unwrap() = vec!["c:/library".to_owned()];
        let db = manager.scheduler.db_for_worker().unwrap();
        let prepared = prepare_thumbnail_prefill(
            &manager.enabled_roots,
            Path::new("c:/library/racing.png"),
            None,
            None,
            10,
            20,
            &image::DynamicImage::new_rgba8(2, 2),
            (2, 2),
        )
        .unwrap()
        .expect("the in-scope offer is prepared as an owned value");
        let blocker = row(1, "c:/library/blocker.png", [8; 32], 10).item;
        let (db_owned_tx, db_owned_rx) = std::sync::mpsc::channel();
        let (resume_tx, resume_rx) = std::sync::mpsc::channel();
        let (off_done_tx, off_done_rx) = std::sync::mpsc::channel();

        std::thread::scope(|scope| {
            let blocking_db = Arc::clone(&db);
            let blocker = blocker.clone();
            let blocker_thread = scope.spawn(move || {
                blocking_db.put_prefill_if(&blocker, || {
                    db_owned_tx.send(()).unwrap();
                    resume_rx.recv().unwrap();
                    false
                })
            });
            db_owned_rx.recv().unwrap();

            let offer_db = Arc::clone(&db);
            let roots = Arc::clone(&manager.enabled_roots);
            let offered = &prepared;
            let offer_thread =
                scope.spawn(move || store_offered_thumbnail_prefill(&offer_db, &roots, offered));
            let scheduler = Arc::clone(&manager.scheduler);
            let off_thread = scope.spawn(move || {
                scheduler.configure(
                    Vec::new(),
                    crate::pdf_passwords::PdfPasswordStore::empty_for_test(),
                    None,
                    Vec::new(),
                );
                let _ = off_done_tx.send(());
            });
            let off_completed = off_done_rx.recv_timeout(Duration::from_secs(2)).is_ok();

            let _ = resume_tx.send(());
            let blocker_result = blocker_thread.join().unwrap().unwrap();
            let inserted = offer_thread.join().unwrap().unwrap();
            off_thread.join().unwrap();
            assert!(off_completed, "scope OFF must not wait for the DB mutex");
            assert!(!blocker_result, "the DB blocker is never inserted");
            assert!(!inserted, "the predicate must observe the latest OFF scope");
        });
        assert!(
            db.load_prefill(
                &prepared.item.item_key,
                prepared.item.mtime,
                prepared.item.file_size,
                prepared.item.hash_version,
            )
            .unwrap()
            .is_none()
        );
    }

    #[test]
    fn old_scope_insert_finishes_before_the_already_completed_off_purge() {
        let temp = tempfile::tempdir().unwrap();
        let manager = SimilarIndexManager::new(temp.path().to_path_buf());
        *manager.enabled_roots.write().unwrap() = vec!["c:/library".to_owned()];
        let db = manager.scheduler.db_for_worker().unwrap();
        let prefill = row(1, "c:/library/racing.png", [9; 32], 10).item;
        let (scope_released_tx, scope_released_rx) = std::sync::mpsc::channel();
        let (resume_tx, resume_rx) = std::sync::mpsc::channel();
        let (off_done_tx, off_done_rx) = std::sync::mpsc::channel();
        let (purge_started_tx, purge_started_rx) = std::sync::mpsc::channel();
        let (purge_done_tx, purge_done_rx) = std::sync::mpsc::channel();

        std::thread::scope(|scope| {
            let prefill_db = Arc::clone(&db);
            let roots = Arc::clone(&manager.enabled_roots);
            let candidate = prefill.clone();
            let writer = scope.spawn(move || {
                prefill_db.put_prefill_if(&candidate, || {
                    let allowed = {
                        let roots = roots.read().unwrap_or_else(|e| e.into_inner());
                        key_is_under_any(&candidate.item_key, &roots)
                    };
                    scope_released_tx.send(()).unwrap();
                    resume_rx.recv().unwrap();
                    allowed
                })
            });
            scope_released_rx.recv().unwrap();

            let scheduler = Arc::clone(&manager.scheduler);
            let off_thread = scope.spawn(move || {
                scheduler.configure(
                    Vec::new(),
                    crate::pdf_passwords::PdfPasswordStore::empty_for_test(),
                    None,
                    Vec::new(),
                );
                let _ = off_done_tx.send(());
            });
            let off_completed = off_done_rx.recv_timeout(Duration::from_secs(2)).is_ok();

            let mut purge_thread = None;
            let mut purge_waited_for_db = false;
            if off_completed {
                let purge_db = Arc::clone(&db);
                purge_thread = Some(scope.spawn(move || {
                    purge_started_tx.send(()).unwrap();
                    let removed = purge_db
                        .purge_roots_except(&["c:/library".to_owned()], &[])
                        .unwrap();
                    let _ = purge_done_tx.send(removed);
                }));
                purge_started_rx.recv().unwrap();
                purge_waited_for_db = purge_done_rx.try_recv().is_err();
            }

            let _ = resume_tx.send(());
            let inserted = writer.join().unwrap().unwrap();
            off_thread.join().unwrap();
            let removed = if let Some(purge_thread) = purge_thread {
                purge_thread.join().unwrap();
                Some(purge_done_rx.recv().unwrap())
            } else {
                None
            };
            assert!(off_completed, "scope OFF must complete while DB is held");
            assert!(
                purge_waited_for_db,
                "purge must serialize on the same DB owner"
            );
            assert!(inserted, "the already-read old scope may finish its insert");
            assert_eq!(
                removed,
                Some(1),
                "the queued purge must run after that insert"
            );
        });
        assert!(
            db.load_prefill(
                &prefill.item_key,
                prefill.mtime,
                prefill.file_size,
                prefill.hash_version,
            )
            .unwrap()
            .is_none(),
            "an old in-flight prefill must not resurrect the disabled root"
        );
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
        let ItemQuery::Ready(ItemMatches { hits, .. }) = query_test_rows(&rows, "origin") else {
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
        let ItemQuery::Ready(ItemMatches { hits, .. }) =
            query_item_ready(&db, &snapshot, "new-origin")
        else {
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
            if let ItemQuery::Ready(ItemMatches { hits, .. }) = result.as_ref() {
                assert_eq!(hits.len(), 1);
                assert_eq!(hits[0].item_key, near.item_key);
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!("item query worker did not publish its result");
    }

    /// 同じフレームで 2 ページを引いても、両方が結果まで辿り着くこと。
    ///
    /// 置き場が 1 枠だったとき、後の照会が前の照会を追い出し、返ってきた結果も捨てられて
    /// **どちらも永久に「読み込み中」**のまま毎フレーム worker を起こし続けた。見開きで
    /// 左右を同時に引くようにして初めて踏んだ。
    #[test]
    fn two_pages_queried_together_both_reach_a_result() {
        let temp = tempfile::tempdir().unwrap();
        let db_path = SimilarDb::db_path_at(temp.path());
        let db = SimilarDb::open_at(&db_path).unwrap();
        let left = row(1, "c:/library/left.png", [0; 32], 1).item;
        let right = row(2, "c:/library/right.png", [0x0f; 32], 1).item;
        let near_left = row(3, "c:/library/left-copy.png", [1; 32], 1).item;
        let near_right = row(4, "c:/library/right-copy.png", [0x0e; 32], 1).item;
        for item in [&left, &right, &near_left, &near_right] {
            db.upsert_loose_item(item).unwrap();
        }
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

        // 描画のたびに両方を引く、という UI と同じ順で回す。
        let mut settled = [None, None];
        for _ in 0..200 {
            for (slot, key) in [&left.item_key, &right.item_key].into_iter().enumerate() {
                let result = manager.query_item(key);
                if matches!(result.as_ref(), ItemQuery::Ready(_)) {
                    settled[slot] = Some(result);
                }
            }
            if settled.iter().all(Option::is_some) {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }

        for (slot, key) in [&left.item_key, &right.item_key].into_iter().enumerate() {
            let Some(result) = settled[slot].as_ref() else {
                panic!("page {key} never settled");
            };
            let ItemQuery::Ready(matches) = result.as_ref() else {
                unreachable!("only Ready is stored");
            };
            assert_eq!(&matches.origin.item_key, key);
            assert_eq!(matches.hits.len(), 1, "{key} should see its own copy");
        }

        // 両方が置き場に残っていること。片方を引いてももう片方が追い出されない。
        assert!(matches!(
            manager.query_item(&left.item_key).as_ref(),
            ItemQuery::Ready(_)
        ));
        assert!(matches!(
            manager.query_item(&right.item_key).as_ref(),
            ItemQuery::Ready(_)
        ));
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

        let ItemQuery::Ready(ItemMatches { hits, .. }) = query_item_ready(&db, &snapshot, "origin")
        else {
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
            if let ItemQuery::Ready(ItemMatches { hits, .. }) = result.as_ref()
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
            Vec::new(),
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
        let favorite_id = Uuid::new_v4();

        // Manager::configure 側の eager 呼び出しを通さず、scheduler の実行開始だけを使う。
        let library_key = crate::search_index_db::normalize_path(&library);
        manager.scheduler.configure(
            vec![ConfiguredRoot {
                favorite_id,
                path: library.clone(),
                key: library_key,
            }],
            crate::pdf_passwords::PdfPasswordStore::empty_for_test(),
            None,
            Vec::new(),
        );
        let registration = manager
            .scheduler
            .begin_watch(favorite_id, &library)
            .expect("configured root accepts its watcher");
        manager
            .scheduler
            .set_watch_health(&registration, WatchHealth::Ready, None);

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
        let publication = DeltaPublication::default();
        let context = ScanContext {
            db: &db,
            pdf_passwords: &passwords,
            cancel: &cancel,
            aggregate: &aggregate,
            report: IndexReport::default(),
            local_seen: ScanLocalSeen::Delta {
                items: HashSet::new(),
                containers: HashSet::new(),
            },
            excluded_root_keys: &[],
            prune_safe: true,
            array_refresh: &ArrayRefreshNotifier {
                scheduler: Weak::new(),
            },
            pass: ScanPass::Delta(&publication),
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

    fn wait_for_book_terminal(
        manager: &SimilarIndexManager,
        client: &SimilarBookQueryClient,
        container_key: &str,
    ) -> Arc<BookQuery> {
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        loop {
            let result = manager.query_book(client, container_key);
            if !matches!(result.as_ref(), BookQuery::Preparing) {
                return result;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "book query did not reach a terminal for {container_key}"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn product_book_query_missing_store_retries_after_soft_publication_without_ui_polling() {
        let temp = tempfile::tempdir().unwrap();
        let (notify_tx, notify_rx) = std::sync::mpsc::channel();
        let manager = SimilarIndexManager::new_with_book_query_notifier(
            temp.path().to_path_buf(),
            move || {
                let _ = notify_tx.send(());
            },
        );
        *manager.enabled_roots.write().unwrap() = vec!["c:/library".to_owned()];
        let client = SimilarBookQueryClient::new();

        assert_eq!(
            wait_for_book_terminal(&manager, &client, "c:/library/book").as_ref(),
            &BookQuery::NotIndexed
        );
        while notify_rx.try_recv().is_ok() {}

        let db = SimilarDb::open_at(&SimilarDb::db_path_at(temp.path())).unwrap();
        publish_book(
            &db,
            "c:/library/book",
            &[book_page("c:/library/book", 0, 7)],
        );
        manager.scheduler.book_query.soft_refresh();

        notify_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("soft publication must finish and repaint without another UI poll");
        assert!(matches!(
            manager.query_book(&client, "c:/library/book").as_ref(),
            BookQuery::Ready(_)
        ));
    }

    #[test]
    fn product_book_query_open_failure_is_per_request_and_does_not_stop_the_executor() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::create_dir(SimilarDb::db_path_at(temp.path())).unwrap();
        let manager = SimilarIndexManager::new(temp.path().to_path_buf());
        *manager.enabled_roots.write().unwrap() = vec!["c:/library".to_owned()];
        let first = SimilarBookQueryClient::new();
        let second = SimilarBookQueryClient::new();

        let first_result = wait_for_book_terminal(&manager, &first, "c:/library/a");
        let second_result = wait_for_book_terminal(&manager, &second, "c:/library/b");
        let BookQuery::Failed(first_error) = first_result.as_ref() else {
            panic!("open failure must be a typed per-request failure: {first_result:?}");
        };
        let BookQuery::Failed(second_error) = second_result.as_ref() else {
            panic!("the next client must also reach its own terminal: {second_result:?}");
        };
        assert!(first_error.contains("database open failed"));
        assert!(second_error.contains("database open failed"));
        assert!(!second_error.contains("executor stopped"));
    }

    #[test]
    fn product_book_query_loading_gate_keeps_request_until_terminal_memory_publication() {
        let temp = tempfile::tempdir().unwrap();
        let db = SimilarDb::open_at(&SimilarDb::db_path_at(temp.path())).unwrap();
        publish_book(
            &db,
            "c:/library/book",
            &[book_page("c:/library/book", 0, 9)],
        );
        let manager = SimilarIndexManager::new(temp.path().to_path_buf());
        *manager.enabled_roots.write().unwrap() = vec!["c:/library".to_owned()];
        *manager.memory.lock().unwrap() = MemoryState::Loading;
        let client = SimilarBookQueryClient::new();

        assert_eq!(
            manager.query_book(&client, "c:/library/book").as_ref(),
            &BookQuery::Preparing
        );
        std::thread::sleep(Duration::from_millis(50));
        assert_eq!(
            manager.query_book(&client, "c:/library/book").as_ref(),
            &BookQuery::Preparing
        );

        *manager.memory.lock().unwrap() = MemoryState::Missing;
        manager.scheduler.book_query.soft_refresh();
        assert!(matches!(
            wait_for_book_terminal(&manager, &client, "c:/library/book").as_ref(),
            BookQuery::Ready(_)
        ));
    }

    #[test]
    fn product_book_query_store_change_retires_retained_ready_and_requeues_only_active() {
        let temp = tempfile::tempdir().unwrap();
        let db_path = SimilarDb::db_path_at(temp.path());
        let db = SimilarDb::open_at(&db_path).unwrap();
        publish_book(
            &db,
            "c:/library/book",
            &[book_page("c:/library/book", 0, 11)],
        );
        let manager = SimilarIndexManager::new(temp.path().to_path_buf());
        *manager.enabled_roots.write().unwrap() = vec!["c:/library".to_owned()];
        *manager.memory.lock().unwrap() = MemoryState::Missing;
        let retained = SimilarBookQueryClient::new();
        let active = SimilarBookQueryClient::new();

        assert!(matches!(
            wait_for_book_terminal(&manager, &retained, "c:/library/book").as_ref(),
            BookQuery::Ready(_)
        ));
        retained.retain();
        assert!(matches!(
            wait_for_book_terminal(&manager, &active, "c:/library/book").as_ref(),
            BookQuery::Ready(_)
        ));
        let old_store = match *manager.scheduler.known_book_store.lock().unwrap() {
            ObservedBookStore::Known(store) => store,
            ObservedBookStore::Unobserved => panic!("completed query did not observe its store"),
        };
        let mut new_store = old_store;
        new_store[0] ^= 0xff;
        let writer = rusqlite::Connection::open(&db_path).unwrap();
        writer
            .execute(
                "UPDATE search_content_state SET store_id = ?1 WHERE singleton = 1",
                [new_store.as_slice()],
            )
            .unwrap();
        drop(writer);

        manager.scheduler.book_query.soft_refresh();
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        loop {
            let observed = *manager.scheduler.known_book_store.lock().unwrap();
            if observed == ObservedBookStore::Known(new_store)
                && matches!(
                    manager.query_book(&active, "c:/library/book").as_ref(),
                    BookQuery::Ready(_)
                )
            {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "store transition did not settle on the active client"
            );
            std::thread::sleep(Duration::from_millis(5));
        }

        assert_eq!(
            manager.query_book(&retained, "c:/library/book").as_ref(),
            &BookQuery::Preparing,
            "global hard invalidation must retire a retained client's old Ready"
        );
        assert!(matches!(
            wait_for_book_terminal(&manager, &retained, "c:/library/book").as_ref(),
            BookQuery::Ready(_)
        ));
    }

    #[test]
    fn product_book_query_inflight_cancellation_hides_a_and_dispatches_b_without_polling() {
        let temp = tempfile::tempdir().unwrap();
        let db = SimilarDb::open_at(&SimilarDb::db_path_at(temp.path())).unwrap();
        let origin_a = "c:/library/a";
        let origin_b = "c:/library/b";
        let pages_a = (0..1_500)
            .map(|page| book_page(origin_a, page, 0x55))
            .collect::<Vec<_>>();
        publish_book(&db, origin_a, &pages_a);
        publish_book(&db, origin_b, &[book_page(origin_b, 0, 0xaa)]);
        drop(db);

        let (notify_tx, notify_rx) = std::sync::mpsc::channel();
        let manager = SimilarIndexManager::new_with_book_query_notifier(
            temp.path().to_path_buf(),
            move || {
                let _ = notify_tx.send(());
            },
        );
        *manager.enabled_roots.write().unwrap() = vec!["c:/library".to_owned()];
        *manager.memory.lock().unwrap() = MemoryState::Missing;
        let (probe, mut phase_control) =
            crate::similar_book_query_test_probe::BookQueryTestPhaseProbe::new(
                crate::similar_book_query_test_probe::BookQueryTestPhase::SqlProgress,
            );
        manager.set_book_query_test_phase_probe(probe);
        let client_a = SimilarBookQueryClient::new();
        let client_b = SimilarBookQueryClient::new();

        assert_eq!(
            manager.query_book(&client_a, origin_a).as_ref(),
            &BookQuery::Preparing
        );
        phase_control.wait_reached();
        assert_eq!(
            manager.query_book(&client_b, origin_b).as_ref(),
            &BookQuery::Preparing
        );

        let (withdrawn_tx, withdrawn_rx) = std::sync::mpsc::channel();
        std::thread::scope(|scope| {
            let withdraw = scope.spawn(|| {
                client_a.withdraw();
                let _ = withdrawn_tx.send(());
            });
            let withdraw_result = withdrawn_rx.recv_timeout(Duration::from_secs(5));
            phase_control.release();
            let join_result = withdraw.join();
            withdraw_result.expect("withdrawing A blocked behind the query worker");
            join_result.unwrap();
        });

        notify_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("B did not publish a terminal without another UI poll");
        assert_eq!(
            client_a.demand_snapshot_for_test(),
            crate::similar_book_query::BookQueryDemandSnapshot::Withdrawn,
            "the cancelled A request must remain unpublished"
        );
        assert!(matches!(
            manager.query_book(&client_b, origin_b).as_ref(),
            BookQuery::Ready(_)
        ));
        assert!(
            notify_rx.try_recv().is_err(),
            "the cancelled A request must not publish a result notification"
        );
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

    #[derive(Debug, PartialEq, Eq)]
    struct DenseBookStripPage {
        state: BookPageState,
        other_page_index: Option<u32>,
        other_target: Option<SimilarItemTarget>,
        other_item_key: Option<String>,
        other_mtime: i64,
        other_file_size: i64,
    }

    fn dense_book_strip_oracle(
        origin_pages: &[crate::similar_db::SearchRow],
        candidate_pages: &[crate::similar_db::SearchRow],
        origin_matches: &BookMatchSet,
        pair: &dupe::book::BookPair,
    ) -> Vec<DenseBookStripPage> {
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
            .map(|(slot, row)| DenseBookStripPage {
                state: if row.item.quality < BOOK_MIN_QUALITY
                    || origin_matches
                        .pages
                        .get(slot)
                        .is_some_and(|page| page.origin_is_common)
                {
                    BookPageState::Excluded
                } else {
                    BookPageState::Unmatched
                },
                other_page_index: None,
                other_target: None,
                other_item_key: None,
                other_mtime: 0,
                other_file_size: 0,
            })
            .collect::<Vec<_>>();
        for &(origin_page, other_page) in &pair.alignment {
            let Some(&slot) = by_origin_page.get(&origin_page) else {
                continue;
            };
            let Some(other) = candidate_by_page.get(&other_page) else {
                continue;
            };
            let distance = hamming256(&origin_pages[slot].item.pdq256, &other.item.pdq256);
            strip[slot] = DenseBookStripPage {
                state: match match_band(distance) {
                    Some(MatchBand::NearlyIdentical) => BookPageState::Strong,
                    _ => BookPageState::Weak,
                },
                other_page_index: Some(other_page),
                other_target: resolved_target_for_item(&other.item),
                other_item_key: Some(other.item.item_key.clone()),
                other_mtime: other.item.mtime,
                other_file_size: other.item.file_size,
            };
        }
        strip
    }

    fn materialize_sparse_book_strip(
        relations: &BookRelations,
        hit: &BookRelationHit,
    ) -> Vec<DenseBookStripPage> {
        let strip = BookStripView::new(&relations.origin, hit);
        (0..strip.len())
            .map(|slot| {
                let page = strip.get(slot).expect("slot belongs to the origin");
                let matched = page.match_override();
                DenseBookStripPage {
                    state: page.state(),
                    other_page_index: matched.map(|matched| matched.other_page_index),
                    other_target: matched.and_then(|matched| matched.other_target.clone()),
                    other_item_key: matched.map(|matched| matched.other_item_key.clone()),
                    other_mtime: matched.map_or(0, |matched| matched.other_mtime),
                    other_file_size: matched.map_or(0, |matched| matched.other_file_size),
                }
            })
            .collect()
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

        let origin_pages = db
            .load_book_pages("book-a", current_hash_version())
            .unwrap();
        let candidate_pages = db
            .load_book_pages("book-b", current_hash_version())
            .unwrap();
        let origin_matches =
            collect_book_page_matches(&db, &snapshot, &origin_pages, current_hash_version())
                .unwrap();
        let mut corpus = BookCorpusBuilder::new("book-a", "book-b");
        corpus.add_book(BOOK_ORIGIN, &origin_pages);
        corpus.add_book(BOOK_CANDIDATE, &candidate_pages);
        corpus.add_context(&origin_matches);
        let pair = dupe::book::classify_pair(
            &corpus.pages,
            dupe::book::Params {
                radius: BOOK_RADIUS,
                max_books_per_page: BOOK_MAX_BOOKS_PER_PAGE,
                min_quality: BOOK_MIN_QUALITY,
                coverage_threshold: BOOK_COVERAGE,
                min_matched_pages: BOOK_MIN_MATCHED_PAGES,
            },
            BOOK_ORIGIN,
            BOOK_CANDIDATE,
        )
        .unwrap();
        let dense_oracle =
            dense_book_strip_oracle(&origin_pages, &candidate_pages, &origin_matches, &pair);

        let BookQuery::Ready(relations) = query_book_ready(&db, &snapshot, "book-a") else {
            panic!("expected a book result");
        };
        assert_eq!(relations.hits.len(), 1);
        assert_eq!(relations.hits[0].other_container_key, "book-b");
        assert_eq!(relations.hits[0].pair.relation, dupe::book::Relation::Same);
        assert_eq!(relations.hits[0].pair.matched, BOOK_MIN_MATCHED_PAGES);
        // 帯の添字と本のページ順が一致していること。ここがずれると、いま見ているページを
        // 帯の別の場所に指してしまう。
        let strip = BookStripView::new(&relations.origin, &relations.hits[0]);
        assert_eq!(relations.origin.pages.len(), strip.len());
        assert_eq!(relations.origin.pages[0].item_key, "book-a/0");
        assert_eq!(relations.hits[0].overrides().len(), 3);
        assert!((0..strip.len()).all(|slot| {
            strip
                .get(slot)
                .is_some_and(|page| page.state() == BookPageState::Strong)
        }));
        assert_eq!(
            materialize_sparse_book_strip(&relations, &relations.hits[0]),
            dense_oracle
        );
    }

    #[test]
    fn book_relation_hit_rejects_duplicate_and_out_of_range_overrides() {
        let pair = dupe::book::BookPair {
            a: BOOK_ORIGIN,
            b: BOOK_CANDIDATE,
            matched: 0,
            distinctive_a: 1,
            distinctive_b: 1,
            coverage_a: 0.0,
            coverage_b: 0.0,
            relation: dupe::book::Relation::Unrelated,
            alignment: Vec::new(),
        };
        let matched = |origin_slot| BookPageMatch {
            origin_slot,
            state: BookPageMatchState::Strong,
            other_page_index: 0,
            other_target: None,
            other_item_key: "other/0".to_owned(),
            other_mtime: 0,
            other_file_size: 0,
        };
        let sorted = BookRelationHit::new(
            "other".to_owned(),
            1,
            pair.clone(),
            vec![matched(2), matched(0)],
            3,
        )
        .unwrap();
        assert_eq!(
            sorted
                .overrides()
                .iter()
                .map(|page| page.origin_slot)
                .collect::<Vec<_>>(),
            vec![0, 2]
        );
        assert!(
            BookRelationHit::new(
                "other".to_owned(),
                1,
                pair.clone(),
                vec![matched(0), matched(0)],
                1,
            )
            .unwrap_err()
            .contains("duplicate")
        );
        assert!(
            BookRelationHit::new("other".to_owned(), 1, pair, vec![matched(1)], 1)
                .unwrap_err()
                .contains("outside")
        );
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

    fn prepared_scale_fixture_dir(name: &str) -> PathBuf {
        use std::os::windows::fs::MetadataExt;

        let run = std::env::var_os("MIV_BOOK_QUERY_SCALE_RUN_DIR")
            .map(PathBuf::from)
            .expect("set MIV_BOOK_QUERY_SCALE_RUN_DIR to the prepared fresh run directory");
        let run = std::fs::canonicalize(&run).expect("scale run directory must already exist");
        let directory = std::fs::canonicalize(run.join(name)).unwrap_or_else(|error| {
            panic!("prepared fixture directory {name} is missing: {error}")
        });
        assert_eq!(directory.parent(), Some(run.as_path()));
        for ancestor in directory.ancestors() {
            let metadata = std::fs::symlink_metadata(ancestor).unwrap();
            assert_eq!(
                metadata.file_attributes() & 0x400,
                0,
                "fixture ancestors must not be reparse points: {}",
                ancestor.display()
            );
            if ancestor == run {
                break;
            }
        }
        assert!(std::fs::read_dir(&directory).unwrap().next().is_none());
        directory
    }

    fn near_white_page_prototype() -> StoredItem {
        let mut raster = image::RgbaImage::from_pixel(64, 64, image::Rgba([255, 255, 255, 255]));
        raster.put_pixel(32, 32, image::Rgba([0, 0, 0, 255]));
        let raster = image::DynamicImage::ImageRgba8(raster);
        let make = || {
            let canonical = proxy_from_source(
                ProxySource::Raster {
                    image: &raster,
                    source_dims: (64, 64),
                    format: SimilarImageFormat::Png,
                },
                None,
            )
            .unwrap();
            stored_from_proxy(
                "c:/library/prototype.png",
                ItemKind::Image,
                Some("c:/library/prototype"),
                Some(0),
                &FileCandidate {
                    path: PathBuf::from("c:/library/prototype.png"),
                    mtime: 1,
                    file_size: 1,
                },
                canonical,
            )
            .unwrap()
        };
        let first = make();
        let second = make();
        assert!(
            first.quality > 0,
            "the real near-white PDQ proxy must be eligible"
        );
        assert_eq!(first.pdq256, second.pdq256);
        assert_eq!(first.quality, second.quality);
        first
    }

    #[test]
    fn near_white_scale_raster_uses_a_repeatable_eligible_real_pdq_proxy() {
        let prototype = near_white_page_prototype();
        assert!(prototype.quality > 0);
        assert_ne!(prototype.pdq256, [0; 32]);
    }

    fn scale_fixture_input(role: &str, path: &Path) -> serde_json::Value {
        use sha2::{Digest as _, Sha256};
        use std::io::Read as _;

        let metadata = std::fs::metadata(path).unwrap();
        assert!(metadata.is_file());
        let mut file = std::fs::File::open(path).unwrap();
        let mut digest = Sha256::new();
        let mut buffer = vec![0u8; 1024 * 1024];
        loop {
            let read = file.read(&mut buffer).unwrap();
            if read == 0 {
                break;
            }
            digest.update(&buffer[..read]);
        }
        serde_json::json!({
            "role": role,
            "file": path.file_name().unwrap().to_string_lossy(),
            "bytes": metadata.len(),
            "sha256": format!("{:x}", digest.finalize()),
        })
    }

    fn publish_repeated_near_white_book(
        db: &SimilarDb,
        prototype: &StoredItem,
        container_key: &str,
        pages: usize,
    ) {
        let items = (0..pages)
            .map(|page| {
                let mut item = prototype.clone();
                item.item_key = format!("{container_key}/{page:05}.png");
                item.container_key = Some(container_key.to_owned());
                item.page_index = Some(page as u32);
                item.mtime = 1 + page as i64;
                item
            })
            .collect::<Vec<_>>();
        publish_book(db, container_key, &items);
    }

    fn finish_near_white_fixture(
        directory: &Path,
        label: &str,
        origin_key: &str,
        pages: usize,
        candidates: usize,
        signature: [u8; 32],
        quality: u8,
    ) {
        let path = directory.join("similar.db");
        let connection = rusqlite::Connection::open(&path).unwrap();
        let journal: String = connection
            .query_row("PRAGMA journal_mode=DELETE", [], |row| row.get(0))
            .unwrap();
        assert_eq!(journal.to_ascii_lowercase(), "delete");
        let quick_check: String = connection
            .query_row("PRAGMA quick_check", [], |row| row.get(0))
            .unwrap();
        assert_eq!(quick_check, "ok");
        drop(connection);
        assert!(!directory.join("similar.db-wal").exists());
        assert!(!directory.join("similar.db-shm").exists());
        let inputs = [
            scale_fixture_input("database", &path),
            scale_fixture_input("base", &directory.join("similar.base")),
        ];
        let file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(directory.join("fixture.json"))
            .unwrap();
        serde_json::to_writer_pretty(
            file,
            &serde_json::json!({
                "schema": 1,
                "fixture": label,
                "origin_key": origin_key,
                "origin_pages": pages,
                "candidate_count": candidates,
                "pdq256_hex": signature.iter().map(|byte| format!("{byte:02x}")).collect::<String>(),
                "quality": quality,
                "inputs": inputs,
                "journal_mode": "delete",
            }),
        )
        .unwrap();
    }

    #[test]
    #[ignore = "generates persistent real-proxy near-white 8-book and 9-book scale stores"]
    fn generate_and_verify_near_white_common_boundary_fixtures() {
        let prototype = near_white_page_prototype();
        let root = "c:/library";
        for (name, total_books, expected_hits, common) in [
            ("near-white-rare-8", 8usize, 7usize, false),
            ("near-white-common-9", 9usize, 0usize, true),
        ] {
            let directory = prepared_scale_fixture_dir(name);
            let path = directory.join("similar.db");
            let db = SimilarDb::open_at(&path).unwrap();
            let origin_key = format!("{root}/{name}-origin");
            publish_repeated_near_white_book(&db, &prototype, &origin_key, 400);
            for candidate in 0..(total_books - 1) {
                publish_repeated_near_white_book(
                    &db,
                    &prototype,
                    &format!("{root}/{name}-candidate-{candidate:02}"),
                    400,
                );
            }
            let snapshot = Arc::new(
                similar_search_array::rebuild_from_sqlite(&db, &directory.join("similar.base"))
                    .unwrap(),
            );
            drop(db);
            let mut engine = SimilarBookQueryEngine::open_at(&path).unwrap().unwrap();
            let observation = engine
                .query(
                    &origin_key,
                    &[root.to_owned()],
                    &[snapshot],
                    Arc::new(AtomicBool::new(false)),
                )
                .unwrap();
            let BookQuery::Ready(relations) = observation.outcome.unwrap() else {
                panic!("{name} must be Ready");
            };
            assert_eq!(relations.origin.pages.len(), 400);
            assert_eq!(relations.hits.len(), expected_hits);
            assert_eq!(
                relations
                    .origin
                    .pages
                    .iter()
                    .filter(|page| page.baseline == BookPageBaseline::Excluded)
                    .count(),
                if common { 400 } else { 0 }
            );
            for hit in &relations.hits {
                assert_eq!(hit.other_page_count, 400);
                assert_eq!(hit.pair.matched, 400);
                assert_eq!(hit.pair.distinctive_a, 400);
                assert_eq!(hit.pair.distinctive_b, 400);
                assert_eq!(hit.pair.coverage_a, 1.0);
                assert_eq!(hit.pair.coverage_b, 1.0);
                assert_eq!(hit.pair.relation, dupe::book::Relation::Same);
                assert_eq!(hit.pair.alignment.len(), 400);
                assert_eq!(hit.overrides().len(), 400);
            }
            drop(engine);
            finish_near_white_fixture(
                &directory,
                name,
                &origin_key,
                400,
                expected_hits,
                prototype.pdq256,
                prototype.quality,
            );
        }
    }

    fn coordinator_state_with_watch(
        favorite_id: Uuid,
        root: &Path,
        health: WatchHealth,
    ) -> SchedulerState {
        let configured_root = ConfiguredRoot {
            favorite_id,
            path: root.to_path_buf(),
            key: crate::search_index_db::normalize_path(root),
        };
        let mut state = SchedulerState {
            desired_config: Some(SchedulerConfig {
                roots: vec![configured_root.clone()],
                excluded_roots: Vec::new(),
                excluded_root_keys: Vec::new(),
                pdf_passwords: crate::pdf_passwords::PdfPasswordStore::empty_for_test(),
                password_revision: 0,
                activity_gate: None,
            }),
            config_epoch: 7,
            ..SchedulerState::default()
        };
        state.watch_by_root.insert(
            favorite_id,
            RootWatchState {
                root_key: configured_root.key,
                registration_generation: 11,
                health,
                gap_epoch: 0,
                repaired_gap_epoch: 0,
            },
        );
        state
    }

    fn initial_full_intent() -> FullIntent {
        FullIntent {
            config_epoch: 7,
            required_gap_epoch: 0,
            reason: FullReason::Initial,
        }
    }

    #[test]
    fn incremental_reconcile_full_watermark_absorbs_only_events_at_or_before_its_start() {
        let favorite_id = Uuid::new_v4();
        let root = PathBuf::from("c:/library");
        let mut state = coordinator_state_with_watch(favorite_id, &root, WatchHealth::Ready);
        let scope = DirtyScope::DirectoryContents(root.join("book"));
        state.next_event_seq = 1;
        state.dirty.insert(scope.clone(), 1);
        state.pending_full = Some(initial_full_intent());

        let plan = state.take_next_job().expect("initial Full is runnable");
        assert!(matches!(plan.running.kind, ReconcileJobKind::Full(_)));
        assert!(plan.running.repairs_watch_gap);
        assert_eq!(plan.running.start_event_seq, 1);
        state.next_event_seq = 2;
        state.dirty.insert(scope.clone(), 2);

        assert_eq!(
            state.finish_successful_job(&plan),
            SuccessfulJobDisposition::MoreWork
        );
        assert_eq!(state.dirty.latest_by_scope.get(&scope), Some(&2));
        state.phase = SchedulerPhase::Idle;
        let next = state
            .take_next_job()
            .expect("post-Full event remains dirty");
        assert!(matches!(next.running.kind, ReconcileJobKind::Delta));
        assert_eq!(next.dirty.latest_by_scope.get(&scope), Some(&2));
    }

    #[test]
    fn incremental_reconcile_dominated_child_promotes_its_post_full_sequence() {
        let favorite_id = Uuid::new_v4();
        let root = PathBuf::from("c:/library");
        let mut state = coordinator_state_with_watch(favorite_id, &root, WatchHealth::Ready);
        let ancestor = DirtyScope::Subtree(root.join("book"));
        state.next_event_seq = 1;
        state.dirty.insert(ancestor.clone(), 1);
        state.pending_full = Some(initial_full_intent());
        let plan = state.take_next_job().expect("Full starts at sequence 1");

        state.next_event_seq = 2;
        state
            .dirty
            .insert(DirtyScope::DirectoryContents(root.join("book/chapter")), 2);
        assert_eq!(state.dirty.latest_by_scope.get(&ancestor), Some(&2));
        assert_eq!(
            state.finish_successful_job(&plan),
            SuccessfulJobDisposition::MoreWork
        );
        assert_eq!(state.dirty.latest_by_scope.get(&ancestor), Some(&2));
    }

    #[test]
    fn incremental_reconcile_full_intent_keeps_the_newest_root_gap_and_reason() {
        let mut state = SchedulerState {
            config_epoch: 7,
            ..SchedulerState::default()
        };
        state.merge_full_intent(FullIntent {
            config_epoch: 7,
            required_gap_epoch: 9,
            reason: FullReason::Overflow,
        });
        state.merge_full_intent(FullIntent {
            config_epoch: 7,
            required_gap_epoch: 3,
            reason: FullReason::WatchRecovery,
        });
        assert_eq!(
            state.pending_full,
            Some(FullIntent {
                config_epoch: 7,
                required_gap_epoch: 9,
                reason: FullReason::Overflow,
            })
        );
    }

    #[test]
    fn incremental_reconcile_older_ready_transition_cannot_replace_another_roots_overflow() {
        let temp = tempfile::tempdir().unwrap();
        let manager = SimilarIndexManager::new(temp.path().join("similar"));
        let root_a = temp.path().join("a");
        let root_b = temp.path().join("b");
        let favorite_a = Uuid::new_v4();
        let favorite_b = Uuid::new_v4();
        let mut state = coordinator_state_with_watch(favorite_a, &root_a, WatchHealth::Pending);
        let configured_b = ConfiguredRoot {
            favorite_id: favorite_b,
            path: root_b.clone(),
            key: crate::search_index_db::normalize_path(&root_b),
        };
        state
            .desired_config
            .as_mut()
            .unwrap()
            .roots
            .push(configured_b.clone());
        state.watch_by_root.get_mut(&favorite_a).unwrap().gap_epoch = 3;
        state.watch_by_root.insert(
            favorite_b,
            RootWatchState {
                root_key: configured_b.key,
                registration_generation: 12,
                health: WatchHealth::Ready,
                gap_epoch: 9,
                repaired_gap_epoch: 0,
            },
        );
        state.next_gap_epoch = 9;
        state.pending_full = Some(FullIntent {
            config_epoch: 7,
            required_gap_epoch: 9,
            reason: FullReason::Overflow,
        });
        state.phase = SchedulerPhase::Running(RunningReconcileJob {
            kind: ReconcileJobKind::Delta,
            config_epoch: 7,
            start_event_seq: 0,
            repairs_watch_gap: false,
            cancel: Arc::new(AtomicBool::new(false)),
        });
        *manager.scheduler.state.lock().unwrap() = state;
        let registration_a = SimilarWatchRegistration {
            favorite_id: favorite_a,
            root_key: crate::search_index_db::normalize_path(&root_a),
            registration_generation: 11,
        };

        manager
            .scheduler
            .set_watch_health(&registration_a, WatchHealth::Ready, None);
        assert_eq!(
            manager.scheduler.state.lock().unwrap().pending_full,
            Some(FullIntent {
                config_epoch: 7,
                required_gap_epoch: 9,
                reason: FullReason::Overflow,
            })
        );
    }

    #[test]
    fn incremental_reconcile_purge_completion_waits_for_the_watch_barrier() {
        let favorite_id = Uuid::new_v4();
        let root = PathBuf::from("c:/library");
        let mut state = coordinator_state_with_watch(favorite_id, &root, WatchHealth::Pending);
        state.pending_full = Some(initial_full_intent());
        state.pending_purge_roots.insert(root.join("removed"));
        let plan = state
            .take_next_job()
            .expect("Purge may run before watch Ready");
        assert!(matches!(plan.running.kind, ReconcileJobKind::Purge));
        assert_eq!(
            state.finish_successful_job(&plan),
            SuccessfulJobDisposition::AwaitingWatch
        );
    }

    #[test]
    fn incremental_reconcile_dirty_burst_coalesces_to_a_bounded_root_repair() {
        let favorite_id = Uuid::new_v4();
        let root = PathBuf::from("c:/library");
        let mut state = coordinator_state_with_watch(favorite_id, &root, WatchHealth::Ready);
        let active_cancel = Arc::new(AtomicBool::new(false));
        state.phase = SchedulerPhase::Running(RunningReconcileJob {
            kind: ReconcileJobKind::Full(initial_full_intent()),
            config_epoch: 7,
            start_event_seq: 0,
            repairs_watch_gap: false,
            cancel: Arc::clone(&active_cancel),
        });
        state.dirty.insert(DirtyScope::Subtree(root.clone()), 0);
        for sequence in 1..=10_000_u64 {
            state.next_event_seq = sequence;
            state.dirty.insert(
                DirtyScope::RemovedPrefix(root.join(format!("removed-{sequence}"))),
                sequence,
            );
            state.bound_dirty_scopes(Some(root.clone()));
        }
        assert_eq!(state.dirty.len(), 1);
        assert_eq!(
            state
                .dirty
                .latest_by_scope
                .get(&DirtyScope::RootRepair(root.clone())),
            Some(&10_000)
        );
        assert!(
            !state
                .dirty
                .latest_by_scope
                .contains_key(&DirtyScope::Subtree(root)),
            "RootRepair must replace an existing Subtree so destructive children are bounded"
        );
        assert!(!active_cancel.load(Ordering::Acquire));
    }

    #[test]
    fn incremental_reconcile_delta_failure_merges_running_and_new_scopes() {
        let favorite_id = Uuid::new_v4();
        let root = PathBuf::from("c:/library");
        let mut state = coordinator_state_with_watch(favorite_id, &root, WatchHealth::Ready);
        let first = DirtyScope::DirectoryContents(root.join("first"));
        let second = DirtyScope::Subtree(root.join("second"));
        state.next_event_seq = 1;
        state.dirty.insert(first.clone(), 1);
        let plan = state.take_next_job().expect("dirty scope starts Delta");
        assert!(matches!(plan.running.kind, ReconcileJobKind::Delta));
        state.next_event_seq = 2;
        state.dirty.insert(second.clone(), 2);

        state.restore_unfinished_job(&plan, false);
        assert_eq!(state.dirty.latest_by_scope.get(&first), Some(&1));
        assert_eq!(state.dirty.latest_by_scope.get(&second), Some(&2));
    }

    #[test]
    fn incremental_reconcile_cancelled_full_keeps_the_newer_overflow_repair() {
        let favorite_id = Uuid::new_v4();
        let root = PathBuf::from("c:/library");
        let mut state = coordinator_state_with_watch(favorite_id, &root, WatchHealth::Ready);
        state.pending_full = Some(initial_full_intent());
        let plan = state.take_next_job().expect("initial Full starts");
        state.pending_full = Some(FullIntent {
            config_epoch: 7,
            required_gap_epoch: 9,
            reason: FullReason::Overflow,
        });

        state.restore_unfinished_job(&plan, false);
        assert_eq!(state.pending_full.unwrap().required_gap_epoch, 9);
    }

    #[test]
    fn incremental_reconcile_skipped_purge_survives_a_new_configuration_epoch() {
        let favorite_id = Uuid::new_v4();
        let root = PathBuf::from("c:/library");
        let removed = root.join("removed");
        let mut state = coordinator_state_with_watch(favorite_id, &root, WatchHealth::Ready);
        state.pending_purge_roots.insert(removed.clone());
        state.pending_full = Some(initial_full_intent());
        let plan = state
            .take_next_job()
            .expect("Purge precedes replacement Full");
        assert!(matches!(plan.running.kind, ReconcileJobKind::Purge));

        state.config_epoch = 8;
        state.restore_unfinished_job(&plan, false);
        assert!(
            state.pending_purge_roots.contains(&removed),
            "an uncommitted purge obligation survives into the new epoch"
        );
    }

    #[test]
    fn incremental_reconcile_watcher_barrier_has_a_finite_degraded_terminal() {
        let favorite_id = Uuid::new_v4();
        let root = PathBuf::from("c:/library");
        let mut state = coordinator_state_with_watch(favorite_id, &root, WatchHealth::Pending);
        state.pending_full = Some(initial_full_intent());
        assert!(!state.has_runnable_work());

        let watch = state.watch_by_root.get_mut(&favorite_id).unwrap();
        watch.health = WatchHealth::Unavailable;
        watch.gap_epoch = 1;
        state.next_gap_epoch = 1;
        state.pending_full = Some(FullIntent {
            config_epoch: 7,
            required_gap_epoch: 1,
            reason: FullReason::WatchRecovery,
        });
        assert!(state.has_runnable_work());
        let plan = state
            .take_next_job()
            .expect("Unavailable is a terminal barrier state");
        assert!(!plan.running.repairs_watch_gap);
        assert_eq!(
            state.finish_successful_job(&plan),
            SuccessfulJobDisposition::DegradedWatch
        );
        assert_eq!(
            state
                .watch_by_root
                .get(&favorite_id)
                .unwrap()
                .repaired_gap_epoch,
            0,
            "a Full that finishes while the watcher is unavailable cannot close its gap"
        );
    }

    #[test]
    fn incremental_reconcile_subtree_dominates_non_destructive_descendants() {
        let root = PathBuf::from("c:/library/book");
        let mut dirty = DirtyScopeSet::default();
        dirty.insert(DirtyScope::DirectoryContents(root.clone()), 1);
        dirty.insert(DirtyScope::DirectoryContents(root.join("chapter")), 2);
        dirty.insert(DirtyScope::RemovedPrefix(root.join("gone")), 3);
        dirty.insert(DirtyScope::Subtree(root.clone()), 4);

        assert_eq!(dirty.latest_by_scope.len(), 2);
        assert_eq!(
            dirty
                .latest_by_scope
                .get(&DirtyScope::Subtree(root.clone())),
            Some(&4)
        );
        assert_eq!(
            dirty
                .latest_by_scope
                .get(&DirtyScope::RemovedPrefix(root.join("gone"))),
            Some(&3)
        );

        let mut equivalent = DirtyScopeSet::default();
        equivalent.insert(
            DirtyScope::DirectoryContents(PathBuf::from("C:/LIBRARY/BOOK")),
            1,
        );
        equivalent.insert(
            DirtyScope::DirectoryContents(PathBuf::from("c:/library/book")),
            2,
        );
        assert_eq!(equivalent.latest_by_scope.len(), 1);
        assert_eq!(equivalent.latest_by_scope.values().copied().next(), Some(2));
    }

    #[test]
    fn incremental_reconcile_overflow_coalesces_and_rejects_old_watch_tokens() {
        let temp = tempfile::tempdir().unwrap();
        let manager = SimilarIndexManager::new(temp.path().to_path_buf());
        let favorite_id = Uuid::new_v4();
        let root = temp.path().join("library");
        let mut state = coordinator_state_with_watch(favorite_id, &root, WatchHealth::Ready);
        let active_cancel = Arc::new(AtomicBool::new(false));
        state.phase = SchedulerPhase::Running(RunningReconcileJob {
            kind: ReconcileJobKind::Delta,
            config_epoch: 7,
            start_event_seq: 0,
            repairs_watch_gap: false,
            cancel: Arc::clone(&active_cancel),
        });
        *manager.scheduler.state.lock().unwrap() = state;
        let current = SimilarWatchRegistration {
            favorite_id,
            root_key: crate::search_index_db::normalize_path(&root),
            registration_generation: 11,
        };

        manager
            .scheduler
            .request_full(&current, FullReason::Overflow);
        manager
            .scheduler
            .request_full(&current, FullReason::Overflow);
        let after_overflow = manager.scheduler.state.lock().unwrap();
        assert!(active_cancel.load(Ordering::Acquire));
        assert_eq!(after_overflow.next_gap_epoch, 2);
        assert_eq!(after_overflow.pending_full.unwrap().required_gap_epoch, 2);
        drop(after_overflow);

        let stale = SimilarWatchRegistration {
            registration_generation: 10,
            ..current
        };
        manager.scheduler.request_full(&stale, FullReason::Manual);
        assert_eq!(
            manager
                .scheduler
                .state
                .lock()
                .unwrap()
                .pending_full
                .unwrap()
                .required_gap_epoch,
            2
        );
    }

    #[test]
    fn incremental_reconcile_replacing_a_ready_watch_records_one_repair_gap() {
        for awaiting_array in [false, true] {
            let temp = tempfile::tempdir().unwrap();
            let manager = SimilarIndexManager::new(temp.path().to_path_buf());
            let favorite_id = Uuid::new_v4();
            let root = temp.path().join("library");
            let mut state = coordinator_state_with_watch(favorite_id, &root, WatchHealth::Ready);
            state.pending_full = Some(initial_full_intent());
            let plan = state.take_next_job().expect("initial Full starts");
            if awaiting_array {
                state.phase = SchedulerPhase::AwaitingArray(plan.running.clone());
            }
            *manager.scheduler.state.lock().unwrap() = state;
            *manager.scheduler.progress.lock().unwrap() = if awaiting_array {
                IndexProgress::AwaitingArray(IndexReport::default())
            } else {
                IndexProgress::Running(RunningProgress {
                    stage: IndexStage::Scanning,
                    current_path: None,
                    report: IndexReport::default(),
                })
            };

            manager
                .scheduler
                .begin_watch(favorite_id, &root)
                .expect("configured watch can be replaced");
            assert!(plan.running.cancel.load(Ordering::Acquire));
            assert_eq!(
                manager
                    .scheduler
                    .settle_interrupted_plan(&plan, false, IndexReport::default(),),
                InterruptedJobDisposition::AwaitingWatch
            );
            let state = manager.scheduler.state.lock().unwrap();
            let watch = state.watch_by_root.get(&favorite_id).unwrap();
            assert_eq!(watch.health, WatchHealth::Pending);
            assert_eq!(watch.gap_epoch, 1);
            assert_eq!(state.next_gap_epoch, 1);
            assert_eq!(state.pending_full.unwrap().required_gap_epoch, 1);
            assert!(matches!(state.phase, SchedulerPhase::Idle));
            assert!(matches!(
                manager.scheduler.progress.lock().unwrap().clone(),
                IndexProgress::AwaitingWatch(_)
            ));
        }
    }

    #[test]
    fn incremental_reconcile_unknown_upsert_remains_a_recursive_scope() {
        let temp = tempfile::tempdir().unwrap();
        let manager = SimilarIndexManager::new(temp.path().join("similar"));
        let favorite_id = Uuid::new_v4();
        let root = temp.path().join("library");
        let changed = root.join("book");
        let mut state = coordinator_state_with_watch(favorite_id, &root, WatchHealth::Ready);
        state.phase = SchedulerPhase::Running(RunningReconcileJob {
            kind: ReconcileJobKind::Delta,
            config_epoch: 7,
            start_event_seq: 0,
            repairs_watch_gap: false,
            cancel: Arc::new(AtomicBool::new(false)),
        });
        *manager.scheduler.state.lock().unwrap() = state;
        let registration = SimilarWatchRegistration {
            favorite_id,
            root_key: crate::search_index_db::normalize_path(&root),
            registration_generation: 11,
        };

        manager.scheduler.request_change_observed(
            &registration,
            changed.clone(),
            crate::search_watcher::ChangeKind::Upsert,
            Some(ChangedPathObservation::Unknown),
        );
        assert!(
            manager
                .scheduler
                .state
                .lock()
                .unwrap()
                .dirty
                .latest_by_scope
                .contains_key(&DirtyScope::Subtree(changed))
        );
    }

    #[test]
    fn incremental_reconcile_missing_subtree_reobservation_prunes_nested_prefix() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("library");
        std::fs::create_dir_all(&root).unwrap();
        let missing = root.join("removed-book");
        let stale_path = missing.join("nested").join("old.png");
        let favorite_id = Uuid::new_v4();
        let state = coordinator_state_with_watch(favorite_id, &root, WatchHealth::Ready);
        let config = state.desired_config.unwrap();
        let db = crate::similar_db::SimilarDb::open_at(&temp.path().join("similar.db")).unwrap();
        let stale_key = crate::search_index_db::normalize_path(&stale_path);
        let stale = row(1, &stale_key, [7; 32], 10).item;
        db.upsert_loose_item(&stale).unwrap();
        let mut dirty = DirtyScopeSet::default();
        dirty.insert(DirtyScope::Subtree(missing), 1);

        let outcome = run_delta_index_job(
            &db,
            &dirty,
            &config,
            &Arc::new(AtomicBool::new(false)),
            &Arc::new(Mutex::new(IndexProgress::Idle)),
            &ArrayRefreshNotifier {
                scheduler: Weak::new(),
            },
        )
        .unwrap();

        assert!(outcome.prune_safe);
        assert_eq!(outcome.report.removed, 1);
        assert_eq!(
            db.full_inventory_load_count(),
            0,
            "Delta must never load the full reconcile inventory"
        );
        assert!(
            db.load_search_rows(current_hash_version())
                .unwrap()
                .is_empty(),
            "confirmed Subtree disappearance must prune every nested stale row"
        );
    }

    #[test]
    fn incremental_reconcile_root_repair_keeps_dirty_work_when_reobservation_is_incomplete() {
        let temp = tempfile::tempdir().unwrap();
        let configured_root = temp.path().join("configured-root");
        std::fs::write(&configured_root, b"temporarily not a directory").unwrap();
        let favorite_id = Uuid::new_v4();
        let state = coordinator_state_with_watch(favorite_id, &configured_root, WatchHealth::Ready);
        let config = state.desired_config.unwrap();
        let mut dirty = DirtyScopeSet::default();
        dirty.insert(DirtyScope::RootRepair(configured_root), 1);
        let db = crate::similar_db::SimilarDb::open_at(&temp.path().join("similar.db")).unwrap();
        let progress = Arc::new(Mutex::new(IndexProgress::Idle));

        let outcome = run_delta_index_job(
            &db,
            &dirty,
            &config,
            &Arc::new(AtomicBool::new(false)),
            &progress,
            &ArrayRefreshNotifier {
                scheduler: Weak::new(),
            },
        )
        .unwrap();
        assert!(!outcome.prune_safe);
    }

    #[test]
    fn incremental_reconcile_missing_upsert_side_of_directory_rename_owns_removed_prefix() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("library");
        let old = root.join("old-book");
        let renamed = root.join("renamed-book");
        std::fs::create_dir_all(&old).unwrap();
        std::fs::rename(&old, &renamed).unwrap();
        let manager = SimilarIndexManager::new(temp.path().join("similar"));
        let favorite_id = Uuid::new_v4();
        let mut state = coordinator_state_with_watch(favorite_id, &root, WatchHealth::Ready);
        state.phase = SchedulerPhase::Running(RunningReconcileJob {
            kind: ReconcileJobKind::Delta,
            config_epoch: 7,
            start_event_seq: 0,
            repairs_watch_gap: false,
            cancel: Arc::new(AtomicBool::new(false)),
        });
        *manager.scheduler.state.lock().unwrap() = state;
        let registration = SimilarWatchRegistration {
            favorite_id,
            root_key: crate::search_index_db::normalize_path(&root),
            registration_generation: 11,
        };

        manager.scheduler.request_change(
            &registration,
            old.clone(),
            crate::search_watcher::ChangeKind::Upsert,
        );

        let state = manager.scheduler.state.lock().unwrap();
        assert!(
            state
                .dirty
                .latest_by_scope
                .contains_key(&DirtyScope::RemovedPrefix(old))
        );
        assert!(
            state
                .dirty
                .latest_by_scope
                .contains_key(&DirtyScope::DirectoryContents(root))
        );
    }

    #[test]
    fn incremental_reconcile_array_ack_requires_store_and_change_sequence() {
        let snapshot = SearchSnapshot::from_base(crate::similar_search_array::BaseArray {
            records: Vec::new().into_boxed_slice(),
            store_id: [3; 16],
            applied_seq: 8,
        });
        assert!(SimilarIndexScheduler::snapshot_reaches(
            &snapshot,
            crate::similar_db::StoreWatermark {
                store_id: [3; 16],
                through_change_seq: 8,
            }
        ));
        assert!(!SimilarIndexScheduler::snapshot_reaches(
            &snapshot,
            crate::similar_db::StoreWatermark {
                store_id: [3; 16],
                through_change_seq: 9,
            }
        ));
        assert!(!SimilarIndexScheduler::snapshot_reaches(
            &snapshot,
            crate::similar_db::StoreWatermark {
                store_id: [4; 16],
                through_change_seq: 1,
            }
        ));
    }

    #[test]
    fn incremental_reconcile_cancelled_array_wait_is_not_a_publication_failure() {
        let temp = tempfile::tempdir().unwrap();
        let manager = SimilarIndexManager::new(temp.path().to_path_buf());
        let favorite_id = Uuid::new_v4();
        let root = temp.path().join("library");
        let mut state = coordinator_state_with_watch(favorite_id, &root, WatchHealth::Ready);
        state.next_event_seq = 1;
        state
            .dirty
            .insert(DirtyScope::DirectoryContents(root.join("changed")), 1);
        let plan = state.take_next_job().expect("Delta starts");
        state.phase = SchedulerPhase::AwaitingArray(plan.running.clone());
        *manager.scheduler.state.lock().unwrap() = state;
        *manager.scheduler.progress.lock().unwrap() =
            IndexProgress::AwaitingArray(IndexReport::default());
        let registration = SimilarWatchRegistration {
            favorite_id,
            root_key: crate::search_index_db::normalize_path(&root),
            registration_generation: 11,
        };
        manager
            .scheduler
            .request_full(&registration, FullReason::Manual);
        let watermark = crate::similar_db::StoreWatermark {
            store_id: [7; 16],
            through_change_seq: 1,
        };
        assert_eq!(
            manager
                .scheduler
                .wait_for_array_ack(watermark, &plan.running.cancel),
            Err(ArrayAckWaitError::Cancelled)
        );
        assert_eq!(
            manager
                .scheduler
                .settle_interrupted_plan(&plan, false, IndexReport::default(),),
            InterruptedJobDisposition::RestartWorker
        );
        assert!(matches!(
            manager.scheduler.progress.lock().unwrap().clone(),
            IndexProgress::Running(_)
        ));
        let mut state = manager.scheduler.state.lock().unwrap();
        assert!(matches!(state.phase, SchedulerPhase::Starting));
        let replacement = state.take_next_job().expect("queued Full is reselected");
        assert!(matches!(
            replacement.running.kind,
            ReconcileJobKind::Full(FullIntent {
                reason: FullReason::Manual,
                ..
            })
        ));
        assert!(
            !state.dirty.is_empty(),
            "the interrupted Delta remains queued behind the required Full"
        );
        drop(state);

        let mut shutdown_state =
            coordinator_state_with_watch(favorite_id, &root, WatchHealth::Ready);
        shutdown_state.next_event_seq = 1;
        shutdown_state
            .dirty
            .insert(DirtyScope::DirectoryContents(root.join("shutdown")), 1);
        let shutdown_plan = shutdown_state.take_next_job().expect("Delta starts");
        shutdown_state.phase = SchedulerPhase::AwaitingArray(shutdown_plan.running.clone());
        shutdown_state.shutdown = true;
        assert_eq!(
            shutdown_state.settle_interrupted_job(&shutdown_plan, false),
            InterruptedJobDisposition::Stopped
        );
        assert!(matches!(shutdown_state.phase, SchedulerPhase::Idle));
        assert!(!shutdown_state.has_runnable_work());
    }

    #[test]
    fn incremental_reconcile_activity_does_not_restart_but_password_change_does() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("library");
        std::fs::create_dir(&root).unwrap();
        let mut favorite = crate::settings::FavoriteEntry::new("library".to_owned(), root);
        favorite.auto_index_similar = true;
        let manager = SimilarIndexManager::new(temp.path().join("similar"));
        let passwords = crate::pdf_passwords::PdfPasswordStore::empty_for_test();
        let first_gate = Arc::new(crate::activity_gate::ActivityGate::new(0));
        manager.configure(
            &[favorite.clone()],
            passwords.clone(),
            Some(first_gate),
            Vec::new(),
        );
        let first_epoch = manager.scheduler.state.lock().unwrap().config_epoch;

        let replacement_gate = Arc::new(crate::activity_gate::ActivityGate::new(0));
        manager.configure(
            &[favorite.clone()],
            passwords,
            Some(Arc::clone(&replacement_gate)),
            Vec::new(),
        );
        {
            let state = manager.scheduler.state.lock().unwrap();
            assert_eq!(state.config_epoch, first_epoch);
            assert!(Arc::ptr_eq(
                state
                    .desired_config
                    .as_ref()
                    .and_then(|config| config.activity_gate.as_ref())
                    .unwrap(),
                &replacement_gate
            ));
        }

        let mut changed_passwords = crate::pdf_passwords::PdfPasswordStore::empty_for_test();
        changed_passwords.bump_credential_revision_for_test(Path::new("c:/book.pdf"));
        manager.configure(&[favorite], changed_passwords, None, Vec::new());
        assert_eq!(
            manager.scheduler.state.lock().unwrap().config_epoch,
            first_epoch + 1
        );
    }

    #[test]
    fn incremental_reconcile_degraded_progress_names_the_failed_boundary() {
        assert!(
            IndexDegradedReason::FilesystemObservationIncomplete
                .user_message()
                .contains("既存の索引は保持")
        );
        assert!(
            IndexDegradedReason::ArrayPublication("履歴不足".to_owned())
                .user_message()
                .contains("検索用一覧")
        );
        assert!(
            IndexDegradedReason::WatchUnavailable
                .user_message()
                .contains("監視")
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
