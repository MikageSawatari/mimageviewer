//! GPU-only color blit/mips, bounded asynchronous readback, and the shared 3D shader.
use crate::app::ViewerContextId;
use crate::depth_estimation::{DepthGuide, DepthMap, DepthSourceKey, INPUT_SIZE};
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
    mpsc,
};

mod export;
#[cfg(test)]
#[path = "depth_focus_sampling_tests.rs"]
mod focus_sampling_tests;
pub(crate) use export::DepthFrameRenderer;

// No sRGB texture format is used: egui stores sRGB-encoded bytes in Rgba8Unorm.
pub(crate) const DEPTH_SHADER: &str = r#"
struct Vertex { @builtin(position) position:vec4<f32>, @location(0) uv:vec2<f32> };
@vertex fn vs_main(@builtin(vertex_index) i:u32)->Vertex {
    var p=array<vec2<f32>,3>(vec2(-1.,-1.),vec2(3.,-1.),vec2(-1.,3.));
    var o:Vertex; o.position=vec4(p[i],0.,1.); o.uv=vec2((p[i].x+1.)*.5,(1.-p[i].y)*.5); return o;
}
struct Params { motion:vec4<f32>, crop:vec4<f32>, view:vec4<f32> };
@group(0) @binding(0) var color:texture_2d<f32>;
@group(0) @binding(1) var depth:texture_2d<f32>;
@group(0) @binding(2) var samp:sampler;
@group(0) @binding(3) var<uniform> params:Params;
fn rotate_uv(p:vec2<f32>)->vec2<f32> {
    let q=u32(params.view.y+.5)%4u;
    if q==1u {return vec2(p.y,1.-p.x);}
    if q==2u {return vec2(1.-p.x,1.-p.y);}
    if q==3u {return vec2(1.-p.y,p.x);}
    return p;
}
fn source_uv(p:vec2<f32>)->vec2<f32> {
    return clamp(mix(params.crop.xy,params.crop.zw,rotate_uv(p)),params.crop.xy,params.crop.zw);
}
fn depth_at(p:vec2<f32>)->f32 {
    return pow(clamp(textureSampleLevel(depth,samp,source_uv(p),0.).r,0.,1.),params.motion.w);
}
@fragment fn fs_main(in:Vertex)->@location(0) vec4<f32> {
    let base=(in.uv-.5)/params.view.x+.5;
    let offset=params.motion.xy*vec2(1.,params.view.z);
    // Derivatives remain uniform even though individual rays terminate at different depths.
    let dx=dpdx(source_uv(base)); let dy=dpdy(source_uv(base));
    var near=1.; var far=0.; var previous=1.; var hit=false;
    // Foreground to background: first crossing wins, including overlapping surfaces.
    for(var i=0u;i<=40u;i+=1u) {
        let t=1.-f32(i)/40.;
        // Same inverse lookup as parallax.render_view: sample_pos = px - ox*(t-focus).
        let p=base-offset*(t-params.motion.z);
        if !hit && depth_at(p)>=t { near=previous; far=t; hit=true; }
        previous=t;
    }
    for(var i=0u;i<5u;i+=1u) {
        let t=(near+far)*.5;
        if depth_at(base-offset*(t-params.motion.z))>=t {far=t;} else {near=t;}
    }
    let uv=source_uv(base-offset*((near+far)*.5-params.motion.z));
    let c=textureSampleGrad(color,samp,uv,dx,dy);
    let h=params.view.w;
    let w=h*(1.-smoothstep(0.03,0.05,abs((near+far)*.5-params.motion.z)));
    return vec4(mix(c.rgb*(1.-0.2*h),vec3(0.,0.85,1.)*c.a,0.55*w),c.a);
}
"#;
const BLIT_SHADER: &str = r#"
struct Vertex { @builtin(position) position:vec4<f32>, @location(0) uv:vec2<f32> };
@vertex fn vs_main(@builtin(vertex_index) i:u32)->Vertex {
    var p=array<vec2<f32>,3>(vec2(-1.,-1.),vec2(3.,-1.),vec2(-1.,3.));
    var o:Vertex;o.position=vec4(p[i],0.,1.);o.uv=vec2((p[i].x+1.)*.5,(1.-p[i].y)*.5);return o;
}
@group(0) @binding(0) var color:texture_2d<f32>;
@group(0) @binding(1) var samp:sampler;
@fragment fn fs_main(in:Vertex)->@location(0) vec4<f32> {return textureSample(color,samp,in.uv);}
"#;

#[derive(Clone, Copy, Debug)]
pub(crate) struct DepthRenderParams {
    pub offset: egui::Vec2,
    pub focus: f32,
    pub gamma: f32,
    pub zoom: f32,
    pub uv: egui::Rect,
    pub quarter_turns: u32,
    pub aspect: f32,
    pub focus_highlight: f32,
}
impl DepthRenderParams {
    pub(crate) fn from_settings(
        settings: &crate::depth_parallax::DepthParallaxSettings,
        median: f32,
        offset: egui::Vec2,
        uv: egui::Rect,
        quarter_turns: u32,
        aspect: f32,
    ) -> Self {
        Self {
            offset,
            focus: settings.focus,
            gamma: depth_gamma(settings.shape_depth, median),
            zoom: 1.0 + 2.2 * settings.strength,
            uv,
            quarter_turns,
            aspect,
            focus_highlight: 0.0,
        }
    }
    pub(crate) fn with_focus_highlight(mut self, intensity: f32) -> Self {
        self.focus_highlight = intensity;
        self
    }
    fn bytes(self) -> [u8; 48] {
        let values = [
            self.offset.x,
            self.offset.y,
            self.focus,
            self.gamma,
            self.uv.min.x,
            self.uv.min.y,
            self.uv.max.x,
            self.uv.max.y,
            self.zoom,
            self.quarter_turns as f32,
            self.aspect,
            self.focus_highlight,
        ];
        let mut bytes = [0; 48];
        for (chunk, v) in bytes.chunks_exact_mut(4).zip(values) {
            chunk.copy_from_slice(&v.to_le_bytes());
        }
        bytes
    }
}
pub(crate) fn depth_gamma(enabled: bool, median: f32) -> f32 {
    if enabled && median.is_finite() && (0.02..=0.98).contains(&median) {
        (0.5f32.ln() / median.ln()).clamp(0.3, 1.5)
    } else {
        1.
    }
}

/// Offset-zero lookup matching DEPTH_SHADER: zoom, rotation, crop, clamp-to-edge
/// bilinear sampling at texel centers, then shaping. No previous-pass geometry.
pub(crate) fn focus_depth_at(
    pos: egui::Pos2,
    paint_rect: egui::Rect,
    params: DepthRenderParams,
    width: u32,
    height: u32,
    values: &[half::f16],
) -> Option<f32> {
    if !paint_rect.contains(pos)
        || paint_rect.width() <= 0.0
        || paint_rect.height() <= 0.0
        || width == 0
        || height == 0
        || values.len() != width as usize * height as usize
    {
        return None;
    }
    let p = ((pos - paint_rect.min) / paint_rect.size() - egui::Vec2::splat(0.5)) / params.zoom
        + egui::Vec2::splat(0.5);
    let p = match params.quarter_turns % 4 {
        1 => egui::vec2(p.y, 1.0 - p.x),
        2 => egui::vec2(1.0 - p.x, 1.0 - p.y),
        3 => egui::vec2(1.0 - p.y, p.x),
        _ => p,
    };
    let uv = (params.uv.min.to_vec2() + params.uv.size() * p)
        .clamp(params.uv.min.to_vec2(), params.uv.max.to_vec2());
    let texel = uv * egui::vec2(width as f32, height as f32) - egui::Vec2::splat(0.5);
    let lo = texel.floor();
    let fraction = texel - lo;
    let at = |x: f32, y: f32| {
        values[y.clamp(0.0, (height - 1) as f32) as usize * width as usize
            + x.clamp(0.0, (width - 1) as f32) as usize]
            .to_f32()
    };
    let top = egui::lerp(at(lo.x, lo.y)..=at(lo.x + 1.0, lo.y), fraction.x);
    let bottom = egui::lerp(
        at(lo.x, lo.y + 1.0)..=at(lo.x + 1.0, lo.y + 1.0),
        fraction.x,
    );
    Some(
        egui::lerp(top..=bottom, fraction.y)
            .clamp(0.0, 1.0)
            .powf(params.gamma),
    )
}

fn texture_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable: true },
            view_dimension: wgpu::TextureViewDimension::D2,
            multisampled: false,
        },
        count: None,
    }
}
fn sampler_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
        count: None,
    }
}
fn render_layout(device: &wgpu::Device) -> wgpu::BindGroupLayout {
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("depth render bindings"),
        entries: &[
            texture_entry(0),
            texture_entry(1),
            sampler_entry(2),
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
    })
}
fn sampler(device: &wgpu::Device) -> wgpu::Sampler {
    device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some("depth clamp trilinear"),
        mag_filter: wgpu::FilterMode::Linear,
        min_filter: wgpu::FilterMode::Linear,
        mipmap_filter: wgpu::MipmapFilterMode::Linear,
        ..Default::default()
    })
}
fn pipeline(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    shader: &str,
    format: wgpu::TextureFormat,
    blend: Option<wgpu::BlendState>,
) -> wgpu::RenderPipeline {
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("depth photo WGSL"),
        source: wgpu::ShaderSource::Wgsl(shader.into()),
    });
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("depth photo pipeline layout"),
        bind_group_layouts: &[Some(layout)],
        immediate_size: 0,
    });
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("depth photo pipeline"),
        layout: Some(&layout),
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: Some("vs_main"),
            buffers: &[],
            compilation_options: Default::default(),
        },
        fragment: Some(wgpu::FragmentState {
            module: &shader,
            entry_point: Some("fs_main"),
            targets: &[Some(wgpu::ColorTargetState {
                format,
                blend,
                write_mask: wgpu::ColorWrites::ALL,
            })],
            compilation_options: Default::default(),
        }),
        primitive: Default::default(),
        depth_stencil: None,
        multisample: Default::default(),
        multiview_mask: None,
        cache: None,
    })
}
fn texture(
    device: &wgpu::Device,
    width: u32,
    height: u32,
    format: wgpu::TextureFormat,
    mips: u32,
    usage: wgpu::TextureUsages,
) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some("depth photo texture"),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: mips,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage,
        view_formats: &[],
    })
}
pub(crate) fn color_byte_size(width: u32, height: u32) -> u64 {
    let mut size = 0;
    for level in 0..egui_wgpu::mip_level_count(width, height) {
        size += u64::from((width >> level).max(1)) * u64::from((height >> level).max(1)) * 4;
    }
    size
}
pub(crate) fn guide_size(width: u32, height: u32) -> [u32; 2] {
    let longest = width.max(height);
    if longest <= 1536 {
        [width, height]
    } else {
        [
            (u64::from(width) * 1536 / u64::from(longest)).max(1) as u32,
            (u64::from(height) * 1536 / u64::from(longest)).max(1) as u32,
        ]
    }
}
pub(crate) struct DepthColor {
    pub texture: wgpu::Texture,
    pub view: wgpu::TextureView,
    pub width: u32,
    pub height: u32,
}
impl DepthColor {
    pub(crate) fn byte_size(&self) -> u64 {
        color_byte_size(self.width, self.height)
    }
}
pub(crate) struct DepthTexture {
    pub texture: wgpu::Texture,
    pub view: wgpu::TextureView,
    pub median: f32,
    pub width: u32,
    pub height: u32,
    pub values: Vec<half::f16>,
}
impl DepthTexture {
    pub(crate) fn upload(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        map: DepthMap,
    ) -> Result<Self, String> {
        if map.width == 0
            || map.height == 0
            || map.width.max(map.height) > 1536
            || map.values.len() != map.width as usize * map.height as usize
        {
            return Err("深度画像の寸法が不正です".into());
        }
        let texture = texture(
            device,
            map.width,
            map.height,
            wgpu::TextureFormat::R16Float,
            1,
            wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        );
        // f16 bits are copied without a second full-size temporary conversion.
        let bytes = unsafe {
            std::slice::from_raw_parts(map.values.as_ptr().cast::<u8>(), map.values.len() * 2)
        };
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            bytes,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(map.width * 2),
                rows_per_image: Some(map.height),
            },
            texture.size(),
        );
        let view = texture.create_view(&Default::default());
        Ok(Self {
            texture,
            view,
            median: map.median,
            width: map.width,
            height: map.height,
            values: map.values,
        })
    }
    pub(crate) fn byte_size(&self) -> u64 {
        u64::from(self.width) * u64::from(self.height) * 2
    }
}

pub(crate) struct DepthGpu {
    layout: wgpu::BindGroupLayout,
    pipeline: wgpu::RenderPipeline,
    sampler: wgpu::Sampler,
    mips: egui_wgpu::MipmapGenerator,
}
impl DepthGpu {
    pub(crate) fn new(device: &wgpu::Device) -> Self {
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("depth blit bindings"),
            entries: &[texture_entry(0), sampler_entry(1)],
        });
        let pipeline = pipeline(
            device,
            &layout,
            BLIT_SHADER,
            wgpu::TextureFormat::Rgba8Unorm,
            None,
        );
        Self {
            layout,
            pipeline,
            sampler: sampler(device),
            mips: egui_wgpu::MipmapGenerator::new(device),
        }
    }
    fn blit(
        &self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        source: &wgpu::TextureView,
        target: &wgpu::TextureView,
    ) {
        let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("depth blit"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(source),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
            ],
        });
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("depth color blit"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                    store: wgpu::StoreOp::Store,
                },
            })],
            ..Default::default()
        });
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &bind, &[]);
        pass.draw(0..3, 0..1);
    }
    pub(crate) fn copy_color(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        source: &wgpu::Texture,
    ) -> Result<DepthColor, String> {
        if source.format() != wgpu::TextureFormat::Rgba8Unorm
            || source.width() == 0
            || source.height() == 0
        {
            return Err("3D 写真の元テクスチャ形式が不正です".into());
        }
        let (width, height) = (source.width(), source.height());
        let texture = texture(
            device,
            width,
            height,
            wgpu::TextureFormat::Rgba8Unorm,
            egui_wgpu::mip_level_count(width, height),
            wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::RENDER_ATTACHMENT,
        );
        let source = source.create_view(&wgpu::TextureViewDescriptor {
            base_mip_level: 0,
            mip_level_count: Some(1),
            ..Default::default()
        });
        let target = texture.create_view(&wgpu::TextureViewDescriptor {
            base_mip_level: 0,
            mip_level_count: Some(1),
            ..Default::default()
        });
        let mut encoder = device.create_command_encoder(&Default::default());
        self.blit(device, &mut encoder, &source, &target);
        queue.submit(Some(encoder.finish()));
        self.mips.generate(device, queue, &texture);
        let view = texture.create_view(&Default::default());
        Ok(DepthColor {
            texture,
            view,
            width,
            height,
        })
    }
    pub(crate) fn readback(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        color: &DepthColor,
    ) -> DepthReadback {
        let mut encoder = device.create_command_encoder(&Default::default());
        let input = ReadbackImage::encode(
            device,
            &mut encoder,
            self,
            &color.view,
            [INPUT_SIZE, INPUT_SIZE],
        );
        let guide = ReadbackImage::encode(
            device,
            &mut encoder,
            self,
            &color.view,
            guide_size(color.width, color.height),
        );
        queue.submit(Some(encoder.finish()));
        DepthReadback {
            input: input.begin(),
            guide: guide.begin(),
        }
    }
}
enum MapStatus {
    Pending(mpsc::Receiver<Result<(), wgpu::BufferAsyncError>>),
    Ready,
    Failed(String),
}
struct ReadbackImage {
    texture: wgpu::Texture,
    buffer: wgpu::Buffer,
    size: [u32; 2],
    stride: u32,
    status: MapStatus,
}
impl ReadbackImage {
    fn encode(
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        gpu: &DepthGpu,
        source: &wgpu::TextureView,
        size: [u32; 2],
    ) -> Self {
        let texture = texture(
            device,
            size[0],
            size[1],
            wgpu::TextureFormat::Rgba8Unorm,
            1,
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        );
        let view = texture.create_view(&Default::default());
        gpu.blit(device, encoder, source, &view);
        let stride = (size[0] * 4).div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT)
            * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("depth bounded readback"),
            size: u64::from(stride) * u64::from(size[1]),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(stride),
                    rows_per_image: Some(size[1]),
                },
            },
            texture.size(),
        );
        Self {
            texture,
            buffer,
            size,
            stride,
            status: MapStatus::Ready,
        }
    }
    fn begin(mut self) -> Self {
        let (tx, rx) = mpsc::channel();
        self.buffer
            .slice(..)
            .map_async(wgpu::MapMode::Read, move |result| {
                let _ = tx.send(result);
            });
        self.status = MapStatus::Pending(rx);
        self
    }
    fn poll(&mut self) -> Option<Result<(), String>> {
        if let MapStatus::Pending(rx) = &self.status {
            match rx.try_recv() {
                Ok(Ok(())) => self.status = MapStatus::Ready,
                Ok(Err(error)) => {
                    self.status =
                        MapStatus::Failed(format!("GPU の読み戻しに失敗しました: {error}"))
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.status = MapStatus::Failed("GPU の読み戻しが中断されました".into())
                }
                Err(mpsc::TryRecvError::Empty) => return None,
            }
        }
        match &self.status {
            MapStatus::Ready => Some(Ok(())),
            MapStatus::Failed(reason) => Some(Err(reason.clone())),
            MapStatus::Pending(_) => None,
        }
    }
    fn pixels(&self) -> Result<Vec<u8>, String> {
        let mapped = self
            .buffer
            .slice(..)
            .get_mapped_range()
            .map_err(|error| format!("GPU の画素を取得できません: {error}"))?;
        let mut pixels = Vec::with_capacity(self.size[0] as usize * self.size[1] as usize * 4);
        for row in mapped.chunks_exact(self.stride as usize) {
            pixels.extend_from_slice(&row[..self.size[0] as usize * 4]);
        }
        drop(mapped);
        self.buffer.unmap();
        Ok(pixels)
    }
    fn byte_size(&self) -> u64 {
        u64::from(self.size[0]) * u64::from(self.size[1]) * 4 + self.buffer.size()
    }
}
pub(crate) struct DepthReadback {
    input: ReadbackImage,
    guide: ReadbackImage,
}
impl DepthReadback {
    pub(crate) fn try_finish(&mut self) -> Option<Result<(Vec<u8>, DepthGuide), String>> {
        let input = self.input.poll();
        let guide = self.guide.poll();
        if let Some(Err(error)) = &input {
            return Some(Err(error.clone()));
        }
        if let Some(Err(error)) = &guide {
            return Some(Err(error.clone()));
        }
        if input.is_none() || guide.is_none() {
            return None;
        }
        Some(self.input.pixels().and_then(|input| {
            self.guide.pixels().map(|rgba| {
                (
                    input,
                    DepthGuide {
                        width: self.guide.size[0],
                        height: self.guide.size[1],
                        rgba,
                    },
                )
            })
        }))
    }
    pub(crate) fn byte_size(&self) -> u64 {
        self.input.byte_size() + self.guide.byte_size()
    }
}

pub(crate) struct DepthRenderResources {
    pub context: ViewerContextId,
    pub key: DepthSourceKey,
    pub resource_id: u64,
    pub color: Arc<DepthColor>,
    pub depth: Arc<DepthTexture>,
    pub uniform: wgpu::Buffer,
    pub bind_group: wgpu::BindGroup,
}
impl DepthRenderResources {
    pub(crate) fn new(
        device: &wgpu::Device,
        context: ViewerContextId,
        key: DepthSourceKey,
        color: Arc<DepthColor>,
        depth: Arc<DepthTexture>,
    ) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let layout = render_layout(device);
        let sampler = sampler(device);
        let uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("depth photo uniform"),
            size: 48,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("depth render resources"),
            layout: &layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&color.view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&depth.view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: uniform.as_entire_binding(),
                },
            ],
        });
        Self {
            context,
            key,
            resource_id: NEXT.fetch_add(1, Ordering::Relaxed),
            color,
            depth,
            uniform,
            bind_group,
        }
    }
}
pub(crate) struct DepthRenderResourcesRef(pub Arc<DepthRenderResources>);
pub(crate) fn clear_depth_slot(
    resources: &mut egui_wgpu::CallbackResources,
    context: ViewerContextId,
) {
    if resources
        .get::<DepthRenderResourcesRef>()
        .is_some_and(|slot| slot.0.context == context)
    {
        resources.remove::<DepthRenderResourcesRef>();
    }
}
struct DepthPipeline {
    format: wgpu::TextureFormat,
    pipeline: wgpu::RenderPipeline,
}
#[derive(Clone, Debug)]
pub(crate) struct DepthShaderCallback {
    pub context: ViewerContextId,
    pub key: DepthSourceKey,
    pub resource_id: u64,
    pub params: DepthRenderParams,
    pub target_format: wgpu::TextureFormat,
}
impl DepthShaderCallback {
    fn matches(&self, r: &DepthRenderResources) -> bool {
        r.context == self.context && r.key == self.key && r.resource_id == self.resource_id
    }
}
impl egui_wgpu::CallbackTrait for DepthShaderCallback {
    fn prepare(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        _screen: &egui_wgpu::ScreenDescriptor,
        _encoder: &mut wgpu::CommandEncoder,
        resources: &mut egui_wgpu::CallbackResources,
    ) -> Vec<wgpu::CommandBuffer> {
        if resources
            .get::<DepthPipeline>()
            .is_none_or(|p| p.format != self.target_format)
        {
            let layout = render_layout(device);
            resources.insert(DepthPipeline {
                format: self.target_format,
                pipeline: pipeline(
                    device,
                    &layout,
                    DEPTH_SHADER,
                    self.target_format,
                    Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                ),
            });
        }
        if let Some(slot) = resources.get::<DepthRenderResourcesRef>()
            && self.matches(&slot.0)
        {
            queue.write_buffer(&slot.0.uniform, 0, &self.params.bytes());
        }
        Vec::new()
    }
    fn paint(
        &self,
        _info: egui::epaint::PaintCallbackInfo,
        pass: &mut wgpu::RenderPass<'static>,
        resources: &egui_wgpu::CallbackResources,
    ) {
        let Some(pipeline) = resources.get::<DepthPipeline>() else {
            return;
        };
        let Some(slot) = resources.get::<DepthRenderResourcesRef>() else {
            return;
        };
        if pipeline.format != self.target_format || !self.matches(&slot.0) {
            return;
        }
        pass.set_pipeline(&pipeline.pipeline);
        pass.set_bind_group(0, &slot.0.bind_group, &[]);
        pass.draw(0..3, 0..1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn depth_gamma_bounds_and_median() {
        assert_eq!(depth_gamma(false, 0.2), 1.);
        assert_eq!(depth_gamma(true, f32::NAN), 1.);
        for m in [0.02, 0.1, 0.2, 0.5, 0.8, 0.98] {
            let g = depth_gamma(true, m);
            assert!((0.3..=1.5).contains(&g));
        }
        assert!((0.2f32.powf(depth_gamma(true, 0.2)) - 0.5).abs() < 1e-6);
    }
    #[test]
    fn bounded_guide_and_exact_mip_accounting() {
        assert_eq!(guide_size(900, 700), [900, 700]);
        assert_eq!(guide_size(8192, 4096), [1536, 768]);
        assert_eq!(guide_size(1, 99999), [1, 1536]);
        assert_eq!(color_byte_size(4, 2), 44);
    }

    pub(super) fn render_egui_frame(
        gpu: &mut crate::egui037_stage4_gpu_tests::OffscreenGpu,
        ctx: &egui::Context,
        size: [u32; 2],
        paint: impl FnOnce(&egui::Painter),
    ) -> Vec<[u8; 4]> {
        let rect =
            egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(size[0] as f32, size[1] as f32));
        ctx.begin_pass(egui::RawInput {
            screen_rect: Some(rect),
            ..Default::default()
        });
        paint(
            &ctx.layer_painter(egui::LayerId::background())
                .with_clip_rect(rect),
        );
        let mut output = ctx.end_pass();
        for (id, deltas) in output.textures_delta.set.drain() {
            for delta in deltas {
                gpu.renderer
                    .update_texture(&gpu.device, &gpu.queue, id, &delta);
            }
        }
        let jobs = ctx.tessellate(std::mem::take(&mut output.shapes), output.pixels_per_point);
        let pixels = gpu.render(&jobs, size, [13, 23, 37, 255]);
        for id in output.textures_delta.free.drain() {
            gpu.renderer.free_texture(&id);
        }
        pixels
    }

    pub(super) fn cropped_quarter_turn(
        viewport: egui::Rect,
        crop: egui::Rect,
    ) -> crate::displayed_image_transform::DisplayedImageTransform {
        use crate::displayed_image_transform::{
            DisplayedImageTransform, DisplayedImageTransformInput, FullscreenFitScaleLimits,
            RectPixelFit, ResolvedDisplayPlacement,
        };
        DisplayedImageTransform::from_resolved_rect(
            DisplayedImageTransformInput {
                page_idx: 0,
                viewport_rect: viewport,
                source_size: egui::vec2(64., 48.),
                texture_size: egui::vec2(64., 48.),
                rotation: crate::rotation_db::Rotation::Cw90,
                free_rotation_rad: 0.,
                content_bbox: Some(crop),
                fit_mode: crate::fullscreen_specified_scale::FullscreenFitInput::legacy(
                    crate::settings::FullscreenFitMode::Page,
                ),
                fit_scale_limits: FullscreenFitScaleLimits::default(),
                pixels_per_point: 1.,
                placement: ResolvedDisplayPlacement::Normal { zoom_pan: None },
                pixel_fit: RectPixelFit::Texels,
            },
            egui::Rect::from_center_size(viewport.center(), egui::vec2(48., 64.)),
        )
        .unwrap()
    }

    pub(super) fn install_two_depth_planes(
        gpu: &mut crate::egui037_stage4_gpu_tests::OffscreenGpu,
        texture_id: egui::TextureId,
        transform: &crate::displayed_image_transform::DisplayedImageTransform,
    ) -> DepthShaderCallback {
        let key = DepthSourceKey { texture_id };
        let source = gpu
            .renderer
            .texture(&texture_id)
            .unwrap()
            .texture
            .as_ref()
            .unwrap();
        let copier = DepthGpu::new(&gpu.device);
        let color = Arc::new(copier.copy_color(&gpu.device, &gpu.queue, source).unwrap());
        let depth = Arc::new(
            DepthTexture::upload(
                &gpu.device,
                &gpu.queue,
                DepthMap {
                    key,
                    width: 64,
                    height: 48,
                    // The boundary is intentionally off-center, so dropping the crop
                    // from depth sampling changes which plane owns test pixels.
                    values: (0..48)
                        .flat_map(|_| {
                            (0..64).map(|x| half::f16::from_f32(if x < 36 { 0.25 } else { 0.75 }))
                        })
                        .collect(),
                    median: 0.25,
                },
            )
            .unwrap(),
        );
        let resources = Arc::new(DepthRenderResources::new(
            &gpu.device,
            ViewerContextId::for_test(20),
            key,
            color,
            depth,
        ));
        let job = DepthShaderCallback {
            context: resources.context,
            key,
            resource_id: resources.resource_id,
            params: DepthRenderParams {
                offset: egui::Vec2::ZERO,
                focus: 0.5,
                gamma: 1.,
                zoom: 1.,
                uv: transform.uv_rect,
                quarter_turns: transform.rotation.degrees() as u32 / 90,
                aspect: transform.paint_rect.aspect_ratio(),
                focus_highlight: 0.0,
            },
            target_format: crate::egui037_stage4_gpu_tests::FORMAT,
        };
        gpu.renderer
            .callback_resources
            .insert(DepthRenderResourcesRef(resources));
        job
    }

    #[test]
    fn depth_gpu_nonuniform_planes_rotate_and_crop_with_color() {
        use crate::egui037_stage4_gpu_tests::{OffscreenGpu, assert_rgba, callback};
        let mut gpu = OffscreenGpu::new();
        let ctx = egui::Context::default();
        let rgba: Vec<_> = (0..48)
            .flat_map(|y| (0..64).map(move |x| egui::Color32::from_rgb(x * 2, y * 3, 80)))
            .collect();
        let source = ctx.load_texture(
            "depth rotated color ramp",
            egui::ColorImage::new([64, 48], rgba),
            egui::TextureOptions::LINEAR,
        );
        let size = [24, 32];
        let viewport = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(24., 32.));
        let transform = cropped_quarter_turn(
            viewport,
            egui::Rect::from_min_max(egui::pos2(0.25, 0.25), egui::pos2(0.75, 0.75)),
        );
        assert_eq!(transform.paint_rect, viewport);
        let two_d = render_egui_frame(&mut gpu, &ctx, size, |painter| {
            transform.paint_texture(painter, source.id(), egui::Color32::WHITE);
        });
        let mut job = install_two_depth_planes(&mut gpu, source.id(), &transform);
        let still = gpu.render(&[callback(viewport, job.clone())], size, [13, 23, 37, 255]);
        for (actual, expected) in still.iter().zip(&two_d) {
            assert_rgba(*actual, *expected);
        }
        // A native-sized clockwise crop maps screen (x,y) to source
        // (16+y, 35-x). This is a fixed pixel oracle, not a second ray marcher.
        for (x, y) in [(3, 4), (15, 18), (8, 26)] {
            assert_rgba(
                still[y * 24 + x],
                [(16 + y) as u8 * 2, (35 - x) as u8 * 3, 80, 255],
            );
        }
        // .25/.75 planes straddle focus .5. At 24px wide, offset 1/6
        // translates them by exactly one screen pixel in opposite directions.
        // After rotation the depth boundary lies at y=20, not at screen x=36.
        job.params.offset = egui::vec2(1. / 6., 0.);
        let moving = gpu.render(&[callback(viewport, job.clone())], size, [13, 23, 37, 255]);
        for y in [3, 10, 18, 23, 28] {
            for x in 2..22 {
                let sample_x = if y < 20 { x + 1 } else { x - 1 };
                assert_rgba(moving[y * 24 + x], two_d[y * 24 + sample_x]);
                assert!(
                    moving[y * 24 + x][1].abs_diff(still[y * 24 + x][1]) >= 2,
                    "both distinct depth planes must move at ({x}, {y})"
                );
            }
        }
        // Force the two planes beyond opposite crop edges. Clamping to the
        // full texture (instead of the cropped UV range) exposes different ramp
        // pixels. The half-texel edge samples are green 34.5 and 106.5.
        job.params.offset = egui::vec2(1., 0.);
        let clamped = gpu.render(&[callback(viewport, job)], size, [13, 23, 37, 255]);
        assert_rgba(clamped[3 * 24 + 23], [38, 35, 80, 255]);
        assert_rgba(clamped[28 * 24], [88, 107, 80, 255]);
    }

    #[test]
    fn depth_gpu_transparency_matches_2d_changing_underlays_without_rebuilding_resources() {
        use crate::egui037_stage4_gpu_tests::{OffscreenGpu, assert_rgba};
        use crate::ui_fullscreen::{paint_image_underlay, transparent_bg_style};
        let mut gpu = OffscreenGpu::new();
        let ctx = egui::Context::default();
        let pixels = (0..48)
            .flat_map(|_| {
                (0..64).map(|x| match x {
                    0..24 => egui::Color32::TRANSPARENT,
                    24..40 => egui::Color32::from_rgba_premultiplied(64, 32, 16, 128),
                    _ => egui::Color32::from_gray(128),
                })
            })
            .collect();
        let source = ctx.load_texture(
            "depth alpha patches",
            egui::ColorImage::new([64, 48], pixels),
            egui::TextureOptions::LINEAR,
        );
        // The normal 2D underlay painter owns repeat UVs and the 16px tile size.
        let checker = ctx.load_texture(
            "depth checker fixture",
            egui::ColorImage::new(
                [16, 16],
                (0..16)
                    .flat_map(|y| {
                        (0..16).map(move |x| {
                            egui::Color32::from_gray(if (x / 8 + y / 8) % 2 == 0 {
                                224
                            } else {
                                176
                            })
                        })
                    })
                    .collect(),
            ),
            egui::TextureOptions {
                wrap_mode: egui::TextureWrapMode::Repeat,
                ..egui::TextureOptions::NEAREST
            },
        );
        let size = [80, 64];
        let viewport = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(80., 64.));
        let transform = cropped_quarter_turn(
            viewport,
            egui::Rect::from_min_max(egui::pos2(0.125, 0.25), egui::pos2(0.875, 0.75)),
        );
        // Upload the managed source before creating the real depth resources.
        render_egui_frame(&mut gpu, &ctx, size, |_| {});
        let job = install_two_depth_planes(&mut gpu, source.id(), &transform);
        // Reuse the same source, depth, callback and uniforms across background changes.
        for mode in [0, 1, 2, 0] {
            let underlay = transparent_bg_style(mode, Some(&checker));
            let two_d = render_egui_frame(&mut gpu, &ctx, size, |painter| {
                paint_image_underlay(painter, transform.paint_quad(), &underlay);
                transform.paint_texture(painter, source.id(), egui::Color32::WHITE);
            });
            let three_d = render_egui_frame(&mut gpu, &ctx, size, |painter| {
                paint_image_underlay(painter, transform.paint_quad(), &underlay);
                painter.add(egui::Shape::Callback(
                    egui_wgpu::Callback::new_paint_callback(transform.paint_rect, job.clone()),
                ));
            });
            for (actual, expected) in three_d.iter().zip(&two_d) {
                assert_rgba(*actual, *expected);
            }
            // Explicit oracles prevent the two paths from agreeing on an
            // absent underlay, wrong blend/alpha, or accidental sRGB transform.
            assert_rgba(three_d[16 * 80 + 10], [13, 23, 37, 255]);
            assert_rgba(three_d[48 * 80 + 32], [128, 128, 128, 255]);
            if mode == 0 {
                assert_rgba(three_d[16 * 80 + 32], [0, 0, 0, 255]);
                assert_rgba(three_d[32 * 80 + 32], [64, 32, 16, 255]);
            } else if mode == 1 {
                assert_rgba(three_d[16 * 80 + 32], [255, 255, 255, 255]);
                assert_rgba(three_d[32 * 80 + 32], [191, 159, 143, 255]);
            } else {
                assert_rgba(three_d[16 * 80 + 32], [176, 176, 176, 255]);
                assert_rgba(three_d[16 * 80 + 40], [224, 224, 224, 255]);
                assert_rgba(three_d[32 * 80 + 32], [152, 120, 104, 255]);
                assert_rgba(three_d[32 * 80 + 40], [176, 144, 128, 255]);
            }
        }
    }

    /// Actual product WGSL/blit/readback on a windowless device. No model,
    /// filesystem, application profile, surface, or desktop interaction.
    #[test]
    fn depth_export_gpu_matches_screen_gamma_alpha_and_rotated_checker() {
        use crate::depth_export::DepthExportUnderlay;
        use crate::egui037_stage4_gpu_tests::{OffscreenGpu, assert_rgba};
        use crate::ui_fullscreen::{paint_image_underlay, transparent_bg_style};
        let mut gpu = OffscreenGpu::new();
        let ctx = egui::Context::default();
        let pixels = (0..48)
            .flat_map(|_| {
                (0..64).map(|x| match x {
                    0..24 => egui::Color32::TRANSPARENT,
                    24..40 => egui::Color32::from_rgba_premultiplied(64, 32, 16, 128),
                    _ => egui::Color32::from_gray(128),
                })
            })
            .collect();
        let source = ctx.load_texture(
            "depth export alpha",
            egui::ColorImage::new([64, 48], pixels),
            egui::TextureOptions::LINEAR,
        );
        let checker = ctx.load_texture(
            "depth export checker",
            egui::ColorImage::new(
                [16, 16],
                (0..16)
                    .flat_map(|y| {
                        (0..16).map(move |x| {
                            egui::Color32::from_gray(if (x / 8 + y / 8) % 2 == 0 {
                                224
                            } else {
                                176
                            })
                        })
                    })
                    .collect(),
            ),
            egui::TextureOptions {
                wrap_mode: egui::TextureWrapMode::Repeat,
                ..egui::TextureOptions::NEAREST
            },
        );
        let size = [80, 64];
        let transform = cropped_quarter_turn(
            egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(80., 64.)),
            egui::Rect::from_min_max(egui::pos2(0.125, 0.25), egui::pos2(0.875, 0.75)),
        );
        render_egui_frame(&mut gpu, &ctx, size, |_| {});
        let job = install_two_depth_planes(&mut gpu, source.id(), &transform);
        let export_size = [
            transform.paint_rect.width() as u32,
            transform.paint_rect.height() as u32,
        ];
        let origin = [
            transform.paint_rect.left() as usize,
            transform.paint_rect.top() as usize,
        ];
        let resource = gpu
            .renderer
            .callback_resources
            .get::<DepthRenderResourcesRef>()
            .unwrap()
            .0
            .clone();
        for mode in [0, 1, 2] {
            let screen_bg = transparent_bg_style(mode, Some(&checker));
            let screen = render_egui_frame(&mut gpu, &ctx, size, |painter| {
                paint_image_underlay(painter, transform.paint_quad(), &screen_bg);
                painter.add(egui::Shape::Callback(
                    egui_wgpu::Callback::new_paint_callback(transform.paint_rect, job.clone()),
                ));
            });
            let export_bg = if mode == 2 {
                DepthExportUnderlay::Checker {
                    cell_size: [8., 8.],
                }
            } else {
                DepthExportUnderlay::Solid([if mode == 1 { 255 } else { 0 }; 3])
            };
            let exporter = DepthFrameRenderer::new(
                gpu.device.clone(),
                gpu.queue.clone(),
                &resource,
                export_size,
                export_bg,
            );
            // Deliberately overwrite the screen uniform after construction. An export
            // using the callback's buffer would now use the wrong crop/motion.
            gpu.queue.write_buffer(
                &resource.uniform,
                0,
                &DepthRenderParams {
                    offset: egui::vec2(0.5, 0.5),
                    ..job.params
                }
                .bytes(),
            );
            let exported = exporter.render(job.params).unwrap();
            let [width, height] = export_size.map(|n| n as usize);
            for y in 0..height {
                for x in 0..width {
                    let actual: [u8; 4] = exported[(y * width + x) * 4..(y * width + x + 1) * 4]
                        .try_into()
                        .unwrap();
                    assert_eq!(actual[3], 255);
                    assert_rgba(actual, screen[(y + origin[1]) * 80 + x + origin[0]]);
                }
            }
            assert_eq!(
                exported[((height - 4) * width + width / 2) * 4
                    ..((height - 4) * width + width / 2 + 1) * 4],
                [128, 128, 128, 255]
            );
        }
    }

    #[test]
    fn depth_gpu_blit_shader_stale_guard_and_lru() {
        use crate::depth_parallax::{DepthSourcePhase, DepthViewState};
        use crate::egui037_stage4_gpu_tests::{FORMAT, OffscreenGpu, callback};
        let mut gpu = OffscreenGpu::new();
        let size = [16, 12];
        let rgba: Vec<u8> = (0..size[1])
            .flat_map(|y| {
                (0..size[0]).flat_map(move |x| {
                    // Premultiplied sRGB-encoded bytes, including half transparent gray.
                    [32 + x as u8 * 3, 24 + y as u8 * 4, 64, 128]
                })
            })
            .collect();
        // Deliberately lacks COPY_SRC: this is the managed-egui source contract.
        let source = texture(
            &gpu.device,
            size[0],
            size[1],
            FORMAT,
            1,
            wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        );
        gpu.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &source,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &rgba,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(size[0] * 4),
                rows_per_image: Some(size[1]),
            },
            source.size(),
        );
        let copier = DepthGpu::new(&gpu.device);
        let color = Arc::new(copier.copy_color(&gpu.device, &gpu.queue, &source).unwrap());
        let mut readback = copier.readback(&gpu.device, &gpu.queue, &color);
        // Wait belongs only to this headless test; the production API uses Poll.
        gpu.device
            .poll(wgpu::PollType::Wait {
                submission_index: None,
                timeout: Some(std::time::Duration::from_secs(30)),
            })
            .unwrap();
        let (input, guide) = readback
            .try_finish()
            .expect("mapped without pointer motion")
            .unwrap();
        assert_eq!(input.len(), 504 * 504 * 4);
        assert_eq!([guide.width, guide.height], size);
        assert_eq!(
            guide.rgba, rgba,
            "blit/readback must preserve encoded gray and alpha"
        );
        let key = DepthSourceKey {
            texture_id: egui::TextureId::Managed(101),
        };
        let owner = ViewerContextId::for_test(10);
        let sibling = ViewerContextId::for_test(11);
        let map = |key, value| DepthMap {
            key,
            width: size[0],
            height: size[1],
            values: vec![half::f16::from_f32(value); (size[0] * size[1]) as usize],
            median: value,
        };
        let depth = Arc::new(DepthTexture::upload(&gpu.device, &gpu.queue, map(key, 0.5)).unwrap());
        let rendered = Arc::new(DepthRenderResources::new(
            &gpu.device,
            owner,
            key,
            color.clone(),
            depth,
        ));
        gpu.renderer
            .callback_resources
            .insert(DepthRenderResourcesRef(rendered.clone()));
        let params = DepthRenderParams {
            offset: egui::Vec2::ZERO,
            focus: 0.5,
            gamma: 1.,
            zoom: 1.,
            uv: egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1., 1.)),
            quarter_turns: 0,
            aspect: size[0] as f32 / size[1] as f32,
            focus_highlight: 0.0,
        };
        let job = DepthShaderCallback {
            context: owner,
            key,
            resource_id: rendered.resource_id,
            params,
            target_format: FORMAT,
        };
        let rect =
            egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(size[0] as f32, size[1] as f32));
        let pixels = gpu.render(&[callback(rect, job.clone())], size, [0, 0, 0, 0]);
        for (actual, expected) in pixels.iter().zip(rgba.chunks_exact(4)) {
            for (a, b) in actual.iter().zip(expected) {
                assert!(
                    a.abs_diff(*b) <= 1,
                    "zero offset {actual:?} vs {expected:?}"
                );
            }
        }
        let mut flat = job.clone();
        flat.params.offset = egui::vec2(0.05, -0.03);
        let shifted = gpu.render(&[callback(rect, flat)], size, [0, 0, 0, 0]);
        for (actual, expected) in shifted.iter().zip(&pixels) {
            for (a, b) in actual.iter().zip(expected) {
                assert!(a.abs_diff(*b) <= 1, "flat focus depth must remain fixed");
            }
        }
        // Spike reference: sample_x = x - offset_x * width * (depth - focus).
        // .125 * 16 * (.5 - 0) = 1: positive offset samples one pixel left.
        let mut translated = job.clone();
        translated.params.focus = 0.;
        translated.params.offset = egui::vec2(0.125, 0.);
        let shifted = gpu.render(&[callback(rect, translated)], size, [0, 0, 0, 0]);
        for y in 0..size[1] as usize {
            for x in 0..size[0] as usize {
                let expected = pixels[y * size[0] as usize + x.saturating_sub(1)];
                for (a, b) in shifted[y * size[0] as usize + x].iter().zip(expected) {
                    assert!(a.abs_diff(b) <= 1, "uniform plane translation");
                }
            }
        }
        // Pointer-side peek deliberately supplies the opposite camera offset.
        // With the reference inverse lookup, a right-side pointer samples to
        // the right and thus moves the foreground image left, away from it.
        let mut mouse = crate::depth_parallax::MouseFollow::default();
        mouse.update(
            Some(egui::pos2(rect.right(), rect.center().y)),
            rect,
            rect,
            false,
            0.05,
            1.,
        );
        assert!(mouse.current.x < 0.);
        let mut peek = job.clone();
        peek.params.focus = 0.;
        peek.params.offset = mouse.current;
        let peeked = gpu.render(&[callback(rect, peek)], size, [0, 0, 0, 0]);
        for y in 0..size[1] as usize {
            for x in 1..size[0] as usize - 1 {
                let index = y * size[0] as usize + x;
                assert!(
                    peeked[index][0] > pixels[index][0],
                    "right pointer must sample the red ramp toward the right"
                );
                assert_eq!(peeked[index][1], pixels[index][1]);
            }
        }
        let mut stale = job.clone();
        stale.context = sibling;
        assert!(
            gpu.render(&[callback(rect, stale)], size, [3, 5, 7, 255])
                .iter()
                .all(|p| *p == [3, 5, 7, 255])
        );
        let mut stale = job.clone();
        stale.resource_id += 1;
        assert!(
            gpu.render(&[callback(rect, stale)], size, [3, 5, 7, 255])
                .iter()
                .all(|p| *p == [3, 5, 7, 255])
        );
        clear_depth_slot(&mut gpu.renderer.callback_resources, sibling);
        assert!(
            gpu.renderer
                .callback_resources
                .get::<DepthRenderResourcesRef>()
                .is_some()
        );
        clear_depth_slot(&mut gpu.renderer.callback_resources, owner);
        assert!(
            gpu.renderer
                .callback_resources
                .get::<DepthRenderResourcesRef>()
                .is_none()
        );
        assert!(
            gpu.render(&[callback(rect, job)], size, [3, 5, 7, 255])
                .iter()
                .all(|p| *p == [3, 5, 7, 255])
        );
        let mut state = DepthViewState::new(owner);
        for index in 0..5 {
            let key = DepthSourceKey {
                texture_id: egui::TextureId::Managed(index),
            };
            state.lru.insert(
                key,
                Arc::new(DepthTexture::upload(&gpu.device, &gpu.queue, map(key, 0.5)).unwrap()),
            );
        }
        assert!(
            state
                .lru
                .get(DepthSourceKey {
                    texture_id: egui::TextureId::Managed(0)
                })
                .is_none()
        );
        assert_eq!(
            state.lru.resident_bytes(),
            u64::from(size[0] * size[1]) * 2 * 4
        );
        let cached = DepthSourceKey {
            texture_id: egui::TextureId::Managed(4),
        };
        state
            .select_source(&gpu.device, &gpu.queue, &source, cached)
            .unwrap();
        assert!(matches!(state.source,DepthSourcePhase::Ready{key} if key==cached));
        assert_eq!(
            state.resident_bytes(),
            color_byte_size(size[0], size[1]) + u64::from(size[0] * size[1]) * 2 * 4
        );
        // Drive the real state machine with stationary input. Only inference is
        // replaced: the GPU blit/maps, Poll, repaint and R16 adoption are real.
        let egui = egui::Context::default();
        let repaint_count = Arc::new(AtomicU64::new(0));
        let observed = repaint_count.clone();
        egui.set_repaint_observer(move |info| {
            assert_eq!(info.viewport_id, egui::ViewportId::ROOT);
            observed.fetch_add(1, Ordering::Relaxed);
        });
        let mut progressing = DepthViewState::new(owner);
        let first_key = DepthSourceKey {
            texture_id: egui::TextureId::Managed(201),
        };
        progressing
            .select_source(&gpu.device, &gpu.queue, &source, first_key)
            .unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        while !matches!(progressing.source, DepthSourcePhase::Ready { .. }) {
            assert!(
                std::time::Instant::now() < deadline,
                "stationary readback must progress to Ready"
            );
            progressing.advance(
                &gpu.device,
                &gpu.queue,
                &egui,
                egui::ViewportId::ROOT,
                |key, input, guide| {
                    assert_eq!(key, first_key);
                    assert_eq!(input.len(), 504 * 504 * 4);
                    assert_eq!([guide.width, guide.height], size);
                    assert_eq!(guide.rgba, rgba);
                    Ok(crate::depth_estimation::DepthPending::completed_for_test(
                        owner,
                        501,
                        map(key, 0.5),
                    ))
                },
            );
            std::thread::yield_now();
        }
        assert!(
            repaint_count.load(Ordering::Relaxed) > 0,
            "ReadingBack owns the wake even without pointer events"
        );
        assert_eq!(progressing.depth_bytes(), u64::from(size[0] * size[1]) * 2);
        // Another key can complete in a discard/retry pass of that same frame.
        // Its reply stays in Estimating until the cumulative frame advances.
        let second_key = DepthSourceKey {
            texture_id: egui::TextureId::Managed(202),
        };
        progressing
            .select_source(&gpu.device, &gpu.queue, &source, second_key)
            .unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        while matches!(progressing.source, DepthSourcePhase::ReadingBack { .. }) {
            assert!(
                std::time::Instant::now() < deadline,
                "second readback must complete"
            );
            progressing.advance(
                &gpu.device,
                &gpu.queue,
                &egui,
                egui::ViewportId::ROOT,
                |key, _, _| {
                    Ok(crate::depth_estimation::DepthPending::completed_for_test(
                        owner,
                        502,
                        map(key, 0.5),
                    ))
                },
            );
            std::thread::yield_now();
        }
        assert!(
            matches!(&progressing.source,DepthSourcePhase::Estimating{key,pending} if *key==second_key && pending.request_id()==502)
        );
        let before_repaint = repaint_count.load(Ordering::Relaxed);
        progressing.advance(
            &gpu.device,
            &gpu.queue,
            &egui,
            egui::ViewportId::ROOT,
            |_, _, _| panic!("already estimating"),
        );
        assert!(
            repaint_count.load(Ordering::Relaxed) > before_repaint,
            "deferred upload requests the next visible frame"
        );
        assert_eq!(
            progressing.depth_bytes(),
            u64::from(size[0] * size[1]) * 2,
            "at most one R16 upload per cumulative frame"
        );
        assert!(progressing.ready().is_none());
        egui.run_pass(Default::default(), |_| {})
            .drop_without_applying_deltas();
        progressing.advance(
            &gpu.device,
            &gpu.queue,
            &egui,
            egui::ViewportId::ROOT,
            |_, _, _| panic!("reply must remain in its original receiver"),
        );
        assert!(matches!(progressing.source,DepthSourcePhase::Ready{key} if key==second_key));
        assert_eq!(
            progressing.depth_bytes(),
            u64::from(size[0] * size[1]) * 2 * 2
        );
        let mut failed = DepthViewState::new(owner);
        failed.source = DepthSourcePhase::Failed {
            key,
            reason: "failed key".to_owned(),
        };
        failed.wait_for_source(crate::depth_parallax::DepthSourceWait::OriginalHold);
        failed
            .select_source(&gpu.device, &gpu.queue, &source, key)
            .unwrap();
        assert!(matches!(failed.source, DepthSourcePhase::Failed { .. }));
        assert_eq!(
            failed.resident_bytes(),
            0,
            "same key must not restart GPU readback"
        );
        let renderer = Arc::new(egui::epaint::mutex::RwLock::new(gpu.renderer));
        state.attach_renderer_handle(&renderer);
        state.publish(&mut renderer.write().callback_resources);
        let mut other = DepthViewState::new(sibling);
        other.attach_renderer_handle(&renderer);
        drop(other);
        assert!(
            renderer
                .read()
                .callback_resources
                .get::<DepthRenderResourcesRef>()
                .is_some(),
            "sibling terminal close must not clear owner"
        );
        drop(state);
        assert!(
            renderer
                .read()
                .callback_resources
                .get::<DepthRenderResourcesRef>()
                .is_none(),
            "terminal close clears callback Arc without App"
        );
    }
}
