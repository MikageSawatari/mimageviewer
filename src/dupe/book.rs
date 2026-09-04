//! Book-level relation analysis over caller-supplied page signatures.
//!
//! This module owns no paths, decoders, PDF state, persistence, or UI. A caller
//! supplies one bit signature per page and decides every measurement parameter.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fmt;

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
        if !params.coverage_threshold.is_finite()
            || !(0.0..=1.0).contains(&params.coverage_threshold)
        {
            return Err(AnalyzeError::InvalidCoverage);
        }

        let mut seen_page_indices = HashSet::new();
        let mut expected_bytes = None;
        let mut prepared = Vec::with_capacity(pages.len());
        let mut stats = BTreeMap::<u32, BookStats>::new();
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
            let book_stats = stats.entry(page.book).or_insert(BookStats {
                book: page.book,
                total_pages: 0,
                distinctive_pages: 0,
                featureless_pages: 0,
                common_pages: 0,
            });
            book_stats.total_pages += 1;
            let featureless = page.quality < params.min_quality;
            if featureless {
                book_stats.featureless_pages += 1;
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
            let book_stats = stats
                .get_mut(&prepared[index].page.book)
                .expect("every prepared page has book stats");
            if is_common {
                book_stats.common_pages += 1;
            } else {
                book_stats.distinctive_pages += 1;
            }
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
            return Ok(BookPair {
                a,
                b,
                matched: 0,
                distinctive_a: stats_a.distinctive_pages,
                distinctive_b: stats_b.distinctive_pages,
                coverage_a: 0.0,
                coverage_b: 0.0,
                relation: Relation::Undecidable,
                alignment: Vec::new(),
            });
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
        let matched = alignment.len() as u32;
        let coverage_a = matched as f32 / stats_a.distinctive_pages as f32;
        let coverage_b = matched as f32 / stats_b.distinctive_pages as f32;
        let covers_a = coverage_a >= self.params.coverage_threshold;
        let covers_b = coverage_b >= self.params.coverage_threshold;
        let relation = if matched < self.params.min_matched_pages {
            Relation::Unrelated
        } else {
            match (covers_a, covers_b) {
                (true, true) => Relation::Same,
                (true, false) => Relation::Contains { whole: b },
                (false, true) => Relation::Contains { whole: a },
                (false, false) => Relation::Unrelated,
            }
        };
        Ok(BookPair {
            a,
            b,
            matched,
            distinctive_a: stats_a.distinctive_pages,
            distinctive_b: stats_b.distinctive_pages,
            coverage_a,
            coverage_b,
            relation,
            alignment,
        })
    }
}

fn ordered_pair(a: u32, b: u32) -> (u32, u32) {
    if a < b { (a, b) } else { (b, a) }
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
