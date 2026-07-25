//! D3D11 full-frame tone and Creative LUT pass for the native video presenter.
//!
//! The identity path in `NativeVideoPresenter` continues to use its original
//! copy/upload implementation. This pipeline is created lazily only when at
//! least one adjustment is active.

use local_adjust_core::CubeLutParams;
use windows::Win32::Graphics::Direct3D::Fxc::D3DCompile;
use windows::Win32::Graphics::Direct3D::{
    D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST, ID3DBlob, ID3DInclude,
};
use windows::Win32::Graphics::Direct3D11::{
    D3D11_BIND_CONSTANT_BUFFER, D3D11_BIND_SHADER_RESOURCE, D3D11_BUFFER_DESC,
    D3D11_FILTER_MIN_MAG_MIP_LINEAR, D3D11_SAMPLER_DESC, D3D11_SUBRESOURCE_DATA,
    D3D11_TEXTURE_ADDRESS_CLAMP, D3D11_TEXTURE2D_DESC, D3D11_TEXTURE3D_DESC, D3D11_USAGE_DEFAULT,
    D3D11_VIEWPORT, ID3D11Buffer, ID3D11Device1, ID3D11DeviceContext, ID3D11PixelShader,
    ID3D11Resource, ID3D11SamplerState, ID3D11ShaderResourceView, ID3D11Texture2D, ID3D11Texture3D,
    ID3D11VertexShader,
};
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_FORMAT_R32G32B32A32_FLOAT, DXGI_SAMPLE_DESC,
};
use windows::core::{Interface, PCSTR};

const GRADE_SHADER: &[u8] = br#"
Texture2D<float4> source_tex : register(t0);
Texture3D<float4> lut_tex : register(t1);
SamplerState linear_sampler : register(s0);

cbuffer GradeConstants : register(b0) {
    float4 tone0;       // brightness, contrast, gamma, saturation
    float4 tone1;       // temperature, black point, white point, midtone
    float4 domain_min;  // rgb, unused
    float4 domain_inv;  // rgb, LUT strength
    float4 lut_coord;   // scale, offset, enabled, unused
};

struct VsOut {
    float4 position : SV_Position;
    float2 uv : TEXCOORD0;
};

VsOut vs_main(uint vertex_id : SV_VertexID) {
    VsOut output;
    float2 uv = float2((vertex_id << 1) & 2, vertex_id & 2);
    output.position = float4(uv * float2(2.0, -2.0) + float2(-1.0, 1.0), 0.0, 1.0);
    output.uv = uv;
    return output;
}

float4 ps_main(VsOut input) : SV_Target {
    float4 sampled = source_tex.SampleLevel(linear_sampler, input.uv, 0.0);
    float3 color = sampled.rgb;

    float range = max(tone1.z - tone1.y, 1.0 / 255.0);
    color = pow(saturate((color - tone1.y) / range), 1.0 / max(tone1.w, 0.1));
    color = pow(saturate(color), 1.0 / max(tone0.z, 0.2));

    float contrast_factor =
        (259.0 * (tone0.y + 255.0)) / (255.0 * (259.0 - tone0.y));
    color = (contrast_factor * (color * 255.0 - 128.0)
        + 128.0 + tone0.x * 2.55) / 255.0;

    if (tone0.w <= -99.999) {
        color = dot(color, float3(0.299, 0.587, 0.114)).xxx;
    } else {
        float lum = (max(color.r, max(color.g, color.b))
            + min(color.r, min(color.g, color.b))) * 0.5;
        color = lum.xxx + (color - lum.xxx) * (1.0 + tone0.w / 100.0);
    }
    color.r += tone1.x * 0.5 / 255.0;
    color.b -= tone1.x * 0.5 / 255.0;
    color = saturate(color);

    if (lut_coord.z > 0.5) {
        float3 normalized = saturate((color - domain_min.rgb) * domain_inv.rgb);
        float3 coord = normalized * lut_coord.x + lut_coord.y;
        float3 graded = lut_tex.SampleLevel(linear_sampler, coord, 0.0).rgb;
        color = lerp(color, graded, domain_inv.w);
    }
    return float4(saturate(color), sampled.a);
}
"#;

#[repr(C)]
#[derive(Clone, Copy)]
struct GradeConstants {
    tone0: [f32; 4],
    tone1: [f32; 4],
    domain_min: [f32; 4],
    domain_inv: [f32; 4],
    lut_coord: [f32; 4],
}

struct CpuSource {
    width: u32,
    height: u32,
    texture: ID3D11Texture2D,
    view: ID3D11ShaderResourceView,
}

pub(super) struct VideoGradePipeline {
    vertex_shader: ID3D11VertexShader,
    pixel_shader: ID3D11PixelShader,
    sampler: ID3D11SamplerState,
    constants: ID3D11Buffer,
    lut_texture: Option<ID3D11Texture3D>,
    lut_view: Option<ID3D11ShaderResourceView>,
    lut_key: Option<(uuid::Uuid, usize)>,
    cpu_source: Option<CpuSource>,
}

impl VideoGradePipeline {
    pub(super) fn new(device: &ID3D11Device1) -> Result<Self, String> {
        let vertex_blob = compile_shader("vs_main", "vs_5_0")?;
        let pixel_blob = compile_shader("ps_main", "ps_5_0")?;
        let vertex_bytes = blob_bytes(&vertex_blob);
        let pixel_bytes = blob_bytes(&pixel_blob);
        let mut vertex_shader = None;
        let mut pixel_shader = None;
        let mut sampler = None;
        let mut constants = None;
        unsafe {
            device
                .CreateVertexShader(vertex_bytes, None, Some(&mut vertex_shader))
                .map_err(|error| format!("CreateVertexShader: {error:?}"))?;
            device
                .CreatePixelShader(pixel_bytes, None, Some(&mut pixel_shader))
                .map_err(|error| format!("CreatePixelShader: {error:?}"))?;
            let sampler_desc = D3D11_SAMPLER_DESC {
                Filter: D3D11_FILTER_MIN_MAG_MIP_LINEAR,
                AddressU: D3D11_TEXTURE_ADDRESS_CLAMP,
                AddressV: D3D11_TEXTURE_ADDRESS_CLAMP,
                AddressW: D3D11_TEXTURE_ADDRESS_CLAMP,
                MaxLOD: f32::MAX,
                ..Default::default()
            };
            device
                .CreateSamplerState(&sampler_desc, Some(&mut sampler))
                .map_err(|error| format!("CreateSamplerState: {error:?}"))?;
            let constants_desc = D3D11_BUFFER_DESC {
                ByteWidth: std::mem::size_of::<GradeConstants>() as u32,
                Usage: D3D11_USAGE_DEFAULT,
                BindFlags: D3D11_BIND_CONSTANT_BUFFER.0 as u32,
                ..Default::default()
            };
            device
                .CreateBuffer(&constants_desc, None, Some(&mut constants))
                .map_err(|error| format!("CreateBuffer grade constants: {error:?}"))?;
        }
        Ok(Self {
            vertex_shader: vertex_shader
                .ok_or_else(|| "CreateVertexShader returned null".to_string())?,
            pixel_shader: pixel_shader
                .ok_or_else(|| "CreatePixelShader returned null".to_string())?,
            sampler: sampler.ok_or_else(|| "CreateSamplerState returned null".to_string())?,
            constants: constants.ok_or_else(|| "CreateBuffer returned null".to_string())?,
            lut_texture: None,
            lut_view: None,
            lut_key: None,
            cpu_source: None,
        })
    }

    pub(super) fn update_grade(
        &mut self,
        device: &ID3D11Device1,
        context: &ID3D11DeviceContext,
        grade: &crate::creative_lut::VideoGradeSnapshot,
    ) -> Result<(), String> {
        let selected_lut = grade.lut.as_deref().filter(|lut| {
            grade.adjustments.creative_lut.id.is_some()
                && grade.adjustments.creative_lut.strength > f32::EPSILON
                && lut.is_loaded()
        });
        let selected_lut_key = selected_lut.and_then(|_| {
            grade.adjustments.creative_lut.id.zip(
                grade
                    .lut
                    .as_ref()
                    .map(|lut| std::sync::Arc::as_ptr(lut) as usize),
            )
        });
        let (domain_min, domain_max, size) = selected_lut
            .map(|lut| (lut.domain_min, lut.domain_max, lut.size))
            .unwrap_or(([0.0; 3], [1.0; 3], 2));
        let constants = GradeConstants {
            tone0: [
                grade.adjustments.brightness,
                grade.adjustments.contrast,
                grade.adjustments.gamma,
                grade.adjustments.saturation,
            ],
            tone1: [
                grade.adjustments.temperature,
                grade.adjustments.black_point as f32 / 255.0,
                grade.adjustments.white_point as f32 / 255.0,
                grade.adjustments.midtone,
            ],
            domain_min: [domain_min[0], domain_min[1], domain_min[2], 0.0],
            domain_inv: [
                1.0 / (domain_max[0] - domain_min[0]).max(f32::EPSILON),
                1.0 / (domain_max[1] - domain_min[1]).max(f32::EPSILON),
                1.0 / (domain_max[2] - domain_min[2]).max(f32::EPSILON),
                grade.adjustments.creative_lut.strength.clamp(0.0, 1.0),
            ],
            lut_coord: [
                (size.saturating_sub(1)) as f32 / size as f32,
                0.5 / size as f32,
                if selected_lut.is_some() { 1.0 } else { 0.0 },
                0.0,
            ],
        };
        unsafe {
            context.UpdateSubresource(
                &self.constants,
                0,
                None,
                (&constants as *const GradeConstants).cast(),
                0,
                0,
            );
        }
        if let Some(lut) = selected_lut {
            if self.lut_key != selected_lut_key {
                let (texture, view) = create_lut_texture(device, lut)?;
                self.lut_texture = Some(texture);
                self.lut_view = Some(view);
                self.lut_key = selected_lut_key;
            }
        } else {
            self.lut_texture = None;
            self.lut_view = None;
            self.lut_key = None;
        }
        Ok(())
    }

    pub(super) fn upload_cpu_source(
        &mut self,
        device: &ID3D11Device1,
        context: &ID3D11DeviceContext,
        bytes: &[u8],
        width: u32,
        height: u32,
    ) -> Result<ID3D11Texture2D, String> {
        if self
            .cpu_source
            .as_ref()
            .is_none_or(|source| source.width != width || source.height != height)
        {
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
                BindFlags: D3D11_BIND_SHADER_RESOURCE.0 as u32,
                ..Default::default()
            };
            let mut texture = None;
            unsafe {
                device
                    .CreateTexture2D(&desc, None, Some(&mut texture))
                    .map_err(|error| format!("CreateTexture2D grade CPU source: {error:?}"))?;
            }
            let texture = texture
                .ok_or_else(|| "CreateTexture2D grade CPU source returned null".to_string())?;
            let view = create_shader_view(device, &texture)?;
            self.cpu_source = Some(CpuSource {
                width,
                height,
                texture,
                view,
            });
        }
        let source = self.cpu_source.as_ref().expect("created above");
        unsafe {
            context.UpdateSubresource(
                &source.texture,
                0,
                None,
                bytes.as_ptr().cast(),
                width.saturating_mul(4),
                0,
            );
        }
        Ok(source.texture.clone())
    }

    pub(super) fn draw(
        &self,
        device: &ID3D11Device1,
        context: &ID3D11DeviceContext,
        source: &ID3D11Texture2D,
        target: &ID3D11Texture2D,
        width: u32,
        height: u32,
    ) -> Result<(), String> {
        let source_view = if let Some(cpu) = self
            .cpu_source
            .as_ref()
            .filter(|cpu| cpu.texture.as_raw() == source.as_raw())
        {
            cpu.view.clone()
        } else {
            create_shader_view(device, source)?
        };
        let mut target_view = None;
        unsafe {
            device
                .CreateRenderTargetView(target, None, Some(&mut target_view))
                .map_err(|error| format!("CreateRenderTargetView grade target: {error:?}"))?;
        }
        let target_view =
            target_view.ok_or_else(|| "CreateRenderTargetView grade returned null".to_string())?;

        unsafe {
            context.IASetInputLayout(None);
            context.IASetPrimitiveTopology(D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
            context.VSSetShader(&self.vertex_shader, None);
            context.PSSetShader(&self.pixel_shader, None);
            context.PSSetConstantBuffers(0, Some(&[Some(self.constants.clone())]));
            context.PSSetSamplers(0, Some(&[Some(self.sampler.clone())]));
            context.PSSetShaderResources(0, Some(&[Some(source_view), self.lut_view.clone()]));
            context.RSSetState(None);
            context.RSSetViewports(Some(&[D3D11_VIEWPORT {
                TopLeftX: 0.0,
                TopLeftY: 0.0,
                Width: width as f32,
                Height: height as f32,
                MinDepth: 0.0,
                MaxDepth: 1.0,
            }]));
            context.OMSetBlendState(None, None, u32::MAX);
            context.OMSetDepthStencilState(None, 0);
            context.OMSetRenderTargets(Some(&[Some(target_view)]), None);
            context.Draw(3, 0);
            // Unbind resources so a later frame can update or recreate them
            // without D3D11 read/write hazard warnings.
            context.PSSetShaderResources(0, Some(&[None, None]));
            context.OMSetRenderTargets(None, None);
        }
        Ok(())
    }
}

fn compile_shader(entry: &'static str, target: &'static str) -> Result<ID3DBlob, String> {
    let mut bytecode = None;
    let mut errors = None;
    let entry = std::ffi::CString::new(entry).expect("static shader entry");
    let target = std::ffi::CString::new(target).expect("static shader target");
    let result = unsafe {
        D3DCompile(
            GRADE_SHADER.as_ptr().cast(),
            GRADE_SHADER.len(),
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
        .map_err(|error| format!("cast shader resource: {error:?}"))?;
    let mut view = None;
    unsafe {
        device
            .CreateShaderResourceView(&resource, None, Some(&mut view))
            .map_err(|error| format!("CreateShaderResourceView: {error:?}"))?;
    }
    view.ok_or_else(|| "CreateShaderResourceView returned null".to_string())
}

fn create_lut_texture(
    device: &ID3D11Device1,
    lut: &CubeLutParams,
) -> Result<(ID3D11Texture3D, ID3D11ShaderResourceView), String> {
    let rgba = lut
        .table
        .iter()
        .map(|rgb| [rgb[0], rgb[1], rgb[2], 1.0])
        .collect::<Vec<_>>();
    let size = lut.size as u32;
    let desc = D3D11_TEXTURE3D_DESC {
        Width: size,
        Height: size,
        Depth: size,
        MipLevels: 1,
        Format: DXGI_FORMAT_R32G32B32A32_FLOAT,
        Usage: D3D11_USAGE_DEFAULT,
        BindFlags: D3D11_BIND_SHADER_RESOURCE.0 as u32,
        ..Default::default()
    };
    let initial = D3D11_SUBRESOURCE_DATA {
        pSysMem: rgba.as_ptr().cast(),
        SysMemPitch: size.saturating_mul(16),
        SysMemSlicePitch: size.saturating_mul(size).saturating_mul(16),
    };
    let mut texture = None;
    unsafe {
        device
            .CreateTexture3D(&desc, Some(&initial), Some(&mut texture))
            .map_err(|error| format!("CreateTexture3D Creative LUT: {error:?}"))?;
    }
    let texture =
        texture.ok_or_else(|| "CreateTexture3D Creative LUT returned null".to_string())?;
    let view = create_shader_view(device, &texture)?;
    Ok((texture, view))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grade_hlsl_compiles_for_shader_model_5() {
        compile_shader("vs_main", "vs_5_0").expect("vertex shader");
        compile_shader("ps_main", "ps_5_0").expect("pixel shader");
    }
}
