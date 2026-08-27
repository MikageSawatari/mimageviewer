use windows::Win32::Graphics::Direct3D::D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST;
use windows::Win32::Graphics::Direct3D11::{
    D3D11_BIND_CONSTANT_BUFFER, D3D11_BIND_RENDER_TARGET, D3D11_BIND_SHADER_RESOURCE,
    D3D11_BUFFER_DESC, D3D11_FILTER_MIN_MAG_LINEAR_MIP_POINT, D3D11_SAMPLER_DESC,
    D3D11_TEXTURE_ADDRESS_CLAMP, D3D11_TEXTURE_ADDRESS_WRAP, D3D11_TEXTURE2D_DESC,
    D3D11_USAGE_DEFAULT, D3D11_VIEWPORT, ID3D11Buffer, ID3D11Device1, ID3D11DeviceContext,
    ID3D11PixelShader, ID3D11RenderTargetView, ID3D11Resource, ID3D11SamplerState,
    ID3D11ShaderResourceView, ID3D11Texture2D, ID3D11VertexShader,
};
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC};
use windows::core::Interface;

use crate::panorama::{PanoPose, PanoUvTransform};
use crate::video::display_metadata::VideoOrientation;

const PANORAMA_VS_MAIN: &[u8] =
    include_bytes!(concat!(env!("OUT_DIR"), "/video_panorama_vs_main.cso"));
const PANORAMA_PS_ORIENT: &[u8] =
    include_bytes!(concat!(env!("OUT_DIR"), "/video_panorama_ps_orient.cso"));
const PANORAMA_PS_MAIN: &[u8] =
    include_bytes!(concat!(env!("OUT_DIR"), "/video_panorama_ps_main.cso"));
const PANORAMA_CONSTANT_BYTES: usize = crate::panorama_wgpu::PANO_UNIFORM_BYTES;

#[repr(C)]
#[derive(Clone, Copy)]
struct OrientationConstants {
    source_oriented: [f32; 4],
    inverse_axes: [f32; 4],
    inverse_offset: [f32; 4],
}

struct OrientationTarget {
    width: u32,
    height: u32,
    _texture: ID3D11Texture2D,
    shader_view: ID3D11ShaderResourceView,
    render_target: ID3D11RenderTargetView,
}

pub(super) struct VideoPanoramaPipeline {
    vertex_shader: ID3D11VertexShader,
    orientation_shader: ID3D11PixelShader,
    panorama_shader: ID3D11PixelShader,
    repeat_clamp_sampler: ID3D11SamplerState,
    clamp_sampler: ID3D11SamplerState,
    panorama_constants: ID3D11Buffer,
    orientation_constants: ID3D11Buffer,
    orientation_target: Option<OrientationTarget>,
}

impl VideoPanoramaPipeline {
    pub(super) fn new(device: &ID3D11Device1) -> Result<Self, String> {
        let mut vertex_shader = None;
        let mut orientation_shader = None;
        let mut panorama_shader = None;
        let mut repeat_clamp_sampler = None;
        let mut clamp_sampler = None;
        let mut panorama_constants = None;
        let mut orientation_constants = None;
        unsafe {
            device
                .CreateVertexShader(PANORAMA_VS_MAIN, None, Some(&mut vertex_shader))
                .map_err(|error| format!("CreateVertexShader panorama: {error:?}"))?;
            device
                .CreatePixelShader(PANORAMA_PS_ORIENT, None, Some(&mut orientation_shader))
                .map_err(|error| format!("CreatePixelShader panorama orientation: {error:?}"))?;
            device
                .CreatePixelShader(PANORAMA_PS_MAIN, None, Some(&mut panorama_shader))
                .map_err(|error| format!("CreatePixelShader panorama: {error:?}"))?;
            let repeat_clamp_desc = D3D11_SAMPLER_DESC {
                Filter: D3D11_FILTER_MIN_MAG_LINEAR_MIP_POINT,
                AddressU: D3D11_TEXTURE_ADDRESS_WRAP,
                AddressV: D3D11_TEXTURE_ADDRESS_CLAMP,
                AddressW: D3D11_TEXTURE_ADDRESS_CLAMP,
                MaxLOD: f32::MAX,
                ..Default::default()
            };
            device
                .CreateSamplerState(&repeat_clamp_desc, Some(&mut repeat_clamp_sampler))
                .map_err(|error| format!("CreateSamplerState panorama repeat/clamp: {error:?}"))?;
            let clamp_desc = D3D11_SAMPLER_DESC {
                AddressU: D3D11_TEXTURE_ADDRESS_CLAMP,
                ..repeat_clamp_desc
            };
            device
                .CreateSamplerState(&clamp_desc, Some(&mut clamp_sampler))
                .map_err(|error| format!("CreateSamplerState panorama clamp: {error:?}"))?;
            let panorama_desc = D3D11_BUFFER_DESC {
                ByteWidth: PANORAMA_CONSTANT_BYTES as u32,
                Usage: D3D11_USAGE_DEFAULT,
                BindFlags: D3D11_BIND_CONSTANT_BUFFER.0 as u32,
                ..Default::default()
            };
            device
                .CreateBuffer(&panorama_desc, None, Some(&mut panorama_constants))
                .map_err(|error| format!("CreateBuffer panorama constants: {error:?}"))?;
            let orientation_desc = D3D11_BUFFER_DESC {
                ByteWidth: std::mem::size_of::<OrientationConstants>() as u32,
                Usage: D3D11_USAGE_DEFAULT,
                BindFlags: D3D11_BIND_CONSTANT_BUFFER.0 as u32,
                ..Default::default()
            };
            device
                .CreateBuffer(&orientation_desc, None, Some(&mut orientation_constants))
                .map_err(|error| {
                    format!("CreateBuffer panorama orientation constants: {error:?}")
                })?;
        }
        Ok(Self {
            vertex_shader: vertex_shader
                .ok_or_else(|| "CreateVertexShader panorama returned null".to_string())?,
            orientation_shader: orientation_shader.ok_or_else(|| {
                "CreatePixelShader panorama orientation returned null".to_string()
            })?,
            panorama_shader: panorama_shader
                .ok_or_else(|| "CreatePixelShader panorama returned null".to_string())?,
            repeat_clamp_sampler: repeat_clamp_sampler.ok_or_else(|| {
                "CreateSamplerState panorama repeat/clamp returned null".to_string()
            })?,
            clamp_sampler: clamp_sampler
                .ok_or_else(|| "CreateSamplerState panorama clamp returned null".to_string())?,
            panorama_constants: panorama_constants
                .ok_or_else(|| "CreateBuffer panorama constants returned null".to_string())?,
            orientation_constants: orientation_constants.ok_or_else(|| {
                "CreateBuffer panorama orientation constants returned null".to_string()
            })?,
            orientation_target: None,
        })
    }

    pub(super) fn prepare(
        &mut self,
        device: &ID3D11Device1,
        source_width: u32,
        source_height: u32,
        orientation: VideoOrientation,
    ) -> Result<(), String> {
        if orientation == VideoOrientation::IDENTITY {
            self.orientation_target = None;
            return Ok(());
        }
        let (width, height) = oriented_dimensions(source_width, source_height, orientation);
        if self
            .orientation_target
            .as_ref()
            .is_some_and(|target| target.width == width && target.height == height)
        {
            return Ok(());
        }
        self.orientation_target = Some(create_orientation_target(device, width, height)?);
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
        pose: PanoPose,
        uv: PanoUvTransform,
    ) -> Result<(), String> {
        let raw_source_view = create_shader_view(device, source)?;
        let target_view = create_render_target(device, target)?;
        let oriented_source_view;
        let source_view = if orientation == VideoOrientation::IDENTITY {
            &raw_source_view
        } else {
            // Display-matrix orientation is materialized before projection so
            // the panorama shader still sees a normal equirectangular image.
            // Rotating only the final viewport would rotate the camera view and
            // would also move the longitude seam onto the wrong texture axis.
            let orientation_target = self
                .orientation_target
                .as_ref()
                .ok_or_else(|| "panorama orientation target was not prepared".to_string())?;
            let mapping = inverse_orientation_mapping(source_width, source_height, orientation);
            if orientation_target.width != mapping.source_axis_x
                || orientation_target.height != mapping.source_axis_y
            {
                return Err(format!(
                    "panorama orientation target mismatch: prepared={}x{} required={}x{}",
                    orientation_target.width,
                    orientation_target.height,
                    mapping.source_axis_x,
                    mapping.source_axis_y
                ));
            }
            let constants = OrientationConstants {
                source_oriented: [
                    source_width.max(1) as f32,
                    source_height.max(1) as f32,
                    mapping.source_axis_x as f32,
                    mapping.source_axis_y as f32,
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
                    &self.orientation_constants,
                    0,
                    None,
                    (&constants as *const OrientationConstants).cast(),
                    0,
                    0,
                );
                bind_fullscreen_state(context, &self.vertex_shader);
                context.PSSetShader(&self.orientation_shader, None);
                context.PSSetConstantBuffers(1, Some(&[Some(self.orientation_constants.clone())]));
                context.PSSetShaderResources(0, Some(&[Some(raw_source_view.clone())]));
                context.RSSetViewports(Some(&[viewport(
                    orientation_target.width,
                    orientation_target.height,
                )]));
                context.OMSetRenderTargets(
                    Some(&[Some(orientation_target.render_target.clone())]),
                    None,
                );
                context.Draw(3, 0);
                context.PSSetShaderResources(0, Some(&[None]));
                context.OMSetRenderTargets(None, None);
            }
            oriented_source_view = orientation_target.shader_view.clone();
            &oriented_source_view
        };

        let aspect = target_width.max(1) as f32 / target_height.max(1) as f32;
        let constants = video_panorama_uniform_bytes(pose, aspect, uv);
        unsafe {
            context.UpdateSubresource(
                &self.panorama_constants,
                0,
                None,
                constants.as_ptr().cast(),
                0,
                0,
            );
            bind_fullscreen_state(context, &self.vertex_shader);
            context.PSSetShader(&self.panorama_shader, None);
            context.PSSetConstantBuffers(0, Some(&[Some(self.panorama_constants.clone())]));
            context.PSSetSamplers(
                0,
                Some(&[
                    Some(self.repeat_clamp_sampler.clone()),
                    Some(self.clamp_sampler.clone()),
                ]),
            );
            context.PSSetShaderResources(0, Some(&[Some(source_view.clone())]));
            context.RSSetViewports(Some(&[viewport(target_width, target_height)]));
            context.OMSetRenderTargets(Some(&[Some(target_view)]), None);
            context.Draw(3, 0);
            context.PSSetShaderResources(0, Some(&[None]));
            context.OMSetRenderTargets(None, None);
        }
        Ok(())
    }
}

fn video_panorama_uniform_bytes(
    pose: PanoPose,
    aspect: f32,
    uv: PanoUvTransform,
) -> [u8; PANORAMA_CONSTANT_BYTES] {
    let map = pose.map();
    let values = [
        pose.yaw,
        pose.pitch,
        pose.fov_y,
        aspect,
        uv.u_offset,
        uv.v_offset,
        uv.u_scale,
        uv.v_scale,
        pose.projection.shader_code() as f32,
        map.coefficient(),
        0.0,
        0.0,
    ];
    let mut bytes = [0_u8; PANORAMA_CONSTANT_BYTES];
    for (index, value) in values.iter().enumerate() {
        bytes[index * 4..index * 4 + 4].copy_from_slice(&value.to_ne_bytes());
    }
    bytes
}

fn create_orientation_target(
    device: &ID3D11Device1,
    width: u32,
    height: u32,
) -> Result<OrientationTarget, String> {
    let desc = D3D11_TEXTURE2D_DESC {
        Width: width.max(1),
        Height: height.max(1),
        MipLevels: 1,
        ArraySize: 1,
        Format: DXGI_FORMAT_B8G8R8A8_UNORM,
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
            .map_err(|error| format!("CreateTexture2D panorama orientation: {error:?}"))?;
    }
    let texture =
        texture.ok_or_else(|| "CreateTexture2D panorama orientation returned null".to_string())?;
    let shader_view = create_shader_view(device, &texture)?;
    let render_target = create_render_target(device, &texture)?;
    Ok(OrientationTarget {
        width: width.max(1),
        height: height.max(1),
        _texture: texture,
        shader_view,
        render_target,
    })
}

fn create_shader_view<T: Interface>(
    device: &ID3D11Device1,
    texture: &T,
) -> Result<ID3D11ShaderResourceView, String> {
    let resource: ID3D11Resource = texture
        .cast()
        .map_err(|error| format!("cast panorama shader resource: {error:?}"))?;
    let mut view = None;
    unsafe {
        device
            .CreateShaderResourceView(&resource, None, Some(&mut view))
            .map_err(|error| format!("CreateShaderResourceView panorama: {error:?}"))?;
    }
    view.ok_or_else(|| "CreateShaderResourceView panorama returned null".to_string())
}

fn create_render_target<T: Interface>(
    device: &ID3D11Device1,
    texture: &T,
) -> Result<ID3D11RenderTargetView, String> {
    let resource: ID3D11Resource = texture
        .cast()
        .map_err(|error| format!("cast panorama render target: {error:?}"))?;
    let mut view = None;
    unsafe {
        device
            .CreateRenderTargetView(&resource, None, Some(&mut view))
            .map_err(|error| format!("CreateRenderTargetView panorama: {error:?}"))?;
    }
    view.ok_or_else(|| "CreateRenderTargetView panorama returned null".to_string())
}

fn bind_fullscreen_state(context: &ID3D11DeviceContext, shader: &ID3D11VertexShader) {
    unsafe {
        context.IASetInputLayout(None);
        context.IASetPrimitiveTopology(D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
        context.VSSetShader(shader, None);
        context.RSSetState(None);
        context.OMSetBlendState(None, None, u32::MAX);
        context.OMSetDepthStencilState(None, 0);
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

#[derive(Clone, Copy)]
struct InverseOrientationMapping {
    source_axis_x: u32,
    source_axis_y: u32,
    inverse_x: [f32; 2],
    inverse_y: [f32; 2],
    offset: [f32; 2],
}

fn oriented_dimensions(width: u32, height: u32, orientation: VideoOrientation) -> (u32, u32) {
    if orientation.swaps_axes() {
        (height.max(1), width.max(1))
    } else {
        (width.max(1), height.max(1))
    }
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
    let source_axis_x =
        (transformed.iter().map(|value| value.0).max().unwrap_or(0) - min_x + 1) as u32;
    let source_axis_y =
        (transformed.iter().map(|value| value.1).max().unwrap_or(0) - min_y + 1) as u32;
    let determinant = i32::from(m11) * i32::from(m22) - i32::from(m12) * i32::from(m21);
    debug_assert!(determinant == 1 || determinant == -1);
    let inverse_x = [
        f32::from(m22) / determinant as f32,
        -f32::from(m12) / determinant as f32,
    ];
    let inverse_y = [
        -f32::from(m21) / determinant as f32,
        f32::from(m11) / determinant as f32,
    ];
    let offset = [
        min_x as f32 * inverse_x[0] + min_y as f32 * inverse_y[0],
        min_x as f32 * inverse_x[1] + min_y as f32 * inverse_y[1],
    ];
    InverseOrientationMapping {
        source_axis_x,
        source_axis_y,
        inverse_x,
        inverse_y,
        offset,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::panorama::PanoProjection;

    const HLSL: &str = include_str!("shaders/video_panorama.hlsl");

    #[test]
    fn hlsl_projection_codes_match_pano_projection() {
        for (name, projection) in [
            ("PROJ_PERSPECTIVE", PanoProjection::Perspective),
            ("PROJ_STEREOGRAPHIC", PanoProjection::Stereographic),
            ("PROJ_EQUIDISTANT", PanoProjection::Equidistant),
            ("PROJ_EQUISOLID", PanoProjection::EquisolidAngle),
        ] {
            assert!(HLSL.contains(&format!(
                "static const uint {name} = {};",
                projection.shader_code()
            )));
        }
    }

    #[test]
    fn panorama_pixel_shader_has_one_return_and_uses_sample_grad() {
        let body = function_body(HLSL, "float4 ps_main").expect("ps_main body");
        assert_eq!(body.matches("return").count(), 1);
        assert!(body.contains("SampleGrad"));
        assert!(body.contains("ddx"));
        assert!(body.contains("ddy"));
    }

    #[test]
    fn constant_pack_matches_the_still_panorama_uniform() {
        let pose = PanoPose::new(0.7, -0.2, 2.4, PanoProjection::Stereographic);
        let uv = PanoUvTransform {
            u_offset: 0.125,
            v_offset: 0.25,
            u_scale: 0.75,
            v_scale: 0.5,
        };
        let aspect = 16.0 / 9.0;
        assert_eq!(
            video_panorama_uniform_bytes(pose, aspect, uv),
            crate::panorama_wgpu::pano_uniform_bytes(pose, aspect, uv)
        );
    }

    #[test]
    fn quarter_turn_orientation_materializes_transposed_equirect() {
        let mapping = inverse_orientation_mapping(8, 4, VideoOrientation::new(90, false));
        assert_eq!((mapping.source_axis_x, mapping.source_axis_y), (4, 8));
    }

    fn function_body<'a>(source: &'a str, signature: &str) -> Option<&'a str> {
        let start = source.find(signature)?;
        let open = source[start..].find('{')? + start;
        let mut depth = 0_u32;
        for (offset, ch) in source[open..].char_indices() {
            match ch {
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        return Some(&source[open + 1..open + offset]);
                    }
                }
                _ => {}
            }
        }
        None
    }
}
