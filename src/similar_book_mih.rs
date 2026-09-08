//! Worker-local exact radius-32 MIH for book similarity queries.
//!
//! The index stores only row indices into immutable [`BaseArray`] and
//! [`SearchSnapshot`] owners. A completed base index can therefore be reused only
//! while the exact same base `Arc` remains mounted. Derived delta postings are
//! replaced as one unpublished generation.

use std::convert::Infallible;
use std::fmt;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use crate::similar_search_array::{BaseArray, SearchRecord, SearchSnapshot};

const BLOCK_COUNT: usize = 16;
const BUCKET_COUNT: usize = 1 << 16;
const SLOT_COUNT: usize = BLOCK_COUNT * BUCKET_COUNT;
const CANCEL_INTERVAL: usize = 1 << 10;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct MihHit {
    pub(crate) item_id: u64,
    pub(crate) revision: u32,
    pub(crate) distance: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MihVisitControl {
    Continue,
    Stop,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MihVisitCompletion {
    Exhausted,
    Stopped,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MihBuildError {
    Cancelled,
    TooManyRows,
    TooManyPostings,
    BucketOverflow,
    IncompatibleBase,
}

impl fmt::Display for MihBuildError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Cancelled => f.write_str("MIH build cancelled"),
            Self::TooManyRows => f.write_str("MIH row count exceeds u32 address space"),
            Self::TooManyPostings => f.write_str("MIH posting count exceeds u32 address space"),
            Self::BucketOverflow => f.write_str("MIH bucket count overflow"),
            Self::IncompatibleBase => {
                f.write_str("MIH derived snapshot does not share its base owner")
            }
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum MihQueryError<E> {
    Cancelled,
    RadiusOutOfRange(u32),
    CorruptIndex,
    Visitor(E),
}

#[derive(Debug)]
struct MihPostings {
    offsets: Box<[u32]>,
    rows: Box<[u32]>,
}

impl MihPostings {
    fn build(
        row_count: usize,
        mut signature_at: impl FnMut(usize) -> Option<[u8; 32]>,
        cancelled: &mut impl FnMut() -> bool,
    ) -> Result<Self, MihBuildError> {
        if row_count > u32::MAX as usize {
            return Err(MihBuildError::TooManyRows);
        }

        let mut counts = vec![0u32; SLOT_COUNT];
        let mut eligible = 0u32;
        for row in 0..row_count {
            check_cancelled_at(row, cancelled)?;
            let Some(signature) = signature_at(row) else {
                continue;
            };
            eligible = eligible.checked_add(1).ok_or(MihBuildError::TooManyRows)?;
            for block in 0..BLOCK_COUNT {
                let slot = posting_slot(&signature, block);
                counts[slot] = counts[slot]
                    .checked_add(1)
                    .ok_or(MihBuildError::BucketOverflow)?;
            }
        }
        if cancelled() {
            return Err(MihBuildError::Cancelled);
        }
        eligible
            .checked_mul(BLOCK_COUNT as u32)
            .ok_or(MihBuildError::TooManyPostings)?;

        let mut offsets = Vec::with_capacity(SLOT_COUNT + 1);
        offsets.push(0u32);
        for (slot, &count) in counts.iter().enumerate() {
            check_cancelled_at(slot, cancelled)?;
            offsets.push(
                offsets[slot]
                    .checked_add(count)
                    .ok_or(MihBuildError::TooManyPostings)?,
            );
        }
        if cancelled() {
            return Err(MihBuildError::Cancelled);
        }

        for (slot, cursor) in counts.iter_mut().enumerate() {
            check_cancelled_at(slot, cancelled)?;
            *cursor = offsets[slot];
        }
        if cancelled() {
            return Err(MihBuildError::Cancelled);
        }
        let posting_count = offsets.last().copied().unwrap_or(0) as usize;
        let mut rows = vec![0u32; posting_count];
        for row in 0..row_count {
            check_cancelled_at(row, cancelled)?;
            let Some(signature) = signature_at(row) else {
                continue;
            };
            for block in 0..BLOCK_COUNT {
                let slot = posting_slot(&signature, block);
                let position = counts[slot] as usize;
                let target = rows
                    .get_mut(position)
                    .ok_or(MihBuildError::TooManyPostings)?;
                *target = row as u32;
                counts[slot] = counts[slot]
                    .checked_add(1)
                    .ok_or(MihBuildError::TooManyPostings)?;
            }
        }
        if cancelled() {
            return Err(MihBuildError::Cancelled);
        }

        Ok(Self {
            offsets: offsets.into_boxed_slice(),
            rows: rows.into_boxed_slice(),
        })
    }

    fn range(&self, block: usize, value: u16) -> Option<&[u32]> {
        let slot = block
            .checked_mul(BUCKET_COUNT)?
            .checked_add(value as usize)?;
        let start = *self.offsets.get(slot)? as usize;
        let end = *self.offsets.get(slot + 1)? as usize;
        self.rows.get(start..end)
    }
}

#[derive(Debug)]
struct BaseMih {
    array: Arc<BaseArray>,
    postings: MihPostings,
}

#[derive(Debug)]
struct DerivedSnapshotMih {
    snapshot: Arc<SearchSnapshot>,
    delta_postings: MihPostings,
}

#[derive(Debug, Default)]
enum MihCacheState {
    #[default]
    Empty,
    BaseOnly(BaseMih),
    Ready {
        base: BaseMih,
        derived: DerivedSnapshotMih,
    },
}

impl MihCacheState {
    fn ready(base: BaseMih, derived: DerivedSnapshotMih) -> Result<Self, MihBuildError> {
        if !Arc::ptr_eq(&base.array, &derived.snapshot.base) {
            return Err(MihBuildError::IncompatibleBase);
        }
        Ok(Self::Ready { base, derived })
    }
}

#[derive(Debug, Default)]
struct QueryScratch {
    base_marks: Vec<u32>,
    delta_marks: Vec<u32>,
    generation: u32,
}

impl QueryScratch {
    fn discard_all(&mut self) {
        *self = Self::default();
    }

    fn discard_delta(&mut self) {
        self.delta_marks = Vec::new();
    }

    fn begin_query(
        &mut self,
        base_rows: usize,
        delta_rows: usize,
        cancelled: &mut impl FnMut() -> bool,
    ) -> Result<u32, MihQueryError<Infallible>> {
        self.base_marks.resize(base_rows, 0);
        self.delta_marks.resize(delta_rows, 0);

        if self.generation == u32::MAX {
            clear_marks_checked(&mut self.base_marks, cancelled)?;
            clear_marks_checked(&mut self.delta_marks, cancelled)?;
            if cancelled() {
                return Err(MihQueryError::Cancelled);
            }
            self.generation = 1;
        } else {
            self.generation += 1;
        }
        Ok(self.generation)
    }
}

/// One worker's single-generation exact MIH cache.
///
/// `clear` is the explicit store-change/shutdown boundary. Origin and scope
/// changes intentionally leave this derived cache available for reuse.
#[derive(Debug, Default)]
pub(crate) struct BookMihRuntime {
    cache: MihCacheState,
    scratch: QueryScratch,
}

impl BookMihRuntime {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn clear(&mut self) {
        self.cache = MihCacheState::Empty;
        self.scratch.discard_all();
    }

    /// Returns a worker-local candidate for the next snapshot selection.
    ///
    /// Ready generations share their exact snapshot. A cancelled derived build
    /// retains its completed base postings, so `BaseOnly` synthesizes a light
    /// base-sequence snapshot that shares the PDQ rows and copies no postings.
    pub(crate) fn cached_snapshot_candidate(&self) -> Option<Arc<SearchSnapshot>> {
        match &self.cache {
            MihCacheState::Ready { derived, .. } => Some(Arc::clone(&derived.snapshot)),
            MihCacheState::BaseOnly(base) => Some(Arc::new(SearchSnapshot {
                delta: Arc::from([]),
                superseded: vec![0; base.array.records.len().div_ceil(64)].into(),
                applied_seq: base.array.applied_seq,
                base: Arc::clone(&base.array),
            })),
            MihCacheState::Empty => None,
        }
    }

    /// Exact base allocation whose completed postings can break a snapshot rank tie.
    pub(crate) fn preferred_base(&self) -> Option<&Arc<BaseArray>> {
        match &self.cache {
            MihCacheState::BaseOnly(base) | MihCacheState::Ready { base, .. } => Some(&base.array),
            MihCacheState::Empty => None,
        }
    }

    pub(crate) fn prepare(
        &mut self,
        snapshot: Arc<SearchSnapshot>,
        cancel: &AtomicBool,
    ) -> Result<(), MihBuildError> {
        self.prepare_checked(snapshot, &mut || cancel.load(Ordering::Acquire))
    }

    fn prepare_checked(
        &mut self,
        snapshot: Arc<SearchSnapshot>,
        cancelled: &mut impl FnMut() -> bool,
    ) -> Result<(), MihBuildError> {
        if matches!(
            &self.cache,
            MihCacheState::Ready { derived, .. }
                if Arc::ptr_eq(&derived.snapshot, &snapshot)
        ) {
            return Ok(());
        }

        let previous = std::mem::take(&mut self.cache);
        let base = match previous {
            MihCacheState::BaseOnly(base) if Arc::ptr_eq(&base.array, &snapshot.base) => base,
            MihCacheState::Ready { base, derived } if Arc::ptr_eq(&base.array, &snapshot.base) => {
                // Release snapshot-specific memory before allocating the replacement.
                drop(derived);
                self.scratch.discard_delta();
                base
            }
            previous => {
                // A row index is meaningful only for the exact BaseArray allocation.
                // Drop the old cache before evaluating the new build's allocations.
                drop(previous);
                self.scratch.discard_all();
                let postings = MihPostings::build(
                    snapshot.base.records.len(),
                    |row| {
                        snapshot
                            .base
                            .records
                            .get(row)
                            .filter(|record| record.quality > 0)
                            .map(|record| record.signature)
                    },
                    cancelled,
                )?;
                BaseMih {
                    array: Arc::clone(&snapshot.base),
                    postings,
                }
            }
        };

        self.cache = MihCacheState::BaseOnly(base);
        let delta_postings = match MihPostings::build(
            snapshot.delta.len(),
            |row| {
                snapshot
                    .delta
                    .get(row)
                    .and_then(|entry| entry.record)
                    .filter(|record| record.quality > 0)
                    .map(|record| record.signature)
            },
            cancelled,
        ) {
            Ok(postings) => postings,
            Err(error) => return Err(error),
        };
        let base = match std::mem::take(&mut self.cache) {
            MihCacheState::BaseOnly(base) => base,
            _ => unreachable!("base postings are published before derived build"),
        };
        let derived = DerivedSnapshotMih {
            snapshot,
            delta_postings,
        };
        self.cache = MihCacheState::ready(base, derived)?;
        Ok(())
    }

    pub(crate) fn visit_within<E>(
        &mut self,
        signature: &[u8; 32],
        radius: u32,
        cancel: &AtomicBool,
        mut visitor: impl FnMut(MihHit) -> Result<MihVisitControl, E>,
    ) -> Result<MihVisitCompletion, MihQueryError<E>> {
        self.visit_within_checked(
            signature,
            radius,
            &mut || cancel.load(Ordering::Acquire),
            &mut visitor,
        )
    }

    fn visit_within_checked<E>(
        &mut self,
        signature: &[u8; 32],
        radius: u32,
        cancelled: &mut impl FnMut() -> bool,
        visitor: &mut impl FnMut(MihHit) -> Result<MihVisitControl, E>,
    ) -> Result<MihVisitCompletion, MihQueryError<E>> {
        if radius > 32 {
            return Err(MihQueryError::RadiusOutOfRange(radius));
        }
        let MihCacheState::Ready { base, derived } = &self.cache else {
            return Err(MihQueryError::CorruptIndex);
        };
        if !Arc::ptr_eq(&base.array, &derived.snapshot.base) {
            return Err(MihQueryError::CorruptIndex);
        }

        let generation = self
            .scratch
            .begin_query(
                base.array.records.len(),
                derived.snapshot.delta.len(),
                cancelled,
            )
            .map_err(map_infallible_query_error)?;

        let base_completion = visit_postings(
            &base.postings,
            &mut self.scratch.base_marks,
            generation,
            signature,
            radius,
            cancelled,
            |row| {
                if derived.snapshot.base_record_is_superseded(row) {
                    None
                } else {
                    base.array.records.get(row).copied()
                }
            },
            visitor,
        )?;
        if base_completion == MihVisitCompletion::Stopped {
            return Ok(base_completion);
        }

        visit_postings(
            &derived.delta_postings,
            &mut self.scratch.delta_marks,
            generation,
            signature,
            radius,
            cancelled,
            |row| {
                derived
                    .snapshot
                    .delta
                    .get(row)
                    .and_then(|entry| entry.record)
                    .filter(|record| record.quality > 0)
            },
            visitor,
        )
    }

    #[cfg(test)]
    fn cache_shape(&self) -> &'static str {
        match self.cache {
            MihCacheState::Empty => "empty",
            MihCacheState::BaseOnly(_) => "base-only",
            MihCacheState::Ready { .. } => "ready",
        }
    }
}

fn visit_postings<E>(
    postings: &MihPostings,
    marks: &mut [u32],
    generation: u32,
    query: &[u8; 32],
    radius: u32,
    cancelled: &mut impl FnMut() -> bool,
    mut record_at: impl FnMut(usize) -> Option<SearchRecord>,
    visitor: &mut impl FnMut(MihHit) -> Result<MihVisitControl, E>,
) -> Result<MihVisitCompletion, MihQueryError<E>> {
    let mut visited_postings = 0usize;
    for block in 0..BLOCK_COUNT {
        let value = block_value(query, block);
        let mut completion = MihVisitCompletion::Exhausted;
        visit_neighbors(value, block == 0, |neighbor| {
            if completion == MihVisitCompletion::Stopped {
                return Ok(());
            }
            if cancelled() {
                return Err(MihQueryError::Cancelled);
            }
            let rows = postings
                .range(block, neighbor)
                .ok_or(MihQueryError::CorruptIndex)?;
            for &row in rows {
                visited_postings += 1;
                if visited_postings % CANCEL_INTERVAL == 0 && cancelled() {
                    return Err(MihQueryError::Cancelled);
                }
                let row = row as usize;
                let mark = marks.get_mut(row).ok_or(MihQueryError::CorruptIndex)?;
                if *mark == generation {
                    continue;
                }
                *mark = generation;
                let Some(record) = record_at(row) else {
                    continue;
                };
                let distance = hamming256(query, &record.signature);
                if distance > radius {
                    continue;
                }
                let hit = MihHit {
                    item_id: record.item_id,
                    revision: record.revision,
                    distance,
                };
                match visitor(hit).map_err(MihQueryError::Visitor)? {
                    MihVisitControl::Continue => {}
                    MihVisitControl::Stop => {
                        completion = MihVisitCompletion::Stopped;
                        break;
                    }
                }
            }
            Ok(())
        })?;
        if completion == MihVisitCompletion::Stopped {
            return Ok(completion);
        }
    }
    if cancelled() {
        Err(MihQueryError::Cancelled)
    } else {
        Ok(MihVisitCompletion::Exhausted)
    }
}

fn visit_neighbors<E>(
    value: u16,
    distance_two: bool,
    mut visit: impl FnMut(u16) -> Result<(), E>,
) -> Result<(), E> {
    visit(value)?;
    for first in 0..16 {
        visit(value ^ (1 << first))?;
    }
    if distance_two {
        for first in 0..16 {
            for second in first + 1..16 {
                visit(value ^ (1 << first) ^ (1 << second))?;
            }
        }
    }
    Ok(())
}

fn posting_slot(signature: &[u8; 32], block: usize) -> usize {
    block * BUCKET_COUNT + block_value(signature, block) as usize
}

fn block_value(signature: &[u8; 32], block: usize) -> u16 {
    u16::from_le_bytes([signature[block * 2], signature[block * 2 + 1]])
}

fn hamming256(left: &[u8; 32], right: &[u8; 32]) -> u32 {
    left.iter()
        .zip(right)
        .map(|(left, right)| (left ^ right).count_ones())
        .sum()
}

fn check_cancelled_at(
    position: usize,
    cancelled: &mut impl FnMut() -> bool,
) -> Result<(), MihBuildError> {
    if position % CANCEL_INTERVAL == 0 && cancelled() {
        Err(MihBuildError::Cancelled)
    } else {
        Ok(())
    }
}

fn clear_marks_checked<E>(
    marks: &mut [u32],
    cancelled: &mut impl FnMut() -> bool,
) -> Result<(), MihQueryError<E>> {
    for (index, mark) in marks.iter_mut().enumerate() {
        if index % CANCEL_INTERVAL == 0 && cancelled() {
            return Err(MihQueryError::Cancelled);
        }
        *mark = 0;
    }
    Ok(())
}

fn map_infallible_query_error<E>(error: MihQueryError<Infallible>) -> MihQueryError<E> {
    match error {
        MihQueryError::Cancelled => MihQueryError::Cancelled,
        MihQueryError::RadiusOutOfRange(radius) => MihQueryError::RadiusOutOfRange(radius),
        MihQueryError::CorruptIndex => MihQueryError::CorruptIndex,
        MihQueryError::Visitor(value) => match value {},
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::sync::atomic::AtomicUsize;

    use super::*;
    use crate::similar_search_array::DeltaEntry;

    fn record(item_id: u64, signature: [u8; 32], quality: u8) -> SearchRecord {
        SearchRecord {
            item_id,
            signature,
            quality,
            revision: item_id as u32 + 10,
        }
    }

    fn snapshot(
        base: Arc<BaseArray>,
        delta: Vec<DeltaEntry>,
        superseded_rows: &[usize],
    ) -> Arc<SearchSnapshot> {
        let mut superseded = vec![0u64; base.records.len().div_ceil(64)];
        for &row in superseded_rows {
            superseded[row / 64] |= 1u64 << (row % 64);
        }
        Arc::new(SearchSnapshot {
            applied_seq: base.applied_seq + delta.len() as u64,
            base,
            delta: delta.into(),
            superseded: superseded.into(),
        })
    }

    fn base(records: Vec<SearchRecord>, seq: u64) -> Arc<BaseArray> {
        Arc::new(BaseArray {
            records: records.into_boxed_slice(),
            store_id: [7; 16],
            applied_seq: seq,
        })
    }

    fn signature_with_bits(bits: &[usize]) -> [u8; 32] {
        let mut signature = [0u8; 32];
        for &bit in bits {
            signature[bit / 8] |= 1 << (bit % 8);
        }
        signature
    }

    fn query_all(
        runtime: &mut BookMihRuntime,
        signature: &[u8; 32],
        radius: u32,
    ) -> Result<Vec<MihHit>, MihQueryError<Infallible>> {
        let mut hits = Vec::new();
        let completion =
            runtime.visit_within(signature, radius, &AtomicBool::new(false), |hit| {
                hits.push(hit);
                Ok(MihVisitControl::Continue)
            })?;
        assert_eq!(completion, MihVisitCompletion::Exhausted);
        hits.sort_by_key(|hit| hit.item_id);
        Ok(hits)
    }

    #[test]
    fn exact_radius32_matches_brute_force_and_excludes_distance33() {
        let mut records = Vec::new();
        for item_id in 1..=192u64 {
            let mut signature = [0u8; 32];
            let mut state = item_id.wrapping_mul(0x9e37_79b9_7f4a_7c15);
            for byte in &mut signature {
                state ^= state << 7;
                state ^= state >> 9;
                state ^= state << 8;
                *byte = state as u8;
            }
            records.push(record(item_id, signature, 1));
        }
        records.push(record(1000, signature_with_bits(&[]), 1));
        records.push(record(
            1001,
            signature_with_bits(&(0..32).collect::<Vec<_>>()),
            1,
        ));
        records.push(record(
            1002,
            signature_with_bits(&(0..33).collect::<Vec<_>>()),
            1,
        ));
        let all_blocks_distance_two = (0..BLOCK_COUNT)
            .flat_map(|block| [block * 16, block * 16 + 1])
            .collect::<Vec<_>>();
        records.push(record(
            1100,
            signature_with_bits(&all_blocks_distance_two),
            1,
        ));
        for rescue_block in 1..BLOCK_COUNT {
            let mut bits = vec![0, 1, 2];
            for block in 1..BLOCK_COUNT {
                bits.push(block * 16);
                if block != rescue_block {
                    bits.push(block * 16 + 1);
                }
            }
            assert_eq!(bits.len(), 32);
            records.push(record(
                1100 + rescue_block as u64,
                signature_with_bits(&bits),
                1,
            ));
        }
        let no_rescue_distance_33 = std::iter::once(0)
            .chain(std::iter::once(1))
            .chain(std::iter::once(2))
            .chain((1..BLOCK_COUNT).flat_map(|block| [block * 16, block * 16 + 1]))
            .collect::<Vec<_>>();
        records.push(record(1200, signature_with_bits(&no_rescue_distance_33), 1));
        let base = base(records, 0);
        let snapshot = snapshot(base, Vec::new(), &[]);
        let mut runtime = BookMihRuntime::new();
        runtime
            .prepare(Arc::clone(&snapshot), &AtomicBool::new(false))
            .unwrap();

        for query in [
            [0u8; 32],
            snapshot.base.records[17].signature,
            snapshot.base.records[91].signature,
        ] {
            let actual = query_all(&mut runtime, &query, 32).unwrap();
            let expected = snapshot
                .base
                .records
                .iter()
                .filter(|record| hamming256(&query, &record.signature) <= 32)
                .map(|record| record.item_id)
                .collect::<Vec<_>>();
            assert_eq!(
                actual.iter().map(|hit| hit.item_id).collect::<Vec<_>>(),
                expected
            );
        }

        let zero_hits = query_all(&mut runtime, &[0; 32], 32).unwrap();
        assert!(zero_hits.iter().any(|hit| hit.item_id == 1001));
        assert!(!zero_hits.iter().any(|hit| hit.item_id == 1002));
        assert!(zero_hits.iter().any(|hit| hit.item_id == 1100));
        for rescue_block in 1..BLOCK_COUNT {
            assert!(
                zero_hits
                    .iter()
                    .any(|hit| hit.item_id == 1100 + rescue_block as u64),
                "block {rescue_block} must rescue its only distance-one projection"
            );
        }
        assert!(!zero_hits.iter().any(|hit| hit.item_id == 1200));
    }

    #[test]
    fn duplicate_bucket_probes_emit_each_row_once() {
        let base = base(vec![record(1, [0; 32], 1), record(2, [0; 32], 1)], 0);
        let snapshot = snapshot(base, Vec::new(), &[]);
        let mut runtime = BookMihRuntime::new();
        runtime.prepare(snapshot, &AtomicBool::new(false)).unwrap();
        let hits = query_all(&mut runtime, &[0; 32], 0).unwrap();
        assert_eq!(
            hits.iter().map(|hit| hit.item_id).collect::<Vec<_>>(),
            [1, 2]
        );
    }

    #[test]
    fn superseded_base_quality_zero_and_delta_delete_are_not_emitted() {
        let base = base(
            vec![
                record(1, [0; 32], 1),
                record(2, [0; 32], 1),
                record(3, [0; 32], 0),
                record(4, [0; 32], 1),
            ],
            10,
        );
        let snapshot = snapshot(
            base,
            vec![
                DeltaEntry {
                    item_id: 1,
                    seq: 11,
                    record: Some(record(1, [0; 32], 2)),
                },
                DeltaEntry {
                    item_id: 2,
                    seq: 12,
                    record: None,
                },
                DeltaEntry {
                    item_id: 5,
                    seq: 13,
                    record: Some(record(5, [0; 32], 0)),
                },
            ],
            &[0, 1],
        );
        let mut runtime = BookMihRuntime::new();
        runtime.prepare(snapshot, &AtomicBool::new(false)).unwrap();
        let hits = query_all(&mut runtime, &[0; 32], 0).unwrap();
        assert_eq!(
            hits.iter().map(|hit| hit.item_id).collect::<Vec<_>>(),
            [1, 4]
        );
        assert_eq!(hits[0].revision, 11);
    }

    #[test]
    fn visitor_stop_error_and_cancel_are_distinct() {
        let base = base(vec![record(1, [0; 32], 1), record(2, [0; 32], 1)], 0);
        let snapshot = snapshot(base, Vec::new(), &[]);
        let mut runtime = BookMihRuntime::new();
        runtime.prepare(snapshot, &AtomicBool::new(false)).unwrap();

        let stopped = runtime
            .visit_within(&[0; 32], 0, &AtomicBool::new(false), |_| {
                Ok::<_, &'static str>(MihVisitControl::Stop)
            })
            .unwrap();
        assert_eq!(stopped, MihVisitCompletion::Stopped);

        let error = runtime.visit_within(&[0; 32], 0, &AtomicBool::new(false), |_| {
            Err::<MihVisitControl, _>("visitor")
        });
        assert_eq!(error, Err(MihQueryError::Visitor("visitor")));

        let cancel = AtomicBool::new(true);
        let error = runtime.visit_within(&[0; 32], 0, &cancel, |_| {
            Ok::<_, Infallible>(MihVisitControl::Continue)
        });
        assert_eq!(error, Err(MihQueryError::Cancelled));
    }

    #[test]
    fn same_base_reuses_base_postings_and_cancelled_delta_leaves_base_only() {
        let base = base(vec![record(1, [0; 32], 1)], 5);
        let first = snapshot(Arc::clone(&base), Vec::new(), &[]);
        let second = snapshot(
            Arc::clone(&base),
            vec![DeltaEntry {
                item_id: 2,
                seq: 6,
                record: Some(record(2, [0; 32], 1)),
            }],
            &[],
        );
        let mut runtime = BookMihRuntime::new();
        runtime.prepare(first, &AtomicBool::new(false)).unwrap();
        let postings_ptr = match &runtime.cache {
            MihCacheState::Ready { base, .. } => base.postings.rows.as_ptr(),
            _ => panic!("expected ready cache"),
        };

        let checks = AtomicUsize::new(0);
        let error =
            runtime.prepare_checked(second, &mut || checks.fetch_add(1, Ordering::Relaxed) >= 1);
        assert_eq!(error, Err(MihBuildError::Cancelled));
        assert_eq!(runtime.cache_shape(), "base-only");
        let retained_ptr = match &runtime.cache {
            MihCacheState::BaseOnly(base) => base.postings.rows.as_ptr(),
            _ => panic!("expected retained base cache"),
        };
        assert_eq!(retained_ptr, postings_ptr);

        let candidate = runtime.cached_snapshot_candidate().unwrap();
        assert!(Arc::ptr_eq(&candidate.base, &base));
        assert!(candidate.delta.is_empty());
        assert_eq!(candidate.applied_seq, base.applied_seq);
        assert_eq!(candidate.superseded.len(), base.records.len().div_ceil(64));
        runtime.prepare(candidate, &AtomicBool::new(false)).unwrap();
        let reused_ptr = match &runtime.cache {
            MihCacheState::Ready { base, .. } => base.postings.rows.as_ptr(),
            _ => panic!("expected rebuilt derived generation"),
        };
        assert_eq!(reused_ptr, postings_ptr);
        assert_eq!(
            query_all(&mut runtime, &[0; 32], 0)
                .unwrap()
                .iter()
                .map(|hit| hit.item_id)
                .collect::<Vec<_>>(),
            [1]
        );
    }

    #[test]
    fn different_base_same_seq_is_dropped_before_cancelled_rebuild() {
        let old_base = base(vec![record(1, [0; 32], 1)], 5);
        let old_snapshot = snapshot(Arc::clone(&old_base), Vec::new(), &[]);
        let new_base = base(vec![record(2, [0; 32], 1)], 5);
        let new_snapshot = snapshot(new_base, Vec::new(), &[]);
        let mut runtime = BookMihRuntime::new();
        runtime
            .prepare(old_snapshot, &AtomicBool::new(false))
            .unwrap();
        assert_eq!(Arc::strong_count(&old_base), 3);

        let error = runtime.prepare_checked(new_snapshot, &mut || true);
        assert_eq!(error, Err(MihBuildError::Cancelled));
        assert_eq!(runtime.cache_shape(), "empty");
        assert_eq!(Arc::strong_count(&old_base), 1);
    }

    #[test]
    fn base_build_cancellation_never_publishes_partial_postings() {
        let records = (0..3000)
            .map(|index| record(index + 1, [index as u8; 32], 1))
            .collect();
        let snapshot = snapshot(base(records, 0), Vec::new(), &[]);
        let mut runtime = BookMihRuntime::new();
        let calls = AtomicUsize::new(0);
        let error =
            runtime.prepare_checked(snapshot, &mut || calls.fetch_add(1, Ordering::Relaxed) >= 2);
        assert_eq!(error, Err(MihBuildError::Cancelled));
        assert_eq!(runtime.cache_shape(), "empty");
    }

    #[test]
    fn cancelled_generation_wrap_clear_is_retried_before_marks_are_reused() {
        let records = (0..3000)
            .map(|index| record(index + 1, [0; 32], 1))
            .collect();
        let snapshot = snapshot(base(records, 0), Vec::new(), &[]);
        let mut runtime = BookMihRuntime::new();
        runtime.prepare(snapshot, &AtomicBool::new(false)).unwrap();
        runtime.scratch.base_marks.resize(3000, 1);
        runtime.scratch.generation = u32::MAX;

        let checks = AtomicUsize::new(0);
        let cancelled = runtime.visit_within_checked(
            &[0; 32],
            0,
            &mut || checks.fetch_add(1, Ordering::Relaxed) >= 2,
            &mut |_| Ok::<_, Infallible>(MihVisitControl::Continue),
        );
        assert_eq!(cancelled, Err(MihQueryError::Cancelled));
        assert_eq!(runtime.scratch.generation, u32::MAX);

        let hits = query_all(&mut runtime, &[0; 32], 0).unwrap();
        assert_eq!(hits.len(), 3000);
        assert_eq!(runtime.scratch.generation, 1);
    }

    #[test]
    fn clear_retires_ready_cache_and_query_requires_a_ready_generation() {
        let snapshot = snapshot(base(vec![record(1, [0; 32], 1)], 0), Vec::new(), &[]);
        let mut runtime = BookMihRuntime::new();
        runtime.prepare(snapshot, &AtomicBool::new(false)).unwrap();
        runtime.clear();
        assert_eq!(runtime.cache_shape(), "empty");
        let result = runtime.visit_within(&[0; 32], 32, &AtomicBool::new(false), |_| {
            Ok::<_, Infallible>(MihVisitControl::Continue)
        });
        assert_eq!(result, Err(MihQueryError::CorruptIndex));
    }

    #[test]
    fn streamed_hits_match_brute_force_for_each_supported_radius_boundary() {
        let signatures = (0..80u64)
            .map(|item_id| {
                let bits = (0..256)
                    .filter(|bit| ((item_id.wrapping_mul(37) + *bit as u64 * 13) % 17) == 0)
                    .collect::<Vec<_>>();
                record(item_id + 1, signature_with_bits(&bits), 1)
            })
            .collect::<Vec<_>>();
        let snapshot = snapshot(base(signatures, 0), Vec::new(), &[]);
        let mut runtime = BookMihRuntime::new();
        runtime
            .prepare(Arc::clone(&snapshot), &AtomicBool::new(false))
            .unwrap();

        for radius in [0, 1, 2, 15, 31, 32] {
            for query in snapshot.base.records.iter().step_by(7) {
                let hits = query_all(&mut runtime, &query.signature, radius).unwrap();
                let actual = hits
                    .iter()
                    .map(|hit| (hit.item_id, hit.distance))
                    .collect::<BTreeMap<_, _>>();
                let expected = snapshot
                    .base
                    .records
                    .iter()
                    .filter_map(|candidate| {
                        let distance = hamming256(&query.signature, &candidate.signature);
                        (distance <= radius).then_some((candidate.item_id, distance))
                    })
                    .collect::<BTreeMap<_, _>>();
                assert_eq!(actual, expected);
            }
        }
    }
}
