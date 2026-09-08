//! Book-level relation analysis over caller-supplied page signatures.
//!
//! This module owns no paths, decoders, PDF state, persistence, or UI. A caller
//! supplies one bit signature per page and decides every measurement parameter.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fmt;
use std::sync::atomic::{AtomicBool, Ordering};

use super::Sig;

pub const DEFAULT_RADIUS: u32 = 8;
pub const DEFAULT_MAX_BOOKS_PER_PAGE: u32 = 8;
pub const DEFAULT_MIN_QUALITY: u8 = 1;
pub const DEFAULT_COVERAGE_THRESHOLD: f32 = 0.5;
pub const DEFAULT_MIN_MATCHED_PAGES: u32 = 3;

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct BookPage {
    pub book: u32,
    pub index: u32,
    pub quality: u8,
    pub sig: Sig,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Params {
    pub radius: u32,
    pub max_books_per_page: u32,
    pub min_quality: u8,
    pub coverage_threshold: f32,
    pub min_matched_pages: u32,
}

impl Default for Params {
    fn default() -> Self {
        Self {
            radius: DEFAULT_RADIUS,
            max_books_per_page: DEFAULT_MAX_BOOKS_PER_PAGE,
            min_quality: DEFAULT_MIN_QUALITY,
            coverage_threshold: DEFAULT_COVERAGE_THRESHOLD,
            min_matched_pages: DEFAULT_MIN_MATCHED_PAGES,
        }
    }
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Relation {
    Same,
    Contains { whole: u32 },
    Unrelated,
    Undecidable,
}

#[derive(Clone, PartialEq, Debug)]
pub struct BookPair {
    pub a: u32,
    pub b: u32,
    pub matched: u32,
    pub distinctive_a: u32,
    pub distinctive_b: u32,
    pub coverage_a: f32,
    pub coverage_b: f32,
    pub relation: Relation,
    pub alignment: Vec<(u32, u32)>,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct BookStats {
    pub book: u32,
    pub total_pages: u32,
    pub distinctive_pages: u32,
    pub featureless_pages: u32,
    pub common_pages: u32,
}

#[derive(Clone, PartialEq, Debug)]
pub struct Analysis {
    /// Pairs sharing at least one distinctive page within the requested radius.
    pub pairs: Vec<BookPair>,
    /// Per-book denominator and exclusion counts, including undecidable books.
    pub books: Vec<BookStats>,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum AnalyzeError {
    InvalidCoverage,
    EmptyBitSignature {
        book: u32,
        index: u32,
    },
    NonBitSignature {
        book: u32,
        index: u32,
    },
    MixedBitWidths {
        book: u32,
        index: u32,
        expected_bytes: usize,
        actual_bytes: usize,
    },
    RadiusExceedsBitWidth {
        radius: u32,
        bit_width: u32,
    },
    DuplicatePageIndex {
        book: u32,
        index: u32,
    },
    UnknownBook(u32),
    SameBook(u32),
}

impl fmt::Display for AnalyzeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidCoverage => {
                write!(formatter, "coverage_threshold must be finite and in 0..=1")
            }
            Self::EmptyBitSignature { book, index } => {
                write!(
                    formatter,
                    "book {book} page {index} has an empty bit signature"
                )
            }
            Self::NonBitSignature { book, index } => {
                write!(
                    formatter,
                    "book {book} page {index} does not have a bit signature"
                )
            }
            Self::MixedBitWidths {
                book,
                index,
                expected_bytes,
                actual_bytes,
            } => write!(
                formatter,
                "book {book} page {index} has {actual_bytes} signature bytes; expected {expected_bytes}"
            ),
            Self::RadiusExceedsBitWidth { radius, bit_width } => {
                write!(
                    formatter,
                    "radius {radius} exceeds signature width {bit_width}"
                )
            }
            Self::DuplicatePageIndex { book, index } => {
                write!(
                    formatter,
                    "book {book} contains duplicate page index {index}"
                )
            }
            Self::UnknownBook(book) => write!(formatter, "unknown book id {book}"),
            Self::SameBook(book) => write!(formatter, "cannot compare book {book} with itself"),
        }
    }
}

impl std::error::Error for AnalyzeError {}

/// Finds candidate book pairs and classifies their directional coverage.
///
/// A pair is emitted only when it shares at least one non-excluded page within
/// the requested radius. Call classify_pair when an explicit unrelated or
/// undecidable pair must also be represented.
pub fn analyze(pages: &[BookPage], params: Params) -> Result<Analysis, AnalyzeError> {
    let prepared = PreparedCorpus::new(pages, params)?;
    let mut pair_keys = BTreeSet::new();
    for near in &prepared.near_pairs {
        if prepared.pages[near.left].common || prepared.pages[near.right].common {
            continue;
        }
        let left_book = prepared.pages[near.left].page.book;
        let right_book = prepared.pages[near.right].page.book;
        if left_book != right_book {
            pair_keys.insert(ordered_pair(left_book, right_book));
        }
    }
    let pairs = pair_keys
        .into_iter()
        .map(|(a, b)| prepared.classify(a, b))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Analysis {
        pairs,
        books: prepared.stats.values().cloned().collect(),
    })
}

/// Classifies one requested pair while using the entire supplied corpus for
/// direct-neighborhood book-frequency exclusion.
pub fn classify_pair(
    pages: &[BookPage],
    params: Params,
    a: u32,
    b: u32,
) -> Result<BookPair, AnalyzeError> {
    PreparedCorpus::new(pages, params)?.classify(a, b)
}

/// One same-transaction page fact used by the streaming book classifier.
///
/// `common` is already resolved against the whole eligible library. The fixed
/// signature width prevents the streaming path from accepting mixed-width
/// input; the public generic classifier retains its existing validation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct VerifiedBookPage {
    pub(crate) index: u32,
    pub(crate) quality: u8,
    pub(crate) common: bool,
    pub(crate) signature: [u8; 32],
}

/// Validated page facts and exclusion statistics reusable across pair queries.
#[derive(Debug)]
pub(crate) struct PreparedBookSide {
    book: u32,
    params: Params,
    pages: Box<[VerifiedBookPage]>,
    distinctive_slots: Box<[usize]>,
    stats: BookStats,
}

impl PreparedBookSide {
    pub(crate) fn new(
        book: u32,
        mut pages: Vec<VerifiedBookPage>,
        params: Params,
    ) -> Result<Self, AnalyzeError> {
        validate_params(params)?;
        if params.radius > 256 {
            return Err(AnalyzeError::RadiusExceedsBitWidth {
                radius: params.radius,
                bit_width: 256,
            });
        }
        if pages.is_empty() {
            return Err(AnalyzeError::UnknownBook(book));
        }
        pages.sort_by_key(|page| page.index);
        if let Some(duplicate) = pages.windows(2).find(|pair| pair[0].index == pair[1].index) {
            return Err(AnalyzeError::DuplicatePageIndex {
                book,
                index: duplicate[0].index,
            });
        }

        let mut stats = BookStats {
            book,
            total_pages: 0,
            distinctive_pages: 0,
            featureless_pages: 0,
            common_pages: 0,
        };
        let mut distinctive_slots = Vec::new();
        for (slot, page) in pages.iter().enumerate() {
            record_page_stats(&mut stats, page.quality, page.common, params.min_quality);
            if page.quality >= params.min_quality && !page.common {
                distinctive_slots.push(slot);
            }
        }
        Ok(Self {
            book,
            params,
            pages: pages.into_boxed_slice(),
            distinctive_slots: distinctive_slots.into_boxed_slice(),
            stats,
        })
    }

    pub(crate) fn book(&self) -> u32 {
        self.book
    }

    pub(crate) fn pages(&self) -> &[VerifiedBookPage] {
        &self.pages
    }

    pub(crate) fn distinctive_slots(&self) -> &[usize] {
        &self.distinctive_slots
    }

    pub(crate) fn stats(&self) -> &BookStats {
        &self.stats
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum EdgeVisitControl {
    Continue,
    Stop,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum EdgeVisitCompletion {
    Exhausted,
    Stopped,
}

/// Re-enumerates every true edge from one prepared A slot into prepared B.
///
/// Discovery saturation or sampling must not be applied here. Repeated calls
/// for the same slot and same read transaction must return the same edge set.
pub(crate) trait ReenumeratedBookEdges {
    type Error;

    fn visit_a(
        &mut self,
        a_slot: usize,
        visitor: &mut dyn FnMut(usize, u32) -> EdgeVisitControl,
    ) -> Result<EdgeVisitCompletion, Self::Error>;
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum ReenumeratedPairError<E> {
    Analyze(AnalyzeError),
    Cancelled,
    Source(E),
    SourceStopped,
    ParamsMismatch,
    InvalidBookOrder { a: u32, b: u32 },
    InvalidOriginSlot(usize),
    DuplicateOriginSlot(usize),
    ExcludedOriginSlot(usize),
    InvalidCandidateSlot { a_slot: usize, b_slot: usize },
    DistanceExceedsRadius { distance: u32, radius: u32 },
    ArithmeticOverflow,
    ReplayInvariant,
}

impl<E> From<AnalyzeError> for ReenumeratedPairError<E> {
    fn from(value: AnalyzeError) -> Self {
        Self::Analyze(value)
    }
}

struct PreparedPage<'a> {
    page: &'a BookPage,
    bits: &'a [u8],
    common: bool,
}

#[derive(Clone, Copy)]
struct NearPair {
    left: usize,
    right: usize,
    distance: u32,
}

struct PreparedCorpus<'a> {
    params: Params,
    pages: Vec<PreparedPage<'a>>,
    near_pairs: Vec<NearPair>,
    stats: BTreeMap<u32, BookStats>,
}

impl<'a> PreparedCorpus<'a> {
    fn new(pages: &'a [BookPage], params: Params) -> Result<Self, AnalyzeError> {
        validate_params(params)?;

        let mut seen_page_indices = HashSet::new();
        let mut expected_bytes = None;
        let mut prepared = Vec::with_capacity(pages.len());
        for page in pages {
            if !seen_page_indices.insert((page.book, page.index)) {
                return Err(AnalyzeError::DuplicatePageIndex {
                    book: page.book,
                    index: page.index,
                });
            }
            let Sig::Bits(bits) = &page.sig else {
                return Err(AnalyzeError::NonBitSignature {
                    book: page.book,
                    index: page.index,
                });
            };
            if bits.is_empty() {
                return Err(AnalyzeError::EmptyBitSignature {
                    book: page.book,
                    index: page.index,
                });
            }
            if let Some(expected) = expected_bytes {
                if bits.len() != expected {
                    return Err(AnalyzeError::MixedBitWidths {
                        book: page.book,
                        index: page.index,
                        expected_bytes: expected,
                        actual_bytes: bits.len(),
                    });
                }
            } else {
                expected_bytes = Some(bits.len());
            }
            prepared.push(PreparedPage {
                page,
                bits,
                common: false,
            });
        }

        let bit_width = expected_bytes.unwrap_or(0) as u32 * 8;
        if bit_width > 0 && params.radius > bit_width {
            return Err(AnalyzeError::RadiusExceedsBitWidth {
                radius: params.radius,
                bit_width,
            });
        }
        let eligible = prepared
            .iter()
            .enumerate()
            .filter_map(|(index, page)| (page.page.quality >= params.min_quality).then_some(index))
            .collect::<Vec<_>>();
        let near_pairs = exact_near_pairs(&prepared, &eligible, params.radius, bit_width);

        let mut neighborhood_books = vec![BTreeSet::new(); prepared.len()];
        for &index in &eligible {
            neighborhood_books[index].insert(prepared[index].page.book);
        }
        for near in &near_pairs {
            let left_book = prepared[near.left].page.book;
            let right_book = prepared[near.right].page.book;
            neighborhood_books[near.left].insert(right_book);
            neighborhood_books[near.right].insert(left_book);
        }
        for &index in &eligible {
            let is_common = neighborhood_books[index].len() as u32 > params.max_books_per_page;
            prepared[index].common = is_common;
        }
        let mut stats = BTreeMap::<u32, BookStats>::new();
        for page in &prepared {
            let book_stats = stats.entry(page.page.book).or_insert(BookStats {
                book: page.page.book,
                total_pages: 0,
                distinctive_pages: 0,
                featureless_pages: 0,
                common_pages: 0,
            });
            record_page_stats(
                book_stats,
                page.page.quality,
                page.common,
                params.min_quality,
            );
        }

        Ok(Self {
            params,
            pages: prepared,
            near_pairs,
            stats,
        })
    }

    fn classify(&self, first: u32, second: u32) -> Result<BookPair, AnalyzeError> {
        if first == second {
            return Err(AnalyzeError::SameBook(first));
        }
        let (a, b) = ordered_pair(first, second);
        let stats_a = self.stats.get(&a).ok_or(AnalyzeError::UnknownBook(a))?;
        let stats_b = self.stats.get(&b).ok_or(AnalyzeError::UnknownBook(b))?;
        if stats_a.distinctive_pages == 0 || stats_b.distinctive_pages == 0 {
            return Ok(finish_pair(a, b, stats_a, stats_b, self.params, Vec::new()));
        }

        let mut candidates = Vec::new();
        for near in &self.near_pairs {
            let left = &self.pages[near.left];
            let right = &self.pages[near.right];
            if left.common || right.common {
                continue;
            }
            let books = ordered_pair(left.page.book, right.page.book);
            if books != (a, b) || left.page.book == right.page.book {
                continue;
            }
            let (index_a, index_b) = if left.page.book == a {
                (left.page.index, right.page.index)
            } else {
                (right.page.index, left.page.index)
            };
            candidates.push(AlignmentCandidate {
                index_a,
                index_b,
                distance: near.distance,
            });
        }
        let alignment = weighted_monotonic_alignment(candidates);
        Ok(finish_pair(a, b, stats_a, stats_b, self.params, alignment))
    }
}

fn validate_params(params: Params) -> Result<(), AnalyzeError> {
    if !params.coverage_threshold.is_finite() || !(0.0..=1.0).contains(&params.coverage_threshold) {
        Err(AnalyzeError::InvalidCoverage)
    } else {
        Ok(())
    }
}

fn record_page_stats(stats: &mut BookStats, quality: u8, common: bool, min_quality: u8) {
    stats.total_pages += 1;
    if quality < min_quality {
        stats.featureless_pages += 1;
    } else if common {
        stats.common_pages += 1;
    } else {
        stats.distinctive_pages += 1;
    }
}

fn finish_pair(
    a: u32,
    b: u32,
    stats_a: &BookStats,
    stats_b: &BookStats,
    params: Params,
    alignment: Vec<(u32, u32)>,
) -> BookPair {
    if stats_a.distinctive_pages == 0 || stats_b.distinctive_pages == 0 {
        return BookPair {
            a,
            b,
            matched: 0,
            distinctive_a: stats_a.distinctive_pages,
            distinctive_b: stats_b.distinctive_pages,
            coverage_a: 0.0,
            coverage_b: 0.0,
            relation: Relation::Undecidable,
            alignment: Vec::new(),
        };
    }

    let matched = alignment.len() as u32;
    let coverage_a = matched as f32 / stats_a.distinctive_pages as f32;
    let coverage_b = matched as f32 / stats_b.distinctive_pages as f32;
    let covers_a = coverage_a >= params.coverage_threshold;
    let covers_b = coverage_b >= params.coverage_threshold;
    let relation = if matched < params.min_matched_pages {
        Relation::Unrelated
    } else {
        match (covers_a, covers_b) {
            (true, true) => Relation::Same,
            (true, false) => Relation::Contains { whole: b },
            (false, true) => Relation::Contains { whole: a },
            (false, false) => Relation::Unrelated,
        }
    };
    BookPair {
        a,
        b,
        matched,
        distinctive_a: stats_a.distinctive_pages,
        distinctive_b: stats_b.distinctive_pages,
        coverage_a,
        coverage_b,
        relation,
        alignment,
    }
}

fn ordered_pair(a: u32, b: u32) -> (u32, u32) {
    if a < b { (a, b) } else { (b, a) }
}

/// Classifies one already ordered pair from a repeatable same-transaction edge source.
///
/// `origin_edge_slots` is the complete, sorted, unique set of distinctive A slots
/// that have at least one true edge into B. Discovery may saturate its per-book
/// count, but it must continue recording these slots. This lets all candidate
/// pairs share one prepared origin without querying every origin page per pair.
pub(crate) fn classify_pair_reenumerated<S>(
    a: &PreparedBookSide,
    b: &PreparedBookSide,
    origin_edge_slots: &[usize],
    source: &mut S,
    cancel: &AtomicBool,
) -> Result<BookPair, ReenumeratedPairError<S::Error>>
where
    S: ReenumeratedBookEdges,
{
    if a.params != b.params {
        return Err(ReenumeratedPairError::ParamsMismatch);
    }
    if a.book >= b.book {
        return Err(ReenumeratedPairError::InvalidBookOrder {
            a: a.book,
            b: b.book,
        });
    }
    validate_origin_edge_slots(a, origin_edge_slots)?;
    check_stream_cancel(cancel)?;

    if a.stats.distinctive_pages == 0 || b.stats.distinctive_pages == 0 {
        return Ok(finish_pair(
            a.book,
            b.book,
            &a.stats,
            &b.stats,
            a.params,
            Vec::new(),
        ));
    }

    // Equal-size distinctive sequences with every k-to-k edge have one possible
    // full monotonic bijection. Check this before discovering global B values;
    // a dense 10k x 10k equal-signature pair must remain O(N).
    if let Some(alignment) = exact_diagonal_alignment(a, b, origin_edge_slots, cancel)? {
        return Ok(finish_pair(
            a.book, b.book, &a.stats, &b.stats, a.params, alignment,
        ));
    }

    let mut b_values = BTreeSet::new();
    for (position, &a_slot) in origin_edge_slots.iter().enumerate() {
        check_stream_cancel_at(position, cancel)?;
        let row = collect_reenumerated_row(a, b, a_slot, source, cancel)?;
        for edge in row {
            b_values.insert(edge.b_index);
        }
    }
    let b_values = b_values.into_iter().collect::<Vec<_>>();
    check_stream_cancel(cancel)?;
    if b_values.is_empty() {
        return Ok(finish_pair(
            a.book,
            b.book,
            &a.stats,
            &b.stats,
            a.params,
            Vec::new(),
        ));
    }

    let block_rows = integer_sqrt_ceil(origin_edge_slots.len()).max(1);
    let mut tree = vec![StreamAlignmentScore::default(); b_values.len() + 1];
    let mut checkpoints = Vec::with_capacity(origin_edge_slots.len().div_ceil(block_rows));
    for (row_position, &a_slot) in origin_edge_slots.iter().enumerate() {
        check_stream_cancel_at(row_position, cancel)?;
        if row_position % block_rows == 0 {
            checkpoints.push(StreamCheckpoint {
                row_position,
                tree: copy_scores_checked(&tree, cancel)?.into_boxed_slice(),
            });
        }
        apply_stream_row(a, b, a_slot, source, cancel, &b_values, &mut tree, None)?;
    }
    let best = stream_fenwick_query(&tree, b_values.len());
    let alignment = restore_stream_alignment(
        a,
        b,
        origin_edge_slots,
        source,
        cancel,
        &b_values,
        block_rows,
        &checkpoints,
        best.tail,
    )?;
    Ok(finish_pair(
        a.book, b.book, &a.stats, &b.stats, a.params, alignment,
    ))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct StreamEdgeKey {
    a_index: u32,
    b_index: u32,
}

#[derive(Clone, Copy, Debug)]
struct StreamRowEdge {
    b_index: u32,
    distance: u32,
}

#[derive(Clone, Copy, Debug, Default)]
struct StreamAlignmentScore {
    matched: u32,
    distance_sum: u64,
    tail: Option<StreamEdgeKey>,
}

struct StreamCheckpoint {
    row_position: usize,
    tree: Box<[StreamAlignmentScore]>,
}

fn validate_origin_edge_slots<E>(
    a: &PreparedBookSide,
    slots: &[usize],
) -> Result<(), ReenumeratedPairError<E>> {
    let mut previous = None;
    for &slot in slots {
        let Some(page) = a.pages.get(slot) else {
            return Err(ReenumeratedPairError::InvalidOriginSlot(slot));
        };
        if previous == Some(slot) {
            return Err(ReenumeratedPairError::DuplicateOriginSlot(slot));
        }
        if previous.is_some_and(|previous| previous > slot) {
            return Err(ReenumeratedPairError::InvalidOriginSlot(slot));
        }
        if page.quality < a.params.min_quality || page.common {
            return Err(ReenumeratedPairError::ExcludedOriginSlot(slot));
        }
        previous = Some(slot);
    }
    Ok(())
}

fn exact_diagonal_alignment<E>(
    a: &PreparedBookSide,
    b: &PreparedBookSide,
    origin_edge_slots: &[usize],
    cancel: &AtomicBool,
) -> Result<Option<Vec<(u32, u32)>>, ReenumeratedPairError<E>> {
    if origin_edge_slots != a.distinctive_slots.as_ref()
        || a.distinctive_slots.len() != b.distinctive_slots.len()
    {
        return Ok(None);
    }
    let mut alignment = Vec::with_capacity(a.distinctive_slots.len());
    for (position, (&a_slot, &b_slot)) in a
        .distinctive_slots
        .iter()
        .zip(b.distinctive_slots.iter())
        .enumerate()
    {
        check_stream_cancel_at(position, cancel)?;
        let a_page = &a.pages[a_slot];
        let b_page = &b.pages[b_slot];
        if hamming_bytes(&a_page.signature, &b_page.signature) > a.params.radius {
            return Ok(None);
        }
        alignment.push((a_page.index, b_page.index));
    }
    Ok(Some(alignment))
}

fn collect_reenumerated_row<S>(
    a: &PreparedBookSide,
    b: &PreparedBookSide,
    a_slot: usize,
    source: &mut S,
    cancel: &AtomicBool,
) -> Result<Vec<StreamRowEdge>, ReenumeratedPairError<S::Error>>
where
    S: ReenumeratedBookEdges,
{
    check_stream_cancel(cancel)?;
    let mut edges = Vec::new();
    let mut invalid = None;
    let mut visited = 0usize;
    let mut cancelled_in_visitor = false;
    let mut visitor = |b_slot: usize, distance: u32| {
        if visited % 1024 == 0 && cancel.load(Ordering::Acquire) {
            cancelled_in_visitor = true;
            return EdgeVisitControl::Stop;
        }
        visited += 1;
        let Some(page) = b.pages.get(b_slot) else {
            invalid = Some(ReenumeratedPairError::InvalidCandidateSlot { a_slot, b_slot });
            return EdgeVisitControl::Stop;
        };
        if distance > a.params.radius {
            invalid = Some(ReenumeratedPairError::DistanceExceedsRadius {
                distance,
                radius: a.params.radius,
            });
            return EdgeVisitControl::Stop;
        }
        if page.quality >= b.params.min_quality && !page.common {
            edges.push(StreamRowEdge {
                b_index: page.index,
                distance,
            });
        }
        EdgeVisitControl::Continue
    };
    let completion = source
        .visit_a(a_slot, &mut visitor)
        .map_err(ReenumeratedPairError::Source)?;
    if cancelled_in_visitor || cancel.load(Ordering::Acquire) {
        return Err(ReenumeratedPairError::Cancelled);
    }
    if let Some(error) = invalid {
        return Err(error);
    }
    if completion != EdgeVisitCompletion::Exhausted {
        return Err(ReenumeratedPairError::SourceStopped);
    }
    check_stream_cancel(cancel)?;
    edges.sort_by_key(|edge| (edge.b_index, edge.distance));
    edges.dedup_by_key(|edge| edge.b_index);
    Ok(edges)
}

fn apply_stream_row<S>(
    a: &PreparedBookSide,
    b: &PreparedBookSide,
    a_slot: usize,
    source: &mut S,
    cancel: &AtomicBool,
    b_values: &[u32],
    tree: &mut [StreamAlignmentScore],
    mut parents: Option<&mut HashMap<StreamEdgeKey, Option<StreamEdgeKey>>>,
) -> Result<(), ReenumeratedPairError<S::Error>>
where
    S: ReenumeratedBookEdges,
{
    let a_index = a.pages[a_slot].index;
    let edges = collect_reenumerated_row(a, b, a_slot, source, cancel)?;
    let mut updates = Vec::with_capacity(edges.len());
    for (position, edge) in edges.into_iter().enumerate() {
        check_stream_cancel_at(position, cancel)?;
        let b_position = b_values
            .binary_search(&edge.b_index)
            .map_err(|_| ReenumeratedPairError::ReplayInvariant)?;
        let previous = stream_fenwick_query(tree, b_position);
        let key = StreamEdgeKey {
            a_index,
            b_index: edge.b_index,
        };
        if let Some(parents) = parents.as_deref_mut() {
            parents.insert(key, previous.tail);
        }
        updates.push((
            b_position + 1,
            StreamAlignmentScore {
                matched: previous
                    .matched
                    .checked_add(1)
                    .ok_or(ReenumeratedPairError::ArithmeticOverflow)?,
                distance_sum: previous
                    .distance_sum
                    .checked_add(edge.distance as u64)
                    .ok_or(ReenumeratedPairError::ArithmeticOverflow)?,
                tail: Some(key),
            },
        ));
    }
    for (position, (b_position, score)) in updates.into_iter().enumerate() {
        check_stream_cancel_at(position, cancel)?;
        stream_fenwick_update(tree, b_position, score);
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn restore_stream_alignment<S>(
    a: &PreparedBookSide,
    b: &PreparedBookSide,
    origin_edge_slots: &[usize],
    source: &mut S,
    cancel: &AtomicBool,
    b_values: &[u32],
    block_rows: usize,
    checkpoints: &[StreamCheckpoint],
    mut cursor: Option<StreamEdgeKey>,
) -> Result<Vec<(u32, u32)>, ReenumeratedPairError<S::Error>>
where
    S: ReenumeratedBookEdges,
{
    let mut reverse = Vec::new();
    let mut previous_block = None;
    while let Some(mut target) = cursor {
        check_stream_cancel(cancel)?;
        let row_position = origin_edge_slots
            .binary_search_by_key(&target.a_index, |&slot| a.pages[slot].index)
            .map_err(|_| ReenumeratedPairError::ReplayInvariant)?;
        let block = row_position / block_rows;
        if previous_block.is_some_and(|previous| block >= previous) {
            return Err(ReenumeratedPairError::ReplayInvariant);
        }
        previous_block = Some(block);
        let checkpoint = checkpoints
            .get(block)
            .filter(|checkpoint| checkpoint.row_position == block * block_rows)
            .ok_or(ReenumeratedPairError::ReplayInvariant)?;
        let mut tree = copy_scores_checked(&checkpoint.tree, cancel)?;
        let mut parents = HashMap::new();
        for (offset, &a_slot) in origin_edge_slots[checkpoint.row_position..=row_position]
            .iter()
            .enumerate()
        {
            check_stream_cancel_at(offset, cancel)?;
            apply_stream_row(
                a,
                b,
                a_slot,
                source,
                cancel,
                b_values,
                &mut tree,
                Some(&mut parents),
            )?;
        }

        let block_start_a = a.pages[origin_edge_slots[checkpoint.row_position]].index;
        loop {
            reverse.push((target.a_index, target.b_index));
            cursor = *parents
                .get(&target)
                .ok_or(ReenumeratedPairError::ReplayInvariant)?;
            let Some(parent) = cursor else {
                break;
            };
            if parent.a_index >= target.a_index {
                return Err(ReenumeratedPairError::ReplayInvariant);
            }
            if parent.a_index < block_start_a {
                break;
            }
            target = parent;
        }
    }
    reverse.reverse();
    Ok(reverse)
}

fn copy_scores_checked<E>(
    scores: &[StreamAlignmentScore],
    cancel: &AtomicBool,
) -> Result<Vec<StreamAlignmentScore>, ReenumeratedPairError<E>> {
    let mut copy = Vec::with_capacity(scores.len());
    for (position, score) in scores.iter().copied().enumerate() {
        check_stream_cancel_at(position, cancel)?;
        copy.push(score);
    }
    Ok(copy)
}

fn integer_sqrt_ceil(value: usize) -> usize {
    let mut root = 1usize;
    while root.saturating_mul(root) < value {
        root += 1;
    }
    root
}

fn stream_score_is_better(candidate: StreamAlignmentScore, current: StreamAlignmentScore) -> bool {
    candidate.matched > current.matched
        || (candidate.matched == current.matched && candidate.distance_sum < current.distance_sum)
}

fn stream_fenwick_query(tree: &[StreamAlignmentScore], mut end: usize) -> StreamAlignmentScore {
    let mut best = StreamAlignmentScore::default();
    while end > 0 {
        if stream_score_is_better(tree[end], best) {
            best = tree[end];
        }
        end &= end - 1;
    }
    best
}

fn stream_fenwick_update(
    tree: &mut [StreamAlignmentScore],
    mut position: usize,
    score: StreamAlignmentScore,
) {
    while position < tree.len() {
        if stream_score_is_better(score, tree[position]) {
            tree[position] = score;
        }
        position += position & position.wrapping_neg();
    }
}

fn check_stream_cancel<E>(cancel: &AtomicBool) -> Result<(), ReenumeratedPairError<E>> {
    if cancel.load(Ordering::Acquire) {
        Err(ReenumeratedPairError::Cancelled)
    } else {
        Ok(())
    }
}

fn check_stream_cancel_at<E>(
    position: usize,
    cancel: &AtomicBool,
) -> Result<(), ReenumeratedPairError<E>> {
    if position % 1024 == 0 {
        check_stream_cancel(cancel)
    } else {
        Ok(())
    }
}

fn exact_near_pairs(
    pages: &[PreparedPage<'_>],
    eligible: &[usize],
    radius: u32,
    bit_width: u32,
) -> Vec<NearPair> {
    if eligible.len() < 2 {
        return Vec::new();
    }
    let mut candidate_pairs = HashSet::<(usize, usize)>::new();
    if radius >= bit_width {
        for (position, &left) in eligible.iter().enumerate() {
            for &right in &eligible[position + 1..] {
                candidate_pairs.insert((left.min(right), left.max(right)));
            }
        }
    } else {
        let block_count = radius as usize + 1;
        for block in 0..block_count {
            let start = block * bit_width as usize / block_count;
            let end = (block + 1) * bit_width as usize / block_count;
            let mut buckets = HashMap::<Vec<u8>, Vec<usize>>::new();
            for &page_index in eligible {
                buckets
                    .entry(bit_block(pages[page_index].bits, start, end))
                    .or_default()
                    .push(page_index);
            }
            for bucket in buckets.values() {
                for left_position in 0..bucket.len() {
                    for right_position in left_position + 1..bucket.len() {
                        let left = bucket[left_position].min(bucket[right_position]);
                        let right = bucket[left_position].max(bucket[right_position]);
                        candidate_pairs.insert((left, right));
                    }
                }
            }
        }
    }

    let mut near_pairs = candidate_pairs
        .into_iter()
        .filter_map(|(left, right)| {
            let distance = hamming_bytes(pages[left].bits, pages[right].bits);
            (distance <= radius).then_some(NearPair {
                left,
                right,
                distance,
            })
        })
        .collect::<Vec<_>>();
    near_pairs.sort_by_key(|pair| (pair.left, pair.right));
    near_pairs
}

fn bit_block(bytes: &[u8], start: usize, end: usize) -> Vec<u8> {
    let mut block = vec![0u8; (end - start).div_ceil(8)];
    for source_bit in start..end {
        let source_mask = 1 << (7 - source_bit % 8);
        if bytes[source_bit / 8] & source_mask != 0 {
            let target_bit = source_bit - start;
            block[target_bit / 8] |= 1 << (7 - target_bit % 8);
        }
    }
    block
}

fn hamming_bytes(a: &[u8], b: &[u8]) -> u32 {
    a.iter()
        .zip(b)
        .map(|(left, right)| (left ^ right).count_ones())
        .sum()
}

#[derive(Clone, Copy)]
struct AlignmentCandidate {
    index_a: u32,
    index_b: u32,
    distance: u32,
}

#[derive(Clone, Copy, Default)]
struct AlignmentScore {
    matched: u32,
    distance_sum: u64,
    tail: Option<usize>,
}

fn score_is_better(candidate: AlignmentScore, current: AlignmentScore) -> bool {
    candidate.matched > current.matched
        || (candidate.matched == current.matched && candidate.distance_sum < current.distance_sum)
}

fn weighted_monotonic_alignment(mut candidates: Vec<AlignmentCandidate>) -> Vec<(u32, u32)> {
    if candidates.is_empty() {
        return Vec::new();
    }
    candidates.sort_by_key(|candidate| (candidate.index_a, candidate.index_b, candidate.distance));
    candidates.dedup_by_key(|candidate| (candidate.index_a, candidate.index_b));
    let mut b_values = candidates
        .iter()
        .map(|candidate| candidate.index_b)
        .collect::<Vec<_>>();
    b_values.sort_unstable();
    b_values.dedup();

    let mut tree = vec![AlignmentScore::default(); b_values.len() + 1];
    let mut predecessor = vec![None; candidates.len()];
    let mut candidate_index = 0usize;
    while candidate_index < candidates.len() {
        let group_start = candidate_index;
        let index_a = candidates[group_start].index_a;
        while candidate_index < candidates.len() && candidates[candidate_index].index_a == index_a {
            candidate_index += 1;
        }

        let mut updates = Vec::with_capacity(candidate_index - group_start);
        for position in group_start..candidate_index {
            let candidate = candidates[position];
            let b_position = b_values
                .binary_search(&candidate.index_b)
                .expect("candidate b index is present");
            let previous = fenwick_query(&tree, b_position);
            predecessor[position] = previous.tail;
            let score = AlignmentScore {
                matched: previous.matched + 1,
                distance_sum: previous.distance_sum + candidate.distance as u64,
                tail: Some(position),
            };
            updates.push((b_position + 1, score));
        }
        for (position, score) in updates {
            fenwick_update(&mut tree, position, score);
        }
    }

    let best = fenwick_query(&tree, b_values.len());
    let mut alignment = Vec::with_capacity(best.matched as usize);
    let mut cursor = best.tail;
    while let Some(position) = cursor {
        let candidate = candidates[position];
        alignment.push((candidate.index_a, candidate.index_b));
        cursor = predecessor[position];
    }
    alignment.reverse();
    alignment
}

fn fenwick_query(tree: &[AlignmentScore], mut end: usize) -> AlignmentScore {
    let mut best = AlignmentScore::default();
    while end > 0 {
        if score_is_better(tree[end], best) {
            best = tree[end];
        }
        end &= end - 1;
    }
    best
}

fn fenwick_update(tree: &mut [AlignmentScore], mut position: usize, score: AlignmentScore) {
    while position < tree.len() {
        if score_is_better(score, tree[position]) {
            tree[position] = score;
        }
        position += position & position.wrapping_neg();
    }
}

#[cfg(test)]
mod tests {
    use std::convert::Infallible;
    use std::sync::atomic::AtomicUsize;

    use super::*;

    fn params(radius: u32, max_books_per_page: u32) -> Params {
        Params {
            radius,
            max_books_per_page,
            min_quality: 1,
            coverage_threshold: 0.9,
            min_matched_pages: 1,
        }
    }

    fn content_sig(content: u32) -> Sig {
        let mut bytes = [0u8; 32];
        let mut state = content as u64 ^ 0x9e37_79b9_7f4a_7c15;
        for byte in &mut bytes {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            *byte = state as u8;
        }
        bytes[..4].copy_from_slice(&content.to_be_bytes());
        Sig::Bits(Box::new(bytes))
    }

    fn page(book: u32, index: u32, content: u32) -> BookPage {
        BookPage {
            book,
            index,
            quality: 100,
            sig: content_sig(content),
        }
    }

    fn page_with(book: u32, index: u32, quality: u8, bytes: &[u8]) -> BookPage {
        BookPage {
            book,
            index,
            quality,
            sig: Sig::Bits(bytes.to_vec().into_boxed_slice()),
        }
    }

    fn fixed_content_sig(content: u32) -> [u8; 32] {
        match content_sig(content) {
            Sig::Bits(bits) => bits.as_ref().try_into().unwrap(),
            Sig::Luma(_) => unreachable!(),
        }
    }

    fn prepared_side(book: u32, contents: &[u32], params: Params) -> PreparedBookSide {
        PreparedBookSide::new(
            book,
            contents
                .iter()
                .enumerate()
                .map(|(index, &content)| VerifiedBookPage {
                    index: index as u32,
                    quality: 100,
                    common: false,
                    signature: fixed_content_sig(content),
                })
                .collect(),
            params,
        )
        .unwrap()
    }

    struct MatrixEdgeSource {
        rows: Vec<Vec<(usize, u32)>>,
        calls: Vec<usize>,
        fail_on: Option<usize>,
        stop_on: Option<usize>,
    }

    impl MatrixEdgeSource {
        fn from_sides(a: &PreparedBookSide, b: &PreparedBookSide) -> Self {
            let rows = a
                .pages
                .iter()
                .map(|a_page| {
                    b.pages
                        .iter()
                        .enumerate()
                        .filter_map(|(b_slot, b_page)| {
                            let distance = hamming_bytes(&a_page.signature, &b_page.signature);
                            (distance <= a.params.radius).then_some((b_slot, distance))
                        })
                        .collect()
                })
                .collect();
            Self {
                rows,
                calls: Vec::new(),
                fail_on: None,
                stop_on: None,
            }
        }

        fn domain(&self, a: &PreparedBookSide) -> Vec<usize> {
            self.rows
                .iter()
                .enumerate()
                .filter_map(|(slot, edges)| {
                    (!edges.is_empty()
                        && a.pages[slot].quality >= a.params.min_quality
                        && !a.pages[slot].common)
                        .then_some(slot)
                })
                .collect()
        }
    }

    impl ReenumeratedBookEdges for MatrixEdgeSource {
        type Error = &'static str;

        fn visit_a(
            &mut self,
            a_slot: usize,
            visitor: &mut dyn FnMut(usize, u32) -> EdgeVisitControl,
        ) -> Result<EdgeVisitCompletion, Self::Error> {
            self.calls.push(a_slot);
            if self.fail_on == Some(a_slot) {
                return Err("edge source failed");
            }
            if self.stop_on == Some(a_slot) {
                return Ok(EdgeVisitCompletion::Stopped);
            }
            for &(b_slot, distance) in self.rows.get(a_slot).into_iter().flatten() {
                if visitor(b_slot, distance) == EdgeVisitControl::Stop {
                    return Ok(EdgeVisitCompletion::Stopped);
                }
            }
            Ok(EdgeVisitCompletion::Exhausted)
        }
    }

    fn old_pair_for_sides(a: &PreparedBookSide, b: &PreparedBookSide) -> BookPair {
        let pages = a
            .pages
            .iter()
            .map(|page| BookPage {
                book: a.book,
                index: page.index,
                quality: page.quality,
                sig: Sig::Bits(Box::new(page.signature)),
            })
            .chain(b.pages.iter().map(|page| BookPage {
                book: b.book,
                index: page.index,
                quality: page.quality,
                sig: Sig::Bits(Box::new(page.signature)),
            }))
            .collect::<Vec<_>>();
        classify_pair(&pages, a.params, a.book, b.book).unwrap()
    }

    fn streamed_pair_for_sides(
        a: &PreparedBookSide,
        b: &PreparedBookSide,
    ) -> (BookPair, MatrixEdgeSource) {
        let mut source = MatrixEdgeSource::from_sides(a, b);
        let domain = source.domain(a);
        let pair = classify_pair_reenumerated(a, b, &domain, &mut source, &AtomicBool::new(false))
            .unwrap();
        (pair, source)
    }

    #[test]
    fn reenumerated_classifier_matches_existing_pair_fields() {
        let params = params(0, 8);
        let a = prepared_side(1, &[10, 20, 30], params);
        let b = prepared_side(2, &[10, 20, 40, 50], params);
        let expected = old_pair_for_sides(&a, &b);
        let (actual, source) = streamed_pair_for_sides(&a, &b);
        assert_eq!(actual, expected);
        assert!(source.calls.iter().all(|slot| [0, 1].contains(slot)));
    }

    #[test]
    fn full_diagonal_shortcut_precedes_edge_source_enumeration() {
        let params = params(0, 8);
        let a = prepared_side(1, &[1, 2, 3, 4], params);
        let b = prepared_side(2, &[1, 2, 3, 4], params);
        let mut source = MatrixEdgeSource::from_sides(&a, &b);
        source.fail_on = Some(0);
        let domain = vec![0, 1, 2, 3];
        let pair =
            classify_pair_reenumerated(&a, &b, &domain, &mut source, &AtomicBool::new(false))
                .unwrap();
        assert_eq!(pair, old_pair_for_sides(&a, &b));
        assert!(source.calls.is_empty());
    }

    #[test]
    fn one_by_repeated_pages_preserves_existing_fenwick_ties() {
        let params = params(0, 8);
        for (b_len, expected_b) in [(3, 2), (4, 0)] {
            let a = prepared_side(1, &[7], params);
            let b = prepared_side(2, &vec![7; b_len], params);
            let expected = old_pair_for_sides(&a, &b);
            let (actual, _) = streamed_pair_for_sides(&a, &b);
            assert_eq!(actual, expected);
            assert_eq!(actual.alignment, vec![(0, expected_b)]);
        }
    }

    #[test]
    fn checkpoint_replay_matches_dense_nonshortcut_oracle() {
        let params = params(0, 8);
        let mut a_contents = vec![7; 15];
        a_contents.push(9);
        let mut b_contents = vec![9];
        b_contents.extend(vec![7; 15]);
        let a = prepared_side(1, &a_contents, params);
        let b = prepared_side(2, &b_contents, params);
        let expected = old_pair_for_sides(&a, &b);
        let (actual, source) = streamed_pair_for_sides(&a, &b);
        assert_eq!(actual, expected);
        assert!(source.calls.len() > a.pages.len() * 2);
    }

    #[test]
    fn checkpoint_replay_crosses_blocks_and_ignores_b_pages_without_edges() {
        let params = params(0, 8);
        let a = prepared_side(1, &[1, 2, 3, 4, 5, 6, 7, 8, 9], params);
        let b = prepared_side(2, &[1, 90, 2, 3, 4, 91, 5, 6, 7, 8, 9, 92], params);
        let expected = old_pair_for_sides(&a, &b);
        let (actual, _) = streamed_pair_for_sides(&a, &b);
        assert_eq!(actual, expected);
        assert_eq!(actual.alignment.first(), Some(&(0, 0)));
        assert_eq!(actual.alignment.last(), Some(&(8, 10)));
    }

    #[test]
    fn sparse_same_length_pair_enumerates_only_its_complete_domain() {
        let params = params(0, 8);
        let a = prepared_side(1, &[1, 2, 3, 4], params);
        let b = prepared_side(2, &[1, 20, 30, 40], params);
        let expected = old_pair_for_sides(&a, &b);
        let (actual, source) = streamed_pair_for_sides(&a, &b);
        assert_eq!(actual, expected);
        assert!(!source.calls.is_empty());
        assert!(source.calls.iter().all(|&slot| slot == 0));
    }

    #[test]
    fn weighted_stream_prefers_lower_total_distance() {
        let params = params(32, 8);
        let a = prepared_side(1, &[1, 2], params);
        let b = prepared_side(2, &[3, 4, 5], params);
        let mut source = MatrixEdgeSource {
            rows: vec![vec![(0, 12), (1, 1)], vec![(2, 10)]],
            calls: Vec::new(),
            fail_on: None,
            stop_on: None,
        };
        let pair =
            classify_pair_reenumerated(&a, &b, &[0, 1], &mut source, &AtomicBool::new(false))
                .unwrap();
        assert_eq!(pair.alignment, [(0, 1), (1, 2)]);
    }

    #[test]
    fn cancellation_inside_a_large_edge_callback_beats_source_stopped() {
        struct CancellingSource<'a> {
            cancel: &'a AtomicBool,
            visited: &'a AtomicUsize,
            observed_stop: &'a AtomicBool,
        }

        impl ReenumeratedBookEdges for CancellingSource<'_> {
            type Error = Infallible;

            fn visit_a(
                &mut self,
                _a_slot: usize,
                visitor: &mut dyn FnMut(usize, u32) -> EdgeVisitControl,
            ) -> Result<EdgeVisitCompletion, Self::Error> {
                for edge in 0..2048 {
                    if edge == 1024 {
                        self.cancel.store(true, Ordering::Release);
                    }
                    self.visited.fetch_add(1, Ordering::Relaxed);
                    if visitor(0, 0) == EdgeVisitControl::Stop {
                        self.observed_stop.store(true, Ordering::Release);
                        return Ok(EdgeVisitCompletion::Stopped);
                    }
                }
                Ok(EdgeVisitCompletion::Exhausted)
            }
        }

        let params = params(0, 8);
        let a = prepared_side(1, &[1], params);
        let b = prepared_side(2, &[1, 2], params);
        let cancel = AtomicBool::new(false);
        let visited = AtomicUsize::new(0);
        let observed_stop = AtomicBool::new(false);
        let mut source = CancellingSource {
            cancel: &cancel,
            visited: &visited,
            observed_stop: &observed_stop,
        };
        assert_eq!(
            classify_pair_reenumerated(&a, &b, &[0], &mut source, &cancel),
            Err(ReenumeratedPairError::Cancelled)
        );
        assert!(observed_stop.load(Ordering::Acquire));
        assert_eq!(visited.load(Ordering::Relaxed), 1025);
    }

    #[test]
    fn one_row_deduplicates_repeated_edges_at_minimum_distance() {
        let params = params(32, 8);
        let a = prepared_side(1, &[1], params);
        let b = prepared_side(2, &[2, 3], params);
        let mut source = MatrixEdgeSource {
            rows: vec![vec![(0, 12), (0, 3), (0, 8), (1, 9)]],
            calls: Vec::new(),
            fail_on: None,
            stop_on: None,
        };
        let row =
            collect_reenumerated_row(&a, &b, 0, &mut source, &AtomicBool::new(false)).unwrap();
        assert_eq!(
            row.iter()
                .map(|edge| (edge.b_index, edge.distance))
                .collect::<Vec<_>>(),
            [(0, 3), (1, 9)]
        );
    }

    #[test]
    fn source_failure_stop_cancel_and_invalid_edges_remain_distinct() {
        let params = params(0, 8);
        let a = prepared_side(1, &[1], params);
        let b = prepared_side(2, &[2, 3], params);

        let mut failed = MatrixEdgeSource {
            rows: vec![vec![]],
            calls: Vec::new(),
            fail_on: Some(0),
            stop_on: None,
        };
        assert_eq!(
            classify_pair_reenumerated(&a, &b, &[0], &mut failed, &AtomicBool::new(false)),
            Err(ReenumeratedPairError::Source("edge source failed"))
        );

        let mut stopped = MatrixEdgeSource {
            rows: vec![vec![]],
            calls: Vec::new(),
            fail_on: None,
            stop_on: Some(0),
        };
        assert_eq!(
            classify_pair_reenumerated(&a, &b, &[0], &mut stopped, &AtomicBool::new(false)),
            Err(ReenumeratedPairError::SourceStopped)
        );

        let mut cancelled_source = MatrixEdgeSource::from_sides(&a, &b);
        assert_eq!(
            classify_pair_reenumerated(&a, &b, &[], &mut cancelled_source, &AtomicBool::new(true)),
            Err(ReenumeratedPairError::Cancelled)
        );

        let mut invalid = MatrixEdgeSource {
            rows: vec![vec![(9, 0)]],
            calls: Vec::new(),
            fail_on: None,
            stop_on: None,
        };
        assert_eq!(
            classify_pair_reenumerated(&a, &b, &[0], &mut invalid, &AtomicBool::new(false)),
            Err(ReenumeratedPairError::InvalidCandidateSlot {
                a_slot: 0,
                b_slot: 9,
            })
        );
    }

    #[test]
    fn prepared_side_binds_params_stats_and_sorted_slots_once() {
        let params = params(32, 8);
        let side = PreparedBookSide::new(
            4,
            vec![
                VerifiedBookPage {
                    index: 9,
                    quality: 0,
                    common: false,
                    signature: [0; 32],
                },
                VerifiedBookPage {
                    index: 2,
                    quality: 100,
                    common: true,
                    signature: [0; 32],
                },
                VerifiedBookPage {
                    index: 6,
                    quality: 100,
                    common: false,
                    signature: [0; 32],
                },
            ],
            params,
        )
        .unwrap();
        assert_eq!(side.book(), 4);
        assert_eq!(
            side.pages()
                .iter()
                .map(|page| page.index)
                .collect::<Vec<_>>(),
            [2, 6, 9]
        );
        assert_eq!(side.distinctive_slots(), [1]);
        assert_eq!(
            side.stats(),
            &BookStats {
                book: 4,
                total_pages: 3,
                distinctive_pages: 1,
                featureless_pages: 1,
                common_pages: 1,
            }
        );
        assert_eq!(
            PreparedBookSide::new(
                4,
                vec![VerifiedBookPage {
                    index: 0,
                    quality: 1,
                    common: false,
                    signature: [0; 32],
                }],
                Params {
                    radius: 257,
                    ..params
                },
            )
            .unwrap_err(),
            AnalyzeError::RadiusExceedsBitWidth {
                radius: 257,
                bit_width: 256,
            }
        );
    }

    #[test]
    fn defaults_are_measurement_parameters_not_hidden_rules() {
        assert_eq!(
            Params::default(),
            Params {
                radius: 8,
                max_books_per_page: 8,
                min_quality: 1,
                coverage_threshold: 0.5,
                min_matched_pages: 3,
            }
        );
    }

    #[test]
    fn one_matching_page_in_a_two_page_book_needs_a_lowered_match_floor() {
        let mut pages = vec![page(1, 0, 7), page(1, 1, 8)];
        for index in 0..10 {
            let content = if index == 0 { 7 } else { 100 + index };
            pages.push(page(2, index, content));
        }

        let default_pair = classify_pair(&pages, Params::default(), 1, 2).unwrap();
        assert_eq!(default_pair.matched, 1);
        assert_eq!(default_pair.coverage_a, 0.5);
        assert_eq!(default_pair.coverage_b, 0.1);
        assert_eq!(default_pair.relation, Relation::Unrelated);

        let lowered_floor = classify_pair(
            &pages,
            Params {
                min_matched_pages: 1,
                ..Params::default()
            },
            1,
            2,
        )
        .unwrap();
        assert_eq!(lowered_floor.matched, 1);
        assert_eq!(lowered_floor.relation, Relation::Contains { whole: 2 });
    }

    #[test]
    fn split_books_are_each_contained_by_the_integrated_book() {
        let mut pages = Vec::new();
        for index in 0..100 {
            pages.push(page(1, index, index));
            pages.push(page(3, index, index));
        }
        for index in 0..100 {
            pages.push(page(2, index, 100 + index));
            pages.push(page(3, 100 + index, 100 + index));
        }

        let analysis = analyze(&pages, params(0, 8)).unwrap();
        assert_eq!(analysis.pairs.len(), 2);
        assert_eq!(analysis.pairs[0].relation, Relation::Contains { whole: 3 });
        assert_eq!(analysis.pairs[1].relation, Relation::Contains { whole: 3 });
        assert_eq!(analysis.pairs[0].matched, 100);
        assert_eq!(analysis.pairs[1].matched, 100);
    }

    #[test]
    fn appended_common_credit_page_does_not_lower_coverage() {
        let mut pages = Vec::new();
        for index in 0..20 {
            pages.push(page(1, index, index));
            pages.push(page(2, index, index));
        }
        for book in 2..=4 {
            pages.push(page(book, 100, 999));
        }

        let pair = classify_pair(&pages, params(0, 2), 1, 2).unwrap();
        assert_eq!(pair.relation, Relation::Same);
        assert_eq!(pair.matched, 20);
        assert_eq!(pair.distinctive_a, 20);
        assert_eq!(pair.distinctive_b, 20);
        assert_eq!(pair.coverage_a, 1.0);
        assert_eq!(pair.coverage_b, 1.0);
    }

    #[test]
    fn credit_inserted_at_front_preserves_monotonic_same_relation() {
        let mut pages = vec![BookPage {
            book: 2,
            index: 0,
            quality: 0,
            sig: content_sig(999),
        }];
        for index in 0..20 {
            pages.push(page(1, index, index));
            pages.push(page(2, index + 1, index));
        }

        let pair = classify_pair(&pages, params(0, 8), 1, 2).unwrap();
        assert_eq!(pair.relation, Relation::Same);
        assert_eq!(pair.alignment.first(), Some(&(0, 1)));
        assert_eq!(pair.alignment.last(), Some(&(19, 20)));
    }

    #[test]
    fn books_sharing_only_a_common_credit_page_are_unrelated() {
        let mut pages = vec![page(1, 0, 10), page(2, 0, 20)];
        for book in 1..=3 {
            pages.push(page(book, 100, 999));
        }

        let pair = classify_pair(&pages, params(0, 2), 1, 2).unwrap();
        assert_eq!(pair.relation, Relation::Unrelated);
        assert_eq!(pair.matched, 0);
        assert_eq!(pair.distinctive_a, 1);
        assert_eq!(pair.distinctive_b, 1);
    }

    #[test]
    fn books_without_distinctive_pages_are_undecidable() {
        let pages = vec![
            BookPage {
                book: 1,
                index: 0,
                quality: 0,
                sig: content_sig(1),
            },
            BookPage {
                book: 2,
                index: 0,
                quality: 0,
                sig: content_sig(1),
            },
        ];

        let pair = classify_pair(&pages, params(0, 8), 1, 2).unwrap();
        assert_eq!(pair.relation, Relation::Undecidable);
        assert_eq!(pair.coverage_a, 0.0);
        assert_eq!(pair.coverage_b, 0.0);
    }

    #[test]
    fn reordered_pages_are_not_same() {
        let mut pages = Vec::new();
        for index in 0..20 {
            pages.push(page(1, index, index));
            pages.push(page(2, 19 - index, index));
        }

        let pair = classify_pair(&pages, params(0, 8), 1, 2).unwrap();
        assert_eq!(pair.relation, Relation::Unrelated);
        assert_eq!(pair.matched, 1);
    }

    #[test]
    fn common_pages_leave_both_numerator_and_denominator() {
        let mut pages = vec![
            page(1, 0, 1),
            page(1, 1, 2),
            page(1, 2, 999),
            page(2, 0, 1),
            page(2, 1, 2),
            page(2, 2, 999),
            page(3, 0, 999),
        ];
        pages.push(page(3, 1, 1000));

        let pair = classify_pair(&pages, params(0, 2), 1, 2).unwrap();
        assert_eq!(pair.relation, Relation::Same);
        assert_eq!(pair.matched, 2);
        assert_eq!(pair.distinctive_a, 2);
        assert_eq!(pair.distinctive_b, 2);
    }

    #[test]
    fn direct_neighborhood_count_does_not_grow_by_single_linkage() {
        let pages = vec![
            page_with(1, 0, 100, &[0b0000_0000]),
            page_with(2, 0, 100, &[0b0000_0001]),
            page_with(3, 0, 100, &[0b0000_0011]),
        ];

        let analysis = analyze(&pages, params(1, 2)).unwrap();
        let stats = analysis
            .books
            .iter()
            .map(|stats| (stats.book, stats.clone()))
            .collect::<BTreeMap<_, _>>();
        assert_eq!(stats[&1].common_pages, 0);
        assert_eq!(stats[&1].distinctive_pages, 1);
        assert_eq!(stats[&2].common_pages, 1);
        assert_eq!(stats[&2].distinctive_pages, 0);
        assert_eq!(stats[&3].common_pages, 0);
        assert_eq!(stats[&3].distinctive_pages, 1);
    }
}
