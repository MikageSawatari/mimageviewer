//! Worker-local same-transaction engine for book similarity queries.
//!
//! The engine owns one read-only SQLite connection and one MIH generation. It resolves every
//! search hit back through the same SQLite transaction before using that hit for common-page
//! detection, candidate discovery, classification, or navigation output.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt;
use std::path::Path;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use crate::dupe::book::{
    AnalyzeError, EdgeVisitCompletion, EdgeVisitControl, PreparedBookSide, ReenumeratedBookEdges,
    ReenumeratedPairError, VerifiedBookPage, classify_pair_reenumerated,
};
use crate::similar_book_mih::{
    BookMihRuntime, MihBuildError, MihHit, MihQueryError, MihVisitCompletion, MihVisitControl,
};
use crate::similar_db::{
    BookHitEligibility, BookHitResolution, BookPageOrderResolver, BookReadError, BookReadMetadata,
    BookReadSnapshot, SearchRow, SimilarBookReader, current_hash_version, key_is_under_any,
};
use crate::similar_index::{
    BOOK_COVERAGE, BOOK_MAX_BOOKS_PER_PAGE, BOOK_MIN_MATCHED_PAGES, BOOK_MIN_QUALITY, BOOK_RADIUS,
    BookOrigin, BookOriginPage, BookPageBaseline, BookQuery, BookRelationHit, BookRelations,
    book_origin_page_slots, build_page_strip_overrides, compare_book_pages,
};
use crate::similar_search_array::{SearchSnapshot, snapshot_for_book_read};

const BOOK_ORIGIN: u32 = 1;
const BOOK_CANDIDATE: u32 = 2;
const HIT_RESOLVE_CHUNK: usize = 64;
const DIRECT_EDGE_CANCEL_INTERVAL: usize = 1 << 10;

pub(crate) struct SimilarBookQueryEngine {
    reader: SimilarBookReader,
    mih: BookMihRuntime,
}

#[derive(Debug)]
pub(crate) struct EngineObservation {
    pub(crate) metadata: BookReadMetadata,
    pub(crate) outcome: Result<BookQuery, EngineError>,
}

#[derive(Debug)]
pub(crate) enum EngineError {
    Cancelled,
    Database(rusqlite::Error),
    MihBuild(MihBuildError),
    MihQuery(MihQueryFailure),
    Analyze(AnalyzeError),
    Pair(ReenumeratedPairError<DirectEdgeError>),
    Invariant(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MihQueryFailure {
    RadiusOutOfRange(u32),
    CorruptIndex,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DirectEdgeError {
    Cancelled,
    InvalidOriginSlot(usize),
}

impl fmt::Display for EngineError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Cancelled => formatter.write_str("book query engine was cancelled"),
            Self::Database(error) => write!(formatter, "book query database read failed: {error}"),
            Self::MihBuild(error) => write!(formatter, "book query {error}"),
            Self::MihQuery(error) => write!(formatter, "book query MIH failed: {error:?}"),
            Self::Analyze(error) => write!(formatter, "book query classification failed: {error}"),
            Self::Pair(error) => {
                write!(
                    formatter,
                    "book query streamed classification failed: {error:?}"
                )
            }
            Self::Invariant(error) => write!(formatter, "book query invariant failed: {error}"),
        }
    }
}

impl std::error::Error for EngineError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Database(error) => Some(error),
            Self::Analyze(error) => Some(error),
            Self::Cancelled
            | Self::MihBuild(_)
            | Self::MihQuery(_)
            | Self::Pair(_)
            | Self::Invariant(_) => None,
        }
    }
}

enum BodyError {
    Database(rusqlite::Error),
    Engine(EngineError),
}

impl From<rusqlite::Error> for BodyError {
    fn from(value: rusqlite::Error) -> Self {
        Self::Database(value)
    }
}

impl From<EngineError> for BodyError {
    fn from(value: EngineError) -> Self {
        Self::Engine(value)
    }
}

type BodyResult<T> = Result<T, BodyError>;

#[derive(Clone, Debug)]
enum SignatureNeighborhood {
    Common,
    Rare(Box<[(String, u8)]>),
}

#[derive(Debug, Default)]
struct CandidateDiscovery {
    matched_edges: u32,
    origin_slots: Vec<usize>,
}

impl SimilarBookQueryEngine {
    pub(crate) fn open_at(path: &Path) -> Result<Option<Self>, BookReadError> {
        Ok(SimilarBookReader::open_at(path)?.map(|reader| Self {
            reader,
            mih: BookMihRuntime::new(),
        }))
    }

    pub(crate) fn query(
        &mut self,
        container_key: &str,
        immutable_roots: &[String],
        shared_snapshot_candidates: &[Arc<SearchSnapshot>],
        cancel: Arc<AtomicBool>,
    ) -> Result<EngineObservation, BookReadError> {
        self.query_after_metadata(
            container_key,
            immutable_roots,
            shared_snapshot_candidates,
            cancel,
            |_| {},
        )
    }

    #[cfg(test)]
    pub(crate) fn set_test_phase_probe(
        &mut self,
        probe: Arc<crate::similar_book_query_test_probe::BookQueryTestPhaseProbe>,
    ) {
        self.reader.set_test_phase_probe(Arc::clone(&probe));
        self.mih.set_test_phase_probe(probe);
    }

    fn query_after_metadata<F>(
        &mut self,
        container_key: &str,
        immutable_roots: &[String],
        shared_snapshot_candidates: &[Arc<SearchSnapshot>],
        cancel: Arc<AtomicBool>,
        after_metadata: F,
    ) -> Result<EngineObservation, BookReadError>
    where
        F: FnOnce(BookReadMetadata),
    {
        let reader = &mut self.reader;
        let mih = &mut self.mih;
        #[cfg(test)]
        let phase_probe = mih.test_phase_probe();
        reader.with_snapshot(Arc::clone(&cancel), |read| {
            let metadata = read.metadata();
            after_metadata(metadata);
            #[cfg(test)]
            if let Some(probe) = phase_probe.as_ref() {
                probe.arm_after_metadata();
            }
            if mih
                .preferred_base()
                .is_some_and(|base| base.store_id != metadata.store_id)
            {
                mih.clear();
            }

            let outcome = match query_snapshot(
                read,
                mih,
                container_key,
                immutable_roots,
                shared_snapshot_candidates,
                cancel.as_ref(),
            ) {
                Ok(query) => Ok(query),
                Err(BodyError::Database(error)) => {
                    Err(classify_body_database_error(error, cancel.as_ref()))
                }
                Err(BodyError::Engine(error)) => Err(error),
            };
            // The closure itself succeeds even when the query body fails. The read-only TX can
            // then close normally while C still receives the store metadata observed by its first
            // SELECT and the typed body failure as one value.
            Ok(EngineObservation { metadata, outcome })
        })
    }

    pub(crate) fn clear(&mut self) {
        self.mih.clear();
    }

    #[cfg(feature = "dev-tools")]
    pub(crate) fn benchmark_cached_snapshot(&self) -> Option<Arc<SearchSnapshot>> {
        self.mih.cached_snapshot_candidate()
    }
}

fn query_snapshot(
    read: &BookReadSnapshot<'_>,
    mih: &mut BookMihRuntime,
    container_key: &str,
    immutable_roots: &[String],
    shared_snapshot_candidates: &[Arc<SearchSnapshot>],
    cancel: &AtomicBool,
) -> BodyResult<BookQuery> {
    check_cancelled(cancel)?;
    if !key_is_under_any(container_key, immutable_roots) {
        return Ok(BookQuery::NotIndexed);
    }

    let origin_pages = read
        .page_order_resolver(compare_book_pages)
        .load_book_pages(container_key, current_hash_version())?;
    if origin_pages.is_empty() {
        return Ok(BookQuery::NotBook);
    }
    if origin_pages
        .iter()
        .all(|row| row.item.quality < BOOK_MIN_QUALITY)
    {
        return Ok(BookQuery::Featureless);
    }

    let mut snapshot_candidates = Vec::with_capacity(shared_snapshot_candidates.len() + 1);
    snapshot_candidates.extend(shared_snapshot_candidates.iter().cloned());
    if let Some(candidate) = mih.cached_snapshot_candidate() {
        snapshot_candidates.push(candidate);
    }
    let snapshot = snapshot_for_book_read(read, &snapshot_candidates, mih.preferred_base())?;
    drop(snapshot_candidates);
    mih.prepare(Arc::clone(&snapshot), cancel)
        .map_err(map_mih_build_error)?;
    // Raw MIH identities are filtered before consulting this resolver. Its completed stale-ZIP
    // maps are shared across signatures, but the resolver itself has a fixed ordinal budget.
    let mut hit_page_order = read.page_order_resolver(compare_book_pages);

    let params = crate::dupe::book::Params {
        radius: BOOK_RADIUS,
        max_books_per_page: BOOK_MAX_BOOKS_PER_PAGE,
        min_quality: BOOK_MIN_QUALITY,
        coverage_threshold: BOOK_COVERAGE,
        min_matched_pages: BOOK_MIN_MATCHED_PAGES,
    };

    let mut origin_indexed_rows = origin_pages
        .iter()
        .filter(|row| row.item.page_index.is_some())
        .collect::<Vec<_>>();
    origin_indexed_rows.sort_by_key(|row| row.item.page_index);

    let mut origin_memo = HashMap::<[u8; 32], SignatureNeighborhood>::new();
    let mut origin_common_items = HashSet::new();
    let mut discoveries = BTreeMap::<String, CandidateDiscovery>::new();
    for (origin_slot, row) in origin_indexed_rows.iter().enumerate() {
        check_cancelled(cancel)?;
        if row.item.quality < BOOK_MIN_QUALITY {
            continue;
        }
        let signature = row.item.pdq256;
        if !origin_memo.contains_key(&signature) {
            let neighborhood = resolve_signature_neighborhood(
                read,
                mih,
                &mut hit_page_order,
                &signature,
                immutable_roots,
                cancel,
            )?;
            origin_memo.insert(signature, neighborhood);
        }
        match origin_memo
            .get(&signature)
            .expect("origin signature memo was inserted")
        {
            SignatureNeighborhood::Common => {
                origin_common_items.insert(row.item_id);
            }
            SignatureNeighborhood::Rare(per_book) => {
                for (candidate_key, count) in per_book {
                    if candidate_key == container_key {
                        continue;
                    }
                    let discovery = discoveries.entry(candidate_key.clone()).or_default();
                    discovery.matched_edges = discovery
                        .matched_edges
                        .saturating_add(u32::from(*count))
                        .min(BOOK_MIN_MATCHED_PAGES);
                    discovery.origin_slots.push(origin_slot);
                }
            }
        }
    }
    discoveries.retain(|_, discovery| discovery.matched_edges >= BOOK_MIN_MATCHED_PAGES);

    let prepared_origin = PreparedBookSide::new(
        BOOK_ORIGIN,
        verified_pages(&origin_indexed_rows, &origin_common_items),
        params,
    )
    .map_err(EngineError::Analyze)?;
    let origin = Arc::new(build_origin(&origin_pages, &origin_common_items));
    let origin_page_slots = book_origin_page_slots(&origin_pages);

    let mut hits = Vec::with_capacity(discoveries.len());
    for (candidate_key, discovery) in discoveries {
        check_cancelled(cancel)?;
        let candidate_pages =
            hit_page_order.load_book_pages(&candidate_key, current_hash_version())?;
        if candidate_pages.is_empty() {
            return Err(EngineError::Invariant(format!(
                "discovered candidate {candidate_key} had no eligible pages in the same transaction"
            ))
            .into());
        }
        let mut candidate_indexed_rows = candidate_pages
            .iter()
            .filter(|row| row.item.page_index.is_some())
            .collect::<Vec<_>>();
        candidate_indexed_rows.sort_by_key(|row| row.item.page_index);

        // Candidate-only signatures live for this candidate only. Origin signatures reuse the
        // request-wide memo, bounding retained memo state to O(origin unique + largest candidate
        // unique) rather than the sum across every candidate.
        let mut candidate_local_memo = HashMap::<[u8; 32], SignatureNeighborhood>::new();
        let mut candidate_common_items = HashSet::new();
        for row in &candidate_indexed_rows {
            check_cancelled(cancel)?;
            if row.item.quality < BOOK_MIN_QUALITY {
                continue;
            }
            let signature = row.item.pdq256;
            let common = if let Some(neighborhood) = origin_memo.get(&signature) {
                matches!(neighborhood, SignatureNeighborhood::Common)
            } else {
                if !candidate_local_memo.contains_key(&signature) {
                    let neighborhood = resolve_signature_neighborhood(
                        read,
                        mih,
                        &mut hit_page_order,
                        &signature,
                        immutable_roots,
                        cancel,
                    )?;
                    candidate_local_memo.insert(signature, neighborhood);
                }
                matches!(
                    candidate_local_memo
                        .get(&signature)
                        .expect("candidate signature memo was inserted"),
                    SignatureNeighborhood::Common
                )
            };
            if common {
                candidate_common_items.insert(row.item_id);
            }
        }

        let prepared_candidate = PreparedBookSide::new(
            BOOK_CANDIDATE,
            verified_pages(&candidate_indexed_rows, &candidate_common_items),
            params,
        )
        .map_err(EngineError::Analyze)?;
        let mut edge_source = DirectBookEdges {
            origin: prepared_origin.pages(),
            candidate: prepared_candidate.pages(),
            radius: BOOK_RADIUS,
            cancel,
            #[cfg(test)]
            phase_probe: mih.test_phase_probe(),
        };
        let pair = classify_pair_reenumerated(
            &prepared_origin,
            &prepared_candidate,
            &discovery.origin_slots,
            &mut edge_source,
            cancel,
        )
        .map_err(map_pair_error)?;
        let overrides =
            build_page_strip_overrides(&origin_pages, &origin_page_slots, &candidate_pages, &pair);
        let other_page_count = u32::try_from(candidate_pages.len()).map_err(|_| {
            EngineError::Invariant(format!("candidate {candidate_key} page count exceeds u32"))
        })?;
        hits.push(
            BookRelationHit::new(
                candidate_key,
                other_page_count,
                pair,
                overrides,
                origin.pages.len(),
            )
            .map_err(EngineError::Invariant)?,
        );
    }

    Ok(BookQuery::Ready(BookRelations { origin, hits }))
}

fn verified_pages(rows: &[&SearchRow], common_items: &HashSet<u64>) -> Vec<VerifiedBookPage> {
    rows.iter()
        .filter_map(|row| {
            Some(VerifiedBookPage {
                index: row.item.page_index?,
                quality: row.item.quality,
                common: common_items.contains(&row.item_id),
                signature: row.item.pdq256,
            })
        })
        .collect()
}

fn build_origin(origin_pages: &[SearchRow], common_items: &HashSet<u64>) -> BookOrigin {
    BookOrigin {
        pages: origin_pages
            .iter()
            .map(|row| BookOriginPage {
                item_key: row.item.item_key.clone(),
                baseline: if row.item.quality < BOOK_MIN_QUALITY
                    || row.item.page_index.is_none()
                    || common_items.contains(&row.item_id)
                {
                    BookPageBaseline::Excluded
                } else {
                    BookPageBaseline::Unmatched
                },
            })
            .collect::<Vec<_>>()
            .into_boxed_slice(),
    }
}

fn resolve_signature_neighborhood<Compare>(
    read: &BookReadSnapshot<'_>,
    mih: &mut BookMihRuntime,
    page_order: &mut BookPageOrderResolver<'_, '_, Compare>,
    signature: &[u8; 32],
    immutable_roots: &[String],
    cancel: &AtomicBool,
) -> BodyResult<SignatureNeighborhood>
where
    Compare: Fn(&str, &str) -> std::cmp::Ordering,
{
    let mut per_book = BTreeMap::<String, u8>::new();
    let mut chunk = Vec::with_capacity(HIT_RESOLVE_CHUNK);
    let mut common = false;
    let completion = mih.visit_within(signature, BOOK_RADIUS, cancel, |hit| {
        chunk.push(hit);
        if chunk.len() < HIT_RESOLVE_CHUNK {
            return Ok(MihVisitControl::Continue);
        }
        common = resolve_hit_chunk(
            read,
            page_order,
            signature,
            &mut chunk,
            &mut per_book,
            immutable_roots,
            cancel,
        )?;
        Ok(if common {
            MihVisitControl::Stop
        } else {
            MihVisitControl::Continue
        })
    });

    let completion = match completion {
        Ok(completion) => completion,
        Err(MihQueryError::Cancelled) => return Err(EngineError::Cancelled.into()),
        Err(MihQueryError::RadiusOutOfRange(radius)) => {
            return Err(EngineError::MihQuery(MihQueryFailure::RadiusOutOfRange(radius)).into());
        }
        Err(MihQueryError::CorruptIndex) => {
            return Err(EngineError::MihQuery(MihQueryFailure::CorruptIndex).into());
        }
        Err(MihQueryError::Visitor(error)) => return Err(error),
    };
    match completion {
        MihVisitCompletion::Exhausted => {
            if !chunk.is_empty() {
                common = resolve_hit_chunk(
                    read,
                    page_order,
                    signature,
                    &mut chunk,
                    &mut per_book,
                    immutable_roots,
                    cancel,
                )?;
            }
        }
        MihVisitCompletion::Stopped if !common => {
            return Err(EngineError::Invariant(
                "MIH neighborhood stopped without a ninth valid book".to_owned(),
            )
            .into());
        }
        MihVisitCompletion::Stopped => {}
    }
    if common {
        Ok(SignatureNeighborhood::Common)
    } else {
        Ok(SignatureNeighborhood::Rare(
            per_book.into_iter().collect::<Vec<_>>().into_boxed_slice(),
        ))
    }
}

fn resolve_hit_chunk<Compare>(
    read: &BookReadSnapshot<'_>,
    page_order: &mut BookPageOrderResolver<'_, '_, Compare>,
    query_signature: &[u8; 32],
    chunk: &mut Vec<MihHit>,
    per_book: &mut BTreeMap<String, u8>,
    immutable_roots: &[String],
    cancel: &AtomicBool,
) -> BodyResult<bool>
where
    Compare: Fn(&str, &str) -> std::cmp::Ordering,
{
    check_cancelled(cancel)?;
    let item_ids = chunk.iter().map(|hit| hit.item_id).collect::<Vec<_>>();
    let resolved = read.resolve_book_hits_raw(&item_ids, current_hash_version())?;
    if resolved.len() != chunk.len() {
        return Err(EngineError::Invariant(
            "raw hit resolver did not preserve MIH input cardinality".to_owned(),
        )
        .into());
    }
    for (hit, resolution) in chunk.iter().copied().zip(resolved) {
        check_cancelled(cancel)?;
        let BookHitResolution::Present {
            mut row,
            eligibility,
        } = resolution
        else {
            return Err(EngineError::Invariant(format!(
                "MIH item {} was absent from the same SQLite snapshot",
                hit.item_id
            ))
            .into());
        };
        if row.item_id != hit.item_id
            || row.revision != hit.revision
            || row.item.pdq256 != hit.signature
        {
            return Err(EngineError::Invariant(format!(
                "MIH identity for item {} differed from the same SQLite snapshot",
                hit.item_id
            ))
            .into());
        }
        let distance = hamming256(query_signature, &row.item.pdq256);
        if distance != hit.distance || distance > BOOK_RADIUS {
            return Err(EngineError::Invariant(format!(
                "MIH distance for item {} was not reproducible",
                hit.item_id
            ))
            .into());
        }
        if eligibility != BookHitEligibility::Eligible || row.item.quality < BOOK_MIN_QUALITY {
            continue;
        }
        let Some(container_key) = row.item.container_key.clone() else {
            continue;
        };
        if !key_is_under_any(&container_key, immutable_roots) {
            continue;
        }
        page_order.apply_eligible_book_hit_order(&mut row)?;
        if row.item.page_index.is_none() {
            continue;
        }
        let count = per_book.entry(container_key).or_default();
        *count = count.saturating_add(1).min(BOOK_MIN_MATCHED_PAGES as u8);
        if per_book.len() > BOOK_MAX_BOOKS_PER_PAGE as usize {
            chunk.clear();
            return Ok(true);
        }
    }
    chunk.clear();
    Ok(false)
}

struct DirectBookEdges<'a> {
    origin: &'a [VerifiedBookPage],
    candidate: &'a [VerifiedBookPage],
    radius: u32,
    cancel: &'a AtomicBool,
    #[cfg(test)]
    phase_probe: Option<Arc<crate::similar_book_query_test_probe::BookQueryTestPhaseProbe>>,
}

impl ReenumeratedBookEdges for DirectBookEdges<'_> {
    type Error = DirectEdgeError;

    fn visit_a(
        &mut self,
        a_slot: usize,
        visitor: &mut dyn FnMut(usize, u32) -> EdgeVisitControl,
    ) -> Result<EdgeVisitCompletion, Self::Error> {
        let Some(origin) = self.origin.get(a_slot) else {
            return Err(DirectEdgeError::InvalidOriginSlot(a_slot));
        };
        for (candidate_slot, candidate) in self.candidate.iter().enumerate() {
            #[cfg(test)]
            if candidate_slot > 0 && candidate_slot % DIRECT_EDGE_CANCEL_INTERVAL == 0 {
                if let Some(probe) = self.phase_probe.as_ref() {
                    probe.checkpoint(
                        crate::similar_book_query_test_probe::BookQueryTestPhase::DirectCandidateSlot,
                    );
                }
            }
            if candidate_slot % DIRECT_EDGE_CANCEL_INTERVAL == 0
                && self.cancel.load(Ordering::Acquire)
            {
                return Err(DirectEdgeError::Cancelled);
            }
            let distance = hamming256(&origin.signature, &candidate.signature);
            if distance <= self.radius
                && visitor(candidate_slot, distance) == EdgeVisitControl::Stop
            {
                return Ok(EdgeVisitCompletion::Stopped);
            }
        }
        if self.cancel.load(Ordering::Acquire) {
            Err(DirectEdgeError::Cancelled)
        } else {
            Ok(EdgeVisitCompletion::Exhausted)
        }
    }
}

fn hamming256(left: &[u8; 32], right: &[u8; 32]) -> u32 {
    left.iter()
        .zip(right)
        .map(|(left, right)| (left ^ right).count_ones())
        .sum()
}

fn check_cancelled(cancel: &AtomicBool) -> Result<(), EngineError> {
    if cancel.load(Ordering::Acquire) {
        Err(EngineError::Cancelled)
    } else {
        Ok(())
    }
}

fn map_mih_build_error(error: MihBuildError) -> EngineError {
    match error {
        MihBuildError::Cancelled => EngineError::Cancelled,
        error => EngineError::MihBuild(error),
    }
}

fn map_pair_error(error: ReenumeratedPairError<DirectEdgeError>) -> EngineError {
    match error {
        ReenumeratedPairError::Cancelled
        | ReenumeratedPairError::Source(DirectEdgeError::Cancelled) => EngineError::Cancelled,
        error => EngineError::Pair(error),
    }
}

fn classify_body_database_error(error: rusqlite::Error, cancel: &AtomicBool) -> EngineError {
    if matches!(
        &error,
        rusqlite::Error::SqliteFailure(code, _)
            if code.code == rusqlite::ErrorCode::OperationInterrupted
    ) && cancel.load(Ordering::Acquire)
    {
        EngineError::Cancelled
    } else {
        EngineError::Database(error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dupe::{self, Sig};
    use crate::similar_db::{ContainerKind, ItemKind, SimilarDb, StoredItem};
    use crate::similar_index::{BookPageMatch, BookPageMatchState, SimilarItemTarget};
    use crate::similar_search_array::{BaseArray, SearchRecord};

    const ROOT: &str = "c:/library";

    fn signature(byte: u8) -> [u8; 32] {
        [byte; 32]
    }

    fn item(
        container_key: &str,
        page_index: u32,
        signature: [u8; 32],
        hash_version: i64,
    ) -> StoredItem {
        StoredItem {
            item_key: format!("{container_key}/{page_index:04}.jpg"),
            kind: ItemKind::Image,
            container_key: Some(container_key.to_owned()),
            page_index: Some(page_index),
            mtime: 100 + i64::from(page_index),
            file_size: 1_000 + i64::from(page_index),
            hash_version,
            pdq256: signature,
            quality: 50,
            width: 800,
            height: 600,
            format: 1,
        }
    }

    fn publish_book(
        db: &SimilarDb,
        container_key: &str,
        signatures: &[[u8; 32]],
        hash_version: i64,
    ) {
        let generation = db
            .begin_container_build(
                container_key,
                ContainerKind::ImageFolder,
                signatures.len() as u32,
                1,
                2,
            )
            .unwrap();
        for (page_index, &signature) in signatures.iter().enumerate() {
            db.stage_item(
                generation,
                &item(container_key, page_index as u32, signature, hash_version),
            )
            .unwrap();
        }
        db.complete_container(container_key, generation).unwrap();
    }

    fn publish_book_with_qualities(db: &SimilarDb, container_key: &str, pages: &[([u8; 32], u8)]) {
        let generation = db
            .begin_container_build(
                container_key,
                ContainerKind::ImageFolder,
                pages.len() as u32,
                1,
                2,
            )
            .unwrap();
        for (page_index, &(signature, quality)) in pages.iter().enumerate() {
            let mut page = item(
                container_key,
                page_index as u32,
                signature,
                current_hash_version(),
            );
            page.quality = quality;
            db.stage_item(generation, &page).unwrap();
        }
        db.complete_container(container_key, generation).unwrap();
    }

    fn publish_zip_book(db: &SimilarDb, container_key: &str, pages: &[(&str, [u8; 32])]) {
        let generation = db
            .begin_container_build(container_key, ContainerKind::Zip, pages.len() as u32, 1, 2)
            .unwrap();
        for (page_index, &(entry_name, signature)) in pages.iter().enumerate() {
            let mut page = item(
                container_key,
                page_index as u32,
                signature,
                current_hash_version(),
            );
            page.item_key = format!("{container_key}\u{1f}{entry_name}");
            page.kind = ItemKind::ZipPage;
            db.stage_item(generation, &page).unwrap();
        }
        db.complete_container(container_key, generation).unwrap();
    }

    fn snapshot(db: &SimilarDb) -> Arc<SearchSnapshot> {
        let rows = db.load_base_search_rows(current_hash_version()).unwrap();
        Arc::new(SearchSnapshot::from_base(BaseArray {
            records: rows.records.into_boxed_slice(),
            store_id: rows.store_id,
            applied_seq: rows.applied_seq,
        }))
    }

    fn query(
        engine: &mut SimilarBookQueryEngine,
        container_key: &str,
        candidates: &[Arc<SearchSnapshot>],
    ) -> EngineObservation {
        engine
            .query(
                container_key,
                &[ROOT.to_owned()],
                candidates,
                Arc::new(AtomicBool::new(false)),
            )
            .unwrap()
    }

    fn cancel_query_at_phase_and_reuse_engine(
        mut engine: SimilarBookQueryEngine,
        container_key: String,
        candidates: Vec<Arc<SearchSnapshot>>,
        phase: crate::similar_book_query_test_probe::BookQueryTestPhase,
    ) {
        let cancel = Arc::new(AtomicBool::new(false));
        let (probe, mut phase_control) =
            crate::similar_book_query_test_probe::BookQueryTestPhaseProbe::new(phase);
        engine.set_test_phase_probe(probe);
        let worker_cancel = Arc::clone(&cancel);
        let worker_key = container_key.clone();
        let worker_candidates = candidates.clone();
        let (completed_tx, completed_rx) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            let result = engine.query(
                &worker_key,
                &[ROOT.to_owned()],
                &worker_candidates,
                worker_cancel,
            );
            let _ = completed_tx.send((engine, result));
        });

        phase_control.wait_reached();
        let cancel_for_thread = Arc::clone(&cancel);
        let canceller = std::thread::spawn(move || {
            cancel_for_thread.store(true, Ordering::Release);
        });
        canceller.join().unwrap();
        phase_control.release();

        let completed = completed_rx.recv_timeout(std::time::Duration::from_secs(5));
        if completed.is_ok() {
            worker.join().unwrap();
        }
        let (mut engine, cancelled_result) =
            completed.expect("cancelled query did not finish within the bounded wait");
        let cancelled_observation = cancelled_result.unwrap();
        assert!(matches!(
            cancelled_observation.outcome,
            Err(EngineError::Cancelled)
        ));
        let next = query(&mut engine, &container_key, &candidates);
        assert!(matches!(next.outcome, Ok(BookQuery::Ready(_))));
    }

    fn params() -> crate::dupe::book::Params {
        crate::dupe::book::Params {
            radius: BOOK_RADIUS,
            max_books_per_page: BOOK_MAX_BOOKS_PER_PAGE,
            min_quality: BOOK_MIN_QUALITY,
            coverage_threshold: BOOK_COVERAGE,
            min_matched_pages: BOOK_MIN_MATCHED_PAGES,
        }
    }

    fn dense_pages(book: u32, rows: &[SearchRow]) -> Vec<dupe::book::BookPage> {
        rows.iter()
            .map(|row| dupe::book::BookPage {
                book,
                index: row.item.page_index.unwrap(),
                quality: row.item.quality,
                sig: Sig::Bits(Box::new(row.item.pdq256)),
            })
            .collect()
    }

    fn brute_strip_overrides(
        origin_rows: &[SearchRow],
        candidate_rows: &[SearchRow],
        pair: &dupe::book::BookPair,
    ) -> Vec<BookPageMatch> {
        let origin_by_page = origin_rows
            .iter()
            .enumerate()
            .map(|(slot, row)| (row.item.page_index.unwrap(), (slot, row)))
            .collect::<BTreeMap<_, _>>();
        let candidate_by_page = candidate_rows
            .iter()
            .map(|row| (row.item.page_index.unwrap(), row))
            .collect::<BTreeMap<_, _>>();
        pair.alignment
            .iter()
            .map(|&(origin_page, candidate_page)| {
                let &(origin_slot, origin) = &origin_by_page[&origin_page];
                let candidate = candidate_by_page[&candidate_page];
                let distance = hamming256(&origin.item.pdq256, &candidate.item.pdq256);
                BookPageMatch {
                    origin_slot,
                    state: if matches!(
                        crate::similar_index::match_band(distance),
                        Some(crate::similar_index::MatchBand::NearlyIdentical)
                    ) {
                        BookPageMatchState::Strong
                    } else {
                        BookPageMatchState::Weak
                    },
                    other_page_index: candidate_page,
                    other_target: Some(SimilarItemTarget::File(
                        candidate.item.item_key.clone().into(),
                    )),
                    other_item_key: candidate.item.item_key.clone(),
                    other_mtime: candidate.item.mtime,
                    other_file_size: candidate.item.file_size,
                }
            })
            .collect()
    }

    #[test]
    fn same_transaction_engine_matches_dense_pair_and_builds_sparse_navigation() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("similar.db");
        let db = SimilarDb::open_at(&path).unwrap();
        let origin_key = format!("{ROOT}/origin");
        let candidate_key = format!("{ROOT}/candidate");
        let outside_candidate = "d:/outside/candidate";
        let signatures = [signature(0x03), signature(0x0c), signature(0x30)];
        publish_book(&db, &origin_key, &signatures, current_hash_version());
        publish_book(&db, &candidate_key, &signatures, current_hash_version());
        publish_book(&db, outside_candidate, &signatures, current_hash_version());
        let shared = snapshot(&db);
        let mut engine = SimilarBookQueryEngine::open_at(&path).unwrap().unwrap();

        let observation = query(&mut engine, &origin_key, &[Arc::clone(&shared)]);
        assert_eq!(observation.metadata.store_id, shared.base.store_id);
        assert_eq!(observation.metadata.read_seq, shared.applied_seq);
        let BookQuery::Ready(relations) = observation.outcome.unwrap() else {
            panic!("expected ready book relations");
        };
        assert_eq!(relations.hits.len(), 1);
        assert_eq!(relations.hits[0].other_container_key, candidate_key);
        assert_eq!(relations.hits[0].overrides().len(), 3);
        assert_eq!(relations.origin.pages.len(), 3);
        assert!(
            relations
                .origin
                .pages
                .iter()
                .all(|page| page.baseline == BookPageBaseline::Unmatched)
        );

        let origin_rows = db
            .load_book_pages(&origin_key, current_hash_version())
            .unwrap();
        let candidate_rows = db
            .load_book_pages(&candidate_key, current_hash_version())
            .unwrap();
        let mut dense_pages = Vec::new();
        for (book, rows) in [(BOOK_ORIGIN, origin_rows), (BOOK_CANDIDATE, candidate_rows)] {
            dense_pages.extend(rows.into_iter().map(|row| dupe::book::BookPage {
                book,
                index: row.item.page_index.unwrap(),
                quality: row.item.quality,
                sig: Sig::Bits(Box::new(row.item.pdq256)),
            }));
        }
        let expected =
            dupe::book::classify_pair(&dense_pages, params(), BOOK_ORIGIN, BOOK_CANDIDATE).unwrap();
        assert_eq!(relations.hits[0].pair, expected);
    }

    #[test]
    fn candidate_side_nontransitive_common_is_excluded_before_classification() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("similar.db");
        let db = SimilarDb::open_at(&path).unwrap();
        let origin_key = format!("{ROOT}/origin");
        let candidate_key = format!("{ROOT}/candidate");
        let origin_signature = [0u8; 32];
        let mut candidate_signature = [0u8; 32];
        candidate_signature[..4].fill(0xff);
        let mut witness_signature = [0u8; 32];
        witness_signature[..8].fill(0xff);
        publish_book(
            &db,
            &origin_key,
            &[origin_signature; 3],
            current_hash_version(),
        );
        publish_book(
            &db,
            &candidate_key,
            &[candidate_signature; 3],
            current_hash_version(),
        );
        for witness in 0..9 {
            publish_book(
                &db,
                &format!("{ROOT}/witness-{witness}"),
                &[witness_signature],
                current_hash_version(),
            );
        }
        let shared = snapshot(&db);
        let mut engine = SimilarBookQueryEngine::open_at(&path).unwrap().unwrap();

        let observation = query(&mut engine, &origin_key, &[shared]);
        let BookQuery::Ready(relations) = observation.outcome.unwrap() else {
            panic!("expected ready book relations");
        };
        assert_eq!(relations.hits.len(), 1);
        let hit = &relations.hits[0];
        assert_eq!(hit.other_container_key, candidate_key);
        assert_eq!(hit.pair.distinctive_a, 3);
        assert_eq!(hit.pair.distinctive_b, 0);
        assert_eq!(hit.pair.relation, dupe::book::Relation::Undecidable);
        assert!(hit.overrides().is_empty());
    }

    #[test]
    fn seven_books_per_origin_signature_produce_all_sparse_candidates_without_common_cutoff() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("similar.db");
        let db = SimilarDb::open_at(&path).unwrap();
        let origin_key = format!("{ROOT}/origin");
        // Even-parity four-bit codewords repeated across all bytes have pairwise distance at
        // least 64, so each origin page has an independent seven-book neighborhood.
        let codes = [0x00, 0x03, 0x05, 0x06, 0x09, 0x0a, 0x0c, 0x0f];
        let origin_signatures = codes.map(signature);
        publish_book(&db, &origin_key, &origin_signatures, current_hash_version());
        for (origin_slot, &origin_signature) in origin_signatures.iter().enumerate() {
            for candidate in 0..7 {
                publish_book(
                    &db,
                    &format!("{ROOT}/candidate-{origin_slot}-{candidate}"),
                    &[origin_signature; 3],
                    current_hash_version(),
                );
            }
        }
        let shared = snapshot(&db);
        let mut engine = SimilarBookQueryEngine::open_at(&path).unwrap().unwrap();

        let observation = query(&mut engine, &origin_key, &[shared]);
        let BookQuery::Ready(relations) = observation.outcome.unwrap() else {
            panic!("expected ready book relations");
        };
        assert_eq!(relations.origin.pages.len(), 8);
        assert!(
            relations
                .origin
                .pages
                .iter()
                .all(|page| page.baseline == BookPageBaseline::Unmatched)
        );
        assert_eq!(relations.hits.len(), 56);
        let actual_keys = relations
            .hits
            .iter()
            .map(|hit| hit.other_container_key.clone())
            .collect::<std::collections::BTreeSet<_>>();
        let expected_keys = (0..8)
            .flat_map(|origin_slot| {
                (0..7).map(move |candidate| format!("{ROOT}/candidate-{origin_slot}-{candidate}"))
            })
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(actual_keys, expected_keys);
        assert_eq!(
            relations
                .hits
                .iter()
                .map(|hit| hit.overrides().len())
                .sum::<usize>(),
            56
        );
        assert!(relations.hits.iter().all(|hit| {
            hit.pair.matched == 1
                && hit.pair.relation == dupe::book::Relation::Unrelated
                && hit.overrides().len() == 1
        }));
    }

    #[test]
    fn stale_hash_rows_followed_from_delta_are_normal_ineligible_hits() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("similar.db");
        let db = SimilarDb::open_at(&path).unwrap();
        let origin_key = format!("{ROOT}/origin");
        let stale_key = format!("{ROOT}/stale");
        let signatures = [signature(0x03), signature(0x0c), signature(0x30)];
        publish_book(&db, &origin_key, &signatures, current_hash_version());
        let before_stale = snapshot(&db);
        publish_book(&db, &stale_key, &signatures, current_hash_version() - 1);
        let mut engine = SimilarBookQueryEngine::open_at(&path).unwrap().unwrap();

        let observation = query(&mut engine, &origin_key, &[before_stale]);
        let BookQuery::Ready(relations) = observation.outcome.unwrap() else {
            panic!("expected ready book relations");
        };
        assert!(relations.hits.is_empty());
    }

    #[test]
    fn missing_same_transaction_mih_identity_is_an_invariant_failure() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("similar.db");
        let db = SimilarDb::open_at(&path).unwrap();
        let origin_key = format!("{ROOT}/origin");
        let signatures = [signature(0x03), signature(0x0c), signature(0x30)];
        publish_book(&db, &origin_key, &signatures, current_hash_version());
        let rows = db.load_base_search_rows(current_hash_version()).unwrap();
        let mut records = rows.records;
        records.push(SearchRecord {
            item_id: u64::MAX - 7,
            signature: signatures[0],
            quality: 50,
            revision: 1,
        });
        records.sort_by_key(|record| record.item_id);
        let fabricated = Arc::new(SearchSnapshot::from_base(BaseArray {
            records: records.into_boxed_slice(),
            store_id: rows.store_id,
            applied_seq: rows.applied_seq,
        }));
        let mut engine = SimilarBookQueryEngine::open_at(&path).unwrap().unwrap();

        let observation = query(&mut engine, &origin_key, &[fabricated]);
        assert!(matches!(
            observation.outcome,
            Err(EngineError::Invariant(message))
                if message.contains("was absent from the same SQLite snapshot")
        ));
    }

    #[test]
    fn scope_rejection_precedes_origin_and_mih_work_but_keeps_metadata() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("similar.db");
        let db = SimilarDb::open_at(&path).unwrap();
        let mut engine = SimilarBookQueryEngine::open_at(&path).unwrap().unwrap();

        let observation = query(&mut engine, "c:/outside/not-present", &[]);
        assert_eq!(observation.metadata.store_id, db.search_store_id().unwrap());
        assert!(matches!(observation.outcome, Ok(BookQuery::NotIndexed)));
    }

    #[test]
    fn not_book_and_featureless_finish_before_mih_preparation() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("similar.db");
        let db = SimilarDb::open_at(&path).unwrap();
        let empty_key = format!("{ROOT}/empty");
        let featureless_key = format!("{ROOT}/featureless");
        let generation = db
            .begin_container_build(&featureless_key, ContainerKind::ImageFolder, 3, 1, 2)
            .unwrap();
        for page_index in 0..3 {
            let mut page = item(
                &featureless_key,
                page_index,
                signature(page_index as u8),
                current_hash_version(),
            );
            page.quality = 0;
            db.stage_item(generation, &page).unwrap();
        }
        db.complete_container(&featureless_key, generation).unwrap();
        let mut engine = SimilarBookQueryEngine::open_at(&path).unwrap().unwrap();

        assert!(matches!(
            query(&mut engine, &empty_key, &[]).outcome,
            Ok(BookQuery::NotBook)
        ));
        assert!(engine.mih.preferred_base().is_none());
        assert!(matches!(
            query(&mut engine, &featureless_key, &[]).outcome,
            Ok(BookQuery::Featureless)
        ));
        assert!(engine.mih.preferred_base().is_none());
    }

    #[test]
    fn repeated_edges_discover_both_shapes_and_follow_a_same_transaction_update() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("similar.db");
        let db = SimilarDb::open_at(&path).unwrap();
        let origin_key = format!("{ROOT}/origin");
        let one_to_three = format!("{ROOT}/one-to-three");
        let three_to_one = format!("{ROOT}/three-to-one");
        let signature_a = signature(0x03);
        let signature_b = signature(0x0c);
        publish_book(
            &db,
            &origin_key,
            &[signature_a, signature_b, signature_b, signature_b],
            current_hash_version(),
        );
        publish_book(
            &db,
            &one_to_three,
            &[signature_a; 3],
            current_hash_version(),
        );
        let before_third_book = snapshot(&db);
        publish_book(&db, &three_to_one, &[signature_b], current_hash_version());
        let mut engine = SimilarBookQueryEngine::open_at(&path).unwrap().unwrap();

        let observation = query(&mut engine, &origin_key, &[before_third_book]);
        let BookQuery::Ready(relations) = observation.outcome.unwrap() else {
            panic!("expected ready book relations");
        };
        let hits = relations
            .hits
            .iter()
            .map(|hit| {
                (
                    hit.other_container_key.as_str(),
                    (hit.pair.matched, hit.overrides().len()),
                )
            })
            .collect::<BTreeMap<_, _>>();
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[one_to_three.as_str()], (1, 1));
        assert_eq!(hits[three_to_one.as_str()], (1, 1));
    }

    #[test]
    fn same_distance_but_different_mih_signature_is_an_invariant_failure() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("similar.db");
        let db = SimilarDb::open_at(&path).unwrap();
        let origin_key = format!("{ROOT}/origin");
        let candidate_key = format!("{ROOT}/candidate");
        let origin_signature = [0; 32];
        let mut stored_signature = [0; 32];
        stored_signature[0] = 0b0000_0001;
        let mut alternate_signature = [0; 32];
        alternate_signature[0] = 0b0000_0010;
        publish_book(
            &db,
            &origin_key,
            &[origin_signature; 3],
            current_hash_version(),
        );
        publish_book(
            &db,
            &candidate_key,
            &[stored_signature; 3],
            current_hash_version(),
        );
        let rows = db.load_base_search_rows(current_hash_version()).unwrap();
        let candidate_ids = db
            .load_book_pages(&candidate_key, current_hash_version())
            .unwrap()
            .into_iter()
            .map(|row| row.item_id)
            .collect::<HashSet<_>>();
        let mut records = rows.records;
        records
            .iter_mut()
            .find(|record| candidate_ids.contains(&record.item_id))
            .unwrap()
            .signature = alternate_signature;
        let mismatched = Arc::new(SearchSnapshot::from_base(BaseArray {
            records: records.into_boxed_slice(),
            store_id: rows.store_id,
            applied_seq: rows.applied_seq,
        }));
        let mut engine = SimilarBookQueryEngine::open_at(&path).unwrap().unwrap();

        let observation = query(&mut engine, &origin_key, &[mismatched]);
        assert!(matches!(
            observation.outcome,
            Err(EngineError::Invariant(message))
                if message.contains("differed from the same SQLite snapshot")
        ));
    }

    #[test]
    fn stale_zip_ordinals_match_the_current_filename_order() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("similar.db");
        let db = SimilarDb::open_at(&path).unwrap();
        let origin_key = format!("{ROOT}/origin");
        let candidate_key = format!("{ROOT}/candidate.zip");
        let signatures = [signature(0x03), signature(0x0c), signature(0x30)];
        publish_book(&db, &origin_key, &signatures, current_hash_version());
        publish_zip_book(
            &db,
            &candidate_key,
            &[
                ("003.jpg", signatures[2]),
                ("001.jpg", signatures[0]),
                ("002.jpg", signatures[1]),
            ],
        );
        rusqlite::Connection::open(&path)
            .unwrap()
            .execute(
                "UPDATE search_content_state SET page_order_version = 0 WHERE singleton = 1",
                [],
            )
            .unwrap();
        let shared = snapshot(&db);
        let mut engine = SimilarBookQueryEngine::open_at(&path).unwrap().unwrap();

        let observation = query(&mut engine, &origin_key, &[shared]);
        let BookQuery::Ready(relations) = observation.outcome.unwrap() else {
            panic!("expected ready book relations");
        };
        assert_eq!(relations.hits.len(), 1);
        assert_eq!(relations.hits[0].other_container_key, candidate_key);
        assert_eq!(
            relations.hits[0].pair.alignment,
            vec![(0, 0), (1, 1), (2, 2)]
        );
        assert_eq!(relations.hits[0].pair.relation, dupe::book::Relation::Same);
    }

    #[test]
    fn cancellation_after_metadata_is_an_inner_terminal_and_releases_the_reader() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("similar.db");
        let db = SimilarDb::open_at(&path).unwrap();
        let origin_key = format!("{ROOT}/origin");
        publish_book(
            &db,
            &origin_key,
            &[signature(0x03), signature(0x0c), signature(0x30)],
            current_hash_version(),
        );
        let mut engine = SimilarBookQueryEngine::open_at(&path).unwrap().unwrap();
        let cancelled = Arc::new(AtomicBool::new(false));
        let cancel_from_body = Arc::clone(&cancelled);
        let observation = engine
            .query_after_metadata(&origin_key, &[ROOT.to_owned()], &[], cancelled, move |_| {
                cancel_from_body.store(true, Ordering::Release)
            })
            .unwrap();
        assert!(matches!(observation.outcome, Err(EngineError::Cancelled)));

        let observation = query(&mut engine, &origin_key, &[]);
        assert!(matches!(observation.outcome, Ok(BookQuery::Ready(_))));
    }

    #[test]
    fn inflight_cancellation_during_mih_posting_scan_is_typed_and_engine_is_reusable() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("similar.db");
        let db = SimilarDb::open_at(&path).unwrap();
        let origin_key = format!("{ROOT}/origin");
        let candidate_key = format!("{ROOT}/candidate");
        publish_book(
            &db,
            &origin_key,
            &vec![signature(0); 3],
            current_hash_version(),
        );
        publish_book(
            &db,
            &candidate_key,
            &vec![signature(0); 70],
            current_hash_version(),
        );
        let shared = snapshot(&db);
        drop(db);

        cancel_query_at_phase_and_reuse_engine(
            SimilarBookQueryEngine::open_at(&path).unwrap().unwrap(),
            origin_key,
            vec![shared],
            crate::similar_book_query_test_probe::BookQueryTestPhase::MihPostingScan,
        );
    }

    #[test]
    fn inflight_cancellation_inside_body_sql_progress_is_typed_and_engine_is_reusable() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("similar.db");
        let db = SimilarDb::open_at(&path).unwrap();
        let origin_key = format!("{ROOT}/origin");
        publish_book(
            &db,
            &origin_key,
            &vec![signature(0x55); 1_500],
            current_hash_version(),
        );
        let shared = snapshot(&db);
        drop(db);

        cancel_query_at_phase_and_reuse_engine(
            SimilarBookQueryEngine::open_at(&path).unwrap().unwrap(),
            origin_key,
            vec![shared],
            crate::similar_book_query_test_probe::BookQueryTestPhase::SqlProgress,
        );
    }

    #[test]
    fn inflight_cancellation_at_direct_candidate_slot_is_typed_and_engine_is_reusable() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("similar.db");
        let db = SimilarDb::open_at(&path).unwrap();
        let origin_key = format!("{ROOT}/origin");
        let candidate_key = format!("{ROOT}/candidate");
        let origin = [signature(0), signature(1), signature(2)];
        let mut candidate = vec![signature(0xff); 1_100];
        candidate[..origin.len()].copy_from_slice(&origin);
        publish_book(&db, &origin_key, &origin, current_hash_version());
        publish_book(&db, &candidate_key, &candidate, current_hash_version());
        let shared = snapshot(&db);
        drop(db);

        cancel_query_at_phase_and_reuse_engine(
            SimilarBookQueryEngine::open_at(&path).unwrap().unwrap(),
            origin_key,
            vec![shared],
            crate::similar_book_query_test_probe::BookQueryTestPhase::DirectCandidateSlot,
        );
    }

    #[test]
    fn metadata_read_fixes_the_database_snapshot_before_a_writer_update() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("similar.db");
        let db = SimilarDb::open_at(&path).unwrap();
        let origin_key = format!("{ROOT}/origin");
        let candidate_key = format!("{ROOT}/candidate");
        let matching = [signature(0x03), signature(0x0c), signature(0x30)];
        let unrelated = [signature(0xc0), signature(0xc3), signature(0xc5)];
        publish_book(&db, &origin_key, &matching, current_hash_version());
        publish_book(&db, &candidate_key, &matching, current_hash_version());
        let shared_before_update = snapshot(&db);
        let mut engine = SimilarBookQueryEngine::open_at(&path).unwrap().unwrap();

        let old_observation = engine
            .query_after_metadata(
                &origin_key,
                &[ROOT.to_owned()],
                &[Arc::clone(&shared_before_update)],
                Arc::new(AtomicBool::new(false)),
                |_| publish_book(&db, &candidate_key, &unrelated, current_hash_version()),
            )
            .unwrap();
        let BookQuery::Ready(old_relations) = old_observation.outcome.unwrap() else {
            panic!("the fixed snapshot lost the pre-update candidate");
        };
        assert_eq!(old_relations.hits.len(), 1);
        assert_eq!(old_relations.hits[0].other_container_key, candidate_key);
        assert_eq!(
            old_relations.hits[0].pair.relation,
            dupe::book::Relation::Same
        );
        assert!(
            old_observation.metadata.read_seq < db.load_item_changes_after(0).unwrap().latest_seq
        );

        let new_observation = query(&mut engine, &origin_key, &[shared_before_update]);
        let BookQuery::Ready(new_relations) = new_observation.outcome.unwrap() else {
            panic!("expected a terminal result after the update");
        };
        assert!(new_relations.hits.is_empty());
        assert!(new_observation.metadata.read_seq > old_observation.metadata.read_seq);
    }

    #[test]
    fn three_hundred_page_pair_flushes_full_and_partial_hit_chunks() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("similar.db");
        let db = SimilarDb::open_at(&path).unwrap();
        let origin_key = format!("{ROOT}/origin");
        let candidate_key = format!("{ROOT}/candidate");
        let signatures = vec![signature(0x03); 300];
        publish_book(&db, &origin_key, &signatures, current_hash_version());
        publish_book(&db, &candidate_key, &signatures, current_hash_version());
        let shared = snapshot(&db);
        let mut engine = SimilarBookQueryEngine::open_at(&path).unwrap().unwrap();

        let observation = query(&mut engine, &origin_key, &[shared]);
        let BookQuery::Ready(relations) = observation.outcome.unwrap() else {
            panic!("expected ready book relations");
        };
        assert_eq!(relations.hits.len(), 1);
        assert_eq!(relations.hits[0].other_container_key, candidate_key);
        assert_eq!(relations.hits[0].pair.matched, 300);
        assert_eq!(relations.hits[0].pair.alignment.len(), 300);
        assert_eq!(relations.hits[0].overrides().len(), 300);
    }

    #[test]
    fn mixed_common_quality_and_delta_corpus_matches_the_dense_oracle() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("similar.db");
        let db = SimilarDb::open_at(&path).unwrap();
        let origin_key = format!("{ROOT}/origin");
        let candidate_a = format!("{ROOT}/candidate-a");
        let candidate_b = format!("{ROOT}/candidate-b");
        let common = signature(0xff);
        let origin_pages = [
            (signature(0x00), 50),
            (signature(0x03), 50),
            (common, 50),
            (signature(0x55), 0),
            (signature(0x05), 50),
        ];
        publish_book_with_qualities(&db, &origin_key, &origin_pages);
        publish_book_with_qualities(&db, &candidate_a, &origin_pages);
        let witness_keys = (0..9)
            .map(|index| format!("{ROOT}/common-witness-{index}"))
            .collect::<Vec<_>>();
        for witness in &witness_keys[..5] {
            publish_book(&db, witness, &[common], current_hash_version());
        }
        let shared_before_delta = snapshot(&db);
        let mut candidate_b_pages = origin_pages;
        candidate_b_pages[0].0[0] = 0xff;
        publish_book_with_qualities(&db, &candidate_b, &candidate_b_pages);
        for witness in &witness_keys[5..] {
            publish_book(&db, witness, &[common], current_hash_version());
        }
        let mut engine = SimilarBookQueryEngine::open_at(&path).unwrap().unwrap();

        let observation = query(&mut engine, &origin_key, &[shared_before_delta]);
        let BookQuery::Ready(relations) = observation.outcome.unwrap() else {
            panic!("expected ready book relations");
        };
        assert_eq!(
            relations
                .origin
                .pages
                .iter()
                .map(|page| page.baseline)
                .collect::<Vec<_>>(),
            vec![
                BookPageBaseline::Unmatched,
                BookPageBaseline::Unmatched,
                BookPageBaseline::Excluded,
                BookPageBaseline::Excluded,
                BookPageBaseline::Unmatched,
            ]
        );
        let actual = relations
            .hits
            .iter()
            .map(|hit| (hit.other_container_key.as_str(), hit))
            .collect::<BTreeMap<_, _>>();
        assert_eq!(
            actual.keys().copied().collect::<Vec<_>>(),
            vec![candidate_a.as_str(), candidate_b.as_str()]
        );

        let origin_rows = db
            .load_book_pages(&origin_key, current_hash_version())
            .unwrap();
        for candidate_key in [&candidate_a, &candidate_b] {
            let candidate_rows = db
                .load_book_pages(candidate_key, current_hash_version())
                .unwrap();
            let mut corpus = dense_pages(BOOK_ORIGIN, &origin_rows);
            corpus.extend(dense_pages(BOOK_CANDIDATE, &candidate_rows));
            for (offset, witness) in witness_keys.iter().enumerate() {
                let rows = db.load_book_pages(witness, current_hash_version()).unwrap();
                corpus.extend(dense_pages(3 + offset as u32, &rows));
            }
            let analysis = dupe::book::analyze(&corpus, params()).unwrap();
            let expected_pair = analysis
                .pairs
                .into_iter()
                .find(|pair| pair.a == BOOK_ORIGIN && pair.b == BOOK_CANDIDATE)
                .expect("dense oracle must retain the candidate pair");
            let hit = actual[candidate_key.as_str()];
            assert_eq!(hit.pair, expected_pair);
            assert_eq!(
                hit.overrides(),
                brute_strip_overrides(&origin_rows, &candidate_rows, &expected_pair)
            );
        }
    }

    fn prepared_scale_fixture_dir(name: &str) -> std::path::PathBuf {
        use std::os::windows::fs::MetadataExt;

        let run = std::env::var_os("MIV_BOOK_QUERY_SCALE_RUN_DIR")
            .map(std::path::PathBuf::from)
            .expect("set MIV_BOOK_QUERY_SCALE_RUN_DIR to the prepared fresh run directory");
        let run = std::fs::canonicalize(&run).expect("scale run directory must already exist");
        let directory = std::fs::canonicalize(run.join(name)).unwrap_or_else(|error| {
            panic!("prepared fixture directory {name} is missing: {error}")
        });
        assert_eq!(
            directory.parent(),
            Some(run.as_path()),
            "fixture directory must be a direct child of the prepared run directory"
        );
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
        assert!(
            std::fs::read_dir(&directory).unwrap().next().is_none(),
            "synthetic fixture directory must be empty: {}",
            directory.display()
        );
        directory
    }

    fn affine_signature(code: usize) -> [u8; 32] {
        assert!(code < 512);
        let a = (code / 2) as u8;
        let b = (code & 1) != 0;
        let mut signature = [0u8; 32];
        for x in 0u16..256 {
            let bit = ((a & x as u8).count_ones() & 1 != 0) ^ b;
            if bit {
                signature[usize::from(x / 8)] |= 1 << (x % 8);
            }
        }
        signature
    }

    fn signature_distance(left: &[u8; 32], right: &[u8; 32]) -> u32 {
        left.iter()
            .zip(right)
            .map(|(&left, &right)| (left ^ right).count_ones())
            .sum()
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

    #[test]
    fn affine_scale_signatures_are_unique_and_outside_the_book_radius() {
        let signatures = (0..400).map(affine_signature).collect::<Vec<_>>();
        for left in 0..signatures.len() {
            for right in (left + 1)..signatures.len() {
                assert!(signature_distance(&signatures[left], &signatures[right]) > BOOK_RADIUS);
            }
        }
    }

    fn finish_scale_fixture(
        directory: &Path,
        label: &str,
        origin_key: &str,
        pages: usize,
        candidates: usize,
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
        assert!(!path.with_file_name("similar.db-wal").exists());
        assert!(!path.with_file_name("similar.db-shm").exists());
        let inputs = [
            scale_fixture_input("database", &path),
            scale_fixture_input("base", &directory.join("similar.base")),
        ];
        let manifest = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(directory.join("fixture.json"))
            .unwrap();
        serde_json::to_writer_pretty(
            manifest,
            &serde_json::json!({
                "schema": 1,
                "fixture": label,
                "origin_key": origin_key,
                "origin_pages": pages,
                "candidate_count": candidates,
                "database": "similar.db",
                "base": "similar.base",
                "inputs": inputs,
                "journal_mode": "delete",
            }),
        )
        .unwrap();
    }

    #[test]
    #[ignore = "generates persistent 7N and 10,000-page scale stores in a prepared fresh directory"]
    fn generate_and_verify_large_book_engine_fixtures() {
        let signatures = (0..400).map(affine_signature).collect::<Vec<_>>();

        let directory = prepared_scale_fixture_dir("affine-7n-400");
        let path = directory.join("similar.db");
        let db = SimilarDb::open_at(&path).unwrap();
        let origin_key = format!("{ROOT}/origin-affine-400");
        publish_book(&db, &origin_key, &signatures, current_hash_version());
        for (origin_slot, &signature) in signatures.iter().enumerate() {
            for witness in 0..7 {
                let key = format!("{ROOT}/candidate-{origin_slot:03}-{witness}");
                publish_book(
                    &db,
                    &key,
                    &[signature, signature, signature],
                    current_hash_version(),
                );
            }
        }
        let snapshot = Arc::new(
            crate::similar_search_array::rebuild_from_sqlite(&db, &directory.join("similar.base"))
                .unwrap(),
        );
        drop(db);
        let mut engine = SimilarBookQueryEngine::open_at(&path).unwrap().unwrap();
        let observation = query(&mut engine, &origin_key, &[snapshot]);
        let BookQuery::Ready(relations) = observation.outcome.unwrap() else {
            panic!("affine 7N fixture must be Ready");
        };
        assert_eq!(relations.origin.pages.len(), 400);
        assert_eq!(relations.hits.len(), 2_800);
        for (index, hit) in relations.hits.iter().enumerate() {
            let origin_slot = index / 7;
            let witness = index % 7;
            assert_eq!(
                hit.other_container_key,
                format!("{ROOT}/candidate-{origin_slot:03}-{witness}")
            );
            assert_eq!(hit.other_page_count, 3);
            assert_eq!(hit.pair.a, BOOK_ORIGIN);
            assert_eq!(hit.pair.b, BOOK_CANDIDATE);
            assert_eq!(hit.pair.matched, 1);
            assert_eq!(hit.pair.distinctive_a, 400);
            assert_eq!(hit.pair.distinctive_b, 3);
            assert_eq!(hit.pair.coverage_a, 1.0 / 400.0);
            assert_eq!(hit.pair.coverage_b, 1.0 / 3.0);
            assert_eq!(hit.pair.relation, dupe::book::Relation::Unrelated);
            // Keep the legacy complete-tie visit order: X versus XXX chooses A0-B2.
            assert_eq!(hit.pair.alignment, vec![(origin_slot as u32, 2)]);
            assert_eq!(hit.overrides().len(), 1);
            assert_eq!(hit.overrides()[0].origin_slot, origin_slot);
            assert_eq!(hit.overrides()[0].other_page_index, 2);
        }
        drop(engine);
        finish_scale_fixture(&directory, "affine-7n-400", &origin_key, 400, 2_800);

        let x = affine_signature(2);
        let y = affine_signature(4);
        assert!(signature_distance(&x, &y) > BOOK_RADIUS);
        for (name, origin, candidate, expected_alignment) in [
            (
                "dense-diagonal-10000",
                vec![x; 10_000],
                vec![x; 10_000],
                (0..10_000)
                    .map(|index| (index as u32, index as u32))
                    .collect::<Vec<_>>(),
            ),
            (
                "dense-general-10000",
                {
                    let mut pages = vec![x; 10_000];
                    pages[9_999] = y;
                    pages
                },
                {
                    let mut pages = vec![x; 10_000];
                    pages[0] = y;
                    pages
                },
                (0..9_999)
                    .map(|index| (index as u32, index as u32 + 1))
                    .collect::<Vec<_>>(),
            ),
        ] {
            let directory = prepared_scale_fixture_dir(name);
            let path = directory.join("similar.db");
            let db = SimilarDb::open_at(&path).unwrap();
            let origin_key = format!("{ROOT}/{name}-origin");
            let candidate_key = format!("{ROOT}/{name}-candidate");
            publish_book(&db, &origin_key, &origin, current_hash_version());
            publish_book(&db, &candidate_key, &candidate, current_hash_version());
            let snapshot = Arc::new(
                crate::similar_search_array::rebuild_from_sqlite(
                    &db,
                    &directory.join("similar.base"),
                )
                .unwrap(),
            );
            drop(db);
            let mut engine = SimilarBookQueryEngine::open_at(&path).unwrap().unwrap();
            let observation = query(&mut engine, &origin_key, &[snapshot]);
            let BookQuery::Ready(relations) = observation.outcome.unwrap() else {
                panic!("{name} fixture must be Ready");
            };
            assert_eq!(relations.hits.len(), 1);
            let hit = &relations.hits[0];
            assert_eq!(hit.other_container_key, candidate_key);
            assert_eq!(hit.other_page_count, 10_000);
            assert_eq!(hit.pair.a, BOOK_ORIGIN);
            assert_eq!(hit.pair.b, BOOK_CANDIDATE);
            assert_eq!(hit.pair.distinctive_a, 10_000);
            assert_eq!(hit.pair.distinctive_b, 10_000);
            assert_eq!(hit.pair.matched as usize, expected_alignment.len());
            assert_eq!(hit.pair.alignment, expected_alignment);
            assert_eq!(hit.pair.coverage_a, hit.pair.matched as f32 / 10_000.0);
            assert_eq!(hit.pair.coverage_b, hit.pair.matched as f32 / 10_000.0);
            assert_eq!(hit.pair.relation, dupe::book::Relation::Same);
            assert_eq!(hit.overrides().len(), hit.pair.matched as usize);
            for (override_page, &(origin_page, candidate_page)) in
                hit.overrides().iter().zip(&hit.pair.alignment)
            {
                assert_eq!(override_page.origin_slot, origin_page as usize);
                assert_eq!(override_page.other_page_index, candidate_page);
            }
            drop(engine);
            finish_scale_fixture(&directory, name, &origin_key, 10_000, 1);
        }
    }
}
