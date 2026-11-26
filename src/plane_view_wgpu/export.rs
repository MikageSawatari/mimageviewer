//! Only workers (or headless GPU tests) may submit and wait for output here.
use super::*;

#[derive(Clone, Copy, Debug)]
pub(crate) enum PlaneOutputChoice {
    Canvas,
    Export(crate::export_dialog::ExportScale),
}

pub(crate) fn resolve_plane_output_size(
    canvas: Vec2,
    choice: PlaneOutputChoice,
    limits: &wgpu::Limits,
) -> [u32; 2] {
    let factor = match choice {
        PlaneOutputChoice::Canvas => 1.,
        PlaneOutputChoice::Export(_) => 2.,
    };
    let base = [
        (canvas.x.max(1.) * factor).round() as usize,
        (canvas.y.max(1.) * factor).round() as usize,
    ];
    let desired = match choice {
        PlaneOutputChoice::Canvas => base,
        PlaneOutputChoice::Export(scale) => scale.scaled_size(base),
    };
    let max = limits.max_texture_dimension_2d.max(1) as f64;
    let mut scale = (max / desired[0] as f64)
        .min(max / desired[1] as f64)
        .min(1.);
    let size_at = |s: f64| {
        [
            (desired[0] as f64 * s).floor().max(1.) as u32,
            (desired[1] as f64 * s).floor().max(1.) as u32,
        ]
    };
    let fits = |s: [u32; 2]| {
        u64::from(s[0] * 4).div_ceil(256) * 256 * u64::from(s[1]) <= limits.max_buffer_size
    };
    if !fits(size_at(scale)) {
        // Monotone binary search accounts for the row padding exactly.
        let mut low = 0.;
        let mut high = scale;
        for _ in 0..48 {
            let mid = (low + high) * 0.5;
            if fits(size_at(mid)) {
                low = mid;
            } else {
                high = mid;
            }
        }
        scale = low;
    }
    size_at(scale)
}

pub(crate) struct PlaneFrameRenderer {
    request: PlaneFrameRequest,
    uniform: PlaneUniform,
    target: wgpu::Texture,
    buffer: wgpu::Buffer,
    stride: u32,
}
impl PlaneFrameRenderer {
    pub(crate) fn new(request: &PlaneFrameRequest) -> Self {
        let request = request.clone();
        let device = &request.device;
        let uniform = PlaneUniform::new(
            device,
            &request.pipeline,
            request.snapshot.params.for_output(request.size),
        );
        let target = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("plane export"),
            size: wgpu::Extent3d {
                width: request.size[0],
                height: request.size[1],
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let stride = (request.size[0] * 4).div_ceil(256) * 256;
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("plane export readback"),
            size: u64::from(stride) * u64::from(request.size[1]),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        Self {
            request,
            uniform,
            target,
            buffer,
            stride,
        }
    }
    pub(crate) fn render(&self) -> Result<Vec<u8>, String> {
        let started = crate::perf::is_enabled().then(std::time::Instant::now);
        let device = &self.request.device;
        let queue = &self.request.queue;
        let mut encoder = device.create_command_encoder(&Default::default());
        let view = self.target.create_view(&Default::default());
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("plane export camera"),
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
            pass.set_pipeline(&self.request.pipeline.pipeline);
            pass.set_bind_group(0, &self.request.snapshot.material.copy.gpu.bind, &[]);
            pass.set_bind_group(1, &self.uniform.bind, &[]);
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
                    rows_per_image: Some(self.request.size[1]),
                },
            },
            self.target.size(),
        );
        let submission = queue.submit(Some(encoder.finish()));
        if let Some(started) = started {
            crate::perf::event(
                "plane_view",
                "export_draw",
                None,
                0,
                &[("ms", (started.elapsed().as_secs_f64() * 1000.).into())],
            );
        }
        let started = crate::perf::is_enabled().then(std::time::Instant::now);
        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        self.buffer
            .slice(..)
            .map_async(wgpu::MapMode::Read, move |result| {
                let _ = tx.send(result);
            });
        device
            .poll(wgpu::PollType::Wait {
                submission_index: Some(submission),
                timeout: None,
            })
            .map_err(|e| e.to_string())?;
        rx.recv()
            .map_err(|e| e.to_string())?
            .map_err(|e| e.to_string())?;
        let mapped = self
            .buffer
            .slice(..)
            .get_mapped_range()
            .map_err(|e| e.to_string())?;
        let mut rgba =
            Vec::with_capacity(self.request.size[0] as usize * self.request.size[1] as usize * 4);
        for row in mapped.chunks_exact(self.stride as usize) {
            rgba.extend_from_slice(&row[..self.request.size[0] as usize * 4]);
        }
        drop(mapped);
        self.buffer.unmap();
        if let Some(started) = started {
            crate::perf::event(
                "plane_view",
                "readback",
                None,
                0,
                &[("ms", (started.elapsed().as_secs_f64() * 1000.).into())],
            );
        }
        Ok(rgba)
    }
}
