//! お気に入り配下の「別バージョン」索引ジョブと遅延ロード線形検索。

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{
    Arc, Mutex, OnceLock, RwLock, Weak,
    atomic::{AtomicBool, AtomicU64, Ordering},
};
use std::thread::JoinHandle;

use image::GenericImageView;

use crate::dupe::{self, Algo, Sig};
use crate::similar_db::{
    CompletedIndexStats, ContainerKind, Freshness, ItemKind, SearchRow, SimilarDb, StoredItem,
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StartError {
    AlreadyRunning,
}

pub struct SimilarIndexManager {
    data_dir: PathBuf,
    progress: Arc<Mutex<IndexProgress>>,
    cancel: Arc<Mutex<Option<Arc<AtomicBool>>>>,
    worker: Mutex<Option<JoinHandle<()>>>,
    memory: Arc<Mutex<MemoryState>>,
    summary: Arc<Mutex<SummaryState>>,
    memory_epoch: Arc<AtomicU64>,
    book_query: Arc<Mutex<BookQueryState>>,
    prefill_db: Arc<Mutex<Option<Arc<SimilarDb>>>>,
}

impl SimilarIndexManager {
    /// DB を開かない軽量 constructor。起動時 I/O を増やさない。
    pub fn new(data_dir: PathBuf) -> Self {
        Self {
            data_dir,
            progress: Arc::new(Mutex::new(IndexProgress::Idle)),
            cancel: Arc::new(Mutex::new(None)),
            worker: Mutex::new(None),
            memory: Arc::new(Mutex::new(MemoryState::Unloaded)),
            summary: Arc::new(Mutex::new(SummaryState::Unloaded)),
            memory_epoch: Arc::new(AtomicU64::new(0)),
            book_query: Arc::new(Mutex::new(BookQueryState::Idle)),
            prefill_db: Arc::new(Mutex::new(None)),
        }
    }

    /// 明示操作からだけ開始する。お気に入りフラグには連動しない。
    pub fn start(
        &self,
        favorite_roots: Vec<PathBuf>,
        pdf_passwords: crate::pdf_passwords::PdfPasswordStore,
        activity_gate: Option<Arc<crate::activity_gate::ActivityGate>>,
    ) -> Result<(), StartError> {
        let mut worker_slot = self.worker.lock().unwrap_or_else(|e| e.into_inner());
        if worker_slot
            .as_ref()
            .is_some_and(|worker| !worker.is_finished())
        {
            return Err(StartError::AlreadyRunning);
        }
        if let Some(worker) = worker_slot.take() {
            let _ = worker.join();
        }

        let cancel = Arc::new(AtomicBool::new(false));
        *self.cancel.lock().unwrap_or_else(|e| e.into_inner()) = Some(Arc::clone(&cancel));
        *self.progress.lock().unwrap_or_else(|e| e.into_inner()) =
            IndexProgress::Running(RunningProgress {
                stage: IndexStage::Opening,
                current_path: None,
                report: IndexReport::default(),
            });

        let db_path = SimilarDb::db_path_at(&self.data_dir);
        let progress = Arc::clone(&self.progress);
        let memory = Arc::clone(&self.memory);
        let summary = Arc::clone(&self.summary);
        let memory_epoch = Arc::clone(&self.memory_epoch);
        let book_query = Arc::clone(&self.book_query);
        let prefill_db = Arc::clone(&self.prefill_db);
        *worker_slot = Some(std::thread::spawn(move || {
            let db = match SimilarDb::open_at(&db_path) {
                Ok(db) => Arc::new(db),
                Err(error) => {
                    *progress.lock().unwrap_or_else(|e| e.into_inner()) =
                        IndexProgress::Failed(format!("similar.db open failed: {error}"));
                    return;
                }
            };
            *prefill_db.lock().unwrap_or_else(|e| e.into_inner()) = Some(Arc::clone(&db));
            register_prefill_db(&db);
            let outcome = run_index_job(
                &db,
                &favorite_roots,
                &pdf_passwords,
                activity_gate.as_deref(),
                &cancel,
                &progress,
            );
            let next = match outcome {
                Ok(report) if cancel.load(Ordering::Relaxed) => IndexProgress::Cancelled(report),
                Ok(report) => IndexProgress::Complete(report),
                Err(error) => IndexProgress::Failed(error),
            };
            memory_epoch.fetch_add(1, Ordering::AcqRel);
            *memory.lock().unwrap_or_else(|e| e.into_inner()) = MemoryState::Unloaded;
            *summary.lock().unwrap_or_else(|e| e.into_inner()) = SummaryState::Unloaded;
            *book_query.lock().unwrap_or_else(|e| e.into_inner()) = BookQueryState::Idle;
            *progress.lock().unwrap_or_else(|e| e.into_inner()) = next;
        }));
        Ok(())
    }

    pub fn cancel(&self) {
        if let Some(cancel) = self
            .cancel
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
        {
            cancel.store(true, Ordering::Relaxed);
        }
    }

    pub fn progress(&self) -> IndexProgress {
        self.progress
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    pub fn query_item(&self, item_key: &str) -> ItemQuery {
        if matches!(self.progress(), IndexProgress::Running(_)) {
            return ItemQuery::Preparing;
        }
        let mut state = self.memory.lock().unwrap_or_else(|e| e.into_inner());
        match &*state {
            MemoryState::Unloaded => {
                self.start_memory_load(&mut state);
                ItemQuery::Preparing
            }
            MemoryState::Missing => ItemQuery::NoIndex,
            MemoryState::Loading => ItemQuery::Preparing,
            MemoryState::Failed(error) => ItemQuery::Failed(error.clone()),
            MemoryState::Ready(index) => query_item_ready(index, item_key),
        }
    }

    /// 設定画面用の軽量集計。署名本体は読まず、DB I/O は専用 worker で行う。
    pub fn summary(&self) -> IndexSummaryStatus {
        if matches!(self.progress(), IndexProgress::Running(_)) {
            return IndexSummaryStatus::Preparing;
        }
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
        if matches!(self.progress(), IndexProgress::Running(_)) {
            return BookQuery::Preparing;
        }
        let mut memory = self.memory.lock().unwrap_or_else(|e| e.into_inner());
        let index = match &*memory {
            MemoryState::Unloaded => {
                self.start_memory_load(&mut memory);
                return BookQuery::Preparing;
            }
            MemoryState::Missing => return BookQuery::NotIndexed,
            MemoryState::Loading => return BookQuery::Preparing,
            MemoryState::Failed(error) => return BookQuery::Failed(error.clone()),
            MemoryState::Ready(index) => Arc::clone(index),
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
        let query_key = item_key.to_owned();
        let epoch = self.memory_epoch.load(Ordering::Acquire);
        let epoch_guard = Arc::clone(&self.memory_epoch);
        std::thread::spawn(move || {
            let result = query_book_ready(&index, &query_key);
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

    fn start_memory_load(&self, state: &mut MemoryState) {
        *state = MemoryState::Loading;
        let db_path = SimilarDb::db_path_at(&self.data_dir);
        let state = Arc::clone(&self.memory);
        let epoch = self.memory_epoch.load(Ordering::Acquire);
        let epoch_guard = Arc::clone(&self.memory_epoch);
        std::thread::spawn(move || {
            let loaded = if !db_path.is_file() {
                Ok(None)
            } else {
                SimilarDb::open_at(&db_path).and_then(|db| {
                    db.load_search_rows(current_hash_version())
                        .map(MemoryIndex::from_rows)
                        .map(Arc::new)
                        .map(Some)
                })
            }
            .map_err(|error| format!("similar index load failed: {error}"));
            if epoch_guard.load(Ordering::Acquire) != epoch {
                return;
            }
            *state.lock().unwrap_or_else(|e| e.into_inner()) = match loaded {
                Ok(Some(index)) => MemoryState::Ready(index),
                Ok(None) => MemoryState::Missing,
                Err(error) => MemoryState::Failed(error),
            };
        });
    }
}

impl Drop for SimilarIndexManager {
    fn drop(&mut self) {
        self.cancel();
    }
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

struct MemoryIndex {
    /// ブリーフどおり、索引構造を持たない 36-byte/件の線形表。
    signatures: Vec<([u8; 32], u32)>,
    rows: HashMap<u32, StoredItem>,
    row_for_key: HashMap<String, u32>,
    book_ids: BTreeMap<String, u32>,
}

impl MemoryIndex {
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

fn query_item_ready(index: &MemoryIndex, item_key: &str) -> ItemQuery {
    let Some(origin_id) = index.row_for_key.get(item_key).copied() else {
        return ItemQuery::NotIndexed;
    };
    let origin = &index.rows[&origin_id];
    if origin.quality == 0 {
        return ItemQuery::Featureless;
    }
    let mut hits = Vec::new();
    for (signature, row_id) in &index.signatures {
        if *row_id == origin_id {
            continue;
        }
        let distance = hamming256(&origin.pdq256, signature);
        let Some(band) = match_band(distance) else {
            continue;
        };
        let item = &index.rows[row_id];
        hits.push(QueryHit {
            row_id: *row_id,
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

pub fn target_for_hit(hit: &QueryHit) -> Option<SimilarItemTarget> {
    match hit.kind {
        ItemKind::Image => Some(SimilarItemTarget::File(PathBuf::from(&hit.item_key))),
        ItemKind::ZipPage => {
            let (zip_path, entry_name) =
                hit.item_key.split_once(crate::search_norm::ZIP_ENTRY_SEP)?;
            Some(SimilarItemTarget::ZipPage {
                zip_path: PathBuf::from(zip_path),
                entry_name: entry_name.to_owned(),
            })
        }
        ItemKind::PdfPage => {
            let (pdf_path, page) = hit.item_key.split_once(crate::search_norm::ZIP_ENTRY_SEP)?;
            let page_num = page.strip_prefix("pdf:")?.parse().ok()?;
            Some(SimilarItemTarget::PdfPage {
                pdf_path: PathBuf::from(pdf_path),
                page_num,
            })
        }
    }
}

fn query_book_ready(index: &MemoryIndex, item_key: &str) -> BookQuery {
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

fn book_pages(index: &MemoryIndex) -> HashMap<u32, Vec<dupe::book::BookPage>> {
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
    index: &MemoryIndex,
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
    let mut context = ScanContext {
        db,
        pdf_passwords,
        activity_gate,
        cancel,
        progress,
        report: IndexReport::default(),
        seen_items: HashSet::new(),
        seen_containers: HashSet::new(),
        visited_dirs: HashSet::new(),
        prune_safe: true,
    };
    for root in roots {
        if context.cancelled() {
            break;
        }
        if let Err(error) = context.scan_directory(root) {
            context.io_error(root, error);
        }
    }
    if context.cancelled() {
        db.cleanup_incomplete()
            .map_err(|error| format!("cancel cleanup failed: {error}"))?;
        return Ok(context.report);
    }
    set_stage(progress, IndexStage::Pruning, None);
    let normalized_roots = roots
        .iter()
        .map(|root| crate::search_index_db::normalize_path(root))
        .collect::<Vec<_>>();
    if context.prune_safe {
        context.report.removed =
            db.prune_under_roots(
                &normalized_roots,
                &context.seen_items,
                &context.seen_containers,
            )
            .map_err(|error| format!("stale row prune failed: {error}"))? as u64;
    }
    publish_report(progress, &context.report);
    let completed_at_unix_secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| i64::try_from(duration.as_secs()).unwrap_or(i64::MAX))
        .unwrap_or(0);
    db.record_completed_index(
        current_hash_version(),
        completed_at_unix_secs,
        CompletedIndexStats {
            password_required_pdfs: context.report.password_required_pdfs,
            corrupt_containers: context.report.corrupt_containers,
            zero_page_containers: context.report.zero_page_containers,
            decode_failures: context.report.decode_failures,
            io_failures: context.report.io_failures,
        },
    )
    .map_err(|error| format!("index summary publish failed: {error}"))?;
    Ok(context.report)
}

struct ScanContext<'a> {
    db: &'a SimilarDb,
    pdf_passwords: &'a crate::pdf_passwords::PdfPasswordStore,
    activity_gate: Option<&'a crate::activity_gate::ActivityGate>,
    cancel: &'a Arc<AtomicBool>,
    progress: &'a Arc<Mutex<IndexProgress>>,
    report: IndexReport,
    seen_items: HashSet<String>,
    seen_containers: HashSet<String>,
    visited_dirs: HashSet<String>,
    /// 走査漏れと削除を区別できない I/O failure が 1 件でもあれば prune しない。
    prune_safe: bool,
}

#[derive(Clone)]
struct FileCandidate {
    path: PathBuf,
    mtime: i64,
    file_size: i64,
}

impl ScanContext<'_> {
    fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }

    fn scan_directory(&mut self, directory: &Path) -> Result<(), String> {
        if self.cancelled() {
            return Ok(());
        }
        if crate::activity_gate::wait_and_check_cancel(self.activity_gate, self.cancel.as_ref()) {
            return Ok(());
        }
        let directory_key = crate::search_index_db::normalize_path(directory);
        if !self.visited_dirs.insert(directory_key.clone()) {
            return Ok(());
        }
        set_stage(
            self.progress,
            IndexStage::Scanning,
            Some(directory.to_path_buf()),
        );
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
                return Ok(());
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
        if is_book {
            self.process_image_book(directory, &directory_key, &images)?;
        } else {
            for (page_index, image) in images.iter().enumerate() {
                self.process_loose_image(image, u32::try_from(page_index).ok())?;
            }
        }
        for zip in &zips {
            self.process_zip(zip)?;
        }
        for pdf in &pdfs {
            self.process_pdf(pdf)?;
        }
        for child in subdirectories {
            if let Err(error) = self.scan_directory(&child) {
                self.io_error(&child, error);
            }
        }
        publish_report(self.progress, &self.report);
        Ok(())
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
        publish_report(self.progress, &self.report);
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
            publish_report(self.progress, &self.report);
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
            publish_report(self.progress, &self.report);
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
            publish_report(self.progress, &self.report);
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
        publish_report(self.progress, &self.report);
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

static PREFILL_DB: OnceLock<RwLock<Option<Weak<SimilarDb>>>> = OnceLock::new();

fn register_prefill_db(db: &Arc<SimilarDb>) {
    *PREFILL_DB
        .get_or_init(|| RwLock::new(None))
        .write()
        .unwrap_or_else(|e| e.into_inner()) = Some(Arc::downgrade(db));
}

#[cfg(test)]
pub(crate) fn register_prefill_db_for_test(db: &Arc<SimilarDb>) {
    register_prefill_db(db);
}

fn prefill_db() -> Option<Arc<SimilarDb>> {
    PREFILL_DB
        .get()?
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .as_ref()?
        .upgrade()
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
    let Some(db) = prefill_db() else {
        return;
    };
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

    #[test]
    fn featureless_origin_is_typed_not_an_empty_result() {
        let index = MemoryIndex::from_rows(vec![row(1, "blank", [0; 32], 0)]);
        assert_eq!(query_item_ready(&index, "blank"), ItemQuery::Featureless);
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
    fn linear_query_uses_the_two_measured_bands() {
        let mut nearly = [0u8; 32];
        nearly[0] = 0xff;
        let mut version = [0u8; 32];
        for byte in version.iter_mut().take(2) {
            *byte = 0xff;
        }
        let unrelated = [0xff; 32];
        let index = MemoryIndex::from_rows(vec![
            row(1, "origin", [0; 32], 1),
            row(2, "near", nearly, 1),
            row(3, "version", version, 1),
            row(4, "far", unrelated, 1),
        ]);
        let ItemQuery::Ready(hits) = query_item_ready(&index, "origin") else {
            panic!("expected ready query");
        };
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].band, MatchBand::NearlyIdentical);
        assert_eq!(hits[0].distance, 8);
        assert_eq!(hits[1].band, MatchBand::OtherVersion);
        assert_eq!(hits[1].distance, 16);
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
        let passwords = crate::pdf_passwords::PdfPasswordStore::empty_for_test();
        let context = ScanContext {
            db: &db,
            pdf_passwords: &passwords,
            activity_gate: None,
            cancel: &cancel,
            progress: &progress,
            report: IndexReport::default(),
            seen_items: HashSet::new(),
            seen_containers: HashSet::new(),
            visited_dirs: HashSet::new(),
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
        let index = MemoryIndex::from_rows(rows);
        let BookQuery::Ready(relations) = query_book_ready(&index, "book-a/0") else {
            panic!("expected a book result");
        };
        assert_eq!(relations.len(), 1);
        assert_eq!(relations[0].pair.relation, dupe::book::Relation::Same);
        assert_eq!(relations[0].pair.matched, BOOK_MIN_MATCHED_PAGES);
    }
}
