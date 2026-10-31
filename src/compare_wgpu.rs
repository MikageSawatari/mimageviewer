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
    pub owner_key: u64,
    pub width: u32,
    pub height: u32,
    pub pinned_rgba: Arc<Vec<u8>>,
    pub current_rgba: Arc<Vec<u8>>,
    pub cpu_pair_bytes: u64,
    pub cpu_pair_buffers: usize,
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
    // 現在の owner に加工版と元画像版の最大2組。各組に Main / Navigator の uniform を持つ。
    owner_key: Option<u64>,
    pairs: [Option<(u64, CompareGpuPair)>; 2],
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

pub fn rgba8_texture_bytes(width: u32, height: u32, mipmapped: bool) -> u128 {
    let mut width = u128::from(width);
    let mut height = u128::from(height);
    let mut total = 0_u128;
    loop {
        total += width * height * 4;
        if !mipmapped || (width == 1 && height == 1) {
            return total;
        }
        width = (width / 2).max(1);
        height = (height / 2).max(1);
    }
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
            bind_group_layouts: &[Some(&bind_group_layout)],
            immediate_size: 0,
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
            multiview_mask: None,
            cache: None,
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("miv_compare_sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            ..Default::default()
        });
        Self {
            target_format,
            pipeline,
            bind_group_layout,
            sampler,
            mipmap_generator: egui_wgpu::MipmapGenerator::new(device),
            owner_key: None,
            pairs: [None, None],
        }
    }

    fn ensure_pair(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        callback: &CompareShaderCallback,
    ) {
        let released_owner_bytes = if self.owner_key != Some(callback.owner_key) {
            let released: u128 = self
                .pairs
                .iter()
                .flatten()
                .map(|(_, pair)| rgba8_texture_bytes(pair.width, pair.height, true) * 2)
                .sum();
            self.pairs = [None, None];
            self.owner_key = Some(callback.owner_key);
            released
        } else {
            0
        };
        let variant_slot = compare_variant_slot(callback.owner_key, callback.key);
        let recreate = self.pairs[variant_slot]
            .as_ref()
            .map(|(key, pair)| {
                *key != callback.key
                    || pair.width != callback.width
                    || pair.height != callback.height
            })
            .unwrap_or(true);
        if recreate {
            if compare_texture_extent(callback.width, callback.height).is_none() {
                return;
            }
            let released_gpu_bytes = released_owner_bytes
                + self.pairs[variant_slot].take().map_or(0, |(_, pair)| {
                    rgba8_texture_bytes(pair.width, pair.height, true) * 2
                });
            let Some(pinned) = upload_rgba_texture(
                device,
                queue,
                "miv_compare_pinned_texture",
                callback.width,
                callback.height,
                &callback.pinned_rgba,
                &self.mipmap_generator,
            ) else {
                return;
            };
            let Some(current) = upload_rgba_texture(
                device,
                queue,
                "miv_compare_current_texture",
                callback.width,
                callback.height,
                &callback.current_rgba,
                &self.mipmap_generator,
            ) else {
                return;
            };
            let slots = CompareGpuSlots::new(
                device,
                &self.bind_group_layout,
                &self.sampler,
                &pinned.view,
                &current.view,
            );
            self.pairs[variant_slot] = Some((
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
            let gpu_bytes: u128 = self
                .pairs
                .iter()
                .flatten()
                .map(|(_, pair)| rgba8_texture_bytes(pair.width, pair.height, true) * 2)
                .sum();
            let gpu_textures = self.pairs.iter().flatten().count() * 2;
            crate::logger::log(format!(
                "[compare-memory] stage=gpu-ready target={}x{} cpu_total_bytes={} cpu_buffers={} gpu_total_bytes={} gpu_textures={} gpu_mips=full released_gpu_bytes={} old_pair_release=before_new_alloc",
                callback.width,
                callback.height,
                callback.cpu_pair_bytes,
                callback.cpu_pair_buffers,
                gpu_bytes,
                gpu_textures,
                released_gpu_bytes,
            ));
        }
        if let Some((key, pair)) = self.pairs[variant_slot].as_ref()
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
/// 加工版 / 元画像版の高解像度texture（最大4枚）を即時dropする。
pub fn clear_gpu_pair(callback_resources: &mut egui_wgpu::CallbackResources) {
    if let Some(resources) = callback_resources.get_mut::<CompareGpuResources>() {
        let released: u128 = resources
            .pairs
            .iter_mut()
            .filter_map(Option::take)
            .map(|(_, pair)| rgba8_texture_bytes(pair.width, pair.height, true) * 2)
            .sum();
        resources.owner_key = None;
        if released > 0 {
            crate::logger::log(format!(
                "[compare-memory] stage=gpu-clear released_gpu_bytes={released} gpu_total_bytes=0 gpu_mips=full",
            ));
        }
    }
}

fn compare_variant_slot(owner_key: u64, key: u64) -> usize {
    usize::from(owner_key != key)
}

struct UploadedTexture {
    texture: wgpu::Texture,
    view: wgpu::TextureView,
}

fn compare_texture_extent(width: u32, height: u32) -> Option<wgpu::Extent3d> {
    crate::capture::validate_compare_canvas_size([
        usize::try_from(width).ok()?,
        usize::try_from(height).ok()?,
    ])
    .ok()?;
    Some(wgpu::Extent3d {
        width,
        height,
        depth_or_array_layers: 1,
    })
}

fn create_compare_texture<T>(
    width: u32,
    height: u32,
    create: impl FnOnce(wgpu::Extent3d) -> T,
) -> Option<(wgpu::Extent3d, T)> {
    let size = compare_texture_extent(width, height)?;
    let texture = create(size);
    Some((size, texture))
}

fn upload_rgba_texture(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    label: &'static str,
    width: u32,
    height: u32,
    rgba: &[u8],
    mipmap_generator: &egui_wgpu::MipmapGenerator,
) -> Option<UploadedTexture> {
    let (size, texture) = create_compare_texture(width, height, |size| {
        device.create_texture(&wgpu::TextureDescriptor {
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
        })
    })?;
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
    Some(UploadedTexture { texture, view })
}

fn uniform_bytes(mode: CompareShaderMode, wipe_fraction: f32, uv_window: [f32; 4]) -> [u8; 32] {
    let mode = match mode {
        CompareShaderMode::Wipe => 0.0_f32,
        CompareShaderMode::Diff => 1.0_f32,
    };
    let values = [
        mode,
        wipe_fraction.clamp(0.0, 1.0),
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
        if resources.owner_key != Some(self.owner_key) {
            return;
        }
        let Some((key, pair)) =
            resources.pairs[compare_variant_slot(self.owner_key, self.key)].as_ref()
        else {
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
    use super::{
        CompareShaderMode, CompareShaderSlot, SHADER, create_compare_texture, rgba8_texture_bytes,
        uniform_bytes, uniform_write,
    };

    #[test]
    fn compare_memory_measurement_counts_the_complete_mip_chain() {
        assert_eq!(rgba8_texture_bytes(8320, 7296, false), 242_810_880);
        assert_eq!(rgba8_texture_bytes(8320, 7296, true), 323_747_664);
        assert_eq!(rgba8_texture_bytes(8320, 7296, true) * 2, 647_495_328);
        assert_eq!(rgba8_texture_bytes(4160, 7296, true) * 2, 323_747_432);
    }

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

    #[test]
    fn compare_canvas_over_limit_never_reaches_texture_creation() {
        let creation_reached = std::cell::Cell::new(false);
        let rejected = create_compare_texture(crate::app::MAX_TEXTURE_DIM as u32 + 1, 1, |_| {
            creation_reached.set(true);
        });

        assert!(rejected.is_none());
        assert!(!creation_reached.get());
        assert!(create_compare_texture(crate::app::MAX_TEXTURE_DIM as u32, 1, |_| ()).is_some());
    }

    #[test]
    fn compare_uniform_allows_wipe_at_both_image_edges() {
        for fraction in [0.0, 1.0] {
            let bytes = uniform_bytes(CompareShaderMode::Wipe, fraction, [0.0, 0.0, 1.0, 1.0]);
            assert_eq!(
                f32::from_ne_bytes(bytes[4..8].try_into().unwrap()),
                fraction
            );
        }
    }
}

#[cfg(test)]
mod gpu_tests {
    use super::*;
    use crate::egui037_stage4_gpu_tests::{FORMAT, OffscreenGpu, assert_rgba, callback};

    fn paint_callback(
        slot: CompareShaderSlot,
        uv_window: [f32; 4],
        pinned_rgba: Arc<Vec<u8>>,
    ) -> CompareShaderCallback {
        CompareShaderCallback {
            slot,
            key: 41,
            owner_key: 41,
            width: 4,
            height: 1,
            current_rgba: Arc::new([0, 255, 0, 255].repeat(4)),
            pinned_rgba,
            cpu_pair_bytes: 32,
            cpu_pair_buffers: 2,
            mode: CompareShaderMode::Wipe,
            wipe_fraction: 1.0,
            uv_window,
            target_format: FORMAT,
        }
    }

    #[test]
    fn compare_offscreen_callbacks_keep_main_and_navigator_uniforms_distinct() {
        let mut gpu = OffscreenGpu::new();
        let pinned = Arc::new(
            [
                [255, 0, 0, 255],
                [255, 0, 0, 255],
                [0, 0, 255, 255],
                [0, 0, 255, 255],
            ]
            .concat(),
        );
        // Both callbacks prepare before either paints. Sharing a uniform would let
        // the last prepare overwrite Main's UV window with Navigator's window.
        let jobs = [
            callback(
                egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(4.0, 4.0)),
                paint_callback(
                    CompareShaderSlot::Main,
                    [0.0, 0.0, 0.5, 1.0],
                    pinned.clone(),
                ),
            ),
            callback(
                egui::Rect::from_min_size(egui::pos2(4.0, 0.0), egui::vec2(4.0, 4.0)),
                paint_callback(CompareShaderSlot::Navigator, [0.5, 0.0, 1.0, 1.0], pinned),
            ),
        ];
        let pixels = gpu.render(&jobs, [8, 4], [0, 0, 0, 0]);
        assert_rgba(pixels[8], [255, 0, 0, 255]);
        assert_rgba(pixels[15], [0, 0, 255, 255]);
    }

    #[test]
    fn compare_offscreen_original_and_processed_keep_two_uploads_and_clear_together() {
        let mut gpu = OffscreenGpu::new();
        let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(4.0, 4.0));
        let mut textures = Vec::new();
        for (key, color) in [
            (41, [255, 0, 0, 255]),
            (93, [0, 0, 255, 255]),
            (41, [255, 0, 0, 255]),
            (93, [0, 0, 255, 255]),
        ] {
            let mut paint = paint_callback(
                CompareShaderSlot::Main,
                [0.0, 0.0, 1.0, 1.0],
                Arc::new(color.repeat(4)),
            );
            paint.key = key;
            let output = gpu.render(&[callback(rect, paint)], [4, 4], [0, 0, 0, 0]);
            assert_rgba(output[8], color);
            let resources = gpu
                .renderer
                .callback_resources
                .get::<CompareGpuResources>()
                .unwrap();
            let pair = &resources.pairs[compare_variant_slot(41, key)]
                .as_ref()
                .unwrap()
                .1;
            textures.push(pair._pinned_texture.clone());
        }
        assert_eq!(
            textures[0], textures[2],
            "releasing must reuse the processed upload"
        );
        assert_eq!(
            textures[1], textures[3],
            "holding again must reuse the original upload"
        );
        let resources = gpu
            .renderer
            .callback_resources
            .get::<CompareGpuResources>()
            .unwrap();
        assert!(resources.pairs.iter().all(Option::is_some));
        clear_gpu_pair(&mut gpu.renderer.callback_resources);
        let resources = gpu
            .renderer
            .callback_resources
            .get::<CompareGpuResources>()
            .unwrap();
        assert!(resources.pairs.iter().all(Option::is_none));
        assert!(resources.owner_key.is_none());
    }

    #[test]
    fn compare_offscreen_owner_change_releases_both_variants_without_clear() {
        let mut gpu = OffscreenGpu::new();
        let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(4.0, 4.0));
        for key in [41, 93] {
            let mut paint = paint_callback(
                CompareShaderSlot::Main,
                [0.0, 0.0, 1.0, 1.0],
                Arc::new([255, 0, 0, 255].repeat(4)),
            );
            paint.key = key;
            gpu.render(&[callback(rect, paint)], [4, 4], [0, 0, 0, 0]);
        }
        let resources = gpu
            .renderer
            .callback_resources
            .get::<CompareGpuResources>()
            .unwrap();
        assert_eq!(resources.owner_key, Some(41));
        let old_textures: Vec<_> = resources
            .pairs
            .iter()
            .map(|variant| {
                let pair = &variant.as_ref().expect("both variants must exist").1;
                (pair._pinned_texture.clone(), pair._current_texture.clone())
            })
            .collect();
        // A new owner drops both old variants, even when only one variant is requested.
        let mut paint = paint_callback(
            CompareShaderSlot::Navigator,
            [0.0, 0.0, 1.0, 1.0],
            Arc::new([0, 255, 0, 255].repeat(4)),
        );
        paint.owner_key = 101;
        paint.key = 101;
        gpu.render(&[callback(rect, paint)], [4, 4], [0, 0, 0, 0]);
        let resources = gpu
            .renderer
            .callback_resources
            .get::<CompareGpuResources>()
            .unwrap();
        assert_eq!(resources.owner_key, Some(101));
        let (key, pair) = resources.pairs[0].as_ref().unwrap();
        assert_eq!(*key, 101);
        for (old_pinned, old_current) in old_textures {
            assert_ne!(pair._pinned_texture, old_pinned);
            assert_ne!(pair._current_texture, old_current);
        }
        assert!(resources.pairs[1].is_none());
    }

    #[test]
    fn compare_offscreen_callback_composites_translucent_pixels() {
        let mut gpu = OffscreenGpu::new();
        let jobs = [callback(
            egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(4.0, 4.0)),
            paint_callback(
                CompareShaderSlot::Main,
                [0.0, 0.0, 1.0, 1.0],
                Arc::new([200, 40, 80, 128].repeat(4)),
            ),
        )];
        for (clear, expected) in [
            ([0, 0, 0, 0], [100, 20, 40, 128]),
            ([20, 60, 100, 255], [110, 50, 90, 255]),
        ] {
            let pixels = gpu.render(&jobs, [4, 4], clear);
            for pixel in pixels {
                assert_rgba(pixel, expected);
            }
        }
    }
}
