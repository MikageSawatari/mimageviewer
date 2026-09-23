//! Context-owned collection Grid lifecycle.
//!
//! The collection actor supplies an immutable logical snapshot. Filesystem classification runs on
//! a separate worker, and only the exact viewer-context/surface/revision owner may install it.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use super::top_level_grid_view::{
    CollectionGridIdentity, CollectionGridInstalledPresentation, CollectionGridLoadState,
    CollectionGridPhysicalLoadOrigin, CollectionGridPhysicalLoadOwner, CollectionGridPosition,
    CollectionGridPrepareReuseKey, CollectionGridPreparedInstall,
    CollectionGridPreparedThumbnailDelivery, CollectionGridPreparedThumbnailSources,
    CollectionGridPresentationSources, CollectionGridRequestStamp, CollectionGridRestore,
    CollectionGridSession, CollectionGridSourceOpenOwner, CollectionGridThumbnailSourceIdentity,
    CollectionGridThumbnailSources, CollectionGridViewportAnchor, TopLevelGridRestore,
    TopLevelGridSurface,
};
use super::{App, GridItem, GridSortLockReason, ViewerContextId};
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

/// コレクション順の変更要求。要求時点の表示 owner と revision を mode / sort と一体で持つ。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct CollectionGridSetOrderIntent {
    pub(crate) content: CollectionGridContentTarget,
    pub(crate) mode: CollectionOrderMode,
    pub(crate) sort: crate::settings::SortOrder,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CollectionGridSetOrderResolution {
    NoOp,
    LocalHeaderReset,
    Mutation(CollectionGridSetOrderIntent),
}

/// 上部 Collection メニューだけが、列ヘッダ所有中の同値 Standard を解除要求にできる。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CollectionGridSetOrderRoute {
    StandardControl,
    CollectionMenu,
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
) -> Result<CollectionGridPreparedThumbnailDelivery, CollectionPrepareError> {
    use sha2::Digest as _;

    if sources.video_sidecars.is_empty() && sources.video_pin_blobs.is_empty() {
        return Ok(CollectionGridPreparedThumbnailDelivery::default());
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

    let identity = CollectionGridThumbnailSourceIdentity(digest.finalize().into());
    let presentation = if super::top_level_grid_view::collection_pin_blob_sizes_fit_retention_budget(
        sources.video_pin_blobs.values().map(Vec::len),
    ) {
        CollectionGridPresentationSources::Retained(Arc::new(
            CollectionGridPreparedThumbnailSources {
                identity,
                payload: sources.clone(),
            },
        ))
    } else {
        CollectionGridPresentationSources::Oversized(identity)
    };
    Ok(CollectionGridPreparedThumbnailDelivery {
        presentation,
        live: sources,
    })
}

pub(in crate::app) fn prepare_collection_grid_install(
    snapshot: &crate::collection_store::CollectionSnapshot,
    display_order: &crate::settings::GridDisplayOrder,
    settings: &crate::settings::Settings,
    cancel: &AtomicBool,
    auto_aspect_client: Option<&crate::auto_aspect_cache::CollectionAutoAspectCacheClient>,
    pin_stamp: Option<crate::video_pins::VideoPinMutationStamp>,
    thumbnail_source_epoch: u64,
    page_edit_availability: super::page_edit_snapshot::PageEditAvailability,
    page_edit_revision: u64,
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
    let edit_items = prepared
        .entries
        .iter()
        .map(|entry| entry.item.clone())
        .collect::<Vec<_>>();
    let page_edits = super::page_edit_snapshot::PageEditSnapshot::load_and_project(
        &edit_items,
        page_edit_availability,
        cancel,
    )
    .map_err(CollectionPrepareError::Io)?
    .ok_or(CollectionPrepareError::Cancelled)?;
    let retained_page_edits = Some(Arc::new(page_edits.0.clone()));
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
            video_pin_blobs: Arc::new(video_pin_blobs),
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
    let auto_aspect_lookup = auto_aspect_client.and_then(|client| {
        client.get_bounded(
            snapshot.collection_id(),
            cancel,
            std::time::Duration::from_millis(100),
        )
    });
    if cancel.load(Ordering::Acquire) {
        return Err(CollectionPrepareError::Cancelled);
    }
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
        page_edits: Some(page_edits),
        retained_page_edits,
        page_edit_revision,
        thumbnail_sources,
        auto_aspect_lookup,
        reuse_key: CollectionGridPrepareReuseKey::new(
            snapshot.collection_id(),
            snapshot.revision(),
            settings,
            pin_stamp,
            thumbnail_source_epoch,
        ),
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
        self.projected_viewer_context_id()
    }

    pub(crate) fn show_collection_jump_feedback_in_origin(
        &mut self,
        origin: &CollectionGridPhysicalLoadOwner,
        message: String,
    ) {
        let origin_context = origin.stamp.context_id;
        if self.collection_grid_context_id() == origin_context {
            self.show_feedback_toast(message);
            return;
        }
        if self
            .with_viewer_context(origin_context, |app| app.show_feedback_toast(message))
            .is_err()
        {
            crate::logger::log(format!(
                "collection physical jump feedback dropped because origin context is gone context_id={}",
                origin_context.serial()
            ));
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
    ) -> Option<Result<CollectionGridOrderTarget, GridSortLockReason>> {
        let session = self.top_level_grid_view.collection_session()?;
        if !matches!(session.position, CollectionGridPosition::Root) {
            return None;
        }
        match session.load {
            CollectionGridLoadState::Deleted { .. } => {
                return Some(Err(GridSortLockReason::CollectionDeleted));
            }
            CollectionGridLoadState::Failed { .. } => {
                return Some(Err(GridSortLockReason::CollectionFailed));
            }
            _ => {}
        }
        if self.collection_grid_refresh_waits_for_viewer() {
            return Some(Err(GridSortLockReason::CollectionViewerDeferred));
        }
        if !matches!(
            session.load,
            CollectionGridLoadState::Ready(_) | CollectionGridLoadState::Empty(_)
        ) {
            return Some(Err(GridSortLockReason::CollectionLoading));
        }
        if session.wanted_revision > session.accepted_revision {
            return Some(Err(GridSortLockReason::CollectionStale));
        }
        let content = match self.collection_grid_content_target() {
            Ok(target) => target,
            Err(_) => return Some(Err(GridSortLockReason::CollectionStale)),
        };
        let Some(prepared) = session.prepared() else {
            return Some(Err(GridSortLockReason::CollectionStale));
        };
        if prepared.collection_id != content.stamp.collection_id {
            return Some(Err(GridSortLockReason::CollectionStale));
        }
        Some(Ok(CollectionGridOrderTarget {
            content,
            mode: prepared.order_mode,
            standard_sort: prepared.standard_sort,
        }))
    }

    /// すべての UI / ring のコレクション順要求を同じ no-op・lock 規則へ通す。
    pub(crate) fn request_collection_grid_set_order(
        &mut self,
        captured: CollectionGridOrderTarget,
        mode: CollectionOrderMode,
        sort: crate::settings::SortOrder,
        route: CollectionGridSetOrderRoute,
    ) -> bool {
        match self.resolve_collection_grid_set_order(captured, mode, sort, route) {
            CollectionGridSetOrderResolution::NoOp => false,
            CollectionGridSetOrderResolution::LocalHeaderReset => {
                self.reset_details_sort_to_toolbar();
                true
            }
            CollectionGridSetOrderResolution::Mutation(intent) => {
                self.start_collection_grid_content_action(
                    intent.content,
                    crate::ui_dialogs::collections::CollectionGridSnapshotAction::SetOrder(intent),
                );
                true
            }
        }
    }

    fn resolve_collection_grid_set_order(
        &self,
        captured: CollectionGridOrderTarget,
        mode: CollectionOrderMode,
        sort: crate::settings::SortOrder,
        route: CollectionGridSetOrderRoute,
    ) -> CollectionGridSetOrderResolution {
        let Some(Ok(current)) = self.collection_grid_root_order() else {
            return CollectionGridSetOrderResolution::NoOp;
        };
        if current.content.stamp != captured.content.stamp
            || current.content.expected_revision != captured.content.expected_revision
        {
            return CollectionGridSetOrderResolution::NoOp;
        }
        if matches!(route, CollectionGridSetOrderRoute::StandardControl)
            && self.grid_sort_lock_reason().is_some()
        {
            return CollectionGridSetOrderResolution::NoOp;
        }

        let same_order = current.mode == mode
            && (mode != CollectionOrderMode::Standard || current.standard_sort == sort);
        if same_order
            && mode == CollectionOrderMode::Standard
            && matches!(route, CollectionGridSetOrderRoute::CollectionMenu)
            && self.details_header_sort_active()
        {
            return CollectionGridSetOrderResolution::LocalHeaderReset;
        }
        if same_order && mode != CollectionOrderMode::Shuffle {
            return CollectionGridSetOrderResolution::NoOp;
        }

        CollectionGridSetOrderResolution::Mutation(CollectionGridSetOrderIntent {
            content: captured.content,
            mode,
            sort,
        })
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

    fn invalidate_current_collection_thumbnail_presentation(
        &mut self,
    ) -> Option<Arc<CollectionGridInstalledPresentation>> {
        self.top_level_grid_view
            .collection_session_mut()?
            .invalidate_thumbnail_presentation()
    }

    /// A metadata import writes through its own attached SQLite connection, so
    /// the UI-owned `VideoPinDb` mutation stamp cannot observe the commit.  Move
    /// every context to a presentation-only retry and bind future preparation to
    /// this app-global epoch.  Item bindings, root/child position and navigation
    /// ownership stay intact.
    pub(crate) fn advance_collection_thumbnail_source_epoch_for_metadata_import(
        &mut self,
        committed_video_pin_changes: usize,
    ) {
        if committed_video_pin_changes == 0 {
            return;
        }
        self.collection_thumbnail_source_epoch =
            self.collection_thumbnail_source_epoch.wrapping_add(1);
        let mut retired = Vec::<super::smart_folder::RetiredSmartFolderPayload>::new();
        let mut invalidated_contexts = 0usize;
        #[cfg(windows)]
        {
            for id in self.viewer_context_ids() {
                if let Ok(Some(presentation)) = self.with_viewer_context(id, |app| {
                    app.invalidate_current_collection_thumbnail_presentation()
                }) {
                    invalidated_contexts = invalidated_contexts.saturating_add(1);
                    retired.push(Box::new(presentation));
                }
            }
        }
        #[cfg(not(windows))]
        if let Some(presentation) = self.invalidate_current_collection_thumbnail_presentation() {
            invalidated_contexts = 1;
            retired.push(Box::new(presentation));
        }
        self.retire_smart_folder_payloads(retired);
        crate::logger::log(format!(
            "metadata import: collection thumbnail sources invalidated changes={} epoch={} contexts={invalidated_contexts}",
            committed_video_pin_changes, self.collection_thumbnail_source_epoch,
        ));
        if crate::perf::is_enabled() {
            crate::perf::event(
                "metadata_import",
                "collection_thumbnail_sources_invalidated",
                None,
                0,
                &[
                    (
                        "changes",
                        serde_json::Value::from(committed_video_pin_changes as u64),
                    ),
                    (
                        "epoch",
                        serde_json::Value::from(self.collection_thumbnail_source_epoch),
                    ),
                    (
                        "contexts",
                        serde_json::Value::from(invalidated_contexts as u64),
                    ),
                ],
            );
        }
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
        if let Some(prepared) = owner.navigation_prepared.clone() {
            let previous_edit_snapshot = self
                .top_level_grid_view
                .collection_session()
                .and_then(CollectionGridSession::installed_presentation)
                .filter(|presentation| {
                    Arc::ptr_eq(&presentation.prepared, &prepared)
                        && presentation.page_edit_revision == self.page_edit_revision
                })
                .and_then(|presentation| presentation.page_edit_snapshot.clone());
            let presentation = owner
                .navigation_request
                .as_ref()
                .and_then(|request| request.root_thumbnail_sources.as_ref())
                .map(|source_owner| {
                    let mut presentation = CollectionGridInstalledPresentation::new(
                        Arc::clone(&prepared),
                        source_owner.presentation.clone(),
                        source_owner.reuse_key.clone(),
                    );
                    presentation.page_edit_snapshot = source_owner
                        .retained_edit_snapshot
                        .clone()
                        .filter(|_| source_owner.page_edit_revision == self.page_edit_revision)
                        .or_else(|| previous_edit_snapshot.clone());
                    presentation.page_edit_revision = self.page_edit_revision;
                    Arc::new(presentation)
                })
                .unwrap_or_else(|| {
                    let mut presentation = CollectionGridInstalledPresentation::new(
                        prepared,
                        CollectionGridPresentationSources::Oversized(
                            CollectionGridThumbnailSourceIdentity::default(),
                        ),
                        self.collection_grid_prepare_reuse_key(
                            owner.stamp.collection_id,
                            owner.accepted_revision,
                        ),
                    );
                    presentation.page_edit_snapshot = previous_edit_snapshot;
                    presentation.page_edit_revision = self.page_edit_revision;
                    Arc::new(presentation)
                });
            if let Some(session) = self.top_level_grid_view.collection_session_mut() {
                session.accepted_revision = presentation.prepared.collection_revision;
                session.wanted_revision = session
                    .wanted_revision
                    .max(presentation.prepared.collection_revision);
                session.load = if presentation.prepared.entries.is_empty() {
                    CollectionGridLoadState::Empty(presentation)
                } else {
                    CollectionGridLoadState::Ready(presentation)
                };
                session.installed_items_generation = None;
            }
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
            .collection_store_client_for_read()
            .ok()
            .flatten()
            .and_then(|client| client.subscribe().ok());
        if let Some(session) = self.top_level_grid_view.collection_session_mut() {
            session.wanted_revision = restore.as_ref().map_or(0, |state| state.revision_at_open);
            session.restore_anchor = restore.and_then(|state| state.viewport_anchor);
            session.watch = watch;
        }

        // Do not leave a prior physical/search grid interactive while the actor and classifier are
        // resolving this collection. The empty install performs no filesystem/database access.
        let collection_seed = self
            .collection_auto_aspect_cache
            .as_ref()
            .and_then(|cache| cache.cached(collection_id));
        self.install_collection_grid_items(Vec::new(), Vec::new(), None, collection_seed);
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
        let CollectionGridLoadState::RequestNeeded { .. } = &session.load else {
            return;
        };
        let now = Instant::now();
        let minimum_revision = session.wanted_revision.max(session.accepted_revision);
        let installed = session.load.installed().cloned();
        let deferred = if let Some(session) = self.top_level_grid_view.collection_session_mut()
            && let CollectionGridLoadState::RequestNeeded { lease, .. } = &mut session.load
        {
            lease.bind_viewer(stamp.context_id.serial(), stamp.surface_generation);
            lease.activate(now, "admission");
            !lease.is_due(now)
        } else {
            true
        };
        if deferred {
            return;
        }
        let client = match self.collection_store_client_for_read() {
            Ok(Some(client)) => client,
            Err(error) if error.is_read_retryable() => {
                if let Some(session) = self.top_level_grid_view.collection_session_mut()
                    && let CollectionGridLoadState::RequestNeeded { lease, .. } = &mut session.load
                {
                    lease.defer(now, "admission");
                }
                return;
            }
            Ok(None) | Err(_) => {
                if let Some(session) = self.top_level_grid_view.collection_session_mut() {
                    if let CollectionGridLoadState::RequestNeeded { lease, .. } = &mut session.load
                    {
                        lease.finish(now, "unavailable");
                    }
                    session.load = CollectionGridLoadState::Failed {
                        message: "コレクションを利用できません".into(),
                        installed,
                    };
                }
                return;
            }
        };
        let queued_at = crate::perf::is_enabled().then(Instant::now);
        match client.load_collection(stamp.collection_id) {
            Ok(receiver) => {
                if let Some(session) = self.top_level_grid_view.collection_session_mut() {
                    let previous = std::mem::replace(
                        &mut session.load,
                        CollectionGridLoadState::Deleted { installed: None },
                    );
                    let CollectionGridLoadState::RequestNeeded {
                        installed,
                        mut lease,
                    } = previous
                    else {
                        session.load = previous;
                        return;
                    };
                    lease.phase_progress(now, "snapshot");
                    session.load = CollectionGridLoadState::Snapshot {
                        stamp,
                        minimum_revision,
                        lease,
                        queued_at,
                        installed,
                        receiver,
                    };
                }
            }
            Err(error) if error.is_read_retryable() => {
                if let Some(session) = self.top_level_grid_view.collection_session_mut()
                    && let CollectionGridLoadState::RequestNeeded { lease, .. } = &mut session.load
                {
                    lease.defer(now, "admission");
                }
            }
            Err(error) => {
                if let Some(session) = self.top_level_grid_view.collection_session_mut() {
                    if let CollectionGridLoadState::RequestNeeded { lease, .. } = &mut session.load
                    {
                        lease.finish(now, "error");
                    }
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
        mut lease: crate::collection_store::CollectionReadLease,
    ) {
        let exact_revision = snapshot.revision();
        let display_order = self.settings.grid_display_order.clone();
        let settings = self.settings.clone();
        let pin_stamp = self.video_pin_db.as_ref().map(|db| db.mutation_stamp());
        let thumbnail_source_epoch = self.collection_thumbnail_source_epoch;
        let page_edit_availability = super::page_edit_snapshot::PageEditAvailability::for_app(self);
        let page_edit_revision = self.page_edit_revision;
        let auto_aspect_client = self
            .collection_auto_aspect_cache
            .as_ref()
            .map(|cache| cache.client());
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
                    auto_aspect_client.as_ref(),
                    pin_stamp,
                    thumbnail_source_epoch,
                    page_edit_availability,
                    page_edit_revision,
                );
                let _ = sender.send(result);
            });
        match spawn {
            Ok(_) => {
                lease.phase_progress(Instant::now(), "prepare");
                if let Some(session) = self.top_level_grid_view.collection_session_mut() {
                    session.load = CollectionGridLoadState::Preparing {
                        stamp,
                        exact_revision,
                        lease,
                        installed,
                        cancel,
                        receiver,
                    };
                }
            }
            Err(error) => {
                lease.finish(Instant::now(), "worker_spawn_error");
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

    /// Delayed polling only for the mounted root. A parked child/fullscreen leaf may keep a
    /// request owner, but it must not keep the UI awake while its presentation is protected.
    pub(crate) fn collection_grid_poll_delay(&self) -> Option<Duration> {
        if !self.collection_grid_root_materialize_active() || self.collection_grid_stamp().is_none()
        {
            return None;
        }
        let session = self.top_level_grid_view.collection_session()?;
        match &session.load {
            CollectionGridLoadState::RequestNeeded { lease, .. } => {
                lease.poll_delay(Instant::now())
            }
            CollectionGridLoadState::Snapshot { lease, .. }
            | CollectionGridLoadState::Preparing { lease, .. } => lease.completion_poll_delay(),
            _ => None,
        }
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
            CollectionGridLoadState::RequestNeeded {
                installed: None, ..
            }
            | CollectionGridLoadState::Snapshot { .. }
            | CollectionGridLoadState::Preparing { .. } => Some("コレクションを読み込み中…".into()),
            CollectionGridLoadState::RequestNeeded {
                installed: Some(_), ..
            } => None,
            CollectionGridLoadState::Empty(_) => Some("コレクションに項目はありません".into()),
            CollectionGridLoadState::Failed {
                message,
                installed: None,
            } => Some(message.clone()),
            CollectionGridLoadState::Failed {
                installed: Some(_), ..
            } => None,
            CollectionGridLoadState::Deleted { .. } => Some("コレクションは削除されました".into()),
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
                .collection_store_client_for_read()
                .ok()
                .flatten()
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
                let installed = session.prepared().cloned();
                session.load = CollectionGridLoadState::Deleted { installed };
                deleted_current_root = matches!(session.position, CollectionGridPosition::Root)
                    && self.fullscreen_idx.is_none();
            }
        }
        if deleted_current_root {
            self.install_collection_grid_items(Vec::new(), Vec::new(), None, None);
            if let Some(session) = self.top_level_grid_view.collection_session_mut() {
                session.installed_items_generation = None;
                session.load = CollectionGridLoadState::Deleted { installed: None };
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
                    matches!(session.load, CollectionGridLoadState::Deleted { .. })
                        && session.installed_items_generation == Some(self.items_generation)
                });
        if presents_deleted_rows {
            self.install_collection_grid_items(Vec::new(), Vec::new(), None, None);
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
                        CollectionGridLoadState::RequestNeeded {
                            installed: None,
                            lease: crate::collection_store::CollectionReadLease::dormant(
                                crate::collection_store::CollectionReadScope::app_global("grid"),
                            ),
                        },
                    ))
                } else {
                    None
                }
            });
        match pending {
            Some(CollectionGridLoadState::Snapshot {
                stamp,
                minimum_revision,
                mut lease,
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
                                self.spawn_collection_grid_prepare(
                                    stamp, snapshot, installed, lease,
                                );
                            } else if let Some(session) =
                                self.top_level_grid_view.collection_session_mut()
                            {
                                lease.defer(Instant::now(), "revision");
                                session.load =
                                    CollectionGridLoadState::RequestNeeded { installed, lease };
                            }
                        }
                    }
                    Ok(Err(error)) if error.is_read_retryable() => {
                        if self.collection_grid_stamp_is_current(stamp)
                            && let Some(session) = self.top_level_grid_view.collection_session_mut()
                        {
                            lease.defer(Instant::now(), "actor_retry");
                            session.load =
                                CollectionGridLoadState::RequestNeeded { installed, lease };
                        }
                    }
                    Ok(Err(error)) => {
                        if self.collection_grid_stamp_is_current(stamp)
                            && let Some(session) = self.top_level_grid_view.collection_session_mut()
                        {
                            lease.finish(Instant::now(), "error");
                            session.load = if matches!(error, CollectionStoreError::NotFound) {
                                CollectionGridLoadState::Deleted { installed }
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
                                lease,
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
                            lease.finish(Instant::now(), "disconnected");
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
                mut lease,
                installed,
                cancel,
                receiver,
            }) => match receiver.try_recv() {
                Ok(Ok(prepared)) => {
                    let accepts = self.collection_grid_stamp_is_current(stamp)
                        && prepared.prepared.collection_id == stamp.collection_id
                        && prepared.prepared.collection_revision == exact_revision
                        && prepared.page_edit_revision == self.page_edit_revision
                        && prepared.reuse_key
                            == self.collection_grid_prepare_reuse_key(
                                stamp.collection_id,
                                exact_revision,
                            )
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
                        lease.finish(Instant::now(), "ready");
                        self.apply_collection_grid_prepared_install(prepared, installed);
                        ctx.request_repaint();
                    } else if self.collection_grid_stamp_is_current(stamp)
                        && let Some(session) = self.top_level_grid_view.collection_session_mut()
                    {
                        lease.defer(Instant::now(), "reprepare");
                        session.load = CollectionGridLoadState::RequestNeeded { installed, lease };
                    }
                }
                Ok(Err(CollectionPrepareError::Cancelled)) => {
                    collection_grid_prepare_result_event(stamp, exact_revision, "cancelled");
                    if self.collection_grid_stamp_is_current(stamp)
                        && let Some(session) = self.top_level_grid_view.collection_session_mut()
                    {
                        lease.defer(Instant::now(), "cancelled_reprepare");
                        session.load = CollectionGridLoadState::RequestNeeded { installed, lease };
                    }
                }
                Ok(Err(error)) => {
                    collection_grid_prepare_result_event(stamp, exact_revision, "error");
                    if self.collection_grid_stamp_is_current(stamp)
                        && let Some(session) = self.top_level_grid_view.collection_session_mut()
                    {
                        lease.finish(Instant::now(), "prepare_error");
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
                                lease,
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
                        lease.finish(Instant::now(), "disconnected");
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
                | CollectionGridLoadState::Deleted { .. },
            )
            | None => {}
        }
        self.schedule_collection_grid_snapshot();
        if let Some(delay) = self.collection_grid_poll_delay() {
            ctx.request_repaint_after(delay);
        }
    }

    pub(in crate::app) fn collection_grid_prepare_reuse_key(
        &self,
        collection_id: crate::collection_store::CollectionId,
        revision: u64,
    ) -> CollectionGridPrepareReuseKey {
        CollectionGridPrepareReuseKey::new(
            collection_id,
            revision,
            &self.settings,
            self.video_pin_db.as_ref().map(|db| db.mutation_stamp()),
            self.collection_thumbnail_source_epoch,
        )
    }

    pub(in crate::app) fn apply_collection_grid_prepared_install(
        &mut self,
        install: CollectionGridPreparedInstall,
        previous: Option<Arc<CollectionPreparedSnapshot>>,
    ) {
        let perf_start = crate::perf::is_enabled().then(Instant::now);
        let CollectionGridPreparedInstall {
            prepared,
            page_edits,
            retained_page_edits,
            page_edit_revision,
            thumbnail_sources,
            auto_aspect_lookup,
            reuse_key,
        } = install;
        let CollectionGridPreparedThumbnailDelivery {
            presentation: presentation_sources,
            live:
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
        let collection_seed = self
            .collection_auto_aspect_cache
            .as_mut()
            .and_then(|cache| cache.adopt_lookup(prepared.collection_id, auto_aspect_lookup));
        self.install_collection_grid_items_with_thumbnail_sources(
            items,
            image_metas,
            selected,
            video_sidecars,
            video_pin_blobs,
            collection_seed,
            prepared.auto_aspect_eligible_total,
            page_edits,
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
            let mut presentation =
                CollectionGridInstalledPresentation::new(prepared, presentation_sources, reuse_key);
            presentation.page_edit_snapshot = retained_page_edits;
            presentation.page_edit_revision = page_edit_revision;
            let presentation = Arc::new(presentation);
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
        let reuse_key = self.collection_grid_prepare_reuse_key(
            prepared.collection_id,
            prepared.collection_revision,
        );
        self.apply_collection_grid_prepared_install(
            CollectionGridPreparedInstall {
                prepared: (*prepared).clone(),
                page_edits: None,
                retained_page_edits: None,
                page_edit_revision: self.page_edit_revision,
                thumbnail_sources: CollectionGridPreparedThumbnailDelivery::default(),
                auto_aspect_lookup: None,
                reuse_key,
            },
            previous,
        );
    }

    fn install_collection_grid_items(
        &mut self,
        items: Vec<GridItem>,
        image_metas: Vec<Option<(i64, i64)>>,
        selected: Option<usize>,
        collection_seed: Option<crate::auto_aspect_cache::AutoAspectCacheEntry>,
    ) {
        let auto_aspect_eligible_total = items
            .iter()
            .filter(|item| {
                !matches!(
                    item,
                    GridItem::CollectionPlaceholder { .. } | GridItem::Audio(_)
                )
            })
            .count();
        self.install_collection_grid_items_with_thumbnail_sources(
            items,
            image_metas,
            selected,
            std::collections::HashMap::new(),
            Arc::new(std::collections::HashMap::new()),
            collection_seed,
            auto_aspect_eligible_total,
            None,
        );
    }

    fn install_collection_grid_items_with_thumbnail_sources(
        &mut self,
        items: Vec<GridItem>,
        image_metas: Vec<Option<(i64, i64)>>,
        selected: Option<usize>,
        video_sidecars: std::collections::HashMap<String, std::path::PathBuf>,
        video_pin_blobs: Arc<std::collections::HashMap<std::path::PathBuf, Vec<u8>>>,
        collection_seed: Option<crate::auto_aspect_cache::AutoAspectCacheEntry>,
        auto_aspect_eligible_total: usize,
        page_edits: Option<(
            super::page_edit_snapshot::PageEditSnapshot,
            super::page_edit_snapshot::PageEditProjection,
        )>,
    ) {
        // A grid menu owns the old item generation. Do not let its saved row index operate on
        // the newly installed collection; a menu owned by another viewer context is untouched.
        if self
            .context_menu_idx
            .is_some_and(|owner| self.grid_context_menu_owner_is_current(owner))
        {
            self.context_menu_idx = None;
        }
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
        if let Some((snapshot, mut projection)) = page_edits {
            self.page_edit_snapshot = Some(snapshot);
            self.adjustment_page_params = std::mem::take(&mut projection.adjustment);
            self.export_crop_page_settings = std::mem::take(&mut projection.export_crop);
            self.export_crop_pages = self.export_crop_page_settings.keys().copied().collect();
            self.view_trim_page_overrides = std::mem::take(&mut projection.view_trim);
            self.mask_pages = std::mem::take(&mut projection.mask);
            self.conceal_pages = std::mem::take(&mut projection.conceal);
            self.comic_pages = std::mem::take(&mut projection.comic);
            self.local_adjust_pages = std::mem::take(&mut projection.local_adjust);
        }
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
        self.reset_and_seed_auto_aspect_with_collection_seed(
            &cache_map,
            collection_seed,
            Some(auto_aspect_eligible_total),
        );
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
    error.user_message()
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};
    use std::time::{Duration, Instant};

    use super::*;
    use crate::collection_store::{
        CollectionRegistration, CollectionResolvedKind, CollectionStoreRuntime,
    };
    use crate::grid_item::ThumbnailState;

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
        while !matches!(app.collection_store_client_for_read(), Ok(Some(_))) {
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
    fn phase_a_collection_mask_is_projected_for_accepted_revision() {
        let temp = tempfile::tempdir().unwrap();
        let image = temp.path().join("masked.png");
        std::fs::write(&image, b"image").unwrap();
        let (mut app, client) = start_ready_app(&temp.path().join("collection.db"));
        let key = crate::adjustment_db::normalize_path(&image);
        app.mask_db
            .as_ref()
            .unwrap()
            .set(&key, &[true], &[], 1, 1)
            .unwrap();
        app.conceal_db
            .as_ref()
            .unwrap()
            .set(&key, &[true], &[], 1, 1)
            .unwrap();
        let snapshot =
            collection_with_sources(&client, &[(image.clone(), CollectionResolvedKind::Image)]);
        app.open_collection_grid(snapshot.collection_id(), None);
        wait_for_grid(&mut app, snapshot.collection_id());
        assert_eq!(app.page_path_key(0).as_deref(), Some(key.as_str()));
        assert!(
            app.mask_pages.contains(&0),
            "paint path must apply the saved mask"
        );
        assert!(
            app.conceal_pages.contains(&0),
            "paint path must apply the saved conceal edit"
        );
        app.shutdown_collection_runtime_for_exit();
    }

    #[test]
    fn phase_a_collection_install_projects_zip_page_key() {
        let temp = tempfile::tempdir().unwrap();
        let zip = temp.path().join("pages.zip");
        std::fs::write(&zip, b"zip").unwrap();
        let (mut app, _client) = start_ready_app(&temp.path().join("collection.db"));
        let item = GridItem::ZipImage {
            zip_path: zip.clone(),
            entry_name: "page.png".into(),
        };
        let key = crate::adjustment_db::zip_entry_key(&zip, "page.png");
        app.mask_db
            .as_ref()
            .unwrap()
            .set(&key, &[true], &[], 1, 1)
            .unwrap();
        let prepared = super::super::page_edit_snapshot::PageEditSnapshot::load_and_project(
            std::slice::from_ref(&item),
            super::super::page_edit_snapshot::PageEditAvailability {
                mask: true,
                ..Default::default()
            },
            &AtomicBool::new(false),
        )
        .unwrap()
        .unwrap();
        app.install_collection_grid_items_with_thumbnail_sources(
            vec![item],
            vec![Some((1, 5))],
            None,
            std::collections::HashMap::new(),
            Arc::new(std::collections::HashMap::new()),
            None,
            0,
            Some(prepared),
        );
        assert_eq!(app.page_path_key(0).as_deref(), Some(key.as_str()));
        assert!(
            app.mask_pages.contains(&0),
            "ZIP member uses its exact page key"
        );
        app.shutdown_collection_runtime_for_exit();
    }

    #[test]
    fn root_auto_aspect_restores_before_real_rows_and_excludes_unsampleable_items() {
        use crate::auto_aspect_cache::CollectionAutoAspectCache;
        use crate::settings::ThumbAspect;

        let temp = tempfile::tempdir().unwrap();
        let image = temp.path().join("cover.png");
        let audio = temp.path().join("song.mp3");
        let missing = temp.path().join("missing.png");
        std::fs::write(&image, b"test image").unwrap();
        std::fs::write(&audio, b"test audio").unwrap();
        let (mut app, client) = start_ready_app(&temp.path().join("collection.db"));
        let created = collection_with_sources(
            &client,
            &[
                (image, CollectionResolvedKind::Image),
                (audio, CollectionResolvedKind::Audio),
                (missing, CollectionResolvedKind::Image),
            ],
        );
        let cache_path = temp.path().join("auto_aspect_cache.db");
        let mut writer = CollectionAutoAspectCache::spawn_at(cache_path.clone()).unwrap();
        writer
            .record(created.collection_id(), ThumbAspect::Landscape16x9, 7, 9)
            .unwrap();
        drop(writer); // Process restart: only the SQLite row survives.
        app.collection_auto_aspect_cache =
            Some(CollectionAutoAspectCache::spawn_at(cache_path).unwrap());
        app.settings.thumb_aspect_auto = true;

        app.open_collection_grid(created.collection_id(), None);
        assert!(app.items.is_empty());
        assert_eq!(app.auto_aspect.current, None);
        wait_for_grid(&mut app, created.collection_id());
        assert_eq!(app.auto_aspect.current, Some(ThumbAspect::Landscape16x9));
        assert_eq!(app.auto_aspect_eligible_total(), 1);
        let installed = app
            .top_level_grid_view
            .collection_session()
            .and_then(CollectionGridSession::installed_presentation)
            .unwrap()
            .clone();
        assert_eq!(installed.prepared.auto_aspect_eligible_total, 1);
        let original_generation = app.items_generation;
        app.items_generation = app.items_generation.wrapping_add(1);
        assert_eq!(
            app.auto_aspect_eligible_total(),
            0,
            "a stale items-generation binding must not expose the prepared denominator"
        );
        app.items_generation = original_generation;
        let mut wrong_context = (*installed.prepared).clone();
        wrong_context.collection_id = CollectionId::new();
        app.top_level_grid_view
            .collection_session_mut()
            .unwrap()
            .load =
            CollectionGridLoadState::Ready(Arc::new(CollectionGridInstalledPresentation::new(
                Arc::new(wrong_context),
                installed.sources.clone(),
                installed.reuse_key.clone(),
            )));
        assert_eq!(
            app.auto_aspect_eligible_total(),
            0,
            "a prepared snapshot from another Collection context must fail closed"
        );
        let mut wrong_revision = (*installed.prepared).clone();
        wrong_revision.collection_revision = wrong_revision.collection_revision.wrapping_add(1);
        app.top_level_grid_view
            .collection_session_mut()
            .unwrap()
            .load =
            CollectionGridLoadState::Ready(Arc::new(CollectionGridInstalledPresentation::new(
                Arc::new(wrong_revision),
                installed.sources.clone(),
                installed.reuse_key.clone(),
            )));
        assert_eq!(app.auto_aspect_eligible_total(), 0);
        let mut wrong_length = (*installed.prepared).clone();
        wrong_length.entries = Arc::from(Vec::new());
        app.top_level_grid_view
            .collection_session_mut()
            .unwrap()
            .load =
            CollectionGridLoadState::Ready(Arc::new(CollectionGridInstalledPresentation::new(
                Arc::new(wrong_length),
                installed.sources.clone(),
                installed.reuse_key.clone(),
            )));
        assert_eq!(app.auto_aspect_eligible_total(), 0);
        app.top_level_grid_view
            .collection_session_mut()
            .unwrap()
            .load = CollectionGridLoadState::Ready(installed);
        assert_eq!(app.auto_aspect_eligible_total(), 1);
        assert!(matches!(app.auto_aspect_cache_target(),
            Some(super::super::AutoAspectCacheTarget::CollectionRoot(id)) if id == created.collection_id()));

        app.save_auto_aspect_cache(ThumbAspect::Portrait3x4, 1, 1);
        app.open_collection_grid(created.collection_id(), None);
        assert!(app.items.is_empty());
        assert_eq!(app.auto_aspect.current, Some(ThumbAspect::Portrait3x4));
        wait_for_grid(&mut app, created.collection_id());
        assert_eq!(app.auto_aspect.current, Some(ThumbAspect::Portrait3x4));

        let entry = app
            .top_level_grid_view
            .collection_session()
            .unwrap()
            .prepared()
            .unwrap()
            .entries[0]
            .clone();
        let session = app.top_level_grid_view.collection_session_mut().unwrap();
        session.wanted_revision += 1;
        assert!(app.auto_aspect_cache_target().is_none());
        app.save_auto_aspect_cache(ThumbAspect::Square, 1, 1);
        assert_eq!(
            app.collection_auto_aspect_cache
                .as_ref()
                .unwrap()
                .cached(created.collection_id())
                .unwrap()
                .aspect,
            ThumbAspect::Portrait3x4
        );
        let session = app.top_level_grid_view.collection_session_mut().unwrap();
        session.wanted_revision = session.accepted_revision;
        session.position = CollectionGridPosition::PhysicalSource {
            entry_id: entry.entry_id,
            source_key: entry.source_key,
            path: temp.path().join("child"),
        };
        assert!(app.auto_aspect_cache_target().is_none());
        let session = app.top_level_grid_view.collection_session_mut().unwrap();
        session.position = CollectionGridPosition::Root;
        session.load = CollectionGridLoadState::Failed {
            message: "test".into(),
            installed: None,
        };
        assert!(app.auto_aspect_cache_target().is_none());
        app.top_level_grid_view
            .collection_session_mut()
            .unwrap()
            .load = CollectionGridLoadState::Deleted { installed: None };
        assert!(app.auto_aspect_cache_target().is_none());
        app.shutdown_collection_runtime_for_exit();
    }

    #[test]
    fn root_order_controls_use_installed_revision_and_preserve_global_sort_and_header_contract() {
        use crate::collection_store::CollectionOrderMode;
        use crate::settings::{DetailsSortKey, GridViewMode, SortOrder};

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

        assert!(app.request_collection_grid_set_order(
            manual,
            CollectionOrderMode::Standard,
            SortOrder::FileName,
            CollectionGridSetOrderRoute::StandardControl,
        ));
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
        assert!(!app.request_collection_grid_set_order(
            standard,
            CollectionOrderMode::Standard,
            standard.standard_sort,
            CollectionGridSetOrderRoute::StandardControl,
        ));
        assert_eq!(app.settings.details_sort_key, DetailsSortKey::Name);
        assert!(app.request_collection_grid_set_order(
            standard,
            CollectionOrderMode::Standard,
            standard.standard_sort,
            CollectionGridSetOrderRoute::CollectionMenu,
        ));
        assert_eq!(app.settings.details_sort_key, DetailsSortKey::Toolbar);
        assert!(
            app.collection_export_all_available(),
            "local header reset must not enqueue an actor operation"
        );
        app.set_details_sort_key(DetailsSortKey::Modified);
        app.set_details_sort_key(DetailsSortKey::Name);
        assert_eq!(app.settings.details_sort_key, DetailsSortKey::Name);
        assert!(app.settings.details_sort_ascending);
        assert_eq!(
            app.settings.details_sort_key,
            DetailsSortKey::Name,
            "a later header selection must have no pending no-op reply to overwrite it"
        );
        assert!(app.request_collection_grid_set_order(
            standard,
            CollectionOrderMode::Standard,
            standard.standard_sort,
            CollectionGridSetOrderRoute::CollectionMenu,
        ));
        assert_eq!(app.settings.details_sort_key, DetailsSortKey::Toolbar);
        let unchanged_standard = app.collection_grid_root_order().unwrap().unwrap();
        assert_eq!(
            unchanged_standard.content.expected_revision,
            standard.content.expected_revision
        );
        assert_eq!(app.settings.sort_order, SortOrder::DateDesc);

        app.settings.details_sort_key = DetailsSortKey::Name;
        app.rebuild_details_order();
        let standard = app.collection_grid_root_order().unwrap().unwrap();
        assert!(app.request_collection_grid_set_order(
            standard,
            CollectionOrderMode::Shuffle,
            standard.standard_sort,
            CollectionGridSetOrderRoute::CollectionMenu,
        ));
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
        let first_shuffle_seed = recv(
            client
                .load_collection(created.collection_id())
                .expect("load first shuffle"),
        )
        .definition
        .shuffle_seed;
        assert!(app.request_collection_grid_set_order(
            shuffle,
            CollectionOrderMode::Shuffle,
            shuffle.standard_sort,
            CollectionGridSetOrderRoute::StandardControl,
        ));
        poll_until(
            &mut app,
            "Shuffle reselection did not install",
            |app| matches!(app.collection_grid_root_order(), Some(Ok(order)) if order.mode == CollectionOrderMode::Shuffle && order.content.expected_revision > shuffle.content.expected_revision),
        );
        assert_eq!(app.settings.sort_order, SortOrder::DateDesc);
        let second_shuffle_seed = recv(
            client
                .load_collection(created.collection_id())
                .expect("load second shuffle"),
        )
        .definition
        .shuffle_seed;
        assert_ne!(first_shuffle_seed, second_shuffle_seed);

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
        assert!(app.request_collection_grid_set_order(
            manual,
            CollectionOrderMode::Standard,
            SortOrder::FileName,
            CollectionGridSetOrderRoute::StandardControl,
        ));
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
                    .is_some_and(|session| {
                        matches!(session.load, CollectionGridLoadState::Deleted { .. })
                    })
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
            auto_aspect_eligible_total: previous.auto_aspect_eligible_total,
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
    fn direct_page_close_from_collection_pdf_and_zip_restores_root_anchor() {
        let temp = tempfile::tempdir().unwrap();
        let pdf = temp.path().join("book.pdf");
        let zip = temp.path().join("book.zip");
        std::fs::write(&pdf, b"%PDF-1.4\n").unwrap();
        std::fs::write(&zip, b"collection return fixture").unwrap();
        let (mut app, client) = start_ready_app(&temp.path().join("collection.db"));
        let snapshot = collection_with_sources(
            &client,
            &[
                (pdf.clone(), CollectionResolvedKind::Pdf),
                (zip.clone(), CollectionResolvedKind::Zip),
            ],
        );
        app.settings.auto_fullscreen_zip_pdf = true;
        app.open_collection_grid(snapshot.collection_id(), None);
        wait_for_grid(&mut app, snapshot.collection_id());
        let expected_order: Vec<_> = app
            .top_level_grid_view
            .collection_session()
            .and_then(CollectionGridSession::prepared)
            .unwrap()
            .entries
            .iter()
            .map(|entry| entry.entry_id)
            .collect();

        for source in [&pdf, &zip] {
            let source_index = app
                .items
                .iter()
                .position(|item| item.drag_source_path() == Some(source.as_path()))
                .expect("collection container row");
            app.selected = Some(source_index);
            app.scroll_offset_y = 360.0;
            app.scroll_to_selected = false;
            let anchor = app
                .collection_grid_source_anchor(source_index, source)
                .expect("stable collection source anchor");
            app.commit_collection_grid_source_open(anchor.clone(), source.clone());
            app.current_folder = Some(source.clone());
            app.items = if source == &pdf {
                vec![GridItem::PdfPage {
                    pdf_path: source.clone(),
                    page_num: 0,
                    content_type: None,
                }]
            } else {
                vec![GridItem::ZipImage {
                    zip_path: source.clone(),
                    entry_name: "page.jpg".into(),
                }]
            };
            app.thumbnails = vec![ThumbnailState::Pending];
            app.image_metas = vec![None];
            app.visible_indices = vec![0];
            app.items_generation = app.items_generation.wrapping_add(1);
            app.fullscreen_idx = Some(0);
            app.pending_return_to_parent = false;

            app.handle_fullscreen_close_request();
            assert!(app.pending_return_to_parent);
            assert_eq!(app.fullscreen_idx, Some(0));
            let close_consumed_before_render = source == &pdf;
            let nav = if close_consumed_before_render {
                let ctx = egui::Context::default();
                ctx.begin_pass(egui::RawInput::default());
                let nav = app.handle_keyboard(&ctx);
                let _ = ctx.end_pass();
                nav.expect("frame-start keyboard handling must consume the close request")
            } else {
                app.take_pending_return_to_parent_nav()
                    .expect("post-render close handling must consume the close request")
            };
            let crate::ui_main::AddressBarNav::Collection(restore) = &nav else {
                panic!("collection-owned book must not fall through to its physical parent")
            };
            assert_eq!(restore.identity.collection_id, snapshot.collection_id());
            assert_eq!(restore.revision_at_open, snapshot.revision());
            assert_eq!(restore.viewport_anchor.as_ref(), Some(&anchor));
            assert!(app.select_after_load.is_none());

            if close_consumed_before_render {
                let crate::ui_main::AddressBarNav::Collection(restore) = nav else {
                    unreachable!("the collection route was checked above")
                };
                app.apply_collection_input_nav(restore, true);
            } else {
                assert!(app.apply_fullscreen_close_nav_immediate(nav));
            }
            wait_for_grid(&mut app, snapshot.collection_id());
            assert_eq!(app.fullscreen_idx, None);
            assert!(matches!(
                app.top_level_grid_view
                    .collection_session()
                    .unwrap()
                    .position,
                CollectionGridPosition::Root
            ));
            let prepared = app
                .top_level_grid_view
                .collection_session()
                .and_then(CollectionGridSession::prepared)
                .unwrap();
            assert_eq!(
                prepared
                    .entries
                    .iter()
                    .map(|entry| entry.entry_id)
                    .collect::<Vec<_>>(),
                expected_order,
                "return must preserve collection order"
            );
            assert_eq!(
                prepared.entries[app.selected.expect("restored selection")].entry_id,
                anchor.entry_id
            );
            assert!(app.scroll_to_selected);
        }
        app.shutdown_collection_runtime_for_exit();
    }

    #[test]
    fn ordinary_collection_navigation_does_not_close_fullscreen() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("page.png");
        std::fs::write(&source, b"image").unwrap();
        let (mut app, client) = start_ready_app(&temp.path().join("collection.db"));
        let snapshot = collection_with_sources(&client, &[(source, CollectionResolvedKind::Image)]);
        app.open_collection_grid(snapshot.collection_id(), None);
        wait_for_grid(&mut app, snapshot.collection_id());
        let TopLevelGridRestore::Collection(restore) = app
            .collection_grid_restore_snapshot()
            .expect("mounted collection restore")
        else {
            panic!("mounted collection must expose its typed restore")
        };

        app.fullscreen_idx = Some(0);
        app.apply_collection_input_nav(restore, false);

        assert_eq!(
            app.fullscreen_idx,
            Some(0),
            "ordinary collection navigation must not inherit direct-page close semantics"
        );
        app.shutdown_collection_runtime_for_exit();
    }

    #[test]
    fn collection_parent_resolution_is_owned_by_the_mounted_viewer_context() {
        let temp = tempfile::tempdir().unwrap();
        let folder = temp.path().join("book");
        let nested = folder.join("chapter");
        std::fs::create_dir_all(&nested).unwrap();
        let (mut app, client) = start_ready_app(&temp.path().join("collection.db"));
        let snapshot =
            collection_with_sources(&client, &[(folder.clone(), CollectionResolvedKind::Folder)]);
        app.open_collection_grid(snapshot.collection_id(), None);
        wait_for_grid(&mut app, snapshot.collection_id());
        let anchor = app
            .collection_grid_source_anchor(0, &folder)
            .expect("collection source anchor");
        app.commit_collection_grid_source_open(anchor.clone(), folder);
        app.current_folder = Some(nested);

        let crate::ui_main::AddressBarNav::Collection(descendant_restore) = app
            .resolve_return_to_parent_nav()
            .expect("collection descendant keeps its root parent")
        else {
            panic!("collection descendant must not fall through to its physical parent")
        };
        assert_eq!(descendant_restore.viewport_anchor.as_ref(), Some(&anchor));

        let sibling_book = temp.path().join("sibling").join("other.pdf");
        let sibling_parent = sibling_book.parent().unwrap().to_path_buf();
        let sibling = app.build_window_context_for_test(9_901, |sibling| {
            sibling.current_folder = Some(sibling_book.clone());
            sibling.select_after_load = None;
        });
        app.with_viewer_context(sibling, |sibling| {
            assert!(matches!(
                sibling.resolve_return_to_parent_nav(),
                Some(crate::ui_main::AddressBarNav::Direct(path)) if path == sibling_parent
            ));
            assert_eq!(sibling.select_after_load.as_deref(), Some("other.pdf"));
        })
        .expect("mount sibling context");

        let crate::ui_main::AddressBarNav::Collection(restore) = app
            .resolve_return_to_parent_nav()
            .expect("mounted collection context owns its parent")
        else {
            panic!("mounted collection context must resolve independently of its sibling")
        };
        assert_eq!(restore.identity.collection_id, snapshot.collection_id());
        assert_eq!(restore.viewport_anchor.as_ref(), Some(&anchor));
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

    fn collection_jump_request(
        app: &App,
        source: &Path,
    ) -> crate::ui_dialogs::context_menu::JumpToFolderRequest {
        use crate::ui_dialogs::context_menu::{
            JumpToFolderDestination, JumpToFolderRequest, JumpToFolderSelection,
        };
        JumpToFolderRequest {
            destination: JumpToFolderDestination::PhysicalDirectory(
                source.parent().unwrap().to_path_buf(),
            ),
            selection: JumpToFolderSelection::ExactPath(source.to_path_buf()),
            origin: Some(
                app.collection_grid_physical_load_owner(0, source)
                    .expect("mounted collection source"),
            ),
        }
    }

    #[test]
    fn collection_location_jump_preserves_root_until_success_and_selects_exact_source() {
        let temp = tempfile::tempdir().unwrap();
        let parent = temp.path().join("source-folder");
        std::fs::create_dir(&parent).unwrap();
        let source = parent.join("image.png");
        std::fs::write(&source, b"image").unwrap();
        let (mut app, client) = start_ready_app(&temp.path().join("collection.db"));
        let snapshot =
            collection_with_sources(&client, &[(source.clone(), CollectionResolvedKind::Image)]);
        app.open_collection_grid(snapshot.collection_id(), None);
        wait_for_grid(&mut app, snapshot.collection_id());
        let sibling = super::super::QuickFolderSlotId::B;
        let sibling_history = vec![super::super::FolderNavHistoryTarget::Path(
            temp.path().join("sibling-location"),
        )];
        app.quick_folder_workspaces[sibling.index()]
            .history
            .back_stack = sibling_history.clone();
        app.selected = Some(0);
        app.scroll_offset_y = 107.0;
        let generation = app.items_generation;
        let address = app.address.clone();
        let request = collection_jump_request(&app, &source);
        assert!(app.begin_context_jump_to_folder(request).is_none());
        assert!(app.context_folder_jump_pending());
        assert_eq!(app.items_generation, generation);
        assert_eq!(app.selected, Some(0));
        assert_eq!(app.scroll_offset_y, 107.0);
        assert_eq!(app.address, address);
        assert!(matches!(
            app.top_level_grid_view.surface(),
            TopLevelGridSurface::Collection(_)
        ));

        let pending = app.folder_pane_open_pending.as_ref().unwrap();
        assert!(matches!(
            pending.purpose,
            super::super::FolderOpenScanPurpose::JumpToPhysicalFolder { .. }
        ));
        app.cancel_folder_pane_open();
        assert!(app.folder_pane_open_pending.is_none());
        assert_eq!(
            app.items_generation, generation,
            "cancellation keeps the source root"
        );

        let request = collection_jump_request(&app, &source);
        assert!(app.begin_context_jump_to_folder(request.clone()).is_none());
        let replaced_cancel = Arc::clone(&app.folder_pane_open_pending.as_ref().unwrap().cancel);
        assert!(app.begin_context_jump_to_folder(request.clone()).is_none());
        assert!(replaced_cancel.load(Ordering::Relaxed));
        assert_eq!(app.items_generation, generation);
        let pending = app.folder_pane_open_pending.take().unwrap();
        pending.cancel.store(true, Ordering::Relaxed);
        let scan = super::super::folder_scan::scan_directory_with_settings(&parent, &app.settings)
            .unwrap();
        app.apply_jump_to_physical_folder_ready(
            parent.clone(),
            scan,
            request.selection,
            request.origin,
        );
        assert!(matches!(
            app.top_level_grid_view.surface(),
            TopLevelGridSurface::Folder
        ));
        assert_eq!(app.current_folder.as_deref(), Some(parent.as_path()));
        assert!(app.selected.is_some_and(|index| {
            app.items[index]
                .drag_source_path()
                .is_some_and(|path| crate::folder_tree::path_eq(path, &source))
        }));
        assert!(app.scroll_to_selected);
        assert!(matches!(
            app.folder_history_back_target(),
            Some(super::super::FolderNavHistoryTarget::Collection(restore))
                if restore.identity.collection_id == snapshot.collection_id()
        ));
        assert_eq!(
            app.quick_folder_workspaces[sibling.index()]
                .history
                .back_stack,
            sibling_history
        );
        app.shutdown_collection_runtime_for_exit();
    }

    #[test]
    fn collection_location_jump_rejects_stale_revision_items_and_context() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("image.png");
        std::fs::write(&source, b"image").unwrap();
        let (mut app, client) = start_ready_app(&temp.path().join("collection.db"));
        let snapshot =
            collection_with_sources(&client, &[(source.clone(), CollectionResolvedKind::Image)]);
        app.open_collection_grid(snapshot.collection_id(), None);
        wait_for_grid(&mut app, snapshot.collection_id());
        let request = collection_jump_request(&app, &source);
        let held_generation = app.items_generation;
        let origin = request.origin.clone().unwrap();
        app.top_level_grid_view
            .collection_session_mut()
            .unwrap()
            .wanted_revision += 1;
        assert!(app.begin_context_jump_to_folder(request.clone()).is_none());
        assert!(app.folder_pane_open_pending.is_none());
        assert!(
            app.fs_feedback_toast
                .as_ref()
                .is_some_and(|toast| { toast.0.contains("もう一度選択してください") })
        );
        let scan =
            super::super::folder_scan::scan_directory_with_settings(temp.path(), &app.settings)
                .unwrap();
        app.apply_jump_to_physical_folder_ready(
            temp.path().to_path_buf(),
            scan,
            request.selection.clone(),
            Some(origin.clone()),
        );
        assert_eq!(app.items_generation, held_generation);
        assert!(matches!(
            app.top_level_grid_view.surface(),
            TopLevelGridSurface::Collection(_)
        ));

        app.top_level_grid_view
            .collection_session_mut()
            .unwrap()
            .wanted_revision = origin.wanted_revision;
        app.items_generation += 1;
        let scan =
            super::super::folder_scan::scan_directory_with_settings(temp.path(), &app.settings)
                .unwrap();
        app.apply_jump_to_physical_folder_ready(
            temp.path().to_path_buf(),
            scan,
            request.selection.clone(),
            Some(origin.clone()),
        );
        assert!(matches!(
            app.top_level_grid_view.surface(),
            TopLevelGridSurface::Collection(_)
        ));

        app.items_generation = held_generation;
        let mut wrong_context = origin;
        wrong_context.stamp.context_id = ViewerContextId::for_test(999);
        let scan =
            super::super::folder_scan::scan_directory_with_settings(temp.path(), &app.settings)
                .unwrap();
        app.apply_jump_to_physical_folder_ready(
            temp.path().to_path_buf(),
            scan,
            request.selection,
            Some(wrong_context),
        );
        assert!(matches!(
            app.top_level_grid_view.surface(),
            TopLevelGridSurface::Collection(_)
        ));
        app.shutdown_collection_runtime_for_exit();
    }

    #[cfg(windows)]
    #[test]
    fn collection_location_jump_scan_failure_keeps_root_and_missing_leaf_reports_exact_miss() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("image.png");
        std::fs::write(&source, b"image").unwrap();
        let (mut app, client) = start_ready_app(&temp.path().join("collection.db"));
        let snapshot =
            collection_with_sources(&client, &[(source.clone(), CollectionResolvedKind::Image)]);
        app.open_collection_grid(snapshot.collection_id(), None);
        wait_for_grid(&mut app, snapshot.collection_id());
        let generation = app.items_generation;
        let request = collection_jump_request(&app, &source);
        assert!(app.begin_context_jump_to_folder(request).is_none());
        let pending = app.folder_pane_open_pending.take().unwrap();
        pending.cancel.store(true, Ordering::Relaxed);
        let ready = super::super::FolderPaneOpenReady {
            path: pending.path,
            scan: Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "denied",
            )),
            purpose: pending.purpose,
        };
        assert!(
            app.resolve_main_folder_open_ready(&egui::Context::default(), ready)
                .is_none()
        );
        assert_eq!(app.items_generation, generation);
        assert!(matches!(
            app.top_level_grid_view.surface(),
            TopLevelGridSurface::Collection(_)
        ));

        let request = collection_jump_request(&app, &source);
        std::fs::remove_file(&source).unwrap();
        let scan =
            super::super::folder_scan::scan_directory_with_settings(temp.path(), &app.settings)
                .unwrap();
        app.apply_jump_to_physical_folder_ready(
            temp.path().to_path_buf(),
            scan,
            request.selection,
            request.origin,
        );
        assert!(matches!(
            app.top_level_grid_view.surface(),
            TopLevelGridSurface::Folder
        ));
        assert!(app.fs_feedback_toast.as_ref().is_some_and(|toast| {
            toast.0.contains("移動先の項目が見つかりません")
        }));
        assert!(!app.selected.is_some_and(|index| {
            app.items[index]
                .drag_source_path()
                .is_some_and(|path| crate::folder_tree::path_eq(path, &source))
        }));
        app.shutdown_collection_runtime_for_exit();
    }

    #[test]
    fn missing_placeholder_location_jump_uses_registered_exact_path_without_physical_capability() {
        let temp = tempfile::tempdir().unwrap();
        let parent = temp.path().join("source-folder");
        std::fs::create_dir(&parent).unwrap();
        let missing = parent.join("missing.png");
        let (mut app, client) = start_ready_app(&temp.path().join("collection.db"));
        let snapshot =
            collection_with_sources(&client, &[(missing.clone(), CollectionResolvedKind::Image)]);
        app.open_collection_grid(snapshot.collection_id(), None);
        wait_for_grid(&mut app, snapshot.collection_id());
        assert!(matches!(
            &app.items[0],
            GridItem::CollectionPlaceholder { path, .. }
                if crate::folder_tree::path_eq(path, &missing)
        ));
        assert!(app.items[0].drag_source_path().is_none());

        let request = collection_jump_request(&app, &missing);
        assert!(app.begin_context_jump_to_folder(request.clone()).is_none());
        assert!(app.context_folder_jump_pending());
        let pending = app.folder_pane_open_pending.take().unwrap();
        pending.cancel.store(true, Ordering::Relaxed);
        let scan = super::super::folder_scan::scan_directory_with_settings(&parent, &app.settings)
            .unwrap();
        app.apply_jump_to_physical_folder_ready(
            parent.clone(),
            scan,
            request.selection,
            request.origin,
        );
        assert!(matches!(
            app.top_level_grid_view.surface(),
            TopLevelGridSurface::Folder
        ));
        assert_eq!(app.current_folder.as_deref(), Some(parent.as_path()));
        assert!(app.fs_feedback_toast.as_ref().is_some_and(|toast| {
            toast.0.contains("移動先の項目が見つかりません")
        }));
        app.shutdown_collection_runtime_for_exit();
    }

    #[test]
    fn collection_location_jump_rejected_by_restore_keeps_root_and_notifies_origin() {
        let temp = tempfile::tempdir().unwrap();
        let parent = temp.path().join("source-folder");
        std::fs::create_dir(&parent).unwrap();
        let source = parent.join("image.png");
        std::fs::write(&source, b"image").unwrap();
        let (mut app, client) = start_ready_app(&temp.path().join("collection.db"));
        let snapshot =
            collection_with_sources(&client, &[(source.clone(), CollectionResolvedKind::Image)]);
        app.open_collection_grid(snapshot.collection_id(), None);
        wait_for_grid(&mut app, snapshot.collection_id());
        let held_generation = app.items_generation;
        let held_address = app.address.clone();
        let request = collection_jump_request(&app, &source);
        assert!(app.begin_context_jump_to_folder(request.clone()).is_none());
        let pending = app.folder_pane_open_pending.take().unwrap();
        pending.cancel.store(true, Ordering::Relaxed);
        let scan = super::super::folder_scan::scan_directory_with_settings(&parent, &app.settings)
            .unwrap();

        // The restore began after the parent scan. The completed scan must be discarded without
        // replacing the source Collection root, and the reason belongs to that origin context.
        app.activate_sidecar_restore_modal_for_test(parent.clone());
        app.apply_jump_to_physical_folder_ready(
            parent.clone(),
            scan,
            request.selection.clone(),
            request.origin.clone(),
        );
        assert_eq!(app.items_generation, held_generation);
        assert_eq!(app.address, held_address);
        assert!(app.current_folder.is_none());
        assert!(matches!(
            app.top_level_grid_view.surface(),
            TopLevelGridSurface::Collection(_)
        ));
        assert!(app.fs_feedback_toast.as_ref().is_some_and(|toast| {
            toast.0.contains("サイドカー復元中") && toast.0.contains("もう一度選択")
        }));

        // The same rejection at admission also leaves no hidden scan to replay later.
        assert!(app.begin_context_jump_to_folder(request).is_none());
        assert!(app.folder_pane_open_pending.is_none());
        app.sidecar_restore = None;
        app.shutdown_collection_runtime_for_exit();
    }

    #[test]
    fn normal_folder_rescan_preserves_menu_on_identical_content_and_retires_it_on_change() {
        let temp = tempfile::tempdir().unwrap();
        let folder = temp.path().join("folder");
        std::fs::create_dir(&folder).unwrap();
        std::fs::write(folder.join("a.png"), b"a").unwrap();
        let mut app = crate::app::setup_app_for_test();
        app.settings.sidecar_backup_enabled = false;
        app.settings.tag_sidecar_backup_enabled = false;
        let initial =
            super::super::folder_scan::scan_directory_with_settings(&folder, &app.settings)
                .unwrap();
        app.load_folder_with_scan(folder.clone(), Some(initial));
        let generation = app.items_generation;
        let menu_owner = app.capture_grid_context_menu_owner(0);
        app.context_menu_idx = Some(menu_owner);

        let unchanged =
            super::super::folder_scan::scan_directory_with_settings(&folder, &app.settings)
                .unwrap();
        app.apply_external_rescan(folder.clone(), std::time::SystemTime::now(), unchanged);
        assert_eq!(app.items_generation, generation);
        assert_eq!(app.context_menu_idx, Some(menu_owner));

        std::fs::write(folder.join("b.png"), b"b").unwrap();
        let changed =
            super::super::folder_scan::scan_directory_with_settings(&folder, &app.settings)
                .unwrap();
        app.apply_external_rescan(folder, std::time::SystemTime::now(), changed);
        assert_ne!(app.items_generation, generation);
        assert!(app.context_menu_idx.is_none());
    }

    #[test]
    fn grid_menu_owner_rejects_replaced_items_and_collection_install_preserves_sibling_menu() {
        let mut app = crate::app::setup_app_for_test();
        let current = app.capture_grid_context_menu_owner(0);
        app.context_menu_idx = Some(current);
        app.install_collection_grid_items(Vec::new(), Vec::new(), None, None);
        assert!(app.context_menu_idx.is_none());

        let current = app.capture_grid_context_menu_owner(0);
        let sibling = crate::ui_dialogs::context_menu::GridContextMenuOwner {
            context_id: ViewerContextId::for_test(current.context_id.serial() + 100),
            ..current
        };
        app.context_menu_idx = Some(sibling);
        app.install_collection_grid_items(Vec::new(), Vec::new(), None, None);
        assert_eq!(app.context_menu_idx, Some(sibling));

        app.context_menu_idx = Some(current);
        assert!(app.show_context_menu(&egui::Context::default()).is_none());
        assert!(app.context_menu_idx.is_none());
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
                        && matches!(session.load, CollectionGridLoadState::Deleted { .. })
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
            CollectionGridLoadState::RequestNeeded {
                installed: Some(_),
                ..
            }
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
                            && matches!(session.load, CollectionGridLoadState::Deleted { .. })
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
            lease: crate::collection_store::CollectionReadLease::new(
                crate::collection_store::CollectionReadScope::app_global("grid-test"),
                Instant::now(),
                "prepare",
            ),
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
            CollectionGridLoadState::RequestNeeded {
                installed: Some(_),
                ..
            }
        ));
        assert_eq!(
            session.prepared().unwrap().collection_revision,
            snapshot.revision()
        );
        app.shutdown_collection_runtime_for_exit();
    }

    #[test]
    #[cfg(windows)]
    fn metadata_import_pin_epoch_refreshes_all_context_presentations_without_rebinding_items() {
        let temp = tempfile::tempdir().unwrap();
        let video = temp.path().join("clip.mp4");
        std::fs::write(&video, b"video").unwrap();
        let (mut app, client) = start_ready_app(&temp.path().join("collection.db"));
        let pin_path = crate::video_pins::VideoPinDb::db_path();
        app.video_pin_db = Some(crate::video_pins::VideoPinDb::open_at(&pin_path).unwrap());
        app.video_pin_db
            .as_ref()
            .unwrap()
            .set_pin(&video, 1.0, b"old-webp")
            .unwrap();
        let snapshot =
            collection_with_sources(&client, &[(video.clone(), CollectionResolvedKind::Video)]);
        app.open_collection_grid(snapshot.collection_id(), None);
        wait_for_grid(&mut app, snapshot.collection_id());
        let installed = app
            .top_level_grid_view
            .collection_session()
            .and_then(CollectionGridSession::installed_presentation)
            .unwrap()
            .clone();
        assert_eq!(
            installed
                .sources
                .retained()
                .unwrap()
                .payload
                .video_pin_blobs
                .get(&video)
                .map(Vec::as_slice),
            Some(b"old-webp".as_slice())
        );
        let prepared = Arc::clone(&installed.prepared);
        let entry = prepared.entries[0].clone();
        let parked_items = app.items.clone();
        let parked_metas = app.image_metas.clone();
        let parked_video = video.clone();
        let parked = app.build_window_context_for_test(709, move |context| {
            context.top_level_grid_view.begin(
                TopLevelGridSurface::Collection(CollectionGridIdentity {
                    collection_id: prepared.collection_id,
                }),
                None,
            );
            context.items = parked_items;
            context.image_metas = parked_metas;
            context.fullscreen_idx = Some(0);
            let generation = context.items_generation;
            let session = context
                .top_level_grid_view
                .collection_session_mut()
                .unwrap();
            session.accepted_revision = prepared.collection_revision;
            session.wanted_revision = prepared.collection_revision;
            session.position = CollectionGridPosition::PhysicalSource {
                entry_id: entry.entry_id,
                source_key: entry.source_key,
                path: parked_video,
            };
            session.load = CollectionGridLoadState::Ready(Arc::clone(&installed));
            session.installed_items_generation = Some(generation);
            context.top_level_grid_view.set_collection_navigation_pending(Some(
                super::super::collection_navigation::CollectionNavigationPending::AwaitingOuterContinuation {
                    steps: 1,
                    fullscreen: true,
                    resume_slideshow: false,
                    native_toast: false,
                },
            ));
        });

        // The import writer is an independent connection.  Drop the App handle to
        // prove that committed-change observation does not depend on its stamp.
        app.video_pin_db = None;
        let conn = rusqlite::Connection::open(&pin_path).unwrap();
        conn.execute(
            "UPDATE video_pins SET thumb_webp = ?2, thumb_pts_secs = 2.0 WHERE path = ?1",
            rusqlite::params![
                crate::path_key::normalize_keep_drive(&video),
                b"new-webp".as_slice()
            ],
        )
        .unwrap();
        drop(conn);
        let root_generation = app.items_generation;
        let root_items = app.items.clone();
        app.fullscreen_idx = Some(0);
        let old_epoch = app.collection_thumbnail_source_epoch;
        let video_worker_cancel = Arc::new(AtomicBool::new(false));
        app.top_level_grid_view
            .collection_session_mut()
            .unwrap()
            .video_worker_cancel = Some(Arc::clone(&video_worker_cancel));

        app.advance_collection_thumbnail_source_epoch_for_metadata_import(0);
        assert_eq!(app.collection_thumbnail_source_epoch, old_epoch);
        assert!(matches!(
            app.top_level_grid_view.collection_session().unwrap().load,
            CollectionGridLoadState::Ready(_)
        ));
        assert!(!video_worker_cancel.load(Ordering::Acquire));

        app.advance_collection_thumbnail_source_epoch_for_metadata_import(1);

        assert_eq!(
            app.collection_thumbnail_source_epoch,
            old_epoch.wrapping_add(1)
        );
        let root_session = app.top_level_grid_view.collection_session().unwrap();
        assert!(matches!(
            root_session.load,
            CollectionGridLoadState::RequestNeeded {
                installed: Some(_),
                ..
            }
        ));
        assert_eq!(
            root_session.installed_items_generation,
            Some(root_generation)
        );
        assert_eq!(app.items, root_items);
        assert_eq!(app.fullscreen_idx, Some(0));
        assert!(video_worker_cancel.load(Ordering::Acquire));
        app.with_viewer_context(parked, |context| {
            let session = context.top_level_grid_view.collection_session().unwrap();
            assert!(matches!(
                session.position,
                CollectionGridPosition::PhysicalSource { .. }
            ));
            assert!(matches!(
                session.load,
                CollectionGridLoadState::RequestNeeded {
                    installed: Some(_),
                    ..
                }
            ));
            assert_eq!(
                session.installed_items_generation,
                Some(context.items_generation)
            );
            assert_eq!(context.fullscreen_idx, Some(0));
            assert!(context.top_level_grid_view.collection_navigation_pending());
            assert!(
                context
                    .top_level_grid_view
                    .collection_navigation_owns_fs_lock()
            );
        })
        .unwrap();

        app.fullscreen_idx = None;
        wait_for_grid(&mut app, snapshot.collection_id());
        let refreshed = app
            .top_level_grid_view
            .collection_session()
            .and_then(CollectionGridSession::installed_presentation)
            .unwrap();
        assert_eq!(refreshed.reuse_key.thumbnail_source_epoch, old_epoch + 1);
        assert_eq!(
            refreshed
                .sources
                .retained()
                .unwrap()
                .payload
                .video_pin_blobs
                .get(&video)
                .map(Vec::as_slice),
            Some(b"new-webp".as_slice())
        );
        app.shutdown_collection_runtime_for_exit();
    }

    #[test]
    fn prepare_result_from_a_previous_thumbnail_source_epoch_is_not_installed() {
        let temp = tempfile::tempdir().unwrap();
        let image = temp.path().join("image.png");
        std::fs::write(&image, b"image").unwrap();
        let (mut app, client) = start_ready_app(&temp.path().join("collection.db"));
        let snapshot = collection_with_sources(&client, &[(image, CollectionResolvedKind::Image)]);
        app.open_collection_grid(snapshot.collection_id(), None);
        wait_for_grid(&mut app, snapshot.collection_id());
        let installed = app
            .top_level_grid_view
            .collection_session()
            .and_then(|session| session.prepared().cloned())
            .unwrap();
        let old_reuse_key = app.collection_grid_prepare_reuse_key(
            installed.collection_id,
            installed.collection_revision,
        );
        let (sender, receiver) = std::sync::mpsc::channel();
        sender
            .send(Ok(CollectionGridPreparedInstall {
                prepared: (*installed).clone(),
                page_edits: None,
                retained_page_edits: None,
                page_edit_revision: app.page_edit_revision,
                thumbnail_sources: prepare_collection_grid_thumbnail_sources(
                    CollectionGridThumbnailSources::default(),
                    &AtomicBool::new(false),
                )
                .unwrap(),
                auto_aspect_lookup: None,
                reuse_key: old_reuse_key,
            }))
            .unwrap();
        app.collection_thumbnail_source_epoch =
            app.collection_thumbnail_source_epoch.wrapping_add(1);
        let stamp = app.collection_grid_stamp().unwrap();
        let generation = app.items_generation;
        let items = app.items.clone();
        app.top_level_grid_view
            .collection_session_mut()
            .unwrap()
            .load = CollectionGridLoadState::Preparing {
            stamp,
            exact_revision: installed.collection_revision,
            lease: crate::collection_store::CollectionReadLease::new(
                crate::collection_store::CollectionReadScope::app_global("grid-test"),
                Instant::now(),
                "prepare",
            ),
            installed: Some(Arc::clone(&installed)),
            cancel: Arc::new(AtomicBool::new(false)),
            receiver,
        };

        app.poll_collection_grid(&egui::Context::default());

        assert!(matches!(
            app.top_level_grid_view.collection_session().unwrap().load,
            CollectionGridLoadState::RequestNeeded {
                installed: Some(_),
                ..
            }
        ));
        assert_eq!(app.items_generation, generation);
        assert_eq!(app.items, items);
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
    fn root_read_waits_for_starting_and_busy_without_losing_its_watch_or_spinning() {
        let temp = tempfile::tempdir().unwrap();
        let (mut app, client) = start_ready_app(&temp.path().join("collection.db"));
        let snapshot = collection_with_sources(&client, &[]);
        let ctx = egui::Context::default();
        app.collection_ui.set_read_phase_for_test(true);
        app.open_collection_grid(snapshot.collection_id(), None);
        let first_deadline = match &app.top_level_grid_view.collection_session().unwrap().load {
            CollectionGridLoadState::RequestNeeded { lease, .. } => {
                lease.next_poll_at_for_test().unwrap()
            }
            _ => panic!("Starting must retain read demand"),
        };
        assert!(
            app.top_level_grid_view
                .collection_session()
                .unwrap()
                .watch
                .is_none()
        );
        assert!(app.collection_grid_poll_delay().unwrap() > Duration::ZERO);
        app.poll_collection_grid(&ctx);
        assert!(matches!(
            app.top_level_grid_view.collection_session().unwrap().load,
            CollectionGridLoadState::RequestNeeded { ref lease, .. }
                if lease.next_poll_at_for_test() == Some(first_deadline)
        ));

        app.collection_ui.set_read_phase_for_test(false);
        let (barrier_reply, entered, release) = client.test_barrier().unwrap();
        entered.recv_timeout(Duration::from_secs(3)).unwrap();
        let mut queued = Vec::new();
        loop {
            match client.list_catalog() {
                Ok(reply) => queued.push(reply),
                Err(CollectionStoreError::Busy) => break,
                other => panic!("unexpected actor admission: {other:?}"),
            }
        }
        if let CollectionGridLoadState::RequestNeeded { lease, .. } = &mut app
            .top_level_grid_view
            .collection_session_mut()
            .unwrap()
            .load
        {
            lease.force_due_for_test(Instant::now());
        }
        app.poll_collection_grid(&ctx);
        assert!(
            app.top_level_grid_view
                .collection_session()
                .unwrap()
                .watch
                .is_some()
        );
        let busy_deadline = match &app.top_level_grid_view.collection_session().unwrap().load {
            CollectionGridLoadState::RequestNeeded { lease, .. } => {
                lease.next_poll_at_for_test().unwrap()
            }
            _ => panic!("Busy must retain read demand"),
        };
        app.poll_collection_grid(&ctx);
        assert!(matches!(
            app.top_level_grid_view.collection_session().unwrap().load,
            CollectionGridLoadState::RequestNeeded { ref lease, .. }
                if lease.next_poll_at_for_test() == Some(busy_deadline)
        ));
        release.send(()).unwrap();
        barrier_reply
            .recv_timeout(Duration::from_secs(3))
            .unwrap()
            .unwrap();
        drop(queued);
        if let CollectionGridLoadState::RequestNeeded { lease, .. } = &mut app
            .top_level_grid_view
            .collection_session_mut()
            .unwrap()
            .load
        {
            lease.force_due_for_test(Instant::now());
        }
        wait_for_grid(&mut app, snapshot.collection_id());
        assert_eq!(app.collection_grid_poll_delay(), None);
        app.shutdown_collection_runtime_for_exit();
    }

    #[test]
    fn long_running_grid_reads_keep_installed_content_adopt_exact_replies_and_honor_cancel() {
        let temp = tempfile::tempdir().unwrap();
        let image = temp.path().join("page.png");
        std::fs::write(&image, b"image").unwrap();
        let (mut app, client) = start_ready_app(&temp.path().join("collection.db"));
        let snapshot = collection_with_sources(&client, &[(image, CollectionResolvedKind::Image)]);
        let collection_id = snapshot.collection_id();
        app.open_collection_grid(collection_id, None);
        wait_for_grid(&mut app, collection_id);
        let installed_prepared = app
            .top_level_grid_view
            .collection_session()
            .and_then(CollectionGridSession::prepared)
            .unwrap()
            .clone();
        let installed_generation = app.items_generation;
        let stamp = app.collection_grid_stamp().unwrap();
        let now = Instant::now();
        let started = now.checked_sub(Duration::from_secs(24 * 60 * 60)).unwrap();

        let (snapshot_sender, snapshot_receiver) = crossbeam_channel::bounded(1);
        let snapshot_lease = crate::collection_store::CollectionReadLease::new(
            crate::collection_store::CollectionReadScope::app_global("grid"),
            started,
            "snapshot",
        );
        let snapshot_request_id = snapshot_lease.request_id();
        app.top_level_grid_view
            .collection_session_mut()
            .unwrap()
            .load = CollectionGridLoadState::Snapshot {
            stamp,
            minimum_revision: installed_prepared.collection_revision,
            lease: snapshot_lease,
            queued_at: None,
            installed: Some(Arc::clone(&installed_prepared)),
            receiver: snapshot_receiver,
        };
        app.poll_collection_grid(&egui::Context::default());
        assert!(matches!(
            app.top_level_grid_view.collection_session().unwrap().load,
            CollectionGridLoadState::Snapshot { ref lease, ref installed, .. }
                if lease.request_id() == snapshot_request_id
                    && installed.as_ref().is_some_and(|value| Arc::ptr_eq(value, &installed_prepared))
        ));
        assert_eq!(app.items_generation, installed_generation);
        snapshot_sender.send(Ok(snapshot.clone())).unwrap();
        wait_for_grid(&mut app, collection_id);
        assert!(matches!(
            app.top_level_grid_view.collection_session().unwrap().load,
            CollectionGridLoadState::Ready(_) | CollectionGridLoadState::Empty(_)
        ));
        let generation_before_prepare = app.items_generation;

        let (prepare_sender, prepare_receiver) = std::sync::mpsc::channel();
        let prepared_sources = prepare_collection_grid_thumbnail_sources(
            CollectionGridThumbnailSources::default(),
            &AtomicBool::new(false),
        )
        .unwrap();
        let cancel = Arc::new(AtomicBool::new(false));
        let prepare_lease = crate::collection_store::CollectionReadLease::new(
            crate::collection_store::CollectionReadScope::app_global("grid"),
            started,
            "prepare",
        );
        let prepare_request_id = prepare_lease.request_id();
        app.top_level_grid_view
            .collection_session_mut()
            .unwrap()
            .load = CollectionGridLoadState::Preparing {
            stamp,
            exact_revision: installed_prepared.collection_revision,
            lease: prepare_lease,
            installed: Some(Arc::clone(&installed_prepared)),
            cancel: Arc::clone(&cancel),
            receiver: prepare_receiver,
        };
        app.poll_collection_grid(&egui::Context::default());
        assert!(!cancel.load(Ordering::Acquire));
        assert!(matches!(
            app.top_level_grid_view.collection_session().unwrap().load,
            CollectionGridLoadState::Preparing { ref lease, ref installed, .. }
                if lease.request_id() == prepare_request_id
                    && installed.as_ref().is_some_and(|value| Arc::ptr_eq(value, &installed_prepared))
        ));
        assert_eq!(app.items_generation, generation_before_prepare);

        prepare_sender
            .send(Ok(CollectionGridPreparedInstall {
                prepared: (*installed_prepared).clone(),
                page_edits: None,
                retained_page_edits: None,
                page_edit_revision: app.page_edit_revision,
                thumbnail_sources: prepared_sources,
                auto_aspect_lookup: None,
                reuse_key: app.collection_grid_prepare_reuse_key(
                    installed_prepared.collection_id,
                    installed_prepared.collection_revision,
                ),
            }))
            .unwrap();
        app.poll_collection_grid(&egui::Context::default());
        assert!(!cancel.load(Ordering::Acquire));
        assert!(matches!(
            app.top_level_grid_view.collection_session().unwrap().load,
            CollectionGridLoadState::Ready(_) | CollectionGridLoadState::Empty(_)
        ));

        let (late_sender, late_receiver) = std::sync::mpsc::channel();
        let late_cancel = Arc::new(AtomicBool::new(false));
        let late_lease = crate::collection_store::CollectionReadLease::new(
            crate::collection_store::CollectionReadScope::app_global("grid"),
            started,
            "prepare",
        );
        let generation_before_cancel = app.items_generation;
        app.top_level_grid_view
            .collection_session_mut()
            .unwrap()
            .load = CollectionGridLoadState::Preparing {
            stamp,
            exact_revision: installed_prepared.collection_revision,
            lease: late_lease,
            installed: Some(Arc::clone(&installed_prepared)),
            cancel: Arc::clone(&late_cancel),
            receiver: late_receiver,
        };
        late_sender
            .send(Ok(CollectionGridPreparedInstall {
                prepared: (*installed_prepared).clone(),
                page_edits: None,
                retained_page_edits: None,
                page_edit_revision: app.page_edit_revision,
                thumbnail_sources: prepare_collection_grid_thumbnail_sources(
                    CollectionGridThumbnailSources::default(),
                    &AtomicBool::new(false),
                )
                .unwrap(),
                auto_aspect_lookup: None,
                reuse_key: app.collection_grid_prepare_reuse_key(
                    installed_prepared.collection_id,
                    installed_prepared.collection_revision,
                ),
            }))
            .unwrap();
        app.top_level_grid_view
            .collection_session_mut()
            .unwrap()
            .cancel_pending();
        assert!(late_cancel.load(Ordering::Acquire));
        app.collection_ui.set_read_phase_for_test(true);
        app.poll_collection_grid(&egui::Context::default());
        assert_eq!(app.items_generation, generation_before_cancel);
        assert!(matches!(
            app.top_level_grid_view.collection_session().unwrap().load,
            CollectionGridLoadState::RequestNeeded { .. }
        ));
        app.shutdown_collection_runtime_for_exit();
    }

    #[test]
    #[ignore = "optimized 10,000-entry collection read pipeline benchmark"]
    fn benchmark_collection_actor_and_root_prepare_with_10000_entries() {
        const COUNT: usize = crate::collection_store::MAX_COLLECTION_ENTRIES;
        let temp = tempfile::tempdir().unwrap();
        let present_root = temp.path().join("present");
        let missing_root = temp.path().join("missing");
        std::fs::create_dir(&present_root).unwrap();
        let (mut app, client) = start_ready_app(&temp.path().join("collection.db"));

        let mut present = Vec::with_capacity(COUNT);
        let file_creation_started = Instant::now();
        for index in 0..COUNT {
            let path = present_root.join(format!("image-{index:05}.png"));
            std::fs::write(&path, []).unwrap();
            present.push(
                CollectionRegistration::from_trusted_path(&path, CollectionResolvedKind::Image)
                    .unwrap(),
            );
        }
        let file_creation_elapsed = file_creation_started.elapsed();

        let present_created = recv(client.create_collection("10k present".into()).unwrap());
        let actor_write_started = Instant::now();
        let present_snapshot = client
            .add_batch(
                present_created.collection_id(),
                present_created.revision(),
                present,
            )
            .unwrap()
            .recv_timeout(Duration::from_secs(120))
            .expect("10k actor reply")
            .expect("10k actor write")
            .snapshot;
        let actor_write_elapsed = actor_write_started.elapsed();

        let catalog_started = Instant::now();
        let catalog = client
            .list_catalog()
            .unwrap()
            .recv_timeout(Duration::from_secs(120))
            .expect("catalog reply")
            .expect("catalog read");
        let catalog_elapsed = catalog_started.elapsed();
        let snapshot_started = Instant::now();
        let reloaded_present = client
            .load_collection(present_snapshot.collection_id())
            .unwrap()
            .recv_timeout(Duration::from_secs(120))
            .expect("snapshot reply")
            .expect("snapshot read");
        let snapshot_elapsed = snapshot_started.elapsed();
        let prepare_present_started = Instant::now();
        let prepared_present = crate::collection_store::prepare_collection_snapshot(
            &reloaded_present,
            &crate::settings::GridDisplayOrder::default(),
            &AtomicBool::new(false),
            |_, _| {},
        )
        .expect("prepare present roots");
        let prepare_present_elapsed = prepare_present_started.elapsed();

        let mut missing = Vec::with_capacity(COUNT);
        for index in 0..COUNT {
            let path = missing_root.join(format!("image-{index:05}.png"));
            missing.push(
                CollectionRegistration::from_trusted_path(&path, CollectionResolvedKind::Image)
                    .unwrap(),
            );
        }
        let missing_created = recv(client.create_collection("10k missing".into()).unwrap());
        let missing_snapshot = client
            .add_batch(
                missing_created.collection_id(),
                missing_created.revision(),
                missing,
            )
            .unwrap()
            .recv_timeout(Duration::from_secs(120))
            .expect("10k missing actor reply")
            .expect("10k missing actor write")
            .snapshot;
        let prepare_missing_started = Instant::now();
        let prepared_missing = crate::collection_store::prepare_collection_snapshot(
            &missing_snapshot,
            &crate::settings::GridDisplayOrder::default(),
            &AtomicBool::new(false),
            |_, _| {},
        )
        .expect("prepare missing roots");
        let prepare_missing_elapsed = prepare_missing_started.elapsed();

        assert_eq!(catalog.definitions.len(), 1);
        assert_eq!(prepared_present.entries.len(), COUNT);
        assert_eq!(prepared_missing.entries.len(), COUNT);
        println!(
            "collection-10k file_create_ms={:.3} actor_write_ms={:.3} catalog_ms={:.3} snapshot_ms={:.3} prepare_present_ms={:.3} prepare_all_missing_ms={:.3}",
            file_creation_elapsed.as_secs_f64() * 1000.0,
            actor_write_elapsed.as_secs_f64() * 1000.0,
            catalog_elapsed.as_secs_f64() * 1000.0,
            snapshot_elapsed.as_secs_f64() * 1000.0,
            prepare_present_elapsed.as_secs_f64() * 1000.0,
            prepare_missing_elapsed.as_secs_f64() * 1000.0,
        );
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
            crate::rename_key_migration::journal_load(temp.path())
                .unwrap()
                .len(),
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
            crate::rename_key_migration::journal_load(temp.path())
                .unwrap()
                .is_empty(),
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
