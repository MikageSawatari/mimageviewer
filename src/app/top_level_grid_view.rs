//! 最上位一覧 surface の所有権と復元先。
//!
//! 検索・snapshot・サブ展開・スマートフォルダなどは同じ `items` surface を共有する。
//! 個別の active flag は描画互換の派生情報として残すが、遷移の正本はこの型に集約する。

use std::path::{Path, PathBuf};
use std::sync::Arc;

use eframe::egui;

use super::subfolder_expansion::SubfolderExpansionRestoreState;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TopLevelSearchView {
    Favorite,
    Global,
    Tag,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum SmartFolderPosition {
    Root,
    /// A root row opened as a virtual book. Ctrl traversal uses its exact root entry.
    Container {
        root_entry: PathBuf,
        current: PathBuf,
    },
    Scoped {
        entry_index: usize,
        entry_root: PathBuf,
        current: PathBuf,
        /// The adopted physical item at `current`. History restoration cannot infer a nested
        /// PDF/ZIP from a row after the source session has been left.
        current_kind: super::smart_folder::SmartChildKind,
        back_stack: Vec<PathBuf>,
    },
}

/// One navigable row in the prepared Smart root's effective display order.
/// Images, videos, and presentation-only rows never enter this list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SmartRootNavEntry {
    pub(crate) logical_path: PathBuf,
    pub(crate) kind: super::smart_folder::SmartChildKind,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SmartFolderViewState {
    pub(crate) definition_id: uuid::Uuid,
    /// The single navigation order, derived from the final prepared root rows.
    pub(crate) navigation_entries: Arc<Vec<SmartRootNavEntry>>,
    pub(crate) position: SmartFolderPosition,
}

impl SmartFolderViewState {
    pub(crate) fn root(definition_id: uuid::Uuid, folder_entries: Vec<PathBuf>) -> Self {
        Self::root_with_navigation_entries(
            definition_id,
            folder_entries
                .into_iter()
                .map(|logical_path| SmartRootNavEntry {
                    logical_path,
                    kind: super::smart_folder::SmartChildKind::Folder,
                })
                .collect(),
        )
    }

    pub(crate) fn root_with_navigation_entries(
        definition_id: uuid::Uuid,
        navigation_entries: Vec<SmartRootNavEntry>,
    ) -> Self {
        Self {
            definition_id,
            navigation_entries: Arc::new(navigation_entries),
            position: SmartFolderPosition::Root,
        }
    }

    pub(crate) fn scoped_current(&self) -> Option<&Path> {
        match &self.position {
            SmartFolderPosition::Root | SmartFolderPosition::Container { .. } => None,
            SmartFolderPosition::Scoped { current, .. } => Some(current),
        }
    }

    pub(crate) fn scoped_entry_root(&self) -> Option<&Path> {
        match &self.position {
            SmartFolderPosition::Root | SmartFolderPosition::Container { .. } => None,
            SmartFolderPosition::Scoped { entry_root, .. } => Some(entry_root),
        }
    }

    #[cfg(test)]
    pub(crate) fn entry_index(&self) -> Option<usize> {
        match self.position {
            SmartFolderPosition::Root | SmartFolderPosition::Container { .. } => None,
            SmartFolderPosition::Scoped { entry_index, .. } => Some(entry_index),
        }
    }

    fn containing_entry_index(&self, path: &Path) -> Option<usize> {
        self.navigation_entries
            .iter()
            .position(|entry| crate::folder_tree::path_eq(&entry.logical_path, path))
            .or_else(|| {
                self.navigation_entries
                    .iter()
                    .enumerate()
                    .filter(|(_, entry)| {
                        entry.kind == super::smart_folder::SmartChildKind::Folder
                            && crate::search_index_db::is_under(path, &entry.logical_path)
                    })
                    .max_by_key(|(_, entry)| entry.logical_path.components().count())
                    .map(|(index, _)| index)
            })
    }

    /// synthetic root から見た直下の entry を返す。scoped current がさらに深い子孫でも、
    /// 通常フォルダの親復帰でいう「戻り先直下の子」に相当する entry root を解決する。
    pub(crate) fn containing_entry(&self, path: &Path) -> Option<&Path> {
        self.containing_entry_index(path)
            .and_then(|index| self.navigation_entries.get(index))
            .map(|entry| entry.logical_path.as_path())
    }

    /// `path` 自体が root entry、またはその子孫なら、その entry の scoped drill を開始する。
    /// 親子の entry が両方 root にある場合は exact match、次に最も深い祖先を優先する。
    pub(crate) fn enter_containing_path(&mut self, path: &Path) -> bool {
        let Some(entry_index) = self.containing_entry_index(path) else {
            return false;
        };
        let entry = &self.navigation_entries[entry_index];
        let entry_root = entry.logical_path.clone();
        if entry.kind != super::smart_folder::SmartChildKind::Folder {
            self.position = SmartFolderPosition::Container {
                root_entry: entry_root.clone(),
                current: entry_root,
            };
            return true;
        }
        self.position = SmartFolderPosition::Scoped {
            entry_index,
            entry_root: entry_root.clone(),
            current: entry_root,
            current_kind: super::smart_folder::SmartChildKind::Folder,
            back_stack: Vec::new(),
        };
        self.move_to(path)
    }

    pub(crate) fn contains_scoped_path(&self, path: &Path) -> bool {
        self.scoped_entry_root()
            .is_some_and(|root| crate::search_index_db::is_under(path, root))
    }

    /// scoped drill 内の実フォルダ遷移を記録する。
    ///
    /// `path` は entry root 配下に限定し、直接の親移動なら stack を pop、子孫移動なら
    /// 現在地を stack に積む。Ctrl+↑↓ のような非隣接 DFS 遷移では stack を root から
    /// 再構築して、Backspace が常に scope 内の親へ戻るようにする。
    pub(crate) fn move_to(&mut self, path: &Path) -> bool {
        let SmartFolderPosition::Scoped {
            entry_root,
            current,
            current_kind,
            back_stack,
            ..
        } = &mut self.position
        else {
            return false;
        };
        if !crate::search_index_db::is_under(path, entry_root) {
            return false;
        }
        if crate::folder_tree::path_eq(path, current) {
            return true;
        }
        if current
            .parent()
            .is_some_and(|parent| crate::folder_tree::path_eq(parent, path))
        {
            *current = path.to_path_buf();
            *current_kind = super::smart_folder::SmartChildKind::Folder;
            back_stack.pop();
            return true;
        }
        if path
            .parent()
            .is_some_and(|parent| crate::folder_tree::path_eq(parent, current))
        {
            back_stack.push(current.clone());
            *current = path.to_path_buf();
            *current_kind = super::smart_folder::SmartChildKind::Folder;
            return true;
        }
        let mut lineage = Vec::new();
        let mut cursor = path.parent();
        while let Some(parent) = cursor {
            if !crate::search_index_db::is_under(parent, entry_root) {
                return false;
            }
            lineage.push(parent.to_path_buf());
            if crate::folder_tree::path_eq(parent, entry_root) {
                break;
            }
            cursor = parent.parent();
        }
        if !lineage
            .last()
            .is_some_and(|root| crate::folder_tree::path_eq(root, entry_root))
        {
            return false;
        }
        lineage.reverse();
        *back_stack = lineage;
        *current = path.to_path_buf();
        *current_kind = super::smart_folder::SmartChildKind::Folder;
        true
    }

    pub(crate) fn parent_target(&self) -> Option<SmartFolderParentTarget> {
        if matches!(self.position, SmartFolderPosition::Container { .. }) {
            return Some(SmartFolderParentTarget::Root);
        }
        let SmartFolderPosition::Scoped {
            entry_root,
            current,
            back_stack,
            ..
        } = &self.position
        else {
            return None;
        };
        if crate::folder_tree::path_eq(entry_root, current) {
            Some(SmartFolderParentTarget::Root)
        } else {
            back_stack
                .last()
                .cloned()
                .or_else(|| current.parent().map(Path::to_path_buf))
                .map(SmartFolderParentTarget::Folder)
        }
    }

    pub(crate) fn navigation_start_index(&self, forward: bool) -> Option<usize> {
        Some(match self.position {
            SmartFolderPosition::Root if forward => 0,
            SmartFolderPosition::Root => self.navigation_entries.len().checked_sub(1)?,
            SmartFolderPosition::Container { ref root_entry, .. } => {
                let current = self.navigation_entries.iter().position(|entry| {
                    crate::folder_tree::path_eq(&entry.logical_path, root_entry)
                })?;
                if forward {
                    current.checked_add(1)?
                } else {
                    current.checked_sub(1)?
                }
            }
            SmartFolderPosition::Scoped { entry_index, .. } if forward => {
                entry_index.checked_add(1)?
            }
            SmartFolderPosition::Scoped { entry_index, .. } => entry_index.checked_sub(1)?,
        })
    }

    pub(crate) fn navigation_entry_at_offset(&self, forward: bool) -> Option<&SmartRootNavEntry> {
        let index = self.navigation_start_index(forward)?;
        self.navigation_entries.get(index)
    }

    pub(crate) fn entry_at_offset(&self, forward: bool) -> Option<&Path> {
        self.navigation_entry_at_offset(forward)
            .map(|entry| entry.logical_path.as_path())
    }

    /// root 再準備後の表示順を取り込む。現在 entry が同じ path として残っていれば
    /// scope と現在地を維持し、削除・リネームで見つからなければ安全に root へ戻す。
    pub(crate) fn refresh_navigation_entries(
        &mut self,
        navigation_entries: Vec<SmartRootNavEntry>,
    ) -> bool {
        let retained_index = self.scoped_entry_root().and_then(|entry_root| {
            navigation_entries.iter().position(|entry| {
                entry.kind == super::smart_folder::SmartChildKind::Folder
                    && crate::folder_tree::path_eq(&entry.logical_path, entry_root)
            })
        });
        let container_retained = match &self.position {
            SmartFolderPosition::Container { root_entry, .. } => {
                let old_kind = self
                    .navigation_entries
                    .iter()
                    .find(|entry| crate::folder_tree::path_eq(&entry.logical_path, root_entry))
                    .map(|entry| entry.kind);
                navigation_entries.iter().any(|entry| {
                    Some(entry.kind) == old_kind
                        && entry.kind != super::smart_folder::SmartChildKind::Folder
                        && crate::folder_tree::path_eq(&entry.logical_path, root_entry)
                })
            }
            _ => false,
        };
        self.navigation_entries = Arc::new(navigation_entries);
        match (&mut self.position, retained_index) {
            (SmartFolderPosition::Root, _) => true,
            (SmartFolderPosition::Container { .. }, _) if container_retained => true,
            (SmartFolderPosition::Container { .. }, _) => {
                self.position = SmartFolderPosition::Root;
                false
            }
            (SmartFolderPosition::Scoped { entry_index, .. }, Some(index)) => {
                *entry_index = index;
                true
            }
            (SmartFolderPosition::Scoped { .. }, None) => {
                self.position = SmartFolderPosition::Root;
                false
            }
        }
    }

    pub(crate) fn refresh_folder_entries(&mut self, folder_entries: Vec<PathBuf>) -> bool {
        self.refresh_navigation_entries(
            folder_entries
                .into_iter()
                .map(|logical_path| SmartRootNavEntry {
                    logical_path,
                    kind: super::smart_folder::SmartChildKind::Folder,
                })
                .collect(),
        )
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum SmartFolderParentTarget {
    Root,
    Folder(PathBuf),
}

#[derive(Clone, Debug)]
pub(crate) enum TopLevelGridRestore {
    Unavailable,
    Folder(PathBuf),
    DriveList,
    ReadingHistory,
    Bookmarks,
    Rating { stars: u8 },
    SubfolderExpansion(SubfolderExpansionRestoreState),
    SmartFolder(SmartFolderViewState),
    Collection(CollectionGridRestore),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct CollectionGridIdentity {
    pub(crate) collection_id: crate::collection_store::CollectionId,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CollectionGridViewportAnchor {
    pub(crate) entry_id: crate::collection_store::CollectionEntryId,
    pub(crate) source_key: crate::collection_store::CollectionSourcePathKey,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CollectionGridRestore {
    pub(crate) identity: CollectionGridIdentity,
    /// Minimum revision hint. The actor may return any newer revision.
    pub(crate) revision_at_open: u64,
    pub(crate) viewport_anchor: Option<CollectionGridViewportAnchor>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum CollectionGridPosition {
    Root,
    PhysicalSource {
        entry_id: crate::collection_store::CollectionEntryId,
        source_key: crate::collection_store::CollectionSourcePathKey,
        path: PathBuf,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct CollectionGridRequestStamp {
    pub(crate) context_id: super::viewer_context_registry::ViewerContextId,
    pub(crate) surface_generation: u64,
    pub(crate) collection_id: crate::collection_store::CollectionId,
}

/// Exact collection-grid owner captured before an asynchronous physical source open starts.
///
/// The surface stamp prevents a completion from crossing viewer contexts or collection
/// replacements. Both revisions are kept because a notice may advance `wanted_revision` while
/// the currently installed immutable binding still has `accepted_revision`.
#[derive(Clone)]
pub(crate) struct CollectionGridSourceOpenOwner {
    pub(crate) stamp: CollectionGridRequestStamp,
    pub(crate) accepted_revision: u64,
    pub(crate) wanted_revision: u64,
    pub(crate) anchor: CollectionGridViewportAnchor,
    /// Exact immutable root selected by collection playback navigation. Ordinary grid opens leave
    /// this empty; delayed archive conversion carries it until the physical-source commit.
    pub(crate) navigation_prepared:
        Option<std::sync::Arc<crate::collection_store::CollectionPreparedSnapshot>>,
    pub(crate) navigation_origin: Option<CollectionGridViewportAnchor>,
    /// Full intent and revision watch transferred from collection navigation into a delayed
    /// archive conversion. They keep close/replacement/revision races observable after the
    /// preflight request itself has left `collection_navigation_pending`.
    pub(in crate::app) navigation_request:
        Option<super::collection_navigation::CollectionNavigationRequest>,
    pub(in crate::app) navigation_watch: Option<crate::collection_store::CollectionRevisionWatch>,
}

/// Exact authorization for one physical load that remains owned by a mounted collection.
///
/// `Root` opens the selected collection entry. `PhysicalSource` covers a descendant open or an
/// in-place reload after that entry has already become the mounted physical source. An ordinary
/// address/history/favourite navigation never receives this value, even when it targets the same
/// path, so the common load boundary can retire the collection session deterministically.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CollectionGridPhysicalLoadOwner {
    pub(crate) stamp: CollectionGridRequestStamp,
    pub(crate) accepted_revision: u64,
    pub(crate) wanted_revision: u64,
    pub(crate) anchor: CollectionGridViewportAnchor,
    pub(crate) root_source_path: PathBuf,
    pub(crate) target_path: PathBuf,
    pub(crate) origin: CollectionGridPhysicalLoadOrigin,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum CollectionGridPhysicalLoadOrigin {
    Root { items_generation: u64 },
    PhysicalSource { current_path: PathBuf },
}

impl CollectionGridPhysicalLoadOwner {
    pub(crate) fn restore(&self, wanted_revision: u64) -> CollectionGridRestore {
        CollectionGridRestore {
            identity: CollectionGridIdentity {
                collection_id: self.stamp.collection_id,
            },
            revision_at_open: self.accepted_revision.max(wanted_revision),
            viewport_anchor: Some(self.anchor.clone()),
        }
    }
}

impl std::fmt::Debug for CollectionGridSourceOpenOwner {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("CollectionGridSourceOpenOwner")
            .field("stamp", &self.stamp)
            .field("accepted_revision", &self.accepted_revision)
            .field("wanted_revision", &self.wanted_revision)
            .field("anchor", &self.anchor)
            .field(
                "navigation_revision",
                &self
                    .navigation_prepared
                    .as_ref()
                    .map(|prepared| prepared.collection_revision),
            )
            .field("navigation_origin", &self.navigation_origin)
            .field("navigation_request", &self.navigation_request)
            .field("navigation_watch", &self.navigation_watch.is_some())
            .finish()
    }
}

impl PartialEq for CollectionGridSourceOpenOwner {
    fn eq(&self, other: &Self) -> bool {
        self.stamp == other.stamp
            && self.accepted_revision == other.accepted_revision
            && self.wanted_revision == other.wanted_revision
            && self.anchor == other.anchor
            && self.navigation_origin == other.navigation_origin
            && self.navigation_request == other.navigation_request
            && self.navigation_watch.is_some() == other.navigation_watch.is_some()
            && self
                .navigation_prepared
                .as_ref()
                .map(|prepared| (prepared.collection_id, prepared.collection_revision))
                == other
                    .navigation_prepared
                    .as_ref()
                    .map(|prepared| (prepared.collection_id, prepared.collection_revision))
    }
}

impl Eq for CollectionGridSourceOpenOwner {}

pub(crate) enum CollectionGridLoadState {
    RequestNeeded {
        installed: Option<std::sync::Arc<crate::collection_store::CollectionPreparedSnapshot>>,
        /// Busy/Starting admission is retried only after this deadline.
        not_before: Option<std::time::Instant>,
    },
    Snapshot {
        stamp: CollectionGridRequestStamp,
        minimum_revision: u64,
        /// Perf-only enqueue time; no effect on request ownership or scheduling.
        queued_at: Option<std::time::Instant>,
        installed: Option<std::sync::Arc<crate::collection_store::CollectionPreparedSnapshot>>,
        receiver: crossbeam_channel::Receiver<
            Result<
                crate::collection_store::CollectionSnapshot,
                crate::collection_store::CollectionStoreError,
            >,
        >,
    },
    Preparing {
        stamp: CollectionGridRequestStamp,
        exact_revision: u64,
        installed: Option<std::sync::Arc<crate::collection_store::CollectionPreparedSnapshot>>,
        cancel: std::sync::Arc<std::sync::atomic::AtomicBool>,
        receiver: std::sync::mpsc::Receiver<
            Result<CollectionGridPreparedInstall, crate::collection_store::CollectionPrepareError>,
        >,
    },
    Ready(std::sync::Arc<CollectionGridInstalledPresentation>),
    Empty(std::sync::Arc<CollectionGridInstalledPresentation>),
    Failed {
        message: String,
        installed: Option<std::sync::Arc<crate::collection_store::CollectionPreparedSnapshot>>,
    },
    Deleted,
}

pub(crate) struct CollectionGridPreparedInstall {
    pub(crate) prepared: crate::collection_store::CollectionPreparedSnapshot,
    pub(crate) thumbnail_sources: CollectionGridPreparedThumbnailSources,
    pub(crate) auto_aspect_lookup: Option<crate::auto_aspect_cache::CollectionAutoAspectLookup>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct CollectionGridThumbnailSources {
    pub(crate) video_sidecars: std::collections::HashMap<String, PathBuf>,
    pub(crate) video_pin_blobs: std::collections::HashMap<PathBuf, Vec<u8>>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct CollectionGridThumbnailSourceIdentity(pub(crate) [u8; 32]);

#[derive(Debug)]
pub(crate) struct CollectionGridPreparedThumbnailSources {
    pub(crate) identity: CollectionGridThumbnailSourceIdentity,
    pub(crate) payload: CollectionGridThumbnailSources,
}

impl PartialEq for CollectionGridPreparedThumbnailSources {
    fn eq(&self, other: &Self) -> bool {
        self.identity == other.identity
    }
}

impl Eq for CollectionGridPreparedThumbnailSources {}

impl Default for CollectionGridPreparedThumbnailSources {
    fn default() -> Self {
        Self {
            identity: CollectionGridThumbnailSourceIdentity::default(),
            payload: CollectionGridThumbnailSources::default(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CollectionGridInstalledPresentation {
    pub(crate) prepared: std::sync::Arc<crate::collection_store::CollectionPreparedSnapshot>,
    pub(crate) thumbnail_source_identity: CollectionGridThumbnailSourceIdentity,
}

impl CollectionGridInstalledPresentation {
    pub(crate) fn new(
        prepared: std::sync::Arc<crate::collection_store::CollectionPreparedSnapshot>,
        thumbnail_source_identity: CollectionGridThumbnailSourceIdentity,
    ) -> Self {
        Self {
            prepared,
            thumbnail_source_identity,
        }
    }

    #[cfg(test)]
    pub(crate) fn without_thumbnail_sources(
        prepared: std::sync::Arc<crate::collection_store::CollectionPreparedSnapshot>,
    ) -> std::sync::Arc<Self> {
        std::sync::Arc::new(Self::new(
            prepared,
            CollectionGridThumbnailSourceIdentity::default(),
        ))
    }
}

impl CollectionGridLoadState {
    pub(crate) fn installed(
        &self,
    ) -> Option<&std::sync::Arc<crate::collection_store::CollectionPreparedSnapshot>> {
        match self {
            Self::RequestNeeded { installed, .. }
            | Self::Snapshot { installed, .. }
            | Self::Preparing { installed, .. }
            | Self::Failed { installed, .. } => installed.as_ref(),
            Self::Ready(presentation) | Self::Empty(presentation) => Some(&presentation.prepared),
            Self::Deleted => None,
        }
    }

    pub(crate) fn installed_presentation(
        &self,
    ) -> Option<&std::sync::Arc<CollectionGridInstalledPresentation>> {
        match self {
            Self::Ready(presentation) | Self::Empty(presentation) => Some(presentation),
            _ => None,
        }
    }
}

pub(crate) struct CollectionGridSession {
    pub(crate) identity: CollectionGridIdentity,
    pub(crate) position: CollectionGridPosition,
    pub(crate) accepted_revision: u64,
    pub(crate) wanted_revision: u64,
    pub(crate) observed_catalog_revision: u64,
    pub(crate) load: CollectionGridLoadState,
    pub(crate) watch: Option<crate::collection_store::CollectionRevisionWatch>,
    pub(crate) restore_anchor: Option<CollectionGridViewportAnchor>,
    pub(crate) installed_items_generation: Option<u64>,
    /// Collection video extraction has no queue shared with the bundle thumbnail pool. Keep its
    /// cancellation lifetime with the collection session; image/container workers remain owned by
    /// `ViewerContextBundle` and may be reused by another synthetic surface.
    pub(crate) video_worker_cancel: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
}

impl CollectionGridSession {
    fn new(identity: CollectionGridIdentity) -> Self {
        Self {
            identity,
            position: CollectionGridPosition::Root,
            accepted_revision: 0,
            wanted_revision: 0,
            observed_catalog_revision: 0,
            load: CollectionGridLoadState::RequestNeeded {
                installed: None,
                not_before: None,
            },
            watch: None,
            restore_anchor: None,
            installed_items_generation: None,
            video_worker_cancel: None,
        }
    }

    pub(crate) fn cancel_pending(&mut self) {
        let previous = std::mem::replace(
            &mut self.load,
            CollectionGridLoadState::RequestNeeded {
                installed: None,
                not_before: None,
            },
        );
        log_collection_grid_pending_cancel(&previous);
        let installed = match previous {
            CollectionGridLoadState::RequestNeeded { installed, .. }
            | CollectionGridLoadState::Failed { installed, .. } => installed,
            CollectionGridLoadState::Snapshot { installed, .. } => installed,
            CollectionGridLoadState::Preparing {
                installed, cancel, ..
            } => {
                cancel.store(true, std::sync::atomic::Ordering::Release);
                installed
            }
            CollectionGridLoadState::Ready(presentation)
            | CollectionGridLoadState::Empty(presentation) => {
                Some(std::sync::Arc::clone(&presentation.prepared))
            }
            CollectionGridLoadState::Deleted => None,
        };
        self.load = CollectionGridLoadState::RequestNeeded {
            installed,
            not_before: None,
        };
    }

    pub(crate) fn prepared(
        &self,
    ) -> Option<&std::sync::Arc<crate::collection_store::CollectionPreparedSnapshot>> {
        self.load.installed()
    }

    pub(crate) fn installed_presentation(
        &self,
    ) -> Option<&std::sync::Arc<CollectionGridInstalledPresentation>> {
        self.load.installed_presentation()
    }
}

impl Drop for CollectionGridSession {
    fn drop(&mut self) {
        log_collection_grid_pending_cancel(&self.load);
        if let CollectionGridLoadState::Preparing { cancel, .. } = &self.load {
            cancel.store(true, std::sync::atomic::Ordering::Release);
        }
        if let Some(cancel) = self.video_worker_cancel.take() {
            cancel.store(true, std::sync::atomic::Ordering::Release);
        }
    }
}

fn log_collection_grid_pending_cancel(load: &CollectionGridLoadState) {
    if !crate::perf::is_enabled() {
        return;
    }
    match load {
        CollectionGridLoadState::Snapshot {
            stamp,
            queued_at: Some(start),
            ..
        } => {
            crate::perf::event(
                "collection",
                "actor_rtt",
                None,
                0,
                &[
                    ("operation", serde_json::Value::from("load_collection")),
                    (
                        "collection_id",
                        serde_json::Value::from(stamp.collection_id.as_uuid().to_string()),
                    ),
                    (
                        "request_generation",
                        serde_json::Value::from(stamp.surface_generation),
                    ),
                    (
                        "ms",
                        serde_json::Value::from(start.elapsed().as_secs_f64() * 1000.0),
                    ),
                    ("outcome", serde_json::Value::from("cancelled")),
                ],
            );
        }
        CollectionGridLoadState::Preparing {
            stamp,
            exact_revision,
            ..
        } => {
            crate::perf::event(
                "collection",
                "prepare_result",
                None,
                0,
                &[
                    (
                        "collection_id",
                        serde_json::Value::from(stamp.collection_id.as_uuid().to_string()),
                    ),
                    (
                        "request_generation",
                        serde_json::Value::from(stamp.surface_generation),
                    ),
                    ("revision", serde_json::Value::from(*exact_revision)),
                    ("outcome", serde_json::Value::from("cancelled")),
                ],
            );
        }
        _ => {}
    }
}

impl Clone for CollectionGridSession {
    fn clone(&self) -> Self {
        Self {
            identity: self.identity,
            position: self.position.clone(),
            accepted_revision: self.accepted_revision,
            wanted_revision: self.wanted_revision,
            observed_catalog_revision: self.observed_catalog_revision,
            load: CollectionGridLoadState::RequestNeeded {
                installed: self.load.installed().cloned(),
                not_before: None,
            },
            watch: None,
            restore_anchor: self.restore_anchor.clone(),
            installed_items_generation: self.installed_items_generation,
            video_worker_cancel: None,
        }
    }
}

impl TopLevelGridRestore {
    pub(crate) fn from_legacy_parts(
        path: Option<PathBuf>,
        subfolder_restore: Option<SubfolderExpansionRestoreState>,
        rating_view_stars: Option<u8>,
        smart_state: Option<SmartFolderViewState>,
    ) -> Self {
        if let Some(state) = subfolder_restore {
            return Self::SubfolderExpansion(state);
        }
        let Some(path) = path else {
            return Self::Unavailable;
        };
        if crate::folder_tree::path_eq(&path, &super::drive_list_synthetic_path()) {
            Self::DriveList
        } else if crate::folder_tree::path_eq(&path, &super::reading_history_synthetic_path()) {
            Self::ReadingHistory
        } else if crate::folder_tree::path_eq(&path, &super::bookmark_view_synthetic_path()) {
            Self::Bookmarks
        } else if crate::folder_tree::path_eq(&path, &super::rating_view_synthetic_path()) {
            rating_view_stars
                .filter(|stars| (1..=5).contains(stars))
                .map(|stars| Self::Rating { stars })
                .unwrap_or(Self::Unavailable)
        } else if let Some(definition_id) =
            super::smart_folder::smart_folder_id_from_synthetic_path(&path)
        {
            let state = smart_state
                .filter(|state| state.definition_id == definition_id)
                .unwrap_or_else(|| SmartFolderViewState::root(definition_id, Vec::new()));
            Self::SmartFolder(state)
        } else {
            Self::Folder(path)
        }
    }

    pub(crate) fn legacy_path(&self) -> Option<PathBuf> {
        match self {
            Self::Unavailable => None,
            Self::Folder(path) => Some(path.clone()),
            Self::DriveList => Some(super::drive_list_synthetic_path()),
            Self::ReadingHistory => Some(super::reading_history_synthetic_path()),
            Self::Bookmarks => Some(super::bookmark_view_synthetic_path()),
            Self::Rating { .. } => Some(super::rating_view_synthetic_path()),
            Self::SubfolderExpansion(_) => Some(super::subfolder_expansion_synthetic_path()),
            Self::SmartFolder(state) => Some(super::smart_folder::smart_folder_synthetic_path(
                state.definition_id,
            )),
            Self::Collection(_) => None,
        }
    }

    pub(crate) fn rating_stars(&self) -> Option<u8> {
        match self {
            Self::Rating { stars } => Some(*stars),
            _ => None,
        }
    }

    pub(crate) fn subfolder_restore(&self) -> Option<SubfolderExpansionRestoreState> {
        match self {
            Self::SubfolderExpansion(state) => Some(state.clone()),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum TopLevelGridSurface {
    Folder,
    DriveList,
    Search(TopLevelSearchView),
    Snapshot,
    SubfolderExpansion,
    SmartFolder(SmartFolderViewState),
    ReadingHistory,
    Bookmarks,
    Rating { stars: u8 },
    Collection(CollectionGridIdentity),
}

impl super::App {
    /// 現在の最上位一覧を、その surface が所有する再入場経路で更新する。
    pub(crate) fn reload_top_level_grid(&mut self, ctx: &egui::Context) {
        let surface = self.top_level_grid_view.surface().clone();
        let folder_pane_active = self.effective_folder();

        match surface {
            TopLevelGridSurface::Folder => {
                let local_search = self.show_search_bar.then(|| self.search_query.clone());
                self.reload_current_folder_preserving_override();
                if let Some(query) = local_search {
                    self.show_search_bar = true;
                    self.search_query = query;
                    self.execute_search(ctx);
                }
            }
            TopLevelGridSurface::SmartFolder(state) => {
                if self.smart_folder_busy() {
                    return;
                }
                let _ = self.refresh_smart_folder_staged(state.definition_id);
            }
            TopLevelGridSurface::SubfolderExpansion => {
                if self.subfolder_expansion_busy() {
                    return;
                }
                if let Some(snapshot) = self.subfolder_expansion_snapshot.as_ref() {
                    self.start_subfolder_expansion_scan_roots(
                        snapshot.root.clone(),
                        snapshot.roots.clone(),
                    );
                }
            }
            TopLevelGridSurface::Search(TopLevelSearchView::Global) => {
                self.spawn_global_search(ctx);
            }
            TopLevelGridSurface::Search(TopLevelSearchView::Favorite) => {
                self.execute_favsearch();
            }
            TopLevelGridSurface::Search(TopLevelSearchView::Tag) => {
                self.open_tag_view_with_query(Some(self.tag_view.query.clone()), false);
            }
            TopLevelGridSurface::Rating { stars } => self.enter_rating_view(stars),
            TopLevelGridSurface::ReadingHistory => self.enter_reading_history(),
            TopLevelGridSurface::Bookmarks => self.enter_bookmark_view(),
            TopLevelGridSurface::DriveList => {
                let origin = self.selected.and_then(|index| match self.items.get(index) {
                    Some(crate::grid_item::GridItem::Folder(path)) => Some(path.clone()),
                    _ => None,
                });
                self.enter_drive_list(origin);
            }
            TopLevelGridSurface::Snapshot => {}
            TopLevelGridSurface::Collection(identity) => {
                self.open_collection_grid(identity.collection_id, None);
            }
        }

        if self.settings.folder_tree_pane_visible {
            self.folder_pane.reload_for_active(
                folder_pane_active.as_deref(),
                crate::folder_pane::FolderPaneListingOptions::from_settings(&self.settings),
            );
        }
    }
}

pub(crate) struct TopLevelGridView {
    surface: TopLevelGridSurface,
    return_to: Option<TopLevelGridRestore>,
    generation: u64,
    /// The completed smart-folder result has exactly the same lifetime as this surface plus
    /// descendants opened from it. `begin` is an explicit top-level transition and always drops
    /// the old session; `replace_surface` preserves it only while the same smart-folder surface
    /// owns the navigation scope.
    smart_folder_session: Option<super::smart_folder::SmartFolderSession>,
    collection_session: Option<CollectionGridSession>,
    /// Playback/navigation work belongs to this exact viewer context. It is deliberately not
    /// cloned into duplicated contexts; dropping/replacing a surface cancels its workers.
    collection_navigation_pending:
        Option<super::collection_navigation::CollectionNavigationPending>,
    /// Monotonic intent identity for collection playback requests in this viewer context.
    /// Navigation producers and terminal actions advance it so an index ABA cannot make an old
    /// asynchronous result current again.
    collection_navigation_sequence: u64,
    /// A surface transition can retire a pending fullscreen request while `TopLevelGridView` is
    /// mutably borrowed on its own. App consumes this bit at the next poll and releases the shared
    /// fullscreen-navigation lock through the normal terminal path.
    collection_navigation_retired_fs_lock: bool,
    collection_navigation_retired_pdf_password: bool,
}

impl Clone for TopLevelGridView {
    fn clone(&self) -> Self {
        Self {
            surface: self.surface.clone(),
            return_to: self.return_to.clone(),
            generation: self.generation,
            // Context duplication may copy the visible grid identity for an independent viewer,
            // but the main smart-folder result remains owned by the main top-level surface.
            smart_folder_session: None,
            collection_session: self.collection_session.clone(),
            collection_navigation_pending: None,
            collection_navigation_sequence: self.collection_navigation_sequence,
            collection_navigation_retired_fs_lock: false,
            collection_navigation_retired_pdf_password: false,
        }
    }
}

impl Drop for TopLevelGridView {
    fn drop(&mut self) {
        if let Some(pending) = self.collection_navigation_pending.as_ref() {
            pending.cancel();
        }
    }
}

impl std::fmt::Debug for TopLevelGridView {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("TopLevelGridView")
            .field("surface", &self.surface)
            .field("return_to", &self.return_to)
            .field("generation", &self.generation)
            .field(
                "has_smart_folder_session",
                &self.smart_folder_session.is_some(),
            )
            .finish()
    }
}

impl Default for TopLevelGridView {
    fn default() -> Self {
        Self {
            surface: TopLevelGridSurface::Folder,
            return_to: None,
            generation: 0,
            smart_folder_session: None,
            collection_session: None,
            collection_navigation_pending: None,
            collection_navigation_sequence: 0,
            collection_navigation_retired_fs_lock: false,
            collection_navigation_retired_pdf_password: false,
        }
    }
}

impl TopLevelGridView {
    pub(crate) fn surface(&self) -> &TopLevelGridSurface {
        &self.surface
    }

    pub(crate) fn generation(&self) -> u64 {
        self.generation
    }

    pub(crate) fn begin(
        &mut self,
        surface: TopLevelGridSurface,
        return_to: Option<TopLevelGridRestore>,
    ) -> u64 {
        self.generation = self.generation.wrapping_add(1);
        self.advance_collection_navigation_sequence();
        self.smart_folder_session = None;
        self.set_collection_navigation_pending(None);
        self.collection_session = match &surface {
            TopLevelGridSurface::Collection(identity) => {
                Some(CollectionGridSession::new(*identity))
            }
            _ => None,
        };
        self.surface = surface;
        self.return_to = return_to;
        self.generation
    }

    pub(crate) fn replace_surface(&mut self, surface: TopLevelGridSurface) -> u64 {
        let keeps_smart_folder_session = matches!(
            (self.smart_folder_session.as_ref(), &surface),
            (
                Some(session),
                TopLevelGridSurface::SmartFolder(state),
            ) if session.definition_id() == state.definition_id
        );
        self.generation = self.generation.wrapping_add(1);
        self.advance_collection_navigation_sequence();
        if !keeps_smart_folder_session {
            self.smart_folder_session = None;
        }
        self.set_collection_navigation_pending(None);
        self.collection_session = match &surface {
            TopLevelGridSurface::Collection(identity) => {
                Some(CollectionGridSession::new(*identity))
            }
            _ => None,
        };
        self.surface = surface;
        self.return_to = None;
        self.generation
    }

    pub(crate) fn take_return_to(&mut self) -> Option<TopLevelGridRestore> {
        self.generation = self.generation.wrapping_add(1);
        self.advance_collection_navigation_sequence();
        self.smart_folder_session = None;
        self.set_collection_navigation_pending(None);
        self.collection_session = None;
        self.surface = TopLevelGridSurface::Folder;
        self.return_to.take()
    }

    pub(crate) fn return_to(&self) -> Option<&TopLevelGridRestore> {
        self.return_to.as_ref()
    }

    /// Installs a return owner captured by the source context into a newly-built physical viewer.
    /// This does not change the new context's current surface or generation.
    pub(crate) fn install_return_to(&mut self, return_to: TopLevelGridRestore) {
        self.return_to = Some(return_to);
    }

    pub(crate) fn collection_session(&self) -> Option<&CollectionGridSession> {
        self.collection_session.as_ref()
    }

    pub(crate) fn collection_session_mut(&mut self) -> Option<&mut CollectionGridSession> {
        self.collection_session.as_mut()
    }

    pub(crate) fn collection_navigation_pending(&self) -> bool {
        self.collection_navigation_pending.is_some()
    }

    pub(in crate::app) fn collection_navigation_owns_fs_lock(&self) -> bool {
        self.collection_navigation_pending
            .as_ref()
            .is_some_and(|pending| pending.owns_fs_navigation_lock())
    }

    pub(in crate::app) fn collection_navigation_owns_pdf_password(&self) -> bool {
        self.collection_navigation_pending
            .as_ref()
            .is_some_and(|pending| pending.is_pdf_password())
    }

    pub(in crate::app) fn advance_collection_navigation_sequence(&mut self) -> u64 {
        self.collection_navigation_sequence = self.collection_navigation_sequence.wrapping_add(1);
        if self.collection_navigation_sequence == 0 {
            self.collection_navigation_sequence = 1;
        }
        self.collection_navigation_sequence
    }

    pub(in crate::app) fn collection_navigation_sequence(&self) -> u64 {
        self.collection_navigation_sequence
    }

    pub(in crate::app) fn collection_navigation_poll_delay(&self) -> Option<std::time::Duration> {
        self.collection_navigation_pending
            .as_ref()
            .map(|pending| pending.poll_delay())
    }

    pub(in crate::app) fn take_collection_navigation_retired_fs_lock(&mut self) -> bool {
        std::mem::take(&mut self.collection_navigation_retired_fs_lock)
    }

    pub(in crate::app) fn take_collection_navigation_retired_pdf_password(&mut self) -> bool {
        std::mem::take(&mut self.collection_navigation_retired_pdf_password)
    }

    pub(crate) fn accumulate_collection_outer_navigation(
        &mut self,
        fullscreen: bool,
        forward: bool,
    ) -> bool {
        self.collection_navigation_pending
            .as_mut()
            .is_some_and(|pending| pending.accumulate_outer(fullscreen, forward))
    }

    pub(in crate::app) fn accumulate_collection_manual_navigation(
        &mut self,
        delta: i32,
        landing: super::ManualMediaNavigationLanding,
        still_only: bool,
        display_unit_step: bool,
    ) -> bool {
        self.collection_navigation_pending
            .as_mut()
            .is_some_and(|pending| {
                pending.accumulate_manual(delta, landing, still_only, display_unit_step)
            })
    }

    pub(crate) fn set_collection_outer_queued_steps(&mut self, steps: i32) {
        if let Some(pending) = self.collection_navigation_pending.as_mut() {
            pending.set_outer_queued_steps(steps);
        }
    }

    pub(in crate::app) fn set_collection_navigation_pending(
        &mut self,
        pending: Option<super::collection_navigation::CollectionNavigationPending>,
    ) {
        if let Some(previous) = std::mem::replace(&mut self.collection_navigation_pending, pending)
        {
            self.collection_navigation_retired_fs_lock |= previous.owns_fs_navigation_lock();
            self.collection_navigation_retired_pdf_password |= previous.is_pdf_password();
            previous.cancel();
        }
    }

    pub(in crate::app) fn take_collection_navigation_pending(
        &mut self,
    ) -> Option<super::collection_navigation::CollectionNavigationPending> {
        self.collection_navigation_pending.take()
    }

    pub(crate) fn smart_folder(&self) -> Option<&SmartFolderViewState> {
        match &self.surface {
            TopLevelGridSurface::SmartFolder(state) => Some(state),
            _ => None,
        }
    }

    pub(crate) fn smart_folder_mut(&mut self) -> Option<&mut SmartFolderViewState> {
        match &mut self.surface {
            TopLevelGridSurface::SmartFolder(state) => Some(state),
            _ => None,
        }
    }

    pub(crate) fn smart_folder_session(&self) -> Option<&super::smart_folder::SmartFolderSession> {
        self.smart_folder_session.as_ref()
    }

    pub(crate) fn smart_folder_session_mut(
        &mut self,
    ) -> Option<&mut super::smart_folder::SmartFolderSession> {
        self.smart_folder_session.as_mut()
    }

    pub(crate) fn install_smart_folder_session(
        &mut self,
        session: super::smart_folder::SmartFolderSession,
    ) {
        debug_assert!(matches!(
            &self.surface,
            TopLevelGridSurface::SmartFolder(state)
                if state.definition_id == session.definition_id()
        ));
        self.smart_folder_session = Some(session);
    }

    pub(crate) fn take_smart_folder_session(
        &mut self,
    ) -> Option<super::smart_folder::SmartFolderSession> {
        self.smart_folder_session.take()
    }

    pub(crate) fn discard_smart_folder_session(&mut self, definition_id: uuid::Uuid) {
        if self
            .smart_folder_session
            .as_ref()
            .is_some_and(|session| session.definition_id() == definition_id)
        {
            self.smart_folder_session = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn smart_folder_scope_never_accepts_sibling_escape() {
        let id = uuid::Uuid::new_v4();
        let root = PathBuf::from(r"C:\books\entry");
        let mut state = SmartFolderViewState::root(id, vec![root.clone()]);
        assert!(state.enter_containing_path(&root));
        assert!(state.move_to(&root.join("child")));
        assert!(!state.move_to(Path::new(r"C:\books\other")));
        assert_eq!(state.scoped_current(), Some(root.join("child").as_path()));
    }

    #[test]
    fn smart_folder_parent_returns_root_without_filesystem_escape() {
        let id = uuid::Uuid::new_v4();
        let root = PathBuf::from(r"C:\books\entry");
        let mut state = SmartFolderViewState::root(id, vec![root.clone()]);
        assert!(state.enter_containing_path(&root));
        assert_eq!(state.parent_target(), Some(SmartFolderParentTarget::Root));
        assert!(state.move_to(&root.join("child")));
        assert_eq!(
            state.parent_target(),
            Some(SmartFolderParentTarget::Folder(root))
        );
    }

    #[test]
    fn smart_folder_entry_order_moves_between_root_entries() {
        let id = uuid::Uuid::new_v4();
        let first = PathBuf::from(r"C:\books\first");
        let second = PathBuf::from(r"D:\library\second");
        let mut state = SmartFolderViewState::root(id, vec![first.clone(), second.clone()]);

        assert_eq!(state.entry_at_offset(true), Some(first.as_path()));
        assert_eq!(state.entry_at_offset(false), Some(second.as_path()));
        assert!(state.enter_containing_path(&first));
        assert_eq!(state.entry_at_offset(true), Some(second.as_path()));
        assert_eq!(state.entry_at_offset(false), None);
        assert!(state.enter_containing_path(&second));
        assert_eq!(state.entry_at_offset(true), None);
        assert_eq!(state.entry_at_offset(false), Some(first.as_path()));
    }

    #[test]
    fn smart_root_navigation_uses_one_display_order_for_folders_and_books() {
        use super::super::smart_folder::SmartChildKind;
        let id = uuid::Uuid::new_v4();
        let folder = PathBuf::from(r"C:\books\folder");
        let nested_pdf = folder.join("nested.pdf");
        let pdf = PathBuf::from(r"C:\books\book.pdf");
        let zip = PathBuf::from(r"C:\books\book.zip");
        let archive = PathBuf::from(r"C:\books\book.7z");
        let entries = vec![
            SmartRootNavEntry {
                logical_path: folder.clone(),
                kind: SmartChildKind::Folder,
            },
            SmartRootNavEntry {
                logical_path: nested_pdf.clone(),
                kind: SmartChildKind::Pdf,
            },
            SmartRootNavEntry {
                logical_path: pdf.clone(),
                kind: SmartChildKind::Pdf,
            },
            SmartRootNavEntry {
                logical_path: zip.clone(),
                kind: SmartChildKind::Zip,
            },
            SmartRootNavEntry {
                logical_path: archive.clone(),
                kind: SmartChildKind::ConvertibleArchive,
            },
        ];
        let mut state = SmartFolderViewState::root_with_navigation_entries(id, entries.clone());
        assert_eq!(state.entry_at_offset(true), Some(folder.as_path()));
        assert_eq!(state.entry_at_offset(false), Some(archive.as_path()));
        assert!(state.enter_containing_path(&nested_pdf));
        assert!(
            matches!(state.position, SmartFolderPosition::Container { .. }),
            "an exact root book must win over its ancestor Folder"
        );
        assert_eq!(state.entry_at_offset(true), Some(pdf.as_path()));
        assert_eq!(state.entry_at_offset(false), Some(folder.as_path()));
        assert!(state.enter_containing_path(&folder.join("chapter")));
        assert_eq!(state.entry_at_offset(true), Some(nested_pdf.as_path()));
        // A nested book may also have its own root row. The two entries are distinct
        // navigation contexts; re-entering it as a root row advances the index and is finite.
        assert!(state.move_to(&nested_pdf));
        assert_eq!(state.entry_at_offset(true), Some(nested_pdf.as_path()));
        assert!(state.enter_containing_path(&nested_pdf));
        assert_eq!(state.entry_at_offset(true), Some(pdf.as_path()));

        assert!(state.enter_containing_path(&folder.join("chapter")));

        let mut reordered = entries;
        reordered.swap(1, 2);
        assert!(state.refresh_navigation_entries(reordered));
        assert_eq!(state.entry_at_offset(true), Some(pdf.as_path()));
        assert!(!state.refresh_navigation_entries(vec![SmartRootNavEntry {
            logical_path: zip,
            kind: SmartChildKind::Zip,
        }]));
        assert!(matches!(state.position, SmartFolderPosition::Root));
    }

    #[test]
    fn smart_folder_refresh_reorders_retained_scope_and_drops_deleted_or_renamed_scope() {
        let id = uuid::Uuid::new_v4();
        let first = PathBuf::from(r"C:\books\first");
        let second = PathBuf::from(r"D:\library\second");
        let renamed = PathBuf::from(r"C:\books\renamed");
        let mut state = SmartFolderViewState::root(id, vec![first.clone(), second.clone()]);
        assert!(state.enter_containing_path(&first));
        assert!(state.refresh_folder_entries(vec![second.clone(), first.clone()]));
        assert_eq!(state.entry_index(), Some(1));
        assert!(state.move_to(&first.join("child")));

        assert!(!state.refresh_folder_entries(vec![renamed, second]));
        assert!(matches!(state.position, SmartFolderPosition::Root));
    }

    #[test]
    fn smart_folder_enters_most_specific_root_for_a_descendant() {
        let id = uuid::Uuid::new_v4();
        let parent = PathBuf::from(r"C:\books");
        let entry = parent.join("series");
        let current = entry.join("volume-1");
        let mut state = SmartFolderViewState::root(id, vec![parent, entry.clone()]);

        assert!(state.enter_containing_path(&current));
        assert_eq!(state.scoped_entry_root(), Some(entry.as_path()));
        assert_eq!(state.scoped_current(), Some(current.as_path()));
    }

    #[test]
    fn direct_top_level_transition_transfers_one_return_owner() {
        let origin = TopLevelGridRestore::Folder(PathBuf::from(r"D:\origin"));
        let mut view = TopLevelGridView::default();
        let first = view.begin(
            TopLevelGridSurface::Search(TopLevelSearchView::Global),
            Some(origin.clone()),
        );
        assert_eq!(first, 1);
        let transferred = view.take_return_to().expect("return owner");
        view.begin(
            TopLevelGridSurface::Search(TopLevelSearchView::Tag),
            Some(transferred),
        );
        assert!(matches!(
            view.return_to(),
            Some(TopLevelGridRestore::Folder(path)) if path == Path::new(r"D:\origin")
        ));
    }

    #[test]
    fn retiring_collection_surface_cancels_only_its_preparing_worker() {
        let first_id = crate::collection_store::CollectionId::new();
        let sibling_id = crate::collection_store::CollectionId::new();
        let first_cancel = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let sibling_cancel = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let (_first_tx, first_rx) = std::sync::mpsc::sync_channel(1);
        let (_sibling_tx, sibling_rx) = std::sync::mpsc::sync_channel(1);
        let stamp = |collection_id| CollectionGridRequestStamp {
            context_id: super::super::viewer_context_registry::ViewerContextId::for_test(1),
            surface_generation: 1,
            collection_id,
        };

        let mut first = TopLevelGridView::default();
        first.begin(
            TopLevelGridSurface::Collection(CollectionGridIdentity {
                collection_id: first_id,
            }),
            None,
        );
        first.collection_session_mut().unwrap().load = CollectionGridLoadState::Preparing {
            stamp: stamp(first_id),
            exact_revision: 1,
            installed: None,
            cancel: Arc::clone(&first_cancel),
            receiver: first_rx,
        };

        let mut sibling = TopLevelGridView::default();
        sibling.begin(
            TopLevelGridSurface::Collection(CollectionGridIdentity {
                collection_id: sibling_id,
            }),
            None,
        );
        sibling.collection_session_mut().unwrap().load = CollectionGridLoadState::Preparing {
            stamp: stamp(sibling_id),
            exact_revision: 1,
            installed: None,
            cancel: Arc::clone(&sibling_cancel),
            receiver: sibling_rx,
        };

        first.replace_surface(TopLevelGridSurface::Folder);
        assert!(first_cancel.load(std::sync::atomic::Ordering::Acquire));
        assert!(!sibling_cancel.load(std::sync::atomic::Ordering::Acquire));
    }

    #[test]
    fn bookmark_synthetic_path_restores_bookmark_surface() {
        // `bookmark_view_synthetic_path` reads the process-global data dir, and this test
        // reads it twice -- once to build the restore and once to compare. Another test
        // swapping `set_test_override` in between makes the two disagree, so take the
        // shared guard `data_dir::test_override_lock` documents for every `get()` caller.
        let _data_dir_guard = crate::data_dir::test_override_lock();
        let restore = TopLevelGridRestore::from_legacy_parts(
            Some(super::super::bookmark_view_synthetic_path()),
            None,
            None,
            None,
        );
        assert!(matches!(restore, TopLevelGridRestore::Bookmarks));
        assert!(crate::folder_tree::path_eq(
            &restore.legacy_path().expect("bookmark path"),
            &super::super::bookmark_view_synthetic_path()
        ));
    }
}
