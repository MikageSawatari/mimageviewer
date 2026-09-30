//! Context-owned collection Grid lifecycle.
//!
//! The collection actor supplies an immutable logical snapshot. Filesystem classification runs on
//! a separate worker, and only the exact viewer-context/surface/revision owner may install it.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use super::top_level_grid_view::{
    CollectionGridIdentity, CollectionGridInstalledPresentation,
    CollectionGridLiveThumbnailSources, CollectionGridLoadState, CollectionGridPhysicalLoadIntent,
    CollectionGridPhysicalLoadOrigin, CollectionGridPhysicalLoadOwner,
    CollectionGridPhysicalRestore, CollectionGridPosition, CollectionGridPrepareReuseKey,
    CollectionGridPreparedInstall, CollectionGridPreparedThumbnailDelivery,
    CollectionGridPreparedThumbnailSources, CollectionGridPresentationSources,
    CollectionGridRequestStamp, CollectionGridRestore, CollectionGridSession,
    CollectionGridSourceOpenOwner, CollectionGridThumbnailSourceIdentity,
    CollectionGridThumbnailSources, CollectionGridViewportAnchor, TopLevelGridRestore,
    TopLevelGridSurface,
};
use super::{App, CollectionMainContextChange, GridItem, GridSortLockReason, ViewerContextId};
use crate::collection_store::{
    CollectionEntryId, CollectionId, CollectionOrderMode, CollectionPrepareError,
    CollectionPreparedSnapshot, CollectionStoreError, prepare_collection_snapshot,
};

/// Offscreen Collection history preparation. The currently mounted surface and rows are owned
/// by the caller until `Ready` has been validated and explicitly adopted.
pub(crate) struct CollectionHistoryPrepare {
    target: CollectionGridRestore,
    minimum_revision: u64,
    client: crate::collection_store::CollectionStoreClient,
    watch: crate::collection_store::CollectionRevisionWatch,
    phase: CollectionHistoryPreparePhase,
}

enum CollectionHistoryPreparePhase {
    Snapshot(
        crossbeam_channel::Receiver<
            Result<crate::collection_store::CollectionSnapshot, CollectionStoreError>,
        >,
    ),
    Preparing {
        exact_revision: u64,
        cancel: Arc<AtomicBool>,
        receiver: std::sync::mpsc::Receiver<
            Result<CollectionGridPreparedInstall, CollectionPrepareError>,
        >,
    },
    Finished,
}

impl Drop for CollectionHistoryPrepare {
    fn drop(&mut self) {
        if let CollectionHistoryPreparePhase::Preparing { cancel, .. } = &self.phase {
            cancel.store(true, Ordering::Release);
        }
    }
}

pub(crate) enum CollectionHistoryPreparePoll {
    Pending,
    Ready(CollectionGridPreparedInstall),
    Failed(String),
}

#[derive(Clone, Debug)]
pub(crate) struct CollectionHistoryChildTarget {
    pub(crate) root: CollectionGridRestore,
    pub(crate) root_source_path: std::path::PathBuf,
    pub(crate) visible_path: std::path::PathBuf,
    pub(crate) kind: crate::collection_store::CollectionResolvedKind,
}

/// Resolve a saved child against the latest prepared entry. A stale entry or a path that no
/// longer belongs to that entry cannot silently fall through to an ordinary physical open.
pub(crate) fn resolve_collection_history_child(
    saved: &CollectionGridPhysicalRestore,
    install: &CollectionGridPreparedInstall,
) -> Option<CollectionHistoryChildTarget> {
    let prepared = &install.prepared;
    if prepared.collection_id != saved.root.identity.collection_id
        || prepared.collection_revision < saved.root.revision_at_open
    {
        return None;
    }
    let anchor = saved.root.viewport_anchor.as_ref()?;
    let entry = prepared
        .entries
        .iter()
        .find(|entry| entry.entry_id == anchor.entry_id)
        .or_else(|| {
            prepared
                .entries
                .iter()
                .find(|entry| entry.source_key == anchor.source_key)
        })?;
    let crate::collection_store::CollectionSourcePreparation::Available { kind, .. } =
        &entry.availability
    else {
        return None;
    };
    if !matches!(
        kind,
        crate::collection_store::CollectionResolvedKind::Folder
            | crate::collection_store::CollectionResolvedKind::Zip
            | crate::collection_store::CollectionResolvedKind::Pdf
            | crate::collection_store::CollectionResolvedKind::ConvertibleArchive
    ) || !crate::folder_tree::path_eq(&saved.root_source_path, &entry.source_path)
        || !saved
            .visible_path
            .ancestors()
            .any(|ancestor| crate::folder_tree::path_eq(ancestor, &entry.source_path))
    {
        return None;
    }
    Some(CollectionHistoryChildTarget {
        root: CollectionGridRestore {
            identity: saved.root.identity,
            revision_at_open: prepared.collection_revision,
            viewport_anchor: Some(CollectionGridViewportAnchor {
                entry_id: entry.entry_id,
                source_key: entry.source_key.clone(),
            }),
        },
        root_source_path: entry.source_path.clone(),
        visible_path: saved.visible_path.clone(),
        kind: *kind,
    })
}

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

    if sources.video_sidecars.is_empty()
        && sources.video_pin_blobs.is_empty()
        && sources.folder_pin_map.is_empty()
        && sources.video_folder_pin_seeds.is_empty()
    {
        return Ok(CollectionGridPreparedThumbnailDelivery::default());
    }
    let mut digest = sha2::Sha256::new();
    digest.update(b"miv.collection-thumbnail-sources.v2\0");

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

    let mut container_pins = sources.folder_pin_map.iter().collect::<Vec<_>>();
    container_pins.sort_unstable_by_key(|(key, _)| *key);
    digest.update((container_pins.len() as u64).to_le_bytes());
    for (key, source) in container_pins {
        if cancel.load(Ordering::Acquire) {
            return Err(CollectionPrepareError::Cancelled);
        }
        digest.update(b"container-pin\0");
        hash_collection_thumbnail_identity_part(&mut digest, key.as_bytes());
        hash_collection_thumbnail_identity_part(&mut digest, source.db_kind().as_bytes());
        hash_collection_thumbnail_identity_part(&mut digest, source.rel().as_bytes());
        use crate::folder_thumb_pins::FolderPinSource;
        match source {
            FolderPinSource::ZipEntry { entry, .. } => {
                hash_collection_thumbnail_identity_part(&mut digest, entry.as_bytes())
            }
            FolderPinSource::ZipDir { dir_prefix, .. } => {
                hash_collection_thumbnail_identity_part(&mut digest, dir_prefix.as_bytes())
            }
            FolderPinSource::PdfPage { page, .. } => digest.update(page.to_le_bytes()),
            FolderPinSource::File { .. } => {}
        }
    }
    digest.update((sources.video_folder_pin_seeds.len() as u64).to_le_bytes());
    for (seed, webp) in sources.video_folder_pin_seeds.iter() {
        if cancel.load(Ordering::Acquire) {
            return Err(CollectionPrepareError::Cancelled);
        }
        digest.update(b"container-video-seed\0");
        hash_collection_thumbnail_identity_part(&mut digest, seed.cache_key.as_bytes());
        hash_collection_thumbnail_identity_part(
            &mut digest,
            crate::path_key::normalize_keep_drive(&seed.video_path).as_bytes(),
        );
        digest.update(seed.mtime.to_le_bytes());
        digest.update(seed.file_size.to_le_bytes());
        match webp {
            Some(webp) => {
                digest.update([1]);
                digest.update(sha2::Sha256::digest(webp));
            }
            None => digest.update([0]),
        }
    }
    let identity = CollectionGridThumbnailSourceIdentity(digest.finalize().into());
    let presentation = if super::top_level_grid_view::collection_pin_blob_sizes_fit_retention_budget(
        sources.video_pin_blobs.values().map(Vec::len).chain(
            sources
                .video_folder_pin_seeds
                .iter()
                .filter_map(|(_, webp)| webp.as_ref().map(Vec::len)),
        ),
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
        live: prepare_collection_grid_live_thumbnail_sources(sources, cancel)?,
    })
}

/// Both cold prepare and retained-source preflight build the install map off the UI thread.
pub(in crate::app) fn prepare_collection_grid_live_thumbnail_sources(
    mut sources: CollectionGridThumbnailSources,
    cancel: &AtomicBool,
) -> Result<CollectionGridLiveThumbnailSources, CollectionPrepareError> {
    let mut folder_pin_cache = std::collections::HashMap::new();
    for (seed, webp) in sources.video_folder_pin_seeds.iter() {
        if cancel.load(Ordering::Acquire) {
            return Err(CollectionPrepareError::Cancelled);
        }
        let Some(webp) = webp else { continue };
        folder_pin_cache.insert(
            seed.cache_key.clone(),
            crate::catalog::CacheEntry {
                mtime: seed.mtime,
                file_size: seed.file_size,
                jpeg_data: webp.clone(),
                source_dims: None,
                layout_dims: None,
                folder_provenance: Some(crate::catalog::FolderThumbProvenance::Seeded),
                selection_proof: None,
            },
        );
    }
    // The UI installs only the derived map. Drop the live raw seeds on this worker; retained
    // presentations keep their own Arc, while oversized blobs must not be freed on the UI.
    sources.video_folder_pin_seeds = Arc::new(Vec::new());
    Ok(CollectionGridLiveThumbnailSources {
        sources,
        folder_pin_cache,
    })
}

pub(in crate::app) fn prepare_collection_grid_install(
    snapshot: &crate::collection_store::CollectionSnapshot,
    display_order: &crate::settings::GridDisplayOrder,
    settings: &crate::settings::Settings,
    cancel: &AtomicBool,
    auto_aspect_client: Option<&crate::auto_aspect_cache::CollectionAutoAspectCacheClient>,
    pin_stamp: Option<crate::video_pins::VideoPinMutationStamp>,
    folder_pin_stamp: Option<crate::folder_thumb_pins::FolderPinMutationStamp>,
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
    let page_edits = super::page_edit_snapshot::PageEditSnapshot::load_and_project_stable(
        &edit_items,
        page_edit_availability,
        cancel,
    )
    .map_err(CollectionPrepareError::Io)?
    .ok_or(CollectionPrepareError::Cancelled)?;
    let page_edits = (page_edits.snapshot, page_edits.projection);
    let retained_page_edits = Some(Arc::new(page_edits.0.clone()));
    let pin_start = crate::perf::is_enabled().then(Instant::now);
    let pin_db =
        crate::video_pins::VideoPinDb::open_readonly(&crate::video_pins::VideoPinDb::db_path());
    let folder_pin_db = crate::folder_thumb_pins::FolderThumbPinDb::open_readonly(
        &crate::folder_thumb_pins::FolderThumbPinDb::db_path(),
    )
    .ok();
    let folder_pin_map = folder_pin_db
        .as_ref()
        .map(|db| db.lookup_many(edit_items.iter().filter_map(GridItem::container_path)))
        .unwrap_or_default();
    let mut video_folder_pin_seeds = super::smart_folder::prepare_video_folder_pin_seeds(
        &edit_items,
        &folder_pin_map,
        folder_pin_db.as_ref(),
        settings.folder_thumb_sort,
        settings.folder_thumb_depth,
        pin_db.as_ref().ok(),
        cancel,
    );
    // Match the aggregate catalog writer: invalid WebP must never become a cache hit.
    for (_, webp) in &mut video_folder_pin_seeds {
        if webp
            .as_ref()
            .is_some_and(|bytes| crate::catalog::decode_thumb_dims(bytes).is_none())
        {
            *webp = None;
        }
    }
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
            folder_pin_map,
            video_folder_pin_seeds: Arc::new(video_folder_pin_seeds),
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
            folder_pin_stamp,
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

    /// Import and content restore write through separate SQLite connections, so
    /// the UI-owned pin DB mutation stamps cannot observe the commit.  Move
    /// every context to a presentation-only retry and bind future preparation to
    /// this app-global epoch.  Item bindings, root/child position and navigation
    /// ownership stay intact.
    pub(crate) fn advance_collection_thumbnail_source_epoch_for_pin_commit(
        &mut self,
        committed_thumbnail_pin_changes: usize,
        source: &str,
    ) {
        if committed_thumbnail_pin_changes == 0 {
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
            "{source}: collection thumbnail sources invalidated changes={} epoch={} contexts={invalidated_contexts}",
            committed_thumbnail_pin_changes, self.collection_thumbnail_source_epoch,
        ));
        if crate::perf::is_enabled() {
            crate::perf::event(
                source,
                "collection_thumbnail_sources_invalidated",
                None,
                0,
                &[
                    (
                        "changes",
                        serde_json::Value::from(committed_thumbnail_pin_changes as u64),
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
        let viewport_anchor = match &session.position {
            CollectionGridPosition::Root => session
                .installed_items_generation
                .filter(|generation| *generation == self.items_generation)
                .and_then(|_| self.selected)
                .and_then(|index| session.prepared()?.entries.get(index))
                .map(|entry| CollectionGridViewportAnchor {
                    entry_id: entry.entry_id,
                    source_key: entry.source_key.clone(),
                })
                .or_else(|| session.restore_anchor.clone()),
            CollectionGridPosition::PhysicalSource {
                entry_id,
                source_key,
                ..
            } => Some(CollectionGridViewportAnchor {
                entry_id: *entry_id,
                source_key: source_key.clone(),
            }),
        };
        Some(TopLevelGridRestore::Collection(CollectionGridRestore {
            identity: session.identity,
            revision_at_open: session.accepted_revision.max(session.wanted_revision),
            viewport_anchor,
        }))
    }

    /// The visible location is distinct from its Collection root return. A deleted definition
    /// leaves the physical rows visible, but that path is now an ordinary physical location.
    pub(crate) fn collection_grid_current_restore_snapshot(&self) -> Option<TopLevelGridRestore> {
        let session = self.top_level_grid_view.collection_session()?;
        let CollectionGridPosition::PhysicalSource { path, .. } = &session.position else {
            return self.collection_grid_restore_snapshot();
        };
        let visible_path = self.effective_folder()?;
        if matches!(session.load, CollectionGridLoadState::Deleted { .. }) {
            return Some(TopLevelGridRestore::Folder(visible_path));
        }
        let TopLevelGridRestore::Collection(root) = self.collection_grid_restore_snapshot()? else {
            return None;
        };
        Some(TopLevelGridRestore::CollectionPhysical(
            CollectionGridPhysicalRestore {
                root,
                root_source_path: path.clone(),
                visible_path,
            },
        ))
    }

    pub(crate) fn collection_grid_parent_nav(&self) -> Option<crate::ui_main::AddressBarNav> {
        let session = self.top_level_grid_view.collection_session()?;
        if matches!(session.load, CollectionGridLoadState::Deleted { .. }) {
            return None;
        }
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
        if matches!(session.load, CollectionGridLoadState::Deleted { .. }) {
            return None;
        }
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
        if matches!(session.load, CollectionGridLoadState::Deleted { .. }) {
            return None;
        }
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
                    intent: CollectionGridPhysicalLoadIntent::Explicit,
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
                intent: CollectionGridPhysicalLoadIntent::Explicit,
            }),
        }
    }

    pub(crate) fn collection_grid_playback_physical_load_owner(
        &self,
        index: usize,
        target_path: &std::path::Path,
    ) -> Option<CollectionGridPhysicalLoadOwner> {
        let mut owner = self.collection_grid_physical_load_owner(index, target_path)?;
        owner.intent = CollectionGridPhysicalLoadIntent::Playback;
        Some(owner)
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
        auto_fullscreen: bool,
    ) -> crate::ui_main::AddressBarNav {
        let virtual_container = self.items.get(index).is_some_and(|item| match item {
            GridItem::ZipFile(item_path) | GridItem::PdfFile(item_path) => {
                crate::folder_tree::path_eq(item_path, &path)
            }
            _ => false,
        });
        if virtual_container {
            let source = match self.collection_grid_physical_load_owner(index, &path) {
                Some(owner) => super::GridVirtualOpenSource::Collection(owner),
                None => match self.rating_view_physical_load_owner(&path) {
                    Some(owner) => super::GridVirtualOpenSource::Rating(owner),
                    None => super::GridVirtualOpenSource::Direct,
                },
            };
            return crate::ui_main::AddressBarNav::GridVirtual(super::GridVirtualOpenIntent {
                path: path.clone(),
                source,
                effects: super::GridVirtualOpenEffects {
                    reading_history_return_from: self
                        .items_are_reading_history_view
                        .then_some(path),
                    suppress_rating_filter: true,
                    // Grid activation only exposes rows accepted by the current facet filter.
                    suppress_facet_filter: self.facet_filter_active(),
                    auto_fullscreen,
                },
            });
        }
        match self.collection_grid_physical_load_owner(index, &path) {
            Some(owner) => crate::ui_main::AddressBarNav::CollectionSource { path, owner },
            None => match self.rating_view_physical_load_owner(&path) {
                Some(owner) => crate::ui_main::AddressBarNav::RatingSource { path, owner },
                None => crate::ui_main::AddressBarNav::Direct(path),
            },
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
        if matches!(session.load, CollectionGridLoadState::Deleted { .. }) {
            return false;
        }
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
                    && (crate::folder_tree::path_eq(&owner.root_source_path, target_path)
                        || (owner
                            .root_source_path
                            .extension()
                            .is_some_and(|ext| ext.eq_ignore_ascii_case("epub"))
                            && crate::folder_tree::path_eq(
                                &owner.root_source_path.with_extension("pdf"),
                                target_path,
                            )))
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
        let root = owner.restore(
            self.top_level_grid_view
                .collection_session()
                .map_or(owner.wanted_revision, |session| session.wanted_revision),
        );
        let from = match &owner.origin {
            CollectionGridPhysicalLoadOrigin::Root { .. } => {
                Some(TopLevelGridRestore::Collection(root.clone()))
            }
            CollectionGridPhysicalLoadOrigin::PhysicalSource { current_path }
                if !crate::folder_tree::path_eq(current_path, target_path) =>
            {
                Some(TopLevelGridRestore::CollectionPhysical(
                    CollectionGridPhysicalRestore {
                        root: root.clone(),
                        root_source_path: owner.root_source_path.clone(),
                        visible_path: current_path.clone(),
                    },
                ))
            }
            CollectionGridPhysicalLoadOrigin::PhysicalSource { .. } => None,
        };
        if let Some(from) =
            from.filter(|_| owner.intent == CollectionGridPhysicalLoadIntent::Explicit)
        {
            self.record_collection_physical_nav_transition(
                from,
                CollectionGridPhysicalRestore {
                    root,
                    root_source_path: owner.root_source_path.clone(),
                    visible_path: target_path.to_path_buf(),
                },
            );
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
        if matches!(session.load, CollectionGridLoadState::Deleted { .. }) {
            return false;
        }
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
                        && presentation
                            .page_edit_snapshot
                            .as_ref()
                            .and_then(|snapshot| snapshot.stamp)
                            .is_some_and(|stamp| {
                                crate::page_edit_write_epoch::PAGE_EDIT_WRITES.accepts(stamp)
                            })
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
                        .filter(|_| {
                            source_owner.page_edit_revision == self.page_edit_revision
                                && source_owner.page_edit_stamp_is_current()
                        })
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
        if owner.navigation_request.is_none() {
            let root = CollectionGridRestore {
                identity: CollectionGridIdentity {
                    collection_id: owner.stamp.collection_id,
                },
                revision_at_open: owner.accepted_revision.max(owner.wanted_revision),
                viewport_anchor: Some(owner.anchor.clone()),
            };
            self.record_collection_physical_nav_transition(
                TopLevelGridRestore::Collection(root.clone()),
                CollectionGridPhysicalRestore {
                    root,
                    root_source_path: path.clone(),
                    visible_path: path.clone(),
                },
            );
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

    fn retire_transient_views_for_collection(&mut self) {
        let _ = self.dismiss_snapshot_without_restore();
        if self.favsearch.active {
            self.dismiss_favsearch_without_restore();
        }
        if self.global_search.active {
            self.dismiss_global_search_without_restore();
        }
        if self.tag_view.active {
            self.dismiss_tag_view_without_restore();
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
    }

    pub(crate) fn collection_root_installed_binding_matches(
        &self,
        collection_id: CollectionId,
    ) -> bool {
        let Some(index) = self.fullscreen_idx else {
            return false;
        };
        let Some(stamp) = self.collection_grid_stamp() else {
            return false;
        };
        let Some(session) = self.top_level_grid_view.collection_session() else {
            return false;
        };
        let Some(prepared) = session.prepared() else {
            return false;
        };
        stamp.collection_id == collection_id
            && matches!(session.position, CollectionGridPosition::Root)
            && session.installed_items_generation == Some(self.items_generation)
            && prepared.collection_id == stamp.collection_id
            && prepared.collection_revision == session.accepted_revision
            && prepared.entries.len() == self.items.len()
            && index < self.items.len()
    }

    pub(crate) fn collection_open_return_to(
        &self,
        collection_id: CollectionId,
        restoring: bool,
    ) -> Option<TopLevelGridRestore> {
        if restoring {
            None
        } else if matches!(
            self.top_level_grid_view.surface(),
            TopLevelGridSurface::Collection(identity) if identity.collection_id == collection_id
        ) {
            self.top_level_grid_view.return_to().cloned()
        } else {
            // Simulate the retirement order without mutation. The first active transient consumes
            // the canonical return_to, so a later one must use its own fallback.
            let mut origin = self.snapshot_return_context_without_restore();
            let mut canonical_return_to = self
                .snapshot
                .is_none()
                .then(|| self.top_level_grid_view.return_to())
                .flatten();
            if self.favsearch.active {
                origin = Some(self.favsearch_return_context_without_restore(canonical_return_to));
                canonical_return_to = None;
            }
            if self.global_search.active {
                origin =
                    Some(self.global_search_return_context_without_restore(canonical_return_to));
                canonical_return_to = None;
            }
            if self.tag_view.active {
                origin = Some(self.tag_view_return_context_without_restore(canonical_return_to));
            }
            origin.or_else(|| self.current_top_level_restore_snapshot())
        }
    }

    /// Opens the collection root. `restore` carries a minimum revision hint and a stable entry
    /// anchor; neither value is treated as an exact actor reply revision.
    pub(crate) fn open_collection_grid(
        &mut self,
        collection_id: CollectionId,
        restore: Option<CollectionGridRestore>,
    ) {
        let return_to = self.collection_open_return_to(collection_id, restore.is_some());
        if let CollectionMainContextChange::Blocked(reason) =
            self.prepare_collection_main_context_change(collection_id)
        {
            crate::logger::log(format!("collection open blocked: {reason}"));
            return;
        }
        self.open_collection_grid_after_context_change(collection_id, restore, return_to);
    }

    pub(crate) fn open_collection_grid_after_context_change(
        &mut self,
        collection_id: CollectionId,
        restore: Option<CollectionGridRestore>,
        return_to: Option<TopLevelGridRestore>,
    ) {
        let perf_start = crate::perf::is_enabled().then(Instant::now);
        let restoring = restore.is_some();
        let retaining_binding = self.collection_root_installed_binding_matches(collection_id);
        if !retaining_binding
            && !restoring
            && !matches!(
                self.top_level_grid_view.surface(),
                TopLevelGridSurface::Collection(identity) if identity.collection_id == collection_id
            )
        {
            self.retire_transient_views_for_collection();
        }
        let identity = CollectionGridIdentity { collection_id };
        if !retaining_binding {
            self.top_level_grid_view
                .begin(TopLevelGridSurface::Collection(identity), return_to);
        }
        let watch = self
            .collection_store_client_for_read()
            .ok()
            .flatten()
            .and_then(|client| client.subscribe().ok());
        if let Some(session) = self.top_level_grid_view.collection_session_mut() {
            if retaining_binding {
                session.cancel_pending();
            }
            session.wanted_revision = restore
                .as_ref()
                .map_or(0, |state| state.revision_at_open)
                .max(session.accepted_revision);
            session.restore_anchor = restore.and_then(|state| state.viewport_anchor);
            session.watch = watch;
        }

        // Do not leave a prior physical/search grid interactive while the actor and classifier are
        // resolving this collection. The empty install performs no filesystem/database access.
        if !retaining_binding {
            let collection_seed = self
                .collection_auto_aspect_cache
                .as_ref()
                .and_then(|cache| cache.cached(collection_id));
            self.install_collection_grid_items(Vec::new(), Vec::new(), None, collection_seed);
        }
        // A header choice belongs to the prior root. A collection open, including history and
        // same-root reopen, starts from its installed collection order.
        if !retaining_binding {
            self.reset_details_sort_to_toolbar();
            self.address = "コレクションを読み込み中…".into();
        }
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
        if self.document_open_modal_admission_blocked() {
            return;
        }
        let source_location = self.collection_nav_history_source();
        let return_to = self.collection_open_return_to(collection_id, false);
        if let CollectionMainContextChange::Blocked(reason) =
            self.prepare_collection_main_context_change(collection_id)
        {
            crate::logger::log(format!("collection navigation blocked: {reason}"));
            return;
        }
        let revision_at_open = self.collection_catalog_revision(collection_id).unwrap_or(0);
        let restore = CollectionGridRestore {
            identity: CollectionGridIdentity { collection_id },
            revision_at_open,
            viewport_anchor: None,
        };
        self.record_collection_nav_transition(restore, source_location);
        self.open_collection_grid_after_context_change(collection_id, None, return_to);
    }

    pub(crate) fn start_collection_history_prepare(
        &self,
        target: CollectionGridRestore,
    ) -> Result<CollectionHistoryPrepare, CollectionStoreError> {
        let client = self
            .collection_store_client_for_read()?
            .ok_or(CollectionStoreError::Unavailable)?;
        let watch = client.subscribe()?;
        let receiver = client.load_collection(target.identity.collection_id)?;
        Ok(CollectionHistoryPrepare {
            minimum_revision: target.revision_at_open,
            target,
            client,
            watch,
            phase: CollectionHistoryPreparePhase::Snapshot(receiver),
        })
    }

    /// Polls the actor and worker without publishing a Collection surface or modifying rows.
    /// A revision notice restarts the offscreen read, so Ready always reflects a current catalog.
    pub(crate) fn poll_collection_history_prepare(
        &self,
        request: &mut CollectionHistoryPrepare,
    ) -> CollectionHistoryPreparePoll {
        if !self.collection_catalog_contains(request.target.identity.collection_id) {
            return CollectionHistoryPreparePoll::Failed("コレクションは削除されました".into());
        }
        if let Some(notice) = request.watch.take_latest() {
            let Some((_, revision)) = notice
                .collection_revisions
                .iter()
                .find(|(id, _)| *id == request.target.identity.collection_id)
            else {
                return CollectionHistoryPreparePoll::Failed("コレクションは削除されました".into());
            };
            request.minimum_revision = request.minimum_revision.max(*revision);
        }
        let phase = std::mem::replace(&mut request.phase, CollectionHistoryPreparePhase::Finished);
        match phase {
            CollectionHistoryPreparePhase::Snapshot(receiver) => match receiver.try_recv() {
                Ok(Ok(snapshot)) => {
                    if snapshot.collection_id() != request.target.identity.collection_id
                        || snapshot.revision() < request.minimum_revision
                    {
                        return self.restart_collection_history_snapshot(request);
                    }
                    let exact_revision = snapshot.revision();
                    let display_order = self.settings.grid_display_order.clone();
                    let settings = self.settings.clone();
                    let pin_stamp = self.video_pin_db.as_ref().map(|db| db.mutation_stamp());
                    let folder_pin_stamp = self
                        .folder_thumb_pin_db
                        .as_ref()
                        .map(|db| db.mutation_stamp());
                    let thumbnail_source_epoch = self.collection_thumbnail_source_epoch;
                    let page_edit_availability =
                        super::page_edit_snapshot::PageEditAvailability::for_app(self);
                    let page_edit_revision = self.page_edit_revision;
                    let auto_aspect_client = self
                        .collection_auto_aspect_cache
                        .as_ref()
                        .map(|cache| cache.client());
                    let cancel = Arc::new(AtomicBool::new(false));
                    let worker_cancel = Arc::clone(&cancel);
                    let (sender, receiver) = std::sync::mpsc::sync_channel(1);
                    #[cfg(test)]
                    let test_epoch_scope = crate::page_edit_write_epoch::TestEpochScope::capture();
                    let spawn = std::thread::Builder::new()
                        .name("collection-history-prepare".into())
                        .spawn(move || {
                            #[cfg(test)]
                            let _test_epoch_scope = test_epoch_scope
                                .map(crate::page_edit_write_epoch::TestEpochScope::enter);
                            let result = prepare_collection_grid_install(
                                &snapshot,
                                &display_order,
                                &settings,
                                &worker_cancel,
                                auto_aspect_client.as_ref(),
                                pin_stamp,
                                folder_pin_stamp,
                                thumbnail_source_epoch,
                                page_edit_availability,
                                page_edit_revision,
                            );
                            let _ = sender.send(result);
                        });
                    match spawn {
                        Ok(_) => {
                            request.phase = CollectionHistoryPreparePhase::Preparing {
                                exact_revision,
                                cancel,
                                receiver,
                            };
                            CollectionHistoryPreparePoll::Pending
                        }
                        Err(error) => CollectionHistoryPreparePoll::Failed(format!(
                            "コレクション一覧を準備できません: {error}"
                        )),
                    }
                }
                Ok(Err(error)) if error.is_read_retryable() => {
                    self.restart_collection_history_snapshot(request)
                }
                Ok(Err(error)) => {
                    CollectionHistoryPreparePoll::Failed(collection_grid_error(&error))
                }
                Err(crossbeam_channel::TryRecvError::Empty) => {
                    request.phase = CollectionHistoryPreparePhase::Snapshot(receiver);
                    CollectionHistoryPreparePoll::Pending
                }
                Err(crossbeam_channel::TryRecvError::Disconnected) => {
                    CollectionHistoryPreparePoll::Failed(
                        "コレクション一覧の応答が失われました".into(),
                    )
                }
            },
            CollectionHistoryPreparePhase::Preparing {
                exact_revision,
                cancel,
                receiver,
            } => match receiver.try_recv() {
                Ok(Ok(prepared)) => {
                    if exact_revision < request.minimum_revision
                        || prepared.prepared.collection_id != request.target.identity.collection_id
                        || prepared.prepared.collection_revision != exact_revision
                        || prepared.page_edit_revision != self.page_edit_revision
                        || prepared.reuse_key
                            != self.collection_grid_prepare_reuse_key(
                                request.target.identity.collection_id,
                                exact_revision,
                            )
                    {
                        return self.restart_collection_history_snapshot(request);
                    }
                    CollectionHistoryPreparePoll::Ready(prepared)
                }
                Ok(Err(error)) => CollectionHistoryPreparePoll::Failed(error.to_string()),
                Err(std::sync::mpsc::TryRecvError::Empty) => {
                    request.phase = CollectionHistoryPreparePhase::Preparing {
                        exact_revision,
                        cancel,
                        receiver,
                    };
                    CollectionHistoryPreparePoll::Pending
                }
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    CollectionHistoryPreparePoll::Failed(
                        "コレクション一覧の準備結果が失われました".into(),
                    )
                }
            },
            CollectionHistoryPreparePhase::Finished => CollectionHistoryPreparePoll::Failed(
                "コレクション一覧の準備は終了しています".into(),
            ),
        }
    }

    fn restart_collection_history_snapshot(
        &self,
        request: &mut CollectionHistoryPrepare,
    ) -> CollectionHistoryPreparePoll {
        match request
            .client
            .load_collection(request.target.identity.collection_id)
        {
            Ok(receiver) => {
                request.phase = CollectionHistoryPreparePhase::Snapshot(receiver);
                CollectionHistoryPreparePoll::Pending
            }
            Err(error) => CollectionHistoryPreparePoll::Failed(collection_grid_error(&error)),
        }
    }

    /// Called only after the history transition has confirmed that this context still owns its
    /// source display and that every target preflight step succeeded.
    pub(crate) fn adopt_collection_history_root(
        &mut self,
        restore: CollectionGridRestore,
        prepared: CollectionGridPreparedInstall,
        return_to: Option<TopLevelGridRestore>,
    ) -> bool {
        let identity = restore.identity;
        let retaining_binding =
            self.collection_root_installed_binding_matches(identity.collection_id);
        if let CollectionMainContextChange::Blocked(reason) =
            self.prepare_collection_main_context_change(identity.collection_id)
        {
            crate::logger::log(format!("collection history adoption blocked: {reason}"));
            return false;
        }
        let watch = self
            .collection_store_client_for_read()
            .ok()
            .flatten()
            .and_then(|client| client.subscribe().ok());
        if !retaining_binding {
            self.top_level_grid_view
                .begin(TopLevelGridSurface::Collection(identity), return_to);
        }
        if let Some(session) = self.top_level_grid_view.collection_session_mut() {
            if retaining_binding {
                session.cancel_pending();
            }
            session.wanted_revision = restore.revision_at_open.max(session.accepted_revision);
            session.restore_anchor = restore.viewport_anchor;
            session.watch = watch;
        }
        if retaining_binding {
            self.schedule_collection_grid_snapshot();
        } else {
            self.reset_details_sort_to_toolbar();
            self.apply_collection_grid_prepared_install(prepared, None);
        }
        true
    }

    /// Final child replay commit step. This mounts the latest Collection identity and BS anchor
    /// without publishing root rows. The caller must already hold a fully preflighted child
    /// listing and install it synchronously in this same adoption boundary.
    pub(crate) fn mount_collection_history_child_session(
        &mut self,
        target: &CollectionHistoryChildTarget,
        install: CollectionGridPreparedInstall,
        return_to: Option<TopLevelGridRestore>,
    ) -> bool {
        if !self.collection_catalog_contains(target.root.identity.collection_id)
            || install.prepared.collection_id != target.root.identity.collection_id
            || install.prepared.collection_revision < target.root.revision_at_open
            || install.page_edit_revision != self.page_edit_revision
            || install.reuse_key
                != self.collection_grid_prepare_reuse_key(
                    target.root.identity.collection_id,
                    install.prepared.collection_revision,
                )
        {
            return false;
        }
        let Some(anchor) = target.root.viewport_anchor.clone() else {
            return false;
        };
        let Some(watch) = self
            .collection_store_client_for_read()
            .ok()
            .flatten()
            .and_then(|client| client.subscribe().ok())
        else {
            return false;
        };
        // Child preflight may take longer than the root snapshot/prepare. A fresh subscription
        // includes the actor's latest published revision, closing that gap before any surface
        // or row mutation. Carry this same watch into the adopted session.
        if self
            .collection_catalog_revision(target.root.identity.collection_id)
            .is_some_and(|revision| revision > install.prepared.collection_revision)
            || watch.take_latest().is_some_and(|notice| {
                notice
                    .collection_revisions
                    .iter()
                    .find(|(id, _)| *id == target.root.identity.collection_id)
                    .is_none_or(|(_, revision)| *revision != install.prepared.collection_revision)
            })
        {
            return false;
        }
        let CollectionGridPreparedInstall {
            prepared,
            retained_page_edits,
            page_edit_revision,
            thumbnail_sources,
            reuse_key,
            ..
        } = install;
        let revision = prepared.collection_revision;
        let mut presentation = CollectionGridInstalledPresentation::new(
            Arc::new(prepared),
            thumbnail_sources.presentation,
            reuse_key,
        );
        presentation.page_edit_snapshot = retained_page_edits;
        presentation.page_edit_revision = page_edit_revision;
        let presentation = Arc::new(presentation);
        self.top_level_grid_view.begin(
            TopLevelGridSurface::Collection(target.root.identity),
            return_to,
        );
        if let Some(session) = self.top_level_grid_view.collection_session_mut() {
            session.position = CollectionGridPosition::PhysicalSource {
                entry_id: anchor.entry_id,
                source_key: anchor.source_key.clone(),
                path: target.root_source_path.clone(),
            };
            session.restore_anchor = Some(anchor);
            session.accepted_revision = revision;
            session.wanted_revision = revision;
            session.watch = Some(watch);
            session.installed_items_generation = None;
            session.load = if presentation.prepared.entries.is_empty() {
                CollectionGridLoadState::Empty(presentation)
            } else {
                CollectionGridLoadState::Ready(presentation)
            };
        }
        true
    }

    /// Commit a fully prepared Collection child replay at one visible boundary. A rejected
    /// preflight leaves the old session and rows intact; successful payloads install their rows
    /// before the caller commits the history stack.
    pub(crate) fn adopt_collection_history_child(
        &mut self,
        target: &CollectionHistoryChildTarget,
        prepared: CollectionGridPreparedInstall,
        payload: super::collection_navigation::PhysicalHistoryPreflightPayload,
        return_to: Option<TopLevelGridRestore>,
    ) -> bool {
        self.adopt_collection_history_child_with_backing(
            target, prepared, payload, return_to, None, None,
        )
    }

    /// `backing_path` is the already enumerated direct RAR or converted-cache ZIP. The visible
    /// location remains `target.visible_path`; the backing path is only an implementation alias.
    pub(crate) fn adopt_collection_history_child_with_backing(
        &mut self,
        target: &CollectionHistoryChildTarget,
        prepared: CollectionGridPreparedInstall,
        payload: super::collection_navigation::PhysicalHistoryPreflightPayload,
        return_to: Option<TopLevelGridRestore>,
        backing_path: Option<std::path::PathBuf>,
        pdf_password_override: Option<String>,
    ) -> bool {
        use super::collection_navigation::PhysicalHistoryPreflightPayload;

        if self.sidecar_restore_active()
            || !matches!(
                &payload,
                PhysicalHistoryPreflightPayload::Folder(_)
                    | PhysicalHistoryPreflightPayload::Zip(_)
                    | PhysicalHistoryPreflightPayload::ZipCached { .. }
                    | PhysicalHistoryPreflightPayload::PdfPages(_)
            )
            || backing_path.is_some()
                && !matches!(&payload, PhysicalHistoryPreflightPayload::Zip(_))
            || pdf_password_override.is_some()
                && !matches!(&payload, PhysicalHistoryPreflightPayload::PdfPages(_))
            || !target
                .visible_path
                .ancestors()
                .any(|ancestor| crate::folder_tree::path_eq(ancestor, &target.root_source_path))
        {
            return false;
        }
        if matches!(&payload, PhysicalHistoryPreflightPayload::PdfPages(_))
            && self.pdf_open_refusal(&target.visible_path).is_some()
        {
            return false;
        }
        let path = target.visible_path.clone();
        let backing = match &payload {
            PhysicalHistoryPreflightPayload::ZipCached { backing_path, .. } => backing_path.clone(),
            _ => backing_path.unwrap_or_else(|| path.clone()),
        };
        let aliased = !crate::folder_tree::path_eq(&backing, &path);
        let load_path = if matches!(
            &payload,
            PhysicalHistoryPreflightPayload::Zip(_)
                | PhysicalHistoryPreflightPayload::ZipCached { .. }
        ) {
            &backing
        } else {
            &path
        };
        let folder_changes = self
            .current_folder
            .as_ref()
            .is_none_or(|current| !crate::folder_tree::path_eq(current, load_path));
        let restore_reading_history = aliased
            && self
                .reading_history_return_from
                .as_ref()
                .is_some_and(|from| crate::folder_tree::path_eq(from, &path));
        let restore_bookmark_view = aliased
            .then(|| {
                self.bookmark_view_state
                    .as_ref()
                    .filter(|state| {
                        state
                            .target()
                            .is_some_and(|target| target.matches_loaded_container(&path))
                    })
                    .cloned()
            })
            .flatten();
        if !self.mount_collection_history_child_session(target, prepared, return_to) {
            return false;
        }
        self.pending_auto_fs_open = false;
        self.cancel_folder_pane_open(super::PaneOpenRestoreExit::Adopted);
        self.reset_details_sort_to_toolbar();
        self.reconcile_bookmark_return_target_for_folder_load(&path);
        if self
            .reading_history_return_from
            .as_ref()
            .is_some_and(|from| !crate::folder_tree::path_eq(from, &path))
        {
            self.reading_history_return_from = None;
        }
        if folder_changes && !self.navigation_scope.is_detached_physical() {
            self.clear_archive_convert_nav_history_rollback();
            crate::thumb_loader::bump_catchup_epoch();
            let _ = crate::pdf_loader::bump_render_context_epoch();
        }
        match payload {
            PhysicalHistoryPreflightPayload::Folder(scan) => {
                self.transition_favorite_view_for_path(Some(&path));
                self.clear_meta_undo();
                crate::zip_loader::clear_nested_cache();
                self.zip_nav = None;
                let started = Instant::now();
                self.install_scanned_folder_listing(
                    path.clone(),
                    scan,
                    super::VisibleInstallAuthority::Ordinary,
                    super::FolderListingMetrics {
                        seq: self.input_seq,
                        started,
                        scan_started: started,
                        pre_scanned: true,
                        path_display: if crate::perf::is_enabled() {
                            path.display().to_string()
                        } else {
                            String::new()
                        },
                    },
                )
            }
            PhysicalHistoryPreflightPayload::Zip(enumeration)
            | PhysicalHistoryPreflightPayload::ZipCached { enumeration, .. } => {
                self.load_zip_as_folder_prepared_with_logical_source(
                    backing.clone(),
                    enumeration,
                    aliased.then_some(path.as_path()),
                );
                if aliased {
                    self.address = path.to_string_lossy().to_string();
                    if !(self.global_search.active || self.favsearch.active) {
                        self.forget_recent_folder(&backing);
                        self.remember_recent_folder(&path);
                    }
                    self.update_active_quick_folder_target(&path);
                    self.archive_source_override = Some(path.clone());
                    self.transition_favorite_view_for_path(Some(&path));
                    if restore_reading_history {
                        self.reading_history_return_from = Some(path);
                    }
                    if let Some(state) = restore_bookmark_view {
                        self.bookmark_view_state = Some(state);
                    }
                }
                true
            }
            PhysicalHistoryPreflightPayload::PdfPages(pages) => {
                matches!(
                    self.load_pdf_as_folder_prepared_with_password(
                        path,
                        pages,
                        pdf_password_override
                    ),
                    super::FolderOpenOutcome::Loaded
                )
            }
            PhysicalHistoryPreflightPayload::PdfPasswordRequired
            | PhysicalHistoryPreflightPayload::PdfOpenFailure(_)
            | PhysicalHistoryPreflightPayload::ConvertibleArchive(_)
            | PhysicalHistoryPreflightPayload::ConvertiblePasswordRequired => unreachable!(),
        }
    }

    fn schedule_collection_grid_snapshot(&mut self) {
        if !self.collection_grid_root_materialize_active() {
            return;
        }

        // Pin writes may originate in a physical child or another mounted context. Observe
        // shared stamps lazily here, but replace only this context's presentation. Existing
        // root/fullscreen admission and receive/landing checks own the asynchronous retry.
        let stale_thumbnail_sources = self
            .top_level_grid_view
            .collection_session()
            .filter(|session| {
                matches!(
                    session.load,
                    CollectionGridLoadState::Ready(_) | CollectionGridLoadState::Empty(_)
                )
            })
            .and_then(CollectionGridSession::installed_presentation)
            .is_some_and(|installed| {
                installed.reuse_key
                    != self.collection_grid_prepare_reuse_key(
                        installed.prepared.collection_id,
                        installed.prepared.collection_revision,
                    )
            });
        if stale_thumbnail_sources
            && let Some(retired) = self.invalidate_current_collection_thumbnail_presentation()
        {
            let retired: super::smart_folder::RetiredSmartFolderPayload = Box::new(retired);
            self.retire_smart_folder_payloads(vec![retired]);
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
        let folder_pin_stamp = self
            .folder_thumb_pin_db
            .as_ref()
            .map(|db| db.mutation_stamp());
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
        #[cfg(test)]
        let test_epoch_scope = crate::page_edit_write_epoch::TestEpochScope::capture();
        let spawn = std::thread::Builder::new()
            .name("collection-grid-prepare".into())
            .spawn(move || {
                #[cfg(test)]
                let _test_epoch_scope =
                    test_epoch_scope.map(crate::page_edit_write_epoch::TestEpochScope::enter);
                let result = prepare_collection_grid_install(
                    &snapshot,
                    &display_order,
                    &settings,
                    &worker_cancel,
                    auto_aspect_client.as_ref(),
                    pin_stamp,
                    folder_pin_stamp,
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
        let Some(session) = self.top_level_grid_view.collection_session() else {
            return false;
        };
        let Some(stamp) = self.collection_grid_stamp() else {
            return false;
        };
        self.collection_root_installed_binding_matches(stamp.collection_id)
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
                        && prepared
                            .page_edits
                            .as_ref()
                            .and_then(|(snapshot, _)| snapshot.stamp)
                            .is_some_and(|stamp| {
                                crate::page_edit_write_epoch::PAGE_EDIT_WRITES.accepts(stamp)
                            })
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
            self.folder_thumb_pin_db
                .as_ref()
                .map(|db| db.mutation_stamp()),
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
                CollectionGridLiveThumbnailSources {
                    sources:
                        CollectionGridThumbnailSources {
                            video_sidecars,
                            video_pin_blobs,
                            folder_pin_map,
                            ..
                        },
                    folder_pin_cache,
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
            folder_pin_map,
            folder_pin_cache,
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
        let items = prepared
            .entries
            .iter()
            .map(|entry| entry.item.clone())
            .collect::<Vec<_>>();
        let edits = super::page_edit_snapshot::PageEditSnapshot::load_and_project_stable(
            &items,
            super::page_edit_snapshot::PageEditAvailability::default(),
            &AtomicBool::new(false),
        )
        .unwrap()
        .unwrap();
        let retained_edits = Some(Arc::new(edits.snapshot.clone()));
        let reuse_key = self.collection_grid_prepare_reuse_key(
            prepared.collection_id,
            prepared.collection_revision,
        );
        self.apply_collection_grid_prepared_install(
            CollectionGridPreparedInstall {
                prepared: (*prepared).clone(),
                page_edits: Some((edits.snapshot, edits.projection)),
                retained_page_edits: retained_edits,
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
            std::collections::HashMap::new(),
            std::collections::HashMap::new(),
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
        folder_pin_map: std::collections::HashMap<
            String,
            crate::folder_thumb_pins::FolderPinSource,
        >,
        folder_pin_cache: std::collections::HashMap<String, crate::catalog::CacheEntry>,
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
        self.display_epub_source = None;
        self.current_folder = None;
        self.archive_source_override = None;
        self.zip_nav = None;
        self.stack_view = None;
        self.stack_return_state = None;
        self.stack_mode_requested = false;
        self.stack_showing_flat = false;
        self.cancel_stack_script_pending();
        self.install_prepared_aggregate_items(items, image_metas);
        self.invalidate_idx_state_and_queues();
        self.clear_page_edit_state();
        if let Some((snapshot, mut projection)) = page_edits {
            self.page_edit_snapshot = Some(snapshot);
            self.adjustment_page_params = std::mem::take(&mut projection.adjustment);
            self.export_crop_page_settings = std::mem::take(&mut projection.export_crop);
            self.export_crop_pages = std::mem::take(&mut projection.export_crop_pages);
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
        self.folder_pin_map = folder_pin_map;
        self.initialize_converted_archive_cache_paths();
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

        // Prepared WebP uses the same full-path Seeded#pin key as search/smart-folder rows.
        // No catalog open/write or image decoding is performed during UI installation.
        let cache_map = Arc::new(std::sync::RwLock::new(folder_pin_cache));
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

    #[test]
    fn audit_collection_grid_readers_with_foreign_writers() {
        crate::page_edit_write_epoch::with_foreign_scoped_writers(|| {
            phase_a_collection_mask_is_projected_for_accepted_revision();
            phase_a_collection_install_projects_zip_page_key();
            phase_a2_collection_prepare_rejects_external_edit_after_worker_read();
        });
    }

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
            app.poll_collection_history_transition(&ctx);
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

    #[cfg(windows)]
    #[test]
    fn collection_grid_snapshot_is_terminal_when_its_viewer_is_parked() {
        let temp = tempfile::tempdir().unwrap();
        let (mut app, client) = start_ready_app(&temp.path().join("collections.db"));
        let created = recv(client.create_collection("Parked grid".into()).unwrap());
        let ctx = egui::Context::default();
        app.open_collection_grid(created.collection_id(), None);
        let deadline = Instant::now() + Duration::from_secs(3);
        while !app
            .top_level_grid_view
            .collection_session()
            .is_some_and(|session| {
                matches!(
                    session.load,
                    CollectionGridLoadState::Snapshot { .. }
                        | CollectionGridLoadState::Preparing { .. }
                )
            })
        {
            assert!(
                Instant::now() < deadline,
                "collection snapshot did not start"
            );
            app.poll_collection_ui(&ctx);
            app.poll_collection_grid(&ctx);
            std::thread::sleep(Duration::from_millis(2));
        }
        assert!(
            app.top_level_grid_view
                .collection_session()
                .is_some_and(|session| {
                    matches!(
                        session.load,
                        CollectionGridLoadState::Snapshot { .. }
                            | CollectionGridLoadState::Preparing { .. }
                    )
                })
        );
        let address = app.address.clone();
        let rows = app.items.clone();
        let back = app.folder_nav_back_stack.clone();
        let forward = app.folder_nav_forward_stack.clone();
        app.pause_mounted_background_work_keep_current_frame();
        assert!(
            app.top_level_grid_view
                .collection_session()
                .is_some_and(|session| {
                    !matches!(
                        session.load,
                        CollectionGridLoadState::Snapshot { .. }
                            | CollectionGridLoadState::Preparing { .. }
                    )
                })
        );
        assert_eq!(app.address, address);
        assert_eq!(app.items, rows);
        assert_eq!(app.folder_nav_back_stack, back);
        assert_eq!(app.folder_nav_forward_stack, forward);
        app.shutdown_collection_runtime_for_exit();
    }

    #[cfg(windows)]
    #[test]
    fn deleted_collection_child_stays_physical_through_park_resume_and_backspace() {
        let _test_epoch_scope = crate::page_edit_write_epoch::TestEpochScope::fresh();
        let temp = tempfile::tempdir().unwrap();
        let child = temp.path().join("child");
        std::fs::create_dir(&child).unwrap();
        std::fs::write(child.join("page.jpg"), b"page").unwrap();
        let (mut app, client) = start_ready_app(&temp.path().join("collection.db"));
        app.active_quick_folder_slot = None;
        let collection =
            collection_with_sources(&client, &[(child.clone(), CollectionResolvedKind::Folder)]);
        app.open_collection_grid(collection.collection_id(), None);
        wait_for_grid(&mut app, collection.collection_id());
        let owner = app
            .collection_grid_physical_load_owner(0, &child)
            .expect("Collection child owner");
        let scan =
            super::super::folder_scan::scan_directory_with_settings(&child, &app.settings).unwrap();
        assert!(app.load_folder_with_scan_owned(
            child.clone(),
            Some(scan),
            super::super::OpenRequestOwner::CollectionGridPhysical(owner),
        ));
        assert!(matches!(
            app.collection_grid_parent_nav(),
            Some(crate::ui_main::AddressBarNav::Collection(_))
        ));

        let deleted = recv(
            client
                .delete_collection(collection.collection_id(), collection.revision())
                .unwrap(),
        );
        poll_until(&mut app, "child deletion did not settle", |app| {
            app.top_level_grid_view
                .collection_session()
                .is_some_and(|session| {
                    session.observed_catalog_revision >= deleted.catalog_revision
                        && matches!(session.load, CollectionGridLoadState::Deleted { .. })
                })
        });
        // The catalog delete notice and history-prune notice may arrive in adjacent polls.
        // Settle both before comparing the parked display with its resumed owner.
        let ctx = egui::Context::default();
        app.poll_collection_ui(&ctx);
        app.poll_collection_grid(&ctx);
        let rows = app.items.clone();
        let address = app.address.clone();
        let selected = app.selected;
        let before_back = app.folder_nav_back_stack.clone();
        let before_forward = app.folder_nav_forward_stack.clone();
        app.pause_mounted_background_work_keep_current_frame();
        let parked = app.stash_mounted_and_start_fresh("deleted_collection_child_park");
        app.with_viewer_context(parked, |resumed| {
            let ctx = egui::Context::default();
            resumed.poll_collection_ui(&ctx);
            resumed.poll_collection_grid(&ctx);
            assert!(
                resumed
                    .top_level_grid_view
                    .collection_session()
                    .is_some_and(|session| matches!(
                        session.load,
                        CollectionGridLoadState::Deleted { .. }
                    ))
            );
            assert_eq!(resumed.current_folder.as_deref(), Some(child.as_path()));
            assert_eq!(resumed.items, rows);
            assert_eq!(resumed.address, address);
            assert_eq!(resumed.selected, selected);
            assert_eq!(resumed.folder_nav_back_stack, before_back);
            assert_eq!(resumed.folder_nav_forward_stack, before_forward);
            assert!(resumed.collection_grid_parent_nav().is_none());
            assert!(matches!(
                resumed.collection_grid_current_restore_snapshot(),
                Some(TopLevelGridRestore::Folder(path))
                    if crate::folder_tree::path_eq(&path, &child)
            ));
            let parent = temp.path().to_path_buf();
            assert!(matches!(
                history_grid_key(resumed, egui::Key::Backspace),
                Some(crate::ui_main::AddressBarNav::Direct(path))
                    if crate::folder_tree::path_eq(&path, &parent)
            ));
            let scan =
                super::super::folder_scan::scan_directory_with_settings(&parent, &resumed.settings)
                    .unwrap();
            assert!(resumed.load_folder_with_scan_owned(
                parent.clone(),
                Some(scan),
                super::super::OpenRequestOwner::Navigation,
            ));
            assert_eq!(resumed.current_folder.as_deref(), Some(parent.as_path()));
            assert!(matches!(
                resumed.top_level_grid_view.surface(),
                TopLevelGridSurface::Folder
            ));
            assert!(
                resumed
                    .folder_nav_back_stack
                    .iter()
                    .all(|target| target.collection_id() != Some(collection.collection_id()))
            );
        })
        .unwrap();
        app.shutdown_collection_runtime_for_exit();
    }

    #[cfg(windows)]
    #[test]
    fn park_keeps_other_settled_collection_load_states() {
        let temp = tempfile::tempdir().unwrap();
        let (mut app, client) = start_ready_app(&temp.path().join("collection.db"));
        let empty = recv(client.create_collection("Empty".into()).unwrap());
        app.open_collection_grid(empty.collection_id(), None);
        wait_for_grid(&mut app, empty.collection_id());
        assert!(matches!(
            app.top_level_grid_view.collection_session().unwrap().load,
            CollectionGridLoadState::Empty(_)
        ));
        app.pause_mounted_background_work_keep_current_frame();
        assert!(matches!(
            app.top_level_grid_view.collection_session().unwrap().load,
            CollectionGridLoadState::Empty(_)
        ));

        let session = app.top_level_grid_view.collection_session_mut().unwrap();
        session.load = CollectionGridLoadState::Failed {
            message: "settled failure".into(),
            installed: None,
        };
        app.pause_mounted_background_work_keep_current_frame();
        assert!(matches!(
            &app.top_level_grid_view.collection_session().unwrap().load,
            CollectionGridLoadState::Failed { message, .. } if message == "settled failure"
        ));

        app.top_level_grid_view
            .collection_session_mut()
            .unwrap()
            .cancel_pending();
        assert!(matches!(
            app.top_level_grid_view.collection_session().unwrap().load,
            CollectionGridLoadState::RequestNeeded { .. }
        ));
        app.pause_mounted_background_work_keep_current_frame();
        assert!(matches!(
            app.top_level_grid_view.collection_session().unwrap().load,
            CollectionGridLoadState::RequestNeeded { .. }
        ));

        let image = temp.path().join("image.png");
        std::fs::write(&image, b"image").unwrap();
        let ready = collection_with_sources(&client, &[(image, CollectionResolvedKind::Image)]);
        app.open_collection_grid(ready.collection_id(), None);
        wait_for_grid(&mut app, ready.collection_id());
        let before = match &app.top_level_grid_view.collection_session().unwrap().load {
            CollectionGridLoadState::Ready(presentation) => Arc::clone(presentation),
            _ => panic!("nonempty collection should be ready"),
        };
        app.pause_mounted_background_work_keep_current_frame();
        assert!(matches!(
            &app.top_level_grid_view.collection_session().unwrap().load,
            CollectionGridLoadState::Ready(presentation) if Arc::ptr_eq(presentation, &before)
        ));
        app.shutdown_collection_runtime_for_exit();
    }

    #[cfg(all(windows, feature = "test-script"))]
    #[test]
    fn seeded_collection_smoke_action_opens_python_seeded_root_after_startup() {
        let repo = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let target = repo.join("target");
        std::fs::create_dir_all(&target).unwrap();
        let temp = tempfile::Builder::new()
            .prefix("rating-sort-collection-lib-")
            .tempdir_in(target)
            .unwrap();
        std::fs::write(
            temp.path().join(".disposable-smoke-data"),
            "mimageviewer-disposable-smoke-v1;test-script=true",
        )
        .unwrap();
        let fixture = temp.path().join("rating-sort-collection").join("fixture");
        std::fs::create_dir_all(&fixture).unwrap();
        let names = ["01-one.png", "02-unrated.png", "03-two.png"];
        for name in names {
            std::fs::copy(
                repo.join("testdata/rating-sort").join(name),
                fixture.join(name),
            )
            .unwrap();
        }
        let seed_script = repo.join("scripts/ui-smoke/seed_rating_sort_collection.py");
        let seed = std::process::Command::new("python")
            .args([&seed_script, &fixture])
            .arg("--seed-db")
            .arg(temp.path())
            .output()
            .expect("Python Collection smoke seeder");
        assert!(
            seed.status.success(),
            "seed failed: {}",
            String::from_utf8_lossy(&seed.stderr)
        );

        let (mut app, _client) = start_ready_app(&temp.path().join("collection.db"));
        assert_eq!(
            app.open_seeded_collection_for_smoke().unwrap_err(),
            "startup folder open is still pending"
        );
        app.initialized = true;
        app.startup_open_path = Some(fixture.clone());
        assert_eq!(
            app.open_seeded_collection_for_smoke().unwrap_err(),
            "startup folder open is still pending"
        );
        app.startup_open_path = None;
        app.open_seeded_collection_for_smoke().unwrap();
        let id = crate::test_script::seeded_collection_smoke_id();
        wait_for_grid(&mut app, id);
        let root = app.collection_grid_root_order().unwrap().unwrap();
        assert_eq!(root.content.stamp.collection_id, id);
        assert_eq!(format!("{:?}", root.mode), "Manual");
        assert_eq!(format!("{:?}", root.standard_sort), "FileName");
        assert_eq!(root.content.expected_revision, 1);
        assert_eq!(
            app.items
                .iter()
                .map(|item| item.name().into_owned())
                .collect::<Vec<_>>(),
            names.map(str::to_owned).to_vec()
        );
        let verify = std::process::Command::new("python")
            .args([&seed_script, &fixture])
            .arg("--verify-db")
            .arg(temp.path())
            .output()
            .expect("Python Collection smoke verifier");
        assert!(
            verify.status.success(),
            "verify failed: {}",
            String::from_utf8_lossy(&verify.stderr)
        );
    }

    fn poll_until(app: &mut App, message: &str, mut condition: impl FnMut(&App) -> bool) {
        let ctx = egui::Context::default();
        let deadline = Instant::now() + Duration::from_secs(5);
        while !condition(app) {
            assert!(Instant::now() < deadline, "{message}");
            app.settle_open_path_classification_for_test();
            app.poll_collection_ui(&ctx);
            app.poll_collection_grid(&ctx);
            app.poll_collection_history_transition(&ctx);
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    fn write_single_page_zip(path: &Path) {
        let file = std::fs::File::create(path).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        zip.start_file("page.jpg", zip::write::SimpleFileOptions::default())
            .unwrap();
        std::io::Write::write_all(&mut zip, b"page").unwrap();
        zip.finish().unwrap();
    }

    fn write_nested_history_zip(path: &Path) {
        let file = std::fs::File::create(path).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        for entry in ["root.png", "chapter/first.png", "chapter/second.png"] {
            zip.start_file(entry, zip::write::SimpleFileOptions::default())
                .unwrap();
            std::io::Write::write_all(&mut zip, b"page").unwrap();
        }
        zip.finish().unwrap();
    }

    fn write_two_page_history_pdf(path: &Path) {
        // A valid two-page PDF lets the normal PDFium worker enumerate the file.
        let objects: [&[u8]; 6] = [
            b"<< /Type /Catalog /Pages 2 0 R >>",
            b"<< /Type /Pages /Kids [4 0 R 6 0 R] /Count 2 >>",
            b"<< /Length 0 >>\nstream\nendstream",
            b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 100 100] /Contents 3 0 R >>",
            b"<< /Length 0 >>\nstream\nendstream",
            b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 100 100] /Contents 5 0 R >>",
        ];
        let mut bytes = b"%PDF-1.4\n".to_vec();
        let mut offsets = vec![0usize];
        for (index, object) in objects.iter().enumerate() {
            offsets.push(bytes.len());
            bytes.extend_from_slice(format!("{} 0 obj\n", index + 1).as_bytes());
            bytes.extend_from_slice(object);
            bytes.extend_from_slice(b"\nendobj\n");
        }
        let xref = bytes.len();
        bytes.extend_from_slice(
            format!("xref\n0 {}\n0000000000 65535 f \n", offsets.len()).as_bytes(),
        );
        for offset in offsets.into_iter().skip(1) {
            bytes.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
        }
        bytes.extend_from_slice(
            format!(
                "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
                objects.len() + 1
            )
            .as_bytes(),
        );
        std::fs::write(path, bytes).unwrap();
    }

    fn write_small_history_epub(path: &Path) {
        let file = std::fs::File::create(path).unwrap();
        let mut epub = zip::ZipWriter::new(file);
        let stored = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        epub.start_file("mimetype", stored).unwrap();
        std::io::Write::write_all(&mut epub, b"application/epub+zip").unwrap();
        let options = zip::write::SimpleFileOptions::default();
        for (name, bytes) in [
            ("META-INF/container.xml", b"<?xml version=\"1.0\"?><container version=\"1.0\" xmlns=\"urn:oasis:names:tc:opendocument:xmlns:container\"><rootfiles><rootfile full-path=\"OEBPS/content.opf\" media-type=\"application/oebps-package+xml\"/></rootfiles></container>".as_slice()),
            ("OEBPS/content.opf", b"<?xml version=\"1.0\"?><package xmlns=\"http://www.idpf.org/2007/opf\" version=\"3.0\" unique-identifier=\"id\"><metadata xmlns:dc=\"http://purl.org/dc/elements/1.1/\"><dc:identifier id=\"id\">history-fixture</dc:identifier><dc:title>History fixture</dc:title><dc:language>en</dc:language></metadata><manifest><item id=\"chapter\" href=\"chapter.xhtml\" media-type=\"application/xhtml+xml\"/></manifest><spine><itemref idref=\"chapter\"/></spine></package>".as_slice()),
            ("OEBPS/chapter.xhtml", b"<?xml version=\"1.0\"?><html xmlns=\"http://www.w3.org/1999/xhtml\"><head><title>Page</title></head><body><p>History fixture</p></body></html>".as_slice()),
        ] {
            epub.start_file(name, options).unwrap();
            std::io::Write::write_all(&mut epub, bytes).unwrap();
        }
        epub.finish().unwrap();
        let mut parsed = zip::ZipArchive::new(std::fs::File::open(path).unwrap()).unwrap();
        assert_eq!(parsed.by_name("mimetype").unwrap().size(), 20);
        assert!(parsed.by_name("OEBPS/content.opf").is_ok());
    }

    fn read_real_pdf_pages_for_history(path: &Path) -> Vec<crate::pdf_loader::PdfPageEntry> {
        use pdfium_render::prelude::Pdfium;

        // The lib-test executable cannot act as the application's --pdf-worker child.
        // Bind PDFium in-process for this test and feed the real file's enumerated pages
        // into the same staged adoption boundary that the worker normally feeds.
        let dll_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("vendor/pdfium/bin");
        let bindings = Pdfium::bind_to_library(Pdfium::pdfium_platform_library_name_at_path(
            dll_dir.to_str().unwrap(),
        ))
        .expect("PDFium DLL binding");
        let pdfium = Pdfium::new(bindings);
        let document = pdfium
            .load_pdf_from_file(path, None)
            .expect("valid real PDF");
        let metadata = std::fs::metadata(path).unwrap();
        let pages = (0..document.pages().len())
            .map(|page_num| crate::pdf_loader::PdfPageEntry {
                page_num: u32::from(page_num),
                mtime: crate::ui_helpers::mtime_secs(&metadata),
                file_size: metadata.len(),
            })
            .collect::<Vec<_>>();
        assert_eq!(pages.len(), 2);
        pages
    }

    fn real_cached_epub_history_fixture(
        source: &Path,
        data_dir: &Path,
    ) -> (
        crate::pdf_loader::TestEpubPin,
        crate::pdf_loader::RemotePdfTestBackend,
        crate::pdf_loader::DocumentStamp,
    ) {
        use crate::epub_cache::{EpubCache, GenerationRow, WriteDenyingSource};

        write_small_history_epub(source);
        let mut cache = EpubCache::open_at(data_dir).unwrap();
        let reserved = cache.reserve_output(source).unwrap();
        let generated = reserved.final_path().to_path_buf();
        std::fs::create_dir_all(generated.parent().unwrap()).unwrap();
        write_two_page_history_pdf(&generated);
        let size = std::fs::metadata(&generated).unwrap().len();
        let row = GenerationRow {
            generation_id: reserved.generation_id(),
            src_path_key: crate::epub_cache::src_key(source),
            src_path: source.to_path_buf(),
            src_state: crate::epub_cache::source_state(&std::fs::metadata(source).unwrap()),
            src_sha256: "history-fixture-full".into(),
            src_head_hash: "history-fixture-head".into(),
            pdf_file: generated,
            pdf_size: size,
            page_count: 2,
            direction: "rtl".into(),
            profile: "reflow-v1".into(),
            created_at: 1,
            output_version: crate::epub_cache::CONVERTER_OUTPUT_VERSION,
        };
        let source_guard = WriteDenyingSource::open(source).unwrap();
        assert!(matches!(
            cache.publish(&row, &source_guard),
            Ok(crate::epub_cache::PublishOutcome::Published)
        ));
        let (pin, target) = crate::pdf_loader::pin_cached_epub_for_test(source, data_dir).unwrap();
        assert_eq!(target.read_path.as_path(), row.pdf_file);
        assert_eq!(
            target.epub_direction,
            Some(crate::pdf_loader::PdfReadingDirection::R2L)
        );
        let backend = crate::pdf_loader::RemotePdfTestBackend::for_path(&row.pdf_file);
        (pin, backend, target.stamp.clone())
    }

    fn assert_cached_epub_worker_adoption(
        app: &mut App,
        source: &Path,
        stamp: &crate::pdf_loader::DocumentStamp,
    ) {
        assert_eq!(
            app.reading_direction,
            crate::settings::ReadingDirection::Rtl
        );
        assert_eq!(app.items.len(), 2);
        let (id, size) = stamp.generation_catalog_pair().unwrap();
        let catalog = app.get_or_open_catalog(source.parent().unwrap()).unwrap();
        let name = source.file_name().unwrap().to_str().unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while catalog.get_pdf_meta(name, id, size).unwrap() != Some((2, false)) {
            assert!(
                Instant::now() < deadline,
                "EPUB worker generation stamp was not adopted"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    fn history_grid_key_with_modifiers(
        app: &mut App,
        key: egui::Key,
        modifiers: egui::Modifiers,
    ) -> Option<crate::ui_main::AddressBarNav> {
        let ctx = egui::Context::default();
        ctx.begin_pass(egui::RawInput {
            events: vec![egui::Event::Key {
                key,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers,
            }],
            ..Default::default()
        });
        let nav = app.handle_keyboard(&ctx);
        let _ = ctx.end_pass();
        nav
    }

    fn history_grid_key(app: &mut App, key: egui::Key) -> Option<crate::ui_main::AddressBarNav> {
        history_grid_key_with_modifiers(app, key, egui::Modifiers::NONE)
    }

    #[cfg(windows)]
    #[test]
    fn collection_child_backspace_back_forward_round_trip_uses_keyboard_history() {
        let _test_epoch_scope = crate::page_edit_write_epoch::TestEpochScope::fresh();
        let temp = tempfile::tempdir().unwrap();
        let child = temp.path().join("B");
        std::fs::create_dir(&child).unwrap();
        std::fs::write(child.join("page.jpg"), b"page").unwrap();
        let (mut app, client) = start_ready_app(&temp.path().join("collection.db"));
        app.active_quick_folder_slot = None;
        let collection =
            collection_with_sources(&client, &[(child.clone(), CollectionResolvedKind::Folder)]);
        app.open_collection_grid(collection.collection_id(), None);
        wait_for_grid(&mut app, collection.collection_id());
        app.selected = Some(0);
        let Some(crate::ui_main::AddressBarNav::CollectionSource { path, owner }) =
            history_grid_key(&mut app, egui::Key::Enter)
        else {
            panic!("Enter must route the Collection root row to its physical child");
        };
        let scan =
            super::super::folder_scan::scan_directory_with_settings(&path, &app.settings).unwrap();
        assert!(app.load_folder_with_scan_owned(
            path,
            Some(scan),
            super::super::OpenRequestOwner::CollectionGridPhysical(owner),
        ));
        let child_target = app.folder_nav_current_target().expect("adopted child");
        let root_target = app
            .folder_history_back_target()
            .cloned()
            .expect("root back target");

        let Some(crate::ui_main::AddressBarNav::Collection(restore)) =
            history_grid_key(&mut app, egui::Key::Backspace)
        else {
            panic!("Backspace must route to the Collection root");
        };
        app.apply_collection_input_nav(restore, false);
        poll_real_history_load(&mut app, None, None);
        wait_for_grid(&mut app, collection.collection_id());
        assert_eq!(app.folder_nav_current_target(), Some(root_target.clone()));
        assert_eq!(app.folder_history_back_target(), Some(&child_target));

        let alt = egui::Modifiers {
            alt: true,
            ..Default::default()
        };
        assert!(matches!(
            history_grid_key_with_modifiers(&mut app, egui::Key::ArrowLeft, alt),
            Some(crate::ui_main::AddressBarNav::HistoryBack)
        ));
        let mut rollback = None;
        assert!(
            app.dispatch_main_folder_history_input(
                super::super::FolderHistoryDirection::Back,
                &mut rollback,
            )
            .is_none()
        );
        poll_real_history_load(&mut app, Some(&child), None);
        assert_eq!(app.folder_nav_current_target(), Some(child_target));
        assert_eq!(app.folder_history_forward_target(), Some(&root_target));
        assert!(matches!(
            history_grid_key_with_modifiers(&mut app, egui::Key::ArrowRight, alt),
            Some(crate::ui_main::AddressBarNav::HistoryForward)
        ));
        assert!(
            app.dispatch_main_folder_history_input(
                super::super::FolderHistoryDirection::Forward,
                &mut rollback,
            )
            .is_none()
        );
        poll_real_history_load(&mut app, None, None);
        assert_eq!(app.folder_nav_current_target(), Some(root_target));
        app.shutdown_collection_runtime_for_exit();
    }

    #[cfg(windows)]
    #[test]
    fn rating_child_backspace_back_forward_round_trip_uses_keyboard_history() {
        let _test_epoch_scope = crate::page_edit_write_epoch::TestEpochScope::fresh();
        let temp = tempfile::tempdir().unwrap();
        let child = temp.path().join("B");
        std::fs::create_dir(&child).unwrap();
        std::fs::write(child.join("page.jpg"), b"page").unwrap();
        let (mut app, _) = start_ready_app(&temp.path().join("collection.db"));
        app.active_quick_folder_slot = None;
        let key = crate::adjustment_db::normalize_path(&child);
        app.rating_db.as_ref().unwrap().set(&key, 1).unwrap();
        app.enter_rating_view_from_menu(1);
        let deadline = Instant::now() + Duration::from_secs(15);
        while app.rating_view_pending.is_some() {
            assert!(Instant::now() < deadline, "Rating root did not settle");
            app.poll_rating_view();
            std::thread::sleep(Duration::from_millis(5));
        }
        let rating_root = app.current_folder.clone();
        assert!(app.items_are_rating_view);
        let root_target = app.folder_nav_current_target().expect("Rating root");
        let row = app
            .items
            .iter()
            .position(|item| {
                item.drag_source_path().is_some_and(|path| {
                    crate::adjustment_db::normalize_path(path)
                        == crate::adjustment_db::normalize_path(&child)
                })
            })
            .expect("Rating child row");
        let rated_child = app.items[row].drag_source_path().unwrap().to_path_buf();
        app.selected = Some(row);
        let Some(crate::ui_main::AddressBarNav::RatingSource { owner, .. }) =
            history_grid_key(&mut app, egui::Key::Enter)
        else {
            panic!("Enter must route the Rating row to its physical child");
        };
        assert!(app.start_rating_physical_open(owner));
        poll_real_history_load(&mut app, Some(&rated_child), None);
        let child_target = app.folder_nav_current_target().expect("Rating child");
        assert_eq!(app.folder_history_back_target(), Some(&root_target));

        assert!(history_grid_key(&mut app, egui::Key::Backspace).is_none());
        poll_real_history_load(&mut app, rating_root.as_deref(), None);
        assert_eq!(app.folder_nav_current_target(), Some(root_target.clone()));
        assert_eq!(app.folder_history_back_target(), Some(&child_target));

        let alt = egui::Modifiers {
            alt: true,
            ..Default::default()
        };
        assert!(matches!(
            history_grid_key_with_modifiers(&mut app, egui::Key::ArrowLeft, alt),
            Some(crate::ui_main::AddressBarNav::HistoryBack)
        ));
        replay_real_history(&mut app, true, Some(&rated_child), None);
        assert_eq!(app.folder_history_forward_target(), Some(&root_target));
        assert!(matches!(
            history_grid_key_with_modifiers(&mut app, egui::Key::ArrowRight, alt),
            Some(crate::ui_main::AddressBarNav::HistoryForward)
        ));
        replay_real_history(&mut app, false, rating_root.as_deref(), None);
        assert_eq!(app.folder_nav_current_target(), Some(root_target));
        app.shutdown_collection_runtime_for_exit();
    }

    #[cfg(windows)]
    #[test]
    fn physical_folder_backspace_back_forward_round_trip_uses_keyboard_history() {
        let _test_epoch_scope = crate::page_edit_write_epoch::TestEpochScope::fresh();
        let temp = tempfile::tempdir().unwrap();
        let parent = temp.path().join("A");
        let child = parent.join("B");
        std::fs::create_dir_all(&child).unwrap();
        std::fs::write(child.join("page.jpg"), b"page").unwrap();
        let (mut app, _) = start_ready_app(&temp.path().join("collection.db"));
        app.active_quick_folder_slot = None;
        for path in [&parent, &child] {
            let scan = super::super::folder_scan::scan_directory_with_settings(path, &app.settings)
                .unwrap();
            assert!(app.load_folder_with_scan_owned(
                path.clone(),
                Some(scan),
                super::super::OpenRequestOwner::Navigation,
            ));
        }
        let child_target = app.folder_nav_current_target().expect("physical child");
        let parent_target = app.folder_history_back_target().cloned().expect("parent");
        let Some(crate::ui_main::AddressBarNav::Direct(path)) =
            history_grid_key(&mut app, egui::Key::Backspace)
        else {
            panic!("Backspace must route to the physical parent");
        };
        assert!(crate::folder_tree::path_eq(&path, &parent));
        let scan =
            super::super::folder_scan::scan_directory_with_settings(&path, &app.settings).unwrap();
        assert!(app.load_folder_with_scan_owned(
            path,
            Some(scan),
            super::super::OpenRequestOwner::Navigation,
        ));
        assert_eq!(app.folder_nav_current_target(), Some(parent_target.clone()));
        assert_eq!(app.folder_history_back_target(), Some(&child_target));

        let alt = egui::Modifiers {
            alt: true,
            ..Default::default()
        };
        for (key, direction, expected, forward) in [
            (
                egui::Key::ArrowLeft,
                super::super::FolderHistoryDirection::Back,
                &child,
                Some(&parent_target),
            ),
            (
                egui::Key::ArrowRight,
                super::super::FolderHistoryDirection::Forward,
                &parent,
                None,
            ),
        ] {
            assert!(history_grid_key_with_modifiers(&mut app, key, alt).is_some());
            let mut rollback = None;
            let path = app
                .dispatch_main_folder_history_input(direction, &mut rollback)
                .expect("physical history path");
            assert!(crate::folder_tree::path_eq(&path, expected));
            let scan =
                super::super::folder_scan::scan_directory_with_settings(&path, &app.settings)
                    .unwrap();
            assert!(app.load_folder_with_scan_owned(
                path,
                Some(scan),
                super::super::OpenRequestOwner::Navigation,
            ));
            assert_eq!(app.folder_history_forward_target(), forward);
        }
        assert_eq!(app.folder_nav_current_target(), Some(parent_target));
        app.shutdown_collection_runtime_for_exit();
    }

    fn seed_real_pdf_preflight_if_ready(
        app: &mut App,
        pages: Option<&crate::pdf_loader::PdfEnumerateResult>,
        seeded: &mut bool,
    ) {
        if *seeded {
            return;
        }
        let Some(pages) = pages else {
            return;
        };
        if matches!(
            app.top_level_grid_view.history_navigation_transition(),
            Some(super::super::HistoryNavigationTransition::Physical(_))
        ) {
            seed_physical_history_preflight(
                app,
                super::super::collection_navigation::PhysicalHistoryPreflightPayload::PdfPages(
                    pages.clone(),
                ),
            );
            *seeded = true;
            return;
        }
        let child_preflighting = matches!(
            app.top_level_grid_view.history_navigation_transition(),
            Some(super::super::HistoryNavigationTransition::Collection(request))
                if matches!(&request.phase, super::super::CollectionHistoryPhase::ChildPreflighting { .. })
        );
        if child_preflighting {
            let Some(super::super::HistoryNavigationTransition::Collection(mut request)) =
                app.top_level_grid_view.take_history_navigation_transition()
            else {
                unreachable!()
            };
            let super::super::CollectionHistoryPhase::ChildPreflighting { preflight, .. } =
                &mut request.phase
            else {
                unreachable!()
            };
            *preflight =
                super::super::collection_navigation::PhysicalHistoryPreflight::ready_for_test(
                    super::super::collection_navigation::PhysicalHistoryPreflightPayload::PdfPages(
                        pages.clone(),
                    ),
                );
            app.top_level_grid_view
                .set_history_navigation_transition(Some(
                    super::super::HistoryNavigationTransition::Collection(request),
                ));
            *seeded = true;
        }
    }

    fn poll_real_history_load(
        app: &mut App,
        expected: Option<&Path>,
        pdf_pages: Option<&crate::pdf_loader::PdfEnumerateResult>,
    ) {
        let ctx = egui::Context::default();
        let deadline = Instant::now() + Duration::from_secs(15);
        let mut seeded = false;
        loop {
            app.settle_open_path_classification_for_test();
            seed_real_pdf_preflight_if_ready(app, pdf_pages, &mut seeded);
            app.poll_collection_ui(&ctx);
            seed_real_pdf_preflight_if_ready(app, pdf_pages, &mut seeded);
            app.poll_collection_grid(&ctx);
            seed_real_pdf_preflight_if_ready(app, pdf_pages, &mut seeded);
            app.poll_rating_view();
            seed_real_pdf_preflight_if_ready(app, pdf_pages, &mut seeded);
            app.poll_collection_history_transition(&ctx);
            if app
                .current_folder
                .as_deref()
                .zip(expected)
                .is_some_and(|(path, expected)| crate::folder_tree::path_eq(path, expected))
                || app.current_folder.is_none() && expected.is_none()
            {
                if app
                    .top_level_grid_view
                    .history_navigation_transition()
                    .is_none()
                    && app.rating_view_pending.is_none()
                {
                    assert!(
                        pdf_pages.is_none() || seeded,
                        "PDF preflight was not adopted"
                    );
                    return;
                }
            }
            assert!(
                Instant::now() < deadline,
                "real history load did not settle: expected={expected:?} current={:?} seeded={seeded} transition={} pdf_pending={} position={:?} items={:?}",
                app.current_folder,
                app.top_level_grid_view
                    .history_navigation_transition()
                    .is_some(),
                app.pdf_enumerate_pending.is_some(),
                app.top_level_grid_view
                    .collection_session()
                    .map(|session| &session.position),
                app.items
            );
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    fn replay_real_history(
        app: &mut App,
        back: bool,
        expected: Option<&Path>,
        pdf_pages: Option<&crate::pdf_loader::PdfEnumerateResult>,
    ) {
        let target = if back {
            app.folder_history_back_target()
        } else {
            app.folder_history_forward_target()
        }
        .cloned()
        .expect("history target");
        let direction = if back {
            super::super::FolderHistoryDirection::Back
        } else {
            super::super::FolderHistoryDirection::Forward
        };
        let started = match &target {
            super::super::FolderNavHistoryTarget::Collection(_)
            | super::super::FolderNavHistoryTarget::CollectionPhysical(_) => app
                .start_collection_history_transition(
                    target.clone(),
                    super::super::CollectionHistoryIntent::Replay {
                        direction,
                        target: target.clone(),
                    },
                    None,
                ),
            super::super::FolderNavHistoryTarget::Rating { stars } => {
                app.start_rating_history_replay(direction, *stars)
            }
            super::super::FolderNavHistoryTarget::RatingPhysical(restore) => app
                .start_rating_physical_restore(
                    restore.clone(),
                    Some((direction, target.clone())),
                    super::super::RatingPhysicalLoadIntent::Restore,
                ),
            _ => panic!("unexpected virtual book history target: {target:?}"),
        };
        assert!(started, "history replay did not start: {target:?}");
        poll_real_history_load(app, expected, pdf_pages);
        assert_eq!(app.folder_nav_current_target(), Some(target));
    }

    fn seed_physical_history_preflight(
        app: &mut App,
        payload: super::super::collection_navigation::PhysicalHistoryPreflightPayload,
    ) {
        let Some(super::super::HistoryNavigationTransition::Physical(mut request)) =
            app.top_level_grid_view.take_history_navigation_transition()
        else {
            panic!("physical preflight must own the pending request")
        };
        request.phase = super::super::PhysicalHistoryPhase::Preflighting {
            preflight:
                super::super::collection_navigation::PhysicalHistoryPreflight::ready_for_test(
                    payload,
                ),
            pdf_password_submission: None,
        };
        app.top_level_grid_view
            .set_history_navigation_transition(Some(
                super::super::HistoryNavigationTransition::Physical(request),
            ));
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

    fn seed_search_or_snapshot_without_return_to(
        app: &mut App,
        source: PathBuf,
        saved_folder: PathBuf,
        snapshot: bool,
    ) {
        app.current_folder = Some(source.parent().unwrap().join("search-results"));
        app.items = vec![GridItem::Video(source)];
        app.thumbnails = vec![ThumbnailState::Pending];
        app.image_metas = vec![None];
        app.visible_indices = vec![0];
        app.global_search.active = true;
        app.global_search.saved_folder = Some(saved_folder);
        app.top_level_grid_view.begin(
            TopLevelGridSurface::Search(
                super::super::top_level_grid_view::TopLevelSearchView::Global,
            ),
            None,
        );
        if snapshot {
            app.activate_snapshot(crate::snapshot::SnapshotSourceLabel::GlobalSearch {
                query: "collection return".into(),
            });
            assert!(app.snapshot.is_some());
            // Exercise the pre-canonical fallback retained for older Snapshot states.
            app.top_level_grid_view
                .replace_surface(TopLevelGridSurface::Snapshot);
        }
        assert!(app.top_level_grid_view.return_to().is_none());
        assert!(app.current_top_level_restore_snapshot().is_none());
    }

    fn assert_collection_return_folder(app: &App, saved_folder: &Path) {
        assert!(
            matches!(
                app.top_level_grid_view.return_to(),
                Some(TopLevelGridRestore::Folder(path))
                    if crate::folder_tree::path_eq(path, saved_folder)
            ),
            "collection return_to={:?}, expected folder={saved_folder:?}",
            app.top_level_grid_view.return_to()
        );
    }

    #[test]
    fn dismiss_global_search_moves_subfolder_restore_without_cloning_large_state() {
        let _test_epoch_scope = crate::page_edit_write_epoch::TestEpochScope::fresh();
        let temp = tempfile::tempdir().unwrap();
        let (mut app, _) = start_ready_app(&temp.path().join("collection.db"));
        let removed_path = "removed-video-path".repeat(32);
        let removed_path_ptr = removed_path.as_ptr();
        app.global_search_subfolder_restore = Some(
            super::super::subfolder_expansion::SubfolderExpansionRestoreState {
                root: None,
                roots: Vec::new(),
                saved_folder: None,
                snapshot: None,
                removed_paths: std::iter::once(removed_path).collect(),
            },
        );
        app.global_search.active = true;
        app.global_search.saved_folder = Some(super::super::subfolder_expansion_synthetic_path());
        app.top_level_grid_view.begin(
            TopLevelGridSurface::Search(
                super::super::top_level_grid_view::TopLevelSearchView::Global,
            ),
            None,
        );

        let returned = app.dismiss_global_search_without_restore();
        let TopLevelGridRestore::SubfolderExpansion(state) = returned else {
            panic!("subfolder expansion restore was not returned");
        };
        assert_eq!(
            state.removed_paths.iter().next().unwrap().as_ptr(),
            removed_path_ptr
        );
        app.shutdown_collection_runtime_for_exit();
    }

    #[test]
    fn collection_open_from_transient_search_or_snapshot_keeps_fallback_return_folder() {
        for snapshot in [true, false] {
            let _test_epoch_scope = crate::page_edit_write_epoch::TestEpochScope::fresh();
            let temp = tempfile::tempdir().unwrap();
            let source = temp.path().join("movie.mp4");
            let saved_folder = temp.path().join("before-search");
            std::fs::write(&source, b"media").unwrap();
            std::fs::create_dir(&saved_folder).unwrap();
            let (mut app, client) = start_ready_app(&temp.path().join("collection.db"));
            let target = recv(client.create_collection("Target".into()).unwrap());
            seed_search_or_snapshot_without_return_to(
                &mut app,
                source,
                saved_folder.clone(),
                snapshot,
            );

            app.open_collection_grid_from_navigation(target.collection_id());
            assert_collection_return_folder(&app, &saved_folder);
            wait_for_grid(&mut app, target.collection_id());
            assert_collection_return_folder(&app, &saved_folder);
            app.shutdown_collection_runtime_for_exit();
        }
    }

    #[test]
    fn collection_open_uses_last_transient_fallback_after_snapshot_consumes_return_to() {
        let _test_epoch_scope = crate::page_edit_write_epoch::TestEpochScope::fresh();
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("movie.mp4");
        let search_folder = temp.path().join("before-search");
        let tag_folder = temp.path().join("before-tag");
        std::fs::write(&source, b"media").unwrap();
        std::fs::create_dir(&search_folder).unwrap();
        std::fs::create_dir(&tag_folder).unwrap();
        let (mut app, client) = start_ready_app(&temp.path().join("collection.db"));
        let target = recv(client.create_collection("Target".into()).unwrap());
        seed_search_or_snapshot_without_return_to(&mut app, source, search_folder.clone(), true);
        app.top_level_grid_view
            .install_return_to(TopLevelGridRestore::Folder(search_folder));
        app.tag_view.active = true;
        app.tag_view.saved_folder = Some(tag_folder.clone());

        app.open_collection_grid_from_navigation(target.collection_id());
        assert_collection_return_folder(&app, &tag_folder);
        assert!(app.snapshot.is_none());
        assert!(!app.tag_view.active);
        app.shutdown_collection_runtime_for_exit();
    }

    #[test]
    #[cfg(windows)]
    fn collection_open_from_detached_transient_video_keeps_fallback_return_folder() {
        for snapshot in [true, false] {
            let _test_epoch_scope = crate::page_edit_write_epoch::TestEpochScope::fresh();
            let temp = tempfile::tempdir().unwrap();
            let source = temp.path().join("movie.mp4");
            let saved_folder = temp.path().join("before-search");
            std::fs::write(&source, b"media").unwrap();
            std::fs::create_dir(&saved_folder).unwrap();
            let (mut app, client) = start_ready_app(&temp.path().join("collection.db"));
            let target = recv(client.create_collection("Target".into()).unwrap());
            seed_search_or_snapshot_without_return_to(
                &mut app,
                source.clone(),
                saved_folder.clone(),
                snapshot,
            );
            let generation = mount_detached_collection_media(&mut app, source, 1_314, false);

            app.open_collection_grid_from_navigation(target.collection_id());
            assert_collection_return_folder(&app, &saved_folder);
            app.with_window_viewer_context(1_314, |window| {
                assert_eq!(window.fullscreen_idx, Some(0));
                assert_eq!(window.items_generation, generation);
                assert!(matches!(window.items.as_slice(), [GridItem::Video(_)]));
                assert!(matches!(
                    window.fs_cache.get(&0),
                    Some(super::super::FsCacheEntry::Video { .. })
                ));
                assert!(matches!(
                    window.top_level_grid_view.surface(),
                    TopLevelGridSurface::Snapshot | TopLevelGridSurface::Search(_)
                ));
            })
            .unwrap();
            wait_for_grid(&mut app, target.collection_id());
            assert_collection_return_folder(&app, &saved_folder);
            app.shutdown_collection_runtime_for_exit();
        }
    }

    fn collection_pin_request(app: &App, index: usize) -> crate::thumb_loader::LoadRequest {
        let (mtime, size) = app.image_metas[index].unwrap_or_default();
        super::super::make_load_request(
            &app.items[index],
            index,
            mtime,
            size,
            false,
            None,
            Some(app.settings.folder_thumb_sort),
            app.settings.folder_thumb_depth,
            &app.folder_pin_map,
            &app.converted_archive_cache_paths,
            None,
            app.current_folder.as_deref(),
            app.folder_thumb_pin_db.as_deref(),
            app.video_pin_db.as_ref(),
            app.use_full_path_cache_keys(),
        )
        .unwrap()
    }

    #[test]
    fn collection_container_pins_prepare_and_install_folder_zip_pdf_archive_epub() {
        use crate::folder_thumb_pins::{FileKind, FolderPinSource, FolderThumbPinDb};
        let _scope = crate::page_edit_write_epoch::TestEpochScope::fresh();
        let temp = tempfile::tempdir().unwrap();
        let folder = temp.path().join("album");
        std::fs::create_dir(&folder).unwrap();
        std::fs::write(folder.join("cover.png"), b"image").unwrap();
        let zip = temp.path().join("book.zip");
        let pdf = temp.path().join("book.pdf");
        let archive = temp.path().join("book.7z");
        let epub = temp.path().join("book.epub");
        for path in [&zip, &pdf, &archive, &epub] {
            std::fs::write(path, b"container").unwrap();
        }
        let (mut app, client) = start_ready_app(&temp.path().join("collection.db"));
        app.folder_thumb_pin_db = Some(Arc::new(
            FolderThumbPinDb::open_at(&FolderThumbPinDb::db_path()).unwrap(),
        ));
        let expected = std::collections::HashMap::from([
            (
                crate::path_key::normalize_keep_drive(&folder),
                FolderPinSource::File {
                    rel: "cover.png".into(),
                    kind: FileKind::Image,
                },
            ),
            (
                crate::path_key::normalize_keep_drive(&zip),
                FolderPinSource::ZipEntry {
                    zip_rel: String::new(),
                    entry: "chosen.png".into(),
                },
            ),
            (
                crate::path_key::normalize_keep_drive(&pdf),
                FolderPinSource::PdfPage {
                    pdf_rel: String::new(),
                    page: 4,
                },
            ),
            (
                crate::path_key::normalize_keep_drive(&archive),
                FolderPinSource::ZipEntry {
                    zip_rel: String::new(),
                    entry: "chosen.png".into(),
                },
            ),
            (
                crate::path_key::normalize_keep_drive(&epub),
                FolderPinSource::PdfPage {
                    pdf_rel: String::new(),
                    page: 2,
                },
            ),
        ]);
        for path in [&folder, &zip, &pdf, &archive, &epub] {
            app.folder_thumb_pin_db
                .as_ref()
                .unwrap()
                .set(
                    path,
                    &expected[&crate::path_key::normalize_keep_drive(path)],
                )
                .unwrap();
        }
        let snapshot = collection_with_sources(
            &client,
            &[
                (folder.clone(), CollectionResolvedKind::Folder),
                (zip.clone(), CollectionResolvedKind::Zip),
                (pdf.clone(), CollectionResolvedKind::Pdf),
                (archive.clone(), CollectionResolvedKind::ConvertibleArchive),
                (epub.clone(), CollectionResolvedKind::Pdf),
            ],
        );
        let install = prepare_collection_grid_install(
            &snapshot,
            &app.settings.grid_display_order,
            &app.settings,
            &AtomicBool::new(false),
            None,
            app.video_pin_db.as_ref().map(|db| db.mutation_stamp()),
            app.folder_thumb_pin_db
                .as_ref()
                .map(|db| db.mutation_stamp()),
            app.collection_thumbnail_source_epoch,
            super::super::page_edit_snapshot::PageEditAvailability::for_app(&app),
            app.page_edit_revision,
        )
        .unwrap();
        assert_eq!(
            install.thumbnail_sources.live.sources.folder_pin_map,
            expected
        );
        assert_eq!(
            install
                .thumbnail_sources
                .presentation
                .retained()
                .unwrap()
                .payload
                .folder_pin_map,
            expected
        );
        app.open_collection_grid(snapshot.collection_id(), None);
        wait_for_grid(&mut app, snapshot.collection_id());
        assert_eq!(app.folder_pin_map, expected);
        assert!(app.use_full_path_cache_keys());
        for (index, item) in app.items.iter().enumerate() {
            let request = collection_pin_request(&app, index);
            let key = request.cache_key_override.as_ref().unwrap();
            assert!(
                key.contains(crate::thumb_loader::CACHE_KEY_PIN_SUFFIX),
                "{item:?}: {key}"
            );
            match item {
                GridItem::Folder(_) => assert_eq!(request.path, folder.join("cover.png")),
                GridItem::ZipFile(_) | GridItem::ConvertibleArchive { .. } => {
                    assert_eq!(request.zip_entry.as_deref(), Some("chosen.png"))
                }
                GridItem::PdfFile(path) => {
                    assert_eq!(request.pdf_page, Some(if path == &pdf { 4 } else { 2 }))
                }
                _ => panic!("unexpected item {item:?}"),
            }
        }
        app.shutdown_collection_runtime_for_exit();
    }

    #[test]
    #[cfg(windows)]
    fn collection_nested_video_pin_seeds_match_requests_and_preserve_sibling_map() {
        use crate::folder_thumb_pins::{FileKind, FolderPinSource, FolderThumbPinDb};
        let _scope = crate::page_edit_write_epoch::TestEpochScope::fresh();
        let temp = tempfile::tempdir().unwrap();
        let outer = temp.path().join("Outer");
        let inner = outer.join("Inner");
        std::fs::create_dir_all(&inner).unwrap();
        let video = inner.join("Clip.mp4");
        std::fs::write(&video, b"video").unwrap();
        let (mut app, client) = start_ready_app(&temp.path().join("collection.db"));
        app.folder_thumb_pin_db = Some(Arc::new(
            FolderThumbPinDb::open_at(&FolderThumbPinDb::db_path()).unwrap(),
        ));
        app.folder_thumb_pin_db
            .as_ref()
            .unwrap()
            .set(
                &outer,
                &FolderPinSource::File {
                    rel: "Inner".into(),
                    kind: FileKind::Folder,
                },
            )
            .unwrap();
        app.folder_thumb_pin_db
            .as_ref()
            .unwrap()
            .set(
                &inner,
                &FolderPinSource::File {
                    rel: "Clip.mp4".into(),
                    kind: FileKind::Video,
                },
            )
            .unwrap();
        app.video_pin_db = Some(
            crate::video_pins::VideoPinDb::open_at(&crate::video_pins::VideoPinDb::db_path())
                .unwrap(),
        );
        let mut webp = Vec::new();
        image::codecs::webp::WebPEncoder::new_lossless(&mut webp)
            .encode(&[255, 0, 0, 255], 1, 1, image::ExtendedColorType::Rgba8)
            .unwrap();
        app.video_pin_db
            .as_ref()
            .unwrap()
            .set_pin(&video, 1.0, &webp)
            .unwrap();
        let sibling_map = std::collections::HashMap::from([(
            "sibling".into(),
            FolderPinSource::PdfPage {
                pdf_rel: String::new(),
                page: 7,
            },
        )]);
        let expected_sibling = sibling_map.clone();
        let sibling = app.build_window_context_for_test(1307, move |context| {
            context.folder_pin_map = sibling_map;
        });
        // Distinct spelling reaches the same normalized video key through both containers.
        let snapshot = collection_with_sources(
            &client,
            &[
                (outer.clone(), CollectionResolvedKind::Folder),
                (
                    PathBuf::from(inner.to_string_lossy().to_lowercase()),
                    CollectionResolvedKind::Folder,
                ),
            ],
        );
        app.open_collection_grid(snapshot.collection_id(), None);
        wait_for_grid(&mut app, snapshot.collection_id());
        let retained = app
            .top_level_grid_view
            .collection_session()
            .unwrap()
            .installed_presentation()
            .unwrap()
            .sources
            .retained()
            .unwrap();
        assert_eq!(retained.payload.video_folder_pin_seeds.len(), 2);
        assert!(
            retained
                .payload
                .video_folder_pin_seeds
                .iter()
                .all(|(_, bytes)| bytes.as_ref() == Some(&webp))
        );
        for index in 0..2 {
            let request = collection_pin_request(&app, index);
            let key = request.cache_key_override.unwrap();
            let cache = app
                .current_color_cache_map
                .as_ref()
                .unwrap()
                .read()
                .unwrap();
            assert_eq!(cache[&key].jpeg_data, webp);
            assert_eq!(
                cache[&key].folder_provenance,
                Some(crate::catalog::FolderThumbProvenance::Seeded)
            );
            assert_eq!(cache[&key].mtime, request.mtime);
            assert_eq!(cache[&key].file_size, request.file_size);
        }
        app.with_viewer_context(sibling, |context| {
            assert_eq!(context.folder_pin_map, expected_sibling)
        })
        .unwrap();
        app.shutdown_collection_runtime_for_exit();
    }

    #[test]
    fn collection_container_pin_mutation_refreshes_root_and_rejects_stale_delivery() {
        use crate::folder_thumb_pins::{FileKind, FolderPinSource, FolderThumbPinDb};
        let _scope = crate::page_edit_write_epoch::TestEpochScope::fresh();
        let temp = tempfile::tempdir().unwrap();
        let folder = temp.path().join("album");
        std::fs::create_dir(&folder).unwrap();
        for name in ["old.png", "new.png"] {
            std::fs::write(folder.join(name), b"image").unwrap();
        }
        let (mut app, client) = start_ready_app(&temp.path().join("collection.db"));
        app.folder_thumb_pin_db = Some(Arc::new(
            FolderThumbPinDb::open_at(&FolderThumbPinDb::db_path()).unwrap(),
        ));
        let pin = |name: &str| FolderPinSource::File {
            rel: name.into(),
            kind: FileKind::Image,
        };
        app.folder_thumb_pin_db
            .as_ref()
            .unwrap()
            .set(&folder, &pin("old.png"))
            .unwrap();
        let snapshot =
            collection_with_sources(&client, &[(folder.clone(), CollectionResolvedKind::Folder)]);
        app.open_collection_grid(snapshot.collection_id(), None);
        wait_for_grid(&mut app, snapshot.collection_id());
        let installed = app
            .top_level_grid_view
            .collection_session()
            .unwrap()
            .installed_presentation()
            .unwrap()
            .clone();
        let old_key = installed.reuse_key.clone();
        let stale_install = prepare_collection_grid_install(
            &snapshot,
            &app.settings.grid_display_order,
            &app.settings,
            &AtomicBool::new(false),
            None,
            app.video_pin_db.as_ref().map(|db| db.mutation_stamp()),
            app.folder_thumb_pin_db
                .as_ref()
                .map(|db| db.mutation_stamp()),
            app.collection_thumbnail_source_epoch,
            super::super::page_edit_snapshot::PageEditAvailability::for_app(&app),
            app.page_edit_revision,
        )
        .unwrap();
        assert!(
            stale_install
                .page_edits
                .as_ref()
                .unwrap()
                .0
                .stamp
                .is_some_and(|stamp| crate::page_edit_write_epoch::PAGE_EDIT_WRITES.accepts(stamp))
        );
        assert!(app.set_folder_thumb_pin(&folder, pin("new.png")));
        assert_ne!(
            old_key,
            app.collection_grid_prepare_reuse_key(snapshot.collection_id(), snapshot.revision())
        );
        // A completion racing a pin write must not be installed under the old stamp.
        let (sender, receiver) = std::sync::mpsc::channel();
        sender.send(Ok(stale_install)).unwrap();
        let stamp = app.collection_grid_stamp().unwrap();
        let generation = app.items_generation;
        app.top_level_grid_view
            .collection_session_mut()
            .unwrap()
            .load = CollectionGridLoadState::Preparing {
            stamp,
            exact_revision: snapshot.revision(),
            installed: Some(Arc::clone(&installed.prepared)),
            lease: crate::collection_store::CollectionReadLease::new(
                crate::collection_store::CollectionReadScope::app_global("grid-test"),
                Instant::now(),
                "prepare",
            ),
            cancel: Arc::new(AtomicBool::new(false)),
            receiver,
        };
        app.poll_collection_grid(&egui::Context::default());
        assert_eq!(app.items_generation, generation);
        wait_for_grid(&mut app, snapshot.collection_id());
        assert_eq!(collection_pin_request(&app, 0).path, folder.join("new.png"));
        assert!(app.remove_folder_thumb_pin(&folder));
        app.consume_folder_thumb_pin_dirty();
        app.poll_collection_grid(&egui::Context::default());
        wait_for_grid(&mut app, snapshot.collection_id());
        assert!(app.folder_pin_map.is_empty());
        assert!(
            !collection_pin_request(&app, 0)
                .cache_key_override
                .unwrap()
                .contains(crate::thumb_loader::CACHE_KEY_PIN_SUFFIX)
        );
        app.shutdown_collection_runtime_for_exit();
    }

    #[test]
    fn collection_container_pin_identity_and_seed_retention_cover_all_sources() {
        use crate::folder_thumb_pins::FolderPinSource;
        let prepare = |sources| {
            prepare_collection_grid_thumbnail_sources(sources, &AtomicBool::new(false)).unwrap()
        };
        let empty = prepare(CollectionGridThumbnailSources::default())
            .presentation
            .identity();
        let mut sources = CollectionGridThumbnailSources::default();
        sources.folder_pin_map.insert(
            "container".into(),
            FolderPinSource::PdfPage {
                pdf_rel: String::new(),
                page: 1,
            },
        );
        let pinned = prepare(sources.clone()).presentation.identity();
        assert_ne!(pinned, empty);
        sources.folder_pin_map.insert(
            "container".into(),
            FolderPinSource::PdfPage {
                pdf_rel: String::new(),
                page: 2,
            },
        );
        assert_ne!(prepare(sources.clone()).presentation.identity(), pinned);
        let seed = super::super::smart_folder::PreparedVideoFolderPinSeed {
            cache_key: "seed#pin:key".into(),
            video_path: PathBuf::from("clip.mp4"),
            mtime: 1,
            file_size: 5,
        };
        sources.video_folder_pin_seeds = Arc::new(vec![(seed.clone(), None)]);
        let missing = prepare(sources.clone()).presentation.identity();
        sources.video_folder_pin_seeds = Arc::new(vec![(
            seed,
            Some(vec![
                1;
                super::super::top_level_grid_view::MAX_RETAINED_COLLECTION_PIN_BLOB_BYTES
                    + 1
            ]),
        )]);
        let oversized = prepare(sources);
        assert!(matches!(
            oversized.presentation,
            CollectionGridPresentationSources::Oversized(_)
        ));
        assert_ne!(oversized.presentation.identity(), missing);
        assert!(oversized.live.folder_pin_cache.contains_key("seed#pin:key"));
        assert!(oversized.live.sources.video_folder_pin_seeds.is_empty());
    }

    #[test]
    fn phase_a_collection_mask_is_projected_for_accepted_revision() {
        let _test_epoch_scope = crate::page_edit_write_epoch::TestEpochScope::fresh();
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
        let _test_epoch_scope = crate::page_edit_write_epoch::TestEpochScope::fresh();
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
            Default::default(),
            Default::default(),
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
    fn phase_a2_collection_prepare_rejects_external_edit_after_worker_read() {
        let _test_epoch_scope = crate::page_edit_write_epoch::TestEpochScope::fresh();
        let temp = tempfile::tempdir().unwrap();
        let image = temp.path().join("stale.png");
        std::fs::write(&image, b"image").unwrap();
        let (mut app, client) = start_ready_app(&temp.path().join("collection.db"));
        let key = crate::adjustment_db::normalize_path(&image);
        app.mask_db
            .as_ref()
            .unwrap()
            .set(&key, &[true], &[], 1, 1)
            .unwrap();
        let snapshot = collection_with_sources(&client, &[(image, CollectionResolvedKind::Image)]);
        app.open_collection_grid(snapshot.collection_id(), None);
        wait_for_grid(&mut app, snapshot.collection_id());
        let installed = app
            .top_level_grid_view
            .collection_session()
            .and_then(|session| session.prepared().cloned())
            .unwrap();
        let old_snapshot = app.page_edit_snapshot.as_ref().unwrap().clone();
        let old_projection = old_snapshot.project(&app.items);
        let (sender, receiver) = std::sync::mpsc::channel();
        sender
            .send(Ok(CollectionGridPreparedInstall {
                prepared: (*installed).clone(),
                page_edits: Some((old_snapshot.clone(), old_projection)),
                retained_page_edits: Some(Arc::new(old_snapshot)),
                page_edit_revision: app.page_edit_revision,
                thumbnail_sources: prepare_collection_grid_thumbnail_sources(
                    CollectionGridThumbnailSources::default(),
                    &AtomicBool::new(false),
                )
                .unwrap(),
                auto_aspect_lookup: None,
                reuse_key: app.collection_grid_prepare_reuse_key(
                    installed.collection_id,
                    installed.collection_revision,
                ),
            }))
            .unwrap();
        app.mask_db.as_ref().unwrap().delete(&key).unwrap();
        let stamp = app.collection_grid_stamp().unwrap();
        let generation = app.items_generation;
        app.top_level_grid_view
            .collection_session_mut()
            .unwrap()
            .load = CollectionGridLoadState::Preparing {
            stamp,
            exact_revision: installed.collection_revision,
            lease: crate::collection_store::CollectionReadLease::new(
                crate::collection_store::CollectionReadScope::app_global("edit-stamp-test"),
                Instant::now(),
                "prepare",
            ),
            installed: Some(Arc::clone(&installed)),
            cancel: Arc::new(AtomicBool::new(false)),
            receiver,
        };
        app.poll_collection_grid(&egui::Context::default());
        assert_eq!(app.items_generation, generation, "stale edit was installed");
        assert!(matches!(
            app.top_level_grid_view.collection_session().unwrap().load,
            CollectionGridLoadState::RequestNeeded { .. }
        ));
        wait_for_grid(&mut app, snapshot.collection_id());
        assert!(!app.mask_pages.contains(&0));
        app.shutdown_collection_runtime_for_exit();
    }

    #[test]
    fn root_auto_aspect_restores_before_real_rows_and_excludes_unsampleable_items() {
        let _test_epoch_scope = crate::page_edit_write_epoch::TestEpochScope::fresh();
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
        let _test_epoch_scope = crate::page_edit_write_epoch::TestEpochScope::fresh();
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
        let _test_epoch_scope = crate::page_edit_write_epoch::TestEpochScope::fresh();
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
        let _test_epoch_scope = crate::page_edit_write_epoch::TestEpochScope::fresh();
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
        assert!(
            matches!(
                app.folder_history_back_target(),
                Some(super::super::FolderNavHistoryTarget::Collection(restore))
                    if restore.identity.collection_id == first.collection_id()
            ),
            "back={:?}, current={:?}",
            app.folder_history_back_target(),
            app.folder_nav_current_target()
        );

        let renamed_first = recv(
            client
                .rename_collection(
                    first.collection_id(),
                    first.revision(),
                    "First renamed".into(),
                )
                .expect("rename first while its history entry is parked"),
        );

        let back = app
            .folder_history_back_target()
            .cloned()
            .expect("back to first");
        assert!(app.start_collection_history_transition(
            back.clone(),
            super::super::CollectionHistoryIntent::Replay {
                direction: super::super::FolderHistoryDirection::Back,
                target: back,
            },
            None,
        ));
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
        assert!(
            matches!(
                app.folder_history_back_target(),
                Some(super::super::FolderNavHistoryTarget::Collection(restore))
                    if restore.identity.collection_id == first.collection_id()
            ),
            "back={:?}, current={:?}",
            app.folder_history_back_target(),
            app.folder_nav_current_target()
        );
        let back = app
            .folder_history_back_target()
            .cloned()
            .expect("back to first again");
        assert!(app.start_collection_history_transition(
            back.clone(),
            super::super::CollectionHistoryIntent::Replay {
                direction: super::super::FolderHistoryDirection::Back,
                target: back,
            },
            None,
        ));
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
        let _test_epoch_scope = crate::page_edit_write_epoch::TestEpochScope::fresh();
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
        let _test_epoch_scope = crate::page_edit_write_epoch::TestEpochScope::fresh();
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
        let _test_epoch_scope = crate::page_edit_write_epoch::TestEpochScope::fresh();
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
        let _test_epoch_scope = crate::page_edit_write_epoch::TestEpochScope::fresh();
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
        let _test_epoch_scope = crate::page_edit_write_epoch::TestEpochScope::fresh();
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
        let _test_epoch_scope = crate::page_edit_write_epoch::TestEpochScope::fresh();
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
        let _test_epoch_scope = crate::page_edit_write_epoch::TestEpochScope::fresh();
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
        let _test_epoch_scope = crate::page_edit_write_epoch::TestEpochScope::fresh();
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
        let _test_epoch_scope = crate::page_edit_write_epoch::TestEpochScope::fresh();
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
        let _test_epoch_scope = crate::page_edit_write_epoch::TestEpochScope::fresh();
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

    #[cfg(windows)]
    fn mount_detached_collection_media(
        app: &mut App,
        source: PathBuf,
        window_id: u64,
        audio: bool,
    ) -> u64 {
        app.fullscreen_idx = Some(0);
        app.viewer_presentation = super::super::ViewerPresentation::DetachedWindow;
        app.set_detached_window_binding_for_test(Some(window_id));
        app.begin_mounted_detached_session_for_test(
            window_id,
            if audio {
                super::super::DetachedSource::Audio
            } else {
                super::super::DetachedSource::Video
            },
        );
        app.fs_cache.insert(
            0,
            super::super::FsCacheEntry::Video {
                player: Box::new(crate::video::VideoPlayer::disconnected_for_test(
                    source, 12.0,
                )),
                load_seq: 0,
            },
        );
        app.items_generation
    }

    #[cfg(windows)]
    fn assert_detached_collection_media_binding(
        app: &mut App,
        window_id: u64,
        collection_id: CollectionId,
        items_generation: u64,
    ) {
        app.with_window_viewer_context(window_id, |window| {
            assert_eq!(window.fullscreen_idx, Some(0));
            assert_eq!(window.items_generation, items_generation);
            assert_eq!(window.items.len(), 1);
            assert!(matches!(
                window.top_level_grid_view.surface(),
                TopLevelGridSurface::Collection(identity) if identity.collection_id == collection_id
            ));
            assert_eq!(
                window
                    .top_level_grid_view
                    .collection_session()
                    .unwrap()
                    .installed_items_generation,
                Some(items_generation)
            );
            assert!(matches!(
                window.fs_cache.get(&0),
                Some(super::super::FsCacheEntry::Video { .. })
            ));
        })
        .expect("detached media window must retain its old viewer context");
    }

    #[test]
    #[cfg(windows)]
    fn collection_open_transfers_video_and_audio_with_their_old_binding() {
        for audio in [false, true] {
            let _test_epoch_scope = crate::page_edit_write_epoch::TestEpochScope::fresh();
            let temp = tempfile::tempdir().unwrap();
            let source = temp
                .path()
                .join(if audio { "song.flac" } else { "movie.mp4" });
            std::fs::write(&source, b"media").unwrap();
            let (mut app, client) = start_ready_app(&temp.path().join("collection.db"));
            app.active_quick_folder_slot = None;
            let old = collection_with_sources(
                &client,
                &[(
                    source.clone(),
                    if audio {
                        CollectionResolvedKind::Audio
                    } else {
                        CollectionResolvedKind::Video
                    },
                )],
            );
            let next = recv(client.create_collection("Next".into()).unwrap());
            app.open_collection_grid(old.collection_id(), None);
            wait_for_grid(&mut app, old.collection_id());
            let generation = mount_detached_collection_media(&mut app, source, 1_304, audio);
            let sibling_path = temp.path().join("sibling.mp4");
            let sibling = app.build_window_context_for_test(1_310, move |window| {
                window.items.push(GridItem::Video(sibling_path.clone()));
                window.fullscreen_idx = Some(0);
                window.fs_cache.insert(
                    0,
                    super::super::FsCacheEntry::Video {
                        player: Box::new(crate::video::VideoPlayer::disconnected_for_test(
                            sibling_path,
                            7.0,
                        )),
                        load_seq: 0,
                    },
                );
            });
            let sibling_generation = app
                .with_viewer_context(sibling, |window| window.items_generation)
                .unwrap();

            app.open_collection_grid_from_navigation(next.collection_id());
            assert!(app.fullscreen_idx.is_none());
            assert_eq!(app.address, "コレクションを読み込み中…");
            assert!(
                matches!(app.folder_history_back_target(), Some(super::super::FolderNavHistoryTarget::Collection(restore)) if restore.identity.collection_id == old.collection_id())
            );
            assert_detached_collection_media_binding(
                &mut app,
                1_304,
                old.collection_id(),
                generation,
            );
            wait_for_grid(&mut app, next.collection_id());
            assert_detached_collection_media_binding(
                &mut app,
                1_304,
                old.collection_id(),
                generation,
            );
            app.with_viewer_context(sibling, |window| {
                assert_eq!(window.items_generation, sibling_generation);
                assert_eq!(window.fullscreen_idx, Some(0));
                assert!(matches!(
                    window.fs_cache.get(&0),
                    Some(super::super::FsCacheEntry::Video { .. })
                ));
            })
            .unwrap();
            app.shutdown_collection_runtime_for_exit();
        }
    }

    #[test]
    #[cfg(windows)]
    fn collection_parent_return_transfers_detached_video_before_loading_root() {
        let _test_epoch_scope = crate::page_edit_write_epoch::TestEpochScope::fresh();
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("movie.mp4");
        std::fs::write(&source, b"media").unwrap();
        let (mut app, client) = start_ready_app(&temp.path().join("collection.db"));
        app.active_quick_folder_slot = None;
        let root =
            collection_with_sources(&client, &[(source.clone(), CollectionResolvedKind::Video)]);
        app.open_collection_grid(root.collection_id(), None);
        wait_for_grid(&mut app, root.collection_id());
        let TopLevelGridRestore::Collection(restore) =
            app.collection_grid_restore_snapshot().unwrap()
        else {
            panic!("root restore")
        };
        let anchor = restore.viewport_anchor.clone().unwrap_or_else(|| {
            let entry = &app
                .top_level_grid_view
                .collection_session()
                .unwrap()
                .prepared()
                .unwrap()
                .entries[0];
            CollectionGridViewportAnchor {
                entry_id: entry.entry_id,
                source_key: entry.source_key.clone(),
            }
        });
        app.commit_collection_grid_source_open(anchor, temp.path().to_path_buf());
        app.current_folder = Some(temp.path().to_path_buf());
        let generation = mount_detached_collection_media(&mut app, source, 1_307, false);

        app.apply_collection_input_nav(restore, false);
        assert!(app.fullscreen_idx.is_none());
        assert!(matches!(
            app.folder_history_back_target(),
            Some(super::super::FolderNavHistoryTarget::CollectionPhysical(_))
        ));
        assert_detached_collection_media_binding(&mut app, 1_307, root.collection_id(), generation);
        wait_for_grid(&mut app, root.collection_id());
        assert_detached_collection_media_binding(&mut app, 1_307, root.collection_id(), generation);
        app.shutdown_collection_runtime_for_exit();
    }

    #[test]
    #[cfg(windows)]
    fn collection_history_root_adoption_transfers_detached_video_only_after_prepare() {
        let _test_epoch_scope = crate::page_edit_write_epoch::TestEpochScope::fresh();
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("movie.mp4");
        std::fs::write(&source, b"media").unwrap();
        let (mut app, client) = start_ready_app(&temp.path().join("collection.db"));
        let old =
            collection_with_sources(&client, &[(source.clone(), CollectionResolvedKind::Video)]);
        let next = recv(client.create_collection("History target".into()).unwrap());
        app.open_collection_grid(old.collection_id(), None);
        wait_for_grid(&mut app, old.collection_id());
        let generation = mount_detached_collection_media(&mut app, source, 1_308, false);
        let restore = CollectionGridRestore {
            identity: CollectionGridIdentity {
                collection_id: next.collection_id(),
            },
            revision_at_open: next.revision(),
            viewport_anchor: None,
        };
        let mut pending = app
            .start_collection_history_prepare(restore.clone())
            .unwrap();
        let prepared = loop {
            match app.poll_collection_history_prepare(&mut pending) {
                CollectionHistoryPreparePoll::Pending => {
                    std::thread::sleep(Duration::from_millis(2))
                }
                CollectionHistoryPreparePoll::Ready(prepared) => break prepared,
                CollectionHistoryPreparePoll::Failed(error) => panic!("{error}"),
            }
        };
        assert_eq!(app.fullscreen_idx, Some(0));
        assert!(matches!(
            app.fs_cache.get(&0),
            Some(super::super::FsCacheEntry::Video { .. })
        ));
        assert!(app.adopt_collection_history_root(restore, prepared, None));
        assert!(app.fullscreen_idx.is_none());
        assert_detached_collection_media_binding(&mut app, 1_308, old.collection_id(), generation);
        assert!(
            matches!(app.top_level_grid_view.surface(), TopLevelGridSurface::Collection(identity) if identity.collection_id == next.collection_id())
        );
        app.shutdown_collection_runtime_for_exit();
    }

    #[test]
    #[cfg(windows)]
    fn failed_or_cancelled_history_prepare_does_not_transfer_detached_media() {
        for fail in [false, true] {
            let _test_epoch_scope = crate::page_edit_write_epoch::TestEpochScope::fresh();
            let temp = tempfile::tempdir().unwrap();
            let source = temp.path().join("movie.mp4");
            std::fs::write(&source, b"media").unwrap();
            let (mut app, client) = start_ready_app(&temp.path().join("collection.db"));
            app.active_quick_folder_slot = None;
            let old = collection_with_sources(
                &client,
                &[(source.clone(), CollectionResolvedKind::Video)],
            );
            let target = recv(client.create_collection("History target".into()).unwrap());
            poll_until(&mut app, "target was not cataloged", |app| {
                app.collection_catalog_contains(target.collection_id())
            });
            app.open_collection_grid(old.collection_id(), None);
            wait_for_grid(&mut app, old.collection_id());
            let generation = mount_detached_collection_media(&mut app, source, 1_311, false);
            let restore = CollectionGridRestore {
                identity: CollectionGridIdentity {
                    collection_id: target.collection_id(),
                },
                revision_at_open: target.revision(),
                viewport_anchor: None,
            };
            let history_target = super::super::FolderNavHistoryTarget::Collection(restore);
            app.folder_nav_back_stack.push(history_target.clone());
            let history = app.folder_nav_history_snapshot();
            let surface_generation = app.top_level_grid_view.generation();
            assert!(app.start_collection_history_transition(
                history_target.clone(),
                super::super::CollectionHistoryIntent::Replay {
                    direction: super::super::FolderHistoryDirection::Back,
                    target: history_target,
                },
                None,
            ));
            if fail {
                let mut transition = app
                    .top_level_grid_view
                    .take_history_navigation_transition()
                    .unwrap();
                let super::super::HistoryNavigationTransition::Collection(request) =
                    &mut transition
                else {
                    panic!("collection request")
                };
                let super::super::CollectionHistoryPhase::Preparing(prepare) = &mut request.phase
                else {
                    panic!("prepare phase")
                };
                prepare.phase = CollectionHistoryPreparePhase::Finished;
                app.top_level_grid_view
                    .set_history_navigation_transition(Some(transition));
                app.poll_collection_history_transition(&egui::Context::default());
            } else {
                app.replace_history_navigation_transition(None);
            }
            assert_eq!(app.top_level_grid_view.generation(), surface_generation);
            assert_eq!(app.items_generation, generation);
            assert_eq!(app.fullscreen_idx, Some(0));
            assert!(matches!(
                app.fs_cache.get(&0),
                Some(super::super::FsCacheEntry::Video { .. })
            ));
            assert_eq!(
                app.folder_nav_history_snapshot().back_stack,
                history.back_stack
            );
            assert_eq!(
                app.folder_nav_history_snapshot().forward_stack,
                history.forward_stack
            );
            app.shutdown_collection_runtime_for_exit();
        }
    }

    #[test]
    #[cfg(windows)]
    fn collection_open_from_global_search_does_not_record_back_after_media_transfer() {
        let _test_epoch_scope = crate::page_edit_write_epoch::TestEpochScope::fresh();
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("movie.mp4");
        std::fs::write(&source, b"media").unwrap();
        let (mut app, client) = start_ready_app(&temp.path().join("collection.db"));
        app.active_quick_folder_slot = None;
        let old =
            collection_with_sources(&client, &[(source.clone(), CollectionResolvedKind::Video)]);
        let next = recv(client.create_collection("Next".into()).unwrap());
        app.open_collection_grid(old.collection_id(), None);
        wait_for_grid(&mut app, old.collection_id());
        let generation = mount_detached_collection_media(&mut app, source, 1_309, false);
        app.global_search.active = true;
        let history = app.folder_nav_history_snapshot();

        app.open_collection_grid_from_navigation(next.collection_id());
        assert_eq!(
            app.folder_nav_history_snapshot().back_stack,
            history.back_stack
        );
        assert_eq!(
            app.folder_nav_history_snapshot().forward_stack,
            history.forward_stack
        );
        assert_detached_collection_media_binding(&mut app, 1_309, old.collection_id(), generation);
        app.shutdown_collection_runtime_for_exit();
    }

    #[test]
    #[cfg(windows)]
    fn collection_open_uses_existing_still_pdf_park_and_linked_close_policy() {
        for (pdf, independent) in [(false, true), (true, true), (false, false)] {
            let _test_epoch_scope = crate::page_edit_write_epoch::TestEpochScope::fresh();
            let temp = tempfile::tempdir().unwrap();
            let source = temp.path().join(if pdf { "book.pdf" } else { "page.png" });
            std::fs::write(&source, b"source").unwrap();
            std::fs::write(temp.path().join("page.png"), b"image").unwrap();
            let (mut app, client) = start_ready_app(&temp.path().join("collection.db"));
            let old = collection_with_sources(
                &client,
                &[(temp.path().join("page.png"), CollectionResolvedKind::Image)],
            );
            let next = recv(client.create_collection("Next".into()).unwrap());
            app.open_collection_grid(old.collection_id(), None);
            wait_for_grid(&mut app, old.collection_id());
            if pdf {
                app.items[0] = GridItem::PdfPage {
                    pdf_path: source.clone(),
                    page_num: 0,
                    content_type: None,
                };
            }
            let ctx = egui::Context::default();
            let pixels = egui::ColorImage::new([2, 1], vec![egui::Color32::WHITE; 2]);
            let tex = ctx.load_texture(
                "collection_still_park",
                pixels.clone(),
                egui::TextureOptions::LINEAR,
            );
            app.fs_cache.insert(
                0,
                super::super::FsCacheEntry::Static {
                    tex,
                    pixels: Arc::new(pixels),
                    source_dims: Some([2, 1]),
                    load_seq: 0,
                    animation: crate::fs_animation::StaticAnimationState::Still,
                },
            );
            app.settings.detached_viewer_open_images_in_window = independent;
            app.fullscreen_idx = Some(0);
            app.viewer_presentation = super::super::ViewerPresentation::DetachedWindow;
            app.set_detached_window_binding_for_test(Some(1_312));
            app.begin_mounted_detached_session_for_test(
                1_312,
                if pdf {
                    super::super::DetachedSource::Book
                } else {
                    super::super::DetachedSource::Image
                },
            );

            app.open_collection_grid_from_navigation(next.collection_id());
            assert!(app.fullscreen_idx.is_none());
            assert_eq!(app.detached_image_windows.len(), usize::from(independent));
            if independent {
                assert_eq!(app.detached_image_windows[0].id, 1_312);
                assert_eq!(
                    app.detached_window_state(1_312),
                    Some(super::super::DetachedWindowState::Parked)
                );
            }
            wait_for_grid(&mut app, next.collection_id());
            app.shutdown_collection_runtime_for_exit();
        }
    }

    #[test]
    #[cfg(windows)]
    fn failed_explicit_collection_open_keeps_detached_media_and_committed_history() {
        let _test_epoch_scope = crate::page_edit_write_epoch::TestEpochScope::fresh();
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("movie.mp4");
        std::fs::write(&source, b"media").unwrap();
        let (mut app, client) = start_ready_app(&temp.path().join("collection.db"));
        app.active_quick_folder_slot = None;
        let old =
            collection_with_sources(&client, &[(source.clone(), CollectionResolvedKind::Video)]);
        app.open_collection_grid(old.collection_id(), None);
        wait_for_grid(&mut app, old.collection_id());
        let generation = mount_detached_collection_media(&mut app, source, 1_305, false);

        // An unavailable actor is a terminal Failed presentation; an unknown UUID is Deleted.
        app.shutdown_collection_runtime_for_exit();
        app.open_collection_grid_from_navigation(CollectionId::new());
        assert!(matches!(
            app.top_level_grid_view.collection_session().unwrap().load,
            CollectionGridLoadState::Failed {
                installed: None,
                ..
            }
        ));
        assert!(app.collection_grid_empty_message().is_some());
        assert!(
            matches!(app.folder_history_back_target(), Some(super::super::FolderNavHistoryTarget::Collection(restore)) if restore.identity.collection_id == old.collection_id())
        );
        assert_detached_collection_media_binding(&mut app, 1_305, old.collection_id(), generation);
    }

    #[test]
    #[cfg(windows)]
    fn blocked_collection_open_leaves_history_and_old_context_unchanged() {
        let _test_epoch_scope = crate::page_edit_write_epoch::TestEpochScope::fresh();
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("movie.mp4");
        std::fs::write(&source, b"media").unwrap();
        let (mut app, client) = start_ready_app(&temp.path().join("collection.db"));
        app.active_quick_folder_slot = None;
        let old =
            collection_with_sources(&client, &[(source.clone(), CollectionResolvedKind::Video)]);
        let next = recv(client.create_collection("Next".into()).unwrap());
        app.open_collection_grid(old.collection_id(), None);
        wait_for_grid(&mut app, old.collection_id());
        let generation = mount_detached_collection_media(&mut app, source, 1_306, false);
        app.fs_nav_locked_gen = Some(generation);
        let history = app.folder_nav_history_snapshot();
        let surface_generation = app.top_level_grid_view.generation();
        let old_items = app.items.clone();

        app.open_collection_grid_from_navigation(next.collection_id());
        assert_eq!(app.top_level_grid_view.generation(), surface_generation);
        assert_eq!(app.items, old_items);
        assert_eq!(
            app.folder_nav_history_snapshot().back_stack,
            history.back_stack
        );
        assert_eq!(
            app.folder_nav_history_snapshot().forward_stack,
            history.forward_stack
        );
        assert_eq!(app.fullscreen_idx, Some(0));
        assert!(matches!(
            app.fs_cache.get(&0),
            Some(super::super::FsCacheEntry::Video { .. })
        ));
        app.fs_nav_locked_gen = None;
        app.shutdown_collection_runtime_for_exit();
    }

    #[test]
    fn same_root_open_retains_installed_items_while_fullscreen_owns_their_indices() {
        let _test_epoch_scope = crate::page_edit_write_epoch::TestEpochScope::fresh();
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("page.png");
        std::fs::write(&source, b"image").unwrap();
        let (mut app, client) = start_ready_app(&temp.path().join("collection.db"));
        let snapshot = collection_with_sources(&client, &[(source, CollectionResolvedKind::Image)]);
        app.open_collection_grid(snapshot.collection_id(), None);
        wait_for_grid(&mut app, snapshot.collection_id());
        let generation = app.items_generation;
        let surface_generation = app.top_level_grid_view.generation();
        let rows = app.items.clone();
        app.fullscreen_idx = Some(0);

        app.open_collection_grid(snapshot.collection_id(), None);
        assert_eq!(app.items_generation, generation);
        assert_eq!(app.top_level_grid_view.generation(), surface_generation);
        assert_eq!(app.items, rows);
        assert!(matches!(
            app.top_level_grid_view.collection_session().unwrap().load,
            CollectionGridLoadState::RequestNeeded {
                installed: Some(_),
                ..
            }
        ));
        assert!(app.collection_grid_refresh_waits_for_viewer());
        app.shutdown_collection_runtime_for_exit();
    }

    #[test]
    fn history_prepare_keeps_old_grid_until_root_adoption() {
        let _test_epoch_scope = crate::page_edit_write_epoch::TestEpochScope::fresh();
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("page.png");
        std::fs::write(&source, b"image").unwrap();
        let (mut app, client) = start_ready_app(&temp.path().join("collection.db"));
        let snapshot = collection_with_sources(&client, &[(source, CollectionResolvedKind::Image)]);
        poll_until(&mut app, "new Collection was not cataloged", |app| {
            app.collection_catalog_revision(snapshot.collection_id()) == Some(snapshot.revision())
        });
        let restore = CollectionGridRestore {
            identity: CollectionGridIdentity {
                collection_id: snapshot.collection_id(),
            },
            revision_at_open: snapshot.revision(),
            viewport_anchor: None,
        };
        let old_surface = app.top_level_grid_view.surface().clone();
        let old_items = app.items.clone();
        let mut pending = app
            .start_collection_history_prepare(restore.clone())
            .expect("offscreen collection read");
        let install = loop {
            match app.poll_collection_history_prepare(&mut pending) {
                CollectionHistoryPreparePoll::Pending => {
                    std::thread::sleep(Duration::from_millis(2))
                }
                CollectionHistoryPreparePoll::Ready(install) => break install,
                CollectionHistoryPreparePoll::Failed(message) => panic!("{message}"),
            }
        };
        assert_eq!(app.top_level_grid_view.surface(), &old_surface);
        assert_eq!(app.items, old_items);
        app.adopt_collection_history_root(restore, install, None);
        assert!(matches!(
            app.top_level_grid_view.surface(),
            TopLevelGridSurface::Collection(_)
        ));
        assert_eq!(app.items.len(), 1);
        app.shutdown_collection_runtime_for_exit();
    }

    #[test]
    fn explicit_collection_child_and_descendant_commit_distinct_history_locations() {
        let _test_epoch_scope = crate::page_edit_write_epoch::TestEpochScope::fresh();
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("book");
        let descendant = root.join("chapter");
        std::fs::create_dir_all(&descendant).unwrap();
        let (mut app, client) = start_ready_app(&temp.path().join("collection.db"));
        app.active_quick_folder_slot = None;
        app.settings.sidecar_backup_enabled = false;
        app.settings.tag_sidecar_backup_enabled = false;
        let snapshot =
            collection_with_sources(&client, &[(root.clone(), CollectionResolvedKind::Folder)]);
        app.open_collection_grid(snapshot.collection_id(), None);
        wait_for_grid(&mut app, snapshot.collection_id());

        let owner = app
            .collection_grid_physical_load_owner(0, &root)
            .expect("root cell owner");
        let scan =
            super::super::folder_scan::scan_directory_with_settings(&root, &app.settings).unwrap();
        assert!(app.load_folder_with_scan_owned(
            root.clone(),
            Some(scan),
            super::super::OpenRequestOwner::CollectionGridPhysical(owner),
        ));
        assert!(matches!(
            app.folder_history_back_target(),
            Some(super::super::FolderNavHistoryTarget::Collection(restore))
                if restore.identity.collection_id == snapshot.collection_id()
        ));

        let index = app
            .items
            .iter()
            .position(|item| matches!(item, GridItem::Folder(path) if path == &descendant))
            .expect("child row");
        let owner = app
            .collection_grid_physical_load_owner(index, &descendant)
            .expect("descendant owner");
        let scan =
            super::super::folder_scan::scan_directory_with_settings(&descendant, &app.settings)
                .unwrap();
        assert!(app.load_folder_with_scan_owned(
            descendant.clone(),
            Some(scan),
            super::super::OpenRequestOwner::CollectionGridPhysical(owner),
        ));
        assert!(matches!(
            app.folder_history_back_target(),
            Some(super::super::FolderNavHistoryTarget::CollectionPhysical(restore))
                if crate::folder_tree::path_eq(&restore.visible_path, &root)
                    && restore.root.identity.collection_id == snapshot.collection_id()
        ));
        assert!(matches!(
            app.collection_grid_current_restore_snapshot(),
            Some(TopLevelGridRestore::CollectionPhysical(restore))
                if crate::folder_tree::path_eq(&restore.visible_path, &descendant)
        ));
        app.shutdown_collection_runtime_for_exit();
    }

    #[test]
    fn direct_collection_zip_waits_for_preflight_before_child_and_history_commit() {
        let _test_epoch_scope = crate::page_edit_write_epoch::TestEpochScope::fresh();
        let temp = tempfile::tempdir().unwrap();
        let zip_path = temp.path().join("book.zip");
        let cached = temp.path().join("book-cached.zip");
        write_single_page_zip(&zip_path);
        write_single_page_zip(&cached);
        let (mut app, client) = start_ready_app(&temp.path().join("collection.db"));
        app.active_quick_folder_slot = None;
        let metadata = std::fs::metadata(&zip_path).unwrap();
        app.archive_cache_db
            .as_ref()
            .expect("archive cache DB")
            .record(
                &zip_path,
                crate::ui_helpers::mtime_secs(&metadata),
                metadata.len() as i64,
                crate::archive_converter::ArchiveFormat::Zip,
                &cached,
                std::fs::metadata(&cached).unwrap().len() as i64,
                1,
                false,
            )
            .unwrap();
        let logical_pin = crate::folder_thumb_pins::FolderPinSource::ZipEntry {
            zip_rel: String::new(),
            entry: "page.jpg".into(),
        };
        app.folder_thumb_pin_db
            .as_ref()
            .expect("folder thumbnail pin DB")
            .set(&zip_path, &logical_pin)
            .unwrap();
        let collection =
            collection_with_sources(&client, &[(zip_path.clone(), CollectionResolvedKind::Zip)]);
        app.open_collection_grid(collection.collection_id(), None);
        wait_for_grid(&mut app, collection.collection_id());

        let before_items = app.items.clone();
        let before_history = app.folder_nav_history_snapshot();
        let owner = app
            .collection_grid_physical_load_owner(0, &zip_path)
            .expect("Collection ZIP owner");
        assert!(app.load_folder_with_scan_owned(
            zip_path.clone(),
            None,
            super::super::OpenRequestOwner::CollectionGridPhysical(owner),
        ));
        assert!(matches!(
            app.top_level_grid_view
                .collection_session()
                .unwrap()
                .position,
            CollectionGridPosition::Root
        ));
        assert_eq!(app.items, before_items);
        assert_eq!(app.folder_nav_back_stack, before_history.back_stack);
        assert!(
            app.top_level_grid_view
                .history_navigation_transition()
                .is_some()
        );

        poll_until(&mut app, "Collection ZIP did not adopt", |app| {
            app.archive_source_override
                .as_deref()
                .is_some_and(|path| crate::folder_tree::path_eq(path, &zip_path))
                && app
                    .top_level_grid_view
                    .collection_session()
                    .is_some_and(|session| {
                        matches!(
                            session.position,
                            CollectionGridPosition::PhysicalSource { .. }
                        )
                    })
        });
        assert_eq!(app.current_folder.as_deref(), Some(cached.as_path()));
        assert_eq!(app.address, zip_path.to_string_lossy());
        assert_eq!(app.pin_container_key().as_deref(), Some(zip_path.as_path()));
        assert_eq!(app.folder_thumb_pin_for(&zip_path), Some(&logical_pin));
        assert!(matches!(
            app.folder_history_back_target(),
            Some(super::super::FolderNavHistoryTarget::Collection(restore))
                if restore.identity.collection_id == collection.collection_id()
        ));
        app.shutdown_collection_runtime_for_exit();
    }

    fn assert_real_virtual_history(zip: bool, collection: bool, epub: bool) {
        let _test_epoch_scope = crate::page_edit_write_epoch::TestEpochScope::fresh();
        let temp = tempfile::tempdir().unwrap();
        let saved = temp.path().join("before-book");
        std::fs::create_dir(&saved).unwrap();
        let source = temp.path().join(if zip {
            "book.zip"
        } else if epub {
            "book.epub"
        } else {
            "book.pdf"
        });
        let mut epub_pin = None;
        let mut epub_worker = None;
        let mut epub_stamp = None;
        let pdf_pages: Option<crate::pdf_loader::PdfEnumerateResult> = if zip {
            write_nested_history_zip(&source);
            None
        } else if epub {
            let (pin, worker, stamp) = real_cached_epub_history_fixture(&source, temp.path());
            epub_pin = Some(pin);
            epub_worker = Some(worker);
            epub_stamp = Some(stamp);
            None
        } else {
            write_two_page_history_pdf(&source);
            Some(read_real_pdf_pages_for_history(&source).into())
        };
        let (mut app, client) = start_ready_app(&temp.path().join("collection.db"));
        app.active_quick_folder_slot = None;
        app.settings.sidecar_backup_enabled = false;
        app.settings.tag_sidecar_backup_enabled = false;
        app.settings.follow_document_reading_direction = epub;
        app.current_folder = Some(saved);

        let root = if collection {
            let snapshot = collection_with_sources(
                &client,
                &[(
                    source.clone(),
                    if zip {
                        CollectionResolvedKind::Zip
                    } else {
                        CollectionResolvedKind::Pdf
                    },
                )],
            );
            app.open_collection_grid_from_navigation(snapshot.collection_id());
            wait_for_grid(&mut app, snapshot.collection_id());
            poll_until(
                &mut app,
                "Collection root transition did not settle",
                |app| {
                    app.top_level_grid_view
                        .history_navigation_transition()
                        .is_none()
                },
            );
            app.current_folder.clone()
        } else {
            let key = crate::adjustment_db::normalize_path(&source);
            app.rating_db.as_ref().unwrap().set(&key, 1).unwrap();
            app.enter_rating_view_from_menu(1);
            let deadline = Instant::now() + Duration::from_secs(15);
            while app.rating_view_pending.is_some() {
                assert!(Instant::now() < deadline, "Rating root did not settle");
                app.poll_rating_view();
                std::thread::sleep(Duration::from_millis(5));
            }
            assert!(app.items_are_rating_view);
            assert!(
                app.items.iter().any(|item| item
                    .drag_source_path()
                    .is_some_and(|path| crate::adjustment_db::normalize_path(path)
                        == crate::adjustment_db::normalize_path(&source))),
                "Rating source row missing: {:?}",
                app.items
            );
            app.current_folder.clone()
        };
        let root_target = app.folder_nav_current_target().expect("root location");

        if collection {
            let index = app
                .items
                .iter()
                .position(|item| {
                    item.drag_source_path()
                        .is_some_and(|path| crate::folder_tree::path_eq(path, &source))
                })
                .unwrap();
            let owner = app
                .collection_grid_physical_load_owner(index, &source)
                .unwrap();
            assert!(
                app.main_folder_history_available(),
                "Collection root must own main history"
            );
            assert!(app.load_folder_with_scan_owned(
                source.clone(),
                None,
                super::super::OpenRequestOwner::CollectionGridPhysical(owner),
            ));
            app.settle_open_path_classification_for_test();
            assert!(
                matches!(
                    app.top_level_grid_view.history_navigation_transition(),
                    Some(super::super::HistoryNavigationTransition::Physical(_))
                ),
                "Collection open must have a staged physical request: source={source:?} items={:?} surface={:?}",
                app.items,
                app.top_level_grid_view.surface()
            );
        } else {
            let owner = app.rating_view_physical_load_owner(&source).unwrap();
            assert!(app.start_rating_physical_open(owner));
        }
        poll_real_history_load(&mut app, Some(&source), pdf_pages.as_ref());
        assert!(
            app.pdf_enumerate_pending.is_none(),
            "staged PDF/EPUB adoption has no second owner"
        );
        if epub {
            assert_cached_epub_worker_adoption(&mut app, &source, epub_stamp.as_ref().unwrap());
            assert_eq!(
                epub_worker.as_ref().unwrap().direction_requests(),
                vec![false]
            );
        }
        let source_target = app
            .folder_nav_current_target()
            .expect("adopted book location");
        assert_ne!(source_target, root_target);
        assert_eq!(app.folder_history_back_target(), Some(&root_target));
        let outer_back = app.folder_nav_back_stack.clone();
        let outer_forward = app.folder_nav_forward_stack.clone();

        if zip {
            assert!(app.items.iter().any(|item| matches!(item, GridItem::ZipDir { dir_prefix, .. } if dir_prefix == "chapter/")));
            app.zip_nav_enter("chapter/");
            assert!(app.items.iter().any(|item| matches!(item, GridItem::ZipImage { entry_name, .. } if entry_name == "chapter/second.png")));
            assert_eq!(app.folder_nav_current_target(), Some(source_target.clone()));
            assert_eq!(app.folder_nav_back_stack, outer_back);
            assert_eq!(app.folder_nav_forward_stack, outer_forward);
            assert!(history_grid_key(&mut app, egui::Key::Backspace).is_none());
            assert!(app.zip_nav.as_ref().is_some_and(|nav| nav.at_root()));
            assert_eq!(app.folder_nav_current_target(), Some(source_target.clone()));
            app.zip_nav_enter("chapter/");
        } else {
            assert_eq!(
                app.items.len(),
                2,
                "PDFium must enumerate the real two-page file"
            );
            assert!(matches!(
                app.items[0],
                GridItem::PdfPage { page_num: 0, .. }
            ));
            assert!(matches!(
                app.items[1],
                GridItem::PdfPage { page_num: 1, .. }
            ));
            app.selected = Some(0);
            assert!(history_grid_key(&mut app, egui::Key::ArrowRight).is_none());
            assert_eq!(
                app.selected,
                Some(1),
                "page selection should move within the PDF"
            );
            assert_eq!(app.folder_nav_current_target(), Some(source_target.clone()));
            assert_eq!(app.folder_nav_back_stack, outer_back);
            assert_eq!(app.folder_nav_forward_stack, outer_forward);
        }

        replay_real_history(&mut app, true, root.as_deref(), None);
        assert_eq!(app.folder_history_forward_target(), Some(&source_target));
        replay_real_history(&mut app, false, Some(&source), pdf_pages.as_ref());
        if epub {
            assert_eq!(
                epub_worker.as_ref().unwrap().direction_requests(),
                vec![false, false],
                "both initial and replay opens must use the worker result"
            );
        }
        assert_eq!(app.folder_nav_current_target(), Some(source_target.clone()));
        assert_eq!(app.folder_history_back_target(), Some(&root_target));

        let parent_nav = history_grid_key(&mut app, egui::Key::Backspace);
        if collection {
            let Some(crate::ui_main::AddressBarNav::Collection(restore)) = parent_nav else {
                panic!("Collection BS must restore its root anchor: {parent_nav:?}");
            };
            app.apply_collection_input_nav(restore, false);
        } else {
            assert!(
                parent_nav.is_none(),
                "Rating BS is handled by its parent router"
            );
        }
        poll_real_history_load(&mut app, root.as_deref(), None);
        if collection {
            let super::super::FolderNavHistoryTarget::Collection(restore) = &root_target else {
                unreachable!();
            };
            wait_for_grid(&mut app, restore.identity.collection_id);
        }
        assert_eq!(app.folder_nav_current_target(), Some(root_target.clone()));
        assert_eq!(app.folder_history_back_target(), Some(&source_target));
        replay_real_history(&mut app, true, Some(&source), pdf_pages.as_ref());
        assert_eq!(app.folder_nav_current_target(), Some(source_target));
        assert_eq!(app.folder_history_forward_target(), Some(&root_target));
        replay_real_history(&mut app, false, root.as_deref(), None);
        assert_eq!(app.folder_nav_current_target(), Some(root_target));
        app.shutdown_collection_runtime_for_exit();
        drop(epub_pin);
    }

    #[test]
    fn real_zip_internal_and_outer_history_from_rating() {
        assert_real_virtual_history(true, false, false);
    }

    #[test]
    fn real_zip_internal_and_outer_history_from_collection() {
        assert_real_virtual_history(true, true, false);
    }

    #[test]
    #[cfg(windows)]
    fn real_pdf_pages_and_outer_history_from_rating() {
        assert_real_virtual_history(false, false, false);
    }

    #[test]
    #[cfg(windows)]
    fn real_pdf_pages_and_outer_history_from_collection() {
        assert_real_virtual_history(false, true, false);
    }

    #[test]
    #[cfg(windows)]
    fn real_epub_pages_and_outer_history_from_rating() {
        assert_real_virtual_history(false, false, true);
    }

    #[test]
    #[cfg(windows)]
    fn real_epub_pages_and_outer_history_from_collection() {
        assert_real_virtual_history(false, true, true);
    }

    #[test]
    #[cfg(windows)]
    fn real_cached_epub_direct_warm_placeholder_commits_before_worker_result() {
        let _test_epoch_scope = crate::page_edit_write_epoch::TestEpochScope::fresh();
        let temp = tempfile::tempdir().unwrap();
        let epub = temp.path().join("warm-book.epub");
        let (_pin, _backend, stamp) = real_cached_epub_history_fixture(&epub, temp.path());
        let target = crate::pdf_loader::pinned_epub_target(&epub).unwrap();
        let pages = read_real_pdf_pages_for_history(target.read_path.as_path());
        let (generation_id, generation_size) = stamp.generation_catalog_pair().unwrap();
        let mut app = crate::app::setup_app_for_test();
        app.get_or_open_catalog(temp.path())
            .unwrap()
            .set_pdf_meta("warm-book.epub", generation_id, generation_size, 2, false)
            .unwrap();
        app.load_folder(temp.path().to_path_buf());
        let source = app.folder_nav_current_target();
        assert!(app.peek_pdf_meta_cache(&epub, false).is_some());
        assert_eq!(
            app.load_pdf_as_folder_with_prepared_pages(
                epub.clone(),
                None,
                super::super::OpenRequestOwner::Navigation,
                None,
                Some(source.clone()),
            ),
            super::super::FolderOpenOutcome::Loaded
        );
        assert!(matches!(
            app.pdf_enumerate_pending.as_ref().map(|pending| &pending.5),
            Some(super::super::PdfOpenPhase::CommittedVerification {
                placeholder_count: 2
            })
        ));
        assert!(matches!(
            app.items.as_slice(),
            [GridItem::PdfPage { .. }, GridItem::PdfPage { .. }]
        ));
        assert_eq!(app.current_folder.as_deref(), Some(epub.as_path()));
        assert_eq!(app.folder_history_back_target(), source.as_ref());
        let placeholder_generation = app.items_generation;
        let placeholder_workers = Arc::clone(&app.cancel_token);
        assert!(!placeholder_workers.load(Ordering::Relaxed));
        app.pdf_enumerate_pending.as_mut().unwrap().2 =
            crate::pdf_loader::completed_enumerate_result_handle(
                &epub,
                Ok(crate::pdf_loader::PdfEnumerateResult {
                    pages,
                    direction: Some(crate::pdf_loader::PdfReadingDirection::R2L),
                    stamp: Some(stamp),
                }),
            );
        app.poll_pdf_enumerate();
        assert_eq!(app.items_generation, placeholder_generation);
        assert!(Arc::ptr_eq(&app.cancel_token, &placeholder_workers));
        assert!(!placeholder_workers.load(Ordering::Relaxed));
        assert_eq!(
            app.reading_direction,
            crate::settings::ReadingDirection::Ltr
        );
        assert_eq!(app.folder_history_back_target(), source.as_ref());
    }

    #[test]
    #[cfg(windows)]
    fn real_epub_ordinary_back_and_forward_replay_adopts_one_outer_point() {
        let _test_epoch_scope = crate::page_edit_write_epoch::TestEpochScope::fresh();
        let temp = tempfile::tempdir().unwrap();
        let old = temp.path().join("ordinary-before-epub");
        std::fs::create_dir(&old).unwrap();
        let source = temp.path().join("ordinary.epub");
        let (_pin, worker, stamp) = real_cached_epub_history_fixture(&source, temp.path());
        assert!(stamp.generation_catalog_pair().is_some());
        let target = super::super::FolderNavHistoryTarget::Path(source.clone());
        for back in [true, false] {
            let (mut app, _) = start_ready_app(&temp.path().join(if back {
                "ordinary-back.db"
            } else {
                "ordinary-forward.db"
            }));
            app.active_quick_folder_slot = None;
            app.settings.follow_document_reading_direction = true;
            app.load_folder(old.clone());
            app.folder_nav_back_stack.clear();
            app.folder_nav_forward_stack.clear();
            if back {
                app.folder_nav_back_stack.push(target.clone());
            } else {
                app.folder_nav_forward_stack.push(target.clone());
            }
            let direction = if back {
                super::super::FolderHistoryDirection::Back
            } else {
                super::super::FolderHistoryDirection::Forward
            };
            assert!(app.start_ordinary_archive_history_replay(direction, target.clone()));
            poll_real_history_load(&mut app, Some(&source), None);
            assert_cached_epub_worker_adoption(&mut app, &source, &stamp);
            assert_eq!(app.folder_nav_current_target(), Some(target.clone()));
            assert_eq!(app.items.len(), 2);
            assert!(app.pdf_enumerate_pending.is_none());
            let opposite = if back {
                &app.folder_nav_forward_stack
            } else {
                &app.folder_nav_back_stack
            };
            assert_eq!(
                opposite,
                &vec![super::super::FolderNavHistoryTarget::Path(old.clone())]
            );
            app.shutdown_collection_runtime_for_exit();
        }
        assert_eq!(worker.direction_requests(), vec![false, false]);
    }

    #[test]
    #[cfg(windows)]
    fn real_epub_ab_switch_changes_slot_only_after_prepared_adoption() {
        let _test_epoch_scope = crate::page_edit_write_epoch::TestEpochScope::fresh();
        let temp = tempfile::tempdir().unwrap();
        let old = temp.path().join("slot-a");
        std::fs::create_dir(&old).unwrap();
        let source = temp.path().join("slot-b.epub");
        let (_pin, worker, stamp) = real_cached_epub_history_fixture(&source, temp.path());
        assert!(stamp.generation_catalog_pair().is_some());
        let (mut app, _) = start_ready_app(&temp.path().join("collection.db"));
        app.settings.follow_document_reading_direction = true;
        app.load_folder(old.clone());
        app.set_quick_folder_slot_target(super::super::QuickFolderSlotId::A, old.clone());
        app.set_quick_folder_slot_target(super::super::QuickFolderSlotId::B, source.clone());
        app.active_quick_folder_slot = Some(super::super::QuickFolderSlotId::A);
        let a_history = app.quick_folder_workspaces[0].history.clone();
        let b_history = app.quick_folder_workspaces[1].history.clone();
        assert_eq!(
            app.activate_quick_folder_slot(super::super::QuickFolderSlotId::B),
            super::super::QuickFolderSwitchTarget::Folder(source.clone())
        );
        assert_eq!(
            app.active_quick_folder_slot,
            Some(super::super::QuickFolderSlotId::A)
        );
        assert!(
            app.start_quick_folder_slot_switch(super::super::QuickFolderSlotId::B, source.clone())
        );
        assert_eq!(app.current_folder.as_deref(), Some(old.as_path()));
        assert_eq!(
            app.active_quick_folder_slot,
            Some(super::super::QuickFolderSlotId::A)
        );
        poll_real_history_load(&mut app, Some(&source), None);
        assert_cached_epub_worker_adoption(&mut app, &source, &stamp);
        assert_eq!(worker.direction_requests(), vec![false]);
        assert_eq!(
            app.active_quick_folder_slot,
            Some(super::super::QuickFolderSlotId::B)
        );
        assert_eq!(app.quick_folder_workspaces[0].history, a_history);
        assert_eq!(app.quick_folder_workspaces[1].history, b_history);
        assert_eq!(
            app.folder_nav_current_target(),
            Some(super::super::FolderNavHistoryTarget::Path(source))
        );
        app.shutdown_collection_runtime_for_exit();
    }

    #[test]
    fn real_unconverted_collection_epub_cancel_and_save_pdf_preserve_root_owner() {
        let _test_epoch_scope = crate::page_edit_write_epoch::TestEpochScope::fresh();
        let temp = tempfile::tempdir().unwrap();
        let epub = temp.path().join("unconverted.epub");
        write_small_history_epub(&epub);
        // The lib test executable has no PDF worker entry point. Keep the real EPUB on disk
        // and return its typed unresolved result at the worker boundary.
        let _failure = crate::pdf_loader::fail_epub_for_test(
            &epub,
            crate::pdf_loader::PdfReadError::NotConverted,
        );
        let (mut app, client) = start_ready_app(&temp.path().join("collection.db"));
        app.active_quick_folder_slot = None;
        app.settings
            .set_archive_file_handling(crate::settings::ArchiveFileHandling::Ask);
        let collection =
            collection_with_sources(&client, &[(epub.clone(), CollectionResolvedKind::Pdf)]);
        app.open_collection_grid(collection.collection_id(), None);
        wait_for_grid(&mut app, collection.collection_id());
        let root = app.folder_nav_current_target();
        let before_rows = app.items.clone();
        let before_address = app.address.clone();
        let before_history = app.folder_nav_history_snapshot();
        let owner = app
            .collection_grid_physical_load_owner(0, &epub)
            .expect("Collection EPUB owner");
        assert!(app.load_folder_with_scan_owned(
            epub.clone(),
            None,
            super::super::OpenRequestOwner::CollectionGridPhysical(owner),
        ));
        poll_until(
            &mut app,
            "unconverted EPUB did not open its dialog",
            |app| {
                matches!(
                    app.top_level_grid_view.history_navigation_transition(),
                    Some(super::super::HistoryNavigationTransition::Physical(request))
                        if matches!(request.phase, super::super::PhysicalHistoryPhase::EpubConverting)
                )
            },
        );
        assert_eq!(app.folder_nav_current_target(), root);
        assert_eq!(app.items, before_rows);
        assert_eq!(app.address, before_address);
        assert_eq!(app.folder_nav_back_stack, before_history.back_stack);
        assert_eq!(app.folder_nav_forward_stack, before_history.forward_stack);
        assert!(matches!(
            app.epub_convert.as_ref().map(|state| &state.continuation),
            Some(crate::ui_dialogs::epub_convert::EpubOpenContinuation::StagedHistory { .. })
        ));
        app.finish_epub_convert(crate::ui_dialogs::epub_convert::EpubConvertExit::Abort);
        assert!(
            app.top_level_grid_view
                .history_navigation_transition()
                .is_none()
        );
        assert_eq!(app.folder_nav_current_target(), root);
        assert_eq!(app.items, before_rows);
        assert_eq!(app.folder_nav_back_stack, before_history.back_stack);

        let pdf = epub.with_extension("pdf");
        std::fs::write(&pdf, b"%PDF-1.4\n").unwrap();
        let owner = app.collection_grid_physical_load_owner(0, &epub).unwrap();
        assert!(app.load_folder_with_scan_owned(
            epub.clone(),
            None,
            super::super::OpenRequestOwner::CollectionGridPhysical(owner),
        ));
        poll_until(
            &mut app,
            "second EPUB request did not open its dialog",
            |app| app.epub_convert.is_some(),
        );
        let original = app.epub_convert.take().unwrap();
        let mut saved = crate::ui_dialogs::epub_convert::EpubConvertState::saved_for_test(
            epub.clone(),
            super::super::OpenRequestOwner::Navigation,
            crate::epub_convert::SavedPdf {
                path: pdf.clone(),
                reused_cache: false,
                user_data_errors: Vec::new(),
            },
            original.surface_generation,
            original.smart_transition_sequence,
        );
        saved.continuation = original.continuation.clone();
        app.epub_convert = Some(saved);
        drop(original);
        let ctx = egui::Context::default();
        let _ = ctx.run(Default::default(), |ctx| app.show_epub_convert_dialog(ctx));
        assert_eq!(app.folder_nav_current_target(), root);
        assert_eq!(app.items, before_rows);
        let Some(mut transition) = app.top_level_grid_view.take_history_navigation_transition()
        else {
            panic!("saved sibling needs staged PDF preflight");
        };
        let super::super::HistoryNavigationTransition::Physical(request) = &mut transition else {
            panic!("Collection root Save PDF keeps a physical child owner");
        };
        assert_eq!(request.path, pdf);
        assert!(matches!(&request.intent,
            super::super::PhysicalHistoryIntent::CollectionGrid { owner }
                if owner.target_path == pdf));
        let super::super::PhysicalHistoryPhase::Preflighting { preflight, .. } = &mut request.phase
        else {
            panic!("saved sibling preflight phase");
        };
        *preflight = super::super::collection_navigation::PhysicalHistoryPreflight::ready_for_test(
            super::super::collection_navigation::PhysicalHistoryPreflightPayload::PdfPages(
                vec![crate::pdf_loader::PdfPageEntry {
                    page_num: 0,
                    mtime: 1,
                    file_size: 1,
                }]
                .into(),
            ),
        );
        app.top_level_grid_view
            .set_history_navigation_transition(Some(transition));
        app.poll_collection_history_transition(&ctx);
        assert_eq!(app.current_folder.as_deref(), Some(pdf.as_path()));
        assert!(matches!(
            &app.top_level_grid_view.collection_session().unwrap().position,
            CollectionGridPosition::PhysicalSource { path, .. }
                if crate::folder_tree::path_eq(path, &epub)
        ));
        assert_eq!(app.folder_history_back_target(), root.as_ref());
        app.shutdown_collection_runtime_for_exit();
    }

    #[test]
    fn stale_convertible_collection_tile_folder_result_commits_source_position() {
        let _test_epoch_scope = crate::page_edit_write_epoch::TestEpochScope::fresh();
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("archive-became-folder.7z");
        std::fs::write(&source, b"old archive tile").unwrap();
        let (mut app, client) = start_ready_app(&temp.path().join("collection.db"));
        app.active_quick_folder_slot = None;
        let collection = collection_with_sources(
            &client,
            &[(source.clone(), CollectionResolvedKind::ConvertibleArchive)],
        );
        app.open_collection_grid(collection.collection_id(), None);
        wait_for_grid(&mut app, collection.collection_id());
        assert!(matches!(
            app.items.first(),
            Some(GridItem::ConvertibleArchive { .. })
        ));

        std::fs::remove_file(&source).unwrap();
        std::fs::create_dir(&source).unwrap();
        std::fs::write(source.join("001.png"), b"dummy").unwrap();
        app.settings
            .set_archive_file_handling(crate::settings::ArchiveFileHandling::Ignore);
        let owner = app.main_grid_archive_open_owner(0, &source);
        assert!(matches!(
            owner,
            super::super::OpenRequestOwner::MainGridArchive(ref intent)
                if intent.collection_grid_owner.is_some()
        ));
        let _ = app.load_folder_or_convert_archive_with_auto_fullscreen_owned(
            source.clone(),
            false,
            owner,
        );
        app.settle_open_path_classification_for_test();
        poll_until(&mut app, "stale Collection tile did not settle", |app| {
            app.current_folder.as_deref() == Some(source.as_path())
                && app
                    .top_level_grid_view
                    .history_navigation_transition()
                    .is_none()
        });

        assert_eq!(app.current_folder.as_deref(), Some(source.as_path()));
        assert!(matches!(app.items.first(), Some(GridItem::Image(_))));
        assert!(matches!(
            app.top_level_grid_view
                .collection_session()
                .map(|session| &session.position),
            Some(CollectionGridPosition::PhysicalSource { .. })
        ));
        assert!(matches!(
            app.folder_history_back_target(),
            Some(super::super::FolderNavHistoryTarget::Collection(restore))
                if restore.identity.collection_id == collection.collection_id()
        ));
        app.shutdown_collection_runtime_for_exit();
    }

    #[test]
    fn failed_collection_zip_preflight_keeps_root_rows_and_history() {
        let _test_epoch_scope = crate::page_edit_write_epoch::TestEpochScope::fresh();
        let temp = tempfile::tempdir().unwrap();
        let zip_path = temp.path().join("broken.zip");
        std::fs::write(&zip_path, b"not a ZIP").unwrap();
        let (mut app, client) = start_ready_app(&temp.path().join("collection.db"));
        app.active_quick_folder_slot = None;
        let collection =
            collection_with_sources(&client, &[(zip_path.clone(), CollectionResolvedKind::Zip)]);
        app.open_collection_grid(collection.collection_id(), None);
        wait_for_grid(&mut app, collection.collection_id());

        let before_items = app.items.clone();
        let before_history = app.folder_nav_history_snapshot();
        let owner = app
            .collection_grid_physical_load_owner(0, &zip_path)
            .unwrap();
        assert!(app.load_folder_with_scan_owned(
            zip_path,
            None,
            super::super::OpenRequestOwner::CollectionGridPhysical(owner),
        ));
        poll_until(&mut app, "failed ZIP preflight did not finish", |app| {
            app.top_level_grid_view
                .history_navigation_transition()
                .is_none()
        });
        assert!(matches!(
            app.top_level_grid_view
                .collection_session()
                .unwrap()
                .position,
            CollectionGridPosition::Root
        ));
        assert_eq!(app.items, before_items);
        assert_eq!(app.folder_nav_back_stack, before_history.back_stack);
        assert_eq!(app.folder_nav_forward_stack, before_history.forward_stack);
        app.shutdown_collection_runtime_for_exit();
    }

    #[test]
    fn collection_pdf_password_cancel_keeps_root_and_retry_commits_once() {
        let _test_epoch_scope = crate::page_edit_write_epoch::TestEpochScope::fresh();
        let temp = tempfile::tempdir().unwrap();
        let pdf_path = temp.path().join("protected.pdf");
        std::fs::write(&pdf_path, b"PDF fixture; enumeration is seeded").unwrap();
        let (mut app, client) = start_ready_app(&temp.path().join("collection.db"));
        app.active_quick_folder_slot = None;
        let collection =
            collection_with_sources(&client, &[(pdf_path.clone(), CollectionResolvedKind::Pdf)]);
        app.open_collection_grid(collection.collection_id(), None);
        wait_for_grid(&mut app, collection.collection_id());
        let before_items = app.items.clone();
        let before_history = app.folder_nav_history_snapshot();

        for cancel in [true, false] {
            let owner = app
                .collection_grid_physical_load_owner(0, &pdf_path)
                .unwrap();
            assert!(app.load_folder_with_scan_owned(
                pdf_path.clone(),
                None,
                super::super::OpenRequestOwner::CollectionGridPhysical(owner),
            ));
            seed_physical_history_preflight(
                &mut app,
                super::super::collection_navigation::PhysicalHistoryPreflightPayload::PdfPasswordRequired,
            );
            app.poll_collection_history_transition(&egui::Context::default());
            assert_eq!(app.items, before_items);
            assert_eq!(app.folder_nav_back_stack, before_history.back_stack);
            assert!(matches!(
                app.top_level_grid_view
                    .collection_session()
                    .unwrap()
                    .position,
                CollectionGridPosition::Root
            ));
            if cancel {
                assert!(app.cancel_pdf_password_dialog_request());
                assert!(
                    app.top_level_grid_view
                        .history_navigation_transition()
                        .is_none()
                );
                continue;
            }
            assert!(app.retry_pdf_password_dialog_request("secret".into(), false));
            seed_physical_history_preflight(
                &mut app,
                super::super::collection_navigation::PhysicalHistoryPreflightPayload::PdfPages(
                    vec![crate::pdf_loader::PdfPageEntry {
                        page_num: 0,
                        mtime: 1,
                        file_size: 1,
                    }]
                    .into(),
                ),
            );
            app.poll_collection_history_transition(&egui::Context::default());
        }
        assert_eq!(app.current_folder.as_deref(), Some(pdf_path.as_path()));
        assert!(matches!(
            app.top_level_grid_view
                .collection_session()
                .unwrap()
                .position,
            CollectionGridPosition::PhysicalSource { .. }
        ));
        assert!(matches!(
            app.folder_history_back_target(),
            Some(super::super::FolderNavHistoryTarget::Collection(restore))
                if restore.identity.collection_id == collection.collection_id()
        ));
        app.shutdown_collection_runtime_for_exit();
    }

    #[test]
    fn descendant_collection_zip_commits_child_history_and_stale_owner_cannot_reopen() {
        let _test_epoch_scope = crate::page_edit_write_epoch::TestEpochScope::fresh();
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("root");
        std::fs::create_dir(&root).unwrap();
        let zip_path = root.join("chapter.zip");
        write_single_page_zip(&zip_path);
        let other = temp.path().join("other");
        std::fs::create_dir(&other).unwrap();
        let (mut app, client) = start_ready_app(&temp.path().join("collection.db"));
        app.active_quick_folder_slot = None;
        let collection =
            collection_with_sources(&client, &[(root.clone(), CollectionResolvedKind::Folder)]);
        app.open_collection_grid(collection.collection_id(), None);
        wait_for_grid(&mut app, collection.collection_id());
        let owner = app.collection_grid_physical_load_owner(0, &root).unwrap();
        let scan =
            super::super::folder_scan::scan_directory_with_settings(&root, &app.settings).unwrap();
        assert!(app.load_folder_with_scan_owned(
            root.clone(),
            Some(scan),
            super::super::OpenRequestOwner::CollectionGridPhysical(owner),
        ));
        let index = app.items.iter().position(|item| {
            matches!(item, GridItem::ZipFile(path) if crate::folder_tree::path_eq(path, &zip_path))
        }).expect("ZIP descendant row");
        let owner = app
            .collection_grid_physical_load_owner(index, &zip_path)
            .unwrap();
        let before_items = app.items.clone();
        let before_back = app.folder_nav_back_stack.clone();
        assert!(app.load_folder_with_scan_owned(
            zip_path.clone(),
            None,
            super::super::OpenRequestOwner::CollectionGridPhysical(owner.clone()),
        ));
        assert_eq!(app.items, before_items);
        assert_eq!(app.folder_nav_back_stack, before_back);
        poll_until(&mut app, "descendant ZIP did not adopt", |app| {
            app.current_folder
                .as_deref()
                .is_some_and(|path| crate::folder_tree::path_eq(path, &zip_path))
        });
        assert!(matches!(
            app.folder_history_back_target(),
            Some(super::super::FolderNavHistoryTarget::CollectionPhysical(restore))
                if crate::folder_tree::path_eq(&restore.visible_path, &root)
        ));

        // A queued owner from the former descendant must never overwrite the winner.
        let stale_owner = owner;
        assert!(!app.collection_grid_physical_load_owner_is_current(&stale_owner, &zip_path));
        let scan =
            super::super::folder_scan::scan_directory_with_settings(&other, &app.settings).unwrap();
        assert!(app.load_folder_with_scan_owned(
            other.clone(),
            Some(scan),
            super::super::OpenRequestOwner::Navigation,
        ));
        assert!(!app.load_folder_with_scan_owned(
            zip_path,
            None,
            super::super::OpenRequestOwner::CollectionGridPhysical(stale_owner),
        ));
        assert_eq!(app.current_folder.as_deref(), Some(other.as_path()));
        app.shutdown_collection_runtime_for_exit();
    }

    fn assert_collection_mutation_refresh(refresh: impl FnOnce(&mut App, &Path)) {
        let _epoch = crate::page_edit_write_epoch::TestEpochScope::fresh();
        let temp = tempfile::tempdir().unwrap();
        let folder = temp.path().join("pin-refresh-child");
        std::fs::create_dir(&folder).unwrap();
        std::fs::write(folder.join("page.png"), b"page").unwrap();
        let video = folder.join("movie.mp4");
        std::fs::write(&video, b"movie").unwrap();
        let (mut app, client) = start_ready_app(&temp.path().join("collection.db"));
        app.settings.sidecar_backup_enabled = false;
        app.settings.tag_sidecar_backup_enabled = false;
        app.active_quick_folder_slot = None;
        let snapshot =
            collection_with_sources(&client, &[(folder.clone(), CollectionResolvedKind::Folder)]);
        app.open_collection_grid(snapshot.collection_id(), None);
        wait_for_grid(&mut app, snapshot.collection_id());
        let owner = app.collection_grid_physical_load_owner(0, &folder).unwrap();
        let scan = super::super::folder_scan::scan_directory_with_settings(&folder, &app.settings)
            .unwrap();
        assert!(app.load_folder_with_scan_owned(
            folder.clone(),
            Some(scan),
            super::super::OpenRequestOwner::CollectionGridPhysical(owner)
        ));
        app.selected = app
            .items
            .iter()
            .position(|it| matches!(it, GridItem::Image(_)));
        app.scroll_offset_y = 125.0;
        let selected_item = app.items[app.selected.unwrap()].clone();
        let location = app.folder_nav_current_target();
        let before = app.folder_nav_history_snapshot();
        // A parked viewer has its own rows, pending work and parent target.
        let sibling = app.build_window_context_for_test(9_902, |sibling| {
            sibling.current_folder = Some(temp.path().join("sibling"));
            sibling.selected = Some(0);
            sibling.scroll_offset_y = 77.0;
            sibling.items = vec![GridItem::Image(temp.path().join("sibling.png"))];
        });
        let sibling_state = app
            .with_viewer_context(sibling, |sibling| {
                (
                    sibling.items.clone(),
                    sibling.items_generation,
                    sibling.cancel_token.clone(),
                    sibling.folder_nav_current_target(),
                    sibling.selected,
                    sibling.scroll_offset_y,
                )
            })
            .unwrap();
        refresh(&mut app, &folder);
        assert_eq!(app.folder_nav_current_target(), location);
        assert!(matches!(
            app.top_level_grid_view
                .collection_session()
                .unwrap()
                .position,
            CollectionGridPosition::PhysicalSource { .. }
        ));
        assert_eq!(app.current_folder.as_deref(), Some(folder.as_path()));
        assert_eq!(
            app.folder_nav_history_snapshot().back_stack,
            before.back_stack
        );
        assert_eq!(
            app.folder_nav_history_snapshot().forward_stack,
            before.forward_stack
        );
        assert_eq!(app.items[app.selected.unwrap()], selected_item);
        assert_eq!(app.scroll_offset_y, 125.0);
        let Some(crate::ui_main::AddressBarNav::Collection(parent)) =
            app.resolve_return_to_parent_nav()
        else {
            panic!("BS must return to the collection root");
        };
        assert_eq!(parent.identity.collection_id, snapshot.collection_id());
        app.with_viewer_context(sibling, |sibling| {
            assert_eq!(sibling.items, sibling_state.0);
            assert_eq!(sibling.items_generation, sibling_state.1);
            assert!(Arc::ptr_eq(&sibling.cancel_token, &sibling_state.2));
            assert!(!sibling.cancel_token.load(Ordering::Relaxed));
            assert_eq!(sibling.folder_nav_current_target(), sibling_state.3);
            assert_eq!(sibling.selected, sibling_state.4);
            assert_eq!(sibling.scroll_offset_y, sibling_state.5);
        })
        .unwrap();
        app.shutdown_collection_runtime_for_exit();
    }

    #[test]
    fn mutation_refresh_collection_pin_preserves_location_history_and_sibling() {
        assert_collection_mutation_refresh(|app, _| {
            app.toggle_folder_pin_for_idx(app.selected.unwrap());
            assert!(!app.folder_thumb_pin_dirty.is_empty());
            app.consume_folder_thumb_pin_dirty();
            assert!(app.folder_thumb_pin_dirty.is_empty());
        });
    }

    #[test]
    fn mutation_refresh_collection_unpin_preserves_location_history_and_sibling() {
        assert_collection_mutation_refresh(|app, folder| {
            app.toggle_folder_pin_for_idx(app.selected.unwrap());
            app.consume_folder_thumb_pin_dirty();
            assert!(app.remove_folder_thumb_pin(folder));
            app.consume_folder_thumb_pin_dirty();
            assert!(app.folder_thumb_pin_for(folder).is_none());
        });
    }

    #[test]
    fn mutation_refresh_collection_video_pin_preserves_location_history_and_sibling() {
        assert_collection_mutation_refresh(|app, folder| {
            app.video_thumb_overrides_dirty_paths
                .insert(folder.join("movie.mp4"));
            app.consume_video_thumb_overrides_dirty();
            assert!(app.video_thumb_overrides_dirty_paths.is_empty());
        });
    }

    #[test]
    fn mutation_refresh_collection_deferred_export_preserves_location_history_and_sibling() {
        assert_collection_mutation_refresh(|app, folder| {
            app.note_exported_file_for_folder_refresh(&folder.join("export.png"));
            assert!(app.folder_refresh_pending.is_some());
            app.consume_folder_refresh_pending();
            assert!(app.folder_refresh_pending.is_none());
        });
    }

    #[test]
    fn mutation_refresh_collection_external_rescan_preserves_location_history_and_sibling() {
        assert_collection_mutation_refresh(|app, folder| {
            std::fs::write(folder.join("new.png"), b"new").unwrap();
            let scan =
                super::super::folder_scan::scan_directory_with_settings(folder, &app.settings)
                    .unwrap();
            app.apply_external_rescan(folder.to_path_buf(), std::time::SystemTime::now(), scan);
            assert!(
                app.items
                    .iter()
                    .any(|it| matches!(it, GridItem::Image(p) if p.ends_with("new.png")))
            );
        });
    }

    #[test]
    fn mutation_refresh_collection_stack_toggle_preserves_location_history_and_sibling() {
        assert_collection_mutation_refresh(|app, _| {
            app.settings.stack_script_enabled = false;
            app.toggle_stack_mode();
            assert!(app.stack_mode_requested);
            app.toggle_stack_mode();
            assert!(!app.stack_mode_requested);
        });
    }

    #[test]
    fn mutation_refresh_collection_f5_preserves_location_history_and_sibling() {
        assert_collection_mutation_refresh(|app, _| {
            app.reload_current_folder_preserving_override()
        });
    }

    #[test]
    fn mutation_refresh_ordinary_cached_archive_f5_keeps_logical_history() {
        let _epoch = crate::page_edit_write_epoch::TestEpochScope::fresh();
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source.7z");
        let cache = temp.path().join("cache.zip");
        write_single_page_zip(&cache);
        let (mut app, _) = start_ready_app(&temp.path().join("collection.db"));
        app.settings.sidecar_backup_enabled = false;
        app.settings.tag_sidecar_backup_enabled = false;
        app.active_quick_folder_slot = None;
        app.current_folder = Some(cache.clone());
        app.archive_source_override = Some(source.clone());
        app.folder_nav_back_stack
            .push(super::super::FolderNavHistoryTarget::Path(
                temp.path().join("previous"),
            ));
        app.folder_nav_forward_stack
            .push(super::super::FolderNavHistoryTarget::Path(
                temp.path().join("next"),
            ));
        let before = app.folder_nav_history_snapshot();
        let location = app.folder_nav_current_target();
        app.reload_current_folder_preserving_override();
        app.settle_open_path_classification_for_test();
        poll_real_history_load(&mut app, Some(&cache), None);
        assert_eq!(app.folder_nav_current_target(), location);
        assert_eq!(app.archive_source_override.as_ref(), Some(&source));
        assert_eq!(
            app.folder_nav_history_snapshot().back_stack,
            before.back_stack
        );
        assert_eq!(
            app.folder_nav_history_snapshot().forward_stack,
            before.forward_stack
        );
        assert!(app.zip_nav.is_some());
        app.shutdown_collection_runtime_for_exit();
    }

    #[test]
    fn mutation_refresh_collection_cached_archive_f5_does_not_record_cache_location() {
        let _epoch = crate::page_edit_write_epoch::TestEpochScope::fresh();
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source.7z");
        let cache = temp.path().join("cache.zip");
        std::fs::write(&source, b"source").unwrap();
        write_single_page_zip(&cache);
        let (mut app, client) = start_ready_app(&temp.path().join("collection.db"));
        app.settings.sidecar_backup_enabled = false;
        app.settings.tag_sidecar_backup_enabled = false;
        app.active_quick_folder_slot = None;
        let snapshot = collection_with_sources(
            &client,
            &[(source.clone(), CollectionResolvedKind::ConvertibleArchive)],
        );
        app.open_collection_grid(snapshot.collection_id(), None);
        wait_for_grid(&mut app, snapshot.collection_id());
        // Model the already accepted conversion: logical source and physical cache are distinct.
        let anchor = app.collection_grid_source_anchor(0, &source).unwrap();
        app.commit_collection_grid_source_open(anchor, source.clone());
        app.current_folder = Some(cache.clone());
        app.archive_source_override = Some(source.clone());
        let before = app.folder_nav_history_snapshot();
        let location = app.folder_nav_current_target();
        app.reload_current_folder_preserving_override();
        app.settle_open_path_classification_for_test();
        poll_real_history_load(&mut app, Some(&cache), None);
        assert_eq!(app.current_folder.as_ref(), Some(&cache));
        assert_eq!(app.archive_source_override.as_ref(), Some(&source));
        assert_eq!(app.folder_nav_current_target(), location);
        assert_eq!(
            app.folder_nav_history_snapshot().back_stack,
            before.back_stack
        );
        assert_eq!(
            app.folder_nav_history_snapshot().forward_stack,
            before.forward_stack
        );
        assert!(
            app.zip_nav.is_some(),
            "F5 must really re-enumerate the backing archive"
        );
        app.shutdown_collection_runtime_for_exit();
    }

    #[test]
    fn collection_parent_resolution_is_owned_by_the_mounted_viewer_context() {
        let _test_epoch_scope = crate::page_edit_write_epoch::TestEpochScope::fresh();
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
        app.commit_collection_grid_source_open(anchor.clone(), folder.clone());
        app.current_folder = Some(nested.clone());

        let TopLevelGridRestore::CollectionPhysical(current) = app
            .collection_grid_current_restore_snapshot()
            .expect("physical current location")
        else {
            panic!("collection child must be a distinct current location")
        };
        assert!(crate::folder_tree::path_eq(
            &current.root_source_path,
            &folder
        ));
        assert!(crate::folder_tree::path_eq(&current.visible_path, &nested));
        assert_eq!(current.root.viewport_anchor.as_ref(), Some(&anchor));
        assert!(matches!(
            app.collection_grid_restore_snapshot(),
            Some(TopLevelGridRestore::Collection(_))
        ));

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
        let _test_epoch_scope = crate::page_edit_write_epoch::TestEpochScope::fresh();
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
        let _test_epoch_scope = crate::page_edit_write_epoch::TestEpochScope::fresh();
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
        let _test_epoch_scope = crate::page_edit_write_epoch::TestEpochScope::fresh();
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
        app.cancel_folder_pane_open(crate::app::PaneOpenRestoreExit::Abandoned);
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
        let _test_epoch_scope = crate::page_edit_write_epoch::TestEpochScope::fresh();
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
        let _test_epoch_scope = crate::page_edit_write_epoch::TestEpochScope::fresh();
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
            epub_restore: None,
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
        let _test_epoch_scope = crate::page_edit_write_epoch::TestEpochScope::fresh();
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
        let _test_epoch_scope = crate::page_edit_write_epoch::TestEpochScope::fresh();
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
        let _test_epoch_scope = crate::page_edit_write_epoch::TestEpochScope::fresh();
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
        let _test_epoch_scope = crate::page_edit_write_epoch::TestEpochScope::fresh();
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
    fn saved_epub_sibling_pdf_keeps_collection_root_owner() {
        let _test_epoch_scope = crate::page_edit_write_epoch::TestEpochScope::fresh();
        let temp = tempfile::tempdir().unwrap();
        let epub = temp.path().join("book.epub");
        let pdf = epub.with_extension("pdf");
        std::fs::write(&epub, b"epub").unwrap();
        std::fs::write(&pdf, b"%PDF-1.4").unwrap();
        let (mut app, client) = start_ready_app(&temp.path().join("collection.db"));
        let snapshot =
            collection_with_sources(&client, &[(epub.clone(), CollectionResolvedKind::Pdf)]);
        app.open_collection_grid(snapshot.collection_id(), None);
        wait_for_grid(&mut app, snapshot.collection_id());
        let mut owner = app.collection_grid_physical_load_owner(0, &epub).unwrap();
        owner.target_path = pdf.clone();
        assert!(app.collection_grid_physical_load_owner_is_current(&owner, &pdf));
        assert!(!app.collection_grid_physical_load_owner_is_current(
            &owner,
            &temp.path().join("other.pdf")
        ));
        assert!(app.commit_collection_grid_physical_load(&owner, &pdf));
        assert!(
            matches!(&app.top_level_grid_view.collection_session().unwrap().position,
            CollectionGridPosition::PhysicalSource { path, .. } if crate::folder_tree::path_eq(path, &epub))
        );
        app.shutdown_collection_runtime_for_exit();
    }

    #[test]
    fn root_load_owner_stales_on_wanted_revision_but_physical_continuation_does_not() {
        let _test_epoch_scope = crate::page_edit_write_epoch::TestEpochScope::fresh();
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
        assert!(app.collection_grid_parent_nav().is_none());
        assert!(matches!(
            app.collection_grid_current_restore_snapshot(),
            Some(TopLevelGridRestore::Folder(path)) if crate::folder_tree::path_eq(&path, &nested)
        ));
        assert!(
            app.collection_grid_physical_load_owner(deeper_index, &deeper)
                .is_none(),
            "deleted collection cannot authorize another owned child open"
        );
        let scan = super::super::folder_scan::scan_directory_with_settings(&deeper, &app.settings)
            .unwrap();
        assert!(app.load_folder_with_scan_owned(
            deeper.clone(),
            Some(scan),
            crate::app::OpenRequestOwner::Navigation,
        ));
        assert_eq!(app.current_folder.as_deref(), Some(deeper.as_path()));
        assert!(matches!(
            app.top_level_grid_view.surface(),
            TopLevelGridSurface::Folder
        ));
        assert!(app.top_level_grid_view.collection_session().is_none());
        app.shutdown_collection_runtime_for_exit();
    }

    #[test]
    fn collection_video_workers_use_generation_owned_full_path_sidecars_and_cancel_on_refresh() {
        let _test_epoch_scope = crate::page_edit_write_epoch::TestEpochScope::fresh();
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
        let _test_epoch_scope = crate::page_edit_write_epoch::TestEpochScope::fresh();
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
        let _test_epoch_scope = crate::page_edit_write_epoch::TestEpochScope::fresh();
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
        let _test_epoch_scope = crate::page_edit_write_epoch::TestEpochScope::fresh();
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
        let _test_epoch_scope = crate::page_edit_write_epoch::TestEpochScope::fresh();
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
        let _test_epoch_scope = crate::page_edit_write_epoch::TestEpochScope::fresh();
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

        app.advance_collection_thumbnail_source_epoch_for_pin_commit(0, "metadata_import");
        assert_eq!(app.collection_thumbnail_source_epoch, old_epoch);
        assert!(matches!(
            app.top_level_grid_view.collection_session().unwrap().load,
            CollectionGridLoadState::Ready(_)
        ));
        assert!(!video_worker_cancel.load(Ordering::Acquire));

        app.advance_collection_thumbnail_source_epoch_for_pin_commit(1, "metadata_import");

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
        let _test_epoch_scope = crate::page_edit_write_epoch::TestEpochScope::fresh();
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
        let _test_epoch_scope = crate::page_edit_write_epoch::TestEpochScope::fresh();
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
        let _test_epoch_scope = crate::page_edit_write_epoch::TestEpochScope::fresh();
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
        let _test_epoch_scope = crate::page_edit_write_epoch::TestEpochScope::fresh();
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
        let _test_epoch_scope = crate::page_edit_write_epoch::TestEpochScope::fresh();
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
        let _test_epoch_scope = crate::page_edit_write_epoch::TestEpochScope::fresh();
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
        let _test_epoch_scope = crate::page_edit_write_epoch::TestEpochScope::fresh();
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

        let edits = super::super::page_edit_snapshot::PageEditSnapshot::load_and_project_stable(
            &app.items,
            super::super::page_edit_snapshot::PageEditAvailability::default(),
            &AtomicBool::new(false),
        )
        .unwrap()
        .unwrap();
        let retained_edits = Some(Arc::new(edits.snapshot.clone()));

        prepare_sender
            .send(Ok(CollectionGridPreparedInstall {
                prepared: (*installed_prepared).clone(),
                page_edits: Some((edits.snapshot, edits.projection)),
                retained_page_edits: retained_edits,
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
        let _test_epoch_scope = crate::page_edit_write_epoch::TestEpochScope::fresh();
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
        let _test_epoch_scope = crate::page_edit_write_epoch::TestEpochScope::fresh();
        let exact = crate::delete_worker::DeleteSourceScope::Exact(PathBuf::from(r"C:\A\B"));
        assert!(source_scope_contains(&exact, Path::new(r"c:/a/b")));
        assert!(!source_scope_contains(&exact, Path::new(r"c:/a/b/c.png")));
        let tree = crate::delete_worker::DeleteSourceScope::Tree(PathBuf::from(r"C:\A\B"));
        assert!(source_scope_contains(&tree, Path::new(r"c:/a/b/c.png")));
        assert!(!source_scope_contains(&tree, Path::new(r"c:/a/b2/c.png")));
    }

    #[test]
    fn durable_source_migration_is_retired_only_after_actor_ack() {
        let _test_epoch_scope = crate::page_edit_write_epoch::TestEpochScope::fresh();
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
