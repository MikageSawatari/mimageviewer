//! Windowless GPU assertions for the actual product paint callbacks.
//! No application, surface, fonts, profile, or desktop input is created here.

pub(crate) const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

pub(crate) struct OffscreenGpu {
    pub(crate) device: wgpu::Device,
    pub(crate) queue: wgpu::Queue,
    pub(crate) renderer: egui_wgpu::Renderer,
}

impl OffscreenGpu {
    pub(crate) fn new() -> Self {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            // Match the existing vendor GPU fixture; avoid Windows WGL initialization.
            backends: if cfg!(target_os = "windows") {
                wgpu::Backends::DX12
            } else {
                wgpu::Backends::PRIMARY
            },
            ..wgpu::InstanceDescriptor::new_without_display_handle()
        });
        let adapter = pollster::block_on(instance.request_adapter(&Default::default()))
            .expect("stage4 callback GPU prerequisite: no offscreen adapter (not a pass/skip)");
        let (device, queue) =
            pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default()))
                .expect("stage4 callback GPU prerequisite: create device");
        let renderer =
            egui_wgpu::Renderer::new(&device, FORMAT, egui_wgpu::RendererOptions::default());
        Self {
            device,
            queue,
            renderer,
        }
    }

    /// Run prepare/finish_prepare/paint through the real renderer, then read raw RGBA bytes.
    pub(crate) fn render(
        &mut self,
        jobs: &[egui::ClippedPrimitive],
        size: [u32; 2],
        clear: [u8; 4],
    ) -> Vec<[u8; 4]> {
        let target = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("stage4 product callback output"),
            size: wgpu::Extent3d {
                width: size[0],
                height: size[1],
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let screen = egui_wgpu::ScreenDescriptor {
            size_in_pixels: size,
            pixels_per_point: 1.0,
        };
        let mut encoder = self.device.create_command_encoder(&Default::default());
        let mut submissions =
            self.renderer
                .update_buffers(&self.device, &self.queue, &mut encoder, jobs, &screen);
        let view = target.create_view(&Default::default());
        {
            let mut pass = encoder
                .begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("stage4 product callback pass"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &view,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color {
                                r: f64::from(clear[0]) / 255.0,
                                g: f64::from(clear[1]) / 255.0,
                                b: f64::from(clear[2]) / 255.0,
                                a: f64::from(clear[3]) / 255.0,
                            }),
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    ..Default::default()
                })
                .forget_lifetime();
            self.renderer.render(&mut pass, jobs, &screen);
        }
        let stride = (size[0] * 4).div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT)
            * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
        let buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("stage4 product callback readback"),
            size: u64::from(stride * size[1]),
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &target,
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
            wgpu::Extent3d {
                width: size[0],
                height: size[1],
                depth_or_array_layers: 1,
            },
        );
        submissions.push(encoder.finish());
        self.queue.submit(submissions);
        let (sender, receiver) = std::sync::mpsc::channel();
        buffer
            .slice(..)
            .map_async(wgpu::MapMode::Read, move |result| {
                sender.send(result).expect("stage4 readback receiver");
            });
        self.device
            .poll(wgpu::PollType::Wait {
                submission_index: None,
                timeout: Some(std::time::Duration::from_secs(30)),
            })
            .expect("stage4 readback GPU completion");
        receiver
            .recv_timeout(std::time::Duration::from_secs(30))
            .expect("stage4 readback callback")
            .expect("stage4 readback mapping");
        let mapped = buffer
            .slice(..)
            .get_mapped_range()
            .expect("stage4 mapped bytes");
        let pixels = mapped
            .chunks_exact(stride as usize)
            .flat_map(|row| row[..size[0] as usize * 4].chunks_exact(4))
            .map(|pixel| pixel.try_into().unwrap())
            .collect();
        drop(mapped);
        buffer.unmap();
        pixels
    }
}

pub(crate) fn callback(
    rect: egui::Rect,
    callback: impl egui_wgpu::CallbackTrait + 'static,
) -> egui::ClippedPrimitive {
    egui::ClippedPrimitive {
        clip_rect: rect,
        primitive: egui::epaint::Primitive::Callback(egui_wgpu::Callback::new_paint_callback(
            rect, callback,
        )),
    }
}

pub(crate) fn assert_rgba(actual: [u8; 4], expected: [u8; 4]) {
    for channel in 0..4 {
        assert!(
            actual[channel].abs_diff(expected[channel]) <= 1,
            "RGBA readback: actual={actual:?}, expected={expected:?}, channel={channel}"
        );
    }
}
