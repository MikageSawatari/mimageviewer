//! Physical-list observations, adopted projection and subscription belong to one viewer context.
//! All filesystem/DB preparation below is called by workers; reducers are pure memory operations.
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use super::folder_scan::{
    self, MaterializedFolderListing, ObservedEntryStamp, RawFolderSnapshot, ScannedDir,
};
use super::viewer_context_registry::ViewerContextId;
use crate::grid_item::GridItem;

pub(crate) type FileOperationId = u64;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct DeleteListingOwner {
    pub(crate) context_id: ViewerContextId,
    pub(crate) folder: PathBuf,
    pub(crate) baseline_revision: u64,
    pub(crate) generation: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct FolderWatchIdentity {
    pub(crate) context_id: ViewerContextId,
    pub(crate) subscription_epoch: u64,
    pub(crate) folder: PathBuf,
    pub(crate) listing_options: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ProjectionRevision {
    pub(crate) settings_key: String,
    pub(crate) rating_write_generation: u64,
    pub(crate) pin_stamp: Option<crate::folder_thumb_pins::FolderPinMutationStamp>,
    pub(crate) metadata_revision: u64,
    pub(crate) stack_mode: bool,
    pub(crate) stack_flat: bool,
}

#[derive(Clone)]
pub(crate) struct FolderPrepareInputs {
    pub(crate) settings: crate::settings::Settings,
    pub(crate) rating_db_path: Option<PathBuf>,
    pub(crate) rating_write_generation: u64,
    pub(crate) pin_db: Option<Arc<crate::folder_thumb_pins::FolderThumbPinDb>>,
    pub(crate) use_full_path_keys: bool,
    pub(crate) stack_mode: bool,
    pub(crate) stack_flat: bool,
    pub(crate) metadata_revision: u64,
}

impl FolderPrepareInputs {
    pub(crate) fn revision(&self) -> ProjectionRevision {
        ProjectionRevision {
            settings_key: listing_options_key(&self.settings),
            rating_write_generation: self.rating_write_generation,
            pin_stamp: self.pin_db.as_ref().map(|db| db.mutation_stamp()),
            metadata_revision: self.metadata_revision,
            stack_mode: self.stack_mode,
            stack_flat: self.stack_flat,
        }
    }
}

/// Only settings which affect the adopted physical projection/representative requests.
pub(crate) fn listing_options_key(settings: &crate::settings::Settings) -> String {
    serde_json::json!({
        "visibility": settings.file_type_visibility.fingerprint(),
        "show_hidden": settings.show_hidden_files,
        "archive": !settings.archive_file_handling_ignores_convertible(),
        "epub": !settings.epub_file_handling_ignores_epub(),
        "skip_zip": settings.skip_zip_if_folder_exists,
        "skip_archive": settings.skip_archive_if_zip_exists,
        "skip_epub": settings.skip_epub_if_pdf_exists,
        "skip_image": settings.skip_image_if_video_exists,
        "sidecar": settings.video_thumb_use_sidecar_image,
        "duplicates": settings.skip_duplicate_images,
        "image_priority": settings.image_ext_priority,
        "sort": settings.sort_order,
        "display_order": settings.grid_display_order,
        "thumb_sort": settings.folder_thumb_sort,
        "thumb_depth": settings.folder_thumb_depth,
        "stack_separator": settings.stack_separator,
        "stack_script": settings.stack_script_enabled,
        "image_book": settings.auto_fullscreen_image_folders_enabled(),
        "books_root": settings.books_root_path(),
        "recognition": folder_scan::image_page_recognition_fingerprint(settings),
    })
    .to_string()
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ProjectionRow {
    pub(crate) item: GridItem,
    pub(crate) display_meta: Option<(i64, i64)>,
    pub(crate) sort_meta: crate::settings::ListingSortMetadata,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ProjectionDependency {
    /// Stable physical owner, never a display index.
    pub(crate) owner: PathBuf,
    pub(crate) path: PathBuf,
    pub(crate) stamp: ObservedEntryStamp,
    pub(crate) identity: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct FolderProjectionProof {
    pub(crate) revision: ProjectionRevision,
    pub(crate) rows: Vec<ProjectionRow>,
    pub(crate) sidecars: BTreeMap<PathBuf, PathBuf>,
    pub(crate) dependencies: Vec<ProjectionDependency>,
    pub(crate) cache_keys: BTreeMap<PathBuf, BTreeSet<String>>,
    pub(crate) image_book: bool,
    pub(crate) stack_members: Vec<(String, Vec<PathBuf>)>,
    /// Counts are visible projection data too; unchanged rows cannot prove an unchanged chip.
    pub(crate) omitted: folder_scan::OmittedFolderEntryCounts,
    /// Failed preparation is not proof that the current display can be retained.
    pub(crate) complete: bool,
}

/// Pure constructor for already prepared/adopted rows. Dependencies must come from the same
/// preparation; missing pin/representative evidence must be passed with `complete = false`.
pub(crate) fn build_projection_proof(
    revision: ProjectionRevision,
    items: &[GridItem],
    metas: &[Option<(i64, i64)>],
    sort_metas: &[crate::settings::ListingSortMetadata],
    sidecars: impl IntoIterator<Item = (PathBuf, PathBuf)>,
    cache_keys: BTreeMap<PathBuf, BTreeSet<String>>,
    mut dependencies: Vec<ProjectionDependency>,
    image_book: bool,
    stack_members: Vec<(String, Vec<PathBuf>)>,
    omitted: folder_scan::OmittedFolderEntryCounts,
    complete: bool,
) -> FolderProjectionProof {
    dependencies.sort_by(|left, right| {
        (&left.owner, &left.path, &left.identity).cmp(&(&right.owner, &right.path, &right.identity))
    });
    let aligned = items.len() == metas.len() && items.len() == sort_metas.len();
    let rows = items
        .iter()
        .zip(metas)
        .zip(sort_metas)
        .map(|((item, meta), sort_meta)| ProjectionRow {
            item: item.clone(),
            display_meta: *meta,
            sort_meta: *sort_meta,
        })
        .collect();
    FolderProjectionProof {
        revision,
        rows,
        sidecars: sidecars.into_iter().collect(),
        cache_keys,
        dependencies,
        image_book,
        stack_members,
        omitted,
        complete: complete && aligned,
    }
}

impl FolderProjectionProof {
    pub(crate) fn without_sources(&self, successful: &HashSet<PathBuf>) -> Self {
        let mut provisional = self.clone();
        provisional.rows.retain(|row| {
            !row.item
                .drag_source_path()
                .is_some_and(|path| successful.contains(path))
        });
        provisional
            .sidecars
            .retain(|source, _| !successful.contains(source));
        provisional
            .dependencies
            .retain(|dependency| !successful.contains(&dependency.owner));
        provisional
            .cache_keys
            .retain(|owner, _| !successful.contains(owner));
        // Preserve group structure: the actual grouping must prove any member/count changes.
        provisional
    }

    pub(crate) fn matches_visible(&self, items: &[GridItem], metas: &[Option<(i64, i64)>]) -> bool {
        items.len() == self.rows.len()
            && metas.len() == items.len()
            && self
                .rows
                .iter()
                .zip(items.iter().zip(metas))
                .all(|(row, (item, meta))| &row.item == item && &row.display_meta == meta)
    }
}

#[derive(Clone)]
pub(crate) struct PreparedFolderScanInputs {
    pub(crate) materialized: MaterializedFolderListing,
    pub(crate) existing_keys: HashSet<String>,
    pub(crate) proof: FolderProjectionProof,
    pub(crate) projected_items: Vec<GridItem>,
    pub(crate) projected_metas: Vec<Option<(i64, i64)>>,
}

#[derive(Clone, Debug)]
pub(crate) enum FolderProjectionState {
    Applied(FolderProjectionProof),
    NeedsProjection {
        provisional: FolderProjectionProof,
        successful: HashSet<PathBuf>,
    },
}

#[derive(Clone, Debug)]
pub(crate) struct AppliedFolderListing {
    pub(crate) identity: FolderWatchIdentity,
    pub(crate) generation: u64,
    pub(crate) revision: u64,
    pub(crate) raw: Arc<RawFolderSnapshot>,
    pub(crate) signature: folder_scan::FolderScanSignature,
    pub(crate) projection: FolderProjectionState,
    applied_operations: HashSet<FileOperationId>,
}

#[derive(Clone, Debug)]
pub(crate) struct DeleteReceipt {
    pub(crate) id: FileOperationId,
    pub(crate) owner: DeleteListingOwner,
    pub(crate) successful: HashSet<PathBuf>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ReceiptAdoption {
    Applied,
    Duplicate,
    Stale,
}

impl AppliedFolderListing {
    pub(crate) fn new(
        identity: FolderWatchIdentity,
        generation: u64,
        scan: &ScannedDir,
        proof: FolderProjectionProof,
    ) -> Self {
        let projection = if proof.complete {
            FolderProjectionState::Applied(proof)
        } else {
            FolderProjectionState::NeedsProjection {
                provisional: proof,
                successful: HashSet::new(),
            }
        };
        Self {
            identity,
            generation,
            revision: 1,
            raw: Arc::clone(&scan.raw),
            signature: folder_scan::signature_from_scan(scan),
            projection,
            applied_operations: HashSet::new(),
        }
    }

    pub(crate) fn delete_owner(&self) -> DeleteListingOwner {
        DeleteListingOwner {
            context_id: self.identity.context_id,
            folder: self.identity.folder.clone(),
            baseline_revision: self.revision,
            generation: self.generation,
        }
    }

    pub(crate) fn adopt_delete(&mut self, receipt: DeleteReceipt) -> ReceiptAdoption {
        if self.applied_operations.contains(&receipt.id) {
            return ReceiptAdoption::Duplicate;
        }
        if receipt.owner != self.delete_owner() {
            return ReceiptAdoption::Stale;
        }
        let proof = match &self.projection {
            FolderProjectionState::Applied(proof) => proof,
            FolderProjectionState::NeedsProjection { provisional, .. } => provisional,
        };
        let provisional = proof.without_sources(&receipt.successful);
        let mut successful = receipt.successful;
        if let FolderProjectionState::NeedsProjection {
            successful: previous,
            ..
        } = &self.projection
        {
            successful.extend(previous.iter().cloned());
        }
        self.raw = Arc::new(self.raw.without_sources(&successful));
        self.projection = FolderProjectionState::NeedsProjection {
            provisional,
            successful,
        };
        self.applied_operations.insert(receipt.id);
        self.revision += 1;
        ReceiptAdoption::Applied
    }

    pub(crate) fn proof(&self) -> &FolderProjectionProof {
        match &self.projection {
            FolderProjectionState::Applied(proof) => proof,
            FolderProjectionState::NeedsProjection { provisional, .. } => provisional,
        }
    }

    pub(crate) fn can_retain(
        &self,
        scan: &ScannedDir,
        proof: &FolderProjectionProof,
        dirty: &WatchDirtySnapshot,
    ) -> bool {
        let successful = match &self.projection {
            FolderProjectionState::Applied(_) => None,
            FolderProjectionState::NeedsProjection { successful, .. } => Some(successful),
        };
        self.raw.same_listing(&scan.raw)
            && self.proof().complete
            && proof.complete
            && self.proof() == proof
            && !dirty.forces_install(
                &self.raw,
                &scan.raw,
                proof,
                successful,
                &self.identity.folder,
            )
    }

    /// Call only at the successful visible-adoption/retain boundary, together with watch ack.
    pub(crate) fn adopt_scan(
        &mut self,
        scan: &ScannedDir,
        proof: FolderProjectionProof,
        generation: u64,
    ) {
        self.raw = Arc::clone(&scan.raw);
        self.signature = folder_scan::signature_from_scan(scan);
        self.projection = if proof.complete {
            FolderProjectionState::Applied(proof)
        } else {
            FolderProjectionState::NeedsProjection {
                provisional: proof,
                successful: HashSet::new(),
            }
        };
        self.generation = generation;
        self.revision += 1;
    }
}

/// Keep §1.314 display/sort/request stamps while advancing real directory observations.
pub(crate) fn preserve_folder_display_metadata(
    scan: &mut ScannedDir,
    baseline: &AppliedFolderListing,
) {
    for entry in &mut scan.folders {
        let GridItem::Folder(path) = &entry.item else {
            continue;
        };
        let same_directory = baseline
            .raw
            .observations
            .get(path)
            .zip(scan.raw.observations.get(path))
            .is_some_and(|(before, after)| {
                before.kind.is_directory() && before.same_listing(after)
            });
        if same_directory {
            if let Some(previous) = baseline
                .proof()
                .rows
                .iter()
                .find(|old| old.item == entry.item)
            {
                entry.display_meta = previous.display_meta;
                entry.sort_meta = previous.sort_meta;
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum WatchChangeKind {
    Namespace,
    Metadata,
    AnyModify,
    Content,
    Unknown,
}

#[derive(Clone, Debug)]
pub(crate) struct WatchDirtyRecord {
    pub(crate) sequence: u64,
    pub(crate) kind: WatchChangeKind,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct WatchDirtySummary {
    pub(crate) observed_watch_sequence: u64,
    pub(crate) reconciled_watch_sequence: u64,
    records: BTreeMap<PathBuf, WatchDirtyRecord>,
    unknown_sequence: Option<u64>,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct WatchDirtySnapshot {
    pub(crate) cutoff: u64,
    pub(crate) records: BTreeMap<PathBuf, WatchDirtyRecord>,
    pub(crate) unknown: bool,
}

impl WatchDirtySummary {
    const MAX_PATHS: usize = 512;

    pub(crate) fn record(
        &mut self,
        kind: WatchChangeKind,
        paths: impl IntoIterator<Item = PathBuf>,
    ) {
        self.observed_watch_sequence += 1;
        let sequence = self.observed_watch_sequence;
        if kind == WatchChangeKind::Unknown {
            self.unknown_sequence = Some(sequence);
            return;
        }
        let mut saw_path = false;
        for path in paths {
            saw_path = true;
            if self.records.len() >= Self::MAX_PATHS && !self.records.contains_key(&path) {
                self.unknown_sequence = Some(sequence);
                break;
            }
            self.records
                .entry(path)
                .and_modify(|record| {
                    record.sequence = sequence;
                    record.kind = record.kind.max(kind);
                })
                .or_insert(WatchDirtyRecord { sequence, kind });
        }
        if !saw_path {
            self.unknown_sequence = Some(sequence);
        }
    }

    pub(crate) fn record_notify(&mut self, event: notify::Result<notify::Event>) {
        use notify::event::ModifyKind;
        let event = match event {
            Ok(event) => event,
            Err(_) => {
                self.record(WatchChangeKind::Unknown, []);
                return;
            }
        };
        let kind = match event.kind {
            notify::EventKind::Access(_) => return,
            notify::EventKind::Create(_)
            | notify::EventKind::Remove(_)
            | notify::EventKind::Modify(ModifyKind::Name(_)) => WatchChangeKind::Namespace,
            notify::EventKind::Modify(ModifyKind::Data(_)) => WatchChangeKind::Content,
            notify::EventKind::Modify(ModifyKind::Metadata(_)) => WatchChangeKind::Metadata,
            notify::EventKind::Modify(ModifyKind::Any) => WatchChangeKind::AnyModify,
            _ => WatchChangeKind::Unknown,
        };
        self.record(kind, event.paths);
    }

    pub(crate) fn snapshot(&self) -> WatchDirtySnapshot {
        WatchDirtySnapshot {
            cutoff: self.observed_watch_sequence,
            records: self.records.clone(),
            unknown: self.unknown_sequence.is_some(),
        }
    }

    pub(crate) fn is_dirty(&self) -> bool {
        self.observed_watch_sequence > self.reconciled_watch_sequence
    }

    /// A later event for the same path keeps its stronger evidence beyond this cutoff.
    pub(crate) fn acknowledge(&mut self, cutoff: u64) {
        let cutoff = cutoff.min(self.observed_watch_sequence);
        self.reconciled_watch_sequence = self.reconciled_watch_sequence.max(cutoff);
        self.records.retain(|_, record| record.sequence > cutoff);
        if self
            .unknown_sequence
            .is_some_and(|sequence| sequence <= cutoff)
        {
            self.unknown_sequence = None;
        }
    }
}

impl WatchDirtySnapshot {
    pub(crate) fn forces_install(
        &self,
        expected: &RawFolderSnapshot,
        actual: &RawFolderSnapshot,
        proof: &FolderProjectionProof,
        successful: Option<&HashSet<PathBuf>>,
        folder: &Path,
    ) -> bool {
        if self.unknown {
            return true;
        }
        let path_key = |path: &Path| {
            crate::path_key::physical_path_spelling(path)
                .unwrap_or_else(|| crate::path_key::normalize_keep_drive(path))
        };
        let expected_stamps = expected
            .observations
            .iter()
            .map(|(path, stamp)| (path_key(path), stamp))
            .collect::<HashMap<_, _>>();
        let actual_stamps = actual
            .observations
            .iter()
            .map(|(path, stamp)| (path_key(path), stamp))
            .collect::<HashMap<_, _>>();
        let deleted = successful
            .into_iter()
            .flat_map(|sources| sources.iter().map(|path| path_key(path)))
            .collect::<HashSet<_>>();
        let dependencies = proof
            .dependencies
            .iter()
            .map(|dependency| path_key(&dependency.path))
            .collect::<HashSet<_>>();
        let root_key = path_key(folder);
        self.records.iter().any(|(path, record)| {
            if record.kind == WatchChangeKind::Namespace {
                return false;
            }
            let key = path_key(path);
            if deleted.contains(&key)
                && !actual_stamps.contains_key(&key)
                && !dependencies.contains(&key)
            {
                return false;
            }
            if matches!(
                record.kind,
                WatchChangeKind::Metadata | WatchChangeKind::AnyModify
            ) {
                // The root is known directory coverage; membership is proved by the actual scan.
                if key == root_key {
                    return false;
                }
                if expected_stamps
                    .get(&key)
                    .zip(actual_stamps.get(&key))
                    .is_some_and(|(before, after)| {
                        before.kind.is_directory() && before.same_listing(after)
                    })
                {
                    return false;
                }
            }
            true
        })
    }
}

pub(crate) struct FolderWatchSubscription {
    pub(crate) identity: FolderWatchIdentity,
    pub(crate) dirty: Arc<Mutex<WatchDirtySummary>>,
    // Registration and destruction never affect another context's subscription.
    _watcher: notify::RecommendedWatcher,
}

impl FolderWatchSubscription {
    /// Worker only. Subscribe before the initial scan, including candidate-list preparation.
    pub(crate) fn register(
        identity: FolderWatchIdentity,
        wake: egui::Context,
    ) -> notify::Result<Self> {
        use notify::Watcher;
        let dirty = Arc::new(Mutex::new(WatchDirtySummary::default()));
        let callback_dirty = Arc::clone(&dirty);
        let watcher_identity = identity.clone();
        let mut watcher = notify::recommended_watcher(move |event| {
            // Immutable subscription identity is captured; mounted App state is never consulted.
            let _identity = &watcher_identity;
            if let Ok(mut summary) = callback_dirty.lock() {
                summary.record_notify(event);
            }
            wake.request_repaint();
        })?;
        watcher.watch(&identity.folder, notify::RecursiveMode::NonRecursive)?;
        Ok(Self {
            identity,
            dirty,
            _watcher: watcher,
        })
    }

    pub(crate) fn snapshot(&self) -> WatchDirtySnapshot {
        self.dirty
            .lock()
            .map(|summary| summary.snapshot())
            .unwrap_or_else(|_| WatchDirtySnapshot {
                unknown: true,
                ..Default::default()
            })
    }

    pub(crate) fn acknowledge(&self, cutoff: u64) {
        if let Ok(mut summary) = self.dirty.lock() {
            summary.acknowledge(cutoff);
        }
    }
}

/// Worker-only handoff: even the first observation is covered by the candidate subscription.
pub(crate) fn watch_and_scan(
    identity: FolderWatchIdentity,
    inputs: &FolderPrepareInputs,
    cancel: &Arc<AtomicBool>,
    wake: egui::Context,
) -> std::io::Result<(FolderWatchSubscription, ScannedDir, WatchDirtySnapshot)> {
    let watch = FolderWatchSubscription::register(identity, wake)
        .map_err(|error| std::io::Error::other(error.to_string()))?;
    let cutoff = watch.snapshot();
    let mut scan = folder_scan::scan_directory_with_visibility_cancel(
        &watch.identity.folder,
        !inputs.settings.archive_file_handling_ignores_convertible(),
        !inputs.settings.epub_file_handling_ignores_epub(),
        inputs.settings.show_hidden_files,
        &inputs.settings.file_type_visibility,
        Some(cancel),
    )?;
    prepare_scan_inputs(&watch.identity.folder, &mut scan, inputs, cancel)?;
    Ok((watch, scan, cutoff))
}

/// Executes existing physical projection, DB reads, pin resolution and Rhai grouping off UI.
pub(crate) fn prepare_scan_inputs(
    path: &Path,
    scan: &mut ScannedDir,
    inputs: &FolderPrepareInputs,
    cancel: &Arc<AtomicBool>,
) -> std::io::Result<()> {
    let revision = inputs.revision();
    let settings = &inputs.settings;
    let order = crate::rating_sort::ListingOrderRequest::from_settings(settings);
    let materialized = folder_scan::materialize_local_folder_listing_with_order(
        path,
        scan.clone(),
        settings,
        order,
        |keys| {
            crate::rating_db::RatingDb::open_readonly(
                inputs
                    .rating_db_path
                    .as_deref()
                    .ok_or_else(|| "rating DB unavailable".to_owned())?,
            )
            .map_err(|error| error.to_string())?
            .get_many_complete(keys, inputs.rating_write_generation)
            .map_err(|error| error.to_string())
        },
    );
    if cancel.load(Ordering::Relaxed) {
        return Err(std::io::ErrorKind::Interrupted.into());
    }
    let containers = materialized
        .items
        .iter()
        .filter_map(|item| match item {
            GridItem::Folder(path)
            | GridItem::ZipFile(path)
            | GridItem::PdfFile(path)
            | GridItem::ConvertibleArchive { path, .. } => Some(path),
            _ => None,
        })
        .collect::<Vec<_>>();
    let pins = inputs
        .pin_db
        .as_ref()
        .map(|db| db.lookup_many(containers))
        .unwrap_or_default();
    let mut cache_keys = BTreeMap::new();
    let mut existing_keys = HashSet::new();
    let mut dependencies = Vec::new();
    // A standalone selection cannot prove what the existing delayed thumbnail worker will show.
    // Until their provenance/adoption boundary is agreed, these projections remain unproven.
    let mut needs_automatic_representative = false;
    for (item, meta) in materialized.items.iter().zip(&materialized.metas) {
        let Some(owner) = item.drag_source_path() else {
            continue;
        };
        let keys = super::folder_thumb_existing_keys_for_with_visibility(
            item,
            *meta,
            &pins,
            inputs.pin_db.as_deref(),
            Some(settings.folder_thumb_sort),
            settings.folder_thumb_depth,
            inputs.use_full_path_keys,
            &settings.file_type_visibility,
        );
        existing_keys.extend(keys.iter().cloned());
        cache_keys.insert(owner.to_path_buf(), keys.into_iter().collect());
        if let Some(pin) = pins.get(&crate::path_key::normalize_keep_drive(owner)) {
            let resolved = match meta {
                Some(metadata) => super::resolve_pin_target_cascaded_with_root_metadata(
                    owner,
                    pin,
                    inputs.pin_db.as_deref(),
                    settings.folder_thumb_depth as usize,
                    *metadata,
                ),
                None => super::resolve_pin_target_cascaded(
                    owner,
                    pin,
                    inputs.pin_db.as_deref(),
                    settings.folder_thumb_depth as usize,
                ),
            };
            if let Some(resolved) = resolved {
                if matches!(
                    resolved.kind,
                    crate::folder_thumb_pins::ResolvedKind::Folder
                ) {
                    needs_automatic_representative = true;
                }
                let metadata = std::fs::metadata(&resolved.abs_path).ok();
                let kind = if metadata.as_ref().is_some_and(|meta| meta.is_dir()) {
                    crate::fs_entry::DirEntryKind::Directory
                } else {
                    crate::fs_entry::DirEntryKind::File
                };
                dependencies.push(ProjectionDependency {
                    owner: owner.to_path_buf(),
                    path: resolved.abs_path,
                    stamp: ObservedEntryStamp::from_metadata(kind, metadata.as_ref()),
                    identity: resolved.source_id,
                });
            } else if matches!(item, GridItem::Folder(_)) {
                needs_automatic_representative = true;
            }
        } else if matches!(item, GridItem::Folder(_)) {
            needs_automatic_representative = true;
        }
    }
    if cancel.load(Ordering::Relaxed) {
        return Err(std::io::ErrorKind::Interrupted.into());
    }
    for (source, sidecar) in &materialized.video_thumb_overrides {
        let metadata = std::fs::metadata(sidecar).ok();
        dependencies.push(ProjectionDependency {
            owner: source.clone(),
            path: sidecar.clone(),
            stamp: ObservedEntryStamp::from_metadata(
                crate::fs_entry::DirEntryKind::File,
                metadata.as_ref(),
            ),
            identity: "sidecar".into(),
        });
    }
    dependencies.sort_by(|left, right| {
        (&left.owner, &left.path, &left.identity).cmp(&(&right.owner, &right.path, &right.identity))
    });
    let (projected_items, projected_metas, stack_members) = if inputs.stack_mode {
        prepare_stack_projection(path, &materialized, inputs, cancel)
    } else {
        (
            materialized.items.clone(),
            materialized.metas.clone(),
            Vec::new(),
        )
    };
    if cancel.load(Ordering::Relaxed) {
        return Err(std::io::ErrorKind::Interrupted.into());
    }
    let sort_by_path = materialized
        .items
        .iter()
        .zip(&materialized.sort_metas)
        .filter_map(|(item, meta)| Some((item.drag_source_path()?.to_path_buf(), *meta)))
        .collect::<HashMap<_, _>>();
    let rows = projected_items
        .iter()
        .zip(&projected_metas)
        .map(|(item, meta)| {
            let sort_meta = item
                .drag_source_path()
                .and_then(|path| sort_by_path.get(path).copied())
                .unwrap_or_else(|| {
                    crate::settings::ListingSortMetadata::new(
                        meta.map_or(0, |value| value.0),
                        meta.map(|value| value.1),
                    )
                });
            ProjectionRow {
                item: item.clone(),
                display_meta: *meta,
                sort_meta,
            }
        })
        .collect();
    let image_book = if crate::books::is_direct_book_folder(&settings.books_root_path(), path) {
        scan.all_media
            .iter()
            .any(|entry| entry.kind == folder_scan::ScanMediaKind::Image)
    } else {
        folder_scan::is_image_only_book_contents(!scan.folders.is_empty(), &scan.all_media)
    };
    let complete = !matches!(
        materialized.rating_order,
        folder_scan::FolderRatingOrder::Failed
    ) && !needs_automatic_representative
        && dependencies
            .iter()
            .all(|dependency| dependency.stamp.modified.is_some())
        && revision == inputs.revision();
    let proof = FolderProjectionProof {
        revision,
        rows,
        sidecars: materialized.video_thumb_overrides.iter().cloned().collect(),
        dependencies,
        cache_keys,
        image_book,
        stack_members,
        omitted: materialized.omitted,
        complete,
    };
    scan.prepared = Some(Box::new(PreparedFolderScanInputs {
        materialized,
        existing_keys,
        proof,
        projected_items,
        projected_metas,
    }));
    Ok(())
}

pub(crate) fn prepare_reconcile_scan_inputs(
    path: &Path,
    scan: &mut ScannedDir,
    inputs: &FolderPrepareInputs,
    cancel: &Arc<AtomicBool>,
    baseline: &AppliedFolderListing,
) -> std::io::Result<()> {
    preserve_folder_display_metadata(scan, baseline);
    prepare_scan_inputs(path, scan, inputs, cancel)?;
    let prepared = scan.prepared.as_mut().expect("prepared scan");
    let proof = &mut prepared.proof;
    let same_revision = baseline.proof().revision == proof.revision;
    // A real directory's accepted representative stamp is retained alongside its display stamp.
    // Files/containers/changed identities never receive this exception.
    for dependency in &mut proof.dependencies {
        if dependency.stamp.kind.is_directory() {
            if let Some(previous) = baseline.proof().dependencies.iter().find(|previous| {
                previous.owner == dependency.owner
                    && previous.path == dependency.path
                    && previous.stamp.same_listing(&dependency.stamp)
                    && same_revision
            }) {
                let new_identity = dependency.identity.clone();
                let old_identity = previous.identity.clone();
                if let Some(keys) = proof.cache_keys.get_mut(&dependency.owner) {
                    *keys = keys
                        .iter()
                        .map(|key| key.replace(&new_identity, &old_identity))
                        .collect();
                }
                dependency.identity = old_identity;
                dependency.stamp = previous.stamp.clone();
            }
        }
    }
    prepared.existing_keys = proof
        .cache_keys
        .values()
        .flat_map(|keys| keys.iter().cloned())
        .collect();
    Ok(())
}

fn prepare_stack_projection(
    path: &Path,
    materialized: &MaterializedFolderListing,
    inputs: &FolderPrepareInputs,
    cancel: &Arc<AtomicBool>,
) -> (
    Vec<GridItem>,
    Vec<Option<(i64, i64)>>,
    Vec<(String, Vec<PathBuf>)>,
) {
    let (passthrough, passthrough_metas, passthrough_sort, media) =
        crate::filename_stack_ui::extract_stack_parts(
            &materialized.items,
            &materialized.metas,
            &materialized.sort_metas,
        );
    let images = media
        .iter()
        .filter(|member| !member.is_video)
        .cloned()
        .collect::<Vec<_>>();
    let script = if inputs.settings.stack_script_enabled {
        crate::filename_stack_script::active_script_source()
    } else {
        crate::filename_stack_script::DEFAULT_SCRIPT.to_owned()
    };
    let image_keys =
        crate::filename_stack_script::group_keys_cancellable(&images, &script, Arc::clone(cancel));
    let mut image_key_iter = image_keys.as_ref().ok().map(|result| result.keys.iter());
    let keys = media
        .iter()
        .map(|member| {
            if member.is_video {
                String::new()
            } else if let Some(key) = image_key_iter.as_mut().and_then(|keys| keys.next()) {
                key.clone()
            } else {
                crate::filename_stack::prefix_of(
                    member
                        .path
                        .file_stem()
                        .and_then(|name| name.to_str())
                        .unwrap_or(""),
                    inputs.settings.stack_separator,
                )
                .to_owned()
            }
        })
        .collect::<Vec<_>>();
    let ratings = match &materialized.rating_order {
        folder_scan::FolderRatingOrder::Prepared { cache, .. } => Some(Arc::new(
            materialized
                .items
                .iter()
                .enumerate()
                .filter_map(|(index, item)| {
                    Some((
                        crate::rating_sort::physical_item_key(item)?,
                        crate::rating_sort::RatingSortKey::supported(*cache.get(&index)?).unwrap(),
                    ))
                })
                .collect::<HashMap<_, _>>(),
        )),
        _ => None,
    };
    let order = if ratings.is_some() {
        crate::rating_sort::ListingOrderRequest::from_settings(&inputs.settings)
    } else {
        crate::rating_sort::ListingOrderRequest::Standard(materialized.folder_sort)
    };
    let groups =
        crate::filename_stack::group_by_keys_with_order(media, &keys, order, ratings.as_deref());
    let members = groups
        .iter()
        .map(|group| {
            (
                group.key.clone(),
                group
                    .members
                    .iter()
                    .map(|member| member.path.clone())
                    .collect(),
            )
        })
        .collect();
    let mut view = crate::filename_stack::StackView::from_groups_with_display_order(
        path.to_path_buf(),
        passthrough,
        passthrough_metas,
        passthrough_sort,
        inputs.settings.stack_separator,
        order.standard_fallback(),
        groups,
        inputs.settings.grid_display_order.clone(),
    );
    if let (crate::rating_sort::ListingOrderRequest::Rating(spec), Some(ratings)) = (order, ratings)
    {
        view.set_rating_order(spec, ratings);
    }
    let (items, metas) = if inputs.stack_flat {
        view.materialize_flat()
    } else {
        view.materialize_aggregated()
    };
    (items, metas, members)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn inputs() -> FolderPrepareInputs {
        FolderPrepareInputs {
            settings: crate::settings::Settings::default(),
            rating_db_path: None,
            rating_write_generation: 0,
            pin_db: None,
            use_full_path_keys: false,
            stack_mode: false,
            stack_flat: false,
            metadata_revision: 0,
        }
    }

    fn scan(path: &Path, inputs: &FolderPrepareInputs) -> ScannedDir {
        let mut scan = folder_scan::scan_directory_with_settings(path, &inputs.settings).unwrap();
        prepare_scan_inputs(path, &mut scan, inputs, &Arc::new(AtomicBool::new(false))).unwrap();
        scan
    }

    fn baseline(path: &Path, scan: &ScannedDir) -> AppliedFolderListing {
        AppliedFolderListing::new(
            FolderWatchIdentity {
                context_id: ViewerContextId::for_test(1),
                subscription_epoch: 1,
                folder: path.to_path_buf(),
                listing_options: "test".into(),
            },
            5,
            scan,
            scan.prepared.as_ref().unwrap().proof.clone(),
        )
    }

    fn receipt(listing: &AppliedFolderListing, id: u64, paths: &[PathBuf]) -> DeleteReceipt {
        DeleteReceipt {
            id,
            owner: listing.delete_owner(),
            successful: paths.iter().cloned().collect(),
        }
    }

    fn fixture(names: &[&str]) -> tempfile::TempDir {
        let temp = tempfile::tempdir().unwrap();
        for name in names {
            std::fs::write(temp.path().join(name), b"fixture").unwrap();
        }
        temp
    }

    fn different_stamp_in_same_second(time: std::time::SystemTime) -> std::time::SystemTime {
        let before = time.duration_since(std::time::UNIX_EPOCH).unwrap();
        // Windows SystemTime is quantized to 100 ns. Stay in this same second even at its end.
        let quantum = std::time::Duration::from_nanos(100);
        let changed = if before.subsec_nanos() >= 999_999_900 {
            time - quantum
        } else {
            time + quantum
        };
        assert_ne!(
            time, changed,
            "fixture must change the full-precision observation"
        );
        assert_eq!(
            before.as_secs(),
            changed
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs(),
            "fixture must preserve the seconds used by the legacy signature"
        );
        changed
    }

    #[test]
    fn successful_delete_retains_only_after_raw_and_projection_proof() {
        let temp = fixture(&["a.jpg", "b.jpg"]);
        let inputs = inputs();
        let before = scan(temp.path(), &inputs);
        let mut listing = baseline(temp.path(), &before);
        let deleted = temp.path().join("a.jpg");
        let receipt = receipt(&listing, 1, &[deleted.clone()]);
        assert_eq!(
            listing.adopt_delete(receipt.clone()),
            ReceiptAdoption::Applied
        );
        assert!(matches!(
            listing.projection,
            FolderProjectionState::NeedsProjection { .. }
        ));
        assert_eq!(listing.adopt_delete(receipt), ReceiptAdoption::Duplicate);
        std::fs::remove_file(&deleted).unwrap();
        let after = scan(temp.path(), &inputs);
        let mut dirty = WatchDirtySummary::default();
        dirty.record(WatchChangeKind::Namespace, [deleted]);
        assert!(listing.can_retain(
            &after,
            &after.prepared.as_ref().unwrap().proof,
            &dirty.snapshot()
        ));
    }

    #[test]
    fn hidden_duplicate_exposure_is_a_new_projection_despite_expected_raw_equality() {
        let temp = fixture(&["a.jpg", "a.png", "b.jpg"]);
        let mut inputs = inputs();
        inputs.settings.skip_duplicate_images = true;
        inputs.settings.image_ext_priority = vec!["jpg".into(), "png".into()];
        let before = scan(temp.path(), &inputs);
        let mut listing = baseline(temp.path(), &before);
        let deleted = temp.path().join("a.jpg");
        listing.adopt_delete(receipt(&listing, 1, &[deleted.clone()]));
        std::fs::remove_file(&deleted).unwrap();
        let after = scan(temp.path(), &inputs);
        assert!(listing.raw.same_listing(&after.raw));
        assert!(!listing.can_retain(
            &after,
            &after.prepared.as_ref().unwrap().proof,
            &Default::default()
        ));
        assert!(
            after
                .prepared
                .as_ref()
                .unwrap()
                .proof
                .rows
                .iter()
                .any(|row| row.item == GridItem::Image(temp.path().join("a.png")))
        );
    }

    #[test]
    fn folder_zip_and_pdf_epub_hidden_candidates_reappear() {
        for (removed, alternative, folder) in [("a", "a.zip", true), ("a.pdf", "a.epub", false)] {
            let temp = fixture(&[alternative, "b.jpg"]);
            let path = temp.path().join(removed);
            if folder {
                std::fs::create_dir(&path).unwrap();
            } else {
                std::fs::write(&path, b"fixture").unwrap();
            }
            let mut inputs = inputs();
            inputs.settings.skip_zip_if_folder_exists = true;
            inputs.settings.skip_epub_if_pdf_exists = true;
            let before = scan(temp.path(), &inputs);
            let mut listing = baseline(temp.path(), &before);
            listing.adopt_delete(receipt(&listing, 1, &[path.clone()]));
            if folder {
                std::fs::remove_dir(&path).unwrap();
            } else {
                std::fs::remove_file(&path).unwrap();
            }
            let after = scan(temp.path(), &inputs);
            assert!(listing.raw.same_listing(&after.raw));
            assert!(!listing.can_retain(
                &after,
                &after.prepared.as_ref().unwrap().proof,
                &Default::default()
            ));
        }
    }

    #[test]
    fn deleting_selected_sidecar_changes_dependency_and_exposes_image() {
        let temp = fixture(&["movie.mp4", "movie.jpg", "b.jpg"]);
        let mut inputs = inputs();
        inputs.settings.skip_image_if_video_exists = true;
        inputs.settings.video_thumb_use_sidecar_image = true;
        let before = scan(temp.path(), &inputs);
        let mut listing = baseline(temp.path(), &before);
        let deleted = temp.path().join("movie.mp4");
        listing.adopt_delete(receipt(&listing, 1, &[deleted.clone()]));
        std::fs::remove_file(&deleted).unwrap();
        let after = scan(temp.path(), &inputs);
        assert!(listing.raw.same_listing(&after.raw));
        assert!(!listing.can_retain(
            &after,
            &after.prepared.as_ref().unwrap().proof,
            &Default::default()
        ));
        assert!(after.prepared.as_ref().unwrap().proof.sidecars.is_empty());
    }

    #[test]
    fn full_precision_stamp_catches_same_second_same_size_overwrite() {
        let temp = fixture(&["a.jpg"]);
        let inputs = inputs();
        let before = scan(temp.path(), &inputs);
        let listing = baseline(temp.path(), &before);
        let mut after = before.clone();
        let raw = Arc::make_mut(&mut after.raw);
        let stamp = raw
            .observations
            .get_mut(&temp.path().join("a.jpg"))
            .unwrap();
        let original_size = stamp.size;
        stamp.modified = stamp.modified.map(different_stamp_in_same_second);
        assert_eq!(stamp.size, original_size);
        assert_eq!(
            folder_scan::signature_from_scan(&before),
            folder_scan::signature_from_scan(&after)
        );
        assert!(!listing.can_retain(
            &after,
            &after.prepared.as_ref().unwrap().proof,
            &Default::default()
        ));
    }

    #[test]
    fn actual_directory_mtime_only_preserves_display_sort_and_dependency_stamps() {
        let temp = fixture(&["b.jpg"]);
        let child = temp.path().join("child");
        std::fs::create_dir(&child).unwrap();
        let inputs = inputs();
        let before = scan(temp.path(), &inputs);
        let mut listing = baseline(temp.path(), &before);
        let original = listing
            .proof()
            .rows
            .iter()
            .find(|row| row.item == GridItem::Folder(child.clone()))
            .unwrap()
            .clone();
        for increment in [10, 20] {
            let mut after =
                folder_scan::scan_directory_with_settings(temp.path(), &inputs.settings).unwrap();
            let raw = Arc::make_mut(&mut after.raw);
            let stamp = raw.observations.get_mut(&child).unwrap();
            stamp.modified = stamp
                .modified
                .map(|time| time + std::time::Duration::from_secs(increment));
            for entry in &mut after.folders {
                if entry.item == GridItem::Folder(child.clone()) {
                    entry.display_meta =
                        Some((original.display_meta.unwrap().0 + increment as i64, 0));
                    entry.sort_meta.mtime += increment as i64;
                }
            }
            prepare_reconcile_scan_inputs(
                temp.path(),
                &mut after,
                &inputs,
                &Arc::new(AtomicBool::new(false)),
                &listing,
            )
            .unwrap();
            let mut dirty = WatchDirtySummary::default();
            dirty.record(WatchChangeKind::AnyModify, [child.clone()]);
            let proof = after.prepared.as_ref().unwrap().proof.clone();
            assert!(listing.raw.same_listing(&after.raw));
            assert!(
                !dirty.snapshot().forces_install(
                    &listing.raw,
                    &after.raw,
                    &proof,
                    None,
                    temp.path()
                ),
                "typed directory mtime-only evidence itself does not force an install"
            );
            assert!(
                !listing.can_retain(&after, &proof, &dirty.snapshot()),
                "automatic representative provenance remains unproven at this checkpoint"
            );
            listing.adopt_scan(&after, proof, 5);
            assert!(matches!(
                listing.projection,
                FolderProjectionState::NeedsProjection { .. }
            ));
            assert_eq!(
                listing
                    .proof()
                    .rows
                    .iter()
                    .find(|row| row.item == original.item),
                Some(&original)
            );
        }
    }

    #[test]
    fn namespace_batch_cannot_hide_remaining_file_content_or_unknown() {
        let temp = fixture(&["b.jpg"]);
        let inputs = inputs();
        let scan = scan(temp.path(), &inputs);
        let listing = baseline(temp.path(), &scan);
        for kind in [
            WatchChangeKind::Content,
            WatchChangeKind::Metadata,
            WatchChangeKind::AnyModify,
            WatchChangeKind::Unknown,
        ] {
            let mut dirty = WatchDirtySummary::default();
            dirty.record(WatchChangeKind::Namespace, [temp.path().join("a.jpg")]);
            dirty.record(kind, [temp.path().join("b.jpg")]);
            assert!(!listing.can_retain(
                &scan,
                &scan.prepared.as_ref().unwrap().proof,
                &dirty.snapshot()
            ));
        }
    }

    #[test]
    fn notify_name_is_namespace_and_access_is_not_mutation() {
        let mut dirty = WatchDirtySummary::default();
        dirty.record_notify(Ok(notify::Event::new(notify::EventKind::Modify(
            notify::event::ModifyKind::Name(notify::event::RenameMode::Both),
        ))
        .add_path(PathBuf::from("old"))
        .add_path(PathBuf::from("new"))));
        assert_eq!(dirty.snapshot().records.len(), 2);
        assert!(
            dirty
                .snapshot()
                .records
                .values()
                .all(|record| record.kind == WatchChangeKind::Namespace)
        );
        let sequence = dirty.observed_watch_sequence;
        dirty.record_notify(Ok(notify::Event::new(notify::EventKind::Access(
            notify::event::AccessKind::Any,
        ))));
        assert_eq!(dirty.observed_watch_sequence, sequence);
    }

    #[test]
    fn cutoff_keeps_later_stronger_evidence_for_the_same_path() {
        let mut dirty = WatchDirtySummary::default();
        let path = PathBuf::from("b.jpg");
        dirty.record(WatchChangeKind::Namespace, [path.clone()]);
        let first = dirty.snapshot();
        dirty.record(WatchChangeKind::Content, [path.clone()]);
        dirty.acknowledge(first.cutoff);
        assert!(dirty.is_dirty());
        assert_eq!(
            dirty.snapshot().records[&path].kind,
            WatchChangeKind::Content
        );
        let last = dirty.snapshot();
        dirty.acknowledge(last.cutoff);
        assert!(!dirty.is_dirty());
        assert!(dirty.snapshot().records.is_empty());
    }

    #[test]
    fn overflow_and_error_promote_to_bounded_unknown() {
        let mut dirty = WatchDirtySummary::default();
        dirty.record(
            WatchChangeKind::Namespace,
            (0..600).map(|index| PathBuf::from(index.to_string())),
        );
        assert_eq!(dirty.records.len(), WatchDirtySummary::MAX_PATHS);
        assert!(dirty.snapshot().unknown);
        dirty.acknowledge(dirty.snapshot().cutoff);
        dirty.record_notify(Err(notify::Error::generic("coverage lost")));
        assert!(dirty.is_dirty());
        assert!(dirty.snapshot().unknown);
    }

    #[test]
    fn contexts_share_neither_receipts_nor_dirty_ack() {
        let temp = fixture(&["a.jpg", "b.jpg"]);
        let scan = scan(temp.path(), &inputs());
        let mut a = baseline(temp.path(), &scan);
        let mut b = a.clone();
        b.identity.context_id = ViewerContextId::for_test(2);
        let receipt = receipt(&a, 1, &[temp.path().join("a.jpg")]);
        assert_eq!(b.adopt_delete(receipt.clone()), ReceiptAdoption::Stale);
        assert_eq!(a.adopt_delete(receipt), ReceiptAdoption::Applied);
        assert_eq!(b.raw.observations.len(), 2);
        let mut a_dirty = WatchDirtySummary::default();
        let mut b_dirty = WatchDirtySummary::default();
        for dirty in [&mut a_dirty, &mut b_dirty] {
            dirty.record(WatchChangeKind::Content, [temp.path().join("b.jpg")]);
        }
        a_dirty.acknowledge(a_dirty.snapshot().cutoff);
        assert!(
            b_dirty.is_dirty(),
            "parked context retains its own event without another notification"
        );
    }

    #[test]
    fn metadata_revision_change_and_external_same_name_recreation_require_install() {
        let temp = fixture(&["a.jpg", "b.jpg"]);
        let mut inputs = inputs();
        let before = scan(temp.path(), &inputs);
        let mut listing = baseline(temp.path(), &before);
        let path = temp.path().join("a.jpg");
        listing.adopt_delete(receipt(&listing, 1, &[path.clone()]));
        // Recreated A is in actual, but is absent from the expected observation owner.
        let actual = scan(temp.path(), &inputs);
        assert!(!listing.can_retain(
            &actual,
            &actual.prepared.as_ref().unwrap().proof,
            &Default::default()
        ));
        std::fs::remove_file(&path).unwrap();
        inputs.metadata_revision += 1;
        let actual = scan(temp.path(), &inputs);
        assert!(listing.raw.same_listing(&actual.raw));
        assert!(!listing.can_retain(
            &actual,
            &actual.prepared.as_ref().unwrap().proof,
            &Default::default()
        ));
    }

    #[test]
    fn audio_sidecar_and_external_addition_are_not_explained_by_delete() {
        let temp = fixture(&["song.mp3", "song.png", "a.jpg", "b.jpg"]);
        let mut inputs = inputs();
        inputs.settings.skip_image_if_video_exists = true;
        inputs.settings.video_thumb_use_sidecar_image = true;
        inputs
            .settings
            .file_type_visibility
            .excluded_extensions
            .insert("png".into());
        let before = scan(temp.path(), &inputs);
        assert_eq!(before.prepared.as_ref().unwrap().proof.sidecars.len(), 1);
        let mut listing = baseline(temp.path(), &before);
        let deleted = temp.path().join("a.jpg");
        listing.adopt_delete(receipt(&listing, 1, &[deleted.clone()]));
        std::fs::remove_file(&deleted).unwrap();
        std::fs::write(temp.path().join("c.jpg"), b"external").unwrap();
        let actual = scan(temp.path(), &inputs);
        assert!(!listing.can_retain(
            &actual,
            &actual.prepared.as_ref().unwrap().proof,
            &Default::default()
        ));
        std::fs::remove_file(temp.path().join("c.jpg")).unwrap();
        let mut actual = scan(temp.path(), &inputs);
        let sidecar = temp.path().join("song.png");
        let stamp = Arc::make_mut(&mut actual.raw)
            .observations
            .get_mut(&sidecar)
            .unwrap();
        let original_size = stamp.size;
        stamp.modified = stamp.modified.map(different_stamp_in_same_second);
        assert_eq!(stamp.size, original_size);
        assert!(!listing.can_retain(
            &actual,
            &actual.prepared.as_ref().unwrap().proof,
            &Default::default()
        ));
    }

    #[test]
    fn stack_proof_uses_real_script_grouping_and_all_member_identities() {
        // Released DEFAULT_SCRIPT requires all filenames to match and at least two distinct keys.
        let temp = fixture(&["post_p0.jpg", "post_p1.jpg", "other_p0.jpg"]);
        let mut inputs = inputs();
        inputs.stack_mode = true;
        inputs.settings.stack_script_enabled = false;
        inputs.settings.sort_order = crate::settings::SortOrder::FileName;
        let before = scan(temp.path(), &inputs);
        let proof = &before.prepared.as_ref().unwrap().proof;
        assert!(
            proof
                .rows
                .iter()
                .any(|row| matches!(row.item, GridItem::Stack { count: 2, .. }))
        );
        assert!(proof.stack_members.iter().any(|(_, members)| members
            == &vec![
                temp.path().join("post_p0.jpg"),
                temp.path().join("post_p1.jpg")
            ]));
        let mut listing = baseline(temp.path(), &before);
        let deleted = temp.path().join("post_p0.jpg");
        listing.adopt_delete(receipt(&listing, 1, &[deleted.clone()]));
        std::fs::remove_file(&deleted).unwrap();
        let after = scan(temp.path(), &inputs);
        assert!(listing.raw.same_listing(&after.raw));
        assert!(!listing.can_retain(
            &after,
            &after.prepared.as_ref().unwrap().proof,
            &Default::default()
        ));
    }

    #[test]
    fn content_on_removed_exact_source_can_be_consumed_but_unknown_cannot() {
        let temp = fixture(&["a.jpg", "b.jpg"]);
        let inputs = inputs();
        let before = scan(temp.path(), &inputs);
        let mut listing = baseline(temp.path(), &before);
        let deleted = temp.path().join("a.jpg");
        listing.adopt_delete(receipt(&listing, 1, &[deleted.clone()]));
        std::fs::remove_file(&deleted).unwrap();
        let actual = scan(temp.path(), &inputs);
        let proof = &actual.prepared.as_ref().unwrap().proof;
        let mut dirty = WatchDirtySummary::default();
        dirty.record(WatchChangeKind::Content, [deleted]);
        assert!(listing.can_retain(&actual, proof, &dirty.snapshot()));
        dirty.record(
            WatchChangeKind::AnyModify,
            [temp.path().join("untyped-child")],
        );
        assert!(!listing.can_retain(&actual, proof, &dirty.snapshot()));
        dirty.record(WatchChangeKind::Unknown, []);
        assert!(!listing.can_retain(&actual, proof, &dirty.snapshot()));
    }

    #[test]
    fn automatic_representative_is_unproven_without_delayed_thumbnail_adoption_evidence() {
        let temp = fixture(&["a.jpg", "b.jpg"]);
        let child = temp.path().join("child");
        std::fs::create_dir(&child).unwrap();
        let winner = child.join("page.jpg");
        std::fs::write(&winner, b"fixture").unwrap();
        let inputs = inputs();
        let before = scan(temp.path(), &inputs);
        let listing = baseline(temp.path(), &before);
        assert!(!listing.proof().complete);
        assert!(
            listing
                .proof()
                .dependencies
                .iter()
                .all(|dependency| dependency.path != winner)
        );
        assert!(matches!(
            listing.projection,
            FolderProjectionState::NeedsProjection { .. }
        ));
        let partial = child.join("download.part");
        std::fs::write(&partial, b"in progress").unwrap();
        let mut actual =
            folder_scan::scan_directory_with_settings(temp.path(), &inputs.settings).unwrap();
        prepare_reconcile_scan_inputs(
            temp.path(),
            &mut actual,
            &inputs,
            &Arc::new(AtomicBool::new(false)),
            &listing,
        )
        .unwrap();
        let mut dirty = WatchDirtySummary::default();
        dirty.record(WatchChangeKind::AnyModify, [child]);
        let proof = actual.prepared.as_ref().unwrap().proof.clone();
        assert!(listing.raw.same_listing(&actual.raw));
        assert!(!proof.complete);
        assert!(!dirty.snapshot().forces_install(
            &listing.raw,
            &actual.raw,
            &proof,
            None,
            temp.path()
        ));
        assert!(
            !listing.can_retain(&actual, &proof, &dirty.snapshot()),
            "later eager selection must not be used as proof of what an earlier/delayed thumbnail displays"
        );
    }

    #[test]
    fn unchanged_display_rows_cannot_hide_changed_omitted_projection_counts() {
        let temp = fixture(&["a.jpg", "b.jpg"]);
        let inputs = inputs();
        let before = scan(temp.path(), &inputs);
        let listing = baseline(temp.path(), &before);
        std::fs::write(temp.path().join("notes.txt"), b"not a displayed image").unwrap();
        let actual = scan(temp.path(), &inputs);
        assert_eq!(
            before.prepared.as_ref().unwrap().projected_items,
            actual.prepared.as_ref().unwrap().projected_items
        );
        assert_eq!(
            folder_scan::signature_from_scan(&before),
            folder_scan::signature_from_scan(&actual)
        );
        assert_eq!(
            actual.raw.omitted.unsupported,
            before.raw.omitted.unsupported + 1
        );
        assert!(!listing.raw.same_listing(&actual.raw));
        assert!(!listing.can_retain(
            &actual,
            &actual.prepared.as_ref().unwrap().proof,
            &Default::default()
        ));

        let mut hidden = before.clone();
        Arc::make_mut(&mut hidden.raw).omitted.hidden += 1;
        hidden.prepared.as_mut().unwrap().proof.omitted.hidden += 1;
        assert!(!listing.raw.same_listing(&hidden.raw));
        assert!(!listing.can_retain(
            &hidden,
            &hidden.prepared.as_ref().unwrap().proof,
            &Default::default()
        ));

        let mut projection = before.prepared.as_ref().unwrap().proof.clone();
        projection.omitted.same_name += 1;
        assert!(listing.raw.same_listing(&before.raw));
        assert!(
            !listing.can_retain(&before, &projection, &Default::default()),
            "a post-filter count change must also be checked at the projection boundary"
        );
    }
}
