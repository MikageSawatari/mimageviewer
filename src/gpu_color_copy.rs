//! Shared GPU-only color copy and mip generation for Depth and PlaneView.
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
pub(crate) struct MipColor {
    // Keep the mip allocation owned until the material/callback releases it.
    pub _texture: wgpu::Texture,
    pub view: wgpu::TextureView,
    pub width: u32,
    pub height: u32,
}
impl MipColor {
    pub(crate) fn byte_size(&self) -> u64 {
        color_byte_size(self.width, self.height)
    }
}
pub(crate) struct ColorMipCopier {
    layout: wgpu::BindGroupLayout,
    pipeline: wgpu::RenderPipeline,
    sampler: wgpu::Sampler,
    mips: egui_wgpu::MipmapGenerator,
}
impl ColorMipCopier {
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
    pub(crate) fn blit(
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
    ) -> Result<MipColor, String> {
        if source.format() != wgpu::TextureFormat::Rgba8Unorm
            || source.width() == 0
            || source.height() == 0
        {
            return Err("表示の元テクスチャ形式が不正です".into());
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
        Ok(MipColor {
            _texture: texture,
            view,
            width,
            height,
        })
    }
}
