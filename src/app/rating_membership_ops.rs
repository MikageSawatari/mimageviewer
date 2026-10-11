//! One destination decision at the context-owned set adoption boundary (§1.371).
use super::*;
use crate::rating_membership::{
    RatingAdoptionIntent, RatingNeighborDecision, RatingOrigin, RatingOriginTarget,
    RatingPreviewRecipe, reconcile_rating_neighbor_with_basis,
};

pub(crate) struct RatingMembershipSnapshot {
    owner: u64,
    listing: String,
    grid: Vec<String>,
    reader: Vec<String>,
    selected: Option<String>,
    shift_anchor: Option<String>,
    page: Option<String>,
    page_origin: RatingPageOrigin,
    media: bool,
    generation: u64,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum RatingPageOrigin {
    Painted,
    Deferred,
}

impl App {
    /// Rating list replacement uses the released editor lifecycle, rather than retaining
    /// or remapping index-owned edits. Only stable-key rating history crosses the boundary.
    pub(super) fn finish_rating_membership_edit_boundary(&mut self) {
        self.meta_undo.retain_rating();
        // Finishing a local gesture may push its discarded Undo and clear Redo. Move only
        // rating history across the existing exits, including its rating Redo entries.
        let rating_history = std::mem::take(&mut self.meta_undo);
        self.fold_local_adjust_edit_state();
        self.clear_meta_undo_for_rating_move();
        self.finish_fullscreen_edit_modes();
        self.adjustment_mode = crate::ui_helpers::MetadataPanelOpenState::Closed;
        self.adjustment_dragging = false;
        self.meta_undo = rating_history;
    }

    pub(super) fn log_successful_rating_save(
        &self,
        source: &str,
        key: &str,
        before: Option<u8>,
        after: u8,
        target: Option<usize>,
        reader_member: bool,
    ) {
        crate::logger::log(format!(
            "[RATING] save success source={source} context={} write_generation={} items_generation={} target={target:?} key={key:?} before={} after={after} settings_filter={:?} effective_filter={:?} reader_member={reader_member} new_matches={} projection={}",
            self.projected_viewer_context_id().serial(),
            self.rating_session_write_generation,
            self.items_generation,
            before
                .map(|stars| stars.to_string())
                .unwrap_or_else(|| "unknown(cache/session unavailable)".into()),
            self.settings.rating_filter,
            self.effective_rating_filter(),
            self.effective_rating_filter()[after.min(5) as usize],
            if self.ring_picker.is_some() {
                "picker-held"
            } else {
                "preparing"
            }
        ));
    }
    pub(crate) fn prepare_rating_membership_items_swap(
        &mut self,
        items: &[GridItem],
    ) -> Option<snapshot_ops::SnapshotViewerIndexSwap> {
        let old_key = self
            .fullscreen_idx
            .and_then(|index| self.items.get(index))
            .and_then(crate::snapshot::snapshot_key_from_grid_item);
        if old_key.is_some_and(|key| {
            !items.iter().any(|item| {
                crate::snapshot::snapshot_key_from_grid_item(item).as_ref() == Some(&key)
            })
        }) {
            // Teardown uses the outgoing index space. Only the accepted set chooses the destination.
            self.close_fullscreen_for_rating_membership();
        }
        self.prepare_rating_membership_viewer_swap(items)
    }
    /// A same-list worker adoption uses the common items generation and exact viewer transfer
    /// owners, rather than a source-open (which clears the reader and navigation history).
    pub(super) fn apply_rating_membership_view_result(
        &mut self,
        result: crate::rating_view::RatingViewBuildResult,
    ) {
        if !self.items_are_rating_view {
            self.apply_rating_view_result(result);
            return;
        }
        let Some(prepared) = result.prepared else {
            return;
        };
        let viewer = self.prepare_rating_membership_items_swap(&prepared.items);
        let thumbnails = self
            .items
            .iter()
            .zip(self.thumbnails.iter())
            .map(|(item, thumbnail)| (item.perf_key(), thumbnail.clone()))
            .collect::<HashMap<_, _>>();
        let header = self
            .details_order
            .iter()
            .filter_map(|&index| self.items.get(index).map(GridItem::perf_key))
            .collect::<Vec<_>>();
        self.rating_view_rows = result.rows;
        self.rating_view_skipped = result.skipped;
        // Persist/clear index-owned edits while indices still belong to the outgoing pages.
        self.clear_page_edit_state();
        self.install_new_items(prepared.items, prepared.image_metas);
        self.invalidate_idx_state_and_queues();
        self.rating_cache = prepared.rating_cache;
        self.replace_tags_cache(prepared.tags_cache);
        self.video_thumb_overrides = prepared.video_thumb_overrides;
        self.folder_pin_map = prepared.pin_map;
        self.items_are_rating_view = true;
        self.rating_view_rows_stars = Some(self.rating_view_stars);
        self.install_prepared_page_edits(prepared.page_edits);
        self.prepare_current_listing_page_identity_index(
            self.settings.file_type_visibility.clone(),
            None,
        );
        for (index, item) in self.items.iter().enumerate() {
            if let Some(thumbnail) = thumbnails.get(&item.perf_key()) {
                self.thumbnails[index] = thumbnail.clone();
            }
        }
        let positions = self
            .items
            .iter()
            .enumerate()
            .map(|(index, item)| (item.perf_key(), index))
            .collect::<HashMap<_, _>>();
        self.details_order = header
            .iter()
            .filter_map(|key| positions.get(key).copied())
            .collect();
        self.finish_rating_membership_viewer_swap(viewer);
    }
    fn rating_listing_identity(&self) -> String {
        let surface = match self.top_level_grid_view.surface() {
            top_level_grid_view::TopLevelGridSurface::SmartFolder(state) => {
                let position = match &state.position {
                    top_level_grid_view::SmartFolderPosition::Root => "root".to_owned(),
                    top_level_grid_view::SmartFolderPosition::Container {
                        root_entry,
                        current,
                    } => format!("container:{root_entry:?}:{current:?}"),
                    top_level_grid_view::SmartFolderPosition::Scoped {
                        entry_root,
                        current,
                        current_kind,
                        ..
                    } => format!("scoped:{entry_root:?}:{current:?}:{current_kind:?}"),
                };
                format!("smart:{}:{position}", state.definition_id)
            }
            other => format!("{other:?}"),
        };
        format!(
            "{:?}|{:?}|{}|{}",
            self.navigation_scope,
            self.current_folder,
            surface,
            if self.items_are_global_search_view {
                self.global_search.last_executed.as_str()
            } else {
                ""
            }
        )
    }

    fn rating_membership_key(&self, index: usize) -> Option<String> {
        self.page_path_key(index)
            .or_else(|| self.rating_path_key(index))
            .or_else(|| self.items.get(index).map(GridItem::perf_key))
    }

    fn rating_membership_keys(&self, order: &[usize]) -> Vec<String> {
        order
            .iter()
            .filter_map(|&idx| self.rating_membership_key(idx))
            .collect()
    }

    fn rating_membership_full_order(&mut self, media: Option<bool>) -> Vec<String> {
        let mut indices = (0..self.items.len()).collect::<Vec<_>>();
        if self.details_header_sort_active()
            && !(self.settings.details_sort_key == crate::settings::DetailsSortKey::PageCount
                && !self.details_image_dims_state.is_ready())
        {
            let key = self.settings.details_sort_key;
            let mut rows = indices
                .into_iter()
                .map(|idx| {
                    if key == crate::settings::DetailsSortKey::Rating {
                        // The full basis must not add lazy DB reads for hidden items.
                        let stars = self
                            .rating_path_key(idx)
                            .and_then(|key| {
                                self.rating_session_writes
                                    .get(&key)
                                    .map(|write| write.stars)
                            })
                            .or_else(|| self.rating_cache.get(&idx).copied());
                        DetailsSortRow {
                            idx,
                            primary: DetailsSortPrimary::U8(stars.unwrap_or(0)),
                            name_key: crate::filename_sort::SortNameKey::file_name(
                                &self.items[idx].name(),
                            ),
                        }
                    } else if key == crate::settings::DetailsSortKey::State {
                        let mut bits = self.details_state_bits(idx);
                        if self
                            .rotation_cache
                            .get(&idx)
                            .is_some_and(|rotation| !rotation.is_none())
                        {
                            bits |= 1 << 5;
                        }
                        DetailsSortRow {
                            idx,
                            primary: DetailsSortPrimary::U32(bits),
                            name_key: crate::filename_sort::SortNameKey::file_name(
                                &self.items[idx].name(),
                            ),
                        }
                    } else {
                        self.details_sort_row(idx, key)
                    }
                })
                .collect::<Vec<_>>();
            rows.sort_by(|a, b| {
                Self::compare_details_sort_rows(a, b, key, self.settings.details_sort_ascending)
            });
            indices = rows.into_iter().map(|row| row.idx).collect();
        }
        if let Some(media) = media {
            indices.retain(|&idx| {
                self.items[idx].has_page_data()
                    || (media && matches!(self.items[idx], GridItem::Video(_) | GridItem::Audio(_)))
            });
        }
        self.rating_membership_keys(&indices)
    }

    fn decide_rating_neighbor(
        &mut self,
        old: &[String],
        anchor: Option<&str>,
        current: &[String],
        media: Option<bool>,
    ) -> RatingNeighborDecision {
        let needs_basis = anchor.is_some_and(|anchor| !current.iter().any(|key| key == anchor))
            && !old.iter().any(|key| current.contains(key));
        let basis = if needs_basis {
            self.rating_membership_full_order(media)
        } else {
            Vec::new()
        };
        reconcile_rating_neighbor_with_basis(old, anchor, current, &basis)
    }

    pub(crate) fn capture_rating_origin(&self) -> Option<RatingOrigin> {
        Some(RatingOrigin {
            owner: self.projected_viewer_context_id().serial(),
            listing_identity: self.rating_listing_identity(),
            target: self
                .sidecar_restore_rating_deferred_target()
                .or(self.fullscreen_idx)
                .and_then(|idx| self.rating_membership_key(idx))
                .map(|key| RatingOriginTarget::Viewer {
                    key,
                    presentation: self.viewer_presentation,
                })
                .or_else(|| {
                    self.selected
                        .and_then(|idx| self.rating_membership_key(idx))
                        .map(RatingOriginTarget::Grid)
                })
                .unwrap_or(RatingOriginTarget::ValueOnly),
        })
    }

    pub(crate) fn capture_rating_membership(&self) -> RatingMembershipSnapshot {
        let deferred = self.sidecar_restore_rating_deferred_target();
        let endpoint = deferred.or(self.fullscreen_idx);
        let media = endpoint.is_some_and(|idx| self.viewer_item_is_media(idx));
        let reader = self
            .current_reader_order()
            .iter()
            .copied()
            .filter(|&idx| {
                self.items.get(idx).is_some_and(|item| {
                    if media {
                        item.has_page_data()
                            || matches!(item, GridItem::Video(_) | GridItem::Audio(_))
                    } else {
                        item.has_page_data()
                    }
                })
            })
            .collect::<Vec<_>>();
        RatingMembershipSnapshot {
            owner: self.projected_viewer_context_id().serial(),
            listing: self.rating_listing_identity(),
            grid: self.rating_membership_keys(self.current_grid_order()),
            reader: self.rating_membership_keys(&reader),
            selected: self
                .selected
                .and_then(|idx| self.rating_membership_key(idx)),
            shift_anchor: self
                .grid_click_selection_anchor
                .and_then(|anchor| anchor.index_for_generation(self.items_generation))
                .and_then(|idx| self.rating_membership_key(idx)),
            page: endpoint.and_then(|idx| self.rating_membership_key(idx)),
            page_origin: if deferred.is_some() {
                RatingPageOrigin::Deferred
            } else {
                RatingPageOrigin::Painted
            },
            media,
            generation: self.items_generation,
        }
    }

    /// Explicit predicate/settings adoption places previously hidden entrants in the
    /// same canonical/Details order, rather than skipping ahead to old visible survivors.
    pub(crate) fn capture_rating_membership_for_filter_change(
        &mut self,
    ) -> RatingMembershipSnapshot {
        let mut old = self.capture_rating_membership();
        old.grid = self.rating_membership_full_order(None);
        old.reader = self.rating_membership_full_order(Some(old.media));
        old
    }

    pub(crate) fn rating_preview_recipe(&self) -> RatingPreviewRecipe {
        self.ring_picker
            .as_ref()
            .filter(|picker| picker.owner == self.projected_viewer_context_id())
            .map(|picker| RatingPreviewRecipe {
                entries: picker
                    .preview_recipe
                    .entries
                    .iter()
                    .filter(|entry| {
                        self.rating_session_writes
                            .get(&entry.key)
                            .is_some_and(|write| write.generation == entry.write_generation)
                    })
                    .cloned()
                    .collect(),
            })
            .unwrap_or_default()
    }

    /// Apply the set owner's destination through §1.370's cursor owner,
    /// retaining this caller's scroll policy.
    /// Destination ownership stays here; a Move always rebases Shift, even at the same raw index.
    fn apply_rating_membership_cursor(&mut self, target: Option<usize>) {
        self.move_grid_cursor(target, crate::app::GridCursorIntent::Automatic);
        self.scroll_to_selected = target.is_some();
    }

    pub(crate) fn adopt_rating_membership(
        &mut self,
        old: RatingMembershipSnapshot,
        intent: RatingAdoptionIntent,
    ) {
        if old.owner != self.projected_viewer_context_id().serial()
            || old.listing != self.rating_listing_identity()
        {
            return;
        }
        let positions = (0..self.items.len())
            .filter_map(|idx| self.rating_membership_key(idx).map(|key| (key, idx)))
            .collect::<HashMap<_, _>>();
        let grid = self.rating_membership_keys(self.current_grid_order());
        let media = old.media
            || matches!(&intent, RatingAdoptionIntent::RestoreOrigin(origin) if origin.viewer_key().and_then(|key| positions.get(key)).is_some_and(|&idx| self.viewer_item_is_media(idx)));
        let reader_indices = self
            .current_reader_order()
            .iter()
            .copied()
            .filter(|&idx| {
                self.items.get(idx).is_some_and(|item| {
                    if media {
                        item.has_page_data()
                            || matches!(item, GridItem::Video(_) | GridItem::Audio(_))
                    } else {
                        item.has_page_data()
                    }
                })
            })
            .collect::<Vec<_>>();
        let reader = self.rating_membership_keys(&reader_indices);
        let restore = match &intent {
            RatingAdoptionIntent::RestoreOrigin(origin)
                if origin.owner == old.owner && origin.listing_identity == old.listing =>
            {
                origin.viewer_key().filter(|key| reader.contains(key))
            }
            _ => None,
        };
        let page = if let Some(key) = restore {
            Some(if old.page.as_ref() == Some(key) {
                RatingNeighborDecision::Keep(key.clone())
            } else {
                RatingNeighborDecision::Move(key.clone())
            })
        } else if old.page.is_some() && !matches!(intent, RatingAdoptionIntent::Manual) {
            Some(self.decide_rating_neighbor(
                &old.reader,
                old.page.as_deref(),
                &reader,
                Some(media),
            ))
        } else {
            None
        };
        let mut cursor = match &intent {
            RatingAdoptionIntent::RestoreOrigin(origin)
                if origin.owner == old.owner
                    && origin.listing_identity == old.listing
                    && old.page.is_none() =>
            {
                match &origin.target {
                    RatingOriginTarget::Grid(key)
                        if grid.contains(key) && old.selected.as_ref() != Some(key) =>
                    {
                        RatingNeighborDecision::Move(key.clone())
                    }
                    _ => {
                        self.decide_rating_neighbor(&old.grid, old.selected.as_deref(), &grid, None)
                    }
                }
            }
            _ => self.decide_rating_neighbor(&old.grid, old.selected.as_deref(), &grid, None),
        };
        match &page {
            Some(RatingNeighborDecision::Move(key)) => {
                let idx = positions[key];
                if let RatingAdoptionIntent::RestoreOrigin(RatingOrigin {
                    target: RatingOriginTarget::Viewer { presentation, .. },
                    ..
                }) = &intent
                {
                    self.viewer_presentation = *presentation;
                }
                self.open_fullscreen_for_rating_membership(idx);
                cursor = RatingNeighborDecision::Move(key.clone());
            }
            Some(RatingNeighborDecision::Keep(key)) => {
                let target = positions[key];
                if old.page_origin == RatingPageOrigin::Deferred {
                    // A replacement generation may have a new preparation owner. Rebind the
                    // already chosen endpoint there; never expose an unmaterialized index.
                    if self.sidecar_restore_rating_deferred_target() != Some(target) {
                        self.open_fullscreen_for_rating_membership(target);
                    }
                } else {
                    self.fullscreen_idx = Some(target);
                }
            }
            Some(RatingNeighborDecision::Empty) => {
                self.close_fullscreen_for_rating_membership();
            }
            None => {}
        }
        match &cursor {
            RatingNeighborDecision::Keep(key) => {
                self.selected = positions.get(key).copied();
                self.grid_click_selection_anchor = old
                    .shift_anchor
                    .as_ref()
                    .and_then(|key| positions.get(key).copied())
                    .filter(|idx| self.visible_indices.contains(idx))
                    .map(|index| GridClickSelectionAnchor::new(index, self.items_generation));
            }
            RatingNeighborDecision::Move(key) => {
                self.apply_rating_membership_cursor(positions.get(key).copied())
            }
            RatingNeighborDecision::Empty => self.apply_rating_membership_cursor(None),
        }
        if let RatingAdoptionIntent::RestoreOrigin(origin) = intent {
            if origin
                .target
                .key()
                .is_some_and(|key| !positions.contains_key(key))
            {
                let restore_viewer = origin.viewer_key().is_some();
                let pending_intent = RatingAdoptionIntent::RestoreOrigin(origin);
                let mut preparing = false;
                if let Some(pending) = self.rating_view_pending.as_mut() {
                    pending.rating_adoption = pending_intent.clone();
                    preparing = true;
                }
                preparing |= self.set_search_rating_adoption_intent(pending_intent);
                if !preparing && restore_viewer {
                    self.show_feedback_toast(
                        "評価は戻しましたが、元のページは現在の一覧にありません".into(),
                    );
                }
            } else if origin.viewer_key().is_some_and(|key| !reader.contains(key)) {
                self.show_feedback_toast(
                    "評価は戻しましたが、元のページは現在の表示条件に含まれません".into(),
                );
            }
        }
        crate::logger::log(format!(
            "[RATING] projection context={} write_generation={} items_generation={}->{} page={page:?} cursor={cursor:?}",
            old.owner, self.rating_session_write_generation, old.generation, self.items_generation
        ));
    }

    pub(crate) fn cancel_rating_membership_restore(&mut self) {
        let owner = self.projected_viewer_context_id().serial();
        let listing = self.rating_listing_identity();
        self.meta_undo.cancel_grid_rating_origins(owner, &listing);
        if let Some(picker) = self.ring_picker.as_mut()
            && let Some(origin) = picker.rating_origin.as_mut()
            && origin.owner == owner
            && origin.listing_identity == listing
            && matches!(origin.target, RatingOriginTarget::Grid(_))
        {
            origin.target = RatingOriginTarget::ValueOnly;
        }
        self.cancel_pending_rating_membership_restore();
    }

    // Undo/Redo replace the pending request, but are not manual navigation and
    // must leave the history entry's symmetric Grid landing permission intact.
    pub(crate) fn cancel_pending_rating_membership_restore(&mut self) {
        self.cancel_sidecar_restore_rating_deferred_target();
        if let Some(pending) = self.rating_view_pending.as_mut() {
            pending.rating_adoption = RatingAdoptionIntent::Reconcile;
        }
        self.cancel_search_rating_adoption_intent();
    }

    pub(crate) fn cancel_rating_membership_restore_for_context(&mut self, id: ViewerContextId) {
        if id == self.projected_viewer_context_id() {
            self.cancel_rating_membership_restore();
        }
        // Retired bundles own their requests; dropping the bundle cancels them without mounting it.
    }

    pub(crate) fn apply_rating_undo_to_origin(
        &mut self,
        changes: &[crate::undo_stack::RatingChange],
        use_before: bool,
        origin: Option<&RatingOrigin>,
    ) -> Result<Option<String>, String> {
        let intent = origin
            .filter(|origin| match origin.target {
                RatingOriginTarget::Grid(_) => true,
                RatingOriginTarget::Viewer { .. } => use_before,
                RatingOriginTarget::ValueOnly => false,
            })
            .cloned()
            .map(RatingAdoptionIntent::RestoreOrigin)
            .unwrap_or(RatingAdoptionIntent::Reconcile);
        let Some(origin) = origin else {
            return self
                .apply_rating_changes_to_app(changes, use_before, intent)
                .map(|()| None);
        };
        let owner = self
            .viewer_context_ids()
            .into_iter()
            .find(|id| id.serial() == origin.owner);
        if let Some(owner) = owner {
            self.with_viewer_context(owner, |app| {
                let same_listing = app.rating_listing_identity() == origin.listing_identity;
                let intent = if same_listing {
                    intent
                } else {
                    RatingAdoptionIntent::Reconcile
                };
                app.cancel_pending_rating_membership_restore();
                app.apply_rating_changes_to_app(changes, use_before, intent)?;
                let unavailable = use_before
                    && origin.viewer_key().is_some_and(|key| {
                        !same_listing
                            || ((0..app.items.len())
                                .any(|idx| app.rating_membership_key(idx).as_ref() == Some(key))
                                && app
                                    .fullscreen_idx
                                    .and_then(|idx| app.rating_membership_key(idx))
                                    .as_ref()
                                    != Some(key))
                    });
                Ok(unavailable.then(|| "元のページは現在の一覧では表示できません".into()))
            })
            .unwrap_or_else(|_| Err("評価の表示文脈を利用できません".into()))
        } else {
            // The durable key edit remains valid, but no dead reader is recreated.
            self.apply_rating_changes_to_app(changes, use_before, RatingAdoptionIntent::Reconcile)
                .map(|()| use_before.then(|| "元の表示文脈は既に閉じられています".into()))
        }
    }

    pub(crate) fn release_rating_preview_membership(&mut self, intent: RatingAdoptionIntent) {
        let old = self.capture_rating_membership();
        self.rebuild_visible_indices_for_rating_adoption();
        if self.items_are_global_search_view {
            self.rebuild_items_from_global_search();
        }
        if self.items_are_rating_view {
            self.request_rating_view_membership_refresh();
        }
        self.refresh_smart_folder_rating_navigation_entries();
        self.adopt_rating_membership(old, intent);
    }
}
