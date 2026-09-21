use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use rusqlite::{Connection, OptionalExtension, Transaction, params};
use uuid::Uuid;

use super::{
    CollectionBatchAddOutcome, CollectionCatalogSnapshot, CollectionDefinition, CollectionEntry,
    CollectionEntryId, CollectionId, CollectionMigrationOutcome, CollectionOrderMode,
    CollectionRegistration, CollectionResolvedKind, CollectionSourceMigration,
    CollectionSourceMigrationBatch, CollectionSourceNamespace, CollectionSourcePathKey,
    CollectionStoreError, MAX_COLLECTION_ENTRIES, collection_sort_order_from_wire_name,
    collection_sort_order_wire_name,
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
    const fn label(self) -> &'static str {
        match self {
            Self::New => "new",
            Self::ExistingUnversioned => "unversioned",
            Self::ExistingV1 => "v1",
            Self::ExistingV2 => "v2",
        }
    }
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SessionBackupState {
    NotRequired,
    Required,
    Completed,
    AttemptedFailed,
}

enum PreparedMutation<N, W> {
    NoOp(N),
    Reject(CollectionStoreError),
    Write(W),
}

#[derive(Clone, Copy)]
struct MutationGuard {
    catalog_revision: u64,
    collection: Option<(CollectionId, u64)>,
}

impl MutationGuard {
    fn catalog(conn: &Connection) -> Result<Self, CollectionStoreError> {
        Ok(Self {
            catalog_revision: catalog_revision(conn)?,
            collection: None,
        })
    }

    fn collection(
        conn: &Connection,
        id: CollectionId,
        expected_revision: u64,
    ) -> Result<Result<Self, CollectionStoreError>, CollectionStoreError> {
        let definition = match load_definition(conn, id) {
            Ok(definition) => definition,
            Err(error @ CollectionStoreError::NotFound) => return Ok(Err(error)),
            Err(error) => return Err(error),
        };
        if let Err(error) = require_revision(expected_revision, definition.revision) {
            return Ok(Err(error));
        }
        Ok(Ok(Self {
            catalog_revision: catalog_revision(conn)?,
            collection: Some((id, expected_revision)),
        }))
    }

    fn revalidate(self, conn: &Connection) -> Result<(), CollectionStoreError> {
        require_revision(self.catalog_revision, catalog_revision(conn)?)?;
        if let Some((id, revision)) = self.collection {
            require_revision(revision, load_definition(conn, id)?.revision)?;
        }
        Ok(())
    }
}

struct PreparedCreate {
    guard: MutationGuard,
    id: CollectionId,
    name: String,
    position: i64,
}

struct PreparedRename {
    guard: MutationGuard,
    id: CollectionId,
    name: String,
}

struct PreparedDelete {
    guard: MutationGuard,
    id: CollectionId,
}

struct PreparedSetOrder {
    guard: MutationGuard,
    id: CollectionId,
    mode: CollectionOrderMode,
    standard_sort: SortOrder,
    shuffle_seed: u64,
}

struct PreparedAddBatch {
    guard: MutationGuard,
    id: CollectionId,
    position: i64,
    added: Vec<(CollectionEntryId, CollectionRegistration)>,
    duplicates: Vec<CollectionSourcePathKey>,
    capacity_rejected: Vec<CollectionSourcePathKey>,
}

struct PreparedRemoveEntries {
    guard: MutationGuard,
    id: CollectionId,
    entry_ids: Vec<CollectionEntryId>,
}

struct PreparedReorderManual {
    guard: MutationGuard,
    id: CollectionId,
    previous: Vec<CollectionEntryId>,
    order: Vec<CollectionEntryId>,
}

struct PreparedRelink {
    guard: MutationGuard,
    id: CollectionId,
    entry_id: CollectionEntryId,
    previous: CollectionEntry,
    registration: CollectionRegistration,
}

struct PreparedMigration {
    guard: MutationGuard,
    previous_entries: Vec<CollectionEntry>,
    replacements: Vec<(
        CollectionEntryId,
        CollectionId,
        CollectionSourcePathKey,
        PathBuf,
    )>,
}

#[derive(Default)]
struct OpenTimings {
    startup: Option<CollectionDbStartup>,
    open: Duration,
    validate: Duration,
    backup: Duration,
    schema: Duration,
}

pub(super) struct CollectionStoreDb {
    conn: Connection,
    path: PathBuf,
    session_backup: SessionBackupState,
    last_mutation_applied: bool,
    #[cfg(test)]
    before_apply_hook: Option<Box<dyn FnOnce() + Send>>,
}

impl CollectionStoreDb {
    pub(super) fn open_at(path: &Path) -> Result<Self, CollectionStoreError> {
        let mut timings = OpenTimings::default();
        let result = Self::open_at_inner(path, &mut timings);
        emit_open_perf(&timings, result.as_ref().err());
        result
    }

    fn open_at_inner(path: &Path, timings: &mut OpenTimings) -> Result<Self, CollectionStoreError> {
        let existed = path
            .try_exists()
            .map_err(|error| CollectionStoreError::Persistence(error.to_string()))?;
        let startup = if existed {
            let started = Instant::now();
            let probe = Connection::open_with_flags(
                path,
                rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_URI,
            )?;
            timings.open += started.elapsed();
            let version: u32 = probe.pragma_query_value(None, "user_version", |row| row.get(0))?;
            let startup = match version {
                0 => CollectionDbStartup::ExistingUnversioned,
                1 => CollectionDbStartup::ExistingV1,
                SCHEMA_VERSION => CollectionDbStartup::ExistingV2,
                newer => return Err(CollectionStoreError::IncompatibleSchema(newer)),
            };
            timings.startup = Some(startup);
            let validate_started = Instant::now();
            let validation = match startup {
                CollectionDbStartup::ExistingUnversioned => validate_integrity(&probe),
                CollectionDbStartup::ExistingV1 => validate_existing_v1(&probe),
                CollectionDbStartup::ExistingV2 => validate_existing_v2(&probe),
                CollectionDbStartup::New => unreachable!(),
            };
            timings.validate = validate_started.elapsed();
            validation?;
            startup
        } else {
            CollectionDbStartup::New
        };
        timings.startup = Some(startup);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|error| CollectionStoreError::Persistence(error.to_string()))?;
        }
        let open_started = Instant::now();
        let conn = Connection::open(path)?;
        conn.busy_timeout(Duration::from_secs(3))?;
        timings.open += open_started.elapsed();
        let version: u32 = conn.pragma_query_value(None, "user_version", |row| row.get(0))?;
        if version > SCHEMA_VERSION {
            return Err(CollectionStoreError::IncompatibleSchema(version));
        }
        if version != startup.expected_version() {
            return Err(CollectionStoreError::Persistence(
                "collection schema changed during startup".into(),
            ));
        }
        let session_backup = match startup {
            // The empty generation is not useful. The first successful write arms the normal
            // first-mutation backup so the next real change preserves that first useful state.
            CollectionDbStartup::New => SessionBackupState::NotRequired,
            CollectionDbStartup::ExistingV2 => {
                let catalog_revision = catalog_revision(&conn)?;
                let collection_count: i64 =
                    conn.query_row("SELECT COUNT(*) FROM collections", [], |row| row.get(0))?;
                if catalog_revision == 0 && collection_count == 0 {
                    SessionBackupState::NotRequired
                } else {
                    SessionBackupState::Required
                }
            }
            CollectionDbStartup::ExistingUnversioned | CollectionDbStartup::ExistingV1 => {
                let backup_started = Instant::now();
                let backup_result = rotate_collection_backup(&conn, path);
                timings.backup = backup_started.elapsed();
                match backup_result {
                    Ok(()) => crate::logger::log(format!(
                        "collection schema backup completed startup={} elapsed_ms={:.3}",
                        startup.label(),
                        timings.backup.as_secs_f64() * 1000.0
                    )),
                    Err(error) => {
                        crate::logger::log(format!(
                            "collection schema backup failed; startup stopped before schema write startup={} elapsed_ms={:.3}: {error}",
                            startup.label(),
                            timings.backup.as_secs_f64() * 1000.0
                        ));
                        return Err(CollectionStoreError::Persistence(error));
                    }
                }
                SessionBackupState::Completed
            }
        };
        conn.pragma_update(None, "foreign_keys", true)?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        let schema_started = Instant::now();
        let schema_result = match startup {
            CollectionDbStartup::New | CollectionDbStartup::ExistingUnversioned => {
                initialize_schema(&conn)
            }
            CollectionDbStartup::ExistingV1 => migrate_v1_to_v2(&conn),
            CollectionDbStartup::ExistingV2 => Ok(()),
        };
        timings.schema = schema_started.elapsed();
        schema_result?;
        Ok(Self {
            conn,
            path: path.to_path_buf(),
            session_backup,
            last_mutation_applied: false,
            #[cfg(test)]
            before_apply_hook: None,
        })
    }

    fn execute_prepared<N, W>(
        &mut self,
        prepared: PreparedMutation<N, W>,
        apply: impl FnOnce(&mut Connection, W) -> Result<N, CollectionStoreError>,
    ) -> Result<N, CollectionStoreError> {
        self.last_mutation_applied = false;
        match prepared {
            PreparedMutation::NoOp(value) => Ok(value),
            PreparedMutation::Reject(error) => Err(error),
            PreparedMutation::Write(write) => {
                self.ensure_session_backup();
                #[cfg(test)]
                if let Some(hook) = self.before_apply_hook.take() {
                    hook();
                }
                let value = apply(&mut self.conn, write)?;
                if self.session_backup == SessionBackupState::NotRequired {
                    self.session_backup = SessionBackupState::Required;
                }
                self.last_mutation_applied = true;
                Ok(value)
            }
        }
    }

    fn ensure_session_backup(&mut self) {
        if self.session_backup != SessionBackupState::Required {
            return;
        }
        // Mark the attempt before starting. A failed normal snapshot is nonfatal and must not
        // be retried for every later command in the same session.
        self.session_backup = SessionBackupState::AttemptedFailed;
        let started = Instant::now();
        let result = rotate_collection_backup(&self.conn, &self.path);
        let elapsed = started.elapsed();
        match result {
            Ok(()) => {
                self.session_backup = SessionBackupState::Completed;
                crate::logger::log(format!(
                    "collection first-mutation backup completed elapsed_ms={:.3}",
                    elapsed.as_secs_f64() * 1000.0
                ));
            }
            Err(error) => crate::logger::log(format!(
                "collection first-mutation backup failed; continuing with write elapsed_ms={:.3}: {error}",
                elapsed.as_secs_f64() * 1000.0
            )),
        }
        emit_backup_perf(
            elapsed,
            match self.session_backup {
                SessionBackupState::Completed => "ok",
                SessionBackupState::AttemptedFailed => "error_continue",
                _ => unreachable!("a required backup attempt has a terminal state"),
            },
        );
    }

    pub(super) fn take_last_mutation_applied(&mut self) -> bool {
        std::mem::take(&mut self.last_mutation_applied)
    }

    #[cfg(test)]
    pub(super) fn set_before_apply_hook(&mut self, hook: impl FnOnce() + Send + 'static) {
        self.before_apply_hook = Some(Box::new(hook));
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
        self.last_mutation_applied = false;
        let prepared = match normalized_name(name) {
            Ok(name) => {
                let tx = self.conn.unchecked_transaction()?;
                let guard = MutationGuard::catalog(&tx)?;
                let position = tx.query_row(
                    "SELECT COALESCE(MAX(catalog_position) + 1, 0) FROM collections",
                    [],
                    |row| row.get(0),
                )?;
                tx.commit()?;
                PreparedMutation::Write(PreparedCreate {
                    guard,
                    id,
                    name: name.to_owned(),
                    position,
                })
            }
            Err(error) => PreparedMutation::Reject(error),
        };
        self.execute_prepared(prepared, |conn, prepared| {
            let tx = conn.transaction()?;
            prepared.guard.revalidate(&tx)?;
            let now = now_ms();
            tx.execute(
                "INSERT INTO collections
                 (id, name, order_mode, sort_order, revision, catalog_position, created_at_ms, updated_at_ms)
                 VALUES (?1, ?2, 'manual', 'file_name', 1, ?3, ?4, ?4)",
                params![
                    prepared.id.to_string(),
                    prepared.name,
                    prepared.position,
                    now
                ],
            )?;
            let catalog_revision = bump_catalog_revision(&tx)?;
            let snapshot =
                load_snapshot_with_catalog(&tx, prepared.id, catalog_revision)?;
            tx.commit()?;
            Ok(snapshot)
        })
    }

    pub(super) fn rename_collection(
        &mut self,
        id: CollectionId,
        expected_revision: u64,
        name: &str,
    ) -> Result<super::CollectionSnapshot, CollectionStoreError> {
        self.last_mutation_applied = false;
        let prepared = match normalized_name(name) {
            Err(error) => PreparedMutation::Reject(error),
            Ok(name) => {
                let tx = self.conn.unchecked_transaction()?;
                let prepared = match MutationGuard::collection(&tx, id, expected_revision)? {
                    Err(error) => PreparedMutation::Reject(error),
                    Ok(guard) => {
                        let definition = load_definition(&tx, id)?;
                        if definition.name == name {
                            PreparedMutation::NoOp(load_snapshot_with_catalog(
                                &tx,
                                id,
                                guard.catalog_revision,
                            )?)
                        } else {
                            PreparedMutation::Write(PreparedRename {
                                guard,
                                id,
                                name: name.to_owned(),
                            })
                        }
                    }
                };
                tx.commit()?;
                prepared
            }
        };
        self.execute_prepared(prepared, |conn, prepared| {
            let tx = conn.transaction()?;
            prepared.guard.revalidate(&tx)?;
            let revision = increment_collection_revision(&tx, prepared.id)?;
            tx.execute(
                "UPDATE collections SET name = ?1, updated_at_ms = ?2 WHERE id = ?3",
                params![prepared.name, now_ms(), prepared.id.to_string()],
            )?;
            let catalog_revision = bump_catalog_revision(&tx)?;
            let snapshot = load_snapshot_with_catalog(&tx, prepared.id, catalog_revision)?;
            debug_assert_eq!(snapshot.revision(), revision);
            tx.commit()?;
            Ok(snapshot)
        })
    }

    pub(super) fn delete_collection(
        &mut self,
        id: CollectionId,
        expected_revision: u64,
    ) -> Result<CollectionCatalogSnapshot, CollectionStoreError> {
        self.last_mutation_applied = false;
        let tx = self.conn.unchecked_transaction()?;
        let prepared = match MutationGuard::collection(&tx, id, expected_revision)? {
            Ok(guard) => PreparedMutation::Write(PreparedDelete { guard, id }),
            Err(error) => PreparedMutation::Reject(error),
        };
        tx.commit()?;
        self.execute_prepared(prepared, |conn, prepared| {
            let tx = conn.transaction()?;
            prepared.guard.revalidate(&tx)?;
            tx.execute(
                "DELETE FROM collections WHERE id = ?1",
                [prepared.id.to_string()],
            )?;
            compact_catalog_positions(&tx)?;
            let revision = bump_catalog_revision(&tx)?;
            let catalog = load_catalog_with_revision(&tx, revision)?;
            tx.commit()?;
            Ok(catalog)
        })
    }

    pub(super) fn set_order(
        &mut self,
        id: CollectionId,
        expected_revision: u64,
        mode: CollectionOrderMode,
        standard_sort: SortOrder,
    ) -> Result<super::CollectionSnapshot, CollectionStoreError> {
        self.last_mutation_applied = false;
        let tx = self.conn.unchecked_transaction()?;
        let prepared = match MutationGuard::collection(&tx, id, expected_revision)? {
            Err(error) => PreparedMutation::Reject(error),
            Ok(guard) => {
                let definition = load_definition(&tx, id)?;
                if mode != CollectionOrderMode::Shuffle
                    && definition.order_mode == mode
                    && definition.standard_sort == standard_sort
                {
                    PreparedMutation::NoOp(load_snapshot_with_catalog(
                        &tx,
                        id,
                        guard.catalog_revision,
                    )?)
                } else {
                    PreparedMutation::Write(PreparedSetOrder {
                        guard,
                        id,
                        mode,
                        standard_sort,
                        shuffle_seed: if mode == CollectionOrderMode::Shuffle {
                            new_shuffle_seed()
                        } else {
                            definition.shuffle_seed
                        },
                    })
                }
            }
        };
        tx.commit()?;
        self.execute_prepared(prepared, |conn, prepared| {
            let tx = conn.transaction()?;
            prepared.guard.revalidate(&tx)?;
            tx.execute(
                "UPDATE collections
                 SET order_mode = ?1, sort_order = ?2, shuffle_seed = ?3,
                     revision = revision + 1, updated_at_ms = ?4
                 WHERE id = ?5",
                params![
                    prepared.mode.as_str(),
                    collection_sort_order_wire_name(prepared.standard_sort),
                    format!("{:016x}", prepared.shuffle_seed),
                    now_ms(),
                    prepared.id.to_string()
                ],
            )?;
            let catalog_revision = bump_catalog_revision(&tx)?;
            let snapshot = load_snapshot_with_catalog(&tx, prepared.id, catalog_revision)?;
            tx.commit()?;
            Ok(snapshot)
        })
    }

    pub(super) fn add_batch(
        &mut self,
        id: CollectionId,
        expected_revision: u64,
        registrations: Vec<CollectionRegistration>,
    ) -> Result<CollectionBatchAddOutcome, CollectionStoreError> {
        self.last_mutation_applied = false;
        let tx = self.conn.unchecked_transaction()?;
        let prepared = match MutationGuard::collection(&tx, id, expected_revision)? {
            Err(error) => PreparedMutation::Reject(error),
            Ok(guard) => {
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
                    added.push((CollectionEntryId::new(), registration));
                    position += 1;
                    remaining -= 1;
                }
                if added.is_empty() {
                    PreparedMutation::NoOp(CollectionBatchAddOutcome {
                        snapshot: load_snapshot_with_catalog(&tx, id, guard.catalog_revision)?,
                        added: Arc::from([]),
                        duplicates: Arc::from(duplicates),
                        capacity_rejected: Arc::from(capacity_rejected),
                    })
                } else {
                    PreparedMutation::Write(PreparedAddBatch {
                        guard,
                        id,
                        position: position - added.len() as i64,
                        added,
                        duplicates,
                        capacity_rejected,
                    })
                }
            }
        };
        tx.commit()?;
        self.execute_prepared(prepared, |conn, prepared| {
            let tx = conn.transaction()?;
            prepared.guard.revalidate(&tx)?;
            let now = now_ms();
            let mut position = prepared.position;
            let added_ids: Vec<CollectionEntryId> =
                prepared.added.iter().map(|(id, _)| *id).collect();
            for (entry_id, registration) in prepared.added {
                tx.execute(
                    "INSERT INTO collection_entries
                     (id, collection_id, source_namespace, source_path, normalized_path,
                      resolved_kind, manual_position, created_at_ms)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                    params![
                        entry_id.to_string(),
                        prepared.id.to_string(),
                        registration.source_key.namespace().as_str(),
                        registration.source_path.to_string_lossy(),
                        registration.source_key.normalized_path(),
                        registration.resolved_kind.as_str(),
                        position,
                        now,
                    ],
                )?;
                position += 1;
            }
            increment_collection_revision(&tx, prepared.id)?;
            let catalog_revision = bump_catalog_revision(&tx)?;
            let snapshot = load_snapshot_with_catalog(&tx, prepared.id, catalog_revision)?;
            tx.commit()?;
            Ok(CollectionBatchAddOutcome {
                snapshot,
                added: Arc::from(added_ids),
                duplicates: Arc::from(prepared.duplicates),
                capacity_rejected: Arc::from(prepared.capacity_rejected),
            })
        })
    }

    pub(super) fn remove_entries(
        &mut self,
        id: CollectionId,
        expected_revision: u64,
        entry_ids: Vec<CollectionEntryId>,
    ) -> Result<super::CollectionSnapshot, CollectionStoreError> {
        self.last_mutation_applied = false;
        let tx = self.conn.unchecked_transaction()?;
        let prepared = match MutationGuard::collection(&tx, id, expected_revision)? {
            Err(error) => PreparedMutation::Reject(error),
            Ok(guard) => {
                let existing = entry_ids_in_manual_order(&tx, id)?
                    .into_iter()
                    .collect::<HashSet<_>>();
                let mut seen = HashSet::new();
                let entry_ids = entry_ids
                    .into_iter()
                    .filter(|entry_id| seen.insert(*entry_id) && existing.contains(entry_id))
                    .collect::<Vec<_>>();
                if entry_ids.is_empty() {
                    PreparedMutation::NoOp(load_snapshot_with_catalog(
                        &tx,
                        id,
                        guard.catalog_revision,
                    )?)
                } else {
                    PreparedMutation::Write(PreparedRemoveEntries {
                        guard,
                        id,
                        entry_ids,
                    })
                }
            }
        };
        tx.commit()?;
        self.execute_prepared(prepared, |conn, prepared| {
            let tx = conn.transaction()?;
            prepared.guard.revalidate(&tx)?;
            for entry_id in &prepared.entry_ids {
                let removed = tx.execute(
                    "DELETE FROM collection_entries WHERE collection_id = ?1 AND id = ?2",
                    params![prepared.id.to_string(), entry_id.to_string()],
                )?;
                if removed != 1 {
                    return Err(CollectionStoreError::Persistence(
                        "collection entry changed during backup".into(),
                    ));
                }
            }
            compact_manual_positions(&tx, prepared.id)?;
            increment_collection_revision(&tx, prepared.id)?;
            let catalog_revision = bump_catalog_revision(&tx)?;
            let snapshot = load_snapshot_with_catalog(&tx, prepared.id, catalog_revision)?;
            tx.commit()?;
            Ok(snapshot)
        })
    }

    pub(super) fn reorder_manual(
        &mut self,
        id: CollectionId,
        expected_revision: u64,
        order: Vec<CollectionEntryId>,
    ) -> Result<super::CollectionSnapshot, CollectionStoreError> {
        self.last_mutation_applied = false;
        let tx = self.conn.unchecked_transaction()?;
        let prepared = match MutationGuard::collection(&tx, id, expected_revision)? {
            Err(error) => PreparedMutation::Reject(error),
            Ok(guard) => {
                let definition = load_definition(&tx, id)?;
                if definition.order_mode != CollectionOrderMode::Manual {
                    PreparedMutation::Reject(CollectionStoreError::ManualOrderInactive)
                } else {
                    let current = entry_ids_in_manual_order(&tx, id)?;
                    if !same_ids_exactly_once(&current, &order) {
                        PreparedMutation::Reject(CollectionStoreError::InvalidOrder)
                    } else if current == order {
                        PreparedMutation::NoOp(load_snapshot_with_catalog(
                            &tx,
                            id,
                            guard.catalog_revision,
                        )?)
                    } else {
                        PreparedMutation::Write(PreparedReorderManual {
                            guard,
                            id,
                            previous: current,
                            order,
                        })
                    }
                }
            }
        };
        tx.commit()?;
        self.execute_prepared(prepared, |conn, prepared| {
            let tx = conn.transaction()?;
            prepared.guard.revalidate(&tx)?;
            if entry_ids_in_manual_order(&tx, prepared.id)? != prepared.previous {
                return Err(CollectionStoreError::Persistence(
                    "collection order changed during backup".into(),
                ));
            }
            tx.execute(
                "UPDATE collection_entries SET manual_position = -manual_position - 1
                 WHERE collection_id = ?1",
                [prepared.id.to_string()],
            )?;
            for (position, entry_id) in prepared.order.iter().enumerate() {
                tx.execute(
                    "UPDATE collection_entries SET manual_position = ?1
                     WHERE collection_id = ?2 AND id = ?3",
                    params![
                        position as i64,
                        prepared.id.to_string(),
                        entry_id.to_string()
                    ],
                )?;
            }
            increment_collection_revision(&tx, prepared.id)?;
            let catalog_revision = bump_catalog_revision(&tx)?;
            let snapshot = load_snapshot_with_catalog(&tx, prepared.id, catalog_revision)?;
            tx.commit()?;
            Ok(snapshot)
        })
    }

    pub(super) fn relink(
        &mut self,
        id: CollectionId,
        expected_revision: u64,
        entry_id: CollectionEntryId,
        registration: CollectionRegistration,
    ) -> Result<super::CollectionSnapshot, CollectionStoreError> {
        self.last_mutation_applied = false;
        let tx = self.conn.unchecked_transaction()?;
        let prepared = match MutationGuard::collection(&tx, id, expected_revision)? {
            Err(error) => PreparedMutation::Reject(error),
            Ok(guard) => match load_entry(&tx, id, entry_id) {
                Err(error @ CollectionStoreError::NotFound) => PreparedMutation::Reject(error),
                Err(error) => return Err(error),
                Ok(current)
                    if current.source_path == registration.source_path
                        && current.source_key == registration.source_key
                        && current.resolved_kind == registration.resolved_kind =>
                {
                    PreparedMutation::NoOp(load_snapshot_with_catalog(
                        &tx,
                        id,
                        guard.catalog_revision,
                    )?)
                }
                Ok(_current)
                    if source_exists_except(&tx, id, entry_id, &registration.source_key)? =>
                {
                    PreparedMutation::Reject(CollectionStoreError::DuplicateSource(
                        registration.source_key,
                    ))
                }
                Ok(current) => PreparedMutation::Write(PreparedRelink {
                    guard,
                    id,
                    entry_id,
                    previous: current,
                    registration,
                }),
            },
        };
        tx.commit()?;
        self.execute_prepared(prepared, |conn, prepared| {
            let tx = conn.transaction()?;
            prepared.guard.revalidate(&tx)?;
            if load_entry(&tx, prepared.id, prepared.entry_id)? != prepared.previous {
                return Err(CollectionStoreError::Persistence(
                    "collection entry changed during backup".into(),
                ));
            }
            if source_exists_except(
                &tx,
                prepared.id,
                prepared.entry_id,
                &prepared.registration.source_key,
            )? {
                return Err(CollectionStoreError::DuplicateSource(
                    prepared.registration.source_key,
                ));
            }
            tx.execute(
                "UPDATE collection_entries
                 SET source_namespace = ?1, source_path = ?2, normalized_path = ?3, resolved_kind = ?4
                 WHERE collection_id = ?5 AND id = ?6",
                params![
                    prepared.registration.source_key.namespace().as_str(),
                    prepared.registration.source_path.to_string_lossy(),
                    prepared.registration.source_key.normalized_path(),
                    prepared.registration.resolved_kind.as_str(),
                    prepared.id.to_string(),
                    prepared.entry_id.to_string(),
                ],
            )?;
            increment_collection_revision(&tx, prepared.id)?;
            let catalog_revision = bump_catalog_revision(&tx)?;
            let snapshot = load_snapshot_with_catalog(&tx, prepared.id, catalog_revision)?;
            tx.commit()?;
            Ok(snapshot)
        })
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
        self.last_mutation_applied = false;
        let tx = self.conn.unchecked_transaction()?;
        let guard = MutationGuard::catalog(&tx)?;
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
                if replacement.path() != entry.source_path || replacement.key() != &entry.source_key
                {
                    replacements.push((
                        entry.id,
                        entry.collection_id,
                        replacement.key().clone(),
                        replacement.path().to_path_buf(),
                    ));
                }
            }
        }

        let prepared = if replacements.is_empty() {
            PreparedMutation::NoOp(CollectionMigrationOutcome {
                catalog_revision: guard.catalog_revision,
                affected: Arc::from([]),
                updated_entries: 0,
            })
        } else {
            PreparedMutation::Write(PreparedMigration {
                guard,
                previous_entries: entries,
                replacements,
            })
        };
        tx.commit()?;
        self.execute_prepared(prepared, |conn, prepared| {
            let tx = conn.transaction()?;
            prepared.guard.revalidate(&tx)?;
            if load_all_entries(&tx)? != prepared.previous_entries {
                return Err(CollectionStoreError::Persistence(
                    "collection sources changed during backup".into(),
                ));
            }
            let mut affected = HashSet::new();
            // A batch may exchange two keys in the same collection. Validation above proves that
            // the final key set is unique, but updating rows directly would still collide with the
            // other row's old UNIQUE key. Move every affected row into a transaction-local
            // namespace first; every following error rolls the transaction back.
            for (entry_id, _, _, _) in &prepared.replacements {
                tx.execute(
                    "UPDATE collection_entries
                     SET source_namespace = '_migration', normalized_path = ?1
                     WHERE id = ?2",
                    params![entry_id.to_string(), entry_id.to_string()],
                )?;
            }
            for (entry_id, collection_id, key, path) in &prepared.replacements {
                tx.execute(
                    "UPDATE collection_entries
                     SET source_namespace = ?1, source_path = ?2, normalized_path = ?3
                     WHERE id = ?4",
                    params![
                        key.namespace().as_str(),
                        path.to_string_lossy(),
                        key.normalized_path(),
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
            let updated_entries = prepared.replacements.len();
            tx.commit()?;
            Ok(CollectionMigrationOutcome {
                catalog_revision,
                affected: Arc::from(revisions),
                updated_entries,
            })
        })
    }
}

fn rotate_collection_backup(conn: &Connection, path: &Path) -> Result<(), String> {
    let db_file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| "collection database has no filename".to_string())?;
    crate::db_backup::rotate_generation_backups(
        path.parent().unwrap_or_else(|| Path::new(".")),
        db_file_name,
        &|message| crate::logger::log(message),
        &|destination| {
            conn.execute("VACUUM INTO ?1", [destination.to_string_lossy().as_ref()])
                .map(|_| ())
                .map_err(|error| error.to_string())
        },
    )
}

fn emit_open_perf(timings: &OpenTimings, error: Option<&CollectionStoreError>) {
    if !crate::perf::is_enabled() {
        return;
    }
    crate::perf::event(
        "collection",
        "db_open",
        None,
        0,
        &[
            (
                "startup",
                serde_json::Value::from(
                    timings
                        .startup
                        .map_or("unknown", CollectionDbStartup::label),
                ),
            ),
            (
                "open_ms",
                serde_json::Value::from(timings.open.as_secs_f64() * 1000.0),
            ),
            (
                "validate_ms",
                serde_json::Value::from(timings.validate.as_secs_f64() * 1000.0),
            ),
            (
                "backup_ms",
                serde_json::Value::from(timings.backup.as_secs_f64() * 1000.0),
            ),
            (
                "schema_ms",
                serde_json::Value::from(timings.schema.as_secs_f64() * 1000.0),
            ),
            (
                "outcome",
                serde_json::Value::from(if error.is_some() { "error" } else { "ok" }),
            ),
        ],
    );
}

fn emit_backup_perf(elapsed: Duration, outcome: &'static str) {
    if !crate::perf::is_enabled() {
        return;
    }
    crate::perf::event(
        "collection",
        "db_backup",
        None,
        0,
        &[
            ("trigger", serde_json::Value::from("first_mutation")),
            ("outcome", serde_json::Value::from(outcome)),
            (
                "ms",
                serde_json::Value::from(elapsed.as_secs_f64() * 1000.0),
            ),
        ],
    );
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
        standard_sort: collection_sort_order_from_wire_name(&sort_order).ok_or_else(|| {
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

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(i64::MAX as u128) as i64
}
