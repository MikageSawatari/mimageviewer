//! Display-resolution Lanczos3 resolve for the native video presenter.
//!
//! The two passes keep filtering separable even for deep downscales. The first
//! pass also materializes the normalized display orientation, so the final swap
//! chain is exactly the physical video display rectangle and DComp only places
//! that rectangle.

#[cfg(test)]
use windows::Win32::Graphics::Direct3D::Fxc::D3DCompile;
use windows::Win32::Graphics::Direct3D::{
    D3D_DRIVER_TYPE_UNKNOWN, D3D_FEATURE_LEVEL, D3D_FEATURE_LEVEL_11_0,
    D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST,
};
#[cfg(test)]
use windows::Win32::Graphics::Direct3D::{ID3DBlob, ID3DInclude};
use windows::Win32::Graphics::Direct3D11::{
    D3D11_BIND_CONSTANT_BUFFER, D3D11_BIND_RENDER_TARGET, D3D11_BIND_SHADER_RESOURCE,
    D3D11_BUFFER_DESC, D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_QUERY_DATA_TIMESTAMP_DISJOINT,
    D3D11_QUERY_DESC, D3D11_QUERY_TIMESTAMP, D3D11_QUERY_TIMESTAMP_DISJOINT, D3D11_SDK_VERSION,
    D3D11_TEXTURE2D_DESC, D3D11_USAGE_DEFAULT, D3D11_VIEWPORT, D3D11CreateDevice, ID3D11Buffer,
    ID3D11Device1, ID3D11DeviceContext, ID3D11PixelShader, ID3D11Query, ID3D11RenderTargetView,
    ID3D11Resource, ID3D11ShaderResourceView, ID3D11Texture2D, ID3D11VertexShader,
};
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_FORMAT_R16G16B16A16_FLOAT, DXGI_SAMPLE_DESC,
};
use windows::Win32::Graphics::Dxgi::{IDXGIAdapter, IDXGIDevice};
use windows::core::Interface;
#[cfg(test)]
use windows::core::PCSTR;

use crate::gpu_anime4k::{Anime4kPassInput, Anime4kVariant};
use crate::settings::VideoScaleFilter;
use crate::video::anime4k_policy::VideoAnime4kMeasurementPoint;
use crate::video::anime4k_policy::VideoAnime4kVariant;
use crate::video::display_metadata::VideoOrientation;
use crate::video::zoom_view::VideoZoomSourceRect;

/// ビルド時に FXC で DXBC 化した resample シェーダ
/// ([shaders/video_resample.hlsl](shaders/video_resample.hlsl))。NIS は WGSL から naga で
/// HLSL に変換したものを、同じくビルド時に DXBC 化してある。実行時 `D3DCompile` は
/// NIS だけで 2.1 秒かかり、placement 切替のたびに払っていた (backlog §1.122)。
const RESAMPLE_VS_MAIN: &[u8] =
    include_bytes!(concat!(env!("OUT_DIR"), "/video_resample_vs_main.cso"));
const RESAMPLE_PS_HORIZONTAL: &[u8] = include_bytes!(concat!(
    env!("OUT_DIR"),
    "/video_resample_ps_horizontal.cso"
));
const RESAMPLE_PS_VERTICAL: &[u8] =
    include_bytes!(concat!(env!("OUT_DIR"), "/video_resample_ps_vertical.cso"));
const RESAMPLE_PS_NEAREST: &[u8] =
    include_bytes!(concat!(env!("OUT_DIR"), "/video_resample_ps_nearest.cso"));
const NIS_FS_NIS: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/video_nis_fs_nis.cso"));

#[repr(C)]
#[derive(Clone, Copy)]
struct ResampleConstants {
    source_target: [f32; 4],
    axis_filter: [f32; 4],
    source_region: [f32; 4],
    inverse_axes: [f32; 4],
    inverse_offset: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy)]
struct NisConstants {
    target_size: [u32; 2],
    source_size: [u32; 2],
    source_origin: [f32; 2],
    source_extent: [f32; 2],
    inverse_x: [f32; 2],
    inverse_y: [f32; 2],
    inverse_offset: [f32; 2],
    _padding: [f32; 2],
}

#[repr(C)]
#[derive(Clone, Copy)]
struct Anime4kConstants {
    output_size: [u32; 2],
    input_size: [u32; 2],
    input_origin: [i32; 2],
    process_origin: [i32; 2],
    source_size: [u32; 2],
    process_size: [u32; 2],
    source_region: [f32; 4],
    inverse_x: [f32; 2],
    inverse_y: [f32; 2],
    inverse_offset: [f32; 2],
    _padding: [f32; 2],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum VideoResampleMode {
    Lanczos3 { smoothing_percent: u32 },
    Nis,
    Nearest,
    Anime4k { variant: VideoAnime4kVariant },
}

pub(super) fn select_video_resample_mode(
    filter: VideoScaleFilter,
    source_axis_width: f32,
    source_axis_height: f32,
    target_width: u32,
    target_height: u32,
    smoothing_percent: u32,
    anime4k_variant: Option<VideoAnime4kVariant>,
) -> Option<VideoResampleMode> {
    if filter == VideoScaleFilter::OsDefault {
        return None;
    }
    let downscaling =
        (target_width as f32) < source_axis_width || (target_height as f32) < source_axis_height;
    Some(match (filter, downscaling) {
        (VideoScaleFilter::Sharp, false) => VideoResampleMode::Nis,
        (VideoScaleFilter::Nearest, false) => VideoResampleMode::Nearest,
        (VideoScaleFilter::Anime, false) => anime4k_variant
            .map(|variant| VideoResampleMode::Anime4k { variant })
            .unwrap_or(VideoResampleMode::Lanczos3 { smoothing_percent }),
        _ => VideoResampleMode::Lanczos3 { smoothing_percent },
    })
}

struct IntermediateTarget {
    width: u32,
    height: u32,
    _texture: ID3D11Texture2D,
    shader_view: ID3D11ShaderResourceView,
    render_target: ID3D11RenderTargetView,
}

struct VideoAnime4kBytecodeVariant {
    variant: Anime4kVariant,
    convolution: &'static [&'static [u8]],
    resolve: &'static [u8],
}

include!(concat!(env!("OUT_DIR"), "/video_anime4k_bytecode.rs"));

struct VideoAnime4kPipeline {
    variant: Anime4kVariant,
    convolution_shaders: Vec<ID3D11PixelShader>,
    resolve_shader: ID3D11PixelShader,
    convolution_constants: ID3D11Buffer,
    resolve_constants: ID3D11Buffer,
    intermediates: Vec<IntermediateTarget>,
}

pub(super) enum VideoResamplePrepareError {
    IntermediateCreation(String),
    Anime4kPipelineUnavailable {
        variant: Anime4kVariant,
        error: String,
    },
    Anime4kIntermediateCreationFailed {
        variant: Anime4kVariant,
        pass_index: usize,
        width: u32,
        height: u32,
        error: String,
    },
}

pub(super) struct VideoResamplePipeline {
    vertex_shader: ID3D11VertexShader,
    horizontal_shader: ID3D11PixelShader,
    vertical_shader: ID3D11PixelShader,
    nearest_shader: ID3D11PixelShader,
    nis_shader: ID3D11PixelShader,
    constants: ID3D11Buffer,
    nis_constants: ID3D11Buffer,
    intermediate: Option<IntermediateTarget>,
    anime4k: Vec<VideoAnime4kPipeline>,
    anime4k_errors: Vec<(VideoAnime4kVariant, String)>,
}

fn gpu_anime4k_variant(variant: VideoAnime4kVariant) -> Anime4kVariant {
    match variant {
        VideoAnime4kVariant::Small => Anime4kVariant::Small,
        VideoAnime4kVariant::Medium => Anime4kVariant::Medium,
        VideoAnime4kVariant::Large => Anime4kVariant::Large,
        VideoAnime4kVariant::VeryLarge => Anime4kVariant::VeryLarge,
        VideoAnime4kVariant::UltraLarge => Anime4kVariant::UltraLarge,
    }
}

impl VideoAnime4kPipeline {
    fn new(device: &ID3D11Device1, variant: Anime4kVariant) -> Result<Self, String> {
        let bytecode = VIDEO_ANIME4K_BYTECODE_VARIANTS
            .iter()
            .find(|data| data.variant == variant)
            .ok_or_else(|| format!("embedded bytecode is missing for {}", variant.label()))?;
        if bytecode.convolution.len() != variant.intermediate_count() {
            return Err(format!(
                "{} bytecode/topology mismatch: shaders={} intermediates={}",
                variant.label(),
                bytecode.convolution.len(),
                variant.intermediate_count()
            ));
        }
        if variant.pass_inputs().len() != bytecode.convolution.len() + 1 {
            return Err(format!(
                "{} bytecode/topology mismatch: passes={} bytecode={}",
                variant.label(),
                variant.pass_inputs().len(),
                bytecode.convolution.len() + 1
            ));
        }

        let load_started = std::time::Instant::now();
        let mut convolution_shaders = Vec::with_capacity(bytecode.convolution.len());
        for (pass_index, shader_bytecode) in bytecode.convolution.iter().enumerate() {
            let mut shader = None;
            unsafe {
                device
                    .CreatePixelShader(shader_bytecode, None, Some(&mut shader))
                    .map_err(|error| {
                        format!(
                            "CreatePixelShader {} pass {pass_index}: {error:?}",
                            variant.label()
                        )
                    })?;
            }
            convolution_shaders.push(shader.ok_or_else(|| {
                format!(
                    "CreatePixelShader {} pass {pass_index} returned null",
                    variant.label()
                )
            })?);
        }
        let mut resolve_shader = None;
        unsafe {
            device
                .CreatePixelShader(bytecode.resolve, None, Some(&mut resolve_shader))
                .map_err(|error| {
                    format!("CreatePixelShader {} resolve: {error:?}", variant.label())
                })?;
        }
        let resolve_shader = resolve_shader.ok_or_else(|| {
            format!(
                "CreatePixelShader {} resolve returned null",
                variant.label()
            )
        })?;
        let convolution_constants = create_constant_buffer::<Anime4kConstants>(
            device,
            &format!("{} convolution constants", variant.label()),
        )?;
        let resolve_constants = create_constant_buffer::<Anime4kConstants>(
            device,
            &format!("{} resolve constants", variant.label()),
        )?;
        let load_ms = load_started.elapsed().as_secs_f64() * 1000.0;
        let bytecode_bytes = bytecode
            .convolution
            .iter()
            .map(|bytes| bytes.len() as u64)
            .sum::<u64>()
            .saturating_add(bytecode.resolve.len() as u64);
        crate::logger::log(format!(
            "native-presenter: Anime4K bytecode loaded variant={} shaders={} bytes={} load_ms={load_ms:.3}",
            variant.label(),
            bytecode.convolution.len() + 1,
            bytecode_bytes
        ));
        if crate::perf::is_enabled() {
            crate::perf::event(
                "native_presenter",
                "video_anime4k_bytecode_loaded",
                None,
                0,
                &[
                    ("variant", serde_json::Value::from(variant.label())),
                    (
                        "shader_count",
                        serde_json::Value::from((bytecode.convolution.len() + 1) as i64),
                    ),
                    ("bytecode_bytes", serde_json::Value::from(bytecode_bytes)),
                    ("load_ms", serde_json::Value::from(load_ms)),
                ],
            );
        }
        Ok(Self {
            variant,
            convolution_shaders,
            resolve_shader,
            convolution_constants,
            resolve_constants,
            intermediates: Vec::new(),
        })
    }

    fn prepare(
        &mut self,
        device: &ID3D11Device1,
        width: u32,
        height: u32,
    ) -> Result<(), VideoResamplePrepareError> {
        let width = width.max(1);
        let height = height.max(1);
        if self.intermediates.len() == self.variant.intermediate_count()
            && self
                .intermediates
                .iter()
                .all(|target| target.width == width && target.height == height)
        {
            return Ok(());
        }
        let mut prepared = Vec::with_capacity(self.variant.intermediate_count());
        for pass_index in 0..self.variant.intermediate_count() {
            let target = create_intermediate_target(device, width, height).map_err(|error| {
                VideoResamplePrepareError::Anime4kIntermediateCreationFailed {
                    variant: self.variant,
                    pass_index,
                    width,
                    height,
                    error,
                }
            })?;
            prepared.push(target);
        }
        self.intermediates = prepared;
        Ok(())
    }

    fn intermediate_vram_bytes(&self) -> u64 {
        self.intermediates.iter().fold(0_u64, |total, target| {
            total.saturating_add(u64::from(target.width) * u64::from(target.height) * 8)
        })
    }
}

impl VideoResamplePipeline {
    pub(super) fn new(device: &ID3D11Device1) -> Result<Self, String> {
        let mut vertex_shader = None;
        let mut horizontal_shader = None;
        let mut vertical_shader = None;
        let mut nearest_shader = None;
        let mut nis_shader = None;
        let mut constants = None;
        let mut nis_constants = None;
        unsafe {
            device
                .CreateVertexShader(RESAMPLE_VS_MAIN, None, Some(&mut vertex_shader))
                .map_err(|error| format!("CreateVertexShader resample: {error:?}"))?;
            device
                .CreatePixelShader(RESAMPLE_PS_HORIZONTAL, None, Some(&mut horizontal_shader))
                .map_err(|error| format!("CreatePixelShader resample horizontal: {error:?}"))?;
            device
                .CreatePixelShader(RESAMPLE_PS_VERTICAL, None, Some(&mut vertical_shader))
                .map_err(|error| format!("CreatePixelShader resample vertical: {error:?}"))?;
            device
                .CreatePixelShader(RESAMPLE_PS_NEAREST, None, Some(&mut nearest_shader))
                .map_err(|error| format!("CreatePixelShader nearest: {error:?}"))?;
            device
                .CreatePixelShader(NIS_FS_NIS, None, Some(&mut nis_shader))
                .map_err(|error| format!("CreatePixelShader NIS: {error:?}"))?;
            let constants_desc = D3D11_BUFFER_DESC {
                ByteWidth: std::mem::size_of::<ResampleConstants>() as u32,
                Usage: D3D11_USAGE_DEFAULT,
                BindFlags: D3D11_BIND_CONSTANT_BUFFER.0 as u32,
                ..Default::default()
            };
            device
                .CreateBuffer(&constants_desc, None, Some(&mut constants))
                .map_err(|error| format!("CreateBuffer resample constants: {error:?}"))?;
            let nis_constants_desc = D3D11_BUFFER_DESC {
                ByteWidth: std::mem::size_of::<NisConstants>() as u32,
                Usage: D3D11_USAGE_DEFAULT,
                BindFlags: D3D11_BIND_CONSTANT_BUFFER.0 as u32,
                ..Default::default()
            };
            device
                .CreateBuffer(&nis_constants_desc, None, Some(&mut nis_constants))
                .map_err(|error| format!("CreateBuffer NIS constants: {error:?}"))?;
        }
        // Anime4K is optional within the resample pipeline so a device-specific
        // shader creation failure does not disable Standard/NIS/Nearest. The
        // retained typed error is surfaced if Anime is actually selected.
        let mut anime4k = Vec::new();
        let mut anime4k_errors = Vec::new();
        for video_variant in VideoAnime4kVariant::ALL {
            let variant = gpu_anime4k_variant(video_variant);
            match VideoAnime4kPipeline::new(device, variant) {
                Ok(pipeline) => anime4k.push(pipeline),
                Err(error) => {
                    crate::logger::log(format!(
                        "native-presenter: Anime4K pipeline unavailable variant={} error={error}",
                        variant.label()
                    ));
                    anime4k_errors.push((video_variant, error));
                }
            }
        }
        Ok(Self {
            vertex_shader: vertex_shader
                .ok_or_else(|| "CreateVertexShader resample returned null".to_string())?,
            horizontal_shader: horizontal_shader
                .ok_or_else(|| "CreatePixelShader resample horizontal returned null".to_string())?,
            vertical_shader: vertical_shader
                .ok_or_else(|| "CreatePixelShader resample vertical returned null".to_string())?,
            nearest_shader: nearest_shader
                .ok_or_else(|| "CreatePixelShader nearest returned null".to_string())?,
            nis_shader: nis_shader
                .ok_or_else(|| "CreatePixelShader NIS returned null".to_string())?,
            constants: constants
                .ok_or_else(|| "CreateBuffer resample constants returned null".to_string())?,
            nis_constants: nis_constants
                .ok_or_else(|| "CreateBuffer NIS constants returned null".to_string())?,
            intermediate: None,
            anime4k,
            anime4k_errors,
        })
    }

    pub(super) fn intermediate_dimensions(
        source_width: u32,
        source_height: u32,
        target_width: u32,
        orientation: VideoOrientation,
    ) -> (u32, u32) {
        let oriented_axis_y = if orientation.swaps_axes() {
            source_width
        } else {
            source_height
        };
        (target_width.max(1), oriented_axis_y.max(1))
    }

    pub(super) fn prepare(
        &mut self,
        device: &ID3D11Device1,
        source_width: u32,
        source_height: u32,
        target_width: u32,
        orientation: VideoOrientation,
        mode: VideoResampleMode,
    ) -> Result<(), VideoResamplePrepareError> {
        if let VideoResampleMode::Anime4k { variant } = mode {
            let gpu_variant = gpu_anime4k_variant(variant);
            let Some(index) = self
                .anime4k
                .iter()
                .position(|value| value.variant == gpu_variant)
            else {
                return Err(VideoResamplePrepareError::Anime4kPipelineUnavailable {
                    variant: gpu_variant,
                    error: self
                        .anime4k_errors
                        .iter()
                        .find(|(value, _)| *value == variant)
                        .map(|(_, error)| error.clone())
                        .unwrap_or_else(|| "pipeline was not created for this variant".to_string()),
                });
            };
            self.anime4k[index].prepare(device, source_width, source_height)?;
            for (other_index, pipeline) in self.anime4k.iter_mut().enumerate() {
                if other_index != index {
                    pipeline.intermediates.clear();
                }
            }
            return Ok(());
        }
        if !matches!(mode, VideoResampleMode::Lanczos3 { .. }) {
            return Ok(());
        }
        let (width, height) =
            Self::intermediate_dimensions(source_width, source_height, target_width, orientation);
        if self
            .intermediate
            .as_ref()
            .is_some_and(|target| target.width == width && target.height == height)
        {
            return Ok(());
        }
        self.intermediate = Some(
            create_intermediate_target(device, width, height)
                .map_err(VideoResamplePrepareError::IntermediateCreation)?,
        );
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn draw(
        &self,
        device: &ID3D11Device1,
        context: &ID3D11DeviceContext,
        source: &ID3D11Texture2D,
        target: &ID3D11Texture2D,
        source_width: u32,
        source_height: u32,
        target_width: u32,
        target_height: u32,
        orientation: VideoOrientation,
        source_rect: VideoZoomSourceRect,
        mode: VideoResampleMode,
    ) -> Result<(), String> {
        match mode {
            VideoResampleMode::Lanczos3 { smoothing_percent } => self.draw_lanczos(
                device,
                context,
                source,
                target,
                source_width,
                source_height,
                target_width,
                target_height,
                orientation,
                source_rect,
                smoothing_percent,
            ),
            VideoResampleMode::Nis | VideoResampleMode::Nearest => self.draw_single_pass(
                device,
                context,
                source,
                target,
                source_width,
                source_height,
                target_width,
                target_height,
                orientation,
                source_rect,
                mode,
            ),
            VideoResampleMode::Anime4k { variant } => self.draw_anime4k(
                device,
                context,
                source,
                target,
                source_width,
                source_height,
                target_width,
                target_height,
                orientation,
                source_rect,
                variant,
            ),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_lanczos(
        &self,
        device: &ID3D11Device1,
        context: &ID3D11DeviceContext,
        source: &ID3D11Texture2D,
        target: &ID3D11Texture2D,
        source_width: u32,
        source_height: u32,
        target_width: u32,
        target_height: u32,
        orientation: VideoOrientation,
        source_rect: VideoZoomSourceRect,
        smoothing_percent: u32,
    ) -> Result<(), String> {
        let (intermediate_width, intermediate_height) =
            Self::intermediate_dimensions(source_width, source_height, target_width, orientation);
        let intermediate = self
            .intermediate
            .as_ref()
            .filter(|value| {
                value.width == intermediate_width && value.height == intermediate_height
            })
            .ok_or_else(|| "resample intermediate was not prepared".to_string())?;
        let source_view = create_shader_view(device, source)?;
        let target_view = create_render_target(device, target)?;
        let mapping = inverse_orientation_mapping(source_width, source_height, orientation);
        let source_axis_x = source_rect.extent[0];
        let source_axis_y = source_rect.extent[1];
        let blur_factor = crate::settings::downscale_smoothing_blur_factor(smoothing_percent);
        let stretch = |source_axis: f32, target_axis: u32| {
            let ratio = source_axis / target_axis.max(1) as f32;
            if ratio > 1.0 {
                ratio * blur_factor
            } else {
                1.0
            }
        };
        let constants = ResampleConstants {
            source_target: [
                source_width.max(1) as f32,
                source_height.max(1) as f32,
                target_width.max(1) as f32,
                target_height.max(1) as f32,
            ],
            axis_filter: [
                mapping.source_axis_x as f32,
                mapping.source_axis_y as f32,
                stretch(source_axis_x, target_width),
                stretch(source_axis_y, target_height),
            ],
            source_region: [
                source_rect.origin[0],
                source_rect.origin[1],
                source_rect.extent[0],
                source_rect.extent[1],
            ],
            inverse_axes: [
                mapping.inverse_x[0],
                mapping.inverse_x[1],
                mapping.inverse_y[0],
                mapping.inverse_y[1],
            ],
            inverse_offset: [mapping.offset[0], mapping.offset[1], 0.0, 0.0],
        };

        unsafe {
            context.UpdateSubresource(
                &self.constants,
                0,
                None,
                (&constants as *const ResampleConstants).cast(),
                0,
                0,
            );
            context.IASetInputLayout(None);
            context.IASetPrimitiveTopology(D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
            context.VSSetShader(&self.vertex_shader, None);
            context.PSSetConstantBuffers(0, Some(&[Some(self.constants.clone())]));
            context.RSSetState(None);
            context.OMSetBlendState(None, None, u32::MAX);
            context.OMSetDepthStencilState(None, 0);

            context.PSSetShader(&self.horizontal_shader, None);
            context.PSSetShaderResources(0, Some(&[Some(source_view)]));
            context.RSSetViewports(Some(&[viewport(intermediate_width, intermediate_height)]));
            context.OMSetRenderTargets(Some(&[Some(intermediate.render_target.clone())]), None);
            context.Draw(3, 0);

            context.PSSetShaderResources(0, Some(&[None]));
            context.OMSetRenderTargets(None, None);
            context.PSSetShader(&self.vertical_shader, None);
            context.PSSetShaderResources(0, Some(&[Some(intermediate.shader_view.clone())]));
            context.RSSetViewports(Some(&[viewport(target_width, target_height)]));
            context.OMSetRenderTargets(Some(&[Some(target_view)]), None);
            context.Draw(3, 0);

            context.PSSetShaderResources(0, Some(&[None]));
            context.OMSetRenderTargets(None, None);
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_single_pass(
        &self,
        device: &ID3D11Device1,
        context: &ID3D11DeviceContext,
        source: &ID3D11Texture2D,
        target: &ID3D11Texture2D,
        source_width: u32,
        source_height: u32,
        target_width: u32,
        target_height: u32,
        orientation: VideoOrientation,
        source_rect: VideoZoomSourceRect,
        mode: VideoResampleMode,
    ) -> Result<(), String> {
        let source_view = create_shader_view(device, source)?;
        let target_view = create_render_target(device, target)?;
        let mapping = inverse_orientation_mapping(source_width, source_height, orientation);
        let common_constants = ResampleConstants {
            source_target: [
                source_width.max(1) as f32,
                source_height.max(1) as f32,
                target_width.max(1) as f32,
                target_height.max(1) as f32,
            ],
            axis_filter: [
                mapping.source_axis_x as f32,
                mapping.source_axis_y as f32,
                1.0,
                1.0,
            ],
            source_region: [
                source_rect.origin[0],
                source_rect.origin[1],
                source_rect.extent[0],
                source_rect.extent[1],
            ],
            inverse_axes: [
                mapping.inverse_x[0],
                mapping.inverse_x[1],
                mapping.inverse_y[0],
                mapping.inverse_y[1],
            ],
            inverse_offset: [mapping.offset[0], mapping.offset[1], 0.0, 0.0],
        };
        let nis_constants = NisConstants {
            target_size: [target_width.max(1), target_height.max(1)],
            source_size: [source_width.max(1), source_height.max(1)],
            source_origin: source_rect.origin,
            source_extent: source_rect.extent,
            inverse_x: mapping.inverse_x,
            inverse_y: mapping.inverse_y,
            inverse_offset: mapping.offset,
            _padding: [0.0, 0.0],
        };

        unsafe {
            context.IASetInputLayout(None);
            context.IASetPrimitiveTopology(D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
            context.RSSetState(None);
            context.OMSetBlendState(None, None, u32::MAX);
            context.OMSetDepthStencilState(None, 0);
            match mode {
                VideoResampleMode::Nearest => {
                    context.UpdateSubresource(
                        &self.constants,
                        0,
                        None,
                        (&common_constants as *const ResampleConstants).cast(),
                        0,
                        0,
                    );
                    context.VSSetShader(&self.vertex_shader, None);
                    context.PSSetConstantBuffers(0, Some(&[Some(self.constants.clone())]));
                    context.PSSetShader(&self.nearest_shader, None);
                }
                VideoResampleMode::Nis => {
                    context.UpdateSubresource(
                        &self.nis_constants,
                        0,
                        None,
                        (&nis_constants as *const NisConstants).cast(),
                        0,
                        0,
                    );
                    // The Naga-generated WGSL vertex shader emits a WebGPU-wound
                    // fullscreen triangle. D3D11's default rasterizer culls that
                    // winding, so use the native D3D fullscreen vertex shader. NIS
                    // only consumes SV_Position, making the two interfaces identical.
                    context.VSSetShader(&self.vertex_shader, None);
                    context.PSSetConstantBuffers(1, Some(&[Some(self.nis_constants.clone())]));
                    context.PSSetShader(&self.nis_shader, None);
                }
                VideoResampleMode::Lanczos3 { .. } => {
                    return Err("Lanczos3 entered the single-pass resampler".to_string());
                }
                VideoResampleMode::Anime4k { .. } => {
                    return Err("Anime4K entered the single-pass resampler".to_string());
                }
            }
            context.PSSetShaderResources(0, Some(&[Some(source_view)]));
            context.RSSetViewports(Some(&[viewport(target_width, target_height)]));
            context.OMSetRenderTargets(Some(&[Some(target_view)]), None);
            context.Draw(3, 0);

            context.PSSetShaderResources(0, Some(&[None]));
            context.OMSetRenderTargets(None, None);
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_anime4k(
        &self,
        device: &ID3D11Device1,
        context: &ID3D11DeviceContext,
        source: &ID3D11Texture2D,
        target: &ID3D11Texture2D,
        source_width: u32,
        source_height: u32,
        target_width: u32,
        target_height: u32,
        orientation: VideoOrientation,
        source_rect: VideoZoomSourceRect,
        variant: VideoAnime4kVariant,
    ) -> Result<(), String> {
        let gpu_variant = gpu_anime4k_variant(variant);
        let pipeline = self
            .anime4k
            .iter()
            .find(|value| value.variant == gpu_variant)
            .ok_or_else(|| {
                format!(
                    "{} pipeline was not prepared: {}",
                    gpu_variant.label(),
                    self.anime4k_errors
                        .iter()
                        .find(|(value, _)| *value == variant)
                        .map(|(_, error)| error.as_str())
                        .unwrap_or("variant mismatch")
                )
            })?;
        if pipeline.intermediates.len() != gpu_variant.intermediate_count() {
            return Err(format!(
                "{} intermediates were not prepared: expected={} actual={}",
                gpu_variant.label(),
                gpu_variant.intermediate_count(),
                pipeline.intermediates.len()
            ));
        }
        if pipeline
            .intermediates
            .iter()
            .any(|value| value.width != source_width || value.height != source_height)
        {
            return Err(format!(
                "{} intermediates do not match source {}x{}",
                gpu_variant.label(),
                source_width,
                source_height
            ));
        }

        let source_view = create_shader_view(device, source)?;
        let target_view = create_render_target(device, target)?;
        let mapping = inverse_orientation_mapping(source_width, source_height, orientation);
        let common = anime4k_video_constants(
            source_width,
            source_height,
            source_width,
            source_height,
            orientation,
            VideoZoomSourceRect {
                origin: [0.0, 0.0],
                extent: [mapping.source_axis_x as f32, mapping.source_axis_y as f32],
            },
        );
        let resolve = anime4k_video_constants(
            source_width,
            source_height,
            target_width,
            target_height,
            orientation,
            source_rect,
        );
        let pass_inputs = gpu_variant.pass_inputs();
        let input_binding_count = gpu_variant.input_binding_count();
        let unbound_views = vec![None; input_binding_count];

        unsafe {
            context.UpdateSubresource(
                &pipeline.convolution_constants,
                0,
                None,
                (&common as *const Anime4kConstants).cast(),
                0,
                0,
            );
            context.UpdateSubresource(
                &pipeline.resolve_constants,
                0,
                None,
                (&resolve as *const Anime4kConstants).cast(),
                0,
                0,
            );
            context.IASetInputLayout(None);
            context.IASetPrimitiveTopology(D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
            // Do not use the Naga-generated vs_main here. Its WebGPU winding is
            // culled by D3D11's default rasterizer. This is the same production
            // native fullscreen vertex shader used by the verified NIS path.
            context.VSSetShader(&self.vertex_shader, None);
            context.RSSetState(None);
            context.OMSetBlendState(None, None, u32::MAX);
            context.OMSetDepthStencilState(None, 0);
            context.PSSetShaderResources(0, Some(&unbound_views));
            context.OMSetRenderTargets(None, None);
            context.PSSetConstantBuffers(0, Some(&[Some(pipeline.convolution_constants.clone())]));
            context.RSSetViewports(Some(&[viewport(source_width, source_height)]));

            for pass_index in 0..gpu_variant.intermediate_count() {
                let views = anime4k_pass_views(
                    pass_inputs[pass_index],
                    input_binding_count,
                    &source_view,
                    &pipeline.intermediates,
                )?;
                context.PSSetShader(&pipeline.convolution_shaders[pass_index], None);
                context.PSSetShaderResources(0, Some(&views));
                context.OMSetRenderTargets(
                    Some(&[Some(
                        pipeline.intermediates[pass_index].render_target.clone(),
                    )]),
                    None,
                );
                context.Draw(3, 0);
                context.PSSetShaderResources(0, Some(&unbound_views));
                context.OMSetRenderTargets(None, None);
            }

            let views = anime4k_pass_views(
                pass_inputs
                    .last()
                    .expect("generated Anime4K topology has a resolve pass"),
                input_binding_count,
                &source_view,
                &pipeline.intermediates,
            )?;
            context.PSSetConstantBuffers(0, Some(&[Some(pipeline.resolve_constants.clone())]));
            context.PSSetShader(&pipeline.resolve_shader, None);
            context.PSSetShaderResources(0, Some(&views));
            context.RSSetViewports(Some(&[viewport(target_width, target_height)]));
            context.OMSetRenderTargets(Some(&[Some(target_view)]), None);
            context.Draw(3, 0);

            context.PSSetShaderResources(0, Some(&unbound_views));
            context.OMSetRenderTargets(None, None);
        }
        Ok(())
    }

    pub(super) fn intermediate_vram_bytes(&self) -> u64 {
        let lanczos = self
            .intermediate
            .as_ref()
            .map(|target| u64::from(target.width) * u64::from(target.height) * 8)
            .unwrap_or(0);
        lanczos.saturating_add(
            self.anime4k
                .iter()
                .map(VideoAnime4kPipeline::intermediate_vram_bytes)
                .sum::<u64>(),
        )
    }
}

#[derive(Clone, Copy)]
struct InverseOrientationMapping {
    source_axis_x: u32,
    source_axis_y: u32,
    inverse_x: [f32; 2],
    inverse_y: [f32; 2],
    offset: [f32; 2],
}

fn inverse_orientation_mapping(
    width: u32,
    height: u32,
    orientation: VideoOrientation,
) -> InverseOrientationMapping {
    let width = width.max(1);
    let height = height.max(1);
    let (m11, m12, m21, m22) = orientation.matrix_2x2();
    let max_x = i64::from(width - 1);
    let max_y = i64::from(height - 1);
    let corners = [(0_i64, 0_i64), (max_x, 0), (0, max_y), (max_x, max_y)];
    let transformed = corners.map(|(x, y)| {
        (
            x * i64::from(m11) + y * i64::from(m21),
            x * i64::from(m12) + y * i64::from(m22),
        )
    });
    let min_x = transformed.iter().map(|value| value.0).min().unwrap_or(0);
    let min_y = transformed.iter().map(|value| value.1).min().unwrap_or(0);
    let offset_x = min_x * i64::from(m11) + min_y * i64::from(m12);
    let offset_y = min_x * i64::from(m21) + min_y * i64::from(m22);
    InverseOrientationMapping {
        source_axis_x: if orientation.swaps_axes() {
            height
        } else {
            width
        },
        source_axis_y: if orientation.swaps_axes() {
            width
        } else {
            height
        },
        inverse_x: [f32::from(m11), f32::from(m21)],
        inverse_y: [f32::from(m12), f32::from(m22)],
        offset: [offset_x as f32, offset_y as f32],
    }
}

fn anime4k_video_constants(
    source_width: u32,
    source_height: u32,
    output_width: u32,
    output_height: u32,
    orientation: VideoOrientation,
    source_rect: VideoZoomSourceRect,
) -> Anime4kConstants {
    let source_width = source_width.max(1);
    let source_height = source_height.max(1);
    let mapping = inverse_orientation_mapping(source_width, source_height, orientation);
    Anime4kConstants {
        output_size: [output_width.max(1), output_height.max(1)],
        input_size: [source_width, source_height],
        input_origin: [0, 0],
        process_origin: [0, 0],
        source_size: [source_width, source_height],
        process_size: [source_width, source_height],
        source_region: [
            source_rect.origin[0],
            source_rect.origin[1],
            source_rect.extent[0],
            source_rect.extent[1],
        ],
        inverse_x: mapping.inverse_x,
        inverse_y: mapping.inverse_y,
        inverse_offset: mapping.offset,
        _padding: [0.0, 0.0],
    }
}

fn viewport(width: u32, height: u32) -> D3D11_VIEWPORT {
    D3D11_VIEWPORT {
        TopLeftX: 0.0,
        TopLeftY: 0.0,
        Width: width.max(1) as f32,
        Height: height.max(1) as f32,
        MinDepth: 0.0,
        MaxDepth: 1.0,
    }
}

fn create_constant_buffer<T>(device: &ID3D11Device1, label: &str) -> Result<ID3D11Buffer, String> {
    let desc = D3D11_BUFFER_DESC {
        ByteWidth: std::mem::size_of::<T>() as u32,
        Usage: D3D11_USAGE_DEFAULT,
        BindFlags: D3D11_BIND_CONSTANT_BUFFER.0 as u32,
        ..Default::default()
    };
    let mut buffer = None;
    unsafe {
        device
            .CreateBuffer(&desc, None, Some(&mut buffer))
            .map_err(|error| format!("CreateBuffer {label}: {error:?}"))?;
    }
    buffer.ok_or_else(|| format!("CreateBuffer {label} returned null"))
}

fn create_intermediate_target(
    device: &ID3D11Device1,
    width: u32,
    height: u32,
) -> Result<IntermediateTarget, String> {
    let desc = D3D11_TEXTURE2D_DESC {
        Width: width.max(1),
        Height: height.max(1),
        MipLevels: 1,
        ArraySize: 1,
        Format: DXGI_FORMAT_R16G16B16A16_FLOAT,
        SampleDesc: DXGI_SAMPLE_DESC {
            Count: 1,
            Quality: 0,
        },
        Usage: D3D11_USAGE_DEFAULT,
        BindFlags: (D3D11_BIND_SHADER_RESOURCE | D3D11_BIND_RENDER_TARGET).0 as u32,
        ..Default::default()
    };
    let mut texture = None;
    unsafe {
        device
            .CreateTexture2D(&desc, None, Some(&mut texture))
            .map_err(|error| format!("CreateTexture2D resample intermediate: {error:?}"))?;
    }
    let texture =
        texture.ok_or_else(|| "CreateTexture2D resample intermediate returned null".to_string())?;
    let shader_view = create_shader_view(device, &texture)?;
    let render_target = create_render_target(device, &texture)?;
    Ok(IntermediateTarget {
        width: width.max(1),
        height: height.max(1),
        _texture: texture,
        shader_view,
        render_target,
    })
}

fn anime4k_pass_views(
    inputs: &[Anime4kPassInput],
    input_binding_count: usize,
    source: &ID3D11ShaderResourceView,
    intermediates: &[IntermediateTarget],
) -> Result<Vec<Option<ID3D11ShaderResourceView>>, String> {
    let selected = inputs
        .iter()
        .map(|input| match *input {
            Anime4kPassInput::Source => Ok(source.clone()),
            Anime4kPassInput::Intermediate(index) => intermediates
                .get(index)
                .map(|target| target.shader_view.clone())
                .ok_or_else(|| format!("Anime4K input references missing intermediate {index}")),
        })
        .collect::<Result<Vec<_>, _>>()?;
    let fallback = selected
        .first()
        .ok_or_else(|| "Anime4K pass has no inputs".to_string())?;
    if selected.len() > input_binding_count {
        return Err(format!(
            "Anime4K pass has {} inputs but only {input_binding_count} bindings",
            selected.len()
        ));
    }
    Ok((0..input_binding_count)
        .map(|binding| Some(selected.get(binding).unwrap_or(fallback).clone()))
        .collect())
}

/// 実行時 HLSL コンパイル。**production 経路では使わない** (シェーダは `build.rs` が
/// FXC で DXBC 化してある)。Anime4K の GPU 実測テストが、生成済み HLSL から
/// その場で pixel shader を作るために残してある (backlog §1.122)。
#[cfg(test)]
fn compile_shader_source(source: &[u8], entry: &str, target: &str) -> Result<ID3DBlob, String> {
    let mut bytecode = None;
    let mut errors = None;
    let entry = std::ffi::CString::new(entry).expect("static shader entry");
    let target = std::ffi::CString::new(target).expect("static shader target");
    let result = unsafe {
        D3DCompile(
            source.as_ptr().cast(),
            source.len(),
            PCSTR::null(),
            None,
            None::<&ID3DInclude>,
            PCSTR(entry.as_ptr().cast()),
            PCSTR(target.as_ptr().cast()),
            0,
            0,
            &mut bytecode,
            Some(&mut errors),
        )
    };
    if let Err(error) = result {
        let details = errors
            .as_ref()
            .map(blob_string)
            .filter(|message| !message.trim().is_empty())
            .unwrap_or_else(|| format!("{error:?}"));
        return Err(format!("D3DCompile {entry:?}/{target:?}: {details}"));
    }
    bytecode.ok_or_else(|| "D3DCompile returned null bytecode".to_string())
}

#[cfg(test)]
fn blob_bytes(blob: &ID3DBlob) -> &[u8] {
    unsafe {
        std::slice::from_raw_parts(blob.GetBufferPointer().cast::<u8>(), blob.GetBufferSize())
    }
}

#[cfg(test)]
fn blob_string(blob: &ID3DBlob) -> String {
    String::from_utf8_lossy(blob_bytes(blob))
        .trim_end_matches('\0')
        .to_string()
}

fn create_shader_view<T: Interface>(
    device: &ID3D11Device1,
    texture: &T,
) -> Result<ID3D11ShaderResourceView, String> {
    let resource: ID3D11Resource = texture
        .cast()
        .map_err(|error| format!("cast resample shader resource: {error:?}"))?;
    let mut view = None;
    unsafe {
        device
            .CreateShaderResourceView(&resource, None, Some(&mut view))
            .map_err(|error| format!("CreateShaderResourceView resample: {error:?}"))?;
    }
    view.ok_or_else(|| "CreateShaderResourceView resample returned null".to_string())
}

fn create_render_target<T: Interface>(
    device: &ID3D11Device1,
    texture: &T,
) -> Result<ID3D11RenderTargetView, String> {
    let resource: ID3D11Resource = texture
        .cast()
        .map_err(|error| format!("cast resample render target: {error:?}"))?;
    let mut view = None;
    unsafe {
        device
            .CreateRenderTargetView(&resource, None, Some(&mut view))
            .map_err(|error| format!("CreateRenderTargetView resample: {error:?}"))?;
    }
    view.ok_or_else(|| "CreateRenderTargetView resample returned null".to_string())
}

pub(super) fn measure_video_anime4k_on_adapter(
    adapter: &IDXGIAdapter,
    cancel: &std::sync::atomic::AtomicBool,
    mut progress: impl FnMut(u32, VideoAnime4kMeasurementPoint),
) -> Result<Vec<VideoAnime4kMeasurementPoint>, String> {
    use std::sync::atomic::Ordering;

    let mut base_device = None;
    let mut context = None;
    let mut feature_level = D3D_FEATURE_LEVEL::default();
    unsafe {
        D3D11CreateDevice(
            adapter,
            D3D_DRIVER_TYPE_UNKNOWN,
            windows::Win32::Foundation::HMODULE::default(),
            D3D11_CREATE_DEVICE_BGRA_SUPPORT,
            Some(&[D3D_FEATURE_LEVEL_11_0]),
            D3D11_SDK_VERSION,
            Some(&mut base_device),
            Some(&mut feature_level),
            Some(&mut context),
        )
        .map_err(|error| format!("Anime4K measurement device: {error:?}"))?;
    }
    if feature_level.0 < D3D_FEATURE_LEVEL_11_0.0 {
        return Err(format!(
            "Anime4K measurement needs D3D feature level 11_0, got {:?}",
            feature_level
        ));
    }
    let device: ID3D11Device1 = base_device
        .ok_or_else(|| "Anime4K measurement device returned null".to_string())?
        .cast()
        .map_err(|error| format!("Anime4K measurement ID3D11Device1: {error:?}"))?;
    let context = context.ok_or_else(|| "Anime4K measurement context returned null".to_string())?;
    if let Ok(dxgi_device) = device.cast::<IDXGIDevice>() {
        unsafe {
            // The playback presenter's device retains the default priority. The
            // benchmark is intentionally background GPU work.
            let _ = dxgi_device.SetGPUThreadPriority(-7);
        }
    }
    let mut pipeline = VideoResamplePipeline::new(&device)?;
    let cases = [(960, 540, 1920, 1080), (1920, 1080, 3840, 2160)];
    let mut points = Vec::with_capacity(6);
    for variant in VideoAnime4kVariant::MEASURED {
        for (source_width, source_height, output_width, output_height) in cases {
            if cancel.load(Ordering::Acquire) {
                return Err("Anime4K measurement cancelled".to_string());
            }
            let measured = measure_video_anime4k_case(
                &device,
                &context,
                &mut pipeline,
                variant,
                source_width,
                source_height,
                output_width,
                output_height,
                cancel,
            );
            let gpu_time_us = match measured {
                Ok(value) => value,
                Err(error) => {
                    crate::logger::log(format!(
                        "native-presenter: Anime4K measurement case failed variant={} source={}x{} output={}x{} error={error}",
                        variant.label(),
                        source_width,
                        source_height,
                        output_width,
                        output_height
                    ));
                    if crate::perf::is_enabled() {
                        crate::perf::event(
                            "native_presenter",
                            "video_anime4k_measurement_case_failed",
                            None,
                            0,
                            &[
                                ("variant", serde_json::Value::from(variant.label())),
                                ("source_width", serde_json::Value::from(source_width)),
                                ("source_height", serde_json::Value::from(source_height)),
                                ("reason", serde_json::Value::from(error.clone())),
                            ],
                        );
                    }
                    return Err(format!(
                        "Anime4K {} measurement failed at {}x{}: {error}",
                        variant.label(),
                        source_width,
                        source_height
                    ));
                }
            };
            let point = VideoAnime4kMeasurementPoint {
                variant,
                source_width,
                source_height,
                output_width,
                output_height,
                gpu_time_us,
            };
            points.push(point);
            progress(points.len() as u32, point);
        }
    }
    Ok(points)
}

#[allow(clippy::too_many_arguments)]
fn measure_video_anime4k_case(
    device: &ID3D11Device1,
    context: &ID3D11DeviceContext,
    pipeline: &mut VideoResamplePipeline,
    variant: VideoAnime4kVariant,
    source_width: u32,
    source_height: u32,
    output_width: u32,
    output_height: u32,
    cancel: &std::sync::atomic::AtomicBool,
) -> Result<u64, String> {
    let source = create_measurement_texture(
        device,
        source_width,
        source_height,
        (D3D11_BIND_SHADER_RESOURCE | D3D11_BIND_RENDER_TARGET).0 as u32,
    )?;
    let target = create_measurement_texture(
        device,
        output_width,
        output_height,
        D3D11_BIND_RENDER_TARGET.0 as u32,
    )?;
    let source_target = create_render_target(device, &source)?;
    unsafe {
        context.ClearRenderTargetView(&source_target, &[0.25, 0.5, 0.75, 1.0]);
    }
    let mode = VideoResampleMode::Anime4k { variant };
    pipeline
        .prepare(
            device,
            source_width,
            source_height,
            output_width,
            VideoOrientation::IDENTITY,
            mode,
        )
        .map_err(|error| match error {
            VideoResamplePrepareError::IntermediateCreation(error) => error,
            VideoResamplePrepareError::Anime4kPipelineUnavailable { variant, error } => {
                format!("{} pipeline unavailable: {error}", variant.label())
            }
            VideoResamplePrepareError::Anime4kIntermediateCreationFailed {
                variant,
                pass_index,
                width,
                height,
                error,
            } => format!(
                "{} pass {pass_index} intermediate {width}x{height}: {error}",
                variant.label()
            ),
        })?;
    for _ in 0..3 {
        pipeline.draw(
            device,
            context,
            &source,
            &target,
            source_width,
            source_height,
            output_width,
            output_height,
            VideoOrientation::IDENTITY,
            VideoZoomSourceRect {
                origin: [0.0, 0.0],
                extent: [source_width as f32, source_height as f32],
            },
            mode,
        )?;
    }
    unsafe { context.Flush() };

    let disjoint = create_query(device, D3D11_QUERY_TIMESTAMP_DISJOINT)?;
    let start = create_query(device, D3D11_QUERY_TIMESTAMP)?;
    let end = create_query(device, D3D11_QUERY_TIMESTAMP)?;
    let mut samples = Vec::with_capacity(7);
    for _ in 0..7 {
        if cancel.load(std::sync::atomic::Ordering::Acquire) {
            return Err("Anime4K measurement cancelled".to_string());
        }
        unsafe {
            context.Begin(&disjoint);
            context.End(&start);
        }
        pipeline.draw(
            device,
            context,
            &source,
            &target,
            source_width,
            source_height,
            output_width,
            output_height,
            VideoOrientation::IDENTITY,
            VideoZoomSourceRect {
                origin: [0.0, 0.0],
                extent: [source_width as f32, source_height as f32],
            },
            mode,
        )?;
        unsafe {
            context.End(&end);
            context.End(&disjoint);
            context.Flush();
        }
        let start_tick: u64 = wait_query_data(context, &start, cancel)?;
        let end_tick: u64 = wait_query_data(context, &end, cancel)?;
        let timing: D3D11_QUERY_DATA_TIMESTAMP_DISJOINT =
            wait_query_data(context, &disjoint, cancel)?;
        if timing.Disjoint.as_bool() || timing.Frequency == 0 || end_tick < start_tick {
            return Err("GPU timestamp interval was disjoint".to_string());
        }
        samples.push(
            ((end_tick - start_tick) as f64 * 1_000_000.0 / timing.Frequency as f64)
                .round()
                .max(1.0) as u64,
        );
    }
    samples.sort_unstable();
    Ok(samples[samples.len() / 2])
}

fn create_measurement_texture(
    device: &ID3D11Device1,
    width: u32,
    height: u32,
    bind_flags: u32,
) -> Result<ID3D11Texture2D, String> {
    let desc = D3D11_TEXTURE2D_DESC {
        Width: width,
        Height: height,
        MipLevels: 1,
        ArraySize: 1,
        Format: DXGI_FORMAT_B8G8R8A8_UNORM,
        SampleDesc: DXGI_SAMPLE_DESC {
            Count: 1,
            Quality: 0,
        },
        Usage: D3D11_USAGE_DEFAULT,
        BindFlags: bind_flags,
        ..Default::default()
    };
    let mut texture = None;
    unsafe {
        device
            .CreateTexture2D(&desc, None, Some(&mut texture))
            .map_err(|error| format!("CreateTexture2D Anime4K measurement: {error:?}"))?;
    }
    texture.ok_or_else(|| "CreateTexture2D Anime4K measurement returned null".to_string())
}

fn create_query(
    device: &ID3D11Device1,
    query_type: windows::Win32::Graphics::Direct3D11::D3D11_QUERY,
) -> Result<ID3D11Query, String> {
    let desc = D3D11_QUERY_DESC {
        Query: query_type,
        MiscFlags: 0,
    };
    let mut query = None;
    unsafe {
        device
            .CreateQuery(&desc, Some(&mut query))
            .map_err(|error| format!("CreateQuery Anime4K measurement: {error:?}"))?;
    }
    query.ok_or_else(|| "CreateQuery Anime4K measurement returned null".to_string())
}

fn wait_query_data<T: Copy + Default>(
    context: &ID3D11DeviceContext,
    query: &ID3D11Query,
    cancel: &std::sync::atomic::AtomicBool,
) -> Result<T, String> {
    use windows::Win32::Foundation::{S_FALSE, S_OK};
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let mut value = T::default();
    loop {
        if cancel.load(std::sync::atomic::Ordering::Acquire) {
            return Err("Anime4K measurement cancelled".to_string());
        }
        let status = unsafe {
            (Interface::vtable(context).GetData)(
                Interface::as_raw(context),
                Interface::as_raw(query),
                (&mut value as *mut T).cast(),
                std::mem::size_of::<T>() as u32,
                0,
            )
        };
        if status == S_OK {
            return Ok(value);
        }
        if status != S_FALSE {
            return Err(format!("GetData Anime4K timestamp failed: {status:?}"));
        }
        if std::time::Instant::now() >= deadline {
            return Err("GPU timestamp did not complete within 10 seconds".to_string());
        }
        std::thread::yield_now();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ANIME4K_VL_HLSL: &[u8] =
        include_bytes!(concat!(env!("OUT_DIR"), "/video_anime4k_vl.hlsl"));
    use windows::Win32::Graphics::Direct3D::{
        D3D_DRIVER_TYPE_HARDWARE, D3D_DRIVER_TYPE_WARP, D3D_FEATURE_LEVEL, D3D_FEATURE_LEVEL_11_0,
    };
    use windows::Win32::Graphics::Direct3D11::{
        D3D11_BIND_SHADER_RESOURCE, D3D11_CPU_ACCESS_READ, D3D11_CREATE_DEVICE_FLAG,
        D3D11_MAP_READ, D3D11_MAPPED_SUBRESOURCE, D3D11_SDK_VERSION, D3D11_SUBRESOURCE_DATA,
        D3D11_USAGE_STAGING, D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext,
    };
    use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;

    fn test_texture_desc(width: u32, height: u32, bind_flags: u32) -> D3D11_TEXTURE2D_DESC {
        D3D11_TEXTURE2D_DESC {
            Width: width,
            Height: height,
            MipLevels: 1,
            ArraySize: 1,
            Format: DXGI_FORMAT_B8G8R8A8_UNORM,
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            Usage: D3D11_USAGE_DEFAULT,
            BindFlags: bind_flags,
            ..Default::default()
        }
    }

    fn warp_device() -> (ID3D11Device1, ID3D11DeviceContext) {
        let mut base_device: Option<ID3D11Device> = None;
        let mut context: Option<ID3D11DeviceContext> = None;
        let mut feature_level = D3D_FEATURE_LEVEL::default();
        unsafe {
            D3D11CreateDevice(
                None,
                D3D_DRIVER_TYPE_WARP,
                windows::Win32::Foundation::HMODULE::default(),
                D3D11_CREATE_DEVICE_FLAG::default(),
                Some(&[D3D_FEATURE_LEVEL_11_0]),
                D3D11_SDK_VERSION,
                Some(&mut base_device),
                Some(&mut feature_level),
                Some(&mut context),
            )
            .expect("create WARP D3D11 device");
        }
        assert_eq!(feature_level, D3D_FEATURE_LEVEL_11_0);
        let device = base_device
            .expect("D3D11 device")
            .cast()
            .expect("ID3D11Device1");
        (device, context.expect("D3D11 immediate context"))
    }

    /// resample / NIS シェーダがビルド時に DXBC 化されていること。
    ///
    /// 以前はここで `D3DCompile` を呼んで「HLSL が通る」ことを確かめていた。NIS の
    /// コンパイルだけで 2.1 秒かかっており、テストにも実行時にも同じ代金を払っていた。
    /// コンパイルは `build.rs` の仕事になった (通らなければビルドが落ちる) ので、
    /// 検査は「実行時に渡す DXBC が実在するか」に合わせる (backlog §1.122)。
    #[test]
    fn resample_shaders_are_precompiled_dxbc() {
        for (label, bytecode) in [
            ("vs_main", RESAMPLE_VS_MAIN),
            ("ps_horizontal", RESAMPLE_PS_HORIZONTAL),
            ("ps_vertical", RESAMPLE_PS_VERTICAL),
            ("ps_nearest", RESAMPLE_PS_NEAREST),
            ("fs_nis", NIS_FS_NIS),
        ] {
            assert!(
                bytecode.len() > 4 && &bytecode[..4] == b"DXBC",
                "{label} is not DXBC bytecode (len={})",
                bytecode.len()
            );
        }
    }

    fn assert_resample_mode_writes_target(mode: VideoResampleMode, label: &str) {
        const SOURCE_WIDTH: u32 = 8;
        const SOURCE_HEIGHT: u32 = 8;
        const TARGET_WIDTH: u32 = 16;
        const TARGET_HEIGHT: u32 = 16;

        let (device, context) = warp_device();

        let source_pixels = vec![
            0xFF_u8;
            (SOURCE_WIDTH * SOURCE_HEIGHT * 4)
                .try_into()
                .expect("source buffer length")
        ];
        let source_initial = D3D11_SUBRESOURCE_DATA {
            pSysMem: source_pixels.as_ptr().cast(),
            SysMemPitch: SOURCE_WIDTH * 4,
            SysMemSlicePitch: SOURCE_WIDTH * SOURCE_HEIGHT * 4,
        };
        let mut source = None;
        let source_desc = test_texture_desc(
            SOURCE_WIDTH,
            SOURCE_HEIGHT,
            D3D11_BIND_SHADER_RESOURCE.0 as u32,
        );
        unsafe {
            device
                .CreateTexture2D(&source_desc, Some(&source_initial), Some(&mut source))
                .unwrap_or_else(|error| panic!("create {label} source texture: {error:?}"));
        }
        let source = source.unwrap_or_else(|| panic!("{label} source texture"));

        let mut target = None;
        let target_desc = test_texture_desc(
            TARGET_WIDTH,
            TARGET_HEIGHT,
            D3D11_BIND_RENDER_TARGET.0 as u32,
        );
        unsafe {
            device
                .CreateTexture2D(&target_desc, None, Some(&mut target))
                .unwrap_or_else(|error| panic!("create {label} target texture: {error:?}"));
        }
        let target = target.unwrap_or_else(|| panic!("{label} target texture"));
        let target_view = create_render_target(&device, &target)
            .unwrap_or_else(|error| panic!("{label} target view: {error}"));
        unsafe {
            context.ClearRenderTargetView(&target_view, &[0.0, 0.0, 0.0, 1.0]);
        }

        let mut pipeline = VideoResamplePipeline::new(&device)
            .unwrap_or_else(|error| panic!("{label} pipeline: {error}"));
        pipeline
            .prepare(
                &device,
                SOURCE_WIDTH,
                SOURCE_HEIGHT,
                TARGET_WIDTH,
                VideoOrientation::IDENTITY,
                mode,
            )
            .unwrap_or_else(|_| panic!("prepare {label}"));
        pipeline
            .draw(
                &device,
                &context,
                &source,
                &target,
                SOURCE_WIDTH,
                SOURCE_HEIGHT,
                TARGET_WIDTH,
                TARGET_HEIGHT,
                VideoOrientation::IDENTITY,
                VideoZoomSourceRect {
                    origin: [0.0, 0.0],
                    extent: [SOURCE_WIDTH as f32, SOURCE_HEIGHT as f32],
                },
                mode,
            )
            .unwrap_or_else(|error| panic!("draw {label}: {error}"));

        let mut staging = None;
        let staging_desc = D3D11_TEXTURE2D_DESC {
            Usage: D3D11_USAGE_STAGING,
            BindFlags: 0,
            CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
            ..test_texture_desc(TARGET_WIDTH, TARGET_HEIGHT, 0)
        };
        unsafe {
            device
                .CreateTexture2D(&staging_desc, None, Some(&mut staging))
                .unwrap_or_else(|error| panic!("create {label} staging texture: {error:?}"));
        }
        let staging = staging.unwrap_or_else(|| panic!("{label} staging texture"));
        let target_resource: ID3D11Resource = target.cast().expect("target resource");
        let staging_resource: ID3D11Resource = staging.cast().expect("staging resource");
        let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
        let first_unwritten = unsafe {
            context.CopyResource(&staging_resource, &target_resource);
            context
                .Map(&staging_resource, 0, D3D11_MAP_READ, 0, Some(&mut mapped))
                .unwrap_or_else(|error| panic!("map {label} target: {error:?}"));
            let mut first_unwritten = None;
            'rows: for y in 0..TARGET_HEIGHT {
                for x in 0..TARGET_WIDTH {
                    let offset = (mapped.RowPitch * y + x * 4) as usize;
                    let bytes = mapped.pData.cast::<u8>().add(offset);
                    let sample = [*bytes, *bytes.add(1), *bytes.add(2), *bytes.add(3)];
                    if sample.iter().any(|channel| *channel < 0xF0) {
                        first_unwritten = Some((x, y, sample));
                        break 'rows;
                    }
                }
            }
            context.Unmap(&staging_resource, 0);
            first_unwritten
        };
        assert!(
            first_unwritten.is_none(),
            "{label} must overwrite the entire cleared target; first unwritten pixel was {first_unwritten:?}"
        );
    }

    #[test]
    fn nis_draw_writes_target_with_default_d3d11_rasterizer() {
        assert_resample_mode_writes_target(VideoResampleMode::Nis, "NIS");
    }

    #[test]
    fn anime4k_chain_writes_target_with_native_fullscreen_vertex_shader() {
        assert_resample_mode_writes_target(
            VideoResampleMode::Anime4k {
                variant: VideoAnime4kVariant::VeryLarge,
            },
            "Anime4K VL",
        );
    }

    #[test]
    fn shader_filter_modes_use_their_upscaler_and_lanczos_for_every_downscale() {
        assert_eq!(
            select_video_resample_mode(
                VideoScaleFilter::Sharp,
                1280.0,
                720.0,
                1920,
                1080,
                40,
                None,
            ),
            Some(VideoResampleMode::Nis)
        );
        assert_eq!(
            select_video_resample_mode(
                VideoScaleFilter::Nearest,
                1280.0,
                720.0,
                1920,
                1080,
                40,
                None,
            ),
            Some(VideoResampleMode::Nearest)
        );
        assert_eq!(
            select_video_resample_mode(
                VideoScaleFilter::Anime,
                1280.0,
                720.0,
                1920,
                1080,
                40,
                Some(VideoAnime4kVariant::VeryLarge)
            ),
            Some(VideoResampleMode::Anime4k {
                variant: VideoAnime4kVariant::VeryLarge
            })
        );
        for filter in [
            VideoScaleFilter::Standard,
            VideoScaleFilter::Sharp,
            VideoScaleFilter::Nearest,
            VideoScaleFilter::Anime,
        ] {
            assert_eq!(
                select_video_resample_mode(
                    filter,
                    3840.0,
                    2160.0,
                    1920,
                    1080,
                    40,
                    Some(VideoAnime4kVariant::VeryLarge)
                ),
                Some(VideoResampleMode::Lanczos3 {
                    smoothing_percent: 40
                })
            );
        }
        assert_eq!(
            select_video_resample_mode(
                VideoScaleFilter::OsDefault,
                1280.0,
                720.0,
                1920,
                1080,
                40,
                None
            ),
            None
        );
    }

    #[test]
    fn zoomed_effective_source_extent_selects_an_upscaler() {
        assert_eq!(
            select_video_resample_mode(VideoScaleFilter::Sharp, 960.0, 540.0, 1920, 1080, 40, None,),
            Some(VideoResampleMode::Nis)
        );
        assert_eq!(
            select_video_resample_mode(
                VideoScaleFilter::Sharp,
                3840.0,
                2160.0,
                1920,
                1080,
                40,
                None,
            ),
            Some(VideoResampleMode::Lanczos3 {
                smoothing_percent: 40,
            })
        );
    }

    #[test]
    fn embedded_anime4k_bytecode_matches_every_generated_topology() {
        for bytecode in VIDEO_ANIME4K_BYTECODE_VARIANTS {
            assert_eq!(
                bytecode.convolution.len(),
                bytecode.variant.intermediate_count(),
                "{}",
                bytecode.variant.label()
            );
            assert_eq!(
                bytecode.variant.pass_inputs().len(),
                bytecode.convolution.len() + 1,
                "{}",
                bytecode.variant.label()
            );
            assert!(
                bytecode.convolution.iter().all(|shader| !shader.is_empty()),
                "{}",
                bytecode.variant.label()
            );
            assert!(!bytecode.resolve.is_empty(), "{}", bytecode.variant.label());
        }
    }

    #[test]
    #[ignore = "manual B3 GPU timestamp calibration on the active hardware adapter"]
    fn measure_anime4k_variants_with_gpu_timestamps() {
        let mut base_device = None;
        let mut context = None;
        let mut feature_level = D3D_FEATURE_LEVEL::default();
        unsafe {
            D3D11CreateDevice(
                None,
                D3D_DRIVER_TYPE_HARDWARE,
                windows::Win32::Foundation::HMODULE::default(),
                D3D11_CREATE_DEVICE_FLAG::default(),
                Some(&[D3D_FEATURE_LEVEL_11_0]),
                D3D11_SDK_VERSION,
                Some(&mut base_device),
                Some(&mut feature_level),
                Some(&mut context),
            )
            .expect("create hardware D3D11 device");
        }
        assert_eq!(feature_level, D3D_FEATURE_LEVEL_11_0);
        let device = base_device.expect("hardware D3D11 device");
        let dxgi_device: IDXGIDevice = device.cast().expect("IDXGIDevice");
        let adapter = unsafe { dxgi_device.GetAdapter() }.expect("hardware adapter");
        let cancel = std::sync::atomic::AtomicBool::new(false);
        let points = measure_video_anime4k_on_adapter(&adapter, &cancel, |completed, point| {
            eprintln!(
                "Anime4K measurement {completed}/6 variant={} source={}x{} output={}x{} gpu_us={}",
                point.variant.label(),
                point.source_width,
                point.source_height,
                point.output_width,
                point.output_height,
                point.gpu_time_us
            );
        })
        .expect("Anime4K GPU timestamp measurement");
        assert_eq!(points.len(), 6);
        assert!(
            points
                .iter()
                .all(|point| point.gpu_time_us > 0 && point.gpu_time_us != u64::MAX)
        );
    }

    #[test]
    #[ignore = "manual B3 cost measurement; compiles all 18 VL passes"]
    fn measure_anime4k_vl_runtime_compile_versus_bytecode_load() {
        let (device, _context) = warp_device();
        let compile_started = std::time::Instant::now();
        let variant = Anime4kVariant::VeryLarge;
        let mut runtime_shaders = Vec::with_capacity(variant.pass_inputs().len());
        for pass_index in 0..variant.intermediate_count() {
            let entry = format!("fs_anime4k_{pass_index}_");
            let blob = compile_shader_source(ANIME4K_VL_HLSL, &entry, "ps_5_0")
                .unwrap_or_else(|error| panic!("compile {entry}: {error}"));
            let mut shader = None;
            unsafe {
                device
                    .CreatePixelShader(blob_bytes(&blob), None, Some(&mut shader))
                    .unwrap_or_else(|error| panic!("create {entry}: {error:?}"));
            }
            runtime_shaders.push(shader.expect("runtime-compiled Anime4K shader"));
        }
        let resolve_blob = compile_shader_source(ANIME4K_VL_HLSL, "fs_anime4k_resolve", "ps_5_0")
            .expect("compile Anime4K resolve");
        let mut resolve_shader = None;
        unsafe {
            device
                .CreatePixelShader(blob_bytes(&resolve_blob), None, Some(&mut resolve_shader))
                .expect("create runtime-compiled Anime4K resolve");
        }
        runtime_shaders.push(resolve_shader.expect("runtime-compiled Anime4K resolve"));
        let _runtime_convolution_constants =
            create_constant_buffer::<Anime4kConstants>(&device, "runtime Anime4K convolution")
                .expect("runtime Anime4K convolution constants");
        let _runtime_resolve_constants =
            create_constant_buffer::<Anime4kConstants>(&device, "runtime Anime4K resolve")
                .expect("runtime Anime4K resolve constants");
        let compile_ms = compile_started.elapsed().as_secs_f64() * 1000.0;

        let load_started = std::time::Instant::now();
        let loaded =
            VideoAnime4kPipeline::new(&device, variant).expect("load embedded Anime4K VL bytecode");
        let load_ms = load_started.elapsed().as_secs_f64() * 1000.0;
        assert_eq!(runtime_shaders.len(), loaded.convolution_shaders.len() + 1);
        let all_load_started = std::time::Instant::now();
        let all_loaded = VideoAnime4kVariant::ALL
            .into_iter()
            .map(|variant| VideoAnime4kPipeline::new(&device, gpu_anime4k_variant(variant)))
            .collect::<Result<Vec<_>, _>>()
            .expect("load embedded Anime4K bytecode for all variants");
        let all_load_ms = all_load_started.elapsed().as_secs_f64() * 1000.0;
        assert_eq!(all_loaded.len(), VideoAnime4kVariant::ALL.len());
        println!(
            "Anime4K VL pipeline: runtime_compile_ms={compile_ms:.3} bytecode_load_ms={load_ms:.3} all_variant_bytecode_load_ms={all_load_ms:.3} shaders={}",
            runtime_shaders.len()
        );
    }

    #[test]
    fn anime4k_constant_layout_matches_generated_shader() {
        assert_eq!(std::mem::size_of::<Anime4kConstants>(), 96);
    }

    #[test]
    fn anime4k_resolve_maps_the_oriented_whole_frame_back_to_raw_texels() {
        let constants = anime4k_video_constants(
            1920,
            1080,
            2160,
            3840,
            VideoOrientation::new(90, false),
            VideoZoomSourceRect {
                origin: [0.0, 0.0],
                extent: [1080.0, 1920.0],
            },
        );
        assert_eq!(constants.output_size, [2160, 3840]);
        assert_eq!(constants.source_size, [1920, 1080]);
        assert_eq!(constants.process_origin, [0, 0]);
        assert_eq!(constants.process_size, [1920, 1080]);
        assert_eq!(constants.source_region, [0.0, 0.0, 1080.0, 1920.0]);
        assert_eq!(constants.inverse_x, [0.0, -1.0]);
        assert_eq!(constants.inverse_y, [1.0, 0.0]);
        assert_eq!(constants.inverse_offset, [0.0, 1079.0]);
    }

    #[test]
    fn inverse_orientation_mapping_handles_quarter_turns_and_reflection() {
        let identity = inverse_orientation_mapping(1920, 1080, VideoOrientation::IDENTITY);
        assert_eq!(identity.source_axis_x, 1920);
        assert_eq!(identity.source_axis_y, 1080);
        assert_eq!(identity.inverse_x, [1.0, 0.0]);
        assert_eq!(identity.inverse_y, [0.0, 1.0]);

        let turn = inverse_orientation_mapping(1920, 1080, VideoOrientation::new(90, false));
        assert_eq!(turn.source_axis_x, 1080);
        assert_eq!(turn.source_axis_y, 1920);
        assert_eq!(turn.inverse_x, [0.0, -1.0]);
        assert_eq!(turn.inverse_y, [1.0, 0.0]);
        assert_eq!(turn.offset, [0.0, 1079.0]);

        let reflected = inverse_orientation_mapping(1920, 1080, VideoOrientation::new(0, true));
        assert_eq!(reflected.inverse_x, [1.0, 0.0]);
        assert_eq!(reflected.inverse_y, [0.0, -1.0]);
        assert_eq!(reflected.offset, [0.0, 1079.0]);
    }
}
