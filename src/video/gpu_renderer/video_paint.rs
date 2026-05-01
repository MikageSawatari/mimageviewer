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
use wgpu::util::DeviceExt;
use windows::Win32::Foundation::HANDLE;

use super::wgpu_import::{ImportedTexture, import_shared_d3d11_texture};

/// 1 度だけ作成する重いリソース (= shader / pipeline / sampler / bind group layout)。
/// eframe `Renderer::callback_resources` に挿入し、各 paint で参照する。
pub struct VideoPipeline {
    pub pipeline: wgpu::RenderPipeline,
    pub sampler: wgpu::Sampler,
    pub bind_group_layout: wgpu::BindGroupLayout,
}

/// D3D11 → D3D12 共有 fence の D3D12 側エンドポイント。`GpuVideoDevice` の
/// fence_shared_handle を 1 回 `OpenSharedHandle` して `ID3D12Fence` を得たら、
/// `callback_resources` にキャッシュしておく。
/// 動画を切り替えると新しい `GpuVideoDevice` (= 新しい fence) になるので、
/// `cached_gen` (プロセス内ユニーク世代 ID) を比較して必要に応じて再 open する。
/// HANDLE 値だけだと kernel が値を再利用したときに stale な fence を使ってしまう。
struct VideoFenceInterop {
    cached_gen: u64,
    fence: windows_058::Win32::Graphics::Direct3D12::ID3D12Fence,
}

// `ID3D12Fence` は COM オブジェクトで windows-rs の `Interface` 実装上 Send + Sync 安全。
// `cached_handle` は単なる i64 値なので問題ない。
unsafe impl Send for VideoFenceInterop {}
unsafe impl Sync for VideoFenceInterop {}

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
    crate::logger::log(
        "Video paint pipeline registered in egui_wgpu callback_resources".to_string(),
    );
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
    /// `GpuVideoDevice` の fence の NT shared handle。`prepare` で 1 回だけ
    /// `ID3D12Device::OpenSharedHandle` し、`callback_resources` にキャッシュ。
    fence_shared_handle: HANDLE,
    /// このフレームに対応する fence 値。`ID3D12CommandQueue::Wait(fence, fence_value)` する。
    fence_value: u64,
    /// プロセス内ユニーク fence 世代 ID (キャッシュ判定キー)。
    fence_gen: u64,
    /// `prepare` で作った per-frame state を持つ。`Arc<Mutex>` で `&self` 互換。
    inner: Arc<std::sync::Mutex<VideoPaintInner>>,
}

#[derive(Default)]
struct VideoPaintInner {
    imported: Option<ImportedTexture>,
    bind_group: Option<wgpu::BindGroup>,
}

/// `callback_resources` にキャッシュする per-handle import + bind group。
/// UI が 60fps paint で decoder が 30fps だと、毎 paint で同じ shared_handle が来るので
/// import/bind_group をキャッシュ流用する。動画切替で handle が変わったら破棄して作り直す。
struct VideoFrameCache {
    cached_handle: isize,
    cached_fence_value: u64,
    /// device 切替後の HANDLE 値再利用に対する identity (Codex P2)。fence_value は
    /// 新 device で 1 から振り直されるので handle + fence_value だけだとヒットしてしまう。
    cached_fence_gen: u64,
    cached_width: u32,
    cached_height: u32,
    cached_ten_bit: bool,
    /// `wgpu::Texture` の所有者。bind_group が中で参照を持つので、cache 寿命中は drop
    /// してはいけない (= dead_code warning を許容する)。
    #[allow(dead_code)]
    imported: ImportedTexture,
    bind_group: wgpu::BindGroup,
}

// ImportedTexture が抱える HANDLE は raw ptr 相当だが、共有 NT handle として
// thread を渡る前提なので Send/Sync を unsafe impl (D3d11Frame 等と同じ論理)。
// `wgpu::Texture/View/BindGroup` はそれ自体 Send/Sync 安全。
unsafe impl Send for VideoFrameCache {}
unsafe impl Sync for VideoFrameCache {}

impl VideoPaintCallback {
    pub fn new(
        handle: HANDLE,
        width: u32,
        height: u32,
        ten_bit: bool,
        fence_shared_handle: HANDLE,
        fence_value: u64,
        fence_gen: u64,
    ) -> Self {
        Self {
            handle,
            width,
            height,
            ten_bit,
            fence_shared_handle,
            fence_value,
            fence_gen,
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
        queue: &wgpu::Queue,
        _screen_descriptor: &egui_wgpu::ScreenDescriptor,
        _egui_encoder: &mut wgpu::CommandEncoder,
        callback_resources: &mut egui_wgpu::CallbackResources,
    ) -> Vec<wgpu::CommandBuffer> {
        let prepare_start = std::time::Instant::now();
        if !callback_resources.get::<VideoPipeline>().is_some() {
            crate::logger::log(
                "VideoPaintCallback::prepare: VideoPipeline not registered".to_string(),
            );
            return Vec::new();
        }

        // Fence interop のキャッシュをチェック (世代 ID で判定)。
        let need_reopen_fence = match callback_resources.get::<VideoFenceInterop>() {
            Some(iop) => iop.cached_gen != self.fence_gen,
            None => true,
        };
        if need_reopen_fence {
            match unsafe { open_d3d12_fence(device, self.fence_shared_handle) } {
                Ok(fence) => {
                    callback_resources.insert(VideoFenceInterop {
                        cached_gen: self.fence_gen,
                        fence,
                    });
                }
                Err(e) => {
                    crate::logger::log(format!(
                        "VideoPaintCallback::prepare: open_d3d12_fence failed: {e}"
                    ));
                    return Vec::new();
                }
            }
        }

        // 共有テクスチャ import + bind group はキャッシュ可能 (handle + fence_value が
        // 同じならスキップ)。UI が 60fps paint / decoder が 30fps で paint 重複時に効く。
        // device 切替で fence_value が 1 から振り直され、HANDLE 値も kernel に再利用
        // されうるため、`fence_gen` + dims + ten_bit も identity に含める (Codex P2)。
        let cache_hit = match callback_resources.get::<VideoFrameCache>() {
            Some(c) => {
                c.cached_handle == self.handle.0 as isize
                    && c.cached_fence_value == self.fence_value
                    && c.cached_fence_gen == self.fence_gen
                    && c.cached_width == self.width
                    && c.cached_height == self.height
                    && c.cached_ten_bit == self.ten_bit
            }
            None => false,
        };
        let mut imported_ms = 0.0_f64;
        let mut bg_ms = 0.0_f64;
        if !cache_hit {
            let t_imp = std::time::Instant::now();
            let imported = match unsafe {
                import_shared_d3d11_texture(
                    device,
                    self.handle,
                    self.width,
                    self.height,
                    self.ten_bit,
                )
            } {
                Ok(t) => t,
                Err(e) => {
                    crate::logger::log(format!(
                        "VideoPaintCallback::prepare: import_shared_d3d11_texture failed: {e}"
                    ));
                    return Vec::new();
                }
            };
            imported_ms = t_imp.elapsed().as_secs_f64() * 1000.0;

            let t_bg = std::time::Instant::now();
            let pipeline = match callback_resources.get::<VideoPipeline>() {
                Some(p) => p,
                None => return Vec::new(),
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
            bg_ms = t_bg.elapsed().as_secs_f64() * 1000.0;

            callback_resources.insert(VideoFrameCache {
                cached_handle: self.handle.0 as isize,
                cached_fence_value: self.fence_value,
                cached_fence_gen: self.fence_gen,
                cached_width: self.width,
                cached_height: self.height,
                cached_ten_bit: self.ten_bit,
                imported,
                bind_group,
            });
        }

        // GPU 側で D3D11 fence が fence_value に到達するまで wait。失敗時は skip。
        let wait_ok = if let Some(iop) = callback_resources.get::<VideoFenceInterop>() {
            match unsafe { queue_wait_fence(queue, &iop.fence, self.fence_value) } {
                Ok(()) => true,
                Err(e) => {
                    crate::logger::log(format!(
                        "VideoPaintCallback::prepare: queue Wait failed: {e}"
                    ));
                    false
                }
            }
        } else {
            false
        };
        if !wait_ok {
            return Vec::new();
        }

        // inner にも参照を流し込む (paint() で取り出すため)。imported は cache 側にあるが、
        // CallbackResources からの per-callback 抜き出しに type-id 衝突回避が要るので、
        // bind_group は paint 側でも cache から get する流れにする (= inner には何も入れない)。
        // ※ 旧設計の `inner` は不要だが、API 互換のため残す。
        let mut inner = self.inner.lock().unwrap();
        inner.imported = None;
        inner.bind_group = None;
        drop(inner);

        // 100 prepare ごとに 1 行診断ログ。"prepare 全部の時間" + "import" + "bind_group"
        // + cache hit/miss で stutter 原因を切り分ける。
        use std::sync::atomic::{AtomicU64, Ordering};
        static PREP_COUNT: AtomicU64 = AtomicU64::new(0);
        let n = PREP_COUNT.fetch_add(1, Ordering::Relaxed);
        if n == 0 || n % 100 == 0 {
            let total_ms = prepare_start.elapsed().as_secs_f64() * 1000.0;
            crate::logger::log(format!(
                "VideoPaintCallback::prepare #{n} total={total_ms:.2}ms \
                 import={imported_ms:.2}ms bg={bg_ms:.2}ms cache_hit={cache_hit} \
                 fence_value={}",
                self.fence_value
            ));
        }

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
        // bind_group は VideoFrameCache 内に置いてある (= prepare で insert)。
        let cache = match callback_resources.get::<VideoFrameCache>() {
            Some(c) => c,
            None => return,
        };
        // キャッシュが現在のコールバックの handle/fence_value と一致しないなら skip。
        // (= paint だけ追従していないケース。通常は prepare とペアで同期して呼ばれる)
        if cache.cached_handle != self.handle.0 as isize
            || cache.cached_fence_value != self.fence_value
        {
            return;
        }
        render_pass.set_pipeline(&pipeline.pipeline);
        render_pass.set_bind_group(0, &cache.bind_group, &[]);
        render_pass.draw(0..3, 0..1);
    }
}

// `wgpu::util::DeviceExt` の使い道はないので prevent unused import warning。
#[allow(dead_code)]
fn _device_ext_marker(_d: &wgpu::Device) {
    let _ = std::any::type_name::<dyn DeviceExt>();
}

/// `GpuVideoDevice::fence_shared_handle` を D3D12 側で開いて `ID3D12Fence` を得る。
/// SAFETY: handle が valid であり、wgpu_device が DX12 backend であること。
unsafe fn open_d3d12_fence(
    wgpu_device: &wgpu::Device,
    handle: HANDLE,
) -> Result<windows_058::Win32::Graphics::Direct3D12::ID3D12Fence, String> {
    use windows_058::Win32::Foundation::HANDLE as WinHandle058;
    use windows_058::Win32::Graphics::Direct3D12::ID3D12Fence;

    // windows 0.61 と 0.58 の HANDLE は同サイズ・同 alignment の transparent newtype。
    // (wgpu_import.rs の `import_shared_d3d11_texture` でも同じ変換を行っている)
    let handle_058 = WinHandle058(handle.0);

    unsafe {
        let hal_dev = wgpu_device
            .as_hal::<wgpu_hal::api::Dx12>()
            .ok_or_else(|| "wgpu device is not dx12".to_string())?;
        let d3d12 = hal_dev.raw_device();
        let mut fence: Option<ID3D12Fence> = None;
        d3d12
            .OpenSharedHandle(handle_058, &mut fence)
            .map_err(|e| format!("OpenSharedHandle: {e:?}"))?;
        fence.ok_or_else(|| "OpenSharedHandle returned null fence".to_string())
    }
}

/// wgpu の DX12 command queue に `Wait(fence, value)` を発行する。GPU 上で
/// fence がその値に到達するまで以後の draw を遅延させる (CPU はブロックしない)。
/// SAFETY: queue が DX12 backend であること、fence が同じ adapter から open されたこと。
unsafe fn queue_wait_fence(
    queue: &wgpu::Queue,
    fence: &windows_058::Win32::Graphics::Direct3D12::ID3D12Fence,
    value: u64,
) -> Result<(), String> {
    unsafe {
        let hal_queue = queue
            .as_hal::<wgpu_hal::api::Dx12>()
            .ok_or_else(|| "wgpu queue is not dx12".to_string())?;
        let raw_queue = hal_queue.as_raw();
        raw_queue
            .Wait(fence, value)
            .map_err(|e| format!("ID3D12CommandQueue::Wait: {e:?}"))
    }
}
