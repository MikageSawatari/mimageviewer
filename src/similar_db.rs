//! 「別バージョンの発見」用の独立 SQLite ストア。
//!
//! `item` / `container` は検索に公開済みの世代だけを持つ。再索引中のページは
//! staging 表へ書き、最後の transaction でだけ公開世代と入れ替える。

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::fmt;
use std::ops::Deref;
use std::path::{Path, PathBuf};
#[cfg(test)]
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rusqlite::{Connection, OpenFlags, OptionalExtension, Transaction, params};

const SCHEMA_VERSION: i64 = 5;
pub const HASH_ALGORITHM_VERSION: u32 = 1;

/// `page_index` がどの並べ方で振られているか。
///
/// 1 = 閲覧側と同じファイル名順。0 は書庫のエントリ順で振られた古い索引で、本単位の
/// 対応付けが崩れる。上げたときは [`SimilarDb::renumber_container_pages`] で振り直す。
/// 署名は変わらないので再デコードは要らない。
pub const PAGE_ORDER_VERSION: i64 = 1;

pub const fn current_hash_version() -> i64 {
    hash_version(HASH_ALGORITHM_VERSION, crate::dupe::PROXY_VERSION)
}

pub const fn hash_version(algorithm_version: u32, proxy_version: u32) -> i64 {
    ((algorithm_version as i64) << 32) | proxy_version as i64
}

#[repr(i64)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ItemKind {
    Image = 0,
    ZipPage = 1,
    PdfPage = 2,
}

impl ItemKind {
    fn from_i64(value: i64) -> rusqlite::Result<Self> {
        match value {
            0 => Ok(Self::Image),
            1 => Ok(Self::ZipPage),
            2 => Ok(Self::PdfPage),
            _ => Err(rusqlite::Error::IntegralValueOutOfRange(1, value)),
        }
    }
}

#[repr(i64)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ContainerKind {
    ImageFolder = 0,
    Zip = 1,
    Pdf = 2,
}

impl ContainerKind {
    fn from_i64(value: i64) -> rusqlite::Result<Self> {
        match value {
            0 => Ok(Self::ImageFolder),
            1 => Ok(Self::Zip),
            2 => Ok(Self::Pdf),
            _ => Err(rusqlite::Error::IntegralValueOutOfRange(1, value)),
        }
    }
}

#[repr(i64)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScanState {
    Building = 0,
    Complete = 1,
    Failed = 2,
}

impl ScanState {
    fn from_i64(value: i64) -> rusqlite::Result<Self> {
        match value {
            0 => Ok(Self::Building),
            1 => Ok(Self::Complete),
            2 => Ok(Self::Failed),
            _ => Err(rusqlite::Error::IntegralValueOutOfRange(1, value)),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoredItem {
    pub item_key: String,
    pub kind: ItemKind,
    pub container_key: Option<String>,
    pub page_index: Option<u32>,
    pub mtime: i64,
    pub file_size: i64,
    pub hash_version: i64,
    pub pdq256: [u8; 32],
    pub quality: u8,
    pub width: u32,
    pub height: u32,
    pub format: i64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoredContainer {
    pub container_key: String,
    pub kind: ContainerKind,
    pub page_count: Option<u32>,
    pub scan_state: ScanState,
    pub generation: u64,
    pub mtime: i64,
    pub file_size: i64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SearchRow {
    pub item_id: u64,
    pub revision: u32,
    pub item: StoredItem,
}

#[derive(Debug)]
pub struct BaseSearchRows {
    pub store_id: [u8; 16],
    pub applied_seq: u64,
    pub records: Vec<BaseSearchRow>,
}

/// Immutable-array publication target captured after a reconcile's final write.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct StoreWatermark {
    pub(crate) store_id: [u8; 16],
    pub(crate) through_change_seq: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ConditionalCommit<T> {
    Committed(T),
    Skipped,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BaseSearchRow {
    pub item_id: u64,
    pub signature: [u8; 32],
    pub quality: u8,
    pub revision: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ItemChangeOp {
    Add,
    Update,
    Delete,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ItemChange {
    pub seq: u64,
    pub item_id: u64,
    pub op: ItemChangeOp,
    pub revision: Option<u32>,
    pub signature: Option<[u8; 32]>,
    pub quality: Option<u8>,
}

#[derive(Debug, PartialEq, Eq)]
pub struct ItemChangeBatch {
    pub latest_seq: u64,
    pub first_available_seq: Option<u64>,
    pub changes: Vec<ItemChange>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CandidateIdentity {
    pub item_id: u64,
    pub revision: u32,
    pub signature: [u8; 32],
}

#[derive(Debug)]
pub struct VerifiedCandidates {
    pub origin: SearchRow,
    pub candidates: Vec<SearchRow>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CompletedIndexStats {
    pub password_required_pdfs: u64,
    pub corrupt_containers: u64,
    pub zero_page_containers: u64,
    pub decode_failures: u64,
    pub io_failures: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StoredIndexSummary {
    pub completed_at_unix_secs: i64,
    pub registered_items: u64,
    pub stats: CompletedIndexStats,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Freshness {
    Missing,
    Stale,
    Current,
}

pub struct SimilarDb {
    conn: Mutex<Connection>,
    #[cfg(test)]
    full_inventory_loads: AtomicUsize,
    #[cfg(test)]
    delta_inventory_loads: AtomicUsize,
    #[cfg(test)]
    cleanup_incomplete_calls: AtomicUsize,
}

const FULL_INVENTORY_CANCEL_POLL_ROWS: usize = 4096;
const FULL_INVENTORY_MAX_CAPACITY_HINT: usize = 16 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct FullItemObservation {
    pub(crate) exact_current: bool,
    pub(crate) reusable_current_row: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct FullContainerObservation {
    pub(crate) freshness: Freshness,
    pub(crate) member_count: u32,
}

struct FullInventoryItem {
    item_id: u64,
    owner: Option<u32>,
    page_index: Option<i64>,
    mtime: i64,
    file_size: i64,
    current_and_published: bool,
}

struct FullInventoryContainerSnapshot {
    kind: i64,
    page_count: Option<i64>,
    scan_state: i64,
    generation: i64,
    mtime: i64,
    file_size: i64,
}

struct FullInventoryContainer {
    snapshot: Option<FullInventoryContainerSnapshot>,
    member_count: u32,
    all_members_current: bool,
}

/// A finite SQLite snapshot used only by one full reconcile.
///
/// Exact keys have one String owner in the maps. Scan workers only borrow this value and mark the
/// word-packed bitsets; after every scoped worker joins, the owner is moved into the finalizer.
pub(crate) struct FullReconcileInventory {
    hash_version: i64,
    items_by_key: HashMap<String, u32>,
    items: Vec<FullInventoryItem>,
    containers_by_key: HashMap<String, u32>,
    containers: Vec<FullInventoryContainer>,
    seen_item_words: Box<[AtomicU64]>,
    seen_container_words: Box<[AtomicU64]>,
    #[cfg(test)]
    item_map_growths: usize,
    #[cfg(test)]
    item_record_growths: usize,
    #[cfg(test)]
    container_map_growths: usize,
    #[cfg(test)]
    container_record_growths: usize,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct DeltaScopePlan {
    pub(crate) directory_contents: Vec<String>,
    pub(crate) subtrees: Vec<String>,
    pub(crate) removed_prefixes: Vec<String>,
}

impl DeltaScopePlan {
    fn normalize(mut self) -> Self {
        for scopes in [
            &mut self.directory_contents,
            &mut self.subtrees,
            &mut self.removed_prefixes,
        ] {
            scopes.sort();
            scopes.dedup();
        }
        self
    }
}

struct DeltaInventoryItem {
    row: SearchRow,
    owner: Option<u32>,
    current_and_published: bool,
}

struct DeltaInventoryContainer {
    snapshot: Option<StoredContainer>,
    member_count: u32,
    all_members_current: bool,
}

/// Exact rows covered by one Delta observation. The read transaction ends before workers start;
/// workers borrow the inventory and only mutate its word-packed seen flags.
pub(crate) struct DeltaScopedInventory {
    hash_version: i64,
    items_by_key: HashMap<String, u32>,
    items: Vec<DeltaInventoryItem>,
    containers_by_key: HashMap<String, u32>,
    containers: Vec<DeltaInventoryContainer>,
    seen_item_words: Box<[AtomicU64]>,
    seen_container_words: Box<[AtomicU64]>,
}

impl DeltaScopedInventory {
    pub(crate) fn observe_item(
        &self,
        item_key: &str,
        owner: Option<&str>,
        page_index: Option<u32>,
        mtime: i64,
        file_size: i64,
    ) -> FullItemObservation {
        let Some(&index) = self.items_by_key.get(item_key) else {
            return FullItemObservation {
                exact_current: false,
                reusable_current_row: false,
            };
        };
        mark_inventory_bit(&self.seen_item_words, index);
        let item = &self.items[index as usize];
        let owner_matches = match (item.owner, owner) {
            (None, None) => true,
            (Some(stored), Some(observed)) => self
                .containers_by_key
                .get(observed)
                .is_some_and(|&current| current == stored),
            _ => false,
        };
        let metadata_matches = item.current_and_published
            && item.row.item.mtime == mtime
            && item.row.item.file_size == file_size;
        FullItemObservation {
            exact_current: metadata_matches
                && owner_matches
                && item.row.item.page_index == page_index,
            reusable_current_row: metadata_matches,
        }
    }

    pub(crate) fn reusable_item(
        &self,
        item_key: &str,
        mtime: i64,
        file_size: i64,
    ) -> Option<StoredItem> {
        let &index = self.items_by_key.get(item_key)?;
        let item = &self.items[index as usize];
        (item.current_and_published
            && item.row.item.mtime == mtime
            && item.row.item.file_size == file_size)
            .then(|| item.row.item.clone())
    }

    pub(crate) fn mark_item(&self, item_key: &str) {
        if let Some(&index) = self.items_by_key.get(item_key) {
            mark_inventory_bit(&self.seen_item_words, index);
        }
    }

    pub(crate) fn observe_container(&self, container_key: &str) {
        if let Some(&index) = self.containers_by_key.get(container_key) {
            mark_inventory_bit(&self.seen_container_words, index);
        }
    }

    pub(crate) fn container_member_count(&self, container_key: &str) -> u32 {
        self.containers_by_key
            .get(container_key)
            .map_or(0, |&index| self.containers[index as usize].member_count)
    }

    pub(crate) fn container_observation(
        &self,
        container_key: &str,
        mtime: i64,
        file_size: i64,
        page_count: u32,
        hash_version: i64,
    ) -> FullContainerObservation {
        let Some(&index) = self.containers_by_key.get(container_key) else {
            return FullContainerObservation {
                freshness: Freshness::Missing,
                member_count: 0,
            };
        };
        let container = &self.containers[index as usize];
        let freshness = match container.snapshot.as_ref() {
            Some(snapshot)
                if snapshot.mtime == mtime
                    && snapshot.file_size == file_size
                    && snapshot.page_count == Some(page_count)
                    && snapshot.scan_state == ScanState::Complete
                    && container.all_members_current
                    && hash_version == self.hash_version =>
            {
                Freshness::Current
            }
            Some(_) => Freshness::Stale,
            None => Freshness::Missing,
        };
        FullContainerObservation {
            freshness,
            member_count: container.member_count,
        }
    }

    fn item_seen(&self, index: u32) -> bool {
        inventory_bit_is_set(&self.seen_item_words, index)
    }

    fn container_seen(&self, index: u32) -> bool {
        inventory_bit_is_set(&self.seen_container_words, index)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DeltaPublishResult {
    Committed {
        removed: usize,
        watermark: StoreWatermark,
    },
    RequiresFull,
    Skipped,
}

impl FullReconcileInventory {
    pub(crate) fn observe_item(
        &self,
        item_key: &str,
        owner: Option<&str>,
        page_index: Option<u32>,
        mtime: i64,
        file_size: i64,
    ) -> FullItemObservation {
        let Some(&index) = self.items_by_key.get(item_key) else {
            return FullItemObservation {
                exact_current: false,
                reusable_current_row: false,
            };
        };
        mark_inventory_bit(&self.seen_item_words, index);
        let item = &self.items[index as usize];
        let owner_matches = match (item.owner, owner) {
            (None, None) => true,
            (Some(stored), Some(observed)) => self
                .containers_by_key
                .get(observed)
                .is_some_and(|&current| current == stored),
            _ => false,
        };
        let metadata_matches =
            item.current_and_published && item.mtime == mtime && item.file_size == file_size;
        FullItemObservation {
            exact_current: metadata_matches
                && owner_matches
                && item.page_index == page_index.map(i64::from),
            reusable_current_row: metadata_matches,
        }
    }

    pub(crate) fn mark_item(&self, item_key: &str) {
        if let Some(&index) = self.items_by_key.get(item_key) {
            mark_inventory_bit(&self.seen_item_words, index);
        }
    }

    pub(crate) fn observe_container(&self, container_key: &str) {
        if let Some(&index) = self.containers_by_key.get(container_key) {
            mark_inventory_bit(&self.seen_container_words, index);
        }
    }

    pub(crate) fn container_member_count(&self, container_key: &str) -> u32 {
        self.containers_by_key
            .get(container_key)
            .map_or(0, |&index| self.containers[index as usize].member_count)
    }

    pub(crate) fn container_observation(
        &self,
        container_key: &str,
        mtime: i64,
        file_size: i64,
        page_count: u32,
        hash_version: i64,
    ) -> FullContainerObservation {
        let Some(&index) = self.containers_by_key.get(container_key) else {
            return FullContainerObservation {
                freshness: Freshness::Missing,
                member_count: 0,
            };
        };
        let container = &self.containers[index as usize];
        let freshness = match container.snapshot.as_ref() {
            Some(snapshot)
                if snapshot.mtime == mtime
                    && snapshot.file_size == file_size
                    && snapshot.page_count == Some(i64::from(page_count))
                    && snapshot.scan_state == ScanState::Complete as i64
                    && container.all_members_current
                    && hash_version == self.hash_version =>
            {
                Freshness::Current
            }
            Some(_) => Freshness::Stale,
            None => Freshness::Missing,
        };
        FullContainerObservation {
            freshness,
            member_count: container.member_count,
        }
    }

    fn item_seen(&self, index: u32) -> bool {
        inventory_bit_is_set(&self.seen_item_words, index)
    }

    fn container_seen(&self, index: u32) -> bool {
        inventory_bit_is_set(&self.seen_container_words, index)
    }

    #[cfg(test)]
    pub(crate) fn accounting(&self) -> FullInventoryAccounting {
        FullInventoryAccounting {
            item_count: self.items.len(),
            item_capacity: self.items.capacity(),
            item_map_capacity: self.items_by_key.capacity(),
            container_count: self.containers.len(),
            container_capacity: self.containers.capacity(),
            container_map_capacity: self.containers_by_key.capacity(),
            key_bytes: self.items_by_key.keys().map(String::len).sum::<usize>()
                + self
                    .containers_by_key
                    .keys()
                    .map(String::len)
                    .sum::<usize>(),
            item_record_capacity_bytes: self.items.capacity() * size_of::<FullInventoryItem>(),
            container_record_capacity_bytes: self.containers.capacity()
                * size_of::<FullInventoryContainer>(),
            item_bitset_bytes: self.seen_item_words.len() * size_of::<AtomicU64>(),
            container_bitset_bytes: self.seen_container_words.len() * size_of::<AtomicU64>(),
            item_map_growths: self.item_map_growths,
            item_record_growths: self.item_record_growths,
            container_map_growths: self.container_map_growths,
            container_record_growths: self.container_record_growths,
        }
    }
}

fn inventory_words(count: usize) -> rusqlite::Result<Box<[AtomicU64]>> {
    let words = count.checked_add(63).ok_or_else(|| {
        rusqlite::Error::ToSqlConversionFailure(
            std::io::Error::other("inventory bitset overflow").into(),
        )
    })? / 64;
    let mut values = Vec::new();
    values.try_reserve_exact(words).map_err(|error| {
        rusqlite::Error::ToSqlConversionFailure(
            std::io::Error::other(format!("inventory bitset allocation failed: {error}")).into(),
        )
    })?;
    values.resize_with(words, || AtomicU64::new(0));
    Ok(values.into_boxed_slice())
}

fn mark_inventory_bit(words: &[AtomicU64], index: u32) {
    let index = index as usize;
    words[index / 64].fetch_or(1u64 << (index % 64), Ordering::Relaxed);
}

fn inventory_bit_is_set(words: &[AtomicU64], index: u32) -> bool {
    let index = index as usize;
    words[index / 64].load(Ordering::Relaxed) & (1u64 << (index % 64)) != 0
}

fn checked_inventory_index(len: usize) -> rusqlite::Result<u32> {
    u32::try_from(len).map_err(|_| {
        rusqlite::Error::ToSqlConversionFailure(
            std::io::Error::other("full reconcile inventory exceeds u32 indices").into(),
        )
    })
}

#[cfg(test)]
#[derive(Clone, Copy, Debug)]
pub(crate) struct FullInventoryAccounting {
    pub(crate) item_count: usize,
    pub(crate) item_capacity: usize,
    pub(crate) item_map_capacity: usize,
    pub(crate) container_count: usize,
    pub(crate) container_capacity: usize,
    pub(crate) container_map_capacity: usize,
    pub(crate) key_bytes: usize,
    pub(crate) item_record_capacity_bytes: usize,
    pub(crate) container_record_capacity_bytes: usize,
    pub(crate) item_bitset_bytes: usize,
    pub(crate) container_bitset_bytes: usize,
    pub(crate) item_map_growths: usize,
    pub(crate) item_record_growths: usize,
    pub(crate) container_map_growths: usize,
    pub(crate) container_record_growths: usize,
}

/// Metadata captured by the first read in a book-query transaction.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct BookReadMetadata {
    pub(crate) store_id: [u8; 16],
    pub(crate) read_seq: u64,
    pub(crate) page_order_version: i64,
}

/// A dedicated, worker-local connection for one-book queries.
///
/// This connection never creates or migrates the store. [`Self::open_at`] returns `Ok(None)` only
/// when the path itself does not exist; permission, corruption, schema, and read failures remain
/// errors for the caller to publish as `Failed`.
pub(crate) struct SimilarBookReader {
    conn: Connection,
    #[cfg(test)]
    phase_probe: Option<Arc<crate::similar_book_query_test_probe::BookQueryTestPhaseProbe>>,
}

/// Failure from the dedicated book reader.
#[derive(Debug)]
pub(crate) enum BookReadError {
    Cancelled,
    Io(std::io::Error),
    Database(rusqlite::Error),
    UnsupportedSchema { found: i64, expected: i64 },
}

impl fmt::Display for BookReadError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Cancelled => formatter.write_str("book query was cancelled"),
            Self::Io(error) => write!(formatter, "book query store access failed: {error}"),
            Self::Database(error) => write!(formatter, "book query database read failed: {error}"),
            Self::UnsupportedSchema { found, expected } => write!(
                formatter,
                "book query store schema {found} is not the supported schema {expected}"
            ),
        }
    }
}

impl std::error::Error for BookReadError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::Database(error) => Some(error),
            Self::Cancelled | Self::UnsupportedSchema { .. } => None,
        }
    }
}

/// Borrowed access to the one SQLite read transaction used by a book query.
pub(crate) struct BookReadSnapshot<'transaction> {
    conn: &'transaction Connection,
    cancel: &'transaction AtomicBool,
    metadata: BookReadMetadata,
}

/// Why a raw item-id hit can or cannot participate in a book query.
///
/// The row is retained for every present item so the query engine can distinguish a corrupt
/// same-transaction MIH reference from a row that became ineligible by normal index lifecycle.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum BookHitEligibility {
    Eligible,
    HashMismatch,
    ContainerNotComplete,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum BookHitResolution {
    Missing {
        item_id: u64,
    },
    Present {
        row: SearchRow,
        eligibility: BookHitEligibility,
    },
}

/// Request-local projection of stale ZIP ordinals into the current viewer page order.
///
/// The cache lives only for one [`BookReadSnapshot`]. It never writes the repaired order back to
/// SQLite, and both full-book rows and item-id targets use the same map.
pub(crate) struct BookPageOrderResolver<'snapshot, 'transaction, Compare> {
    snapshot: &'snapshot BookReadSnapshot<'transaction>,
    compare: Compare,
    effective_zip_orders: BTreeMap<String, CachedBookPageOrder>,
    effective_zip_order_fifo: VecDeque<String>,
    effective_zip_order_weight: usize,
    effective_zip_order_weight_limit: usize,
    effective_zip_order_entry_limit: usize,
}

#[derive(Clone)]
struct CachedBookPageOrder {
    order: Option<Arc<BTreeMap<u64, u32>>>,
    weight: usize,
}

// This holds every stale order needed by an eight-book rare neighborhood plus its ninth-book
// common cutoff inside the 10,000-page verification envelope. A single larger ZIP remains exact,
// but lives only for the current call and can therefore be sorted again on a later lookup.
const BOOK_PAGE_ORDER_CACHE_ORDINAL_LIMIT: usize = 1 << 17;
const BOOK_PAGE_ORDER_CACHE_ENTRY_LIMIT: usize = 64;

#[derive(Clone, Debug, PartialEq, Eq)]
struct ContainerPageKey {
    item_id: u64,
    item_key: String,
}

struct CancellableReadTransaction<'conn> {
    transaction: Option<Transaction<'conn>>,
}

/// WAL への変換だけを直列化する。
///
/// `PRAGMA journal_mode=WAL` は排他ロックへ昇格するため、同じ fresh DB を複数接続が同時に
/// 開くと `busy_timeout` では待たず `SQLITE_BUSY` を返す。mode は DB に永続するので、まず
/// 読み取りだけで確認し、変換が必要な接続だけを process 内で直列化する。mutex を待つ前の
/// statement は `journal_mode_is_wal` の return で解放済みであり、mutex 内でも再確認する。
fn ensure_wal_journal(conn: &Connection) -> rusqlite::Result<()> {
    if journal_mode_is_wal(conn)? {
        return Ok(());
    }

    static CONVERT: Mutex<()> = Mutex::new(());
    let _serialized = CONVERT.lock().unwrap_or_else(|error| error.into_inner());
    if journal_mode_is_wal(conn)? {
        return Ok(());
    }

    match conn.execute_batch("PRAGMA journal_mode=WAL;") {
        Ok(()) => Ok(()),
        Err(error) if journal_mode_is_wal(conn).unwrap_or(false) => {
            // 別 process が先に変換を終えた場合も目的は達成済み。
            let _ = error;
            Ok(())
        }
        Err(error) => Err(error),
    }
}

fn journal_mode_is_wal(conn: &Connection) -> rusqlite::Result<bool> {
    let mode: String = conn.query_row("PRAGMA journal_mode", [], |row| row.get(0))?;
    Ok(mode.eq_ignore_ascii_case("wal"))
}

const BOOK_READER_BUSY_TIMEOUT: Duration = Duration::from_secs(5);
const BOOK_READER_PROGRESS_OPS: i32 = 1_000;

impl SimilarBookReader {
    /// Opens an existing store without creating directories, changing journal settings, or
    /// migrating schema. `None` means that the path did not exist at the metadata check.
    pub(crate) fn open_at(path: &Path) -> Result<Option<Self>, BookReadError> {
        match std::fs::symlink_metadata(path) {
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(BookReadError::Io(error)),
        }

        let conn = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .map_err(BookReadError::Database)?;
        // SQLite's busy wait does not run the progress handler. Keep the connection default
        // explicit instead of inheriting the writer's 180-second migration timeout.
        conn.busy_timeout(BOOK_READER_BUSY_TIMEOUT)
            .map_err(BookReadError::Database)?;
        Ok(Some(Self {
            conn,
            #[cfg(test)]
            phase_probe: None,
        }))
    }

    #[cfg(test)]
    pub(crate) fn set_test_phase_probe(
        &mut self,
        probe: Arc<crate::similar_book_query_test_probe::BookQueryTestPhaseProbe>,
    ) {
        self.phase_probe = Some(probe);
    }

    /// Runs `read` inside one deferred SQLite read transaction.
    ///
    /// `BEGIN DEFERRED` alone does not select a database snapshot. The metadata query below is the
    /// first read and fixes the snapshot before any base, delta, page, or target lookup. The
    /// progress handler belongs only to this request and is removed before commit/rollback so a
    /// cancelled token cannot leak into the next request on the worker-local connection.
    pub(crate) fn with_snapshot<R, F>(
        &mut self,
        cancel: Arc<AtomicBool>,
        read: F,
    ) -> Result<R, BookReadError>
    where
        F: for<'snapshot> FnOnce(&BookReadSnapshot<'snapshot>) -> rusqlite::Result<R>,
    {
        let transaction = self.conn.transaction().map_err(BookReadError::Database)?;
        let progress_cancel = Arc::clone(&cancel);
        #[cfg(test)]
        let phase_probe = self.phase_probe.clone();
        transaction.progress_handler(
            BOOK_READER_PROGRESS_OPS,
            Some(move || {
                #[cfg(test)]
                if let Some(probe) = phase_probe.as_ref() {
                    probe.checkpoint(
                        crate::similar_book_query_test_probe::BookQueryTestPhase::SqlProgress,
                    );
                }
                progress_cancel.load(Ordering::Acquire)
            }),
        );
        let transaction = CancellableReadTransaction {
            transaction: Some(transaction),
        };

        let metadata = match book_read_metadata(&transaction, Some(cancel.as_ref())) {
            Ok(metadata) => metadata,
            Err(error) => return Err(classify_book_read_error(error, cancel.as_ref())),
        };
        let schema_version = match stored_schema_version(&transaction) {
            Ok(version) => version,
            Err(error) => return Err(classify_book_read_error(error, cancel.as_ref())),
        };
        if schema_version != SCHEMA_VERSION {
            return Err(BookReadError::UnsupportedSchema {
                found: schema_version,
                expected: SCHEMA_VERSION,
            });
        }

        let result = {
            let snapshot = BookReadSnapshot {
                conn: &transaction,
                cancel: cancel.as_ref(),
                metadata,
            };
            read(&snapshot)
        };
        match result {
            Ok(value) => {
                transaction.commit().map_err(BookReadError::Database)?;
                Ok(value)
            }
            Err(error) => Err(classify_book_read_error(error, cancel.as_ref())),
        }
    }
}

impl BookReadSnapshot<'_> {
    pub(crate) fn metadata(&self) -> BookReadMetadata {
        self.metadata
    }

    pub(crate) fn check_cancelled(&self) -> rusqlite::Result<()> {
        check_book_read_cancelled(Some(self.cancel))
    }

    pub(crate) fn load_base_search_rows(
        &self,
        hash_version: i64,
    ) -> rusqlite::Result<BaseSearchRows> {
        Ok(BaseSearchRows {
            store_id: self.metadata.store_id,
            applied_seq: self.metadata.read_seq,
            records: load_base_search_records(self.conn, hash_version, Some(self.cancel))?,
        })
    }

    pub(crate) fn load_item_changes_after(
        &self,
        after_seq: u64,
    ) -> rusqlite::Result<ItemChangeBatch> {
        load_item_changes_after_connection(
            self.conn,
            after_seq,
            self.metadata.read_seq,
            Some(self.cancel),
        )
    }

    pub(crate) fn load_item(
        &self,
        item_key: &str,
        hash_version: i64,
    ) -> rusqlite::Result<Option<SearchRow>> {
        self.check_cancelled()?;
        let row = load_search_item_by_key(self.conn, item_key, hash_version)?;
        self.check_cancelled()?;
        Ok(row)
    }

    pub(crate) fn load_book_pages(
        &self,
        container_key: &str,
        hash_version: i64,
    ) -> rusqlite::Result<Vec<SearchRow>> {
        load_book_pages_connection(self.conn, container_key, hash_version, Some(self.cancel))
    }

    pub(crate) fn resolve_pages_by_item_id(
        &self,
        item_ids: &[u64],
        hash_version: i64,
    ) -> rusqlite::Result<Vec<SearchRow>> {
        resolve_pages_by_item_id_connection(self.conn, item_ids, hash_version, Some(self.cancel))
    }

    pub(crate) fn resolve_book_hits_raw(
        &self,
        item_ids: &[u64],
        hash_version: i64,
    ) -> rusqlite::Result<Vec<BookHitResolution>> {
        resolve_book_hits_raw_connection(self.conn, item_ids, hash_version, Some(self.cancel))
    }

    pub(crate) fn page_order_resolver<Compare>(
        &self,
        compare: Compare,
    ) -> BookPageOrderResolver<'_, '_, Compare>
    where
        Compare: Fn(&str, &str) -> std::cmp::Ordering,
    {
        BookPageOrderResolver {
            snapshot: self,
            compare,
            effective_zip_orders: BTreeMap::new(),
            effective_zip_order_fifo: VecDeque::new(),
            effective_zip_order_weight: 0,
            effective_zip_order_weight_limit: BOOK_PAGE_ORDER_CACHE_ORDINAL_LIMIT,
            effective_zip_order_entry_limit: BOOK_PAGE_ORDER_CACHE_ENTRY_LIMIT,
        }
    }
}

impl<Compare> BookPageOrderResolver<'_, '_, Compare>
where
    Compare: Fn(&str, &str) -> std::cmp::Ordering,
{
    pub(crate) fn load_book_pages(
        &mut self,
        container_key: &str,
        hash_version: i64,
    ) -> rusqlite::Result<Vec<SearchRow>> {
        let pages = self.snapshot.load_book_pages(container_key, hash_version)?;
        let Some(order) = self.effective_zip_order(container_key)? else {
            return Ok(pages);
        };

        let mut ordered = BTreeMap::new();
        for mut page in pages {
            self.snapshot.check_cancelled()?;
            let page_index = order.get(&page.item_id).copied().ok_or_else(|| {
                rusqlite::Error::ToSqlConversionFailure(
                    "book page was absent from its complete ZIP order".into(),
                )
            })?;
            page.item.page_index = Some(page_index);
            if ordered.insert(page_index, page).is_some() {
                return Err(rusqlite::Error::ToSqlConversionFailure(
                    "complete ZIP order contained a duplicate ordinal".into(),
                ));
            }
        }
        self.snapshot.check_cancelled()?;
        let mut pages = Vec::with_capacity(ordered.len());
        for page in ordered.into_values() {
            self.snapshot.check_cancelled()?;
            pages.push(page);
        }
        Ok(pages)
    }

    pub(crate) fn resolve_pages_by_item_id(
        &mut self,
        item_ids: &[u64],
        hash_version: i64,
    ) -> rusqlite::Result<Vec<SearchRow>> {
        let mut rows = self
            .snapshot
            .resolve_pages_by_item_id(item_ids, hash_version)?;
        for row in &mut rows {
            self.snapshot.check_cancelled()?;
            let Some(container_key) = row.item.container_key.as_deref() else {
                continue;
            };
            if let Some(order) = self.effective_zip_order(container_key)? {
                row.item.page_index = Some(order.get(&row.item_id).copied().ok_or_else(|| {
                    rusqlite::Error::ToSqlConversionFailure(
                        "resolved page was absent from its complete ZIP order".into(),
                    )
                })?);
            }
        }
        self.snapshot.check_cancelled()?;
        Ok(rows)
    }

    pub(crate) fn resolve_book_hits_raw(
        &mut self,
        item_ids: &[u64],
        hash_version: i64,
    ) -> rusqlite::Result<Vec<BookHitResolution>> {
        let mut hits = self
            .snapshot
            .resolve_book_hits_raw(item_ids, hash_version)?;
        for hit in &mut hits {
            self.snapshot.check_cancelled()?;
            let BookHitResolution::Present {
                row,
                eligibility: BookHitEligibility::Eligible,
            } = hit
            else {
                continue;
            };
            let Some(container_key) = row.item.container_key.as_deref() else {
                continue;
            };
            if let Some(order) = self.effective_zip_order(container_key)? {
                row.item.page_index = Some(order.get(&row.item_id).copied().ok_or_else(|| {
                    rusqlite::Error::ToSqlConversionFailure(
                        "eligible book hit was absent from its complete ZIP order".into(),
                    )
                })?);
            }
        }
        self.snapshot.check_cancelled()?;
        Ok(hits)
    }

    /// Applies the same private stale-ZIP ordinal used by full-book reads to one already validated
    /// eligible hit. Callers can reject identity, lifecycle, quality, and scope first so unrelated
    /// MIH hits do not populate the bounded page-order cache.
    pub(crate) fn apply_eligible_book_hit_order(
        &mut self,
        row: &mut SearchRow,
    ) -> rusqlite::Result<()> {
        let Some(container_key) = row.item.container_key.as_deref() else {
            return Ok(());
        };
        if let Some(order) = self.effective_zip_order(container_key)? {
            row.item.page_index = Some(order.get(&row.item_id).copied().ok_or_else(|| {
                rusqlite::Error::ToSqlConversionFailure(
                    "eligible book hit was absent from its complete ZIP order".into(),
                )
            })?);
        }
        Ok(())
    }

    fn effective_zip_order(
        &mut self,
        container_key: &str,
    ) -> rusqlite::Result<Option<Arc<BTreeMap<u64, u32>>>> {
        if self.snapshot.metadata.page_order_version == PAGE_ORDER_VERSION {
            return Ok(None);
        }
        if let Some(order) = self.effective_zip_orders.get(container_key) {
            return Ok(order.order.clone());
        }

        let order = load_complete_zip_page_keys(
            self.snapshot.conn,
            container_key,
            Some(self.snapshot.cancel),
        )?
        .map(|pages| {
            canonical_page_ordinals(
                container_key,
                &pages,
                &self.compare,
                Some(self.snapshot.cancel),
            )
            .map(Arc::new)
        })
        .transpose()?;
        let weight = order.as_ref().map_or(1, |order| order.len().max(1));
        if weight <= self.effective_zip_order_weight_limit
            && self.effective_zip_order_entry_limit > 0
        {
            while self.effective_zip_orders.len() >= self.effective_zip_order_entry_limit
                || self.effective_zip_order_weight.saturating_add(weight)
                    > self.effective_zip_order_weight_limit
            {
                let evicted = self
                    .effective_zip_order_fifo
                    .pop_front()
                    .expect("non-empty page-order weight must have a FIFO entry");
                let evicted = self
                    .effective_zip_orders
                    .remove(&evicted)
                    .expect("page-order FIFO entry must have cached state");
                self.effective_zip_order_weight = self
                    .effective_zip_order_weight
                    .checked_sub(evicted.weight)
                    .expect("page-order cache weight must include each entry");
            }
            let key = container_key.to_owned();
            self.effective_zip_order_fifo.push_back(key.clone());
            self.effective_zip_order_weight += weight;
            let replaced = self.effective_zip_orders.insert(
                key,
                CachedBookPageOrder {
                    order: order.clone(),
                    weight,
                },
            );
            debug_assert!(replaced.is_none());
        }
        Ok(order)
    }
}

impl Deref for CancellableReadTransaction<'_> {
    type Target = Connection;

    fn deref(&self) -> &Self::Target {
        self.transaction
            .as_ref()
            .expect("book read transaction must exist")
    }
}

impl CancellableReadTransaction<'_> {
    fn commit(mut self) -> rusqlite::Result<()> {
        let transaction = self
            .transaction
            .take()
            .expect("book read transaction must exist");
        transaction.progress_handler(0, None::<fn() -> bool>);
        transaction.commit()
    }
}

impl Drop for CancellableReadTransaction<'_> {
    fn drop(&mut self) {
        if let Some(transaction) = self.transaction.as_ref() {
            // This runs before Transaction::drop attempts ROLLBACK. Keeping a true cancellation
            // hook installed during rollback could interrupt cleanup, whose Drop error is ignored.
            transaction.progress_handler(0, None::<fn() -> bool>);
        }
    }
}

fn classify_book_read_error(error: rusqlite::Error, cancel: &AtomicBool) -> BookReadError {
    if matches!(
        &error,
        rusqlite::Error::SqliteFailure(code, _)
            if code.code == rusqlite::ErrorCode::OperationInterrupted
    ) && cancel.load(Ordering::Acquire)
    {
        BookReadError::Cancelled
    } else {
        BookReadError::Database(error)
    }
}

fn check_book_read_cancelled(cancel: Option<&AtomicBool>) -> rusqlite::Result<()> {
    if cancel.is_some_and(|cancel| cancel.load(Ordering::Acquire)) {
        Err(rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_INTERRUPT),
            Some("book query was cancelled".to_owned()),
        ))
    } else {
        Ok(())
    }
}

impl SimilarDb {
    pub fn open() -> rusqlite::Result<Self> {
        Self::open_at(&Self::db_path())
    }

    pub fn open_at(path: &Path) -> rusqlite::Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        let mut conn = Connection::open(path)?;
        // 既定の 5 秒では v1 からの一括移行を待ち切れない。実店 4,628,611 行の移行は 38.9 秒
        // かかり、その間に別 worker がこの店を開くと `init_schema` の DDL が書き込みロックを
        // 取れずに失敗する。開くのは常に worker なので、待つことで UI は止まらない。移行後の
        // 書き込みはどれも短いため、この時間が実際に使われるのは最初の一度だけになる。
        conn.busy_timeout(std::time::Duration::from_secs(180))?;
        // timeout の位置だけでは journal mode の昇格競合は待てない。変換要否を読んでから、
        // 必要な場合だけ上の専用 owner へ渡す。
        ensure_wal_journal(&conn)?;
        conn.execute_batch("PRAGMA synchronous=NORMAL;")?;
        init_schema(&mut conn)?;
        Ok(Self {
            conn: Mutex::new(conn),
            #[cfg(test)]
            full_inventory_loads: AtomicUsize::new(0),
            #[cfg(test)]
            delta_inventory_loads: AtomicUsize::new(0),
            #[cfg(test)]
            cleanup_incomplete_calls: AtomicUsize::new(0),
        })
    }

    pub fn open_in_memory() -> rusqlite::Result<Self> {
        let mut conn = Connection::open_in_memory()?;
        init_schema(&mut conn)?;
        Ok(Self {
            conn: Mutex::new(conn),
            #[cfg(test)]
            full_inventory_loads: AtomicUsize::new(0),
            #[cfg(test)]
            delta_inventory_loads: AtomicUsize::new(0),
            #[cfg(test)]
            cleanup_incomplete_calls: AtomicUsize::new(0),
        })
    }

    pub fn db_path() -> PathBuf {
        crate::data_dir::get().join("similar.db")
    }

    pub fn db_path_at(data_dir: &Path) -> PathBuf {
        data_dir.join("similar.db")
    }

    /// 前回停止時に公開されなかった世代だけを掃除する。
    pub fn cleanup_incomplete(&self) -> rusqlite::Result<usize> {
        #[cfg(test)]
        self.cleanup_incomplete_calls
            .fetch_add(1, Ordering::Relaxed);
        let mut conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let transaction = write_transaction(&mut conn)?;
        let staged = transaction.execute("DELETE FROM item_build", [])?;
        transaction.execute("DELETE FROM container_build", [])?;
        let unpublished = load_items_for_container_state(&transaction, ScanState::Building)?;
        for row in &unpublished {
            delete_item_with_change(&transaction, row)?;
        }
        let containers = transaction.execute(
            "DELETE FROM container WHERE scan_state = ?1",
            [ScanState::Building as i64],
        )?;
        transaction.commit()?;
        Ok(staged + containers + unpublished.len())
    }

    pub fn item_freshness(
        &self,
        item_key: &str,
        mtime: i64,
        file_size: i64,
        hash_version: i64,
    ) -> rusqlite::Result<Freshness> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let found = conn
            .query_row(
                "SELECT mtime, file_size, hash_version FROM item WHERE item_key = ?1",
                [item_key],
                |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, i64>(2)?,
                    ))
                },
            )
            .optional()?;
        Ok(match found {
            None => Freshness::Missing,
            Some((stored_mtime, stored_size, stored_version))
                if stored_mtime == mtime
                    && stored_size == file_size
                    && stored_version == hash_version =>
            {
                Freshness::Current
            }
            Some(_) => Freshness::Stale,
        })
    }

    pub fn container_freshness(
        &self,
        container_key: &str,
        mtime: i64,
        file_size: i64,
        page_count: u32,
        hash_version: i64,
    ) -> rusqlite::Result<Freshness> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let found = conn
            .query_row(
                "SELECT mtime, file_size, page_count, scan_state,
                        NOT EXISTS(
                          SELECT 1 FROM item
                          WHERE container_key = ?1 AND hash_version != ?2
                        )
                   FROM container WHERE container_key = ?1",
                params![container_key, hash_version],
                |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, Option<i64>>(2)?,
                        row.get::<_, i64>(3)?,
                        row.get::<_, bool>(4)?,
                    ))
                },
            )
            .optional()?;
        Ok(match found {
            None => Freshness::Missing,
            Some((stored_mtime, stored_size, stored_pages, state, versions_current))
                if stored_mtime == mtime
                    && stored_size == file_size
                    && stored_pages == Some(i64::from(page_count))
                    && state == ScanState::Complete as i64
                    && versions_current =>
            {
                Freshness::Current
            }
            Some(_) => Freshness::Stale,
        })
    }

    /// 単独画像を差分 upsert する。戻り値は再計算した行なら true。
    pub fn upsert_loose_item(&self, item: &StoredItem) -> rusqlite::Result<bool> {
        self.upsert_loose_item_if(item, || true)
    }

    pub(crate) fn upsert_loose_item_if(
        &self,
        item: &StoredItem,
        should_publish: impl Fn() -> bool,
    ) -> rusqlite::Result<bool> {
        debug_assert!(item.container_key.is_none());
        let mut conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        if !should_publish() {
            return Ok(false);
        }
        let transaction = write_transaction(&mut conn)?;
        let existing = load_item_raw(&transaction, &item.item_key)?;
        if existing.as_ref().is_some_and(|existing| {
            existing.item.container_key.is_none()
                && existing.item.page_index == item.page_index
                && existing.item.mtime == item.mtime
                && existing.item.file_size == item.file_size
                && existing.item.hash_version == item.hash_version
        }) {
            return Ok(false);
        }
        publish_item(&transaction, existing.as_ref(), item)?;
        if !should_publish() {
            return Err(rusqlite::Error::InvalidQuery);
        }
        transaction.commit()?;
        Ok(true)
    }

    /// サムネイル元 buffer から得た署名を、公開索引とは別に再利用用として置く。
    pub fn put_prefill(&self, item: &StoredItem) -> rusqlite::Result<()> {
        self.put_prefill_if(item, || true).map(|_| ())
    }

    /// DB write ownershipを先に取得してから、呼び出し元の最新scope判定を実行する。
    ///
    /// predicateは短時間で完了し、外部lockのguardを返値より先に解放すること。これにより
    /// scope更新側はDBを待たず、scopeを外れた行は後続purgeより後に復活しない。
    pub(crate) fn put_prefill_if(
        &self,
        item: &StoredItem,
        should_insert: impl FnOnce() -> bool,
    ) -> rusqlite::Result<bool> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        if !should_insert() {
            return Ok(false);
        }
        conn.execute(
            "INSERT INTO item_prefill
             (item_key, mtime, file_size, hash_version, pdq256, quality, width, height, format)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
             ON CONFLICT(item_key) DO UPDATE SET
               mtime=excluded.mtime, file_size=excluded.file_size,
               hash_version=excluded.hash_version, pdq256=excluded.pdq256,
               quality=excluded.quality, width=excluded.width,
               height=excluded.height, format=excluded.format",
            params![
                item.item_key,
                item.mtime,
                item.file_size,
                item.hash_version,
                item.pdq256.as_slice(),
                i64::from(item.quality),
                i64::from(item.width),
                i64::from(item.height),
                item.format,
            ],
        )?;
        Ok(true)
    }

    pub fn load_prefill(
        &self,
        item_key: &str,
        mtime: i64,
        file_size: i64,
        hash_version: i64,
    ) -> rusqlite::Result<Option<StoredItem>> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        conn.query_row(
            "SELECT pdq256, quality, width, height, format
             FROM item_prefill
             WHERE item_key = ?1 AND mtime = ?2 AND file_size = ?3 AND hash_version = ?4",
            params![item_key, mtime, file_size, hash_version],
            |row| {
                let bytes = row.get::<_, Vec<u8>>(0)?;
                let pdq256: [u8; 32] = bytes.try_into().map_err(|value: Vec<u8>| {
                    rusqlite::Error::FromSqlConversionFailure(
                        value.len(),
                        rusqlite::types::Type::Blob,
                        "pdq256 must contain 32 bytes".into(),
                    )
                })?;
                let quality = row.get::<_, i64>(1)?;
                Ok(StoredItem {
                    item_key: item_key.to_owned(),
                    kind: ItemKind::Image,
                    container_key: None,
                    page_index: None,
                    mtime,
                    file_size,
                    hash_version,
                    pdq256,
                    quality: u8::try_from(quality)
                        .map_err(|_| rusqlite::Error::IntegralValueOutOfRange(1, quality))?,
                    width: i64_to_u32(row.get(2)?, 2)?,
                    height: i64_to_u32(row.get(3)?, 3)?,
                    format: row.get(4)?,
                })
            },
        )
        .optional()
    }

    /// 新しい非公開世代を開始する。既存の Complete 世代は変更しない。
    pub fn begin_container_build(
        &self,
        container_key: &str,
        kind: ContainerKind,
        page_count: u32,
        mtime: i64,
        file_size: i64,
    ) -> rusqlite::Result<u64> {
        let mut conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let transaction = write_transaction(&mut conn)?;
        let active_generation = transaction
            .query_row(
                "SELECT generation FROM container WHERE container_key = ?1",
                [container_key],
                |row| row.get::<_, i64>(0),
            )
            .optional()?
            .unwrap_or(0);
        let generation = active_generation.saturating_add(1).max(1);
        transaction.execute(
            "DELETE FROM item_build WHERE container_key = ?1",
            [container_key],
        )?;
        transaction.execute(
            "INSERT INTO container_build
             (container_key, source_parent_key, kind, page_count, scan_state, generation, mtime,
              file_size)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
             ON CONFLICT(container_key) DO UPDATE SET
               source_parent_key=excluded.source_parent_key,
               kind=excluded.kind, page_count=excluded.page_count,
               scan_state=excluded.scan_state, generation=excluded.generation,
               mtime=excluded.mtime, file_size=excluded.file_size",
            params![
                container_key,
                source_parent_key(container_key),
                kind as i64,
                i64::from(page_count),
                ScanState::Building as i64,
                generation,
                mtime,
                file_size,
            ],
        )?;
        // 初回だけは公開 table にも Building を記録する。再索引時は既存 Complete を保つ。
        transaction.execute(
            "INSERT OR IGNORE INTO container
             (container_key, source_parent_key, kind, page_count, scan_state, generation, mtime,
              file_size)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                container_key,
                source_parent_key(container_key),
                kind as i64,
                i64::from(page_count),
                ScanState::Building as i64,
                generation,
                mtime,
                file_size,
            ],
        )?;
        transaction.commit()?;
        u64::try_from(generation)
            .map_err(|_| rusqlite::Error::IntegralValueOutOfRange(4, generation))
    }

    pub fn stage_item(&self, generation: u64, item: &StoredItem) -> rusqlite::Result<()> {
        let Some(container_key) = item.container_key.as_deref() else {
            return Err(rusqlite::Error::InvalidParameterName(
                "container_key".to_owned(),
            ));
        };
        let generation_i64 = i64::try_from(generation).map_err(|_| {
            rusqlite::Error::ToSqlConversionFailure("generation exceeds SQLite INTEGER".into())
        })?;
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        conn.execute(
            "INSERT OR REPLACE INTO item_build
             (item_key, kind, container_key, page_index, mtime, file_size, hash_version,
              pdq256, quality, width, height, format, generation)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
            rusqlite::params_from_iter(item_params(item, Some(generation_i64))),
        )?;
        // 世代を取り違えた stage を黙って受理しない。
        let building = conn.query_row(
            "SELECT COUNT(*) FROM container_build
             WHERE container_key = ?1 AND generation = ?2 AND scan_state = ?3",
            params![container_key, generation, ScanState::Building as i64],
            |row| row.get::<_, i64>(0),
        )?;
        if building != 1 {
            conn.execute(
                "DELETE FROM item_build WHERE container_key = ?1 AND generation = ?2",
                params![container_key, generation],
            )?;
            return Err(rusqlite::Error::InvalidQuery);
        }
        Ok(())
    }

    /// staging 行数を確認し、公開世代を 1 transaction で置換する。
    pub fn complete_container(&self, container_key: &str, generation: u64) -> rusqlite::Result<()> {
        self.complete_container_if(container_key, generation, || true)
            .map(|_| ())
    }

    pub(crate) fn complete_container_if(
        &self,
        container_key: &str,
        generation: u64,
        should_publish: impl Fn() -> bool,
    ) -> rusqlite::Result<bool> {
        let mut conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        if !should_publish() {
            return Ok(false);
        }
        let transaction = write_transaction(&mut conn)?;
        complete_container_transaction(&transaction, container_key, generation)?;
        if !should_publish() {
            return Err(rusqlite::Error::InvalidQuery);
        }
        transaction.commit()?;
        Ok(true)
    }

    /// 失敗した新規 container は Failed として残す。再索引なら旧 Complete を保つ。
    pub fn fail_container(&self, container_key: &str, generation: u64) -> rusqlite::Result<()> {
        let mut conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let transaction = write_transaction(&mut conn)?;
        transaction.execute(
            "UPDATE container SET scan_state = ?3
             WHERE container_key = ?1 AND generation = ?2 AND scan_state = ?4",
            params![
                container_key,
                generation,
                ScanState::Failed as i64,
                ScanState::Building as i64
            ],
        )?;
        transaction.execute(
            "DELETE FROM item_build WHERE container_key = ?1 AND generation = ?2",
            params![container_key, generation],
        )?;
        transaction.execute(
            "DELETE FROM container_build WHERE container_key = ?1 AND generation = ?2",
            params![container_key, generation],
        )?;
        transaction.commit()
    }

    /// 列挙前に失敗した container を型付きで記録する。既存 Complete は保持する。
    pub fn record_container_failure(
        &self,
        container_key: &str,
        kind: ContainerKind,
        mtime: i64,
        file_size: i64,
    ) -> rusqlite::Result<()> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let current = conn
            .query_row(
                "SELECT scan_state, generation FROM container WHERE container_key = ?1",
                [container_key],
                |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
            )
            .optional()?;
        if current.is_some_and(|(state, _)| state == ScanState::Complete as i64) {
            return Ok(());
        }
        let generation = current.map_or(1, |(_, generation)| generation.saturating_add(1));
        conn.execute(
            "INSERT INTO container
             (container_key, source_parent_key, kind, page_count, scan_state, generation, mtime,
              file_size)
             VALUES (?1, ?2, ?3, NULL, ?4, ?5, ?6, ?7)
             ON CONFLICT(container_key) DO UPDATE SET
               source_parent_key=excluded.source_parent_key,
               kind=excluded.kind, page_count=NULL, scan_state=excluded.scan_state,
               generation=excluded.generation, mtime=excluded.mtime,
               file_size=excluded.file_size",
            params![
                container_key,
                source_parent_key(container_key),
                kind as i64,
                ScanState::Failed as i64,
                generation,
                mtime,
                file_size
            ],
        )?;
        Ok(())
    }

    pub fn load_search_rows(&self, hash_version: i64) -> rusqlite::Result<Vec<SearchRow>> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let mut statement = conn.prepare(
            "SELECT i.item_id, i.revision, i.item_key, i.kind, i.container_key, i.page_index,
                    i.mtime, i.file_size, i.hash_version, i.pdq256, i.quality,
                    i.width, i.height, i.format
             FROM item i
             LEFT JOIN container c ON c.container_key = i.container_key
             WHERE i.hash_version = ?1
               AND (i.container_key IS NULL OR c.scan_state = ?2)
             ORDER BY i.item_id",
        )?;
        statement
            .query_map(
                params![hash_version, ScanState::Complete as i64],
                row_to_search_row,
            )?
            .collect()
    }

    /// SQLite の一つの read snapshot から、不変 base とその適用済み change seq を作る。
    pub fn load_base_search_rows(&self, hash_version: i64) -> rusqlite::Result<BaseSearchRows> {
        let mut conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        // 読み取り専用。ここで書くなら `write_transaction` に替えること。
        let transaction = conn.transaction()?;
        let store_id = search_store_id(&transaction)?;
        let applied_seq = latest_change_seq(&transaction)?;
        let records = load_base_search_records(&transaction, hash_version, None)?;
        transaction.commit()?;
        Ok(BaseSearchRows {
            store_id,
            applied_seq,
            records,
        })
    }

    pub fn search_store_id(&self) -> rusqlite::Result<[u8; 16]> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        search_store_id(&conn)
    }

    /// Capture store identity and the latest committed change in one read snapshot.
    pub(crate) fn change_watermark(&self) -> rusqlite::Result<StoreWatermark> {
        let mut conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let transaction = conn.transaction()?;
        let watermark = StoreWatermark {
            store_id: search_store_id(&transaction)?,
            through_change_seq: latest_change_seq(&transaction)?,
        };
        transaction.commit()?;
        Ok(watermark)
    }

    pub fn load_item_changes_after(&self, after_seq: u64) -> rusqlite::Result<ItemChangeBatch> {
        let mut conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        // 読み取り専用。ここで書くなら `write_transaction` に替えること。
        let transaction = conn.transaction()?;
        let latest_seq = latest_change_seq(&transaction)?;
        let batch = load_item_changes_after_connection(&transaction, after_seq, latest_seq, None)?;
        transaction.commit()?;
        Ok(batch)
    }

    pub fn prune_item_changes_through(&self, through_seq: u64) -> rusqlite::Result<usize> {
        let through = i64::try_from(through_seq).map_err(|_| {
            rusqlite::Error::ToSqlConversionFailure("change seq exceeds SQLite INTEGER".into())
        })?;
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        conn.execute("DELETE FROM item_change WHERE seq <= ?1", [through])
    }

    /// origin と全候補を同じ SQLite read snapshot で検証する。
    pub fn verify_item_candidates(
        &self,
        item_key: &str,
        hash_version: i64,
        candidates: &[CandidateIdentity],
    ) -> rusqlite::Result<Option<VerifiedCandidates>> {
        let mut conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        // 読み取り専用。ここで書くなら `write_transaction` に替えること。
        let transaction = conn.transaction()?;
        let Some(origin) = load_search_item_by_key(&transaction, item_key, hash_version)? else {
            transaction.commit()?;
            return Ok(None);
        };
        let mut verified = Vec::with_capacity(candidates.len());
        for candidate in candidates {
            let Some(row) = load_search_item_by_id(&transaction, candidate.item_id, hash_version)?
            else {
                continue;
            };
            if row.revision == candidate.revision && row.item.pdq256 == candidate.signature {
                verified.push(row);
            }
        }
        transaction.commit()?;
        Ok(Some(VerifiedCandidates {
            origin,
            candidates: verified,
        }))
    }

    /// 完走した索引ジョブの表示用集計を、公開済み行数と同じ transaction で記録する。
    /// キャンセル・失敗したジョブからは呼ばないため、「最終更新」は完走時だけ進む。
    pub fn record_completed_index(
        &self,
        hash_version: i64,
        completed_at_unix_secs: i64,
        stats: CompletedIndexStats,
    ) -> rusqlite::Result<StoredIndexSummary> {
        let mut conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let transaction = write_transaction(&mut conn)?;
        let summary = record_completed_index_transaction(
            &transaction,
            hash_version,
            completed_at_unix_secs,
            stats,
        )?;
        transaction.commit()?;
        Ok(summary)
    }

    pub fn load_index_summary(
        &self,
        hash_version: i64,
    ) -> rusqlite::Result<Option<StoredIndexSummary>> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        conn.query_row(
            "SELECT completed_at_unix_secs, registered_items,
                    password_required_pdfs, corrupt_containers, zero_page_containers,
                    decode_failures, io_failures
             FROM index_run WHERE singleton = 1 AND hash_version = ?1",
            [hash_version],
            |row| {
                Ok(StoredIndexSummary {
                    completed_at_unix_secs: row.get(0)?,
                    registered_items: i64_to_u64(row.get(1)?, 1)?,
                    stats: CompletedIndexStats {
                        password_required_pdfs: i64_to_u64(row.get(2)?, 2)?,
                        corrupt_containers: i64_to_u64(row.get(3)?, 3)?,
                        zero_page_containers: i64_to_u64(row.get(4)?, 4)?,
                        decode_failures: i64_to_u64(row.get(5)?, 5)?,
                        io_failures: i64_to_u64(row.get(6)?, 6)?,
                    },
                })
            },
        )
        .optional()
    }

    pub fn load_item(
        &self,
        item_key: &str,
        hash_version: i64,
    ) -> rusqlite::Result<Option<SearchRow>> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        conn.query_row(
            "SELECT i.item_id, i.revision, i.item_key, i.kind, i.container_key, i.page_index,
                    i.mtime, i.file_size, i.hash_version, i.pdq256, i.quality,
                    i.width, i.height, i.format
             FROM item i
             LEFT JOIN container c ON c.container_key = i.container_key
             WHERE i.item_key = ?1 AND i.hash_version = ?2
               AND (i.container_key IS NULL OR c.scan_state = ?3)",
            params![item_key, hash_version, ScanState::Complete as i64],
            row_to_search_row,
        )
        .optional()
    }

    pub fn load_item_by_id(
        &self,
        item_id: u64,
        hash_version: i64,
    ) -> rusqlite::Result<Option<SearchRow>> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let item_id = i64::try_from(item_id).map_err(|_| {
            rusqlite::Error::ToSqlConversionFailure("item_id exceeds SQLite INTEGER".into())
        })?;
        conn.query_row(
            "SELECT i.item_id, i.revision, i.item_key, i.kind, i.container_key, i.page_index,
                    i.mtime, i.file_size, i.hash_version, i.pdq256, i.quality,
                    i.width, i.height, i.format
             FROM item i
             LEFT JOIN container c ON c.container_key = i.container_key
             WHERE i.item_id = ?1 AND i.hash_version = ?2
               AND (i.container_key IS NULL OR c.scan_state = ?3)",
            params![item_id, hash_version, ScanState::Complete as i64],
            row_to_search_row,
        )
        .optional()
    }

    pub fn load_complete_containers(&self) -> rusqlite::Result<Vec<StoredContainer>> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let mut statement = conn.prepare(
            "SELECT container_key, kind, page_count, scan_state, generation, mtime, file_size
             FROM container WHERE scan_state = ?1 ORDER BY container_key",
        )?;
        statement
            .query_map([ScanState::Complete as i64], |row| {
                Ok(StoredContainer {
                    container_key: row.get(0)?,
                    kind: ContainerKind::from_i64(row.get(1)?)?,
                    page_count: row
                        .get::<_, Option<i64>>(2)?
                        .map(u32::try_from)
                        .transpose()
                        .map_err(|_| rusqlite::Error::IntegralValueOutOfRange(2, -1))?,
                    scan_state: ScanState::Complete,
                    generation: i64_to_u64(row.get(4)?, 4)?,
                    mtime: row.get(5)?,
                    file_size: row.get(6)?,
                })
            })?
            .collect()
    }

    pub fn item_keys_for_container(&self, container_key: &str) -> rusqlite::Result<Vec<String>> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let mut statement =
            conn.prepare("SELECT item_key FROM item WHERE container_key = ?1 ORDER BY page_index")?;
        statement
            .query_map([container_key], |row| row.get(0))?
            .collect()
    }

    pub fn page_order_version(&self) -> rusqlite::Result<i64> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        conn.query_row(
            "SELECT page_order_version FROM search_content_state WHERE singleton = 1",
            [],
            |row| row.get(0),
        )
    }

    /// 保存済みのページを、呼び出し側の並べ方で振り直す。
    ///
    /// 署名も mtime も触らないので**再デコードは起きない**。並べ方の判断はこの層では
    /// 行わず、比較関数を受け取る。返り値は振り直したコンテナの数。
    pub fn renumber_container_pages(
        &self,
        page_order_version: i64,
        order: impl Fn(&str, &str) -> std::cmp::Ordering,
    ) -> rusqlite::Result<usize> {
        let mut conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let transaction = write_transaction(&mut conn)?;
        let containers = {
            let mut statement = transaction.prepare(
                "SELECT container_key FROM container WHERE kind = ?1 AND scan_state = ?2",
            )?;
            statement
                .query_map(
                    params![ContainerKind::Zip as i64, ScanState::Complete as i64],
                    |row| row.get::<_, String>(0),
                )?
                .collect::<rusqlite::Result<Vec<_>>>()?
        };
        let mut renumbered = 0usize;
        for container_key in containers {
            let pages = load_container_page_keys(&transaction, &container_key, None)?;
            let ordinals = canonical_page_ordinals(&container_key, &pages, &order, None)?;
            for (item_id, page_index) in ordinals {
                transaction.execute(
                    "UPDATE item SET page_index = ?1 WHERE item_id = ?2",
                    params![i64::from(page_index), item_id],
                )?;
            }
            renumbered += 1;
        }
        transaction.execute(
            "UPDATE search_content_state SET page_order_version = ?1 WHERE singleton = 1",
            [page_order_version],
        )?;
        transaction.commit()?;
        Ok(renumbered)
    }

    /// 公開済みコンテナ 1 冊分のページを、ページ順で読む。
    ///
    /// 本単位の照会はこれと `resolve_pages_by_item_id` の 2 つだけで identity を得る。
    /// 全行を読み込んだ在メモリ表は作らない (4.6M 行で 7.7 GB / 24 秒かかった)。
    pub fn load_book_pages(
        &self,
        container_key: &str,
        hash_version: i64,
    ) -> rusqlite::Result<Vec<SearchRow>> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        load_book_pages_connection(&conn, container_key, hash_version, None)
    }

    /// 配列が提案した item_id 群を、一つの読み取り snapshot で解決する。
    ///
    /// 単体画像の照会と同じ契約で、公開済み Complete 世代と `hash_version` の一致を SQL 側で
    /// 強制する。署名が配列と食い違う候補を捨てるのは呼び出し側の責任。
    pub fn resolve_pages_by_item_id(
        &self,
        item_ids: &[u64],
        hash_version: i64,
    ) -> rusqlite::Result<Vec<SearchRow>> {
        let mut conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let transaction = conn.transaction()?;
        let rows = resolve_pages_by_item_id_connection(&transaction, item_ids, hash_version, None)?;
        transaction.commit()?;
        Ok(rows)
    }

    /// 完走した、有効な favorites 全体の snapshot に存在しなかった公開行を削除する。
    pub fn prune_except_seen(
        &self,
        seen_items: &HashSet<String>,
        seen_containers: &HashSet<String>,
    ) -> rusqlite::Result<usize> {
        let mut conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let transaction = write_transaction(&mut conn)?;
        let removed = prune_except_seen_transaction(&transaction, seen_items, seen_containers)?;
        transaction.commit()?;
        Ok(removed)
    }

    /// Loads the exact-key metadata snapshot owned by one full reconcile.
    ///
    /// The SQLite read transaction ends before this method returns. Scan workers may then borrow
    /// the returned inventory without holding the DB mutex or pinning the WAL snapshot.
    pub(crate) fn load_full_reconcile_inventory(
        &self,
        hash_version: i64,
        should_continue: impl Fn() -> bool,
    ) -> rusqlite::Result<Option<FullReconcileInventory>> {
        #[cfg(test)]
        self.full_inventory_loads.fetch_add(1, Ordering::Relaxed);

        let mut conn = self.conn.lock().unwrap_or_else(|error| error.into_inner());
        if !should_continue() {
            return Ok(None);
        }
        let transaction = conn.transaction()?;
        let capacity_hint = transaction
            .query_row(
                "SELECT registered_items FROM index_run WHERE singleton = 1",
                [],
                |row| row.get::<_, i64>(0),
            )
            .optional()?
            .and_then(|value| usize::try_from(value).ok())
            .filter(|&value| value <= FULL_INVENTORY_MAX_CAPACITY_HINT)
            .unwrap_or(0);

        let allocation_error = |what: &'static str, error: std::collections::TryReserveError| {
            rusqlite::Error::ToSqlConversionFailure(
                std::io::Error::other(format!("{what} allocation failed: {error}")).into(),
            )
        };
        let mut items_by_key = HashMap::new();
        let mut items = Vec::new();
        #[cfg(test)]
        let mut item_map_growths = 0usize;
        #[cfg(test)]
        let mut item_record_growths = 0usize;
        items_by_key
            .try_reserve(capacity_hint)
            .map_err(|error| allocation_error("full item key inventory", error))?;
        items
            .try_reserve(capacity_hint)
            .map_err(|error| allocation_error("full item record inventory", error))?;
        let mut containers_by_key = HashMap::new();
        let mut containers = Vec::new();
        #[cfg(test)]
        let mut container_map_growths = 0usize;
        #[cfg(test)]
        let mut container_record_growths = 0usize;

        {
            let mut statement = transaction.prepare(
                "SELECT container_key, kind, page_count, scan_state, generation, mtime, file_size
                 FROM container ORDER BY container_key",
            )?;
            let mut rows = statement.query([])?;
            let mut loaded = 0usize;
            while let Some(row) = rows.next()? {
                if loaded % FULL_INVENTORY_CANCEL_POLL_ROWS == 0 && !should_continue() {
                    return Ok(None);
                }
                let index = checked_inventory_index(containers.len())?;
                let key: String = row.get(0)?;
                if containers_by_key.len() == containers_by_key.capacity() {
                    #[cfg(test)]
                    {
                        container_map_growths += 1;
                    }
                    containers_by_key
                        .try_reserve(containers_by_key.len().max(1_024))
                        .map_err(|error| allocation_error("full container key inventory", error))?;
                }
                if containers.len() == containers.capacity() {
                    #[cfg(test)]
                    {
                        container_record_growths += 1;
                    }
                    containers
                        .try_reserve(containers.len().max(1_024))
                        .map_err(|error| {
                            allocation_error("full container record inventory", error)
                        })?;
                }
                let replaced = containers_by_key.insert(key, index);
                debug_assert!(replaced.is_none());
                containers.push(FullInventoryContainer {
                    snapshot: Some(FullInventoryContainerSnapshot {
                        kind: row.get(1)?,
                        page_count: row.get(2)?,
                        scan_state: row.get(3)?,
                        generation: row.get(4)?,
                        mtime: row.get(5)?,
                        file_size: row.get(6)?,
                    }),
                    member_count: 0,
                    all_members_current: true,
                });
                loaded += 1;
            }
        }

        {
            let mut statement = transaction.prepare(
                "SELECT item_id, item_key, container_key, page_index, mtime, file_size,
                        hash_version
                 FROM item ORDER BY item_id",
            )?;
            let mut rows = statement.query([])?;
            let mut loaded = 0usize;
            while let Some(row) = rows.next()? {
                if loaded % FULL_INVENTORY_CANCEL_POLL_ROWS == 0 && !should_continue() {
                    return Ok(None);
                }
                let owner = match row.get::<_, Option<String>>(2)? {
                    Some(owner) => Some(match containers_by_key.get(&owner).copied() {
                        Some(index) => index,
                        None => {
                            let index = checked_inventory_index(containers.len())?;
                            if containers_by_key.len() == containers_by_key.capacity() {
                                #[cfg(test)]
                                {
                                    container_map_growths += 1;
                                }
                                containers_by_key
                                    .try_reserve(containers_by_key.len().max(1_024))
                                    .map_err(|error| {
                                        allocation_error("full container key inventory", error)
                                    })?;
                            }
                            if containers.len() == containers.capacity() {
                                #[cfg(test)]
                                {
                                    container_record_growths += 1;
                                }
                                containers
                                    .try_reserve(containers.len().max(1_024))
                                    .map_err(|error| {
                                        allocation_error("full container record inventory", error)
                                    })?;
                            }
                            containers_by_key.insert(owner, index);
                            containers.push(FullInventoryContainer {
                                snapshot: None,
                                member_count: 0,
                                all_members_current: true,
                            });
                            index
                        }
                    }),
                    None => None,
                };
                let stored_hash_version: i64 = row.get(6)?;
                let published = owner.is_none()
                    || owner.is_some_and(|index| {
                        containers[index as usize]
                            .snapshot
                            .as_ref()
                            .is_some_and(|container| {
                                container.scan_state == ScanState::Complete as i64
                            })
                    });
                if let Some(index) = owner {
                    let container = &mut containers[index as usize];
                    container.member_count =
                        container.member_count.checked_add(1).ok_or_else(|| {
                            rusqlite::Error::ToSqlConversionFailure(
                                std::io::Error::other("container member count exceeds u32").into(),
                            )
                        })?;
                    container.all_members_current &= stored_hash_version == hash_version;
                }
                let index = checked_inventory_index(items.len())?;
                let item_key: String = row.get(1)?;
                if items_by_key.len() == items_by_key.capacity() {
                    #[cfg(test)]
                    {
                        item_map_growths += 1;
                    }
                    items_by_key
                        .try_reserve(items_by_key.len().max(1_024))
                        .map_err(|error| {
                            allocation_error("full item key inventory growth", error)
                        })?;
                }
                if items.len() == items.capacity() {
                    #[cfg(test)]
                    {
                        item_record_growths += 1;
                    }
                    items.try_reserve(items.len().max(1_024)).map_err(|error| {
                        allocation_error("full item record inventory growth", error)
                    })?;
                }
                let replaced = items_by_key.insert(item_key, index);
                debug_assert!(replaced.is_none());
                items.push(FullInventoryItem {
                    item_id: i64_to_u64(row.get(0)?, 0)?,
                    owner,
                    page_index: row.get(3)?,
                    mtime: row.get(4)?,
                    file_size: row.get(5)?,
                    current_and_published: stored_hash_version == hash_version && published,
                });
                loaded += 1;
            }
        }
        if !should_continue() {
            return Ok(None);
        }
        transaction.commit()?;
        let seen_item_words = inventory_words(items.len())?;
        let seen_container_words = inventory_words(containers.len())?;
        Ok(Some(FullReconcileInventory {
            hash_version,
            items_by_key,
            items,
            containers_by_key,
            containers,
            seen_item_words,
            seen_container_words,
            #[cfg(test)]
            item_map_growths,
            #[cfg(test)]
            item_record_growths,
            #[cfg(test)]
            container_map_growths,
            #[cfg(test)]
            container_record_growths,
        }))
    }

    /// Load only rows whose filesystem ownership is covered by this Delta plan.
    /// The SQLite snapshot is released before scan workers start.
    pub(crate) fn load_delta_scoped_inventory(
        &self,
        scopes: DeltaScopePlan,
        hash_version: i64,
        should_continue: impl Fn() -> bool,
    ) -> rusqlite::Result<Option<DeltaScopedInventory>> {
        #[cfg(test)]
        self.delta_inventory_loads.fetch_add(1, Ordering::Relaxed);
        let scopes = scopes.normalize();
        let mut conn = self.conn.lock().unwrap_or_else(|error| error.into_inner());
        if !should_continue() {
            return Ok(None);
        }
        let transaction = conn.transaction()?;
        let mut items_by_key = HashMap::new();
        let mut items = Vec::new();
        let mut containers_by_key = HashMap::new();
        let mut containers = Vec::new();

        for directory in &scopes.directory_contents {
            if !load_delta_loose_items_by_parent(
                &transaction,
                directory,
                hash_version,
                &mut items_by_key,
                &mut items,
                &should_continue,
            )? {
                return Ok(None);
            }
        }
        for prefix in scopes.subtrees.iter().chain(scopes.removed_prefixes.iter()) {
            if !load_delta_loose_items_by_prefix(
                &transaction,
                prefix,
                hash_version,
                &mut items_by_key,
                &mut items,
                &should_continue,
            )? {
                return Ok(None);
            }
        }

        let mut candidate_containers = HashSet::new();
        for directory in &scopes.directory_contents {
            insert_delta_container_key(&mut candidate_containers, directory.clone())?;
            if !load_delta_child_file_container_keys(
                &transaction,
                directory,
                &mut candidate_containers,
                &should_continue,
            )? {
                return Ok(None);
            }
            if !load_delta_orphan_container_keys(
                &transaction,
                directory,
                true,
                &mut candidate_containers,
                &should_continue,
            )? {
                return Ok(None);
            }
        }
        for prefix in scopes.subtrees.iter().chain(scopes.removed_prefixes.iter()) {
            if !load_delta_container_keys_by_prefix(
                &transaction,
                prefix,
                &mut candidate_containers,
                &should_continue,
            )? {
                return Ok(None);
            }
            if !load_delta_orphan_container_keys(
                &transaction,
                prefix,
                false,
                &mut candidate_containers,
                &should_continue,
            )? {
                return Ok(None);
            }
        }
        let mut sorted_candidate_containers = Vec::new();
        sorted_candidate_containers
            .try_reserve_exact(candidate_containers.len())
            .map_err(|error| {
                rusqlite::Error::ToSqlConversionFailure(
                    std::io::Error::other(format!(
                        "delta sorted container allocation failed: {error}"
                    ))
                    .into(),
                )
            })?;
        sorted_candidate_containers.extend(candidate_containers);
        let mut candidate_containers = sorted_candidate_containers;
        candidate_containers.sort();
        for (loaded, container_key) in candidate_containers.into_iter().enumerate() {
            if loaded % FULL_INVENTORY_CANCEL_POLL_ROWS == 0 && !should_continue() {
                return Ok(None);
            }
            if !load_delta_container_and_members(
                &transaction,
                &container_key,
                hash_version,
                &mut containers_by_key,
                &mut containers,
                &mut items_by_key,
                &mut items,
                &should_continue,
            )? {
                return Ok(None);
            }
        }
        if !should_continue() {
            return Ok(None);
        }
        transaction.commit()?;
        let seen_item_words = inventory_words(items.len())?;
        let seen_container_words = inventory_words(containers.len())?;
        Ok(Some(DeltaScopedInventory {
            hash_version,
            items_by_key,
            items,
            containers_by_key,
            containers,
            seen_item_words,
            seen_container_words,
        }))
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn finalize_full_reconcile_inventory_if(
        &self,
        inventory: FullReconcileInventory,
        hash_version: i64,
        completed_at_unix_secs: i64,
        stats: CompletedIndexStats,
        should_publish: impl Fn() -> bool,
    ) -> rusqlite::Result<(usize, StoredIndexSummary)> {
        let mut conn = self.conn.lock().unwrap_or_else(|error| error.into_inner());
        if !should_publish() {
            return Err(rusqlite::Error::InvalidQuery);
        }
        let transaction = write_transaction(&mut conn)?;

        // Resolve owner indices through borrowed map keys. This owns only one pointer per
        // container for the finalizer lifetime and never clones the exact String corpus.
        let mut container_keys = Vec::new();
        container_keys
            .try_reserve_exact(inventory.containers.len())
            .map_err(|error| {
                rusqlite::Error::ToSqlConversionFailure(
                    std::io::Error::other(format!(
                        "full finalizer owner index allocation failed: {error}"
                    ))
                    .into(),
                )
            })?;
        container_keys.resize(inventory.containers.len(), None);
        for (key, &index) in &inventory.containers_by_key {
            container_keys[index as usize] = Some(key.as_str());
        }
        let mut owner_snapshot_unchanged = Vec::new();
        owner_snapshot_unchanged
            .try_reserve_exact(inventory.containers.len())
            .map_err(|error| {
                rusqlite::Error::ToSqlConversionFailure(
                    std::io::Error::other(format!(
                        "full finalizer identity index allocation failed: {error}"
                    ))
                    .into(),
                )
            })?;
        owner_snapshot_unchanged.resize(inventory.containers.len(), false);
        for (key, &index) in &inventory.containers_by_key {
            let container = &inventory.containers[index as usize];
            owner_snapshot_unchanged[index as usize] = match container.snapshot.as_ref() {
                Some(snapshot) => transaction.query_row(
                    "SELECT EXISTS(
                       SELECT 1 FROM container
                       WHERE container_key = ?1 AND kind = ?2 AND page_count IS ?3
                         AND scan_state = ?4 AND generation = ?5 AND mtime = ?6
                         AND file_size = ?7
                     )",
                    params![
                        key,
                        snapshot.kind,
                        snapshot.page_count,
                        snapshot.scan_state,
                        snapshot.generation,
                        snapshot.mtime,
                        snapshot.file_size,
                    ],
                    |row| row.get(0),
                )?,
                None => !transaction.query_row(
                    "SELECT EXISTS(SELECT 1 FROM container WHERE container_key = ?1)",
                    [key],
                    |row| row.get::<_, bool>(0),
                )?,
            };
        }

        let mut removed = 0usize;
        for (index, item) in inventory.items.iter().enumerate() {
            let index = u32::try_from(index).map_err(|_| {
                rusqlite::Error::ToSqlConversionFailure(
                    std::io::Error::other("full item index exceeds u32").into(),
                )
            })?;
            let protected_container = item
                .owner
                .is_some_and(|owner| inventory.container_seen(owner));
            if inventory.item_seen(index) || protected_container {
                continue;
            }
            if item
                .owner
                .is_some_and(|owner| !owner_snapshot_unchanged[owner as usize])
            {
                continue;
            }
            let owner = item.owner.and_then(|owner| container_keys[owner as usize]);
            let item_id = i64::try_from(item.item_id).map_err(|_| {
                rusqlite::Error::ToSqlConversionFailure(
                    std::io::Error::other("item_id exceeds SQLite INTEGER").into(),
                )
            })?;
            let journaled = transaction.execute(
                "INSERT INTO item_change (item_id, op, revision, pdq256, quality)
                 SELECT item_id, 2, NULL, NULL, NULL FROM item
                 WHERE item_id = ?1 AND container_key IS ?2",
                params![item_id, owner],
            )?;
            if journaled == 0 {
                continue;
            }
            let deleted = transaction.execute(
                "DELETE FROM item WHERE item_id = ?1 AND container_key IS ?2",
                params![item_id, owner],
            )?;
            debug_assert_eq!(deleted, 1);
            removed += deleted;
        }

        for (key, &index) in &inventory.containers_by_key {
            let container = &inventory.containers[index as usize];
            let Some(snapshot) = container.snapshot.as_ref() else {
                continue;
            };
            if inventory.container_seen(index) {
                continue;
            }
            if !owner_snapshot_unchanged[index as usize] {
                continue;
            }
            removed += transaction.execute(
                "DELETE FROM container
                 WHERE container_key = ?1 AND kind = ?2 AND page_count IS ?3
                   AND scan_state = ?4 AND generation = ?5 AND mtime = ?6 AND file_size = ?7
                   AND NOT EXISTS (
                     SELECT 1 FROM item WHERE item.container_key = container.container_key
                   )",
                params![
                    key,
                    snapshot.kind,
                    snapshot.page_count,
                    snapshot.scan_state,
                    snapshot.generation,
                    snapshot.mtime,
                    snapshot.file_size,
                ],
            )?;
        }
        let summary = record_completed_index_transaction(
            &transaction,
            hash_version,
            completed_at_unix_secs,
            stats,
        )?;
        if !should_publish() {
            return Err(rusqlite::Error::InvalidQuery);
        }
        transaction.commit()?;
        Ok((removed, summary))
    }

    #[cfg(test)]
    pub(crate) fn full_inventory_load_count(&self) -> usize {
        self.full_inventory_loads.load(Ordering::Relaxed)
    }

    #[cfg(test)]
    pub(crate) fn delta_inventory_load_count(&self) -> usize {
        self.delta_inventory_loads.load(Ordering::Relaxed)
    }

    #[cfg(test)]
    pub(crate) fn cleanup_incomplete_call_count(&self) -> usize {
        self.cleanup_incomplete_calls.load(Ordering::Relaxed)
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn finalize_full_reconcile_if(
        &self,
        seen_items: &HashSet<String>,
        seen_containers: &HashSet<String>,
        hash_version: i64,
        completed_at_unix_secs: i64,
        stats: CompletedIndexStats,
        should_publish: impl Fn() -> bool,
    ) -> rusqlite::Result<(usize, StoredIndexSummary)> {
        let mut conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        if !should_publish() {
            return Err(rusqlite::Error::InvalidQuery);
        }
        let transaction = write_transaction(&mut conn)?;
        let removed = prune_except_seen_transaction(&transaction, seen_items, seen_containers)?;
        let summary = record_completed_index_transaction(
            &transaction,
            hash_version,
            completed_at_unix_secs,
            stats,
        )?;
        if !should_publish() {
            return Err(rusqlite::Error::InvalidQuery);
        }
        transaction.commit()?;
        Ok((removed, summary))
    }

    /// Remove stale rows only inside successfully observed delta scopes.
    ///
    /// A directory-content scope owns its loose children, file containers, and an image-book
    /// container whose key is the directory itself.  A subtree/removal scope owns every key below
    /// it.  Seen containers protect their previous Complete pages when enumeration, password, or
    /// decoding failed; only a successful replacement generation may retire those pages.
    #[cfg(test)]
    pub fn prune_scopes_except_seen(
        &self,
        directory_contents: &[String],
        subtrees: &[String],
        removed_prefixes: &[String],
        seen_items: &HashSet<String>,
        seen_containers: &HashSet<String>,
    ) -> rusqlite::Result<usize> {
        let mut conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let transaction = write_transaction(&mut conn)?;
        let removed = prune_scopes_transaction(
            &transaction,
            directory_contents,
            subtrees,
            removed_prefixes,
            seen_items,
            seen_containers,
        )?;
        transaction.commit()?;
        Ok(removed)
    }

    /// Atomically publish every successfully prepared row for one delta batch and retire only
    /// rows inside the scopes observed by that same batch. Container pages remain in the existing
    /// build tables until this transaction, while loose rows stay in memory, so book/loose
    /// representation changes are never visible half-applied.
    #[allow(clippy::too_many_arguments)]
    #[cfg(test)]
    pub fn publish_delta_reconcile(
        &self,
        loose_items: &[StoredItem],
        completed_containers: &[(String, u64)],
        directory_contents: &[String],
        subtrees: &[String],
        removed_prefixes: &[String],
        seen_items: &HashSet<String>,
        seen_containers: &HashSet<String>,
        hash_version: i64,
        completed_at_unix_secs: i64,
    ) -> rusqlite::Result<usize> {
        self.publish_delta_reconcile_if(
            loose_items,
            completed_containers,
            directory_contents,
            subtrees,
            removed_prefixes,
            seen_items,
            seen_containers,
            hash_version,
            completed_at_unix_secs,
            || true,
        )
    }

    #[allow(clippy::too_many_arguments)]
    #[cfg(test)]
    pub(crate) fn publish_delta_reconcile_if(
        &self,
        loose_items: &[StoredItem],
        completed_containers: &[(String, u64)],
        directory_contents: &[String],
        subtrees: &[String],
        removed_prefixes: &[String],
        seen_items: &HashSet<String>,
        seen_containers: &HashSet<String>,
        hash_version: i64,
        completed_at_unix_secs: i64,
        should_publish: impl Fn() -> bool,
    ) -> rusqlite::Result<usize> {
        let mut conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        if !should_publish() {
            return Ok(0);
        }
        let transaction = write_transaction(&mut conn)?;
        for item in loose_items {
            debug_assert!(item.container_key.is_none());
            let existing = load_item_raw(&transaction, &item.item_key)?;
            publish_item(&transaction, existing.as_ref(), item)?;
        }
        for (container_key, generation) in completed_containers {
            complete_container_transaction(&transaction, container_key, *generation)?;
        }
        let removed = prune_scopes_transaction(
            &transaction,
            directory_contents,
            subtrees,
            removed_prefixes,
            seen_items,
            seen_containers,
        )?;
        refresh_index_summary_count_transaction(
            &transaction,
            hash_version,
            completed_at_unix_secs,
        )?;
        if !should_publish() {
            return Err(rusqlite::Error::InvalidQuery);
        }
        transaction.commit()?;
        Ok(removed)
    }

    /// Publish one Delta using the exact scoped snapshot loaded before filesystem observation.
    /// A missing or stale summary baseline is returned as typed repair work before any visible
    /// mutation is made; callers must schedule a Full rather than synchronously counting N rows.
    pub(crate) fn publish_delta_scoped_reconcile_if(
        &self,
        inventory: DeltaScopedInventory,
        loose_items: &[StoredItem],
        completed_containers: &[(String, u64)],
        hash_version: i64,
        completed_at_unix_secs: i64,
        should_publish: impl Fn() -> bool,
    ) -> rusqlite::Result<DeltaPublishResult> {
        let mut conn = self.conn.lock().unwrap_or_else(|error| error.into_inner());
        if !should_publish() {
            return Ok(DeltaPublishResult::Skipped);
        }
        let transaction = write_transaction(&mut conn)?;
        let Some(baseline) = load_valid_summary_baseline(&transaction, hash_version)? else {
            return Ok(DeltaPublishResult::RequiresFull);
        };

        prepare_delta_touched_table(&transaction)?;
        for item in &inventory.items {
            insert_delta_touched_key(&transaction, &item.row.item.item_key)?;
        }
        for item in loose_items {
            insert_delta_touched_key(&transaction, &item.item_key)?;
        }
        for (container_key, generation) in completed_containers {
            let mut statement = transaction.prepare(
                "SELECT item_key FROM item_build
                 WHERE container_key = ?1 AND generation = ?2",
            )?;
            let keys = statement
                .query_map(params![container_key, generation], |row| {
                    row.get::<_, String>(0)
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            for key in keys {
                insert_delta_touched_key(&transaction, &key)?;
            }
        }
        let visible_before = count_visible_delta_touched(&transaction, hash_version)?;

        for item in loose_items {
            debug_assert!(item.container_key.is_none());
            let existing = load_item_raw(&transaction, &item.item_key)?;
            publish_item(&transaction, existing.as_ref(), item)?;
        }
        for (container_key, generation) in completed_containers {
            complete_container_transaction(&transaction, container_key, *generation)?;
        }

        let mut container_keys = vec![None; inventory.containers.len()];
        for (key, &index) in &inventory.containers_by_key {
            container_keys[index as usize] = Some(key.as_str());
        }
        let mut owner_snapshot_unchanged = vec![false; inventory.containers.len()];
        for (key, &index) in &inventory.containers_by_key {
            let container = &inventory.containers[index as usize];
            owner_snapshot_unchanged[index as usize] = match container.snapshot.as_ref() {
                Some(snapshot) => container_snapshot_matches(&transaction, snapshot)?,
                None => !transaction.query_row(
                    "SELECT EXISTS(SELECT 1 FROM container WHERE container_key = ?1)",
                    [key],
                    |row| row.get::<_, bool>(0),
                )?,
            };
        }

        prepare_delta_delete_table(&transaction)?;
        for (index, item) in inventory.items.iter().enumerate() {
            let index = u32::try_from(index).map_err(|_| {
                rusqlite::Error::ToSqlConversionFailure(
                    std::io::Error::other("delta item index exceeds u32").into(),
                )
            })?;
            if inventory.item_seen(index)
                || item
                    .owner
                    .is_some_and(|owner| inventory.container_seen(owner))
                || item
                    .owner
                    .is_some_and(|owner| !owner_snapshot_unchanged[owner as usize])
            {
                continue;
            }
            let owner = item.owner.and_then(|owner| container_keys[owner as usize]);
            transaction.execute(
                "INSERT OR IGNORE INTO temp.delta_delete_candidate
                 (item_id, revision, item_key, container_key)
                 VALUES (?1, ?2, ?3, ?4)",
                params![
                    i64::try_from(item.row.item_id).unwrap_or(i64::MAX),
                    i64::from(item.row.revision),
                    item.row.item.item_key,
                    owner,
                ],
            )?;
        }
        let removed_items = transaction.execute(&delta_insert_deleted_changes_sql(false), [])?;
        transaction.execute(&delta_delete_items_sql(false), [])?;

        let mut removed_containers = 0usize;
        for (key, &index) in &inventory.containers_by_key {
            let container = &inventory.containers[index as usize];
            let Some(snapshot) = container.snapshot.as_ref() else {
                continue;
            };
            if inventory.container_seen(index) || !owner_snapshot_unchanged[index as usize] {
                continue;
            }
            removed_containers += transaction.execute(
                "DELETE FROM container
                 WHERE container_key = ?1 AND kind = ?2 AND page_count IS ?3
                   AND scan_state = ?4 AND generation = ?5 AND mtime = ?6 AND file_size = ?7
                   AND NOT EXISTS (
                     SELECT 1 FROM item WHERE item.container_key = container.container_key
                   )",
                params![
                    key,
                    snapshot.kind as i64,
                    snapshot.page_count.map(i64::from),
                    snapshot.scan_state as i64,
                    i64::try_from(snapshot.generation).unwrap_or(i64::MAX),
                    snapshot.mtime,
                    snapshot.file_size,
                ],
            )?;
        }

        let visible_after = count_visible_delta_touched(&transaction, hash_version)?;
        let registered_items = u64::try_from(
            i128::from(baseline.registered_items) + i128::from(visible_after)
                - i128::from(visible_before),
        )
        .map_err(|_| {
            rusqlite::Error::ToSqlConversionFailure(
                std::io::Error::other("delta summary count overflow").into(),
            )
        })?;
        let through_change_seq = latest_change_seq(&transaction)?;
        let updated = transaction.execute(
            "UPDATE index_run
             SET registered_items = ?1, completed_at_unix_secs = ?2,
                 through_change_seq = ?3
             WHERE singleton = 1 AND store_id = ?4 AND hash_version = ?5
               AND through_change_seq = ?6",
            params![
                i64::try_from(registered_items).unwrap_or(i64::MAX),
                completed_at_unix_secs,
                i64::try_from(through_change_seq).unwrap_or(i64::MAX),
                baseline.store_id.as_slice(),
                baseline.hash_version,
                i64::try_from(baseline.through_change_seq).unwrap_or(i64::MAX),
            ],
        )?;
        if updated != 1 {
            return Err(rusqlite::Error::InvalidQuery);
        }
        if !should_publish() {
            return Ok(DeltaPublishResult::Skipped);
        }
        transaction.commit()?;
        Ok(DeltaPublishResult::Committed {
            removed: removed_items + removed_containers,
            watermark: StoreWatermark {
                store_id: baseline.store_id,
                through_change_seq,
            },
        })
    }

    /// OFF になった favorite 配下の索引データを、次の全走査の完走を待たずに削除する。
    ///
    /// favorite は重なり得るため、`keep_roots` 配下でもあるキーは残す。公開済み世代、
    /// 構築途中の世代、prefill を同じ transaction で掃除し、公開件数の集計も追従させる。
    pub fn purge_roots_except(
        &self,
        purge_roots: &[String],
        keep_roots: &[String],
    ) -> rusqlite::Result<usize> {
        match self.purge_roots_except_if(purge_roots, keep_roots, || true)? {
            ConditionalCommit::Committed(removed) => Ok(removed),
            ConditionalCommit::Skipped => unreachable!("unconditional purge was skipped"),
        }
    }

    pub(crate) fn purge_roots_except_if(
        &self,
        purge_roots: &[String],
        keep_roots: &[String],
        should_publish: impl Fn() -> bool,
    ) -> rusqlite::Result<ConditionalCommit<usize>> {
        if purge_roots.is_empty() {
            return Ok(ConditionalCommit::Committed(0));
        }

        let mut conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        if !should_publish() {
            return Ok(ConditionalCommit::Skipped);
        }
        let transaction = write_transaction(&mut conn)?;
        let should_purge =
            |key: &str| key_is_under_any(key, purge_roots) && !key_is_under_any(key, keep_roots);

        let item_keys = query_string_column(&transaction, "SELECT item_key FROM item")?;
        let container_keys =
            query_string_column(&transaction, "SELECT container_key FROM container")?;
        let build_item_keys = query_string_column(&transaction, "SELECT item_key FROM item_build")?;
        let build_container_keys =
            query_string_column(&transaction, "SELECT container_key FROM container_build")?;
        let prefill_keys = query_string_column(&transaction, "SELECT item_key FROM item_prefill")?;

        let mut removed = 0;
        for key in item_keys.into_iter().filter(|key| should_purge(key)) {
            if let Some(row) = load_item_raw(&transaction, &key)? {
                delete_item_with_change(&transaction, &row)?;
                removed += 1;
            }
        }
        for key in container_keys.into_iter().filter(|key| should_purge(key)) {
            for row in load_items_for_container_raw(&transaction, &key)? {
                delete_item_with_change(&transaction, &row)?;
                removed += 1;
            }
            removed +=
                transaction.execute("DELETE FROM container WHERE container_key = ?1", [&key])?;
        }
        for key in build_item_keys.into_iter().filter(|key| should_purge(key)) {
            removed += transaction.execute("DELETE FROM item_build WHERE item_key = ?1", [&key])?;
        }
        for key in build_container_keys
            .into_iter()
            .filter(|key| should_purge(key))
        {
            removed +=
                transaction.execute("DELETE FROM item_build WHERE container_key = ?1", [&key])?;
            removed += transaction.execute(
                "DELETE FROM container_build WHERE container_key = ?1",
                [&key],
            )?;
        }
        for key in prefill_keys.into_iter().filter(|key| should_purge(key)) {
            removed +=
                transaction.execute("DELETE FROM item_prefill WHERE item_key = ?1", [&key])?;
        }

        transaction.execute(
            "UPDATE index_run
             SET store_id = ?2,
                 through_change_seq = ?3,
                 registered_items = (
               SELECT COUNT(*) FROM item i
               LEFT JOIN container c ON c.container_key = i.container_key
               WHERE i.hash_version = index_run.hash_version
                 AND (i.container_key IS NULL OR c.scan_state = ?1)
             )
             WHERE singleton = 1",
            params![
                ScanState::Complete as i64,
                search_store_id(&transaction)?.as_slice(),
                i64::try_from(latest_change_seq(&transaction)?).unwrap_or(i64::MAX),
            ],
        )?;
        if !should_publish() {
            return Ok(ConditionalCommit::Skipped);
        }
        transaction.commit()?;
        Ok(ConditionalCommit::Committed(removed))
    }

    /// Refresh the corpus count after a successful delta without replacing the last full-run
    /// failure summary with statistics from one dirty directory.
    #[cfg(test)]
    pub fn refresh_index_summary_count(
        &self,
        hash_version: i64,
        completed_at_unix_secs: i64,
    ) -> rusqlite::Result<()> {
        let mut conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let transaction = write_transaction(&mut conn)?;
        refresh_index_summary_count_transaction(
            &transaction,
            hash_version,
            completed_at_unix_secs,
        )?;
        transaction.commit()
    }

    #[cfg(test)]
    fn count_staged(&self) -> usize {
        self.conn
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .query_row("SELECT COUNT(*) FROM item_build", [], |row| row.get(0))
            .unwrap()
    }
}

pub(crate) fn key_is_under_any(key: &str, roots: &[String]) -> bool {
    roots.iter().any(|root| {
        key == root
            || key
                .strip_prefix(root)
                .is_some_and(|suffix| root.ends_with('/') || suffix.starts_with('/'))
    })
}

fn normalized_parent(key: &str) -> Option<&str> {
    key.rsplit_once('/').map(|(parent, _)| parent)
}

fn source_parent_key(key: &str) -> String {
    let Some(parent) = normalized_parent(key) else {
        return String::new();
    };
    if parent.len() == 2 && parent.as_bytes()[1] == b':' {
        format!("{parent}/")
    } else {
        parent.to_owned()
    }
}

#[derive(Clone, Copy)]
struct SummaryBaseline {
    store_id: [u8; 16],
    through_change_seq: u64,
    hash_version: i64,
    registered_items: u64,
}

fn load_valid_summary_baseline(
    conn: &Connection,
    hash_version: i64,
) -> rusqlite::Result<Option<SummaryBaseline>> {
    let baseline = conn
        .query_row(
            "SELECT store_id, through_change_seq, hash_version, registered_items
             FROM index_run WHERE singleton = 1",
            [],
            |row| {
                let bytes = row.get::<_, Vec<u8>>(0)?;
                let store_id: [u8; 16] = bytes.try_into().map_err(|value: Vec<u8>| {
                    rusqlite::Error::FromSqlConversionFailure(
                        value.len(),
                        rusqlite::types::Type::Blob,
                        "index summary store_id must contain 16 bytes".into(),
                    )
                })?;
                Ok(SummaryBaseline {
                    store_id,
                    through_change_seq: i64_to_u64(row.get(1)?, 1)?,
                    hash_version: row.get(2)?,
                    registered_items: i64_to_u64(row.get(3)?, 3)?,
                })
            },
        )
        .optional()?;
    let Some(baseline) = baseline else {
        return Ok(None);
    };
    Ok((baseline.store_id == search_store_id(conn)?
        && baseline.hash_version == hash_version
        && baseline.through_change_seq == latest_change_seq(conn)?)
    .then_some(baseline))
}

fn prepare_delta_touched_table(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        "CREATE TEMP TABLE IF NOT EXISTS delta_touched (
           item_key TEXT PRIMARY KEY
         ) WITHOUT ROWID;
         DELETE FROM temp.delta_touched;",
    )
}

fn insert_delta_touched_key(conn: &Connection, item_key: &str) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT OR IGNORE INTO temp.delta_touched (item_key) VALUES (?1)",
        [item_key],
    )?;
    Ok(())
}

fn count_visible_delta_touched(conn: &Connection, hash_version: i64) -> rusqlite::Result<u64> {
    let count = conn.query_row(
        &delta_count_visible_touched_sql(false),
        params![hash_version, ScanState::Complete as i64],
        |row| row.get::<_, i64>(0),
    )?;
    i64_to_u64(count, 0)
}

fn prepare_delta_delete_table(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        "CREATE TEMP TABLE IF NOT EXISTS delta_delete_candidate (
           item_id INTEGER PRIMARY KEY,
           revision INTEGER NOT NULL,
           item_key TEXT NOT NULL,
           container_key TEXT
         );
         DELETE FROM temp.delta_delete_candidate;",
    )
}

fn container_snapshot_matches(
    conn: &Connection,
    snapshot: &StoredContainer,
) -> rusqlite::Result<bool> {
    conn.query_row(
        "SELECT EXISTS(
           SELECT 1 FROM container
           WHERE container_key = ?1 AND kind = ?2 AND page_count IS ?3
             AND scan_state = ?4 AND generation = ?5 AND mtime = ?6 AND file_size = ?7
         )",
        params![
            snapshot.container_key,
            snapshot.kind as i64,
            snapshot.page_count.map(i64::from),
            snapshot.scan_state as i64,
            i64::try_from(snapshot.generation).unwrap_or(i64::MAX),
            snapshot.mtime,
            snapshot.file_size,
        ],
        |row| row.get(0),
    )
}

fn descendant_key_bounds(key: &str) -> (String, String) {
    if key.ends_with('/') {
        (key.to_owned(), format!("{}0", key.trim_end_matches('/')))
    } else {
        (format!("{key}/"), format!("{key}0"))
    }
}

fn delta_sql_prefix(explain: bool) -> &'static str {
    if explain { "EXPLAIN QUERY PLAN " } else { "" }
}

fn delta_loose_parent_sql(explain: bool) -> String {
    format!(
        "{}{SEARCH_ROW_SELECT}
         WHERE i.container_key IS NULL AND i.source_parent_key = ?1",
        delta_sql_prefix(explain)
    )
}

fn delta_loose_prefix_sql(explain: bool) -> String {
    format!(
        "{}{SEARCH_ROW_SELECT}
         INDEXED BY item_loose_key_idx
         WHERE i.container_key IS NULL AND i.item_key = ?1
         UNION ALL
         {SEARCH_ROW_SELECT}
         INDEXED BY item_loose_key_idx
         WHERE i.container_key IS NULL AND i.item_key >= ?2 AND i.item_key < ?3",
        delta_sql_prefix(explain)
    )
}

fn delta_count_visible_touched_sql(explain: bool) -> String {
    format!(
        "{}SELECT COUNT(*)
         FROM temp.delta_touched t
         CROSS JOIN item i ON i.item_key = t.item_key
         LEFT JOIN container c ON c.container_key = i.container_key
         WHERE i.hash_version = ?1
           AND (i.container_key IS NULL OR c.scan_state = ?2)",
        delta_sql_prefix(explain)
    )
}

fn delta_insert_deleted_changes_sql(explain: bool) -> String {
    format!(
        "{}INSERT INTO item_change (item_id, op, revision, pdq256, quality)
         SELECT i.item_id, 2, NULL, NULL, NULL
         FROM temp.delta_delete_candidate d
         CROSS JOIN item i ON i.item_id = d.item_id
         WHERE d.revision = i.revision AND d.item_key = i.item_key
           AND i.container_key IS d.container_key
         ORDER BY d.item_id",
        delta_sql_prefix(explain)
    )
}

fn delta_delete_items_sql(explain: bool) -> String {
    format!(
        "{}DELETE FROM item
         WHERE item_id IN (
           SELECT i.item_id FROM temp.delta_delete_candidate d
           CROSS JOIN item i ON i.item_id = d.item_id
           WHERE d.revision = i.revision AND d.item_key = i.item_key
             AND i.container_key IS d.container_key
         )",
        delta_sql_prefix(explain)
    )
}

fn delta_child_file_container_sql(explain: bool) -> String {
    format!(
        "{}SELECT container_key FROM container
         WHERE source_parent_key = ?1 AND kind IN (?2, ?3)",
        delta_sql_prefix(explain)
    )
}

fn delta_container_prefix_sql(explain: bool) -> String {
    format!(
        "{}SELECT container_key FROM container
         WHERE container_key = ?1 OR (container_key >= ?2 AND container_key < ?3)",
        delta_sql_prefix(explain)
    )
}

fn delta_orphan_container_sql(explain: bool) -> String {
    format!(
        "{}SELECT i.container_key, i.kind
         FROM item i
         LEFT JOIN container c ON c.container_key = i.container_key
         WHERE i.container_key IS NOT NULL AND c.container_key IS NULL
           AND (i.container_key = ?1 OR (i.container_key >= ?2 AND i.container_key < ?3))",
        delta_sql_prefix(explain)
    )
}

fn delta_container_members_sql(explain: bool) -> String {
    format!(
        "{}{SEARCH_ROW_SELECT} WHERE i.container_key = ?1",
        delta_sql_prefix(explain)
    )
}

fn push_delta_inventory_item(
    items_by_key: &mut HashMap<String, u32>,
    items: &mut Vec<DeltaInventoryItem>,
    row: SearchRow,
    owner: Option<u32>,
    current_and_published: bool,
) -> rusqlite::Result<()> {
    if items_by_key.contains_key(&row.item.item_key) {
        return Ok(());
    }
    items_by_key.try_reserve(1).map_err(|error| {
        rusqlite::Error::ToSqlConversionFailure(
            std::io::Error::other(format!(
                "delta item key inventory allocation failed: {error}"
            ))
            .into(),
        )
    })?;
    items.try_reserve(1).map_err(|error| {
        rusqlite::Error::ToSqlConversionFailure(
            std::io::Error::other(format!("delta item inventory allocation failed: {error}"))
                .into(),
        )
    })?;
    let index = checked_inventory_index(items.len())?;
    items_by_key.insert(row.item.item_key.clone(), index);
    items.push(DeltaInventoryItem {
        row,
        owner,
        current_and_published,
    });
    Ok(())
}

fn load_delta_loose_items_by_parent(
    conn: &Connection,
    parent: &str,
    hash_version: i64,
    items_by_key: &mut HashMap<String, u32>,
    items: &mut Vec<DeltaInventoryItem>,
    should_continue: &dyn Fn() -> bool,
) -> rusqlite::Result<bool> {
    let mut statement = conn.prepare(&delta_loose_parent_sql(false))?;
    let mut rows = statement.query([parent])?;
    let mut loaded = 0usize;
    loop {
        if loaded % FULL_INVENTORY_CANCEL_POLL_ROWS == 0 && !should_continue() {
            return Ok(false);
        }
        let Some(row) = rows.next()? else {
            break;
        };
        let row = row_to_search_row(row)?;
        let current = row.item.hash_version == hash_version;
        push_delta_inventory_item(items_by_key, items, row, None, current)?;
        loaded += 1;
    }
    Ok(true)
}

fn load_delta_loose_items_by_prefix(
    conn: &Connection,
    prefix: &str,
    hash_version: i64,
    items_by_key: &mut HashMap<String, u32>,
    items: &mut Vec<DeltaInventoryItem>,
    should_continue: &dyn Fn() -> bool,
) -> rusqlite::Result<bool> {
    let (lower, upper) = descendant_key_bounds(prefix);
    let mut statement = conn.prepare(&delta_loose_prefix_sql(false))?;
    let mut rows = statement.query(params![prefix, lower, upper])?;
    let mut loaded = 0usize;
    loop {
        if loaded % FULL_INVENTORY_CANCEL_POLL_ROWS == 0 && !should_continue() {
            return Ok(false);
        }
        let Some(row) = rows.next()? else {
            break;
        };
        let row = row_to_search_row(row)?;
        let current = row.item.hash_version == hash_version;
        push_delta_inventory_item(items_by_key, items, row, None, current)?;
        loaded += 1;
    }
    Ok(true)
}

fn load_delta_child_file_container_keys(
    conn: &Connection,
    parent: &str,
    keys: &mut HashSet<String>,
    should_continue: &dyn Fn() -> bool,
) -> rusqlite::Result<bool> {
    let mut statement = conn.prepare(&delta_child_file_container_sql(false))?;
    let mut rows = statement.query(params![
        parent,
        ContainerKind::Zip as i64,
        ContainerKind::Pdf as i64
    ])?;
    let mut loaded = 0usize;
    loop {
        if loaded % FULL_INVENTORY_CANCEL_POLL_ROWS == 0 && !should_continue() {
            return Ok(false);
        }
        let Some(row) = rows.next()? else {
            break;
        };
        insert_delta_container_key(keys, row.get(0)?)?;
        loaded += 1;
    }
    Ok(true)
}

fn load_delta_container_keys_by_prefix(
    conn: &Connection,
    prefix: &str,
    keys: &mut HashSet<String>,
    should_continue: &dyn Fn() -> bool,
) -> rusqlite::Result<bool> {
    let (lower, upper) = descendant_key_bounds(prefix);
    let mut statement = conn.prepare(&delta_container_prefix_sql(false))?;
    let mut rows = statement.query(params![prefix, lower, upper])?;
    let mut loaded = 0usize;
    loop {
        if loaded % FULL_INVENTORY_CANCEL_POLL_ROWS == 0 && !should_continue() {
            return Ok(false);
        }
        let Some(row) = rows.next()? else {
            break;
        };
        insert_delta_container_key(keys, row.get(0)?)?;
        loaded += 1;
    }
    Ok(true)
}

fn insert_delta_container_key(keys: &mut HashSet<String>, key: String) -> rusqlite::Result<()> {
    if keys.contains(&key) {
        return Ok(());
    }
    keys.try_reserve(1).map_err(|error| {
        rusqlite::Error::ToSqlConversionFailure(
            std::io::Error::other(format!("delta container key allocation failed: {error}")).into(),
        )
    })?;
    keys.insert(key);
    Ok(())
}

/// A missing owner row is invalid but remains observable from item provenance. Include only
/// recognized owner kinds; malformed kinds make no deletion claim.
fn load_delta_orphan_container_keys(
    conn: &Connection,
    scope: &str,
    direct_children_only: bool,
    keys: &mut HashSet<String>,
    should_continue: &dyn Fn() -> bool,
) -> rusqlite::Result<bool> {
    let (lower, upper) = descendant_key_bounds(scope);
    let mut statement = conn.prepare(&delta_orphan_container_sql(false))?;
    let mut rows = statement.query(params![scope, lower, upper])?;
    let mut loaded = 0usize;
    loop {
        if loaded % FULL_INVENTORY_CANCEL_POLL_ROWS == 0 && !should_continue() {
            return Ok(false);
        }
        let Some(row) = rows.next()? else {
            break;
        };
        let key = row.get::<_, String>(0)?;
        let item_kind = row.get::<_, i64>(1)?;
        let recognized = match item_kind {
            value if value == ItemKind::Image as i64 => key == scope || !direct_children_only,
            value if value == ItemKind::ZipPage as i64 || value == ItemKind::PdfPage as i64 => {
                !direct_children_only || source_parent_key(&key) == scope
            }
            _ => false,
        };
        if recognized {
            insert_delta_container_key(keys, key)?;
        }
        loaded += 1;
    }
    Ok(true)
}

#[allow(clippy::too_many_arguments)]
fn load_delta_container_and_members(
    conn: &Connection,
    container_key: &str,
    hash_version: i64,
    containers_by_key: &mut HashMap<String, u32>,
    containers: &mut Vec<DeltaInventoryContainer>,
    items_by_key: &mut HashMap<String, u32>,
    items: &mut Vec<DeltaInventoryItem>,
    should_continue: &dyn Fn() -> bool,
) -> rusqlite::Result<bool> {
    if containers_by_key.contains_key(container_key) {
        return Ok(true);
    }
    if !should_continue() {
        return Ok(false);
    }
    let snapshot = conn
        .query_row(
            "SELECT container_key, kind, page_count, scan_state, generation, mtime, file_size
             FROM container WHERE container_key = ?1",
            [container_key],
            |row| {
                Ok(StoredContainer {
                    container_key: row.get(0)?,
                    kind: ContainerKind::from_i64(row.get(1)?)?,
                    page_count: row
                        .get::<_, Option<i64>>(2)?
                        .map(u32::try_from)
                        .transpose()
                        .map_err(|_| rusqlite::Error::IntegralValueOutOfRange(2, -1))?,
                    scan_state: ScanState::from_i64(row.get(3)?)?,
                    generation: i64_to_u64(row.get(4)?, 4)?,
                    mtime: row.get(5)?,
                    file_size: row.get(6)?,
                })
            },
        )
        .optional()?;
    let published = snapshot
        .as_ref()
        .is_some_and(|container| container.scan_state == ScanState::Complete);
    containers_by_key.try_reserve(1).map_err(|error| {
        rusqlite::Error::ToSqlConversionFailure(
            std::io::Error::other(format!("delta container map allocation failed: {error}")).into(),
        )
    })?;
    containers.try_reserve(1).map_err(|error| {
        rusqlite::Error::ToSqlConversionFailure(
            std::io::Error::other(format!(
                "delta container inventory allocation failed: {error}"
            ))
            .into(),
        )
    })?;
    let container_index = checked_inventory_index(containers.len())?;
    containers_by_key.insert(container_key.to_owned(), container_index);
    containers.push(DeltaInventoryContainer {
        snapshot,
        member_count: 0,
        all_members_current: true,
    });

    let mut statement = conn.prepare(&delta_container_members_sql(false))?;
    let mut rows = statement.query([container_key])?;
    let mut loaded = 0usize;
    loop {
        if loaded % FULL_INVENTORY_CANCEL_POLL_ROWS == 0 && !should_continue() {
            return Ok(false);
        }
        let Some(row) = rows.next()? else {
            break;
        };
        let row = row_to_search_row(row)?;
        let current = row.item.hash_version == hash_version;
        let container = &mut containers[container_index as usize];
        container.member_count = container.member_count.checked_add(1).ok_or_else(|| {
            rusqlite::Error::ToSqlConversionFailure(
                std::io::Error::other("delta container member count exceeds u32").into(),
            )
        })?;
        container.all_members_current &= current;
        push_delta_inventory_item(
            items_by_key,
            items,
            row,
            Some(container_index),
            current && published,
        )?;
        loaded += 1;
    }
    Ok(true)
}

fn record_completed_index_transaction(
    transaction: &Transaction<'_>,
    hash_version: i64,
    completed_at_unix_secs: i64,
    stats: CompletedIndexStats,
) -> rusqlite::Result<StoredIndexSummary> {
    let registered_items = transaction.query_row(
        "SELECT COUNT(*) FROM item i
         LEFT JOIN container c ON c.container_key = i.container_key
         WHERE i.hash_version = ?1
           AND (i.container_key IS NULL OR c.scan_state = ?2)",
        params![hash_version, ScanState::Complete as i64],
        |row| row.get::<_, i64>(0),
    )?;
    transaction.execute(
        "INSERT INTO index_run
         (singleton, store_id, through_change_seq, hash_version,
          completed_at_unix_secs, registered_items,
          password_required_pdfs, corrupt_containers, zero_page_containers,
          decode_failures, io_failures)
         VALUES (1, ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
         ON CONFLICT(singleton) DO UPDATE SET
           store_id=excluded.store_id,
           through_change_seq=excluded.through_change_seq,
           hash_version=excluded.hash_version,
           completed_at_unix_secs=excluded.completed_at_unix_secs,
           registered_items=excluded.registered_items,
           password_required_pdfs=excluded.password_required_pdfs,
           corrupt_containers=excluded.corrupt_containers,
           zero_page_containers=excluded.zero_page_containers,
           decode_failures=excluded.decode_failures,
           io_failures=excluded.io_failures",
        params![
            search_store_id(transaction)?.as_slice(),
            i64::try_from(latest_change_seq(transaction)?).unwrap_or(i64::MAX),
            hash_version,
            completed_at_unix_secs,
            registered_items,
            i64::try_from(stats.password_required_pdfs).unwrap_or(i64::MAX),
            i64::try_from(stats.corrupt_containers).unwrap_or(i64::MAX),
            i64::try_from(stats.zero_page_containers).unwrap_or(i64::MAX),
            i64::try_from(stats.decode_failures).unwrap_or(i64::MAX),
            i64::try_from(stats.io_failures).unwrap_or(i64::MAX),
        ],
    )?;
    Ok(StoredIndexSummary {
        completed_at_unix_secs,
        registered_items: u64::try_from(registered_items).unwrap_or(0),
        stats,
    })
}

fn prune_except_seen_transaction(
    transaction: &Transaction<'_>,
    seen_items: &HashSet<String>,
    seen_containers: &HashSet<String>,
) -> rusqlite::Result<usize> {
    let item_keys = query_string_column(transaction, "SELECT item_key FROM item")?;
    let container_keys = query_string_column(transaction, "SELECT container_key FROM container")?;
    let mut removed = 0usize;
    for key in item_keys {
        let Some(row) = load_item_raw(transaction, &key)? else {
            continue;
        };
        let protected_container = row
            .item
            .container_key
            .as_ref()
            .is_some_and(|container| seen_containers.contains(container));
        if !seen_items.contains(&key) && !protected_container {
            delete_item_with_change(transaction, &row)?;
            removed += 1;
        }
    }
    for key in container_keys {
        if !seen_containers.contains(&key) {
            for row in load_items_for_container_raw(transaction, &key)? {
                delete_item_with_change(transaction, &row)?;
                removed += 1;
            }
            removed +=
                transaction.execute("DELETE FROM container WHERE container_key = ?1", [&key])?;
        }
    }
    Ok(removed)
}

fn prune_scopes_transaction(
    transaction: &Transaction<'_>,
    directory_contents: &[String],
    subtrees: &[String],
    removed_prefixes: &[String],
    seen_items: &HashSet<String>,
    seen_containers: &HashSet<String>,
) -> rusqlite::Result<usize> {
    let item_keys = query_string_column(transaction, "SELECT item_key FROM item")?;
    let container_keys = query_string_column(transaction, "SELECT container_key FROM container")?;
    let covered_container = |key: &str| {
        directory_contents
            .iter()
            .any(|directory| key == directory || normalized_parent(key) == Some(directory.as_str()))
            || key_is_under_any(key, subtrees)
            || key_is_under_any(key, removed_prefixes)
    };
    let covered_loose = |key: &str| {
        directory_contents
            .iter()
            .any(|directory| normalized_parent(key) == Some(directory.as_str()))
            || key_is_under_any(key, subtrees)
            || key_is_under_any(key, removed_prefixes)
    };

    let mut removed = 0usize;
    for key in item_keys {
        let Some(row) = load_item_raw(transaction, &key)? else {
            continue;
        };
        let covered = row.item.container_key.as_ref().map_or_else(
            || covered_loose(&row.item.item_key),
            |container| covered_container(container),
        );
        let protected_container = row
            .item
            .container_key
            .as_ref()
            .is_some_and(|container| seen_containers.contains(container));
        if covered && !seen_items.contains(&key) && !protected_container {
            delete_item_with_change(transaction, &row)?;
            removed += 1;
        }
    }
    for key in container_keys {
        if covered_container(&key) && !seen_containers.contains(&key) {
            for row in load_items_for_container_raw(transaction, &key)? {
                delete_item_with_change(transaction, &row)?;
                removed += 1;
            }
            removed +=
                transaction.execute("DELETE FROM container WHERE container_key = ?1", [&key])?;
        }
    }
    Ok(removed)
}

fn refresh_index_summary_count_transaction(
    transaction: &Transaction<'_>,
    hash_version: i64,
    completed_at_unix_secs: i64,
) -> rusqlite::Result<()> {
    transaction.execute(
        "UPDATE index_run
         SET completed_at_unix_secs = ?2,
             store_id = ?4,
             through_change_seq = ?5,
             registered_items = (
               SELECT COUNT(*) FROM item i
               LEFT JOIN container c ON c.container_key = i.container_key
               WHERE i.hash_version = ?1
                 AND (i.container_key IS NULL OR c.scan_state = ?3)
             )
         WHERE singleton = 1",
        params![
            hash_version,
            completed_at_unix_secs,
            ScanState::Complete as i64,
            search_store_id(transaction)?.as_slice(),
            i64::try_from(latest_change_seq(transaction)?).unwrap_or(i64::MAX),
        ],
    )?;
    Ok(())
}

fn query_string_column(transaction: &Transaction<'_>, sql: &str) -> rusqlite::Result<Vec<String>> {
    let mut statement = transaction.prepare(sql)?;
    statement
        .query_map([], |row| row.get::<_, String>(0))?
        .collect()
}

fn building_page_count(
    transaction: &Transaction<'_>,
    container_key: &str,
    generation: u64,
) -> rusqlite::Result<i64> {
    transaction.query_row(
        "SELECT page_count FROM container_build
         WHERE container_key = ?1 AND generation = ?2 AND scan_state = ?3",
        params![container_key, generation, ScanState::Building as i64],
        |row| row.get(0),
    )
}

fn complete_container_transaction(
    transaction: &Transaction<'_>,
    container_key: &str,
    generation: u64,
) -> rusqlite::Result<()> {
    let expected = building_page_count(transaction, container_key, generation)?;
    let actual = transaction.query_row(
        "SELECT COUNT(*) FROM item_build WHERE container_key = ?1 AND generation = ?2",
        params![container_key, generation],
        |row| row.get::<_, i64>(0),
    )?;
    if actual != expected {
        return Err(rusqlite::Error::InvalidQuery);
    }
    let staged = load_staged_items(transaction, container_key, generation)?;
    let staged_keys = staged
        .iter()
        .map(|item| item.item_key.as_str())
        .collect::<HashSet<_>>();
    for old in load_items_for_container_raw(transaction, container_key)? {
        if !staged_keys.contains(old.item.item_key.as_str()) {
            delete_item_with_change(transaction, &old)?;
        }
    }
    for item in &staged {
        let existing = load_item_raw(transaction, &item.item_key)?;
        publish_item(transaction, existing.as_ref(), item)?;
    }
    transaction.execute(
        "INSERT INTO container
         (container_key, source_parent_key, kind, page_count, scan_state, generation, mtime,
          file_size)
         SELECT container_key, source_parent_key, kind, page_count, ?3, generation, mtime,
                file_size
         FROM container_build WHERE container_key = ?1 AND generation = ?2
         ON CONFLICT(container_key) DO UPDATE SET
           source_parent_key=excluded.source_parent_key,
           kind=excluded.kind, page_count=excluded.page_count,
           scan_state=excluded.scan_state, generation=excluded.generation,
           mtime=excluded.mtime, file_size=excluded.file_size",
        params![container_key, generation, ScanState::Complete as i64],
    )?;
    transaction.execute(
        "DELETE FROM item_build WHERE container_key = ?1 AND generation = ?2",
        params![container_key, generation],
    )?;
    transaction.execute(
        "DELETE FROM container_build WHERE container_key = ?1 AND generation = ?2",
        params![container_key, generation],
    )?;
    Ok(())
}

fn item_params(item: &StoredItem, generation: Option<i64>) -> Vec<rusqlite::types::Value> {
    let mut values = vec![
        item.item_key.clone().into(),
        (item.kind as i64).into(),
        item.container_key.clone().into(),
        item.page_index.map(i64::from).into(),
        item.mtime.into(),
        item.file_size.into(),
        item.hash_version.into(),
        item.pdq256.to_vec().into(),
        i64::from(item.quality).into(),
        i64::from(item.width).into(),
        i64::from(item.height).into(),
        item.format.into(),
    ];
    if let Some(generation) = generation {
        values.push(generation.into());
    }
    values
}

fn publish_item(
    conn: &Connection,
    existing: Option<&SearchRow>,
    item: &StoredItem,
) -> rusqlite::Result<SearchRow> {
    let (item_id, revision, op) = if let Some(existing) = existing {
        let revision = existing.revision.checked_add(1).ok_or_else(|| {
            rusqlite::Error::ToSqlConversionFailure("item revision exhausted".into())
        })?;
        conn.execute(
            "UPDATE item SET revision=?2, item_key=?3, kind=?4, container_key=?5,
               source_parent_key=?6, page_index=?7, mtime=?8, file_size=?9,
               hash_version=?10, pdq256=?11, quality=?12, width=?13, height=?14,
               format=?15 WHERE item_id=?1",
            params![
                i64::try_from(existing.item_id).unwrap_or(i64::MAX),
                i64::from(revision),
                item.item_key,
                item.kind as i64,
                item.container_key,
                item.container_key
                    .is_none()
                    .then(|| source_parent_key(&item.item_key)),
                item.page_index.map(i64::from),
                item.mtime,
                item.file_size,
                item.hash_version,
                item.pdq256.as_slice(),
                i64::from(item.quality),
                i64::from(item.width),
                i64::from(item.height),
                item.format,
            ],
        )?;
        (existing.item_id, revision, ItemChangeOp::Update)
    } else {
        conn.execute(
            "INSERT INTO item
             (revision, item_key, kind, container_key, source_parent_key, page_index, mtime,
              file_size, hash_version, pdq256, quality, width, height, format)
             VALUES (1, ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
            params![
                item.item_key,
                item.kind as i64,
                item.container_key,
                item.container_key
                    .is_none()
                    .then(|| source_parent_key(&item.item_key)),
                item.page_index.map(i64::from),
                item.mtime,
                item.file_size,
                item.hash_version,
                item.pdq256.as_slice(),
                i64::from(item.quality),
                i64::from(item.width),
                i64::from(item.height),
                item.format,
            ],
        )?;
        (
            i64_to_u64(conn.last_insert_rowid(), 0)?,
            1,
            ItemChangeOp::Add,
        )
    };
    insert_item_change(conn, item_id, op, Some(revision), Some(item))?;
    Ok(SearchRow {
        item_id,
        revision,
        item: item.clone(),
    })
}

fn delete_item_with_change(conn: &Connection, row: &SearchRow) -> rusqlite::Result<()> {
    insert_item_change(conn, row.item_id, ItemChangeOp::Delete, None, None)?;
    conn.execute(
        "DELETE FROM item WHERE item_id = ?1",
        [i64::try_from(row.item_id).unwrap_or(i64::MAX)],
    )?;
    Ok(())
}

fn insert_item_change(
    conn: &Connection,
    item_id: u64,
    op: ItemChangeOp,
    revision: Option<u32>,
    item: Option<&StoredItem>,
) -> rusqlite::Result<()> {
    let op = match op {
        ItemChangeOp::Add => 0,
        ItemChangeOp::Update => 1,
        ItemChangeOp::Delete => 2,
    };
    conn.execute(
        "INSERT INTO item_change (item_id, op, revision, pdq256, quality)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![
            i64::try_from(item_id).unwrap_or(i64::MAX),
            op,
            revision.map(i64::from),
            item.map(|item| item.pdq256.as_slice()),
            item.map(|item| i64::from(item.quality)),
        ],
    )?;
    Ok(())
}

fn row_to_search_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<SearchRow> {
    let item_id = i64_to_u64(row.get(0)?, 0)?;
    let revision = i64_to_u32(row.get(1)?, 1)?;
    let pdq = row.get::<_, Vec<u8>>(9)?;
    let pdq256: [u8; 32] = pdq.try_into().map_err(|value: Vec<u8>| {
        rusqlite::Error::FromSqlConversionFailure(
            value.len(),
            rusqlite::types::Type::Blob,
            "pdq256 must contain 32 bytes".into(),
        )
    })?;
    let quality_i64 = row.get::<_, i64>(10)?;
    Ok(SearchRow {
        item_id,
        revision,
        item: StoredItem {
            item_key: row.get(2)?,
            kind: ItemKind::from_i64(row.get(3)?)?,
            container_key: row.get(4)?,
            page_index: row
                .get::<_, Option<i64>>(5)?
                .map(u32::try_from)
                .transpose()
                .map_err(|_| rusqlite::Error::IntegralValueOutOfRange(5, -1))?,
            mtime: row.get(6)?,
            file_size: row.get(7)?,
            hash_version: row.get(8)?,
            pdq256,
            quality: u8::try_from(quality_i64)
                .map_err(|_| rusqlite::Error::IntegralValueOutOfRange(10, quality_i64))?,
            width: i64_to_u32(row.get(11)?, 11)?,
            height: i64_to_u32(row.get(12)?, 12)?,
            format: row.get(13)?,
        },
    })
}

const SEARCH_ROW_SELECT: &str =
    "SELECT i.item_id, i.revision, i.item_key, i.kind, i.container_key, i.page_index,
            i.mtime, i.file_size, i.hash_version, i.pdq256, i.quality,
            i.width, i.height, i.format FROM item i";

fn load_item_raw(conn: &Connection, item_key: &str) -> rusqlite::Result<Option<SearchRow>> {
    conn.query_row(
        &format!("{SEARCH_ROW_SELECT} WHERE i.item_key = ?1"),
        [item_key],
        row_to_search_row,
    )
    .optional()
}

fn load_search_item_by_key(
    conn: &Connection,
    item_key: &str,
    hash_version: i64,
) -> rusqlite::Result<Option<SearchRow>> {
    conn.query_row(
        &format!(
            "{SEARCH_ROW_SELECT} LEFT JOIN container c ON c.container_key=i.container_key
             WHERE i.item_key=?1 AND i.hash_version=?2
               AND (i.container_key IS NULL OR c.scan_state=?3)"
        ),
        params![item_key, hash_version, ScanState::Complete as i64],
        row_to_search_row,
    )
    .optional()
}

fn load_search_item_by_id(
    conn: &Connection,
    item_id: u64,
    hash_version: i64,
) -> rusqlite::Result<Option<SearchRow>> {
    conn.query_row(
        &format!(
            "{SEARCH_ROW_SELECT} LEFT JOIN container c ON c.container_key=i.container_key
             WHERE i.item_id=?1 AND i.hash_version=?2
               AND (i.container_key IS NULL OR c.scan_state=?3)"
        ),
        params![
            i64::try_from(item_id).unwrap_or(i64::MAX),
            hash_version,
            ScanState::Complete as i64
        ],
        row_to_search_row,
    )
    .optional()
}

fn load_items_for_container_raw(
    conn: &Connection,
    container_key: &str,
) -> rusqlite::Result<Vec<SearchRow>> {
    let mut statement = conn.prepare(&format!(
        "{SEARCH_ROW_SELECT} WHERE i.container_key=?1 ORDER BY i.item_id"
    ))?;
    statement
        .query_map([container_key], row_to_search_row)?
        .collect()
}

fn load_items_for_container_state(
    conn: &Connection,
    state: ScanState,
) -> rusqlite::Result<Vec<SearchRow>> {
    let mut statement = conn.prepare(&format!(
        "{SEARCH_ROW_SELECT} JOIN container c ON c.container_key=i.container_key
         WHERE c.scan_state=?1 ORDER BY i.item_id"
    ))?;
    statement
        .query_map([state as i64], row_to_search_row)?
        .collect()
}

fn load_staged_items(
    conn: &Connection,
    container_key: &str,
    generation: u64,
) -> rusqlite::Result<Vec<StoredItem>> {
    let mut statement = conn.prepare(
        "SELECT item_key, kind, container_key, page_index, mtime, file_size, hash_version,
                pdq256, quality, width, height, format
         FROM item_build WHERE container_key=?1 AND generation=?2 ORDER BY page_index",
    )?;
    statement
        .query_map(params![container_key, generation], row_to_stored_item)?
        .collect()
}

fn row_to_stored_item(row: &rusqlite::Row<'_>) -> rusqlite::Result<StoredItem> {
    let pdq = row.get::<_, Vec<u8>>(7)?;
    let pdq256: [u8; 32] = pdq.try_into().map_err(|value: Vec<u8>| {
        rusqlite::Error::FromSqlConversionFailure(
            value.len(),
            rusqlite::types::Type::Blob,
            "pdq256 must contain 32 bytes".into(),
        )
    })?;
    let quality = row.get::<_, i64>(8)?;
    Ok(StoredItem {
        item_key: row.get(0)?,
        kind: ItemKind::from_i64(row.get(1)?)?,
        container_key: row.get(2)?,
        page_index: row
            .get::<_, Option<i64>>(3)?
            .map(u32::try_from)
            .transpose()
            .map_err(|_| rusqlite::Error::IntegralValueOutOfRange(3, -1))?,
        mtime: row.get(4)?,
        file_size: row.get(5)?,
        hash_version: row.get(6)?,
        pdq256,
        quality: u8::try_from(quality)
            .map_err(|_| rusqlite::Error::IntegralValueOutOfRange(8, quality))?,
        width: i64_to_u32(row.get(9)?, 9)?,
        height: i64_to_u32(row.get(10)?, 10)?,
        format: row.get(11)?,
    })
}

fn row_to_item_change(row: &rusqlite::Row<'_>) -> rusqlite::Result<ItemChange> {
    let op_value = row.get::<_, i64>(2)?;
    let op = match op_value {
        0 => ItemChangeOp::Add,
        1 => ItemChangeOp::Update,
        2 => ItemChangeOp::Delete,
        _ => return Err(rusqlite::Error::IntegralValueOutOfRange(2, op_value)),
    };
    let signature = row
        .get::<_, Option<Vec<u8>>>(4)?
        .map(|bytes| {
            let len = bytes.len();
            bytes.try_into().map_err(|_| {
                rusqlite::Error::FromSqlConversionFailure(
                    len,
                    rusqlite::types::Type::Blob,
                    "pdq256 must contain 32 bytes".into(),
                )
            })
        })
        .transpose()?;
    let quality_value = row.get::<_, Option<i64>>(5)?;
    Ok(ItemChange {
        seq: i64_to_u64(row.get(0)?, 0)?,
        item_id: i64_to_u64(row.get(1)?, 1)?,
        op,
        revision: row
            .get::<_, Option<i64>>(3)?
            .map(|v| i64_to_u32(v, 3))
            .transpose()?,
        signature,
        quality: quality_value
            .map(|v| u8::try_from(v).map_err(|_| rusqlite::Error::IntegralValueOutOfRange(5, v)))
            .transpose()?,
    })
}

fn book_read_metadata(
    conn: &Connection,
    cancel: Option<&AtomicBool>,
) -> rusqlite::Result<BookReadMetadata> {
    check_book_read_cancelled(cancel)?;
    // This is deliberately the first SELECT after BEGIN DEFERRED: it fixes the SQLite snapshot
    // used by every subsequent lookup in BookReadSnapshot.
    let (store_id, page_order_version, read_seq) = conn.query_row(
        "SELECT store_id, page_order_version,
                COALESCE((SELECT seq FROM sqlite_sequence WHERE name = 'item_change'), 0)
           FROM search_content_state WHERE singleton = 1",
        [],
        |row| {
            Ok((
                row.get::<_, Vec<u8>>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, i64>(2)?,
            ))
        },
    )?;
    let store_id: [u8; 16] = store_id.try_into().map_err(|value: Vec<u8>| {
        rusqlite::Error::FromSqlConversionFailure(
            value.len(),
            rusqlite::types::Type::Blob,
            "search content store_id must contain 16 bytes".into(),
        )
    })?;
    let read_seq = i64_to_u64(read_seq, 2)?;
    check_book_read_cancelled(cancel)?;
    Ok(BookReadMetadata {
        store_id,
        read_seq,
        page_order_version,
    })
}

fn load_base_search_records(
    conn: &Connection,
    hash_version: i64,
    cancel: Option<&AtomicBool>,
) -> rusqlite::Result<Vec<BaseSearchRow>> {
    check_book_read_cancelled(cancel)?;
    let count_i64 = conn.query_row(
        "SELECT COUNT(*) FROM item i
         LEFT JOIN container c ON c.container_key = i.container_key
         WHERE i.hash_version = ?1
           AND (i.container_key IS NULL OR c.scan_state = ?2)",
        params![hash_version, ScanState::Complete as i64],
        |row| row.get::<_, i64>(0),
    )?;
    let count = usize::try_from(count_i64)
        .map_err(|_| rusqlite::Error::IntegralValueOutOfRange(0, count_i64))?;
    check_book_read_cancelled(cancel)?;

    let mut records = Vec::with_capacity(count);
    let mut statement = conn.prepare(
        "SELECT i.item_id, i.pdq256, i.quality, i.revision
         FROM item i
         LEFT JOIN container c ON c.container_key = i.container_key
         WHERE i.hash_version = ?1
           AND (i.container_key IS NULL OR c.scan_state = ?2)
         ORDER BY i.item_id",
    )?;
    let mut rows = statement.query(params![hash_version, ScanState::Complete as i64])?;
    while let Some(row) = rows.next()? {
        check_book_read_cancelled(cancel)?;
        let item_id = i64_to_u64(row.get(0)?, 0)?;
        let pdq = row.get_ref(1)?.as_blob()?;
        let signature: [u8; 32] = pdq.try_into().map_err(|_| {
            rusqlite::Error::FromSqlConversionFailure(
                pdq.len(),
                rusqlite::types::Type::Blob,
                "pdq256 must contain 32 bytes".into(),
            )
        })?;
        let quality_i64 = row.get::<_, i64>(2)?;
        records.push(BaseSearchRow {
            item_id,
            signature,
            quality: u8::try_from(quality_i64)
                .map_err(|_| rusqlite::Error::IntegralValueOutOfRange(2, quality_i64))?,
            revision: i64_to_u32(row.get(3)?, 3)?,
        });
    }
    drop(rows);
    drop(statement);
    check_book_read_cancelled(cancel)?;
    Ok(records)
}

fn load_item_changes_after_connection(
    conn: &Connection,
    after_seq: u64,
    latest_seq: u64,
    cancel: Option<&AtomicBool>,
) -> rusqlite::Result<ItemChangeBatch> {
    check_book_read_cancelled(cancel)?;
    let first_available_seq = conn
        .query_row("SELECT MIN(seq) FROM item_change", [], |row| {
            row.get::<_, Option<i64>>(0)
        })?
        .map(|value| i64_to_u64(value, 0))
        .transpose()?;
    let after_i64 = i64::try_from(after_seq).map_err(|_| {
        rusqlite::Error::ToSqlConversionFailure("change seq exceeds SQLite INTEGER".into())
    })?;
    let mut statement = conn.prepare(
        "SELECT seq, item_id, op, revision, pdq256, quality
         FROM item_change WHERE seq > ?1 ORDER BY seq",
    )?;
    let mut query = statement.query([after_i64])?;
    let mut changes = Vec::new();
    while let Some(row) = query.next()? {
        check_book_read_cancelled(cancel)?;
        changes.push(row_to_item_change(row)?);
    }
    drop(query);
    drop(statement);
    check_book_read_cancelled(cancel)?;
    Ok(ItemChangeBatch {
        latest_seq,
        first_available_seq,
        changes,
    })
}

fn load_container_page_keys(
    conn: &Connection,
    container_key: &str,
    cancel: Option<&AtomicBool>,
) -> rusqlite::Result<Vec<ContainerPageKey>> {
    check_book_read_cancelled(cancel)?;
    let mut statement = conn.prepare(
        "SELECT item_id, item_key FROM item WHERE container_key = ?1
         ORDER BY page_index, item_id",
    )?;
    let mut query = statement.query([container_key])?;
    let mut pages = Vec::new();
    while let Some(row) = query.next()? {
        check_book_read_cancelled(cancel)?;
        pages.push(ContainerPageKey {
            item_id: i64_to_u64(row.get(0)?, 0)?,
            item_key: row.get(1)?,
        });
    }
    drop(query);
    drop(statement);
    check_book_read_cancelled(cancel)?;
    Ok(pages)
}

fn load_complete_zip_page_keys(
    conn: &Connection,
    container_key: &str,
    cancel: Option<&AtomicBool>,
) -> rusqlite::Result<Option<Vec<ContainerPageKey>>> {
    check_book_read_cancelled(cancel)?;
    let kind_and_state = conn
        .query_row(
            "SELECT kind, scan_state FROM container WHERE container_key = ?1",
            [container_key],
            |row| {
                Ok((
                    ContainerKind::from_i64(row.get(0)?)?,
                    ScanState::from_i64(row.get(1)?)?,
                ))
            },
        )
        .optional()?;
    check_book_read_cancelled(cancel)?;
    if kind_and_state != Some((ContainerKind::Zip, ScanState::Complete)) {
        return Ok(None);
    }
    load_container_page_keys(conn, container_key, cancel).map(Some)
}

fn canonical_page_ordinals<Compare>(
    container_key: &str,
    pages: &[ContainerPageKey],
    compare: &Compare,
    cancel: Option<&AtomicBool>,
) -> rusqlite::Result<BTreeMap<u64, u32>>
where
    Compare: Fn(&str, &str) -> std::cmp::Ordering,
{
    check_book_read_cancelled(cancel)?;
    let mut order = Vec::with_capacity(pages.len());
    for index in 0..pages.len() {
        check_book_read_cancelled(cancel)?;
        order.push(index);
    }

    // Bottom-up merge sort lets cancellation abort the Rust-side ordering work. Equal keys take
    // the left item, preserving the stored (page_index, item_id) order supplied by the SQL above.
    let prefix_len = container_key.len().saturating_add(1);
    let mut scratch = vec![0usize; order.len()];
    let mut width = 1usize;
    while width < order.len() {
        let mut start = 0usize;
        while start < order.len() {
            let middle = start.saturating_add(width).min(order.len());
            let end = middle.saturating_add(width).min(order.len());
            let (mut left, mut right, mut output) = (start, middle, start);
            while left < middle && right < end {
                check_book_read_cancelled(cancel)?;
                let left_key = pages[order[left]]
                    .item_key
                    .get(prefix_len..)
                    .unwrap_or(&pages[order[left]].item_key);
                let right_key = pages[order[right]]
                    .item_key
                    .get(prefix_len..)
                    .unwrap_or(&pages[order[right]].item_key);
                if compare(left_key, right_key) != std::cmp::Ordering::Greater {
                    scratch[output] = order[left];
                    left += 1;
                } else {
                    scratch[output] = order[right];
                    right += 1;
                }
                output += 1;
            }
            while left < middle {
                check_book_read_cancelled(cancel)?;
                scratch[output] = order[left];
                left += 1;
                output += 1;
            }
            while right < end {
                check_book_read_cancelled(cancel)?;
                scratch[output] = order[right];
                right += 1;
                output += 1;
            }
            start = end;
        }
        std::mem::swap(&mut order, &mut scratch);
        width = width.saturating_mul(2);
    }

    let mut ordinals = BTreeMap::new();
    for (page_index, source_index) in order.into_iter().enumerate() {
        check_book_read_cancelled(cancel)?;
        let page_index = u32::try_from(page_index).map_err(|_| {
            rusqlite::Error::ToSqlConversionFailure(
                "complete ZIP contains more than u32::MAX pages".into(),
            )
        })?;
        if ordinals
            .insert(pages[source_index].item_id, page_index)
            .is_some()
        {
            return Err(rusqlite::Error::ToSqlConversionFailure(
                "complete ZIP contains duplicate item ids".into(),
            ));
        }
    }
    check_book_read_cancelled(cancel)?;
    Ok(ordinals)
}

fn load_book_pages_connection(
    conn: &Connection,
    container_key: &str,
    hash_version: i64,
    cancel: Option<&AtomicBool>,
) -> rusqlite::Result<Vec<SearchRow>> {
    check_book_read_cancelled(cancel)?;
    let mut statement = conn.prepare(&format!(
        "{SEARCH_ROW_SELECT} JOIN container c ON c.container_key = i.container_key
         WHERE i.container_key = ?1 AND i.hash_version = ?2 AND c.scan_state = ?3
         ORDER BY i.page_index"
    ))?;
    let mut query = statement.query(params![
        container_key,
        hash_version,
        ScanState::Complete as i64
    ])?;
    let mut pages = Vec::new();
    while let Some(row) = query.next()? {
        check_book_read_cancelled(cancel)?;
        pages.push(row_to_search_row(row)?);
    }
    drop(query);
    drop(statement);
    check_book_read_cancelled(cancel)?;
    Ok(pages)
}

fn resolve_pages_by_item_id_connection(
    conn: &Connection,
    item_ids: &[u64],
    hash_version: i64,
    cancel: Option<&AtomicBool>,
) -> rusqlite::Result<Vec<SearchRow>> {
    let mut rows = Vec::with_capacity(item_ids.len());
    for &item_id in item_ids {
        check_book_read_cancelled(cancel)?;
        if let Some(row) = load_search_item_by_id(conn, item_id, hash_version)? {
            rows.push(row);
        }
    }
    check_book_read_cancelled(cancel)?;
    Ok(rows)
}

fn resolve_book_hits_raw_connection(
    conn: &Connection,
    item_ids: &[u64],
    hash_version: i64,
    cancel: Option<&AtomicBool>,
) -> rusqlite::Result<Vec<BookHitResolution>> {
    check_book_read_cancelled(cancel)?;
    let mut statement = conn.prepare(
        "SELECT i.item_id, i.revision, i.item_key, i.kind, i.container_key, i.page_index,
                i.mtime, i.file_size, i.hash_version, i.pdq256, i.quality,
                i.width, i.height, i.format, c.scan_state
         FROM item i LEFT JOIN container c ON c.container_key = i.container_key
         WHERE i.item_id = ?1",
    )?;
    let mut hits = Vec::with_capacity(item_ids.len());
    for &item_id in item_ids {
        check_book_read_cancelled(cancel)?;
        let hit = statement
            .query_row([i64::try_from(item_id).unwrap_or(i64::MAX)], |row| {
                let search_row = row_to_search_row(row)?;
                let scan_state = row
                    .get::<_, Option<i64>>(14)?
                    .map(ScanState::from_i64)
                    .transpose()?;
                let eligibility = if search_row.item.hash_version != hash_version {
                    BookHitEligibility::HashMismatch
                } else if search_row.item.container_key.is_some()
                    && scan_state != Some(ScanState::Complete)
                {
                    BookHitEligibility::ContainerNotComplete
                } else {
                    BookHitEligibility::Eligible
                };
                Ok(BookHitResolution::Present {
                    row: search_row,
                    eligibility,
                })
            })
            .optional()?
            .unwrap_or(BookHitResolution::Missing { item_id });
        hits.push(hit);
    }
    drop(statement);
    check_book_read_cancelled(cancel)?;
    Ok(hits)
}

fn i64_to_u32(value: i64, column: usize) -> rusqlite::Result<u32> {
    u32::try_from(value).map_err(|_| rusqlite::Error::IntegralValueOutOfRange(column, value))
}

fn i64_to_u64(value: i64, column: usize) -> rusqlite::Result<u64> {
    u64::try_from(value).map_err(|_| rusqlite::Error::IntegralValueOutOfRange(column, value))
}

fn search_store_id(conn: &Connection) -> rusqlite::Result<[u8; 16]> {
    let store_id = conn.query_row(
        "SELECT store_id FROM search_content_state WHERE singleton = 1",
        [],
        |row| row.get::<_, Vec<u8>>(0),
    )?;
    let store_id: [u8; 16] = store_id.try_into().map_err(|value: Vec<u8>| {
        rusqlite::Error::FromSqlConversionFailure(
            value.len(),
            rusqlite::types::Type::Blob,
            "search content store_id must contain 16 bytes".into(),
        )
    })?;
    Ok(store_id)
}

fn latest_change_seq(conn: &Connection) -> rusqlite::Result<u64> {
    let seq = conn
        .query_row(
            "SELECT seq FROM sqlite_sequence WHERE name = 'item_change'",
            [],
            |row| row.get::<_, i64>(0),
        )
        .optional()?
        .unwrap_or(0);
    i64_to_u64(seq, 0)
}

/// Step 5 より前の店かどうかを、世代番号ではなく**形**で見る。
///
/// 旧 `init_schema` は `PRAGMA user_version` を一度も書いておらず、実店を読むと 0 だった。
/// 新規ファイルも 0 なので番号では区別できない。v1 にしかない形 ——`item` はあるが `item_id`
/// を持たない—— で判定し、`search_content_state` の同居も要求する。片方しかない店は v1 として
/// 読まず、未知の世代と同じく作り直す。
fn is_v1_layout(conn: &Connection) -> rusqlite::Result<bool> {
    let table_exists = |name: &str| -> rusqlite::Result<bool> {
        conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1)",
            [name],
            |row| row.get(0),
        )
    };
    if !table_exists("item")? || !table_exists("search_content_state")? {
        return Ok(false);
    }
    let has_item_id: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM pragma_table_info('item') WHERE name = 'item_id')",
        [],
        |row| row.get(0),
    )?;
    Ok(!has_item_id)
}

/// v1 の店を v2 へ移す。**署名は作り直さない。**
///
/// `dupe` は Step 5 で一行も変わっていないので、保存された PDQ 署名と `quality` の意味は
/// v1 と同一である。v1 と v2 の違いは `item` が `item_id` と `revision` を得たことと、
/// `search_content_state` が使わなくなった `generation` を落としたことだけで、他のテーブルは
/// 定義が一致する。値が同じものを再デコードする理由はない (実店で約 11.6 時間かかる)。
///
/// 旧表をここで退避し、共通の CREATE が新しい形を作った後に [`copy_v1_rows_into_v2`] が
/// 中身を移す。`ALTER TABLE ... RENAME` は索引を連れて行かないため、同名で作り直せるよう
/// 旧索引を先に落とす。
fn move_v1_tables_aside(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        "ALTER TABLE item RENAME TO item_v1;
         DROP INDEX IF EXISTS item_container_idx;
         ALTER TABLE search_content_state RENAME TO search_content_state_v1;",
    )
}

/// 退避した v1 の行を新しい表へ移し、旧表を捨てる。
///
/// `item_id` は AUTOINCREMENT が採番し、`revision` は 1 から始める。`item_change` は空のまま
/// なので、最初の配列構築が SQLite から base を作り直す。`store_id` は引き継ぐ。
fn copy_v1_rows_into_v2(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        "INSERT INTO item (item_key, revision, kind, container_key, page_index, mtime,
                           file_size, hash_version, pdq256, quality, width, height, format)
           SELECT item_key, 1, kind, container_key, page_index, mtime,
                  file_size, hash_version, pdq256, quality, width, height, format
           FROM item_v1;
         INSERT INTO search_content_state (singleton, store_id, page_order_version)
           SELECT singleton, store_id, 0 FROM search_content_state_v1;
         DROP TABLE item_v1;
         DROP TABLE search_content_state_v1;",
    )
}

/// 書き込みを行う transaction。**DEFERRED を使わない。**
///
/// WAL では、読み取りで始まった transaction が後から書こうとしたときに別の接続が書いていると、
/// SQLite は待たずに SQLITE_BUSY を返す。`busy_timeout` はこの昇格を待てないので、
/// 「database is locked」がそのまま呼び出し側の失敗になる。索引 worker と配列更新 worker は
/// どちらもこの店へ書くため、書く側は最初から書き込みロックを取って順番に待つ。
fn write_transaction(conn: &mut Connection) -> rusqlite::Result<Transaction<'_>> {
    conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
}

fn stored_schema_version(conn: &Connection) -> rusqlite::Result<i64> {
    conn.query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
}

fn schema_column_exists(conn: &Connection, table: &str, column: &str) -> rusqlite::Result<bool> {
    conn.query_row(
        "SELECT EXISTS(
           SELECT 1 FROM pragma_table_info(?1) WHERE name = ?2
         )",
        params![table, column],
        |row| row.get(0),
    )
}

fn add_schema_column_if_missing(
    conn: &Connection,
    table: &str,
    column: &str,
    definition: &str,
) -> rusqlite::Result<()> {
    if !schema_column_exists(conn, table, column)? {
        conn.execute_batch(&format!("ALTER TABLE {table} ADD COLUMN {definition};"))?;
    }
    Ok(())
}

/// Populate the normalized parent columns without retaining the complete key corpus in memory.
/// The migration transaction is intentionally atomic; the bounded batches only cap Rust memory.
fn backfill_source_parent_keys(
    conn: &Connection,
    table: &str,
    key_column: &str,
    predicate: &str,
) -> rusqlite::Result<()> {
    let mut after_rowid = i64::MIN;
    loop {
        let rows = {
            let mut statement = conn.prepare(&format!(
                "SELECT rowid, {key_column} FROM {table}
                 WHERE rowid > ?1 AND ({predicate}) ORDER BY rowid LIMIT 4096"
            ))?;
            statement
                .query_map([after_rowid], |row| {
                    Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?
        };
        let Some(&(last_rowid, _)) = rows.last() else {
            break;
        };
        let mut update = conn.prepare(&format!(
            "UPDATE {table} SET source_parent_key = ?2 WHERE rowid = ?1"
        ))?;
        for (rowid, key) in rows {
            update.execute(params![rowid, source_parent_key(&key)])?;
        }
        after_rowid = last_rowid;
    }
    Ok(())
}

fn init_schema(conn: &mut Connection) -> rusqlite::Result<()> {
    // 定常状態では書き込みロックを一切取らない。索引 worker・配列更新 worker・パネル照会・
    // 集計はそれぞれ別の接続でこの店を開くので、開くこと自体が writer になると互いに競合する。
    if stored_schema_version(conn)? == SCHEMA_VERSION {
        return Ok(());
    }
    // 作成と移行は writer なので、読んでから書きへ上げない。WAL の deferred transaction は
    // 読み取り後に別の接続が書いていると SQLITE_BUSY を即返し、これは busy_timeout では
    // 待てない (「database is locked」で開けなくなる)。最初から IMMEDIATE を取る。
    let transaction = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let user_version = stored_schema_version(&transaction)?;
    if user_version == SCHEMA_VERSION {
        // ロックを待っている間に、別の接続が作成か移行を終えていた。
        return transaction.commit();
    }
    // 中断しても旧schemaのまま残るかv5へ移り切るかのどちらかになるよう、退避・作成・移送・
    // user_version の更新を一つの transaction に入れる。
    let migrating_v1 = user_version == 0 && is_v1_layout(&transaction)?;
    if migrating_v1 {
        move_v1_tables_aside(&transaction)?;
    } else if user_version == 2 {
        // v2 の行はそのまま使える。`page_index` の並べ方だけが分からないので 0 を記録し、
        // 起動時の振り直しに任せる。署名は変わらないので再索引は起きない。
        transaction.execute_batch(
            "ALTER TABLE search_content_state
               ADD COLUMN page_order_version INTEGER NOT NULL DEFAULT 0;",
        )?;
    } else if !matches!(user_version, 3 | 4) {
        // Only the known v1/v2/v3/v4 layouts are migrated. Preserve the existing recovery contract
        // for an unknown generation instead of guessing at its table meanings.
        transaction.execute_batch(
            "DROP TABLE IF EXISTS item_change;
             DROP TABLE IF EXISTS item_build;
             DROP TABLE IF EXISTS item_prefill;
             DROP TABLE IF EXISTS container_build;
             DROP TABLE IF EXISTS item;
             DROP TABLE IF EXISTS container;
             DROP TABLE IF EXISTS index_run;
             DROP TABLE IF EXISTS search_content_state;",
        )?;
    }
    transaction.execute_batch(
        "CREATE TABLE IF NOT EXISTS item (
           item_id INTEGER PRIMARY KEY AUTOINCREMENT,
           item_key TEXT NOT NULL UNIQUE,
           revision INTEGER NOT NULL CHECK(revision > 0),
           kind INTEGER NOT NULL,
           container_key TEXT,
           source_parent_key TEXT,
           page_index INTEGER,
           mtime INTEGER NOT NULL,
           file_size INTEGER NOT NULL,
           hash_version INTEGER NOT NULL,
           pdq256 BLOB NOT NULL CHECK(length(pdq256) = 32),
           quality INTEGER NOT NULL,
           width INTEGER NOT NULL,
           height INTEGER NOT NULL,
           format INTEGER NOT NULL
         );
         CREATE INDEX IF NOT EXISTS item_container_idx ON item(container_key, page_index);
         CREATE TABLE IF NOT EXISTS item_change (
           seq INTEGER PRIMARY KEY AUTOINCREMENT,
           item_id INTEGER NOT NULL,
           op INTEGER NOT NULL CHECK(op IN (0, 1, 2)),
           revision INTEGER,
           pdq256 BLOB CHECK(pdq256 IS NULL OR length(pdq256) = 32),
           quality INTEGER,
           CHECK((op = 2 AND revision IS NULL AND pdq256 IS NULL AND quality IS NULL)
              OR (op IN (0, 1) AND revision IS NOT NULL AND pdq256 IS NOT NULL AND quality IS NOT NULL))
         );
         CREATE TABLE IF NOT EXISTS container (
           container_key TEXT PRIMARY KEY,
           source_parent_key TEXT,
           kind INTEGER NOT NULL,
           page_count INTEGER,
           scan_state INTEGER NOT NULL,
           generation INTEGER NOT NULL,
           mtime INTEGER NOT NULL,
           file_size INTEGER NOT NULL
         );
         CREATE TABLE IF NOT EXISTS container_build (
           container_key TEXT PRIMARY KEY,
           source_parent_key TEXT,
           kind INTEGER NOT NULL,
           page_count INTEGER NOT NULL,
           scan_state INTEGER NOT NULL,
           generation INTEGER NOT NULL,
           mtime INTEGER NOT NULL,
           file_size INTEGER NOT NULL
         );
         CREATE TABLE IF NOT EXISTS item_build (
           item_key TEXT NOT NULL,
           kind INTEGER NOT NULL,
           container_key TEXT NOT NULL,
           page_index INTEGER NOT NULL,
           mtime INTEGER NOT NULL,
           file_size INTEGER NOT NULL,
           hash_version INTEGER NOT NULL,
           pdq256 BLOB NOT NULL CHECK(length(pdq256) = 32),
           quality INTEGER NOT NULL,
           width INTEGER NOT NULL,
           height INTEGER NOT NULL,
           format INTEGER NOT NULL,
           generation INTEGER NOT NULL,
           PRIMARY KEY(container_key, generation, item_key)
         );
         CREATE TABLE IF NOT EXISTS item_prefill (
           item_key TEXT PRIMARY KEY,
           mtime INTEGER NOT NULL,
           file_size INTEGER NOT NULL,
           hash_version INTEGER NOT NULL,
           pdq256 BLOB NOT NULL CHECK(length(pdq256) = 32),
           quality INTEGER NOT NULL,
           width INTEGER NOT NULL,
           height INTEGER NOT NULL,
           format INTEGER NOT NULL
         );
         CREATE TABLE IF NOT EXISTS index_run (
           singleton INTEGER PRIMARY KEY CHECK(singleton = 1),
           store_id BLOB NOT NULL CHECK(length(store_id) = 16),
           through_change_seq INTEGER NOT NULL,
           hash_version INTEGER NOT NULL,
           completed_at_unix_secs INTEGER NOT NULL,
           registered_items INTEGER NOT NULL,
           password_required_pdfs INTEGER NOT NULL,
           corrupt_containers INTEGER NOT NULL,
           zero_page_containers INTEGER NOT NULL,
           decode_failures INTEGER NOT NULL,
           io_failures INTEGER NOT NULL
         );
         CREATE TABLE IF NOT EXISTS search_content_state (
           singleton INTEGER PRIMARY KEY CHECK(singleton = 1),
           store_id BLOB NOT NULL CHECK(length(store_id) = 16),
           page_order_version INTEGER NOT NULL
         );
         ",
    )?;
    if migrating_v1 {
        copy_v1_rows_into_v2(&transaction)?;
    }
    let store_id = *uuid::Uuid::new_v4().as_bytes();
    transaction.execute(
        "INSERT OR IGNORE INTO search_content_state (singleton, store_id, page_order_version)
         VALUES (1, ?1, ?2)",
        params![store_id.as_slice(), PAGE_ORDER_VERSION],
    )?;

    add_schema_column_if_missing(
        &transaction,
        "item",
        "source_parent_key",
        "source_parent_key TEXT",
    )?;
    add_schema_column_if_missing(
        &transaction,
        "container",
        "source_parent_key",
        "source_parent_key TEXT",
    )?;
    add_schema_column_if_missing(
        &transaction,
        "container_build",
        "source_parent_key",
        "source_parent_key TEXT",
    )?;
    add_schema_column_if_missing(&transaction, "index_run", "store_id", "store_id BLOB")?;
    add_schema_column_if_missing(
        &transaction,
        "index_run",
        "through_change_seq",
        "through_change_seq INTEGER",
    )?;

    backfill_source_parent_keys(
        &transaction,
        "item",
        "item_key",
        "container_key IS NULL AND source_parent_key IS NULL",
    )?;
    backfill_source_parent_keys(
        &transaction,
        "container",
        "container_key",
        "source_parent_key IS NULL",
    )?;
    backfill_source_parent_keys(
        &transaction,
        "container_build",
        "container_key",
        "source_parent_key IS NULL",
    )?;
    let current_store_id = search_store_id(&transaction)?;
    let through_change_seq = latest_change_seq(&transaction)?;
    transaction.execute(
        "UPDATE index_run
         SET store_id = ?1, through_change_seq = ?2
         WHERE singleton = 1",
        params![
            current_store_id.as_slice(),
            i64::try_from(through_change_seq).unwrap_or(i64::MAX)
        ],
    )?;
    transaction.execute_batch(
        "CREATE INDEX IF NOT EXISTS item_source_parent_idx
           ON item(source_parent_key, item_id) WHERE container_key IS NULL;
         CREATE INDEX IF NOT EXISTS item_loose_key_idx
           ON item(item_key) WHERE container_key IS NULL;
         CREATE INDEX IF NOT EXISTS container_source_parent_idx
           ON container(source_parent_key, kind, container_key);
         PRAGMA user_version = 5;",
    )?;
    transaction.commit()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(key: &str, container: Option<&str>, page: Option<u32>, marker: u8) -> StoredItem {
        StoredItem {
            item_key: key.to_owned(),
            kind: ItemKind::Image,
            container_key: container.map(str::to_owned),
            page_index: page,
            mtime: 10,
            file_size: 20,
            hash_version: current_hash_version(),
            pdq256: [marker; 32],
            quality: 50,
            width: 100,
            height: 80,
            format: 1,
        }
    }

    fn publish_test_book(db: &SimilarDb, book: &str, pages: &[(&str, u8)]) {
        let generation = db
            .begin_container_build(book, ContainerKind::ImageFolder, pages.len() as u32, 1, 2)
            .unwrap();
        for (page_index, (key, marker)) in pages.iter().enumerate() {
            db.stage_item(
                generation,
                &item(key, Some(book), Some(page_index as u32), *marker),
            )
            .unwrap();
        }
        db.complete_container(book, generation).unwrap();
    }

    fn publish_test_container(
        db: &SimilarDb,
        key: &str,
        kind: ContainerKind,
        pages: &[(&str, u8)],
    ) {
        let generation = db
            .begin_container_build(key, kind, pages.len() as u32, 1, 2)
            .unwrap();
        for (page_index, (page_key, marker)) in pages.iter().enumerate() {
            let mut page = item(page_key, Some(key), Some(page_index as u32), *marker);
            page.kind = match kind {
                ContainerKind::Zip => ItemKind::ZipPage,
                ContainerKind::Pdf => ItemKind::PdfPage,
                ContainerKind::ImageFolder => ItemKind::Image,
            };
            db.stage_item(generation, &page).unwrap();
        }
        db.complete_container(key, generation).unwrap();
    }

    fn delta_inventory(db: &SimilarDb, plan: DeltaScopePlan) -> DeltaScopedInventory {
        db.load_delta_scoped_inventory(plan, current_hash_version(), || true)
            .unwrap()
            .unwrap()
    }

    fn downgrade_current_store(path: &Path, schema_version: i64) {
        let conn = Connection::open(path).unwrap();
        conn.execute_batch("DROP INDEX item_loose_key_idx;")
            .unwrap();
        if schema_version < 4 {
            conn.execute_batch(
                "DROP INDEX item_source_parent_idx;
                 DROP INDEX container_source_parent_idx;
                 ALTER TABLE item DROP COLUMN source_parent_key;
                 ALTER TABLE container DROP COLUMN source_parent_key;
                 ALTER TABLE container_build DROP COLUMN source_parent_key;
                 ALTER TABLE index_run DROP COLUMN store_id;
                 ALTER TABLE index_run DROP COLUMN through_change_seq;",
            )
            .unwrap();
        }
        if schema_version == 2 {
            conn.execute_batch(
                "ALTER TABLE search_content_state DROP COLUMN page_order_version;
                 ALTER TABLE search_content_state
                   ADD COLUMN generation INTEGER NOT NULL DEFAULT 0;",
            )
            .unwrap();
        }
        conn.execute_batch(&format!("PRAGMA user_version = {schema_version};"))
            .unwrap();
    }

    #[test]
    fn book_reader_keeps_metadata_base_delta_pages_and_targets_in_one_snapshot() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("similar.db");
        let db = SimilarDb::open_at(&path).unwrap();
        publish_test_book(&db, "book", &[("book/old", 0x11)]);

        let old_base = db.load_base_search_rows(current_hash_version()).unwrap();
        let old_changes = db.load_item_changes_after(0).unwrap();
        let old_pages = db.load_book_pages("book", current_hash_version()).unwrap();
        let old_item_id = old_pages[0].item_id;
        let mut reader = SimilarBookReader::open_at(&path).unwrap().unwrap();
        let cancel = Arc::new(AtomicBool::new(false));

        let (metadata, base, changes, pages, targets, origin) = reader
            .with_snapshot(Arc::clone(&cancel), |snapshot| {
                let metadata = snapshot.metadata();
                // The metadata SELECT above fixed this read snapshot. A separate WAL writer may
                // publish a new complete generation without changing any later read in this TX.
                publish_test_book(&db, "book", &[("book/new-a", 0x22), ("book/new-b", 0x33)]);
                Ok((
                    metadata,
                    snapshot.load_base_search_rows(current_hash_version())?,
                    snapshot.load_item_changes_after(0)?,
                    snapshot.load_book_pages("book", current_hash_version())?,
                    snapshot.resolve_pages_by_item_id(&[old_item_id], current_hash_version())?,
                    snapshot.load_item("book/old", current_hash_version())?,
                ))
            })
            .unwrap();

        assert_eq!(metadata.store_id, old_base.store_id);
        assert_eq!(metadata.read_seq, old_base.applied_seq);
        assert_eq!(metadata.page_order_version, PAGE_ORDER_VERSION);
        assert_eq!(base.store_id, old_base.store_id);
        assert_eq!(base.applied_seq, old_base.applied_seq);
        assert_eq!(base.records, old_base.records);
        assert_eq!(changes, old_changes);
        assert_eq!(pages, old_pages);
        assert_eq!(targets, old_pages);
        assert_eq!(origin, Some(old_pages[0].clone()));

        let (next_metadata, next_pages) = reader
            .with_snapshot(cancel, |snapshot| {
                Ok((
                    snapshot.metadata(),
                    snapshot.load_book_pages("book", current_hash_version())?,
                ))
            })
            .unwrap();
        assert!(next_metadata.read_seq > metadata.read_seq);
        assert_eq!(
            next_pages
                .iter()
                .map(|page| page.item.item_key.as_str())
                .collect::<Vec<_>>(),
            vec!["book/new-a", "book/new-b"]
        );
    }

    #[test]
    fn book_reader_raw_hit_resolution_preserves_order_duplicates_and_ineligibility() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("similar.db");
        let db = SimilarDb::open_at(&path).unwrap();
        publish_test_book(&db, "eligible", &[("eligible/page", 0x11)]);
        publish_test_book(&db, "stale", &[("stale/page", 0x22)]);
        publish_test_book(&db, "building", &[("building/page", 0x33)]);

        let (eligible_id, stale_id, building_id) = {
            let conn = db.conn.lock().unwrap_or_else(|error| error.into_inner());
            let id_for = |key: &str| {
                conn.query_row(
                    "SELECT item_id FROM item WHERE item_key = ?1",
                    [key],
                    |row| i64_to_u64(row.get(0)?, 0),
                )
                .unwrap()
            };
            let ids = (
                id_for("eligible/page"),
                id_for("stale/page"),
                id_for("building/page"),
            );
            conn.execute(
                "UPDATE item SET hash_version = ?1 WHERE item_id = ?2",
                params![current_hash_version() - 1, i64::try_from(ids.1).unwrap()],
            )
            .unwrap();
            conn.execute(
                "UPDATE container SET scan_state = ?1 WHERE container_key = 'building'",
                [ScanState::Building as i64],
            )
            .unwrap();
            ids
        };
        let missing_id = u64::MAX - 1;
        let requested = [eligible_id, missing_id, stale_id, building_id, eligible_id];

        let mut reader = SimilarBookReader::open_at(&path).unwrap().unwrap();
        let (raw, filtered) = reader
            .with_snapshot(Arc::new(AtomicBool::new(false)), |snapshot| {
                Ok((
                    snapshot.resolve_book_hits_raw(&requested, current_hash_version())?,
                    snapshot.resolve_pages_by_item_id(&requested, current_hash_version())?,
                ))
            })
            .unwrap();

        assert_eq!(raw.len(), requested.len());
        assert!(matches!(
            &raw[0],
            BookHitResolution::Present {
                row,
                eligibility: BookHitEligibility::Eligible,
            } if row.item_id == eligible_id && row.item.item_key == "eligible/page"
        ));
        assert_eq!(
            raw[1],
            BookHitResolution::Missing {
                item_id: missing_id
            }
        );
        assert!(matches!(
            &raw[2],
            BookHitResolution::Present {
                row,
                eligibility: BookHitEligibility::HashMismatch,
            } if row.item_id == stale_id && row.item.item_key == "stale/page"
        ));
        assert!(matches!(
            &raw[3],
            BookHitResolution::Present {
                row,
                eligibility: BookHitEligibility::ContainerNotComplete,
            } if row.item_id == building_id && row.item.item_key == "building/page"
        ));
        assert_eq!(raw[4], raw[0]);
        assert_eq!(
            filtered.iter().map(|row| row.item_id).collect::<Vec<_>>(),
            vec![eligible_id, eligible_id]
        );
    }

    #[test]
    fn book_reader_private_zip_order_matches_writer_repair_before_filtering() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("similar.db");
        let db = SimilarDb::open_at(&path).unwrap();
        let book = "book.zip";
        let separator = '\u{1f}';
        let names = ["same-b", "old", "a", "quality", "same-a"];
        let generation = db
            .begin_container_build(book, ContainerKind::Zip, names.len() as u32, 1, 2)
            .unwrap();
        for (page_index, name) in names.iter().enumerate() {
            let mut page = item(
                &format!("{book}{separator}{name}"),
                Some(book),
                Some(page_index as u32),
                page_index as u8 + 1,
            );
            page.kind = ItemKind::ZipPage;
            db.stage_item(generation, &page).unwrap();
        }
        db.complete_container(book, generation).unwrap();

        let (old_id, same_a_id) = {
            let conn = db.conn.lock().unwrap_or_else(|error| error.into_inner());
            conn.execute(
                "UPDATE item SET hash_version = ?1, page_index = 99 WHERE item_key = ?2",
                params![current_hash_version() - 1, format!("{book}{separator}old")],
            )
            .unwrap();
            conn.execute(
                "UPDATE item SET quality = 0 WHERE item_key = ?1",
                [format!("{book}{separator}quality")],
            )
            .unwrap();
            conn.execute(
                "UPDATE item SET page_index = NULL WHERE item_key = ?1",
                [format!("{book}{separator}same-a")],
            )
            .unwrap();
            conn.execute(
                "UPDATE search_content_state SET page_order_version = 0 WHERE singleton = 1",
                [],
            )
            .unwrap();
            (
                conn.query_row(
                    "SELECT item_id FROM item WHERE item_key = ?1",
                    [format!("{book}{separator}old")],
                    |row| i64_to_u64(row.get(0)?, 0),
                )
                .unwrap(),
                conn.query_row(
                    "SELECT item_id FROM item WHERE item_key = ?1",
                    [format!("{book}{separator}same-a")],
                    |row| i64_to_u64(row.get(0)?, 0),
                )
                .unwrap(),
            )
        };
        let before_seq = db.load_item_changes_after(0).unwrap().latest_seq;
        let compare = |left: &str, right: &str| {
            let rank = |value: &str| match value {
                "a" => 0,
                "old" => 1,
                "same-a" | "same-b" => 2,
                "quality" => 3,
                _ => 4,
            };
            rank(left).cmp(&rank(right))
        };

        let mut reader = SimilarBookReader::open_at(&path).unwrap().unwrap();
        let (private_pages, private_targets, private_raw_targets) = reader
            .with_snapshot(Arc::new(AtomicBool::new(false)), |snapshot| {
                let mut resolver = snapshot.page_order_resolver(compare);
                let pages = resolver.load_book_pages(book, current_hash_version())?;
                let targets = resolver.resolve_pages_by_item_id(
                    &[same_a_id, old_id, pages[0].item_id],
                    current_hash_version(),
                )?;
                let raw_targets = resolver.resolve_book_hits_raw(
                    &[same_a_id, old_id, pages[0].item_id],
                    current_hash_version(),
                )?;
                Ok((pages, targets, raw_targets))
            })
            .unwrap();
        let private_projection = private_pages
            .iter()
            .map(|row| {
                (
                    row.item
                        .item_key
                        .rsplit(separator)
                        .next()
                        .unwrap()
                        .to_owned(),
                    row.item.page_index,
                    row.item.quality,
                )
            })
            .collect::<Vec<_>>();
        assert_eq!(
            private_projection,
            vec![
                ("a".to_owned(), Some(0), 50),
                ("same-a".to_owned(), Some(2), 50),
                ("same-b".to_owned(), Some(3), 50),
                ("quality".to_owned(), Some(4), 0),
            ]
        );
        assert_eq!(
            private_targets
                .iter()
                .map(|row| (
                    row.item.item_key.rsplit(separator).next().unwrap(),
                    row.item.page_index
                ))
                .collect::<Vec<_>>(),
            vec![("same-a", Some(2)), ("a", Some(0))]
        );
        assert!(matches!(
            &private_raw_targets[0],
            BookHitResolution::Present {
                row,
                eligibility: BookHitEligibility::Eligible,
            } if row.item.item_key.ends_with("same-a") && row.item.page_index == Some(2)
        ));
        assert!(matches!(
            &private_raw_targets[1],
            BookHitResolution::Present {
                row,
                eligibility: BookHitEligibility::HashMismatch,
            } if row.item.item_key.ends_with("old") && row.item.page_index == Some(99)
        ));
        assert!(matches!(
            &private_raw_targets[2],
            BookHitResolution::Present {
                row,
                eligibility: BookHitEligibility::Eligible,
            } if row.item.item_key.ends_with('a') && row.item.page_index == Some(0)
        ));
        assert_eq!(db.page_order_version().unwrap(), 0);
        assert_eq!(
            db.load_item_changes_after(0).unwrap().latest_seq,
            before_seq
        );

        db.conn
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .execute(
                "UPDATE search_content_state SET page_order_version = ?1 WHERE singleton = 1",
                [PAGE_ORDER_VERSION + 1],
            )
            .unwrap();
        let future_version_pages = reader
            .with_snapshot(Arc::new(AtomicBool::new(false)), |snapshot| {
                snapshot
                    .page_order_resolver(compare)
                    .load_book_pages(book, current_hash_version())
            })
            .unwrap();
        assert_eq!(future_version_pages, private_pages);
        db.conn
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .execute(
                "UPDATE search_content_state SET page_order_version = 0 WHERE singleton = 1",
                [],
            )
            .unwrap();

        assert_eq!(
            db.renumber_container_pages(PAGE_ORDER_VERSION, compare)
                .unwrap(),
            1
        );
        let repaired_pages = db.load_book_pages(book, current_hash_version()).unwrap();
        let repaired_targets = db
            .resolve_pages_by_item_id(
                &[same_a_id, old_id, repaired_pages[0].item_id],
                current_hash_version(),
            )
            .unwrap();
        assert_eq!(repaired_pages, private_pages);
        assert_eq!(repaired_targets, private_targets);
        assert_eq!(
            db.load_item_changes_after(0).unwrap().latest_seq,
            before_seq
        );
    }

    #[test]
    fn cancellable_page_ordering_propagates_interrupt() {
        let pages = (0..4_096)
            .map(|index| ContainerPageKey {
                item_id: index,
                item_key: format!("book.zip\u{1f}{:08}", 4_096 - index),
            })
            .collect::<Vec<_>>();
        let cancel = AtomicBool::new(false);
        let comparisons = std::sync::atomic::AtomicUsize::new(0);
        let compare = |left: &str, right: &str| {
            if comparisons.fetch_add(1, Ordering::Relaxed) == 31 {
                cancel.store(true, Ordering::Release);
            }
            left.cmp(right)
        };
        let result = canonical_page_ordinals("book.zip", &pages, &compare, Some(&cancel));
        assert_eq!(comparisons.load(Ordering::Relaxed), 32);
        assert!(
            matches!(result, Err(rusqlite::Error::SqliteFailure(error, _)) if error.code == rusqlite::ErrorCode::OperationInterrupted)
        );
    }

    #[test]
    fn stale_zip_order_cache_has_a_weighted_fifo_limit() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("similar.db");
        let db = SimilarDb::open_at(&path).unwrap();
        let separator = '\u{1f}';
        for (book, page_count) in [("a.zip", 1), ("b.zip", 1), ("c.zip", 1), ("large.zip", 2)] {
            let kind = if book == "large.zip" {
                ContainerKind::Zip
            } else {
                ContainerKind::ImageFolder
            };
            let generation = db
                .begin_container_build(book, kind, page_count, 1, 1)
                .unwrap();
            for page in 0..page_count {
                let mut row = item(
                    &format!("{book}{separator}{page}"),
                    Some(book),
                    Some(page),
                    page as u8,
                );
                row.kind = if kind == ContainerKind::Zip {
                    ItemKind::ZipPage
                } else {
                    ItemKind::Image
                };
                db.stage_item(generation, &row).unwrap();
            }
            db.complete_container(book, generation).unwrap();
        }
        db.conn
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .execute(
                "UPDATE search_content_state SET page_order_version = 0 WHERE singleton = 1",
                [],
            )
            .unwrap();

        let mut reader = SimilarBookReader::open_at(&path).unwrap().unwrap();
        reader
            .with_snapshot(Arc::new(AtomicBool::new(false)), |snapshot| {
                let mut resolver = snapshot.page_order_resolver(str::cmp);
                resolver.effective_zip_order_weight_limit = usize::MAX;
                resolver.effective_zip_order_entry_limit = 2;

                resolver.load_book_pages("a.zip", current_hash_version())?;
                resolver.load_book_pages("b.zip", current_hash_version())?;
                // Cache hits do not alter FIFO insertion order.
                resolver.load_book_pages("a.zip", current_hash_version())?;
                assert_eq!(
                    resolver
                        .effective_zip_order_fifo
                        .iter()
                        .map(String::as_str)
                        .collect::<Vec<_>>(),
                    vec!["a.zip", "b.zip"]
                );
                assert_eq!(resolver.effective_zip_order_weight, 2);

                resolver.load_book_pages("c.zip", current_hash_version())?;
                assert_eq!(
                    resolver
                        .effective_zip_order_fifo
                        .iter()
                        .map(String::as_str)
                        .collect::<Vec<_>>(),
                    vec!["b.zip", "c.zip"]
                );
                assert_eq!(resolver.effective_zip_order_weight, 2);

                resolver.effective_zip_orders.clear();
                resolver.effective_zip_order_fifo.clear();
                resolver.effective_zip_order_weight = 0;
                resolver.effective_zip_order_weight_limit = 1;
                resolver.effective_zip_order_entry_limit = usize::MAX;
                let pages = resolver.load_book_pages("large.zip", current_hash_version())?;
                assert_eq!(pages.len(), 2);
                assert!(resolver.effective_zip_orders.is_empty());
                assert!(resolver.effective_zip_order_fifo.is_empty());
                assert_eq!(resolver.effective_zip_order_weight, 0);
                Ok(())
            })
            .unwrap();
    }

    #[test]
    fn book_reader_distinguishes_missing_corrupt_and_unsupported_stores() {
        let tmp = tempfile::TempDir::new().unwrap();
        let missing = tmp.path().join("missing.db");
        assert!(SimilarBookReader::open_at(&missing).unwrap().is_none());

        let directory = tmp.path().join("directory.db");
        std::fs::create_dir(&directory).unwrap();
        assert!(matches!(
            SimilarBookReader::open_at(&directory),
            Err(BookReadError::Database(_))
        ));

        let corrupt = tmp.path().join("corrupt.db");
        std::fs::write(&corrupt, b"not a sqlite database").unwrap();
        match SimilarBookReader::open_at(&corrupt) {
            Err(BookReadError::Database(_)) => {}
            Ok(Some(mut reader)) => assert!(matches!(
                reader.with_snapshot(Arc::new(AtomicBool::new(false)), |_| Ok(())),
                Err(BookReadError::Database(_))
            )),
            Ok(None) | Err(BookReadError::Cancelled | BookReadError::Io(_)) => {
                panic!("a present corrupt store was classified as absent or cancelled")
            }
            Err(BookReadError::UnsupportedSchema { .. }) => {
                panic!("a corrupt store was classified as a valid alternate schema")
            }
        }

        let unsupported = tmp.path().join("unsupported.db");
        let db = SimilarDb::open_at(&unsupported).unwrap();
        db.conn
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .execute_batch(&format!("PRAGMA user_version = {};", SCHEMA_VERSION + 1))
            .unwrap();
        drop(db);
        let mut reader = SimilarBookReader::open_at(&unsupported).unwrap().unwrap();
        assert!(matches!(
            reader.with_snapshot(Arc::new(AtomicBool::new(false)), |_| Ok(())),
            Err(BookReadError::UnsupportedSchema {
                found,
                expected: SCHEMA_VERSION
            }) if found == SCHEMA_VERSION + 1
        ));
    }

    #[test]
    fn book_reader_sql_cancel_is_request_scoped_and_connection_is_reusable() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("similar.db");
        let writer = SimilarDb::open_at(&path).unwrap();
        let mut reader = SimilarBookReader::open_at(&path).unwrap().unwrap();
        let cancel = Arc::new(AtomicBool::new(false));
        let cancel_weak = Arc::downgrade(&cancel);
        let worker_cancel = Arc::clone(&cancel);
        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            let interrupted = reader.with_snapshot(Arc::clone(&worker_cancel), |snapshot| {
                started_tx.send(()).unwrap();
                snapshot.conn.query_row(
                    "WITH RECURSIVE count(value) AS (
                         VALUES(0) UNION ALL SELECT value + 1 FROM count WHERE value < 100000000
                     ) SELECT sum(value) FROM count",
                    [],
                    |row| row.get::<_, i64>(0),
                )?;
                Ok(())
            });
            drop(worker_cancel);
            (interrupted, reader)
        });

        let started = started_rx.recv_timeout(Duration::from_secs(3));
        cancel.store(true, Ordering::Release);
        let concurrent_write = writer.upsert_loose_item(&item("writer", None, None, 0x77));
        let joined = worker.join();
        assert!(
            started.is_ok(),
            "long reader query did not start: {started:?}"
        );
        let (interrupted, mut reader) = joined.expect("book reader worker panicked");
        assert!(matches!(interrupted, Err(BookReadError::Cancelled)));
        assert!(
            concurrent_write.is_ok(),
            "cancelled read transaction interfered with a separate WAL writer: {concurrent_write:?}"
        );
        assert!(
            writer
                .load_item("writer", current_hash_version())
                .unwrap()
                .is_some()
        );
        drop(cancel);
        assert!(
            cancel_weak.upgrade().is_none(),
            "cancelled request hook retained its token"
        );
        assert!(
            reader
                .with_snapshot(Arc::new(AtomicBool::new(false)), |snapshot| {
                    Ok(snapshot.metadata())
                })
                .is_ok(),
            "cancelled request left its read transaction active"
        );

        let error_token = Arc::new(AtomicBool::new(false));
        let error_token_weak = Arc::downgrade(&error_token);
        let failed = reader.with_snapshot(Arc::clone(&error_token), |snapshot| {
            snapshot.conn.query_row(
                "SELECT value FROM table_that_does_not_exist",
                [],
                |_| Ok(()),
            )
        });
        assert!(matches!(failed, Err(BookReadError::Database(_))));
        drop(error_token);
        assert!(
            error_token_weak.upgrade().is_none(),
            "failed request hook retained its token"
        );
        assert!(
            reader
                .with_snapshot(Arc::new(AtomicBool::new(false)), |snapshot| {
                    Ok(snapshot.metadata())
                })
                .is_ok(),
            "SQL error left the reader transaction or hook active"
        );

        let panic_token = Arc::new(AtomicBool::new(false));
        let panic_token_weak = Arc::downgrade(&panic_token);
        let panicked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _ = reader.with_snapshot(
                Arc::clone(&panic_token),
                |_snapshot| -> rusqlite::Result<()> {
                    panic!("test panic after progress hook installation")
                },
            );
        }));
        assert!(panicked.is_err());
        drop(panic_token);
        assert!(
            panic_token_weak.upgrade().is_none(),
            "panicked request hook retained its token"
        );
        assert!(
            reader
                .with_snapshot(Arc::new(AtomicBool::new(false)), |snapshot| {
                    Ok(snapshot.metadata())
                })
                .is_ok(),
            "panic left the reader transaction or hook active"
        );
    }

    #[test]
    fn book_reader_is_read_only_and_uses_the_bounded_busy_timeout() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("similar.db");
        drop(SimilarDb::open_at(&path).unwrap());
        let reader = SimilarBookReader::open_at(&path).unwrap().unwrap();
        let timeout_ms: i64 = reader
            .conn
            .query_row("PRAGMA busy_timeout", [], |row| row.get(0))
            .unwrap();
        assert_eq!(timeout_ms, BOOK_READER_BUSY_TIMEOUT.as_millis() as i64);
        assert!(reader.conn.execute("DELETE FROM item", []).is_err());
    }

    /// Step 5 より前の店を作る。移行が「同じ値を運べたか」を問えるように、DDL は当時のまま
    /// 書き写す。ここを現在の定義から生成すると、移行が壊れても気付けない。
    ///
    /// `PRAGMA user_version` を**設定しない**のが要点。旧 `init_schema` は書いておらず、
    /// 実店 (4,628,611 行) を読んでも 0 だった。世代番号で移行を選ぶと、この店は新規ファイル
    /// と区別されずに捨てられる。
    fn create_v1_store(path: &std::path::Path, store_id: [u8; 16]) {
        let conn = Connection::open(path).unwrap();
        conn.execute_batch(
            "CREATE TABLE item (
               item_key TEXT PRIMARY KEY,
               kind INTEGER NOT NULL,
               container_key TEXT,
               page_index INTEGER,
               mtime INTEGER NOT NULL,
               file_size INTEGER NOT NULL,
               hash_version INTEGER NOT NULL,
               pdq256 BLOB NOT NULL CHECK(length(pdq256) = 32),
               quality INTEGER NOT NULL,
               width INTEGER NOT NULL,
               height INTEGER NOT NULL,
               format INTEGER NOT NULL
             );
             CREATE INDEX IF NOT EXISTS item_container_idx ON item(container_key, page_index);
             CREATE TABLE container (
               container_key TEXT PRIMARY KEY,
               kind INTEGER NOT NULL,
               page_count INTEGER,
               scan_state INTEGER NOT NULL,
               generation INTEGER NOT NULL,
               mtime INTEGER NOT NULL,
               file_size INTEGER NOT NULL
             );
             CREATE TABLE search_content_state (
               singleton INTEGER PRIMARY KEY CHECK(singleton = 1),
               store_id BLOB NOT NULL CHECK(length(store_id) = 16),
               generation INTEGER NOT NULL CHECK(generation >= 0)
             );",
        )
        .unwrap();
        conn.execute(
            "INSERT INTO search_content_state (singleton, store_id, generation) VALUES (1, ?1, 7)",
            [store_id.as_slice()],
        )
        .unwrap();
        let mut insert = conn
            .prepare(
                "INSERT INTO item (item_key, kind, container_key, page_index, mtime, file_size,
                                   hash_version, pdq256, quality, width, height, format)
                 VALUES (?1, ?2, NULL, NULL, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            )
            .unwrap();
        for (key, marker, quality) in [("a", 0x11u8, 40i64), ("b", 0x22, 60)] {
            insert
                .execute(params![
                    key,
                    ItemKind::Image as i64,
                    10i64,
                    20i64,
                    current_hash_version(),
                    [marker; 32].as_slice(),
                    quality,
                    100i64,
                    80i64,
                    1i64
                ])
                .unwrap();
        }
    }

    #[test]
    fn v1_store_keeps_its_signatures_instead_of_rehashing() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("similar.db");
        let store_id = [0x5au8; 16];
        create_v1_store(&path, store_id);

        let db = SimilarDb::open_at(&path).unwrap();
        let base = db.load_base_search_rows(current_hash_version()).unwrap();

        // 再デコードしていたら署名は同じでも「移行できた」ことにならないので、行が残って
        // いること自体を先に確かめる。索引ジョブはこのテストでは一度も走っていない。
        assert_eq!(base.records.len(), 2);
        let signatures = base
            .records
            .iter()
            .map(|record| record.signature[0])
            .collect::<Vec<_>>();
        assert_eq!(signatures, vec![0x11, 0x22]);
        assert_eq!(
            base.records.iter().map(|r| r.quality).collect::<Vec<_>>(),
            vec![40, 60]
        );
        // item_id は採番され、重複しない。revision は 1 から始まる。
        assert_eq!(
            base.records.iter().map(|r| r.item_id).collect::<Vec<_>>(),
            vec![1, 2]
        );
        assert!(base.records.iter().all(|record| record.revision == 1));
        assert_eq!(base.store_id, store_id);
        assert_eq!(base.applied_seq, 0);

        let key_a = db.load_item("a", current_hash_version()).unwrap().unwrap();
        assert_eq!(key_a.item.pdq256, [0x11; 32]);
        assert_eq!(key_a.item.width, 100);
    }

    /// WAL 化済みの v1 store で既存 writer が移行を塞いでいる間は待機し、解放後に
    /// 署名と行を保ったまま現行 schema へ移行する。
    #[test]
    fn v1_migration_waits_for_an_existing_wal_writer_and_preserves_rows() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("similar.db");
        let store_id = [0x6bu8; 16];
        create_v1_store(&path, store_id);

        // WAL 変換と schema migration の待機を分ける。ここで WAL へ変換しておけば、
        // opener は journal-mode writer ではなく既存 IMMEDIATE writer の解放を待つ。
        let wal = Connection::open(&path).unwrap();
        wal.execute_batch("PRAGMA journal_mode=WAL;").unwrap();
        drop(wal);

        let blocker = Connection::open(&path).unwrap();
        blocker.execute_batch("BEGIN IMMEDIATE;").unwrap();

        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let (result_tx, result_rx) = std::sync::mpsc::channel();
        let opener_path = path.clone();
        let opener = std::thread::spawn(move || {
            started_tx.send(()).unwrap();
            let result = SimilarDb::open_at(&opener_path)
                .and_then(|db| db.load_base_search_rows(current_hash_version()));
            result_tx.send(result).unwrap();
        });

        started_rx.recv().unwrap();
        let early = result_rx.recv_timeout(std::time::Duration::from_millis(100));

        blocker.execute_batch("ROLLBACK;").unwrap();
        let (finished_while_held, terminal) = match early {
            Ok(result) => (true, Ok(result)),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => (
                false,
                result_rx.recv_timeout(std::time::Duration::from_secs(5)),
            ),
            Err(error @ std::sync::mpsc::RecvTimeoutError::Disconnected) => (false, Err(error)),
        };
        let joined = opener.join();

        assert!(joined.is_ok(), "v1 migration opener panicked");
        assert!(
            !finished_while_held,
            "v1 migration completed while its IMMEDIATE writer was still held"
        );
        let base = terminal
            .expect("v1 migration opener did not return a terminal result after writer release")
            .unwrap();

        assert_eq!(base.records.len(), 2);
        assert_eq!(
            base.records
                .iter()
                .map(|record| record.signature[0])
                .collect::<Vec<_>>(),
            vec![0x11, 0x22]
        );
        assert_eq!(base.store_id, store_id);
        assert_eq!(base.applied_seq, 0);
    }

    /// Every known normalized store generation is migrated in place.  The fixture deliberately
    /// removes the newer columns/index after publishing data so a version-number-only implementation would
    /// either drop the corpus or fail to reconstruct its parent and summary baseline.
    #[test]
    fn schema_v5_migrates_v2_v3_and_v4_in_place_and_reopens() {
        for legacy_version in [2, 3, 4] {
            let tmp = tempfile::TempDir::new().unwrap();
            let path = tmp.path().join(format!("v{legacy_version}.db"));
            let expected_store;
            {
                let db = SimilarDb::open_at(&path).unwrap();
                db.upsert_loose_item(&item("c:/library/kept.jpg", None, None, 9))
                    .unwrap();
                publish_test_container(
                    &db,
                    "c:/library/book.zip",
                    ContainerKind::Zip,
                    &[("c:/library/book.zip\u{1f}page.jpg", 7)],
                );
                db.record_completed_index(
                    current_hash_version(),
                    123,
                    CompletedIndexStats {
                        io_failures: 4,
                        ..CompletedIndexStats::default()
                    },
                )
                .unwrap();
                expected_store = db.search_store_id().unwrap();
            }
            downgrade_current_store(&path, legacy_version);
            if legacy_version == 4 {
                let conn = Connection::open(&path).unwrap();
                conn.execute_batch(
                    "CREATE TRIGGER reject_v4_item_parent_rewrite
                       BEFORE UPDATE OF source_parent_key ON item
                       BEGIN SELECT RAISE(ABORT, 'v4 item parent was already populated'); END;
                     CREATE TRIGGER reject_v4_container_parent_rewrite
                       BEFORE UPDATE OF source_parent_key ON container
                       BEGIN SELECT RAISE(ABORT, 'v4 container parent was already populated'); END;
                     CREATE TRIGGER reject_v4_build_parent_rewrite
                       BEFORE UPDATE OF source_parent_key ON container_build
                       BEGIN SELECT RAISE(ABORT, 'v4 build parent was already populated'); END;",
                )
                .unwrap();
            }

            for _ in 0..2 {
                let db = SimilarDb::open_at(&path).unwrap();
                let base = db.load_base_search_rows(current_hash_version()).unwrap();
                assert_eq!(base.records.len(), 2);
                assert_eq!(base.store_id, expected_store);
                let summary = db
                    .load_index_summary(current_hash_version())
                    .unwrap()
                    .unwrap();
                assert_eq!(summary.registered_items, 2);
                assert_eq!(summary.stats.io_failures, 4);
                let conn = db.conn.lock().unwrap_or_else(|error| error.into_inner());
                assert_eq!(stored_schema_version(&conn).unwrap(), SCHEMA_VERSION);
                assert!(schema_column_exists(&conn, "item", "source_parent_key").unwrap());
                assert_eq!(
                    conn.query_row(
                        "SELECT COUNT(*) FROM sqlite_master
                         WHERE type = 'index' AND name = 'item_loose_key_idx'",
                        [],
                        |row| row.get::<_, i64>(0),
                    )
                    .unwrap(),
                    1
                );
                assert_eq!(
                    conn.query_row(
                        "SELECT source_parent_key FROM item WHERE item_key = 'c:/library/kept.jpg'",
                        [],
                        |row| row.get::<_, String>(0),
                    )
                    .unwrap(),
                    "c:/library"
                );
                assert_eq!(
                    conn.query_row(
                        "SELECT source_parent_key FROM container
                         WHERE container_key = 'c:/library/book.zip'",
                        [],
                        |row| row.get::<_, String>(0),
                    )
                    .unwrap(),
                    "c:/library"
                );
            }
        }
    }

    #[test]
    fn schema_v5_migration_failure_rolls_back_and_can_be_reopened() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("rollback.db");
        {
            let db = SimilarDb::open_at(&path).unwrap();
            db.upsert_loose_item(&item("c:/library/kept.jpg", None, None, 9))
                .unwrap();
        }
        downgrade_current_store(&path, 3);
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute(
                "INSERT INTO item
                 (revision, item_key, kind, container_key, page_index, mtime, file_size,
                  hash_version, pdq256, quality, width, height, format)
                 VALUES (1, x'FF', 0, NULL, NULL, 1, 1, ?1, zeroblob(32), 1, 1, 1, 1)",
                [current_hash_version()],
            )
            .unwrap();
        }

        assert!(SimilarDb::open_at(&path).is_err());
        {
            let conn = Connection::open(&path).unwrap();
            assert_eq!(stored_schema_version(&conn).unwrap(), 3);
            assert!(!schema_column_exists(&conn, "item", "source_parent_key").unwrap());
            conn.execute("DELETE FROM item WHERE typeof(item_key) = 'blob'", [])
                .unwrap();
        }
        let reopened = SimilarDb::open_at(&path).unwrap();
        assert!(
            reopened
                .load_item("c:/library/kept.jpg", current_hash_version())
                .unwrap()
                .is_some()
        );
        assert_eq!(
            stored_schema_version(&reopened.conn.lock().unwrap()).unwrap(),
            SCHEMA_VERSION
        );
    }

    /// 一括移行を待てる時間が実際に設定されていること。既定の 5 秒に戻ると、移行中に別 worker
    /// が開いた瞬間に「database is locked」で落ちる。
    #[test]
    fn a_bulk_migration_can_be_waited_out_by_another_opener() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("similar.db");
        let db = SimilarDb::open_at(&path).unwrap();
        let conn = db.conn.lock().unwrap();
        let timeout_ms: i64 = conn
            .query_row("PRAGMA busy_timeout", [], |row| row.get(0))
            .unwrap();
        assert!(
            timeout_ms >= 120_000,
            "busy_timeout {timeout_ms} ms is shorter than a measured 38.9 s migration"
        );
    }

    /// fresh DB の初回 WAL 変換へ複数 worker が同時に入っても、open 自体を失敗させない。
    ///
    /// 既存の並列回帰は先に一度 open していたため、全接続が既に WAL を読む定常経路しか覆わず、
    /// index worker と memory loader の初回競合を検出できなかった。
    #[test]
    fn opening_fresh_stores_from_many_workers_at_once_never_fails() {
        let tmp = tempfile::TempDir::new().unwrap();
        for round in 0..8 {
            let path = tmp.path().join(format!("fresh-{round}")).join("similar.db");
            let barrier = std::sync::Arc::new(std::sync::Barrier::new(8));
            let handles = (0..8)
                .map(|_| {
                    let path = path.clone();
                    let barrier = std::sync::Arc::clone(&barrier);
                    std::thread::spawn(move || {
                        barrier.wait();
                        SimilarDb::open_at(&path).map(|_| ())
                    })
                })
                .collect::<Vec<_>>();
            let errors = handles
                .into_iter()
                .filter_map(|handle| handle.join().unwrap().err())
                .collect::<Vec<_>>();
            assert!(errors.is_empty(), "round {round} failed: {errors:?}");
        }
    }

    /// 既に WAL の store は active writer がいても journal mode の変換を試みない。
    #[test]
    fn opening_an_existing_wal_store_does_not_contend_with_its_writer() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("similar.db");
        let owner = SimilarDb::open_at(&path).unwrap();
        let owner_conn = owner.conn.lock().unwrap_or_else(|error| error.into_inner());
        owner_conn.execute_batch("BEGIN IMMEDIATE;").unwrap();

        let reopened = SimilarDb::open_at(&path);

        owner_conn.execute_batch("ROLLBACK;").unwrap();
        assert!(
            reopened.is_ok(),
            "an existing WAL store open tried to take the writer lock: {:?}",
            reopened.err()
        );
    }

    /// 同じ店を複数の worker が同時に開いても失敗しないこと。
    ///
    /// 索引 worker・配列更新 worker・パネル照会・集計はそれぞれ別の接続でこの店を開く。
    /// どれか一つでも開けないと、その経路は結果を返せずに諦める。
    #[test]
    fn opening_the_same_store_from_many_workers_at_once_never_fails() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("similar.db");
        SimilarDb::open_at(&path).unwrap();

        let barrier = std::sync::Arc::new(std::sync::Barrier::new(8));
        let failures = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let mut handles = Vec::new();
        for _ in 0..8 {
            let path = path.clone();
            let barrier = std::sync::Arc::clone(&barrier);
            let failures = std::sync::Arc::clone(&failures);
            handles.push(std::thread::spawn(move || {
                barrier.wait();
                for _ in 0..20 {
                    match SimilarDb::open_at(&path) {
                        Ok(db) => {
                            db.upsert_loose_item(&item("k", None, None, 1)).unwrap();
                        }
                        Err(error) => {
                            eprintln!("open failed: {error}");
                            failures.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        }
                    }
                }
            }));
        }
        for handle in handles {
            handle.join().unwrap();
        }
        assert_eq!(failures.load(std::sync::atomic::Ordering::Relaxed), 0);
    }

    /// 実店の複製に対して移行を通す手動確認。`MIV_SIMILAR_V1_COPY` に **複製の** パスを渡す。
    /// 元の店を指さないこと。移行はその場でファイルを書き換える。
    #[test]
    #[ignore = "manual migration check against a caller-supplied copy of a real v1 store"]
    fn migrate_a_real_v1_store_copy() {
        let path = std::path::PathBuf::from(
            std::env::var("MIV_SIMILAR_V1_COPY").expect("MIV_SIMILAR_V1_COPY"),
        );
        let before =
            Connection::open_with_flags(&path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
        let expected_rows: i64 = before
            .query_row("SELECT COUNT(*) FROM item", [], |row| row.get(0))
            .unwrap();
        let sample: (String, Vec<u8>, i64) = before
            .query_row(
                "SELECT item_key, pdq256, quality FROM item ORDER BY item_key LIMIT 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        drop(before);

        let started = std::time::Instant::now();
        let db = SimilarDb::open_at(&path).unwrap();
        let elapsed = started.elapsed();
        let base = db.load_base_search_rows(current_hash_version()).unwrap();
        println!(
            "migrated {expected_rows} rows in {:.1}s, {} searchable",
            elapsed.as_secs_f64(),
            base.records.len()
        );

        let migrated = db
            .load_item(&sample.0, current_hash_version())
            .unwrap()
            .unwrap();
        assert_eq!(migrated.item.pdq256.as_slice(), sample.1.as_slice());
        assert_eq!(migrated.item.quality as i64, sample.2);
        assert_eq!(migrated.revision, 1);
    }

    /// 読めない世代は移行せず作り直す。v1 の移行を足したことで、この経路が消えていないこと。
    #[test]
    fn an_unknown_schema_generation_is_still_rebuilt() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("similar.db");
        create_v1_store(&path, [0x5a; 16]);
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch("PRAGMA user_version = 99;").unwrap();
        drop(conn);

        let db = SimilarDb::open_at(&path).unwrap();
        let base = db.load_base_search_rows(current_hash_version()).unwrap();
        assert!(base.records.is_empty());
    }

    #[test]
    fn building_generation_is_not_searchable_and_cleanup_removes_it() {
        let db = SimilarDb::open_in_memory().unwrap();
        let generation = db
            .begin_container_build("book", ContainerKind::ImageFolder, 2, 1, 0)
            .unwrap();
        db.stage_item(generation, &item("a", Some("book"), Some(0), 1))
            .unwrap();

        assert!(
            db.load_search_rows(current_hash_version())
                .unwrap()
                .is_empty()
        );
        assert!(
            db.load_base_search_rows(current_hash_version())
                .unwrap()
                .records
                .is_empty()
        );
        assert_eq!(db.count_staged(), 1);
        db.cleanup_incomplete().unwrap();
        assert_eq!(db.count_staged(), 0);
        assert!(
            db.load_search_rows(current_hash_version())
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn item_change_tracks_only_published_changes() {
        let db = SimilarDb::open_in_memory().unwrap();
        let initial_store = db.search_store_id().unwrap();
        assert_eq!(db.load_item_changes_after(0).unwrap().latest_seq, 0);

        let generation = db
            .begin_container_build("book", ContainerKind::ImageFolder, 1, 1, 0)
            .unwrap();
        db.stage_item(generation, &item("page", Some("book"), Some(0), 1))
            .unwrap();
        assert_eq!(db.load_item_changes_after(0).unwrap().latest_seq, 0);

        db.complete_container("book", generation).unwrap();
        let after_publish = db.load_item_changes_after(0).unwrap();
        assert_eq!(db.search_store_id().unwrap(), initial_store);
        assert_eq!(after_publish.changes.len(), 1);
        assert_eq!(after_publish.changes[0].op, ItemChangeOp::Add);

        let loose = item("loose", None, None, 2);
        assert!(db.upsert_loose_item(&loose).unwrap());
        let after_loose = db.load_item_changes_after(0).unwrap();
        assert_eq!(after_loose.latest_seq, after_publish.latest_seq + 1);
        assert!(!db.upsert_loose_item(&loose).unwrap());
        assert_eq!(db.load_item_changes_after(0).unwrap(), after_loose);

        let seen_items = HashSet::from(["loose".to_owned()]);
        let seen_containers = HashSet::new();
        assert!(db.prune_except_seen(&seen_items, &seen_containers).unwrap() > 0);
        let after_prune = db.load_item_changes_after(0).unwrap();
        assert!(after_prune.latest_seq > after_loose.latest_seq);
        assert!(
            after_prune
                .changes
                .iter()
                .any(|change| change.op == ItemChangeOp::Delete)
        );
    }

    #[test]
    fn item_and_change_log_roll_back_together() {
        let db = SimilarDb::open_in_memory().unwrap();
        db.conn
            .lock()
            .unwrap()
            .execute_batch(
                "CREATE TRIGGER reject_item_change
                 BEFORE INSERT ON item_change
                 BEGIN
                   SELECT RAISE(ABORT, 'test item_change failure');
                 END;",
            )
            .unwrap();

        assert!(
            db.upsert_loose_item(&item("never-published", None, None, 1))
                .is_err()
        );
        assert!(
            db.load_item("never-published", current_hash_version())
                .unwrap()
                .is_none()
        );
        assert_eq!(db.load_item_changes_after(0).unwrap().latest_seq, 0);
    }

    #[test]
    fn each_store_gets_a_distinct_content_identity() {
        let first = SimilarDb::open_in_memory()
            .unwrap()
            .search_store_id()
            .unwrap();
        let second = SimilarDb::open_in_memory()
            .unwrap()
            .search_store_id()
            .unwrap();
        assert_ne!(first, second);
    }

    #[test]
    fn generation_swap_is_atomic_and_cancel_preserves_complete_generation() {
        let db = SimilarDb::open_in_memory().unwrap();
        let first = db
            .begin_container_build("book", ContainerKind::ImageFolder, 1, 1, 0)
            .unwrap();
        db.stage_item(first, &item("old", Some("book"), Some(0), 1))
            .unwrap();
        db.complete_container("book", first).unwrap();
        let compact = db.load_base_search_rows(current_hash_version()).unwrap();
        assert_eq!(compact.records.len(), 1);
        let old_item_id = compact.records[0].item_id;
        assert_eq!(
            db.load_item_by_id(old_item_id, current_hash_version())
                .unwrap()
                .unwrap()
                .item
                .item_key,
            "old"
        );

        let second = db
            .begin_container_build("book", ContainerKind::ImageFolder, 2, 2, 0)
            .unwrap();
        db.stage_item(second, &item("new-a", Some("book"), Some(0), 2))
            .unwrap();
        let during = db.load_search_rows(current_hash_version()).unwrap();
        assert_eq!(during.len(), 1);
        assert_eq!(during[0].item.item_key, "old");

        // cancel/cleanup しても公開済み世代はそのまま。
        db.cleanup_incomplete().unwrap();
        assert_eq!(db.load_search_rows(current_hash_version()).unwrap(), during);

        let second = db
            .begin_container_build("book", ContainerKind::ImageFolder, 2, 2, 0)
            .unwrap();
        db.stage_item(second, &item("new-a", Some("book"), Some(0), 2))
            .unwrap();
        db.stage_item(second, &item("new-b", Some("book"), Some(1), 3))
            .unwrap();
        db.complete_container("book", second).unwrap();
        let after = db.load_search_rows(current_hash_version()).unwrap();
        assert_eq!(after.len(), 2);
        assert!(
            after
                .iter()
                .all(|row| row.item.item_key.starts_with("new-"))
        );
        assert_eq!(db.load_complete_containers().unwrap()[0].generation, 2);
    }

    #[test]
    fn hash_and_proxy_versions_invalidate_rows() {
        let db = SimilarDb::open_in_memory().unwrap();
        db.upsert_loose_item(&item("a", None, None, 1)).unwrap();
        assert_eq!(
            db.item_freshness("a", 10, 20, current_hash_version())
                .unwrap(),
            Freshness::Current
        );
        assert_eq!(
            db.item_freshness("a", 10, 20, current_hash_version() + 1)
                .unwrap(),
            Freshness::Stale
        );
        assert!(
            db.load_search_rows(current_hash_version() + 1)
                .unwrap()
                .is_empty()
        );
        assert_ne!(
            hash_version(HASH_ALGORITHM_VERSION, crate::dupe::PROXY_VERSION),
            hash_version(HASH_ALGORITHM_VERSION, crate::dupe::PROXY_VERSION + 1)
        );
        assert_ne!(
            hash_version(HASH_ALGORITHM_VERSION, crate::dupe::PROXY_VERSION),
            hash_version(HASH_ALGORITHM_VERSION + 1, crate::dupe::PROXY_VERSION)
        );
    }

    #[test]
    fn only_changed_item_is_recomputed() {
        let db = SimilarDb::open_in_memory().unwrap();
        let first = item("a", None, None, 1);
        assert!(db.upsert_loose_item(&first).unwrap());
        assert!(!db.upsert_loose_item(&first).unwrap());

        let mut changed = first.clone();
        changed.file_size += 1;
        assert!(db.upsert_loose_item(&changed).unwrap());
        assert_eq!(
            db.load_search_rows(current_hash_version()).unwrap().len(),
            1
        );
        let changes = db.load_item_changes_after(0).unwrap();
        assert_eq!(changes.changes.len(), 2);
        assert_eq!(changes.changes[0].item_id, changes.changes[1].item_id);
        assert_eq!(changes.changes[1].revision, Some(2));
    }

    #[test]
    fn deleted_item_id_is_never_reused_for_the_same_key() {
        let db = SimilarDb::open_in_memory().unwrap();
        db.upsert_loose_item(&item("same", None, None, 1)).unwrap();
        let first_id = db
            .load_item("same", current_hash_version())
            .unwrap()
            .unwrap()
            .item_id;
        assert_eq!(
            db.prune_except_seen(&HashSet::new(), &HashSet::new())
                .unwrap(),
            1
        );
        db.upsert_loose_item(&item("same", None, None, 2)).unwrap();
        let second_id = db
            .load_item("same", current_hash_version())
            .unwrap()
            .unwrap()
            .item_id;
        assert!(second_id > first_id);
        let changes = db.load_item_changes_after(0).unwrap();
        assert_eq!(
            changes.changes.iter().map(|c| c.op).collect::<Vec<_>>(),
            vec![ItemChangeOp::Add, ItemChangeOp::Delete, ItemChangeOp::Add]
        );
    }

    #[test]
    fn completed_index_summary_counts_only_searchable_rows_and_round_trips_failures() {
        let db = SimilarDb::open_in_memory().unwrap();
        db.upsert_loose_item(&item("loose", None, None, 1)).unwrap();
        let complete = db
            .begin_container_build("complete", ContainerKind::Zip, 1, 1, 10)
            .unwrap();
        db.stage_item(
            complete,
            &item("complete-page", Some("complete"), Some(0), 2),
        )
        .unwrap();
        db.complete_container("complete", complete).unwrap();
        let building = db
            .begin_container_build("building", ContainerKind::Pdf, 1, 1, 10)
            .unwrap();
        db.stage_item(
            building,
            &item("building-page", Some("building"), Some(0), 3),
        )
        .unwrap();

        let stats = CompletedIndexStats {
            password_required_pdfs: 1,
            corrupt_containers: 2,
            zero_page_containers: 3,
            decode_failures: 4,
            io_failures: 5,
        };
        let stored = db
            .record_completed_index(current_hash_version(), 1234, stats)
            .unwrap();
        assert_eq!(stored.registered_items, 2);
        assert_eq!(
            db.load_index_summary(current_hash_version()).unwrap(),
            Some(stored)
        );
        assert_eq!(
            db.load_index_summary(current_hash_version() + 1).unwrap(),
            None
        );
    }

    #[test]
    fn completed_scope_snapshot_prunes_rows_from_disabled_roots() {
        let db = SimilarDb::open_in_memory().unwrap();
        db.upsert_loose_item(&item("c:/enabled/a.jpg", None, None, 1))
            .unwrap();
        db.upsert_loose_item(&item("c:/disabled/b.jpg", None, None, 2))
            .unwrap();

        let seen_items = HashSet::from(["c:/enabled/a.jpg".to_owned()]);
        assert_eq!(
            db.prune_except_seen(&seen_items, &HashSet::new()).unwrap(),
            1
        );
        let rows = db.load_search_rows(current_hash_version()).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].item.item_key, "c:/enabled/a.jpg");
    }

    #[test]
    fn disabled_favorite_purge_preserves_enabled_overlap_and_prefix_sibling() {
        let db = SimilarDb::open_in_memory().unwrap();
        for (key, marker) in [
            ("c:/library/drop/a.jpg", 1),
            ("c:/library/keep/a.jpg", 2),
            ("c:/library-old/a.jpg", 3),
        ] {
            db.upsert_loose_item(&item(key, None, None, marker))
                .unwrap();
        }
        let drop_book = db
            .begin_container_build("c:/library/drop/book.zip", ContainerKind::Zip, 1, 1, 10)
            .unwrap();
        db.stage_item(
            drop_book,
            &item(
                "c:/library/drop/book.zip\u{1f}page.jpg",
                Some("c:/library/drop/book.zip"),
                Some(0),
                4,
            ),
        )
        .unwrap();
        db.complete_container("c:/library/drop/book.zip", drop_book)
            .unwrap();
        let building = db
            .begin_container_build("c:/library/drop/building.zip", ContainerKind::Zip, 1, 1, 10)
            .unwrap();
        db.stage_item(
            building,
            &item(
                "c:/library/drop/building.zip\u{1f}page.jpg",
                Some("c:/library/drop/building.zip"),
                Some(0),
                5,
            ),
        )
        .unwrap();
        let prefill = item("c:/library/drop/prefill.jpg", None, None, 6);
        db.put_prefill(&prefill).unwrap();
        db.record_completed_index(current_hash_version(), 1234, CompletedIndexStats::default())
            .unwrap();

        let removed = db
            .purge_roots_except(&["c:/library".to_owned()], &["c:/library/keep".to_owned()])
            .unwrap();
        // loose + complete page/container + Building page/public/build row + prefill。
        assert_eq!(removed, 7);

        let keys = db
            .load_search_rows(current_hash_version())
            .unwrap()
            .into_iter()
            .map(|row| row.item.item_key)
            .collect::<Vec<_>>();
        assert_eq!(
            keys,
            vec![
                "c:/library/keep/a.jpg".to_owned(),
                "c:/library-old/a.jpg".to_owned(),
            ]
        );
        assert!(db.load_complete_containers().unwrap().is_empty());
        assert_eq!(db.count_staged(), 0);
        assert!(
            db.load_prefill(
                &prefill.item_key,
                prefill.mtime,
                prefill.file_size,
                prefill.hash_version,
            )
            .unwrap()
            .is_none()
        );
        assert_eq!(
            db.load_index_summary(current_hash_version())
                .unwrap()
                .unwrap()
                .registered_items,
            2
        );
    }

    #[test]
    fn incremental_reconcile_skipped_conditional_purge_rolls_back_and_is_typed() {
        let db = SimilarDb::open_in_memory().unwrap();
        db.upsert_loose_item(&item("c:/library/a.jpg", None, None, 1))
            .unwrap();
        let before = db.load_search_rows(current_hash_version()).unwrap();
        let checks = std::sync::atomic::AtomicUsize::new(0);

        let result = db
            .purge_roots_except_if(&["c:/library".to_owned()], &[], || {
                checks.fetch_add(1, Ordering::AcqRel) == 0
            })
            .unwrap();
        assert_eq!(result, ConditionalCommit::Skipped);
        assert_eq!(db.load_search_rows(current_hash_version()).unwrap(), before);

        assert_eq!(
            db.purge_roots_except_if(&["c:/library".to_owned()], &[], || false)
                .unwrap(),
            ConditionalCommit::Skipped
        );
        assert_eq!(db.load_search_rows(current_hash_version()).unwrap(), before);
    }

    #[test]
    fn incremental_reconcile_failed_container_keeps_previous_complete_pages() {
        let db = SimilarDb::open_in_memory().unwrap();
        let generation = db
            .begin_container_build("c:/library/book.zip", ContainerKind::Zip, 1, 1, 10)
            .unwrap();
        db.stage_item(
            generation,
            &item(
                "c:/library/book.zip\u{1f}001.jpg",
                Some("c:/library/book.zip"),
                Some(0),
                7,
            ),
        )
        .unwrap();
        db.complete_container("c:/library/book.zip", generation)
            .unwrap();

        let seen_containers = HashSet::from(["c:/library/book.zip".to_owned()]);
        assert_eq!(
            db.prune_except_seen(&HashSet::new(), &seen_containers)
                .unwrap(),
            0
        );
        let rows = db.load_search_rows(current_hash_version()).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].item.item_key, "c:/library/book.zip\u{1f}001.jpg");
    }

    #[test]
    fn incremental_reconcile_delta_publish_switches_book_to_loose_atomically() {
        let db = SimilarDb::open_in_memory().unwrap();
        let book = "c:/library/book";
        let page_key = "c:/library/book/001.jpg";
        let generation = db
            .begin_container_build(book, ContainerKind::ImageFolder, 1, 1, 0)
            .unwrap();
        db.stage_item(generation, &item(page_key, Some(book), Some(0), 1))
            .unwrap();
        db.complete_container(book, generation).unwrap();

        let loose = item(page_key, None, Some(0), 2);
        let seen_items = HashSet::from([page_key.to_owned()]);
        db.publish_delta_reconcile(
            &[loose],
            &[],
            &[book.to_owned()],
            &[],
            &[],
            &seen_items,
            &HashSet::new(),
            current_hash_version(),
            123,
        )
        .unwrap();

        let rows = db.load_search_rows(current_hash_version()).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].item.item_key, page_key);
        assert_eq!(rows[0].item.container_key, None);
        assert!(db.load_complete_containers().unwrap().is_empty());
    }

    #[test]
    fn incremental_reconcile_failed_delta_publish_rolls_back_the_scope() {
        let db = SimilarDb::open_in_memory().unwrap();
        db.upsert_loose_item(&item("c:/library/keep.jpg", None, None, 1))
            .unwrap();
        let before = db.load_search_rows(current_hash_version()).unwrap();
        let new_item = item("c:/library/new.jpg", None, None, 2);
        let seen_items = HashSet::from([new_item.item_key.clone()]);

        assert!(
            db.publish_delta_reconcile(
                &[new_item],
                &[("c:/library/missing.zip".to_owned(), 99)],
                &["c:/library".to_owned()],
                &[],
                &[],
                &seen_items,
                &HashSet::new(),
                current_hash_version(),
                124,
            )
            .is_err()
        );
        assert_eq!(db.load_search_rows(current_hash_version()).unwrap(), before);
    }

    #[test]
    fn incremental_reconcile_cancelled_delta_rolls_back_rows_prune_and_summary() {
        let db = SimilarDb::open_in_memory().unwrap();
        db.upsert_loose_item(&item("c:/library/keep.jpg", None, None, 1))
            .unwrap();
        let previous_summary = db
            .record_completed_index(current_hash_version(), 100, CompletedIndexStats::default())
            .unwrap();
        let before = db.load_search_rows(current_hash_version()).unwrap();
        let new_item = item("c:/library/new.jpg", None, None, 2);
        let seen_items = HashSet::from([new_item.item_key.clone()]);
        let publish_checks = std::sync::atomic::AtomicUsize::new(0);

        assert!(
            db.publish_delta_reconcile_if(
                &[new_item],
                &[],
                &["c:/library".to_owned()],
                &[],
                &[],
                &seen_items,
                &HashSet::new(),
                current_hash_version(),
                200,
                || publish_checks.fetch_add(1, Ordering::AcqRel) == 0,
            )
            .is_err()
        );
        assert_eq!(db.load_search_rows(current_hash_version()).unwrap(), before);
        assert_eq!(
            db.load_index_summary(current_hash_version()).unwrap(),
            Some(previous_summary)
        );
    }

    #[test]
    fn delta_scoped_directory_contents_selects_only_observed_ownership() {
        let db = SimilarDb::open_in_memory().unwrap();
        db.upsert_loose_item(&item("c:/library/loose.jpg", None, None, 1))
            .unwrap();
        publish_test_container(
            &db,
            "c:/library/archive.zip",
            ContainerKind::Zip,
            &[("c:/library/archive.zip\u{1f}page.jpg", 2)],
        );
        publish_test_container(
            &db,
            "c:/library/document.pdf",
            ContainerKind::Pdf,
            &[("c:/library/document.pdf\u{1f}0", 3)],
        );
        publish_test_book(
            &db,
            "c:/library/child-book",
            &[("c:/library/child-book/page.jpg", 4)],
        );
        db.record_completed_index(current_hash_version(), 1, CompletedIndexStats::default())
            .unwrap();

        let inventory = delta_inventory(
            &db,
            DeltaScopePlan {
                directory_contents: vec!["c:/library".to_owned()],
                ..DeltaScopePlan::default()
            },
        );
        let mut items = inventory
            .items_by_key
            .keys()
            .map(String::as_str)
            .collect::<Vec<_>>();
        items.sort_unstable();
        assert_eq!(
            items,
            vec![
                "c:/library/archive.zip\u{1f}page.jpg",
                "c:/library/document.pdf\u{1f}0",
                "c:/library/loose.jpg",
            ]
        );
        assert!(
            inventory
                .containers_by_key
                .contains_key("c:/library/archive.zip")
        );
        assert!(
            inventory
                .containers_by_key
                .contains_key("c:/library/document.pdf")
        );
        assert!(
            !inventory
                .containers_by_key
                .contains_key("c:/library/child-book")
        );
        assert!(
            !inventory
                .items_by_key
                .contains_key("c:/library/child-book/page.jpg")
        );
    }

    #[test]
    fn delta_scoped_drive_root_parent_is_preserved() {
        assert_eq!(source_parent_key("c:/root.jpg"), "c:/");
        assert_eq!(source_parent_key("c:/library/root.jpg"), "c:/library");
        assert_eq!(
            source_parent_key("//server/share/root.jpg"),
            "//server/share"
        );

        let db = SimilarDb::open_in_memory().unwrap();
        db.upsert_loose_item(&item("c:/root.jpg", None, None, 1))
            .unwrap();
        publish_test_container(
            &db,
            "c:/archive.zip",
            ContainerKind::Zip,
            &[("c:/archive.zip\u{1f}page.jpg", 2)],
        );
        db.record_completed_index(current_hash_version(), 1, CompletedIndexStats::default())
            .unwrap();

        let inventory = delta_inventory(
            &db,
            DeltaScopePlan {
                directory_contents: vec!["c:/".to_owned()],
                ..DeltaScopePlan::default()
            },
        );
        assert!(inventory.items_by_key.contains_key("c:/root.jpg"));
        assert!(inventory.containers_by_key.contains_key("c:/archive.zip"));
        assert!(
            inventory
                .items_by_key
                .contains_key("c:/archive.zip\u{1f}page.jpg")
        );
    }

    #[test]
    fn delta_scoped_child_scope_retires_an_image_book_without_pruning_its_sibling() {
        let db = SimilarDb::open_in_memory().unwrap();
        publish_test_book(
            &db,
            "c:/library/child-book",
            &[("c:/library/child-book/page.jpg", 1)],
        );
        publish_test_book(
            &db,
            "c:/library/child-book-old",
            &[("c:/library/child-book-old/page.jpg", 2)],
        );
        db.record_completed_index(current_hash_version(), 1, CompletedIndexStats::default())
            .unwrap();
        let inventory = delta_inventory(
            &db,
            DeltaScopePlan {
                removed_prefixes: vec!["c:/library/child-book".to_owned()],
                ..DeltaScopePlan::default()
            },
        );

        let result = db
            .publish_delta_scoped_reconcile_if(
                inventory,
                &[],
                &[],
                current_hash_version(),
                2,
                || true,
            )
            .unwrap();
        let DeltaPublishResult::Committed { removed, watermark } = result else {
            panic!("valid scoped deletion must commit");
        };
        assert_eq!(removed, 2, "one page and its container are retired");
        assert!(watermark.through_change_seq > 0);
        let keys = db
            .load_search_rows(current_hash_version())
            .unwrap()
            .into_iter()
            .map(|row| row.item.item_key)
            .collect::<Vec<_>>();
        assert_eq!(keys, vec!["c:/library/child-book-old/page.jpg"]);
        assert_eq!(
            db.load_index_summary(current_hash_version())
                .unwrap()
                .unwrap()
                .registered_items,
            1
        );
    }

    #[test]
    fn delta_scoped_identity_checks_preserve_new_members_owner_moves_and_generations() {
        let db = SimilarDb::open_in_memory().unwrap();
        publish_test_book(&db, "c:/library/book", &[("c:/library/book/old.jpg", 1)]);
        db.record_completed_index(current_hash_version(), 1, CompletedIndexStats::default())
            .unwrap();
        let inventory = delta_inventory(
            &db,
            DeltaScopePlan {
                subtrees: vec!["c:/library".to_owned()],
                ..DeltaScopePlan::default()
            },
        );
        {
            let conn = db.conn.lock().unwrap_or_else(|error| error.into_inner());
            conn.execute(
                "UPDATE item SET container_key = NULL, source_parent_key = 'c:/library/book',
                                 page_index = NULL
                 WHERE item_key = 'c:/library/book/old.jpg'",
                [],
            )
            .unwrap();
            conn.execute(
                "UPDATE container SET generation = generation + 1
                 WHERE container_key = 'c:/library/book'",
                [],
            )
            .unwrap();
            publish_item(
                &conn,
                None,
                &item(
                    "c:/library/book/new.jpg",
                    Some("c:/library/book"),
                    Some(1),
                    2,
                ),
            )
            .unwrap();
            // Keep the baseline exact for this adversarial identity-only race. Production writes
            // advance item_change and therefore conservatively schedule a Full before finalizing.
            conn.execute(
                "UPDATE index_run SET through_change_seq =
                   COALESCE((SELECT seq FROM sqlite_sequence WHERE name = 'item_change'), 0)
                 WHERE singleton = 1",
                [],
            )
            .unwrap();
        }

        let result = db
            .publish_delta_scoped_reconcile_if(
                inventory,
                &[],
                &[],
                current_hash_version(),
                2,
                || true,
            )
            .unwrap();
        assert!(matches!(
            result,
            DeltaPublishResult::Committed { removed: 0, .. }
        ));
        let mut keys = db
            .load_search_rows(current_hash_version())
            .unwrap()
            .into_iter()
            .map(|row| row.item.item_key)
            .collect::<Vec<_>>();
        keys.sort();
        assert_eq!(
            keys,
            vec![
                "c:/library/book/new.jpg".to_owned(),
                "c:/library/book/old.jpg".to_owned(),
            ]
        );
    }

    #[test]
    fn delta_scoped_invalid_summary_requests_full_without_mutation() {
        for invalidation in ["store", "hash", "seq"] {
            let db = SimilarDb::open_in_memory().unwrap();
            db.upsert_loose_item(&item("c:/library/keep.jpg", None, None, 1))
                .unwrap();
            db.record_completed_index(current_hash_version(), 1, CompletedIndexStats::default())
                .unwrap();
            let inventory = delta_inventory(
                &db,
                DeltaScopePlan {
                    directory_contents: vec!["c:/library".to_owned()],
                    ..DeltaScopePlan::default()
                },
            );
            let before = db.load_search_rows(current_hash_version()).unwrap();
            {
                let conn = db.conn.lock().unwrap_or_else(|error| error.into_inner());
                match invalidation {
                    "store" => {
                        conn.execute(
                            "UPDATE index_run SET store_id = zeroblob(16) WHERE singleton = 1",
                            [],
                        )
                        .unwrap();
                    }
                    "hash" => {
                        conn.execute(
                            "UPDATE index_run SET hash_version = hash_version + 1
                             WHERE singleton = 1",
                            [],
                        )
                        .unwrap();
                    }
                    "seq" => {
                        conn.execute(
                            "UPDATE index_run SET through_change_seq = through_change_seq + 1
                             WHERE singleton = 1",
                            [],
                        )
                        .unwrap();
                    }
                    _ => unreachable!(),
                }
            }
            let result = db
                .publish_delta_scoped_reconcile_if(
                    inventory,
                    &[],
                    &[],
                    current_hash_version(),
                    2,
                    || true,
                )
                .unwrap();
            assert_eq!(result, DeltaPublishResult::RequiresFull, "{invalidation}");
            assert_eq!(db.load_search_rows(current_hash_version()).unwrap(), before);
        }
    }

    #[test]
    fn delta_scoped_cancel_rolls_back_rows_journal_and_summary() {
        let db = SimilarDb::open_in_memory().unwrap();
        db.upsert_loose_item(&item("c:/library/remove.jpg", None, None, 1))
            .unwrap();
        let summary = db
            .record_completed_index(current_hash_version(), 1, CompletedIndexStats::default())
            .unwrap();
        let inventory = delta_inventory(
            &db,
            DeltaScopePlan {
                directory_contents: vec!["c:/library".to_owned()],
                ..DeltaScopePlan::default()
            },
        );
        let before_rows = db.load_search_rows(current_hash_version()).unwrap();
        let before_changes = db.load_item_changes_after(0).unwrap();
        let checks = AtomicUsize::new(0);

        let result = db
            .publish_delta_scoped_reconcile_if(
                inventory,
                &[],
                &[],
                current_hash_version(),
                2,
                || checks.fetch_add(1, Ordering::AcqRel) == 0,
            )
            .unwrap();
        assert_eq!(result, DeltaPublishResult::Skipped);
        assert_eq!(
            db.load_search_rows(current_hash_version()).unwrap(),
            before_rows
        );
        assert_eq!(db.load_item_changes_after(0).unwrap(), before_changes);
        assert_eq!(
            db.load_index_summary(current_hash_version()).unwrap(),
            Some(summary)
        );
    }

    #[test]
    fn delta_scoped_parent_indexes_keep_candidate_count_independent_of_store_size() {
        let db = SimilarDb::open_in_memory().unwrap();
        {
            let conn = db.conn.lock().unwrap_or_else(|error| error.into_inner());
            conn.execute(
                "WITH RECURSIVE ids(value) AS (
                   VALUES(1) UNION ALL SELECT value + 1 FROM ids WHERE value < 10000
                 )
                 INSERT INTO item
                   (revision, item_key, kind, container_key, source_parent_key, page_index,
                    mtime, file_size, hash_version, pdq256, quality, width, height, format)
                 SELECT 1, printf('c:/outside/%05d.jpg', value), 0, NULL, 'c:/outside', NULL,
                        1, 1, ?1, zeroblob(32), 1, 1, 1, 1 FROM ids",
                [current_hash_version()],
            )
            .unwrap();
        }
        db.upsert_loose_item(&item("c:/target/a.jpg", None, None, 1))
            .unwrap();
        db.upsert_loose_item(&item("c:/target/b.jpg", None, None, 2))
            .unwrap();
        db.record_completed_index(current_hash_version(), 1, CompletedIndexStats::default())
            .unwrap();

        let inventory = delta_inventory(
            &db,
            DeltaScopePlan {
                directory_contents: vec!["c:/target".to_owned()],
                ..DeltaScopePlan::default()
            },
        );
        assert_eq!(inventory.items.len(), 2);
        let conn = db.conn.lock().unwrap_or_else(|error| error.into_inner());
        let detail = conn
            .prepare(
                "EXPLAIN QUERY PLAN SELECT item_id FROM item
                 WHERE source_parent_key = ?1 AND container_key IS NULL",
            )
            .unwrap()
            .query_map(["c:/target"], |row| row.get::<_, String>(3))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap()
            .join(" | ");
        assert!(detail.contains("item_source_parent_idx"), "{detail}");
    }

    #[test]
    fn delta_scoped_loose_loader_cancels_mid_stream_and_releases_the_database() {
        let db = SimilarDb::open_in_memory().unwrap();
        {
            let conn = db.conn.lock().unwrap_or_else(|error| error.into_inner());
            conn.execute(
                "WITH RECURSIVE ids(value) AS (
                   VALUES(1) UNION ALL SELECT value + 1 FROM ids WHERE value < 9000
                 )
                 INSERT INTO item
                   (revision, item_key, kind, container_key, source_parent_key, page_index,
                    mtime, file_size, hash_version, pdq256, quality, width, height, format)
                 SELECT 1, printf('c:/target/%05d.jpg', value), 0, NULL, 'c:/target', NULL,
                        1, 1, ?1, zeroblob(32), 1, 1, 1, 1 FROM ids",
                [current_hash_version()],
            )
            .unwrap();
        }
        let polls = AtomicUsize::new(0);
        let inventory = db
            .load_delta_scoped_inventory(
                DeltaScopePlan {
                    directory_contents: vec!["c:/target".to_owned()],
                    ..DeltaScopePlan::default()
                },
                current_hash_version(),
                || polls.fetch_add(1, Ordering::AcqRel) < 2,
            )
            .unwrap();
        assert!(inventory.is_none());
        assert_eq!(polls.load(Ordering::Acquire), 3);
        let conn = db.conn.lock().unwrap_or_else(|error| error.into_inner());
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM item", [], |row| row.get::<_, i64>(0))
                .unwrap(),
            9000
        );
    }

    #[test]
    fn delta_scoped_container_member_loader_cancels_mid_stream_and_releases_the_database() {
        let db = SimilarDb::open_in_memory().unwrap();
        {
            let conn = db.conn.lock().unwrap_or_else(|error| error.into_inner());
            conn.execute(
                "INSERT INTO container
                   (container_key, source_parent_key, kind, page_count, scan_state, generation,
                    mtime, file_size)
                 VALUES ('c:/target/book.zip', 'c:/target', ?1, 9000, ?2, 1, 1, 1)",
                params![ContainerKind::Zip as i64, ScanState::Complete as i64],
            )
            .unwrap();
            conn.execute(
                "WITH RECURSIVE ids(value) AS (
                   VALUES(1) UNION ALL SELECT value + 1 FROM ids WHERE value < 9000
                 )
                 INSERT INTO item
                   (revision, item_key, kind, container_key, source_parent_key, page_index,
                    mtime, file_size, hash_version, pdq256, quality, width, height, format)
                 SELECT 1, printf('c:/target/book.zip\\u001f%05d.jpg', value), ?1,
                        'c:/target/book.zip', NULL, value - 1, 1, 1, ?2,
                        zeroblob(32), 1, 1, 1, 1 FROM ids",
                params![ItemKind::ZipPage as i64, current_hash_version()],
            )
            .unwrap();
        }
        let polls = AtomicUsize::new(0);
        let inventory = db
            .load_delta_scoped_inventory(
                DeltaScopePlan {
                    subtrees: vec!["c:/target".to_owned()],
                    ..DeltaScopePlan::default()
                },
                current_hash_version(),
                || polls.fetch_add(1, Ordering::AcqRel) < 7,
            )
            .unwrap();
        assert!(inventory.is_none());
        assert_eq!(polls.load(Ordering::Acquire), 8);
        let conn = db.conn.lock().unwrap_or_else(|error| error.into_inner());
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM item", [], |row| row.get::<_, i64>(0))
                .unwrap(),
            9000
        );
    }

    #[test]
    fn incremental_reconcile_cancelled_full_rolls_back_prune_and_completion_summary() {
        let db = SimilarDb::open_in_memory().unwrap();
        for (key, marker) in [("c:/library/keep.jpg", 1), ("c:/library/stale.jpg", 2)] {
            db.upsert_loose_item(&item(key, None, None, marker))
                .unwrap();
        }
        let previous_summary = db
            .record_completed_index(current_hash_version(), 100, CompletedIndexStats::default())
            .unwrap();
        let before = db.load_search_rows(current_hash_version()).unwrap();
        let publish_checks = std::sync::atomic::AtomicUsize::new(0);

        assert!(
            db.finalize_full_reconcile_if(
                &HashSet::from(["c:/library/keep.jpg".to_owned()]),
                &HashSet::new(),
                current_hash_version(),
                200,
                CompletedIndexStats {
                    io_failures: 1,
                    ..CompletedIndexStats::default()
                },
                || publish_checks.fetch_add(1, Ordering::AcqRel) == 0,
            )
            .is_err()
        );
        assert_eq!(db.load_search_rows(current_hash_version()).unwrap(), before);
        assert_eq!(
            db.load_index_summary(current_hash_version()).unwrap(),
            Some(previous_summary)
        );
    }

    fn full_inventory(db: &SimilarDb) -> FullReconcileInventory {
        db.load_full_reconcile_inventory(current_hash_version(), || true)
            .unwrap()
            .unwrap()
    }

    fn finalize_inventory(db: &SimilarDb, inventory: FullReconcileInventory) -> usize {
        db.finalize_full_reconcile_inventory_if(
            inventory,
            current_hash_version(),
            456,
            CompletedIndexStats::default(),
            || true,
        )
        .unwrap()
        .0
    }

    #[derive(Debug, PartialEq, Eq)]
    struct RawFullItemState {
        item_id: i64,
        item_key: String,
        revision: i64,
        kind: i64,
        container_key: Option<String>,
        page_index: Option<i64>,
        mtime: i64,
        file_size: i64,
        hash_version: i64,
        pdq256: Vec<u8>,
    }

    #[derive(Debug, PartialEq, Eq)]
    struct RawFullContainerState {
        container_key: String,
        kind: i64,
        page_count: Option<i64>,
        scan_state: i64,
        generation: i64,
        mtime: i64,
        file_size: i64,
    }

    fn raw_full_state(db: &SimilarDb) -> (Vec<RawFullItemState>, Vec<RawFullContainerState>) {
        let conn = db.conn.lock().unwrap_or_else(|error| error.into_inner());
        let items = {
            let mut statement = conn
                .prepare(
                    "SELECT item_id, item_key, revision, kind, container_key, page_index,
                            mtime, file_size, hash_version, pdq256
                     FROM item ORDER BY item_id",
                )
                .unwrap();
            statement
                .query_map([], |row| {
                    Ok(RawFullItemState {
                        item_id: row.get(0)?,
                        item_key: row.get(1)?,
                        revision: row.get(2)?,
                        kind: row.get(3)?,
                        container_key: row.get(4)?,
                        page_index: row.get(5)?,
                        mtime: row.get(6)?,
                        file_size: row.get(7)?,
                        hash_version: row.get(8)?,
                        pdq256: row.get(9)?,
                    })
                })
                .unwrap()
                .collect::<rusqlite::Result<Vec<_>>>()
                .unwrap()
        };
        let containers = {
            let mut statement = conn
                .prepare(
                    "SELECT container_key, kind, page_count, scan_state, generation, mtime,
                            file_size
                     FROM container ORDER BY container_key",
                )
                .unwrap();
            statement
                .query_map([], |row| {
                    Ok(RawFullContainerState {
                        container_key: row.get(0)?,
                        kind: row.get(1)?,
                        page_count: row.get(2)?,
                        scan_state: row.get(3)?,
                        generation: row.get(4)?,
                        mtime: row.get(5)?,
                        file_size: row.get(6)?,
                    })
                })
                .unwrap()
                .collect::<rusqlite::Result<Vec<_>>>()
                .unwrap()
        };
        (items, containers)
    }

    fn normalized_changes(db: &SimilarDb, after: u64) -> Vec<(u64, String)> {
        let mut changes = db
            .load_item_changes_after(after)
            .unwrap()
            .changes
            .into_iter()
            .map(|change| (change.item_id, format!("{:?}", change.op)))
            .collect::<Vec<_>>();
        changes.sort();
        changes
    }

    fn populate_full_inventory_equivalence_matrix(db: &SimilarDb) {
        for (key, marker) in [
            ("keep", 1),
            ("stale-loose", 2),
            ("old-loose", 3),
            ("orphan", 4),
            ("remove-loose", 5),
        ] {
            db.upsert_loose_item(&item(key, None, None, marker))
                .unwrap();
        }
        publish_test_book(
            db,
            "extra-book",
            &[("extra-book/0", 6), ("extra-book/1", 7)],
        );
        let zero_generation = db
            .begin_container_build("zero-book", ContainerKind::Zip, 0, 1, 2)
            .unwrap();
        db.complete_container("zero-book", zero_generation).unwrap();
        publish_test_book(db, "old-hash-book", &[("old-hash-book/0", 8)]);
        publish_test_book(db, "invalid-book", &[("invalid-book/0", 9)]);
        publish_test_book(db, "stale-book", &[("stale-book/0", 10)]);
        publish_test_book(db, "remove-book", &[("remove-book/0", 11)]);

        let conn = db.conn.lock().unwrap_or_else(|error| error.into_inner());
        conn.execute(
            "UPDATE item SET container_key = 'missing-owner' WHERE item_key = 'orphan'",
            [],
        )
        .unwrap();
        conn.execute(
            "UPDATE item SET hash_version = ?1 WHERE item_key IN ('old-loose', 'old-hash-book/0')",
            [current_hash_version() - 1],
        )
        .unwrap();
        conn.execute(
            "UPDATE container SET page_count = 1 WHERE container_key = 'extra-book'",
            [],
        )
        .unwrap();
        conn.execute(
            "UPDATE container SET scan_state = 99, page_count = -1
             WHERE container_key = 'invalid-book'",
            [],
        )
        .unwrap();
    }

    #[test]
    fn full_reconcile_inventory_matches_legacy_freshness_and_prune() {
        fn populate(db: &SimilarDb) {
            db.upsert_loose_item(&item("keep", None, None, 1)).unwrap();
            db.upsert_loose_item(&item("remove", None, None, 2))
                .unwrap();
            publish_test_book(db, "book", &[("book/page", 3)]);
        }

        let legacy = SimilarDb::open_in_memory().unwrap();
        let compact = SimilarDb::open_in_memory().unwrap();
        populate(&legacy);
        populate(&compact);
        let legacy_start = legacy.load_item_changes_after(0).unwrap().latest_seq;
        let compact_start = compact.load_item_changes_after(0).unwrap().latest_seq;

        legacy
            .finalize_full_reconcile_if(
                &HashSet::from(["keep".to_owned()]),
                &HashSet::from(["book".to_owned()]),
                current_hash_version(),
                456,
                CompletedIndexStats::default(),
                || true,
            )
            .unwrap();
        let inventory = full_inventory(&compact);
        assert_eq!(
            inventory.observe_item("keep", None, None, 10, 20),
            FullItemObservation {
                exact_current: true,
                reusable_current_row: true,
            }
        );
        inventory.observe_container("book");
        finalize_inventory(&compact, inventory);

        assert_eq!(
            legacy.load_search_rows(current_hash_version()).unwrap(),
            compact.load_search_rows(current_hash_version()).unwrap()
        );
        assert_eq!(
            legacy.load_complete_containers().unwrap(),
            compact.load_complete_containers().unwrap()
        );
        let normalized_changes = |db: &SimilarDb, after| {
            let mut changes = db
                .load_item_changes_after(after)
                .unwrap()
                .changes
                .into_iter()
                .map(|change| (change.item_id, change.op))
                .collect::<Vec<_>>();
            changes.sort_by_key(|entry| entry.0);
            changes
        };
        assert_eq!(
            normalized_changes(&legacy, legacy_start),
            normalized_changes(&compact, compact_start)
        );
        assert_eq!(
            legacy.load_index_summary(current_hash_version()).unwrap(),
            compact.load_index_summary(current_hash_version()).unwrap()
        );
    }

    #[test]
    fn full_reconcile_inventory_matches_legacy_edge_case_matrix() {
        let legacy = SimilarDb::open_in_memory().unwrap();
        let compact = SimilarDb::open_in_memory().unwrap();
        populate_full_inventory_equivalence_matrix(&legacy);
        populate_full_inventory_equivalence_matrix(&compact);
        let legacy_start = legacy.load_item_changes_after(0).unwrap().latest_seq;
        let compact_start = compact.load_item_changes_after(0).unwrap().latest_seq;
        let inventory = full_inventory(&compact);

        let item_cases = [
            ("keep", None, None, 10, 20),
            ("stale-loose", None, None, 11, 20),
            ("old-loose", None, None, 10, 20),
            ("orphan", Some("missing-owner"), None, 10, 20),
        ];
        let mut seen_items = HashSet::new();
        for (key, owner, page_index, mtime, file_size) in item_cases {
            let legacy_row = legacy.load_item(key, current_hash_version()).unwrap();
            let legacy_reusable = legacy_row
                .as_ref()
                .is_some_and(|row| row.item.mtime == mtime && row.item.file_size == file_size);
            let legacy_exact = legacy_row.as_ref().is_some_and(|row| {
                row.item.mtime == mtime
                    && row.item.file_size == file_size
                    && row.item.container_key.as_deref() == owner
                    && row.item.page_index == page_index
            });
            let observed = inventory.observe_item(key, owner, page_index, mtime, file_size);
            assert_eq!(observed.exact_current, legacy_exact, "item case {key}");
            assert_eq!(
                observed.reusable_current_row, legacy_reusable,
                "item reuse case {key}"
            );
            seen_items.insert(key.to_owned());
        }

        let container_cases = [
            ("extra-book", 1, 2, 1),
            ("zero-book", 1, 2, 0),
            ("old-hash-book", 1, 2, 1),
            ("invalid-book", 1, 2, 1),
            ("stale-book", 2, 2, 1),
        ];
        let mut seen_containers = HashSet::new();
        for (key, mtime, file_size, page_count) in container_cases {
            let legacy_freshness = legacy
                .container_freshness(key, mtime, file_size, page_count, current_hash_version())
                .unwrap();
            inventory.observe_container(key);
            let observed = inventory.container_observation(
                key,
                mtime,
                file_size,
                page_count,
                current_hash_version(),
            );
            assert_eq!(observed.freshness, legacy_freshness, "container case {key}");
            seen_containers.insert(key.to_owned());
        }

        let stats = CompletedIndexStats {
            password_required_pdfs: 1,
            corrupt_containers: 2,
            zero_page_containers: 3,
            decode_failures: 4,
            io_failures: 5,
        };
        let legacy_removed = legacy
            .finalize_full_reconcile_if(
                &seen_items,
                &seen_containers,
                current_hash_version(),
                789,
                stats,
                || true,
            )
            .unwrap()
            .0;
        let compact_removed = compact
            .finalize_full_reconcile_inventory_if(
                inventory,
                current_hash_version(),
                789,
                stats,
                || true,
            )
            .unwrap()
            .0;

        assert_eq!(compact_removed, legacy_removed);
        assert_eq!(raw_full_state(&compact), raw_full_state(&legacy));
        assert_eq!(
            compact.load_index_summary(current_hash_version()).unwrap(),
            legacy.load_index_summary(current_hash_version()).unwrap()
        );
        assert_eq!(
            normalized_changes(&compact, compact_start),
            normalized_changes(&legacy, legacy_start),
            "delete journal must match as a sorted multiset without hiding duplicates"
        );
    }

    #[test]
    fn full_reconcile_inventory_loader_polls_cancellation_during_row_scan() {
        let db = SimilarDb::open_in_memory().unwrap();
        {
            let conn = db.conn.lock().unwrap_or_else(|error| error.into_inner());
            conn.execute(
                "WITH RECURSIVE ids(value) AS (
                   VALUES(1) UNION ALL SELECT value + 1 FROM ids WHERE value <= ?1
                 )
                 INSERT INTO item
                   (revision, item_key, kind, container_key, page_index, mtime, file_size,
                    hash_version, pdq256, quality, width, height, format)
                 SELECT 1, printf('cancel-%05d', value), 0, NULL, NULL, 1, 1,
                        ?2, zeroblob(32), 1, 1, 1, 1
                 FROM ids",
                params![
                    i64::try_from(FULL_INVENTORY_CANCEL_POLL_ROWS).unwrap(),
                    current_hash_version()
                ],
            )
            .unwrap();
        }
        let checks = AtomicUsize::new(0);

        let loaded = db
            .load_full_reconcile_inventory(current_hash_version(), || {
                checks.fetch_add(1, Ordering::AcqRel) < 2
            })
            .unwrap();

        assert!(
            loaded.is_none(),
            "periodic cancellation must abort the loader"
        );
        assert!(
            checks.load(Ordering::Acquire) >= 3,
            "the cancellation predicate must be checked again after the first row block"
        );
        db.upsert_loose_item(&item("after-cancel", None, None, 99))
            .unwrap();
    }

    #[test]
    fn full_reconcile_inventory_new_member_after_snapshot_survives() {
        let db = SimilarDb::open_in_memory().unwrap();
        publish_test_book(&db, "book", &[("book/old", 1)]);
        let inventory = full_inventory(&db);
        let before = db.load_item_changes_after(0).unwrap().latest_seq;
        {
            let conn = db.conn.lock().unwrap_or_else(|error| error.into_inner());
            publish_item(&conn, None, &item("book/new", Some("book"), Some(1), 2)).unwrap();
        }

        assert_eq!(finalize_inventory(&db, inventory), 1);
        let rows = db.load_search_rows(current_hash_version()).unwrap();
        assert_eq!(
            rows.iter()
                .map(|row| row.item.item_key.as_str())
                .collect::<Vec<_>>(),
            vec!["book/new"]
        );
        assert_eq!(db.load_complete_containers().unwrap().len(), 1);
        let deletes = db
            .load_item_changes_after(before)
            .unwrap()
            .changes
            .into_iter()
            .filter(|change| change.op == ItemChangeOp::Delete)
            .collect::<Vec<_>>();
        assert_eq!(deletes.len(), 1, "one initial row is journaled once");
    }

    #[test]
    fn full_reconcile_inventory_owner_changes_survive() {
        let db = SimilarDb::open_in_memory().unwrap();
        publish_test_book(&db, "old-book", &[("old-book/page", 1)]);
        db.upsert_loose_item(&item("orphan", None, None, 2))
            .unwrap();
        db.upsert_loose_item(&item("move-to-book", None, None, 3))
            .unwrap();
        db.conn
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .execute(
                "UPDATE item SET container_key = 'missing-owner' WHERE item_key = 'orphan'",
                [],
            )
            .unwrap();
        let inventory = full_inventory(&db);

        {
            let conn = db.conn.lock().unwrap_or_else(|error| error.into_inner());
            conn.execute(
                "UPDATE item SET container_key = NULL, page_index = NULL
                 WHERE item_key IN ('old-book/page', 'orphan')",
                [],
            )
            .unwrap();
        }
        let generation = db
            .begin_container_build("new-book", ContainerKind::ImageFolder, 1, 1, 2)
            .unwrap();
        db.stage_item(
            generation,
            &item("move-to-book", Some("new-book"), Some(0), 4),
        )
        .unwrap();
        db.complete_container("new-book", generation).unwrap();

        assert_eq!(finalize_inventory(&db, inventory), 1);
        let mut rows = db
            .load_search_rows(current_hash_version())
            .unwrap()
            .into_iter()
            .map(|row| (row.item.item_key, row.item.container_key))
            .collect::<Vec<_>>();
        rows.sort();
        assert_eq!(
            rows,
            vec![
                ("move-to-book".to_owned(), Some("new-book".to_owned())),
                ("old-book/page".to_owned(), None),
                ("orphan".to_owned(), None),
            ]
        );
        assert_eq!(
            db.load_complete_containers()
                .unwrap()
                .into_iter()
                .map(|container| container.container_key)
                .collect::<Vec<_>>(),
            vec!["new-book"]
        );
    }

    #[test]
    fn full_reconcile_inventory_same_container_key_rebuild_survives() {
        let db = SimilarDb::open_in_memory().unwrap();
        publish_test_book(&db, "book", &[("book/page", 1)]);
        let inventory = full_inventory(&db);
        let before = db.load_item_changes_after(0).unwrap().latest_seq;
        publish_test_book(&db, "book", &[("book/page", 9)]);

        assert_eq!(finalize_inventory(&db, inventory), 0);
        let row = db
            .load_item("book/page", current_hash_version())
            .unwrap()
            .unwrap();
        assert_eq!(row.item.pdq256, [9; 32]);
        assert_eq!(db.load_complete_containers().unwrap()[0].generation, 2);
        assert!(
            db.load_item_changes_after(before)
                .unwrap()
                .changes
                .iter()
                .all(|change| change.op != ItemChangeOp::Delete)
        );
    }

    #[test]
    fn full_reconcile_inventory_failed_container_keeps_previous_complete_pages() {
        let db = SimilarDb::open_in_memory().unwrap();
        publish_test_book(&db, "book", &[("book/page", 1)]);
        let inventory = full_inventory(&db);
        inventory.observe_container("book");

        assert_eq!(finalize_inventory(&db, inventory), 0);
        assert!(
            db.load_item("book/page", current_hash_version())
                .unwrap()
                .is_some()
        );
    }

    #[test]
    fn full_reconcile_inventory_invalid_container_state_is_stale_not_fatal() {
        let db = SimilarDb::open_in_memory().unwrap();
        publish_test_book(&db, "book", &[("book/page", 1)]);
        db.conn
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .execute(
                "UPDATE container SET scan_state = 99, page_count = -1 WHERE container_key='book'",
                [],
            )
            .unwrap();

        let inventory = full_inventory(&db);
        assert_eq!(
            inventory
                .container_observation("book", 1, 2, 1, current_hash_version())
                .freshness,
            Freshness::Stale
        );
        assert!(
            !inventory
                .observe_item("book/page", Some("book"), Some(0), 10, 20)
                .reusable_current_row
        );
    }

    #[test]
    fn full_reconcile_inventory_cancelled_finalize_rolls_back_everything() {
        let db = SimilarDb::open_in_memory().unwrap();
        db.upsert_loose_item(&item("remove", None, None, 1))
            .unwrap();
        let previous = db
            .record_completed_index(current_hash_version(), 100, CompletedIndexStats::default())
            .unwrap();
        let before = db.load_item_changes_after(0).unwrap();
        let inventory = full_inventory(&db);
        let checks = AtomicUsize::new(0);

        assert!(
            db.finalize_full_reconcile_inventory_if(
                inventory,
                current_hash_version(),
                200,
                CompletedIndexStats::default(),
                || checks.fetch_add(1, Ordering::AcqRel) == 0,
            )
            .is_err()
        );
        assert!(
            db.load_item("remove", current_hash_version())
                .unwrap()
                .is_some()
        );
        assert_eq!(db.load_item_changes_after(0).unwrap(), before);
        assert_eq!(
            db.load_index_summary(current_hash_version()).unwrap(),
            Some(previous)
        );
    }

    #[test]
    fn full_reconcile_inventory_uses_word_packed_seen_bits() {
        let db = SimilarDb::open_in_memory().unwrap();
        for index in 0..65 {
            db.upsert_loose_item(&item(&format!("item-{index}"), None, None, index as u8))
                .unwrap();
        }
        let inventory = full_inventory(&db);
        let accounting = inventory.accounting();
        assert_eq!(accounting.item_count, 65);
        assert!(accounting.item_capacity >= accounting.item_count);
        assert!(accounting.item_map_capacity >= accounting.item_count);
        assert_eq!(accounting.container_count, 0);
        assert!(accounting.container_capacity >= accounting.container_count);
        assert_eq!(accounting.container_map_capacity, 0);
        assert!(accounting.key_bytes >= "item-0".len() * 65);
        assert!(accounting.item_record_capacity_bytes >= 65 * size_of::<FullInventoryItem>());
        assert_eq!(accounting.container_record_capacity_bytes, 0);
        assert_eq!(accounting.item_bitset_bytes, 16);
        assert_eq!(accounting.container_bitset_bytes, 0);
        assert!(accounting.item_map_growths >= 1);
        assert!(accounting.item_record_growths >= 1);
        assert_eq!(accounting.container_map_growths, 0);
        assert_eq!(accounting.container_record_growths, 0);
    }
}

#[cfg(test)]
#[path = "similar_db/full_inventory_benchmark_tests.rs"]
mod full_inventory_benchmark_tests;
