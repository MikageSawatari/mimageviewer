//! Write-side cache deletion completion and passive retained-list freshness.
use super::*;
use crate::cover_rotation_cache::{
    AutoCoverSlot, BookCoverId, CatalogScope, CoverCacheStamp, InvalidationCompletion,
    InvalidationRequest, SelectionHint, SourceHint,
};

#[derive(Default)]
pub(super) struct CoverCacheInputs {
    pub(super) hints: Vec<SourceHint>,
    pub(super) scopes: Vec<CatalogScope>,
}

pub(super) fn cover_book_for_item(item: &GridItem, original: Option<&Path>) -> Option<BookCoverId> {
    let mut book = BookCoverId::from_grid_item(item)?;
    if let Some(original) = original {
        let logical = PathBuf::from(crate::adjustment_db::normalize_path(original));
        book = match book {
            BookCoverId::Zip {
                prefix: Some(prefix),
                ..
            } => BookCoverId::Zip {
                path: logical,
                prefix: Some(prefix),
            },
            BookCoverId::Zip { .. } => BookCoverId::Archive(logical),
            book => book,
        };
    }
    Some(book)
}

fn is_book_tile(item: &GridItem) -> bool {
    matches!(
        item,
        GridItem::Folder(_)
            | GridItem::ZipFile(_)
            | GridItem::PdfFile(_)
            | GridItem::ZipDir { .. }
            | GridItem::ConvertibleArchive { .. }
    )
}

impl CoverCacheInputs {
    pub(super) fn scope(
        &mut self,
        folder: PathBuf,
        memory: Option<crate::cover_rotation_cache::CacheMap>,
    ) {
        if let Some(scope) = self
            .scopes
            .iter_mut()
            .find(|scope| crate::path_key::eq_keep_drive(&scope.folder, &folder))
        {
            if let Some(memory) = memory {
                if !scope.memory.iter().any(|old| Arc::ptr_eq(old, &memory)) {
                    scope.memory.push(memory);
                }
            }
        } else {
            self.scopes.push(CatalogScope {
                folder,
                memory: memory.into_iter().collect(),
            });
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn grid(
        &mut self,
        items: &[GridItem],
        thumbnails: &[ThumbnailState],
        requests: Option<&ThumbnailRequests>,
        original: Option<&Path>,
        converted: &HashMap<String, ConvertedArchiveSourceState>,
        pool: &ThumbnailPoolCatalog,
        memory: Option<&crate::cover_rotation_cache::CacheMap>,
        settings: &crate::settings::Settings,
        pin_db: Option<&Arc<crate::folder_thumb_pins::FolderThumbPinDb>>,
    ) {
        if let ThumbnailPoolCatalog::Folder(path) | ThumbnailPoolCatalog::View(path) = pool {
            self.scope(path.clone(), memory.cloned());
        }
        let pending = requests
            .map(ThumbnailRequests::automatic_cover_requests)
            .unwrap_or_default();
        for (idx, item) in items.iter().enumerate() {
            let Some(mut book) = cover_book_for_item(item, original) else {
                continue;
            };
            // A retained converted-ZIP grid can outlive its mounted override.
            // The stamp remembers the logical archive, while the item retains
            // the writer kind/prefix and physical read alias.
            if let Some(ThumbnailState::Loaded {
                cover_cache_stamp, ..
            }) = thumbnails.get(idx)
            {
                if let Some(slot) = cover_cache_stamp.slot() {
                    if let BookCoverId::Archive(logical) = &slot.logical_book {
                        match &mut book {
                            BookCoverId::Zip { path, .. } | BookCoverId::Archive(path) => {
                                *path = logical.clone()
                            }
                            _ => {}
                        }
                    }
                }
            }
            let mut hint = SourceHint::new(book);
            if let Some(physical) = BookCoverId::from_grid_item(item) {
                if !crate::path_key::eq_keep_drive(physical.path(), hint.book.path()) {
                    hint.aliases.push(physical.path().to_owned());
                }
            }
            if let Some(state) =
                converted.get(&crate::adjustment_db::normalize_path(hint.book.path()))
            {
                if let ConvertedArchiveSourceState::CachedZip {
                    logical_source,
                    path,
                } = state.read_source()
                {
                    if crate::path_key::eq_keep_drive(logical_source, hint.book.path()) {
                        hint.aliases.push(path.clone());
                    }
                }
            }
            if is_book_tile(item) {
                if let Some(ThumbnailState::Loaded {
                    cover: crate::cover_source_scope::LoadedCoverIdentity::Page(key),
                    auto_material: crate::cover_auto_material::AutoMaterial::Automatic(_),
                    ..
                }) = thumbnails.get(idx)
                {
                    hint.representative_page_keys.push(key.clone());
                }
                let mut auto = crate::cover_auto_crop::AutoCropIdentity::new(
                    settings.thumb_cover_auto_crop_binding,
                );
                auto.folders = settings.thumb_cover_auto_crop_folders;
                let request = pending
                    .iter()
                    .find(|req| req.idx == idx)
                    .cloned()
                    .unwrap_or_else(|| crate::thumb_loader::LoadRequest {
                        path: hint.book.path().to_owned(),
                        auto_cover_crop: Some(auto),
                        folder_thumb_sort: Some(settings.folder_thumb_sort),
                        folder_thumb_depth: settings.folder_thumb_depth,
                        folder_thumb_provenance: matches!(hint.book, BookCoverId::Folder(_))
                            .then_some(crate::catalog::FolderThumbProvenance::AutoSelected),
                        file_type_visibility: settings.file_type_visibility.clone(),
                        zip_dir_prefix: match &hint.book {
                            BookCoverId::Zip { prefix, .. } => prefix.clone(),
                            _ => None,
                        },
                        ..Default::default()
                    });
                hint.selection = Some(SelectionHint {
                    request,
                    pin_db: pin_db.cloned(),
                });
            }
            // Only a collection map uses full paths across parents. A basename map
            // belongs exclusively to its actual pool scope above.
            if let Some(parent) = hint.book.path().parent() {
                self.scope(parent.to_owned(), None);
                if let Some(collection) = crate::catalog::collection_source_parent_identity(parent)
                {
                    self.scope(
                        collection,
                        matches!(pool, ThumbnailPoolCatalog::Collection)
                            .then(|| memory.cloned())
                            .flatten(),
                    );
                }
            }
            if let BookCoverId::Zip { .. } = hint.book {
                self.scope(hint.book.path().to_owned(), None);
            }
            self.hints.push(hint);
        }
    }
}

pub(super) fn affected_cover_indices(
    items: &[GridItem],
    thumbnails: &[ThumbnailState],
    requested: &ThumbnailRequests,
    original: Option<&Path>,
    pool: &ThumbnailPoolCatalog,
    completed: &[InvalidationCompletion],
) -> HashSet<usize> {
    let mut result = HashSet::new();
    let affected: HashSet<_> = completed
        .iter()
        .flat_map(|done| &done.affected_slots)
        .collect();
    for (idx, (item, state)) in items.iter().zip(thumbnails).enumerate() {
        if !is_book_tile(item) {
            continue;
        }
        if let ThumbnailState::Loaded {
            auto_material: crate::cover_auto_material::AutoMaterial::Automatic(_),
            cover: crate::cover_source_scope::LoadedCoverIdentity::Page(page),
            cover_cache_stamp,
            ..
        } = state
        {
            let slot = match cover_cache_stamp {
                CoverCacheStamp::Tracked { slot, .. } => Some(slot.as_ref().clone()),
                CoverCacheStamp::Untracked => cover_book_for_item(item, original)
                    .map(|book| AutoCoverSlot::new(book, page.clone())),
            };
            if slot.as_ref().is_some_and(|slot| affected.contains(slot)) {
                result.insert(idx);
            }
        }
    }
    for req in requested.automatic_cover_requests() {
        let Some(item) = items.get(req.idx).filter(|item| is_book_tile(item)) else {
            continue;
        };
        let Some(book) = cover_book_for_item(item, original) else {
            continue;
        };
        let explicit = req
            .zip_entry
            .as_ref()
            .map(|entry| crate::adjustment_db::zip_entry_key(&req.path, entry))
            .or_else(|| {
                req.pdf_page
                    .map(|page| crate::edit_source::page_key_for_pdf(&req.path, page))
            });
        let matches_slot = |slot: &AutoCoverSlot| {
            slot.logical_book == book.invalidation_owner()
                && explicit
                    .as_ref()
                    .is_none_or(|page| page == &slot.representative_page_key)
        };
        // A request may be admitted after the worker captured its inputs.
        // Existing rows identify that request even without a captured pending
        // selection. An unresolved request must match its exact deleted key;
        // an unrelated historical pin must not cancel a cover-only request.
        let request_key = match &req.catalog_target {
            crate::thumb_loader::RequestCatalog::Parent { row, .. } => {
                Some(std::borrow::Cow::Borrowed(row.row_key.as_str()))
            }
            _ => crate::thumb_loader::cache_key_for_request(&req),
        };
        let catalog = match &req.catalog_target {
            crate::thumb_loader::RequestCatalog::Parent { row, .. } => Some(&row.folder),
            crate::thumb_loader::RequestCatalog::CollectionSourceParent { identity, .. } => {
                Some(identity)
            }
            crate::thumb_loader::RequestCatalog::Fixed { .. } => match pool {
                ThumbnailPoolCatalog::Folder(path) | ThumbnailPoolCatalog::View(path) => Some(path),
                _ => None,
            },
        };
        let affected_request = completed.iter().any(|done| {
            done.pending_slots.iter().any(matches_slot)
                || (explicit.is_some() && done.affected_slots.iter().any(matches_slot))
                || done.deleted_rows.iter().any(|row| {
                    matches_slot(&row.slot)
                        && (explicit.is_some()
                            || (catalog.is_some_and(|scope| {
                                crate::path_key::eq_keep_drive(scope, &row.catalog)
                            }) && request_key
                                .as_ref()
                                .is_some_and(|key| row.key == key.as_ref())))
                })
        });
        if affected_request {
            result.insert(req.idx);
        }
    }
    result
}

pub(super) fn clear_deleted_cover_memory(
    memory: Option<&crate::cover_rotation_cache::CacheMap>,
    pool: &ThumbnailPoolCatalog,
    completed: &[InvalidationCompletion],
) {
    let Some(memory) = memory else { return };
    let Ok(mut map) = memory.write() else { return };
    for row in completed.iter().flat_map(|done| &done.deleted_rows) {
        let same_scope = match pool {
            ThumbnailPoolCatalog::Folder(folder) | ThumbnailPoolCatalog::View(folder) => {
                crate::path_key::eq_keep_drive(&row.catalog, folder)
            }
            _ => false,
        };
        let same_representative = map.get(&row.key).is_some_and(|entry| {
            entry.cover
                == crate::cover_identity::CoverIdentity::Page(
                    row.slot.representative_page_key.clone(),
                )
        });
        if same_scope || (matches!(pool, ThumbnailPoolCatalog::Collection) && same_representative) {
            map.remove(&row.key);
        }
    }
}

impl App {
    pub(super) fn remove_stale_automatic_cover_memory(&self, matching: &[usize]) {
        let slots = matching
            .iter()
            .filter_map(|idx| match self.thumbnails.get(*idx) {
                Some(ThumbnailState::Loaded {
                    cover_cache_stamp, ..
                }) => cover_cache_stamp.slot().cloned(),
                _ => None,
            })
            .collect::<Vec<_>>();
        if slots.is_empty() {
            return;
        }
        let mut inputs = CoverCacheInputs::default();
        for idx in matching {
            if let (Some(item), Some(thumbnail)) = (self.items.get(*idx), self.thumbnails.get(*idx))
            {
                inputs.grid(
                    std::slice::from_ref(item),
                    std::slice::from_ref(thumbnail),
                    None,
                    self.archive_source_override.as_deref(),
                    &self.converted_archive_cache_paths,
                    &self.thumbnail_pool_catalog,
                    self.current_color_cache_map.as_ref(),
                    &self.settings,
                    self.folder_thumb_pin_db.as_ref(),
                );
            }
        }
        let Some(memory) = &self.current_color_cache_map else {
            return;
        };
        // A restored list may install its old map after the deletion completed.
        // The exact logical-book key and Page identity qualify every removal;
        // no catalog access or guessed basename deletion is needed here.
        for scope in &inputs.scopes {
            crate::cover_rotation_cache::remove_invalidated_memory_rows(
                &scope.folder,
                memory,
                &slots,
                &inputs.hints,
            );
        }
    }

    pub(super) fn automatic_cover_stamp(
        &self,
        idx: usize,
        cover: &crate::cover_identity::CoverIdentity,
        material: crate::cover_auto_material::AutoMaterial,
    ) -> CoverCacheStamp {
        if !matches!(
            material,
            crate::cover_auto_material::AutoMaterial::Automatic(_)
        ) {
            return Default::default();
        }
        let Some(item) = self.items.get(idx).filter(|item| is_book_tile(item)) else {
            return Default::default();
        };
        let Some(logical_book) = cover_book_for_item(item, self.archive_source_override.as_deref())
        else {
            return Default::default();
        };
        let crate::cover_identity::CoverIdentity::Page(page) = cover else {
            return Default::default();
        };
        self.cover_rotation_cache
            .stamp(AutoCoverSlot::new(logical_book, page.clone()))
    }

    pub(super) fn invalidate_automatic_covers_after_rotation(&mut self, changed: &[String]) {
        let mut inputs = CoverCacheInputs::default();
        self.append_current_cover_cache_inputs(&mut inputs);
        self.append_parked_cover_cache_inputs(&mut inputs);
        // A retained archive tile may know its logical owner and cover, while
        // the mounted reader alone knows the converted ZIP alias. Keep these
        // facts together until the worker has resolved and joined aliases.
        // Only actual owner maps and relevant source-parent identities need
        // scanning. A collection with many books must not open every unrelated
        // physical catalog because one page was rotated.
        inputs.scopes.retain(|scope| {
            !scope.memory.is_empty()
                || inputs.hints.iter().any(|hint| {
                    if !changed.iter().any(|key| hint.contains_page_key(key)) {
                        return false;
                    }
                    hint.book.path().parent().is_some_and(|parent| {
                        crate::path_key::eq_keep_drive(parent, &scope.folder)
                            || crate::catalog::collection_source_parent_identity(parent)
                                .is_some_and(|identity| identity == scope.folder)
                    }) || crate::path_key::eq_keep_drive(hint.book.path(), &scope.folder)
                })
        });
        for folder in [
            search_results_synthetic_path(),
            reading_history_synthetic_path(),
            bookmark_view_synthetic_path(),
            rating_view_synthetic_path(),
            subfolder_expansion_synthetic_path(),
        ] {
            inputs.scope(folder, None);
        }
        for definition in &self.settings.smart_folders {
            inputs.scope(
                smart_folder::smart_folder_synthetic_path(definition.id),
                None,
            );
        }
        self.cover_rotation_cache.submit(
            InvalidationRequest {
                changed_keys: changed.to_vec(),
                source_hints: inputs.hints,
                scopes: inputs.scopes,
                work: crate::catalog::CatalogWork::capture(&crate::catalog::default_cache_dir()),
                archive_cache: self.archive_cache_db.clone(),
            },
            self.edit_preview_repaint_ctx.clone(),
        );
    }

    fn append_current_cover_cache_inputs(&self, inputs: &mut CoverCacheInputs) {
        inputs.grid(
            &self.items,
            &self.thumbnails,
            Some(&self.requested),
            self.archive_source_override.as_deref(),
            &self.converted_archive_cache_paths,
            &self.thumbnail_pool_catalog,
            self.current_color_cache_map.as_ref(),
            &self.settings,
            self.folder_thumb_pin_db.as_ref(),
        );
        if let Some(snapshot) = &self.snapshot {
            for (items, thumbs) in [
                (&snapshot.saved_items, &snapshot.saved_thumbnails),
                (&snapshot.list_view_items, &snapshot.list_view_thumbnails),
            ] {
                inputs.grid(
                    items,
                    thumbs,
                    None,
                    None,
                    &self.converted_archive_cache_paths,
                    &self.thumbnail_pool_catalog,
                    self.current_color_cache_map.as_ref(),
                    &self.settings,
                    self.folder_thumb_pin_db.as_ref(),
                );
            }
        }
        if let Some(session) = self.top_level_grid_view.smart_folder_session() {
            session.append_cover_cache_inputs(
                inputs,
                &self.settings,
                self.folder_thumb_pin_db.as_ref(),
            );
        }
    }

    pub(super) fn poll_cover_rotation_cache(&mut self, ctx: &egui::Context) {
        let completed = self.cover_rotation_cache.poll();
        if completed.is_empty() {
            return;
        }
        clear_deleted_cover_memory(
            self.current_color_cache_map.as_ref(),
            &self.thumbnail_pool_catalog,
            &completed,
        );
        let matching = affected_cover_indices(
            &self.items,
            &self.thumbnails,
            &self.requested,
            self.archive_source_override.as_deref(),
            &self.thumbnail_pool_catalog,
            &completed,
        );
        let changed = !matching.is_empty();
        for idx in matching {
            self.evict_thumbnail_for_reload(idx);
            self.auto_aspect.samples.remove(&idx);
        }
        self.evict_parked_automatic_covers(&completed);
        if changed {
            self.auto_aspect.streak = None;
            self.auto_aspect.cached_sample_gate = None;
        }
        for error in completed.iter().flat_map(|done| &done.errors) {
            crate::logger::log(format!("cover cache invalidation: {error}"));
        }
        ctx.request_repaint();
    }
}
