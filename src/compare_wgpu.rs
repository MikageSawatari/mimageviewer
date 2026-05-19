use std::collections::HashMap;
use std::sync::Arc;

const SHADER: &str = r#"
struct VsOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) vertex_index: u32) -> VsOut {
    var positions = array<vec2<f32>, 6>(
        vec2<f32>(-1.0, -1.0),
        vec2<f32>( 1.0, -1.0),
        vec2<f32>( 1.0,  1.0),
        vec2<f32>(-1.0, -1.0),
        vec2<f32>( 1.0,  1.0),
        vec2<f32>(-1.0,  1.0),
    );
    var uvs = array<vec2<f32>, 6>(
        vec2<f32>(0.0, 1.0),
        vec2<f32>(1.0, 1.0),
        vec2<f32>(1.0, 0.0),
        vec2<f32>(0.0, 1.0),
        vec2<f32>(1.0, 0.0),
        vec2<f32>(0.0, 0.0),
    );

    var out: VsOut;
    out.pos = vec4<f32>(positions[vertex_index], 0.0, 1.0);
    out.uv = uvs[vertex_index];
    return out;
}

struct Params {
    mode_wipe: vec4<f32>,
};

@group(0) @binding(0) var pinned_tex: texture_2d<f32>;
@group(0) @binding(1) var current_tex: texture_2d<f32>;
@group(0) @binding(2) var compare_sampler: sampler;
@group(0) @binding(3) var<uniform> params: Params;

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let pinned = textureSample(pinned_tex, compare_sampler, in.uv);
    let current = textureSample(current_tex, compare_sampler, in.uv);
    if (params.mode_wipe.x < 0.5) {
        if (in.uv.x <= params.mode_wipe.y) {
            return pinned;
        }
        return current;
    }

    let delta = abs(pinned.rgb - current.rgb);
    return vec4<f32>(sqrt(delta), 1.0);
}
"#;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CompareShaderMode {
    Wipe,
    Diff,
}

pub struct CompareShaderCallback {
    pub key: u64,
    pub width: u32,
    pub height: u32,
    pub pinned_rgba: Arc<Vec<u8>>,
    pub current_rgba: Arc<Vec<u8>>,
    pub mode: CompareShaderMode,
    pub wipe_fraction: f32,
    pub target_format: wgpu::TextureFormat,
}

struct CompareGpuResources {
    target_format: wgpu::TextureFormat,
    pipeline: wgpu::RenderPipeline,
    bind_group_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    pairs: HashMap<u64, CompareGpuPair>,
}

struct CompareGpuPair {
    _pinned_texture: wgpu::Texture,
    _current_texture: wgpu::Texture,
    _pinned_view: wgpu::TextureView,
    _current_view: wgpu::TextureView,
    bind_group: wgpu::BindGroup,
    uniform: wgpu::Buffer,
    width: u32,
    height: u32,
}

impl CompareGpuResources {
    fn new(device: &wgpu::Device, target_format: wgpu::TextureFormat) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("miv_compare_shader"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("miv_compare_bind_group_layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
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
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("miv_compare_pipeline_layout"),
            bind_group_layouts: &[&bind_group_layout],
            push_constant_ranges: &[],
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("miv_compare_pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                buffers: &[],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: target_format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
            cache: None,
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("miv_compare_sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::FilterMode::Nearest,
            ..Default::default()
        });
        Self {
            target_format,
            pipeline,
            bind_group_layout,
            sampler,
            pairs: HashMap::new(),
        }
    }

    fn ensure_pair(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        callback: &CompareShaderCallback,
    ) {
        let recreate = self
            .pairs
            .get(&callback.key)
            .map(|p| p.width != callback.width || p.height != callback.height)
            .unwrap_or(true);
        if recreate {
            if self.pairs.len() > 8 {
                self.pairs.clear();
            }
            let pinned = upload_rgba_texture(
                device,
                queue,
                "miv_compare_pinned_texture",
                callback.width,
                callback.height,
                &callback.pinned_rgba,
            );
            let current = upload_rgba_texture(
                device,
                queue,
                "miv_compare_current_texture",
                callback.width,
                callback.height,
                &callback.current_rgba,
            );
            let uniform = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("miv_compare_uniform"),
                size: 16,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("miv_compare_bind_group"),
                layout: &self.bind_group_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&pinned.view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(&current.view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::Sampler(&self.sampler),
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: uniform.as_entire_binding(),
                    },
                ],
            });
            self.pairs.insert(
                callback.key,
                CompareGpuPair {
                    _pinned_texture: pinned.texture,
                    _current_texture: current.texture,
                    _pinned_view: pinned.view,
                    _current_view: current.view,
                    bind_group,
                    uniform,
                    width: callback.width,
                    height: callback.height,
                },
            );
        }
        if let Some(pair) = self.pairs.get(&callback.key) {
            queue.write_buffer(
                &pair.uniform,
                0,
                &uniform_bytes(callback.mode, callback.wipe_fraction),
            );
        }
    }
}

struct UploadedTexture {
    texture: wgpu::Texture,
    view: wgpu::TextureView,
}

fn upload_rgba_texture(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    label: &'static str,
    width: u32,
    height: u32,
    rgba: &[u8],
) -> UploadedTexture {
    let size = wgpu::Extent3d {
        width,
        height,
        depth_or_array_layers: 1,
    };
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        rgba,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(width * 4),
            rows_per_image: Some(height),
        },
        size,
    );
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    UploadedTexture { texture, view }
}

fn uniform_bytes(mode: CompareShaderMode, wipe_fraction: f32) -> [u8; 16] {
    let mode = match mode {
        CompareShaderMode::Wipe => 0.0_f32,
        CompareShaderMode::Diff => 1.0_f32,
    };
    let values = [mode, wipe_fraction.clamp(0.05, 0.95), 0.0, 0.0];
    let mut bytes = [0_u8; 16];
    for (i, value) in values.iter().enumerate() {
        bytes[i * 4..i * 4 + 4].copy_from_slice(&value.to_ne_bytes());
    }
    bytes
}

impl egui_wgpu::CallbackTrait for CompareShaderCallback {
    fn prepare(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        _screen_descriptor: &egui_wgpu::ScreenDescriptor,
        _egui_encoder: &mut wgpu::CommandEncoder,
        callback_resources: &mut egui_wgpu::CallbackResources,
    ) -> Vec<wgpu::CommandBuffer> {
        if callback_resources
            .get::<CompareGpuResources>()
            .is_none_or(|r| r.target_format != self.target_format)
        {
            callback_resources.insert(CompareGpuResources::new(device, self.target_format));
        }
        if let Some(resources) = callback_resources.get_mut::<CompareGpuResources>() {
            resources.ensure_pair(device, queue, self);
        }
        Vec::new()
    }

    fn paint(
        &self,
        _info: egui::epaint::PaintCallbackInfo,
        render_pass: &mut wgpu::RenderPass<'static>,
        callback_resources: &egui_wgpu::CallbackResources,
    ) {
        let Some(resources) = callback_resources.get::<CompareGpuResources>() else {
            return;
        };
        let Some(pair) = resources.pairs.get(&self.key) else {
            return;
        };
        render_pass.set_pipeline(&resources.pipeline);
        render_pass.set_bind_group(0, &pair.bind_group, &[]);
        render_pass.draw(0..6, 0..1);
    }
}
