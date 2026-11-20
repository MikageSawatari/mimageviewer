//! Length requirements and completion belong to the listing's metadata session.
use super::*;
use crate::settings::{FacetDurationPreset, ListingDuration, ListingSortMetadata};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct DurationScanProof {
    pub(crate) generation: u64,
    pub(crate) items_len: usize,
}

impl DurationScanProof {
    fn matches(self, app: &App) -> bool {
        self.generation == app.items_generation && self.items_len == app.items.len()
    }
}

impl App {
    pub(super) fn listing_may_sort_by_duration(&self) -> bool {
        self.settings
            .listing_may_sort_by_duration(self.favorite_view_states.values())
    }

    // Match the destination owner before adoption, without publishing a surface change.
    pub(super) fn physical_duration_sort_surface(
        &self,
        path: &Path,
        owner: Option<&OpenRequestOwner>,
        navigation: Option<&MainListNavigation>,
    ) -> top_level_grid_view::TopLevelGridSurface {
        use top_level_grid_view::TopLevelGridSurface;
        match owner {
            Some(OpenRequestOwner::QuickFolderSwitch(_) | OpenRequestOwner::RatingPhysical(_)) => {
                TopLevelGridSurface::Folder
            }
            Some(OpenRequestOwner::MainGridArchive(archive))
                if archive.rating_grid_owner.is_some() =>
            {
                TopLevelGridSurface::Folder
            }
            Some(OpenRequestOwner::CollectionGridPhysical(collection)) => {
                TopLevelGridSurface::Collection(top_level_grid_view::CollectionGridIdentity {
                    collection_id: collection.stamp.collection_id,
                })
            }
            Some(OpenRequestOwner::MainGridArchive(archive))
                if archive.collection_grid_owner.is_some() =>
            {
                TopLevelGridSurface::Collection(top_level_grid_view::CollectionGridIdentity {
                    collection_id: archive
                        .collection_grid_owner
                        .as_ref()
                        .unwrap()
                        .physical
                        .stamp
                        .collection_id,
                })
            }
            // A pinned snapshot cannot be dismissed by an explicit jump. Its scope
            // admission remains authoritative even when a caller carries that intent.
            _ if self.is_snapshot_active() => self.top_level_grid_view.surface().clone(),
            // Prepare precedes adoption: use the same intent that dismisses the
            // outgoing search/Smart/Collection return owner on successful adoption.
            _ if navigation.is_some_and(MainListNavigation::exits_temporary_view) => {
                TopLevelGridSurface::Folder
            }
            _ if self.smart_folder_session_owns_load(path)
                || self.global_search.active
                || self.favsearch.active
                || self.tag_view.active =>
            {
                self.top_level_grid_view.surface().clone()
            }
            None if matches!(
                self.top_level_grid_view.surface(),
                TopLevelGridSurface::Collection(_)
            ) =>
            {
                self.top_level_grid_view.surface().clone()
            }
            _ => TopLevelGridSurface::Folder,
        }
    }

    pub(super) fn physical_duration_sort_order(
        &self,
        sort: crate::settings::SortOrder,
        surface: &top_level_grid_view::TopLevelGridSurface,
        image_book: bool,
    ) -> crate::settings::SortOrder {
        listing_sort::listing_sort_capabilities(listing_sort::ListingSortContext {
            surface,
            recipe: &book_materialization::ProjectionRecipe::OrdinaryList,
            page_order_locked: image_book,
            details_sort_active: false,
            collection_root: false,
            existing_lock: None,
        })
        .effective_order(sort)
    }

    pub(crate) fn duration_lengths_wanted(&self) -> bool {
        self.duration_facet_requested
            || self.settings.facet_filter.duration_preset.is_some()
            || (self.settings.sort_order.is_duration()
                && self.listing_sort_capabilities().duration_allowed)
    }

    pub(super) fn whole_listing_meta_scan_wanted(&self) -> bool {
        self.ai_model_facet_should_load() || self.duration_lengths_wanted()
    }

    pub(crate) fn duration_scan_ready(&self) -> bool {
        self.duration_scan_proof
            .is_some_and(|proof| proof.matches(self))
    }

    pub(super) fn duration_scan_snapshot(&self) -> DurationScanProof {
        DurationScanProof {
            generation: self.items_generation,
            items_len: self.items.len(),
        }
    }

    pub(super) fn complete_duration_scan(&mut self, scan: Option<DurationScanProof>) -> bool {
        let Some(scan) = scan.filter(|scan| scan.matches(self)) else {
            return false;
        };
        let changed = self.duration_scan_proof != Some(scan);
        self.duration_scan_proof = Some(scan);
        crate::perf::event(
            "details_meta",
            "duration_scan_complete",
            None,
            scan.generation,
            &[("items", serde_json::Value::from(scan.items_len))],
        );
        changed
    }

    pub(crate) fn request_duration_facet_load(&mut self) {
        if !self.duration_facet_requested {
            self.duration_facet_requested = true;
            if !self.duration_scan_ready() {
                self.invalidate_details_meta_requirements();
            }
        }
    }

    pub(crate) fn duration_loading_text(&self) -> Option<String> {
        if !self.duration_lengths_wanted() || self.duration_scan_ready() {
            return None;
        }
        match self.details_image_dims_state {
            LazyColumnState::Loading { done, total }
                if self
                    .details_meta_pending
                    .as_ref()
                    .is_some_and(|p| p.duration_scan.is_some()) =>
            {
                Some(format!("長さを読み込み中 {done}/{total}"))
            }
            LazyColumnState::Cancelled => Some("長さの読み込みを停止中".to_string()),
            _ => Some("長さを読み込み中 0/0".to_string()),
        }
    }

    fn media_duration_from_meta(meta: &DetailsLazyMeta) -> ListingDuration {
        match &meta.media {
            DetailsMediaMeta::Read(probe) => ListingDuration::from_seconds(probe.duration_secs),
            DetailsMediaMeta::Unreadable => ListingDuration::Unavailable,
            DetailsMediaMeta::NotFetched | DetailsMediaMeta::RetryLater { .. } => {
                ListingDuration::Missing
            }
        }
    }

    pub(super) fn listing_duration_for_path(
        &self,
        path: &Path,
        stamp: ListingSortMetadata,
    ) -> Option<ListingDuration> {
        let size = stamp.file_size?;
        let key = crate::adjustment_db::normalize_path(path);
        let meta = self.details_lazy_meta.get(&key)?;
        meta.matches_source(Some((stamp.mtime, size)))
            .then(|| Self::media_duration_from_meta(meta))
            .filter(|value| *value != ListingDuration::Missing)
    }

    pub(super) fn listing_duration_snapshot(&self) -> Arc<folder_scan::FolderDurationSnapshot> {
        let started = crate::perf::is_enabled().then(std::time::Instant::now);
        let values = self
            .items
            .iter()
            .enumerate()
            .filter_map(|(idx, item)| {
                let (GridItem::Video(path) | GridItem::Audio(path)) = item else {
                    return None;
                };
                let (mtime, size) = self.image_metas.get(idx).copied().flatten()?;
                let value = self
                    .listing_duration_for_path(path, ListingSortMetadata::new(mtime, Some(size)))?;
                Some((
                    (crate::adjustment_db::normalize_path(path), mtime, size),
                    value,
                ))
            })
            .collect::<folder_scan::FolderDurationSnapshot>();
        if let Some(started) = started {
            crate::perf::event(
                "listing",
                "duration_memory_snapshot",
                None,
                self.items_generation,
                &[
                    ("items", serde_json::Value::from(values.len())),
                    (
                        "ms",
                        serde_json::Value::from(started.elapsed().as_secs_f64() * 1000.0),
                    ),
                ],
            );
        }
        Arc::new(values)
    }

    pub(super) fn duration_matches_preset(&self, idx: usize, preset: FacetDurationPreset) -> bool {
        matches!(
            self.items.get(idx),
            Some(GridItem::Video(_) | GridItem::Audio(_))
        ) && self
            .details_lazy_meta_for_idx(idx)
            .and_then(|meta| meta.media.read())
            .and_then(|probe| probe.duration_secs)
            .is_some_and(|seconds| preset.matches_seconds(seconds))
    }

    pub(crate) fn facet_duration_count(&mut self, preset: FacetDurationPreset) -> usize {
        self.facet_candidate_indices(FacetField::Duration)
            .into_iter()
            .filter(|&idx| self.duration_matches_preset(idx, preset))
            .count()
    }
}
