//! A3a: 内容 identity が一致した物理ファイル間の永続 edit state copy。
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
}

#[derive(Debug, Default)]
pub(crate) struct ContentRestoreReport {
    pub(crate) rows: usize,
    pub(crate) errors: Vec<String>,
    pub(crate) sidecar_mirrors: Vec<RestoreSidecarMirror>,
    pub(crate) presence: RestorePresence,
    pub(crate) ledger_entry: Option<LedgerEntry>,
}

/// A3b が worker から呼ぶ復元入口。A2 の候補と選択された復元元だけを受け取り、
/// UI state に触れずに shared STORES copy と target ledger 昇格を完了する。
pub(crate) fn restore_candidate_at(
    data_dir: &Path,
    candidate: &RestoreCandidate,
    source: &RestoreSourceCandidate,
) -> ContentRestoreReport {
    let mappings = restore_copy_mappings(data_dir, candidate, source);
    let copied = crate::rename_key_migration::copy_stores_at(data_dir, &mappings);
    let mut report = ContentRestoreReport {
        rows: copied.rows,
        errors: copied.errors,
        ..ContentRestoreReport::default()
    };

    match mark_restored_origin_at(data_dir, candidate, source) {
        Ok((entry, changed)) => {
            report.rows += usize::from(changed);
            report.ledger_entry = Some(entry);
        }
        Err(error) => report.errors.push(format!("content_identity: {error}")),
    }

    match load_restore_runtime_updates(data_dir, candidate, source) {
        Ok((sidecar_mirrors, presence)) => {
            report.sidecar_mirrors = sidecar_mirrors;
            report.presence = presence;
        }
        Err(error) => report.errors.push(format!("sidecar mirror: {error}")),
    }
    report
}

/// `(full_hash, target_key)` の恒久辞退記録。A3a は API だけを提供し、操作の判断は A3b が行う。
pub(crate) fn record_restore_declined_at(
    data_dir: &Path,
    full_hash: &str,
    target_key: &str,
) -> Result<bool, String> {
    let db = ContentIdentityDb::open_at(&data_dir.join("content_identity.db"))
        .map_err(|error| error.to_string())?;
    db.conn
        .execute(
            "INSERT OR IGNORE INTO restore_declined(full_hash, target_key) VALUES (?1, ?2)",
            rusqlite::params![full_hash, target_key],
        )
        .map(|rows| rows > 0)
        .map_err(|error| error.to_string())
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

fn mark_restored_origin_at(
    data_dir: &Path,
    candidate: &RestoreCandidate,
    source: &RestoreSourceCandidate,
) -> Result<(LedgerEntry, bool), String> {
    let db = ContentIdentityDb::open_at(&data_dir.join("content_identity.db"))
        .map_err(|error| error.to_string())?;
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
) -> Result<(), String> {
    let path = data_dir.join(file);
    if !path.exists() || families.is_empty() {
        return Ok(());
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
    Ok(())
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
    candidate: &RestoreCandidate,
    source: &RestoreSourceCandidate,
) -> Result<(Vec<RestoreSidecarMirror>, RestorePresence), String> {
    let families = destination_edit_families(data_dir, candidate, source);
    let mut states = BTreeMap::<String, crate::sidecar::SidecarEntry>::new();

    query_family_rows(
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
    query_family_rows(
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
    query_family_rows(
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
    query_family_rows(
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
                Some(serde_json::from_str(&json).map_err(|error| error.to_string())?);
            Ok(())
        },
    )?;
    query_family_rows(
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
    query_family_rows(
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

    let mut presence = RestorePresence::default();
    let mut mirrors = Vec::new();
    for (key, entry) in states {
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
    Ok((mirrors, presence))
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
        let report = restore_candidate_at(data.path(), &candidate, &source);
        assert!(report.errors.is_empty(), "{:?}", report.errors);
        let ledger = report.ledger_entry.as_ref().unwrap();
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
    fn restore_declined_writer_is_idempotent_and_matches_a2_reader() {
        let data = tempfile::tempdir().unwrap();
        assert!(record_restore_declined_at(data.path(), "full", "c:/target.png").unwrap());
        assert!(!record_restore_declined_at(data.path(), "full", "c:/target.png").unwrap());
        let db = ContentIdentityDb::open_at(&data.path().join("content_identity.db")).unwrap();
        assert!(db.restore_was_declined("full", "c:/target.png").unwrap());
    }
}
