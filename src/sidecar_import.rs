//! Transactional sidecar-to-database import engine.
//!
//! This module deliberately has no `App` or UI wiring.  It owns the worker-safe
//! engine boundary for sidecar recovery: loading produces a typed source
//! identity, preparation performs every fallible decode/serialization before
//! database locks are taken, commit uses one transaction for all six edit
//! stores, and the Stage 2 probe/marker-clear primitives expose typed terminal
//! states.  The caller still owns navigation, viewer generations, cache
//! application, writer quiescence, and cancellation lifecycle.

use std::io::Read;
use std::path::{Component, Path};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use rusqlite::{Connection, OpenFlags, OptionalExtension, params};

use crate::sidecar::{
    ImportStats, LoadedSidecarImport, RelKeyKind, SidecarFile, SidecarImportSource, SidecarMask,
    classify_rel_key,
};

const MAX_MASK_PIXELS: usize = 128 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ImportFamilies {
    pub edits: bool,
    pub tags: bool,
}

impl ImportFamilies {
    pub const ALL: Self = Self {
        edits: true,
        tags: true,
    };
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SidecarProbeFamilyOutcome {
    NotRequested,
    AlreadySynchronized,
    ImportRequired,
    MarkerClearRequired,
    Cancelled,
    SourceChanged(String),
    Failed(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SidecarProbeResult {
    pub edits: SidecarProbeFamilyOutcome,
    pub tags: SidecarProbeFamilyOutcome,
    /// Present only when the sidecar content itself failed semantic validation.
    /// The matching `Current` owner is write-disabled for this session.
    pub source_validation_error: Option<String>,
    pub load_elapsed: Duration,
    pub prepare_elapsed: Duration,
    pub probe_elapsed: Duration,
}

/// Read-only decision for the Stage 2 restore coordinator.
///
/// Only `Current` exposes a `SidecarFile` that may be installed as a current
/// cache value.  Write-required and source-changed variants deliberately expose
/// no snapshot; the coordinator must drain writers and run a strict operation.
pub enum SidecarImportProbe {
    Current {
        sidecar: SidecarFile,
        result: SidecarProbeResult,
    },
    ImportRequired {
        result: SidecarProbeResult,
    },
    MarkerClearRequired {
        result: SidecarProbeResult,
    },
    SourceChanged {
        error: String,
        result: SidecarProbeResult,
    },
    Cancelled {
        result: SidecarProbeResult,
    },
    Failed {
        error: String,
        result: SidecarProbeResult,
    },
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MarkerClearReport {
    pub marker_cleared: bool,
    pub transaction_committed: bool,
    pub transaction_elapsed: Duration,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MissingMarkerClearResult {
    pub edits: ImportFamilyOutcome<MarkerClearReport>,
    pub tags: ImportFamilyOutcome<MarkerClearReport>,
    pub elapsed: Duration,
}

/// Terminal result of clearing markers for a source that was strictly Missing.
pub enum MissingMarkerClearCompletion {
    Current {
        sidecar: SidecarFile,
        result: MissingMarkerClearResult,
    },
    SourceChanged {
        error: String,
        result: MissingMarkerClearResult,
    },
    Cancelled {
        result: MissingMarkerClearResult,
    },
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct EditImportDelta {
    pub adjusted: Vec<String>,
    pub masked: Vec<String>,
    pub concealed: Vec<String>,
    pub local_adjusted: Vec<String>,
    pub cropped: Vec<String>,
    pub comic: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct EditImportReport {
    pub stats: ImportStats,
    pub delta: EditImportDelta,
    pub transaction_committed: bool,
    pub sync_marker_recorded: bool,
    pub transaction_elapsed: Duration,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TagImportReport {
    pub imported_items: usize,
    pub inserted_tags: usize,
    pub skipped_decided_items: usize,
    pub imported: Vec<(String, Vec<String>)>,
    pub transaction_committed: bool,
    pub sync_marker_recorded: bool,
    pub transaction_elapsed: Duration,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ImportFamilyOutcome<T> {
    NotRequested,
    AlreadySynchronized,
    Applied(T),
    Cancelled,
    SourceChanged(String),
    Failed(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SidecarImportResult {
    pub edits: ImportFamilyOutcome<EditImportReport>,
    pub tags: ImportFamilyOutcome<TagImportReport>,
    /// Present only when the sidecar content itself failed semantic validation.
    /// The matching `Current` owner is write-disabled for this session.
    pub source_validation_error: Option<String>,
    pub prepare_elapsed: Duration,
    pub commit_elapsed: Duration,
}

/// Terminal state of one prepared import.
///
/// Only `Current` exposes the loaded sidecar.  A cancelled or superseded
/// snapshot can therefore never be installed into the UI/cache by treating all
/// completions alike.
pub enum SidecarImportCompletion {
    Current {
        sidecar: SidecarFile,
        result: SidecarImportResult,
    },
    SourceChanged {
        error: String,
        result: SidecarImportResult,
    },
    Cancelled {
        result: SidecarImportResult,
    },
}

enum PreparedFamily<T> {
    NotRequested,
    Ready(T),
    Cancelled,
    Failed(String),
}

pub struct PreparedSidecarImport {
    folder_key: String,
    sidecar: SidecarFile,
    source: SidecarImportSource,
    edits: PreparedFamily<Vec<PreparedEditRow>>,
    tags: PreparedFamily<Vec<crate::tags_db::PreparedSidecarTagItem>>,
    source_validation_error: Option<String>,
    prepare_elapsed: Duration,
}

pub(crate) enum SidecarCacheValidation {
    Current(SidecarFile),
    ValidationFailed { sidecar: SidecarFile, error: String },
    SourceChanged(String),
    Cancelled,
    Failed(String),
}

#[derive(Clone)]
struct PreparedMask {
    compressed: Vec<u8>,
    width: i64,
    height: i64,
    shapes_json: Option<String>,
}

struct PreparedEditRow {
    key: String,
    adjust_json: Option<String>,
    mask: Option<PreparedMask>,
    conceal: Option<PreparedMask>,
    local_adjust_json: Option<String>,
    crop: Option<crate::export_crop::CropSettings>,
    comic_json: Option<String>,
}

impl PreparedEditRow {
    fn is_empty(&self) -> bool {
        self.adjust_json.is_none()
            && self.mask.is_none()
            && self.conceal.is_none()
            && self.local_adjust_json.is_none()
            && self.crop.is_none()
            && self.comic_json.is_none()
    }
}

/// Validate and serialize an immutable sidecar snapshot before taking any DB lock.
pub fn prepare(
    loaded: LoadedSidecarImport,
    families: ImportFamilies,
    cancel: &AtomicBool,
) -> Result<PreparedSidecarImport, String> {
    let started = Instant::now();
    let (mut sidecar, source) = loaded.into_parts();
    let folder = sidecar.folder().to_path_buf();
    let folder_key = crate::adjustment_db::normalize_path(&folder);

    let mut edit_rows = Vec::new();
    let mut tag_items = Vec::new();
    let mut edit_error = None;
    let mut tag_error = None;
    let mut source_validation_error = None;
    let mut cancelled = false;

    for (relative_key, entry) in sidecar.items() {
        if cancel.load(Ordering::Relaxed) {
            cancelled = true;
            break;
        }

        let has_edit = entry.adjust.is_some()
            || entry.mask.is_some()
            || entry.conceal.is_some()
            || entry
                .local_adjust_layers
                .as_ref()
                .is_some_and(|layers| !layers.is_empty())
            || entry.export_crop.is_some()
            || entry
                .comic
                .as_ref()
                .is_some_and(|objects| !objects.is_empty());
        let has_tags = entry.tags.as_ref().is_some_and(|tags| !tags.is_empty());
        let key = match validate_and_reconstruct_key(&folder, relative_key) {
            Ok(key) => key,
            Err(error) => {
                if source_validation_error.is_none() {
                    source_validation_error = Some(error.clone());
                }
                if families.edits && has_edit && edit_error.is_none() {
                    edit_error = Some(error.clone());
                }
                if families.tags && has_tags && tag_error.is_none() {
                    tag_error = Some(error);
                }
                continue;
            }
        };

        if has_edit {
            match prepare_edit_row(key.clone(), entry) {
                Ok(row) if families.edits && edit_error.is_none() && !row.is_empty() => {
                    edit_rows.push(row);
                }
                Ok(_) => {}
                Err(error) => {
                    let error = format!("{relative_key}: {error}");
                    if source_validation_error.is_none() {
                        source_validation_error = Some(error.clone());
                    }
                    if families.edits && edit_error.is_none() {
                        edit_error = Some(error);
                    }
                }
            }
        }

        if let Some(tags) = entry.tags.as_ref().filter(|tags| !tags.is_empty()) {
            match crate::tags_db::prepare_sidecar_tag_item(key, tags) {
                Ok(Some(item))
                    if families.tags
                        && tag_error.is_none()
                        && matches!(classify_rel_key(relative_key), RelKeyKind::Image) =>
                {
                    tag_items.push(item);
                }
                Ok(None) => {}
                Ok(Some(_)) => {}
                Err(error) => {
                    if source_validation_error.is_none() {
                        source_validation_error = Some(error.clone());
                    }
                    if families.tags
                        && tag_error.is_none()
                        && matches!(classify_rel_key(relative_key), RelKeyKind::Image)
                    {
                        tag_error = Some(error);
                    }
                }
            }
        }
    }

    // A syntactically valid sidecar can still contain an escaping key or an
    // invalid field value.  Keep that exact snapshot available to the caller,
    // but never let a later UI edit flush a partial reconstruction over it.
    // Runtime DB probe/commit errors are handled later and do not disable a
    // source that itself passed validation.
    let source_validation_error = source_validation_error.map(|error| {
        format!(
            "{error}; sidecar writes are disabled for this session to preserve the invalid source"
        )
    });
    if source_validation_error.is_some() {
        sidecar.disable_writes_for_session();
    }

    Ok(PreparedSidecarImport {
        folder_key,
        sidecar,
        source,
        edits: if !families.edits {
            PreparedFamily::NotRequested
        } else if cancelled {
            PreparedFamily::Cancelled
        } else if let Some(error) = edit_error {
            PreparedFamily::Failed(error)
        } else {
            PreparedFamily::Ready(edit_rows)
        },
        tags: if !families.tags {
            PreparedFamily::NotRequested
        } else if cancelled {
            PreparedFamily::Cancelled
        } else if let Some(error) = tag_error {
            PreparedFamily::Failed(error)
        } else {
            PreparedFamily::Ready(tag_items)
        },
        source_validation_error,
        prepare_elapsed: started.elapsed(),
    })
}

/// Validate a loaded source for later cache ownership without touching a DB.
///
/// Unlike a forgiving load, semantic validation failures retain the exact
/// display snapshot as a write-disabled owner. Source changes and cancellation
/// never expose the stale snapshot.
pub(crate) fn validate_cache_snapshot(
    loaded: LoadedSidecarImport,
    families: ImportFamilies,
    cancel: &AtomicBool,
) -> SidecarCacheValidation {
    let prepared = match prepare(loaded, families, cancel) {
        Ok(prepared) => prepared,
        Err(error) => return SidecarCacheValidation::Failed(error),
    };
    let PreparedSidecarImport {
        sidecar,
        source,
        edits,
        tags,
        source_validation_error,
        ..
    } = prepared;
    if cancel.load(Ordering::Relaxed)
        || prepared_family_cancelled(&edits)
        || prepared_family_cancelled(&tags)
    {
        return SidecarCacheValidation::Cancelled;
    }
    if let Err(error) = crate::sidecar::revalidate_import_source(&sidecar, &source) {
        return SidecarCacheValidation::SourceChanged(error);
    }
    if let Some(error) = source_validation_error {
        SidecarCacheValidation::ValidationFailed { sidecar, error }
    } else {
        SidecarCacheValidation::Current(sidecar)
    }
}

/// Strictly load, validate, and inspect v2 markers without mutating a database.
///
/// This function performs filesystem reads, decoding, and SQLite reads and must
/// run on a worker.  A current snapshot is returned only when no requested
/// family needs a write.  Stage 2 must still establish its writer barriers
/// before installing the snapshot or resuming first-display hydration.
pub fn probe(
    folder: &Path,
    data_dir: &Path,
    families: ImportFamilies,
    cancel: &AtomicBool,
) -> SidecarImportProbe {
    let started = Instant::now();
    if cancel.load(Ordering::Relaxed) {
        return SidecarImportProbe::Cancelled {
            result: probe_result(
                started,
                Duration::ZERO,
                Duration::ZERO,
                requested_probe_cancelled(families.edits),
                requested_probe_cancelled(families.tags),
            ),
        };
    }

    let load_started = Instant::now();
    let loaded = SidecarFile::load_for_import(folder);
    let load_elapsed = load_started.elapsed();
    match loaded {
        crate::sidecar::SidecarImportLoad::Loaded(loaded) => {
            let prepared = match prepare(loaded, families, cancel) {
                Ok(prepared) => prepared,
                Err(error) => {
                    return SidecarImportProbe::Failed {
                        error: error.clone(),
                        result: probe_result(
                            started,
                            load_elapsed,
                            Duration::ZERO,
                            requested_probe_failed(families.edits, &error),
                            requested_probe_failed(families.tags, &error),
                        ),
                    };
                }
            };
            let PreparedSidecarImport {
                folder_key,
                sidecar,
                source,
                edits,
                tags,
                source_validation_error,
                prepare_elapsed,
            } = prepared;
            if cancel.load(Ordering::Relaxed)
                || prepared_family_cancelled(&edits)
                || prepared_family_cancelled(&tags)
            {
                return SidecarImportProbe::Cancelled {
                    result: probe_result_with_validation(
                        started,
                        load_elapsed,
                        prepare_elapsed,
                        reject_probe_cancelled(edits),
                        reject_probe_cancelled(tags),
                        source_validation_error,
                    ),
                };
            }
            let marker = match &source {
                SidecarImportSource::Disk(token) => Some(token.sync_marker()),
                SidecarImportSource::PendingWriter { .. } => None,
            };
            let mut edit_outcome =
                probe_prepared_family(edits, data_dir, &folder_key, marker, MarkerStore::Edits);
            let mut tag_outcome =
                probe_prepared_family(tags, data_dir, &folder_key, marker, MarkerStore::Tags);
            if cancel.load(Ordering::Relaxed) {
                edit_outcome = reject_probe_outcome_cancelled(edit_outcome);
                tag_outcome = reject_probe_outcome_cancelled(tag_outcome);
                return SidecarImportProbe::Cancelled {
                    result: probe_result_with_validation(
                        started,
                        load_elapsed,
                        prepare_elapsed,
                        edit_outcome,
                        tag_outcome,
                        source_validation_error,
                    ),
                };
            }
            if let Err(error) = crate::sidecar::revalidate_import_source(&sidecar, &source) {
                edit_outcome = reject_probe_outcome_source_changed(edit_outcome, &error);
                tag_outcome = reject_probe_outcome_source_changed(tag_outcome, &error);
                return SidecarImportProbe::SourceChanged {
                    error,
                    result: probe_result_with_validation(
                        started,
                        load_elapsed,
                        prepare_elapsed,
                        edit_outcome,
                        tag_outcome,
                        source_validation_error,
                    ),
                };
            }
            let result = probe_result_with_validation(
                started,
                load_elapsed,
                prepare_elapsed,
                edit_outcome,
                tag_outcome,
                source_validation_error,
            );
            if probe_requires_import(&result) {
                SidecarImportProbe::ImportRequired { result }
            } else {
                SidecarImportProbe::Current { sidecar, result }
            }
        }
        crate::sidecar::SidecarImportLoad::Missing { sidecar } => {
            let folder_key = crate::adjustment_db::normalize_path(folder);
            let mut edit_outcome =
                probe_missing_family(families.edits, data_dir, &folder_key, MarkerStore::Edits);
            let mut tag_outcome =
                probe_missing_family(families.tags, data_dir, &folder_key, MarkerStore::Tags);
            if cancel.load(Ordering::Relaxed) {
                edit_outcome = reject_probe_outcome_cancelled(edit_outcome);
                tag_outcome = reject_probe_outcome_cancelled(tag_outcome);
                return SidecarImportProbe::Cancelled {
                    result: probe_result(
                        started,
                        load_elapsed,
                        Duration::ZERO,
                        edit_outcome,
                        tag_outcome,
                    ),
                };
            }
            if let Err(error) = crate::sidecar::revalidate_missing_import_source(folder) {
                edit_outcome = reject_probe_outcome_source_changed(edit_outcome, &error);
                tag_outcome = reject_probe_outcome_source_changed(tag_outcome, &error);
                return SidecarImportProbe::SourceChanged {
                    error,
                    result: probe_result(
                        started,
                        load_elapsed,
                        Duration::ZERO,
                        edit_outcome,
                        tag_outcome,
                    ),
                };
            }
            let result = probe_result(
                started,
                load_elapsed,
                Duration::ZERO,
                edit_outcome,
                tag_outcome,
            );
            if probe_requires_marker_clear(&result) {
                SidecarImportProbe::MarkerClearRequired { result }
            } else {
                SidecarImportProbe::Current { sidecar, result }
            }
        }
        crate::sidecar::SidecarImportLoad::Unreadable { sidecar, error }
        | crate::sidecar::SidecarImportLoad::Corrupt { sidecar, error } => {
            terminal_probe(started, load_elapsed, sidecar, families, error)
        }
        crate::sidecar::SidecarImportLoad::UnsupportedVersion { sidecar, version } => {
            terminal_probe(
                started,
                load_elapsed,
                sidecar,
                families,
                format!("sidecar version {version} is newer than this application"),
            )
        }
        crate::sidecar::SidecarImportLoad::WriterFailed { sidecar } => terminal_probe(
            started,
            load_elapsed,
            sidecar,
            families,
            "sidecar writer previously failed for this folder".to_string(),
        ),
        crate::sidecar::SidecarImportLoad::ChangedDuringRead { .. } => {
            let error = "sidecar changed while its import snapshot was being read".to_string();
            SidecarImportProbe::SourceChanged {
                error: error.clone(),
                result: probe_result(
                    started,
                    load_elapsed,
                    Duration::ZERO,
                    requested_probe_source_changed(families.edits, &error),
                    requested_probe_source_changed(families.tags, &error),
                ),
            }
        }
    }
}

#[derive(Clone, Copy)]
enum MarkerStore {
    Edits,
    Tags,
}

impl MarkerStore {
    fn path_and_table(self, data_dir: &Path) -> (std::path::PathBuf, &'static str) {
        match self {
            Self::Edits => (data_dir.join("adjustment.db"), "sidecar_sync"),
            Self::Tags => (data_dir.join("tags.db"), "tag_sidecar_sync"),
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Edits => "edit",
            Self::Tags => "tag",
        }
    }
}

fn probe_result(
    started: Instant,
    load_elapsed: Duration,
    prepare_elapsed: Duration,
    edits: SidecarProbeFamilyOutcome,
    tags: SidecarProbeFamilyOutcome,
) -> SidecarProbeResult {
    SidecarProbeResult {
        edits,
        tags,
        source_validation_error: None,
        load_elapsed,
        prepare_elapsed,
        probe_elapsed: started.elapsed(),
    }
}

fn probe_result_with_validation(
    started: Instant,
    load_elapsed: Duration,
    prepare_elapsed: Duration,
    edits: SidecarProbeFamilyOutcome,
    tags: SidecarProbeFamilyOutcome,
    source_validation_error: Option<String>,
) -> SidecarProbeResult {
    let mut result = probe_result(started, load_elapsed, prepare_elapsed, edits, tags);
    result.source_validation_error = source_validation_error;
    result
}

fn terminal_probe(
    started: Instant,
    load_elapsed: Duration,
    sidecar: SidecarFile,
    families: ImportFamilies,
    error: String,
) -> SidecarImportProbe {
    SidecarImportProbe::Current {
        sidecar,
        result: probe_result(
            started,
            load_elapsed,
            Duration::ZERO,
            requested_probe_failed(families.edits, &error),
            requested_probe_failed(families.tags, &error),
        ),
    }
}

fn requested_probe_cancelled(requested: bool) -> SidecarProbeFamilyOutcome {
    if requested {
        SidecarProbeFamilyOutcome::Cancelled
    } else {
        SidecarProbeFamilyOutcome::NotRequested
    }
}

fn requested_probe_failed(requested: bool, error: &str) -> SidecarProbeFamilyOutcome {
    if requested {
        SidecarProbeFamilyOutcome::Failed(error.to_string())
    } else {
        SidecarProbeFamilyOutcome::NotRequested
    }
}

fn requested_probe_source_changed(requested: bool, error: &str) -> SidecarProbeFamilyOutcome {
    if requested {
        SidecarProbeFamilyOutcome::SourceChanged(error.to_string())
    } else {
        SidecarProbeFamilyOutcome::NotRequested
    }
}

fn reject_probe_cancelled<T>(family: PreparedFamily<T>) -> SidecarProbeFamilyOutcome {
    match family {
        PreparedFamily::NotRequested => SidecarProbeFamilyOutcome::NotRequested,
        PreparedFamily::Ready(_) | PreparedFamily::Cancelled => {
            SidecarProbeFamilyOutcome::Cancelled
        }
        PreparedFamily::Failed(error) => SidecarProbeFamilyOutcome::Failed(error),
    }
}

fn reject_probe_outcome_cancelled(outcome: SidecarProbeFamilyOutcome) -> SidecarProbeFamilyOutcome {
    match outcome {
        SidecarProbeFamilyOutcome::NotRequested => SidecarProbeFamilyOutcome::NotRequested,
        SidecarProbeFamilyOutcome::Failed(error) => SidecarProbeFamilyOutcome::Failed(error),
        _ => SidecarProbeFamilyOutcome::Cancelled,
    }
}

fn reject_probe_outcome_source_changed(
    outcome: SidecarProbeFamilyOutcome,
    error: &str,
) -> SidecarProbeFamilyOutcome {
    match outcome {
        SidecarProbeFamilyOutcome::NotRequested => SidecarProbeFamilyOutcome::NotRequested,
        SidecarProbeFamilyOutcome::Failed(error) => SidecarProbeFamilyOutcome::Failed(error),
        SidecarProbeFamilyOutcome::Cancelled => SidecarProbeFamilyOutcome::Cancelled,
        _ => SidecarProbeFamilyOutcome::SourceChanged(error.to_string()),
    }
}

fn probe_prepared_family<T>(
    family: PreparedFamily<T>,
    data_dir: &Path,
    folder_key: &str,
    marker: Option<i64>,
    store: MarkerStore,
) -> SidecarProbeFamilyOutcome {
    match family {
        PreparedFamily::NotRequested => SidecarProbeFamilyOutcome::NotRequested,
        PreparedFamily::Cancelled => SidecarProbeFamilyOutcome::Cancelled,
        PreparedFamily::Failed(error) => SidecarProbeFamilyOutcome::Failed(error),
        PreparedFamily::Ready(_) => {
            let Some(marker) = marker else {
                return SidecarProbeFamilyOutcome::ImportRequired;
            };
            match read_marker(data_dir, folder_key, store) {
                Ok(Some(current)) if current == marker => {
                    SidecarProbeFamilyOutcome::AlreadySynchronized
                }
                Ok(_) => SidecarProbeFamilyOutcome::ImportRequired,
                Err(error) => SidecarProbeFamilyOutcome::Failed(error),
            }
        }
    }
}

fn probe_missing_family(
    requested: bool,
    data_dir: &Path,
    folder_key: &str,
    store: MarkerStore,
) -> SidecarProbeFamilyOutcome {
    if !requested {
        return SidecarProbeFamilyOutcome::NotRequested;
    }
    match read_marker(data_dir, folder_key, store) {
        Ok(Some(_)) => SidecarProbeFamilyOutcome::MarkerClearRequired,
        Ok(None) => SidecarProbeFamilyOutcome::AlreadySynchronized,
        Err(error) => SidecarProbeFamilyOutcome::Failed(error),
    }
}

fn read_marker(
    data_dir: &Path,
    folder_key: &str,
    store: MarkerStore,
) -> Result<Option<i64>, String> {
    let (path, table) = store.path_and_table(data_dir);
    if !path.is_file() {
        return Err(format!(
            "required {} marker store is missing: {}",
            store.label(),
            path.display()
        ));
    }
    let connection = Connection::open_with_flags(
        &path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|error| {
        format!(
            "cannot open {} marker store read-only: {error}",
            store.label()
        )
    })?;
    connection
        .busy_timeout(Duration::from_secs(2))
        .map_err(|error| format!("cannot configure marker read timeout: {error}"))?;
    connection
        .query_row(
            &format!("SELECT sidecar_mtime FROM {table} WHERE folder_key = ?1"),
            [folder_key],
            |row| row.get::<_, i64>(0),
        )
        .optional()
        .map_err(|error| format!("cannot read {} sidecar marker: {error}", store.label()))
}

fn probe_requires_import(result: &SidecarProbeResult) -> bool {
    matches!(&result.edits, SidecarProbeFamilyOutcome::ImportRequired)
        || matches!(&result.tags, SidecarProbeFamilyOutcome::ImportRequired)
}

fn probe_requires_marker_clear(result: &SidecarProbeResult) -> bool {
    matches!(
        &result.edits,
        SidecarProbeFamilyOutcome::MarkerClearRequired
    ) || matches!(&result.tags, SidecarProbeFamilyOutcome::MarkerClearRequired)
}

/// Clear edit/tag markers after a strict Missing load.
///
/// Each family owns a separate transaction and outcome.  The source is checked
/// before either transaction and again between them, so a newly created or
/// pending sidecar never looks synchronized.  Marker absence is conservative:
/// an external file created after the final check will mismatch on the next
/// probe rather than being skipped.
pub fn clear_missing_markers(
    folder: &Path,
    data_dir: &Path,
    families: ImportFamilies,
    cancel: &AtomicBool,
) -> MissingMarkerClearCompletion {
    clear_missing_markers_with_progress(folder, data_dir, families, cancel, &mut |_, _| {})
}

fn clear_missing_markers_with_progress(
    folder: &Path,
    data_dir: &Path,
    families: ImportFamilies,
    cancel: &AtomicBool,
    progress: &mut impl FnMut(MarkerStore, &AtomicBool),
) -> MissingMarkerClearCompletion {
    let started = Instant::now();
    if cancel.load(Ordering::Relaxed) {
        return MissingMarkerClearCompletion::Cancelled {
            result: MissingMarkerClearResult {
                edits: requested_import_cancelled(families.edits),
                tags: requested_import_cancelled(families.tags),
                elapsed: started.elapsed(),
            },
        };
    }

    let sidecar = match SidecarFile::load_for_import(folder) {
        crate::sidecar::SidecarImportLoad::Missing { sidecar } => sidecar,
        crate::sidecar::SidecarImportLoad::Loaded(_) => {
            let error = "sidecar appeared before missing markers were cleared".to_string();
            return missing_marker_source_changed(started, families, error);
        }
        crate::sidecar::SidecarImportLoad::Unreadable { sidecar, error }
        | crate::sidecar::SidecarImportLoad::Corrupt { sidecar, error } => {
            return missing_marker_terminal(started, sidecar, families, error);
        }
        crate::sidecar::SidecarImportLoad::UnsupportedVersion { sidecar, version } => {
            return missing_marker_terminal(
                started,
                sidecar,
                families,
                format!("sidecar version {version} is newer than this application"),
            );
        }
        crate::sidecar::SidecarImportLoad::WriterFailed { sidecar } => {
            return missing_marker_terminal(
                started,
                sidecar,
                families,
                "sidecar writer previously failed for this folder".to_string(),
            );
        }
        crate::sidecar::SidecarImportLoad::ChangedDuringRead { .. } => {
            return missing_marker_source_changed(
                started,
                families,
                "sidecar changed while checking whether it was missing".to_string(),
            );
        }
    };

    if let Err(error) = crate::sidecar::revalidate_missing_import_source(folder) {
        return MissingMarkerClearCompletion::SourceChanged {
            error: error.clone(),
            result: MissingMarkerClearResult {
                edits: requested_import_source_changed(families.edits, &error),
                tags: requested_import_source_changed(families.tags, &error),
                elapsed: started.elapsed(),
            },
        };
    }
    let folder_key = crate::adjustment_db::normalize_path(folder);
    let edits = if families.edits {
        clear_marker_atomic(data_dir, &folder_key, MarkerStore::Edits, cancel, progress)
    } else {
        ImportFamilyOutcome::NotRequested
    };
    if family_cancelled(&edits) || cancel.load(Ordering::Relaxed) {
        return MissingMarkerClearCompletion::Cancelled {
            result: MissingMarkerClearResult {
                edits,
                tags: requested_import_cancelled(families.tags),
                elapsed: started.elapsed(),
            },
        };
    }

    let mut source_change = None;
    let tags = if families.tags {
        match crate::sidecar::revalidate_missing_import_source(folder) {
            Ok(()) => {
                clear_marker_atomic(data_dir, &folder_key, MarkerStore::Tags, cancel, progress)
            }
            Err(error) => {
                source_change = Some(error.clone());
                ImportFamilyOutcome::SourceChanged(error)
            }
        }
    } else {
        ImportFamilyOutcome::NotRequested
    };
    let result = MissingMarkerClearResult {
        edits,
        tags,
        elapsed: started.elapsed(),
    };
    if family_cancelled(&result.tags) || cancel.load(Ordering::Relaxed) {
        return MissingMarkerClearCompletion::Cancelled { result };
    }
    if let Some(error) = source_change {
        return MissingMarkerClearCompletion::SourceChanged { error, result };
    }
    if let Err(error) = crate::sidecar::revalidate_missing_import_source(folder) {
        return MissingMarkerClearCompletion::SourceChanged { error, result };
    }
    MissingMarkerClearCompletion::Current { sidecar, result }
}

fn missing_marker_terminal(
    started: Instant,
    sidecar: SidecarFile,
    families: ImportFamilies,
    error: String,
) -> MissingMarkerClearCompletion {
    MissingMarkerClearCompletion::Current {
        sidecar,
        result: MissingMarkerClearResult {
            edits: requested_import_failed(families.edits, &error),
            tags: requested_import_failed(families.tags, &error),
            elapsed: started.elapsed(),
        },
    }
}

fn missing_marker_source_changed(
    started: Instant,
    families: ImportFamilies,
    error: String,
) -> MissingMarkerClearCompletion {
    MissingMarkerClearCompletion::SourceChanged {
        error: error.clone(),
        result: MissingMarkerClearResult {
            edits: requested_import_source_changed(families.edits, &error),
            tags: requested_import_source_changed(families.tags, &error),
            elapsed: started.elapsed(),
        },
    }
}

fn requested_import_cancelled<T>(requested: bool) -> ImportFamilyOutcome<T> {
    if requested {
        ImportFamilyOutcome::Cancelled
    } else {
        ImportFamilyOutcome::NotRequested
    }
}

fn requested_import_source_changed<T>(requested: bool, error: &str) -> ImportFamilyOutcome<T> {
    if requested {
        ImportFamilyOutcome::SourceChanged(error.to_string())
    } else {
        ImportFamilyOutcome::NotRequested
    }
}

fn requested_import_failed<T>(requested: bool, error: &str) -> ImportFamilyOutcome<T> {
    if requested {
        ImportFamilyOutcome::Failed(error.to_string())
    } else {
        ImportFamilyOutcome::NotRequested
    }
}

fn clear_marker_atomic(
    data_dir: &Path,
    folder_key: &str,
    store: MarkerStore,
    cancel: &AtomicBool,
    progress: &mut impl FnMut(MarkerStore, &AtomicBool),
) -> ImportFamilyOutcome<MarkerClearReport> {
    if cancel.load(Ordering::Relaxed) {
        return ImportFamilyOutcome::Cancelled;
    }
    let (path, table) = store.path_and_table(data_dir);
    if !path.is_file() {
        return ImportFamilyOutcome::Failed(format!(
            "required {} marker store is missing: {}",
            store.label(),
            path.display()
        ));
    }
    let connection = match Connection::open_with_flags(
        &path,
        OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    ) {
        Ok(connection) => connection,
        Err(error) => {
            return ImportFamilyOutcome::Failed(format!(
                "cannot open {} marker store: {error}",
                store.label()
            ));
        }
    };
    if let Err(error) = connection.busy_timeout(Duration::from_secs(2)) {
        return ImportFamilyOutcome::Failed(format!(
            "cannot configure {} marker clear timeout: {error}",
            store.label()
        ));
    }
    let transaction_started = Instant::now();
    if let Err(error) = connection.execute_batch("BEGIN IMMEDIATE") {
        return ImportFamilyOutcome::Failed(format!(
            "cannot begin {} marker clear: {error}",
            store.label()
        ));
    }
    if cancel.load(Ordering::Relaxed) {
        let _ = connection.execute_batch("ROLLBACK");
        return ImportFamilyOutcome::Cancelled;
    }
    let current = match connection
        .query_row(
            &format!("SELECT sidecar_mtime FROM {table} WHERE folder_key = ?1"),
            [folder_key],
            |row| row.get::<_, i64>(0),
        )
        .optional()
    {
        Ok(current) => current,
        Err(error) => {
            let _ = connection.execute_batch("ROLLBACK");
            return ImportFamilyOutcome::Failed(format!(
                "cannot read {} sidecar marker before clear: {error}",
                store.label()
            ));
        }
    };
    if current.is_none() {
        let _ = connection.execute_batch("ROLLBACK");
        return ImportFamilyOutcome::AlreadySynchronized;
    }
    let deleted = match connection.execute(
        &format!("DELETE FROM {table} WHERE folder_key = ?1"),
        [folder_key],
    ) {
        Ok(deleted) => deleted,
        Err(error) => {
            let _ = connection.execute_batch("ROLLBACK");
            return ImportFamilyOutcome::Failed(format!(
                "cannot clear {} sidecar marker: {error}",
                store.label()
            ));
        }
    };
    progress(store, cancel);
    if cancel.load(Ordering::Relaxed) {
        let _ = connection.execute_batch("ROLLBACK");
        return ImportFamilyOutcome::Cancelled;
    }
    if let Err(error) = connection.execute_batch("COMMIT") {
        let _ = connection.execute_batch("ROLLBACK");
        return ImportFamilyOutcome::Failed(format!(
            "cannot commit {} marker clear: {error}",
            store.label()
        ));
    }
    ImportFamilyOutcome::Applied(MarkerClearReport {
        marker_cleared: deleted > 0,
        transaction_committed: true,
        transaction_elapsed: transaction_started.elapsed(),
    })
}

/// Commit prepared families to the canonical stores rooted at `data_dir`.
///
/// `data_dir` is the only path input.  Attached filenames and schema aliases are
/// fixed below; a sidecar key can never select an ATTACH target or SQL identifier.
pub fn commit(
    data_dir: &Path,
    prepared: PreparedSidecarImport,
    cancel: &AtomicBool,
) -> SidecarImportCompletion {
    commit_with_progress(data_dir, prepared, cancel, |_, _| {})
}

fn commit_with_progress(
    data_dir: &Path,
    prepared: PreparedSidecarImport,
    cancel: &AtomicBool,
    mut progress: impl FnMut(usize, &AtomicBool),
) -> SidecarImportCompletion {
    let commit_started = Instant::now();
    let PreparedSidecarImport {
        folder_key,
        sidecar,
        source,
        edits,
        tags,
        source_validation_error,
        prepare_elapsed,
    } = prepared;

    // Cancellation is checked before source identity.  In particular, a
    // cancelled navigation must not re-read and hash a large disk sidecar, open
    // a database, or rotate tag backups.
    if cancel.load(Ordering::Relaxed)
        || prepared_family_cancelled(&edits)
        || prepared_family_cancelled(&tags)
    {
        return SidecarImportCompletion::Cancelled {
            result: SidecarImportResult {
                edits: reject_cancelled(edits),
                tags: reject_cancelled(tags),
                source_validation_error,
                prepare_elapsed,
                commit_elapsed: commit_started.elapsed(),
            },
        };
    }
    if let Err(error) = crate::sidecar::revalidate_import_source(&sidecar, &source) {
        return SidecarImportCompletion::SourceChanged {
            error: error.clone(),
            result: SidecarImportResult {
                edits: reject_source_changed(edits, &error),
                tags: reject_source_changed(tags, &error),
                source_validation_error,
                prepare_elapsed,
                commit_elapsed: commit_started.elapsed(),
            },
        };
    }
    let marker = match &source {
        SidecarImportSource::Disk(token) => Some(token.sync_marker()),
        SidecarImportSource::PendingWriter { .. } => None,
    };

    let edits = match edits {
        PreparedFamily::NotRequested => ImportFamilyOutcome::NotRequested,
        PreparedFamily::Cancelled => ImportFamilyOutcome::Cancelled,
        PreparedFamily::Failed(error) => ImportFamilyOutcome::Failed(error),
        PreparedFamily::Ready(rows) => {
            match commit_edit_rows(data_dir, &folder_key, marker, &rows, cancel, &mut progress) {
                Ok(outcome) => outcome,
                Err(error) => ImportFamilyOutcome::Failed(error),
            }
        }
    };

    let tags = match tags {
        PreparedFamily::NotRequested => ImportFamilyOutcome::NotRequested,
        PreparedFamily::Cancelled => ImportFamilyOutcome::Cancelled,
        PreparedFamily::Failed(error) => ImportFamilyOutcome::Failed(error),
        PreparedFamily::Ready(items) => {
            if cancel.load(Ordering::Relaxed) {
                ImportFamilyOutcome::Cancelled
            } else {
                let path = data_dir.join("tags.db");
                if !path.is_file() {
                    ImportFamilyOutcome::Failed(format!(
                        "required tag store is missing: {}",
                        path.display()
                    ))
                } else {
                    match crate::tags_db::TagsDb::open_at(&path).and_then(|mut db| {
                        db.import_sidecar_tags_atomic(&folder_key, marker, &items, cancel)
                    }) {
                        Ok(crate::tags_db::SidecarTagBatchOutcome::AlreadySynchronized) => {
                            ImportFamilyOutcome::AlreadySynchronized
                        }
                        Ok(crate::tags_db::SidecarTagBatchOutcome::Cancelled) => {
                            ImportFamilyOutcome::Cancelled
                        }
                        Ok(crate::tags_db::SidecarTagBatchOutcome::Applied(report)) => {
                            ImportFamilyOutcome::Applied(TagImportReport {
                                imported_items: report.imported_items,
                                inserted_tags: report.inserted_tags,
                                skipped_decided_items: report.skipped_decided_items,
                                imported: report.imported,
                                transaction_committed: report.transaction_committed,
                                sync_marker_recorded: report.sync_marker_recorded,
                                transaction_elapsed: report.transaction_elapsed,
                            })
                        }
                        Err(error) => ImportFamilyOutcome::Failed(format!(
                            "tag sidecar import failed: {error}"
                        )),
                    }
                }
            }
        }
    };

    let result = SidecarImportResult {
        edits,
        tags,
        source_validation_error,
        prepare_elapsed,
        commit_elapsed: commit_started.elapsed(),
    };
    if family_cancelled(&result.edits) || family_cancelled(&result.tags) {
        SidecarImportCompletion::Cancelled { result }
    } else {
        SidecarImportCompletion::Current { sidecar, result }
    }
}

fn prepared_family_cancelled<T>(family: &PreparedFamily<T>) -> bool {
    matches!(family, PreparedFamily::Cancelled)
}

fn family_cancelled<T>(family: &ImportFamilyOutcome<T>) -> bool {
    matches!(family, ImportFamilyOutcome::Cancelled)
}

fn reject_cancelled<T, U>(family: PreparedFamily<T>) -> ImportFamilyOutcome<U> {
    match family {
        PreparedFamily::NotRequested => ImportFamilyOutcome::NotRequested,
        PreparedFamily::Ready(_) | PreparedFamily::Cancelled => ImportFamilyOutcome::Cancelled,
        PreparedFamily::Failed(error) => ImportFamilyOutcome::Failed(error),
    }
}

fn reject_source_changed<T, U>(family: PreparedFamily<T>, error: &str) -> ImportFamilyOutcome<U> {
    match family {
        PreparedFamily::NotRequested => ImportFamilyOutcome::NotRequested,
        PreparedFamily::Ready(_) => ImportFamilyOutcome::SourceChanged(error.to_string()),
        PreparedFamily::Cancelled => ImportFamilyOutcome::Cancelled,
        PreparedFamily::Failed(error) => ImportFamilyOutcome::Failed(error),
    }
}

fn validate_and_reconstruct_key(folder: &Path, relative_key: &str) -> Result<String, String> {
    if relative_key.is_empty() || relative_key.contains('\0') {
        return Err("empty or NUL-containing sidecar key".to_string());
    }
    if let Some((container, tail)) = relative_key.split_once("::") {
        validate_single_filename(container)?;
        if tail.is_empty() || tail.starts_with('/') || tail.starts_with('\\') {
            return Err(format!("invalid virtual sidecar key: {relative_key}"));
        }
        if tail
            .split(|character| character == '/' || character == '\\')
            .any(|segment| segment.is_empty() || segment == "." || segment == "..")
            || Path::new(tail)
                .components()
                .any(|component| !matches!(component, Component::Normal(_)))
        {
            return Err(format!("invalid virtual sidecar key: {relative_key}"));
        }
        return Ok(crate::adjustment_db::zip_entry_key(
            &folder.join(container),
            tail,
        ));
    }
    validate_single_filename(relative_key)?;
    Ok(crate::sidecar::reconstruct_image_key(folder, relative_key))
}

fn validate_single_filename(value: &str) -> Result<(), String> {
    let mut components = Path::new(value).components();
    let valid = matches!(components.next(), Some(Component::Normal(_)))
        && components.next().is_none()
        && Path::new(value)
            .file_name()
            .is_some_and(|name| name == std::ffi::OsStr::new(value));
    if valid {
        Ok(())
    } else {
        Err(format!("sidecar key escapes its owning folder: {value}"))
    }
}

fn prepare_edit_row(
    key: String,
    entry: &crate::sidecar::SidecarEntry,
) -> Result<PreparedEditRow, String> {
    let adjust_json = entry
        .adjust
        .as_ref()
        .map(serde_json::to_string)
        .transpose()
        .map_err(|error| format!("adjustment serialization failed: {error}"))?;
    let mask = entry
        .mask
        .as_ref()
        .map(prepare_mask)
        .transpose()
        .map_err(|error| format!("mask is invalid: {error}"))?;
    let conceal = entry
        .conceal
        .as_ref()
        .map(prepare_mask)
        .transpose()
        .map_err(|error| format!("conceal mask is invalid: {error}"))?;
    let local_adjust_json = entry
        .local_adjust_layers
        .as_ref()
        .filter(|layers| !layers.is_empty())
        .map(serde_json::to_string)
        .transpose()
        .map_err(|error| format!("local adjustment serialization failed: {error}"))?;
    let crop = entry.export_crop.map(validate_crop).transpose()?;
    let comic_json = entry
        .comic
        .as_ref()
        .filter(|objects| !objects.is_empty())
        .map(serde_json::to_string)
        .transpose()
        .map_err(|error| format!("comic serialization failed: {error}"))?;
    Ok(PreparedEditRow {
        key,
        adjust_json,
        mask,
        conceal,
        local_adjust_json,
        crop,
        comic_json,
    })
}

fn prepare_mask(mask: &SidecarMask) -> Result<PreparedMask, String> {
    let width = usize::try_from(mask.w).map_err(|_| "width does not fit usize")?;
    let height = usize::try_from(mask.h).map_err(|_| "height does not fit usize")?;
    let pixels = width
        .checked_mul(height)
        .ok_or("mask dimensions overflow")?;
    if pixels == 0 || pixels > MAX_MASK_PIXELS {
        return Err(format!(
            "mask pixel count {pixels} is outside the supported range"
        ));
    }
    let compressed = mask.decode().ok_or("base64 decode failed")?;
    validate_compressed_mask(&compressed, pixels)?;
    let shapes_json = if mask.vectors.is_empty() {
        None
    } else {
        Some(
            serde_json::to_string(&mask.vectors)
                .map_err(|error| format!("shape serialization failed: {error}"))?,
        )
    };
    Ok(PreparedMask {
        compressed,
        width: i64::from(mask.w),
        height: i64::from(mask.h),
        shapes_json,
    })
}

fn validate_compressed_mask(compressed: &[u8], pixels: usize) -> Result<(), String> {
    let expected = pixels.div_ceil(8);
    let limit = u64::try_from(expected)
        .map_err(|_| "mask byte count does not fit u64")?
        .saturating_add(1);
    let decoder = flate2::read::DeflateDecoder::new(compressed);
    let mut decoded = Vec::with_capacity(expected.min(1024 * 1024));
    decoder
        .take(limit)
        .read_to_end(&mut decoded)
        .map_err(|error| format!("deflate decode failed: {error}"))?;
    if decoded.len() != expected {
        return Err(format!(
            "decoded mask has {} bytes; expected {expected}",
            decoded.len()
        ));
    }
    Ok(())
}

fn validate_crop(
    crop: crate::export_crop::CropSettings,
) -> Result<crate::export_crop::CropSettings, String> {
    let rect = crop.rect;
    if ![rect.min_x, rect.min_y, rect.max_x, rect.max_y]
        .into_iter()
        .all(f32::is_finite)
        || rect.min_x < 0.0
        || rect.min_y < 0.0
        || rect.max_x <= rect.min_x
        || rect.max_y <= rect.min_y
    {
        return Err("crop rectangle is outside the supported domain".to_string());
    }
    if crop.source_size.is_some() && crop.valid_source_size().is_none() {
        return Err("crop source size is invalid".to_string());
    }
    if let Some([width, height]) = crop.valid_source_size()
        && (rect.max_x > width as f32 + 0.5 || rect.max_y > height as f32 + 0.5)
    {
        return Err("crop rectangle exceeds its source size".to_string());
    }
    Ok(crop)
}

fn commit_edit_rows(
    data_dir: &Path,
    folder_key: &str,
    sync_marker: Option<i64>,
    rows: &[PreparedEditRow],
    cancel: &AtomicBool,
    progress: &mut impl FnMut(usize, &AtomicBool),
) -> Result<ImportFamilyOutcome<EditImportReport>, String> {
    if cancel.load(Ordering::Relaxed) {
        return Ok(ImportFamilyOutcome::Cancelled);
    }
    let stores = [
        ("main", "adjustment.db", "page_params"),
        ("mask_edit", "mask.db", "masks"),
        ("conceal_edit", "conceal.db", "conceal_entries"),
        ("local_edit", "local_adjust.db", "local_adjust_pages"),
        ("crop_edit", "export_crop.db", "export_crop_pages"),
        ("comic_edit", "comic.db", "comic_entries"),
    ];
    for (_, filename, _) in stores {
        let path = data_dir.join(filename);
        if !path.is_file() {
            return Err(format!(
                "required edit store is missing: {}",
                path.display()
            ));
        }
    }

    let connection = Connection::open(data_dir.join("adjustment.db"))
        .map_err(|error| format!("cannot open adjustment.db for sidecar import: {error}"))?;
    connection
        .busy_timeout(Duration::from_secs(2))
        .map_err(|error| format!("cannot configure edit import busy timeout: {error}"))?;
    for (schema, filename, _) in stores.into_iter().skip(1) {
        connection
            .execute(
                &format!("ATTACH DATABASE ?1 AS {schema}"),
                [data_dir.join(filename).to_string_lossy().as_ref()],
            )
            .map_err(|error| format!("cannot attach {filename}: {error}"))?;
    }
    for (schema, _, table) in stores {
        let exists = connection
            .query_row(
                &format!(
                    "SELECT EXISTS(SELECT 1 FROM {schema}.sqlite_schema \
                     WHERE type = 'table' AND name = ?1)"
                ),
                [table],
                |row| row.get::<_, bool>(0),
            )
            .map_err(|error| format!("cannot inspect {schema}.{table}: {error}"))?;
        if !exists {
            return Err(format!("required edit schema is missing: {schema}.{table}"));
        }
        let journal_mode: String = connection
            .query_row(&format!("PRAGMA {schema}.journal_mode"), [], |row| {
                row.get(0)
            })
            .map_err(|error| format!("cannot inspect {schema} journal mode: {error}"))?;
        if ["wal", "memory", "off"]
            .iter()
            .any(|unsafe_mode| journal_mode.eq_ignore_ascii_case(unsafe_mode))
        {
            return Err(format!(
                "edit store {schema} uses unsafe journal mode {journal_mode}"
            ));
        }
    }
    let marker_table_exists = connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM main.sqlite_schema \
             WHERE type = 'table' AND name = 'sidecar_sync')",
            [],
            |row| row.get::<_, bool>(0),
        )
        .map_err(|error| format!("cannot inspect main.sidecar_sync: {error}"))?;
    if !marker_table_exists {
        return Err("required edit schema is missing: main.sidecar_sync".to_string());
    }
    if let Some(marker) = sync_marker {
        let current = connection
            .query_row(
                "SELECT sidecar_mtime FROM main.sidecar_sync WHERE folder_key = ?1",
                [folder_key],
                |row| row.get::<_, i64>(0),
            )
            .optional()
            .map_err(|error| format!("cannot read edit sidecar marker: {error}"))?;
        if current == Some(marker) {
            return Ok(ImportFamilyOutcome::AlreadySynchronized);
        }
    }

    let transaction_started = Instant::now();
    connection
        .execute_batch("BEGIN IMMEDIATE")
        .map_err(|error| format!("cannot begin atomic edit sidecar import: {error}"))?;
    if let Some(marker) = sync_marker {
        let current = connection
            .query_row(
                "SELECT sidecar_mtime FROM main.sidecar_sync WHERE folder_key = ?1",
                [folder_key],
                |row| row.get::<_, i64>(0),
            )
            .optional()
            .map_err(|error| format!("cannot recheck edit sidecar marker: {error}"))?;
        if current == Some(marker) {
            let _ = connection.execute_batch("ROLLBACK");
            return Ok(ImportFamilyOutcome::AlreadySynchronized);
        }
    }
    let result = apply_edit_rows_inside_transaction(
        &connection,
        folder_key,
        sync_marker,
        rows,
        cancel,
        progress,
    );
    match result {
        Ok(Some(mut report)) => {
            connection
                .execute_batch("COMMIT")
                .map_err(|error| format!("cannot commit edit sidecar import: {error}"))?;
            report.transaction_committed = true;
            report.transaction_elapsed = transaction_started.elapsed();
            Ok(ImportFamilyOutcome::Applied(report))
        }
        Ok(None) => {
            let _ = connection.execute_batch("ROLLBACK");
            Ok(ImportFamilyOutcome::Cancelled)
        }
        Err(error) => {
            let _ = connection.execute_batch("ROLLBACK");
            Err(error)
        }
    }
}

fn apply_edit_rows_inside_transaction(
    connection: &Connection,
    folder_key: &str,
    sync_marker: Option<i64>,
    rows: &[PreparedEditRow],
    cancel: &AtomicBool,
    progress: &mut impl FnMut(usize, &AtomicBool),
) -> Result<Option<EditImportReport>, String> {
    let mut report = EditImportReport::default();
    let mut processed_fields = 0usize;
    macro_rules! run_field {
        ($body:expr) => {{
            progress(processed_fields, cancel);
            if cancel.load(Ordering::Relaxed) {
                return Ok(None);
            }
            let changed = $body.map_err(|error| format!("edit sidecar insert failed: {error}"))?;
            processed_fields += 1;
            changed
        }};
    }

    for row in rows {
        if let Some(json) = &row.adjust_json {
            let inserted = run_field!(connection.execute(
                "INSERT INTO main.page_params (page_path, params_json) VALUES (?1, ?2)
                 ON CONFLICT(page_path) DO NOTHING",
                params![row.key, json],
            ));
            if inserted > 0 {
                report.stats.imported_adjust += 1;
                report.delta.adjusted.push(row.key.clone());
            } else {
                report.stats.skipped_adjust += 1;
            }
        }
        if let Some(mask) = &row.mask {
            let inserted = run_field!(connection.execute(
                "INSERT INTO mask_edit.masks (path, mask_data, width, height, vectors)
                 VALUES (?1, ?2, ?3, ?4, ?5)
                 ON CONFLICT(path) DO NOTHING",
                params![
                    row.key,
                    mask.compressed,
                    mask.width,
                    mask.height,
                    mask.shapes_json
                ],
            ));
            if inserted > 0 {
                report.stats.imported_mask += 1;
                report.delta.masked.push(row.key.clone());
            } else {
                report.stats.skipped_mask += 1;
            }
        }
        if let Some(mask) = &row.conceal {
            let inserted = run_field!(connection.execute(
                "INSERT INTO conceal_edit.conceal_entries
                    (page_path, bitmap_w, bitmap_h, bitmap_data, shapes)
                 VALUES (?1, ?2, ?3, ?4, ?5)
                 ON CONFLICT(page_path) DO NOTHING",
                params![
                    row.key,
                    mask.width,
                    mask.height,
                    mask.compressed,
                    mask.shapes_json
                ],
            ));
            if inserted > 0 {
                report.stats.imported_conceal += 1;
                report.delta.concealed.push(row.key.clone());
            } else {
                report.stats.skipped_conceal += 1;
            }
        }
        if let Some(json) = &row.local_adjust_json {
            let inserted = run_field!(connection.execute(
                "INSERT INTO local_edit.local_adjust_pages (page_path, layers_json, updated_at)
                 VALUES (?1, ?2, unixepoch())
                 ON CONFLICT(page_path) DO NOTHING",
                params![row.key, json],
            ));
            if inserted > 0 {
                report.stats.imported_local_adjust += 1;
                report.delta.local_adjusted.push(row.key.clone());
            } else {
                report.stats.skipped_local_adjust += 1;
            }
        }
        if let Some(crop) = row.crop {
            let inserted = run_field!(connection.execute(
                "INSERT INTO crop_edit.export_crop_pages
                    (page_path, min_x, min_y, max_x, max_y, aspect_mode,
                     source_width, source_height, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, unixepoch())
                 ON CONFLICT(page_path) DO NOTHING",
                params![
                    row.key,
                    crop.rect.min_x,
                    crop.rect.min_y,
                    crop.rect.max_x,
                    crop.rect.max_y,
                    crop.aspect_mode.stable_key(),
                    crop.valid_source_size()
                        .and_then(|size| i64::try_from(size[0]).ok()),
                    crop.valid_source_size()
                        .and_then(|size| i64::try_from(size[1]).ok()),
                ],
            ));
            if inserted > 0 {
                report.stats.imported_export_crop += 1;
                report.delta.cropped.push(row.key.clone());
            } else {
                report.stats.skipped_export_crop += 1;
            }
        }
        if let Some(json) = &row.comic_json {
            let inserted = run_field!(connection.execute(
                "INSERT INTO comic_edit.comic_entries (page_path, doc_version, doc_json)
                 VALUES (?1, ?2, ?3)
                 ON CONFLICT(page_path) DO NOTHING",
                params![row.key, crate::comic_db::DOC_VERSION as i64, json],
            ));
            if inserted > 0 {
                report.stats.imported_comic += 1;
                report.delta.comic.push(row.key.clone());
            } else {
                report.stats.skipped_comic += 1;
            }
        }
    }
    progress(processed_fields, cancel);
    if cancel.load(Ordering::Relaxed) {
        return Ok(None);
    }
    if let Some(marker) = sync_marker {
        connection
            .execute(
                "INSERT INTO main.sidecar_sync (folder_key, sidecar_mtime) VALUES (?1, ?2)
                 ON CONFLICT(folder_key) DO UPDATE SET sidecar_mtime = excluded.sidecar_mtime",
                params![folder_key, marker],
            )
            .map_err(|error| format!("cannot record edit sidecar marker: {error}"))?;
        report.sync_marker_recorded = true;
    }
    progress(processed_fields.saturating_add(1), cancel);
    if cancel.load(Ordering::Relaxed) {
        return Ok(None);
    }
    Ok(Some(report))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn init_edit_stores(data_dir: &Path) {
        drop(crate::adjustment_db::AdjustmentDb::open_at(&data_dir.join("adjustment.db")).unwrap());
        drop(crate::mask_db::MaskDb::open_at(&data_dir.join("mask.db")).unwrap());
        drop(crate::conceal_db::ConcealDb::open_at(&data_dir.join("conceal.db")).unwrap());
        drop(
            crate::local_adjust_db::LocalAdjustDb::open_at(&data_dir.join("local_adjust.db"))
                .unwrap(),
        );
        drop(crate::export_crop::CropDb::open_at(&data_dir.join("export_crop.db")).unwrap());
        drop(crate::comic_db::ComicDb::open_at(&data_dir.join("comic.db")).unwrap());
    }

    fn init_all_stores(data_dir: &Path) {
        init_edit_stores(data_dir);
        drop(crate::tags_db::TagsDb::open_at(&data_dir.join("tags.db")).unwrap());
    }

    fn flush_and_load(sidecar: &mut SidecarFile) -> LoadedSidecarImport {
        assert!(sidecar.flush_blocking());
        let crate::sidecar::SidecarImportLoad::Loaded(loaded) =
            SidecarFile::load_for_import(sidecar.folder())
        else {
            panic!("test sidecar did not load from disk");
        };
        loaded
    }

    fn params(brightness: f32) -> crate::adjustment::AdjustParams {
        let mut params = crate::adjustment::AdjustParams::default();
        params.brightness = brightness;
        params
    }

    fn current_result(completion: SidecarImportCompletion) -> SidecarImportResult {
        match completion {
            SidecarImportCompletion::Current { result, .. } => result,
            SidecarImportCompletion::SourceChanged { error, .. } => {
                panic!("test import source unexpectedly changed: {error}")
            }
            SidecarImportCompletion::Cancelled { .. } => {
                panic!("test import was unexpectedly cancelled")
            }
        }
    }

    fn import_sample_sidecar(media: &Path, data: &Path, cancel: &AtomicBool) {
        let mut sidecar = SidecarFile::new(media.to_path_buf());
        sidecar.set_adjust("a.jpg", params(1.0));
        sidecar.set_tags("a.jpg", ["#sidecar"]);
        let loaded = flush_and_load(&mut sidecar);
        let prepared = prepare(loaded, ImportFamilies::ALL, cancel).unwrap();
        let result = current_result(commit(data, prepared, cancel));
        assert!(matches!(result.edits, ImportFamilyOutcome::Applied(_)));
        assert!(matches!(result.tags, ImportFamilyOutcome::Applied(_)));
    }

    #[test]
    fn checked_keys_reject_folder_escape() {
        let folder = Path::new("C:/pictures");
        assert!(validate_and_reconstruct_key(folder, "../outside.jpg").is_err());
        assert!(validate_and_reconstruct_key(folder, "C:/outside.jpg").is_err());
        assert!(validate_and_reconstruct_key(folder, "../book.zip::a.jpg").is_err());
        assert!(validate_and_reconstruct_key(folder, "book.zip::/absolute.jpg").is_err());
        assert!(validate_and_reconstruct_key(folder, "book.zip::../outside.jpg").is_err());
        assert!(validate_and_reconstruct_key(folder, "book.zip::dir/./a.jpg").is_err());
        assert!(validate_and_reconstruct_key(folder, "photo.jpg").is_ok());
        assert!(validate_and_reconstruct_key(folder, "book.zip::dir/a.jpg").is_ok());
    }

    #[test]
    fn mid_transaction_cancel_rolls_back_inserted_rows() {
        let media = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        init_edit_stores(data.path());
        let mut sidecar = SidecarFile::new(media.path().to_path_buf());
        sidecar.set_adjust("a.jpg", params(1.0));
        sidecar.set_adjust("b.jpg", params(2.0));
        let cancel = AtomicBool::new(false);
        let loaded = flush_and_load(&mut sidecar);
        let prepared = prepare(
            loaded,
            ImportFamilies {
                edits: true,
                tags: false,
            },
            &cancel,
        )
        .unwrap();

        let completion =
            commit_with_progress(data.path(), prepared, &cancel, |processed, cancel| {
                if processed == 1 {
                    cancel.store(true, Ordering::Relaxed);
                }
            });
        let SidecarImportCompletion::Cancelled { result } = completion else {
            panic!("mid-transaction cancellation must be a terminal cancellation");
        };
        assert!(matches!(result.edits, ImportFamilyOutcome::Cancelled));

        let db = crate::adjustment_db::AdjustmentDb::open_at(&data.path().join("adjustment.db"))
            .unwrap();
        assert!(
            db.get_page_params(&crate::sidecar::reconstruct_image_key(
                media.path(),
                "a.jpg"
            ))
            .is_none()
        );
        assert!(
            db.get_page_params(&crate::sidecar::reconstruct_image_key(
                media.path(),
                "b.jpg"
            ))
            .is_none()
        );
    }

    #[test]
    fn marker_write_fault_rolls_back_the_whole_edit_family() {
        let media = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        init_edit_stores(data.path());
        let mut sidecar = SidecarFile::new(media.path().to_path_buf());
        sidecar.set_adjust("a.jpg", params(1.0));
        let cancel = AtomicBool::new(false);
        let loaded = flush_and_load(&mut sidecar);
        let prepared = prepare(
            loaded,
            ImportFamilies {
                edits: true,
                tags: false,
            },
            &cancel,
        )
        .unwrap();
        let connection = Connection::open(data.path().join("adjustment.db")).unwrap();
        connection
            .execute_batch(
                "CREATE TRIGGER fail_sidecar_marker
                 BEFORE INSERT ON sidecar_sync
                 BEGIN SELECT RAISE(ABORT, 'marker fault'); END;",
            )
            .unwrap();
        drop(connection);

        let completion = commit(data.path(), prepared, &cancel);
        let result = current_result(completion);
        assert!(matches!(result.edits, ImportFamilyOutcome::Failed(_)));
        let db = crate::adjustment_db::AdjustmentDb::open_at(&data.path().join("adjustment.db"))
            .unwrap();
        let key = crate::sidecar::reconstruct_image_key(media.path(), "a.jpg");
        assert!(db.get_page_params(&key).is_none());
        assert_eq!(
            db.sidecar_sync_get(&crate::adjustment_db::normalize_path(media.path())),
            None
        );
        drop(db);

        let connection = Connection::open(data.path().join("adjustment.db")).unwrap();
        connection
            .execute_batch("DROP TRIGGER fail_sidecar_marker")
            .unwrap();
        drop(connection);
        let retry_loaded = match SidecarFile::load_for_import(media.path()) {
            crate::sidecar::SidecarImportLoad::Loaded(loaded) => loaded,
            _ => panic!("retry sidecar did not load"),
        };
        let retry = prepare(
            retry_loaded,
            ImportFamilies {
                edits: true,
                tags: false,
            },
            &cancel,
        )
        .unwrap();
        let retried = current_result(commit(data.path(), retry, &cancel));
        let ImportFamilyOutcome::Applied(report) = retried.edits else {
            panic!("retry after marker fault did not apply");
        };
        assert_eq!(report.stats.imported_adjust, 1);
        assert!(report.transaction_committed);
        assert!(report.sync_marker_recorded);
    }

    #[test]
    fn late_attached_store_fault_rolls_back_every_edit_store_and_marker() {
        use comic_core::{AnnotationObject, TextBlock};

        let media = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        init_edit_stores(data.path());
        let mut sidecar = SidecarFile::new(media.path().to_path_buf());
        sidecar.set_adjust("a.jpg", params(1.0));
        let raw = crate::mask_db::compress_mask(&[true; 64]);
        sidecar.set_mask("a.jpg", SidecarMask::from_raw(&raw, &[], 8, 8));
        sidecar.set_comic(
            "a.jpg",
            vec![AnnotationObject::new_text(
                1,
                (1.0, 2.0),
                TextBlock {
                    text: "late fault".to_string(),
                    ..TextBlock::default()
                },
            )],
        );
        let cancel = AtomicBool::new(false);
        let loaded = flush_and_load(&mut sidecar);
        let prepared = prepare(
            loaded,
            ImportFamilies {
                edits: true,
                tags: false,
            },
            &cancel,
        )
        .unwrap();

        let comic_connection = Connection::open(data.path().join("comic.db")).unwrap();
        comic_connection
            .execute_batch(
                "CREATE TRIGGER fail_late_comic_insert
                 BEFORE INSERT ON comic_entries
                 BEGIN SELECT RAISE(ABORT, 'late attached fault'); END;",
            )
            .unwrap();
        drop(comic_connection);

        let result = current_result(commit(data.path(), prepared, &cancel));
        assert!(matches!(result.edits, ImportFamilyOutcome::Failed(_)));
        let key = crate::sidecar::reconstruct_image_key(media.path(), "a.jpg");
        let adjustment =
            crate::adjustment_db::AdjustmentDb::open_at(&data.path().join("adjustment.db"))
                .unwrap();
        assert!(adjustment.get_page_params(&key).is_none());
        assert_eq!(
            adjustment.sidecar_sync_get(&crate::adjustment_db::normalize_path(media.path())),
            None
        );
        let mask = crate::mask_db::MaskDb::open_at(&data.path().join("mask.db")).unwrap();
        assert!(mask.get(&key, 8, 8).is_none());
        let comic = Connection::open(data.path().join("comic.db")).unwrap();
        assert_eq!(
            comic
                .query_row(
                    "SELECT COUNT(*) FROM comic_entries WHERE page_path = ?1",
                    [&key],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            0
        );
    }

    #[test]
    fn unsafe_attached_journal_mode_is_rejected_before_transaction() {
        let media = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        init_edit_stores(data.path());
        let mut sidecar = SidecarFile::new(media.path().to_path_buf());
        sidecar.set_adjust("a.jpg", params(1.0));
        let cancel = AtomicBool::new(false);
        let loaded = flush_and_load(&mut sidecar);
        let prepared = prepare(
            loaded,
            ImportFamilies {
                edits: true,
                tags: false,
            },
            &cancel,
        )
        .unwrap();

        let mask = Connection::open(data.path().join("mask.db")).unwrap();
        let mode: String = mask
            .query_row("PRAGMA journal_mode = WAL", [], |row| row.get(0))
            .unwrap();
        assert_eq!(mode.to_ascii_lowercase(), "wal");
        drop(mask);

        let SidecarImportCompletion::Current {
            mut sidecar,
            result,
        } = commit(data.path(), prepared, &cancel)
        else {
            panic!("a DB policy failure must retain the validated source owner");
        };
        let ImportFamilyOutcome::Failed(error) = result.edits else {
            panic!("unsafe attached journal mode must reject the edit family");
        };
        assert!(error.contains("unsafe journal mode"));
        assert!(result.source_validation_error.is_none());
        let adjustment =
            crate::adjustment_db::AdjustmentDb::open_at(&data.path().join("adjustment.db"))
                .unwrap();
        let key = crate::sidecar::reconstruct_image_key(media.path(), "a.jpg");
        assert!(adjustment.get_page_params(&key).is_none());
        assert_eq!(
            adjustment.sidecar_sync_get(&crate::adjustment_db::normalize_path(media.path())),
            None
        );
        sidecar.set_adjust("after-db-error.jpg", params(2.0));
        assert!(
            sidecar.flush_blocking(),
            "a DB-only failure must not disable a validated sidecar source"
        );
    }

    #[test]
    fn escaping_edit_key_rejects_the_whole_prepared_family() {
        let media = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        init_edit_stores(data.path());
        let mut sidecar = SidecarFile::new(media.path().to_path_buf());
        sidecar.set_adjust("valid.jpg", params(1.0));
        sidecar.set_adjust("../escape.jpg", params(2.0));
        let cancel = AtomicBool::new(false);
        let loaded = flush_and_load(&mut sidecar);
        let source_path = media.path().join(crate::sidecar::SIDECAR_FILENAME);
        let original_source = std::fs::read(&source_path).unwrap();
        let SidecarImportProbe::Current {
            sidecar: mut probe_sidecar,
            result: probe_result,
        } = probe(
            media.path(),
            data.path(),
            ImportFamilies {
                edits: true,
                tags: false,
            },
            &cancel,
        )
        else {
            panic!("stable invalid source must have a non-writing probe owner");
        };
        assert!(matches!(
            probe_result.edits,
            SidecarProbeFamilyOutcome::Failed(_)
        ));
        assert!(
            probe_result
                .source_validation_error
                .as_deref()
                .is_some_and(|error| error.contains("writes are disabled"))
        );
        probe_sidecar.set_adjust("probe-later.jpg", params(4.0));
        assert!(!probe_sidecar.flush_blocking());
        assert_eq!(std::fs::read(&source_path).unwrap(), original_source);
        let prepared = prepare(
            loaded,
            ImportFamilies {
                edits: true,
                tags: false,
            },
            &cancel,
        )
        .unwrap();

        let SidecarImportCompletion::Current {
            mut sidecar,
            result,
        } = commit(data.path(), prepared, &cancel)
        else {
            panic!("stable invalid source must remain a current, non-install-overwriting snapshot");
        };
        let ImportFamilyOutcome::Failed(_) = result.edits else {
            panic!("escaping key must fail the edit family");
        };
        assert!(
            result
                .source_validation_error
                .as_deref()
                .is_some_and(|error| error.contains("writes are disabled"))
        );
        let adjustment =
            crate::adjustment_db::AdjustmentDb::open_at(&data.path().join("adjustment.db"))
                .unwrap();
        let valid_key = crate::sidecar::reconstruct_image_key(media.path(), "valid.jpg");
        assert!(adjustment.get_page_params(&valid_key).is_none());
        assert_eq!(
            adjustment.sidecar_sync_get(&crate::adjustment_db::normalize_path(media.path())),
            None
        );

        sidecar.set_adjust("later.jpg", params(3.0));
        assert!(
            !sidecar.flush_blocking(),
            "a validation-failed source must not report a later flush as durable"
        );
        assert_eq!(
            std::fs::read(source_path).unwrap(),
            original_source,
            "a later edit must not overwrite the invalid source"
        );
    }

    #[test]
    fn invalid_unrequested_tag_content_disables_whole_file_writes() {
        let media = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        init_edit_stores(data.path());
        let mut source = SidecarFile::new(media.path().to_path_buf());
        source.set_adjust("valid.jpg", params(1.0));
        source.set_tags("../escape.jpg", ["#tag"]);
        let cancel = AtomicBool::new(false);
        let loaded = flush_and_load(&mut source);
        let source_path = media.path().join(crate::sidecar::SIDECAR_FILENAME);
        let original_source = std::fs::read(&source_path).unwrap();
        let prepared = prepare(
            loaded,
            ImportFamilies {
                edits: true,
                tags: false,
            },
            &cancel,
        )
        .unwrap();

        let SidecarImportCompletion::Current {
            mut sidecar,
            result,
        } = commit(data.path(), prepared, &cancel)
        else {
            panic!("stable source must retain a typed current owner");
        };
        assert!(matches!(result.edits, ImportFamilyOutcome::Applied(_)));
        assert!(matches!(result.tags, ImportFamilyOutcome::NotRequested));
        assert!(
            result
                .source_validation_error
                .as_deref()
                .is_some_and(|error| error.contains("writes are disabled"))
        );
        sidecar.set_adjust("valid.jpg", params(9.0));
        assert!(!sidecar.flush_blocking());
        assert_eq!(std::fs::read(source_path).unwrap(), original_source);
    }

    #[test]
    fn row_created_after_prepare_remains_authoritative() {
        let media = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        init_edit_stores(data.path());
        let mut sidecar = SidecarFile::new(media.path().to_path_buf());
        sidecar.set_adjust("a.jpg", params(1.0));
        let cancel = AtomicBool::new(false);
        let loaded = flush_and_load(&mut sidecar);
        let prepared = prepare(
            loaded,
            ImportFamilies {
                edits: true,
                tags: false,
            },
            &cancel,
        )
        .unwrap();

        let db = crate::adjustment_db::AdjustmentDb::open_at(&data.path().join("adjustment.db"))
            .unwrap();
        let key = crate::sidecar::reconstruct_image_key(media.path(), "a.jpg");
        db.set_page_params(&key, &params(99.0)).unwrap();
        drop(db);

        let completion = current_result(commit(data.path(), prepared, &cancel));
        let ImportFamilyOutcome::Applied(report) = completion.edits else {
            panic!("edit import did not complete");
        };
        assert_eq!(report.stats.imported_adjust, 0);
        assert_eq!(report.stats.skipped_adjust, 1);
        let db = crate::adjustment_db::AdjustmentDb::open_at(&data.path().join("adjustment.db"))
            .unwrap();
        assert_eq!(db.get_page_params(&key).unwrap().brightness, 99.0);
    }

    #[test]
    fn tag_marker_fault_rolls_back_tags_and_decision_state() {
        let media = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        drop(crate::tags_db::TagsDb::open_at(&data.path().join("tags.db")).unwrap());
        let mut sidecar = SidecarFile::new(media.path().to_path_buf());
        sidecar.set_tags("a.jpg", ["#sidecar"]);
        let cancel = AtomicBool::new(false);
        let loaded = flush_and_load(&mut sidecar);
        let prepared = prepare(
            loaded,
            ImportFamilies {
                edits: false,
                tags: true,
            },
            &cancel,
        )
        .unwrap();
        let connection = Connection::open(data.path().join("tags.db")).unwrap();
        connection
            .execute_batch(
                "CREATE TRIGGER fail_tag_sidecar_marker
                 BEFORE INSERT ON tag_sidecar_sync
                 BEGIN SELECT RAISE(ABORT, 'tag marker fault'); END;",
            )
            .unwrap();
        drop(connection);

        let completion = commit(data.path(), prepared, &cancel);
        let result = current_result(completion);
        assert!(matches!(result.tags, ImportFamilyOutcome::Failed(_)));
        let db = crate::tags_db::TagsDb::open_at(&data.path().join("tags.db")).unwrap();
        let key = crate::sidecar::reconstruct_image_key(media.path(), "a.jpg");
        assert!(db.display_tags_for_item(&key).is_empty());
        assert!(!db.has_item_state(&key));
        assert_eq!(
            db.sidecar_sync_get(&crate::adjustment_db::normalize_path(media.path())),
            None
        );
    }

    #[test]
    fn legacy_tag_row_created_after_prepare_remains_authoritative() {
        let media = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        drop(crate::tags_db::TagsDb::open_at(&data.path().join("tags.db")).unwrap());
        let mut sidecar = SidecarFile::new(media.path().to_path_buf());
        sidecar.set_tags("a.jpg", ["#sidecar"]);
        let cancel = AtomicBool::new(false);
        let loaded = flush_and_load(&mut sidecar);
        let prepared = prepare(
            loaded,
            ImportFamilies {
                edits: false,
                tags: true,
            },
            &cancel,
        )
        .unwrap();

        let key = crate::sidecar::reconstruct_image_key(media.path(), "a.jpg");
        let connection = Connection::open(data.path().join("tags.db")).unwrap();
        connection
            .execute(
                "INSERT INTO item_tags (item_key, tag, tag_key, applied_at)
                 VALUES (?1, 'legacy', 'legacy', 1)",
                [&key],
            )
            .unwrap();
        drop(connection);

        let completion = current_result(commit(data.path(), prepared, &cancel));
        let ImportFamilyOutcome::Applied(report) = completion.tags else {
            panic!("tag import did not complete");
        };
        assert_eq!(report.imported_items, 0);
        assert_eq!(report.skipped_decided_items, 1);
        let db = crate::tags_db::TagsDb::open_at(&data.path().join("tags.db")).unwrap();
        assert_eq!(db.display_tags_for_item(&key), vec!["#legacy"]);
        assert!(!db.has_item_state(&key));
    }

    #[test]
    fn edit_failure_and_tag_success_are_reported_independently() {
        let media = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        init_edit_stores(data.path());
        drop(crate::tags_db::TagsDb::open_at(&data.path().join("tags.db")).unwrap());
        let connection = Connection::open(data.path().join("adjustment.db")).unwrap();
        connection
            .execute_batch(
                "CREATE TRIGGER fail_sidecar_marker
                 BEFORE INSERT ON sidecar_sync
                 BEGIN SELECT RAISE(ABORT, 'marker fault'); END;",
            )
            .unwrap();
        drop(connection);

        let mut sidecar = SidecarFile::new(media.path().to_path_buf());
        sidecar.set_adjust("a.jpg", params(1.0));
        sidecar.set_tags("a.jpg", ["#sidecar"]);
        let cancel = AtomicBool::new(false);
        let loaded = flush_and_load(&mut sidecar);
        let prepared = prepare(loaded, ImportFamilies::ALL, &cancel).unwrap();
        let result = current_result(commit(data.path(), prepared, &cancel));
        assert!(matches!(result.edits, ImportFamilyOutcome::Failed(_)));
        assert!(matches!(result.tags, ImportFamilyOutcome::Applied(_)));

        let adjustment =
            crate::adjustment_db::AdjustmentDb::open_at(&data.path().join("adjustment.db"))
                .unwrap();
        let key = crate::sidecar::reconstruct_image_key(media.path(), "a.jpg");
        assert!(adjustment.get_page_params(&key).is_none());
        let tags = crate::tags_db::TagsDb::open_at(&data.path().join("tags.db")).unwrap();
        assert_eq!(tags.display_tags_for_item(&key), vec!["#sidecar"]);
    }

    #[test]
    fn source_change_before_commit_rejects_both_families() {
        let media = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        init_edit_stores(data.path());
        drop(crate::tags_db::TagsDb::open_at(&data.path().join("tags.db")).unwrap());

        let mut sidecar = SidecarFile::new(media.path().to_path_buf());
        sidecar.set_adjust("a.jpg", params(1.0));
        sidecar.set_tags("a.jpg", ["#sidecar"]);
        let loaded = flush_and_load(&mut sidecar);
        let cancel = AtomicBool::new(false);
        let prepared = prepare(loaded, ImportFamilies::ALL, &cancel).unwrap();

        std::fs::write(
            media.path().join(crate::sidecar::SIDECAR_FILENAME),
            br#"{"version":1,"items":{}}"#,
        )
        .unwrap();
        let completion = commit(data.path(), prepared, &cancel);
        let SidecarImportCompletion::SourceChanged { result, .. } = completion else {
            panic!("changed disk bytes must be a terminal source change");
        };
        assert!(matches!(
            result.edits,
            ImportFamilyOutcome::SourceChanged(_)
        ));
        assert!(matches!(result.tags, ImportFamilyOutcome::SourceChanged(_)));

        let key = crate::sidecar::reconstruct_image_key(media.path(), "a.jpg");
        let adjustment =
            crate::adjustment_db::AdjustmentDb::open_at(&data.path().join("adjustment.db"))
                .unwrap();
        assert!(adjustment.get_page_params(&key).is_none());
        assert_eq!(
            adjustment.sidecar_sync_get(&crate::adjustment_db::normalize_path(media.path())),
            None
        );
        let tags = crate::tags_db::TagsDb::open_at(&data.path().join("tags.db")).unwrap();
        assert!(tags.display_tags_for_item(&key).is_empty());
        assert_eq!(
            tags.sidecar_sync_get(&crate::adjustment_db::normalize_path(media.path())),
            None
        );
    }

    #[test]
    fn pre_cancel_skips_source_revalidation_and_database_work() {
        let media = tempfile::tempdir().unwrap();
        let missing_data_dir = media.path().join("databases-must-not-be-opened");
        let mut sidecar = SidecarFile::new(media.path().to_path_buf());
        sidecar.set_adjust("a.jpg", params(1.0));
        sidecar.set_tags("a.jpg", ["#tag"]);
        let cancel = AtomicBool::new(false);
        let loaded = flush_and_load(&mut sidecar);
        let prepared = prepare(loaded, ImportFamilies::ALL, &cancel).unwrap();

        std::fs::remove_file(media.path().join(crate::sidecar::SIDECAR_FILENAME)).unwrap();
        cancel.store(true, Ordering::Relaxed);
        let completion = commit(&missing_data_dir, prepared, &cancel);
        let SidecarImportCompletion::Cancelled { result } = completion else {
            panic!("pre-cancel must win over a missing source and missing databases");
        };
        assert!(matches!(result.edits, ImportFamilyOutcome::Cancelled));
        assert!(matches!(result.tags, ImportFamilyOutcome::Cancelled));
        assert!(!missing_data_dir.exists());
    }

    #[test]
    fn read_only_probe_reports_import_without_mutating_stores() {
        let media = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        init_all_stores(data.path());
        let mut sidecar = SidecarFile::new(media.path().to_path_buf());
        sidecar.set_adjust("a.jpg", params(1.0));
        sidecar.set_tags("a.jpg", ["#sidecar"]);
        assert!(sidecar.flush_blocking());

        let cancel = AtomicBool::new(false);
        let SidecarImportProbe::ImportRequired { result } =
            probe(media.path(), data.path(), ImportFamilies::ALL, &cancel)
        else {
            panic!("fresh sidecar must require an import");
        };
        assert_eq!(result.edits, SidecarProbeFamilyOutcome::ImportRequired);
        assert_eq!(result.tags, SidecarProbeFamilyOutcome::ImportRequired);

        let folder_key = crate::adjustment_db::normalize_path(media.path());
        let key = crate::sidecar::reconstruct_image_key(media.path(), "a.jpg");
        let adjustment =
            crate::adjustment_db::AdjustmentDb::open_at(&data.path().join("adjustment.db"))
                .unwrap();
        assert!(adjustment.get_page_params(&key).is_none());
        assert_eq!(adjustment.sidecar_sync_get(&folder_key), None);
        let tags = crate::tags_db::TagsDb::open_at(&data.path().join("tags.db")).unwrap();
        assert!(tags.display_tags_for_item(&key).is_empty());
        assert_eq!(tags.sidecar_sync_get(&folder_key), None);
        assert!(!data.path().join("tags.db.bak1").exists());
    }

    #[test]
    fn probe_distinguishes_synchronized_and_missing_marker_clear() {
        let media = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        init_all_stores(data.path());
        let cancel = AtomicBool::new(false);
        import_sample_sidecar(media.path(), data.path(), &cancel);

        let SidecarImportProbe::Current { sidecar, result } =
            probe(media.path(), data.path(), ImportFamilies::ALL, &cancel)
        else {
            panic!("committed sidecar must probe as synchronized");
        };
        assert!(!sidecar.items().is_empty());
        assert_eq!(result.edits, SidecarProbeFamilyOutcome::AlreadySynchronized);
        assert_eq!(result.tags, SidecarProbeFamilyOutcome::AlreadySynchronized);

        std::fs::remove_file(media.path().join(crate::sidecar::SIDECAR_FILENAME)).unwrap();
        let SidecarImportProbe::MarkerClearRequired { result } =
            probe(media.path(), data.path(), ImportFamilies::ALL, &cancel)
        else {
            panic!("missing sidecar with old markers must require marker clear");
        };
        assert_eq!(result.edits, SidecarProbeFamilyOutcome::MarkerClearRequired);
        assert_eq!(result.tags, SidecarProbeFamilyOutcome::MarkerClearRequired);
    }

    #[test]
    fn missing_marker_clear_commits_each_family_and_preserves_rows() {
        let media = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        init_all_stores(data.path());
        let cancel = AtomicBool::new(false);
        import_sample_sidecar(media.path(), data.path(), &cancel);
        std::fs::remove_file(media.path().join(crate::sidecar::SIDECAR_FILENAME)).unwrap();

        let MissingMarkerClearCompletion::Current { sidecar, result } =
            clear_missing_markers(media.path(), data.path(), ImportFamilies::ALL, &cancel)
        else {
            panic!("stable missing source must clear both marker families");
        };
        assert!(sidecar.items().is_empty());
        let ImportFamilyOutcome::Applied(edit_report) = result.edits else {
            panic!("edit marker was not cleared");
        };
        let ImportFamilyOutcome::Applied(tag_report) = result.tags else {
            panic!("tag marker was not cleared");
        };
        assert!(edit_report.marker_cleared && edit_report.transaction_committed);
        assert!(tag_report.marker_cleared && tag_report.transaction_committed);

        let folder_key = crate::adjustment_db::normalize_path(media.path());
        let key = crate::sidecar::reconstruct_image_key(media.path(), "a.jpg");
        let adjustment =
            crate::adjustment_db::AdjustmentDb::open_at(&data.path().join("adjustment.db"))
                .unwrap();
        assert_eq!(adjustment.sidecar_sync_get(&folder_key), None);
        assert!(adjustment.get_page_params(&key).is_some());
        let tags = crate::tags_db::TagsDb::open_at(&data.path().join("tags.db")).unwrap();
        assert_eq!(tags.sidecar_sync_get(&folder_key), None);
        assert_eq!(tags.display_tags_for_item(&key), vec!["#sidecar"]);
    }

    #[test]
    fn marker_clear_fault_keeps_that_marker_and_reports_partial_result() {
        let media = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        init_all_stores(data.path());
        let cancel = AtomicBool::new(false);
        import_sample_sidecar(media.path(), data.path(), &cancel);
        std::fs::remove_file(media.path().join(crate::sidecar::SIDECAR_FILENAME)).unwrap();
        let connection = Connection::open(data.path().join("adjustment.db")).unwrap();
        connection
            .execute_batch(
                "CREATE TRIGGER fail_edit_marker_clear
                 BEFORE DELETE ON sidecar_sync
                 BEGIN SELECT RAISE(ABORT, 'marker clear fault'); END;",
            )
            .unwrap();
        drop(connection);

        let MissingMarkerClearCompletion::Current { result, .. } =
            clear_missing_markers(media.path(), data.path(), ImportFamilies::ALL, &cancel)
        else {
            panic!("a family fault must retain explicit family outcomes");
        };
        assert!(matches!(result.edits, ImportFamilyOutcome::Failed(_)));
        assert!(matches!(result.tags, ImportFamilyOutcome::Applied(_)));

        let folder_key = crate::adjustment_db::normalize_path(media.path());
        let adjustment =
            crate::adjustment_db::AdjustmentDb::open_at(&data.path().join("adjustment.db"))
                .unwrap();
        assert!(adjustment.sidecar_sync_get(&folder_key).is_some());
        let tags = crate::tags_db::TagsDb::open_at(&data.path().join("tags.db")).unwrap();
        assert_eq!(tags.sidecar_sync_get(&folder_key), None);
    }

    #[test]
    fn marker_clear_cancel_rolls_back_before_reporting_success() {
        let media = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        init_all_stores(data.path());
        let cancel = AtomicBool::new(false);
        import_sample_sidecar(media.path(), data.path(), &cancel);
        std::fs::remove_file(media.path().join(crate::sidecar::SIDECAR_FILENAME)).unwrap();

        let completion = clear_missing_markers_with_progress(
            media.path(),
            data.path(),
            ImportFamilies::ALL,
            &cancel,
            &mut |store, cancel| {
                if matches!(store, MarkerStore::Edits) {
                    cancel.store(true, Ordering::Relaxed);
                }
            },
        );
        let MissingMarkerClearCompletion::Cancelled { result } = completion else {
            panic!("cancellation after DELETE must be terminal");
        };
        assert!(matches!(result.edits, ImportFamilyOutcome::Cancelled));
        assert!(matches!(result.tags, ImportFamilyOutcome::Cancelled));

        let folder_key = crate::adjustment_db::normalize_path(media.path());
        let adjustment =
            crate::adjustment_db::AdjustmentDb::open_at(&data.path().join("adjustment.db"))
                .unwrap();
        assert!(adjustment.sidecar_sync_get(&folder_key).is_some());
        let tags = crate::tags_db::TagsDb::open_at(&data.path().join("tags.db")).unwrap();
        assert!(tags.sidecar_sync_get(&folder_key).is_some());
    }

    #[test]
    fn sidecar_appearing_after_edit_clear_preserves_applied_and_stops_tags() {
        let media = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        init_all_stores(data.path());
        let cancel = AtomicBool::new(false);
        import_sample_sidecar(media.path(), data.path(), &cancel);
        let sidecar_path = media.path().join(crate::sidecar::SIDECAR_FILENAME);
        std::fs::remove_file(&sidecar_path).unwrap();

        let completion = clear_missing_markers_with_progress(
            media.path(),
            data.path(),
            ImportFamilies::ALL,
            &cancel,
            &mut |store, _| {
                if matches!(store, MarkerStore::Edits) {
                    std::fs::write(&sidecar_path, br#"{"version":1,"items":{}}"#).unwrap();
                }
            },
        );
        let MissingMarkerClearCompletion::SourceChanged { result, .. } = completion else {
            panic!("a sidecar appearing between family transactions must be source-changed");
        };
        assert!(matches!(result.edits, ImportFamilyOutcome::Applied(_)));
        assert!(matches!(result.tags, ImportFamilyOutcome::SourceChanged(_)));

        let folder_key = crate::adjustment_db::normalize_path(media.path());
        let adjustment =
            crate::adjustment_db::AdjustmentDb::open_at(&data.path().join("adjustment.db"))
                .unwrap();
        assert_eq!(adjustment.sidecar_sync_get(&folder_key), None);
        let tags = crate::tags_db::TagsDb::open_at(&data.path().join("tags.db")).unwrap();
        assert!(tags.sidecar_sync_get(&folder_key).is_some());
    }

    #[test]
    fn sidecar_appearing_after_tag_clear_keeps_both_applied_outcomes_typed() {
        let media = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        init_all_stores(data.path());
        let cancel = AtomicBool::new(false);
        import_sample_sidecar(media.path(), data.path(), &cancel);
        let sidecar_path = media.path().join(crate::sidecar::SIDECAR_FILENAME);
        std::fs::remove_file(&sidecar_path).unwrap();

        let completion = clear_missing_markers_with_progress(
            media.path(),
            data.path(),
            ImportFamilies::ALL,
            &cancel,
            &mut |store, _| {
                if matches!(store, MarkerStore::Tags) {
                    std::fs::write(&sidecar_path, br#"{"version":1,"items":{}}"#).unwrap();
                }
            },
        );
        let MissingMarkerClearCompletion::SourceChanged { result, .. } = completion else {
            panic!("a sidecar appearing after tag DELETE must fail the outer source state");
        };
        assert!(matches!(result.edits, ImportFamilyOutcome::Applied(_)));
        assert!(matches!(result.tags, ImportFamilyOutcome::Applied(_)));

        let folder_key = crate::adjustment_db::normalize_path(media.path());
        let adjustment =
            crate::adjustment_db::AdjustmentDb::open_at(&data.path().join("adjustment.db"))
                .unwrap();
        assert_eq!(adjustment.sidecar_sync_get(&folder_key), None);
        let tags = crate::tags_db::TagsDb::open_at(&data.path().join("tags.db")).unwrap();
        assert_eq!(tags.sidecar_sync_get(&folder_key), None);
    }

    #[test]
    fn missing_marker_clear_rejects_a_new_sidecar_without_touching_markers() {
        let media = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        init_all_stores(data.path());
        let folder_key = crate::adjustment_db::normalize_path(media.path());
        let adjustment =
            crate::adjustment_db::AdjustmentDb::open_at(&data.path().join("adjustment.db"))
                .unwrap();
        adjustment.sidecar_sync_upsert(&folder_key, 11).unwrap();
        drop(adjustment);
        let tags = crate::tags_db::TagsDb::open_at(&data.path().join("tags.db")).unwrap();
        tags.sidecar_sync_upsert(&folder_key, 22).unwrap();
        drop(tags);
        let mut sidecar = SidecarFile::new(media.path().to_path_buf());
        sidecar.set_adjust("new.jpg", params(2.0));
        assert!(sidecar.flush_blocking());

        let cancel = AtomicBool::new(false);
        let MissingMarkerClearCompletion::SourceChanged { result, .. } =
            clear_missing_markers(media.path(), data.path(), ImportFamilies::ALL, &cancel)
        else {
            panic!("a newly present sidecar must reject marker clear");
        };
        assert!(matches!(
            result.edits,
            ImportFamilyOutcome::SourceChanged(_)
        ));
        assert!(matches!(result.tags, ImportFamilyOutcome::SourceChanged(_)));
        let adjustment =
            crate::adjustment_db::AdjustmentDb::open_at(&data.path().join("adjustment.db"))
                .unwrap();
        assert_eq!(adjustment.sidecar_sync_get(&folder_key), Some(11));
        let tags = crate::tags_db::TagsDb::open_at(&data.path().join("tags.db")).unwrap();
        assert_eq!(tags.sidecar_sync_get(&folder_key), Some(22));
    }

    #[test]
    fn probe_and_marker_clear_pre_cancel_do_no_io() {
        let media = tempfile::tempdir().unwrap();
        let data = media.path().join("data-must-stay-missing");
        let cancel = AtomicBool::new(true);

        let SidecarImportProbe::Cancelled { result } =
            probe(media.path(), &data, ImportFamilies::ALL, &cancel)
        else {
            panic!("pre-cancelled probe must stop before source and database I/O");
        };
        assert_eq!(result.edits, SidecarProbeFamilyOutcome::Cancelled);
        assert_eq!(result.tags, SidecarProbeFamilyOutcome::Cancelled);
        let MissingMarkerClearCompletion::Cancelled { result } =
            clear_missing_markers(media.path(), &data, ImportFamilies::ALL, &cancel)
        else {
            panic!("pre-cancelled marker clear must stop before database I/O");
        };
        assert!(matches!(result.edits, ImportFamilyOutcome::Cancelled));
        assert!(matches!(result.tags, ImportFamilyOutcome::Cancelled));
        assert!(!data.exists());
    }

    #[test]
    fn unstable_missing_load_maps_to_non_installable_source_changed() {
        let completion = missing_marker_source_changed(
            Instant::now(),
            ImportFamilies::ALL,
            "changed during read".to_string(),
        );
        let MissingMarkerClearCompletion::SourceChanged { result, .. } = completion else {
            panic!("an unstable missing-source load must not expose a Current sidecar");
        };
        assert!(matches!(
            result.edits,
            ImportFamilyOutcome::SourceChanged(_)
        ));
        assert!(matches!(result.tags, ImportFamilyOutcome::SourceChanged(_)));
    }
}
