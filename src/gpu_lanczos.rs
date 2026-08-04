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

use crate::{adjustment::PostFilter, displayed_image_transform::physical_scale_is_near_integer};

const LANCZOS3_SHADER: &str = include_str!("gpu_lanczos_spike.wgsl");
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
    OriginalUpscale,
}

impl FullscreenPaintScaleBranch {
    fn uses_lanczos(self) -> bool {
        matches!(self, Self::DownscaleLanczos | Self::UpscaleLanczos)
    }

    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::DownscaleLanczos => "downscale",
            Self::OriginalOneToOne => "one_to_one",
            Self::UpscaleLanczos => "upscale",
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
    } else if post_filter == PostFilter::None {
        FullscreenPaintScaleBranch::UpscaleLanczos
    } else {
        FullscreenPaintScaleBranch::OriginalUpscale
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
}

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
struct LanczosFallbackSourceKey {
    page_idx: usize,
    source_texture_id: egui::TextureId,
    generation: FullscreenPaintSourceGeneration,
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
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct LanczosLimitFallbackStats {
    pub(crate) source_size: [u32; 2],
    pub(crate) target_size: [u32; 2],
    pub(crate) max_dimension: u32,
    pub(crate) max_pixels: u64,
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum LanczosPerfEvent {
    Generated(LanczosGenerationStats),
    UpscaleLimitFallback(LanczosLimitFallbackStats),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LanczosTargetDecision {
    Resample,
    OriginalFallback,
}

#[derive(Default)]
pub(crate) struct GpuLanczosCache {
    resampler: Option<Lanczos3Resampler>,
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
        post_filter: PostFilter,
        smoothing_percent: u32,
    ) -> (FullscreenPaintResource, Option<LanczosPerfEvent>) {
        let smoothing_percent =
            crate::settings::sanitize_downscale_smoothing_percent(smoothing_percent);
        self.sync_smoothing_percent(smoothing_percent);
        let scale_branch =
            fullscreen_paint_scale_branch(logical_scale, pixels_per_point, post_filter);
        if resource.resampleable_parts().is_none() || !scale_branch.uses_lanczos() {
            return (resource.original_resampleable(), None);
        }
        let Some(render_state) = render_state else {
            return (resource.original_resampleable(), None);
        };
        let (page_idx, source, generation) = resource.resampleable_parts().unwrap();
        let source_size = [source.size()[0] as u32, source.size()[1] as u32];
        let physical_scale = physical_scale(logical_scale, pixels_per_point);
        let target_size = quantized_target_size(source_size, physical_scale, TARGET_SIZE_QUANTUM);
        if target_size == source_size {
            return (resource.original_resampleable(), None);
        }
        let fallback_source = LanczosFallbackSourceKey {
            page_idx,
            source_texture_id: source.id(),
            generation,
        };
        if lanczos_target_decision(scale_branch, target_size)
            == LanczosTargetDecision::OriginalFallback
        {
            let first_for_source = self.limit_fallback_sources.insert(fallback_source);
            let event = first_for_source.then_some(LanczosPerfEvent::UpscaleLimitFallback(
                LanczosLimitFallbackStats {
                    source_size,
                    target_size,
                    max_dimension: MAX_UPSCALE_TARGET_DIMENSION,
                    max_pixels: MAX_UPSCALE_TARGET_PIXELS,
                },
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
        let plan = match LanczosPlan::new(
            [resource.size()[0] as u32, resource.size()[1] as u32],
            key.target_size,
            key.smoothing_percent,
        ) {
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
        plan: LanczosPlan,
    ) -> (FullscreenPaintResource, Option<LanczosPerfEvent>) {
        let started = Instant::now();
        let mut renderer = render_state.renderer.write();
        let Some(source_texture) = renderer
            .texture(&resource.source_texture_id())
            .and_then(|texture| texture.texture.as_ref())
        else {
            return (resource.original_resampleable(), None);
        };
        let resampler = self
            .resampler
            .get_or_insert_with(|| Lanczos3Resampler::new(&render_state.device));
        let job = match resampler.prepare_job(&render_state.device, source_texture, plan) {
            Ok(job) => job,
            Err(_) => return (resource.original_resampleable(), None),
        };
        let texture_id = renderer.register_native_texture(
            &render_state.device,
            &job.output_view,
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
        plan: LanczosPlan,
        job: LanczosJob,
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
        plan: LanczosPlan,
        job: LanczosJob,
        texture_id: egui::TextureId,
        started: Instant,
    ) -> (FullscreenPaintResource, Option<LanczosPerfEvent>) {
        let mut encoder = rs.device.create_command_encoder(&Default::default());
        self.resampler.as_ref().unwrap().encode(&mut encoder, &job);
        rs.queue.submit(Some(encoder.finish()));
        let output = Arc::new(LanczosOutput::new(
            texture_id,
            key.target_size,
            job.output_texture,
            rs.clone(),
        ));
        self.finish_generation(resource, key, plan, output, started)
    }

    fn finish_generation(
        &mut self,
        resource: &FullscreenPaintResource,
        key: LanczosCacheKey,
        plan: LanczosPlan,
        output: Arc<LanczosOutput>,
        started: Instant,
    ) -> (FullscreenPaintResource, Option<LanczosPerfEvent>) {
        self.regeneration_count = self.regeneration_count.wrapping_add(1);
        let stats = LanczosGenerationStats {
            source_size: plan.source_size,
            target_size: plan.target_size,
            smoothing_percent: plan.smoothing_percent,
            blur_factor: plan.blur_factor,
            texture_fetches: plan.texture_fetches,
            encode_submit_cpu_ms: started.elapsed().as_secs_f64() * 1000.0,
            regeneration_count: self.regeneration_count,
            scale_branch: key.scale_branch,
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
            .filter(|(key, _)| {
                key.page_idx == inserted.page_idx
                    && key.source_texture_id == inserted.source_texture_id
                    && key.generation == inserted.generation
                    && key.smoothing_percent == inserted.smoothing_percent
            })
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
                .filter(|(key, _)| key.scale_branch == FullscreenPaintScaleBranch::UpscaleLanczos)
                .fold(0_u64, |pixels, (key, _)| {
                    pixels.saturating_add(target_pixels(key.target_size))
                });
            if total <= MAX_CACHED_UPSCALE_PIXELS {
                break;
            }
            let Some(oldest) = self
                .entries
                .iter()
                .filter(|(key, _)| key.scale_branch == FullscreenPaintScaleBranch::UpscaleLanczos)
                .min_by_key(|(_, entry)| entry.last_used)
                .map(|(key, _)| *key)
            else {
                break;
            };
            self.entries.remove(&oldest);
        }
    }
}

pub(crate) struct LanczosOutput {
    texture_id_lease: NativeTextureIdLease,
    size: [u32; 2],
    _texture: wgpu::Texture,
}

impl LanczosOutput {
    fn new(
        texture_id: egui::TextureId,
        size: [u32; 2],
        texture: wgpu::Texture,
        render_state: egui_wgpu::RenderState,
    ) -> Self {
        Self {
            texture_id_lease: NativeTextureIdLease {
                texture_id,
                releaser: Arc::new(RendererTextureIdReleaser { render_state }),
            },
            size,
            _texture: texture,
        }
    }

    pub(crate) fn texture_id(&self) -> egui::TextureId {
        self.texture_id_lease.texture_id
    }

    pub(crate) fn size(&self) -> [usize; 2] {
        [self.size[0] as usize, self.size[1] as usize]
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

fn quantized_target_size(source_size: [u32; 2], scale: f32, quantum: u32) -> [u32; 2] {
    let quantum = quantum.max(1);
    let quantize = |source: u32| {
        let exact = ((source as f64 * f64::from(scale)).floor() as u32).max(1);
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
    if scale_branch == FullscreenPaintScaleBranch::UpscaleLanczos
        && !upscale_target_within_limits(target_size)
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

struct Lanczos3Resampler {
    bind_group_layout: wgpu::BindGroupLayout,
    vertical_pipeline: wgpu::RenderPipeline,
    horizontal_pipeline: wgpu::RenderPipeline,
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
            lanczos_target_decision(FullscreenPaintScaleBranch::DownscaleLanczos, [8192, 8192]),
            LanczosTargetDecision::Resample,
            "the new upscale guard must not change the existing downscale path"
        );
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
    }
}
