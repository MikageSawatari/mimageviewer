//! A3a/A3b: 内容 identity が一致した物理ファイル間の永続 edit state copy。
//!
//! このモジュールは worker から呼べる同期処理だけを持ち、`App` / egui / UI thread の
//! state を要求しない。UI 所有の sidecar / presence / cache は report を A3b が適用する。

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use super::{
    ContentIdentityDb, ContentKind, LedgerEntry, RestoreCandidate, RestoreSourceCandidate,
    metadata_mtime,
};
use crate::rename_key_migration::StoreCopyPathMapping;

#[derive(Clone, Debug)]
pub(crate) struct RestoreSidecarMirror {
    pub(crate) folder: PathBuf,
    pub(crate) rel_key: String,
    pub(crate) entry: crate::sidecar::SidecarEntry,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct RestorePresence {
    pub(crate) adjusted: BTreeSet<String>,
    pub(crate) masks: BTreeSet<String>,
    pub(crate) conceals: BTreeSet<String>,
    pub(crate) local_adjustments: BTreeSet<String>,
    pub(crate) comics: BTreeSet<String>,
    pub(crate) rotations: BTreeSet<String>,
    pub(crate) crops: BTreeSet<String>,
    /// Destination page keys with any restored sidecar-backed edit. The UI uses this union to
    /// evict only thumbnails that may have materialized the pre-restore edit-preview state.
    pub(crate) page_edits: BTreeSet<String>,
}

#[derive(Default)]
pub(crate) struct ContentRestoreReport {
    pub(crate) requested_restores: usize,
    pub(crate) requested_declines: usize,
    pub(crate) rows: usize,
    pub(crate) database_opens: usize,
    pub(crate) errors: Vec<String>,
    pub(crate) sidecar_mirrors: Vec<RestoreSidecarMirror>,
    pub(crate) sidecar_bases: Vec<crate::sidecar::SidecarFile>,
    pub(crate) presence: RestorePresence,
    pub(crate) ledger_entries: Vec<LedgerEntry>,
}

#[derive(Clone, Debug)]
pub(crate) struct SelectedRestore {
    pub(crate) candidate: RestoreCandidate,
    pub(crate) source: RestoreSourceCandidate,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct DeclinedRestore {
    pub(crate) full_hash: String,
    pub(crate) target_key: String,
}

/// Worker-side seam for byte-identical copies created by mIV itself.
/// It reuses only the source ledger full hash and never reads either file.
/// Other in-app copy producers can use the same recorder later.
pub(crate) struct InternalByteCopyDeclineRecorder {
    db_path: PathBuf,
    db: InternalByteCopyDeclineDb,
    report: InternalByteCopyDeclineReport,
}

enum InternalByteCopyDeclineDb {
    Unopened,
    Ready(ContentIdentityDb),
    Unavailable,
}

enum InternalByteCopyDeclineOutcome {
    Recorded,
    AlreadyRecorded,
    SourceNotTracked,
    SourceHashUnavailable,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct InternalByteCopyDeclineReport {
    pub(crate) requested: usize,
    pub(crate) recorded: usize,
    pub(crate) already_recorded: usize,
    pub(crate) source_not_tracked: usize,
    pub(crate) source_hash_unavailable: usize,
    pub(crate) errors: Vec<String>,
}

impl InternalByteCopyDeclineRecorder {
    pub(crate) fn new(data_dir: &Path) -> Self {
        Self {
            db_path: data_dir.join("content_identity.db"),
            db: InternalByteCopyDeclineDb::Unopened,
            report: InternalByteCopyDeclineReport::default(),
        }
    }

    pub(crate) fn record(&mut self, source_path: &Path, target_path: &Path) {
        self.report.requested += 1;
        if matches!(self.db, InternalByteCopyDeclineDb::Unopened) {
            self.db = match ContentIdentityDb::open_at(&self.db_path) {
                Ok(db) => InternalByteCopyDeclineDb::Ready(db),
                Err(error) => {
                    self.push_error(format!(
                        "content_identity internal byte-copy DB open {}: {error}",
                        self.db_path.display()
                    ));
                    InternalByteCopyDeclineDb::Unavailable
                }
            };
        }
        let InternalByteCopyDeclineDb::Ready(db) = &self.db else {
            return;
        };
        let outcome = record_internal_byte_copy_decline(db, source_path, target_path);
        match outcome {
            Ok(InternalByteCopyDeclineOutcome::Recorded) => self.report.recorded += 1,
            Ok(InternalByteCopyDeclineOutcome::AlreadyRecorded) => {
                self.report.already_recorded += 1
            }
            Ok(InternalByteCopyDeclineOutcome::SourceNotTracked) => {
                self.report.source_not_tracked += 1
            }
            Ok(InternalByteCopyDeclineOutcome::SourceHashUnavailable) => {
                self.report.source_hash_unavailable += 1
            }
            Err(error) => self.push_error(error),
        }
    }

    pub(crate) fn finish(self) -> InternalByteCopyDeclineReport {
        self.report
    }

    fn push_error(&mut self, error: String) {
        crate::logger::log(error.clone());
        self.report.errors.push(error);
    }
}

/// A3b worker の batch 入口。全候補の mapping を先に集約し、shared STORES と
/// runtime update の各 DB は候補数にかかわらず 1 回ずつだけ開く。
#[cfg(test)]
fn restore_candidates_at(
    data_dir: &Path,
    selected: &[SelectedRestore],
    declined: &[DeclinedRestore],
    load_sidecar_bases: bool,
) -> ContentRestoreReport {
    restore_candidates_at_with_progress(
        data_dir,
        selected,
        declined,
        load_sidecar_bases,
        |_, _, _| {},
    )
}

pub(crate) fn restore_candidates_at_with_progress(
    data_dir: &Path,
    selected: &[SelectedRestore],
    declined: &[DeclinedRestore],
    load_sidecar_bases: bool,
    mut on_progress: impl FnMut(&'static str, usize, usize),
) -> ContentRestoreReport {
    let ordinary = selected
        .iter()
        .filter(|selection| selection.candidate.target_kind != ContentKind::Epub)
        .collect::<Vec<_>>();
    let mappings = ordinary
        .iter()
        .flat_map(|selection| {
            restore_copy_mappings(data_dir, &selection.candidate, &selection.source)
        })
        .collect::<Vec<_>>();
    let copied = crate::rename_key_migration::copy_stores_at_with_progress(
        data_dir,
        &mappings,
        |processed, total| on_progress("copy", processed, total),
    );
    let mut report = ContentRestoreReport {
        requested_restores: selected.len(),
        requested_declines: declined.len(),
        rows: copied.rows,
        database_opens: copied.database_opens,
        errors: copied.errors,
        ..ContentRestoreReport::default()
    };

    apply_batch_ledger_updates(data_dir, &ordinary, declined, &mut report, &mut on_progress);

    let mut accepted = ordinary;
    let epub_selections = selected
        .iter()
        .filter(|selection| selection.candidate.target_kind == ContentKind::Epub)
        .collect::<Vec<_>>();
    let epub_mappings = epub_selections
        .iter()
        .map(|selection| restore_copy_mappings(data_dir, &selection.candidate, &selection.source))
        .collect::<Vec<_>>();
    let epub_store_count = crate::rename_key_migration::STORES
        .iter()
        .filter(|store| store.unique && store.file != "content_identity.db")
        .count();
    let epub_total = epub_mappings
        .iter()
        .map(|mappings| mappings.len() * epub_store_count + 1)
        .sum();
    let mut epub_processed = 0;
    for (selection, mappings) in epub_selections.into_iter().zip(epub_mappings) {
        let copy_work = mappings.len() * epub_store_count;
        let processed_before = epub_processed;
        let Some(expected) = selection.candidate.epub_source_state else {
            epub_processed += copy_work + 1;
            on_progress("epub", epub_processed, epub_total);
            continue;
        };
        let result = crate::pdf_loader::with_epub_pin_guard(
            &selection.candidate.target_path,
            || {
                if super::capture_epub_provenance(&selection.candidate.target_path)?
                    != super::EpubProvenance::Unpinned(expected)
                {
                    return Ok(None);
                }
                let db = ContentIdentityDb::open_at(&data_dir.join("content_identity.db"))
                    .map_err(|error| error.to_string())?;
                let source_entry = db
                    .ledger_entry(&selection.source.file_key)?
                    .ok_or_else(|| "restore source ledger row is missing".to_string())?;
                if source_entry.full_hash.as_deref() != Some(selection.candidate.full_hash.as_str())
                {
                    return Err("restore source hash changed".into());
                }
                if db
                    .ledger_entry(&selection.candidate.target_key)?
                    .is_some_and(|entry| entry.has_restorable_content)
                {
                    return Ok(None);
                }
                let copied =
                    crate::rename_key_migration::copy_restore_stores_without_identity_at_with_progress(
                        data_dir,
                        &mappings,
                        |processed, _| {
                            on_progress("epub", processed_before + processed, epub_total)
                        },
                    );
                if !copied.errors.is_empty() {
                    return Err(copied.errors.join("; "));
                }
                let (entry, changed) = mark_restored_origin_inner(
                    &db,
                    &selection.candidate,
                    &selection.source,
                    Some(expected),
                )?;
                Ok(Some((entry, changed, copied)))
            },
        );
        match result {
            Ok(Some((entry, changed, copied))) => {
                report.database_opens += copied.database_opens + 1;
                report.rows += copied.rows + usize::from(changed);
                report.ledger_entries.push(entry);
                accepted.push(selection);
            }
            Ok(None) => {}
            Err(error) => report.errors.push(format!(
                "content_identity target={}: {error}",
                selection.candidate.target_path.display()
            )),
        }
        epub_processed += copy_work + 1;
        on_progress("epub", epub_processed, epub_total);
    }

    match load_restore_runtime_updates(data_dir, &accepted) {
        Ok((sidecar_mirrors, presence, database_opens)) => {
            report.database_opens += database_opens;
            report.sidecar_mirrors = sidecar_mirrors;
            report.presence = presence;
            if load_sidecar_bases {
                report.sidecar_bases = load_restore_sidecar_bases(&report.sidecar_mirrors);
            }
        }
        Err(error) => report.errors.push(format!("sidecar mirror: {error}")),
    }
    on_progress("runtime", 1, 1);
    report
}

fn load_restore_sidecar_bases(
    mirrors: &[RestoreSidecarMirror],
) -> Vec<crate::sidecar::SidecarFile> {
    mirrors
        .iter()
        .map(|mirror| mirror.folder.clone())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .map(|folder| crate::sidecar::SidecarFile::load(&folder))
        .collect()
}

fn apply_batch_ledger_updates(
    data_dir: &Path,
    selected: &[&SelectedRestore],
    declined: &[DeclinedRestore],
    report: &mut ContentRestoreReport,
    on_progress: &mut impl FnMut(&'static str, usize, usize),
) {
    if selected.is_empty() && declined.is_empty() {
        return;
    }
    report.database_opens += 1;
    let db = match ContentIdentityDb::open_at(&data_dir.join("content_identity.db")) {
        Ok(db) => db,
        Err(error) => {
            report.errors.push(format!("content_identity: {error}"));
            return;
        }
    };
    let total = selected.len() + declined.len();
    let mut processed = 0;
    for selection in selected {
        match mark_restored_origin(&db, &selection.candidate, &selection.source) {
            Ok(Some((entry, changed))) => {
                report.rows += usize::from(changed);
                report.ledger_entries.push(entry);
            }
            Ok(None) => {} // a pin or source replacement superseded this restore
            Err(error) => report.errors.push(format!(
                "content_identity target={}: {error}",
                selection.candidate.target_path.display()
            )),
        }
        processed += 1;
        on_progress("ledger", processed, total);
    }
    for refusal in declined {
        match record_restore_declined(&db, refusal) {
            Ok(changed) => report.rows += usize::from(changed),
            Err(error) => report.errors.push(format!(
                "content_identity target={}: {error}",
                refusal.target_key
            )),
        }
        processed += 1;
        on_progress("ledger", processed, total);
    }
}

fn record_restore_declined(
    db: &ContentIdentityDb,
    refusal: &DeclinedRestore,
) -> Result<bool, String> {
    super::with_epub_ledger_key_guard(&refusal.target_key, || {
        db.conn
            .execute(
                "INSERT OR IGNORE INTO restore_declined(full_hash, target_key) VALUES (?1, ?2)",
                rusqlite::params![refusal.full_hash, refusal.target_key],
            )
            .map(|rows| rows > 0)
            .map_err(|error| error.to_string())
    })
}

fn record_internal_byte_copy_decline(
    db: &ContentIdentityDb,
    source_path: &Path,
    target_path: &Path,
) -> Result<InternalByteCopyDeclineOutcome, String> {
    let source_key = crate::path_key::normalize_keep_drive(source_path);
    let target_key = crate::path_key::normalize_keep_drive(target_path);
    let Some(source) = db.ledger_entry(&source_key).map_err(|error| {
        format!(
            "content_identity internal byte-copy source={source_key} target={target_key}: {error}"
        )
    })?
    else {
        return Ok(InternalByteCopyDeclineOutcome::SourceNotTracked);
    };
    let Some(full_hash) = source.full_hash else {
        return Ok(InternalByteCopyDeclineOutcome::SourceHashUnavailable);
    };
    let refusal = DeclinedRestore {
        full_hash,
        target_key,
    };
    record_restore_declined(db, &refusal)
        .map(|changed| {
            if changed {
                InternalByteCopyDeclineOutcome::Recorded
            } else {
                InternalByteCopyDeclineOutcome::AlreadyRecorded
            }
        })
        .map_err(|error| {
            format!(
                "content_identity internal byte-copy target={}: {error}",
                refusal.target_key
            )
        })
}

fn restore_copy_mappings(
    data_dir: &Path,
    candidate: &RestoreCandidate,
    source: &RestoreSourceCandidate,
) -> Vec<StoreCopyPathMapping> {
    let mut mappings = vec![StoreCopyPathMapping::exact(
        &source.path,
        &candidate.target_path,
    )];
    if matches!(
        source.kind,
        ContentKind::Zip | ContentKind::Pdf | ContentKind::Epub | ContentKind::Convertible
    ) {
        mappings.push(StoreCopyPathMapping::virtual_prefix(
            &source.path,
            &candidate.target_path,
        ));
    }
    if source.kind == ContentKind::Convertible && candidate.target_kind == ContentKind::Convertible
    {
        let old_cache = crate::archive_cache::cache_zip_path_for_data_dir(data_dir, &source.path);
        let new_cache =
            crate::archive_cache::cache_zip_path_for_data_dir(data_dir, &candidate.target_path);
        mappings.push(StoreCopyPathMapping::exact(&old_cache, &new_cache));
        mappings.push(StoreCopyPathMapping::virtual_prefix(old_cache, new_cache));
    }
    mappings
}

fn mark_restored_origin(
    db: &ContentIdentityDb,
    candidate: &RestoreCandidate,
    source: &RestoreSourceCandidate,
) -> Result<Option<(LedgerEntry, bool)>, String> {
    let epub_state = if candidate.target_kind == ContentKind::Epub {
        match super::capture_epub_provenance(&candidate.target_path)? {
            super::EpubProvenance::Unpinned(state) => Some(state),
            super::EpubProvenance::Pinned(_) => return Ok(None),
        }
    } else {
        None
    };
    let action = || {
        if let Some(expected) = epub_state
            && super::capture_epub_provenance(&candidate.target_path)?
                != super::EpubProvenance::Unpinned(expected)
        {
            return Ok(None);
        }
        mark_restored_origin_inner(db, candidate, source, epub_state).map(Some)
    };
    if epub_state.is_some() {
        crate::pdf_loader::with_epub_pin_guard(&candidate.target_path, action)
    } else {
        action()
    }
}

fn mark_restored_origin_inner(
    db: &ContentIdentityDb,
    candidate: &RestoreCandidate,
    source: &RestoreSourceCandidate,
    epub_state: Option<crate::epub_cache::SourceState>,
) -> Result<(LedgerEntry, bool), String> {
    let source_entry = db
        .ledger_entry(&source.file_key)?
        .ok_or_else(|| format!("source ledger row is missing: {}", source.file_key))?;
    if source_entry.full_hash.as_deref() != Some(candidate.full_hash.as_str()) {
        return Err(format!(
            "source hash changed before restore: {}",
            source.file_key
        ));
    }
    if let Some(existing) = db.ledger_entry(&candidate.target_key)?
        && existing.has_restorable_content
    {
        return Ok((existing, false));
    }

    let metadata = std::fs::metadata(&candidate.target_path).map_err(|error| {
        format!(
            "target metadata {}: {error}",
            candidate.target_path.display()
        )
    })?;
    if !metadata.is_file() {
        return Err(format!(
            "restore target is not a regular file: {}",
            candidate.target_path.display()
        ));
    }
    if epub_state.is_some_and(|expected| crate::epub_cache::source_state(&metadata) != expected) {
        return Err("restore target EPUB changed before ledger write".into());
    }
    let size = i64::try_from(metadata.len())
        .map_err(|_| "target file size exceeds SQLite INTEGER".to_string())?;
    let hashed_mtime = metadata_mtime(&metadata)?;
    let changed = db
        .conn
        .execute(
            "INSERT INTO edit_origin
                 (file_key, size, head_hash, full_hash, hashed_mtime, kind, last_edit_at,
                  has_restorable_content)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 1)
             ON CONFLICT(file_key) DO UPDATE SET
                 size = excluded.size,
                 head_hash = excluded.head_hash,
                 full_hash = excluded.full_hash,
                 hashed_mtime = excluded.hashed_mtime,
                 kind = excluded.kind,
                 last_edit_at = excluded.last_edit_at,
                 has_restorable_content = 1
             WHERE edit_origin.has_restorable_content = 0",
            rusqlite::params![
                candidate.target_key,
                size,
                source_entry.head_hash,
                candidate.full_hash,
                hashed_mtime,
                candidate.target_kind.as_str(),
                source_entry.last_edit_at,
            ],
        )
        .map_err(|error| error.to_string())?
        > 0;
    let entry = db
        .ledger_entry(&candidate.target_key)?
        .ok_or_else(|| "restored target ledger row was not stored".to_string())?;
    Ok((entry, changed))
}

#[derive(Clone, Debug)]
struct DestinationEditFamily {
    base_path: PathBuf,
    base_key: String,
}

fn destination_edit_families(
    data_dir: &Path,
    candidate: &RestoreCandidate,
    source: &RestoreSourceCandidate,
) -> Vec<DestinationEditFamily> {
    let mut families = Vec::new();
    let mut push_if_changed = |old_path: &Path, new_path: &Path| {
        let old_key = crate::path_key::normalize_keep_drive(old_path);
        let new_key = crate::path_key::normalize_keep_drive(new_path);
        if old_key != new_key {
            families.push(DestinationEditFamily {
                base_path: new_path.to_path_buf(),
                base_key: new_key,
            });
        }
    };
    push_if_changed(&source.path, &candidate.target_path);
    if source.kind == ContentKind::Convertible && candidate.target_kind == ContentKind::Convertible
    {
        let old_cache = crate::archive_cache::cache_zip_path_for_data_dir(data_dir, &source.path);
        let new_cache =
            crate::archive_cache::cache_zip_path_for_data_dir(data_dir, &candidate.target_path);
        push_if_changed(&old_cache, &new_cache);
    }
    families
}

fn query_family_rows(
    data_dir: &Path,
    file: &str,
    table: &str,
    key_column: &str,
    selected_columns: &str,
    families: &[DestinationEditFamily],
    mut visit: impl FnMut(&rusqlite::Row<'_>) -> Result<(), String>,
) -> Result<usize, String> {
    let path = data_dir.join(file);
    if !path.exists() || families.is_empty() {
        return Ok(0);
    }
    let conn = rusqlite::Connection::open(&path).map_err(|error| error.to_string())?;
    conn.busy_timeout(std::time::Duration::from_secs(5))
        .map_err(|error| error.to_string())?;
    let select = format!("SELECT {key_column}, {selected_columns} FROM {table}");
    let mut exact = conn
        .prepare(&format!("{select} WHERE {key_column} = ?1"))
        .map_err(|error| error.to_string())?;
    let mut range = conn
        .prepare(&format!(
            "{select} WHERE {key_column} >= ?1 AND {key_column} < ?2"
        ))
        .map_err(|error| error.to_string())?;
    let mut no_upper = conn
        .prepare(&format!(
            "{select} WHERE {key_column} >= ?1 AND substr({key_column}, 1, ?2) = ?1"
        ))
        .map_err(|error| error.to_string())?;
    for family in families {
        let prefix = format!("{}::", family.base_key);
        let mut rows = exact
            .query([&family.base_key])
            .map_err(|error| error.to_string())?;
        while let Some(row) = rows.next().map_err(|error| error.to_string())? {
            visit(row)?;
        }
        drop(rows);
        let mut rows =
            if let Some(upper) = crate::rename_key_migration::prefix_upper_bound(&prefix) {
                range.query(rusqlite::params![prefix, upper])
            } else {
                no_upper.query(rusqlite::params![prefix, prefix.chars().count() as i64])
            }
            .map_err(|error| error.to_string())?;
        while let Some(row) = rows.next().map_err(|error| error.to_string())? {
            visit(row)?;
        }
    }
    Ok(1)
}

fn sidecar_mask_from_row(
    row: &rusqlite::Row<'_>,
    data_index: usize,
    width_index: usize,
    height_index: usize,
    shapes_index: usize,
) -> Result<crate::sidecar::SidecarMask, String> {
    let raw: Vec<u8> = row.get(data_index).map_err(|error| error.to_string())?;
    let width: i64 = row.get(width_index).map_err(|error| error.to_string())?;
    let height: i64 = row.get(height_index).map_err(|error| error.to_string())?;
    let shapes: Option<String> = row.get(shapes_index).map_err(|error| error.to_string())?;
    let width = u32::try_from(width)
        .ok()
        .filter(|value| *value > 0)
        .ok_or_else(|| format!("invalid mask width: {width}"))?;
    let height = u32::try_from(height)
        .ok()
        .filter(|value| *value > 0)
        .ok_or_else(|| format!("invalid mask height: {height}"))?;
    let shapes = shapes
        .as_deref()
        .map(crate::mask_db::try_shapes_from_json)
        .transpose()
        .map_err(|error| error.to_string())?
        .unwrap_or_default();
    Ok(crate::sidecar::SidecarMask::from_raw(
        &raw, &shapes, width, height,
    ))
}

fn load_restore_runtime_updates(
    data_dir: &Path,
    selected: &[&SelectedRestore],
) -> Result<(Vec<RestoreSidecarMirror>, RestorePresence, usize), String> {
    let families = selected
        .iter()
        .flat_map(|selection| {
            destination_edit_families(data_dir, &selection.candidate, &selection.source)
        })
        .collect::<Vec<_>>();
    let mut database_opens = 0;
    let mut states = BTreeMap::<String, crate::sidecar::SidecarEntry>::new();

    database_opens += query_family_rows(
        data_dir,
        "adjustment.db",
        "page_params",
        "page_path",
        "params_json",
        &families,
        |row| {
            let key: String = row.get(0).map_err(|error| error.to_string())?;
            let json: String = row.get(1).map_err(|error| error.to_string())?;
            states.entry(key).or_default().adjust =
                Some(serde_json::from_str(&json).map_err(|error| error.to_string())?);
            Ok(())
        },
    )?;
    database_opens += query_family_rows(
        data_dir,
        "mask.db",
        "masks",
        "path",
        "mask_data, width, height, vectors",
        &families,
        |row| {
            let key: String = row.get(0).map_err(|error| error.to_string())?;
            states.entry(key).or_default().mask = Some(sidecar_mask_from_row(row, 1, 2, 3, 4)?);
            Ok(())
        },
    )?;
    database_opens += query_family_rows(
        data_dir,
        "conceal.db",
        "conceal_entries",
        "page_path",
        "bitmap_data, bitmap_w, bitmap_h, shapes",
        &families,
        |row| {
            let key: String = row.get(0).map_err(|error| error.to_string())?;
            states.entry(key).or_default().conceal = Some(sidecar_mask_from_row(row, 1, 2, 3, 4)?);
            Ok(())
        },
    )?;
    database_opens += query_family_rows(
        data_dir,
        "local_adjust.db",
        "local_adjust_pages",
        "page_path",
        "layers_json",
        &families,
        |row| {
            let key: String = row.get(0).map_err(|error| error.to_string())?;
            let json: String = row.get(1).map_err(|error| error.to_string())?;
            states.entry(key).or_default().local_adjust_layers =
                Some(local_adjust_core::LocalAdjustmentLayers::new(
                    crate::local_adjust_db::parse_layers_json(&json)
                        .map_err(|error| error.to_string())?,
                ));
            Ok(())
        },
    )?;
    database_opens += query_family_rows(
        data_dir,
        "export_crop.db",
        "export_crop_pages",
        "page_path",
        "min_x, min_y, max_x, max_y, aspect_mode, source_width, source_height",
        &families,
        |row| {
            let key: String = row.get(0).map_err(|error| error.to_string())?;
            let aspect: String = row.get(5).map_err(|error| error.to_string())?;
            states.entry(key).or_default().export_crop = Some(crate::export_crop::CropSettings {
                rect: crate::export_crop::CropRect {
                    min_x: row.get(1).map_err(|error| error.to_string())?,
                    min_y: row.get(2).map_err(|error| error.to_string())?,
                    max_x: row.get(3).map_err(|error| error.to_string())?,
                    max_y: row.get(4).map_err(|error| error.to_string())?,
                },
                aspect_mode: crate::export_crop::CropAspectMode::from_stable_key(&aspect),
                source_size: crate::export_crop::read_source_size(row, 6, 7)
                    .map_err(|error| error.to_string())?,
            });
            Ok(())
        },
    )?;
    database_opens += query_family_rows(
        data_dir,
        "comic.db",
        "comic_entries",
        "page_path",
        "doc_json",
        &families,
        |row| {
            let key: String = row.get(0).map_err(|error| error.to_string())?;
            let json: String = row.get(1).map_err(|error| error.to_string())?;
            states.entry(key).or_default().comic =
                Some(serde_json::from_str(&json).map_err(|error| error.to_string())?);
            Ok(())
        },
    )?;
    let mut rotations = BTreeSet::new();
    database_opens += query_family_rows(
        data_dir,
        "rotation.db",
        "rotations",
        "path",
        "angle",
        &families,
        |row| {
            let key: String = row.get(0).map_err(|error| error.to_string())?;
            rotations.insert(key);
            Ok(())
        },
    )?;

    let mut presence = RestorePresence {
        rotations,
        ..RestorePresence::default()
    };
    let mut mirrors = Vec::new();
    for (key, entry) in states {
        presence.page_edits.insert(key.clone());
        if entry.adjust.is_some() {
            presence.adjusted.insert(key.clone());
        }
        if entry.mask.is_some() {
            presence.masks.insert(key.clone());
        }
        if entry.conceal.is_some() {
            presence.conceals.insert(key.clone());
        }
        if entry
            .local_adjust_layers
            .as_ref()
            .is_some_and(|layers| !layers.is_empty())
        {
            presence.local_adjustments.insert(key.clone());
        }
        if entry
            .comic
            .as_ref()
            .is_some_and(|objects| !objects.is_empty())
        {
            presence.comics.insert(key.clone());
        }
        if entry.export_crop.is_some() {
            presence.crops.insert(key.clone());
        }
        if let Some((folder, rel_key)) = sidecar_coords_for_key(&families, &key) {
            mirrors.push(RestoreSidecarMirror {
                folder,
                rel_key,
                entry,
            });
        }
    }
    Ok((mirrors, presence, database_opens))
}

fn sidecar_coords_for_key(
    families: &[DestinationEditFamily],
    key: &str,
) -> Option<(PathBuf, String)> {
    for family in families {
        let suffix = if key == family.base_key {
            ""
        } else if let Some(suffix) = key.strip_prefix(&format!("{}::", family.base_key)) {
            suffix
        } else {
            continue;
        };
        let folder = family.base_path.parent()?.to_path_buf();
        let file_name = family
            .base_path
            .file_name()?
            .to_string_lossy()
            .to_lowercase();
        let rel_key = if suffix.is_empty() {
            file_name
        } else {
            format!("{file_name}::{suffix}")
        };
        return Some((folder, rel_key));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn restore_promotion_cannot_replace_a_pinned_epub_identity() {
        let fixture = crate::epub_cache::reconverted_for_worker_test();
        let temp = tempfile::tempdir().unwrap();
        let db = ContentIdentityDb::open_at(&temp.path().join("content_identity.db")).unwrap();
        let origin = temp.path().join("origin.png");
        std::fs::write(&origin, b"source").unwrap();
        let source = ContentIdentitySource::from_path(&origin).unwrap();
        let origin_key = crate::path_key::normalize_keep_drive(&origin);
        db.upsert(
            &source,
            &RecordedFileState {
                file_key: origin_key.clone(),
                size: 6,
                hashed_mtime: 1,
            },
            "head",
            "full",
            1,
            ObservationRole::RestorableContent,
        )
        .unwrap();
        let restore_source = RestoreSourceCandidate {
            file_key: origin_key,
            path: origin,
            kind: ContentKind::Image,
            last_edit_at: 1,
            source_exists: true,
        };
        let candidate = RestoreCandidate {
            target_key: crate::path_key::normalize_keep_drive(&fixture.source),
            target_path: fixture.source.clone(),
            target_kind: ContentKind::Epub,
            full_hash: "full".into(),
            sources: vec![restore_source.clone()],
            epub_source_state: None,
        };
        assert!(
            mark_restored_origin(&db, &candidate, &restore_source)
                .unwrap()
                .is_none()
        );
        assert!(db.ledger_entry(&candidate.target_key).unwrap().is_none());
    }

    #[test]
    fn restore_candidates_rechecks_epub_before_copying_edits() {
        let data = tempfile::tempdir().unwrap();
        let origin = data.path().join("origin.png");
        let epub = data.path().join("book.epub");
        let original_bytes = b"source";
        std::fs::write(&origin, original_bytes).unwrap();
        std::fs::write(&epub, original_bytes).unwrap();
        let db = ContentIdentityDb::open_at(&data.path().join("content_identity.db")).unwrap();
        let origin_key = crate::path_key::normalize_keep_drive(&origin);
        let target_key = crate::path_key::normalize_keep_drive(&epub);
        let head = super::super::stage1_head_hash(
            &mut std::io::Cursor::new(original_bytes),
            original_bytes.len() as u64,
        )
        .unwrap();
        let full =
            super::super::stage2_full_hash(&mut std::io::Cursor::new(original_bytes)).unwrap();
        db.upsert(
            &ContentIdentitySource::from_path(&origin).unwrap(),
            &RecordedFileState {
                file_key: origin_key.clone(),
                size: 6,
                hashed_mtime: 1,
            },
            &head,
            &full,
            1,
            ObservationRole::RestorableContent,
        )
        .unwrap();
        let rotations = rusqlite::Connection::open(data.path().join("rotation.db")).unwrap();
        rotations
            .execute_batch("CREATE TABLE rotations (path TEXT PRIMARY KEY, angle INTEGER NOT NULL)")
            .unwrap();
        rotations
            .execute(
                "INSERT INTO rotations(path, angle) VALUES (?1, 90)",
                [&origin_key],
            )
            .unwrap();
        let origin_entry = db.ledger_entry(&origin_key).unwrap().unwrap();
        let detection = super::super::DetectionTarget {
            source: ContentIdentitySource::from_path(&epub).unwrap(),
            file_key: target_key.clone(),
            size: original_bytes.len() as u64,
            origins: vec![origin_entry],
        };
        let (candidate, observed) =
            super::super::detect_target(&db, detection, &std::sync::atomic::AtomicBool::new(false))
                .unwrap()
                .unwrap();
        let candidate = candidate.expect("matching EPUB must create a restore candidate");
        assert!(candidate.epub_source_state.is_some());
        let source = candidate.sources[0].clone();

        // Replacement and pin happen after detection but before restore.
        std::fs::write(&epub, b"replacement with different content").unwrap();
        let _pin = crate::pdf_loader::pin_epub_for_test(&epub, 44, 4096);
        let selection = SelectedRestore { candidate, source };
        let report = restore_candidates_at(data.path(), &[selection], &[], false);
        assert!(report.ledger_entries.is_empty());
        assert_eq!(db.ledger_entry(&target_key).unwrap(), Some(observed));
        let copied: i64 = rotations
            .query_row(
                "SELECT COUNT(*) FROM rotations WHERE path = ?1",
                [&target_key],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(copied, 0, "stale restore must not copy any edit rows");
    }

    #[test]
    fn epub_restore_copies_virtual_pages_and_reports_progress() {
        let data = tempfile::tempdir().unwrap();
        let origin = data.path().join("origin.epub");
        let target = data.path().join("copy.epub");
        std::fs::write(&origin, b"same book bytes").unwrap();
        std::fs::write(&target, b"same book bytes").unwrap();
        let origin_key = crate::path_key::normalize_keep_drive(&origin);
        let target_key = crate::path_key::normalize_keep_drive(&target);
        let db = ContentIdentityDb::open_at(&data.path().join("content_identity.db")).unwrap();
        db.upsert(
            &ContentIdentitySource::from_path(&origin).unwrap(),
            &RecordedFileState {
                file_key: origin_key.clone(),
                size: 15,
                hashed_mtime: 1,
            },
            "head",
            "full",
            1,
            ObservationRole::RestorableContent,
        )
        .unwrap();
        let rotations = rusqlite::Connection::open(data.path().join("rotation.db")).unwrap();
        rotations
            .execute_batch("CREATE TABLE rotations (path TEXT PRIMARY KEY, angle INTEGER NOT NULL)")
            .unwrap();
        for key in [origin_key.clone(), format!("{origin_key}::page_1")] {
            rotations
                .execute("INSERT INTO rotations(path, angle) VALUES (?1, 90)", [key])
                .unwrap();
        }
        let (mut candidate, source) = candidate(origin, target.clone(), ContentKind::Epub, "full");
        candidate.epub_source_state = Some(crate::epub_cache::source_state(
            &std::fs::metadata(target).unwrap(),
        ));
        let mut progress = Vec::new();
        let report = restore_candidates_at_with_progress(
            data.path(),
            &[SelectedRestore { candidate, source }],
            &[],
            false,
            |stage, processed, total| progress.push((stage, processed, total)),
        );
        assert!(report.errors.is_empty(), "{:?}", report.errors);
        assert_eq!(report.ledger_entries.len(), 1);
        assert!(
            db.ledger_entry(&target_key)
                .unwrap()
                .unwrap()
                .has_restorable_content
        );
        for key in [target_key.clone(), format!("{target_key}::page_1")] {
            assert_eq!(
                rotations
                    .query_row(
                        "SELECT angle FROM rotations WHERE path = ?1",
                        [key],
                        |row| row.get::<_, i64>(0)
                    )
                    .unwrap(),
                90,
            );
        }
        let epub_progress = progress
            .iter()
            .filter(|event| event.0 == "epub")
            .collect::<Vec<_>>();
        assert!(!epub_progress.is_empty());
        assert!(
            epub_progress
                .windows(2)
                .all(|events| events[0].1 <= events[1].1)
        );
        assert_eq!(
            epub_progress.last().unwrap().1,
            epub_progress.last().unwrap().2
        );
    }
    use crate::content_identity::{
        ContentIdentitySource, ObservationRole, RecordedFileState, stage0_target,
    };
    use comic_core::{AnnotationObject, TextBlock};

    fn candidate(
        source_path: PathBuf,
        target_path: PathBuf,
        kind: ContentKind,
        full_hash: &str,
    ) -> (RestoreCandidate, RestoreSourceCandidate) {
        let source_key = crate::path_key::normalize_keep_drive(&source_path);
        let target_key = crate::path_key::normalize_keep_drive(&target_path);
        (
            RestoreCandidate {
                target_key,
                target_path,
                target_kind: kind,
                full_hash: full_hash.to_string(),
                sources: Vec::new(),
                epub_source_state: None,
            },
            RestoreSourceCandidate {
                file_key: source_key,
                path: source_path,
                kind,
                last_edit_at: 10,
                source_exists: true,
            },
        )
    }

    fn create_rotation_rows(data_dir: &Path, keys: &[(String, i64)]) {
        let connection = rusqlite::Connection::open(data_dir.join("rotation.db")).unwrap();
        connection
            .execute_batch("CREATE TABLE rotations (path TEXT PRIMARY KEY, angle INTEGER NOT NULL)")
            .unwrap();
        for (key, angle) in keys {
            connection
                .execute(
                    "INSERT INTO rotations(path, angle) VALUES (?1, ?2)",
                    rusqlite::params![key, angle],
                )
                .unwrap();
        }
    }

    #[test]
    fn image_mappings_skip_virtual_prefix_and_containers_keep_it() {
        let data = tempfile::tempdir().unwrap();
        for (kind, expected) in [
            (ContentKind::Image, 1),
            (ContentKind::Zip, 2),
            (ContentKind::Pdf, 2),
            (ContentKind::Epub, 2),
            (ContentKind::Convertible, 4),
        ] {
            let (candidate, source) = candidate(
                PathBuf::from(r"C:\本\old.ext"),
                PathBuf::from(r"D:\本\new.ext"),
                kind,
                "hash",
            );
            let mappings = restore_copy_mappings(data.path(), &candidate, &source);
            assert_eq!(mappings.len(), expected, "{kind:?}");
            assert_eq!(
                mappings
                    .iter()
                    .filter(|mapping| matches!(mapping, StoreCopyPathMapping::VirtualPrefix { .. }))
                    .count(),
                if kind == ContentKind::Convertible {
                    2
                } else {
                    expected - 1
                },
                "{kind:?}"
            );
        }
    }

    fn assert_runtime_family_query_plans(data_dir: &Path) {
        for (file, table, column, selected) in [
            ("adjustment.db", "page_params", "page_path", "params_json"),
            (
                "mask.db",
                "masks",
                "path",
                "mask_data, width, height, vectors",
            ),
            (
                "conceal.db",
                "conceal_entries",
                "page_path",
                "bitmap_data, bitmap_w, bitmap_h, shapes",
            ),
            (
                "local_adjust.db",
                "local_adjust_pages",
                "page_path",
                "layers_json",
            ),
            (
                "export_crop.db",
                "export_crop_pages",
                "page_path",
                "min_x, min_y, max_x, max_y, aspect_mode, source_width, source_height",
            ),
            ("comic.db", "comic_entries", "page_path", "doc_json"),
            ("rotation.db", "rotations", "path", "angle"),
        ] {
            let connection = rusqlite::Connection::open(data_dir.join(file)).unwrap();
            for predicate in [
                format!("{column} = ?1"),
                format!("{column} >= ?1 AND {column} < ?2"),
                format!("{column} >= ?1 AND substr({column}, 1, ?2) = ?1"),
            ] {
                let sql = format!(
                    "EXPLAIN QUERY PLAN SELECT {column}, {selected} FROM {table} WHERE {predicate}"
                );
                let mut statement = connection.prepare(&sql).unwrap();
                let plan: String = if predicate == format!("{column} = ?1") {
                    statement
                        .query_row(["c:/日本/本::"], |row| row.get(3))
                        .unwrap()
                } else {
                    statement
                        .query_row(
                            rusqlite::params!["c:/日本/本::", "c:/日本/本:;"],
                            |row| row.get(3),
                        )
                        .unwrap()
                };
                assert!(
                    plan.contains("SEARCH") && plan.contains("INDEX") && !plan.contains("SCAN"),
                    "{file}.{table}: {plan}"
                );
            }
        }
    }

    #[test]
    fn runtime_family_exact_and_prefix_query_plans_search_key_index() {
        let data = tempfile::tempdir().unwrap();
        create_production_store_schemas(data.path());
        assert_runtime_family_query_plans(data.path());
    }

    #[test]
    fn runtime_family_rows_match_legacy_or_substr_for_unicode_and_case() {
        let data = tempfile::tempdir().unwrap();
        let connection = rusqlite::Connection::open(data.path().join("rotation.db")).unwrap();
        connection
            .execute_batch("CREATE TABLE rotations (path TEXT PRIMARY KEY, angle INTEGER NOT NULL)")
            .unwrap();
        for (index, key) in [
            "c:/日本/本.zip",
            "c:/日本/本.zip::一.jpg",
            "c:/日本/本.zip::二.jpg",
            "c:/日本/本.zip:;outside",
            "c:/A.zip",
            "c:/A.zip::upper",
            "c:/a.zip",
            "c:/a.zip::lower",
            "c:/\u{10ffff}.zip::edge",
        ]
        .into_iter()
        .enumerate()
        {
            connection
                .execute(
                    "INSERT INTO rotations VALUES (?1, ?2)",
                    rusqlite::params![key, index as i64],
                )
                .unwrap();
        }
        for base in [
            "c:/日本/本.zip",
            "c:/A.zip",
            "c:/a.zip",
            "c:/\u{10ffff}.zip",
        ] {
            let family = DestinationEditFamily {
                base_path: PathBuf::from(base),
                base_key: base.to_owned(),
            };
            let mut actual = Vec::new();
            query_family_rows(
                data.path(),
                "rotation.db",
                "rotations",
                "path",
                "angle",
                &[family],
                |row| {
                    actual.push((
                        row.get::<_, String>(0).map_err(|error| error.to_string())?,
                        row.get::<_, i64>(1).map_err(|error| error.to_string())?,
                    ));
                    Ok(())
                },
            )
            .unwrap();
            let prefix = format!("{base}::");
            let mut expected = connection
                .prepare(
                    "SELECT path, angle FROM rotations WHERE path = ?1 OR substr(path, 1, ?2) = ?3",
                )
                .unwrap()
                .query_map(
                    rusqlite::params![base, prefix.chars().count() as i64, prefix],
                    |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
                )
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap();
            actual.sort();
            expected.sort();
            assert_eq!(actual, expected, "{base}");
        }
    }

    fn create_all_unique_store_schemas(data_dir: &Path) {
        for descriptor in crate::rename_key_migration::STORES
            .iter()
            .filter(|descriptor| descriptor.unique && descriptor.file != "content_identity.db")
        {
            let connection = rusqlite::Connection::open(data_dir.join(descriptor.file)).unwrap();
            let sql = match descriptor.table {
                "ratings" => {
                    "CREATE TABLE IF NOT EXISTS ratings (
                    path TEXT PRIMARY KEY, source_path TEXT)"
                }
                "page_params" => {
                    "CREATE TABLE IF NOT EXISTS page_params (
                    page_path TEXT PRIMARY KEY, params_json TEXT NOT NULL)"
                }
                "masks" => {
                    "CREATE TABLE IF NOT EXISTS masks (
                    path TEXT PRIMARY KEY, mask_data BLOB, width INTEGER,
                    height INTEGER, vectors TEXT)"
                }
                "conceal_entries" => {
                    "CREATE TABLE IF NOT EXISTS conceal_entries (
                    page_path TEXT PRIMARY KEY, bitmap_data BLOB, bitmap_w INTEGER,
                    bitmap_h INTEGER, shapes TEXT)"
                }
                "local_adjust_pages" => {
                    "CREATE TABLE IF NOT EXISTS local_adjust_pages (
                    page_path TEXT PRIMARY KEY, layers_json TEXT NOT NULL)"
                }
                "comic_entries" => {
                    "CREATE TABLE IF NOT EXISTS comic_entries (
                    page_path TEXT PRIMARY KEY, doc_json TEXT NOT NULL)"
                }
                "export_crop_pages" => {
                    "CREATE TABLE IF NOT EXISTS export_crop_pages (
                    page_path TEXT PRIMARY KEY, min_x REAL, min_y REAL, max_x REAL,
                    max_y REAL, aspect_mode TEXT, source_width INTEGER,
                    source_height INTEGER)"
                }
                "rotations" => {
                    "CREATE TABLE IF NOT EXISTS rotations (
                    path TEXT PRIMARY KEY, angle INTEGER NOT NULL DEFAULT 0)"
                }
                "reading_history" => {
                    "CREATE TABLE IF NOT EXISTS reading_history (
                    key TEXT PRIMARY KEY, path TEXT NOT NULL)"
                }
                _ => {
                    let sql = format!(
                        "CREATE TABLE IF NOT EXISTS {} ({} TEXT PRIMARY KEY)",
                        descriptor.table, descriptor.column
                    );
                    connection.execute_batch(&sql).unwrap();
                    continue;
                }
            };
            connection.execute_batch(sql).unwrap();
        }
    }

    /// Exercise the schema creation and additive migrations used by the application, rather
    /// than test-local CREATE TABLE statements. Every STORES file is opened at least once.
    fn create_production_store_schemas(data_dir: &Path) {
        drop(crate::rating_db::RatingDb::open_at(data_dir.join("rating.db")).unwrap());
        drop(ContentIdentityDb::open_at(&data_dir.join("content_identity.db")).unwrap());
        drop(crate::adjustment_db::AdjustmentDb::open_at(&data_dir.join("adjustment.db")).unwrap());
        drop(crate::mask_db::MaskDb::open_at(&data_dir.join("mask.db")).unwrap());
        drop(crate::conceal_db::ConcealDb::open_at(&data_dir.join("conceal.db")).unwrap());
        drop(
            crate::local_adjust_db::LocalAdjustDb::open_at(&data_dir.join("local_adjust.db"))
                .unwrap(),
        );
        drop(crate::comic_db::ComicDb::open_at(&data_dir.join("comic.db")).unwrap());
        drop(crate::export_crop::CropDb::open_at(&data_dir.join("export_crop.db")).unwrap());
        drop(
            crate::edit_preview_cache::EditPreviewCacheDb::open_at(
                &data_dir.join("edit_preview_cache.db"),
            )
            .unwrap(),
        );
        drop(crate::tags_db::TagsDb::open_at(&data_dir.join("tags.db")).unwrap());
        drop(crate::rotation_db::RotationDb::open_at(&data_dir.join("rotation.db")).unwrap());
        drop(crate::view_trim_db::ViewTrimDb::open_at(&data_dir.join("view_trim.db")).unwrap());
        drop(crate::video_pins::VideoPinDb::open_at(&data_dir.join("video_pins.db")).unwrap());
        drop(
            crate::video_bookmarks::VideoBookmarkDb::open_at(&data_dir.join("video_bookmarks.db"))
                .unwrap(),
        );
        drop(
            crate::folder_thumb_pins::FolderThumbPinDb::open_at(
                &data_dir.join("folder_thumb_pins.db"),
            )
            .unwrap(),
        );
        drop(
            crate::book_resume_db::BookResumeDb::open_at(&data_dir.join("book_resume.db")).unwrap(),
        );
        drop(crate::spread_db::SpreadDb::open_at(&data_dir.join("spread.db")).unwrap());
        drop(
            crate::reading_history_db::ReadingHistoryDb::open_at(
                data_dir.join("reading_history.db"),
            )
            .unwrap(),
        );
    }

    fn assert_production_store_prefix_plans(data_dir: &Path) {
        for descriptor in crate::rename_key_migration::STORES {
            let connection = rusqlite::Connection::open(data_dir.join(descriptor.file)).unwrap();
            let indexes = connection
                .prepare(&format!("PRAGMA index_list({})", descriptor.table))
                .unwrap()
                .query_map([], |row| row.get::<_, String>(1))
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap();
            let leading_binary = indexes.iter().any(|index| {
                let mut statement = connection
                    .prepare(&format!(
                        "PRAGMA index_xinfo('{}')",
                        index.replace('\'', "''")
                    ))
                    .unwrap();
                statement
                    .query_map([], |row| {
                        Ok((
                            row.get::<_, i64>(0)?,
                            row.get::<_, Option<String>>(2)?,
                            row.get::<_, String>(4)?,
                            row.get::<_, i64>(5)?,
                        ))
                    })
                    .unwrap()
                    .collect::<Result<Vec<_>, _>>()
                    .unwrap()
                    .iter()
                    .any(|(seq, column, collation, key)| {
                        *seq == 0
                            && column.as_deref() == Some(descriptor.column)
                            && collation == "BINARY"
                            && *key == 1
                    })
            });
            assert!(
                leading_binary,
                "{}.{}.{} lacks BINARY leading-key index",
                descriptor.file, descriptor.table, descriptor.column
            );
            let column = descriptor.column;
            let table = descriptor.table;
            for predicate in [
                format!("{column} >= ?1 AND {column} < ?2"),
                format!("{column} >= ?1 AND substr({column}, 1, ?2) = ?1"),
            ] {
                let sql = format!(
                    "EXPLAIN QUERY PLAN SELECT DISTINCT {column} FROM {table} WHERE {predicate}"
                );
                let plan: String = connection
                    .prepare(&sql)
                    .unwrap()
                    .query_row(
                        rusqlite::params!["c:/日本/本.zip::", "c:/日本/本.zip:;"],
                        |row| row.get(3),
                    )
                    .unwrap();
                assert!(
                    plan.contains("SEARCH") && plan.contains("INDEX") && !plan.contains("SCAN"),
                    "{}.{} {predicate}: {plan}",
                    descriptor.file,
                    descriptor.table
                );
            }
        }
    }

    #[test]
    fn production_store_prefix_plans_use_binary_leading_indexes() {
        let data = tempfile::tempdir().unwrap();
        create_production_store_schemas(data.path());
        assert_production_store_prefix_plans(data.path());
        // Reopening an already current schema uses the same production constructors.
        create_production_store_schemas(data.path());
        assert_production_store_prefix_plans(data.path());
    }

    #[test]
    fn migrated_production_store_prefix_plans_use_binary_leading_indexes() {
        let data = tempfile::tempdir().unwrap();
        // Released/development predecessor schemas exercise the additive or rebuild branches
        // of the same open_at constructors. All other stores start from an absent DB.
        for (file, sql) in [
            (
                "rating.db",
                "CREATE TABLE ratings(path TEXT PRIMARY KEY, stars INTEGER NOT NULL)",
            ),
            (
                "content_identity.db",
                "CREATE TABLE edit_origin (
                    file_key TEXT PRIMARY KEY, size INTEGER NOT NULL, head_hash TEXT NOT NULL,
                    full_hash TEXT, hashed_mtime INTEGER NOT NULL, kind TEXT NOT NULL,
                    last_edit_at INTEGER NOT NULL);
                 PRAGMA user_version = 0;",
            ),
            (
                "adjustment.db",
                "CREATE TABLE sidecar_sync (
                    folder_key TEXT PRIMARY KEY, synced_at INTEGER NOT NULL)",
            ),
            (
                "mask.db",
                "CREATE TABLE masks (
                    path TEXT PRIMARY KEY, mask_data BLOB, width INTEGER, height INTEGER)",
            ),
            (
                "export_crop.db",
                "CREATE TABLE export_crop_pages (
                    page_path TEXT PRIMARY KEY, min_x REAL NOT NULL, min_y REAL NOT NULL,
                    max_x REAL NOT NULL, max_y REAL NOT NULL, aspect_mode TEXT NOT NULL)",
            ),
            (
                "edit_preview_cache.db",
                "CREATE TABLE edit_previews(item_key TEXT PRIMARY KEY);
                 PRAGMA user_version = 1;",
            ),
            (
                "video_pins.db",
                "CREATE TABLE video_pins (
                    path TEXT PRIMARY KEY, pin_pts_secs REAL NOT NULL, thumb_webp BLOB)",
            ),
            (
                "video_bookmarks.db",
                "CREATE TABLE video_bookmarks (
                    id INTEGER PRIMARY KEY AUTOINCREMENT, path TEXT NOT NULL,
                    pts_secs REAL NOT NULL, title TEXT, thumb_webp BLOB,
                    created_at INTEGER NOT NULL)",
            ),
            (
                "folder_thumb_pins.db",
                "CREATE TABLE folder_thumb_pins (
                    container_key TEXT PRIMARY KEY, source_kind TEXT NOT NULL,
                    source_rel TEXT NOT NULL, source_entry TEXT, source_page INTEGER)",
            ),
            (
                "spread.db",
                "CREATE TABLE spreads(path TEXT PRIMARY KEY, mode INTEGER NOT NULL DEFAULT 0);
                 CREATE TABLE singleton_spread_placements (
                    path TEXT PRIMARY KEY, preference INTEGER NOT NULL)",
            ),
            (
                "reading_history.db",
                "CREATE TABLE reading_history (
                    key TEXT PRIMARY KEY, path TEXT NOT NULL, kind TEXT NOT NULL,
                    archive_format TEXT, title TEXT NOT NULL,
                    last_read_at_ms INTEGER NOT NULL, last_page INTEGER, page_count INTEGER,
                    file_size INTEGER, mtime_ms INTEGER)",
            ),
        ] {
            let connection = rusqlite::Connection::open(data.path().join(file)).unwrap();
            connection.execute_batch(sql).unwrap();
        }
        create_production_store_schemas(data.path());
        assert_production_store_prefix_plans(data.path());
        assert_runtime_family_query_plans(data.path());
    }

    #[test]
    fn retry_after_partial_store_commits_matches_uninterrupted_restore_rows() {
        let files = tempfile::tempdir().unwrap();
        let uninterrupted = tempfile::tempdir().unwrap();
        let resumed = tempfile::tempdir().unwrap();
        let source_path = files.path().join("origin.zip");
        let target_path = files.path().join("target.zip");
        std::fs::write(&source_path, b"same archive bytes").unwrap();
        std::fs::write(&target_path, b"same archive bytes").unwrap();
        let source_key = crate::path_key::normalize_keep_drive(&source_path);
        let target_key = crate::path_key::normalize_keep_drive(&target_path);
        let metadata = std::fs::metadata(&target_path).unwrap();
        let full_hash = "same-full-hash";
        let fixture = |data_dir: &Path| {
            create_all_unique_store_schemas(data_dir);
            let ledger = ContentIdentityDb::open_at(&data_dir.join("content_identity.db")).unwrap();
            ledger
                .upsert(
                    &ContentIdentitySource::new(&source_path, ContentKind::Zip),
                    &RecordedFileState {
                        file_key: source_key.clone(),
                        size: metadata.len(),
                        hashed_mtime: 1,
                    },
                    "head",
                    full_hash,
                    99,
                    ObservationRole::RestorableContent,
                )
                .unwrap();
            ledger
                .upsert(
                    &ContentIdentitySource::new(&target_path, ContentKind::Zip),
                    &RecordedFileState {
                        file_key: target_key.clone(),
                        size: metadata.len(),
                        hashed_mtime: metadata_mtime(&metadata).unwrap(),
                    },
                    "head",
                    full_hash,
                    0,
                    ObservationRole::DetectionCache,
                )
                .unwrap();
            drop(ledger);

            let ratings = rusqlite::Connection::open(data_dir.join("rating.db")).unwrap();
            for key in [source_key.clone(), format!("{source_key}::一.jpg")] {
                ratings
                    .execute(
                        "INSERT INTO ratings(path, source_path) VALUES (?1, ?2)",
                        rusqlite::params![key, source_key],
                    )
                    .unwrap();
            }
            let rotations = rusqlite::Connection::open(data_dir.join("rotation.db")).unwrap();
            for key in [source_key.clone(), format!("{source_key}::一.jpg")] {
                rotations
                    .execute("INSERT INTO rotations(path, angle) VALUES (?1, 90)", [key])
                    .unwrap();
            }
            rusqlite::Connection::open(data_dir.join("reading_history.db"))
                .unwrap()
                .execute(
                    "INSERT INTO reading_history(key, path) VALUES (?1, ?2)",
                    rusqlite::params![source_key, source_path.to_string_lossy()],
                )
                .unwrap();
        };
        fixture(uninterrupted.path());
        fixture(resumed.path());

        let (candidate, source) = candidate(source_path, target_path, ContentKind::Zip, full_hash);
        let selection = SelectedRestore { candidate, source };
        let selected = [selection.clone()];
        let complete = restore_candidates_at(uninterrupted.path(), &selected, &[], false);
        assert!(complete.errors.is_empty(), "{:?}", complete.errors);

        // Simulate process exit after rating.db committed, before later stores or ledger update.
        let paused_files = ["rotation.db", "reading_history.db"].map(|name| {
            (
                resumed.path().join(name),
                resumed.path().join(format!("{name}.paused")),
            )
        });
        for (path, paused) in &paused_files {
            std::fs::rename(path, paused).unwrap();
        }
        let mappings =
            restore_copy_mappings(resumed.path(), &selection.candidate, &selection.source);
        let partial = crate::rename_key_migration::copy_stores_at(resumed.path(), &mappings);
        assert!(partial.errors.is_empty(), "{:?}", partial.errors);
        assert_eq!(partial.rows, 2, "rating exact and virtual rows committed");
        let ledger =
            ContentIdentityDb::open_at(&resumed.path().join("content_identity.db")).unwrap();
        assert!(
            !ledger
                .ledger_entry(&target_key)
                .unwrap()
                .unwrap()
                .has_restorable_content,
            "target ledger must still be unrecorded at interruption"
        );
        drop(ledger);
        for (path, paused) in &paused_files {
            std::fs::rename(paused, path).unwrap();
        }
        let retried = restore_candidates_at(resumed.path(), &selected, &[], false);
        assert!(retried.errors.is_empty(), "{:?}", retried.errors);

        let snapshot = |data_dir: &Path| {
            let mut databases = std::collections::BTreeMap::new();
            for entry in std::fs::read_dir(data_dir).unwrap() {
                let path = entry.unwrap().path();
                if path.extension().is_none_or(|extension| extension != "db") {
                    continue;
                }
                let connection = rusqlite::Connection::open(&path).unwrap();
                let table_names = connection
                    .prepare("SELECT name FROM sqlite_master WHERE type = 'table' ORDER BY name")
                    .unwrap()
                    .query_map([], |row| row.get::<_, String>(0))
                    .unwrap()
                    .collect::<Result<Vec<_>, _>>()
                    .unwrap();
                let mut tables = std::collections::BTreeMap::new();
                for table in table_names {
                    let mut statement = connection
                        .prepare(&format!("SELECT * FROM \"{}\"", table.replace('"', "\"\"")))
                        .unwrap();
                    let column_count = statement.column_count();
                    let mut rows = statement
                        .query_map([], |row| {
                            (0..column_count)
                                .map(|column| row.get::<_, rusqlite::types::Value>(column))
                                .collect::<Result<Vec<_>, _>>()
                        })
                        .unwrap()
                        .map(|row| format!("{:?}", row.unwrap()))
                        .collect::<Vec<_>>();
                    rows.sort();
                    tables.insert(table, rows);
                }
                databases.insert(
                    path.file_name().unwrap().to_string_lossy().into_owned(),
                    tables,
                );
            }
            databases
        };
        assert_eq!(
            snapshot(uninterrupted.path()),
            snapshot(resumed.path()),
            "every DB table row, including ledger and rating source_path, must match"
        );
        for data_dir in [uninterrupted.path(), resumed.path()] {
            let ratings = rusqlite::Connection::open(data_dir.join("rating.db")).unwrap();
            for key in [&target_key, &format!("{target_key}::一.jpg")] {
                let source_path: String = ratings
                    .query_row(
                        "SELECT source_path FROM ratings WHERE path = ?1",
                        [key],
                        |row| row.get(0),
                    )
                    .unwrap();
                assert_eq!(source_path, target_key);
            }
            let rotation = rusqlite::Connection::open(data_dir.join("rotation.db")).unwrap();
            let angle: i64 = rotation
                .query_row(
                    "SELECT angle FROM rotations WHERE path = ?1",
                    [format!("{target_key}::一.jpg")],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(angle, 90, "a later store must be copied on retry");
            let history = rusqlite::Connection::open(data_dir.join("reading_history.db")).unwrap();
            let raw_path: String = history
                .query_row(
                    "SELECT path FROM reading_history WHERE key = ?1",
                    [&target_key],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(raw_path, selection.candidate.target_path.to_string_lossy());
            let ledger = ContentIdentityDb::open_at(&data_dir.join("content_identity.db")).unwrap();
            assert!(
                ledger
                    .ledger_entry(&target_key)
                    .unwrap()
                    .unwrap()
                    .has_restorable_content
            );
        }
    }

    fn sample_comic_objects() -> Vec<AnnotationObject> {
        vec![AnnotationObject::new_text(
            1,
            (10.0, 20.0),
            TextBlock {
                text: "restored annotation".to_string(),
                ..TextBlock::default()
            },
        )]
    }

    fn measure_batch_database_opens(candidate_count: usize) -> usize {
        let data = tempfile::tempdir().unwrap();
        create_all_unique_store_schemas(data.path());
        let db = ContentIdentityDb::open_at(&data.path().join("content_identity.db")).unwrap();
        let mut selected = Vec::new();
        for index in 0..candidate_count {
            let source_path = data.path().join(format!("origin-{index}.png"));
            let target_path = data.path().join(format!("target-{index}.png"));
            std::fs::write(&target_path, b"same").unwrap();
            let metadata = std::fs::metadata(&target_path).unwrap();
            let source_key = crate::path_key::normalize_keep_drive(&source_path);
            let target_key = crate::path_key::normalize_keep_drive(&target_path);
            let full_hash = format!("full-{index}");
            db.upsert(
                &ContentIdentitySource::new(&source_path, ContentKind::Image),
                &RecordedFileState {
                    file_key: source_key.clone(),
                    size: metadata.len(),
                    hashed_mtime: 1,
                },
                &format!("head-{index}"),
                &full_hash,
                10,
                ObservationRole::RestorableContent,
            )
            .unwrap();
            db.upsert(
                &ContentIdentitySource::new(&target_path, ContentKind::Image),
                &RecordedFileState {
                    file_key: target_key.clone(),
                    size: metadata.len(),
                    hashed_mtime: metadata_mtime(&metadata).unwrap(),
                },
                &format!("head-{index}"),
                &full_hash,
                0,
                ObservationRole::DetectionCache,
            )
            .unwrap();
            let (candidate, source) =
                candidate(source_path, target_path, ContentKind::Image, &full_hash);
            selected.push(SelectedRestore { candidate, source });
        }
        drop(db);
        let mut progress = Vec::new();
        let report = restore_candidates_at_with_progress(
            data.path(),
            &selected,
            &[],
            true,
            |stage, processed, total| progress.push((stage, processed, total)),
        );
        assert!(report.errors.is_empty(), "{:?}", report.errors);
        assert_eq!(report.ledger_entries.len(), candidate_count);
        for stage in ["copy", "ledger", "runtime"] {
            let events = progress
                .iter()
                .filter(|event| event.0 == stage)
                .collect::<Vec<_>>();
            assert!(!events.is_empty(), "{stage}");
            assert!(events.windows(2).all(|events| events[0].1 <= events[1].1));
            assert_eq!(
                events.last().unwrap().1,
                events.last().unwrap().2,
                "{stage}"
            );
        }
        report.database_opens
    }

    #[test]
    fn batch_restore_database_opens_are_constant_for_one_and_hundred_candidates() {
        let one = measure_batch_database_opens(1);
        let hundred = measure_batch_database_opens(100);

        assert_eq!(
            one, 33,
            "24 store copy + 1 origin batch + 8 runtime reads, including endpoint and page-alone placement"
        );
        assert_eq!(hundred, one, "DB open 回数を候補数に比例させない");
    }

    #[test]
    fn convertible_restore_copies_all_four_faces_without_archive_cache_db() {
        let data = tempfile::tempdir().unwrap();
        let old = PathBuf::from(r"C:\本\old.rar");
        let new = PathBuf::from(r"D:\移動先\new.rar");
        let old_cache = crate::archive_cache::cache_zip_path_for_data_dir(data.path(), &old);
        let new_cache = crate::archive_cache::cache_zip_path_for_data_dir(data.path(), &new);
        let old_key = crate::path_key::normalize_keep_drive(&old);
        let old_cache_key = crate::path_key::normalize_keep_drive(&old_cache);
        create_rotation_rows(
            data.path(),
            &[
                (old_key.clone(), 1),
                (format!("{old_key}::001.jpg"), 2),
                (old_cache_key.clone(), 3),
                (format!("{old_cache_key}::001.jpg"), 4),
            ],
        );
        let (candidate, source) = candidate(old, new.clone(), ContentKind::Convertible, "hash");
        let report = crate::rename_key_migration::copy_stores_at(
            data.path(),
            &restore_copy_mappings(data.path(), &candidate, &source),
        );
        assert!(report.errors.is_empty(), "{:?}", report.errors);
        assert_eq!(report.rows, 4);
        let connection = rusqlite::Connection::open(data.path().join("rotation.db")).unwrap();
        let new_key = crate::path_key::normalize_keep_drive(&new);
        let new_cache_key = crate::path_key::normalize_keep_drive(&new_cache);
        for (key, angle) in [
            (new_key.clone(), 1),
            (format!("{new_key}::001.jpg"), 2),
            (new_cache_key.clone(), 3),
            (format!("{new_cache_key}::001.jpg"), 4),
        ] {
            let copied: i64 = connection
                .query_row(
                    "SELECT angle FROM rotations WHERE path = ?1",
                    [key],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(copied, angle);
        }
        assert!(!data.path().join("archive_cache.db").exists());
    }

    #[test]
    fn converted_cache_faces_are_noop_across_drive_letters_with_same_relative_path() {
        let data = tempfile::tempdir().unwrap();
        let old = PathBuf::from(r"C:\a\x.rar");
        let new = PathBuf::from(r"D:\a\x.rar");
        let old_cache = crate::archive_cache::cache_zip_path_for_data_dir(data.path(), &old);
        let new_cache = crate::archive_cache::cache_zip_path_for_data_dir(data.path(), &new);
        assert_eq!(old_cache, new_cache, "cache hash は drive letter を落とす");
        let old_key = crate::path_key::normalize_keep_drive(&old);
        let cache_key = crate::path_key::normalize_keep_drive(&old_cache);
        create_rotation_rows(
            data.path(),
            &[
                (old_key.clone(), 1),
                (format!("{old_key}::001.jpg"), 2),
                (cache_key.clone(), 3),
                (format!("{cache_key}::001.jpg"), 4),
            ],
        );
        let (candidate, source) = candidate(old, new.clone(), ContentKind::Convertible, "hash");
        let report = crate::rename_key_migration::copy_stores_at(
            data.path(),
            &restore_copy_mappings(data.path(), &candidate, &source),
        );
        assert!(report.errors.is_empty(), "{:?}", report.errors);
        assert_eq!(
            report.rows, 2,
            "cache exact / prefix は同一 key なので no-op"
        );

        let connection = rusqlite::Connection::open(data.path().join("rotation.db")).unwrap();
        let cache_rows: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM rotations
                  WHERE path = ?1 OR substr(path, 1, ?2) = ?3",
                rusqlite::params![
                    cache_key,
                    format!("{cache_key}::").chars().count() as i64,
                    format!("{cache_key}::"),
                ],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(cache_rows, 2);
        let new_key = crate::path_key::normalize_keep_drive(&new);
        for key in [new_key.clone(), format!("{new_key}::001.jpg")] {
            assert!(
                connection
                    .query_row("SELECT 1 FROM rotations WHERE path = ?1", [key], |_| Ok(()),)
                    .is_ok()
            );
        }
    }

    #[test]
    fn restore_promotes_target_for_a2_and_prepares_in_memory_sidecar_mirror() {
        let data = tempfile::tempdir().unwrap();
        let source_path = data.path().join("origin.png");
        let target_path = data.path().join("target.png");
        std::fs::write(&target_path, b"same bytes").unwrap();
        let source_key = crate::path_key::normalize_keep_drive(&source_path);
        let target_key = crate::path_key::normalize_keep_drive(&target_path);
        let metadata = std::fs::metadata(&target_path).unwrap();
        let size = metadata.len();
        let target_mtime = metadata_mtime(&metadata).unwrap();
        let db = ContentIdentityDb::open_at(&data.path().join("content_identity.db")).unwrap();
        db.upsert(
            &ContentIdentitySource::new(&source_path, ContentKind::Image),
            &RecordedFileState {
                file_key: source_key.clone(),
                size,
                hashed_mtime: 1,
            },
            "head",
            "full",
            123,
            ObservationRole::RestorableContent,
        )
        .unwrap();
        db.upsert(
            &ContentIdentitySource::new(&target_path, ContentKind::Image),
            &RecordedFileState {
                file_key: target_key.clone(),
                size,
                hashed_mtime: target_mtime,
            },
            "head",
            "full",
            0,
            ObservationRole::DetectionCache,
        )
        .unwrap();
        drop(db);

        let adjustment =
            crate::adjustment_db::AdjustmentDb::open_at(&data.path().join("adjustment.db"))
                .unwrap();
        let mut params = crate::adjustment::AdjustParams::default();
        params.brightness = 17.0;
        adjustment.set_page_params(&source_key, &params).unwrap();
        drop(adjustment);

        let (candidate, source) =
            candidate(source_path, target_path.clone(), ContentKind::Image, "full");
        let report = restore_candidates_at(
            data.path(),
            &[SelectedRestore { candidate, source }],
            &[],
            true,
        );
        assert!(report.errors.is_empty(), "{:?}", report.errors);
        let ledger = report.ledger_entries.first().unwrap();
        assert_eq!(ledger.file_key, target_key);
        assert_eq!(ledger.last_edit_at, 123);
        assert!(ledger.has_restorable_content);
        assert_eq!(ledger.hashed_mtime, target_mtime);

        let adjustment =
            crate::adjustment_db::AdjustmentDb::open_at(&data.path().join("adjustment.db"))
                .unwrap();
        assert_eq!(adjustment.get_page_params(&target_key), Some(params));
        let db = ContentIdentityDb::open_at(&data.path().join("content_identity.db")).unwrap();
        let index = db
            .load_index(&std::sync::atomic::AtomicBool::new(false))
            .unwrap()
            .unwrap();
        assert!(
            stage0_target(
                &index,
                ContentIdentitySource::new(&target_path, ContentKind::Image),
                size,
            )
            .is_none(),
            "復元済み target は同じフォルダを開き直しても再提案しない"
        );
        let third = stage0_target(
            &index,
            ContentIdentitySource::new(data.path().join("third.png"), ContentKind::Image),
            size,
        )
        .unwrap();
        assert!(
            third
                .origins
                .iter()
                .any(|entry| entry.file_key == target_key)
        );

        assert!(report.presence.adjusted.contains(&target_key));
        assert_eq!(report.sidecar_mirrors.len(), 1);
        assert_eq!(report.sidecar_bases.len(), 1);
        assert_eq!(
            report.sidecar_bases[0].folder(),
            target_path.parent().unwrap()
        );
        let mirror = &report.sidecar_mirrors[0];
        assert_eq!(mirror.folder, target_path.parent().unwrap());
        assert_eq!(mirror.rel_key, "target.png");
        assert_eq!(
            mirror.entry.adjust.as_ref().map(|value| value.brightness),
            Some(17.0)
        );
        let mut sidecar = crate::sidecar::SidecarFile::new(mirror.folder.clone());
        sidecar.replace_edit_bundle(&mirror.rel_key, mirror.entry.clone());
        assert!(sidecar.is_dirty());
        assert!(sidecar.items().get("target.png").unwrap().adjust.is_some());
        assert!(
            !target_path
                .parent()
                .unwrap()
                .join(crate::sidecar::SIDECAR_FILENAME)
                .exists()
        );
    }

    #[test]
    fn restore_reloads_stale_empty_comic_doc_and_followup_save_preserves_row() {
        let data = tempfile::tempdir().unwrap();
        let source_path = data.path().join("origin.png");
        let target_path = data.path().join("target.png");
        std::fs::write(&source_path, b"same bytes").unwrap();
        std::fs::write(&target_path, b"same bytes").unwrap();
        let source_key = crate::path_key::normalize_keep_drive(&source_path);
        let target_key = crate::path_key::normalize_keep_drive(&target_path);
        let source_metadata = std::fs::metadata(&source_path).unwrap();
        let target_metadata = std::fs::metadata(&target_path).unwrap();

        let identity =
            ContentIdentityDb::open_at(&data.path().join("content_identity.db")).unwrap();
        identity
            .upsert(
                &ContentIdentitySource::new(&source_path, ContentKind::Image),
                &RecordedFileState {
                    file_key: source_key.clone(),
                    size: source_metadata.len(),
                    hashed_mtime: metadata_mtime(&source_metadata).unwrap(),
                },
                "head",
                "full",
                10,
                ObservationRole::RestorableContent,
            )
            .unwrap();
        identity
            .upsert(
                &ContentIdentitySource::new(&target_path, ContentKind::Image),
                &RecordedFileState {
                    file_key: target_key.clone(),
                    size: target_metadata.len(),
                    hashed_mtime: metadata_mtime(&target_metadata).unwrap(),
                },
                "head",
                "full",
                0,
                ObservationRole::DetectionCache,
            )
            .unwrap();
        drop(identity);

        let objects = sample_comic_objects();
        crate::comic_db::ComicDb::open_at(&data.path().join("comic.db"))
            .unwrap()
            .set(&source_key, &objects)
            .unwrap();
        crate::rotation_db::RotationDb::open_at(&data.path().join("rotation.db"))
            .unwrap()
            .set_key(&source_key, crate::rotation_db::Rotation::Cw90)
            .unwrap();

        let mut app = crate::app::setup_app_for_test();
        app.comic_db =
            Some(crate::comic_db::ComicDb::open_at(&data.path().join("comic.db")).unwrap());
        app.rotation_db = Some(
            crate::rotation_db::RotationDb::open_at(&data.path().join("rotation.db")).unwrap(),
        );
        app.items = vec![crate::grid_item::GridItem::Image(target_path.clone())];
        app.comic_docs.insert(target_key.clone(), Vec::new());
        app.rotation_cache
            .insert(0, crate::rotation_db::Rotation::None);

        let (candidate, source) = candidate(source_path, target_path, ContentKind::Image, "full");
        let report = restore_candidates_at(
            data.path(),
            &[SelectedRestore { candidate, source }],
            &[],
            false,
        );
        assert!(report.errors.is_empty(), "{:?}", report.errors);
        assert_eq!(app.comic_docs.get(&target_key), Some(&Vec::new()));
        assert!(report.presence.comics.contains(&target_key));
        assert!(report.presence.rotations.contains(&target_key));

        app.apply_content_restore_presence(report.presence);
        app.finish_content_identity_restore(report.errors);

        assert!(
            !app.comic_docs.contains_key(&target_key),
            "restore completion must return the stale empty sentinel to the unread state"
        );
        app.ensure_comic_doc_loaded(&target_key);
        let loaded = app.comic_docs.get(&target_key).cloned().unwrap();
        assert_eq!(loaded, objects);
        assert_eq!(app.get_rotation(0), crate::rotation_db::Rotation::Cw90);

        app.save_comic_objects(0, &target_key, &loaded);
        assert_eq!(
            app.comic_db.as_ref().unwrap().get(&target_key),
            Some(objects)
        );
    }

    #[test]
    fn restore_declined_writer_is_idempotent_and_matches_a2_reader() {
        let data = tempfile::tempdir().unwrap();
        let refusal = DeclinedRestore {
            full_hash: "full".to_string(),
            target_key: "c:/target.png".to_string(),
        };
        let first = restore_candidates_at(data.path(), &[], std::slice::from_ref(&refusal), true);
        let second = restore_candidates_at(data.path(), &[], &[refusal], true);
        assert_eq!(first.rows, 1);
        assert_eq!(second.rows, 0);
        let db = ContentIdentityDb::open_at(&data.path().join("content_identity.db")).unwrap();
        assert!(db.restore_was_declined("full", "c:/target.png").unwrap());
    }

    #[test]
    fn internal_byte_copy_recorder_reuses_ledger_hash_and_skips_unhashable_sources() {
        let data = tempfile::tempdir().unwrap();
        let source_path = data.path().join("source.png");
        let target_path = data.path().join("target.png");
        let pending_path = data.path().join("pending.png");
        let pending_target = data.path().join("pending-target.png");
        let missing_path = data.path().join("missing.png");
        let missing_target = data.path().join("missing-target.png");
        let source_key = crate::path_key::normalize_keep_drive(&source_path);
        let pending_key = crate::path_key::normalize_keep_drive(&pending_path);

        let db = ContentIdentityDb::open_at(&data.path().join("content_identity.db")).unwrap();
        db.upsert(
            &ContentIdentitySource::new(&source_path, ContentKind::Image),
            &RecordedFileState {
                file_key: source_key,
                size: 10,
                hashed_mtime: 1,
            },
            "head",
            "ledger-full",
            1,
            ObservationRole::RestorableContent,
        )
        .unwrap();
        db.conn
            .execute(
                "INSERT INTO edit_origin
                     (file_key, size, head_hash, full_hash, hashed_mtime, kind, last_edit_at,
                      has_restorable_content)
                 VALUES (?1, 10, 'head', NULL, 1, 'image', 1, 1)",
                [&pending_key],
            )
            .unwrap();
        drop(db);

        let mut recorder = InternalByteCopyDeclineRecorder::new(data.path());
        recorder.record(&source_path, &target_path);
        recorder.record(&source_path, &target_path);
        recorder.record(&pending_path, &pending_target);
        recorder.record(&missing_path, &missing_target);
        let report = recorder.finish();
        assert_eq!(report.requested, 4);
        assert_eq!(report.recorded, 1);
        assert_eq!(report.already_recorded, 1);
        assert_eq!(report.source_hash_unavailable, 1);
        assert_eq!(report.source_not_tracked, 1);
        assert!(report.errors.is_empty());

        let db = ContentIdentityDb::open_at(&data.path().join("content_identity.db")).unwrap();
        assert!(
            db.restore_was_declined(
                "ledger-full",
                &crate::path_key::normalize_keep_drive(&target_path)
            )
            .unwrap()
        );
        assert!(
            !db.restore_was_declined(
                "ledger-full",
                &crate::path_key::normalize_keep_drive(&pending_target)
            )
            .unwrap()
        );
    }
}
