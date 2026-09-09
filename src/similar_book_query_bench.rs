//! Opt-in, read-only measurement facade for the product similar-book engine.
//!
//! This module is compiled only by `dev-tools`. It deliberately accepts no application data
//! directory and owns no writer connection. The caller must hold [`GuardedBenchInputs`] until the
//! worker and its SQLite reader have stopped, then verify the same guarded bytes before releasing
//! the Windows sharing locks.

use std::collections::{HashMap, HashSet};
use std::ffi::{OsString, c_void};
use std::fs::{File, OpenOptions};
use std::io::{BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::{Arc, atomic::AtomicBool};

use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::dupe::book::Relation;
use crate::similar_book_engine::{EngineObservation, SimilarBookQueryEngine};
use crate::similar_db::{BookReadMetadata, SimilarBookReader, key_is_under_any};
use crate::similar_index::{BookPageBaseline, BookPageMatchState, BookQuery, SimilarItemTarget};
use crate::similar_search_array::{SearchSnapshot, read_book_query_benchmark_base};

#[derive(Clone, Debug)]
pub struct BenchInputSpec {
    pub db_path: PathBuf,
    pub active_base_paths: Vec<PathBuf>,
    pub retained_base_paths: Vec<PathBuf>,
    pub immutable_roots: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct BenchEngineConfig {
    db_path: PathBuf,
    active_bases: Vec<GuardedBaseSpec>,
    retained_bases: Vec<GuardedBaseSpec>,
    immutable_roots: Vec<String>,
}

#[derive(Clone, Debug)]
struct GuardedBaseSpec {
    path: PathBuf,
    identity: FileIdentity,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct BenchInputFingerprint {
    pub roles: Vec<String>,
    pub path: String,
    pub bytes: u64,
    pub sha256: String,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct BenchBaseInfo {
    pub role: String,
    pub path: String,
    pub records: u64,
    pub base_seq: u64,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct BenchCacheObservation {
    pub source: String,
    pub base_seq: u64,
    pub snapshot_seq: u64,
    pub records: u64,
    pub reused_snapshot_owner: bool,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct BenchQuerySummary {
    pub store_id_hex: String,
    pub read_seq: u64,
    pub page_order_version: i64,
    pub outcome: String,
    pub origin_pages: u64,
    pub hit_count: u64,
    pub override_count: u64,
    pub result_sha256: String,
}

/// Owned product result used by the independent real-book verifier.
///
/// This mirrors only the externally meaningful result fields. It deliberately carries no MIH,
/// SQLite, or sparse-strip implementation state into the verifier.
#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct BenchQueryCertificate {
    pub metadata: BenchBookReadMetadata,
    pub summary: BenchQuerySummary,
    pub outcome: BenchBookQueryOutcome,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct BenchBookReadMetadata {
    pub store_id_hex: String,
    pub read_seq: u64,
    pub page_order_version: i64,
}

#[derive(Clone, Debug, Serialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum BenchBookQueryOutcome {
    Preparing,
    Featureless,
    NotIndexed,
    NotBook,
    Failed { message: String },
    Ready { relations: BenchBookRelations },
    EngineError { message: String },
}

#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct BenchBookRelations {
    pub origin_pages: Vec<BenchBookOriginPage>,
    pub hits: Vec<BenchBookRelationHit>,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct BenchBookOriginPage {
    pub item_key: String,
    pub baseline: BenchBookPageBaseline,
}

#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BenchBookPageBaseline {
    Unmatched,
    Excluded,
}

#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct BenchBookRelationHit {
    pub other_container_key: String,
    pub other_page_count: u32,
    pub pair: BenchBookPair,
    pub overrides: Vec<BenchBookPageMatch>,
}

#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct BenchBookPair {
    pub a: u32,
    pub b: u32,
    pub matched: u32,
    pub distinctive_a: u32,
    pub distinctive_b: u32,
    pub coverage_a_bits: u32,
    pub coverage_b_bits: u32,
    pub relation: BenchBookRelation,
    pub alignment: Vec<(u32, u32)>,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum BenchBookRelation {
    Same,
    Contains { whole: u32 },
    Unrelated,
    Undecidable,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct BenchBookPageMatch {
    pub origin_slot: usize,
    pub state: BenchBookPageMatchState,
    pub other_page_index: u32,
    pub other_target: Option<BenchSimilarItemTarget>,
    pub other_item_key: String,
    pub other_mtime: i64,
    pub other_file_size: i64,
}

#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BenchBookPageMatchState {
    Strong,
    Weak,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum BenchSimilarItemTarget {
    File {
        path: String,
    },
    ZipPage {
        zip_path: String,
        entry_name: String,
    },
    PdfPage {
        pdf_path: String,
        page_num: u32,
    },
}

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
struct FileIdentity {
    volume: u32,
    index: u64,
}

struct GuardedFile {
    path: PathBuf,
    identity: FileIdentity,
    roles: Vec<String>,
    file: File,
    initial: BenchInputFingerprint,
}

/// Canonical input paths and read handles that deny concurrent write/delete opens on Windows.
pub struct GuardedBenchInputs {
    db_path: PathBuf,
    active_bases: Vec<GuardedBaseSpec>,
    retained_bases: Vec<GuardedBaseSpec>,
    immutable_roots: Vec<String>,
    guarded: Vec<GuardedFile>,
    sidecars: [PathBuf; 2],
    output_path: PathBuf,
    output: File,
}

impl GuardedBenchInputs {
    /// Resolves every input, acquires every read-sharing guard, rejects WAL sidecars, fingerprints
    /// the guarded bytes, and finally creates a new output file outside all protected identities.
    pub fn acquire(spec: BenchInputSpec, output_path: &Path) -> Result<Self, String> {
        if !cfg!(windows) {
            return Err(
                "similar-book adoption measurement requires Windows file sharing guards".to_owned(),
            );
        }
        if spec.active_base_paths.is_empty() {
            return Err("at least one --base is required".to_owned());
        }
        if spec.immutable_roots.is_empty()
            || spec.immutable_roots.iter().any(|root| root.is_empty())
        {
            return Err("at least one non-empty logical --root is required".to_owned());
        }

        let db_path = canonical_regular_file(&spec.db_path, "database")?;
        let active_paths = spec
            .active_base_paths
            .iter()
            .map(|path| canonical_regular_file(path, "active base"))
            .collect::<Result<Vec<_>, _>>()?;
        let retained_paths = spec
            .retained_base_paths
            .iter()
            .map(|path| canonical_regular_file(path, "retained base"))
            .collect::<Result<Vec<_>, _>>()?;
        let output_path = resolve_new_output_path(output_path)?;
        let sidecars = [
            append_suffix(&db_path, "-wal"),
            append_suffix(&db_path, "-shm"),
        ];

        let mut resolved = Vec::<(PathBuf, Vec<String>)>::new();
        add_resolved_role(&mut resolved, db_path.clone(), "database".to_owned());
        for (index, path) in active_paths.iter().enumerate() {
            add_resolved_role(&mut resolved, path.clone(), format!("active_base[{index}]"));
        }
        for (index, path) in retained_paths.iter().enumerate() {
            add_resolved_role(
                &mut resolved,
                path.clone(),
                format!("retained_base[{index}]"),
            );
        }

        let forbidden_outputs = resolved
            .iter()
            .map(|(path, _)| path.clone())
            .chain(sidecars.iter().cloned())
            .collect::<Vec<_>>();
        ensure_output_is_distinct(&output_path, &forbidden_outputs)?;

        // Acquire every guard before checking sidecars or hashing. A writer already holding the
        // database fails this acquisition because its desired-write access conflicts with our
        // FILE_SHARE_READ-only handle.
        let mut guarded = Vec::with_capacity(resolved.len());
        for (path, roles) in resolved {
            let file = open_read_sharing_guard(&path)?;
            let identity = file_identity(&file)?;
            guarded.push(GuardedFile {
                path,
                identity,
                roles,
                file,
                initial: BenchInputFingerprint {
                    roles: Vec::new(),
                    path: String::new(),
                    bytes: 0,
                    sha256: String::new(),
                },
            });
        }
        let db_guard = guarded
            .iter()
            .find(|guarded| guarded.path == db_path)
            .expect("canonical database path was added to the guarded inputs");
        ensure_rollback_journal_header(db_guard)?;
        ensure_sidecars_absent(&sidecars)?;

        for guarded_file in &mut guarded {
            guarded_file.initial = fingerprint_guarded_file(guarded_file)?;
        }

        let identity_by_path = guarded
            .iter()
            .map(|guarded| (guarded.path.clone(), guarded.identity))
            .collect::<HashMap<_, _>>();
        let active_bases = active_paths
            .into_iter()
            .map(|path| GuardedBaseSpec {
                identity: identity_by_path[&path],
                path,
            })
            .collect();
        let retained_bases = retained_paths
            .into_iter()
            .map(|path| GuardedBaseSpec {
                identity: identity_by_path[&path],
                path,
            })
            .collect();

        let output = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&output_path)
            .map_err(|error| {
                format!(
                    "measurement output create failed for {}: {error}",
                    output_path.display()
                )
            })?;

        Ok(Self {
            db_path,
            active_bases,
            retained_bases,
            immutable_roots: spec.immutable_roots,
            guarded,
            sidecars,
            output_path,
            output,
        })
    }

    pub fn engine_config(&self) -> BenchEngineConfig {
        BenchEngineConfig {
            db_path: self.db_path.clone(),
            active_bases: self.active_bases.clone(),
            retained_bases: self.retained_bases.clone(),
            immutable_roots: self.immutable_roots.clone(),
        }
    }

    pub fn initial_fingerprints(&self) -> Vec<BenchInputFingerprint> {
        self.guarded
            .iter()
            .map(|guarded| guarded.initial.clone())
            .collect()
    }

    pub fn output_path(&self) -> &Path {
        &self.output_path
    }

    pub fn output_file(&mut self) -> &mut File {
        &mut self.output
    }

    /// Canonical database path held by this guard, for a separate read-only oracle connection.
    pub fn database_path(&self) -> &Path {
        &self.db_path
    }

    /// Re-hashes bytes while every read-sharing guard is still held.
    ///
    /// The caller must first drop/join the engine worker and stop the memory sampler. This method
    /// also verifies that no SQLite WAL sidecar appeared while the guarded reader was active.
    pub fn verify_unchanged(&self) -> Result<Vec<BenchInputFingerprint>, String> {
        ensure_sidecars_absent(&self.sidecars)?;
        let mut final_fingerprints = Vec::with_capacity(self.guarded.len());
        for guarded in &self.guarded {
            let final_fingerprint = fingerprint_guarded_file(guarded)?;
            if final_fingerprint.bytes != guarded.initial.bytes
                || final_fingerprint.sha256 != guarded.initial.sha256
            {
                return Err(format!(
                    "guarded measurement input changed: {}",
                    guarded.path.display()
                ));
            }
            final_fingerprints.push(final_fingerprint);
        }
        Ok(final_fingerprints)
    }
}

pub struct BenchEngine {
    engine: SimilarBookQueryEngine,
    active_bases: Vec<LoadedBenchBase>,
    active_snapshots: Vec<Arc<SearchSnapshot>>,
    _retained_bases: Vec<LoadedBenchBase>,
    immutable_roots: Vec<String>,
    previous_snapshot_owner: Option<usize>,
}

struct LoadedBenchBase {
    role: String,
    path: PathBuf,
    snapshot: Arc<SearchSnapshot>,
}

pub struct BenchQueryRun {
    observation: EngineObservation,
}

impl BenchEngine {
    /// Opens the product read-only engine and loads only the explicitly guarded base files.
    pub fn open(config: BenchEngineConfig) -> Result<Self, String> {
        let mut metadata_reader = SimilarBookReader::open_at(&config.db_path)
            .map_err(|error| error.to_string())?
            .ok_or_else(|| "guarded measurement database disappeared".to_owned())?;
        let metadata = metadata_reader
            .with_snapshot(Arc::new(AtomicBool::new(false)), |read| Ok(read.metadata()))
            .map_err(|error| error.to_string())?;
        drop(metadata_reader);

        let mut loaded_by_identity = HashMap::<FileIdentity, Arc<SearchSnapshot>>::new();
        let mut load_base = |spec: &GuardedBaseSpec| -> Result<Arc<SearchSnapshot>, String> {
            if let Some(snapshot) = loaded_by_identity.get(&spec.identity) {
                return Ok(Arc::clone(snapshot));
            }
            let snapshot =
                read_book_query_benchmark_base(&spec.path, metadata.store_id).map_err(|error| {
                    format!(
                        "measurement base rejected at {}: {error}",
                        spec.path.display()
                    )
                })?;
            loaded_by_identity.insert(spec.identity, Arc::clone(&snapshot));
            Ok(snapshot)
        };

        let mut active_bases = Vec::with_capacity(config.active_bases.len());
        let mut active_seen = HashSet::new();
        for (index, spec) in config.active_bases.iter().enumerate() {
            if active_seen.insert(spec.identity) {
                active_bases.push(LoadedBenchBase {
                    role: format!("active_base[{index}]"),
                    path: spec.path.clone(),
                    snapshot: load_base(spec)?,
                });
            }
        }
        let mut retained_bases = Vec::with_capacity(config.retained_bases.len());
        let mut retained_seen = HashSet::new();
        for (index, spec) in config.retained_bases.iter().enumerate() {
            if retained_seen.insert(spec.identity) {
                retained_bases.push(LoadedBenchBase {
                    role: format!("retained_base[{index}]"),
                    path: spec.path.clone(),
                    snapshot: load_base(spec)?,
                });
            }
        }
        drop(load_base);
        drop(loaded_by_identity);

        let engine = SimilarBookQueryEngine::open_at(&config.db_path)
            .map_err(|error| error.to_string())?
            .ok_or_else(|| "guarded measurement database disappeared".to_owned())?;
        let active_snapshots = active_bases
            .iter()
            .map(|base| Arc::clone(&base.snapshot))
            .collect();
        Ok(Self {
            engine,
            active_bases,
            active_snapshots,
            _retained_bases: retained_bases,
            immutable_roots: config.immutable_roots,
            previous_snapshot_owner: None,
        })
    }

    pub fn base_infos(&self) -> Vec<BenchBaseInfo> {
        self.active_bases
            .iter()
            .chain(self._retained_bases.iter())
            .map(|base| BenchBaseInfo {
                role: base.role.clone(),
                path: base.path.display().to_string(),
                records: u64::try_from(base.snapshot.record_count()).unwrap_or(u64::MAX),
                base_seq: base.snapshot.base.applied_seq,
            })
            .collect()
    }

    pub fn validate_origin(&self, origin: &str) -> Result<(), String> {
        if key_is_under_any(origin, &self.immutable_roots) {
            Ok(())
        } else {
            Err("origin is outside every explicit logical root".to_owned())
        }
    }

    /// This call contains exactly one product `SimilarBookQueryEngine::query` invocation.
    /// Callers should put the query-core wall/CPU boundary immediately around this method.
    pub fn query(&mut self, origin: &str) -> Result<BenchQueryRun, String> {
        let observation = self
            .engine
            .query(
                origin,
                &self.immutable_roots,
                &self.active_snapshots,
                Arc::new(AtomicBool::new(false)),
            )
            .map_err(|error| error.to_string())?;
        Ok(BenchQueryRun { observation })
    }

    /// Reads cache identity after the query-core timer has stopped.
    pub fn cache_observation(&mut self) -> Option<BenchCacheObservation> {
        let cached = self.engine.benchmark_cached_snapshot()?;
        let snapshot_owner = Arc::as_ptr(&cached) as usize;
        let reused_snapshot_owner = self.previous_snapshot_owner == Some(snapshot_owner);
        self.previous_snapshot_owner = Some(snapshot_owner);
        let source = self
            .active_bases
            .iter()
            .find(|base| Arc::ptr_eq(&base.snapshot.base, &cached.base))
            .map_or_else(|| "same_tx_private".to_owned(), |base| base.role.clone());
        Some(BenchCacheObservation {
            source,
            base_seq: cached.base.applied_seq,
            snapshot_seq: cached.applied_seq,
            records: u64::try_from(cached.record_count()).unwrap_or(u64::MAX),
            reused_snapshot_owner,
        })
    }
}

impl BenchQueryRun {
    pub fn into_summary(self) -> BenchQuerySummary {
        summarize_observation(&self.observation)
    }

    pub fn into_certificate(self) -> BenchQueryCertificate {
        let summary = summarize_observation(&self.observation);
        let metadata = BenchBookReadMetadata {
            store_id_hex: hex_bytes(&self.observation.metadata.store_id),
            read_seq: self.observation.metadata.read_seq,
            page_order_version: self.observation.metadata.page_order_version,
        };
        let outcome = certificate_outcome(self.observation.outcome);
        BenchQueryCertificate {
            metadata,
            summary,
            outcome,
        }
    }
}

fn certificate_outcome(
    outcome: Result<BookQuery, crate::similar_book_engine::EngineError>,
) -> BenchBookQueryOutcome {
    match outcome {
        Ok(BookQuery::Preparing) => BenchBookQueryOutcome::Preparing,
        Ok(BookQuery::Featureless) => BenchBookQueryOutcome::Featureless,
        Ok(BookQuery::NotIndexed) => BenchBookQueryOutcome::NotIndexed,
        Ok(BookQuery::NotBook) => BenchBookQueryOutcome::NotBook,
        Ok(BookQuery::Failed(message)) => BenchBookQueryOutcome::Failed { message },
        Ok(BookQuery::Ready(relations)) => BenchBookQueryOutcome::Ready {
            relations: BenchBookRelations {
                origin_pages: relations
                    .origin
                    .pages
                    .iter()
                    .map(|page| BenchBookOriginPage {
                        item_key: page.item_key.clone(),
                        baseline: match page.baseline {
                            BookPageBaseline::Unmatched => BenchBookPageBaseline::Unmatched,
                            BookPageBaseline::Excluded => BenchBookPageBaseline::Excluded,
                        },
                    })
                    .collect(),
                hits: relations.hits.iter().map(certificate_hit).collect(),
            },
        },
        Err(error) => BenchBookQueryOutcome::EngineError {
            message: error.to_string(),
        },
    }
}

fn certificate_hit(hit: &crate::similar_index::BookRelationHit) -> BenchBookRelationHit {
    BenchBookRelationHit {
        other_container_key: hit.other_container_key.clone(),
        other_page_count: hit.other_page_count,
        pair: BenchBookPair {
            a: hit.pair.a,
            b: hit.pair.b,
            matched: hit.pair.matched,
            distinctive_a: hit.pair.distinctive_a,
            distinctive_b: hit.pair.distinctive_b,
            coverage_a_bits: hit.pair.coverage_a.to_bits(),
            coverage_b_bits: hit.pair.coverage_b.to_bits(),
            relation: match hit.pair.relation {
                Relation::Same => BenchBookRelation::Same,
                Relation::Contains { whole } => BenchBookRelation::Contains { whole },
                Relation::Unrelated => BenchBookRelation::Unrelated,
                Relation::Undecidable => BenchBookRelation::Undecidable,
            },
            alignment: hit.pair.alignment.clone(),
        },
        overrides: hit
            .overrides()
            .iter()
            .map(|entry| BenchBookPageMatch {
                origin_slot: entry.origin_slot,
                state: match entry.state {
                    BookPageMatchState::Strong => BenchBookPageMatchState::Strong,
                    BookPageMatchState::Weak => BenchBookPageMatchState::Weak,
                },
                other_page_index: entry.other_page_index,
                other_target: entry.other_target.as_ref().map(certificate_target),
                other_item_key: entry.other_item_key.clone(),
                other_mtime: entry.other_mtime,
                other_file_size: entry.other_file_size,
            })
            .collect(),
    }
}

fn certificate_target(target: &SimilarItemTarget) -> BenchSimilarItemTarget {
    match target {
        SimilarItemTarget::File(path) => BenchSimilarItemTarget::File {
            path: path.display().to_string(),
        },
        SimilarItemTarget::ZipPage {
            zip_path,
            entry_name,
        } => BenchSimilarItemTarget::ZipPage {
            zip_path: zip_path.display().to_string(),
            entry_name: entry_name.clone(),
        },
        SimilarItemTarget::PdfPage { pdf_path, page_num } => BenchSimilarItemTarget::PdfPage {
            pdf_path: pdf_path.display().to_string(),
            page_num: *page_num,
        },
    }
}

pub fn lower_product_book_query_worker_priority() {
    crate::similar_index::lower_current_thread_priority();
}

pub fn product_book_query_worker_priority() -> Result<i32, String> {
    #[cfg(windows)]
    unsafe {
        use windows::Win32::System::Threading::{GetCurrentThread, GetThreadPriority};
        let priority = GetThreadPriority(GetCurrentThread());
        if priority == i32::MAX {
            Err("GetThreadPriority failed".to_owned())
        } else {
            Ok(priority)
        }
    }
    #[cfg(not(windows))]
    {
        Err("thread priority observation is only supported on Windows".to_owned())
    }
}

pub fn expected_product_book_query_worker_priority() -> i32 {
    #[cfg(windows)]
    {
        windows::Win32::System::Threading::THREAD_PRIORITY_BELOW_NORMAL.0
    }
    #[cfg(not(windows))]
    {
        -1
    }
}

fn summarize_observation(observation: &EngineObservation) -> BenchQuerySummary {
    let mut digest = Sha256::new();
    hash_metadata(&mut digest, observation.metadata);
    let (outcome, origin_pages, hit_count, override_count) = match &observation.outcome {
        Ok(BookQuery::Preparing) => {
            put_bytes(&mut digest, b"preparing");
            ("preparing".to_owned(), 0, 0, 0)
        }
        Ok(BookQuery::Featureless) => {
            put_bytes(&mut digest, b"featureless");
            ("featureless".to_owned(), 0, 0, 0)
        }
        Ok(BookQuery::NotIndexed) => {
            put_bytes(&mut digest, b"not_indexed");
            ("not_indexed".to_owned(), 0, 0, 0)
        }
        Ok(BookQuery::NotBook) => {
            put_bytes(&mut digest, b"not_book");
            ("not_book".to_owned(), 0, 0, 0)
        }
        Ok(BookQuery::Failed(error)) => {
            put_bytes(&mut digest, b"failed");
            put_string(&mut digest, error);
            ("failed".to_owned(), 0, 0, 0)
        }
        Ok(BookQuery::Ready(relations)) => {
            put_bytes(&mut digest, b"ready");
            put_u64(&mut digest, relations.origin.pages.len() as u64);
            for page in &relations.origin.pages {
                put_string(&mut digest, &page.item_key);
                digest.update([match page.baseline {
                    BookPageBaseline::Unmatched => 0,
                    BookPageBaseline::Excluded => 1,
                }]);
            }
            let mut overrides = 0u64;
            put_u64(&mut digest, relations.hits.len() as u64);
            for hit in &relations.hits {
                put_string(&mut digest, &hit.other_container_key);
                put_u32(&mut digest, hit.other_page_count);
                put_u32(&mut digest, hit.pair.a);
                put_u32(&mut digest, hit.pair.b);
                put_u32(&mut digest, hit.pair.matched);
                put_u32(&mut digest, hit.pair.distinctive_a);
                put_u32(&mut digest, hit.pair.distinctive_b);
                put_u32(&mut digest, hit.pair.coverage_a.to_bits());
                put_u32(&mut digest, hit.pair.coverage_b.to_bits());
                match hit.pair.relation {
                    Relation::Same => digest.update([0]),
                    Relation::Contains { whole } => {
                        digest.update([1]);
                        put_u32(&mut digest, whole);
                    }
                    Relation::Unrelated => digest.update([2]),
                    Relation::Undecidable => digest.update([3]),
                }
                put_u64(&mut digest, hit.pair.alignment.len() as u64);
                for &(left, right) in &hit.pair.alignment {
                    put_u32(&mut digest, left);
                    put_u32(&mut digest, right);
                }
                put_u64(&mut digest, hit.overrides().len() as u64);
                overrides = overrides.saturating_add(hit.overrides().len() as u64);
                for entry in hit.overrides() {
                    put_u64(&mut digest, entry.origin_slot as u64);
                    digest.update([match entry.state {
                        BookPageMatchState::Strong => 0,
                        BookPageMatchState::Weak => 1,
                    }]);
                    put_u32(&mut digest, entry.other_page_index);
                    hash_target(&mut digest, entry.other_target.as_ref());
                    put_string(&mut digest, &entry.other_item_key);
                    put_i64(&mut digest, entry.other_mtime);
                    put_i64(&mut digest, entry.other_file_size);
                }
            }
            (
                "ready".to_owned(),
                relations.origin.pages.len() as u64,
                relations.hits.len() as u64,
                overrides,
            )
        }
        Err(error) => {
            put_bytes(&mut digest, b"engine_error");
            put_string(&mut digest, &error.to_string());
            (format!("engine_error:{error}"), 0, 0, 0)
        }
    };
    BenchQuerySummary {
        store_id_hex: hex_bytes(&observation.metadata.store_id),
        read_seq: observation.metadata.read_seq,
        page_order_version: observation.metadata.page_order_version,
        outcome,
        origin_pages,
        hit_count,
        override_count,
        result_sha256: hex_bytes(&digest.finalize()),
    }
}

fn hash_metadata(digest: &mut Sha256, metadata: BookReadMetadata) {
    digest.update(metadata.store_id);
    put_u64(digest, metadata.read_seq);
    put_i64(digest, metadata.page_order_version);
}

fn hash_target(digest: &mut Sha256, target: Option<&SimilarItemTarget>) {
    match target {
        None => digest.update([0]),
        Some(SimilarItemTarget::File(path)) => {
            digest.update([1]);
            put_string(digest, &path.display().to_string());
        }
        Some(SimilarItemTarget::ZipPage {
            zip_path,
            entry_name,
        }) => {
            digest.update([2]);
            put_string(digest, &zip_path.display().to_string());
            put_string(digest, entry_name);
        }
        Some(SimilarItemTarget::PdfPage { pdf_path, page_num }) => {
            digest.update([3]);
            put_string(digest, &pdf_path.display().to_string());
            put_u32(digest, *page_num);
        }
    }
}

fn put_string(digest: &mut Sha256, value: &str) {
    put_bytes(digest, value.as_bytes());
}

fn put_bytes(digest: &mut Sha256, value: &[u8]) {
    put_u64(digest, value.len() as u64);
    digest.update(value);
}

fn put_u64(digest: &mut Sha256, value: u64) {
    digest.update(value.to_le_bytes());
}

fn put_u32(digest: &mut Sha256, value: u32) {
    digest.update(value.to_le_bytes());
}

fn put_i64(digest: &mut Sha256, value: i64) {
    digest.update(value.to_le_bytes());
}

fn hex_bytes(bytes: &[u8]) -> String {
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        use std::fmt::Write as _;
        let _ = write!(encoded, "{byte:02x}");
    }
    encoded
}

fn add_resolved_role(resolved: &mut Vec<(PathBuf, Vec<String>)>, path: PathBuf, role: String) {
    if let Some((_, roles)) = resolved.iter_mut().find(|(known, _)| known == &path) {
        roles.push(role);
    } else {
        resolved.push((path, vec![role]));
    }
}

fn canonical_regular_file(path: &Path, role: &str) -> Result<PathBuf, String> {
    let canonical = std::fs::canonicalize(path)
        .map_err(|error| format!("{role} canonicalize failed for {}: {error}", path.display()))?;
    let metadata = std::fs::metadata(&canonical).map_err(|error| {
        format!(
            "{role} metadata failed for {}: {error}",
            canonical.display()
        )
    })?;
    if !metadata.is_file() {
        return Err(format!(
            "{role} is not a regular file: {}",
            canonical.display()
        ));
    }
    Ok(canonical)
}

fn resolve_new_output_path(path: &Path) -> Result<PathBuf, String> {
    if path.file_name().is_none() {
        return Err("measurement output must name a new file".to_owned());
    }
    if path.exists() {
        return Err(format!(
            "measurement output already exists (outputs are create-new): {}",
            path.display()
        ));
    }
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let canonical_parent = std::fs::canonicalize(parent).map_err(|error| {
        format!(
            "measurement output parent canonicalize failed for {}: {error}",
            parent.display()
        )
    })?;
    Ok(canonical_parent.join(path.file_name().expect("file name was checked")))
}

fn ensure_output_is_distinct(output: &Path, forbidden: &[PathBuf]) -> Result<(), String> {
    let output_key = comparison_path(output);
    if forbidden
        .iter()
        .any(|input| comparison_path(input) == output_key)
    {
        return Err(format!(
            "measurement output aliases an input or SQLite sidecar: {}",
            output.display()
        ));
    }
    Ok(())
}

fn comparison_path(path: &Path) -> String {
    let value = path.as_os_str().to_string_lossy().replace('/', "\\");
    if cfg!(windows) {
        value.to_lowercase()
    } else {
        value
    }
}

fn append_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut value = OsString::from(path.as_os_str());
    value.push(suffix);
    PathBuf::from(value)
}

fn ensure_sidecars_absent(sidecars: &[PathBuf; 2]) -> Result<(), String> {
    for sidecar in sidecars {
        match std::fs::symlink_metadata(sidecar) {
            Ok(_) => {
                return Err(format!(
                    "measurement database has a live SQLite sidecar: {}",
                    sidecar.display()
                ));
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(format!(
                    "SQLite sidecar check failed for {}: {error}",
                    sidecar.display()
                ));
            }
        }
    }
    Ok(())
}

/// SQLite read-only connections still create `-wal`/`-shm` files when the database header is in
/// WAL mode. Measurements therefore require a separately prepared, fully checkpointed rollback-
/// journal copy. Reading the header through the already acquired guard rejects WAL input before
/// the product engine can open SQLite or create a sidecar.
fn ensure_rollback_journal_header(guarded: &GuardedFile) -> Result<(), String> {
    const SQLITE_HEADER_LEN: usize = 20;
    const SQLITE_SIGNATURE: &[u8; 16] = b"SQLite format 3\0";
    const ROLLBACK_JOURNAL: [u8; 2] = [1, 1];

    let mut file = guarded.file.try_clone().map_err(|error| {
        format!(
            "guarded database header clone failed for {}: {error}",
            guarded.path.display()
        )
    })?;
    file.seek(SeekFrom::Start(0)).map_err(|error| {
        format!(
            "guarded database header seek failed for {}: {error}",
            guarded.path.display()
        )
    })?;
    let mut header = [0u8; SQLITE_HEADER_LEN];
    file.read_exact(&mut header).map_err(|error| {
        format!(
            "guarded database header read failed for {}: {error}",
            guarded.path.display()
        )
    })?;
    if &header[..SQLITE_SIGNATURE.len()] != SQLITE_SIGNATURE {
        return Err(format!(
            "measurement database is not a SQLite 3 file: {}",
            guarded.path.display()
        ));
    }
    let journal_versions = [header[18], header[19]];
    if journal_versions != ROLLBACK_JOURNAL {
        return Err(format!(
            "measurement database must be a checkpointed rollback-journal copy (header write/read versions 1/1); found {}/{} at {}",
            journal_versions[0],
            journal_versions[1],
            guarded.path.display()
        ));
    }
    Ok(())
}

fn fingerprint_guarded_file(guarded: &GuardedFile) -> Result<BenchInputFingerprint, String> {
    let mut file = guarded.file.try_clone().map_err(|error| {
        format!(
            "guarded input clone failed for {}: {error}",
            guarded.path.display()
        )
    })?;
    file.seek(SeekFrom::Start(0)).map_err(|error| {
        format!(
            "guarded input seek failed for {}: {error}",
            guarded.path.display()
        )
    })?;
    let bytes = file
        .metadata()
        .map_err(|error| {
            format!(
                "guarded input metadata failed for {}: {error}",
                guarded.path.display()
            )
        })?
        .len();
    let mut reader = BufReader::with_capacity(1024 * 1024, file);
    let mut buffer = vec![0u8; 1024 * 1024];
    let mut digest = Sha256::new();
    loop {
        let read = reader.read(&mut buffer).map_err(|error| {
            format!(
                "guarded input hash read failed for {}: {error}",
                guarded.path.display()
            )
        })?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
    }
    Ok(BenchInputFingerprint {
        roles: guarded.roles.clone(),
        path: guarded.path.display().to_string(),
        bytes,
        sha256: hex_bytes(&digest.finalize()),
    })
}

#[cfg(windows)]
fn open_read_sharing_guard(path: &Path) -> Result<File, String> {
    use std::os::windows::fs::OpenOptionsExt;

    const FILE_SHARE_READ: u32 = 0x0000_0001;
    OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ)
        .open(path)
        .map_err(|error| {
            format!(
                "read-sharing guard acquisition failed for {}: {error}",
                path.display()
            )
        })
}

#[cfg(not(windows))]
fn open_read_sharing_guard(_path: &Path) -> Result<File, String> {
    Err("Windows read-sharing guards are unavailable".to_owned())
}

#[cfg(windows)]
fn file_identity(file: &File) -> Result<FileIdentity, String> {
    use std::os::windows::io::AsRawHandle;

    #[repr(C)]
    struct FileTime {
        low: u32,
        high: u32,
    }

    #[repr(C)]
    struct ByHandleFileInformation {
        attributes: u32,
        creation: FileTime,
        access: FileTime,
        write: FileTime,
        volume_serial: u32,
        size_high: u32,
        size_low: u32,
        link_count: u32,
        index_high: u32,
        index_low: u32,
    }

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetFileInformationByHandle(
            file: *mut c_void,
            information: *mut ByHandleFileInformation,
        ) -> i32;
    }

    let mut information = ByHandleFileInformation {
        attributes: 0,
        creation: FileTime { low: 0, high: 0 },
        access: FileTime { low: 0, high: 0 },
        write: FileTime { low: 0, high: 0 },
        volume_serial: 0,
        size_high: 0,
        size_low: 0,
        link_count: 0,
        index_high: 0,
        index_low: 0,
    };
    let ok = unsafe { GetFileInformationByHandle(file.as_raw_handle().cast(), &mut information) };
    if ok == 0 {
        return Err(format!(
            "GetFileInformationByHandle failed: {}",
            std::io::Error::last_os_error()
        ));
    }
    Ok(FileIdentity {
        volume: information.volume_serial,
        index: (u64::from(information.index_high) << 32) | u64::from(information.index_low),
    })
}

#[cfg(not(windows))]
fn file_identity(_file: &File) -> Result<FileIdentity, String> {
    Err("Windows file identity is unavailable".to_owned())
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use crate::similar_db::SimilarDb;
    use crate::similar_search_array::{base_path, load_or_rebuild};
    use std::io::Write;
    use std::os::windows::fs::OpenOptionsExt;

    fn dummy_spec(db: &Path, base: &Path) -> BenchInputSpec {
        BenchInputSpec {
            db_path: db.to_path_buf(),
            active_base_paths: vec![base.to_path_buf()],
            retained_base_paths: vec![base.to_path_buf()],
            immutable_roots: vec!["c:/library".to_owned()],
        }
    }

    #[test]
    fn guard_rejects_an_already_open_writer_and_blocks_write_delete_until_drop() {
        const SHARE_ALL: u32 = 0x0000_0001 | 0x0000_0002 | 0x0000_0004;
        let temp = tempfile::tempdir().unwrap();
        let db = temp.path().join("similar.db");
        let base = temp.path().join("similar.base");
        let connection = rusqlite::Connection::open(&db).unwrap();
        let mode: String = connection
            .query_row("PRAGMA journal_mode=DELETE", [], |row| row.get(0))
            .unwrap();
        assert_eq!(mode.to_ascii_lowercase(), "delete");
        connection
            .execute_batch("CREATE TABLE guard_fixture(value INTEGER)")
            .unwrap();
        drop(connection);
        std::fs::write(&base, b"base").unwrap();

        let existing_writer = OpenOptions::new()
            .read(true)
            .write(true)
            .share_mode(SHARE_ALL)
            .open(&db)
            .unwrap();
        let rejected = GuardedBenchInputs::acquire(
            dummy_spec(&db, &base),
            &temp.path().join("writer-open.jsonl"),
        );
        assert!(rejected.is_err());
        drop(existing_writer);

        let guarded =
            GuardedBenchInputs::acquire(dummy_spec(&db, &base), &temp.path().join("guarded.jsonl"))
                .unwrap();
        assert!(OpenOptions::new().write(true).open(&db).is_err());
        assert!(std::fs::rename(&base, temp.path().join("moved.base")).is_err());
        guarded.verify_unchanged().unwrap();
        drop(guarded);

        OpenOptions::new()
            .write(true)
            .open(&db)
            .unwrap()
            .write_all(b"ok")
            .unwrap();
        std::fs::rename(&base, temp.path().join("moved.base")).unwrap();
    }

    #[test]
    fn guarded_product_reader_is_read_only_and_active_retained_paths_share_one_base_arc() {
        let temp = tempfile::tempdir().unwrap();
        let db_path = SimilarDb::db_path_at(temp.path());
        let db = SimilarDb::open_at(&db_path).unwrap();
        let base_path = base_path(temp.path());
        let loaded = load_or_rebuild(&db, &base_path).unwrap();
        assert_eq!(loaded.snapshot.applied_seq, 0);
        drop(loaded);
        drop(db);
        let checkpoint = rusqlite::Connection::open(&db_path).unwrap();
        let mode: String = checkpoint
            .query_row("PRAGMA journal_mode", [], |row| row.get(0))
            .unwrap();
        assert_eq!(mode.to_ascii_lowercase(), "wal");
        checkpoint
            .execute_batch("PRAGMA wal_checkpoint(TRUNCATE)")
            .unwrap();
        drop(checkpoint);
        // This disposable database first demonstrates the real SQLite behavior behind the input
        // contract: even without live sidecars, a WAL-mode header is rejected before the product
        // reader can recreate them.
        for sidecar in [
            append_suffix(&db_path, "-wal"),
            append_suffix(&db_path, "-shm"),
        ] {
            match std::fs::remove_file(sidecar) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => panic!("prepared sidecar cleanup failed: {error}"),
            }
        }
        let header = std::fs::read(&db_path).unwrap();
        assert_eq!((&header[18..20]), &[2, 2], "fixture must remain WAL-mode");
        assert!(!append_suffix(&db_path, "-wal").exists());
        assert!(!append_suffix(&db_path, "-shm").exists());
        let wal_result = GuardedBenchInputs::acquire(
            dummy_spec(&db_path, &base_path),
            &temp.path().join("wal-result.jsonl"),
        );
        let wal_error = match wal_result {
            Ok(_) => panic!("WAL-mode input must be rejected before the engine opens"),
            Err(error) => error,
        };
        assert!(wal_error.contains("header write/read versions 1/1"));
        assert!(!temp.path().join("wal-result.jsonl").exists());
        assert!(!append_suffix(&db_path, "-wal").exists());
        assert!(!append_suffix(&db_path, "-shm").exists());

        // Real measurements use the same operation on a separately prepared copy, never on the
        // source backup. The rollback-mode copy remains sidecar-free through the product reader.
        let preparation = rusqlite::Connection::open(&db_path).unwrap();
        let prepared_mode: String = preparation
            .query_row("PRAGMA journal_mode=DELETE", [], |row| row.get(0))
            .unwrap();
        assert_eq!(prepared_mode.to_ascii_lowercase(), "delete");
        drop(preparation);
        let header = std::fs::read(&db_path).unwrap();
        assert_eq!(
            (&header[18..20]),
            &[1, 1],
            "prepared copy must use rollback journal mode"
        );
        assert!(!append_suffix(&db_path, "-wal").exists());
        assert!(!append_suffix(&db_path, "-shm").exists());

        let guarded = GuardedBenchInputs::acquire(
            dummy_spec(&db_path, &base_path),
            &temp.path().join("result.jsonl"),
        )
        .unwrap();
        let mut engine = BenchEngine::open(guarded.engine_config()).unwrap();
        assert!(Arc::ptr_eq(
            &engine.active_bases[0].snapshot,
            &engine._retained_bases[0].snapshot
        ));
        let run = engine.query("c:/library/not-a-book").unwrap();
        assert_eq!(run.into_summary().outcome, "not_book");
        drop(engine);
        guarded.verify_unchanged().unwrap();
        assert!(!append_suffix(&db_path, "-wal").exists());
        assert!(!append_suffix(&db_path, "-shm").exists());
    }

    #[test]
    fn output_cannot_alias_an_input_or_sqlite_sidecar() {
        let temp = tempfile::tempdir().unwrap();
        let db = temp.path().join("similar.db");
        let base = temp.path().join("similar.base");
        std::fs::write(&db, b"db").unwrap();
        std::fs::write(&base, b"base").unwrap();
        assert!(GuardedBenchInputs::acquire(dummy_spec(&db, &base), &db).is_err());
        assert!(
            GuardedBenchInputs::acquire(dummy_spec(&db, &base), &append_suffix(&db, "-wal"))
                .is_err()
        );
    }
}
