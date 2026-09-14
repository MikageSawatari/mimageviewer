use std::fmt;
use std::path::PathBuf;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::settings::{ListingSortMetadata, SortOrder};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct CollectionId(Uuid);

impl CollectionId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }

    pub const fn from_uuid(id: Uuid) -> Self {
        Self(id)
    }

    pub const fn as_uuid(self) -> Uuid {
        self.0
    }
}

impl Default for CollectionId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for CollectionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct CollectionEntryId(Uuid);

impl CollectionEntryId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }

    pub const fn from_uuid(id: Uuid) -> Self {
        Self(id)
    }

    pub const fn as_uuid(self) -> Uuid {
        self.0
    }
}

impl Default for CollectionEntryId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for CollectionEntryId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CollectionSourceNamespace {
    FileSystemPath,
}

impl CollectionSourceNamespace {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::FileSystemPath => "filesystem_path",
        }
    }

    pub(crate) fn from_str(value: &str) -> Option<Self> {
        match value {
            "filesystem_path" => Some(Self::FileSystemPath),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct CollectionSourcePathKey {
    pub(super) namespace: CollectionSourceNamespace,
    pub(super) normalized_path: String,
}

impl CollectionSourcePathKey {
    pub(super) fn new_filesystem(normalized_path: String) -> Self {
        Self {
            namespace: CollectionSourceNamespace::FileSystemPath,
            normalized_path,
        }
    }

    pub const fn namespace(&self) -> CollectionSourceNamespace {
        self.namespace
    }

    pub fn normalized_path(&self) -> &str {
        &self.normalized_path
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CollectionResolvedKind {
    Image,
    Video,
    Audio,
    Folder,
    Zip,
    Pdf,
    ConvertibleArchive,
    Unresolved,
}

impl CollectionResolvedKind {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Image => "image",
            Self::Video => "video",
            Self::Audio => "audio",
            Self::Folder => "folder",
            Self::Zip => "zip",
            Self::Pdf => "pdf",
            Self::ConvertibleArchive => "convertible_archive",
            Self::Unresolved => "unresolved",
        }
    }

    pub(crate) fn from_str(value: &str) -> Option<Self> {
        match value {
            "image" => Some(Self::Image),
            "video" => Some(Self::Video),
            "audio" => Some(Self::Audio),
            "folder" => Some(Self::Folder),
            "zip" => Some(Self::Zip),
            "pdf" => Some(Self::Pdf),
            "convertible_archive" => Some(Self::ConvertibleArchive),
            "unresolved" => Some(Self::Unresolved),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CollectionOrderMode {
    #[default]
    Manual,
    Standard,
}

impl CollectionOrderMode {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Manual => "manual",
            Self::Standard => "standard",
        }
    }

    pub(crate) fn from_str(value: &str) -> Option<Self> {
        match value {
            "manual" => Some(Self::Manual),
            "standard" => Some(Self::Standard),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CollectionDefinition {
    pub id: CollectionId,
    pub name: String,
    pub order_mode: CollectionOrderMode,
    pub standard_sort: SortOrder,
    pub revision: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CollectionEntry {
    pub id: CollectionEntryId,
    pub collection_id: CollectionId,
    pub source_path: PathBuf,
    pub source_key: CollectionSourcePathKey,
    pub resolved_kind: CollectionResolvedKind,
    pub manual_position: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CollectionCatalogSnapshot {
    pub catalog_revision: u64,
    pub definitions: Arc<[CollectionDefinition]>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CollectionSnapshot {
    pub catalog_revision: u64,
    pub definition: CollectionDefinition,
    pub entries: Arc<[CollectionEntry]>,
}

impl CollectionSnapshot {
    pub const fn collection_id(&self) -> CollectionId {
        self.definition.id
    }

    pub const fn revision(&self) -> u64 {
        self.definition.revision
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CollectionRegistration {
    pub(super) source_path: PathBuf,
    pub(super) source_key: CollectionSourcePathKey,
    pub(super) resolved_kind: CollectionResolvedKind,
}

impl CollectionRegistration {
    pub fn source_path(&self) -> &std::path::Path {
        &self.source_path
    }

    pub fn source_key(&self) -> &CollectionSourcePathKey {
        &self.source_key
    }

    pub const fn resolved_kind(&self) -> CollectionResolvedKind {
        self.resolved_kind
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CollectionSourceMigrationScope {
    Exact,
    Tree,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CollectionSourceMigration {
    pub(super) old_path: PathBuf,
    pub(super) old_key: CollectionSourcePathKey,
    pub(super) new_path: PathBuf,
    pub(super) new_key: CollectionSourcePathKey,
    pub scope: CollectionSourceMigrationScope,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CollectionRevisionNotice {
    pub catalog_revision: u64,
    pub collection_revisions: Arc<[(CollectionId, u64)]>,
}

impl CollectionRevisionNotice {
    pub fn from_catalog(catalog: &CollectionCatalogSnapshot) -> Self {
        Self {
            catalog_revision: catalog.catalog_revision,
            collection_revisions: Arc::from(
                catalog
                    .definitions
                    .iter()
                    .map(|definition| (definition.id, definition.revision))
                    .collect::<Vec<_>>(),
            ),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CollectionBatchAddOutcome {
    pub snapshot: CollectionSnapshot,
    pub added: Arc<[CollectionEntryId]>,
    pub duplicates: Arc<[CollectionSourcePathKey]>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CollectionMigrationOutcome {
    pub catalog_revision: u64,
    pub affected: Arc<[(CollectionId, u64)]>,
    pub updated_entries: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CollectionStoreError {
    Busy,
    Starting,
    Unavailable,
    NotFound,
    DuplicateSource(CollectionSourcePathKey),
    Conflict { expected: u64, actual: u64 },
    InvalidName,
    InvalidPath(String),
    InvalidOrder,
    ManualOrderInactive,
    IncompatibleSchema(u32),
    Persistence(String),
}

impl fmt::Display for CollectionStoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Busy => f.write_str("collection store is busy"),
            Self::Starting => f.write_str("collection store is starting"),
            Self::Unavailable => f.write_str("collection store is unavailable"),
            Self::NotFound => f.write_str("collection or entry was not found"),
            Self::DuplicateSource(key) => {
                write!(f, "duplicate collection source: {}", key.normalized_path())
            }
            Self::Conflict { expected, actual } => {
                write!(
                    f,
                    "collection revision conflict: expected {expected}, actual {actual}"
                )
            }
            Self::InvalidName => f.write_str("collection name is empty"),
            Self::InvalidPath(reason) => write!(f, "invalid collection path: {reason}"),
            Self::InvalidOrder => f.write_str("manual order must contain every entry exactly once"),
            Self::ManualOrderInactive => {
                f.write_str("manual reordering is unavailable while standard sorting is active")
            }
            Self::IncompatibleSchema(version) => {
                write!(f, "unsupported collection database schema {version}")
            }
            Self::Persistence(message) => write!(f, "collection database error: {message}"),
        }
    }
}

impl std::error::Error for CollectionStoreError {}

impl From<rusqlite::Error> for CollectionStoreError {
    fn from(value: rusqlite::Error) -> Self {
        Self::Persistence(value.to_string())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CollectionSortFacts {
    pub entry_id: CollectionEntryId,
    pub category_rank: u8,
    pub name: String,
    pub mtime: i64,
    pub file_size: Option<i64>,
}

pub fn effective_collection_order(
    snapshot: &CollectionSnapshot,
    facts: &[CollectionSortFacts],
) -> Result<Vec<CollectionEntryId>, CollectionStoreError> {
    if snapshot.definition.order_mode == CollectionOrderMode::Manual {
        return Ok(snapshot.entries.iter().map(|entry| entry.id).collect());
    }
    if facts.len() != snapshot.entries.len() {
        return Err(CollectionStoreError::InvalidOrder);
    }

    let mut prepared = Vec::with_capacity(facts.len());
    let mut seen = std::collections::HashSet::with_capacity(facts.len());
    for fact in facts {
        if !seen.insert(fact.entry_id)
            || !snapshot
                .entries
                .iter()
                .any(|entry| entry.id == fact.entry_id)
        {
            return Err(CollectionStoreError::InvalidOrder);
        }
        prepared.push((fact, snapshot.definition.standard_sort.name_key(&fact.name)));
    }

    let sort = snapshot.definition.standard_sort;
    prepared.sort_by(|(a, a_name), (b, b_name)| {
        a.category_rank.cmp(&b.category_rank).then_with(|| {
            sort.compare_listing_keys(
                a_name,
                ListingSortMetadata::new(a.mtime, a.file_size),
                b_name,
                ListingSortMetadata::new(b.mtime, b.file_size),
            )
        })
    });
    Ok(prepared
        .into_iter()
        .map(|(fact, _)| fact.entry_id)
        .collect())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CollectionPlaybackKind {
    Image,
    Video,
    Audio,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CollectionTailBehavior {
    Stop,
    Loop,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CollectionCurrentEntry {
    pub entry_id: Option<CollectionEntryId>,
    pub source_key: Option<CollectionSourcePathKey>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CollectionPreparedSourceState {
    Available(CollectionResolvedKind),
    Unavailable,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CollectionPreparedNavigationFact {
    pub entry_id: CollectionEntryId,
    pub source_state: CollectionPreparedSourceState,
}

pub fn resolve_latest_collection_next(
    snapshot: &CollectionSnapshot,
    effective_order: &[CollectionEntryId],
    prepared_facts: &[CollectionPreparedNavigationFact],
    current: &CollectionCurrentEntry,
    target_kind: CollectionPlaybackKind,
    tail: CollectionTailBehavior,
) -> Result<Option<CollectionEntryId>, CollectionStoreError> {
    let entry_by_id: std::collections::HashMap<_, _> = snapshot
        .entries
        .iter()
        .map(|entry| (entry.id, entry))
        .collect();
    if effective_order.len() != entry_by_id.len() || prepared_facts.len() != entry_by_id.len() {
        return Err(CollectionStoreError::InvalidOrder);
    }
    let ordered_ids: std::collections::HashSet<_> = effective_order.iter().copied().collect();
    if ordered_ids.len() != entry_by_id.len()
        || !entry_by_id
            .keys()
            .all(|entry_id| ordered_ids.contains(entry_id))
    {
        return Err(CollectionStoreError::InvalidOrder);
    }
    let prepared_by_id: std::collections::HashMap<_, _> = prepared_facts
        .iter()
        .map(|fact| (fact.entry_id, fact.source_state))
        .collect();
    if prepared_by_id.len() != entry_by_id.len()
        || !entry_by_id
            .keys()
            .all(|entry_id| prepared_by_id.contains_key(entry_id))
    {
        return Err(CollectionStoreError::InvalidOrder);
    }
    let matches_kind = |entry_id: CollectionEntryId| match (target_kind, prepared_by_id[&entry_id])
    {
        (
            CollectionPlaybackKind::Image,
            CollectionPreparedSourceState::Available(CollectionResolvedKind::Image),
        )
        | (
            CollectionPlaybackKind::Video,
            CollectionPreparedSourceState::Available(CollectionResolvedKind::Video),
        )
        | (
            CollectionPlaybackKind::Audio,
            CollectionPreparedSourceState::Available(CollectionResolvedKind::Audio),
        ) => true,
        _ => false,
    };

    let anchor = current
        .entry_id
        .and_then(|id| {
            effective_order
                .iter()
                .position(|candidate| *candidate == id)
        })
        .or_else(|| {
            current.source_key.as_ref().and_then(|key| {
                effective_order.iter().position(|candidate| {
                    entry_by_id
                        .get(candidate)
                        .is_some_and(|entry| &entry.source_key == key)
                })
            })
        });

    let first = || {
        effective_order
            .iter()
            .find_map(|id| matches_kind(*id).then_some(*id))
    };
    let Some(anchor) = anchor else {
        return Ok(first());
    };
    if let Some(next) = effective_order
        .iter()
        .skip(anchor + 1)
        .find_map(|id| matches_kind(*id).then_some(*id))
    {
        return Ok(Some(next));
    }
    Ok((tail == CollectionTailBehavior::Loop).then(first).flatten())
}
