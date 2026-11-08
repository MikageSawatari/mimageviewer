//! Worker-owned render resources. The screen callback's uniform is never written here.
use super::*;
use crate::depth_export::DepthExportUnderlay;

const BACKGROUND: &str = r#"
struct Vertex { @builtin(position) position:vec4<f32>, @location(0) uv:vec2<f32> };
@vertex fn vs_main(@builtin(vertex_index) i:u32)->Vertex {
    var p=array<vec2<f32>,3>(vec2(-1.,-1.),vec2(3.,-1.),vec2(-1.,3.));
    var o:Vertex;o.position=vec4(p[i],0.,1.);o.uv=vec2((p[i].x+1.)*.5,(1.-p[i].y)*.5);return o;
}
struct Background { size_cell:vec4<f32>, color_kind:vec4<f32> };
@group(0) @binding(0) var<uniform> bg:Background;
@fragment fn fs_main(in:Vertex)->@location(0) vec4<f32> {
    if bg.color_kind.w<.5 {return vec4(bg.color_kind.xyz,1.);}
    // paint_quad uses the displayed rectangle: quarter-turns rotate the photo
    // samples, while the underlay starts at the displayed image's top left.
    let p=in.uv*bg.size_cell.xy/bg.size_cell.zw;
    let odd=(u32(floor(p.x))+u32(floor(p.y)))%2u;
    let v=select(224./255.,176./255.,odd==1u);
    return vec4(v,v,v,1.);
}
"#;

pub(crate) struct DepthFrameRenderer {
    device: wgpu::Device,
    queue: wgpu::Queue,
    resources: DepthRenderResources,
    pipeline: wgpu::RenderPipeline,
    background_pipeline: wgpu::RenderPipeline,
    background_bind: wgpu::BindGroup,
    target: wgpu::Texture,
    buffer: wgpu::Buffer,
    size: [u32; 2],
    stride: u32,
}

impl DepthFrameRenderer {
    pub(crate) fn new(
        device: wgpu::Device,
        queue: wgpu::Queue,
        source: &DepthRenderResources,
        size: [u32; 2],
        underlay: DepthExportUnderlay,
    ) -> Self {
        use wgpu::util::DeviceExt;
        let resources = DepthRenderResources::new(
            &device,
            source.context,
            source.key,
            source.color.clone(),
            source.depth.clone(),
        );
        let depth_pipeline = pipeline(
            &device,
            &render_layout(&device),
            DEPTH_SHADER,
            wgpu::TextureFormat::Rgba8Unorm,
            Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
        );
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("depth export background layout"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let (rgb, cell, kind) = match underlay {
            DepthExportUnderlay::Solid(rgb) => (rgb, [1., 1.], 0.),
            DepthExportUnderlay::Checker { cell_size } => ([0; 3], cell_size, 1.),
        };
        let values = [
            size[0] as f32,
            size[1] as f32,
            cell[0],
            cell[1],
            rgb[0] as f32 / 255.,
            rgb[1] as f32 / 255.,
            rgb[2] as f32 / 255.,
            kind,
        ];
        let bytes: Vec<u8> = values.into_iter().flat_map(f32::to_le_bytes).collect();
        let background_uniform = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("depth export frozen background"),
            contents: &bytes,
            usage: wgpu::BufferUsages::UNIFORM,
        });
        let background_bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("depth export background"),
            layout: &layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: background_uniform.as_entire_binding(),
            }],
        });
        let background_pipeline = pipeline(
            &device,
            &layout,
            BACKGROUND,
            wgpu::TextureFormat::Rgba8Unorm,
            None,
        );
        let target = texture(
            &device,
            size[0],
            size[1],
            wgpu::TextureFormat::Rgba8Unorm,
            1,
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        );
        let stride = (size[0] * 4).div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT)
            * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("depth export frame readback"),
            size: u64::from(stride) * u64::from(size[1]),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        Self {
            device,
            queue,
            resources,
            pipeline: depth_pipeline,
            background_pipeline,
            background_bind,
            target,
            buffer,
            size,
            stride,
        }
    }

    /// Called only by the export thread (and headless GPU tests).
    pub(crate) fn render(&self, params: DepthRenderParams) -> Result<Vec<u8>, String> {
        self.queue
            .write_buffer(&self.resources.uniform, 0, &params.bytes());
        let mut encoder = self.device.create_command_encoder(&Default::default());
        let view = self.target.create_view(&Default::default());
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("depth export opaque frame"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                ..Default::default()
            });
            pass.set_pipeline(&self.background_pipeline);
            pass.set_bind_group(0, &self.background_bind, &[]);
            pass.draw(0..3, 0..1);
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &self.resources.bind_group, &[]);
            pass.draw(0..3, 0..1);
        }
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &self.target,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &self.buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(self.stride),
                    rows_per_image: Some(self.size[1]),
                },
            },
            self.target.size(),
        );
        let submission = self.queue.submit(Some(encoder.finish()));
        let (tx, rx) = mpsc::sync_channel(1);
        self.buffer
            .slice(..)
            .map_async(wgpu::MapMode::Read, move |result| {
                let _ = tx.send(result);
            });
        self.device
            .poll(wgpu::PollType::Wait {
                submission_index: Some(submission),
                timeout: None,
            })
            .map_err(|error| format!("GPU の書き出し待機に失敗しました: {error}"))?;
        rx.recv()
            .map_err(|error| error.to_string())?
            .map_err(|error| error.to_string())?;
        let mapped = self
            .buffer
            .slice(..)
            .get_mapped_range()
            .map_err(|error| error.to_string())?;
        let mut rgba = Vec::with_capacity(self.size[0] as usize * self.size[1] as usize * 4);
        for row in mapped.chunks_exact(self.stride as usize) {
            rgba.extend_from_slice(&row[..self.size[0] as usize * 4]);
        }
        drop(mapped);
        self.buffer.unmap();
        Ok(rgba)
    }
}
