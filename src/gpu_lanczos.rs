//! Product GPU Lanczos3 resampling for fullscreen still-image paint resources.
//!
//! The original [`egui::TextureHandle`] remains the logical-size owner. A native
//! resampled texture only replaces the [`egui::TextureId`] supplied to paint.

use std::{
    borrow::Cow,
    collections::{HashMap, HashSet},
    sync::Arc,
    time::Instant,
};

use wgpu::util::DeviceExt as _;

use crate::{
    adjustment::PostFilter,
    displayed_image_transform::{
        VisibleRegionRequest, physical_pixel_extent, physical_scale_is_near_integer,
    },
};
use crate::{
    gpu_anime4k::{Anime4kJob, Anime4kPlan, Anime4kResampler, STILL_IMAGE_ANIME4K_VARIANT},
    settings::AnimeUpscaleSourceLimit,
};

const LANCZOS3_SHADER: &str = include_str!("gpu_lanczos_spike.wgsl");
const VISIBLE_UPSCALE_LANCZOS3_SHADER: &str = include_str!("gpu_lanczos_visible_upscale.wgsl");
const NIS_UPSCALE_SHADER: &str = include_str!("gpu_nis.wgsl");
const PIXEL_AA_UPSCALE_SHADER: &str = include_str!("gpu_pixel_aa.wgsl");
const MAX_CACHE_ENTRIES: usize = 64;
const MAX_TARGETS_PER_SOURCE: usize = 2;
// Keep the generated texture within the app's cross-GPU edge limit and cap one
// persistent RGBA8 output at 64 MiB. The two-pass intermediate is also bounded
// because each upscale axis is smaller than its corresponding output axis.
const MAX_UPSCALE_TARGET_DIMENSION: u32 = crate::app::MAX_TEXTURE_DIM as u32;
const MAX_UPSCALE_TARGET_PIXELS: u64 = 4096 * 4096;
const MAX_CACHED_UPSCALE_PIXELS: u64 = MAX_UPSCALE_TARGET_PIXELS * 2;

/// A coarser bucket visibly shifts hard edges after the final linear resize. The
/// CPU-reference comparison is recorded in the stage-4 plan, so exact 1 px targets
/// deliberately take precedence over allocation bucketing.
pub(crate) const TARGET_SIZE_QUANTUM: u32 = 1;

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
pub(crate) enum FullscreenPaintScaleBranch {
    DownscaleLanczos,
    OriginalOneToOne,
    UpscaleLanczos,
    UpscaleNis,
    UpscaleAnime,
    UpscalePixelArt,
    OriginalUpscale,
}

impl FullscreenPaintScaleBranch {
    fn uses_resampler(self) -> bool {
        matches!(
            self,
            Self::DownscaleLanczos
                | Self::UpscaleLanczos
                | Self::UpscaleNis
                | Self::UpscaleAnime
                | Self::UpscalePixelArt
        )
    }

    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::DownscaleLanczos => "downscale",
            Self::OriginalOneToOne => "one_to_one",
            Self::UpscaleLanczos => "upscale",
            Self::UpscaleNis => "upscale_nis",
            Self::UpscaleAnime => "upscale_anime",
            Self::UpscalePixelArt => "upscale_pixel_art",
            Self::OriginalUpscale => "original_upscale",
        }
    }
}

pub(crate) fn fullscreen_paint_scale_branch(
    logical_scale: f32,
    pixels_per_point: f32,
    post_filter: PostFilter,
) -> FullscreenPaintScaleBranch {
    let physical_scale = physical_scale(logical_scale, pixels_per_point);
    if physical_scale_is_near_integer(logical_scale, pixels_per_point)
        && physical_scale <= 1.0 + 1.0e-4
    {
        FullscreenPaintScaleBranch::OriginalOneToOne
    } else if physical_scale < 1.0 {
        FullscreenPaintScaleBranch::DownscaleLanczos
    } else {
        match post_filter {
            PostFilter::None => FullscreenPaintScaleBranch::UpscaleLanczos,
            PostFilter::UpscaleSharp => FullscreenPaintScaleBranch::UpscaleNis,
            PostFilter::UpscaleAnime => FullscreenPaintScaleBranch::UpscaleAnime,
            PostFilter::UpscalePixelArt => FullscreenPaintScaleBranch::UpscalePixelArt,
            _ => FullscreenPaintScaleBranch::OriginalUpscale,
        }
    }
}

fn physical_scale(logical_scale: f32, pixels_per_point: f32) -> f32 {
    let pixels_per_point = if pixels_per_point.is_finite() && pixels_per_point > 0.0 {
        pixels_per_point
    } else {
        1.0
    };
    logical_scale * pixels_per_point
}

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
pub(crate) struct FullscreenPaintSourceGeneration {
    pub(crate) items: u64,
    pub(crate) input: u64,
}

/// One typed resource shared by live, continuous, holdover, and detached routes.
#[derive(Clone)]
pub(crate) enum FullscreenPaintResource {
    Direct {
        source: egui::TextureHandle,
    },
    Resampleable {
        page_idx: usize,
        source: egui::TextureHandle,
        generation: FullscreenPaintSourceGeneration,
    },
    Lanczos {
        page_idx: usize,
        source: egui::TextureHandle,
        generation: FullscreenPaintSourceGeneration,
        smoothing_percent: u32,
        output: Arc<LanczosOutput>,
    },
}

impl FullscreenPaintResource {
    pub(crate) fn direct(source: egui::TextureHandle) -> Self {
        Self::Direct { source }
    }

    pub(crate) fn resampleable(
        page_idx: usize,
        source: egui::TextureHandle,
        generation: FullscreenPaintSourceGeneration,
    ) -> Self {
        Self::Resampleable {
            page_idx,
            source,
            generation,
        }
    }

    pub(crate) fn source_texture(&self) -> &egui::TextureHandle {
        match self {
            Self::Direct { source }
            | Self::Resampleable { source, .. }
            | Self::Lanczos { source, .. } => source,
        }
    }

    pub(crate) fn size_vec2(&self) -> egui::Vec2 {
        self.source_texture().size_vec2()
    }

    pub(crate) fn size(&self) -> [usize; 2] {
        self.source_texture().size()
    }

    pub(crate) fn source_texture_id(&self) -> egui::TextureId {
        self.source_texture().id()
    }

    pub(crate) fn paint_texture_id(&self) -> egui::TextureId {
        match self {
            Self::Lanczos { output, .. } => output.texture_id(),
            Self::Direct { source } | Self::Resampleable { source, .. } => source.id(),
        }
    }

    pub(crate) fn id(&self) -> egui::TextureId {
        self.paint_texture_id()
    }

    pub(crate) fn lanczos_output(&self) -> Option<&Arc<LanczosOutput>> {
        match self {
            Self::Lanczos { output, .. } => Some(output),
            Self::Direct { .. } | Self::Resampleable { .. } => None,
        }
    }

    pub(crate) fn visible_source_uv_rect(&self) -> Option<egui::Rect> {
        self.lanczos_output()
            .and_then(|output| output.visible_source_uv_rect())
    }

    pub(crate) fn page_idx(&self) -> Option<usize> {
        match self {
            Self::Resampleable { page_idx, .. } | Self::Lanczos { page_idx, .. } => Some(*page_idx),
            Self::Direct { .. } => None,
        }
    }

    fn resampleable_parts(
        &self,
    ) -> Option<(usize, &egui::TextureHandle, FullscreenPaintSourceGeneration)> {
        match self {
            Self::Resampleable {
                page_idx,
                source,
                generation,
            }
            | Self::Lanczos {
                page_idx,
                source,
                generation,
                ..
            } => Some((*page_idx, source, *generation)),
            Self::Direct { .. } => None,
        }
    }

    fn original_resampleable(&self) -> Self {
        match self {
            Self::Lanczos {
                page_idx,
                source,
                generation,
                ..
            } => Self::resampleable(*page_idx, source.clone(), *generation),
            Self::Direct { .. } | Self::Resampleable { .. } => self.clone(),
        }
    }

    fn with_lanczos(&self, output: Arc<LanczosOutput>, smoothing_percent: u32) -> Self {
        let (page_idx, source, generation) = self.resampleable_parts().unwrap();
        Self::Lanczos {
            page_idx,
            source: source.clone(),
            generation,
            smoothing_percent,
            output,
        }
    }
}

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
struct LanczosCacheKey {
    page_idx: usize,
    source_texture_id: egui::TextureId,
    generation: FullscreenPaintSourceGeneration,
    target_size: [u32; 2],
    smoothing_percent: u32,
    scale_branch: FullscreenPaintScaleBranch,
    source_region: LanczosSourceRegionKey,
}

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
enum LanczosSourceRegionKey {
    Full,
    Visible {
        min_x: u32,
        min_y: u32,
        max_x: u32,
        max_y: u32,
    },
}

impl LanczosSourceRegionKey {
    fn from_visible_rect(rect: egui::Rect) -> Self {
        Self::Visible {
            min_x: rect.min.x.to_bits(),
            min_y: rect.min.y.to_bits(),
            max_x: rect.max.x.to_bits(),
            max_y: rect.max.y.to_bits(),
        }
    }

    fn visible_rect(self) -> Option<egui::Rect> {
        match self {
            Self::Full => None,
            Self::Visible {
                min_x,
                min_y,
                max_x,
                max_y,
            } => Some(egui::Rect::from_min_max(
                egui::pos2(f32::from_bits(min_x), f32::from_bits(min_y)),
                egui::pos2(f32::from_bits(max_x), f32::from_bits(max_y)),
            )),
        }
    }
}

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
struct LanczosFallbackSourceKey {
    page_idx: usize,
    source_texture_id: egui::TextureId,
    generation: FullscreenPaintSourceGeneration,
    scale_branch: FullscreenPaintScaleBranch,
}

struct LanczosCacheEntry {
    output: Arc<LanczosOutput>,
    last_used: u64,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct LanczosGenerationStats {
    pub(crate) source_size: [u32; 2],
    pub(crate) target_size: [u32; 2],
    pub(crate) smoothing_percent: u32,
    pub(crate) blur_factor: f32,
    pub(crate) texture_fetches: u64,
    pub(crate) encode_submit_cpu_ms: f64,
    pub(crate) regeneration_count: u64,
    pub(crate) scale_branch: FullscreenPaintScaleBranch,
    /// Source-pixel origin and size when an upscale was generated from the visible region.
    pub(crate) visible_source_region: Option<[f32; 4]>,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct LanczosLimitFallbackStats {
    pub(crate) source_size: [u32; 2],
    pub(crate) target_size: [u32; 2],
    pub(crate) max_dimension: u32,
    pub(crate) max_pixels: u64,
    pub(crate) scale_branch: FullscreenPaintScaleBranch,
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum LanczosPerfEvent {
    Generated(LanczosGenerationStats),
    UpscaleLimitFallback(LanczosLimitFallbackStats),
}

fn upscale_limit_fallback_event(
    source_size: [u32; 2],
    target_size: [u32; 2],
    scale_branch: FullscreenPaintScaleBranch,
) -> LanczosPerfEvent {
    LanczosPerfEvent::UpscaleLimitFallback(LanczosLimitFallbackStats {
        source_size,
        target_size,
        max_dimension: MAX_UPSCALE_TARGET_DIMENSION,
        max_pixels: MAX_UPSCALE_TARGET_PIXELS,
        scale_branch,
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LanczosTargetDecision {
    Resample,
    OriginalFallback,
}

#[derive(Default)]
pub(crate) struct GpuLanczosCache {
    resampler: Option<Lanczos3Resampler>,
    anime_resampler: Option<Anime4kResampler>,
    entries: HashMap<LanczosCacheKey, LanczosCacheEntry>,
    use_clock: u64,
    regeneration_count: u64,
    active_smoothing_percent: u32,
    limit_fallback_sources: HashSet<LanczosFallbackSourceKey>,
}

impl GpuLanczosCache {
    pub(crate) fn resolve(
        &mut self,
        render_state: Option<&egui_wgpu::RenderState>,
        resource: &FullscreenPaintResource,
        logical_scale: f32,
        pixels_per_point: f32,
        visible_region: Option<VisibleRegionRequest>,
        post_filter: PostFilter,
        smoothing_percent: u32,
        anime_source_limit: AnimeUpscaleSourceLimit,
    ) -> (FullscreenPaintResource, Option<LanczosPerfEvent>) {
        let smoothing_percent =
            crate::settings::sanitize_downscale_smoothing_percent(smoothing_percent);
        self.sync_smoothing_percent(smoothing_percent);
        let mut scale_branch =
            fullscreen_paint_scale_branch(logical_scale, pixels_per_point, post_filter);
        if resource.resampleable_parts().is_none() || !scale_branch.uses_resampler() {
            return (resource.original_resampleable(), None);
        }
        let Some(render_state) = render_state else {
            return (resource.original_resampleable(), None);
        };
        let (page_idx, source, generation) = resource.resampleable_parts().unwrap();
        let source_size = [source.size()[0] as u32, source.size()[1] as u32];
        let physical_scale = physical_scale(logical_scale, pixels_per_point);
        let Some((target_size, source_region)) = target_and_source_region_for_branch(
            source_size,
            physical_scale,
            scale_branch,
            visible_region,
        ) else {
            return (resource.original_resampleable(), None);
        };
        scale_branch = scale_branch_with_anime_source_limit(
            scale_branch,
            source_size,
            source_region,
            anime_source_limit,
        );
        if source_region == LanczosSourceRegionKey::Full && target_size == source_size {
            return (resource.original_resampleable(), None);
        }
        let fallback_source = LanczosFallbackSourceKey {
            page_idx,
            source_texture_id: source.id(),
            generation,
            scale_branch,
        };
        if lanczos_target_decision(scale_branch, target_size)
            == LanczosTargetDecision::OriginalFallback
        {
            let first_for_source = self.limit_fallback_sources.insert(fallback_source);
            let event = first_for_source.then_some(upscale_limit_fallback_event(
                source_size,
                target_size,
                scale_branch,
            ));
            return (resource.original_resampleable(), event);
        }
        self.limit_fallback_sources.remove(&fallback_source);
        // The user-facing smoothing control is specifically for downscaling. Keeping
        // upscale at zero also leaves the Lanczos3 support radius at exactly 3.0.
        let effective_smoothing_percent =
            if scale_branch == FullscreenPaintScaleBranch::DownscaleLanczos {
                smoothing_percent
            } else {
                0
            };
        if let FullscreenPaintResource::Lanczos {
            output,
            smoothing_percent: resource_smoothing,
            ..
        } = resource
            && output.size == target_size
            && *resource_smoothing == effective_smoothing_percent
            && output.scale_branch == scale_branch
            && output.source_region == source_region
        {
            return (resource.clone(), None);
        }
        let key = LanczosCacheKey {
            page_idx,
            source_texture_id: source.id(),
            generation,
            target_size,
            smoothing_percent: effective_smoothing_percent,
            scale_branch,
            source_region,
        };
        self.resolve_key(render_state, resource, key)
    }

    fn resolve_key(
        &mut self,
        render_state: &egui_wgpu::RenderState,
        resource: &FullscreenPaintResource,
        key: LanczosCacheKey,
    ) -> (FullscreenPaintResource, Option<LanczosPerfEvent>) {
        self.use_clock = self.use_clock.wrapping_add(1);
        if let Some(entry) = self.entries.get_mut(&key) {
            entry.last_used = self.use_clock;
            return (
                resource.with_lanczos(entry.output.clone(), key.smoothing_percent),
                None,
            );
        }
        self.entries.retain(|candidate, _| {
            candidate.page_idx != key.page_idx
                || (candidate.source_texture_id == key.source_texture_id
                    && candidate.generation == key.generation)
        });
        let source_size = [resource.size()[0] as u32, resource.size()[1] as u32];
        let plan = work_plan_for_key(source_size, key);
        let plan = match plan {
            Ok(plan) => plan,
            Err(_) => return (resource.original_resampleable(), None),
        };
        self.generate(render_state, resource, key, plan)
    }

    fn generate(
        &mut self,
        render_state: &egui_wgpu::RenderState,
        resource: &FullscreenPaintResource,
        key: LanczosCacheKey,
        plan: LanczosWorkPlan,
    ) -> (FullscreenPaintResource, Option<LanczosPerfEvent>) {
        let started = Instant::now();
        let mut renderer = render_state.renderer.write();
        let Some(source_texture) = renderer
            .texture(&resource.source_texture_id())
            .and_then(|texture| texture.texture.as_ref())
        else {
            return (resource.original_resampleable(), None);
        };
        let job = match plan {
            LanczosWorkPlan::AnimeUpscale(plan) => self
                .anime_resampler
                .get_or_insert_with(|| {
                    Anime4kResampler::new(&render_state.device, STILL_IMAGE_ANIME4K_VARIANT)
                })
                .prepare_job(&render_state.device, source_texture, plan)
                .map(LanczosWorkJob::AnimeUpscale),
            plan => {
                let resampler = self
                    .resampler
                    .get_or_insert_with(|| Lanczos3Resampler::new(&render_state.device));
                match plan {
                    LanczosWorkPlan::Full(plan) => resampler
                        .prepare_job(&render_state.device, source_texture, plan)
                        .map(LanczosWorkJob::Full),
                    LanczosWorkPlan::VisibleUpscale(plan) => resampler
                        .prepare_visible_upscale_job(&render_state.device, source_texture, plan)
                        .map(LanczosWorkJob::VisibleUpscale),
                    LanczosWorkPlan::NisUpscale(plan) => resampler
                        .prepare_nis_upscale_job(&render_state.device, source_texture, plan)
                        .map(LanczosWorkJob::NisUpscale),
                    LanczosWorkPlan::PixelArtUpscale(plan) => resampler
                        .prepare_pixel_aa_job(&render_state.device, source_texture, plan)
                        .map(LanczosWorkJob::PixelArtUpscale),
                    LanczosWorkPlan::AnimeUpscale(_) => unreachable!(),
                }
            }
        };
        let job = match job {
            Ok(job) => job,
            Err(_) => return (resource.original_resampleable(), None),
        };
        let texture_id = renderer.register_native_texture(
            &render_state.device,
            job.output_view(),
            wgpu::FilterMode::Linear,
        );
        drop(renderer);
        self.submit_job(render_state, resource, key, plan, job, texture_id, started)
    }

    #[allow(clippy::too_many_arguments)]
    fn submit_job(
        &mut self,
        rs: &egui_wgpu::RenderState,
        resource: &FullscreenPaintResource,
        key: LanczosCacheKey,
        plan: LanczosWorkPlan,
        job: LanczosWorkJob,
        texture_id: egui::TextureId,
        started: Instant,
    ) -> (FullscreenPaintResource, Option<LanczosPerfEvent>) {
        self.encode_and_finish(rs, resource, key, plan, job, texture_id, started)
    }

    #[allow(clippy::too_many_arguments)]
    fn encode_and_finish(
        &mut self,
        rs: &egui_wgpu::RenderState,
        resource: &FullscreenPaintResource,
        key: LanczosCacheKey,
        plan: LanczosWorkPlan,
        job: LanczosWorkJob,
        texture_id: egui::TextureId,
        started: Instant,
    ) -> (FullscreenPaintResource, Option<LanczosPerfEvent>) {
        let mut encoder = rs.device.create_command_encoder(&Default::default());
        match &job {
            LanczosWorkJob::Full(job) => {
                self.resampler.as_ref().unwrap().encode(&mut encoder, job);
            }
            LanczosWorkJob::VisibleUpscale(job) => {
                self.resampler
                    .as_ref()
                    .unwrap()
                    .encode_visible_upscale(&mut encoder, job);
            }
            LanczosWorkJob::NisUpscale(job) => {
                self.resampler
                    .as_ref()
                    .unwrap()
                    .encode_nis_upscale(&mut encoder, job);
            }
            LanczosWorkJob::PixelArtUpscale(job) => {
                self.resampler
                    .as_ref()
                    .unwrap()
                    .encode_pixel_aa(&mut encoder, job);
            }
            LanczosWorkJob::AnimeUpscale(job) => {
                self.anime_resampler
                    .as_ref()
                    .unwrap()
                    .encode(&mut encoder, job);
            }
        }
        rs.queue.submit(Some(encoder.finish()));
        let output_texture = job.into_output_texture();
        let output = Arc::new(LanczosOutput::new(
            texture_id,
            key.target_size,
            output_texture,
            rs.clone(),
            key.scale_branch,
            key.source_region,
        ));
        self.finish_generation(resource, key, plan, output, started)
    }

    fn finish_generation(
        &mut self,
        resource: &FullscreenPaintResource,
        key: LanczosCacheKey,
        plan: LanczosWorkPlan,
        output: Arc<LanczosOutput>,
        started: Instant,
    ) -> (FullscreenPaintResource, Option<LanczosPerfEvent>) {
        self.regeneration_count = self.regeneration_count.wrapping_add(1);
        let stats = LanczosGenerationStats {
            source_size: plan.source_size(),
            target_size: plan.target_size(),
            smoothing_percent: plan.smoothing_percent(),
            blur_factor: plan.blur_factor(),
            texture_fetches: plan.texture_fetches(),
            encode_submit_cpu_ms: started.elapsed().as_secs_f64() * 1000.0,
            regeneration_count: self.regeneration_count,
            scale_branch: key.scale_branch,
            visible_source_region: plan.visible_source_region(),
        };
        self.entries.insert(
            key,
            LanczosCacheEntry {
                output: output.clone(),
                last_used: self.use_clock,
            },
        );
        self.prune_source_targets(key);
        self.prune_global_lru();
        self.prune_upscale_pixels();
        (
            resource.with_lanczos(output, key.smoothing_percent),
            Some(LanczosPerfEvent::Generated(stats)),
        )
    }

    pub(crate) fn retain_page_indices(&mut self, keep: &std::collections::HashSet<usize>) {
        self.entries.retain(|key, _| keep.contains(&key.page_idx));
        self.limit_fallback_sources
            .retain(|key| keep.contains(&key.page_idx));
    }

    pub(crate) fn remove_page(&mut self, page_idx: usize) {
        self.entries.retain(|key, _| key.page_idx != page_idx);
        self.limit_fallback_sources
            .retain(|key| key.page_idx != page_idx);
    }

    pub(crate) fn clear(&mut self) {
        self.entries.clear();
        self.limit_fallback_sources.clear();
    }

    fn sync_smoothing_percent(&mut self, smoothing_percent: u32) -> bool {
        let smoothing_percent =
            crate::settings::sanitize_downscale_smoothing_percent(smoothing_percent);
        if self.active_smoothing_percent == smoothing_percent {
            return false;
        }
        self.entries.clear();
        self.active_smoothing_percent = smoothing_percent;
        true
    }

    pub(crate) fn outputs(&self) -> impl Iterator<Item = (usize, &Arc<LanczosOutput>)> {
        self.entries
            .iter()
            .map(|(key, entry)| (key.page_idx, &entry.output))
    }

    fn prune_source_targets(&mut self, inserted: LanczosCacheKey) {
        let mut siblings = self
            .entries
            .iter()
            .filter(|(key, _)| same_source_target_family(**key, inserted))
            .map(|(key, entry)| (*key, entry.last_used))
            .collect::<Vec<_>>();
        siblings.sort_unstable_by_key(|(_, used)| std::cmp::Reverse(*used));
        for (key, _) in siblings.into_iter().skip(MAX_TARGETS_PER_SOURCE) {
            self.entries.remove(&key);
        }
    }

    fn prune_global_lru(&mut self) {
        while self.entries.len() > MAX_CACHE_ENTRIES {
            let Some(oldest) = self
                .entries
                .iter()
                .min_by_key(|(_, entry)| entry.last_used)
                .map(|(key, _)| *key)
            else {
                break;
            };
            self.entries.remove(&oldest);
        }
    }

    fn prune_upscale_pixels(&mut self) {
        loop {
            let total = self
                .entries
                .iter()
                .filter(|(key, _)| {
                    matches!(
                        key.scale_branch,
                        FullscreenPaintScaleBranch::UpscaleLanczos
                            | FullscreenPaintScaleBranch::UpscaleNis
                            | FullscreenPaintScaleBranch::UpscaleAnime
                            | FullscreenPaintScaleBranch::UpscalePixelArt
                    )
                })
                .fold(0_u64, |pixels, (key, _)| {
                    pixels.saturating_add(target_pixels(key.target_size))
                });
            if total <= MAX_CACHED_UPSCALE_PIXELS {
                break;
            }
            let Some(oldest) = self
                .entries
                .iter()
                .filter(|(key, _)| {
                    matches!(
                        key.scale_branch,
                        FullscreenPaintScaleBranch::UpscaleLanczos
                            | FullscreenPaintScaleBranch::UpscaleNis
                            | FullscreenPaintScaleBranch::UpscaleAnime
                            | FullscreenPaintScaleBranch::UpscalePixelArt
                    )
                })
                .min_by_key(|(_, entry)| entry.last_used)
                .map(|(key, _)| *key)
            else {
                break;
            };
            self.entries.remove(&oldest);
        }
    }
}

fn same_source_target_family(left: LanczosCacheKey, right: LanczosCacheKey) -> bool {
    left.page_idx == right.page_idx
        && left.source_texture_id == right.source_texture_id
        && left.generation == right.generation
        && left.smoothing_percent == right.smoothing_percent
        && left.scale_branch == right.scale_branch
}

pub(crate) struct LanczosOutput {
    texture_id_lease: NativeTextureIdLease,
    size: [u32; 2],
    scale_branch: FullscreenPaintScaleBranch,
    source_region: LanczosSourceRegionKey,
    _texture: wgpu::Texture,
}

impl LanczosOutput {
    fn new(
        texture_id: egui::TextureId,
        size: [u32; 2],
        texture: wgpu::Texture,
        render_state: egui_wgpu::RenderState,
        scale_branch: FullscreenPaintScaleBranch,
        source_region: LanczosSourceRegionKey,
    ) -> Self {
        Self {
            texture_id_lease: NativeTextureIdLease {
                texture_id,
                releaser: Arc::new(RendererTextureIdReleaser { render_state }),
            },
            size,
            scale_branch,
            source_region,
            _texture: texture,
        }
    }

    pub(crate) fn texture_id(&self) -> egui::TextureId {
        self.texture_id_lease.texture_id
    }

    pub(crate) fn size(&self) -> [usize; 2] {
        [self.size[0] as usize, self.size[1] as usize]
    }

    pub(crate) fn visible_source_uv_rect(&self) -> Option<egui::Rect> {
        self.source_region.visible_rect()
    }
}

trait NativeTextureIdReleaser: Send + Sync {
    fn free_texture(&self, texture_id: egui::TextureId);
}

struct RendererTextureIdReleaser {
    render_state: egui_wgpu::RenderState,
}

impl NativeTextureIdReleaser for RendererTextureIdReleaser {
    fn free_texture(&self, texture_id: egui::TextureId) {
        self.render_state.renderer.write().free_texture(&texture_id);
    }
}

struct NativeTextureIdLease {
    texture_id: egui::TextureId,
    releaser: Arc<dyn NativeTextureIdReleaser>,
}

impl Drop for NativeTextureIdLease {
    fn drop(&mut self) {
        self.releaser.free_texture(self.texture_id);
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct LanczosPlan {
    source_size: [u32; 2],
    target_size: [u32; 2],
    smoothing_percent: u32,
    blur_factor: f32,
    texture_fetches: u64,
}

impl LanczosPlan {
    fn new(
        source_size: [u32; 2],
        target_size: [u32; 2],
        smoothing_percent: u32,
    ) -> Result<Self, ()> {
        if source_size.contains(&0) || target_size.contains(&0) {
            return Err(());
        }
        let smoothing_percent =
            crate::settings::sanitize_downscale_smoothing_percent(smoothing_percent);
        let blur_factor = crate::settings::downscale_smoothing_blur_factor(smoothing_percent);
        let vertical = axis_fetch_count(source_size[1], target_size[1], blur_factor)
            .saturating_mul(u64::from(source_size[0]));
        let horizontal = axis_fetch_count(source_size[0], target_size[0], blur_factor)
            .saturating_mul(u64::from(target_size[1]));
        Ok(Self {
            source_size,
            target_size,
            smoothing_percent,
            blur_factor,
            texture_fetches: vertical.saturating_add(horizontal),
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct VisibleUpscalePlan {
    source_size: [u32; 2],
    target_size: [u32; 2],
    source_region_px: [f32; 4],
    intermediate_x_start: u32,
    intermediate_x_width: u32,
    texture_fetches: u64,
}

impl VisibleUpscalePlan {
    fn new(
        source_size: [u32; 2],
        target_size: [u32; 2],
        source_uv_rect: egui::Rect,
    ) -> Result<Self, ()> {
        if source_size.contains(&0) || target_size.contains(&0) {
            return Err(());
        }
        let source_region_px = source_region_pixels(source_size, source_uv_rect)?;
        let x_start = source_region_px[0];
        let y_start = source_region_px[1];
        let x_len = source_region_px[2];
        let y_len = source_region_px[3];
        let (intermediate_x_start, _) =
            region_sample_range(source_size[0], target_size[0], 0, x_start, x_len);
        let (_, intermediate_x_end) = region_sample_range(
            source_size[0],
            target_size[0],
            target_size[0] - 1,
            x_start,
            x_len,
        );
        let intermediate_x_width = intermediate_x_end.saturating_sub(intermediate_x_start);
        if intermediate_x_width == 0 {
            return Err(());
        }
        let vertical = axis_fetch_count_region(source_size[1], target_size[1], y_start, y_len)
            .saturating_mul(u64::from(intermediate_x_width));
        let horizontal = axis_fetch_count_region(
            intermediate_x_width,
            target_size[0],
            x_start - intermediate_x_start as f32,
            x_len,
        )
        .saturating_mul(u64::from(target_size[1]));
        Ok(Self {
            source_size,
            target_size,
            source_region_px,
            intermediate_x_start,
            intermediate_x_width,
            texture_fetches: vertical.saturating_add(horizontal),
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct NisUpscalePlan {
    source_size: [u32; 2],
    target_size: [u32; 2],
    source_region_px: [f32; 4],
    texture_fetches: u64,
}

impl NisUpscalePlan {
    fn new(
        source_size: [u32; 2],
        target_size: [u32; 2],
        source_uv_rect: egui::Rect,
    ) -> Result<Self, ()> {
        if source_size.contains(&0) || target_size.contains(&0) {
            return Err(());
        }
        let source_region_px = source_region_pixels(source_size, source_uv_rect)?;
        // The fragment port loads one 6x6 support tile per output pixel. Its bilinear
        // RGBA base sample reuses the central four loads, so no extra source fetches
        // are counted here.
        let texture_fetches = target_pixels(target_size).saturating_mul(36);
        Ok(Self {
            source_size,
            target_size,
            source_region_px,
            texture_fetches,
        })
    }
}

/// Keep this identical to `pixel_aa_axis_weight` in gpu_pixel_aa.wgsl. This is the
/// sharpness-1.0 slopestep from libretro pixel_aa: the transition width is exactly
/// one destination pixel. A slope above 1.0 is a possible future generalization.
#[cfg(test)]
fn pixel_aa_axis_weight(frac: f32, tx_per_px: f32) -> f32 {
    let lower_bound = 0.5 - 0.5 * tx_per_px;
    let upper_bound = 0.5 + 0.5 * tx_per_px;
    let width = upper_bound - lower_bound;
    if width < 1.0e-6 {
        if frac < 0.5 { 0.0 } else { 1.0 }
    } else {
        ((frac - lower_bound) / width).clamp(0.0, 1.0)
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct PixelArtUpscalePlan {
    source_size: [u32; 2],
    target_size: [u32; 2],
    source_region_px: [f32; 4],
    texture_fetches: u64,
}

impl PixelArtUpscalePlan {
    fn new(
        source_size: [u32; 2],
        target_size: [u32; 2],
        source_uv_rect: egui::Rect,
    ) -> Result<Self, ()> {
        if source_size.contains(&0) || target_size.contains(&0) {
            return Err(());
        }
        let source_region_px = source_region_pixels(source_size, source_uv_rect)?;
        let texture_fetches = target_pixels(target_size).saturating_mul(4);
        Ok(Self {
            source_size,
            target_size,
            source_region_px,
            texture_fetches,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum LanczosWorkPlan {
    Full(LanczosPlan),
    VisibleUpscale(VisibleUpscalePlan),
    NisUpscale(NisUpscalePlan),
    PixelArtUpscale(PixelArtUpscalePlan),
    AnimeUpscale(Anime4kPlan),
}

fn work_plan_for_key(source_size: [u32; 2], key: LanczosCacheKey) -> Result<LanczosWorkPlan, ()> {
    match (key.scale_branch, key.source_region) {
        (FullscreenPaintScaleBranch::UpscaleAnime, source_region) => {
            let source_uv_rect = source_region.visible_rect().ok_or(())?;
            let source_region_px = source_region_pixels(source_size, source_uv_rect)?;
            Anime4kPlan::new(source_size, key.target_size, source_region_px)
                .map(LanczosWorkPlan::AnimeUpscale)
        }
        (FullscreenPaintScaleBranch::UpscaleNis, source_region) => {
            let source_uv_rect = source_region.visible_rect().unwrap_or_else(|| {
                egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0))
            });
            NisUpscalePlan::new(source_size, key.target_size, source_uv_rect)
                .map(LanczosWorkPlan::NisUpscale)
        }
        (FullscreenPaintScaleBranch::UpscalePixelArt, source_region) => {
            let source_uv_rect = source_region.visible_rect().unwrap_or_else(|| {
                egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0))
            });
            PixelArtUpscalePlan::new(source_size, key.target_size, source_uv_rect)
                .map(LanczosWorkPlan::PixelArtUpscale)
        }
        (_, LanczosSourceRegionKey::Full) => {
            LanczosPlan::new(source_size, key.target_size, key.smoothing_percent)
                .map(LanczosWorkPlan::Full)
        }
        (_, LanczosSourceRegionKey::Visible { .. }) => VisibleUpscalePlan::new(
            source_size,
            key.target_size,
            key.source_region.visible_rect().unwrap(),
        )
        .map(LanczosWorkPlan::VisibleUpscale),
    }
}

impl LanczosWorkPlan {
    fn source_size(self) -> [u32; 2] {
        match self {
            Self::Full(plan) => plan.source_size,
            Self::VisibleUpscale(plan) => plan.source_size,
            Self::NisUpscale(plan) => plan.source_size,
            Self::PixelArtUpscale(plan) => plan.source_size,
            Self::AnimeUpscale(plan) => plan.source_size,
        }
    }

    fn target_size(self) -> [u32; 2] {
        match self {
            Self::Full(plan) => plan.target_size,
            Self::VisibleUpscale(plan) => plan.target_size,
            Self::NisUpscale(plan) => plan.target_size,
            Self::PixelArtUpscale(plan) => plan.target_size,
            Self::AnimeUpscale(plan) => plan.target_size,
        }
    }

    fn smoothing_percent(self) -> u32 {
        match self {
            Self::Full(plan) => plan.smoothing_percent,
            Self::VisibleUpscale(_)
            | Self::NisUpscale(_)
            | Self::PixelArtUpscale(_)
            | Self::AnimeUpscale(_) => 0,
        }
    }

    fn blur_factor(self) -> f32 {
        match self {
            Self::Full(plan) => plan.blur_factor,
            Self::VisibleUpscale(_)
            | Self::NisUpscale(_)
            | Self::PixelArtUpscale(_)
            | Self::AnimeUpscale(_) => 1.0,
        }
    }

    fn texture_fetches(self) -> u64 {
        match self {
            Self::Full(plan) => plan.texture_fetches,
            Self::VisibleUpscale(plan) => plan.texture_fetches,
            Self::NisUpscale(plan) => plan.texture_fetches,
            Self::PixelArtUpscale(plan) => plan.texture_fetches,
            Self::AnimeUpscale(plan) => plan.texture_fetches,
        }
    }

    fn visible_source_region(self) -> Option<[f32; 4]> {
        match self {
            Self::Full(_) => None,
            Self::VisibleUpscale(plan) => Some(plan.source_region_px),
            Self::NisUpscale(plan) => Some(plan.source_region_px),
            Self::PixelArtUpscale(plan) => Some(plan.source_region_px),
            Self::AnimeUpscale(plan) => Some(plan.source_region_px),
        }
    }
}

fn quantized_target_size(source_size: [u32; 2], scale: f32, quantum: u32) -> [u32; 2] {
    let quantum = quantum.max(1);
    let quantize = |source: u32| {
        // 描画矩形と同じ関数を通す。ここだけ floor にすると、貼り先が 1px 大きくなって
        // egui/wgpu がもう一度バイリニアを掛ける (§1.0e)。
        let exact = physical_pixel_extent(source as f32, scale);
        let quantized = exact
            .saturating_add(quantum - 1)
            .checked_div(quantum)
            .unwrap_or(1)
            .saturating_mul(quantum);
        if scale <= 1.0 {
            quantized.min(source)
        } else {
            quantized
        }
    };
    [quantize(source_size[0]), quantize(source_size[1])]
}

/// 出力テクセル数と、リサンプラが読む範囲。
///
/// **縮小 (全体) は倍率から、拡大 (可視領域) は要求から**決める。全体を寄せるときは
/// 貼り先も [`physical_pixel_extent`] を同じ長さ・同じ倍率で通るので、ここで倍率から
/// 出し直しても一致する。部分矩形はそうならないので、貼り先を決めた側が持ってきた
/// テクセル数をそのまま使う (理由は
/// [`DisplayedImageTransform::visible_region_request`] の doc、backlog §1.161)。
fn target_and_source_region_for_branch(
    source_size: [u32; 2],
    physical_scale: f32,
    scale_branch: FullscreenPaintScaleBranch,
    visible_region: Option<VisibleRegionRequest>,
) -> Option<([u32; 2], LanczosSourceRegionKey)> {
    match scale_branch {
        FullscreenPaintScaleBranch::DownscaleLanczos => Some((
            quantized_target_size(source_size, physical_scale, TARGET_SIZE_QUANTUM),
            LanczosSourceRegionKey::Full,
        )),
        FullscreenPaintScaleBranch::UpscaleLanczos
        | FullscreenPaintScaleBranch::UpscaleNis
        | FullscreenPaintScaleBranch::UpscaleAnime
        | FullscreenPaintScaleBranch::UpscalePixelArt => {
            let request = visible_region?;
            let visible = sanitize_visible_source_rect(request.source_uv_rect)?;
            source_region_pixels(source_size, visible).ok()?;
            let target_size = request.target_size;
            (target_size[0] > 0 && target_size[1] > 0).then_some((
                target_size,
                LanczosSourceRegionKey::from_visible_rect(visible),
            ))
        }
        FullscreenPaintScaleBranch::OriginalOneToOne
        | FullscreenPaintScaleBranch::OriginalUpscale => None,
    }
}

fn sanitize_visible_source_rect(rect: egui::Rect) -> Option<egui::Rect> {
    if !rect.min.x.is_finite()
        || !rect.min.y.is_finite()
        || !rect.max.x.is_finite()
        || !rect.max.y.is_finite()
    {
        return None;
    }
    let full = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
    let rect = rect.intersect(full);
    (rect.width() > 0.0 && rect.height() > 0.0).then_some(rect)
}

fn source_region_pixels(source_size: [u32; 2], source_uv_rect: egui::Rect) -> Result<[f32; 4], ()> {
    let source_uv_rect = sanitize_visible_source_rect(source_uv_rect).ok_or(())?;
    let min_x = source_uv_rect.min.x * source_size[0] as f32;
    let min_y = source_uv_rect.min.y * source_size[1] as f32;
    let width = source_uv_rect.width() * source_size[0] as f32;
    let height = source_uv_rect.height() * source_size[1] as f32;
    if width <= 0.0 || height <= 0.0 {
        return Err(());
    }
    Ok([min_x, min_y, width, height])
}

pub(crate) fn anime_source_region_dimensions(
    source_size: [u32; 2],
    source_uv_rect: egui::Rect,
) -> Option<[u32; 2]> {
    let region = source_region_pixels(source_size, source_uv_rect).ok()?;
    Some([region[2].ceil() as u32, region[3].ceil() as u32])
}

fn anime_source_region_within_limit(
    source_size: [u32; 2],
    source_region: LanczosSourceRegionKey,
    limit: AnimeUpscaleSourceLimit,
) -> bool {
    let Some(max_long_edge) = limit.max_long_edge() else {
        return true;
    };
    let dimensions = match source_region.visible_rect() {
        Some(rect) => anime_source_region_dimensions(source_size, rect),
        None => Some(source_size),
    };
    dimensions.is_some_and(|size| size[0].max(size[1]) <= max_long_edge)
}

fn scale_branch_with_anime_source_limit(
    branch: FullscreenPaintScaleBranch,
    source_size: [u32; 2],
    source_region: LanczosSourceRegionKey,
    limit: AnimeUpscaleSourceLimit,
) -> FullscreenPaintScaleBranch {
    if branch == FullscreenPaintScaleBranch::UpscaleAnime
        && !anime_source_region_within_limit(source_size, source_region, limit)
    {
        FullscreenPaintScaleBranch::UpscaleLanczos
    } else {
        branch
    }
}

fn target_pixels(size: [u32; 2]) -> u64 {
    u64::from(size[0]).saturating_mul(u64::from(size[1]))
}

fn upscale_target_within_limits(target_size: [u32; 2]) -> bool {
    target_size[0] <= MAX_UPSCALE_TARGET_DIMENSION
        && target_size[1] <= MAX_UPSCALE_TARGET_DIMENSION
        && target_pixels(target_size) <= MAX_UPSCALE_TARGET_PIXELS
}

fn lanczos_target_decision(
    scale_branch: FullscreenPaintScaleBranch,
    target_size: [u32; 2],
) -> LanczosTargetDecision {
    if matches!(
        scale_branch,
        FullscreenPaintScaleBranch::UpscaleLanczos
            | FullscreenPaintScaleBranch::UpscaleNis
            | FullscreenPaintScaleBranch::UpscaleAnime
            | FullscreenPaintScaleBranch::UpscalePixelArt
    ) && !upscale_target_within_limits(target_size)
    {
        LanczosTargetDecision::OriginalFallback
    } else {
        LanczosTargetDecision::Resample
    }
}

fn sample_range(
    source_len: u32,
    target_len: u32,
    target_index: u32,
    blur_factor: f32,
) -> (u32, u32) {
    let scale = target_len as f64 / source_len as f64;
    let stretch = (1.0 / scale).max(1.0) * f64::from(blur_factor.clamp(1.0, 1.3));
    let support = 3.0 * stretch;
    let center = (target_index as f64 + 0.5) / scale;
    let start =
        ((center - 0.5 - support).floor() as i64 + 1).clamp(0, i64::from(source_len)) as u32;
    let end = ((center - 0.5 + support).ceil() as i64).clamp(0, i64::from(source_len)) as u32;
    (start, end.max(start))
}

fn axis_fetch_count(source_len: u32, target_len: u32, blur_factor: f32) -> u64 {
    (0..target_len)
        .map(|index| {
            let (start, end) = sample_range(source_len, target_len, index, blur_factor);
            u64::from(end - start)
        })
        .sum()
}

fn region_sample_range(
    source_len: u32,
    target_len: u32,
    target_index: u32,
    region_start: f32,
    region_len: f32,
) -> (u32, u32) {
    let scale = target_len as f64 / f64::from(region_len);
    // Upscale intentionally keeps Lanczos3 at its native support radius. Unlike
    // downscale, the kernel must not be widened by 1 / scale.
    let center = f64::from(region_start) + (target_index as f64 + 0.5) / scale;
    let start = ((center - 0.5 - 3.0).floor() as i64 + 1).clamp(0, i64::from(source_len)) as u32;
    let end = ((center - 0.5 + 3.0).ceil() as i64).clamp(0, i64::from(source_len)) as u32;
    (start, end.max(start))
}

fn axis_fetch_count_region(
    source_len: u32,
    target_len: u32,
    region_start: f32,
    region_len: f32,
) -> u64 {
    (0..target_len)
        .map(|index| {
            let (start, end) =
                region_sample_range(source_len, target_len, index, region_start, region_len);
            u64::from(end - start)
        })
        .sum()
}

struct Lanczos3Resampler {
    bind_group_layout: wgpu::BindGroupLayout,
    vertical_pipeline: wgpu::RenderPipeline,
    horizontal_pipeline: wgpu::RenderPipeline,
    visible_upscale_vertical_pipeline: wgpu::RenderPipeline,
    visible_upscale_horizontal_pipeline: wgpu::RenderPipeline,
    nis_upscale_pipeline: wgpu::RenderPipeline,
    pixel_aa_pipeline: wgpu::RenderPipeline,
}

impl Lanczos3Resampler {
    fn new(device: &wgpu::Device) -> Self {
        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: None,
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        multisampled: false,
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: None,
            bind_group_layouts: &[&bind_group_layout],
            push_constant_ranges: &[],
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: None,
            source: wgpu::ShaderSource::Wgsl(Cow::Borrowed(LANCZOS3_SHADER)),
        });
        let visible_upscale_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: None,
            source: wgpu::ShaderSource::Wgsl(Cow::Borrowed(VISIBLE_UPSCALE_LANCZOS3_SHADER)),
        });
        let nis_upscale_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: None,
            source: wgpu::ShaderSource::Wgsl(Cow::Borrowed(NIS_UPSCALE_SHADER)),
        });
        let pixel_aa_upscale_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: None,
            source: wgpu::ShaderSource::Wgsl(Cow::Borrowed(PIXEL_AA_UPSCALE_SHADER)),
        });
        Self {
            vertical_pipeline: create_pipeline(
                device,
                &layout,
                &shader,
                "fs_vertical",
                wgpu::TextureFormat::Rgba16Float,
            ),
            horizontal_pipeline: create_pipeline(
                device,
                &layout,
                &shader,
                "fs_horizontal",
                wgpu::TextureFormat::Rgba8Unorm,
            ),
            visible_upscale_vertical_pipeline: create_pipeline(
                device,
                &layout,
                &visible_upscale_shader,
                "fs_vertical",
                wgpu::TextureFormat::Rgba16Float,
            ),
            visible_upscale_horizontal_pipeline: create_pipeline(
                device,
                &layout,
                &visible_upscale_shader,
                "fs_horizontal",
                wgpu::TextureFormat::Rgba8Unorm,
            ),
            nis_upscale_pipeline: create_pipeline(
                device,
                &layout,
                &nis_upscale_shader,
                "fs_nis",
                wgpu::TextureFormat::Rgba8Unorm,
            ),
            pixel_aa_pipeline: create_pipeline(
                device,
                &layout,
                &pixel_aa_upscale_shader,
                "fs_pixel_aa",
                wgpu::TextureFormat::Rgba8Unorm,
            ),
            bind_group_layout,
        }
    }

    fn prepare_job(
        &self,
        device: &wgpu::Device,
        source: &wgpu::Texture,
        plan: LanczosPlan,
    ) -> Result<LanczosJob, ()> {
        if source.format() != wgpu::TextureFormat::Rgba8Unorm
            || [source.width(), source.height()] != plan.source_size
        {
            return Err(());
        }
        let source_view = source.create_view(&wgpu::TextureViewDescriptor {
            base_mip_level: 0,
            mip_level_count: Some(1),
            ..Default::default()
        });
        let intermediate_texture = create_target_texture(
            device,
            [plan.source_size[0], plan.target_size[1]],
            wgpu::TextureFormat::Rgba16Float,
        );
        let intermediate_view = intermediate_texture.create_view(&Default::default());
        let output_texture =
            create_target_texture(device, plan.target_size, wgpu::TextureFormat::Rgba8Unorm);
        let output_view = output_texture.create_view(&Default::default());
        let vertical_uniform =
            resample_params_uniform(device, plan.target_size[1], plan.blur_factor);
        let horizontal_uniform =
            resample_params_uniform(device, plan.target_size[0], plan.blur_factor);
        let vertical_bind_group = create_bind_group(
            device,
            &self.bind_group_layout,
            &source_view,
            &vertical_uniform,
        );
        let horizontal_bind_group = create_bind_group(
            device,
            &self.bind_group_layout,
            &intermediate_view,
            &horizontal_uniform,
        );
        Ok(LanczosJob {
            _intermediate_texture: intermediate_texture,
            intermediate_view,
            output_texture,
            output_view,
            vertical_bind_group,
            horizontal_bind_group,
            _vertical_uniform: vertical_uniform,
            _horizontal_uniform: horizontal_uniform,
        })
    }

    fn encode(&self, encoder: &mut wgpu::CommandEncoder, job: &LanczosJob) {
        self.encode_pass(
            encoder,
            &job.intermediate_view,
            &self.vertical_pipeline,
            &job.vertical_bind_group,
        );
        self.encode_pass(
            encoder,
            &job.output_view,
            &self.horizontal_pipeline,
            &job.horizontal_bind_group,
        );
    }

    fn prepare_visible_upscale_job(
        &self,
        device: &wgpu::Device,
        source: &wgpu::Texture,
        plan: VisibleUpscalePlan,
    ) -> Result<LanczosJob, ()> {
        if source.format() != wgpu::TextureFormat::Rgba8Unorm
            || [source.width(), source.height()] != plan.source_size
        {
            return Err(());
        }
        let source_view = source.create_view(&wgpu::TextureViewDescriptor {
            base_mip_level: 0,
            mip_level_count: Some(1),
            ..Default::default()
        });
        let intermediate_texture = create_target_texture(
            device,
            [plan.intermediate_x_width, plan.target_size[1]],
            wgpu::TextureFormat::Rgba16Float,
        );
        let intermediate_view = intermediate_texture.create_view(&Default::default());
        let output_texture =
            create_target_texture(device, plan.target_size, wgpu::TextureFormat::Rgba8Unorm);
        let output_view = output_texture.create_view(&Default::default());
        let vertical_uniform = resample_region_params_uniform(
            device,
            plan.target_size[1],
            plan.source_region_px[1],
            plan.source_region_px[3],
            plan.intermediate_x_start,
        );
        let horizontal_uniform = resample_region_params_uniform(
            device,
            plan.target_size[0],
            plan.source_region_px[0] - plan.intermediate_x_start as f32,
            plan.source_region_px[2],
            0,
        );
        let vertical_bind_group = create_bind_group(
            device,
            &self.bind_group_layout,
            &source_view,
            &vertical_uniform,
        );
        let horizontal_bind_group = create_bind_group(
            device,
            &self.bind_group_layout,
            &intermediate_view,
            &horizontal_uniform,
        );
        Ok(LanczosJob {
            _intermediate_texture: intermediate_texture,
            intermediate_view,
            output_texture,
            output_view,
            vertical_bind_group,
            horizontal_bind_group,
            _vertical_uniform: vertical_uniform,
            _horizontal_uniform: horizontal_uniform,
        })
    }

    fn encode_visible_upscale(&self, encoder: &mut wgpu::CommandEncoder, job: &LanczosJob) {
        self.encode_pass(
            encoder,
            &job.intermediate_view,
            &self.visible_upscale_vertical_pipeline,
            &job.vertical_bind_group,
        );
        self.encode_pass(
            encoder,
            &job.output_view,
            &self.visible_upscale_horizontal_pipeline,
            &job.horizontal_bind_group,
        );
    }

    fn prepare_nis_upscale_job(
        &self,
        device: &wgpu::Device,
        source: &wgpu::Texture,
        plan: NisUpscalePlan,
    ) -> Result<NisJob, ()> {
        if source.format() != wgpu::TextureFormat::Rgba8Unorm
            || [source.width(), source.height()] != plan.source_size
        {
            return Err(());
        }
        let source_view = source.create_view(&wgpu::TextureViewDescriptor {
            base_mip_level: 0,
            mip_level_count: Some(1),
            ..Default::default()
        });
        let output_texture =
            create_target_texture(device, plan.target_size, wgpu::TextureFormat::Rgba8Unorm);
        let output_view = output_texture.create_view(&Default::default());
        let uniform = nis_params_uniform(device, plan);
        let bind_group = create_bind_group(device, &self.bind_group_layout, &source_view, &uniform);
        Ok(NisJob {
            output_texture,
            output_view,
            bind_group,
            _uniform: uniform,
        })
    }

    fn encode_nis_upscale(&self, encoder: &mut wgpu::CommandEncoder, job: &NisJob) {
        self.encode_pass(
            encoder,
            &job.output_view,
            &self.nis_upscale_pipeline,
            &job.bind_group,
        );
    }

    fn prepare_pixel_aa_job(
        &self,
        device: &wgpu::Device,
        source: &wgpu::Texture,
        plan: PixelArtUpscalePlan,
    ) -> Result<PixelAaJob, ()> {
        if source.format() != wgpu::TextureFormat::Rgba8Unorm
            || [source.width(), source.height()] != plan.source_size
        {
            return Err(());
        }
        let source_view = source.create_view(&wgpu::TextureViewDescriptor {
            base_mip_level: 0,
            mip_level_count: Some(1),
            ..Default::default()
        });
        let output_texture =
            create_target_texture(device, plan.target_size, wgpu::TextureFormat::Rgba8Unorm);
        let output_view = output_texture.create_view(&Default::default());
        let uniform = pixel_aa_params_uniform(device, plan);
        let bind_group = create_bind_group(device, &self.bind_group_layout, &source_view, &uniform);
        Ok(PixelAaJob {
            output_texture,
            output_view,
            bind_group,
            _uniform: uniform,
        })
    }

    fn encode_pixel_aa(&self, encoder: &mut wgpu::CommandEncoder, job: &PixelAaJob) {
        self.encode_pass(
            encoder,
            &job.output_view,
            &self.pixel_aa_pipeline,
            &job.bind_group,
        );
    }

    fn encode_pass(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        view: &wgpu::TextureView,
        pipeline: &wgpu::RenderPipeline,
        bind_group: &wgpu::BindGroup,
    ) {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: None,
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
        });
        pass.set_pipeline(pipeline);
        pass.set_bind_group(0, bind_group, &[]);
        pass.draw(0..3, 0..1);
    }
}

struct LanczosJob {
    _intermediate_texture: wgpu::Texture,
    intermediate_view: wgpu::TextureView,
    output_texture: wgpu::Texture,
    output_view: wgpu::TextureView,
    vertical_bind_group: wgpu::BindGroup,
    horizontal_bind_group: wgpu::BindGroup,
    _vertical_uniform: wgpu::Buffer,
    _horizontal_uniform: wgpu::Buffer,
}

struct NisJob {
    output_texture: wgpu::Texture,
    output_view: wgpu::TextureView,
    bind_group: wgpu::BindGroup,
    _uniform: wgpu::Buffer,
}

struct PixelAaJob {
    output_texture: wgpu::Texture,
    output_view: wgpu::TextureView,
    bind_group: wgpu::BindGroup,
    _uniform: wgpu::Buffer,
}

enum LanczosWorkJob {
    Full(LanczosJob),
    VisibleUpscale(LanczosJob),
    NisUpscale(NisJob),
    PixelArtUpscale(PixelAaJob),
    AnimeUpscale(Anime4kJob),
}

impl LanczosWorkJob {
    fn output_view(&self) -> &wgpu::TextureView {
        match self {
            Self::Full(job) | Self::VisibleUpscale(job) => &job.output_view,
            Self::NisUpscale(job) => &job.output_view,
            Self::PixelArtUpscale(job) => &job.output_view,
            Self::AnimeUpscale(job) => job.output_view(),
        }
    }

    fn into_output_texture(self) -> wgpu::Texture {
        match self {
            Self::Full(job) | Self::VisibleUpscale(job) => job.output_texture,
            Self::NisUpscale(job) => job.output_texture,
            Self::PixelArtUpscale(job) => job.output_texture,
            Self::AnimeUpscale(job) => job.into_output_texture(),
        }
    }
}

fn create_target_texture(
    device: &wgpu::Device,
    size: [u32; 2],
    format: wgpu::TextureFormat,
) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: None,
        size: wgpu::Extent3d {
            width: size[0],
            height: size[1],
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    })
}

fn resample_params_uniform(
    device: &wgpu::Device,
    target_len: u32,
    blur_factor: f32,
) -> wgpu::Buffer {
    let mut bytes = [0_u8; 16];
    bytes[..4].copy_from_slice(&target_len.to_ne_bytes());
    bytes[4..8].copy_from_slice(&blur_factor.to_ne_bytes());
    device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: None,
        contents: &bytes,
        usage: wgpu::BufferUsages::UNIFORM,
    })
}

fn resample_region_params_uniform(
    device: &wgpu::Device,
    target_len: u32,
    source_start: f32,
    source_len: f32,
    cross_start: u32,
) -> wgpu::Buffer {
    let mut bytes = [0_u8; 16];
    bytes[..4].copy_from_slice(&target_len.to_ne_bytes());
    bytes[4..8].copy_from_slice(&source_start.to_ne_bytes());
    bytes[8..12].copy_from_slice(&source_len.to_ne_bytes());
    bytes[12..16].copy_from_slice(&cross_start.to_ne_bytes());
    device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: None,
        contents: &bytes,
        usage: wgpu::BufferUsages::UNIFORM,
    })
}

fn nis_params_uniform(device: &wgpu::Device, plan: NisUpscalePlan) -> wgpu::Buffer {
    let mut bytes = [0_u8; 56];
    bytes[..4].copy_from_slice(&plan.target_size[0].to_ne_bytes());
    bytes[4..8].copy_from_slice(&plan.target_size[1].to_ne_bytes());
    bytes[8..12].copy_from_slice(&plan.source_size[0].to_ne_bytes());
    bytes[12..16].copy_from_slice(&plan.source_size[1].to_ne_bytes());
    for (offset, value) in plan.source_region_px.into_iter().enumerate() {
        let start = 16 + offset * 4;
        bytes[start..start + 4].copy_from_slice(&value.to_ne_bytes());
    }
    for (offset, value) in [1.0_f32, 0.0, 0.0, 1.0, 0.0, 0.0].into_iter().enumerate() {
        bytes[32 + offset * 4..36 + offset * 4].copy_from_slice(&value.to_ne_bytes());
    }
    device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: None,
        contents: &bytes,
        usage: wgpu::BufferUsages::UNIFORM,
    })
}

fn pixel_aa_params_uniform(device: &wgpu::Device, plan: PixelArtUpscalePlan) -> wgpu::Buffer {
    let mut bytes = [0_u8; 32];
    bytes[..4].copy_from_slice(&plan.target_size[0].to_ne_bytes());
    bytes[4..8].copy_from_slice(&plan.target_size[1].to_ne_bytes());
    bytes[8..12].copy_from_slice(&plan.source_size[0].to_ne_bytes());
    bytes[12..16].copy_from_slice(&plan.source_size[1].to_ne_bytes());
    for (offset, value) in plan.source_region_px.into_iter().enumerate() {
        let start = 16 + offset * 4;
        bytes[start..start + 4].copy_from_slice(&value.to_ne_bytes());
    }
    device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: None,
        contents: &bytes,
        usage: wgpu::BufferUsages::UNIFORM,
    })
}

fn create_bind_group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    source_view: &wgpu::TextureView,
    uniform: &wgpu::Buffer,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: None,
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(source_view),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: uniform.as_entire_binding(),
            },
        ],
    })
}

fn create_pipeline(
    device: &wgpu::Device,
    layout: &wgpu::PipelineLayout,
    shader: &wgpu::ShaderModule,
    fragment_entry: &str,
    format: wgpu::TextureFormat,
) -> wgpu::RenderPipeline {
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: None,
        layout: Some(layout),
        vertex: wgpu::VertexState {
            module: shader,
            entry_point: Some("vs_main"),
            compilation_options: Default::default(),
            buffers: &[],
        },
        primitive: Default::default(),
        depth_stencil: None,
        multisample: Default::default(),
        fragment: Some(wgpu::FragmentState {
            module: shader,
            entry_point: Some(fragment_entry),
            compilation_options: Default::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format,
                blend: None,
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        multiview: None,
        cache: None,
    })
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    #[test]
    fn scale_branches_match_dot_by_dot_boundary() {
        assert_eq!(
            fullscreen_paint_scale_branch(0.75, 1.0, PostFilter::None),
            FullscreenPaintScaleBranch::DownscaleLanczos
        );
        assert_eq!(
            fullscreen_paint_scale_branch(1.0, 1.0, PostFilter::None),
            FullscreenPaintScaleBranch::OriginalOneToOne
        );
        assert_eq!(
            fullscreen_paint_scale_branch(1.25, 1.0, PostFilter::None),
            FullscreenPaintScaleBranch::UpscaleLanczos
        );
        assert_eq!(
            fullscreen_paint_scale_branch(2.0, 1.0, PostFilter::None),
            FullscreenPaintScaleBranch::UpscaleLanczos
        );
        assert!(physical_scale_is_near_integer(2.0, 1.0));
        assert_eq!(
            fullscreen_paint_scale_branch(0.5, 2.0, PostFilter::None),
            FullscreenPaintScaleBranch::OriginalOneToOne
        );
        assert!(physical_scale_is_near_integer(0.5, 2.0));
    }

    #[test]
    fn nearest_and_effect_filters_only_bypass_upscale_lanczos() {
        assert_eq!(
            fullscreen_paint_scale_branch(2.0, 1.0, PostFilter::UpscaleSharp),
            FullscreenPaintScaleBranch::UpscaleNis
        );
        assert_eq!(
            fullscreen_paint_scale_branch(2.0, 1.0, PostFilter::UpscaleAnime),
            FullscreenPaintScaleBranch::UpscaleAnime
        );
        assert_eq!(
            fullscreen_paint_scale_branch(0.75, 1.0, PostFilter::UpscaleSharp),
            FullscreenPaintScaleBranch::DownscaleLanczos
        );
        assert_eq!(
            fullscreen_paint_scale_branch(1.0, 1.0, PostFilter::UpscaleSharp),
            FullscreenPaintScaleBranch::OriginalOneToOne
        );
        assert_eq!(
            fullscreen_paint_scale_branch(0.75, 1.0, PostFilter::UpscaleAnime),
            FullscreenPaintScaleBranch::DownscaleLanczos
        );
        assert_eq!(
            fullscreen_paint_scale_branch(1.0, 1.0, PostFilter::UpscaleAnime),
            FullscreenPaintScaleBranch::OriginalOneToOne
        );
        assert_eq!(
            fullscreen_paint_scale_branch(2.0, 1.0, PostFilter::UpscalePixelArt),
            FullscreenPaintScaleBranch::UpscalePixelArt
        );
        assert_eq!(
            fullscreen_paint_scale_branch(0.75, 1.0, PostFilter::UpscalePixelArt),
            FullscreenPaintScaleBranch::DownscaleLanczos
        );
        assert_eq!(
            fullscreen_paint_scale_branch(1.0, 1.0, PostFilter::UpscalePixelArt),
            FullscreenPaintScaleBranch::OriginalOneToOne
        );
        assert_eq!(
            fullscreen_paint_scale_branch(2.0, 1.0, PostFilter::Nearest),
            FullscreenPaintScaleBranch::OriginalUpscale
        );
        assert_eq!(
            fullscreen_paint_scale_branch(1.25, 1.0, PostFilter::Sepia),
            FullscreenPaintScaleBranch::OriginalUpscale
        );
        assert_eq!(
            fullscreen_paint_scale_branch(0.75, 1.0, PostFilter::Nearest),
            FullscreenPaintScaleBranch::DownscaleLanczos
        );
        assert_eq!(
            fullscreen_paint_scale_branch(0.75, 1.0, PostFilter::Sepia),
            FullscreenPaintScaleBranch::DownscaleLanczos
        );
    }

    #[test]
    fn near_one_never_enters_lanczos() {
        for scale in [1.0, 1.0 - 5.0e-5, 1.0 + 5.0e-5] {
            assert_eq!(
                fullscreen_paint_scale_branch(scale, 1.0, PostFilter::None),
                FullscreenPaintScaleBranch::OriginalOneToOne
            );
        }
    }

    /// リサンプラの出力サイズと、実際に貼る矩形の物理ピクセルサイズは一致しなければ
    /// ならない。食い違うと egui/wgpu が **もう一度バイリニアで貼り直す**ので、
    /// Lanczos の結果がボケる (backlog §1.0e、利用者報告 2026-08-29)。
    ///
    /// 実測: 2560x1440 100% で他ビューア 5 本のラプラシアン標準偏差 104.6 に対し
    /// mIV は 61.5 だった。カーネルではなくサイズと矩形の食い違いが原因。
    fn drawn_physical_size_rotated(
        source: [u32; 2],
        viewport: egui::Vec2,
        ppp: f32,
        rotation: crate::rotation_db::Rotation,
    ) -> [u32; 2] {
        let source_size = egui::vec2(source[0] as f32, source[1] as f32);
        let transform = crate::displayed_image_transform::DisplayedImageTransform::resolve(
            crate::displayed_image_transform::DisplayedImageTransformInput {
                pixel_fit: crate::displayed_image_transform::RectPixelFit::Texels,
                page_idx: 0,
                viewport_rect: egui::Rect::from_min_size(egui::Pos2::ZERO, viewport),
                source_size,
                texture_size: source_size,
                rotation,
                free_rotation_rad: 0.0,
                content_bbox: None,
                fit_mode: crate::settings::FullscreenFitMode::Page,
                fit_scale_limits:
                    crate::displayed_image_transform::FullscreenFitScaleLimits::default(),
                pixels_per_point: ppp,
                placement: crate::displayed_image_transform::ResolvedDisplayPlacement::Normal {
                    zoom_pan: None,
                },
            },
        )
        .expect("transform");
        let size = transform.full_image_rect.size() * ppp;
        [size.x.round() as u32, size.y.round() as u32]
    }

    fn drawn_physical_size(source: [u32; 2], viewport: egui::Vec2, ppp: f32) -> [u32; 2] {
        drawn_physical_size_rotated(source, viewport, ppp, crate::rotation_db::Rotation::None)
    }

    fn page_fit_logical_scale(source: [u32; 2], viewport: egui::Vec2) -> f32 {
        (viewport.x / source[0] as f32).min(viewport.y / source[1] as f32)
    }

    #[test]
    fn the_resampled_texture_is_exactly_the_size_it_is_drawn_at() {
        // 利用者が比較に使った 2 枚と、DPI 125% / 縦長ウィンドウを足した組み合わせ。
        let cases: &[([u32; 2], egui::Vec2, f32)] = &[
            ([1120, 1600], egui::vec2(2560.0, 1440.0), 1.0),
            ([4248, 6048], egui::vec2(2560.0, 1440.0), 1.0),
            ([2480, 3508], egui::vec2(1920.0, 1080.0), 1.0),
            ([1120, 1600], egui::vec2(2048.0, 1152.0), 1.25),
            ([3000, 2000], egui::vec2(1600.0, 1200.0), 1.0),
        ];
        for &(source, viewport, ppp) in cases {
            let logical_scale = page_fit_logical_scale(source, viewport);
            let physical = physical_scale(logical_scale, ppp);
            if physical >= 1.0 {
                continue;
            }
            let target = quantized_target_size(source, physical, TARGET_SIZE_QUANTUM);
            let drawn = drawn_physical_size(source, viewport, ppp);
            assert_eq!(
                target, drawn,
                "source={source:?} viewport={viewport:?} ppp={ppp}                  リサンプル先 {target:?} と描画サイズ {drawn:?} が違う (再度バイリニアが掛かる)"
            );
        }
    }

    /// `paint_geometry()` が返す **対** が、それだけで矛盾しないこと。
    ///
    /// detached の焼き込み 2 か所は、矩形を transform から取りながら倍率だけ自前の
    /// `min()` で計算していた (§1.0e、2026-08-30 レビュー指摘 1(b)(c))。対で返す入口に
    /// したので、**この 1 本がその対の不変条件を持つ**。
    ///
    /// 表示 trim を含む組み合わせを入れてあるのは、`full_image_rect` と `paint_rect` が
    /// 違う値になる唯一の状況だから。trim を返すように変えると落ちる。
    #[test]
    fn the_pair_paint_geometry_returns_agrees_with_itself() {
        let cases: &[([u32; 2], egui::Vec2, f32, Option<egui::Rect>)] = &[
            ([1120, 1600], egui::vec2(2560.0, 1440.0), 1.0, None),
            ([4248, 6048], egui::vec2(2560.0, 1440.0), 1.0, None),
            ([1249, 2272], egui::vec2(2560.0, 1440.0), 1.0, None),
            ([2480, 3508], egui::vec2(2048.0, 1152.0), 1.25, None),
            (
                [4248, 6048],
                egui::vec2(2560.0, 1440.0),
                1.0,
                Some(egui::Rect::from_min_max(
                    egui::pos2(0.05, 0.05),
                    egui::pos2(0.95, 0.95),
                )),
            ),
        ];
        let mut trimmed_cases = 0usize;
        for &(source, viewport, ppp, content_bbox) in cases {
            let source_size = egui::vec2(source[0] as f32, source[1] as f32);
            let transform = crate::displayed_image_transform::DisplayedImageTransform::resolve(
                crate::displayed_image_transform::DisplayedImageTransformInput {
                    pixel_fit: crate::displayed_image_transform::RectPixelFit::Texels,
                    page_idx: 0,
                    viewport_rect: egui::Rect::from_min_size(egui::Pos2::ZERO, viewport),
                    source_size,
                    texture_size: source_size,
                    rotation: crate::rotation_db::Rotation::None,
                    free_rotation_rad: 0.0,
                    content_bbox,
                    fit_mode: crate::settings::FullscreenFitMode::Page,
                    fit_scale_limits:
                        crate::displayed_image_transform::FullscreenFitScaleLimits::default(),
                    pixels_per_point: ppp,
                    placement: crate::displayed_image_transform::ResolvedDisplayPlacement::Normal {
                        zoom_pan: None,
                    },
                },
            )
            .expect("transform");
            let (rect, scale) = transform.paint_geometry();
            if content_bbox.is_some() {
                assert_ne!(
                    rect, transform.paint_rect,
                    "trim のある組み合わせで 2 つが同じでは、取り違えを検出できない"
                );
                trimmed_cases += 1;
            }
            let physical = physical_scale(scale, ppp);
            if physical >= 1.0 {
                continue;
            }
            let target = quantized_target_size(source, physical, TARGET_SIZE_QUANTUM);
            // **`round()` してから比べない。** 整数へ丸めてから突き合わせると、矩形が
            // 1439.44 物理 px でリサンプル先が 1439 のような「0.44px ずれているのに
            // 丸めれば一致する」状態を見逃す。その 0.44px でも GPU はバイリニアを掛ける。
            let drawn = [rect.width() * ppp, rect.height() * ppp];
            for axis in 0..2 {
                let gap = (drawn[axis] - target[axis] as f32).abs();
                assert!(
                    gap <= 1.0e-3,
                    "source={source:?} viewport={viewport:?} ppp={ppp} bbox={content_bbox:?}                      軸 {axis}: 倍率 {scale} が作る {} に対し、対で返した矩形は {} 物理 px (差 {gap})",
                    target[axis],
                    drawn[axis]
                );
            }
        }
        assert!(trimmed_cases > 0, "trim の組み合わせが 1 つも通っていない");
    }

    /// detached の静止画スナップショットは、**貼り先と倍率を対で**受け取らなければならない。
    ///
    /// 以前は `build_active_snapshot` が矩形だけ受け取り、`min(rect / tex)` で倍率を
    /// 組み直していた。渡ってくる矩形は既に物理ピクセルへ寄っているので、その逆算は
    /// 軸ごとの floor の分だけ小さい倍率になり、リサンプラの出力が子ウィンドウの貼り先より
    /// 1〜2 物理ピクセル短くなる (§1.0e、2026-08-30 に計測)。
    ///
    /// 丸めてから比べない。0.44px のずれでも GPU はもう一度バイリニアを掛ける。
    #[test]
    fn the_detached_snapshot_geometry_is_a_pair_the_resampler_can_reproduce() {
        // 利用者の 4K と、DPI 125% / 150%、横長・縦長を混ぜる。
        let cases: &[([u32; 2], egui::Vec2, f32)] = &[
            ([1120, 1600], egui::vec2(2560.0, 1440.0), 1.0),
            ([4248, 6048], egui::vec2(2560.0, 1440.0), 1.0),
            ([1249, 2272], egui::vec2(2560.0, 1440.0), 1.0),
            ([2480, 3508], egui::vec2(2048.0, 1152.0), 1.25),
            ([3000, 2000], egui::vec2(1600.0, 1200.0), 1.0),
            ([1612, 2418], egui::vec2(1707.0, 960.0), 1.5),
        ];
        let mut downscales = 0usize;
        for &(source, window, ppp) in cases {
            let tex = egui::vec2(source[0] as f32, source[1] as f32);
            let full_rect = egui::Rect::from_min_size(egui::Pos2::ZERO, window);
            let (rect, scale) = crate::ui_fullscreen::fs_image_draw_geometry_for_size(
                full_rect,
                tex,
                crate::rotation_db::Rotation::None,
                None,
                0.0,
                crate::settings::FullscreenFitMode::Page,
                crate::displayed_image_transform::FullscreenFitScaleLimits::default(),
                ppp,
                None,
            )
            .expect("geometry");
            let physical = scale * ppp;
            if physical >= 1.0 {
                continue;
            }
            downscales += 1;
            let target = quantized_target_size(source, physical, TARGET_SIZE_QUANTUM);
            let drawn = [rect.width() * ppp, rect.height() * ppp];
            for axis in 0..2 {
                let gap = (drawn[axis] - target[axis] as f32).abs();
                assert!(
                    gap <= 1.0e-3,
                    "source={source:?} window={window:?} ppp={ppp} 軸 {axis}:                      リサンプル先 {} に対し貼り先は {} 物理 px (差 {gap})",
                    target[axis],
                    drawn[axis]
                );
            }
        }
        assert!(downscales >= 4, "縮小のケースが足りない: {downscales}");
    }

    /// **transform を経由しない描画経路**でも、リサンプラの出力サイズと描画矩形の
    /// 物理サイズが一致すること。
    ///
    /// detached の keepalive backstop は `DisplayedImageTransform` を通らず、
    /// `scale = min(avail / tex)` から矩形を手で組んでリサンプラ出力を貼る。
    /// **元のバグがそこだけ残っていた** (2026-08-30 レビュー指摘 1(a))。矩形側は
    /// `snap_rect_to_physical_pixels`、リサンプラ側は `quantized_target_size` と
    /// 別の入口だが、どちらも同じ `physical_pixel_extent` に行き着く必要がある。
    ///
    /// 寄せをやめると落ちる。**この経路にはテストが 1 本も無かった。**
    #[test]
    fn the_backstop_rect_matches_the_resampler_even_without_a_transform() {
        let cases: &[([u32; 2], egui::Vec2, f32)] = &[
            ([1612, 2418], egui::vec2(2560.0, 1440.0), 1.0),
            ([1249, 2272], egui::vec2(2560.0, 1440.0), 1.0),
            ([4248, 6048], egui::vec2(1707.0, 960.0), 1.5),
            ([2480, 3508], egui::vec2(1280.0, 720.0), 1.25),
            ([3000, 2000], egui::vec2(1600.0, 1200.0), 1.0),
        ];
        for &(source, avail, ppp) in cases {
            let tex = egui::vec2(source[0] as f32, source[1] as f32);
            // backstop と同じ組み方。
            let scale = (avail.x / tex.x).min(avail.y / tex.y);
            let physical = physical_scale(scale, ppp);
            if physical >= 1.0 {
                continue;
            }
            let rect = crate::displayed_image_transform::snap_rect_to_physical_pixels(
                egui::Rect::from_center_size(egui::Pos2::ZERO, tex * scale),
                tex,
                scale,
                ppp,
            );
            let drawn = [
                (rect.width() * ppp).round() as u32,
                (rect.height() * ppp).round() as u32,
            ];
            let target = quantized_target_size(source, physical, TARGET_SIZE_QUANTUM);
            assert_eq!(
                target, drawn,
                "source={source:?} avail={avail:?} ppp={ppp}                  リサンプル先 {target:?} と backstop の描画サイズ {drawn:?} が違う"
            );
        }
    }

    /// 回転していても一致すること。回転後の表示サイズから倍率もサイズも決まるので、
    /// 片方だけ回転前の辺を見ていると 90 度で崩れる。
    #[test]
    fn the_resampled_texture_matches_the_drawn_size_when_rotated() {
        let source = [4248u32, 6048u32];
        let viewport = egui::vec2(2560.0, 1440.0);
        for rotation in [
            crate::rotation_db::Rotation::Cw90,
            crate::rotation_db::Rotation::Cw180,
            crate::rotation_db::Rotation::Cw270,
        ] {
            let rotated = match rotation {
                crate::rotation_db::Rotation::Cw90 | crate::rotation_db::Rotation::Cw270 => {
                    [source[1], source[0]]
                }
                _ => source,
            };
            let logical_scale = page_fit_logical_scale(rotated, viewport);
            let physical = physical_scale(logical_scale, 1.0);
            assert!(physical < 1.0, "この組み合わせは縮小のはず");
            let target = quantized_target_size(rotated, physical, TARGET_SIZE_QUANTUM);
            let drawn = drawn_physical_size_rotated(source, viewport, 1.0, rotation);
            assert_eq!(target, drawn, "rotation={rotation:?}");
        }
    }

    /// 端数が本物の倍率では切り捨てる。丸め上げると矩形が 1px はみ出す。
    #[test]
    fn a_real_fraction_still_truncates() {
        // 2480 * (1080/3508) = 763.54... 整数から遠いので 763。
        let scale = 1080.0f32 / 3508.0;
        assert_eq!(
            quantized_target_size([2480, 3508], scale, TARGET_SIZE_QUANTUM),
            [763, 1080]
        );
    }

    /// f32 の倍率が持つ誤差だけは吸収する。ここが floor に戻ると全面がボケる。
    #[test]
    fn the_f32_scale_error_does_not_shrink_the_target() {
        let scale = 1440.0f32 / 1600.0;
        assert!(
            (f64::from(1600.0f32) * f64::from(scale)) < 1440.0,
            "f32 の 1440/1600 は 1440 をわずかに下回るはず (この前提が崩れたら不要な test)"
        );
        assert_eq!(
            quantized_target_size([1120, 1600], scale, TARGET_SIZE_QUANTUM),
            [1008, 1440]
        );
    }

    #[test]
    fn spread_pages_choose_targets_independently() {
        let left = quantized_target_size([1200, 1800], 0.5, TARGET_SIZE_QUANTUM);
        let right = quantized_target_size([1000, 1600], 0.625, TARGET_SIZE_QUANTUM);
        assert_eq!(left, [600, 900]);
        assert_eq!(right, [625, 1000]);
    }

    #[test]
    fn exact_quantum_matches_cpu_reference_dimensions() {
        assert_eq!(
            quantized_target_size([2480, 3508], 0.63, TARGET_SIZE_QUANTUM),
            [1562, 2210]
        );
        assert_eq!(
            quantized_target_size([2480, 3508], 0.41, TARGET_SIZE_QUANTUM),
            [1016, 1438]
        );
    }

    #[test]
    fn upscale_targets_include_integer_and_fractional_scales() {
        assert_eq!(
            quantized_target_size([1200, 800], 2.0, TARGET_SIZE_QUANTUM),
            [2400, 1600]
        );
        assert_eq!(
            quantized_target_size([1200, 800], 1.25, TARGET_SIZE_QUANTUM),
            [1500, 1000]
        );
    }

    /// 貼り先を決めた側が持ってきたテクセル数を、この経路は**曲げない**。
    fn region_request(uv: egui::Rect, target_size: [u32; 2]) -> VisibleRegionRequest {
        VisibleRegionRequest {
            source_uv_rect: uv,
            target_size,
        }
    }

    /// 全面表示の要求。貼り先は寄せた全体矩形そのものなので、軸ごとに
    /// [`physical_pixel_extent`] を通った値になる。
    fn full_region_request(source: [u32; 2], physical_scale: f32) -> VisibleRegionRequest {
        VisibleRegionRequest::full([
            physical_pixel_extent(source[0] as f32, physical_scale),
            physical_pixel_extent(source[1] as f32, physical_scale),
        ])
    }

    /// 可視領域の出力テクセル数は**要求から**来る。倍率から出し直さない。
    ///
    /// 以前はここで「元領域の長さ × 倍率」を計算していたが、部分矩形ではそれが貼り先と
    /// 最大 1px 食い違う。倍率はスカラなので、全体矩形を画素へ寄せた端数を持てない
    /// (backlog §1.161)。倍率が 2 でも 4 でも、要求が同じなら答えは同じになる。
    #[test]
    fn the_visible_upscale_target_comes_from_the_request_not_the_scale() {
        let source = [5184, 3888];
        let at_two = egui::Rect::from_min_size(
            egui::pos2(0.10, 0.20),
            egui::vec2(3840.0 / (5184.0 * 2.0), 2160.0 / (3888.0 * 2.0)),
        );
        let at_four = egui::Rect::from_min_size(
            egui::pos2(0.30, 0.40),
            egui::vec2(3840.0 / (5184.0 * 4.0), 2160.0 / (3888.0 * 4.0)),
        );
        let (target_two, region_two) = target_and_source_region_for_branch(
            source,
            2.0,
            FullscreenPaintScaleBranch::UpscaleLanczos,
            Some(region_request(at_two, [3840, 2160])),
        )
        .unwrap();
        let (target_four, region_four) = target_and_source_region_for_branch(
            source,
            4.0,
            FullscreenPaintScaleBranch::UpscaleLanczos,
            Some(region_request(at_four, [3840, 2160])),
        )
        .unwrap();
        assert_eq!(target_two, [3840, 2160]);
        assert_eq!(target_four, [3840, 2160]);
        assert_ne!(region_two, region_four);
    }

    #[test]
    fn selected_upscalers_share_lanczos_target_and_source_region() {
        let source = [5184, 3888];
        let visible = egui::Rect::from_min_max(egui::pos2(0.125, 0.25), egui::pos2(0.625, 0.75));
        let visible = region_request(visible, [6480, 4860]);
        let lanczos = target_and_source_region_for_branch(
            source,
            2.5,
            FullscreenPaintScaleBranch::UpscaleLanczos,
            Some(visible),
        );
        let nis = target_and_source_region_for_branch(
            source,
            2.5,
            FullscreenPaintScaleBranch::UpscaleNis,
            Some(visible),
        );
        assert_eq!(nis, lanczos);
        let anime = target_and_source_region_for_branch(
            source,
            2.5,
            FullscreenPaintScaleBranch::UpscaleAnime,
            Some(visible),
        );
        assert_eq!(anime, lanczos);
        let pixel_art = target_and_source_region_for_branch(
            source,
            2.5,
            FullscreenPaintScaleBranch::UpscalePixelArt,
            Some(visible),
        );
        assert_eq!(pixel_art, lanczos);
    }

    #[test]
    fn anime_source_long_edge_limit_allows_boundary_and_falls_back_above_it() {
        let exact = LanczosSourceRegionKey::from_visible_rect(egui::Rect::from_min_max(
            egui::pos2(0.0, 0.0),
            egui::pos2(0.5, 1.0),
        ));
        assert!(anime_source_region_within_limit(
            [4096, 4096],
            exact,
            AnimeUpscaleSourceLimit::Px4096,
        ));
        let over = LanczosSourceRegionKey::from_visible_rect(egui::Rect::from_min_max(
            egui::pos2(0.0, 0.0),
            egui::pos2(1.0, 1.0),
        ));
        assert!(!anime_source_region_within_limit(
            [4097, 2048],
            over,
            AnimeUpscaleSourceLimit::Px4096,
        ));
        assert_eq!(
            scale_branch_with_anime_source_limit(
                FullscreenPaintScaleBranch::UpscaleAnime,
                [4097, 2048],
                over,
                AnimeUpscaleSourceLimit::Px4096,
            ),
            FullscreenPaintScaleBranch::UpscaleLanczos,
        );
        assert_eq!(
            scale_branch_with_anime_source_limit(
                FullscreenPaintScaleBranch::UpscaleAnime,
                [4096, 4096],
                exact,
                AnimeUpscaleSourceLimit::Px4096,
            ),
            FullscreenPaintScaleBranch::UpscaleAnime,
        );
        assert!(anime_source_region_within_limit(
            [10000, 10000],
            over,
            AnimeUpscaleSourceLimit::Unlimited,
        ));
    }

    #[test]
    fn downscale_target_ignores_visible_region_and_keeps_full_source_math() {
        let (target, region) = target_and_source_region_for_branch(
            [2480, 3508],
            0.41,
            FullscreenPaintScaleBranch::DownscaleLanczos,
            // 要求は縮小では読まれない。読んでいれば 1x1 が漏れて落ちる。
            Some(region_request(
                egui::Rect::from_min_max(egui::pos2(0.2, 0.3), egui::pos2(0.4, 0.5)),
                [1, 1],
            )),
        )
        .unwrap();
        assert_eq!(
            target,
            quantized_target_size([2480, 3508], 0.41, TARGET_SIZE_QUANTUM)
        );
        assert_eq!(region, LanczosSourceRegionKey::Full);
    }

    #[test]
    fn fully_visible_upscale_degenerates_to_the_previous_full_image_target() {
        let source = [1000, 1600];
        let (target, region) = target_and_source_region_for_branch(
            source,
            1.125,
            FullscreenPaintScaleBranch::UpscaleLanczos,
            Some(full_region_request(source, 1.125)),
        )
        .unwrap();
        assert_eq!(
            target,
            quantized_target_size(source, 1.125, TARGET_SIZE_QUANTUM)
        );
        assert!(matches!(region, LanczosSourceRegionKey::Visible { .. }));
    }

    #[test]
    fn height_matched_spread_keeps_full_page_targets_when_both_pages_are_visible() {
        let left = target_and_source_region_for_branch(
            [1200, 1800],
            0.75,
            FullscreenPaintScaleBranch::DownscaleLanczos,
            Some(full_region_request([1200, 1800], 0.75)),
        )
        .unwrap();
        let right = target_and_source_region_for_branch(
            [1000, 1600],
            1.125,
            FullscreenPaintScaleBranch::UpscaleLanczos,
            Some(full_region_request([1000, 1600], 1.125)),
        )
        .unwrap();
        assert_eq!(left.0, [900, 1350]);
        assert_eq!(right.0, [1125, 1800]);
    }

    #[test]
    fn upscale_limits_fall_back_to_original_before_allocating_oversized_targets() {
        assert!(upscale_target_within_limits([4096, 4096]));
        assert!(upscale_target_within_limits([8192, 2048]));
        assert!(!upscale_target_within_limits([4097, 4096]));
        assert!(!upscale_target_within_limits([8193, 1]));
        assert_eq!(
            lanczos_target_decision(FullscreenPaintScaleBranch::UpscaleLanczos, [4097, 4096]),
            LanczosTargetDecision::OriginalFallback
        );
        assert_eq!(
            lanczos_target_decision(FullscreenPaintScaleBranch::UpscaleLanczos, [8193, 1]),
            LanczosTargetDecision::OriginalFallback
        );
        assert_eq!(
            lanczos_target_decision(FullscreenPaintScaleBranch::UpscaleNis, [4097, 4096]),
            LanczosTargetDecision::OriginalFallback
        );
        assert_eq!(
            lanczos_target_decision(FullscreenPaintScaleBranch::UpscalePixelArt, [4097, 4096]),
            LanczosTargetDecision::OriginalFallback
        );
        assert_eq!(
            lanczos_target_decision(FullscreenPaintScaleBranch::DownscaleLanczos, [8192, 8192]),
            LanczosTargetDecision::Resample,
            "the new upscale guard must not change the existing downscale path"
        );
        let event = upscale_limit_fallback_event(
            [4096, 4096],
            [4097, 4096],
            FullscreenPaintScaleBranch::UpscaleNis,
        );
        let LanczosPerfEvent::UpscaleLimitFallback(stats) = event else {
            panic!("NIS limit fallback must emit the upscale fallback event");
        };
        assert_eq!(stats.scale_branch, FullscreenPaintScaleBranch::UpscaleNis);
        assert_eq!(stats.target_size, [4097, 4096]);
    }

    #[test]
    fn fully_visible_nis_uses_the_nis_work_plan() {
        let key = LanczosCacheKey {
            page_idx: 0,
            source_texture_id: egui::TextureId::Managed(1),
            generation: FullscreenPaintSourceGeneration { items: 1, input: 1 },
            target_size: [3840, 2160],
            smoothing_percent: 0,
            scale_branch: FullscreenPaintScaleBranch::UpscaleNis,
            source_region: LanczosSourceRegionKey::Full,
        };
        let plan = work_plan_for_key([1920, 1080], key).unwrap();
        let LanczosWorkPlan::NisUpscale(plan) = plan else {
            panic!("a fully visible NIS upscale must not use a Lanczos work plan");
        };
        assert_eq!(plan.source_region_px, [0.0, 0.0, 1920.0, 1080.0]);
        assert_eq!(plan.texture_fetches, 3840 * 2160 * 36);
    }

    #[test]
    fn fully_visible_pixel_art_uses_four_tap_work_plan() {
        let key = LanczosCacheKey {
            page_idx: 0,
            source_texture_id: egui::TextureId::Managed(3),
            generation: FullscreenPaintSourceGeneration { items: 1, input: 1 },
            target_size: [3840, 2160],
            smoothing_percent: 0,
            scale_branch: FullscreenPaintScaleBranch::UpscalePixelArt,
            source_region: LanczosSourceRegionKey::Full,
        };
        let plan = work_plan_for_key([1920, 1080], key).unwrap();
        let LanczosWorkPlan::PixelArtUpscale(plan) = plan else {
            panic!("a fully visible pixel-art upscale must use its dedicated work plan");
        };
        assert_eq!(plan.source_region_px, [0.0, 0.0, 1920.0, 1080.0]);
        assert_eq!(plan.texture_fetches, 3840 * 2160 * 4);
    }

    #[test]
    fn anime_work_plan_keeps_all_intermediates_at_expanded_source_resolution() {
        let visible = egui::Rect::from_min_max(egui::pos2(0.25, 0.25), egui::pos2(0.75, 0.75));
        let key = LanczosCacheKey {
            page_idx: 0,
            source_texture_id: egui::TextureId::Managed(2),
            generation: FullscreenPaintSourceGeneration { items: 1, input: 1 },
            target_size: [2048, 2048],
            smoothing_percent: 0,
            scale_branch: FullscreenPaintScaleBranch::UpscaleAnime,
            source_region: LanczosSourceRegionKey::from_visible_rect(visible),
        };
        let LanczosWorkPlan::AnimeUpscale(plan) = work_plan_for_key([2048, 2048], key).unwrap()
        else {
            panic!("anime upscale must use its dedicated work plan");
        };
        assert_eq!(plan.source_region_px, [512.0, 512.0, 1024.0, 1024.0]);
        assert_eq!(plan.process_origin, [504, 504]);
        assert_eq!(plan.process_size, [1040, 1040]);
        assert!(plan.texture_fetches > 0);
    }

    #[test]
    fn source_generation_is_cache_identity() {
        let base = LanczosCacheKey {
            page_idx: 3,
            source_texture_id: egui::TextureId::Managed(7),
            generation: FullscreenPaintSourceGeneration { items: 4, input: 8 },
            target_size: [640, 480],
            smoothing_percent: 0,
            scale_branch: FullscreenPaintScaleBranch::DownscaleLanczos,
            source_region: LanczosSourceRegionKey::Full,
        };
        let changed = LanczosCacheKey {
            generation: FullscreenPaintSourceGeneration { items: 4, input: 9 },
            ..base
        };
        assert_ne!(base, changed);
        let smoothing_changed = LanczosCacheKey {
            smoothing_percent: 50,
            ..base
        };
        assert_ne!(base, smoothing_changed);
        let branch_changed = LanczosCacheKey {
            scale_branch: FullscreenPaintScaleBranch::UpscaleLanczos,
            ..base
        };
        assert_ne!(base, branch_changed);
        let nis_branch = LanczosCacheKey {
            scale_branch: FullscreenPaintScaleBranch::UpscaleNis,
            ..base
        };
        assert_ne!(branch_changed, nis_branch);
        let anime_branch = LanczosCacheKey {
            scale_branch: FullscreenPaintScaleBranch::UpscaleAnime,
            ..base
        };
        assert_ne!(branch_changed, anime_branch);
        assert_ne!(nis_branch, anime_branch);
        let pixel_art_branch = LanczosCacheKey {
            scale_branch: FullscreenPaintScaleBranch::UpscalePixelArt,
            ..base
        };
        assert_ne!(branch_changed, pixel_art_branch);
        assert_ne!(nis_branch, pixel_art_branch);
        assert_ne!(anime_branch, pixel_art_branch);
        let lanczos_fallback = LanczosFallbackSourceKey {
            page_idx: base.page_idx,
            source_texture_id: base.source_texture_id,
            generation: base.generation,
            scale_branch: FullscreenPaintScaleBranch::UpscaleLanczos,
        };
        let nis_fallback = LanczosFallbackSourceKey {
            scale_branch: FullscreenPaintScaleBranch::UpscaleNis,
            ..lanczos_fallback
        };
        assert_ne!(lanczos_fallback, nis_fallback);
        let anime_fallback = LanczosFallbackSourceKey {
            scale_branch: FullscreenPaintScaleBranch::UpscaleAnime,
            ..lanczos_fallback
        };
        assert_ne!(nis_fallback, anime_fallback);
        let pixel_art_fallback = LanczosFallbackSourceKey {
            scale_branch: FullscreenPaintScaleBranch::UpscalePixelArt,
            ..lanczos_fallback
        };
        assert_ne!(nis_fallback, pixel_art_fallback);
        assert_ne!(anime_fallback, pixel_art_fallback);
        let source_region_changed = LanczosCacheKey {
            source_region: LanczosSourceRegionKey::from_visible_rect(egui::Rect::from_min_max(
                egui::pos2(0.0, 0.0),
                egui::pos2(0.5, 0.5),
            )),
            ..base
        };
        assert_ne!(base, source_region_changed);
        assert!(
            same_source_target_family(base, source_region_changed),
            "panning entries must share the two-entry LRU family"
        );
        assert!(
            !same_source_target_family(base, branch_changed),
            "upscale panning must not evict a downscale entry"
        );
    }

    struct CountingReleaser(AtomicUsize);

    impl NativeTextureIdReleaser for CountingReleaser {
        fn free_texture(&self, _texture_id: egui::TextureId) {
            self.0.fetch_add(1, Ordering::Relaxed);
        }
    }

    #[test]
    fn native_texture_id_lease_frees_once() {
        let releaser = Arc::new(CountingReleaser(AtomicUsize::new(0)));
        let lease = Arc::new(NativeTextureIdLease {
            texture_id: egui::TextureId::User(42),
            releaser: releaser.clone(),
        });
        let snapshot_owner = Arc::clone(&lease);
        drop(lease);
        assert_eq!(releaser.0.load(Ordering::Relaxed), 0);
        drop(snapshot_owner);
        assert_eq!(releaser.0.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn product_plan_is_direct_level_zero_work() {
        let plan = LanczosPlan::new([2480, 3508], [620, 877], 0).unwrap();
        assert_eq!(plan.source_size, [2480, 3508]);
        assert_eq!(plan.target_size, [620, 877]);
        assert_eq!(plan.smoothing_percent, 0);
        assert!((plan.blur_factor - 1.0).abs() < f32::EPSILON);
        assert!(plan.texture_fetches > 0);
    }

    #[test]
    fn product_plan_accepts_upscale_targets_without_widening_support() {
        let plan = LanczosPlan::new([1920, 1080], [3840, 2160], 0).unwrap();
        assert_eq!(plan.source_size, [1920, 1080]);
        assert_eq!(plan.target_size, [3840, 2160]);
        assert!((plan.blur_factor - 1.0).abs() < f32::EPSILON);
        assert_eq!(
            sample_range(1920, 3840, 2000, plan.blur_factor),
            (997, 1003)
        );
        assert!(plan.texture_fetches > 0);
    }

    #[test]
    fn pixel_aa_axis_weight_matches_bilinear_at_one_to_one() {
        for frac in [0.0, 0.1, 0.25, 0.5, 0.75, 0.9, 1.0] {
            assert!((pixel_aa_axis_weight(frac, 1.0) - frac).abs() < 1.0e-6);
        }
    }

    #[test]
    fn pixel_aa_axis_weight_converges_to_nearest_at_high_scale() {
        assert_eq!(pixel_aa_axis_weight(0.49, 1.0e-8), 0.0);
        assert_eq!(pixel_aa_axis_weight(0.51, 1.0e-8), 1.0);
    }

    #[test]
    fn pixel_aa_axis_weight_is_monotonic_and_bounded() {
        let mut previous = 0.0;
        for index in 0..=100 {
            let frac = index as f32 / 100.0;
            let weight = pixel_aa_axis_weight(frac, 0.37);
            assert!((0.0..=1.0).contains(&weight));
            assert!(weight >= previous);
            previous = weight;
        }
    }

    #[test]
    fn pixel_aa_four_x_transition_band_is_one_output_pixel_wide() {
        assert_eq!(pixel_aa_axis_weight(0.374, 0.25), 0.0);
        assert_eq!(pixel_aa_axis_weight(0.375, 0.25), 0.0);
        assert!((pixel_aa_axis_weight(0.5, 0.25) - 0.5).abs() < 1.0e-6);
        assert_eq!(pixel_aa_axis_weight(0.625, 0.25), 1.0);
        assert_eq!(pixel_aa_axis_weight(0.626, 0.25), 1.0);
    }

    #[test]
    fn visible_upscale_plan_crops_the_intermediate_but_keeps_lanczos_support() {
        let visible = egui::Rect::from_min_max(egui::pos2(0.25, 0.20), egui::pos2(0.50, 0.60));
        let plan = VisibleUpscalePlan::new([4000, 3000], [2000, 2400], visible).unwrap();
        for (actual, expected) in plan
            .source_region_px
            .into_iter()
            .zip([1000.0, 600.0, 1000.0, 1200.0])
        {
            assert!((actual - expected).abs() < 0.001);
        }
        assert!(plan.intermediate_x_start <= 997);
        assert!(plan.intermediate_x_start + plan.intermediate_x_width >= 2003);
        assert!(plan.intermediate_x_width < 4000);
        assert!(plan.texture_fetches > 0);
    }

    #[test]
    fn smoothing_change_invalidates_cache_identity_once() {
        let mut cache = GpuLanczosCache::default();
        assert!(!cache.sync_smoothing_percent(0));
        assert!(cache.sync_smoothing_percent(50));
        assert_eq!(cache.active_smoothing_percent, 50);
        assert!(!cache.sync_smoothing_percent(50));
        assert!(cache.sync_smoothing_percent(100));
        assert_eq!(cache.active_smoothing_percent, 100);
    }

    #[test]
    fn smoothing_expands_sample_bounds_and_fetch_estimate() {
        let standard = LanczosPlan::new([2480, 3508], [1562, 2210], 0).unwrap();
        let smooth = LanczosPlan::new([2480, 3508], [1562, 2210], 100).unwrap();
        assert_eq!(smooth.smoothing_percent, 100);
        assert!((smooth.blur_factor - 1.30).abs() < f32::EPSILON);
        assert!(smooth.texture_fetches > standard.texture_fetches);

        let standard_range = sample_range(2480, 1562, 700, standard.blur_factor);
        let smooth_range = sample_range(2480, 1562, 700, smooth.blur_factor);
        assert!(smooth_range.1 - smooth_range.0 > standard_range.1 - standard_range.0);
    }

    #[test]
    fn product_shader_validates() {
        assert!(LANCZOS3_SHADER.contains("blur_factor"));
        assert!(LANCZOS3_SHADER.contains("clamp(params.blur_factor, 1.0, 1.3)"));
        let module = wgpu::naga::front::wgsl::parse_str(LANCZOS3_SHADER).unwrap();
        wgpu::naga::valid::Validator::new(
            wgpu::naga::valid::ValidationFlags::all(),
            wgpu::naga::valid::Capabilities::all(),
        )
        .validate(&module)
        .unwrap();

        let visible_module =
            wgpu::naga::front::wgsl::parse_str(VISIBLE_UPSCALE_LANCZOS3_SHADER).unwrap();
        wgpu::naga::valid::Validator::new(
            wgpu::naga::valid::ValidationFlags::all(),
            wgpu::naga::valid::Capabilities::all(),
        )
        .validate(&visible_module)
        .unwrap();

        let nis_module = wgpu::naga::front::wgsl::parse_str(NIS_UPSCALE_SHADER).unwrap();
        wgpu::naga::valid::Validator::new(
            wgpu::naga::valid::ValidationFlags::all(),
            wgpu::naga::valid::Capabilities::all(),
        )
        .validate(&nis_module)
        .unwrap();

        let pixel_aa_module = wgpu::naga::front::wgsl::parse_str(PIXEL_AA_UPSCALE_SHADER).unwrap();
        wgpu::naga::valid::Validator::new(
            wgpu::naga::valid::ValidationFlags::all(),
            wgpu::naga::valid::Capabilities::all(),
        )
        .validate(&pixel_aa_module)
        .unwrap();

        for variant in crate::gpu_anime4k::Anime4kVariant::ALL {
            let anime_module = wgpu::naga::front::wgsl::parse_str(variant.shader())
                .unwrap_or_else(|error| panic!("{variant:?} WGSL parse failed: {error}"));
            wgpu::naga::valid::Validator::new(
                wgpu::naga::valid::ValidationFlags::all(),
                wgpu::naga::valid::Capabilities::all(),
            )
            .validate(&anime_module)
            .unwrap_or_else(|error| panic!("{variant:?} WGSL validation failed: {error}"));
        }
    }
}
