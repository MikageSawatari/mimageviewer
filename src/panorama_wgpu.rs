//! 360 度パノラマビュー (equirectangular projection) の wgpu callback。
//!
//! 設計は [docs/panorama-360-view-plan.md](../docs/panorama-360-view-plan.md) を参照。
//! 構造は [`compare_wgpu`] とほぼ同じだが、以下の点で異なる:
//!
//! - **callback はテクスチャ実体を持たない** (§4.1)。アップロード済み wgpu リソースは
//!   App 側 (`pano_uploaded: Option<Arc<UploadedPanoTexture>>`) で管理し、callback は
//!   毎フレーム `source_key + cache_key` で stale チェックする
//! - **WGSL は equirect の逆射影**を fragment で計算 (§3.3)
//! - 静的リソース ([`PanoStaticGpu`]) は `target_format` 単位で 1 つだけ作る
//! - `BindGroup` は [`UploadedPanoTexture`] 構築時に焼き付ける (毎フレーム再生成しない)
//!
//! Base texture uses a complete mip chain with trilinear filtering. The screen-sized
//! settle overlay remains single-level; anisotropic filtering is not used.

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
    out.uv  = uvs[vertex_index];
    return out;
}

struct Params {
    // pose.xyzw = yaw / pitch / fov_y / aspect (radians, viewport_w/viewport_h)
    pose: vec4<f32>,
    // crop.xy = uv_offset (フル球面上での画像左上の位置、[0,1])
    // crop.zw = uv_scale (フル球面に対する画像の覆う範囲、[0,1])
    // フル equirect なら (0, 0, 1, 1)。
    // 部分 FOV equirect (GPano CroppedArea*) は実画像 / フル球面比から計算 (Phase 1.5)。
    crop: vec4<f32>,
};

@group(0) @binding(0) var pano_tex: texture_2d<f32>;
@group(0) @binding(1) var pano_samp: sampler;
@group(0) @binding(2) var<uniform> params: Params;

const PI: f32 = 3.141592653589793;
const INV_TWO_PI: f32 = 0.15915494309189535;
const INV_PI: f32 = 0.3183098861837907;

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let yaw    = params.pose.x;
    let pitch  = params.pose.y;
    let fov_y  = params.pose.z;
    let aspect = params.pose.w;

    let tan_half = tan(fov_y * 0.5);
    // NDC: 中心 (0,0)、Y は上向き正
    let ndc = vec2<f32>(in.uv.x * 2.0 - 1.0, 1.0 - in.uv.y * 2.0);
    let cam_dir = normalize(vec3<f32>(
        ndc.x * tan_half * aspect,
        ndc.y * tan_half,
        -1.0,
    ));

    // pitch (X 軸回転) → yaw (Y 軸回転) を順に適用
    let cp = cos(pitch);
    let sp = sin(pitch);
    let p1 = vec3<f32>(
        cam_dir.x,
        cp * cam_dir.y - sp * cam_dir.z,
        sp * cam_dir.y + cp * cam_dir.z,
    );
    let cy = cos(yaw);
    let sy = sin(yaw);
    let wd = vec3<f32>(
        cy * p1.x + sy * p1.z,
        p1.y,
        -sy * p1.x + cy * p1.z,
    );

    let lon = atan2(wd.x, -wd.z);
    let lat = asin(clamp(wd.y, -1.0, 1.0));
    // フル球面 UV (0..1)
    let sphere_uv = vec2<f32>(lon * INV_TWO_PI + 0.5, 0.5 - lat * INV_PI);
    // 画像が球面の crop 範囲 (crop.xy ～ crop.xy+crop.zw) しか覆っていない場合に、
    // 球面 UV を画像テクスチャ UV に変換する (Phase 1.5: 部分 FOV equirect 対応)。
    // フル equirect なら crop = (0,0,1,1) で恒等変換。
    let texture_uv_raw = (sphere_uv - params.crop.xy) / params.crop.zw;
    // **crop 時は軸別に half-texel inset で clamp** (Codex P2 第 22/23 ラウンド)。
    //
    // 背景: sampler は `address_mode_u: Repeat` + `address_mode_v: ClampToEdge` の
    // 組み合わせ。フル equirect の U 経度シーム (u=0 と u=1 の連続) を自然にラップ
    // させるため U は Repeat にしている。crop 時に texture_uv.x が [0,1] 範囲外に
    // なると Repeat が反対側の画素を取りに行き、欠落視野に画像が複製表示される。
    //
    // **軸別判定**: 垂直 crop のみ (= DSLR 三脚 nodal panhead 撮影の典型、水平 360° は
    // フルに覆い垂直は天頂 / 地面が抜けている) のとき、U は Repeat の seam wrap が
    // 自然なので clamp しない。第 22 ラウンドの単一 `crop_active` だと U も clamp
    // されてしまい、シーム位置で 1 texel ぶん端色が引き伸ばされて不自然になる。
    // U / V を別フラグで判定して、必要な軸だけ clamp する (第 23 ラウンド反映)。
    //
    // **Linear filter 対応 (half-texel inset)**: `u = 0.0 ちょうど` をサンプルすると
    // Linear が左右の隣接 texel を補間する。Repeat の場合「u<0 相当」は反対端 texel
    // を取りに行くため、境界 1 texel ぶんで反対端の色が 50% 混ざる。これを防ぐには、
    // 最外側 texel の **中心** に対応する `[0.5/W, 1 - 0.5/W]` に clamp する
    // (= ハードウェア ClampToEdge 相当の動作)。
    //
    // **判定**: `u_crop` / `v_crop` は scale が 1 から離れている (= 部分覆い) または
    // offset が 0 から離れている (= 位置ずれ、稀) のどちらかで真。フル equirect
    // (IDENTITY) は両方とも偽になり、U Repeat の seam wrap + V ClampToEdge という
    // 「平時」の挙動を維持する。
    let u_crop = (params.crop.z < 0.999) || (abs(params.crop.x) > 0.001);
    let v_crop = (params.crop.w < 0.999) || (abs(params.crop.y) > 0.001);
    let dims = vec2<f32>(textureDimensions(pano_tex));
    let half_texel = 0.5 / dims;
    let max_uv = vec2<f32>(1.0, 1.0) - half_texel;
    let texture_uv = vec2<f32>(
        select(
            texture_uv_raw.x,
            clamp(texture_uv_raw.x, half_texel.x, max_uv.x),
            u_crop,
        ),
        select(
            texture_uv_raw.y,
            clamp(texture_uv_raw.y, half_texel.y, max_uv.y),
            v_crop,
        ),
    );
    return textureSample(pano_tex, pano_samp, texture_uv);
}
"#;

/// 360 度パノラマビュー callback の本体。
///
/// テクスチャ実体は持たず、`source_key + cache_key` で `UploadedPanoTextureRef`
/// (App 側でアップロード済みのもの) を CallbackResources から引く。
/// stale (補正 / AI / 別画像へナビ) なら `paint` は静かに no-op になり、
/// 8K base が未アップロードのときと同じく描画しない。
#[derive(Clone, Debug)]
pub struct PanoramaShaderCallback {
    /// `App::metadata_cache_key(idx)` の戻り値。どの画像に対応する 360 描画かを表す。
    pub source_key: String,
    /// `(idx_hash, source_kind, adjust_gen, ai_gen)` を u64 にパックしたキー。
    /// `resolve_pano_source(fs_idx)` の出力をそのまま焼き付ける (§4.1.2)。
    pub cache_key: u64,
    pub yaw: f32,
    pub pitch: f32,
    pub fov_y: f32,
    pub aspect: f32,
    /// Phase 1.5 部分 FOV equirect: フル球面 UV → 画像テクスチャ UV 変換。
    /// `crate::panorama::PanoUvTransform::IDENTITY` ならフル equirect。
    /// GPano `CroppedArea*` 宣言から計算 (`App::compute_pano_uv_transform`)。
    pub uv_transform: crate::panorama::PanoUvTransform,
    pub target_format: wgpu::TextureFormat,
}

/// `CallbackResources` 経由で `paint` に渡される、アップロード済みパノラマテクスチャ。
/// App 側で `Option<Arc<UploadedPanoTexture>>` を持ち、毎フレーム CallbackResources に
/// `Arc::clone` を newtype 包んで insert する。
///
/// `uniform` フィールドは `bind_group` の binding(2) に焼き込まれているため、
/// 構造体内で **必ず保持** する必要がある (drop されると bind_group が dangling 化)。
/// 毎フレーム `prepare()` 内で `queue.write_buffer(&uniform, ..)` で yaw/pitch/fov を更新する。
///
/// `target_format` は構築時のフォーマット。`prepare()` で `PanoStaticGpu` を新フォーマット
/// で作り直したとき、`bind_group` の `layout` (= 旧 `PanoStaticGpu.bind_group_layout`) が
/// 不一致になる可能性があるので、stale 判定 (Codex P2 第 19 ラウンド) で使う。
pub struct UploadedPanoTexture {
    pub source_key: String,
    pub cache_key: u64,
    pub target_format: wgpu::TextureFormat,
    pub texture: wgpu::Texture,
    pub view: wgpu::TextureView,
    pub bind_group: wgpu::BindGroup,
    pub uniform: wgpu::Buffer,
    pub width: u32,
    pub height: u32,
}

/// CallbackResources に insert するための newtype ラッパ。
/// `Arc<UploadedPanoTexture>` をそのまま入れると型 ID が `Arc<...>` で安定しないため、
/// 専用 newtype にしている (compare_wgpu の `CompareGpuResources` と同じ思想)。
pub struct UploadedPanoTextureRef(pub Arc<UploadedPanoTexture>);

/// `target_format` 単位で 1 つだけ作る静的リソース。pipeline / bind_group_layout /
/// sampler を含む。target_format が変わったら作り直す (compare_wgpu と同パターン)。
pub struct PanoStaticGpu {
    pub target_format: wgpu::TextureFormat,
    pub pipeline: wgpu::RenderPipeline,
    pub bind_group_layout: wgpu::BindGroupLayout,
    pub sampler: wgpu::Sampler,
    mipmap_generator: egui_wgpu::MipmapGenerator,
}

impl PanoStaticGpu {
    pub fn new(device: &wgpu::Device, target_format: wgpu::TextureFormat) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("miv_panorama_shader"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("miv_panorama_bind_group_layout"),
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
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
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
            label: Some("miv_panorama_pipeline_layout"),
            bind_group_layouts: &[&bind_group_layout],
            push_constant_ranges: &[],
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("miv_panorama_pipeline"),
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
        // U 方向は経度ラップ (Repeat)、V 方向は極でクランプ (ClampToEdge)。
        // 広角 FOV での強い縮小に備えて、mip level 間も Linear 補間する。
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("miv_panorama_sampler"),
            address_mode_u: wgpu::AddressMode::Repeat,
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
        }
    }

    /// 8K (or 任意サイズ) RGBA8 を新規 wgpu テクスチャとしてアップロードし、
    /// bind_group まで作って [`UploadedPanoTexture`] にまとめて返す。
    ///
    /// 呼び出し側 (`App`) は返り値を `Arc` で包んで `pano_uploaded` に格納する。
    /// **テクスチャ寿命**: `UploadedPanoTexture` が drop されると wgpu リソースも
    /// drop される (CPU 側 RGBA は `fs_cache` 側で管理)。
    ///
    /// **コスト見積もり** (8K = 8192×4096 RGBA8 = 134 MB):
    /// - `create_texture`: ~1 ms
    /// - `queue.write_texture`: ~10-30 ms (PCIe 転送)
    /// - mip chain generation: GPU render passes (level 0 に対して約 1/3 texel 追加)
    /// - `create_bind_group`: <1 ms
    pub fn create_uploaded_texture(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        source_key: String,
        cache_key: u64,
        width: u32,
        height: u32,
        rgba: &[u8],
    ) -> UploadedPanoTexture {
        let extent = wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        };
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("miv_panorama_texture"),
            size: extent,
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
            extent,
        );
        self.mipmap_generator.generate(device, queue, &texture);
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());

        // Uniform buffer は paint() の前 (prepare()) で毎フレーム write_buffer される。
        // ここでは初期値 0 で作っておく (Params = 2 × vec4<f32> = 32 bytes)。
        let uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("miv_panorama_uniform"),
            size: 32,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("miv_panorama_bind_group"),
            layout: &self.bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: uniform.as_entire_binding(),
                },
            ],
        });

        // uniform は `bind_group` の binding(2) に焼き込まれているので、struct に
        // 保持して dangling 化を防ぐ。`prepare()` 内で `queue.write_buffer` 経由で
        // yaw/pitch/fov を毎フレ更新する。
        UploadedPanoTexture {
            source_key,
            cache_key,
            target_format: self.target_format,
            texture,
            view,
            bind_group,
            uniform,
            width,
            height,
        }
    }
}

impl egui_wgpu::CallbackTrait for PanoramaShaderCallback {
    fn prepare(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        _screen_descriptor: &egui_wgpu::ScreenDescriptor,
        _egui_encoder: &mut wgpu::CommandEncoder,
        callback_resources: &mut egui_wgpu::CallbackResources,
    ) -> Vec<wgpu::CommandBuffer> {
        // 静的リソース (pipeline / layout / sampler) を初回 or target_format 変化時に作る。
        let need_rebuild = callback_resources
            .get::<PanoStaticGpu>()
            .is_none_or(|r| r.target_format != self.target_format);
        if need_rebuild {
            callback_resources.insert(PanoStaticGpu::new(device, self.target_format));
        }

        // 動的: uniform buffer の更新。
        // App 側で `pano_uploaded` の Arc を `UploadedPanoTextureRef` newtype で
        // CallbackResources に毎フレーム挿入している前提。
        if let Some(uploaded) = callback_resources.get::<UploadedPanoTextureRef>() {
            // stale 一致時のみ uniform を書く。`paint` 側でも同じ stale guard を掛けるが、
            // 後のフレームで stale が解消したときに古い pose で 1 フレーム描画する事故を
            // 防ぐため、毎フレ最新 yaw/pitch/fov を流し込んでおく。
            if uploaded.0.source_key == self.source_key && uploaded.0.cache_key == self.cache_key {
                let bytes = pano_uniform_bytes(
                    self.yaw,
                    self.pitch,
                    self.fov_y,
                    self.aspect,
                    self.uv_transform,
                );
                queue.write_buffer(&uploaded.0.uniform, 0, &bytes);
            }
        }
        let _ = device;
        Vec::new()
    }

    fn paint(
        &self,
        _info: egui::epaint::PaintCallbackInfo,
        render_pass: &mut wgpu::RenderPass<'static>,
        callback_resources: &egui_wgpu::CallbackResources,
    ) {
        let Some(resources) = callback_resources.get::<PanoStaticGpu>() else {
            return;
        };
        let Some(uploaded) = callback_resources.get::<UploadedPanoTextureRef>() else {
            // アップロード未完了: 何も描画しない (上位レイヤーで通常の 2D 表示が出る)
            return;
        };
        // stale guard: source_key と cache_key の両方一致を要求 (§4.1)
        if uploaded.0.source_key != self.source_key {
            return;
        }
        if uploaded.0.cache_key != self.cache_key {
            return;
        }
        render_pass.set_pipeline(&resources.pipeline);
        render_pass.set_bind_group(0, &uploaded.0.bind_group, &[]);
        render_pass.draw(0..6, 0..1);
    }
}

// ──────────────────────────────────────────────────────────────────
// Phase 2a: settle-refinement overlay (§3.6 / §4.6)
// 8K base の上に、CPU で計算した 1920×960 (or 任意) の RGBA overlay を
// alpha ブレンドして描画する。
// ──────────────────────────────────────────────────────────────────

const SETTLE_OVERLAY_SHADER: &str = r#"
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
    out.uv  = uvs[vertex_index];
    return out;
}

struct OverlayParams {
    // x: alpha (0.0 = transparent, 1.0 = full overlay)
    // y/z/w: reserved (0)
    pose: vec4<f32>,
};

@group(0) @binding(0) var overlay_tex: texture_2d<f32>;
@group(0) @binding(1) var overlay_samp: sampler;
@group(0) @binding(2) var<uniform> params: OverlayParams;

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let alpha = clamp(params.pose.x, 0.0, 1.0);
    let c = textureSample(overlay_tex, overlay_samp, in.uv);
    return vec4<f32>(c.rgb, c.a * alpha);
}
"#;

/// settle overlay 用静的 GPU リソース。`target_format` 単位で 1 つ。
pub struct SettleOverlayGpu {
    pub target_format: wgpu::TextureFormat,
    pub pipeline: wgpu::RenderPipeline,
    pub bind_group_layout: wgpu::BindGroupLayout,
    pub sampler: wgpu::Sampler,
}

impl SettleOverlayGpu {
    pub fn new(device: &wgpu::Device, target_format: wgpu::TextureFormat) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("miv_panorama_settle_shader"),
            source: wgpu::ShaderSource::Wgsl(SETTLE_OVERLAY_SHADER.into()),
        });
        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("miv_panorama_settle_bgl"),
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
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
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
            label: Some("miv_panorama_settle_pl"),
            bind_group_layouts: &[&bind_group_layout],
            push_constant_ranges: &[],
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("miv_panorama_settle_pipeline"),
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
            label: Some("miv_panorama_settle_sampler"),
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
        }
    }

    /// settle overlay RGBA を新規 wgpu テクスチャに格納し bind_group まで作る。
    pub fn create_uploaded_overlay(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        width: u32,
        height: u32,
        rgba: &[u8],
    ) -> UploadedSettleOverlay {
        let extent = wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        };
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("miv_panorama_settle_texture"),
            size: extent,
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
            extent,
        );
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("miv_panorama_settle_uniform"),
            size: 16, // 1 × vec4<f32>
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("miv_panorama_settle_bg"),
            layout: &self.bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: uniform.as_entire_binding(),
                },
            ],
        });
        UploadedSettleOverlay {
            target_format: self.target_format,
            texture,
            view,
            bind_group,
            uniform,
            width,
            height,
        }
    }
}

/// settle overlay の wgpu リソース実体 (`SettleOverlay.gpu` に Arc で格納)。
///
/// `target_format` は構築時のフォーマット。`upload_settle_overlay` で
/// `SettleOverlayGpu` を新フォーマットで作り直したとき、`bind_group` の `layout`
/// (= 旧 `SettleOverlayGpu.bind_group_layout`) が不一致になる可能性があるので、
/// stale 判定 (Codex P2 第 2、2026-05) で使う。
pub struct UploadedSettleOverlay {
    pub target_format: wgpu::TextureFormat,
    pub texture: wgpu::Texture,
    pub view: wgpu::TextureView,
    pub bind_group: wgpu::BindGroup,
    pub uniform: wgpu::Buffer,
    pub width: u32,
    pub height: u32,
}

/// settle overlay callback (alpha blend で 8K base の上に描画)。
/// 描画 ordering: ui_fullscreen が `try_paint_panorama` 後にこれを emit すれば、
/// egui_wgpu のレンダパス内で同フレーム後段で描かれる。
#[derive(Clone)]
pub struct SettleOverlayCallback {
    pub source_key: String,
    pub cache_key: u64,
    pub pose: (f32, f32, f32),
    pub alpha: f32,
    pub target_format: wgpu::TextureFormat,
    /// `SettleOverlay.gpu` から `Arc::clone` で渡される。
    /// `dyn Any` 内の実型は `UploadedSettleOverlay`。
    pub gpu: Arc<dyn std::any::Any + Send + Sync>,
}

/// CallbackResources に挿入される settle overlay の参照 newtype。
pub struct SettleOverlayRef {
    pub source_key: String,
    pub cache_key: u64,
    pub pose: (f32, f32, f32),
    pub alpha: f32,
    pub gpu: Arc<UploadedSettleOverlay>,
}

impl egui_wgpu::CallbackTrait for SettleOverlayCallback {
    fn prepare(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        _screen_descriptor: &egui_wgpu::ScreenDescriptor,
        _egui_encoder: &mut wgpu::CommandEncoder,
        callback_resources: &mut egui_wgpu::CallbackResources,
    ) -> Vec<wgpu::CommandBuffer> {
        let need_rebuild = callback_resources
            .get::<SettleOverlayGpu>()
            .is_none_or(|r| r.target_format != self.target_format);
        if need_rebuild {
            callback_resources.insert(SettleOverlayGpu::new(device, self.target_format));
        }
        // 内部実型 (UploadedSettleOverlay) を Arc downcast で取り出す
        let Ok(uploaded) = self.gpu.clone().downcast::<UploadedSettleOverlay>() else {
            return Vec::new();
        };
        // **target_format 不一致 guard** (Codex P2 第 3 ラウンド、2026-05):
        // `UploadedSettleOverlay.bind_group` は **構築時の SettleOverlayGpu の
        // bind_group_layout** に焼き込まれている。target_format が変化してこのフレで
        // pipeline を作り直した場合、旧 bind_group を新 pipeline と組み合わせると
        // wgpu の validation で reject される可能性があり、たとえ通っても layout 互換性が
        // 怪しい。確実に skip して、次回 `upload_settle_overlay` で新 format の
        // UploadedSettleOverlay が来るまで何も描かない (= 8K base 単独表示)。
        if uploaded.target_format != self.target_format {
            callback_resources.remove::<SettleOverlayRef>();
            return Vec::new();
        }
        // alpha を uniform に書く
        let mut bytes = [0u8; 16];
        bytes[0..4].copy_from_slice(&self.alpha.to_ne_bytes());
        queue.write_buffer(&uploaded.uniform, 0, &bytes);
        // 毎フレ ref を CallbackResources に挿入 (paint で使う)
        callback_resources.insert(SettleOverlayRef {
            source_key: self.source_key.clone(),
            cache_key: self.cache_key,
            pose: self.pose,
            alpha: self.alpha,
            gpu: uploaded,
        });
        let _ = device;
        Vec::new()
    }

    fn paint(
        &self,
        _info: egui::epaint::PaintCallbackInfo,
        render_pass: &mut wgpu::RenderPass<'static>,
        callback_resources: &egui_wgpu::CallbackResources,
    ) {
        let Some(resources) = callback_resources.get::<SettleOverlayGpu>() else {
            return;
        };
        let Some(overlay_ref) = callback_resources.get::<SettleOverlayRef>() else {
            return;
        };
        // stale guard
        if overlay_ref.source_key != self.source_key
            || overlay_ref.cache_key != self.cache_key
            || overlay_ref.pose != self.pose
        {
            return;
        }
        render_pass.set_pipeline(&resources.pipeline);
        render_pass.set_bind_group(0, &overlay_ref.gpu.bind_group, &[]);
        render_pass.draw(0..6, 0..1);
    }
}

/// `Params` uniform 用バイト列 (2 × vec4<f32> = 8 floats、32 bytes)。
///
/// レイアウト:
/// - bytes[0..16]: pose = (yaw, pitch, fov_y, aspect)
/// - bytes[16..32]: crop = (u_offset, v_offset, u_scale, v_scale)
pub fn pano_uniform_bytes(
    yaw: f32,
    pitch: f32,
    fov_y: f32,
    aspect: f32,
    uv_transform: crate::panorama::PanoUvTransform,
) -> [u8; 32] {
    let values = [
        yaw,
        pitch,
        fov_y,
        aspect,
        uv_transform.u_offset,
        uv_transform.v_offset,
        uv_transform.u_scale,
        uv_transform.v_scale,
    ];
    let mut bytes = [0u8; 32];
    for (i, v) in values.iter().enumerate() {
        bytes[i * 4..i * 4 + 4].copy_from_slice(&v.to_ne_bytes());
    }
    bytes
}
