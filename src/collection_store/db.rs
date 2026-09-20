use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rusqlite::{Connection, OptionalExtension, Transaction, params};
use uuid::Uuid;

use super::{
    CollectionBatchAddOutcome, CollectionCatalogSnapshot, CollectionDefinition, CollectionEntry,
    CollectionEntryId, CollectionId, CollectionMigrationOutcome, CollectionOrderMode,
    CollectionRegistration, CollectionResolvedKind, CollectionSourceMigration,
    CollectionSourceMigrationBatch, CollectionSourceNamespace, CollectionSourcePathKey,
    CollectionStoreError, MAX_COLLECTION_ENTRIES,
};
use crate::settings::SortOrder;

const SCHEMA_VERSION: u32 = 2;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CollectionDbStartup {
    New,
    ExistingUnversioned,
    ExistingV1,
    ExistingV2,
}

impl CollectionDbStartup {
    fn expected_version(self) -> u32 {
        match self {
            Self::New | Self::ExistingUnversioned => 0,
            Self::ExistingV1 => 1,
            Self::ExistingV2 => SCHEMA_VERSION,
        }
    }
}

pub(super) struct CollectionStoreDb {
    conn: Connection,
}

impl CollectionStoreDb {
    pub(super) fn open_at(path: &Path) -> Result<Self, CollectionStoreError> {
        let existed = path
            .try_exists()
            .map_err(|error| CollectionStoreError::Persistence(error.to_string()))?;
        let startup = if existed {
            let probe = Connection::open_with_flags(
                path,
                rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_URI,
            )?;
            let version: u32 = probe.pragma_query_value(None, "user_version", |row| row.get(0))?;
            match version {
                0 => CollectionDbStartup::ExistingUnversioned,
                1 => {
                    validate_existing_v1(&probe)?;
                    CollectionDbStartup::ExistingV1
                }
                SCHEMA_VERSION => {
                    validate_existing_v2(&probe)?;
                    CollectionDbStartup::ExistingV2
                }
                newer => return Err(CollectionStoreError::IncompatibleSchema(newer)),
            }
        } else {
            CollectionDbStartup::New
        };
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|error| CollectionStoreError::Persistence(error.to_string()))?;
        }
        let conn = Connection::open(path)?;
        conn.busy_timeout(Duration::from_secs(3))?;
        let version: u32 = conn.pragma_query_value(None, "user_version", |row| row.get(0))?;
        if version > SCHEMA_VERSION {
            return Err(CollectionStoreError::IncompatibleSchema(version));
        }
        if version != startup.expected_version() {
            return Err(CollectionStoreError::Persistence(
                "collection schema changed during startup".into(),
            ));
        }
        conn.pragma_update(None, "foreign_keys", true)?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        match startup {
            CollectionDbStartup::New | CollectionDbStartup::ExistingUnversioned => {
                initialize_schema(&conn)?;
            }
            CollectionDbStartup::ExistingV1 => {
                // VACUUM INTO includes committed WAL rows. Keep a complete v1 snapshot before
                // the first schema write; a backup failure must leave the original untouched.
                let db_file_name =
                    path.file_name()
                        .and_then(|name| name.to_str())
                        .ok_or_else(|| {
                            CollectionStoreError::Persistence(
                                "collection database has no filename".into(),
                            )
                        })?;
                crate::db_backup::rotate_generation_backups(
                    path.parent().unwrap_or_else(|| Path::new(".")),
                    db_file_name,
                    &|message| eprintln!("{message}"),
                    &|destination| {
                        conn.execute("VACUUM INTO ?1", [destination.to_string_lossy().as_ref()])
                            .map(|_| ())
                            .map_err(|error| error.to_string())
                    },
                )
                .map_err(CollectionStoreError::Persistence)?;
                migrate_v1_to_v2(&conn)?;
            }
            CollectionDbStartup::ExistingV2 => {
                // This is the only normal-startup rotation. A newly created DB and the
                // v1 migration above must not displace an older known-good generation.
                let db_file_name =
                    path.file_name()
                        .and_then(|name| name.to_str())
                        .ok_or_else(|| {
                            CollectionStoreError::Persistence(
                                "collection database has no filename".into(),
                            )
                        })?;
                if let Err(error) = crate::db_backup::rotate_generation_backups(
                    path.parent().unwrap_or_else(|| Path::new(".")),
                    db_file_name,
                    &|message| eprintln!("{message}"),
                    &|destination| {
                        conn.execute("VACUUM INTO ?1", [destination.to_string_lossy().as_ref()])
                            .map(|_| ())
                            .map_err(|error| error.to_string())
                    },
                ) {
                    eprintln!(
                        "collection startup backup failed; continuing with validated database: {error}"
                    );
                }
            }
        }
        Ok(Self { conn })
    }

    pub(super) fn catalog(&self) -> Result<CollectionCatalogSnapshot, CollectionStoreError> {
        load_catalog(&self.conn)
    }

    pub(super) fn snapshot(
        &self,
        id: CollectionId,
    ) -> Result<super::CollectionSnapshot, CollectionStoreError> {
        load_snapshot(&self.conn, id)
    }

    pub(super) fn export_all_snapshot(
        &self,
        cancel: &AtomicBool,
        progress: &(AtomicUsize, AtomicUsize),
    ) -> Result<super::CollectionAllExportSnapshot, CollectionStoreError> {
        let tx = self.conn.unchecked_transaction()?;
        let catalog = load_catalog(&tx)?;
        progress
            .1
            .store(catalog.definitions.len(), Ordering::Release);
        let mut snapshots = Vec::with_capacity(catalog.definitions.len());
        for definition in catalog.definitions.iter() {
            if cancel.load(Ordering::Acquire) {
                return Err(CollectionStoreError::Cancelled);
            }
            snapshots.push(load_snapshot_with_catalog_cancel(
                &tx,
                definition.id,
                catalog.catalog_revision,
                Some(cancel),
            )?);
            progress.0.fetch_add(1, Ordering::Release);
        }
        if cancel.load(Ordering::Acquire) {
            return Err(CollectionStoreError::Cancelled);
        }
        tx.commit()?;
        Ok(super::CollectionAllExportSnapshot { catalog, snapshots })
    }

    pub(super) fn create_collection(
        &mut self,
        name: &str,
    ) -> Result<super::CollectionSnapshot, CollectionStoreError> {
        self.create_collection_with_id(CollectionId::new(), name)
    }

    fn create_collection_with_id(
        &mut self,
        id: CollectionId,
        name: &str,
    ) -> Result<super::CollectionSnapshot, CollectionStoreError> {
        let name = normalized_name(name)?;
        let tx = self.conn.transaction()?;
        let position: i64 = tx.query_row(
            "SELECT COALESCE(MAX(catalog_position) + 1, 0) FROM collections",
            [],
            |row| row.get(0),
        )?;
        let now = now_ms();
        tx.execute(
            "INSERT INTO collections
             (id, name, order_mode, sort_order, revision, catalog_position, created_at_ms, updated_at_ms)
             VALUES (?1, ?2, 'manual', 'file_name', 1, ?3, ?4, ?4)",
            params![id.to_string(), name, position, now],
        )?;
        let catalog_revision = bump_catalog_revision(&tx)?;
        let snapshot = load_snapshot_with_catalog(&tx, id, catalog_revision)?;
        tx.commit()?;
        Ok(snapshot)
    }

    pub(super) fn rename_collection(
        &mut self,
        id: CollectionId,
        expected_revision: u64,
        name: &str,
    ) -> Result<super::CollectionSnapshot, CollectionStoreError> {
        let name = normalized_name(name)?;
        let tx = self.conn.transaction()?;
        let definition = load_definition(&tx, id)?;
        require_revision(expected_revision, definition.revision)?;
        if definition.name == name {
            let snapshot = load_snapshot_with_catalog(&tx, id, catalog_revision(&tx)?)?;
            tx.commit()?;
            return Ok(snapshot);
        }
        let revision = increment_collection_revision(&tx, id)?;
        tx.execute(
            "UPDATE collections SET name = ?1, updated_at_ms = ?2 WHERE id = ?3",
            params![name, now_ms(), id.to_string()],
        )?;
        let catalog_revision = bump_catalog_revision(&tx)?;
        let snapshot = load_snapshot_with_catalog(&tx, id, catalog_revision)?;
        debug_assert_eq!(snapshot.revision(), revision);
        tx.commit()?;
        Ok(snapshot)
    }

    pub(super) fn delete_collection(
        &mut self,
        id: CollectionId,
        expected_revision: u64,
    ) -> Result<CollectionCatalogSnapshot, CollectionStoreError> {
        let tx = self.conn.transaction()?;
        let definition = load_definition(&tx, id)?;
        require_revision(expected_revision, definition.revision)?;
        tx.execute("DELETE FROM collections WHERE id = ?1", [id.to_string()])?;
        compact_catalog_positions(&tx)?;
        let revision = bump_catalog_revision(&tx)?;
        let catalog = load_catalog_with_revision(&tx, revision)?;
        tx.commit()?;
        Ok(catalog)
    }

    pub(super) fn set_order(
        &mut self,
        id: CollectionId,
        expected_revision: u64,
        mode: CollectionOrderMode,
        standard_sort: SortOrder,
    ) -> Result<super::CollectionSnapshot, CollectionStoreError> {
        let tx = self.conn.transaction()?;
        let definition = load_definition(&tx, id)?;
        require_revision(expected_revision, definition.revision)?;
        if mode != CollectionOrderMode::Shuffle
            && definition.order_mode == mode
            && definition.standard_sort == standard_sort
        {
            let snapshot = load_snapshot_with_catalog(&tx, id, catalog_revision(&tx)?)?;
            tx.commit()?;
            return Ok(snapshot);
        }
        tx.execute(
            "UPDATE collections
             SET order_mode = ?1, sort_order = ?2, shuffle_seed = ?3,
                 revision = revision + 1, updated_at_ms = ?4
             WHERE id = ?5",
            params![
                mode.as_str(),
                sort_order_as_str(standard_sort),
                if mode == CollectionOrderMode::Shuffle {
                    format!("{:016x}", new_shuffle_seed())
                } else {
                    format!("{:016x}", definition.shuffle_seed)
                },
                now_ms(),
                id.to_string()
            ],
        )?;
        let catalog_revision = bump_catalog_revision(&tx)?;
        let snapshot = load_snapshot_with_catalog(&tx, id, catalog_revision)?;
        tx.commit()?;
        Ok(snapshot)
    }

    pub(super) fn add_batch(
        &mut self,
        id: CollectionId,
        expected_revision: u64,
        registrations: Vec<CollectionRegistration>,
    ) -> Result<CollectionBatchAddOutcome, CollectionStoreError> {
        let tx = self.conn.transaction()?;
        let definition = load_definition(&tx, id)?;
        require_revision(expected_revision, definition.revision)?;
        let mut position: i64 = tx.query_row(
            "SELECT COALESCE(MAX(manual_position) + 1, 0)
             FROM collection_entries WHERE collection_id = ?1",
            [id.to_string()],
            |row| row.get(0),
        )?;
        let existing_count: i64 = tx.query_row(
            "SELECT COUNT(*) FROM collection_entries WHERE collection_id = ?1",
            [id.to_string()],
            |row| row.get(0),
        )?;
        let mut remaining = MAX_COLLECTION_ENTRIES.saturating_sub(existing_count as usize);
        let mut added = Vec::new();
        let mut duplicates = Vec::new();
        let mut capacity_rejected = Vec::new();
        let now = now_ms();
        let mut batch_seen = HashSet::new();
        for registration in registrations {
            if !batch_seen.insert(registration.source_key.clone())
                || source_exists(&tx, id, &registration.source_key)?
            {
                duplicates.push(registration.source_key);
                continue;
            }
            if remaining == 0 {
                capacity_rejected.push(registration.source_key);
                continue;
            }
            let entry_id = CollectionEntryId::new();
            tx.execute(
                "INSERT INTO collection_entries
                 (id, collection_id, source_namespace, source_path, normalized_path,
                  resolved_kind, manual_position, created_at_ms)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    entry_id.to_string(),
                    id.to_string(),
                    registration.source_key.namespace().as_str(),
                    registration.source_path.to_string_lossy(),
                    registration.source_key.normalized_path(),
                    registration.resolved_kind.as_str(),
                    position,
                    now,
                ],
            )?;
            position += 1;
            remaining -= 1;
            added.push(entry_id);
        }
        let catalog_revision = if added.is_empty() {
            catalog_revision(&tx)?
        } else {
            increment_collection_revision(&tx, id)?;
            bump_catalog_revision(&tx)?
        };
        let snapshot = load_snapshot_with_catalog(&tx, id, catalog_revision)?;
        tx.commit()?;
        Ok(CollectionBatchAddOutcome {
            snapshot,
            added: Arc::from(added),
            duplicates: Arc::from(duplicates),
            capacity_rejected: Arc::from(capacity_rejected),
        })
    }

    pub(super) fn remove_entries(
        &mut self,
        id: CollectionId,
        expected_revision: u64,
        entry_ids: Vec<CollectionEntryId>,
    ) -> Result<super::CollectionSnapshot, CollectionStoreError> {
        let tx = self.conn.transaction()?;
        let definition = load_definition(&tx, id)?;
        require_revision(expected_revision, definition.revision)?;
        let mut removed = 0;
        let mut seen = HashSet::new();
        for entry_id in entry_ids {
            if seen.insert(entry_id) {
                removed += tx.execute(
                    "DELETE FROM collection_entries WHERE collection_id = ?1 AND id = ?2",
                    params![id.to_string(), entry_id.to_string()],
                )?;
            }
        }
        let catalog_revision = if removed == 0 {
            catalog_revision(&tx)?
        } else {
            compact_manual_positions(&tx, id)?;
            increment_collection_revision(&tx, id)?;
            bump_catalog_revision(&tx)?
        };
        let snapshot = load_snapshot_with_catalog(&tx, id, catalog_revision)?;
        tx.commit()?;
        Ok(snapshot)
    }

    pub(super) fn reorder_manual(
        &mut self,
        id: CollectionId,
        expected_revision: u64,
        order: Vec<CollectionEntryId>,
    ) -> Result<super::CollectionSnapshot, CollectionStoreError> {
        let tx = self.conn.transaction()?;
        let definition = load_definition(&tx, id)?;
        require_revision(expected_revision, definition.revision)?;
        if definition.order_mode != CollectionOrderMode::Manual {
            return Err(CollectionStoreError::ManualOrderInactive);
        }
        let current = entry_ids_in_manual_order(&tx, id)?;
        if !same_ids_exactly_once(&current, &order) {
            return Err(CollectionStoreError::InvalidOrder);
        }
        if current == order {
            let snapshot = load_snapshot_with_catalog(&tx, id, catalog_revision(&tx)?)?;
            tx.commit()?;
            return Ok(snapshot);
        }
        tx.execute(
            "UPDATE collection_entries SET manual_position = -manual_position - 1
             WHERE collection_id = ?1",
            [id.to_string()],
        )?;
        for (position, entry_id) in order.iter().enumerate() {
            tx.execute(
                "UPDATE collection_entries SET manual_position = ?1
                 WHERE collection_id = ?2 AND id = ?3",
                params![position as i64, id.to_string(), entry_id.to_string()],
            )?;
        }
        increment_collection_revision(&tx, id)?;
        let catalog_revision = bump_catalog_revision(&tx)?;
        let snapshot = load_snapshot_with_catalog(&tx, id, catalog_revision)?;
        tx.commit()?;
        Ok(snapshot)
    }

    pub(super) fn relink(
        &mut self,
        id: CollectionId,
        expected_revision: u64,
        entry_id: CollectionEntryId,
        registration: CollectionRegistration,
    ) -> Result<super::CollectionSnapshot, CollectionStoreError> {
        let tx = self.conn.transaction()?;
        let definition = load_definition(&tx, id)?;
        require_revision(expected_revision, definition.revision)?;
        let current = load_entry(&tx, id, entry_id)?;
        if current.source_path == registration.source_path
            && current.source_key == registration.source_key
            && current.resolved_kind == registration.resolved_kind
        {
            let snapshot = load_snapshot_with_catalog(&tx, id, catalog_revision(&tx)?)?;
            tx.commit()?;
            return Ok(snapshot);
        }
        if source_exists_except(&tx, id, entry_id, &registration.source_key)? {
            return Err(CollectionStoreError::DuplicateSource(
                registration.source_key,
            ));
        }
        tx.execute(
            "UPDATE collection_entries
             SET source_namespace = ?1, source_path = ?2, normalized_path = ?3, resolved_kind = ?4
             WHERE collection_id = ?5 AND id = ?6",
            params![
                registration.source_key.namespace().as_str(),
                registration.source_path.to_string_lossy(),
                registration.source_key.normalized_path(),
                registration.resolved_kind.as_str(),
                id.to_string(),
                entry_id.to_string(),
            ],
        )?;
        increment_collection_revision(&tx, id)?;
        let catalog_revision = bump_catalog_revision(&tx)?;
        let snapshot = load_snapshot_with_catalog(&tx, id, catalog_revision)?;
        tx.commit()?;
        Ok(snapshot)
    }

    pub(super) fn migrate_sources(
        &mut self,
        migration: CollectionSourceMigration,
    ) -> Result<CollectionMigrationOutcome, CollectionStoreError> {
        self.migrate_source_batch(CollectionSourceMigrationBatch {
            migrations: vec![migration],
        })
    }

    pub(super) fn migrate_source_batch(
        &mut self,
        batch: CollectionSourceMigrationBatch,
    ) -> Result<CollectionMigrationOutcome, CollectionStoreError> {
        let tx = self.conn.transaction()?;
        let entries = load_all_entries(&tx)?;
        let mut replacements = Vec::new();
        let mut resulting_keys: HashMap<CollectionId, HashSet<CollectionSourcePathKey>> =
            HashMap::new();

        for entry in &entries {
            let mut replacement = None;
            for migration in &batch.migrations {
                let candidate = migration.replacement_for(&entry.source_path, &entry.source_key)?;
                if candidate.is_some() && replacement.is_some() {
                    return Err(CollectionStoreError::InvalidPath(
                        "collection source migration mappings overlap".into(),
                    ));
                }
                if candidate.is_some() {
                    replacement = candidate;
                }
            }
            let key = replacement
                .as_ref()
                .map_or_else(|| entry.source_key.clone(), |source| source.key().clone());
            if !resulting_keys
                .entry(entry.collection_id)
                .or_default()
                .insert(key.clone())
            {
                return Err(CollectionStoreError::DuplicateSource(key));
            }
            if let Some(replacement) = replacement {
                replacements.push((entry.id, entry.collection_id, replacement));
            }
        }

        if replacements.is_empty() {
            let catalog_revision = catalog_revision(&tx)?;
            tx.commit()?;
            return Ok(CollectionMigrationOutcome {
                catalog_revision,
                affected: Arc::from([]),
                updated_entries: 0,
            });
        }
        let mut affected = HashSet::new();
        // A batch may exchange two keys in the same collection. Validation above proves that the
        // final key set is unique, but updating rows directly would still collide with the other
        // row's old UNIQUE key. Move every affected row into a transaction-local namespace first;
        // these values can never commit because every following error rolls the transaction back.
        for (entry_id, _, _) in &replacements {
            tx.execute(
                "UPDATE collection_entries
                 SET source_namespace = '_migration', normalized_path = ?1
                 WHERE id = ?2",
                params![entry_id.to_string(), entry_id.to_string()],
            )?;
        }
        for (entry_id, collection_id, replacement) in &replacements {
            tx.execute(
                "UPDATE collection_entries
                 SET source_namespace = ?1, source_path = ?2, normalized_path = ?3
                 WHERE id = ?4",
                params![
                    replacement.key().namespace().as_str(),
                    replacement.path().to_string_lossy(),
                    replacement.key().normalized_path(),
                    entry_id.to_string(),
                ],
            )?;
            affected.insert(*collection_id);
        }
        let mut revisions = Vec::with_capacity(affected.len());
        for id in affected {
            revisions.push((id, increment_collection_revision(&tx, id)?));
        }
        revisions.sort_by_key(|(id, _)| id.as_uuid());
        let catalog_revision = bump_catalog_revision(&tx)?;
        let updated_entries = replacements.len();
        tx.commit()?;
        Ok(CollectionMigrationOutcome {
            catalog_revision,
            affected: Arc::from(revisions),
            updated_entries,
        })
    }
}

fn validate_integrity(conn: &Connection) -> Result<(), CollectionStoreError> {
    let integrity: String = conn.query_row("PRAGMA integrity_check(1)", [], |row| row.get(0))?;
    if integrity != "ok" {
        return Err(CollectionStoreError::Persistence(format!(
            "collection database integrity check failed: {integrity}"
        )));
    }
    Ok(())
}

fn validate_existing_v1(conn: &Connection) -> Result<(), CollectionStoreError> {
    validate_integrity(conn)?;
    catalog_revision(conn)?;
    let mut collections = conn.prepare(
        "SELECT id, name, order_mode, sort_order, revision, '0000000000000000'
         FROM collections ORDER BY catalog_position ASC",
    )?;
    for row in collections.query_map([], read_definition_row)? {
        row?;
    }
    validate_entries_and_foreign_keys(conn)
}

fn validate_existing_v2(conn: &Connection) -> Result<(), CollectionStoreError> {
    validate_integrity(conn)?;
    // Read-only probing includes committed WAL rows. Validate every row before displacing an
    // older known-good generation; neither SQLite integrity_check nor a catalog query checks FK.
    load_catalog(conn)?;
    validate_entries_and_foreign_keys(conn)
}

fn validate_entries_and_foreign_keys(conn: &Connection) -> Result<(), CollectionStoreError> {
    let mut entries = conn.prepare(
        "SELECT id, collection_id, source_namespace, source_path, normalized_path,
                resolved_kind, manual_position
         FROM collection_entries ORDER BY collection_id, manual_position",
    )?;
    for row in entries.query_map([], read_entry_row)? {
        row?;
    }
    let mut foreign_keys = conn.prepare("PRAGMA foreign_key_check")?;
    if foreign_keys.query([])?.next()?.is_some() {
        return Err(CollectionStoreError::Persistence(
            "collection database foreign key check failed".into(),
        ));
    }
    Ok(())
}

fn initialize_schema(conn: &Connection) -> Result<(), CollectionStoreError> {
    let tx = conn.unchecked_transaction()?;
    tx.execute_batch(
        "CREATE TABLE collection_meta (
            singleton INTEGER PRIMARY KEY CHECK(singleton = 1),
            catalog_revision INTEGER NOT NULL
         );
         INSERT INTO collection_meta(singleton, catalog_revision) VALUES (1, 0);
         CREATE TABLE collections (
            id TEXT PRIMARY KEY,
            name TEXT NOT NULL,
            order_mode TEXT NOT NULL,
            sort_order TEXT NOT NULL,
            shuffle_seed TEXT NOT NULL DEFAULT '0000000000000000',
            revision INTEGER NOT NULL,
            catalog_position INTEGER NOT NULL UNIQUE,
            created_at_ms INTEGER NOT NULL,
            updated_at_ms INTEGER NOT NULL
         );
         CREATE TABLE collection_entries (
            id TEXT PRIMARY KEY,
            collection_id TEXT NOT NULL REFERENCES collections(id) ON DELETE CASCADE,
            source_namespace TEXT NOT NULL,
            source_path TEXT NOT NULL,
            normalized_path TEXT NOT NULL,
            resolved_kind TEXT NOT NULL,
            manual_position INTEGER NOT NULL,
            created_at_ms INTEGER NOT NULL,
            UNIQUE(collection_id, source_namespace, normalized_path),
            UNIQUE(collection_id, manual_position)
         );
         CREATE INDEX idx_collection_entries_collection
            ON collection_entries(collection_id, manual_position);
         PRAGMA user_version = 2;",
    )?;
    tx.commit()?;
    Ok(())
}

fn migrate_v1_to_v2(conn: &Connection) -> Result<(), CollectionStoreError> {
    let tx = conn.unchecked_transaction()?;
    tx.execute_batch(
        "ALTER TABLE collections ADD COLUMN shuffle_seed TEXT NOT NULL DEFAULT '0000000000000000';
         PRAGMA user_version = 2;",
    )?;
    tx.commit()?;
    Ok(())
}

fn new_shuffle_seed() -> u64 {
    let bytes = Uuid::new_v4().into_bytes();
    u64::from_le_bytes(bytes[..8].try_into().expect("UUID has eight leading bytes"))
}

fn load_catalog(conn: &Connection) -> Result<CollectionCatalogSnapshot, CollectionStoreError> {
    load_catalog_with_revision(conn, catalog_revision(conn)?)
}

fn load_catalog_with_revision(
    conn: &Connection,
    revision: u64,
) -> Result<CollectionCatalogSnapshot, CollectionStoreError> {
    let mut statement = conn.prepare(
        "SELECT id, name, order_mode, sort_order, revision, shuffle_seed
         FROM collections ORDER BY catalog_position ASC",
    )?;
    let rows = statement.query_map([], read_definition_row)?;
    let mut definitions = Vec::new();
    for row in rows {
        definitions.push(row?);
    }
    Ok(CollectionCatalogSnapshot {
        catalog_revision: revision,
        definitions: Arc::from(definitions),
    })
}

fn load_snapshot(
    conn: &Connection,
    id: CollectionId,
) -> Result<super::CollectionSnapshot, CollectionStoreError> {
    load_snapshot_with_catalog(conn, id, catalog_revision(conn)?)
}

fn load_snapshot_with_catalog(
    conn: &Connection,
    id: CollectionId,
    catalog_revision: u64,
) -> Result<super::CollectionSnapshot, CollectionStoreError> {
    load_snapshot_with_catalog_cancel(conn, id, catalog_revision, None)
}

fn load_snapshot_with_catalog_cancel(
    conn: &Connection,
    id: CollectionId,
    catalog_revision: u64,
    cancel: Option<&AtomicBool>,
) -> Result<super::CollectionSnapshot, CollectionStoreError> {
    let definition = load_definition(conn, id)?;
    let mut statement = conn.prepare(
        "SELECT id, collection_id, source_namespace, source_path, normalized_path,
                resolved_kind, manual_position
         FROM collection_entries WHERE collection_id = ?1
         ORDER BY manual_position ASC",
    )?;
    let rows = statement.query_map([id.to_string()], read_entry_row)?;
    let mut entries = Vec::new();
    for row in rows {
        if cancel.is_some_and(|cancel| cancel.load(Ordering::Acquire)) {
            return Err(CollectionStoreError::Cancelled);
        }
        entries.push(row?);
    }
    Ok(super::CollectionSnapshot {
        catalog_revision,
        definition,
        entries: Arc::from(entries),
    })
}

fn load_definition(
    conn: &Connection,
    id: CollectionId,
) -> Result<CollectionDefinition, CollectionStoreError> {
    conn.query_row(
        "SELECT id, name, order_mode, sort_order, revision, shuffle_seed FROM collections WHERE id = ?1",
        [id.to_string()],
        read_definition_row,
    )
    .optional()?
    .ok_or(CollectionStoreError::NotFound)
}

fn load_entry(
    conn: &Connection,
    collection_id: CollectionId,
    entry_id: CollectionEntryId,
) -> Result<CollectionEntry, CollectionStoreError> {
    conn.query_row(
        "SELECT id, collection_id, source_namespace, source_path, normalized_path,
                resolved_kind, manual_position
         FROM collection_entries WHERE collection_id = ?1 AND id = ?2",
        params![collection_id.to_string(), entry_id.to_string()],
        read_entry_row,
    )
    .optional()?
    .ok_or(CollectionStoreError::NotFound)
}

fn load_all_entries(conn: &Connection) -> Result<Vec<CollectionEntry>, CollectionStoreError> {
    let mut statement = conn.prepare(
        "SELECT id, collection_id, source_namespace, source_path, normalized_path,
                resolved_kind, manual_position
         FROM collection_entries ORDER BY collection_id, manual_position",
    )?;
    let rows = statement.query_map([], read_entry_row)?;
    let mut entries = Vec::new();
    for row in rows {
        entries.push(row?);
    }
    Ok(entries)
}

fn read_definition_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<CollectionDefinition> {
    let id: String = row.get(0)?;
    let order_mode: String = row.get(2)?;
    let sort_order: String = row.get(3)?;
    let revision: i64 = row.get(4)?;
    let seed: String = row.get(5)?;
    Ok(CollectionDefinition {
        id: CollectionId::from_uuid(parse_uuid_sql(&id)?),
        name: row.get(1)?,
        order_mode: CollectionOrderMode::from_str(&order_mode).ok_or_else(|| {
            rusqlite::Error::FromSqlConversionFailure(
                2,
                rusqlite::types::Type::Text,
                format!("unknown collection order mode {order_mode}").into(),
            )
        })?,
        standard_sort: sort_order_from_str(&sort_order).ok_or_else(|| {
            rusqlite::Error::FromSqlConversionFailure(
                3,
                rusqlite::types::Type::Text,
                format!("unknown collection sort order {sort_order}").into(),
            )
        })?,
        revision: checked_u64_sql(revision, 4)?,
        shuffle_seed: u64::from_str_radix(&seed, 16).map_err(|error| {
            rusqlite::Error::FromSqlConversionFailure(
                5,
                rusqlite::types::Type::Text,
                Box::new(error),
            )
        })?,
    })
}

fn read_entry_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<CollectionEntry> {
    let id: String = row.get(0)?;
    let collection_id: String = row.get(1)?;
    let namespace: String = row.get(2)?;
    let kind: String = row.get(5)?;
    let manual_position: i64 = row.get(6)?;
    Ok(CollectionEntry {
        id: CollectionEntryId::from_uuid(parse_uuid_sql(&id)?),
        collection_id: CollectionId::from_uuid(parse_uuid_sql(&collection_id)?),
        source_path: PathBuf::from(row.get::<_, String>(3)?),
        source_key: CollectionSourcePathKey {
            namespace: CollectionSourceNamespace::from_str(&namespace).ok_or_else(|| {
                rusqlite::Error::FromSqlConversionFailure(
                    2,
                    rusqlite::types::Type::Text,
                    format!("unknown collection source namespace {namespace}").into(),
                )
            })?,
            normalized_path: row.get(4)?,
        },
        resolved_kind: CollectionResolvedKind::from_str(&kind).ok_or_else(|| {
            rusqlite::Error::FromSqlConversionFailure(
                5,
                rusqlite::types::Type::Text,
                format!("unknown collection resolved kind {kind}").into(),
            )
        })?,
        manual_position: checked_u64_sql(manual_position, 6)?,
    })
}

fn catalog_revision(conn: &Connection) -> Result<u64, CollectionStoreError> {
    let value: i64 = conn.query_row(
        "SELECT catalog_revision FROM collection_meta WHERE singleton = 1",
        [],
        |row| row.get(0),
    )?;
    u64::try_from(value)
        .map_err(|_| CollectionStoreError::Persistence("negative catalog revision".into()))
}

fn bump_catalog_revision(tx: &Transaction<'_>) -> Result<u64, CollectionStoreError> {
    tx.execute(
        "UPDATE collection_meta SET catalog_revision = catalog_revision + 1 WHERE singleton = 1",
        [],
    )?;
    catalog_revision(tx)
}

fn increment_collection_revision(
    tx: &Transaction<'_>,
    id: CollectionId,
) -> Result<u64, CollectionStoreError> {
    let changed = tx.execute(
        "UPDATE collections SET revision = revision + 1, updated_at_ms = ?1 WHERE id = ?2",
        params![now_ms(), id.to_string()],
    )?;
    if changed == 0 {
        return Err(CollectionStoreError::NotFound);
    }
    Ok(load_definition(tx, id)?.revision)
}

fn source_exists(
    conn: &Connection,
    collection_id: CollectionId,
    key: &CollectionSourcePathKey,
) -> Result<bool, CollectionStoreError> {
    let exists: Option<i64> = conn
        .query_row(
            "SELECT 1 FROM collection_entries
             WHERE collection_id = ?1 AND source_namespace = ?2 AND normalized_path = ?3",
            params![
                collection_id.to_string(),
                key.namespace().as_str(),
                key.normalized_path()
            ],
            |row| row.get(0),
        )
        .optional()?;
    Ok(exists.is_some())
}

fn source_exists_except(
    conn: &Connection,
    collection_id: CollectionId,
    entry_id: CollectionEntryId,
    key: &CollectionSourcePathKey,
) -> Result<bool, CollectionStoreError> {
    let exists: Option<i64> = conn
        .query_row(
            "SELECT 1 FROM collection_entries
             WHERE collection_id = ?1 AND id <> ?2
               AND source_namespace = ?3 AND normalized_path = ?4",
            params![
                collection_id.to_string(),
                entry_id.to_string(),
                key.namespace().as_str(),
                key.normalized_path()
            ],
            |row| row.get(0),
        )
        .optional()?;
    Ok(exists.is_some())
}

fn entry_ids_in_manual_order(
    conn: &Connection,
    id: CollectionId,
) -> Result<Vec<CollectionEntryId>, CollectionStoreError> {
    let mut statement = conn.prepare(
        "SELECT id FROM collection_entries WHERE collection_id = ?1 ORDER BY manual_position ASC",
    )?;
    let rows = statement.query_map([id.to_string()], |row| {
        let id: String = row.get(0)?;
        Ok(CollectionEntryId::from_uuid(parse_uuid_sql(&id)?))
    })?;
    rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
}

fn compact_manual_positions(
    tx: &Transaction<'_>,
    id: CollectionId,
) -> Result<(), CollectionStoreError> {
    let entries = entry_ids_in_manual_order(tx, id)?;
    tx.execute(
        "UPDATE collection_entries SET manual_position = -manual_position - 1
         WHERE collection_id = ?1",
        [id.to_string()],
    )?;
    for (position, entry_id) in entries.iter().enumerate() {
        tx.execute(
            "UPDATE collection_entries SET manual_position = ?1 WHERE id = ?2",
            params![position as i64, entry_id.to_string()],
        )?;
    }
    Ok(())
}

fn compact_catalog_positions(tx: &Transaction<'_>) -> Result<(), CollectionStoreError> {
    let mut statement = tx.prepare("SELECT id FROM collections ORDER BY catalog_position ASC")?;
    let ids: Vec<String> = statement
        .query_map([], |row| row.get(0))?
        .collect::<Result<_, _>>()?;
    drop(statement);
    tx.execute(
        "UPDATE collections SET catalog_position = -catalog_position - 1",
        [],
    )?;
    for (position, id) in ids.iter().enumerate() {
        tx.execute(
            "UPDATE collections SET catalog_position = ?1 WHERE id = ?2",
            params![position as i64, id],
        )?;
    }
    Ok(())
}

fn same_ids_exactly_once(left: &[CollectionEntryId], right: &[CollectionEntryId]) -> bool {
    left.len() == right.len()
        && left.iter().copied().collect::<HashSet<_>>()
            == right.iter().copied().collect::<HashSet<_>>()
        && right.iter().copied().collect::<HashSet<_>>().len() == right.len()
}

fn require_revision(expected: u64, actual: u64) -> Result<(), CollectionStoreError> {
    if expected == actual {
        Ok(())
    } else {
        Err(CollectionStoreError::Conflict { expected, actual })
    }
}

fn normalized_name(name: &str) -> Result<&str, CollectionStoreError> {
    let name = name.trim();
    (!name.is_empty())
        .then_some(name)
        .ok_or(CollectionStoreError::InvalidName)
}

fn parse_uuid_sql(value: &str) -> rusqlite::Result<Uuid> {
    Uuid::parse_str(value).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(error))
    })
}

fn checked_u64_sql(value: i64, column: usize) -> rusqlite::Result<u64> {
    u64::try_from(value).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(
            column,
            rusqlite::types::Type::Integer,
            Box::new(error),
        )
    })
}

fn sort_order_as_str(sort: SortOrder) -> &'static str {
    match sort {
        SortOrder::FileName => "file_name",
        SortOrder::FileNameDesc => "file_name_desc",
        SortOrder::Numeric => "numeric",
        SortOrder::NumericDesc => "numeric_desc",
        SortOrder::DateAsc => "date_asc",
        SortOrder::DateDesc => "date_desc",
        SortOrder::SizeAsc => "size_asc",
        SortOrder::SizeDesc => "size_desc",
    }
}

fn sort_order_from_str(value: &str) -> Option<SortOrder> {
    match value {
        "file_name" => Some(SortOrder::FileName),
        "file_name_desc" => Some(SortOrder::FileNameDesc),
        "numeric" => Some(SortOrder::Numeric),
        "numeric_desc" => Some(SortOrder::NumericDesc),
        "date_asc" => Some(SortOrder::DateAsc),
        "date_desc" => Some(SortOrder::DateDesc),
        "size_asc" => Some(SortOrder::SizeAsc),
        "size_desc" => Some(SortOrder::SizeDesc),
        _ => None,
    }
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(i64::MAX as u128) as i64
}
