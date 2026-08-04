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

fn add_lanczos_cache(
    accountant: &mut VramAccountant,
    cache: &crate::gpu_lanczos::GpuLanczosCache,
    indices: Option<&std::collections::HashSet<usize>>,
) {
    for (idx, output) in cache.outputs() {
        if includes_idx(indices, idx) {
            accountant.add_texture_id(
                VramSubsystem::LanczosOutputs,
                output.texture_id(),
                output.size(),
                false,
            );
        }
    }
}

impl App {
    #[cfg(windows)]
    fn add_detached_lanczos_textures(&self, accountant: &mut VramAccountant) {
        if let Some(active) = self.active_detached_viewer_context.as_ref() {
            add_lanczos_cache(accountant, &active.bundle.fs_lanczos_cache, None);
        }
        for window in &self.detached_image_windows {
            add_lanczos_resource(accountant, &window.texture);
            for page in &window.frozen_continuous_pages {
                add_lanczos_resource(accountant, &page.texture);
            }
            if let Some(bundle) = window.paused_bundle.as_deref() {
                add_lanczos_cache(accountant, &bundle.fs_lanczos_cache, None);
            }
        }
        for shared in self.deferred_detached_image_window_views.values() {
            if let Some(view) = shared.view() {
                add_lanczos_resource(accountant, &view.texture);
                for page in &view.frozen_continuous_pages {
                    add_lanczos_resource(accountant, &page.texture);
                }
            }
        }
    }

    fn add_fullscreen_textures(
        &self,
        accountant: &mut VramAccountant,
        indices: Option<&std::collections::HashSet<usize>>,
    ) {
        add_lanczos_cache(accountant, &self.fs_lanczos_cache, indices);
        for (&idx, entry) in &self.fs_cache {
            if includes_idx(indices, idx) {
                add_fs_cache_entry(accountant, VramSubsystem::FsCache, entry);
            }
        }
        for (key, entry) in &self.final_composite_cache {
            if includes_idx(indices, key.edit_key.idx) {
                accountant.add_texture(VramSubsystem::FinalCompositeCache, &entry.texture, true);
            }
        }
        for (&idx, entry) in &self.adjustment_cache {
            if includes_idx(indices, idx) {
                add_fs_cache_entry(accountant, VramSubsystem::AdjustmentCache, entry);
            }
        }
        for (&(idx, _), entry) in &self.ai_upscale_cache {
            if includes_idx(indices, idx) {
                add_fs_cache_entry(accountant, VramSubsystem::AiUpscaleCache, entry);
            }
        }
        for (key, entry) in &self.erase_result_cache {
            if includes_idx(indices, key.idx) {
                accountant.add_texture(VramSubsystem::EraseResultCache, &entry.texture, true);
            }
        }
        for (key, entry) in &self.local_adjust_cache {
            if includes_idx(indices, key.idx) {
                accountant.add_texture(VramSubsystem::LocalAdjustCache, &entry.texture, true);
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
        for (&idx, entry) in &self.conceal_cache {
            if includes_idx(indices, idx) {
                accountant.add_texture(VramSubsystem::ConcealCache, &entry.texture, true);
            }
        }
        for (&idx, entry) in &self.comic_cache {
            if includes_idx(indices, idx) {
                accountant.add_texture(VramSubsystem::ComicCache, &entry.texture, true);
            }
        }
        for (key, entry) in &self.edit_result_cache {
            if includes_idx(indices, key.idx)
                && let Some(texture) = entry.texture.as_ref()
            {
                accountant.add_texture(VramSubsystem::EditResultCache, texture, true);
            }
        }
        for (&idx, entry) in &self.continuous_page_transitions {
            if includes_idx(indices, idx) {
                accountant.add_texture(
                    VramSubsystem::ContinuousPageTransitions,
                    entry.texture.source_texture(),
                    true,
                );
                add_lanczos_resource(accountant, &entry.texture);
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
            for thumbnail in &self.thumbnails {
                if let ThumbnailState::Loaded { tex, .. } = thumbnail {
                    accountant.add_texture(VramSubsystem::Thumbnails, tex, false);
                }
            }
            if let Some(state) = self.book_reorder.as_ref() {
                for texture in state.thumb_textures.values() {
                    accountant.add_texture(VramSubsystem::ThumbTextures, texture, false);
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
    }

    pub(crate) fn vram_accounting_snapshot(&self) -> VramAccountingSnapshot {
        let mut accountant = VramAccountant::default();
        self.add_fullscreen_textures(&mut accountant, None);
        #[cfg(windows)]
        self.add_detached_lanczos_textures(&mut accountant);
        self.add_thumbnail_textures(&mut accountant, None);
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
