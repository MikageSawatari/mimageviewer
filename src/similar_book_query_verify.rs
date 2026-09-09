//! Independent, bounded correctness verifier for real similar-book queries.
//!
//! The product engine supplies only the result under test. This module derives eligibility,
//! common-page evidence, candidate books, dense legacy classification, and sparse strips from a
//! separate read-only SQLite transaction. It deliberately does not call the product MIH, raw-hit
//! resolver, page-order resolver, streamed classifier, or strip builder.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

use rusqlite::{Connection, OpenFlags, OptionalExtension, TransactionBehavior, params};
use serde::Serialize;

use crate::dupe::{Sig, book};
use crate::search_norm::ZIP_ENTRY_SEP;
use crate::similar_book_query_bench::{
    BenchBookOriginPage, BenchBookPageBaseline, BenchBookPageMatch, BenchBookPageMatchState,
    BenchBookPair, BenchBookQueryOutcome, BenchBookRelation, BenchBookRelationHit,
    BenchBookRelations, BenchEngine, BenchEngineConfig, BenchQueryCertificate,
    BenchSimilarItemTarget, expected_product_book_query_worker_priority,
    product_book_query_worker_priority,
};
use crate::similar_db::{PAGE_ORDER_VERSION, current_hash_version};
use crate::similar_index::{
    BOOK_COVERAGE, BOOK_MAX_BOOKS_PER_PAGE, BOOK_MIN_MATCHED_PAGES, BOOK_MIN_QUALITY, BOOK_RADIUS,
    NEARLY_IDENTICAL_MAX_DISTANCE, compare_book_pages,
};

const BOOK_ORIGIN: u32 = 1;
const BOOK_CANDIDATE: u32 = 2;
const ITEM_KIND_IMAGE: i64 = 0;
const ITEM_KIND_ZIP_PAGE: i64 = 1;
const ITEM_KIND_PDF_PAGE: i64 = 2;
const CONTAINER_KIND_ZIP: i64 = 1;
const SCAN_STATE_COMPLETE: i64 = 1;

#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
pub struct VerificationBudgets {
    pub max_candidates: u64,
    pub max_hamming_comparisons: u64,
    pub max_certificate_page_units: u64,
    pub max_legacy_possible_pairs: u64,
    pub max_stale_zip_pages: u64,
    pub max_output_bytes: u64,
}

impl Default for VerificationBudgets {
    fn default() -> Self {
        Self {
            max_candidates: 64,
            max_hamming_comparisons: 5_000_000_000,
            max_certificate_page_units: 10_000,
            max_legacy_possible_pairs: 2_000_000,
            max_stale_zip_pages: 100_000,
            max_output_bytes: 64 * 1024 * 1024,
        }
    }
}

#[derive(Clone, Debug)]
pub struct VerificationSpec {
    pub origin: String,
    pub immutable_roots: Vec<String>,
    pub expected_result_sha256: String,
    pub budgets: VerificationBudgets,
}

#[derive(Debug, Serialize)]
pub struct VerificationReport {
    pub schema_version: u32,
    pub worker_priority: i32,
    pub expected_worker_priority: i32,
    pub expected_result_sha256: String,
    pub budgets: VerificationBudgets,
    pub product: BenchQueryCertificate,
    pub oracle: OracleCertificate,
    pub checks: VerificationChecks,
}

#[derive(Debug, Serialize)]
pub struct VerificationChecks {
    pub result_digest_matches_measurement: bool,
    pub metadata_matches_independent_transaction: bool,
    pub candidate_keys_match: bool,
    pub every_pair_field_matches: bool,
    pub every_strip_field_matches: bool,
    pub target_resolution_matches_current_filesystem_observation: bool,
}

#[derive(Debug, Serialize)]
pub struct OracleCertificate {
    pub metadata: OracleMetadata,
    pub corpus: OracleCorpusEvidence,
    pub origin: OracleBookCertificate,
    pub origin_signatures: Vec<OracleSignatureEvidence>,
    pub candidates: Vec<OracleCandidateCertificate>,
    pub witness_pages: Vec<OraclePageCertificate>,
    pub expected_relations: BenchBookRelations,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct OracleMetadata {
    pub store_id_hex: String,
    pub read_seq: u64,
    pub page_order_version: i64,
}

#[derive(Debug, Serialize)]
pub struct OracleCorpusEvidence {
    pub complete_current_hash_rows_upper_bound: u64,
    pub actual_hamming_comparisons: u64,
    pub charged_hamming_comparisons: u64,
    pub certificate_page_units: u64,
    pub legacy_eligible_pages: u64,
    pub legacy_possible_pairs: u64,
    pub current_hash_version: i64,
    pub shared_natural_page_comparator_only: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct OracleBookCertificate {
    pub container_key: String,
    pub container_kind: i64,
    pub scan_state: i64,
    pub raw_pages: Vec<OraclePageCertificate>,
    pub product_eligible_pages: u64,
    pub legacy_indexed_pages: u64,
}

#[derive(Clone, Debug, Serialize)]
pub struct OracleCandidateCertificate {
    pub container_key: String,
    pub discovery_edges: u32,
    pub book: OracleBookCertificate,
    pub signature_evidence: Vec<OracleCandidateSignatureEvidence>,
    pub legacy_pair: BenchBookPair,
    pub expected_hit: BenchBookRelationHit,
}

#[derive(Clone, Debug, Serialize)]
pub struct OracleCandidateSignatureEvidence {
    pub signature_hex: String,
    pub reused_origin_evidence: bool,
    pub evidence: Option<OracleNeighborhoodEvidence>,
}

#[derive(Clone, Debug, Serialize)]
pub struct OracleSignatureEvidence {
    pub signature_hex: String,
    pub evidence: OracleNeighborhoodEvidence,
}

#[derive(Clone, Debug, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum OracleNeighborhoodEvidence {
    Common { witnesses: Vec<OracleWitness> },
    Rare { books: Vec<OracleRareBook> },
}

#[derive(Clone, Debug, Serialize)]
pub struct OracleWitness {
    pub item_id: u64,
    pub container_key: String,
    pub effective_page_index: u32,
    pub distance: u32,
    pub eligibility: &'static str,
}

#[derive(Clone, Debug, Serialize)]
pub struct OracleRareBook {
    pub container_key: String,
    pub near_page_count_capped_at_three: u8,
}

#[derive(Clone, Debug, Serialize)]
pub struct OraclePageCertificate {
    pub item_id: u64,
    pub revision: u32,
    pub item_key: String,
    pub item_kind: i64,
    pub container_key: String,
    pub stored_page_index: Option<u32>,
    pub effective_page_index: Option<u32>,
    pub mtime: i64,
    pub file_size: i64,
    pub hash_version: i64,
    pub signature_hex: String,
    pub quality: u8,
    pub width: u32,
    pub height: u32,
    pub format: i64,
    pub eligibility: OraclePageEligibility,
    pub target: Option<BenchSimilarItemTarget>,
    pub target_path_resolution: OracleTargetPathResolution,
}

#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum OraclePageEligibility {
    CurrentHashIndexed,
    CurrentHashWithoutEffectiveIndex,
    HashMismatch,
}

#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum OracleTargetPathResolution {
    CanonicalizedExistingPath,
    RawPathFallback,
    InvalidItemKey,
}

#[derive(Clone, Debug)]
struct OraclePage {
    item_id: u64,
    revision: u32,
    item_key: String,
    item_kind: i64,
    container_key: String,
    stored_page_index: Option<u32>,
    effective_page_index: Option<u32>,
    mtime: i64,
    file_size: i64,
    hash_version: i64,
    signature: [u8; 32],
    quality: u8,
    width: u32,
    height: u32,
    format: i64,
}

#[derive(Clone, Debug)]
struct OracleBook {
    container_key: String,
    container_kind: i64,
    scan_state: i64,
    raw_pages: Vec<OraclePage>,
    eligible_pages: Vec<OraclePage>,
}

#[derive(Clone, Debug)]
struct RareMatch {
    count: u8,
    witness: OraclePage,
    distance: u32,
}

#[derive(Clone, Debug)]
enum ProbeState {
    Rare(BTreeMap<String, RareMatch>),
    Common(Vec<OracleWitness>),
}

#[derive(Clone, Debug)]
struct SignatureProbe {
    signature: [u8; 32],
    state: ProbeState,
}

struct OracleBudgetState {
    limits: VerificationBudgets,
    charged_hamming_comparisons: u64,
    actual_hamming_comparisons: u64,
    certificate_page_units: u64,
    legacy_eligible_pages: u64,
    legacy_possible_pairs: u64,
}

impl OracleBudgetState {
    fn new(limits: VerificationBudgets) -> Self {
        Self {
            limits,
            charged_hamming_comparisons: 0,
            actual_hamming_comparisons: 0,
            certificate_page_units: 0,
            legacy_eligible_pages: 0,
            legacy_possible_pairs: 0,
        }
    }

    fn charge_scan(&mut self, rows: u64, signatures: u64) -> Result<(), String> {
        let charge = rows
            .checked_mul(signatures)
            .ok_or_else(|| "oracle comparison budget overflow".to_owned())?;
        self.charged_hamming_comparisons = self
            .charged_hamming_comparisons
            .checked_add(charge)
            .ok_or_else(|| "oracle comparison budget overflow".to_owned())?;
        if self.charged_hamming_comparisons > self.limits.max_hamming_comparisons {
            return Err(format!(
                "oracle would perform more than {} Hamming comparisons",
                self.limits.max_hamming_comparisons
            ));
        }
        Ok(())
    }

    fn charge_page_units(&mut self, units: u64) -> Result<(), String> {
        self.certificate_page_units = self
            .certificate_page_units
            .checked_add(units)
            .ok_or_else(|| "certificate page budget overflow".to_owned())?;
        if self.certificate_page_units > self.limits.max_certificate_page_units {
            return Err(format!(
                "certificate exceeds {} raw-page and witness-reference units",
                self.limits.max_certificate_page_units
            ));
        }
        Ok(())
    }

    fn record_legacy_size(&mut self, eligible_pages: usize) -> Result<(), String> {
        let eligible = u64::try_from(eligible_pages)
            .map_err(|_| "legacy eligible page count exceeds u64".to_owned())?;
        let possible = eligible
            .checked_mul(eligible.saturating_sub(1))
            .and_then(|value| value.checked_div(2))
            .ok_or_else(|| "legacy possible-pair count overflow".to_owned())?;
        if possible > self.limits.max_legacy_possible_pairs {
            return Err(format!(
                "legacy oracle would admit {possible} possible page pairs, limit is {}",
                self.limits.max_legacy_possible_pairs
            ));
        }
        self.legacy_eligible_pages = self
            .legacy_eligible_pages
            .checked_add(eligible)
            .ok_or_else(|| "legacy eligible page total overflow".to_owned())?;
        self.legacy_possible_pairs = self
            .legacy_possible_pairs
            .checked_add(possible)
            .ok_or_else(|| "legacy possible-pair total overflow".to_owned())?;
        if self.legacy_possible_pairs > self.limits.max_legacy_possible_pairs {
            return Err(format!(
                "all legacy certificates together exceed {} possible page pairs",
                self.limits.max_legacy_possible_pairs
            ));
        }
        Ok(())
    }
}

/// Runs one product query followed by a separate direct-SQL oracle on the same guarded bytes.
pub fn verify_real_book(
    engine_config: BenchEngineConfig,
    database_path: &Path,
    spec: VerificationSpec,
) -> Result<VerificationReport, String> {
    let worker_priority = product_book_query_worker_priority()?;
    let expected_worker_priority = expected_product_book_query_worker_priority();
    if worker_priority != expected_worker_priority {
        return Err(format!(
            "verification worker priority {worker_priority} != expected {expected_worker_priority}"
        ));
    }

    let mut engine = BenchEngine::open(engine_config)?;
    engine.validate_origin(&spec.origin)?;
    let product = engine.query(&spec.origin)?.into_certificate();
    drop(engine);
    if product.summary.result_sha256 != spec.expected_result_sha256 {
        return Err(format!(
            "product result digest {} != expected measurement digest {}",
            product.summary.result_sha256, spec.expected_result_sha256
        ));
    }

    let oracle = verify_with_direct_sql(database_path, &spec, &product)?;
    Ok(VerificationReport {
        schema_version: 1,
        worker_priority,
        expected_worker_priority,
        expected_result_sha256: spec.expected_result_sha256,
        budgets: spec.budgets,
        product,
        oracle,
        checks: VerificationChecks {
            result_digest_matches_measurement: true,
            metadata_matches_independent_transaction: true,
            candidate_keys_match: true,
            every_pair_field_matches: true,
            every_strip_field_matches: true,
            target_resolution_matches_current_filesystem_observation: true,
        },
    })
}

/// Serializes directly into a bounded writer. The caller has already bounded the DTO itself;
/// this second boundary prevents unexpectedly long escaped paths from creating an unbounded JSON.
pub fn write_json_bounded<T: Serialize>(
    output: &mut impl Write,
    value: &T,
    max_bytes: u64,
) -> Result<u64, String> {
    let mut limited = LimitedWriter {
        inner: output,
        written: 0,
        limit: max_bytes,
    };
    serde_json::to_writer_pretty(&mut limited, value).map_err(|error| error.to_string())?;
    limited
        .write_all(b"\n")
        .map_err(|error| error.to_string())?;
    Ok(limited.written)
}

struct LimitedWriter<'a, W> {
    inner: &'a mut W,
    written: u64,
    limit: u64,
}

impl<W: Write> Write for LimitedWriter<'_, W> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        let bytes_len = u64::try_from(bytes.len()).map_err(|_| {
            std::io::Error::new(std::io::ErrorKind::FileTooLarge, "JSON chunk exceeds u64")
        })?;
        let next = self.written.checked_add(bytes_len).ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::FileTooLarge, "JSON size overflow")
        })?;
        if next > self.limit {
            return Err(std::io::Error::new(
                std::io::ErrorKind::FileTooLarge,
                format!("verification JSON exceeds {} bytes", self.limit),
            ));
        }
        self.inner.write_all(bytes)?;
        self.written = next;
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

fn verify_with_direct_sql(
    database_path: &Path,
    spec: &VerificationSpec,
    product: &BenchQueryCertificate,
) -> Result<OracleCertificate, String> {
    let mut conn = Connection::open_with_flags(
        database_path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|error| format!("oracle database open failed: {error}"))?;
    conn.busy_timeout(Duration::from_secs(5))
        .map_err(|error| format!("oracle busy timeout failed: {error}"))?;
    conn.pragma_update(None, "query_only", true)
        .map_err(|error| format!("oracle query_only failed: {error}"))?;
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Deferred)
        .map_err(|error| format!("oracle read transaction failed: {error}"))?;
    let metadata = read_metadata(&tx)?;
    let expected_metadata = OracleMetadata {
        store_id_hex: product.metadata.store_id_hex.clone(),
        read_seq: product.metadata.read_seq,
        page_order_version: product.metadata.page_order_version,
    };
    if metadata != expected_metadata {
        return Err(format!(
            "oracle metadata {metadata:?} != product metadata {expected_metadata:?}"
        ));
    }
    let product_relations = match &product.outcome {
        BenchBookQueryOutcome::Ready { relations } => relations,
        outcome => {
            return Err(format!(
                "real-book certificate requires a Ready product result, got {outcome:?}"
            ));
        }
    };

    let mut budgets = OracleBudgetState::new(spec.budgets);
    let origin = load_book(&tx, &spec.origin, metadata.page_order_version, &mut budgets)?
        .ok_or_else(|| "oracle origin is not a Complete container".to_owned())?;
    if !key_is_under_any_independent(&origin.container_key, &spec.immutable_roots) {
        return Err("oracle origin is outside the explicit logical roots".to_owned());
    }
    if origin.eligible_pages.is_empty() {
        return Err("oracle origin has no current-hash Complete pages".to_owned());
    }

    let origin_indexed = indexed_pages(&origin.eligible_pages)?;
    let origin_signatures = unique_quality_signatures(&origin_indexed);
    let row_upper = complete_current_hash_row_upper_bound(&tx)?;
    let mut witness_pages = BTreeMap::<u64, OraclePage>::new();
    let mut origin_probes = build_probes(origin_signatures);
    scan_signature_probes(
        &tx,
        metadata.page_order_version,
        &spec.immutable_roots,
        &mut origin_probes,
        &mut budgets,
        row_upper,
        &mut witness_pages,
    )?;

    let mut discoveries = BTreeMap::<String, (u32, BTreeSet<usize>)>::new();
    let mut origin_common_items = HashSet::new();
    for (origin_slot, row) in origin_indexed.iter().enumerate() {
        if row.quality < BOOK_MIN_QUALITY {
            continue;
        }
        let probe = origin_probes
            .get(&row.signature)
            .ok_or_else(|| "oracle origin signature probe disappeared".to_owned())?;
        match &probe.state {
            ProbeState::Common(_) => {
                origin_common_items.insert(row.item_id);
            }
            ProbeState::Rare(books) => {
                for (container_key, near) in books {
                    if container_key == &origin.container_key {
                        continue;
                    }
                    let (edges, slots) = discoveries.entry(container_key.clone()).or_default();
                    *edges = edges
                        .saturating_add(u32::from(near.count))
                        .min(BOOK_MIN_MATCHED_PAGES);
                    slots.insert(origin_slot);
                }
            }
        }
    }
    discoveries.retain(|_, (edges, _)| *edges >= BOOK_MIN_MATCHED_PAGES);
    let candidate_count =
        u64::try_from(discoveries.len()).map_err(|_| "candidate count exceeds u64".to_owned())?;
    if candidate_count > spec.budgets.max_candidates {
        return Err(format!(
            "oracle discovered {candidate_count} candidates, limit is {}",
            spec.budgets.max_candidates
        ));
    }

    let product_candidate_keys = product_relations
        .hits
        .iter()
        .map(|hit| hit.other_container_key.clone())
        .collect::<Vec<_>>();
    let oracle_candidate_keys = discoveries.keys().cloned().collect::<Vec<_>>();
    if oracle_candidate_keys != product_candidate_keys {
        return Err(format!(
            "oracle candidate keys differ from product: oracle={oracle_candidate_keys:?}, product={product_candidate_keys:?}"
        ));
    }

    let expected_origin = build_expected_origin(&origin.eligible_pages, &origin_common_items);
    if expected_origin != product_relations.origin_pages {
        return Err("oracle origin strip differs from the product result".to_owned());
    }

    let mut candidate_certificates = Vec::with_capacity(discoveries.len());
    let mut expected_hits = Vec::with_capacity(discoveries.len());
    let mut legacy_eligible_total = 0u64;
    let mut legacy_pair_total = 0u64;
    for ((candidate_key, (discovery_edges, _)), product_hit) in
        discoveries.into_iter().zip(&product_relations.hits)
    {
        let candidate = load_book(
            &tx,
            &candidate_key,
            metadata.page_order_version,
            &mut budgets,
        )?
        .ok_or_else(|| format!("oracle candidate {candidate_key} is not Complete"))?;
        let candidate_indexed = indexed_pages(&candidate.eligible_pages)?;
        let mut candidate_local = build_probes(
            unique_quality_signatures(&candidate_indexed)
                .into_iter()
                .filter(|signature| !origin_probes.contains_key(signature))
                .collect(),
        );
        scan_signature_probes(
            &tx,
            metadata.page_order_version,
            &spec.immutable_roots,
            &mut candidate_local,
            &mut budgets,
            row_upper,
            &mut witness_pages,
        )?;

        let witness_ids = common_witness_ids_for_pair(
            &origin_indexed,
            &candidate_indexed,
            &origin_probes,
            &candidate_local,
        );
        let pair = legacy_pair(
            &origin,
            &candidate,
            &witness_ids,
            &witness_pages,
            &mut budgets,
        )?;
        legacy_eligible_total = budgets.legacy_eligible_pages;
        legacy_pair_total = budgets.legacy_possible_pairs;
        let expected_hit = build_expected_hit(&origin, &candidate, pair.clone())?;
        if &expected_hit != product_hit {
            return Err(format!(
                "oracle pair or strip differs for candidate {candidate_key}"
            ));
        }
        expected_hits.push(expected_hit.clone());
        candidate_certificates.push(OracleCandidateCertificate {
            container_key: candidate_key,
            discovery_edges,
            book: book_certificate(&candidate),
            signature_evidence: candidate_signature_evidence(
                &candidate_indexed,
                &origin_probes,
                &candidate_local,
            ),
            legacy_pair: pair,
            expected_hit,
        });
    }

    let expected_relations = BenchBookRelations {
        origin_pages: expected_origin,
        hits: expected_hits,
    };
    if &expected_relations != product_relations {
        return Err("oracle full relations differ from product".to_owned());
    }
    let witness_pages = witness_pages
        .into_values()
        .map(|page| page_certificate(&page))
        .collect();
    let origin_signatures = probe_evidence(&origin_probes);
    let certificate = OracleCertificate {
        metadata,
        corpus: OracleCorpusEvidence {
            complete_current_hash_rows_upper_bound: row_upper,
            actual_hamming_comparisons: budgets.actual_hamming_comparisons,
            charged_hamming_comparisons: budgets.charged_hamming_comparisons,
            certificate_page_units: budgets.certificate_page_units,
            legacy_eligible_pages: legacy_eligible_total,
            legacy_possible_pairs: legacy_pair_total,
            current_hash_version: current_hash_version(),
            shared_natural_page_comparator_only: true,
        },
        origin: book_certificate(&origin),
        origin_signatures,
        candidates: candidate_certificates,
        witness_pages,
        expected_relations,
    };
    tx.commit()
        .map_err(|error| format!("oracle read transaction close failed: {error}"))?;
    Ok(certificate)
}

fn read_metadata(conn: &Connection) -> Result<OracleMetadata, String> {
    let (store_id, page_order_version, read_seq) = conn
        .query_row(
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
        )
        .map_err(|error| format!("oracle metadata query failed: {error}"))?;
    if store_id.len() != 16 {
        return Err(format!("oracle store id length {} != 16", store_id.len()));
    }
    Ok(OracleMetadata {
        store_id_hex: hex_bytes(&store_id),
        read_seq: u64::try_from(read_seq)
            .map_err(|_| format!("oracle read_seq is negative: {read_seq}"))?,
        page_order_version,
    })
}

fn complete_current_hash_row_upper_bound(conn: &Connection) -> Result<u64, String> {
    let count = conn
        .query_row(
            "SELECT COUNT(*) FROM item i
               JOIN container c ON c.container_key=i.container_key
              WHERE c.scan_state=?1 AND i.hash_version=?2",
            params![SCAN_STATE_COMPLETE, current_hash_version()],
            |row| row.get::<_, i64>(0),
        )
        .map_err(|error| format!("oracle corpus count failed: {error}"))?;
    u64::try_from(count).map_err(|_| format!("oracle corpus count is negative: {count}"))
}

fn load_book(
    conn: &Connection,
    container_key: &str,
    page_order_version: i64,
    budgets: &mut OracleBudgetState,
) -> Result<Option<OracleBook>, String> {
    let container = conn
        .query_row(
            "SELECT kind, scan_state FROM container WHERE container_key=?1",
            [container_key],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
        )
        .optional()
        .map_err(|error| format!("oracle container lookup failed: {error}"))?;
    let Some((container_kind, scan_state)) = container else {
        return Ok(None);
    };
    if scan_state != SCAN_STATE_COMPLETE {
        return Ok(None);
    }
    let count = conn
        .query_row(
            "SELECT COUNT(*) FROM item WHERE container_key=?1",
            [container_key],
            |row| row.get::<_, i64>(0),
        )
        .map_err(|error| format!("oracle book page count failed: {error}"))?;
    let count =
        u64::try_from(count).map_err(|_| format!("oracle book page count is negative: {count}"))?;
    budgets.charge_page_units(count)?;

    let mut statement = conn
        .prepare(
            "SELECT item_id, revision, item_key, kind, container_key, page_index,
                    mtime, file_size, hash_version, pdq256, quality, width, height, format
               FROM item WHERE container_key=?1 ORDER BY page_index, item_id",
        )
        .map_err(|error| format!("oracle book page prepare failed: {error}"))?;
    let mut rows = statement
        .query([container_key])
        .map_err(|error| format!("oracle book page query failed: {error}"))?;
    let mut raw_pages = Vec::with_capacity(
        usize::try_from(count).map_err(|_| "oracle book page count exceeds usize".to_owned())?,
    );
    while let Some(row) = rows
        .next()
        .map_err(|error| format!("oracle book row read failed: {error}"))?
    {
        raw_pages.push(read_oracle_page(row)?);
    }
    drop(rows);
    drop(statement);
    if page_order_version != PAGE_ORDER_VERSION && container_kind == CONTAINER_KIND_ZIP {
        apply_independent_stale_zip_order(container_key, &mut raw_pages, budgets)?;
    }
    let mut eligible_pages = raw_pages
        .iter()
        .filter(|page| page.hash_version == current_hash_version())
        .cloned()
        .collect::<Vec<_>>();
    eligible_pages.sort_by(|left, right| {
        left.effective_page_index
            .cmp(&right.effective_page_index)
            .then_with(|| left.item_id.cmp(&right.item_id))
    });
    assert_unique_effective_indices(&eligible_pages, container_key)?;
    Ok(Some(OracleBook {
        container_key: container_key.to_owned(),
        container_kind,
        scan_state,
        raw_pages,
        eligible_pages,
    }))
}

fn apply_independent_stale_zip_order(
    container_key: &str,
    pages: &mut [OraclePage],
    budgets: &OracleBudgetState,
) -> Result<(), String> {
    let count =
        u64::try_from(pages.len()).map_err(|_| "stale ZIP page count exceeds u64".to_owned())?;
    if count > budgets.limits.max_stale_zip_pages {
        return Err(format!(
            "stale ZIP {container_key:?} contains {count} rows, verifier limit is {}",
            budgets.limits.max_stale_zip_pages
        ));
    }
    // `pages` entered in stored (page_index,item_id) order. Stable sorting therefore gives equal
    // natural keys the same tie order as the writer without calling the product order resolver.
    let prefix_len = container_key.len().saturating_add(ZIP_ENTRY_SEP.len_utf8());
    let mut order = (0..pages.len()).collect::<Vec<_>>();
    order.sort_by(|&left, &right| {
        let left_key = pages[left]
            .item_key
            .get(prefix_len..)
            .unwrap_or(&pages[left].item_key);
        let right_key = pages[right]
            .item_key
            .get(prefix_len..)
            .unwrap_or(&pages[right].item_key);
        compare_book_pages(left_key, right_key)
    });
    for (ordinal, source) in order.into_iter().enumerate() {
        pages[source].effective_page_index = Some(
            u32::try_from(ordinal)
                .map_err(|_| "stale ZIP effective ordinal exceeds u32".to_owned())?,
        );
    }
    Ok(())
}

fn assert_unique_effective_indices(
    pages: &[OraclePage],
    container_key: &str,
) -> Result<(), String> {
    let mut previous = None;
    for page in pages {
        let Some(index) = page.effective_page_index else {
            continue;
        };
        if previous == Some(index) {
            return Err(format!(
                "oracle book {container_key:?} contains duplicate effective page index {index}"
            ));
        }
        previous = Some(index);
    }
    Ok(())
}

fn read_oracle_page(row: &rusqlite::Row<'_>) -> Result<OraclePage, String> {
    let signature = row
        .get::<_, Vec<u8>>(9)
        .map_err(|error| format!("oracle signature read failed: {error}"))?;
    let signature: [u8; 32] = signature.try_into().map_err(|signature: Vec<u8>| {
        format!(
            "oracle signature has {} bytes, expected 32",
            signature.len()
        )
    })?;
    let page_index = row
        .get::<_, Option<i64>>(5)
        .map_err(|error| format!("oracle page index read failed: {error}"))?
        .map(|value| {
            u32::try_from(value).map_err(|_| format!("oracle page index is invalid: {value}"))
        })
        .transpose()?;
    let item_id = checked_u64(row, 0, "item_id")?;
    let revision = checked_u32(row, 1, "revision")?;
    let quality = checked_u8(row, 10, "quality")?;
    let width = checked_u32(row, 11, "width")?;
    let height = checked_u32(row, 12, "height")?;
    Ok(OraclePage {
        item_id,
        revision,
        item_key: row
            .get(2)
            .map_err(|error| format!("oracle item key read failed: {error}"))?,
        item_kind: row
            .get(3)
            .map_err(|error| format!("oracle item kind read failed: {error}"))?,
        container_key: row
            .get(4)
            .map_err(|error| format!("oracle container key read failed: {error}"))?,
        stored_page_index: page_index,
        effective_page_index: page_index,
        mtime: row
            .get(6)
            .map_err(|error| format!("oracle mtime read failed: {error}"))?,
        file_size: row
            .get(7)
            .map_err(|error| format!("oracle file size read failed: {error}"))?,
        hash_version: row
            .get(8)
            .map_err(|error| format!("oracle hash version read failed: {error}"))?,
        signature,
        quality,
        width,
        height,
        format: row
            .get(13)
            .map_err(|error| format!("oracle format read failed: {error}"))?,
    })
}

fn checked_u64(row: &rusqlite::Row<'_>, column: usize, name: &str) -> Result<u64, String> {
    let value = row
        .get::<_, i64>(column)
        .map_err(|error| format!("oracle {name} read failed: {error}"))?;
    u64::try_from(value).map_err(|_| format!("oracle {name} is invalid: {value}"))
}

fn checked_u32(row: &rusqlite::Row<'_>, column: usize, name: &str) -> Result<u32, String> {
    let value = row
        .get::<_, i64>(column)
        .map_err(|error| format!("oracle {name} read failed: {error}"))?;
    u32::try_from(value).map_err(|_| format!("oracle {name} is invalid: {value}"))
}

fn checked_u8(row: &rusqlite::Row<'_>, column: usize, name: &str) -> Result<u8, String> {
    let value = row
        .get::<_, i64>(column)
        .map_err(|error| format!("oracle {name} read failed: {error}"))?;
    u8::try_from(value).map_err(|_| format!("oracle {name} is invalid: {value}"))
}

fn indexed_pages(pages: &[OraclePage]) -> Result<Vec<OraclePage>, String> {
    let mut indexed = pages
        .iter()
        .filter(|page| page.effective_page_index.is_some())
        .cloned()
        .collect::<Vec<_>>();
    indexed.sort_by_key(|page| page.effective_page_index);
    Ok(indexed)
}

fn unique_quality_signatures(pages: &[OraclePage]) -> BTreeSet<[u8; 32]> {
    pages
        .iter()
        .filter(|page| page.quality >= BOOK_MIN_QUALITY)
        .map(|page| page.signature)
        .collect()
}

fn build_probes(signatures: BTreeSet<[u8; 32]>) -> BTreeMap<[u8; 32], SignatureProbe> {
    signatures
        .into_iter()
        .map(|signature| {
            (
                signature,
                SignatureProbe {
                    signature,
                    state: ProbeState::Rare(BTreeMap::new()),
                },
            )
        })
        .collect()
}

fn scan_signature_probes(
    conn: &Connection,
    page_order_version: i64,
    immutable_roots: &[String],
    probes: &mut BTreeMap<[u8; 32], SignatureProbe>,
    budgets: &mut OracleBudgetState,
    row_upper: u64,
    witness_pages: &mut BTreeMap<u64, OraclePage>,
) -> Result<(), String> {
    if probes.is_empty() {
        return Ok(());
    }
    budgets.charge_scan(
        row_upper,
        u64::try_from(probes.len()).map_err(|_| "signature count exceeds u64".to_owned())?,
    )?;
    if page_order_version == PAGE_ORDER_VERSION {
        scan_current_order_rows(conn, immutable_roots, probes, budgets, witness_pages)
    } else {
        scan_stale_order_rows(conn, immutable_roots, probes, budgets, witness_pages)
    }
}

fn scan_current_order_rows(
    conn: &Connection,
    immutable_roots: &[String],
    probes: &mut BTreeMap<[u8; 32], SignatureProbe>,
    budgets: &mut OracleBudgetState,
    witness_pages: &mut BTreeMap<u64, OraclePage>,
) -> Result<(), String> {
    let mut statement = conn
        .prepare(
            "SELECT i.item_id, i.revision, i.item_key, i.kind, i.container_key, i.page_index,
                    i.mtime, i.file_size, i.hash_version, i.pdq256, i.quality,
                    i.width, i.height, i.format
               FROM item i JOIN container c ON c.container_key=i.container_key
              WHERE c.scan_state=?1 AND i.hash_version=?2
              ORDER BY i.item_id",
        )
        .map_err(|error| format!("oracle corpus scan prepare failed: {error}"))?;
    let mut rows = statement
        .query(params![SCAN_STATE_COMPLETE, current_hash_version()])
        .map_err(|error| format!("oracle corpus scan query failed: {error}"))?;
    while let Some(row) = rows
        .next()
        .map_err(|error| format!("oracle corpus scan failed: {error}"))?
    {
        let page = read_oracle_page(row)?;
        consider_scannable_page(&page, immutable_roots, probes, budgets, witness_pages)?;
    }
    Ok(())
}

fn scan_stale_order_rows(
    conn: &Connection,
    immutable_roots: &[String],
    probes: &mut BTreeMap<[u8; 32], SignatureProbe>,
    budgets: &mut OracleBudgetState,
    witness_pages: &mut BTreeMap<u64, OraclePage>,
) -> Result<(), String> {
    let mut statement = conn
        .prepare(
            "SELECT i.item_id, i.revision, i.item_key, i.kind, i.container_key, i.page_index,
                    i.mtime, i.file_size, i.hash_version, i.pdq256, i.quality,
                    i.width, i.height, i.format, c.kind
               FROM item i JOIN container c ON c.container_key=i.container_key
              WHERE c.scan_state=?1
              ORDER BY c.container_key, i.page_index, i.item_id",
        )
        .map_err(|error| format!("oracle stale corpus scan prepare failed: {error}"))?;
    let mut rows = statement
        .query([SCAN_STATE_COMPLETE])
        .map_err(|error| format!("oracle stale corpus scan query failed: {error}"))?;
    let mut zip_pages = Vec::<OraclePage>::new();
    let mut zip_key = None::<String>;
    while let Some(row) = rows
        .next()
        .map_err(|error| format!("oracle stale corpus scan failed: {error}"))?
    {
        let page = read_oracle_page(row)?;
        let container_kind = row
            .get::<_, i64>(14)
            .map_err(|error| format!("oracle corpus container kind read failed: {error}"))?;
        if container_kind == CONTAINER_KIND_ZIP {
            if zip_key
                .as_deref()
                .is_some_and(|key| key != page.container_key)
            {
                flush_stale_zip(
                    zip_key.take().expect("stale ZIP key exists"),
                    &mut zip_pages,
                    immutable_roots,
                    probes,
                    budgets,
                    witness_pages,
                )?;
            }
            if zip_key.is_none() {
                zip_key = Some(page.container_key.clone());
            }
            // A stale-order ZIP needs all of its rows only when the container is in this
            // request's scope. Keep the current group key so the preceding scoped group is still
            // flushed exactly once, but never buffer, cap, or sort an out-of-scope group.
            if !key_is_under_any_independent(&page.container_key, immutable_roots) {
                continue;
            }
            let next_count = u64::try_from(zip_pages.len())
                .ok()
                .and_then(|count| count.checked_add(1))
                .ok_or_else(|| "stale ZIP scan page count overflow".to_owned())?;
            if next_count > budgets.limits.max_stale_zip_pages {
                return Err(format!(
                    "stale ZIP {:?} exceeds verifier row limit {}",
                    page.container_key, budgets.limits.max_stale_zip_pages
                ));
            }
            zip_pages.push(page);
        } else {
            if let Some(key) = zip_key.take() {
                flush_stale_zip(
                    key,
                    &mut zip_pages,
                    immutable_roots,
                    probes,
                    budgets,
                    witness_pages,
                )?;
            }
            consider_scannable_page(&page, immutable_roots, probes, budgets, witness_pages)?;
        }
    }
    if let Some(key) = zip_key {
        flush_stale_zip(
            key,
            &mut zip_pages,
            immutable_roots,
            probes,
            budgets,
            witness_pages,
        )?;
    }
    Ok(())
}

fn flush_stale_zip(
    container_key: String,
    pages: &mut Vec<OraclePage>,
    immutable_roots: &[String],
    probes: &mut BTreeMap<[u8; 32], SignatureProbe>,
    budgets: &mut OracleBudgetState,
    witness_pages: &mut BTreeMap<u64, OraclePage>,
) -> Result<(), String> {
    apply_independent_stale_zip_order(&container_key, pages, budgets)?;
    for page in pages.drain(..) {
        consider_scannable_page(&page, immutable_roots, probes, budgets, witness_pages)?;
    }
    Ok(())
}

fn consider_scannable_page(
    page: &OraclePage,
    immutable_roots: &[String],
    probes: &mut BTreeMap<[u8; 32], SignatureProbe>,
    budgets: &mut OracleBudgetState,
    witness_pages: &mut BTreeMap<u64, OraclePage>,
) -> Result<(), String> {
    if page.hash_version != current_hash_version()
        || page.quality < BOOK_MIN_QUALITY
        || page.effective_page_index.is_none()
        || !key_is_under_any_independent(&page.container_key, immutable_roots)
    {
        return Ok(());
    }
    for probe in probes.values_mut() {
        if matches!(probe.state, ProbeState::Common(_)) {
            continue;
        }
        budgets.actual_hamming_comparisons = budgets
            .actual_hamming_comparisons
            .checked_add(1)
            .ok_or_else(|| "actual Hamming comparison count overflow".to_owned())?;
        let distance = hamming256(&probe.signature, &page.signature);
        if distance > BOOK_RADIUS {
            continue;
        }
        let promote = match &mut probe.state {
            ProbeState::Rare(books) => {
                let near = books
                    .entry(page.container_key.clone())
                    .or_insert_with(|| RareMatch {
                        count: 0,
                        witness: page.clone(),
                        distance,
                    });
                near.count = near
                    .count
                    .saturating_add(1)
                    .min(BOOK_MIN_MATCHED_PAGES as u8);
                (books.len() > BOOK_MAX_BOOKS_PER_PAGE as usize).then(|| {
                    books
                        .values()
                        .map(|near| OracleWitness {
                            item_id: near.witness.item_id,
                            container_key: near.witness.container_key.clone(),
                            effective_page_index: near
                                .witness
                                .effective_page_index
                                .expect("scannable witness has an index"),
                            distance: near.distance,
                            eligibility: "complete_current_hash_scoped_indexed_quality",
                        })
                        .collect::<Vec<_>>()
                })
            }
            ProbeState::Common(_) => None,
        };
        if let Some(witnesses) = promote {
            budgets.charge_page_units(
                u64::try_from(witnesses.len())
                    .map_err(|_| "common witness count exceeds u64".to_owned())?,
            )?;
            if let ProbeState::Rare(books) = &probe.state {
                for near in books.values() {
                    witness_pages
                        .entry(near.witness.item_id)
                        .or_insert_with(|| near.witness.clone());
                }
            }
            probe.state = ProbeState::Common(witnesses);
        }
    }
    Ok(())
}

fn probe_evidence(probes: &BTreeMap<[u8; 32], SignatureProbe>) -> Vec<OracleSignatureEvidence> {
    probes
        .values()
        .map(|probe| OracleSignatureEvidence {
            signature_hex: hex_bytes(&probe.signature),
            evidence: neighborhood_evidence(&probe.state),
        })
        .collect()
}

fn neighborhood_evidence(state: &ProbeState) -> OracleNeighborhoodEvidence {
    match state {
        ProbeState::Common(witnesses) => OracleNeighborhoodEvidence::Common {
            witnesses: witnesses.clone(),
        },
        ProbeState::Rare(books) => OracleNeighborhoodEvidence::Rare {
            books: books
                .iter()
                .map(|(container_key, near)| OracleRareBook {
                    container_key: container_key.clone(),
                    near_page_count_capped_at_three: near.count,
                })
                .collect(),
        },
    }
}

fn candidate_signature_evidence(
    candidate_pages: &[OraclePage],
    origin: &BTreeMap<[u8; 32], SignatureProbe>,
    local: &BTreeMap<[u8; 32], SignatureProbe>,
) -> Vec<OracleCandidateSignatureEvidence> {
    unique_quality_signatures(candidate_pages)
        .into_iter()
        .filter_map(|signature| {
            let reused = origin.contains_key(&signature);
            let probe = origin.get(&signature).or_else(|| local.get(&signature))?;
            Some(OracleCandidateSignatureEvidence {
                signature_hex: hex_bytes(&signature),
                reused_origin_evidence: reused,
                evidence: (!reused).then(|| neighborhood_evidence(&probe.state)),
            })
        })
        .collect()
}

fn common_witness_ids_for_pair(
    origin_pages: &[OraclePage],
    candidate_pages: &[OraclePage],
    origin: &BTreeMap<[u8; 32], SignatureProbe>,
    local: &BTreeMap<[u8; 32], SignatureProbe>,
) -> BTreeSet<u64> {
    origin_pages
        .iter()
        .chain(candidate_pages)
        .filter(|page| page.quality >= BOOK_MIN_QUALITY)
        .filter_map(|page| {
            origin
                .get(&page.signature)
                .or_else(|| local.get(&page.signature))
        })
        .filter_map(|probe| match &probe.state {
            ProbeState::Common(witnesses) => Some(witnesses),
            ProbeState::Rare(_) => None,
        })
        .flat_map(|witnesses| witnesses.iter().map(|witness| witness.item_id))
        .collect()
}

fn legacy_pair(
    origin: &OracleBook,
    candidate: &OracleBook,
    witness_ids: &BTreeSet<u64>,
    witness_pages: &BTreeMap<u64, OraclePage>,
    budgets: &mut OracleBudgetState,
) -> Result<BenchBookPair, String> {
    // One real container always maps to one classifier book across every signature. Origin and
    // candidate retain the product's fixed direction; witness rows never receive per-row IDs.
    let mut witness_containers = BTreeSet::new();
    for item_id in witness_ids {
        let page = witness_pages
            .get(item_id)
            .ok_or_else(|| format!("common witness item {item_id} was not retained"))?;
        if page.container_key != origin.container_key
            && page.container_key != candidate.container_key
        {
            witness_containers.insert(page.container_key.clone());
        }
    }
    let mut book_ids = BTreeMap::new();
    book_ids.insert(origin.container_key.clone(), BOOK_ORIGIN);
    book_ids.insert(candidate.container_key.clone(), BOOK_CANDIDATE);
    for (offset, container_key) in witness_containers.into_iter().enumerate() {
        let book_id = u32::try_from(offset)
            .ok()
            .and_then(|offset| offset.checked_add(BOOK_CANDIDATE + 1))
            .ok_or_else(|| "witness book id overflow".to_owned())?;
        book_ids.insert(container_key, book_id);
    }

    let mut pages = Vec::new();
    let mut seen_items = HashSet::new();
    append_legacy_book_pages(origin, BOOK_ORIGIN, &mut pages, &mut seen_items)?;
    append_legacy_book_pages(candidate, BOOK_CANDIDATE, &mut pages, &mut seen_items)?;
    for item_id in witness_ids {
        if !seen_items.insert(*item_id) {
            continue;
        }
        let page = witness_pages
            .get(item_id)
            .ok_or_else(|| format!("common witness item {item_id} was not retained"))?;
        let Some(index) = page.effective_page_index else {
            return Err(format!(
                "common witness item {item_id} has no effective index"
            ));
        };
        let book_id = *book_ids
            .get(&page.container_key)
            .ok_or_else(|| "common witness container id disappeared".to_owned())?;
        pages.push(book::BookPage {
            book: book_id,
            index,
            quality: page.quality,
            sig: Sig::Bits(Box::new(page.signature)),
        });
    }
    let eligible_count = pages
        .iter()
        .filter(|page| page.quality >= BOOK_MIN_QUALITY)
        .count();
    budgets.record_legacy_size(eligible_count)?;
    let params = book::Params {
        radius: BOOK_RADIUS,
        max_books_per_page: BOOK_MAX_BOOKS_PER_PAGE,
        min_quality: BOOK_MIN_QUALITY,
        coverage_threshold: BOOK_COVERAGE,
        min_matched_pages: BOOK_MIN_MATCHED_PAGES,
    };
    let pair = book::classify_pair(&pages, params, BOOK_ORIGIN, BOOK_CANDIDATE)
        .map_err(|error| format!("legacy pair oracle failed: {error}"))?;
    Ok(bench_pair(&pair))
}

fn append_legacy_book_pages(
    book: &OracleBook,
    book_id: u32,
    pages: &mut Vec<book::BookPage>,
    seen_items: &mut HashSet<u64>,
) -> Result<(), String> {
    for page in &book.eligible_pages {
        let Some(index) = page.effective_page_index else {
            continue;
        };
        if !seen_items.insert(page.item_id) {
            return Err(format!(
                "item {} appears in more than one oracle book",
                page.item_id
            ));
        }
        pages.push(book::BookPage {
            book: book_id,
            index,
            quality: page.quality,
            sig: Sig::Bits(Box::new(page.signature)),
        });
    }
    Ok(())
}

fn bench_pair(pair: &book::BookPair) -> BenchBookPair {
    BenchBookPair {
        a: pair.a,
        b: pair.b,
        matched: pair.matched,
        distinctive_a: pair.distinctive_a,
        distinctive_b: pair.distinctive_b,
        coverage_a_bits: pair.coverage_a.to_bits(),
        coverage_b_bits: pair.coverage_b.to_bits(),
        relation: match pair.relation {
            book::Relation::Same => BenchBookRelation::Same,
            book::Relation::Contains { whole } => BenchBookRelation::Contains { whole },
            book::Relation::Unrelated => BenchBookRelation::Unrelated,
            book::Relation::Undecidable => BenchBookRelation::Undecidable,
        },
        alignment: pair.alignment.clone(),
    }
}

fn build_expected_origin(
    pages: &[OraclePage],
    common_items: &HashSet<u64>,
) -> Vec<BenchBookOriginPage> {
    pages
        .iter()
        .map(|page| BenchBookOriginPage {
            item_key: page.item_key.clone(),
            baseline: if page.quality < BOOK_MIN_QUALITY
                || page.effective_page_index.is_none()
                || common_items.contains(&page.item_id)
            {
                BenchBookPageBaseline::Excluded
            } else {
                BenchBookPageBaseline::Unmatched
            },
        })
        .collect()
}

fn build_expected_hit(
    origin: &OracleBook,
    candidate: &OracleBook,
    pair: BenchBookPair,
) -> Result<BenchBookRelationHit, String> {
    let mut origin_slots = BTreeMap::new();
    for (slot, page) in origin.eligible_pages.iter().enumerate() {
        if let Some(index) = page.effective_page_index
            && origin_slots.insert(index, slot).is_some()
        {
            return Err(format!(
                "origin contains duplicate effective page index {index}"
            ));
        }
    }
    let mut candidate_pages = BTreeMap::new();
    for page in &candidate.eligible_pages {
        if let Some(index) = page.effective_page_index
            && candidate_pages.insert(index, page).is_some()
        {
            return Err(format!(
                "candidate contains duplicate effective page index {index}"
            ));
        }
    }
    let mut overrides = Vec::with_capacity(pair.alignment.len());
    for &(origin_page, candidate_page) in &pair.alignment {
        let slot = *origin_slots.get(&origin_page).ok_or_else(|| {
            format!("legacy alignment origin page {origin_page} is absent from oracle book")
        })?;
        let other = candidate_pages.get(&candidate_page).ok_or_else(|| {
            format!("legacy alignment candidate page {candidate_page} is absent from oracle book")
        })?;
        let distance = hamming256(&origin.eligible_pages[slot].signature, &other.signature);
        let (other_target, _) = independent_target(other);
        overrides.push(BenchBookPageMatch {
            origin_slot: slot,
            state: if distance <= NEARLY_IDENTICAL_MAX_DISTANCE {
                BenchBookPageMatchState::Strong
            } else {
                BenchBookPageMatchState::Weak
            },
            other_page_index: candidate_page,
            other_target,
            other_item_key: other.item_key.clone(),
            other_mtime: other.mtime,
            other_file_size: other.file_size,
        });
    }
    overrides.sort_by_key(|entry| entry.origin_slot);
    Ok(BenchBookRelationHit {
        other_container_key: candidate.container_key.clone(),
        other_page_count: u32::try_from(candidate.eligible_pages.len())
            .map_err(|_| "candidate page count exceeds u32".to_owned())?,
        pair,
        overrides,
    })
}

fn book_certificate(book: &OracleBook) -> OracleBookCertificate {
    OracleBookCertificate {
        container_key: book.container_key.clone(),
        container_kind: book.container_kind,
        scan_state: book.scan_state,
        raw_pages: book.raw_pages.iter().map(page_certificate).collect(),
        product_eligible_pages: book.eligible_pages.len() as u64,
        legacy_indexed_pages: book
            .eligible_pages
            .iter()
            .filter(|page| page.effective_page_index.is_some())
            .count() as u64,
    }
}

fn page_certificate(page: &OraclePage) -> OraclePageCertificate {
    let (target, target_path_resolution) = independent_target(page);
    OraclePageCertificate {
        item_id: page.item_id,
        revision: page.revision,
        item_key: page.item_key.clone(),
        item_kind: page.item_kind,
        container_key: page.container_key.clone(),
        stored_page_index: page.stored_page_index,
        effective_page_index: page.effective_page_index,
        mtime: page.mtime,
        file_size: page.file_size,
        hash_version: page.hash_version,
        signature_hex: hex_bytes(&page.signature),
        quality: page.quality,
        width: page.width,
        height: page.height,
        format: page.format,
        eligibility: if page.hash_version != current_hash_version() {
            OraclePageEligibility::HashMismatch
        } else if page.effective_page_index.is_none() {
            OraclePageEligibility::CurrentHashWithoutEffectiveIndex
        } else {
            OraclePageEligibility::CurrentHashIndexed
        },
        target,
        target_path_resolution,
    }
}

fn independent_target(
    page: &OraclePage,
) -> (Option<BenchSimilarItemTarget>, OracleTargetPathResolution) {
    match page.item_kind {
        ITEM_KIND_IMAGE => {
            let (path, resolution) = canonicalize_or_fallback(Path::new(&page.item_key));
            (Some(BenchSimilarItemTarget::File { path }), resolution)
        }
        ITEM_KIND_ZIP_PAGE => {
            let Some((zip_path, entry_name)) = page.item_key.split_once(ZIP_ENTRY_SEP) else {
                return (None, OracleTargetPathResolution::InvalidItemKey);
            };
            let (zip_path, resolution) = canonicalize_or_fallback(Path::new(zip_path));
            (
                Some(BenchSimilarItemTarget::ZipPage {
                    zip_path,
                    entry_name: entry_name.to_owned(),
                }),
                resolution,
            )
        }
        ITEM_KIND_PDF_PAGE => {
            let Some((pdf_path, suffix)) = page.item_key.split_once(ZIP_ENTRY_SEP) else {
                return (None, OracleTargetPathResolution::InvalidItemKey);
            };
            let Some(page_num) = suffix
                .strip_prefix("pdf:")
                .and_then(|number| number.parse::<u32>().ok())
            else {
                return (None, OracleTargetPathResolution::InvalidItemKey);
            };
            let (pdf_path, resolution) = canonicalize_or_fallback(Path::new(pdf_path));
            (
                Some(BenchSimilarItemTarget::PdfPage { pdf_path, page_num }),
                resolution,
            )
        }
        _ => (None, OracleTargetPathResolution::InvalidItemKey),
    }
}

fn canonicalize_or_fallback(path: &Path) -> (String, OracleTargetPathResolution) {
    match std::fs::canonicalize(path) {
        Ok(path) => (
            strip_windows_verbatim_prefix(path).display().to_string(),
            OracleTargetPathResolution::CanonicalizedExistingPath,
        ),
        Err(_) => (
            path.display().to_string(),
            OracleTargetPathResolution::RawPathFallback,
        ),
    }
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

fn key_is_under_any_independent(key: &str, roots: &[String]) -> bool {
    roots.iter().any(|root| {
        key == root
            || key
                .strip_prefix(root)
                .is_some_and(|suffix| root.ends_with('/') || suffix.starts_with('/'))
    })
}

fn hamming256(left: &[u8; 32], right: &[u8; 32]) -> u32 {
    left.iter()
        .zip(right)
        .map(|(left, right)| (left ^ right).count_ones())
        .sum()
}

fn hex_bytes(bytes: &[u8]) -> String {
    let mut value = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        use std::fmt::Write as _;
        let _ = write!(value, "{byte:02x}");
    }
    value
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::similar_book_query_bench::{
        BenchInputSpec, GuardedBenchInputs, lower_product_book_query_worker_priority,
    };
    use crate::similar_db::{ContainerKind, ItemKind, SimilarDb, StoredItem};
    use crate::similar_search_array::{base_path, load_or_rebuild};

    fn page(item_id: u64, book: &str, index: Option<u32>, marker: u8) -> OraclePage {
        OraclePage {
            item_id,
            revision: 1,
            item_key: format!("{book}{ZIP_ENTRY_SEP}{marker}.jpg"),
            item_kind: ITEM_KIND_ZIP_PAGE,
            container_key: book.to_owned(),
            stored_page_index: index,
            effective_page_index: index,
            mtime: 10,
            file_size: 20,
            hash_version: current_hash_version(),
            signature: [marker; 32],
            quality: 50,
            width: 100,
            height: 200,
            format: 1,
        }
    }

    fn stage_zip(db: &SimilarDb, book: &str, signatures: &[[u8; 32]]) {
        let generation = db
            .begin_container_build(book, ContainerKind::Zip, signatures.len() as u32, 10, 20)
            .unwrap();
        for (index, signature) in signatures.iter().enumerate() {
            db.stage_item(
                generation,
                &StoredItem {
                    item_key: format!("{book}{ZIP_ENTRY_SEP}{index}.jpg"),
                    kind: ItemKind::ZipPage,
                    container_key: Some(book.to_owned()),
                    page_index: Some(index as u32),
                    mtime: 10,
                    file_size: 20,
                    hash_version: current_hash_version(),
                    pdq256: *signature,
                    quality: 50,
                    width: 100,
                    height: 200,
                    format: 1,
                },
            )
            .unwrap();
        }
        db.complete_container(book, generation).unwrap();
    }

    #[test]
    fn independent_probe_distinguishes_rare_eight_from_common_nine_and_caps_edges() {
        let signature = [0x55; 32];
        let mut probes = build_probes(BTreeSet::from([signature]));
        let mut budgets = OracleBudgetState::new(VerificationBudgets::default());
        let mut witnesses = BTreeMap::new();
        let roots = vec!["c:/library".to_owned()];
        for book in 0..8 {
            for page_index in 0..5 {
                let mut row = page(
                    book * 10 + page_index + 1,
                    &format!("c:/library/book-{book}"),
                    Some(page_index as u32),
                    0x55,
                );
                row.signature = signature;
                consider_scannable_page(&row, &roots, &mut probes, &mut budgets, &mut witnesses)
                    .unwrap();
            }
        }
        let ProbeState::Rare(books) = &probes[&signature].state else {
            panic!("eight real books must remain rare");
        };
        assert_eq!(books.len(), 8);
        assert!(books.values().all(|near| near.count == 3));

        let mut ninth = page(999, "c:/library/book-8", Some(0), 0x55);
        ninth.signature = signature;
        consider_scannable_page(&ninth, &roots, &mut probes, &mut budgets, &mut witnesses).unwrap();
        let ProbeState::Common(common) = &probes[&signature].state else {
            panic!("ninth distinct real book must make the signature common");
        };
        assert_eq!(common.len(), 9);
        assert_eq!(
            common
                .iter()
                .map(|witness| witness.container_key.as_str())
                .collect::<BTreeSet<_>>()
                .len(),
            9
        );
        assert_eq!(witnesses.len(), 9);
        assert_eq!(budgets.certificate_page_units, 9);
    }

    #[test]
    fn independent_stale_zip_order_allocates_old_hash_holes_and_current_none() {
        let book = "c:/library/book.zip";
        let mut old = page(10, book, Some(0), 1);
        old.item_key = format!("{book}{ZIP_ENTRY_SEP}1.jpg");
        old.hash_version = current_hash_version() - 1;
        let mut current_none = page(11, book, None, 2);
        current_none.item_key = format!("{book}{ZIP_ENTRY_SEP}2.jpg");
        let mut current_ten = page(12, book, Some(2), 10);
        current_ten.item_key = format!("{book}{ZIP_ENTRY_SEP}10.jpg");
        let mut equal_tie = page(13, book, Some(3), 20);
        // Duplicate keys cannot occur in the production table, but using one here isolates the
        // stable-sort tie rule from Windows collation details.
        equal_tie.item_key = format!("{book}{ZIP_ENTRY_SEP}10.jpg");
        let mut pages = vec![old, current_none, current_ten, equal_tie];
        let budgets = OracleBudgetState::new(VerificationBudgets::default());
        apply_independent_stale_zip_order(book, &mut pages, &budgets).unwrap();

        let by_id = pages
            .into_iter()
            .map(|page| (page.item_id, page.effective_page_index))
            .collect::<BTreeMap<_, _>>();
        assert_eq!(by_id[&10], Some(0), "old hash row reserves ordinal zero");
        assert_eq!(by_id[&11], Some(1), "current None row becomes eligible");
        assert_eq!(by_id[&12], Some(2));
        assert_eq!(
            by_id[&13],
            Some(3),
            "natural-equal key preserves stored page_index/item_id tie order"
        );
    }

    #[test]
    fn legacy_pair_budget_is_checked_before_dense_oracle() {
        let mut limits = VerificationBudgets::default();
        limits.max_legacy_possible_pairs = 3;
        let mut budgets = OracleBudgetState::new(limits);
        budgets.record_legacy_size(3).unwrap();
        let error = budgets.record_legacy_size(3).unwrap_err();
        assert!(error.contains("all legacy certificates"));
    }

    #[test]
    fn legacy_pair_and_independent_sparse_strip_preserve_slots_and_all_fields() {
        let origin_key = "c:/library/origin.zip";
        let candidate_key = "c:/library/candidate.zip";
        let mut without_index = page(1, origin_key, None, 0x33);
        without_index.item_key = format!("{origin_key}{ZIP_ENTRY_SEP}cover.jpg");
        let signatures = [[0x00; 32], [0xff; 32], [0x0f; 32]];
        let mut origin_pages = vec![without_index];
        let mut candidate_pages = Vec::new();
        for (slot, signature) in signatures.into_iter().enumerate() {
            let mut origin_page = page(10 + slot as u64, origin_key, Some(slot as u32), 0);
            origin_page.signature = signature;
            origin_page.item_key = format!("{origin_key}{ZIP_ENTRY_SEP}{slot}.jpg");
            origin_pages.push(origin_page);

            let mut candidate_page = page(20 + slot as u64, candidate_key, Some(slot as u32), 0);
            candidate_page.signature = signature;
            candidate_page.item_key = format!("{candidate_key}{ZIP_ENTRY_SEP}{slot}.jpg");
            candidate_page.mtime = 100 + slot as i64;
            candidate_page.file_size = 200 + slot as i64;
            candidate_pages.push(candidate_page);
        }
        let origin = OracleBook {
            container_key: origin_key.to_owned(),
            container_kind: CONTAINER_KIND_ZIP,
            scan_state: SCAN_STATE_COMPLETE,
            raw_pages: origin_pages.clone(),
            eligible_pages: origin_pages,
        };
        let candidate = OracleBook {
            container_key: candidate_key.to_owned(),
            container_kind: CONTAINER_KIND_ZIP,
            scan_state: SCAN_STATE_COMPLETE,
            raw_pages: candidate_pages.clone(),
            eligible_pages: candidate_pages,
        };
        let mut budgets = OracleBudgetState::new(VerificationBudgets::default());
        let pair = legacy_pair(
            &origin,
            &candidate,
            &BTreeSet::new(),
            &BTreeMap::new(),
            &mut budgets,
        )
        .unwrap();
        assert_eq!(pair.a, BOOK_ORIGIN);
        assert_eq!(pair.b, BOOK_CANDIDATE);
        assert_eq!(pair.matched, 3);
        assert_eq!(pair.distinctive_a, 3);
        assert_eq!(pair.distinctive_b, 3);
        assert_eq!(pair.coverage_a_bits, 1.0f32.to_bits());
        assert_eq!(pair.coverage_b_bits, 1.0f32.to_bits());
        assert_eq!(pair.relation, BenchBookRelation::Same);
        assert_eq!(pair.alignment, vec![(0, 0), (1, 1), (2, 2)]);

        let origin_strip = build_expected_origin(&origin.eligible_pages, &HashSet::new());
        assert_eq!(origin_strip[0].baseline, BenchBookPageBaseline::Excluded);
        assert!(
            origin_strip[1..]
                .iter()
                .all(|page| page.baseline == BenchBookPageBaseline::Unmatched)
        );
        let hit = build_expected_hit(&origin, &candidate, pair).unwrap();
        assert_eq!(hit.other_container_key, candidate_key);
        assert_eq!(hit.other_page_count, 3);
        assert_eq!(hit.overrides.len(), 3);
        for (slot, page_match) in hit.overrides.iter().enumerate() {
            assert_eq!(page_match.origin_slot, slot + 1);
            assert_eq!(page_match.state, BenchBookPageMatchState::Strong);
            assert_eq!(page_match.other_page_index, slot as u32);
            assert_eq!(page_match.other_mtime, 100 + slot as i64);
            assert_eq!(page_match.other_file_size, 200 + slot as i64);
            assert_eq!(
                page_match.other_item_key,
                format!("{candidate_key}{ZIP_ENTRY_SEP}{slot}.jpg")
            );
            assert!(matches!(
                page_match.other_target,
                Some(BenchSimilarItemTarget::ZipPage { .. })
            ));
        }
    }

    #[test]
    fn bounded_json_writer_stops_before_exceeding_the_cap() {
        let mut bytes = Vec::new();
        let error = write_json_bounded(&mut bytes, &"abcdefghij", 4).unwrap_err();
        assert!(error.contains("verification JSON exceeds"));
        assert!(bytes.len() <= 4);
    }

    #[test]
    fn current_order_none_rows_match_the_existing_sql_order_without_dropping_slots() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("similar.db");
        let book = "c:/library/book";
        let db = SimilarDb::open_at(&path).unwrap();
        let generation = db
            .begin_container_build(book, ContainerKind::ImageFolder, 3, 1, 1)
            .unwrap();
        for index in 0..3 {
            db.stage_item(
                generation,
                &StoredItem {
                    item_key: format!("{book}/{index}.jpg"),
                    kind: ItemKind::Image,
                    container_key: Some(book.to_owned()),
                    page_index: Some(index),
                    mtime: 1,
                    file_size: 1,
                    hash_version: current_hash_version(),
                    pdq256: [index as u8; 32],
                    quality: 50,
                    width: 1,
                    height: 1,
                    format: 1,
                },
            )
            .unwrap();
        }
        db.complete_container(book, generation).unwrap();
        drop(db);
        let conn = Connection::open(&path).unwrap();
        conn.execute(
            "UPDATE item SET page_index=NULL WHERE item_key IN (?1,?2)",
            params![format!("{book}/0.jpg"), format!("{book}/1.jpg")],
        )
        .unwrap();
        drop(conn);

        let db = SimilarDb::open_at(&path).unwrap();
        let product = db
            .load_book_pages(book, current_hash_version())
            .unwrap()
            .into_iter()
            .map(|row| row.item.item_key)
            .collect::<Vec<_>>();
        drop(db);
        let conn = Connection::open(&path).unwrap();
        let mut budgets = OracleBudgetState::new(VerificationBudgets::default());
        let oracle = load_book(&conn, book, PAGE_ORDER_VERSION, &mut budgets)
            .unwrap()
            .unwrap()
            .eligible_pages
            .into_iter()
            .map(|page| page.item_key)
            .collect::<Vec<_>>();
        assert_eq!(oracle, product);
        assert_eq!(oracle.len(), 3, "current-hash None slots remain visible");
    }

    #[test]
    fn full_verifier_skips_out_of_scope_stale_zip_before_its_page_cap() {
        let temp = tempfile::tempdir().unwrap();
        let database_path = SimilarDb::db_path_at(temp.path());
        let base = base_path(temp.path());
        let origin = "c:/library/origin.zip";
        let candidate = "c:/library/candidate.zip";
        let signatures = [[0x00; 32], [0xff; 32], [0x0f; 32]];
        let db = SimilarDb::open_at(&database_path).unwrap();
        stage_zip(&db, origin, &signatures);
        stage_zip(&db, candidate, &signatures);
        stage_zip(
            &db,
            "c:/outside/over-cap.zip",
            &[[0x00; 32], [0xff; 32], [0x0f; 32], [0xf0; 32]],
        );
        drop(load_or_rebuild(&db, &base).unwrap());
        drop(db);

        let checkpoint = Connection::open(&database_path).unwrap();
        let journal_mode: String = checkpoint
            .query_row("PRAGMA journal_mode=DELETE", [], |row| row.get(0))
            .unwrap();
        assert_eq!(journal_mode.to_ascii_lowercase(), "delete");
        checkpoint
            .execute(
                "UPDATE search_content_state SET page_order_version=0 WHERE singleton=1",
                [],
            )
            .unwrap();
        drop(checkpoint);

        let guarded = GuardedBenchInputs::acquire(
            BenchInputSpec {
                db_path: database_path.clone(),
                active_base_paths: vec![base],
                retained_base_paths: Vec::new(),
                immutable_roots: vec!["c:/library".to_owned()],
            },
            &temp.path().join("certificate.json"),
        )
        .unwrap();
        let config = guarded.engine_config();
        let mut engine = BenchEngine::open(config.clone()).unwrap();
        engine.validate_origin(origin).unwrap();
        let measured = engine.query(origin).unwrap().into_certificate();
        drop(engine);
        assert_eq!(measured.summary.outcome, "ready");
        assert_eq!(measured.summary.hit_count, 1);
        let expected_digest = measured.summary.result_sha256;

        let oracle_path = guarded.database_path().to_path_buf();
        let report = std::thread::spawn(move || {
            lower_product_book_query_worker_priority();
            let mut budgets = VerificationBudgets::default();
            budgets.max_stale_zip_pages = 3;
            verify_real_book(
                config,
                &oracle_path,
                VerificationSpec {
                    origin: origin.to_owned(),
                    immutable_roots: vec!["c:/library".to_owned()],
                    expected_result_sha256: expected_digest,
                    budgets,
                },
            )
        })
        .join()
        .unwrap()
        .unwrap();
        assert_eq!(report.oracle.candidates.len(), 1);
        assert_eq!(report.oracle.candidates[0].container_key, candidate);
        assert!(
            report
                .oracle
                .witness_pages
                .iter()
                .all(|page| page.container_key != "c:/outside/over-cap.zip")
        );
        guarded.verify_unchanged().unwrap();
    }
}
