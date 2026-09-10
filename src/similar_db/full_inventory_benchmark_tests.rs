//! Manual, ignored A/B harness for the Phase 2 full-reconcile inventory.
//!
//! Every invocation performs exactly one `setup`, `legacy`, `inventory`, or `verify` mode. The
//! caller must provide a fresh, real directory below this checkout's `target` and point both
//! `TEMP` and `TMP` at its empty `temp` child before the test process starts. Output files use
//! create-new semantics. This keeps the synthetic store, SQLite spill files, copies, and result
//! JSON away from APPDATA and makes setup/probe/verification memory peaks belong to separate
//! processes. Snapshot-after-start mutations are deliberately excluded from timing because the
//! legacy prune is unsafe for them; the product regression tests are their correctness oracle.

use super::*;

use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use std::error::Error;
use std::fs::{File, OpenOptions};
use std::io::{BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering as AtomicOrdering};
use std::time::{Duration, Instant};

const REFERENCE_ROWS: u64 = 4_627_166;
const DEFAULT_KEY_BYTES: usize = 80;
const ALLOWED_KEY_BYTES: [usize; 3] = [48, 80, 120];
const CONTAINER_PAGES: u64 = 256;
const WORKERS: usize = 16;
const ENV_MODE: &str = "MIV_FULL_INVENTORY_BENCH_MODE";
const ENV_RUN_DIR: &str = "MIV_FULL_INVENTORY_BENCH_DIR";
const ENV_ROWS: &str = "MIV_FULL_INVENTORY_BENCH_ROWS";
const ENV_KEY_BYTES: &str = "MIV_FULL_INVENTORY_BENCH_KEY_BYTES";
const ENV_TRIAL: &str = "MIV_FULL_INVENTORY_BENCH_TRIAL";
const FIXED_COMPLETED_AT: i64 = 2_000_000_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BenchMode {
    Setup,
    Legacy,
    Inventory,
    Verify,
}

impl BenchMode {
    fn parse(value: &str) -> Result<Self, String> {
        match value {
            "setup" => Ok(Self::Setup),
            "legacy" => Ok(Self::Legacy),
            "inventory" => Ok(Self::Inventory),
            "verify" => Ok(Self::Verify),
            _ => Err(format!(
                "{ENV_MODE} must be setup, legacy, inventory, or verify"
            )),
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Setup => "setup",
            Self::Legacy => "legacy",
            Self::Inventory => "inventory",
            Self::Verify => "verify",
        }
    }
}

#[derive(Clone, Debug)]
struct BenchConfig {
    mode: BenchMode,
    run_dir: PathBuf,
    temp_dir: PathBuf,
    rows: u64,
    key_bytes: usize,
    trial: Option<String>,
}

impl BenchConfig {
    fn from_env() -> Result<Self, String> {
        let mode = BenchMode::parse(
            &std::env::var(ENV_MODE).map_err(|_| format!("{ENV_MODE} is required"))?,
        )?;
        let rows = std::env::var(ENV_ROWS)
            .ok()
            .map(|value| {
                value
                    .parse::<u64>()
                    .map_err(|error| format!("invalid {ENV_ROWS}: {error}"))
            })
            .transpose()?
            .unwrap_or(REFERENCE_ROWS);
        if !(1..=REFERENCE_ROWS).contains(&rows) {
            return Err(format!("{ENV_ROWS} must be between 1 and {REFERENCE_ROWS}"));
        }
        let key_bytes = std::env::var(ENV_KEY_BYTES)
            .ok()
            .map(|value| {
                value
                    .parse::<usize>()
                    .map_err(|error| format!("invalid {ENV_KEY_BYTES}: {error}"))
            })
            .transpose()?
            .unwrap_or(DEFAULT_KEY_BYTES);
        if !ALLOWED_KEY_BYTES.contains(&key_bytes) {
            return Err(format!("{ENV_KEY_BYTES} must be 48, 80, or 120"));
        }
        let requested_run_dir = PathBuf::from(
            std::env::var_os(ENV_RUN_DIR).ok_or_else(|| format!("{ENV_RUN_DIR} is required"))?,
        );
        if !requested_run_dir.is_absolute() {
            return Err(format!("{ENV_RUN_DIR} must be an absolute path"));
        }
        let manifest_target = Path::new(env!("CARGO_MANIFEST_DIR")).join("target");
        reject_absolute_reparse_chain(&manifest_target)?;
        reject_absolute_reparse_chain(&requested_run_dir)?;
        let target = manifest_target
            .canonicalize()
            .map_err(|error| format!("canonicalize {}: {error}", manifest_target.display()))?;
        let run_dir = requested_run_dir.canonicalize().map_err(|error| {
            format!(
                "the benchmark run directory must already exist: {}: {error}",
                requested_run_dir.display()
            )
        })?;
        if !run_dir.starts_with(&target) || run_dir == target {
            return Err(format!(
                "benchmark run directory must be a dedicated child of {}",
                target.display()
            ));
        }
        reject_reparse_chain(&target, &run_dir)?;

        let requested_temp_dir = requested_run_dir.join("temp");
        reject_absolute_reparse_chain(&requested_temp_dir)?;
        let temp_dir = requested_temp_dir.canonicalize().map_err(|error| {
            format!(
                "the benchmark temp directory must already exist: {}/temp: {error}",
                run_dir.display()
            )
        })?;
        if temp_dir.parent() != Some(run_dir.as_path()) {
            return Err("benchmark temp directory escaped its run directory".to_owned());
        }
        reject_reparse_path(&temp_dir)?;
        require_empty_directory(&temp_dir)?;
        for name in ["TEMP", "TMP"] {
            let configured_path =
                PathBuf::from(std::env::var_os(name).ok_or_else(|| format!("{name} must be set"))?);
            reject_absolute_reparse_chain(&configured_path)?;
            let configured = configured_path
                .canonicalize()
                .map_err(|error| format!("canonicalize {name}: {error}"))?;
            if configured != temp_dir {
                return Err(format!(
                    "{name} must point to the dedicated benchmark temp directory {}",
                    temp_dir.display()
                ));
            }
        }

        let trial = std::env::var(ENV_TRIAL).ok();
        match mode {
            BenchMode::Legacy => require_trial(&trial, &["ab-legacy", "ba-legacy"])?,
            BenchMode::Inventory => require_trial(&trial, &["ab-inventory", "ba-inventory"])?,
            BenchMode::Setup | BenchMode::Verify if trial.is_some() => {
                return Err(format!("{ENV_TRIAL} is not used by {}", mode.label()));
            }
            BenchMode::Setup | BenchMode::Verify => {}
        }
        Ok(Self {
            mode,
            run_dir,
            temp_dir,
            rows,
            key_bytes,
            trial,
        })
    }

    fn base_db(&self) -> PathBuf {
        self.run_dir.join("base.db")
    }

    fn trial_db(&self) -> PathBuf {
        self.run_dir.join(format!(
            "{}.db",
            self.trial.as_deref().expect("trial mode must have a name")
        ))
    }

    fn trial_result(&self) -> PathBuf {
        self.run_dir.join(format!(
            "{}.json",
            self.trial.as_deref().expect("trial mode must have a name")
        ))
    }
}

fn require_trial(trial: &Option<String>, allowed: &[&str]) -> Result<(), String> {
    let Some(trial) = trial.as_deref() else {
        return Err(format!("{ENV_TRIAL} is required for this mode"));
    };
    if allowed.contains(&trial) {
        Ok(())
    } else {
        Err(format!(
            "{ENV_TRIAL}={trial:?} does not match this mode's AB/BA trial names"
        ))
    }
}

#[cfg(windows)]
fn reject_reparse_path(path: &Path) -> Result<(), String> {
    use std::os::windows::fs::MetadataExt as _;
    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0400;
    let metadata = std::fs::symlink_metadata(path)
        .map_err(|error| format!("metadata {}: {error}", path.display()))?;
    if metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        Err(format!(
            "benchmark paths must not contain reparse points: {}",
            path.display()
        ))
    } else {
        Ok(())
    }
}

#[cfg(not(windows))]
fn reject_reparse_path(path: &Path) -> Result<(), String> {
    let metadata = std::fs::symlink_metadata(path)
        .map_err(|error| format!("metadata {}: {error}", path.display()))?;
    if metadata.file_type().is_symlink() {
        Err(format!(
            "benchmark paths must not contain symbolic links: {}",
            path.display()
        ))
    } else {
        Ok(())
    }
}

fn reject_reparse_chain(target: &Path, run_dir: &Path) -> Result<(), String> {
    reject_reparse_path(target)?;
    let relative = run_dir
        .strip_prefix(target)
        .map_err(|_| "benchmark run directory escaped target".to_owned())?;
    let mut current = target.to_path_buf();
    for component in relative.components() {
        current.push(component);
        reject_reparse_path(&current)?;
    }
    Ok(())
}

fn reject_absolute_reparse_chain(path: &Path) -> Result<(), String> {
    if !path.is_absolute() {
        return Err(format!(
            "reparse validation requires an absolute path: {}",
            path.display()
        ));
    }
    let mut current = PathBuf::new();
    for component in path.components() {
        current.push(component);
        if !current.has_root() {
            continue;
        }
        reject_reparse_path(&current)?;
    }
    Ok(())
}

fn require_empty_directory(path: &Path) -> Result<(), String> {
    let mut entries =
        std::fs::read_dir(path).map_err(|error| format!("read_dir {}: {error}", path.display()))?;
    if entries
        .next()
        .transpose()
        .map_err(|error| error.to_string())?
        .is_some()
    {
        Err(format!(
            "benchmark temp directory must be empty at process start: {}",
            path.display()
        ))
    } else {
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
struct ClassificationCounts {
    current: u64,
    old_hash: u64,
    stale_metadata: u64,
    missing: u64,
    new_probe: u64,
}

impl ClassificationCounts {
    fn add(&mut self, class: UnitClass, count: u64) {
        let target = match class {
            UnitClass::Current => &mut self.current,
            UnitClass::OldHash => &mut self.old_hash,
            UnitClass::StaleMetadata => &mut self.stale_metadata,
            UnitClass::Missing => &mut self.missing,
            UnitClass::NewProbe => &mut self.new_probe,
        };
        *target = target.saturating_add(count);
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
struct DatasetCounts {
    rows: u64,
    key_bytes: usize,
    loose_rows: u64,
    contained_rows: u64,
    containers: u64,
    loose_units: ClassificationCounts,
    container_units: ClassificationCounts,
    classified_rows: ClassificationCounts,
}

impl DatasetCounts {
    fn new(rows: u64, key_bytes: usize) -> Self {
        let loose_rows = rows.saturating_add(3) / 4;
        let contained_rows = rows - loose_rows;
        let containers = contained_rows.saturating_add(CONTAINER_PAGES - 1) / CONTAINER_PAGES;
        let mut loose_units = ClassificationCounts::default();
        let mut container_units = ClassificationCounts::default();
        let mut classified_rows = ClassificationCounts::default();
        for ordinal in 0..loose_rows {
            let class = unit_class(ordinal);
            loose_units.add(class, 1);
            classified_rows.add(class, 1);
        }
        for container_id in 0..containers {
            let class = unit_class(container_id);
            let pages = container_page_count(contained_rows, container_id);
            container_units.add(class, 1);
            classified_rows.add(class, pages);
        }
        Self {
            rows,
            key_bytes,
            loose_rows,
            contained_rows,
            containers,
            loose_units,
            container_units,
            classified_rows,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum UnitClass {
    Current,
    OldHash,
    StaleMetadata,
    Missing,
    NewProbe,
}

fn unit_class(unit_id: u64) -> UnitClass {
    if unit_id % 100 != 0 {
        return UnitClass::Current;
    }
    match (unit_id / 100) % 4 {
        0 => UnitClass::OldHash,
        1 => UnitClass::StaleMetadata,
        2 => UnitClass::Missing,
        _ => UnitClass::NewProbe,
    }
}

fn fixed_key(prefix: &str, id: u64, key_bytes: usize) -> String {
    let mut key = format!("{prefix}/{id:010}/");
    assert!(key.len() <= key_bytes);
    key.extend(std::iter::repeat_n('x', key_bytes - key.len()));
    key
}

fn item_key(index: u64, key_bytes: usize) -> String {
    fixed_key("item", index, key_bytes)
}

fn container_key(index: u64, key_bytes: usize) -> String {
    fixed_key("book", index, key_bytes)
}

fn loose_item_index(ordinal: u64) -> u64 {
    ordinal * 4
}

fn contained_item_index(rank: u64) -> u64 {
    (rank / 3) * 4 + (rank % 3) + 1
}

fn container_page_count(contained_rows: u64, container_id: u64) -> u64 {
    contained_rows
        .saturating_sub(container_id.saturating_mul(CONTAINER_PAGES))
        .min(CONTAINER_PAGES)
}

fn item_metadata(index: u64) -> (i64, i64) {
    (
        10_000_000i64.saturating_add(i64::try_from(index).unwrap_or(i64::MAX)),
        20_000i64.saturating_add(i64::try_from(index % 1_000).unwrap()),
    )
}

fn container_metadata(container_id: u64, page_count: u64) -> (i64, i64) {
    (
        30_000_000i64.saturating_add(i64::try_from(container_id).unwrap_or(i64::MAX)),
        40_000i64.saturating_add(i64::try_from(page_count).unwrap()),
    )
}

fn container_kind(container_id: u64) -> ContainerKind {
    match container_id % 3 {
        0 => ContainerKind::ImageFolder,
        1 => ContainerKind::Zip,
        _ => ContainerKind::Pdf,
    }
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
struct LogicalCalls {
    item_load: u64,
    container_freshness: u64,
    item_key_list: u64,
    inventory_load: u64,
    inventory_item_observe: u64,
    inventory_container_observe: u64,
    finalizer: u64,
    finalizer_all_item_key_scans: u64,
    finalizer_all_container_key_scans: u64,
    finalizer_item_row_lookups: u64,
    finalizer_container_identity_lookups: u64,
    finalizer_item_journal_inserts: u64,
    finalizer_item_deletes: u64,
    finalizer_container_member_lists: u64,
    finalizer_container_deletes: u64,
    summary_count_queries: u64,
    summary_writes: u64,
}

impl LogicalCalls {
    fn merge(&mut self, other: Self) {
        self.item_load += other.item_load;
        self.container_freshness += other.container_freshness;
        self.item_key_list += other.item_key_list;
        self.inventory_load += other.inventory_load;
        self.inventory_item_observe += other.inventory_item_observe;
        self.inventory_container_observe += other.inventory_container_observe;
        self.finalizer += other.finalizer;
        self.finalizer_all_item_key_scans += other.finalizer_all_item_key_scans;
        self.finalizer_all_container_key_scans += other.finalizer_all_container_key_scans;
        self.finalizer_item_row_lookups += other.finalizer_item_row_lookups;
        self.finalizer_container_identity_lookups += other.finalizer_container_identity_lookups;
        self.finalizer_item_journal_inserts += other.finalizer_item_journal_inserts;
        self.finalizer_item_deletes += other.finalizer_item_deletes;
        self.finalizer_container_member_lists += other.finalizer_container_member_lists;
        self.finalizer_container_deletes += other.finalizer_container_deletes;
        self.summary_count_queries += other.summary_count_queries;
        self.summary_writes += other.summary_writes;
    }
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
struct ProbeSummary {
    units: u64,
    observations: u64,
    checksum_sum: u64,
    checksum_xor: u64,
}

impl ProbeSummary {
    fn record(&mut self, identity: u64, code: u64) {
        let mixed = splitmix64(identity ^ code.wrapping_mul(0x9e37_79b9_7f4a_7c15));
        self.observations += 1;
        self.checksum_sum = self.checksum_sum.wrapping_add(mixed);
        self.checksum_xor ^= mixed.rotate_left((identity % 63) as u32);
    }

    fn merge(&mut self, other: Self) {
        self.units += other.units;
        self.observations += other.observations;
        self.checksum_sum = self.checksum_sum.wrapping_add(other.checksum_sum);
        self.checksum_xor ^= other.checksum_xor;
    }
}

fn splitmix64(mut value: u64) -> u64 {
    value = value.wrapping_add(0x9e37_79b9_7f4a_7c15);
    value = (value ^ (value >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    value = (value ^ (value >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    value ^ (value >> 31)
}

#[derive(Default)]
struct LegacyWorkerResult {
    seen_items: HashSet<String>,
    seen_containers: HashSet<String>,
    probe: ProbeSummary,
    calls: LogicalCalls,
}

#[derive(Default)]
struct InventoryWorkerResult {
    probe: ProbeSummary,
    calls: LogicalCalls,
}

fn run_legacy_probes(
    db: &SimilarDb,
    dataset: &DatasetCounts,
) -> Result<(HashSet<String>, HashSet<String>, ProbeSummary, LogicalCalls), String> {
    let next_task = AtomicU64::new(0);
    let task_count = dataset.loose_rows + dataset.containers;
    let workers = std::thread::scope(|scope| {
        let mut workers = Vec::with_capacity(WORKERS);
        for _ in 0..WORKERS {
            let next_task = &next_task;
            workers.push(scope.spawn(move || {
                let mut result = LegacyWorkerResult::default();
                loop {
                    let task = next_task.fetch_add(1, AtomicOrdering::Relaxed);
                    if task >= task_count {
                        break;
                    }
                    if task < dataset.loose_rows {
                        probe_legacy_loose(db, task, dataset.key_bytes, &mut result)?;
                    } else {
                        probe_legacy_container(
                            db,
                            dataset.contained_rows,
                            task - dataset.loose_rows,
                            dataset.key_bytes,
                            &mut result,
                        )?;
                    }
                }
                Ok::<_, String>(result)
            }));
        }
        workers
            .into_iter()
            .map(|worker| {
                worker
                    .join()
                    .map_err(|_| "legacy benchmark worker panicked".to_owned())?
            })
            .collect::<Result<Vec<_>, _>>()
    })?;
    let mut seen_items = HashSet::new();
    let mut seen_containers = HashSet::new();
    let mut probe = ProbeSummary::default();
    let mut calls = LogicalCalls::default();
    for mut worker in workers {
        seen_items.extend(worker.seen_items.drain());
        seen_containers.extend(worker.seen_containers.drain());
        probe.merge(worker.probe);
        calls.merge(worker.calls);
    }
    Ok((seen_items, seen_containers, probe, calls))
}

fn run_inventory_probes(
    inventory: &FullReconcileInventory,
    dataset: &DatasetCounts,
) -> Result<(ProbeSummary, LogicalCalls), String> {
    let next_task = AtomicU64::new(0);
    let task_count = dataset.loose_rows + dataset.containers;
    let workers = std::thread::scope(|scope| {
        let mut workers = Vec::with_capacity(WORKERS);
        for _ in 0..WORKERS {
            let next_task = &next_task;
            workers.push(scope.spawn(move || {
                let mut result = InventoryWorkerResult::default();
                loop {
                    let task = next_task.fetch_add(1, AtomicOrdering::Relaxed);
                    if task >= task_count {
                        break;
                    }
                    if task < dataset.loose_rows {
                        probe_inventory_loose(inventory, task, dataset.key_bytes, &mut result);
                    } else {
                        probe_inventory_container(
                            inventory,
                            dataset.contained_rows,
                            task - dataset.loose_rows,
                            dataset.key_bytes,
                            &mut result,
                        );
                    }
                }
                result
            }));
        }
        workers
            .into_iter()
            .map(|worker| {
                worker
                    .join()
                    .map_err(|_| "inventory benchmark worker panicked".to_owned())
            })
            .collect::<Result<Vec<_>, _>>()
    })?;
    let mut probe = ProbeSummary::default();
    let mut calls = LogicalCalls::default();
    for worker in workers {
        probe.merge(worker.probe);
        calls.merge(worker.calls);
    }
    Ok((probe, calls))
}

fn probe_legacy_loose(
    db: &SimilarDb,
    ordinal: u64,
    key_bytes: usize,
    result: &mut LegacyWorkerResult,
) -> Result<(), String> {
    result.probe.units += 1;
    let class = unit_class(ordinal);
    result.probe.record(ordinal, 0x10 + class_code(class));
    if class == UnitClass::Missing {
        return Ok(());
    }
    let index = loose_item_index(ordinal);
    let key = item_key(index, key_bytes);
    let (mtime, file_size) = item_metadata(index);
    let observed_mtime = if class == UnitClass::StaleMetadata {
        mtime + 1
    } else {
        mtime
    };
    let row = db
        .load_item(&key, current_hash_version())
        .map_err(|error| error.to_string())?;
    result.calls.item_load += 1;
    let reusable = row
        .as_ref()
        .is_some_and(|row| row.item.mtime == observed_mtime && row.item.file_size == file_size);
    let exact = reusable
        && row
            .as_ref()
            .is_some_and(|row| row.item.container_key.is_none() && row.item.page_index.is_none());
    result
        .probe
        .record(ordinal, observation_code(exact, reusable));
    result.seen_items.insert(key);
    if class == UnitClass::NewProbe {
        let missing = fixed_key("new-item", index, key_bytes);
        let row = db
            .load_item(&missing, current_hash_version())
            .map_err(|error| error.to_string())?;
        result.calls.item_load += 1;
        result.probe.record(ordinal, 0x21 + row.is_none() as u64);
        result.seen_items.insert(missing);
    }
    Ok(())
}

fn probe_inventory_loose(
    inventory: &FullReconcileInventory,
    ordinal: u64,
    key_bytes: usize,
    result: &mut InventoryWorkerResult,
) {
    result.probe.units += 1;
    let class = unit_class(ordinal);
    result.probe.record(ordinal, 0x10 + class_code(class));
    if class == UnitClass::Missing {
        return;
    }
    let index = loose_item_index(ordinal);
    let key = item_key(index, key_bytes);
    let (mtime, file_size) = item_metadata(index);
    let observed_mtime = if class == UnitClass::StaleMetadata {
        mtime + 1
    } else {
        mtime
    };
    let observed = inventory.observe_item(&key, None, None, observed_mtime, file_size);
    result.calls.inventory_item_observe += 1;
    result.probe.record(
        ordinal,
        observation_code(observed.exact_current, observed.reusable_current_row),
    );
    if class == UnitClass::NewProbe {
        let missing = fixed_key("new-item", index, key_bytes);
        let observed = inventory.observe_item(&missing, None, None, mtime, file_size);
        result.calls.inventory_item_observe += 1;
        result
            .probe
            .record(ordinal, 0x21 + (!observed.exact_current) as u64);
    }
}

fn probe_legacy_container(
    db: &SimilarDb,
    contained_rows: u64,
    container_id: u64,
    key_bytes: usize,
    result: &mut LegacyWorkerResult,
) -> Result<(), String> {
    result.probe.units += 1;
    let identity = (1u64 << 63) | container_id;
    let class = unit_class(container_id);
    result.probe.record(identity, 0x30 + class_code(class));
    if class == UnitClass::Missing {
        return Ok(());
    }
    let key = container_key(container_id, key_bytes);
    let pages = container_page_count(contained_rows, container_id);
    let (mtime, file_size) = container_metadata(container_id, pages);
    let observed_mtime = if class == UnitClass::StaleMetadata {
        mtime + 1
    } else {
        mtime
    };
    result.seen_containers.insert(key.clone());
    let freshness = db
        .container_freshness(
            &key,
            observed_mtime,
            file_size,
            u32::try_from(pages).unwrap(),
            current_hash_version(),
        )
        .map_err(|error| error.to_string())?;
    result.calls.container_freshness += 1;
    let mut exact_pages = true;
    if freshness == Freshness::Current && container_kind(container_id) == ContainerKind::ImageFolder
    {
        for page in 0..pages {
            let rank = container_id * CONTAINER_PAGES + page;
            let index = contained_item_index(rank);
            let item_key = item_key(index, key_bytes);
            let (item_mtime, item_size) = item_metadata(index);
            let row = db
                .load_item(&item_key, current_hash_version())
                .map_err(|error| error.to_string())?;
            result.calls.item_load += 1;
            exact_pages &= row.as_ref().is_some_and(|row| {
                row.item.container_key.as_deref() == Some(key.as_str())
                    && row.item.page_index == Some(page as u32)
                    && row.item.mtime == item_mtime
                    && row.item.file_size == item_size
            });
        }
    }
    let member_count = if freshness == Freshness::Current && exact_pages {
        let keys = db
            .item_keys_for_container(&key)
            .map_err(|error| error.to_string())?;
        result.calls.item_key_list += 1;
        let count = keys.len() as u64;
        result.seen_items.extend(keys);
        count
    } else {
        0
    };
    result.probe.record(
        identity,
        container_code(freshness, exact_pages, member_count),
    );
    if class == UnitClass::NewProbe {
        let missing = fixed_key("new-book", container_id, key_bytes);
        let freshness = db
            .container_freshness(&missing, mtime, file_size, 1, current_hash_version())
            .map_err(|error| error.to_string())?;
        result.calls.container_freshness += 1;
        result
            .probe
            .record(identity, 0x41 + freshness_code(freshness));
        result.seen_containers.insert(missing);
    }
    Ok(())
}

fn probe_inventory_container(
    inventory: &FullReconcileInventory,
    contained_rows: u64,
    container_id: u64,
    key_bytes: usize,
    result: &mut InventoryWorkerResult,
) {
    result.probe.units += 1;
    let identity = (1u64 << 63) | container_id;
    let class = unit_class(container_id);
    result.probe.record(identity, 0x30 + class_code(class));
    if class == UnitClass::Missing {
        return;
    }
    let key = container_key(container_id, key_bytes);
    let pages = container_page_count(contained_rows, container_id);
    let (mtime, file_size) = container_metadata(container_id, pages);
    let observed_mtime = if class == UnitClass::StaleMetadata {
        mtime + 1
    } else {
        mtime
    };
    inventory.observe_container(&key);
    result.calls.inventory_container_observe += 1;
    let observation = inventory.container_observation(
        &key,
        observed_mtime,
        file_size,
        pages as u32,
        current_hash_version(),
    );
    let mut exact_pages = true;
    if observation.freshness == Freshness::Current
        && container_kind(container_id) == ContainerKind::ImageFolder
    {
        for page in 0..pages {
            let rank = container_id * CONTAINER_PAGES + page;
            let index = contained_item_index(rank);
            let item_key = item_key(index, key_bytes);
            let (item_mtime, item_size) = item_metadata(index);
            let observed = inventory.observe_item(
                &item_key,
                Some(&key),
                Some(page as u32),
                item_mtime,
                item_size,
            );
            result.calls.inventory_item_observe += 1;
            exact_pages &= observed.exact_current;
        }
    }
    let member_count = if observation.freshness == Freshness::Current && exact_pages {
        u64::from(inventory.container_member_count(&key))
    } else {
        0
    };
    result.probe.record(
        identity,
        container_code(observation.freshness, exact_pages, member_count),
    );
    if class == UnitClass::NewProbe {
        let missing = fixed_key("new-book", container_id, key_bytes);
        let observation =
            inventory.container_observation(&missing, mtime, file_size, 1, current_hash_version());
        result.calls.inventory_container_observe += 1;
        result
            .probe
            .record(identity, 0x41 + freshness_code(observation.freshness));
    }
}

fn class_code(class: UnitClass) -> u64 {
    match class {
        UnitClass::Current => 0,
        UnitClass::OldHash => 1,
        UnitClass::StaleMetadata => 2,
        UnitClass::Missing => 3,
        UnitClass::NewProbe => 4,
    }
}

fn observation_code(exact: bool, reusable: bool) -> u64 {
    0x100 + exact as u64 + 2 * reusable as u64
}

fn freshness_code(freshness: Freshness) -> u64 {
    match freshness {
        Freshness::Missing => 0,
        Freshness::Stale => 1,
        Freshness::Current => 2,
    }
}

fn container_code(freshness: Freshness, exact_pages: bool, member_count: u64) -> u64 {
    0x200 + freshness_code(freshness) + 4 * exact_pages as u64 + member_count.wrapping_mul(16)
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize)]
struct ProcessMemory {
    working_set: u64,
    peak_working_set: u64,
    private_bytes: u64,
    peak_pagefile_bytes: u64,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize)]
struct StrategyLiveMemory {
    sample: ProcessMemory,
    working_set_delta_from_sampler_start: u64,
    private_delta_from_sampler_start: u64,
}

#[cfg(windows)]
fn process_memory() -> ProcessMemory {
    use std::ffi::c_void;

    #[repr(C)]
    struct ProcessMemoryCountersEx {
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
        private_usage: usize,
    }

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetCurrentProcess() -> *mut c_void;
    }
    #[link(name = "psapi")]
    unsafe extern "system" {
        fn GetProcessMemoryInfo(
            process: *mut c_void,
            counters: *mut ProcessMemoryCountersEx,
            size: u32,
        ) -> i32;
    }

    let mut counters = ProcessMemoryCountersEx {
        cb: std::mem::size_of::<ProcessMemoryCountersEx>() as u32,
        page_fault_count: 0,
        peak_working_set_size: 0,
        working_set_size: 0,
        quota_peak_paged_pool_usage: 0,
        quota_paged_pool_usage: 0,
        quota_peak_non_paged_pool_usage: 0,
        quota_non_paged_pool_usage: 0,
        pagefile_usage: 0,
        peak_pagefile_usage: 0,
        private_usage: 0,
    };
    let ok = unsafe {
        GetProcessMemoryInfo(
            GetCurrentProcess(),
            &mut counters,
            std::mem::size_of::<ProcessMemoryCountersEx>() as u32,
        )
    };
    assert_ne!(ok, 0, "GetProcessMemoryInfo failed");
    ProcessMemory {
        working_set: counters.working_set_size as u64,
        peak_working_set: counters.peak_working_set_size as u64,
        private_bytes: counters.private_usage as u64,
        peak_pagefile_bytes: counters.peak_pagefile_usage as u64,
    }
}

#[cfg(not(windows))]
fn process_memory() -> ProcessMemory {
    ProcessMemory::default()
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
struct ResourceMetrics {
    initial: ProcessMemory,
    final_sample: ProcessMemory,
    sampled_peak_working_set: u64,
    sampled_peak_private_bytes: u64,
    sampled_working_set_delta: u64,
    sampled_private_delta: u64,
    initial_db_bytes: u64,
    peak_db_bytes: u64,
    final_db_bytes: u64,
    initial_wal_bytes: u64,
    peak_wal_bytes: u64,
    final_wal_bytes: u64,
    initial_temp_bytes: u64,
    peak_temp_bytes: u64,
    final_temp_bytes: u64,
}

struct ResourceSampler {
    stop: Arc<AtomicBool>,
    peak_working_set: Arc<AtomicU64>,
    peak_private: Arc<AtomicU64>,
    peak_db: Arc<AtomicU64>,
    peak_wal: Arc<AtomicU64>,
    peak_temp: Arc<AtomicU64>,
    error: Arc<Mutex<Option<String>>>,
    initial: ProcessMemory,
    initial_db: u64,
    initial_wal: u64,
    initial_temp: u64,
    db_path: PathBuf,
    temp_dir: PathBuf,
    worker: Option<std::thread::JoinHandle<()>>,
}

impl ResourceSampler {
    fn start(db_path: &Path, temp_dir: &Path) -> Result<Self, String> {
        let initial = process_memory();
        let initial_db = file_len(db_path);
        let initial_wal = file_len(&wal_path(db_path));
        let initial_temp = directory_bytes(temp_dir)?;
        let stop = Arc::new(AtomicBool::new(false));
        let peak_working_set = Arc::new(AtomicU64::new(initial.working_set));
        let peak_private = Arc::new(AtomicU64::new(initial.private_bytes));
        let peak_db = Arc::new(AtomicU64::new(initial_db));
        let peak_wal = Arc::new(AtomicU64::new(initial_wal));
        let peak_temp = Arc::new(AtomicU64::new(initial_temp));
        let error = Arc::new(Mutex::new(None));
        let worker_stop = Arc::clone(&stop);
        let worker_working_set = Arc::clone(&peak_working_set);
        let worker_private = Arc::clone(&peak_private);
        let worker_db_bytes = Arc::clone(&peak_db);
        let worker_wal = Arc::clone(&peak_wal);
        let worker_temp = Arc::clone(&peak_temp);
        let worker_error = Arc::clone(&error);
        let worker_db = db_path.to_path_buf();
        let worker_temp_dir = temp_dir.to_path_buf();
        let worker = std::thread::spawn(move || {
            while !worker_stop.load(AtomicOrdering::Acquire) {
                let memory = process_memory();
                worker_working_set.fetch_max(memory.working_set, AtomicOrdering::Relaxed);
                worker_private.fetch_max(memory.private_bytes, AtomicOrdering::Relaxed);
                worker_db_bytes.fetch_max(file_len(&worker_db), AtomicOrdering::Relaxed);
                worker_wal.fetch_max(file_len(&wal_path(&worker_db)), AtomicOrdering::Relaxed);
                match directory_bytes(&worker_temp_dir) {
                    Ok(bytes) => {
                        worker_temp.fetch_max(bytes, AtomicOrdering::Relaxed);
                    }
                    Err(error) => {
                        *worker_error
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(error);
                        break;
                    }
                }
                std::thread::sleep(Duration::from_millis(50));
            }
        });
        Ok(Self {
            stop,
            peak_working_set,
            peak_private,
            peak_db,
            peak_wal,
            peak_temp,
            error,
            initial,
            initial_db,
            initial_wal,
            initial_temp,
            db_path: db_path.to_path_buf(),
            temp_dir: temp_dir.to_path_buf(),
            worker: Some(worker),
        })
    }

    /// Samples the process while the selected reconcile strategy still owns its
    /// complete live state. This is intentionally O(1) and runs after all probe
    /// workers join but before either finalizer consumes or drops that state.
    fn strategy_live_sample(&self) -> StrategyLiveMemory {
        let sample = process_memory();
        self.peak_working_set
            .fetch_max(sample.working_set, AtomicOrdering::Relaxed);
        self.peak_private
            .fetch_max(sample.private_bytes, AtomicOrdering::Relaxed);
        StrategyLiveMemory {
            sample,
            working_set_delta_from_sampler_start: sample
                .working_set
                .saturating_sub(self.initial.working_set),
            private_delta_from_sampler_start: sample
                .private_bytes
                .saturating_sub(self.initial.private_bytes),
        }
    }

    fn finish(mut self) -> Result<ResourceMetrics, String> {
        self.stop.store(true, AtomicOrdering::Release);
        if let Some(worker) = self.worker.take() {
            worker
                .join()
                .map_err(|_| "resource sampler panicked".to_owned())?;
        }
        if let Some(error) = self
            .error
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take()
        {
            return Err(format!("resource sampler stopped: {error}"));
        }
        let final_sample = process_memory();
        self.peak_working_set
            .fetch_max(final_sample.working_set, AtomicOrdering::Relaxed);
        self.peak_private
            .fetch_max(final_sample.private_bytes, AtomicOrdering::Relaxed);
        let final_db = file_len(&self.db_path);
        let final_wal = file_len(&wal_path(&self.db_path));
        let final_temp = directory_bytes(&self.temp_dir)?;
        self.peak_db.fetch_max(final_db, AtomicOrdering::Relaxed);
        self.peak_wal.fetch_max(final_wal, AtomicOrdering::Relaxed);
        self.peak_temp
            .fetch_max(final_temp, AtomicOrdering::Relaxed);
        let sampled_peak_working_set = self.peak_working_set.load(AtomicOrdering::Relaxed);
        let sampled_peak_private_bytes = self.peak_private.load(AtomicOrdering::Relaxed);
        Ok(ResourceMetrics {
            initial: self.initial,
            final_sample,
            sampled_peak_working_set,
            sampled_peak_private_bytes,
            sampled_working_set_delta: sampled_peak_working_set
                .saturating_sub(self.initial.working_set),
            sampled_private_delta: sampled_peak_private_bytes
                .saturating_sub(self.initial.private_bytes),
            initial_db_bytes: self.initial_db,
            peak_db_bytes: self.peak_db.load(AtomicOrdering::Relaxed),
            final_db_bytes: final_db,
            initial_wal_bytes: self.initial_wal,
            peak_wal_bytes: self.peak_wal.load(AtomicOrdering::Relaxed),
            final_wal_bytes: final_wal,
            initial_temp_bytes: self.initial_temp,
            peak_temp_bytes: self.peak_temp.load(AtomicOrdering::Relaxed),
            final_temp_bytes: final_temp,
        })
    }
}

fn file_len(path: &Path) -> u64 {
    std::fs::metadata(path).map_or(0, |metadata| metadata.len())
}

fn wal_path(db_path: &Path) -> PathBuf {
    let mut path = db_path.as_os_str().to_os_string();
    path.push("-wal");
    PathBuf::from(path)
}

fn directory_bytes(path: &Path) -> Result<u64, String> {
    reject_reparse_path(path)?;
    let mut total = 0u64;
    for entry in
        std::fs::read_dir(path).map_err(|error| format!("read_dir {}: {error}", path.display()))?
    {
        let entry = entry.map_err(|error| error.to_string())?;
        let path = entry.path();
        reject_reparse_path(&path)?;
        let metadata = entry.metadata().map_err(|error| error.to_string())?;
        if metadata.is_dir() {
            total = total.saturating_add(directory_bytes(&path)?);
        } else if metadata.is_file() {
            total = total.saturating_add(metadata.len());
        }
    }
    Ok(total)
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
struct SummarySnapshot {
    completed_at_unix_secs: i64,
    registered_items: u64,
    password_required_pdfs: u64,
    corrupt_containers: u64,
    zero_page_containers: u64,
    decode_failures: u64,
    io_failures: u64,
}

impl From<StoredIndexSummary> for SummarySnapshot {
    fn from(summary: StoredIndexSummary) -> Self {
        Self {
            completed_at_unix_secs: summary.completed_at_unix_secs,
            registered_items: summary.registered_items,
            password_required_pdfs: summary.stats.password_required_pdfs,
            corrupt_containers: summary.stats.corrupt_containers,
            zero_page_containers: summary.stats.zero_page_containers,
            decode_failures: summary.stats.decode_failures,
            io_failures: summary.stats.io_failures,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
struct StateFingerprint {
    item_rows: u64,
    container_rows: u64,
    item_sha256: String,
    container_sha256: String,
    journal_rows: u64,
    journal_multiset_sha256: String,
    summary: Option<SummarySnapshot>,
    store_id_hex: String,
}

fn state_fingerprint(conn: &Connection) -> rusqlite::Result<StateFingerprint> {
    let mut item_hash = Sha256::new();
    let mut item_rows = 0u64;
    {
        let mut statement = conn.prepare(
            "SELECT item_id, item_key, revision, kind, container_key, page_index, mtime,
                    file_size, hash_version, pdq256, quality, width, height, format
             FROM item ORDER BY item_id",
        )?;
        let mut rows = statement.query([])?;
        while let Some(row) = rows.next()? {
            hash_i64(&mut item_hash, row.get(0)?);
            hash_string(&mut item_hash, &row.get::<_, String>(1)?);
            hash_i64(&mut item_hash, row.get(2)?);
            hash_i64(&mut item_hash, row.get(3)?);
            hash_optional_string(&mut item_hash, row.get::<_, Option<String>>(4)?.as_deref());
            hash_optional_i64(&mut item_hash, row.get(5)?);
            for column in 6..=8 {
                hash_i64(&mut item_hash, row.get(column)?);
            }
            hash_bytes(&mut item_hash, &row.get::<_, Vec<u8>>(9)?);
            for column in 10..=13 {
                hash_i64(&mut item_hash, row.get(column)?);
            }
            item_rows += 1;
        }
    }

    let mut container_hash = Sha256::new();
    let mut container_rows = 0u64;
    {
        let mut statement = conn.prepare(
            "SELECT container_key, kind, page_count, scan_state, generation, mtime, file_size
             FROM container ORDER BY container_key",
        )?;
        let mut rows = statement.query([])?;
        while let Some(row) = rows.next()? {
            hash_string(&mut container_hash, &row.get::<_, String>(0)?);
            hash_i64(&mut container_hash, row.get(1)?);
            hash_optional_i64(&mut container_hash, row.get(2)?);
            for column in 3..=6 {
                hash_i64(&mut container_hash, row.get(column)?);
            }
            container_rows += 1;
        }
    }

    let mut journal_hash = Sha256::new();
    let mut journal_rows = 0u64;
    {
        let mut statement =
            conn.prepare("SELECT item_id, op FROM item_change ORDER BY item_id, op")?;
        let mut rows = statement.query([])?;
        while let Some(row) = rows.next()? {
            hash_i64(&mut journal_hash, row.get(0)?);
            hash_i64(&mut journal_hash, row.get(1)?);
            journal_rows += 1;
        }
    }
    let summary = conn
        .query_row(
            "SELECT completed_at_unix_secs, registered_items, password_required_pdfs,
                    corrupt_containers, zero_page_containers, decode_failures, io_failures
             FROM index_run WHERE singleton=1",
            [],
            |row| {
                Ok(SummarySnapshot {
                    completed_at_unix_secs: row.get(0)?,
                    registered_items: i64_to_u64(row.get(1)?, 1)?,
                    password_required_pdfs: i64_to_u64(row.get(2)?, 2)?,
                    corrupt_containers: i64_to_u64(row.get(3)?, 3)?,
                    zero_page_containers: i64_to_u64(row.get(4)?, 4)?,
                    decode_failures: i64_to_u64(row.get(5)?, 5)?,
                    io_failures: i64_to_u64(row.get(6)?, 6)?,
                })
            },
        )
        .optional()?;
    let store_id = conn.query_row(
        "SELECT store_id FROM search_content_state WHERE singleton=1",
        [],
        |row| row.get::<_, Vec<u8>>(0),
    )?;
    Ok(StateFingerprint {
        item_rows,
        container_rows,
        item_sha256: hex_digest(item_hash.finalize()),
        container_sha256: hex_digest(container_hash.finalize()),
        journal_rows,
        journal_multiset_sha256: hex_digest(journal_hash.finalize()),
        summary,
        store_id_hex: store_id.iter().map(|byte| format!("{byte:02x}")).collect(),
    })
}

fn hash_i64(hash: &mut Sha256, value: i64) {
    hash.update(value.to_le_bytes());
}

fn hash_optional_i64(hash: &mut Sha256, value: Option<i64>) {
    hash.update([u8::from(value.is_some())]);
    if let Some(value) = value {
        hash_i64(hash, value);
    }
}

fn hash_string(hash: &mut Sha256, value: &str) {
    hash_bytes(hash, value.as_bytes());
}

fn hash_optional_string(hash: &mut Sha256, value: Option<&str>) {
    hash.update([u8::from(value.is_some())]);
    if let Some(value) = value {
        hash_string(hash, value);
    }
}

fn hash_bytes(hash: &mut Sha256, value: &[u8]) {
    hash.update((value.len() as u64).to_le_bytes());
    hash.update(value);
}

fn hex_digest(bytes: impl AsRef<[u8]>) -> String {
    bytes
        .as_ref()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct InventoryAccountingSnapshot {
    item_count: usize,
    item_capacity: usize,
    item_map_capacity: usize,
    container_count: usize,
    container_capacity: usize,
    container_map_capacity: usize,
    key_bytes: usize,
    item_record_capacity_bytes: usize,
    container_record_capacity_bytes: usize,
    item_bitset_bytes: usize,
    container_bitset_bytes: usize,
    item_map_growths: usize,
    item_record_growths: usize,
    container_map_growths: usize,
    container_record_growths: usize,
}

impl From<FullInventoryAccounting> for InventoryAccountingSnapshot {
    fn from(accounting: FullInventoryAccounting) -> Self {
        Self {
            item_count: accounting.item_count,
            item_capacity: accounting.item_capacity,
            item_map_capacity: accounting.item_map_capacity,
            container_count: accounting.container_count,
            container_capacity: accounting.container_capacity,
            container_map_capacity: accounting.container_map_capacity,
            key_bytes: accounting.key_bytes,
            item_record_capacity_bytes: accounting.item_record_capacity_bytes,
            container_record_capacity_bytes: accounting.container_record_capacity_bytes,
            item_bitset_bytes: accounting.item_bitset_bytes,
            container_bitset_bytes: accounting.container_bitset_bytes,
            item_map_growths: accounting.item_map_growths,
            item_record_growths: accounting.item_record_growths,
            container_map_growths: accounting.container_map_growths,
            container_record_growths: accounting.container_record_growths,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct TrialResult {
    mode: String,
    trial: String,
    dataset: DatasetCounts,
    workers: usize,
    copy_ms: f64,
    load_ms: f64,
    lookup_ms: f64,
    finalize_ms: f64,
    total_ms: f64,
    items_per_second: f64,
    removed_rows_and_containers: usize,
    start_change_seq: u64,
    end_change_seq: u64,
    probe: ProbeSummary,
    strategy_live_sample: StrategyLiveMemory,
    logical_calls: LogicalCalls,
    inventory: Option<InventoryAccountingSnapshot>,
    state: StateFingerprint,
    resources: ResourceMetrics,
    limitations: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct SetupResult {
    mode: String,
    dataset: DatasetCounts,
    setup_ms: f64,
    resources: ResourceMetrics,
    state: StateFingerprint,
    note: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct VerificationResult {
    mode: String,
    dataset: DatasetCounts,
    verified_trials: Vec<String>,
    ab_equal: bool,
    ba_equal: bool,
    repeated_legacy_equal: bool,
    repeated_inventory_equal: bool,
    database_fingerprints_match_results: bool,
    verify_ms: f64,
    resources: ResourceMetrics,
    note: String,
}

fn write_json_create_new(path: &Path, value: &impl Serialize) -> Result<(), Box<dyn Error>> {
    let file = OpenOptions::new().write(true).create_new(true).open(path)?;
    let mut writer = BufWriter::new(file);
    serde_json::to_writer_pretty(&mut writer, value)?;
    writer.write_all(b"\n")?;
    writer.flush()?;
    Ok(())
}

fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<T, Box<dyn Error>> {
    Ok(serde_json::from_reader(BufReader::new(File::open(path)?))?)
}

fn copy_create_new(source: &Path, destination: &Path) -> Result<u64, Box<dyn Error>> {
    let mut source = File::open(source)?;
    let mut destination = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(destination)?;
    let copied = std::io::copy(&mut source, &mut destination)?;
    destination.sync_all()?;
    Ok(copied)
}

fn configure_benchmark_connection(db: &SimilarDb) -> rusqlite::Result<()> {
    db.conn
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .execute_batch("PRAGMA temp_store=FILE; PRAGMA synchronous=NORMAL;")
}

fn checkpoint(db: &SimilarDb) -> rusqlite::Result<()> {
    db.conn
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")
}

fn account_finalizer_sql(mode: BenchMode, dataset: &DatasetCounts, calls: &mut LogicalCalls) {
    let removed_items = dataset.classified_rows.missing;
    let removed_containers = dataset.container_units.missing;
    calls.finalizer = 1;
    calls.finalizer_item_journal_inserts = removed_items;
    calls.finalizer_item_deletes = removed_items;
    calls.finalizer_container_deletes = removed_containers;
    calls.summary_count_queries = 1;
    calls.summary_writes = 1;
    match mode {
        BenchMode::Legacy => {
            calls.finalizer_all_item_key_scans = 1;
            calls.finalizer_all_container_key_scans = 1;
            calls.finalizer_item_row_lookups = dataset.rows;
            calls.finalizer_container_member_lists = removed_containers;
        }
        BenchMode::Inventory => {
            calls.finalizer_container_identity_lookups = dataset.containers;
        }
        BenchMode::Setup | BenchMode::Verify => unreachable!(),
    }
}

fn setup(config: &BenchConfig) -> Result<(), Box<dyn Error>> {
    let base_db = config.base_db();
    let result_path = config.run_dir.join("setup.json");
    if base_db.exists() || result_path.exists() {
        return Err("setup refuses to overwrite base.db or setup.json".into());
    }
    let sampler = ResourceSampler::start(&base_db, &config.temp_dir)?;
    let started = Instant::now();
    drop(SimilarDb::open_at(&base_db)?);
    let mut conn = Connection::open(&base_db)?;
    conn.execute_batch("PRAGMA synchronous=OFF; PRAGMA temp_store=FILE;")?;
    let transaction = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let dataset = DatasetCounts::new(config.rows, config.key_bytes);
    // `item/%010d/` and `book/%010d/` are both 16 bytes. The run identity limits
    // key size to the three reviewed cardinalities, so this padding is fixed once
    // per setup process and not consulted from the row-generation loop.
    let key_padding = config.key_bytes - 16;
    if dataset.containers > 0 {
        let insert_containers = format!(
            "WITH RECURSIVE ids(value) AS (
               VALUES(0) UNION ALL SELECT value + 1 FROM ids WHERE value + 1 < ?1
             )
             INSERT INTO container
               (container_key, kind, page_count, scan_state, generation, mtime, file_size)
             SELECT printf('book/%010d/', value) || replace(printf('%{key_padding}s', ''), ' ', 'x'),
                    value % 3,
                    min(256, ?2 - value * 256),
                    1, 1, 30000000 + value,
                    40000 + min(256, ?2 - value * 256)
             FROM ids"
        );
        transaction.execute(
            &insert_containers,
            params![
                i64::try_from(dataset.containers)?,
                i64::try_from(dataset.contained_rows)?
            ],
        )?;
    }
    let insert_items = format!(
        "WITH RECURSIVE ids(value) AS (
           VALUES(0) UNION ALL SELECT value + 1 FROM ids WHERE value + 1 < ?1
         ), positions AS (
           SELECT value,
                  CASE WHEN value % 4 = 0 THEN NULL
                       ELSE (value / 4) * 3 + (value % 4) - 1 END AS member_rank
           FROM ids
         ), classified AS (
           SELECT value, member_rank,
                  CASE WHEN member_rank IS NULL THEN value / 4
                       ELSE (member_rank / 256) END AS unit_id
           FROM positions
         )
         INSERT INTO item
           (item_id, revision, item_key, kind, container_key, page_index, mtime, file_size,
            hash_version, pdq256, quality, width, height, format)
         SELECT value + 1, 1,
                printf('item/%010d/', value) || replace(printf('%{key_padding}s', ''), ' ', 'x'),
                CASE WHEN member_rank IS NULL THEN 0 ELSE (member_rank / 256) % 3 END,
                CASE WHEN member_rank IS NULL THEN NULL
                     ELSE printf('book/%010d/', member_rank / 256)
                          || replace(printf('%{key_padding}s', ''), ' ', 'x') END,
                CASE WHEN member_rank IS NULL THEN NULL ELSE member_rank % 256 END,
                10000000 + value, 20000 + (value % 1000),
                CASE WHEN unit_id % 100 = 0 AND (unit_id / 100) % 4 = 0
                     THEN ?2 - 1 ELSE ?2 END,
                zeroblob(32), 50, 100, 100, 1
         FROM classified"
    );
    transaction.execute(
        &insert_items,
        params![i64::try_from(config.rows)?, current_hash_version()],
    )?;
    transaction.execute(
        "INSERT INTO index_run
           (singleton, hash_version, completed_at_unix_secs, registered_items,
            password_required_pdfs, corrupt_containers, zero_page_containers,
            decode_failures, io_failures)
         VALUES (1, ?1, 1, ?2, 0, 0, 0, 0, 0)",
        params![current_hash_version(), i64::try_from(config.rows)?],
    )?;
    transaction.commit()?;
    conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")?;
    let setup_ms = started.elapsed().as_secs_f64() * 1_000.0;
    let resources = sampler.finish()?;
    let state = state_fingerprint(&conn)?;
    drop(conn);
    let result = SetupResult {
        mode: "setup".to_owned(),
        dataset,
        setup_ms,
        resources,
        state,
        note: "Synthetic only; no APPDATA or production DB. Time is unmeasured until this mode runs. SQLite temp_store=FILE and TEMP/TMP are confined to run/temp.".to_owned(),
    };
    write_json_create_new(&result_path, &result)?;
    eprintln!(
        "full_inventory_benchmark setup rows={} key_bytes={} elapsed_ms={setup_ms:.3} result={}",
        config.rows,
        config.key_bytes,
        result_path.display()
    );
    Ok(())
}

fn run_trial(config: &BenchConfig) -> Result<(), Box<dyn Error>> {
    let result_path = config.trial_result();
    let db_path = config.trial_db();
    if result_path.exists() || db_path.exists() {
        return Err(format!(
            "trial refuses to overwrite {} or {}",
            result_path.display(),
            db_path.display()
        )
        .into());
    }
    let setup: SetupResult = read_json(&config.run_dir.join("setup.json"))?;
    let dataset = DatasetCounts::new(config.rows, config.key_bytes);
    if setup.dataset != dataset {
        return Err("setup.json row count or dataset contract does not match this trial".into());
    }
    let copy_started = Instant::now();
    copy_create_new(&config.base_db(), &db_path)?;
    let copy_ms = copy_started.elapsed().as_secs_f64() * 1_000.0;
    let db = SimilarDb::open_at(&db_path)?;
    configure_benchmark_connection(&db)?;
    let start_change_seq = db.change_watermark()?.through_change_seq;
    let sampler = ResourceSampler::start(&db_path, &config.temp_dir)?;
    let total_started = Instant::now();

    let (
        load_ms,
        lookup_ms,
        finalize_ms,
        removed,
        probe,
        strategy_live_sample,
        mut calls,
        inventory_accounting,
    ) = match config.mode {
        BenchMode::Legacy => {
            let lookup_started = Instant::now();
            let (seen_items, seen_containers, probe, calls) =
                run_legacy_probes(&db, &dataset).map_err(std::io::Error::other)?;
            let lookup_ms = lookup_started.elapsed().as_secs_f64() * 1_000.0;
            let strategy_live_sample = sampler.strategy_live_sample();
            let finalize_started = Instant::now();
            let removed = db
                .finalize_full_reconcile_if(
                    &seen_items,
                    &seen_containers,
                    current_hash_version(),
                    FIXED_COMPLETED_AT,
                    CompletedIndexStats::default(),
                    || true,
                )?
                .0;
            let finalize_ms = finalize_started.elapsed().as_secs_f64() * 1_000.0;
            drop(seen_items);
            drop(seen_containers);
            (
                0.0,
                lookup_ms,
                finalize_ms,
                removed,
                probe,
                strategy_live_sample,
                calls,
                None,
            )
        }
        BenchMode::Inventory => {
            let load_started = Instant::now();
            let inventory = db
                .load_full_reconcile_inventory(current_hash_version(), || true)?
                .ok_or("inventory load was unexpectedly cancelled")?;
            let load_ms = load_started.elapsed().as_secs_f64() * 1_000.0;
            let accounting = InventoryAccountingSnapshot::from(inventory.accounting());
            let lookup_started = Instant::now();
            let (probe, calls) =
                run_inventory_probes(&inventory, &dataset).map_err(std::io::Error::other)?;
            let lookup_ms = lookup_started.elapsed().as_secs_f64() * 1_000.0;
            let strategy_live_sample = sampler.strategy_live_sample();
            let finalize_started = Instant::now();
            let removed = db
                .finalize_full_reconcile_inventory_if(
                    inventory,
                    current_hash_version(),
                    FIXED_COMPLETED_AT,
                    CompletedIndexStats::default(),
                    || true,
                )?
                .0;
            let finalize_ms = finalize_started.elapsed().as_secs_f64() * 1_000.0;
            (
                load_ms,
                lookup_ms,
                finalize_ms,
                removed,
                probe,
                strategy_live_sample,
                calls,
                Some(accounting),
            )
        }
        BenchMode::Setup | BenchMode::Verify => unreachable!(),
    };
    account_finalizer_sql(config.mode, &dataset, &mut calls);
    if config.mode == BenchMode::Inventory {
        calls.inventory_load = 1;
    }
    let total_ms = total_started.elapsed().as_secs_f64() * 1_000.0;
    let resources = sampler.finish()?;
    let end_change_seq = db.change_watermark()?.through_change_seq;
    if end_change_seq < start_change_seq {
        return Err("item_change watermark moved backwards".into());
    }
    let state = {
        let conn = db.conn.lock().unwrap_or_else(|error| error.into_inner());
        state_fingerprint(&conn)?
    };
    checkpoint(&db)?;
    drop(db);
    let trial = config.trial.as_deref().unwrap().to_owned();
    let result = TrialResult {
        mode: config.mode.label().to_owned(),
        trial: trial.clone(),
        dataset,
        workers: WORKERS,
        copy_ms,
        load_ms,
        lookup_ms,
        finalize_ms,
        total_ms,
        items_per_second: config.rows as f64 / (total_ms / 1_000.0),
        removed_rows_and_containers: removed,
        start_change_seq,
        end_change_seq,
        probe,
        strategy_live_sample,
        logical_calls: calls,
        inventory: inventory_accounting,
        state,
        resources,
        limitations: vec![
            "Times are unmeasured until this mode runs; setup/copy/verify are excluded from total_ms.".to_owned(),
            "OS cache state is not forced or described as warm/cold; AB and BA orders are both required.".to_owned(),
            "Snapshot-after-new/owner-change/container-rebuild races are covered by dedicated safety regressions, not timed against the unsafe legacy prune.".to_owned(),
            "Logical SQL counts combine observed benchmark API calls with the deterministic stable-dataset finalizer statement model; SQLite VM steps and allocator-internal temporary allocations are not traced.".to_owned(),
            "Inventory growth counters count explicit map/vector capacity expansions after the initial capacity-hint reserve; allocator-internal reallocations are not observable.".to_owned(),
            "The sampler belongs to this one process; setup and verify memory are reported by their own processes.".to_owned(),
            "strategy_live_sample is an O(1) process sample after probe workers join and before the strategy state is consumed or dropped; its deltas use the sampler start after copy/open/config/watermark, while resources.final_sample is the post-finalize residual.".to_owned(),
            "The synthetic legacy strategy moves worker String sets into one final set and does not reproduce the product aggregate.snapshot() clone with the aggregate still live; application arrays, decode queues, and other resident state are not included.".to_owned(),
            "dataset.key_bytes is the fixed UTF-8 byte length of each synthetic ASCII key, not an assertion about the average key length in a production database.".to_owned(),
        ],
    };
    write_json_create_new(&result_path, &result)?;
    eprintln!(
        "full_inventory_benchmark mode={} trial={} rows={} key_bytes={} load_ms={load_ms:.3} lookup_ms={lookup_ms:.3} finalize_ms={finalize_ms:.3} total_ms={total_ms:.3} result={}",
        config.mode.label(),
        trial,
        config.rows,
        config.key_bytes,
        result_path.display()
    );
    Ok(())
}

fn read_only_fingerprint(path: &Path) -> Result<StateFingerprint, Box<dyn Error>> {
    let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    conn.execute_batch("PRAGMA query_only=ON; PRAGMA temp_store=FILE;")?;
    Ok(state_fingerprint(&conn)?)
}

fn equivalent_results(left: &TrialResult, right: &TrialResult) -> bool {
    left.dataset == right.dataset
        && left.removed_rows_and_containers == right.removed_rows_and_containers
        && left.probe == right.probe
        && left.state == right.state
        && left.end_change_seq >= left.start_change_seq
        && right.end_change_seq >= right.start_change_seq
}

fn verify(config: &BenchConfig) -> Result<(), Box<dyn Error>> {
    let result_path = config.run_dir.join("verify.json");
    if result_path.exists() {
        return Err("verify refuses to overwrite verify.json".into());
    }
    let names = ["ab-legacy", "ab-inventory", "ba-inventory", "ba-legacy"];
    let results = names
        .iter()
        .map(|name| read_json::<TrialResult>(&config.run_dir.join(format!("{name}.json"))))
        .collect::<Result<Vec<_>, _>>()?;
    let dataset = DatasetCounts::new(config.rows, config.key_bytes);
    if results.iter().any(|result| result.dataset != dataset) {
        return Err("one or more trial results use a different dataset".into());
    }
    let sampler = ResourceSampler::start(&config.base_db(), &config.temp_dir)?;
    let started = Instant::now();
    let actual = names
        .iter()
        .map(|name| read_only_fingerprint(&config.run_dir.join(format!("{name}.db"))))
        .collect::<Result<Vec<_>, _>>()?;
    let database_fingerprints_match_results = results
        .iter()
        .zip(&actual)
        .all(|(result, actual)| result.state == *actual);
    let ab_equal = equivalent_results(&results[0], &results[1]);
    let ba_equal = equivalent_results(&results[3], &results[2]);
    let repeated_legacy_equal = equivalent_results(&results[0], &results[3]);
    let repeated_inventory_equal = equivalent_results(&results[1], &results[2]);
    let verify_ms = started.elapsed().as_secs_f64() * 1_000.0;
    let resources = sampler.finish()?;
    let verified = VerificationResult {
        mode: "verify".to_owned(),
        dataset,
        verified_trials: names.iter().map(|name| (*name).to_owned()).collect(),
        ab_equal,
        ba_equal,
        repeated_legacy_equal,
        repeated_inventory_equal,
        database_fingerprints_match_results,
        verify_ms,
        resources,
        note: "Equality covers stable-snapshot final rows, containers, summary, sorted (item_id, op) journal multiset, probe checksum, and monotonic watermarks. Raw journal sequence order and snapshot-after mutation safety are intentionally outside timed A/B.".to_owned(),
    };
    write_json_create_new(&result_path, &verified)?;
    if !(ab_equal
        && ba_equal
        && repeated_legacy_equal
        && repeated_inventory_equal
        && database_fingerprints_match_results)
    {
        return Err(format!(
            "benchmark verification failed; inspect {}",
            result_path.display()
        )
        .into());
    }
    eprintln!(
        "full_inventory_benchmark verify rows={} elapsed_ms={verify_ms:.3} result={}",
        config.rows,
        result_path.display()
    );
    Ok(())
}

#[test]
#[ignore = "manual target-only setup/legacy/inventory/verify measurement; never uses APPDATA"]
fn measure_full_reconcile_inventory_reference_cardinality() {
    let config = BenchConfig::from_env().unwrap_or_else(|error| panic!("{error}"));
    let mode = config.mode;
    let result = match mode {
        BenchMode::Setup => setup(&config),
        BenchMode::Legacy | BenchMode::Inventory => run_trial(&config),
        BenchMode::Verify => verify(&config),
    };
    result.unwrap_or_else(|error| panic!("{} mode failed: {error}", mode.label()));
}
