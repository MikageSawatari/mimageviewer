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
    material_display_basis::MaterialDisplayBasis,
    page_dims::DisplayOriginalDimensions,
    source_coverage::SourceCoverage,
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

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
pub(crate) enum FullscreenPaintSourceId {
    Page(usize),
    SimilarPreview(u64),
}

/// Keeps a native fullscreen output alive through egui tessellation and the renderer paint pass.
///
/// `NativeTextureIdLease::drop` unregisters the texture immediately, while egui renders shapes
/// only after the UI closure returns. A no-op callback in the same paint list therefore owns the
/// output for exactly as long as the batch that refers to its native texture id.
struct FullscreenPaintBatchLifetime {
    _owner: Arc<dyn Send + Sync>,
}

impl egui_wgpu::CallbackTrait for FullscreenPaintBatchLifetime {
    fn paint(
        &self,
        _info: egui::PaintCallbackInfo,
        _render_pass: &mut wgpu::RenderPass<'static>,
        _callback_resources: &egui_wgpu::CallbackResources,
    ) {
    }
}

pub(crate) fn retain_owner_for_fullscreen_paint_batch<T>(painter: &egui::Painter, owner: Arc<T>)
where
    T: Send + Sync + 'static,
{
    let owner: Arc<dyn Send + Sync> = owner;
    painter.add(egui::Shape::Callback(
        egui_wgpu::Callback::new_paint_callback(
            egui::Rect::ZERO,
            FullscreenPaintBatchLifetime { _owner: owner },
        ),
    ));
}

/// A partial preview's rounded sampler dimensions carry sampling density, not
/// the 100% display basis. Preserve the producer's uncropped normal raster;
/// canonical annotation coordinates may be larger than the GPU-clamped raster.
fn original_display_dimensions(
    size: [usize; 2],
    coverage: SourceCoverage,
) -> DisplayOriginalDimensions {
    if coverage.is_partial() {
        coverage.uncropped_raster_dimensions().unwrap()
    } else {
        DisplayOriginalDimensions::new(size[0] as u32, size[1] as u32)
    }
}

/// Geometry requests are expressed in full-source coordinates, whereas a
/// native resampler reads the immutable sampler allocation. Quantize only
/// after intersecting with the actual material, using that material's texels.
fn sampler_visible_region(
    coverage: SourceCoverage,
    request: Option<VisibleRegionRequest>,
) -> Option<VisibleRegionRequest> {
    let request = request?;
    let coverage = coverage.source_uv_rect()?;
    if coverage == egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)) {
        return Some(request);
    }
    let intersection = coverage.intersect(request.source_uv_rect);
    if !intersection.is_finite() || intersection.width() <= 0.0 || intersection.height() <= 0.0 {
        return None;
    }
    let source_uv_rect = egui::Rect::from_min_max(
        egui::pos2(
            (intersection.min.x - coverage.min.x) / coverage.width(),
            (intersection.min.y - coverage.min.y) / coverage.height(),
        ),
        egui::pos2(
            (intersection.max.x - coverage.min.x) / coverage.width(),
            (intersection.max.y - coverage.min.y) / coverage.height(),
        ),
    );
    let target_size = if intersection == request.source_uv_rect {
        request.target_size
    } else {
        // Compatibility for callers without the coverage-aware geometry
        // request. Ordinary paint prepares the already intersected pair.
        [
            (request.target_size[0] as f32 * intersection.width() / request.source_uv_rect.width())
                .round()
                .max(1.0) as u32,
            (request.target_size[1] as f32 * intersection.height()
                / request.source_uv_rect.height())
            .round()
            .max(1.0) as u32,
        ]
    };
    Some(VisibleRegionRequest {
        source_uv_rect,
        target_size,
    })
}

#[cfg(test)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct TestPaintProvenance {
    pub(crate) context: crate::app::ViewerContextId,
    pub(crate) items_generation: u64,
    pub(crate) item: String,
    pub(crate) page: usize,
}

#[derive(Clone)]
pub(crate) enum FullscreenPaintResource {
    Direct {
        source: egui::TextureHandle,
        coverage: SourceCoverage,
        display_basis: MaterialDisplayBasis,
        #[cfg(test)]
        test_paint_provenance: Option<TestPaintProvenance>,
        #[cfg(all(windows, feature = "test-script"))]
        test_script_content_proof: Option<crate::test_script::TestScriptContentProof>,
    },
    Resampleable {
        source_id: FullscreenPaintSourceId,
        source: egui::TextureHandle,
        generation: FullscreenPaintSourceGeneration,
        coverage: SourceCoverage,
        display_basis: MaterialDisplayBasis,
        #[cfg(test)]
        test_paint_provenance: Option<TestPaintProvenance>,
        #[cfg(all(windows, feature = "test-script"))]
        test_script_content_proof: Option<crate::test_script::TestScriptContentProof>,
    },
    Lanczos {
        source_id: FullscreenPaintSourceId,
        source: egui::TextureHandle,
        generation: FullscreenPaintSourceGeneration,
        smoothing_percent: u32,
        output: Arc<LanczosOutput>,
        coverage: SourceCoverage,
        display_basis: MaterialDisplayBasis,
        #[cfg(test)]
        test_paint_provenance: Option<TestPaintProvenance>,
        #[cfg(all(windows, feature = "test-script"))]
        test_script_content_proof: Option<crate::test_script::TestScriptContentProof>,
    },
}

impl FullscreenPaintResource {
    /// Capture the allocation actually painted, without retaining a native output's input.
    /// Display sources are immutable allocations: their sampling options do not
    /// change after load. epaint 0.33 metadata records those initial options.
    pub(crate) fn capture_presentation(
        &self,
        ctx: &egui::Context,
    ) -> FullscreenPresentationResource {
        let material = match self.lanczos_output() {
            Some(output) => PresentationMaterial::Native(Arc::clone(output)),
            None => {
                let handle = self.source_texture().clone();
                let actual_options = ctx
                    .tex_manager()
                    .read()
                    .meta(handle.id())
                    .expect("a live managed presentation texture has metadata")
                    .options;
                PresentationMaterial::Managed {
                    handle,
                    actual_options,
                }
            }
        };
        FullscreenPresentationResource {
            source_texture_id: self.source_texture_id(),
            logical_size: self.size(),
            source_id: self.source_id(),
            generation: self.source_generation(),
            material,
            coverage: self.source_coverage(),
            display_basis: self.display_basis(),
            #[cfg(test)]
            test_paint_provenance: self.test_paint_provenance().cloned(),
            #[cfg(all(windows, feature = "test-script"))]
            test_script_content_proof: self.test_script_content_proof().cloned(),
        }
    }

    pub(crate) fn direct(source: egui::TextureHandle) -> Self {
        let display_basis =
            MaterialDisplayBasis::from_sampler(source.size(), SourceCoverage::FullSource);
        Self::Direct {
            source,
            coverage: SourceCoverage::FullSource,
            display_basis,
            #[cfg(test)]
            test_paint_provenance: None,
            #[cfg(all(windows, feature = "test-script"))]
            test_script_content_proof: None,
        }
    }

    pub(crate) fn resampleable(
        page_idx: usize,
        source: egui::TextureHandle,
        generation: FullscreenPaintSourceGeneration,
    ) -> Self {
        Self::resampleable_for_source(FullscreenPaintSourceId::Page(page_idx), source, generation)
    }

    pub(crate) fn resampleable_similar_preview(
        resource_id: u64,
        source: egui::TextureHandle,
        generation: FullscreenPaintSourceGeneration,
    ) -> Self {
        Self::resampleable_for_source(
            FullscreenPaintSourceId::SimilarPreview(resource_id),
            source,
            generation,
        )
    }

    fn resampleable_for_source(
        source_id: FullscreenPaintSourceId,
        source: egui::TextureHandle,
        generation: FullscreenPaintSourceGeneration,
    ) -> Self {
        let display_basis =
            MaterialDisplayBasis::from_sampler(source.size(), SourceCoverage::FullSource);
        Self::Resampleable {
            source_id,
            source,
            generation,
            coverage: SourceCoverage::FullSource,
            display_basis,
            #[cfg(test)]
            test_paint_provenance: None,
            #[cfg(all(windows, feature = "test-script"))]
            test_script_content_proof: None,
        }
    }

    pub(crate) fn source_texture(&self) -> &egui::TextureHandle {
        match self {
            Self::Direct { source, .. }
            | Self::Resampleable { source, .. }
            | Self::Lanczos { source, .. } => source,
        }
    }

    pub(crate) fn source_coverage(&self) -> SourceCoverage {
        match self {
            Self::Direct { coverage, .. }
            | Self::Resampleable { coverage, .. }
            | Self::Lanczos { coverage, .. } => *coverage,
        }
    }

    /// Coverage is sampling metadata; changing it never re-resolves accepted fit values.
    /// Initial partial-material acceptance supplies its basis with `with_display_basis`.
    pub(crate) fn with_source_coverage(mut self, value: SourceCoverage) -> Self {
        match &mut self {
            Self::Direct { coverage, .. }
            | Self::Resampleable { coverage, .. }
            | Self::Lanczos { coverage, .. } => *coverage = value,
        }
        self
    }

    pub(crate) fn display_basis(&self) -> MaterialDisplayBasis {
        match self {
            Self::Direct { display_basis, .. }
            | Self::Resampleable { display_basis, .. }
            | Self::Lanczos { display_basis, .. } => *display_basis,
        }
    }

    pub(crate) fn with_display_basis(mut self, value: MaterialDisplayBasis) -> Self {
        match &mut self {
            Self::Direct { display_basis, .. }
            | Self::Resampleable { display_basis, .. }
            | Self::Lanczos { display_basis, .. } => *display_basis = value,
        }
        self
    }

    /// Full-image display basis, independent of a partial sampler's rounded
    /// dimensions and canonical annotation coordinates. PDF layout and RAW
    /// display policies remain separate inputs at their existing owners.
    pub(crate) fn original_display_dimensions(&self) -> DisplayOriginalDimensions {
        original_display_dimensions(self.size(), self.source_coverage())
    }

    pub(crate) fn full_source_size_vec2(&self) -> egui::Vec2 {
        self.original_display_dimensions().as_vec2()
    }

    pub(crate) fn anime_upscale_source_dimensions(
        &self,
        geometry: &crate::displayed_image_transform::DisplayedImageGeometry,
        pixels_per_point: f32,
    ) -> Option<[u32; 2]> {
        anime_material_source_dimensions(
            self.size(),
            self.source_coverage(),
            geometry,
            pixels_per_point,
        )
    }

    pub(crate) fn visible_region_request(
        &self,
        geometry: &crate::displayed_image_transform::DisplayedImageGeometry,
        clip_rect: egui::Rect,
    ) -> Option<VisibleRegionRequest> {
        geometry.visible_region_request_for_coverage(
            clip_rect,
            self.source_coverage().source_uv_rect()?,
        )
    }

    pub(crate) fn paint_geometry(
        &self,
        geometry: &crate::displayed_image_transform::DisplayedImageGeometry,
        painter: &egui::Painter,
        tint: egui::Color32,
    ) {
        if let Some(coverage) = self.source_coverage().source_uv_rect() {
            geometry.paint_material_texture(
                painter,
                self.paint_texture_id(),
                coverage,
                self.visible_source_uv_rect(),
                tint,
            );
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn paint_source_region(
        &self,
        painter: &egui::Painter,
        full_image_rect: egui::Rect,
        rotation: crate::rotation_db::Rotation,
        free_rotation_rad: f32,
        requested_source_rect: egui::Rect,
        tint: egui::Color32,
    ) {
        if let Some(coverage) = self.source_coverage().source_uv_rect() {
            crate::displayed_image_transform::paint_source_coverage_texture(
                painter,
                self.paint_texture_id(),
                full_image_rect,
                rotation,
                free_rotation_rad,
                coverage,
                requested_source_rect,
                self.visible_source_uv_rect(),
                tint,
            );
        }
    }

    pub(crate) fn paint_texture(
        &self,
        transform: &crate::displayed_image_transform::DisplayedImageTransform,
        painter: &egui::Painter,
        tint: egui::Color32,
    ) {
        if let Some(coverage) = self.source_coverage().source_uv_rect() {
            transform.paint_material_texture(
                painter,
                self.paint_texture_id(),
                coverage,
                self.visible_source_uv_rect(),
                tint,
            );
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
            Self::Direct { source, .. } | Self::Resampleable { source, .. } => source.id(),
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

    /// Attach the native output lease to the egui paint batch that uses its texture id.
    pub(crate) fn retain_native_output_for_paint(&self, painter: &egui::Painter) {
        if let Some(output) = self.lanczos_output() {
            retain_owner_for_fullscreen_paint_batch(painter, Arc::clone(output));
        }
    }

    pub(crate) fn visible_source_uv_rect(&self) -> Option<egui::Rect> {
        self.lanczos_output()
            .and_then(|output| output.visible_source_uv_rect())
    }

    pub(crate) fn page_idx(&self) -> Option<usize> {
        match self.source_id()? {
            FullscreenPaintSourceId::Page(page_idx) => Some(page_idx),
            FullscreenPaintSourceId::SimilarPreview(_) => None,
        }
    }

    pub(crate) fn source_id(&self) -> Option<FullscreenPaintSourceId> {
        match self {
            Self::Resampleable { source_id, .. } | Self::Lanczos { source_id, .. } => {
                Some(*source_id)
            }
            Self::Direct { .. } => None,
        }
    }

    pub(crate) fn source_generation(&self) -> Option<FullscreenPaintSourceGeneration> {
        match self {
            Self::Resampleable { generation, .. } | Self::Lanczos { generation, .. } => {
                Some(*generation)
            }
            Self::Direct { .. } => None,
        }
    }

    #[cfg(test)]
    pub(crate) fn with_test_paint_provenance(mut self, proof: TestPaintProvenance) -> Self {
        match &mut self {
            Self::Direct {
                test_paint_provenance,
                ..
            }
            | Self::Resampleable {
                test_paint_provenance,
                ..
            }
            | Self::Lanczos {
                test_paint_provenance,
                ..
            } => *test_paint_provenance = Some(proof),
        }
        self
    }

    #[cfg(test)]
    pub(crate) fn test_paint_provenance(&self) -> Option<&TestPaintProvenance> {
        match self {
            Self::Direct {
                test_paint_provenance,
                ..
            }
            | Self::Resampleable {
                test_paint_provenance,
                ..
            }
            | Self::Lanczos {
                test_paint_provenance,
                ..
            } => test_paint_provenance.as_ref(),
        }
    }

    #[cfg(all(windows, feature = "test-script"))]
    pub(crate) fn with_test_script_content_proof(
        mut self,
        proof: crate::test_script::TestScriptContentProof,
    ) -> Self {
        match &mut self {
            Self::Direct {
                test_script_content_proof,
                ..
            }
            | Self::Resampleable {
                test_script_content_proof,
                ..
            }
            | Self::Lanczos {
                test_script_content_proof,
                ..
            } => *test_script_content_proof = Some(proof),
        }
        self
    }

    #[cfg(all(windows, feature = "test-script"))]
    pub(crate) fn test_script_content_proof(
        &self,
    ) -> Option<&crate::test_script::TestScriptContentProof> {
        match self {
            Self::Direct {
                test_script_content_proof,
                ..
            }
            | Self::Resampleable {
                test_script_content_proof,
                ..
            }
            | Self::Lanczos {
                test_script_content_proof,
                ..
            } => test_script_content_proof.as_ref(),
        }
    }

    fn resampleable_parts(
        &self,
    ) -> Option<(
        FullscreenPaintSourceId,
        &egui::TextureHandle,
        FullscreenPaintSourceGeneration,
    )> {
        match self {
            Self::Resampleable {
                source_id,
                source,
                generation,
                ..
            }
            | Self::Lanczos {
                source_id,
                source,
                generation,
                ..
            } => Some((*source_id, source, *generation)),
            Self::Direct { .. } => None,
        }
    }

    fn original_resampleable(&self) -> Self {
        match self {
            Self::Lanczos {
                source_id,
                source,
                generation,
                ..
            } => {
                let original =
                    Self::resampleable_for_source(*source_id, source.clone(), *generation)
                        .with_source_coverage(self.source_coverage())
                        .with_display_basis(self.display_basis());
                #[cfg(test)]
                let original = if let Some(proof) = self.test_paint_provenance().cloned() {
                    original.with_test_paint_provenance(proof)
                } else {
                    original
                };
                #[cfg(all(windows, feature = "test-script"))]
                if let Some(proof) = self.test_script_content_proof().cloned() {
                    return original.with_test_script_content_proof(proof);
                }
                original
            }
            Self::Direct { .. } | Self::Resampleable { .. } => self.clone(),
        }
    }

    fn with_lanczos(&self, output: Arc<LanczosOutput>, smoothing_percent: u32) -> Self {
        let (source_id, source, generation) = self.resampleable_parts().unwrap();
        Self::Lanczos {
            source_id,
            source: source.clone(),
            generation,
            smoothing_percent,
            output,
            coverage: self.source_coverage(),
            display_basis: self.display_basis(),
            #[cfg(test)]
            test_paint_provenance: self.test_paint_provenance().cloned(),
            #[cfg(all(windows, feature = "test-script"))]
            test_script_content_proof: self.test_script_content_proof().cloned(),
        }
    }
}

/// Headless fixture using the product resource/output lease graph without a GPU allocation.
#[cfg(test)]
pub(crate) fn test_native_resource(
    source: FullscreenPaintResource,
    output_size: [u32; 2],
) -> FullscreenPaintResource {
    test_native_resource_for_region(source, output_size, LanczosSourceRegionKey::Full)
}

#[cfg(test)]
pub(crate) fn test_native_resource_with_region(
    source: FullscreenPaintResource,
    output_size: [u32; 2],
    region: egui::Rect,
) -> FullscreenPaintResource {
    test_native_resource_for_region(
        source,
        output_size,
        LanczosSourceRegionKey::from_visible_rect(region),
    )
}

#[cfg(test)]
fn test_native_resource_for_region(
    source: FullscreenPaintResource,
    output_size: [u32; 2],
    source_region: LanczosSourceRegionKey,
) -> FullscreenPaintResource {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT_TEST_NATIVE_ID: AtomicU64 = AtomicU64::new(1 << 48);
    struct TestReleaser;
    impl NativeTextureIdReleaser for TestReleaser {
        fn free_texture(&self, _texture_id: egui::TextureId) {}
    }
    let (source_id, texture, generation) = source.resampleable_parts().unwrap();
    let key = LanczosCacheKey {
        source_id,
        source_texture_id: texture.id(),
        generation,
        target_size: output_size,
        smoothing_percent: 0,
        scale_branch: if source_region == LanczosSourceRegionKey::Full {
            FullscreenPaintScaleBranch::DownscaleLanczos
        } else {
            FullscreenPaintScaleBranch::UpscaleLanczos
        },
        source_region,
    };
    source.with_lanczos(
        Arc::new(LanczosOutput {
            texture_id_lease: NativeTextureIdLease {
                texture_id: egui::TextureId::User(
                    NEXT_TEST_NATIVE_ID.fetch_add(1, Ordering::Relaxed),
                ),
                releaser: Arc::new(TestReleaser),
            },
            key,
            _texture: None,
        }),
        0,
    )
}

/// A presentation owns its paint allocation; source identity and dimensions are values only.
#[derive(Clone)]
pub(crate) struct FullscreenPresentationResource {
    source_texture_id: egui::TextureId,
    logical_size: [usize; 2],
    source_id: Option<FullscreenPaintSourceId>,
    generation: Option<FullscreenPaintSourceGeneration>,
    material: PresentationMaterial,
    coverage: SourceCoverage,
    display_basis: MaterialDisplayBasis,
    #[cfg(test)]
    test_paint_provenance: Option<TestPaintProvenance>,
    #[cfg(all(windows, feature = "test-script"))]
    test_script_content_proof: Option<crate::test_script::TestScriptContentProof>,
}

#[derive(Clone)]
enum PresentationMaterial {
    Native(Arc<LanczosOutput>),
    Managed {
        handle: egui::TextureHandle,
        // Preserve the real sampler, including its mip policy, rather than assuming display defaults.
        actual_options: egui::TextureOptions,
    },
}

impl FullscreenPresentationResource {
    pub(crate) fn display_basis(&self) -> MaterialDisplayBasis {
        self.display_basis
    }

    pub(crate) fn source_coverage(&self) -> SourceCoverage {
        self.coverage
    }

    pub(crate) fn original_display_dimensions(&self) -> DisplayOriginalDimensions {
        original_display_dimensions(self.size(), self.source_coverage())
    }

    pub(crate) fn full_source_size_vec2(&self) -> egui::Vec2 {
        self.original_display_dimensions().as_vec2()
    }

    pub(crate) fn anime_upscale_source_dimensions(
        &self,
        geometry: &crate::displayed_image_transform::DisplayedImageGeometry,
        pixels_per_point: f32,
    ) -> Option<[u32; 2]> {
        anime_material_source_dimensions(
            self.size(),
            self.source_coverage(),
            geometry,
            pixels_per_point,
        )
    }

    pub(crate) fn visible_region_request(
        &self,
        geometry: &crate::displayed_image_transform::DisplayedImageGeometry,
        clip_rect: egui::Rect,
    ) -> Option<VisibleRegionRequest> {
        geometry.visible_region_request_for_coverage(
            clip_rect,
            self.source_coverage().source_uv_rect()?,
        )
    }

    pub(crate) fn paint_geometry(
        &self,
        geometry: &crate::displayed_image_transform::DisplayedImageGeometry,
        painter: &egui::Painter,
        tint: egui::Color32,
    ) {
        if let Some(coverage) = self.source_coverage().source_uv_rect() {
            geometry.paint_material_texture(
                painter,
                self.paint_texture_id(),
                coverage,
                self.visible_source_uv_rect(),
                tint,
            );
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn paint_source_region(
        &self,
        painter: &egui::Painter,
        full_image_rect: egui::Rect,
        rotation: crate::rotation_db::Rotation,
        free_rotation_rad: f32,
        requested_source_rect: egui::Rect,
        tint: egui::Color32,
    ) {
        if let Some(coverage) = self.source_coverage().source_uv_rect() {
            crate::displayed_image_transform::paint_source_coverage_texture(
                painter,
                self.paint_texture_id(),
                full_image_rect,
                rotation,
                free_rotation_rad,
                coverage,
                requested_source_rect,
                self.visible_source_uv_rect(),
                tint,
            );
        }
    }

    pub(crate) fn paint_texture(
        &self,
        transform: &crate::displayed_image_transform::DisplayedImageTransform,
        painter: &egui::Painter,
        tint: egui::Color32,
    ) {
        if let Some(coverage) = self.coverage.source_uv_rect() {
            transform.paint_material_texture(
                painter,
                self.paint_texture_id(),
                coverage,
                self.visible_source_uv_rect(),
                tint,
            );
        }
    }
    /// Reuse only the native allocation ordinary preparation would request for
    /// this held source. A newly published source is deliberately not an input.
    pub(crate) fn matches_prepare(
        &self,
        logical_scale: f32,
        pixels_per_point: f32,
        visible_region: Option<VisibleRegionRequest>,
        post_filter: PostFilter,
        smoothing_percent: u32,
        anime_source_limit: AnimeUpscaleSourceLimit,
    ) -> bool {
        let PresentationMaterial::Native(output) = &self.material else {
            return false;
        };
        let (Some(source_id), Some(generation)) = (self.source_id, self.generation) else {
            return false;
        };
        let Some(key) = requested_covered_lanczos_key(
            source_id,
            self.source_texture_id,
            generation,
            [self.logical_size[0] as u32, self.logical_size[1] as u32],
            logical_scale,
            pixels_per_point,
            visible_region,
            self.coverage,
            post_filter,
            smoothing_percent,
            anime_source_limit,
        ) else {
            return false;
        };
        lanczos_target_decision(key.scale_branch, key.target_size)
            == LanczosTargetDecision::Resample
            && output.key == key
    }

    pub(crate) fn source_texture(&self) -> Option<&egui::TextureHandle> {
        match &self.material {
            PresentationMaterial::Native(_) => None,
            PresentationMaterial::Managed { handle, .. } => Some(handle),
        }
    }

    pub(crate) fn size(&self) -> [usize; 2] {
        self.logical_size
    }

    pub(crate) fn size_vec2(&self) -> egui::Vec2 {
        egui::vec2(self.logical_size[0] as f32, self.logical_size[1] as f32)
    }

    pub(crate) fn source_texture_id(&self) -> egui::TextureId {
        self.source_texture_id
    }

    pub(crate) fn paint_texture_id(&self) -> egui::TextureId {
        match &self.material {
            PresentationMaterial::Native(output) => output.texture_id(),
            PresentationMaterial::Managed { handle, .. } => handle.id(),
        }
    }

    pub(crate) fn id(&self) -> egui::TextureId {
        self.paint_texture_id()
    }

    pub(crate) fn source_id(&self) -> Option<FullscreenPaintSourceId> {
        self.source_id
    }

    pub(crate) fn source_generation(&self) -> Option<FullscreenPaintSourceGeneration> {
        self.generation
    }

    pub(crate) fn page_idx(&self) -> Option<usize> {
        match self.source_id? {
            FullscreenPaintSourceId::Page(page) => Some(page),
            FullscreenPaintSourceId::SimilarPreview(_) => None,
        }
    }

    pub(crate) fn lanczos_output(&self) -> Option<&Arc<LanczosOutput>> {
        match &self.material {
            PresentationMaterial::Native(output) => Some(output),
            PresentationMaterial::Managed { .. } => None,
        }
    }

    pub(crate) fn visible_source_uv_rect(&self) -> Option<egui::Rect> {
        self.lanczos_output()
            .and_then(|output| output.visible_source_uv_rect())
    }

    pub(crate) fn retain_native_output_for_paint(&self, painter: &egui::Painter) {
        if let Some(output) = self.lanczos_output() {
            retain_owner_for_fullscreen_paint_batch(painter, Arc::clone(output));
        }
    }

    pub(crate) fn account_material(&self, accountant: &mut crate::vram_budget::VramAccountant) {
        use crate::vram_budget::VramSubsystem;
        match &self.material {
            PresentationMaterial::Native(output) => accountant.add_texture_id(
                VramSubsystem::PageTurnPresentation,
                output.texture_id(),
                output.size(),
                false,
            ),
            PresentationMaterial::Managed {
                handle,
                actual_options,
            } => accountant.add_texture(
                VramSubsystem::PageTurnPresentation,
                handle,
                actual_options.mipmap_mode.is_some(),
            ),
        }
    }

    #[cfg(test)]
    pub(crate) fn test_paint_provenance(&self) -> Option<&TestPaintProvenance> {
        self.test_paint_provenance.as_ref()
    }

    #[cfg(all(windows, feature = "test-script"))]
    pub(crate) fn test_script_content_proof(
        &self,
    ) -> Option<&crate::test_script::TestScriptContentProof> {
        self.test_script_content_proof.as_ref()
    }
}

/// Rendering boundaries distinguish ordinary source preparation from immutable presentation.
#[derive(Clone)]
pub(crate) enum FullscreenPaintPayload {
    NormalResource(FullscreenPaintResource),
    PresentationMaterial(FullscreenPresentationResource),
}

impl From<FullscreenPaintResource> for FullscreenPaintPayload {
    fn from(resource: FullscreenPaintResource) -> Self {
        Self::NormalResource(resource)
    }
}

impl FullscreenPaintPayload {
    pub(crate) fn display_basis(&self) -> MaterialDisplayBasis {
        match self {
            Self::NormalResource(resource) => resource.display_basis(),
            Self::PresentationMaterial(resource) => resource.display_basis(),
        }
    }

    pub(crate) fn source_coverage(&self) -> SourceCoverage {
        match self {
            Self::NormalResource(resource) => resource.source_coverage(),
            Self::PresentationMaterial(resource) => resource.source_coverage(),
        }
    }

    pub(crate) fn original_display_dimensions(&self) -> DisplayOriginalDimensions {
        original_display_dimensions(self.size(), self.source_coverage())
    }

    pub(crate) fn full_source_size_vec2(&self) -> egui::Vec2 {
        self.original_display_dimensions().as_vec2()
    }

    pub(crate) fn anime_upscale_source_dimensions(
        &self,
        geometry: &crate::displayed_image_transform::DisplayedImageGeometry,
        pixels_per_point: f32,
    ) -> Option<[u32; 2]> {
        anime_material_source_dimensions(
            self.size(),
            self.source_coverage(),
            geometry,
            pixels_per_point,
        )
    }

    pub(crate) fn visible_region_request(
        &self,
        geometry: &crate::displayed_image_transform::DisplayedImageGeometry,
        clip_rect: egui::Rect,
    ) -> Option<VisibleRegionRequest> {
        geometry.visible_region_request_for_coverage(
            clip_rect,
            self.source_coverage().source_uv_rect()?,
        )
    }

    pub(crate) fn paint_geometry(
        &self,
        geometry: &crate::displayed_image_transform::DisplayedImageGeometry,
        painter: &egui::Painter,
        tint: egui::Color32,
    ) {
        if let Some(coverage) = self.source_coverage().source_uv_rect() {
            geometry.paint_material_texture(
                painter,
                self.paint_texture_id(),
                coverage,
                self.visible_source_uv_rect(),
                tint,
            );
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn paint_source_region(
        &self,
        painter: &egui::Painter,
        full_image_rect: egui::Rect,
        rotation: crate::rotation_db::Rotation,
        free_rotation_rad: f32,
        requested_source_rect: egui::Rect,
        tint: egui::Color32,
    ) {
        if let Some(coverage) = self.source_coverage().source_uv_rect() {
            crate::displayed_image_transform::paint_source_coverage_texture(
                painter,
                self.paint_texture_id(),
                full_image_rect,
                rotation,
                free_rotation_rad,
                coverage,
                requested_source_rect,
                self.visible_source_uv_rect(),
                tint,
            );
        }
    }

    pub(crate) fn paint_texture(
        &self,
        transform: &crate::displayed_image_transform::DisplayedImageTransform,
        painter: &egui::Painter,
        tint: egui::Color32,
    ) {
        match self {
            Self::NormalResource(resource) => resource.paint_texture(transform, painter, tint),
            Self::PresentationMaterial(resource) => {
                resource.paint_texture(transform, painter, tint)
            }
        }
    }
    pub(crate) fn capture_presentation(
        &self,
        ctx: &egui::Context,
    ) -> FullscreenPresentationResource {
        match self {
            Self::NormalResource(resource) => resource.capture_presentation(ctx),
            Self::PresentationMaterial(resource) => resource.clone(),
        }
    }

    pub(crate) fn prepare(
        &self,
        cache: &mut GpuLanczosCache,
        render_state: Option<&egui_wgpu::RenderState>,
        logical_scale: f32,
        pixels_per_point: f32,
        visible_region: Option<VisibleRegionRequest>,
        post_filter: PostFilter,
        smoothing_percent: u32,
        anime_source_limit: AnimeUpscaleSourceLimit,
    ) -> (Self, Option<LanczosPerfEvent>) {
        match self {
            Self::NormalResource(resource) => {
                let (resource, event) = cache.resolve(
                    render_state,
                    resource,
                    logical_scale,
                    pixels_per_point,
                    visible_region,
                    post_filter,
                    smoothing_percent,
                    anime_source_limit,
                );
                (Self::NormalResource(resource), event)
            }
            Self::PresentationMaterial(_) => (self.clone(), None),
        }
    }

    pub(crate) fn source_texture(&self) -> Option<&egui::TextureHandle> {
        match self {
            Self::NormalResource(resource) => Some(resource.source_texture()),
            Self::PresentationMaterial(resource) => resource.source_texture(),
        }
    }

    pub(crate) fn size(&self) -> [usize; 2] {
        match self {
            Self::NormalResource(resource) => resource.size(),
            Self::PresentationMaterial(resource) => resource.size(),
        }
    }

    pub(crate) fn size_vec2(&self) -> egui::Vec2 {
        match self {
            Self::NormalResource(resource) => resource.size_vec2(),
            Self::PresentationMaterial(resource) => resource.size_vec2(),
        }
    }

    pub(crate) fn source_texture_id(&self) -> egui::TextureId {
        match self {
            Self::NormalResource(resource) => resource.source_texture_id(),
            Self::PresentationMaterial(resource) => resource.source_texture_id(),
        }
    }

    pub(crate) fn paint_texture_id(&self) -> egui::TextureId {
        match self {
            Self::NormalResource(resource) => resource.paint_texture_id(),
            Self::PresentationMaterial(resource) => resource.paint_texture_id(),
        }
    }

    pub(crate) fn id(&self) -> egui::TextureId {
        self.paint_texture_id()
    }

    pub(crate) fn source_id(&self) -> Option<FullscreenPaintSourceId> {
        match self {
            Self::NormalResource(resource) => resource.source_id(),
            Self::PresentationMaterial(resource) => resource.source_id(),
        }
    }

    pub(crate) fn source_generation(&self) -> Option<FullscreenPaintSourceGeneration> {
        match self {
            Self::NormalResource(resource) => resource.source_generation(),
            Self::PresentationMaterial(resource) => resource.source_generation(),
        }
    }

    pub(crate) fn page_idx(&self) -> Option<usize> {
        match self {
            Self::NormalResource(resource) => resource.page_idx(),
            Self::PresentationMaterial(resource) => resource.page_idx(),
        }
    }

    pub(crate) fn lanczos_output(&self) -> Option<&Arc<LanczosOutput>> {
        match self {
            Self::NormalResource(resource) => resource.lanczos_output(),
            Self::PresentationMaterial(resource) => resource.lanczos_output(),
        }
    }

    pub(crate) fn visible_source_uv_rect(&self) -> Option<egui::Rect> {
        match self {
            Self::NormalResource(resource) => resource.visible_source_uv_rect(),
            Self::PresentationMaterial(resource) => resource.visible_source_uv_rect(),
        }
    }

    pub(crate) fn retain_native_output_for_paint(&self, painter: &egui::Painter) {
        match self {
            Self::NormalResource(resource) => resource.retain_native_output_for_paint(painter),
            Self::PresentationMaterial(resource) => {
                resource.retain_native_output_for_paint(painter)
            }
        }
    }

    pub(crate) fn account_material(&self, accountant: &mut crate::vram_budget::VramAccountant) {
        use crate::vram_budget::VramSubsystem;
        match self {
            Self::NormalResource(resource) => {
                accountant.add_texture(
                    VramSubsystem::PageTurnPresentation,
                    resource.source_texture(),
                    true,
                );
                if let Some(output) = resource.lanczos_output() {
                    accountant.add_texture_id(
                        VramSubsystem::PageTurnPresentation,
                        output.texture_id(),
                        output.size(),
                        false,
                    );
                }
            }
            Self::PresentationMaterial(resource) => resource.account_material(accountant),
        }
    }

    #[cfg(test)]
    pub(crate) fn with_test_paint_provenance(mut self, proof: TestPaintProvenance) -> Self {
        match &mut self {
            Self::NormalResource(resource) => {
                *resource = resource.clone().with_test_paint_provenance(proof);
            }
            Self::PresentationMaterial(resource) => resource.test_paint_provenance = Some(proof),
        }
        self
    }

    #[cfg(test)]
    pub(crate) fn test_paint_provenance(&self) -> Option<&TestPaintProvenance> {
        match self {
            Self::NormalResource(resource) => resource.test_paint_provenance(),
            Self::PresentationMaterial(resource) => resource.test_paint_provenance(),
        }
    }

    #[cfg(all(windows, feature = "test-script"))]
    pub(crate) fn with_test_script_content_proof(
        mut self,
        proof: crate::test_script::TestScriptContentProof,
    ) -> Self {
        match &mut self {
            Self::NormalResource(resource) => {
                *resource = resource.clone().with_test_script_content_proof(proof);
            }
            Self::PresentationMaterial(resource) => {
                resource.test_script_content_proof = Some(proof)
            }
        }
        self
    }

    #[cfg(all(windows, feature = "test-script"))]
    pub(crate) fn test_script_content_proof(
        &self,
    ) -> Option<&crate::test_script::TestScriptContentProof> {
        match self {
            Self::NormalResource(resource) => resource.test_script_content_proof(),
            Self::PresentationMaterial(resource) => resource.test_script_content_proof(),
        }
    }
}

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
pub(crate) struct LanczosCacheKey {
    source_id: FullscreenPaintSourceId,
    source_texture_id: egui::TextureId,
    generation: FullscreenPaintSourceGeneration,
    target_size: [u32; 2],
    smoothing_percent: u32,
    scale_branch: FullscreenPaintScaleBranch,
    source_region: LanczosSourceRegionKey,
}

impl LanczosCacheKey {
    /// Exact diagnostic identity, including the bitwise ROI key. Construct only
    /// while perf logging is enabled.
    pub(crate) fn perf_key(self) -> String {
        format!("{self:?}")
    }

    pub(crate) fn source_texture_id(self) -> egui::TextureId {
        self.source_texture_id
    }

    pub(crate) fn target_size(self) -> [u32; 2] {
        self.target_size
    }

    pub(crate) fn scale_branch(self) -> FullscreenPaintScaleBranch {
        self.scale_branch
    }

    pub(crate) fn visible_source_uv_rect(self) -> Option<egui::Rect> {
        self.source_region.visible_rect()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct NativeAllocationBytes {
    pub(crate) output_bytes: u64,
    pub(crate) temporary_bytes: u64,
}

impl NativeAllocationBytes {
    pub(crate) fn total(self) -> u64 {
        self.output_bytes.saturating_add(self.temporary_bytes)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum NativePrewarmUnavailable {
    NoRenderer,
    MissingBacking,
    BackingMismatch,
    InvalidPlan,
    SourceChanged,
    ClaimDenied,
    CacheSmoothingClear,
    CacheSourceReplacement,
    CacheCapacity,
}

impl NativePrewarmUnavailable {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::NoRenderer => "no_renderer",
            Self::MissingBacking => "missing_backing",
            Self::BackingMismatch => "backing_mismatch",
            Self::InvalidPlan => "invalid_plan",
            Self::SourceChanged => "source_changed",
            Self::ClaimDenied => "claim_denied",
            Self::CacheSmoothingClear => "cache_smoothing_clear",
            Self::CacheSourceReplacement => "cache_source_replacement",
            Self::CacheCapacity => "cache_capacity",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum NativePrewarmPolicy {
    Ordinary,
    NoRemoval,
}

/// A call-local calculation, never a pending owner and never a source lease.
#[derive(Clone, Copy, Debug)]
pub(crate) struct NativePrewarmPlan {
    key: LanczosCacheKey,
    work: LanczosWorkPlan,
    smoothing_percent: u32,
}

impl NativePrewarmPlan {
    pub(crate) fn key(self) -> LanczosCacheKey {
        self.key
    }

    pub(crate) fn allocation(self) -> NativeAllocationBytes {
        self.work.allocation_bytes()
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum NativePrewarmQuery {
    NotNeeded,
    Hit(LanczosCacheKey),
    Unavailable(NativePrewarmUnavailable),
    Miss(NativePrewarmPlan),
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum NativePrewarmExecution {
    Hit,
    Unavailable(NativePrewarmUnavailable),
    Submitted {
        output_texture_id: egui::TextureId,
        stats: LanczosGenerationStats,
    },
}

/// Both normal resolve and speculative planning must use this exact request.
#[allow(clippy::too_many_arguments)]
fn resource_lanczos_key(
    resource: &FullscreenPaintResource,
    logical_scale: f32,
    pixels_per_point: f32,
    visible_region: Option<VisibleRegionRequest>,
    post_filter: PostFilter,
    smoothing_percent: u32,
    anime_source_limit: AnimeUpscaleSourceLimit,
) -> Option<LanczosCacheKey> {
    let (source_id, source, generation) = resource.resampleable_parts()?;
    requested_covered_lanczos_key(
        source_id,
        source.id(),
        generation,
        [source.size()[0] as u32, source.size()[1] as u32],
        logical_scale,
        pixels_per_point,
        visible_region,
        resource.source_coverage(),
        post_filter,
        smoothing_percent,
        anime_source_limit,
    )
}

#[cfg(test)]
#[derive(Debug)]
pub(crate) struct NativeLookupForTest {
    pub(crate) source_texture_id: egui::TextureId,
    pub(crate) key: Option<LanczosCacheKey>,
    pub(crate) outcome: &'static str,
}

#[cfg(test)]
std::thread_local! {
    static NATIVE_LOOKUPS_FOR_TEST: std::cell::RefCell<Option<Vec<NativeLookupForTest>>> = const {
        std::cell::RefCell::new(None)
    };
}

#[cfg(test)]
pub(crate) fn begin_native_lookup_observation_for_test() {
    NATIVE_LOOKUPS_FOR_TEST.with(|lookups| *lookups.borrow_mut() = Some(Vec::new()));
}

#[cfg(test)]
pub(crate) fn take_native_lookups_for_test() -> Vec<NativeLookupForTest> {
    NATIVE_LOOKUPS_FOR_TEST.with(|lookups| lookups.borrow_mut().take().unwrap_or_default())
}

fn record_native_lookup(
    render_state: Option<&egui_wgpu::RenderState>,
    resource: &FullscreenPaintResource,
    key: Option<LanczosCacheKey>,
    outcome: &'static str,
) {
    #[cfg(test)]
    NATIVE_LOOKUPS_FOR_TEST.with(|lookups| {
        if let Some(lookups) = lookups.borrow_mut().as_mut() {
            lookups.push(NativeLookupForTest {
                source_texture_id: resource.source_texture_id(),
                key,
                outcome,
            });
        }
    });
    if !crate::perf::is_enabled() {
        return;
    }
    crate::perf::event(
        "fs",
        "final_prepare_native_lookup",
        None,
        0,
        &[
            (
                "renderer",
                render_state
                    .map(|state| state.renderer.read().diag_id().into())
                    .unwrap_or(serde_json::Value::Null),
            ),
            (
                "source_texture_id",
                format!("{:?}", resource.source_texture_id()).into(),
            ),
            (
                "native_key",
                key.map(|key| key.perf_key().into())
                    .unwrap_or(serde_json::Value::Null),
            ),
            ("outcome", outcome.into()),
        ],
    );
}

/// Pure request identity shared by ordinary preparation and held native reuse.
#[allow(clippy::too_many_arguments)]
fn requested_covered_lanczos_key(
    source_id: FullscreenPaintSourceId,
    source_texture_id: egui::TextureId,
    generation: FullscreenPaintSourceGeneration,
    source_size: [u32; 2],
    logical_scale: f32,
    pixels_per_point: f32,
    visible_region: Option<VisibleRegionRequest>,
    coverage: SourceCoverage,
    post_filter: PostFilter,
    smoothing_percent: u32,
    anime_source_limit: AnimeUpscaleSourceLimit,
) -> Option<LanczosCacheKey> {
    let (scale_branch, target_size, source_region) = covered_sampler_request(
        source_size,
        logical_scale,
        pixels_per_point,
        visible_region,
        coverage,
        post_filter,
    )?;
    finish_requested_lanczos_key(
        source_id,
        source_texture_id,
        generation,
        source_size,
        scale_branch,
        target_size,
        source_region,
        smoothing_percent,
        anime_source_limit,
    )
}

fn covered_sampler_request(
    source_size: [u32; 2],
    logical_scale: f32,
    pixels_per_point: f32,
    visible_region: Option<VisibleRegionRequest>,
    coverage: SourceCoverage,
    post_filter: PostFilter,
) -> Option<(FullscreenPaintScaleBranch, [u32; 2], LanczosSourceRegionKey)> {
    if !coverage.is_partial() {
        let scale_branch =
            fullscreen_paint_scale_branch(logical_scale, pixels_per_point, post_filter);
        if !scale_branch.uses_resampler() {
            return None;
        }
        let (target, region) = target_and_source_region_for_branch(
            source_size,
            physical_scale(logical_scale, pixels_per_point),
            scale_branch,
            visible_region,
        )?;
        return Some((scale_branch, target, region));
    }
    if source_size.contains(&0) || !logical_scale.is_finite() || logical_scale <= 0.0 {
        return None;
    }
    let source_extent = egui::vec2(source_size[0] as f32, source_size[1] as f32);
    let material_extent =
        original_display_dimensions([source_size[0] as usize, source_size[1] as usize], coverage)
            .as_vec2()
            * coverage.source_uv_rect()?.size();
    // Sampling density can differ on each axis after integer thumbnail resize.
    // It affects output texels/filter choice, never fit, contain or page aspect.
    let sampler_scale = material_extent / source_extent * logical_scale;
    let branch_x = fullscreen_paint_scale_branch(sampler_scale.x, pixels_per_point, post_filter);
    let branch_y = fullscreen_paint_scale_branch(sampler_scale.y, pixels_per_point, post_filter);
    let scale_branch = if branch_x == FullscreenPaintScaleBranch::OriginalOneToOne
        && branch_y == FullscreenPaintScaleBranch::OriginalOneToOne
    {
        FullscreenPaintScaleBranch::OriginalOneToOne
    } else if physical_scale(sampler_scale.max_elem(), pixels_per_point) <= 1.0 + 1.0e-4 {
        FullscreenPaintScaleBranch::DownscaleLanczos
    } else {
        fullscreen_paint_scale_branch(sampler_scale.max_elem(), pixels_per_point, post_filter)
    };
    if !scale_branch.uses_resampler() {
        return None;
    }
    let (target_size, source_region) =
        if scale_branch == FullscreenPaintScaleBranch::DownscaleLanczos {
            let scale = physical_scale(logical_scale, pixels_per_point);
            (
                [
                    physical_pixel_extent(material_extent.x, scale).min(source_size[0]),
                    physical_pixel_extent(material_extent.y, scale).min(source_size[1]),
                ],
                LanczosSourceRegionKey::Full,
            )
        } else {
            target_and_source_region_for_branch(
                source_size,
                1.0,
                scale_branch,
                sampler_visible_region(coverage, visible_region),
            )?
        };
    Some((scale_branch, target_size, source_region))
}

fn anime_material_source_dimensions(
    source_size: [usize; 2],
    coverage: SourceCoverage,
    geometry: &crate::displayed_image_transform::DisplayedImageGeometry,
    pixels_per_point: f32,
) -> Option<[u32; 2]> {
    let source_size = [source_size[0] as u32, source_size[1] as u32];
    let request = geometry
        .visible_region_request_for_coverage(geometry.viewport_rect, coverage.source_uv_rect()?);
    let (branch, _, region) = covered_sampler_request(
        source_size,
        geometry.total_scale,
        pixels_per_point,
        request,
        coverage,
        PostFilter::UpscaleAnime,
    )?;
    if branch != FullscreenPaintScaleBranch::UpscaleAnime {
        return None;
    }
    anime_source_region_dimensions(source_size, region.visible_rect()?)
}

/// Full-source requests retain the established sampler sizing contract.
fn requested_lanczos_key(
    source_id: FullscreenPaintSourceId,
    source_texture_id: egui::TextureId,
    generation: FullscreenPaintSourceGeneration,
    source_size: [u32; 2],
    logical_scale: f32,
    pixels_per_point: f32,
    visible_region: Option<VisibleRegionRequest>,
    post_filter: PostFilter,
    smoothing_percent: u32,
    anime_source_limit: AnimeUpscaleSourceLimit,
) -> Option<LanczosCacheKey> {
    let scale_branch = fullscreen_paint_scale_branch(logical_scale, pixels_per_point, post_filter);
    if !scale_branch.uses_resampler() {
        return None;
    }
    let (target_size, source_region) = target_and_source_region_for_branch(
        source_size,
        physical_scale(logical_scale, pixels_per_point),
        scale_branch,
        visible_region,
    )?;
    finish_requested_lanczos_key(
        source_id,
        source_texture_id,
        generation,
        source_size,
        scale_branch,
        target_size,
        source_region,
        smoothing_percent,
        anime_source_limit,
    )
}

#[allow(clippy::too_many_arguments)]
fn finish_requested_lanczos_key(
    source_id: FullscreenPaintSourceId,
    source_texture_id: egui::TextureId,
    generation: FullscreenPaintSourceGeneration,
    source_size: [u32; 2],
    scale_branch: FullscreenPaintScaleBranch,
    target_size: [u32; 2],
    source_region: LanczosSourceRegionKey,
    smoothing_percent: u32,
    anime_source_limit: AnimeUpscaleSourceLimit,
) -> Option<LanczosCacheKey> {
    let scale_branch = scale_branch_with_anime_source_limit(
        scale_branch,
        source_size,
        source_region,
        anime_source_limit,
    );
    if source_region == LanczosSourceRegionKey::Full && target_size == source_size {
        return None;
    }
    Some(LanczosCacheKey {
        source_id,
        source_texture_id,
        generation,
        target_size,
        // The smoothing setting changes downscale support only.
        smoothing_percent: if scale_branch == FullscreenPaintScaleBranch::DownscaleLanczos {
            crate::settings::sanitize_downscale_smoothing_percent(smoothing_percent)
        } else {
            0
        },
        scale_branch,
        source_region,
    })
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
    source_id: FullscreenPaintSourceId,
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
    #[cfg(test)]
    resolve_input_ids: Vec<egui::TextureId>,
}

impl GpuLanczosCache {
    /// Preserve ordinary successful-reuse housekeeping without preparing or
    /// retaining a newly published source, or advancing the cache-use clock.
    pub(crate) fn reuse_presentation(
        &mut self,
        material: &FullscreenPresentationResource,
        logical_scale: f32,
        pixels_per_point: f32,
        visible_region: Option<VisibleRegionRequest>,
        post_filter: PostFilter,
        smoothing_percent: u32,
        anime_source_limit: AnimeUpscaleSourceLimit,
    ) -> bool {
        if !material.matches_prepare(
            logical_scale,
            pixels_per_point,
            visible_region,
            post_filter,
            smoothing_percent,
            anime_source_limit,
        ) {
            return false;
        }
        let output = material.lanczos_output().unwrap();
        self.sync_smoothing_percent(crate::settings::sanitize_downscale_smoothing_percent(
            smoothing_percent,
        ));
        self.limit_fallback_sources
            .remove(&LanczosFallbackSourceKey {
                source_id: output.key.source_id,
                source_texture_id: output.key.source_texture_id,
                generation: output.key.generation,
                scale_branch: output.key.scale_branch,
            });
        true
    }

    #[cfg(test)]
    pub(crate) fn take_test_resolve_input_ids(&mut self) -> Vec<egui::TextureId> {
        std::mem::take(&mut self.resolve_input_ids)
    }

    /// Inspect the ordinary request/cache and renderer backing without changing
    /// cache recency, allocating GPU resources, or claiming a frame slot.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn query_prewarm(
        &self,
        render_state: Option<&egui_wgpu::RenderState>,
        resource: &FullscreenPaintResource,
        logical_scale: f32,
        pixels_per_point: f32,
        visible_region: Option<VisibleRegionRequest>,
        post_filter: PostFilter,
        smoothing_percent: u32,
        anime_source_limit: AnimeUpscaleSourceLimit,
    ) -> NativePrewarmQuery {
        let Some(key) = resource_lanczos_key(
            resource,
            logical_scale,
            pixels_per_point,
            visible_region,
            post_filter,
            smoothing_percent,
            anime_source_limit,
        ) else {
            return NativePrewarmQuery::NotNeeded;
        };
        if lanczos_target_decision(key.scale_branch, key.target_size)
            == LanczosTargetDecision::OriginalFallback
        {
            return NativePrewarmQuery::NotNeeded;
        }
        let smoothing_percent =
            crate::settings::sanitize_downscale_smoothing_percent(smoothing_percent);
        if (self.active_smoothing_percent == smoothing_percent && self.entries.contains_key(&key))
            || resource
                .lanczos_output()
                .is_some_and(|output| output.key == key)
        {
            return NativePrewarmQuery::Hit(key);
        }
        let Ok(work) =
            work_plan_for_key([resource.size()[0] as u32, resource.size()[1] as u32], key)
        else {
            return NativePrewarmQuery::Unavailable(NativePrewarmUnavailable::InvalidPlan);
        };
        let Some(render_state) = render_state else {
            return NativePrewarmQuery::Unavailable(NativePrewarmUnavailable::NoRenderer);
        };
        if let Err(reason) =
            prewarm_source_backing(render_state, key.source_texture_id, work.source_size())
        {
            return NativePrewarmQuery::Unavailable(reason);
        }
        NativePrewarmQuery::Miss(NativePrewarmPlan {
            key,
            work,
            smoothing_percent,
        })
    }

    /// The caller revalidates demand, live activity and global VRAM admission
    /// before entering. `claim` only claims the app-wide frame slot: it runs
    /// after backing validation, with no renderer lock, immediately before GPU
    /// allocation. A cloned backing keeps that validated allocation alive.
    pub(crate) fn execute_prewarm(
        &mut self,
        render_state: &egui_wgpu::RenderState,
        resource: &FullscreenPaintResource,
        plan: NativePrewarmPlan,
        claim: impl FnOnce(NativeAllocationBytes) -> bool,
    ) -> NativePrewarmExecution {
        self.execute_prewarm_with_policy(
            render_state,
            resource,
            plan,
            NativePrewarmPolicy::Ordinary,
            claim,
        )
    }

    /// Pure metadata admission shared by the caller and the execution boundary.
    /// Includes invalidation removals as well as post-insertion pruning.
    pub(crate) fn prewarm_no_removal_admission(
        &self,
        plan: NativePrewarmPlan,
    ) -> Result<(), NativePrewarmUnavailable> {
        if self.active_smoothing_percent != plan.smoothing_percent && !self.entries.is_empty() {
            return Err(NativePrewarmUnavailable::CacheSmoothingClear);
        }
        if self.entries.keys().any(|key| {
            key.source_id == plan.key.source_id
                && (key.source_texture_id != plan.key.source_texture_id
                    || key.generation != plan.key.generation)
        }) {
            return Err(NativePrewarmUnavailable::CacheSourceReplacement);
        }
        let increment = usize::from(!self.entries.contains_key(&plan.key));
        if self.entries.len() + increment > MAX_CACHE_ENTRIES
            || self
                .entries
                .keys()
                .filter(|key| same_source_target_family(**key, plan.key))
                .count()
                + increment
                > MAX_TARGETS_PER_SOURCE
            || self
                .entries
                .keys()
                .copied()
                .chain((increment != 0).then_some(plan.key))
                .map(cached_upscale_pixels)
                .fold(0_u64, u64::saturating_add)
                > MAX_CACHED_UPSCALE_PIXELS
        {
            return Err(NativePrewarmUnavailable::CacheCapacity);
        }
        Ok(())
    }

    pub(crate) fn execute_prewarm_with_policy(
        &mut self,
        render_state: &egui_wgpu::RenderState,
        resource: &FullscreenPaintResource,
        plan: NativePrewarmPlan,
        policy: NativePrewarmPolicy,
        claim: impl FnOnce(NativeAllocationBytes) -> bool,
    ) -> NativePrewarmExecution {
        let Some((source_id, _, generation)) = resource.resampleable_parts() else {
            return NativePrewarmExecution::Unavailable(NativePrewarmUnavailable::SourceChanged);
        };
        if source_id != plan.key.source_id
            || resource.source_texture_id() != plan.key.source_texture_id
            || generation != plan.key.generation
            || resource.size() != plan.work.source_size().map(|size| size as usize)
        {
            return NativePrewarmExecution::Unavailable(NativePrewarmUnavailable::SourceChanged);
        }
        if (self.active_smoothing_percent == plan.smoothing_percent
            && self.entries.contains_key(&plan.key))
            || resource
                .lanczos_output()
                .is_some_and(|output| output.key == plan.key)
        {
            return NativePrewarmExecution::Hit;
        }
        if policy == NativePrewarmPolicy::NoRemoval
            && let Err(reason) = self.prewarm_no_removal_admission(plan)
        {
            return NativePrewarmExecution::Unavailable(reason);
        }
        let source = match prewarm_source_backing(
            render_state,
            plan.key.source_texture_id,
            plan.work.source_size(),
        ) {
            Ok(source) => source,
            Err(reason) => return NativePrewarmExecution::Unavailable(reason),
        };
        if !claim(plan.allocation()) {
            return NativePrewarmExecution::Unavailable(NativePrewarmUnavailable::ClaimDenied);
        }
        self.sync_smoothing_percent(plan.smoothing_percent);
        self.use_clock = self.use_clock.wrapping_add(1);
        self.entries.retain(|candidate, _| {
            candidate.source_id != plan.key.source_id
                || (candidate.source_texture_id == plan.key.source_texture_id
                    && candidate.generation == plan.key.generation)
        });
        let (output, event) =
            self.generate_with_source(render_state, resource, plan.key, plan.work, &source);
        match event {
            Some(LanczosPerfEvent::Generated(stats)) => NativePrewarmExecution::Submitted {
                output_texture_id: output.paint_texture_id(),
                stats,
            },
            _ => NativePrewarmExecution::Unavailable(NativePrewarmUnavailable::InvalidPlan),
        }
    }

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
        #[cfg(test)]
        self.resolve_input_ids.push(resource.source_texture_id());
        let smoothing_percent =
            crate::settings::sanitize_downscale_smoothing_percent(smoothing_percent);
        self.sync_smoothing_percent(smoothing_percent);
        if resource.resampleable_parts().is_none() {
            record_native_lookup(render_state, resource, None, "not_needed");
            return (resource.original_resampleable(), None);
        }
        let (source_id, source, generation) = resource.resampleable_parts().unwrap();
        let source_size = [source.size()[0] as u32, source.size()[1] as u32];
        let Some(key) = resource_lanczos_key(
            resource,
            logical_scale,
            pixels_per_point,
            visible_region,
            post_filter,
            smoothing_percent,
            anime_source_limit,
        ) else {
            record_native_lookup(render_state, resource, None, "not_needed");
            return (resource.original_resampleable(), None);
        };
        let Some(render_state) = render_state else {
            record_native_lookup(None, resource, None, "no_renderer");
            return (resource.original_resampleable(), None);
        };
        let fallback_source = LanczosFallbackSourceKey {
            source_id,
            source_texture_id: source.id(),
            generation,
            scale_branch: key.scale_branch,
        };
        if lanczos_target_decision(key.scale_branch, key.target_size)
            == LanczosTargetDecision::OriginalFallback
        {
            record_native_lookup(Some(render_state), resource, Some(key), "not_needed");
            let first_for_source = self.limit_fallback_sources.insert(fallback_source);
            let event = first_for_source.then_some(upscale_limit_fallback_event(
                source_size,
                key.target_size,
                key.scale_branch,
            ));
            return (resource.original_resampleable(), event);
        }
        self.limit_fallback_sources.remove(&fallback_source);
        if let FullscreenPaintResource::Lanczos {
            output,
            smoothing_percent: resource_smoothing,
            ..
        } = resource
            && output.key.target_size == key.target_size
            && *resource_smoothing == key.smoothing_percent
            && output.key.scale_branch == key.scale_branch
            && output.key.source_region == key.source_region
        {
            record_native_lookup(Some(render_state), resource, Some(key), "hit");
            return (resource.clone(), None);
        }
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
            record_native_lookup(Some(render_state), resource, Some(key), "hit");
            entry.last_used = self.use_clock;
            return (
                resource.with_lanczos(entry.output.clone(), key.smoothing_percent),
                None,
            );
        }
        record_native_lookup(Some(render_state), resource, Some(key), "miss");
        self.entries.retain(|candidate, _| {
            candidate.source_id != key.source_id
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
        let source_texture = {
            let renderer = render_state.renderer.read();
            let Some(source_texture) = renderer
                .texture(&resource.source_texture_id())
                .and_then(|texture| texture.texture.as_ref())
            else {
                return (resource.original_resampleable(), None);
            };
            source_texture.clone()
        };
        self.generate_with_source(render_state, resource, key, plan, &source_texture)
    }

    fn generate_with_source(
        &mut self,
        render_state: &egui_wgpu::RenderState,
        resource: &FullscreenPaintResource,
        key: LanczosCacheKey,
        plan: LanczosWorkPlan,
        source_texture: &wgpu::Texture,
    ) -> (FullscreenPaintResource, Option<LanczosPerfEvent>) {
        let started = Instant::now();
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
        let texture_id = render_state.renderer.write().register_native_texture(
            &render_state.device,
            job.output_view(),
            wgpu::FilterMode::Linear,
        );
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
            key,
            output_texture,
            rs.clone(),
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
        self.entries.retain(|key, _| match key.source_id {
            FullscreenPaintSourceId::Page(page_idx) => keep.contains(&page_idx),
            FullscreenPaintSourceId::SimilarPreview(_) => true,
        });
        self.limit_fallback_sources
            .retain(|key| match key.source_id {
                FullscreenPaintSourceId::Page(page_idx) => keep.contains(&page_idx),
                FullscreenPaintSourceId::SimilarPreview(_) => true,
            });
    }

    pub(crate) fn remove_page(&mut self, page_idx: usize) {
        self.entries.retain(
            |key, _| !matches!(key.source_id, FullscreenPaintSourceId::Page(idx) if idx == page_idx),
        );
        self.limit_fallback_sources.retain(
            |key| !matches!(key.source_id, FullscreenPaintSourceId::Page(idx) if idx == page_idx),
        );
    }

    pub(crate) fn retain_similar_preview_resource(&mut self, keep: Option<u64>) {
        self.entries.retain(|key, _| match key.source_id {
            FullscreenPaintSourceId::Page(_) => true,
            FullscreenPaintSourceId::SimilarPreview(resource_id) => Some(resource_id) == keep,
        });
        self.limit_fallback_sources
            .retain(|key| match key.source_id {
                FullscreenPaintSourceId::Page(_) => true,
                FullscreenPaintSourceId::SimilarPreview(resource_id) => Some(resource_id) == keep,
            });
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

    pub(crate) fn outputs(
        &self,
    ) -> impl Iterator<Item = (FullscreenPaintSourceId, &Arc<LanczosOutput>)> {
        self.entries
            .iter()
            .map(|(key, entry)| (key.source_id, &entry.output))
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
                .keys()
                .copied()
                .map(cached_upscale_pixels)
                .fold(0_u64, u64::saturating_add);
            if total <= MAX_CACHED_UPSCALE_PIXELS {
                break;
            }
            let Some(oldest) = self
                .entries
                .iter()
                .filter(|(key, _)| cached_upscale_pixels(**key) != 0)
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
    left.source_id == right.source_id
        && left.source_texture_id == right.source_texture_id
        && left.generation == right.generation
        && left.smoothing_percent == right.smoothing_percent
        && left.scale_branch == right.scale_branch
}

fn cached_upscale_pixels(key: LanczosCacheKey) -> u64 {
    match key.scale_branch {
        FullscreenPaintScaleBranch::UpscaleLanczos
        | FullscreenPaintScaleBranch::UpscaleNis
        | FullscreenPaintScaleBranch::UpscaleAnime
        | FullscreenPaintScaleBranch::UpscalePixelArt => target_pixels(key.target_size),
        _ => 0,
    }
}

pub(crate) struct LanczosOutput {
    texture_id_lease: NativeTextureIdLease,
    key: LanczosCacheKey,
    #[cfg(not(test))]
    _texture: wgpu::Texture,
    #[cfg(test)]
    _texture: Option<wgpu::Texture>,
}

impl LanczosOutput {
    fn new(
        texture_id: egui::TextureId,
        key: LanczosCacheKey,
        texture: wgpu::Texture,
        render_state: egui_wgpu::RenderState,
    ) -> Self {
        Self {
            texture_id_lease: NativeTextureIdLease {
                texture_id,
                releaser: Arc::new(RendererTextureIdReleaser { render_state }),
            },
            key,
            #[cfg(not(test))]
            _texture: texture,
            #[cfg(test)]
            _texture: Some(texture),
        }
    }

    pub(crate) fn texture_id(&self) -> egui::TextureId {
        self.texture_id_lease.texture_id
    }

    pub(crate) fn size(&self) -> [usize; 2] {
        [
            self.key.target_size[0] as usize,
            self.key.target_size[1] as usize,
        ]
    }

    pub(crate) fn visible_source_uv_rect(&self) -> Option<egui::Rect> {
        self.key.source_region.visible_rect()
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
    fn allocation_bytes(self) -> NativeAllocationBytes {
        let rgba16_bytes = |size: [u32; 2]| {
            u64::from(size[0])
                .saturating_mul(u64::from(size[1]))
                .saturating_mul(8)
        };
        let temporary_bytes = match self {
            Self::Full(plan) => rgba16_bytes([plan.source_size[0], plan.target_size[1]]),
            Self::VisibleUpscale(plan) => {
                rgba16_bytes([plan.intermediate_x_width, plan.target_size[1]])
            }
            Self::AnimeUpscale(plan) => rgba16_bytes(plan.process_size)
                .saturating_mul(STILL_IMAGE_ANIME4K_VARIANT.intermediate_count() as u64),
            Self::NisUpscale(_) | Self::PixelArtUpscale(_) => 0,
        };
        let target = self.target_size();
        NativeAllocationBytes {
            output_bytes: u64::from(target[0])
                .saturating_mul(u64::from(target[1]))
                .saturating_mul(4),
            temporary_bytes,
        }
    }

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

fn backing_matches_plan(
    actual_size: [u32; 2],
    format: wgpu::TextureFormat,
    source_size: [u32; 2],
) -> bool {
    actual_size == source_size && format == wgpu::TextureFormat::Rgba8Unorm
}

fn prewarm_source_backing(
    render_state: &egui_wgpu::RenderState,
    texture_id: egui::TextureId,
    source_size: [u32; 2],
) -> Result<wgpu::Texture, NativePrewarmUnavailable> {
    let renderer = render_state.renderer.read();
    let source = renderer
        .texture(&texture_id)
        .and_then(|texture| texture.texture.as_ref())
        .ok_or(NativePrewarmUnavailable::MissingBacking)?;
    if !backing_matches_plan(
        [source.width(), source.height()],
        source.format(),
        source_size,
    ) {
        return Err(NativePrewarmUnavailable::BackingMismatch);
    }
    Ok(source.clone())
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
            bind_group_layouts: &[Some(&bind_group_layout)],
            immediate_size: 0,
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
            multiview_mask: None,
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

const NIS_PARAMS_UNIFORM_SIZE: usize = 80;

fn pack_nis_params_uniform(plan: NisUpscalePlan) -> [u8; NIS_PARAMS_UNIFORM_SIZE] {
    let mut bytes = [0_u8; NIS_PARAMS_UNIFORM_SIZE];
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
    // The WGSL resolver is shared with native video. Still-image NIS keeps its historical
    // opaque-black source-exterior contract in the aligned vec4 at byte 64.
    for (offset, value) in [0.0_f32, 0.0, 0.0, 1.0].into_iter().enumerate() {
        bytes[64 + offset * 4..68 + offset * 4].copy_from_slice(&value.to_ne_bytes());
    }
    bytes
}

fn nis_params_uniform(device: &wgpu::Device, plan: NisUpscalePlan) -> wgpu::Buffer {
    let bytes = pack_nis_params_uniform(plan);
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
        multiview_mask: None,
        cache: None,
    })
}

#[cfg(test)]
#[path = "gpu_lanczos/prewarm_tests.rs"]
mod prewarm_tests;

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
    fn display_basis_survives_native_capture_payload_and_original_reconstruction() {
        use crate::{
            fullscreen_specified_scale::FullscreenFitBasis,
            material_display_basis::{MaterialDisplayView, MaterialLegacyBasis},
        };

        let ctx = egui::Context::default();
        let texture = ctx.load_texture(
            "display-basis-resource",
            egui::ColorImage::filled([60, 80], egui::Color32::WHITE),
            egui::TextureOptions::LINEAR,
        );
        let partial = SourceCoverage::Known(
            crate::source_coverage::EditPreviewGeometry::new(
                crate::page_dims::CanonicalPageDimensions::new(15000, 20000),
                (6143, 8192),
                (1000, 500, 3000, 6000),
            )
            .unwrap(),
        );
        let whole_basis = MaterialDisplayBasis::new(
            MaterialLegacyBasis::Raster {
                layout_size: egui::vec2(60.0, 80.0),
                original: DisplayOriginalDimensions::new(60, 80),
            },
            MaterialLegacyBasis::Raster {
                layout_size: egui::vec2(6144.0, 8192.0),
                original: DisplayOriginalDimensions::new(6144, 8192),
            },
            MaterialLegacyBasis::SourceLayout {
                basis: crate::fullscreen_specified_scale::LegacySourceLayoutBasis {
                    layout_size: egui::vec2(15000.0, 20000.0),
                    reference_size: egui::vec2(15000.0, 20000.0),
                },
            },
            Ok(
                FullscreenFitBasis::new(egui::vec2(6144.0, 8192.0), egui::vec2(6144.0, 8192.0))
                    .unwrap(),
            ),
        );
        for (coverage, basis) in [
            (SourceCoverage::FullSource, whole_basis),
            (
                partial,
                MaterialDisplayBasis::from_sampler(texture.size(), partial),
            ),
        ] {
            let direct = FullscreenPaintResource::direct(texture.clone())
                .with_source_coverage(coverage)
                .with_display_basis(basis);
            let original = FullscreenPaintResource::resampleable(
                2,
                texture.clone(),
                FullscreenPaintSourceGeneration { items: 4, input: 7 },
            )
            .with_source_coverage(coverage)
            .with_display_basis(basis);
            let native = test_native_resource(original.clone(), [30, 40]);
            for resource in [
                direct,
                original,
                native.clone(),
                native.original_resampleable(),
            ] {
                assert_eq!(resource.display_basis(), basis);
                let payload = FullscreenPaintPayload::NormalResource(resource.clone());
                assert_eq!(payload.display_basis(), basis);
                let captured = payload.capture_presentation(&ctx);
                assert_eq!(captured.display_basis(), basis);
                let held = FullscreenPaintPayload::PresentationMaterial(captured);
                assert_eq!(held.display_basis(), basis);
                assert_eq!(held.capture_presentation(&ctx).display_basis(), basis);
                for view in [
                    MaterialDisplayView::Single,
                    MaterialDisplayView::Spread,
                    MaterialDisplayView::Continuous,
                ] {
                    assert_eq!(held.display_basis().legacy(view), basis.legacy(view));
                }
                assert_eq!(held.display_basis().specified(), basis.specified());
                assert_eq!(
                    resource
                        .with_source_coverage(SourceCoverage::Unknown)
                        .display_basis(),
                    basis,
                    "coverage changes must never reinterpret an accepted fit descriptor"
                );
            }
        }
    }

    #[test]
    fn source_coverage_survives_native_capture_payload_and_original_fallback() {
        let ctx = egui::Context::default();
        let texture = ctx.load_texture(
            "coverage-resource",
            egui::ColorImage::filled([80, 60], egui::Color32::WHITE),
            egui::TextureOptions::LINEAR,
        );
        let coverage = SourceCoverage::Known(
            crate::source_coverage::EditPreviewGeometry::new(
                crate::page_dims::CanonicalPageDimensions::new(1200, 800),
                (600, 400),
                (100, 50, 400, 300),
            )
            .unwrap(),
        );
        let resource = FullscreenPaintResource::resampleable(
            2,
            texture.clone(),
            FullscreenPaintSourceGeneration { items: 4, input: 7 },
        )
        .with_source_coverage(coverage);
        assert_eq!(
            resource.original_display_dimensions().as_tuple(),
            (600, 400)
        );
        assert!((resource.full_source_size_vec2() - egui::vec2(600.0, 400.0)).length() < 0.001);
        let native_roi = egui::Rect::from_min_max(egui::pos2(0.125, 0.25), egui::pos2(0.875, 0.75));
        let native = test_native_resource_with_region(resource.clone(), [120, 60], native_roi);
        for resource in [
            resource,
            native.clone(),
            native.original_resampleable(),
            FullscreenPaintResource::direct(texture).with_source_coverage(coverage),
        ] {
            assert_eq!(resource.source_coverage(), coverage);
            let captured = resource.capture_presentation(&ctx);
            assert_eq!(captured.source_coverage(), coverage);
            assert_eq!(
                captured.original_display_dimensions(),
                resource.original_display_dimensions()
            );
            assert_eq!(
                captured.full_source_size_vec2(),
                resource.full_source_size_vec2()
            );
            let payload = FullscreenPaintPayload::PresentationMaterial(captured);
            assert_eq!(payload.source_coverage(), coverage);
            assert_eq!(
                payload.original_display_dimensions(),
                resource.original_display_dimensions()
            );
            assert_eq!(
                payload.capture_presentation(&ctx).source_coverage(),
                coverage
            );
        }
        assert_eq!(
            native.visible_source_uv_rect(),
            Some(native_roi),
            "native ROI remains sampler-local"
        );
    }

    #[test]
    fn source_coverage_native_request_uses_sampler_coordinates_and_physical_material_extent() {
        let coverage = SourceCoverage::Known(
            crate::source_coverage::EditPreviewGeometry::new(
                crate::page_dims::CanonicalPageDimensions::new(1200, 800),
                (600, 400),
                (150, 100, 300, 200),
            )
            .unwrap(),
        );
        let request = VisibleRegionRequest {
            source_uv_rect: egui::Rect::from_min_max(egui::pos2(0.5, 0.25), egui::pos2(0.75, 0.75)),
            target_size: [375, 500],
        };
        let sampler = sampler_visible_region(coverage, Some(request)).unwrap();
        assert_eq!(
            sampler.source_uv_rect,
            egui::Rect::from_min_max(egui::pos2(0.5, 0.0), egui::pos2(1.0, 1.0))
        );
        assert_eq!(sampler.target_size, [375, 500]);
        let outside = VisibleRegionRequest {
            source_uv_rect: egui::Rect::from_min_max(egui::pos2(0.9, 0.0), egui::pos2(1.0, 1.0)),
            target_size: [100, 100],
        };
        assert!(sampler_visible_region(coverage, Some(outside)).is_none());
    }

    #[test]
    fn source_coverage_original_basis_uses_uncropped_gpu_raster_and_keeps_coordinate_units_separate()
     {
        let ctx = egui::Context::default();
        let coordinate_dims = crate::page_dims::CanonicalPageDimensions::new(20000, 15000);
        let coverage = SourceCoverage::Known(
            crate::source_coverage::EditPreviewGeometry::new(
                coordinate_dims,
                (8192, 6144),
                (3840, 0, 256, 6144),
            )
            .unwrap(),
        );
        let resource = FullscreenPaintResource::resampleable(
            0,
            metadata_only_texture(&ctx, [11, 256], egui::TextureOptions::LINEAR),
            FullscreenPaintSourceGeneration { items: 1, input: 0 },
        )
        .with_source_coverage(coverage);
        assert_eq!(coverage.coordinate_dims(), Some(coordinate_dims));
        assert_eq!(
            resource.original_display_dimensions(),
            DisplayOriginalDimensions::new(8192, 6144)
        );
        assert_eq!(resource.full_source_size_vec2(), egui::vec2(8192.0, 6144.0));
        let raw = FullscreenPaintResource::direct(metadata_only_texture(
            &ctx,
            [8192, 6144],
            egui::TextureOptions::LINEAR,
        ));
        assert_eq!(
            resource.original_display_dimensions(),
            raw.original_display_dimensions()
        );
        // Native density must use the same uncropped raster basis: the source
        // coordinate dimensions would incorrectly request an upscale here.
        let key = resource_lanczos_key(
            &resource,
            0.032,
            1.25,
            None,
            PostFilter::None,
            25,
            AnimeUpscaleSourceLimit::default(),
        )
        .unwrap();
        assert_eq!(
            key.scale_branch,
            FullscreenPaintScaleBranch::DownscaleLanczos
        );
        assert_eq!(key.target_size, [10, 245]);
        let native = test_native_resource(resource.clone(), [10, 245]);
        assert_eq!(
            native.original_resampleable().original_display_dimensions(),
            resource.original_display_dimensions()
        );
        assert_eq!(
            native
                .capture_presentation(&ctx)
                .original_display_dimensions(),
            resource.original_display_dimensions()
        );
    }

    #[test]
    fn source_coverage_full_source_and_uncropped_known_keep_actual_texture_original_basis_after_ai()
    {
        let ctx = egui::Context::default();
        let coverage = SourceCoverage::Known(
            crate::source_coverage::EditPreviewGeometry::new(
                crate::page_dims::CanonicalPageDimensions::new(1000, 750),
                (1000, 750),
                (0, 0, 1000, 750),
            )
            .unwrap(),
        );
        let ordinary = FullscreenPaintResource::direct(metadata_only_texture(
            &ctx,
            [128, 96],
            egui::TextureOptions::LINEAR,
        ))
        .with_source_coverage(coverage);
        assert_eq!(ordinary.original_display_dimensions().as_tuple(), (128, 96));
        assert_eq!(ordinary.full_source_size_vec2(), egui::vec2(128.0, 96.0));
        let after_ai = FullscreenPaintResource::direct(metadata_only_texture(
            &ctx,
            [4000, 3000],
            egui::TextureOptions::LINEAR,
        ));
        assert_eq!(
            after_ai.original_display_dimensions().as_tuple(),
            (4000, 3000)
        );
        assert_eq!(
            after_ai
                .capture_presentation(&ctx)
                .original_display_dimensions(),
            after_ai.original_display_dimensions()
        );
    }

    #[test]
    fn source_coverage_thin_rounded_sampler_keeps_canonical_aspect_and_native_pixel_density_separate()
     {
        let ctx = egui::Context::default();
        let coverage = SourceCoverage::Known(
            crate::source_coverage::EditPreviewGeometry::new(
                crate::page_dims::CanonicalPageDimensions::new(800, 600),
                (800, 600),
                (390, 0, 20, 600),
            )
            .unwrap(),
        );
        let source = ctx.load_texture(
            "thin-rounded",
            egui::ColorImage::filled([9, 256], egui::Color32::WHITE),
            egui::TextureOptions::LINEAR,
        );
        let resource = FullscreenPaintResource::resampleable(
            0,
            source,
            FullscreenPaintSourceGeneration { items: 1, input: 0 },
        )
        .with_source_coverage(coverage);
        assert_eq!(resource.full_source_size_vec2(), egui::vec2(800.0, 600.0));
        assert_eq!(resource.size(), [9, 256]);
        // At 0.4 physical px per canonical px the rounded 9x256 sampler
        // downsamples to 8x240, even though scalar 0.4 is not its texel scale.
        let key = resource_lanczos_key(
            &resource,
            0.32,
            1.25,
            None,
            PostFilter::None,
            25,
            AnimeUpscaleSourceLimit::default(),
        )
        .unwrap();
        assert_eq!(
            key.scale_branch,
            FullscreenPaintScaleBranch::DownscaleLanczos
        );
        assert_eq!(key.target_size, [8, 240]);
        assert_eq!(key.source_region, LanczosSourceRegionKey::Full);
        // At this boundary the height is 1:1 while the rounded narrow axis
        // still shrinks. The maximum axis alone must not bypass resampling.
        let key = resource_lanczos_key(
            &resource,
            (256.0 / 600.0) / 1.25,
            1.25,
            None,
            PostFilter::None,
            25,
            AnimeUpscaleSourceLimit::default(),
        )
        .unwrap();
        assert_eq!(
            key.scale_branch,
            FullscreenPaintScaleBranch::DownscaleLanczos
        );
        assert_eq!(key.target_size, [8, 256]);
        // Height crosses into upscale first, while width remains below 1:1.
        // Output dimensions remain the geometry-owned pair for both axes.
        let mixed_request = VisibleRegionRequest {
            source_uv_rect: coverage.source_uv_rect().unwrap(),
            target_size: [9, 264],
        };
        let mixed = resource_lanczos_key(
            &resource,
            0.44 / 1.25,
            1.25,
            Some(mixed_request),
            PostFilter::None,
            25,
            AnimeUpscaleSourceLimit::default(),
        )
        .unwrap();
        assert_eq!(
            mixed.scale_branch,
            FullscreenPaintScaleBranch::UpscaleLanczos
        );
        assert_eq!(mixed.target_size, [9, 264]);
        assert!(work_plan_for_key([9, 256], mixed).is_ok());
        for ppp in [1.25, 1.375, 1.5] {
            let geometry = crate::displayed_image_transform::DisplayedImageGeometry::resolve(
                crate::displayed_image_transform::DisplayedImageGeometryInput {
                    viewport_rect: egui::Rect::from_min_size(
                        egui::pos2(13.0, 17.0),
                        egui::vec2(1003.0, 747.0),
                    ),
                    source_size: egui::vec2(800.0, 600.0),
                    texture_size: resource.full_source_size_vec2(),
                    rotation: crate::rotation_db::Rotation::None,
                    free_rotation_rad: 0.0,
                    content_bbox: None,
                    fit_mode: crate::fullscreen_specified_scale::FullscreenFitInput::legacy(
                        crate::settings::FullscreenFitMode::Page,
                    ),
                    fit_scale_limits:
                        crate::displayed_image_transform::FullscreenFitScaleLimits::default(),
                    pixels_per_point: ppp,
                    placement: crate::displayed_image_transform::ResolvedDisplayPlacement::Normal {
                        zoom_pan: None,
                    },
                    pixel_fit: crate::displayed_image_transform::RectPixelFit::Texels,
                },
            )
            .unwrap();
            let request = resource
                .visible_region_request(&geometry, geometry.viewport_rect)
                .unwrap();
            assert_eq!(
                resource.anime_upscale_source_dimensions(&geometry, ppp),
                Some([9, 256]),
                "Anime budget uses the actual sampler, not the 800x600 geometry basis"
            );
            let held = resource.capture_presentation(&ctx);
            assert_eq!(
                held.anime_upscale_source_dimensions(&geometry, ppp),
                Some([9, 256])
            );
            assert_eq!(
                FullscreenPaintPayload::PresentationMaterial(held)
                    .anime_upscale_source_dimensions(&geometry, ppp),
                Some([9, 256])
            );
            let key = resource_lanczos_key(
                &resource,
                geometry.total_scale,
                ppp,
                Some(request),
                PostFilter::None,
                25,
                AnimeUpscaleSourceLimit::default(),
            )
            .unwrap();
            assert_eq!(key.scale_branch, FullscreenPaintScaleBranch::UpscaleLanczos);
            assert_eq!(
                key.target_size, request.target_size,
                "native output uses the geometry-owned physical span"
            );
            assert_eq!(
                key.source_region.visible_rect().unwrap(),
                egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0))
            );
            let native = resource.with_lanczos(
                Arc::new(LanczosOutput {
                    texture_id_lease: NativeTextureIdLease {
                        texture_id: egui::TextureId::User(900_000),
                        releaser: Arc::new(CountingReleaser(AtomicUsize::new(0))),
                    },
                    key,
                    _texture: None,
                }),
                key.smoothing_percent,
            );
            assert!(
                native.capture_presentation(&ctx).matches_prepare(
                    geometry.total_scale,
                    ppp,
                    Some(request),
                    PostFilter::None,
                    25,
                    AnimeUpscaleSourceLimit::default()
                ),
                "native capture and normal preparation must use the same sampling density"
            );
        }
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
                fit_mode: crate::fullscreen_specified_scale::FullscreenFitInput::legacy(
                    crate::settings::FullscreenFitMode::Page,
                ),
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
                    fit_mode: crate::fullscreen_specified_scale::FullscreenFitInput::legacy(
                        crate::settings::FullscreenFitMode::Page,
                    ),
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
                crate::fullscreen_specified_scale::FullscreenFitInput::legacy(
                    crate::settings::FullscreenFitMode::Page,
                ),
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
            source_id: FullscreenPaintSourceId::Page(0),
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
            source_id: FullscreenPaintSourceId::Page(0),
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
            source_id: FullscreenPaintSourceId::Page(0),
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
    fn page_retention_and_preview_retirement_do_not_cross_source_owners() {
        let generation = FullscreenPaintSourceGeneration { items: 4, input: 8 };
        let fallback = |source_id| LanczosFallbackSourceKey {
            source_id,
            source_texture_id: egui::TextureId::Managed(7),
            generation,
            scale_branch: FullscreenPaintScaleBranch::UpscaleLanczos,
        };
        let page = fallback(FullscreenPaintSourceId::Page(7));
        let preview_7 = fallback(FullscreenPaintSourceId::SimilarPreview(7));
        let preview_9 = fallback(FullscreenPaintSourceId::SimilarPreview(9));
        let mut cache = GpuLanczosCache::default();
        cache
            .limit_fallback_sources
            .extend([page, preview_7, preview_9]);

        cache.retain_page_indices(&std::collections::HashSet::from([7]));
        assert_eq!(cache.limit_fallback_sources.len(), 3);
        cache.remove_page(7);
        assert_eq!(
            cache.limit_fallback_sources,
            std::collections::HashSet::from([preview_7, preview_9])
        );
        cache.retain_similar_preview_resource(Some(9));
        assert_eq!(
            cache.limit_fallback_sources,
            std::collections::HashSet::from([preview_9])
        );
        cache.retain_similar_preview_resource(None);
        assert!(cache.limit_fallback_sources.is_empty());
    }

    #[test]
    fn source_generation_is_cache_identity() {
        let base = LanczosCacheKey {
            source_id: FullscreenPaintSourceId::Page(3),
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
        let preview = LanczosCacheKey {
            source_id: FullscreenPaintSourceId::SimilarPreview(3),
            ..base
        };
        assert_ne!(
            base, preview,
            "page and transient preview IDs must stay distinct even for the same numeric value"
        );
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
            source_id: base.source_id,
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

    #[derive(Clone, Copy)]
    struct TestPrepareRequest {
        scale: f32,
        ppp: f32,
        visible: Option<VisibleRegionRequest>,
        filter: PostFilter,
        smoothing: u32,
        anime_limit: AnimeUpscaleSourceLimit,
    }

    impl TestPrepareRequest {
        fn matches(self, material: &FullscreenPresentationResource) -> bool {
            material.matches_prepare(
                self.scale,
                self.ppp,
                self.visible,
                self.filter,
                self.smoothing,
                self.anime_limit,
            )
        }

        fn reuse(
            self,
            cache: &mut GpuLanczosCache,
            material: &FullscreenPresentationResource,
        ) -> bool {
            cache.reuse_presentation(
                material,
                self.scale,
                self.ppp,
                self.visible,
                self.filter,
                self.smoothing,
                self.anime_limit,
            )
        }

        fn capture_native(
            self,
            ctx: &egui::Context,
            source_size: [usize; 2],
        ) -> FullscreenPresentationResource {
            let source = FullscreenPaintResource::resampleable(
                3,
                metadata_only_texture(ctx, source_size, egui::TextureOptions::LINEAR),
                FullscreenPaintSourceGeneration {
                    items: 7,
                    input: 11,
                },
            );
            let key = requested_lanczos_key(
                source.source_id().unwrap(),
                source.source_texture_id(),
                source.source_generation().unwrap(),
                [source_size[0] as u32, source_size[1] as u32],
                self.scale,
                self.ppp,
                self.visible,
                self.filter,
                self.smoothing,
                self.anime_limit,
            )
            .unwrap();
            source
                .with_lanczos(
                    Arc::new(LanczosOutput {
                        texture_id_lease: NativeTextureIdLease {
                            texture_id: egui::TextureId::User(9003),
                            releaser: Arc::new(CountingReleaser(AtomicUsize::new(0))),
                        },
                        key,
                        _texture: None,
                    }),
                    key.smoothing_percent,
                )
                .capture_presentation(ctx)
        }
    }

    #[test]
    fn held_native_prepare_identity_uses_held_source_and_actual_downscale_request() {
        let ctx = egui::Context::default();
        let request = TestPrepareRequest {
            scale: 0.25,
            ppp: 1.0,
            visible: None,
            filter: PostFilter::None,
            smoothing: 20,
            anime_limit: AnimeUpscaleSourceLimit::Px4096,
        };
        let held = request.capture_native(&ctx, [8000, 8192]);
        assert!(
            ctx.tex_manager()
                .read()
                .meta(held.source_texture_id())
                .is_none()
        );
        // A publication may have a different resolution and generation. The
        // held allocation's request identity remains based on its own source.
        let new_source = FullscreenPaintResource::resampleable(
            3,
            metadata_only_texture(&ctx, [4000, 4096], egui::TextureOptions::LINEAR),
            FullscreenPaintSourceGeneration {
                items: 7,
                input: 12,
            },
        );
        assert_ne!(new_source.source_texture_id(), held.source_texture_id());
        assert!(request.matches(&held));
        for changed in [
            TestPrepareRequest {
                scale: 0.3,
                ..request
            },
            TestPrepareRequest {
                ppp: 2.0,
                ..request
            },
            TestPrepareRequest {
                scale: 1.0,
                ..request
            },
            TestPrepareRequest {
                smoothing: 30,
                ..request
            },
        ] {
            assert!(!changed.matches(&held));
        }
        // The ordinary downscale path ignores visible ROI, upscale filter and
        // Anime limits; request identity must preserve that same equivalence.
        assert!(
            TestPrepareRequest {
                visible: Some(region_request(
                    egui::Rect::from_min_max(egui::pos2(0.2, 0.3), egui::pos2(0.5, 0.8)),
                    [1, 1],
                )),
                filter: PostFilter::Nearest,
                anime_limit: AnimeUpscaleSourceLimit::Px2048,
                ..request
            }
            .matches(&held)
        );
        let managed = new_source.capture_presentation(&ctx);
        assert!(!request.matches(&managed));
    }

    #[test]
    fn held_native_prepare_identity_checks_upscale_region_target_filter_and_anime_limit() {
        let ctx = egui::Context::default();
        let roi = egui::Rect::from_min_max(egui::pos2(0.1, 0.2), egui::pos2(0.6, 0.7));
        let request = TestPrepareRequest {
            scale: 2.0,
            ppp: 1.0,
            visible: Some(region_request(roi, [1000, 1000])),
            filter: PostFilter::None,
            smoothing: 20,
            anime_limit: AnimeUpscaleSourceLimit::Px4096,
        };
        let held = request.capture_native(&ctx, [5000, 4000]);
        assert!(request.matches(&held));
        for changed in [
            TestPrepareRequest {
                visible: None,
                ..request
            },
            TestPrepareRequest {
                visible: Some(region_request(roi, [1001, 1000])),
                ..request
            },
            TestPrepareRequest {
                visible: Some(region_request(
                    egui::Rect::from_min_max(egui::pos2(0.11, 0.2), roi.max),
                    [1000, 1000],
                )),
                ..request
            },
            TestPrepareRequest {
                visible: Some(region_request(roi, [MAX_UPSCALE_TARGET_DIMENSION + 1, 8])),
                ..request
            },
            TestPrepareRequest {
                filter: PostFilter::Nearest,
                ..request
            },
            TestPrepareRequest {
                filter: PostFilter::UpscaleSharp,
                ..request
            },
            TestPrepareRequest {
                filter: PostFilter::UpscaleAnime,
                ..request
            },
            TestPrepareRequest {
                filter: PostFilter::UpscalePixelArt,
                ..request
            },
        ] {
            assert!(!changed.matches(&held));
        }
        assert!(
            TestPrepareRequest {
                smoothing: 100,
                ..request
            }
            .matches(&held)
        );
        // Scale/ppp do not form extra GPU identity when the caller's explicit
        // visible texel target and branch remain identical; layout is checked by its owner.
        assert!(
            TestPrepareRequest {
                scale: 2.5,
                ppp: 2.0,
                ..request
            }
            .matches(&held)
        );

        let anime = TestPrepareRequest {
            filter: PostFilter::UpscaleAnime,
            visible: Some(VisibleRegionRequest::full([1000, 1000])),
            ..request
        };
        let fallback = anime.capture_native(&ctx, [5000, 4000]);
        assert!(anime.matches(&fallback));
        assert!(
            !TestPrepareRequest {
                anime_limit: AnimeUpscaleSourceLimit::Unlimited,
                ..anime
            }
            .matches(&fallback)
        );
        assert!(
            TestPrepareRequest {
                anime_limit: AnimeUpscaleSourceLimit::Px2048,
                ..anime
            }
            .matches(&fallback)
        );
    }

    #[test]
    fn held_native_reuse_preserves_housekeeping_without_preparing_or_advancing_cache_clock() {
        let ctx = egui::Context::default();
        let request = TestPrepareRequest {
            scale: 2.0,
            ppp: 1.0,
            visible: Some(VisibleRegionRequest::full([16, 24])),
            filter: PostFilter::None,
            smoothing: 20,
            anime_limit: AnimeUpscaleSourceLimit::Px4096,
        };
        let held = request.capture_native(&ctx, [8, 12]);
        let output = held.lanczos_output().unwrap();
        let key = output.key;
        let fallback = LanczosFallbackSourceKey {
            source_id: key.source_id,
            source_texture_id: key.source_texture_id,
            generation: key.generation,
            scale_branch: key.scale_branch,
        };
        let sibling = LanczosFallbackSourceKey {
            source_id: FullscreenPaintSourceId::Page(4),
            ..fallback
        };
        let mut cache = GpuLanczosCache {
            active_smoothing_percent: 20,
            use_clock: 23,
            regeneration_count: 9,
            ..Default::default()
        };
        cache.entries.insert(
            key,
            LanczosCacheEntry {
                output: output.clone(),
                last_used: 7,
            },
        );
        cache.limit_fallback_sources.extend([fallback, sibling]);
        assert!(request.reuse(&mut cache, &held));
        assert!(!cache.limit_fallback_sources.contains(&fallback));
        assert!(cache.limit_fallback_sources.contains(&sibling));
        assert_eq!(cache.entries[&key].last_used, 7);
        assert_eq!(cache.use_clock, 23);
        assert_eq!(cache.regeneration_count, 9);
        assert!(cache.take_test_resolve_input_ids().is_empty());

        cache.limit_fallback_sources.insert(fallback);
        assert!(
            !TestPrepareRequest {
                visible: None,
                ..request
            }
            .reuse(&mut cache, &held)
        );
        assert!(cache.limit_fallback_sources.contains(&fallback));
        assert_eq!(cache.entries[&key].last_used, 7);
        // Upscale ignores support-percent for its output identity, but ordinary
        // preparation still synchronizes the setting and clears obsolete cache entries.
        assert!(
            TestPrepareRequest {
                smoothing: 200,
                ..request
            }
            .reuse(&mut cache, &held)
        );
        assert_eq!(cache.active_smoothing_percent, 100);
        assert!(cache.entries.is_empty());
        assert!(!cache.limit_fallback_sources.contains(&fallback));
        assert!(cache.limit_fallback_sources.contains(&sibling));
        assert_eq!(cache.use_clock, 23);
        assert_eq!(cache.regeneration_count, 9);
        assert!(cache.take_test_resolve_input_ids().is_empty());
        assert!(held.source_texture().is_none());
    }

    #[test]
    fn presentation_native_releases_source_and_preserves_metadata_through_batch() {
        let ctx = egui::Context::default();
        let source = ctx.load_texture(
            "presentation-native-input",
            egui::ColorImage::filled([16, 8], egui::Color32::WHITE),
            egui::TextureOptions::LINEAR,
        );
        let source_texture_id = source.id();
        let generation = FullscreenPaintSourceGeneration {
            items: 17,
            input: 9,
        };
        let proof = TestPaintProvenance {
            context: crate::app::ViewerContextId::for_test(4),
            items_generation: generation.items,
            item: "presentation-native-input".to_owned(),
            page: 3,
        };
        let releaser = Arc::new(CountingReleaser(AtomicUsize::new(0)));
        let roi = egui::Rect::from_min_max(egui::pos2(0.25, 0.0), egui::pos2(0.75, 1.0));
        let resource = FullscreenPaintResource::resampleable(3, source, generation)
            .with_test_paint_provenance(proof.clone())
            .with_lanczos(
                Arc::new(LanczosOutput {
                    texture_id_lease: NativeTextureIdLease {
                        texture_id: egui::TextureId::User(9001),
                        releaser: releaser.clone(),
                    },
                    key: LanczosCacheKey {
                        source_id: FullscreenPaintSourceId::Page(3),
                        source_texture_id,
                        generation,
                        target_size: [8, 4],
                        smoothing_percent: 0,
                        scale_branch: FullscreenPaintScaleBranch::UpscaleLanczos,
                        source_region: LanczosSourceRegionKey::from_visible_rect(roi),
                    },
                    _texture: None,
                }),
                0,
            );
        let payload =
            FullscreenPaintPayload::PresentationMaterial(resource.capture_presentation(&ctx));
        drop(resource);
        assert!(ctx.tex_manager().read().meta(source_texture_id).is_none());
        assert!(payload.source_texture().is_none());
        assert_eq!(payload.size(), [16, 8]);
        assert_eq!(payload.size_vec2(), egui::vec2(16.0, 8.0));
        assert_eq!(payload.source_texture_id(), source_texture_id);
        assert_eq!(payload.source_id(), Some(FullscreenPaintSourceId::Page(3)));
        assert_eq!(payload.page_idx(), Some(3));
        assert_eq!(payload.source_generation(), Some(generation));
        assert_eq!(payload.test_paint_provenance(), Some(&proof));
        assert_eq!(payload.visible_source_uv_rect(), Some(roi));
        let mut accountant = crate::vram_budget::VramAccountant::default();
        payload.account_material(&mut accountant);
        assert_eq!(accountant.finish().total_bytes(), 8 * 4 * 4);

        let mut cache = GpuLanczosCache::default();
        let (prepared, event) = payload.prepare(
            &mut cache,
            None,
            1.0,
            2.0,
            None,
            PostFilter::Nearest,
            80,
            AnimeUpscaleSourceLimit::default(),
        );
        assert!(event.is_none());
        assert_eq!(prepared.id(), payload.id());
        assert_eq!(prepared.visible_source_uv_rect(), Some(roi));
        assert_eq!(cache.active_smoothing_percent, 0);
        assert!(cache.entries.is_empty());
        drop(prepared);

        let mut raw = egui::RawInput::default();
        raw.screen_rect = Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(64.0, 64.0),
        ));
        let mut output = ctx.run_pass(raw, |ctx| {
            let painter = ctx.debug_painter();
            painter.image(
                payload.id(),
                egui::Rect::from_min_size(egui::pos2(2.0, 2.0), egui::vec2(16.0, 8.0)),
                egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
                egui::Color32::WHITE,
            );
            payload.retain_native_output_for_paint(&painter);
        });
        drop(payload);
        assert_eq!(releaser.0.load(Ordering::Relaxed), 0);
        let primitives =
            ctx.tessellate(std::mem::take(&mut output.shapes), output.pixels_per_point);
        output.drop_without_applying_deltas();
        assert!(
            primitives.iter().any(|primitive| matches!(
                &primitive.primitive,
                egui::epaint::Primitive::Callback(_)
            ))
        );
        assert_eq!(releaser.0.load(Ordering::Relaxed), 0);
        drop(primitives);
        assert_eq!(releaser.0.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn presentation_managed_preserves_actual_sampler_and_mip_accounting() {
        for options in [
            egui::TextureOptions::NEAREST,
            egui::TextureOptions::LINEAR.with_mipmap_mode(Some(egui::TextureFilter::Linear)),
        ] {
            let ctx = egui::Context::default();
            let source = ctx.load_texture(
                "presentation-managed-input",
                egui::ColorImage::filled([7, 5], egui::Color32::WHITE),
                options,
            );
            // Display sources are immutable allocations. epaint 0.33 metadata records
            // initial options; changing a handle's sampler with set is not this contract.
            let source_id = source.id();
            let resource = FullscreenPaintResource::resampleable(
                2,
                source,
                FullscreenPaintSourceGeneration { items: 6, input: 4 },
            );
            let presentation = resource.capture_presentation(&ctx);
            drop(resource);
            let PresentationMaterial::Managed { actual_options, .. } = &presentation.material
            else {
                panic!("ordinary source painting remains Managed");
            };
            assert_eq!(*actual_options, options);
            assert_eq!(presentation.source_texture().unwrap().id(), source_id);
            assert_eq!(presentation.id(), source_id);
            assert_eq!(presentation.size(), [7, 5]);
            let mut accountant = crate::vram_budget::VramAccountant::default();
            presentation.account_material(&mut accountant);
            assert_eq!(
                accountant.finish().total_bytes(),
                (crate::vram_budget::texture_size_texels([7, 5], options.mipmap_mode.is_some()) * 4)
                    as u64,
            );
            assert!(ctx.tex_manager().read().meta(source_id).is_some());
            drop(presentation);
            assert!(ctx.tex_manager().read().meta(source_id).is_none());
        }
    }

    fn metadata_only_texture(
        ctx: &egui::Context,
        size: [usize; 2],
        options: egui::TextureOptions,
    ) -> egui::TextureHandle {
        let manager = ctx.tex_manager();
        let id = manager.write().alloc(
            "managed-prepare-metadata".to_owned(),
            egui::ImageData::Color(Arc::new(egui::ColorImage {
                size,
                source_size: egui::vec2(size[0] as f32, size[1] as f32),
                pixels: Vec::new(),
            })),
            options,
        );
        egui::TextureHandle::new(manager, id)
    }

    fn assert_managed_prepared_allocation(
        ctx: &egui::Context,
        prepared: &FullscreenPaintResource,
        original: &FullscreenPaintResource,
        options: egui::TextureOptions,
    ) {
        let captured = prepared.capture_presentation(ctx);
        assert!(captured.lanczos_output().is_none());
        assert!(captured.visible_source_uv_rect().is_none());
        assert_eq!(captured.id(), original.source_texture_id());
        assert_eq!(captured.size(), original.size());
        assert_eq!(captured.source_id(), original.source_id());
        assert_eq!(captured.source_generation(), original.source_generation());
        assert_eq!(
            captured.source_texture().unwrap().id(),
            original.source_texture_id()
        );
        let mut accountant = crate::vram_budget::VramAccountant::default();
        captured.account_material(&mut accountant);
        assert_eq!(
            accountant.finish().total_bytes(),
            crate::vram_budget::texture_size_texels(original.size(), options.mipmap_mode.is_some())
                as u64
                * 4,
        );
    }

    #[test]
    fn ordinary_prepare_managed_matrix_keeps_allocation_geometry_and_accounting() {
        let ctx = egui::Context::default();
        let mip = egui::TextureOptions::LINEAR.with_mipmap_mode(Some(egui::TextureFilter::Linear));
        for (name, size, options, scale, filter, resampleable) in [
            ("one-to-one", [8, 12], mip, 1.0, PostFilter::None, true),
            (
                "nearest-upscale",
                [8, 12],
                egui::TextureOptions::NEAREST,
                2.0,
                PostFilter::Nearest,
                true,
            ),
            (
                "large-direct",
                [8000, 8192],
                mip,
                0.25,
                PostFilter::None,
                false,
            ),
            (
                "catalog-thumbnail",
                [7, 5],
                egui::TextureOptions::LINEAR,
                3.0,
                PostFilter::None,
                false,
            ),
        ] {
            let source = metadata_only_texture(&ctx, size, options);
            let original = if resampleable {
                FullscreenPaintResource::resampleable(
                    3,
                    source,
                    FullscreenPaintSourceGeneration {
                        items: 7,
                        input: 11,
                    },
                )
            } else {
                FullscreenPaintResource::direct(source)
            };
            let mut cache = GpuLanczosCache::default();
            let (prepared, event) = cache.resolve(
                None,
                &original,
                scale,
                1.0,
                None,
                filter,
                0,
                AnimeUpscaleSourceLimit::default(),
            );
            assert!(event.is_none(), "{name}");
            assert_managed_prepared_allocation(&ctx, &prepared, &original, options);
            assert_eq!(cache.regeneration_count, 0, "{name}");
            assert_eq!(cache.use_clock, 0, "{name}");
            assert!(cache.entries.is_empty(), "{name}");
            assert!(cache.resampler.is_none(), "{name}");
        }
    }

    #[test]
    fn ordinary_prepare_output_limit_fallback_captures_actual_managed_allocation() {
        // A real headless state gets past the ordinary renderer-availability gate.
        // No surface/window, texture upload, resampler or presentation is created.
        let mut descriptor = wgpu::InstanceDescriptor::new_without_display_handle();
        descriptor.backend_options.dx12.shader_compiler = wgpu::Dx12Compiler::Fxc;
        let instance = wgpu::Instance::new(descriptor);
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            compatible_surface: None,
            ..Default::default()
        }))
        .expect("output-limit prepare regression requires a headless wgpu adapter");
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("managed output-limit prepare regression"),
            ..Default::default()
        }))
        .expect("headless output-limit device");
        let target_format = wgpu::TextureFormat::Rgba8Unorm;
        let renderer = egui_wgpu::Renderer::new(&device, target_format, Default::default());
        let render_state = egui_wgpu::RenderState {
            instance,
            adapter,
            available_adapters: Vec::new(),
            device,
            queue,
            target_format,
            renderer: Arc::new(egui::epaint::mutex::RwLock::new(renderer)),
        };
        let ctx = egui::Context::default();
        let options =
            egui::TextureOptions::LINEAR.with_mipmap_mode(Some(egui::TextureFilter::Linear));
        let original = FullscreenPaintResource::resampleable(
            3,
            metadata_only_texture(&ctx, [8, 12], options),
            FullscreenPaintSourceGeneration {
                items: 7,
                input: 11,
            },
        );
        let mut cache = GpuLanczosCache::default();
        for first in [true, false] {
            let (prepared, event) = cache.resolve(
                Some(&render_state),
                &original,
                2.0,
                1.0,
                Some(VisibleRegionRequest::full([
                    MAX_UPSCALE_TARGET_DIMENSION + 1,
                    8,
                ])),
                PostFilter::None,
                0,
                AnimeUpscaleSourceLimit::default(),
            );
            assert_eq!(
                matches!(event, Some(LanczosPerfEvent::UpscaleLimitFallback(_))),
                first
            );
            assert_managed_prepared_allocation(&ctx, &prepared, &original, options);
            assert_eq!(cache.regeneration_count, 0);
            assert_eq!(cache.use_clock, 0);
            assert!(cache.entries.is_empty());
            assert!(cache.resampler.is_none());
            assert_eq!(cache.limit_fallback_sources.len(), 1);
        }
    }

    #[cfg(all(windows, feature = "test-script"))]
    #[test]
    fn test_script_content_proof_survives_all_paint_resource_forms() {
        let context = egui::Context::default();
        let source = context.load_texture(
            "test-script-proof-source",
            egui::ColorImage::new([1, 1], vec![egui::Color32::WHITE]),
            egui::TextureOptions::LINEAR,
        );
        let proof = crate::test_script::TestScriptContentProof {
            context_serial: 12,
            items_generation: 34,
            page_index: 5,
            item_identity: "proof-item".to_owned(),
            source_texture_id: source.id(),
            source_kind: crate::test_script::TestScriptPaintSourceKind::FullOrProcessed,
            final_composite_complete: false,
        };

        let direct = FullscreenPaintResource::direct(source.clone())
            .with_test_script_content_proof(proof.clone());
        assert_eq!(direct.test_script_content_proof(), Some(&proof));
        let presentation = direct.capture_presentation(&context);
        assert_eq!(presentation.test_script_content_proof(), Some(&proof));

        let generation = FullscreenPaintSourceGeneration {
            items: 34,
            input: 7,
        };
        let resampleable = FullscreenPaintResource::resampleable(5, source, generation)
            .with_test_script_content_proof(proof.clone());
        assert_eq!(resampleable.test_script_content_proof(), Some(&proof));

        let releaser = Arc::new(CountingReleaser(AtomicUsize::new(0)));
        let output = Arc::new(LanczosOutput {
            texture_id_lease: NativeTextureIdLease {
                texture_id: egui::TextureId::User(8128),
                releaser: releaser.clone(),
            },
            key: LanczosCacheKey {
                source_id: FullscreenPaintSourceId::Page(5),
                source_texture_id: resampleable.source_texture_id(),
                generation,
                target_size: [1, 1],
                smoothing_percent: 0,
                scale_branch: FullscreenPaintScaleBranch::DownscaleLanczos,
                source_region: LanczosSourceRegionKey::Full,
            },
            _texture: None,
        });
        let lanczos = resampleable.with_lanczos(output, 0);
        assert_eq!(lanczos.test_script_content_proof(), Some(&proof));
        let presentation = lanczos.capture_presentation(&context);
        assert_eq!(presentation.test_script_content_proof(), Some(&proof));
        let payload = FullscreenPaintPayload::PresentationMaterial(presentation);
        assert_eq!(payload.test_script_content_proof(), Some(&proof));

        let restored = lanczos.original_resampleable();
        assert_eq!(restored.test_script_content_proof(), Some(&proof));
        assert!(matches!(
            restored,
            FullscreenPaintResource::Resampleable {
                source_id: FullscreenPaintSourceId::Page(5),
                generation: restored_generation,
                ..
            } if restored_generation == generation
        ));
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

    fn native_texture_batch_output(
        texture_id: egui::TextureId,
        owner: Arc<NativeTextureIdLease>,
        clip_rect: egui::Rect,
    ) -> (egui::Context, egui::FullOutput) {
        let ctx = egui::Context::default();
        let mut raw = egui::RawInput::default();
        raw.screen_rect = Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(64.0, 64.0),
        ));
        let output = ctx.run_pass(raw, |ctx| {
            let painter = ctx.debug_painter().with_clip_rect(clip_rect);
            painter.image(
                texture_id,
                egui::Rect::from_min_size(egui::pos2(8.0, 9.0), egui::vec2(16.0, 18.0)),
                egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
                egui::Color32::WHITE,
            );
            retain_owner_for_fullscreen_paint_batch(&painter, Arc::clone(&owner));
        });
        drop(owner);
        (ctx, output)
    }

    fn tessellated_native_texture_batch(
        texture_id: egui::TextureId,
        owner: Arc<NativeTextureIdLease>,
    ) -> Vec<egui::ClippedPrimitive> {
        let clip_rect = egui::Rect::from_min_max(egui::pos2(3.0, 5.0), egui::pos2(41.0, 43.0));
        let (ctx, mut output) = native_texture_batch_output(texture_id, owner, clip_rect);
        let primitives =
            ctx.tessellate(std::mem::take(&mut output.shapes), output.pixels_per_point);
        output.drop_without_applying_deltas();
        assert!(primitives.iter().any(|primitive| {
            matches!(
                &primitive.primitive,
                egui::epaint::Primitive::Mesh(mesh) if mesh.texture_id == texture_id
            )
        }));
        let callback = primitives
            .iter()
            .find_map(|primitive| match &primitive.primitive {
                egui::epaint::Primitive::Callback(callback) => Some((primitive, callback)),
                egui::epaint::Primitive::Mesh(_) => None,
            })
            .expect("paint-batch lifetime callback");
        assert_eq!(callback.1.rect, egui::Rect::ZERO);
        assert_eq!(callback.0.clip_rect, clip_rect);
        primitives
    }

    #[test]
    fn native_texture_lease_survives_shapes_and_tessellated_batch() {
        let releaser = Arc::new(CountingReleaser(AtomicUsize::new(0)));
        let lease = Arc::new(NativeTextureIdLease {
            texture_id: egui::TextureId::User(43),
            releaser: releaser.clone(),
        });
        let clip_rect = egui::Rect::from_min_max(egui::pos2(3.0, 5.0), egui::pos2(41.0, 43.0));
        let (ctx, mut output) =
            native_texture_batch_output(lease.texture_id, Arc::clone(&lease), clip_rect);
        drop(lease);
        assert_eq!(releaser.0.load(Ordering::Relaxed), 0);
        let primitives =
            ctx.tessellate(std::mem::take(&mut output.shapes), output.pixels_per_point);
        output.drop_without_applying_deltas();
        assert_eq!(releaser.0.load(Ordering::Relaxed), 0);
        drop(primitives);
        assert_eq!(releaser.0.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn dropping_unrendered_full_output_releases_native_texture_lease() {
        let releaser = Arc::new(CountingReleaser(AtomicUsize::new(0)));
        let lease = Arc::new(NativeTextureIdLease {
            texture_id: egui::TextureId::User(47),
            releaser: releaser.clone(),
        });
        let (_, output) = native_texture_batch_output(
            lease.texture_id,
            Arc::clone(&lease),
            egui::Rect::EVERYTHING,
        );
        drop(lease);
        assert_eq!(releaser.0.load(Ordering::Relaxed), 0);
        output.drop_without_applying_deltas();
        assert_eq!(releaser.0.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn empty_clip_discards_mesh_and_callback_and_releases_lease() {
        let releaser = Arc::new(CountingReleaser(AtomicUsize::new(0)));
        let lease = Arc::new(NativeTextureIdLease {
            texture_id: egui::TextureId::User(48),
            releaser: releaser.clone(),
        });
        let empty_clip = egui::Rect::from_min_max(egui::pos2(7.0, 7.0), egui::pos2(7.0, 7.0));
        let (ctx, mut output) =
            native_texture_batch_output(lease.texture_id, Arc::clone(&lease), empty_clip);
        drop(lease);
        assert_eq!(releaser.0.load(Ordering::Relaxed), 0);
        let primitives =
            ctx.tessellate(std::mem::take(&mut output.shapes), output.pixels_per_point);
        output.drop_without_applying_deltas();
        assert!(primitives.is_empty());
        assert_eq!(releaser.0.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn shared_native_texture_lease_waits_for_both_paint_batches() {
        let releaser = Arc::new(CountingReleaser(AtomicUsize::new(0)));
        let lease = Arc::new(NativeTextureIdLease {
            texture_id: egui::TextureId::User(44),
            releaser: releaser.clone(),
        });
        let batch_a = tessellated_native_texture_batch(lease.texture_id, Arc::clone(&lease));
        let batch_b = tessellated_native_texture_batch(lease.texture_id, Arc::clone(&lease));
        drop(lease);
        drop(batch_a);
        assert_eq!(releaser.0.load(Ordering::Relaxed), 0);
        drop(batch_b);
        assert_eq!(releaser.0.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn independent_native_texture_leases_retire_with_their_own_paint_batches() {
        let releaser_a = Arc::new(CountingReleaser(AtomicUsize::new(0)));
        let releaser_b = Arc::new(CountingReleaser(AtomicUsize::new(0)));
        let lease_a = Arc::new(NativeTextureIdLease {
            texture_id: egui::TextureId::User(45),
            releaser: releaser_a.clone(),
        });
        let lease_b = Arc::new(NativeTextureIdLease {
            texture_id: egui::TextureId::User(46),
            releaser: releaser_b.clone(),
        });
        let batch_a = tessellated_native_texture_batch(lease_a.texture_id, Arc::clone(&lease_a));
        let batch_b = tessellated_native_texture_batch(lease_b.texture_id, Arc::clone(&lease_b));
        drop(lease_a);
        drop(lease_b);
        drop(batch_a);
        assert_eq!(releaser_a.0.load(Ordering::Relaxed), 1);
        assert_eq!(releaser_b.0.load(Ordering::Relaxed), 0);
        drop(batch_b);
        assert_eq!(releaser_b.0.load(Ordering::Relaxed), 1);
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

    fn back_fake_entry(cache: &mut GpuLanczosCache, key: LanczosCacheKey) {
        cache.entries.insert(
            key,
            LanczosCacheEntry {
                output: Arc::new(LanczosOutput {
                    texture_id_lease: NativeTextureIdLease {
                        texture_id: egui::TextureId::User(9900),
                        releaser: Arc::new(CountingReleaser(AtomicUsize::new(0))),
                    },
                    key,
                    _texture: None,
                }),
                last_used: 17,
            },
        );
    }

    fn back_plan(key: LanczosCacheKey, smoothing_percent: u32) -> NativePrewarmPlan {
        NativePrewarmPlan {
            key,
            work: work_plan_for_key([16, 24], key).unwrap(),
            smoothing_percent,
        }
    }

    #[test]
    fn back_native_no_removal_covers_all_invalidation_and_pruning_rules_purely() {
        let base = LanczosCacheKey {
            source_id: FullscreenPaintSourceId::Page(3),
            source_texture_id: egui::TextureId::Managed(7),
            generation: FullscreenPaintSourceGeneration { items: 4, input: 8 },
            target_size: [32, 48],
            smoothing_percent: 0,
            scale_branch: FullscreenPaintScaleBranch::UpscaleLanczos,
            source_region: LanczosSourceRegionKey::Full,
        };
        let plan = back_plan(base, 50);
        let mut cache = GpuLanczosCache::default();
        assert!(
            cache.prewarm_no_removal_admission(plan).is_ok(),
            "empty cache can synchronize smoothing"
        );
        back_fake_entry(
            &mut cache,
            LanczosCacheKey {
                source_id: FullscreenPaintSourceId::Page(99),
                ..base
            },
        );
        cache.use_clock = 29;
        assert_eq!(
            cache.prewarm_no_removal_admission(plan),
            Err(NativePrewarmUnavailable::CacheSmoothingClear)
        );
        assert_eq!(
            (
                cache.entries.len(),
                cache.active_smoothing_percent,
                cache.use_clock
            ),
            (1, 0, 29)
        );
        cache.active_smoothing_percent = 50;
        assert!(
            cache.prewarm_no_removal_admission(plan).is_ok(),
            "other sources do not trigger source replacement"
        );
        for old in [
            LanczosCacheKey {
                source_texture_id: egui::TextureId::Managed(8),
                ..base
            },
            LanczosCacheKey {
                generation: FullscreenPaintSourceGeneration { items: 4, input: 7 },
                ..base
            },
        ] {
            cache.entries.clear();
            back_fake_entry(&mut cache, old);
            assert_eq!(
                cache.prewarm_no_removal_admission(plan),
                Err(NativePrewarmUnavailable::CacheSourceReplacement)
            );
            assert_eq!(cache.entries.len(), 1);
        }
        cache.entries.clear();
        for target_size in [[34, 50], [36, 52]] {
            back_fake_entry(
                &mut cache,
                LanczosCacheKey {
                    target_size,
                    ..base
                },
            );
        }
        assert_eq!(
            cache.prewarm_no_removal_admission(plan),
            Err(NativePrewarmUnavailable::CacheCapacity)
        );
        cache.entries.clear();
        for idx in 0..MAX_CACHE_ENTRIES {
            back_fake_entry(
                &mut cache,
                LanczosCacheKey {
                    source_id: FullscreenPaintSourceId::Page(idx + 100),
                    ..base
                },
            );
        }
        assert_eq!(
            cache.prewarm_no_removal_admission(plan),
            Err(NativePrewarmUnavailable::CacheCapacity)
        );
        cache.entries.clear();
        for idx in 0..2 {
            back_fake_entry(
                &mut cache,
                LanczosCacheKey {
                    source_id: FullscreenPaintSourceId::Page(idx + 100),
                    target_size: [4096, 4096],
                    ..base
                },
            );
        }
        assert_eq!(
            cache.prewarm_no_removal_admission(plan),
            Err(NativePrewarmUnavailable::CacheCapacity)
        );
    }

    #[test]
    fn back_native_execution_rechecks_no_removal_before_backing_claim_or_mutation() {
        let mut descriptor = wgpu::InstanceDescriptor::new_without_display_handle();
        descriptor.backend_options.dx12.shader_compiler = wgpu::Dx12Compiler::Fxc;
        let instance = wgpu::Instance::new(descriptor);
        let adapter = pollster::block_on(instance.request_adapter(&Default::default())).unwrap();
        let (device, queue) =
            pollster::block_on(adapter.request_device(&Default::default())).unwrap();
        let target_format = wgpu::TextureFormat::Rgba8Unorm;
        let renderer = egui_wgpu::Renderer::new(&device, target_format, Default::default());
        let rs = egui_wgpu::RenderState {
            instance,
            adapter,
            available_adapters: Vec::new(),
            device,
            queue,
            target_format,
            renderer: Arc::new(egui::epaint::mutex::RwLock::new(renderer)),
        };
        let ctx = egui::Context::default();
        let tex = metadata_only_texture(&ctx, [16, 24], egui::TextureOptions::LINEAR);
        let resource = FullscreenPaintResource::resampleable(
            3,
            tex,
            FullscreenPaintSourceGeneration { items: 4, input: 8 },
        );
        let key = resource_lanczos_key(
            &resource,
            0.5,
            1.0,
            None,
            PostFilter::None,
            0,
            AnimeUpscaleSourceLimit::Px4096,
        )
        .unwrap();
        let plan = back_plan(key, 0);
        let mut cache = GpuLanczosCache::default();
        assert!(cache.prewarm_no_removal_admission(plan).is_ok());
        let old = LanczosCacheKey {
            source_texture_id: egui::TextureId::Managed(9999),
            ..key
        };
        back_fake_entry(&mut cache, old); // changes after caller's pure preflight
        let claimed = std::cell::Cell::new(false);
        assert!(matches!(
            cache.execute_prewarm_with_policy(
                &rs,
                &resource,
                plan,
                NativePrewarmPolicy::NoRemoval,
                |_| {
                    claimed.set(true);
                    true
                }
            ),
            NativePrewarmExecution::Unavailable(NativePrewarmUnavailable::CacheSourceReplacement)
        ));
        assert!(!claimed.get());
        assert_eq!(cache.use_clock, 0);
        assert_eq!(cache.entries.len(), 1);
        cache.active_smoothing_percent = 50;
        assert!(matches!(
            cache.execute_prewarm_with_policy(
                &rs,
                &resource,
                plan,
                NativePrewarmPolicy::NoRemoval,
                |_| {
                    claimed.set(true);
                    true
                }
            ),
            NativePrewarmExecution::Unavailable(NativePrewarmUnavailable::CacheSmoothingClear)
        ));
        assert!(!claimed.get());
        assert_eq!(cache.active_smoothing_percent, 50);
        assert_eq!(cache.use_clock, 0);
        assert!(cache.entries.contains_key(&old));
        cache.active_smoothing_percent = 0;
        rs.renderer.write().update_texture(
            &rs.device,
            &rs.queue,
            resource.source_texture_id(),
            &egui::epaint::ImageDelta::full(
                egui::ColorImage::filled([16, 24], egui::Color32::WHITE),
                egui::TextureOptions::LINEAR,
            ),
        );
        assert!(matches!(
            cache.execute_prewarm(&rs, &resource, plan, |_| true),
            NativePrewarmExecution::Submitted { .. }
        ));
        assert!(!cache.entries.contains_key(&old));
        assert!(cache.entries.contains_key(&key));
        let mut empty = GpuLanczosCache::default();
        empty.active_smoothing_percent = 50;
        assert!(matches!(
            empty.execute_prewarm_with_policy(
                &rs,
                &resource,
                plan,
                NativePrewarmPolicy::NoRemoval,
                |_| true
            ),
            NativePrewarmExecution::Submitted { .. }
        ));
        assert_eq!(empty.active_smoothing_percent, 0);
        assert!(empty.entries.contains_key(&key));
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
    fn still_nis_uniform_keeps_source_exterior_opaque_black() {
        let bytes = pack_nis_params_uniform(NisUpscalePlan {
            source_size: [101, 202],
            target_size: [303, 404],
            source_region_px: [11.5, 12.5, 13.5, 14.5],
            texture_fetches: 0,
        });
        assert_eq!(bytes.len(), NIS_PARAMS_UNIFORM_SIZE);
        for (offset, expected) in [
            (64, 0.0_f32.to_ne_bytes()),
            (68, 0.0_f32.to_ne_bytes()),
            (72, 0.0_f32.to_ne_bytes()),
            (76, 1.0_f32.to_ne_bytes()),
        ] {
            assert_eq!(&bytes[offset..offset + 4], &expected);
        }
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
