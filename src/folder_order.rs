//! Physical-folder manual order. The process owns one FIFO actor; listing workers
//! request reads through its handle and the UI only submits/polls messages.
//! Remote and Agent IPC workers share the App's actor. Reading never records an order.

use rusqlite::{Connection, OptionalExtension, params};
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex, mpsc};
use std::time::{Duration, Instant};

mod listing;
pub(crate) use listing::*;

pub(crate) const DB_NAME: &str = "folder_order.db";

/// Folder identities ignore trailing separators, except for rooted drive paths.
pub(crate) fn folder_key(path: &Path) -> String {
    let key = crate::path_key::normalize_keep_drive(path);
    let trimmed = key.trim_end_matches('/');
    if trimmed.ends_with(':') || trimmed.is_empty() {
        format!("{trimmed}/")
    } else {
        trimmed.to_owned()
    }
}

/// Shared by read/save/reset, including drive and UNC share roots.
pub(crate) fn direct_child_prefix(folder: &str) -> String {
    format!("{}/", folder.trim_end_matches('/'))
}

fn prefix_bounds(folder: &str) -> (String, String) {
    let prefix = direct_child_prefix(folder);
    // '/' is the final ASCII byte. Its successor bounds every descendant,
    // without SQL LIKE wildcard interpretation of '%' or '_' in a real path.
    let upper = format!("{}0", prefix.trim_end_matches('/'));
    (prefix, upper)
}

fn is_direct_child(folder: &str, key: &str) -> bool {
    key.strip_prefix(&direct_child_prefix(folder))
        .is_some_and(|tail| !tail.is_empty() && !tail.contains('/'))
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct FolderOrderRecord {
    pub active: bool,
    pub revision: u64,
    /// Keys in recorded order, including currently hidden but present children.
    pub items: Vec<String>,
}

/// Complete post-reorder listing plus pre-projection directory inventory.
/// `revision` correlates the caller's request/completion; the persisted revision
/// is independently incremented by the actor inside the save transaction.
#[derive(Clone, Debug)]
pub(crate) struct FolderOrderSave {
    pub listing_order: Vec<String>,
    pub inventory: HashSet<String>,
    pub revision: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct FolderOrderError {
    pub message: String,
}

impl FolderOrderError {
    pub(crate) fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl std::fmt::Display for FolderOrderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for FolderOrderError {}

impl From<rusqlite::Error> for FolderOrderError {
    fn from(error: rusqlite::Error) -> Self {
        Self::new(error.to_string())
    }
}

type OrderResult<T> = Result<T, FolderOrderError>;

/// Replace visible slots only. Full order includes facet-hidden listing items.
pub(crate) fn merge_visible_order(
    full: &[String],
    visible: &HashSet<String>,
    reordered: &[String],
) -> OrderResult<Vec<String>> {
    let full_set = full.iter().map(String::as_str).collect::<HashSet<_>>();
    let reordered_set = reordered.iter().cloned().collect::<HashSet<_>>();
    if full_set.len() != full.len()
        || !visible.iter().all(|key| full_set.contains(key.as_str()))
        || reordered_set.len() != reordered.len()
        || &reordered_set != visible
    {
        return Err(FolderOrderError::new("invalid visible folder order"));
    }
    let mut replacements = reordered.iter();
    Ok(full
        .iter()
        .map(|key| {
            if visible.contains(key) {
                replacements.next().unwrap().clone()
            } else {
                key.clone()
            }
        })
        .collect())
}

/// Compose a newly reordered whole listing with its previous stored order.
/// Recorded inventory-only children retain their slots; unrecorded inventory-only
/// children remain unknown. Missing children are removed. New listing items fill
/// listing slots in their new order and any remainder is appended.
pub(crate) fn merge_inventory_order(
    folder: &str,
    recorded: &[String],
    listing_order: &[String],
    inventory: &HashSet<String>,
) -> OrderResult<Vec<String>> {
    let listed = listing_order
        .iter()
        .map(String::as_str)
        .collect::<HashSet<_>>();
    let previous = recorded.iter().map(String::as_str).collect::<HashSet<_>>();
    if listed.len() != listing_order.len()
        || previous.len() != recorded.len()
        || !listing_order.iter().all(|key| inventory.contains(key))
        || !inventory.iter().all(|key| is_direct_child(folder, key))
        || !recorded.iter().all(|key| is_direct_child(folder, key))
    {
        return Err(FolderOrderError::new("invalid folder order inventory"));
    }
    let mut replacements = listing_order.iter();
    let mut result = Vec::with_capacity(recorded.len() + listing_order.len());
    for key in recorded.iter().filter(|key| inventory.contains(*key)) {
        if listed.contains(key.as_str()) {
            if let Some(next) = replacements.next() {
                result.push(next.clone());
            }
        } else {
            result.push(key.clone());
        }
    }
    result.extend(replacements.cloned());
    Ok(result)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RenameScope {
    Exact,
    Tree,
}

#[derive(Clone, Debug)]
pub(crate) struct FolderOrderRename {
    pub old_key: String,
    pub new_key: String,
    pub scope: RenameScope,
}

impl FolderOrderRename {
    pub(crate) fn new(old: &Path, new: &Path, scope: RenameScope) -> Self {
        Self {
            old_key: folder_key(old),
            new_key: folder_key(new),
            scope,
        }
    }

    fn matches(&self, root: &str, key: &str) -> bool {
        key == root
            || (self.scope == RenameScope::Tree && key.starts_with(&direct_child_prefix(root)))
    }
}

/// Shared persistence/memory rule: source wins, stale destination disappears,
/// including when the source has no record. Case-only renames are no-ops.
pub(crate) fn rewrite_key(key: &str, rename: &FolderOrderRename) -> Option<String> {
    if rename.old_key == rename.new_key {
        return Some(key.to_owned());
    }
    if rename.matches(&rename.old_key, key) {
        if key == rename.old_key {
            Some(rename.new_key.clone())
        } else {
            let prefix = direct_child_prefix(&rename.old_key);
            let suffix = key.strip_prefix(&prefix).unwrap();
            Some(format!("{}{suffix}", direct_child_prefix(&rename.new_key)))
        }
    } else if rename.matches(&rename.new_key, key) {
        None
    } else {
        Some(key.to_owned())
    }
}

/// A batch uses the original source identities simultaneously (including swaps).
/// A source match takes precedence over every stale destination. For nested
/// source trees the most specific mapping owns the key.
pub(crate) fn rewrite_key_with_mappings(
    key: &str,
    mappings: &[FolderOrderRename],
) -> Option<String> {
    if let Some(source) = mappings
        .iter()
        .filter(|mapping| mapping.matches(&mapping.old_key, key))
        .max_by_key(|mapping| mapping.old_key.len())
    {
        return rewrite_key(key, source);
    }
    if mappings
        .iter()
        .any(|mapping| mapping.old_key != mapping.new_key && mapping.matches(&mapping.new_key, key))
    {
        None
    } else {
        Some(key.to_owned())
    }
}

struct FolderOrderDb {
    conn: Connection,
}

/// Production-schema fixture entry point, without a second production opener.
#[cfg(test)]
pub(crate) fn initialize_database_at(path: &Path) -> OrderResult<()> {
    FolderOrderDb::open_at(path).map(|_| ())
}

impl FolderOrderDb {
    fn open_at(path: &Path) -> OrderResult<Self> {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let mut conn = Connection::open(path)?;
        conn.busy_timeout(Duration::from_secs(2))?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let version: i64 = tx.pragma_query_value(None, "user_version", |row| row.get(0))?;
        match version {
            0 => {
                // No migration/recreation: unexpected existing tables fail intact.
                tx.execute_batch("CREATE TABLE folder_order_items (
                    item_key TEXT PRIMARY KEY, position INTEGER NOT NULL);
                    CREATE TABLE folder_order_folders (
                    folder_key TEXT PRIMARY KEY, active INTEGER NOT NULL, revision INTEGER NOT NULL);
                    PRAGMA user_version = 1;")?;
            }
            1 => {}
            _ => {
                return Err(FolderOrderError::new(format!(
                    "unsupported folder order schema {version}"
                )));
            }
        }
        tx.prepare("SELECT item_key, position FROM folder_order_items LIMIT 0")?;
        tx.prepare("SELECT folder_key, active, revision FROM folder_order_folders LIMIT 0")?;
        tx.commit()?;
        Ok(Self { conn })
    }

    fn read(&self, folder: &str) -> OrderResult<Option<FolderOrderRecord>> {
        // One read transaction also gives Remote a coherent folders/items pair.
        let tx = self.conn.unchecked_transaction()?;
        let record = Self::read_from(&tx, folder)?;
        tx.commit()?;
        Ok(record)
    }

    fn folder_keys(&self) -> OrderResult<HashSet<String>> {
        let mut stmt = self
            .conn
            .prepare("SELECT folder_key FROM folder_order_folders")?;
        let keys = stmt
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<rusqlite::Result<HashSet<_>>>()?;
        if keys.iter().any(|key| folder_key(Path::new(key)) != *key) {
            return Err(FolderOrderError::new("invalid folder order folder key"));
        }
        Ok(keys)
    }

    fn read_from(conn: &Connection, folder: &str) -> OrderResult<Option<FolderOrderRecord>> {
        let meta = conn
            .query_row(
                "SELECT active, revision FROM folder_order_folders WHERE folder_key=?1",
                [folder],
                |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
            )
            .optional()?;
        let Some((active, revision)) = meta else {
            return Ok(None);
        };
        if !matches!(active, 0 | 1) || revision < 0 {
            return Err(FolderOrderError::new("invalid folder order record"));
        }
        let (prefix, upper) = prefix_bounds(folder);
        let rows = {
            let mut stmt = conn.prepare(
                "SELECT item_key, position FROM folder_order_items
                WHERE item_key>=?1 AND item_key<?2 ORDER BY position, item_key",
            )?;
            stmt.query_map(params![prefix, upper], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?
        };
        let mut positions = HashSet::new();
        let mut items = Vec::new();
        for (key, position) in rows
            .into_iter()
            .filter(|(key, _)| is_direct_child(folder, key))
        {
            if position < 0
                || !positions.insert(position)
                || crate::path_key::normalize_keep_drive(Path::new(&key)) != key
            {
                return Err(FolderOrderError::new("invalid folder order item"));
            }
            items.push(key);
        }
        Ok(Some(FolderOrderRecord {
            active: active == 1,
            revision: revision as u64,
            items,
        }))
    }

    fn delete_children(conn: &Connection, folder: &str) -> OrderResult<()> {
        let (prefix, upper) = prefix_bounds(folder);
        let keys = {
            let mut stmt = conn.prepare(
                "SELECT item_key FROM folder_order_items WHERE item_key>=?1 AND item_key<?2",
            )?;
            stmt.query_map(params![prefix, upper], |row| row.get::<_, String>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?
        };
        for key in keys.into_iter().filter(|key| is_direct_child(folder, key)) {
            conn.execute("DELETE FROM folder_order_items WHERE item_key=?1", [key])?;
        }
        Ok(())
    }

    fn save_listing(&mut self, folder: &str, save: &FolderOrderSave) -> OrderResult<()> {
        let tx = self
            .conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let previous = Self::read_from(&tx, folder)?;
        let revision = previous
            .as_ref()
            .map_or(0, |record| record.revision)
            .checked_add(1)
            .ok_or_else(|| FolderOrderError::new("folder order revision overflow"))?;
        let listing = save
            .listing_order
            .iter()
            .map(|key| crate::path_key::normalize_keep_drive(Path::new(key)))
            .collect::<Vec<_>>();
        let inventory = save
            .inventory
            .iter()
            .map(|key| crate::path_key::normalize_keep_drive(Path::new(key)))
            .collect::<HashSet<_>>();
        let items = merge_inventory_order(
            folder,
            previous
                .as_ref()
                .map_or(&[], |record| record.items.as_slice()),
            &listing,
            &inventory,
        )?;
        Self::write_record(
            &tx,
            folder,
            &FolderOrderRecord {
                active: true,
                revision,
                items,
            },
        )?;
        tx.commit()?;
        Ok(())
    }

    fn write_record(
        conn: &Connection,
        folder: &str,
        record: &FolderOrderRecord,
    ) -> OrderResult<()> {
        let revision = i64::try_from(record.revision)
            .map_err(|_| FolderOrderError::new("folder order revision overflow"))?;
        let unique = record.items.iter().collect::<HashSet<_>>();
        if unique.len() != record.items.len()
            || !record.items.iter().all(|key| {
                is_direct_child(folder, key)
                    && crate::path_key::normalize_keep_drive(Path::new(key)) == *key
            })
        {
            return Err(FolderOrderError::new("invalid saved folder order"));
        }
        Self::delete_children(conn, folder)?;
        for (position, key) in record.items.iter().enumerate() {
            conn.execute(
                "INSERT INTO folder_order_items(item_key,position) VALUES(?1,?2)",
                params![key, position as i64],
            )?;
        }
        conn.execute("INSERT INTO folder_order_folders(folder_key,active,revision) VALUES(?1,?2,?3)
            ON CONFLICT(folder_key) DO UPDATE SET active=excluded.active,revision=excluded.revision",
            params![folder, record.active, revision])?;
        Ok(())
    }

    #[cfg(test)]
    fn save(&mut self, folder: &str, record: &FolderOrderRecord) -> OrderResult<()> {
        self.read(folder)?;
        let tx = self
            .conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        Self::write_record(&tx, folder, record)?;
        tx.commit()?;
        Ok(())
    }

    fn set_active(&mut self, folder: &str, active: bool) -> OrderResult<()> {
        let tx = self
            .conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let Some(record) = Self::read_from(&tx, folder)? else {
            return Ok(());
        };
        let revision = record
            .revision
            .checked_add(1)
            .ok_or_else(|| FolderOrderError::new("folder order revision overflow"))?;
        let revision = i64::try_from(revision)
            .map_err(|_| FolderOrderError::new("folder order revision overflow"))?;
        // A switch without a saved order must not create a record.
        tx.execute(
            "UPDATE folder_order_folders SET active=?1,revision=?2 WHERE folder_key=?3",
            params![active, revision, folder],
        )?;
        tx.commit()?;
        Ok(())
    }

    fn reset(&mut self, folder: &str) -> OrderResult<()> {
        self.read(folder)?;
        let tx = self
            .conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        Self::delete_children(&tx, folder)?;
        tx.execute(
            "DELETE FROM folder_order_folders WHERE folder_key=?1",
            [folder],
        )?;
        tx.commit()?;
        Ok(())
    }

    fn rename_mappings(&mut self, mappings: &[FolderOrderRename]) -> OrderResult<()> {
        if mappings
            .iter()
            .all(|mapping| mapping.old_key == mapping.new_key)
        {
            return Ok(());
        }
        let tx = self
            .conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        for (table, column, fields) in [
            ("folder_order_items", "item_key", "position"),
            ("folder_order_folders", "folder_key", "active,revision"),
        ] {
            let mut rows = Vec::new();
            for rename in mappings {
                for root in [&rename.old_key, &rename.new_key] {
                    let (prefix, upper) = prefix_bounds(root);
                    let query = if rename.scope == RenameScope::Tree {
                        format!(
                            "SELECT {column},{fields} FROM {table} WHERE {column}=?1 OR ({column}>=?2 AND {column}<?3)"
                        )
                    } else {
                        format!("SELECT {column},{fields} FROM {table} WHERE {column}=?1")
                    };
                    let mut stmt = tx.prepare(&query)?;
                    let parameters: Vec<&dyn rusqlite::ToSql> = if rename.scope == RenameScope::Tree
                    {
                        vec![root, &prefix, &upper]
                    } else {
                        vec![root]
                    };
                    let count = stmt.column_count();
                    rows.extend(
                        stmt.query_map(parameters.as_slice(), |row| {
                            Ok((
                                row.get::<_, String>(0)?,
                                (1..count)
                                    .map(|n| row.get::<_, rusqlite::types::Value>(n))
                                    .collect::<rusqlite::Result<Vec<_>>>()?,
                            ))
                        })?
                        .collect::<rusqlite::Result<Vec<_>>>()?,
                    );
                }
            }
            let mut seen = HashSet::new();
            rows.retain(|(key, _)| seen.insert(key.clone()));
            for (key, _) in &rows {
                tx.execute(&format!("DELETE FROM {table} WHERE {column}=?1"), [key])?;
            }
            for (key, values) in rows {
                if !mappings
                    .iter()
                    .any(|mapping| mapping.matches(&mapping.old_key, &key))
                {
                    continue;
                }
                if let Some(new) = rewrite_key_with_mappings(&key, mappings) {
                    let mut values = values;
                    values.insert(0, rusqlite::types::Value::Text(new));
                    let placeholders = vec!["?"; values.len()].join(",");
                    tx.execute(
                        &format!("INSERT INTO {table}({column},{fields}) VALUES({placeholders})"),
                        rusqlite::params_from_iter(values),
                    )?;
                }
            }
        }
        tx.commit()?;
        Ok(())
    }

    #[cfg(test)]
    fn rename(&mut self, rename: &FolderOrderRename) -> OrderResult<()> {
        self.rename_mappings(std::slice::from_ref(rename))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FolderOrderOperation {
    Save,
    SetActive,
    Reset,
    Rename,
}

#[derive(Clone, Debug)]
pub(crate) struct FolderOrderCompletion {
    /// Viewer context that submitted this write; readers have no completion.
    pub owner: Option<u64>,
    pub folder_key: String,
    pub revision: u64,
    pub operation: FolderOrderOperation,
    pub result: OrderResult<()>,
}

enum Request {
    Read {
        folder: String,
        reply: mpsc::Sender<OrderResult<Option<FolderOrderRecord>>>,
    },
    Save {
        folder: String,
        save: FolderOrderSave,
    },
    SetActive {
        folder: String,
        active: bool,
        revision: u64,
    },
    Reset {
        folder: String,
        revision: u64,
    },
    Rename {
        folder: String,
        revision: u64,
        mappings: Vec<FolderOrderRename>,
    },
}

struct QueuedRequest {
    request: Request,
    owner: Option<u64>,
}

/// A conservative, append-only superset of persisted and accepted folder keys.
/// Admission and startup publication use the queue mutex, so a later accepted
/// save/rename can never be lost when the actor publishes its initial DB keys.
#[derive(Default)]
struct RecordIndex {
    keys: HashSet<String>,
    ready: bool,
    startup_renames: Vec<Vec<FolderOrderRename>>,
}

impl RecordIndex {
    fn append_renamed_keys(keys: &mut HashSet<String>, mappings: &[FolderOrderRename]) {
        // Each batch maps the original snapshot simultaneously, including swaps
        // and nested source trees. Old keys deliberately remain in the index.
        let destinations = keys
            .iter()
            .filter_map(|key| rewrite_key_with_mappings(key, mappings))
            .collect::<Vec<_>>();
        keys.extend(destinations);
    }

    fn admit(&mut self, request: &Request) {
        match request {
            Request::Save { folder, .. } => {
                self.keys.insert(folder.clone());
            }
            Request::Rename { mappings, .. } => {
                Self::append_renamed_keys(&mut self.keys, mappings);
                if !self.ready {
                    self.startup_renames.push(mappings.clone());
                }
            }
            Request::Read { .. } | Request::SetActive { .. } | Request::Reset { .. } => {}
        }
    }

    fn publish(&mut self, mut persisted_keys: HashSet<String>) {
        for mappings in self.startup_renames.drain(..) {
            Self::append_renamed_keys(&mut persisted_keys, &mappings);
        }
        self.keys.extend(persisted_keys);
        self.ready = true;
    }
}

#[derive(Default)]
struct Queue {
    requests: VecDeque<QueuedRequest>,
    closed: bool,
    record_index: RecordIndex,
    #[cfg(test)]
    read_requests: usize,
}

impl Queue {
    #[cfg(test)]
    fn push(&mut self, request: Request) {
        self.push_owned(request, None);
    }

    fn push_owned(&mut self, request: Request, owner: Option<u64>) {
        self.record_index.admit(&request);
        #[cfg(test)]
        if matches!(&request, Request::Read { .. }) {
            self.read_requests += 1;
        }
        // Only adjacent saves may replace one another. Every read, reset, rename,
        // active switch and other-folder request is a FIFO barrier.
        if let (
            Some(QueuedRequest {
                request:
                    Request::Save {
                        folder: old,
                        save: previous,
                    },
                owner: previous_owner,
            }),
            Request::Save {
                folder: new,
                save: latest,
            },
        ) = (self.requests.back(), &request)
        {
            // A previous save may have first recorded a child that became hidden
            // (or dropped a child absent from that inventory). Such a save is a
            // meaningful transition, even before either request reaches SQLite.
            let same_projection = *previous_owner == owner
                && old == new
                && previous.inventory == latest.inventory
                && previous.listing_order.len() == latest.listing_order.len()
                && previous.listing_order.iter().collect::<HashSet<_>>()
                    == latest.listing_order.iter().collect::<HashSet<_>>();
            if same_projection {
                self.requests.pop_back();
            }
        }
        self.requests.push_back(QueuedRequest { request, owner });
    }
}

type SharedQueue = Arc<(Mutex<Queue>, Condvar)>;

struct Client {
    queue: SharedQueue,
}
impl Drop for Client {
    fn drop(&mut self) {
        let (lock, wake) = &*self.queue;
        lock.lock().unwrap_or_else(|e| e.into_inner()).closed = true;
        wake.notify_one();
    }
}

#[derive(Clone)]
pub(crate) struct FolderOrderHandle {
    client: Arc<Client>,
    owner: Option<u64>,
}

impl FolderOrderHandle {
    /// The UI may skip an actor read only after startup proved this folder has
    /// no record. This lookup performs no I/O and never waits for actor work.
    pub(crate) fn record_is_known_absent(&self, folder: &Path) -> bool {
        let key = folder_key(folder);
        let queue = self
            .client
            .queue
            .0
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        queue.record_index.ready && !queue.record_index.keys.contains(&key)
    }

    #[cfg(test)]
    pub(crate) fn index_ready_for_test(&self) -> bool {
        self.client
            .queue
            .0
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .record_index
            .ready
    }

    #[cfg(test)]
    pub(crate) fn read_request_count_for_test(&self) -> usize {
        self.client
            .queue
            .0
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .read_requests
    }

    /// Bind only writes submitted through this clone to their owning viewer.
    /// The token is context identity, independent of folder and order revision.
    pub(crate) fn for_owner(&self, owner: u64) -> Self {
        Self {
            client: self.client.clone(),
            owner: Some(owner),
        }
    }

    fn submit(&self, request: Request) -> OrderResult<()> {
        let (lock, wake) = &*self.client.queue;
        let mut queue = lock.lock().unwrap_or_else(|e| e.into_inner());
        if queue.closed {
            return Err(FolderOrderError::new("folder order actor stopped"));
        }
        queue.push_owned(request, self.owner);
        wake.notify_one();
        Ok(())
    }

    /// Receiver belongs to this read only. Waiting is permitted on a worker only.
    pub(crate) fn read(
        &self,
        folder: &Path,
    ) -> OrderResult<mpsc::Receiver<OrderResult<Option<FolderOrderRecord>>>> {
        let (reply, rx) = mpsc::channel();
        self.submit(Request::Read {
            folder: folder_key(folder),
            reply,
        })?;
        Ok(rx)
    }
    pub(crate) fn save(&self, folder: &Path, save: FolderOrderSave) -> OrderResult<()> {
        self.submit(Request::Save {
            folder: folder_key(folder),
            save,
        })
    }
    pub(crate) fn set_active(&self, folder: &Path, active: bool, revision: u64) -> OrderResult<()> {
        self.submit(Request::SetActive {
            folder: folder_key(folder),
            active,
            revision,
        })
    }
    pub(crate) fn reset(&self, folder: &Path, revision: u64) -> OrderResult<()> {
        self.submit(Request::Reset {
            folder: folder_key(folder),
            revision,
        })
    }
    pub(crate) fn rename(
        &self,
        folder: &Path,
        revision: u64,
        mappings: Vec<FolderOrderRename>,
    ) -> OrderResult<()> {
        self.submit(Request::Rename {
            folder: folder_key(folder),
            revision,
            mappings,
        })
    }
}

pub(crate) struct FolderOrderService {
    pub handle: FolderOrderHandle,
    pub completions: mpsc::Receiver<FolderOrderCompletion>,
    worker: Option<std::thread::JoinHandle<()>>,
}

impl FolderOrderService {
    pub(crate) fn start() -> std::io::Result<Self> {
        Self::start_at(crate::data_dir::get().join(DB_NAME))
    }
    /// Opening and schema setup happen inside the actor, never on the caller.
    pub(crate) fn start_at(path: PathBuf) -> std::io::Result<Self> {
        Self::start_at_before_open(path, || {})
    }

    #[cfg(test)]
    pub(crate) fn start_at_paused_for_test(
        path: PathBuf,
    ) -> std::io::Result<(Self, mpsc::Sender<()>)> {
        let (release, gate) = mpsc::channel();
        let service = Self::start_at_before_open(path, move || {
            let _ = gate.recv();
        })?;
        Ok((service, release))
    }

    fn start_at_before_open(
        path: PathBuf,
        before_open: impl FnOnce() + Send + 'static,
    ) -> std::io::Result<Self> {
        let queue = Arc::new((Mutex::new(Queue::default()), Condvar::new()));
        let (tx, completions) = mpsc::channel();
        let worker_queue = queue.clone();
        let worker = std::thread::Builder::new()
            .name("folder-order".into())
            .spawn(move || {
                before_open();
                run_actor(worker_queue, tx, path);
            })?;
        Ok(Self {
            handle: FolderOrderHandle {
                client: Arc::new(Client { queue }),
                owner: None,
            },
            completions,
            worker: Some(worker),
        })
    }

    /// Final-exit / worker / test boundary only. Ordinary UI drop never joins.
    /// Close admission, drain already accepted FIFO requests, and release the DB.
    pub(crate) fn shutdown(mut self) {
        let (lock, wake) = &*self.handle.client.queue;
        lock.lock()
            .unwrap_or_else(|error| error.into_inner())
            .closed = true;
        wake.notify_one();
        if let Some(worker) = self.worker.take() {
            if worker.join().is_err() {
                crate::logger::log("folder_order: worker failed during shutdown".to_owned());
            }
        }
    }
}

/// A panic drops queued read senders and makes later submits fail immediately.
struct ActorGuard(SharedQueue);
impl Drop for ActorGuard {
    fn drop(&mut self) {
        let (lock, _) = &*self.0;
        let mut queue = lock.lock().unwrap_or_else(|e| e.into_inner());
        queue.closed = true;
        queue.requests.clear();
    }
}

fn run_actor(queue: SharedQueue, completions: mpsc::Sender<FolderOrderCompletion>, path: PathBuf) {
    let _guard = ActorGuard(queue.clone());
    let mut db = FolderOrderDb::open_at(&path);
    if let Err(error) = &db {
        crate::logger::log(format!("folder_order: open failed: {error}"));
    }
    // No request may execute before the complete initial key set is available.
    // Failure leaves the index unready and all requests receive the same typed
    // startup error; do not retry, recreate data, or guess that a record is absent.
    if let Ok(opened) = &db {
        match opened.folder_keys() {
            Ok(keys) => queue
                .0
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .record_index
                .publish(keys),
            Err(error) => {
                crate::logger::log(format!("folder_order: index read failed: {error}"));
                db = Err(error);
            }
        }
    }
    let mut blocked = HashMap::<String, FolderOrderError>::new();
    loop {
        let QueuedRequest { request, owner } = {
            let (lock, wake) = &*queue;
            let mut pending = lock.lock().unwrap_or_else(|e| e.into_inner());
            while pending.requests.is_empty() && !pending.closed {
                pending = wake.wait(pending).unwrap_or_else(|e| e.into_inner());
            }
            let Some(request) = pending.requests.pop_front() else {
                break;
            };
            request
        };
        let started = Instant::now();
        let (folder, revision, operation, result) = match request {
            Request::Read { folder, reply } => {
                let result = match &db {
                    Ok(db) => db.read(&folder),
                    Err(error) => Err(error.clone()),
                };
                if let Err(error) = &result {
                    blocked.insert(folder.clone(), error.clone());
                    crate::logger::log(format!("folder_order: read failed for {folder}: {error}"));
                }
                if crate::perf::is_enabled() {
                    crate::perf::event(
                        "folder_order",
                        "read",
                        Some(&folder),
                        0,
                        &[
                            (
                                "ms",
                                serde_json::json!(started.elapsed().as_secs_f64() * 1000.0),
                            ),
                            ("ok", serde_json::json!(result.is_ok())),
                        ],
                    );
                }
                let _ = reply.send(result);
                continue;
            }
            Request::Save { folder, save } => {
                let result = write_if_ready(&mut db, &blocked, &folder, |db| {
                    db.save_listing(&folder, &save)
                });
                (folder, save.revision, FolderOrderOperation::Save, result)
            }
            Request::SetActive {
                folder,
                active,
                revision,
            } => {
                let result = write_if_ready(&mut db, &blocked, &folder, |db| {
                    db.set_active(&folder, active)
                });
                (folder, revision, FolderOrderOperation::SetActive, result)
            }
            Request::Reset { folder, revision } => {
                let result = write_if_ready(&mut db, &blocked, &folder, |db| db.reset(&folder));
                (folder, revision, FolderOrderOperation::Reset, result)
            }
            Request::Rename {
                folder,
                revision,
                mappings,
            } => {
                let result = write_if_ready(&mut db, &blocked, &folder, |db| {
                    db.rename_mappings(&mappings)
                });
                // Keep unreadable folders blocked under their new identities too.
                blocked = blocked
                    .into_iter()
                    .filter_map(|(key, error)| {
                        rewrite_key_with_mappings(&key, &mappings).map(|key| (key, error))
                    })
                    .collect();
                (folder, revision, FolderOrderOperation::Rename, result)
            }
        };
        if let Err(error) = &result {
            crate::logger::log(format!(
                "folder_order: {operation:?} failed for {folder} revision {revision}: {error}"
            ));
        }
        if crate::perf::is_enabled() {
            crate::perf::event(
                "folder_order",
                "write",
                Some(&folder),
                0,
                &[
                    (
                        "ms",
                        serde_json::json!(started.elapsed().as_secs_f64() * 1000.0),
                    ),
                    ("revision", serde_json::json!(revision)),
                    ("operation", serde_json::json!(format!("{operation:?}"))),
                    ("ok", serde_json::json!(result.is_ok())),
                ],
            );
        }
        let _ = completions.send(FolderOrderCompletion {
            owner,
            folder_key: folder,
            revision,
            operation,
            result,
        });
    }
}

fn write_if_ready(
    db: &mut OrderResult<FolderOrderDb>,
    blocked: &HashMap<String, FolderOrderError>,
    folder: &str,
    write: impl FnOnce(&mut FolderOrderDb) -> OrderResult<()>,
) -> OrderResult<()> {
    if let Some(error) = blocked.get(folder) {
        return Err(error.clone());
    }
    match db {
        Ok(db) => write(db),
        Err(error) => Err(error.clone()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys(values: &[&str]) -> Vec<String> {
        values
            .iter()
            .map(|value| crate::path_key::normalize_keep_drive(Path::new(value)))
            .collect()
    }

    fn inventory(values: &[&str]) -> HashSet<String> {
        keys(values).into_iter().collect()
    }

    fn record(revision: u64, values: &[&str]) -> FolderOrderRecord {
        FolderOrderRecord {
            active: true,
            revision,
            items: keys(values),
        }
    }

    fn save_data(revision: u64, values: &[&str]) -> FolderOrderSave {
        FolderOrderSave {
            revision,
            listing_order: keys(values),
            inventory: inventory(values),
        }
    }

    fn db() -> FolderOrderDb {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE folder_order_items(item_key TEXT PRIMARY KEY,position INTEGER NOT NULL);
            CREATE TABLE folder_order_folders(folder_key TEXT PRIMARY KEY,active INTEGER NOT NULL,revision INTEGER NOT NULL);
            PRAGMA user_version=1;").unwrap();
        FolderOrderDb { conn }
    }

    struct ActorTest {
        service: Option<FolderOrderService>,
        worker: Option<std::thread::JoinHandle<()>>,
        temp: tempfile::TempDir,
    }

    impl ActorTest {
        fn new(requests: Vec<Request>) -> Self {
            let temp = tempfile::tempdir().unwrap();
            let queue = Arc::new((Mutex::new(Queue::default()), Condvar::new()));
            for request in requests {
                queue.0.lock().unwrap().push(request);
            }
            let (tx, completions) = mpsc::channel();
            let path = temp.path().join(DB_NAME);
            let worker_queue = queue.clone();
            let worker = std::thread::spawn(move || run_actor(worker_queue, tx, path));
            Self {
                service: Some(FolderOrderService {
                    handle: FolderOrderHandle {
                        client: Arc::new(Client { queue }),
                        owner: None,
                    },
                    completions,
                    worker: None,
                }),
                worker: Some(worker),
                temp,
            }
        }
        fn handle(&self) -> &FolderOrderHandle {
            &self.service.as_ref().unwrap().handle
        }
        fn read(&self, folder: &str) -> OrderResult<Option<FolderOrderRecord>> {
            self.handle()
                .read(Path::new(folder))
                .unwrap()
                .recv_timeout(Duration::from_secs(10))
                .unwrap()
        }
        fn completion(&self) -> FolderOrderCompletion {
            self.service
                .as_ref()
                .unwrap()
                .completions
                .recv_timeout(Duration::from_secs(10))
                .unwrap()
        }
        fn close(&mut self) {
            self.service.take();
            if let Some(worker) = self.worker.take() {
                worker.join().unwrap();
            }
        }
    }

    impl Drop for ActorTest {
        fn drop(&mut self) {
            self.close();
        }
    }

    fn save_request(folder: &str, revision: u64, values: &[&str]) -> Request {
        Request::Save {
            folder: folder_key(Path::new(folder)),
            save: save_data(revision, values),
        }
    }

    #[test]
    fn folder_order_prefix_covers_drive_unc_and_trailing_separators_once() {
        for (input, expected) in [
            (r"C:\", "c:/"),
            ("C:/", "c:/"),
            ("C:/album///", "c:/album/"),
            (r"\\Server\Share", "//server/share/"),
            (r"\\Server\Share\", "//server/share/"),
        ] {
            let key = folder_key(Path::new(input));
            assert_eq!(direct_child_prefix(&key), expected);
            assert!(is_direct_child(&key, &format!("{expected}a.jpg")));
            assert!(!is_direct_child(&key, &format!("{expected}sub/a.jpg")));
        }
    }

    #[test]
    fn folder_order_merge_visible_preserves_facet_hidden_slots() {
        let full = keys(&["c:/f/a", "c:/f/hidden", "c:/f/b"]);
        let visible = inventory(&["c:/f/a", "c:/f/b"]);
        assert_eq!(
            merge_visible_order(&full, &visible, &keys(&["c:/f/b", "c:/f/a"])).unwrap(),
            keys(&["c:/f/b", "c:/f/hidden", "c:/f/a"])
        );
        assert_eq!(
            merge_visible_order(&full, &HashSet::new(), &[]).unwrap(),
            full
        );
    }

    #[test]
    fn folder_order_merge_visible_rejects_duplicate_missing_and_foreign_keys() {
        let full = keys(&["c:/f/a", "c:/f/b"]);
        let visible = inventory(&["c:/f/a", "c:/f/b"]);
        for bad in [
            keys(&["c:/f/a", "c:/f/a"]),
            keys(&["c:/f/a"]),
            keys(&["c:/f/a", "c:/f/z"]),
        ] {
            assert!(merge_visible_order(&full, &visible, &bad).is_err());
        }
        assert!(merge_visible_order(&keys(&["c:/f/a", "c:/f/a"]), &visible, &full).is_err());
    }

    #[test]
    fn folder_order_inventory_merge_preserves_type_hidden_and_deduplicated_children() {
        let stored = keys(&[
            "c:/f/a",
            "c:/f/type-hidden",
            "c:/f/b",
            "c:/f/deduplicated",
            "c:/f/gone",
        ]);
        let listing = keys(&["c:/f/b", "c:/f/new", "c:/f/a"]);
        let current = inventory(&[
            "c:/f/a",
            "c:/f/b",
            "c:/f/new",
            "c:/f/type-hidden",
            "c:/f/deduplicated",
            "c:/f/hidden-unknown",
        ]);
        let merged = merge_inventory_order("c:/f", &stored, &listing, &current).unwrap();
        assert_eq!(
            merged,
            keys(&[
                "c:/f/b",
                "c:/f/type-hidden",
                "c:/f/new",
                "c:/f/deduplicated",
                "c:/f/a"
            ])
        );
        assert!(!merged.contains(&"c:/f/hidden-unknown".into()));
        assert!(!merged.contains(&"c:/f/gone".into()));
    }

    #[test]
    fn folder_order_inventory_first_reorder_records_listing_only() {
        let listing = keys(&["c:/f/b", "c:/f/a"]);
        assert_eq!(
            merge_inventory_order(
                "c:/f",
                &[],
                &listing,
                &inventory(&["c:/f/a", "c:/f/b", "c:/f/hidden"])
            )
            .unwrap(),
            listing
        );
    }

    #[test]
    fn folder_order_save_merges_stored_hidden_inventory_and_owns_revision_and_active() {
        let mut db = db();
        db.save_listing(
            "c:/f",
            &FolderOrderSave {
                revision: 90,
                listing_order: keys(&["c:/f/a", "c:/f/hidden", "c:/f/b", "c:/f/gone"]),
                inventory: inventory(&[
                    "c:/f/a",
                    "c:/f/hidden",
                    "c:/f/b",
                    "c:/f/gone",
                    "c:/f/unknown-hidden",
                ]),
            },
        )
        .unwrap();
        assert_eq!(
            db.read("c:/f").unwrap(),
            Some(record(1, &["c:/f/a", "c:/f/hidden", "c:/f/b", "c:/f/gone"]))
        );
        db.set_active("c:/f", false).unwrap();
        assert!(!db.read("c:/f").unwrap().unwrap().active);
        db.save_listing(
            "c:/f",
            &FolderOrderSave {
                revision: 100,
                listing_order: keys(&["c:/f/b", "c:/f/a"]),
                inventory: inventory(&["c:/f/a", "c:/f/hidden", "c:/f/b", "c:/f/unknown-hidden"]),
            },
        )
        .unwrap();
        assert_eq!(
            db.read("c:/f").unwrap(),
            Some(record(3, &["c:/f/b", "c:/f/hidden", "c:/f/a"]))
        );
    }

    #[test]
    fn folder_order_failed_save_rolls_back_original_order_and_revision() {
        let mut db = db();
        db.save_listing("c:/f", &save_data(1, &["c:/f/a", "c:/f/b"]))
            .unwrap();
        db.conn
            .execute_batch(
                "CREATE TRIGGER reject_folder_order_insert BEFORE INSERT ON folder_order_items
            WHEN NEW.item_key='c:/f/fail' BEGIN SELECT RAISE(ABORT, 'test write failure'); END;",
            )
            .unwrap();
        assert!(
            db.save_listing("c:/f", &save_data(2, &["c:/f/fail"]))
                .is_err()
        );
        assert_eq!(
            db.read("c:/f").unwrap(),
            Some(record(1, &["c:/f/a", "c:/f/b"]))
        );
    }

    #[test]
    fn folder_order_inventory_rejects_foreign_and_duplicate_children() {
        let listing = keys(&["c:/f/a"]);
        assert!(merge_inventory_order("c:/f", &[], &listing, &HashSet::new()).is_err());
        assert!(
            merge_inventory_order("c:/f", &[], &listing, &inventory(&["c:/f/a", "c:/f/sub/a"]))
                .is_err()
        );
        assert!(
            merge_inventory_order(
                "c:/f",
                &[],
                &keys(&["c:/f/a", "c:/f/a"]),
                &inventory(&["c:/f/a"])
            )
            .is_err()
        );
    }

    #[test]
    fn folder_order_schema_opens_wal_and_reopens_without_changes() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("nested").join(DB_NAME);
        {
            let mut db = FolderOrderDb::open_at(&path).unwrap();
            assert_eq!(
                db.conn
                    .pragma_query_value(None, "journal_mode", |row| row.get::<_, String>(0))
                    .unwrap(),
                "wal"
            );
            assert_eq!(
                db.conn
                    .pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))
                    .unwrap(),
                1
            );
            db.save("c:/f", &record(7, &["c:/f/b", "c:/f/a"])).unwrap();
        }
        assert_eq!(
            FolderOrderDb::open_at(&path).unwrap().read("c:/f").unwrap(),
            Some(record(7, &["c:/f/b", "c:/f/a"]))
        );
    }

    #[test]
    fn folder_order_unknown_schema_keeps_original_data() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join(DB_NAME);
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch("CREATE TABLE original(value TEXT); INSERT INTO original VALUES('keep'); PRAGMA user_version=99;").unwrap();
        }
        assert!(FolderOrderDb::open_at(&path).is_err());
        let conn = Connection::open(&path).unwrap();
        assert_eq!(
            conn.query_row("SELECT value FROM original", [], |row| row
                .get::<_, String>(0))
                .unwrap(),
            "keep"
        );
        assert_eq!(
            conn.pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))
                .unwrap(),
            99
        );
    }

    #[test]
    fn folder_order_read_does_not_write_and_set_active_does_not_start_recording() {
        let mut db = db();
        assert_eq!(db.read("c:/f").unwrap(), None);
        db.set_active("c:/f", true).unwrap();
        assert_eq!(db.read("c:/f").unwrap(), None);
        assert_eq!(
            db.conn
                .query_row("SELECT total_changes()", [], |row| row.get::<_, i64>(0))
                .unwrap(),
            0
        );
        db.save("c:/f", &record(2, &["c:/f/a"])).unwrap();
        db.set_active("c:/f", false).unwrap();
        assert_eq!(
            db.read("c:/f").unwrap(),
            Some(FolderOrderRecord {
                active: false,
                revision: 3,
                items: keys(&["c:/f/a"])
            })
        );
        db.set_active("c:/f", true).unwrap();
        assert_eq!(db.read("c:/f").unwrap(), Some(record(4, &["c:/f/a"])));
    }

    #[test]
    fn folder_order_save_and_reset_touch_direct_children_only_with_literal_wildcards() {
        let mut db = db();
        db.save("c:/f%_", &record(1, &["c:/f%_/a", "c:/f%_/b"]))
            .unwrap();
        db.save("c:/f%_/sub", &record(2, &["c:/f%_/sub/a"]))
            .unwrap();
        db.save("c:/fxx", &record(3, &["c:/fxx/a"])).unwrap();
        db.save("c:/f%_", &record(4, &["c:/f%_/b"])).unwrap();
        assert_eq!(db.read("c:/f%_").unwrap(), Some(record(4, &["c:/f%_/b"])));
        assert_eq!(
            db.read("c:/f%_/sub").unwrap(),
            Some(record(2, &["c:/f%_/sub/a"]))
        );
        db.reset("c:/f%_").unwrap();
        assert_eq!(db.read("c:/f%_").unwrap(), None);
        assert_eq!(
            db.read("c:/f%_/sub").unwrap(),
            Some(record(2, &["c:/f%_/sub/a"]))
        );
        assert_eq!(db.read("c:/fxx").unwrap(), Some(record(3, &["c:/fxx/a"])));
    }

    #[test]
    fn folder_order_root_reads_saves_and_resets_use_same_prefix() {
        for root in ["c:/", "//server/share"] {
            let mut db = db();
            let child = format!("{}a", direct_child_prefix(root));
            db.save(
                root,
                &FolderOrderRecord {
                    active: true,
                    revision: 1,
                    items: vec![child.clone()],
                },
            )
            .unwrap();
            assert_eq!(db.read(root).unwrap().unwrap().items, vec![child]);
            db.reset(root).unwrap();
            assert_eq!(db.read(root).unwrap(), None);
            assert_eq!(
                db.conn
                    .query_row("SELECT count(*) FROM folder_order_items", [], |r| r
                        .get::<_, i64>(0))
                    .unwrap(),
                0
            );
        }
    }

    #[test]
    fn folder_order_unreadable_record_is_not_overwritten_or_reset() {
        let mut db = db();
        db.save("c:/f", &record(1, &["c:/f/a", "c:/f/b"])).unwrap();
        db.conn
            .execute(
                "UPDATE folder_order_items SET position=0 WHERE item_key='c:/f/b'",
                [],
            )
            .unwrap();
        assert!(db.read("c:/f").is_err());
        assert!(db.save("c:/f", &record(2, &["c:/f/a"])).is_err());
        assert!(db.set_active("c:/f", false).is_err());
        assert!(db.reset("c:/f").is_err());
        assert_eq!(
            db.conn
                .query_row("SELECT count(*) FROM folder_order_items", [], |r| r
                    .get::<_, i64>(0))
                .unwrap(),
            2
        );
    }

    #[test]
    fn folder_order_key_rewrite_is_source_wins_and_case_only_is_noop() {
        let exact =
            FolderOrderRename::new(Path::new("C:/f/A"), Path::new("c:/f/B"), RenameScope::Exact);
        assert_eq!(rewrite_key("c:/f/a", &exact), Some("c:/f/b".into()));
        assert_eq!(rewrite_key("c:/f/b", &exact), None);
        assert_eq!(rewrite_key("c:/f/b/sub", &exact), Some("c:/f/b/sub".into()));
        let case =
            FolderOrderRename::new(Path::new("C:/F/A"), Path::new("c:/f/a"), RenameScope::Tree);
        assert_eq!(rewrite_key("c:/f/a/x", &case), Some("c:/f/a/x".into()));
        let tree =
            FolderOrderRename::new(Path::new("c:/old"), Path::new("c:/new"), RenameScope::Tree);
        assert_eq!(rewrite_key("c:/old/a", &tree), Some("c:/new/a".into()));
        assert_eq!(rewrite_key("c:/older/a", &tree), Some("c:/older/a".into()));
        assert_eq!(rewrite_key("c:/new/a", &tree), None);
        let drive =
            FolderOrderRename::new(Path::new("c:/"), Path::new("d:/target"), RenameScope::Tree);
        assert_eq!(rewrite_key("c:/a", &drive), Some("d:/target/a".into()));
    }

    #[test]
    fn folder_order_exact_rename_replaces_stale_destination_position() {
        let mut db = db();
        db.save("c:/f", &record(1, &["c:/f/a", "c:/f/z", "c:/f/b"]))
            .unwrap();
        db.rename(&FolderOrderRename::new(
            Path::new("c:/f/a"),
            Path::new("c:/f/b"),
            RenameScope::Exact,
        ))
        .unwrap();
        assert_eq!(
            db.read("c:/f").unwrap(),
            Some(record(1, &["c:/f/b", "c:/f/z"]))
        );
    }

    #[test]
    fn folder_order_missing_exact_source_discards_stale_destination() {
        let mut db = db();
        db.save("c:/f", &record(1, &["c:/f/z", "c:/f/b"])).unwrap();
        db.rename(&FolderOrderRename::new(
            Path::new("c:/f/a"),
            Path::new("c:/f/b"),
            RenameScope::Exact,
        ))
        .unwrap();
        assert_eq!(db.read("c:/f").unwrap(), Some(record(1, &["c:/f/z"])));
    }

    #[test]
    fn folder_order_tree_rename_moves_metadata_and_descendants_source_wins() {
        let mut db = db();
        db.save(
            "c:/parent",
            &record(1, &["c:/parent/old", "c:/parent/z", "c:/parent/new"]),
        )
        .unwrap();
        db.save("c:/parent/old", &record(2, &["c:/parent/old/a"]))
            .unwrap();
        db.save("c:/parent/old/sub", &record(3, &["c:/parent/old/sub/x"]))
            .unwrap();
        db.save("c:/parent/new", &record(4, &["c:/parent/new/stale"]))
            .unwrap();
        db.save(
            "c:/parent/new/stale-sub",
            &record(5, &["c:/parent/new/stale-sub/y"]),
        )
        .unwrap();
        db.save("c:/parent/newer", &record(6, &["c:/parent/newer/sibling"]))
            .unwrap();
        db.rename(&FolderOrderRename::new(
            Path::new("c:/parent/old"),
            Path::new("c:/parent/new"),
            RenameScope::Tree,
        ))
        .unwrap();
        assert_eq!(
            db.read("c:/parent").unwrap(),
            Some(record(1, &["c:/parent/new", "c:/parent/z"]))
        );
        assert_eq!(
            db.read("c:/parent/new").unwrap(),
            Some(record(2, &["c:/parent/new/a"]))
        );
        assert_eq!(
            db.read("c:/parent/new/sub").unwrap(),
            Some(record(3, &["c:/parent/new/sub/x"]))
        );
        assert_eq!(db.read("c:/parent/old").unwrap(), None);
        assert_eq!(db.read("c:/parent/old/sub").unwrap(), None);
        assert_eq!(db.read("c:/parent/new/stale-sub").unwrap(), None);
        assert_eq!(
            db.read("c:/parent/newer").unwrap(),
            Some(record(6, &["c:/parent/newer/sibling"]))
        );
    }

    #[test]
    fn folder_order_missing_tree_source_discards_entire_stale_destination() {
        let mut db = db();
        db.save("c:/new", &record(1, &["c:/new/a"])).unwrap();
        db.save("c:/new/sub", &record(2, &["c:/new/sub/b"]))
            .unwrap();
        db.rename(&FolderOrderRename::new(
            Path::new("c:/old"),
            Path::new("c:/new"),
            RenameScope::Tree,
        ))
        .unwrap();
        assert_eq!(db.read("c:/new").unwrap(), None);
        assert_eq!(db.read("c:/new/sub").unwrap(), None);
        assert_eq!(
            db.conn
                .query_row("SELECT count(*) FROM folder_order_items", [], |r| r
                    .get::<_, i64>(0))
                .unwrap(),
            0
        );
    }

    #[test]
    fn folder_order_case_only_rename_preserves_order_and_revision() {
        let mut db = db();
        db.save_listing("c:/f", &save_data(1, &["c:/f/a", "c:/f/z"]))
            .unwrap();
        db.rename(&FolderOrderRename::new(
            Path::new("C:/F/A"),
            Path::new("c:/f/a"),
            RenameScope::Exact,
        ))
        .unwrap();
        db.rename(&FolderOrderRename::new(
            Path::new("C:/F"),
            Path::new("c:/f"),
            RenameScope::Tree,
        ))
        .unwrap();
        assert_eq!(
            db.read("c:/f").unwrap(),
            Some(record(1, &["c:/f/a", "c:/f/z"]))
        );
    }

    #[test]
    fn folder_order_rename_batch_uses_source_snapshot_for_file_and_tree_swaps() {
        let mut db = db();
        db.save("c:/f", &record(1, &["c:/f/a", "c:/f/z", "c:/f/b"]))
            .unwrap();
        let file_swap = vec![
            FolderOrderRename::new(Path::new("c:/f/a"), Path::new("c:/f/b"), RenameScope::Exact),
            FolderOrderRename::new(Path::new("c:/f/b"), Path::new("c:/f/a"), RenameScope::Exact),
        ];
        let expected = record(1, &["c:/f/b", "c:/f/z", "c:/f/a"]);
        assert_eq!(
            keys(&["c:/f/a", "c:/f/z", "c:/f/b"])
                .iter()
                .filter_map(|key| rewrite_key_with_mappings(key, &file_swap))
                .collect::<Vec<_>>(),
            expected.items
        );
        db.rename_mappings(&file_swap).unwrap();
        assert_eq!(db.read("c:/f").unwrap(), Some(expected));
        db.save("c:/left", &record(4, &["c:/left/a"])).unwrap();
        db.save("c:/right", &record(5, &["c:/right/b"])).unwrap();
        db.rename_mappings(&[
            FolderOrderRename::new(
                Path::new("c:/left"),
                Path::new("c:/right"),
                RenameScope::Tree,
            ),
            FolderOrderRename::new(
                Path::new("c:/right"),
                Path::new("c:/left"),
                RenameScope::Tree,
            ),
        ])
        .unwrap();
        assert_eq!(
            db.read("c:/right").unwrap(),
            Some(record(4, &["c:/right/a"]))
        );
        assert_eq!(db.read("c:/left").unwrap(), Some(record(5, &["c:/left/b"])));
    }

    #[test]
    fn folder_order_rename_batch_failure_rolls_back_both_tables() {
        let mut db = db();
        db.save("c:/old", &record(1, &["c:/old/a"])).unwrap();
        db.conn
            .execute_batch(
                "CREATE TRIGGER reject_folder_rename BEFORE INSERT ON folder_order_folders
            WHEN NEW.folder_key='c:/new' BEGIN SELECT RAISE(ABORT, 'test rename failure'); END;",
            )
            .unwrap();
        assert!(
            db.rename(&FolderOrderRename::new(
                Path::new("c:/old"),
                Path::new("c:/new"),
                RenameScope::Tree
            ))
            .is_err()
        );
        assert_eq!(db.read("c:/old").unwrap(), Some(record(1, &["c:/old/a"])));
        assert_eq!(db.read("c:/new").unwrap(), None);
    }

    #[test]
    fn folder_order_inventory_after_delete_and_rename_does_not_restore_old_positions() {
        let mut db = db();
        let rename =
            FolderOrderRename::new(Path::new("c:/f/a"), Path::new("c:/f/b"), RenameScope::Exact);
        let original = keys(&["c:/f/a", "c:/f/z", "c:/f/b", "c:/f/deleted"]);
        let rewritten = original
            .iter()
            .filter(|key| key.as_str() != "c:/f/deleted")
            .filter_map(|key| rewrite_key(key, &rename))
            .collect::<Vec<_>>();
        let current = inventory(&["c:/f/b", "c:/f/z"]);
        let merged =
            merge_inventory_order("c:/f", &rewritten, &keys(&["c:/f/z", "c:/f/b"]), &current)
                .unwrap();
        db.save(
            "c:/f",
            &FolderOrderRecord {
                active: true,
                revision: 2,
                items: merged,
            },
        )
        .unwrap();
        assert_eq!(
            db.read("c:/f").unwrap(),
            Some(record(2, &["c:/f/z", "c:/f/b"]))
        );
    }

    #[test]
    fn folder_order_actor_save_then_read_returns_saved_order_and_correlated_completion() {
        let actor = ActorTest::new(vec![]);
        actor
            .handle()
            .save(Path::new("C:/F/"), save_data(9, &["C:/F/b", "C:/F/a"]))
            .unwrap();
        assert_eq!(
            actor.read("c:/f").unwrap(),
            Some(record(1, &["c:/f/b", "c:/f/a"]))
        );
        let done = actor.completion();
        assert_eq!(done.owner, None);
        assert_eq!(
            (done.folder_key.as_str(), done.revision, done.operation),
            ("c:/f", 9, FolderOrderOperation::Save)
        );
        assert!(done.result.is_ok());
    }

    #[test]
    fn folder_order_actor_coalesces_only_adjacent_unexecuted_same_folder_saves() {
        let actor = ActorTest::new(vec![
            save_request("c:/a", 1, &["c:/a/one", "c:/a/two"]),
            save_request("c:/a", 2, &["c:/a/two", "c:/a/one"]),
            save_request("c:/b", 3, &["c:/b/item"]),
            save_request("c:/a", 4, &["c:/a/final"]),
        ]);
        assert_eq!(
            actor.read("c:/a").unwrap(),
            Some(record(2, &["c:/a/final"]))
        );
        assert_eq!(actor.read("c:/b").unwrap(), Some(record(1, &["c:/b/item"])));
        assert_eq!(
            [
                actor.completion().revision,
                actor.completion().revision,
                actor.completion().revision
            ],
            [2, 3, 4]
        );
        assert!(
            actor
                .service
                .as_ref()
                .unwrap()
                .completions
                .try_recv()
                .is_err()
        );
    }

    #[test]
    fn folder_order_owned_handles_preserve_completion_owner_and_do_not_coalesce_sibling_contexts() {
        let temp = tempfile::tempdir().unwrap();
        let queue = Arc::new((Mutex::new(Queue::default()), Condvar::new()));
        let handle = FolderOrderHandle {
            client: Arc::new(Client {
                queue: queue.clone(),
            }),
            owner: None,
        };
        let first = handle.for_owner(11);
        let sibling = handle.for_owner(22);
        let folder = Path::new("c:/f");
        first
            .save(folder, save_data(1, &["c:/f/a", "c:/f/b"]))
            .unwrap();
        first
            .save(folder, save_data(2, &["c:/f/b", "c:/f/a"]))
            .unwrap();
        // Same folder and same per-context revision still belong to another list.
        sibling
            .save(folder, save_data(2, &["c:/f/a", "c:/f/b"]))
            .unwrap();
        assert_eq!(queue.0.lock().unwrap().requests.len(), 2);
        first.set_active(folder, false, 3).unwrap();
        sibling.reset(folder, 3).unwrap();
        // Run the prefilled FIFO deterministically; no actor scheduling determines
        // whether adjacent saves were available for coalescing in this test.
        queue.0.lock().unwrap().closed = true;
        let (tx, completions) = mpsc::channel();
        run_actor(queue, tx, temp.path().join(DB_NAME));
        let done = completions
            .try_iter()
            .map(|completion| {
                assert!(completion.result.is_ok());
                (completion.owner, completion.revision, completion.operation)
            })
            .collect::<Vec<_>>();
        assert_eq!(
            done,
            [
                (Some(11), 2, FolderOrderOperation::Save),
                (Some(22), 2, FolderOrderOperation::Save),
                (Some(11), 3, FolderOrderOperation::SetActive),
                (Some(22), 3, FolderOrderOperation::Reset),
            ]
        );
        assert_eq!(handle.owner, None);
    }

    #[test]
    fn folder_order_actor_projection_or_inventory_change_preserves_prior_save_transition() {
        let actor = ActorTest::new(vec![
            save_request("c:/f", 1, &["c:/f/a", "c:/f/hidden", "c:/f/b"]),
            Request::Save {
                folder: "c:/f".into(),
                save: FolderOrderSave {
                    revision: 2,
                    listing_order: keys(&["c:/f/b", "c:/f/a"]),
                    inventory: inventory(&["c:/f/a", "c:/f/hidden", "c:/f/b"]),
                },
            },
        ]);
        assert_eq!(
            actor.read("c:/f").unwrap(),
            Some(record(2, &["c:/f/b", "c:/f/hidden", "c:/f/a"]))
        );
        assert_eq!(
            [actor.completion().revision, actor.completion().revision],
            [1, 2]
        );

        // A disappeared hidden child that later returns must stay unknown.
        let actor = ActorTest::new(vec![
            save_request("c:/f", 1, &["c:/f/a", "c:/f/hidden", "c:/f/b"]),
            save_request("c:/f", 2, &["c:/f/a", "c:/f/b"]),
            Request::Save {
                folder: "c:/f".into(),
                save: FolderOrderSave {
                    revision: 3,
                    listing_order: keys(&["c:/f/b", "c:/f/a"]),
                    inventory: inventory(&["c:/f/a", "c:/f/hidden", "c:/f/b"]),
                },
            },
        ]);
        assert_eq!(
            actor.read("c:/f").unwrap(),
            Some(record(3, &["c:/f/b", "c:/f/a"]))
        );
        assert_eq!(
            [
                actor.completion().revision,
                actor.completion().revision,
                actor.completion().revision
            ],
            [1, 2, 3]
        );
    }

    #[test]
    fn folder_order_actor_read_is_a_coalescing_barrier_with_private_replies() {
        let (reply1, read1) = mpsc::channel();
        let (reply2, read2) = mpsc::channel();
        let actor = ActorTest::new(vec![
            save_request("c:/f", 1, &["c:/f/a", "c:/f/b"]),
            Request::Read {
                folder: "c:/f".into(),
                reply: reply1,
            },
            save_request("c:/f", 2, &["c:/f/b", "c:/f/a"]),
            Request::Read {
                folder: "c:/f".into(),
                reply: reply2,
            },
        ]);
        assert_eq!(
            read1
                .recv_timeout(Duration::from_secs(10))
                .unwrap()
                .unwrap(),
            Some(record(1, &["c:/f/a", "c:/f/b"]))
        );
        assert_eq!(
            read2
                .recv_timeout(Duration::from_secs(10))
                .unwrap()
                .unwrap(),
            Some(record(2, &["c:/f/b", "c:/f/a"]))
        );
        assert_eq!(
            [actor.completion().revision, actor.completion().revision],
            [1, 2]
        );
    }

    #[test]
    fn folder_order_actor_reset_cannot_be_overtaken_by_old_save() {
        let actor = ActorTest::new(vec![
            save_request("c:/f", 1, &["c:/f/a", "c:/f/b"]),
            save_request("c:/f", 2, &["c:/f/b", "c:/f/a"]),
            Request::Reset {
                folder: "c:/f".into(),
                revision: 3,
            },
        ]);
        assert_eq!(actor.read("c:/f").unwrap(), None);
        assert_eq!(actor.completion().revision, 2);
        let reset = actor.completion();
        assert_eq!(
            (reset.revision, reset.operation),
            (3, FolderOrderOperation::Reset)
        );
        assert!(reset.result.is_ok());
    }

    #[test]
    fn folder_order_actor_reset_is_barrier_before_new_save() {
        let actor = ActorTest::new(vec![
            save_request("c:/f", 1, &["c:/f/a", "c:/f/b"]),
            Request::Reset {
                folder: "c:/f".into(),
                revision: 2,
            },
            save_request("c:/f", 3, &["c:/f/b", "c:/f/a"]),
        ]);
        assert_eq!(
            actor.read("c:/f").unwrap(),
            Some(record(1, &["c:/f/b", "c:/f/a"]))
        );
        assert_eq!(
            [
                actor.completion().revision,
                actor.completion().revision,
                actor.completion().revision
            ],
            [1, 2, 3]
        );
    }

    #[test]
    fn folder_order_record_index_waits_for_startup_and_includes_inactive_rows() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join(DB_NAME);
        let mut db = FolderOrderDb::open_at(&path).unwrap();
        db.save("c:/active", &record(1, &["c:/active/a"])).unwrap();
        let mut inactive = record(2, &["c:/inactive/a"]);
        inactive.active = false;
        db.save("c:/inactive", &inactive).unwrap();
        drop(db);

        let (service, release) = FolderOrderService::start_at_paused_for_test(path).unwrap();
        let handle = &service.handle;
        assert!(!handle.index_ready_for_test());
        assert!(!handle.record_is_known_absent(Path::new("c:/missing")));
        let barrier = handle.read(Path::new("c:/missing")).unwrap();
        release.send(()).unwrap();
        assert_eq!(
            barrier.recv_timeout(Duration::from_secs(10)).unwrap(),
            Ok(None)
        );
        assert!(handle.index_ready_for_test());
        assert!(!handle.record_is_known_absent(Path::new("C:/Active/")));
        assert!(!handle.record_is_known_absent(Path::new("C:/Inactive/")));
        assert!(handle.record_is_known_absent(Path::new("c:/missing")));
        service.shutdown();
    }

    #[test]
    fn folder_order_record_index_reset_retains_key_and_later_save_admission() {
        let actor = ActorTest::new(vec![]);
        assert_eq!(actor.read("c:/f").unwrap(), None);
        let handle = actor.handle();
        assert!(handle.index_ready_for_test());
        assert!(handle.record_is_known_absent(Path::new("c:/f")));
        handle
            .save(Path::new("c:/f"), save_data(1, &["c:/f/a"]))
            .unwrap();
        assert!(!handle.record_is_known_absent(Path::new("C:/F/")));
        assert!(actor.completion().result.is_ok());
        handle.reset(Path::new("c:/f"), 2).unwrap();
        assert_eq!(actor.read("c:/f").unwrap(), None);
        assert!(actor.completion().result.is_ok());
        assert!(!handle.record_is_known_absent(Path::new("c:/f")));

        // A later save is indexed at admission. Completing the earlier reset
        // must never remove that key, regardless of actor scheduling.
        handle.reset(Path::new("c:/f"), 3).unwrap();
        handle
            .save(Path::new("c:/f"), save_data(4, &["c:/f/b"]))
            .unwrap();
        assert!(!handle.record_is_known_absent(Path::new("c:/f")));
        assert_eq!(actor.read("c:/f").unwrap(), Some(record(1, &["c:/f/b"])));
        assert!(actor.completion().result.is_ok());
        assert!(actor.completion().result.is_ok());
        assert!(!handle.record_is_known_absent(Path::new("c:/f")));

        // Prefill both requests before running the actor so reset completion is
        // deterministically later than the final save's index admission.
        let temp = tempfile::tempdir().unwrap();
        let queue = Arc::new((Mutex::new(Queue::default()), Condvar::new()));
        {
            let mut pending = queue.0.lock().unwrap();
            pending.push(save_request("c:/f", 1, &["c:/f/a"]));
            pending.push(Request::Reset {
                folder: "c:/f".into(),
                revision: 2,
            });
            pending.push(save_request("c:/f", 3, &["c:/f/b"]));
            pending.closed = true;
        }
        let (tx, _completions) = mpsc::channel();
        run_actor(queue.clone(), tx, temp.path().join(DB_NAME));
        let finished = queue.0.lock().unwrap();
        assert!(finished.record_index.ready);
        assert!(finished.record_index.keys.contains("c:/f"));
    }

    #[test]
    fn folder_order_record_index_rename_admission_adds_exact_and_tree_destinations() {
        let actor = ActorTest::new(vec![
            save_request("c:/old", 1, &["c:/old/a"]),
            save_request("c:/old/sub", 2, &["c:/old/sub/a"]),
        ]);
        assert!(actor.read("c:/old").unwrap().is_some());
        let handle = actor.handle();
        handle
            .rename(
                Path::new("c:/exact"),
                3,
                vec![FolderOrderRename::new(
                    Path::new("c:/old"),
                    Path::new("c:/exact"),
                    RenameScope::Exact,
                )],
            )
            .unwrap();
        assert!(!handle.record_is_known_absent(Path::new("c:/old")));
        assert!(!handle.record_is_known_absent(Path::new("c:/exact")));
        assert!(handle.record_is_known_absent(Path::new("c:/exact/sub")));
        handle
            .rename(
                Path::new("c:/tree"),
                4,
                vec![FolderOrderRename::new(
                    Path::new("c:/old"),
                    Path::new("c:/tree"),
                    RenameScope::Tree,
                )],
            )
            .unwrap();
        assert!(!handle.record_is_known_absent(Path::new("c:/tree")));
        assert!(!handle.record_is_known_absent(Path::new("c:/tree/sub")));
        assert!(actor.read("c:/tree/sub").unwrap().is_some());
    }

    #[test]
    fn folder_order_record_index_startup_replays_rename_chain_and_unions_pending_saves() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join(DB_NAME);
        let mut db = FolderOrderDb::open_at(&path).unwrap();
        db.save("c:/a/sub", &record(1, &["c:/a/sub/item"])).unwrap();
        drop(db);
        let (service, release) = FolderOrderService::start_at_paused_for_test(path).unwrap();
        let handle = &service.handle;
        for (old, new, revision) in [("c:/a", "c:/b", 1), ("c:/b", "c:/c", 2)] {
            handle
                .rename(
                    Path::new(new),
                    revision,
                    vec![FolderOrderRename::new(
                        Path::new(old),
                        Path::new(new),
                        RenameScope::Tree,
                    )],
                )
                .unwrap();
        }
        handle
            .save(Path::new("c:/saved"), save_data(3, &["c:/saved/item"]))
            .unwrap();
        assert!(!handle.index_ready_for_test());
        let read = handle.read(Path::new("c:/c/sub")).unwrap();
        release.send(()).unwrap();
        assert_eq!(
            read.recv_timeout(Duration::from_secs(10)).unwrap().unwrap(),
            Some(record(1, &["c:/c/sub/item"]))
        );
        assert!(handle.index_ready_for_test());
        for folder in ["c:/a/sub", "c:/b/sub", "c:/c/sub", "c:/saved"] {
            assert!(
                !handle.record_is_known_absent(Path::new(folder)),
                "{folder}"
            );
        }
        assert!(handle.record_is_known_absent(Path::new("c:/missing")));
        service.shutdown();
    }

    #[test]
    fn folder_order_record_index_rename_batch_uses_simultaneous_source_rules() {
        let mut index = RecordIndex::default();
        index.publish(inventory(&["c:/a/sub", "c:/a/sub/deep", "c:/b/sub"]));
        index.admit(&Request::Rename {
            folder: "c:/a".into(),
            revision: 1,
            mappings: vec![
                FolderOrderRename::new(Path::new("c:/a"), Path::new("c:/b"), RenameScope::Tree),
                FolderOrderRename::new(Path::new("c:/b"), Path::new("c:/a"), RenameScope::Tree),
                FolderOrderRename::new(
                    Path::new("c:/a/sub"),
                    Path::new("c:/specific"),
                    RenameScope::Tree,
                ),
            ],
        });
        assert_eq!(
            index.keys,
            inventory(&[
                "c:/a/sub",
                "c:/a/sub/deep",
                "c:/b/sub",
                "c:/specific",
                "c:/specific/deep",
            ])
        );
    }

    #[test]
    fn folder_order_record_index_startup_read_failure_stays_unready_and_returns_typed_errors() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join(DB_NAME);
        let db = FolderOrderDb::open_at(&path).unwrap();
        // The schema opens successfully, but a non-text folder key cannot be
        // loaded into the complete startup index.
        db.conn
            .execute(
                "INSERT INTO folder_order_folders(folder_key,active,revision) VALUES(?1,0,1)",
                [vec![0xff_u8]],
            )
            .unwrap();
        drop(db);
        let (service, release) =
            FolderOrderService::start_at_paused_for_test(path.clone()).unwrap();
        let handle = &service.handle;
        handle
            .save(Path::new("c:/f"), save_data(1, &["c:/f/item"]))
            .unwrap();
        let read = handle.read(Path::new("c:/missing")).unwrap();
        release.send(()).unwrap();
        let read_error = read
            .recv_timeout(Duration::from_secs(10))
            .unwrap()
            .unwrap_err();
        let write_error = service
            .completions
            .recv_timeout(Duration::from_secs(10))
            .unwrap()
            .result
            .unwrap_err();
        assert_eq!(read_error, write_error);
        assert!(!handle.index_ready_for_test());
        assert!(!handle.record_is_known_absent(Path::new("c:/missing")));
        service.shutdown();
        let conn = Connection::open(path).unwrap();
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM folder_order_folders", [], |row| {
                row.get::<_, usize>(0)
            })
            .unwrap(),
            1
        );
    }

    #[test]
    fn folder_order_actor_set_active_is_fifo_barrier_without_record_creation() {
        let actor = ActorTest::new(vec![
            save_request("c:/f", 1, &["c:/f/a", "c:/f/b"]),
            Request::SetActive {
                folder: "c:/f".into(),
                active: false,
                revision: 2,
            },
            save_request("c:/f", 3, &["c:/f/b", "c:/f/a"]),
        ]);
        assert_eq!(
            actor.read("c:/f").unwrap(),
            Some(record(3, &["c:/f/b", "c:/f/a"]))
        );
        assert_eq!(
            [
                actor.completion().revision,
                actor.completion().revision,
                actor.completion().revision
            ],
            [1, 2, 3]
        );
        actor
            .handle()
            .set_active(Path::new("c:/missing"), true, 4)
            .unwrap();
        assert_eq!(actor.read("c:/missing").unwrap(), None);
    }

    #[test]
    fn folder_order_actor_old_save_rename_read_new_save_preserves_z_position() {
        let rename =
            FolderOrderRename::new(Path::new("c:/f/a"), Path::new("c:/f/b"), RenameScope::Exact);
        let (reply, read) = mpsc::channel();
        let actor = ActorTest::new(vec![
            save_request("c:/f", 1, &["c:/f/a", "c:/f/z"]),
            Request::Rename {
                folder: "c:/f".into(),
                revision: 2,
                mappings: vec![rename],
            },
            Request::Read {
                folder: "c:/f".into(),
                reply,
            },
            Request::Save {
                folder: "c:/f".into(),
                save: FolderOrderSave {
                    revision: 3,
                    listing_order: keys(&["c:/f/b"]),
                    inventory: inventory(&["c:/f/b", "c:/f/z"]),
                },
            },
        ]);
        assert_eq!(
            read.recv_timeout(Duration::from_secs(10)).unwrap().unwrap(),
            Some(record(1, &["c:/f/b", "c:/f/z"]))
        );
        assert_eq!(
            actor.read("c:/f").unwrap(),
            Some(record(2, &["c:/f/b", "c:/f/z"]))
        );
        assert_eq!(
            [
                actor.completion().revision,
                actor.completion().revision,
                actor.completion().revision
            ],
            [1, 2, 3]
        );
    }

    #[test]
    fn folder_order_actor_tree_rename_read_uses_new_folder_key() {
        let actor = ActorTest::new(vec![]);
        actor
            .handle()
            .save(Path::new("c:/old"), save_data(1, &["c:/old/a", "c:/old/z"]))
            .unwrap();
        actor
            .handle()
            .rename(
                Path::new("c:/new"),
                2,
                vec![FolderOrderRename::new(
                    Path::new("c:/old"),
                    Path::new("c:/new"),
                    RenameScope::Tree,
                )],
            )
            .unwrap();
        assert_eq!(
            actor.read("c:/new").unwrap(),
            Some(record(1, &["c:/new/a", "c:/new/z"]))
        );
        assert_eq!(actor.read("c:/old").unwrap(), None);
        let _ = actor.completion();
        let done = actor.completion();
        assert_eq!(
            (done.folder_key.as_str(), done.revision, done.operation),
            ("c:/new", 2, FolderOrderOperation::Rename)
        );
    }

    #[test]
    fn folder_order_actor_is_independent_of_generic_migration_db_lock() {
        let actor = ActorTest::new(vec![]);
        let generic =
            Connection::open(actor.temp.path().join("rename_migration_journal.db")).unwrap();
        generic
            .execute_batch("CREATE TABLE marker(value INTEGER); BEGIN EXCLUSIVE;")
            .unwrap();
        actor
            .handle()
            .save(Path::new("c:/f"), save_data(1, &["c:/f/a"]))
            .unwrap();
        assert_eq!(actor.read("c:/f").unwrap(), Some(record(1, &["c:/f/a"])));
        generic.execute_batch("ROLLBACK").unwrap();
    }

    #[test]
    fn folder_order_actor_failure_is_typed_and_does_not_retry_or_recreate() {
        let actor = ActorTest::new(vec![]);
        assert_eq!(actor.read("c:/f").unwrap(), None);
        // Invalid data is a failed write, and a later unrelated folder still works.
        actor
            .handle()
            .save(Path::new("c:/f"), save_data(5, &["c:/other/a"]))
            .unwrap();
        let failed = actor.completion();
        assert_eq!((failed.folder_key.as_str(), failed.revision), ("c:/f", 5));
        assert!(failed.result.is_err());
        actor
            .handle()
            .save(Path::new("c:/g"), save_data(6, &["c:/g/a"]))
            .unwrap();
        assert_eq!(actor.read("c:/g").unwrap(), Some(record(1, &["c:/g/a"])));
        assert!(actor.completion().result.is_ok());
        assert_eq!(actor.read("c:/f").unwrap(), None);
    }

    #[test]
    fn folder_order_actor_unreadable_folder_blocks_only_its_future_writes() {
        let actor = ActorTest::new(vec![]);
        actor
            .handle()
            .save(Path::new("c:/f"), save_data(1, &["c:/f/a"]))
            .unwrap();
        assert!(actor.completion().result.is_ok());
        let conn = Connection::open(actor.temp.path().join(DB_NAME)).unwrap();
        conn.execute(
            "UPDATE folder_order_folders SET active=9 WHERE folder_key='c:/f'",
            [],
        )
        .unwrap();
        assert!(actor.read("c:/f").is_err());
        conn.execute(
            "UPDATE folder_order_folders SET active=1 WHERE folder_key='c:/f'",
            [],
        )
        .unwrap();
        actor.handle().reset(Path::new("c:/f"), 2).unwrap();
        assert!(actor.completion().result.is_err());
        assert_eq!(actor.read("c:/f").unwrap(), Some(record(1, &["c:/f/a"])));
        actor
            .handle()
            .save(Path::new("c:/g"), save_data(3, &["c:/g/b"]))
            .unwrap();
        assert!(actor.completion().result.is_ok());
        assert_eq!(actor.read("c:/g").unwrap(), Some(record(1, &["c:/g/b"])));
    }

    #[test]
    fn folder_order_actor_read_normalizes_folder_identity_without_recording() {
        let actor = ActorTest::new(vec![]);
        assert_eq!(actor.read("C:/F/"), Ok(None));
        actor
            .handle()
            .save(Path::new("c:/f"), save_data(1, &["c:/f/a"]))
            .unwrap();
        assert_eq!(actor.read("C:/F/").unwrap(), Some(record(1, &["c:/f/a"])));
        assert_eq!(actor.read("c:/f").unwrap(), Some(record(1, &["c:/f/a"])));
    }

    #[test]
    fn folder_order_actor_shutdown_drains_accepted_requests() {
        let mut actor = ActorTest::new(vec![
            save_request("c:/f", 1, &["c:/f/a"]),
            save_request("c:/g", 2, &["c:/g/b"]),
        ]);
        actor.close();
        let db = FolderOrderDb::open_at(&actor.temp.path().join(DB_NAME)).unwrap();
        assert_eq!(db.read("c:/f").unwrap(), Some(record(1, &["c:/f/a"])));
        assert_eq!(db.read("c:/g").unwrap(), Some(record(1, &["c:/g/b"])));
    }

    #[test]
    fn folder_order_actor_guard_disconnects_waiting_read_and_rejects_new_requests() {
        let queue = Arc::new((Mutex::new(Queue::default()), Condvar::new()));
        let handle = FolderOrderHandle {
            client: Arc::new(Client {
                queue: queue.clone(),
            }),
            owner: None,
        };
        let read = handle.read(Path::new("c:/f")).unwrap();
        drop(ActorGuard(queue));
        assert!(matches!(
            read.recv_timeout(Duration::from_secs(1)),
            Err(mpsc::RecvTimeoutError::Disconnected)
        ));
        assert!(handle.read(Path::new("c:/g")).is_err());
        assert!(
            handle
                .save(Path::new("c:/f"), save_data(1, &["c:/f/a"]))
                .is_err()
        );
    }

    #[test]
    fn folder_order_actor_open_failure_replies_without_recreating_database() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join(DB_NAME);
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch("CREATE TABLE original(value TEXT); INSERT INTO original VALUES('keep'); PRAGMA user_version=99;").unwrap();
        }
        let queue = Arc::new((Mutex::new(Queue::default()), Condvar::new()));
        let handle = FolderOrderHandle {
            client: Arc::new(Client {
                queue: queue.clone(),
            }),
            owner: None,
        };
        let (tx, completions) = mpsc::channel();
        let worker_path = path.clone();
        let worker = std::thread::spawn(move || run_actor(queue, tx, worker_path));
        handle
            .save(Path::new("c:/f"), save_data(8, &["c:/f/a"]))
            .unwrap();
        let failed = completions.recv_timeout(Duration::from_secs(10)).unwrap();
        assert_eq!((failed.folder_key.as_str(), failed.revision), ("c:/f", 8));
        assert!(failed.result.is_err());
        assert!(
            handle
                .read(Path::new("c:/f"))
                .unwrap()
                .recv_timeout(Duration::from_secs(10))
                .unwrap()
                .is_err()
        );
        assert!(!handle.index_ready_for_test());
        assert!(!handle.record_is_known_absent(Path::new("c:/missing")));
        drop(handle);
        worker.join().unwrap();
        let conn = Connection::open(path).unwrap();
        assert_eq!(
            conn.query_row("SELECT value FROM original", [], |r| r.get::<_, String>(0))
                .unwrap(),
            "keep"
        );
    }
}
