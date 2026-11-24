//! Whole-listing requirements and completion belong to the metadata session.
use super::*;
use crate::settings::{
    FacetDurationPreset, LazyMetric, ListingDuration, ListingPageCount, ListingSortMetadata,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ListingMetaScanProof {
    pub(crate) generation: u64,
    pub(crate) items_len: usize,
    pub(crate) requirements: ListingMetaRequirements,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct ListingMetaRequirements {
    pub(crate) duration: bool,
    pub(crate) ai: bool,
    pub(crate) page_count: bool,
}

impl ListingMetaRequirements {
    pub(crate) fn is_empty(self) -> bool {
        !self.duration && !self.ai && !self.page_count
    }

    pub(crate) fn contains(self, other: Self) -> bool {
        (!other.duration || self.duration)
            && (!other.ai || self.ai)
            && (!other.page_count || self.page_count)
    }

    pub(crate) fn union(self, other: Self) -> Self {
        Self {
            duration: self.duration || other.duration,
            ai: self.ai || other.ai,
            page_count: self.page_count || other.page_count,
        }
    }

    pub(crate) fn without(self, proven: Self) -> Self {
        Self {
            duration: self.duration && !proven.duration,
            ai: self.ai && !proven.ai,
            page_count: self.page_count && !proven.page_count,
        }
    }
}

impl ListingMetaScanProof {
    pub(crate) fn matches(self, app: &App) -> bool {
        self.generation == app.items_generation && self.items_len == app.items.len()
    }
}

impl App {
    pub(super) fn listing_may_sort_by_duration(&self) -> bool {
        self.settings
            .listing_may_sort_by_duration(self.favorite_view_states.values())
    }

    pub(super) fn listing_may_sort_by_page_count(&self) -> bool {
        self.settings
            .listing_may_sort_by_page_count(self.favorite_view_states.values())
    }

    /// Capture the recognition identity with the request, never on a scan worker.
    pub(super) fn listing_page_count_facts_fingerprint(&self) -> Option<i64> {
        self.listing_may_sort_by_page_count()
            .then(|| folder_scan::image_folder_page_count_options(&self.settings).fingerprint)
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
                && self.listing_sort_capabilities().lazy_metric_allowed)
    }

    pub(crate) fn page_counts_wanted(&self) -> bool {
        self.settings.sort_order.lazy_metric() == Some(LazyMetric::PageCount)
            && self.listing_sort_capabilities().lazy_metric_allowed
    }

    pub(super) fn whole_listing_meta_scan_wanted(&self) -> bool {
        self.ai_model_facet_should_load()
            || self.duration_lengths_wanted()
            || self.page_counts_wanted()
    }

    pub(crate) fn duration_scan_ready(&self) -> bool {
        self.proven_listing_meta_requirements().duration
    }

    pub(crate) fn page_count_scan_ready(&self) -> bool {
        self.proven_listing_meta_requirements().page_count
    }

    pub(crate) fn listing_meta_requirements(&self) -> ListingMetaRequirements {
        ListingMetaRequirements {
            duration: self.duration_lengths_wanted(),
            ai: self.ai_model_facet_should_load(),
            page_count: self.page_counts_wanted(),
        }
    }

    // Target construction must finish the fields promised by the active scan,
    // even if the current filter stops requesting one of them mid-plan.
    pub(super) fn active_listing_meta_requirements(&self) -> ListingMetaRequirements {
        self.details_meta_pending
            .as_ref()
            .and_then(|pending| pending.listing_meta_scan)
            .filter(|scan| scan.matches(self))
            .map_or_else(
                || self.listing_meta_requirements(),
                |scan| scan.requirements.union(self.listing_meta_requirements()),
            )
    }

    fn proven_listing_meta_requirements(&self) -> ListingMetaRequirements {
        self.listing_meta_scan_proof
            .filter(|proof| proof.matches(self))
            .map_or_else(ListingMetaRequirements::default, |proof| proof.requirements)
    }

    pub(crate) fn unproven_listing_meta_requirements(&self) -> ListingMetaRequirements {
        self.listing_meta_requirements()
            .without(self.proven_listing_meta_requirements())
    }

    pub(super) fn listing_meta_scan_snapshot(&self) -> ListingMetaScanProof {
        ListingMetaScanProof {
            generation: self.items_generation,
            items_len: self.items.len(),
            requirements: self.listing_meta_requirements(),
        }
    }

    pub(super) fn complete_listing_meta_scan(
        &mut self,
        scan: Option<ListingMetaScanProof>,
    ) -> ListingMetaRequirements {
        let Some(scan) = scan.filter(|scan| scan.matches(self)) else {
            return ListingMetaRequirements::default();
        };
        let previous = self.proven_listing_meta_requirements();
        self.listing_meta_scan_proof = Some(ListingMetaScanProof {
            requirements: previous.union(scan.requirements),
            ..scan
        });
        crate::perf::event(
            "details_meta",
            "listing_meta_scan_complete",
            None,
            scan.generation,
            &[
                ("items", serde_json::Value::from(scan.items_len)),
                (
                    "duration",
                    serde_json::Value::from(scan.requirements.duration),
                ),
                ("ai", serde_json::Value::from(scan.requirements.ai)),
                (
                    "page_count",
                    serde_json::Value::from(scan.requirements.page_count),
                ),
            ],
        );
        scan.requirements.without(previous)
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
        self.lazy_metric_loading_text(LazyMetric::Duration)
    }

    pub(crate) fn lazy_metric_loading_text(&self, metric: LazyMetric) -> Option<String> {
        let (wanted, ready, label) = match metric {
            LazyMetric::Duration => (
                self.duration_lengths_wanted(),
                self.duration_scan_ready(),
                "長さ",
            ),
            LazyMetric::PageCount => (
                self.page_counts_wanted(),
                self.page_count_scan_ready(),
                "ページ数",
            ),
        };
        if !wanted || ready {
            return None;
        }
        match self.details_image_dims_state {
            LazyColumnState::Loading { done, total }
                if self.details_meta_pending.as_ref().is_some_and(|p| {
                    p.listing_meta_scan.is_some_and(|scan| match metric {
                        LazyMetric::Duration => scan.requirements.duration,
                        LazyMetric::PageCount => scan.requirements.page_count,
                    })
                }) =>
            {
                Some(format!("{label}を読み込み中 {done}/{total}"))
            }
            LazyColumnState::Cancelled => Some(format!("{label}の読み込みを停止中")),
            _ => Some(format!("{label}を読み込み中 0/0")),
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

    /// Sorting a new listing uses the request's identity, unlike the last-value display accessor.
    /// EPUB generation only triggers refetching; it does not invalidate a matching memory value.
    pub(super) fn listing_page_count_for_item(
        &self,
        item: &GridItem,
        stamp: (i64, i64),
        fingerprint: i64,
    ) -> Option<ListingPageCount> {
        let path = folder_scan::listing_page_count_path(item)?;
        let key = crate::adjustment_db::normalize_path(path);
        let meta = self.details_lazy_meta.get(&key)?;
        if !meta.matches_source(Some(stamp))
            || !meta.page_count_checked
            || meta.page_count_fingerprint != fingerprint
        {
            return None;
        }
        if matches!(item, GridItem::PdfFile(_))
            && meta.page_count_pdf_password_revision
                != Some(self.pdf_passwords.credential_revision(path))
        {
            return None;
        }
        Some(
            match meta
                .page_count
                .filter(|&count| count > 0 && !meta.page_count_failed)
            {
                Some(count) => ListingPageCount::Known(count),
                None => ListingPageCount::Unavailable,
            },
        )
    }

    pub(super) fn listing_metadata_snapshot(
        &self,
        fingerprint: i64,
        metric: LazyMetric,
    ) -> Arc<folder_scan::FolderListingMetadataSnapshot> {
        let started = crate::perf::is_enabled().then(std::time::Instant::now);
        let mut values = folder_scan::FolderListingMetadataSnapshot::default();
        for (idx, item) in self.items.iter().enumerate() {
            let Some((mtime, size)) = self.image_metas.get(idx).copied().flatten() else {
                continue;
            };
            if metric == LazyMetric::Duration
                && let GridItem::Video(path) | GridItem::Audio(path) = item
            {
                if let Some(value) = self
                    .listing_duration_for_path(path, ListingSortMetadata::new(mtime, Some(size)))
                {
                    values.duration.insert(
                        (crate::adjustment_db::normalize_path(path), mtime, size),
                        value,
                    );
                }
            } else if metric == LazyMetric::PageCount
                && self.details_item_page_count_needs_fetch(idx)
            {
                let path = match item {
                    GridItem::Folder(path)
                    | GridItem::ZipFile(path)
                    | GridItem::PdfFile(path)
                    | GridItem::ConvertibleArchive { path, .. } => path,
                    _ => continue,
                };
                if let Some(value) =
                    self.listing_page_count_for_item(item, (mtime, size), fingerprint)
                {
                    values.page_count.insert(
                        (crate::adjustment_db::normalize_path(path), mtime, size),
                        value,
                    );
                }
            }
        }
        if let Some(started) = started {
            crate::perf::event(
                "listing",
                "lazy_metric_memory_snapshot",
                None,
                self.items_generation,
                &[
                    ("duration", serde_json::Value::from(values.duration.len())),
                    (
                        "page_count",
                        serde_json::Value::from(values.page_count.len()),
                    ),
                    (
                        "ms",
                        serde_json::Value::from(started.elapsed().as_secs_f64() * 1000.0),
                    ),
                ],
            );
        }
        Arc::new(values)
    }

    #[cfg(test)]
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
