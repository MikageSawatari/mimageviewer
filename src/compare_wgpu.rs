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
    mode_wipe: vec2<f32>,
    _padding: vec2<f32>,
    uv_window: vec4<f32>,
};

@group(0) @binding(0) var pinned_tex: texture_2d<f32>;
@group(0) @binding(1) var current_tex: texture_2d<f32>;
@group(0) @binding(2) var compare_sampler: sampler;
@group(0) @binding(3) var<uniform> params: Params;

fn sample_compare_texture(tex: texture_2d<f32>, uv: vec2<f32>) -> vec4<f32> {
    return textureSample(tex, compare_sampler, uv);
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let image_uv = mix(params.uv_window.xy, params.uv_window.zw, in.uv);
    let pinned = sample_compare_texture(pinned_tex, image_uv);
    let current = sample_compare_texture(current_tex, image_uv);
    if (params.mode_wipe.x < 0.5) {
        if (image_uv.x <= params.mode_wipe.y) {
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CompareShaderSlot {
    Main,
    Navigator,
}

pub struct CompareShaderCallback {
    pub slot: CompareShaderSlot,
    pub key: u64,
    pub width: u32,
    pub height: u32,
    pub pinned_rgba: Arc<Vec<u8>>,
    pub current_rgba: Arc<Vec<u8>>,
    pub mode: CompareShaderMode,
    pub wipe_fraction: f32,
    pub uv_window: [f32; 4],
    pub target_format: wgpu::TextureFormat,
}

struct CompareGpuResources {
    target_format: wgpu::TextureFormat,
    pipeline: wgpu::RenderPipeline,
    bind_group_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    mipmap_generator: egui_wgpu::MipmapGenerator,
    // App側が保持する準備済み比較組は常に1組だけで、keyも再利用されない。
    // 過去組をHashMapへ残すと、完全なmip chainを持つ2 textureが組数分だけ
    // VRAMへ蓄積するため、GPU側も現在組1つだけを所有する。
    pair: Option<(u64, CompareGpuPair)>,
}

struct CompareGpuPair {
    _pinned_texture: wgpu::Texture,
    _current_texture: wgpu::Texture,
    _pinned_view: wgpu::TextureView,
    _current_view: wgpu::TextureView,
    // 重いtexture / mip chainはpairで共有し、prepareごとに変わる軽量状態だけを分離する。
    slots: CompareGpuSlots,
    width: u32,
    height: u32,
}

struct CompareGpuSlots {
    main: CompareGpuSlot,
    navigator: CompareGpuSlot,
}

struct CompareGpuSlot {
    bind_group: wgpu::BindGroup,
    uniform: wgpu::Buffer,
}

impl CompareGpuSlots {
    fn new(
        device: &wgpu::Device,
        bind_group_layout: &wgpu::BindGroupLayout,
        sampler: &wgpu::Sampler,
        pinned_view: &wgpu::TextureView,
        current_view: &wgpu::TextureView,
    ) -> Self {
        Self {
            main: CompareGpuSlot::new(
                device,
                bind_group_layout,
                sampler,
                pinned_view,
                current_view,
                CompareShaderSlot::Main,
            ),
            navigator: CompareGpuSlot::new(
                device,
                bind_group_layout,
                sampler,
                pinned_view,
                current_view,
                CompareShaderSlot::Navigator,
            ),
        }
    }

    fn get(&self, slot: CompareShaderSlot) -> &CompareGpuSlot {
        // 新しい呼び出し箇所をenumへ追加したとき、slotの割り当て漏れはcompile errorにする。
        match slot {
            CompareShaderSlot::Main => &self.main,
            CompareShaderSlot::Navigator => &self.navigator,
        }
    }
}

impl CompareGpuSlot {
    fn new(
        device: &wgpu::Device,
        bind_group_layout: &wgpu::BindGroupLayout,
        sampler: &wgpu::Sampler,
        pinned_view: &wgpu::TextureView,
        current_view: &wgpu::TextureView,
        slot: CompareShaderSlot,
    ) -> Self {
        let (uniform_label, bind_group_label) = match slot {
            CompareShaderSlot::Main => ("miv_compare_main_uniform", "miv_compare_main_bind_group"),
            CompareShaderSlot::Navigator => (
                "miv_compare_navigator_uniform",
                "miv_compare_navigator_bind_group",
            ),
        };
        let uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(uniform_label),
            size: 32,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some(bind_group_label),
            layout: bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(pinned_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(current_view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: uniform.as_entire_binding(),
                },
            ],
        });
        Self {
            bind_group,
            uniform,
        }
    }
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
            mipmap_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        Self {
            target_format,
            pipeline,
            bind_group_layout,
            sampler,
            mipmap_generator: egui_wgpu::MipmapGenerator::new(device),
            pair: None,
        }
    }

    fn ensure_pair(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        callback: &CompareShaderCallback,
    ) {
        let recreate = self
            .pair
            .as_ref()
            .map(|(key, pair)| {
                *key != callback.key
                    || pair.width != callback.width
                    || pair.height != callback.height
            })
            .unwrap_or(true);
        if recreate {
            // 新textureを確保する前に旧組をdropし、8K比較で旧/new組が同時に
            // VRAMへ残る時間を作らない。wgpu backend側のin-flight解放遅延を除けば、
            // CompareGpuResourcesが所有する完全mip chainは常に2枚までになる。
            self.pair = None;
            let pinned = upload_rgba_texture(
                device,
                queue,
                "miv_compare_pinned_texture",
                callback.width,
                callback.height,
                &callback.pinned_rgba,
                &self.mipmap_generator,
            );
            let current = upload_rgba_texture(
                device,
                queue,
                "miv_compare_current_texture",
                callback.width,
                callback.height,
                &callback.current_rgba,
                &self.mipmap_generator,
            );
            let slots = CompareGpuSlots::new(
                device,
                &self.bind_group_layout,
                &self.sampler,
                &pinned.view,
                &current.view,
            );
            self.pair = Some((
                callback.key,
                CompareGpuPair {
                    _pinned_texture: pinned.texture,
                    _current_texture: current.texture,
                    _pinned_view: pinned.view,
                    _current_view: current.view,
                    slots,
                    width: callback.width,
                    height: callback.height,
                },
            ));
        }
        if let Some((key, pair)) = self.pair.as_ref()
            && *key == callback.key
        {
            let write = uniform_write(
                callback.slot,
                callback.mode,
                callback.wipe_fraction,
                callback.uv_window,
            );
            queue.write_buffer(&pair.slots.get(write.slot).uniform, 0, &write.bytes);
        }
    }
}

/// 比較overlayを使わない状態へ移るとき、pipeline/samplerは保持したまま
/// 高解像度texture 2枚だけを即時dropする。
pub fn clear_gpu_pair(callback_resources: &mut egui_wgpu::CallbackResources) {
    if let Some(resources) = callback_resources.get_mut::<CompareGpuResources>() {
        resources.pair = None;
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
    mipmap_generator: &egui_wgpu::MipmapGenerator,
) -> UploadedTexture {
    let size = wgpu::Extent3d {
        width,
        height,
        depth_or_array_layers: 1,
    };
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size,
        mip_level_count: egui_wgpu::mip_level_count(width, height),
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING
            | wgpu::TextureUsages::COPY_DST
            | wgpu::TextureUsages::RENDER_ATTACHMENT,
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
    mipmap_generator.generate(device, queue, &texture);
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    UploadedTexture { texture, view }
}

fn uniform_bytes(mode: CompareShaderMode, wipe_fraction: f32, uv_window: [f32; 4]) -> [u8; 32] {
    let mode = match mode {
        CompareShaderMode::Wipe => 0.0_f32,
        CompareShaderMode::Diff => 1.0_f32,
    };
    let values = [
        mode,
        wipe_fraction.clamp(0.05, 0.95),
        0.0,
        0.0,
        uv_window[0],
        uv_window[1],
        uv_window[2],
        uv_window[3],
    ];
    let mut bytes = [0_u8; 32];
    for (i, value) in values.iter().enumerate() {
        bytes[i * 4..i * 4 + 4].copy_from_slice(&value.to_ne_bytes());
    }
    bytes
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct CompareUniformWrite {
    slot: CompareShaderSlot,
    bytes: [u8; 32],
}

fn uniform_write(
    slot: CompareShaderSlot,
    mode: CompareShaderMode,
    wipe_fraction: f32,
    uv_window: [f32; 4],
) -> CompareUniformWrite {
    CompareUniformWrite {
        slot,
        bytes: uniform_bytes(mode, wipe_fraction, uv_window),
    }
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
        let Some((key, pair)) = resources.pair.as_ref() else {
            return;
        };
        if *key != self.key {
            return;
        }
        render_pass.set_pipeline(&resources.pipeline);
        render_pass.set_bind_group(0, &pair.slots.get(self.slot).bind_group, &[]);
        render_pass.draw(0..6, 0..1);
    }
}

#[cfg(test)]
mod tests {
    use super::{CompareShaderMode, CompareShaderSlot, SHADER, uniform_bytes, uniform_write};

    #[test]
    fn compare_uses_fixed_mipmap_sampling_and_compact_uniform() {
        assert!(SHADER.contains("textureSample(tex, compare_sampler, uv)"));
        assert!(!SHADER.contains("textureSampleBias"));
        assert!(!SHADER.contains("textureSampleLevel"));
        let default_write = uniform_write(
            CompareShaderSlot::Main,
            CompareShaderMode::Wipe,
            0.5,
            [0.1, 0.2, 0.8, 0.9],
        );
        assert_eq!(default_write.slot, CompareShaderSlot::Main);
        let default_bytes = default_write.bytes;
        assert_eq!(
            f32::from_ne_bytes(default_bytes[0..4].try_into().unwrap()),
            0.0
        );
        assert_eq!(
            f32::from_ne_bytes(default_bytes[4..8].try_into().unwrap()),
            0.5
        );
        assert_eq!(&default_bytes[8..16], &[0; 8]);
        assert_eq!(
            default_bytes[16..]
                .chunks_exact(4)
                .map(|bytes| f32::from_ne_bytes(bytes.try_into().unwrap()))
                .collect::<Vec<_>>(),
            vec![0.1, 0.2, 0.8, 0.9]
        );

        let module = wgpu::naga::front::wgsl::parse_str(SHADER).expect("parse compare WGSL");
        wgpu::naga::valid::Validator::new(
            wgpu::naga::valid::ValidationFlags::all(),
            wgpu::naga::valid::Capabilities::all(),
        )
        .validate(&module)
        .expect("validate compare WGSL");
    }

    #[test]
    fn compare_uniform_writes_keep_main_and_navigator_bytes_in_distinct_slots() {
        let main = uniform_write(
            CompareShaderSlot::Main,
            CompareShaderMode::Wipe,
            0.37,
            [0.25, 0.2, 0.75, 0.8],
        );
        let navigator = uniform_write(
            CompareShaderSlot::Navigator,
            CompareShaderMode::Wipe,
            0.37,
            [0.0, 0.0, 1.0, 1.0],
        );

        assert_eq!(main.slot, CompareShaderSlot::Main);
        assert_eq!(navigator.slot, CompareShaderSlot::Navigator);
        assert_ne!(main.slot, navigator.slot);
        assert_ne!(main.bytes, navigator.bytes);
        assert_eq!(
            &main.bytes,
            &uniform_bytes(CompareShaderMode::Wipe, 0.37, [0.25, 0.2, 0.75, 0.8])
        );
        assert_eq!(
            &navigator.bytes,
            &uniform_bytes(CompareShaderMode::Wipe, 0.37, [0.0, 0.0, 1.0, 1.0])
        );
    }
}
