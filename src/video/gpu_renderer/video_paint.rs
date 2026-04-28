//! egui_wgpu の `CallbackTrait` を実装し、共有 D3D11 RGBA テクスチャを fullscreen quad
//! として描画する。
//!
//! ## 流れ
//! 1. App 起動時に `init_video_pipeline` を呼んで `VideoPipeline` (= shader / sampler /
//!    bind group layout) を `Renderer::callback_resources` に挿入する。
//! 2. UI tick でビデオフレームが届いた瞬間に、共有 NT handle を
//!    `import_shared_d3d11_texture` で wgpu::Texture 化し、`VideoPaintCallback` を
//!    `egui::PaintCallback` 経由で `egui::Painter` に積む。
//! 3. egui_wgpu Renderer が `prepare` で bind group を構築、`paint` で fullscreen quad を
//!    draw する。
//!
//! ## なぜ texture を毎フレーム作り直すか
//! 共有テクスチャは decoder が出力するたび新しい NT handle で渡ってくる
//! (= 毎フレーム別の D3D11 ID3D11Texture2D)。一定数の ring buffer 化で再利用するには
//! D3D11 側 / wgpu 側の同期が複雑になる (keyed mutex / fence)。コストは
//! `OpenSharedHandle + create_texture_from_hal + bind group` で 1080p で ~0.5ms /
//! 4K で ~1ms と無視できる範囲。

use std::sync::Arc;

use egui_wgpu::CallbackTrait;
use windows::Win32::Foundation::HANDLE;
use wgpu::util::DeviceExt;

use super::wgpu_import::{ImportedTexture, import_shared_d3d11_texture};

/// 1 度だけ作成する重いリソース (= shader / pipeline / sampler / bind group layout)。
/// eframe `Renderer::callback_resources` に挿入し、各 paint で参照する。
pub struct VideoPipeline {
    pub pipeline: wgpu::RenderPipeline,
    pub sampler: wgpu::Sampler,
    pub bind_group_layout: wgpu::BindGroupLayout,
}

const VIDEO_SHADER: &str = r#"
@group(0) @binding(0) var t_video: texture_2d<f32>;
@group(0) @binding(1) var s_video: sampler;

struct VsOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) vid: u32) -> VsOut {
    // Fullscreen triangle (3 vertices that cover the whole NDC space).
    var positions = array<vec2<f32>, 3>(
        vec2<f32>(-1.0, -3.0),
        vec2<f32>(-1.0,  1.0),
        vec2<f32>( 3.0,  1.0),
    );
    var uvs = array<vec2<f32>, 3>(
        vec2<f32>(0.0, 2.0),
        vec2<f32>(0.0, 0.0),
        vec2<f32>(2.0, 0.0),
    );
    var out: VsOut;
    out.pos = vec4<f32>(positions[vid], 0.0, 1.0);
    out.uv = uvs[vid];
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    return textureSample(t_video, s_video, in.uv);
}
"#;

/// `App` 起動時に 1 度だけ呼んで、wgpu パイプラインを `RenderState::renderer.callback_resources`
/// に登録する。eframe は描画中にこの resources を CallbackTrait に渡してくる。
pub fn init_video_pipeline(render_state: &egui_wgpu::RenderState) {
    let device = &render_state.device;
    let target_format = render_state.target_format;

    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("video-paint-shader"),
        source: wgpu::ShaderSource::Wgsl(VIDEO_SHADER.into()),
    });

    let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("video-paint-bgl"),
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
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                count: None,
            },
        ],
    });

    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("video-paint-pl"),
        bind_group_layouts: &[&bind_group_layout],
        push_constant_ranges: &[],
    });

    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("video-paint-pipeline"),
        layout: Some(&pipeline_layout),
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: Some("vs_main"),
            compilation_options: Default::default(),
            buffers: &[],
        },
        fragment: Some(wgpu::FragmentState {
            module: &shader,
            entry_point: Some("fs_main"),
            compilation_options: Default::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format: target_format,
                blend: Some(wgpu::BlendState::REPLACE),
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        primitive: wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::TriangleList,
            strip_index_format: None,
            front_face: wgpu::FrontFace::Ccw,
            cull_mode: None,
            unclipped_depth: false,
            polygon_mode: wgpu::PolygonMode::Fill,
            conservative: false,
        },
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        multiview: None,
        cache: None,
    });

    let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some("video-paint-sampler"),
        address_mode_u: wgpu::AddressMode::ClampToEdge,
        address_mode_v: wgpu::AddressMode::ClampToEdge,
        address_mode_w: wgpu::AddressMode::ClampToEdge,
        mag_filter: wgpu::FilterMode::Linear,
        min_filter: wgpu::FilterMode::Linear,
        mipmap_filter: wgpu::FilterMode::Nearest,
        lod_min_clamp: 0.0,
        lod_max_clamp: 0.0,
        compare: None,
        anisotropy_clamp: 1,
        border_color: None,
    });

    render_state
        .renderer
        .write()
        .callback_resources
        .insert(VideoPipeline {
            pipeline,
            sampler,
            bind_group_layout,
        });
    crate::logger::log("Video paint pipeline registered in egui_wgpu callback_resources".to_string());
}

/// 1 フレーム分の描画コールバック。`prepare` で wgpu テクスチャを import + bind group を作り、
/// `paint` で fullscreen quad を draw する。
pub struct VideoPaintCallback {
    /// 共有 NT handle (decoder thread が `IDXGIResource1::CreateSharedHandle` で取得)。
    /// `Arc<Mutex>` で包むのは `prepare` で 1 度しか open しないためだが、
    /// CallbackTrait は `&self` を要求するので内部 mutability を持つ。
    handle: HANDLE,
    width: u32,
    height: u32,
    ten_bit: bool,
    /// `prepare` で作った per-frame state を持つ。`Arc<Mutex>` で `&self` 互換。
    inner: Arc<std::sync::Mutex<VideoPaintInner>>,
}

#[derive(Default)]
struct VideoPaintInner {
    imported: Option<ImportedTexture>,
    bind_group: Option<wgpu::BindGroup>,
}

impl VideoPaintCallback {
    pub fn new(handle: HANDLE, width: u32, height: u32, ten_bit: bool) -> Self {
        Self {
            handle,
            width,
            height,
            ten_bit,
            inner: Arc::new(std::sync::Mutex::new(VideoPaintInner::default())),
        }
    }
}

// HANDLE は raw pointer 相当だが、egui_wgpu は CallbackTrait に Send + Sync を要求する。
// HANDLE を所有して thread を渡るのは安全 (= D3d11Frame と同様の論理)。
unsafe impl Send for VideoPaintCallback {}
unsafe impl Sync for VideoPaintCallback {}

impl CallbackTrait for VideoPaintCallback {
    fn prepare(
        &self,
        device: &wgpu::Device,
        _queue: &wgpu::Queue,
        _screen_descriptor: &egui_wgpu::ScreenDescriptor,
        _egui_encoder: &mut wgpu::CommandEncoder,
        callback_resources: &mut egui_wgpu::CallbackResources,
    ) -> Vec<wgpu::CommandBuffer> {
        let pipeline = match callback_resources.get::<VideoPipeline>() {
            Some(p) => p,
            None => {
                crate::logger::log(
                    "VideoPaintCallback::prepare: VideoPipeline not registered".to_string(),
                );
                return Vec::new();
            }
        };

        let imported = match unsafe {
            import_shared_d3d11_texture(device, self.handle, self.width, self.height, self.ten_bit)
        } {
            Ok(t) => t,
            Err(e) => {
                crate::logger::log(format!(
                    "VideoPaintCallback::prepare: import_shared_d3d11_texture failed: {e}"
                ));
                return Vec::new();
            }
        };

        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("video-paint-bg"),
            layout: &pipeline.bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&imported.view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&pipeline.sampler),
                },
            ],
        });

        let mut inner = self.inner.lock().unwrap();
        inner.imported = Some(imported);
        inner.bind_group = Some(bind_group);
        Vec::new()
    }

    fn paint(
        &self,
        _info: egui::PaintCallbackInfo,
        render_pass: &mut wgpu::RenderPass<'static>,
        callback_resources: &egui_wgpu::CallbackResources,
    ) {
        let pipeline = match callback_resources.get::<VideoPipeline>() {
            Some(p) => p,
            None => return,
        };
        let inner = self.inner.lock().unwrap();
        let bind_group = match inner.bind_group.as_ref() {
            Some(b) => b,
            None => return,
        };
        render_pass.set_pipeline(&pipeline.pipeline);
        render_pass.set_bind_group(0, bind_group, &[]);
        render_pass.draw(0..3, 0..1);
    }
}

// `wgpu::util::DeviceExt` の使い道はないので prevent unused import warning。
#[allow(dead_code)]
fn _device_ext_marker(_d: &wgpu::Device) {
    let _ = std::any::type_name::<dyn DeviceExt>();
}
