use super::*;
use crate::vram_budget::{
    VramAccountant, VramAccountingSnapshot, VramBudgetMode, VramPoolBudget, VramSubsystem,
};

const VRAM_ACCOUNTING_INTERVAL: std::time::Duration = std::time::Duration::from_secs(1);

fn add_fs_cache_entry(
    accountant: &mut VramAccountant,
    subsystem: VramSubsystem,
    entry: &FsCacheEntry,
) {
    match entry {
        FsCacheEntry::RawPreview { preview, .. } => {
            if let Some(preview) = preview {
                accountant.add_texture(subsystem, &preview.tex, true);
            }
        }
        FsCacheEntry::Static { tex, .. } => accountant.add_texture(subsystem, tex, true),
        FsCacheEntry::Animated { frames, .. } => {
            for (texture, _) in frames {
                accountant.add_texture(subsystem, texture, false);
            }
        }
        FsCacheEntry::Failed | FsCacheEntry::Video { .. } => {}
    }
}

fn includes_idx(indices: Option<&std::collections::HashSet<usize>>, idx: usize) -> bool {
    indices.is_none_or(|indices| indices.contains(&idx))
}

fn add_lanczos_resource(
    accountant: &mut VramAccountant,
    resource: &crate::gpu_lanczos::FullscreenPaintResource,
) {
    if let Some(output) = resource.lanczos_output() {
        accountant.add_texture_id(
            VramSubsystem::LanczosOutputs,
            output.texture_id(),
            output.size(),
            false,
        );
    }
}

fn add_similar_preview_source(
    accountant: &mut VramAccountant,
    panel: &crate::ui_metadata_panel::SimilarPanelState,
) {
    if let Some(texture) = panel.preview.cached_texture() {
        accountant.add_texture(VramSubsystem::FsCache, texture, true);
    }
}

fn add_lanczos_cache(
    accountant: &mut VramAccountant,
    cache: &crate::gpu_lanczos::GpuLanczosCache,
    indices: Option<&std::collections::HashSet<usize>>,
) {
    for (source_id, output) in cache.outputs() {
        let included = match source_id {
            crate::gpu_lanczos::FullscreenPaintSourceId::Page(idx) => includes_idx(indices, idx),
            crate::gpu_lanczos::FullscreenPaintSourceId::SimilarPreview(_) => indices.is_none(),
        };
        if included {
            accountant.add_texture_id(
                VramSubsystem::LanczosOutputs,
                output.texture_id(),
                output.size(),
                false,
            );
        }
    }
}

/// Read-only normal GPU owners shared by mounted and at-rest contexts.
/// Presentation inventory is deliberately independent of these resident stores.
pub(in crate::app) struct ViewerTextureCaches<'a> {
    pub(in crate::app) fs_lanczos_cache: &'a crate::gpu_lanczos::GpuLanczosCache,
    pub(in crate::app) similar_panel: &'a crate::ui_metadata_panel::SimilarPanelState,
    pub(in crate::app) fs_cache: &'a ItemsGenerationMap<FsCacheEntry>,
    pub(in crate::app) final_composite_cache: &'a FinalCompositeCache,
    pub(in crate::app) adjustment_cache: &'a std::collections::HashMap<usize, FsCacheEntry>,
    pub(in crate::app) erase_result_cache:
        &'a std::collections::HashMap<EraseResultKey, EraseResultCacheEntry>,
    pub(in crate::app) erase_preview_cache:
        &'a std::collections::HashMap<usize, ErasePreviewCacheEntry>,
    pub(in crate::app) analysis_overlay_texture: Option<&'a egui::TextureHandle>,
    pub(in crate::app) analysis_sv_texture: Option<&'a egui::TextureHandle>,
    pub(in crate::app) snapshot: Option<&'a crate::snapshot::SnapshotState>,
    pub(in crate::app) composition_editor:
        &'a super::book_composition_editor::CompositionEditorState,
    pub(in crate::app) local_adjust_cache:
        &'a std::collections::HashMap<LocalAdjustResultKey, LocalAdjustCacheEntry>,
    pub(in crate::app) conceal_cache: &'a std::collections::HashMap<usize, ConcealCacheEntry>,
    pub(in crate::app) comic_cache: &'a std::collections::HashMap<usize, ComicCacheEntry>,
    pub(in crate::app) edit_result_cache:
        &'a std::collections::HashMap<EditResultKey, EditResultEntry>,
    pub(in crate::app) continuous_page_transitions:
        &'a std::collections::HashMap<usize, ContinuousPageTransition>,
    pub(in crate::app) passthrough_rendition_cache: &'a PassthroughRenditionCache,
    pub(in crate::app) fs_holdover_tex: &'a Option<FsHoldover>,
    pub(in crate::app) thumbnails: &'a [ThumbnailState],
    pub(in crate::app) thumb_adjust_tex: &'a std::collections::HashMap<usize, egui::TextureHandle>,
}

impl ViewerTextureCaches<'_> {
    fn add_fullscreen_textures(
        &self,
        accountant: &mut VramAccountant,
        indices: Option<&std::collections::HashSet<usize>>,
    ) {
        add_lanczos_cache(accountant, self.fs_lanczos_cache, indices);
        if indices.is_none() {
            add_similar_preview_source(accountant, self.similar_panel);
        }
        for (&idx, entry) in self.fs_cache {
            if includes_idx(indices, idx) {
                add_fs_cache_entry(accountant, VramSubsystem::FsCache, entry);
            }
        }
        for (key, entry) in self.final_composite_cache {
            if includes_idx(indices, key.edit_key.idx) {
                accountant.add_texture(
                    VramSubsystem::FinalCompositeCache,
                    &entry.texture,
                    entry.texture_options.mipmap_mode.is_some(),
                );
            }
        }
        for (&idx, entry) in self.adjustment_cache {
            if includes_idx(indices, idx) {
                add_fs_cache_entry(accountant, VramSubsystem::AdjustmentCache, entry);
            }
        }
        for (key, entry) in self.erase_result_cache {
            if includes_idx(indices, key.idx) {
                accountant.add_texture(VramSubsystem::EraseResultCache, &entry.texture, true);
            }
        }
        if indices.is_none() {
            for entry in self.erase_preview_cache.values() {
                accountant.add_texture(VramSubsystem::EraseResultCache, &entry.texture, true);
            }
            if let Some(texture) = self.analysis_overlay_texture {
                // The analysis filter/grayscale producers use DISPLAY_IMAGE_TEXTURE_OPTIONS.
                accountant.add_texture(VramSubsystem::FsCache, texture, true);
            }
            if let Some(texture) = self.analysis_sv_texture {
                // No production writer remains; preserve any still-owned single-level chart.
                accountant.add_texture(VramSubsystem::FsCache, texture, false);
            }
        }
        for (key, entry) in self.local_adjust_cache {
            if includes_idx(indices, key.idx) {
                accountant.add_texture(VramSubsystem::LocalAdjustCache, &entry.texture, true);
            }
        }
        for (&idx, entry) in self.conceal_cache {
            if includes_idx(indices, idx) {
                accountant.add_texture(VramSubsystem::ConcealCache, &entry.texture, true);
            }
        }
        for (&idx, entry) in self.comic_cache {
            if includes_idx(indices, idx) {
                accountant.add_texture(VramSubsystem::ComicCache, &entry.texture, true);
            }
        }
        for (key, entry) in self.edit_result_cache {
            if includes_idx(indices, key.idx)
                && let Some(texture) = entry.texture.as_ref()
            {
                accountant.add_texture(VramSubsystem::EditResultCache, texture, true);
            }
        }
        for (&idx, entry) in self.continuous_page_transitions {
            if includes_idx(indices, idx) {
                accountant.add_texture(
                    VramSubsystem::ContinuousPageTransitions,
                    entry.texture.source_texture(),
                    true,
                );
                add_lanczos_resource(accountant, &entry.texture);
            }
        }
        if indices.is_none() {
            for entry in self.passthrough_rendition_cache.entries.values() {
                accountant.add_texture(VramSubsystem::FinalCompositeCache, &entry.texture, false);
            }
        }
        if indices.is_none() {
            let unit = self
                .fs_holdover_tex
                .as_ref()
                .and_then(|holdover| match holdover {
                    FsHoldover::FolderNavigation(unit, _) => unit.as_ref(),
                    FsHoldover::PresentationSwitch(unit) => Some(unit),
                    FsHoldover::NavigationSequence(sequence) => sequence.previous.as_ref(),
                    FsHoldover::FinalEffectSourceReload(holdover) => Some(&holdover.previous),
                });
            if let Some(unit) = unit {
                for page in &unit.pages {
                    accountant.add_texture(
                        VramSubsystem::FsCache,
                        page.texture.source_texture(),
                        true,
                    );
                    add_lanczos_resource(accountant, &page.texture);
                }
            }
        }
    }

    fn add_thumbnail_textures(
        &self,
        accountant: &mut VramAccountant,
        indices: Option<&std::collections::HashSet<usize>>,
    ) {
        if let Some(indices) = indices {
            for &idx in indices {
                if let Some(ThumbnailState::Loaded { tex, .. }) = self.thumbnails.get(idx) {
                    accountant.add_texture(VramSubsystem::Thumbnails, tex, false);
                }
            }
        } else {
            for thumbnail in self.thumbnails {
                if let ThumbnailState::Loaded { tex, .. } = thumbnail {
                    accountant.add_texture(VramSubsystem::Thumbnails, tex, false);
                }
            }
        }
        if let Some(indices) = indices {
            for idx in indices {
                if let Some(texture) = self.thumb_adjust_tex.get(idx) {
                    accountant.add_texture(VramSubsystem::ThumbAdjustTex, texture, false);
                }
            }
        } else {
            for texture in self.thumb_adjust_tex.values() {
                accountant.add_texture(VramSubsystem::ThumbAdjustTex, texture, false);
            }
        }
        if indices.is_none() {
            if let Some(snapshot) = self.snapshot {
                for state in snapshot
                    .saved_thumbnails
                    .iter()
                    .chain(&snapshot.list_view_thumbnails)
                {
                    if let ThumbnailState::Loaded { tex, .. } = state {
                        accountant.add_texture(VramSubsystem::Thumbnails, tex, false);
                    }
                }
            }
            for texture in self.similar_panel.resident_thumbnail_textures() {
                accountant.add_texture(VramSubsystem::Thumbnails, texture, false);
            }
            for texture in self.composition_editor.resident_thumbnail_textures() {
                accountant.add_texture(VramSubsystem::ThumbTextures, texture, false);
            }
        }
    }
}

impl App {
    #[cfg(windows)]
    fn add_other_viewer_textures(&self, accountant: &mut VramAccountant) {
        for id in self.other_viewer_context_ids() {
            let _ = self.with_viewer_context_ref(id, |context| {
                let caches = context.texture_caches();
                caches.add_fullscreen_textures(accountant, None);
                caches.add_thumbnail_textures(accountant, None);
            });
        }
        for window in &self.detached_image_windows {
            accountant.add_texture(
                VramSubsystem::FsCache,
                window.texture.source_texture(),
                true,
            );
            add_lanczos_resource(accountant, &window.texture);
            for page in &window.frozen_continuous_pages {
                accountant.add_texture(VramSubsystem::FsCache, page.texture.source_texture(), true);
                add_lanczos_resource(accountant, &page.texture);
            }
        }
        for shared in self.deferred_detached_image_window_views.values() {
            if let Some(view) = shared.view() {
                accountant.add_texture(VramSubsystem::FsCache, view.texture.source_texture(), true);
                add_lanczos_resource(accountant, &view.texture);
                for page in &view.frozen_continuous_pages {
                    accountant.add_texture(
                        VramSubsystem::FsCache,
                        page.texture.source_texture(),
                        true,
                    );
                    add_lanczos_resource(accountant, &page.texture);
                }
            }
        }
    }

    pub(in crate::app) fn viewer_texture_caches(&self) -> ViewerTextureCaches<'_> {
        ViewerTextureCaches {
            fs_lanczos_cache: &self.fs_lanczos_cache,
            similar_panel: &self.similar_panel,
            fs_cache: &self.fs_cache,
            final_composite_cache: &self.final_composite_cache,
            adjustment_cache: &self.adjustment_cache,
            erase_result_cache: &self.erase_result_cache,
            erase_preview_cache: &self.erase_preview_cache,
            analysis_overlay_texture: self.analysis_overlay_cache.as_ref().map(|entry| &entry.0),
            analysis_sv_texture: self.analysis_sv_cache.as_ref().map(|entry| &entry.3),
            snapshot: self.snapshot.as_ref(),
            composition_editor: &self.composition_editor,
            local_adjust_cache: &self.local_adjust_cache,
            conceal_cache: &self.conceal_cache,
            comic_cache: &self.comic_cache,
            edit_result_cache: &self.edit_result_cache,
            continuous_page_transitions: &self.continuous_page_transitions,
            passthrough_rendition_cache: &self.passthrough_rendition_cache,
            fs_holdover_tex: &self.fs_holdover_tex,
            thumbnails: &self.thumbnails,
            thumb_adjust_tex: &self.thumb_adjust_tex,
        }
    }

    fn add_fullscreen_textures(
        &self,
        accountant: &mut VramAccountant,
        indices: Option<&std::collections::HashSet<usize>>,
    ) {
        self.viewer_texture_caches()
            .add_fullscreen_textures(accountant, indices);
        for (&(idx, _), entry) in &self.ai_upscale_cache {
            if includes_idx(indices, idx) {
                add_fs_cache_entry(accountant, VramSubsystem::AiUpscaleCache, entry);
            }
        }
        for (key, entry) in &self.local_adjust_layer_bypass_cache {
            if includes_idx(indices, key.result_key.idx) {
                accountant.add_texture(VramSubsystem::LocalAdjustCache, &entry.texture, true);
            }
        }
        for (key, entry) in &self.local_adjust_prefix_preview_cache {
            if includes_idx(indices, key.result_key.idx) {
                accountant.add_texture(VramSubsystem::LocalAdjustCache, &entry.texture, true);
            }
        }
    }

    fn add_thumbnail_textures(
        &self,
        accountant: &mut VramAccountant,
        indices: Option<&std::collections::HashSet<usize>>,
    ) {
        self.viewer_texture_caches()
            .add_thumbnail_textures(accountant, indices);
        if indices.is_none()
            && let Some(state) = self.book_reorder.as_ref()
        {
            for texture in state.thumb_textures.values() {
                accountant.add_texture(VramSubsystem::ThumbTextures, texture, false);
            }
        }
    }

    pub(crate) fn vram_accounting_snapshot(&self) -> VramAccountingSnapshot {
        let mut accountant = VramAccountant::default();
        self.add_fullscreen_textures(&mut accountant, None);
        #[cfg(windows)]
        self.add_other_viewer_textures(&mut accountant);
        self.add_thumbnail_textures(&mut accountant, None);
        // Weak inventory includes context-local resting units and submitted
        // batches after cache eviction. Existing stores keep attribution first;
        // the common TextureId accountant deduplicates shared sources/outputs.
        self.page_turn_leases.account(&mut accountant);
        if let Some(pending) = &self.pending_fs_page_turn_ready {
            pending.account(&mut accountant);
        }
        if self.settings.page_turn_animation_enabled {
            for id in self.other_viewer_context_ids() {
                let _ = self.with_viewer_context_ref(id, |context| {
                    if let Some(pending) = context.pending_fs_page_turn_ready() {
                        pending.account(&mut accountant);
                    }
                });
            }
        }
        accountant.finish()
    }

    pub(crate) fn fullscreen_texture_texels_for_indices(
        &self,
        indices: &std::collections::HashSet<usize>,
    ) -> usize {
        let mut accountant = VramAccountant::default();
        self.add_fullscreen_textures(&mut accountant, Some(indices));
        accountant.finish().total_texels
    }

    pub(crate) fn thumbnail_texture_texels_for_indices(
        &self,
        indices: &std::collections::HashSet<usize>,
    ) -> usize {
        let mut accountant = VramAccountant::default();
        self.add_thumbnail_textures(&mut accountant, Some(indices));
        accountant.finish().total_texels
    }

    pub(crate) fn vram_budget_mode(&self) -> VramBudgetMode {
        if self.fullscreen_idx.is_some() {
            VramBudgetMode::Fullscreen
        } else {
            VramBudgetMode::Grid
        }
    }

    pub(crate) fn vram_pool_budget(&self) -> VramPoolBudget {
        VramPoolBudget::from_percent(self.settings.gpu_memory_percent, self.vram_budget_mode())
    }

    pub(crate) fn emit_vram_accounting_if_due(&mut self, now: std::time::Instant) {
        if !crate::perf::is_enabled()
            || self
                .last_vram_accounting_at
                .is_some_and(|last| now.saturating_duration_since(last) < VRAM_ACCOUNTING_INTERVAL)
        {
            return;
        }
        self.last_vram_accounting_at = Some(now);

        let snapshot = self.vram_accounting_snapshot();
        let mode = self.vram_budget_mode();
        let budget = self.vram_pool_budget();
        crate::perf::event(
            "gpu",
            "vram_accounting",
            None,
            self.input_seq,
            &[
                ("mode", mode.as_str().into()),
                ("total_texels", (snapshot.total_texels as u64).into()),
                ("total_bytes", snapshot.total_bytes().into()),
                ("pool_limit_bytes", budget.pool_bytes.unwrap_or(0).into()),
                (
                    "thumbnail_limit_bytes",
                    budget.thumbnail_bytes.unwrap_or(0).into(),
                ),
                (
                    "fullscreen_limit_bytes",
                    budget.fullscreen_bytes.unwrap_or(0).into(),
                ),
                ("unlimited", budget.pool_bytes.is_none().into()),
                ("subsystems", snapshot.subsystems_json()),
            ],
        );
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    #[test]
    fn source_texture_sharing_counts_once_and_frees_only_after_last_cache_owner() {
        let ctx = egui::Context::default();
        let mut app = crate::app::setup_app_for_test();
        app.items = vec![GridItem::Image(PathBuf::from("C:/shared/account.jpg"))];
        let id = crate::app::source_texture_test_support::seed(&mut app, &ctx, 0);
        drop(app.ensure_final_composite_texture(&ctx, 0).unwrap());
        let expected = crate::vram_budget::texture_size_texels([4, 6], true);
        let key = app.current_edit_result_key(0);
        assert_eq!(
            app.edit_result_cache[&key].texture.as_ref().unwrap().id(),
            id
        );
        assert_eq!(app.current_final_composite_texture(0).unwrap().id(), id);
        ctx.tex_manager().write().take_delta().clear();
        assert_eq!(app.vram_accounting_snapshot().total_texels, expected);
        app.fs_cache.clear();
        assert_eq!(app.vram_accounting_snapshot().total_texels, expected);
        assert!(ctx.tex_manager().write().take_delta().is_empty());
        app.edit_result_cache.clear();
        assert_eq!(app.vram_accounting_snapshot().total_texels, expected);
        assert!(ctx.tex_manager().write().take_delta().is_empty());
        app.final_composite_cache.clear();
        assert_eq!(app.vram_accounting_snapshot().total_texels, 0);
        assert!(ctx.tex_manager().read().meta(id).is_none());
        let mut delta = ctx.tex_manager().write().take_delta();
        assert_eq!(delta.free.iter().copied().collect::<Vec<_>>(), vec![id]);
        delta.clear();
    }

    fn texture(
        ctx: &egui::Context,
        name: &str,
        size: [usize; 2],
        mipmapped: bool,
    ) -> egui::TextureHandle {
        ctx.load_texture(
            name,
            egui::ColorImage::filled(size, egui::Color32::WHITE),
            egui::TextureOptions::LINEAR
                .with_mipmap_mode(mipmapped.then_some(egui::TextureFilter::Linear)),
        )
    }

    fn static_entry(texture: egui::TextureHandle) -> FsCacheEntry {
        let size = texture.size();
        FsCacheEntry::Static {
            pdf_rerender_failures: Vec::new(),
            pdf_metadata: None,
            tex: texture,
            pixels: Arc::new(egui::ColorImage::filled(size, egui::Color32::WHITE)),
            source_dims: Some(size),
            load_seq: 0,
            animation: StaticAnimationState::Still,
        }
    }

    fn seed_book(owner: &mut App, source: egui::TextureHandle) {
        let folder = PathBuf::from("C:/vram-accounting/shared-book");
        owner.current_folder = Some(folder.clone());
        owner.items = vec![GridItem::Image(folder.join("page.png"))];
        owner.fullscreen_idx = Some(0);
        owner.fs_cache.insert(0, static_entry(source));
    }

    #[test]
    fn normal_resident_accounting_preserves_parked_sibling_after_eviction_and_retire() {
        let mut app = crate::app::setup_app_for_test();
        let ctx = egui::Context::default();
        let source = texture(&ctx, "shared-source", [4, 4], true);
        let derived = texture(&ctx, "sibling-derived", [8, 8], true);
        let a_only = texture(&ctx, "a-only", [2, 2], true);
        let a = app.build_window_context_for_test(27101, |owner| {
            seed_book(owner, source.clone());
            owner.adjustment_cache.insert(0, static_entry(a_only));
        });
        let b = app.build_window_context_for_test(27102, |owner| {
            seed_book(owner, source.clone());
            // The same physical source is also resident in a derived cache.
            owner.adjustment_cache.insert(0, static_entry(source));
            owner.conceal_cache.insert(
                0,
                ConcealCacheEntry::raster(
                    Arc::new(egui::ColorImage::filled([8, 8], egui::Color32::WHITE)),
                    derived,
                    7,
                ),
            );
        });
        // No presentation lease is registered: all resident evidence is normal ownership.
        // Full mip chains: 4x4 => 21, 8x8 => 85, 2x2 => 5.
        assert_eq!(app.vram_accounting_snapshot().total_texels, 111);
        let b_identity = app
            .with_viewer_context_ref(b, |context| {
                (
                    context.items_generation(),
                    context.current_folder().unwrap().to_path_buf(),
                )
            })
            .unwrap();
        app.with_viewer_context(a, |owner| {
            assert_eq!(owner.vram_accounting_snapshot().total_texels, 111);
            owner.fs_cache.clear();
            owner.adjustment_cache.clear();
            assert_eq!(owner.vram_accounting_snapshot().total_texels, 106);
        })
        .unwrap();
        app.retire_context(a, "vram_accounting_regression", |_| ())
            .unwrap();
        app.with_viewer_context(b, |owner| {
            owner.pause_mounted_background_work_keep_current_frame();
            assert!(matches!(owner.fs_cache.get(&0), Some(FsCacheEntry::Static { tex, .. }) if tex.size() == [4, 4]));
            assert_eq!(owner.conceal_cache[&0].generation, 7);
            assert_eq!(owner.vram_accounting_snapshot().total_texels, 106);
        }).unwrap();
        assert_eq!(
            app.with_viewer_context_ref(b, |context| {
                (
                    context.items_generation(),
                    context.current_folder().unwrap().to_path_buf(),
                )
            })
            .unwrap(),
            b_identity
        );
        for enabled in [false, true] {
            app.settings.page_turn_animation_enabled = enabled;
            let snapshot = app.vram_accounting_snapshot();
            assert_eq!(snapshot.total_texels, 106);
            assert_eq!(snapshot.subsystem_texels(VramSubsystem::FsCache), 21);
            assert_eq!(snapshot.subsystem_texels(VramSubsystem::ConcealCache), 85);
            assert_eq!(snapshot.subsystem_texels(VramSubsystem::AdjustmentCache), 0);
        }
    }

    #[test]
    fn normal_resident_accounting_counts_parked_preview_and_analysis_allocations() {
        let mut app = crate::app::setup_app_for_test();
        let ctx = egui::Context::default();
        let preview = texture(&ctx, "erase-preview", [4, 4], true);
        let overlay = texture(&ctx, "analysis-overlay", [8, 8], true);
        let sv = texture(&ctx, "analysis-sv", [3, 2], false);
        let b = app.build_window_context_for_test(27104, |owner| {
            owner.erase_preview_cache.insert(
                0,
                ErasePreviewCacheEntry::raster(
                    Arc::new(egui::ColorImage::filled([4, 4], egui::Color32::WHITE)),
                    preview,
                ),
            );
            owner.analysis_overlay_cache = Some((overlay, 255, None, 1.0, egui::Vec2::ZERO, 0));
            owner.analysis_sv_cache = Some((1.0, egui::Vec2::ZERO, 0, sv));
        });
        let snapshot = app.vram_accounting_snapshot();
        assert_eq!(snapshot.total_texels, 112);
        assert_eq!(
            snapshot.subsystem_texels(VramSubsystem::EraseResultCache),
            21
        );
        assert_eq!(snapshot.subsystem_texels(VramSubsystem::FsCache), 91);
        app.with_viewer_context(b, |owner| {
            assert_eq!(owner.vram_accounting_snapshot(), snapshot);
            owner.pause_mounted_background_work_keep_current_frame();
            assert_eq!(owner.vram_accounting_snapshot(), snapshot);
        })
        .unwrap();
        assert_eq!(app.vram_accounting_snapshot(), snapshot);
    }

    #[test]
    fn normal_resident_accounting_counts_parked_snapshot_thumbnail_owners_and_deduplicates() {
        let mut app = crate::app::setup_app_for_test();
        let ctx = egui::Context::default();
        let saved = texture(&ctx, "snapshot-saved-thumb", [3, 2], false);
        let list = texture(&ctx, "snapshot-list-thumb", [5, 2], false);
        let loaded = |tex| {
            ThumbnailState::loaded_raster(
                tex,
                crate::thumb_loader::ThumbLoadOrigin::FinalCache,
                false,
                5,
                crate::source_coverage::SourceCoverage::FullSource,
                None,
                None,
            )
        };
        let b = app.build_window_context_for_test(27105, |owner| {
            owner.thumbnails = vec![loaded(saved.clone())];
            let root = PathBuf::from("C:/vram-accounting/snapshot");
            owner.snapshot = Some(crate::snapshot::SnapshotState {
                page_edit_source: crate::snapshot::SnapshotPageEditSource::PhysicalPrefix,
                items: Vec::new(),
                membership: Default::default(),
                origin: root.clone(),
                filter_at_capture: crate::snapshot::FilterState::from_rating([true; 6]),
                source_label: crate::snapshot::SnapshotSourceLabel::DirectMedia {
                    target: root.join("page.png"),
                    policy: owner.settings.file_type_visibility.clone(),
                    reader_order: Vec::new(),
                    details_sort: None,
                    basis_generation: owner.items_generation,
                },
                generation_id: owner.items_generation,
                saved_items: Vec::new(),
                saved_projection_recipe: owner.projection_recipe.clone(),
                saved_viewing_state: owner.capture_snapshot_viewing_state(),
                saved_page_identity_index: owner.page_identity_index.clone(),
                saved_thumbnails: vec![loaded(saved)],
                saved_image_metas: Vec::new(),
                saved_visible_indices: Vec::new(),
                saved_scroll_offset_y: 0.0,
                saved_selected: None,
                saved_zip_nav: None,
                list_view_items: Vec::new(),
                list_view_projection_recipe: owner.projection_recipe.clone(),
                list_view_thumbnails: vec![loaded(list)],
                list_view_image_metas: Vec::new(),
                list_view_page_identity_index: owner.page_identity_index.clone(),
                pre_snapshot_search_origin: None,
            });
        });
        assert_eq!(app.vram_accounting_snapshot().total_texels, 16);
        app.with_viewer_context(b, |owner| {
            owner.thumbnails.clear();
            assert_eq!(owner.vram_accounting_snapshot().total_texels, 16);
            owner.snapshot.as_mut().unwrap().saved_thumbnails.clear();
            assert_eq!(owner.vram_accounting_snapshot().total_texels, 10);
        })
        .unwrap();
        assert_eq!(app.vram_accounting_snapshot().total_texels, 10);
    }

    #[test]
    fn normal_resident_accounting_counts_at_rest_thumbnails_without_presentation() {
        let mut app = crate::app::setup_app_for_test();
        let ctx = egui::Context::default();
        let thumb = texture(&ctx, "parked-thumbnail", [3, 2], false);
        let adjusted = texture(&ctx, "parked-adjusted-thumbnail", [2, 2], false);
        let b = app.build_window_context_for_test(27103, |owner| {
            owner.thumbnails = vec![ThumbnailState::loaded_raster(
                thumb,
                crate::thumb_loader::ThumbLoadOrigin::FinalCache,
                false,
                3,
                crate::source_coverage::SourceCoverage::FullSource,
                None,
                None,
            )];
            owner.thumb_adjust_tex.insert(0, adjusted);
        });
        let snapshot = app.vram_accounting_snapshot();
        assert_eq!(snapshot.subsystem_texels(VramSubsystem::Thumbnails), 6);
        assert_eq!(snapshot.subsystem_texels(VramSubsystem::ThumbAdjustTex), 4);
        assert_eq!(snapshot.total_texels, 10);
        app.with_viewer_context(b, |owner| {
            assert_eq!(owner.vram_accounting_snapshot(), snapshot);
            let indices = std::collections::HashSet::from([0]);
            assert_eq!(owner.thumbnail_texture_texels_for_indices(&indices), 10);
        })
        .unwrap();
        assert_eq!(app.vram_accounting_snapshot(), snapshot);
    }
}
