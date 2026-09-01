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
pub(crate) fn restore_candidates_at(
    data_dir: &Path,
    selected: &[SelectedRestore],
    declined: &[DeclinedRestore],
    load_sidecar_bases: bool,
) -> ContentRestoreReport {
    let mappings = selected
        .iter()
        .flat_map(|selection| {
            restore_copy_mappings(data_dir, &selection.candidate, &selection.source)
        })
        .collect::<Vec<_>>();
    let copied = crate::rename_key_migration::copy_stores_at(data_dir, &mappings);
    let mut report = ContentRestoreReport {
        requested_restores: selected.len(),
        requested_declines: declined.len(),
        rows: copied.rows,
        database_opens: copied.database_opens,
        errors: copied.errors,
        ..ContentRestoreReport::default()
    };

    apply_batch_ledger_updates(data_dir, selected, declined, &mut report);

    match load_restore_runtime_updates(data_dir, selected) {
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
    selected: &[SelectedRestore],
    declined: &[DeclinedRestore],
    report: &mut ContentRestoreReport,
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
    for selection in selected {
        match mark_restored_origin(&db, &selection.candidate, &selection.source) {
            Ok((entry, changed)) => {
                report.rows += usize::from(changed);
                report.ledger_entries.push(entry);
            }
            Err(error) => report.errors.push(format!(
                "content_identity target={}: {error}",
                selection.candidate.target_path.display()
            )),
        }
    }
    for refusal in declined {
        match record_restore_declined(&db, refusal) {
            Ok(changed) => report.rows += usize::from(changed),
            Err(error) => report.errors.push(format!(
                "content_identity target={}: {error}",
                refusal.target_key
            )),
        }
    }
}

fn record_restore_declined(
    db: &ContentIdentityDb,
    refusal: &DeclinedRestore,
) -> Result<bool, String> {
    db.conn
        .execute(
            "INSERT OR IGNORE INTO restore_declined(full_hash, target_key) VALUES (?1, ?2)",
            rusqlite::params![refusal.full_hash, refusal.target_key],
        )
        .map(|rows| rows > 0)
        .map_err(|error| error.to_string())
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
    let mut mappings = vec![
        StoreCopyPathMapping::exact(&source.path, &candidate.target_path),
        StoreCopyPathMapping::virtual_prefix(&source.path, &candidate.target_path),
    ];
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
    let sql = format!(
        "SELECT {key_column}, {selected_columns} FROM {table}
          WHERE {key_column} = ?1 OR substr({key_column}, 1, ?2) = ?3"
    );
    let mut statement = conn.prepare(&sql).map_err(|error| error.to_string())?;
    for family in families {
        let prefix = format!("{}::", family.base_key);
        let mut rows = statement
            .query(rusqlite::params![
                family.base_key,
                prefix.chars().count() as i64,
                prefix,
            ])
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
    selected: &[SelectedRestore],
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
        let report = restore_candidates_at(data.path(), &selected, &[], true);
        assert!(report.errors.is_empty(), "{:?}", report.errors);
        assert_eq!(report.ledger_entries.len(), candidate_count);
        report.database_opens
    }

    #[test]
    fn batch_restore_database_opens_are_constant_for_one_and_hundred_candidates() {
        let one = measure_batch_database_opens(1);
        let hundred = measure_batch_database_opens(100);

        assert_eq!(one, 29, "21 store copy + 1 origin batch + 7 runtime reads");
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
