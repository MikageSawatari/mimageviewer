//! Context-owned collection Grid lifecycle.
//!
//! The collection actor supplies an immutable logical snapshot. Filesystem classification runs on
//! a separate worker, and only the exact viewer-context/surface/revision owner may install it.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use super::top_level_grid_view::{
    CollectionGridIdentity, CollectionGridInstalledPresentation, CollectionGridLoadState,
    CollectionGridPhysicalLoadOrigin, CollectionGridPhysicalLoadOwner, CollectionGridPosition,
    CollectionGridPreparedInstall, CollectionGridPreparedThumbnailSources,
    CollectionGridRequestStamp, CollectionGridRestore, CollectionGridSession,
    CollectionGridSourceOpenOwner, CollectionGridThumbnailSourceIdentity,
    CollectionGridThumbnailSources, CollectionGridViewportAnchor, TopLevelGridRestore,
    TopLevelGridSurface,
};
use super::{App, GridItem, ViewerContextId};
use crate::collection_store::{
    CollectionEntryId, CollectionId, CollectionOrderMode, CollectionPrepareError,
    CollectionPreparedSnapshot, CollectionStoreError, prepare_collection_snapshot,
};

#[derive(Clone, Debug)]
pub(crate) struct CollectionGridRemoveTarget {
    pub(crate) stamp: CollectionGridRequestStamp,
    pub(crate) expected_revision: u64,
    pub(crate) entry_ids: Vec<CollectionEntryId>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct CollectionGridContentTarget {
    pub(crate) stamp: CollectionGridRequestStamp,
    pub(crate) expected_revision: u64,
    pub(crate) selected_entry_id: Option<CollectionEntryId>,
}

/// The visible order is read from the exact installed root, never from the independently
/// refreshed catalog or the global folder sort setting.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct CollectionGridOrderTarget {
    pub(crate) content: CollectionGridContentTarget,
    pub(crate) mode: CollectionOrderMode,
    pub(crate) standard_sort: crate::settings::SortOrder,
}

/// Delete-key/context-menu resolution for the mounted collection surface. Root is fail-closed:
/// an unavailable immutable binding must never fall through to a source-file deletion.
#[derive(Clone, Debug)]
pub(crate) enum CollectionRootDeleteResolution {
    Ready(CollectionGridRemoveTarget),
    Unavailable(&'static str),
    NotCollectionRoot,
}

impl CollectionRootDeleteResolution {
    pub(crate) fn is_collection_root(&self) -> bool {
        !matches!(self, Self::NotCollectionRoot)
    }

    pub(crate) fn ready_target(&self) -> Option<&CollectionGridRemoveTarget> {
        match self {
            Self::Ready(target) => Some(target),
            Self::Unavailable(_) | Self::NotCollectionRoot => None,
        }
    }
}

fn hash_collection_thumbnail_identity_part(digest: &mut sha2::Sha256, bytes: &[u8]) {
    use sha2::Digest as _;

    digest.update((bytes.len() as u64).to_le_bytes());
    digest.update(bytes);
}

pub(in crate::app) fn prepare_collection_grid_thumbnail_sources(
    sources: CollectionGridThumbnailSources,
    cancel: &AtomicBool,
) -> Result<CollectionGridPreparedThumbnailSources, CollectionPrepareError> {
    use sha2::Digest as _;

    if sources.video_sidecars.is_empty() && sources.video_pin_blobs.is_empty() {
        return Ok(CollectionGridPreparedThumbnailSources::default());
    }
    let mut digest = sha2::Sha256::new();
    digest.update(b"miv.collection-thumbnail-sources.v1\0");

    let mut sidecars = sources.video_sidecars.iter().collect::<Vec<_>>();
    sidecars.sort_unstable_by(|left, right| left.0.cmp(right.0));
    digest.update((sidecars.len() as u64).to_le_bytes());
    for (video_key, sidecar_path) in sidecars {
        if cancel.load(Ordering::Acquire) {
            return Err(CollectionPrepareError::Cancelled);
        }
        digest.update(b"sidecar\0");
        hash_collection_thumbnail_identity_part(&mut digest, video_key.as_bytes());
        let sidecar_key = crate::path_key::normalize_keep_drive(sidecar_path);
        hash_collection_thumbnail_identity_part(&mut digest, sidecar_key.as_bytes());
        match std::fs::metadata(sidecar_path) {
            Ok(metadata) => {
                digest.update([1, u8::from(metadata.is_file())]);
                digest.update(metadata.len().to_le_bytes());
                match metadata
                    .modified()
                    .ok()
                    .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
                {
                    Some(modified) => {
                        digest.update([1]);
                        digest.update(modified.as_secs().to_le_bytes());
                        digest.update(modified.subsec_nanos().to_le_bytes());
                    }
                    None => digest.update([0]),
                }
            }
            Err(_) => digest.update([0]),
        }
    }

    let mut pins = sources.video_pin_blobs.iter().collect::<Vec<_>>();
    pins.sort_unstable_by_key(|(path, _)| crate::path_key::normalize_keep_drive(path));
    digest.update((pins.len() as u64).to_le_bytes());
    for (video_path, webp) in pins {
        if cancel.load(Ordering::Acquire) {
            return Err(CollectionPrepareError::Cancelled);
        }
        digest.update(b"pin\0");
        let video_key = crate::path_key::normalize_keep_drive(video_path);
        hash_collection_thumbnail_identity_part(&mut digest, video_key.as_bytes());
        digest.update(sha2::Sha256::digest(webp));
    }

    Ok(CollectionGridPreparedThumbnailSources {
        identity: CollectionGridThumbnailSourceIdentity(digest.finalize().into()),
        payload: sources,
    })
}

pub(in crate::app) fn prepare_collection_grid_install(
    snapshot: &crate::collection_store::CollectionSnapshot,
    display_order: &crate::settings::GridDisplayOrder,
    settings: &crate::settings::Settings,
    cancel: &AtomicBool,
) -> Result<CollectionGridPreparedInstall, CollectionPrepareError> {
    let perf_start = crate::perf::is_enabled().then(Instant::now);
    let classify_start = crate::perf::is_enabled().then(Instant::now);
    let classified = prepare_collection_snapshot(snapshot, display_order, cancel, |_, _| {});
    collection_prepare_stage_event(
        snapshot,
        "classify",
        classify_start,
        snapshot.entries.len(),
        0,
        &classified,
    );
    let prepared = classified?;
    let videos = prepared
        .entries
        .iter()
        .filter_map(|entry| match &entry.item {
            GridItem::Video(path) => Some(path.clone()),
            _ => None,
        })
        .collect::<Vec<_>>();
    let sidecar_start = crate::perf::is_enabled().then(Instant::now);
    let sidecars =
        super::folder_scan::discover_aggregate_video_sidecars_while(settings, &videos, 64, || {
            !cancel.load(Ordering::Acquire)
        });
    if let Some(start) = sidecar_start {
        collection_prepare_stage_event(
            snapshot,
            "sidecar_scan",
            Some(start),
            videos.len(),
            sidecars.as_ref().map_or(0, |value| value.scanned_parents),
            &sidecars.as_ref().ok_or(CollectionPrepareError::Cancelled),
        );
    }
    let Some(sidecars) = sidecars else {
        return Err(CollectionPrepareError::Cancelled);
    };
    if sidecars.skipped_parents > 0 {
        crate::logger::log(format!(
            "collection grid: aggregate sidecar parent scan capped limit=64 scanned={} skipped_parents={}",
            sidecars.scanned_parents, sidecars.skipped_parents,
        ));
    }
    for (parent, error) in sidecars.scan_errors {
        crate::logger::log(format!(
            "collection grid: aggregate sidecar scan failed parent={} error={error}",
            parent.display()
        ));
    }
    if cancel.load(Ordering::Acquire) {
        return Err(CollectionPrepareError::Cancelled);
    }
    let pin_start = crate::perf::is_enabled().then(Instant::now);
    let pin_db =
        crate::video_pins::VideoPinDb::open_readonly(&crate::video_pins::VideoPinDb::db_path());
    let pin_db_available = pin_db.is_ok();
    let video_pin_blobs = pin_db
        .ok()
        .map(|db| db.lookup_webps_many(videos.iter()))
        .unwrap_or_default();
    if let Some(start) = pin_start {
        crate::perf::event(
            "collection",
            "prepare",
            None,
            0,
            &[
                (
                    "collection_id",
                    serde_json::Value::from(snapshot.collection_id().as_uuid().to_string()),
                ),
                ("revision", serde_json::Value::from(snapshot.revision())),
                ("stage", serde_json::Value::from("pin_db")),
                ("entries", serde_json::Value::from(videos.len())),
                (
                    "ms",
                    serde_json::Value::from(start.elapsed().as_secs_f64() * 1000.0),
                ),
                (
                    "outcome",
                    serde_json::Value::from(if pin_db_available { "ok" } else { "fallback" }),
                ),
            ],
        );
    }
    if cancel.load(Ordering::Acquire) {
        return Err(CollectionPrepareError::Cancelled);
    }
    let identity_start = crate::perf::is_enabled().then(Instant::now);
    let thumbnail_sources = prepare_collection_grid_thumbnail_sources(
        CollectionGridThumbnailSources {
            video_sidecars: sidecars.by_video_path,
            video_pin_blobs,
        },
        cancel,
    );
    collection_prepare_stage_event(
        snapshot,
        "identity",
        identity_start,
        videos.len(),
        0,
        &thumbnail_sources,
    );
    let thumbnail_sources = thumbnail_sources?;
    if let Some(start) = perf_start {
        crate::perf::event(
            "collection",
            "prepare",
            None,
            0,
            &[
                (
                    "collection_id",
                    serde_json::Value::from(snapshot.collection_id().as_uuid().to_string()),
                ),
                ("revision", serde_json::Value::from(snapshot.revision())),
                ("entries", serde_json::Value::from(snapshot.entries.len())),
                ("videos", serde_json::Value::from(videos.len())),
                (
                    "ms",
                    serde_json::Value::from(start.elapsed().as_secs_f64() * 1000.0),
                ),
                ("outcome", serde_json::Value::from("ok")),
            ],
        );
    }
    Ok(CollectionGridPreparedInstall {
        prepared,
        thumbnail_sources,
    })
}

fn collection_prepare_stage_event<T>(
    snapshot: &crate::collection_store::CollectionSnapshot,
    stage: &'static str,
    start: Option<Instant>,
    entries: usize,
    parents: usize,
    result: &Result<T, CollectionPrepareError>,
) {
    let Some(start) = start else { return };
    crate::perf::event(
        "collection",
        "prepare",
        None,
        0,
        &[
            (
                "collection_id",
                serde_json::Value::from(snapshot.collection_id().as_uuid().to_string()),
            ),
            ("revision", serde_json::Value::from(snapshot.revision())),
            ("stage", serde_json::Value::from(stage)),
            ("entries", serde_json::Value::from(entries)),
            ("parents", serde_json::Value::from(parents)),
            (
                "ms",
                serde_json::Value::from(start.elapsed().as_secs_f64() * 1000.0),
            ),
            (
                "outcome",
                serde_json::Value::from(match result {
                    Ok(_) => "ok",
                    Err(CollectionPrepareError::Cancelled) => "cancelled",
                    Err(_) => "error",
                }),
            ),
        ],
    );
}

impl App {
    pub(crate) fn collection_grid_context_id(&self) -> ViewerContextId {
        #[cfg(windows)]
        {
            self.projected_viewer_context_id()
        }
        #[cfg(not(windows))]
        {
            ViewerContextId::single_context()
        }
    }

    fn collection_grid_stamp(&self) -> Option<CollectionGridRequestStamp> {
        let TopLevelGridSurface::Collection(identity) = self.top_level_grid_view.surface() else {
            return None;
        };
        Some(CollectionGridRequestStamp {
            context_id: self.collection_grid_context_id(),
            surface_generation: self.top_level_grid_view.generation(),
            collection_id: identity.collection_id,
        })
    }

    fn collection_grid_stamp_is_current(&self, stamp: CollectionGridRequestStamp) -> bool {
        self.collection_grid_stamp() == Some(stamp)
    }

    pub(crate) fn collection_grid_request_stamp_is_current(
        &self,
        stamp: CollectionGridRequestStamp,
    ) -> bool {
        self.collection_grid_stamp_is_current(stamp)
            && self
                .top_level_grid_view
                .collection_session()
                .is_some_and(|session| {
                    matches!(session.position, CollectionGridPosition::Root)
                        && session.installed_items_generation == Some(self.items_generation)
                })
    }

    pub(crate) fn collection_grid_content_target(
        &self,
    ) -> Result<CollectionGridContentTarget, &'static str> {
        let Some(stamp) = self.collection_grid_stamp() else {
            return Err("現在の一覧はコレクション直下ではありません");
        };
        let Some(session) = self.top_level_grid_view.collection_session() else {
            return Err("コレクション一覧を更新中です");
        };
        if !matches!(session.position, CollectionGridPosition::Root)
            || session.installed_items_generation != Some(self.items_generation)
        {
            return Err("コレクション直下を開くと使用できます");
        }
        let Some(prepared) = session.prepared() else {
            return Err("コレクション一覧を更新中です");
        };
        if prepared.collection_revision != session.accepted_revision
            || prepared.entries.len() != self.items.len()
        {
            return Err("コレクション一覧を更新中です");
        }
        let selected_entry_id = self
            .selected
            .and_then(|index| prepared.entries.get(index))
            .map(|entry| entry.entry_id);
        Ok(CollectionGridContentTarget {
            stamp,
            expected_revision: prepared.collection_revision,
            selected_entry_id,
        })
    }

    pub(crate) fn collection_grid_root_order(
        &self,
    ) -> Option<Result<CollectionGridOrderTarget, &'static str>> {
        let session = self.top_level_grid_view.collection_session()?;
        if !matches!(session.position, CollectionGridPosition::Root) {
            return None;
        }
        match session.load {
            CollectionGridLoadState::Deleted => return Some(Err("コレクションは削除されました")),
            CollectionGridLoadState::Failed { .. } => {
                return Some(Err("コレクション一覧を読み込めませんでした"));
            }
            CollectionGridLoadState::Ready(_) | CollectionGridLoadState::Empty(_) => {}
            _ => return Some(Err("コレクション一覧を読み込み中です")),
        }
        if session.wanted_revision > session.accepted_revision {
            return Some(Err("コレクション一覧の更新を待っています"));
        }
        let content = match self.collection_grid_content_target() {
            Ok(target) => target,
            Err(reason) => return Some(Err(reason)),
        };
        let Some(prepared) = session.prepared() else {
            return Some(Err("コレクション一覧を更新中です"));
        };
        if prepared.collection_id != content.stamp.collection_id {
            return Some(Err("コレクション一覧を更新中です"));
        }
        Some(Ok(CollectionGridOrderTarget {
            content,
            mode: prepared.order_mode,
            standard_sort: prepared.standard_sort,
        }))
    }

    pub(crate) fn apply_collection_grid_remove_success(
        &mut self,
        stamp: CollectionGridRequestStamp,
        removed: &[CollectionEntryId],
    ) {
        #[cfg(windows)]
        if stamp.context_id != self.projected_viewer_context_id() {
            let _ = self.with_viewer_context(stamp.context_id, |owner| {
                owner.apply_collection_grid_remove_success_in_current_context(stamp, removed);
            });
            return;
        }
        self.apply_collection_grid_remove_success_in_current_context(stamp, removed);
    }

    fn apply_collection_grid_remove_success_in_current_context(
        &mut self,
        stamp: CollectionGridRequestStamp,
        removed: &[CollectionEntryId],
    ) {
        if !self.collection_grid_stamp_is_current(stamp) {
            return;
        }
        let Some(session) = self.top_level_grid_view.collection_session() else {
            return;
        };
        if !matches!(session.position, CollectionGridPosition::Root)
            || session.installed_items_generation != Some(self.items_generation)
        {
            return;
        }
        let Some(prepared) = session.prepared() else {
            return;
        };
        let removed_indices = prepared
            .entries
            .iter()
            .enumerate()
            .filter_map(|(index, entry)| removed.contains(&entry.entry_id).then_some(index))
            .collect::<std::collections::HashSet<_>>();
        self.checked
            .retain(|index| !removed_indices.contains(index));
        if self
            .selected
            .is_some_and(|index| removed_indices.contains(&index))
        {
            self.selected = None;
        }
    }

    fn invalidate_current_collection_grid_sources(
        &mut self,
        scopes: &[crate::delete_worker::DeleteSourceScope],
    ) {
        let Some(session) = self.top_level_grid_view.collection_session_mut() else {
            return;
        };
        let affected = session.prepared().is_some_and(|prepared| {
            prepared.entries.iter().any(|entry| {
                scopes
                    .iter()
                    .any(|scope| source_scope_contains(scope, &entry.source_path))
            })
        });
        if affected {
            // accepted_revision remains the actor revision. Clearing the filesystem preparation
            // makes the next mounted poll request the same logical snapshot and reclassify it.
            session.cancel_pending();
            session.installed_items_generation = None;
        }
    }

    pub(crate) fn invalidate_collection_grid_sources(
        &mut self,
        scopes: &[crate::delete_worker::DeleteSourceScope],
    ) {
        if scopes.is_empty() {
            return;
        }
        #[cfg(windows)]
        {
            let ids = self.viewer_context_ids();
            for id in ids {
                let _ = self.with_viewer_context(id, |app| {
                    app.invalidate_current_collection_grid_sources(scopes);
                });
            }
        }
        #[cfg(not(windows))]
        self.invalidate_current_collection_grid_sources(scopes);
    }

    /// Resolves a context-menu cell/checked selection through the currently installed immutable
    /// binding. A stale generation or a child physical view yields no target rather than mapping
    /// indices through another surface's items.
    pub(crate) fn collection_grid_remove_target(
        &self,
        item_index: Option<usize>,
        has_checked: bool,
    ) -> Option<CollectionGridRemoveTarget> {
        match self.collection_root_delete_resolution(item_index, has_checked) {
            CollectionRootDeleteResolution::Ready(target) => Some(target),
            CollectionRootDeleteResolution::Unavailable(_)
            | CollectionRootDeleteResolution::NotCollectionRoot => None,
        }
    }

    pub(crate) fn collection_root_delete_resolution(
        &self,
        item_index: Option<usize>,
        has_checked: bool,
    ) -> CollectionRootDeleteResolution {
        let TopLevelGridSurface::Collection(_) = self.top_level_grid_view.surface() else {
            return CollectionRootDeleteResolution::NotCollectionRoot;
        };
        let Some(session) = self.top_level_grid_view.collection_session() else {
            return CollectionRootDeleteResolution::Unavailable(
                "コレクション一覧を更新中のため、登録解除できません",
            );
        };
        if !matches!(session.position, CollectionGridPosition::Root) {
            return CollectionRootDeleteResolution::NotCollectionRoot;
        }
        let Some(stamp) = self.collection_grid_stamp() else {
            return CollectionRootDeleteResolution::Unavailable(
                "コレクション一覧を更新中のため、登録解除できません",
            );
        };
        if !matches!(session.position, CollectionGridPosition::Root)
            || session.installed_items_generation != Some(self.items_generation)
        {
            return CollectionRootDeleteResolution::Unavailable(
                "コレクション一覧を更新中のため、登録解除できません",
            );
        }
        let Some(prepared) = session.prepared() else {
            return CollectionRootDeleteResolution::Unavailable(
                "コレクション一覧を更新中のため、登録解除できません",
            );
        };
        if prepared.collection_revision != session.accepted_revision
            || prepared.entries.len() != self.items.len()
        {
            return CollectionRootDeleteResolution::Unavailable(
                "コレクション一覧を更新中のため、登録解除できません",
            );
        }
        let mut indices: Vec<usize> = if has_checked {
            self.checked.iter().copied().collect()
        } else {
            let Some(item_index) = item_index else {
                return CollectionRootDeleteResolution::Unavailable(
                    "登録解除する項目を選択してください",
                );
            };
            vec![item_index]
        };
        indices.sort_unstable();
        indices.dedup();
        if indices.is_empty() {
            return CollectionRootDeleteResolution::Unavailable(
                "登録解除する項目を選択してください",
            );
        }
        let Some(entry_ids) = indices
            .into_iter()
            .map(|index| prepared.entries.get(index).map(|entry| entry.entry_id))
            .collect::<Option<Vec<_>>>()
        else {
            return CollectionRootDeleteResolution::Unavailable(
                "コレクション一覧を更新中のため、登録解除できません",
            );
        };
        CollectionRootDeleteResolution::Ready(CollectionGridRemoveTarget {
            stamp,
            expected_revision: prepared.collection_revision,
            entry_ids,
        })
    }

    pub(crate) fn collection_grid_restore_snapshot(&self) -> Option<TopLevelGridRestore> {
        let session = self.top_level_grid_view.collection_session()?;
        let viewport_anchor = session
            .installed_items_generation
            .filter(|generation| *generation == self.items_generation)
            .and_then(|_| self.selected)
            .and_then(|index| session.prepared()?.entries.get(index))
            .map(|entry| CollectionGridViewportAnchor {
                entry_id: entry.entry_id,
                source_key: entry.source_key.clone(),
            })
            .or_else(|| session.restore_anchor.clone());
        Some(TopLevelGridRestore::Collection(CollectionGridRestore {
            identity: session.identity,
            revision_at_open: session.accepted_revision.max(session.wanted_revision),
            viewport_anchor,
        }))
    }

    pub(crate) fn collection_grid_parent_nav(&self) -> Option<crate::ui_main::AddressBarNav> {
        let session = self.top_level_grid_view.collection_session()?;
        let CollectionGridPosition::PhysicalSource {
            entry_id,
            source_key,
            ..
        } = &session.position
        else {
            return None;
        };
        Some(crate::ui_main::AddressBarNav::Collection(
            CollectionGridRestore {
                identity: session.identity,
                revision_at_open: session.accepted_revision.max(session.wanted_revision),
                viewport_anchor: Some(CollectionGridViewportAnchor {
                    entry_id: *entry_id,
                    source_key: source_key.clone(),
                }),
            },
        ))
    }

    /// Captures the stable collection origin before a physical container begins loading. Leaf
    /// media remain at the collection root and use the same immutable item binding in fullscreen.
    pub(crate) fn collection_grid_source_anchor(
        &self,
        index: usize,
        path: &std::path::Path,
    ) -> Option<CollectionGridViewportAnchor> {
        let generation = self.items_generation;
        self.top_level_grid_view
            .collection_session()
            .filter(|session| {
                matches!(session.position, CollectionGridPosition::Root)
                    && session.installed_items_generation == Some(generation)
            })
            .and_then(CollectionGridSession::prepared)
            .and_then(|prepared| prepared.entries.get(index))
            .filter(|entry| crate::folder_tree::path_eq(&entry.source_path, path))
            .map(|entry| CollectionGridViewportAnchor {
                entry_id: entry.entry_id,
                source_key: entry.source_key.clone(),
            })
    }

    /// Captures the complete owner for an asynchronous archive probe/conversion initiated by a
    /// collection cell. A later completion may mutate the collection position only while this
    /// exact context, surface generation, collection, and revision pair is still current.
    pub(crate) fn collection_grid_source_open_owner(
        &self,
        index: usize,
        path: &std::path::Path,
    ) -> Option<CollectionGridSourceOpenOwner> {
        let stamp = self.collection_grid_stamp()?;
        let session = self.top_level_grid_view.collection_session()?;
        let anchor = self.collection_grid_source_anchor(index, path)?;
        Some(CollectionGridSourceOpenOwner {
            stamp,
            accepted_revision: session.accepted_revision,
            wanted_revision: session.wanted_revision,
            anchor,
            navigation_prepared: None,
            navigation_origin: None,
            navigation_request: None,
            navigation_watch: None,
        })
    }

    /// Capture the exact collection-owned physical load represented by a grid item.
    ///
    /// At the root the item must still be the immutable prepared entry selected by `index`.
    /// Inside a physical source the root anchor remains stable while `target_path` identifies the
    /// descendant requested by this one interaction. Callers must carry the returned owner through
    /// scan/conversion and may not reconstruct it from the later selection.
    pub(crate) fn collection_grid_physical_load_owner(
        &self,
        index: usize,
        target_path: &std::path::Path,
    ) -> Option<CollectionGridPhysicalLoadOwner> {
        let stamp = self.collection_grid_stamp()?;
        let session = self.top_level_grid_view.collection_session()?;
        match &session.position {
            CollectionGridPosition::Root => {
                let anchor = self.collection_grid_source_anchor(index, target_path)?;
                Some(CollectionGridPhysicalLoadOwner {
                    stamp,
                    accepted_revision: session.accepted_revision,
                    wanted_revision: session.wanted_revision,
                    anchor,
                    root_source_path: target_path.to_path_buf(),
                    target_path: target_path.to_path_buf(),
                    origin: CollectionGridPhysicalLoadOrigin::Root {
                        items_generation: self.items_generation,
                    },
                })
            }
            CollectionGridPosition::PhysicalSource {
                entry_id,
                source_key,
                path,
            } => Some(CollectionGridPhysicalLoadOwner {
                stamp,
                accepted_revision: session.accepted_revision,
                wanted_revision: session.wanted_revision,
                anchor: CollectionGridViewportAnchor {
                    entry_id: *entry_id,
                    source_key: source_key.clone(),
                },
                root_source_path: path.clone(),
                target_path: target_path.to_path_buf(),
                origin: CollectionGridPhysicalLoadOrigin::PhysicalSource {
                    current_path: self.effective_folder()?,
                },
            }),
        }
    }

    /// Capture a collection-owned in-place reload. Only a mounted physical descendant can own
    /// this route; root materialization has its own actor/prepare lifecycle.
    pub(crate) fn collection_grid_physical_reload_owner(
        &self,
        target_path: &std::path::Path,
    ) -> Option<CollectionGridPhysicalLoadOwner> {
        self.collection_grid_physical_load_owner(self.selected.unwrap_or(0), target_path)
            .filter(|owner| {
                matches!(
                    owner.origin,
                    CollectionGridPhysicalLoadOrigin::PhysicalSource { .. }
                )
            })
    }

    pub(crate) fn grid_physical_navigation(
        &self,
        index: usize,
        path: std::path::PathBuf,
    ) -> crate::ui_main::AddressBarNav {
        match self.collection_grid_physical_load_owner(index, &path) {
            Some(owner) => crate::ui_main::AddressBarNav::CollectionSource { path, owner },
            None => crate::ui_main::AddressBarNav::Direct(path),
        }
    }

    pub(crate) fn collection_grid_physical_load_owner_is_current(
        &self,
        owner: &CollectionGridPhysicalLoadOwner,
        target_path: &std::path::Path,
    ) -> bool {
        if !crate::folder_tree::path_eq(&owner.target_path, target_path)
            || !self.collection_grid_stamp_is_current(owner.stamp)
        {
            return false;
        }
        let Some(session) = self.top_level_grid_view.collection_session() else {
            return false;
        };
        if session.accepted_revision != owner.accepted_revision {
            return false;
        }
        match &owner.origin {
            CollectionGridPhysicalLoadOrigin::Root { items_generation } => {
                let root_entry_is_current = session.prepared().is_some_and(|prepared| {
                    prepared.entries.iter().any(|entry| {
                        entry.entry_id == owner.anchor.entry_id
                            && entry.source_key == owner.anchor.source_key
                            && crate::folder_tree::path_eq(
                                &entry.source_path,
                                &owner.root_source_path,
                            )
                    })
                });
                root_entry_is_current
                    && matches!(session.position, CollectionGridPosition::Root)
                    && session.wanted_revision == owner.wanted_revision
                    && session.installed_items_generation == Some(*items_generation)
                    && self.items_generation == *items_generation
                    && crate::folder_tree::path_eq(&owner.root_source_path, target_path)
            }
            CollectionGridPhysicalLoadOrigin::PhysicalSource { current_path } => {
                matches!(
                    &session.position,
                    CollectionGridPosition::PhysicalSource {
                        entry_id,
                        source_key,
                        path,
                    } if *entry_id == owner.anchor.entry_id
                        && *source_key == owner.anchor.source_key
                        && crate::folder_tree::path_eq(path, &owner.root_source_path)
                ) && self
                    .effective_folder()
                    .as_deref()
                    .is_some_and(|current| crate::folder_tree::path_eq(current, current_path))
            }
        }
    }

    /// Commit one already-adopted physical load. Root opens advance to PhysicalSource; descendant
    /// loads and same-folder reloads keep the original collection entry as their parent anchor.
    pub(crate) fn commit_collection_grid_physical_load(
        &mut self,
        owner: &CollectionGridPhysicalLoadOwner,
        target_path: &std::path::Path,
    ) -> bool {
        if !self.collection_grid_physical_load_owner_is_current(owner, target_path) {
            return false;
        }
        if matches!(owner.origin, CollectionGridPhysicalLoadOrigin::Root { .. }) {
            // Root thumbnails are generation-owned presentation work. Once the physical child
            // is adopted, that root is no longer visible, including for ZIP/PDF children whose
            // collection session remains mounted for parent navigation. The shared image pool
            // remains owned by ViewerContextBundle and is intentionally not cancelled here.
            if let Some(session) = self.top_level_grid_view.collection_session_mut()
                && let Some(cancel) = session.video_worker_cancel.take()
            {
                cancel.store(true, Ordering::Release);
            }
            self.commit_collection_grid_source_open(
                owner.anchor.clone(),
                owner.root_source_path.clone(),
            );
        }
        true
    }

    pub(crate) fn collection_grid_source_open_owner_is_current(
        &self,
        owner: &CollectionGridSourceOpenOwner,
        path: &std::path::Path,
    ) -> bool {
        if !self.collection_grid_stamp_is_current(owner.stamp) {
            return false;
        }
        let Some(session) = self.top_level_grid_view.collection_session() else {
            return false;
        };
        if let Some(prepared) = owner.navigation_prepared.as_ref() {
            // The grid and navigation use independent revision watches. Navigation may already
            // own exact revision N while the mounted grid still reports N-1, then the grid watch
            // may catch up to N while an archive conversion is pending. Only a revision newer
            // than the transferred exact prepared snapshot makes that navigation owner stale.
            if session.wanted_revision > prepared.collection_revision {
                return false;
            }
        } else if session.accepted_revision != owner.accepted_revision
            || session.wanted_revision != owner.wanted_revision
        {
            return false;
        }
        if !self.collection_navigation_source_owner_is_current(owner) {
            return false;
        }
        if let Some(origin) = &owner.navigation_origin {
            if !matches!(
                &session.position,
                CollectionGridPosition::PhysicalSource {
                    entry_id,
                    source_key,
                    ..
                } if *entry_id == origin.entry_id && *source_key == origin.source_key
            ) && !(matches!(session.position, CollectionGridPosition::Root)
                && session.installed_items_generation == Some(self.items_generation))
            {
                return false;
            }
        } else if !matches!(session.position, CollectionGridPosition::Root)
            || session.installed_items_generation != Some(self.items_generation)
        {
            return false;
        }
        owner
            .navigation_prepared
            .as_ref()
            .or_else(|| session.prepared())
            .is_some_and(|prepared| {
                (owner.navigation_prepared.is_some()
                    || prepared.collection_revision == owner.accepted_revision)
                    && prepared.entries.iter().any(|entry| {
                        entry.entry_id == owner.anchor.entry_id
                            && entry.source_key == owner.anchor.source_key
                            && crate::folder_tree::path_eq(&entry.source_path, path)
                    })
            })
    }

    pub(crate) fn commit_collection_grid_source_open_owned(
        &mut self,
        owner: &CollectionGridSourceOpenOwner,
        path: std::path::PathBuf,
    ) -> bool {
        if !self.collection_grid_source_open_owner_landed_is_current(owner, &path) {
            return false;
        }
        if let Some(prepared) = owner.navigation_prepared.clone()
            && let Some(session) = self.top_level_grid_view.collection_session_mut()
        {
            session.accepted_revision = prepared.collection_revision;
            session.wanted_revision = session.wanted_revision.max(prepared.collection_revision);
            let presentation = Arc::new(CollectionGridInstalledPresentation::new(
                prepared,
                CollectionGridThumbnailSourceIdentity::default(),
            ));
            session.load = if presentation.prepared.entries.is_empty() {
                CollectionGridLoadState::Empty(presentation)
            } else {
                CollectionGridLoadState::Ready(presentation)
            };
            session.installed_items_generation = None;
        }
        self.commit_collection_grid_source_open(owner.anchor.clone(), path);
        true
    }

    pub(crate) fn collection_grid_source_open_owner_landed_is_current(
        &self,
        owner: &CollectionGridSourceOpenOwner,
        path: &std::path::Path,
    ) -> bool {
        if owner.navigation_request.is_none() {
            return self.collection_grid_source_open_owner_is_current(owner, path);
        }
        if !self.collection_grid_stamp_is_current(owner.stamp)
            || !self.collection_navigation_source_owner_landed_is_current(owner)
            || !self
                .effective_folder()
                .as_deref()
                .is_some_and(|current| crate::folder_tree::path_eq(current, path))
        {
            return false;
        }
        let Some(session) = self.top_level_grid_view.collection_session() else {
            return false;
        };
        session.identity.collection_id == owner.stamp.collection_id
            && owner
                .navigation_prepared
                .as_ref()
                .is_some_and(|prepared| session.wanted_revision <= prepared.collection_revision)
    }

    pub(crate) fn commit_collection_grid_source_open(
        &mut self,
        anchor: CollectionGridViewportAnchor,
        path: std::path::PathBuf,
    ) {
        if let Some(session) = self.top_level_grid_view.collection_session_mut() {
            session.restore_anchor = Some(anchor.clone());
            session.position = CollectionGridPosition::PhysicalSource {
                entry_id: anchor.entry_id,
                source_key: anchor.source_key,
                path,
            };
        }
    }

    /// Captures a collection return owner for a new physical viewer context without mutating the
    /// still-mounted collection root. Detached folder/archive classification may finish later, so
    /// the complete owner must travel with that request rather than be reconstructed from App.
    pub(crate) fn collection_grid_restore_for_source(
        &self,
        index: usize,
        path: &std::path::Path,
    ) -> Option<CollectionGridRestore> {
        let anchor = self.collection_grid_source_anchor(index, path)?;
        let session = self.top_level_grid_view.collection_session()?;
        Some(CollectionGridRestore {
            identity: session.identity,
            revision_at_open: session.accepted_revision.max(session.wanted_revision),
            viewport_anchor: Some(anchor),
        })
    }

    fn retire_transient_views_for_collection(&mut self) -> Option<TopLevelGridRestore> {
        let current = self.current_top_level_restore_snapshot();
        let mut origin = self.dismiss_snapshot_without_restore();
        if self.favsearch.active {
            origin = Some(self.dismiss_favsearch_without_restore());
        }
        if self.global_search.active {
            origin = Some(self.dismiss_global_search_without_restore());
        }
        if self.tag_view.active {
            origin = Some(self.dismiss_tag_view_without_restore());
        }
        if self.show_search_bar {
            self.show_search_bar = false;
            self.search_query.clear();
            self.search_filter = None;
            self.search_filter_origin_folder = None;
            self.search_has_focus = false;
            self.search_tag_bridge.clear();
            self.cancel_search_pending();
        }
        self.cancel_pending_folder_nav();
        origin.or(current)
    }

    /// Opens the collection root. `restore` carries a minimum revision hint and a stable entry
    /// anchor; neither value is treated as an exact actor reply revision.
    pub(crate) fn open_collection_grid(
        &mut self,
        collection_id: CollectionId,
        restore: Option<CollectionGridRestore>,
    ) {
        let perf_start = crate::perf::is_enabled().then(Instant::now);
        let restoring = restore.is_some();
        let return_to = if restore.is_some() {
            None
        } else if matches!(
            self.top_level_grid_view.surface(),
            TopLevelGridSurface::Collection(identity) if identity.collection_id == collection_id
        ) {
            self.top_level_grid_view.return_to().cloned()
        } else {
            self.retire_transient_views_for_collection()
        };
        let identity = CollectionGridIdentity { collection_id };
        self.top_level_grid_view
            .begin(TopLevelGridSurface::Collection(identity), return_to);
        let watch = self
            .collection_store_client()
            .and_then(|client| client.subscribe().ok());
        if let Some(session) = self.top_level_grid_view.collection_session_mut() {
            session.wanted_revision = restore.as_ref().map_or(0, |state| state.revision_at_open);
            session.restore_anchor = restore.and_then(|state| state.viewport_anchor);
            session.watch = watch;
        }

        // Do not leave a prior physical/search grid interactive while the actor and classifier are
        // resolving this collection. The empty install performs no filesystem/database access.
        self.install_collection_grid_items(Vec::new(), Vec::new(), None);
        // A header choice belongs to the prior root. A collection open, including history and
        // same-root reopen, starts from its installed collection order.
        self.reset_details_sort_to_toolbar();
        self.address = "コレクションを読み込み中…".into();
        self.schedule_collection_grid_snapshot();
        if let Some(start) = perf_start {
            crate::perf::event(
                "collection",
                "open",
                None,
                0,
                &[
                    (
                        "collection_id",
                        serde_json::Value::from(collection_id.as_uuid().to_string()),
                    ),
                    (
                        "context",
                        serde_json::Value::from(format!("{:?}", self.collection_grid_context_id())),
                    ),
                    ("restore", serde_json::Value::from(restoring)),
                    (
                        "ms",
                        serde_json::Value::from(start.elapsed().as_secs_f64() * 1000.0),
                    ),
                ],
            );
        }
    }

    /// Explicit user navigation into a collection participates in the same Back/Forward history
    /// as physical folders. Actor refreshes, toolbar Add, and collection-owned child navigation
    /// continue to call `open_collection_grid` directly and therefore do not create entries.
    pub(crate) fn open_collection_grid_from_navigation(&mut self, collection_id: CollectionId) {
        let revision_at_open = self.collection_catalog_revision(collection_id).unwrap_or(0);
        let restore = CollectionGridRestore {
            identity: CollectionGridIdentity { collection_id },
            revision_at_open,
            viewport_anchor: None,
        };
        self.record_collection_nav_transition(restore);
        self.open_collection_grid(collection_id, None);
    }

    fn schedule_collection_grid_snapshot(&mut self) {
        if !self.collection_grid_root_materialize_active() {
            return;
        }
        let Some(stamp) = self.collection_grid_stamp() else {
            return;
        };
        let Some(session) = self.top_level_grid_view.collection_session() else {
            return;
        };
        if !matches!(session.load, CollectionGridLoadState::RequestNeeded { .. }) {
            return;
        }
        let minimum_revision = session.wanted_revision.max(session.accepted_revision);
        let installed = session.load.installed().cloned();
        let Some(client) = self.collection_store_client() else {
            if let Some(session) = self.top_level_grid_view.collection_session_mut() {
                session.load = CollectionGridLoadState::Failed {
                    message: "コレクションを利用できません".into(),
                    installed,
                };
            }
            return;
        };
        let queued_at = crate::perf::is_enabled().then(Instant::now);
        match client.load_collection(stamp.collection_id) {
            Ok(receiver) => {
                if let Some(session) = self.top_level_grid_view.collection_session_mut() {
                    session.load = CollectionGridLoadState::Snapshot {
                        stamp,
                        minimum_revision,
                        queued_at,
                        installed,
                        receiver,
                    };
                }
            }
            Err(error) => {
                if let Some(session) = self.top_level_grid_view.collection_session_mut() {
                    session.load = CollectionGridLoadState::Failed {
                        message: collection_grid_error(&error),
                        installed,
                    };
                }
            }
        }
    }

    fn spawn_collection_grid_prepare(
        &mut self,
        stamp: CollectionGridRequestStamp,
        snapshot: crate::collection_store::CollectionSnapshot,
        installed: Option<Arc<CollectionPreparedSnapshot>>,
    ) {
        let exact_revision = snapshot.revision();
        let display_order = self.settings.grid_display_order.clone();
        let settings = self.settings.clone();
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = Arc::clone(&cancel);
        let (sender, receiver) = std::sync::mpsc::sync_channel(1);
        let spawn = std::thread::Builder::new()
            .name("collection-grid-prepare".into())
            .spawn(move || {
                let result = prepare_collection_grid_install(
                    &snapshot,
                    &display_order,
                    &settings,
                    &worker_cancel,
                );
                let _ = sender.send(result);
            });
        match spawn {
            Ok(_) => {
                if let Some(session) = self.top_level_grid_view.collection_session_mut() {
                    session.load = CollectionGridLoadState::Preparing {
                        stamp,
                        exact_revision,
                        installed,
                        cancel,
                        receiver,
                    };
                }
            }
            Err(error) => {
                if let Some(session) = self.top_level_grid_view.collection_session_mut() {
                    session.load = CollectionGridLoadState::Failed {
                        message: format!("コレクション一覧を準備できません: {error}"),
                        installed,
                    };
                }
            }
        }
    }

    fn collection_grid_root_materialize_active(&self) -> bool {
        self.fullscreen_idx.is_none()
            && self
                .top_level_grid_view
                .collection_session()
                .is_some_and(|session| matches!(session.position, CollectionGridPosition::Root))
    }

    /// A newer root is waiting for the fullscreen leaf to release the installed item indices.
    /// This is a display reason only: the existing poll/navigation owners decide when to install.
    pub(crate) fn collection_grid_refresh_waits_for_viewer(&self) -> bool {
        let Some(index) = self.fullscreen_idx else {
            return false;
        };
        let Some(session) = self.top_level_grid_view.collection_session() else {
            return false;
        };
        let Some(stamp) = self.collection_grid_stamp() else {
            return false;
        };
        let Some(prepared) = session.prepared() else {
            return false;
        };
        matches!(session.position, CollectionGridPosition::Root)
            && session.installed_items_generation == Some(self.items_generation)
            && prepared.collection_id == stamp.collection_id
            && prepared.collection_revision == session.accepted_revision
            && prepared.entries.len() == self.items.len()
            && index < self.items.len()
            && (session.wanted_revision > session.accepted_revision
                || matches!(session.load, CollectionGridLoadState::RequestNeeded { .. }))
    }

    pub(crate) fn collection_grid_empty_message(&self) -> Option<String> {
        let session = self.top_level_grid_view.collection_session()?;
        if !matches!(session.position, CollectionGridPosition::Root) {
            return None;
        }
        match &session.load {
            CollectionGridLoadState::RequestNeeded { installed: None }
            | CollectionGridLoadState::Snapshot { .. }
            | CollectionGridLoadState::Preparing { .. } => Some("コレクションを読み込み中…".into()),
            CollectionGridLoadState::RequestNeeded { installed: Some(_) } => None,
            CollectionGridLoadState::Empty(_) => Some("コレクションに項目はありません".into()),
            CollectionGridLoadState::Failed {
                message,
                installed: None,
            } => Some(message.clone()),
            CollectionGridLoadState::Failed {
                installed: Some(_), ..
            } => None,
            CollectionGridLoadState::Deleted => Some("コレクションは削除されました".into()),
            CollectionGridLoadState::Ready(_) => None,
        }
    }

    pub(crate) fn collection_grid_stale_error_message(&self) -> Option<&str> {
        let session = self.top_level_grid_view.collection_session()?;
        if !matches!(session.position, CollectionGridPosition::Root) {
            return None;
        }
        match &session.load {
            CollectionGridLoadState::Failed {
                message,
                installed: Some(_),
            } => Some(message),
            _ => None,
        }
    }

    /// Drained before viewport/fullscreen early returns. No branch blocks the UI thread.
    pub(crate) fn poll_collection_grid(&mut self, ctx: &egui::Context) {
        // A duplicated/parked viewer context carries the immutable prepared result, but each
        // context needs its own fan-out receiver. Reattach lazily after the context becomes
        // current instead of copying a single-consumer receiver across viewports.
        let needs_watch = self
            .top_level_grid_view
            .collection_session()
            .is_some_and(|session| session.watch.is_none());
        if needs_watch
            && let Some(watch) = self
                .collection_store_client()
                .and_then(|client| client.subscribe().ok())
            && let Some(session) = self.top_level_grid_view.collection_session_mut()
        {
            session.watch = Some(watch);
        }
        let notice = self
            .top_level_grid_view
            .collection_session()
            .and_then(|session| session.watch.as_ref())
            .and_then(crate::collection_store::CollectionRevisionWatch::take_latest);
        let mut deleted_current_root = false;
        if let Some(notice) = notice
            && let Some(stamp) = self.collection_grid_stamp()
            && let Some(session) = self.top_level_grid_view.collection_session_mut()
            && notice.catalog_revision > session.observed_catalog_revision
        {
            session.observed_catalog_revision = notice.catalog_revision;
            if let Some((_, revision)) = notice
                .collection_revisions
                .iter()
                .find(|(id, _)| *id == stamp.collection_id)
            {
                if *revision > session.wanted_revision {
                    session.wanted_revision = *revision;
                    if matches!(session.position, CollectionGridPosition::Root) {
                        session.cancel_pending();
                    }
                }
            } else {
                session.cancel_pending();
                session.load = CollectionGridLoadState::Deleted;
                deleted_current_root = matches!(session.position, CollectionGridPosition::Root)
                    && self.fullscreen_idx.is_none();
            }
        }
        if deleted_current_root {
            self.install_collection_grid_items(Vec::new(), Vec::new(), None);
            if let Some(session) = self.top_level_grid_view.collection_session_mut() {
                session.installed_items_generation = None;
            }
            self.address = "コレクションは削除されました".into();
            ctx.request_repaint();
        }

        // A physical child and a fullscreen leaf retain the exact installed binding. Revision
        // notices only advance wanted_revision until the owning context returns to its root Grid.
        if !self.collection_grid_root_materialize_active() {
            return;
        }

        // A delete notice may have arrived while a leaf was fullscreen. The notice owns the
        // terminal state immediately, while presentation replacement waits until the root Grid
        // can be changed without rebinding the fullscreen index.
        let presents_deleted_rows =
            self.top_level_grid_view
                .collection_session()
                .is_some_and(|session| {
                    matches!(session.load, CollectionGridLoadState::Deleted)
                        && session.installed_items_generation == Some(self.items_generation)
                });
        if presents_deleted_rows {
            self.install_collection_grid_items(Vec::new(), Vec::new(), None);
            if let Some(session) = self.top_level_grid_view.collection_session_mut() {
                session.installed_items_generation = None;
            }
            self.address = "コレクションは削除されました".into();
            ctx.request_repaint();
        }

        let pending = self
            .top_level_grid_view
            .collection_session_mut()
            .and_then(|session| {
                if matches!(
                    session.load,
                    CollectionGridLoadState::Snapshot { .. }
                        | CollectionGridLoadState::Preparing { .. }
                ) {
                    Some(std::mem::replace(
                        &mut session.load,
                        CollectionGridLoadState::RequestNeeded { installed: None },
                    ))
                } else {
                    None
                }
            });
        match pending {
            Some(CollectionGridLoadState::Snapshot {
                stamp,
                minimum_revision,
                queued_at,
                installed,
                receiver,
            }) => {
                let reply = receiver.try_recv();
                if let Some(start) = queued_at
                    && !matches!(&reply, Err(crossbeam_channel::TryRecvError::Empty))
                {
                    let (outcome, entries) = match &reply {
                        Ok(Ok(snapshot))
                            if !self.collection_grid_stamp_is_current(stamp)
                                || snapshot.collection_id() != stamp.collection_id
                                || snapshot.revision() < minimum_revision
                                || self.top_level_grid_view.collection_session().is_some_and(
                                    |session| snapshot.revision() < session.wanted_revision,
                                ) =>
                        {
                            ("stale", snapshot.entries.len())
                        }
                        Ok(Ok(snapshot)) => ("ok", snapshot.entries.len()),
                        Ok(Err(_)) => ("error", 0),
                        Err(_) => ("disconnected", 0),
                    };
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
                                "context",
                                serde_json::Value::from(format!("{:?}", stamp.context_id)),
                            ),
                            ("entries", serde_json::Value::from(entries)),
                            (
                                "ms",
                                serde_json::Value::from(start.elapsed().as_secs_f64() * 1000.0),
                            ),
                            ("outcome", serde_json::Value::from(outcome)),
                        ],
                    );
                }
                match reply {
                    Ok(Ok(snapshot)) => {
                        if self.collection_grid_stamp_is_current(stamp) {
                            let wanted = self
                                .top_level_grid_view
                                .collection_session()
                                .map_or(minimum_revision, |session| session.wanted_revision);
                            if snapshot.collection_id() == stamp.collection_id
                                && snapshot.revision() >= minimum_revision
                                && snapshot.revision() >= wanted
                            {
                                if let Some(session) =
                                    self.top_level_grid_view.collection_session_mut()
                                {
                                    session.observed_catalog_revision = session
                                        .observed_catalog_revision
                                        .max(snapshot.catalog_revision);
                                }
                                self.spawn_collection_grid_prepare(stamp, snapshot, installed);
                            } else if let Some(session) =
                                self.top_level_grid_view.collection_session_mut()
                            {
                                session.load = CollectionGridLoadState::RequestNeeded { installed };
                            }
                        }
                    }
                    Ok(Err(error)) => {
                        if self.collection_grid_stamp_is_current(stamp)
                            && let Some(session) = self.top_level_grid_view.collection_session_mut()
                        {
                            session.load = if matches!(error, CollectionStoreError::NotFound) {
                                CollectionGridLoadState::Deleted
                            } else {
                                CollectionGridLoadState::Failed {
                                    message: collection_grid_error(&error),
                                    installed,
                                }
                            };
                        }
                    }
                    Err(crossbeam_channel::TryRecvError::Empty) => {
                        if self.collection_grid_stamp_is_current(stamp)
                            && let Some(session) = self.top_level_grid_view.collection_session_mut()
                        {
                            session.load = CollectionGridLoadState::Snapshot {
                                stamp,
                                minimum_revision,
                                queued_at,
                                installed,
                                receiver,
                            };
                        }
                    }
                    Err(crossbeam_channel::TryRecvError::Disconnected) => {
                        if self.collection_grid_stamp_is_current(stamp)
                            && let Some(session) = self.top_level_grid_view.collection_session_mut()
                        {
                            session.load = CollectionGridLoadState::Failed {
                                message: "コレクション一覧の応答が失われました".into(),
                                installed,
                            };
                        }
                    }
                }
            }
            Some(CollectionGridLoadState::Preparing {
                stamp,
                exact_revision,
                installed,
                cancel,
                receiver,
            }) => match receiver.try_recv() {
                Ok(Ok(prepared)) => {
                    let accepts = self.collection_grid_stamp_is_current(stamp)
                        && prepared.prepared.collection_id == stamp.collection_id
                        && prepared.prepared.collection_revision == exact_revision
                        && self
                            .top_level_grid_view
                            .collection_session()
                            .is_some_and(|session| session.wanted_revision <= exact_revision);
                    if !accepts && crate::perf::is_enabled() {
                        crate::perf::event(
                            "collection",
                            "prepare_result",
                            None,
                            0,
                            &[
                                (
                                    "collection_id",
                                    serde_json::Value::from(
                                        stamp.collection_id.as_uuid().to_string(),
                                    ),
                                ),
                                (
                                    "request_generation",
                                    serde_json::Value::from(stamp.surface_generation),
                                ),
                                ("revision", serde_json::Value::from(exact_revision)),
                                (
                                    "entries",
                                    serde_json::Value::from(prepared.prepared.entries.len()),
                                ),
                                ("outcome", serde_json::Value::from("stale")),
                            ],
                        );
                    }
                    if accepts {
                        self.apply_collection_grid_prepared_install(prepared, installed);
                        ctx.request_repaint();
                    }
                }
                Ok(Err(CollectionPrepareError::Cancelled)) => {
                    collection_grid_prepare_result_event(stamp, exact_revision, "cancelled");
                    if self.collection_grid_stamp_is_current(stamp)
                        && let Some(session) = self.top_level_grid_view.collection_session_mut()
                    {
                        session.load = CollectionGridLoadState::RequestNeeded { installed };
                    }
                }
                Ok(Err(error)) => {
                    collection_grid_prepare_result_event(stamp, exact_revision, "error");
                    if self.collection_grid_stamp_is_current(stamp)
                        && let Some(session) = self.top_level_grid_view.collection_session_mut()
                    {
                        session.load = CollectionGridLoadState::Failed {
                            message: error.to_string(),
                            installed,
                        };
                    }
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => {
                    if self.collection_grid_stamp_is_current(stamp) {
                        if let Some(session) = self.top_level_grid_view.collection_session_mut() {
                            session.load = CollectionGridLoadState::Preparing {
                                stamp,
                                exact_revision,
                                installed,
                                cancel,
                                receiver,
                            };
                        }
                    } else {
                        cancel.store(true, Ordering::Release);
                    }
                }
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    collection_grid_prepare_result_event(stamp, exact_revision, "disconnected");
                    if self.collection_grid_stamp_is_current(stamp)
                        && let Some(session) = self.top_level_grid_view.collection_session_mut()
                    {
                        session.load = CollectionGridLoadState::Failed {
                            message: "コレクション準備workerが終了しました".into(),
                            installed,
                        };
                    }
                }
            },
            Some(
                CollectionGridLoadState::RequestNeeded { .. }
                | CollectionGridLoadState::Ready(_)
                | CollectionGridLoadState::Empty(_)
                | CollectionGridLoadState::Failed { .. }
                | CollectionGridLoadState::Deleted,
            )
            | None => {}
        }
        self.schedule_collection_grid_snapshot();
    }

    pub(in crate::app) fn apply_collection_grid_prepared_install(
        &mut self,
        install: CollectionGridPreparedInstall,
        previous: Option<Arc<CollectionPreparedSnapshot>>,
    ) {
        let perf_start = crate::perf::is_enabled().then(Instant::now);
        let CollectionGridPreparedInstall {
            prepared,
            thumbnail_sources,
        } = install;
        let CollectionGridPreparedThumbnailSources {
            identity: thumbnail_source_identity,
            payload:
                CollectionGridThumbnailSources {
                    video_sidecars,
                    video_pin_blobs,
                },
        } = thumbnail_sources;
        let prepared = Arc::new(prepared);
        if previous.as_ref().is_some_and(|old| {
            old.order_mode != prepared.order_mode || old.standard_sort != prepared.standard_sort
        }) && self.settings.details_sort_key != crate::settings::DetailsSortKey::Toolbar
        {
            self.settings.details_sort_key = crate::settings::DetailsSortKey::Toolbar;
            self.settings.details_sort_ascending = true;
            self.settings.save();
        }
        let old_binding_is_current =
            self.top_level_grid_view
                .collection_session()
                .is_some_and(|session| {
                    session.installed_items_generation == Some(self.items_generation)
                });
        let old_selected = old_binding_is_current
            .then_some(self.selected)
            .flatten()
            .and_then(|index| previous.as_ref()?.entries.get(index))
            .map(|entry| CollectionGridViewportAnchor {
                entry_id: entry.entry_id,
                source_key: entry.source_key.clone(),
            });
        let old_checked = if old_binding_is_current {
            self.checked
                .iter()
                .filter_map(|index| previous.as_ref()?.entries.get(*index))
                .map(|entry| CollectionGridViewportAnchor {
                    entry_id: entry.entry_id,
                    source_key: entry.source_key.clone(),
                })
                .collect::<Vec<_>>()
        } else {
            Vec::new()
        };
        let old_search_anchors = if old_binding_is_current {
            self.search_filter.as_ref().map(|filter| {
                filter
                    .iter()
                    .filter_map(|index| previous.as_ref()?.entries.get(*index))
                    .map(|entry| CollectionGridViewportAnchor {
                        entry_id: entry.entry_id,
                        source_key: entry.source_key.clone(),
                    })
                    .collect::<Vec<_>>()
            })
        } else {
            None
        };
        let old_search_query = old_binding_is_current.then(|| self.search_query.clone());
        let old_search_origin = old_binding_is_current
            .then(|| self.search_filter_origin_folder.clone())
            .flatten();
        let restore_anchor = self
            .top_level_grid_view
            .collection_session_mut()
            .and_then(|session| session.restore_anchor.take());
        let items = prepared
            .entries
            .iter()
            .map(|entry| entry.item.clone())
            .collect();
        let image_metas = prepared
            .entries
            .iter()
            .map(|entry| entry.display_meta)
            .collect();
        let selected = restore_anchor
            .as_ref()
            .or(old_selected.as_ref())
            .and_then(|anchor| {
                prepared
                    .entries
                    .iter()
                    .position(|entry| entry.entry_id == anchor.entry_id)
                    .or_else(|| {
                        prepared
                            .entries
                            .iter()
                            .position(|entry| entry.source_key == anchor.source_key)
                    })
            });
        let checked = old_checked
            .iter()
            .filter_map(|anchor| {
                prepared
                    .entries
                    .iter()
                    .position(|entry| entry.entry_id == anchor.entry_id)
            })
            .collect::<std::collections::HashSet<_>>();
        self.install_collection_grid_items_with_thumbnail_sources(
            items,
            image_metas,
            selected,
            video_sidecars,
            video_pin_blobs,
        );
        self.checked = checked;
        if let Some(query) = old_search_query {
            self.search_query = query;
            self.search_filter_origin_folder = old_search_origin;
        }
        if let Some(anchors) = old_search_anchors {
            self.search_filter = Some(
                anchors
                    .iter()
                    .filter_map(|anchor| {
                        prepared
                            .entries
                            .iter()
                            .position(|entry| entry.entry_id == anchor.entry_id)
                            .or_else(|| {
                                prepared
                                    .entries
                                    .iter()
                                    .position(|entry| entry.source_key == anchor.source_key)
                            })
                    })
                    .collect(),
            );
            self.rebuild_visible_indices();
        }
        self.address = format!("コレクション: {}", prepared.collection_name);
        let installed_generation = self.items_generation;
        if let Some(session) = self.top_level_grid_view.collection_session_mut() {
            session.position = CollectionGridPosition::Root;
            session.accepted_revision = prepared.collection_revision;
            session.wanted_revision = session.wanted_revision.max(prepared.collection_revision);
            session.installed_items_generation = Some(installed_generation);
            let presentation = Arc::new(CollectionGridInstalledPresentation::new(
                prepared,
                thumbnail_source_identity,
            ));
            session.load = if presentation.prepared.entries.is_empty() {
                CollectionGridLoadState::Empty(presentation)
            } else {
                CollectionGridLoadState::Ready(presentation)
            };
        }
        // install_collection_grid_items rebuilds details while the old load is still Preparing.
        // Rebuild once the exact new order is published so Standard's display-only header choice
        // can resume without leaking an old Standard choice into Manual or Shuffle.
        if self.settings.details_sort_key != crate::settings::DetailsSortKey::Toolbar {
            self.rebuild_details_order();
        }
        if let Some(start) = perf_start {
            crate::perf::event(
                "collection",
                "install",
                None,
                0,
                &[
                    (
                        "collection_id",
                        serde_json::Value::from(
                            self.top_level_grid_view
                                .collection_session()
                                .map(|session| {
                                    session.identity.collection_id.as_uuid().to_string()
                                }),
                        ),
                    ),
                    (
                        "request_generation",
                        serde_json::Value::from(self.top_level_grid_view.generation()),
                    ),
                    ("entries", serde_json::Value::from(self.items.len())),
                    (
                        "ms",
                        serde_json::Value::from(start.elapsed().as_secs_f64() * 1000.0),
                    ),
                    ("reused", serde_json::Value::from(false)),
                ],
            );
        }
    }

    #[cfg(test)]
    pub(in crate::app) fn apply_collection_grid_prepared(
        &mut self,
        prepared: Arc<CollectionPreparedSnapshot>,
        previous: Option<Arc<CollectionPreparedSnapshot>>,
    ) {
        self.apply_collection_grid_prepared_install(
            CollectionGridPreparedInstall {
                prepared: (*prepared).clone(),
                thumbnail_sources: CollectionGridPreparedThumbnailSources::default(),
            },
            previous,
        );
    }

    fn install_collection_grid_items(
        &mut self,
        items: Vec<GridItem>,
        image_metas: Vec<Option<(i64, i64)>>,
        selected: Option<usize>,
    ) {
        self.install_collection_grid_items_with_thumbnail_sources(
            items,
            image_metas,
            selected,
            std::collections::HashMap::new(),
            std::collections::HashMap::new(),
        );
    }

    fn install_collection_grid_items_with_thumbnail_sources(
        &mut self,
        items: Vec<GridItem>,
        image_metas: Vec<Option<(i64, i64)>>,
        selected: Option<usize>,
        video_sidecars: std::collections::HashMap<String, std::path::PathBuf>,
        video_pin_blobs: std::collections::HashMap<std::path::PathBuf, Vec<u8>>,
    ) {
        if let Some(session) = self.top_level_grid_view.collection_session_mut()
            && let Some(cancel) = session.video_worker_cancel.take()
        {
            cancel.store(true, Ordering::Release);
        }
        // The regular thumbnail pool belongs to this ViewerContextBundle. Rebuild it for the new
        // aggregate generation, but do not transfer ownership to the collection session: another
        // synthetic surface may intentionally reuse this pool after the collection is retired.
        self.bump_full_context_for_load();
        let image_worker_cancel = Arc::clone(&self.cancel_token);
        let (tx, rx) = std::sync::mpsc::channel();
        self.tx = tx.clone();
        self.rx = rx;
        self.current_folder = None;
        self.archive_source_override = None;
        self.zip_nav = None;
        self.stack_view = None;
        self.stack_mode_requested = false;
        self.cancel_stack_script_pending();
        self.install_prepared_aggregate_items(items, image_metas);
        self.invalidate_idx_state_and_queues();
        self.clear_page_edit_state();
        self.metadata_cache.clear();
        self.exif_cache.clear();
        self.xmp_cache.clear();
        self.clear_tags_cache();
        self.folder_pin_map.clear();
        self.converted_archive_cache_paths.clear();
        self.video_thumb_overrides = video_sidecars;
        self.search_filter = None;
        self.search_query.clear();

        self.selected = selected;
        self.scroll_to_selected = selected.is_some();
        if selected.is_none() {
            self.scroll_offset_y = 0.0;
        }
        self.rebuild_visible_indices();
        self.prewarm_grid_tags();

        let cache_map = Arc::new(std::sync::RwLock::new(std::collections::HashMap::new()));
        self.current_color_cache_map = Some(Arc::clone(&cache_map));
        self.current_color_catalog = None;
        self.reset_and_seed_auto_aspect(&cache_map);
        self.cache_gen_total = 0;
        self.cache_gen_done = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let initial_display_px = super::compute_display_px(
            self.last_cell_size,
            self.last_cell_h,
            self.last_pixels_per_point,
        );
        self.display_px_shared
            .store(initial_display_px, Ordering::Relaxed);

        if self.items.is_empty() {
            self.reload_queue = None;
            self.heavy_io_queue = None;
            return;
        }

        let reload_queue: Arc<super::NotifyQueue> =
            Arc::new((std::sync::Mutex::new(Vec::new()), std::sync::Condvar::new()));
        let heavy_io_queue: Arc<super::NotifyQueue> =
            Arc::new((std::sync::Mutex::new(Vec::new()), std::sync::Condvar::new()));
        self.reload_queue = Some(Arc::clone(&reload_queue));
        self.heavy_io_queue = Some(Arc::clone(&heavy_io_queue));
        self.spawn_thumbnail_workers(
            &tx,
            image_worker_cancel,
            reload_queue,
            heavy_io_queue,
            cache_map,
            None,
            self.folder_thumb_pin_db.clone(),
        );

        let video_items = self
            .items
            .iter()
            .enumerate()
            .filter_map(|(index, item)| match item {
                GridItem::Video(path) => Some((
                    index,
                    path.clone(),
                    self.image_metas
                        .get(index)
                        .and_then(|meta| *meta)
                        .map_or(0, |(_, size)| size.max(0) as u64),
                )),
                _ => None,
            })
            .collect::<Vec<_>>();
        if !video_items.is_empty() {
            let video_worker_cancel = Arc::new(AtomicBool::new(false));
            self.spawn_video_thread(
                tx,
                Arc::clone(&video_worker_cancel),
                video_items,
                self.video_thumb_overrides.clone(),
                video_pin_blobs,
            );
            if let Some(session) = self.top_level_grid_view.collection_session_mut() {
                session.video_worker_cancel = Some(video_worker_cancel);
            }
        }
    }
}

fn source_scope_contains(
    scope: &crate::delete_worker::DeleteSourceScope,
    source: &std::path::Path,
) -> bool {
    let source_key = crate::path_key::normalize_keep_drive(source);
    let (root, tree) = match scope {
        crate::delete_worker::DeleteSourceScope::Exact(path) => {
            (crate::path_key::normalize_keep_drive(path), false)
        }
        crate::delete_worker::DeleteSourceScope::Tree(path) => {
            (crate::path_key::normalize_keep_drive(path), true)
        }
    };
    source_key == root
        || tree
            && source_key
                .strip_prefix(root.trim_end_matches('/'))
                .is_some_and(|suffix| suffix.starts_with('/'))
}

fn collection_grid_prepare_result_event(
    stamp: CollectionGridRequestStamp,
    revision: u64,
    outcome: &'static str,
) {
    if !crate::perf::is_enabled() {
        return;
    }
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
            ("revision", serde_json::Value::from(revision)),
            ("outcome", serde_json::Value::from(outcome)),
        ],
    );
}

fn collection_grid_error(error: &CollectionStoreError) -> String {
    match error {
        CollectionStoreError::Starting => "コレクションを準備しています".into(),
        CollectionStoreError::Busy => "コレクション処理が混み合っています".into(),
        CollectionStoreError::Unavailable => "コレクションを利用できません".into(),
        CollectionStoreError::NotFound => "コレクションが見つかりません".into(),
        _ => format!("コレクションを読み込めません: {error}"),
    }
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};
    use std::time::{Duration, Instant};

    use super::*;
    use crate::collection_store::{
        CollectionRegistration, CollectionResolvedKind, CollectionStoreRuntime,
    };

    fn recv<T>(receiver: crossbeam_channel::Receiver<Result<T, CollectionStoreError>>) -> T {
        receiver
            .recv_timeout(Duration::from_secs(3))
            .expect("collection actor reply")
            .expect("collection actor operation")
    }

    fn start_ready_app(
        db_path: &Path,
    ) -> (
        crate::app::AppTestEnvForTest,
        crate::collection_store::CollectionStoreClient,
    ) {
        let runtime =
            CollectionStoreRuntime::start_at(db_path.to_path_buf()).expect("collection runtime");
        let client = runtime.client();
        let mut app = crate::app::setup_app_for_test();
        app.install_collection_runtime(runtime);
        let ctx = egui::Context::default();
        let deadline = Instant::now() + Duration::from_secs(3);
        while app.collection_store_client().is_none() {
            assert!(
                Instant::now() < deadline,
                "collection runtime did not become ready"
            );
            app.poll_collection_ui(&ctx);
            std::thread::sleep(Duration::from_millis(2));
        }
        (app, client)
    }

    fn wait_for_grid(app: &mut App, collection_id: CollectionId) {
        let ctx = egui::Context::default();
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            app.poll_collection_ui(&ctx);
            app.poll_collection_grid(&ctx);
            let ready = app
                .top_level_grid_view
                .collection_session()
                .is_some_and(|session| {
                    session.identity.collection_id == collection_id
                        && matches!(
                            session.load,
                            CollectionGridLoadState::Ready(_) | CollectionGridLoadState::Empty(_)
                        )
                        && session.installed_items_generation == Some(app.items_generation)
                });
            if ready {
                return;
            }
            assert!(Instant::now() < deadline, "collection Grid did not settle");
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    fn poll_until(app: &mut App, message: &str, mut condition: impl FnMut(&App) -> bool) {
        let ctx = egui::Context::default();
        let deadline = Instant::now() + Duration::from_secs(5);
        while !condition(app) {
            assert!(Instant::now() < deadline, "{message}");
            app.poll_collection_ui(&ctx);
            app.poll_collection_grid(&ctx);
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    fn collection_with_sources(
        client: &crate::collection_store::CollectionStoreClient,
        sources: &[(PathBuf, CollectionResolvedKind)],
    ) -> crate::collection_store::CollectionSnapshot {
        let created = recv(
            client
                .create_collection("Grid integration".into())
                .expect("create request"),
        );
        let registrations = sources
            .iter()
            .map(|(path, kind)| {
                CollectionRegistration::from_trusted_path(path, *kind).expect("registration")
            })
            .collect();
        recv(
            client
                .add_batch(created.collection_id(), created.revision(), registrations)
                .expect("add request"),
        )
        .snapshot
    }

    #[test]
    fn root_order_controls_use_installed_revision_and_preserve_global_sort_and_header_contract() {
        use crate::collection_store::CollectionOrderMode;
        use crate::settings::{DetailsSortKey, GridViewMode, SortOrder};
        use crate::ui_dialogs::collections::CollectionGridSnapshotAction;

        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source.png");
        std::fs::write(&source, b"source").unwrap();
        let (mut app, client) = start_ready_app(&temp.path().join("collection.db"));
        let created = collection_with_sources(&client, &[(source, CollectionResolvedKind::Image)]);
        app.settings.grid_view_mode = GridViewMode::Details;
        app.settings.sort_order = SortOrder::DateDesc;
        app.settings.details_sort_key = DetailsSortKey::Name;
        app.open_collection_grid(created.collection_id(), None);
        assert_eq!(app.settings.details_sort_key, DetailsSortKey::Toolbar);
        assert!(app.collection_grid_root_order().unwrap().is_err());
        assert_eq!(
            app.grid_sort_lock_reason(),
            Some(super::super::GridSortLockReason::CollectionLoading)
        );
        wait_for_grid(&mut app, created.collection_id());
        let manual = app.collection_grid_root_order().unwrap().unwrap();
        assert_eq!(manual.mode, CollectionOrderMode::Manual);
        assert!(app.details_header_sort_locked());
        assert_eq!(app.grid_sort_lock_reason(), None);

        app.start_collection_grid_content_action(
            manual.content,
            CollectionGridSnapshotAction::SetOrder {
                mode: CollectionOrderMode::Standard,
                sort: SortOrder::FileName,
            },
        );
        poll_until(
            &mut app,
            "Standard order did not install",
            |app| matches!(app.collection_grid_root_order(), Some(Ok(order)) if order.mode == CollectionOrderMode::Standard && order.content.expected_revision > manual.content.expected_revision),
        );
        assert_eq!(app.settings.sort_order, SortOrder::DateDesc);
        assert!(!app.details_header_sort_locked());
        app.settings.details_sort_key = DetailsSortKey::Name;
        app.rebuild_details_order();
        assert!(app.details_header_sort_active());
        assert_eq!(
            app.grid_sort_lock_reason(),
            Some(super::super::GridSortLockReason::DetailsHeaderSort)
        );

        let standard = app.collection_grid_root_order().unwrap().unwrap();
        app.start_collection_grid_content_action(
            standard.content,
            CollectionGridSnapshotAction::SetOrder {
                mode: CollectionOrderMode::Shuffle,
                sort: standard.standard_sort,
            },
        );
        poll_until(
            &mut app,
            "Shuffle order did not install",
            |app| matches!(app.collection_grid_root_order(), Some(Ok(order)) if order.mode == CollectionOrderMode::Shuffle && order.content.expected_revision > standard.content.expected_revision),
        );
        assert_eq!(app.settings.details_sort_key, DetailsSortKey::Toolbar);
        assert!(app.details_header_sort_locked());
        assert_eq!(app.grid_sort_lock_reason(), None);
        assert_eq!(app.settings.sort_order, SortOrder::DateDesc);

        let shuffle = app.collection_grid_root_order().unwrap().unwrap();
        app.start_collection_grid_content_action(
            shuffle.content,
            CollectionGridSnapshotAction::SetOrder {
                mode: CollectionOrderMode::Shuffle,
                sort: shuffle.standard_sort,
            },
        );
        poll_until(
            &mut app,
            "Shuffle reselection did not install",
            |app| matches!(app.collection_grid_root_order(), Some(Ok(order)) if order.mode == CollectionOrderMode::Shuffle && order.content.expected_revision > shuffle.content.expected_revision),
        );
        assert_eq!(app.settings.sort_order, SortOrder::DateDesc);

        let anchor = app
            .top_level_grid_view
            .collection_session()
            .unwrap()
            .prepared()
            .unwrap()
            .entries[0]
            .clone();
        let child = temp.path().join("child.zip");
        app.top_level_grid_view
            .collection_session_mut()
            .unwrap()
            .position = CollectionGridPosition::PhysicalSource {
            entry_id: anchor.entry_id,
            source_key: anchor.source_key,
            path: child.clone(),
        };
        app.current_folder = Some(child);
        app.items = vec![
            GridItem::Image(temp.path().join("b.jpg")),
            GridItem::Image(temp.path().join("a.jpg")),
        ];
        app.visible_indices = vec![0, 1];
        app.settings.details_sort_key = DetailsSortKey::Name;
        app.rebuild_details_order();
        assert!(app.collection_grid_root_order().is_none());
        assert!(app.page_order_locked_for_current_view());
        assert!(!app.details_header_sort_active());
        assert_eq!(app.details_order, vec![0, 1]);
        app.shutdown_collection_runtime_for_exit();
    }

    #[test]
    fn standard_root_header_sort_only_reorders_rows_not_reader_or_spread() {
        use crate::collection_store::CollectionOrderMode;
        use crate::settings::{DetailsSortKey, GridViewMode, SortOrder, SpreadMode};
        use crate::ui_dialogs::collections::CollectionGridSnapshotAction;
        use crate::ui_fullscreen::SpreadPair;

        let temp = tempfile::tempdir().unwrap();
        let sources = ["c.png", "a.png", "b.png"]
            .into_iter()
            .map(|name| {
                let source = temp.path().join(name);
                std::fs::write(&source, b"source").unwrap();
                (source, CollectionResolvedKind::Image)
            })
            .collect::<Vec<_>>();
        let (mut app, client) = start_ready_app(&temp.path().join("collection.db"));
        let created = collection_with_sources(&client, &sources);
        app.settings.grid_view_mode = GridViewMode::Details;
        app.open_collection_grid(created.collection_id(), None);
        wait_for_grid(&mut app, created.collection_id());

        let manual = app.collection_grid_root_order().unwrap().unwrap();
        app.start_collection_grid_content_action(
            manual.content,
            CollectionGridSnapshotAction::SetOrder {
                mode: CollectionOrderMode::Standard,
                sort: SortOrder::FileName,
            },
        );
        poll_until(&mut app, "Standard order did not install", |app| {
            matches!(
                app.collection_grid_root_order(),
                Some(Ok(order)) if order.mode == CollectionOrderMode::Standard
                    && order.content.expected_revision > manual.content.expected_revision
            )
        });
        assert_eq!(
            app.items
                .iter()
                .map(|item| item.name().into_owned())
                .collect::<Vec<_>>(),
            ["a.png", "b.png", "c.png"]
        );

        app.settings.details_sort_key = DetailsSortKey::Name;
        app.settings.details_sort_ascending = false;
        app.rebuild_details_order();
        assert_eq!(app.current_grid_order(), &[2, 1, 0]);
        assert_eq!(app.current_reader_order(), &[0, 1, 2]);
        assert_eq!(app.get_still_image_indices().as_ref(), &[0, 1, 2]);
        assert_eq!(app.fullscreen_boundary_jump_target(0, true), Some(2));
        app.spread_mode = SpreadMode::Ltr;
        assert_eq!(
            app.resolve_spread_pair(0),
            SpreadPair::Double { left: 0, right: 1 }
        );

        // A watch notice may make the root stale while fullscreen still owns the installed rows.
        app.top_level_grid_view
            .collection_session_mut()
            .unwrap()
            .wanted_revision += 1;
        assert!(app.collection_grid_root_order().unwrap().is_err());
        assert_eq!(app.current_reader_order(), &[0, 1, 2]);

        // A physical child keeps its existing details/navigation order.
        let entry = app
            .top_level_grid_view
            .collection_session()
            .unwrap()
            .prepared()
            .unwrap()
            .entries[0]
            .clone();
        app.top_level_grid_view
            .collection_session_mut()
            .unwrap()
            .position = CollectionGridPosition::PhysicalSource {
            entry_id: entry.entry_id,
            source_key: entry.source_key,
            path: temp.path().join("child"),
        };
        assert_eq!(app.current_reader_order(), app.current_grid_order());
        app.shutdown_collection_runtime_for_exit();
    }

    #[test]
    fn explicit_collection_open_participates_in_typed_back_forward_history() {
        let temp = tempfile::tempdir().unwrap();
        let physical_a = temp.path().join("physical-a");
        let physical_b = temp.path().join("physical-b");
        std::fs::create_dir(&physical_a).unwrap();
        std::fs::create_dir(&physical_b).unwrap();
        std::fs::write(physical_b.join("page.jpg"), b"page").unwrap();
        let (mut app, client) = start_ready_app(&temp.path().join("collection.db"));
        app.active_quick_folder_slot = None;
        app.current_folder = Some(physical_a.clone());

        let first = recv(
            client
                .create_collection("First".into())
                .expect("create first"),
        );
        let second = recv(
            client
                .create_collection("Second".into())
                .expect("create second"),
        );
        app.open_collection_grid_from_navigation(first.collection_id());
        wait_for_grid(&mut app, first.collection_id());
        assert!(matches!(
            app.folder_history_back_target(),
            Some(super::super::FolderNavHistoryTarget::Path(path))
                if crate::folder_tree::path_eq(path, &physical_a)
        ));

        app.open_collection_grid_from_navigation(second.collection_id());
        wait_for_grid(&mut app, second.collection_id());
        assert!(matches!(
            app.folder_history_back_target(),
            Some(super::super::FolderNavHistoryTarget::Collection(restore))
                if restore.identity.collection_id == first.collection_id()
        ));

        let renamed_first = recv(
            client
                .rename_collection(
                    first.collection_id(),
                    first.revision(),
                    "First renamed".into(),
                )
                .expect("rename first while its history entry is parked"),
        );

        let back = app.navigate_folder_history_back().expect("back to first");
        assert_eq!(
            app.dispatch_synthetic_folder_history_target(&back),
            super::super::SyntheticFolderHistoryDispatch::Restored
        );
        wait_for_grid(&mut app, first.collection_id());
        assert_eq!(
            app.top_level_grid_view
                .collection_session()
                .unwrap()
                .accepted_revision,
            renamed_first.revision(),
            "history owns the stable collection id and reloads its latest revision",
        );
        assert_eq!(app.address, "コレクション: First renamed");
        assert!(matches!(
            app.folder_history_forward_target(),
            Some(super::super::FolderNavHistoryTarget::Collection(restore))
                if restore.identity.collection_id == second.collection_id()
        ));

        let scan =
            super::super::folder_scan::scan_directory_with_settings(&physical_b, &app.settings)
                .unwrap();
        assert!(app.load_folder_with_scan_owned(
            physical_b.clone(),
            Some(scan),
            super::super::OpenRequestOwner::Navigation,
        ));
        assert!(matches!(
            app.folder_history_back_target(),
            Some(super::super::FolderNavHistoryTarget::Collection(restore))
                if restore.identity.collection_id == first.collection_id()
        ));
        let back = app
            .navigate_folder_history_back()
            .expect("back to first again");
        assert_eq!(
            app.dispatch_synthetic_folder_history_target(&back),
            super::super::SyntheticFolderHistoryDispatch::Restored
        );
        wait_for_grid(&mut app, first.collection_id());
        assert!(matches!(
            app.folder_history_forward_target(),
            Some(super::super::FolderNavHistoryTarget::Path(path))
                if crate::folder_tree::path_eq(path, &physical_b)
        ));
        app.shutdown_collection_runtime_for_exit();
    }

    #[test]
    fn failed_independent_folder_load_keeps_collection_surface_and_history_unchanged() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source.png");
        std::fs::write(&source, b"source").unwrap();
        let (mut app, client) = start_ready_app(&temp.path().join("collection.db"));
        let snapshot = collection_with_sources(&client, &[(source, CollectionResolvedKind::Image)]);
        app.open_collection_grid_from_navigation(snapshot.collection_id());
        wait_for_grid(&mut app, snapshot.collection_id());
        let before = app.folder_nav_history_snapshot();

        assert!(!app.load_folder_with_scan_owned(
            temp.path().join("missing-folder"),
            None,
            super::super::OpenRequestOwner::Navigation,
        ));
        assert!(matches!(
            app.top_level_grid_view.surface(),
            TopLevelGridSurface::Collection(identity)
                if identity.collection_id == snapshot.collection_id()
        ));
        assert_eq!(app.folder_nav_back_stack, before.back_stack);
        assert_eq!(app.folder_nav_forward_stack, before.forward_stack);
        app.shutdown_collection_runtime_for_exit();
    }

    #[test]
    fn deleted_mounted_root_is_not_recaptured_by_back_or_physical_navigation() {
        let temp = tempfile::tempdir().unwrap();
        let physical_a = temp.path().join("physical-a");
        let physical_b = temp.path().join("physical-b");
        std::fs::create_dir(&physical_a).unwrap();
        std::fs::create_dir(&physical_b).unwrap();
        std::fs::write(physical_b.join("page.jpg"), b"page").unwrap();
        let (mut app, client) = start_ready_app(&temp.path().join("collection.db"));
        app.active_quick_folder_slot = None;
        app.current_folder = Some(physical_a.clone());
        let snapshot = recv(
            client
                .create_collection("Deleted while mounted".into())
                .expect("create collection"),
        );
        app.open_collection_grid_from_navigation(snapshot.collection_id());
        wait_for_grid(&mut app, snapshot.collection_id());
        assert!(matches!(
            app.folder_history_back_target(),
            Some(super::super::FolderNavHistoryTarget::Path(path))
                if crate::folder_tree::path_eq(path, &physical_a)
        ));

        recv(
            client
                .delete_collection(snapshot.collection_id(), snapshot.revision())
                .expect("delete collection"),
        );
        poll_until(&mut app, "deleted root did not settle", |app| {
            !app.collection_catalog_contains(snapshot.collection_id())
                && app
                    .top_level_grid_view
                    .collection_session()
                    .is_some_and(|session| matches!(session.load, CollectionGridLoadState::Deleted))
        });

        let back = app
            .navigate_folder_history_back()
            .expect("physical A remains the valid Back target");
        assert!(matches!(
            back,
            super::super::FolderNavHistoryTarget::Path(ref path)
                if crate::folder_tree::path_eq(path, &physical_a)
        ));
        assert!(
            app.folder_nav_forward_stack
                .iter()
                .all(|target| target.collection_id() != Some(snapshot.collection_id())),
            "the mounted Deleted presentation is feedback, not a Forward destination",
        );

        // The Back target is only selected above; until dispatch, the Deleted root remains the
        // visible surface. Recreate that valid physical target and verify a successful independent
        // load cannot capture the deleted collection as its origin either.
        app.set_active_folder_nav_suppress_record_once(false);
        app.folder_nav_back_stack = vec![super::super::FolderNavHistoryTarget::Path(
            physical_a.clone(),
        )];
        let scan =
            super::super::folder_scan::scan_directory_with_settings(&physical_b, &app.settings)
                .unwrap();
        assert!(app.load_folder_with_scan_owned(
            physical_b.clone(),
            Some(scan),
            super::super::OpenRequestOwner::Navigation,
        ));
        assert_eq!(app.current_folder.as_deref(), Some(physical_b.as_path()));
        assert!(
            app.folder_nav_back_stack
                .iter()
                .all(|target| target.collection_id() != Some(snapshot.collection_id())),
            "successful physical adoption must not reinsert an authoritatively deleted ID",
        );
        app.shutdown_collection_runtime_for_exit();
    }

    #[test]
    fn root_delete_resolution_is_checked_first_fail_closed_and_child_physical() {
        let temp = tempfile::tempdir().unwrap();
        let first = temp.path().join("first.png");
        let second = temp.path().join("second.png");
        let folder = temp.path().join("folder");
        std::fs::write(&first, b"first").unwrap();
        std::fs::write(&second, b"second").unwrap();
        std::fs::create_dir(&folder).unwrap();
        let (mut app, client) = start_ready_app(&temp.path().join("collection.db"));
        let snapshot = collection_with_sources(
            &client,
            &[
                (first.clone(), CollectionResolvedKind::Image),
                (second.clone(), CollectionResolvedKind::Image),
                (folder.clone(), CollectionResolvedKind::Folder),
            ],
        );
        app.open_collection_grid(snapshot.collection_id(), None);
        wait_for_grid(&mut app, snapshot.collection_id());
        let prepared = app
            .top_level_grid_view
            .collection_session()
            .and_then(CollectionGridSession::prepared)
            .unwrap()
            .clone();
        let first_index = prepared
            .entries
            .iter()
            .position(|entry| entry.source_path == first)
            .unwrap();
        let second_index = prepared
            .entries
            .iter()
            .position(|entry| entry.source_path == second)
            .unwrap();
        let folder_index = prepared
            .entries
            .iter()
            .position(|entry| entry.source_path == folder)
            .unwrap();
        app.selected = Some(first_index);
        app.checked.insert(second_index);
        let CollectionRootDeleteResolution::Ready(target) =
            app.collection_root_delete_resolution(app.selected, true)
        else {
            panic!("installed root binding must resolve")
        };
        assert_eq!(
            target.entry_ids,
            vec![prepared.entries[second_index].entry_id]
        );
        assert_eq!(std::fs::read(&first).unwrap(), b"first");
        assert_eq!(std::fs::read(&second).unwrap(), b"second");

        app.top_level_grid_view
            .collection_session_mut()
            .unwrap()
            .installed_items_generation = None;
        assert!(matches!(
            app.collection_root_delete_resolution(Some(first_index), false),
            CollectionRootDeleteResolution::Unavailable(_)
        ));
        app.top_level_grid_view
            .collection_session_mut()
            .unwrap()
            .installed_items_generation = Some(app.items_generation);
        let owner = app
            .collection_grid_physical_load_owner(folder_index, &folder)
            .expect("folder owner");
        assert!(app.commit_collection_grid_physical_load(&owner, &folder));
        assert!(matches!(
            app.collection_root_delete_resolution(Some(folder_index), false),
            CollectionRootDeleteResolution::NotCollectionRoot
        ));
        app.shutdown_collection_runtime_for_exit();
    }

    #[test]
    fn navigation_materialization_preserves_and_remaps_local_search_filter() {
        let temp = tempfile::tempdir().unwrap();
        let first = temp.path().join("a.png");
        let second = temp.path().join("b.png");
        std::fs::write(&first, b"first").unwrap();
        std::fs::write(&second, b"second").unwrap();
        let (mut app, client) = start_ready_app(&temp.path().join("collection.db"));
        let snapshot = collection_with_sources(
            &client,
            &[
                (first, CollectionResolvedKind::Image),
                (second, CollectionResolvedKind::Image),
            ],
        );
        app.open_collection_grid(snapshot.collection_id(), None);
        wait_for_grid(&mut app, snapshot.collection_id());
        let previous = app
            .top_level_grid_view
            .collection_session()
            .and_then(CollectionGridSession::prepared)
            .cloned()
            .unwrap();
        let filtered_identity = previous.entries[0].entry_id;
        app.search_query = "a".into();
        app.search_filter_origin_folder = Some(temp.path().to_path_buf());
        app.search_filter = Some([0].into_iter().collect());
        app.rebuild_visible_indices();

        let mut entries = previous.entries.to_vec();
        entries.reverse();
        let reordered = Arc::new(CollectionPreparedSnapshot {
            collection_id: previous.collection_id,
            collection_revision: previous.collection_revision + 1,
            collection_name: previous.collection_name.clone(),
            order_mode: previous.order_mode,
            standard_sort: previous.standard_sort,
            entries: Arc::from(entries.into_boxed_slice()),
        });
        app.apply_collection_grid_prepared(Arc::clone(&reordered), Some(previous));

        assert_eq!(app.search_query, "a");
        assert_eq!(
            app.search_filter_origin_folder.as_deref(),
            Some(temp.path())
        );
        assert_eq!(
            app.search_filter,
            Some(
                [reordered
                    .entries
                    .iter()
                    .position(|entry| entry.entry_id == filtered_identity)
                    .unwrap()]
                .into_iter()
                .collect()
            )
        );
        app.shutdown_collection_runtime_for_exit();
    }

    #[test]
    fn prepared_grid_keeps_missing_and_restores_entry_by_source_when_entry_id_is_recreated() {
        let temp = tempfile::tempdir().unwrap();
        let image = temp.path().join("visible.png");
        let folder = temp.path().join("book");
        let missing = temp.path().join("missing.png");
        std::fs::write(&image, b"not decoded by collection preparation").unwrap();
        std::fs::create_dir(&folder).unwrap();
        let (mut app, client) = start_ready_app(&temp.path().join("collection.db"));
        let snapshot = collection_with_sources(
            &client,
            &[
                (image.clone(), CollectionResolvedKind::Image),
                (folder.clone(), CollectionResolvedKind::Folder),
                (missing.clone(), CollectionResolvedKind::Image),
            ],
        );

        app.open_collection_grid(snapshot.collection_id(), None);
        wait_for_grid(&mut app, snapshot.collection_id());
        let missing_index = app
            .items
            .iter()
            .position(|item| matches!(item, GridItem::CollectionPlaceholder { path, .. } if path == &missing))
            .expect("missing reference remains visible");
        let folder_index = app
            .items
            .iter()
            .position(|item| matches!(item, GridItem::Folder(path) if path == &folder))
            .expect("physical folder remains openable");
        assert_ne!(missing_index, folder_index);

        let anchor = app
            .collection_grid_source_anchor(folder_index, &folder)
            .expect("stable physical-source anchor");
        app.commit_collection_grid_source_open(anchor.clone(), folder.clone());
        let mut restore = match app.collection_grid_parent_nav().expect("collection parent") {
            crate::ui_main::AddressBarNav::Collection(restore) => restore,
            _ => panic!("physical source must return to its collection"),
        };
        restore
            .viewport_anchor
            .as_mut()
            .expect("source anchor")
            .entry_id = CollectionEntryId::new();
        app.open_collection_grid(snapshot.collection_id(), Some(restore));
        wait_for_grid(&mut app, snapshot.collection_id());

        let selected = app.selected.expect("opened entry selection restored");
        let prepared = app
            .top_level_grid_view
            .collection_session()
            .and_then(CollectionGridSession::prepared)
            .expect("prepared collection");
        assert_eq!(prepared.entries[selected].source_key, anchor.source_key);
        assert!(
            app.scroll_to_selected,
            "restored entry must be scrolled into view"
        );
        app.shutdown_collection_runtime_for_exit();
    }

    #[test]
    fn collection_grid_loading_empty_failure_and_deleted_are_terminal_presentations() {
        let temp = tempfile::tempdir().unwrap();
        let (mut app, client) = start_ready_app(&temp.path().join("collection.db"));
        let empty = recv(
            client
                .create_collection("Empty".into())
                .expect("create request"),
        );
        app.open_collection_grid(empty.collection_id(), None);
        assert_eq!(
            app.collection_grid_empty_message().as_deref(),
            Some("コレクションを読み込み中…")
        );
        wait_for_grid(&mut app, empty.collection_id());
        assert_eq!(
            app.collection_grid_empty_message().as_deref(),
            Some("コレクションに項目はありません")
        );

        app.shutdown_collection_runtime_for_exit();
        app.open_collection_grid(empty.collection_id(), None);
        let failure = app
            .collection_grid_empty_message()
            .expect("terminal unavailable message");
        assert_eq!(failure, "コレクションを利用できません");
        let generation = app.top_level_grid_view.generation();
        for _ in 0..4 {
            app.poll_collection_grid(&egui::Context::default());
            assert_eq!(app.top_level_grid_view.generation(), generation);
            assert!(matches!(
                app.top_level_grid_view.collection_session().unwrap().load,
                CollectionGridLoadState::Failed {
                    installed: None,
                    ..
                }
            ));
        }
    }

    #[test]
    fn revision_refresh_remaps_selected_and_checked_entry_identity() {
        let temp = tempfile::tempdir().unwrap();
        let first = temp.path().join("first.png");
        let second = temp.path().join("second.png");
        std::fs::write(&first, b"first").unwrap();
        std::fs::write(&second, b"second").unwrap();
        let (mut app, client) = start_ready_app(&temp.path().join("collection.db"));
        let snapshot = collection_with_sources(
            &client,
            &[
                (first.clone(), CollectionResolvedKind::Image),
                (second, CollectionResolvedKind::Image),
            ],
        );
        app.open_collection_grid(snapshot.collection_id(), None);
        wait_for_grid(&mut app, snapshot.collection_id());
        let selected_id = snapshot.entries[0].id;
        let selected_index = app
            .top_level_grid_view
            .collection_session()
            .and_then(CollectionGridSession::prepared)
            .unwrap()
            .entries
            .iter()
            .position(|entry| entry.entry_id == selected_id)
            .unwrap();
        app.selected = Some(selected_index);
        app.checked.insert(selected_index);

        let reordered = recv(
            client
                .reorder_manual(
                    snapshot.collection_id(),
                    snapshot.revision(),
                    snapshot
                        .entries
                        .iter()
                        .rev()
                        .map(|entry| entry.id)
                        .collect(),
                )
                .expect("reorder request"),
        );
        poll_until(&mut app, "collection reorder did not refresh", |app| {
            app.top_level_grid_view
                .collection_session()
                .is_some_and(|session| session.accepted_revision == reordered.revision())
        });
        let prepared = app
            .top_level_grid_view
            .collection_session()
            .and_then(CollectionGridSession::prepared)
            .unwrap();
        let remapped = prepared
            .entries
            .iter()
            .position(|entry| entry.entry_id == selected_id)
            .unwrap();
        assert_eq!(app.selected, Some(remapped));
        assert_eq!(app.checked, std::collections::HashSet::from([remapped]));
        assert!(app.scroll_to_selected);

        let removed = recv(
            client
                .remove_entries(
                    snapshot.collection_id(),
                    reordered.revision(),
                    vec![selected_id],
                )
                .expect("remove request"),
        );
        let readded = recv(
            client
                .add_batch(
                    snapshot.collection_id(),
                    removed.revision(),
                    vec![
                        CollectionRegistration::from_trusted_path(
                            &first,
                            CollectionResolvedKind::Image,
                        )
                        .unwrap(),
                    ],
                )
                .expect("re-add request"),
        )
        .snapshot;
        poll_until(&mut app, "re-added source did not refresh", |app| {
            app.top_level_grid_view
                .collection_session()
                .is_some_and(|session| session.accepted_revision == readded.revision())
        });
        let new_entry = readded
            .entries
            .iter()
            .find(|entry| entry.source_path == first)
            .unwrap();
        assert_ne!(new_entry.id, selected_id);
        assert!(
            app.checked.is_empty(),
            "checks never transfer to a new entry ID"
        );
        app.shutdown_collection_runtime_for_exit();
    }

    #[test]
    fn physical_child_defers_revision_install_until_typed_collection_return() {
        let temp = tempfile::tempdir().unwrap();
        let folder = temp.path().join("book");
        let child = folder.join("child.png");
        std::fs::create_dir(&folder).unwrap();
        std::fs::write(&child, b"child").unwrap();
        let (mut app, client) = start_ready_app(&temp.path().join("collection.db"));
        let snapshot =
            collection_with_sources(&client, &[(folder.clone(), CollectionResolvedKind::Folder)]);
        app.open_collection_grid(snapshot.collection_id(), None);
        wait_for_grid(&mut app, snapshot.collection_id());
        let anchor = app
            .collection_grid_source_anchor(0, &folder)
            .expect("collection source anchor");
        app.commit_collection_grid_source_open(anchor.clone(), folder.clone());
        app.items = vec![GridItem::Image(child.clone())];
        app.image_metas = vec![Some((0, 5))];
        app.items_generation = app.items_generation.wrapping_add(1);
        let child_generation = app.items_generation;

        let renamed = recv(
            client
                .rename_collection(
                    snapshot.collection_id(),
                    snapshot.revision(),
                    "Renamed while in child".into(),
                )
                .expect("rename request"),
        );
        poll_until(&mut app, "child did not observe latest revision", |app| {
            app.top_level_grid_view
                .collection_session()
                .is_some_and(|session| session.wanted_revision >= renamed.revision())
        });
        assert!(matches!(
            app.top_level_grid_view
                .collection_session()
                .unwrap()
                .position,
            CollectionGridPosition::PhysicalSource { .. }
        ));
        assert!(matches!(
            app.items.as_slice(),
            [GridItem::Image(path)] if path == &child
        ));
        assert_eq!(app.items_generation, child_generation);

        let restore = match app.collection_grid_parent_nav().expect("collection return") {
            crate::ui_main::AddressBarNav::Collection(restore) => restore,
            _ => panic!("physical source must return to collection"),
        };
        app.open_collection_grid(snapshot.collection_id(), Some(restore));
        poll_until(
            &mut app,
            "returned root did not reach latest revision",
            |app| {
                app.top_level_grid_view
                    .collection_session()
                    .is_some_and(|session| session.accepted_revision == renamed.revision())
            },
        );
        let selected = app.selected.expect("opened source restored");
        let prepared = app
            .top_level_grid_view
            .collection_session()
            .and_then(CollectionGridSession::prepared)
            .unwrap();
        assert_eq!(prepared.entries[selected].entry_id, anchor.entry_id);
        assert_eq!(app.address, "コレクション: Renamed while in child");
        app.shutdown_collection_runtime_for_exit();
    }

    #[test]
    fn independent_physical_load_retires_root_watch_before_later_collection_revision() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source.png");
        let physical = temp.path().join("physical");
        let added = temp.path().join("later.png");
        std::fs::write(&source, b"source").unwrap();
        std::fs::write(&added, b"later").unwrap();
        std::fs::create_dir(&physical).unwrap();
        std::fs::write(physical.join("visible.png"), b"visible").unwrap();
        let (mut app, client) = start_ready_app(&temp.path().join("collection.db"));
        let snapshot = collection_with_sources(&client, &[(source, CollectionResolvedKind::Image)]);
        app.open_collection_grid(snapshot.collection_id(), None);
        wait_for_grid(&mut app, snapshot.collection_id());

        let scan =
            super::super::folder_scan::scan_directory_with_settings(&physical, &app.settings)
                .unwrap();
        assert!(app.load_folder_with_scan_owned(
            physical.clone(),
            Some(scan),
            crate::app::OpenRequestOwner::Navigation,
        ));
        assert!(matches!(
            app.top_level_grid_view.surface(),
            TopLevelGridSurface::Folder
        ));
        assert!(app.top_level_grid_view.collection_session().is_none());
        let held_generation = app.items_generation;
        let held_address = app.address.clone();
        let held_items = app
            .items
            .iter()
            .map(|item| item.drag_source_path().map(Path::to_path_buf))
            .collect::<Vec<_>>();
        let held_selected = app.selected;
        let held_checked = app.checked.clone();
        let held_scroll = app.scroll_offset_y;

        let updated = recv(
            client
                .add_batch(
                    snapshot.collection_id(),
                    snapshot.revision(),
                    vec![
                        CollectionRegistration::from_trusted_path(
                            &added,
                            CollectionResolvedKind::Image,
                        )
                        .unwrap(),
                    ],
                )
                .expect("add request"),
        );
        for _ in 0..4 {
            app.poll_collection_grid(&egui::Context::default());
        }
        assert_eq!(updated.snapshot.revision(), snapshot.revision() + 1);
        assert_eq!(app.items_generation, held_generation);
        assert_eq!(app.address, held_address);
        assert_eq!(
            app.items
                .iter()
                .map(|item| item.drag_source_path().map(Path::to_path_buf))
                .collect::<Vec<_>>(),
            held_items
        );
        assert_eq!(app.selected, held_selected);
        assert_eq!(app.checked, held_checked);
        assert_eq!(app.scroll_offset_y, held_scroll);
        assert_eq!(app.current_folder.as_deref(), Some(physical.as_path()));
        app.shutdown_collection_runtime_for_exit();
    }

    #[test]
    fn failed_independent_scan_keeps_collection_root_surface_and_items_atomic() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source.png");
        std::fs::write(&source, b"source").unwrap();
        let (mut app, client) = start_ready_app(&temp.path().join("collection.db"));
        let snapshot = collection_with_sources(&client, &[(source, CollectionResolvedKind::Image)]);
        app.open_collection_grid(snapshot.collection_id(), None);
        wait_for_grid(&mut app, snapshot.collection_id());
        let held_generation = app.items_generation;
        let held_address = app.address.clone();
        let held_source = app.items[0].drag_source_path().unwrap().to_path_buf();

        assert!(!app.load_folder_with_scan_owned(
            temp.path().join("missing"),
            None,
            crate::app::OpenRequestOwner::Navigation,
        ));
        assert!(matches!(
            app.top_level_grid_view.surface(),
            TopLevelGridSurface::Collection(identity)
                if identity.collection_id == snapshot.collection_id()
        ));
        assert!(matches!(
            app.top_level_grid_view
                .collection_session()
                .unwrap()
                .position,
            CollectionGridPosition::Root
        ));
        assert_eq!(app.items_generation, held_generation);
        assert_eq!(app.address, held_address);
        assert_eq!(
            app.items[0].drag_source_path().map(Path::to_path_buf),
            Some(held_source)
        );
        assert!(app.current_folder.is_none());
        app.shutdown_collection_runtime_for_exit();
    }

    #[test]
    fn root_load_owner_stales_on_wanted_revision_but_physical_continuation_does_not() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("book");
        let nested = root.join("nested");
        let deeper = nested.join("deeper");
        let other = temp.path().join("other");
        std::fs::create_dir_all(&deeper).unwrap();
        std::fs::create_dir_all(&other).unwrap();
        std::fs::write(root.join("page.png"), b"page").unwrap();
        let root_same = root.join("same.jpg");
        let other_same = other.join("same.jpg");
        std::fs::write(&root_same, b"root same").unwrap();
        std::fs::write(&other_same, b"other same").unwrap();
        std::fs::write(nested.join("nested.png"), b"nested").unwrap();
        std::fs::write(deeper.join("deep.png"), b"deep").unwrap();
        let (mut app, client) = start_ready_app(&temp.path().join("collection.db"));
        // This test exercises two immediate accepted loads. A sidecar restore deliberately blocks
        // sibling loads until its continuation finishes, which is an orthogonal lifecycle.
        app.settings.sidecar_backup_enabled = false;
        app.settings.tag_sidecar_backup_enabled = false;
        let snapshot =
            collection_with_sources(&client, &[(root.clone(), CollectionResolvedKind::Folder)]);
        app.open_collection_grid(snapshot.collection_id(), None);
        wait_for_grid(&mut app, snapshot.collection_id());

        let stale_root = app
            .collection_grid_physical_load_owner(0, &root)
            .expect("root owner");
        app.top_level_grid_view
            .collection_session_mut()
            .unwrap()
            .wanted_revision += 1;
        assert!(!app.collection_grid_physical_load_owner_is_current(&stale_root, &root));
        let scan =
            super::super::folder_scan::scan_directory_with_settings(&root, &app.settings).unwrap();
        assert!(!app.load_folder_with_scan_owned(
            root.clone(),
            Some(scan),
            crate::app::OpenRequestOwner::CollectionGridPhysical(stale_root.clone()),
        ));
        assert!(matches!(
            app.top_level_grid_view
                .collection_session()
                .unwrap()
                .position,
            CollectionGridPosition::Root
        ));
        assert!(app.current_folder.is_none());
        app.top_level_grid_view
            .collection_session_mut()
            .unwrap()
            .wanted_revision = stale_root.wanted_revision;

        let scan =
            super::super::folder_scan::scan_directory_with_settings(&root, &app.settings).unwrap();
        assert!(app.load_folder_with_scan_owned(
            root.clone(),
            Some(scan),
            crate::app::OpenRequestOwner::CollectionGridPhysical(stale_root),
        ));
        assert!(matches!(
            app.top_level_grid_view
                .collection_session()
                .unwrap()
                .position,
            CollectionGridPosition::PhysicalSource { .. }
        ));

        // Collection-owned PhysicalSource keeps the Collection surface, so load requests and
        // delete_missing must both use full paths. Model two same-basename rows from different
        // parents in one catalog and verify pruning retains exactly the keys the readers request.
        assert!(app.use_full_path_cache_keys());
        let same_name_items = [
            GridItem::Image(root_same.clone()),
            GridItem::Image(other_same.clone()),
        ];
        let existing_keys = same_name_items
            .iter()
            .flat_map(|item| {
                super::super::folder_thumb_existing_keys_for(
                    item,
                    None,
                    &std::collections::HashMap::new(),
                    None,
                    Some(app.settings.folder_thumb_sort),
                    app.settings.folder_thumb_depth,
                    app.use_full_path_cache_keys(),
                )
            })
            .collect::<std::collections::HashSet<_>>();
        let requested_keys = same_name_items
            .iter()
            .enumerate()
            .map(|(idx, item)| {
                super::super::make_load_request(
                    item,
                    idx,
                    1,
                    1,
                    false,
                    None,
                    Some(app.settings.folder_thumb_sort),
                    app.settings.folder_thumb_depth,
                    &std::collections::HashMap::new(),
                    &std::collections::HashMap::new(),
                    None,
                    app.current_folder.as_deref(),
                    None,
                    None,
                    app.use_full_path_cache_keys(),
                )
                .unwrap()
                .cache_key_override
                .unwrap()
            })
            .collect::<std::collections::HashSet<_>>();
        assert_eq!(existing_keys, requested_keys);
        assert_eq!(existing_keys.len(), 2);
        let catalog = crate::catalog::CatalogDb::open(&temp.path().join("cache"), &root).unwrap();
        for key in &requested_keys {
            catalog.save(key, 1, 1, 1, 1, None, b"thumb").unwrap();
        }
        catalog.delete_missing(&existing_keys).unwrap();
        let retained = catalog.load_all().unwrap();
        assert_eq!(retained.len(), 2);
        assert!(requested_keys.iter().all(|key| retained.contains_key(key)));

        let renamed = recv(
            client
                .rename_collection(
                    snapshot.collection_id(),
                    snapshot.revision(),
                    "newer".into(),
                )
                .expect("rename request"),
        );
        poll_until(&mut app, "physical child did not observe revision", |app| {
            app.top_level_grid_view
                .collection_session()
                .is_some_and(|session| session.wanted_revision >= renamed.revision())
        });
        let nested_index = app
            .items
            .iter()
            .position(|item| matches!(item, GridItem::Folder(path) if path == &nested))
            .unwrap();
        let continuation = app
            .collection_grid_physical_load_owner(nested_index, &nested)
            .expect("physical continuation");
        assert!(app.collection_grid_physical_load_owner_is_current(&continuation, &nested));
        let scan = super::super::folder_scan::scan_directory_with_settings(&nested, &app.settings)
            .unwrap();
        assert!(app.load_folder_with_scan_owned(
            nested.clone(),
            Some(scan),
            crate::app::OpenRequestOwner::CollectionGridPhysical(continuation),
        ));
        assert_eq!(app.current_folder.as_deref(), Some(nested.as_path()));
        assert!(matches!(
            app.collection_grid_parent_nav(),
            Some(crate::ui_main::AddressBarNav::Collection(_))
        ));
        let reload_owner = app
            .collection_grid_physical_reload_owner(&nested)
            .expect("physical same-folder reload owner");
        let scan = super::super::folder_scan::scan_directory_with_settings(&nested, &app.settings)
            .unwrap();
        assert!(app.load_folder_with_scan_owned(
            nested.clone(),
            Some(scan),
            crate::app::OpenRequestOwner::CollectionGridPhysical(reload_owner),
        ));
        assert_eq!(app.current_folder.as_deref(), Some(nested.as_path()));

        let deleted = recv(
            client
                .delete_collection(snapshot.collection_id(), renamed.revision())
                .expect("delete request"),
        );
        poll_until(&mut app, "physical child did not observe deletion", |app| {
            app.top_level_grid_view
                .collection_session()
                .is_some_and(|session| {
                    session.observed_catalog_revision >= deleted.catalog_revision
                        && matches!(session.load, CollectionGridLoadState::Deleted)
                })
        });
        let deeper_index = app
            .items
            .iter()
            .position(|item| matches!(item, GridItem::Folder(path) if path == &deeper))
            .unwrap();
        let continuation = app
            .collection_grid_physical_load_owner(deeper_index, &deeper)
            .expect("deleted collection keeps its mounted physical continuation");
        let scan = super::super::folder_scan::scan_directory_with_settings(&deeper, &app.settings)
            .unwrap();
        assert!(app.load_folder_with_scan_owned(
            deeper.clone(),
            Some(scan),
            crate::app::OpenRequestOwner::CollectionGridPhysical(continuation),
        ));
        assert_eq!(app.current_folder.as_deref(), Some(deeper.as_path()));

        // Typing the same currently visible path in the address bar is an independent
        // navigation intent. Path equality must not preserve the collection owner.
        let scan = super::super::folder_scan::scan_directory_with_settings(&deeper, &app.settings)
            .unwrap();
        assert!(app.load_folder_with_scan_owned(
            deeper.clone(),
            Some(scan),
            crate::app::OpenRequestOwner::Navigation,
        ));
        assert!(matches!(
            app.top_level_grid_view.surface(),
            TopLevelGridSurface::Folder
        ));
        assert!(app.top_level_grid_view.collection_session().is_none());
        app.shutdown_collection_runtime_for_exit();
    }

    #[test]
    fn collection_video_workers_use_generation_owned_full_path_sidecars_and_cancel_on_refresh() {
        let temp = tempfile::tempdir().unwrap();
        let left = temp.path().join("left");
        let right = temp.path().join("right");
        let child_folder = temp.path().join("child-folder");
        let child_zip = temp.path().join("child.zip");
        let child_pdf = temp.path().join("child.pdf");
        std::fs::create_dir(&left).unwrap();
        std::fs::create_dir(&right).unwrap();
        std::fs::create_dir(&child_folder).unwrap();
        std::fs::write(&child_zip, b"zip fixture").unwrap();
        std::fs::write(&child_pdf, b"pdf fixture").unwrap();
        let left_video = left.join("same.mp4");
        let right_video = right.join("same.mp4");
        let left_image = left.join("same.jpg");
        let right_image = right.join("same.jpg");
        std::fs::write(&left_video, b"left video").unwrap();
        std::fs::write(&right_video, b"right video").unwrap();
        image::RgbImage::from_pixel(2, 2, image::Rgb([255, 0, 0]))
            .save(&left_image)
            .unwrap();
        image::RgbImage::from_pixel(2, 2, image::Rgb([0, 255, 0]))
            .save(&right_image)
            .unwrap();
        let (mut app, client) = start_ready_app(&temp.path().join("collection.db"));
        app.settings.skip_image_if_video_exists = true;
        app.settings.video_thumb_use_sidecar_image = true;
        let snapshot = collection_with_sources(
            &client,
            &[
                (left_video.clone(), CollectionResolvedKind::Video),
                (right_video.clone(), CollectionResolvedKind::Video),
                (child_folder.clone(), CollectionResolvedKind::Folder),
                (child_zip.clone(), CollectionResolvedKind::Zip),
                (child_pdf.clone(), CollectionResolvedKind::Pdf),
            ],
        );
        app.open_collection_grid(snapshot.collection_id(), None);
        wait_for_grid(&mut app, snapshot.collection_id());

        assert!(app.use_full_path_cache_keys());
        assert_eq!(
            app.video_thumb_overrides
                .get(&crate::path_key::normalize_keep_drive(&left_video)),
            Some(&left_image)
        );
        assert_eq!(
            app.video_thumb_overrides
                .get(&crate::path_key::normalize_keep_drive(&right_video)),
            Some(&right_image)
        );
        assert!(!app.video_thumb_overrides.contains_key("same"));
        assert!(app.reload_queue.is_some() && app.heavy_io_queue.is_some());
        assert!(app.current_color_catalog.is_none());
        let old_video_cancel = app
            .top_level_grid_view
            .collection_session()
            .unwrap()
            .video_worker_cancel
            .as_ref()
            .cloned()
            .expect("video worker cancel");
        let old_shared_pool_cancel = Arc::clone(&app.cancel_token);

        let renamed = recv(
            client
                .rename_collection(
                    snapshot.collection_id(),
                    snapshot.revision(),
                    "refreshed".into(),
                )
                .expect("rename request"),
        );
        poll_until(&mut app, "collection refresh did not install", |app| {
            app.top_level_grid_view
                .collection_session()
                .is_some_and(|session| session.accepted_revision == renamed.revision())
        });
        assert!(old_video_cancel.load(Ordering::Acquire));
        assert!(
            !app.top_level_grid_view
                .collection_session()
                .unwrap()
                .video_worker_cancel
                .as_ref()
                .unwrap()
                .load(Ordering::Acquire)
        );
        assert!(old_shared_pool_cancel.load(Ordering::Acquire));

        for child in [&child_folder, &child_zip, &child_pdf] {
            let index = app
                .items
                .iter()
                .position(|item| item.drag_source_path() == Some(child.as_path()))
                .unwrap();
            let owner = app
                .collection_grid_physical_load_owner(index, child)
                .expect("root physical owner");
            let root_video_cancel = app
                .top_level_grid_view
                .collection_session()
                .and_then(|session| session.video_worker_cancel.as_ref())
                .cloned()
                .expect("root video worker");
            let physical_shared_pool_cancel = Arc::clone(&app.cancel_token);
            assert!(app.commit_collection_grid_physical_load(&owner, child));
            assert!(root_video_cancel.load(Ordering::Acquire));
            assert!(
                app.top_level_grid_view
                    .collection_session()
                    .unwrap()
                    .video_worker_cancel
                    .is_none()
            );
            assert!(
                !physical_shared_pool_cancel.load(Ordering::Acquire),
                "root Folder/ZIP/PDF transition must leave the bundle pool alive"
            );
            app.open_collection_grid(snapshot.collection_id(), None);
            wait_for_grid(&mut app, snapshot.collection_id());
        }

        let exit_video_cancel = app
            .top_level_grid_view
            .collection_session()
            .and_then(|session| session.video_worker_cancel.as_ref())
            .cloned()
            .expect("reopened root video worker");
        let exit_shared_pool_cancel = Arc::clone(&app.cancel_token);
        app.top_level_grid_view
            .replace_surface(TopLevelGridSurface::Search(
                super::super::top_level_grid_view::TopLevelSearchView::Global,
            ));
        assert!(exit_video_cancel.load(Ordering::Acquire));
        assert!(
            !exit_shared_pool_cancel.load(Ordering::Acquire),
            "collection session drop must not cancel the bundle-owned image/container pool"
        );
        app.shutdown_collection_runtime_for_exit();
    }

    #[test]
    fn fullscreen_root_order_change_reports_viewer_deferred_until_close() {
        use crate::collection_store::CollectionOrderMode;
        use crate::settings::SortOrder;

        let temp = tempfile::tempdir().unwrap();
        let sources = ["first.mp4", "second.mp4"]
            .into_iter()
            .map(|name| {
                let path = temp.path().join(name);
                std::fs::write(&path, b"video").unwrap();
                (path, CollectionResolvedKind::Video)
            })
            .collect::<Vec<_>>();
        let (mut app, client) = start_ready_app(&temp.path().join("collection.db"));
        let snapshot = collection_with_sources(&client, &sources);
        app.open_collection_grid(snapshot.collection_id(), None);
        assert_eq!(
            app.grid_sort_lock_reason(),
            Some(super::super::GridSortLockReason::CollectionLoading)
        );
        wait_for_grid(&mut app, snapshot.collection_id());
        assert_eq!(app.grid_sort_lock_reason(), None);
        let original_generation = app.items_generation;
        app.fullscreen_idx = Some(0);

        let shuffled = recv(
            client
                .set_order(
                    snapshot.collection_id(),
                    snapshot.revision(),
                    CollectionOrderMode::Shuffle,
                    SortOrder::FileName,
                )
                .expect("set order request"),
        );
        poll_until(&mut app, "fullscreen missed Shuffle notice", |app| {
            app.top_level_grid_view
                .collection_session()
                .is_some_and(|session| session.wanted_revision == shuffled.revision())
        });
        assert!(matches!(
            app.top_level_grid_view.collection_session().unwrap().load,
            CollectionGridLoadState::RequestNeeded { installed: Some(_) }
        ));
        assert_eq!(app.items_generation, original_generation);
        let deferred = super::super::GridSortLockReason::CollectionViewerDeferred;
        assert_eq!(app.grid_sort_lock_reason(), Some(deferred));
        assert_eq!(deferred.short_label(), "反映待ち");
        assert!(
            deferred
                .tooltip()
                .contains("次の項目への移動が成功した場合")
        );

        // The label follows the current installed binding; a stale generation cannot claim
        // that the active viewer is what prevents an unrelated Grid refresh.
        app.top_level_grid_view
            .collection_session_mut()
            .unwrap()
            .installed_items_generation = Some(original_generation + 1);
        assert_eq!(
            app.grid_sort_lock_reason(),
            Some(super::super::GridSortLockReason::CollectionLoading)
        );
        app.top_level_grid_view
            .collection_session_mut()
            .unwrap()
            .installed_items_generation = Some(original_generation);

        app.fullscreen_idx = None;
        poll_until(
            &mut app,
            "Shuffle order did not install after closing viewer",
            |app| {
                matches!(
                    app.collection_grid_root_order(),
                    Some(Ok(root)) if root.mode == CollectionOrderMode::Shuffle
                        && root.content.expected_revision == shuffled.revision()
                )
            },
        );
        assert_eq!(app.grid_sort_lock_reason(), None);
        app.shutdown_collection_runtime_for_exit();
    }

    #[test]
    fn fullscreen_leaf_defers_refresh_and_delete_presentation_until_close() {
        let temp = tempfile::tempdir().unwrap();
        let image = temp.path().join("visible.png");
        std::fs::write(&image, b"image").unwrap();
        let (mut app, client) = start_ready_app(&temp.path().join("collection.db"));
        let snapshot = collection_with_sources(&client, &[(image, CollectionResolvedKind::Image)]);
        app.open_collection_grid(snapshot.collection_id(), None);
        wait_for_grid(&mut app, snapshot.collection_id());
        app.fullscreen_idx = Some(0);
        let held_source = app.items[0].drag_source_path().unwrap().to_path_buf();
        let held_generation = app.items_generation;

        let deleted = recv(
            client
                .delete_collection(snapshot.collection_id(), snapshot.revision())
                .expect("delete request"),
        );
        poll_until(
            &mut app,
            "fullscreen did not observe delete notice",
            |app| {
                app.top_level_grid_view
                    .collection_session()
                    .is_some_and(|session| {
                        session.observed_catalog_revision >= deleted.catalog_revision
                            && matches!(session.load, CollectionGridLoadState::Deleted)
                    })
            },
        );
        assert!(matches!(
            app.items.as_slice(),
            [item] if item.drag_source_path().is_some_and(|path| path == held_source)
        ));
        assert_eq!(app.items_generation, held_generation);
        assert_eq!(app.fullscreen_idx, Some(0));
        assert_eq!(
            app.grid_sort_lock_reason(),
            Some(super::super::GridSortLockReason::CollectionDeleted)
        );

        app.fullscreen_idx = None;
        app.poll_collection_grid(&egui::Context::default());
        assert!(app.items.is_empty());
        assert_eq!(
            app.collection_grid_empty_message().as_deref(),
            Some("コレクションは削除されました")
        );
        app.shutdown_collection_runtime_for_exit();
    }

    #[test]
    fn deleting_another_collection_does_not_retire_current_grid() {
        let temp = tempfile::tempdir().unwrap();
        let current_path = temp.path().join("current.png");
        let other_path = temp.path().join("other.png");
        std::fs::write(&current_path, b"current").unwrap();
        std::fs::write(&other_path, b"other").unwrap();
        let (mut app, client) = start_ready_app(&temp.path().join("collection.db"));
        let current =
            collection_with_sources(&client, &[(current_path, CollectionResolvedKind::Image)]);
        let other =
            collection_with_sources(&client, &[(other_path, CollectionResolvedKind::Image)]);
        app.open_collection_grid(current.collection_id(), None);
        wait_for_grid(&mut app, current.collection_id());
        let held_source = app.items[0].drag_source_path().unwrap().to_path_buf();
        let deleted = recv(
            client
                .delete_collection(other.collection_id(), other.revision())
                .expect("delete request"),
        );
        poll_until(&mut app, "catalog notice was not observed", |app| {
            app.top_level_grid_view
                .collection_session()
                .is_some_and(|session| {
                    session.observed_catalog_revision >= deleted.catalog_revision
                })
        });
        assert!(matches!(
            app.items.as_slice(),
            [item] if item.drag_source_path().is_some_and(|path| path == held_source)
        ));
        assert!(matches!(
            app.top_level_grid_view.collection_session().unwrap().load,
            CollectionGridLoadState::Ready(_)
        ));
        app.shutdown_collection_runtime_for_exit();
    }

    #[test]
    fn source_delete_during_prepare_cancels_and_keeps_installed_binding_for_reclassification() {
        let temp = tempfile::tempdir().unwrap();
        let image = temp.path().join("visible.png");
        std::fs::write(&image, b"image").unwrap();
        let (mut app, client) = start_ready_app(&temp.path().join("collection.db"));
        let snapshot =
            collection_with_sources(&client, &[(image.clone(), CollectionResolvedKind::Image)]);
        app.open_collection_grid(snapshot.collection_id(), None);
        wait_for_grid(&mut app, snapshot.collection_id());
        let installed = app
            .top_level_grid_view
            .collection_session()
            .and_then(|session| session.prepared().cloned())
            .unwrap();
        let cancel = Arc::new(AtomicBool::new(false));
        let (_tx, rx) = std::sync::mpsc::sync_channel(1);
        let stamp = app.collection_grid_stamp().unwrap();
        app.top_level_grid_view
            .collection_session_mut()
            .unwrap()
            .load = CollectionGridLoadState::Preparing {
            stamp,
            exact_revision: snapshot.revision(),
            installed: Some(Arc::clone(&installed)),
            cancel: Arc::clone(&cancel),
            receiver: rx,
        };

        app.invalidate_current_collection_grid_sources(&[
            crate::delete_worker::DeleteSourceScope::Exact(image),
        ]);
        assert!(cancel.load(Ordering::Acquire));
        let session = app.top_level_grid_view.collection_session().unwrap();
        assert!(matches!(
            session.load,
            CollectionGridLoadState::RequestNeeded { installed: Some(_) }
        ));
        assert_eq!(
            session.prepared().unwrap().collection_revision,
            snapshot.revision()
        );
        app.shutdown_collection_runtime_for_exit();
    }

    #[test]
    fn cloned_collection_session_reattaches_an_independent_revision_watch() {
        let temp = tempfile::tempdir().unwrap();
        let image = temp.path().join("visible.png");
        std::fs::write(&image, b"image").unwrap();
        let (mut app, client) = start_ready_app(&temp.path().join("collection.db"));
        let snapshot = collection_with_sources(&client, &[(image, CollectionResolvedKind::Image)]);
        app.open_collection_grid(snapshot.collection_id(), None);
        wait_for_grid(&mut app, snapshot.collection_id());
        app.fullscreen_idx = Some(0);
        let renamed = recv(
            client
                .rename_collection(
                    snapshot.collection_id(),
                    snapshot.revision(),
                    "Renamed while copied".into(),
                )
                .expect("rename request"),
        );
        let ctx = egui::Context::default();
        let deadline = Instant::now() + Duration::from_secs(5);
        while app
            .top_level_grid_view
            .collection_session()
            .is_none_or(|session| session.wanted_revision != renamed.revision())
        {
            assert!(Instant::now() < deadline, "source context missed revision");
            app.poll_collection_ui(&ctx);
            app.poll_collection_grid(&ctx);
            std::thread::sleep(Duration::from_millis(2));
        }
        assert_eq!(
            app.top_level_grid_view
                .collection_session()
                .unwrap()
                .accepted_revision,
            snapshot.revision(),
            "fullscreen source keeps its accepted binding"
        );

        app.top_level_grid_view = app.top_level_grid_view.clone();
        let copied = app.top_level_grid_view.collection_session().unwrap();
        assert!(
            copied.watch.is_none(),
            "a copied context owns no source receiver"
        );
        assert_eq!(copied.wanted_revision, renamed.revision());
        assert!(copied.observed_catalog_revision >= renamed.catalog_revision);
        app.fullscreen_idx = None;
        let deadline = Instant::now() + Duration::from_secs(5);
        while app
            .top_level_grid_view
            .collection_session()
            .is_none_or(|session| session.accepted_revision != renamed.revision())
        {
            assert!(Instant::now() < deadline, "copied context stayed stale");
            app.poll_collection_ui(&ctx);
            app.poll_collection_grid(&ctx);
            std::thread::sleep(Duration::from_millis(2));
        }
        let session = app
            .top_level_grid_view
            .collection_session()
            .expect("collection session");
        assert_eq!(session.accepted_revision, renamed.revision());
        assert_eq!(app.address, "コレクション: Renamed while copied");
        assert!(
            session.watch.is_some(),
            "copied context must own a new watch"
        );
        app.shutdown_collection_runtime_for_exit();
    }

    #[test]
    fn stamped_remove_and_delete_invalidation_stay_with_the_owning_collection_surface() {
        let temp = tempfile::tempdir().unwrap();
        let folder = temp.path().join("folder");
        let child = folder.join("child.png");
        std::fs::create_dir(&folder).unwrap();
        std::fs::write(&child, b"child").unwrap();
        let (mut app, client) = start_ready_app(&temp.path().join("collection.db"));
        let snapshot =
            collection_with_sources(&client, &[(child.clone(), CollectionResolvedKind::Image)]);
        app.open_collection_grid(snapshot.collection_id(), None);
        wait_for_grid(&mut app, snapshot.collection_id());
        app.selected = Some(0);
        app.checked.insert(0);
        let target = app
            .collection_grid_remove_target(Some(0), true)
            .expect("context-owned remove target");
        assert_eq!(target.entry_ids.len(), 1);

        // A new surface generation must not let an old successful response clear its selection.
        app.open_collection_grid(snapshot.collection_id(), None);
        app.selected = Some(0);
        app.checked.insert(0);
        app.apply_collection_grid_remove_success(target.stamp, &target.entry_ids);
        assert_eq!(app.selected, Some(0));
        assert!(app.checked.contains(&0));
        wait_for_grid(&mut app, snapshot.collection_id());

        app.invalidate_current_collection_grid_sources(&[
            crate::delete_worker::DeleteSourceScope::Exact(temp.path().join("other.png")),
        ]);
        assert!(
            app.top_level_grid_view
                .collection_session()
                .is_some_and(|session| session.prepared().is_some()),
            "unrelated exact delete must not invalidate the collection"
        );
        app.invalidate_current_collection_grid_sources(&[
            crate::delete_worker::DeleteSourceScope::Tree(folder.clone()),
        ]);
        assert!(
            app.top_level_grid_view
                .collection_session()
                .is_some_and(|session| matches!(
                    session.load,
                    CollectionGridLoadState::RequestNeeded { .. }
                )),
            "tree delete must schedule fresh classification without post-delete stat"
        );
        app.shutdown_collection_runtime_for_exit();
    }

    #[test]
    #[cfg(windows)]
    fn remove_completion_routes_to_its_parked_viewer_context_only() {
        let temp = tempfile::tempdir().unwrap();
        let image = temp.path().join("shared.png");
        std::fs::write(&image, b"image").unwrap();
        let (mut app, client) = start_ready_app(&temp.path().join("collection.db"));
        let snapshot =
            collection_with_sources(&client, &[(image.clone(), CollectionResolvedKind::Image)]);
        app.open_collection_grid(snapshot.collection_id(), None);
        wait_for_grid(&mut app, snapshot.collection_id());
        app.selected = Some(0);
        app.checked.insert(0);

        let prepared = app
            .top_level_grid_view
            .collection_session()
            .and_then(|session| session.prepared().cloned())
            .unwrap();
        let items = app.items.clone();
        let image_metas = app.image_metas.clone();
        let identity = CollectionGridIdentity {
            collection_id: snapshot.collection_id(),
        };
        let parked = app.build_window_context_for_test(707, move |context| {
            context
                .top_level_grid_view
                .begin(TopLevelGridSurface::Collection(identity), None);
            context.items = items;
            context.image_metas = image_metas;
            context.selected = Some(0);
            context.checked.insert(0);
            let generation = context.items_generation;
            let session = context
                .top_level_grid_view
                .collection_session_mut()
                .unwrap();
            session.accepted_revision = prepared.collection_revision;
            session.wanted_revision = prepared.collection_revision;
            session.load = CollectionGridLoadState::Ready(
                CollectionGridInstalledPresentation::without_thumbnail_sources(prepared),
            );
            session.installed_items_generation = Some(generation);
        });
        let target = app
            .with_viewer_context(parked, |context| {
                context.collection_grid_remove_target(Some(0), true)
            })
            .unwrap()
            .expect("parked context target");

        app.apply_collection_grid_remove_success(target.stamp, &target.entry_ids);
        assert_eq!(
            app.selected,
            Some(0),
            "main selection belongs to another context"
        );
        assert!(app.checked.contains(&0));
        app.with_viewer_context(parked, |context| {
            assert_eq!(context.selected, None);
            assert!(context.checked.is_empty());
        })
        .unwrap();
        app.shutdown_collection_runtime_for_exit();
    }

    #[test]
    #[cfg(windows)]
    fn source_invalidation_reaches_matching_parked_context_and_skips_sibling_collection() {
        let temp = tempfile::tempdir().unwrap();
        let parked_image = temp.path().join("parked.png");
        let sibling_image = temp.path().join("sibling.png");
        std::fs::write(&parked_image, b"parked").unwrap();
        std::fs::write(&sibling_image, b"sibling").unwrap();
        let (mut app, client) = start_ready_app(&temp.path().join("collection.db"));
        let parked_snapshot = collection_with_sources(
            &client,
            &[(parked_image.clone(), CollectionResolvedKind::Image)],
        );
        app.open_collection_grid(parked_snapshot.collection_id(), None);
        wait_for_grid(&mut app, parked_snapshot.collection_id());
        let parked_prepared = app
            .top_level_grid_view
            .collection_session()
            .and_then(|session| session.prepared().cloned())
            .unwrap();
        let parked_items = app.items.clone();
        let parked_metas = app.image_metas.clone();

        let sibling_snapshot =
            collection_with_sources(&client, &[(sibling_image, CollectionResolvedKind::Image)]);
        app.open_collection_grid(sibling_snapshot.collection_id(), None);
        wait_for_grid(&mut app, sibling_snapshot.collection_id());

        let identity = CollectionGridIdentity {
            collection_id: parked_snapshot.collection_id(),
        };
        let parked = app.build_window_context_for_test(708, move |context| {
            context
                .top_level_grid_view
                .begin(TopLevelGridSurface::Collection(identity), None);
            context.items = parked_items;
            context.image_metas = parked_metas;
            let generation = context.items_generation;
            let session = context
                .top_level_grid_view
                .collection_session_mut()
                .unwrap();
            session.accepted_revision = parked_prepared.collection_revision;
            session.wanted_revision = parked_prepared.collection_revision;
            session.load = CollectionGridLoadState::Ready(
                CollectionGridInstalledPresentation::without_thumbnail_sources(parked_prepared),
            );
            session.installed_items_generation = Some(generation);
        });

        app.invalidate_collection_grid_sources(&[crate::delete_worker::DeleteSourceScope::Exact(
            parked_image,
        )]);
        assert!(
            app.top_level_grid_view
                .collection_session()
                .is_some_and(|session| session.prepared().is_some()),
            "unrelated mounted collection must keep its prepared binding"
        );
        app.with_viewer_context(parked, |context| {
            assert!(
                context
                    .top_level_grid_view
                    .collection_session()
                    .is_some_and(|session| matches!(
                        session.load,
                        CollectionGridLoadState::RequestNeeded { .. }
                    )),
                "matching parked collection must request reclassification"
            );
        })
        .unwrap();
        app.shutdown_collection_runtime_for_exit();
    }

    #[test]
    fn source_scope_exact_and_tree_use_component_boundaries() {
        let exact = crate::delete_worker::DeleteSourceScope::Exact(PathBuf::from(r"C:\A\B"));
        assert!(source_scope_contains(&exact, Path::new(r"c:/a/b")));
        assert!(!source_scope_contains(&exact, Path::new(r"c:/a/b/c.png")));
        let tree = crate::delete_worker::DeleteSourceScope::Tree(PathBuf::from(r"C:\A\B"));
        assert!(source_scope_contains(&tree, Path::new(r"c:/a/b/c.png")));
        assert!(!source_scope_contains(&tree, Path::new(r"c:/a/b2/c.png")));
    }

    #[test]
    fn durable_source_migration_is_retired_only_after_actor_ack() {
        let temp = tempfile::tempdir().unwrap();
        let old = temp.path().join("before.png");
        let new = temp.path().join("after.png");
        let (mut app, client) = start_ready_app(&temp.path().join("collection.db"));
        let snapshot =
            collection_with_sources(&client, &[(old.clone(), CollectionResolvedKind::Image)]);
        app.rename_migration_data_dir_override = Some(temp.path().to_path_buf());

        app.enqueue_collection_source_migration_batch(vec![
            crate::rename_key_migration::PathMigrationMapping {
                old_path: old,
                new_path: new.clone(),
                tree: false,
            },
        ]);
        app.flush_rename_migration_journal().unwrap();
        assert_eq!(
            crate::rename_key_migration::journal_load(temp.path()).len(),
            1,
            "admitted actor command remains durable until its result is consumed"
        );

        let ctx = egui::Context::default();
        let deadline = Instant::now() + Duration::from_secs(5);
        while app.rename_migration_in_flight.is_some() || !app.rename_migration_queue.is_empty() {
            assert!(
                Instant::now() < deadline,
                "collection migration did not settle"
            );
            app.poll_rename_migration_pending(&ctx);
            std::thread::sleep(Duration::from_millis(2));
        }
        app.flush_rename_migration_journal().unwrap();
        assert!(app.rename_migration_boot_retry.is_empty());
        assert!(
            crate::rename_key_migration::journal_load(temp.path()).is_empty(),
            "exact actor acknowledgement retires the durable stage"
        );
        let migrated = recv(
            client
                .load_collection(snapshot.collection_id())
                .expect("load migrated collection"),
        );
        assert_eq!(migrated.entries[0].source_path, new);
        assert_eq!(migrated.entries[0].id, snapshot.entries[0].id);
        app.shutdown_collection_runtime_for_exit();
    }
}
