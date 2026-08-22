//! Display-resolution Lanczos3 resolve for the native video presenter.
//!
//! The two passes keep filtering separable even for deep downscales. The first
//! pass also materializes the normalized display orientation, so the final swap
//! chain is exactly the physical video display rectangle and DComp only places
//! that rectangle.

use windows::Win32::Graphics::Direct3D::Fxc::D3DCompile;
use windows::Win32::Graphics::Direct3D::{
    D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST, ID3DBlob, ID3DInclude,
};
use windows::Win32::Graphics::Direct3D11::{
    D3D11_BIND_CONSTANT_BUFFER, D3D11_BIND_RENDER_TARGET, D3D11_BIND_SHADER_RESOURCE,
    D3D11_BUFFER_DESC, D3D11_TEXTURE2D_DESC, D3D11_USAGE_DEFAULT, D3D11_VIEWPORT, ID3D11Buffer,
    ID3D11Device1, ID3D11DeviceContext, ID3D11PixelShader, ID3D11RenderTargetView, ID3D11Resource,
    ID3D11ShaderResourceView, ID3D11Texture2D, ID3D11VertexShader,
};
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_R16G16B16A16_FLOAT, DXGI_SAMPLE_DESC};
use windows::core::{Interface, PCSTR};

use crate::settings::VideoScaleFilter;
use crate::video::display_metadata::VideoOrientation;

const NIS_SHADER: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/video_nis.hlsl"));
const RESAMPLE_SHADER: &[u8] = br#"
Texture2D<float4> source_tex : register(t0);

cbuffer ResampleConstants : register(b0) {
    float4 source_target;  // raw source width/height, final target width/height
    float4 axis_filter;    // oriented source axis lengths, horizontal/vertical stretch
    float4 inverse_axes;   // d(raw xy)/d(oriented x), d(raw xy)/d(oriented y)
    float4 inverse_offset; // raw xy at oriented (0,0), unused
};

struct VsOut {
    float4 position : SV_Position;
};

VsOut vs_main(uint vertex_id : SV_VertexID) {
    VsOut output;
    float2 uv = float2((vertex_id << 1) & 2, vertex_id & 2);
    output.position = float4(uv * float2(2.0, -2.0) + float2(-1.0, 1.0), 0.0, 1.0);
    return output;
}

float sinc_pi(float value) {
    float x = value * 3.14159265358979323846;
    return abs(x) < 1.0e-5 ? 1.0 : sin(x) / x;
}

float lanczos3_weight(float distance, float stretch) {
    float x = abs(distance) / stretch;
    if (x >= 3.0) {
        return 0.0;
    }
    return sinc_pi(x) * sinc_pi(x / 3.0);
}

float4 ps_horizontal(VsOut input) : SV_Target {
    float source_axis_x = axis_filter.x;
    float source_axis_y = axis_filter.y;
    float target_width = source_target.z;
    float source_position = input.position.x * source_axis_x / target_width - 0.5;
    float oriented_y = clamp(input.position.y - 0.5, 0.0, source_axis_y - 1.0);
    float stretch = axis_filter.z;
    int radius = (int)ceil(3.0 * stretch);
    int first = (int)floor(source_position) - radius;
    int last = (int)floor(source_position) + radius + 1;
    int2 raw_max = int2(source_target.xy) - 1;
    float4 accumulated = 0.0;
    float weight_sum = 0.0;
    [loop]
    for (int sample_index = first; sample_index <= last; ++sample_index) {
        float weight = lanczos3_weight(source_position - sample_index, stretch);
        float2 raw_position =
            (float)sample_index * inverse_axes.xy
            + oriented_y * inverse_axes.zw
            + inverse_offset.xy;
        int2 raw_coord = clamp(int2(round(raw_position)), int2(0, 0), raw_max);
        accumulated += source_tex.Load(int3(raw_coord, 0)) * weight;
        weight_sum += weight;
    }
    return abs(weight_sum) > 1.0e-6 ? accumulated / weight_sum : accumulated;
}

float4 ps_vertical(VsOut input) : SV_Target {
    float source_axis_y = axis_filter.y;
    float target_height = source_target.w;
    float source_position = input.position.y * source_axis_y / target_height - 0.5;
    float stretch = axis_filter.w;
    int radius = (int)ceil(3.0 * stretch);
    int first = (int)floor(source_position) - radius;
    int last = (int)floor(source_position) + radius + 1;
    int source_x = clamp((int)input.position.x, 0, (int)source_target.z - 1);
    int source_y_max = (int)source_axis_y - 1;
    float4 accumulated = 0.0;
    float weight_sum = 0.0;
    [loop]
    for (int sample_index = first; sample_index <= last; ++sample_index) {
        float weight = lanczos3_weight(source_position - sample_index, stretch);
        int source_y = clamp(sample_index, 0, source_y_max);
        accumulated += source_tex.Load(int3(source_x, source_y, 0)) * weight;
        weight_sum += weight;
    }
    return abs(weight_sum) > 1.0e-6 ? accumulated / weight_sum : accumulated;
}

float4 ps_nearest(VsOut input) : SV_Target {
    float2 oriented_position =
        input.position.xy * axis_filter.xy / source_target.zw - 0.5;
    float2 raw_position =
        round(oriented_position.x) * inverse_axes.xy
        + round(oriented_position.y) * inverse_axes.zw
        + inverse_offset.xy;
    int2 raw_max = int2(source_target.xy) - 1;
    int2 raw_coord = clamp(int2(round(raw_position)), int2(0, 0), raw_max);
    return source_tex.Load(int3(raw_coord, 0));
}
"#;

#[repr(C)]
#[derive(Clone, Copy)]
struct ResampleConstants {
    source_target: [f32; 4],
    axis_filter: [f32; 4],
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum VideoResampleMode {
    Lanczos3 { smoothing_percent: u32 },
    Nis,
    Nearest,
}

pub(super) fn select_video_resample_mode(
    filter: VideoScaleFilter,
    source_axis_width: u32,
    source_axis_height: u32,
    target_width: u32,
    target_height: u32,
    smoothing_percent: u32,
) -> Option<VideoResampleMode> {
    if filter == VideoScaleFilter::OsDefault {
        return None;
    }
    let downscaling = target_width < source_axis_width || target_height < source_axis_height;
    Some(match (filter, downscaling) {
        (VideoScaleFilter::Sharp, false) => VideoResampleMode::Nis,
        (VideoScaleFilter::Nearest, false) => VideoResampleMode::Nearest,
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

pub(super) struct VideoResamplePipeline {
    vertex_shader: ID3D11VertexShader,
    nis_vertex_shader: ID3D11VertexShader,
    horizontal_shader: ID3D11PixelShader,
    vertical_shader: ID3D11PixelShader,
    nearest_shader: ID3D11PixelShader,
    nis_shader: ID3D11PixelShader,
    constants: ID3D11Buffer,
    nis_constants: ID3D11Buffer,
    intermediate: Option<IntermediateTarget>,
}

impl VideoResamplePipeline {
    pub(super) fn new(device: &ID3D11Device1) -> Result<Self, String> {
        let vertex_blob = compile_resample_shader("vs_main", "vs_5_0")?;
        let horizontal_blob = compile_resample_shader("ps_horizontal", "ps_5_0")?;
        let vertical_blob = compile_resample_shader("ps_vertical", "ps_5_0")?;
        let nearest_blob = compile_resample_shader("ps_nearest", "ps_5_0")?;
        let nis_vertex_blob = compile_shader_source(NIS_SHADER, "vs_main", "vs_5_0")?;
        let nis_blob = compile_shader_source(NIS_SHADER, "fs_nis", "ps_5_0")?;
        let mut vertex_shader = None;
        let mut nis_vertex_shader = None;
        let mut horizontal_shader = None;
        let mut vertical_shader = None;
        let mut nearest_shader = None;
        let mut nis_shader = None;
        let mut constants = None;
        let mut nis_constants = None;
        unsafe {
            device
                .CreateVertexShader(blob_bytes(&vertex_blob), None, Some(&mut vertex_shader))
                .map_err(|error| format!("CreateVertexShader resample: {error:?}"))?;
            device
                .CreateVertexShader(
                    blob_bytes(&nis_vertex_blob),
                    None,
                    Some(&mut nis_vertex_shader),
                )
                .map_err(|error| format!("CreateVertexShader NIS: {error:?}"))?;
            device
                .CreatePixelShader(
                    blob_bytes(&horizontal_blob),
                    None,
                    Some(&mut horizontal_shader),
                )
                .map_err(|error| format!("CreatePixelShader resample horizontal: {error:?}"))?;
            device
                .CreatePixelShader(blob_bytes(&vertical_blob), None, Some(&mut vertical_shader))
                .map_err(|error| format!("CreatePixelShader resample vertical: {error:?}"))?;
            device
                .CreatePixelShader(blob_bytes(&nearest_blob), None, Some(&mut nearest_shader))
                .map_err(|error| format!("CreatePixelShader nearest: {error:?}"))?;
            device
                .CreatePixelShader(blob_bytes(&nis_blob), None, Some(&mut nis_shader))
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
        Ok(Self {
            vertex_shader: vertex_shader
                .ok_or_else(|| "CreateVertexShader resample returned null".to_string())?,
            nis_vertex_shader: nis_vertex_shader
                .ok_or_else(|| "CreateVertexShader NIS returned null".to_string())?,
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
    ) -> Result<(), String> {
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
        let desc = D3D11_TEXTURE2D_DESC {
            Width: width,
            Height: height,
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
        let texture = texture
            .ok_or_else(|| "CreateTexture2D resample intermediate returned null".to_string())?;
        let shader_view = create_shader_view(device, &texture)?;
        let render_target = create_render_target(device, &texture)?;
        self.intermediate = Some(IntermediateTarget {
            width,
            height,
            _texture: texture,
            shader_view,
            render_target,
        });
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
                mode,
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
        let source_axis_x = mapping.source_axis_x as f32;
        let source_axis_y = mapping.source_axis_y as f32;
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
                source_axis_x,
                source_axis_y,
                stretch(source_axis_x, target_width),
                stretch(source_axis_y, target_height),
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
            source_origin: [0.0, 0.0],
            source_extent: [mapping.source_axis_x as f32, mapping.source_axis_y as f32],
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
                    context.VSSetShader(&self.nis_vertex_shader, None);
                    context.PSSetConstantBuffers(1, Some(&[Some(self.nis_constants.clone())]));
                    context.PSSetShader(&self.nis_shader, None);
                }
                VideoResampleMode::Lanczos3 { .. } => {
                    return Err("Lanczos3 entered the single-pass resampler".to_string());
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

    pub(super) fn intermediate_vram_bytes(&self) -> u64 {
        self.intermediate
            .as_ref()
            .map(|target| u64::from(target.width) * u64::from(target.height) * 8)
            .unwrap_or(0)
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

fn compile_resample_shader(entry: &'static str, target: &'static str) -> Result<ID3DBlob, String> {
    compile_shader_source(RESAMPLE_SHADER, entry, target)
}

fn compile_shader_source(
    source: &[u8],
    entry: &'static str,
    target: &'static str,
) -> Result<ID3DBlob, String> {
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

fn blob_bytes(blob: &ID3DBlob) -> &[u8] {
    unsafe {
        std::slice::from_raw_parts(blob.GetBufferPointer().cast::<u8>(), blob.GetBufferSize())
    }
}

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resample_hlsl_compiles_for_shader_model_5() {
        compile_resample_shader("vs_main", "vs_5_0").expect("vertex shader");
        compile_resample_shader("ps_horizontal", "ps_5_0").expect("horizontal shader");
        compile_resample_shader("ps_vertical", "ps_5_0").expect("vertical shader");
        compile_resample_shader("ps_nearest", "ps_5_0").expect("nearest shader");
    }

    #[test]
    fn nis_converter_output_compiles_for_shader_model_5() {
        compile_shader_source(NIS_SHADER, "vs_main", "vs_5_0").expect("NIS vertex shader");
        compile_shader_source(NIS_SHADER, "fs_nis", "ps_5_0").expect("NIS pixel shader");
    }

    #[test]
    fn phase_a_filter_modes_use_lanczos_for_every_downscale() {
        assert_eq!(
            select_video_resample_mode(VideoScaleFilter::Sharp, 1280, 720, 1920, 1080, 40),
            Some(VideoResampleMode::Nis)
        );
        assert_eq!(
            select_video_resample_mode(VideoScaleFilter::Nearest, 1280, 720, 1920, 1080, 40),
            Some(VideoResampleMode::Nearest)
        );
        for filter in [
            VideoScaleFilter::Standard,
            VideoScaleFilter::Sharp,
            VideoScaleFilter::Nearest,
        ] {
            assert_eq!(
                select_video_resample_mode(filter, 3840, 2160, 1920, 1080, 40),
                Some(VideoResampleMode::Lanczos3 {
                    smoothing_percent: 40
                })
            );
        }
        assert_eq!(
            select_video_resample_mode(VideoScaleFilter::OsDefault, 1280, 720, 1920, 1080, 40),
            None
        );
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
